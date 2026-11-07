# Replay bundles

## What it is

A [bundle](../guide/00-concepts.md#bundle) is a recorded run a reviewer replays exactly: what the production code
asked for and what it received at every decision — clocks, named random streams, timers, scheduling, storage,
external inputs — plus the build identity, the configuration and, when there is one, the witness that failed. Each of
those decisions goes through a [seam](../guide/00-concepts.md#seam), a replaceable adapter behind which the same
production logic runs. A replay checks every request against the recording, so a changed build says where it first
departed rather than merely re-seeding. The `mycelium-sim` crate is the harness; the seams are off in every shipped
build.

## When to use it, and when not

- **Use it when** a failure must be reproduced by someone else, later, on another build, or a fix must be shown to
  remove a failure the bundle's witness records.
- **Not when** you need a multi-node schedule or task interleaving beyond one node's waits, or anything at runtime:
  a shipped build does not link the harness ([guide 19 § What this does not establish](../guide/19-replay-and-simulation.md#what-this-does-not-establish)).

## Turn it on

This feature is turned on in code: build with `sim` (on `mycelium`, which turns on `mycelium-core`'s `sim` and pulls
in `mycelium-sim`). Without it the seams compile to the calls they replaced.

- [`install`](../../mycelium-core/src/sim_seam.rs): install a recording or replaying kernel for this thread before
  the node starts; one thread per node, a current-thread runtime.
- [`Bundle`](../../mycelium-sim/src/bundle.rs): `new` over the trace, `witnessed_by` the assertion that failed,
  `write` to a directory; `can_prove_its_failure` before believing a green replay.
- The node binary built with `cli,sim` records a run when `GOSSIP_RECORD_BUNDLE_DIR` is set, writing the bundle at
  shutdown; the bundle has no witness.
- The snippet in [guide 19 § Recording a node](../guide/19-replay-and-simulation.md#recording-a-node-not-a-function)
  is lifted from a test rather than read by one.
- `the_divergence_names_the_first_departure_not_merely_that_one_happened`, in
  [`mycelium-sim/tests/divergence.rs`](../../mycelium-sim/tests/divergence.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`replay_a_bundle`](../../examples/README.md#the-capability-matrix): a bundle recorded and replayed, and a changed
build caught at its first departure. CI runs it, and CI also records the co-op fleet through the node binary's
`test --faults replay` and replays it decision for decision.

## What it guarantees, and what it does not

- No guarantee or rule id: replay is the repository's harness, not a node's runtime behaviour, so nothing in the
  [guarantee catalogue](../reference/guarantee-catalogue.md) describes it.
- **Not built:** a multi-node schedule; task interleaving beyond the scheduler seam's arms; a recording from a
  persisted node replayed decision for decision
  ([what-is-proven § a persisted node's recording](../operations/what-is-proven.md#persisted-node-s-recording-replays-decision)).
  A seam not on the [inventory](../design/replay-nondeterminism-inventory.md) is not captured, and the gate
  `scripts/check-sim-seams.sh` keeps new direct clock, random and file calls out of the three crates it scans.
- **Evidence:** the ledger rows [a recorded node replays without divergence](../operations/what-is-proven.md#replays-without-divergence)
  and [a live node's recording replays decision for decision](../operations/what-is-proven.md#live-node-s-recording-replays-decision).

## Go deeper

- **Guide:** [19, replay and simulation](../guide/19-replay-and-simulation.md).
- **Runbook:** [diagnostics § Capturing a replay bundle from production](../operations/diagnostics.md#capturing-a-replay-bundle-from-production).
- **Design:** [`replay-nondeterminism-inventory.md`](../design/replay-nondeterminism-inventory.md).
- **Code:** [`mycelium-sim/src/`](../../mycelium-sim/src/lib.rs) (the kernel, the trace, the bundle) and
  [`mycelium-core/src/sim_seam.rs`](../../mycelium-core/src/sim_seam.rs) (the seams).
