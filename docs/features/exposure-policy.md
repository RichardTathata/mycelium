# Causal exposure policy

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

The [causal exposure policy](../guide/00-concepts.md#exposure-policy) is a `[[rule]]` with an `exposure` table. It
holds new work an actor asks a gateway's doors for while the actor's **unresolved closure** — its effects whose
outcome is not known, under the [side-effect contract](../guide/00-concepts.md#side-effect-contract) — is past a
threshold: how many there are, or how old the oldest is. It never holds the work that resolves the closure, the
probes and compensations of [selective recovery](../guide/00-concepts.md#selective-recovery), and it counts at a
[history scope](../guide/00-concepts.md#history-scope) as a history limit does.

## When to use it, and when not

- **Use it when** an agent should stop starting new work while the effects it already caused have no known outcome
  — payments sent into a destination that has not answered — until a probe, a compensation or an operator resolves
  them.
- **Not when** you need an exact fleet-wide bound: the fleet scope is a gossiped tally, approximate, and the exact
  (consensus) scope is not built; nor to bound how often an actor acts — that is a history limit.

## Turn it on

This feature is turned on in code. The rule is a `[[rule]]` with an `exposure` table
([unit-file § `[[rule]]`](../reference/unit-file.md#rule)); the embedder builds the policy from the rules it loads
(`RuleDecl::to_exposure_rule`) and attaches it beside the evaluator.

- [`with_exposure_policy`](../../src/agent/mod.rs) — attaches the policy; without an action evaluator the doors
  decide nothing and it is never consulted.
- [`with_exposure_source`](../../src/agent/mod.rs) — attaches where the closure is read; without one every request
  a rule selects is *not established*.
- [`ExposurePolicy`](../../src/agent/exposure.rs) — the rules, each an actor, operation and resource selector with
  its two thresholds and its scope.
- [`ExposureSource`](../../src/agent/exposure.rs) — what holds the effects; the external-effect `Reconciler`
  implements it, counting an actor's records whose outcome is not known.
- [`apply_as`](../../src/effects/external.rs) — applies an effect attributed to an actor, so the reconciler counts
  it; a composed effect is attributed to its composition's principal.
- [`set_control_profile`](../../src/agent/control_profile.rs) — the rung the policy acts under: shadow under
  `legacy` and `observe`, held under `enforce-local` and `enforce-allocated`.
- `a_compensation_is_admitted_while_a_new_dependent_call_is_held`, in
  [`src/agent/exposure_tests.rs`](../../src/agent/exposure_tests.rs): a node whose actor has an unresolved effect,
  on an enforcing rung, holds a new tool call at the door and permits the compensation the reconciler asks for.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/agent/exposure_tests.rs`](../../src/agent/exposure_tests.rs):
`a_compensation_is_admitted_while_a_new_dependent_call_is_held` (the plant that makes the rule structural: moved into
the shared seam, the gate refuses the compensation and this test fails), `the_shadow_profile_counts_and_holds_nothing`,
`the_aged_record_threshold_fires_with_the_size_threshold_unmet`, `an_unreadable_closure_is_not_established_on_an_enforcing_rung`,
`the_reconciler_reports_an_actors_unresolved_effects` and `unknown_coverage_in_the_tally_is_not_established`.

## What it guarantees, and what it does not

- The rule is [`ae.exposure`](../reference/rule-catalogue.md): a selected, permitted call is held when the closure
  holds `max_unresolved` effects or its oldest is older than `max_oldest_age_ms`, each threshold on its own; the hold
  is a deny naming the threshold, recorded before nothing is dispatched.
- **What it never holds is structural.** The gate is in the gateway's decision, not in the shared seam a compensation
  is decided through; a probe passes no evaluator; a protective shed and a rescue from zero are governor actions, not
  requests; and at a door the reconciler's own operations (`effect.*`) and a grant's (`grant.*`) are exempt whatever a
  rule selects (the [module documentation](../../src/agent/exposure.rs) says why).
- **Unknown coverage fails closed** on an enforcing rung: no source, a source that cannot read its records, a peer
  with no summary or a stale, future-dated or truncated one, or a node out of view whose last summary lists the actor
  — each is *not established*, never an empty closure. In shadow it is counted and nothing is held.
- **Shadow first.** Under `legacy` and `observe` the policy decides and counts (`GossipAgent::exposure_counts`:
  `would_hold`, `would_not_establish`) and holds nothing; the evidence record of each call it would have held says so.
- **The fleet tally is approximate** — detection, not prevention, as row K2's tally is: each node's summary is
  republished on the tally heartbeat's tick, so a peer's closure can grow unseen within the staleness bound.
  Summaries are unsigned; a member forging another's is counted by the `sys/` tripwire, not refused (K2's limit).
- What it counts: an effect the reconciler holds attributed to the actor (by `apply_as` or a composition). An effect
  applied with neither names no actor and counts toward none; a reconciler the application does not attach is
  outside the count.
- Not built, stated in the plan's Y5 row: the exact `fleet_consensus` scope (an exact bound needs a seat held until
  the effect resolves, which no fixed lease expresses — refused at load, by name); a gate at a tuple take or a signal
  emit (neither has an enforcement point today); unsigned summaries (K2's limit). The
  [what-is-proven](../operations/what-is-proven.md) ledger has no row for it yet.

## Go deeper

- The design record is the plan's D18 and row Y5 ([`plans/adopter-surface.md`](../plans/adopter-surface.md)); the
  decisive rule it applies unchanged is the stability record's
  ([`design/adaptive-stability.md`](../design/adaptive-stability.md) §2).
- The code: [`src/agent/exposure.rs`](../../src/agent/exposure.rs); the gate's place is `ae_decide` in
  [`src/agent/http/authz.rs`](../../src/agent/http/authz.rs).
- The profile ladder's runbook is [`operations/control-profiles.md`](../operations/control-profiles.md); no
  runbook of its own.
