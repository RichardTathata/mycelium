#!/usr/bin/env python3
"""
A Strands fleet on Mycelium (post-360 plan row P): Strands agents on three nodes plan tonight's food
redistribution, keep their session and swarm memory in the fleet, and call fleet skills through ``/a2a``.

    cargo build --example strands_fleet_node --features tls,a2a,compliance
    pip install ./mycelium-py ./strands-session-mycelium cryptography httpx
    python examples/strands_fleet/run_demo.py

It starts the co-op's mandate authority and three nodes (``examples/strands_fleet_node.rs``), runs four acts and
checks each from the fleet's own records, then prints ``STRANDS FLEET OK``:

1. a Strands ``Swarm`` plans the night: the collector, attached to the kitchen node, asks the bakery's skill (on
   the hub) and the kitchen's; the planner, attached to the hub, plans; the swarm's state is kept in the fleet's KV;
2. the planner tries to dispatch a van. Dispatching is a role the co-op's authority appointed the courier to, and
   the authority signs the planner no grant: the gateway refuses, and the refusal is in the hub's evidence journal;
3. the courier, holding the dispatch appointment, sends the first van through the hub;
4. the hub is stopped; the courier is resumed through the depot node from its stored session, with the
   conversation it had, and sends the second van — and the swarm's plan is readable there too.

What the run shows and what it does not: ``examples/strands_fleet/README.md``.
"""

from __future__ import annotations

import json
import os
import secrets
import shutil
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
        ok = True
    except BaseException:
        ok = False
        print(f"\nlogs and journals kept in {work}", flush=True)
        raise
    finally:
        for p in procs.values():
            if p.poll() is None:
                p.terminate()
        for p in procs.values():
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()
        if ok:
            shutil.rmtree(work, ignore_errors=True)


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
        env={**os.environ, "ROLE": "operator", "OPERATOR_PORT": str(OPERATOR_PORT), "AUTHORITY_SEED": authority_seed,
             "APPOINT": "courier"},
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

    def grant(agent: str) -> httpx.Response:
        return httpx.get(f"http://127.0.0.1:{OPERATOR_PORT}/grant", params={"holder": f"token:fleet/{agent}"})

    # Dispatching is an appointment the authority made at start, to the courier, from its own configuration. The
    # collector and the planner hold none — they ask, and the policy lets the fleet's own agents ask.
    courier_grant = grant("courier")
    check(courier_grant.status_code == 200 and courier_grant.json()["mandate"]["operations"] == [OP + "depot/dispatch"],
          "authority up; it appointed the courier to dispatch, and nothing else")
    check(grant("planner").status_code == 403, "the authority signs the planner no grant on request (403 not_appointed)")
    mandates = {"collector": None, "planner": None, "courier": Mandate(grant=courier_grant.json(), key=keys["courier"])}
    hub = f"http://127.0.0.1:{NODES['hub'][1]}"
    depot = f"http://127.0.0.1:{NODES['depot'][1]}"
    kitchen = f"http://127.0.0.1:{NODES['kitchen'][1]}"
    check(True, "hub, depot and kitchen meshed, each serving one skill")

    # ── 1. the plan, by a Strands Swarm whose memory is in the fleet ─────────
    step("1. a collector and a planner plan tonight's run (a Strands Swarm, its state in the fleet)")
    # Each agent attaches to a node of its own: the collector to the kitchen, the planner and the courier to the hub.
    home = {"collector": kitchen, "planner": hub, "courier": hub}
    callers = {n: FleetCaller(home[n], tokens[n], mandates[n]) for n in names}
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
    swarm_manager = MyceliumSessionManager(
        swarm_session, host="127.0.0.1", port=NODES["hub"][1], token=tokens["planner"], writer="coordinator")
    swarm = Swarm([collector, planner], id="tonight", session_manager=swarm_manager)
    result = swarm("plan tonight's redistribution")
    check(result.status.value == "completed", "the swarm completed")
    check([n.node_id for n in result.node_history] == ["collector", "planner"], "collector handed to planner")
    answers = [c.get("answer", "") for c in callers["collector"].calls]
    check(len(answers) == 2 and answers[0].startswith("surplus tonight") and answers[1].startswith("needs tonight"),
          "the collector, through the kitchen node, got the bakery's answer and the kitchen's")
    asked = [r for r in evidence(work / "kitchen" / "evidence.log")
             if r.get("subject") == "token:fleet/collector" and r.get("kind") == "decided" and r.get("decision") == "permit"]
    check(sorted(r["resource"] for r in asked) == sorted([f"skill:bakery/surplus@127.0.0.1:{NODES['hub'][0]}",
                                                         f"skill:kitchen/needs@127.0.0.1:{NODES['kitchen'][0]}"]),
          "the kitchen node's journal: both permitted, the bakery's resolved to the hub node, the kitchen's to itself")

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

    # A grant names its holder, and the gateway establishes it only for that principal — the caller its bearer
    # names. The courier's grant, presented by the planner, establishes nothing (whatever key signs the proof).
    borrowed = FleetCaller(hub, tokens["planner"], Mandate(grant=mandates["courier"].grant, key=keys["planner"]))
    borrowed.call("depot/dispatch", "van 1: Rye Lane bakery -> Northside shelter")
    errors = json.dumps(borrowed.calls[-1].get("refused", {}).get("data", {}).get("errors", []))
    check("refused" in borrowed.calls[-1] and "holder" in errors,
          f"the planner presenting the courier's grant is refused, on the holder: {errors[:160]}")
    refused = [r for r in evidence(work / "hub" / "evidence.log")
               if r.get("subject") == "token:fleet/planner" and r.get("resource", "").startswith("skill:depot/dispatch@")]
    check(len(refused) == 2 and all(r["execution"] == "none" for r in refused), "both refusals in the hub's journal, nothing executed")

    # ── 3. the courier sends the first van through the hub ───────────────────
    step("3. the courier dispatches van 1 through the hub")
    courier_session = f"courier-{uuid.uuid4().hex[:8]}"
    courier_manager = MyceliumSessionManager(courier_session, host="127.0.0.1", port=NODES["hub"][1],
                                             token=tokens["courier"], writer="courier")
    courier = Agent(agent_id="courier", callback_handler=None, tools=[fleet_tool(callers["courier"])],
                    session_manager=courier_manager,
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
    swarm_manager.close()
    courier_manager.close()
    check(True, "hub stopped")
    on_depot = MyceliumAgent("127.0.0.1", NODES["depot"][1], token=tokens["courier"])
    until(lambda: json.loads(on_depot.get(f"strands/{courier_session}/agent/courier/count") or b'{"count":0}')["count"] >= len(courier.messages),
          what="the courier's session on the depot")
    callers["courier"].repoint(depot)
    resumed_manager = MyceliumSessionManager(courier_session, host="127.0.0.1", port=NODES["depot"][1],
                                             token=tokens["courier"], writer="courier")
    resumed = Agent(agent_id="courier", callback_handler=None, tools=[fleet_tool(callers["courier"])],
                    session_manager=resumed_manager,
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
    resumed_manager.close()
    on_depot.close()
    sent = [r for r in evidence(work / "depot" / "evidence.log")
            if r.get("subject") == "token:fleet/courier" and r.get("kind") == "decided"
            and r.get("resource", "").startswith("skill:depot/dispatch@") and r.get("decision") == "permit"]
    check(len(sent) == 1 and "mandate scope=dispatch holder=token:fleet/courier term=t1 epoch=1" in sent[0]["checked"],
          "the depot's own journal has its decision on the courier's second dispatch, naming the appointment")

    print("\nSTRANDS FLEET OK", flush=True)


if __name__ == "__main__":
    main()
