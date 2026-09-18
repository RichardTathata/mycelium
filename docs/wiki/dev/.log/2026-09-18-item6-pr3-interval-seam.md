## [2026-09-18] ingest | item 6 PR 3 tail — the timer seam's second arm: intervals

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.3 row 2 · code
`mycelium-core/src/sim_seam.rs` (`interval_ms`, `Ticker`), `mycelium-sim/src/seams.rs` (`Seams::timer` gains
`op`) · gate `scripts/check-sim-seams.sh`.

### What landed

`Ticker::tick()` as a drop-in for `tokio::time::Interval::tick`, through the same kernel `Timer` choice as
`sleep_ms`. The recorded decision is the nominal schedule — an immediate first tick, then one period each — so a
replay ticks the loop's written cadence and never wall-waits. Nine `src/agent` tickers routed, one stream per
loop.

### Two things worth carrying forward

1. **A tick and a sleep are different decisions with the same duration.** The kernel's timer request now names
   the operation, and a recorded sleep replayed as a tick diverges. Without that, a trace could replay a periodic
   loop from a one-off wait — the graph would look right and the loop would run once.
2. **The forbidden-call check has a second alias blind spot.** `use tokio::time; … time::interval(…)` is
   invisible to its `tokio::time::*` pattern — the exact gap its header already closes for `fs as <alias>`. Three
   of the nine routed tickers had never been counted, which is why the baseline moved for only four files. The
   tell, again, was the baseline not moving where a site had. Recorded in the script's header; closing it
   regenerates pre-existing sites across the tree, so it is its own change.

### Not routed here

`membership_governor.rs` (in the item 4 stack — its last forbidden site is this ticker), `swim.rs`, and the
four `mycelium-core` tickers (`persistence` snapshot, `kv_persist`, `mesh_handle` ×2).

### Pages touched

- [history.md](../history.md) — a paragraph under the item 6 PR 3 section.
- The inventory's §2.3 row 2 is canon and was updated in the same PR.
