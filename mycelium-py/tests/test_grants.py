"""The supervisor's grant verbs (adopter-surface X3's door; X7's *grants* cell): ``grants_pending`` reads
``GET /gateway/grants/pending`` and ``grant_decide`` posts ``{decision, ttl_secs?, reason?}`` to
``POST /gateway/grants/{agent}/{request}``, each under the bearer. No node: a stub records what the SDK sent.
"""
from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest

from mycelium import MyceliumAgent

PENDING = [{"key": "grant/node:127.0.0.1:7946/r1", "agent": "node:127.0.0.1:7946", "request": "r1", "tool": "purchase.place",
            "resource": "tool:purchase.place@127.0.0.1:7946", "arguments_digest": "ab" * 32, "policy_revision": "rev-1",
            "requested_at_ms": 1, "expires_at_ms": 2}]
SHOWN = {"tool": "purchase.place", "resource": "tool:purchase.place@127.0.0.1:7946", "arguments_digest": "ab" * 32, "request_expires_at_ms": 2}


def _row(request: str, agent: str = "node:127.0.0.1:7946") -> dict:
    return {**PENDING[0], "request": request, "agent": agent, "key": f"grant/{agent}/{request}"}


class _Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    seen: list[tuple[str, str, dict, str | None]] = []
    status = 200

    def _json(self, body: dict, status: int = 200) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        _Stub.seen.append(("GET", self.path, {}, self.headers.get("Authorization")))
        if self.path.endswith("/gateway/grants/stream"):
            body = (b'event: grant_request\ndata: {"request":"r1","agent":"node:127.0.0.1:7946","tool":"deploy","key":"grant/node:127.0.0.1:7946/r1"}\n\n'
                    b'event: grant_answered\ndata: {"request":"r1","outcome":"issued","issuer":"token:gw/alice"}\n\n')
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/health":
            self._json({"status": "ok", "node_id": "127.0.0.1:7946"})
            return
        self._json({"ok": True, "pending": PENDING, "now_ms": 1})

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        _Stub.seen.append(("POST", self.path, body, self.headers.get("Authorization")))
        if _Stub.status != 200:
            self._json({"ok": False, "error": "not_a_member", "detail": "no member's agent"}, _Stub.status)
            return
        self._json({"ok": True, "key": "grant/a/r1", "operation_id": "op-1", "local_durability": "on_disk"})

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv("MYCELIUM_GATEWAY_TOKEN", raising=False)
    _Stub.seen, _Stub.status = [], 200
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def test_grants_pending_reads_the_route_under_the_bearer(stub):
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    rows = agent.grants_pending()
    assert rows == PENDING
    assert _Stub.seen == [("GET", "/gateway/grants/pending", {}, "Bearer super")]


def test_grant_decide_posts_the_decision_naming_the_record_and_returns_the_records_receipt(stub):
    # Identity-and-lifecycle I7-S (D4): a bearer's decision on a record names the record's tool, resource, digest and
    # expiry beside it (`shown`), for the node to compare with its own record.
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    answer = agent.grant_decide(_row("r1"), "issue", ttl_secs=120)
    assert answer["ok"] is True and answer["local_durability"] == "on_disk"
    refusal = agent.grant_decide(_row("r2"), "refuse", reason="not today")
    assert refusal["ok"] is True
    assert [(m, p, b) for m, p, b, _ in _Stub.seen] == [
        ("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r1", {"decision": "issue", "ttl_secs": 120, "shown": SHOWN}),
        ("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r2", {"decision": "refuse", "reason": "not today", "shown": SHOWN}),
    ]
    assert all(auth == "Bearer super" for *_, auth in _Stub.seen)


def test_the_ids_only_form_is_deprecated_and_names_nothing(stub):
    # I7-S: `grant_decide(agent, request, decision)` is kept for one release, warned about; it sends no block, which
    # a gateway after I7-S refuses (`decision_names_nothing`) unless its operator accepts unnamed decisions.
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    with pytest.warns(DeprecationWarning, match="grant_decide\\(record, decision\\)"):
        agent.grant_decide("node:127.0.0.1:7946", "r1", "issue", ttl_secs=120)
    assert [(m, p, b) for m, p, b, _ in _Stub.seen] == [("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r1", {"decision": "issue", "ttl_secs": 120})]


def test_every_deprecated_shape_still_posts_for_the_window(stub):
    # #820's review, L1: the ids-only call kept for one release in each shape a caller may have written — positional,
    # the decision by keyword, all by keyword — and the record form by keyword too.
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    with pytest.warns(DeprecationWarning):
        agent.grant_decide("node:127.0.0.1:7946", "r1", decision="issue")
    with pytest.warns(DeprecationWarning):
        agent.grant_decide(agent="node:127.0.0.1:7946", request="r2", decision="refuse", reason="no")
    agent.grant_decide(record=_row("r3"), decision="issue")
    assert [(p, b) for _, p, b, _ in _Stub.seen] == [
        ("/gateway/grants/node%3A127.0.0.1%3A7946/r1", {"decision": "issue"}),
        ("/gateway/grants/node%3A127.0.0.1%3A7946/r2", {"decision": "refuse", "reason": "no"}),
        ("/gateway/grants/node%3A127.0.0.1%3A7946/r3", {"decision": "issue", "shown": SHOWN}),
    ]
    with pytest.raises(TypeError):
        agent.grant_decide(_row("r4"), "issue", decision="issue")


def test_a_record_lacking_a_field_it_names_is_refused_before_it_is_sent(stub):
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    for field in ("tool", "resource", "arguments_digest", "expires_at_ms"):
        row = _row("r1")
        del row[field]
        with pytest.raises(ValueError):
            agent.grant_decide(row, "issue")
    assert _Stub.seen == []


def test_grants_stream_yields_the_requests_and_answers_it_is_sent(stub):
    import asyncio
    async def first_two():
        agent = MyceliumAgent("127.0.0.1", stub, token="super")
        got = []
        async for ev in agent.grants_stream():
            got.append(ev)
            if len(got) == 2:
                break
        return got
    got = asyncio.run(first_two())
    assert got[0]["event"] == "grant_request" and got[0]["request"] == "r1" and got[0]["tool"] == "deploy"
    assert got[1]["event"] == "grant_answered" and got[1]["outcome"] == "issued"
    assert _Stub.seen[0][:2] == ("GET", "/gateway/grants/stream") and _Stub.seen[0][3] == "Bearer super"


def test_grant_revoke_posts_to_the_revoke_route_under_the_bearer(stub):
    agent = MyceliumAgent("127.0.0.1", stub, token="super")
    answer = agent.grant_revoke("node:127.0.0.1:7946", "r1", reason="freeze")
    assert answer["ok"] is True
    assert [(m, p, b) for m, p, b, _ in _Stub.seen] == [("POST", "/gateway/grants/node%3A127.0.0.1%3A7946/r1/revoke", {"reason": "freeze"})]
    assert _Stub.seen[0][3] == "Bearer super"


def test_a_bad_decision_is_refused_before_it_is_sent_and_a_refusal_surfaces(stub):
    agent = MyceliumAgent("127.0.0.1", stub)
    with pytest.raises(ValueError):
        agent.grant_decide(_row("r"), "maybe")
    assert _Stub.seen == []
    _Stub.status = 409
    with pytest.raises(httpx.HTTPStatusError) as e:
        agent.grant_decide(_row("r1", "node:other"), "issue")
    assert e.value.response.status_code == 409
    assert e.value.response.json()["error"] in ("not_a_member",)


def test_a_signed_decision_and_revocation_carry_the_signers_signature_addressed_to_the_node(stub):
    # Identity-and-lifecycle I3: with a signer, the body carries `signed` over exactly this decision, addressed to
    # the node `GET /health` names (read once), so a bearer holding only `grant:relay` can carry it.
    import base64

    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    from mycelium import ClientSigner
    from mycelium.client_sig import grant_decision_message

    signer = ClientSigner(bytes([7] * 32), "token:fleet/treasurer")
    agent = MyceliumAgent("127.0.0.1", stub, token="relay")
    agent.grant_decide(_row("r1", "token:fleet/forecaster"), "issue", ttl_secs=120, signer=signer)
    agent.grant_revoke("token:fleet/forecaster", "r1", reason="season over", signer=signer)
    posts = [b for m, _, b, _ in _Stub.seen if m == "POST"]
    assert [p for m, p, *_ in _Stub.seen if m == "GET"] == ["/health"], "the node is read once"
    # I7-S: the issue names the record's block inside the signature, under the grade `service`, and nothing beside it;
    # the revocation names none.
    assert (posts[0]["signed"]["shown"], posts[0]["signed"]["grade"], "shown" in posts[0]) == (SHOWN, "service", False)
    assert "shown" not in posts[1]["signed"] and "grade" not in posts[1]["signed"]
    for body, decision, ttl, reason, grade, shown in [(posts[0], "issue", 120, None, "service", SHOWN), (posts[1], "revoke", None, "season over", None, None)]:
        s = body["signed"]
        assert (s["decider"], s["node"], s["agent"], s["request"]) == ("token:fleet/treasurer", "127.0.0.1:7946", "token:fleet/forecaster", "r1")
        msg = grant_decision_message(s["decider"], s["node"], s["agent"], s["request"], decision, ttl, reason, s["t"], grade, shown)
        Ed25519PublicKey.from_public_bytes(base64.b64decode(s["k"])).verify(base64.b64decode(s["sig"]), msg)
