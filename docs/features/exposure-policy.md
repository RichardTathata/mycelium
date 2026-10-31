# Causal exposure policy

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

The [causal exposure policy](../guide/00-concepts.md#exposure-policy) is a `[[rule]]` with an `exposure` table. It
holds new work that would **extend an unresolved closure** — work whose causal fields name an effect whose outcome is
not known, under the [side-effect contract](../guide/00-concepts.md#side-effect-contract) — while that closure is past
a threshold: how many unresolved effects it holds, or how old the oldest is. Work that names no unresolved effect is
independent and passes, as [selective recovery](../guide/00-concepts.md#selective-recovery) leaves it untouched, and
the policy counts at a [history scope](../guide/00-concepts.md#history-scope) as a history limit does.

## When to use it, and when not

- **Use it when** a run should stop piling new work onto effects whose outcome it does not know — payments sent into
  a destination that has not answered — until a probe finds out, while the same agent's unrelated work goes on.
- **Not when** you need an exact fleet-wide bound: the fleet scope is a gossiped tally, approximate, and the exact
  (consensus) scope is not built; nor to bound how often an actor acts — that is a history limit.

## Turn it on

This feature is turned on in code. The rule is a `[[rule]]` with an `exposure` table
([unit-file § `[[rule]]`](../reference/unit-file.md#rule)); the embedder builds the policy from the rules it loads
(`RuleDecl::to_exposure_rule`) and attaches it beside the evaluator. A request names what it depends on with its
causal fields, `_meta.parent` and `_meta.correlation`.

- [`with_exposure_policy`](../../src/agent/mod.rs) — attaches the policy; without an action evaluator the doors
  decide nothing and it is never consulted.
- [`with_exposure_source`](../../src/agent/mod.rs) — attaches where the unresolved effects are read; an
  `enforcement_point` rule on a node without one is *not established* for every request it selects.
- [`ExposurePolicy`](../../src/agent/exposure.rs) — the rules, each an actor, operation and resource selector with
  its two thresholds and its scope.
- [`ExposureSource`](../../src/agent/exposure.rs) — what holds the effects; the external-effect `Reconciler`
  implements it, listing its records whose outcome is not known by their prepared record's hash, their latest
  execution evidence's hash and their run.
- [`set_control_profile`](../../src/agent/control_profile.rs) — the rung the policy acts under: untouched under
  `legacy`, shadow under `observe`, held under `enforce-local` and `enforce-allocated`.
- [`DOOR_OPERATIONS`](../../src/agent/door.rs) — the operations of the take, claim and emit doors (adopter-surface
  Y11); a rule naming one of them, or `*`, selects that door, and each gateway route reads the body's `_meta`.
- [`emit_in`](../../src/agent/door.rs) — the emit door's Rust verb, taking the causal fields; the mailbox,
  reliable-emit and sharded-emit verbs sit beside it on the service handle.
- [`take_in`](../../mycelium-tuple-space/src/lib.rs) — the take door's Rust verb; the keyed take's is beside it.
- [`claim_in`](../../mycelium-blackboard/src/lib.rs) — the claim door's Rust verb.
- `independent_work_passes_while_work_extending_the_closure_is_held`, in
  [`src/agent/exposure_tests.rs`](../../src/agent/exposure_tests.rs): on an enforcing rung, a call naming the
  unresolved effect as its parent, or carrying its run, is held, and a root call or one naming nothing unresolved is
  admitted.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/agent/exposure_tests.rs`](../../src/agent/exposure_tests.rs):
`independent_work_passes_while_work_extending_the_closure_is_held`, `a_compensation_is_admitted_while_a_new_dependent_call_is_held`
(the plant that makes the rule structural: run in the shared seam too, the gate refuses the compensation and this test
fails), `the_shadow_profile_counts_and_holds_nothing`, `a_policy_denial_counts_nothing`,
`the_aged_record_threshold_fires_with_the_size_threshold_unmet`, `an_unreadable_closure_is_not_established_on_an_enforcing_rung`,
`the_fleet_scope_reads_others_skips_its_own_key_and_removed_members`,
`publishing_departing_and_collecting_write_what_readers_rely_on` and `unknown_coverage_in_the_tally_is_not_established`. The doors' tests, in [`tests/exposure_doors.rs`](../../tests/exposure_doors.rs): every take, claim and emit door, by
its Rust verb and its gateway route, held and admitted (`every_door_holds_work_extending_an_unresolved_closure_and_admits_independent_work`),
counted in shadow, untouched when no rule selects it (`a_door_no_rule_selects_is_untouched`), and its record carrying
the fields.

## What it guarantees, and what it does not

- The rule is [`ae.exposure`](../reference/rule-catalogue.md): a selected, permitted call that extends an unresolved
  closure is held when that closure holds `max_unresolved` effects or its oldest is older than `max_oldest_age_ms`,
  each threshold on its own; the hold is a deny naming the threshold, recorded before nothing is dispatched.
- **Only dependent work.** A request extends a closure when its `parent` is an unresolved effect's prepared record or
  any execution evidence it has had (a probe writes a new one; the earlier ones still name it), or its `correlation`
  is that effect's run. A request whose parent is a *descendant* of one in another run, naming no unresolved effect
  itself, is not followed through the evidence journal (not built).
- **At the take, claim and emit doors, opt-in** (Y11, the plan's D29). A tuple take, a blackboard claim and the four
  emit doors are decided only when a rule selects their operation; otherwise they are untouched — no decision, no
  record, no clock read, and a malformed `_meta` is not read. A rule whose operation is `*` selects them too. Their
  causal fields go on the door's own decision record only (`node:{door}` for a Rust verb, its actor the node,
  `node:{id}`; `gateway:{door}` for a route, its actor the caller), never onward: a signal frame and a tuple's WAL
  record carry no field for them. A door asking under an `EXEMPT_OPERATIONS` name is refused by name.
  - **Which verbs are doors.** The `*_in` verbs — `emit_in`, `emit_ordered_in`, `emit_async_in` on the agent;
    `deliver_event_in`, `emit_reliable_in`, `emit_sharded_in` on the service handle; `take_in` and `take_by_key_in` on
    the tuple space; `claim_in` on the board — and the seven gateway routes. A raw gateway call of a companion's take
    or claim kind (`/gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable` of `tuple.{ns}.take`,
    `tuple.{ns}.take_by_key`, `blackboard.{ns}.claim`) runs that door's decision, selected or untouched exactly as the
    door is — **only at a gateway node that hosts the companion**, since only the companion registers its kinds:
    through a gateway without it the call reaches the provider undecided, and the decision at the provider is not
    built. A reliable emit of a door's kind that the overlay door already decided (a `*` rule selects both) is
    decided once, by the overlay door: one record, one count. **Never decided:** the plain verbs (`emit`,
    `emit_ordered`, `emit_async`, `request`, `advertise`, `advertise_persistent`, `mycelium_core::ops`'s
    `emit_signal`, `emit_signal_ordered` and `emit_signal_async`, `deliver_event`, `emit_reliable`, `emit_sharded`,
    `take`, `take_by_key`, `claim`, a member's own `rpc_call`, `rpc_call_with_mandate`, `rpc_call_as`,
    `scatter_gather` and `bulk_call`), a hosted WASM component's `mesh.emit`, and a remote worker's take or claim where
    the primary serves it: a door is decided on the node the work starts on.
  - **The resources a rule selects on:** `tuple:{ns}/{stage}` for a take (a raw call whose payload names no stage is
    decided on `tuple:{ns}/`), `bb:{ns}` for a claim (a predicate names no single resource), `signal:{kind}` for the
    four emits.
  - **On the shadow rung a selected door holds nothing,** whatever the evaluator says: a refusal the evaluator decides
    is recorded as decided, with what the exposure gate would have done, and the door admits — its record's execution
    `attempted`, then the door's own outcome. Only a permit is counted. Four cases are still refused on the shadow
    rung, because the door cannot act on them or cannot say it did: a malformed `_meta`, an evaluator that panics, a
    door asking under an exempt operation name, and a decision record that cannot be written under a `Strict`
    evidence journal.
  - **A parked take is decided before it waits,** so its check-then-act window is the park time.
  - Not built: the SDKs raise a door's 403 as their verbs' ordinary error and do not surface its `reason`.
- **The omission limit.** A call with neither `_meta.parent` nor `_meta.correlation` is a root and is never held: the
  policy bounds causal extension as each envelope declares it, so it binds only callers that propagate their lineage.
  A cap on what an actor does whatever it declares is row K2's history rules.
- **Where the causal fields reach.** At the gateway's doors, the request's own `_meta`. At the provider check, only
  `mcp.invoke` carries them (its JSON-RPC `params._meta`); a `skill.invoke` payload is the skill's text and an
  operator's protected kind's is opaque, so the provider reads either as a root and never holds it on exposure — the
  door that dispatched it decided on the request's own `_meta` (`/a2a` for a skill). The caller envelope carries no
  causal fields by design.
- **What it never holds is structural.** The gate is in the gateway's decision, not in the shared seam a compensation
  is decided through; a probe passes no evaluator; a protective shed and a rescue from zero are governor actions, not
  requests; and at a door the operations in `EXEMPT_OPERATIONS` (a probe, a compensation, a leave, a grant's issue and
  revocation, named exactly) are exempt whatever a rule selects (the [module documentation](../../src/agent/exposure.rs)
  says why). A `protected_rpc_kinds` entry named as one of them is refused at start.
- **How a closure ends.** A probe that finds an *unknown* effect's fate resolves it. An effect escalated *unresolved* —
  its probe budget spent, or left as irreversible and unobservable — is terminal: a compensation takes only a committed
  effect, and no operator route resolves one. Such an effect holds only the work that names it, for good; work outside
  its cone is never held.
- **Unknown coverage fails closed** on an enforcing rung: a source that cannot read its records, an `enforcement_point`
  rule on a node with no source, a peer with no summary or an unreadable, stale, future-dated or truncated one, or a
  member out of view whose last summary lists an effect the request extends — each is *not established*, never an
  empty closure. In shadow it is counted and nothing is held.
- **Shadow first.** Under `legacy` the policy is neither read nor counted. Under `observe` it decides and counts
  (`GossipAgent::exposure_counts`: `would_hold`, `would_not_establish`) and holds nothing; the evidence record of each
  call it would have held says so. Only a call the evaluator permitted is counted.
- **The size bound is approximate** under concurrency at both scopes: two requests read the closure before either's
  own effect is prepared, so both can pass a size threshold one of them reaches.
- **The fleet tally is approximate** — detection, not prevention, as row K2's tally is: each node's summary is
  republished on the tally heartbeat's tick, so a peer's closure can grow unseen within the staleness bound. A
  `fleet_tally` rule fails closed until every peer publishes, so it waits for the whole fleet to be upgraded — and
  only a node that attaches an exposure policy with a rule, or an exposure source, publishes: a node with neither is
  inert (no summary, no collection, no clock read, so a recording made without the feature replays unchanged), and
  a fleet-tally rule reads it as unknown coverage. A gateway
  with no source of its own reads only the summaries. An out-of-view node's unreadable, future-dated or non-member
  summary is dropped and counted, and any summary past the history horizon is collected — the one place the policy
  forgets an unresolved effect. Summaries are unsigned — the limit is the last *not built* item below.
- Not built, stated in the plan's Y5 row (the narrowing the maintainer accepted, 2026-10-30):
  - **the consensus-seat scope for an exact bound** — a closure *shrinks* as its effects resolve, so a seat would need a decrement no fixed lease expresses, and concurrent admissions overshoot without a reservation taken before the work is admitted; `fleet_consensus` is refused at load, by name.
  - **holding at a tuple take and a signal emit** — delivered by row Y11, opt-in and with door-only lineage (above); not built there: carrying the lineage onward through the signal frame or the tuple WAL, and deciding these doors whenever an evaluator is attached without a selecting rule.
  - **an operator route that resolves an `Unresolved` record** — such a record is terminal, a compensation takes only a committed effect, so it holds the work that names it for good.
  - **the limit of declared lineage** — a caller that omits `_meta.parent` (or names only a descendant in another run) is not held: the policy bounds causal extension as each envelope declares it, and per-actor caps are row K2's history rules.
  - **unsigned tally entries** — row K2's limit: a member can write another's summary, counted by the `sys/` tripwire at the owner; for an owner in view its next tick overwrites the forgery, but a forged summary for an owner **out of view** that marks it `departed` holds the digests it lists fleet-wide until the 24-hour history horizon collects it; and without `[tls]` one under an id this node cannot rule out is read the same way.
- The [what-is-proven](../operations/what-is-proven.md) ledger has no row for it yet.

## Go deeper

- The design record is the plan's D18 and D29 and rows Y5 and Y11 ([`plans/adopter-surface.md`](../plans/adopter-surface.md)); the
  decisive rule it applies unchanged is the stability record's
  ([`design/adaptive-stability.md`](../design/adaptive-stability.md) §2).
- The code: [`src/agent/exposure.rs`](../../src/agent/exposure.rs); the gate's place is `ae_decide` in
  [`src/agent/http/authz.rs`](../../src/agent/http/authz.rs), and the take, claim and emit doors' is
  [`src/agent/door.rs`](../../src/agent/door.rs).
- The profile ladder's runbook is [`operations/control-profiles.md`](../operations/control-profiles.md); no
  runbook of its own.
