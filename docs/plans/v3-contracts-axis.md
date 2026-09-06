# Mycelium v3.0 — the contracts axis: roadmap and implementation plan

**Status:** adopted plan — **approved by the reviewer as the strategic baseline at rev 1.2** · rev 1.3 records their four implementation requirements · rev 1.4 adds §12, the delivery surfaces · **rev 1.5, 2026-09-06** adds item 3's two gates and §13, the composition hypothesis (§11 lists changes) · **Owner:** Mycelium maintainers · **Version of record:** this file
(`docs/plans/v3-contracts-axis.md`); `ROADMAP.md § v3.0` carries the index and points here.

**Provenance.** On 2026-09-05 an external reviewer (a) found five defects in v2.4.1 — three P1 persistence
faults, an unauthenticated tool-invocation surface, and a blocking async path — all fixed and released the same
day (v2.4.2, v2.4.3, `mycelium-py` 0.2.4, `mycelium-ts` 0.1.1); and (b) proposed **six enhancements** for the
next epoch, each with a seven-PR implementation plan. Those six documents are vendored unmodified under
[`docs/plans/external/`](external/) with attribution. This document is *our* plan: what we adopt, what we
decided differently and why, how the six compose, and the order we will build in. Every divergence from the
reviewer's text is marked **⚠ Divergence** and collected in [§7, the decision register](#7-decision-register).

**"v3.0" is a roadmap epoch, not a version.** The released substrate is `mycelium` **2.4.3** (wire v12, PREV 11).
Nothing in this plan changes the wire. **The one compatibility rule, used throughout (rev 1.3):** *ship compatible
additions on 2.x; assess any incompatible public-API or protocol change on its merits* — §4.2 names the one
*protocol* candidate; §9 states the rule for public types. Deliverables ship as companion crates on their own version lines and as
additive core APIs on the 2.x line.

---

## 1. Why this axis, and its posture

### 1.1 The evidence that motivates it
The snapshot/WAL race fixed in v2.4.2 was **deterministic on a single-thread runtime, shipped in every release
with persistence, and invisible to a suite of ~800 tests** — it was found by an external probe. The `>=`
quorum acknowledgement (`set_with_min_acks` counts *any newer* peer write as an ack, and no ack says anything
about disk) is a second overclaim of the same family, still live. `/mcp` invoked any cluster tool *with the
node's own identity* for any unauthenticated caller. The common thread: **the substrate had mechanisms whose
contracts were stated by folklore, not by executable tests.** This axis makes the contracts explicit and gives
the project a way to attack them before a reviewer does.

### 1.2 What it is
Six items, in the reviewer's numbering (kept so the external documents cross-reference cleanly):

| # | Item | One line | Kind | First deliverable |
|---|------|----------|------|-------------------|
| 1 | **Contracts** | typed receipts for what an acknowledgement means; one complete external-effect adapter | core (additive) + companion | contract ADR + regression floor |
| 6 | **Deterministic replay** | run production decision logic under controlled time/RNG/scheduling/storage; replay a failure from a bundle | core seams + `mycelium-sim` | nondeterminism inventory + trace schema |
| 2 | **Federated domains** | a domain = one independently admitted mesh; federation = explicitly exported services over an authenticated edge protocol | companion `mycelium-federation` | domain ADR + two-mesh harness |
| 3 | **Knowledge layer** | claim · observation · assessment · acceptance as attributable records; reader-specific acceptance | companion `mycelium-knowledge` | contract ADR + typed records |
| 4 | **Adaptive stability** | a shared admission contract for every governor; strict budgets as allocated rights | agent-layer interfaces + `mycelium-control` | ADR + actuator inventory + fixture contract |
| 5 | **Scoped mandates** | authority checked by the protected resource, never inferred from a role advertisement | wiki + consensus + companion | ADR + exhaustive wiki mutation-path inventory |
| 7 | **Gateway caller identity** *(rev 1.2, near-term)* | every gateway-originated call carries the client's identity to the provider; the node never acts *as itself* on a client's behalf | core gateway (additive) | `GatewayCaller` context on rpc/scatter/propose/tool-call |
| 8 | **Threat model rev 2** *(rev 1.2, gate)* | foreign principals, authenticated-but-abusive clients, evidence confidentiality, a compromised former holder | `docs/threat-model.md` | one revised document cited by items 2, 3, 5 |

### 1.3 Posture: the rules every item obeys
These are the philosophy's litmus tests applied once, so the six entries do not each re-argue them.

1. **Substrate untouched.** No item conditions signal propagation or KV replication. Core changes are limited to
   (a) *honesty* fixes — what an ack means, what a receipt carries — and (b) *seams* — clock/RNG/filesystem
   injection points. Both are additive on the 2.x line.
2. **Composition before primitives.** Each plan was reconciled toward existing pieces: the durable log verb, leased
   consensus slots and the commit-HLC fencing token, the schema registry, the guardrails strength tiers, the A2A
   and OIDC edges, the egress policy. A new primitive needs a written argument that composition cannot express it.
3. **Prevention is permitted in exactly three shapes — and never taught to Layer I.** The hot invariant is
   *detection, not prevention*. Three items introduce prevention; they are admissible because each is
   (i) **requested by the caller as a contract** (a required-sync write that refuses when persistence is off),
   (ii) **enforced at the resource the caller already trusts** (a mandate checked inside the store's own atomic
   boundary), or (iii) **an opt-in profile with a declared promise strength** (allocated rights, `enforce-allocated`).
   Every prevention mechanism states its strength in the guardrails crate's tier vocabulary
   (`HardPrevention` / `SelfImposedPrevention` / `SelfImposedTransition`).
4. **Expressible ≠ supported.** A contract, a decision, a boundary exists when a CI-tested example or gate exercises
   it. Each item's decisive demonstration ships as a gallery entry at the coop/blackboard bar, or it stays a claim.
5. **Roles evaporate; authority expires; attribution does not.** Every role this axis introduces — gateway, curator,
   holder, observer — carries an expiry, and its authority is recomputed, not inherited, on renewal. Three lifecycles
   are distinct and never conflated: the *permission to issue new statements* (expires with the role), the
   *evidential freshness* of an existing statement (expires on its own validity), and the *historical attribution* of a
   statement (never expires — an observation keeps its provenance after its observer's appointment ends).
6. **A named mechanism is not the composed guarantee.** Consensus, a lease, a CAS, a signature — each proves what it
   proves and no more. Every composed guarantee in this plan is established by an explicit safety argument *and* a
   replay/CI gate, never by the presence of an appropriately named primitive.

### 1.4 How this is the philosophy's own trajectory
The axis is not an import. Item 5's term/epoch split with incumbency rules **is Property 6** (the mandate-TTL
principle) made structural. Item 5's handover journal and item 3's provenance records **are Property 7**
(epistemic symmetry: causal history replicated, not just current values — the failure mode the philosophy says
TTL alone cannot fix). Item 2 is the **subsidiarity table's third row** — the higher layer invoked only for
cross-boundary problems local governance cannot resolve, and *never* by merging the villages. Item 1 is
subsidiarity applied to consistency: *coordinate only where the application's invariant requires it.* Items 4
and 6 are the *legible emergence* commitment carried into control and verification.

### 1.5 What this changes strategically
The v3.0 epoch had one axis — the companion/DX axis (`mycelium-reason`, `mycelium-guardrails`, the pattern
gallery), shipped July 2026. This adds a second, and moves the epoch's centre of gravity from *coverage and DX*
to *contracts and verification*. That is a deliberate response to §1.1. The gallery discipline is unchanged and is
what each item's demonstration must meet.

---

## 2. The map: dependencies and order

| Item | Depends on | Provides to |
|------|-----------|-------------|
| 1 Contracts | — (Phase 0 done: dir fsync, read-back abort, honest WAL acks) | 6 (durability oracle receipt) · 2 (effectful retries) · 3, 5 (durable evidence/proposals) · 4 (actuator receipts) |
| 6 Replay | — | 4 (the combined-feedback harness = replay stage 6) · 5 (scenario B = the decisive handover test) · 3 (clock seam for expiry timers) |
| 2 Domains | 1 (only for effectful profile) | 5 (cross-scope mandates need explicit recognition) · 4 (future allocation scope) |
| 3 Knowledge | 1 (receipts) · 6 (clock seam) | 5 (optional provenance refs) · 4 (optional observation provenance) |
| 4 Stability | 6 (harness) · 1 (action-ID / unknown-outcome conventions) | — |
| 5 Mandates | 1 (receipts) · 6 (scenario B) · 2 (cross-scope, later) | 3 (adoption decisions, later) |
| 7 Gateway caller identity | — (standalone; Phase A) | 2 (the federated-caller adapter is this, across a boundary) · 5 (who invoked a mutation) |
| 8 Threat model rev 2 | — (document; Phase A) | 2, 3, 5 (each PR 1 cites it) |

**Order.** Items **1 and 6 first, together**: one states what must hold, the other attacks it. Then **2** as the
structural investment. **3, 4, 5** follow as companions above the substrate, each once its dependencies' first
releases exist. Within each item, the first two-to-three PRs are the usable release; the rest are gated on
demonstrated semantics.

**Phases with exit gates** (no calendar estimates — the reviewer was right that none is justified before each
ADR):

| Phase | Contents | Exit gate |
|-------|----------|-----------|
| **A** | 1·PR1–3 (ADR, identities + receipts, required local sync) · 6·PR1–4 (inventory, kernel, persistence adapters, WAL/snapshot scenario) · the cooldown parameter (4, standalone) · **7 gateway caller identity** · **8 threat model rev 2** · **V1 the nightly scale runner green** · **V2 on-disk golden fixtures in CI** | typed local durability usable from Rust; the WAL/snapshot race replays from a bundle and its merge-removed witness fails; **item 7's four negative cases + the `authorized_callers` gate pass in CI and the secure profile refuses legacy dispatch**; **item 8 published and cited by items 2/3/5's PR-1 ADRs**; **V1: three consecutive green nightlies (`resilience` + `entries`; `scale` classified)**; **V2: every released WAL/snapshot format replays in CI** |
| **B** | 1·PR4a (exact-identity ack on the existing quorum path) · 1·PR4b (persisted-by-peer protocol) · 2·PR1–3 (domain profile, trust bundles, filtered catalogs) | `set_with_min_acks` acknowledges the *exact* payload only; **a peer that acknowledged persistence holds the record across its own crash/restart** (a crash-before-ack peer does not count); two meshes discover selected exports without merging |
| **C** | 1·PR5–7 (effects companion, tuple-space consumer, SDK parity) · 2·PR4–7 (calls, gateways, partition, example) · 3·PR1–3 · 5·PR1–2 | the adversarial release demos of 1 and 2 pass in CI |
| **D** | 3·PR4–6 · 5·PR3–6 (on replay scenario B) · 4·PR1–5 · 6·PR5–6 | evidence-aware resolution and curator handover both replay deterministically; **item 3's semantic gate: misleading evidence cannot erase a conflicting observation, refresh expired evidence, or confer authority (three replayed negative cases, rev 1.5)** |
| **E** | 3·PR7 · 4·PR6–7 · 5·PR7 · 6·PR7 | combined-feedback scenario green; shadow-mode rollout documented |

*(rev 1.4)* Every phase exit also requires the **§12 alignment gate** for the items it ships: the gallery entry,
the doc-coverage row with no missing cell, the runbook rows, and — at A and C — the re-linted decks.

---

## 3. Item 1 — Contracts

**Adopt.** Four receipts kept strictly separate: *local application* (applied / superseded under LWW) · *local
sync* (the exact operation crossed the local persistence barrier) · *replica sync* (named distinct peers, origin
excluded, persisted that exact operation) · *destination commit* (the destination committed the business change
**and** its dedup result in one transaction). Every operation gets a caller-generated identity before dispatch,
stable across retries and worker replacement; same id + different content is a conflict; a timeout returns partial
evidence or *uncertainty*, never "nothing happened". One effects companion with a transactional reference
destination; one tuple-space consumer with effect recovery; HTTP/py/ts parity. The reviewer's ordering.

**Verified against code (2026-09-05).** `WalMsg::Append { force_sync }` exists since v2.4.2 — PR 3's mechanism is an
API exposure, not a redesign. `kv_quorum.rs::observe` counts `timestamp >= write_ts` from any peer — the overclaim
is real. The snapshot path lacked a directory fsync — fixed 2026-09-05 (#183).

**Sequence.** PR1 ADR + regression floor · PR2 operation identity, typed receipts, failure vocabulary · PR3 required
local sync + retained operation status · **PR4a** exact-identity ack on the existing quorum path · **PR4b**
persisted-by-peer protocol · PR5 effects companion + SQLite reference destination · PR6 tuple-space consumer ·
PR7 gateway/SDK parity. PRs 2–3 are the first usable release.

**⚠ Divergences.**
- **Ordering of the strong path.** Reviewer: persist → sync → apply. We keep v2.4.2's *apply → persist* as the
  invariant and permit persist-first on the strong path **only because the snapshot's WAL-tail merge holds**; the
  ADR states that dependency and a test pins it. *Why:* two write orderings drifting apart is how the race shipped.
  **Rev 1.1 — the application-visible half.** The orderings are not only a snapshot detail: an apply-first operation
  is *observable* (readers, subscribers, effects) before its sync can fail, so a caller receiving a durability
  failure cannot infer that nothing happened. The ADR defines, independently, for each receipt kind: when the value
  becomes visible · when the exact operation becomes durable · whether subscribers or effects may run before
  durability · what remains true after a synchronisation failure. Both orderings may have legitimate contracts;
  they may not silently share one meaning.
- **`persisted` becomes a representational tri-state (item 1, PR 2) — additively (rev 1.3).** Today it reads `true`
  when persistence is unconfigured. Replacing the public `bool` field with an enum is **not** additive under Rust's
  compatibility rules; the migration is a **new** field/result representation (`local_durability: LocalDurability
  { OnDisk, Failed, NotConfigured }`) beside the old one, `persisted` kept and `#[deprecated]` on 2.x, gateway JSON
  gaining the new key beside `"persisted"`, SDKs exposing the new state (they already model absence). Removal is a
  §6.6 ledger entry.
- **PR 4 split.** Reviewer: one "explicit peer persistence service" PR. We split 4a/4b. *Why:* 4a fixes a live overclaim
  on a shipped API in a few lines; 4b is a new protocol (negotiation, authenticated peers, failure-domain metadata,
  status retention) and the bulk of the plan. Pulling 4a forward is the honest priority.
- **Directory fsync is Phase 0, not a caveat.** Done.
- **Reconcile with a prior "declined" decision.** `docs/design/exactly-once-effect.md` declined to extract the
  claim/ack/requeue shape as code, with evidence. The effects companion adds destination-side dedup — related but
  distinct; the ADR must say why this extraction is justified when that one was not.
- **Additions the plan lacked:** a core API to submit a *pre-stamped* update (retries must not tick a fresh HLC);
  a mapping of the existing at-least-once primitives (`emit_reliable`, mailbox, tuple-space lease) into the receipt
  vocabulary; citation of in-tree prior art (the wiki's idempotent bulk ingest; the v2.4.0 exactly-once work-
  distribution proof) stating what a SQLite destination adds.

---

## 4. Item 6 — Deterministic replay

**Adopt.** A `mycelium-sim` harness with three modes — *exploration* (seeded schedules + faults), *exact replay*
(pinned build, stop at first divergence), *scenario replay* (same causal workload against changed code) — because a
seed alone is not a durable reproduction artefact. Production decision logic runs unchanged behind replaceable
adapters: monotonic and wall clocks separately, named RNG streams, scheduling/channel readiness, network, storage
(volatile bytes · durable bytes · directory entries distinct; process death ≠ power loss; write completion ≠ sync),
external work. Failure bundle + minimisation. First scenario: the WAL/snapshot race as a controlled schedule with a
merge-removed witness that must fail; then partitioned ownership handover (item 5's decisive test); then interacting
governors (item 4's harness). The reviewer's ordering.

**Verified.** The storage section already paid for itself: it flagged that the v2.4.2 merge's read-back mapped a read
error to an empty tail — a real data-loss path, fixed the same day (v2.4.3). Entanglement measured: HLC reads
`SystemTime::now()` at one site (a clean seam); `persistence.rs` holds ~25 `tokio::fs` calls in one module (the
right first target); `tasks.rs` ~6 `fastrand` + ~6 timers; `connection.rs` ~9 `Instant::now`;
`membership_governor::decide` and `tuning_governor::gate` are already pure.

**Sequence.** PR1 nondeterminism inventory + coverage map + trace schema · PR2 event kernel + clock/RNG interfaces +
record/replay · PR3 storage/channel adapters + WAL writer seams **+ the static forbidden-call check** · PR4 WAL/snapshot
scenario + fault sweep + witness · PR5 handover (on item 5's real code) · PR6 interacting governors · PR7 CI corpus +
tooling.

**⚠ Divergences.**
- **Inventory additions.** Papaya `compute` CAS retries (the wiki's recurring race family) cannot be represented by a
  one-transition kernel — the coverage map assigns them to Loom/real-thread tests explicitly. `AHashMap` iteration
  order is per-process random except where the store fixes the seed (consensus voter maps iterate randomly).
  Clock injection must reach `causal_now_ms` lease-expiry reads in consensus, not only the HLC.
- **Static check moves from PR 7 to PR 3.** *Why:* without it the seams erode while the harness is built.
- **The witness is a `cfg(test)` toggle**, not a manual edit (the 2026-09-05 fix was verified by hand-disabling the merge).
- **Bundle format ships light first — but sufficient for *exact* reproduction from PR 2**: build identity, configuration,
  initial state (disk images / fixtures), the choices trace (scheduling, time, RNG, faults) and every external input
  are captured or reproducibly derived; boundary-event views and checkpoints follow when a real failure needs them.
  Exact replay **detects divergence** (each requested effect is checked against the recorded next effect) — it never
  merely consumes the same seed.
- **4.2 — the `3.0.0` candidates.** Nothing here changes the wire. The only *protocol* change anywhere in this axis is a
  *later* authenticated, domain-bound SWIM/handshake (item 2's deferred milestone). A substrate major can also be
  triggered by an incompatible *public-API* change; each such change is assessed on its merits under the one rule
  (header), and none is planned — every public change in this axis is designed as a compatible addition with the
  old surface deprecated (§6.6, D24).

---

## 5. Item 2 — Federated domains

**Adopt.** A domain is one independently admitted gossip mesh (its own membership, replication, dissemination,
policy, electorate); federation connects explicitly exported services between meshes over a separate authenticated
protocol and never joins the transports — foreign nodes never enter membership, native `cap/`/`grp/`/`sys/`/
`consensus/`, anti-entropy state or a quorum. `DomainId` + signed `DomainDescriptor` + revisioned `DomainPolicy`;
three trust relationships kept apart (membership · federation identity · service authorization); allowlist catalogs
as attributable per-gateway observations; a distinct `RemoteCapability`; origin preserved to the provider through a
federation-aware adapter; ≥ 2 replaceable gateways with no federation leader; failover only for repeatable exports;
`DeliveryUnknown`; fixed per-gateway quota slots; on disconnect discovery expires and calls fail explicitly, issued
authority lasts only to its expiry, reconnect refreshes before new work and never merges. `cluster_name` stays a label.

**Verified.** SWIM control datagrams are **unauthenticated UDP** (`swim.rs` signs nothing) — "disable SWIM in the enforced
v1 profile" is grounded. Intra-domain admission is a per-node CA `RootCertStore`. `federation_facts.rs` + guide 17 are
the starting point. `FACTS_PREFIX` is the full board the plan says not to export by default.

**Sequence.** PR1 domain ADR + enforced profile + two-mesh harness · PR2 identity, trust bundles, typed policy, vectors ·
PR3 filtered catalogs + remote resolver · PR4 authenticated unary calls + provider adapter · PR5 two-gateway operation,
budgets, outcomes · PR6 partition/reconnect, revocation, rotation · PR7 example, SDKs, diagnostics, docs. Release gate:
the two-mesh demonstration — discover, invoke, lose a gateway, sever every link, keep working locally, change permissions
mid-partition, reconnect, and prove from membership tables, consensus state and traces that the meshes never merged.

**⚠ Divergences.**
- **Compose with the A2A adapter before defining a second call protocol.** `/.well-known/agent.json` + `/a2a` is the
  standards-based inter-agent invocation edge already shipped. Either the federation call *is* A2A with domain-bound
  origin credentials, or the ADR states why `POST /federation/v1/call` must exist. *Why:* two invocation edges with
  different auth models is the drift the gateway-auth fixes just cleaned up.
- **Reuse the OIDC verifier's cryptography — not its trust policy.** `jsonwebtoken`, JWKS refresh and the algorithm
  allowlist are shared; federation objects carry their own issuer, audience, object-type and authority rules, so a
  token valid for one purpose is never valid for another through shared verification code. **Extend
  `EgressPolicy`** (`allow_hosts`) for the source-side rule; **AgentFacts stays the public well-known descriptor** via
  a filtered builder. *Why:* three existing mechanisms, three proposed duplicates — reuse the code, not the trust.
- **No `federation/` KV prefix — as an explicit invariant.** Foreign observations live in the companion's cache. The
  wiki lint's namespace sweep checks it.
- **Our own correction.** We had recorded domains as "the one item likely to touch the wire". Withdrawn: v1 federates over
  HTTPS at gateways (separate roots + SWIM off) and leaves the wire untouched.
- **Process isolation is the example deployment's claim, not the library's.** The plan says so; we hold it in every doc.
- **The NANDA boundary (rev 1.3, D25).** The AgentFacts companion already is Mycelium's NANDA edge: self-certified,
  publicly fetchable, pulled by a quilt, deliberately un-gated, NANDA's field names isolated in one serializer.
  Federation must not become a second discovery edge: the `DomainDescriptor`'s **public** subset is an AgentFacts
  profile (no second well-known), trust bundles stay **bilateral operator configuration** (no registry, no TRS),
  and item 3's assessments reach AgentFacts `certification` only through explicit export projections. NANDA =
  what a domain says about itself, verifiable by any fetcher; federation = who may invoke what, between partners.
  If NANDA later standardises attestations or cross-registry federation, we adapt the projection — edges are
  adopted, not competed with.

---

## 6. Items 3, 4, 5 — the companions above the substrate

### 6.1 Item 3 — Knowledge layer
**Adopt.** `mycelium-knowledge`: four record types — claim · observation · **assessment** (judging is not recording) ·
acceptance decision — as signed immutable records with explicit links (`supports` · `challenges` · `derived_from` ·
`supersedes` · `retracts` · `adopts`); an issuer retracts only its own statements; gossip KV carries bounded signed
discovery heads (`knowledge/head/{issuer}/{stream}`), records and evidence live in an authorized store, so LWW moves a
pointer without erasing competing statements and equivocation is preserved, never HLC-resolved; evidence-aware
resolution wraps `resolve_for_caller` (native gates first, then bind to the exact release, verify/classify, evaluate a
deterministic reader policy, return `Accepted` / `Rejected` / `InsufficientEvidence` / `Conflicted` **with reasons**,
compose with load/locality; evidence never grants what authorization denies); competence stays contextual (no reputation
scalar); identity ≠ independence (reader-configured control groups); missing evidence is uncertainty; expiry and
correction are active with a dependency index; refreshing an advertisement never refreshes evidence. Deferred:
aggregated reputation, inferred independence, mandatory LLM judgment, consensus over truth.

**Verified.** `resolve_for_caller` applies `is_fresh` + schema gates (the wrap point). The capability refresh is the
evaporation lease. `blob.rs` verifies on read behind `llm:read` — the hash is still the credential, so the plan's
opaque-address rule for confidential evidence is necessary. AgentFacts is Ed25519 self-signed; `tls::verify_bytes` is
public; the wiki mints `SectionId`s. **`TraceEvent { hlc, node, kind, detail }` has no parent link** — its "causal story"
is HLC adjacency.

**Status wording.** PR 1's typed records are an *initial API release*, not completion of the knowledge contract; the
contract is complete at PR 6 (the demonstration), and adapters (PR 7) extend it.

**Two gates, not one *(rev 1.5)*.** The knowledge layer's most attractive claim — *substantive disagreement is
preserved while useful coordination continues* — is also its least evidenced, so it gets a **semantic gate** and a
**behavioural experiment**, and they are not confused with each other. *Semantic (a Phase D exit condition, in
CI):* misleading evidence cannot silently erase a conflicting observation, cannot refresh expired evidence, and
cannot confer authority — three negative cases, replayed. *Behavioural (research track, §13):* whether
evidence-sensitive resolution improves outcomes under **misleading self-advertisements, correlated observers, stale
successful histories and contradictory task-specific results**, compared with ordinary `resolve_for_caller` on the
same resources; measured as **errors and opportunity costs** — bad selections, unnecessary refusals, completion
quality, evaluation overhead — because a resolver that rejects everything looks safe while being useless. The
second gate supports a *bounded* empirical claim; it can never establish that evidence-aware selection is always
better, and the plan does not say it is.

**⚠ Divergences.** Reserve `knowledge/` in the namespace table and both front-door lists at PR 1 · the trace adapter adds
`derived_from` links rather than importing HLC order as causation · reuse `schemas/{schema_id}` as the schema *locator*
plus a content digest, not a second registry · expiry timers on the replay clock seam, not `tokio::time` · rank *within*
the accepted class, then hand survivors to the reasoning router's load/reservation ranking (composition order in the
ADR) · the decisive demonstration is a CI gallery entry.

### 6.2 Item 4 — Adaptive stability
**Adopt.** Three promises kept apart — hard bounds (exclusive rights + durable accounting) · stability objectives
(spacing, hysteresis, settling, combined testing) · service objectives (admission control, fair scheduling; **rejected
work reported beside completions**). A `ControlSpec` per governor and the flow observe → propose → reserve → act →
reconcile with stable action IDs and one owner per actuator. `ViewConfidence` made actionable and per-input, driving
policy predicates with the asymmetry: **uncertainty holds speculation and routine scale-down, never protective
shedding or rescue from zero capacity.** Strict budgets as fixed, disjoint allocated rights, counted across
installing/warming/serving/draining/unknown, persisted before acting, never reclaimed because an owner vanished from
discovery. Concrete loop-breaking points. Profiles `legacy` / `observe` / `enforce-local` / `enforce-allocated`, shadow
before enforcement. The reviewer's fixture targets are targets, not guarantees.

**Verified.** `demand.rs` is a declaring-node/provider count (not work). The provisioner self-elects probabilistically
with an `Installing` reservation. `opacity.rs` is a 100 ms loop with a pure decision and the full-channel veto override.
**`ViewConfidence::max_staleness_ms` is 0 when no peers were heard.** **The membership cooldown is
`3 × health_check_interval`, computed once at start** from the immutable config snapshot, and `converge` checks
elapsed monotonic time against that fixed `Duration`. **Rev 1.1 correction:** rev 1.0 claimed a timing intent that
shortens the interval shortens the cooldown *live*; it does not — the timing governor writes `ctx.hot`, which this
governor never reads. What is true: the cooldown *derives* from the health-check interval at configuration time (a
coupling worth removing), and this governor ignores live timing intents altogether (its tick is also fixed at start),
which is a separate consistency gap against the timing governor's "live re-timing" claim.

**⚠ Divergences.** Promise strength stated in the **guardrails tier vocabulary**, not a new one (a self-enforced budget is
Tier A; a fleet ceiling holds only with exclusive rights) · the workload probe **consumes the companions' existing depth
signals** (`TupleSpace::depth`, `Blackboard::depth`, KV-ring stages) before a new metric · the combined-feedback harness
**is replay stage 6**, built once, reusing the governors' pure decision functions · **make the cooldown an explicit parameter with a declared bound** (an absolute `Duration`, not a multiple of the
health-check interval), and decide in the same change whether this governor honours live timing intents — small,
standalone, gateable now · give staleness a
"no observation" state (`staleness_known`) · one owner per deficit named in the ADR · detection-not-prevention for
everything except the rights ledger.

### 6.3 Item 5 — Scoped mandates
**Adopt.** A common mandate contract (holder, establishing authority, purpose, scope, enumerated operations, **authority
epoch** and a separate **term identity**, validity, renewal/revocation/outstanding-operation policies, provenance) shared
by curator / primary / proposer without shared powers. **CAS ≠ authorization**: the wiki's section/manifest CAS defeats
stale content, but a former curator who re-reads fresh content passes it, so every canonical mutation checks *both*
current revision and current mandate, and a stale mandate is `MandateSuperseded`, never a `Conflict` fed to the retry
loop. Every mutation path protected (apply, bulk ingest, erase, bootstrap, imports, admin, raw credentials). The three
lifecycle events recorded separately: role expiry · permission withdrawal · outstanding-operation invalidation. A
handover journal the successor inherits **as history, not as conclusions**, behind a readiness gate. Incumbency rules
(consecutive terms, cumulative tenure, cooling-off, eligibility, affiliated principals). Fail-closed authority restart.
The reviewer's partition table and decisive test.

**Verified.** The curator's write entitlement is `is_curator: AtomicBool` — the indicted inference. Store CAS returns
`Conflict` on version only. `GitStore` serialises through `update-ref` CAS + push under a single-writer assumption.
Proposals are **evaporating KV** — a delivery hint. `LockService`'s fencing token is the commit HLC (#166).

**⚠ Divergences — including the one architectural disagreement of the axis.**
- **The decisive invariant (rev 1.1, adopted verbatim from the reviewer's response):** *once the protected resource
  acknowledges installation of epoch E2, no operation authorized only under E1 can commit there — even if its holder
  refreshes the content revision, retries, reconnects or restarts.* Every mechanism below is judged against this
  sentence, and the partition policy follows from it: a disconnected curator may prepare proposals but cannot promise
  canonical acceptance without reaching the enforcing resource.
- **No resource-authoritative service process.** Reviewer: v1 puts the mandate and the canonical commit in a new
  SQLite-backed daemon per wiki scope. That is a control plane for the scope — *"No daemon, no orchestrator, no control
  plane"* (philosophy § Not a platform). We put the check **inside the canonical store's own atomic boundary**: for
  `GitStore` a `refs/mycelium/mandate/{group}` object updated under the *same* `update-ref` CAS as content, and on the
  shared remote a **pre-receive hook** verifying the signed epoch (that remote already is the external authority in that
  deployment); for `FsStore` the epoch in the same mutator critical section. A SQLite service is acceptable **only as an
  application-owned reference resource** (the effects companion's destination shape). *Why:* the reviewer's own
  criterion — "the resource must enforce" — is met by the store that already serialises the bytes; adding a process to
  hold the truth is the coordinator arriving through the side door. **Rev 1.1 — what "inside the boundary" must mean,
  per mechanism (the reviewer's qualification, accepted):** *(i) `GitStore`:* the mandate check and the content commit
  are **one git ref transaction** — `git update-ref --stdin` (`start` / `prepare` / `commit`) updating
  `refs/mycelium/mandate/{group}` and the content ref together, each with its expected old value, so two separately
  successful CAS operations can never interleave; *(ii) the shared remote (rev 1.3 — precise):* the curator pushes with **`git push --atomic`** so the remote updates
  all requested refs or none, and asserts the mandate ref's current value with
  **`--force-with-lease=refs/mycelium/mandate/{group}:<expected>`** on **every** push — including ordinary content
  writes that leave the mandate unchanged — so the mandate check is part of the same remote ref transaction as the
  content update, not an earlier hook-time read; the **pre-receive hook** additionally verifies the signed epoch and
  rejects the whole atomic push on any mismatch. **Fail closed:** a remote that does not honour atomic pushes (the
  client learns this from the push result) is not a supported strict-profile remote; the write is refused, never
  downgraded to a non-atomic push. Locally, every content transaction carries `verify refs/mycelium/mandate/{group}
  <expected>` in the same `update-ref --stdin` transaction, so the unchanged mandate is still checked *through
  commit*, not before it; *(iii)
  `FsStore`:* its mutator `Mutex` is per store instance and does **not** serialise separate processes, so the strict
  profile on `FsStore` is **out of scope** unless an OS-level exclusive lock is added; legacy semantics are declared,
  not implied. The replay gate (scenario B) covers competing appointments, delayed holders, resource restart, expiry,
  and revocation with no subsequent content write.
- **Establishment via the substrate's own consensus — conditional (rev 1.1).** A mandate *may* be a **leased consensus
  slot** `mandate/{scope}` whose committed value is (holder, epoch), the commit HLC as epoch, `committed_lease_secs` as
  term — **only after** its safety assumptions (membership, quorum overlap, durable state) and the resource's
  installation protocol are shown to satisfy the handover contract under the replay gate. Until then **owner-authorized
  appointment is the supported baseline.** *Why:* the consensus module's own commentary describes near-simultaneous
  optimistic commitments that later converge on a holder; eventually agreeing on a holder does not prove that
  conflicting holders could never both act, and a commit HLC orders epochs without proving its bearer was authorized
  to establish one. D2 is therefore conditional on D4.
- **Durable proposals via the existing log verb** (`KvHandle::append` → `log/wiki/{group}/proposals`) plus item 1's
  receipts — not a service database; the evaporating queue becomes the discovery hint the plan wants.
- **Do not build a second fence beside `LockService`.** The plan declines to certify its converged-view issuance; we audit
  it under the replay harness (scenario B) before certifying or replacing.
- **The decisive test is replay scenario B** — built once, there.
- Reserve `mandate/` + `log/wiki/` at PR 1 · hold claims at "enforces configured eligibility rules".

### 6.4 Item 7 — Gateway caller identity inside a domain *(rev 1.2)*
**Why it is missing from the six.** The `/mcp` finding (fixed 2026-09-05) was a confused deputy: the gateway dispatched
`tools/call` *as the node*. That class is not specific to MCP. Every gateway-originated RPC, scatter, proposal or
tool call from a Python/TypeScript client runs under the node's identity, so provider-side `authorized_callers`
sees the node and never the client. Item 2's `FederatedCaller` adapter solves this across a domain boundary;
nothing solves it inside one. **Adopt (rev 1.3 — the contract):** a `GatewayCaller` context carried from the auth middleware to the provider on
every gateway dispatch path, distinguishing **three things a provider can verify**: the *originating principal*
(who the client is — the resolved bearer/scoped-token/OIDC principal, never a client-supplied string), the *gateway
acting on its behalf* (this node's identity, bound in), and the *authority granted for this request* (the scopes
the resolved credential actually holds, intersected with what the route needs). The context is **constructed only
by the auth layer** and is attested to the provider by the node's identity over the request digest — a struct
containing a principal name and a digest, supplied by anyone else, is not evidence. The node's own identity is used
only for the node's own actions. **Secure profile rejects:** a client-supplied (forged) caller context · a missing
context falling back to the node's identity · a gateway asserting more scope than the client's credential holds ·
an older provider that cannot enforce the context (the call is refused, not silently run as the node). **Legacy
node-as-caller dispatch** stays available only under an explicit `legacy` profile and is a §6.6 removal-ledger
entry — compatibility never silently preserves impersonation in the secure profile. Small, standalone, Phase A;
the precursor to item 2's adapter. **Gates:** a provider restricting `authorized_callers` rejects a gateway client
outside the list although the node is listed; each of the four negative cases above is a CI test.

### 6.5 Item 8 — Threat model rev 2 *(rev 1.2)*
`docs/threat-model.md` models an admitted mesh with cooperative members (the crown-jewel work). Items 2, 3 and 5
each state a threat model in prose — foreign principals and authenticated-but-abusive clients (2), evidence
confidentiality and hash-as-credential (3), a compromised former holder and a forged epoch (5) — with no single
document they cite. **Adopt:** one revision of `docs/threat-model.md` covering the three, written in Phase A and
cited by each item's PR 1 ADR. Not code; a gate.

### 6.6 The `3.0.0` removal ledger *(rev 1.2)*
"Additive only on 2.x" is a slogan until the list of what a substrate major would remove exists. Seeded now,
maintained in `ROADMAP.md`:

| Candidate for removal at `3.0.0` | Since | Replacement |
|---|---|---|
| `system_propose` (`#[deprecated]` alias) | 2.1.0 | `cluster_propose` |
| `cluster_name` as a cosmetic label with no isolation | — | item 2's `DomainId` (the label may stay as a display name) |
| the inferred `>=` acknowledgement in `set_with_min_acks` | — | item 1 PR 4a's exact-identity ack (kept behind a legacy flag until then) |
| `ConsensusResult::Committed { persisted: bool }` | 2.4.2 | the D24 tri-state |
| `GatewayAgent`-as-caller dispatch (the node acting for gateway clients) | — | item 7's `GatewayCaller` |

None of these *requires* `3.0.0`; each is additive-with-deprecation on 2.x. The ledger exists so the major, if
D23's trigger ever fires, is a removal of announced things and nothing else.

---

## 7. Decision register

Every place this plan departs from the reviewer's six documents. "Kept" means we adopt their text; the rest are ours.

| # | Item | Reviewer proposed | We decided | Why |
|---|------|-------------------|------------|-----|
| D1 | 5 | A resource-authoritative **service process** (SQLite daemon) holds mandates + canonical commits | Fence **inside the canonical store's atomic boundary** (mandate ref under the same `update-ref` CAS; pre-receive hook on the shared remote; epoch in `FsStore`'s mutator section). SQLite only as an application-owned reference resource | *Not a platform*: no daemon, no control plane. The store already serialises the bytes; put the fence where they serialise |
| D2 | 5 | Owner-signed appointment as the establishment mechanism | **Rev 1.1:** owner-authorized appointment is the **baseline**; a leased consensus slot becomes the establishment mechanism **only after** D4's replay gate + an explicit safety argument (membership, quorum overlap, durable state) | A commit HLC orders epochs; it does not prove its bearer was authorized. Conditional on D4 |
| D3 | 5 | A durable proposal journal in the authority service | `KvHandle::append` log stream + item-1 receipts | Composition over a new store |
| D4 | 5 | The lock service's issuance is "insufficient" for strict mode; build the new fence | Audit `LockService` under replay scenario B first; no second fence | Two fences beside each other is how guarantees drift |
| D5 | 2 | A new `POST /federation/v1/call` protocol | Compose with the shipped **A2A** edge, or write the ADR argument for a second protocol | Two invocation edges with different auth models is the drift we just cleaned up |
| D6 | 2 | New JWS / signed-object stack; new egress rule; new public descriptor | Reuse the **OIDC verifier's cryptography** (not its trust policy — federation objects carry their own issuer/audience/type/authority rules), extend **`EgressPolicy`**, keep **AgentFacts** as the public descriptor via a filtered builder | Reuse the code, never the trust |
| D7 | 2 | (implicit) foreign observations disseminated in KV later | **No `federation/` KV prefix** as an explicit, lint-checked invariant | Foreign state never enters the medium |
| D8 | 1 | Strong path: persist → sync → apply | Keep **apply → persist** as the invariant; persist-first permitted only because the WAL-tail merge holds; dependency pinned by a test. **Rev 1.1:** the ADR defines visibility, durability, pre-durability effects and post-failure truth *independently* per receipt kind | The race shipped because two orderings drifted; the orderings are also application-visible |
| D9 | 1 | One PR for the peer-persistence service | **Split 4a/4b**; 4a (exact-identity ack) pulled forward | 4a fixes a live overclaim cheaply; 4b is a protocol |
| D10 | 1 | Directory fsync noted as a caveat | **Phase 0 fix** — done 2026-09-05 | A few lines; the difference between a power-loss claim we can and cannot make |
| D11 | 1 | (absent) | Reconcile with `exactly-once-effect.md`'s **declined** extraction; add pre-stamped updates; map `emit_reliable` / mailbox / tuple lease into the vocabulary; cite in-tree prior art | Two design records must not contradict; retries must not tick a fresh HLC |
| D12 | 6 | Static forbidden-call check in PR 7 | **PR 3** | Seams erode while the harness is built |
| D13 | 6 | (absent from inventory) | Add **papaya CAS retries** (owned by Loom), **`AHashMap` iteration order**, **lease-expiry clock reads** | Real nondeterminism sources in the covered paths |
| D14 | 6 | Full failure-bundle format from the start | Minimum bundle **sufficient for exact reproduction** from PR 2 (build, config, initial state, choices, external inputs) with divergence detection; event views/checkpoints later | Ship the reproducible core early — but reproducible, not merely re-seeded |
| D15 | 3 | (absent) | Reserve `knowledge/`; reuse `schemas/` as the schema locator; **trace `derived_from` links** rather than HLC order as causation | The trace has no parent link today; HLC adjacency is not causality |
| D16 | 3 | A second ranker in the resolver | Rank **within the accepted class**, then hand survivors to the reasoning router | Two rankers must compose in one stated order |
| D17 | 4 | A new promise-strength vocabulary | The **guardrails tiers** | One vocabulary for "how strong is this promise" |
| D18 | 4 | A new `WorkloadProbe` metric | Consume the companions' **depth** signals first | Backlog is already measured where work queues |
| D19 | 4 | A bespoke combined-feedback harness | **Replay stage 6**, built once | One harness, one clock seam |
| D20 | 4 | Cooldown coupling listed as a design concern | **Rev 1.1:** an explicit cooldown parameter with a declared bound, decoupled from the health-check interval; decide whether the governor honours live timing intents | Small, standalone, gateable. *Rev 1.0 overstated this as a live runtime feedback defect — corrected, see §8* |
| D21 | 4·6 | Combined tests as their own rigs | Every item's decisive demonstration is a **CI gallery entry** | *Expressible ≠ supported* |
| D22 | all | Six separate seven-PR plans | **One plan, one dependency graph, one posture** (this document); the six kept vendored for attribution | The six overlap at receipts, clock, harness, namespaces — one order avoids six orderings |
| D23 | 2 | ("4.0", "5.0" epochs) | Roadmap **epochs, not versions**. **Rev 1.3, one rule:** ship compatible additions on 2.x; assess any incompatible public-API or protocol change on its merits — the one *protocol* candidate is a later authenticated domain-bound SWIM; public-type changes are designed additive (D24) | Version numbers follow breaking changes, not ambitions; and "additive" is Rust's definition, not ours |
| D24 | 1 | `persisted: bool` documented as "true = no promise broken" when unconfigured | A **new** `local_durability` representation beside the old field, `persisted` deprecated on 2.x, removal in the §6.6 ledger (rev 1.3: an enum-for-bool swap is not additive) | Documentation is inadequate for a contracts programme; Rust's compatibility rules define "additive" |
| D25 | 2 | A second public well-known descriptor (`/.well-known/mycelium-domain`) and trust bundles | **NANDA stays the public discovery edge** (AgentFacts, self-certified, pull); federation is the authenticated export-and-invoke edge; the descriptor's public subset is an **AgentFacts profile** through the existing serializer; **no second well-known, no registry, no TRS**; "trust is the fetcher's decision" holds on both sides; assessments reach AgentFacts `certification` only through explicit projections | Two public descriptors and a trust index are how federation leaks into NANDA space |
| D26 | 5 | "both refs in the same push" | **`git push --atomic` + `--force-with-lease=<mandate-ref>:<expected>` on every push, incl. ordinary content writes; fail closed on a non-atomic remote; local `verify` of the mandate ref inside every `update-ref` transaction** | An ordinary push is not atomic; a hook-time read is not a check through commit (the reviewer's requirement) |
| D28 *(rev 1.5)* | beyond | A *commitment* subsystem as the central application abstraction; "the system constructs coordination arrangements" | A commitment is a **composition** of five existing records (requirement · acceptance · mandate/allocation · receipt · assessment) plus a correlation record; obligations arise only by authorized acceptance; no planner — **agreed with the reviewer after one exchange** (§13.2) | *Composition before primitives*; the planner is the ceremony the philosophy strips |
| D27 | 7 | (a struct with principal + digest) | The context is **constructed only by the auth layer and attested by the node over the request digest**; four negative cases in CI; legacy node-as-caller only under `legacy`, never in the secure profile | Receiving a struct is not verifying a relationship |

**Kept without change:** the four-receipt vocabulary; the three trust relationships; the four record types; the three
lifecycle events; term ≠ epoch; fixed allocated rights never reclaimed on disappearance; the asymmetric uncertainty rule;
`DeliveryUnknown`; every "this proves X, not Y" honesty clause; every deferred list.

---

## 8. Corrections to our own claims (2026-09-05)
- We wrote that domains were "the one item likely to touch the wire — the first honest `3.0.0` trigger". Withdrawn (D23).
- Two sentences we wrote the same morning claimed a plain `set_async` surfaces a WAL error as an `Err`. It does not — its
  `bool` is the gossip-queue result and the WAL error was silently discarded. Now logged at `warn`; the receipt is item 1.
- Our `persisted` flag reads `true` when persistence is unconfigured ("no promise broken"). The SDKs model absence as
  `None`/`null`; the Rust type gains a `NotConfigured` state in item 1 PR 2 (D24).
- **(2026-09-06)** Rev 1.0 §6.2 claimed the membership cooldown shortens *live* when a timing intent shortens the
  health-check interval. False: both are fixed at start from the config snapshot; the governor never reads the hot
  timing value. Corrected in §6.2 and D20; the ROADMAP row and the wiki carry dated corrections. The reviewer found it.

---

## 9. Cross-cutting rules
- **Namespaces reserved at each item's PR 1**, in `src/lib.rs` *and both* front-door lists: `knowledge/`, `mandate/`,
  `log/wiki/`; **explicitly none** for federation.
- **Versioning (rev 1.3, the one rule).** Ship compatible additions on 2.x — "compatible" by Rust's rules, so a public field or type never changes shape: a new representation is added beside the old, which is deprecated, and removal goes to the §6.6 ledger; assess any incompatible public-API or protocol change on its merits (the one protocol candidate is D23's). Companions on their own lines.
- **Every gate is a test in CI** without a live node where possible (the day's pattern: stub servers, fetch recorders,
  in-process writers); Docker suites for the two-mesh and combined scenarios.
- **Documentation ingest**: each PR updates the wiki page it touches and adds a dated `.log/` entry; the wiki lint's
  doc-vs-code sweep and the doc-coverage must-work rule apply to every instruction this plan adds.
- **The parity gate *(rev 1.2)*:** no gateway change ships without the Python and TypeScript SDKs and the operator
  docs in the **same PR**. The SDKs lagged core twice on 2026-09-05 (bearer support; the `persisted` field), both
  found by audits, neither by a rule.
- **The public surface as code *(rev 1.2, tightened 1.3)*:** one routing-defined list of public routes
  (`pub fn public_routes()`) with a test that diffs it against `docs/operations/rbac.md` and the wiki security page
  **and** an executable authentication test per route class (401 without a bearer on every non-public route; never
  401 on the public set — `regression_node_level_routes_require_bearer_when_token_set` is the seed). Code and
  documentation can agree on a wrong classification; only the executable test says what the router does.
- **Secrets never become observability *(rev 1.3)*:** `GatewayCaller` carries a resolved principal and granted
  scopes — **never the reusable credential itself**; replay bundles capture external inputs **redacted of bearer
  credentials and keys**, with a protected-artefact class for anything that cannot be redacted and still reproduce;
  audit and trace records carry verified claims and scoped attestations, not tokens. **Item 8 (threat model rev 2)
  specifies** verified claims, scoped attestations, redaction rules and protected reproduction artefacts — the
  identity and replay features both depend on it.
- **Every implementation PR states, in its description *(rev 1.3, the reviewer's discipline)*:** the guarantee ·
  its assumptions · the enforcing component · the failure behaviour · **the test that would detect its violation**.
  A PR without the five is not ready for review.
- **Verification infrastructure is a named line *(rev 1.2; owners + gates 1.3)*:** the nightly scale runner (100-node, resilience,
  entry-volume) is owned here — it has failed in its Docker image build for days and Scalability has carried a
  stale score for many runs; items 2 and 4 make scale claims that need it green. Plus **on-disk golden fixtures**
  (WAL + snapshot files from each released format) replayed in CI, since item 1 will change the format and today
  nothing tests that an old file still replays. **Owners and gates (rev 1.3):** V1 the nightly runner — owner: the
  maintainer running `scripts/launchd/`; gate: three consecutive green `resilience` + `entries` nightlies with the
  `scale` row classified, a Phase A exit condition. V2 golden fixtures — owner: item 1's PR 1; gate: a CI job that
  replays a fixture from every released on-disk format, a Phase A exit condition.
- **The research track is cross-linked *(rev 1.2; §13 rev 1.5)*:** the replay harness (6) is a reproducible-experiment engine and
  the combined-feedback scenario (4) is a case study for the three-arm work-distribution paper, the way the council
  substrate already doubles as Paper 1's case study — planned once, cited from `docs/wiki/domain/publications.md`.
- **Deferred, by decision:** a policy DSL; automatic algorithm selection; distributed transactions; aggregated
  reputation; inferred observer independence; mandatory LLM judgment; consensus over truth; authenticated SWIM;
  transitive federation; dynamic quota transfer; delegation of mandates; replicated authority.

## 10. Immediate next steps
1. **Item 1, PR 1** — the contract ADR (with D8, D11) and the regression floor. *(This unblocks everything.)*
2. **Item 6, PR 1** — the nondeterminism inventory with D13 and the trace schema.
3. **Item 4's cooldown fix** — standalone, this week.
4. **Item 1, PR 4a** — the exact-identity ack; fixes the live `>=` overclaim.
5. Item 2's ADR (D5–D7) in parallel, since it has no code dependency on 1 or 6 for its discovery release.
6. **Item 7 (gateway caller identity)** — small, standalone, closes a live security-design gap *(rev 1.2)*.
7. **Item 8 (threat model rev 2)** — a document; gates items 2, 3, 5 *(rev 1.2)*.
8. **Fix the nightly scale runner's image-build deadline** — nothing scale-related can be evidenced until it is green.
9. **Phase A's §12 lines** *(rev 1.4)*: the philosophy revision and the concepts-chapter vocabulary travel in item 1's
   ADR PR; the receipt-ladder example is item 1 PR 3's gate; the core deck's federation and PAIR slides are re-linted
   at the Phase A exit.
10. **Done 2026-09-06:** `set_with_min_acks` documented honestly at all seven sites (rustdoc, both SDK READMEs and
   docstrings, two guides) — the reviewer's "document its actual semantics immediately".

## 11. Revision log
- **rev 1.5 (2026-09-06)** — from the reviewer's post-axis assessment and our exchange on it: **item 3 gets two
  gates** (a semantic gate in Phase D's exit — misleading evidence cannot erase a conflicting observation, refresh
  expired evidence or confer authority; and a behavioural experiment on the research track measuring errors *and*
  opportunity costs against ordinary resolution); **§13** records the agreed position — the axis' novelty is the
  composition, not a primitive; correctness work is not novelty; the positioning sentence; the prior-art set incl.
  the OSGi/Paremus lineage; a **commitment is a composition of five records, not a subsystem** (**D28**, the reviewer
  withdrew the planner); the composition hypothesis with the four-arm instrument, run only when items 3, 4, 5 exist;
  the co-op supply-disruption demonstration; replay's counterfactual limit. Scheduled inside phases A–E: only the
  semantic gate.
- **rev 1.4 (2026-09-06)** — **§12 Delivery surfaces**, from the maintainer's requirement that the roadmap include
  compelling examples (refresh + new), developer and operations documentation re-alignment, and both presentations:
  six lines (S1 examples — one decisive demonstration per item plus the refresh of every ack-showing example; S2 dev
  docs — concepts with each ADR, how-to chapters with each release gate, wiki pages, the audit tools' own
  inventories; S3 ops runbooks in the same PR as the knob; S4 the core and customer decks at Phase A and C exits
  under `/publication-lint`; S5 the philosophy revision with item 1's ADR; S6 front door, companion onboarding
  checklist, migration notes per deprecation, a Phase-C adversarial self-audit + fuzz targets, the analysis series,
  a research candidate). Each gated by an existing mechanism; **no phase exit while its §12 lines are open** (§2).
  No decision-register change — the reviewer's plans contain no documentation, example, or presentation lines;
  §12 is ours.
- **rev 1.3 (2026-09-06)** — the reviewer approved rev 1.2 as the strategic baseline with four implementation
  requirements, now recorded: **D26** atomic remote enforcement incl. ordinary writes (`--atomic`,
  `--force-with-lease` on the mandate ref, fail closed; local `verify` through commit); **one compatibility rule**
  (header, §4.2, §9, D23) and **D24 made genuinely additive** (new representation + deprecation, not an enum-for-bool
  swap); **Phase A's exit gate** now carries items 7 and 8 and the two verification lines (V1 nightly runner,
  V2 golden fixtures) with owners and gates; the public-surface rule requires **executable** auth tests, not doc
  agreement; a **secrets-never-become-observability** rule tying `GatewayCaller` and replay bundles to item 8;
  **D27** — item 7's contract (originating principal · gateway acting · authority for this request; auth-layer
  constructed, node-attested; four negative cases; legacy impersonation only under `legacy`); the **per-PR five-part
  statement**. Also **D25** (NANDA boundary, from our own question). Scope otherwise stable; the next action is
  unchanged: contracts ADR + replay foundation, with item 7 as immediate security work.
- **rev 1.2 (2026-09-06)** — six additions from our own 360 review after rev 1.1: items **7** (gateway caller
  identity inside a domain) and **8** (threat model rev 2) as near-term Phase-A work; the **`3.0.0` removal
  ledger** (§6.6); the **parity gate** and **public-surface-as-code** rules; **verification infrastructure** named
  (nightly runner, on-disk golden fixtures); the **research-track** cross-link. And the `set_with_min_acks`
  wording corrected at seven sites (not a plan change — a live doc overclaim the plan had deferred to PR 1).
- **rev 1.1 (2026-09-06)** — after the reviewer's response to rev 1.0. Adopted: the decisive mandate invariant and the
  per-mechanism transaction specification (§6.3, D1); D2 made conditional on D4; D8's application-visible
  visibility/durability contract; D24 (`persisted` tri-state); D6 crypto-not-trust; D14 minimum-bundle sufficiency +
  divergence detection; posture rules 5 (attribution outlives authority) and 6 (a named mechanism is not the composed
  guarantee); Phase B's peer-durability crash/restart gate; item 3's "initial API release" wording. **Corrected:** the
  live cooldown coupling (§6.2, D20, §8). The reviewer's closing risk — *treating the existence of an appropriately named
  mechanism as evidence that the stronger composed guarantee follows* — is now posture rule 6.
- **rev 1.0 (2026-09-05)** — consolidation of the six entries into one plan.

## 12. Delivery surfaces — examples, documentation, presentations *(rev 1.4)*

The reviewer's six plans are engineering plans; rev 1.0–1.3 kept that shape. That is a gap for a programme whose
posture is *expressible ≠ supported*: a contract nobody can run, read about, or see demonstrated is a claim. This
section names the **non-code deliverables** of the axis, ties each to the phase that produces the thing it
describes, and gates each with a mechanism the repository already has — the three lint skills (`/wiki-lint`,
`/doc-coverage`, `/publication-lint`) and the analysis series — rather than a new process. **Rule: no phase exit
is declared while its §12 lines are open.** The §2 table's exit gates are engineering gates; §12 is the
alignment gate that sits beside every one of them.

| Line | What ships | When | Gate (existing mechanism) |
|------|-----------|------|---------------------------|
| **S1 Examples** — refresh + new | See §12.1 | each item's decisive demonstration lands **before** that item's release gate is called met | `examples/README.md` matrix row · CI-run · `__CONCEPTS__` + Ops Console link for browser examples ([UI-example contract](../wiki/dev/ui-example-contract.md)) |
| **S2 Dev documentation** | See §12.2 | concept vocabulary with each item's PR 1 (the ADR); the how-to chapter with its release gate | `/doc-coverage` — a new matrix row per concept, no ✗ cell at the phase exit; `/wiki-lint` doc-vs-code clean |
| **S3 Operations documentation** | See §12.3 | runbook rows in the **same PR** as the operator-facing change (the parity gate, §9, already says so for gateway changes — this extends it to every operator knob) | `/doc-coverage` Ops cells; the must-work rule (an instruction that silently no-ops is *Thin*) |
| **S4 Presentations** — core + customer | See §12.4 | at **Phase A** exit (first shipped tranche) and **Phase C** exit (federation + effects demos); re-linted at every companion release | `/publication-lint` clean — every "shipped" claim cites a tag; roadmap items carry their phase status |
| **S5 Philosophy** | See §12.5 | with item 1's ADR (Phase A) | `/publication-lint` (the philosophy is a lint target); the analysis series' dimension 1 |
| **S6 The rest** | See §12.6 | as listed | as listed |

### 12.1 Examples — the gallery is the proof

*Refresh existing.* Every example that calls a verb whose ack changes meaning under item 1 shows the receipt it
gets back — the zero-setup ladder (`hello_mesh`, `distributed_lock`), `three_node_demo`, the coop suite's
consensus and mailbox demos, the LangGraph checkpointer ladder (chapter 15), and the Ops Console (an operator's
first contact with `persisted` / `local_durability`). Refresh is not decoration: an example that keeps printing
"ok" for a write the receipt says was *not* durable is an overclaim in code.

*New, one per item — the decisive demonstration, at the coop/blackboard bar (README block per the examples doc
template, constructive domain, CI-run):*

| Item | Demonstration | Kind |
|------|---------------|------|
| 1 Contracts | the **receipt ladder**: the same write under `none` / `local` / `required-sync` / peer-persisted, then a peer crash — which receipts survive, which were only claims | CLI + Docker (crash) |
| 6 Replay | **replay a bundle**: the WAL/snapshot race captured as a bundle, replayed to the failing witness, then the fix replayed green | CLI (`mycelium-sim`) |
| 2 Domains | **two meshes, one exported service**: discovery without merging, a call across the edge, a partition, a revoked gateway — as a `*_viz` browser example so the boundary is *visible* | browser + Docker |
| 3 Knowledge | in the coop world: "which pantry is really open" — claim, observation, assessment, and two readers with different acceptance policies resolving differently from the same evidence | CLI |
| 4 Stability | the **control-envelope viz**: allocated rights and budgets under load, the `enforce-allocated` profile versus advisory, and the combined-feedback scenario | browser |
| 5 Mandates | **curator handover** in the council substrate: appointment, an atomic-enforced write, expiry, revocation mid-write, attribution surviving the handover | CLI over `GitStore` |

The Ops Console gains a panel per shipped concept (receipts, domains, mandates) — it is the operator's demo
surface and the UI-example contract's consumer.

### 12.2 Developer documentation

- **Concepts first.** `docs/guide/00-concepts.md` gains the axis vocabulary with each item's ADR: receipt kinds and
  `DeliveryUnknown` (1); the seams and what a bundle is (6); *domain* versus the NANDA discovery edge (2, D25);
  claim/observation/assessment/acceptance (3); allocated rights and promise strength (4); mandate, term, epoch (5);
  gateway caller identity (7). `building-on-mycelium.md` and the FAQ get the reserved prefixes and the one
  compatibility rule the day they are reserved (§9).
- **How-to chapters** with each item's release gate: a **contracts & receipts** chapter (the receipt ladder as its
  worked example); a **replay & simulation** chapter; chapter 17 (federation) restructured into *public discovery*
  (AgentFacts, unchanged) and **federated domains** (new — trust bundles, exported services, the edge protocol);
  **knowledge**, **stability & control**, and **mandates** chapters for the companions; `error-handling.md` gains
  the receipt outcomes; the cookbook gains one recipe per item.
- **SDK docs are part of the parity gate** already; this adds the *narrative* side: the Python and TypeScript
  READMEs and the LangGraph chapter show receipts, not just carry the field.
- **The wiki** gets a page per new mechanism (contracts under architecture; replay under testing; domains and the
  threat model under security; the four companions under companions) and `AGENTS.md` routing rules for them,
  ingested with the PR that ships the mechanism — the §9 ingest rule, restated because it is where rev 1.0–1.3
  stopped.
- **`CLAUDE.md`** hot invariants gain the ack-semantics rule (what a receipt proves) and the seam rule (no direct
  clock/RNG/filesystem access in production logic once item 6's kernel lands).
- **The audit tools drift too.** `/doc-coverage`'s concept inventory, `/wiki-lint`'s cited-constants and namespace
  sweeps, and `/mycelium-analysis`'s dimension prompts name the *current* concepts; each gains the axis' concepts
  the day they ship (a hardcoded inventory is the same drift bug one level up — the wiki lint's own calibration
  ledger says so).

### 12.3 Operations documentation

Runbook rows, in the PR that ships the knob: `deployment.md` (sync modes → the required-sync contract; persistence
profiles and what each receipt costs); `production-readiness.md` (durability receipt and golden-fixture replay as
checklist rows); `rbac.md` (the public surface as code; caller identity; `authorized_callers`; the `legacy` profile
and its removal date); a **new `federation.md`** runbook (trust bundles, gateway rotation and revocation, partition
behaviour, what the edge exports and never exports); `observability.md` + `metrics.md` (receipt counters, control
envelopes, evidence freshness); `diagnostics.md` (capturing a **replay bundle from production**, with the redaction
rules item 8 specifies); `audit.md` (mandate lifecycle events, attestations); `tuning.md` and `dynamic-scaling.md`
(allocated rights, budgets); the **shared-responsibility matrix** gains rows for domains and mandates; the threat
model rev 2 (item 8) lands under `docs/design/` and is linked from `crown-jewel.md` and the wiki security page.

### 12.4 Presentations — core and customer

- **Core deck** (`docs/publications/presentation.html`, engineer-facing): the architecture story gains the second
  epoch — receipts as the honest ack, replay as the verification engine, *domains are not NANDA* — with every
  claim labelled by phase status (**shipped** with a tag · **in CI** · **planned**). The deck's existing federation
  slide and the PAIR-shaped slide are the two that change first.
- **Positioning sentence for both decks *(rev 1.5, §13.1)*:** *a distributed coordination substrate for autonomous agents,
  combining local decision-making with evidence-aware capability selection, scoped authority and bounded federation;
  its coordination contracts are tested through deterministic replay.* No "unique", "first", or new-category claim.
- **Customer deck** (`customer-pitch.html`, buyer-facing): the *Honest next* card is rewritten to name typed
  durability receipts, deterministic replay, and federated domains as the next capabilities; the security list gains
  caller identity and the threat model once they ship; "no third-party production deployment yet" stays until it is
  false. The deck's discipline — demonstrated versus next, never roadmap sold as shipped — is the point of
  `/publication-lint`, and the lint runs at each phase exit and each companion release, not on a calendar.
- **Derived PDFs** are regenerated from source at the same points; the source is what is linted.

### 12.5 Philosophy

§1.4 claims this axis is the philosophy's own trajectory. The philosophy must say so itself, or §1.4 is our
assertion about a document that does not make it. With item 1's ADR: the *contract* is added to the property
list (an ack names what it proves); posture rules 3 (the three admissible shapes of prevention) and 6 (a named
mechanism is not the composed guarantee) become litmus tests; Property 7's epistemic symmetry is extended to
evidence (item 3: a reader decides acceptance, the substrate never decides truth). This is the WHY cell for the
axis in the doc-coverage matrix; without it every new row is ~ at best.

### 12.6 The rest — what else belongs on this list

- **Front door refresh** at each phase exit: `README.md` (capability list, companion table), `docs/README.md`,
  `docs/plans/README.md`, and the `CLAUDE.md` on-ramp's *Active work* paragraph — the pages a newcomer reads before
  any chapter.
- **A companion onboarding checklist**, since the axis adds up to five crates (effects, `mycelium-sim`, the
  federation companion, `mycelium-knowledge`, `mycelium-control`, plus mandates over the wiki): a Cargo feature
  line, a CI job, a Docker-suite membership where it makes a cross-node claim, a row in
  `docs/operations/companions.md` and the wiki companions page, a gallery entry, an SDK verb where it has a gateway
  route. The checklist is a wiki page; a companion missing a row is a lint finding.
- **Migration notes per deprecation.** Every §6.6 ledger entry ships with a `CHANGELOG` migration note and a
  guide paragraph the day it is deprecated, not the day it is removed — the ledger is a promise to adopters, and
  the migration text is how the promise is kept.
- **An adversarial self-audit at Phase C exit**, the v2.2.0 pattern (five passes, findings fixed before the
  release), run against items 1, 2 and 7 together — because those three compose into the first *composed*
  guarantee this plan makes (a durable, attributed, cross-domain effect), and posture rule 6 says composed
  guarantees are established by argument *and* gate, never by naming. **Fuzz targets** for every new parser on a
  trust edge (trust bundles, the edge protocol frames, replay bundle decoding) join the existing input-fuzz gate.
- **The analysis series** (`docs/analysis/ratings.md`) keeps running; the axis does not get a dimension of its own.
  What changes: the rotating deep-dives take the axis' items as they ship, and the falsification quota's probes
  target composed guarantees first (rule 6).
- **Research track:** already cross-linked (§9). Added here only as a *candidate*, not a commitment: the receipt
  vocabulary and the replay harness together are the material for a third paper (typed receipts in a
  coordinator-free substrate); decide after Phase B, when the peer-persisted protocol has data.
- **What is deliberately *not* here:** a marketing site, a video, a certification programme, a training course.
  The gallery, the guide, and two decks are the whole persuasion surface, by decision (the publications README).

---

## 13. Beyond the axis — the composition hypothesis *(rev 1.5; recorded now, run later)*

**Not a v3.0 deliverable.** The axis delivers the hypothesis' prerequisites (items 3, 4, 5) and item 3's semantic
gate; the experiment is the opening question of the *next* epoch, and its result decides whether there is one.

On 2026-09-06 the reviewer assessed what a completed axis amounts to and what a further step would need. We
agreed, after one exchange, on a position that this section records so it is not re-argued. **Nothing here is
scheduled inside phases A–E.** It is the hypothesis the axis exists to make testable, and the reason items 3, 4
and 5 have the gates they have.

### 13.1 The assessment we accept

A completed axis makes Mycelium a *distinctive coordination architecture*, with its novelty in the **composition**
— discovery, belief, permission and commitment kept separate, so a participant can discover a service without
trusting its competence, trust its competence without granting it authority, grant limited authority without
admitting it into its domain, and get evidence from a call without that evidence becoming universally accepted
knowledge. It does **not** introduce a new distributed-computing primitive and is not "the first system of its
kind"; the ingredients (gossip, leases, fencing, durable logs, signatures, evidence records, deterministic
simulation) are established, and the prior-art comparison starts there. **Correctness work is not novelty**: items
1, 7 and the persistence fixes remove reasons to dismiss the thesis; they are never presented as differentiation.
The positioning sentence we will use (§12.4): *a distributed coordination substrate for autonomous agents,
combining local decision-making with evidence-aware capability selection, scoped authority and bounded
federation; its coordination contracts are tested through deterministic replay.* "Unique", "first" and a
new-category claim are reserved until a source-based prior-art comparison and comparative experiments support
them. That comparison includes Linda, Zenoh, Automerge and FoundationDB **and** the project's documented governance
lineage — OSGi and the Paremus fabric, argued in
[management-as-intent](../wiki/domain/theory/management-as-intent.md) — compared from sources, not assumed
equivalent.

### 13.2 A commitment is a composition, not a subsystem

The reviewer's first proposal made a *commitment* the central application abstraction and had "the system
construct coordination arrangements". We objected that this is the deferred policy DSL plus a planner — the
supervisory machinery the philosophy strips — and the reviewer withdrew both. The agreed definition:

> A commitment links a **declared requirement**, an authorized participant's **acceptance**, the relevant
> **mandate** and **resource allocation**, and the **receipts** and **assessments** that accumulate afterwards
> establishing what happened.

Five records, each keeping its own meaning: a requirement expresses desired work (`declare_requirement`, item 4's
allocated rights); acceptance records someone undertaking it (a signed knowledge record, item 3); a mandate
establishes permission (item 5); a receipt establishes an operation's outcome (item 1); an assessment decides
whether the requirement was satisfied (item 3). A commitment has a lifecycle — at acceptance it references
requirement, authority and acceptance criteria; receipts and assessments arrive later — and needs at most a small
**correlation record** to link the five. No DSL, no new consensus, no service that assigns obligations. **Nobody
assigns another participant's obligations**: obligations arise only through authorized acceptance; a participant
may plan under a scoped mandate, and no planner is permanent. The arrangement of work is *observable through the
records*, not constructed by a component. Posture rule 2 (composition before primitives) applies in full: a
commitment subsystem needs a written argument that this composition cannot express it.

**The risk that makes it worth testing:** the absence of a planner is not evidence of coordination. The complexity
may simply have moved into participants. The experiment must show that local rules handle dependencies, prevent
conflicting commitments where required, and recover when an obligation becomes infeasible.

### 13.3 The hypothesis and its instrument

> Composing requirements, voluntary acceptance, scoped mandates, allocated resources and attributable outcomes
> allows local participants to reorganise work under changing conditions, with less manual coordination and
> without weakening declared safety boundaries.

- **Instrument:** the existing three-arm harness (`docs/plans/three_arm_workdist.md`), extended to four arms on
  identical substrate, workload, models, tools and budgets: a well-engineered **fixed workflow** · an
  **orchestrator-led** agent system · **the axis with fixed coordination policies** · **the axis choosing among a
  small number of explicit arrangements**. Measured: completion quality, cost, recovery, human interventions,
  authority violations, and performance on unfamiliar disruptions. The report **must expose the cases where the
  fixed workflow or the orchestrator wins**; a result that never does is not credible.
- **When:** record now; **run only when items 3, 4 and 5 are available** — Phase B alone does not supply the
  prerequisites (mandates, evidence, control). Earliest honest slot: after Phase D.
- **Demonstration:** self-contained, in the co-op world, at the gallery bar — three independently governed teams
  (domains) handling a **supply disruption**: a human authorizes an outcome, budget and permitted interventions;
  participants discover capabilities and accept bounded responsibilities; each team keeps its detailed evidence
  local; a key participant disappears and one domain disconnects; the remaining participants revise the
  arrangement within existing authority; conflicting assessments stay visible without stopping useful work;
  completion is established by agreed evidence; a *qualified* lesson about which arrangement helped is retained.
  Human owners can inspect, reject and interrupt at every step. Independent application evidence from a deployment
  is welcome later; the repository's acceptance artefact is this demonstration.
- **What replay can and cannot do here:** item 6 reproduces histories; it **cannot produce counterfactuals** — a
  different action changes subsequent observations and external responses, and replaying the original responses
  yields a convincing, invalid answer. Evaluating a proposed arrangement needs explicit environment models with
  stated limits, scenario variation and sensitivity analysis, item 4's shadow mode, and bounded live experiments for
  what simulation cannot establish. The output is *evidence for a decision with uncertainty attached*, never a
  post-hoc explanation. This is why "automatic algorithm selection" stays deferred (§9).
- **Not launched:** collective learning about arrangements (institutional memory with applicability conditions,
  uncertainty, provenance and expiry) and mechanically-checkable composition properties across a domain edge
  (authority exists · allocated rights cover the obligation · deadlines compatible · no obligation can be left
  unresolved · completion evidence sufficient for the receiving domain) are the two directions after the
  hypothesis holds. Both are compositions of items 2–5; neither is a new subsystem. One vertical slice, one
  bounded experiment, then decide.

---

## Appendix A — verified anchors (2026-09-05)
| Claim in a plan | Where verified | Holds? |
|---|---|---|
| `set_with_min_acks` acks on `timestamp >= write_ts` from any peer | `src/agent/kv_quorum.rs::observe` | yes — overclaim |
| snapshot: write → fsync file → rename, no directory fsync | `persistence.rs::do_snapshot` | was true; fixed #183 |
| forced per-record fsync exists | `WalMsg::Append { force_sync }` | yes (v2.4.2) |
| HLC reads the system clock at one site | `hlc.rs:130` | yes |
| `membership_governor::decide` is pure | signature takes probabilities + roll | yes |
| SWIM datagrams unauthenticated | `swim.rs` — no sign/verify | yes |
| curator write entitlement is a local boolean | `mycelium-wiki/src/agent.rs::is_curator` | yes |
| store CAS is version-only | `store.rs` `WikiError::Conflict` | yes |
| proposals are evaporating KV | `wiki/{group}/proposal/{id}` | yes |
| `TraceEvent` has no parent link | `mycelium-reason/src/trace.rs` | yes |
| `max_staleness_ms` = 0 with no peers heard | `emergent.rs:370` | yes |
| membership cooldown = 3 × health-check interval, **fixed at start**; not re-derived from live timing intents | `membership_governor.rs:215–216`, `converge` | yes — rev 1.0's "live shortening" was wrong |
| A2A, OIDC verifier, `EgressPolicy` exist | `a2a.rs`, `oidc.rs`, `config.rs` | yes |

## Appendix B — the reviewer's documents (vendored, unmodified)
`docs/plans/external/2026-09-05-1-contracts.md` · `…-6-replay.md` · `…-2-domains.md` · `…-3-knowledge.md` ·
`…-4-stability.md` · `…-5-mandates.md`. Each carries an attribution header; the body is verbatim.
