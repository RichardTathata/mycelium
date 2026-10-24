"""The saver reads the receipt its writes return (adopter-surface X7: the adapter contract's *receipts* cell).

Before this, ``_kv_set`` posted to ``/gateway/kv`` and checked the status only, so ``put()`` acknowledged an HTTP 200
and nothing of what the gateway node established. Now every row and blob-index write yields a ``WriteReceipt`` —
rung 1 always, rung 2 as ``local_durability`` — kept on ``last_receipt``, handed to ``on_receipt``, and refused under
``require_on_disk`` when it names no disk. No node: an ``httpx.MockTransport`` answers as the route does since v2.16.0.
"""
from __future__ import annotations

import base64
import json

import httpx
import pytest
from langgraph.checkpoint.base import empty_checkpoint

from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver, WriteNotOnDisk, WriteReceipt


class ReceiptGateway:
    """An in-memory gateway whose ``POST /gateway/kv`` answers the receipt a real one answers."""

    def __init__(self, durability: str = "on_disk", error: str | None = None) -> None:
        self.kv: dict[str, bytes] = {}
        self.durability = durability
        self.error = error
        self.writes = 0

    def handle(self, req: httpx.Request) -> httpx.Response:
        path, q = req.url.path, dict(req.url.params)
        if path == "/gateway/kv" and req.method == "POST":
            body = json.loads(req.content)
            self.kv[body["key"]] = base64.b64decode(body["value_b64"])
            self.writes += 1
            answer = {"ok": True, "operation_id": f"op-{self.writes}", "local_durability": self.durability}
            if self.error:
                answer["local_durability_error"] = self.error
            return httpx.Response(200, json=answer)
        if path == "/gateway/kv" and req.method == "GET":
            v = self.kv.get(q["key"])
            return httpx.Response(200, json={"found": False} if v is None else {"found": True, "value_b64": base64.b64encode(v).decode()})
        if path == "/gateway/kv/keys":
            return httpx.Response(200, json={"keys": sorted(k for k in self.kv if k.startswith(q.get("prefix", "")))})
        if path == "/gateway/reason/blob" and req.method == "PUT":
            return httpx.Response(200, json={"id": "b" * 64})
        return httpx.Response(404)


def saver_over(gw: ReceiptGateway, **kw) -> MyceliumCheckpointSaver:
    saver = MyceliumCheckpointSaver("127.0.0.1", 1, **kw)
    transport = httpx.MockTransport(gw.handle)
    saver._client = httpx.Client(base_url="http://test", transport=transport)
    saver._aclient = httpx.AsyncClient(base_url="http://test", transport=transport)
    return saver


def put_one(saver: MyceliumCheckpointSaver) -> None:
    config = {"configurable": {"thread_id": "t1", "checkpoint_ns": ""}}
    saver.put(config, empty_checkpoint(), {"source": "input", "step": 0, "writes": {}, "parents": {}}, {})


def test_put_hands_back_the_gateways_receipt() -> None:
    gw = ReceiptGateway("on_disk")
    seen: list[WriteReceipt] = []
    saver = saver_over(gw, on_receipt=seen.append)
    put_one(saver)
    assert gw.writes >= 1, "put wrote at least the row"
    assert saver.last_receipt is not None
    assert saver.last_receipt.local_durability == "on_disk" and saver.last_receipt.on_disk
    assert saver.last_receipt.operation_id == f"op-{gw.writes}", "the latest write's receipt is the one kept"
    assert [r.operation_id for r in seen] == [f"op-{i}" for i in range(1, gw.writes + 1)], "every write's receipt was handed on, in order"
    assert all(r.key for r in seen), "each receipt names its key"


def test_require_on_disk_refuses_a_write_the_node_did_not_establish() -> None:
    gw = ReceiptGateway("buffered")
    saver = saver_over(gw, require_on_disk=True)
    with pytest.raises(WriteNotOnDisk) as e:
        put_one(saver)
    assert e.value.receipt.local_durability == "buffered"
    assert "buffered" in str(e.value)
    assert gw.writes >= 1, "the row was applied on the gateway node all the same; the refusal is about its disk"
    # The one state that satisfies it.
    put_one(saver_over(ReceiptGateway("on_disk"), require_on_disk=True))


def test_a_failed_append_names_its_error_and_an_old_gateway_reads_as_unknown() -> None:
    gw = ReceiptGateway("failed", error="disk full")
    saver = saver_over(gw)
    put_one(saver)
    assert saver.last_receipt is not None and saver.last_receipt.local_durability == "failed"
    assert saver.last_receipt.local_durability_error == "disk full"

    class Bare(ReceiptGateway):
        def handle(self, req: httpx.Request) -> httpx.Response:
            if req.url.path == "/gateway/kv" and req.method == "POST":
                self.writes += 1
                return httpx.Response(200, json={"ok": True})
            return super().handle(req)

    old = saver_over(Bare())
    put_one(old)
    assert old.last_receipt is not None and old.last_receipt.operation_id is None and not old.last_receipt.on_disk


@pytest.mark.asyncio
async def test_the_async_writes_yield_the_same_receipt() -> None:
    gw = ReceiptGateway("on_disk")
    saver = saver_over(gw)
    config = {"configurable": {"thread_id": "t2", "checkpoint_ns": ""}}
    await saver.aput(config, empty_checkpoint(), {"source": "input", "step": 0, "writes": {}, "parents": {}}, {})
    assert saver.last_receipt is not None and saver.last_receipt.on_disk
