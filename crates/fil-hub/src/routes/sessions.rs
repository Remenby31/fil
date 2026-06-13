use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::AuthUser;
use crate::sessions::DeviceState;
use crate::state::AppState;

pub async fn list_sessions(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceState>>, StatusCode> {
    let devices = state.sessions.get_user_sessions(&auth.user_id);
    Ok(Json(devices))
}
