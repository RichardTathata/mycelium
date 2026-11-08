# Adaptive stability: governors and control profiles

## What it is

A [governor](../guide/00-concepts.md#governor) is a node-local loop that holds something to a published intent with no
coordinator: membership by self-election, tuning parameters a node has not pinned, timing as a set, opacity at the
boundary. The membership, tuning and opacity governors decide through one control contract (the timing governor
does not): a move is classified by its cost, and on a view the node cannot trust — too few peers heard, too stale —
speculative moves are held while protection and rescue never are; spacing and settling keep a loop from chasing its
own last change. How much of the contract binds is the node's control profile, a ladder from `Legacy` to `EnforceAllocated`, whose top rung backs ceilings with
[allocated rights](../guide/00-concepts.md#allocated-right) in a node-local, fsynced ledger.

## When to use it, and when not

- **Use it when** a fleet's loops — pool sizes, timing, load shedding — must not oscillate or act on a stale view, and
  you want to see what a rule would hold before it holds anything (`Observe`).
- **Not when** you need a hard bound without rights behind it: `EnforceLocal` budgets this node's own actuators, which
  is not a hard bound ([guide 22 § What this does not establish](../guide/22-stability-and-control.md#what-this-does-not-establish)).

## Turn it on

This feature is turned on in code. Every governor runs under `Legacy`, the default, until a profile is set; under
`Legacy` the control contract is not consulted, though the membership governor keeps its own cooldown and hysteresis
— its spacing and settling — as before.

- [`set_control_profile`](../../src/agent/control_profile.rs) with a [`Profile`](../../src/control.rs): `Legacy`,
  `Observe` (evaluated and counted, nothing held), `EnforceLocal` or `EnforceAllocated`.
- [`control_max_staleness_ms`](../reference/configuration.md#control_max_staleness_ms) and
  [`control_min_peers_heard`](../reference/configuration.md#control_min_peers_heard): the confidence bound a view
  must meet.
- [`govern_timing`](../../src/agent/introspect.rs)`(health_check_interval_secs, reconnect_backoff_secs, target)`,
  which publishes a [`TimingIntent`](../../src/agent/timing_governor.rs), and
  [`publish_membership_intent`](../../src/agent/membership_governor.rs) for a group
  ([governed membership](governed-membership.md)).
- [`RightsLedger`](../../src/control/ledger.rs): the node-local rights journal `EnforceAllocated` consults, through a
  stem provisioner's install rights.
- `observe_records_a_would_hold_and_legacy_never_consults_the_predicate`, in [`src/control.rs`](../../src/control.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`control_envelope_viz`](../../examples/README.md#the-capability-matrix): the same trace under hold and would-hold, in a
browser; CI compiles it and does not run it. [`elastic_intent`](../../examples/README.md#the-capability-matrix), which
CI runs, shows the membership governor holding a band under the default profile, not the ladder. That the ladder
changes what a breaker does is `under_legacy_the_breakers_are_not_consulted_and_under_observe_they_only_count`, in
[`src/control/scenario_c.rs`](../../src/control/scenario_c.rs).

## What it guarantees, and what it does not

- No guarantee id: the profile is read with `control_profile()` and on the tuning governor's snapshot
  (`GovernorSnapshot.profile`), not in the
  [guarantee catalogue](../reference/guarantee-catalogue.md); the membership governor's decision is
  [`membership.governed`](governed-membership.md)'s rule and the install-rights admission is the stem's.
- **Not built:** `EnforceLocal` is not a hard bound; the ledger is node-local, not a cluster-wide allocator; units
  are resource-native, never money. Combined feedback — loops oscillating together while each is stable alone — is
  not shown ([what-is-proven § combined-feedback oscillation](../operations/what-is-proven.md#combined-feedback-oscillation)).
- **Evidence:** the ledger row
  [hysteresis in the control loops is load-bearing](../operations/what-is-proven.md#hysteresis-control-loops-load-bearing).

## Go deeper

- **Guide:** [22, stability and control](../guide/22-stability-and-control.md).
- **Runbook:** [control profiles](../operations/control-profiles.md) (the ladder, the rollout, rollback).
- **Design:** [`adaptive-stability.md`](../design/adaptive-stability.md).
- **Code:** [`src/control.rs`](../../src/control.rs) (the contract, the profile), [`src/control/ledger.rs`](../../src/control/ledger.rs),
  [`src/agent/membership_governor.rs`](../../src/agent/membership_governor.rs),
  [`src/agent/tuning_governor.rs`](../../src/agent/tuning_governor.rs),
  [`src/agent/timing_governor.rs`](../../src/agent/timing_governor.rs).
