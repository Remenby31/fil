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

        authenticate_token(token, state).await
    }
}

pub async fn authenticate_token(token: &str, state: &AppState) -> Result<AuthUser, StatusCode> {
    let claims =
        verify_token(token, &state.config.jwt_secret).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let user_id = sqlx::query_scalar::<_, String>(
        "SELECT canonical.id FROM users u
         LEFT JOIN user_links ul ON ul.provider = u.provider AND ul.provider_id = u.provider_id
         JOIN users canonical ON canonical.id = COALESCE(ul.user_id, u.id)
         WHERE u.id = ? LIMIT 1",
    )
    .bind(&claims.sub)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(AuthUser { user_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deleted_account_token_is_rejected() {
        let state = crate::state::test_state().await;
        let token = jwt::create_token("deleted-user", &state.config.jwt_secret).unwrap();
        let request = axum::http::Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();
        assert!(
            AuthUser::from_request_parts(&mut parts, &state)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn existing_and_merged_tokens_resolve_to_canonical_account() {
        let state = crate::state::test_state().await;
        sqlx::query("INSERT INTO users (id, provider, provider_id) VALUES ('canonical', 'github', '123'), ('alias', 'apple', 'apple-sub')")
            .execute(&state.db.pool).await.unwrap();
        sqlx::query("INSERT INTO user_links (id, user_id, provider, provider_id) VALUES ('merge', 'canonical', 'apple', 'apple-sub')")
            .execute(&state.db.pool).await.unwrap();
        for subject in ["canonical", "alias"] {
            let token = jwt::create_token(subject, &state.config.jwt_secret).unwrap();
            assert_eq!(
                authenticate_token(&token, &state).await.unwrap().user_id,
                "canonical"
            );
        }
    }

    #[tokio::test]
    async fn dangling_canonical_link_cannot_authenticate() {
        let mut state = crate::state::test_state().await;
        // Deliberately model an old database without enforced foreign keys.
        state.db.pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(false),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE users (id TEXT, provider TEXT, provider_id TEXT);
            CREATE TABLE user_links (user_id TEXT, provider TEXT, provider_id TEXT);
            INSERT INTO users VALUES ('alias', 'apple', 'apple-sub');
            INSERT INTO user_links VALUES ('deleted-canonical', 'apple', 'apple-sub');",
        )
        .execute(&state.db.pool)
        .await
        .unwrap();
        let token = jwt::create_token("alias", &state.config.jwt_secret).unwrap();
        assert!(matches!(
            authenticate_token(&token, &state).await,
            Err(StatusCode::UNAUTHORIZED)
        ));
        assert!(
            shared::find_or_create_user(&state.db.pool, "apple", "apple-sub", None, "Apple")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn database_failure_is_not_an_authenticated_user() {
        let state = crate::state::test_state().await;
        let token = jwt::create_token("user", &state.config.jwt_secret).unwrap();
        state.db.pool.close().await;
        assert!(matches!(
            authenticate_token(&token, &state).await,
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        ));
    }

    #[tokio::test]
    async fn malformed_and_wrong_signature_tokens_are_unauthorized_before_db_lookup() {
        let state = crate::state::test_state().await;
        let wrong_signature = jwt::create_token("user", "wrong-test-secret").unwrap();
        state.db.pool.close().await;
        for token in ["", "not-a-token", &wrong_signature] {
            assert!(matches!(
                authenticate_token(token, &state).await,
                Err(StatusCode::UNAUTHORIZED)
            ));
        }
    }
}
