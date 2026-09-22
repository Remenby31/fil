use serde::de::DeserializeOwned;
use sqlx::SqlitePool;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

/// Resolve an authenticated provider identity, preserving explicit account links.
/// Email is profile metadata, never proof of ownership of another account.
pub async fn find_or_create_user(
    pool: &SqlitePool,
    provider: &str,
    provider_id: &str,
    email: Option<&str>,
    display_name: &str,
) -> Result<String, sqlx::Error> {
    // Acquire the write reservation before any read so simultaneous first logins
    // cannot each observe a missing identity and create divergent accounts.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Explicit links take precedence over legacy users rows after account merges.
    if let Some(id) = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM user_links WHERE provider = ? AND provider_id = ?",
    )
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        // Do not authenticate dangling links from a legacy/inconsistent database.
        let id = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(id);
    }

    let existing_id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM users WHERE provider = ? AND provider_id = ?",
    )
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(&mut *tx)
    .await?;

    let user_id = match existing_id {
        Some(id) => id,
        None => {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO users (id, provider, provider_id, email, display_name) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(provider)
            .bind(provider_id)
            .bind(email)
            .bind(display_name)
            .execute(&mut *tx)
            .await?;
            id
        }
    };

    // Register legacy and newly created identities atomically with user creation.
    sqlx::query("INSERT INTO user_links (id, user_id, provider, provider_id) VALUES (?, ?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(&user_id)
        .bind(provider)
        .bind(provider_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(user_id)
}

fn provider_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("fil-hub")
}

pub(super) fn provider_http_client() -> Result<&'static reqwest::Client, &'static reqwest::Error> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    CLIENT
        .get_or_init(|| provider_client_builder().https_only(true).build())
        .as_ref()
}

const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;

/// Read a bounded response. Errors deliberately exclude URLs, headers, and bodies
/// so OAuth codes, client secrets, and bearer tokens cannot escape into logs.
pub(super) async fn provider_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, &'static str> {
    let mut response = request
        .send()
        .await
        .map_err(|_| "provider request failed")?;
    if !response.status().is_success() {
        return Err("provider returned an unsuccessful status");
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_PROVIDER_RESPONSE_BYTES as u64)
    {
        return Err("provider response too large");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "provider response failed")?
    {
        if chunk.len() > MAX_PROVIDER_RESPONSE_BYTES - body.len() {
            return Err("provider response too large");
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid provider response")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) struct TestProvider {
        pub url: String,
        pub client: reqwest::Client,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestProvider {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    pub(crate) async fn mock_provider(router: axum::Router) -> TestProvider {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        TestProvider {
            url,
            client: provider_client_builder().no_proxy().build().unwrap(),
            task,
        }
    }

    #[tokio::test]
    async fn matching_email_is_not_permission_to_link_accounts() {
        let state = crate::state::test_state().await;
        let first = find_or_create_user(
            &state.db.pool,
            "apple",
            "apple-subject",
            Some("shared@example.test"),
            "Apple",
        )
        .await
        .unwrap();
        let second = find_or_create_user(
            &state.db.pool,
            "github",
            "123",
            Some("shared@example.test"),
            "GitHub",
        )
        .await
        .unwrap();
        assert_ne!(
            first, second,
            "email alone must not merge independent identities"
        );
    }

    #[tokio::test]
    async fn failed_user_insert_returns_error_not_a_phantom_user() {
        let state = crate::state::test_state().await;
        sqlx::query("CREATE TRIGGER fail_user_insert BEFORE INSERT ON users BEGIN SELECT RAISE(ABORT, 'test-only write failure'); END")
            .execute(&state.db.pool).await.unwrap();
        let result = find_or_create_user(&state.db.pool, "github", "123", None, "GitHub").await;
        assert!(
            result.is_err(),
            "a rejected insert must not authenticate a nonexistent user"
        );
    }

    #[tokio::test]
    async fn failed_link_insert_rolls_back_new_user() {
        let state = crate::state::test_state().await;
        sqlx::query("CREATE TRIGGER fail_link_insert BEFORE INSERT ON user_links BEGIN SELECT RAISE(ABORT, 'test-only link failure'); END")
            .execute(&state.db.pool).await.unwrap();
        assert!(
            find_or_create_user(&state.db.pool, "github", "123", None, "GitHub")
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "the failed transaction must not leave an orphan user"
        );
    }

    #[tokio::test]
    async fn lookup_and_closed_pool_errors_are_propagated() {
        let state = crate::state::test_state().await;
        sqlx::query("DROP TABLE user_links")
            .execute(&state.db.pool)
            .await
            .unwrap();
        assert!(
            find_or_create_user(&state.db.pool, "github", "123", None, "GitHub")
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        state.db.pool.close().await;
        assert!(matches!(
            find_or_create_user(&state.db.pool, "github", "123", None, "GitHub").await,
            Err(sqlx::Error::PoolClosed)
        ));
    }

    #[tokio::test]
    async fn legacy_user_gets_a_link_and_explicit_merge_wins() {
        let state = crate::state::test_state().await;
        sqlx::query("INSERT INTO users (id, provider, provider_id) VALUES ('canonical', 'github', '123'), ('alias', 'apple', 'apple-sub')")
            .execute(&state.db.pool).await.unwrap();
        assert_eq!(
            find_or_create_user(&state.db.pool, "github", "123", None, "GitHub")
                .await
                .unwrap(),
            "canonical"
        );
        sqlx::query("INSERT INTO user_links (id, user_id, provider, provider_id) VALUES ('merge', 'canonical', 'apple', 'apple-sub')")
            .execute(&state.db.pool).await.unwrap();
        assert_eq!(
            find_or_create_user(&state.db.pool, "apple", "apple-sub", None, "Apple")
                .await
                .unwrap(),
            "canonical"
        );
        assert_eq!(
            find_or_create_user(&state.db.pool, "github", "123", None, "GitHub")
                .await
                .unwrap(),
            "canonical"
        );
    }

    #[tokio::test]
    async fn concurrent_first_logins_converge_on_one_user_and_link() {
        // A file-backed WAL database exercises distinct physical connections and
        // SQLite's actual write lock (not a serialized one-connection mock).
        let path = std::env::temp_dir().join(format!("fil-auth-race-{}.db", Uuid::new_v4()));
        let db = crate::db::Database::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&db.pool)
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let pool = db.pool.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                find_or_create_user(&pool, "github", "123", None, "GitHub")
                    .await
                    .unwrap()
            });
        }
        let mut ids = std::collections::HashSet::new();
        while let Some(result) = tasks.join_next().await {
            ids.insert(result.unwrap());
        }
        assert_eq!(ids.len(), 1);
        for table in ["users", "user_links"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&db.pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
        db.pool.close().await;
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn provider_client_is_reused() {
        assert!(std::ptr::eq(
            provider_http_client().unwrap(),
            provider_http_client().unwrap()
        ));
    }

    #[tokio::test]
    async fn provider_json_checks_status_redirects_size_and_redacts_errors() {
        use axum::{Router, routing::get};
        let router = Router::new()
            .route(
                "/ok",
                get(|| async { axum::Json(serde_json::json!({"ok":true})) }),
            )
            .route(
                "/error",
                get(|| async { (axum::http::StatusCode::UNAUTHORIZED, "test-only-secret") }),
            )
            .route(
                "/redirect",
                get(|| async { axum::response::Redirect::temporary("/ok") }),
            )
            .route(
                "/oversize",
                get(|| async { "x".repeat(MAX_PROVIDER_RESPONSE_BYTES + 1) }),
            )
            .route("/invalid", get(|| async { "test-only-secret" }));
        let server = mock_provider(router).await;
        assert_eq!(
            provider_json::<serde_json::Value>(server.client.get(format!("{}/ok", server.url)))
                .await
                .unwrap()["ok"],
            true
        );
        for endpoint in ["error", "redirect", "oversize", "invalid"] {
            let error = provider_json::<serde_json::Value>(
                server
                    .client
                    .get(format!("{}/{endpoint}?token=test-only-secret", server.url)),
            )
            .await
            .unwrap_err();
            assert!(!error.contains("test-only-secret"));
            assert!(!error.contains(&server.url));
        }
    }

    #[tokio::test]
    async fn provider_json_caps_streams_without_content_length() {
        let router = axum::Router::new().route(
            "/",
            axum::routing::get(|| async {
                let chunks = futures_util::stream::iter((0..5).map(|_| {
                    Ok::<_, std::convert::Infallible>(vec![b'x'; MAX_PROVIDER_RESPONSE_BYTES / 4])
                }));
                axum::body::Body::from_stream(chunks)
            }),
        );
        let server = mock_provider(router).await;
        assert_eq!(
            provider_json::<serde_json::Value>(server.client.get(&server.url))
                .await
                .unwrap_err(),
            "provider response too large"
        );
    }

    #[tokio::test]
    async fn provider_json_times_out_while_reading_response_body() {
        let router = axum::Router::new().route(
            "/",
            axum::routing::get(|| async {
                axum::body::Body::from_stream(futures_util::stream::pending::<
                    Result<Vec<u8>, std::io::Error>,
                >())
            }),
        );
        let server = mock_provider(router).await;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            provider_json::<serde_json::Value>(
                server
                    .client
                    .get(&server.url)
                    .timeout(Duration::from_millis(100)),
            ),
        )
        .await
        .expect("the per-request timeout must cover body reads");
        assert!(result.is_err());
    }
}
