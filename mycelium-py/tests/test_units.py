"""
Unit files from an SDK agent (design-time-tooling.md Q2). A stub answers with the JSON
`/gateway/units/declare` emits. What matters: the file goes out as text (the node parses it), the
handle retracts the whole unit, and a refusal surfaces.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest

from mycelium import MyceliumAgent, UnitHandle


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    status: int = 200
    reply: dict = {}
    seen: list = []

    def _respond(self) -> None:
        data = json.dumps(_Stub.reply).encode()
        self.send_response(_Stub.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self) -> None:  # noqa: N802
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)) or 0)
        _Stub.seen.append(("POST", self.path, json.loads(raw or b"{}")))
        self._respond()

    def do_DELETE(self) -> None:  # noqa: N802
        _Stub.seen.append(("DELETE", self.path, None))
        self._respond()

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def port():
    _Stub.seen = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


UNIT = 'principal = "sdk-planner"\n[[requirement]]\nns = "data"\nname = "realtime"\n'


def test_declare_from_posts_the_file_as_text_and_the_handle_retracts_it(port, tmp_path):
    path = tmp_path / "planner.toml"
    path.write_text(UNIT)
    _Stub.status = 200
    _Stub.reply = {"handle_id": "h1", "principal": "sdk-planner",
                   "declared": {"capabilities": 0, "requirements": 1, "groups": 0}, "not_enforced": []}
    h = MyceliumAgent("127.0.0.1", port).declare_from(str(path), lease_secs=10)
    assert isinstance(h, UnitHandle)
    assert (h.handle_id, h.principal, h.declared["requirements"]) == ("h1", "sdk-planner", 1)
    method, route, body = _Stub.seen[-1]
    assert (method, route) == ("POST", "/gateway/units/declare")
    assert body == {"toml": UNIT, "interval_secs": 30, "lease_secs": 10}, "the file travels as text"
    _Stub.reply = {"ok": True}
    h.drop()
    assert _Stub.seen[-1][:2] == ("DELETE", "/gateway/capability/h1")


def test_a_hosting_section_is_refused(port):
    _Stub.status, _Stub.reply = 422, {"error": "hosting sections", "detail": "[hosts] belong to a stem"}
    with pytest.raises(httpx.HTTPStatusError) as e:
        MyceliumAgent("127.0.0.1", port).declare_units("[hosts]\nkinds = [\"blob\"]\n")
    assert e.value.response.status_code == 422
