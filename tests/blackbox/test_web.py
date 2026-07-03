"""Blackbox tests for `mgmt web`: the HTTP API, auth flow, and mutations.

Drives the real binary: sets a password via `mgmt web setpass`, spawns `mgmt web serve` on an
ephemeral port, and exercises the endpoints over stdlib HTTP (no extra deps).
"""

import http.cookiejar
import json
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request

import pytest


def _setup_auth(env, tmp_path):
    cfg_dir = tmp_path / "cfg" / "mgmt"
    cfg_dir.mkdir(parents=True)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")
    return cfg_dir


def _run(mgmt_bin, env, data_dir, *args, **kwargs):
    return subprocess.run(
        [mgmt_bin, "--data-dir", str(data_dir), *args],
        env=env, capture_output=True, text=True, timeout=30, **kwargs,
    )


class Server:
    """A spawned `mgmt web serve` on an ephemeral port, torn down on exit."""

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


def _opener():
    jar = http.cookiejar.CookieJar()
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))


def _get(opener, url, headers=None):
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with opener.open(req, timeout=10) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def _post(opener, url, body=None, headers=None):
    return _send(opener, "POST", url, body, headers)


def _put(opener, url, body=None, headers=None):
    return _send(opener, "PUT", url, body, headers)


def _send(opener, method, url, body=None, headers=None):
    data = json.dumps(body).encode() if body is not None else b""
    h = {"content-type": "application/json"}
    h.update(headers or {})
    req = urllib.request.Request(url, data=data, headers=h, method=method)
    try:
        with opener.open(req, timeout=10) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def test_auth_flow_and_mutation(mgmt_bin, env, data_dir, tmp_path):
    _setup_auth(env, tmp_path)
    env["MGMT_WEB_PASSWORD"] = "s3cret"
    assert _run(mgmt_bin, env, data_dir, "web", "setpass").returncode == 0
    _run(mgmt_bin, env, data_dir, "add", "Seed task")

    with Server(mgmt_bin, env, data_dir) as srv:
        op = _opener()
        # health is public
        assert _get(op, f"{srv.base}/api/health")[0] == 200
        # unauthenticated read is rejected
        assert _get(op, f"{srv.base}/api/tasks")[0] == 401
        # bad login rejected
        assert _post(op, f"{srv.base}/api/auth/login", {"password": "nope"})[0] == 401
        # good login sets a cookie on the jar
        assert _post(op, f"{srv.base}/api/auth/login", {"password": "s3cret"})[0] == 200
        # now reads work
        status, body = _get(op, f"{srv.base}/api/tasks")
        assert status == 200
        assert any(t["title"] == "Seed task" for t in json.loads(body))
        # create a task via the API
        status, body = _post(op, f"{srv.base}/api/tasks", {"title": "From API", "project": "wng"})
        assert status == 200
        assert json.loads(body)["title"] == "From API"


def test_bearer_token_reaches_sync_endpoint(mgmt_bin, env, data_dir, tmp_path):
    _setup_auth(env, tmp_path)
    env["MGMT_WEB_PASSWORD"] = "pw"
    _run(mgmt_bin, env, data_dir, "web", "setpass")
    _run(mgmt_bin, env, data_dir, "add", "A task")
    out = _run(mgmt_bin, env, data_dir, "web", "token-new", "laptop").stdout
    token = out.strip().splitlines()[-1].strip()

    with Server(mgmt_bin, env, data_dir) as srv:
        op = _opener()
        # sync endpoint rejects anonymous access
        assert _get(op, f"{srv.base}/api/sync/tasks")[0] == 401
        # ...but accepts the bearer token
        status, body = _get(op, f"{srv.base}/api/sync/tasks", {"Authorization": f"Bearer {token}"})
        assert status == 200
        items = json.loads(body)
        assert len(items) == 1
        assert items[0]["href"].endswith(".md")
        assert items[0]["etag"].startswith('"')


def test_loopback_serve_without_password_warns_but_runs(mgmt_bin, env, data_dir, tmp_path):
    # No password set; loopback bind is allowed (dev), server should still come up.
    _setup_auth(env, tmp_path)
    _run(mgmt_bin, env, data_dir, "add", "Open task")
    with Server(mgmt_bin, env, data_dir) as srv:
        op = _opener()
        # With auth disabled, reads are open.
        status, body = _get(op, f"{srv.base}/api/tasks")
        assert status == 200
        assert any(t["title"] == "Open task" for t in json.loads(body))


def test_new_endpoints_agenda_state_trash_project_meta(mgmt_bin, env, data_dir, tmp_path):
    # Open loopback server (no password) keeps the HTTP checks simple.
    _setup_auth(env, tmp_path)
    _run(mgmt_bin, env, data_dir, "add", "Trashable")

    with Server(mgmt_bin, env, data_dir) as srv:
        op = _opener()
        base = srv.base

        # /api/meta now carries calendar + theme + config views
        status, body = _get(op, f"{base}/api/meta")
        assert status == 200
        meta = json.loads(body)
        assert "calendar" in meta and "month_panel_style" in meta["calendar"]
        assert "theme" in meta and "default_reminders" in meta
        assert isinstance(meta["views"], list) and len(meta["views"]) >= 1

        # /api/state shape
        st = json.loads(_get(op, f"{base}/api/state")[1])
        assert set(["dirty", "can_undo", "can_redo"]).issubset(st.keys())

        # /api/agenda returns {events, tasks}
        ag = json.loads(_get(op, f"{base}/api/agenda?from=2026-01-01T00:00:00Z&to=2027-01-01T00:00:00Z")[1])
        assert "events" in ag and "tasks" in ag

        # project color via PUT /projects/:name → reflected in /meta
        assert _post(op, f"{base}/api/projects", {"name": "wng"})[0] == 200
        assert _put(op, f"{base}/api/projects/wng", {"color": "#89b4fa"})[0] == 200
        meta2 = json.loads(_get(op, f"{base}/api/meta")[1])
        assert any(p["name"] == "wng" and p["color"] == "#89b4fa" for p in meta2["projects"])

        # priority cycle + project assign
        uid = json.loads(_get(op, f"{base}/api/tasks")[1])[0]["uid"]
        assert json.loads(_post(op, f"{base}/api/tasks/{uid}/priority")[1])["priority"] != "None"
        assert json.loads(_post(op, f"{base}/api/tasks/{uid}/project", {"project": "wng"})[1])["project"] == "wng"

        # trash lifecycle: delete → in trash → restore → gone
        assert _send(op, "DELETE", f"{base}/api/tasks/{uid}")[0] == 200
        trash = json.loads(_get(op, f"{base}/api/trash")[1])
        assert not trash["empty"] and any(t["uid"] == uid for t in trash["tasks"])
        assert _post(op, f"{base}/api/trash/restore", {"kind": "task", "id": uid})[0] == 200
        assert json.loads(_get(op, f"{base}/api/trash")[1])["empty"]
        assert any(t["uid"] == uid for t in json.loads(_get(op, f"{base}/api/tasks")[1]))


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
