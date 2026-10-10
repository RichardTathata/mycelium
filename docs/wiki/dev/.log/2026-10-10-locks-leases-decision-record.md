## [2026-10-10] ingest | one record per decision (row A, after the design review on #600)

- Three review rounds broke every reading rule over `committed` + `lease` + `decided`; the cause was structural
  (independent LWW keys; HLC order is not decision order). `docs/design/lock-lifecycle.md` (rev 2, adopted)
  replaces them: Paxos decides an envelope carrying the lifecycle, one immutable record per decided ballot read by
  the highest ballot, release markers keyed by lineage, wall-clock lease expiry, explicit renewal, C2 shrinking to a
  node-owned floor. Five appended wire variants; wire v12.
- Tests: one black-box test per §9 row that both codes expose (`src/agent/lock_lifecycle_tests.rs`, six seen failing
  on the pre-record branch), record-level rows against the new API (`src/agent/lock_lifecycle_api_tests.rs`).
- Found while implementing: the prefix index drops tombstoned keys, so collection stubs lower records instead of
  tombstoning them (a tombstone would read as "never decided" and trigger the legacy fallback); a shrunk floor needs
  a sentinel proposer to refuse the ended ballot itself; the collector's first tick raced tests — it now runs one
  interval after start.
- Pages touched: `dev/architecture/runtime-invariants.md`, `dev/concurrency/lock-order.md` (row 56),
  `docs/guide/04-consensus.md`, `docs/design/lock-lifecycle.md`, `src/lib.rs` KV table.
