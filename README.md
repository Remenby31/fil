# fil.

**Your terminals, everywhere.**

Access your Mac terminal sessions from your iPhone. Fil is the invisible thread that connects your sessions across devices.

## How it works

```
Mac (Ghostty/kitty/any terminal)
  └── fil (PTY proxy) ──── WebSocket ────► Hub (VPS/Docker) ◄──── iOS App
                                            session registry
                                            TLS-protected routing
```

1. **Install on your Mac**: `brew install fil && fil setup`
2. **Sign in**: OAuth (GitHub / Apple) — zero config
3. **Open on iPhone**: your sessions are already there

## Architecture

| Component | Language | Role |
|-----------|----------|------|
| `fil` daemon | Rust | PTY proxy, launched by your terminal instead of bash |
| `fil-hub` | Rust | Central server — session registry, auth, WebSocket routing |
| iOS app | Swift (SwiftUI + TCA) | Native terminal client with SwiftTerm |
| `fil-protocol` | Rust + Protobuf | Shared messages, certificate pinning, experimental Noise primitives |

## Features

- **Real-time session access** from iPhone
- **Multi-machine** — all your Macs, one hub
- **Agent notifications** — Claude Code, Codex & co. ping your phone when done, and ask you questions you answer from the notification
- **Dynamic Island** — long-running processes on your lock screen
- **Encrypted in transit** — authenticated QUIC with certificate pinning, with HTTPS/WebSocket fallback. The trusted hub relays plaintext and keeps a bounded in-memory replay buffer; this is not end-to-end encryption.
- **Self-hostable** — one Docker command, your data stays yours

## Quick start

### Daemon (Mac)

```bash
brew install fil
fil setup
# Restart your terminal — fil is now active
```

### Hub (self-hosted)

```bash
docker run -d -p 127.0.0.1:3100:3100 -p 16433:16433/udp \
  -e JWT_SECRET -e PUBLIC_URL -e FIL_TRUSTED_PROXY_IPS \
  -e GITHUB_CLIENT_ID=xxx \
  -e GITHUB_CLIENT_SECRET=xxx \
  -v fil-data:/data \
  fil/hub
```

Set `JWT_SECRET` to a persistent random secret of at least 32 bytes,
`PUBLIC_URL` to your HTTPS origin, and `FIL_TRUSTED_PROXY_IPS` to the exact
immediate proxy address as seen inside the container. Terminate HTTPS at that
proxy and keep the HTTP origin private. See [security and migration settings](docs/security-hardening.md).

ActivityKit updates continue locally without APNs. To keep Live Activities current while the app is suspended, mount an Apple APNs `.p8` provider key as a Docker secret and configure:

```bash
-e APNS_TEAM_ID=3SNT64YKAS \
-e APNS_KEY_ID=YOUR_KEY_ID \
-e APNS_TOPIC=sh.fil.app \
-e APNS_PRIVATE_KEY_PATH=/run/secrets/apns_key \
-v /secure/AuthKey.p8:/run/secrets/apns_key:ro
```

Never commit or bake the `.p8` key into the image. `APNS_PRIVATE_KEY` is also supported for secret managers that inject multiline environment values.

The same key sends agent notifications (below); without it the hub reports
`agent_notifications: false` in `/health` and agents get a clear error.

## Agent notifications

`fil mcp` is a small local MCP server that lets coding agents reach your
phone. It exposes three tools:

- **`notify`** — a push notification; tapping it opens the agent's terminal.
- **`ask`** — a question with 2-4 options, answered from the notification
  buttons or in the app. The answer comes back as the tool result.
- **`get_answer`** — collects an answer for clients that cut tool calls short.

```bash
fil mcp install --dry-run   # show what would change
fil mcp install             # register with every agent found on this Mac
fil mcp test                # send yourself a test notification
fil mcp status              # daemon reachability + per-agent state
fil mcp uninstall
```

`install` detects Claude Code, Codex CLI, Cursor, Gemini CLI, opencode,
VS Code (Copilot), Claude Desktop, Windsurf and Mistral Vibe (`--agent <id>`
picks some). It only touches the `fil` entry, keeps key order, leaves a
`.fil-backup` next to the first file it edits, is idempotent, and prints a
snippet instead of rewriting a file that contains comments. Claude Code is
configured through `claude mcp add --scope user`.

No configuration is needed per terminal: the fil proxy exports
`FIL_SESSION_ID`, and the daemon also matches the agent's process tree to its
terminal. Agents started outside a Fil terminal still notify; the
notification then opens the app.

Questions stay answerable for 10 minutes (`timeout_seconds`, up to 1 hour).
Codex, Gemini and Vibe get a longer tool timeout from `install`; Cursor,
Claude Desktop, VS Code, Windsurf and opencode stop tool calls after about a
minute, so there `ask` returns "pending" after 45 seconds (`FIL_ASK_WAIT`) and
the agent follows up with `get_answer`. Notification text goes through your
hub and Apple's push service like Live Activity updates do. The hub allows
20 notifications per minute per account.

### Development

```bash
# Backend (Rust)
cargo build --release

# Hub
cargo run -p fil-hub

# Daemon
cargo run -p fil-daemon

# Landing page (Astro)
cd web && npm install && npm run dev

# iOS app
cd ios && xcodegen generate && open Fil.xcodeproj
```

## Project structure

```
├── crates/
│   ├── fil-daemon/     # PTY proxy binary
│   ├── fil-hub/        # Central server
│   └── fil-protocol/   # Protobuf + transport verification
├── ios/                # SwiftUI iOS app
│   ├── Fil/            # App source
│   ├── FilWidgets/     # Widget extension
│   └── project.yml     # XcodeGen config
├── web/                # Landing page (Astro)
├── Dockerfile          # Hub container
└── docker-compose.yml
```

## License

MIT
