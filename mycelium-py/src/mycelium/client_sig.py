"""A client's own signature on a gateway call — ``cs`` and ``cn`` (identity-and-lifecycle I1, decision D1′).

A service holding a client certificate (``mycelium tls issue --client <name> --issuer <issuer>``) signs each
``tools/call`` or A2A send with the certificate's key and sends the result in ``params._meta``; the provider that runs
the call checks it — the signature, that the key is the principal's current one, that it covers this call's
operation, resource and arguments, and that its ``cn`` was not seen before.

The signed bytes are :func:`client_signature_message`, pinned by the vector in ``tests/fixtures/cs/vector.json``
that the gateway's Rust tests and the TypeScript SDK check too. Signing needs the optional ``cryptography``
dependency (``pip install mycelium-py[sign]``); building the message does not.

The same key proves **possession of a grant held by the client** (``POST /gateway/grants/request``):
:meth:`ClientSigner.prove_possession` signs :func:`possession_message` over the grant's
:func:`mandate_canonical_bytes` and the call's :func:`~mycelium.a2a.mandate_request_bytes`, pinned by
``tests/fixtures/possession/vector.json``, which the gateway's Rust tests and the TypeScript SDK check too.
"""

from __future__ import annotations

import os
import struct
import threading
import time
from typing import Any, Dict, Optional, Tuple


TAG = b"mycelium.gateway/client-signature/1"


def client_signature_message(
    principal: str,
    operation: str,
    resource: str,
    digest: bytes,
    issued_at_ms: int,
    instance: int,
    counter: int,
) -> bytes:
    """The bytes a client signs: the tag; the principal, the operation and the resource, each a ``u32`` little-endian
    length and its UTF-8 bytes; the 32-byte arguments digest; then ``issued_at_ms``, ``instance`` and ``counter`` as
    ``u64`` little-endian. ``resource`` is the bare resource (``tool:{name}``, ``skill:{id}``), whole: a name holding
    ``@`` is bound in full, and no provider is named (one signature serves every provider of the tool)."""
    if len(digest) != 32:
        raise ValueError("the arguments digest is 32 bytes")
    out = bytearray(TAG)
    for part in (principal, operation, resource):
        b = part.encode("utf-8")
        out += struct.pack("<I", len(b)) + b
    out += digest
    out += struct.pack("<QQQ", issued_at_ms, instance, counter)
    return bytes(out)


_MAX_SAFE_INTEGER = 2**53 - 1


def _es_number(x: Any) -> str:
    """ECMAScript's ``Number::toString``: the shortest round-trip digits, fixed notation for exponents in (-7, 21],
    exponent notation with an explicit sign otherwise. An int beyond ±2**53 is the double it rounds to."""
    import math
    from decimal import Decimal

    if isinstance(x, int) and not isinstance(x, bool):
        if abs(x) <= _MAX_SAFE_INTEGER:
            return str(x)
        x = float(x)
    if not math.isfinite(x):
        raise ValueError("JSON has no NaN or infinity")
    if x == 0:
        return "0"
    sign = "-" if x < 0 else ""
    t = Decimal(repr(abs(x))).as_tuple()
    digits = "".join(map(str, t.digits)).rstrip("0") or "0"
    exp = t.exponent + (len(t.digits) - len(digits))
    k = len(digits)
    n = k + exp
    if k <= n <= 21:
        body = digits + "0" * (n - k)
    elif 0 < n <= 21:
        body = digits[:n] + "." + digits[n:]
    elif -6 < n <= 0:
        body = "0." + "0" * (-n) + digits
    else:
        e = n - 1
        es = f"+{e}" if e >= 0 else f"-{-e}"
        body = (digits if k == 1 else digits[0] + "." + digits[1:]) + "e" + es
    return sign + body


def canonical_json(value: Any) -> str:
    """``value`` in the JSON Canonicalization Scheme (RFC 8785), as the gateway and the TypeScript SDK write it: no
    whitespace; object members sorted by their names' UTF-16 code units; strings escaped as ECMAScript's
    ``JSON.stringify`` does; numbers in ECMAScript's form (``0.00001``, ``1e+21``)."""
    import json

    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, (int, float)):
        return _es_number(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, (list, tuple)):
        return "[" + ",".join(canonical_json(v) for v in value) + "]"
    if isinstance(value, dict):
        keys = sorted(value, key=lambda k: k.encode("utf-16-be"))
        return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canonical_json(value[k]) for k in keys) + "}"
    raise TypeError(f"not JSON: {type(value).__name__}")


def client_arguments_digest(arguments: Any) -> bytes:
    """SHA-256 of ``arguments``' canonical form (:func:`canonical_json`) — what a client signature binds."""
    import hashlib

    return hashlib.sha256(canonical_json(arguments).encode("utf-8")).digest()


def holds_unsafe_integer(value: Any) -> bool:
    """Does ``value`` hold, at any depth, an integral number outside ±(2**53 - 1)? RFC 8785 writes it as the nearest
    double, so a signature could not tell it from its neighbour: such a call is refused, at the signer and the
    provider. Send large ids as strings."""
    if isinstance(value, bool):
        return False
    if isinstance(value, int):
        return abs(value) > _MAX_SAFE_INTEGER
    if isinstance(value, float):
        return value.is_integer() and abs(value) > _MAX_SAFE_INTEGER
    if isinstance(value, (list, tuple)):
        return any(holds_unsafe_integer(v) for v in value)
    if isinstance(value, dict):
        return any(holds_unsafe_integer(v) for v in value.values())
    return False


def mcp_binding(name: str, arguments: Dict[str, Any]) -> Tuple[str, str, bytes]:
    """What a ``tools/call`` of ``name`` signs: ``("tools/call", "tool:{name}", digest(arguments))``. Raises
    ``ValueError`` for arguments holding an integer outside ±(2**53 - 1) (:func:`holds_unsafe_integer`)."""
    if holds_unsafe_integer(arguments):
        raise ValueError("an integer outside +-(2^53 - 1) cannot be bound exactly by a client signature: send it as a string")
    return "tools/call", f"tool:{name}", client_arguments_digest(arguments)


def a2a_binding(skill_id: str, text: str) -> Tuple[str, str, bytes]:
    """What an A2A send of ``text`` to ``skill_id`` signs: ``("skill.invoke", "skill:{id}", digest({"text": text}))``."""
    return "skill.invoke", f"skill:{skill_id}", client_arguments_digest({"text": text})


def _lp(out: bytearray, b: bytes) -> None:
    out += struct.pack("<I", len(b)) + b


def mandate_canonical_bytes(mandate: Dict[str, Any]) -> bytes:
    """The bytes a mandate's establishing authority signs — the gateway's ``Mandate::canonical_bytes``: the tag
    ``mycelium.mandate/grant/1``, ``holder``, ``established_by``, ``purpose`` and ``scope`` each as a ``u32``
    little-endian length and its UTF-8 bytes; the number of ``operations`` as a ``u32`` and each operation so; then
    ``epoch`` as a ``u64``, ``term`` length-prefixed, ``valid_from_ms`` and ``valid_until_ms`` as ``u64``, all
    little-endian. ``mandate`` is the ``"mandate"`` of a grant as the gateway returns it."""
    out = bytearray()
    _lp(out, b"mycelium.mandate/grant/1")
    for field in ("holder", "established_by", "purpose", "scope"):
        _lp(out, str(mandate[field]).encode("utf-8"))
    ops = list(mandate["operations"])
    out += struct.pack("<I", len(ops))
    for op in ops:
        _lp(out, str(op).encode("utf-8"))
    out += struct.pack("<Q", int(mandate["epoch"]))
    _lp(out, str(mandate["term"]).encode("utf-8"))
    out += struct.pack("<QQ", int(mandate["valid_from_ms"]), int(mandate["valid_until_ms"]))
    return bytes(out)


def mandate_digest(mandate: Dict[str, Any]) -> bytes:
    """SHA-256 of :func:`mandate_canonical_bytes`."""
    import hashlib

    return hashlib.sha256(mandate_canonical_bytes(mandate)).digest()


def possession_message(mandate: Dict[str, Any], request: bytes) -> bytes:
    """What a grant's holder signs to prove possession for one call: the tag ``mycelium.mandate/possession/1``, the
    grant's :func:`mandate_digest`, then ``request`` (:func:`~mycelium.a2a.mandate_request_bytes`) as a ``u32``
    little-endian length and its bytes."""
    out = bytearray(b"mycelium.mandate/possession/1")
    out += mandate_digest(mandate)
    _lp(out, request)
    return bytes(out)


class ClientSigner:
    """A service's signer: its client certificate's Ed25519 key and principal (``token:{issuer}/{name}``), an
    instance id drawn once from ``os.urandom``, and a counter. Two processes under one certificate draw different
    instance ids, so their counters never collide.

    ``key`` is the PEM of the key ``mycelium tls issue --client`` wrote, or its 32-byte seed.
    """

    def __init__(self, key: bytes, principal: str, *, instance: Optional[int] = None) -> None:
        try:
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        except ImportError as e:  # pragma: no cover - exercised only without the extra
            raise ImportError("client signatures need the optional dependency: pip install 'mycelium-py[sign]'") from e
        if len(key) == 32:
            self._key = Ed25519PrivateKey.from_private_bytes(key)
        else:
            loaded = serialization.load_pem_private_key(key, password=None)
            if not isinstance(loaded, Ed25519PrivateKey):
                raise ValueError("a client certificate's key is Ed25519")
            self._key = loaded
        self._raw = self._key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        self.principal = principal
        self.instance = instance if instance is not None else int.from_bytes(os.urandom(8), "big")
        self._counter = 0
        # Taken under a lock, so two threads never sign with one counter — a free-threaded build has no GIL to lean on,
        # and a repeated counter is refused at the provider as a replay.
        self._counter_lock = threading.Lock()

    def sign_tools_call(self, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        """Sign a ``tools/call`` of ``name`` (:func:`mcp_binding`, which refuses an unsafe integer)."""
        op, res, digest = mcp_binding(name, arguments)
        return self.sign(op, res, digest)

    def prove_possession(self, grant: Dict[str, Any], operation: str, resource: str, arguments: Any) -> Dict[str, Any]:
        """Present ``grant`` — held by this signer's principal — for one call: ``{"grant": grant, "possession":
        base64}``, to send as ``params._meta.mandate``. The proof binds the grant, the operation, the resource before
        ``@`` and the call's arguments by the gateway's mandate digest (:func:`~mycelium.a2a.arguments_digest`), so
        it is good for this call alone. Raises ``ValueError`` for a grant another principal holds.

        The mandate digest is not RFC 8785: for a float written in exponent form it differs from the gateway's, and
        the call is refused (identity-and-lifecycle §9 O1) — send such a number as a string.
        """
        import base64

        from .a2a import arguments_digest, mandate_request_bytes

        mandate = grant["mandate"]
        if mandate.get("holder") != self.principal:
            raise ValueError(f"the grant is held by {mandate.get('holder')!r}, not {self.principal!r}")
        request = mandate_request_bytes(operation, resource, arguments_digest(arguments))
        proof = self._key.sign(possession_message(mandate, request))
        return {"grant": grant, "possession": base64.b64encode(proof).decode()}

    def sign(self, operation: str, resource: str, digest: bytes, *, issued_at_ms: Optional[int] = None,
             counter: Optional[int] = None) -> Dict[str, Any]:
        """Sign one call; returns ``{"cs": …, "cn": …}`` to merge into ``params._meta``. Each call takes the next
        counter unless one is given."""
        import base64

        t = issued_at_ms if issued_at_ms is not None else int(time.time() * 1000)
        if counter is None:
            with self._counter_lock:
                counter, self._counter = self._counter, self._counter + 1
        msg = client_signature_message(self.principal, operation, resource, digest, t, self.instance, counter)
        sig = self._key.sign(msg)
        return {
            "cs": {"k": base64.b64encode(self._raw).decode(), "sig": base64.b64encode(sig).decode()},
            "cn": {"t": t, "i": f"{self.instance:016x}", "c": counter},
        }
