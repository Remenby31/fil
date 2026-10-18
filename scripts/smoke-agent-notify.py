#!/usr/bin/env python3
"""Local end-to-end check of agent notifications; never reads user credentials.

Starts a scratch hub (with a mock APNs on loopback), a real fil-daemon and a
real fil proxy in a PTY. Inside that terminal a driver speaks MCP to `fil mcp`
the way an agent would, while this script plays the phone: it reads the pushes
the mock APNs received and answers questions through the hub's HTTP API.

Usage: scripts/smoke-agent-notify.py [--skip-build]
Binaries come from $CARGO_TARGET_DIR (default ./target)/debug.
"""

import argparse
import hashlib
import hmac
import base64
import http.client
import http.server
import json
import os
from pathlib import Path
import pty
import secrets
import select
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
PUSH_TOKEN = "ab" * 32

# Runs inside the Fil terminal, i.e. as a descendant of the fil proxy.
DRIVER = r'''
import json, os, subprocess, sys

def session(fil, extra_env=None):
    env = dict(os.environ, **(extra_env or {}))
    proc = subprocess.Popen([fil, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            env=env, text=True, bufsize=1)
    counter = [0]
    def call(method, params=None):
        counter[0] += 1
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": counter[0], "method": method,
                                     "params": params or {}}) + "\n")
        while True:
            message = json.loads(proc.stdout.readline())
            if message.get("id") == counter[0]:
                return message
    def tool(name, arguments):
        return call("tools/call", {"name": name, "arguments": arguments})["result"]
    init = call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "smoke", "version": "0"}})
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    return proc, call, tool, init

fil, out = sys.argv[1], sys.argv[2]
results = {"env_session": os.environ.get("FIL_SESSION_ID")}
proc, call, tool, init = session(fil)
results["init"] = init["result"]
results["tools"] = [t["name"] for t in call("tools/list")["result"]["tools"]]
results["notify"] = tool("notify", {"title": "Tests passing", "message": "All 42 tests pass."})
results["ask"] = tool("ask", {"question": "Deploy to prod?", "options": ["Ship it", "Hold"],
                              "title": "Deploy"})
results["invalid"] = tool("ask", {"question": "?", "options": ["only one"]})
proc.stdin.close(); proc.wait(timeout=5)

# A client with a short tool timeout: ask returns pending, get_answer collects it.
proc, call, tool, init = session(fil, {"FIL_ASK_WAIT": "1"})
pending = tool("ask", {"question": "Which database?", "options": ["Postgres", "SQLite"]})
results["pending"] = pending
question_id = pending.get("structuredContent", {}).get("question_id")
results["get_answer"] = tool("get_answer", {"question_id": question_id}) if question_id else None
proc.stdin.close(); proc.wait(timeout=5)
open(out, "w").write(json.dumps(results))
'''


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def passed(message):
    print(f"PASS {message}", flush=True)


def jwt(subject, secret, lifetime=600):
    def encode(value):
        return base64.urlsafe_b64encode(value).rstrip(b"=")
    now = int(time.time())
    header = encode(b'{"alg":"HS256","typ":"JWT"}')
    payload = encode(json.dumps({"sub": subject, "iat": now, "exp": now + lifetime}).encode())
    signing = header + b"." + payload
    return (signing + b"." + encode(hmac.new(secret, signing, hashlib.sha256).digest())).decode()


def free_port(kind=socket.SOCK_STREAM):
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class MockApns(http.server.ThreadingHTTPServer):
    def __init__(self):
        self.pushes = []
        super().__init__(("127.0.0.1", 0), MockApnsHandler)


class MockApnsHandler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        self.server.pushes.append({"path": self.path, "headers": dict(self.headers.items()),
                                   "body": json.loads(body)})
        self.send_response(200)
        self.send_header("content-length", "0")
        self.end_headers()

    def log_message(self, *_args):
        pass


class Smoke:
    def __init__(self, bins, directory):
        self.bins = bins
        self.dir = Path(directory)
        self.port = free_port()
        self.quic_port = free_port(socket.SOCK_DGRAM)
        self.secret = secrets.token_hex(32)
        self.owner = jwt("smoke-owner", self.secret.encode())
        self.processes = []
        self.apns = MockApns()
        threading.Thread(target=self.apns.serve_forever, daemon=True).start()

    def http(self, method, path, body=None, expected=200):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        headers = {"Content-Type": "application/json", "Authorization": f"Bearer {self.owner}"}
        try:
            connection.request(method, path, body=None if body is None else json.dumps(body), headers=headers)
            response = connection.getresponse()
            raw = response.read()
            check(response.status == expected, f"{method} {path}: expected {expected}, got {response.status}")
            return json.loads(raw) if raw else None
        finally:
            connection.close()

    def spawn(self, command, env, **kwargs):
        log = open(self.dir / f"{Path(command[0]).name}.log", "w")
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, **kwargs)
        self.processes.append(process)
        return process

    def wait_until(self, predicate, seconds, message):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        raise AssertionError(message)

    def start_hub(self):
        key = self.dir / "apns.p8"
        ec = subprocess.run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout"],
                            capture_output=True, check=True).stdout
        key.write_bytes(subprocess.run(["openssl", "pkcs8", "-topk8", "-nocrypt"], input=ec,
                                       capture_output=True, check=True).stdout)
        database = self.dir / "hub.sqlite"
        env = {"PATH": os.environ["PATH"], "PORT": str(self.port), "QUIC_PORT": str(self.quic_port),
               "JWT_SECRET": self.secret, "DATABASE_URL": f"sqlite:{database}?mode=rwc",
               "DATA_DIR": str(self.dir), "PUBLIC_URL": f"http://127.0.0.1:{self.port}",
               "APNS_TEAM_ID": "SMOKETEAM1", "APNS_KEY_ID": "SMOKEKEY01", "APNS_PRIVATE_KEY_PATH": str(key),
               "APNS_ENDPOINT": f"http://127.0.0.1:{self.apns.server_address[1]}", "FIL_LOG": "fil_hub=info"}
        self.spawn([str(self.bins / "fil-hub")], env)
        self.wait_until(lambda: self._healthy(), 15, "hub did not become healthy")
        with sqlite3.connect(database, timeout=3) as db:
            db.execute("INSERT INTO users (id,provider,provider_id) VALUES ('smoke-owner','github','smoke-owner')")
        health = self.http("GET", "/health")
        check(health.get("agent_notifications") is True, "hub does not advertise agent notifications")
        self.device = self.http("POST", "/devices", {"name": "smoke-mac", "os": "macos", "hostname": "smoke"},
                                expected=201)["id"]
        self.http("POST", "/push-tokens", {"token": PUSH_TOKEN, "environment": "sandbox"}, expected=204)
        passed("hub with mock APNs; phone push token registered")

    def _healthy(self):
        try:
            self.http("GET", "/health")
            return True
        except (OSError, http.client.HTTPException, AssertionError):
            return False

    def start_daemon(self):
        self.config = self.dir / "cfg"
        self.config.mkdir(mode=0o700)
        (self.config / "config.toml").write_text(
            f'hub_url = "http://127.0.0.1:{self.port}"\nquic_host = "127.0.0.1"\nquic_port = {self.quic_port}\n'
            f'token = "{self.owner}"\ndevice_id = "{self.device}"\ndevice_name = "smoke-mac"\n')
        os.chmod(self.config / "config.toml", 0o600)
        env = {"PATH": os.environ["PATH"], "HOME": str(self.dir), "FIL_CONFIG_DIR": str(self.config),
               "FIL_LOG": "fil_daemon=info"}
        self.spawn([str(self.bins / "fil-daemon")], env)
        self.wait_until(lambda: (self.config / "daemon.sock").exists(), 10, "daemon socket missing")
        self.wait_until(lambda: any(d["device_id"] == self.device and d["connected"]
                                    for d in self.http("GET", "/sessions")), 10, "daemon never connected")
        passed("real fil-daemon connected to the scratch hub")

    def run_terminal(self):
        driver = self.dir / "driver.py"
        driver.write_text(DRIVER)
        out = self.dir / "results.json"
        env = {"PATH": os.environ["PATH"], "HOME": str(self.dir), "SHELL": "/bin/sh", "TERM": "xterm",
               "FIL_CONFIG_DIR": str(self.config)}
        pid, master = pty.fork()
        if pid == 0:
            os.execve(str(self.bins / "fil"), [str(self.bins / "fil")], env)
        self.proxy_pid = pid
        session = self.wait_until(self._only_session, 10, "proxy session never reached the hub")
        os.write(master, f"{sys.executable} {driver} {self.bins / 'fil'} {out}; exit\n".encode())

        answered = set()
        deadline = time.monotonic() + 60
        while not out.exists():
            check(time.monotonic() < deadline, "driver did not finish")
            if select.select([master], [], [], 0.05)[0]:
                try:
                    os.read(master, 65536)
                except OSError:
                    pass
            for push in list(self.apns.pushes):
                fil = push["body"]["fil"]
                if fil["kind"] == "ask" and fil["notification_id"] not in answered:
                    answered.add(fil["notification_id"])
                    if fil["options"] == ["Ship it", "Hold"]:
                        self.http("POST", f"/agent-notifications/{fil['notification_id']}/answer",
                                  {"answer": "Not an option"}, expected=400)
                        choice = "Hold"
                    else:
                        time.sleep(1.5)  # let the short-wait client give up first
                        choice = fil["options"][0]
                    self.http("POST", f"/agent-notifications/{fil['notification_id']}/answer",
                              {"answer": choice}, expected=204)
                    self.http("POST", f"/agent-notifications/{fil['notification_id']}/answer",
                              {"answer": choice}, expected=410)
        os.waitpid(pid, 0)
        os.close(master)
        return session, json.loads(out.read_text())

    def _only_session(self):
        for device in self.http("GET", "/sessions"):
            if device["device_id"] == self.device and device["sessions"]:
                return device["sessions"][0]["session_id"]
        return None

    def verify(self, session, results):
        check(results["env_session"] == session, "proxy did not export FIL_SESSION_ID to its shell")
        check(results["init"]["serverInfo"]["name"] == "fil", "initialize failed")
        check(results["tools"] == ["notify", "ask", "get_answer"], f"unexpected tools {results['tools']}")
        passed("fil mcp initialize + tools/list")

        notify = results["notify"]
        check(not notify.get("isError") and notify["structuredContent"]["delivered"] == 1,
              f"notify failed: {notify}")
        check(notify["structuredContent"]["session_id"] == session, "notify not tied to the terminal")
        push = next(p for p in self.apns.pushes if p["body"]["fil"]["kind"] == "notify")
        check(push["path"] == f"/3/device/{PUSH_TOKEN}", "push sent to the wrong token")
        check(push["headers"].get("apns-push-type") == "alert", "not an alert push")
        check(push["headers"].get("apns-topic") == "sh.fil.app", "wrong APNs topic")
        check(push["headers"].get("authorization", "").startswith("bearer "), "missing provider token")
        aps = push["body"]["aps"]
        check(aps["alert"]["title"] == "Tests passing" and aps["alert"]["subtitle"].startswith("smoke-mac"),
              f"unexpected alert {aps}")
        check(push["body"]["fil"]["session_id"] == session and aps["thread-id"] == session,
              "push does not deep link to the session")
        passed("notify → daemon → hub → APNs alert deep-linked to the terminal session")

        ask = results["ask"]
        check(ask["structuredContent"] == {"status": "answered", "answer": "Hold",
                                           "question_id": ask["structuredContent"]["question_id"]},
              f"ask failed: {ask}")
        push = next(p for p in self.apns.pushes if p["body"]["fil"]["options"] == ["Ship it", "Hold"])
        check(push["body"]["aps"]["mutable-content"] == 1 and push["body"]["aps"]["category"] == "fil.agent.ask",
              "question push cannot get option buttons")
        check(push["headers"].get("apns-expiration"), "question push has no expiry")
        passed("ask: blocking call returns the option chosen on the phone; bad/duplicate answers rejected")

        check(results["invalid"].get("isError"), "invalid options were accepted")
        check(results["pending"]["structuredContent"]["status"] == "pending", f"no pending: {results['pending']}")
        check(results["get_answer"]["structuredContent"]["answer"] == "Postgres",
              f"get_answer failed: {results['get_answer']}")
        passed("short-timeout clients: ask → pending → get_answer collects the answer")

    def close(self):
        for process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
        if getattr(self, "proxy_pid", None):
            try:
                os.kill(self.proxy_pid, 9)
            except ProcessLookupError:
                pass
        self.apns.shutdown()

    def run(self):
        try:
            self.start_hub()
            self.start_daemon()
            session, results = self.run_terminal()
            self.verify(session, results)
        except BaseException:
            for log in sorted(self.dir.glob("*.log")):
                tail = log.read_text(errors="replace").splitlines()[-15:]
                print(f"--- {log.name}\n" + "\n".join(tail), file=sys.stderr)
            raise
        finally:
            self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not args.skip_build:
        subprocess.run(["cargo", "build", "-p", "fil-hub", "-p", "fil-daemon", "-p", "fil-proxy"],
                       cwd=ROOT, check=True, timeout=600)
    bins_source = target / "debug"
    # sockaddr_un paths are short and macOS temp dirs are long.
    with tempfile.TemporaryDirectory(prefix="fil-an-", dir="/tmp") as directory:
        # Copy: a shared target dir can be rebuilt by another checkout mid-run.
        bins = Path(directory) / "bin"
        bins.mkdir()
        for name in ("fil", "fil-daemon", "fil-hub"):
            (bins / name).write_bytes((bins_source / name).read_bytes())
            os.chmod(bins / name, 0o755)
        Smoke(bins, directory).run()
    print("PASS agent notification smoke complete", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, ValueError, KeyError, http.client.HTTPException) as error:
        print(f"FAIL {error!r}", file=sys.stderr)
        sys.exit(1)
