"""
The scheme is the caller's to say — no node needed.

Every handle built its base URL as ``f"http://{host}:{port}"``: a gateway serving HTTPS
(``gateway_tls``, v2.3.0) was unreachable from this SDK, and the bearer travelled in cleartext off
loopback. Each handle now takes ``scheme="https"`` (default ``"http"``, unchanged) and ``ca_file=``
for a private fleet CA; verification stays on. Seen failing on 0.2.8 (``scheme`` was not a parameter).
"""

from __future__ import annotations

import ssl

import pytest

from mycelium import MyceliumAgent, base_url
from mycelium.artifacts import Artifacts
from mycelium.blackboard import Blackboard
from mycelium.federation import Federation
from mycelium.prompt_skill import PromptSkillClient
from mycelium.reason import ReasonClient
from mycelium.tuple import TupleSpace
from mycelium.wiki import Wiki

# A throwaway self-signed CA (CN=mycelium-sdk-test-ca), generated for this file; it signs nothing.
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


def _pool_base(handle) -> str:
    return handle._pool._base_url


def test_base_url_helper():
    assert base_url("10.0.0.5", 8300) == "http://10.0.0.5:8300"
    assert base_url("10.0.0.5", 8300, scheme="https") == "https://10.0.0.5:8300"
    assert base_url("gw.example", 9443, scheme="HTTPS") == "https://gw.example:9443"
    with pytest.raises(ValueError, match="scheme"):
        base_url("h", 1, scheme="ftp")


def test_every_handle_takes_the_scheme():
    kw = {"scheme": "https"}
    handles = [
        MyceliumAgent("10.0.0.5", 8300, **kw),
        PromptSkillClient("10.0.0.5", 8300, **kw),
        ReasonClient("10.0.0.5", 8300, **kw),
        Wiki("10.0.0.5", 8300, "g", **kw),
        TupleSpace("10.0.0.5", 8300, "ns", **kw),
        Blackboard("10.0.0.5", 8300, "b", **kw),
        Federation("10.0.0.5", 8300, **kw),
        Artifacts("10.0.0.5", 8300, **kw),
    ]
    for h in handles:
        assert _pool_base(h) == "https://10.0.0.5:8300", type(h).__name__
    # The agent's own record (the SSE streams build their URLs from it) agrees with its pool.
    assert handles[0]._base_url == "https://10.0.0.5:8300"


def test_default_scheme_is_still_http():
    assert _pool_base(MyceliumAgent("127.0.0.1", 7946)) == "http://127.0.0.1:7946"
    assert _pool_base(Wiki("127.0.0.1", 7946)) == "http://127.0.0.1:7946"


def test_derived_handles_inherit_the_agent_scheme():
    agent = MyceliumAgent("10.0.0.5", 8300, scheme="https")
    assert _pool_base(agent.federation()) == "https://10.0.0.5:8300"
    assert _pool_base(agent.artifacts()) == "https://10.0.0.5:8300"


def test_verification_stays_on_by_default():
    agent = MyceliumAgent("10.0.0.5", 8300, scheme="https")
    assert agent._pool.verify is True
    with agent._pool.sync() as c:
        # httpx keeps the verify setting on its transport's SSL context: hostname checking on.
        ctx = c._client._transport._pool._ssl_context
        assert ctx.verify_mode == ssl.CERT_REQUIRED
        assert ctx.check_hostname is True


def test_ca_file_pins_a_private_fleet_ca(tmp_path):
    ca = tmp_path / "ca-cert.pem"
    ca.write_text(TEST_CA)
    agent = MyceliumAgent("10.0.0.5", 8300, scheme="https", ca_file=str(ca))
    ctx = agent._pool.verify
    assert isinstance(ctx, ssl.SSLContext)
    assert ctx.verify_mode == ssl.CERT_REQUIRED and ctx.check_hostname is True
    subjects = [dict(x[0] for x in c["subject"]) for c in ctx.get_ca_certs()]
    assert {"commonName": "mycelium-sdk-test-ca"} in subjects
    # The companions take the same file.
    assert isinstance(Wiki("10.0.0.5", 8300, scheme="https", ca_file=str(ca))._pool.verify, ssl.SSLContext)


def test_a_missing_ca_file_is_refused_at_construction(tmp_path):
    with pytest.raises(FileNotFoundError):
        MyceliumAgent("10.0.0.5", 8300, scheme="https", ca_file=str(tmp_path / "absent.pem"))
