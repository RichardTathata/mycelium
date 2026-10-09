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
