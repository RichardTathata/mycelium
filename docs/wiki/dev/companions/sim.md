# mycelium-sim — the replay harness

↑ [companions](companions.md) · guide: [19 · Replay & simulation](../../../guide/19-replay-and-simulation.md) ·
inventory: `docs/design/replay-nondeterminism-inventory.md`

The odd one out: **a companion with no production surface at all.** A shipped node does not link it,
and the operator runbook says so rather than leaving the absence to be inferred.

Key facts:

- **The thesis.** *A seed is not a durable reproduction artefact.* A run reproduced by re-seeding is
  reproducible only while the code is unchanged — which is exactly when nobody needs it. The kernel
  records **what the production code asked for and what it received**, so a replay against a changed
  build answers the question a reviewer actually has: *where did it first depart?*
- **Production pays nothing.** `mycelium_core::sim_seam` is what production calls instead of
  `SystemTime::now` and friends. **Without the `sim` feature the seam's body is the call it
  replaced** — no indirection, no branch, no kernel. A harness that made production pay for its
  existence would be refused on those grounds alone.
- **The cost is paid elsewhere, and named.** A thread-local rather than a threaded-through handle
  means **a call that forgets the seam is invisible rather than a compile error**. That is why
  `scripts/check-sim-seams.sh` exists, holds a baseline of permitted call sites, and runs in
  `make check`.
- **Nine choice kinds.** `Rng` · `Wall` · `Mono` · `Sched` · `Chan` · `Timer` · `Fs` · `Input` ·
  `Fault`. `Wall` and `Mono` are separate **because they fail differently** — wall time jumps,
  monotonic time does not — and collapsing them would hide the clock-stepped-backwards class.
- **A bundle is the trace plus what makes it mean something**: `build` (commit, version, features,
  target, rustc), `config` with secrets replaced, per-node `initial` images, redacted `inputs`, the
  `trace` itself, and a `witness`.
- **Exact vs scenario replay.** Against the pinned build it is *exact*; against another commit it is
  a **scenario** replay — a weaker claim. The bundle records the build so the difference is *visible*
  rather than assumed.
- **The witness is the part people skip.** `Bundle::can_prove_its_failure()`. A bundle without one
  replays a run in which nothing went wrong, and reproducing that proves only that the harness works.
  A `None` toggle means *the failure needs no toggle* — it does **not** mean nobody checked.
- **Two divergence behaviours, both right.** The kernel *returns* a `Divergence` carrying both sides
  and never guesses a result; the production-side seam **panics** with it, because a clock read cannot
  return "the replay departed" and a run that continued past one would be neither the recording nor an
  honest fresh run.
- **Secrets.** `config.json` carries placeholders and `inputs/` is redacted, per the threat model §6.
  A bundle travels to whoever is debugging. *A bundle carrying a signing key is an incident, not an
  artefact.*

## The gate

`mycelium-sim/tests/divergence.rs` → `a_same_length_different_content_write_is_a_divergence`.

Record a run, replay it with **one storage record's bytes changed at equal length**, and require a
divergence. Equal length is the load-bearing part: a harness that only noticed a length change would
pass a weaker test while detecting nothing about content. The test asserts its own premise — that the
two records are the same length — which is the detail worth copying into any similar gate.

> A harness that passes that test unchanged is not detecting divergence, only re-seeding.

## The demonstration

`cargo run -p mycelium-sim --example replay_a_bundle`. A food-rescue depot sweeps surplus-food offers;
step 4 injects a bug (it drops the grace window for a late van) and the replay localises it. **The
injected fault is the example's own, not the library's** — injecting a fault into the library itself
is a test-only affordance and stays that way.

The checked-in corpus bundle for the real write-ahead-log race lives in
`mycelium-core/tests/replay-corpus/` and is replayed by its own test rather than by the example,
because reproducing *that* failure means removing a durability fix, and the switch that does so is
`cfg(test)` on purpose: a binary that could disable a durability fix would be a data-loss switch.

## What it does not establish

Not a multi-node schedule; the scheduler seam's first arm covers one node's waits. Not task
interleaving beyond that. And **nothing in production** — it governs this repository's own harness and
prevents nothing at runtime.
