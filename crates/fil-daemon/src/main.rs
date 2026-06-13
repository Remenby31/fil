mod pty;
mod terminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{debug, info};

#[derive(Parser)]
#[command(name = "fil", version, about = "The thread to your terminals.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Set up Fil: authenticate and configure your terminal
    Setup,
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
        Some(Commands::Setup) => {
            info!("fil setup — coming in Phase 2");
            println!("fil setup is not yet implemented. Coming soon!");
            Ok(())
        }
        Some(Commands::Version) => {
            println!("fil v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            // Default: launch as PTY proxy (the main use case)
            debug!("starting fil as PTY proxy");
            run_proxy().await
        }
    }
}

async fn run_proxy() -> Result<()> {
    let shell = pty::detect_shell();
    debug!(shell = %shell, "detected shell");

    let pty_process = pty::spawn_pty(&shell)?;
    info!(pid = pty_process.child_pid, "spawned shell");

    // Put our stdin in raw mode so keystrokes pass through unmodified
    let _raw_guard = terminal::RawModeGuard::new()?;

    // Run the proxy: forward bytes between stdin/stdout and the PTY
    let exit_code = pty::proxy_loop(pty_process).await?;

    debug!(exit_code, "shell exited");
    std::process::exit(exit_code);
}
