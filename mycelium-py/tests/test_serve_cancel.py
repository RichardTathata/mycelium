"""Cancellation on the SDK serve stream (identity-and-lifecycle I4, PR 3b; the maintainer's decision (C) of
2026-11-16). No node needed: a stub streams the events a node sends and records what arrived.

* **The declaration is the subscriber's own.** ``rpc_serve(kind, stops_on_cancel=True)`` opens the stream with
  ``stops_on_cancel=true``; every stream says it reads ``cancel`` events (``cancel_events=true``), and an undeclared
  one makes no promise.
* **A cancel reaches the request it names.** The node's ``cancel`` event sets that request's ``cancelled`` — and, on a
  declaring stream, while the caller still holds the request (the stream is read beside it). A ``cancel`` event is
  never yielded as a request.
* **The confirmation** (the maintainer's decisions of 2026-11-17). ``rpc_confirm_stopped`` sends ``{"nonce_hex",
  "sender", "stopped": true, "stream"}`` — the stream the request came on — and a ``409`` is ``False``. It raises
  :class:`CancelNotReceivedError`, sending nothing, unless that stream received a ``cancel`` event for the request: the
  SDK never claims a stop it was not asked for.
"""

from __future__ import annotations

import asyncio
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import CancelNotReceivedError, MyceliumAgent

REQ = {"nonce_hex": "00000000000000aa", "sender": "127.0.0.1:7001", "payload_b64": "d29yaw==", "stream": "7"}
OTHER = {"nonce_hex": "00000000000000bb", "sender": "127.0.0.1:7001", "payload_b64": "d29yaw==", "stream": "7"}
CANCEL = {"cancel": True, "nonce_hex": REQ["nonce_hex"], "sender": REQ["sender"], "reason": "cancelled", "stream": "7"}


class _Node(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"
    seen: list[tuple[str, str, dict]] = []
    respond_status = 200
    burst = 0

    def do_GET(self) -> None:  # noqa: N802
        _Node.seen.append(("GET", self.path, {}))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def send(event: str, data: dict) -> None:
            self.wfile.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())
            self.wfile.flush()

        if _Node.burst:
            # The request in hand, more behind it, then the cancel of the first.
            send("depot.custom", REQ)
            for i in range(1, _Node.burst):
                send("depot.custom", dict(REQ, nonce_hex=f"{i:016x}"))
            time.sleep(0.2)
            send("cancel", CANCEL)
            time.sleep(1.0)
            return
        send("depot.custom", REQ)
        time.sleep(0.2)
        send("cancel", CANCEL)
        send("depot.custom", OTHER)
        time.sleep(0.3)

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", 0) or 0)
        _Node.seen.append(("POST", self.path, json.loads(self.rfile.read(length) or b"{}")))
        body = b'{"ok": true, "stopped": true}' if _Node.respond_status == 200 else b'{"ok": false, "error": "not_cancelled"}'
        self.send_response(_Node.respond_status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def port():
    _Node.seen, _Node.respond_status, _Node.burst = [], 200, 0
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Node)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def _serve_path() -> str:
    return next(p for (m, p, _) in _Node.seen if m == "GET")


def test_a_declaring_stream_says_so_and_an_undeclared_one_does_not(port):
    async def first(**kw):
        async for req in MyceliumAgent("127.0.0.1", port).rpc_serve("depot.custom", **kw):
            return req

    asyncio.run(first(stops_on_cancel=True))
    assert _serve_path() == "/gateway/rpc/serve/depot.custom?cancel_events=true&stops_on_cancel=true&read_ahead=256"
    _Node.seen.clear()
    asyncio.run(first())
    assert _serve_path() == "/gateway/rpc/serve/depot.custom?cancel_events=true"


def test_a_cancel_reaches_the_request_a_declaring_subscriber_still_holds(port):
    async def run() -> tuple[bool, bool, list[str]]:
        stream = MyceliumAgent("127.0.0.1", port).rpc_serve("depot.custom", stops_on_cancel=True)
        req = await stream.__anext__()
        # Not asking for the next request: the stream is read beside us, so the cancel still arrives.
        await asyncio.wait_for(req.cancelled.wait(), timeout=5)
        other = await stream.__anext__()
        await stream.aclose()
        return req.cancelled.is_set(), other.cancelled.is_set(), [req.nonce_hex, other.nonce_hex]

    cancelled, other_cancelled, nonces = asyncio.run(run())
    assert cancelled and not other_cancelled, "the cancel names its own request only"
    assert nonces == [REQ["nonce_hex"], OTHER["nonce_hex"]], "a cancel event is never yielded as a request"


def test_an_undeclared_subscriber_sees_the_cancel_when_it_next_asks(port):
    async def run() -> tuple[bool, bool]:
        stream = MyceliumAgent("127.0.0.1", port).rpc_serve("depot.custom")
        req = await stream.__anext__()
        await asyncio.sleep(0.5)
        before = req.cancelled.is_set()
        other = await stream.__anext__()
        await stream.aclose()
        assert other.nonce_hex == OTHER["nonce_hex"]
        return before, req.cancelled.is_set()

    before, after = asyncio.run(run())
    assert (before, after) == (False, True)


def _cancelled_and_not(agent):
    """The first request once its cancel arrived, and the second (never cancelled)."""
    async def run():
        stream = agent.rpc_serve("depot.custom", stops_on_cancel=True)
        req = await stream.__anext__()
        await asyncio.wait_for(req.cancelled.wait(), timeout=5)
        other = await stream.__anext__()
        await stream.aclose()
        return req, other
    return asyncio.run(run())


def _posts():
    return [b for (m, p, b) in _Node.seen if m == "POST" and p == "/gateway/rpc/respond"]


def test_the_confirmation_names_its_stream_and_a_refusal_is_false(port):
    agent = MyceliumAgent("127.0.0.1", port)
    req, _ = _cancelled_and_not(agent)
    assert agent.rpc_confirm_stopped(req) is True
    assert _posts()[-1] == {"nonce_hex": REQ["nonce_hex"], "sender": REQ["sender"], "stopped": True, "stream": "7"}
    _Node.respond_status = 409
    assert agent.rpc_confirm_stopped(req) is False


def test_the_sdk_never_confirms_a_stop_it_was_not_asked_for(port):
    agent = MyceliumAgent("127.0.0.1", port)
    _, other = _cancelled_and_not(agent)
    # Never cancelled; one built by hand; one whose `cancelled` the caller set itself: refused, nothing sent.
    by_hand = type(other)(kind="depot.custom", nonce_hex=REQ["nonce_hex"], sender=REQ["sender"], payload=b"", stream="7")
    by_hand.cancelled.set()
    for req in (other, by_hand):
        with pytest.raises(CancelNotReceivedError):
            agent.rpc_confirm_stopped(req)
    assert _posts() == []


def test_an_answer_names_its_stream(port):
    agent = MyceliumAgent("127.0.0.1", port)
    _, other = _cancelled_and_not(agent)
    agent.rpc_respond(other, b"done")
    assert _posts()[-1]["stream"] == "7"


def test_a_cancel_reaches_the_request_in_hand_whatever_is_queued_behind_it(port):
    """#806's confirm round, C1: with ``read_ahead=1`` and two more requests already sent, the cancel of the request
    in hand still arrives — the stream is never paused; the node, not the SDK, bounds what the stream is handed."""
    _Node.burst = 3

    async def run() -> bool:
        stream = MyceliumAgent("127.0.0.1", port).rpc_serve("depot.custom", stops_on_cancel=True, read_ahead=1)
        req = await stream.__anext__()
        try:
            await asyncio.wait_for(req.cancelled.wait(), timeout=2)
        except asyncio.TimeoutError:
            return False
        finally:
            await stream.aclose()
        return True

    assert asyncio.run(run()), "the cancel for the request in hand was not received"
    assert _serve_path() == "/gateway/rpc/serve/depot.custom?cancel_events=true&stops_on_cancel=true&read_ahead=1"
