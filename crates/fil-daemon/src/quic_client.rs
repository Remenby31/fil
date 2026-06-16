use anyhow::{Context, Result};
use quinn::Endpoint;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub struct QuicDataClient {
    hub_host: String,
    hub_port: u16,
}

impl QuicDataClient {
    pub fn new(hub_url: &str, quic_port: u16) -> Self {
        let host = hub_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("localhost")
            .to_string();

        Self {
            hub_host: host,
            hub_port: quic_port,
        }
    }

    pub async fn connect_and_stream(
        &self,
        session_id: String,
        mut pty_output_rx: mpsc::Receiver<Vec<u8>>,
        client_input_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<()> {
        // Configure client — skip server cert verification for self-signed
        let mut crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth();

        crypto.alpn_protocols = vec![b"fil".to_vec()];

        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
        ));

        let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
        endpoint.set_default_client_config(client_config);

        // Resolve hostname to IP (SocketAddr::parse only accepts IPs)
        let addr_str = format!("{}:{}", self.hub_host, self.hub_port);
        let addr = tokio::net::lookup_host(&addr_str)
            .await
            .context("DNS resolution failed")?
            .find(|a| a.is_ipv4()) // Prefer IPv4
            .or_else(|| {
                // Fallback: try parsing as IP directly
                addr_str.parse().ok()
            })
            .context("could not resolve hub address")?;

        info!(addr = %addr, "connecting QUIC to hub");

        let conn = endpoint
            .connect(addr, &self.hub_host)?
            .await
            .context("QUIC connection failed")?;

        info!("QUIC connected to hub");

        // Open bidirectional stream for this session
        let (mut send, mut recv) = conn.open_bi().await?;

        // Send stream header: 0x01 = daemon data stream
        send.write_all(&[0x01]).await?;

        // Send session_id (length-prefixed)
        let sid_bytes = session_id.as_bytes();
        send.write_all(&(sid_bytes.len() as u16).to_be_bytes()).await?;
        send.write_all(sid_bytes).await?;

        debug!(session_id = %session_id, "QUIC data stream opened");

        // Forward PTY output to hub
        let send_task = async move {
            while let Some(data) = pty_output_rx.recv().await {
                if send.write_all(&data).await.is_err() {
                    break;
                }
            }
            send.finish().ok();
        };

        // Receive client input from hub
        let recv_task = async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match recv.read(&mut buf).await {
                    Ok(Some(n)) => {
                        if client_input_tx.send(buf[..n].to_vec()).await.is_err() {
                            break;
                        }
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

        info!("QUIC data stream closed");
        Ok(())
    }
}

// Skip server certificate verification for self-signed certs (TOFU)
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
