# 19 · Replay & Simulation

Chapter 18 was about what an answer means. This one is about getting the *same* answer twice.

It is grounded in [`mycelium-core/src/sim_seam.rs`](../../mycelium-core/src/sim_seam.rs) (what
production calls) and the [`mycelium-sim`](../../mycelium-sim/) crate (what records and replays it).
The design record is
[`docs/design/replay-nondeterminism-inventory.md`](../design/replay-nondeterminism-inventory.md).
The runnable demonstration is
[`mycelium-sim/examples/replay_a_bundle.rs`](../../mycelium-sim/examples/replay_a_bundle.rs).

Two questions this chapter answers:

- **"It failed on the third run yesterday. Now what?"** You ship the recording. A bundle turns that
  sentence into *"effect 7 differs, and here is what changed"*.
- **"Can't I just save the seed?"** No, and this is the whole design. See below.

---

## The honest core: a seed is not a durable reproduction artefact

A run reproduced by re-seeding is reproducible **only while the code is unchanged** — which is
exactly when nobody needs it. The moment you edit the thing you are debugging, the seed replays a
different sequence of decisions and tells you nothing.

So the harness records something else: **what the production code asked for, and what it received.**
A replay checks each request against the recording. When the build has changed, that gives you the
question a reviewer actually has, which is *where did it first depart* rather than *does it still
fail*.

---

## What counts as a decision

Nine kinds, and the taxonomy is deliberate:

| Kind | What it records |
|---|---|
| `Rng` | a draw from a **named** stream |
| `Wall` | a wall-clock read |
| `Mono` | a monotonic-clock read |
| `Sched` | which branch a scheduling point took |
| `Chan` | a channel operation and what it returned |
| `Timer` | a deadline, and whether it fired |
| `Fs` | a storage effect, with content hash, target, offset and flags |
| `Input` | an external input — a token verification, a model reply, a tool response |
| `Fault` | an injected fault |

`Wall` and `Mono` are separate kinds on purpose, **because they fail differently**: wall time jumps,
monotonic time does not. Collapsing them would hide the class of bug where a clock stepped backwards.

---

## The seam, and what it costs production

Production never calls `SystemTime::now` directly. It calls the seam, and the `sim` feature decides
what that means:

- **Without `sim`** — every shipped build — the seam's body *is* the call it replaced. No
  indirection, no branch, no kernel.
- **With `sim`**, the read routes through a thread-local kernel.

That asymmetry is the design's price of admission: a harness that made production pay for its
existence would be refused on those grounds alone.

The cost of a thread-local rather than a passed-in handle is stated honestly in the module: **a call
that forgets the seam is invisible rather than a compile error.** That is precisely why
`scripts/check-sim-seams.sh` exists, holds a baseline of permitted call sites, and runs in `make
check`. Adding a direct clock, random or filesystem call fails the gate until it is routed or the
baseline changes in the open.

---

## A bundle is a recording plus what it takes to trust it

```rust
pub struct Bundle {
    build:   Build,                        // commit, version, features, target, rustc
    config:  BTreeMap<String, String>,     // secrets already replaced
    initial: BTreeMap<String, Vec<u8>>,    // per-node initial disk images
    inputs:  Vec<(String, Vec<u8>)>,       // external inputs in arrival order, redacted
    trace:   Trace,                        // the reproduction itself
    witness: Option<Witness>,              // the assertion it was captured to reproduce
}
```

The trace is the reproduction. Everything around it is what makes the reproduction *mean* something.

### Exact replay vs. scenario replay

`Build` is recorded because **exact replay is only meaningful against a pinned build**. Replay a
trace against the commit that produced it and you have an *exact* replay. Replay it against a
different commit and you have a **scenario** replay, which is a different and weaker claim.

Both are useful. The point is that the difference is *visible* rather than assumed, which is only
possible because the bundle carries the build that made it.

### The witness

```rust
pub struct Witness { assertion: String, toggle: Option<String> }
```

A bundle without a witness replays a run in which nothing went wrong, and reproducing that proves
only that the harness works. `Bundle::can_prove_its_failure()` is the check.

Read the optional toggle precisely. `None` means *the failure needs no toggle*. It does **not** mean
"we did not check" — a bundle whose witness is unknown is a bundle that cannot prove its own fix.

---

## What a divergence does

Two layers, two behaviours, and both are right:

- **The kernel returns a `Divergence`.** It carries both sides. It never guesses a result, and the
  value it returns on a match is always the *recorded* one, never a freshly computed one.
- **The production-side seam panics with it.** A clock read cannot return "the replay departed", and
  a replay that silently continued past a divergence would produce a run that is neither the
  recording nor an honest fresh run. The contract is to *stop*, and in a harness a panic is stopping.

---

## Run it

```bash
cargo run -p mycelium-sim --example replay_a_bundle
```

A food-rescue depot sweeps its surplus-food offers: read the clock, draw a scheduling jitter, journal
a verdict per offer. Small on purpose — the point is not the sweep, it is that the sweep's decisions
are reproducible. Step 4 injects a bug (it drops the grace window a depot allows for a late van) and
the replay localises it.

**The example is careful about whose bug it is.** The injected fault is the example's own, not the
library's. Injecting a fault into the library itself is a test-only affordance and stays that way.

---

## The gate that proves it detects rather than re-seeds

`mycelium-sim/tests/divergence.rs` records a run, replays it with **one storage record's bytes
changed at equal length**, and requires a divergence.

Equal length is the load-bearing part. A harness that only noticed a length change would pass a
weaker test while detecting nothing about content, and a harness that passes the equal-length test
unchanged is not detecting divergence — it is re-seeding.

---

## Recording a node, not a function

The example above installs the seams around a free function. A **node** — an agent, a companion —
records the same way, with three more facts: the kernel is per thread, so one thread per node; the
`sim` feature must be on in *your* build (`mycelium = { …, features = ["sim"] }` — off in every
shipped build); and the clock is paused for the replay. This is what the commitment crate's own test
does (`mycelium-commitment/src/lib.rs`, the whole-node recording), lifted out:

```rust
use mycelium::sim_seam::{install, take, SimContext};
use mycelium_sim::{seams::Sources, Bundle, Kernel};

// Record: one current-thread runtime, the seams installed before the node starts.
install(SimContext { kernel: Kernel::recording(), sources: Sources::seeded(seed, wall_ms),
                     node: "depot-north".into(), offsets: Default::default() });
run_the_node().await;                                   // every choice goes through the seams
let ctx = take().expect("the recording");
Bundle::new(ctx.kernel.trace().clone())
    .witnessed_by("the assertion that failed", Some("the toggle that makes it fail again".into()))
    .write(&dir)?;

// Replay: the same node, the bundle's trace, the wall clock held still.
mycelium::sim_seam::pause_clock_for_replay();
install(SimContext { kernel: Kernel::replaying(bundle.trace.clone()), .. });
run_the_node().await;                                   // a divergence panics, naming the choice
take(); mycelium::sim_seam::resume_clock_after_replay();
```

The node binary has the same recipe built in, **under `sim` only**: a build with
`--features cli,sim` started with `GOSSIP_RECORD_BUNDLE_DIR=<dir>` runs on a current-thread runtime
under the seams and writes the bundle at shutdown (`GOSSIP_RECORD_SEED` picks the seed) — the
operator's capture path, [diagnostics.md § Capturing a replay bundle](../operations/diagnostics.md).
An embedded agent records the way this section shows; there is no route or config field that starts
a recording on a node built without `sim`. The scheduler seam (v2.9.0) is what makes a whole node
replay without divergence; before it, task interleaving diverged.

## What this does not establish

- **Not a multi-node schedule.** The scheduler seam's first arm covers one node's waits.
- **Not task interleaving** beyond that.
- **Not that a trace proves a fix.** A bundle with no witness replays a run in which nothing went
  wrong. Check `can_prove_its_failure()` before believing a green replay.
- **Nothing in production.** The crate governs this repository's own harness and prevents nothing at
  runtime; a shipped build does not link it.

The checked-in corpus bundle for the real write-ahead-log race lives in
`mycelium-core/tests/replay-corpus/` and is replayed by its own test rather than by the example,
because reproducing that failure means removing a durability fix, and the switch that does so is
test-only on purpose. A binary that could disable a durability fix would be a data-loss switch.

---

## Where to go next

| You want | Read |
|---|---|
| the coverage map: every nondeterministic read and which seam owns it | [`design/replay-nondeterminism-inventory.md`](../design/replay-nondeterminism-inventory.md) |
| the seam surface production calls | [`mycelium-core/src/sim_seam.rs`](../../mycelium-core/src/sim_seam.rs) |
| what a receipt means, since replayed runs produce the same ones | [18 · Contracts & receipts](18-contracts-and-receipts.md) |
| the crate's own module map | [`mycelium-sim/`](../../mycelium-sim/) |
