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
from strands_session_mycelium import IncompleteSession, MyceliumSessionManager, MyceliumSessionRepository, StaleWriter

MSG = "strands/s1/agent/planner/msg/{:010d}"
AGENT = "strands/s1/agent/planner"


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
    # B holds nothing of A's second turn — not its rows, not its claims — and an older count: it reads a
    # shorter conversation without error, which no view of its own can tell from the whole one.
    late = ("/msg/0000000002", "/msg/0000000003")
    arrive(a, b, keep=lambda k: not k.endswith(late) and "/v/" not in k.rsplit("/agent/", 1)[-1][len("planner"):] or k.endswith("/v/0"))
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


def test_a_node_that_holds_a_later_claim_but_not_its_row_waits():
    a, b = fleet()
    agent(a, ["r1", "r2"], writer="node-a")("one")
    agent(a, ["r2"], writer="node-a")("two")
    arrive(a, b, keep=lambda k: not k.endswith(("/msg/0000000002", "/msg/0000000003")) or "/claim/" in k)
    with pytest.raises(IncompleteSession) as e:
        agent(b, [], writer="node-b")
    assert e.value.missing == [MSG.format(2)]


def test_the_same_writer_restarted_behind_its_own_writes_cannot_write_over_them():
    """Review finding 1 of 57e405d5: a writer id is not a licence — only the write a claim names is."""
    a, b = fleet()
    agent(a, ["r1"], writer="host-h")("one")
    agent(a, ["r2"], writer="host-h")("two")
    arrive(a, b, keep=lambda k: not k.endswith(("/msg/0000000002", "/msg/0000000003"))
           and not ("/claim/" in k and "/msg/" in k and k.endswith(("2", "3"))) and "/v/" not in k or k.endswith("/v/0"))
    b.rows["strands/s1/agent/planner/count"] = b'{"count":2}'
    with pytest.raises(StaleWriter):
        agent(b, ["late"], writer="host-h")("three")
    arrive(b, a)
    assert text(a, 2) == "two"


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
    m_a = MyceliumSessionManager("s1", agent=a, writer="node-a")
    # B's view has none of it, so B creates the session too — and, losing the claim, learns its decision.
    m_b = MyceliumSessionManager("s1", agent=b, writer="node-b")
    generation = m_a.repository._generation("s1")
    assert generation is not None and generation == m_b.repository._generation("s1")


def test_a_deleted_sessions_id_is_refused_by_name_not_reused():
    gw = FakeGateway()
    agent(gw, ["r1"], writer="node-a")("one")
    MyceliumSessionManager("s1", agent=gw, writer="node-a").repository.delete_session("s1")
    with pytest.raises(SessionException, match="not reused"):
        MyceliumSessionManager("s1", agent=gw, writer="node-a")


@pytest.mark.parametrize("writer", ["node-a", "node-b"])
def test_a_message_claimed_and_never_written_is_read_from_its_claim(writer):
    """Review finding 3 of 57e405d5: a writer that claimed and died blocks no one — the claim names its write."""
    gw = FakeGateway()
    first = agent(gw, ["r1", "r2"], writer="node-a")
    first("one")
    gw.fail_once.add(MSG.format(2))
    with pytest.raises(SessionException):
        first("two")  # message 2 claimed, its row not written — and then this writer is gone
    again = agent(gw, ["r3"], writer=writer)  # its restart, or a replacement on another host
    assert [b["text"] for m in again.messages for b in m["content"]] == ["one", "r1", "two"]
    again("three")
    # Message 2 is served from its claim's pending copy, never rewritten by a reader; 3 follows it.
    assert MSG.format(2) not in gw.rows and text(gw, 3) == "three"
    after = agent(gw, [], writer="node-c")
    assert [b["text"] for m in after.messages for b in m["content"]][:4] == ["one", "r1", "two", "three"]


def test_an_agent_claimed_and_never_written_is_read_from_its_claim():
    gw = FakeGateway()
    MyceliumSessionManager("s1", agent=gw, writer="pod-1")
    gw.fail_once.add("strands/s1/agent/planner/count")
    with pytest.raises(SessionException):
        agent(gw, [], writer="pod-1")  # its version 0 claimed; neither count nor row written
    again = agent(gw, ["r1"], writer="pod-2")
    again("one")
    assert text(gw, 0) == "one"


def test_a_swarm_resumed_behind_its_newer_state_cannot_write_over_it():
    """Review finding 2 of 57e405d5: a multi-agent state's updates are claimed by version."""
    from strands.multiagent import Swarm

    a, b = fleet()

    def swarm(gw, w, reply):
        c = Agent(name="collector", model=ScriptedModel([reply]), callback_handler=None)
        return Swarm([c], id="redistribution", session_manager=MyceliumSessionManager("s1", agent=gw, writer=w))

    swarm(a, "node-a", "first")("plan 1")
    arrive(a, b)
    swarm(a, "node-a", "second")("plan 2")
    key = "strands/s1/multi/redistribution"
    older = b.rows[key]
    with pytest.raises(StaleWriter):
        swarm(b, "node-b", "stale")("plan 3 from behind")  # B holds the older state, not its later claims
    assert b.rows[key] == older  # B wrote no state of its own over the swarm's
    assert json.loads(a.rows[key])["mycelium_version"] > json.loads(older)["mycelium_version"]


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


# ── a reader never writes (the claims round's confirmation, N1) ─────────────


def test_a_reader_serving_a_claimed_version_writes_nothing_over_a_newer_one():
    """A write a reader makes from its own lagging view is a last-writer-wins write with a newer clock: it
    would bury whatever the claimant wrote since. A reader takes a claimed write from its pending copy and
    writes nothing."""
    from strands.types.session import Session, SessionType

    a, b = fleet()
    ra = MyceliumSessionRepository(a, writer="node-a")
    ra.create_session(Session(session_id="s1", session_type=SessionType.AGENT))
    record = SessionAgent(agent_id="planner", state={"v": 0}, conversation_manager_state={})
    ra.create_agent("s1", record)
    ra.read_agent("s1", "planner")
    arrive(a, b)
    record.state = {"v": 1}
    ra.update_agent("s1", record)
    # B holds version 1's claim and its pending copy (the tombstone after the row has not reached it), not
    # its row; version 2 is then written on A.
    claim = next(k for k in a.decided if k.endswith("/agent/planner/v/1"))
    token = a.decided[claim].decode().split("\n")[1]
    arrive(a, b, keep=lambda k: "/claim/" in k)
    b.rows[f"strands/s1/pending/{token}"] = a.rows[AGENT]
    record.state = {"v": 2}
    ra.update_agent("s1", record)
    before = list(b.writes)
    seen_on_b = MyceliumSessionRepository(b, writer="node-b").read_agent("s1", "planner")
    assert seen_on_b.state == {"v": 1}  # the newest version B can establish
    assert b.writes == before  # and nothing written for it
    assert json.loads(a.rows[AGENT])["state"] == {"v": 2}


def test_a_reader_serving_a_claimed_message_writes_nothing_over_its_redaction():
    a, b = fleet()
    m = MyceliumSessionManager("s1", agent=a, writer="node-a")
    talker = Agent(agent_id="planner", model=ScriptedModel(["your card is 4111"]), callback_handler=None,
                   session_manager=m)
    talker("what card is on file?")
    original = a.rows[MSG.format(1)]
    token = a.decided[next(k for k in a.decided if k.endswith("/msg/0000000001"))].decode().split("\n")[1]
    m.redact_latest_message({"role": "assistant", "content": [{"text": "[redacted]"}]}, talker)
    assert "redact_message" in a.rows[MSG.format(1)].decode() and '"[redacted]"' in a.rows[MSG.format(1)].decode()
    arrive(a, b, keep=lambda k: k != MSG.format(1))
    b.rows[f"strands/s1/pending/{token}"] = original  # the pending copy's tombstone not yet arrived
    before = list(b.writes)
    MyceliumSessionRepository(b, writer="node-b").list_messages("s1", "planner")
    assert b.writes == before  # no unredacted copy written back over the fleet's rows
