# Fleet-wide history limits

## What it is

A history-limited rule, such as *at most three posts in ten minutes*, can count an actor's calls across the whole
fleet instead of at one enforcement point. The rule names a [history scope](../guide/00-concepts.md#history-scope)
and a counter id: `FleetConsensus` counts exactly, by claiming one of N consensus seats before a call is admitted,
and `FleetTally` counts approximately, by summing the permits every enforcement point gossips. Without a fleet scope
the count stays per enforcement point, so an actor spread over N gateways can be admitted up to N times the limit.

## When to use it, and when not

- **Use it when** a limit must hold for an actor however many gateways or providers its calls reach.
- **Not when** a per-point limit is enough, the limit is above 1 024 in a window or the window is longer than 24
  hours (not established), or the exact scope cannot get a quorum and the tally cannot verify every member (both
  fail closed).

## Turn it on

This feature is turned on in code.

- [`Rule::at_most`](../../src/agent/action_evaluator.rs): the limit and its window, as for a per-point limit.
- [`Rule::scoped`](../../src/agent/action_evaluator.rs): the fleet scope, `HistoryScope::FleetConsensus` or
  `HistoryScope::FleetTally`, and an opaque counter id.
- [`with_action_evaluator`](../../src/agent/mod.rs): attach the evaluator, and an evidence journal with
  `with_evidence_journal`, in a build with features `gateway` and `tls`; without a journal every history view is
  unavailable.
- [`start_consensus_listener`](../../src/agent/consensus_handle.rs): for `FleetConsensus`, seats are decided by
  consensus, so the voting nodes run a consensus listener (the tests start one on every node; guide 20 does not
  say so).
- Settings the count reads, each a row of [configuration.md § The fields](../reference/configuration.md#the-fields):
  `control_max_staleness_ms`, `control_min_peers_heard` and `max_clock_drift_ms` (the tally's coverage), `tls` and
  `require_identity_proofs` (which members the tally can verify), and `consensus_require_electorate` (where seats
  are decided).
- `a_saturating_actor_gets_its_limit_each_short_window`, in
  [`src/agent/fleet_history_tests.rs`](../../src/agent/fleet_history_tests.rs): builds its policy with
  `at_most(..).scoped(..)` and attaches it as above. It does not read guide 20's snippet; that snippet has no test.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The end-to-end runs are tests in
[`src/agent/fleet_history_tests.rs`](../../src/agent/fleet_history_tests.rs), against live nodes:
`the_consensus_scope_admits_at_most_n_across_three_gateways_under_concurrent_calls` (exact),
`the_tally_scope_converges_and_refuses_after_convergence` (approximate) and
`a_saturating_actor_gets_its_limit_each_short_window`. The ledger row below names them and where they run.

## What it guarantees, and what it does not

- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight), whose inputs include the fleet counter's
  seats and the fleet tally.
- **Guarantees:** [`ae.authorised_at_seam` and `ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#the-descriptors)
  cover each decision. No guarantee id is specific to the fleet scopes.
- **Contract:** the nine `fleet_*` cases and `PolicyClause::AllowAtMostAcross` in
  [`ae_contract.rs`](../../src/agent/ae_contract.rs), which a replacement evaluator must pass or decline
  ([guide 20 § Checking your evaluator against the contract](../guide/20-authorising-actions.md#checking-your-evaluator-against-the-contract)).
- **Bounds:** at most `MAX_FLEET_SEATS` (1 024) and a window of at most `MAX_FLEET_WINDOW_MS` (24 hours), in
  [`action_evaluator.rs`](../../src/agent/action_evaluator.rs); outside them the rule is refused, not clamped. Not yet
  released ([deprecations §55](../guide/deprecations.md#55-a-fleet-limit-is-bounded-at-most-1-024-seats-a-window-of-at-most-24-hours-unreleased-post-360-plan-row-k2s-bounds)).
- **Not built:** row K2 of the [post-360 plan](../plans/post-360-hardening.md#33--the-batch) and its *Not built,
  stated* list in [§3.5](../plans/post-360-hardening.md#35--delivered-rows-2320-onward--each-promise-beside-its-evidence):
  tally entries are unsigned, so a forgery under a dead member's namespace is not detected; without `[tls]` a
  non-peer inside the horizon cannot be verified and the tally reads `Indeterminate`; a seat's release marker is not
  checked against its owner. The tally is detection, not prevention, and the exact count holds to within the nodes'
  wall-clock skew.
- **Evidence:** the fleet history row of
  [what-is-proven § Proven in CI on every merge](../operations/what-is-proven.md#proven-in-ci-on-every-merge).

## Go deeper

- **Guide:** [20 § Deciding on what an actor has already done](../guide/20-authorising-actions.md#deciding-on-what-an-actor-has-already-done),
  its *Across the fleet (row K2)* part and *Bounds*.
- **Runbook:** none specific to the fleet scopes. Each decision is an evidence record:
  [audit § 10, Evidence export](../operations/audit.md#10-evidence-export--what-a-gap-looks-like-to-the-consumer).
- **Design:** no design record of its own; row K2 of the post-360 plan (above) is the record, and a seat is a
  decision record of the [lock lifecycle](../design/lock-lifecycle.md). Upgrade notes:
  [deprecations §53](../guide/deprecations.md#53-a-history-limited-rule-may-count-across-the-fleet-two-new-scopes-and-five-structs-gained-a-field-2360-post-360-plan-row-k2)
  and §55 (above). Maintainer notes: [wiki dev § V3 runtime authorisation and evidence](../wiki/dev/dev.md#v3-runtime-authorisation-and-evidence-the-ae-slice).
- **Code:** [`src/agent/fleet_history.rs`](../../src/agent/fleet_history.rs) (`establish`, `claim_seat`,
  `publish_permit`), [`src/agent/action_evaluator.rs`](../../src/agent/action_evaluator.rs) (`HistoryScope`,
  `FleetCounter`, `check_fleet_bounds`).
