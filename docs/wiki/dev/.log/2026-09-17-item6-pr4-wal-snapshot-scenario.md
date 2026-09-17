## [2026-09-17] ingest | item 6 PR 4 — the WAL/snapshot scenario, and the two bugs replaying it found

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.4, §2.5 · plan
`docs/plans/v3-contracts-axis.md` §4 · code `mycelium-core/src/{persistence,sim_seam}.rs`.

### What was asked for

§4: *"First scenario: the WAL/snapshot race as a controlled schedule with a merge-removed witness
that must fail"*, and the Phase A exit gate: *"the WAL/snapshot race replays from a bundle and its
merge-removed witness fails"*.

The race itself was already pinned — `regression_snapshot_retains_wal_record_acked_before_local_apply`
puts a record in the WAL that its caller has not yet applied to the store, snapshots, and shows it
survives the truncation. **I initially claimed it had no test at all; that was wrong, and the claim
came from reading the wrong test module** (`persist_tests` opens `wal.log`, so *its* merge runs
against an empty tail; the real coverage is in `durability_tests` against `wal.bin`). Corrected
before it reached a commit message.

What genuinely did not exist is the other half: a **witness**, and a **replay**.

### The witness

A `cfg(test)` toggle, not a hand edit — the plan is explicit, and the reason is good: the 2026-09-05
fix (v2.4.3) was verified by hand-disabling the merge, and a hand edit cannot be named in a bundle,
re-run by someone else, or used to show the fix still holds. Thread-local rather than a global flag,
because `cargo test` runs tests concurrently in one process.

`the_merge_removed_witness_loses_the_acknowledged_record` is the exact inverse of the regression
test above, and it is what a bundle names.

### The two bugs that replaying it found

Neither was reachable by reading the code. Both came from running the scenario through the kernel.

**1. Replay suppressed writes, so a run could not read its own writes.** The rule was *"a write's
bytes are its request, so the kernel already knows them and the replay can skip the effect"* — sound
until a run reads back what it just wrote. `do_snapshot` reads the WAL tail that `wal_append` wrote
earlier in the *same* run; the bundle's `initial/` image restores the state *before* the run, so the
suppressed write left the tail empty and the replay diverged against its own recording. The seam now
performs the effect in both modes and lets the kernel decide the **outcome** — which is also what
makes an injected fault replay as a fault rather than as whatever the replay's disk happened to do.

**2. An effect's request embedded absolute paths.** `fs_rename` recorded
`/tmp/myc-run-123/snapshot.tmp->/tmp/myc-run-123/snapshot.bin`, so the request was run-specific and a
bundle could only ever replay in the directory that produced it — which defeats the point of a
bundle. File names only; both ends are still recorded, so "a rename that moved a different file" is
still a different effect.

### The bug the replay found that had nothing to do with replay

With those fixed the scenario replayed — and then failed **half the time**, with snapshots of
identical length and different content hashes. The cause:

> `do_snapshot` built `entries` by iterating the store, so **the snapshot file's bytes depended on
> papaya's iteration order**. The store's *hasher* is seeded (`store.rs`,
> `RandomState::with_seeds(1,2,3,4)`); its *iteration* is not.

The replay consequence is the small half. The real one: **two nodes holding identical logical state
wrote byte-different snapshot files**, so any byte-level comparison of snapshots — a checksum, a
dedup, a fixture diff — was unsound. §2.5 names hash iteration order as a nondeterminism source and
says each order-sensitive consumer must be listed; this encoder was one and was not listed.

Fixed by sorting entries by key before encoding: the file becomes a function of the state it
represents. Nothing reads a snapshot positionally (replay folds it in under LWW), and old snapshots
still replay — only what is *written* changes, which the golden-fixture test confirms.

**The gate that found it is worse than the gate that now guards it.** The replay caught it 4 times
in 8; `a_snapshot_is_byte_identical_for_the_same_state_whatever_order_it_was_built_in` inserts the
same keys in two different orders and catches it 4 times in 4. A flaky detector is a detector that
teaches people to re-run, so the flaky one was replaced rather than kept.

### Gates

`make check` clean · core **186** / **212** (sim) · `mycelium-sim` 24 + 6 · mycelium **522**
(`compliance,a2a`) / **403** (`sim`).

### What PR 4 still owes

The **fault sweep** (§4 lists "scenario + fault sweep + witness"). The seam change above is its
precondition — a recorded `FsOutcome::Err` now replays as an error — but sweeping a fault across each
of the five ordered install effects, and asserting no outcome loses an acknowledged record, is not
written yet.
