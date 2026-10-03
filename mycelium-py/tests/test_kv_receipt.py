"""
`set()` returns the receipt `POST /gateway/kv` has answered since v2.16.0 — rung 1 always, rung 2 as
`local_durability` — instead of discarding it (zero-gaps Z7; the doc-coverage matrix carried the gap
since run 18). No node needed: a stub answers with the JSON the gateway emits.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import KvReceipt, MyceliumAgent


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    reply: dict = {"ok": True}

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


@pytest.mark.parametrize("reply,operation_id,durability,error", [
    ({"ok": True, "operation_id": "op-7f3a", "local_durability": "on_disk"}, "op-7f3a", "on_disk", None),
    ({"ok": True, "operation_id": "op-7f3b", "local_durability": "failed",
      "local_durability_error": "the WAL append did not acknowledge"},
     "op-7f3b", "failed", "the WAL append did not acknowledge"),
    ({"ok": True}, None, None, None),   # a pre-v2.16.0 gateway: a bare ok
])
def test_set_returns_the_receipt(port, reply, operation_id, durability, error):
    _Stub.reply = reply
    with MyceliumAgent("127.0.0.1", port) as agent:
        r = agent.set("k", b"v")
    assert isinstance(r, KvReceipt)
    assert r.operation_id == operation_id
    assert r.local_durability == durability
    assert r.local_durability_error == error
    assert r.on_disk is (durability == "on_disk")
