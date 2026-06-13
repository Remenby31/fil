use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("fil_hub=debug,tower_http=debug")
        .init();

    info!("fil-hub v{} — starting", env!("CARGO_PKG_VERSION"));

    // Phase 2: HTTP server, auth, device registration
    info!("hub is a skeleton — implementation coming in Phase 2");

    Ok(())
}
