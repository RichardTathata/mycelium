## [2026-09-13] ingest | item 6 PR 1 — the nondeterminism inventory, coverage map, trace schema

Up: [dev](../dev.md) · page: [testing](../testing/testing.md) · record
`docs/design/replay-nondeterminism-inventory.md` · plan §4, D12–D14, §10.12.6.

**Finding.** The plan's entanglement measurement (one HLC clock site; ~25 `tokio::fs` calls in one module; ~6
`fastrand` + ~6 timers in `tasks.rs`; ~9 `Instant::now` in `connection.rs`; two pure governors) was a sample. Counted
on production paths only: 22 wall-clock, 71 monotonic-clock, 27 RNG, 46 timer and 15 filesystem sites, every
`select!`, one seeded hasher (`store.rs`) and one unseeded process-global one (`framing.rs` `shard_hasher` — which
gossip shard a key lands on, hence queue fullness and drop points, is per-process random), plus the papaya CAS family.
The monotonic sites are the bulk and were absent from the plan's sample: dedup windows, suppression, backoff, rate
windows, SWIM suspicion, the governor cooldown.

**Change.** One record: the inventory by kind and module with owners; the coverage map (kernel · Loom · fuzz ·
Docker; each with what it does *not* cover); the sleeps whose duration is a correctness assumption — the 1 s
"let the winning commit converge" after a lock commit is the exemplar, and the same day's writer-backoff finding is
named as a schedule the kernel should reach on purpose; the choices trace (`seq · node · kind · stream/seam · value`,
values are what production code *received*) and the minimum bundle (build, config, initial disk state, redacted
inputs, trace, witness) sufficient for exact reproduction with divergence detection (D14); the static forbidden-call
check in PR 3 (D12); D13's three additions in place.

**Decided here.** Two clocks, not one (`wall` feeds the HLC and `causal_now_ms`; `mono` feeds intervals); five named
RNG streams so a draw out of order is a divergence, not a coincidence; channel fullness is a *fault* the kernel
schedules, since `kv().set == false` and dropped frames are exactly what the flake tier keeps seeing; test-module
sites are out of scope (the structural-poll rule governs them). A new site on a covered path is admitted only by
editing the inventory — the check enforces that from PR 3.
