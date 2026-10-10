"""
langgraph_checkpoint_mycelium.saver — a LangGraph checkpointer backed by the
Mycelium mesh.

Storage split (binding — ``docs/plans/mycelium-reason.md``, 2026-07-07/08 addenda):

* **Metadata / index → gossiped KV** (small rows only), via the node's HTTP
  gateway KV endpoints. One row per checkpoint under
  ``ckpt/{thread_id}/{checkpoint_ns}/{checkpoint_id}`` and one per pending write
  under ``ckptw/{thread_id}/{checkpoint_ns}/{checkpoint_id}/{task_id}/{idx}``.
  Rows carry the checkpoint metadata inline (source / step / parents — small),
  so :meth:`MyceliumCheckpointSaver.list` filters without fetching any payload.
* **Payloads → the content-addressed blob tier** (``PUT /gateway/reason/blob``,
  ``GET /gateway/reason/blob/{id}``): the checkpoint skeleton (the ``Checkpoint``
  dict minus ``channel_values``) is one blob, and **each channel value is its
  own blob** — content addressing (a blob's id is its SHA-256) dedups unchanged
  channel values across super-steps for free, the real cost issue for chatty
  graphs. Blobs never enter KV: KV floods every node and is size-gated.

Key encoding: every path segment is percent-encoded (``quote(seg, safe="")``)
so thread ids and namespaces cannot forge ``/`` separators; the **empty**
``checkpoint_ns`` (LangGraph's default namespace is ``""``) is encoded as the
sentinel segment ``__root__`` — which is therefore reserved as a namespace name.

Latest-checkpoint resolution: LangGraph checkpoint ids are UUID6 — fixed-width
hex with the timestamp in the most significant bits, so **lexicographic order is
chronological order** (verified against ``langgraph.checkpoint.base.id.uuid6``);
``get_tuple`` with no ``checkpoint_id`` picks the lexicographic max key.

Incomplete checkpoints: a checkpoint whose row is visible but one of whose referenced blobs
(skeleton, a channel value, a pending write) cannot be fetched yet raises
:class:`IncompleteCheckpoint` from ``get_tuple`` / ``aget_tuple`` / ``list`` / ``alist`` —
retriable, naming the missing blob ids. ``None`` means only that no checkpoint row exists.

Consistency, stated honestly: KV rows are gossip-replicated (eventual);
read-your-writes holds only against the *same* node's gateway. Cross-node
readers poll for convergence. Payload blobs are fetched through the gateway's
local-then-mesh path, so a checkpoint whose row has gossiped in is fetchable
from any node that can reach a ``reason/blob-cache`` provider. One blob is
capped at 8 MiB (the v1 single-frame mesh-fetch ceiling).
"""

from __future__ import annotations

import json
import os
import ssl
from collections.abc import AsyncIterator, Iterator, Sequence
from typing import Any
from urllib.parse import quote, unquote

import httpx
from langchain_core.runnables import RunnableConfig
from langgraph.checkpoint.base import (
    WRITES_IDX_MAP,
    BaseCheckpointSaver,
    ChannelVersions,
    Checkpoint,
    CheckpointMetadata,
    CheckpointTuple,
    SerializerProtocol,
    get_checkpoint_id,
    get_checkpoint_metadata,
)

class IncompleteCheckpoint(Exception):
    """A checkpoint exists, but a payload it references cannot be fetched yet.

    The index row gossips ahead of the blobs, so a reader on another node can see a checkpoint
    whose skeleton, channel values or pending writes are not yet reachable. That is **neither** a
    missing checkpoint **nor** a smaller one: returning ``None`` made LangGraph start the thread
    over, and dropping a pending write made it re-run a task that had already completed (and could
    lose an ``__error__``/``__interrupt__``/``__resume__`` write). Retry once the blobs propagate.

    ``missing`` lists the content ids that could not be fetched, and ``reasons`` says why for each
    (0.3.0 — the plan's "absence, temporary unavailability, authorization refusal and corrupt content
    stay distinguishable"): ``"not_found"`` (no reachable holder has it yet), ``"unavailable"`` (a
    holder could not be reached, the node or a proxy in front of it failed, or the read was throttled),
    ``"unauthorized"`` (the gateway refused the read — a token or scope problem), ``"refused"`` (0.3.1: the blob
    route's own 403 — every holder refused for good, a removed member or a denied action), ``"corrupt"`` (every copy
    currently on offer — the node's own and each advertised provider's — fails the content address),
    ``"unsupported"`` (the node does not serve the blob route — no reason companion). ``retriable`` is
    true only when every reason is transient (``not_found`` or ``unavailable``).
    """

    retriable = True  # the class default; an instance says whether *its* reasons are all transient
    TRANSIENT = frozenset({"not_found", "unavailable"})

    def __init__(
        self,
        thread_id: str,
        checkpoint_ns: str,
        checkpoint_id: str,
        missing: list[str],
        reasons: dict[str, str] | None = None,
    ):
        self.thread_id = thread_id
        self.checkpoint_ns = checkpoint_ns
        self.checkpoint_id = checkpoint_id
        self.missing = missing
        self.reasons = reasons if reasons is not None else {m: "not_found" for m in missing}
        self.retriable = all(r in self.TRANSIENT for r in self.reasons.values())
        why = ", ".join(f"{m[:12]} {self.reasons.get(m, 'not_found')}" for m in missing)
        super().__init__(
            f"checkpoint {checkpoint_id} of thread {thread_id!r} (ns {checkpoint_ns!r}) is incomplete: "
            f"{len(missing)} referenced blob(s) not fetchable ({why}); "
            + ("retry" if self.retriable else "retrying will not fix this")
        )


def _route_error(resp: httpx.Response) -> str | None:
    """The blob route's own ``{"error": …}`` word, or ``None`` if the body is not the route's."""
    try:
        body = resp.json()
    except ValueError:
        return None
    return body.get("error") if isinstance(body, dict) and isinstance(body.get("error"), str) else None


def _blob_reason(resp: httpx.Response) -> str | None:
    """Why the blob route did not serve a blob (``None`` = it did, or the status is not a miss).

    The status alone is not enough: a reverse proxy answers 502 with its own page when the node is down,
    and a node without the reason companion answers a bare 404 — so ``corrupt`` and ``not_found`` are
    read only from the route's own body (``mycelium-reason`` 0.7.0+).
    """
    status = resp.status_code
    if status == 200:
        return None
    if status == 404:
        return "not_found" if _route_error(resp) == "not_found" else "unsupported"
    if status == 403 and _route_error(resp) == "refused":
        return "refused"  # the route's own: every holder refused for good (mycelium-reason 0.7.1)
    if status in (401, 403):
        return "unauthorized"
    if status == 502 and _route_error(resp) == "corrupt":
        return "corrupt"
    if status >= 500 or status in (408, 429):
        return "unavailable"
    return None  # a client error: let raise_for_status report it


# How serious a reason is: a blob fetched more than once in one read keeps its most serious reason, so
# ``retriable`` does not depend on which fetch came last.
_SEVERITY = {"not_found": 1, "unavailable": 2, "corrupt": 3, "unauthorized": 3, "refused": 3, "unsupported": 3}


def _merge_reason(reasons: dict[str, str], blob_id: str, reason: str) -> None:
    held = reasons.get(blob_id)
    if held is None or _SEVERITY.get(reason, 3) > _SEVERITY.get(held, 3):
        reasons[blob_id] = reason


#: The URL schemes the saver accepts (``https`` for a gateway serving TLS — ``gateway_tls`` — or a
#: TLS-terminating proxy; ``http`` is the default, so an existing deployment is unchanged).
SCHEMES = ("http", "https")


def _base_url(host: str, port: int, scheme: str) -> str:
    """``"{scheme}://{host}:{port}"`` — the Python SDK's rule (``mycelium._pool.base_url``), mirrored
    here because this package depends on httpx only: the scheme case-insensitive, ``None`` or a
    non-string a ``ValueError``, an IPv6 literal host bracketed once (``::1`` → ``[::1]``).
    Before 0.3.2 ``http://`` was hard-coded."""
    if not isinstance(scheme, str) or scheme.lower() not in SCHEMES:
        raise ValueError(f"scheme must be one of {SCHEMES}, not {scheme!r}")
    if ":" in host and not host.startswith("["):
        host = f"[{host}]"
    return f"{scheme.lower()}://{host}:{port}"


#: Environment variable consulted when the saver is constructed without ``token=`` — the Python
#: SDK's (``mycelium._pool.TOKEN_ENV``), so one variable serves both.
TOKEN_ENV = "MYCELIUM_GATEWAY_TOKEN"


def _auth_headers(token: str | None) -> dict[str, str]:
    """The Python SDK's rule, mirrored: an explicit ``token`` wins, then :data:`TOKEN_ENV`, and an
    empty string means *none*. ``{"Authorization": "Bearer …"}`` or ``{}`` — the token travels in
    this header only, never in a URL, and the saver keeps no attribute that would show it in a
    ``repr`` or an error (httpx redacts the header in its own)."""
    resolved = (token or None) if token is not None else (os.environ.get(TOKEN_ENV) or None)
    return {"Authorization": f"Bearer {resolved}"} if resolved else {}


def _ssl_verify(ca_file: str | None) -> ssl.SSLContext | bool:
    """httpx's ``verify=``: the system trust store, or a default context that trusts a private fleet
    CA's PEM. Chain and hostname verification stay on either way; a missing file is refused here,
    and an empty ``ca_file`` means *none* (``create_default_context(cafile="")`` would load nothing
    silently)."""
    return True if not ca_file else ssl.create_default_context(cafile=ca_file)


CKPT_PREFIX  = "ckpt"
WRITE_PREFIX = "ckptw"
ROOT_NS      = "__root__"   # sentinel for checkpoint_ns == "" (reserved)


def _seg(s: str) -> str:
    """Encode one key path segment (empty ns → the ``__root__`` sentinel)."""
    return ROOT_NS if s == "" else quote(s, safe="")


def _unseg(s: str) -> str:
    """Decode one key path segment."""
    return "" if s == ROOT_NS else unquote(s)


class MyceliumCheckpointSaver(BaseCheckpointSaver[int]):
    """A :class:`BaseCheckpointSaver` whose storage is a Mycelium mesh node.

    One-line swap::

        graph = builder.compile(checkpointer=MyceliumCheckpointSaver("127.0.0.1", 8101))

    Checkpoint metadata gossips to every node; payloads live in the
    content-addressed blob tier and are fetched from whichever peer holds them —
    so a saver pointed at node B's gateway resumes a thread checkpointed via
    node A once the metadata has gossiped in (the cross-node resume property).

    :param host:    Hostname or IP of the Mycelium HTTP gateway.
    :param port:    HTTP port of the gateway.
    :param timeout: Default request timeout in seconds.
    :param serde:   Serializer (defaults to LangGraph's ``JsonPlusSerializer``).
    :param token:   Gateway bearer, sent as ``Authorization: Bearer`` on both clients; when
                    ``None``, ``MYCELIUM_GATEWAY_TOKEN`` is used, as the Python SDK does (0.3.2 —
                    before it the saver could not present a bearer at all, so a token-protected
                    gateway answered 401 to every call).
    :param scheme:  ``"http"`` (default) or ``"https"`` for a gateway serving TLS (0.3.2).
    :param ca_file: PEM bundle of a private fleet CA to trust instead of the system store
                    (verification stays on; 0.3.2).

    Under scoped tokens the saver needs ``kv:read`` and ``kv:write`` (the index rows) and
    ``llm:read`` and ``llm:write`` (the blob tier — every ``put()`` writes blobs through
    ``PUT /gateway/reason/blob``, which is ``llm:write``).
    """

    def __init__(
        self,
        host: str,
        port: int = 8080,
        *,
        timeout: float = 30.0,
        serde: SerializerProtocol | None = None,
        token: str | None = None,
        scheme: str = "http",
        ca_file: str | None = None,
    ) -> None:
        super().__init__(serde=serde)
        self._base = _base_url(host, port, scheme)
        verify = _ssl_verify(ca_file)
        headers = _auth_headers(token)
        self._client  = httpx.Client(base_url=self._base, timeout=timeout, verify=verify, headers=headers)
        self._aclient = httpx.AsyncClient(base_url=self._base, timeout=timeout, verify=verify, headers=headers)

    # ── Lifecycle ────────────────────────────────────────────────────────────

    def close(self) -> None:
        """Close the underlying sync HTTP client."""
        self._client.close()

    async def aclose(self) -> None:
        """Close both underlying HTTP clients."""
        self._client.close()
        await self._aclient.aclose()

    def __enter__(self) -> "MyceliumCheckpointSaver":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    async def __aenter__(self) -> "MyceliumCheckpointSaver":
        return self

    async def __aexit__(self, *_: Any) -> None:
        await self.aclose()

    # ── Gateway primitives (sync) ────────────────────────────────────────────

    def _kv_get(self, key: str) -> bytes | None:
        import base64
        resp = self._client.get("/gateway/kv", params={"key": key})
        resp.raise_for_status()
        data = resp.json()
        if not data.get("found"):
            return None
        return base64.b64decode(data.get("value_b64", ""))

    def _kv_set(self, key: str, value: bytes) -> None:
        import base64
        body = {"key": key, "value_b64": base64.b64encode(value).decode()}
        self._client.post("/gateway/kv", json=body).raise_for_status()

    def _kv_del(self, key: str) -> None:
        self._client.delete("/gateway/kv", params={"key": key}).raise_for_status()

    def _kv_keys(self, prefix: str) -> list[str]:
        resp = self._client.get("/gateway/kv/keys", params={"prefix": prefix})
        resp.raise_for_status()
        return resp.json()["keys"]

    def _blob_put(self, data: bytes) -> str:
        resp = self._client.put("/gateway/reason/blob", content=data)
        resp.raise_for_status()
        return resp.json()["id"]

    def _blob_get(self, blob_id: str) -> bytes | None:
        return self._blob_try(blob_id)

    def _blob_try(self, blob_id: str, reasons: dict[str, str] | None = None) -> bytes | None:
        """The blob, or ``None`` with why recorded in ``reasons`` (``IncompleteCheckpoint.reasons``)."""
        try:
            resp = self._client.get(f"/gateway/reason/blob/{blob_id}")
        except httpx.TransportError:
            if reasons is not None:
                _merge_reason(reasons, blob_id, "unavailable")
            return None
        reason = _blob_reason(resp)
        if reason is not None:
            if reasons is not None:
                _merge_reason(reasons, blob_id, reason)
            return None
        resp.raise_for_status()
        return resp.content

    # ── Gateway primitives (async) ───────────────────────────────────────────

    async def _akv_get(self, key: str) -> bytes | None:
        import base64
        resp = await self._aclient.get("/gateway/kv", params={"key": key})
        resp.raise_for_status()
        data = resp.json()
        if not data.get("found"):
            return None
        return base64.b64decode(data.get("value_b64", ""))

    async def _akv_set(self, key: str, value: bytes) -> None:
        import base64
        body = {"key": key, "value_b64": base64.b64encode(value).decode()}
        (await self._aclient.post("/gateway/kv", json=body)).raise_for_status()

    async def _akv_del(self, key: str) -> None:
        (await self._aclient.delete("/gateway/kv", params={"key": key})).raise_for_status()

    async def _akv_keys(self, prefix: str) -> list[str]:
        resp = await self._aclient.get("/gateway/kv/keys", params={"prefix": prefix})
        resp.raise_for_status()
        return resp.json()["keys"]

    async def _ablob_put(self, data: bytes) -> str:
        resp = await self._aclient.put("/gateway/reason/blob", content=data)
        resp.raise_for_status()
        return resp.json()["id"]

    async def _ablob_get(self, blob_id: str) -> bytes | None:
        return await self._ablob_try(blob_id)

    async def _ablob_try(self, blob_id: str, reasons: dict[str, str] | None = None) -> bytes | None:
        """Async :meth:`_blob_try`."""
        try:
            resp = await self._aclient.get(f"/gateway/reason/blob/{blob_id}")
        except httpx.TransportError:
            if reasons is not None:
                _merge_reason(reasons, blob_id, "unavailable")
            return None
        reason = _blob_reason(resp)
        if reason is not None:
            if reasons is not None:
                _merge_reason(reasons, blob_id, reason)
            return None
        resp.raise_for_status()
        return resp.content

    # ── Pure key / row helpers (shared by sync and async paths) ─────────────

    @staticmethod
    def _ckpt_key(thread_id: str, checkpoint_ns: str, checkpoint_id: str) -> str:
        return f"{CKPT_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}/{_seg(checkpoint_id)}"

    @staticmethod
    def _write_key(
        thread_id: str, checkpoint_ns: str, checkpoint_id: str, task_id: str, idx: int
    ) -> str:
        return (
            f"{WRITE_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}"
            f"/{_seg(checkpoint_id)}/{_seg(task_id)}/{idx}"
        )

    def _make_row(
        self,
        config: RunnableConfig,
        metadata: CheckpointMetadata,
        skeleton_blob: str,
        skeleton_type: str,
        channels: dict[str, list[str]],
    ) -> bytes:
        row = {
            "v": 1,
            "blob": skeleton_blob,
            "type": skeleton_type,
            "channels": channels,
            "parent": config["configurable"].get("checkpoint_id"),
            "metadata": get_checkpoint_metadata(config, metadata),
        }
        return json.dumps(row).encode()

    @staticmethod
    def _config_for(thread_id: str, checkpoint_ns: str, checkpoint_id: str | None) -> RunnableConfig | None:
        if not checkpoint_id:
            return None
        return {
            "configurable": {
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint_id,
            }
        }

    def _assemble(
        self,
        thread_id: str,
        checkpoint_ns: str,
        checkpoint_id: str,
        row: dict[str, Any],
        skeleton_bytes: bytes,
        channel_bytes: dict[str, bytes],
        writes: list[tuple[str, int, dict[str, Any], bytes]],
        config: RunnableConfig | None = None,
    ) -> CheckpointTuple:
        """Build a CheckpointTuple from already-fetched pieces (pure)."""
        checkpoint: Checkpoint = self.serde.loads_typed((row["type"], skeleton_bytes))
        checkpoint["channel_values"] = {
            ch: self.serde.loads_typed((row["channels"][ch][1], data))
            for ch, data in channel_bytes.items()
        }
        pending = [
            (task_id, wrow["channel"], self.serde.loads_typed((wrow["type"], data)))
            for task_id, _idx, wrow, data in sorted(writes, key=lambda w: (w[0], w[1]))
        ]
        return CheckpointTuple(
            config=config
            or {
                "configurable": {
                    "thread_id": thread_id,
                    "checkpoint_ns": checkpoint_ns,
                    "checkpoint_id": checkpoint_id,
                }
            },
            checkpoint=checkpoint,
            metadata=row.get("metadata") or {},
            parent_config=self._config_for(thread_id, checkpoint_ns, row.get("parent")),
            pending_writes=pending,
        )

    @staticmethod
    def _parse_write_key(key: str) -> tuple[str, int] | None:
        """``ckptw/…/{task_id}/{idx}`` → (task_id, idx); None on malformed keys."""
        parts = key.split("/")
        if len(parts) != 6:
            return None
        try:
            return _unseg(parts[4]), int(parts[5])
        except ValueError:
            return None

    # ── Sync read path ───────────────────────────────────────────────────────

    def _read_tuple(
        self,
        thread_id: str,
        checkpoint_ns: str,
        checkpoint_id: str,
        row: dict[str, Any] | None = None,
        config: RunnableConfig | None = None,
    ) -> CheckpointTuple | None:
        if row is None:
            raw = self._kv_get(self._ckpt_key(thread_id, checkpoint_ns, checkpoint_id))
            if raw is None:
                return None
            row = json.loads(raw)
        # The row exists, so this checkpoint exists: a payload that cannot be fetched makes it
        # *incomplete*, never absent or smaller (realignment repairs S5).
        missing: list[str] = []
        reasons: dict[str, str] = {}
        skeleton = self._blob_try(row["blob"], reasons)
        if skeleton is None:
            missing.append(row["blob"])
        channel_bytes: dict[str, bytes] = {}
        for ch, (blob_id, _type) in row.get("channels", {}).items():
            data = self._blob_try(blob_id, reasons)
            if data is None:
                missing.append(blob_id)
            else:
                channel_bytes[ch] = data
        writes: list[tuple[str, int, dict[str, Any], bytes]] = []
        wprefix = f"{WRITE_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}/{_seg(checkpoint_id)}/"
        for key in self._kv_keys(wprefix):
            parsed = self._parse_write_key(key)
            raw = self._kv_get(key)
            if parsed is None or raw is None:
                continue  # a malformed key, or a row deleted since the listing: not part of it
            wrow = json.loads(raw)
            data = self._blob_try(wrow["blob"], reasons)
            if data is None:
                missing.append(wrow["blob"])
                continue
            writes.append((parsed[0], parsed[1], wrow, data))
        if missing:
            raise IncompleteCheckpoint(thread_id, checkpoint_ns, checkpoint_id, missing, reasons)
        assert skeleton is not None
        return self._assemble(
            thread_id, checkpoint_ns, checkpoint_id, row, skeleton, channel_bytes, writes, config
        )

    def get_tuple(self, config: RunnableConfig) -> CheckpointTuple | None:
        """Fetch one checkpoint tuple — by id, or the latest for the thread/ns."""
        thread_id: str = config["configurable"]["thread_id"]
        checkpoint_ns: str = config["configurable"].get("checkpoint_ns", "")
        if checkpoint_id := get_checkpoint_id(config):
            return self._read_tuple(thread_id, checkpoint_ns, checkpoint_id, config=config)
        prefix = f"{CKPT_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}/"
        keys = self._kv_keys(prefix)
        if not keys:
            return None
        # UUID6 ids: lexicographic max == latest (see module doc).
        latest = _unseg(max(keys).rsplit("/", 1)[1])
        return self._read_tuple(thread_id, checkpoint_ns, latest)

    def list(
        self,
        config: RunnableConfig | None,
        *,
        filter: dict[str, Any] | None = None,
        before: RunnableConfig | None = None,
        limit: int | None = None,
    ) -> Iterator[CheckpointTuple]:
        """List checkpoints, newest first. Metadata filtering happens on the KV
        rows alone — payload blobs are fetched only for yielded tuples."""
        for thread_id, checkpoint_ns, checkpoint_id, row in self._list_rows(config, filter, before, limit):
            tup = self._read_tuple(thread_id, checkpoint_ns, checkpoint_id, row=row)
            if tup is not None:
                yield tup

    # ── Row selection (shared by list / alist) ───────────────────────────────
    #
    # The pure half — which KV prefix to scan, which keys fall inside the
    # request's (ns, id, before) window, which rows pass the metadata filter — is
    # factored out so the sync and async drivers below are the same code modulo
    # `await`. Review 2026-09-05 (finding 5): `alist` used to run the sync driver,
    # so every `kv/keys` scan and per-row `kv` GET blocked the event loop; the
    # payload half was awaited but row selection was not. `_alist_rows` now uses
    # the async client end to end. The two drivers are gated for parity by
    # `tests/test_alist_async.py`.

    @staticmethod
    def _row_window(
        config: RunnableConfig | None,
        before: RunnableConfig | None,
    ) -> tuple[str, Any]:
        """Returns ``(prefix, select)``: the KV prefix to scan and a selector
        mapping a key to ``(thread, ns, id)`` — or ``None`` when the key is
        outside the request's namespace / checkpoint-id / ``before`` window."""
        config_ns = config["configurable"].get("checkpoint_ns") if config else None
        config_id = get_checkpoint_id(config) if config else None
        before_id = get_checkpoint_id(before) if before else None
        if config:
            prefix = f"{CKPT_PREFIX}/{_seg(config['configurable']['thread_id'])}/"
        else:
            prefix = f"{CKPT_PREFIX}/"

        def select(key: str) -> tuple[str, str, str] | None:
            parts = key.split("/")
            if len(parts) != 4:
                return None
            thread_id, checkpoint_ns, checkpoint_id = (
                _unseg(parts[1]), _unseg(parts[2]), _unseg(parts[3]),
            )
            if config_ns is not None and checkpoint_ns != config_ns:
                return None
            if config_id and checkpoint_id != config_id:
                return None
            if before_id and checkpoint_id >= before_id:
                return None
            return thread_id, checkpoint_ns, checkpoint_id

        return prefix, select

    @staticmethod
    def _row_if_matches(raw: bytes, filter: dict[str, Any] | None) -> dict[str, Any] | None:
        """Decodes a checkpoint row and applies the metadata ``filter``;
        ``None`` for an undecodable row or a filter miss."""
        try:
            row = json.loads(raw)
        except ValueError:
            return None
        metadata = row.get("metadata") or {}
        if filter and not all(v == metadata.get(k) for k, v in filter.items()):
            return None
        return row

    def _list_rows(
        self,
        config: RunnableConfig | None,
        filter: dict[str, Any] | None,
        before: RunnableConfig | None,
        limit: int | None,
    ) -> Iterator[tuple[str, str, str, dict[str, Any]]]:
        """The row-selection half of :meth:`list`: yields ``(thread, ns, id, row)``
        after all KV-side filtering, newest first. Sync client throughout."""
        prefix, select = self._row_window(config, before)
        remaining = limit
        # Newest first within each (thread, ns): full-key descending sort gives
        # descending checkpoint ids per prefix group (fixed-width UUID6).
        for key in sorted(self._kv_keys(prefix), reverse=True):
            selected = select(key)
            if selected is None:
                continue
            raw = self._kv_get(key)
            if raw is None:
                continue  # tombstoned between keys() and get()
            row = self._row_if_matches(raw, filter)
            if row is None:
                continue
            if remaining is not None:
                if remaining <= 0:
                    return
                remaining -= 1
            yield (*selected, row)

    async def _alist_rows(
        self,
        config: RunnableConfig | None,
        filter: dict[str, Any] | None,
        before: RunnableConfig | None,
        limit: int | None,
    ) -> AsyncIterator[tuple[str, str, str, dict[str, Any]]]:
        """Async :meth:`_list_rows` — the same selection over the async client,
        so :meth:`alist` never blocks the event loop on key enumeration or row
        reads (review 2026-09-05, finding 5)."""
        prefix, select = self._row_window(config, before)
        remaining = limit
        for key in sorted(await self._akv_keys(prefix), reverse=True):
            selected = select(key)
            if selected is None:
                continue
            raw = await self._akv_get(key)
            if raw is None:
                continue  # tombstoned between keys() and get()
            row = self._row_if_matches(raw, filter)
            if row is None:
                continue
            if remaining is not None:
                if remaining <= 0:
                    return
                remaining -= 1
            yield (*selected, row)

    # ── Sync write path ──────────────────────────────────────────────────────

    def put(
        self,
        config: RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
        new_versions: ChannelVersions,
    ) -> RunnableConfig:
        """Store a checkpoint: every payload becomes a content-addressed blob
        (unchanged channel values re-hash to the same id — free dedup), then one
        small index row enters gossiped KV."""
        thread_id = config["configurable"]["thread_id"]
        checkpoint_ns = config["configurable"].get("checkpoint_ns", "")
        c = checkpoint.copy()
        values: dict[str, Any] = c.pop("channel_values")  # type: ignore[misc]
        skeleton_type, skeleton_data = self.serde.dumps_typed(c)
        skeleton_blob = self._blob_put(skeleton_data)
        channels: dict[str, list[str]] = {}
        for ch, val in values.items():
            vtype, vdata = self.serde.dumps_typed(val)
            channels[ch] = [self._blob_put(vdata), vtype]
        self._kv_set(
            self._ckpt_key(thread_id, checkpoint_ns, checkpoint["id"]),
            self._make_row(config, metadata, skeleton_blob, skeleton_type, channels),
        )
        return {
            "configurable": {
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint["id"],
            }
        }

    def put_writes(
        self,
        config: RunnableConfig,
        writes: Sequence[tuple[str, Any]],
        task_id: str,
        task_path: str = "",
    ) -> None:
        """Store pending writes: one blob + one small KV row per write.

        Writes are LWW-idempotent by key (``…/{task_id}/{idx}``), so a retried
        super-step overwrites each row with identical content instead of the
        reference implementation's skip-if-present check — same net state,
        no read-before-write round-trip.
        """
        thread_id = config["configurable"]["thread_id"]
        checkpoint_ns = config["configurable"].get("checkpoint_ns", "")
        checkpoint_id = config["configurable"]["checkpoint_id"]
        for idx, (channel, value) in enumerate(writes):
            vtype, vdata = self.serde.dumps_typed(value)
            row = {
                "v": 1,
                "channel": channel,
                "blob": self._blob_put(vdata),
                "type": vtype,
                "task_path": task_path,
            }
            key = self._write_key(
                thread_id, checkpoint_ns, checkpoint_id, task_id, WRITES_IDX_MAP.get(channel, idx)
            )
            self._kv_set(key, json.dumps(row).encode())

    def delete_thread(self, thread_id: str) -> None:
        """Tombstone every index row for the thread (all namespaces).

        Payload blobs stay in the content-addressed tier: they may be shared
        with other threads (dedup) and unreferenced blobs are a GC concern,
        not a correctness one.
        """
        for prefix in (CKPT_PREFIX, WRITE_PREFIX):
            for key in self._kv_keys(f"{prefix}/{_seg(thread_id)}/"):
                self._kv_del(key)

    # ── Async variants ───────────────────────────────────────────────────────

    async def _aread_tuple(
        self,
        thread_id: str,
        checkpoint_ns: str,
        checkpoint_id: str,
        row: dict[str, Any] | None = None,
        config: RunnableConfig | None = None,
    ) -> CheckpointTuple | None:
        if row is None:
            raw = await self._akv_get(self._ckpt_key(thread_id, checkpoint_ns, checkpoint_id))
            if raw is None:
                return None
            row = json.loads(raw)
        # The same rule as the sync loader: present but unfetchable is *incomplete* (S5).
        missing: list[str] = []
        reasons: dict[str, str] = {}
        skeleton = await self._ablob_try(row["blob"], reasons)
        if skeleton is None:
            missing.append(row["blob"])
        channel_bytes: dict[str, bytes] = {}
        for ch, (blob_id, _type) in row.get("channels", {}).items():
            data = await self._ablob_try(blob_id, reasons)
            if data is None:
                missing.append(blob_id)
            else:
                channel_bytes[ch] = data
        writes: list[tuple[str, int, dict[str, Any], bytes]] = []
        wprefix = f"{WRITE_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}/{_seg(checkpoint_id)}/"
        for key in await self._akv_keys(wprefix):
            parsed = self._parse_write_key(key)
            raw = await self._akv_get(key)
            if parsed is None or raw is None:
                continue
            wrow = json.loads(raw)
            data = await self._ablob_try(wrow["blob"], reasons)
            if data is None:
                missing.append(wrow["blob"])
                continue
            writes.append((parsed[0], parsed[1], wrow, data))
        if missing:
            raise IncompleteCheckpoint(thread_id, checkpoint_ns, checkpoint_id, missing, reasons)
        assert skeleton is not None
        return self._assemble(
            thread_id, checkpoint_ns, checkpoint_id, row, skeleton, channel_bytes, writes, config
        )

    async def aget_tuple(self, config: RunnableConfig) -> CheckpointTuple | None:
        """Async :meth:`get_tuple`."""
        thread_id: str = config["configurable"]["thread_id"]
        checkpoint_ns: str = config["configurable"].get("checkpoint_ns", "")
        if checkpoint_id := get_checkpoint_id(config):
            return await self._aread_tuple(thread_id, checkpoint_ns, checkpoint_id, config=config)
        prefix = f"{CKPT_PREFIX}/{_seg(thread_id)}/{_seg(checkpoint_ns)}/"
        keys = await self._akv_keys(prefix)
        if not keys:
            return None
        latest = _unseg(max(keys).rsplit("/", 1)[1])
        return await self._aread_tuple(thread_id, checkpoint_ns, latest)

    async def alist(
        self,
        config: RunnableConfig | None,
        *,
        filter: dict[str, Any] | None = None,
        before: RunnableConfig | None = None,
        limit: int | None = None,
    ) -> AsyncIterator[CheckpointTuple]:
        """Async :meth:`list` — row selection *and* payload fetches on the
        async client; nothing here blocks the event loop."""
        async for thread_id, checkpoint_ns, checkpoint_id, row in self._alist_rows(
            config, filter, before, limit
        ):
            tup = await self._aread_tuple(thread_id, checkpoint_ns, checkpoint_id, row=row)
            if tup is not None:
                yield tup

    async def aput(
        self,
        config: RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
        new_versions: ChannelVersions,
    ) -> RunnableConfig:
        """Async :meth:`put`."""
        thread_id = config["configurable"]["thread_id"]
        checkpoint_ns = config["configurable"].get("checkpoint_ns", "")
        c = checkpoint.copy()
        values: dict[str, Any] = c.pop("channel_values")  # type: ignore[misc]
        skeleton_type, skeleton_data = self.serde.dumps_typed(c)
        skeleton_blob = await self._ablob_put(skeleton_data)
        channels: dict[str, list[str]] = {}
        for ch, val in values.items():
            vtype, vdata = self.serde.dumps_typed(val)
            channels[ch] = [await self._ablob_put(vdata), vtype]
        await self._akv_set(
            self._ckpt_key(thread_id, checkpoint_ns, checkpoint["id"]),
            self._make_row(config, metadata, skeleton_blob, skeleton_type, channels),
        )
        return {
            "configurable": {
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint["id"],
            }
        }

    async def aput_writes(
        self,
        config: RunnableConfig,
        writes: Sequence[tuple[str, Any]],
        task_id: str,
        task_path: str = "",
    ) -> None:
        """Async :meth:`put_writes`."""
        thread_id = config["configurable"]["thread_id"]
        checkpoint_ns = config["configurable"].get("checkpoint_ns", "")
        checkpoint_id = config["configurable"]["checkpoint_id"]
        for idx, (channel, value) in enumerate(writes):
            vtype, vdata = self.serde.dumps_typed(value)
            row = {
                "v": 1,
                "channel": channel,
                "blob": await self._ablob_put(vdata),
                "type": vtype,
                "task_path": task_path,
            }
            key = self._write_key(
                thread_id, checkpoint_ns, checkpoint_id, task_id, WRITES_IDX_MAP.get(channel, idx)
            )
            await self._akv_set(key, json.dumps(row).encode())

    async def adelete_thread(self, thread_id: str) -> None:
        """Async :meth:`delete_thread`."""
        for prefix in (CKPT_PREFIX, WRITE_PREFIX):
            for key in await self._akv_keys(f"{prefix}/{_seg(thread_id)}/"):
                await self._akv_del(key)
