use crate::apns::ApnsClient;
use crate::config::Config;
use crate::db::Database;
use crate::sessions::SessionRegistry;
use crate::tickets::TicketStore;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: Database,
    pub sessions: SessionRegistry,
    pub apns: ApnsClient,
    pub tickets: TicketStore,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = Database::connect(&config.database_url).await?;
        let sessions = SessionRegistry::new();
        let apns = ApnsClient::new(config.apns.clone())?;

        Ok(Self {
            config,
            db,
            sessions,
            apns,
            tickets: TicketStore::new(),
        })
    }
}
