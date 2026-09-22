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
    pub quic_certificate: String,
    pub quic_router: std::sync::Arc<crate::quic::QuicRouter>,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = Database::connect(&config.database_url).await?;
        let sessions = SessionRegistry::new();
        let apns = ApnsClient::new(config.apns.clone())?;
        use base64::Engine;
        let certs = crate::quic_certs::QuicCerts::load_or_generate(&config.data_dir)?;
        let quic_certificate = base64::engine::general_purpose::STANDARD.encode(&certs.cert_der);

        Ok(Self {
            config,
            db,
            sessions,
            apns,
            tickets: TicketStore::new(),
            quic_certificate,
            quic_router: std::sync::Arc::new(crate::quic::QuicRouter::new()),
        })
    }
}

#[cfg(test)]
pub async fn test_state() -> AppState {
    AppState::new(Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".into(),
        jwt_secret: "audit-test-secret-not-for-production".into(),
        github_client_id: "test-client".into(),
        github_client_secret: "test-secret".into(),
        apple_client_id: "sh.fil.app".into(),
        public_url: "http://localhost".into(),
        quic_port: 0,
        data_dir: std::env::temp_dir()
            .join(format!("fil-test-{}", uuid::Uuid::new_v4()))
            .display()
            .to_string(),
        require_attach_ticket: true,
        apns: None,
    })
    .await
    .unwrap()
}
