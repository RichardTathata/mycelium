"""
An incomplete checkpoint is an error, not a shorter or absent one (realignment repairs S5; the
review's F10).

A checkpoint's index row gossips in before every payload it references can be fetched, so a reader
on another node can see the row while a blob is still unavailable. The loaders used to answer that
two different wrong ways: a missing **pending-write** blob was skipped, returning a tuple with fewer
`pending_writes` (LangGraph then re-runs a task that already completed, and can lose an
`__error__`/`__interrupt__`/`__resume__` write); a missing **skeleton or channel** blob returned
`None`, which LangGraph reads as *no checkpoint* and starts the thread over. Both now raise
`IncompleteCheckpoint`, naming what is missing, so a caller retries instead of resuming wrong.

No node: the saver's HTTP clients are pointed at an in-memory gateway (`httpx.MockTransport`) that
serves the five routes it uses with the gateway's own shapes. Seen failing on the unfixed saver.
"""

import base64
import hashlib
import json

import httpx
import pytest

from langgraph.checkpoint.base import empty_checkpoint
from langgraph.checkpoint.base.id import uuid6

import langgraph_checkpoint_mycelium as pkg
from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver

# A stand-in when the package predates the error, so the fail-first run reaches the behaviour (a
# shorter tuple, or None) instead of stopping at the import.
IncompleteCheckpoint = getattr(pkg, "IncompleteCheckpoint", type("IncompleteCheckpoint", (Exception,), {}))


class FakeGateway:
    """The KV and blob routes the saver calls, in memory, answering as the gateway does."""

    def __init__(self) -> None:
        self.kv: dict[str, bytes] = {}
        self.blobs: dict[str, bytes] = {}
        # A status the blob route answers for an id instead of serving it (S5's four reasons).
        self.blob_status: dict[str, int] = {}
        # A body the blob route answers with instead of the gateway's own JSON (a proxy's page, nothing).
        self.blob_body: dict[str, bytes] = {}

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
        if path == "/gateway/kv" and req.method == "DELETE":
            self.kv.pop(q["key"], None)
            return httpx.Response(200, json={"ok": True})
        if path == "/gateway/kv/keys":
            return httpx.Response(200, json={"keys": sorted(k for k in self.kv if k.startswith(q.get("prefix", "")))})
        if path == "/gateway/reason/blob" and req.method == "PUT":
            blob_id = hashlib.sha256(req.content).hexdigest()
            self.blobs[blob_id] = req.content
            return httpx.Response(200, json={"id": blob_id})
        if path.startswith("/gateway/reason/blob/"):
            wanted = path.rsplit("/", 1)[1]
            if wanted in self.blob_status:
                code = self.blob_status[wanted]
                body = self.blob_body.get(wanted)
                if body is not None:
                    return httpx.Response(code, content=body)
                return httpx.Response(code, json={"error": {404: "not_found", 503: "unavailable", 502: "corrupt"}.get(code, "denied")})
            blob = self.blobs.get(wanted)
            # The real route's miss carries its reason in the body (mycelium-reason` 0.7.0+).
            return httpx.Response(404, json={"error": "not_found"}) if blob is None else httpx.Response(200, content=blob)
        return httpx.Response(404)


@pytest.fixture
def world():
    gw = FakeGateway()
    saver = MyceliumCheckpointSaver("127.0.0.1", 1)
    transport = httpx.MockTransport(gw.handle)
    saver._client = httpx.Client(base_url="http://test", transport=transport)
    saver._aclient = httpx.AsyncClient(base_url="http://test", transport=transport)
    return gw, saver


def checkpoint_with_write(saver: MyceliumCheckpointSaver, thread: str):
    """One checkpoint with a channel value and one pending write; returns its config."""
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


def blob_of(gw: FakeGateway, saver: MyceliumCheckpointSaver, value) -> str:
    """The content id the saver stored `value` under."""
    _type, data = saver.serde.dumps_typed(value)
    blob_id = hashlib.sha256(data).hexdigest()
    assert blob_id in gw.blobs, "the blob is where the saver put it"
    return blob_id


def test_a_complete_checkpoint_reads_back_whole(world):
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-whole")
    tup = saver.get_tuple(cfg)
    assert tup is not None
    assert [w[2] for w in tup.pending_writes] == ["a completed task's result"]


def test_a_missing_pending_write_blob_raises_instead_of_dropping_the_write(world):
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-write")
    missing = blob_of(gw, saver, "a completed task's result")
    del gw.blobs[missing]
    with pytest.raises(IncompleteCheckpoint) as e:
        saver.get_tuple(cfg)
    assert missing in e.value.missing
    gw_restore_and_reread(gw, saver, cfg, missing, "a completed task's result")


def test_a_missing_channel_blob_raises_instead_of_reading_as_no_checkpoint(world):
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-channel")
    missing = blob_of(gw, saver, ["hello"])
    del gw.blobs[missing]
    with pytest.raises(IncompleteCheckpoint):
        saver.get_tuple(cfg)
    # The latest-checkpoint form must not read as "no checkpoint" either: that restarts the thread.
    latest = {"configurable": {"thread_id": "t-channel", "checkpoint_ns": ""}}
    with pytest.raises(IncompleteCheckpoint):
        saver.get_tuple(latest)


def test_no_checkpoint_at_all_is_still_none(world):
    _gw, saver = world
    assert saver.get_tuple({"configurable": {"thread_id": "never-written", "checkpoint_ns": ""}}) is None


async def test_the_async_loader_raises_the_same_way(world):
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-async")
    missing = blob_of(gw, saver, "a completed task's result")
    del gw.blobs[missing]
    with pytest.raises(IncompleteCheckpoint):
        await saver.aget_tuple(cfg)


def gw_restore_and_reread(gw, saver, cfg, missing, value):
    """Once the blob arrives the same read succeeds, with the completed task's write present."""
    _type, data = saver.serde.dumps_typed(value)
    gw.blobs[missing] = data
    tup = saver.get_tuple(cfg)
    assert [w[2] for w in tup.pending_writes] == [value]


# ── S5's unbuilt half: why a blob is missing stays distinguishable (doc-coverage run 20) ──────────
# The plan promised "absence, temporary unavailability, authorization refusal and corrupt content stay
# distinguishable in the error". The saver read 404 as missing and raised a bare HTTP error for any
# other status, so a corrupt or refused blob was either indistinguishable from a slow one or escaped as
# an unrelated exception.

@pytest.mark.parametrize("status,reason,retriable", [
    (404, "not_found", True),
    (503, "unavailable", True),
    (502, "corrupt", False),
    (401, "unauthorized", False),
    (403, "unauthorized", False),
])
def test_why_a_blob_is_missing_is_named_and_decides_retriable(world, status, reason, retriable):
    gw, saver = world
    cfg = checkpoint_with_write(saver, f"t-why-{status}")
    blob = blob_of(gw, saver, "a completed task's result")
    gw.blob_status[blob] = status
    with pytest.raises(IncompleteCheckpoint) as e:
        saver.get_tuple(cfg)
    assert e.value.reasons[blob] == reason
    assert e.value.retriable is retriable


async def test_the_async_loader_names_the_reason_too(world):
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-why-async")
    blob = blob_of(gw, saver, "a completed task's result")
    gw.blob_status[blob] = 502
    with pytest.raises(IncompleteCheckpoint) as e:
        await saver.aget_tuple(cfg)
    assert e.value.reasons[blob] == "corrupt"
    assert e.value.retriable is False



# ── The adversarial review of #542: what the status alone cannot say ────────────────────────────────

def test_a_proxys_502_is_unavailable_not_corrupt(world):
    """nginx / an ELB / Envoy answer 502 with their own page when the node is down or restarting. Only
    the route's own `{"error":"corrupt"}` body means corrupt content."""
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-proxy")
    blob = blob_of(gw, saver, "a completed task's result")
    gw.blob_status[blob] = 502
    gw.blob_body[blob] = b"<html><body>502 Bad Gateway</body></html>"
    with pytest.raises(IncompleteCheckpoint) as e:
        saver.get_tuple(cfg)
    assert e.value.reasons[blob] == "unavailable"
    assert e.value.retriable is True


def test_a_404_without_the_routes_body_means_the_route_is_not_served(world):
    """A node without the reason companion answers a bodyless 404: the blob route does not exist there, and
    retrying will not make it appear — not "not found yet"."""
    gw, saver = world
    cfg = checkpoint_with_write(saver, "t-noroute")
    blob = blob_of(gw, saver, "a completed task's result")
    gw.blob_status[blob] = 404
    gw.blob_body[blob] = b""
    with pytest.raises(IncompleteCheckpoint) as e:
        saver.get_tuple(cfg)
    assert e.value.reasons[blob] == "unsupported"
    assert e.value.retriable is False


@pytest.mark.parametrize("status", [408, 429])
def test_a_throttled_or_timed_out_read_is_unavailable(world, status):
    gw, saver = world
    cfg = checkpoint_with_write(saver, f"t-transient-{status}")
    blob = blob_of(gw, saver, "a completed task's result")
    gw.blob_status[blob] = status
    with pytest.raises(IncompleteCheckpoint) as e:
        saver.get_tuple(cfg)
    assert e.value.reasons[blob] == "unavailable"
    assert e.value.retriable is True


def test_the_sync_blob_helper_returns_the_bytes(world):
    gw, saver = world
    blob_id = saver._blob_put(b"payload")
    assert saver._blob_get(blob_id) == b"payload"
    assert saver._blob_get("0" * 64) is None


def test_a_blob_fetched_twice_keeps_its_most_serious_reason():
    """Content addressing makes repeated ids common (two channels with one value). The reason recorded
    must not depend on which fetch came last: the more serious one stands."""
    from langgraph_checkpoint_mycelium.saver import _merge_reason

    reasons: dict[str, str] = {}
    for r in ("unavailable", "not_found"):
        _merge_reason(reasons, "b", r)
    assert reasons == {"b": "unavailable"}
    for r in ("corrupt", "unavailable"):
        _merge_reason(reasons, "c", r)
    assert reasons["c"] == "corrupt"
