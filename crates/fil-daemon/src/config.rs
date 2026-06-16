use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonConfig {
    #[serde(default = "default_hub_url")]
    pub hub_url: String,
    #[serde(default)]
    pub quic_host: String,
    #[serde(default = "default_quic_port")]
    pub quic_port: u16,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
}

fn default_hub_url() -> String {
    "http://localhost:3100".to_string()
}

fn default_quic_port() -> u16 {
    16433
}

impl DaemonConfig {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("fil")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), content)?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        !self.token.is_empty() && !self.device_id.is_empty()
    }

    pub fn effective_quic_host(&self) -> String {
        if !self.quic_host.is_empty() {
            return self.quic_host.clone();
        }
        // Derive from hub_url: fil.remenby.fr → quic.fil.remenby.fr
        let host = self.hub_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("localhost");
        format!("quic.{host}")
    }
}
