use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::sessions::{SessionInfo, SessionStatus};
use crate::state::AppState;
use fil_protocol::proto;

#[derive(Deserialize)]
pub struct WsParams {
    device_token: String,
    device_id: String,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Validate the device exists
    let device = sqlx::query_as::<_, (String, String, String)>(
        "SELECT d.id, d.user_id, d.name FROM devices d WHERE d.id = ?",
    )
    .bind(&params.device_id)
    .fetch_optional(&state.db.pool)
    .await;

    match device {
        Ok(Some((device_id, user_id, device_name))) => {
            info!(device_id = %device_id, user_id = %user_id, "WebSocket connection accepted");
            ws.on_upgrade(move |socket| handle_socket(socket, device_id, user_id, device_name, state))
        }
        _ => {
            warn!(device_id = %params.device_id, "WebSocket connection rejected: unknown device");
            axum::http::StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

async fn handle_socket(socket: WebSocket, device_id: String, user_id: String, device_name: String, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Register device as connected
    state.sessions.register_device(&device_id, &user_id, &device_name);
    info!(device_id = %device_id, "device connected");

    // Update last_seen
    let _ = sqlx::query("UPDATE devices SET last_seen = datetime('now') WHERE id = ?")
        .bind(&device_id)
        .execute(&state.db.pool)
        .await;

    // Process incoming messages from the daemon
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                if let Ok(daemon_msg) = proto::DaemonMessage::decode(data.as_ref()) {
                    handle_daemon_message(&daemon_msg, &device_id, &user_id, &state).await;
                }
            }
            Ok(Message::Close(_)) => {
                debug!(device_id = %device_id, "WebSocket closed by client");
                break;
            }
            Ok(Message::Ping(data)) => {
                if sender.send(Message::Pong(data)).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                debug!(device_id = %device_id, error = %e, "WebSocket error");
                break;
            }
            _ => {}
        }
    }

    // Mark device as disconnected
    state.sessions.set_device_connected(&device_id, false);
    info!(device_id = %device_id, "device disconnected");
}

async fn handle_daemon_message(
    msg: &proto::DaemonMessage,
    device_id: &str,
    _user_id: &str,
    state: &AppState,
) {
    let Some(payload) = &msg.payload else {
        return;
    };

    match payload {
        proto::daemon_message::Payload::SessionCreated(created) => {
            let session = SessionInfo {
                session_id: created.session_id.clone(),
                device_id: device_id.to_string(),
                shell: created.shell.clone(),
                cwd: created.cwd.clone(),
                cols: created.cols,
                rows: created.rows,
                status: SessionStatus::Online,
                created_at: chrono::Utc::now(),
            };
            state.sessions.add_session(device_id, session);
            debug!(
                session_id = %created.session_id,
                shell = %created.shell,
                "session created"
            );
        }
        proto::daemon_message::Payload::SessionDestroyed(destroyed) => {
            state
                .sessions
                .remove_session(device_id, &destroyed.session_id);
            debug!(session_id = %destroyed.session_id, "session destroyed");
        }
        proto::daemon_message::Payload::Heartbeat(heartbeat) => {
            let sessions: Vec<SessionInfo> = heartbeat
                .sessions
                .iter()
                .map(|s| SessionInfo {
                    session_id: s.session_id.clone(),
                    device_id: device_id.to_string(),
                    shell: s.shell.clone(),
                    cwd: s.cwd.clone(),
                    cols: s.cols,
                    rows: s.rows,
                    status: SessionStatus::Online,
                    created_at: chrono::Utc::now(),
                })
                .collect();
            state.sessions.update_heartbeat(device_id, sessions);
            debug!(
                device_id = %device_id,
                session_count = heartbeat.sessions.len(),
                "heartbeat received"
            );
        }
        proto::daemon_message::Payload::SessionData(data) => {
            // Phase 4: forward to connected iOS clients
            debug!(
                session_id = %data.session_id,
                bytes = data.data.len(),
                "session data received"
            );
        }
        proto::daemon_message::Payload::SessionResize(resize) => {
            debug!(
                session_id = %resize.session_id,
                cols = resize.cols,
                rows = resize.rows,
                "session resized"
            );
        }
        _ => {}
    }
}
