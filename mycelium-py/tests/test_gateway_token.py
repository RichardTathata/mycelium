"""
Gateway bearer support — no node needed. A stub HTTP server records the
``Authorization`` header of every request; each handle must send
``Bearer <token>`` (explicit ``token=`` or ``MYCELIUM_GATEWAY_TOKEN``), on the
pooled sync + async clients and on the dedicated SSE/stream clients, and must
send nothing when no token is configured (the open-gateway path is unchanged).
"""

from __future__ import annotations

import asyncio
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent, TOKEN_ENV, auth_headers, resolve_token
from mycelium.a2a import A2aClient
from mycelium.prompt_skill import PromptSkillClient
from mycelium.reason import ReasonClient
from mycelium.tuple import TupleSpace
from mycelium.wiki import Wiki
from mycelium.blackboard import Blackboard


class _Recorder(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[str, str | None]] = []

    def _record(self) -> None:
        _Recorder.seen.append((self.path, self.headers.get("Authorization")))

    def _json(self, body: dict, status: int = 200) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        self._record()
        if "/gateway/signal/sse/" in self.path:
            body = b"event: k\ndata: {\"sender\":\"n\",\"payload\":\"\"}\n\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path.startswith("/.well-known/"):
            self._json({"name": "n", "url": "u", "skills": [], "capabilities": {}})
            return
        self._json({"found": False, "keys": [], "prompts": [], "stages": [], "available": 0, "inflight": 0})

    def do_POST(self) -> None:  # noqa: N802
        self._record()
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self._json({"ok": True, "page": None, "id": 1, "facts": [], "candidates": []})

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv(TOKEN_ENV, raising=False)
    _Recorder.seen = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def _auths() -> list[str | None]:
    return [a for _p, a in _Recorder.seen]


def test_helpers(monkeypatch):
    monkeypatch.setenv(TOKEN_ENV, "from-env")
    assert resolve_token("explicit") == "explicit"
    assert resolve_token(None) == "from-env"
    assert resolve_token("") is None
    monkeypatch.delenv(TOKEN_ENV)
    assert resolve_token(None) is None
    assert auth_headers("t") == {"Authorization": "Bearer t"}
    assert auth_headers(None) == {}


def test_agent_sync_calls_send_bearer(stub):
    with MyceliumAgent("127.0.0.1", stub, token="secret") as agent:
        agent.get("k")
        agent.set("k", b"v")
    assert _auths() == ["Bearer secret", "Bearer secret"]


def test_agent_reads_env_when_no_token_given(stub, monkeypatch):
    monkeypatch.setenv(TOKEN_ENV, "env-token")
    with MyceliumAgent("127.0.0.1", stub) as agent:
        agent.get("k")
    assert _auths() == ["Bearer env-token"]


def test_no_token_sends_no_header(stub):
    with MyceliumAgent("127.0.0.1", stub) as agent:
        agent.get("k")
    assert _auths() == [None]


def test_agent_sse_stream_carries_bearer(stub):
    async def first_event():
        agent = MyceliumAgent("127.0.0.1", stub, token="sse-tok")
        async for _ev in agent.on_signal("k"):
            break
    asyncio.run(first_event())
    assert _auths() == ["Bearer sse-tok"]


def test_async_handles_send_bearer(stub):
    async def go():
        await Wiki("127.0.0.1", stub, "g", token="w").read("p")
        await TupleSpace("127.0.0.1", stub, "ns", token="t").depth()
        await Blackboard("127.0.0.1", stub, "b", token="b").depth()
        await PromptSkillClient("127.0.0.1", stub, token="p").list()
    asyncio.run(go())
    assert _auths() == ["Bearer w", "Bearer t", "Bearer b", "Bearer p"]


def test_a2a_card_and_reason_client_send_bearer(stub):
    A2aClient(f"http://127.0.0.1:{stub}", token="a").fetch_card()
    assert _auths() == ["Bearer a"]
    # ReasonClient shares the pool: constructing with a token wires the header
    rc = ReasonClient("127.0.0.1", stub, token="r")
    assert rc._pool.headers == {"Authorization": "Bearer r"}
