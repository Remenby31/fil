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

    // Drop and recreate oauth_states to add cli_callback column
    sqlx::query("DROP TABLE IF EXISTS oauth_states")
        .execute(pool)
        .await?;
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

    // Indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_devices_user_id ON devices(user_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_provider ON users(provider, provider_id)")
        .execute(pool)
        .await?;

    Ok(())
}
