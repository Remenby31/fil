mod migrations;

use anyhow::Result;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::info;

#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(url)
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
