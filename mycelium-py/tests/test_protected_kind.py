"""
Closure plan C1: the gateway refuses protected RPC kinds (``mcp.invoke``, ``skill.invoke``,
``llm.invoke``, operator-listed) on its raw mesh routes with ``403 protected_kind``. The SDK raises
``ProtectedKindError`` for it, naming the kind, rather than a bare HTTP error. No node needed: a stub
answers with the gateway's shapes and records what the SDK sent.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent, ProtectedKindError


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    status: int = 403
    reply: dict = {}
    seen: list = []

    def do_POST(self) -> None:  # noqa: N802
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        _Stub.seen.append((self.path, json.loads(body or b"{}")))
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
    _Stub.seen = []
    try:
        yield MyceliumAgent("127.0.0.1", server.server_address[1])
    finally:
        server.shutdown()


def _refuse(kind: str) -> None:
    _Stub.status = 403
    _Stub.reply = {"ok": False, "error": "protected_kind", "kind": kind,
                   "message": f"`{kind}` is protected work and is not accepted on raw mesh routes; use /mcp (tools/call)"}


def test_rpc_call_raises_protected_kind_error_naming_the_kind(agent):
    _refuse("mcp.invoke")
    with pytest.raises(ProtectedKindError) as e:
        agent.rpc_call("127.0.0.1:1", "mcp.invoke", b"{}")
    assert e.value.kind == "mcp.invoke"
    assert "/mcp" in str(e.value)
    assert isinstance(e.value, PermissionError)
    path, body = _Stub.seen[-1]
    assert path == "/gateway/rpc/call" and body["method"] == "mcp.invoke"


def test_emit_and_mailbox_raise_it_too(agent):
    _refuse("skill.invoke")
    with pytest.raises(ProtectedKindError):
        agent.emit("skill.invoke", b"x")
    with pytest.raises(ProtectedKindError):
        agent.deliver_event("127.0.0.1:1", "skill.invoke", b"x")


def test_an_ordinary_403_is_not_mistaken_for_it(agent):
    """The plant: a scope refusal is still an HTTP error, not a ProtectedKindError."""
    _Stub.status = 403
    _Stub.reply = {"error": "insufficient scope", "required_scope": "mesh:write"}
    with pytest.raises(Exception) as e:
        agent.rpc_call("127.0.0.1:1", "echo", b"{}")
    assert not isinstance(e.value, ProtectedKindError)
