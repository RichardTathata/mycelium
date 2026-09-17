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

**The debt, measured: 193 sites across 43 files** — after the alias fix. `hlc.rs` has left the
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

**The payoff, earlier than expected.** `snapshot_install_syncs_the_directory` opens by saying the
power-loss property "is not observable without a filesystem adapter — the replay plan's point", and
could only pin the wiring. With the adapter it is a **sequence in the trace**: write the temp file,
sync its bytes, rename it into place, sync the *directory*, then truncate the WAL. The new test
asserts those five in order, and — checked by reversing the production code — **fails when the
directory sync moves after the truncation**, printing the offending trace:

```
[("snapshot.tmp","write"), ("snapshot.tmp","sync_data"), ("snapshot.bin","rename"),
 ("wal.bin#truncate","sync_data"), ("dir","sync_dir")]
```

That reversal *is* v2.4.4: a power loss between the truncation and the directory sync leaves the old
`snapshot.bin` beside an empty, fsynced `wal.bin`, and every acknowledged record since the previous
snapshot is gone. A test asserting only "fsync_dir was called" passes on it. This is the first time
the fix has had a test that could have caught the bug.

**The RNG seam.** `ops.rs`'s three nonces and its shedding roll now draw from the named streams
`nonce` and `shed`. Two properties are pinned through *production's* path rather than the kernel's
API: a nonce draw does not move a shedding roll (the reason the streams are named at all), and a roll
is never `1.0` — `fastrand::f32()` is in `[0,1)` and the shedding code relies on it, since a fill of
`0.0` must always shed. A seam may change *where* a number comes from; it must not quietly change
what kind of number it is. `ops.rs` has left the baseline: 4 → 0.

**Next:** `persistence.rs`'s remaining 10 sites, `connection.rs` (~9 `Instant::now`), `tasks.rs`
(~6 `fastrand` + ~6 timers), then PR 4's storage model and the WAL/snapshot witness.

**Gates.** `make check` clean (the forbidden-call check inside it); core 179 without `sim`, 183 with;
`make check-full` and CI now build and test the `sim` path.
