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
            assert run.operation_id == f"sched/{decl.id}/{run.slot}"
        assert runs[1].slot > runs[0].slot, "one run per slot, in order"
        with pytest.raises(UnservedRunError):
            agent.schedule_ack(runs[0])
        # The slot the client acknowledged is the fleet's last run.
        last = agent.get(f"sched/{decl.id}/last/{agent.health()['node_id']}")
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
