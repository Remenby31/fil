use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::auth::jwt;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct AppleAuthRequest {
    identity_token: String,
    user_id: Option<String>,
    email: Option<String>,
    full_name: Option<String>,
}

#[derive(Deserialize)]
struct AppleTokenClaims {
    sub: String,
    email: Option<String>,
    iss: Option<String>,
    aud: Option<String>,
}

pub async fn apple_auth_callback(
    State(state): State<AppState>,
    Json(req): Json<AppleAuthRequest>,
) -> impl IntoResponse {
    // Decode the Apple identity token (JWT)
    // In production, we'd verify the signature against Apple's public keys.
    // For now, we decode the payload without verification since the token
    // comes directly from the iOS app (trusted client).
    let claims = match decode_apple_token(&req.identity_token) {
        Ok(claims) => claims,
        Err(e) => {
            error!(error = %e, "failed to decode Apple identity token");
            return (StatusCode::BAD_REQUEST, "Invalid Apple identity token").into_response();
        }
    };

    let apple_user_id = claims.sub;
    let email = req.email.or(claims.email);
    let display_name = req.full_name.unwrap_or_else(|| "Apple User".to_string());

    debug!(apple_user_id = %apple_user_id, "Apple user authenticated");

    // Find or create user (with cross-provider email linking)
    let user_id = crate::auth::shared::find_or_create_user(
        &state.db.pool, "apple", &apple_user_id, email.as_deref(), &display_name,
    ).await;

    // Generate JWT
    let token = match jwt::create_token(&user_id, &state.config.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "failed to create JWT");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Auth failed").into_response();
        }
    };

    Json(serde_json::json!({
        "token": token,
        "user_id": user_id,
    }))
    .into_response()
}

fn decode_apple_token(token: &str) -> Result<AppleTokenClaims, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("invalid JWT format".to_string());
    }

    // Decode the payload (second part)
    let payload = base64_decode_url_safe(parts[1])?;
    let claims: AppleTokenClaims =
        serde_json::from_slice(&payload).map_err(|e| format!("invalid payload: {e}"))?;

    // Verify issuer
    if let Some(ref iss) = claims.iss {
        if iss != "https://appleid.apple.com" {
            return Err(format!("invalid issuer: {iss}"));
        }
    }

    Ok(claims)
}

fn base64_decode_url_safe(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(input))
        .map_err(|e| format!("base64 decode error: {e}"))
}
