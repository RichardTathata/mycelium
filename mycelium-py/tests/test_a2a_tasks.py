"""``A2aClient.cancel`` and ``get_task`` (adopter-surface X7's *cancellation* cell): ``tasks/cancel`` and
``tasks/get`` for one task id, under the bearer, the node's JSON-RPC error raised. No node: a stub answers as
``/a2a`` does.
"""
from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium.a2a import A2aClient, A2aError


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[dict, str | None]] = []
    terminal = False

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        _Stub.seen.append((body, self.headers.get("Authorization")))
        tid = body["params"]["id"]
        if body["method"] == "tasks/cancel" and _Stub.terminal:
            answer = {"jsonrpc": "2.0", "id": body["id"], "error": {"code": -32002, "message": "task completed; cannot cancel"}}
        else:
            state = "canceled" if body["method"] == "tasks/cancel" else "working"
            answer = {"jsonrpc": "2.0", "id": body["id"], "result": {"id": tid, "status": {"state": state}, "artifacts": []}}
        data = json.dumps(answer).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    _Stub.seen, _Stub.terminal = [], False
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()


def test_cancel_sends_tasks_cancel_for_the_task_and_returns_its_state(stub):
    client = A2aClient(stub, token="owner")
    task = client.cancel("t-1")
    assert task["status"]["state"] == "canceled"
    body, auth = _Stub.seen[0]
    assert (body["method"], body["params"]) == ("tasks/cancel", {"id": "t-1"})
    assert auth == "Bearer owner"


def test_get_task_reads_the_task_and_a_terminal_cancel_raises(stub):
    client = A2aClient(stub)
    assert client.get_task("t-2")["status"]["state"] == "working"
    assert _Stub.seen[-1][0]["method"] == "tasks/get"
    _Stub.terminal = True
    with pytest.raises(A2aError) as e:
        client.cancel("t-2")
    assert e.value.code == -32002
