use anyhow::Result;
use quinn::{Endpoint, RecvStream, SendStream};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::quic_certs::QuicCerts;
use crate::sessions::{SessionInfo, SessionRegistry, SessionStatus};

/// A connected client (iOS app) watching a session
struct AttachedClient {
    sender: mpsc::Sender<Vec<u8>>,
}

/// Routes bytes between daemons and attached clients
pub struct QuicRouter {
    /// session_id → list of attached clients
    clients: Arc<RwLock<HashMap<String, Vec<AttachedClient>>>>,
    /// device_id → sender to daemon's data stream
    daemon_inputs: Arc<RwLock<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
}

impl QuicRouter {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            daemon_inputs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn forward_to_clients(&self, session_id: &str, data: &[u8]) {
        let clients = self.clients.read().await;
        if let Some(senders) = clients.get(session_id) {
            for client in senders {
                client.sender.send(data.to_vec()).await.ok();
            }
        }
    }

    pub async fn attach_client(
        &self,
        session_id: &str,
    ) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel(512);
        let mut clients = self.clients.write().await;
        clients
            .entry(session_id.to_string())
            .or_default()
            .push(AttachedClient { sender: tx });
        info!(session_id = %session_id, "client attached to session");
        rx
    }

    pub async fn detach_clients(&self, session_id: &str) {
        let mut clients = self.clients.write().await;
        clients.remove(session_id);
    }

    pub async fn register_daemon_input(
        &self,
        session_id: &str,
        tx: mpsc::Sender<Vec<u8>>,
    ) {
        let mut inputs = self.daemon_inputs.write().await;
        inputs.insert(session_id.to_string(), tx);
    }

    pub async fn send_to_daemon(&self, session_id: &str, data: &[u8]) {
        let inputs = self.daemon_inputs.read().await;
        if let Some(tx) = inputs.get(session_id) {
            tx.send(data.to_vec()).await.ok();
        }
    }

    pub async fn unregister_daemon(&self, session_id: &str) {
        let mut inputs = self.daemon_inputs.write().await;
        inputs.remove(session_id);
    }
}

pub async fn start_quic_server(
    addr: SocketAddr,
    certs: QuicCerts,
    sessions: SessionRegistry,
) -> Result<()> {
    let cert_chain = vec![rustls::pki_types::CertificateDer::from(certs.cert_der)];
    let key = rustls::pki_types::PrivateKeyDer::try_from(certs.key_der)
        .map_err(|e| anyhow::anyhow!("invalid private key: {e}"))?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;

    server_crypto.alpn_protocols = vec![b"fil".to_vec()];

    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));

    let endpoint = Endpoint::server(server_config, addr)?;
    info!(addr = %addr, "QUIC server listening");

    let router = Arc::new(QuicRouter::new());

    while let Some(incoming) = endpoint.accept().await {
        let router = router.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let remote = conn.remote_address();
                    info!(remote = %remote, "QUIC connection accepted");
                    handle_connection(conn, router, sessions).await;
                }
                Err(e) => {
                    warn!(error = %e, "QUIC connection failed");
                }
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    conn: quinn::Connection,
    router: Arc<QuicRouter>,
    sessions: SessionRegistry,
) {
    let remote = conn.remote_address();

    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let router = router.clone();
                let sessions = sessions.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_stream(send, recv, router, sessions).await {
                        debug!(error = %e, "stream ended");
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                info!(remote = %remote, "QUIC connection closed");
                break;
            }
            Err(e) => {
                warn!(remote = %remote, error = %e, "QUIC accept error");
                break;
            }
        }
    }
}

async fn handle_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    router: Arc<QuicRouter>,
    sessions: SessionRegistry,
) -> Result<()> {
    // First message identifies the stream type
    let mut header = [0u8; 1];
    recv.read_exact(&mut header).await?;

    match header[0] {
        // 0x01 = Daemon data stream (PTY output)
        0x01 => {
            // Read session_id (length-prefixed)
            let mut len_buf = [0u8; 2];
            recv.read_exact(&mut len_buf).await?;
            let sid_len = u16::from_be_bytes(len_buf) as usize;
            let mut sid_buf = vec![0u8; sid_len];
            recv.read_exact(&mut sid_buf).await?;
            let session_id = String::from_utf8(sid_buf)?;

            debug!(session_id = %session_id, "daemon data stream opened");

            // Register daemon input channel
            let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(256);
            router.register_daemon_input(&session_id, input_tx).await;

            // Bidirectional: read PTY output, write client input
            let router_fwd = router.clone();
            let sid_fwd = session_id.clone();

            // Forward daemon output to attached clients
            let read_task = async {
                let mut buf = vec![0u8; 16384];
                loop {
                    match recv.read(&mut buf).await {
                        Ok(Some(n)) => {
                            router_fwd.forward_to_clients(&sid_fwd, &buf[..n]).await;
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            };

            // Forward client input to daemon
            let write_task = async {
                while let Some(data) = input_rx.recv().await {
                    if send.write_all(&data).await.is_err() {
                        break;
                    }
                }
            };

            tokio::select! {
                _ = read_task => {},
                _ = write_task => {},
            }

            router.unregister_daemon(&session_id).await;
            debug!(session_id = %session_id, "daemon data stream closed");
        }

        // 0x02 = Client attach (iOS app watching a session)
        0x02 => {
            // Read session_id
            let mut len_buf = [0u8; 2];
            recv.read_exact(&mut len_buf).await?;
            let sid_len = u16::from_be_bytes(len_buf) as usize;
            let mut sid_buf = vec![0u8; sid_len];
            recv.read_exact(&mut sid_buf).await?;
            let session_id = String::from_utf8(sid_buf)?;

            debug!(session_id = %session_id, "client attached to session");

            // Subscribe to session output
            let mut output_rx = router.attach_client(&session_id).await;

            // Forward output to client
            let send_task = async move {
                while let Some(data) = output_rx.recv().await {
                    if send.write_all(&data).await.is_err() {
                        break;
                    }
                }
            };

            // Forward client input to daemon
            let router_input = router.clone();
            let sid_input = session_id.clone();
            let recv_task = async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    match recv.read(&mut buf).await {
                        Ok(Some(n)) => {
                            router_input.send_to_daemon(&sid_input, &buf[..n]).await;
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            };

            tokio::select! {
                _ = send_task => {},
                _ = recv_task => {},
            }

            router.detach_clients(&session_id).await;
            debug!(session_id = %session_id, "client detached");
        }

        other => {
            warn!(stream_type = other, "unknown stream type");
        }
    }

    Ok(())
}
