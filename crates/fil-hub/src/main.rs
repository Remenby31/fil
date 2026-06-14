mod auth;
mod config;
mod db;
mod routes;
mod sessions;
mod state;
mod ws;

use axum::routing::{delete, get, post};
use axum::Router;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("FIL_LOG")
                .unwrap_or_else(|_| "fil_hub=debug,tower_http=debug".to_string()),
        )
        .init();

    let config = Config::from_env();
    let addr = config.addr;

    info!("fil-hub v{} — starting on {}", env!("CARGO_PKG_VERSION"), addr);

    let state = AppState::new(config).await?;

    let app = Router::new()
        // Public routes
        .route("/health", get(routes::health_check))
        .route("/auth/github/start", get(auth::github_auth_start))
        .route("/auth/github/callback", get(auth::github_auth_callback))
        .route("/auth/apple/callback", post(auth::apple_auth_callback))
        // Authenticated routes
        .route("/devices", post(routes::register_device))
        .route("/devices", get(routes::list_devices))
        .route("/devices/{device_id}", delete(routes::delete_device))
        .route("/sessions", get(routes::list_sessions))
        // WebSocket for daemon connections
        .route("/ws", get(ws::ws_handler))
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("hub shut down gracefully");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received");
}
