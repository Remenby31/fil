use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::collections::HashSet;

use crate::auth::AuthUser;
use crate::sessions::DeviceState;
use crate::state::AppState;

pub async fn list_sessions(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceState>>, StatusCode> {
    let mut devices = state.sessions.get_user_sessions(&auth.user_id);
    let live_device_ids: HashSet<String> = devices
        .iter()
        .map(|device| device.device_id.clone())
        .collect();

    let registered_devices = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, name, last_seen FROM devices WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    devices.extend(
        registered_devices
            .into_iter()
            .filter(|(device_id, _, _)| !live_device_ids.contains(device_id))
            .map(|(device_id, device_name, last_seen)| DeviceState {
                device_id,
                device_name,
                user_id: auth.user_id.clone(),
                sessions: Vec::new(),
                last_heartbeat: parse_sqlite_datetime(&last_seen),
                connected: false,
            }),
    );

    Ok(Json(devices))
}

fn parse_sqlite_datetime(value: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
        .unwrap_or_else(|_| Utc::now())
}
