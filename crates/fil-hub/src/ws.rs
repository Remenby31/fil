use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::sessions::{SessionInfo, SessionStatus};
use crate::state::AppState;
use fil_protocol::proto;

#[derive(Deserialize)]
pub struct WsParams {
    device_id: String,
}

pub async fn ws_handler(
    auth: crate::auth::AuthUser,
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Validate the device exists
    let device = sqlx::query_as::<_, (String, String, String)>(
        "SELECT d.id, d.user_id, d.name FROM devices d WHERE d.id = ? AND d.user_id = ?",
    )
    .bind(&params.device_id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db.pool)
    .await;

    match device {
        Ok(Some((device_id, user_id, device_name))) => {
            info!(device_id = %device_id, user_id = %user_id, "WebSocket connection accepted");
            ws.max_message_size(256 * 1024)
                .max_frame_size(256 * 1024)
                .on_upgrade(move |socket| {
                    handle_socket(socket, device_id, user_id, device_name, state)
                })
        }
        _ => {
            warn!(device_id = %params.device_id, "WebSocket connection rejected: unknown device");
            axum::http::StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

async fn handle_socket(
    socket: WebSocket,
    device_id: String,
    user_id: String,
    device_name: String,
    state: AppState,
) {
    let (mut sender, mut receiver) = socket.split();

    let admission = state.quic_router.admission.lock().await;
    // An HTTP upgrade can finish after DELETE. Recheck under the same gate as
    // deletion before recreating any in-memory state.
    let exists =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM devices WHERE id = ? AND user_id = ?")
            .bind(&device_id)
            .bind(&user_id)
            .fetch_one(&state.db.pool)
            .await;
    if !matches!(exists, Ok(1)) {
        return;
    }

    // Register device as connected
    state
        .sessions
        .register_device(&device_id, &user_id, &device_name);
    let access = state
        .sessions
        .device_access(&device_id)
        .expect("registered device has access");
    drop(admission);
    info!(device_id = %device_id, "device connected");

    // Update last_seen
    let _ = sqlx::query("UPDATE devices SET last_seen = datetime('now') WHERE id = ?")
        .bind(&device_id)
        .execute(&state.db.pool)
        .await;

    // Process incoming messages from the daemon
    loop {
        let msg = tokio::select! {
            biased;
            _ = access.cancelled() => break,
            msg = receiver.next() => match msg { Some(msg) => msg, None => break },
        };
        match msg {
            Ok(Message::Binary(data)) => {
                if let Ok(daemon_msg) = proto::DaemonMessage::decode(data.as_ref()) {
                    let _admission = state.quic_router.admission.lock().await;
                    if access.is_cancelled() {
                        break;
                    }
                    handle_daemon_message(&daemon_msg, &device_id, &user_id, &state).await;
                }
            }
            Ok(Message::Close(_)) => {
                debug!(device_id = %device_id, "WebSocket closed by client");
                break;
            }
            Ok(Message::Ping(data)) if sender.send(Message::Pong(data.clone())).await.is_err() => {
                break;
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
                command: created.command.clone(),
                cwd: created.cwd.clone(),
                cols: created.cols,
                rows: created.rows,
                status: SessionStatus::Online,
                created_at: DateTime::from_timestamp(created.created_at, 0)
                    .unwrap_or_else(Utc::now),
            };
            state.sessions.add_session(device_id, session);
            debug!(
                session_id = %created.session_id,
                shell = %created.shell,
                "session created"
            );
        }
        proto::daemon_message::Payload::SessionDestroyed(destroyed) => {
            let removed = state
                .sessions
                .remove_session(device_id, &destroyed.session_id);
            if removed {
                state
                    .quic_router
                    .forget_session(&destroyed.session_id)
                    .await;
            }
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
                    command: s.command.clone(),
                    cwd: s.cwd.clone(),
                    cols: s.cols,
                    rows: s.rows,
                    status: SessionStatus::Online,
                    created_at: DateTime::from_timestamp(s.created_at, 0).unwrap_or_else(Utc::now),
                })
                .collect();
            let removed = state.sessions.update_heartbeat(device_id, sessions);
            for session_id in removed {
                state.quic_router.forget_session(&session_id).await;
            }
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
            state.sessions.update_device_session_size(
                device_id,
                &resize.session_id,
                resize.cols,
                resize.rows,
            );
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

#[cfg(test)]
mod security_tests {
    use super::*;

    async fn state() -> AppState {
        let state = crate::state::test_state().await;
        for (device, user, sid) in [
            ("victim-device", "victim-user", "victim"),
            ("other-device", "other-user", "other"),
        ] {
            state.sessions.register_device(device, user, device);
            state.sessions.add_session(
                device,
                SessionInfo {
                    session_id: sid.into(),
                    device_id: device.into(),
                    shell: "sh".into(),
                    command: String::new(),
                    cwd: "/tmp".into(),
                    cols: 80,
                    rows: 24,
                    status: SessionStatus::Online,
                    created_at: Utc::now(),
                },
            );
            state
                .quic_router
                .forward_to_clients(sid, sid.as_bytes())
                .await;
        }
        state
    }

    async fn output(state: &AppState, sid: &str) -> Vec<u8> {
        let (id, _, _, bytes, _) = state.quic_router.attach_client(sid, None).await;
        state.quic_router.detach_client(sid, id).await;
        bytes
    }

    #[tokio::test]
    async fn foreign_destroy_does_not_touch_registry_or_router() {
        let state = state().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state.quic_router.register_daemon_input("victim", tx).await;
        let message = proto::DaemonMessage {
            payload: Some(proto::daemon_message::Payload::SessionDestroyed(
                proto::SessionDestroyed {
                    session_id: "victim".into(),
                    ..Default::default()
                },
            )),
        };
        handle_daemon_message(&message, "other-device", "other-user", &state).await;
        assert!(state.sessions.owns_session("victim-user", "victim"));
        assert_eq!(output(&state, "victim").await, b"victim");
        state
            .quic_router
            .send_to_daemon("victim", b"still routed")
            .await;
        assert!(
            matches!(rx.try_recv(), Ok(crate::quic::DaemonCommand::Input(data)) if data == b"still routed")
        );
        handle_daemon_message(&message, "victim-device", "victim-user", &state).await;
        assert!(!state.sessions.owns_session("victim-user", "victim"));
        assert!(output(&state, "victim").await.is_empty());
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn foreign_resize_cannot_change_victim_dimensions() {
        let state = state().await;
        let message = proto::DaemonMessage {
            payload: Some(proto::daemon_message::Payload::SessionResize(
                proto::SessionResize {
                    session_id: "victim".into(),
                    cols: 1,
                    rows: 1,
                },
            )),
        };
        handle_daemon_message(&message, "other-device", "other-user", &state).await;
        let victim = &state.sessions.get_user_sessions("victim-user")[0].sessions[0];
        assert_eq!((victim.cols, victim.rows), (80, 24));
        handle_daemon_message(&message, "victim-device", "victim-user", &state).await;
        let victim = &state.sessions.get_user_sessions("victim-user")[0].sessions[0];
        assert_eq!((victim.cols, victim.rows), (1, 1));
    }

    #[tokio::test]
    async fn heartbeat_forgets_only_its_own_removed_routes_and_buffers() {
        let state = state().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state.quic_router.register_daemon_input("other", tx).await;
        let message = proto::DaemonMessage {
            payload: Some(proto::daemon_message::Payload::Heartbeat(
                proto::Heartbeat {
                    sessions: vec![proto::SessionInfo {
                        session_id: "victim".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
        };
        handle_daemon_message(&message, "other-device", "other-user", &state).await;
        assert!(output(&state, "other").await.is_empty());
        assert!(rx.recv().await.is_none());
        assert_eq!(output(&state, "victim").await, b"victim");
        assert!(state.sessions.owns_session("victim-user", "victim"));
        assert!(!state.sessions.owns_session("other-user", "victim"));
    }
}
