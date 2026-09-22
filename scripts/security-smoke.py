#!/usr/bin/env python3
"""Regression checks on a disposable hub, using only synthetic credentials."""

import argparse
import importlib.util
from pathlib import Path
import socket
import tempfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("smoke", ROOT / "scripts/smoke-test.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def device_revocation(s):
    sibling_id = s.http("POST", "/devices", s.owner, {"name": "sibling"}, expected=201)["id"]
    sibling_sid = "security-sibling"
    control = smoke.WebSocket(s.port, f"/ws?device_id={sibling_id}", s.owner)
    s.sockets.append(control)
    control.send(smoke.heartbeat(sibling_id, (sibling_sid,)))
    control.send(b"barrier", opcode=9)
    smoke.check(control.receive() == (10, b"barrier"), "sibling control barrier")
    sibling_daemon = smoke.WebSocket(s.port, f"/ws/data/{sibling_sid}?role=daemon", s.owner)
    sibling_client = smoke.WebSocket(s.port, f"/ws/data/{sibling_sid}?role=client", s.owner)
    s.sockets.extend((sibling_daemon, sibling_client))
    smoke.check(len(sibling_client.binary()) == 8, "missing sibling cursor")
    daemon = smoke.WebSocket(s.port, f"/ws/data/{s.sid}?role=daemon", s.owner)
    s.sockets.append(daemon)
    client = smoke.WebSocket(s.port, f"/ws/data/{s.sid}?role=client", s.owner)
    s.sockets.append(client)
    smoke.check(len(client.binary()) == 8, "missing initial cursor")
    marker = b"SYNTHETIC_SECURITY_REPLAY\n"
    daemon.send(marker)
    smoke.check(client.binary() == marker, "initial replay missing")
    pending_ticket = s.ticket()

    # Deleting another account's device must not revoke the real owner.
    s.http("DELETE", f"/devices/{s.device}", s.other, expected=404)
    s.ticket()
    s.http("DELETE", f"/devices/{s.device}", s.owner, expected=204)
    devices = s.http("GET", "/sessions", s.owner)
    smoke.check(not any(d["device_id"] == s.device for d in devices),
                "deleted device is still advertised with live sessions")
    for role in ("ticket", "daemon-ticket"):
        s.http("POST", f"/sessions/{s.sid}/{role}", s.owner, expected=404)
    for role in ("client", "daemon"):
        smoke.WebSocket(s.port, f"/ws/data/{s.sid}?role={role}", s.owner, expected_status=404)
    smoke.WebSocket(s.port, f"/ws?device_id={s.device}", s.owner, expected_status=401)
    s.client(ticket=pending_ticket, success=False, diagnostic="attach rejected")
    # Existing peers must close too, not merely be hidden from /devices.
    for peer in (client, daemon):
        closed = False
        try:
            for _ in range(4):
                opcode, _ = peer.receive()
                if opcode == 8:
                    closed = True
                    break
        except (AssertionError, ConnectionError):
            closed = True
        except socket.timeout:
            pass
        smoke.check(closed, "deleted device retained an open terminal data socket")
    sibling_daemon.send(b"UNAFFECTED_SIBLING\n")
    smoke.check(sibling_client.binary() == b"UNAFFECTED_SIBLING\n", "deletion interrupted another device")
    s.http("POST", f"/sessions/{sibling_sid}/ticket", s.owner)
    smoke.passed("device deletion revokes discovery, upgrades, live data sockets and pending tickets")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hub-bin", type=Path, default=ROOT / "target/debug/fil-hub")
    parser.add_argument("--client-bin", type=Path, default=ROOT / "target/debug/fil-testclient")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="fil-security-") as directory:
        s = smoke.Smoke(args, directory)
        try:
            s.start()
            events = smoke.WebSocket(s.port, "/ws/client", s.owner)
            s.sockets.append(events)
            smoke.check(events.receive()[0] == 1, "header-authenticated event feed unavailable")
            smoke.WebSocket(s.port, "/ws/client?token=" + s.owner, expected_status=401)
            smoke.passed("event feed accepts Authorization and rejects query credentials by default")
            device_revocation(s)
        finally:
            s.close()
    smoke.passed("security fixtures cleaned up; no user terminals or credentials used")


if __name__ == "__main__":
    main()
