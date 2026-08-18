"""Connection-reuse regression gate (no Mycelium node needed).

The bridge once opened a fresh TCP connection per call, which exhausts macOS
ephemeral ports at Group-scale write rates (~16k rapid KV calls, found by a
downstream project 2026-08-18). These tests drive the public client surface
against a local keep-alive stub server that counts *distinct TCP connections*:
with the pooled persistent client, hundreds of calls must ride a handful of
connections. Before the fix this count equalled the call count.
"""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent
from mycelium.wiki import Wiki


class _CountingHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"  # keep-alive

    def _reply(self, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        self._reply({"found": False, "keys": []})

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", 0))
        self.rfile.read(length)
        self._reply({"ok": True, "page": None})

    def log_message(self, *_: object) -> None:  # quiet
        pass


class _CountingServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.connections = 0
        self._lock = threading.Lock()

    def get_request(self):
        sock, addr = super().get_request()
        with self._lock:
            self.connections += 1
        return sock, addr


@pytest.fixture
def stub():
    server = _CountingServer(("127.0.0.1", 0), _CountingHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    try:
        yield server
    finally:
        server.shutdown()


def test_sync_kv_calls_reuse_connections(stub):
    port = stub.server_address[1]
    with MyceliumAgent("127.0.0.1", port) as agent:
        for i in range(200):
            agent.get(f"k/{i}")
            agent.set(f"k/{i}", b"v")
    assert stub.connections <= 5, (
        f"400 KV calls used {stub.connections} TCP connections — "
        "the per-call-client regression is back (ephemeral-port exhaustion)"
    )


def test_async_calls_reuse_connections_across_event_loops(stub):
    port = stub.server_address[1]
    wiki = Wiki("127.0.0.1", port)

    async def burst():
        for _ in range(50):
            await wiki.read("page")

    # Two separate asyncio.run() loops against one handle: the pool must both
    # reuse connections within a loop AND survive the loop change.
    asyncio.run(burst())
    asyncio.run(burst())
    assert stub.connections <= 6, (
        f"100 async reads across two loops used {stub.connections} connections"
    )
