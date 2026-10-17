"""An unmodified A2A client against a Mycelium node (post-360 plan row N).

The 2026-10-12 probe (`docs/plans/external/2026-10-12-a2a-sdk-probe/`) found that the current `a2a-sdk` — the
library Strands Agents uses — could not reach a node's skills through `/a2a`: the card was not where the SDK
reads it, named the gossip port, and the methods it sends answered "method not found". This suite is that probe
as a test: the SDK's own card resolver, client factory and transports, against `examples/a2a_sdk_node.rs`, with
nothing Mycelium-specific on the client side except `metadata.skillId` where a skill is named.

Needs a running node (`MYCELIUM_A2A_URL`, its gateway; `MYCELIUM_A2A_TOKEN`, a bearer it knows) and `a2a-sdk`
installed. Without the URL every test
skips, unless `MYCELIUM_A2A_LIVE_REQUIRED` is set — the CI step that starts the node sets it, so a node that
never came up fails the job rather than skipping it green. The SDK is imported inside each test so that
collecting this file (CI's test universe) needs neither.
"""

import asyncio
import os
import uuid

import pytest

URL = os.environ.get("MYCELIUM_A2A_URL", "")
# A bearer the node knows, so the client has an identity: a task is read back only by the one that created it.
TOKEN = os.environ.get("MYCELIUM_A2A_TOKEN", "")
REQUIRED = os.environ.get("MYCELIUM_A2A_LIVE_REQUIRED", "") not in ("", "0")

if not URL and REQUIRED:
    raise RuntimeError("MYCELIUM_A2A_LIVE_REQUIRED is set but MYCELIUM_A2A_URL is not: the node is missing")

pytestmark = pytest.mark.skipif(not URL, reason="MYCELIUM_A2A_URL is not set (no node to drive)")

SDK_VERSION = "1.2.2"


def run(coro):
    return asyncio.run(coro)


def _http(timeout=30):
    import httpx

    headers = {"Authorization": f"Bearer {TOKEN}"} if TOKEN else {}
    return httpx.AsyncClient(timeout=timeout, headers=headers)


async def _card(http):
    from a2a.client.card_resolver import A2ACardResolver

    # The resolver's default path, `/.well-known/agent-card.json`: the one the probe found missing.
    return await A2ACardResolver(http, URL).get_agent_card()


def _message(text, skill=None):
    from a2a.helpers.proto_helpers import new_text_message
    from a2a.types import Role, SendMessageRequest

    msg = new_text_message(text, role=Role.ROLE_USER)
    if skill is not None:
        msg.metadata.update({"skillId": skill})
    return SendMessageRequest(message=msg)


async def _send(text, *, skill=None, streaming):
    """Send one message the way Strands does — a client built from the card — and collect the events."""
    from a2a.client.client_factory import ClientConfig, ClientFactory

    async with _http() as http:
        card = await _card(http)
        client = ClientFactory(ClientConfig(httpx_client=http, streaming=streaming)).create(card)
        return [event async for event in client.send_message(_message(text, skill))]


def _final_task(events):
    """The task the events end in, whether they came as tasks or as status and artifact updates."""
    from a2a.types import TaskState

    task, state, texts = None, None, []
    for ev in events:
        which = ev.WhichOneof("payload")
        if which == "task":
            task = ev.task
            state = ev.task.status.state
            texts += [p.text for a in ev.task.artifacts for p in a.parts]
        elif which == "status_update":
            state = ev.status_update.status.state
        elif which == "artifact_update":
            texts += [p.text for p in ev.artifact_update.artifact.parts]
    return task, TaskState.Name(state) if state is not None else None, texts


def test_the_sdk_is_the_version_the_row_names():
    import importlib.metadata as md

    assert md.version("a2a-sdk") == SDK_VERSION


def test_the_card_resolves_at_the_sdks_default_path_and_offers_jsonrpc_1_0():
    async def go():
        async with _http(10) as http:
            return await _card(http)

    card = run(go())
    interfaces = [(i.url, i.protocol_binding, i.protocol_version) for i in card.supported_interfaces]
    assert (f"{URL}/a2a", "JSONRPC", "1.0") in interfaces, interfaces
    assert (f"{URL}/a2a", "JSONRPC", "0.3") in interfaces, interfaces
    assert card.capabilities.streaming
    assert "depot/dispatch" in [s.id for s in card.skills]


def test_the_card_validates_as_an_a2a_0_3_card():
    """A 0.3 client validates the card with 0.3's own model, which requires each skill's `tags`; the 1.0
    resolver above ignores unknown and missing fields, so it cannot see this (#633 review, finding 1)."""
    from a2a.compat.v0_3 import types as v03

    async def go():
        async with _http(10) as http:
            r = await http.get(f"{URL}/.well-known/agent-card.json")
            r.raise_for_status()
            return r.json()

    card = v03.AgentCard.model_validate(run(go()))
    assert card.url == f"{URL}/a2a"


def test_send_message_reaches_the_default_skill():
    _, state, texts = _final_task(run(_send("one crate", streaming=False)))
    assert state == "TASK_STATE_COMPLETED"
    assert texts == ["dispatched: one crate"]


def test_a_streamed_send_reaches_the_default_skill():
    events = run(_send("two crates", streaming=True))
    _, state, texts = _final_task(events)
    assert state == "TASK_STATE_COMPLETED", events
    assert texts == ["dispatched: two crates"], events


def test_a_skill_named_in_metadata_is_called_and_an_unknown_one_is_refused():
    from a2a.utils.errors import InvalidParamsError

    _, state, texts = _final_task(run(_send("three crates", skill="depot/dispatch", streaming=False)))
    assert (state, texts) == ("TASK_STATE_COMPLETED", ["dispatched: three crates"])
    with pytest.raises(InvalidParamsError):
        run(_send("nothing", skill="depot/missing", streaming=False))


def test_get_task_returns_the_task_a_send_created():
    from a2a.client.client_factory import ClientConfig, ClientFactory
    from a2a.types import GetTaskRequest

    async def go():
        async with _http() as http:
            card = await _card(http)
            client = ClientFactory(ClientConfig(httpx_client=http, streaming=False)).create(card)
            events = [e async for e in client.send_message(_message("four crates"))]
            task, _, _ = _final_task(events)
            assert task is not None, events
            return task.id, await client.get_task(GetTaskRequest(id=task.id))

    task_id, task = run(go())
    assert task.id == task_id
    assert [p.text for a in task.artifacts for p in a.parts] == ["dispatched: four crates"]


def test_the_sdks_0_3_transport_reaches_the_skill_too():
    """The SDK's compatibility transport, which a client picks for a 0.3 interface: `message/send`."""
    from a2a.compat.v0_3.jsonrpc_transport import CompatJsonRpcTransport
    from a2a.types import TaskState

    async def go():
        async with _http() as http:
            card = await _card(http)
            transport = CompatJsonRpcTransport(http, card, f"{URL}/a2a")
            return await transport.send_message(_message(f"crate {uuid.uuid4().hex[:6]}"))

    response = run(go())
    assert response.WhichOneof("payload") == "task", response
    assert TaskState.Name(response.task.status.state) == "TASK_STATE_COMPLETED"
    assert response.task.artifacts[0].parts[0].text.startswith("dispatched: crate ")
