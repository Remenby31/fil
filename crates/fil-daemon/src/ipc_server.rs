use crate::config::DaemonConfig;
use crate::hub_quic::QuicHub;
use crate::hub_ws;
use crate::process_metadata;
use crate::session_manager::{ProxyCommand, SessionManager};
use anyhow::Result;
use fil_protocol::ipc::{self, DaemonMessage, FrameReader};
use fil_protocol::proto;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{info, warn};

pub async fn run(
    sock_path: &Path,
    sessions: Arc<SessionManager>,
    ws_tx: mpsc::Sender<proto::DaemonMessage>,
    quic_hub: QuicHub,
    config: DaemonConfig,
) -> Result<()> {
    let listener = UnixListener::bind(sock_path)?;
    let quic_hub = Arc::new(quic_hub);

    loop {
        let (stream, _) = listener.accept().await?;
        let sessions = sessions.clone();
        let ws_tx = ws_tx.clone();
        let quic_hub = quic_hub.clone();
        let config = config.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_proxy(stream, sessions, ws_tx, quic_hub, config).await {
                warn!(error = %e, "proxy session ended with error");
            }
        });
    }
}

async fn handle_proxy(
    stream: tokio::net::UnixStream,
    sessions: Arc<SessionManager>,
    ws_tx: mpsc::Sender<proto::DaemonMessage>,
    quic_hub: Arc<QuicHub>,
    _config: DaemonConfig,
) -> Result<()> {
    let peer_pid = process_metadata::peer_pid(&stream);
    let (mut reader, mut writer) = stream.into_split();

    // Read the Register message first
    let mut frame_reader = FrameReader::new();
    let reg = loop {
        let mut tmp = [0u8; 4096];
        let n = reader.read(&mut tmp).await?;
        if n == 0 {
            anyhow::bail!("proxy disconnected before Register");
        }
        frame_reader.feed(&tmp[..n]);
        if let Some((msg_type, payload)) = frame_reader.try_parse_frame() {
            if let Some(ipc::ProxyMessage::Register {
                session_id,
                shell,
                cwd,
                cols,
                rows,
            }) = FrameReader::parse_proxy_message(msg_type, payload)
            {
                break (session_id, shell, cwd, cols, rows);
            }
            anyhow::bail!("first message must be Register");
        }
    };

    let (session_id, shell, cwd, cols, rows) = reg;
    let metadata = peer_pid
        .map(|pid| process_metadata::snapshot(pid, &cwd, &shell))
        .unwrap_or_else(|| process_metadata::ProcessMetadata {
            created_at: chrono::Utc::now().timestamp(),
            cwd: cwd.clone(),
            command: shell.rsplit('/').next().unwrap_or(&shell).to_string(),
        });
    info!(session = %session_id, shell = %shell, "proxy registered");

    // Channel for daemon → proxy messages
    let (proxy_tx, mut proxy_rx) = mpsc::channel::<ProxyCommand>(256);

    // Register session
    sessions.add(
        session_id.clone(),
        shell.clone(),
        metadata.command.clone(),
        metadata.cwd.clone(),
        metadata.created_at,
        cols,
        rows,
        proxy_tx.clone(),
    );

    // Notify hub via WebSocket
    let created_msg = hub_ws::build_session_created(
        &session_id,
        &shell,
        &metadata.cwd,
        &metadata.command,
        metadata.created_at,
        cols,
        rows,
    );
    match ws_tx.try_send(created_msg) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            warn!(session = %session_id, "WebSocket control queue full; heartbeat will reconcile session state");
        }
        Err(TrySendError::Closed(_)) => {
            warn!(session = %session_id, "WebSocket control queue closed");
        }
    }

    // Open QUIC stream for this session
    info!(session = %session_id, "opening QUIC stream to hub...");
    let quic_tx = match quic_hub
        .open_session_stream(session_id.clone(), proxy_tx)
        .await
    {
        Ok(tx) => {
            info!(session = %session_id, "QUIC stream opened");
            Some(tx)
        }
        Err(e) => {
            warn!(error = %e, session = %session_id, "QUIC stream failed, session runs without remote");
            None
        }
    };

    // Bidirectional forwarding
    let sid = session_id.clone();
    let sessions_ref = sessions.clone();

    // Proxy → daemon (read IPC frames, forward output to QUIC)
    let read_task = async {
        let mut quic_tx = quic_tx;
        let mut quic_backpressured = false;
        loop {
            let mut tmp = [0u8; 16384];
            let n = reader.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            frame_reader.feed(&tmp[..n]);
            while let Some((msg_type, payload)) = frame_reader.try_parse_frame() {
                match msg_type {
                    ipc::MSG_OUTPUT => {
                        let send_result = quic_tx.as_ref().map(|tx| tx.try_send(payload));
                        match send_result {
                            Some(Ok(())) => quic_backpressured = false,
                            Some(Err(TrySendError::Full(_))) if !quic_backpressured => {
                                warn!(session = %sid, "QUIC output queue full; dropping remote output until it recovers");
                                quic_backpressured = true;
                            }
                            Some(Err(TrySendError::Full(_))) => {}
                            Some(Err(TrySendError::Closed(_))) => {
                                quic_tx = None;
                                quic_backpressured = false;
                            }
                            None => {}
                        }
                    }
                    ipc::MSG_RESIZE if payload.len() >= 4 => {
                        let cols = u16::from_be_bytes([payload[0], payload[1]]);
                        let rows = u16::from_be_bytes([payload[2], payload[3]]);
                        sessions_ref.update_size(&sid, u32::from(cols), u32::from(rows));
                    }
                    ipc::MSG_DESTROYED => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    };

    // Daemon → proxy (forward hub commands to proxy via IPC)
    let sid_write = session_id.clone();
    let sessions_write = sessions.clone();
    let write_task = async {
        let mut encode_buf = Vec::with_capacity(4096);
        while let Some(cmd) = proxy_rx.recv().await {
            info!(session = %sid_write, cmd = ?cmd, "daemon → proxy");
            encode_buf.clear();
            let msg = match cmd {
                ProxyCommand::Input(data) => DaemonMessage::Input(data),
                ProxyCommand::Resize { cols, rows } => {
                    sessions_write.update_size(&sid_write, u32::from(cols), u32::from(rows));
                    DaemonMessage::Resize { cols, rows }
                }
                ProxyCommand::ClientAttached => DaemonMessage::ClientAttached,
                ProxyCommand::ClientDetached => DaemonMessage::ClientDetached,
            };
            if msg.encode(&mut encode_buf).is_ok() && writer.write_all(&encode_buf).await.is_err() {
                break;
            }
        }
    };

    let sid_metadata = session_id.clone();
    let shell_metadata = shell.clone();
    let cwd_metadata = cwd.clone();
    let sessions_metadata = sessions.clone();
    let metadata_task = async move {
        let Some(peer_pid) = peer_pid else {
            std::future::pending::<()>().await;
            return;
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            let metadata = process_metadata::snapshot(peer_pid, &cwd_metadata, &shell_metadata);
            sessions_metadata.update_metadata(&sid_metadata, metadata.cwd, metadata.command);
        }
    };

    tokio::select! {
        _ = read_task => {},
        _ = write_task => {},
        _ = metadata_task => {},
    }

    // Cleanup
    info!(session = %session_id, "proxy disconnected");
    sessions.remove(&session_id);
    let destroyed_msg = hub_ws::build_session_destroyed(&session_id, 0);
    match ws_tx.try_send(destroyed_msg) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            warn!(session = %session_id, "WebSocket control queue full; heartbeat will reconcile session state");
        }
        Err(TrySendError::Closed(_)) => {
            warn!(session = %session_id, "WebSocket control queue closed");
        }
    }

    Ok(())
}
