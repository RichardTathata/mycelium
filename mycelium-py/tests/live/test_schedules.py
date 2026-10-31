"""Adopter-surface Y12 against a live node: a schedule declared through the gateway, its runs received by the client.

The node the client is connected to stands in the schedule's holder band — it advertises the band's capability here,
through the gateway — and hands the client each run; an acknowledged run is the slot run, a run answered twice is
refused, and a declaration the unit-file loader refuses is refused by name.
"""

import asyncio
import os
import uuid

import pytest

from mycelium import MyceliumAgent, ScheduleDeclaration, ScheduleRefusedError, UnservedRunError

pytestmark = pytest.mark.skipif(
    not os.getenv("MYCELIUM_TEST_HOST"),
    reason="MYCELIUM_TEST_HOST not set — the live gateway suite needs a running node",
)

TEST_HOST = os.getenv("MYCELIUM_TEST_HOST", "127.0.0.1")
TEST_PORT = int(os.getenv("MYCELIUM_TEST_PORT", "8100"))


def _decl(agent_tag: str) -> ScheduleDeclaration:
    return ScheduleDeclaration(f"py-{agent_tag}", 1, ("py-sched", f"holder-{agent_tag}"), on_missed="skip")


@pytest.mark.asyncio
async def test_a_schedule_declared_through_the_gateway_hands_the_client_its_runs() -> None:
    agent = MyceliumAgent(TEST_HOST, TEST_PORT)
    tag = uuid.uuid4().hex[:8]
    decl = _decl(tag)
    band = agent.advertise_capability("py-sched", f"holder-{tag}", interval_secs=1)
    try:
        async def first_two():
            got = []
            async for run in agent.schedule_runs(decl):
                got.append(run)
                agent.schedule_ack(run)
                if len(got) == 2:
                    return got
        runs = await asyncio.wait_for(first_two(), timeout=30)
        for run in runs:
            assert run.schedule == decl.id
            assert run.kind == "on_time"
            # An open gateway: every client is `anonymous`, its schedule run as `{id}~anonymous`.
            assert run.operation_id == f"sched/{decl.id}~anonymous/{run.slot}"
        assert runs[1].slot > runs[0].slot, "one run per slot, in order"
        with pytest.raises(UnservedRunError):
            agent.schedule_ack(runs[0])
        # The slot the client acknowledged is the fleet's last run — written after the answer, so polled.
        key = f"sched/{decl.id}~anonymous/last/{agent.health()['node_id']}"
        last = None
        for _ in range(50):
            last = agent.get(key)
            if last is not None and int(last.split()[0]) >= runs[1].slot:
                break
            await asyncio.sleep(0.2)
        assert last is not None and int(last.split()[0]) >= runs[1].slot
    finally:
        band.drop()


@pytest.mark.asyncio
async def test_a_declaration_the_loader_refuses_is_refused_by_name() -> None:
    agent = MyceliumAgent(TEST_HOST, TEST_PORT)
    bad = ScheduleDeclaration("py-bad", 0, ("py-sched", "holder"))
    with pytest.raises(ScheduleRefusedError) as e:
        async for _ in agent.schedule_runs(bad):
            pass
    assert e.value.status == 422
    assert "period_secs" in e.value.detail


@pytest.mark.asyncio
async def test_a_node_outside_the_band_says_not_eligible() -> None:
    agent = MyceliumAgent(TEST_HOST, TEST_PORT)
    decl = ScheduleDeclaration(f"py-none-{uuid.uuid4().hex[:8]}", 1, ("py-sched", "nobody-advertises-this"))
    said: list = []

    async def until_said() -> None:
        async for _ in agent.schedule_runs(decl, on_status=said.append):
            raise AssertionError("a node outside the band hands out no run")

    task = asyncio.ensure_future(until_said())
    for _ in range(100):
        if said:
            break
        await asyncio.sleep(0.1)
    task.cancel()
    assert said[:1] == ["not_eligible"], said
