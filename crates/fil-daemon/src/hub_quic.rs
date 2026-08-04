use crate::config::DaemonConfig;
use crate::session_manager::ProxyCommand;
use anyhow::{Context, Result};
use quinn::Endpoint;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

const FRAME_INPUT: u8 = 0x00;
const FRAME_RESIZE: u8 = 0x01;
const FRAME_CLIENT_ATTACHED: u8 = 0x02;
const FRAME_CLIENT_DETACHED: u8 = 0x03;
const MAX_INPUT_FRAME_BYTES: usize = 1024 * 1024;
const QUIC_IDLE_TIMEOUT_SECS: u64 = 15;
const QUIC_KEEP_ALIVE_SECS: u64 = 5;
const INITIAL_BACKOFF_SECS: u64 = 1;
/// A long reconnect gap is worse than a busy one: while the daemon is not
/// registered, the hub happily accepts an iOS attach and then drops every
/// keystroke on the floor. Keep the hole short.
const MAX_BACKOFF_SECS: u64 = 5;

/// What a single QUIC stream attempt ended up doing, reported even when the
/// attempt finishes with an error (a stream can be established and then fail
/// mid-flight, which still counts as a success for backoff purposes).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct StreamOutcome {
    /// The bidirectional stream was opened and the daemon header was accepted.
    established: bool,
    /// A remote client was still attached when the stream ended.
    client_attached: bool,
}

pub async fn start(config: DaemonConfig) -> Result<QuicHub> {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
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

    let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(client_config);

    Ok(QuicHub {
        endpoint,
        host: config.effective_quic_host(),
        port: config.quic_port,
    })
}

pub struct QuicHub {
    endpoint: Endpoint,
    host: String,
    port: u16,
}

impl QuicHub {
    pub async fn open_session_stream(
        &self,
        session_id: String,
        proxy_tx: mpsc::Sender<ProxyCommand>,
    ) -> Result<mpsc::Sender<Vec<u8>>> {
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(512);

        let endpoint = self.endpoint.clone();
        let host = self.host.clone();
        let port = self.port;

        tokio::spawn(async move {
            quic_stream_loop(endpoint, host, port, session_id, proxy_tx, output_rx).await;
        });

        Ok(output_tx)
    }
}

async fn quic_stream_loop(
    endpoint: Endpoint,
    host: String,
    port: u16,
    session_id: String,
    proxy_tx: mpsc::Sender<ProxyCommand>,
    mut output_rx: mpsc::Receiver<Vec<u8>>,
) {
    let sid = session_id.clone();
    // Calls are strictly sequential, but the borrow has to outlive each closure
    // invocation, so it goes behind a mutex rather than a &mut capture.
    let output_rx = Arc::new(tokio::sync::Mutex::new(output_rx));

    reconnect_loop(session_id, proxy_tx.clone(), move || {
        let endpoint = endpoint.clone();
        let host = host.clone();
        let sid = sid.clone();
        let proxy_tx = proxy_tx.clone();
        let output_rx = output_rx.clone();
        async move {
            let mut rx = output_rx.lock().await;
            let mut outcome = StreamOutcome::default();
            match connect_and_run_stream(
                &endpoint,
                &host,
                port,
                &sid,
                &proxy_tx,
                &mut rx,
                &mut outcome,
            )
            .await
            {
                Ok(()) => info!(session_id = %sid, "QUIC stream closed normally"),
                Err(e) => warn!(session_id = %sid, error = %e, "QUIC stream error"),
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
        let outcome = connect().await;

        // Only tell the proxy to restore its local window size if a client was
        // genuinely attached. Firing this unconditionally SIGWINCHes the user's
        // shell on every reconnect attempt, even when nobody was ever connected.
        if outcome.client_attached {
            proxy_tx.send(ProxyCommand::ClientDetached).await.ok();
        }

        // Check if the proxy is still alive (output_rx not closed)
        if proxy_tx.is_closed() {
            info!(session_id = %session_id, "proxy gone, stopping QUIC reconnect");
            break;
        }

        // A stream that was established and later dropped is a success, not a
        // failure: without this reset the daemon ratchets up to the cap and
        // stays there for the rest of the process's life.
        if outcome.established {
            backoff = INITIAL_BACKOFF_SECS;
        }

        info!(session_id = %session_id, backoff_s = backoff, "reconnecting QUIC stream...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
    }
}

async fn connect_and_run_stream(
    endpoint: &Endpoint,
    host: &str,
    port: u16,
    session_id: &str,
    proxy_tx: &mpsc::Sender<ProxyCommand>,
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
    outcome: &mut StreamOutcome,
) -> Result<()> {
    let addr_str = format!("{}:{}", host, port);
    let addr = tokio::net::lookup_host(&addr_str)
        .await
        .context("DNS resolution failed")?
        .find(|a| a.is_ipv4())
        .or_else(|| addr_str.parse().ok())
        .context("could not resolve hub QUIC address")?;

    let conn = endpoint
        .connect(addr, host)?
        .await
        .context("QUIC connection failed")?;

    let (mut send, mut recv) = conn.open_bi().await?;

    // Stream header: 0x01 = daemon, then session_id
    send.write_all(&[0x01]).await?;
    let sid_bytes = session_id.as_bytes();
    send.write_all(&(sid_bytes.len() as u16).to_be_bytes())
        .await?;
    send.write_all(sid_bytes).await?;

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
        loop {
            match read_command_frame(&mut recv).await {
                Ok(Some(cmd)) => {
                    info!(session_id = %session_id, cmd = ?cmd, "QUIC → proxy command");
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
                Ok(None) | Err(_) => break,
            }
        }
    };

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    outcome.client_attached = client_attached.load(std::sync::atomic::Ordering::Relaxed);

    Ok(())
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

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
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
}
