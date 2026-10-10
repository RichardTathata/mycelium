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
- **Found on the way, not fixed (needs a decision):** in both companions a write appends to the WAL and *then*
  updates memory, while compaction rewrites the log from memory — a record appended between the two can be absent
  from the rewritten log, so a crash after that compaction loses it.
- Pinned by four tests per crate, each seen failing first. Pages touched: `dev/companions/tuple-space.md`,
  `dev/companions/blackboard.md`; outside the wiki `docs/operations/companions.md`, `docs/operations/deployment.md`.
