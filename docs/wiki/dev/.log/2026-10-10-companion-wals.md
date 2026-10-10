## [2026-10-10] ingest | Companion WALs: poison, torn vs corrupt, one owner, durable compaction (row C)

- **The defects** (360 review, verified): both companion WAL writers (`mycelium-tuple-space/src/store.rs`,
  `mycelium-blackboard/src/wal.rs`) carried on after a failed `write_all`, so the next acknowledged record landed
  behind a torn frame and the next open — which `break`s at the first undecodable record and `set_len`s there —
  discarded it silently; any corrupt middle record did the same to everything after it; neither file had an owner
  lock; compaction skipped the directory fsync (tuple space) or both syncs (blackboard: `std::fs::write` + `rename`).
- **The fix** mirrors the core (`mycelium-core/src/persistence.rs` `WriterState` / `WalEnd`, `src/agent/journal.rs`):
  `OwnershipLock` (now re-exported as `mycelium::OwnershipLock`) taken before the file is read; `WalInner::poison`
  set by a failed append, cleared by a successful compaction (which `wants_compaction` requests while poisoned — the
  analogue of the core's repairing snapshot); `scan_frame` → `Record | Torn | Corrupt`, a torn final frame truncated
  and synced with its directory, a corrupt frame with data after it refusing the open untouched; compaction in the
  snapshot's install order. No quarantine switch (the companions' configs carry no `on_unreadable`).
- **Limit:** no per-record checksum, so a length prefix corrupted to run past EOF still reads as a torn tail.
- **Found on the way, then fixed:** in both companions a write appends to the WAL and *then* updates memory, while
  compaction rewrote the log from a memory snapshot — a record appended between the two was absent from the
  rewritten log, so a crash after that compaction lost an acknowledged record. Compaction now folds the log itself
  (`fold_wal`, shared with open) under the WAL lock alone; memory-first writes (take, ack, claim, release) append to
  the new file afterwards. The tuple space's compaction no longer nests stage/in-flight locks inside the WAL lock,
  and `inflight_snapshot` (its only user) is gone — [lock-order](../concurrency/lock-order.md) rows 25 and 28.
- Pinned by five tests per crate, each seen failing first. Pages touched: `dev/companions/tuple-space.md`,
  `dev/companions/blackboard.md`, `dev/concurrency/lock-order.md`; outside the wiki
  `docs/design/exactly-once-effect.md`, `docs/operations/companions.md`, `docs/operations/deployment.md`.
- **The PR's adversarial review (#597):** a regression — the poison was cleared only by compaction, which only the
  primary ran, so a poisoned *secondary* refused every later mirrored record while `apply_records` / the replicate
  handler had already marked the ids seen (failover lost everything after the error). Fixed: maintenance spawned in
  `init_store` for any persistent store (`spawn_wal_maintenance` / the board's equivalent), a refused mirror apply
  un-marks the id and sets `mirror_gap` (the backfill loop stays alive and re-drains on it), `remove_queued` counts an
  ack only when logged. Also: Auto-mode `init_store` refusals logged and the candidacy withdrawn
  (`withdraw_after_store_refusal`); valid-kind-then-zeros is torn (and the zero-filled-body phantom decode stated as a
  limit); repair back-off in `WalInner::{repair_failures, repair_skip}`; `maintenance_tick` runs the repair even when
  the sync fails; temp path `<wal>.compact`; compaction keeps a lone `Ack` for the id high-water mark, and `fold_wal`
  counts an `Ack` as terminal only for an item it holds. No new lock (one `AtomicBool` per handle).
