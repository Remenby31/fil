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
    let mut backoff = 1u64;

    loop {
        match connect_and_run_stream(
            &endpoint,
            &host,
            port,
            &session_id,
            &proxy_tx,
            &mut output_rx,
        )
        .await
        {
            Ok(()) => {
                info!(session_id = %session_id, "QUIC stream closed normally");
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "QUIC stream error");
            }
        }

        // Notify proxy that remote client is gone
        proxy_tx.send(ProxyCommand::ClientDetached).await.ok();

        // Check if the proxy is still alive (output_rx not closed)
        if proxy_tx.is_closed() {
            info!(session_id = %session_id, "proxy gone, stopping QUIC reconnect");
            break;
        }

        info!(session_id = %session_id, backoff_s = backoff, "reconnecting QUIC stream...");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn connect_and_run_stream(
    endpoint: &Endpoint,
    host: &str,
    port: u16,
    session_id: &str,
    proxy_tx: &mpsc::Sender<ProxyCommand>,
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
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
