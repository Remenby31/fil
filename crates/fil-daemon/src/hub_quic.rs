use crate::config::DaemonConfig;
use crate::session_manager::ProxyCommand;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use quinn::Endpoint;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tracing::{info, warn};
use url::Url;

const FRAME_INPUT: u8 = 0x00;
const FRAME_RESIZE: u8 = 0x01;
const FRAME_CLIENT_ATTACHED: u8 = 0x02;
const FRAME_CLIENT_DETACHED: u8 = 0x03;
const MAX_INPUT_FRAME_BYTES: usize = 1024 * 1024;
const QUIC_IDLE_TIMEOUT_SECS: u64 = 15;
const QUIC_KEEP_ALIVE_SECS: u64 = 5;
const QUIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_BACKOFF_SECS: u64 = 1;
/// A long reconnect gap is worse than a busy one: while the daemon is not
/// registered, the hub happily accepts an iOS attach and then drops every
/// keystroke on the floor. Keep the hole short.
const MAX_BACKOFF_SECS: u64 = 5;

/// What a single data stream attempt ended up doing, reported even when the
/// attempt finishes with an error (a stream can be established and then fail
/// mid-flight, which still counts as a success for backoff purposes).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct StreamOutcome {
    /// The bidirectional stream was opened and the daemon header was accepted.
    established: bool,
    /// A remote client was still attached when the stream ended.
    client_attached: bool,
}

fn client_config(certificate: Vec<u8>) -> Result<quinn::ClientConfig> {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(fil_protocol::tls::PinnedServerCertificate(
            certificate,
        )))
        .with_no_client_auth();

    crypto.alpn_protocols = vec![b"fil".to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(std::time::Duration::from_secs(QUIC_IDLE_TIMEOUT_SECS))
            .unwrap(),
    ));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(QUIC_KEEP_ALIVE_SECS)));

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

pub async fn start(config: DaemonConfig) -> Result<QuicHub> {
    let endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    Ok(QuicHub { endpoint, config })
}

pub struct QuicHub {
    endpoint: Endpoint,
    config: DaemonConfig,
}

impl QuicHub {
    pub async fn open_session_stream(
        &self,
        session_id: String,
        proxy_tx: mpsc::Sender<ProxyCommand>,
    ) -> Result<mpsc::Sender<Vec<u8>>> {
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(512);

        let endpoint = self.endpoint.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            quic_stream_loop(endpoint, config, session_id, proxy_tx, output_rx).await;
        });

        Ok(output_tx)
    }
}

async fn quic_stream_loop(
    endpoint: Endpoint,
    config: DaemonConfig,
    session_id: String,
    proxy_tx: mpsc::Sender<ProxyCommand>,
    output_rx: mpsc::Receiver<Vec<u8>>,
) {
    let sid = session_id.clone();
    // Calls are strictly sequential, but the borrow has to outlive each closure
    // invocation, so it goes behind a mutex rather than a &mut capture.
    let output_rx = Arc::new(tokio::sync::Mutex::new(output_rx));

    reconnect_loop(session_id, proxy_tx.clone(), move || {
        let endpoint = endpoint.clone();
        let config = config.clone();
        let sid = sid.clone();
        let proxy_tx = proxy_tx.clone();
        let output_rx = output_rx.clone();
        async move {
            let mut rx = output_rx.lock().await;
            let mut outcome = StreamOutcome::default();
            match connect_and_run_stream(&endpoint, &config, &sid, &proxy_tx, &mut rx, &mut outcome)
                .await
            {
                Ok(()) => info!(session_id = %sid, "data stream closed normally"),
                Err(e) => warn!(session_id = %sid, error = %e, "data stream error"),
            }
            outcome
        }
    })
    .await
}

/// The reconnect policy, split out from the transport so it can be driven by a
/// mock connector under a paused clock.
async fn reconnect_loop<F, Fut>(
    session_id: String,
    proxy_tx: mpsc::Sender<ProxyCommand>,
    mut connect: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = StreamOutcome>,
{
    let mut backoff = INITIAL_BACKOFF_SECS;

    loop {
        let outcome = tokio::select! {
            outcome = connect() => outcome,
            _ = proxy_tx.closed() => break,
        };

        // Only tell the proxy to restore its local window size if a client was
        // genuinely attached. Firing this unconditionally SIGWINCHes the user's
        // shell on every reconnect attempt, even when nobody was ever connected.
        if outcome.client_attached {
            proxy_tx.send(ProxyCommand::ClientDetached).await.ok();
        }

        // Check if the proxy is still alive (output_rx not closed)
        if proxy_tx.is_closed() {
            info!(session_id = %session_id, "proxy gone, stopping data reconnect");
            break;
        }

        // A stream that was established and later dropped is a success, not a
        // failure: without this reset the daemon ratchets up to the cap and
        // stays there for the rest of the process's life.
        if outcome.established {
            backoff = INITIAL_BACKOFF_SECS;
        }

        info!(session_id = %session_id, backoff_s = backoff, "reconnecting data stream...");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {},
            _ = proxy_tx.closed() => break,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
    }
}

async fn connect_and_run_stream(
    endpoint: &Endpoint,
    config: &DaemonConfig,
    session_id: &str,
    proxy_tx: &mpsc::Sender<ProxyCommand>,
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
    outcome: &mut StreamOutcome,
) -> Result<()> {
    use base64::Engine;
    #[derive(serde::Deserialize)]
    struct Ticket {
        ticket: String,
        quic_certificate: String,
    }
    // No redirect may downgrade HTTPS or send the bearer token to another host.
    // In particular, auth/ownership errors must not be treated as UDP failures.
    let ticket: Ticket = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow::anyhow!("could not initialize HTTPS client"))?
        .post(data_url(&config.hub_url, session_id, false)?)
        .bearer_auth(&config.token)
        .timeout(CONNECT_TIMEOUT)
        .send()
        .await
        .map_err(ticket_error)?
        .error_for_status()
        .map_err(ticket_error)?
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("invalid daemon ticket response"))?;
    let certificate = base64::engine::general_purpose::STANDARD
        .decode(&ticket.quic_certificate)
        .context("invalid QUIC certificate encoding")?;

    match timeout(
        QUIC_CONNECT_TIMEOUT,
        connect_quic_stream(endpoint, config, session_id, &ticket.ticket, certificate),
    )
    .await
    {
        Ok(Ok((conn, send, recv))) => {
            run_quic_stream(conn, send, recv, session_id, proxy_tx, output_rx, outcome).await
        }
        result => {
            let reason = match result {
                Err(_) => "QUIC connect timed out".to_owned(),
                Ok(Err(error)) => error.to_string(),
                Ok(Ok(_)) => unreachable!(),
            };
            warn!(%session_id, %reason, "trying authenticated WebSocket data fallback");
            connect_and_run_ws(config, session_id, proxy_tx, output_rx, outcome).await
        }
    }
}

fn ticket_error(error: reqwest::Error) -> anyhow::Error {
    // Never format a reqwest URL or an HTTP/WebSocket response: either can
    // contain credentials. Keep the actionable category/status, not secrets.
    if let Some(status) = error.status() {
        anyhow::anyhow!("daemon ticket rejected: HTTP {}", status.as_u16())
    } else if error.is_timeout() {
        anyhow::anyhow!("daemon ticket HTTPS request timed out")
    } else if error.is_connect() {
        anyhow::anyhow!("daemon ticket HTTPS connection or TLS verification failed")
    } else {
        anyhow::anyhow!("daemon ticket HTTPS request failed")
    }
}

fn data_url(hub_url: &str, session_id: &str, websocket: bool) -> Result<Url> {
    let mut url = fil_protocol::tls::hub_url(hub_url).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "hub URL must not contain credentials"
    );
    let scheme = match (url.scheme(), websocket) {
        ("https", true) => "wss",
        ("http", true) => "ws", // Explicit local-development HTTP configuration only.
        ("https", false) => "https",
        ("http", false) => "http",
        _ => anyhow::bail!("hub URL must use HTTP or HTTPS"),
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("invalid hub URL scheme"))?;
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid hub URL path"))?;
        path.clear();
        if websocket {
            path.extend(["ws", "data", session_id]);
        } else {
            path.extend(["sessions", session_id, "daemon-ticket"]);
        }
    }
    if websocket {
        url.query_pairs_mut().append_pair("role", "daemon");
    }
    Ok(url)
}

fn data_ws_request(
    config: &DaemonConfig,
    session_id: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let url = data_url(&config.hub_url, session_id, true)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| anyhow::anyhow!("invalid data WebSocket request"))?;
    let mut authorization = format!("Bearer {}", config.token)
        .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
        .map_err(|_| anyhow::anyhow!("invalid bearer header"))?;
    authorization.set_sensitive(true);
    request.headers_mut().insert("Authorization", authorization);
    Ok(request)
}

async fn connect_quic_stream(
    endpoint: &Endpoint,
    config: &DaemonConfig,
    session_id: &str,
    ticket: &str,
    certificate: Vec<u8>,
) -> Result<(quinn::Connection, quinn::SendStream, quinn::RecvStream)> {
    let host = config.effective_quic_host();
    let addr = tokio::net::lookup_host((host.as_str(), config.quic_port))
        .await
        .context("DNS resolution failed")?
        .find(|a| a.is_ipv4())
        .context("could not resolve hub QUIC address")?;

    let conn = endpoint
        .connect_with(client_config(certificate)?, addr, &host)
        .map_err(|_| anyhow::anyhow!("QUIC connection could not start"))?
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{}",
                match error {
                    quinn::ConnectionError::TimedOut => "QUIC connection timed out",
                    quinn::ConnectionError::TransportError(_) =>
                        "QUIC transport or certificate verification failed",
                    _ => "QUIC connection failed",
                }
            )
        })?;

    let (mut send, recv) = conn.open_bi().await.context("QUIC stream open failed")?;

    // Authenticated daemon stream, with a purpose-bound single-use ticket.
    let mut header = vec![0x11];
    let sid_bytes = session_id.as_bytes();
    header.extend_from_slice(
        &u16::try_from(sid_bytes.len())
            .context("session ID too long")?
            .to_be_bytes(),
    );
    header.extend_from_slice(sid_bytes);
    header.extend_from_slice(
        &u16::try_from(ticket.len())
            .context("daemon ticket too long")?
            .to_be_bytes(),
    );
    header.extend_from_slice(ticket.as_bytes());
    send.write_all(&header)
        .await
        .context("QUIC authentication header write failed")?;
    Ok((conn, send, recv))
}

async fn run_quic_stream(
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    session_id: &str,
    proxy_tx: &mpsc::Sender<ProxyCommand>,
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
    outcome: &mut StreamOutcome,
) -> Result<()> {
    info!(session_id = %session_id, "QUIC stream opened");
    outcome.established = true;

    // Written by recv_task, read by the loop after the select resolves.
    let client_attached = std::sync::atomic::AtomicBool::new(false);

    let send_task = async {
        while let Some(data) = output_rx.recv().await {
            if send.write_all(&data).await.is_err() {
                break;
            }
        }
        send.finish().ok();
    };

    let recv_task = async {
        while let Ok(Some(cmd)) = read_command_frame(&mut recv).await {
            // Never log input bytes: they may contain passwords.
            match cmd {
                ProxyCommand::ClientAttached => {
                    client_attached.store(true, std::sync::atomic::Ordering::Relaxed)
                }
                ProxyCommand::ClientDetached => {
                    client_attached.store(false, std::sync::atomic::Ordering::Relaxed)
                }
                _ => {}
            }
            if proxy_tx.send(cmd).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    outcome.client_attached = client_attached.load(std::sync::atomic::Ordering::Relaxed);
    conn.close(0u32.into(), b"data stream ended");

    Ok(())
}

async fn connect_and_run_ws(
    config: &DaemonConfig,
    session_id: &str,
    proxy_tx: &mpsc::Sender<ProxyCommand>,
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
    outcome: &mut StreamOutcome,
) -> Result<()> {
    let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_INPUT_FRAME_BYTES + 5))
        .max_frame_size(Some(MAX_INPUT_FRAME_BYTES + 5));
    let (mut socket, _) = timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(
            data_ws_request(config, session_id)?,
            Some(ws_config),
            true,
        ),
    )
    .await
    .context("data WebSocket connect timed out")?
    .map_err(|error| match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => anyhow::anyhow!(
            "data WebSocket rejected: HTTP {}",
            response.status().as_u16()
        ),
        tokio_tungstenite::tungstenite::Error::Tls(_) => {
            anyhow::anyhow!("data WebSocket TLS verification failed")
        }
        tokio_tungstenite::tungstenite::Error::Io(_) => {
            anyhow::anyhow!("data WebSocket TCP connection failed")
        }
        _ => anyhow::anyhow!("data WebSocket handshake failed"),
    })?;
    outcome.established = true;
    info!(%session_id, "authenticated WebSocket data stream opened");
    let result: Result<()> = async {
        let mut ticker = tokio::time::interval(Duration::from_secs(QUIC_KEEP_ALIVE_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                data = output_rx.recv() => {
                    let Some(data) = data else { return Ok(()); };
                    // Keep each message bounded, even if a future proxy batches output.
                    for chunk in data.chunks(MAX_INPUT_FRAME_BYTES) {
                        timeout(IO_TIMEOUT, socket.send(Message::Binary(chunk.to_vec().into())))
                            .await.context("data WebSocket write timed out")?
                            .map_err(|_| anyhow::anyhow!("data WebSocket write failed"))?;
                    }
                }
                message = socket.next() => {
                    last_seen = Instant::now();
                    match message {
                        Some(Ok(Message::Binary(frame))) => {
                            let command = parse_ws_command(&frame)?;
                            match command {
                                ProxyCommand::ClientAttached => outcome.client_attached = true,
                                ProxyCommand::ClientDetached => outcome.client_attached = false,
                                _ => {},
                            }
                            timeout(IO_TIMEOUT, proxy_tx.send(command)).await
                                .context("proxy command timed out")?
                                .map_err(|_| anyhow::anyhow!("proxy closed"))?;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            timeout(IO_TIMEOUT, socket.send(Message::Pong(data))).await
                                .context("data WebSocket pong timed out")?
                                .map_err(|_| anyhow::anyhow!("data WebSocket pong failed"))?;
                        }
                        Some(Ok(Message::Pong(_))) => {},
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Err(_)) => anyhow::bail!("data WebSocket read failed"),
                        _ => anyhow::bail!("invalid data WebSocket message"),
                    }
                }
                _ = ticker.tick() => {
                    anyhow::ensure!(last_seen.elapsed() < Duration::from_secs(QUIC_IDLE_TIMEOUT_SECS), "data WebSocket peer timed out");
                    timeout(IO_TIMEOUT, socket.send(Message::Ping(Vec::new().into()))).await
                        .context("data WebSocket ping timed out")?
                        .map_err(|_| anyhow::anyhow!("data WebSocket ping failed"))?;
                }
                _ = proxy_tx.closed() => return Ok(()),
            }
        }
    }.await;
    let _ = timeout(Duration::from_secs(1), socket.close(None)).await;
    result
}

fn parse_ws_command(frame: &[u8]) -> Result<ProxyCommand> {
    match frame.first() {
        Some(&FRAME_INPUT) if frame.len() >= 5 => {
            let len = u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize;
            anyhow::ensure!(
                len <= MAX_INPUT_FRAME_BYTES && frame.len() - 5 == len,
                "invalid WebSocket input frame length"
            );
            Ok(ProxyCommand::Input(frame[5..].to_vec()))
        }
        Some(&FRAME_RESIZE) if frame.len() == 5 => Ok(ProxyCommand::Resize {
            cols: u16::from_be_bytes([frame[1], frame[2]]),
            rows: u16::from_be_bytes([frame[3], frame[4]]),
        }),
        Some(&FRAME_CLIENT_ATTACHED) if frame.len() == 1 => Ok(ProxyCommand::ClientAttached),
        Some(&FRAME_CLIENT_DETACHED) if frame.len() == 1 => Ok(ProxyCommand::ClientDetached),
        _ => anyhow::bail!("invalid WebSocket daemon command"),
    }
}

async fn read_command_frame(recv: &mut quinn::RecvStream) -> Result<Option<ProxyCommand>> {
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
            Ok(Some(ProxyCommand::Input(data)))
        }
        FRAME_RESIZE => {
            let mut size_buf = [0u8; 4];
            if !read_exact_or_eof(recv, &mut size_buf).await? {
                return Ok(None);
            }
            let cols = u16::from_be_bytes([size_buf[0], size_buf[1]]);
            let rows = u16::from_be_bytes([size_buf[2], size_buf[3]]);
            Ok(Some(ProxyCommand::Resize { cols, rows }))
        }
        FRAME_CLIENT_ATTACHED => Ok(Some(ProxyCommand::ClientAttached)),
        FRAME_CLIENT_DETACHED => Ok(Some(ProxyCommand::ClientDetached)),
        other => anyhow::bail!("unknown command frame type: {other}"),
    }
}

async fn read_exact_or_eof(recv: &mut quinn::RecvStream, buf: &mut [u8]) -> Result<bool> {
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
mod reconnect_tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::time::Instant;

    /// One scripted stream attempt: how it ends, and what it reports.
    type Attempt = StreamOutcome;

    /// Never got a stream up (DNS, refused, handshake failure).
    const FAILED_CONNECT: Attempt = StreamOutcome {
        established: false,
        client_attached: false,
    };
    /// Stream was up for a while, then the link dropped with nobody attached.
    const LIVED_THEN_DROPPED: Attempt = StreamOutcome {
        established: true,
        client_attached: false,
    };

    /// Drives `reconnect_loop` with a scripted connector under a paused clock
    /// and returns the sleep observed before each retry.
    async fn observe_delays(script: Vec<Attempt>) -> (Vec<u64>, Vec<ProxyCommand>) {
        let (proxy_tx, mut proxy_rx) = mpsc::channel::<ProxyCommand>(32);
        let delays = Arc::new(Mutex::new(Vec::<u64>::new()));
        let calls = Arc::new(Mutex::new(0usize));
        let last = Arc::new(Mutex::new(Instant::now()));
        let total = script.len();

        {
            let delays = delays.clone();
            let calls = calls.clone();
            let last = last.clone();
            let loop_fut = reconnect_loop("sid".into(), proxy_tx.clone(), move || {
                let delays = delays.clone();
                let calls = calls.clone();
                let last = last.clone();
                let script = script.clone();
                async move {
                    let n = {
                        let mut c = calls.lock().unwrap();
                        let n = *c;
                        *c += 1;
                        n
                    };
                    if n > 0 {
                        // Under a paused clock the only elapsed time is the
                        // loop's own sleep, so this IS the backoff delay.
                        let elapsed = last.lock().unwrap().elapsed().as_secs();
                        delays.lock().unwrap().push(elapsed);
                    }
                    if n >= script.len() {
                        // Park forever; the outer select's timeout ends the test.
                        futures_util::future::pending::<()>().await;
                    }
                    *last.lock().unwrap() = Instant::now();
                    script[n]
                }
            });

            tokio::select! {
                _ = loop_fut => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {}
            }
        }

        drop(proxy_tx);
        let mut cmds = Vec::new();
        while let Ok(c) = proxy_rx.try_recv() {
            cmds.push(c);
        }
        let d = delays.lock().unwrap().clone();
        assert!(d.len() >= total.saturating_sub(1), "loop ran too few times");
        (d, cmds)
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_resets_after_an_established_stream() {
        // fail, fail, then a stream that lived and dropped, then fail again.
        let (delays, _) = observe_delays(vec![
            FAILED_CONNECT,
            FAILED_CONNECT,
            LIVED_THEN_DROPPED,
            FAILED_CONNECT,
        ])
        .await;

        // delays[0] follows attempt 0, delays[1] follows attempt 1,
        // delays[2] follows the established attempt -> must be back to 1s.
        assert_eq!(
            delays[2], INITIAL_BACKOFF_SECS,
            "backoff must reset after an established stream, got {delays:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_caps_at_max_and_never_reaches_thirty() {
        let (delays, _) = observe_delays(vec![FAILED_CONNECT; 9]).await;
        assert_eq!(
            *delays.iter().max().unwrap(),
            MAX_BACKOFF_SECS,
            "backoff must cap at {MAX_BACKOFF_SECS}s, got {delays:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_client_detached_when_no_client_was_ever_attached() {
        let (_, cmds) = observe_delays(vec![FAILED_CONNECT; 5]).await;
        assert!(
            cmds.is_empty(),
            "reconnect attempts with no attached client must not SIGWINCH the shell, got {cmds:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn client_detached_is_still_sent_when_a_client_was_attached() {
        let attached = Attempt {
            established: true,
            client_attached: true,
        };
        let (_, cmds) = observe_delays(vec![attached, FAILED_CONNECT]).await;
        assert_eq!(
            cmds.len(),
            1,
            "a real attachment ending must still restore the local size, got {cmds:?}"
        );
        assert!(matches!(cmds[0], ProxyCommand::ClientDetached));
    }

    #[tokio::test(start_paused = true)]
    async fn closed_proxy_cancels_a_pending_connection() {
        let (proxy_tx, proxy_rx) = mpsc::channel(1);
        drop(proxy_rx);
        timeout(
            Duration::from_secs(1),
            reconnect_loop("sid".into(), proxy_tx, || async {
                futures_util::future::pending::<StreamOutcome>().await
            }),
        )
        .await
        .expect("closed proxy must stop reconnect work immediately");
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn websocket_request_keeps_bearer_out_of_the_url_and_preserves_tls() {
        let config = DaemonConfig {
            hub_url: "https://example.invalid/old?token=discarded#discarded".into(),
            token: "test-only-bearer".into(),
            ..Default::default()
        };
        let request = data_ws_request(&config, "sid").unwrap();
        assert_eq!(
            request.uri(),
            "wss://example.invalid/ws/data/sid?role=daemon"
        );
        assert_eq!(
            request.headers()["Authorization"],
            "Bearer test-only-bearer"
        );
        assert!(request.headers()["Authorization"].is_sensitive());
        assert_eq!(
            data_url("http://localhost:3100", "sid", true)
                .unwrap()
                .scheme(),
            "ws"
        );
        assert_eq!(
            data_url("https://example.invalid", "sid", false)
                .unwrap()
                .as_str(),
            "https://example.invalid/sessions/sid/daemon-ticket"
        );
        for invalid in [
            "ftp://example.invalid",
            "wss://example.invalid",
            "https://user:password@example.invalid",
        ] {
            assert!(data_url(invalid, "sid", true).is_err());
        }
        assert!(
            data_ws_request(
                &DaemonConfig {
                    token: "bad\r\nheader".into(),
                    ..config
                },
                "sid"
            )
            .is_err()
        );
    }

    #[test]
    fn websocket_commands_match_quic_frames_and_reject_truncation_or_trailing_bytes() {
        assert!(
            matches!(parse_ws_command(&[0, 0, 0, 0, 2, 0, 0xff]).unwrap(), ProxyCommand::Input(data) if data == [0, 0xff])
        );
        assert!(matches!(
            parse_ws_command(&[1, 0, 120, 0, 40]).unwrap(),
            ProxyCommand::Resize {
                cols: 120,
                rows: 40
            }
        ));
        assert!(matches!(
            parse_ws_command(&[2]).unwrap(),
            ProxyCommand::ClientAttached
        ));
        assert!(matches!(
            parse_ws_command(&[3]).unwrap(),
            ProxyCommand::ClientDetached
        ));
        for frame in [
            &[][..],
            &[0],
            &[0, 0, 0, 0, 1],
            &[0, 0, 0, 0, 0, 1],
            &[0, 0xff, 0xff, 0xff, 0xff],
            &[1, 0, 1, 0],
            &[1, 0, 1, 0, 1, 0],
            &[2, 0],
            &[3, 0],
            &[4],
        ] {
            assert!(parse_ws_command(frame).is_err());
        }
        let mut largest = vec![0];
        largest.extend_from_slice(&(MAX_INPUT_FRAME_BYTES as u32).to_be_bytes());
        largest.resize(MAX_INPUT_FRAME_BYTES + 5, 7);
        assert!(parse_ws_command(&largest).is_ok());
    }

    async fn serve_ticket(listener: &TcpListener, status: u16) {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
            assert!(request.len() < 8192);
        }
        let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /sessions/sid/daemon-ticket "));
        assert!(request.contains("authorization: bearer test-only-bearer\r\n"));
        let body = r#"{"ticket":"test-only-ticket","quic_certificate":"AQID"}"#;
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn silent_udp_falls_back_with_auth_and_preserves_queued_output_and_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        // A bound socket that never answers models a VPN dropping QUIC packets.
        let silent_udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = DaemonConfig {
            hub_url: format!("http://{}", listener.local_addr().unwrap()),
            quic_host: "127.0.0.1".into(),
            quic_port: silent_udp.local_addr().unwrap().port(),
            token: "test-only-bearer".into(),
            ..Default::default()
        };
        let server = tokio::spawn(async move {
            serve_ticket(&listener, 200).await;
            let (socket, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)] // The callback error type belongs to tungstenite.
            let check = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                         response| {
                assert_eq!(request.uri(), "/ws/data/sid?role=daemon");
                assert_eq!(
                    request.headers()["Authorization"],
                    "Bearer test-only-bearer"
                );
                Ok(response)
            };
            let mut socket = tokio_tungstenite::accept_hdr_async(socket, check)
                .await
                .unwrap();
            loop {
                if let Message::Binary(data) = socket.next().await.unwrap().unwrap() {
                    assert_eq!(data.as_ref(), b"queued before connection");
                    break;
                }
            }
            for frame in [vec![2], vec![1, 0, 120, 0, 40], vec![0, 0, 0, 0, 1, b'x']] {
                socket.send(Message::Binary(frame.into())).await.unwrap();
            }
            socket.send(Message::Ping(vec![42].into())).await.unwrap();
            loop {
                if let Message::Pong(data) = socket.next().await.unwrap().unwrap()
                    && data.as_ref() == [42]
                {
                    break;
                }
            }
            socket.close(None).await.unwrap();
        });
        let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let (proxy_tx, mut proxy_rx) = mpsc::channel(8);
        let (output_tx, mut output_rx) = mpsc::channel(8);
        output_tx
            .send(b"queued before connection".to_vec())
            .await
            .unwrap();
        let mut outcome = StreamOutcome::default();
        let started = Instant::now();
        timeout(
            Duration::from_secs(6),
            connect_and_run_stream(
                &endpoint,
                &config,
                "sid",
                &proxy_tx,
                &mut output_rx,
                &mut outcome,
            ),
        )
        .await
        .expect("silent UDP must not wait for the 15s QUIC idle timeout")
        .unwrap();
        assert!(started.elapsed() >= QUIC_CONNECT_TIMEOUT);
        assert!(outcome.established && outcome.client_attached);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ProxyCommand::ClientAttached)
        ));
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ProxyCommand::Resize {
                cols: 120,
                rows: 40
            })
        ));
        assert!(matches!(proxy_rx.try_recv(), Ok(ProxyCommand::Input(data)) if data == b"x"));
        timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn ticket_auth_rejection_never_attempts_websocket_or_drains_output() {
        for status in [401, 403, 404] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = DaemonConfig {
                hub_url: format!("http://{}", listener.local_addr().unwrap()),
                token: "test-only-bearer".into(),
                ..Default::default()
            };
            let server = tokio::spawn(async move {
                serve_ticket(&listener, status).await;
                assert!(
                    timeout(Duration::from_millis(50), listener.accept())
                        .await
                        .is_err(),
                    "auth rejection must not initiate a fallback connection"
                );
            });
            let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
            let (proxy_tx, _proxy_rx) = mpsc::channel(8);
            let (output_tx, mut output_rx) = mpsc::channel(8);
            output_tx.send(b"retained".to_vec()).await.unwrap();
            let mut outcome = StreamOutcome::default();
            let error = connect_and_run_stream(
                &endpoint,
                &config,
                "sid",
                &proxy_tx,
                &mut output_rx,
                &mut outcome,
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("daemon ticket rejected: HTTP {status}")
            );
            assert_eq!(outcome, StreamOutcome::default());
            assert_eq!(output_rx.try_recv().unwrap(), b"retained");
            server.await.unwrap();
        }
    }
}
