## [2026-09-17] ingest | item 6 PR 3 — the monotonic-clock seam, and a test that should have existed

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.1 · code
`mycelium-core/src/sim_seam.rs`, `writer.rs`, `connection.rs`.

### The seam

`sim_seam::mono_now_ns` and `mono_since`. The kernel side already existed — `Seams::mono_now_ns`
has been in `mycelium-sim` since PR 2 — and had no production caller.

**It is a second function rather than a use of `wall_now_ms`, and that is a correctness property
rather than a preference.** Every site it replaces measures an *interval*: how long since the rate
window opened, how long since the last connect failure. `Instant` is monotonic, so a backwards NTP
step cannot make an interval negative or enormous; `SystemTime` gives no such guarantee. Routing
these through the wall clock would have been one function fewer and a new class of bug — a rate
window that never expires, a backoff that fires instantly.

`Instant` has no epoch, so the seam supplies one (the first read), which is what `Seams::mono_now_ns`
already meant by "since the run began". `mono_since` replaces `Instant::elapsed` and **saturates**:
"earlier is actually later" cannot happen, and a wrapping subtraction would express that
impossibility as five centuries elapsed — which a cooldown reads as long expired.

### Scope, stated rather than implied

Three kinds of `Instant` live in the inventory's §2.1 row, and only one is this seam's:

1. **Function-local elapsed timers** — `writer.rs`'s `last_fail`, `connection.rs`'s
   `rate_window_start` and `last_state_sent`. Converted in place. **Done.**
2. **`tokio::time::Instant` deadlines** — `writer.rs`'s `idle_deadline`, fed to `sleep_until`. The
   **timer** seam's, not this one: converting the reading without owning the sleep leaves the
   deadline deterministic and the wait still real.
3. **`Instant` in a shared type** — `Arc<papaya::HashMap<NodeId, Instant>>`, the peer table's
   last-heard-from stamp, in `ConnContext`, `SwimState`, `TaskContext` and their tests. A type change
   across two crates; its own increment, not a rider on this one.

Baseline **179 across 43 files**, from 186. `connection.rs` 6 → 2, `writer.rs` 6 → 3.

### What the conversion exposed

**The reconnect backoff had no test in `mycelium-core` at all.** Breaking `mono_since` to return
zero left all 180 core tests green and failed exactly *one* test in the outer crate —
`an_acknowledged_replica_still_holds_the_record_after_restart` — by a route with nothing to do with
reconnecting. Had that conversion been wrong, the bug report would have been about replica restarts.

So the seam's first call site got the test its absence made obvious:
`the_reconnect_backoff_drops_frames_while_it_runs_and_stops_once_it_expires`, verified against
**both** failure directions — `mono_since → ZERO` (a backoff that never expires, which drops
everything) and `mono_since → MAX` (one that never applies). A gate checked in only one direction
catches only one kind of mistake, and these two are different mistakes.

The general form is worth keeping: **converting unguarded code is not a safe refactor, it is an
untested change to production behaviour**, and the honest cost of the conversion includes writing
the test that should already have been there.

### Gates

`make check` clean · core **181** / **204** (sim) · mycelium **521** (`compliance,a2a`) / **402**
(`sim`).
