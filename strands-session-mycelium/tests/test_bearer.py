"""The store's bearer reaches the gateway on every request (adopter-surface X7: the adapter contract's *identity
propagation* cell). The store writes through ``mycelium-py``'s ``MyceliumAgent``; this holds that the token given to
that agent rides every request the store makes — the principal the gateway resolves and records is the token's. No
node: a stub HTTP server records each request's ``Authorization`` header and answers as the KV routes do.
"""
from __future__ import annotations

import base64
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from mycelium import MyceliumAgent
from strands.types.session import Session, SessionType

from strands_session_mycelium import MyceliumSessionRepository


class _Recorder(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[str, str, str | None]] = []
    kv: dict[str, bytes] = {}

    def _json(self, body: dict, status: int = 200) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        _Recorder.seen.append(("GET", self.path, self.headers.get("Authorization")))
        if self.path.startswith("/gateway/kv?"):
            key = self.path.split("key=", 1)[1].split("&")[0]
            from urllib.parse import unquote
            v = _Recorder.kv.get(unquote(key))
            self._json({"found": False} if v is None else {"found": True, "value_b64": base64.b64encode(v).decode()})
            return
        self._json({"found": False, "keys": []})

    def do_POST(self) -> None:  # noqa: N802
        _Recorder.seen.append(("POST", self.path, self.headers.get("Authorization")))
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        if "key" in body and "value_b64" in body:
            _Recorder.kv[body["key"]] = base64.b64decode(body["value_b64"])
        self._json({"ok": True, "operation_id": "op", "local_durability": "on_disk"})

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub():
    _Recorder.seen, _Recorder.kv = [], {}
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def test_the_bearer_rides_every_request_the_store_makes(stub, monkeypatch) -> None:
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    repo = MyceliumSessionRepository(MyceliumAgent("127.0.0.1", stub, token="planner-token"))
    repo.create_session(Session(session_id="s1", session_type=SessionType.AGENT))
    repo.read_session("s1")
    assert _Recorder.seen, "the store reached the gateway"
    methods = {m for m, _, _ in _Recorder.seen}
    assert "POST" in methods and "GET" in methods, _Recorder.seen
    assert all(auth == "Bearer planner-token" for _, _, auth in _Recorder.seen), _Recorder.seen


def test_no_token_sends_no_header(stub, monkeypatch) -> None:
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    repo = MyceliumSessionRepository(MyceliumAgent("127.0.0.1", stub))
    repo.read_session("s2")
    assert _Recorder.seen and all(auth is None for _, _, auth in _Recorder.seen), _Recorder.seen
