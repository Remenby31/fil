use anyhow::Result;
use quinn::{Endpoint, RecvStream, SendStream};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

use crate::quic_certs::QuicCerts;
use crate::sessions::SessionRegistry;
use crate::tickets::TicketStore;

const FRAME_INPUT: u8 = 0x00;
const FRAME_RESIZE: u8 = 0x01;
const CLIENT_FRAME_DETACH: u8 = 0x02;
const MAX_INPUT_FRAME_BYTES: usize = 1024 * 1024;
const QUIC_IDLE_TIMEOUT_SECS: u64 = 15;
const QUIC_KEEP_ALIVE_SECS: u64 = 5;
/// A hex-encoded 32-byte ticket is 64 chars; the cap only bounds a hostile peer.
const MAX_TICKET_BYTES: usize = 128;

/// A connected client (iOS app) watching a session
struct AttachedClient {
    id: u64,
    sender: mpsc::Sender<Vec<u8>>,
}

pub(crate) enum DaemonCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    ClientAttached,
    ClientDetached,
}

enum ClientCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

/// Circular buffer for scrollback catch-up
struct ScrollbackBuffer {
    buf: Vec<u8>,
    max_size: usize,
}

impl ScrollbackBuffer {
    fn new(max_size: usize) -> Self {
        Self {
            buf: Vec::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() > self.max_size {
            let excess = self.buf.len() - self.max_size;
            self.buf.drain(..excess);
            self.align_to_line_start();
        }
    }

    /// Drop the partial line at the front.
    ///
    /// Truncating at an arbitrary byte offset means the replay can begin in the
    /// middle of a CSI sequence or a UTF-8 codepoint. Measured on the real
    /// 64 KiB path: 14 of 16 alignments started mid-escape and 2 produced
    /// invalid UTF-8, so a reattaching client rendered literal junk like "1m"
    /// or lost its colour state.
    ///
    /// `\n` is a safe cut point: it can never appear inside a CSI sequence
    /// (parameters are 0x30-0x3F, intermediates 0x20-0x2F, final 0x40-0x7E) and
    /// it is never a UTF-8 continuation byte.
    fn align_to_line_start(&mut self) {
        if let Some(idx) = self.buf.iter().position(|&b| b == b'\n') {
            self.buf.drain(..=idx);
        } else {
            // No newline in the whole buffer: nothing can be safely salvaged.
            self.buf.clear();
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.buf.clone()
    }
}

/// Routes bytes between daemons and attached clients
pub struct QuicRouter {
    /// session_id → list of attached clients
    clients: Arc<RwLock<HashMap<String, Vec<AttachedClient>>>>,
    /// session_id → the daemon's command stream, tagged with the generation
    /// that registered it so a dying stream cannot unregister a newer one.
    daemon_inputs: Arc<RwLock<HashMap<String, RegisteredDaemon>>>,
    /// session_id → scrollback buffer (last 64KB of output)
    scrollback: Arc<RwLock<HashMap<String, ScrollbackBuffer>>>,
    next_client_id: AtomicU64,
    next_daemon_generation: AtomicU64,
}

struct RegisteredDaemon {
    generation: u64,
    sender: mpsc::Sender<DaemonCommand>,
}

impl QuicRouter {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            daemon_inputs: Arc::new(RwLock::new(HashMap::new())),
            scrollback: Arc::new(RwLock::new(HashMap::new())),
            next_client_id: AtomicU64::new(1),
            next_daemon_generation: AtomicU64::new(1),
        }
    }

    pub async fn forward_to_clients(&self, session_id: &str, data: &[u8]) {
        // Lock order is clients -> scrollback everywhere. attach_client used to
        // release the clients lock before taking scrollback, which left a
        // window where a just-attached client received a byte live AND saw it
        // again in its catch-up snapshot.
        let clients = self.clients.read().await;
        {
            let mut scrollback = self.scrollback.write().await;
            scrollback
                .entry(session_id.to_string())
                .or_insert_with(|| ScrollbackBuffer::new(65536))
                .push(data);
        }

        if let Some(senders) = clients.get(session_id) {
            for client in senders {
                // Remote delivery is best-effort. A slow phone must never
                // stall the daemon stream (and indirectly the local PTY).
                client.sender.try_send(data.to_vec()).ok();
            }
        }
    }

    pub async fn attach_client(
        &self,
        session_id: &str,
    ) -> (u64, bool, mpsc::Receiver<Vec<u8>>, Vec<u8>) {
        let (tx, rx) = mpsc::channel(512);
        let client_id = self.next_client_id.fetch_add(1, Ordering::Relaxed);

        // Hold clients across the scrollback read so the snapshot and the
        // start of live delivery are one atomic step -- same lock order as
        // forward_to_clients, so this cannot deadlock.
        let mut clients = self.clients.write().await;
        let attached = clients.entry(session_id.to_string()).or_default();
        let first_client = attached.is_empty();
        attached.push(AttachedClient {
            id: client_id,
            sender: tx,
        });

        let catchup = {
            let scrollback = self.scrollback.read().await;
            scrollback
                .get(session_id)
                .map(|sb| sb.snapshot())
                .unwrap_or_default()
        };
        drop(clients);

        info!(session_id = %session_id, catchup_bytes = catchup.len(), "client attached to session");
        (client_id, first_client, rx, catchup)
    }

    /// Detach one client and report whether it was the final client for the session.
    pub async fn detach_client(&self, session_id: &str, client_id: u64) -> bool {
        let mut clients = self.clients.write().await;
        let mut detached_last_client = false;

        if let Some(attached) = clients.get_mut(session_id) {
            let previous_len = attached.len();
            attached.retain(|client| client.id != client_id);
            detached_last_client = attached.len() != previous_len && attached.is_empty();
        }

        if detached_last_client {
            clients.remove(session_id);
        }

        detached_last_client
    }

    /// Returns the generation token that must be presented to unregister.
    pub async fn register_daemon_input(
        &self,
        session_id: &str,
        tx: mpsc::Sender<DaemonCommand>,
    ) -> u64 {
        let generation = self.next_daemon_generation.fetch_add(1, Ordering::Relaxed);
        let mut inputs = self.daemon_inputs.write().await;
        inputs.insert(
            session_id.to_string(),
            RegisteredDaemon {
                generation,
                sender: tx,
            },
        );
        generation
    }

    pub async fn send_to_daemon(&self, session_id: &str, data: &[u8]) {
        let inputs = self.daemon_inputs.read().await;
        if let Some(d) = inputs.get(session_id) {
            d.sender.send(DaemonCommand::Input(data.to_vec())).await.ok();
        }
    }

    pub async fn resize_daemon(&self, session_id: &str, cols: u16, rows: u16) {
        let inputs = self.daemon_inputs.read().await;
        if let Some(d) = inputs.get(session_id) {
            d.sender.send(DaemonCommand::Resize { cols, rows }).await.ok();
        }
    }

    /// Remove the daemon channel only if the caller still owns it.
    ///
    /// The unconditional remove was reproduced live: a daemon reconnect races
    /// its own dying stream's cleanup, the old stream deletes the channel the
    /// new one just registered, and the session goes input-dead while output
    /// keeps flowing. Measured cascade: 1.4 ms.
    pub async fn unregister_daemon(&self, session_id: &str, generation: u64) {
        let mut inputs = self.daemon_inputs.write().await;
        if inputs
            .get(session_id)
            .is_some_and(|d| d.generation == generation)
        {
            inputs.remove(session_id);
        } else {
            debug!(
                session_id = %session_id,
                generation,
                "stale daemon stream cleanup ignored; a newer stream owns this session"
            );
        }
    }

    /// Drop all per-session state. Called when the session itself goes away;
    /// without this the 64 KB scrollback leaked for every session id ever seen.
    pub async fn forget_session(&self, session_id: &str) {
        self.clients.write().await.remove(session_id);
        self.daemon_inputs.write().await.remove(session_id);
        self.scrollback.write().await.remove(session_id);
    }

    pub async fn notify_daemon_client_attached(&self, session_id: &str) {
        let inputs = self.daemon_inputs.read().await;
        if let Some(d) = inputs.get(session_id) {
            d.sender.send(DaemonCommand::ClientAttached).await.ok();
        }
    }

    pub async fn notify_daemon_client_detached(&self, session_id: &str) {
        let inputs = self.daemon_inputs.read().await;
        if let Some(d) = inputs.get(session_id) {
            d.sender.send(DaemonCommand::ClientDetached).await.ok();
        }
    }
}

pub async fn start_quic_server(
    addr: SocketAddr,
    certs: QuicCerts,
    sessions: SessionRegistry,
    tickets: TicketStore,
    require_ticket: bool,
) -> Result<()> {
    let cert_chain = vec![rustls::pki_types::CertificateDer::from(certs.cert_der)];
    let key = rustls::pki_types::PrivateKeyDer::try_from(certs.key_der)
        .map_err(|e| anyhow::anyhow!("invalid private key: {e}"))?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;

    server_crypto.alpn_protocols = vec![b"fil".to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(std::time::Duration::from_secs(QUIC_IDLE_TIMEOUT_SECS))
            .unwrap(),
    ));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(QUIC_KEEP_ALIVE_SECS)));

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));
    server_config.transport_config(Arc::new(transport));

    let endpoint = Endpoint::server(server_config, addr)?;
    info!(addr = %addr, "QUIC server listening");

    let router = Arc::new(QuicRouter::new());

    while let Some(incoming) = endpoint.accept().await {
        let router = router.clone();
        let sessions = sessions.clone();
        let tickets = tickets.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let remote = conn.remote_address();
                    info!(remote = %remote, "QUIC connection accepted");
                    handle_connection(conn, router, sessions, tickets, require_ticket).await;
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
    tickets: TicketStore,
    require_ticket: bool,
) {
    let remote = conn.remote_address();

    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let router = router.clone();
                let sessions = sessions.clone();
                let tickets = tickets.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_stream(send, recv, router, sessions, tickets, require_ticket).await {
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
    tickets: TicketStore,
    require_ticket: bool,
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
            let (input_tx, mut input_rx) = mpsc::channel::<DaemonCommand>(256);
            let generation = router.register_daemon_input(&session_id, input_tx).await;

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
                while let Some(command) = input_rx.recv().await {
                    if write_daemon_command(&mut send, command).await.is_err() {
                        break;
                    }
                }
            };

            tokio::select! {
                _ = read_task => {},
                _ = write_task => {},
            }

            router.unregister_daemon(&session_id, generation).await;
            debug!(session_id = %session_id, "daemon data stream closed");
        }

        // 0x02 = Client attach (iOS app watching a session)
        // After the session id, client->hub data is framed:
        // 0x00 + u32 length + bytes for input, or 0x01 + u16 cols + u16 rows.
        // 0x02 = v1 client attach, unauthenticated. Still accepted so a hub
        // deployed ahead of the app keeps working; remove once the fleet has
        // moved to 0x12.
        // 0x12 = v2 client attach, ticket-authenticated.
        0x02 | 0x12 => {
            let is_v2 = header[0] == 0x12;

            // Read session_id
            let mut len_buf = [0u8; 2];
            recv.read_exact(&mut len_buf).await?;
            let sid_len = u16::from_be_bytes(len_buf) as usize;
            let mut sid_buf = vec![0u8; sid_len];
            recv.read_exact(&mut sid_buf).await?;
            let session_id = String::from_utf8(sid_buf)?;

            if is_v2 {
                let mut tlen = [0u8; 2];
                recv.read_exact(&mut tlen).await?;
                let ticket_len = u16::from_be_bytes(tlen) as usize;
                if ticket_len > MAX_TICKET_BYTES {
                    warn!(session_id = %session_id, ticket_len, "attach ticket too large");
                    return Ok(());
                }
                let mut ticket_buf = vec![0u8; ticket_len];
                recv.read_exact(&mut ticket_buf).await?;
                let ticket = String::from_utf8(ticket_buf)?;

                match tickets.redeem(&ticket, &session_id) {
                    Some(user_id) => {
                        debug!(session_id = %session_id, %user_id, "attach ticket accepted");
                    }
                    None => {
                        warn!(session_id = %session_id, "attach rejected: invalid or spent ticket");
                        return Ok(());
                    }
                }
            } else if require_ticket {
                warn!(
                    session_id = %session_id,
                    "rejected unauthenticated v1 attach (FIL_REQUIRE_ATTACH_TICKET is on)"
                );
                return Ok(());
            } else {
                warn!(
                    session_id = %session_id,
                    "unauthenticated v1 attach accepted; client should upgrade to 0x12"
                );
            }

            debug!(session_id = %session_id, "client attached to session");

            // Subscribe to session output + get scrollback catch-up
            let (client_id, first_client, mut output_rx, catchup) =
                router.attach_client(&session_id).await;

            if first_client {
                router.notify_daemon_client_attached(&session_id).await;
            }

            // Keep cleanup outside the stream body so every exit path,
            // including a catch-up write failure, emits the detach transition.
            let stream_result: Result<()> = async {
                if !catchup.is_empty() {
                    send.write_all(&catchup).await?;
                }

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
                let sessions_input = sessions.clone();
                let recv_task = async move {
                    loop {
                        match read_client_command(&mut recv).await {
                            Ok(Some(ClientCommand::Input(data))) => {
                                router_input.send_to_daemon(&sid_input, &data).await;
                            }
                            Ok(Some(ClientCommand::Resize { cols, rows })) => {
                                sessions_input.update_session_size(
                                    &sid_input,
                                    u32::from(cols),
                                    u32::from(rows),
                                );
                                router_input.resize_daemon(&sid_input, cols, rows).await;
                            }
                            Ok(Some(ClientCommand::Detach)) | Ok(None) | Err(_) => break,
                        }
                    }
                };

                tokio::select! {
                    _ = send_task => {},
                    _ = recv_task => {},
                }

                Ok(())
            }
            .await;

            if router.detach_client(&session_id, client_id).await {
                router.notify_daemon_client_detached(&session_id).await;
            }
            debug!(session_id = %session_id, "client detached");
            stream_result?;
        }

        other => {
            warn!(stream_type = other, "unknown stream type");
        }
    }

    Ok(())
}

const FRAME_CLIENT_ATTACHED: u8 = 0x02;
const FRAME_CLIENT_DETACHED: u8 = 0x03;

async fn write_daemon_command(send: &mut SendStream, command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Input(data) => {
            send.write_all(&[FRAME_INPUT]).await?;
            send.write_all(&(data.len() as u32).to_be_bytes()).await?;
            send.write_all(&data).await?;
        }
        DaemonCommand::Resize { cols, rows } => {
            send.write_all(&[FRAME_RESIZE]).await?;
            send.write_all(&cols.to_be_bytes()).await?;
            send.write_all(&rows.to_be_bytes()).await?;
        }
        DaemonCommand::ClientAttached => {
            send.write_all(&[FRAME_CLIENT_ATTACHED]).await?;
        }
        DaemonCommand::ClientDetached => {
            send.write_all(&[FRAME_CLIENT_DETACHED]).await?;
        }
    }
    Ok(())
}

async fn read_client_command(recv: &mut RecvStream) -> Result<Option<ClientCommand>> {
    let mut frame_type = [0u8; 1];
    if !read_exact_or_eof(recv, &mut frame_type).await? {
        return Ok(None);
    }

    match frame_type[0] {
        FRAME_INPUT => {
            let mut len_buf = [0u8; 4];
            if !read_exact_or_eof(recv, &mut len_buf).await? {
                return Ok(None);
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len > MAX_INPUT_FRAME_BYTES {
                anyhow::bail!("input frame too large: {len} bytes");
            }
            let mut data = vec![0u8; len];
            if len > 0 && !read_exact_or_eof(recv, &mut data).await? {
                return Ok(None);
            }
            Ok(Some(ClientCommand::Input(data)))
        }
        FRAME_RESIZE => {
            let mut size_buf = [0u8; 4];
            if !read_exact_or_eof(recv, &mut size_buf).await? {
                return Ok(None);
            }
            let cols = u16::from_be_bytes([size_buf[0], size_buf[1]]);
            let rows = u16::from_be_bytes([size_buf[2], size_buf[3]]);
            Ok(Some(ClientCommand::Resize { cols, rows }))
        }
        CLIENT_FRAME_DETACH => Ok(Some(ClientCommand::Detach)),
        other => anyhow::bail!("unknown client frame type: {other}"),
    }
}

async fn read_exact_or_eof(recv: &mut RecvStream, buf: &mut [u8]) -> Result<bool> {
    let mut offset = 0;
    while offset < buf.len() {
        match recv.read(&mut buf[offset..]).await? {
            Some(0) | None => return Ok(false),
            Some(n) => offset += n,
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_final_client_detach_closes_the_remote_attachment() {
        let router = QuicRouter::new();

        let (first_id, first_for_session, _first_rx, _) = router.attach_client("session").await;
        let (second_id, second_for_session, _second_rx, _) = router.attach_client("session").await;

        assert!(first_for_session);
        assert!(!second_for_session);
        assert!(!router.detach_client("session", first_id).await);
        assert!(router.detach_client("session", second_id).await);
        assert!(!router.detach_client("session", second_id).await);
    }
}

#[cfg(test)]
mod router_tests {
    use super::*;

    /// Reproduced live before the fix: a daemon reconnects, then its previous
    /// stream's cleanup runs and deletes the channel the new stream just
    /// registered. Output keeps flowing, input goes nowhere.
    #[tokio::test]
    async fn stale_daemon_cleanup_cannot_evict_a_reconnected_daemon() {
        let router = QuicRouter::new();
        let sid = "s1";

        let (tx_a, _rx_a) = mpsc::channel(8);
        let gen_a = router.register_daemon_input(sid, tx_a).await;

        // Daemon reconnects on a fresh stream.
        let (tx_b, mut rx_b) = mpsc::channel(8);
        let _gen_b = router.register_daemon_input(sid, tx_b).await;

        // Now A's dying stream cleans up, presenting its own (stale) token.
        router.unregister_daemon(sid, gen_a).await;

        router.send_to_daemon(sid, b"keystroke").await;

        assert!(
            rx_b.try_recv().is_ok(),
            "input must still reach the reconnected daemon"
        );
    }

    #[tokio::test]
    async fn the_owning_daemon_can_still_unregister() {
        let router = QuicRouter::new();
        let sid = "s2";
        let (tx, mut rx) = mpsc::channel(8);
        let generation = router.register_daemon_input(sid, tx).await;

        router.unregister_daemon(sid, generation).await;
        router.send_to_daemon(sid, b"x").await;

        assert!(
            rx.try_recv().is_err(),
            "after a legitimate unregister nothing should be delivered"
        );
    }

    #[test]
    fn scrollback_truncation_starts_on_a_line_boundary() {
        // Lines carrying multi-byte UTF-8 and CSI sequences, as a real shell does.
        let line = "\u{1b}[32m✓\u{1b}[0m déployé — résumé\n";
        let mut sb = ScrollbackBuffer::new(1024);
        for _ in 0..200 {
            sb.push(line.as_bytes());
        }
        let snap = sb.snapshot();

        assert!(
            std::str::from_utf8(&snap).is_ok(),
            "replay must be valid UTF-8, got {:?}",
            &snap[..snap.len().min(8)]
        );
        assert!(
            snap.starts_with(b"\x1b["),
            "replay must start at the beginning of a line, got {:?}",
            String::from_utf8_lossy(&snap[..snap.len().min(24)])
        );
    }

    #[test]
    fn scrollback_alignment_holds_at_every_offset() {
        let line = "\u{1b}[32m✓\u{1b}[0m déployé — résumé\n";
        // Sweep the tail so the cut lands at a different place each time.
        for shift in 0..16usize {
            let mut sb = ScrollbackBuffer::new(1024);
            for _ in 0..200 {
                sb.push(line.as_bytes());
            }
            sb.push(&vec![b'x'; shift]);
            sb.push(b"\n");
            let snap = sb.snapshot();
            assert!(
                std::str::from_utf8(&snap).is_ok(),
                "shift {shift} produced invalid UTF-8"
            );
        }
    }

    #[tokio::test]
    async fn forget_session_drops_the_scrollback() {
        let router = QuicRouter::new();
        let sid = "s3";
        router.forward_to_clients(sid, b"hello\n").await;
        assert!(router.scrollback.read().await.contains_key(sid));

        router.forget_session(sid).await;

        assert!(
            !router.scrollback.read().await.contains_key(sid),
            "64KB per session id leaked without this"
        );
    }
}
