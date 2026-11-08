"""A client's own signature on a gateway call — ``cs`` and ``cn`` (identity-and-lifecycle I1, decision D1′).

A service holding a client certificate (``mycelium tls issue --client <name> --issuer <issuer>``) signs each
``tools/call`` or A2A send with the certificate's key and sends the result in ``params._meta``; the provider that runs
the call checks it — the signature, that the key is the principal's current one, that it covers this call's
operation, resource and arguments, and that its ``cn`` was not seen before.

The signed bytes are :func:`client_signature_message`, pinned by the vector in ``tests/fixtures/cs/vector.json``
that the gateway's Rust tests and the TypeScript SDK check too. Signing needs the optional ``cryptography``
dependency (``pip install mycelium-py[sign]``); building the message does not.
"""

from __future__ import annotations

import os
import struct
import time
from typing import Any, Dict, Optional, Tuple

from .a2a import arguments_digest

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
    """The bytes a client signs: the tag; the principal, the operation and the resource before ``@``, each a ``u32``
    little-endian length and its UTF-8 bytes; the 32-byte arguments digest; then ``issued_at_ms``, ``instance`` and
    ``counter`` as ``u64`` little-endian."""
    if len(digest) != 32:
        raise ValueError("the arguments digest is 32 bytes")
    out = bytearray(TAG)
    for part in (principal, operation, resource.split("@", 1)[0]):
        b = part.encode("utf-8")
        out += struct.pack("<I", len(b)) + b
    out += digest
    out += struct.pack("<QQQ", issued_at_ms, instance, counter)
    return bytes(out)


def mcp_binding(name: str, arguments: Dict[str, Any]) -> Tuple[str, str, bytes]:
    """What a ``tools/call`` of ``name`` signs: ``("tools/call", "tool:{name}", digest(arguments))``."""
    return "tools/call", f"tool:{name}", arguments_digest(arguments)


def a2a_binding(skill_id: str, text: str) -> Tuple[str, str, bytes]:
    """What an A2A send of ``text`` to ``skill_id`` signs: ``("skill.invoke", "skill:{id}", digest({"text": text}))``."""
    return "skill.invoke", f"skill:{skill_id}", arguments_digest({"text": text})


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

    def sign(self, operation: str, resource: str, digest: bytes, *, issued_at_ms: Optional[int] = None,
             counter: Optional[int] = None) -> Dict[str, Any]:
        """Sign one call; returns ``{"cs": …, "cn": …}`` to merge into ``params._meta``. Each call takes the next
        counter unless one is given."""
        import base64

        t = issued_at_ms if issued_at_ms is not None else int(time.time() * 1000)
        if counter is None:
            counter, self._counter = self._counter, self._counter + 1
        msg = client_signature_message(self.principal, operation, resource, digest, t, self.instance, counter)
        sig = self._key.sign(msg)
        return {
            "cs": {"k": base64.b64encode(self._raw).decode(), "sig": base64.b64encode(sig).decode()},
            "cn": {"t": t, "i": f"{self.instance:016x}", "c": counter},
        }
