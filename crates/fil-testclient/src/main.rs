// Minimal QUIC client speaking the same stream handshake as the iOS app:
//   stream type 0x02, [u16 sid_len][session_id], then read bytes.
use anyhow::Result;
use clap::Parser;
use quinn::Endpoint;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    session: String,
    #[arg(long, default_value = "127.0.0.1:16433")]
    addr: String,
    #[arg(long, default_value = "localhost")]
    server_name: String,
    /// Bytes to send to the session after attaching (client -> daemon input)
    #[arg(long)]
    send: Option<String>,
    /// Delay before sending, seconds
    #[arg(long, default_value_t = 0.5)]
    send_after: f64,
    /// How long to stay attached and read, seconds
    #[arg(long, default_value_t = 5.0)]
    read_secs: f64,
    /// graceful = close connection properly; hard = process::exit without any QUIC close frame
    #[arg(long, default_value = "graceful")]
    exit_mode: String,
    /// Also print received payload bytes as escaped text
    #[arg(long, default_value_t = false)]
    echo: bool,
    /// Stream type byte (0x02 = client attach)
    #[arg(long, default_value_t = 2)]
    stream_type: u8,
    /// Declare a terminal size right after attaching, as COLSxROWS (e.g. 40x25).
    /// This is what the iOS app does, and it is what makes the detach-time
    /// restore a real resize rather than a no-op.
    #[arg(long)]
    resize: Option<String>,
}

/// Client resize frame: [0x01][u16 cols BE][u16 rows BE]
fn resize_frame(spec: &str) -> Option<Vec<u8>> {
    let (c, r) = spec.split_once(['x', 'X'])?;
    let cols: u16 = c.trim().parse().ok()?;
    let rows: u16 = r.trim().parse().ok()?;
    let mut f = vec![0x01u8];
    f.extend_from_slice(&cols.to_be_bytes());
    f.extend_from_slice(&rows.to_be_bytes());
    Some(f)
}

fn ts() -> String {
    let now = SystemTime::now();
    let d = now.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let secs = d.as_secs();
    let ms = d.subsec_millis();
    // UTC HH:MM:SS.mmm
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

#[derive(Debug)]
struct SkipVerify;

impl rustls::client::danger::ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _e: &rustls::pki_types::CertificateDer<'_>,
        _i: &[rustls::pki_types::CertificateDer<'_>],
        _n: &rustls::pki_types::ServerName<'_>,
        _o: &[u8],
        _t: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"fil".to_vec()];

    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));

    let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(client_config);

    let addr: SocketAddr = cli.addr.parse()?;
    eprintln!("[{}] CONNECTING addr={addr}", ts());
    let conn = endpoint.connect(addr, &cli.server_name)?.await?;
    eprintln!("[{}] CONNECTED local_port={}", ts(), endpoint.local_addr()?.port());

    let (mut send, mut recv) = conn.open_bi().await?;

    // handshake: [stream_type][u16 sid_len][session_id]
    let sid = cli.session.as_bytes();
    let mut hdr = Vec::new();
    hdr.push(cli.stream_type);
    hdr.extend_from_slice(&(sid.len() as u16).to_be_bytes());
    hdr.extend_from_slice(sid);
    send.write_all(&hdr).await?;
    let t_attach = Instant::now();
    eprintln!(
        "[{}] ATTACH_SENT type=0x{:02x} sid_len={} sid={}",
        ts(),
        cli.stream_type,
        sid.len(),
        cli.session
    );

    if let Some(spec) = cli.resize.as_deref() {
        match resize_frame(spec) {
            Some(frame) => {
                send.write_all(&frame).await?;
                eprintln!("[{}] RESIZE_SENT {spec}", ts());
            }
            None => anyhow::bail!("bad --resize value {spec:?}, expected COLSxROWS"),
        }
    }

    if let Some(payload) = cli.send.clone() {
        let after = Duration::from_secs_f64(cli.send_after);
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            eprintln!("[{}] SEND {} bytes: {:?}", ts(), payload.len(), payload);
            let _ = send.write_all(payload.as_bytes()).await;
            // keep `send` alive for the rest of the process
            std::mem::forget(send);
        });
    }

    let echo = cli.echo;
    let read_task = async move {
        let mut buf = vec![0u8; 65536];
        let mut total: usize = 0;
        let mut first_burst: usize = 0;
        let mut burst_open = true;
        let mut last = Instant::now();
        loop {
            match recv.read(&mut buf).await {
                Ok(Some(n)) => {
                    let now = Instant::now();
                    if now.duration_since(last) > Duration::from_millis(300) {
                        burst_open = false;
                    }
                    if burst_open {
                        first_burst += n;
                    }
                    last = now;
                    total += n;
                    eprintln!(
                        "[{}] RECV n={n} total={total} t+{:.3}s{}",
                        ts(),
                        t_attach.elapsed().as_secs_f64(),
                        if echo {
                            format!(" data={:?}", String::from_utf8_lossy(&buf[..n.min(200)]))
                        } else {
                            String::new()
                        }
                    );
                }
                Ok(None) => {
                    eprintln!("[{}] STREAM_FIN total={total}", ts());
                    break;
                }
                Err(e) => {
                    eprintln!("[{}] READ_ERR {e} total={total}", ts());
                    break;
                }
            }
        }
        (total, first_burst)
    };

    let handle = tokio::spawn(read_task);
    tokio::time::sleep(Duration::from_secs_f64(cli.read_secs)).await;

    if cli.exit_mode == "hard" {
        eprintln!("[{}] HARD_EXIT (no QUIC CONNECTION_CLOSE sent)", ts());
        // Flush stderr, then die without running destructors.
        use std::io::Write;
        std::io::stderr().flush().ok();
        std::process::exit(9);
    }

    handle.abort();
    eprintln!("[{}] GRACEFUL_CLOSE", ts());
    conn.close(0u32.into(), b"bye");
    endpoint.wait_idle().await;
    Ok(())
}
