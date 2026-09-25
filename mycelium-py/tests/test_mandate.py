"""Presenting a mandate through the SDK (Boundary H, A1 at the gateway).

The gateway establishes a presented mandate only if the holder's possession proof covers exactly the
bytes the gateway computes. These golden vectors are shared with the gateway's own tests
(`src/agent/gateway_authority.rs`) and the TypeScript SDK, so the three cannot drift apart silently.
"""

from __future__ import annotations

from mycelium import arguments_digest, mandate_request_bytes
from mycelium.a2a import _task_params


def test_arguments_digest_matches_the_gateways_canonical_form():
    assert arguments_digest({"text": "dispatch"}).hex() == (
        "719121f66b67e12629032511ad5cff8f9b541591eed0fffbe645b9f5e14a7a23"
    )


def test_mandate_request_bytes_golden_vector():
    got = mandate_request_bytes("skill.invoke", "skill:depot/dispatch@10.0.0.1:57000", bytes([0xAB]) * 32)
    assert got.hex() == (
        "6d7963656c69756d2e676174657761792f6d616e646174652d726571756573742f31"
        "0c000000" "736b696c6c2e696e766f6b65"
        "14000000" "736b696c6c3a6465706f742f6469737061746368"
        + "ab" * 32
    )


def test_a_presented_mandate_travels_in_meta_and_is_absent_otherwise():
    mandate = {"grant": {"mandate": {}, "signature": []}, "possession": "c2ln"}
    assert _task_params("t", "depot/dispatch", "go", mandate)["_meta"] == {"mandate": mandate}
    assert "_meta" not in _task_params("t", "depot/dispatch", "go", None)
