mod apple;
mod github;
mod jwt;
pub mod shared;

pub use apple::apple_auth_callback;
pub use github::{github_auth_callback, github_auth_start};
pub use jwt::verify_token;

use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;

pub struct AuthUser {
    pub user_id: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let claims =
            verify_token(token, &state.config.jwt_secret).map_err(|_| StatusCode::UNAUTHORIZED)?;

        // Existing installations may still hold a JWT issued before two
        // provider identities were linked. Resolve the token subject through
        // user_links so those tokens immediately see the canonical account.
        let linked_user_id = sqlx::query_scalar::<_, String>(
            "SELECT ul.user_id \
             FROM users u \
             JOIN user_links ul \
               ON ul.provider = u.provider \
              AND ul.provider_id = u.provider_id \
             WHERE u.id = ? \
             LIMIT 1",
        )
        .bind(&claims.sub)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok(AuthUser {
            user_id: linked_user_id.unwrap_or(claims.sub),
        })
    }
}
