# Mycelium v3.0 — the contracts axis: roadmap and implementation plan

**Status:** adopted plan, 2026-09-05 · **Owner:** Mycelium maintainers · **Version of record:** this file
(`docs/plans/v3-contracts-axis.md`); `ROADMAP.md § v3.0` carries the index and points here.

**Provenance.** On 2026-09-05 an external reviewer (a) found five defects in v2.4.1 — three P1 persistence
faults, an unauthenticated tool-invocation surface, and a blocking async path — all fixed and released the same
day (v2.4.2, v2.4.3, `mycelium-py` 0.2.4, `mycelium-ts` 0.1.1); and (b) proposed **six enhancements** for the
next epoch, each with a seven-PR implementation plan. Those six documents are vendored unmodified under
[`docs/plans/external/`](external/) with attribution. This document is *our* plan: what we adopt, what we
decided differently and why, how the six compose, and the order we will build in. Every divergence from the
reviewer's text is marked **⚠ Divergence** and collected in [§7, the decision register](#7-decision-register).

**"v3.0" is a roadmap epoch, not a version.** The released substrate is `mycelium` **2.4.3** (wire v12, PREV 11).
Nothing in this plan changes the wire; a substrate `3.0.0` is triggered only by a breaking wire or public-API
change, and §4.2 names the one candidate. Deliverables ship as companion crates on their own version lines and as
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
5. **Roles evaporate; authority expires.** Every role this axis introduces — gateway, curator, holder, observer —
   carries an expiry, and its authority is recomputed, not inherited, on renewal.

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

**Order.** Items **1 and 6 first, together**: one states what must hold, the other attacks it. Then **2** as the
structural investment. **3, 4, 5** follow as companions above the substrate, each once its dependencies' first
releases exist. Within each item, the first two-to-three PRs are the usable release; the rest are gated on
demonstrated semantics.

**Phases with exit gates** (no calendar estimates — the reviewer was right that none is justified before each
ADR):

| Phase | Contents | Exit gate |
|-------|----------|-----------|
| **A** | 1·PR1–3 (ADR, identities + receipts, required local sync) · 6·PR1–4 (inventory, kernel, persistence adapters, WAL/snapshot scenario) · the cooldown-coupling fix (4, standalone) | typed local durability usable from Rust; the WAL/snapshot race replays from a bundle and its merge-removed witness fails |
| **B** | 1·PR4a (exact-identity ack on the existing quorum path) · 1·PR4b (persisted-by-peer protocol) · 2·PR1–3 (domain profile, trust bundles, filtered catalogs) | `set_with_min_acks` acknowledges the *exact* payload only; two meshes discover selected exports without merging |
| **C** | 1·PR5–7 (effects companion, tuple-space consumer, SDK parity) · 2·PR4–7 (calls, gateways, partition, example) · 3·PR1–3 · 5·PR1–2 | the adversarial release demos of 1 and 2 pass in CI |
| **D** | 3·PR4–6 · 5·PR3–6 (on replay scenario B) · 4·PR1–5 · 6·PR5–6 | evidence-aware resolution and curator handover both replay deterministically |
| **E** | 3·PR7 · 4·PR6–7 · 5·PR7 · 6·PR7 | combined-feedback scenario green; shadow-mode rollout documented |

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
- **Bundle format ships light first**: manifest + choices trace in PR 2; boundary events and checkpoints when a real
  failure needs them.
- **4.2 — the `3.0.0` candidate.** Nothing here changes the wire. The only wire-touching work anywhere in this axis is a
  *later* authenticated, domain-bound SWIM/handshake (item 2's deferred milestone); that, and nothing else, would
  justify a substrate major.

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
- **Reuse the OIDC verifier** (`jsonwebtoken`, JWKS refresh, algorithm allowlist) for JWS validation; **extend
  `EgressPolicy`** (`allow_hosts`) for the source-side rule; **AgentFacts stays the public well-known descriptor** via
  a filtered builder. *Why:* three existing mechanisms, three proposed duplicates.
- **No `federation/` KV prefix — as an explicit invariant.** Foreign observations live in the companion's cache. The
  wiki lint's namespace sweep checks it.
- **Our own correction.** We had recorded domains as "the one item likely to touch the wire". Withdrawn: v1 federates over
  HTTPS at gateways (separate roots + SWIM off) and leaves the wire untouched.
- **Process isolation is the example deployment's claim, not the library's.** The plan says so; we hold it in every doc.

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
`3 × health_check_interval`** — elapsed time scaled by the tick, so a timing intent that shortens the interval shortens the
cooldown: the plan's warning is a live coupling.

**⚠ Divergences.** Promise strength stated in the **guardrails tier vocabulary**, not a new one (a self-enforced budget is
Tier A; a fleet ceiling holds only with exclusive rights) · the workload probe **consumes the companions' existing depth
signals** (`TupleSpace::depth`, `Blackboard::depth`, KV-ring stages) before a new metric · the combined-feedback harness
**is replay stage 6**, built once, reusing the governors' pure decision functions · **fix the cooldown coupling first**
(absolute `Duration`, or a declared bound in the timing governor) — small, standalone, gateable now · give staleness a
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
- **No resource-authoritative service process.** Reviewer: v1 puts the mandate and the canonical commit in a new
  SQLite-backed daemon per wiki scope. That is a control plane for the scope — *"No daemon, no orchestrator, no control
  plane"* (philosophy § Not a platform). We put the check **inside the canonical store's own atomic boundary**: for
  `GitStore` a `refs/mycelium/mandate/{group}` object updated under the *same* `update-ref` CAS as content, and on the
  shared remote a **pre-receive hook** verifying the signed epoch (that remote already is the external authority in that
  deployment); for `FsStore` the epoch in the same mutator critical section. A SQLite service is acceptable **only as an
  application-owned reference resource** (the effects companion's destination shape). *Why:* the reviewer's own
  criterion — "the resource must enforce" — is met by the store that already serialises the bytes; adding a process to
  hold the truth is the coordinator arriving through the side door.
- **Establishment via the substrate's own consensus.** A mandate is a **leased consensus slot** `mandate/{scope}` whose
  committed value is (holder, epoch): the commit HLC is the epoch (monotonic across holders, #166), `committed_lease_secs`
  is the term. Owner-signed appointment remains the pinned-deployment alternative.
- **Durable proposals via the existing log verb** (`KvHandle::append` → `log/wiki/{group}/proposals`) plus item 1's
  receipts — not a service database; the evaporating queue becomes the discovery hint the plan wants.
- **Do not build a second fence beside `LockService`.** The plan declines to certify its converged-view issuance; we audit
  it under the replay harness (scenario B) before certifying or replacing.
- **The decisive test is replay scenario B** — built once, there.
- Reserve `mandate/` + `log/wiki/` at PR 1 · hold claims at "enforces configured eligibility rules".

---

## 7. Decision register

Every place this plan departs from the reviewer's six documents. "Kept" means we adopt their text; the rest are ours.

| # | Item | Reviewer proposed | We decided | Why |
|---|------|-------------------|------------|-----|
| D1 | 5 | A resource-authoritative **service process** (SQLite daemon) holds mandates + canonical commits | Fence **inside the canonical store's atomic boundary** (mandate ref under the same `update-ref` CAS; pre-receive hook on the shared remote; epoch in `FsStore`'s mutator section). SQLite only as an application-owned reference resource | *Not a platform*: no daemon, no control plane. The store already serialises the bytes; put the fence where they serialise |
| D2 | 5 | Owner-signed appointment as the establishment mechanism | **Leased consensus slot** `mandate/{scope}` (commit HLC = epoch, lease = term) as the default; owner-signed appointment for pinned deployments | Uses the substrate's own agreement + the #166 fencing-token precedent instead of a new authority |
| D3 | 5 | A durable proposal journal in the authority service | `KvHandle::append` log stream + item-1 receipts | Composition over a new store |
| D4 | 5 | The lock service's issuance is "insufficient" for strict mode; build the new fence | Audit `LockService` under replay scenario B first; no second fence | Two fences beside each other is how guarantees drift |
| D5 | 2 | A new `POST /federation/v1/call` protocol | Compose with the shipped **A2A** edge, or write the ADR argument for a second protocol | Two invocation edges with different auth models is the drift we just cleaned up |
| D6 | 2 | New JWS / signed-object stack; new egress rule; new public descriptor | Reuse the **OIDC verifier**, extend **`EgressPolicy`**, keep **AgentFacts** as the public descriptor via a filtered builder | Three existing mechanisms; no duplicates |
| D7 | 2 | (implicit) foreign observations disseminated in KV later | **No `federation/` KV prefix** as an explicit, lint-checked invariant | Foreign state never enters the medium |
| D8 | 1 | Strong path: persist → sync → apply | Keep **apply → persist** as the invariant; persist-first permitted only because the WAL-tail merge holds; dependency pinned by a test | The race shipped because two orderings drifted |
| D9 | 1 | One PR for the peer-persistence service | **Split 4a/4b**; 4a (exact-identity ack) pulled forward | 4a fixes a live overclaim cheaply; 4b is a protocol |
| D10 | 1 | Directory fsync noted as a caveat | **Phase 0 fix** — done 2026-09-05 | A few lines; the difference between a power-loss claim we can and cannot make |
| D11 | 1 | (absent) | Reconcile with `exactly-once-effect.md`'s **declined** extraction; add pre-stamped updates; map `emit_reliable` / mailbox / tuple lease into the vocabulary; cite in-tree prior art | Two design records must not contradict; retries must not tick a fresh HLC |
| D12 | 6 | Static forbidden-call check in PR 7 | **PR 3** | Seams erode while the harness is built |
| D13 | 6 | (absent from inventory) | Add **papaya CAS retries** (owned by Loom), **`AHashMap` iteration order**, **lease-expiry clock reads** | Real nondeterminism sources in the covered paths |
| D14 | 6 | Full failure-bundle format from the start | Manifest + choices trace first; events/checkpoints when a failure needs them | Ship the reproducible core early |
| D15 | 3 | (absent) | Reserve `knowledge/`; reuse `schemas/` as the schema locator; **trace `derived_from` links** rather than HLC order as causation | The trace has no parent link today; HLC adjacency is not causality |
| D16 | 3 | A second ranker in the resolver | Rank **within the accepted class**, then hand survivors to the reasoning router | Two rankers must compose in one stated order |
| D17 | 4 | A new promise-strength vocabulary | The **guardrails tiers** | One vocabulary for "how strong is this promise" |
| D18 | 4 | A new `WorkloadProbe` metric | Consume the companions' **depth** signals first | Backlog is already measured where work queues |
| D19 | 4 | A bespoke combined-feedback harness | **Replay stage 6**, built once | One harness, one clock seam |
| D20 | 4 | Cooldown coupling listed as a design concern | **Fix it now** (absolute `Duration` / declared bound) | Small, standalone, gateable; live today |
| D21 | 4·6 | Combined tests as their own rigs | Every item's decisive demonstration is a **CI gallery entry** | *Expressible ≠ supported* |
| D22 | all | Six separate seven-PR plans | **One plan, one dependency graph, one posture** (this document); the six kept vendored for attribution | The six overlap at receipts, clock, harness, namespaces — one order avoids six orderings |
| D23 | 2 | ("4.0", "5.0" epochs) | Roadmap **epochs, not versions**; the only `3.0.0` candidate is a later authenticated domain-bound SWIM | Version numbers follow breaking changes, not ambitions |

**Kept without change:** the four-receipt vocabulary; the three trust relationships; the four record types; the three
lifecycle events; term ≠ epoch; fixed allocated rights never reclaimed on disappearance; the asymmetric uncertainty rule;
`DeliveryUnknown`; every "this proves X, not Y" honesty clause; every deferred list.

---

## 8. Corrections to our own claims (2026-09-05)
- We wrote that domains were "the one item likely to touch the wire — the first honest `3.0.0` trigger". Withdrawn (D23).
- Two sentences we wrote the same morning claimed a plain `set_async` surfaces a WAL error as an `Err`. It does not — its
  `bool` is the gossip-queue result and the WAL error was silently discarded. Now logged at `warn`; the receipt is item 1.
- Our `persisted` flag reads `true` when persistence is unconfigured ("no promise broken"). The SDKs model absence as
  `None`/`null`; the Rust type needs a tri-state or documentation — item 1's ADR.

---

## 9. Cross-cutting rules
- **Namespaces reserved at each item's PR 1**, in `src/lib.rs` *and both* front-door lists: `knowledge/`, `mandate/`,
  `log/wiki/`; **explicitly none** for federation.
- **Versioning.** Companions on their own lines; core additions on 2.x; a `3.0.0` only for D23's candidate.
- **Every gate is a test in CI** without a live node where possible (the day's pattern: stub servers, fetch recorders,
  in-process writers); Docker suites for the two-mesh and combined scenarios.
- **Documentation ingest**: each PR updates the wiki page it touches and adds a dated `.log/` entry; the wiki lint's
  doc-vs-code sweep and the doc-coverage must-work rule apply to every instruction this plan adds.
- **Deferred, by decision:** a policy DSL; automatic algorithm selection; distributed transactions; aggregated
  reputation; inferred observer independence; mandatory LLM judgment; consensus over truth; authenticated SWIM;
  transitive federation; dynamic quota transfer; delegation of mandates; replicated authority.

## 10. Immediate next steps
1. **Item 1, PR 1** — the contract ADR (with D8, D11) and the regression floor. *(This unblocks everything.)*
2. **Item 6, PR 1** — the nondeterminism inventory with D13 and the trace schema.
3. **Item 4's cooldown fix** — standalone, this week.
4. **Item 1, PR 4a** — the exact-identity ack; fixes the live `>=` overclaim.
5. Item 2's ADR (D5–D7) in parallel, since it has no code dependency on 1 or 6 for its discovery release.

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
| membership cooldown = 3 × health-check interval | `membership_governor.rs:216` | yes |
| A2A, OIDC verifier, `EgressPolicy` exist | `a2a.rs`, `oidc.rs`, `config.rs` | yes |

## Appendix B — the reviewer's documents (vendored, unmodified)
`docs/plans/external/2026-09-05-1-contracts.md` · `…-6-replay.md` · `…-2-domains.md` · `…-3-knowledge.md` ·
`…-4-stability.md` · `…-5-mandates.md`. Each carries an attribution header; the body is verbatim.
