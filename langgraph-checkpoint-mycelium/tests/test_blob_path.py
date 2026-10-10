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
