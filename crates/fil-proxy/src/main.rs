mod config;
mod ipc;
mod pty;
mod setup;
mod terminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::DaemonConfig;
use std::process::ExitCode;
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
    /// Show daemon and connection status
    Status,
    /// Show version information
    Version,
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();

    let default_level = if cli.command.is_some() {
        "fil=info"
    } else {
        "fil=off"
    };
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("FIL_LOG").unwrap_or_else(|_| default_level.to_string()))
        .with_writer(std::io::stderr)
        .init();

    match cli.command {
        Some(Commands::Setup { hub }) => {
            setup::run_setup(hub).await?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Uninstall) => {
            setup::run_uninstall()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Status) => run_status(),
        Some(Commands::Version) => {
            println!("fil v{}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        None => run_proxy(),
    }
}

fn run_status() -> Result<ExitCode> {
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m status\n");

    let config = DaemonConfig::load();

    // Config
    if config.is_configured() {
        println!(
            "  Config:   \x1b[32m✓\x1b[0m {}",
            DaemonConfig::config_path().display()
        );
        println!("  Hub:      {}", config.hub_url);
        println!(
            "  Device:   {} ({})",
            config.device_name,
            &config.device_id[..8]
        );
    } else {
        println!("  Config:   \x1b[31m✗\x1b[0m not configured (run `fil setup`)");
        println!();
        return Ok(ExitCode::SUCCESS);
    }

    // Daemon
    let sock_path = DaemonConfig::socket_path();
    if sock_path.exists() {
        if std::os::unix::net::UnixStream::connect(&sock_path).is_ok() {
            println!("  Daemon:   \x1b[32m✓\x1b[0m running");
        } else {
            println!("  Daemon:   \x1b[33m!\x1b[0m socket exists but not responding");
        }
    } else {
        println!("  Daemon:   \x1b[31m✗\x1b[0m not running");
    }

    println!();
    Ok(ExitCode::SUCCESS)
}

fn run_proxy() -> Result<ExitCode> {
    let config = DaemonConfig::load();
    let shell = pty::detect_shell();
    let pty_process = pty::spawn_pty(&shell)?;
    let session_id = Uuid::new_v4().to_string();
    let cwd = std::env::current_dir()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let registration = ipc::ProxyRegistration::new(session_id, shell, cwd);
    let (cols, rows) = pty::current_window_dimensions();

    let daemon = if config.is_configured() {
        ipc::connect_and_register(&registration, cols, rows)
    } else {
        None
    };

    let _raw_guard = terminal::RawModeGuard::new()?;
    let exit_code = pty::proxy_loop_sync(
        &pty_process,
        daemon,
        config.is_configured().then_some(&registration),
    )?;

    Ok(ExitCode::from(u8::try_from(exit_code).unwrap_or(1)))
}
