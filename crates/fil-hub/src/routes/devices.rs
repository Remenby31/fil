use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tracing::debug;

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

    sqlx::query(
        "INSERT INTO devices (id, user_id, name, os, hostname) VALUES (?, ?, ?, ?, ?)"
    )
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
    let result = sqlx::query(
        "DELETE FROM devices WHERE id = ? AND user_id = ?"
    )
        .bind(&device_id)
        .bind(&auth.user_id)
        .execute(&state.db.pool)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            debug!(device_id = %device_id, "device deleted");
            StatusCode::NO_CONTENT
        }
        Ok(_) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
