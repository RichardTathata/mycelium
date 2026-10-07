"""
`MyceliumAgent.node_id` — this node's id, as the TypeScript SDK's `nodeId` gives it (doc-coverage run 20:
the README's leader-election example read `agent.node_id`, which did not exist, so it raised
AttributeError). Read once from `GET /health` and cached. No node needed: a stub answers `/health`.
"""
from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent


class _Health(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    calls = 0

    def do_GET(self) -> None:  # noqa: N802
        _Health.calls += 1
        data = json.dumps({"status": "ok", "node_id": "127.0.0.1:9302"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def port():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Health)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def test_node_id_is_this_nodes_id_and_is_read_once(port):
    _Health.calls = 0
    agent = MyceliumAgent("127.0.0.1", port)
    assert agent.node_id == "127.0.0.1:9302"
    assert agent.node_id == "127.0.0.1:9302"
    assert _Health.calls == 1, "cached after the first read"
