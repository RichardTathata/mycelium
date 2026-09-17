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

**Three bugs in the checker, all found by testing the checker.**

1. It excluded *everything after the first `#[cfg(test)]`*, so a live `Instant::now()` appended below
   a test module passed. Fixing it to skip each test *item* raised the count 165 → 190: twenty-five
   sites silently ignored.
2. It counted **comments**. The doc comment explaining why `hlc.rs` no longer calls `SystemTime::now`
   matched the pattern for `SystemTime::now`, so routing the seam appeared to change nothing.
   Excluding comment lines dropped the count 190 → 185 and made `hlc.rs` leave the baseline entirely
   — which is what routing a seam is supposed to look like.

3. It did not follow **aliased imports**. `persistence.rs` does `use tokio::{fs as tfs, …}` and then
   writes `tfs::read(…)`; the check matched only `tokio::fs::`, so the module the inventory calls
   *"the right first target"* — 26 fs references — was **entirely invisible**. The check reported
   "clean" on the single most important file in its scope. Following `fs as <alias>` imports and
   every use of the alias added 14 sites, and surfaced 2 more in `lifecycle.rs`.

All three were found the same way: plant a site, check the checker fails; write a comment, check it
does not; route a real seam and check the count actually moves. *A checker nobody has watched fail is
a checker nobody knows works* — and one nobody has watched **pass** on a near-miss will cry wolf
until it is ignored.

**The arithmetic is the point.** The check read 165 sites when it first went green. It now reads
**199**. Every one of the three bugs was wrong in the *safe* direction — under-reporting — which is
how a tool quietly stops meaning anything while still passing. Thirty-four sites, about a fifth of
the total, were invisible to a check whose entire job was to see them.

**The debt, measured: 199 sites across 44 files** — after the alias fix. `hlc.rs` has left the
baseline entirely, and `persistence.rs`'s WAL write path is routed even though its other sites
remain.

**The storage seams, and what they do not yet do.** The WAL's `write_all`, its `sync_data` and the
directory `sync_all` (the v2.4.4 effect) are kernel effects now. Their *order* is the durability
property, so **dropping the sync is a divergence** — caught by the trace before PR 4's storage model
can simulate the loss it would cause. A replayed write does not touch the disk: the mode is checked
before the `await`, or a replay would mutate state the recording already accounted for.

What they do **not** do: model storage state. A replay checks the sequence and content of effects; it
does not reconstruct the disk, so it cannot yet answer *what would a reader see after a power loss*.
That needs the three-layer model (process memory · page cache · durable · directory metadata) and the
fault injection of PR 4 — because *process death is not power loss*, and the harness must not
manufacture a loss a process kill cannot cause nor certify durability only a sync establishes.

**Next:** `persistence.rs`'s remaining 12 sites, `connection.rs` (~9 `Instant::now`), `tasks.rs`
(~6 `fastrand` + ~6 timers), then PR 4's storage model and the WAL/snapshot witness.

**Gates.** `make check` clean (the forbidden-call check inside it); core 179 without `sim`, 183 with;
`make check-full` and CI now build and test the `sim` path.
