mod config;
mod hub;
mod pty;
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
        Some(Commands::Setup { hub }) => run_setup(hub).await,
        Some(Commands::Version) => {
            println!("fil v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => run_proxy().await,
    }
}

async fn run_setup(hub_url: Option<String>) -> Result<()> {
    println!("\n  fil v{}\n", env!("CARGO_PKG_VERSION"));

    let mut config = DaemonConfig::load();

    if let Some(url) = hub_url {
        config.hub_url = url;
    }

    println!("  Opening browser for GitHub authentication...\n");

    let auth_url = format!("{}/auth/github/start", config.hub_url);
    if open::that(&auth_url).is_err() {
        println!("  Open this URL in your browser:");
        println!("  {auth_url}\n");
    }

    println!("  Waiting for authentication...");
    println!("  (Paste the JSON response from the browser here)");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    #[derive(serde::Deserialize)]
    struct AuthResponse {
        token: String,
        user_id: String,
    }

    let auth: AuthResponse = serde_json::from_str(input.trim())?;
    config.token = auth.token.clone();

    // Register device
    let hostname = gethostname::gethostname()
        .to_string_lossy()
        .to_string();
    config.device_name = hostname.clone();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/devices", config.hub_url))
        .header("Authorization", format!("Bearer {}", config.token))
        .json(&serde_json::json!({
            "name": hostname,
            "os": std::env::consts::OS,
            "hostname": hostname,
        }))
        .send()
        .await?;

    #[derive(serde::Deserialize)]
    struct DeviceResp {
        id: String,
    }

    let device: DeviceResp = resp.json().await?;
    config.device_id = device.id;

    config.save()?;

    println!("\n  \x1b[32m✓\x1b[0m Signed in");
    println!("  \x1b[32m✓\x1b[0m Device \x1b[1m{}\x1b[0m registered", config.device_name);
    println!("  \x1b[32m✓\x1b[0m Config saved to {:?}", DaemonConfig::config_path());
    println!("\n  Restart your terminal to activate fil.\n");

    Ok(())
}

async fn run_proxy() -> Result<()> {
    let config = DaemonConfig::load();
    let shell = pty::detect_shell();
    debug!(shell = %shell, "detected shell");

    let pty_process = pty::spawn_pty(&shell)?;
    info!(pid = pty_process.child_pid, "spawned shell");

    let session_id = Uuid::new_v4().to_string();

    // Start hub connection in background if configured
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

            // Send session created
            let created_msg = hub::build_session_created(
                &sid, &shell_name, &cwd, 80, 24,
            );
            tx.send(created_msg).await.ok();

            // Start heartbeat
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

    // Put stdin in raw mode
    let _raw_guard = terminal::RawModeGuard::new()?;

    // Run the proxy
    let exit_code = pty::proxy_loop(pty_process).await?;

    debug!(exit_code, "shell exited");
    std::process::exit(exit_code);
}
