"""Blackbox tests for `POST /api/chat` / `GET /api/chat/config`.

Drives the real `mgmt web serve` binary against a fake OpenAI-compatible `/v1/chat/completions`
stub spun up in-process (`http.server.ThreadingHTTPServer` in a background thread), scripted per
test with canned SSE frames (a `tool_calls` delta, then a plain-text delta, exactly like
llama-server streams). No real model, no real user data: every test runs against a fresh
`--data-dir` and a fresh stub instance.
"""

import json
import threading
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from test_web import Server, _opener, _get, _post  # reuse the auth-flow helpers


# ---- the fake upstream model --------------------------------------------------------------

def _frame(delta):
    payload = {"choices": [{"index": 0, "delta": delta, "finish_reason": None}]}
    return f"data: {json.dumps(payload)}\n\n".encode()


_DONE = b"data: [DONE]\n\n"


def _tool_round(name, args, call_id="call_1"):
    """One streamed round: a single tool-call delta, then `[DONE]` (no text)."""
    delta = {"tool_calls": [{"index": 0, "id": call_id, "function": {"name": name, "arguments": json.dumps(args)}}]}
    return _frame(delta) + _DONE


def _text_round(text):
    """One streamed round: a single content delta, then `[DONE]` (no tool calls)."""
    return _frame({"content": text}) + _DONE


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)  # drain the request body; the stub ignores its contents
        with self.server.lock:
            self.server.requests += 1
            body = self.server.queue.pop(0) if self.server.queue else _DONE
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def log_message(self, *a, **k):
        pass  # keep test output quiet


class Stub:
    """A scripted fake `/v1/chat/completions`: each queued entry is one round's full SSE body,
    consumed in request order. `requests` counts how many times it was actually called."""

    def __init__(self, rounds):
        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.srv.queue = list(rounds)
        self.srv.requests = 0
        self.srv.lock = threading.Lock()
        self.thread = threading.Thread(target=self.srv.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self.srv.server_port}/v1"

    @property
    def requests(self):
        return self.srv.requests

    def push(self, *rounds):
        with self.srv.lock:
            self.srv.queue.extend(rounds)

    def close(self):
        self.srv.shutdown()
        self.thread.join(timeout=2)


# ---- SSE client ---------------------------------------------------------------------------

def _post_sse(opener, url, body, headers=None):
    """POST `body` as JSON and parse the full SSE response (the mgmt server closes the stream
    once the loop ends, so a blocking read-to-EOF captures every event deterministically)."""
    data = json.dumps(body).encode()
    h = {"content-type": "application/json"}
    h.update(headers or {})
    req = urllib.request.Request(url, data=data, headers=h, method="POST")
    try:
        with opener.open(req, timeout=20) as r:
            status = r.status
            raw = r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, []
    events = []
    for block in raw.split("\n\n"):
        if not block.strip():
            continue
        name = data_line = None
        for line in block.splitlines():
            if line.startswith("event:"):
                name = line[len("event:"):].strip()
            elif line.startswith("data:"):
                data_line = line[len("data:"):].strip()
        if name is not None:
            events.append((name, data_line))
    return status, events


def _of_type(events, kind):
    return [d for e, d in events if e == kind]


# ---- config plumbing ------------------------------------------------------------------------

def _write_config(env, tmp_path, llm_yaml):
    cfg_dir = tmp_path / "cfg" / "mgmt"
    cfg_dir.mkdir(parents=True, exist_ok=True)
    (cfg_dir / "config.yaml").write_text(llm_yaml)
    env["XDG_CONFIG_HOME"] = str(tmp_path / "cfg")
    return cfg_dir


def _enable_llm(env, tmp_path, stub, max_tool_rounds=None):
    extra = f"\n  max_tool_rounds: {max_tool_rounds}" if max_tool_rounds else ""
    _write_config(
        env, tmp_path,
        f"llm:\n  enabled: true\n  base_url: '{stub.base_url}'\n  model: 'test-model'{extra}\n",
    )


def _password_login(cli, mgmt_bin, env, data_dir, srv, password="s3cret"):
    env["MGMT_WEB_PASSWORD"] = password
    assert cli("web", "setpass").returncode == 0
    op = _opener()
    assert _post(op, f"{srv.base}/api/auth/login", {"password": password})[0] == 200
    return op


def _seed_event(op, base, calendar, summary, start, end):
    status, body = _post(op, f"{base}/api/events", {
        "uid": "", "calendar": calendar, "summary": summary, "start": start, "end": end,
    })
    assert status == 200, body
    return json.loads(body)["uid"]


def _ics_files(data_dir, user, calendar):
    return list((data_dir / "users" / user / "calendars" / calendar).glob("*.ics")) if \
        (data_dir / "users" / user / "calendars" / calendar).exists() else []


# ---- tests ---------------------------------------------------------------------------------

def test_chat_config_and_404_when_disabled(mgmt_bin, env, data_dir, tmp_path):
    # No `llm:` section at all -> the feature probe reports disabled, and the endpoint 404s.
    _write_config(env, tmp_path, "statuses: []\n")
    with Server(mgmt_bin, env, data_dir) as srv:
        op = _opener()
        status, body = _get(op, f"{srv.base}/api/chat/config")
        assert status == 200
        assert json.loads(body)["enabled"] is False

        status, body = _post(op, f"{srv.base}/api/chat", {"messages": [{"role": "user", "content": "hi"}]})
        assert status == 404


def test_chat_401_unauthenticated_even_when_enabled(cli, mgmt_bin, env, data_dir, tmp_path):
    stub = Stub([])
    try:
        _enable_llm(env, tmp_path, stub)
        env["MGMT_WEB_PASSWORD"] = "s3cret"
        assert cli("web", "setpass").returncode == 0
        with Server(mgmt_bin, env, data_dir) as srv:
            op = _opener()  # never logged in
            status, _ = _post(op, f"{srv.base}/api/chat", {"messages": [{"role": "user", "content": "hi"}]})
            assert status == 401
        # The guard must reject before any upstream call is attempted.
        assert stub.requests == 0
    finally:
        stub.close()


def test_chat_one_round_create_writes_event_into_vault(cli, mgmt_bin, env, data_dir, tmp_path):
    create_args = {
        "calendar": "default", "summary": "standup",
        "start": "2026-10-12T09:00:00Z", "end": "2026-10-12T09:30:00Z",
    }
    stub = Stub([_tool_round("create_event", create_args), _text_round("done, created the standup.")])
    try:
        _enable_llm(env, tmp_path, stub)
        with Server(mgmt_bin, env, data_dir) as srv:
            op = _password_login(cli, mgmt_bin, env, data_dir, srv)
            status, events = _post_sse(
                op, f"{srv.base}/api/chat", {"messages": [{"role": "user", "content": "add a standup tomorrow at 9"}]}
            )
        assert status == 200
        tools = [json.loads(d) for d in _of_type(events, "tool")]
        assert len(tools) == 1, events
        assert tools[0]["name"] == "create_event"
        assert tools[0]["ok"] is True
        assert tools[0]["undo"] is True
        assert "standup" in tools[0]["summary"]
        assert "".join(_of_type(events, "token")) == "done, created the standup."
        assert len(_of_type(events, "done")) == 1
        assert _of_type(events, "error") == []

        files = _ics_files(data_dir, "admin", "default")
        assert len(files) == 1, "the event must be written to disk, not just reported"
        assert "SUMMARY:standup" in files[0].read_text()
        assert stub.requests == 2  # the create round, then the round that produced plain text
    finally:
        stub.close()


def test_chat_round_cap_ends_with_error_and_stops_calling_upstream(cli, mgmt_bin, env, data_dir, tmp_path):
    args = {"from": "2026-01-01T00:00:00Z", "to": "2026-01-02T00:00:00Z"}
    stub = Stub([_tool_round("list_events", args), _tool_round("list_events", args)])
    try:
        _enable_llm(env, tmp_path, stub, max_tool_rounds=2)
        with Server(mgmt_bin, env, data_dir) as srv:
            op = _password_login(cli, mgmt_bin, env, data_dir, srv)
            status, events = _post_sse(
                op, f"{srv.base}/api/chat", {"messages": [{"role": "user", "content": "list everything forever"}]}
            )
        assert status == 200
        errors = _of_type(events, "error")
        assert len(errors) == 1, events
        assert "2" in errors[0] and "round" in errors[0]
        assert _of_type(events, "done") == []
        # Exactly the two allowed rounds were made — a third would have drained the (now empty)
        # queue and returned the fallback `[DONE]`, which the assertion below rules out.
        assert stub.requests == 2
    finally:
        stub.close()


def test_chat_delete_confirms_then_approve_deletes(cli, mgmt_bin, env, data_dir, tmp_path):
    stub = Stub([])
    try:
        _enable_llm(env, tmp_path, stub)
        with Server(mgmt_bin, env, data_dir) as srv:
            op = _password_login(cli, mgmt_bin, env, data_dir, srv)
            uid = _seed_event(op, srv.base, "default", "standup", "2026-10-12T09:00:00Z", "2026-10-12T09:30:00Z")
            assert len(_ics_files(data_dir, "admin", "default")) == 1

            delete_args = {"uid": uid}
            stub.push(_tool_round("delete_event", delete_args))
            messages = [{"role": "user", "content": "delete the standup"}]
            status, events = _post_sse(op, f"{srv.base}/api/chat", {"messages": messages})
            assert status == 200
            confirms = [json.loads(d) for d in _of_type(events, "confirm")]
            assert len(confirms) == 1, events
            assert confirms[0]["name"] == "delete_event"
            assert "standup" in confirms[0]["summary"]
            assert _of_type(events, "tool") == []  # not executed yet
            assert stub.requests == 1  # the loop stopped at the confirm, no further round

            # The event must still exist — a confirm must never delete on its own.
            assert len(_ics_files(data_dir, "admin", "default")) == 1

            # Re-POST the identical conversation with `approve` set to that call id.
            call_id = confirms[0]["id"]
            stub.push(_tool_round("delete_event", delete_args), _text_round("deleted."))
            status, events = _post_sse(op, f"{srv.base}/api/chat", {"messages": messages, "approve": call_id})
            assert status == 200
            tools = [json.loads(d) for d in _of_type(events, "tool")]
            assert len(tools) == 1, events
            assert tools[0]["name"] == "delete_event"
            assert tools[0]["ok"] is True
            assert len(_of_type(events, "done")) == 1

            assert _ics_files(data_dir, "admin", "default") == []
            assert stub.requests == 3
    finally:
        stub.close()


def test_chat_bad_tool_arguments_become_a_tool_result_not_a_500(cli, mgmt_bin, env, data_dir, tmp_path):
    # `list_events` requires `from`/`to`; an empty args object fails to deserialize.
    stub = Stub([_tool_round("list_events", {}), _text_round("sorry, I need a date range.")])
    try:
        _enable_llm(env, tmp_path, stub)
        with Server(mgmt_bin, env, data_dir) as srv:
            op = _password_login(cli, mgmt_bin, env, data_dir, srv)
            status, events = _post_sse(
                op, f"{srv.base}/api/chat", {"messages": [{"role": "user", "content": "what's on today"}]}
            )
        assert status == 200
        tools = [json.loads(d) for d in _of_type(events, "tool")]
        assert len(tools) == 1, events
        assert tools[0]["ok"] is False
        assert "invalid arguments" in tools[0]["summary"]
        # The loop must recover, not abort: it still reaches a normal end.
        assert len(_of_type(events, "done")) == 1
        assert _of_type(events, "error") == []
        # No vault mutation happened.
        assert _ics_files(data_dir, "admin", "default") == []
    finally:
        stub.close()


def test_chat_vault_isolation_between_users(cli, mgmt_bin, env, data_dir, tmp_path):
    create_args = {
        "calendar": "default", "summary": "bob's meeting",
        "start": "2026-10-13T10:00:00Z", "end": "2026-10-13T10:30:00Z",
    }
    stub = Stub([_tool_round("create_event", create_args), _text_round("done.")])
    try:
        _enable_llm(env, tmp_path, stub)
        with Server(mgmt_bin, env, data_dir) as srv:
            admin_op = _password_login(cli, mgmt_bin, env, data_dir, srv)

            status, body = _post(admin_op, f"{srv.base}/api/admin/users", {"id": "bob", "name": "Bob", "email": "bob@example.com"})
            assert status == 201, body
            status, body = _post(admin_op, f"{srv.base}/api/admin/users/bob/tokens", {"name": "t1"})
            assert status == 200, body
            token = json.loads(body)["token"]

            bob_op = _opener()  # a fresh client; no admin session cookie rides along
            status, events = _post_sse(
                bob_op, f"{srv.base}/api/chat",
                {"messages": [{"role": "user", "content": "add a meeting"}]},
                headers={"Authorization": f"Bearer {token}"},
            )
            assert status == 200
            assert len(_of_type(events, "tool")) == 1, events

        # Bob's vault got the event; the admin vault was never touched.
        assert _ics_files(data_dir, "admin", "default") == []
        bob_files = _ics_files(data_dir, "bob", "default")
        assert len(bob_files) == 1
        assert "bob" in bob_files[0].read_text().lower()
    finally:
        stub.close()


if __name__ == "__main__":
    import pytest
    raise SystemExit(pytest.main([__file__, "-q"]))
