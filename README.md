# fil.

**Your terminals, everywhere.**

Access your Mac terminal sessions from your iPhone. Fil is the invisible thread that connects your sessions across devices.

## How it works

```
Mac (Ghostty/kitty/any terminal)
  └── fil (PTY proxy) ──── WebSocket ────► Hub (VPS/Docker) ◄──── iOS App
                                            session registry
                                            E2E encrypted routing
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
| `fil-protocol` | Rust + Protobuf | Shared protocol definitions + E2E crypto |

## Features

- **Real-time session access** from iPhone
- **Multi-machine** — all your Macs, one hub
- **Smart notifications** — build finished, prompt waiting, errors
- **Dynamic Island** — long-running processes on your lock screen
- **E2E encrypted** — Noise Protocol (XX), ChaCha20-Poly1305
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
docker run -d -p 3100:3100 \
  -e JWT_SECRET=your-secret \
  -e GITHUB_CLIENT_ID=xxx \
  -e GITHUB_CLIENT_SECRET=xxx \
  -v fil-data:/data \
  fil/hub
```

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
│   └── fil-protocol/   # Protobuf + E2E crypto
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
