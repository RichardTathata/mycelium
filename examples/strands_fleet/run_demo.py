#!/usr/bin/env python3
"""
A Strands fleet on Mycelium (post-360 plan row P): Strands agents on three nodes plan tonight's food
redistribution, keep their session and swarm memory in the fleet, and call fleet skills through ``/a2a``.

    cargo build --example strands_fleet_node --features tls,a2a,compliance
    pip install ./mycelium-py ./strands-session-mycelium cryptography httpx
    python examples/strands_fleet/run_demo.py

It starts the co-op's mandate authority and three nodes (``examples/strands_fleet_node.rs``), runs four acts and
checks each from the fleet's own records, then prints ``STRANDS FLEET OK``:

1. a Strands ``Swarm`` — a collector and a planner — plans the night through the hub node, calling the bakery's
   and the kitchen's skills; its state is kept in the fleet's KV;
2. the planner tries to dispatch a van, which its mandate does not name: the gateway refuses, and the refusal is
   in the hub's evidence journal;
3. the courier, whose mandate names dispatch, sends the first van through the hub;
4. the hub is stopped; the courier is resumed through the depot node from its stored session, with the
   conversation it had, and sends the second van — and the swarm's plan is readable there too.

What the run shows and what it does not: ``examples/strands_fleet/README.md``.
"""

from __future__ import annotations

import json
import os
import secrets
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from mycelium import MyceliumAgent
from strands import Agent
from strands.multiagent import Swarm
from strands_session_mycelium import MyceliumSessionManager, MyceliumSessionRepository

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fleet import FleetCaller, Mandate, ScriptedModel, fleet_tool  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
NODE = ROOT / "target" / "debug" / "examples" / "strands_fleet_node"
if os.environ.get("CARGO_TARGET_DIR"):
    NODE = Path(os.environ["CARGO_TARGET_DIR"]) / "debug" / "examples" / "strands_fleet_node"

OPERATOR_PORT = 7480
NODES = {  # name: (gossip port, gateway port, the skill it serves)
    "hub": (7401, 7411, "bakery/surplus"),
    "depot": (7402, 7412, "depot/dispatch"),
    "kitchen": (7403, 7413, "kitchen/needs"),
}
OP = "skill.invoke:skill:"  # a grant enumerates `skill.invoke:skill:{ns/name}` — absence is denial


def step(title: str) -> None:
    print(f"\n── {title}", flush=True)


def check(condition: bool, what: str) -> None:
    if not condition:
        raise SystemExit(f"FAILED: {what}")
    print(f"   ✓ {what}", flush=True)


def wait_for_line(log: Path, prefix: str, proc: subprocess.Popen, within: float = 120.0) -> str:
    deadline = time.monotonic() + within
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise SystemExit(f"FAILED: {log.name} exited early:\n{log.read_text()}")
        for line in log.read_text().splitlines():
            if line.startswith(prefix):
                return line
        time.sleep(0.2)
    raise SystemExit(f"FAILED: {log.name} never printed {prefix!r}:\n{log.read_text()}")


def until(predicate, within: float = 60.0, what: str = "a condition"):
    deadline = time.monotonic() + within
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    raise SystemExit(f"FAILED: {what} within {within:.0f}s")


def evidence(journal: Path) -> list[dict]:
    out = subprocess.run([NODE, "evidence", str(journal)], capture_output=True, text=True, check=True).stdout
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def texts(agent: Agent) -> list[str]:
    out = []
    for m in agent.messages:
        for b in m["content"]:
            if "text" in b:
                out.append(b["text"])
            elif "toolResult" in b:
                out.extend(c.get("text", "") for c in b["toolResult"]["content"])
    return out


def main() -> None:
    if not NODE.exists():
        raise SystemExit(f"{NODE} is not built: cargo build --example strands_fleet_node --features tls,a2a,compliance")
    work = Path(tempfile.mkdtemp(prefix="strands-fleet-"))
    procs: dict[str, subprocess.Popen] = {}
    try:
        run(work, procs)
    finally:
        for p in procs.values():
            if p.poll() is None:
                p.terminate()
        for p in procs.values():
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()


def run(work: Path, procs: dict[str, subprocess.Popen]) -> None:
    step("the co-op's authority, three nodes, three agents")
    subprocess.run([NODE, "ca-init", str(work / "ca")], check=True, capture_output=True)
    authority_seed = secrets.token_hex(32)
    authority_pub = subprocess.run([NODE, "pubkey", authority_seed], capture_output=True, text=True, check=True).stdout.strip()

    names = ["collector", "planner", "courier"]
    keys = {n: Ed25519PrivateKey.generate() for n in names}
    tokens = {n: secrets.token_hex(16) for n in names}
    public = {
        n: k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()
        for n, k in keys.items()
    }
    agents_env = ",".join(f"{n}:{tokens[n]}:{public[n]}" for n in names)
    providers = ",".join(f"{skill}@127.0.0.1:{gossip}" for gossip, _, skill in NODES.values())

    log = work / "operator.log"
    procs["operator"] = subprocess.Popen(
        [NODE], stdout=log.open("w"), stderr=subprocess.STDOUT,
        env={**os.environ, "ROLE": "operator", "OPERATOR_PORT": str(OPERATOR_PORT), "AUTHORITY_SEED": authority_seed},
    )
    wait_for_line(log, "ready", procs["operator"])
    first = None
    for name, (gossip, http, skill) in NODES.items():
        log = work / f"{name}.log"
        procs[name] = subprocess.Popen(
            [NODE], stdout=log.open("w"), stderr=subprocess.STDOUT,
            env={
                **os.environ, "ROLE": "node", "NAME": name, "PORT": str(gossip), "HTTP_PORT": str(http),
                "PEERS": first or "", "DATA_DIR": str(work / name), "CA_DIR": str(work / "ca"), "SKILL": skill,
                "PROVIDERS": providers, "AUTHORITY_PUB": authority_pub, "AGENTS": agents_env,
                "OPERATOR_URL": f"http://127.0.0.1:{OPERATOR_PORT}", "RUST_LOG": os.environ.get("RUST_LOG", "warn"),
            },
        )
        first = first or f"127.0.0.1:{gossip}"
    for name in NODES:
        print("  ", wait_for_line(work / f"{name}.log", "ready", procs[name]), flush=True)

    import httpx

    def grant(agent: str, skills: list[str]) -> Mandate:
        g = httpx.get(f"http://127.0.0.1:{OPERATOR_PORT}/grant",
                      params={"holder": f"token:fleet/{agent}", "ops": ",".join(OP + s for s in skills)}).json()
        return Mandate(grant=g, key=keys[agent])

    # Dispatching is an appointment: one courier holds the `dispatch` mandate. The collector and the planner hold
    # none — they ask, and the policy lets the fleet's own agents ask.
    mandates = {"collector": None, "planner": None, "courier": grant("courier", ["depot/dispatch"])}
    hub = f"http://127.0.0.1:{NODES['hub'][1]}"
    depot = f"http://127.0.0.1:{NODES['depot'][1]}"
    check(True, "authority up; hub, depot and kitchen meshed, each serving one skill; the courier appointed to dispatch")

    # ── 1. the plan, by a Strands Swarm whose memory is in the fleet ─────────
    step("1. a collector and a planner plan tonight's run (a Strands Swarm, its state in the fleet)")
    callers = {n: FleetCaller(hub, tokens[n], mandates[n]) for n in names}
    collector = Agent(name="collector", callback_handler=None, tools=[fleet_tool(callers["collector"])], model=ScriptedModel([
        ("tool", "fleet_skill", {"skill": "bakery/surplus", "text": "what is left over tonight?"}),
        ("tool", "fleet_skill", {"skill": "kitchen/needs", "text": "who needs meals tonight?"}),
        ("tool", "handoff_to_agent", {"agent_name": "planner", "message": "surplus and needs gathered: plan the runs"}),
        ("text", "handed to the planner"),
    ]))
    planner = Agent(name="planner", callback_handler=None, tools=[fleet_tool(callers["planner"])], model=ScriptedModel([
        ("tool", "fleet_skill", {"skill": "depot/dispatch", "text": "van 1: Rye Lane bakery -> Northside shelter"}),
        ("text", "plan: van 1 Rye Lane bakery -> Northside shelter (30 meals); van 2 Market Street bakery -> "
                 "Eastgate community kitchen (20 meals). Dispatch is the courier's."),
    ]))
    swarm_session = f"redistribution-{uuid.uuid4().hex[:8]}"
    swarm = Swarm([collector, planner], id="tonight", session_manager=MyceliumSessionManager(
        swarm_session, host="127.0.0.1", port=NODES["hub"][1], token=tokens["collector"], writer="coordinator"))
    result = swarm("plan tonight's redistribution")
    check(result.status.value == "completed", "the swarm completed")
    check([n.node_id for n in result.node_history] == ["collector", "planner"], "collector handed to planner")
    asked = [c["skill"] for c in callers["collector"].calls if "answer" in c]
    check(asked == ["bakery/surplus", "kitchen/needs"], "the collector reached the bakery's skill (hub) and the kitchen's (kitchen node)")

    # ── 2. outside a mandate: refused, and on the record ─────────────────────
    step("2. the planner tries to dispatch: it holds no dispatch mandate")
    refusal = callers["planner"].calls[0]
    check("refused" in refusal, f"the gateway refused the planner's depot/dispatch: {refusal.get('refused')}")
    records = evidence(work / "hub" / "evidence.log")
    if os.environ.get("STRANDS_FLEET_SHOW_EVIDENCE"):
        for r in records:
            print("     ", json.dumps(r)[:400])
    # Not `deny`: the policy allows dispatch to anyone *with* a mandate, and the planner could not establish one —
    # so the decision is `indeterminate`, and an indeterminate decision runs nothing.
    refused = [r for r in records if r.get("subject") == "token:fleet/planner" and r.get("resource", "").startswith("skill:depot/dispatch@")]
    check(len(refused) == 1 and refused[0]["decision"] == "indeterminate" and refused[0]["execution"] == "none",
          "the refusal is in the hub's evidence journal: token:fleet/planner, depot/dispatch, indeterminate, nothing executed")
    print(f"     {json.dumps({k: refused[0].get(k) for k in ('subject', 'resource', 'decision', 'execution', 'reason', 'checked')})}")
    permitted = [r for r in records if r.get("subject") == "token:fleet/collector" and r.get("kind") == "decided"
                 and r.get("decision") == "permit"]
    check(len(permitted) == 2, "the collector's two calls are there too, permitted")

    # A grant is an appointment, not a bearer token: the courier's, presented by the planner, proves nothing.
    borrowed = FleetCaller(hub, tokens["planner"], Mandate(grant=mandates["courier"].grant, key=keys["planner"]))
    said = borrowed.call("depot/dispatch", "van 1: Rye Lane bakery -> Northside shelter")
    check(said.startswith("refused"), f"the planner presenting the courier's grant is refused too: {said[:120]}")

    # ── 3. the courier sends the first van through the hub ───────────────────
    step("3. the courier dispatches van 1 through the hub")
    courier_session = f"courier-{uuid.uuid4().hex[:8]}"
    courier = Agent(agent_id="courier", callback_handler=None, tools=[fleet_tool(callers["courier"])],
                    session_manager=MyceliumSessionManager(courier_session, host="127.0.0.1", port=NODES["hub"][1],
                                                           token=tokens["courier"], writer="courier"),
                    model=ScriptedModel([
                        ("tool", "fleet_skill", {"skill": "depot/dispatch", "text": "van 1: Rye Lane bakery -> Northside shelter, 30 meals"}),
                        ("text", "van 1 is on its way"),
                    ]))
    courier("dispatch tonight's runs")
    check(callers["courier"].calls[-1].get("answer", "").startswith("dispatched: van 1"), "the depot dispatched van 1")
    sent = [r for r in evidence(work / "hub" / "evidence.log")
            if r.get("subject") == "token:fleet/courier" and r.get("kind") == "decided"]
    check(len(sent) == 1 and sent[0]["decision"] == "permit"
          and "mandate scope=dispatch holder=token:fleet/courier term=t1 epoch=1" in sent[0]["checked"],
          "the hub permitted it, on the record, naming the appointment it relied on")
    print(f"     {json.dumps({k: sent[0].get(k) for k in ('subject', 'resource', 'decision', 'checked')})}")
    before = texts(courier)

    # ── 4. the hub stops; the work resumes on the depot from the stored session ──
    step("4. the hub is stopped; the courier resumes through the depot")
    procs["hub"].terminate()
    procs["hub"].wait(timeout=30)
    check(True, "hub stopped")
    on_depot = MyceliumAgent("127.0.0.1", NODES["depot"][1], token=tokens["courier"])
    until(lambda: json.loads(on_depot.get(f"strands/{courier_session}/agent/courier/count") or b'{"count":0}')["count"] >= len(courier.messages),
          what="the courier's session on the depot")
    callers["courier"].repoint(depot)
    resumed = Agent(agent_id="courier", callback_handler=None, tools=[fleet_tool(callers["courier"])],
                    session_manager=MyceliumSessionManager(courier_session, host="127.0.0.1", port=NODES["depot"][1],
                                                           token=tokens["courier"], writer="courier"),
                    model=ScriptedModel([
                        ("tool", "fleet_skill", {"skill": "depot/dispatch", "text": "van 2: Market Street bakery -> Eastgate community kitchen, 20 meals"}),
                        ("text", "both vans are on their way"),
                    ]))
    check(texts(resumed) == before, f"the courier resumed with the conversation it had ({len(resumed.messages)} messages, van 1 in it)")
    resumed("and the second run")
    check(callers["courier"].calls[-1].get("answer", "").startswith("dispatched: van 2"), "the depot dispatched van 2 — with the hub gone")
    plan = until(lambda: MyceliumSessionRepository(on_depot, writer="reader").read_multi_agent(swarm_session, "tonight"),
                 what="the swarm's state on the depot")
    check(plan.get("status") == "completed" and plan.get("node_history") == ["collector", "planner"],
          "the swarm's plan is readable on the depot: completed, collector then planner")
    depot_records = evidence(work / "depot" / "evidence.log")
    check(any(r.get("subject") == "token:fleet/courier" and str(r.get("decision", "")).lower() == "permit" for r in depot_records),
          "the depot's own journal has its decision on the courier's second dispatch")

    print("\nSTRANDS FLEET OK", flush=True)


if __name__ == "__main__":
    main()
