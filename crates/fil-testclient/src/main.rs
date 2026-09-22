//! QUIC verification client. Credentials come only from the environment.
//! Client v3 (0x13) returns the *start* cursor followed by raw terminal bytes.
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use clap::{Parser, ValueEnum};
use quinn::{Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::{Instant, timeout, timeout_at};

const MAX_INPUT: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum ExitMode {
    Graceful,
    Hard,
}

#[derive(Parser)]
#[command(about = "Pinned, ticket-authenticated Fil QUIC verification client")]
struct Cli {
    #[arg(long)]
    session: String,
    #[arg(long, default_value = "127.0.0.1:16433")]
    addr: SocketAddr,
    #[arg(long, default_value = "localhost")]
    server_name: String,
    /// Input bytes (framed for clients; initial raw output for synthetic daemons).
    #[arg(long)]
    send: Option<String>,
    /// Delay before sending, seconds (must be less than --read-secs).
    #[arg(long, default_value_t = 0.5)]
    send_after: f64,
    /// Total payload-read budget, seconds; not reset by incoming traffic.
    #[arg(long, default_value_t = 5.0)]
    read_secs: f64,
    #[arg(long, default_value_t = 5.0)]
    connect_timeout_secs: f64,
    /// Budget for stream setup, individual writes, and the initial cursor.
    #[arg(long, default_value_t = 5.0)]
    io_timeout_secs: f64,
    /// Fail unless this exact UTF-8 byte sequence is observed, even across reads.
    #[arg(long)]
    expect: Option<String>,
    /// Last consumed cursor from SUMMARY; zero requests a cold catch-up.
    #[arg(long, default_value_t = 0)]
    resume_from: u64,
    /// hard exits with status 9 without sending a QUIC close frame.
    #[arg(long, value_enum, default_value = "graceful")]
    exit_mode: ExitMode,
    /// Explicitly print escaped payload (may contain sensitive terminal data).
    #[arg(long)]
    echo: bool,
    /// 0x13 = client v3; 0x11 = synthetic echo daemon (no shell/PTY).
    /// Legacy 0x01/0x02 require --insecure. 0x12 is never supported.
    #[arg(long, default_value = "0x13", value_parser = parse_stream_type)]
    stream_type: u8,
    /// Terminal size as COLSxROWS, sent immediately on attach.
    #[arg(long)]
    resize: Option<String>,
    /// Allow an unpinned connection ONLY when FIL_QUIC_CERTIFICATE is absent.
    /// A supplied pin is always enforced, even with this flag.
    #[arg(long)]
    insecure: bool,
    /// macOS only: bind this IPv4 UDP socket to an interface, e.g. en0 or lo0.
    /// Does not change routes, VPN settings, or other sockets.
    #[arg(long)]
    interface: Option<String>,
}

fn parse_stream_type(value: &str) -> Result<u8, String> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse::<u8>(), |hex| u8::from_str_radix(hex, 16))
        .map_err(|_| "expected a byte in decimal or 0x-prefixed hex".into())
}

fn seconds(value: f64, name: &str, allow_zero: bool) -> Result<Duration> {
    ensure!(
        value.is_finite() && value >= 0.0 && (allow_zero || value > 0.0) && value <= 86400.0,
        "{name} must be {} and no greater than 86400 seconds",
        if allow_zero {
            "nonnegative"
        } else {
            "positive"
        }
    );
    Ok(Duration::from_secs_f64(value))
}

fn resize_frame(spec: &str) -> Result<Vec<u8>> {
    let (c, r) = spec
        .split_once(['x', 'X'])
        .context("--resize expects COLSxROWS")?;
    let cols: u16 = c.trim().parse().context("invalid resize columns")?;
    let rows: u16 = r.trim().parse().context("invalid resize rows")?;
    ensure!(cols > 0 && rows > 0, "resize dimensions must be nonzero");
    let mut frame = vec![0x01];
    frame.extend_from_slice(&cols.to_be_bytes());
    frame.extend_from_slice(&rows.to_be_bytes());
    Ok(frame)
}

fn input_frame(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len() <= MAX_INPUT,
        "input exceeds the hub's 1 MiB frame limit"
    );
    let mut frame = vec![0x00];
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(data);
    Ok(frame)
}

fn attach_header(cli: &Cli) -> Result<Vec<u8>> {
    let sid = cli.session.as_bytes();
    ensure!(
        !sid.is_empty() && sid.len() <= u16::MAX as usize,
        "session id must be 1..65535 bytes"
    );
    let mut header = vec![cli.stream_type];
    header.extend_from_slice(&(sid.len() as u16).to_be_bytes());
    header.extend_from_slice(sid);
    match cli.stream_type {
        0x11 | 0x13 => {
            // VarError::NotUnicode includes the original value in its Display.
            // Never attach that error as a source: the ticket is a credential.
            let ticket = std::env::var("FIL_ATTACH_TICKET").map_err(|_| anyhow::anyhow!(
                "FIL_ATTACH_TICKET is required as UTF-8; obtain a fresh role-specific ticket via authenticated HTTP"
            ))?;
            ensure!(
                !ticket.is_empty() && ticket.len() <= 128,
                "FIL_ATTACH_TICKET must be 1..128 bytes"
            );
            header.extend_from_slice(&(ticket.len() as u16).to_be_bytes());
            header.extend_from_slice(ticket.as_bytes());
            if cli.stream_type == 0x13 {
                header.extend_from_slice(&cli.resume_from.to_be_bytes());
            }
        }
        0x01 | 0x02 if cli.insecure => {}
        0x01 | 0x02 => bail!("legacy streams require explicit --insecure for negative tests"),
        0x12 => bail!("legacy v2 (0x12) is not supported; use v3 (0x13) start cursors"),
        _ => bail!("unsupported stream type"),
    }
    Ok(header)
}

fn ts() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        d.subsec_millis()
    )
}

/// Only reachable through an explicit negative-test flag and an absent pin.
#[derive(Debug)]
struct UnpinnedForNegativeTest;

impl rustls::client::danger::ServerCertVerifier for UnpinnedForNegativeTest {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn client_config(cli: &Cli) -> Result<quinn::ClientConfig> {
    let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> = match std::env::var(
        "FIL_QUIC_CERTIFICATE",
    ) {
        Ok(value) => {
            let der = base64::engine::general_purpose::STANDARD
                .decode(value.trim())
                .context("FIL_QUIC_CERTIFICATE must contain base64 DER")?;
            ensure!(!der.is_empty(), "FIL_QUIC_CERTIFICATE is empty");
            Arc::new(fil_protocol::tls::PinnedServerCertificate(der))
        }
        Err(std::env::VarError::NotPresent) if cli.insecure => {
            eprintln!("WARNING: --insecure disables certificate pinning (negative tests only)");
            Arc::new(UnpinnedForNegativeTest)
        }
        _ => bail!(
            "FIL_QUIC_CERTIFICATE is required (base64 DER); --insecure is only for deliberate negative tests"
        ),
    };
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"fil".to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(15).try_into()?));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

fn endpoint(cli: &Cli) -> Result<Endpoint> {
    let bind: SocketAddr = if cli.addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    }
    .parse()?;
    if let Some(interface) = &cli.interface {
        #[cfg(target_os = "macos")]
        {
            ensure!(
                cli.addr.is_ipv4(),
                "--interface currently supports IPv4 only"
            );
            let name =
                std::ffi::CString::new(interface.as_str()).context("invalid interface name")?;
            // if_nametoindex reads an interface index; it does not change networking.
            let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
            let index = std::num::NonZeroU32::new(index).context("unknown network interface")?;
            let socket = std::net::UdpSocket::bind(bind)?;
            socket.set_nonblocking(true)?;
            // socket2 0.6's name for the macOS IP_BOUND_IF socket option.
            socket2::SockRef::from(&socket).bind_device_by_index_v4(Some(index))?;
            return Ok(Endpoint::new(
                quinn::EndpointConfig::default(),
                None,
                socket,
                Arc::new(quinn::TokioRuntime),
            )?);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = interface;
            bail!("--interface is supported on macOS only");
        }
    }
    Ok(Endpoint::client(bind)?)
}

/// Streaming KMP matching: bounded memory, including split UTF-8 and overlaps.
struct Expected {
    bytes: Vec<u8>,
    prefix: Vec<usize>,
    matched: usize,
    seen: bool,
}

impl Expected {
    fn new(value: &str) -> Self {
        let bytes = value.as_bytes().to_vec();
        let mut prefix = vec![0; bytes.len()];
        let mut matched = 0;
        for i in 1..bytes.len() {
            while matched > 0 && bytes[i] != bytes[matched] {
                matched = prefix[matched - 1];
            }
            if bytes[i] == bytes[matched] {
                matched += 1;
            }
            prefix[i] = matched;
        }
        Self {
            bytes,
            prefix,
            matched: 0,
            seen: value.is_empty(),
        }
    }
    fn observe(&mut self, data: &[u8]) {
        if self.seen {
            return;
        }
        for byte in data {
            while self.matched > 0 && *byte != self.bytes[self.matched] {
                self.matched = self.prefix[self.matched - 1];
            }
            if *byte == self.bytes[self.matched] {
                self.matched += 1;
            }
            if self.matched == self.bytes.len() {
                self.seen = true;
                break;
            }
        }
    }
}

struct Stats {
    received: u64,
    start: Option<u64>,
    cursor: Option<u64>,
    expected: Option<Expected>,
}

impl Stats {
    fn observe(&mut self, data: &[u8], echo: bool) -> Result<()> {
        self.received = self
            .received
            .checked_add(data.len() as u64)
            .context("byte count overflow")?;
        if let Some(cursor) = self.cursor.as_mut() {
            *cursor = cursor
                .checked_add(data.len() as u64)
                .context("cursor overflow")?;
        }
        if let Some(expected) = self.expected.as_mut() {
            expected.observe(data);
        }
        if echo {
            eprintln!("[{}] DATA {:?}", ts(), String::from_utf8_lossy(data));
        }
        Ok(())
    }
    fn assertion(&self) -> Result<()> {
        ensure!(
            self.expected.as_ref().is_none_or(|e| e.seen),
            "--expect assertion failed: requested bytes were never observed"
        );
        Ok(())
    }
    fn summary(&self) {
        let offset = |n: Option<u64>| n.map_or_else(|| "none".into(), |v| v.to_string());
        let expected = self
            .expected
            .as_ref()
            .map_or("none", |e| if e.seen { "matched" } else { "missing" });
        eprintln!(
            "SUMMARY received_bytes={} start_offset={} cursor={} expect={expected}",
            self.received,
            offset(self.start),
            offset(self.cursor)
        );
    }
}

async fn write(send: &mut SendStream, data: &[u8], budget: Duration) -> Result<()> {
    timeout(budget, send.write_all(data))
        .await
        .context("QUIC write timed out")?
        .context("QUIC write failed")
}

async fn read_client(
    cli: &Cli,
    send: &mut SendStream,
    recv: &mut RecvStream,
    stats: &mut Stats,
    io: Duration,
    read_for: Duration,
    send_after: Duration,
) -> Result<()> {
    if cli.stream_type == 0x13 {
        let mut offset = [0; 8];
        timeout(io, recv.read_exact(&mut offset))
            .await
            .context("attach response timed out")?
            .context("attach rejected or truncated start cursor")?;
        let start = u64::from_be_bytes(offset);
        stats.start = Some(start);
        stats.cursor = Some(start);
        eprintln!("[{}] ATTACHED start_offset={start}", ts());
    }
    if let Some(spec) = &cli.resize {
        write(send, &resize_frame(spec)?, io).await?;
        eprintln!("[{}] RESIZE_SENT", ts());
    }
    let started = Instant::now();
    let deadline = started + read_for;
    let send_at = started + send_after;
    let mut pending_input = cli
        .send
        .as_deref()
        .map(|s| input_frame(s.as_bytes()))
        .transpose()?;
    let mut buf = vec![0; 65536];
    loop {
        // read() is cancellation safe; there is just one owned send stream.
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tokio::time::sleep_until(send_at), if pending_input.is_some() => {
                let frame = pending_input.take().unwrap();
                timeout_at(deadline, write(send, &frame, io)).await.context("input send exceeded read budget")??;
                eprintln!("[{}] INPUT_SENT bytes={}", ts(), frame.len() - 5);
            }
            result = recv.read(&mut buf) => {
                match result.context("QUIC payload read failed")? {
                    Some(n) => stats.observe(&buf[..n], cli.echo)?,
                    None => bail!("server closed the stream before the read budget elapsed"),
                }
            }
        }
    }
    ensure!(
        pending_input.is_none(),
        "configured input was not sent before the read deadline"
    );
    stats.assertion()
}

enum DaemonCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
    Attached,
    Detached,
}

async fn daemon_command(recv: &mut RecvStream) -> Result<DaemonCommand> {
    let mut kind = [0];
    recv.read_exact(&mut kind)
        .await
        .context("daemon stream closed or rejected")?;
    Ok(match kind[0] {
        0x00 => {
            let mut len = [0; 4];
            recv.read_exact(&mut len).await?;
            let len = u32::from_be_bytes(len) as usize;
            ensure!(len <= MAX_INPUT, "oversized daemon input frame");
            let mut data = vec![0; len];
            recv.read_exact(&mut data).await?;
            DaemonCommand::Input(data)
        }
        0x01 => {
            let mut size = [0; 4];
            recv.read_exact(&mut size).await?;
            DaemonCommand::Resize(
                u16::from_be_bytes([size[0], size[1]]),
                u16::from_be_bytes([size[2], size[3]]),
            )
        }
        0x02 => DaemonCommand::Attached,
        0x03 => DaemonCommand::Detached,
        _ => bail!("unknown daemon command frame"),
    })
}

async fn echo_daemon(
    cli: &Cli,
    send: &mut SendStream,
    recv: &mut RecvStream,
    stats: &mut Stats,
    io: Duration,
    read_for: Duration,
    send_after: Duration,
) -> Result<()> {
    let started = Instant::now();
    let deadline = started + read_for;
    if let Some(output) = &cli.send {
        tokio::time::sleep_until(started + send_after).await;
        timeout_at(deadline, write(send, output.as_bytes(), io))
            .await
            .context("initial daemon output timed out")??;
    }
    eprintln!("[{}] SYNTHETIC_DAEMON_READY (no shell or PTY)", ts());
    let mut commands = 0;
    loop {
        let command = match timeout_at(deadline, daemon_command(recv)).await {
            Ok(result) => result?,
            Err(_) => break,
        };
        commands += 1;
        let response = match command {
            DaemonCommand::Input(data) => {
                stats.observe(&data, cli.echo)?;
                eprintln!("[{}] DAEMON_INPUT bytes={}", ts(), data.len());
                data
            }
            DaemonCommand::Resize(cols, rows) => {
                eprintln!("[{}] DAEMON_RESIZE cols={cols} rows={rows}", ts());
                format!("SMOKE_RESIZE={cols}x{rows}\n").into_bytes()
            }
            DaemonCommand::Attached => {
                eprintln!("[{}] DAEMON_ATTACHED", ts());
                continue;
            }
            DaemonCommand::Detached => {
                eprintln!("[{}] DAEMON_DETACHED", ts());
                b"SMOKE_DETACHED\n".to_vec()
            }
        };
        timeout_at(deadline, write(send, &response, io))
            .await
            .context("daemon echo exceeded read budget")??;
    }
    // A submitted header is not proof of authentication. Demand hub traffic.
    ensure!(commands > 0, "synthetic daemon received no hub commands");
    stats.assertion()
}

async fn run(cli: &Cli, stats: &mut Stats) -> Result<()> {
    let connect = seconds(cli.connect_timeout_secs, "--connect-timeout-secs", false)?;
    let io = seconds(cli.io_timeout_secs, "--io-timeout-secs", false)?;
    let read_for = seconds(cli.read_secs, "--read-secs", false)?;
    let send_after = seconds(cli.send_after, "--send-after", true)?;
    if cli.send.is_some() {
        ensure!(
            send_after < read_for,
            "--send-after must be less than --read-secs"
        );
    }
    if let Some(spec) = &cli.resize {
        resize_frame(spec)?;
    }
    let daemon = matches!(cli.stream_type, 0x01 | 0x11);
    ensure!(
        !daemon || (cli.resize.is_none() && cli.resume_from == 0),
        "daemon mode does not accept --resize or --resume-from"
    );
    if !daemon && let Some(data) = &cli.send {
        input_frame(data.as_bytes())?;
    }
    if let Some(expected) = &cli.expect {
        ensure!(!expected.is_empty(), "--expect must not be empty");
    }
    let config = client_config(cli)?;
    let header = attach_header(cli)?;
    let mut endpoint = endpoint(cli)?;
    endpoint.set_default_client_config(config);
    eprintln!("[{}] CONNECTING addr={}", ts(), cli.addr);
    let connection = timeout(connect, endpoint.connect(cli.addr, &cli.server_name)?)
        .await
        .context("QUIC connect timed out")?
        .context("QUIC connect failed")?;
    eprintln!(
        "[{}] CONNECTED local_port={}",
        ts(),
        endpoint.local_addr()?.port()
    );
    let result: Result<()> = async {
        let (mut send, mut recv) = timeout(io, connection.open_bi())
            .await
            .context("opening stream timed out")??;
        write(&mut send, &header, io).await?;
        eprintln!(
            "[{}] ATTACH_SENT type=0x{:02x} sid_len={}",
            ts(),
            cli.stream_type,
            cli.session.len()
        );
        let result = if daemon {
            echo_daemon(cli, &mut send, &mut recv, stats, io, read_for, send_after).await
        } else {
            read_client(cli, &mut send, &mut recv, stats, io, read_for, send_after).await
        };
        if result.is_ok() && cli.exit_mode == ExitMode::Hard {
            stats.summary();
            eprintln!("[{}] HARD_EXIT (no QUIC CONNECTION_CLOSE sent)", ts());
            use std::io::Write;
            std::io::stderr().flush().ok();
            std::process::exit(9);
        }
        let detach = if !daemon && stats.start.is_some() {
            write(&mut send, &[0x02], io).await
        } else {
            Ok(())
        };
        let finish = send.finish();
        result?;
        detach?;
        finish.context("finishing stream failed")?;
        Ok(())
    }
    .await;
    connection.close(0u32.into(), b"smoke complete");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut stats = Stats {
        received: 0,
        start: None,
        cursor: None,
        expected: cli.expect.as_deref().map(Expected::new),
    };
    let result = run(&cli, &mut stats).await;
    stats.summary();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ERROR: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
