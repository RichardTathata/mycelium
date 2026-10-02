# 5 · Explain and replay a decision

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

## Dev notes

The controlled bug belongs to this example. It does not reproduce the library's
WAL/snapshot race, a multi-node schedule or arbitrary task interleaving. The real
regression corpus and CI coverage are linked from [guide 19](../19-replay-and-simulation.md).
The example runs in-process and needs no service cleanup. For customer code,
identify nondeterministic inputs first, then route them through the supported seams.
An evidence journal is valuable independently; it is not automatically such a bundle.
