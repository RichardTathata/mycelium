"""
How the saver reaches the gateway — the scheme and the bearer — with no node.

The saver built its base URL as ``f"http://{host}:{port}"`` and opened both httpx clients with no
headers: a gateway serving HTTPS (``gateway_tls``) was unreachable, and a token-protected one answered
401 to every call while this package's own docs described ``"unauthorized"`` as a token problem.
It now takes ``scheme=`` / ``ca_file=`` (0.3.2) and ``token=`` resolved as the Python SDK resolves it
(the argument, then ``MYCELIUM_GATEWAY_TOKEN``; header only — never in a URL, a ``repr`` or an error).
Seen failing on 0.3.1.
"""

from __future__ import annotations

import ssl

import pytest

from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver

# A throwaway self-signed CA (CN=mycelium-sdk-test-ca), generated for the SDK tests; it signs nothing.
TEST_CA = """-----BEGIN CERTIFICATE-----
MIIBkzCCATmgAwIBAgIUKFsNRviPZSdtoj6oVqdUMNIv2AIwCgYIKoZIzj0EAwIw
HzEdMBsGA1UEAwwUbXljZWxpdW0tc2RrLXRlc3QtY2EwHhcNMjYxMDA5MTgxMTIw
WhcNMzYxMDA2MTgxMTIwWjAfMR0wGwYDVQQDDBRteWNlbGl1bS1zZGstdGVzdC1j
YTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABMSmxuFXw3f0qPtlGM0wja68muU0
ADvMMnUdxWZ8IS7GnrxE9+tay1AMEJHRylvK+NpZLNgxoT0bWAkxusXXduOjUzBR
MB0GA1UdDgQWBBQsuPY68HQ4ZguDP0+o5PUXEWz2czAfBgNVHSMEGDAWgBQsuPY6
8HQ4ZguDP0+o5PUXEWz2czAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0gA
MEUCIQCnuIeFoWJcoCvu4VUMAXDD+Jsob8RUokvy7Lfdb9oK9wIgVblYalOZBced
CetJsIqWHnfdoSCdh52G1lMextSTS08=
-----END CERTIFICATE-----
"""


# ── the scheme (0.3.2) ───────────────────────────────────────────────────────


def test_https_scheme_reaches_both_clients():
    with MyceliumCheckpointSaver("10.0.0.5", 8101, scheme="https") as s:
        assert str(s._client.base_url) == "https://10.0.0.5:8101"
        assert str(s._aclient.base_url) == "https://10.0.0.5:8101"


def test_default_scheme_is_still_http():
    with MyceliumCheckpointSaver("127.0.0.1", 8101) as s:
        assert str(s._client.base_url) == "http://127.0.0.1:8101"


def test_an_unknown_scheme_is_refused():
    with pytest.raises(ValueError, match="scheme"):
        MyceliumCheckpointSaver("127.0.0.1", 8101, scheme="ftp")


def test_ca_file_pins_a_private_fleet_ca_and_keeps_verification_on(tmp_path):
    ca = tmp_path / "ca-cert.pem"
    ca.write_text(TEST_CA)
    with MyceliumCheckpointSaver("10.0.0.5", 8101, scheme="https", ca_file=str(ca)) as s:
        ctx = s._client._transport._pool._ssl_context
        assert isinstance(ctx, ssl.SSLContext)
        assert ctx.verify_mode == ssl.CERT_REQUIRED and ctx.check_hostname is True
        subjects = [dict(x[0] for x in c["subject"]) for c in ctx.get_ca_certs()]
        assert {"commonName": "mycelium-sdk-test-ca"} in subjects
    with pytest.raises(FileNotFoundError):
        MyceliumCheckpointSaver("10.0.0.5", 8101, scheme="https", ca_file=str(tmp_path / "absent.pem"))


# ── the bearer (0.3.2) ───────────────────────────────────────────────────────

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN_ENV = "MYCELIUM_GATEWAY_TOKEN"


class _Recorder(BaseHTTPRequestHandler):
    """Records the ``Authorization`` header of every request; answers 401 when told to."""

    protocol_version = "HTTP/1.1"
    seen: list[str | None] = []
    refuse = False

    def do_GET(self) -> None:  # noqa: N802
        _Recorder.seen.append(self.headers.get("Authorization"))
        status, body = (401, {"error": "unauthorized"}) if _Recorder.refuse else (200, {"found": False})
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_: object) -> None:
        pass


@pytest.fixture
def stub(monkeypatch):
    monkeypatch.delenv(TOKEN_ENV, raising=False)
    _Recorder.seen, _Recorder.refuse = [], False
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()


def test_an_explicit_token_rides_both_clients(stub):
    with MyceliumCheckpointSaver("127.0.0.1", stub, token="secret") as s:
        s._kv_get("ckpt/x")
        asyncio.run(s._akv_get("ckpt/x"))
    assert _Recorder.seen == ["Bearer secret", "Bearer secret"]


def test_the_env_token_is_used_when_none_is_given(stub, monkeypatch):
    monkeypatch.setenv(TOKEN_ENV, "env-token")
    with MyceliumCheckpointSaver("127.0.0.1", stub) as s:
        s._kv_get("ckpt/x")
    assert _Recorder.seen == ["Bearer env-token"]


def test_the_argument_beats_the_env_and_empty_means_none(stub, monkeypatch):
    monkeypatch.setenv(TOKEN_ENV, "env-token")
    with MyceliumCheckpointSaver("127.0.0.1", stub, token="arg") as s:
        s._kv_get("ckpt/x")
    with MyceliumCheckpointSaver("127.0.0.1", stub, token="") as s:
        s._kv_get("ckpt/x")
    assert _Recorder.seen == ["Bearer arg", None]


def test_no_token_sends_no_header(stub):
    with MyceliumCheckpointSaver("127.0.0.1", stub) as s:
        s._kv_get("ckpt/x")
    assert _Recorder.seen == [None]


def test_the_token_is_never_shown(stub):
    import httpx

    secret = "s3cr3t-bearer-value"
    with MyceliumCheckpointSaver("127.0.0.1", stub, token=secret) as s:
        assert secret not in repr(s) and secret not in str(s)
        assert secret not in s._base
        _Recorder.refuse = True
        with pytest.raises(httpx.HTTPStatusError) as ei:
            s._kv_get("ckpt/x")
        assert secret not in str(ei.value) and secret not in repr(ei.value)
        assert secret not in repr(ei.value.request.headers)   # httpx redacts, and we rely on it
