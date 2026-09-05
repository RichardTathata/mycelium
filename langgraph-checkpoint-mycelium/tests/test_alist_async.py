"""
Unit gates for `alist` row selection (review 2026-09-05, finding 5). No node
required: the gateway primitives are replaced with in-memory fakes.

Two properties:
1. `alist` never touches the sync client — every key scan and row read goes
   through the async primitives (pre-fix it ran `_list_rows`, blocking the loop).
2. `_alist_rows` and `_list_rows` select identical rows for the same request
   across namespace / checkpoint-id / `before` / metadata filter / limit.
"""

from __future__ import annotations

import itertools
import json
from typing import Any

import pytest

from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver
from langgraph_checkpoint_mycelium.saver import CKPT_PREFIX, _seg


# UUID6-shaped fixed-width ids: lexicographic order == chronological order.
def cid(n: int) -> str:
    return f"1ef00000-0000-6000-8000-{n:012x}"


def make_store() -> dict[str, bytes]:
    """Three threads; t-a has two namespaces; a mix of metadata sources."""
    rows: dict[str, bytes] = {}

    def put(thread: str, ns: str, n: int, source: str, step: int) -> None:
        key = f"{CKPT_PREFIX}/{_seg(thread)}/{_seg(ns)}/{_seg(cid(n))}"
        rows[key] = json.dumps({
            "blob": f"blob-{thread}-{ns}-{n}", "channels": {},
            "metadata": {"source": source, "step": step, "parents": {}},
            "parent": None,
        }).encode()

    for n in range(6):
        put("t-a", "", n, "loop" if n % 2 else "input", n)
    for n in range(3):
        put("t-a", "sub", 10 + n, "loop", n)
    for n in range(4):
        put("t-b", "", 20 + n, "update" if n == 2 else "loop", n)
    put("t-c/with slash", "", 30, "input", 0)  # percent-encoded thread id
    rows["ckpt/bad-shape"] = b"{}"              # wrong depth → skipped
    rows[f"{CKPT_PREFIX}/{_seg('t-b')}/{_seg('')}/{_seg(cid(99))}"] = b"not json"
    return rows


class FakeSaver(MyceliumCheckpointSaver):
    """Gateway primitives backed by a dict. Sync primitives record their use so a
    test can assert `alist` never reaches them."""

    def __init__(self, store: dict[str, bytes]) -> None:
        super().__init__("127.0.0.1", 1)  # httpx clients are lazy: no connection
        self.store = store
        self.sync_calls: list[str] = []
        self.async_calls: list[str] = []

    # sync
    def _kv_keys(self, prefix: str) -> list[str]:
        self.sync_calls.append(f"keys {prefix}")
        return [k for k in self.store if k.startswith(prefix)]

    def _kv_get(self, key: str) -> bytes | None:
        self.sync_calls.append(f"get {key}")
        return self.store.get(key)

    # async
    async def _akv_keys(self, prefix: str) -> list[str]:
        self.async_calls.append(f"keys {prefix}")
        return [k for k in self.store if k.startswith(prefix)]

    async def _akv_get(self, key: str) -> bytes | None:
        self.async_calls.append(f"get {key}")
        return self.store.get(key)


def cfg(thread: str, ns: str | None = None, checkpoint_id: str | None = None) -> dict[str, Any]:
    c: dict[str, Any] = {"configurable": {"thread_id": thread}}
    if ns is not None:
        c["configurable"]["checkpoint_ns"] = ns
    if checkpoint_id is not None:
        c["configurable"]["checkpoint_id"] = checkpoint_id
    return c


REQUESTS: list[dict[str, Any]] = [
    dict(config=None),
    dict(config=cfg("t-a")),
    dict(config=cfg("t-a", "")),
    dict(config=cfg("t-a", "sub")),
    dict(config=cfg("t-a", "", cid(3))),
    dict(config=cfg("t-b"), before=cfg("t-b", "", cid(22))),
    dict(config=cfg("t-c/with slash")),
    dict(config=None, filter={"source": "loop"}),
    dict(config=cfg("t-a"), filter={"source": "input", "step": 2}),
    dict(config=cfg("t-b"), filter={"source": "update"}),
    dict(config=None, limit=0),
    dict(config=None, limit=1),
    dict(config=cfg("t-a"), limit=4),
    dict(config=cfg("t-a", ""), before=cfg("t-a", "", cid(4)), filter={"source": "loop"}, limit=1),
]


def _norm(req: dict[str, Any]) -> dict[str, Any]:
    return {"config": req.get("config"), "filter": req.get("filter"),
            "before": req.get("before"), "limit": req.get("limit")}


@pytest.mark.parametrize("req", REQUESTS, ids=[str(i) for i in range(len(REQUESTS))])
async def test_alist_rows_matches_list_rows(req: dict[str, Any]) -> None:
    store = make_store()
    saver = FakeSaver(store)
    kw = _norm(req)
    sync_rows = list(saver._list_rows(**kw))
    async_rows = [r async for r in saver._alist_rows(**kw)]
    assert async_rows == sync_rows, "async row selection must equal the sync selection"
    # The async driver used only async primitives.
    assert saver.async_calls, "async driver made no async calls"


async def test_alist_never_uses_the_sync_client() -> None:
    """The regression itself: pre-fix `alist` ran `_list_rows` on the sync client."""
    saver = FakeSaver(make_store())
    seen: list[tuple[str, str, str]] = []

    async def fake_aread_tuple(thread_id, checkpoint_ns, checkpoint_id, row=None, config=None):
        seen.append((thread_id, checkpoint_ns, checkpoint_id))
        return None  # payload half is out of scope here

    saver._aread_tuple = fake_aread_tuple  # type: ignore[method-assign]
    got = [t async for t in saver.alist(cfg("t-a"), filter={"source": "loop"})]
    assert got == []  # every tuple filtered by the fake payload read
    assert saver.sync_calls == [], f"alist reached the sync client: {saver.sync_calls}"
    assert any(c.startswith("keys ") for c in saver.async_calls)
    assert seen == [("t-a", "sub", cid(12)), ("t-a", "sub", cid(11)), ("t-a", "sub", cid(10)),
                    ("t-a", "", cid(5)), ("t-a", "", cid(3)), ("t-a", "", cid(1))]


def test_sync_list_rows_selection_shape() -> None:
    """Pin the selection semantics both drivers share: newest first per
    (thread, ns), `before` is exclusive, malformed keys/rows are skipped,
    limit=0 yields nothing."""
    saver = FakeSaver(make_store())
    rows = list(saver._list_rows(cfg("t-b"), None, cfg("t-b", "", cid(22)), None))
    assert [r[2] for r in rows] == [cid(21), cid(20)]
    assert list(saver._list_rows(None, None, None, 0)) == []
    everything = list(saver._list_rows(None, None, None, None))
    assert len(everything) == 6 + 3 + 4 + 1  # bad-shape key and non-JSON row skipped
