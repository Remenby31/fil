use crate::config::ApnsConfig;
use crate::sessions::{DeviceState, SessionInfo, SessionStatus, UserSessionUpdate};
use anyhow::{Context, Result};
use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

const LIVE_ACTIVITY_TOPIC_SUFFIX: &str = ".push-type.liveactivity";
const STALE_AFTER_SECONDS: i64 = 20;

#[derive(Clone)]
pub struct ApnsClient {
    config: Option<ApnsConfig>,
    http: Client,
    cached_auth: Arc<RwLock<Option<CachedAuthorization>>>,
}

#[derive(Clone)]
struct CachedAuthorization {
    token: String,
    issued_at: i64,
}

#[derive(Serialize)]
struct ProviderClaims<'a> {
    iss: &'a str,
    iat: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveActivityState {
    pub status: String,
    pub project_name: String,
    pub shell: String,
    pub other_session_count: usize,
    pub last_updated_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveActivityRegistration {
    pub activity_id: String,
    pub user_id: String,
    pub session_id: String,
    pub push_token: String,
    pub environment: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PushDisposition {
    Delivered,
    InvalidToken,
    Retryable,
}

impl ApnsClient {
    pub fn new(config: Option<ApnsConfig>) -> Result<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .http2_prior_knowledge()
            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
            .build()?;
        if config.is_some() {
            info!("ActivityKit APNs delivery enabled");
        } else {
            warn!("ActivityKit APNs delivery disabled: credentials are not configured");
        }
        Ok(Self {
            config,
            http,
            cached_auth: Arc::new(RwLock::new(None)),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub async fn deliver_update(&self, pool: &SqlitePool, update: &UserSessionUpdate) {
        let registrations = match registrations_for_user(pool, &update.user_id).await {
            Ok(registrations) => registrations,
            Err(error) => {
                warn!(%error, user_id = %update.user_id, "failed to load Live Activity registrations");
                return;
            }
        };

        for registration in registrations {
            let session = find_session(&update.devices, &registration.session_id);
            let (event, state) = match session {
                Some((device, session)) => (
                    "update",
                    state_for_session(device, session, &update.devices),
                ),
                None => (
                    "end",
                    LiveActivityState {
                        status: "ended".into(),
                        project_name: String::new(),
                        shell: String::new(),
                        other_session_count: 0,
                        last_updated_at: Utc::now().timestamp(),
                    },
                ),
            };

            let disposition = match self.send(&registration, event, &state).await {
                Ok(disposition) => disposition,
                Err(error) => {
                    warn!(
                        error = %format!("{error:#}"),
                        activity_id = %registration.activity_id,
                        "APNs Live Activity push failed"
                    );
                    continue;
                }
            };

            if (event == "end" || disposition == PushDisposition::InvalidToken)
                && let Err(error) =
                    delete_registration(pool, &registration.activity_id, &registration.user_id)
                        .await
            {
                warn!(%error, activity_id = %registration.activity_id, "failed to delete stale Live Activity registration");
            }
        }
    }

    async fn send(
        &self,
        registration: &LiveActivityRegistration,
        event: &str,
        state: &LiveActivityState,
    ) -> Result<PushDisposition> {
        let Some(config) = &self.config else {
            debug!(activity_id = %registration.activity_id, "skipping APNs delivery because it is disabled");
            return Ok(PushDisposition::Retryable);
        };
        let authorization = self.authorization(config).await?;
        let now = Utc::now().timestamp();
        let mut aps = json!({
            "timestamp": now,
            "event": event,
            "content-state": state,
            "stale-date": now + STALE_AFTER_SECONDS,
        });
        if event == "end" {
            aps["dismissal-date"] = json!(now + 8);
        }
        let endpoint = if registration.environment == "sandbox" {
            "https://api.sandbox.push.apple.com"
        } else {
            "https://api.push.apple.com"
        };
        let response = self
            .http
            .post(format!("{endpoint}/3/device/{}", registration.push_token))
            .header("authorization", format!("bearer {authorization}"))
            .header(
                "apns-topic",
                format!("{}{}", config.topic, LIVE_ACTIVITY_TOPIC_SUFFIX),
            )
            .header("apns-push-type", "liveactivity")
            .header("apns-priority", "10")
            .json(&json!({ "aps": aps }))
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            return Ok(PushDisposition::Delivered);
        }
        let body = response.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
            warn!(%status, %body, "APNs rejected an expired Live Activity token");
            Ok(PushDisposition::InvalidToken)
        } else {
            warn!(%status, %body, "APNs rejected a Live Activity push");
            Ok(PushDisposition::Retryable)
        }
    }

    async fn authorization(&self, config: &ApnsConfig) -> Result<String> {
        let now = Utc::now().timestamp();
        if let Some(cached) = self.cached_auth.read().await.as_ref()
            && now - cached.issued_at < 50 * 60
        {
            return Ok(cached.token.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(config.key_id.clone());
        let claims = ProviderClaims {
            iss: &config.team_id,
            iat: now as usize,
        };
        let key = EncodingKey::from_ec_pem(config.private_key.as_bytes())
            .context("invalid APNs ES256 private key")?;
        let token = encode(&header, &claims, &key)?;
        *self.cached_auth.write().await = Some(CachedAuthorization {
            token: token.clone(),
            issued_at: now,
        });
        Ok(token)
    }
}

fn find_session<'a>(
    devices: &'a [DeviceState],
    session_id: &str,
) -> Option<(&'a DeviceState, &'a SessionInfo)> {
    devices.iter().find_map(|device| {
        device
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| (device, session))
    })
}

fn state_for_session(
    device: &DeviceState,
    session: &SessionInfo,
    devices: &[DeviceState],
) -> LiveActivityState {
    let active_count = devices
        .iter()
        .flat_map(|device| &device.sessions)
        .filter(|session| session.status == SessionStatus::Online)
        .count();
    let project_name = display_project_name(&session.cwd);
    LiveActivityState {
        status: if device.connected && session.status == SessionStatus::Online {
            "connected".into()
        } else {
            "reconnecting".into()
        },
        project_name,
        shell: display_process_name(session),
        other_session_count: active_count.saturating_sub(1),
        last_updated_at: Utc::now().timestamp(),
    }
}

fn display_project_name(cwd: &str) -> String {
    let components = cwd
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.len() == 2 && matches!(components[0], "Users" | "home") {
        return "Home".into();
    }
    if cwd == "/" {
        return "Root".into();
    }
    components
        .last()
        .map(|name| name.trim_start_matches('~').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Terminal".into())
}

fn display_process_name(session: &SessionInfo) -> String {
    let raw = if session.command.trim().is_empty() {
        session.shell.as_str()
    } else {
        session.command.as_str()
    };
    let executable = raw
        .split_whitespace()
        .next()
        .unwrap_or(raw)
        .rsplit('/')
        .next()
        .unwrap_or(raw);
    match executable.to_ascii_lowercase().as_str() {
        name if name == "codex" || name.starts_with("codex-") => "Codex".into(),
        name if name == "claude" || name.starts_with("claude-") => "Claude Code".into(),
        "node" => "Node.js".into(),
        "zsh" | "bash" | "fish" | "sh" | "dash" => "Shell".into(),
        name if name.starts_with("python") => "Python".into(),
        _ => executable.to_string(),
    }
}

pub async fn registrations_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<LiveActivityRegistration>, sqlx::Error> {
    sqlx::query_as::<_, LiveActivityRegistration>(
        "SELECT activity_id, user_id, session_id, device_id, push_token, environment
         FROM live_activities WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn delete_registration(
    pool: &SqlitePool,
    activity_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_activities WHERE activity_id = ? AND user_id = ?")
        .bind(activity_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_exposes_only_the_final_path_component() {
        let session = SessionInfo {
            session_id: "s".into(),
            device_id: "d".into(),
            shell: "zsh".into(),
            command: "codex".into(),
            cwd: "/secret/client/project".into(),
            cols: 80,
            rows: 24,
            status: SessionStatus::Online,
            created_at: Utc::now(),
        };
        let device = DeviceState {
            device_id: "d".into(),
            device_name: "Mac".into(),
            user_id: "u".into(),
            sessions: vec![session.clone()],
            last_heartbeat: Utc::now(),
            connected: true,
        };
        let state = state_for_session(&device, &session, std::slice::from_ref(&device));
        assert_eq!(state.project_name, "project");
        assert_eq!(state.shell, "Codex");
        assert_eq!(display_project_name("/Users/baptistec"), "Home");
        assert_eq!(display_project_name("/"), "Root");
    }
}
