"""The client-signature vector (identity-and-lifecycle I1, D1′), shared with the gateway's Rust tests and the
TypeScript SDK: the message rebuilt from its inputs, and the signature the seed's key makes over it."""

from __future__ import annotations

import base64
import json
from pathlib import Path

from mycelium.client_sig import ClientSigner, client_signature_message, mcp_binding

VECTOR = json.loads((Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "cs" / "vector.json").read_text())


def test_the_message_is_the_vectors():
    op, res, digest = mcp_binding("weather.forecast", VECTOR["arguments"])
    assert (op, res, digest.hex()) == (VECTOR["operation"], "tool:weather.forecast", VECTOR["arguments_digest_hex"])
    msg = client_signature_message(
        VECTOR["principal"], op, VECTOR["resource"], digest,
        VECTOR["issued_at_ms"], int(VECTOR["instance_hex"], 16), VECTOR["counter"],
    )
    assert msg.hex() == VECTOR["message_hex"]


def test_the_signer_produces_the_vectors_cs_and_cn():
    signer = ClientSigner(bytes.fromhex(VECTOR["seed_hex"]), VECTOR["principal"], instance=int(VECTOR["instance_hex"], 16))
    _, res, digest = mcp_binding("weather.forecast", VECTOR["arguments"])
    meta = signer.sign("tools/call", VECTOR["resource"], digest, issued_at_ms=VECTOR["issued_at_ms"], counter=VECTOR["counter"])
    assert meta == VECTOR["meta"]
    assert base64.b64decode(meta["cs"]["k"]) == base64.b64decode(VECTOR["verifying_key_b64"])


def test_each_call_takes_the_next_counter_and_processes_draw_different_instances():
    a = ClientSigner(bytes(range(1, 33)), "token:fleet/forecaster")
    b = ClientSigner(bytes(range(1, 33)), "token:fleet/forecaster")
    d = bytes(32)
    assert [a.sign("tools/call", "tool:x", d)["cn"]["c"] for _ in range(3)] == [0, 1, 2]
    assert a.instance != b.instance


def test_the_canonical_arguments_are_the_same_in_every_language():
    # #742's review, finding 1: RFC 8785, as the gateway and the TypeScript SDK compute it.
    from mycelium.client_sig import canonical_json, client_arguments_digest

    for case in VECTOR["canonical_cases"]:
        assert canonical_json(case["arguments"]) == case["canonical"], case["arguments"]
        assert client_arguments_digest(case["arguments"]).hex() == case["digest_hex"], case["arguments"]


def test_an_integer_beyond_2_53_is_refused_by_the_signer():
    # #742's confirm round, finding 1: RFC 8785 cannot bind such an integer exactly; send large ids as strings.
    import pytest

    from mycelium.client_sig import mcp_binding

    for text in VECTOR["unsafe_integer_cases"]:
        with pytest.raises(ValueError):
            mcp_binding("weather.forecast", json.loads(text))
    for text in VECTOR["safe_integer_cases"]:
        mcp_binding("weather.forecast", json.loads(text))
