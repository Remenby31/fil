use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::state::AppState;

pub async fn health_check(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "fil-hub",
        "build": option_env!("FIL_BUILD_REVISION").unwrap_or("development"),
        "authenticated_quic": state.config.require_attach_ticket,
        "live_activity_push_enabled": state.apns.is_enabled(),
    }))
}
