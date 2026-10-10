"""The SDK's edges with the gateway (post-360 hardening, row G). No node needed: a stub records what arrived.

* **Whole-second timeouts.** The gateway reads ``timeout_secs`` as an unsigned integer —
  ``as_u64().unwrap_or(30|10)`` on ``rpc/call`` and ``scatter``, a ``u64`` field on ``emit_reliable``,
  ``wiki/ingest`` and the tuple ``take`` routes. A fraction was silently replaced by the route's default (or
  refused 422) while the client waited ``t + 5``; the tuple ``take`` sent ``int(t)``, so ``0 < t < 1`` parked
  for zero seconds. A fraction now rounds **up**: never below 1 on the mesh routes (as ``mycelium-ts``'s
  ``wholeSeconds`` does), never below 0 on ``take``, where ``0`` is the documented poll.
* **Path segments escaped.** A caller- or row-supplied value that becomes one path segment is percent-encoded,
  so ``/``, ``?`` and ``#`` stay inside the segment they were meant for; a bare ``.``/``..``, which any URL
  parser resolves, is refused before a request is made.
* **One** ``scatter_gather`` **default.** ``min_ok`` defaults to 1, the gateway's own default (``mycelium-ts``
  waited for every target; it now agrees).
"""

from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote

import pytest

from mycelium import MyceliumAgent, PromptSkillClient, ReasonClient, TupleSpace, Wiki
from mycelium.agent import CapabilityHandle, LockGuard


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[str, str, dict]] = []
    reply: dict = {}

    def _answer(self) -> None:
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw) if raw else {}
        except ValueError:
            body = {}
        _Stub.seen.append((self.command, self.path, body))
        if "/sse/" in self.path or "/rpc/serve/" in self.path or "/mailbox/" in self.path:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if self.command == "GET" and ("/prompts/" in self.path or "/reason/blob/" in self.path):
            data = b'{"error":"not_found"}'
            self.send_response(404)
        else:
            data = json.dumps(_Stub.reply).encode()
            self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_DELETE = _answer  # noqa: N815

    def log_message(self, *_: object) -> None:
        pass


REPLY = {
    "ok": True, "result_b64": "", "replies": [], "ack": "acknowledged",
    "id": 7, "payload_b64": "", "owner": "127.0.0.1:1", "summary": {},
    "run_id": "r", "events": [], "narrative": "", "domain": "d",
}


@pytest.fixture
def port():
    _Stub.seen, _Stub.reply = [], dict(REPLY)
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def _last_body() -> dict:
    return _Stub.seen[-1][2]


# ── Whole-second timeouts ────────────────────────────────────────────────────

MESH_TIMEOUTS = {
    "rpc_call":       lambda a, t: a.rpc_call("127.0.0.1:1", "echo", timeout_secs=t),
    "scatter_gather": lambda a, t: a.scatter_gather(["127.0.0.1:1"], "echo", timeout_secs=t),
    "emit_reliable":  lambda a, t: a.emit_reliable("127.0.0.1:1", "echo", timeout_secs=t),
}


@pytest.mark.parametrize("verb", list(MESH_TIMEOUTS))
@pytest.mark.parametrize("given,sent", [(2.5, 3), (0.3, 1), (4, 4), (0, 1)])
def test_a_mesh_timeout_is_sent_in_whole_seconds_rounded_up(port, verb, given, sent):
    MESH_TIMEOUTS[verb](MyceliumAgent("127.0.0.1", port), given)
    value = _last_body()["timeout_secs"]
    # An integer the gateway's `as_u64()` reads, not a float it replaces with its default.
    assert type(value) is int and value == sent


@pytest.mark.parametrize("given,sent", [(1.5, 2), (0.2, 1), (60, 60)])
def test_wiki_ingest_sends_whole_seconds(port, given, sent):
    asyncio.run(Wiki("127.0.0.1", port).ingest("batch/1", timeout_secs=given))
    value = _last_body()["timeout_secs"]
    assert type(value) is int and value == sent


TAKES = {
    "take":        lambda ts, t: ts.take("stage-a", timeout_secs=t),
    "take_by_key": lambda ts, t: ts.take_by_key("stage-a", "k", timeout_secs=t),
}


@pytest.mark.parametrize("verb", list(TAKES))
@pytest.mark.parametrize("given,sent", [(0.3, 1), (2.2, 3), (5, 5), (0, 0)])
def test_a_tuple_take_does_not_truncate_its_park(port, verb, given, sent):
    # `int(0.3)` was 0: the worker asked to wait and the gateway answered at once.
    asyncio.run(TAKES[verb](TupleSpace("127.0.0.1", port), given))
    value = _last_body()["timeout_secs"]
    assert type(value) is int and value == sent


# ── Path segments ────────────────────────────────────────────────────────────

HOSTILE = ["a/b", "x?y=1#z", "sp ace%", "%2e%2e"]


def _seg(s: str) -> str:
    return quote(s, safe="")


async def _drain(gen) -> None:
    async for _ in gen:
        pass


SEGMENT_CALLS = {
    "on_signal":       (lambda a, s: asyncio.run(_drain(a.on_signal(s))),            "/gateway/signal/sse/{}"),
    "rpc_serve":       (lambda a, s: asyncio.run(_drain(a.rpc_serve(s))),            "/gateway/rpc/serve/{}"),
    "mailbox":         (lambda a, s: asyncio.run(_drain(a.mailbox(s))),              "/gateway/mailbox/{}"),
    "shard_for":       (lambda a, s: a.shard_for(s, s, "k"),                         "/gateway/shard/{0}/{0}?key=k"),
    "catalog":         (lambda a, s: a.federation().catalog(s),                      "/gateway/federation/catalog/{}"),
    "capability.drop": (lambda a, s: CapabilityHandle(_agent=a, handle_id=s).drop(), "/gateway/capability/{}"),
    "capability.beat": (lambda a, s: CapabilityHandle(_agent=a, handle_id=s).heartbeat(),
                        "/gateway/capability/{}/heartbeat"),
    "lock.release":    (lambda a, s: LockGuard(_agent=a, guard_id=s, token=1).release(),
                        "/gateway/overlay/lock/{}"),
    "prompt.get":      (lambda a, s: asyncio.run(PromptSkillClient("127.0.0.1", a._port).get(s, s)),
                        "/gateway/prompts/{0}/{0}"),
    "prompt.delete":   (lambda a, s: asyncio.run(PromptSkillClient("127.0.0.1", a._port).delete_prompt(s, s)),
                        "/gateway/prompts/{0}/{0}"),
    "reason.trace":    (lambda a, s: asyncio.run(ReasonClient("127.0.0.1", a._port).trace(s)),
                        "/gateway/reason/trace/{}"),
    "reason.blob_get": (lambda a, s: asyncio.run(ReasonClient("127.0.0.1", a._port).blob_get(s)),
                        "/gateway/reason/blob/{}"),
}


@pytest.mark.parametrize("verb", list(SEGMENT_CALLS))
@pytest.mark.parametrize("seg", HOSTILE)
def test_a_caller_supplied_value_stays_one_path_segment(port, verb, seg):
    agent = MyceliumAgent("127.0.0.1", port)
    agent._port = port
    call, shape = SEGMENT_CALLS[verb]
    call(agent, seg)
    assert _Stub.seen, "nothing reached the gateway"
    assert _Stub.seen[-1][1] == shape.format(_seg(seg))


@pytest.mark.parametrize("verb", list(SEGMENT_CALLS))
@pytest.mark.parametrize("seg", [".", ".."])
def test_a_dot_segment_is_refused_before_any_request(port, verb, seg):
    # A URL parser resolves `..` — encoded or not — so it would reach another route.
    agent = MyceliumAgent("127.0.0.1", port)
    agent._port = port
    with pytest.raises(ValueError, match="dot segment"):
        SEGMENT_CALLS[verb][0](agent, seg)
    assert _Stub.seen == []


def test_update_prompt_escapes_its_segments(port):
    from mycelium import PromptTemplate
    t = PromptTemplate(system="s", user_template="u")
    asyncio.run(PromptSkillClient("127.0.0.1", port).update_prompt("a/b", "c?d", t))
    assert _Stub.seen[-1][1] == "/gateway/prompts/a%2Fb/c%3Fd"


# ── scatter_gather's default ─────────────────────────────────────────────────

def test_scatter_gather_waits_for_one_reply_by_default(port):
    MyceliumAgent("127.0.0.1", port).scatter_gather(["127.0.0.1:1", "127.0.0.1:2"], "echo")
    assert _last_body()["min_ok"] == 1
