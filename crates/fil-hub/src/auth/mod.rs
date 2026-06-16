mod jwt;
mod github;
mod apple;
pub mod shared;

pub use jwt::verify_token;
pub use github::{github_auth_start, github_auth_callback};
pub use apple::apple_auth_callback;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use crate::state::AppState;

pub struct AuthUser {
    pub user_id: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let claims = verify_token(token, &state.config.jwt_secret)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        Ok(AuthUser {
            user_id: claims.sub,
        })
    }
}
