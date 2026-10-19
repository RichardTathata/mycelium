# 00 · Concepts & Vocabulary

Read this first. Mycelium has its own vocabulary (Capability, Skill, Signal,
Requirement) that sits *next to* industry standards it speaks at its edges
(A2A, MCP, NANDA AgentFacts) — and the two are easy to conflate. This chapter
draws the lines, says when to reach for each, and points every term at a
runnable demo.

Every concept below is, ultimately, **a key in the one gossip KV store** — that
is the substrate unity the [guide intro](README.md) describes. The differences
are about *what the key means* and *who reads it*, not about separate systems.

---

## The one distinction that matters: native model vs. edge standards

| | What it is | Examples | Where it lives |
|---|---|---|---|
| **Mycelium-native model** | The substrate's own vocabulary — how nodes describe, find, and run work *inside* the mesh | Capability · Skill · Requirement · Signal · Artifact | KV prefixes (`cap/`, `prompts/`, `req/`, …) + the signal mesh |
| **Edge standards** | Industry protocols Mycelium **speaks at its boundary** so the outside world can interoperate — *export / bridge formats, not the internal model* | A2A AgentCard · MCP tool · NANDA AgentFacts | `/.well-known/*` endpoints + `tools/` |

If you remember one thing: **inside the mesh you think in Capabilities and
Skills; at the edge you speak A2A / MCP / AgentFacts.** The standards are
generated *from* the native model (the A2A card is built from `cap/`; AgentFacts
are signed from live capabilities), never the other way around.

---

## Capability vs. Skill vs. MCP tool vs. A2A vs. AgentFact

This is the headline confusion. Here is the whole thing in one table:

| Term | Native or standard? | One-line definition | When you use it | Demo |
|---|---|---|---|---|
| **Capability** | native | A declarative advertisement: "this node provides `ns/name`." The discovery *atom*. It does nothing by itself — it is found, not called. | Advertise what a node can do; discover providers at call time | [`provisioning`](../../examples/coop/src/bin/provisioning.rs) |
| **Skill** | native | A **Capability + an executable handler**. A *Prompt Skill* is LLM-backed (template in `prompts/`, an `LlmBackend` runs the inference); a *SkillRunner skill* is a hosted process. | Make a capability *invokable* — the unit you actually call | [`mailbox_llm`](../../examples/coop/src/bin/mailbox_llm.rs), [`llm_council`](../../examples/coop/src/bin/llm_council.rs) |
| **MCP tool** | **standard** (Model Context Protocol) | A JSON-schema'd tool registered at `tools/{name}/{node}`, invoked over `mcp.invoke`. Bridges LLM tool-use ↔ the mesh, both directions. | Expose tools to an LLM, or bridge an external MCP server's tools in | [`mcp_toolgrowth`](../../examples/coop/src/bin/mcp_toolgrowth.rs) |
| **A2A AgentCard** | **standard** (Google A2A) | An agent-discovery document served at `/.well-known/agent-card.json`, **built dynamically from `cap/`** — the skills `/a2a` can call (not the fleet's plumbing, not prompt skills). | Let an *external agent framework* (LangChain, AutoGen) discover & call your agents | [`a2a_langchain`](../../examples/a2a_langchain/) |
| **AgentFacts** | **standard** (NANDA) | A self-certified metadata document at `/.well-known/agent-facts.json` for cross-domain discovery. | Federate discovery across *domains* with no shared trust authority | [`federation_facts`](../../examples/coop/src/bin/federation_facts.rs) |

### Why "Skill" had to exist (it's not just a renamed Capability)

A Capability is *only* an advertisement — `cap/{node}/nlp/summarize` says "I
summarize," but there is nothing behind it to call. `register_prompt_skill`
does three things at once, and that triple is the Skill:

1. writes the prompt **template** to `prompts/{ns}/{name}` (the configuration),
2. **advertises a `Capability`** under `cap/{node}/{ns}/{name}` (so it's discoverable),
3. registers an **`LlmBackend`** as the handler (so it's runnable).

So: **a Skill is a Capability you can invoke.** The Capability is how peers
*find* it; the template + backend are how a call *runs*. Strip the handler and
you're back to a bare Capability; strip the LLM backend and use a process and
you have a SkillRunner skill instead.

### Why A2A / MCP / AgentFacts are *not* the same thing

They are **external interop standards**, each for a different audience:

- **MCP** is for *LLM tool-use*: an LLM (yours or someone else's) discovers and
  calls tools. Mycelium can publish a Skill as an MCP tool, or pull a remote
  MCP server's tools into the mesh.
- **A2A** is for *other agent frameworks*: a LangChain or AutoGen agent treats
  your Mycelium skills as callable tools by reading the AgentCard. A prompt skill is not on
  it — it answers `llm.invoke`, reached through `POST /gateway/llm/call` (`llm:invoke`).
- **AgentFacts (NANDA)** is for *cross-domain federation*: a neighbouring
  cluster discovers your domain's capabilities by pulling a signed facts
  document — no shared CA, trust is the fetcher's decision.

A Capability/Skill is the thing; A2A/MCP/AgentFacts are three different windows
onto it for three different outside callers. The federation window has its own
how-to: [17 · Federation](17-federation.md) (serve + verify the edge doc, no shared CA).

---

## The other concept-pairs people conflate

Short, so you can skim. Each links the demo that makes it concrete.

**Capability vs. Requirement vs. Demand.** A **Capability** (`cap/`) is "I
provide X." A **Requirement** (`req/`) is "I need X." **Demand** is the derived
pressure — requirements with no matching provider — that a provisioner reacts
to. → [`provisioning`](../../examples/coop/src/bin/provisioning.rs),
[`mcp_toolgrowth`](../../examples/coop/src/bin/mcp_toolgrowth.rs).

**Prompt Skill vs. SkillRunner skill vs. MCP tool.** Three "invokable units."
*Prompt Skill* = LLM call behind a capability (in-process backend). *SkillRunner
skill* = a hosted process advertising a capability (the `skillrunner` binary,
TOML manifest). *MCP tool* = the standards-based bridge for LLM tool-use, in the
separate `tools/` namespace. The first two are native Capabilities; the third is
an edge standard.

**Ack vs. receipt (v3 contracts axis, item 1).** Today's `bool` / `Ok` acknowledgements each
prove something different — `kv().set` proves *queued for gossip*, the deprecated `set_with_min_acks` proves
**nothing a caller can obtain** (§1a of the contracts record — its replacement `set_with_replica_sync`
asks peers instead and names them), `Committed { persisted }` folds *fsynced* and *never promised*
into one `true`. A **receipt** names its rung on the ladder and nothing above it:
**local application** (`Applied` / `AlreadyCurrent` / `Superseded` / `Refused` — four, because the
store's "nothing changed" covers three different truths) · **local sync** (this exact operation
crossed this node's persistence barrier: `OnDisk` / `Buffered` / `Failed` / `NotConfigured`) ·
**replica sync** (named peers persisted this exact operation; peers that were asked and did not
answer are **unknown**, not "did not persist") · **destination commit** (an external destination
committed the business change and its dedup result in one transaction — the only rung that
establishes an exactly-once effect, and the only one the substrate never provides on its own).
`Buffered` is the one to read carefully: it survives a process crash and is lost to a power
failure, so it is a *different* claim from `OnDisk` rather than a weaker one. Every operation
carries a caller-minted **`operation_id`** (stable across retries and worker replacement) and an
**`attempt_id`**; same id + different content is a `Conflict`; a timeout returns the rungs that
were established plus **`DeliveryUnknown`** — never "nothing happened". → [18 · Contracts &
receipts](18-contracts-and-receipts.md), [`design/contracts-receipts.md`](../design/contracts-receipts.md),
[`receipt_ladder`](../../examples/receipt_ladder.rs).

**Signal vs. KV entry.** A **Signal** (Layer II) is an *ephemeral* scoped event
— miss it and it's gone. A **KV entry** (Layer I) is *durable* state that
gossips and heals via anti-entropy. Use a Signal to *notify*; use KV to *record*.
The mailbox is the bridge: durable, ordered event delivery built on KV. →
[`mailbox_llm`](../../examples/coop/src/bin/mailbox_llm.rs).

**Replication vs. persistence.** KV *replication* (gossip + anti-entropy) is what makes a value
survive a **node** dying — peers hold it. *Persistence* (`PersistenceConfig`: a local WAL +
snapshot, off by default) is what makes a value survive the **whole cluster** restarting, or a
single node that was the only holder. They compose: a restarted node replays its own WAL, then
re-learns anything newer from peers. → [01 · Gossip KV § Persistence](01-gossip-kv.md),
[deployment § Persistence modes](../operations/deployment.md#persistence-modes).

**The two "TTL"s — don't conflate them.** The wire frame's **hop-count TTL**
(`u8`, decremented per forward) bounds how far a frame travels. Key
**evaporation** is a *read-side convention*: capability/load entries carry a
`refresh_interval_ms` and readers discard anything older than 3× it. The store
never time-evicts a live key; evaporation is "stop believing a stale
advertisement," not "delete." → see [`01-gossip-kv.md`](01-gossip-kv.md).

**Three kinds of "group."** An **emergent group** forms when nodes self-join by
matching a `CapabilityGroupDef` filter (no coordinator). A **consensus group**
is the set of voters a `group_propose` / `cross_group_propose` requires quorum
from. A **membership-governed group** is an emergent group whose size is bounded
by a `MembershipIntent` the `MembershipGovernor` reconciles toward. →
[`consensus`](../../examples/coop/src/bin/consensus.rs) (consensus group),
[`elastic_intent`](../../examples/coop/src/bin/elastic_intent.rs) (governed).
That is the split by *governance role*; groups also split by *API* — a **signal group**
(`mesh().join_group`) vs a **capability group** (`capabilities().define_capability_group`), an
orthogonal distinction covered in [the group-API note](README.md#groups--two-distinct-concepts).

**Governed vs. ungoverned.** By default a group is **ungoverned**: it just self-organises — nodes
join/leave by capability, and the size is whatever it is. A group becomes **governed** only when
someone publishes a `MembershipIntent` for it; then a `MembershipGovernor` actively holds it to a
`min`/`max` band (spinning members up/down or flagging a `conflict`). Govern a group when you need
a *bounded pool* ("keep 5–10 render workers") or want it observable against a target; leave it
ungoverned when self-organisation is enough (most groups). Governance is additive — the *same*
group is just being watched by a governor, and its membership becomes a governance act: over HTTP a node
joins or leaves it through `/gateway/govern/group` (`govern:write`), not `/gateway/mesh/group` (2.29.0). **Why create a group at all** →
[cookbook: "why (and when) create a group"](cookbook.md#why-and-when-would-i-create-a-group-within-a-cluster).

**Consensus vs. LWW vs. tuple-space rendezvous — "how do agents agree?"**
**LWW** (last-write-wins, the default) — no agreement, newest timestamp wins;
use for soft state. **Consensus** (Layer III) — real agreement via quorum; use
when a decision must be singular and durable. **Tuple-space rendezvous** — no
agreement at all; workers *pull* work when ready (the coordinator-free
alternative to predicting who does what). →
[`consensus`](../../examples/coop/src/bin/consensus.rs),
[`llm_pipeline`](../../examples/coop/src/bin/llm_pipeline.rs).

**Tuple space vs. blackboard — "how does a consumer find its work?"** Two
companion crates, two answers, both on the public API. The
[`mycelium-tuple-space`](../../mycelium-tuple-space/) routes by **position**:
named FIFO lanes (stages), topology known up front, O(1) claims — for pipelines
whose stages you know. Fan-in joins by correlation key use its keyed
`take_by_key` (M13). The [`mycelium-blackboard`](../../mycelium-blackboard/)
routes by **content**: a consumer names a **predicate** over fact attributes and
the topology is *emergent per item* — for opportunistic reasoning where *which*
agent acts is decided by the fact, not by where it was put. The blackboard adds
the one primitive the substrate lacks — **competitive destructive
claim-by-predicate** (Linda's `in`); non-destructive shared reads (`rd`) are
already the substrate's (gossiped facts + predicate filters). One line: *known
stages → tuple space; emergent topology over shared facts → blackboard.*

Both of those are **transient** — work found and consumed. The durable sibling is
[`mycelium-wiki`](../../mycelium-wiki/): a group-scoped, **LLM-curated** knowledge
canon (the *maintained-meaning / authoritative-specific* layer, composing with
Postgres metrics + RAG background by a shared id namespace). It answers a
different question — *"where does a group keep what it learns?"* Not in gossiped
KV: the corpus lives in a node-independent store, a single elected **curator**
serialises writes (so concurrent edits need no CRDT) while agents **read directly,
in parallel**, and Mycelium supplies only the control functions (curator election +
ring-failover, an evaporating proposal queue, MCP + gateway, a membership-gated
access broker). One line: *transient work → tuple space / blackboard; durable
curated knowledge → wiki.*

**Opacity / pheromone / load / backpressure.** All one mechanism: a node writes
its own state under `sys/load/{self}/…`; anything scanning that prefix sees a
consistent picture with no coordination. **Load** is the raw fill; **opacity**
is the boolean "skip me" a governor derives from it; the written entry is the
**pheromone** others read; **backpressure** is the resulting reroute. →
[`stigmergy`](../../examples/coop/src/bin/stigmergy.rs).

**Mailbox vs. Signal vs. RPC vs. Bulk vs. Scatter.** The service patterns, by
shape: **Signal** = fire-and-forget event. **RPC** = request/reply to one
provider. **Mailbox** = durable, HLC-ordered event delivery (survives a restart
within the TTL window). **Bulk** = large payloads over HTTP staging, not the
signal mesh. **Scatter** = query many providers, aggregate replies. →
[`mailbox_llm`](../../examples/coop/src/bin/mailbox_llm.rs).

**Artifact vs. Capability vs. Skill vs. Tool — the deployable-unit ladder.**
An **Artifact** is deployable *bytes* (a WASM component / model) in the
`installable/` catalogue. When a node pulls + verifies + instantiates it, it
**becomes a Capability** the node advertises — and if that capability has a
handler it's a **Skill**, or if exposed over MCP a **Tool**. Artifact is the
package; the rest are what it turns into once running. →
[`provisioning`](../../examples/coop/src/bin/provisioning.rs); the catalogue
itself — where it lives and how to author/publish to it — is the
[`mycelium-wasm-host`](../../mycelium-wasm-host/) crate, and publishing, verifying
and accepting entries is [`operations/artifacts.md`](../operations/artifacts.md).

**Node = Agent = Process.** One Mycelium **node** is one `GossipAgent` is one OS
process is a full peer (registry + bus + scheduler in one). The coop demos name
their nodes "depots" / "agents" for the story — same thing.

**Promise-strength vs. mechanism-strength.** Namespace ownership (e.g. only
`{node}` should write `sys/load/{node}/…`) is **convention**, not enforced: a
rogue write is *applied* per LWW but **flagged** by a tripwire counter
(`commit_conflicts`, `sys_namespace_violations` on `/stats`) — **detection, not
prevention**. Commitments are promise-strength, the only honest strength a
coordinator-free system can offer. The axis names three tiers (`Strength`):
**hard prevention** (the provider rejects it), **self-imposed prevention** (a boundary drops it),
**self-imposed transition** (a state change). Every module states its own, and the load-bearing
rule is that a *hard bound* is hard prevention **only when backed by exclusive, durably accounted
rights** — without those it is not a hard bound however it is labelled. A gateway that checks
before dispatching is self-imposed prevention for the routes it fronts and **nothing at all** for
routes it does not. → [16 · Guardrails](16-guardrails.md),
[`consensus`](../../examples/coop/src/bin/consensus.rs)
(an empty bloc can't be coerced), [`federation_facts`](../../examples/coop/src/bin/federation_facts.rs)
(a tampered document fails verification at read).

---

**Mandate vs. term vs. epoch (item 5).** A **mandate** is one appointment: who may do what, under
whose authority, for how long (`Mandate`, with an *enumerated* `operations` list — absence is
denial, there is no wildcard). The **term** (`TermId`) identifies *which appointment this is*; the
**epoch** (`u64`) *orders authority*. Collapsing them would make "same holder, reappointed after a
gap" indistinguishable from "never lapsed". A resource installs an epoch and never goes backwards,
so once it acknowledges epoch 2, nothing authorised only under epoch 1 can commit there — however
the holder retries, reconnects or restarts. The refusals are typed on purpose: a superseded mandate
is `MandateSuperseded`, **never `Conflict`**, because `Conflict` is a retry loop's input and the
retry loop would launder the revocation. → [`curator_handover`](../../mycelium-wiki/examples/curator_handover.rs),
[`design/scoped-mandates.md`](../design/scoped-mandates.md).

**Allocated right vs. soft state (item 4).** Everything a governor reads from discovery is soft
state that **evaporates** when its owner goes quiet. That is the right shape for an *observation*
and the wrong shape for a *right*: an allocation that vanished with its holder would be issued
twice. So a **right** (`Right`: holder, resource, `units` in resource-native units — never money,
term, state) lives in a node-local, append-only, **fsynced journal that is never gossiped**; only a
bounded signed `RightsHead` leaves the node. Three rules: *persisted before acting*, *never
reclaimed because an owner vanished from discovery*, and *a rejected admission is a first-class
recorded outcome* — because a rejection nobody recorded is a silence, and a silence is
indistinguishable from work nobody asked for. Every state counts against the budget, including
`Unknown`. → [`control_envelope_viz`](../../examples/control_envelope_viz.rs),
[`design/adaptive-stability.md`](../design/adaptive-stability.md).

**Federated domain vs. public discovery (item 2).** A **domain** is one independently admitted
gossip mesh, owning its own membership, replication, policy and electorate. Admission is the per-node
certificate trust root, **not a name** — `cluster_name` stays a cosmetic label. Federation connects
*explicitly exported services* between meshes and **never joins the transports**: a foreign node
never enters membership, the native namespaces, anti-entropy or a quorum. That is the whole
invariant, and it is why there is deliberately no `federation/` key prefix. Against that,
**AgentFacts** is *unilateral public discovery* — a self-certified document anyone may fetch, where
trust is the reader's decision and nothing is granted. They are one model, not two: a domain
descriptor's public subset is serialised as an AgentFacts profile through the existing serialiser,
so there is no second well-known document. → [17 · Federation](17-federation.md),
[`federated_domains`](../../examples/federated_domains.rs).

**Announcement · offer · award · report · assessment (the contract net).** Five records, one
mechanism each. The **award** is the one to understand: it is a *receipt-bearing operation*, never a
key-value write alone, so a retry is recognisable and its durability is exactly what the receipt
says. One award per requirement — a second is **refused**, never written over the first. The award
also names the offer it accepted, which is the mechanical form of *no component assigns another
participant's obligation*. A participant's `Outcome` is `Fulfilled` / `Failed` / **`Unknown`**, the
same honesty as item 1's receipts. And an unsigned **assessment** verifies as `false` — *unproven*,
not forged. → [`redistribution_cn`](../../mycelium-commitment/examples/redistribution_cn.rs).

**Deny vs. indeterminate (the authorisation seam).** An **action envelope** is who is asking to do
what, to which resource, with which arguments — assembled only by the enforcement point, from facts
it verified. The evaluator returns `Permit`, `Deny` or `Indeterminate`, and **`Indeterminate` is
never `Permit`**. `Deny` says the policy establishes that this action is refused. `Indeterminate`
says authority could *not be established*: nothing says the action is forbidden, only that nothing
says it is allowed. A declared argument missing from a request must produce `Indeterminate`, never a
guess. Separately, **coverage is a field, not a hope**: the evidence names the routes the enforcement
point does not see, rather than leaving a reader to infer an all-clear from silence.

**Enforcement point vs. evidence journal.** An **enforcement point** is the place that checks
authority before work runs: it assembles the action envelope from facts it verified, asks the
evaluator, and dispatches or refuses. The substrate has two, and each records its decisions under its
own name: the **gateway** (`point=gateway` — `/mcp`, `/a2a`, the LLM and federation routes count
together) and the **provider** check that `with_provider_enforcement()` adds before a handler runs
(`point=provider`). A resource that commits an effect checks its own installed epoch at its commit
boundary, a separate check with its own refusal (guide 20, tutorial 4). Where it stops: an
enforcement point covers only the routes it fronts, and a route no enforcement point fronts inherits
nothing; it needs the `gateway` and `tls` features (`compliance` includes both and adds the audit
chain), an attached evaluator and configuration; and provider enforcement with no evaluator refuses protected
work rather than allowing it. The **evidence journal** (`EvidenceJournal`,
`src/agent/evidence_journal.rs`) is where an enforcement point records what it decided and what
became of the action: node-local, append-only, fsynced, **never gossiped**. Only a hash-bearing
reference enters the tamper-evident audit chain. Without a journal an attached evaluator still
enforces and records nothing, and the node warns at attach time. The journal is evidence of
decisions, not a replay bundle: it does not let a run be repeated. → [20 · Authorising
actions](20-authorising-actions.md).

**Exact replay vs. scenario replay (item 6).** A **seed is not a durable reproduction artefact** —
a run reproduced by re-seeding is reproducible only while the code is unchanged, which is exactly
when nobody needs it. A **bundle** instead records what the production code *asked for* and what it
*received*, so a replay checks each request against the recording and tells you **where** a changed
build first departed. Replaying against the pinned build is an **exact** replay; replaying against a
different commit is a **scenario** replay, a weaker claim, and the bundle records the build so the
difference is visible rather than assumed. A bundle also carries a **witness** — the assertion that
failed — because a bundle without one replays a run in which nothing went wrong, which proves only
that the harness works. → `cargo run -p mycelium-sim --example replay_a_bundle`.

**Claim vs. observation vs. assessment vs. acceptance (item 3).** A **claim** is an issuer asserting
something. An **observation** is an issuer reporting what it saw. An **assessment** is a *judgement*,
with an author. An **acceptance decision** is a reader deciding what to do. The kind is part of the
signed bytes, so an observation's signature can never authenticate an assessment. Only assessments
count as evidence, and **a self-assessment does not count**, which is why the first two kinds are
split from the third at all. The reader's verdict has four outcomes, and
`InsufficientEvidence` ("we do not know") is deliberately distinct from `Rejected` ("we looked, and
it is bad"). Disagreement is preserved rather than resolved: `Challenges` links across issuers are
the point. → [`design/knowledge-layer.md`](../design/knowledge-layer.md).

**The gateway caller vs. the gateway node (item 7).** When a request arrives over the gateway, the
provider is handed a **`GatewayCaller`** — the verified *client* principal, issuer-qualified
(`oidc:{issuer}/{subject}`, `token:{issuer}/{name}`), not the gateway's own node identity, and never
the credential itself. Authorisation judges that client: **listing the gateway node does not admit
its clients**. A context that fails verification is a denial, never a fall-back to running the call
as the node. One thing, three names: this guide says **gateway caller**; the code also says **caller
context** for the same value, and **caller envelope** for its form on the wire — the
`GatewayCaller` framed into the RPC payload. Under `tls` the envelope is signed by the gateway's
identity key; without `tls` it is unsigned, and carried exactly as strong as the frame's sender. Either
way a provider refuses one issued more than 300 s plus the node's clock-drift bound ago. That a provider
admits each signed envelope only once (per gateway, issue time and id) is post-360 row E's nonce
cache, 2.35.0 (`src/agent/gateway_caller.rs`). The caller envelope is not the **action envelope** below: the caller
envelope tells a provider who called; the action envelope is what an enforcement point hands its
evaluator. → [09 · Security](09-security.md), [`operations/rbac.md`](../operations/rbac.md).

**Refusals are typed by what you should do next.** This is the axis's actual thesis, and it shows up
in nearly every error type above. A refusal a retry loop would consume is a *different type* from
one it would not. `MandateSuperseded` is not `Conflict`, or the retry loop launders a revocation.
A bad signature is not a policy gap, or the operator edits the wrong file. `Indeterminate` is not
`Deny`. `InsufficientEvidence` is not `Rejected`. A durability `Failed` does not mean the record is
absent. `DeliveryUnknown` does not mean nothing happened — and there is deliberately **no** variant
that means that, because no verb on that path can establish it. When you add an error variant here,
the question to answer is not "what went wrong" but "what should the caller do differently".

---

## Fleet growth

How a fleet acquires a capability it lacks, in the words [tutorial 1](tutorials/01-first-stem-fleet.md)
uses. The workflow is [capability-lifecycle.md](../operations/capability-lifecycle.md); every field is in
the [unit-file reference](../reference/unit-file.md). Each term has a one-line row in the
[glossary](#glossary-quick-reference).

**Stem node.** A **stem node** (*stem* for short) is a node that declares from its unit file and loads
what the declarations call for ([`stem.rs`](../../mycelium-wasm-host/src/stem.rs); the `mycelium-stem`
binary, or `Stem::start` embedded). At start it advertises every `[[capability]]`, declares every
`[[requirement]]` and defines every `[[group]]`. When the file has a `[hosts]` table it also runs a
**provisioner** configured from it, with every `[[presence]]` as a standing want. **Only a stem with
`[hosts]` installs anything.** A **stem fleet** is a fleet of stems started from one image. The name is
from biology: like a stem cell, a stem starts generic and undifferentiated, and specialises when the
fleet needs a capability. The analogy stops early. Only two kinds of thing load dynamically: WASM
components, and blobs for a runtime already on the node. New native code, a companion crate, a gateway
route or a TLS change needs a new image, and the stem image is itself deployed directly. And the
catalogue is the supply chain: a stem installs what the keys in `[hosts].trusted_publishers` signed,
unless its unit file sets `[hosts].accept_unsigned = true`, which installs entries with no provenance;
those keys, and that setting, are the posture to review. Two other kinds of node install nothing: an **SDK agent** (Python
or TypeScript, through the gateway) can declare a unit file with `POST /gateway/units/declare` but is
refused the hosting sections, and a plain **`mycelium` node** does not read a unit file at all.

**Catalogue · library · librarian.** The **catalogue** is the set of installable entries in gossiped
KV: one `installable/{ns}/{name}/{hex}` entry per artifact, naming the capability it provides, its
kind, its content address, its footprint and, when it has one, its publisher's signature (an entry
can be unsigned; a host refuses it unless it accepts unsigned entries). Every node holds the whole
catalogue, and there is no registry server. An entry is only the announcement. The bytes travel
separately, pulled from a holder on demand and verified against the content address. The **library**
is the durable source behind the catalogue: a directory, HTTP blob store or object store holding the
blobs and a signed manifest that CI publishes into. The **librarian** is a role a node takes over a
library (`spawn_librarian`, or `mycelium-stem --librarian`): it serves the bytes, advertises
`artifact/librarian`, and keeps the catalogue reconciled to the manifest, publishing additions and
tombstoning removals. So the library is where artifacts are kept, the librarian serves it, and the
catalogue is what every node sees of it. A **catalogue entry** is one of those announcements. →
[artifacts.md](../operations/artifacts.md).

**Unit file.** One TOML file per deployable unit. It says what the unit offers, what it needs, the
groups it defines, the lanes it touches, the authority it expects, what it can host (`[hosts]`), the
presence it wants, and how a hosted blob reaches its runtime. Every section is optional. One loader
(`NodeCapabilityConfig::from_toml_str`) serves `mycelium wire-check`, the stem and
`POST /gateway/units/declare`. "Unit declaration" and "declaration" mean the same file. A declaration
is not execution: `[[lane]]` (a tuple-space stage, by name and side), `[[mandate]]` and `[[rule]]` are
vocabulary for the offline check and the evaluator, and nothing enforces them at runtime.

**Presence band: floor and ceiling.** A `[[presence]]` section asks for between `min_providers` (the
**presence floor**) and `max_providers` (the **ceiling**) live providers of a capability across the
fleet, independent of demand. The two together are the **presence band**. A band is supervised per
stem, not published to the fleet: every stem that declares this `[[presence]]` in its own unit file,
and whose `[hosts]` can take the entry, reconciles toward it locally; a host that does not declare it
does not fill the band. Below the floor, eligible declaring hosts self-elect and install. Above the
ceiling, the hosts ranked beyond `max` by the band's rendezvous order withdraw (rule `prov.shed`), and
every host with the same view agrees which. Only providers that advertise the band's `prov-shed` mark
are ranked; one that does not (registered in code, or a stem without this band) keeps its place
([unit-file § `[[presence]]`](../reference/unit-file.md#presence)). A band is a **convergence target,
not a cardinality guarantee**: the count can overshoot or dip for a round, and a gap is filled only after
a host sees it. "Provider floor" and "desired floor" mean the presence floor. A stem refuses
`[[presence]]` without `[hosts]`.

**Provisioner vs. governor, and self-election.** The **provisioner** is the loop on a host that closes
a gap: it watches demand (a `req/` entry with no live provider) and presence floors, resolves them
against the catalogue, pulls, verifies and installs through the runtime for the entry's kind, and
advertises the capability ([`provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs)). It is an
ordinary agent on the public API: the library never provisions on its own. A **governor** is a
node-local loop that holds something to a published intent, with no coordinator. They differ in how
they act. The `MembershipGovernor` sizes a group toward a `MembershipIntent`'s `[min, max]` by
self-election. The tuning governor applies a fleet intent to the auto-tuner wherever a parameter is not
pinned locally, and the timing governor applies a whole timing set in one reconcile pass; neither draws.
The membership and tuning governors act under the node's [control
profile](../operations/control-profiles.md); the timing governor does not read it. **Self-election** is
how the provisioner and the membership governor decide: each eligible node rolls a probability every
round and acts only if the roll says so. Nobody assigns the work. The provisioner's probability is
fixed (`mycelium-stem --self-elect`, default `0.5`); the membership governor's grows with the gap. The probability damps the herd, so two nodes acting at once is expected and corrected in a
later round. A node with no runtime for an entry's kind, or no budget for it, never self-elects for it:
eligibility is node-local truth.

**Runtime kind, activation and `[[serve]]`.** An artifact's **kind** says how it becomes live, and a
host's `[hosts].kinds` lists the kinds it has a runtime for: `wasm-component` (instantiated in the
node's WASM sandbox) or `blob` (bytes placed on disk for a native runtime already on the node, such as
an Ollama process). An **activation** (`[[activation]]`) is how a placed blob reaches that runtime: after
the blob is placed and verified, the stem runs the operator's `command` (an argv), then gates the
capability on `probe`, which it re-runs in the background; when the probe fails the install is
withdrawn and a later round reinstalls. Without an `[activation.confine]` table the command and its
probe run with the stem's own privileges: this is **not a sandbox**, since the WASM sandbox covers
components, not these commands. With the table, both run under an OS confinement profile (post-360 row
M, 2.35.0): they may read the placement directory, a fixed system set and the declared `read`
paths, write only the declared `write` paths, and open TCP connections only to the declared ports
(`localhost:<port>` or `*:<port>`; none when `connect` is empty). Seatbelt enforces it on macOS, and
Landlock with seccomp on Linux. A profile the platform cannot apply, or a declared path that is
missing, refuses the activation by name and runs nothing, never unconfined. Confinement narrows what
the command can reach; it does not make an untrusted command safe. It does not limit `stat`, CPU or
memory, and the filesystem and port limits hold until the stem restarts; a confined command starts with
a cleared environment (2.36.0)
([unit-file § `[activation.confine]`](../reference/unit-file.md#activationconfine)). To reach hosts **by
name**, a profile adds `[activation.confine.egress]`: the stem runs an egress proxy on loopback for the
activation, confines the command to that one port, and the
proxy carries its requests only to the listed hosts — a list that can change while the stem runs —
attaching the node's credential bindings as it sends them on (rows L and M, 2.36.0;
[unit-file § `[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress)).
**`[[serve]]`** turns a live install into a routable prompt
skill: it registers `{ns}/{name}` over an OpenAI-compatible endpoint while the install named by
`while_live` is live, and retracts it when the install goes.

**Shadow → accept.** An artifact description with `proposed = true` publishes a **proposed** entry. A
host loads it only into the **shadow lane**: advertised as `{ns}/{name}.shadow`, callable by that name
for comparison, and never resolved by a requirer's filter, so it takes no demand. It loads for real only
after a **reviewer** co-signs it with `mycelium-artifact accept`. A reviewer is a key listed in that
host's `[hosts].trusted_reviewers`, a separate list from `[hosts].trusted_publishers` (nothing refuses
one key in both), and an empty list means a proposed entry never loads for real on that host. A reviewer's signature says someone accepted
the entry, not that the function is correct. → [16 · Guardrails](16-guardrails.md).

**wire-check.** `mycelium wire-check` is a pure function over unit files and, optionally, artifact
descriptions. It applies the mesh's own match rule and reports by name what could not bind
(`unwired requirement`, `presence unhostable`, …). Its answer is **would bind under these
declarations**, never *is bound*: liveness, load, membership now and whether a mandate is current are
runtime facts it does not read ([`wire_check.rs`](../../src/wire_check.rs)).

## Layers at a glance

| Layer | What it adds | Native concepts | Chapter |
|---|---|---|---|
| **I — Gossip KV** | eventually-consistent shared state (LWW, HLC, anti-entropy) | KV entry, evaporation | [01](01-gossip-kv.md) |
| **II — Signal mesh** | ephemeral scoped events with admission boundaries | Signal, Boundary, opacity | [03](03-signals.md) |
| **III — Consensus** | opt-in quorum agreement on top of I (supported profile: threat model §7) | quorum, leased commit, consistent_set | [04](04-consensus.md) |
| **Capability system** | broker-less discovery (cuts across I–III) | Capability, Requirement, Demand, group | [02](02-capabilities.md) |
| **Application / edge** | the patterns + the standards spoken outward | Skill, Artifact · MCP, A2A, AgentFacts | [05](05-skills.md)–[08](08-a2a-interop.md) |

> The substrate (Layers I + II) is the `mycelium-core` crate; everything above
> is the full `mycelium` crate. See the [guide intro](README.md) for which to
> depend on.

---

## Glossary quick-reference

| Term | Native / standard | One line | Demo |
|---|---|---|---|
| Capability | native | "this node provides `ns/name`" — discovery atom | `provisioning` |
| Skill | native | a Capability + a handler (Prompt = LLM; SkillRunner = process) | `mailbox_llm` |
| Requirement | native | "this node needs `ns/name`" — creates Demand | `mcp_toolgrowth` |
| Demand | native | requirements with no provider — the provisioning trigger | `provisioning` |
| Signal | native | ephemeral scoped event (Layer II) | `mailbox_llm` |
| Boundary | native | a node's receptor set — decides if it *acts* on a signal | `03-signals.md` |
| Mailbox | native | durable, HLC-ordered event delivery on KV | `mailbox_llm` |
| Opacity / pheromone | native | self-written `sys/load/` state others route on | `stigmergy` |
| Group (emergent) | native | nodes self-join by capability filter | `02-capabilities.md` |
| Consensus | native | quorum agreement (Layer III), promise-strength | `consensus` |
| Intent | native | evaporating soft-state desired-state; nodes reconcile locally | `elastic_intent` |
| Artifact | native | deployable bytes in the `installable/` catalogue → becomes a Capability | `provisioning` |
| MCP tool | standard | LLM-tool bridge at `tools/{name}/{node}` | `mcp_toolgrowth` |
| A2A AgentCard | standard | agent discovery for external frameworks (`/.well-known/agent-card.json`) | `a2a_langchain` |
| AgentFacts | standard | self-certified cross-domain federation (`/.well-known/agent-facts.json`) | `federation_facts` |
| Receipt | native | what an ack proves, by rung: local application · local sync · replica sync · destination commit (contracts axis item 1) | `design/contracts-receipts.md` |
| `operation_id` / `attempt_id` | native | caller-minted identity of an operation (stable across retries) and of each attempt; the key of every receipt | `design/contracts-receipts.md` |
| `DeliveryUnknown` | native | a timeout's honest answer: the rungs established so far, then *unknown* — never a negative | `design/contracts-receipts.md` |
| `Buffered` | native | a durability rung: survives a process crash, lost to a power failure. A *different* claim from `OnDisk`, not a weaker one | `receipt_ladder` |
| <a id="destination-commit"></a>Destination commit / `mycelium-effects` | native | rung 4 — one effect, once, at a destination you own; a SQLite dedup table keyed by `operation_id`, `DeliveryUnknown` on a passed deadline. `Fresh`: the effect was applied now. `Replayed`: this `operation_id` was already applied with the same content, so nothing is applied twice | guide [18](18-contracts-and-receipts.md) § Rung 4 |
| <a id="mandate"></a>Mandate / term / epoch | native | one appointment (enumerated operations, no wildcard) · which appointment this is · what orders authority; an installed epoch never goes backwards. A mandate's **scope** names what it covers (a group, a scope name), and epochs are ordered per scope. The **resource epoch** is the one a resource has installed: it refuses anything authorised only under an older epoch, whatever an earlier check permitted | `curator_handover`, `composed_commit` |
| Strict eligibility · coverage · `Unknown` | native | a verdict per configured incumbency rule (consecutive terms · cumulative tenure · cooling-off) from a `TermHistory` whose coverage was verified from chained terms; `Unknown` is never eligible; `ready` = the handover journal read **and** every rule decided in favour | guide [21](21-mandates.md) § Eligibility · `strict_eligibility` |
| Egress allow-list | native | a **hostname** list each node enforces on the substrate's own outbound calls — the first URL and every redirect hop (≤ 5, never https → http); an object store on the endpoint it dials; names, not addresses | `operations/crown-jewel.md` § 2 |
| Allocated right | native | exclusive units in a node-local fsynced journal, never gossiped and never reclaimed because a holder went quiet (contracts axis item 4) | `design/adaptive-stability.md` |
| Domain | native | one independently admitted mesh; federation exports *services* between domains and never joins the transports | `federated_domains` |
| Award | native | the contract net's accepted offer — a receipt-bearing operation, one per requirement, a second is refused | `redistribution_cn` |
| `Indeterminate` | native | authority could not be **established** — never a permit, and never the same as a deny | `design/action-envelope-ae0.md` |
| "No control plane" | definition | *Control decisions live in participating nodes: enabled governors and provisioners act locally, subject to configured consensus and resource-side authority checks. No separate Mycelium control-plane service is required.* — no *separate* service to run, not the absence of control | `positioning.md` |
| Things called "profile" | disambiguation | a **domain profile** (`domain_profile = Enforced`: this mesh is a federated domain — TLS required, SWIM refused) · a **guarantee (startup) profile** (`profile = "secure-single-domain"`: the guarantees a node refuses to start without) · a **control profile** (`Legacy` → `Observe` → `EnforceLocal` → `EnforceAllocated`: how the node's governors act, set at runtime) · a **consensus policy** (per operation: quorum, lease, ballot). None implies another | `operations/control-profiles.md` |
| Guarantee report · profile | native | what this node enforces and why not the rest — every core guarantee resolved at `start()` to `enforced` · `not_configured` · `not_in_build` · `not_applicable` · `not_verifiable_here` (`guarantee_report()`, `GET /gateway/guarantees`); a **profile** (`dev`, `secure-single-domain`) names the set a node refuses to start without | `reference/guarantee-catalogue.md`, guide 20 |
| Rule catalogue | native | every decision point the substrate has, described by stable id — trigger, inputs, typed outcomes and reasons, guards, relations as hypotheses, code and tests, trace policy; generated and gated, `mycelium rules` | `reference/rule-catalogue.md`, guide 19 |
| Decision trace | native | a bounded record of what each instrumented decision point read, how it ended and why — off unless attached, changes no decision, never waits; `decisions.jsonl` + `coverage.json` in a bundle, read with `mycelium explain` | guide 19 § reading what a node decided |
| Claim / observation / assessment | native | an assertion · a report · a *judgement with an author*. Only assessments count as evidence, and not one's own | `design/knowledge-layer.md` |
| <a id="gateway-caller"></a>Gateway caller (caller context, caller envelope) | native | the verified client principal a provider sees behind a gateway call (`GatewayCaller`, item 7); *caller context* is the same value, *caller envelope* its wire form — signed under `tls` (unsigned without it) and refused past 300 s plus the drift bound; a signed envelope admitted once only (post-360 row E's nonce cache, 2.35.0). Not the action envelope | `docs/operations/rbac.md` §7 |
| Action envelope | native | who is asking to do what, to which resource, with which arguments — assembled only by the enforcement point from verified facts (AE0) | `design/action-envelope-ae0.md` |
| Evaluator · permit / deny / indeterminate | native | a replaceable, deterministic decision over an envelope + authority facts; indeterminate is never permit; the secure profile refuses on it | `design/action-envelope-ae0.md` |
| Policy revision · coverage · deployment report | native | the digest of the policy actually evaluated; the routes an enforcement point does not see (`coverage.complete: false`); attributable testimony about activation, never proof of coverage | `design/action-envelope-ae0.md` |
| Seam | native | a replaceable adapter behind which production decision logic runs unchanged — wall clock, monotonic clock, named RNG streams, timers, scheduling, channels, storage, external inputs (replay, item 6) | `design/replay-nondeterminism-inventory.md` |
| <a id="bundle"></a>Bundle | native | a recorded run that a reviewer replays exactly: what production asked for and received (build identity, config, initial disk state, redacted inputs, the ordered choices trace), plus the witness that failed; exact replay detects divergence, never merely re-seeds | `replay_a_bundle`, `design/replay-nondeterminism-inventory.md` |
| **Fleet growth** | | *§ [Fleet growth](#fleet-growth) has the longer form of each row below* | |
| <a id="stem"></a>Stem · stem node · stem fleet | native | a node that declares from its unit file (`mycelium-stem`, `Stem::start`); with a `[hosts]` table it runs a provisioner and installs signed artifacts, and only then. A stem fleet is many stems from one image. Named after a stem cell: generic until the fleet needs something. An SDK agent or a plain `mycelium` node installs nothing | `first_stem_fleet` |
| <a id="catalogue"></a>Catalogue · catalogue entry | native | the gossiped `installable/{ns}/{name}/{hex}` entries every node holds: capability, kind, content address, footprint, and a publisher signature unless the entry is unsigned (installed only where `accept_unsigned = true`). The announcement only: the bytes are pulled and verified separately | `catalog` |
| <a id="library"></a>Library · librarian | native | the durable store of blobs and a signed manifest (directory, HTTP or object store) · the node role that serves it and keeps the catalogue reconciled to the manifest | `catalog`, `operations/artifacts.md` |
| <a id="unit-file"></a>Unit file (unit declaration, declaration) | native | one TOML file per deployable unit: offers, needs, groups, lanes, authority, hosting, presence, activation, serving; one loader for `wire-check`, the stem and `POST /gateway/units/declare`. Declaring is not executing | `reference/unit-file.md` |
| <a id="lane"></a>Lane (`[[lane]]`) | native | a tuple-space stage a unit names, with its side (`produces` / `consumes`); the checker joins lanes by name, and nothing runs them at runtime | `reference/unit-file.md` |
| <a id="presence"></a>Presence band · floor · ceiling | native | `[[presence]]`: keep from `min_providers` (the floor) to `max_providers` (the ceiling) live providers, independent of demand, supervised by each stem that declares it (a host that does not declare it does not fill it); above the ceiling the hosts ranked beyond `max` among those advertising the band's `prov-shed` mark withdraw (rule `prov.shed`). A convergence target, not a cardinality guarantee. "Provider floor" and "desired floor" mean the presence floor | `first_stem_fleet` |
| <a id="provisioner"></a>Provisioner | native | a host's loop: demand or a presence gap → the catalogue → pull, verify, install → advertise. An ordinary agent on the public API; the library never provisions on its own | `provisioning` |
| <a id="governor"></a>Governor | native | a node-local loop that holds something to a published intent, with no coordinator: membership (by self-election), tuning (a fleet intent wherever a parameter is not pinned locally), timing (a whole set in one pass); the membership and tuning governors act under the node's control profile | `elastic_intent` |
| <a id="self-election"></a>Self-election | native | how the provisioner and the membership governor decide: each eligible node rolls a damped probability every round and acts only on its own roll; nobody assigns the work, and an overshoot is corrected in a later round | `provisioning` |
| <a id="runtime-kind"></a>Runtime kind | native | how an artifact becomes live: `wasm-component` (the node's WASM sandbox) or `blob` (bytes placed for a native runtime already on the node); `[hosts].kinds` lists the kinds a host can run | `reference/unit-file.md` |
| <a id="activation"></a>Activation (`[[activation]]`) | native | how a placed blob reaches its runtime: the stem runs the operator's command, then gates the capability on a probe it keeps re-running; a failed probe withdraws the install. Without `[activation.confine]` it runs with the stem's privileges and is not a sandbox; with it, a declared filesystem and TCP-port allowance enforced by Seatbelt (macOS) or Landlock + seccomp (Linux), and refused by name where it cannot be applied. A confined command starts with a cleared environment. With `[activation.confine.egress]`, hosts reached by name through an egress proxy the stem runs for it on loopback — the list changeable while the stem runs. Confinement narrows what the command can reach; it does not make it safe (post-360 row M, 2.35.0; the egress proxy 2.36.0) | `reference/unit-file.md` |
| <a id="serve"></a>`[[serve]]` | native | a prompt skill over an OpenAI-compatible endpoint, registered while a named install is live and retracted when the install goes | `reference/unit-file.md` |
| <a id="shadow"></a>Proposed · shadow lane · reviewer | native | a `proposed = true` entry loads only as `{ns}/{name}.shadow` (callable by name, takes no demand) until a **reviewer** — a key in that host's `[hosts].trusted_reviewers` — co-signs it with `mycelium-artifact accept`. Reviewers are a separate list from publishers (nothing refuses one key in both) | `provisioning`, guide 16 |
| <a id="wire-check"></a>`wire-check` | native | `mycelium wire-check`: a pure function over unit files that names what could not bind. Its answer is *would bind*, never *is bound* | `reference/unit-file.md` |
| <a id="wiring-report"></a>Static wiring report · observation consumer | native | in `first_stem_fleet`: the `wire-check` report over the three hosts' declarations (what *would* bind), exported, when the example is given a directory, beside one observer's snapshot of who provides `demo/echo` · the example `compare_stem_observations`, which validates that export against `declaration.schema.json` and checks the observed providers against what was declared | `first_stem_fleet`, `examples/compare_stem_observations.rs` |
| <a id="incumbent"></a>Incumbent | native | in `provisioning`'s final wave: the v1 provider already live under `route/optimize` before a proposed v2 is published; it keeps serving the live name while the proposal runs only in the shadow lane | `provisioning` |
| **Authority** | | | |
| <a id="enforcement-point"></a>Enforcement point | native | where authority is checked before work runs — the gateway (`point=gateway`) or the provider check (`point=provider`) — by assembling the action envelope and asking the evaluator. It covers only the routes it fronts | guide 20 |
| <a id="evidence-journal"></a>Evidence journal | native | the node-local, append-only, fsynced, never-gossiped record of what an enforcement point decided and what became of the action; only a hash-bearing reference enters the audit chain. Not a replay bundle | guide 20 |
| <a id="continuation"></a>Continuation · confirmed drain | native | how admitted long-running work continues once its authority may lapse (`mandate::Continuation`: re-check every interval and cancel on failure, or run to completion within a declared limit, which is a bound only where the resource enforces it — `enforced`) · in `authority_drain`, the time from a revocation to the last stop the authority **confirmed**; a stop requested but never confirmed is reported, never counted as stopped | `authority_drain` |
| <a id="history-scope"></a>History scope | native | whose history a count covers, named on every history decision. `HistoryScope::EnforcementPoint` (the default): the decisions this enforcement point's journal recorded at this node (gateway and provider counted apart) — exact here, with no quorum; an actor spread over N gateways can be admitted up to N times a per-gateway limit (post-360 row K, 2.35.0). Two fleet scopes, chosen per rule (row K2): `FleetConsensus` — a counter of seats agreed by consensus, each permit claiming one before it is admitted: exact across gateways, `Indeterminate` without a quorum; `FleetTally` — the sum of every enforcement point's gossiped tally: available without a quorum, approximate (detection, not prevention), and not established when its coverage is not | guide 20 § Deciding on what an actor has already done |
| <a id="credential-binding"></a>Credential binding · placeholder | native | `[[egress_credentials]]`: a secret the node's egress gate attaches only on a request to the bound host (and port and path prefix, when set); a caller holds only the placeholder `mycelium-credential:<name>`. A request anywhere else is refused, and a redirect off the host carries nothing. An `[[activation]]` command names a binding in `[activation.confine.egress] credentials` and sends the placeholder through its stem's egress proxy, which attaches the value at the bound host; the value is never in the command's environment (post-360 row L, 2.35.0; activations 2.36.0) | `operations/crown-jewel.md` § Credentials attached at egress |
| **Words with several meanings** | | | |
| <a id="claim"></a>Things called "claim" | disambiguation | a **knowledge claim** (`RecordKind::Claim`: an issuer asserting something; not evidence) · a **work claim**: a tuple-space worker taking an item from a stage, or the blackboard's competitive claim-by-predicate (one consumer; *work is claimed, not dispatched* means these) · the **log claim** (`clog/{stream}/{group}/claim`: a leased consensus commitment naming a log group's one active consumer) · a **Strands session claim** (`strands-session-mycelium`: a consensus write fixing who writes a message index or record version, made before the write) · a **role claim** (`SignedRoleClaim` at `sys/role/{node}`, RBAC) · a **namespace claim** (a companion owning a KV prefix, such as `tuple/`). None implies another | `design/knowledge-layer.md`, `strands-session-mycelium/README.md` § Claims |
| <a id="data-class"></a>Data class · locality | native | **data class**: the class of the data a routed request carries (`DataClass`: `public`, `confidential`, … in the operator's order), *declared, never detected* — a floor bound to the caller's principal, a client declaration that may raise it and never lower it, or the most restrictive class by default (`ClassSource`). **Locality**: where a destination's work runs relative to the domain (`Locality`: `in_domain` · `external` · `unknown`), the provider's own declaration in its `llm-meta` ad; `unknown` is never read as in-domain. Both reach the evaluator from a routing door, so a policy can say where each class may go (post-360 row R) | guide 20 § Deciding where data may go |
| <a id="scope"></a>Things called "scope" | disambiguation | a **signal scope** (`SignalScope`: `Cluster` · `Group` · `Individual`, who a signal or a consensus proposal addresses) · a **token scope** (`kv:write`, `mesh:serve`, …: what a gateway credential may do) · a **history scope** (`HistoryScope`: whose decisions a history count covers) · a **mandate scope** (`Mandate::scope`: what a mandate covers; epochs are ordered per scope). None implies another | `03-signals.md`, `operations/rbac.md`, guide 20, guide 21 |
| <a id="journal"></a>Things called "journal" | disambiguation | the **node-local journal** (`src/agent/journal.rs`: the mechanism — append-only, fsynced, never gossiped; a full queue is refused, never silently dropped) · the **evidence journal** (its profile for enforcement points; "decision journal" and "authority journal" mean this) · the **rights journal** (the rights ledger's records, on the same mechanism) · the **handover journal** (`HandoverJournal`: what a mandate's successor inherits as history, not conclusions). The **decision trace** is not a journal: it is bounded, drops its newest records when full, and changes no decision | guide 20, guide 21, guide 19 |
| <a id="catalogue-meanings"></a>Things called "catalogue" | disambiguation | the **installable catalogue** (`installable/`: what a stem can install) · the **reviewed catalogue** (the business-activity catalogue an action envelope's `mapping` names, owned by the evidence consumer) · the **federation catalog** (a domain's filtered, signed list of the exports a partner may see) · the **rule catalogue** (every decision point, `reference/rule-catalogue.md`) · the **guarantee catalogue** (every core guarantee, `reference/guarantee-catalogue.md`). Code identifiers spell it `catalog` (`InstallableCatalog`, `CatalogObservation`); prose spells it `catalogue` | `operations/artifacts.md`, `design/action-envelope-ae0.md` §6, guide 17 |
| <a id="admission"></a>Things called "admission" | disambiguation | **peer admission** (mTLS: a node joins a mesh by a certificate from its CA, never by `cluster_name`; a removed member is refused at the handshake) · **signal admission** (a node's `Boundary`: whether it acts on a signal; forwarding is not scoped) · **provider admission** (the provider enforcement point: whether a protected call reaches its handler) · **admission control** (a companion's queue bound: work past the watermark is refused and counted). None implies another | guide 09, `03-signals.md`, guide 20, `operations/admission-control.md` |

**Next:** [01 · Gossip KV](01-gossip-kv.md). For the full design argument behind
this vocabulary, see [philosophy.md](../philosophy.md).

---

## Reference — Skills vs MCP tools: choosing the right primitive

*Moved from the repo README (2026-07-10).*

Mycelium supports two ways to extend what an LLM agent can do. They solve
different problems and compose naturally together.

#### Mental model

> **MCP tool** = a function in the mesh. The LLM calls it to look something
> up, run a calculation, or fetch data. Written in any language.
>
> **Skill** = an LLM agent in the mesh. It has its own identity, prompt, and
> capability declaration. It can be called by any node — including other skills.

#### Comparison

| | MCP Tool | Skill |
|---|---|---|
| What it is | A function registered on a node | An LLM agent node |
| Written in | Any language | TOML manifest — no code |
| Calls an LLM | Optionally | Always |
| Can call other skills | No | Yes — composition |
| Discovered via | `tools/` KV prefix | Capability system (`ns`/`name`) |
| Started with | Any binary / language | `skillrunner --skill manifest.toml` |
| Live chat example | `three_node_demo` — `wiki`, `weather`, `calculate` | `examples/community/` — researcher, writer, orchestrator |
| Guide | [06-tool-discovery.md](06-tool-discovery.md) | [05-skills.md](05-skills.md) |

#### When to use each

Use an **MCP tool** when:
- You need to call an external API (weather, Wikipedia, a database)
- You need deterministic computation (arithmetic, format conversion)
- You want to write the tool in Python, TypeScript, Go, or any language
- The operation is stateless and fast

Use a **Skill** when:
- You need an LLM reasoning step in a pipeline
- You want to compose agents — an orchestrator that calls a researcher that calls a writer
- You want a persistent, named agent role that any node on the mesh can discover and invoke
- You want to scale a reasoning step horizontally (run two researchers; the orchestrator uses both)

#### They compose naturally

The `three_node_demo` LLM node uses MCP tools for external lookups (`wiki`,
`weather`, `sf_lookup`, `book_plot`). The `examples/community/` orchestrator
uses Skills for LLM reasoning steps (`researcher`, `writer`). There is no
conflict — a single planner can have both in scope simultaneously.

---
