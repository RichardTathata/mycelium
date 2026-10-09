"""
mycelium.artifacts — the artifact catalogue's gateway door (plan A3).

``POST /gateway/artifacts/publish`` takes one **already-signed** catalogue line — the hex a
``mycelium-artifact publish`` step wrote to the library's manifest (or one line of
``mycelium-artifact list``'s source, the manifest file) — verifies its provenance against the
node's trusted publishers, and writes it into the gossiped ``installable/`` catalogue. The
publisher's key never reaches a gateway, and the bytes never ride the request: they live at the
library the librarians mirror. Scope family: ``artifact:publish``.

Refusals are by name (``ArtifactError.kind``): ``unsigned entry``, ``untrusted publisher``,
``provenance does not verify``, ``no trusted publishers configured`` (all 403),
``librarian-managed signer`` (409 — a librarian's manifest is the truth for its key and would
tombstone the line), ``malformed entry`` (400).

Example::

    from mycelium import MyceliumAgent

    with MyceliumAgent("127.0.0.1", 7946, token="...") as agent:
        line = open("library/manifest").read().splitlines()[-1]   # the line CI just published
        receipt = agent.artifacts().publish(line)
        print(receipt["key"], receipt["signer"])
"""

from __future__ import annotations

from typing import Any, Optional

import httpx

from ._pool import ClientPool, PoolOwner, base_url


class ArtifactError(Exception):
    """A refusal from ``/gateway/artifacts/publish``, with the gateway's own name for it."""

    def __init__(self, kind: str, detail: str, *, status: int, body: dict) -> None:
        super().__init__(f"{kind}: {detail}")
        self.kind = kind
        self.detail = detail
        self.status = status
        self.body = body


def _raise(resp: httpx.Response) -> None:
    try:
        body = resp.json()
    except ValueError:
        body = {}
    if not isinstance(body, dict):
        body = {}
    kind = str(body.get("error", f"http-{resp.status_code}"))
    detail = str(body.get("detail", body.get("required_scope", resp.text)))
    raise ArtifactError(kind, detail, status=resp.status_code, body=body)


class Artifacts(PoolOwner):
    """The artifact catalogue verbs on one node's gateway. Get one from
    :meth:`mycelium.MyceliumAgent.artifacts` or construct it against a host and port."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 7946,
        *,
        token: Optional[str] = None,
        scheme: str = "http",
        ca_file: Optional[str] = None,
        _pool: Optional[ClientPool] = None,
    ) -> None:
        self._pool = (
            _pool if _pool is not None
            else ClientPool(base_url(host, port, scheme), token=token, ca_file=ca_file)
        )

    def publish(self, entry_hex: str, *, timeout: Optional[float] = None) -> dict[str, Any]:
        """Publish one signed catalogue line. Returns the gateway's receipt
        (``key``, ``artifact``, ``signer``, ``kind``, ``provides``); raises :class:`ArtifactError`
        with the refusal's name otherwise."""
        with self._pool.sync(timeout) as c:
            r = c.post("/gateway/artifacts/publish", json={"entry_hex": entry_hex.strip()})
        if r.status_code != 200:
            _raise(r)
        return r.json()

    async def apublish(self, entry_hex: str, *, timeout: Optional[float] = None) -> dict[str, Any]:
        """Async :meth:`publish`."""
        async with self._pool.asy(timeout) as c:
            r = await c.post("/gateway/artifacts/publish", json={"entry_hex": entry_hex.strip()})
        if r.status_code != 200:
            _raise(r)
        return r.json()
