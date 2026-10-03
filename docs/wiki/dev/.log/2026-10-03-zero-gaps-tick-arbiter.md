## [2026-10-03] ingest | zero gaps Z5 — the scheduler seam's second arm

**What:** `Kernel::next_is`, `Seams::timer_if_next`, `installed::{timer_replay_if_next, position}`,
`sim_seam::wait_turn` (bounded by `ARBITER_STALL_ROUNDS`), the replay `Ticker`'s absolute `next_due`
and post-check-in state; `two_equal_period_tickers_replay_in_the_recorded_order_whichever_wakes_first`
(core) and `a_recorded_node_replays_decision_for_decision` (whole node). Plan D5.

**Durable knowledge:** the divergence class was *two tasks runnable at one paused instant*, which no
wait-ordering arm can touch — tokio's wake order is not a wait. The fix is an arbiter that asks the
trace "am I next?" and defers if not, rather than a scheduler that chooses: the recording already
chose. The bound matters: a deferred tick that never becomes next must still diverge with both sides
named, or a genuinely different run hangs. And a yield loop blocks tokio's paused-clock auto-advance,
so every few rounds the wait must be a sleep, not a yield.
