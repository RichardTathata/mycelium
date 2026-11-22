"""
A checkpoint row another member wrote cannot run code where it is loaded (0.3.5).

A `ckpt/` or `ckptw/` row is an unsigned record in the gossip KV: any admitted member, and any HTTP
client holding `kv:write`, can write one. The row names the blob each payload is in and the type tag
it is decoded with, and the saver hands both to `serde.loads_typed`. Under LangGraph's default
`JsonPlusSerializer` (langgraph-checkpoint 4.1.1 through 4.3.2 at least), a `"msgpack"` payload
naming any importable callable is imported and called, with a warning only, unless
`LANGGRAPH_STRICT_MSGPACK=true` is set in the loading process. The saver's default serializer is now
LangGraph's strict mode whatever that variable says (`JsonPlusSerializer(allowed_msgpack_modules=None)`):
a type outside LangGraph's safe set loads as its raw arguments and is never called.

No node: the saver's HTTP clients are pointed at an in-memory gateway. Seen failing on the 0.3.5
floor without the default (the marker was created by `os.system` and by `os.mkdir`).
"""

import base64
import hashlib
import json
import os

import httpx
import ormsgpack
import pytest

from langgraph.checkpoint.base import empty_checkpoint
from langgraph.checkpoint.base.id import uuid6
from langgraph.checkpoint.serde.jsonplus import (
    EXT_CONSTRUCTOR_POS_ARGS,
    EXT_CONSTRUCTOR_SINGLE_ARG,
    JsonPlusSerializer,
)

from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver


class FakeGateway:
    """The KV and blob routes the saver calls, in memory, as the gateway answers them."""

    def __init__(self) -> None:
        self.kv: dict[str, bytes] = {}
        self.blobs: dict[str, bytes] = {}

    def handle(self, req: httpx.Request) -> httpx.Response:
        path, q = req.url.path, req.url.params
        if path == "/gateway/kv" and req.method == "GET":
            v = self.kv.get(q["key"])
            if v is None:
                return httpx.Response(200, json={"found": False})
            return httpx.Response(200, json={"found": True, "value_b64": base64.b64encode(v).decode()})
        if path == "/gateway/kv" and req.method == "POST":
            body = json.loads(req.content)
            self.kv[body["key"]] = base64.b64decode(body["value_b64"])
            return httpx.Response(200, json={"ok": True})
        if path == "/gateway/kv/keys":
            return httpx.Response(200, json={"keys": sorted(k for k in self.kv if k.startswith(q.get("prefix", "")))})
        if path == "/gateway/reason/blob" and req.method == "PUT":
            return httpx.Response(200, json={"id": self.put_blob(req.content)})
        if path.startswith("/gateway/reason/blob/"):
            blob = self.blobs.get(path.rsplit("/", 1)[1])
            return httpx.Response(404, json={"error": "not_found"}) if blob is None else httpx.Response(200, content=blob)
        return httpx.Response(404)

    def put_blob(self, data: bytes) -> str:
        blob_id = hashlib.sha256(data).hexdigest()
        self.blobs[blob_id] = data
        return blob_id


def wire(saver: MyceliumCheckpointSaver, gw: FakeGateway) -> MyceliumCheckpointSaver:
    transport = httpx.MockTransport(gw.handle)
    saver._client = httpx.Client(base_url="http://test", transport=transport)
    saver._aclient = httpx.AsyncClient(base_url="http://test", transport=transport)
    return saver


def honest_checkpoint(saver: MyceliumCheckpointSaver, thread: str) -> dict:
    """What the saver itself writes: one checkpoint with a channel value and a pending write."""
    cp = empty_checkpoint()
    cp["id"] = str(uuid6())
    cp["channel_values"] = {"messages": ["hello"]}
    cp["channel_versions"] = {"messages": 1}
    cfg = saver.put(
        {"configurable": {"thread_id": thread, "checkpoint_ns": ""}},
        cp,
        {"source": "loop", "step": 1, "parents": {}},
        {"messages": 1},
    )
    saver.put_writes(cfg, [("messages", "a completed task's result")], task_id="task-1")
    return cfg


def forged_payload(ext_code: int, module: str, name: str, args) -> bytes:
    """A msgpack payload naming `module.name` as the constructor, as a member can write it."""
    return ormsgpack.packb(ormsgpack.Ext(ext_code, ormsgpack.packb((module, name, args))))


# Each forgery's side effect is a path under the test's tmp dir; none may come into being.
FORGERIES = [
    pytest.param(lambda m: (EXT_CONSTRUCTOR_POS_ARGS, "os", "system", [f"touch '{m}'"]), id="os.system"),
    pytest.param(lambda m: (EXT_CONSTRUCTOR_POS_ARGS, "os", "mkdir", [str(m)]), id="os.mkdir-pos"),
    pytest.param(lambda m: (EXT_CONSTRUCTOR_SINGLE_ARG, "os", "mkdir", str(m)), id="os.mkdir-single"),
]

# Where a member can put a forged payload: the channel value, the skeleton, a pending write.
SLOTS = ["channel", "skeleton", "write"]


def forge(gw: FakeGateway, slot: str, payload: bytes) -> None:
    blob_id = gw.put_blob(payload)
    for key, raw in list(gw.kv.items()):
        row = json.loads(raw)
        if slot == "channel" and key.startswith("ckpt/"):
            row["channels"]["messages"] = [blob_id, "msgpack"]
        elif slot == "skeleton" and key.startswith("ckpt/"):
            row["blob"], row["type"] = blob_id, "msgpack"
        elif slot == "write" and key.startswith("ckptw/"):
            row["blob"], row["type"] = blob_id, "msgpack"
        else:
            continue
        gw.kv[key] = json.dumps(row).encode()
        return
    raise AssertionError(f"no row to forge for {slot}")


def load(saver: MyceliumCheckpointSaver, cfg: dict, slot: str):
    """Load the forged checkpoint; a forged skeleton may fail to assemble, which is a refusal too."""
    try:
        return saver.get_tuple(cfg)
    except Exception:
        if slot == "skeleton":
            return None
        raise


@pytest.mark.parametrize("slot", SLOTS)
@pytest.mark.parametrize("make", FORGERIES)
def test_a_member_written_row_naming_a_callable_does_not_run_it_under_the_default_saver(tmp_path, make, slot):
    gw = FakeGateway()
    saver = wire(MyceliumCheckpointSaver("127.0.0.1", 1), gw)
    cfg = honest_checkpoint(saver, f"t-{slot}")
    marker = tmp_path / "ran"
    ext_code, module, name, args = make(marker)
    forge(gw, slot, forged_payload(ext_code, module, name, args))

    tup = load(saver, cfg, slot)

    assert not os.path.exists(marker), f"{module}.{name} ran at load"
    if tup is not None and slot == "channel":
        # Refused, not called: the raw arguments come back, never the callable's result.
        assert tup.checkpoint["channel_values"]["messages"] == args
    if tup is not None and slot == "write":
        assert [w[2] for w in tup.pending_writes] == [args]


@pytest.mark.parametrize("make", FORGERIES)
async def test_the_async_loader_refuses_it_too(tmp_path, make):
    gw = FakeGateway()
    saver = wire(MyceliumCheckpointSaver("127.0.0.1", 1), gw)
    cfg = honest_checkpoint(saver, "t-async")
    marker = tmp_path / "ran"
    forge(gw, "channel", forged_payload(*make(marker)))

    await saver.aget_tuple(cfg)

    assert not os.path.exists(marker)


def test_the_default_holds_whatever_the_strict_variable_says(tmp_path, monkeypatch):
    # LangGraph reads LANGGRAPH_STRICT_MSGPACK once, at import; the saver must not depend on it.
    import langgraph.checkpoint.serde._msgpack as lg_msgpack

    monkeypatch.setattr(lg_msgpack, "STRICT_MSGPACK_ENABLED", False)
    gw = FakeGateway()
    saver = wire(MyceliumCheckpointSaver("127.0.0.1", 1), gw)
    cfg = honest_checkpoint(saver, "t-env")
    marker = tmp_path / "ran"
    forge(gw, "channel", forged_payload(EXT_CONSTRUCTOR_POS_ARGS, "os", "mkdir", [str(marker)]))

    saver.get_tuple(cfg)

    assert not marker.exists()


def test_the_saver_s_own_round_trip_still_loads_the_safe_types():
    from datetime import datetime, timezone
    from uuid import UUID

    from langchain_core.messages import AIMessage, HumanMessage
    from langgraph.types import Send

    gw = FakeGateway()
    saver = wire(MyceliumCheckpointSaver("127.0.0.1", 1), gw)
    value = {
        "messages": [HumanMessage(content="hi"), AIMessage(content="hello")],
        "when": datetime(2026, 11, 22, tzinfo=timezone.utc),
        "id": UUID(int=7),
        "tags": {"a", "b"},
    }
    cp = empty_checkpoint()
    cp["id"] = str(uuid6())
    cp["channel_values"] = {"state": value}
    cp["channel_versions"] = {"state": 1}
    cfg = saver.put({"configurable": {"thread_id": "t-own", "checkpoint_ns": ""}}, cp, {"source": "loop", "step": 1, "parents": {}}, {"state": 1})
    saver.put_writes(cfg, [("branch:to:x", Send("x", {"n": 1}))], task_id="task-1")

    tup = saver.get_tuple(cfg)

    assert tup.checkpoint["channel_values"]["state"] == value
    assert tup.pending_writes[0][2] == Send("x", {"n": 1})


def test_a_serializer_the_caller_passes_is_kept(tmp_path):
    # The caller's choice, theirs to make: here one that allows every type, so the forgery runs.
    gw = FakeGateway()
    permissive = JsonPlusSerializer(allowed_msgpack_modules=True)
    saver = wire(MyceliumCheckpointSaver("127.0.0.1", 1, serde=permissive), gw)
    assert saver.serde is permissive
    cfg = honest_checkpoint(saver, "t-caller")
    marker = tmp_path / "ran"
    forge(gw, "channel", forged_payload(EXT_CONSTRUCTOR_POS_ARGS, "os", "mkdir", [str(marker)]))

    saver.get_tuple(cfg)

    assert marker.exists()
