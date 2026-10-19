# Inference routing as policy

## What it is

When `mycelium-reason`'s router chooses among several providers of a model, the action evaluator decides each
candidate before the router commits to it. The evaluator sees where the call would go — the destination and
its declared [locality](../guide/00-concepts.md#data-class), in the domain or outside it — and the
[data class](../guide/00-concepts.md#data-class) of the request, so a policy can say, for example, that
confidential data goes only to an in-domain model. The router dispatches to the first permitted candidate, or
refuses the call naming every candidate it passed over.

## When to use it, and when not

- **Use it when** some prompts must not leave the domain, or must go only to certain models, and callers reach
  models through the reason router's gateway doors.
- **Not when** you need the content inspected or the provider's real behaviour observed: the class is declared,
  never detected, and the locality is the provider's own declaration.

## Turn it on

This feature is turned on in code. Either reason door gives the preflight each candidate's destination and
declared locality; only the data class needs the classifying door and a class policy.

- [`reason_router_classified`](../../mycelium-reason/src/http.rs): the door that classifies each request; without
  an action evaluator it refuses every request. The plain `reason_router` sends destination and locality, no class.
- [`DataClassPolicy`](../../mycelium-reason/src/classify.rs): the classes, least to most restrictive, and the floors
  bound to caller principals. A client may raise its class with the `x-mycelium-data-class` header; no SDK sends
  that header yet.
- [`with_action_evaluator`](../../src/agent/mod.rs): attach the evaluator, and an evidence journal, before
  `start()`, in a build with feature `tls`.
- [`Rule::for_class`](../../src/agent/action_evaluator.rs): a rule that applies only to one class (with
  `Rule::at_locality`, only at one locality).
- [`ModelProfile::in_domain`](../../mycelium-reason/src/route.rs): a provider's declaration of where its inference
  runs (or `external()`); one that declares nothing is unknown, never in-domain.
- `guide_20_routing_policy`, in [`mycelium-reason/tests/gateway.rs`](../../mycelium-reason/tests/gateway.rs): runs a
  hand-kept copy of the wiring in
  [guide 20 § Deciding where data may go](../guide/20-authorising-actions.md#deciding-where-data-may-go-inference-routing-as-policy),
  not the page itself.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature, and its reasoning examples mount the plain router. The end-to-end run is a test in
[`mycelium-reason/tests/gateway.rs`](../../mycelium-reason/tests/gateway.rs):
`a_confidential_request_is_routed_to_an_in_domain_model_and_the_external_one_is_passed_over`. The ledger row below
names it and where it runs.

## What it guarantees, and what it does not

- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight), whose inputs include the routing door's
  destination, locality and data class.
- **Guarantees:** [`ae.authorised_at_seam` and `ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#the-descriptors)
  cover each candidate's decision, which is its own evidence record. No guarantee id is specific to routing.
- **Contract:** the five `route_*` cases and `PolicyClause::ForbidClassAt` in
  [`ae_contract.rs`](../../src/agent/ae_contract.rs), which a replacement evaluator must pass or decline
  ([guide 20 § Checking your evaluator against the contract](../guide/20-authorising-actions.md#checking-your-evaluator-against-the-contract)).
- **Not built:** the *What this does not establish* list in
  [guide 20 § Deciding where data may go](../guide/20-authorising-actions.md#deciding-where-data-may-go-inference-routing-as-policy),
  and row R of the [post-360 plan](../plans/post-360-hardening.md#33--the-batch), closed on evidence in
  [§3.5](../plans/post-360-hardening.md#35--delivered-rows-2320-onward--each-promise-beside-its-evidence). The
  node's own in-process calls and the provider's re-check consult neither class nor locality.
- **Evidence:** the routed-call row of
  [what-is-proven § Proven in CI on every merge](../operations/what-is-proven.md#proven-in-ci-on-every-merge).

## Go deeper

- **Guide:** [20 § Deciding where data may go](../guide/20-authorising-actions.md#deciding-where-data-may-go-inference-routing-as-policy);
  [15, reasoning and LangGraph](../guide/15-reasoning-and-langgraph.md) for the router itself.
- **Runbook:** none specific to routing. Each candidate's decision is an evidence record:
  [audit § 10, Evidence export](../operations/audit.md#10-evidence-export--what-a-gap-looks-like-to-the-consumer).
- **Design:** no design record; row R of the post-360 plan (above) is the record. Upgrade notes:
  [deprecations §52](../guide/deprecations.md#52-the-evaluator-sees-where-a-routed-call-would-go-and-what-it-carries-2360-post-360-plan-row-r).
  Maintainer notes: [wiki dev § V3 runtime authorisation and evidence](../wiki/dev/dev.md#v3-runtime-authorisation-and-evidence-the-ae-slice).
- **Code:** [`src/agent/route_facts.rs`](../../src/agent/route_facts.rs) (`Destination`, `Locality`, `DataClass`),
  `gateway_preflight_routed` in [`src/agent/http/authz.rs`](../../src/agent/http/authz.rs),
  [`mycelium-reason/src/route.rs`](../../mycelium-reason/src/route.rs) (the router asking per candidate) and
  [`classify.rs`](../../mycelium-reason/src/classify.rs) (the classes and floors).
