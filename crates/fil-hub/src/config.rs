use std::net::SocketAddr;

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
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| {
                    tracing::warn!("JWT_SECRET not set — using random secret (not suitable for production)");
                    uuid::Uuid::new_v4().to_string()
                }),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default(),
            apple_client_id: std::env::var("APPLE_CLIENT_ID").unwrap_or_default(),
            apple_team_id: std::env::var("APPLE_TEAM_ID").unwrap_or_default(),
            apple_key_id: std::env::var("APPLE_KEY_ID").unwrap_or_default(),
            public_url: std::env::var("PUBLIC_URL")
                .unwrap_or_else(|_| format!("http://localhost:{port}")),
        }
    }
}
