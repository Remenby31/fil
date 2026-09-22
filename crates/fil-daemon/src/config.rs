use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            hub_url: default_hub_url(),
            quic_port: default_quic_port(),
            quic_host: String::new(),
            token: String::new(),
            device_id: String::new(),
            device_name: String::new(),
        }
    }
}

impl DaemonConfig {
    pub fn config_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("FIL_CONFIG_DIR") {
            return PathBuf::from(dir);
        }
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

    pub fn is_configured(&self) -> bool {
        !self.token.is_empty() && !self.device_id.is_empty()
    }

    pub fn effective_quic_host(&self) -> String {
        if !self.quic_host.is_empty() {
            return self.quic_host.clone();
        }
        // Derive from hub_url: fil.remenby.fr → quic.fil.remenby.fr
        let url = url::Url::parse(&self.hub_url).ok();
        let host = url
            .as_ref()
            .and_then(url::Url::host_str)
            .unwrap_or("localhost");
        if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[')
        {
            host.trim_matches(['[', ']']).to_string()
        } else {
            format!("quic.{host}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_empty_config_and_local_hosts_are_not_prefixed() {
        let empty: DaemonConfig = toml::from_str("").unwrap();
        assert_eq!(DaemonConfig::default().quic_port, empty.quic_port);
        assert_eq!(DaemonConfig::default().hub_url, empty.hub_url);
        for (hub, host) in [
            ("http://localhost:3100", "localhost"),
            ("http://127.0.0.1:3100", "127.0.0.1"),
            ("https://fil.example/", "quic.fil.example"),
        ] {
            assert_eq!(
                DaemonConfig {
                    hub_url: hub.into(),
                    ..Default::default()
                }
                .effective_quic_host(),
                host
            );
        }
    }
}
