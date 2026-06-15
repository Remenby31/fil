use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use crate::config::DaemonConfig;

struct TerminalInfo {
    name: &'static str,
    config_path: PathBuf,
}

pub async fn run_setup(hub_url: Option<String>) -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m v{}\n", env!("CARGO_PKG_VERSION"));

    let mut config = DaemonConfig::load();
    if let Some(url) = hub_url {
        config.hub_url = url;
    }

    // Step 1: Detect terminals
    println!("  \x1b[2m[1/4]\x1b[0m Detecting terminals...");
    let terminals = detect_terminals();

    if terminals.is_empty() {
        println!("    \x1b[33m!\x1b[0m No supported terminals detected");
    } else {
        for t in &terminals {
            println!("    \x1b[32m✓\x1b[0m {}", t.name);
        }
    }

    // Step 2: Configure terminals
    if !terminals.is_empty() {
        println!("\n  \x1b[2m[2/4]\x1b[0m Configuring terminals...");

        let fil_path = which_fil();

        for t in &terminals {
            if let Err(e) = configure_terminal(t, &fil_path) {
                println!("    \x1b[31m✗\x1b[0m {} — {}", t.name, e);
            } else {
                println!("    \x1b[32m✓\x1b[0m {}", t.name);
            }
        }
    }

    // Step 3: Authenticate via local OAuth callback server
    println!("\n  \x1b[2m[3/4]\x1b[0m Authenticating...");

    let token = run_oauth_flow(&config.hub_url).await?;
    config.token = token;

    println!("    \x1b[32m✓\x1b[0m Signed in");

    // Step 4: Register device
    println!("\n  \x1b[2m[4/4]\x1b[0m Registering device...");

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

    // Done
    println!("\n  \x1b[32m✓ All set!\x1b[0m");
    println!("  Restart your terminal to activate fil.\n");

    Ok(())
}

async fn run_oauth_flow(hub_url: &str) -> Result<String> {
    // Start a local HTTP server to receive the OAuth callback
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_port = listener.local_addr()?.port();
    let callback_url = format!("http://localhost:{local_port}/callback");

    let (tx, rx) = oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));

    // Build the auth URL — we pass our local callback as a query param
    // The hub will redirect GitHub's callback to us
    let auth_url = format!(
        "{}/auth/github/start?cli_callback={}",
        hub_url,
        urlencoding::encode(&callback_url)
    );

    // Open browser
    println!("    Opening browser...");
    if open::that(&auth_url).is_err() {
        println!("\n    Open this URL in your browser:");
        println!("    \x1b[4m{auth_url}\x1b[0m\n");
    }

    // Wait for the callback with the token
    println!("    Waiting for authentication...");

    let server = async {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await?;
            let request = String::from_utf8_lossy(&buf[..n]);

            // Parse the token from the query string
            if let Some(token) = extract_token_from_request(&request) {
                // Send success HTML response
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

    // Timeout after 2 minutes
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

    if !path.starts_with("/callback") {
        return None;
    }

    let url = url::Url::parse(&format!("http://localhost{path}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.to_string())
}

fn which_fil() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| {
            // Check common locations
            for path in &["/usr/local/bin/fil", "/opt/homebrew/bin/fil"] {
                let p = PathBuf::from(path);
                if p.exists() { return p; }
            }
            PathBuf::from("fil")
        })
}

pub fn run_uninstall() -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m uninstall\n");

    let terminals = detect_terminals();
    for t in &terminals {
        if let Err(e) = unconfigure_terminal(t) {
            println!("  \x1b[31m✗\x1b[0m {} — {}", t.name, e);
        } else {
            println!("  \x1b[32m✓\x1b[0m {} restored", t.name);
        }
    }

    let config_dir = DaemonConfig::config_dir();
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir)?;
        println!("  \x1b[32m✓\x1b[0m Config removed");
    }

    println!("\n  Uninstalled. Restart your terminal.\n");
    Ok(())
}

fn detect_terminals() -> Vec<TerminalInfo> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut found = Vec::new();

    let candidates = [
        ("Ghostty", home.join(".config/ghostty/config")),
        ("kitty", home.join(".config/kitty/kitty.conf")),
        ("Alacritty", home.join(".config/alacritty/alacritty.toml")),
        ("WezTerm", home.join(".wezterm.lua")),
    ];

    for (name, path) in candidates {
        if path.exists() {
            found.push(TerminalInfo { name, config_path: path });
        }
    }

    found
}

fn configure_terminal(terminal: &TerminalInfo, fil_path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(&terminal.config_path)?;
    let fil_abs = fil_path.to_string_lossy();

    if content.contains("fil") {
        return Ok(());
    }

    // Backup
    let backup = format!("{}.fil-backup", terminal.config_path.display());
    std::fs::copy(&terminal.config_path, &backup)?;

    let addition = match terminal.name {
        "Ghostty" => format!("\n# Added by fil setup\ncommand = {fil_abs}\n"),
        "kitty" => format!("\n# Added by fil setup\nshell {fil_abs}\n"),
        "Alacritty" => format!("\n# Added by fil setup\n[shell]\nprogram = \"{fil_abs}\"\n"),
        "WezTerm" => format!("\n-- Added by fil setup\nconfig.default_prog = {{ '{fil_abs}' }}\n"),
        _ => return Ok(()),
    };

    std::fs::write(&terminal.config_path, format!("{content}{addition}"))?;
    Ok(())
}

fn unconfigure_terminal(terminal: &TerminalInfo) -> Result<()> {
    let backup = format!("{}.fil-backup", terminal.config_path.display());
    if std::path::Path::new(&backup).exists() {
        std::fs::copy(&backup, &terminal.config_path)?;
        std::fs::remove_file(&backup)?;
    }
    Ok(())
}
