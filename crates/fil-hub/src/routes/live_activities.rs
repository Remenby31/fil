use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterLiveActivityRequest {
    pub activity_id: String,
    pub session_id: String,
    pub device_id: String,
    pub push_token: String,
    pub environment: String,
}

pub async fn register_live_activity(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(request): Json<RegisterLiveActivityRequest>,
) -> Result<StatusCode, StatusCode> {
    if request.activity_id.is_empty()
        || request.push_token.is_empty()
        || !matches!(request.environment.as_str(), "sandbox" | "production")
        || !state
            .sessions
            .owns_session(&auth.user_id, &request.session_id)
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    sqlx::query(
        "INSERT INTO live_activities
         (activity_id, user_id, session_id, device_id, push_token, environment)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(activity_id) DO UPDATE SET
             user_id = excluded.user_id,
             session_id = excluded.session_id,
             device_id = excluded.device_id,
             push_token = excluded.push_token,
             environment = excluded.environment,
             updated_at = datetime('now')",
    )
    .bind(&request.activity_id)
    .bind(&auth.user_id)
    .bind(&request.session_id)
    .bind(&request.device_id)
    .bind(&request.push_token)
    .bind(&request.environment)
    .execute(&state.db.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_live_activity(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(activity_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM live_activities WHERE activity_id = ? AND user_id = ?")
        .bind(activity_id)
        .bind(auth.user_id)
        .execute(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
