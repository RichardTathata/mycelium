# Authorising actions

## What it is

An [enforcement point](../guide/00-concepts.md#enforcement-point) — the gateway, or a provider with enforcement on —
assembles an [action envelope](../guide/00-concepts.md#action-envelope) from facts it verified itself (who is asking,
to do what, to which resource, with which arguments) and asks a replaceable
[evaluator](../guide/00-concepts.md#evaluator) before the work runs. The answer is permit, deny or
[indeterminate](../guide/00-concepts.md#indeterminate), and indeterminate is never a permit. The decision carries the
[policy revision](../guide/00-concepts.md#policy-revision) it was made under and is recorded before dispatch.

## When to use it, and when not

- **Use it when** a verified caller's credential is not enough, and what that caller may do, to which resource and
  with which arguments must be decided by a policy you deploy and can show.
- **Not when** the provider can be reached without passing an enforcement point: a gateway's check covers only the
  routes it fronts ([guide 20 § The honest core](../guide/20-authorising-actions.md#the-honest-core-what-a-gateway-can-promise));
  turn on [provider enforcement](provider-enforcement.md) for that.

## Turn it on

This feature is turned on in code, in a build with `gateway` and `tls`. With no evaluator the gateway dispatches as
before.

- [`with_action_evaluator`](../../src/agent/mod.rs): attach the evaluator before `start()`. Without `compliance` it
  warns that decisions cannot be sealed into the audit chain.
- [`ActionEvaluator`](../../src/agent/action_evaluator.rs): the trait a policy engine implements;
  [`ReferenceEvaluator`](../../src/agent/action_evaluator.rs) is the in-tree one.
- [`with_evidence_journal`](../../src/agent/mod.rs): where each decision is recorded; without it an evaluator
  enforces and keeps no record of its decisions ([evidence journal](evidence-journal.md)).
- [`set_deployed_policy_revision`](../../src/agent/mod.rs): the revision the operator reports as deployed; until it is
  set the stale-policy check does not fire.
- [`preflight`](../../src/agent/action_evaluator.rs): the seam's checks for an enforcement point that is not this
  gateway; `evaluate` alone performs none of them.
- The wiring snippet in [guide 20 § Wiring it up](../guide/20-authorising-actions.md#wiring-it-up) is not run by any
  test; the gap goes to the `/doc-coverage` pass.
- `an_expired_envelope_is_denied_before_the_evaluator_runs`, in
  [`src/agent/action_evaluator.rs`](../../src/agent/action_evaluator.rs): the seam refuses before the evaluator is
  asked.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists [`mcp_tool_authority`](../../examples/README.md#the-capability-matrix): an evaluator deciding
which MCP tools an agent may call at the gateway, run in CI. [`a2a_skill_authority`](../../examples/README.md#the-capability-matrix) does the same at `/a2a`
([A2A interoperability](a2a.md)).

## What it guarantees, and what it does not

- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight) — permit, deny or indeterminate, recorded
  before the work runs; a refused mandate denies before policy runs.
- **Guarantees:** [`ae.authorised_at_seam`](../reference/guarantee-catalogue.md#ae.authorised_at_seam) and
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#ae.recorded_before_dispatch).
- **Contract:** the evaluator-neutral cases in [`ae_contract.rs`](../../src/agent/ae_contract.rs) (`cases`, `run`),
  which a replacement evaluator must pass or decline
  ([guide 20 § Checking your evaluator against the contract](../guide/20-authorising-actions.md#checking-your-evaluator-against-the-contract)).
- **Not built:** the list in [guide 20 § What this does not establish](../guide/20-authorising-actions.md#what-this-does-not-establish).
  A gateway's check is a route-level preflight: a provider reached another way is outside it unless it enforces too.
- **Evidence:** the ledger rows
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing) and
  [an evaluator can decide on what an actor has done](../operations/what-is-proven.md#evaluator-can-decide-what-actor-has).

## Go deeper

- **Guide:** [20, authorising actions](../guide/20-authorising-actions.md).
- **Runbook:** [rbac § 8, The action evaluator](../operations/rbac.md#8-the-action-evaluator--authorising-what-a-verified-caller-may-do).
- **Design:** [`action-envelope-ae0.md`](../design/action-envelope-ae0.md).
- **Code:** [`src/agent/action_evaluator.rs`](../../src/agent/action_evaluator.rs) (the envelope, the trait, the
  reference evaluator, `preflight`), `ae_preflight` and `ae_record` in
  [`src/agent/http/authz.rs`](../../src/agent/http/authz.rs).
