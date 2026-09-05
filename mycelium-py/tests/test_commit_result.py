"""
`consistent_set` / `cross_group_propose` surface the gateway's `persisted` field (v2.4.2:
whether the committed slot reached the gateway node's own disk). No node needed — a stub
answers with the JSON shapes the gateway emits.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import CommitResult, MyceliumAgent


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    reply: dict = {"ok": True, "persisted": True}

    def do_POST(self) -> None:  # noqa: N802
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        data = json.dumps(_Stub.reply).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def port():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


GROUPS = [{"group": "a", "quorum": 0.5}]


@pytest.mark.parametrize("reply,expected", [
    ({"ok": True, "persisted": True}, True),
    ({"ok": True, "persisted": False}, False),
    ({"ok": True}, None),                       # pre-v2.4.2 gateway: field absent
])
def test_persisted_is_surfaced(port, reply, expected):
    _Stub.reply = reply
    with MyceliumAgent("127.0.0.1", port) as agent:
        r1 = agent.consistent_set("k", b"v")
        r2 = agent.cross_group_propose("slot", b"v", GROUPS)
    for r in (r1, r2):
        assert isinstance(r, CommitResult)
        assert r.persisted is expected
        assert bool(r) is (expected is True)


def test_failed_commit_still_raises(port):
    _Stub.reply = {"ok": False, "error": "superseded"}
    with MyceliumAgent("127.0.0.1", port) as agent:
        with pytest.raises(RuntimeError, match="superseded"):
            agent.consistent_set("k", b"v")
        with pytest.raises(RuntimeError):
            agent.cross_group_propose("slot", b"v", GROUPS)
