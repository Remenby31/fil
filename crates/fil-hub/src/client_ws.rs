use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::debug;

use crate::auth::verify_token;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ClientWsParams {
    token: String,
}

pub async fn client_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ClientWsParams>,
    State(state): State<AppState>,
) -> Response {
    let claims = match verify_token(&params.token, &state.config.jwt_secret) {
        Ok(claims) => claims,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    ws.on_upgrade(move |socket| client_socket(socket, claims.sub, state))
}

async fn client_socket(socket: WebSocket, user_id: String, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let initial = state.sessions.get_user_sessions(&user_id);
    if let Ok(payload) = serde_json::to_string(&initial)
        && sender.send(Message::Text(payload.into())).await.is_err()
    {
        return;
    }

    let mut updates = state.sessions.subscribe();
    loop {
        tokio::select! {
            result = updates.recv() => {
                match result {
                    Ok(update) if update.user_id == user_id => {
                        let Ok(payload) = serde_json::to_string(&update.devices) else { continue };
                        if sender.send(Message::Text(payload.into())).await.is_err() { break; }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let Ok(payload) = serde_json::to_string(&state.sessions.get_user_sessions(&user_id)) else { continue };
                        if sender.send(Message::Text(payload.into())).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    debug!(%user_id, "iOS client WebSocket disconnected");
}
