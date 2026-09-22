use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterDeviceRequest {
    pub name: String,
    pub os: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Serialize)]
pub struct DeviceResponse {
    pub id: String,
    pub name: String,
    pub os: Option<String>,
    pub hostname: Option<String>,
    pub created_at: String,
}

pub async fn register_device(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RegisterDeviceRequest>,
) -> Result<(StatusCode, Json<DeviceResponse>), StatusCode> {
    let device_id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO devices (id, user_id, name, os, hostname) VALUES (?, ?, ?, ?, ?)")
        .bind(&device_id)
        .bind(&auth.user_id)
        .bind(&req.name)
        .bind(&req.os)
        .bind(&req.hostname)
        .execute(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    debug!(device_id = %device_id, user_id = %auth.user_id, name = %req.name, "device registered");

    Ok((
        StatusCode::CREATED,
        Json(DeviceResponse {
            id: device_id,
            name: req.name,
            os: req.os,
            hostname: req.hostname,
            created_at: chrono::Utc::now().to_rfc3339(),
        }),
    ))
}

pub async fn list_devices(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceResponse>>, StatusCode> {
    let devices = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, String)>(
        "SELECT id, name, os, hostname, created_at FROM devices WHERE user_id = ? ORDER BY created_at DESC"
    )
        .bind(&auth.user_id)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let response: Vec<DeviceResponse> = devices
        .into_iter()
        .map(|(id, name, os, hostname, created_at)| DeviceResponse {
            id,
            name,
            os,
            hostname,
            created_at,
        })
        .collect();

    Ok(Json(response))
}

pub async fn delete_device(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> StatusCode {
    let _lifecycle = state.lifecycle.write().await;
    let result = sqlx::query("DELETE FROM devices WHERE id = ? AND user_id = ?")
        .bind(&device_id)
        .bind(&auth.user_id)
        .execute(&state.db.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let _admission = state.quic_router.admission.lock().await;
            for session_id in state.sessions.remove_device(&auth.user_id, &device_id) {
                state.tickets.revoke_session(&session_id);
                state.quic_router.forget_session(&session_id).await;
            }
            debug!(device_id = %device_id, "device deleted");
            StatusCode::NO_CONTENT
        }
        Ok(_) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::DaemonCommand;
    use std::time::Duration;

    #[tokio::test]
    async fn a_blocked_device_deletion_does_not_block_another_terminal() {
        let state = crate::state::test_state().await;
        state.sessions.register_device("sibling", "user", "sibling");
        let access = state.sessions.device_access("sibling").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .quic_router
            .register_daemon_input("sibling-session", tx)
            .await;
        let mut connections = Vec::new();
        for _ in 0..5 {
            connections.push(state.db.pool.acquire().await.unwrap());
        }
        let task_state = state.clone();
        let deletion = tokio::spawn(async move {
            delete_device(
                AuthUser {
                    user_id: "user".into(),
                },
                State(task_state),
                Path("deleted".into()),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.lifecycle.try_read().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(
            Duration::from_millis(100),
            state.quic_router.command_authorized(
                "sibling-session",
                &access,
                &state.sessions,
                DaemonCommand::Input(b"still-live".to_vec()),
            ),
        )
        .await
        .expect("database contention must not stall existing terminal bytes");
        assert!(
            matches!(rx.recv().await, Some(DaemonCommand::Input(data)) if data == b"still-live")
        );
        deletion.abort();
        let _ = deletion.await;
        drop(connections);
    }
}
