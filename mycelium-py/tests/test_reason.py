"""
Integration tests for mycelium.reason.ReasonClient against a running reason
node (the `reason_node` example: EchoBackend + template ``{{input}}``, serving
``llm/{MODEL}`` — so a routed call's output is ``echo: {input}``).

Requires MYCELIUM_TEST_PORT (a running reason node); tests skip cleanly when it
is unset. The served model is ``MYCELIUM_TEST_MODEL`` (default ``fable-mini``).
"""

import os
import uuid

import pytest

from mycelium import NoProviderError, ReasonClient

TEST_HOST = os.getenv("MYCELIUM_TEST_HOST", "127.0.0.1")
TEST_PORT = os.getenv("MYCELIUM_TEST_PORT")
TEST_MODEL = os.getenv("MYCELIUM_TEST_MODEL", "fable-mini")

pytestmark = pytest.mark.skipif(
    TEST_PORT is None,
    reason="MYCELIUM_TEST_PORT not set — no reason node to test against",
)


@pytest.mark.asyncio
async def test_route_echoes_and_reports_provider() -> None:
    async with ReasonClient(TEST_HOST, int(TEST_PORT)) as reason:
        result = await reason.route(TEST_MODEL, "hello-mesh")
        assert "hello-mesh" in result["output"]
        # A provider answered, addressed as ip:port; first candidate → attempt 1.
        assert ":" in result["provider"]
        assert result["attempt"] == 1


@pytest.mark.asyncio
async def test_route_no_provider_raises() -> None:
    async with ReasonClient(TEST_HOST, int(TEST_PORT)) as reason:
        with pytest.raises(NoProviderError):
            await reason.route("no-such-model", "x")


@pytest.mark.asyncio
async def test_blob_put_get_roundtrip() -> None:
    async with ReasonClient(TEST_HOST, int(TEST_PORT)) as reason:
        payload = b"routed-blob-fixture\x00\x01\x02"
        blob_id = await reason.blob_put(payload)
        assert blob_id
        assert await reason.blob_get(blob_id) == payload


@pytest.mark.asyncio
async def test_trace_unknown_run_is_empty() -> None:
    async with ReasonClient(TEST_HOST, int(TEST_PORT)) as reason:
        trace = await reason.trace("nonexistent-run")
        assert trace["run_id"] == "nonexistent-run"
        assert trace["events"] == []


@pytest.mark.asyncio
async def test_route_with_run_id_records_trace() -> None:
    """Routing with a run_id makes the route endpoint record a trace: after two
    routed calls, trace(run_id) yields a non-empty event list that includes a
    ``route`` event (the routing decision) — the surface rung 5 stands on."""
    run_id = f"rung5-{uuid.uuid4()}"
    async with ReasonClient(TEST_HOST, int(TEST_PORT)) as reason:
        for i in range(2):
            result = await reason.route(TEST_MODEL, f"trace-me-{i}", run_id=run_id)
            assert f"trace-me-{i}" in result["output"]

        trace = await reason.trace(run_id)
        assert trace["run_id"] == run_id
        events = trace["events"]
        assert events, "a routed call under a run_id must record at least one event"
        kinds = {e["kind"] for e in events}
        assert "route" in kinds, f"expected a route event, got kinds {kinds}"


@pytest.mark.asyncio
async def test_openai_facade_routes_a_chat_completion() -> None:
    """The OpenAI-compatible façade: a plain OpenAI chat request against
    ``/gateway/reason/v1`` routes to the served ``llm/{MODEL}`` and answers in the
    OpenAI shape — what any OpenAI client sees after changing only its base URL."""
    import httpx

    base = f"http://{TEST_HOST}:{TEST_PORT}/gateway/reason/v1"
    async with httpx.AsyncClient(base_url=base, timeout=30.0) as c:
        models = (await c.get("/models")).json()
        assert TEST_MODEL in [m["id"] for m in models["data"]]

        resp = await c.post(
            "/chat/completions",
            json={"model": TEST_MODEL, "messages": [{"role": "user", "content": "facade-hello"}]},
        )
        assert resp.status_code == 200, resp.text
        body = resp.json()
        assert body["object"] == "chat.completion"
        assert "facade-hello" in body["choices"][0]["message"]["content"]
        assert body["mycelium"]["provider"]

        missing = await c.post(
            "/chat/completions",
            json={"model": "no-such-model", "messages": [{"role": "user", "content": "x"}]},
        )
        assert missing.status_code == 404
        assert missing.json()["error"]["code"] == "model_not_found"
