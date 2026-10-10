## [2026-10-10] ingest | the implementation review of row A (PR #600)

- D1 (renewal decides on the full report set), D2 (no set-aside by `decided`), D3 (Layer I scope index for
  `consensus/life/{slot}/`, a read index; collection filters before scanning; measured 136.7 ms → 1.35 ms per read at
  5,000 × 100), D4 (lease rounded up to the deadline; the skew bound stated, §10's direction corrected), D5 (a
  conflicting newer `CommitTerm` written and counted), D6 (the gateway's consistent read and `GET /consensus/{slot}`
  through the record; `consensus_rx` documented as a raw, lagging view), D7 (lowercase-only record keys, sorted
  collection writes, learner records to the WAL, `expired_before_grant`, the 2.31 visibility gap stated).
- Growth: stubs replaced by one per-slot sentinel; lower records tombstoned; steady state three keys per slot.
- Test infrastructure: a test-only **frame filter** in `mycelium-core` (feature `test-support`; `FrameView`,
  `KvStore::frame_filter`, checked on the receive path for `Data`, `SignedData`, anti-entropy entries and signals):
  interleaving tests withhold real frames instead of editing a node's store.
- Pages touched: `docs/design/lock-lifecycle.md` (rev 3), `dev/architecture/runtime-invariants.md`, `src/lib.rs` KV
  table (the sentinel row).
