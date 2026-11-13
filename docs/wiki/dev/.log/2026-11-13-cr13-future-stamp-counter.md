## [2026-11-13] ingest | an inbound KV entry stamped beyond the drift bound is counted (CR13, step 1)

The drift clamp bounded a node's clock and counted nothing, so nobody knew how often a healthy fleet would
trip the refusal the critique plan's row CR13 proposes as its second step. Step 1 is the counter alone: the
three inbound doors count an entry the clamp caught and apply, forward and persist it as before.

- Pages touched: `dev/architecture/runtime-invariants.md` (the future-stamp tripwire, under the detection
  posture), `dev/operations.md` (the `/stats` row).
- Outside the wiki: `docs/operations/observability.md`, `metrics.md`, `what-is-proven.md` (a new row: detected,
  not prevented), `docs/features/gossip-kv.md`, `CHANGELOG.md`, the critique plan (CR13's State line).
- Code: `mycelium-core/src/hlc.rs` (`beyond_drift_bound`, `Hlc::observe_reporting`; `observe` is its first
  half), `bounds.rs` (`StampDoor`, `TransportBounds::count_future_stamp`), `connection.rs` (the three arms),
  `sim_seam.rs` (`pin_wall_clock_for_test`, test-support only), `src/agent/http/node.rs`, `introspect.rs`,
  `src/agent/mod.rs` (`SystemStats::kv_future_stamps`).
- Why the verdict comes from `observe`: a separate check would read the wall clock a second time per inbound
  entry — a second recorded decision under replay, and a read that can disagree with the clamp's.
- Enumeration (where a remote stamp enters): the three arms, counted; WAL and snapshot replay
  (`src/agent/lifecycle.rs`), not counted — step 2's fourth door; a signal's `hlc_seq` and a consensus
  envelope's token, clamped by `observe`, not KV entries; every other `apply_and_notify` site stamps from the
  node's own clock or re-uses a stamp the store already holds. Replica sync and bulk transfer apply no entry.
- Observed, for step 2: a receiver more than the bound behind counts the fleet's ordinary entries at every
  door (`a_node_whose_clock_is_behind_counts_the_fleets_ordinary_entries`).
- Not built: step 2 (refusal or local non-admission), a count at replay, a node-local repair.
- #782's review (same day): the count also rises for honest senders with good clocks — a node that has heard a
  stamp at or past its limit with a full logical counter stamps its next local write just past it (with a
  lower counter, on it: counted by a peer even a millisecond behind) — and a stamp exactly on the limit with a
  full counter does this uncounted. Recorded as a third cause on every page that gave two, as a fifth exit case for step 2 in the
  plan row, and as tests of the existing clock behaviour (`hlc.rs`, `lib_tests.rs`). Also stated: a held entry is
  recounted by anti-entropy while its bucket differs; a member can raise the count at will; a frame refused
  before the clamp is not counted. `StampDoor` is `#[non_exhaustive]`.
