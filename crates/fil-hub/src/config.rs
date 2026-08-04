use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct ApnsConfig {
    pub team_id: String,
    pub key_id: String,
    pub private_key: String,
    pub topic: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: SocketAddr,
    pub database_url: String,
    pub jwt_secret: String,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub apple_client_id: String,
    pub apple_team_id: String,
    pub apple_key_id: String,
    pub public_url: String,
    pub quic_port: u16,
    pub data_dir: String,
    /// Reject unauthenticated v1 QUIC attaches. Defaults to false so a hub can
    /// be deployed before the app that mints tickets; flip it once the fleet
    /// has upgraded, which fully closes the data plane.
    pub require_attach_ticket: bool,
    pub apns: Option<ApnsConfig>,
}

impl Config {
    pub fn from_env() -> Self {
        let port: u16 = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3100);

        Self {
            addr: SocketAddr::from(([0, 0, 0, 0], port)),
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:fil-hub.db?mode=rwc".to_string()),
            jwt_secret: std::env::var("JWT_SECRET").unwrap_or_else(|_| {
                tracing::warn!(
                    "JWT_SECRET not set — using random secret (not suitable for production)"
                );
                uuid::Uuid::new_v4().to_string()
            }),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default(),
            apple_client_id: std::env::var("APPLE_CLIENT_ID").unwrap_or_default(),
            apple_team_id: std::env::var("APPLE_TEAM_ID").unwrap_or_default(),
            apple_key_id: std::env::var("APPLE_KEY_ID").unwrap_or_default(),
            public_url: std::env::var("PUBLIC_URL")
                .unwrap_or_else(|_| format!("http://localhost:{port}")),
            quic_port: std::env::var("QUIC_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(16433),
            data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| ".".to_string()),
            require_attach_ticket: std::env::var("FIL_REQUIRE_ATTACH_TICKET")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            apns: apns_from_env(),
        }
    }
}

fn apns_from_env() -> Option<ApnsConfig> {
    let team_id = nonempty_env("APNS_TEAM_ID")?;
    let key_id = nonempty_env("APNS_KEY_ID")?;
    let topic = std::env::var("APNS_TOPIC").unwrap_or_else(|_| "sh.fil.app".to_string());
    let private_key = if let Ok(value) = std::env::var("APNS_PRIVATE_KEY") {
        value.replace("\\n", "\n")
    } else {
        let path = std::env::var("APNS_PRIVATE_KEY_PATH").ok()?;
        match std::fs::read_to_string(&path) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, %path, "unable to read APNs private key");
                return None;
            }
        }
    };
    Some(ApnsConfig {
        team_id,
        key_id,
        private_key,
        topic,
    })
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
