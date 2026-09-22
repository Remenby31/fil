use anyhow::Result;
use sqlx::SqlitePool;
use tracing::debug;

pub async fn run(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            email TEXT,
            display_name TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(provider, provider_id)
        )",
    )
    .execute(pool)
    .await?;
    debug!("table 'users' ready");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            name TEXT NOT NULL,
            os TEXT,
            hostname TEXT,
            last_seen TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    debug!("table 'devices' ready");

    // Migrations must preserve logins that are in flight during a restart.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS oauth_states (
            state TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            cli_callback TEXT DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    debug!("table 'oauth_states' ready");
    let has_callback: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('oauth_states') WHERE name = 'cli_callback'",
    )
    .fetch_one(pool)
    .await?;
    if has_callback == 0 {
        sqlx::query("ALTER TABLE oauth_states ADD COLUMN cli_callback TEXT DEFAULT ''")
            .execute(pool)
            .await?;
    }

    // Link table: multiple auth providers → same user
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_links (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            provider TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(provider, provider_id)
        )",
    )
    .execute(pool)
    .await?;
    debug!("table 'user_links' ready");

    // Indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_devices_user_id ON devices(user_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_provider ON users(provider, provider_id)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS live_activities (
            activity_id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            push_token TEXT NOT NULL UNIQUE,
            environment TEXT NOT NULL CHECK(environment IN ('sandbox', 'production')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_live_activities_user_id ON live_activities(user_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_live_activities_session_id ON live_activities(session_id)",
    )
    .execute(pool)
    .await?;
    debug!("table 'live_activities' ready");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restart_preserves_pending_oauth_states() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run(&pool).await.unwrap();
        sqlx::query("INSERT INTO oauth_states (state, provider) VALUES ('pending', 'github')")
            .execute(&pool)
            .await
            .unwrap();
        run(&pool).await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM oauth_states WHERE state = 'pending'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn legacy_oauth_state_table_is_upgraded_without_losing_state_or_timestamp() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE oauth_states (
            state TEXT PRIMARY KEY, provider TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        ); INSERT INTO oauth_states (state, provider, created_at)
           VALUES ('legacy-pending', 'github', '2026-09-19 10:00:00');",
        )
        .execute(&pool)
        .await
        .unwrap();
        run(&pool).await.unwrap();
        run(&pool).await.unwrap();
        let row: (String, String, String) = sqlx::query_as(
            "SELECT provider, cli_callback, created_at FROM oauth_states WHERE state = 'legacy-pending'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(
            row,
            ("github".into(), "".into(), "2026-09-19 10:00:00".into())
        );
    }

    #[tokio::test]
    async fn migration_database_errors_are_propagated() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        pool.close().await;
        assert!(run(&pool).await.is_err());
    }
}
