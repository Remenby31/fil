# Security hardening

## Scope and acceptance criteria

Preserve the audited application baseline and its current user experience. Ship
changes in an isolated worktree, review the PR, merge, then deploy the merged
revision with a rollback path. Existing user shells must remain untouched.

- Never log terminal input or credential values; protect local logs and IPC.
- Deleting a device must revoke its existing control/data connections, buffered
  output and outstanding attach tickets. Late heartbeats/upgrades must not
  resurrect it. Other devices and their live terminals must remain usable.
- Repair existing private-key permissions without silently replacing the key;
  newly created keys/configuration must be private from their first write.
- Update vulnerable active dependencies and retain locked, reproducible builds.
- Move iOS event WebSocket credentials from URLs into Authorization headers.
  A bounded, explicitly configurable migration window may support old installed
  clients; do not require everyone to sign in again during deployment.
- Require HTTPS for remote credentials, with loopback-only local development.
  Trust forwarded headers only from explicitly configured proxy addresses;
  keep the production HTTP origin off the LAN/public interfaces.
- Bound authentication/control-plane abuse without throttling terminal bytes,
  normal heartbeats, or bursts of reconnecting legitimate sessions.
- Verify Rust and iOS tests, QUIC/WSS isolation, device revocation and actual
  PTY round trips. Do not claim a native release is available before verifying it.

## Non-goals

No changes to the terminal UI, shortcuts, WSS-first startup or replay protocol.
No E2E claim: the trusted hub still decrypts terminal traffic. No global JWT
rotation or unrelated service restarts. Existing leaked credentials cannot be
made secret again by a code change; credential rotation remains a separate,
explicitly communicated recovery action.

## Deployment settings

`docker compose` now publishes HTTP on `127.0.0.1` only. A local HTTPS reverse
proxy/tunnel must supply `X-Forwarded-Proto`. Configure `FIL_TRUSTED_PROXY_IPS`
with the exact immediate proxy IP as seen inside the container (for a host
tunnel, usually the Docker bridge gateway); do not use a broad trusted subnet.
`CF-Connecting-IP` is used for rate limits only from these trusted peers.
Keep `/health` accessible from container loopback for the Docker healthcheck.

The updated iOS event feed sends `Authorization: Bearer ...`. For a staged native
rollout, set `FIL_LEGACY_WS_TOKEN_UNTIL` to an absolute Unix timestamp no later
than 14 days after deployment. Default `0` disables query credentials. Existing
legacy sockets are rechecked every 30 seconds. Do not extend this window on
every restart; announce the required app update before its fixed end date.

Keep `JWT_SECRET` unchanged across deployment to preserve login. New native
clients and daemon/proxy builds reject remote HTTP; local loopback tests remain
supported. The rate limiter applies to requests/upgrades, never terminal frames.
Native HTTP loopback is available only in Debug Simulator builds, not releases.
Bare development hubs reject non-loopback peers even if forwarding headers are
forged. Container deployments must use the explicitly configured HTTPS proxy.

On a binary-only Mac daemon upgrade, startup repairs the old
`/tmp/fil-daemon.log` permissions before opening hub connections. Re-pairing is
not required. Operators can also move the existing LaunchAgent's stdout/stderr
destinations into the private Fil configuration directory when restarting the
daemon; retain the label, executable and account pairing.

Verification commands: `cargo test --workspace --locked`, strict Clippy,
`python3 scripts/smoke-test.py --ws-data`, `python3 scripts/security-smoke.py`,
`python3 scripts/audit-dependencies.py`, and `npm audit` / `npm run build` in
`web`. The security smoke suite uses a disposable hub, proves revocation of live
sockets and outstanding tickets, and checks another device stays usable.
