mod apns;
mod auth;
mod client_ws;
mod config;
mod data_ws;
mod db;
mod quic;
mod quic_certs;
mod routes;
mod security;
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
    config.validate()?;
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
    let quic_router = state.quic_router.clone();
    let require_ticket = state.config.require_attach_ticket;
    // Fail startup if the data plane cannot bind. A healthy HTTP endpoint is
    // misleading when every terminal connection is broken.
    let quic_addr = SocketAddr::from(([0, 0, 0, 0], quic_port));
    let certs = quic_certs::QuicCerts::load_or_generate(&data_dir)?;
    let quic_endpoint = quic::bind_quic_server(quic_addr, certs)?;
    let mut activity_updates = state.sessions.subscribe();
    let activity_db = state.db.pool.clone();
    let activity_apns = state.apns.clone();
    tokio::spawn(async move {
        loop {
            match activity_updates.recv().await {
                Ok(update) => activity_apns.deliver_update(&activity_db, &update).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
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
        .route("/privacy", get(|| async { axum::response::Html(include_str!("../static/privacy.html")) }))
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
        .route("/sessions/{session_id}/daemon-ticket", post(routes::create_daemon_ticket))
        .route("/live-activities", post(routes::register_live_activity))
        .route(
            "/live-activities/{activity_id}",
            delete(routes::delete_live_activity),
        )
        // WebSockets for daemon and authenticated iOS clients
        .route("/ws", get(ws::ws_handler))
        .route("/ws/client", get(client_ws::client_ws_handler))
        .route("/ws/data/{session_id}", get(data_ws::data_ws_handler))
        // Middleware
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<axum::body::Body>| {
            // Query strings can contain OAuth codes or WebSocket credentials.
            tracing::info_span!("request", method = %request.method(), path = request.uri().path())
        }))
        .layer(CorsLayer::new())
        .layer(axum::middleware::from_fn_with_state(state.clone(), security::protect))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP listening on {}", addr);

    // Start QUIC server for data plane
    tokio::spawn(async move {
        if let Err(e) = quic::start_quic_server(
            quic_endpoint,
            quic_sessions,
            quic_tickets,
            require_ticket,
            quic_router,
        )
        .await
        {
            error!(error = %e, "QUIC server failed");
        }
    });

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
