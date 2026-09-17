## [2026-09-17] ingest | item 6 PR 3 — the writer channels, and a cost the seam was imposing

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.3 · code
`mycelium-core/src/framing.rs`, `src/agent/tasks.rs`, `Makefile`, `.github/workflows/ci.yml`.

### The intended work

Four peer-writer sends — gossip forwards and TCP pings, each with its respawn retry — routed through
the channel seam. Baseline **186 across 43 files**, from 190.

**Streams are per destination, not per call site.** `targets` is an `AHashSet`, and §2.5 already
records that its iteration order is not stable across processes. A single `writer/forward` stream
would therefore have handed peer A's recorded verdict to peer B on a replay whose iteration order
differed — and the symptom would have been *a frame apparently lost*, which is a bug hunt in the
wrong module entirely. Per-peer streams make the verdicts robust to an ordering the kernel does not
yet own. Forwards and pings get separate stream *families* although they share a channel: they are
two tasks, and the interleaving of two tasks on one channel is itself nondeterministic, so one
sequence each is what lets each replay independently.

### What the work actually found

**The seam was allocating on the hottest path in the system.** `chan_try_send` takes its stream as
`&str`, and an argument is evaluated whether or not a kernel is installed. So the
`format!("gossip/shard{n}")` introduced two commits ago cost a heap allocation **per frame dispatch
in ordinary production builds** — not in sim, in production. Nothing failed; nothing would have,
until someone benchmarked forwarding and found it slower than the release before.

This is the failure mode a replay harness is most exposed to, and it is worth stating as a rule: *a
harness that makes the system slower in order to watch it has changed the thing it was measuring.*
The fix is a static `SHARD_STREAMS` table with a cold formatted fallback above it (`gossip_shards`
is only validated non-zero, so a configuration above the table is legal), and per-peer names built
once and cached beside the sender.

The table then needs its own guard, because a typo in entry 37 costs nothing at compile time and
would silently split shard 37's trace onto a stream nobody reads. `the_shard_name_table_agrees_with_
the_formatted_fallback` compares every entry against the fallback — and was **verified by planting
`"gossip/shard73"` at index 37 and watching it fail**, then restoring.

**A second gap, in the gates.** `mycelium-sim` has been clippy-gated since the crate landed, but
`mycelium-core --features sim` — the arm that *routes* through it, which is where every seam call
site's sim path lives — was only ever compiled by the test job. Three redundant closures had been
sitting in `sim_seam.rs` unseen. Added to `make check` and CI.

This is the **third instance of one family**: compliance-gated code (found 2026-09-04), core's own
test scope (found 2026-07-21), and now the sim feature. The pattern is *a feature whose code is
tested but never linted* — running a suite is not the same claim as linting the scope, and the two
have to be added separately every time.

### Decided, not deferred

The two A2A SSE sends (`a2a.rs:408,429`) stay outside the seam, and §2.3 now says why rather than
listing them as debt. Each is a fresh per-request channel of capacity 8 whose first send happens on
creation, so it cannot be full; recording it adds an event that is always `Sent`. The second is
worse — it runs in a detached task after a 100 ms sleep, so recording it would make the trace depend
on the tokio scheduler, which is the thing the kernel exists to remove. They remain counted in the
baseline so a *new* send in that file still fails the check.

### Gates

`make check` clean (now including the sim arm) · core **180** / **201** (sim) · mycelium **521**
(`compliance,a2a`) / **402** (`sim`) · `mycelium-sim` 24 + 6.
