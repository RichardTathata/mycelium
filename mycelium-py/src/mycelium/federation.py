"""
mycelium.federation — the consumer side of federated domains, over the gateway.

A **domain** is one independently admitted mesh. Federation is not two meshes merging:
it is one domain calling a service another domain has explicitly *exported* to it,
over an authenticated edge, with neither mesh learning the other's members. The
membership tables of both sides stay disjoint, by design and under test.

This client drives the verbs your **own** node exposes — it never speaks the
cross-domain protocol itself. That is deliberate: the credential minted for a
federated call is signed with your domain's key and names *you*, and neither the
key nor the trust bundle belongs in an SDK process.

    from mycelium import MyceliumAgent

    agent = MyceliumAgent("127.0.0.1", 7946, token="…")
    fed   = agent.federation()

    fed.connect("partner.example")            # discover: what have they exported to us?
    fed.partners()                            # [{domain, link, last_catalogue}]
    reply = fed.call("partner.example", "invoice.status", "INV-42")

**Who the partner sees.** The credential names *the principal your bearer token
resolved to at your own gateway*, not the node and not a service account — the
gateway never calls a partner as itself on your behalf. On a gateway with no token
model configured that principal is ``anonymous``, which is an honest statement and
usually not the one you want in a partner's evidence: configure tokens.

**Reading a refusal.** Every failure raises a :class:`FederationError`, and the two
fields that matter are not the message:

``sent``
    Did any byte reach the partner? ``False`` means the refusal happened at your own
    gateway — a link that is down, an export not in a fresh catalogue, no free slot,
    a TLS pin that did not match — so nothing ran and a retry is safe.

``delivery``
    ``none`` · ``refused`` · ``completed`` · ``unknown``.

:class:`DeliveryUnknown` is raised for the last of those and is **not** a failure.
It means a gateway went silent and nobody can say whether the call ran. Retrying it
is retrying an effect that may already have happened; that is the caller's decision
to make, not this SDK's, and ``repeatable=True`` is how you tell the substrate you
have already made it.
"""

from __future__ import annotations

from typing import Any, Optional

import httpx

from ._pool import ClientPool


class FederationError(Exception):
    """A federated call did not complete. See the module docstring for ``sent``/``delivery``."""

    def __init__(
        self,
        kind: str,
        detail: str,
        *,
        sent: bool = True,
        delivery: str = "unknown",
        status: int = 0,
        body: Optional[dict] = None,
    ) -> None:
        self.kind = kind
        self.detail = detail
        self.sent = sent
        self.delivery = delivery
        self.status = status
        self.body = body or {}
        super().__init__(f"{kind}: {detail}")

    @property
    def nothing_was_sent(self) -> bool:
        """True when the refusal happened here and no byte crossed — the call is safe to retry.

        Note what this is *not*: a promise that a retry will succeed, only that retrying
        cannot double-run an effect at the partner.
        """
        return not self.sent


class DeliveryUnknown(FederationError):
    """The call may have run. **Not** a negative — see the module docstring.

    ``attempted_via`` names the gateways tried, in order, so an operator can go and look.
    """

    @property
    def attempted_via(self) -> list[str]:
        return list(self.body.get("attempted_via") or [])


def _raise(resp: httpx.Response) -> None:
    """Turn a refusal body into the typed error that says what it means."""
    try:
        body = resp.json()
    except ValueError:
        body = {}
    if not isinstance(body, dict):
        body = {}
    kind = str(body.get("error", f"http-{resp.status_code}"))
    detail = str(body.get("detail", body.get("error", resp.text)))
    sent = bool(body.get("sent", True))
    delivery = str(body.get("delivery", "unknown"))
    cls = DeliveryUnknown if delivery == "unknown" else FederationError
    raise cls(kind, detail, sent=sent, delivery=delivery, status=resp.status_code, body=body)


class Federation:
    """Federation verbs on one node's gateway.

    Get one from :meth:`mycelium.MyceliumAgent.federation` (it shares the agent's
    connection pool and bearer), or construct it directly against a host and port.
    """

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 7946,
        *,
        token: Optional[str] = None,
        _pool: Optional[ClientPool] = None,
    ) -> None:
        self._pool = _pool if _pool is not None else ClientPool(f"http://{host}:{port}", token=token)

    # ── Read (federation:read) ──────────────────────────────────────────────

    def domain(self) -> dict[str, Any]:
        """This node's own federated identity: ``{configured, domain, exports, policy_revision,
        signs_catalogue}``. ``{"configured": False}`` when this gateway serves no domain."""
        with self._pool.sync() as c:
            r = c.get("/gateway/federation/domain")
        if r.status_code != 200:
            _raise(r)
        return r.json()

    def partners(self) -> list[dict[str, Any]]:
        """One row per configured partner: ``{domain, link, last_catalogue}``.

        ``link`` is ``down`` · ``refreshing`` · ``ready``. ``last_catalogue`` is what the last
        successful :meth:`connect` was granted — *remembered, not fresh*. Whether an export may
        be called right now is decided at call time; this is what an operator looks at.
        """
        with self._pool.sync() as c:
            r = c.get("/gateway/federation/partners")
        if r.status_code != 200:
            _raise(r)
        return list(r.json().get("partners", []))

    def catalog(self, domain: str) -> dict[str, Any]:
        """The **last observed** catalogue for one partner — no network, so looking at a partner
        during an outage does not change the link's state. :meth:`connect` is the one that asks."""
        with self._pool.sync() as c:
            r = c.get(f"/gateway/federation/catalog/{domain}")
        if r.status_code != 200:
            _raise(r)
        return r.json()

    # ── Invoke (federation:invoke) ──────────────────────────────────────────

    def connect(self, domain: str, *, timeout: Optional[float] = None) -> list[str]:
        """Fetch a partner's catalogue and bring the link up. Returns the exports **granted to
        us** — which is the grant, not the partner's full export list."""
        with self._pool.sync(timeout) as c:
            r = c.post("/gateway/federation/connect", json={"domain": domain})
        if r.status_code != 200:
            _raise(r)
        return list(r.json().get("exports", []))

    def call(
        self,
        domain: str,
        export: str,
        text: str = "",
        *,
        repeatable: bool = False,
        timeout: Optional[float] = None,
    ) -> str:
        """Invoke one of a partner's exports and return its first text artifact.

        ``repeatable`` is a statement about **your** effect, and it is the only thing that
        decides whether a silent gateway may be retried elsewhere. It defaults to ``False``
        because the safe default for an unstated effect is the one that never runs twice.

        Raises :class:`DeliveryUnknown` when nobody can say whether it ran, and
        :class:`FederationError` otherwise — check ``nothing_was_sent`` before retrying.
        """
        body = {"domain": domain, "export": export, "text": text, "repeatable": repeatable}
        with self._pool.sync(timeout) as c:
            r = c.post("/gateway/federation/call", json=body)
        if r.status_code != 200:
            _raise(r)
        return str(r.json().get("reply", ""))

    # ── Async variants ──────────────────────────────────────────────────────

    async def aconnect(self, domain: str, *, timeout: Optional[float] = None) -> list[str]:
        """Async :meth:`connect`."""
        async with self._pool.asy(timeout) as c:
            r = await c.post("/gateway/federation/connect", json={"domain": domain})
        if r.status_code != 200:
            _raise(r)
        return list(r.json().get("exports", []))

    async def acall(
        self,
        domain: str,
        export: str,
        text: str = "",
        *,
        repeatable: bool = False,
        timeout: Optional[float] = None,
    ) -> str:
        """Async :meth:`call`."""
        body = {"domain": domain, "export": export, "text": text, "repeatable": repeatable}
        async with self._pool.asy(timeout) as c:
            r = await c.post("/gateway/federation/call", json=body)
        if r.status_code != 200:
            _raise(r)
        return str(r.json().get("reply", ""))
