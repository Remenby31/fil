use crate::config::DaemonConfig;
use crate::session_manager::SessionManager;
use anyhow::{Context, Result};
use fil_protocol::proto;
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use std::sync::Arc;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};
use url::Url;

pub async fn start(
    config: DaemonConfig,
    sessions: Arc<SessionManager>,
) -> mpsc::Sender<proto::DaemonMessage> {
    let (tx, rx) = mpsc::channel::<proto::DaemonMessage>(512);

    let hb_tx = tx.clone();
    let hb_config = config.clone();

    // WebSocket connection task (with reconnection)
    tokio::spawn(async move {
        run_ws_loop(config, rx).await;
    });

    // Aggregated heartbeat task
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            let session_infos = sessions.all_session_infos();
            let msg = build_heartbeat(&hb_config.device_id, session_infos);
            match hb_tx.try_send(msg) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => break,
            }
        }
    });

    tx
}

/// Matches the QUIC data path. A 30s cap meant a device could stay "offline"
/// in the app for half a minute after a brief network hiccup.
const MAX_BACKOFF_SECS: u64 = 5;

async fn run_ws_loop(config: DaemonConfig, mut outgoing: mpsc::Receiver<proto::DaemonMessage>) {
    let mut backoff = 1u64;

    loop {
        match connect_ws(&config).await {
            Ok((mut ws_sender, mut ws_receiver)) => {
                info!("WebSocket connected to hub");
                backoff = 1;

                let connected = async {
                    loop {
                        tokio::select! {
                            msg = outgoing.recv() => {
                                let Some(msg) = msg else { return; };
                                let mut buf = Vec::new();
                                msg.encode(&mut buf).ok();
                                if ws_sender.send(Message::Binary(buf.into())).await.is_err() {
                                    return;
                                }
                            }
                            msg = ws_receiver.next() => {
                                match msg {
                                    Some(Ok(Message::Binary(_data))) => {
                                        // Hub messages handled here if needed
                                    }
                                    Some(Ok(Message::Close(_))) | None => return,
                                    Some(Err(e)) => {
                                        warn!(error = %e, "WebSocket error");
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                };

                connected.await;
                warn!("WebSocket disconnected, reconnecting...");
            }
            Err(e) => {
                warn!(error = %e, backoff_s = backoff, "WebSocket connection failed");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
    }
}

async fn connect_ws(
    config: &DaemonConfig,
) -> Result<(
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
)> {
    let ws_url = build_ws_url(&config.hub_url, &config.device_id)?;
    let (ws_stream, _) = connect_async(ws_url.as_str())
        .await
        .context("WebSocket connect failed")?;
    Ok(ws_stream.split())
}

fn build_ws_url(hub_url: &str, device_id: &str) -> Result<Url> {
    let mut url = Url::parse(hub_url)?;
    match url.scheme() {
        "http" => url.set_scheme("ws").ok(),
        "https" => url.set_scheme("wss").ok(),
        _ => None,
    };
    url.set_path("/ws");
    url.query_pairs_mut()
        .append_pair("device_id", device_id)
        .append_pair("device_token", device_id);
    Ok(url)
}

pub fn build_session_created(
    session_id: &str,
    shell: &str,
    cwd: &str,
    command: &str,
    created_at: i64,
    cols: u32,
    rows: u32,
) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::SessionCreated(
            proto::SessionCreated {
                session_id: session_id.to_string(),
                device_id: String::new(),
                shell: shell.to_string(),
                cwd: cwd.to_string(),
                created_at,
                cols,
                rows,
                command: command.to_string(),
            },
        )),
    }
}

pub fn build_session_destroyed(session_id: &str, exit_code: i32) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::SessionDestroyed(
            proto::SessionDestroyed {
                session_id: session_id.to_string(),
                device_id: String::new(),
                exit_code,
                destroyed_at: chrono::Utc::now().timestamp(),
            },
        )),
    }
}

fn build_heartbeat(device_id: &str, sessions: Vec<proto::SessionInfo>) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::Heartbeat(
            proto::Heartbeat {
                device_id: device_id.to_string(),
                sessions,
                timestamp: chrono::Utc::now().timestamp(),
            },
        )),
    }
}
