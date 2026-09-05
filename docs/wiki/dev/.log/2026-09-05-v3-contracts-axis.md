# v3.0 contracts axis — assessment of the third-party proposal (2026-09-05)

Two external artefacts arrived the same day as the v2.4.2 review fixes: a six-item **v3.0 enhancement
proposal** ("make Mycelium explicit about what local autonomy can safely compose into") and a **seven-PR
implementation plan** for its first item (typed contracts: local application · local sync · replica sync ·
destination commit; operation identities stable across retries; uncertainty as a first-class outcome).
Recorded in `ROADMAP.md` § v3.0 → *The contracts axis* as **proposed, not committed**.

## Verdict
Strong and correctly ordered. Items 1 + 6 (contracts + deterministic replay) are exactly where the day's
persistence work points — the snapshot/WAL race was deterministic and shipped in every release; the suite
never saw it. Item 2 (bounded domains) is the genuinely new structural investment and the one likely to touch
the wire → the first honest `3.0.0` trigger. Items 3 and 5 must stay companions.

## Anchors verified against code
- `src/agent/kv_quorum.rs`: `observe()` counts `sender != self && timestamp >= write_ts` — **a newer
  competing peer write acks a payload that peer never held; nothing about disk.** Real overclaim on a shipped
  API (`set_with_min_acks`). Document now; fix as the plan's PR 4a.
- `mycelium-core/src/persistence.rs::do_snapshot`: write tmp → `sync_data` → `rename`, **no directory
  fsync** → power-loss durability of the snapshot install is unproven (only process-kill is tested). Phase-0 fix.
- `Committed { persisted }` returns `true` with persistence unconfigured ("no promise broken") — tri-state or
  document; the SDKs already model absence as `None`/`null`.
- `WalMsg::Append { force_sync }` exists since v2.4.2 → the plan's PR 3 (required local sync) is an API
  exposure, not a redesign.
- `docs/design/exactly-once-effect.md` **declined-with-evidence** to extract the claim/ack/requeue shape as
  code (WS-G Phase 6); the proposed `mycelium-effects` companion adds destination-side dedup instead — related
  but distinct; the ADR must reconcile.

## The four reconciliations (PR-1 ADR)
1. Persist-first strong path vs the v2.4.2 apply-first invariant — safe only via the WAL-tail merge; state
   the dependency, pin it with a test.
2. Split PR 4 into 4a (exact-identity ack on the existing quorum path) and 4b (persisted-by-peer protocol).
3. Say that PR 3's mechanism exists; it changes the effort estimate and justifies PRs 2–3 as the first release.
4. Reconcile the effects companion with the declined extraction decision.

## Missing from the plan
Pre-stamped update submission (retries must not tick a fresh HLC — only replay does this today); mapping the
existing at-least-once primitives (`emit_reliable`, mailbox, tuple-space lease) into the vocabulary; citing
in-tree prior art (idempotent wiki ingest; the v2.4.0 exactly-once work-distribution proof) and stating what
a SQLite destination adds.

## Reusable lesson
A receipt vocabulary is only as honest as its weakest ack. The `>=` quorum ack sat under a "durability
counting" label since it shipped because nobody asked *which payload* and *which barrier* the ack attested.
Every future ack-like API states both.

## Addendum — the deterministic-replay plan (item 6), same day

Seven-PR plan for a `mycelium-sim` harness (exploration / exact replay / scenario replay; adapters for
clocks, RNG streams, scheduling, network, storage, external work; failure bundles + minimisation). Verdict:
sound; incremental seam-based approach is right for a multi-thread tokio runtime; its storage section is
the strongest part.

**It found a real defect in the same-day fix:** `do_snapshot`'s WAL read-back used `unwrap_or_default()`,
so a transient read error → snapshot written *without* the tail → WAL truncated → loss. Fixed: the snapshot
aborts on read failure (absent file = empty tail); gate
`regression_snapshot_aborts_when_wal_tail_is_unreadable` (models the failure as `wal.bin` being a
directory — the one read failure injectable without a filesystem adapter, which is the plan's point).

**Entanglement measured** (grep counts, hot files): HLC → `SystemTime::now()` ×1 (one seam);
persistence.rs `tokio::fs` ×25 (one module — first target); tasks.rs `fastrand` ×6, timers ×6;
connection.rs `Instant::now` ×9; consensus.rs `Instant`/`physical_ms` ×7 + rand ×2; wasm-host provisioner
timers ×14. `membership_governor::decide(am_member, is_drain, join_p, leave_p, roll)` and
`tuning_governor::gate` are pure as claimed; `timing_governor` applies effects directly.

**Reconciliations for the plan's PR 1:** papaya CAS-retry nondeterminism is outside a one-transition
kernel (coverage map must say Loom/real threads own it); `AHashMap` iteration order is per-process-random
except the store's fixed-seed state; clock injection must reach `causal_now_ms` lease checks; the static
forbidden-call check moves to PR 3; the witness becomes a `cfg(test)` toggle. Pair the durability oracle
with item 1's receipts.

## Reusable lesson (replay)
`unwrap_or_default()` on a read in a path that later *truncates* is a data-loss primitive. The review found
it by asking "what does the model do on a read error?" — the question a filesystem adapter forces at every
call site; without one, grep `unwrap_or_default\|unwrap_or(Vec::new` in persistence code each lint.

## Addendum — the knowledge layer (item 3), same day

Seven-PR plan for an optional `mycelium-knowledge` companion: four record types (claim · observation ·
assessment · acceptance decision — the *assessment* split from *observation* is the plan's best move),
signed immutable records with explicit links, discovery manifests in KV under `knowledge/head/…` with records
and evidence in an authorized store, evidence-aware resolution wrapping `resolve_for_caller` with a
deterministic reader policy and four-way outcomes with reasons, contextual competence (no reputation scalar),
identity ≠ independence, active expiry/correction. Verdict: agree with the whole shape — "converge on what
participants said and reported; leave what should be believed explicit, contextual, revisable" is item 3 as
the September proposal meant it, and it stays above the substrate.

**Anchors verified:** `resolve_for_caller` (`is_fresh` + schema gates) is the wrap point; the capability
refresh is the evaporation lease; `blob.rs` verify-on-read + `llm:read` gate (hash still = credential);
AgentFacts Ed25519 self-signed; `tls::verify_bytes` pub; wiki `SectionId` minting. **Trace has no parent
link** — `TraceEvent { hlc, node, kind, detail }` — so its causal claim is HLC adjacency; the plan's
`derived_from` is the fix, not an import of HLC order.

**Six PR-1 reconciliations** (in ROADMAP): reserve `knowledge/` in lib.rs + both front-door lists; add
trace parent links rather than promote HLC order; reuse `schemas/` as the schema locator; expiry timers on
the replay clock seam; rank within the accepted class then hand to the reason router (state composition
order); PR 6's demo ships as a CI gallery entry.

## Reusable lesson (knowledge)
A signature proves who said it; a hash proves what was said; neither proves it is true, and a "causal"
trace without parent links proves only order. Every record type in the plan names which of these it
carries — keep that discipline when the adapters import today's traces and advertisements.

## Addendum — scoped mandates (item 5), same day

Seven-PR plan: a common mandate contract (epoch ≠ term), CAS ≠ authorization, every mutation path protected,
three lifecycle events, a handover journal the successor inherits as history, incumbency rules, an explicit
partition table, a fail-closed authority restart. Verdict: agree with all of that.

**Anchors verified:** `is_curator: AtomicBool` is the write entitlement (the indicted inference); store CAS =
`Conflict` on version only; `GitStore` `update-ref` CAS + push, single-writer assumption; proposals evaporating
KV `wiki/{group}/proposal/{id}`; `LockService` fencing token = commit HLC; `revocation.rs`/`transparency.rs`;
leased consensus slots; `KvHandle::append`.

**The one disagreement — v1's shape.** The plan's reference authority is a new SQLite-backed service process
per wiki scope. That is a control plane for the scope; philosophy § Not a platform: *"No daemon, no
orchestrator, no control plane."* The check belongs *inside the canonical store's own atomic boundary* — a
mandate ref under the same `update-ref` CAS, a pre-receive hook on the shared remote (already the external
authority in that deployment), the epoch in `FsStore`'s mutator section — with establishment as a leased
consensus slot (`mandate/{scope}`, commit HLC = epoch, lease = term) and durable proposals via the existing
log verb. A SQLite service is fine as an *application's* reference resource; never Mycelium-shipped.

**Further reconciliations:** don't build a second fence beside `LockService` — audit it under the replay
harness first; the decisive test *is* replay scenario B — build it once there; reserve `mandate/` +
`log/wiki/` at PR 1; hold the claim at "enforces configured rules".

## Reusable lesson (mandates)
When a plan says "the resource must enforce", ask *which* resource already has an atomic boundary. Adding a
process to hold the truth is the coordinator arriving through the side door; the substrate's answer is to put
the fence where the bytes already serialise.

## Addendum — bounded, federated domains (item 2), same day

Seven-PR plan: a domain = one independently admitted mesh; federation = explicitly exported services over a
separate authenticated protocol (HTTPS between gateways); three trust relationships; allowlist catalogs;
`RemoteCapability`; preserved origin; ≥2 gateways, no leader; `DeliveryUnknown`; fixed quota slots; no merge
on reconnect. Verdict: agree with the whole shape.

**Anchors verified:** SWIM = unauthenticated UDP (drops malformed, signs nothing) → disabling it in the
enforced profile is grounded; per-node CA `RootCertStore` is the admission root the plan says never to
pollute; `federation_facts.rs` + guide 17 exist; `FACTS_PREFIX` is the full board.

**Correction to my own earlier note:** I had called domains "the one item likely to touch the wire, the first
honest `3.0.0` trigger". The plan keeps the gossip wire untouched (separate roots + SWIM off); only a later
authenticated domain-bound SWIM/handshake would trigger a substrate major. Corrected in the ROADMAP.

**Reconciliations:** compose with the shipped A2A edge or say why not; reuse the OIDC verifier for JWS; extend
`EgressPolicy`; AgentFacts stays the public descriptor via a filtered builder; no `federation/` KV prefix as an
explicit invariant; process isolation is the example's claim, not the library's.

## Reusable lesson (domains)
Before writing "touches the wire" about a proposal, read how it moves bytes. A boundary can be enforced
entirely at the edges (roots, listeners, catalogs) while the medium inside stays exactly as it is — which is
the substrate's own design, applied once more.

## Addendum — adaptive stability discipline (item 4), same day

Seven-PR plan for `mycelium-control`: three promises kept apart (hard bounds / stability / service), a
`ControlSpec` per governor and observe → propose → reserve → act → reconcile with stable action IDs, an
actionable per-input `ControlView`, strict budgets as fixed disjoint rights never reclaimed on disappearance,
concrete loop-breaking points, shadow-before-enforce profiles. Verdict: agree with the whole shape.

**Anchors verified:** `demand.rs` = declaring-node/provider count over `req/ cap/ gcap/` (not work);
provisioner self-elects probabilistically with an `Installing` reservation; opacity 100 ms loop + pure decision
+ full-channel veto override; **`max_staleness_ms` = 0 when no peers heard**; **membership cooldown =
3 × `health_check_interval`** — a live coupling to the timing governor; tuning floors/ceilings/ratchets;
`provisioning.rs` example; guardrails `Strength` tiers; `TupleSpace::depth` / `Blackboard::depth`.

**Reconciliations:** rights = prevention → opt-in profile, promise strength in the guardrails tier vocabulary;
workload probe reads the companions' depth signals; the harness is replay stage 6; fix the cooldown coupling
now (absolute Duration or declared bound); give staleness a "no observation" state; one owner per deficit;
detection-not-prevention for everything but the ledger.

## Reusable lesson (stability)
A cooldown expressed in ticks is a cooldown that the thing tuning the tick can shorten. Every damping constant
should be a Duration the governor that damps owns — or its coupling should be written down where the other
governor's bounds are.
