# Mycelium FAQ — start here

**Learning path:** [six developer tutorials](tutorials/README.md) connect stem declarations,
acceptance, authority and runtime evidence with runnable examples.

The rest of this guide is deep reference. This page is the **first read**: short
answers that point you at the one doc or example you actually need. If a question
here doesn't route you well, that's a docs bug — open an issue.

---

## Is Mycelium for me?

**A fleet that grows the capabilities it lacks, under rules it can show it kept.**

Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays. (That is the project's
description — [`docs/positioning.md`](../positioning.md) — and the README opens with it.)

Mycelium earns its keep when you have **many agents (or nodes) that must
coordinate, and you don't want a coordinator** — no broker, no scheduler, no
control plane to run or scale or fail. State converges by gossip; work is claimed,
not dispatched; roles are discovered, not assigned. It embeds as a Rust library
inside your process — there is no daemon.

It is **probably overkill** if you just want to chain a few LLM calls, run one
orchestrator that fans out to workers, or coordinate through a database or queue
you already operate. Reach for a workflow engine (LangGraph, Temporal) or a broker
(NATS, Kafka) there — see *[Why not X?](#why-not-langgraph--temporal--nats--)* below.

The sweet spot: coordination **at scale, without a single point of control** —
fleets that partition and heal, edge/on-prem meshes, systems where "who is in
charge" must be emergent and recallable. The rationale is in
[`philosophy.md`](../philosophy.md) and [`ROADMAP.md`](../../ROADMAP.md)
("The Structural Inversion").

---

## Which primitive or companion do I want?

`mycelium-core` provides Layers I and II: gossip KV and the signal/boundary mesh.
`mycelium` adds Layer III consensus, capabilities, services and optional gateway/security features.

Use the [capability map](../capabilities.md) to choose by the task: discover and signal,
coordinate work, acquire capabilities, constrain actions, or inspect evidence. Each entry connects
an explanation to an example, operating guidance and the limits of the evidence.

Choose your [integration mode](installation.md) before copying an example. For your own companion,
see the [coordination approaches](../design/coordination-approaches.md) and the
[public API contract](building-on-mycelium.md).

---

## Where do I start — hello world?

1. **`cargo run --example hello_mesh`** — two agents share a value by gossip, no setup, no LLM.
   Read the ~25 lines ([`examples/hello_mesh.rs`](../../examples/hello_mesh.rs)): that's the whole
   Layer-I substrate everything else builds on.
2. Read guide [00-concepts.md](00-concepts.md) for the three-layer model and the
   sub-handle API (`kv()`, `mesh()`, `capabilities()`, `consensus()`).
3. Skim a runnable example close to your problem from the capability map.

For an interactive two-node REPL (`set`/`get` keys by hand) see the README
[Run](../../README.md#run) section.

**Building a use case *on top* of Mycelium** (a coordinator, an agent fleet, your
own companion crate)? Go to **[Building on Mycelium](building-on-mycelium.md)** — the
integrator contract: the dependency, the public-API-only rule, which KV prefixes to
avoid, the invariants you must respect, and a copyable `CLAUDE.md` snippet for your
own agent-driven project.

---

## Which example maps to my problem?

| I want to see… | Example |
|---|---|
| A real gossip mesh doing visible work | [`conway.rs`](../../examples/conway.rs) (Game of Life on a 16×16 mesh) |
| An LLM agent using tools + skills over the mesh | [`llm_agent.rs`](../../examples/llm_agent.rs), [`three_node_demo.rs`](../../examples/three_node_demo.rs) |
| Invoking a named skill hosted on another node | [`invoke_skill.rs`](../../examples/invoke_skill.rs) |
| Agents self-organising by meaning | [`semantic_coordination.rs`](../../examples/semantic_coordination.rs) |
| **Proof** the coordination is really coordinator-free | [`coordinator_comparison.rs`](../../examples/coordinator_comparison.rs), [`three_arm_workdist.rs`](../../examples/three_arm_workdist.rs) |
| A full companion worked example | [`redistribution.rs`](../../mycelium-tuple-space/examples/redistribution.rs) (tuple-space), [`microgrid.rs`](../../mycelium-blackboard/examples/microgrid.rs) (blackboard), [`wiki_chat.rs`](../../mycelium-wiki/examples/wiki_chat.rs) (wiki) |

---

## Why not LangGraph / Temporal / NATS / …?

Short version — Mycelium is a **substrate**, not a framework, and the distinction
is the coordinator:

- **LangGraph / AutoGen** — great for a *defined* graph of steps driven by one
  process. Mycelium has no central graph: topology is emergent and survives the
  loss of any node, including "the orchestrator." *You don't have to choose: you can
  run a LangGraph graph **on** Mycelium — the [langgraph ladder](../../examples/langgraph/)
  does, and its **deploy/reheal** rung shows a graph's reasoning surviving the death of
  the mesh node it ran against, resuming on a peer — the claim above, demonstrated.*
- **Temporal / durable workflow engines** — give you a durable *scheduler you
  operate*. Mycelium removes the scheduler; there is nothing central to run.
- **NATS / Kafka / a broker** — a broker *is* the coordinator: a separate process
  you provision, scale, and monitor, with availability depending on its deployment and replication design. Mycelium's mesh is
  the bus, registry, and scheduler at once, with no broker process to run.
- **Erlang/OTP, Akka** — closest in spirit (supervision, location transparency),
  but still a managed cluster with explicit process addressing. Mycelium adds
  receiver-side signal boundaries, gossip-convergent state, and roles discovered
  by capability rather than addressed by PID.

If your problem *has* a natural coordinator and you're happy running it, one of
the above is likely the simpler choice. Mycelium is for when you specifically
don't want that dependency.

---

## Common gotchas

- **Call `shutdown()`.** Companions with background curators/loops (e.g. the wiki)
  hold task cycles; drop alone won't stop them. See
  [14-patterns-and-pitfalls.md](14-patterns-and-pitfalls.md).
- **The gateway has no auth by default — on loopback.** With no credential the HTTP gateway
  serves only a loopback `http_addr`; a non-loopback one (`0.0.0.0`, a LAN address) refuses to
  start unless you set a credential or the explicit `gateway_allow_unauthenticated` opt-in
  (unreleased). Put mTLS / a proxy in front on untrusted networks — see guide
  [09-security.md](09-security.md) and [operations](../operations/README.md).
- **Rolling upgrades are wire-version gated.** One version step per rollout
  (`WIRE_VERSION`/`PREV_WIRE_VERSION`); mixed clusters spanning two steps won't
  talk. See guide [13-cluster-topology.md](13-cluster-topology.md).
- **Opacity/load is emergent, not commanded.** You don't "mark a node down"; nodes
  advertise load and peers route around it. See [00-concepts.md](00-concepts.md).
- **Consistency is opt-in.** `kv()` is eventually consistent; reach for
  `consensus()` only where you need quorum agreement and can satisfy its documented trust and durability assumptions (guide
  [04-consensus.md](04-consensus.md)).

More failure modes: [14-patterns-and-pitfalls.md](14-patterns-and-pitfalls.md) and
[error-handling.md](error-handling.md).

---

## Is consensus safe across membership changes?

**Not across a change of the electorate, and that is a design choice, not a gap.** Discovery —
who is in the mesh, which groups a node's capabilities put it in — is dynamic. The electorate that
decides a lock, a leader or any exclusive outcome is a **fixed set for the life of that decision**;
quorum intersection is a property of that set, so the supported profile fixes it
([threat model §7](../threat-model.md#7-safety-sensitive-agreement-the-supported-profile)) and you
fence the effect at the resource. Today the code refuses an empty or below-floor roster, refuses a
proposer outside the group, and lets the gateway move a *governed* group's membership only through
an audited governance route; nothing yet *requires* a governed group for a safety-sensitive proposal
(planned, not built), and versioned electorates with joint-consensus transitions are a later plan.
The decision and its reasons: [design/consensus-electorate.md](../design/consensus-electorate.md);
the how-to: [04-consensus.md § Discovery is not an electorate](04-consensus.md#discovery-is-not-an-electorate).

---

## How do I build, test, and run?

- **Build:** `cargo build --release` (a `--no-default-features` build drops the
  gateway; see [Cargo features](../../README.md)).
- **Pre-push gate:** `make check` (clippy across the CI feature matrix, ~3 min);
  `make check-full` adds the test suites.
- **Run a mesh:** README [Run](../../README.md#run) section.
- **Task recipes** ("how do I do X?"): [cookbook.md](cookbook.md).

---

*Deeper than this page:* the numbered guide chapters ([README](README.md)) for
reference, [`docs/operations/`](../operations/README.md) for running it in
production, and [`docs/wiki/`](../wiki/wiki.md) for the maintainer-facing synthesis.
