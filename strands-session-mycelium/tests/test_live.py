"""
The session store against a meshed fleet of ``mycelium`` nodes, which vote, so the store's claims are
decided by a real quorum: a session written through node A's gateway is resumed through node B's; two
agents writing one session on two nodes at once both keep their conversations; two writers of *one*
agent cannot both write an index; a sliding window of 0 resumes without writing over its history.

Needs ``MYCELIUM_STRANDS_PORTS`` — the gateways' ports, comma-separated, at least two (CI starts three).
Skipped without it, except where ``MYCELIUM_STRANDS_LIVE_REQUIRED`` is set (the CI step that starts the
nodes), where it fails instead.
"""

from __future__ import annotations

import json
import os
import threading
import time
import uuid

import pytest
from mycelium import MyceliumAgent
from strands import Agent

from fakes import ScriptedModel, largest_value
from strands.types.exceptions import SessionException

from strands_session_mycelium import IncompleteSession, MyceliumSessionManager, MyceliumSessionRepository, StaleWriter

HOST = os.environ.get("MYCELIUM_TEST_HOST", "127.0.0.1")
PORTS = [p for p in os.environ.get("MYCELIUM_STRANDS_PORTS", "").split(",") if p]
PORT_A, PORT_B = (PORTS + [None, None])[:2]

# A live CI step sets MYCELIUM_STRANDS_LIVE_REQUIRED, so a step that lost its nodes fails here instead of
# skipping green (verification policy rule 3). The skip is a mark, not a module-level skip, so the tests
# stay collected — the test universe lists them and the coverage job sees whether they ran.
if os.environ.get("MYCELIUM_STRANDS_LIVE_REQUIRED") and not (PORT_A and PORT_B):
    raise RuntimeError("MYCELIUM_STRANDS_LIVE_REQUIRED is set but MYCELIUM_STRANDS_PORTS names fewer than two nodes: this live suite would skip")

pytestmark = pytest.mark.skipif(
    not (PORT_A and PORT_B),
    reason="no Mycelium fleet configured (MYCELIUM_STRANDS_PORTS)",
)


def node(port: str) -> MyceliumAgent:
    return MyceliumAgent(HOST, int(port))


def resume(port: str, session: str, agent_id: str, replies: list[str], *, within: float = 30.0,
           writer: str | None = None, **kw) -> Agent:
    """An agent resumed through ``port``'s gateway — polling while the session is still arriving."""
    deadline = time.monotonic() + within
    while True:
        try:
            return Agent(
                agent_id=agent_id,
                model=ScriptedModel(replies),
                session_manager=MyceliumSessionManager(session, agent=node(port), writer=writer or f"node-{port}"),
                callback_handler=None,
                **kw,
            )
        except IncompleteSession:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.2)


def arrived(port: str, session: str, agent_id: str, count: int) -> bool:
    row = node(port).get(f"strands/{session}/agent/{agent_id}/count")
    return row is not None and json.loads(row)["count"] >= count


def texts(agent: Agent) -> list[str]:
    return [block["text"] for m in agent.messages for block in m["content"] if "text" in block]


def until(predicate, within: float = 30.0):
    deadline = time.monotonic() + within
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() > deadline:
            raise AssertionError("not reached within the bound")
        time.sleep(0.2)


def test_a_session_written_on_one_node_resumes_on_another():
    session = f"live-{uuid.uuid4().hex}"
    m = MyceliumSessionManager(session, agent=node(PORT_A), writer=f"node-{PORT_A}")
    a = Agent(agent_id="planner", model=ScriptedModel(["bakery first", "then the shelter"]), session_manager=m,
              callback_handler=None)
    a("where do we start?")
    a("and after?")
    receipt = m.last_receipt
    # The CI nodes run without [persistence]: the gateway says it promised nothing on disk.
    assert receipt is not None and receipt.operation_id
    assert receipt.local_durability == "not_configured"

    written = texts(a)
    # Resume where the agent's rows have arrived. Before then node B cannot tell this agent from a new one,
    # and its claim — another writer's — answers IncompleteSession (test_claims.py has that case).
    until(lambda: arrived(PORT_B, session, "planner", 4))
    b = resume(PORT_B, session, "planner", ["kitchen"])
    assert texts(b) == written
    b("and last?")

    # Node A sees B's continuation once it gossips back: one conversation, nothing overwritten.
    until(lambda: arrived(PORT_A, session, "planner", 6))
    back = resume(PORT_A, session, "planner", [])
    assert texts(back) == written + ["and last?", "kitchen"]


def test_two_agents_on_two_nodes_in_one_session_keep_both_conversations():
    session = f"live-{uuid.uuid4().hex}"
    MyceliumSessionManager(session, agent=node(PORT_A), writer=f"node-{PORT_A}")  # the session row, once
    until(lambda: node(PORT_B).get(f"strands/{session}/session"))

    def converse(port: str, agent_id: str) -> None:
        agent = resume(port, session, agent_id, [f"{agent_id}-{i}" for i in range(3)])
        for i in range(3):
            agent(f"{agent_id} turn {i}")

    workers = [threading.Thread(target=converse, args=(PORT_A, "collector")),
               threading.Thread(target=converse, args=(PORT_B, "courier"))]
    for w in workers:
        w.start()
    for w in workers:
        w.join()

    for port in (PORT_A, PORT_B):
        for agent_id in ("collector", "courier"):
            until(lambda: arrived(port, session, agent_id, 6))
            got = resume(port, session, agent_id, [])
            assert texts(got) == [x for i in range(3) for x in (f"{agent_id} turn {i}", f"{agent_id}-{i}")]


def test_the_largest_row_the_store_sends_is_one_the_gateway_takes():
    """The size check matches the gateway's real request limit: the largest row passes, one more is refused."""
    repo = MyceliumSessionRepository(node(PORT_A))
    key = f"strands/live-{uuid.uuid4().hex}/big"
    n = largest_value(key)
    repo._write(key, {"x": "y" * n})  # a 413 here would mean the store's limit is above the gateway's
    assert len(node(PORT_A).get(key)) > n
    with pytest.raises(SessionException, match="at most"):
        repo._write(key, {"x": "y" * (n + 3)})
    node(PORT_A).delete(key)


def test_two_writers_of_one_agent_on_two_nodes_never_write_over_each_other():
    session = f"live-{uuid.uuid4().hex}"
    MyceliumSessionManager(session, agent=node(PORT_A), writer=f"node-{PORT_A}")
    until(lambda: node(PORT_B).get(f"strands/{session}/session"))
    Agent(agent_id="planner", model=ScriptedModel([]), callback_handler=None,
          session_manager=MyceliumSessionManager(session, agent=node(PORT_A), writer=f"node-{PORT_A}"))
    until(lambda: arrived(PORT_B, session, "planner", 0))
    on_a = resume(PORT_A, session, "planner", [f"a{i}" for i in range(5)])
    on_b = resume(PORT_B, session, "planner", [f"b{i}" for i in range(5)])
    refused: list[str] = []
    stopped: list[str] = []

    def converse(agent: Agent, name: str) -> None:
        for i in range(5):
            try:
                agent(f"{name} turn {i}")
            except StaleWriter:
                refused.append(name)
                return
            except SessionException:  # a claim that met no quorum in time under the contention (504)
                stopped.append(name)
                return

    workers = [threading.Thread(target=converse, args=(on_a, "a")), threading.Thread(target=converse, args=(on_b, "b"))]
    for w in workers:
        w.start()
    for w in workers:
        w.join()
    assert refused or stopped, "two writers of one agent both wrote every turn"
    # Every index holds the message its claimant wrote: each turn is a writer's own prompt then its reply.
    winner_count = max(json.loads(node(p).get(f"strands/{session}/agent/planner/count") or b'{"count":0}')["count"]
                       for p in (PORT_A, PORT_B))
    until(lambda: arrived(PORT_A, session, "planner", winner_count))
    stored = resume(PORT_A, session, "planner", [])
    got = texts(stored)
    for prompt, reply in zip(got[::2], got[1::2]):
        assert prompt.split(" ")[0] == reply[0], f"{prompt!r} answered by {reply!r}: a message was written over"


def test_a_window_of_zero_resumes_on_another_node_without_writing_over_its_history():
    from strands.agent.conversation_manager import SlidingWindowConversationManager as Window

    session = f"live-{uuid.uuid4().hex}"
    first = Agent(agent_id="planner", model=ScriptedModel(["r1", "r2"]), callback_handler=None,
                  conversation_manager=Window(window_size=0),
                  session_manager=MyceliumSessionManager(session, agent=node(PORT_A), writer=f"node-{PORT_A}"))
    first("one")
    first("two")
    until(lambda: arrived(PORT_B, session, "planner", 4))
    again = resume(PORT_B, session, "planner", ["r3"], conversation_manager=Window(window_size=0))
    again("three")
    row = lambda i: json.loads(node(PORT_B).get(f"strands/{session}/agent/planner/msg/{i:010d}"))  # noqa: E731
    assert row(0)["message"]["content"][0]["text"] == "one"
    assert row(4)["message"]["content"][0]["text"] == "three"
