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
