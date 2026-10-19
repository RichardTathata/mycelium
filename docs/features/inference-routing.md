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

This feature is turned on in code, not by a configuration field or a unit-file section.

- **The door:** mount `reason_router_classified` with a `DataClassPolicy` (the classes, least to most
  restrictive, and the floors bound to caller principals), in [`classify.rs`](../../mycelium-reason/src/classify.rs)
  and [`http.rs`](../../mycelium-reason/src/http.rs). A client may raise its class with the
  `x-mycelium-data-class` header.
- **The policy:** attach an evidence journal and an action evaluator before `start()`, with rules narrowed by
  `Rule::for_class` and `Rule::at_locality` ([`action_evaluator.rs`](../../src/agent/action_evaluator.rs)). The
  preflight needs a build with feature `tls`; without an evaluator, the classifying router refuses every request.
- **The providers:** each declares its locality with `ModelProfile::in_domain()` or `external()`
  ([`route.rs`](../../mycelium-reason/src/route.rs)); one that declares nothing is unknown, never in-domain.
- **Code a test runs:** the wiring in
  [guide 20 § Deciding where data may go](../guide/20-authorising-actions.md#deciding-where-data-may-go-inference-routing-as-policy)
  is compiled and run as the test `guide_20_routing_policy` in [`mycelium-reason/tests/gateway.rs`](../../mycelium-reason/tests/gateway.rs).

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature, and its reasoning examples mount the plain router. The end-to-end run is a test in
[`mycelium-reason/tests/gateway.rs`](../../mycelium-reason/tests/gateway.rs):
`a_confidential_request_is_routed_to_an_in_domain_model_and_the_external_one_is_passed_over`, which runs in CI's
`reason` job.

## What it guarantees, and what it does not

- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight), whose inputs include the routing door's
  destination, locality and data class.
- **Guarantees:** [`ae.authorised_at_seam` and `ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#the-descriptors)
  cover each candidate's decision, which is its own evidence record. No guarantee id is specific to routing.
- **Contract:** the five `route_*` cases and `PolicyClause::ForbidClassAt` in
  [`ae_contract.rs`](../../src/agent/ae_contract.rs), which a replacement evaluator must pass or decline
  ([guide 20 § Checking your evaluator against the contract](../guide/20-authorising-actions.md#checking-your-evaluator-against-the-contract)).
  The private Cedar adapter enforces no class or locality until its next re-pin.
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
- **Runbook:** none yet. The decisions it makes are evidence records, so the
  [enforcement point](../guide/00-concepts.md#enforcement-point) and its evidence journal are where an operator
  reads them.
- **Design:** no design record; row R of the post-360 plan (above) is the record. Upgrade notes:
  [deprecations §52](../guide/deprecations.md#52-the-evaluator-sees-where-a-routed-call-would-go-and-what-it-carries-2360-post-360-plan-row-r).
  Maintainer notes: [wiki dev § V3 runtime authorisation and evidence](../wiki/dev/dev.md#v3-runtime-authorisation-and-evidence-the-ae-slice).
- **Code:** [`src/agent/route_facts.rs`](../../src/agent/route_facts.rs) (`Destination`, `Locality`, `DataClass`),
  `gateway_preflight_routed` in [`src/agent/http/authz.rs`](../../src/agent/http/authz.rs),
  [`mycelium-reason/src/route.rs`](../../mycelium-reason/src/route.rs) (the router asking per candidate) and
  [`classify.rs`](../../mycelium-reason/src/classify.rs) (the classes and floors).
