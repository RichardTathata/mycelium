## [2026-09-17] ingest | item 6 PR 3 — the first replay seam, and two bugs in the checker

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.1, §6 · code
`mycelium-core/src/sim_seam.rs`, `mycelium-core/src/hlc.rs`.

**The seam.** The HLC's single wall-clock read now goes through `sim_seam::wall_now_ms()`. The
inventory calls it "the clean seam", and it is also the highest-leverage one: every HLC tick depends
on it, so every write's LWW rank does.

**The rule the shape follows.** Without the `sim` feature, `sim_seam` compiles to the call it
replaced — no branch, no kernel, no indirection. A harness that made production pay for its existence
would deserve to be refused on those grounds, and this one does not ask.

**Why a thread-local rather than a parameter.** Threading a kernel handle through `Hlc::tick` would
put it in the signature of every caller of every clock read — hundreds of sites, most of which have
no idea time is involved. The kernel is per-run and single-threaded by construction, so a
thread-local is the honest representation of what it already is. The cost is that a call which
*forgets* the seam is invisible rather than a compile error — which is exactly why the forbidden-call
check exists, and why it landed first.

**No kernel installed falls back to the real clock.** Most tests here never install one, and they
must keep working unchanged: 179 core tests pass without `sim`, 183 with it.

**Two bugs in the checker, both found by testing the checker.**

1. It excluded *everything after the first `#[cfg(test)]`*, so a live `Instant::now()` appended below
   a test module passed. Fixing it to skip each test *item* raised the count 165 → 190: twenty-five
   sites silently ignored.
2. It counted **comments**. The doc comment explaining why `hlc.rs` no longer calls `SystemTime::now`
   matched the pattern for `SystemTime::now`, so routing the seam appeared to change nothing.
   Excluding comment lines dropped the count 190 → 185 and made `hlc.rs` leave the baseline entirely
   — which is what routing a seam is supposed to look like.

Both were found the same way: plant a site, check the checker fails; write a comment, check it does
not. *A checker nobody has watched fail is a checker nobody knows works* — and a checker nobody has
watched **pass** on a near-miss is one that will cry wolf until it is ignored.

**The debt, measured: 185 sites across 43 files.** One down.

**Next in PR 3:** the storage adapters — `persistence.rs` (~25 `tokio::fs` calls, the inventory's
"right first target"), then `connection.rs` (~9 `Instant::now`) and `tasks.rs` (~6 `fastrand` + ~6
timers). Storage is a bigger change than the clock: it needs the three-layer model (process memory ·
page cache · durable · directory metadata) the inventory sets out, because *process death is not
power loss* and the harness must not manufacture a loss a process kill cannot cause.

**Gates.** `make check` clean (the forbidden-call check inside it); core 179 without `sim`, 183 with;
`make check-full` and CI now build and test the `sim` path.
