"""Persistent, keep-alive-pooled httpx clients for the gateway bridge.

Why this exists: the bridge originally opened a fresh ``httpx.Client`` per call
(connect → request → close). Every close leaves a TIME_WAIT socket, and at
Group-scale write rates (~16k rapid KV calls) that exhausts macOS ephemeral
ports. A single persistent client with keep-alive pooling reuses one (or a few)
connections for the whole session.

Async caveat: an ``httpx.AsyncClient``'s connections are bound to the event
loop they were created on, and bridge users routinely run several separate
``asyncio.run(...)`` calls against one long-lived handle object. The pool is
therefore **loop-aware**: it keeps one ``AsyncClient`` per running loop and
lazily replaces a client whose loop is gone (the orphan is abandoned to GC —
its loop is dead, so its close coroutine can never run).

Long-lived SSE streams deliberately do NOT use the pool: a stream occupies a
connection for its lifetime, and streams are per-subscription singletons, not
the hot path — pooling them would only starve request/response traffic.
"""

from __future__ import annotations

import asyncio
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


class ClientPool:
    """Lazily-created persistent httpx clients (one sync; one async per loop)."""

    def __init__(self, base_url: str, timeout: float) -> None:
        self._base_url = base_url
        self._timeout = timeout
        self._lock = threading.Lock()
        self._sync_client: Optional[httpx.Client] = None
        # id(loop) -> AsyncClient. Loop-aware (see module doc); at most a
        # handful of entries in practice (usually exactly one).
        self._async_clients: dict[int, httpx.AsyncClient] = {}

    # ── borrows (context managers so call sites keep their `with … as c` shape) ──

    @contextmanager
    def sync(self, timeout: float | None | object = _UNSET) -> Iterator[_Bound]:
        with self._lock:
            if self._sync_client is None or self._sync_client.is_closed:
                self._sync_client = httpx.Client(
                    base_url=self._base_url, timeout=self._timeout
                )
            client = self._sync_client
        yield _Bound(client, timeout)

    @asynccontextmanager
    async def asy(self, timeout: float | None | object = _UNSET) -> AsyncIterator[_Bound]:
        loop = asyncio.get_running_loop()
        key = id(loop)
        client = self._async_clients.get(key)
        if client is None or client.is_closed:
            # Drop clients whose loops are gone (their sockets are already dead).
            for k in [k for k, c in self._async_clients.items() if k != key]:
                self._async_clients.pop(k, None)
            client = httpx.AsyncClient(base_url=self._base_url, timeout=self._timeout)
            self._async_clients[key] = client
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
        client = self._async_clients.pop(key, None)
        if client is not None:
            await client.aclose()
        self.close()
