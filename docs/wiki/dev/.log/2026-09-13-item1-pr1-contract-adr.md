## [2026-09-13] ingest | v3 item 1 PR 1 — the contracts-and-receipts ADR, regression floor, golden fixtures

Up: [dev](../dev.md) · pages: [runtime-invariants](../architecture/runtime-invariants.md) §Persistence,
[testing](../testing/testing.md) · record `docs/design/contracts-receipts.md` · plan §3, D8/D9/D10/D11/D24.

**Finding.** Every public acknowledgement reads as success and none says the same thing: `kv().set`
proves *queued for gossip*; `set_with_min_acks` proves *propagation* and counts a newer overwrite as an
ack (`kv_quorum.rs::observe`, `>=`); `Committed { persisted }` folds *fsynced* and *never promised*
into one `true`; `emit_reliable`'s `Timeout` is not "not delivered"; the tuple lease re-queues and a
second worker acts again. And because every write site applies before it persists, a durability
failure never means nothing happened. None of this was written in one place, so nothing could pin it.

**Change.** The ADR states the contract PRs 2–4 implement — four receipts kept strictly separate, each
answering *visible when · durable when · may effects run before durability · what remains true after a
sync failure* on its own; `operation_id` (caller-minted, pre-dispatch, stable across retries — retries
re-submit the same stamped update) and `attempt_id`; `Conflict`; `DeliveryUnknown`; apply→persist as
the invariant with persist-first admissible only because the snapshot's WAL-tail merge holds (pinned by
two existing durability tests); D24's additive `local_durability` beside a deprecated `persisted`; the
D11 reconciliation — the effects companion adds a *destination-side* transactional dedup and receipt,
beyond both companions, which is why it is justified where the shared in-flight tracker overlay was not.
Code (no public type changes): two `floor_*` pins of today's semantics, and the V2 golden fixtures —
`tests/fixtures/persistence/fixint-v1/` written by the current writer (format unchanged since v1.0.0;
the bincode → `serde_fixint` swap was byte-identical), replayed in CI through the production path,
holding an older-than-watermark WAL record and a tombstone. Docs: concepts vocabulary, philosophy
Property 8 + litmus tests 4–5 (§12.5 delivered with this ADR), `building-on-mycelium.md`'s one
compatibility rule, CLAUDE.md's ack invariant.

**External review before merge (2026-09-14), three P2 corrections, all taken.** (1) `Failed` is *durability not
established*, never *absent*: `wal_append` writes before `sync_data`, so a failed sync may leave the bytes on
disk and a later replay may restore them — the ADR's local-sync row and §1 now say so, and the
`Committed { persisted: false }` rustdoc that claimed "not in this node's WAL" is corrected in the same PR.
(2) Tuple-space `complete` is the *pipeline's* receipt (acknowledged + next stage queued, atomically at the
primary); it neither performs nor verifies the consumer's business transaction, so it never stands in for a
destination-commit receipt — the two are separate records linked by `operation_id`. (3) Adding
`local_durability` beside `persisted` inside `ConsensusResult::Committed` is **breaking** under Rust's rules
(non-`non_exhaustive` variant fields; v2.4.2's `persisted` addition already forced `..`): D24's "new
representation beside the old" is read as a **new receipt-returning API** (`*_propose_receipt`, a
`#[non_exhaustive]` `CommitReceipt`), the variant unchanged on 2.x, the field addition scheduled with the
`3.0.0` ledger.

**Kept deliberately.** The floor pins the *overclaim* on purpose: PR 4a must flip
`floor_observe_counts_any_update_at_or_after_write_ts`'s last assertion, and PR 2 must extend
`floor_committed_persisted_is_true_when_persistence_unconfigured` with `NotConfigured` — meaning changes
happen by changing a named test, never silently.
