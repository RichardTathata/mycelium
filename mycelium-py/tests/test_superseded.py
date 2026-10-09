"""A consensus write that lost raises :class:`SupersededError`, not a bare HTTP error (substrate 2.30.0+).

``consistent_set`` and ``cross_group_propose`` answer **409** ``{"ok": false, "error": "superseded"}`` when the
slot was decided for another value — since 2.30.0 the normal answer to a concurrent loser. The SDK raised the
``httpx.HTTPStatusError`` of ``raise_for_status`` for it, which a caller could only tell apart from a timeout
or a topology refusal by reading the body. ``SupersededError`` is still an ``httpx.HTTPStatusError``, so an
existing ``except`` keeps catching it. No node needed: a stub answers with the gateway's shapes.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest

from mycelium import MyceliumAgent, SupersededError


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    status: int = 409
    reply: dict = {}

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", 0))
        self.rfile.read(length)
        data = json.dumps(_Stub.reply).encode()
        self.send_response(_Stub.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def agent():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield MyceliumAgent("127.0.0.1", server.server_address[1])
    finally:
        server.shutdown()


CALLS = {
    "consistent_set":      lambda a: a.consistent_set("config/endpoint", b"x"),
    "cross_group_propose": lambda a: a.cross_group_propose("slot", b"x", [{"group": "g", "quorum": 0.5}]),
}


@pytest.mark.parametrize("verb", list(CALLS))
def test_a_lost_consensus_write_raises_superseded(agent, verb):
    _Stub.status, _Stub.reply = 409, {"ok": False, "error": "superseded"}
    with pytest.raises(SupersededError) as e:
        CALLS[verb](agent)
    # Still the HTTP error these verbs raised before, so existing handlers keep catching it.
    assert isinstance(e.value, httpx.HTTPStatusError)
    assert e.value.response.status_code == 409


@pytest.mark.parametrize("verb", list(CALLS))
def test_other_refusals_stay_http_errors(agent, verb):
    _Stub.status, _Stub.reply = 409, {"ok": False, "error": "topology_unsatisfied"}
    with pytest.raises(httpx.HTTPStatusError) as e:
        CALLS[verb](agent)
    assert not isinstance(e.value, SupersededError)
    _Stub.status, _Stub.reply = 504, {"ok": False, "error": "timeout after 3 ballot(s)"}
    with pytest.raises(httpx.HTTPStatusError) as e:
        CALLS[verb](agent)
    assert not isinstance(e.value, SupersededError)
