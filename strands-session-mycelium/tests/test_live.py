"""
The session store against two meshed Mycelium nodes (CI's reason-node pair): a session written
through node A's gateway is resumed through node B's, and two agents writing one session on the two
nodes at once both keep their conversations.

Needs ``MYCELIUM_TEST_PORT`` and ``MYCELIUM_TEST_PORT_B``; skipped without them, except where
``MYCELIUM_STRANDS_LIVE_REQUIRED`` is set (the CI step that starts the nodes), where it fails instead.
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

from fakes import ScriptedModel
from strands_session_mycelium import IncompleteSession, MyceliumSessionManager

HOST = os.environ.get("MYCELIUM_TEST_HOST", "127.0.0.1")
PORT_A = os.environ.get("MYCELIUM_TEST_PORT")
PORT_B = os.environ.get("MYCELIUM_TEST_PORT_B")

if not (PORT_A and PORT_B):
    if os.environ.get("MYCELIUM_STRANDS_LIVE_REQUIRED"):
        raise RuntimeError("MYCELIUM_STRANDS_LIVE_REQUIRED is set but MYCELIUM_TEST_PORT/_B are not")
    pytest.skip("no Mycelium nodes configured (MYCELIUM_TEST_PORT, MYCELIUM_TEST_PORT_B)", allow_module_level=True)


def node(port: str) -> MyceliumAgent:
    return MyceliumAgent(HOST, int(port))


def resume(port: str, session: str, agent_id: str, replies: list[str], *, within: float = 30.0) -> Agent:
    """An agent resumed through ``port``'s gateway — polling while the session is still arriving."""
    deadline = time.monotonic() + within
    while True:
        try:
            return Agent(
                agent_id=agent_id,
                model=ScriptedModel(replies),
                session_manager=MyceliumSessionManager(session, agent=node(port)),
                callback_handler=None,
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
    m = MyceliumSessionManager(session, agent=node(PORT_A))
    a = Agent(agent_id="planner", model=ScriptedModel(["bakery first", "then the shelter"]), session_manager=m,
              callback_handler=None)
    a("where do we start?")
    a("and after?")
    receipt = m.last_receipt
    assert receipt is not None and receipt.operation_id and receipt.local_durability is not None

    written = texts(a)
    # Resume where the agent's rows have arrived. Before then node B cannot tell this agent from a new
    # one, and Strands would start it afresh there (the README's "Resuming on another node").
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
    MyceliumSessionManager(session, agent=node(PORT_A))  # the session row, once
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
