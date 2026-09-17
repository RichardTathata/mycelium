## [2026-09-17] ingest | item 6 PR 3 — signal.rs's clocks, and the origin the seam turned out to need

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.1 · code
`mycelium-core/src/{signal,mesh_handle,sim_seam}.rs`, `mycelium-sim/src/seams.rs`.

### The conversion

`signal.rs`'s ten interval sites onto the clock seam: the sender log, the per-kind `last_seen`, the
suppression table, the quorum-evidence rate limiter, and the reorder buffer's hold. Baseline **159
across 43 files**, from 170. `signal.rs` 11 → 1.

`MeshHandle::last_signal` changed from `Option<Instant>` to `Option<Duration>` — **the age**. Not a
concession to the representation: the age is what the only real caller computed from the `Instant`
(`mesh_handle.rs`'s advertise loop did `.map(|t| t.elapsed())` immediately), and it is what the
sibling `last_signal_persistent` has always returned. The pair is now consistent, and unlike a raw
seam reading the value means something on its own.

### A suspected bug that was not one

`SignalLog::trim` computes `Instant::now() - window`, and `impl Sub<Duration> for Instant` panics on
underflow. With `signal_window_secs` defaulting to **600** and `Instant` being `CLOCK_MONOTONIC`
(time since *boot* on Linux), this looked like a reachable panic on any node started within ten
minutes of boot — a systemd-managed node, a container on a fresh host, a CI runner — and the panic
would be inside the health-check task, which a `tokio::spawn` swallows, so the symptom would be *the
entire health-check loop silently dying*.

**It is not real.** Probed directly rather than reasoned about: `Instant::checked_sub` returns `Some`
on this platform even for `u64::MAX / 2` seconds. Rust's `Instant` is internally signed/offset on
both Linux (`Timespec { tv_sec: i64 }`) and macOS, so "before boot" is representable and the
subtraction never underflows for any realistic window.

Recording it because the reasoning was sound and the conclusion was wrong, and the only thing that
separated them was running it.

### What that same property meant for the conversion

The probe answered a different question than the one it was asked. **`Instant` can represent a point
before process start; a bare `u64` of nanoseconds since process start cannot.** That is not academic:

`SignalLog::seed` reconstructs a sender-log entry that arrived `age_ms` ago, from a `sys/quorum/`
record, and `warm_quorum_from_layer1` calls it **at startup** — when the process is milliseconds
old. Counting from zero would clamp every seeded entry to the run's origin, so warmed quorum evidence
would look brand new and a quorum built on stale evidence would read as live, **in the one moment the
mechanism exists for**.

So `sim_seam::MONO_ORIGIN_NS` — one year of headroom, more than any configured window, leaving ~583
years of runtime in a `u64` of nanoseconds. `mycelium-sim`'s `Sources` starts at the same origin: a
harness that disagreed with production here would disagree in the direction that *hides* the bug.

### The test that did not exist

`seed_sender_log` had no test at all, in either representation. Zeroing the origin left all 181 core
tests green.

It has one now — `a_seeded_entry_keeps_the_age_it_was_seeded_with`: seed at 500 ms, assert the entry
counts inside a 5 s quorum window and **does not** count inside a 200 ms one. A clamped seed passes
the first and fails the second. Verified by zeroing `MONO_ORIGIN_NS` and watching exactly that
assertion fire.

### Coverage

Breaking `mono_between` → `ZERO` fails 4 tests, → `MAX` fails 7 (the reorder buffer's four, SWIM
suspicion, `quorum_distinct_senders`, the reconnect backoff). The origin is pinned by the new seed
test and by `the_monotonic_origin_leaves_room_to_point_before_the_run_began` (sim-gated, which CI
runs).

### Gates

`make check` clean · core **183** / **207** (sim) · `mycelium-sim` 24 + 6 · mycelium **522**
(`compliance,a2a`) / **403** (`sim`).
