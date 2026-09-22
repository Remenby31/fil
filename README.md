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
- **Smart notifications** — build finished, prompt waiting, errors
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
docker run -d -p 3100:3100 \
  -e JWT_SECRET=your-secret \
  -e GITHUB_CLIENT_ID=xxx \
  -e GITHUB_CLIENT_SECRET=xxx \
  -v fil-data:/data \
  fil/hub
```

ActivityKit updates continue locally without APNs. To keep Live Activities current while the app is suspended, mount an Apple APNs `.p8` provider key as a Docker secret and configure:

```bash
-e APNS_TEAM_ID=3SNT64YKAS \
-e APNS_KEY_ID=YOUR_KEY_ID \
-e APNS_TOPIC=sh.fil.app \
-e APNS_PRIVATE_KEY_PATH=/run/secrets/apns_key \
-v /secure/AuthKey.p8:/run/secrets/apns_key:ro
```

Never commit or bake the `.p8` key into the image. `APNS_PRIVATE_KEY` is also supported for secret managers that inject multiline environment values.

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
