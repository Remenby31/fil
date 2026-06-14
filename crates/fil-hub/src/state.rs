use crate::config::Config;
use crate::db::Database;
use crate::sessions::SessionRegistry;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: Database,
    pub sessions: SessionRegistry,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = Database::connect(&config.database_url).await?;
        let sessions = SessionRegistry::new();

        Ok(Self {
            config,
            db,
            sessions,
        })
    }
}
