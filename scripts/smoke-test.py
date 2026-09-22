#!/usr/bin/env python3
"""Disposable local Fil protocol verification; never reads user credentials."""

import argparse
import base64
import hashlib
import hmac
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parent.parent


def clean_env():
    # Do not inherit production tokens, data paths, proxies, or APNs settings.
    allowed = ("PATH", "HOME", "TMPDIR", "SYSTEMROOT", "CARGO_HOME", "RUSTUP_HOME")
    return {key: os.environ[key] for key in allowed if key in os.environ}


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def passed(message):
    print(f"PASS {message}", flush=True)


def jwt(subject, secret, lifetime=300):
    def encode(value):
        return base64.urlsafe_b64encode(value).rstrip(b"=")
    now = int(time.time())
    header = encode(b'{"alg":"HS256","typ":"JWT"}')
    payload = encode(json.dumps({"sub": subject, "iat": now, "exp": now + lifetime}).encode())
    signing = header + b"." + payload
    return (signing + b"." + encode(hmac.new(secret, signing, hashlib.sha256).digest())).decode()


def varint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def field(number, value):
    """Tiny fixture encoder for the checked-in fil.proto, not a mock registry."""
    if isinstance(value, int):
        return varint(number << 3) + varint(value)
    if isinstance(value, str):
        value = value.encode()
    return varint((number << 3) | 2) + varint(len(value)) + value


def heartbeat(device, sessions):
    now = int(time.time())
    payload = field(1, device)
    for sid in sessions:
        info = (field(1, sid) + field(2, "smoke-echo") + field(3, "/smoke")
                + field(4, now) + field(5, 80) + field(6, 24) + field(7, "synthetic"))
        payload += field(2, info)
    return field(6, payload + field(3, now))


class WebSocket:
    """Minimal bounded RFC6455 client for a disposable *loopback-only* hub."""
    def __init__(self, port, path, token=None, expected_status=101):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=3)
        self.file = self.sock.makefile("rb")
        self.lock = threading.Lock()
        key = base64.b64encode(secrets.token_bytes(16)).decode()
        headers = [f"GET {path} HTTP/1.1", f"Host: 127.0.0.1:{port}",
                   "Upgrade: websocket", "Connection: Upgrade",
                   "Sec-WebSocket-Version: 13", f"Sec-WebSocket-Key: {key}"]
        if token:
            headers.append(f"Authorization: Bearer {token}")
        try:
            self.sock.sendall(("\r\n".join(headers) + "\r\n\r\n").encode())
            status = self.file.readline(4096).split()
            check(len(status) >= 2, "invalid WebSocket HTTP response")
            status = int(status[1])
            response_headers = {}
            for _ in range(100):
                line = self.file.readline(8192)
                if line == b"\r\n":
                    break
                check(b":" in line, "invalid WebSocket upgrade header")
                name, value = line.split(b":", 1)
                response_headers[name.lower()] = value.strip()
            else:
                raise AssertionError("too many WebSocket headers")
            check(status == expected_status, f"WebSocket expected HTTP {expected_status}, got {status}")
            if status == 101:
                expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
                check(response_headers.get(b"sec-websocket-accept") == expected, "invalid WebSocket accept proof")
            else:
                self.close()
        except BaseException:
            self.close()
            raise

    def send(self, data, opcode=2):
        mask = secrets.token_bytes(4)
        length = len(data)
        header = bytes([0x80 | opcode])
        if length < 126:
            header += bytes([0x80 | length])
        elif length < 65536:
            header += b"\xfe" + struct.pack("!H", length)
        else:
            header += b"\xff" + struct.pack("!Q", length)
        frame = header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        with self.lock:
            self.sock.sendall(frame)

    def exact(self, length):
        data = self.file.read(length)
        check(len(data) == length, "WebSocket closed before expected frame")
        return data

    def receive(self):
        first, second = self.exact(2)
        check(first & 0x80 and not (first & 0x70), "unexpected fragmented/extended WebSocket frame")
        check(not second & 0x80, "server frame must not be masked")
        length = second & 127
        if length == 126:
            length = struct.unpack("!H", self.exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self.exact(8))[0]
        check(length <= 2 * 1024 * 1024, "oversized WebSocket test frame")
        return first & 15, self.exact(length)

    def binary(self):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            opcode, data = self.receive()
            if opcode == 9:
                self.send(data, opcode=10)
            elif opcode == 2:
                return data
            elif opcode != 10:
                raise AssertionError("WebSocket closed instead of returning binary data")
        raise AssertionError("WebSocket binary-read deadline exceeded")

    def close(self):
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.file.close()
        self.sock.close()


class Child:
    def __init__(self, command, env):
        self.lines = []
        self.process = subprocess.Popen(command, env=env, cwd=ROOT, stdin=subprocess.DEVNULL,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        for line in self.process.stdout:
            self.lines.append(line)

    def output(self):
        return "".join(self.lines)

    def wait_line(self, marker, minimum=1, timeout=4):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.output().count(marker) >= minimum:
                return
            check(self.process.poll() is None, "synthetic daemon exited unexpectedly")
            time.sleep(0.02)
        raise AssertionError(f"synthetic daemon did not report {marker}")

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
        self.reader.join(timeout=1)
        self.process.stdout.close()


def port_for(kind):
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Smoke:
    def __init__(self, args, directory):
        self.args = args
        self.directory = Path(directory)
        self.port = port_for(socket.SOCK_STREAM)
        self.quic_port = port_for(socket.SOCK_DGRAM)
        self.secret = secrets.token_hex(32)
        self.owner = jwt("smoke-owner", self.secret.encode())
        self.other = jwt("smoke-other", self.secret.encode())
        self.credentials = [self.secret, self.owner, self.other]
        self.children = []
        self.sockets = []
        self.stop_heartbeat = threading.Event()
        self.heartbeat_thread = None
        self.heartbeat_error = None
        self.sid = "smoke-" + secrets.token_hex(8)
        self.sid2 = "smoke-" + secrets.token_hex(8)

    def redact(self, text):
        for value in self.credentials:
            text = text.replace(value, "<REDACTED>")
        return text

    def http(self, method, path, token=None, body=None, expected=200):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=3)
        headers = {"Content-Type": "application/json"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        try:
            connection.request(method, path, body=None if body is None else json.dumps(body), headers=headers)
            response = connection.getresponse()
            raw = response.read(1024 * 1024)
            check(response.status == expected, f"{method} {path}: expected HTTP {expected}, got {response.status}")
            return json.loads(raw) if raw else None
        finally:
            connection.close()

    def ticket(self, role="client", sid=None):
        path = "daemon-ticket" if role == "daemon" else "ticket"
        value = self.http("POST", f"/sessions/{sid or self.sid}/{path}", self.owner)
        check(isinstance(value.get("ticket"), str) and value["ticket"], "missing attach ticket")
        check(isinstance(value.get("quic_certificate"), str), "missing certificate pin")
        check(base64.b64decode(value["quic_certificate"], validate=True), "empty certificate pin")
        self.credentials.append(value["ticket"])
        return value

    def env(self, ticket):
        env = clean_env()
        env.update(FIL_ATTACH_TICKET=ticket["ticket"], FIL_QUIC_CERTIFICATE=ticket["quic_certificate"])
        return env

    def command(self, sid=None, read_secs=0.35):
        return [str(self.args.client_bin), "--session", sid or self.sid,
                "--addr", f"127.0.0.1:{self.quic_port}", "--server-name", "localhost",
                "--read-secs", str(read_secs), "--connect-timeout-secs", "1", "--io-timeout-secs", "1"]

    def client(self, extra=(), ticket=None, sid=None, success=True, diagnostic=None, read_secs=0.35):
        ticket = ticket or self.ticket(sid=sid)
        result = subprocess.run(self.command(sid, read_secs) + list(extra), env=self.env(ticket),
                                capture_output=True, text=True, timeout=6)
        output = result.stdout + result.stderr
        for credential in self.credentials:
            check(credential not in output, "credential leaked in test client output")
        check((result.returncode == 0) == success,
              f"test client unexpected exit={result.returncode}: {self.redact(output)}")
        if diagnostic:
            check(diagnostic in output, f"expected diagnostic {diagnostic}: {self.redact(output)}")
        summary = re.search(r"SUMMARY received_bytes=(\d+) start_offset=(\d+|none) cursor=(\d+|none) expect=(\w+)", output)
        check(summary is not None, "test client omitted SUMMARY")
        received, start, cursor, expectation = summary.groups()
        return {"received": int(received), "start": None if start == "none" else int(start),
                "cursor": None if cursor == "none" else int(cursor), "expect": expectation, "output": output}

    def start(self):
        env = clean_env()
        database = self.directory / "hub.sqlite"
        env.update(PORT=str(self.port), QUIC_PORT=str(self.quic_port), JWT_SECRET=self.secret,
                   DATABASE_URL=f"sqlite:{database}?mode=rwc", DATA_DIR=str(self.directory),
                   PUBLIC_URL=f"http://127.0.0.1:{self.port}", FIL_REQUIRE_ATTACH_TICKET="true", FIL_LOG="warn")
        hub = Child([str(self.args.hub_bin)], env)
        self.children.append(hub)
        deadline = time.monotonic() + 15
        while True:
            check(hub.process.poll() is None, "scratch hub exited during startup")
            try:
                self.http("GET", "/health")
                break
            except (OSError, http.client.HTTPException):
                check(time.monotonic() < deadline, "scratch hub health deadline exceeded")
                time.sleep(0.05)
        # Fixture accounts only; device/session verification goes through public APIs.
        with sqlite3.connect(database, timeout=3) as db:
            for name in ("smoke-owner", "smoke-other"):
                db.execute("INSERT INTO users (id,provider,provider_id) VALUES (?,?,?)", (name, "github", name))
        self.device = self.http("POST", "/devices", self.owner,
                                {"name": "smoke-local", "os": "synthetic", "hostname": "loopback"}, expected=201)["id"]
        for role in ("ticket", "daemon-ticket"):
            self.http("POST", f"/sessions/{self.sid}/{role}", self.owner, expected=404)
        passed("session tickets unavailable before authenticated daemon heartbeat")
        for token in (None, "invalid-local-smoke-jwt", self.other):
            WebSocket(self.port, f"/ws?device_id={self.device}", token, expected_status=401)
        passed("daemon control WebSocket rejects missing, invalid, and foreign-owner JWTs")
        control = WebSocket(self.port, f"/ws?device_id={self.device}", self.owner)
        self.sockets.append(control)
        control.send(heartbeat(self.device, (self.sid, self.sid2)))
        control.send(b"smoke-control", opcode=9)
        opcode, data = control.receive()
        check(opcode == 10 and data == b"smoke-control", "control WebSocket ping/pong failed")

        def pump():
            while not self.stop_heartbeat.wait(2):
                try:
                    control.send(heartbeat(self.device, (self.sid, self.sid2)))
                except OSError as error:
                    self.heartbeat_error = error
                    return
        self.heartbeat_thread = threading.Thread(target=pump, daemon=True)
        self.heartbeat_thread.start()
        devices = self.http("GET", "/sessions", self.owner)
        device = next(d for d in devices if d["device_id"] == self.device)
        check(device["connected"] and {s["session_id"] for s in device["sessions"]} == {self.sid, self.sid2},
              "heartbeat sessions missing from authenticated registry")
        passed("authenticated /ws heartbeat registers sessions; bearer remains in header")

    def auth_checks(self):
        bad_signature = jwt("smoke-owner", b"wrong-local-secret")
        expired = jwt("smoke-owner", self.secret.encode(), lifetime=-3600)
        deleted = jwt("smoke-deleted", self.secret.encode())
        self.credentials.extend([bad_signature, expired, deleted])
        for route in ("ticket", "daemon-ticket"):
            path = f"/sessions/{self.sid}/{route}"
            for token in (None, "invalid-local-smoke-jwt", bad_signature, expired, deleted):
                self.http("POST", path, token, expected=401)
            self.http("POST", path, self.other, expected=404)
            self.http("POST", f"/sessions/nonexistent/{route}", self.owner, expected=404)
        passed("both ticket endpoints reject unauthenticated, forged, expired, deleted, and foreign accounts")
        wrong_pin = self.ticket()
        wrong_pin["quic_certificate"] = base64.b64encode(b"not-the-hub-certificate").decode()
        for extra in ((), ("--insecure",)):
            failure = self.client(extra, wrong_pin, success=False, diagnostic="QUIC connect failed")
            check(failure["start"] is None and failure["received"] == 0, "wrong certificate exposed session bytes")
        passed("wrong pin is rejected, including with --insecure")
        invalid = self.ticket()
        invalid["ticket"] = "00" * 32
        self.credentials.append(invalid["ticket"])
        self.client(ticket=invalid, success=False, diagnostic="attach rejected")
        self.client(("--stream-type", "0x11"), invalid, success=False, diagnostic="daemon stream closed or rejected")
        self.client(ticket=self.ticket("daemon"), success=False, diagnostic="attach rejected")
        self.client(("--stream-type", "0x11"), self.ticket(), success=False, diagnostic="daemon stream closed or rejected")
        self.client(ticket=self.ticket(sid=self.sid2), success=False, diagnostic="attach rejected")
        passed("QUIC rejects invented, wrong-role, and wrong-session tickets")
        for stream in ("0x01", "0x02"):
            self.client(("--stream-type", stream, "--insecure"), success=False)
        passed("explicit legacy negative tests are rejected by the secured hub")

    def roundtrip(self):
        initial = "SMOKE_INITIAL\n"
        marker = "SMOKE_ROUNDTRIP_é_✓\n"
        daemon_ticket = self.ticket("daemon")
        daemon = Child(self.command(read_secs=45) + ["--stream-type", "0x11", "--send", initial,
                       "--send-after", "0", "--expect", marker], self.env(daemon_ticket))
        self.children.append(daemon)
        daemon.wait_line("SYNTHETIC_DAEMON_READY")
        cold = self.client(("--expect", initial))
        check(cold["received"] == len(initial.encode()) and cold["start"] == 0
              and cold["cursor"] == len(initial.encode()), "v3 cursor counted header or end offset instead of payload")
        daemon.wait_line("DAEMON_DETACHED")
        passed("pinned 0x11 daemon output reaches 0x13 client; cold cursor excludes 8-byte header")

        single_use = self.ticket()
        live = self.client(("--resume-from", str(cold["cursor"]), "--resize", "91x31",
                            "--send", marker, "--send-after", "0.05", "--expect", marker), single_use)
        expected_bytes = len(("SMOKE_DETACHED\nSMOKE_RESIZE=91x31\n" + marker).encode())
        check(live["start"] == cold["cursor"] and live["received"] == expected_bytes
              and live["cursor"] == cold["cursor"] + expected_bytes, "live input/resize framing or resume accounting is wrong")
        daemon.wait_line("DAEMON_INPUT")
        daemon.wait_line("DAEMON_RESIZE cols=91 rows=31")
        daemon.wait_line("DAEMON_DETACHED", minimum=2)
        passed("framed Unicode input echoes byte-exactly, resize reaches daemon, explicit detach is observed")
        self.client(ticket=single_use, success=False, diagnostic="attach rejected")
        self.client(("--stream-type", "0x11"), daemon_ticket, success=False, diagnostic="daemon stream closed or rejected")
        passed("client and daemon tickets are single-use")

        delta = self.client(("--resume-from", str(live["cursor"]), "--expect", "SMOKE_DETACHED\n"))
        check(delta["start"] == live["cursor"] and delta["received"] == 15
              and delta["cursor"] == live["cursor"] + 15, "delta replay duplicated consumed bytes")
        # SMOKE_DETACHED is 15 bytes including newline; no previous input may replay.
        passed("reattach returns only the unseen detach delta and advances the consumed cursor")
        missing = self.client(("--expect", "SMOKE_THIS_MUST_NEVER_APPEAR"), success=False,
                              diagnostic="--expect assertion failed")
        check(missing["expect"] == "missing" and missing["received"] > 0, "missing expectation was not enforced")
        passed("--expect exits nonzero when output is present but the requested marker is absent")
        if sys.platform == "darwin":
            self.client(("--interface", "lo0", "--expect", initial))
            passed("macOS per-socket interface binding works on lo0 (no routing changes)")
        check(self.heartbeat_error is None, "control heartbeat stopped unexpectedly")

        # Initial output and a later framed-input echo form one overlapping match.
        split_daemon = Child(self.command(self.sid2, read_secs=10) + ["--stream-type", "0x11",
                             "--send", "ababa", "--send-after", "0", "--expect", "café ✓"],
                             self.env(self.ticket("daemon", self.sid2)))
        self.children.append(split_daemon)
        split_daemon.wait_line("SYNTHETIC_DAEMON_READY")
        split = self.client(("--send", "café ✓", "--send-after", "0.1", "--expect", "abacafé ✓"), sid=self.sid2)
        check(split["received"] == 14 and split["cursor"] == 14, "split Unicode expectation or byte counting failed")
        passed("--expect matches overlapping UTF-8 text across catch-up and later live output")
        split_daemon.stop()

    def ws_data(self):
        # Use sid2 after its QUIC daemon stopped; no user daemon is involved.
        for role in ("daemon", "client"):
            path = f"/ws/data/{self.sid2}?role={role}"
            for token, status in ((None, 401), ("invalid-local-smoke-jwt", 401), (self.other, 404)):
                WebSocket(self.port, path, token, expected_status=status)
        passed("optional WS data endpoints enforce bearer authentication and session ownership")
        daemon = WebSocket(self.port, f"/ws/data/{self.sid2}?role=daemon", self.owner)
        self.sockets.append(daemon)
        # Ping/pong is an ordered barrier after daemon registration.
        daemon.send(b"ws-ready", opcode=9)
        for _ in range(10):
            opcode, data = daemon.receive()
            if opcode == 9:
                daemon.send(data, opcode=10)
            if opcode == 10 and data == b"ws-ready":
                break
        else:
            raise AssertionError("WS daemon registration barrier failed")
        client = WebSocket(self.port, f"/ws/data/{self.sid2}?role=client&resume_from=0", self.owner)
        self.sockets.append(client)
        header = client.binary()
        check(len(header) == 8, "WS client did not receive a u64 start cursor first")
        start = struct.unpack("!Q", header)[0]
        catchup = client.binary()
        check(start == 0 and catchup.startswith("ababacafé ✓".encode()), "WS catch-up differs from QUIC history")
        cursor = start + len(catchup)
        check(daemon.binary() == b"\x02", "WS daemon missed client attached frame")

        payload = "WS_ROUNDTRIP_✓\n".encode()
        frame = b"\x00" + struct.pack("!I", len(payload)) + payload
        client.send(frame)
        check(daemon.binary() == frame, "WS client input framing changed")
        daemon.send(payload)
        check(client.binary() == payload, "WS daemon raw output did not reach client")
        cursor += len(payload)
        size = b"\x01" + struct.pack("!HH", 99, 33)
        client.send(size)
        check(daemon.binary() == size, "WS resize framing changed")
        client.send(b"\x02")
        check(daemon.binary() == b"\x03", "WS explicit detach did not reach daemon")
        client.close()
        self.sockets.remove(client)
        delta = b"WS_DELTA_ONLY\n"
        daemon.send(delta)
        resumed = WebSocket(self.port, f"/ws/data/{self.sid2}?role=client&resume_from={cursor}", self.owner)
        self.sockets.append(resumed)
        check(struct.unpack("!Q", resumed.binary())[0] == cursor, "WS resume start cursor changed")
        check(resumed.binary() == delta, "WS resume replayed consumed history")
        check(daemon.binary() == b"\x02", "resumed WS attach was not forwarded")
        resumed.send(b"\x02")
        check(daemon.binary() == b"\x03", "resumed WS detach was not forwarded")
        passed("optional WS-only raw output, framed input, resize, detach, and delta replay round-trip")

        # Cross-transport routing uses the same history and auth registry.
        cross = self.client(("--resume-from", str(cursor), "--expect", delta.decode()), sid=self.sid2)
        check(cross["start"] == cursor and cross["received"] == len(delta), "WS-to-QUIC history did not preserve cursor")
        passed("optional WebSocket-daemon output resumes through pinned QUIC client")

    def close(self):
        self.stop_heartbeat.set()
        if self.heartbeat_thread:
            self.heartbeat_thread.join(timeout=4)
        for child in reversed(self.children):
            child.stop()
        for ws in reversed(self.sockets):
            ws.close()

    def run(self):
        try:
            self.start()
            self.auth_checks()
            self.roundtrip()
            if self.args.ws_data:
                self.ws_data()
        except BaseException:
            for child in self.children:
                tail = "".join(child.lines[-16:])
                if tail:
                    print(self.redact(tail), file=sys.stderr)
            raise
        finally:
            self.close()


def cli_checks(client):
    env = clean_env()
    env["FIL_ATTACH_TICKET"] = "local-smoke-ticket-not-a-real-credential"
    try:
        result = subprocess.run(
            [str(client), "--session", "local-smoke", "--addr", "127.0.0.1:9",
             "--read-secs", "0.1"],
            env=env, capture_output=True, text=True, timeout=2,
        )
    except subprocess.TimeoutExpired:
        raise AssertionError("missing certificate must fail before attempting QUIC") from None
    check(result.returncode != 0, "missing certificate was accepted")
    check("FIL_QUIC_CERTIFICATE" in result.stderr, "missing certificate diagnostic absent")
    check("CONNECTING" not in result.stderr, "network used before credentials validated")
    check(env["FIL_ATTACH_TICKET"] not in result.stderr, "ticket leaked in diagnostics")
    print("PASS CLI rejects missing certificate before network I/O", flush=True)
    env.pop("FIL_ATTACH_TICKET")
    env["FIL_QUIC_CERTIFICATE"] = "AQID"
    try:
        result = subprocess.run(
            [str(client), "--session", "local-smoke", "--addr", "127.0.0.1:9"],
            env=env, capture_output=True, text=True, timeout=2,
        )
    except subprocess.TimeoutExpired:
        raise AssertionError("missing ticket must fail before attempting QUIC") from None
    check(result.returncode != 0 and "FIL_ATTACH_TICKET" in result.stderr, "missing ticket was accepted")
    check("CONNECTING" not in result.stderr, "network used before ticket validated")
    print("PASS CLI rejects missing ticket before network I/O", flush=True)
    env["FIL_ATTACH_TICKET"] = "local-smoke-ticket-not-a-real-credential"
    base = [str(client), "--session", "local-smoke", "--addr", "127.0.0.1:9"]
    cases = [(["--read-secs", "NaN"], {}), (["--read-secs", "0"], {}),
             (["--connect-timeout-secs", "inf"], {}), (["--io-timeout-secs=-1"], {}),
             (["--resize", "0x24"], {}), (["--resize", "65536x24"], {}),
             (["--resize", "invalid"], {}), (["--stream-type", "0x12"], {}),
             (["--stream-type", "2"], {}), (["--expect", ""], {}),
             (["--session", ""], {}), (["--send", "x", "--send-after", "6"], {}),
             ([], {"FIL_ATTACH_TICKET": ""}), ([], {"FIL_ATTACH_TICKET": "x" * 129}),
             ([], {"FIL_QUIC_CERTIFICATE": "not-base64"}),
             (["--insecure"], {"FIL_QUIC_CERTIFICATE": ""})]
    if os.name == "posix":
        cases.append(([], {"FIL_ATTACH_TICKET": "private-\udcff-value"}))
    for extra, overrides in cases:
        result = subprocess.run(base + extra, env={**env, **overrides}, capture_output=True, text=True, timeout=2)
        check(result.returncode != 0 and "CONNECTING" not in result.stderr, "invalid CLI configuration used the network")
        check("private-" not in result.stderr and "panicked" not in result.stderr, "secret leaked or input validation panicked")
    passed("CLI rejects invalid durations, dimensions, stream versions, credentials, and empty expectations")

    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sink:
        sink.bind(("127.0.0.1", 0))
        command = [str(client), "--session", "local-smoke", "--addr", f"127.0.0.1:{sink.getsockname()[1]}",
                   "--connect-timeout-secs", "0.1"]
        began = time.monotonic()
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=2)
        check(result.returncode != 0 and "QUIC connect timed out" in result.stderr, "silent UDP peer did not time out")
        check(time.monotonic() - began < 2, "connect deadline was exceeded")
    passed("silent UDP peer fails within the configured connect deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client-bin", type=Path, default=ROOT / "target/debug/fil-testclient")
    parser.add_argument("--hub-bin", type=Path, default=ROOT / "target/debug/fil-hub")
    parser.add_argument("--skip-build", action="store_true", help="use already-built binaries")
    parser.add_argument("--cli-only", action="store_true")
    parser.add_argument("--ws-data", action="store_true", help="also verify the optional /ws/data fallback and cross-transport replay")
    args = parser.parse_args()
    args.client_bin = args.client_bin.resolve()
    args.hub_bin = args.hub_bin.resolve()
    if not args.skip_build:
        subprocess.run(["cargo", "build", "--locked", "-p", "fil-testclient", "-p", "fil-hub"],
                       cwd=ROOT, check=True, timeout=300)
    cli_checks(args.client_bin)
    if not args.cli_only:
        with tempfile.TemporaryDirectory(prefix="fil-smoke-") as directory:
            Smoke(args, directory).run()
    print("PASS smoke suite complete; disposable processes and state cleaned up", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, ValueError, http.client.HTTPException) as error:
        print(f"FAIL {error}", file=sys.stderr)
        sys.exit(1)
    except (subprocess.SubprocessError, KeyboardInterrupt):
        # Subprocess exceptions can include argv: do not print captured commands.
        print("FAIL subprocess failed/timed out or smoke run interrupted", file=sys.stderr)
        sys.exit(1)
