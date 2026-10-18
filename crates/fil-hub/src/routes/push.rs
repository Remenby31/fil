use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::agent::AnswerError;
use crate::auth::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterPushTokenRequest {
    pub token: String,
    pub environment: String,
}

fn is_valid_token(token: &str) -> bool {
    (32..=200).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Idempotent. A token belongs to one phone install, so re-registering under
/// another account moves it rather than notifying both accounts.
pub async fn register_push_token(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(request): Json<RegisterPushTokenRequest>,
) -> Result<StatusCode, StatusCode> {
    if !is_valid_token(&request.token)
        || !matches!(request.environment.as_str(), "sandbox" | "production")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    sqlx::query(
        "INSERT INTO push_tokens (token, user_id, environment) VALUES (?, ?, ?)
         ON CONFLICT(token) DO UPDATE SET
             user_id = excluded.user_id,
             environment = excluded.environment,
             updated_at = datetime('now')",
    )
    .bind(request.token.to_ascii_lowercase())
    .bind(&auth.user_id)
    .bind(&request.environment)
    .execute(&state.db.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_push_token(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM push_tokens WHERE token = ? AND user_id = ?")
        .bind(token.to_ascii_lowercase())
        .bind(&auth.user_id)
        .execute(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct AnswerRequest {
    pub answer: String,
}

pub async fn answer_agent_notification(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(notification_id): Path<String>,
    Json(request): Json<AnswerRequest>,
) -> StatusCode {
    match state
        .agent
        .answer(&auth.user_id, &notification_id, &request.answer)
    {
        Ok(()) => StatusCode::NO_CONTENT,
        // Expired, already answered, or someone else's: the app says "no longer open".
        Err(AnswerError::NotFound) => StatusCode::GONE,
        Err(AnswerError::InvalidOption) => StatusCode::BAD_REQUEST,
        Err(AnswerError::DaemonOffline) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_tokens_are_validated_and_move_between_accounts() {
        let state = crate::state::test_state().await;
        sqlx::query("INSERT INTO users (id, provider, provider_id) VALUES ('a', 'github', '1'), ('b', 'github', '2')")
            .execute(&state.db.pool)
            .await
            .unwrap();
        let token = "AB".repeat(32);
        let register = |user: &str, token: &str, environment: &str| {
            register_push_token(
                AuthUser {
                    user_id: user.into(),
                },
                State(state.clone()),
                Json(RegisterPushTokenRequest {
                    token: token.into(),
                    environment: environment.into(),
                }),
            )
        };
        assert_eq!(
            register("a", "not-hex", "sandbox").await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            register("a", &token, "staging").await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            register("a", &token, "sandbox").await,
            Ok(StatusCode::NO_CONTENT)
        );
        assert_eq!(
            register("a", &token, "sandbox").await,
            Ok(StatusCode::NO_CONTENT)
        );
        assert_eq!(
            register("b", &token, "production").await,
            Ok(StatusCode::NO_CONTENT)
        );
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT token, user_id, environment FROM push_tokens")
                .fetch_all(&state.db.pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![(token.to_ascii_lowercase(), "b".into(), "production".into())]
        );

        // Another account cannot delete it.
        delete_push_token(
            AuthUser {
                user_id: "a".into(),
            },
            State(state.clone()),
            Path(token.clone()),
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_tokens")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
