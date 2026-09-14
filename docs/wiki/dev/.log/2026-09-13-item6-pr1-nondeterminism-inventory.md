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

**External review before merge (2026-09-14), three P2 corrections, all taken.** (1) *The trace recorded length, not
identity:* two WAL records of equal length are a different write, so a replay checking `len=214` would accept
changed content as faithful. Entries now carry a **canonical request digest** (content hash, target, offset, the
flags that change meaning) **and the result** — `Ok(n)`, a typed error, or a partial completion (`wrote=96`) — with
a same-length/different-content rejection test as PR 2's gate. (2) *Process death ≠ power loss:* the draft's
"volatile bytes" conflated process memory, the OS page cache and durable storage. Now a four-layer table (process
memory · page cache · durable storage · directory metadata) with each fault naming the layers it takes: a process
kill loses process memory only (a completed write survives in the page cache); power loss additionally drops
unsynced page-cache bytes and unsynced directory entries — which is exactly why v2.4.4 fsyncs the snapshot's
directory before truncating the WAL. Short write and sync-failure faults added. (3) *Authentication is not
replayable by recording its inputs:* verified in code — the JWKS cache holds `CachedKeys { at: Instant }` and
compares `at.elapsed()` (`oidc.rs:128,174,187`), while `validate_exp = true` makes `jsonwebtoken` read the real
wall clock inside a dependency we do not own. The kernel now records the **verified result** (`principal=…` or the
refusal) as the input and declares authentication internals outside its coverage; the JWKS cache's monotonic read
is ours and joins the monotonic seam if refresh behaviour ever needs replaying. The first draft listed that module
under the wall clock alone — the inventory's own honesty rule caught by a reader, not by us.

**Decided here.** Two clocks, not one (`wall` feeds the HLC and `causal_now_ms`; `mono` feeds intervals); five named
RNG streams so a draw out of order is a divergence, not a coincidence; channel fullness is a *fault* the kernel
schedules, since `kv().set == false` and dropped frames are exactly what the flake tier keeps seeing; test-module
sites are out of scope (the structural-poll rule governs them). A new site on a covered path is admitted only by
editing the inventory — the check enforces that from PR 3.
