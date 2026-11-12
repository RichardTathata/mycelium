"""
Since the gateway's `kv:write` decision (#549) the raw KV routes — ``POST``/``DELETE /gateway/kv``,
``POST /gateway/kv/quorum``, ``POST /gateway/overlay/consistent/set`` — refuse a key in a namespace the
substrate or a companion owns with ``403 protected_key``, and the log routes (``append``, ``compact``)
refuse a stream under ``cn/``, ``wiki/`` or ``reason/`` with ``403 protected_stream``. Each refusal's
``message`` names the route to use. The SDK raises :class:`ProtectedKeyError` /
:class:`ProtectedStreamError` carrying it, from every verb that reaches those routes. Both are still a
``PermissionError`` and an ``httpx.HTTPStatusError`` (what these verbs raised before), so existing
``except`` clauses keep catching them. No node needed: a stub answers with the gateway's shapes.
"""

from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest

from mycelium import MyceliumAgent, ProtectedKeyError, ProtectedKindError, ProtectedStreamError


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    status: int = 403
    reply: dict = {}
    seen: list = []

    def _answer(self) -> None:
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""
        _Stub.seen.append((self.command, self.path, json.loads(body or b"{}")))
        data = json.dumps(_Stub.reply).encode()
        self.send_response(_Stub.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_POST = _answer  # noqa: N815
    do_DELETE = _answer  # noqa: N815
    do_GET = _answer  # noqa: N815

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def agent():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Stub.seen = []
    try:
        yield MyceliumAgent("127.0.0.1", server.server_address[1])
    finally:
        server.shutdown()


KEY_DOOR = "prompt templates are written through /gateway/prompts/{ns}/{name} (llm:write)"
STREAM_DOOR = "this log stream belongs to a component (commitment, wiki, reason) and is written through it"


def _refuse_key() -> None:
    _Stub.status = 403
    _Stub.reply = {"ok": False, "error": "protected_key", "message": KEY_DOOR}


def _refuse_stream() -> None:
    _Stub.status = 403
    _Stub.reply = {"ok": False, "error": "protected_stream", "message": STREAM_DOOR}


@pytest.mark.parametrize("verb", ["set", "delete", "set_with_min_acks", "consistent_set"])
def test_every_kv_door_raises_protected_key_error_with_the_route(agent, verb):
    _refuse_key()
    call = {
        "set":               lambda: agent.set("prompts/ai/chat", b"x"),
        "delete":            lambda: agent.delete("prompts/ai/chat"),
        "set_with_min_acks": lambda: agent.set_with_min_acks("prompts/ai/chat", b"x", 1),
        "consistent_set":    lambda: agent.consistent_set("prompts/ai/chat", b"x"),
    }[verb]
    with pytest.raises(ProtectedKeyError) as e:
        call()
    assert e.value.key == "prompts/ai/chat"
    assert e.value.message == KEY_DOOR
    assert "/gateway/prompts/" in str(e.value)
    # Existing handlers keep catching it: a PermissionError, and the HTTP error these verbs raised before.
    assert isinstance(e.value, PermissionError)
    assert isinstance(e.value, httpx.HTTPStatusError)
    assert e.value.response.status_code == 403


@pytest.mark.parametrize("verb", ["append", "compact_log"])
def test_both_log_doors_raise_protected_stream_error_with_the_message(agent, verb):
    _refuse_stream()
    call = {
        "append":      lambda: agent.append("reason/trace", b"x"),
        "compact_log": lambda: agent.compact_log("reason/trace", 10),
    }[verb]
    with pytest.raises(ProtectedStreamError) as e:
        call()
    assert e.value.stream == "reason/trace"
    assert e.value.message == STREAM_DOOR
    assert isinstance(e.value, PermissionError)
    assert isinstance(e.value, httpx.HTTPStatusError)


def test_the_kinds_are_told_apart(agent):
    """A protected-key refusal is not a protected-kind one, and vice versa."""
    _refuse_key()
    with pytest.raises(ProtectedKeyError) as e:
        agent.set("grp/x/y", b"x")
    assert not isinstance(e.value, (ProtectedKindError, ProtectedStreamError))


@pytest.mark.parametrize("verb", ["set", "delete", "append"])
def test_an_ordinary_403_is_not_mistaken_for_it(agent, verb):
    """The plant: a scope refusal stays the HTTP error it was."""
    _Stub.status = 403
    _Stub.reply = {"error": "insufficient scope", "required_scope": "kv:write"}
    call = {
        "set":    lambda: agent.set("app/k", b"x"),
        "delete": lambda: agent.delete("app/k"),
        "append": lambda: agent.append("app", b"x"),
    }[verb]
    with pytest.raises(httpx.HTTPStatusError) as e:
        call()
    assert not isinstance(e.value, (ProtectedKeyError, ProtectedStreamError))


# The kv:read decision (2026-11-12, 0.2.16): the raw KV read routes refuse an owned key as the write routes do.
READ_DOOR = "grant requests are read through GET /gateway/grants/pending and GET /gateway/grants/request/{request} (grant:read)"

READ_VERBS = {
    "get":            (lambda a: a.get("grant/a/r1"), "grant/a/r1"),
    "consistent_get": (lambda a: a.consistent_get("grant/a/r1"), "grant/a/r1"),
    "keys":           (lambda a: a.keys(prefix="grant/"), "grant/"),
    "scan_prefix":    (lambda a: a.scan_prefix("grant/"), "grant/"),
}


@pytest.mark.parametrize("verb", sorted(READ_VERBS))
def test_every_kv_read_door_raises_protected_key_error_with_the_route(agent, verb):
    _Stub.status = 403
    _Stub.reply = {"ok": False, "error": "protected_key", "message": READ_DOOR}
    call, key = READ_VERBS[verb]
    with pytest.raises(ProtectedKeyError) as e:
        call(agent)
    assert _Stub.seen[-1][0] == "GET"
    assert e.value.key == key
    assert e.value.message == READ_DOOR
    assert isinstance(e.value, PermissionError)
    assert isinstance(e.value, httpx.HTTPStatusError)
    assert e.value.response.status_code == 403


@pytest.mark.parametrize("verb", sorted(READ_VERBS))
def test_an_ordinary_403_on_a_read_is_not_mistaken_for_it(agent, verb):
    """The plant: a scope refusal stays the HTTP error it was."""
    _Stub.status = 403
    _Stub.reply = {"error": "insufficient scope", "required_scope": "kv:read"}
    with pytest.raises(httpx.HTTPStatusError) as e:
        READ_VERBS[verb][0](agent)
    assert not isinstance(e.value, ProtectedKeyError)


def test_schedule_last_reads_the_schedules_own_route(agent):
    _Stub.status = 200
    _Stub.reply = {"schedule": "close", "last": [{"node": "10.0.0.5:9000", "slot": 7, "period_ms": 1000, "phase_ms": 0}]}
    assert agent.schedule_last("close") == [{"node": "10.0.0.5:9000", "slot": 7, "period_ms": 1000, "phase_ms": 0}]
    method, path, _ = _Stub.seen[-1]
    assert (method, path) == ("GET", "/gateway/schedules/last?id=close")


# CR1's follow-up (2026-11-12, 0.2.16): the log read routes refuse a component-owned stream as append and
# compact do, and the three read verbs raise the typed error — the subscriptions before any entry.
LOG_READ_DOOR = "a reasoning trace is read through GET /gateway/reason/trace/{run_id} (llm:read), where mycelium-reason's routes are merged"


async def _drain(gen) -> list:
    return [entry async for entry in gen]


LOG_READ_VERBS = {
    "scan_log":            (lambda a: a.scan_log("reason/run-1/n"), "/gateway/overlay/log/scan"),
    "subscribe_log":       (lambda a: asyncio.run(_drain(a.subscribe_log("reason/run-1/n"))), "/gateway/overlay/log/subscribe"),
    "subscribe_log_group": (lambda a: asyncio.run(_drain(a.subscribe_log_group("reason/run-1/n", "g"))),
                            "/gateway/overlay/log/group/subscribe"),
}


@pytest.mark.parametrize("verb", sorted(LOG_READ_VERBS))
def test_every_log_read_door_raises_protected_stream_error_with_the_route(agent, verb):
    _Stub.status = 403
    _Stub.reply = {"ok": False, "error": "protected_stream", "message": LOG_READ_DOOR}
    call, path = LOG_READ_VERBS[verb]
    with pytest.raises(ProtectedStreamError) as e:
        call(agent)
    method, seen_path, _ = _Stub.seen[-1]
    assert method == "GET" and seen_path.startswith(path + "?stream=reason")
    assert e.value.stream == "reason/run-1/n"
    assert e.value.message == LOG_READ_DOOR
    assert "/gateway/reason/trace/" in str(e.value)
    assert isinstance(e.value, PermissionError)
    assert isinstance(e.value, httpx.HTTPStatusError)
    assert e.value.response.status_code == 403


@pytest.mark.parametrize("verb", sorted(LOG_READ_VERBS))
def test_an_ordinary_403_on_a_log_read_is_not_mistaken_for_it(agent, verb):
    """The plant: a scope refusal stays an HTTP error, on the subscriptions too (never an SSE content-type error)."""
    _Stub.status = 403
    _Stub.reply = {"error": "insufficient scope", "required_scope": "consensus:read"}
    with pytest.raises(httpx.HTTPStatusError) as e:
        LOG_READ_VERBS[verb][0](agent)
    assert not isinstance(e.value, ProtectedStreamError)
    assert e.value.response.status_code == 403
