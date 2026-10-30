"""
Adopter-surface X4 in the Python SDK: the prompt-skill and federation ``call`` verbs put ``correlation`` and
``parent`` in the body's ``_meta`` — and nothing when neither is given. Y11 adds the take, claim and emit verbs, whose
doors read ``_meta`` when an exposure rule selects them. No node: a stub records the body.
"""
from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent
from mycelium.blackboard import Blackboard
from mycelium.tuple import TupleSpace
from mycelium.prompt_skill import PromptSkillClient


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    last_body: dict = {}

    def do_POST(self) -> None:  # noqa: N802
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        _Stub.last_body = json.loads(raw or b"{}")
        data = json.dumps({
            "output": "ok", "provider": "127.0.0.1:1", "reply": "ok",
            # The take, claim and emit replies (Y11).
            "ok": True, "ack": "acknowledged", "owner": "127.0.0.1:2", "id": 1, "payload_b64": "", "claimed": False,
        }).encode()
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


H = "cd" * 32


def test_the_prompt_skill_call_carries_the_fields_in_meta(port):
    async def run():
        client = PromptSkillClient("127.0.0.1", port)
        await client.call("demo", "greeting", "x", correlation="run-1", parent=H)
        assert _Stub.last_body["_meta"] == {"correlation": "run-1", "parent": H}
        await client.call("demo", "greeting", "x")
        assert "_meta" not in _Stub.last_body
        await client.close()
    asyncio.run(run())


def test_the_federation_call_carries_the_fields_in_meta(port):
    with MyceliumAgent("127.0.0.1", port) as agent:
        fed = agent.federation()
        fed.call("partner.example", "depot.read", "x", correlation="run-1", parent=H)
        assert _Stub.last_body["_meta"] == {"correlation": "run-1", "parent": H}
        fed.call("partner.example", "depot.read", "x")
        assert "_meta" not in _Stub.last_body


def test_the_take_claim_and_emit_verbs_carry_the_fields_in_meta(port):
    """Y11: every take, claim and emit verb sends ``_meta`` with the fields given, and no ``_meta`` without them —
    an untouched door's body is what it was before. Seen failing first with ``with_causal`` dropping the fields."""
    meta = {"correlation": "run-1", "parent": H}
    with MyceliumAgent("127.0.0.1", port) as agent:
        sync_verbs = [
            lambda **kw: agent.emit("k", b"", **kw),
            lambda **kw: agent.deliver_event("127.0.0.1:2", "k", b"", **kw),
            lambda **kw: agent.emit_reliable("127.0.0.1:2", "k", b"", **kw),
            lambda **kw: agent.emit_sharded("k", "ns", "name", "key", b"", **kw),
        ]
        for verb in sync_verbs:
            verb(correlation="run-1", parent=H)
            assert _Stub.last_body["_meta"] == meta
            verb()
            assert "_meta" not in _Stub.last_body

    async def run():
        ts = TupleSpace("127.0.0.1", port)
        bb = Blackboard("127.0.0.1", port)
        async_verbs = [
            lambda **kw: ts.take("in", 1, **kw),
            lambda **kw: ts.take_by_key("in", "k", 1, **kw),
            lambda **kw: bb.claim({"kind": "job"}, **kw),
        ]
        for verb in async_verbs:
            await verb(correlation="run-1", parent=H)
            assert _Stub.last_body["_meta"] == meta
            await verb()
            assert "_meta" not in _Stub.last_body
    asyncio.run(run())
