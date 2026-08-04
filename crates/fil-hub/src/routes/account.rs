use axum::extract::State;
use axum::http::StatusCode;
use tracing::info;

use crate::auth::AuthUser;
use crate::state::AppState;

pub async fn delete_account(auth: AuthUser, State(state): State<AppState>) -> StatusCode {
    match delete_account_data(&state, &auth.user_id).await {
        Ok(true) => {
            state.sessions.remove_user(&auth.user_id);
            info!(user_id = %auth.user_id, "account deleted");
            StatusCode::NO_CONTENT
        }
        Ok(false) => StatusCode::NOT_FOUND,
        Err(error) => {
            tracing::error!(%error, user_id = %auth.user_id, "account deletion failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn delete_account_data(state: &AppState, user_id: &str) -> anyhow::Result<bool> {
    let mut tx = state.db.pool.begin().await?;
    let linked_identities = sqlx::query_as::<_, (String, String)>(
        "SELECT provider, provider_id FROM user_links WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM live_activities WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM devices WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM user_links WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    for (provider, provider_id) in linked_identities {
        sqlx::query("DELETE FROM users WHERE provider = ? AND provider_id = ? AND id != ?")
            .bind(provider)
            .bind(provider_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }

    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::net::SocketAddr;

    #[tokio::test]
    async fn deletion_removes_account_dependencies_and_linked_identity_rows() {
        let path =
            std::env::temp_dir().join(format!("fil-account-delete-{}.db", uuid::Uuid::new_v4()));
        let config = Config {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            database_url: format!("sqlite:{}?mode=rwc", path.display()),
            jwt_secret: "test".into(),
            github_client_id: String::new(),
            github_client_secret: String::new(),
            apple_client_id: String::new(),
            apple_team_id: String::new(),
            apple_key_id: String::new(),
            public_url: "http://localhost".into(),
            quic_port: 0,
            data_dir: std::env::temp_dir().display().to_string(),
            apns: None,
        };
        let state = AppState::new(config).await.unwrap();
        let user_id = "canonical-user";
        let alias_id = "legacy-alias";

        sqlx::query(
            "INSERT INTO users (id, provider, provider_id, email, display_name)
             VALUES (?, 'github', 'github-1', 'user@example.com', 'User')",
        )
        .bind(user_id)
        .execute(&state.db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO users (id, provider, provider_id, email, display_name)
             VALUES (?, 'apple', 'apple-1', 'user@example.com', 'User')",
        )
        .bind(alias_id)
        .execute(&state.db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_links (id, user_id, provider, provider_id)
             VALUES ('link-1', ?, 'apple', 'apple-1')",
        )
        .bind(user_id)
        .execute(&state.db.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO devices (id, user_id, name) VALUES ('device-1', ?, 'Mac')")
            .bind(user_id)
            .execute(&state.db.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO live_activities
             (activity_id, user_id, session_id, device_id, push_token, environment)
             VALUES ('activity-1', ?, 'session-1', 'device-1', 'token-1', 'sandbox')",
        )
        .bind(user_id)
        .execute(&state.db.pool)
        .await
        .unwrap();

        assert!(delete_account_data(&state, user_id).await.unwrap());

        for table in ["users", "user_links", "devices", "live_activities"] {
            let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table} should be empty");
        }

        state.db.pool.close().await;
        let _ = std::fs::remove_file(path);
    }
}
