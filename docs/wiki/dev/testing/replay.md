# dev/testing/replay — determinism, the seams, and what a bundle proves

↑ [testing/](testing.md) · crate page: [companions/sim.md](../companions/sim.md) · record
`docs/design/replay-nondeterminism-inventory.md` · guide
[19 — replay & simulation](../../../guide/19-replay-and-simulation.md)

**What this page is for.** Item 6's claim is that a production failure can be *re-run* rather than
argued about. That claim is only as good as two things: the inventory of everything the process did
not decide, and the gates that prove a recorded run replays to the same run. Both live here; the
crate that does the replaying has its own page, and the code is canon for both.

**The rule that keeps it true**, restated from `CLAUDE.md` because this is where it is tested: no
direct clock, RNG or filesystem access in production logic — every one goes through
`mycelium_core::sim_seam`, and `scripts/check-sim-seams.sh` (in `make check`) holds the baseline of
permitted call sites. A new one fails the gate until it is routed **or the baseline is updated in
the open**, which is the point: the list of things that can make a run irreproducible is a
completeness claim, like the lock-order table.

## The nondeterminism inventory and the coverage map (replay, item 6 PR 1)

[`docs/design/replay-nondeterminism-inventory.md`](../../../design/replay-nondeterminism-inventory.md) names every
production site that depends on something the process did not decide — 22 wall-clock, 71 monotonic-clock, 27
RNG, 46 timer, 15 filesystem sites, every `select!`, the one unseeded shared hasher (`framing.rs` `shard_hasher`),
and the papaya CAS retries — and assigns each an owner: the `mycelium-sim` kernel's seams (two clocks, five named
RNG streams, timers, scheduler, channel fullness, storage with volatile/durable/directory distinctions), **Loom** for
CAS interleavings (D13), fuzz for decoders, Docker suites for real timing. It also lists the sleeps whose duration is
a correctness assumption (the 1 s convergence wait after a lock commit first among them), fixes the choices-trace
and bundle shape (D14: exact reproduction with divergence detection from PR 2), and moves the static forbidden-call
check to PR 3 (D12). A new nondeterminism site on a covered path is admitted only by editing that inventory.

## Replay scenarios A and B (item 6 PRs 4–5, 2026-09-17)

The inventory above named what must be reproducible; these are the first two things reproduced with it.

**Scenario A — the WAL/snapshot race** (`mycelium-core/src/persistence.rs`, PR #241). A controlled schedule
with a `cfg(test)` **merge-removed witness** (`MergeRemoved`, RAII) that *must* fail — the Phase A exit gate.
Replaying it found three bugs in the harness itself and one in the product: replay suppressed writes, so a run
could not read its own; effect requests embedded absolute paths, so no bundle replayed elsewhere; an injected
fault did not prevent the effect because the seam acted before deciding (now `kernel_fs` + `planned_fs`: decide,
then act); and **a snapshot's bytes depended on papaya iteration order** — two nodes with identical logical
state wrote byte-different files. The fix is a canonical sort by key; the test that pins it is
`a_snapshot_is_byte_identical_for_the_same_state_whatever_order_it_was_built_in`.

**Scenario B — scoped mandates** (`src/mandate/scenario_b.rs`, PR #260). The decisive test of
`docs/design/scoped-mandates.md` §8, built as a **schedule sweep** rather than five hand-written cases: the
invariant (*nothing authorized only under a superseded epoch commits*) is asserted after **every step of every
schedule**. The case a hand-written test omits is the one the ADR singles out — revocation with **no**
subsequent write — so the sweep includes idle schedules that knock only much later. It also asserts its own
size, so it cannot quietly shrink.

**The discipline both established, worth reusing: verify a gate by breaking the thing it guards, in both
directions.** Every gate here was checked by planting the failure it exists to catch (a resource that ignores
its installed epoch; a byte-order dependence) *and* by confirming the honest case still passes (a current
mandate still commits; the same state still snapshots identically). A gate that only refuses is satisfied by a
system that does nothing — scenario B's `a_current_mandate_still_commits` and the knowledge gate's positive
controls exist for exactly that reason. Ledger: [history](../history.md) → *item 6 PRs 4–5*.

## Replay scenario C — the interacting governors (item 6 PR 6, 2026-09-18)

`src/control/scenario_c.rs` (test-only): the combined-feedback harness `docs/design/adaptive-stability.md` §5
promised as *replay stage 6, built once, reusing the governors' pure decision functions*. Same shape as B — a
schedule sweep (48 schedules × 2 profiles) with the invariants asserted after every step, a witness, and a size
assertion — over the **shipped** decisions: `TuningGovernor::gate_at`/`acted_at`, `opacity_state_for` →
`opacity_transition` → `spaced_transition`, `membership_governor::{decide, classify}` under `control::decide`,
with their real spacing, settling and hysteresis. What is *modelled* is the plant that couples them: this node's
share of a fleet inbound, halved while opaque, minus what the writer drains; an advisor recommending a writer
depth from group size and load. The objectives are numbers — two **releases** never closer than 300 ms, two
knob changes never closer than 200 ms, at rest 15 ticks after the last disturbance, no routine scale-down on a
stale view under an enforcing profile — so the witness (every breaker off) can violate them, and does: release
flaps and knob chatter.

**Three lessons.** *State the objective on what the breaker governs:* the first sweep said "any two transitions"
and failed under the shipped breakers on a release re-shed 100 ms later — the decisive rule working, not a
flap. *A first-only violation report masks:* the witness said "no flap" while the same schedules chattered.
*A plant that is not caught is a finding:* removing the hysteresis in shipped code did **not** fail the sweep —
the 1 s release spacing alone bounds the release rate, so a lost hysteresis only changes how often a proposal is
held; recorded as what the sweep does not prove. The tuning plant (no spacing) was caught at once. **Not
shown:** the ADR's sharper sentence — loops oscillating together while each is stable alone — the witness
removes every breaker at once.

*Attempted 2026-09-22, and the reason it stays unshown is now precise rather than vague.* To ask
whether stability **composes** you have to run one loop closed and the others not, which means
deciding what "the others" do meanwhile. Two ways were built and measured; **neither can pose the
question**, for opposite reasons:

- **Freeze the other actuators at their initial values.** This does not isolate a loop, it removes
  the plant's means of relief: with opacity and the writer knob frozen, `drain` is pinned at 0.125
  while `share` reaches 0.35, so `fill` saturates and stays there. The run diverges for want of an
  actuator. Measured: 24/96 schedules "violate" under *membership alone* — an uncontrollable plant,
  not an unstable loop, and reading it as the ADR's sentence would have been a fabricated result.
- **Drive the other actuators from the coupled run's own trace.** This keeps the plant controllable
  and every exogenous condition identical, which is the right shape — but the reference *carries the
  coupled dynamics into the isolated run*. Measured with the breakers off: **24/96 for every
  configuration alike**, one-loop and three-loop, with identical violation counts. The chatter is
  imported from the trace, not generated by the loop under test.

With the shipped breakers on, every configuration is quiet: **0 of 96 schedules × 2 enforcing
profiles**, coupled or isolated. So what the harness does establish is that the shipped breakers
hold across this sweep; what it cannot establish is whether they are *needed* because of coupling.

The obstacle is structural, not a missing afternoon's work: **the three loops share one state
variable.** Any way of holding "the others" fixed is either uncontrollable or is the coupled run
wearing a disguise. Demonstrating the ADR's sentence needs a plant where the loops act on
*different* variables that interact, which this one is not. Recorded so the next attempt starts
after these two rather than repeating them. Ledger: [history](../history.md) → *item 6 PR 6*. *Since the profile ladder was wired through every governor
(item 4 §7, 2026-09-18):* the "settles" sweep runs under the enforcing profiles, where the breakers act; under
`Legacy` the same breakers are not consulted and the schedules flap; under `Observe` they flap while counting
every hold not made — `Legacy` is the ladder's own witness.

## The replay corpus (item 6 PR 7, 2026-09-18)

`mycelium-core/tests/replay-corpus/<name>/` holds checked-in bundles; the first is scenario A (thirteen
effects). Two tests in `persistence.rs`'s `durability_tests` own it. **The recorder**,
`record_scenario_a_into_the_corpus`, runs only under `MYCELIUM_RECORD_CORPUS=1` and rewrites the entry from a
fresh recording — the same code records the same bytes, so a re-record is reviewed as a diff of `choices.trace`,
the way the seams baseline is regenerated on purpose. **The gate**,
`the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording`, runs in every `--features sim`
suite (CI's `mycelium-core --features sim` job): the committed schedule must replay on *this* machine without
divergence, the bundle must still name the witness toggle this build knows, and a fresh recording must ask for
the same effects in the same order. That last check is what makes the corpus a gate rather than a souvenir: a
new effect or changed bytes in the snapshot path fails the suite, with the differing line printed from both
sides, until someone re-records deliberately. Verified by tampering one content hash in the committed trace (the
gate fails on that effect) and re-recording (identical bytes). Not built: the minimiser and a replay binary. Log:
[`.log/2026-09-18-item6-pr7-replay-corpus.md`](../.log/2026-09-18-item6-pr7-replay-corpus.md).

## The scheduler seam's first arm (item 6, 2026-09-19)

`mycelium_core::sim_seam::pause_clock_for_replay` (paired with `resume_clock_after_replay`) makes a replayed
wait *ordering information* again: taken on tokio's paused clock, so it costs no wall time but still orders
this task against every other waiting task. Before it, every replayed wait collapsed to one `yield_now`, and a
whole node's tasks resumed in whatever order the runtime chose.

Two tests, at two scales, and the pair is the point:

- **The unit** (`sim_seam::tests`): two tasks, 50 ms spawned *before* 10 ms, so spawn order and completion
  order disagree. Armed, the replay reproduces the recorded order; unarmed
  (`without_the_paused_clock_the_same_two_waits_diverge`), the same trace diverges. This is the test that
  found the real blocker — see below.
- **The whole node** (`mycelium-commitment`): a started `GossipAgent` committing a linearizable award,
  recorded and replayed on the same identity. This was a **pin on a gap** for a day; it is now the claim
  (`a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam`), with the unarmed replay
  kept beside it as the plant.

**What actually blocked it, recorded because the design note guessed wrong.** Pausing the clock alone did not
flip the pin. `Record` wrote its trace entry *after* the wait and `Replay` checked its request *before* it, so
a recording's order was the order waits **completed** while a replay's was the order tasks **entered** them —
different whenever two waits overlap, which is the only case the arm exists for. The seam now checks in at the
same point in both modes.

**What it does not cover:** a multi-threaded runtime (tokio's clock control refuses one), real peers (the
network seam is still unbuilt), and two tasks that are both runnable at the same instant with no wait between
them — a paused clock orders waits, it does not choose between ready tasks.
