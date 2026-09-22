use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::debug;

use crate::auth::authenticate_token;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ClientWsParams {
    token: Option<String>,
}

fn credential<'a>(
    headers: &'a HeaderMap,
    params: &'a ClientWsParams,
    legacy_until: Option<i64>,
    now: i64,
) -> Result<&'a str, StatusCode> {
    if let Some(value) = headers.get("authorization") {
        return value
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(StatusCode::UNAUTHORIZED);
    }
    if legacy_until.is_some_and(|deadline| now < deadline) {
        return params
            .token
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or(StatusCode::UNAUTHORIZED);
    }
    Err(StatusCode::UNAUTHORIZED)
}

pub async fn client_ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ClientWsParams>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let token = match credential(
        &headers,
        &params,
        state.config.legacy_ws_token_until,
        chrono::Utc::now().timestamp(),
    ) {
        Ok(token) => token.to_owned(),
        Err(status) => return status.into_response(),
    };
    let legacy = !headers.contains_key("authorization");
    let auth = match authenticate_token(&token, &state).await {
        Ok(auth) => auth,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    ws.max_message_size(16 * 1024)
        .on_upgrade(move |socket| client_socket(socket, auth.user_id, token, legacy, state))
}

async fn client_socket(
    socket: WebSocket,
    user_id: String,
    token: String,
    legacy: bool,
    state: AppState,
) {
    let (mut sender, mut receiver) = socket.split();
    let initial = state.sessions.get_user_sessions(&user_id);
    if let Ok(payload) = serde_json::to_string(&initial)
        && sender.send(Message::Text(payload.into())).await.is_err()
    {
        return;
    }

    let mut updates = state.sessions.subscribe();
    let mut recheck = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if (legacy && state.config.legacy_ws_token_until.is_none_or(|t| chrono::Utc::now().timestamp() >= t))
                    || authenticate_token(&token, &state).await.is_err() { break; }
            }
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
                    Some(Ok(Message::Ping(data))) if sender.send(Message::Pong(data.clone())).await.is_err() => break,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    debug!(%user_id, "iOS client WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bearer_is_preferred_and_legacy_query_access_has_an_explicit_deadline() {
        let query = ClientWsParams {
            token: Some("legacy".into()),
        };
        let mut headers = HeaderMap::new();
        assert!(credential(&headers, &query, None, 100).is_err());
        assert_eq!(credential(&headers, &query, Some(101), 100), Ok("legacy"));
        assert!(credential(&headers, &query, Some(100), 100).is_err());
        headers.insert("authorization", "Bearer modern".parse().unwrap());
        assert_eq!(credential(&headers, &query, None, 100), Ok("modern"));
        headers.insert("authorization", "Basic invalid".parse().unwrap());
        assert!(credential(&headers, &query, Some(101), 100).is_err());
    }
}
