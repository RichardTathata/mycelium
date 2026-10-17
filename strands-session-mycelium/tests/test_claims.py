"""
Claims: the edges a node's own view cannot see, closed by deciding each record and message index once,
fleet-wide. Two ``FakeGateway``s sharing one ``decided`` dict are two nodes of one fleet — each with its
own rows, which a test holds back to make a node that is behind — and one set of claims no node can
decide twice, which is what a quorum gives the real store (``tests/test_live.py`` runs it on a mesh).
"""

from __future__ import annotations

import json

import pytest
from strands import Agent
from strands.types.exceptions import SessionException
from strands.types.session import SessionAgent

from fakes import FakeGateway, ScriptedModel
from strands_session_mycelium import IncompleteSession, MyceliumSessionManager, StaleWriter

MSG = "strands/s1/agent/planner/msg/{:010d}"


def fleet() -> tuple[FakeGateway, FakeGateway]:
    decided: dict[str, bytes] = {}
    return FakeGateway(decided=decided), FakeGateway(decided=decided)


def agent(gw: FakeGateway, replies: list[str], *, writer: str, agent_id: str = "planner", **kw) -> Agent:
    return Agent(agent_id=agent_id, model=ScriptedModel(replies), callback_handler=None,
                 session_manager=MyceliumSessionManager("s1", agent=gw, writer=writer), **kw)


def text(gw: FakeGateway, i: int) -> str:
    return json.loads(gw.rows[MSG.format(i)])["message"]["content"][0]["text"]


def arrive(src: FakeGateway, dst: FakeGateway, keep=lambda k: True) -> None:
    """Gossip: ``src``'s rows reach ``dst`` — those ``keep`` admits."""
    dst.rows.update({k: v for k, v in src.rows.items() if keep(k)})
    dst.seen |= {k for k in src.seen if keep(k)}


def test_a_node_behind_as_a_whole_cannot_write_over_what_it_has_not_seen():
    a, b = fleet()
    agent(a, ["r1", "r2"], writer="node-a")("one")
    agent(a, ["r2"], writer="node-a")("two")  # messages 0..3 on A
    # B holds everything but A's second turn — including an older count: it reads a shorter
    # conversation without error, which no view of its own can tell from the whole one.
    arrive(a, b, keep=lambda k: k not in (MSG.format(2), MSG.format(3)))
    b.rows["strands/s1/agent/planner/count"] = b'{"count":2}'
    m = MyceliumSessionManager("s1", agent=b, writer="node-b")
    stale = Agent(agent_id="planner", model=ScriptedModel(["late reply"]), callback_handler=None, session_manager=m)
    assert len(stale.messages) == 2
    with pytest.raises(StaleWriter):
        stale("three")
    assert MSG.format(2) not in b.rows  # B wrote nothing over A's message 2
    with pytest.raises(StaleWriter):  # fenced: its agent state is not written over A's either
        m.repository.update_agent("s1", SessionAgent.from_agent(stale))
    arrive(a, b)
    assert text(b, 2) == "two" and text(b, 3) == "r2"
    # Re-created where the session has arrived, B carries on after A's messages.
    agent(b, ["r3"], writer="node-b")("three")
    assert text(b, 4) == "three"


def test_an_agent_created_elsewhere_and_not_arrived_is_incomplete_not_new():
    a, b = fleet()
    agent(a, ["r1"], writer="node-a")("one")
    arrive(a, b, keep=lambda k: "/agent/" not in k)  # the session and its claims, not the agent
    with pytest.raises(IncompleteSession):
        agent(b, [], writer="node-b")
    assert not [k for k in b.rows if "/agent/planner" in k and "/claim/" not in k]


def test_two_writers_of_one_agent_cannot_both_write_one_index():
    a, b = fleet()
    agent(a, [], writer="node-a")
    arrive(a, b)
    on_a = agent(a, ["from a"], writer="node-a")
    on_b = agent(b, ["from b"], writer="node-b")
    on_a("hello from a")
    with pytest.raises(StaleWriter):
        on_b("hello from b")
    arrive(a, b)
    assert text(b, 0) == "hello from a"


def test_two_nodes_creating_one_session_agree_on_its_generation():
    a, b = fleet()
    MyceliumSessionManager("s1", agent=a, writer="node-a")
    with pytest.raises(IncompleteSession):  # B's view has none of it: its claim is decided, not arrived
        MyceliumSessionManager("s1", agent=b, writer="node-b")
    arrive(a, b)
    m_a = MyceliumSessionManager("s1", agent=a, writer="node-a")
    m_b = MyceliumSessionManager("s1", agent=b, writer="node-b")
    assert m_a.repository._generation("s1") == m_b.repository._generation("s1")


def test_a_deleted_sessions_id_is_refused_by_name_not_reused():
    gw = FakeGateway()
    agent(gw, ["r1"], writer="node-a")("one")
    MyceliumSessionManager("s1", agent=gw, writer="node-a").repository.delete_session("s1")
    with pytest.raises(SessionException, match="not reused"):
        MyceliumSessionManager("s1", agent=gw, writer="node-a")


def test_a_claim_whose_row_was_never_written_is_taken_back_by_its_writer():
    gw = FakeGateway()
    first = agent(gw, ["r1", "r2"], writer="node-a")
    first("one")
    gw.fail_once.add(MSG.format(2))
    with pytest.raises(SessionException):
        first("two")  # message 2 claimed, its row not written — and then this process is gone
    again = agent(gw, ["r3"], writer="node-a")  # restarted on the same host: the same writer
    assert len(again.messages) == 2
    again("three")
    assert text(gw, 2) == "three"


def test_a_claim_whose_row_was_never_written_is_not_taken_by_another_writer():
    gw = FakeGateway()
    first = agent(gw, ["r1", "r2"], writer="node-a")
    first("one")
    gw.fail_once.add(MSG.format(2))
    with pytest.raises(SessionException):
        first("two")
    with pytest.raises(StaleWriter):
        agent(gw, ["r3"], writer="node-b")("three")


def test_a_swarm_created_elsewhere_and_not_arrived_is_incomplete_not_new():
    from strands.multiagent import Swarm

    a, b = fleet()
    collector = Agent(name="collector", model=ScriptedModel(["40 loaves"]), callback_handler=None)
    Swarm([collector], id="redistribution",
          session_manager=MyceliumSessionManager("s1", agent=a, writer="node-a"))("plan")
    arrive(a, b, keep=lambda k: "/multi/" not in k)
    late = Agent(name="collector", model=ScriptedModel(["x"]), callback_handler=None)
    with pytest.raises(IncompleteSession):
        Swarm([late], id="redistribution",
              session_manager=MyceliumSessionManager("s1", agent=b, writer="node-b"))


def test_without_claims_the_store_needs_no_consensus():
    class KvOnly(FakeGateway):
        def consistent_set(self, key, value):
            raise AssertionError("claims=False made a claim")

        def consistent_get(self, key):
            raise AssertionError("claims=False read a claim")

    gw = KvOnly()
    m = MyceliumSessionManager("s1", agent=gw, claims=False)
    Agent(agent_id="planner", model=ScriptedModel(["r1"]), callback_handler=None, session_manager=m)("one")
    assert text(gw, 1) == "r1"
