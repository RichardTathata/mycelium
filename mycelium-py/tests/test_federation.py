"""
The federation verbs (item 2 row 11). A stub answers with the JSON shapes
`/gateway/federation/*` emits, so no node is needed.

What is worth testing here is not the happy path — it is that a refusal keeps its two
load-bearing fields on the way through the SDK. A `delivery: unknown` that arrives as a
generic exception is an invitation to retry an effect that may already have run, which is
exactly what the receipt vocabulary exists to prevent.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import DeliveryUnknown, FederationError, MyceliumAgent


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

    def do_GET(self) -> None:  # noqa: N802
        _Stub.seen.append(("GET", self.path, None))
        self._respond()

    def do_POST(self) -> None:  # noqa: N802
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)) or 0)
        _Stub.seen.append(("POST", self.path, json.loads(raw or b"{}")))
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


def fed(port: int):
    return MyceliumAgent("127.0.0.1", port).federation()


def test_read_verbs_hit_the_documented_routes(port):
    _Stub.status, _Stub.reply = 200, {"partners": [{"domain": "b.example", "link": "ready", "last_catalogue": ["x"]}]}
    rows = fed(port).partners()
    assert rows[0]["link"] == "ready"
    assert _Stub.seen[-1][:2] == ("GET", "/gateway/federation/partners")

    _Stub.reply = {"configured": True, "domain": "a.example", "exports": ["x"], "policy_revision": 3}
    assert fed(port).domain()["policy_revision"] == 3
    assert _Stub.seen[-1][:2] == ("GET", "/gateway/federation/domain")

    _Stub.reply = {"domain": "b.example", "link": "ready", "observed": True, "exports": ["x"]}
    assert fed(port).catalog("b.example")["observed"] is True
    assert _Stub.seen[-1][:2] == ("GET", "/gateway/federation/catalog/b.example")


def test_call_sends_repeatability_and_returns_the_reply(port):
    _Stub.status, _Stub.reply = 200, {"reply": "ok", "sent": True, "delivery": "completed"}
    assert fed(port).call("b.example", "invoice.status", "INV-42") == "ok"
    method, path, body = _Stub.seen[-1]
    assert (method, path) == ("POST", "/gateway/federation/call")
    # The default is the safe one: an unstated effect is never retried elsewhere.
    assert body == {"domain": "b.example", "export": "invoice.status", "text": "INV-42", "repeatable": False}

    fed(port).call("b.example", "invoice.status", "INV-42", repeatable=True)
    assert _Stub.seen[-1][2]["repeatable"] is True


def test_a_local_refusal_says_nothing_was_sent(port):
    """The link was down: the gateway refused before any byte crossed, so a retry is safe."""
    _Stub.status = 409
    _Stub.reply = {"error": "link", "detail": "link: Down", "sent": False, "delivery": "none"}
    with pytest.raises(FederationError) as e:
        fed(port).call("b.example", "invoice.status")
    assert e.value.kind == "link"
    assert e.value.nothing_was_sent is True
    assert e.value.delivery == "none"
    assert not isinstance(e.value, DeliveryUnknown)


def test_delivery_unknown_is_its_own_type(port):
    """A silent gateway is not a failure. It is *we cannot say*, and the SDK must not let a
    caller mistake the two — hence a distinct exception rather than a field to remember."""
    _Stub.status = 504
    _Stub.reply = {
        "error": "delivery-unknown", "detail": "outcome: DeliveryUnknown", "sent": True,
        "delivery": "unknown", "attempted_via": ["gw-1", "gw-2"],
    }
    with pytest.raises(DeliveryUnknown) as e:
        fed(port).call("b.example", "invoice.status")
    assert e.value.attempted_via == ["gw-1", "gw-2"]
    assert e.value.nothing_was_sent is False


def test_a_partner_refusal_is_not_delivery_unknown(port):
    """The partner answered and said no: it ran nothing, and that *is* knowable."""
    _Stub.status = 502
    _Stub.reply = {"error": "refused", "detail": "refused (403)", "sent": True, "delivery": "refused",
                   "partner_status": 403}
    with pytest.raises(FederationError) as e:
        fed(port).call("b.example", "invoice.status")
    assert not isinstance(e.value, DeliveryUnknown)
    assert e.value.body["partner_status"] == 403


def test_a_body_that_is_not_json_still_raises_a_typed_error(port):
    """A proxy in the path can answer HTML. The SDK must not turn that into a success, and
    must not crash while reporting it."""
    class _Html(_Stub):
        def _respond(self) -> None:
            data = b"<html>502</html>"
            self.send_response(502)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _Html)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with pytest.raises(FederationError) as e:
            fed(server.server_address[1]).call("b.example", "invoice.status")
        assert e.value.delivery == "unknown", "an unreadable refusal fails closed"
    finally:
        server.shutdown()


@pytest.mark.asyncio
async def test_async_variants_reach_the_same_routes(port):
    _Stub.status, _Stub.reply = 200, {"exports": ["x"], "link": "ready"}
    assert await fed(port).aconnect("b.example") == ["x"]
    assert _Stub.seen[-1][:2] == ("POST", "/gateway/federation/connect")

    _Stub.reply = {"reply": "ok"}
    assert await fed(port).acall("b.example", "invoice.status") == "ok"
    assert _Stub.seen[-1][:2] == ("POST", "/gateway/federation/call")
