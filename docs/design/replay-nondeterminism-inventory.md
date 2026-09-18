# Deterministic replay — the nondeterminism inventory, the coverage map, and the trace schema (item 6 PR 1)

**Status:** adopted 2026-09-13 · **item 6 PR 1** of `docs/plans/v3-contracts-axis.md` §4 (decisions D12, D13,
D14; §10.12.6). A record, not code: it names every place production behaviour depends on something the process
did not decide, says which mechanism will own each (a replaceable seam under the `mycelium-sim` kernel, Loom, a
fuzz target, or a Docker suite), and fixes the shape of the **choices trace** and the **failure bundle** the
kernel records and replays. PR 2 (event kernel, clock/RNG interfaces, record/replay) and PR 3 (storage/channel
adapters, WAL writer seams, **the static forbidden-call check**) implement against this list; a site not in this
list that reaches a covered path is a defect in this record first.

> Posture: the production decision logic runs **unchanged** behind adapters. Nothing here conditions signal
> propagation or KV replication (rule 1); the seams are injection points, not semantics. A seed is not a
> reproduction artefact (D14): exact replay checks each requested effect against the recorded next effect and
> **detects divergence** — it never merely re-seeds.

## 1. Method

Counted with `grep` over `mycelium-core/src`, `src/agent`, `src/consensus.rs` and `src/capability.rs`,
**production paths only** (lines before each file's test module), on `main` at `cafbdd7` (2026-09-13). Test-module
sites are excluded: a test that sleeps is the test's problem, and the structural-poll rule already governs it.
Eight kinds:

| Kind | Pattern | What makes it nondeterministic |
|---|---|---|
| **W** wall clock | `SystemTime::now` | host time; skew between nodes; jumps |
| **M** monotonic clock | `Instant::now` (std and tokio) | host uptime; only intervals are meaningful; not comparable across nodes |
| **R** RNG | `fastrand::*` | unseeded per-thread generator |
| **T** timers | `tokio::time::{sleep, interval, timeout, timeout_at}` | the runtime's timer wheel and scheduling |
| **F** filesystem | `tokio::fs`, `std::fs` | volatile vs durable bytes, directory entries, `sync` vs write completion, crash vs power loss |
| **S** scheduling / channel readiness | `tokio::select!`, bounded `mpsc` `try_send` | which branch wins; whether a queue was full at that instant |
| **H** hash iteration order | `AHashMap`, `HashMap` iteration | per-process random keys unless seeded |
| **C** CAS retry | `papaya` `compute` / `update` closures | which thread wins a retry — a *thread interleaving*, not a single transition |

**Totals (production sites):** W 22 · M 71 · R 27 · T 46 · F 15 · S — every `select!` (not counted per site; owned
by the kernel's scheduler adapter) · H one seeded (`store.rs`), one unseeded shared state (`framing.rs`
`shard_hasher`), the rest per-map · C the wiki's recurring race family.

## 2. The inventory, by module

Each row: the site(s), what decision depends on it, and the **owner** — the mechanism that makes it deterministic
under replay (§3). Line numbers are anchors at the commit above and will drift; the pattern and the function are
the canon.

### 2.1 Clocks

| Site | Depends on it | Owner |
|---|---|---|
| `mycelium-core/src/hlc.rs` `wall_now_ms` (**the one HLC read**, line 130) | every HLC tick, so every write's LWW rank | **wall-clock seam** — the clean seam the plan verified; the kernel's wall clock feeds `Hlc` |
| `src/consensus.rs:337` `wall_now_ms` → `causal_now_ms` (= `max(wall, hlc physical)`) — used at 14 call sites: 3 in `consensus.rs`, 3 in `consensus_handle.rs`, 4 in `http.rs`, 1 in `overlay_consistent.rs` | **lease expiry**: whether a committed slot is live, whether a lock is held (D13: clock injection must reach these reads, not only the HLC) | wall-clock seam, via `causal_now_ms` taking the injected clock |
| `src/agent/opacity.rs:108,130,149,359` · `capability_ops.rs:178,408` · `lifecycle.rs:464` · `tasks.rs:968` · `wiring.rs` · `consensus_handle.rs:187,250` · `mesh_handle.rs:253,316` · `kv_handle.rs:227,345` · `signal.rs:468` · `prompt.rs` | freshness of soft state (`is_fresh`, opacity, requirement expiry, intent TTLs), evidence timestamps | wall-clock seam (one `now_ms()` on the kernel clock) |
| **OIDC verification** — `oidc.rs:128,174,187` (the JWKS cache's `CachedKeys { at: Instant }` and `at.elapsed() < JWKS_TTL`) and, **inside `jsonwebtoken`**, the wall-clock read behind `validation.validate_exp = true` (`oidc.rs:96`) | whether a token is accepted now, and whether the JWKS is refetched | **the verified result is the recorded input, not the token.** Recording the JWT and the JWKS response does *not* make authentication replayable: the same bytes replayed later fail an expiry check that reads the real wall clock inside a dependency we do not own, and the cache's monotonic TTL decides refetching independently. The kernel records `input oidc/verify → principal=…` (or the refusal) and **authentication internals are declared outside its coverage**; the JWKS cache's `Instant` read is ours and joins the monotonic seam if replaying refresh behaviour is ever wanted. Corrected 2026-09-14 after review — the first draft listed this module under the wall clock and called the token a sufficient input |
| `mycelium-core/src/writer.rs:57,78,124,131,177` (idle timeout, reconnect backoff) · `connection.rs:131,168,240,373,464` (rate window, state-request cadence) · `swim.rs:160,222,232,237,473` · `swim_membership.rs` · `signal.rs:375–1014` (dedup windows, suppression, quorum-evidence trim, pending ordering) · `tasks.rs:780,867` · `mesh_handle.rs:136,165,271` · `rpc.rs:146,148` · `a2a.rs:124,318,400` · `http.rs:2718,2726` · `membership_governor.rs:186,191` (the **cooldown**, WP5) · `capability_handle.rs` | every interval-based decision: is this peer alive, is this signal a duplicate, has the backoff elapsed, has the cooldown elapsed | **monotonic-clock seam** — separate from the wall clock (the plan's rule: "monotonic and wall clocks separately"); `Instant` values become kernel ticks. **Seam landed 2026-09-17** (`sim_seam::mono_now_ns` / `mono_since`); `writer.rs`'s reconnect backoff and `connection.rs`'s rate window + anti-entropy cooldown are through it |

**Why the monotonic seam is a second function and not a use of `wall_now_ms`.** Every site above
measures an *interval*. `Instant` is monotonic, so a backwards NTP step cannot make an interval
negative or enormous; `SystemTime` gives no such guarantee. Routing these through the wall clock
would have been one function fewer and a new class of bug — a rate window that never expires, a
backoff that fires instantly. `Instant` has no epoch, so the seam supplies one (the first read),
which is what `Seams::mono_now_ns` already meant by "since the run began" — **offset by
`MONO_ORIGIN_NS`**, a year, so that a point *before* the run is representable. That offset is not
cosmetic: `Instant` has the property (it is internally signed on Linux and offset on macOS, so
`Instant::now() - 600s` is fine however young the process is) and a bare count from zero does not.
`SignalLog::seed` reconstructs an entry that arrived `age_ms` ago and `warm_quorum_from_layer1` calls
it *at startup*, so a zero origin would clamp every warmed record to the run's start and make stale
quorum evidence read as live. `mycelium-sim`'s `Sources` starts at the same origin — a harness that
disagreed here would disagree in the direction that hides the bug. `mono_since` replaces
`Instant::elapsed` and saturates, because "earlier is actually later" cannot happen and a wrap would
express that impossibility as five centuries elapsed — which a cooldown reads as long expired.

**Deliberate admission (2026-09-17, reversing part of the same day's work).** The peer table,
`SwimMembership`, `SignalHandlers` and `MeshHandle::last_signal` were converted to `u64` monotonic
nanoseconds and have been **converted back to `Instant`**. Those are public types
(`CoreCtx::peers`, `ConnContext::peers`, `pub mod swim_membership`, `pub mod signal`,
`pub mod mesh_handle`), and changing them forced a MAJOR release for a representation change with
**no consumers in the workspace** — every external use is `agent.peers()`, the method, which never
changed.

What a replay must reproduce is the **decision, not the representation**, and every staleness
decision is one of three shapes. Each now has a seam, and the `Instant::now()` sites that merely
*take a stamp* are admitted here because nothing branches on them directly:

| Shape | Seam | Used by |
|---|---|---|
| "how long since this" | `mono_elapsed(&Instant)` | sender-log window, quorum windows, `last_signal_age` |
| "how long between these two" | `mono_span(&Instant, &Instant)` | SWIM suspicion timeout, quorum-evidence rate limit, reorder hold |
| "has this deadline passed" | `mono_before(&Instant, &Instant)` | suppression expiry, sender-log trim cutoff, peer eviction |

The third exists because two stamps taken at different moments, compared directly, would have a
replay comparing *its own* elapsed wall time rather than the recording's. Recording the **verdict**
is what makes that reproducible without touching the stored type.

The same admission covers `src/federation/catalog.rs` (item 2 PR 3): `CatalogObservation.observed_at`
stores an `Instant`, and the one decision derived from it — *has discovery expired* — goes through
`mono_elapsed`. The file's only forbidden-check hit is the `use std::time::Instant` import, which the
check counts because such an import *enables* unqualified calls; there are none.

**Three kinds of `Instant` live in this list, and only one of them is this seam's.**

1. *Function-local elapsed timers* — `writer.rs`'s `last_fail`, `connection.rs`'s
   `rate_window_start` and `last_state_sent`. A `u64` of monotonic nanoseconds, converted in place.
   These are done.
2. *`tokio::time::Instant` deadlines* — `writer.rs`'s `idle_deadline`, fed to `sleep_until`. These
   belong to the **timer seam**, not this one: converting the reading without owning the sleep would
   leave the deadline deterministic and the wait still real.
3. *`Instant` in a shared type* — `Arc<papaya::HashMap<NodeId, Instant>>`, the peer table's
   last-heard-from stamp, which appears in `CoreCtx`, `ConnContext`, `SwimState`, `TaskContext` and
   their tests. **Done 2026-09-17**, together with `SwimMembership`: `merge_gossip` reads the clock
   once and passes the same `now` to the membership table *and* to the peer table, so converting
   either alone would have left that function reading two clocks. `SwimMembership` needed no
   restructuring — it has always taken its clock as a parameter.

**What the representation costs, and what that bought.** Monotonic nanoseconds count from *process
start*, so `Instant::now() - Duration::from_secs(600)` has no equivalent — a test process alive for
milliseconds has no "ten minutes ago" to insert. This made `compute_view_confidence`'s staleness
branch unreachable from a test, which the existing test had never covered. The answer is the pattern
`SwimMembership` already demonstrated: `compute_view_confidence_at(ctx, now_ns)`. The conversion
therefore left the function *more* testable than it found it, and that is the question to ask at
every remaining site — **what was the old type quietly making impossible to test?**

### 2.2 Randomness

| Site | Depends on it | Owner |
|---|---|---|
| `mycelium-core/src/ops.rs:57,95,130` · `mesh_handle.rs:156` · `rpc.rs:138` · `a2a.rs:155` · `http.rs:1448,2382,2410` · `consensus_handle.rs:433` | **nonces** (signal dedup, RPC correlation, lock values, handle ids) | **named RNG stream `nonce`** |
| `ops.rs:42` · `connection.rs:518` | **opacity shedding roll** (`fastrand::f32() >= fill`) | stream `shed` |
| `writer.rs:25` (jittered backoff) · `consensus.rs:877,1084` (ballot retry jitter) · `tasks.rs:554` (tick jitter) · `membership_governor.rs:229` (pass jitter) | **jitter** | stream `jitter` |
| `swim.rs:446,455` · `swim_membership.rs:211` · `tasks.rs:417,484,497,656,737` | **peer selection and shuffles** (ping target, fan-out victims, bootstrap order) | stream `select` |
| `membership_governor.rs:182` (`decide(…, fastrand::f64())`) | the **self-election roll** — the decision function is already pure; only the roll is random | stream `govern` |

Five named streams, each a seeded PRNG in the kernel; recording captures the draws per stream so a replay can
detect a draw made out of order (divergence), not merely reproduce the sequence.

### 2.3 Timers and scheduling

| Site | Depends on it | Owner |
|---|---|---|
| `swim.rs:287,322,432` · `rpc.rs:91` · `http.rs:1464,1890` · `mesh_handle.rs` deadlines | **timeouts** (ping ack, RPC reply, lease heartbeat, scatter) | timer seam: a deadline is a kernel event; expiry is a scheduled transition, never a wall wait |
| `swim.rs:432` · `membership_governor.rs:224` · `emergent.rs:749` · `a2a.rs:121` · `tasks.rs` health/anti-entropy tickers | **periodic loops** | timer seam (`interval` → kernel tick stream). **Routed 2026-09-18** (`sim_seam::interval_ms` → `Ticker::tick`) for nine sites in `src/agent`: the tuner, opacity (one stream per kind), the detectors, `gcap` reassert, the intent reconciler (one stream per key), the A2A sweep, the health ticker (both constructions) and GC. The recorded decision is the **nominal schedule** — an immediate first tick, then one period each — so a replay ticks the loop's written cadence, not the wall's jitter; a tick and a sleep are different requests (`tick(…)` vs `sleep(…)`). **The remaining seven routed the same day:** `kv-persist/{key}`, `mesh/emit/{kind}`, `mesh/stale/{kind}`, `persistence/snapshot`, `rate/decide` (`Delay` — a late decision shifts the schedule rather than bursting), `swim/probe` (its 1 ms floor kept: `interval_ms(0)` would panic exactly as `tokio::time::interval(ZERO)` does) and `membership/tick`. **Every periodic loop in the tree now ticks through the seam.** The alias gap the nine exposed (`time::interval` through `use tokio::time`, three of the nine never counted) was closed the same day in the script (`alias_pattern`, the `time` half) |
| `consensus.rs:878,1085` · `consensus_handle.rs:157,162,223,232,447,494` · `http.rs:2395,2468,2702,2709` · `a2a.rs:387` · `kv_handle.rs:385` | **fixed sleeps inside protocol logic** — the 1 s "let the winning commit converge" after `distributed_lock`'s commit is the one whose *duration is a correctness assumption* (§4, and the 2026-09-13 lock-race finding) | timer seam; each is listed in the coverage map as a **schedule the kernel must explore** (0, exact, and > the sleep). **Routed 2026-09-18** (`sim_seam::sleep_ms`) for `consensus_handle.rs`'s six sleeps — the two converge sleeps as `lock/converge` and `elect/converge`, the ballot defers as `consensus/defer` and `consensus/suggest-defer`. The request is `sleep(<ms>)` and the result the *effective* elapsed time: a replayed sleep advances both simulated clocks and **never wall-waits**. Because replay checks the request but supplies the result, an edited trace line makes the timer return 0 or 5000 — the **hook** for exploring, not the exploration: in exact replay the clock reads that follow are supplied from the trace too, so **exact replay reproduces and cannot explore**. Re-deriving those reads is scenario replay (§5), which the two-mode kernel does not yet have; the seam test pins that boundary. The remaining sites in this row are not yet routed |
| every `tokio::select!` in `connection.rs`, `writer.rs`, `tasks.rs`, `swim.rs`, `signal.rs`, the governors, `mcp.rs`, `lifecycle.rs` | **which ready branch wins** | **scheduler seam**: the kernel decides readiness order; exploration permutes it |
| bounded `mpsc` `try_send` (`gossip_txs` shards, WAL channel, signal handlers, the writer channels, the audit drain, the AE evidence journal) | **"was the queue full"** — drops a frame, skips a WAL append, `false` from `kv().set` | **channel-readiness seam**: capacity and fullness are kernel state, so "full" is a schedulable fault. **Routed** as of 2026-09-17 |

**Stream identity is per destination, not per call site.** A drop on gossip shard 2 and a drop on
shard 5 are different events; so are a drop to peer A and a drop to peer B. `targets` is an
`AHashSet`, whose iteration order is not stable across processes (§2.5), so a single shared stream
would hand one peer's recorded verdict to another — and the failure would look like a frame lost
rather than a trace misread. Forwards and pings get *separate* stream families even though they share
a channel, because they run in two different tasks and the interleaving of two tasks on one channel
is itself nondeterministic; one sequence each is what lets each replay independently.

**The seam's stream argument is eager, so a name is never formatted per event.** `chan_try_send`
takes `&str`, and the argument is evaluated whether or not a kernel is installed — so a `format!` at
a call site costs an allocation *in production builds*. Shard names come from a static table
(`framing::SHARD_STREAMS`, with a cold formatted fallback above it, since `gossip_shards` is only
validated non-zero); peer names are built once and cached beside the sender. A harness that makes the
system slower in order to watch it has changed the thing it was measuring.

**Deliberately outside the seam: the A2A SSE channels** (`a2a.rs:408,429`). Each is a fresh
per-request channel of capacity 8 whose first send happens immediately on creation, so it cannot be
full; recording it would add an event that is always `Sent`. The second send is worse than useless —
it runs in a detached task after a 100 ms sleep, so recording it would make the trace depend on the
tokio scheduler, which is precisely what the kernel exists to remove. These are a *decision*, not
debt; they stay in the baseline as counted sites so that a *new* send in that file still fails the
check.

### 2.4 Storage

| Site | Depends on it | Owner |
|---|---|---|
| `mycelium-core/src/persistence.rs`: `open_wal` (418), `wal_append` (429–), `fsync_dir` (465), `do_snapshot` (493–: read-back 549, `tfs::write` 583, `File::open`+`sync_data` 585, `rename` 588), `replay` (212, 259) | **durability**: what survives a process kill versus a power loss; whether a write is in the page cache or on the platter; whether the directory entry for `snapshot.bin` is durable before `wal.bin` is truncated | **storage adapter** with the plan's five distinctions: volatile bytes · durable bytes · directory entries · process death ≠ power loss · write completion ≠ sync. The first scenario (PR 4) is the WAL/snapshot race with a merge-removed witness |
| `src/agent/lifecycle.rs:64` (`create_dir_all`) · `http.rs:349,350` (cert PEMs) · `schema_handle.rs` | start-up I/O, configuration inputs | recorded **external inputs** (bundle §5), not a fault surface |

### 2.5 Hash iteration order (D13)

| Site | Depends on it | Owner |
|---|---|---|
| `mycelium-core/src/store.rs:356` — `RandomState::with_seeds(1, 2, 3, 4)` | store hashing is **fixed** (Merkle/anti-entropy stability) | none needed — already deterministic |
| `mycelium-core/src/framing.rs:230` — `shard_hasher()` `RandomState::new()` (process-global, once) | **which gossip shard a key maps to** — per-process random, so the same workload lands on different shard channels in different runs (which changes queue-fullness and drop points) | seeded by the kernel (a process seed the bundle records) |
| `mycelium-core/src/persistence.rs` `do_snapshot` — `entries` built by iterating the store | **the snapshot file's bytes**: two nodes holding identical logical state wrote byte-different files, so any byte-level comparison (checksum, dedup, fixture diff) was unsound — and a recorded run could not reproduce its own snapshot | **fixed 2026-09-17** (item 6 PR 4, found by replaying the WAL/snapshot scenario): entries are sorted by key before encoding, so the file is a function of the state it represents. The store's *hasher* was already seeded; its *iteration* was not, and this encoder was an order-sensitive consumer this section had not listed |
| `src/consensus.rs` (21 `AHashMap` uses; e.g. `voters.values().flatten()` at 419) · `tasks.rs` · `emergent_groups.rs` · `capability_handle.rs` · `swim*.rs` · `wiring.rs` · `opacity.rs` · `helpers.rs` | **iteration order** where an algorithm picks "the first" or accumulates order-sensitively (voter locality sets, fan-out victim sets, group rosters) | seeded hasher under replay; each order-sensitive consumer listed in the coverage map; the static check (PR 3) forbids new unseeded maps on covered paths |

### 2.6 CAS retries (D13, owned by Loom)

`papaya` `compute`/`update` closures (`mycelium-core/src/store.rs`, `ops.rs`, `signal.rs`, `src/agent/*` —
the recurring "act on a stale read" family in `docs/wiki/dev/concurrency/lock-free-and-atomics.md`) are **thread
interleavings**, not single-transition choices; a one-transition kernel cannot represent them. They are
assigned to `loom-spike` (exhaustive interleavings under `--cfg loom`, models citing the real code) and to
real-thread stress tests — *explicitly outside* the kernel's coverage claim.

### 2.7 External inputs

OIDC JWTs and JWKS fetches (`oidc.rs`), MCP servers reached by the client bridge, LLM backends, A2A peers, the
evidence sink, configuration files and certificates: **recorded**, redacted per the threat model's §6 (bearer
credentials and keys replaced by stable placeholders; a protected-artefact class for what cannot be redacted and
still reproduce), and replayed from the bundle.

## 3. The coverage map — what each mechanism owns, and what it does not

| Mechanism | Owns | Explicitly does not cover |
|---|---|---|
| **`mycelium-sim` kernel** (PR 2–4) | clocks (W, M as two seams), the five RNG streams, timers, `select!` readiness, channel fullness, storage faults, external inputs; **divergence detection** on exact replay | CAS interleavings (C); real network timing; anything a Docker suite exists for |
| **Loom** (`loom-spike`) | C — the atomic patterns: once-guard, unique-id, publish-then-observe; new patterns cited from real code | tokio-linked code (cannot compile under `--cfg loom`); protocol-level schedules |
| **Fuzz** (`fuzz/`: `wire_decode`, `capability_decode`, `frame_apply`) | decoder robustness on adversarial bytes | semantics |
| **Docker suites** (integration 13 scenarios, overlay S11–S13, scale) | real network partitions, restarts, multi-process timing at scale | determinism — they are evidence of behaviour under real timing, classified by the CI flake tier |
| **Structural-poll rule** (`docs/wiki/dev/testing/testing.md`) | every multi-node test's readiness | — a rule, not a mechanism; the kernel removes the need for it inside the harness |

**Order-sensitive consumers the kernel must schedule (from §2):** the 1 s convergence sleep in `distributed_lock`
and its gateway twins (`http.rs:2395,2468,2702`) · ballot retry jitter · the reconnect backoff window (`writer.rs`)
— the 2026-09-13 finding that a write fanned out before a bootstrap peer listened left that direction dropping
frames for the backoff window is exactly a schedule the kernel should reach on purpose · the anti-entropy tick
versus a snapshot threshold · the governor cooldown versus the tick.

## 4. Sleeps whose duration is a correctness assumption

These are the sites where a fixed `sleep` stands in for a missing structural wait. The kernel's exploration mode
runs each at *0*, at the *exact* value, and at *longer than the thing it waits for*; the witness for each is the
assertion that would fail if the assumption were false.

| Site | The assumption | Witness |
|---|---|---|
| `consensus_handle.rs:447,494` and `http.rs:2395,2468,2702` — 1 s after a `Committed` before reading the converged lock value | the competing commit's KV update arrives within 1 s | two holders (the `#164` gate) — reproducible in the kernel by delaying the peer's commit frame past 1 s |
| `consensus_handle.rs:157,162,223,232` — opacity defer and suggested-leader defer | deferring yields to the suggested proposer | a ballot storm when both defer equally |
| `a2a.rs:387` — 100 ms before emitting `working` | cosmetic | none — excluded |
| `kv_handle.rs:385` — 100 ms poll in a wait loop | polling granularity only | none — excluded |

## 5. The choices trace and the failure bundle (D14: sufficient for exact reproduction from PR 2)

A bundle is what a failing run leaves behind and what a reviewer replays. Its minimum content, from PR 2:

```text
bundle/
  build.json        # crate versions, git commit, features, target triple, rustc — build identity
  config.json       # every GossipConfig field per node (secrets replaced by placeholders, threat model §6)
  initial/          # disk images per node (wal.bin, snapshot.bin) or the fixture ids they were built from
  inputs/           # every external input, in arrival order, redacted (JWTs, JWKS, MCP/LLM/A2A responses)
  choices.trace     # the ordered choice log — the reproduction itself
  witness.json      # the assertion that failed, and the toggle (cfg(test)) that must make it fail again
```

**`choices.trace` — one line per kernel decision, in order:**

```text
seq  node  kind   stream/seam    request                                              result
1    n1    rng    nonce          draw(u64)                                            0x9f3a…
2    n1    wall   -              now_ms()                                             1789322039042
3    n2    mono   -              now()                                                +12ms
4    -     sched  select         conn/n1→n2#3 ready?                                  branch=recv
5    -     chan   gossip/shard2  try_send(len=88)                                     Err(Full)
6    n1    timer  ballot-jitter  deadline(+37ms)                                      fire
7    n1    fs     wal.bin        append d=sha256:4f1c… len=214 sync=true off=8192     Ok(214)
8    n1    fs     wal.bin        append d=sha256:9ab0… len=214 sync=true off=8410     Err(EIO) wrote=96
9    n1    fs     snapshot.bin   rename from=snapshot.tmp fsync_dir=false             Ok
10   n2    input  oidc/verify    token d=sha256:c31e…                                 principal=oidc:idp/alice
```

Rules: every entry names its **kind** (`rng`, `wall`, `mono`, `sched`, `chan`, `timer`, `fs`, `input`, `fault`),
its **stream or seam**, a **canonical request** and the **result** the production code received. *Request* is a
canonical digest of everything the effect depends on — for a write: the **content hash of the bytes**, the target
(file, offset), and the flags that change its meaning (`sync`, `create`, `truncate`) — never a length alone: two
WAL records of equal length are a different write, and a trace that recorded only `len=214` would accept changed
content as a faithful replay (entries 7 and 8 above are exactly that pair). *Result* is what came back: `Ok(n)`,
a typed error, or a **partial completion** (`wrote=96` — the short write the durability argument turns on).
Values are what the production code *received*, never what it did with them.

Exact replay feeds entries back in order and **checks each request against the recorded next entry** — a request
for `rng/jitter` when the trace says `rng/nonce`, an `fs` append whose content digest differs from the recorded
one, a different offset or a flipped `sync` flag is a divergence and stops the run with both sides printed. The
gate for this is a **same-length, different-content rejection test** (PR 2): record a run, replay it with one WAL
record's bytes changed at equal length, and require a divergence — a replay harness that passes that unchanged is
not detecting divergence, only re-seeding. Scenario replay keeps the causal workload (`input`, `fault`) and lets the kernel
re-derive `sched`/`timer`/`rng` under changed code. Boundary-event views and checkpoints (D14's deferred half)
extend the trace without changing an entry's shape.

**The storage model — three layers, kept apart.** The plan's five distinctions collapse if "volatile" is one
word, so the adapter models each layer and each fault names the layers it takes:

| Layer | Holds | Lost on process kill | Lost on power loss |
|---|---|---|---|
| **process memory** | the store, buffered writer state, anything not yet handed to the kernel | **yes** | yes |
| **OS page cache** | bytes accepted by `write` but not yet `fsync`/`fdatasync`ed | **no** — a completed write survives the process | **yes** |
| **durable storage** | bytes whose sync returned `Ok` | no | no |
| **directory metadata** | the entry a `rename` created, until the *directory* is fsynced | no | **yes**, independently of the file's own bytes |

So a **process kill** loses process memory only: a write that returned `Ok` is still in the page cache and a
restart reads it back. A **power loss** additionally drops unsynced page-cache bytes and unsynced directory
entries — which is why the snapshot's rename is fsynced at the directory *before* the WAL is truncated (v2.4.4),
and why `snapshot_install_syncs_the_directory` pins the wiring only: the property is unobservable without this
adapter. The harness must not manufacture loss a process kill cannot cause, nor certify durability that only a
sync establishes.

**Faults the kernel can inject** (each a `fault` entry): channel full · frame drop on a named connection ·
partition (a set of connections silent for a window) · **process kill** (process memory only) · **power loss**
(process memory + unsynced page cache + unsynced directory entries) · short write (`Ok(n)` with `n <` requested) ·
sync failure (`Err` after the bytes were accepted — the local-sync receipt's *durability not established*,
`contracts-receipts.md` §2) · clock jump (wall) · slow disk (storage effects delayed past a timer).

## 6. The static forbidden-call check (D12: PR 3, not PR 7)

On every path the coverage map assigns to the kernel, a direct `SystemTime::now`, `Instant::now`, `fastrand::`,
`tokio::time::*`, `tokio::fs`/`std::fs` or unseeded `RandomState::new` call outside the seam modules is a build
error under the `sim` feature (a `#[cfg]`-gated `deny` lint or a `clippy.toml` `disallowed-methods` list scoped
to those modules). It lands *with* the adapters in PR 3 so the seams cannot erode while PR 4–7 are built. Test
modules and the seam implementations are exempt; a new site is admitted only by editing this inventory.

**Shipped 2026-09-17 as `scripts/check-sim-seams.sh`, and neither mechanism this record suggested.** Clippy's
`disallowed-methods` is real but **workspace-global** — it cannot be scoped to modules, so it would fire inside
the seam implementations and every test, which is exactly where these calls belong; and Rust has no `#[cfg]`-gated
lint for "do not call this function". The rule this record actually states — *a new site is admitted only by
editing this inventory* — is enforced instead by diffing per-file counts against a checked-in baseline
(`scripts/sim-seams-baseline.txt`), which fails on an increase and reports a decrease as progress. It runs in
`make check` and in CI's clippy job.

**The measured baseline: 190 sites across 45 files.** That is the debt PR 3's adapters and PR 4–7 draw down, and
the number is now visible rather than estimated.

*What it cannot see, stated rather than glossed:* it matches qualified call sites and the `use` imports that enable
unqualified ones, but it does not parse Rust — a call reached through an unknown re-export or a type alias is
invisible. **And a caution from building it:** the first version excluded *everything after the first
`#[cfg(test)]`*, so a live `Instant::now` appended below a test module passed. Fixing it to skip each test item
rather than the file's tail raised the count from 165 to 190 — twenty-five sites that a plausible-looking check had
been silently ignoring. It was found by planting a site and checking the checker failed, which is the only way
these are found. **A second one, 2026-09-18:** a file that imports the timer module (`use tokio::time;`, or
`time` inside a grouped `use tokio::{…}`) and calls `time::sleep(…)` was invisible — the `fs as <alias>` gap over
again, for the timer. Noticed because routing nine tickers moved the baseline for four files and not for the
three that go through the alias; closed by counting `<alias>::sleep|interval|timeout|Instant` in any file with
such an import, which admitted **19 pre-existing sites in eight files (166 → 185)**. The signal both times was
the same: *a baseline that moves when no site moved, or fails to move when one did.*

## 7. What this record does not decide

The kernel's API (PR 2), the adapter traits' exact shapes (PR 3), the minimiser (PR 4+), and whether the
governors' harness (item 4) shares the kernel or wraps it (WP12) — each PR carries its five-part statement.
Papaya CAS interleavings stay with Loom by decision (D13); a future kernel that models threads would revisit that
in this record first.
