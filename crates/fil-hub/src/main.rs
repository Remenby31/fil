mod apns;
mod auth;
mod client_ws;
mod config;
mod db;
mod quic;
mod quic_certs;
mod routes;
mod sessions;
mod state;
mod tickets;
mod ws;

use axum::Router;
use axum::routing::{delete, get, post};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

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
    let quic_port = config.quic_port;
    let data_dir = config.data_dir.clone();

    info!(
        "fil-hub v{} — starting on {}",
        env!("CARGO_PKG_VERSION"),
        addr
    );

    let state = AppState::new(config).await?;
    let quic_sessions = state.sessions.clone();
    let quic_tickets = state.tickets.clone();
    let require_ticket = state.config.require_attach_ticket;
    let mut activity_updates = state.sessions.subscribe();
    let activity_db = state.db.pool.clone();
    let activity_apns = state.apns.clone();
    tokio::spawn(async move {
        while let Ok(update) = activity_updates.recv().await {
            activity_apns.deliver_update(&activity_db, &update).await;
        }
    });

    // Heartbeat reaper. The daemon heartbeats every 5s, so 20s of silence
    // means it is gone. Without this the hub had no periodic task at all and
    // `last_heartbeat` was written but never compared, leaving a slept or
    // force-quit Mac advertised as Online indefinitely.
    let reaper_sessions = state.sessions.clone();
    tokio::spawn(async move {
        const REAP_INTERVAL: Duration = Duration::from_secs(10);
        const MAX_SILENCE_SECS: i64 = 20;
        let mut ticker = tokio::time::interval(REAP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let reaped =
                reaper_sessions.reap_stale_devices(chrono::Duration::seconds(MAX_SILENCE_SECS));
            if reaped > 0 {
                info!(count = reaped, "marked stale devices offline");
            }
        }
    });

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
        .route("/account", delete(routes::delete_account))
        .route("/sessions", get(routes::list_sessions))
        .route(
            "/sessions/{session_id}/ticket",
            post(routes::create_session_ticket),
        )
        .route("/live-activities", post(routes::register_live_activity))
        .route(
            "/live-activities/{activity_id}",
            delete(routes::delete_live_activity),
        )
        // WebSockets for daemon and authenticated iOS clients
        .route("/ws", get(ws::ws_handler))
        .route("/ws/client", get(client_ws::client_ws_handler))
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP listening on {}", addr);

    // Start QUIC server for data plane
    let quic_addr = SocketAddr::from(([0, 0, 0, 0], quic_port));
    tokio::spawn(async move {
        match quic_certs::QuicCerts::load_or_generate(&data_dir) {
            Ok(certs) => {
                info!(fingerprint = %certs.fingerprint(), "QUIC cert fingerprint");
                if let Err(e) = quic::start_quic_server(quic_addr, certs, quic_sessions, quic_tickets, require_ticket)
                    .await {
                    error!(error = %e, "QUIC server failed");
                }
            }
            Err(e) => {
                error!(error = %e, "failed to generate QUIC certificates");
            }
        }
    });

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
