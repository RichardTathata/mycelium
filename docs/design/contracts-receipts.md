# Contracts and receipts — what an acknowledgement proves (ADR, v3 contracts axis item 1)

**Status:** adopted 2026-09-13 · item 1 PR 1 of `docs/plans/v3-contracts-axis.md` (§3; decisions D8, D9,
D10, D11, D24; the §9 cross-cutting rules). This record states the *contract*; the code that returns
typed receipts lands in PR 2 (identities, receipts, failure vocabulary), PR 3 (required local sync),
PR 4a (exact-identity ack), PR 4b (persisted-by-peer). Nothing here changes a public type. What this
PR adds in code is the **regression floor** (§8) and the **golden on-disk fixtures** (§9).

> The philosophy's own statement of this axis: `docs/philosophy.md` — *Property 8, the contract: an
> ack names what it proves* and litmus tests 4–5. This record is the WHY cell; the HOW is the guide's
> contracts-and-receipts chapter, written with PR 3's release gate.

## 1. Context — what an acknowledgement means today, site by site

Every verb below returns something that reads as success. None of them says the same thing, and none
of them says what a caller reading `true` or `Ok` usually assumes. This inventory is the contract's
baseline; each row is pinned by a test in §8 so that PRs 2–4 change a meaning only by changing a test.

| Site | Returns | What it actually proves | What it does **not** prove |
|---|---|---|---|
| `KvHandle::set` / `delete` (`mycelium-core/src/kv_handle.rs`) | `bool` | the update was **applied locally** and **queued** for gossip (`false`: queue full or shard dead — still applied locally, anti-entropy repairs; or the write was rejected as oversized — nothing applied) | that it reached disk, a peer, or anyone's handler |
| `KvHandle::set_async` / `delete_async` | `bool` | as above, after awaiting queue capacity | as above |
| `set_with_min_acks` (`KvQuorumExt`, `src/agent/kv_quorum_ext.rs`) | `Result<usize, QuorumError>` | `min_acks` distinct peers gossiped **some** update for the key with `timestamp >= write_ts` — *propagation evidence*, counted in `kv_quorum.rs::observe` | that any peer holds **this** payload (a newer overwrite counts), or persisted it. **The `>=` is an overclaim** (D9): PR 4a replaces it with an exact-identity ack |
| `ConsensusResult::Committed { persisted, .. }` (`src/consensus.rs`) | `persisted: bool` | quorum committed and applied locally; `true` = the forced-`fdatasync` WAL append succeeded **or persistence is not configured** (nothing was promised); `false` = committed cluster-wide, **local durability not established** (the record may still be on disk: the write precedes the sync) | a tri-state collapsed into a bool (D24): `true` cannot distinguish *on disk* from *never promised*; `false` was documented as "not in this node's WAL", an overclaim corrected in this PR |
| gateway `POST /gateway/overlay/consistent/set`, `/consensus/cross_group_propose` | JSON `"persisted": bool` | the same bool | the same collapse; SDKs model absence (`persisted: Optional[bool]` / `boolean \| null`) |
| `emit_reliable` (`ServiceHandle`) | `AckResult::{Acknowledged, Timeout}` | `Acknowledged`: the target's handler ran and answered | `Timeout` is **not** "not delivered" — the handler may have run and the ack been lost |
| `deliver_event` (mailbox) | `bool` | a KV write under `mailbox/{target}/{kind}/{hlc}` was applied locally and queued | delivery: that is the target's drain, later |
| `TupleSpace::take` → `complete` (`mycelium-tuple-space`) | item / `Ok` | a claim under a lease; `complete` acknowledges the item and queues the next stage atomically (WAL-backed at the primary) — a receipt about the **pipeline**, not about the consumer's effect | that the consumer's business transaction happened, or that a worker which died after acting but before `complete` did not act — the expired lease **re-queues**, and the second worker acts again |
| `mycelium-wiki` `GitStore` CAS / claim-check ingest | `Ok` / `Conflict` | at-least-once, never-lose; the reconcile is an **idempotent append-merge** | store-level exactly-once — a `Conflict` caller may apply an edit more than once (wiki page, `concurrent_idempotent_appends_deliver_every_edit_exactly_once`) |

Two facts that shape everything below:

- **An apply-first operation is observable before its durability can fail.** Every write site applies
  to the store, then hands the record to the WAL (runtime invariant, `docs/wiki/dev/architecture/runtime-invariants.md`
  §Persistence). Readers, subscribers and effects can see the value before `append_sync` returns
  `Err`. A caller receiving a durability failure therefore **cannot infer that nothing happened**.
- **A timeout is not a negative.** In every row above, the absence of an ack within a deadline is
  compatible with the operation having fully happened. The substrate has no verb that returns
  "nothing happened".

## 2. Decision — four receipts, kept strictly separate

An acknowledgement is a **receipt**: a typed statement of exactly what was established, by which
component, at which point. Four kinds, never conflated, never inferred from one another:

| Receipt | Establishes | Established by | Strength |
|---|---|---|---|
| **Local application** | what became of the operation at this node's store — `Applied` · `AlreadyCurrent` (an idempotent retry: the store already holds exactly this operation) · `Superseded` (a *different, newer* value won) · `Refused` (the live-entry cap declined it; the key may be absent) | `apply_and_notify_reporting` | the store's own LWW rule; visible immediately |
| **Local sync** | **this exact operation** crossed this node's persistence barrier — `OnDisk` (fsynced WAL record), `Buffered` (the WAL accepted it but this node's `SyncMode` does not sync per append, so it survives a process crash and not a power loss), `Failed` (**durability not established**: the writer was gone or the write or sync returned an error — the bytes may or may not be on disk, since `wal_append` writes before it syncs), `NotConfigured` (no persistence: nothing was promised) | the WAL writer's forced-`fdatasync` append (`WalHandle::append_sync`, since v2.4.2) | a durability claim in the module-doc sense: `Err` is never swallowed, and `Failed` is never read as *absent* |
| **Replica sync** | **named, distinct peers** — origin excluded — persisted **this exact operation** | a per-peer, identity-bound persisted ack (PR 4b; PR 4a first makes the existing count exact-identity) | strong only when the peer's ack is itself a local-sync receipt for the same `operation_id` |
| **Destination commit** | the destination committed the business change **and** its dedup result **in one transaction** | the effects companion's reference destination (PR 5; SQLite) | exactly-once *effect*, by the destination's transaction, never by the substrate |

Rules that hold for every kind:

1. **A receipt names its point on the ladder.** `Applied` is not `OnDisk`; `OnDisk` on one node is not
   `ReplicaSync`; three replicas are not a destination commit. A higher rung is never implied.
2. **Same identity, different content, is a conflict** — never a silent overwrite of a receipt.
3. **A timeout returns partial evidence or `DeliveryUnknown`** — the rungs that were established, then
   an explicit *unknown* for the rest. Never a negative.
4. **A receipt carries who established it** (node id for local/replica; destination id for commit), so a
   later reader can tell an attestation from a claim.

### 2.1 Per-receipt semantics (D8, rev 1.1) — defined independently

Each row answers four questions on its own; no row borrows another's answer.

| | Visible when | Durable when | May subscribers / effects run before durability? | After a sync failure, what remains true |
|---|---|---|---|---|
| Local application | at `apply_and_notify` | never promised | yes — that is the meaning of this receipt | the value is still in the store and may have gossiped; `Applied` stands |
| Local sync | at `apply_and_notify` (apply-first) | when `append_sync` returns `Ok` | **yes** — the value is visible and may have propagated before `Err`; the receipt says `Failed`, the *application* receipt still says `Applied`. A caller that must not act on an undurable value reads the local-sync receipt before acting, not the store | applied and possibly propagated; **durability unknown** — `wal_append` writes the record before `sync_data`, so a failed sync leaves the bytes possibly on disk and a later `replay` may restore them; the receipt never claims the record is absent. What is promised: nothing; what may happen on restart: local replay *or* anti-entropy from peers |
| Local sync (**required**, PR 3) | **only after** the forced sync returns — persist → apply → gossip | when `append_sync` returns `Ok`; the receipt is always `OnDisk` | **no** — nothing is applied, notified or gossiped until durability is established | on failure, **nothing became visible at this node**: no store entry, no subscriber, no frame. The one contract under which a durability error means the operation did not happen *here* (it says nothing about other nodes) |
| Replica sync | at the origin's apply; at each replica's apply | when the named peer's persisted ack for this `operation_id` arrives | yes at the origin; the receipt lists which replicas answered | the origin's local receipts stand; the replica set is partial and named; `DeliveryUnknown` for the rest |
| Destination commit | at the destination's transaction commit — **not** before (the destination decides visibility) | at that commit | **no** — the destination's transaction is the boundary; a retry before commit is deduped by `operation_id` inside it | the destination either committed (and will answer the retry with the same receipt) or did not; the companion reports `DeliveryUnknown` until a retry resolves it |

### 2.2 Ordering (D8)

**Apply → persist stays the invariant.** Every write site applies to the store, then hands the record
to the WAL. The reviewer's *persist → sync → apply* is permitted on the strong path (PR 3's required
local sync) **only because the snapshot merges the WAL tail before truncating** — so a record the writer
fsynced before the caller applied it cannot be discarded. That dependency is pinned by
`regression_snapshot_retains_wal_record_acked_before_local_apply` and
`regression_writer_threshold_snapshot_right_after_ack_keeps_write` (`mycelium-core/src/persistence.rs`,
`durability_tests`): if either fails, persist-first is no longer admissible. Two orderings may each have a
contract (§2.1); they may not silently share one meaning.

## 3. Identities (PR 2 code; stated here so nothing else mints a second scheme)

- **`operation_id`** — caller-generated, assigned **before dispatch**, stable across retries, worker
  replacement and gateway hops. It is the key of every receipt and of destination-side dedup. A retry
  **re-submits the same stamped update**: retries must not tick a fresh HLC (D11) — PR 2 adds a core
  API to submit a pre-stamped update. Same `operation_id` + different content ⇒ `Conflict`.
- **`attempt_id`** — one per delivery attempt of an operation; distinguishes "the second try was
  acked" from "the first was". Receipts carry both.
- These are the identities the **AE0 action envelope** binds (plan §6.8; queue item 3) and the
  `GatewayCaller` context (item 7, shipped) attributes: one identity scheme for durability receipts,
  authorisation evidence and resource accounting (§6.7 RA1).
- Wire: identities travel in the payload the existing verbs already carry; **wire v12 unchanged**.

## 4. Failure vocabulary (names fixed here; types in PR 2)

`LocalApplication::{Applied, AlreadyCurrent, Superseded, Refused}` ·
`LocalDurability::{OnDisk, Buffered, Failed, NotConfigured}` ·
`ReplicaSync { persisted_by: [NodeId], missing: [NodeId] }` · `DestinationCommit { destination,
dedup: Fresh | Replayed }` · `Conflict` · **`DeliveryUnknown`**. Every verb on the receipt path
returns one of these or a typed error; `bool` survives only on the existing verbs, deprecated where
a receipt replaces it.

**Four application outcomes, not two (review, 2026-09-15).** The store's "nothing changed" covers
three different truths: the operation *is* already the current value (an idempotent retry), a
*different, newer* value won, or the live-entry cap refused the write and the key may be absent.
Reporting all three as `Superseded` would claim a newer value had won when none had — the same class
of overclaim as the durability one below, in the rung above it.

**`Buffered` — a state this record was missing, found by implementing it (PR 2, 2026-09-15).** The
three original states assumed every write is either forced to disk or failed. That is true only under
`SyncMode::Flush` or an explicit `append_sync`: under `Async` or `Os` a successful append reaches the
OS page cache, which **survives a process crash and is lost to a power failure**. A receipt claiming
`OnDisk` there would have claimed a durability the node never established — precisely the overclaim
this axis exists to end — and one claiming `Failed` would have denied a write that is very likely
recoverable. `Buffered` says what is true, and PR 3's required-sync write is how a caller *demands*
`OnDisk` instead. The layering matches item 6's storage model (`replay-nondeterminism-inventory.md`
§5): process memory, OS page cache, durable storage, directory metadata are four different things.

## 5. Compatibility (§9's one rule; D24)

Compatible additions on 2.x, "compatible" by **Rust's** rules — and by those rules **adding a field to
`ConsensusResult::Committed { .. }` is breaking**: the variant's fields are not `#[non_exhaustive]`, so an
exhaustive destructure without `..` and every construction stop compiling (v2.4.2's own addition of
`persisted` forced exactly that `..`, CHANGELOG 2.4.2). D24's "a new representation beside the old"
therefore means a **new result-returning API**, not a new field: PR 2 adds `cluster_propose_receipt` /
`group_propose_receipt` (names indicative) returning a new `CommitReceipt { local_durability:
LocalDurability, .. }` type marked `#[non_exhaustive]` from birth, and the KV write receipts arrive
the same way on new verbs (`set_with_receipt`, …). `ConsensusResult::Committed { persisted }` is
**unchanged in shape**; `persisted` is documented as the collapsed bool and `#[deprecated]` in favour of
the receipt API; the gateway JSON gains `"local_durability"` beside `"persisted"` (JSON is additive-safe);
the SDKs expose the new state (they already model absence). Adding the field to the variant itself is
scheduled for the `3.0.0` ledger with the removal of `persisted` (§6.6). `set_with_min_acks` keeps its
signature; its *meaning* tightens in PR 4a (exact identity) with the `>=` behaviour kept behind a legacy
flag until then.

## 6. Reconciling with `exactly-once-effect.md` (D11)

That record **declined-with-evidence** a shared *in-flight tracker* code overlay across the tuple space
and the blackboard: their in-flight clocks diverge (wall-clock-ms + persisted + cross-node versus
monotonic `Instant` + in-process), and a shared kernel would couple crates with divergent evolution.
The contract there — *at-least-once + idempotent merge = exactly-once effect* — is exactly rule 1 of
§2: the substrate never provides the last rung.

The effects companion (PR 5) does not revisit that decision. It adds a **different artefact**: a
transactional *reference destination* whose dedup table is keyed by `operation_id` and committed in the
same transaction as the business change — the **destination-commit receipt**. The in-tree users prove
the pattern with consumer-specific idempotent merges (the tuple space's claim/ack/requeue in the
exactly-once work-distribution proof, `docs/wiki/dev/history.md`; the wiki's append-merge that skips an
already-contained body). None of them yields a *receipt* a third party can read, and each idempotency
is bespoke. What a SQLite destination adds is the generic, inspectable form of that last rung: the
transaction boundary, the dedup row, and the receipt that says which one this attempt was. It is
justified where the tracker overlay was not because it does not couple the two companions — it sits
*beyond* both, at the resource that the caller already trusts (posture rule 3(ii)).

## 7. Mapping the existing at-least-once primitives into the vocabulary

| Primitive | Today's ack | In the vocabulary |
|---|---|---|
| `emit_reliable` | `Acknowledged` / `Timeout` | an application-level *receipt of handling*; `Timeout` ⇒ `DeliveryUnknown` |
| mailbox `deliver_event` → `open_mailbox` | `bool` queued; the drain delivers | local application at the sender; durability = the mailbox key's local sync on a persisted node; delivery = the target's drain (a later local application there); no receipt crosses back |
| tuple-space `take` / `complete` / requeue | claim under lease; atomic lane move | `take` = local application at the primary (+ local sync via its WAL); `complete` = the **pipeline's own** receipt — the item was acknowledged and the next stage queued, atomically, at the primary (local application + local sync there). It neither performs nor verifies the consumer's business transaction: idempotency makes a retry *safe*, it does not prove the effect *happened*. A destination-commit receipt, when the effects companion (PR 5/6) provides one, is a separate record linked to the same `operation_id`; `complete` never stands in for it. Lease expiry = `DeliveryUnknown` resolved by re-delivery |
| `set_with_min_acks` | propagation count `>=` | today: replica *propagation* evidence, not replica sync; PR 4a: exact-identity; PR 4b: replica sync proper |
| `Committed { persisted }` | bool | local application (cluster-committed) + a collapsed local-sync receipt; PR 2 adds a receipt-returning propose API beside it (§5) — the variant's shape does not change on 2.x |

## 8. The regression floor (this PR, code)

Executable pins of *today's* semantics. A later PR changes a meaning by changing a pin, in the open.

| Pin | Where | What it holds |
|---|---|---|
| `floor_observe_counts_any_update_at_or_after_write_ts` | `src/agent/kv_quorum.rs` | the `>=` propagation semantics of the min-acks tracker — the overclaim PR 4a replaces |
| `floor_committed_persisted_is_true_when_persistence_unconfigured` | `src/lib_tests.rs` | `persisted: true` with no persistence configured — the D24 collapse PR 2 lifts |
| `consensus_commit_reports_persisted_and_survives_restart` (existing) | `src/lib_tests.rs` | `persisted: true` means the fsynced WAL append |
| `regression_snapshot_retains_wal_record_acked_before_local_apply`, `regression_writer_threshold_snapshot_right_after_ack_keeps_write` (existing) | `mycelium-core/src/persistence.rs` | the WAL-tail merge that makes persist-first admissible (§2.2) |
| `regression_closed_writer_never_acks_success`, `regression_writer_dying_mid_request_is_an_error`, `append_sync_fdatasyncs_in_async_mode` (existing) | same | an ack is a durability claim |
| `golden_fixture_replays_every_released_on_disk_format` | same | §9 |
| `set_with_min_acks_zero`, `set_with_min_acks_timeout_no_peers` (existing) | `src/agent/kv_handle_tests.rs` | the timeout is an error and the write is not rolled back |
| `receipt_tests::*` (PR 2) | `src/lib_tests.rs` | the receipt path: an unpersisted node promises nothing; `Buffered` ≠ `OnDisk`; a late retry is `Superseded` and does not clobber the newer value (D11's rationale, executable); an *identical* retry is `AlreadyCurrent`, not `Superseded`; two retries from one receipt get distinct attempt identities; a retry reuses its stamp; same identity + different content is a `Conflict` that writes nothing; an oversized write is `Rejected` before it applies; a commit receipt separates agreement from local durability and reads a timeout as `DeliveryUnknown` |
| `a_required_sync_write_establishes_disk_even_in_async_mode`, `a_refused_required_sync_write_leaves_nothing_visible`, `a_required_sync_retry_keeps_the_stamp_and_refuses_changed_content` (PR 3) | `src/lib_tests.rs` | the strong path forces `OnDisk` whatever the node's `SyncMode`; a refused required-sync write leaves **nothing visible** — no store entry and no subscriber, which is what persist-first buys and the ordinary path cannot promise; retries keep the stamp and refuse changed content |
| `regression_append_acked_never_acks_a_dead_writer` (PR 2) | `mycelium-core/src/persistence.rs` | the receipt path's append awaits an acknowledgement: a dead writer is `Failed` in every sync mode, never `Buffered`. `append`'s fire-and-forget `Ok` in `Async`/`Os` is asserted alongside — it is why the receipt path cannot use it |
| `content_hash_golden_vectors` (PR 2) | `mycelium-core/src/receipt.rs` | the receipt's content hash is FNV-1a/64 over a specified canonical encoding, pinned by literals verified against an independent implementation — a receipt travels between builds, so a drifting hash would raise false `Conflict`s |

## 9. Golden on-disk fixtures (V2, this PR)

`tests/fixtures/persistence/<format>/{wal.bin, snapshot.bin, expected.json}` — real files written by
the writer of a released format, replayed in CI by `golden_fixture_replays_every_released_on_disk_format`
through the production `replay` + LWW apply path, and checked against `expected.json`. Every released
format gets a directory; the test walks all of them. Today there is **one** family, `fixint-v1`: the
`serde_fixint` (bincode-fixed-int-identical) encoding of `KvSnapshot { snapshot_hlc, entries }` and the
`u32le`-length-prefixed `SyncEntry` WAL records, unchanged since persistence shipped in v1.0.0 (the
bincode → `serde_fixint` swap was byte-identical, pinned by `layout_is_pinned_by_goldens`). The fixture
exercises what the invariants protect: a key present in both snapshot and WAL with LWW deciding, a
WAL-only key whose HLC is *older* than the snapshot watermark (invariant 2), and a tombstone.

When item 1 changes the format (a record with identities, PR 2/3), the writer of the new format adds
`fixint-v2/…` and the old directory stays: every released file must still replay. The fixture is
regenerated only by the ignored test `regenerate_golden_fixture_fixint_v1`, run by hand, never in CI.

## 9a. Retained operation status — deferred, and why *(PR 3, 2026-09-15)*

The plan pairs "required local sync" with **retained operation status**: a node remembering an
operation's outcome so a retry can be answered from the record instead of re-executed. The
required-sync half is built; the retention half is **deliberately not**, because implementing it as
specified would cut against §6.

A per-node map from `operation_id` to its receipt is *destination-side dedup wearing a different
hat*. §6 places that rung at the destination on purpose: the effects companion's reference
destination commits the business change **and** its dedup row in one transaction, which is what makes
the effect exactly-once. A substrate-level registry cannot do that — it is not in the destination's
transaction — so it would answer a retry "already done" while the destination's own state remains
unknown. That is a higher rung claimed from a lower one, the failure this whole record exists to
prevent.

It also needs decisions a receipt PR should not make alone: how long a node must remember an
operation, what bounds the memory, what eviction means for a caller whose retry arrives after it
(silently re-executing is the honest answer, which is exactly what happens today without a registry).

**Open question, not omission.** If retained status earns its place it will be as an explicit,
bounded, opt-in facility whose receipts say *this node remembers doing it*, never *the destination
did it* — and it belongs next to PR 5's destination work, where the rung it serves actually lives.

## 10. What this record does not decide

The receipt types' exact shapes (PR 2), the required-sync API (PR 3), the peer-persistence protocol
(PR 4b), the destination schema (PR 5) — each PR carries its five-part statement. And the substrate
still never decides truth: a receipt is evidence a reader interprets; acceptance is the reader's
(Property 7, extended to evidence by item 3).
