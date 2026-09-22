#!/usr/bin/env python3
"""Bounded, scratch-only real Fil PTY smoke test (HTTPS/WSS by default).

Production execution is operator-controlled; this script never deploys, pairs,
starts/restarts a daemon, or selects an existing terminal. Requires Python 3.11+.
"""

import argparse
import base64
import errno
import fcntl
import hashlib
import http.client
import json
import os
from pathlib import Path
import pty
import re
import secrets
import select
import shlex
import signal
import socket
import ssl
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tomllib
import urllib.parse

MAX_BYTES = 2 * 1024 * 1024
INITIAL_ROWS, INITIAL_COLS = 31, 97


class SmokeFailure(Exception):
    """Only static, credential-free diagnostics may be stored here."""


def check(condition, message):
    if not condition:
        raise SmokeFailure(message)


def hub_address(url, allow_local_http=False):
    try:
        check(isinstance(url, str) and not any(ord(c) < 33 for c in url), "invalid hub URL")
        parsed = urllib.parse.urlsplit(url)
        check(not parsed.username and not parsed.password and not parsed.query and not parsed.fragment,
              "hub URL must not contain credentials, a query, or a fragment")
        check(parsed.path in ("", "/") and parsed.hostname, "hub URL must be an origin, without a path")
        scheme, host = parsed.scheme, parsed.hostname
        check(scheme == "https" or (allow_local_http and scheme == "http" and host in ("127.0.0.1", "::1")),
              "verified HTTPS is required (HTTP is allowed only for explicit literal-loopback validation)")
        port = parsed.port or (443 if scheme == "https" else 80)
        check(1 <= port <= 65535, "invalid hub port")
        return scheme, host, port
    except ValueError:
        raise SmokeFailure("invalid hub URL") from None


def split_print(marker, newline=True):
    """The output marker is never contiguous in the echoed shell command."""
    middle = len(marker) // 2
    fmt = "%s%s\\n" if newline else "%s%s"
    return f"printf '{fmt}' {shlex.quote(marker[:middle])} {shlex.quote(marker[middle:])}"


def marker_command(marker):
    command = (split_print(marker) + "\n").encode("ascii")
    check(marker.encode() not in command, "marker would be satisfied by terminal input echo")
    return command


class Budget:
    def __init__(self, seconds):
        self.started = time.monotonic()
        self.end = self.started + seconds

    def remaining(self, cap=3):
        value = min(cap, self.end - time.monotonic())
        check(value > 0, "total work deadline exceeded")
        return value

    def until(self, seconds):
        return time.monotonic() + self.remaining(seconds)


class Hub:
    def __init__(self, address, token, budget, ca_file=None):
        self.scheme, self.host, self.port = address
        self.token = token
        self.budget = budget
        self.context = ssl.create_default_context(cafile=ca_file)
        self.context.minimum_version = ssl.TLSVersion.TLSv1_2

    def sessions(self):
        if self.scheme == "https":
            connection = http.client.HTTPSConnection(self.host, self.port,
                         context=self.context, timeout=self.budget.remaining())
        else:
            connection = http.client.HTTPConnection(self.host, self.port, timeout=self.budget.remaining())
        try:
            connection.request("GET", "/sessions", headers={"Authorization": f"Bearer {self.token}"})
            response = connection.getresponse()
            check(response.status == 200, "authenticated /sessions request failed (no redirects followed)")
            raw = response.read(MAX_BYTES + 1)
            check(len(raw) <= MAX_BYTES, "session snapshot exceeded the safety limit")
            data = json.loads(raw)
            check(isinstance(data, list), "invalid session snapshot")
            sessions, devices = {}, {}
            for device in data:
                check(isinstance(device, dict) and isinstance(device.get("device_id"), str)
                      and isinstance(device.get("sessions"), list), "invalid device in session snapshot")
                device_id = device["device_id"]
                check(device_id not in devices, "duplicate device in session snapshot")
                devices[device_id] = device
                for session in device["sessions"]:
                    check(isinstance(session, dict) and isinstance(session.get("session_id"), str),
                          "invalid session in snapshot")
                    sid = session["session_id"]
                    check(sid not in sessions, "ambiguous duplicate session ID")
                    sessions[sid] = (device_id, session)
            return devices, sessions
        finally:
            connection.close()


class Viewer:
    """Verified WSS + bounded incremental RFC6455 parsing; no third-party deps."""
    def __init__(self, hub, sid, resume_from):
        self.hub = hub
        self.sock = None
        self.wire = bytearray()
        self.fragments = None
        self.data = bytearray()
        self.start = None
        self.cursor = None
        self.closed = False
        try:
            self.sock = socket.create_connection((hub.host, hub.port), timeout=hub.budget.remaining())
            if hub.scheme == "https":
                self.sock = hub.context.wrap_socket(self.sock, server_hostname=hub.host)
            self.sock.settimeout(hub.budget.remaining())
            key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
            host = f"[{hub.host}]" if ":" in hub.host else hub.host
            path = f"/ws/data/{urllib.parse.quote(sid, safe='')}?role=client&resume_from={resume_from}"
            request = (f"GET {path} HTTP/1.1\r\nHost: {host}:{hub.port}\r\n"
                       "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
                       f"Sec-WebSocket-Key: {key}\r\nAuthorization: Bearer {hub.token}\r\n\r\n")
            self.sock.sendall(request.encode("ascii"))
            while b"\r\n\r\n" not in self.wire:
                chunk = self.sock.recv(4096)
                check(chunk, "WSS upgrade ended before its HTTP response")
                self.wire.extend(chunk)
                check(len(self.wire) <= 32768, "oversized WSS upgrade response")
            header, leftover = self.wire.split(b"\r\n\r\n", 1)
            self.wire = bytearray(leftover)
            lines = bytes(header).split(b"\r\n")
            check(len(lines[0].split()) >= 2 and lines[0].split()[1] == b"101", "WSS upgrade rejected")
            headers = {}
            for line in lines[1:]:
                check(b":" in line, "invalid WSS response header")
                name, value = line.split(b":", 1)
                name = name.lower()
                check(name not in headers, "duplicate WSS response header")
                headers[name] = value.strip()
            proof = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
            check(headers.get(b"sec-websocket-accept") == proof
                  and headers.get(b"upgrade", b"").lower() == b"websocket"
                  and b"upgrade" in headers.get(b"connection", b"").lower().split(b", "), "invalid WSS upgrade proof")
            check(b"sec-websocket-extensions" not in headers, "unrequested WSS extensions")
            self.sock.setblocking(False)
        except BaseException:
            self.close()
            raise

    def send(self, data, opcode=2):
        check(not self.closed and self.sock is not None, "write on closed scratch viewer")
        check(len(data) <= 1024 * 1024, "outgoing WSS frame exceeded safety limit")
        mask = secrets.token_bytes(4)
        size = len(data)
        if size < 126:
            header = bytes([0x80 | opcode, 0x80 | size])
        elif size < 65536:
            header = bytes([0x80 | opcode, 0xfe]) + struct.pack("!H", size)
        else:
            header = bytes([0x80 | opcode, 0xff]) + struct.pack("!Q", size)
        frame = header + mask + bytes(value ^ mask[i % 4] for i, value in enumerate(data))
        end = self.hub.budget.until(3)
        view = memoryview(frame)
        while view:
            check(time.monotonic() < end, "WSS write deadline exceeded")
            try:
                count = self.sock.send(view)
                check(count > 0, "WSS write closed unexpectedly")
                view = view[count:]
            except (BlockingIOError, ssl.SSLWantWriteError):
                select.select([], [self.sock], [], 0.02)
            except ssl.SSLWantReadError:
                select.select([self.sock], [], [], 0.02)

    def input(self, data):
        self.send(b"\x00" + struct.pack("!I", len(data)) + data)

    def resize(self, rows, cols):
        self.send(b"\x01" + struct.pack("!HH", cols, rows))

    def binary(self, data):
        if self.start is None:
            check(len(data) == 8, "first WSS binary message was not the start cursor")
            self.start = self.cursor = struct.unpack("!Q", data)[0]
            return
        check(len(self.data) + len(data) <= MAX_BYTES, "remote terminal output exceeded safety limit")
        check(self.cursor + len(data) <= 2**64 - 1, "remote cursor overflow")
        self.data.extend(data)
        self.cursor += len(data)

    def parse_frames(self):
        while len(self.wire) >= 2:
            first, second = self.wire[:2]
            check(not (first & 0x70) and not (second & 0x80), "invalid server WSS frame flags")
            opcode, final = first & 15, bool(first & 0x80)
            size, offset = second & 127, 2
            if size in (126, 127):
                width = 2 if size == 126 else 8
                if len(self.wire) < offset + width:
                    return
                size = int.from_bytes(self.wire[offset:offset + width], "big")
                offset += width
            check(size <= MAX_BYTES, "oversized WSS frame")
            if opcode >= 8:
                check(final and size <= 125, "invalid WSS control frame")
            if len(self.wire) < offset + size:
                return
            data = bytes(self.wire[offset:offset + size])
            del self.wire[:offset + size]
            if opcode == 9:
                self.send(data, opcode=10)
            elif opcode == 10:
                continue
            elif opcode == 8:
                check(size != 1, "invalid WSS close frame")
                self.send(data, opcode=8)
                self.close()
                return
            elif opcode == 2:
                check(self.fragments is None, "overlapping WSS fragmented messages")
                if final:
                    self.binary(data)
                else:
                    self.fragments = bytearray(data)
            elif opcode == 0:
                check(self.fragments is not None, "unexpected WSS continuation")
                self.fragments.extend(data)
                check(len(self.fragments) <= MAX_BYTES, "oversized fragmented WSS message")
                if final:
                    self.binary(bytes(self.fragments))
                    self.fragments = None
            else:
                raise SmokeFailure("expected binary scratch terminal output")

    def poll(self):
        if self.closed:
            return
        self.parse_frames()
        # Limit work per pump so a flooding peer cannot starve the PTY/deadlines.
        for _ in range(32):
            if self.closed:
                return
            try:
                data = self.sock.recv(65536)
            except (BlockingIOError, ssl.SSLWantReadError, ssl.SSLWantWriteError):
                return
            if not data:
                self.close()
                return
            self.wire.extend(data)
            check(len(self.wire) <= MAX_BYTES + 14, "WSS receive buffer exceeded safety limit")
            self.parse_frames()

    def close(self, abrupt=False):
        # No TLS unwrap or WebSocket close frame here. abrupt models a lost
        # transport; graceful detach is explicitly sent by the test beforehand.
        self.closed = True
        if self.sock is not None:
            if abrupt:
                try:
                    self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                except OSError:
                    pass
            self.sock.close()
            self.sock = None


class LiveSmoke:
    def __init__(self, args, budget):
        self.args, self.budget = args, budget
        self.stage = "preflight"
        self.passes = []
        self.proxy = None
        self.master = None
        self.temporary = None
        self.local = bytearray()
        self.local_eof = False
        self.viewers = []
        self.sid = None
        self.baseline = set()
        self.cleanup_ok = False

    def passed(self, name):
        self.passes.append(name)
        print(f"PASS {name}", file=sys.stderr if self.args.json else sys.stdout, flush=True)

    def config_bytes(self):
        with (self.config_dir / "config.toml").open("rb") as source:
            metadata = os.fstat(source.fileno())
            check(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid(),
                  "pairing config must be a regular file owned by the current user")
            check(metadata.st_mode & 0o077 == 0, "pairing config must be private (mode 0600 or stricter)")
            raw = source.read(65537)
            check(len(raw) <= 65536, "pairing config exceeded safety limit")
            return raw

    def preflight(self):
        self.config_dir = self.args.config_dir.resolve(strict=True)
        config_raw = self.config_bytes()
        self.config_hash = hashlib.sha256(config_raw).digest()
        config = tomllib.loads(config_raw.decode("utf-8"))
        token, device_id = config.get("token"), config.get("device_id")
        check(isinstance(token, str) and token and token.isascii()
              and not any(c.isspace() or ord(c) < 33 for c in token), "pairing token is missing or invalid")
        check(isinstance(device_id, str) and device_id, "pairing device ID is missing")
        address = hub_address(self.args.hub, self.args.allow_local_http)
        check(address == hub_address(config.get("hub_url"), self.args.allow_local_http),
              "explicit hub URL does not match the pairing config; refusing to send credentials")
        sock = (self.config_dir / "daemon.sock").stat()
        check(stat.S_ISSOCK(sock.st_mode), "existing daemon socket is required; no daemon will be started")
        self.socket_identity = sock.st_dev, sock.st_ino
        self.proxy_bin = self.args.proxy_bin.resolve(strict=True)
        check(self.proxy_bin.is_file() and os.access(self.proxy_bin, os.X_OK), "proxy binary is not executable")
        self.device_id = device_id
        self.hub = Hub(address, token, self.budget, self.args.ca_file)
        devices, sessions = self.hub.sessions()
        check(device_id in devices and devices[device_id].get("connected") is True,
              "paired daemon is not online in the authenticated session registry")
        self.baseline = set(sessions)
        self.passed("authenticated baseline and private pairing preflight")

    def launch(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="fil-live-pty-")
        self.cwd = str(Path(self.temporary.name).resolve())
        env = {"HOME": self.cwd, "PWD": self.cwd, "TMPDIR": self.cwd, "SHELL": "/bin/sh",
               "TERM": "xterm-256color", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LC_ALL": "C",
               "ENV": "/dev/null", "BASH_ENV": "/dev/null", "HISTFILE": "/dev/null", "PS1": "",
               "FIL_CONFIG_DIR": str(self.config_dir), "FIL_LOG": "off"}
        master, slave = pty.openpty()
        self.master = master
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", INITIAL_ROWS, INITIAL_COLS, 0, 0))
            os.set_blocking(master, False)
            self.proxy = subprocess.Popen([str(self.proxy_bin)], cwd=self.cwd, env=env,
                         stdin=slave, stdout=slave, stderr=slave, close_fds=True, start_new_session=True)
        finally:
            os.close(slave)

    def pump(self):
        self.budget.remaining()
        if self.master is not None and not self.local_eof:
            for _ in range(32):
                try:
                    data = os.read(self.master, 65536)
                except BlockingIOError:
                    break
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                if not data:
                    self.local_eof = True
                    break
                check(len(self.local) + len(data) <= MAX_BYTES, "local PTY output exceeded safety limit")
                self.local.extend(data)
        for viewer in self.viewers:
            viewer.poll()

    def local_write(self, data):
        check(self.proxy is not None and self.proxy.poll() is None, "own proxy exited before local input")
        view = memoryview(data)
        end = self.budget.until(3)
        while view:
            check(time.monotonic() < end, "local PTY write deadline exceeded")
            try:
                count = os.write(self.master, view)
                check(count > 0, "local PTY write closed")
                view = view[count:]
            except BlockingIOError:
                self.pump()
                select.select([], [self.master], [], 0.02)

    def wait(self, predicate, seconds, message, viewer=None, allow_exit=False):
        end = self.budget.until(seconds)
        while True:
            self.pump()
            if predicate():
                return
            check(time.monotonic() < end, message)
            if not allow_exit:
                check(self.proxy.poll() is None, "own proxy exited unexpectedly")
            if viewer is not None:
                check(not viewer.closed, "scratch viewer closed before the expected output")
            time.sleep(0.02)

    def marker(self, label):
        return "FILSMOKE_" + label + "_" + secrets.token_hex(12)

    def local_marker(self, label, viewer=None):
        marker = self.marker(label)
        self.local_write(marker_command(marker))
        needle = marker.encode()
        self.wait(lambda: needle in self.local and (viewer is None or needle in viewer.data), 8,
                  "local marker did not reach its expected destinations", viewer)
        check(self.local.count(needle) == 1 and (viewer is None or viewer.data.count(needle) == 1),
              "local output marker duplicated")
        return needle

    def remote_marker(self, viewer):
        marker = self.marker("REMOTE")
        viewer.input(marker_command(marker))
        needle = marker.encode()
        self.wait(lambda: needle in self.local and needle in viewer.data, 8,
                  "remote input did not reach both the viewer and local PTY", viewer)
        check(self.local.count(needle) == 1 and viewer.data.count(needle) == 1, "remote output marker duplicated")
        return needle

    def identify(self):
        end = self.budget.until(12)
        while time.monotonic() < end:
            self.pump()
            _, sessions = self.hub.sessions()
            candidates = [sid for sid, (device, session) in sessions.items()
                          if sid not in self.baseline and device == self.device_id and session.get("cwd") == self.cwd]
            check(len(candidates) <= 1, "multiple scratch sessions matched; refusing to attach")
            if candidates:
                self.sid = candidates[0]
                self.passed("only a new ID on the paired device with the unique scratch cwd is selected")
                return
            check(self.proxy.poll() is None, "own proxy exited before session registration")
            time.sleep(0.15)
        raise SmokeFailure("own scratch session was not registered before the deadline")

    def attach(self, cursor=0):
        check(self.sid is not None and self.sid not in self.baseline, "no positively identified scratch session")
        viewer = Viewer(self.hub, self.sid, cursor)
        self.viewers.append(viewer)
        self.wait(lambda: viewer.start is not None, 5, "WSS start cursor timed out", viewer)
        return viewer

    def geometry(self, seconds=2):
        prefix, suffix = self.marker("SIZE") + "=", self.marker("SIZE_END")
        command = (split_print(prefix, False) + "; /bin/stty size; " + split_print(suffix) + "\n").encode()
        check(prefix.encode() not in command and suffix.encode() not in command, "geometry marker leaked into command echo")
        self.local_write(command)
        self.wait(lambda: suffix.encode() in self.local, seconds, "stty size probe timed out")
        match = re.search(re.escape(prefix.encode()) + rb"(\d+)\s+(\d+)\r?\n" + re.escape(suffix.encode()), self.local)
        check(match is not None, "stty size did not return a valid geometry")
        return tuple(int(value) for value in match.groups())

    def geometry_until(self, expected, seconds, message):
        end = self.budget.until(seconds)
        while time.monotonic() < end:
            if self.geometry(min(2, max(0.05, end - time.monotonic()))) == expected:
                return
            time.sleep(0.15)
        raise SmokeFailure(message)

    def run(self):
        self.preflight()
        self.stage = "scratch_proxy_registration"
        self.launch()
        self.local_marker("READY")
        self.identify()
        check(self.geometry() == (INITIAL_ROWS, INITIAL_COLS), "initial real PTY geometry is incorrect")
        self.passed("isolated real /bin/sh PTY starts at the requested initial geometry")

        self.stage = "wss_roundtrip"
        viewer = self.attach()
        local_marker = self.local_marker("LOCAL_TO_REMOTE", viewer)
        self.passed("local input produces a non-echo marker on the authenticated remote viewer")
        remote_marker = self.remote_marker(viewer)
        self.passed("framed remote input reaches both the remote viewer and local real PTY")

        self.stage = "resize_and_detach"
        viewer.resize(19, 73)
        self.geometry_until((19, 73), 6, "remote resize did not change actual stty size")
        self.passed("remote resize changes actual shell stty size")
        viewer.send(b"\x02")
        self.wait(lambda: viewer.closed, 4, "explicit detach did not close the scratch viewer")
        cursor = viewer.cursor
        self.geometry_until((INITIAL_ROWS, INITIAL_COLS), 8, "explicit detach did not restore local PTY geometry")
        self.passed("explicit detach restores the initial local PTY geometry")

        self.stage = "resume_delta"
        delta = self.local_marker("DETACHED_DELTA")
        resumed = self.attach(cursor)
        check(resumed.start == cursor, "resume start cursor differs from the last consumed byte offset")
        self.wait(lambda: delta in resumed.data, 8, "resume delta did not arrive", resumed)
        check(resumed.data.count(delta) == 1 and local_marker not in resumed.data and remote_marker not in resumed.data,
              "resume duplicated already consumed markers")
        self.remote_marker(resumed)
        # Include everything received during the resumed command, not just the first frame.
        check(resumed.data.count(delta) == 1 and local_marker not in resumed.data and remote_marker not in resumed.data,
              "late replay duplicated a consumed marker")
        self.passed("reconnect replays only the unseen delta and remote input still works")

        self.stage = "abrupt_disconnect"
        resumed.resize(22, 85)
        self.geometry_until((22, 85), 6, "resumed resize did not reach the real shell")
        resumed.close(abrupt=True)
        self.geometry_until((INITIAL_ROWS, INITIAL_COLS), 18, "abrupt WSS disconnect did not restore geometry within 18 seconds")
        self.passed("abrupt WSS socket loss restores initial PTY geometry within 18 seconds")

        self.stage = "scratch_shell_exit"
        _, before_exit = self.hub.sessions()
        check(self.sid in before_exit, "scratch session disappeared before shell exit")
        survivors = (set(before_exit) | self.baseline) - {self.sid}
        exit_marker = self.marker("EXIT")
        self.local_write((split_print(exit_marker) + "; exit 0\n").encode())
        self.wait(lambda: self.proxy.poll() is not None, 6, "own proxy did not terminate after shell exit", allow_exit=True)
        check(self.proxy.returncode == 0 and exit_marker.encode() in self.local, "scratch shell did not exit cleanly")
        end = self.budget.until(10)
        while time.monotonic() < end:
            _, sessions = self.hub.sessions()
            if self.sid not in sessions:
                check(survivors <= set(sessions), "other session IDs changed; cannot confirm scratch-only removal")
                break
            time.sleep(0.15)
        else:
            raise SmokeFailure("scratch session remained registered after shell exit")
        check(hashlib.sha256(self.config_bytes()).digest() == self.config_hash, "pairing config changed during verification")
        sock = (self.config_dir / "daemon.sock").stat()
        check((sock.st_dev, sock.st_ino) == self.socket_identity, "daemon socket changed during verification")
        self.passed("shell exit removes only the scratch session; pairing and daemon socket remain unchanged")

    def cleanup(self):
        # No REST deletion, daemon control, external terminal input, or config writes.
        end = time.monotonic() + 4
        failures = False
        for viewer in self.viewers:
            try:
                viewer.close()
            except BaseException:
                failures = True
        try:
            if self.proxy is not None and self.proxy.poll() is None:
                try:
                    os.killpg(self.proxy.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    self.proxy.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    # Still our unreaped child, launched as a new process-group
                    # leader. Never signal the daemon's or a user's group.
                    try:
                        os.killpg(self.proxy.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    self.proxy.wait(timeout=max(0.1, end - time.monotonic()))
        except BaseException:
            failures = True
        finally:
            if self.master is not None:
                try:
                    os.close(self.master)
                except BaseException:
                    failures = True
                self.master = None
            if self.temporary is not None:
                try:
                    self.temporary.cleanup()
                except BaseException:
                    failures = True
        self.cleanup_ok = not failures
        check(self.cleanup_ok, "own-process cleanup did not complete")

    def summary(self, ok, error=None):
        return {"ok": ok, "stage": "complete" if ok else self.stage, "error": error,
                "passes": self.passes, "elapsed_seconds": round(time.monotonic() - self.budget.started, 3),
                "local_received_bytes": len(self.local), "remote_received_bytes": sum(len(v.data) for v in self.viewers),
                "cleanup_ok": self.cleanup_ok, "existing_sessions_at_start": len(self.baseline)}


def self_test():
    for unsafe in ("http://example.com", "https://user:secret@example.com",
                   "https://example.com?token=secret", "https://example.com/#secret"):
        try:
            hub_address(unsafe)
        except SmokeFailure:
            continue
        raise SmokeFailure("self-test: unsafe hub URL was accepted")
    check(hub_address("https://example.com") == ("https", "example.com", 443),
          "self-test: HTTPS normalization failed")
    print("PASS fail-closed hub URL validation", flush=True)
    for unsafe in ("http://localhost:8000", "http://127.0.0.2:8000", "http://example.com"):
        try:
            hub_address(unsafe, allow_local_http=True)
        except SmokeFailure:
            continue
        raise SmokeFailure("self-test: local HTTP exception escaped literal-loopback scope")
    check(hub_address("http://127.0.0.1:8000", True) == ("http", "127.0.0.1", 8000), "self-test: loopback URL failed")
    for marker in ("FILSMOKE_0123456789abcdef", "FILSMOKE_LONGER_ASCII_MARKER_1234567890"):
        command = marker_command(marker)
        check(marker.encode() not in command, "self-test: marker is contiguous in command")
        output = subprocess.run(["/bin/sh", "-c", command.decode()], capture_output=True, timeout=2, check=True)
        check(output.stdout == marker.encode() + b"\n", "self-test: command does not print its intended marker")
    context = ssl.create_default_context()
    check(context.check_hostname and context.verify_mode == ssl.CERT_REQUIRED, "self-test: TLS verification is disabled")
    print("PASS split-marker shell commands and verified TLS defaults", flush=True)
    from unittest.mock import patch
    class UpgradeFixture:
        def settimeout(self, _value): pass
        def setblocking(self, _value): pass
        def close(self): pass
        def sendall(self, request):
            key = re.search(rb"Sec-WebSocket-Key: ([^\r]+)", request).group(1)
            proof = base64.b64encode(hashlib.sha1(key + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest())
            self.reply = (b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                          b"Connection: Upgrade\r\nSec-WebSocket-Accept: " + proof + b"\r\n\r\n"
                          b"\x82\x08" + struct.pack("!Q", 37))
        def recv(self, _size):
            reply, self.reply = self.reply, b""
            return reply
    with patch.object(socket, "create_connection", return_value=UpgradeFixture()):
        viewer = Viewer(Hub(("http", "127.0.0.1", 1), "fixture-only", Budget(3)), "scratch", 37)
        viewer.parse_frames()
        check(viewer.start == 37 and viewer.cursor == 37 and not viewer.data,
              "self-test: upgrade discarded or counted the cursor bytes as terminal output")
        # One UTF-8 payload split across two WebSocket fragments and wire reads.
        viewer.wire.extend(b"\x02\x02a\xc3\x80")
        viewer.parse_frames()
        viewer.wire.extend(b"\x01\xa9")
        viewer.parse_frames()
        check(viewer.data == "aé".encode() and viewer.cursor == 40, "self-test: fragmented payload/cursor failed")
        viewer.close()
    print("PASS WSS upgrade, coalesced start cursor, and fragmented UTF-8 payload", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog=(
        "Operator invocation: python3 scripts/smoke-live-pty.py --hub https://YOUR-HUB "
        "--config-dir /absolute/private/fil --proxy-bin ./target/debug/fil --json"))
    parser.add_argument("--self-test", action="store_true", help="offline, no pairing or hub access")
    parser.add_argument("--hub", help="explicit HTTPS origin; must match config.toml hub_url")
    parser.add_argument("--config-dir", type=Path, help="existing private pairing directory (read-only)")
    parser.add_argument("--proxy-bin", type=Path, help="fil proxy executable; never a daemon executable")
    parser.add_argument("--timeout-secs", type=float, default=85, help="total budget including cleanup, 20..90 (default 85)")
    parser.add_argument("--ca-file", help="optional explicit trusted CA bundle; hostname verification remains enabled")
    parser.add_argument("--allow-local-http", action="store_true", help="dry-validation only: allow literal 127.0.0.1 or ::1 HTTP")
    parser.add_argument("--json", action="store_true", help="one JSON summary on stdout; compact PASS lines on stderr")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not (args.hub and args.config_dir and args.proxy_bin):
        parser.error("--hub, --config-dir and --proxy-bin are required")
    if not 20 <= args.timeout_secs <= 90:
        parser.error("--timeout-secs must be finite and between 20 and 90")
    budget = Budget(args.timeout_secs - 5)
    smoke = LiveSmoke(args, budget)
    def interrupt(_signum, _frame):
        raise SmokeFailure("work deadline exceeded or run interrupted")
    old_handlers = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGALRM, signal.SIGTERM, signal.SIGINT)}
    signal.setitimer(signal.ITIMER_REAL, args.timeout_secs - 5)
    ok, diagnostic = False, None
    try:
        smoke.run()
        ok = True
    except SmokeFailure as error:
        diagnostic = str(error)
    except ssl.SSLCertVerificationError:
        diagnostic = "TLS certificate or hostname verification failed"
    except BaseException as error:
        # Library exceptions can embed headers, TOML contents, paths, or raw
        # terminal data. Never stringify them or print a traceback.
        diagnostic = "verification failed (" + type(error).__name__ + ")"
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        try:
            smoke.cleanup()
        except BaseException:
            ok, diagnostic = False, "own-process cleanup did not complete"
        for sig, handler in old_handlers.items():
            signal.signal(sig, handler)
    summary = smoke.summary(ok, diagnostic)
    if not ok:
        print(f"FAIL {summary['stage']}: {diagnostic}", file=sys.stderr, flush=True)
    if args.json:
        print(json.dumps(summary, separators=(",", ":")), flush=True)
    elif ok:
        print(f"PASS live PTY smoke complete ({summary['elapsed_seconds']:.2f}s); own process and descriptors cleaned up", flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
