use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::config::DaemonConfig;

pub async fn run_setup(hub_url: Option<String>) -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m v{}\n", env!("CARGO_PKG_VERSION"));

    let mut config = DaemonConfig::load();
    if let Some(url) = hub_url {
        config.hub_url = url.clone();
        // Derive QUIC host from hub URL
        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("localhost");
        config.quic_host = format!("quic.{host}");
    }

    // Step 1: Authenticate
    println!("  \x1b[2m[1/3]\x1b[0m Authenticating...");

    let token = run_oauth_flow(&config.hub_url).await?;
    config.token = token;

    println!("    \x1b[32m✓\x1b[0m Signed in");

    // Step 2: Register device
    println!("\n  \x1b[2m[2/3]\x1b[0m Registering device...");

    let hostname = gethostname::gethostname()
        .to_string_lossy()
        .to_string();
    config.device_name = hostname.clone();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/devices", config.hub_url))
        .header("Authorization", format!("Bearer {}", config.token))
        .json(&serde_json::json!({
            "name": hostname,
            "os": std::env::consts::OS,
            "hostname": hostname,
        }))
        .send()
        .await
        .context("failed to register device")?;

    if !resp.status().is_success() {
        anyhow::bail!("failed to register device ({})", resp.status());
    }

    #[derive(serde::Deserialize)]
    struct DeviceResp { id: String }

    let device: DeviceResp = resp.json().await?;
    config.device_id = device.id;
    config.save()?;

    println!("    \x1b[32m✓\x1b[0m {}", config.device_name);

    // Step 3: Show instructions
    println!("\n  \x1b[2m[3/3]\x1b[0m Ready!");
    println!();
    println!("  \x1b[32m✓ All set!\x1b[0m Config saved to \x1b[2m{}\x1b[0m", DaemonConfig::config_path().display());
    println!();
    println!("  To use fil, run it in any terminal:");
    println!("    \x1b[32m$\x1b[0m fil");
    println!();
    println!("  Or configure your terminal to use it as the default shell:");
    println!("    \x1b[2mGhostty:\x1b[0m  command = /usr/local/bin/fil");
    println!("    \x1b[2mkitty:\x1b[0m    shell /usr/local/bin/fil");
    println!();

    Ok(())
}

async fn run_oauth_flow(hub_url: &str) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_port = listener.local_addr()?.port();
    let callback_url = format!("http://localhost:{local_port}/callback");

    let (tx, rx) = oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));

    let auth_url = format!(
        "{}/auth/github/start?cli_callback={}",
        hub_url,
        urlencoding::encode(&callback_url)
    );

    println!("    Opening browser...");
    if open::that(&auth_url).is_err() {
        println!("\n    Open this URL in your browser:");
        println!("    \x1b[4m{auth_url}\x1b[0m\n");
    }

    println!("    Waiting for authentication...");

    let server = async {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await?;
            let request = String::from_utf8_lossy(&buf[..n]);

            if let Some(token) = extract_token_from_request(&request) {
                let html = r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<style>body{background:#0A0A0F;color:#FAFAFA;font-family:-apple-system,sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0}
.box{text-align:center}.logo{font-size:48px;font-weight:300;margin-bottom:8px}.dot{color:#00D4AA}.sub{color:rgba(250,250,250,0.5);font-size:16px}.check{font-size:48px;margin-bottom:16px}</style>
</head><body><div class="box"><div class="check">✓</div><div class="logo">fil<span class="dot">.sh</span></div><div class="sub">Authenticated! You can close this tab.</div></div></body></html>"#;

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(), html
                );
                stream.write_all(response.as_bytes()).await.ok();
                stream.flush().await.ok();

                if let Some(sender) = tx.lock().unwrap().take() {
                    sender.send(token).ok();
                }
                break;
            } else {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(response.as_bytes()).await.ok();
            }
        }
        Ok::<(), anyhow::Error>(())
    };

    let token = tokio::select! {
        _ = server => {
            rx.await.context("failed to receive token")?
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => {
            anyhow::bail!("authentication timed out (2 minutes)");
        }
    };

    Ok(token)
}

fn extract_token_from_request(request: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    if !path.starts_with("/callback") { return None; }
    let url = url::Url::parse(&format!("http://localhost{path}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.to_string())
}

pub fn run_uninstall() -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m uninstall\n");

    let config_dir = DaemonConfig::config_dir();
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir)?;
        println!("  \x1b[32m✓\x1b[0m Config removed");
    }

    println!("\n  Uninstalled.\n");
    Ok(())
}
