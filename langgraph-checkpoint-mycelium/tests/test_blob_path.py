"""A blob id read from a checkpoint row stays one path segment (post-360 hardening, row G).

The index row is peer-writable KV, and the saver fetched ``/gateway/reason/blob/{blob_id}`` with the id
interpolated raw — so a row naming ``../../kv/keys?prefix=`` sent the saver's bearer to a different route.
The id is now percent-encoded as one segment (the saver's own key segments always were); an id of ``.`` or
``..``, which any URL parser resolves to another path even encoded, is not fetched at all and reads as
``corrupt`` — no content address is a dot. No node: an ``httpx.MockTransport`` records the raw path.
"""

import asyncio

import httpx
import pytest

from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver


@pytest.fixture
def saver_and_paths():
    paths: list[bytes] = []

    def handle(req: httpx.Request) -> httpx.Response:
        paths.append(req.url.raw_path)
        return httpx.Response(404, json={"error": "not_found"})

    saver = MyceliumCheckpointSaver("127.0.0.1", 1)
    transport = httpx.MockTransport(handle)
    saver._client = httpx.Client(base_url="http://test", transport=transport)
    saver._aclient = httpx.AsyncClient(base_url="http://test", transport=transport)
    return saver, paths


HOSTILE = {
    "../../kv/keys?prefix=": b"/gateway/reason/blob/..%2F..%2Fkv%2Fkeys%3Fprefix%3D",
    "a/b#c":                 b"/gateway/reason/blob/a%2Fb%23c",
}


@pytest.mark.parametrize("blob_id", list(HOSTILE))
def test_a_row_supplied_blob_id_is_one_path_segment(saver_and_paths, blob_id):
    saver, paths = saver_and_paths
    reasons: dict[str, str] = {}
    assert saver._blob_try(blob_id, reasons) is None
    assert asyncio.run(saver._ablob_try(blob_id, reasons)) is None
    assert paths == [HOSTILE[blob_id], HOSTILE[blob_id]]
    assert reasons == {blob_id: "not_found"}


@pytest.mark.parametrize("blob_id", [".", ".."])
def test_a_dot_blob_id_is_not_fetched_and_reads_as_corrupt(saver_and_paths, blob_id):
    saver, paths = saver_and_paths
    reasons: dict[str, str] = {}
    assert saver._blob_try(blob_id, reasons) is None
    assert asyncio.run(saver._ablob_try(blob_id, reasons)) is None
    assert paths == []
    assert reasons == {blob_id: "corrupt"}


@pytest.mark.parametrize("blob_id", ["", "\ud800"])
def test_an_id_that_cannot_be_one_segment_is_not_fetched_and_reads_as_corrupt(saver_and_paths, blob_id):
    # Adversarial review of #595: an empty id reaches another route; a lone surrogate has no encoding.
    saver, paths = saver_and_paths
    reasons: dict[str, str] = {}
    assert saver._blob_try(blob_id, reasons) is None
    assert asyncio.run(saver._ablob_try(blob_id, reasons)) is None
    assert paths == []
    assert reasons == {blob_id: "corrupt"}


# A forged row whose `blob` is not a string (the row is peer-writable): the loaders raised `TypeError`.
# It is an id no content address can be, so the checkpoint is incomplete with the reason `corrupt`.
from test_incomplete import FakeGateway, IncompleteCheckpoint, checkpoint_with_write  # noqa: E402


@pytest.mark.parametrize("forged", [7, None, ["x"], {"id": "x"}])
@pytest.mark.parametrize("loader", ["sync", "async"])
def test_a_non_string_blob_in_a_row_reads_as_corrupt(forged, loader):
    import json

    gw = FakeGateway()
    saver = MyceliumCheckpointSaver("127.0.0.1", 1)
    transport = httpx.MockTransport(gw.handle)
    saver._client = httpx.Client(base_url="http://test", transport=transport)
    saver._aclient = httpx.AsyncClient(base_url="http://test", transport=transport)
    cfg = checkpoint_with_write(saver, "t-forged")
    key = next(k for k in gw.kv if k.startswith("ckpt/"))
    row = json.loads(gw.kv[key])
    row["blob"] = forged
    gw.kv[key] = json.dumps(row).encode()
    with pytest.raises(IncompleteCheckpoint) as e:
        if loader == "sync":
            saver.get_tuple(cfg)
        else:
            asyncio.run(saver.aget_tuple(cfg))
    assert list(e.value.reasons.values()) == ["corrupt"]
    assert e.value.retriable is False
