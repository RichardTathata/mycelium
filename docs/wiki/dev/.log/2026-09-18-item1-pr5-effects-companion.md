## [2026-09-18] ingest | item 1 PR 5 — the effects companion, the fourth receipt as a reference destination

Up: [dev](../dev.md) · record `docs/design/contracts-receipts.md` §2, §6 · code `mycelium-effects/` ·
folder-note [companions](../companions/companions.md).

### What landed

A companion crate whose one job is the receipt the substrate never provides. `EffectDestination::apply` commits
the `operation_id` dedup row and the business change in one transaction; `SqliteDestination` is the reference
destination. The vocabulary is item 1's — `DestinationCommit`, `DedupOutcome::{Fresh, Replayed}`,
`content_hash` — not a second one.

### Three things worth carrying forward

1. **The dedup row and the business change are one unit, and the failure case is where that is tested.** A
   handler that writes its row and *then* fails must leave neither. Planting a commit on handler failure fails
   exactly the atomicity test. Without it a retry would find a dedup row for an effect that never happened, and
   `Replayed` would make the effect vanish.
2. **A timeout is `DeliveryUnknown`, never "nothing happened".** `apply_within` does not cancel the blocking
   apply; it may commit after the caller stopped waiting, and the test shows the retry resolving it as
   `Replayed` with exactly one business row. This is the receipts record's rule made executable: uncertainty is
   a state, not a failure.
3. **Reuse the vocabulary's hash.** `content_hash(key, value, tombstone)` is specified and golden-pinned so that
   a retry on another build hashes the same; the effect uses it keyed by the operation id. A second hash here
   would be D30's error in miniature — a vocabulary before the contract.

### Not done here

The in-process half (the plan's §13 local fiber runtime — D34); the tuple-space consumer with effect recovery
(PR 6); gateway/SDK parity (PR 7). A pooled connection: one is opened per apply, stated in the module doc.

### Pages touched

- [companions/companions.md](../companions/companions.md) — a `mycelium-effects` bullet.
- [history.md](../history.md) — the ledger section.
- `CLAUDE.md`'s companion test line.
