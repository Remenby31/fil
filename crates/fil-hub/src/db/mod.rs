mod migrations;

use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use tracing::info;

#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn connect(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?;
        let path = options.get_filename();
        // SQLx assigns synthetic filenames to memory databases, and its
        // to_url_lossy() panics on those names. Read the already-validated URL
        // instead; never materialize a memory database as an empty disk file.
        let location = url
            .strip_prefix("sqlite://")
            .or_else(|| url.strip_prefix("sqlite:"))
            .unwrap_or(url);
        let (filename, query) = location.split_once('?').unwrap_or((location, ""));
        let in_memory = filename == ":memory:"
            || url::form_urlencoded::parse(query.as_bytes())
                .any(|(k, v)| k == "mode" && v == "memory");
        if !in_memory {
            fil_protocol::private_fs::open(path)?;
            // SQLite sidecars created by earlier releases can contain the same
            // sensitive data. Do not create sidecars; only repair existing ones.
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
                if sidecar.exists() {
                    fil_protocol::private_fs::open(&sidecar)?;
                }
            }
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        let db = Self { pool };
        db.run_migrations().await?;
        info!("database connected and migrated");

        Ok(db)
    }

    async fn run_migrations(&self) -> Result<()> {
        migrations::run(&self.pool).await
    }
}
