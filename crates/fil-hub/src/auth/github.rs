use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use serde::Deserialize;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::auth::jwt;
use crate::auth::shared::{find_or_create_user, provider_http_client, provider_json};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct StartParams {
    cli_callback: Option<String>,
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

fn verified_primary_email(emails: &[GitHubEmail]) -> Option<&str> {
    emails
        .iter()
        .find(|email| email.primary && email.verified && !email.email.trim().is_empty())
        .map(|email| email.email.as_str())
}

pub async fn github_auth_start(
    State(state): State<AppState>,
    Query(params): Query<StartParams>,
) -> impl IntoResponse {
    let cli_callback = params.cli_callback.unwrap_or_default();
    if !valid_callback(&cli_callback) {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid callback URL").into_response();
    }
    if state.config.github_client_id.is_empty() {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let oauth_state = Uuid::new_v4().to_string();

    // Store state + optional CLI callback for later
    if sqlx::query(
        "INSERT INTO oauth_states (state, provider, cli_callback) VALUES (?, 'github', ?)",
    )
    .bind(&oauth_state)
    .bind(&cli_callback)
    .execute(&state.db.pool)
    .await
    .is_err()
    {
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let url = reqwest::Url::parse_with_params(
        "https://github.com/login/oauth/authorize",
        &[
            ("client_id", state.config.github_client_id.as_str()),
            (
                "redirect_uri",
                &format!("{}/auth/github/callback", state.config.public_url),
            ),
            ("state", &oauth_state),
            ("scope", "user:email"),
        ],
    );
    let Ok(url) = url else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    Redirect::temporary(url.as_str()).into_response()
}

fn valid_callback(callback: &str) -> bool {
    if callback.is_empty() || callback == "fil://callback" {
        return true;
    }
    // URL parsers normalize whitespace and backslashes; callbacks must not rely
    // on those browser-dependent interpretations.
    if callback
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return false;
    }
    // Match the literal authority and path: parsing alone would normalize
    // numeric IP aliases, dot segments, and an explicitly supplied default port.
    let Some((host, port)) = callback
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix("/callback"))
        .and_then(|authority| authority.rsplit_once(':'))
    else {
        return false;
    };
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

async fn consume_state(
    pool: &sqlx::SqlitePool,
    state: &str,
) -> Result<Option<String>, sqlx::Error> {
    // DELETE RETURNING makes the one-use check atomic across concurrent callbacks.
    sqlx::query_scalar(
        "DELETE FROM oauth_states WHERE state = ? AND provider = 'github'
         AND created_at >= datetime('now', '-10 minutes') AND created_at <= datetime('now')
         RETURNING COALESCE(cli_callback, '')",
    )
    .bind(state)
    .fetch_optional(pool)
    .await
}

pub async fn github_auth_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    let client = match provider_http_client() {
        Ok(client) => client,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    github_auth_with_provider(
        &state,
        params,
        client,
        "https://github.com/login/oauth/access_token",
        "https://api.github.com",
    )
    .await
}

async fn github_auth_with_provider(
    state: &AppState,
    params: CallbackParams,
    client: &reqwest::Client,
    token_url: &str,
    api_url: &str,
) -> axum::response::Response {
    // Verify CSRF state and get cli_callback
    let cli_callback = match consume_state(&state.db.pool, &params.state).await {
        Ok(Some(cb)) if valid_callback(&cb) => cb,
        Ok(_) => {
            return (axum::http::StatusCode::BAD_REQUEST, "Invalid OAuth state").into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Exchange code for access token
    let token_data = match provider_json::<GitHubTokenResponse>(
        client
            .post(token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", state.config.github_client_id.as_str()),
                ("client_secret", state.config.github_client_secret.as_str()),
                ("code", &params.code),
                (
                    "redirect_uri",
                    &format!("{}/auth/github/callback", state.config.public_url),
                ),
            ]),
    )
    .await
    {
        Ok(data) if !data.access_token.trim().is_empty() => data,
        Ok(_) => return (StatusCode::BAD_GATEWAY, "GitHub auth failed").into_response(),
        Err(e) => {
            error!(error = %e, "failed to exchange GitHub code");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "GitHub auth failed",
            )
                .into_response();
        }
    };

    // Get user info
    let github_user = match provider_json::<GitHubUser>(
        client
            .get(format!("{api_url}/user"))
            .bearer_auth(&token_data.access_token)
            .header("Accept", "application/vnd.github+json"),
    )
    .await
    {
        Ok(user) if user.id > 0 && !user.login.trim().is_empty() => user,
        Ok(_) => return (StatusCode::BAD_GATEWAY, "GitHub auth failed").into_response(),
        Err(e) => {
            error!(error = %e, "failed to fetch GitHub user");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "GitHub auth failed",
            )
                .into_response();
        }
    };

    // The public profile email is not a verification signal. Only the authenticated
    // email endpoint can supply a verified primary address. Fail closed on errors.
    let emails = match provider_json::<Vec<GitHubEmail>>(
        client
            .get(format!("{api_url}/user/emails?per_page=100"))
            .bearer_auth(&token_data.access_token)
            .header("Accept", "application/vnd.github+json"),
    )
    .await
    {
        Ok(emails) => emails,
        Err(e) => {
            error!(error = %e, "failed to fetch verified GitHub emails");
            return (StatusCode::BAD_GATEWAY, "GitHub auth failed").into_response();
        }
    };

    debug!(github_id = github_user.id, login = %github_user.login, "GitHub user authenticated");

    // Resolve the provider identity; verified email remains profile metadata.
    let provider_id = github_user.id.to_string();
    let email = verified_primary_email(&emails);
    let display_name = github_user.name.unwrap_or(github_user.login);

    let user_id =
        match find_or_create_user(&state.db.pool, "github", &provider_id, email, &display_name)
            .await
        {
            Ok(user_id) => user_id,
            Err(_) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "GitHub auth failed",
                )
                    .into_response();
            }
        };

    // Generate JWT
    let token = match jwt::create_token(&user_id, &state.config.jwt_secret) {
        Ok(token) => token,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "GitHub auth failed").into_response(),
    };

    // If CLI callback is set, redirect the token to the local CLI server
    if !cli_callback.is_empty() {
        let redirect_url = format!("{}?token={}", cli_callback, token);
        info!("redirecting token to CLI callback");
        return Redirect::temporary(&redirect_url).into_response();
    }

    // Otherwise return JSON (for web/app clients)
    axum::Json(serde_json::json!({
        "token": token,
        "user_id": user_id,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::shared::tests::{TestProvider, mock_provider};
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use serde_json::{Value, json};

    async fn insert_state(state: &AppState, name: &str, callback: &str) {
        sqlx::query(
            "INSERT INTO oauth_states (state, provider, cli_callback) VALUES (?, 'github', ?)",
        )
        .bind(name)
        .bind(callback)
        .execute(&state.db.pool)
        .await
        .unwrap();
    }

    async fn github_provider(emails: Value, email_status: StatusCode) -> TestProvider {
        mock_provider(Router::new()
            .route("/token", post(|axum::Form(form): axum::Form<std::collections::HashMap<String, String>>| async move {
                assert_eq!(form.get("code").map(String::as_str), Some("test-only-code"));
                assert_eq!(form.get("redirect_uri").map(String::as_str), Some("http://localhost/auth/github/callback"));
                Json(json!({"access_token": "test-only-provider-token"}))
            }))
            .route("/user", get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer test-only-provider-token");
                Json(json!({"id": 123, "login": "test-user", "email": "untrusted-profile@example.test", "name": "Test User"}))
            }))
            .route("/user/emails", get(move |headers: axum::http::HeaderMap| {
                assert_eq!(headers["authorization"], "Bearer test-only-provider-token");
                let emails = emails.clone();
                async move { (email_status, Json(emails)) }
            }))).await
    }

    async fn callback(
        state: &AppState,
        provider: &TestProvider,
        oauth_state: &str,
    ) -> axum::response::Response {
        github_auth_with_provider(
            state,
            CallbackParams {
                code: "test-only-code".into(),
                state: oauth_state.into(),
            },
            &provider.client,
            &format!("{}/token", provider.url),
            &provider.url,
        )
        .await
    }

    #[test]
    fn email_requires_both_verified_and_primary_flags() {
        let emails: Vec<GitHubEmail> = serde_json::from_value(json!([
            {"email":"primary-unverified@example.test", "primary":true, "verified":false},
            {"email":"secondary@example.test", "primary":false, "verified":true},
            {"email":"  ", "primary":true, "verified":true},
            {"email":"verified@example.test", "primary":true, "verified":true},
        ]))
        .unwrap();
        assert_eq!(
            verified_primary_email(&emails),
            Some("verified@example.test")
        );
        assert_eq!(verified_primary_email(&emails[..3]), None);
        assert_eq!(verified_primary_email(&[]), None);
    }

    #[tokio::test]
    async fn callback_uses_verified_primary_email_never_profile_email() {
        for emails in [
            json!([
                {"email":"secondary@example.test", "primary":false, "verified":true},
                {"email":"verified@example.test", "primary":true, "verified":true},
            ]),
            json!([]),
        ] {
            let provider = github_provider(emails.clone(), StatusCode::OK).await;
            let state = crate::state::test_state().await;
            insert_state(&state, "pending", "").await;
            let response = callback(&state, &provider, "pending").await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap();
            assert!(!String::from_utf8_lossy(&body).contains("test-only-provider-token"));
            let auth: Value = serde_json::from_slice(&body).unwrap();
            let authenticated =
                crate::auth::authenticate_token(auth["token"].as_str().unwrap(), &state)
                    .await
                    .unwrap();
            assert_eq!(authenticated.user_id, auth["user_id"].as_str().unwrap());
            let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = ?")
                .bind(&authenticated.user_id)
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
            assert_eq!(
                email.as_deref(),
                (!emails.as_array().unwrap().is_empty()).then_some("verified@example.test")
            );
            assert_eq!(
                callback(&state, &provider, "pending").await.status(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[tokio::test]
    async fn verified_email_endpoint_failure_does_not_fall_back_to_profile() {
        let provider = github_provider(
            json!({"message":"test-only-provider-token"}),
            StatusCode::FORBIDDEN,
        )
        .await;
        let state = crate::state::test_state().await;
        insert_state(&state, "pending", "").await;
        let response = callback(&state, &provider, "pending").await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"GitHub auth failed");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            consume_state(&state.db.pool, "pending").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn malformed_provider_tokens_never_panic_or_escape_into_responses() {
        for access_token in ["", "test-only-token\r\nInjected: header"] {
            let provider = mock_provider(Router::new().route(
                "/token",
                post(move || async move { Json(json!({"access_token": access_token})) }),
            ))
            .await;
            let state = crate::state::test_state().await;
            insert_state(&state, "pending", "fil://callback").await;
            let response = callback(&state, &provider, "pending").await;
            assert!(response.status().is_server_error());
            assert!(!response.headers().contains_key("location"));
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), b"GitHub auth failed");
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn github_db_failure_returns_no_token_or_redirect() {
        let provider = github_provider(json!([]), StatusCode::OK).await;
        let state = crate::state::test_state().await;
        insert_state(&state, "pending", "fil://callback").await;
        sqlx::query("CREATE TRIGGER fail_user_insert BEFORE INSERT ON users BEGIN SELECT RAISE(ABORT, 'test-only failure'); END")
            .execute(&state.db.pool).await.unwrap();
        let response = callback(&state, &provider, "pending").await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!response.headers().contains_key("location"));
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"GitHub auth failed");
    }

    #[tokio::test]
    async fn app_and_loopback_callbacks_receive_only_the_hub_token() {
        let provider = github_provider(json!([]), StatusCode::OK).await;
        let state = crate::state::test_state().await;
        for (index, target) in [
            "fil://callback",
            "http://127.0.0.1:12345/callback",
            "http://[::1]:12345/callback",
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("pending-{index}");
            insert_state(&state, &name, target).await;
            let response = callback(&state, &provider, &name).await;
            assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
            let location = response.headers()["location"].to_str().unwrap();
            let token = location.strip_prefix(&format!("{target}?token=")).unwrap();
            assert!(!location.contains("test-only-provider-token"));
            assert!(crate::auth::authenticate_token(token, &state).await.is_ok());
        }
    }

    #[tokio::test]
    async fn oauth_state_is_one_use_and_rejects_stale_future_and_other_providers() {
        let state = crate::state::test_state().await;
        insert_state(&state, "fresh", "fil://callback").await;
        sqlx::query(
            "INSERT INTO oauth_states (state, provider, created_at) VALUES
            ('stale', 'github', datetime('now', '-11 minutes')),
            ('future', 'github', datetime('now', '+1 minute')),
            ('wrong-provider', 'apple', datetime('now')),
            ('malformed-time', 'github', 'invalid')",
        )
        .execute(&state.db.pool)
        .await
        .unwrap();
        assert_eq!(
            consume_state(&state.db.pool, "fresh")
                .await
                .unwrap()
                .as_deref(),
            Some("fil://callback")
        );
        for name in [
            "fresh",
            "stale",
            "future",
            "wrong-provider",
            "malformed-time",
            "missing",
            "' OR 1=1 --",
        ] {
            assert_eq!(
                consume_state(&state.db.pool, name).await.unwrap(),
                None,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn concurrent_callbacks_cannot_consume_the_same_state_twice() {
        let state = crate::state::test_state().await;
        insert_state(&state, "fresh", "fil://callback").await;
        let mut tasks = tokio::task::JoinSet::new();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
        for _ in 0..16 {
            let pool = state.db.pool.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                consume_state(&pool, "fresh").await.unwrap()
            });
        }
        let mut accepted = 0;
        while let Some(result) = tasks.join_next().await {
            accepted += usize::from(result.unwrap().is_some());
        }
        assert_eq!(accepted, 1);
    }

    #[tokio::test]
    async fn invalid_state_and_untrusted_stored_callback_never_contact_provider() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let provider = mock_provider(Router::new().fallback(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::INTERNAL_SERVER_ERROR }
        }))
        .await;
        let state = crate::state::test_state().await;
        insert_state(&state, "bad-callback", "https://attacker.invalid/steal").await;
        insert_state(&state, "stale", "").await;
        sqlx::query("UPDATE oauth_states SET created_at = datetime('now', '-11 minutes') WHERE state = 'stale'")
            .execute(&state.db.pool).await.unwrap();
        for name in ["missing", "stale", "bad-callback"] {
            assert_eq!(
                callback(&state, &provider, name).await.status(),
                StatusCode::BAD_REQUEST
            );
        }
        state.db.pool.close().await;
        assert_eq!(
            callback(&state, &provider, "pending").await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn start_encodes_redirect_parameters_and_handles_database_failure() {
        let mut state = crate::state::test_state().await;
        state.config.github_client_id = "test-client&scope=unwanted".into();
        let response = github_auth_start(
            State(state.clone()),
            Query(StartParams {
                cli_callback: Some("fil://callback".into()),
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let url = reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["client_id"], "test-client&scope=unwanted");
        assert_eq!(query["scope"], "user:email");
        assert_eq!(
            query["redirect_uri"],
            "http://localhost/auth/github/callback"
        );
        assert_eq!(
            consume_state(&state.db.pool, &query["state"])
                .await
                .unwrap()
                .as_deref(),
            Some("fil://callback")
        );
        state.db.pool.close().await;
        let response = github_auth_start(State(state), Query(StartParams { cli_callback: None }))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!response.headers().contains_key("location"));
    }

    #[test]
    fn callback_allowlist_accepts_only_explicit_local_endpoints() {
        for callback in [
            "",
            "fil://callback",
            "http://localhost:12345/callback",
            "http://127.0.0.1:65535/callback",
            "http://[::1]:12345/callback",
            "http://localhost:80/callback",
        ] {
            assert!(
                valid_callback(callback),
                "rejected valid callback: {callback}"
            );
        }
        for callback in [
            "https://attacker.invalid/callback",
            "https://localhost:12345/callback",
            "http://localhost/callback",
            "http://localhost:0/callback",
            "http://localhost:65536/callback",
            "http://127.0.0.2:12345/callback",
            "http://0.0.0.0:12345/callback",
            "http://localhost.attacker.invalid:12345/callback",
            "http://user@localhost:12345/callback",
            "http://localhost:12345/callback?next=https://attacker.invalid",
            "http://localhost:12345/callback#fragment",
            "http://localhost:12345/other",
            "fil://callback/extra",
            "fil://callback?token=bad",
            "fil://attacker",
            "javascript:alert(1)",
            "//localhost:12345/callback",
            " http://localhost:12345/callback",
            "http://local\nhost:12345/callback",
            "http://localhost:12345/a/../callback",
            "http://127.1:12345/callback",
            "http://2130706433:12345/callback",
            "http://localhost:+80/callback",
        ] {
            assert!(
                !valid_callback(callback),
                "accepted unsafe callback: {callback}"
            );
        }
    }

    #[tokio::test]
    async fn untrusted_callback_is_rejected_before_creating_state() {
        let state = crate::state::test_state().await;
        let response = github_auth_start(
            State(state.clone()),
            Query(StartParams {
                cli_callback: Some("https://attacker.invalid/steal".into()),
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_states")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
