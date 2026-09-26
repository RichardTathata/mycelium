# Food-Rescue Co-op — example suite

## Objective

A cohesive set of runnable demos for Mycelium's newer capabilities — the **mailbox**, **governance
(management-as-intent)**, **autonomic provisioning**, **federation (AgentFacts)**, and the
**tuple-space** — composed in *one constructive world* rather than as isolated API toys.

> **The world.** A regional **food-rescue logistics co-op**: a network of **depot** nodes rescues
> surplus food (from markets, farms, bakeries) and routes it to community kitchens before it
> spoils. There is **no central dispatcher** — depots advertise capabilities, claim work when
> ready, and self-organise. A neighbouring co-op is a separate *domain* the federation demo talks to.

Full design + roadmap history: [`docs/plans/example-suite.md`](../../docs/plans/example-suite.md).

## Reading the co-op as your business

The co-op is kept because it is understandable, constructive, and forces real distributed decisions.
Read it with this translation and every demo is your estate:

| In the co-op | In an enterprise estate |
|---|---|
| a depot | an operating site, a service instance, a worker pool |
| a donation | a work item, a request, a job |
| a kitchen | a downstream customer or consuming system |
| a purchasing ceiling | delegated financial or operational authority, with a limit |
| a steward | an authorised human intervener |
| a partner co-op | a supplier or partner domain with its own trust root |
| the morning rush | a demand change |
| a depot going quiet | a node, zone or dependency failure |

Simulated effects are labelled as such in each demo's output; a real effect (a real model, a real
tool) is marked **manual** in the table below.

## How to run

**Fifteen demos are shipped: thirteen run Docker-free in CI, two are manual** (they need real
model weights). An earlier version of this line said twelve while `ci_smoke.sh` ran thirteen —
the count is now checked against the script, not remembered. Everything shares the [repo setup](../README.md#shared-setup); then:

```bash
./ci_smoke.sh          # the twelve CI demos, in order, with assertions (also the CI job)
# or any single demo:
cargo run -p mycelium-coop-examples --bin stigmergy
```

The two manual demos — [`model_deploy`](#m--model_deploy-manual--a-real-llm-through-the-library)
and [`reheal_deploy`](#m--reheal_deploy-manual--the-real-model-reheal-flagship) — are documented
below with their own run steps.

## Shared harness (`src/common/`)

Every demo builds on this:

| Module | What it gives you |
|---|---|
| `domain` | the constructive vocabulary (`Donation`, zones) |
| `bootstrap` | `spawn_depot(...)` — a depot with gateway + tls identity, consistent across the suite |
| `facts_lens` | mounts the WS-F **AgentFacts edge endpoint on every depot**, so any running example is inspectable live at `/.well-known/agent-facts.json` |

The AgentFacts lens is *infrastructure, not a separate example*: while any demo runs, you can
inspect the federation view of a depot at the gateway port it prints on startup —

```bash
curl http://127.0.0.1:<printed-port>/.well-known/agent-facts.json
```

## Examples

| # | Bin | Status | Demonstrates |
|---|-----|--------|--------------|
| 01 | `mailbox_llm` | ✅ shipped | actor ↔ LLM via the durable, HLC-ordered **mailbox** |
| 02 | `stigmergy` | ✅ shipped | coordinator-free load shedding via `sys/load` pheromone |
| 03 | `elastic_intent` | ✅ shipped | elastic sizing as evaporating **intent** (operator-optional) |
| 04 | `provisioning` ⭐ | ✅ shipped | the full **autonomic loop**: buffer in a tuple-space lane while peers self-provision the missing capability |
| 05 | `federation_facts` | ✅ shipped | cross-domain edge discovery via self-certified AgentFacts |
| 06 | `rotation` | ✅ shipped | zero-disruption identity rotation; pre-rotation facts still verify |
| 07 | `consensus` | ✅ shipped | multi-bloc agreement via cross-group consensus + leased (decaying) decisions |
| 08 | `llm_pipeline` | ✅ shipped | LLM agents coordinating a multi-stage pipeline purely via a tuple space |
| 09 | `mcp_toolgrowth` | ✅ shipped | an LLM agent grows the fabric's toolset at runtime — declares a need, the tool's **code arrives** (catalogue → pull → verify → instantiate), is bridged over MCP, then invoked |
| 10 | `llm_council` | ✅ shipped | a council of **differentiated** LLM agents deliberates a shared task — fan-out → synthesis → iterative refinement, all via the tuple space |
| 11 | `catalog` | ✅ shipped | the **cluster-wide artifact catalogue** — register a deployable, discover it via gossip, pull bytes over the mesh, provision & invoke (no registry server) |
| 12 | `diagnostics` | ✅ shipped | **diagnosing an emergent condition** from a node's own local view — an intent-vs-reality mismatch induced on one depot, named by a *different* depot with no collector |
| 13 | `procurement_authority` ⭐ | ✅ shipped | **what a gateway can promise about an agent's actions, and what it cannot** — six acts: permitted, denied, *authority not established*, an authorised intervention, a bypass the evidence names, a correction that cites what it replaces |
| M | `model_deploy` | ✅ shipped (manual) | **a real LLM model deployed through the artifact library** — weights (GGUF) **and** their deployment **profile** as two signed artifacts, profile → weights by content address → library → catalogue → resource-checked election → streamed with live percent → resolved + `ollama create` → probe-gated → real tokens under the governed profile. Needs Ollama; not in `ci_smoke` |

## Browser showcases

Four demos have **watchable browser variants** (the `/state`-feed-behind-a-canvas pattern; run
continuously, Ctrl-C to stop; **not** in CI). All follow the
[UI-example contract](../../docs/wiki/dev/ui-example-contract.md) — gateway+metrics on, Ops Console
linked (`ui/viz` + a `⚙ Ops Console` back-link), and a "what you're seeing" concepts box; the two
artifact ones also print the `## Loads` banner.

| Showcase | Port | Run | What you watch |
|---|:--:|---|---|
| `stigmergy_viz` | `:8092` | `… --bin stigmergy_viz --features metrics` | dispatch reroutes around a busy depot (opacity pheromone) |
| `llm_council_viz` | `:8094` | `… --bin llm_council_viz --features metrics` | fan-out · synthesis · critic↔reviser DAG (`EchoBackend`, no key) |
| `provisioning_viz` ⭐ | `:8097` | `… --features wasm,metrics --bin provisioning_viz` | a capability **self-provisions**, then **heals onto a standby** when the active node is killed — no coordinator |
| `catalog_viz` | `:8098` | `… --features wasm,metrics --bin catalog_viz` | the origin (librarian) **dies + its library is deleted**, yet a late node **installs from a verified peer cache** |

(`…` = `cargo run -p mycelium-coop-examples`.) Each is the visual variant of its batch demo
(`stigmergy` / `llm_council` / `provisioning` / `catalog`), which stay the CI-gated versions.

## Patterns & pitfalls

Each demo also teaches a **pitfall** — the right way vs. the anti-pattern, with the *why*. The full
write-up is [guide chapter 14](../../docs/guide/14-patterns-and-pitfalls.md); the one-line index:

| Demo | Pitfall it teaches (don't do the wrong thing) |
|---|---|
| `mailbox_llm` | host an invokable skill on a *separate* node (don't self-RPC); gate readiness on capability **and** peers |
| `stigmergy` | model load as a node's *own* backlog (don't inject load into a remote node) |
| `elastic_intent` | let the governor *own* a group under intent (don't expect `max`/`drain` to hold against emergent auto-join) |
| `provisioning` | the gossip catalogue is the cluster path (don't ship artifacts via node-local `InMemorySource`) |
| `rotation` | use a faster anti-entropy tick to read a freshly-signed fact across startup |
| `consensus` | commitments are promise-strength (an empty bloc can't be coerced) |
| `llm_council` | keep fan-in joins to a single synthesizer today (keyed fan-in is M13) |
| `mcp_toolgrowth` | activation ≠ installation (registering compiled-in code is not code arrival); bridge an MCP tool by *also* advertising a `tool/` capability |
| `catalog` | the library is an origin tier, not a read dependency (don't make installs require the origin alive — a peer's cache verifies identically) |
| all | structural polls, never fixed sleeps; bind N ports at once |

### 01 — `mailbox_llm`

```bash
cargo run -p mycelium-coop-examples --bin mailbox_llm
```

Three depots: `kitchen-router` hosts a `routing/suggest` Prompt Skill (`EchoBackend`, so no model
or API key is needed); `depot-intake` receives donations and **delivers an event to triage's
mailbox** asking where to route each; `depot-triage` is the **actor** — it drains its mailbox in
**HLC-causal order**, consults the router (a genuine cross-node RPC), and delivers the answer back
to intake's reply mailbox.

**Philosophy beat** ("would Holland approve?"): actor-style messaging — addressed, ordered, durable
within the gossip TTL window — emerges from Layer I (KV) + HLC ordering. No broker, no actor
registry, no explicit lifecycle. Addressing is just the target `NodeId` + a `kind` string.

### 02 — `stigmergy`

```bash
cargo run -p mycelium-coop-examples --bin stigmergy
```

Three worker depots advertise `depot/intake` and run an opacity governor over their `work.intake`
queue; a `depot-dispatch` node decides where intake goes by **reading the pheromone trail the medium
carries**. When one depot hits a local backlog, its governor writes an `is_opaque` pheromone to
`sys/load/{depot}/work.intake`; dispatch reads it (`is_node_opaque`) and routes around the busy
depot. Drain the queue and the pheromone evaporates — the depot rejoins the eligible set on its own.

**Philosophy beat:** load shedding with **no coordinator, no message, no failure detector**. The
busy node reports only its own saturation; every other node reads the trail and decides locally.

### 03 — `elastic_intent`

```bash
cargo run -p mycelium-coop-examples --bin elastic_intent
```

An operator declares "keep `rush-pool` in `[2, 3]` depots" by publishing an evaporating
`MembershipIntent` (soft-state, **not** a command). Five candidate depots run a `MembershipGovernor`
and **self-elect** so the pool holds a *subset* in the band — no controller picks who. Then: (2) the
operator goes offline and the band still holds (the intent persists in gossip within its TTL); (3) a
pool member is killed and the governors self-heal the count back to `MIN`.

**Philosophy beat:** *management = intent + local reconcile*. There is no privileged controller —
just an evaporating desired-state and nodes that reconcile locally. The litmus *"if management
vanishes, does the cluster keep working?"* is shown by killing the operator. (Requires the governor
to own a group under intent — see #56 / the emergent-defers-to-governor fix.)

### 04 — `provisioning` ⭐ (the flagship)

```bash
cargo run -p mycelium-coop-examples --features wasm --bin provisioning
```

The whole thesis in one process. A surge of donations needs a `route/optimize` capability **no depot
has yet**, so they **buffer in a tuple-space lane**. The worker declares the requirement (the demand);
a provider depot **self-provisions** the optimizer — a real WASM component pulled, content-verified,
and instantiated — advertises it, and serves it over RPC. The worker then drains the backlog: `take`
→ invoke the optimizer → `complete` to `done`. Finally the **active optimizer is killed**: its
capability evaporates, a second wave buffers, and a **standby self-provisions** to restore it —
*restart ≡ provisioning*. Both waves drain.

The WASM artifact is the committed `echo_component.wasm` fixture (it echoes its input — a
deterministic "optimized route"), so CI needs no wasm toolchain.

**Philosophy beat:** nothing predicted who would run the optimizer. It was **unmet demand** (a
pheromone), satisfied by a node **electing to provision**; the buffer lost no item; and the
rendezvous **self-healed** across a provider death — no coordinator anywhere in the loop.

### 05 — `federation_facts`

```bash
cargo run -p mycelium-coop-examples --bin federation_facts
```

**Two separate domains** (separate clusters, separate auto-CAs — they do *not* peer): our co-op
(`coop-a`, advertising `route/optimize`) and a neighbouring co-op (`coop-b`) with overflow. The
neighbour discovers our capability the way a NANDA-style quilt does — it **pulls our AgentFacts at
the edge** (`/.well-known/agent-facts.json`, served by the facts lens), a self-signed JSON-LD
document, and **verifies the signature itself**. It reads the capability list, decides to route
overflow to us, and — a tampered copy of the document fails verification.

**Philosophy beat:** discovery across a trust boundary with **no shared CA and no issuer authority**.
The facts are self-certified by the node identity; trust is the *fetcher's* decision (Core Principle
1). A Mycelium domain is a sovereign quilt-patch.

### 06 — `rotation`

```bash
cargo run -p mycelium-coop-examples --bin rotation
```

`depot-a` publishes a signed AgentFacts field, then **rotates its Ed25519 identity** mid-operation
(routine hygiene). Its peer `depot-b` keeps verifying across the rotation: (1) the field before the
rotation; (2) the **same old-key-signed field after the rotation** — it still verifies, because A's
`sys/identity/{a}` retains `new ‖ old` and every verify path tries the whole **retained key set**;
(3) a fresh field A signs with the new key.

**Philosophy beat:** key hygiene with **no disruption and no re-signing of history** — a retired key
stays verifiable for what it signed. This is the runnable form of the retained-key-set fix
(PR #51 / `crdt.rs::verify_any`).

### 07 — `consensus`

```bash
cargo run -p mycelium-coop-examples --bin consensus
```

A large donation spans two depot blocs (`north`, `south`) and accepting it commits *both* to
cold-chain capacity — so acceptance requires **each bloc to independently reach quorum**
(`cross_group_propose` over two `GroupQuorum`s). Phase 1 commits (both blocs agree); Phase 2 — adding
a third bloc with no voters — **times out** (no bloc can be coerced); Phase 3 commits a
**short-leased** decision that **decays**, so the slot reads back as reopened.

**Philosophy beat:** Layer III — an emergent coordinator (proposer + quorum) that exists only for the
decision and **dissolves once it commits**, riding ordinary signals on the same substrate. Commitments
are *promise-strength* (a bloc with no voters can't be bound), and decisions evaporate like any other
mandate (epoch-leased commit). "Complex societies do need coordinators; they emerge — they aren't the
starting point."

### 08 — `llm_pipeline`

```bash
cargo run -p mycelium-coop-examples --bin llm_pipeline
```

A two-stage donation pipeline whose workers are **LLM agents**: `classify ──▶ route ──▶ done`, where
each stage is a tuple-space lane. Two LLM workers (`agent-a`, `agent-b`) each loop — **pull** an item
from the deepest pending lane, run **their own model** on it (an `EchoBackend` stand-in invoked
directly, so the worker *is* the agent, not a caller of a central skill), and **complete** it to the
next lane. They compete per-lane; no dispatcher predicts who does what. A finished item carries
nested echoes proving it went through both LLM passes (`route(classify(donation))`).

**Philosophy beat:** multi-agent LLM coordination with **no orchestrator** — the lanes are the only
coordination, readiness is self-announced by the pull, and the model call lives *between* `take` and
`complete`. This is the LLM-over-tuple-space composition (Paper 1 §9.4 territory), built on the public
API only.

### 09 — `mcp_toolgrowth`

```bash
cargo run -p mycelium-coop-examples --features wasm --bin mcp_toolgrowth
```

An LLM agent, mid-task, finds it needs a tool the fabric doesn't yet offer (a kg→tonnes converter).
It **declares the requirement**; a `tool-host` node — running dark — sees the unmet demand and
**installs the tool for real**: the converter's arithmetic lives in a WASM component whose bytes
**arrive over the mesh** (catalogue entry → provenance check → pull from a discovered librarian →
content-address verify → instantiate), and the arrived component is then **bridged** as an MCP tool
(`register_mcp_tool` → `tools/unit-convert/{host}`, the handler a thin shim into the sandboxed
guest) with the matching `tool/` capability advertised so the demand resolves. The agent
**discovers and invokes** it over the MCP path (`rpc_call` with `mcp.invoke`), gets
`{"tonnes": 5.0}` — computed *inside the component that just arrived* — and its model composes the
receipt.

**Activation ≠ installation:** the tool-host also registers a compiled-in `ping` tool at startup,
explicitly labelled as *activation* — turning on code you already shipped. The converter is
*installation*: `grep` the demo for arithmetic; there is none. (The guest source:
`mycelium-wasm-host/tests/fixtures/unit-convert-component/`.)

**Philosophy beat:** the agentic self-extension loop — the fabric's *capability surface grows because
an agent asked for it.* No operator wired the tool in advance, no coordinator decided who hosts it;
it's the same demand→provision pheromone as the WASM flagship (04), and the same library/catalogue
machinery as demo 11 — here surfacing the arrived code as an **MCP tool**.

### 10 — `llm_council`

```bash
cargo run -p mycelium-coop-examples --bin llm_council
```

The capstone of the LLM-coordination examples. A raw donation **evolves** into an approved
distribution plan through a *council of differentiated agents*, each pulling only its own lane — no
orchestrator, the tuple space is the only coordination. It composes three collaboration modes in
sequence:

1. **Fan-out → specialists** — a fan-out agent copies the donation into three lanes; three
   *differentiated* agents (perishability / routing / allergen) each pull their own lane, in
   parallel, and emit a partial.
2. **Fan-in synthesis** — a synthesizer drains `partials`, accumulates them **by donation id**, and
   once it holds all three for an id merges them into a draft plan.
3. **Iterative refinement** — a critic scores the draft; on a fail it sends the item **back to
   `revise`**; a reviser improves it and sends it **back to `draft`** — the item cycles until the
   critic approves (deterministically: quality 0.6 → 0.8 → 1.0, exactly two refinement cycles).

Every role is a real `LlmBackend::complete` call (an `EchoBackend` stand-in, CI needs no key); the
structured decisions are deterministic so the demo asserts each approved plan carries all three
specialists' contributions **and** went through ≥2 revisions.

**Philosophy beat:** a *group* of differentiated LLM agents collaborating on one shared artifact —
fan-out, synthesize, refine — with no orchestrator. The plan matures through three modes; the agents
never address each other, only the lanes.

**Architectural note (the boundary it sits on):** with a *single* synthesizer the fan-in join is done
in the synthesizer's own memory (accumulate-by-id after `take`) — fully expressible today. *Competing*
synthesizers would each grab fragments of one donation's partial set, which needs keyed-exact-match
`take` (ROADMAP **M13**, Paper 1 §9.4). This demo names that line rather than crossing it.

### 11 — `catalog`

```bash
cargo run -p mycelium-coop-examples --features wasm --bin catalog
```

The **cluster-wide artifact catalogue**, end to end — the real path that demo 04's node-local
`InMemorySource` shortcut stands in for, with no build-time embedding and no hardcoded providers.
CI (plain code, no node) reads the component **from disk at runtime**, stores it in a durable
**library** (`FsLibrarySource` directory + Ed25519-**signed manifest** — publisher keys never
touch a node). A `librarian` node takes the role (`spawn_librarian`): serves the library's bytes,
advertises `artifact/librarian`, and syncs manifest → `installable/` catalogue. An `installer`
**discovers** the entry via `InstallableCatalog::from_kv` (no registry server — the catalogue
*is* the gossip store), **verifies provenance**, pulls via `MeshArtifactSource::resolving` (the
holder is *discovered through the capability ring*), provisions the WASM component, serves
`route/optimize` — and **re-serves its verified cache** as a peer holder. A `caller` invokes it.
Then the librarian is killed **and the library directory deleted** — the origin tier is gone —
and a `late` node joins, still finds the catalogue entry (ordinary KV), and installs **from the
installer's cache**: same hash, same verify, holders are interchangeable.

**Philosophy beat:** the catalogue is not a server you deploy — it's gossiped KV, so it's as
available as the cluster. The library is an **origin tier, never a mandatory read path**: content
addressing makes every holder (librarian or peer cache) equally verifiable, so losing the origin
pauses nothing that any live holder can serve. Full operator + developer guide:
[operations/artifacts.md](../../docs/operations/artifacts.md); design record:
[design/artifact-library.md](../../docs/design/artifact-library.md).

### 12 — `diagnostics`

**Purpose.** The operator surface for *Legible Emergence*. The morning-rush coordinator publishes an
intent — keep `rush-pool` between one and two depots — while four are already serving it. That is a
benign intent-vs-reality mismatch, the kind an on-call volunteer would otherwise see only as "the
depot count looks off". Every depot computes a **fleet diagnosis** from the gossiped KV it already
holds, so we induce the mismatch on `depot-a` and ask `depot-b`, which never saw the operator's
action, *what's wrong* — and it names the group, the band and the fix from its own local view.

```bash
cargo run -p mycelium-coop-examples --bin diagnostics
```

**Expect:** `All assertions passed`, with a line beginning `diagnosed` that names `rush-pool`, the
`[1, 2]` band and the four registered depots. **Key code:** `agent.fleet_diagnosis()`; the same
answer over `GET /gateway/diagnose`. **Exercise:** widen the band to `[1, 4]` and rerun — the
diagnosis should disappear, not soften. **Induced failure:** stop `depot-a` before `depot-b`
diagnoses; the mismatch is still named, because the intent and the registrations are in the KV,
not in `depot-a`. **Proves:** diagnosis is *data*, computable anywhere the KV is. **Does not
prove:** that a diagnosis is acted on — nothing here changes the pool. **Adapt:** replace the
depot registrations with your service's group registrations; the intent key and the diagnosis
API are unchanged.

### 13 — `procurement_authority` ⭐ (the governed-autonomy flagship)

**Purpose.** What a gateway can promise about an agent's actions, and what it cannot — on the
**public seam and the reference evaluator**, so an adopter can run it, read it and build against it
(the Cedar adapter and the evidence exporter are the private companion's; this runs without them).
A co-op agent holds an approved purchasing remit with a ceiling. Six things happen:

1. **A purchase within the remit** — permitted; the decision says what it checked.
2. **An over-limit purchase** — *denied*, no business effect, and note who attests that.
3. **A purchase nobody wrote a rule about** — **not denied**: *authority not established*, a
   different fact that must never be reported as drift.
4. **A steward's intervention** — a different principal, authorised on its own terms.
5. **A misconfigured route** — denied, and it happened anyway. The evidence says **both**. This is
   the act worth staying for: permission is not proof of outcome.
6. **A corrected observation** — the supplier's first report was wrong; the correction cites the
   record it replaces and the original stays on disk.

Then it runs the **contract fixtures** against the evaluator — what an operator does to a
replacement evaluator before trusting it.

```bash
cargo run -p mycelium-coop-examples --bin procurement_authority
```

**Expect:** `All assertions passed`; act three prints `authority not established`; act five prints
`denied, and it ran anyway`. **Key code:** the `ActionEvaluator` attach, the `ReferenceEvaluator`
rules, `AeEvidence` records, the correction. **Exercise:** raise the ceiling and watch act two
become act one. **Induced failure:** remove the rule for act one; it becomes act three, *not* a
denial. **Proves:** decision, execution and outcome are distinct records, and a bypass is
*evidenced*, not prevented, by a route-level preflight. **Does not prove:** enforcement at the
resource (that is `with_provider_enforcement` and the confined-fleet deployment), or a deployed
cloud integration or a completed consumer acceptance journey — the fixture consumer here is a stub.
**Adapt:** swap the co-op's operations for your tool names and the remit for your policy; the
evaluator seam and the evidence shape are what you keep. **Business translation:** see the table
under *Reading the co-op as your business* above.

### M — `model_deploy` (manual — a real LLM through the library)

```bash
# needs: ollama daemon running + any GGUF file (19 MB TinyStories shown)
curl -L -o /tmp/stories15M-q4_0.gguf \
  https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories15M-q4_0.gguf
MODEL_GGUF=/tmp/stories15M-q4_0.gguf \
  cargo run -p mycelium-coop-examples --features wasm --bin model_deploy
```

The Blob path proven with **nothing simulated** — and **both halves of a model deployment
governed**: the **weights** (a genuine GGUF) *and* the **profile** (system prompt +
parameters) travel the library as two signed, content-addressed artifacts. The profile
references the weights **by content address** (`FROM artifact:{hex}`); activation resolves
the reference against the local placement dir and runs the real `ollama create` — a profile
that activates before its weights simply fails and retries (restart ≡ provisioning is the
ordering; no dependency resolver, M15 one-hop preserved). A librarian syncs the catalogue, a
model-host **self-elects under the real resource probe**, streams the weights **direct from
the store** (design §5 — the mesh RPC's 10 MiB frame is for WASM-sized artifacts) with the
`llm/loading` percent driven by actual bytes, probe-gates the capability on the activation
health bit — and an `app` node **generates real tokens under the governed profile**, with
`ollama show` asserted to carry the SYSTEM prompt that arrived in the signed artifact.
Deliberately **not** in `ci_smoke.sh` (needs Ollama + a model); run it when you want to see
the artifact library move something real.

**Philosophy beat:** the same demand→provision loop as 04/09/11 — but the artifact is a
neural network, the progress bar is honest, and the proof is the story it tells you.


### M+ — `reheal_deploy` (manual — the real-model reheal flagship)

**Objective.** The composition of `model_deploy` and the LangGraph rung-6 flagship, with a
*real* neural network: **a governed GGUF model reheals onto the surviving node and generates
real tokens through routed inference after its origin dies.** The one story that beats a
commodity checkpoint store on non-commodity terms (the echo-model CI variant is
[`langgraph/`](../langgraph/README.md) rung 6; this is the same choreography with real weights).

**Run** (needs Ollama + a GGUF, like `model_deploy`):
```bash
cargo run -p mycelium-coop-examples --features wasm --bin reheal_deploy
```

**What it demonstrates.** The full artifact-library pipeline (profile → weights by content
address → resource-checked election → streamed activation) *plus* origin death: the library's
durable tier re-serves the artifacts, the survivor self-elects, reheals the model, and the
routed inference call returns real tokens — deploy, kill, reheal, generate, all coordinator-free.
See the header of [`src/bin/reheal_deploy.rs`](src/bin/reheal_deploy.rs) for the design notes.

## CI

`./ci_smoke.sh` runs the shipped demos Docker-free and asserts on their output (wired the same way
as the AFN fluid-pipeline smoke).
