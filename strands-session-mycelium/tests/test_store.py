"""
The session store against an in-memory gateway — no node. A real Strands ``Agent`` with a scripted
model saves and resumes through it; the cases a live mesh produces only by timing (a row gossiped
ahead of another) are made here by removing a row.
"""

from __future__ import annotations

import json

import pytest
from strands import Agent
from strands.types.exceptions import SessionException

from fakes import FakeGateway, ScriptedModel
from strands_session_mycelium import IncompleteSession, MyceliumSessionManager, MyceliumSessionRepository
from strands_session_mycelium.store import MAX_ROW_BYTES


def manager(gw: FakeGateway, session: str = "s1", **kw) -> MyceliumSessionManager:
    return MyceliumSessionManager(session, agent=gw, **kw)


def run_agent(gw: FakeGateway, prompts: list[str], *, agent_id: str = "planner", session: str = "s1") -> Agent:
    agent = Agent(
        agent_id=agent_id,
        model=ScriptedModel([f"reply to {p}" for p in prompts]),
        session_manager=manager(gw, session),
        callback_handler=None,
    )
    for p in prompts:
        agent(p)
    return agent


def texts(agent: Agent) -> list[str]:
    return [block["text"] for m in agent.messages for block in m["content"] if "text" in block]


# ── a session saved is a session resumed ────────────────────────────────────


def test_a_conversation_and_its_state_resume_in_another_manager():
    gw = FakeGateway()
    m = manager(gw)
    first = Agent(agent_id="planner", model=ScriptedModel(["r1", "r2"]), session_manager=m, callback_handler=None)
    first("pick up at the bakery")
    first.state.set("route", ["bakery", "shelter"])
    first("drop at the shelter")  # the state change is synced with this turn

    again = Agent(agent_id="planner", model=ScriptedModel(["ok"]), session_manager=manager(gw), callback_handler=None)
    assert texts(again) == texts(first)
    assert again.state.get("route") == ["bakery", "shelter"]

    # The resumed agent appends after what it read, at the next index — nothing overwritten.
    again("and the community kitchen")
    count = json.loads(gw.rows["strands/s1/agent/planner/count"])["count"]
    assert count == 6
    assert len([k for k in gw.rows if "/msg/" in k]) == 6


def test_two_agents_in_one_session_keep_each_others_writes():
    gw = FakeGateway()
    a = Agent(agent_id="collector", model=ScriptedModel(["a1", "a2"]), session_manager=manager(gw), callback_handler=None)
    b = Agent(agent_id="courier", model=ScriptedModel(["b1", "b2"]), session_manager=manager(gw), callback_handler=None)
    a("one")
    b("two")
    a("three")
    b("four")
    for agent_id, expected in (("collector", ["one", "a1", "three", "a2"]), ("courier", ["two", "b1", "four", "b2"])):
        back = Agent(agent_id=agent_id, model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)
        assert texts(back) == expected


# ── incomplete is never empty ───────────────────────────────────────────────


def test_a_missing_middle_message_is_incomplete_not_a_shorter_conversation():
    gw = FakeGateway()
    run_agent(gw, ["one", "two"])
    gone = "strands/s1/agent/planner/msg/0000000001"
    del gw.rows[gone]
    with pytest.raises(IncompleteSession) as e:
        Agent(agent_id="planner", model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)
    assert e.value.missing == [gone]


def test_a_missing_last_message_is_incomplete_because_the_count_names_it():
    gw = FakeGateway()
    run_agent(gw, ["one", "two"])
    gone = "strands/s1/agent/planner/msg/0000000003"
    del gw.rows[gone]
    with pytest.raises(IncompleteSession) as e:
        Agent(agent_id="planner", model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)
    assert e.value.missing == [gone]


def test_messages_ahead_of_a_lagging_count_are_read_and_not_overwritten():
    gw = FakeGateway()
    run_agent(gw, ["one", "two"])
    gw.rows["strands/s1/agent/planner/count"] = b'{"count":2}'  # the count row has not caught up
    again = Agent(agent_id="planner", model=ScriptedModel(["r"]), session_manager=manager(gw), callback_handler=None)
    assert texts(again) == ["one", "reply to one", "two", "reply to two"]
    again("three")
    assert json.loads(gw.rows["strands/s1/agent/planner/msg/0000000004"])["message"]["content"][0]["text"] == "three"
    assert json.loads(gw.rows["strands/s1/agent/planner/msg/0000000003"])["message"]["content"][0]["text"] == "reply to two"


def test_messages_without_their_agent_row_are_incomplete():
    gw = FakeGateway()
    run_agent(gw, ["one"])
    del gw.rows["strands/s1/agent/planner"]
    with pytest.raises(IncompleteSession) as e:
        Agent(agent_id="planner", model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)
    assert e.value.missing == ["strands/s1/agent/planner"]


def test_an_agent_without_its_session_row_is_incomplete_not_a_new_session():
    gw = FakeGateway()
    run_agent(gw, ["one"])
    del gw.rows["strands/s1/session"]
    with pytest.raises(IncompleteSession):
        manager(gw)
    assert "strands/s1/session" not in gw.rows  # nothing was created over it


def test_messages_without_a_count_are_incomplete():
    gw = FakeGateway()
    run_agent(gw, ["one"])
    del gw.rows["strands/s1/agent/planner/count"]
    with pytest.raises(IncompleteSession) as e:
        Agent(agent_id="planner", model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)
    assert e.value.missing == ["strands/s1/agent/planner/count"]


def test_an_unreadable_row_is_an_error_not_an_absent_one():
    gw = FakeGateway()
    run_agent(gw, ["one"])
    gw.rows["strands/s1/agent/planner"] = b"\xff not json"
    with pytest.raises(SessionException, match="unreadable"):
        Agent(agent_id="planner", model=ScriptedModel([]), session_manager=manager(gw), callback_handler=None)


def test_a_gateway_refusal_is_an_error_and_creates_nothing():
    gw = FakeGateway()
    gw.fail_with = 401
    with pytest.raises(SessionException, match="401"):
        manager(gw)
    assert gw.writes == []


def test_a_brand_new_session_is_created_once():
    gw = FakeGateway()
    manager(gw)
    assert list(gw.rows) == ["strands/s1/session"]
    manager(gw)  # read back, not recreated
    assert gw.writes == ["strands/s1/session"]


# ── receipts ────────────────────────────────────────────────────────────────


def test_every_write_hands_back_the_sdks_receipt():
    gw = FakeGateway(durability="buffered")
    seen = []
    m = manager(gw, on_receipt=lambda key, r: seen.append((key, r.local_durability)))
    Agent(agent_id="planner", model=ScriptedModel(["r"]), session_manager=m, callback_handler=None)("hello")
    assert m.last_receipt is not None and m.last_receipt.local_durability == "buffered"
    assert [k for k, _ in seen] == gw.writes and len(seen) > 3


def test_require_on_disk_refuses_a_write_that_is_not():
    gw = FakeGateway(durability="not_configured")
    with pytest.raises(SessionException, match="not on disk"):
        manager(gw, require_on_disk=True)


# ── keys ────────────────────────────────────────────────────────────────────


def test_an_id_cannot_forge_a_separator():
    gw = FakeGateway()
    repo = MyceliumSessionRepository(gw)
    assert repo._agent_key("s/1", "a/msg") == "strands/s%2F1/agent/a%2Fmsg"
    with pytest.raises(SessionException):
        repo._agent_key("", "a")


def test_a_row_past_the_kv_write_cap_is_refused_before_it_is_sent():
    gw = FakeGateway()
    repo = MyceliumSessionRepository(gw)
    with pytest.raises(SessionException, match="at most"):
        repo._write("strands/s1/big", {"x": "y" * MAX_ROW_BYTES})
    assert gw.writes == []


# ── multi-agent state ───────────────────────────────────────────────────────


class Orchestrator:
    def __init__(self, id: str, state: dict) -> None:
        self.id = id
        self.state = state

    def serialize_state(self) -> dict:
        return dict(self.state)


def test_a_multi_agent_state_round_trips_and_an_update_needs_one():
    gw = FakeGateway()
    repo = MyceliumSessionRepository(gw)
    with pytest.raises(SessionException, match="does not exist"):
        repo.update_multi_agent("s1", Orchestrator("swarm", {"n": 1}))
    repo.create_multi_agent("s1", Orchestrator("swarm", {"n": 1}))
    repo.update_multi_agent("s1", Orchestrator("swarm", {"n": 2}))
    assert repo.read_multi_agent("s1", "swarm") == {"n": 2}
    assert repo.read_multi_agent("s1", "graph") is None


def test_a_swarms_shared_state_lives_in_the_session():
    from strands.multiagent import Swarm

    gw = FakeGateway()
    collector = Agent(name="collector", model=ScriptedModel(["collected 40 loaves"]), callback_handler=None)
    swarm = Swarm([collector], id="redistribution", session_manager=manager(gw, "s9"))
    assert swarm("plan the pickups").status.value == "completed"
    row = json.loads(gw.rows["strands/s9/multi/redistribution"])
    assert row["type"] == "swarm" and row["node_history"] == ["collector"]
