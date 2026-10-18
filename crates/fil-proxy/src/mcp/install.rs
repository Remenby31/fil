//! `fil mcp install|uninstall|status`: register `fil mcp` with the coding
//! agents found on this machine.
//!
//! Every edit is non-destructive: only the `fil` entry is touched, other keys
//! keep their order, the first write leaves a `.fil-backup` copy, files are
//! replaced atomically, and a file with comments (JSONC) is never rewritten:
//! the snippet is printed for the user instead.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVER_NAME: &str = "fil";
/// Above the default `ask` timeout (600s), so the client never cuts a wait short.
const LONG_TOOL_TIMEOUT_SECS: i64 = 660;
/// For clients with a fixed ~60s tool timeout: `ask` returns "pending" before
/// it and the agent follows up with `get_answer`.
const SHORT_CLIENT_WAIT_SECS: &str = "45";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
    Gemini,
    Opencode,
    VsCode,
    ClaudeDesktop,
    Windsurf,
    Vibe,
}

const ALL: [Agent; 9] = [
    Agent::ClaudeCode,
    Agent::Codex,
    Agent::Cursor,
    Agent::Gemini,
    Agent::Opencode,
    Agent::VsCode,
    Agent::ClaudeDesktop,
    Agent::Windsurf,
    Agent::Vibe,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// `{ <key>: { "fil": {...} } }`
    Json(&'static str),
    CodexToml,
    VibeToml,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Gemini => "gemini",
            Agent::Opencode => "opencode",
            Agent::VsCode => "vscode",
            Agent::ClaudeDesktop => "claude-desktop",
            Agent::Windsurf => "windsurf",
            Agent::Vibe => "vibe",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex CLI",
            Agent::Cursor => "Cursor",
            Agent::Gemini => "Gemini CLI",
            Agent::Opencode => "opencode",
            Agent::VsCode => "VS Code (Copilot)",
            Agent::ClaudeDesktop => "Claude Desktop",
            Agent::Windsurf => "Windsurf",
            Agent::Vibe => "Mistral Vibe",
        }
    }

    fn format(self) -> Format {
        match self {
            Agent::VsCode => Format::Json("servers"),
            Agent::Opencode => Format::Json("mcp"),
            Agent::Codex => Format::CodexToml,
            Agent::Vibe => Format::VibeToml,
            _ => Format::Json("mcpServers"),
        }
    }

    /// Clients whose tool-call timeout cannot be raised from their config.
    fn has_short_tool_timeout(self) -> bool {
        matches!(
            self,
            Agent::Cursor
                | Agent::ClaudeDesktop
                | Agent::VsCode
                | Agent::Windsurf
                | Agent::Opencode
        )
    }

    fn config_path(self, env: &Env) -> PathBuf {
        let home = &env.home;
        let app_support = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else {
            env.config_home.clone()
        };
        match self {
            Agent::ClaudeCode => home.join(".claude.json"),
            Agent::Codex => env
                .var("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("config.toml"),
            Agent::Cursor => home.join(".cursor/mcp.json"),
            Agent::Gemini => home.join(".gemini/settings.json"),
            Agent::Opencode => {
                let dir = env.config_home.join("opencode");
                let jsonc = dir.join("opencode.jsonc");
                if jsonc.exists() {
                    jsonc
                } else {
                    dir.join("opencode.json")
                }
            }
            Agent::VsCode => app_support.join("Code/User/mcp.json"),
            Agent::ClaudeDesktop => app_support.join("Claude/claude_desktop_config.json"),
            Agent::Windsurf => home.join(".codeium/windsurf/mcp_config.json"),
            Agent::Vibe => env
                .var("VIBE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".vibe"))
                .join("config.toml"),
        }
    }

    /// Installed means its binary is on PATH or its config directory exists.
    fn detected(self, env: &Env) -> bool {
        let binary = match self {
            Agent::ClaudeCode => Some("claude"),
            Agent::Codex => Some("codex"),
            Agent::Cursor => Some("cursor"),
            Agent::Gemini => Some("gemini"),
            Agent::Opencode => Some("opencode"),
            Agent::VsCode => Some("code"),
            Agent::Windsurf => Some("windsurf"),
            Agent::Vibe => Some("vibe"),
            Agent::ClaudeDesktop => None,
        };
        if binary.is_some_and(|binary| env.on_path(binary)) {
            return true;
        }
        let path = self.config_path(env);
        match self {
            Agent::ClaudeCode => path.exists(),
            Agent::ClaudeDesktop => {
                path.parent().is_some_and(Path::exists)
                    || Path::new("/Applications/Claude.app").exists()
            }
            _ => path.parent().is_some_and(Path::exists),
        }
    }
}

pub struct Env {
    home: PathBuf,
    config_home: PathBuf,
    path: Option<std::ffi::OsString>,
    vars: Vec<(String, String)>,
}

impl Env {
    fn current() -> Result<Self> {
        let home = dirs::home_dir().context("cannot find the home directory")?;
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            home,
            config_home,
            path: std::env::var_os("PATH"),
            vars: ["CODEX_HOME", "VIBE_HOME"]
                .into_iter()
                .filter_map(|name| Some((name.to_string(), std::env::var(name).ok()?)))
                .collect(),
        })
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn on_path(&self, binary: &str) -> bool {
        self.path.as_ref().is_some_and(|path| {
            std::env::split_paths(path).any(|dir| {
                let candidate = dir.join(binary);
                candidate.is_file()
                    && std::fs::metadata(&candidate).is_ok_and(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    })
            })
        })
    }
}

/// What gets registered. GUI apps do not inherit the shell PATH, so the
/// command is the absolute path of this binary (not canonicalized: a Homebrew
/// symlink must survive upgrades).
#[derive(Debug, Clone)]
pub struct Entry {
    command: String,
}

impl Entry {
    fn current() -> Self {
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| "fil".into());
        Self { command }
    }

    fn env(&self, agent: Agent) -> Map<String, Value> {
        let mut env = Map::new();
        if agent.has_short_tool_timeout() {
            env.insert("FIL_ASK_WAIT".into(), json!(SHORT_CLIENT_WAIT_SECS));
        }
        env
    }

    fn json(&self, agent: Agent) -> Value {
        let env = self.env(agent);
        let mut entry = match agent {
            Agent::Opencode => json!({
                "type": "local",
                "command": [self.command, "mcp"],
                "enabled": true,
            }),
            Agent::VsCode | Agent::ClaudeCode => json!({
                "type": "stdio",
                "command": self.command,
                "args": ["mcp"],
            }),
            _ => json!({ "command": self.command, "args": ["mcp"] }),
        };
        if agent == Agent::Gemini {
            entry["timeout"] = json!(LONG_TOOL_TIMEOUT_SECS * 1000);
        }
        if !env.is_empty() {
            let key = if agent == Agent::Opencode {
                "environment"
            } else {
                "env"
            };
            entry[key] = Value::Object(env);
        }
        entry
    }
}

#[derive(Debug, PartialEq)]
pub enum Edit {
    Unchanged,
    Write(String),
    /// The file cannot be edited safely; show this snippet instead.
    Manual {
        reason: String,
        snippet: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

/// Pure: the new content of `agent`'s config file, or why it cannot be written.
pub fn plan_edit(
    agent: Agent,
    content: Option<&str>,
    entry: &Entry,
    action: Action,
) -> Result<Edit> {
    match agent.format() {
        Format::Json(key) => plan_json(agent, key, content, entry, action),
        Format::CodexToml => plan_codex(content, entry, action),
        Format::VibeToml => plan_vibe(content, entry, action),
    }
}

fn normalized(entry: &Value) -> Value {
    let mut entry = entry.clone();
    if let Some(object) = entry.as_object_mut()
        && object
            .get("env")
            .is_some_and(|env| env.as_object().is_some_and(Map::is_empty))
    {
        object.remove("env");
    }
    entry
}

fn plan_json(
    agent: Agent,
    key: &str,
    content: Option<&str>,
    entry: &Entry,
    action: Action,
) -> Result<Edit> {
    let desired = entry.json(agent);
    let text = content.unwrap_or("").trim();
    let mut root: Value = if text.is_empty() {
        json!({})
    } else {
        match serde_json::from_str(text) {
            Ok(root) => root,
            Err(error) if has_comments(text) => {
                let _ = error;
                let snippet =
                    serde_json::to_string_pretty(&json!({ key: { SERVER_NAME: desired } }))?;
                return Ok(Edit::Manual {
                    reason: "the file contains comments, which an automatic rewrite would lose"
                        .into(),
                    snippet,
                });
            }
            Err(error) => bail!("not valid JSON ({error}); fix or remove it first"),
        }
    };
    let Some(object) = root.as_object_mut() else {
        bail!("expected a JSON object at the top level");
    };
    match action {
        Action::Install => {
            let servers = object.entry(key).or_insert_with(|| json!({}));
            let Some(servers) = servers.as_object_mut() else {
                bail!("\"{key}\" is not an object");
            };
            if servers.get(SERVER_NAME).map(normalized) == Some(normalized(&desired)) {
                return Ok(Edit::Unchanged);
            }
            servers.insert(SERVER_NAME.into(), desired);
        }
        Action::Uninstall => {
            let Some(servers) = object.get_mut(key).and_then(Value::as_object_mut) else {
                return Ok(Edit::Unchanged);
            };
            if servers.remove(SERVER_NAME).is_none() {
                return Ok(Edit::Unchanged);
            }
        }
    }
    let indent = detect_indent(text);
    Ok(Edit::Write(to_json(&root, &indent)?))
}

fn to_json(value: &Value, indent: &str) -> Result<String> {
    use serde::Serialize;
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut serializer)?;
    out.push(b'\n');
    Ok(String::from_utf8(out)?)
}

fn detect_indent(text: &str) -> String {
    text.lines()
        .skip(1)
        .find_map(|line| {
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            (!indent.is_empty()).then_some(indent)
        })
        .unwrap_or_else(|| "  ".into())
}

/// `//` or `/*` outside strings. Trailing commas alone are rejected as invalid.
fn has_comments(text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut previous = '\0';
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if previous == '/' && (c == '/' || c == '*') {
            return true;
        }
        previous = c;
    }
    false
}

fn plan_codex(content: Option<&str>, entry: &Entry, action: Action) -> Result<Edit> {
    use toml_edit::{DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = content.unwrap_or("").parse().context("not valid TOML")?;
    match action {
        Action::Install => {
            if !doc.contains_key("mcp_servers") {
                let mut servers = Table::new();
                servers.set_implicit(true);
                doc.insert("mcp_servers", Item::Table(servers));
            }
            let servers = doc["mcp_servers"]
                .as_table_mut()
                .context("`mcp_servers` is not a table")?;
            let mut table = Table::new();
            table.insert("command", value(entry.command.as_str()));
            table.insert("args", value(toml_edit::Array::from_iter(["mcp"])));
            table.insert("tool_timeout_sec", value(LONG_TOOL_TIMEOUT_SECS));
            if servers
                .get(SERVER_NAME)
                .and_then(Item::as_table)
                .is_some_and(|existing| same_toml_table(existing, &table))
            {
                return Ok(Edit::Unchanged);
            }
            servers.insert(SERVER_NAME, Item::Table(table));
        }
        Action::Uninstall => {
            let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) else {
                return Ok(Edit::Unchanged);
            };
            if servers.remove(SERVER_NAME).is_none() {
                return Ok(Edit::Unchanged);
            }
            if servers.is_empty() {
                doc.remove("mcp_servers");
            }
        }
    }
    Ok(Edit::Write(doc.to_string()))
}

fn same_toml_table(existing: &toml_edit::Table, desired: &toml_edit::Table) -> bool {
    existing.len() == desired.len()
        && desired.iter().all(|(key, item)| {
            existing
                .get(key)
                .is_some_and(|other| other.to_string().trim() == item.to_string().trim())
        })
}

/// Vibe keeps `[[mcp_servers]]` entries identified by `name`.
fn plan_vibe(content: Option<&str>, entry: &Entry, action: Action) -> Result<Edit> {
    use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = content.unwrap_or("").parse().context("not valid TOML")?;
    let mut table = Table::new();
    table.insert("name", value(SERVER_NAME));
    table.insert("transport", value("stdio"));
    table.insert("command", value(entry.command.as_str()));
    table.insert("args", value(toml_edit::Array::from_iter(["mcp"])));
    table.insert("tool_timeout_sec", value(LONG_TOOL_TIMEOUT_SECS as f64));

    // A fresh config holds `mcp_servers = []`, an inline array.
    if doc
        .get("mcp_servers")
        .and_then(Item::as_array)
        .is_some_and(|array| array.is_empty())
    {
        doc.remove("mcp_servers");
    }
    if doc
        .get("mcp_servers")
        .is_some_and(|item| !item.is_array_of_tables())
    {
        let snippet = format!("[[mcp_servers]]\n{table}");
        return Ok(Edit::Manual {
            reason: "`mcp_servers` is written inline; add the entry by hand".into(),
            snippet,
        });
    }
    let is_ours = |t: &Table| t.get("name").and_then(Item::as_str) == Some(SERVER_NAME);

    match action {
        Action::Install => {
            if !doc.contains_key("mcp_servers") {
                doc.insert("mcp_servers", Item::ArrayOfTables(ArrayOfTables::new()));
            }
            let servers = doc["mcp_servers"].as_array_of_tables_mut().unwrap();
            if let Some(existing) = servers.iter_mut().find(|t| is_ours(t)) {
                if same_toml_table(existing, &table) {
                    return Ok(Edit::Unchanged);
                }
                *existing = table;
            } else {
                servers.push(table);
            }
        }
        Action::Uninstall => {
            let Some(servers) = doc
                .get_mut("mcp_servers")
                .and_then(Item::as_array_of_tables_mut)
            else {
                return Ok(Edit::Unchanged);
            };
            let before = servers.len();
            servers.retain(|t| !is_ours(t));
            if servers.len() == before {
                return Ok(Edit::Unchanged);
            }
        }
    }
    Ok(Edit::Write(doc.to_string()))
}

fn is_configured(agent: Agent, content: Option<&str>) -> bool {
    let Some(content) = content else {
        return false;
    };
    match agent.format() {
        Format::Json(key) => serde_json::from_str::<Value>(content)
            .ok()
            .is_some_and(|root| root.get(key).and_then(|s| s.get(SERVER_NAME)).is_some()),
        Format::CodexToml => content
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .is_some_and(|doc| {
                doc.get("mcp_servers")
                    .and_then(|s| s.get(SERVER_NAME))
                    .is_some()
            }),
        Format::VibeToml => content
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .and_then(|doc| {
                doc.get("mcp_servers")
                    .and_then(|s| s.as_array_of_tables())
                    .cloned()
            })
            .is_some_and(|servers| {
                servers
                    .iter()
                    .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(SERVER_NAME))
            }),
    }
}

fn write_atomically(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let permissions = std::fs::metadata(path).ok().map(|m| m.permissions());
    if path.exists() {
        let backup = path.with_file_name(format!(
            "{}.fil-backup",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        if !backup.exists() {
            std::fs::copy(path, &backup)?;
        }
    }
    let temporary = path.with_file_name(format!(
        ".{}.fil-tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temporary, content)?;
    if let Some(permissions) = permissions {
        std::fs::set_permissions(&temporary, permissions)?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

pub struct Options {
    pub dry_run: bool,
    /// Agent ids; empty means every detected agent.
    pub agents: Vec<String>,
}

fn selected(env: &Env, options: &Options) -> Result<Vec<Agent>> {
    if options.agents.is_empty() {
        return Ok(ALL
            .into_iter()
            .filter(|agent| agent.detected(env))
            .collect());
    }
    options
        .agents
        .iter()
        .map(|id| {
            ALL.into_iter()
                .find(|agent| agent.id() == id)
                .with_context(|| {
                    let ids: Vec<_> = ALL.iter().map(|a| a.id()).collect();
                    format!("unknown agent \"{id}\" (known: {})", ids.join(", "))
                })
        })
        .collect()
}

/// Claude Code rewrites ~/.claude.json constantly; its own CLI is the safe
/// way to change it when available.
fn claude_cli_commands(entry: &Entry, action: Action) -> Vec<Vec<String>> {
    let remove = ["claude", "mcp", "remove", "--scope", "user", SERVER_NAME];
    let mut commands = vec![remove.iter().map(|s| s.to_string()).collect()];
    if action == Action::Install {
        commands.push(
            [
                "claude",
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "stdio",
                SERVER_NAME,
                "--",
                &entry.command,
                "mcp",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        );
    }
    commands
}

pub fn run(action: Action, options: Options) -> Result<std::process::ExitCode> {
    let env = Env::current()?;
    let entry = Entry::current();
    let agents = selected(&env, &options)?;
    let verb = if action == Action::Install {
        "install"
    } else {
        "uninstall"
    };
    println!(
        "\n  \x1b[1mfil\x1b[32m.sh\x1b[0m mcp {verb}{}\n",
        if options.dry_run { " (dry run)" } else { "" }
    );
    if agents.is_empty() {
        println!("  No supported coding agent found. Use --agent <id> to pick one explicitly.\n");
        return Ok(std::process::ExitCode::SUCCESS);
    }

    let mut failed = false;
    for agent in agents {
        let path = agent.config_path(&env);
        let content = std::fs::read_to_string(&path).ok();
        let label = format!("{:<18}", agent.name());

        if agent == Agent::ClaudeCode && env.on_path("claude") {
            let desired = Some(normalized(&entry.json(agent)));
            let current = content
                .as_deref()
                .and_then(|c| serde_json::from_str::<Value>(c).ok())
                .and_then(|root| root.pointer("/mcpServers/fil").map(normalized));
            let unchanged = match action {
                Action::Install => current == desired,
                Action::Uninstall => current.is_none(),
            };
            if unchanged {
                println!("  \x1b[2m=\x1b[0m {label} already {}", done(action));
                continue;
            }
            let mut commands = claude_cli_commands(&entry, action);
            if current.is_none() {
                commands.remove(0);
            }
            for command in &commands {
                if options.dry_run {
                    println!(
                        "  \x1b[33m~\x1b[0m {label} would run: {}",
                        command.join(" ")
                    );
                    continue;
                }
                let output = Command::new(&command[0]).args(&command[1..]).output();
                if !output.as_ref().is_ok_and(|o| o.status.success()) {
                    failed = true;
                    println!("  \x1b[31m✗\x1b[0m {label} `{}` failed", command.join(" "));
                }
            }
            if !options.dry_run && !failed {
                println!(
                    "  \x1b[32m✓\x1b[0m {label} {} (via `claude mcp`)",
                    done(action)
                );
            }
            continue;
        }

        match plan_edit(agent, content.as_deref(), &entry, action) {
            Ok(Edit::Unchanged) => {
                println!("  \x1b[2m=\x1b[0m {label} already {}", done(action))
            }
            Ok(Edit::Write(new_content)) if options.dry_run => {
                println!("  \x1b[33m~\x1b[0m {label} would update {}", path.display());
                if std::env::var_os("FIL_MCP_SHOW_DIFF").is_some() {
                    println!("{new_content}");
                }
            }
            Ok(Edit::Write(new_content)) => match write_atomically(&path, &new_content) {
                Ok(()) => println!(
                    "  \x1b[32m✓\x1b[0m {label} {} → {}",
                    done(action),
                    path.display()
                ),
                Err(error) => {
                    failed = true;
                    println!("  \x1b[31m✗\x1b[0m {label} {}: {error:#}", path.display());
                }
            },
            Ok(Edit::Manual { reason, snippet }) => {
                println!("  \x1b[33m!\x1b[0m {label} {}: {reason}", path.display());
                if action == Action::Install {
                    for line in snippet.lines() {
                        println!("      {line}");
                    }
                }
            }
            Err(error) => {
                failed = true;
                println!("  \x1b[31m✗\x1b[0m {label} {}: {error:#}", path.display());
            }
        }
    }
    if action == Action::Install && !options.dry_run {
        println!("\n  Restart running agents to load it. Try it: \x1b[32m$\x1b[0m fil mcp test");
    }
    println!();
    Ok(if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

fn done(action: Action) -> &'static str {
    match action {
        Action::Install => "installed",
        Action::Uninstall => "removed",
    }
}

pub async fn status() -> Result<std::process::ExitCode> {
    use fil_protocol::agent::AgentReply;
    let env = Env::current()?;
    println!("\n  \x1b[1mfil\x1b[32m.sh\x1b[0m mcp status\n");
    match super::daemon_status().await {
        Ok(AgentReply::Status {
            hub_connected,
            session_id,
            sessions,
        }) => {
            let hub = if hub_connected {
                "\x1b[32mconnected\x1b[0m"
            } else {
                "\x1b[33mdisconnected\x1b[0m"
            };
            println!("  Daemon:   \x1b[32m✓\x1b[0m running, hub {hub}, {sessions} session(s)");
            match session_id {
                Some(id) => println!(
                    "  Terminal: \x1b[32m✓\x1b[0m Fil session {}",
                    id.chars().take(8).collect::<String>()
                ),
                None => println!(
                    "  Terminal: \x1b[2mnot a Fil session (notifications open the app)\x1b[0m"
                ),
            }
        }
        Ok(AgentReply::Error { message }) => println!("  Daemon:   \x1b[31m✗\x1b[0m {message}"),
        Ok(_) | Err(_) => println!(
            "  Daemon:   \x1b[31m✗\x1b[0m not reachable or too old for agent notifications"
        ),
    }
    println!();
    for agent in ALL {
        let path = agent.config_path(&env);
        let content = std::fs::read_to_string(&path).ok();
        let configured = is_configured(agent, content.as_deref());
        let state = match (agent.detected(&env), configured) {
            (_, true) => "\x1b[32m✓ configured\x1b[0m",
            (true, false) => "\x1b[33m· not configured\x1b[0m",
            (false, false) => "\x1b[2m- not found\x1b[0m",
        };
        println!("  {:<18} {:<28} {}", agent.name(), state, agent.id());
    }
    println!("\n  Configure with: \x1b[32m$\x1b[0m fil mcp install [--agent <id>] [--dry-run]\n");
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            command: "/usr/local/bin/fil".into(),
        }
    }

    fn written(edit: Edit) -> String {
        match edit {
            Edit::Write(content) => content,
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn json_install_preserves_other_servers_order_and_indentation() {
        let original = "{\n    \"zeta\": 1,\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\" }\n    },\n    \"alpha\": true\n}\n";
        let updated =
            written(plan_edit(Agent::Cursor, Some(original), &entry(), Action::Install).unwrap());
        let root: Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(root["mcpServers"]["other"]["command"], "x");
        assert_eq!(root["mcpServers"]["fil"]["command"], "/usr/local/bin/fil");
        assert_eq!(
            root["mcpServers"]["fil"]["env"]["FIL_ASK_WAIT"],
            SHORT_CLIENT_WAIT_SECS
        );
        let keys: Vec<_> = root.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["zeta", "mcpServers", "alpha"]);
        assert!(updated.contains("\n    \"zeta\""));

        // Idempotent, then reversible.
        assert_eq!(
            plan_edit(Agent::Cursor, Some(&updated), &entry(), Action::Install).unwrap(),
            Edit::Unchanged
        );
        let removed =
            written(plan_edit(Agent::Cursor, Some(&updated), &entry(), Action::Uninstall).unwrap());
        let root: Value = serde_json::from_str(&removed).unwrap();
        assert!(root["mcpServers"].get("fil").is_none());
        assert_eq!(
            plan_edit(Agent::Cursor, Some(&removed), &entry(), Action::Uninstall).unwrap(),
            Edit::Unchanged
        );
    }

    #[test]
    fn json_shapes_match_each_client() {
        let gemini: Value = serde_json::from_str(&written(
            plan_edit(Agent::Gemini, None, &entry(), Action::Install).unwrap(),
        ))
        .unwrap();
        assert_eq!(gemini["mcpServers"]["fil"]["timeout"], 660_000);
        assert!(gemini["mcpServers"]["fil"].get("env").is_none());

        let vscode: Value = serde_json::from_str(&written(
            plan_edit(Agent::VsCode, None, &entry(), Action::Install).unwrap(),
        ))
        .unwrap();
        assert_eq!(vscode["servers"]["fil"]["type"], "stdio");

        let opencode: Value = serde_json::from_str(&written(
            plan_edit(
                Agent::Opencode,
                Some("{\"$schema\":\"https://opencode.ai/config.json\"}"),
                &entry(),
                Action::Install,
            )
            .unwrap(),
        ))
        .unwrap();
        assert_eq!(
            opencode["mcp"]["fil"]["command"],
            json!(["/usr/local/bin/fil", "mcp"])
        );
        assert_eq!(opencode["mcp"]["fil"]["type"], "local");
        assert!(opencode["mcp"]["fil"]["environment"]["FIL_ASK_WAIT"].is_string());
    }

    #[test]
    fn commented_or_invalid_json_is_never_rewritten() {
        let jsonc = "{\n  // my servers\n  \"servers\": {}\n}";
        assert!(matches!(
            plan_edit(Agent::VsCode, Some(jsonc), &entry(), Action::Install).unwrap(),
            Edit::Manual { .. }
        ));
        assert!(plan_edit(Agent::Cursor, Some("{ nope"), &entry(), Action::Install).is_err());
        assert!(plan_edit(Agent::Cursor, Some("[]"), &entry(), Action::Install).is_err());
        assert!(!has_comments(r#"{"url": "https://example.com/*x*/"}"#));
    }

    #[test]
    fn codex_toml_is_edited_in_place() {
        let original = "# my config\nmodel = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let updated =
            written(plan_edit(Agent::Codex, Some(original), &entry(), Action::Install).unwrap());
        assert!(updated.starts_with("# my config\nmodel = \"gpt-5\""));
        assert!(updated.contains("[mcp_servers.other]"));
        assert!(updated.contains("[mcp_servers.fil]"));
        assert!(updated.contains("tool_timeout_sec = 660"));
        assert_eq!(
            plan_edit(Agent::Codex, Some(&updated), &entry(), Action::Install).unwrap(),
            Edit::Unchanged
        );
        let removed =
            written(plan_edit(Agent::Codex, Some(&updated), &entry(), Action::Uninstall).unwrap());
        assert!(!removed.contains("mcp_servers.fil"));
        assert!(removed.contains("[mcp_servers.other]"));

        let fresh = written(plan_edit(Agent::Codex, None, &entry(), Action::Install).unwrap());
        let removed =
            written(plan_edit(Agent::Codex, Some(&fresh), &entry(), Action::Uninstall).unwrap());
        assert_eq!(removed.trim(), "");
    }

    #[test]
    fn vibe_array_of_tables_is_edited_by_name() {
        let fresh = "active_model = \"devstral\"\nmcp_servers = []\n";
        let updated =
            written(plan_edit(Agent::Vibe, Some(fresh), &entry(), Action::Install).unwrap());
        assert!(updated.contains("[[mcp_servers]]"));
        assert!(updated.contains("name = \"fil\""));
        assert!(is_configured(Agent::Vibe, Some(&updated)));

        let existing = "[[mcp_servers]]\nname = \"docs\"\ntransport = \"streamable-http\"\nurl = \"https://x\"\n";
        let updated =
            written(plan_edit(Agent::Vibe, Some(existing), &entry(), Action::Install).unwrap());
        assert_eq!(updated.matches("[[mcp_servers]]").count(), 2);
        assert_eq!(
            plan_edit(Agent::Vibe, Some(&updated), &entry(), Action::Install).unwrap(),
            Edit::Unchanged
        );
        let removed =
            written(plan_edit(Agent::Vibe, Some(&updated), &entry(), Action::Uninstall).unwrap());
        assert!(removed.contains("name = \"docs\""));
        assert!(!removed.contains("name = \"fil\""));
    }

    #[test]
    fn claude_code_cli_entry_matches_what_the_cli_writes() {
        let written_by_cli = json!({
            "type": "stdio",
            "command": "/usr/local/bin/fil",
            "args": ["mcp"],
            "env": {},
        });
        assert_eq!(
            normalized(&written_by_cli),
            normalized(&entry().json(Agent::ClaudeCode))
        );
        let commands = claude_cli_commands(&entry(), Action::Install);
        assert_eq!(commands[1].last().unwrap(), "mcp");
        assert!(commands[1].contains(&"--scope".to_string()));
    }

    #[test]
    fn atomic_write_keeps_a_single_backup() {
        let dir = std::env::temp_dir().join(format!("fil-install-{}", uuid::Uuid::new_v4()));
        let path = dir.join("nested/mcp.json");
        write_atomically(&path, "{}\n").unwrap();
        write_atomically(&path, "{\"a\":1}\n").unwrap();
        write_atomically(&path, "{\"a\":2}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("nested/mcp.json.fil-backup")).unwrap(),
            "{}\n"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
