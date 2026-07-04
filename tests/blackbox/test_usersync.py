"""Blackbox e2e for native user pairing/sync.

Drives two real vaults: a server (`mgmt web` on an ephemeral loopback port, open mode) and a client
that imports a `mgmt://pair/...` URL, clones the server vault, then syncs bidirectionally.
"""

import base64
import json
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request

import pytest


def _run(mgmt_bin, env, data_dir, *args):
    return subprocess.run(
        [mgmt_bin, "--data-dir", str(data_dir), *args],
        env=env, capture_output=True, text=True, timeout=60,
    )


class Server:
    """A spawned `mgmt web serve` on an ephemeral loopback port (open mode), torn down on exit."""

    def __init__(self, mgmt_bin, env, data_dir):
        self.proc = subprocess.Popen(
            [mgmt_bin, "--data-dir", str(data_dir), "web", "serve", "--bind", "127.0.0.1:0"],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        self.base = None
        deadline = time.time() + 15
        while time.time() < deadline:
            line = self.proc.stdout.readline()
            if not line:
                if self.proc.poll() is not None:
                    raise RuntimeError("server exited before binding")
                continue
            m = re.search(r"listening on (http://\S+)", line)
            if m:
                self.base = m.group(1)
                break
        if not self.base:
            raise RuntimeError("server did not report a listening address")

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.proc.send_signal(signal.SIGINT)
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def _get(url, headers=None):
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def _pair_url(base, token="x", user="admin"):
    payload = json.dumps({"host": base, "token": token, "user": user}).encode()
    blob = base64.urlsafe_b64encode(payload).rstrip(b"=").decode()
    return f"mgmt://pair/{blob}"


def test_bidirectional_pairing_sync(mgmt_bin, env, tmp_path):
    # Isolate the shared config home (holds sync-pairings.yaml).
    (tmp_path / "cfg" / "mgmt").mkdir(parents=True)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")
    server_dir = tmp_path / "server"
    server_dir.mkdir()
    client_dir = tmp_path / "client"
    client_dir.mkdir()

    # Seed a task on the server, then bring the (open, loopback) server up.
    assert _run(mgmt_bin, env, server_dir, "add", "on-server").returncode == 0

    with Server(mgmt_bin, env, server_dir) as srv:
        # The server migrated its vault to users/admin and serves it over /api/sync.
        url = _pair_url(srv.base)

        # Client imports the URL: clone the server vault.
        imp = _run(mgmt_bin, env, client_dir, "pair", "import", url)
        assert imp.returncode == 0, imp.stderr
        client_tasks = list((client_dir / "tasks").glob("*.md"))
        assert any("on-server" in p.read_text() for p in client_tasks), imp.stdout

        # Create a task on the client and sync — it should push to the server.
        assert _run(mgmt_bin, env, client_dir, "add", "on-client").returncode == 0
        sync = _run(mgmt_bin, env, client_dir, "sync")
        assert sync.returncode == 0, sync.stderr

        # The server (open mode) now lists both tasks over /api/sync.
        status, body = _get(f"{srv.base}/api/sync/tasks")
        assert status == 200, body
        assert len(json.loads(body)) == 2, body

    # A second server pass would also pull `on-client` back to any other paired node; here we've
    # proven clone (pull) + local-create push in one round trip.


def _send(method, url, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers={"content-type": "application/json"}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def test_caldav_config_crud_via_web(mgmt_bin, env, tmp_path):
    # Open loopback server (no password) => requests run as admin, so the admin-only CalDAV config
    # endpoints are reachable without a login.
    (tmp_path / "cfg" / "mgmt").mkdir(parents=True)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")
    data_dir = tmp_path / "data"
    data_dir.mkdir()

    with Server(mgmt_bin, env, data_dir) as srv:
        base = srv.base
        # Save a CalDAV account + one collection.
        status, _ = _send("POST", f"{base}/api/config/caldav/accounts", {
            "account": {"name": "fastmail", "auth": "basic", "username": "me", "password": "secret"},
            "collections": [{"name": "work", "kind": "events", "url": "https://caldav.fastmail.com/dav/calendars/x/"}],
        })
        assert status == 200

        # GET reflects it, with the password redacted (never returned in the clear).
        status, body = _get(f"{base}/api/config/caldav")
        assert status == 200
        cfg = json.loads(body)
        acct = next(a for a in cfg["accounts"] if a["name"] == "fastmail")
        assert acct["has_password"] is True
        assert "password" not in acct
        assert any(c["name"] == "work" and c["kind"] == "events" for c in cfg["collections"])

    # It was written to the web-managed caldav.yaml (never config.yaml).
    caldav_yaml = (tmp_path / "cfg" / "mgmt" / "caldav.yaml").read_text()
    assert "fastmail" in caldav_yaml and "secret" in caldav_yaml
    assert not (tmp_path / "cfg" / "mgmt" / "config.yaml").exists()

    with Server(mgmt_bin, env, data_dir) as srv:
        # Delete removes the account and its collections.
        assert _send("DELETE", f"{srv.base}/api/config/caldav/accounts/fastmail")[0] == 200
        cfg = json.loads(_get(f"{srv.base}/api/config/caldav")[1])
        assert cfg["accounts"] == [] and cfg["collections"] == []


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
