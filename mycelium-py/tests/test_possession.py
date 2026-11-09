"""A grant held by a client, and the SDK's ``tools/call`` and A2A clients signing as it (identity-and-lifecycle I1).

The possession vector — ``tests/fixtures/possession/vector.json`` — is shared with the gateway's Rust tests (which
check it with the gateway's own functions and verifier) and the TypeScript SDK. The clients are run against a stub
that records what they sent; the signatures in it are verified here, as the provider would.
"""
from __future__ import annotations

import base64
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

from mycelium import (A2aClient, ClientSigner, GrantNotIssuedError, McpClient, a2a_binding, arguments_digest,
                      client_signature_message, mandate_canonical_bytes, mandate_digest, mandate_request_bytes, mcp_binding,
                      possession_message)

VECTOR = json.loads((Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "possession" / "vector.json").read_text())
HOLDER = bytes.fromhex(VECTOR["holder_seed_hex"])


def _verify(key_b64: str, message: bytes, sig_b64: str) -> None:
    Ed25519PublicKey.from_public_bytes(base64.b64decode(key_b64)).verify(base64.b64decode(sig_b64), message)


def test_the_possession_vector_is_rebuilt_from_its_inputs():
    mandate = VECTOR["grant"]["mandate"]
    assert mandate_canonical_bytes(mandate).hex() == VECTOR["mandate_canonical_hex"]
    assert mandate_digest(mandate).hex() == VECTOR["mandate_digest_hex"]
    assert arguments_digest(VECTOR["arguments"]).hex() == VECTOR["arguments_digest_hex"]
    request = mandate_request_bytes(VECTOR["operation"], VECTOR["resource"], arguments_digest(VECTOR["arguments"]))
    assert request.hex() == VECTOR["request_hex"]
    assert possession_message(mandate, request).hex() == VECTOR["possession_message_hex"]
    _verify(VECTOR["issuer_key_b64"], mandate_canonical_bytes(mandate), base64.b64encode(bytes(VECTOR["grant"]["signature"])).decode())


def test_the_signer_proves_possession_as_the_vector_does_and_only_for_its_own_grant():
    signer = ClientSigner(HOLDER, VECTOR["principal"])
    presented = signer.prove_possession(VECTOR["grant"], VECTOR["operation"], VECTOR["resource"], VECTOR["arguments"])
    assert presented == {"grant": VECTOR["grant"], "possession": VECTOR["possession_b64"]}
    with pytest.raises(ValueError, match="held by"):
        ClientSigner(HOLDER, "token:fleet/someone-else").prove_possession(VECTOR["grant"], "tools/call", "tool:weather.forecast", {})


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list = []
    answer: dict = {}

    def _json(self, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        _Stub.seen.append(("GET", self.path, None))
        self._json(_Stub.answer)

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        _Stub.seen.append(("POST", self.path, body))
        if self.path == "/mcp":
            self._json({"jsonrpc": "2.0", "id": body["id"], "result": {"content": [{"type": "text", "text": "sunny"}]}})
        elif self.path == "/a2a":
            self._json({"jsonrpc": "2.0", "id": 1, "result": {"id": "t", "status": {"state": "completed"}, "artifacts": [{"parts": [{"type": "text", "text": "done"}]}]}})
        else:
            self._json({"ok": True, "request": "r-1", "key": "grant/ab/r-1", "resource": VECTOR["resource"], "reused": False})

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    _Stub.seen = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def _cs_message(signer: ClientSigner, meta: dict, operation: str, resource: str, digest: bytes) -> bytes:
    return client_signature_message(signer.principal, operation, resource, digest, meta["cn"]["t"], int(meta["cn"]["i"], 16), meta["cn"]["c"])


def test_a_tools_call_is_signed_and_presents_its_grant_with_a_proof_for_this_call(stub):
    signer = ClientSigner(HOLDER, VECTOR["principal"])
    mcp = McpClient(stub, token="fc", signer=signer)
    result = mcp.call_tool("weather.forecast", VECTOR["arguments"], grant=VECTOR["grant"], correlation="run-1")
    assert result["content"][0]["text"] == "sunny"
    method, path, body = _Stub.seen[-1]
    assert (method, path, body["method"], body["params"]["name"]) == ("POST", "/mcp", "tools/call", "weather.forecast")
    meta = body["params"]["_meta"]
    # The client's signature, as the provider checks it: over the tool, the arguments' JCS digest, the nonce.
    op, res, digest = mcp_binding("weather.forecast", body["params"]["arguments"])
    _verify(meta["cs"]["k"], _cs_message(signer, meta, op, res, digest), meta["cs"]["sig"])
    # The grant, with a possession proof over this call, by the same key.
    assert meta["mandate"]["grant"] == VECTOR["grant"]
    request = mandate_request_bytes("tools/call", "tool:weather.forecast", arguments_digest(body["params"]["arguments"]))
    _verify(VECTOR["holder_key_b64"], possession_message(VECTOR["grant"]["mandate"], request), meta["mandate"]["possession"])
    assert meta["correlation"] == "run-1"


def test_an_unsigned_client_sends_no_meta_and_an_unsafe_integer_is_refused_before_sending(stub):
    McpClient(stub).call_tool("weather.forecast", {"city": "Lyon"})
    assert "_meta" not in _Stub.seen[-1][2]["params"]
    sent = len(_Stub.seen)
    with pytest.raises(ValueError, match="2\\^53"):
        McpClient(stub, signer=ClientSigner(HOLDER, VECTOR["principal"])).call_tool("weather.forecast", {"id": 2**60})
    with pytest.raises(ValueError, match="signer"):
        McpClient(stub).call_tool("weather.forecast", {}, grant=VECTOR["grant"])
    assert len(_Stub.seen) == sent, "nothing was sent"


def test_the_grant_verbs_ask_and_read_the_callers_own_answer(stub):
    mcp = McpClient(stub, token="fc")
    asked = mcp.request_grant("weather.forecast", {"city": "Lyon"}, arguments_class="public", window_secs=30)
    assert asked["request"] == "r-1"
    assert _Stub.seen[-1] == ("POST", "/gateway/grants/request", {"tool": "weather.forecast", "arguments": {"city": "Lyon"}, "arguments_class": "public", "window_secs": 30})
    _Stub.answer = {"ok": True, "state": "issued", "grant": VECTOR["grant"]}
    assert mcp.wait_for_grant("01a1-10.0.0.2:7946")["grant"] == VECTOR["grant"]
    assert _Stub.seen[-1][:2] == ("GET", "/gateway/grants/request/01a1-10.0.0.2%3A7946"), "the request id is one percent-encoded segment"
    _Stub.answer = {"ok": True, "state": "refused", "refusal": {"reason": "not today"}}
    with pytest.raises(GrantNotIssuedError) as e:
        mcp.wait_for_grant("r-1")
    assert e.value.state == "refused"


def test_an_a2a_send_and_stream_are_signed_over_the_skill_and_the_text(stub):
    signer = ClientSigner(HOLDER, VECTOR["principal"])
    assert A2aClient(stub, signer=signer).send("depot/dispatch", "go") == "done"
    meta = _Stub.seen[-1][2]["params"]["_meta"]
    _verify(meta["cs"]["k"], _cs_message(signer, meta, *a2a_binding("depot/dispatch", "go")), meta["cs"]["sig"])
    from mycelium.a2a import _task_params
    streamed = _task_params("t", "depot/dispatch", "go", None, signer=signer)["_meta"]
    assert streamed["cn"]["c"] == meta["cn"]["c"] + 1, "each send takes the next counter"
    _verify(streamed["cs"]["k"], _cs_message(signer, streamed, *a2a_binding("depot/dispatch", "go")), streamed["cs"]["sig"])
    assert "_meta" not in _task_params("t", "depot/dispatch", "go", None)
