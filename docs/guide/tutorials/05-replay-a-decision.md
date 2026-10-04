# 5 · Replay a bundle

> Formerly titled *Explain and replay a decision*; the decision trace and `mycelium explain` are in
> [guide 19 § reading what a node decided](../19-replay-and-simulation.md).

↑ [Tutorials](README.md) · Next: [Declared versus observed](06-declared-versus-observed.md)

## Objective

Turn a timing-dependent result into a recording that detects a changed decision.
Distinguish an authority journal (what a boundary recorded) from a replay bundle
(the choices needed to repeat a particular scenario).

## How to run

Prerequisites: Rust only. From the repository root:

```sh
cargo run -p mycelium-sim --example replay_a_bundle
```

Follow its numbered stages: capture, bundle, replay, deliberately diverging replay,
and the stated boundary of the demonstration. The grace-window bug in step 4 is
expected; the binary checks that the divergence is detected.

## What it demonstrates

Read [replay_a_bundle.rs](../../../mycelium-sim/examples/replay_a_bundle.rs).
The offer sweep observes a clock, scheduling randomness and storage effects through
record/replay seams. The bundle identifies the build and assertion as well as the
recorded choices. Removing the grace window changes a recorded effect: the replay
reports the discrepancy instead of silently accepting the new outcome.

## Try a change, then diagnose it

Change `GRACE_MS` and make a fresh capture by rerunning. Explain why a new recording
of changed behaviour can replay successfully. Then inspect step 4's deliberate
removal of the grace window against the original capture: that is the negative
case. A seed alone cannot supply missing clock or storage observations.

When a replay fails, inspect the first differing choice/effect and bundle build
identity before assuming the replay engine is faulty. Never relabel an unrecorded
choice as deterministic just to make the check pass.

## Then a whole node, decision for decision

The example above replays one task's choices. Since v2.22.0 a **live node** replays too — every
periodic loop's tick in the recorded order, even two due at the same instant — and the decision sink
reproduces the same records in the same order:

```sh
cargo test --features sim,test-util --test decision_trace_replay -- --nocapture
```

`a_recorded_node_replays_decision_for_decision` records a node joining a governed group, then replays
the trace under the paused clock and asserts the whole trace was consumed and the decisions compare
equal. Before the scheduler seam's second arm this diverged at the kernel's 20th choice, where two
timers fell due together; [guide 19](../19-replay-and-simulation.md) § reading what a node decided and
`docs/design/replay-nondeterminism-inventory.md` §3.1.2 say what the arm does and what it still does
not (which branch of a `select!` wins between two *different* events).

## Dev notes

The controlled bug belongs to this example. It does not reproduce the library's
WAL/snapshot race, a multi-node schedule or arbitrary task interleaving. The real
regression corpus and CI coverage are linked from [guide 19](../19-replay-and-simulation.md).
The example runs in-process and needs no service cleanup. For customer code,
identify nondeterministic inputs first, then route them through the supported seams.
An evidence journal is valuable independently; it is not automatically such a bundle.
