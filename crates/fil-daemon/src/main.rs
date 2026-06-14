mod config;
mod hub;
mod pty;
mod setup;
mod terminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::DaemonConfig;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "fil", version, about = "The thread to your terminals.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Set up Fil: authenticate and configure your terminal
    Setup {
        /// Hub URL (default: http://localhost:3100)
        #[arg(long)]
        hub: Option<String>,
    },
    /// Remove Fil configuration and restore terminal settings
    Uninstall,
    /// Show version information
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("FIL_LOG")
                .unwrap_or_else(|_| "fil=info".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Setup { hub }) => setup::run_setup(hub).await,
        Some(Commands::Uninstall) => setup::run_uninstall(),
        Some(Commands::Version) => {
            println!("fil v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => run_proxy().await,
    }
}

async fn run_proxy() -> Result<()> {
    let config = DaemonConfig::load();
    let shell = pty::detect_shell();
    debug!(shell = %shell, "detected shell");

    let pty_process = pty::spawn_pty(&shell)?;
    info!(pid = pty_process.child_pid, "spawned shell");

    let session_id = Uuid::new_v4().to_string();

    if config.is_configured() {
        let hub_config = config.clone();
        let sid = session_id.clone();
        let shell_name = shell.clone();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        tokio::spawn(async move {
            let (hub_conn, outgoing_rx) =
                hub::HubConnection::new(&hub_config.hub_url, &hub_config.device_id);

            let tx = hub_conn.sender();

            let created_msg = hub::build_session_created(&sid, &shell_name, &cwd, 80, 24);
            tx.send(created_msg).await.ok();

            let hb_tx = tx.clone();
            let hb_device_id = hub_config.device_id.clone();
            let hb_sid = sid.clone();
            let hb_shell = shell_name.clone();
            let hb_cwd = cwd.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    let session_info = fil_protocol::proto::SessionInfo {
                        session_id: hb_sid.clone(),
                        shell: hb_shell.clone(),
                        cwd: hb_cwd.clone(),
                        created_at: 0,
                        cols: 80,
                        rows: 24,
                    };
                    let hb = hub::build_heartbeat(&hb_device_id, vec![session_info]);
                    if hb_tx.send(hb).await.is_err() {
                        break;
                    }
                }
            });

            let (incoming_tx, _incoming_rx) = mpsc::channel(256);
            if let Err(e) = hub_conn.connect_and_run(outgoing_rx, incoming_tx).await {
                warn!(error = %e, "hub connection failed — running in offline mode");
            }
        });

        debug!("hub connection started in background");
    } else {
        debug!("no config found — running in offline mode (run 'fil setup' first)");
    }

    let _raw_guard = terminal::RawModeGuard::new()?;
    let exit_code = pty::proxy_loop(pty_process).await?;

    debug!(exit_code, "shell exited");
    std::process::exit(exit_code);
}
