//! Agent notifications: daemon → APNs alert pushes, and phone answers back to
//! the daemon that asked.

use crate::apns::{AlertPush, PushDisposition, display_project_name};
use crate::state::AppState;
use fil_protocol::agent;
use fil_protocol::proto;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Per account. Generous for real use, tight enough that a looping agent
/// cannot turn the hub into a pager flood or burn the APNs budget.
const RATE_LIMIT: usize = 20;
const RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_PENDING_PER_USER: usize = 32;
const MAX_ID_LEN: usize = 64;

struct PendingAsk {
    user_id: String,
    device_id: String,
    options: Vec<String>,
    expires_at: i64,
}

#[derive(Debug, PartialEq)]
pub enum AnswerError {
    /// Unknown, foreign, answered or expired. Indistinguishable on purpose.
    NotFound,
    InvalidOption,
    DaemonOffline,
}

#[derive(Default)]
pub struct AgentRelay {
    daemons: Mutex<HashMap<String, (u64, mpsc::Sender<proto::HubMessage>)>>,
    pending: Mutex<HashMap<String, PendingAsk>>,
    recent: Mutex<HashMap<String, VecDeque<Instant>>>,
    next_connection: AtomicU64,
}

impl AgentRelay {
    /// The newest control socket of a device receives its answers.
    pub fn attach_daemon(&self, device_id: &str, sender: mpsc::Sender<proto::HubMessage>) -> u64 {
        let connection = self.next_connection.fetch_add(1, Ordering::Relaxed);
        self.daemons
            .lock()
            .unwrap()
            .insert(device_id.to_string(), (connection, sender));
        connection
    }

    pub fn detach_daemon(&self, device_id: &str, connection: u64) {
        let mut daemons = self.daemons.lock().unwrap();
        if daemons
            .get(device_id)
            .is_some_and(|(current, _)| *current == connection)
        {
            daemons.remove(device_id);
        }
    }

    fn send_to_daemon(&self, device_id: &str, message: proto::HubMessage) -> bool {
        self.daemons
            .lock()
            .unwrap()
            .get(device_id)
            .is_some_and(|(_, sender)| sender.try_send(message).is_ok())
    }

    fn allow(&self, user_id: &str) -> bool {
        let now = Instant::now();
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|_, sent| {
            sent.retain(|at| now.duration_since(*at) < RATE_WINDOW);
            !sent.is_empty()
        });
        let sent = recent.entry(user_id.to_string()).or_default();
        if sent.len() >= RATE_LIMIT {
            return false;
        }
        sent.push_back(now);
        true
    }

    fn insert_pending(&self, id: &str, ask: PendingAsk) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, ask| ask.expires_at > now);
        if pending.contains_key(id) {
            return Err("duplicate notification id".into());
        }
        if pending
            .values()
            .filter(|p| p.user_id == ask.user_id)
            .count()
            >= MAX_PENDING_PER_USER
        {
            return Err("too many unanswered questions".into());
        }
        pending.insert(id.to_string(), ask);
        Ok(())
    }

    fn close(&self, device_id: &str, id: &str) {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .get(id)
            .is_some_and(|ask| ask.device_id == device_id)
        {
            pending.remove(id);
        }
    }

    pub fn answer(&self, user_id: &str, id: &str, answer: &str) -> Result<(), AnswerError> {
        let mut pending = self.pending.lock().unwrap();
        let ask = pending
            .get(id)
            .filter(|ask| ask.user_id == user_id)
            .ok_or(AnswerError::NotFound)?;
        if ask.expires_at <= chrono::Utc::now().timestamp() {
            pending.remove(id);
            return Err(AnswerError::NotFound);
        }
        if !ask.options.iter().any(|option| option == answer) {
            return Err(AnswerError::InvalidOption);
        }
        let message = proto::HubMessage {
            payload: Some(proto::hub_message::Payload::AgentAnswer(
                proto::AgentAnswer {
                    notification_id: id.to_string(),
                    answer: answer.to_string(),
                },
            )),
        };
        // Keep the question open if the Mac is unreachable, so a retry works.
        if !self.send_to_daemon(&ask.device_id, message) {
            return Err(AnswerError::DaemonOffline);
        }
        pending.remove(id);
        Ok(())
    }

    pub fn remove_user(&self, user_id: &str) {
        self.pending
            .lock()
            .unwrap()
            .retain(|_, ask| ask.user_id != user_id);
        self.recent.lock().unwrap().remove(user_id);
    }
}

pub fn handle_ask_closed(state: &AppState, device_id: &str, closed: &proto::AgentAskClosed) {
    state.agent.close(device_id, &closed.notification_id);
}

/// Runs off the control socket's read loop: APNs round trips must not stall
/// heartbeats or session updates.
pub async fn handle_notification(
    state: AppState,
    device_id: String,
    user_id: String,
    notification: proto::AgentNotification,
) {
    let id = notification.notification_id.clone();
    let (delivered, error) = match deliver(&state, &device_id, &user_id, notification).await {
        Ok(delivered) => (delivered, String::new()),
        Err(error) => (0, error),
    };
    if delivered == 0 {
        debug!(%device_id, %error, "agent notification not delivered");
    }
    let ack = proto::HubMessage {
        payload: Some(proto::hub_message::Payload::AgentNotificationAck(
            proto::AgentNotificationAck {
                notification_id: id,
                delivered,
                error,
            },
        )),
    };
    state.agent.send_to_daemon(&device_id, ack);
}

async fn deliver(
    state: &AppState,
    device_id: &str,
    user_id: &str,
    notification: proto::AgentNotification,
) -> Result<u32, String> {
    let id = notification.notification_id.as_str();
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err("invalid notification id".into());
    }
    agent::validate_notification(
        &notification.title,
        &notification.body,
        &notification.options,
    )?;
    let is_question = !notification.options.is_empty();
    let now = chrono::Utc::now().timestamp();
    if is_question
        && (notification.expires_at <= now
            || notification.expires_at > now + agent::MAX_ASK_TIMEOUT_SECS as i64 + 60)
    {
        return Err("invalid question expiry".into());
    }
    if !state.apns.is_enabled() {
        return Err("push notifications are not configured on this hub".into());
    }
    if !state.agent.allow(user_id) {
        return Err("too many notifications in the last minute; slow down".into());
    }

    let tokens = push_tokens_for_user(&state.db.pool, user_id)
        .await
        .map_err(|_| "could not load push tokens".to_string())?;
    if tokens.is_empty() {
        return Err(
            "no iPhone has enabled Fil notifications yet (open the Fil app and allow notifications)"
                .into(),
        );
    }

    // Only deep link to a session this very device reported.
    let devices = state.sessions.get_user_sessions(user_id);
    let device = devices.iter().find(|device| device.device_id == device_id);
    let session = device.and_then(|device| {
        device
            .sessions
            .iter()
            .find(|session| session.session_id == notification.session_id)
    });
    let device_name = device.map(|d| d.device_name.as_str()).unwrap_or_default();
    let subtitle = match session {
        Some(session) => format!("{device_name} · {}", display_project_name(&session.cwd)),
        None => device_name.to_string(),
    };

    if is_question {
        state.agent.insert_pending(
            id,
            PendingAsk {
                user_id: user_id.to_string(),
                device_id: device_id.to_string(),
                options: notification.options.clone(),
                expires_at: notification.expires_at,
            },
        )?;
    }

    let push = AlertPush {
        notification_id: id.to_string(),
        title: if notification.title.trim().is_empty() {
            if is_question { "Question" } else { "Agent" }.to_string()
        } else {
            notification.title.clone()
        },
        subtitle,
        body: notification.body.clone(),
        session_id: session.map(|session| session.session_id.clone()),
        options: notification.options.clone(),
        expires_at: is_question.then_some(notification.expires_at),
        collapse_id: id.to_string(),
        expiration: is_question.then_some(notification.expires_at),
    };

    let mut delivered = 0;
    for (token, environment) in tokens {
        match state.apns.send_alert(&token, &environment, &push).await {
            Ok(PushDisposition::Delivered) => delivered += 1,
            Ok(PushDisposition::InvalidToken) => {
                let _ = sqlx::query("DELETE FROM push_tokens WHERE token = ? AND user_id = ?")
                    .bind(&token)
                    .bind(user_id)
                    .execute(&state.db.pool)
                    .await;
            }
            Ok(PushDisposition::Retryable) => {}
            Err(error) => warn!(error = %format!("{error:#}"), "APNs alert push failed"),
        }
    }
    if delivered == 0 {
        if is_question {
            state.agent.close(device_id, id);
        }
        return Err("Apple Push rejected the notification for every registered iPhone".into());
    }
    info!(%device_id, delivered, question = is_question, "agent notification sent");
    Ok(delivered)
}

async fn push_tokens_for_user(
    pool: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT token, environment FROM push_tokens WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(user: &str, device: &str) -> PendingAsk {
        PendingAsk {
            user_id: user.into(),
            device_id: device.into(),
            options: vec!["Yes".into(), "No".into()],
            expires_at: chrono::Utc::now().timestamp() + 60,
        }
    }

    #[tokio::test]
    async fn answers_reach_only_the_asking_device_of_the_same_user() {
        let relay = AgentRelay::default();
        let (tx, mut rx) = mpsc::channel(4);
        relay.attach_daemon("mac", tx);
        relay.insert_pending("q1", ask("alice", "mac")).unwrap();

        assert_eq!(
            relay.answer("mallory", "q1", "Yes"),
            Err(AnswerError::NotFound)
        );
        assert_eq!(
            relay.answer("alice", "q1", "Maybe"),
            Err(AnswerError::InvalidOption)
        );
        assert_eq!(relay.answer("alice", "q1", "No"), Ok(()));
        assert_eq!(
            relay.answer("alice", "q1", "Yes"),
            Err(AnswerError::NotFound)
        );
        match rx.recv().await.unwrap().payload {
            Some(proto::hub_message::Payload::AgentAnswer(answer)) => {
                assert_eq!(
                    (answer.notification_id.as_str(), answer.answer.as_str()),
                    ("q1", "No")
                )
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn offline_daemon_keeps_the_question_open() {
        let relay = AgentRelay::default();
        relay.insert_pending("q1", ask("alice", "mac")).unwrap();
        assert_eq!(
            relay.answer("alice", "q1", "Yes"),
            Err(AnswerError::DaemonOffline)
        );
        let (tx, _rx) = mpsc::channel(4);
        relay.attach_daemon("mac", tx);
        assert_eq!(relay.answer("alice", "q1", "Yes"), Ok(()));
    }

    #[test]
    fn stale_connection_cannot_detach_its_replacement() {
        let relay = AgentRelay::default();
        let (old, _old_rx) = mpsc::channel(4);
        let (new, _new_rx) = mpsc::channel(4);
        let old_id = relay.attach_daemon("mac", old);
        relay.attach_daemon("mac", new);
        relay.detach_daemon("mac", old_id);
        relay.insert_pending("q1", ask("alice", "mac")).unwrap();
        assert_eq!(relay.answer("alice", "q1", "Yes"), Ok(()));
    }

    #[test]
    fn closing_and_expiry_remove_questions() {
        let relay = AgentRelay::default();
        let (tx, _rx) = mpsc::channel(4);
        relay.attach_daemon("mac", tx);
        relay.insert_pending("q1", ask("alice", "mac")).unwrap();
        relay.close("other-mac", "q1");
        relay.close("mac", "q1");
        assert_eq!(
            relay.answer("alice", "q1", "Yes"),
            Err(AnswerError::NotFound)
        );

        let mut expired = ask("alice", "mac");
        expired.expires_at = chrono::Utc::now().timestamp() - 1;
        relay.insert_pending("q2", expired).unwrap();
        assert_eq!(
            relay.answer("alice", "q2", "Yes"),
            Err(AnswerError::NotFound)
        );
    }

    #[test]
    fn rate_limit_is_per_account() {
        let relay = AgentRelay::default();
        for _ in 0..RATE_LIMIT {
            assert!(relay.allow("alice"));
        }
        assert!(!relay.allow("alice"));
        assert!(relay.allow("bob"));
    }

    #[tokio::test]
    async fn notification_without_apns_is_acknowledged_as_undelivered() {
        let state = crate::state::test_state().await;
        let (tx, mut rx) = mpsc::channel(4);
        state.agent.attach_daemon("mac", tx);
        handle_notification(
            state.clone(),
            "mac".into(),
            "alice".into(),
            proto::AgentNotification {
                notification_id: "n1".into(),
                title: "Done".into(),
                ..Default::default()
            },
        )
        .await;
        match rx.recv().await.unwrap().payload {
            Some(proto::hub_message::Payload::AgentNotificationAck(ack)) => {
                assert_eq!(ack.notification_id, "n1");
                assert_eq!(ack.delivered, 0);
                assert!(ack.error.contains("not configured"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
