"""Persistent, keep-alive-pooled httpx clients for the gateway bridge.

Why this exists: the bridge originally opened a fresh ``httpx.Client`` per call
(connect → request → close). Every close leaves a TIME_WAIT socket, and at
Group-scale write rates (~16k rapid KV calls) that exhausts macOS ephemeral
ports. A single persistent client with keep-alive pooling reuses one (or a few)
connections for the whole session.

Async caveat: an ``httpx.AsyncClient``'s connections are bound to the event
loop they were created on, and bridge users routinely run several separate
``asyncio.run(...)`` calls against one long-lived handle object. The pool is
therefore **loop-aware**: it keeps one ``AsyncClient`` per event loop and
lazily evicts clients whose loop has *closed* (the orphan is abandoned to GC —
its loop is dead, so its close coroutine can never run). Eviction checks the
loop, never mere "not the current loop": several threads may each be running a
live loop against one handle concurrently, and evicting a live sibling would
put every borrow back on a fresh client — the per-call regression this module
exists to prevent. All map access is under a ``threading.Lock`` for the same
reason (borrows themselves never hold it across I/O).

Long-lived SSE streams deliberately do NOT use the pool: a stream occupies a
connection for its lifetime, and streams are per-subscription singletons, not
the hot path — pooling them would only starve request/response traffic.
"""

from __future__ import annotations

import asyncio
import os
import threading
from contextlib import asynccontextmanager, contextmanager
from typing import Any, AsyncIterator, Iterator, Optional

import httpx


class _Bound:
    """A borrowed client with an optional per-borrow timeout override.

    Forwards the request methods the bridge uses, injecting ``timeout=`` when
    the borrow carries one — so call sites that used to build a dedicated
    client with a custom timeout keep their exact semantics on the shared one.
    """

    __slots__ = ("_client", "_timeout")

    def __init__(self, client: Any, timeout: Optional[float | object]) -> None:
        self._client = client
        self._timeout = timeout

    def _kw(self, kwargs: dict[str, Any]) -> dict[str, Any]:
        if self._timeout is not _UNSET and "timeout" not in kwargs:
            kwargs["timeout"] = self._timeout
        return kwargs

    def get(self, *a: Any, **kw: Any):
        return self._client.get(*a, **self._kw(kw))

    def post(self, *a: Any, **kw: Any):
        return self._client.post(*a, **self._kw(kw))

    def put(self, *a: Any, **kw: Any):
        return self._client.put(*a, **self._kw(kw))

    def delete(self, *a: Any, **kw: Any):
        return self._client.delete(*a, **self._kw(kw))


_UNSET = object()

#: Environment variable consulted when a handle is constructed without ``token=``.
TOKEN_ENV = "MYCELIUM_GATEWAY_TOKEN"


def resolve_token(token: Optional[str]) -> Optional[str]:
    """Explicit ``token`` wins; then :data:`TOKEN_ENV`; empty strings mean *none*."""
    if token is not None:
        return token or None
    return os.environ.get(TOKEN_ENV) or None


def auth_headers(token: Optional[str]) -> dict[str, str]:
    """``{"Authorization": "Bearer …"}`` for a token, ``{}`` otherwise.

    A node with ``gateway_auth_token`` (or scoped tokens / OIDC) set answers every
    ``/gateway/*`` route — and the node-level ``/mcp``, ``/signals/{kind}``,
    ``/consensus/{slot}`` — with 401 unless this header is present. No token → no
    header: the open (loopback) gateway is unchanged.
    """
    return {"Authorization": f"Bearer {token}"} if token else {}

#: The SDK-wide default request timeout for pooled clients (seconds). The one
#: place to tune it — every handle that doesn't take a user-facing ``timeout``
#: parameter (Wiki/TupleSpace/Blackboard) constructs its pool with this.
DEFAULT_TIMEOUT = 15.0

#: No connection cap. httpx's default (100 total / 20 keep-alive) is sized for
#: request/response traffic; this bridge also carries long-polls (``take()``
#: parks a connection server-side for up to ``timeout_secs``), and a worker
#: fleet with >100 tasks parked on one handle must not have its 101st take
#: queued behind the pool — the per-call clients this pool replaced never
#: capped concurrency, so neither does this. Concurrency stays bounded by the
#: caller's own tasks; idle connections still expire (default 5 s).
_LIMITS = httpx.Limits(max_connections=None, max_keepalive_connections=None)


class ClientPool:
    """Lazily-created persistent httpx clients (one sync; one async per loop)."""

    def __init__(
        self, base_url: str, timeout: float = DEFAULT_TIMEOUT, *, token: Optional[str] = None,
    ) -> None:
        self._base_url = base_url
        self._timeout = timeout
        #: Headers every pooled request carries (the gateway bearer, if any). Handles
        #: that open dedicated clients (SSE streams) pass this to them too.
        self.headers: dict[str, str] = auth_headers(resolve_token(token))
        self._lock = threading.Lock()
        self._sync_client: Optional[httpx.Client] = None
        # id(loop) -> (loop, AsyncClient). Loop-aware (see module doc); at most
        # one entry per thread running a loop (usually exactly one overall).
        # Storing the loop object both enables the is_closed() liveness check
        # and pins the id — a dead loop can't be GC'd and its id reused for a
        # different loop while its entry is still in the map.
        self._async_clients: dict[int, tuple[asyncio.AbstractEventLoop, httpx.AsyncClient]] = {}

    # ── borrows (context managers so call sites keep their `with … as c` shape) ──

    @contextmanager
    def sync(self, timeout: float | None | object = _UNSET) -> Iterator[_Bound]:
        with self._lock:
            if self._sync_client is None or self._sync_client.is_closed:
                self._sync_client = httpx.Client(
                    base_url=self._base_url, timeout=self._timeout, limits=_LIMITS, headers=self.headers
                )
            client = self._sync_client
        yield _Bound(client, timeout)

    @asynccontextmanager
    async def asy(self, timeout: float | None | object = _UNSET) -> AsyncIterator[_Bound]:
        loop = asyncio.get_running_loop()
        key = id(loop)
        with self._lock:  # never held across I/O — map lookup/insert only
            entry = self._async_clients.get(key)
            client = entry[1] if entry is not None else None
            if client is None or client.is_closed:
                # Drop clients whose loops have CLOSED (their sockets are dead).
                # A sibling entry whose loop is still live belongs to another
                # thread mid-flight — leave it alone.
                for k, (l, _c) in list(self._async_clients.items()):
                    if k != key and l.is_closed():
                        self._async_clients.pop(k, None)
                client = httpx.AsyncClient(
                    base_url=self._base_url, timeout=self._timeout, limits=_LIMITS, headers=self.headers
                )
                self._async_clients[key] = (loop, client)
        yield _Bound(client, timeout)

    # ── lifecycle ────────────────────────────────────────────────────────────

    def close(self) -> None:
        """Close the sync client (async clients belong to their loops)."""
        with self._lock:
            client, self._sync_client = self._sync_client, None
        if client is not None:
            client.close()

    async def aclose(self) -> None:
        """Close this loop's async client and the sync client."""
        key = id(asyncio.get_running_loop())
        with self._lock:
            entry = self._async_clients.pop(key, None)
        if entry is not None:
            await entry[1].aclose()
        self.close()


class PoolOwner:
    """Mixin for gateway handles that own a :class:`ClientPool` — the one
    definition of the shared ``aclose()`` surface."""

    _pool: ClientPool

    async def aclose(self) -> None:
        """Close the pooled HTTP client (optional; sockets are reclaimed on exit)."""
        await self._pool.aclose()
