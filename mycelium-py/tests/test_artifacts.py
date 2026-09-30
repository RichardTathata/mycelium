"""
The artifact publish verb (plan A3). A stub answers with the JSON `/gateway/artifacts/publish`
emits, so no node is needed. What matters is that the line goes out untouched and a refusal keeps
the gateway's own name for itself — `untrusted publisher` is not the same event as `unsigned entry`.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import ArtifactError, MyceliumAgent


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    status: int = 200
    reply: dict = {}
    seen: list = []

    def do_POST(self) -> None:  # noqa: N802
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)) or 0)
        _Stub.seen.append(("POST", self.path, json.loads(raw or b"{}"), self.headers.get("Authorization")))
        data = json.dumps(_Stub.reply).encode()
        self.send_response(_Stub.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

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


LINE = "01000102" * 20


def test_publish_posts_the_line_as_is_with_the_bearer_and_returns_the_receipt(port):
    _Stub.status = 200
    _Stub.reply = {"key": "installable/route/optimize/ab", "artifact": "ab", "signer": "ed25519:cd",
                   "kind": "wasm-component", "provides": {"ns": "route", "name": "optimize"}}
    receipt = MyceliumAgent("127.0.0.1", port, token="artpub").artifacts().publish("  " + LINE + "\n")
    assert receipt["key"] == "installable/route/optimize/ab"
    method, path, body, auth = _Stub.seen[-1]
    assert (method, path) == ("POST", "/gateway/artifacts/publish")
    assert body == {"entry_hex": LINE}, "the signed line travels untouched (trimmed only)"
    assert auth == "Bearer artpub"


def test_a_refusal_keeps_the_gateway_s_name_for_it(port):
    _Stub.status = 403
    _Stub.reply = {"error": "untrusted publisher", "detail": "ed25519:cd is not in this node's trusted publishers"}
    with pytest.raises(ArtifactError) as e:
        MyceliumAgent("127.0.0.1", port).artifacts().publish(LINE)
    assert e.value.kind == "untrusted publisher"
    assert e.value.status == 403
    assert "trusted publishers" in e.value.detail

    _Stub.status, _Stub.reply = 403, {"error": "insufficient scope", "required_scope": "artifact:publish"}
    with pytest.raises(ArtifactError) as e:
        MyceliumAgent("127.0.0.1", port).artifacts().publish(LINE)
    assert e.value.kind == "insufficient scope"
    assert e.value.detail == "artifact:publish"


@pytest.mark.asyncio
async def test_the_async_verb_is_the_same_call(port):
    _Stub.status, _Stub.reply = 409, {"error": "librarian-managed signer", "detail": "publish to the library instead"}
    with pytest.raises(ArtifactError) as e:
        await MyceliumAgent("127.0.0.1", port).artifacts().apublish(LINE)
    assert e.value.kind == "librarian-managed signer"
