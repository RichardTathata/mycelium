"""The supervisor's grant verbs (adopter-surface X3's door; X7's *grants* cell): ``grants_pending`` reads
``GET /gateway/grants/pending`` and ``grant_decide`` posts ``{decision, ttl_secs?, reason?}`` to
``POST /gateway/grants/{agent}/{request}``, each under the bearer. No node: a stub records what the SDK sent.
"""
from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest

from mycelium import MyceliumAgent

PENDING = [{"key": "grant/node:127.0.0.1:7946/r1", "agent": "node:127.0.0.1:7946", "request": "r1", "tool": "purchase.place",
            "arguments_digest": "ab" * 32, "policy_revision": "rev-1", "requested_at_ms": 1, "expires_at_ms": 2}]


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[str, str, dict, str | None]] = []
    status = 200

    def _json(self, body: dict, status: int = 200) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        _Stub.seen.append(("GET", self.path, {}, self.headers.get("Authorization")))
        self._json({"ok": True, "pending": PENDING, "now_ms": 1})

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        _Stub.seen.append(("POST", self.path, body, self.headers.get("Authorization")))
        if _Stub.status != 200:
            self._json({"ok": False, "error": "not_this_node", "detail": "another node's agent"}, _Stub.status)
            return
        self._json({"ok": True, "key": "grant/a/r1", "operation_id": "op-1", "local_durability": "on_disk"})

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    _Stub.seen, _Stub.status = [], 200
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def test_grants_pending_reads_the_route_under_the_bearer(stub):
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    rows = agent.grants_pending()
    assert rows == PENDING
    assert _Stub.seen == [("GET", "/gateway/grants/pending", {}, "Bearer super")]


def test_grant_decide_posts_the_decision_and_returns_the_records_receipt(stub):
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    answer = agent.grant_decide("node:127.0.0.1:7946", "r1", "issue", ttl_secs=120)
    assert answer["ok"] is True and answer["local_durability"] == "on_disk"
    refusal = agent.grant_decide("node:127.0.0.1:7946", "r2", "refuse", reason="not today")
    assert refusal["ok"] is True
    assert [(m, p, b) for m, p, b, _ in _Stub.seen] == [
        ("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r1", {"decision": "issue", "ttl_secs": 120}),
        ("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r2", {"decision": "refuse", "reason": "not today"}),
    ]
    assert all(auth == "Bearer super" for *_, auth in _Stub.seen)


def test_a_bad_decision_is_refused_before_it_is_sent_and_a_refusal_surfaces(stub):
    agent = MyceliumAgent("127.0.0.1", stub)
    with pytest.raises(ValueError):
        agent.grant_decide("a", "r", "maybe")
    assert _Stub.seen == []
    _Stub.status = 409
    with pytest.raises(httpx.HTTPStatusError) as e:
        agent.grant_decide("node:other", "r1", "issue")
    assert e.value.response.status_code == 409
    assert e.value.response.json()["error"] == "not_this_node"
