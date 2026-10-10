# Mycelium

**A fleet that grows the capabilities it lacks, under rules it can show it kept.**

Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays.

For teams running fleets of AI agents: generic nodes find and install the capabilities they need, coordinate with no central controller, and — where you configure it — check authority before work runs and keep a record of what they decided.

## What do you want to do?

<!-- router:start -->
- **Build a fleet** → the [developer guide](docs/guide/README.md) and its [tutorials](docs/guide/tutorials/README.md), starting with the [five steps below](#build-a-fleet--five-steps).
- **Run a fleet** → the [operator journey](docs/operations/README.md#operator-journey): deploy, then the [production-readiness checklist](docs/operations/production-readiness.md), then [observability](docs/operations/observability.md).
- **See it work** → [four examples](examples/README.md#what-do-you-want-to-see): the mesh ([`conway`](docs/guide/01-gossip-kv.md#the-example)) · self-provisioning ([`provisioning_viz`](examples/coop/README.md#browser-showcases)) · governed action ([`procurement_authority`](examples/coop/README.md#13--procurement_authority--the-governed-autonomy-flagship)) · operational insight ([`diagnostics`](examples/coop/README.md#12--diagnostics)).
- **Check the evidence** → [`what-is-proven.md`](docs/operations/what-is-proven.md): what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- router:end -->

## What it can do

<!-- compass:start -->
| Area | In one line | Learn it |
|---|---|---|
| [Coordination](docs/capabilities.md) | Nodes share state by gossip, find each other by capability, signal, and agree by consensus where you ask for it. | [guide 01](docs/guide/01-gossip-kv.md) · [02](docs/guide/02-capabilities.md) · [04](docs/guide/04-consensus.md) |
| [Tools and reasoning](docs/capabilities.md) | LLM skills and MCP tools are found and called by name across the mesh, outside agents reach them over A2A, and a companion routes inference across the fleet. | [guide 05](docs/guide/05-skills.md) · [06](docs/guide/06-tool-discovery.md) · [15](docs/guide/15-reasoning-and-langgraph.md) |
| [Adaptive capabilities](docs/capabilities.md) | Stem hosts install signed components from a catalogue when demand goes unmet and restore the provider floors they declared. | [first stem fleet](docs/guide/tutorials/01-first-stem-fleet.md) |
| [Authority](docs/capabilities.md) | Where an enforcement point is configured, mandates and an evaluator decide before work runs; with an evidence journal attached, the decision is recorded. | [guide 20](docs/guide/20-authorising-actions.md) · [21](docs/guide/21-mandates.md) |
| [Federation](docs/capabilities.md) | Separately admitted meshes exchange explicitly exported services without merging, and trust does not pass through a partner. | [guide 17](docs/guide/17-federation.md) |
| [Replay and knowledge](docs/capabilities.md) | Receipts name how far a write got, a recorded run replays against its captured inputs, and knowledge keeps its provenance and disagreements. | [guide 18](docs/guide/18-contracts-and-receipts.md) · [19](docs/guide/19-replay-and-simulation.md) · [23](docs/guide/23-knowledge.md) |
<!-- compass:end -->

Every row has a runnable starting point and its limits in the [capability map](docs/capabilities.md).

## Build a fleet — five steps

<!-- path:start -->
1. **[`hello_mesh`](examples/hello_mesh.rs)** — two embedded agents share state by gossip: 30 seconds, no setup.
2. **[`hello_capability`](examples/hello_capability.rs)** — one node says what it does, another finds it by name and calls it: no registry, no addresses.
3. **[Your first stem fleet](docs/guide/tutorials/01-first-stem-fleet.md)** — generic hosts discover and install a signed echo component, answer a call, and restore a declared provider floor after graceful removal.
4. **[guide 20](docs/guide/20-authorising-actions.md) with [`authority_drain`](examples/authority_drain.rs)** — what an agent may do, checked where the work happens, and stopped when its authority lapses.
5. **[`what-is-proven.md`](docs/operations/what-is-proven.md)** — what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- path:end -->

**Learn the newer surfaces:** [six developer tutorials](docs/guide/tutorials/README.md) — stem fleets,
declarations, shadow acceptance, authority boundaries, replay, and declared versus observed.

## Is it for you?

Mycelium is built in three layers — a gossip KV store, a signal mesh, and epidemic
consensus — with capability discovery across them: no broker, no registry, no daemon, no control
plane. State converges by gossip; work is claimed, not dispatched; roles are discovered, not
assigned. It is **probably overkill** if you want to chain a few LLM calls, run one orchestrator
that fans out to workers, or coordinate through a database or queue you already operate; reach for
a workflow engine or a broker there ([why not X?](docs/guide/faq.md#why-not-langgraph--temporal--nats--)).
It earns its keep when many agents must coordinate and nobody should be in charge: fleets that
partition and heal, edge and on-prem meshes, systems where *who is in charge* must be emergent and
recallable. The sentence at the top is the project's one description ([`docs/positioning.md`](docs/positioning.md));
what it can prove, and what it cannot yet, is one page: [`what-is-proven.md`](docs/operations/what-is-proven.md).

## Hello, mesh — 30 seconds, no setup

```sh
cargo run --example hello_mesh
```

Two embedded agents on loopback: one writes a value, the other learns it **by gossip** — no
broker, no config, no LLM, no features to enable. That's Layer I, the shared KV store everything
else builds on. It's ~25 readable lines — start there: [`examples/hello_mesh.rs`](examples/hello_mesh.rs).

Then `cargo run --example hello_capability` ([source](examples/hello_capability.rs)) shows the value
proposition itself — one node advertises what it *does*, another finds it *by name* and calls it over
RPC, **no registry and no configured addresses**.

## Where next?

The router above covers building, running, watching and checking. For the rest:
the [audience routes](docs/README.md) and the [capability map](docs/capabilities.md).
Prospects: [buyer deck](docs/publications/customer-pitch.html) → [pilot](docs/operations/customer-pilot.md).
Researchers: [research guide](docs/publications/research-guide.md).

| You are… | Go to |
|---|---|
| **New here** — is this for me? which primitive? why-not-X? | the **[FAQ](docs/guide/faq.md)** — your map, and the intended first read |
| **Building a use case *on* Mycelium** | [Building on Mycelium](docs/guide/building-on-mycelium.md) — the integrator contract (dependency, public-API rule, reserved KV prefixes, a copyable `CLAUDE.md`) |
| **Choosing among every example** | the **[capability matrix](examples/README.md#the-capability-matrix)** — every runnable example fingerprinted by layer + facet (level · surface · LLM · audit · metrics), each linking to its run-doc |

> The rest of this page is a **short orientation** — what the system is and a
> layers-at-a-glance table. Every deep dive lives one link away in the
> [guide](docs/guide/README.md) and [operations](docs/operations/README.md) docs.

## What it is

Mycelium is three layers: a broker-less gossip KV store (Layer I), an ephemeral
scoped event mesh (Layer II), and an opt-in consensus overlay (Layer III). The
capability system sits across all three layers and provides broker-less service
discovery. Four application patterns build on this substrate: Skills (LLM agents
as mesh nodes), MCP tool discovery (LLM finds tools dynamically from the KV
store), fluid pipelines (Agentic Flow Networks), and A2A interop (LangChain /
AutoGen). Built on TCP epidemic propagation with last-write-wins conflict
resolution; each agent chooses its own payload serialisation.

### Which crate? — `mycelium` vs `mycelium-core`

The workspace offers two substrate crates, obtained from a pinned git tag. See [installation and integration modes](docs/guide/installation.md). **Most users want `mycelium`** —
the full runtime. Reach for `mycelium-core` only for a minimal embed.

| | `mycelium` | `mycelium-core` |
|---|---|---|
| **Layers** | I + II + III (gossip KV, signal mesh, consensus, capabilities, services, gateway, TLS) | I + II only (gossip KV + signal/boundary mesh) |
| **Dependency tree** | ~140 crates (pulls in Axum/hyper for the HTTP gateway) | ~50 crates — no axum/hyper/gateway |
| **Use it when** | You need RPC, consensus, the capability system, the HTTP/MCP/A2A gateway, or RBAC/audit | You only need last-write-wins KV propagation + the scoped event mesh, on bare-metal / size-constrained / no-gateway targets |

```toml
# Full runtime (default):
mycelium = { git = "https://github.com/RichardTathata/mycelium", tag = "<release tag>", features = ["tls"] }   # current tag: docs/guide/building-on-mycelium.md §1

# Minimal substrate embed (Layers I + II, no gateway):
mycelium-core = { git = "https://github.com/RichardTathata/mycelium", tag = "<release tag>" }
```

`mycelium` re-exports everything in `mycelium-core`, and a `mycelium-core` node still *forwards*
all traffic (including consensus frames) — it just never acts on the higher layers. You can also
trim `mycelium` itself toward the core with `default-features = false` (drops the gateway) and
`--no-default-features --features gateway` (drops consensus). The split landed in v2.0 M1 — see
[ROADMAP §v2.0 Milestones](ROADMAP.md) for the rationale. Two coordination companion crates are
built entirely on the public API: [`mycelium-tuple-space`](mycelium-tuple-space/) (a pull-based
pipeline buffer — work routed by lane *position*) and
[`mycelium-blackboard`](mycelium-blackboard/) (shared working memory — facts claimed by *content*
predicate, competitive and exactly-once). Known stages → tuple space; emergent topology over shared
facts → blackboard.

---

## Skills vs MCP tools — in one line

> **MCP tool** = a *function* in the mesh (any language; the LLM calls it).
> **Skill** = an *LLM agent* in the mesh (TOML manifest, no code; callable by any node — including other skills).

They compose naturally; the full comparison and when-to-use guide is in
[guide ch. 00 — Concepts](docs/guide/00-concepts.md#reference--skills-vs-mcp-tools-choosing-the-right-primitive).

## Build

```
cargo build --release
```

The fuzz harness (`fuzz/` — wire + capability decoders) needs nightly:
`cargo +nightly fuzz run wire_decode -- -max_total_time=60`.

## Run

Two nodes on one machine, no config:

```
cargo run -- --port 7946                          # bootstrap node
cargo run -- --port 7947 --peers 127.0.0.1:7946   # joins via the first
```

Type `set k v` / `get k` in either terminal and watch the other converge. The richer
interactive cluster (HTTP dashboards, MCP tools, chat) is
[`three_node_demo`](examples/chat/README.md).

**No local setup** — Docker one-liners: `make test-llm-agent` (11 scenarios, no Ollama) ·
`make test-llm-demo` (interactive chat, needs Ollama) · `make test-overlay` (3-node consensus).
The full runnable set — objective, setup, walkthrough each — is the [examples](examples/README.md).

## The system, layer by layer

Each row is a one-line orientation; the linked chapter carries the concept *and* the full
reference (API surface, observability, design notes) — one home per fact.

| Layer / subsystem | What it gives you | Depth |
|---|---|---|
| **Layer I — gossip KV** | Broker-less shared state: LWW + HLC, Merkle anti-entropy, WAL persistence | [ch. 01](docs/guide/01-gossip-kv.md) |
| **Layer II — signal mesh** | Ephemeral scoped events: unconditional forwarding, boundary-gated *action*, opacity & inhibition | [ch. 03](docs/guide/03-signals.md) |
| **Layer III — consensus** | Opt-in agreement: ballots/quorum, `consistent_set`/`consistent_get`, locks, leader election, durable log + consumer groups | [ch. 04](docs/guide/04-consensus.md) |
| **Capabilities** | Discovery by *what a node does*: advertise/resolve, schema registry, requirements & demand pressure, emergent groups, locality | [ch. 02](docs/guide/02-capabilities.md) |
| **Service layer** | RPC, bulk transfer, scatter-gather, actor mailboxes — on the mesh, no broker | [cookbook](docs/guide/cookbook.md#reference--the-service-layer-rpc-bulk-scatter-gather-mailbox) |
| **Skills & prompt skills** | LLM agents as mesh nodes (TOML manifests, composition) + LLM-backed capabilities in KV | [ch. 05](docs/guide/05-skills.md) |
| **Contracts & receipts** | What an acknowledgement *proves*, by rung: applied here · on this node's disk · persisted by named peers · committed at a destination. A timeout is `DeliveryUnknown`, never a failure; a required-sync write **refuses** rather than applying an undurable one | [ch. 18](docs/guide/18-contracts-and-receipts.md) |
| **Deterministic replay** | A recorded run as a file you can re-run — because a seed is only reproducible while the code is unchanged, which is when nobody needs it. A replay tells you *where* a changed build first departed | [ch. 19](docs/guide/19-replay-and-simulation.md) · [`mycelium-sim`](mycelium-sim/) |
| **Authorisation at gateway and provider** | A preflight before dispatch with your own policy engine: permit / deny / **indeterminate**, an evidence journal, and coverage stated as a field rather than implied by silence | [ch. 20](docs/guide/20-authorising-actions.md) |
| **Scoped mandates** | Is the caller still the one who was appointed? An installed epoch is terminal for work authorised under an earlier one, checked inside the resource's own atomic boundary | [ch. 21](docs/guide/21-mandates.md) |
| **Adaptive stability** | Deciding when *not* to act on an uncertain view, and budgets backed by durably accounted rights rather than soft state that evaporates | [ch. 22](docs/guide/22-stability-and-control.md) |
| **Knowledge** | Keeping disagreement instead of resolving it: claims, observations and *judgements with authors* — where a self-assessment does not count as evidence | [ch. 23](docs/guide/23-knowledge.md) |
| **Federated domains** | Two independently admitted meshes exchanging *explicitly exported services* — and never joining their transports | [ch. 17](docs/guide/17-federation.md) · [runbook](docs/operations/federation.md) |
| **Companions** | `mycelium-tuple-space` (pull pipeline by lane *position*) · `mycelium-blackboard` (claims by *content* predicate) · `mycelium-commitment` (the contract net — negotiate, then award) · `mycelium-reason` (fleet inference routing with local reservations + failover, an **OpenAI-compatible endpoint on every node**, fleet-reasoning traces, model-following resume) · `mycelium-effects`, `mycelium-sim`, `mycelium-wiki` | crate docs ([tuple-space](mycelium-tuple-space/) · [blackboard](mycelium-blackboard/) · [commitment](mycelium-commitment/) · [reason](mycelium-reason/)) · [ch. 15](docs/guide/15-reasoning-and-langgraph.md) · [ch. 24](docs/guide/24-commitments.md) |

## Security

mTLS peer admission (the real data-isolation boundary), Ed25519-signed consensus, RBAC +
tamper-evident audit (`compliance` feature), hot identity rotation. Posture + threat framing:
[guide ch. 09](docs/guide/09-security.md) · [threat model](docs/threat-model.md) · operator
runbooks under [`docs/operations/`](docs/operations/README.md) (rbac, sso, audit,
cert-rotation, crown-jewel). **The default configuration is a development posture, not this one:**
a `GossipConfig::default()` node gossips in plaintext and trusts every peer, opens its gateway to
anyone if `http_port` is set, allows all egress and runs under no profile; `profile =
"secure-single-domain"` makes `start()` refuse unless the enforced posture holds on the node
([production readiness](docs/operations/production-readiness.md)).

## Operating it

Prometheus `/metrics`, dashboards, readiness, diagnostics, tuning (including the performance
baselines and the `GossipConfig` reference): start at the
[operations index](docs/operations/README.md) — notably
[observability](docs/operations/observability.md), the
[metrics reference](docs/operations/metrics.md), and [tuning](docs/operations/tuning.md).

## Language bridges

Python (`mycelium-py`) and TypeScript (`mycelium-ts`) clients drive a node through the embedded
gateway — capabilities, signals, RPC, KV, mailboxes, tuple space. [Guide ch. 10](docs/guide/10-language-bridges.md)
has the API tour; each package README has the quick start.

## How the project keeps itself honest

Mycelium runs an adversarial **self-audit system**: four LLM-assisted audits (code · internal docs ·
documentation coverage · external claims) plus deterministic CI gates — and each audit keeps a dated
**ledger of its own misses**, a verdict it once declared clean that later proved wrong. The
mechanisms cross-correct. For reviewers doing technical diligence,
[`docs/analysis/README.md`](docs/analysis/README.md) explains the system and links the real, in-repo
ledgers (candid by design — a self-audit is only worth the failures it records).

## Research & citation

[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.20665238.svg)](https://doi.org/10.5281/zenodo.20665238)

Mycelium is the working implementation behind a published architectural argument. The lead paper:
R. Nicholson, *"The Coordinator Trap: Structural Scaling Liabilities in Mediated Multi-Agent
Architectures and a Substrate-Based Alternative,"* Tathata Systems Ltd, 2026 —
[doi:10.5281/zenodo.20665238](https://doi.org/10.5281/zenodo.20665238) (CC BY 4.0; source in
[`docs/publications/`](docs/publications/), reproducible at tag
[`paper-submission-v2`](https://github.com/RichardTathata/mycelium/tree/paper-submission-v2)).

It is the lead of a **four-part corpus** (all CC BY 4.0; full read-order, dependency graph, and
DOIs in [`docs/publications/README.md`](docs/publications/README.md)):

- **Heterogeneous Local Knowledge Systems (HLKS)** — the cross-domain convergence argument
  (`mycelium-tuple-space` is its constructive evidence) — [doi:10.5281/zenodo.20813058](https://doi.org/10.5281/zenodo.20813058)
- **The Capture Problem** — power vs knowledge; closes the sequence — [doi:10.5281/zenodo.20813463](https://doi.org/10.5281/zenodo.20813463)
- **Monetary Ecology** — the MCB/P/S/Î evaluation framework the distributive argument draws on — [doi:10.5281/zenodo.20811062](https://doi.org/10.5281/zenodo.20811062)

## License

Mycelium is released under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only).

**Open use:** Any project distributed under a compatible open-source license may use Mycelium freely under the AGPL terms. Network-deployed applications using Mycelium must make their source available to users of that service.

**Commercial embedding:** Organisations that need to embed Mycelium in a proprietary product without the AGPL copyleft obligation can obtain a commercial license. Contact [tathatasystems@proton.me](mailto:tathatasystems@proton.me) to discuss terms.
