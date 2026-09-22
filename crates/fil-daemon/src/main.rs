mod config;
mod hub_quic;
mod hub_ws;
mod ipc_server;
mod process_metadata;
mod session_manager;

use anyhow::Result;
use config::DaemonConfig;
use session_manager::SessionManager;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("FIL_LOG").unwrap_or_else(|_| "fil_daemon=info".to_string()))
        .with_writer(std::io::stderr)
        .init();

    fil_protocol::private_fs::directory(&DaemonConfig::config_dir())?;
    // Protect historical bytes on a binary-only upgrade, without re-pairing.
    #[cfg(target_os = "macos")]
    {
        let legacy_log = std::path::Path::new("/tmp/fil-daemon.log");
        fil_protocol::private_fs::harden_owned_legacy_log(legacy_log)?;
    }
    let current_log = DaemonConfig::config_dir().join("daemon.log");
    if current_log.exists() {
        fil_protocol::private_fs::read(&current_log)?;
    }
    if DaemonConfig::config_path().exists() {
        fil_protocol::private_fs::read(&DaemonConfig::config_path())?;
    }
    let config = DaemonConfig::load();
    if !config.is_configured() {
        anyhow::bail!("fil is not configured. Run `fil setup` first.");
    }
    let hub_origin = fil_protocol::tls::hub_url(&config.hub_url)
        .map_err(anyhow::Error::msg)?
        .origin()
        .ascii_serialization();

    info!(
        device = %config.device_name,
        hub = %hub_origin,
        "fil-daemon starting"
    );

    // Write pidfile
    let pid_path = DaemonConfig::config_dir().join("daemon.pid");
    std::fs::write(&pid_path, std::process::id().to_string())?;

    let session_manager = Arc::new(SessionManager::new());

    // Start hub connections (WebSocket + QUIC)
    let ws_tx = hub_ws::start(config.clone(), session_manager.clone()).await;
    let quic_endpoint = hub_quic::start(config.clone()).await?;

    // Start IPC server (accepts proxy connections)
    let sock_path = DaemonConfig::config_dir().join("daemon.sock");
    if sock_path.exists() {
        std::fs::remove_file(&sock_path).ok();
    }

    info!(path = %sock_path.display(), "listening for proxies");

    let result = ipc_server::run(
        &sock_path,
        session_manager.clone(),
        ws_tx,
        quic_endpoint,
        config,
    )
    .await;

    // Cleanup
    std::fs::remove_file(&sock_path).ok();
    std::fs::remove_file(&pid_path).ok();

    result
}
