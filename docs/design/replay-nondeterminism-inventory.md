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
| `src/agent/opacity.rs:108,130,149,359` · `capability_ops.rs:178,408` · `lifecycle.rs:464` · `tasks.rs:968` · `wiring.rs` · `consensus_handle.rs:187,250` · `mesh_handle.rs:253,316` · `kv_handle.rs:227,345` · `signal.rs:468` · `prompt.rs` · `oidc.rs` | freshness of soft state (`is_fresh`, opacity, requirement expiry, intent TTLs), evidence timestamps, JWT `exp` | wall-clock seam (one `now_ms()` on the kernel clock); `oidc` stays wall (external tokens) and is a recorded external input |
| `mycelium-core/src/writer.rs:57,78,124,131,177` (idle timeout, reconnect backoff) · `connection.rs:131,168,240,373,464` (rate window, state-request cadence) · `swim.rs:160,222,232,237,473` · `swim_membership.rs` · `signal.rs:375–1014` (dedup windows, suppression, quorum-evidence trim, pending ordering) · `tasks.rs:780,867` · `mesh_handle.rs:136,165,271` · `rpc.rs:146,148` · `a2a.rs:124,318,400` · `http.rs:2718,2726` · `membership_governor.rs:186,191` (the **cooldown**, WP5) · `capability_handle.rs` | every interval-based decision: is this peer alive, is this signal a duplicate, has the backoff elapsed, has the cooldown elapsed | **monotonic-clock seam** — separate from the wall clock (the plan's rule: "monotonic and wall clocks separately"); `Instant` values become kernel ticks |

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
| `swim.rs:432` · `membership_governor.rs:224` · `emergent.rs:749` · `a2a.rs:121` · `tasks.rs` health/anti-entropy tickers | **periodic loops** | timer seam (`interval` → kernel tick stream) |
| `consensus.rs:878,1085` · `consensus_handle.rs:157,162,223,232,447,494` · `http.rs:2395,2468,2702,2709` · `a2a.rs:387` · `kv_handle.rs:385` | **fixed sleeps inside protocol logic** — the 1 s "let the winning commit converge" after `distributed_lock`'s commit is the one whose *duration is a correctness assumption* (§4, and the 2026-09-13 lock-race finding) | timer seam; each is listed in the coverage map as a **schedule the kernel must explore** (0, exact, and > the sleep) |
| every `tokio::select!` in `connection.rs`, `writer.rs`, `tasks.rs`, `swim.rs`, `signal.rs`, the governors, `mcp.rs`, `lifecycle.rs` | **which ready branch wins** | **scheduler seam**: the kernel decides readiness order; exploration permutes it |
| bounded `mpsc` `try_send` (`gossip_txs` shards, WAL channel, signal handlers) | **"was the queue full"** — drops a frame, skips a WAL append, `false` from `kv().set` | **channel-readiness seam**: capacity and fullness are kernel state, so "full" is a schedulable fault |

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
seq  node  kind      stream/seam   value                       # meaning
1    n1    rng       nonce         0x9f3a…                     # a draw from a named stream
2    n1    wall      -             1789322039042               # the wall clock the HLC read
3    n2    mono      -             +12ms                       # a monotonic advance
4    -     sched     select        conn/n1→n2#3 branch=recv    # which select! branch became ready
5    -     chan      gossip/shard2 full                        # a channel-fullness fault
6    n1    timer     ballot-jitter fire                        # a deadline firing
7    n1    fs        wal.bin       append len=214 sync=true    # a storage effect and its durability
8    n1    fs        snapshot.bin  rename durable_dir=false    # a storage fault (power loss before fsync_dir)
9    n2    input     oidc/jwks     #7                          # an external input consumed
```

Rules: every entry names its **kind** (`rng`, `wall`, `mono`, `sched`, `chan`, `timer`, `fs`, `input`, `fault`)
and its **stream or seam**; values are what the production code *received*, not what it did with them. Exact
replay feeds entries back in order and **checks each request against the recorded next entry** — a request for
`rng/jitter` when the trace says `rng/nonce`, or an `fs` append of a different length, is a divergence and stops
the run with both sides printed. Scenario replay keeps the causal workload (`input`, `fault`) and lets the kernel
re-derive `sched`/`timer`/`rng` under changed code. Boundary-event views and checkpoints (D14's deferred half)
extend the trace without changing an entry's shape.

**Faults the kernel can inject** (each a `fault` entry): channel full · frame drop on a named connection ·
partition (a set of connections silent for a window) · process kill (volatile bytes lost, durable kept) · power
loss (durable-but-unsynced bytes lost; directory entries not yet synced lost) · clock jump (wall) · slow disk
(storage effects delayed past a timer).

## 6. The static forbidden-call check (D12: PR 3, not PR 7)

On every path the coverage map assigns to the kernel, a direct `SystemTime::now`, `Instant::now`, `fastrand::`,
`tokio::time::*`, `tokio::fs`/`std::fs` or unseeded `RandomState::new` call outside the seam modules is a build
error under the `sim` feature (a `#[cfg]`-gated `deny` lint or a `clippy.toml` `disallowed-methods` list scoped
to those modules). It lands *with* the adapters in PR 3 so the seams cannot erode while PR 4–7 are built. Test
modules and the seam implementations are exempt; a new site is admitted only by editing this inventory.

## 7. What this record does not decide

The kernel's API (PR 2), the adapter traits' exact shapes (PR 3), the minimiser (PR 4+), and whether the
governors' harness (item 4) shares the kernel or wraps it (WP12) — each PR carries its five-part statement.
Papaya CAS interleavings stay with Loom by decision (D13); a future kernel that models threads would revisit that
in this record first.
