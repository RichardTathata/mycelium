# A Strands fleet on Mycelium

Strands agents on three Mycelium nodes plan tonight's food redistribution. They keep their session and
swarm memory in the fleet, and call fleet skills through `/a2a`. A call outside an agent's mandate is
refused and recorded. One node is stopped, and the work carries on from another node out of the
stored session. Post-360 plan row P; it rests on row N (A2A at the current specification) and row O
(the session store).

```sh
cargo build --example strands_fleet_node --features tls,a2a,compliance
pip install ./mycelium-py ./strands-session-mycelium cryptography httpx
python examples/strands_fleet/run_demo.py        # ends with STRANDS FLEET OK
```

CI runs exactly this in the `strands-fleet` job.

## The run

The driver starts the co-op's **mandate authority** and three **nodes**, all from
[`strands_fleet_node`](../strands_fleet_node.rs). Each node is a gateway with `/a2a`, the action
evaluator, the mandate verifier, an evidence journal and a consensus voter, and each serves one skill:

| Node | Skill | Answers |
|---|---|---|
| hub | `bakery/surplus` | what the bakeries have left over tonight |
| kitchen | `kitchen/needs` | who needs meals tonight |
| depot | `depot/dispatch` | dispatches a van |

There are three agents, each a principal its bearer names (`token:fleet/collector`, and so on):

1. **A Strands `Swarm` plans.** The collector asks the bakery and the kitchen through the hub's
   `/a2a`, then hands off to the planner. One call reaches a skill on the hub itself and one on the
   kitchen node, through the same gateway. The swarm's state goes to the fleet's KV through
   [`strands-session-mycelium`](../../strands-session-mycelium/README.md).
2. **The planner tries to dispatch a van, and is refused.** Dispatching is a mandated role: the
   co-op's policy allows `depot/dispatch` only with an established mandate in scope `dispatch`, and
   the authority appointed the courier, not the planner. The gateway answers `-32031 authority not
   established`. The driver then reads the hub's **evidence journal**, which is node-local and never
   gossiped, and finds the refusal: `indeterminate`, with nothing executed. The planner then
   presents the courier's own grant, signed with its own key, and is refused again. A grant is an
   appointment, not a bearer token: it counts only with the holder's proof for that very call.
3. **The courier dispatches van 1 through the hub.** It presents its grant with a possession proof
   bound to this call, signed with a key only the courier holds. The evidence record names the
   appointment it relied on: `mandate scope=dispatch holder=token:fleet/courier term=t1 epoch=1`.
4. **The hub is stopped.** The courier is resumed through the **depot** from its stored session, with
   the conversation it had, van 1 included, and dispatches van 2. The swarm's plan is readable on the
   depot too, and the depot's own journal records its decision on the second dispatch.

## What Strands provides, and what Mycelium adds

| Strands Agents | Mycelium |
|---|---|
| The agent loop, tools, the `Swarm` and its handoffs | Where the agents' calls go: skills discovered across the fleet by name, reached through any node's gateway |
| The `SessionManager` interface | The session in the fleet's KV: one node writes it, another resumes from it, and no write goes over what a node has not seen (row O's claims) |
| An A2A client, through `a2a-sdk` | `/a2a` serving the A2A 1.0 methods (row N) behind the action evaluator |
| — | Who may call what: the policy, mandates the gateway establishes from a signed grant and a per-call possession proof, and the evidence journal recording each decision |

## What the run shows, and what it does not

**Shows:**
- Strands agents calling skills served on other nodes through one gateway's `/a2a`.
- A Swarm's state and an agent's conversation kept in the fleet and read from a different node after
  the first one is stopped.
- A call outside the caller's mandate refused, and that refusal read back from the deciding node's
  own evidence journal.
- A borrowed grant refused.
- A permitted call's record naming its appointment.
- The session store's claims keeping a quorum with one of three voters gone.

**Does not show:**
- **A model's judgement.** Every agent runs a scripted model (`fleet.ScriptedModel`), so the plan is
  the script's. The run is about where calls go and who may make them, which a model cannot change.
  A real model plugs into the same agents.
- **Authentication between processes beyond the loopback.** All four processes run on one host. The
  mesh runs TLS, with the fleet CA's key on every node (the development profile).
- **A node failing mid-call.** The hub is stopped between two turns, not in the middle of one.
- **Revocation.** The authority's checkpoints revoke nothing here; `authority_drain` and the
  confined-fleet suite show a revocation stopping work.
- **Strands' own A2A server or client classes.** The agents call `/a2a` with the A2A 1.0 JSON-RPC
  request a standard client sends (`fleet.FleetCaller`). The `a2a-sdk` CI job drives the SDK itself
  against a node.
- **A real deployment's scale or timing.** Three local processes, one machine.
