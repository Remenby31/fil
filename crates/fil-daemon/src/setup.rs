use anyhow::Result;
use std::path::PathBuf;
use crate::config::DaemonConfig;

struct TerminalInfo {
    name: &'static str,
    config_path: PathBuf,
    config_line: &'static str,
}

pub async fn run_setup(hub_url: Option<String>) -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.\x1b[0m v{}\n", env!("CARGO_PKG_VERSION"));

    let mut config = DaemonConfig::load();
    if let Some(url) = hub_url {
        config.hub_url = url;
    }

    // Step 1: Detect terminals
    println!("  \x1b[1mStep 1\x1b[0m — Detecting terminals...\n");
    let terminals = detect_terminals();

    if terminals.is_empty() {
        println!("  \x1b[33m!\x1b[0m No supported terminals detected.");
        println!("    Fil works with Ghostty, kitty, Alacritty, WezTerm, iTerm2.\n");
    } else {
        for t in &terminals {
            println!("    \x1b[32m✓\x1b[0m {} found", t.name);
        }
        println!();
    }

    // Step 2: Configure terminals
    if !terminals.is_empty() {
        println!("  \x1b[1mStep 2\x1b[0m — Configuring terminals...\n");

        let fil_path = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("fil"));

        for t in &terminals {
            if let Err(e) = configure_terminal(t, &fil_path) {
                println!("    \x1b[31m✗\x1b[0m {} — {}", t.name, e);
            } else {
                println!("    \x1b[32m✓\x1b[0m {} configured", t.name);
            }
        }
        println!();
    }

    // Step 3: Authenticate
    println!("  \x1b[1mStep 3\x1b[0m — Authentication\n");
    println!("  Opening browser for GitHub sign-in...\n");

    let auth_url = format!("{}/auth/github/start", config.hub_url);
    if open::that(&auth_url).is_err() {
        println!("  Open this URL in your browser:");
        println!("  \x1b[4m{auth_url}\x1b[0m\n");
    }

    println!("  Paste the JSON response here:");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    #[derive(serde::Deserialize)]
    struct AuthResponse {
        token: String,
    }

    let auth: AuthResponse = serde_json::from_str(input.trim())?;
    config.token = auth.token;

    // Step 4: Register device
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
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to register device: {}", resp.status());
    }

    #[derive(serde::Deserialize)]
    struct DeviceResp {
        id: String,
    }

    let device: DeviceResp = resp.json().await?;
    config.device_id = device.id;
    config.save()?;

    println!();
    println!("  \x1b[32m✓\x1b[0m Signed in");
    println!("  \x1b[32m✓\x1b[0m Device \x1b[1m{}\x1b[0m registered", config.device_name);
    println!("  \x1b[32m✓\x1b[0m Config saved to \x1b[2m{}\x1b[0m", DaemonConfig::config_path().display());
    println!();
    println!("  \x1b[1mRestart your terminal to activate fil.\x1b[0m\n");

    Ok(())
}

pub fn run_uninstall() -> Result<()> {
    println!("\n  \x1b[1mfil\x1b[32m.\x1b[0m uninstall\n");

    let terminals = detect_terminals();
    for t in &terminals {
        if let Err(e) = unconfigure_terminal(t) {
            println!("  \x1b[31m✗\x1b[0m {} — {}", t.name, e);
        } else {
            println!("  \x1b[32m✓\x1b[0m {} config restored", t.name);
        }
    }

    let config_dir = DaemonConfig::config_dir();
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir)?;
        println!("  \x1b[32m✓\x1b[0m Config directory removed");
    }

    println!("\n  Fil has been uninstalled. Restart your terminal.\n");
    Ok(())
}

fn detect_terminals() -> Vec<TerminalInfo> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut found = Vec::new();

    // Ghostty
    let ghostty_config = home.join(".config/ghostty/config");
    if ghostty_config.exists() {
        found.push(TerminalInfo {
            name: "Ghostty",
            config_path: ghostty_config,
            config_line: "command",
        });
    }

    // kitty
    let kitty_config = home.join(".config/kitty/kitty.conf");
    if kitty_config.exists() {
        found.push(TerminalInfo {
            name: "kitty",
            config_path: kitty_config,
            config_line: "shell",
        });
    }

    // Alacritty
    let alacritty_config = home.join(".config/alacritty/alacritty.toml");
    if alacritty_config.exists() {
        found.push(TerminalInfo {
            name: "Alacritty",
            config_path: alacritty_config,
            config_line: "program", // under [shell]
        });
    }

    // WezTerm
    let wezterm_config = home.join(".wezterm.lua");
    if wezterm_config.exists() {
        found.push(TerminalInfo {
            name: "WezTerm",
            config_path: wezterm_config,
            config_line: "default_prog",
        });
    }

    found
}

fn configure_terminal(terminal: &TerminalInfo, fil_path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(&terminal.config_path)?;
    let fil_abs = fil_path.to_string_lossy();

    // Check if already configured
    if content.contains("fil") {
        return Ok(());
    }

    // Backup
    let backup = format!("{}.fil-backup", terminal.config_path.display());
    std::fs::copy(&terminal.config_path, &backup)?;

    match terminal.name {
        "Ghostty" => {
            let new_content = format!("{content}\n# Added by fil setup\ncommand = {fil_abs}\n");
            std::fs::write(&terminal.config_path, new_content)?;
        }
        "kitty" => {
            let new_content = format!("{content}\n# Added by fil setup\nshell {fil_abs}\n");
            std::fs::write(&terminal.config_path, new_content)?;
        }
        "Alacritty" => {
            let new_content = format!("{content}\n# Added by fil setup\n[shell]\nprogram = \"{fil_abs}\"\n");
            std::fs::write(&terminal.config_path, new_content)?;
        }
        "WezTerm" => {
            let new_content = format!("{content}\n-- Added by fil setup\nconfig.default_prog = {{ '{fil_abs}' }}\n");
            std::fs::write(&terminal.config_path, new_content)?;
        }
        _ => {}
    }

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
