"""S12 — Leader Election + Consensus-Durable Config.

All three nodes concurrently call elect_leader("demo") via the gossip consensus.
The cluster must converge on exactly one leader — all nodes must return the same
leader string.

The elected leader then writes a config value via consistent_set. All three
nodes verify they see the same value via consistent_get.
"""

from __future__ import annotations

import concurrent.futures
import httpx
from mycelium import MyceliumAgent
from .helpers import (
    NODE_A_HOST, NODE_B_HOST, NODE_C_HOST, NODE_HTTP_PORT,
    node_url, wait_for_cluster_ready, poll_until, assert_eq,
)

GROUP  = "s12-demo"
CONFIG_KEY = "s12/config/endpoint"
CONFIG_VAL = b"https://api.example.com/v2"


def _elect(host: str) -> str:
    agent = MyceliumAgent(host, NODE_HTTP_PORT)
    return agent.elect_leader(GROUP)


def run() -> None:
    # Cluster is already converged — quick re-check (run.py waits at startup)
    # 30s, not 5. This used to be a free no-op: the readiness check wrote fixed sentinel keys
    # and merely counted them, so once they had propagated in the first scenario every later call
    # passed on the residue regardless of the timeout. It is now a real round trip — a freshly
    # nonced write from every node, read back by every node with its exact value — and a fresh
    # write needs more than 5s to cross three gossiping nodes on a loaded CI box, especially as
    # the first scenario after start-up.
    wait_for_cluster_ready(timeout=30)

    agents = {
        NODE_A_HOST: MyceliumAgent(NODE_A_HOST, NODE_HTTP_PORT),
        NODE_B_HOST: MyceliumAgent(NODE_B_HOST, NODE_HTTP_PORT),
        NODE_C_HOST: MyceliumAgent(NODE_C_HOST, NODE_HTTP_PORT),
    }

    # Step 0 — establish the electorate. This scenario ran for months without it: nothing joined
    # `s12-demo`, so every node proposed into an EMPTY roster, which used to be counted as one
    # member with a quorum of one and satisfied by the proposer's own self-vote. Three nodes
    # therefore held three singleton elections and agreed only by luck of gossip timing.
    # An election with no electorate is now refused, so the setup is no longer optional — which is
    # the point: the test could not previously fail for the right reason.
    for host in agents:
        r = httpx.post(f"{node_url(host)}/gateway/mesh/group",
                       json={"group": GROUP}, timeout=5.0)
        r.raise_for_status()

    # And the roster must be COMPLETE on every node before anyone proposes. A partial view is
    # refused too, but waiting here means the scenario tests the election rather than the race to
    # see the roster.
    def roster_complete() -> bool:
        for host in agents:
            r = httpx.get(f"{node_url(host)}/gateway/mesh/group",
                          params={"group": GROUP}, timeout=3.0)
            if r.status_code >= 400 or len(r.json().get("members", [])) != len(agents):
                return False
        return True

    assert_true = poll_until(roster_complete, timeout=30)
    if not assert_true:
        seen = {
            h: httpx.get(f"{node_url(h)}/gateway/mesh/group",
                         params={"group": GROUP}, timeout=3.0).json()
            for h in agents
        }
        raise AssertionError(f"group {GROUP} roster never converged on all nodes: {seen}")

    # Step 1 — all three nodes call elect_leader concurrently
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        futures = {h: pool.submit(_elect, h) for h in agents}
        leaders = {h: f.result(timeout=30) for h, f in futures.items()}

    # All must agree on the same leader
    unique_leaders = set(leaders.values())
    if len(unique_leaders) != 1:
        raise AssertionError(
            f"Nodes disagree on leader: {leaders}"
        )

    elected = next(iter(unique_leaders))
    if not elected:
        raise AssertionError("elect_leader returned empty string")

    # Step 2 — whichever node is the elected leader writes config
    # (Use node-a as the writer regardless; consistent_set is ballot-serialized
    # from any node — it goes through consensus, not just the elected host.)
    agents[NODE_A_HOST].consistent_set(CONFIG_KEY, CONFIG_VAL)

    # Step 3 — all nodes must read the same config value via consistent_get
    def all_agree() -> bool:
        vals = [a.consistent_get(CONFIG_KEY) for a in agents.values()]
        return all(v == CONFIG_VAL for v in vals)

    if not poll_until(all_agree, timeout=20):
        vals = {h: a.consistent_get(CONFIG_KEY) for h, a in agents.items()}
        raise AssertionError(
            f"Nodes did not converge on config value within 20s: {vals}"
        )
