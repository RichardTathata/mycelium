# Provider enforcement

## What it is

With provider enforcement on, every protected call a node receives — `mcp.invoke`, `skill.invoke` and any kind in
`protected_rpc_kinds` — runs the same action preflight a gateway runs, as the
[enforcement point](../guide/00-concepts.md#enforcement-point) `provider`, before any handler sees it. It applies
whichever door the call came through: a gateway, a member's direct call, or the SDK serve stream. A presented mandate
is verified by the node's own execution authority, never taken from a gateway, and the decision is recorded in the
node's evidence journal.

## When to use it, and when not

- **Use it when** a provider must not rely on every caller having passed a gateway: members can reach it directly,
  or more than one gateway fronts it.
- **Not when** the node has no action evaluator to attach: enforcement then fails closed and refuses every protected
  call with `no_evaluator`.

## Turn it on

This feature is turned on in code, in a build with `gateway` and `tls`.

- [`with_provider_enforcement`](../../src/agent/mod.rs): turn it on; with no evaluator attached, protected work is
  refused.
- [`with_action_evaluator`](../../src/agent/mod.rs): the evaluator that decides ([authorising actions](action-authorisation.md)).
- [`with_execution_authority`](../../src/agent/mod.rs): verifies a presented mandate at this node
  ([mandates](mandates.md)).
- [`with_cohort_budget`](../../src/agent/mod.rs): optionally, caps a declared population together at this provider;
  it applies with enforcement off too.
- [`protected_rpc_kinds`](../reference/configuration.md#protected_rpc_kinds): further kinds to treat as protected.
- `test_c7_the_bypass_matrix_no_door_runs_revoked_work`, in [`src/lib_tests.rs`](../../src/lib_tests.rs): after a
  revocation, no door runs the work, counted by the handlers themselves.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`authority_drain`](../../examples/README.md#the-capability-matrix): a node with provider enforcement on serves a
tool, the authority is revoked, and the node's own records give the time to the last admitted call and to the last
confirmed stop, checked against the class's bound. CI runs it.

## What it guarantees, and what it does not

- **Rule:** [`provider.enforcement`](../reference/rule-catalogue.md#providerenforcement) — a permit, or a refusal:
  the action preflight's (a deny, authority not established, evidence not recorded), `no_evaluator`, `caller_context_refused`, `malformed_call` (a tool call that is
  not JSON or names no tool), `resource_unnamed`, `resource_not_here`, `resource_not_served`, `at_capacity` or `mandate_lapsed_before_run` (the
  preflight permitted on an established mandate and the re-assessment that registers the work no longer finds it
  established — a revocation in between refuses the call rather than running it unregistered; it answers as
  *authority not established*, `data.detail` `mandate_lapsed_before_run`, and the evidence journal records that
  nothing ran, `execution: none`, beside the permit; under a seat claim that window can be a consensus round long). With
  enforcement off no authority is checked; an attached cohort budget still verifies the caller context and admits by
  cohort.
- **Guarantee:** [`prov.enforcement`](../reference/guarantee-catalogue.md#prov.enforcement).
- **Not built:** the check-then-act window between the check and the effect is narrowed and stated, not eliminated
  ([what-is-proven § the check-then-act window](../operations/what-is-proven.md#check-then-act-window)): a revocation
  before the work is registered refuses it, one after is the sweep's to stop; a provider
  that does not turn enforcement on checks no authority, whatever the gateway did, and one that turns it on with no
  evaluator refuses every protected call (`no_evaluator`).
- **Evidence:** the ledger rows [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing)
  and [running work stops](../operations/what-is-proven.md#running-work-stops).

## Go deeper

- **Guide:** [21 § Presenting and enforcing a mandate at the gateway and the provider](../guide/21-mandates.md#presenting-and-enforcing-a-mandate-at-the-gateway-and-the-provider-2150);
  [20 § Enforcing somewhere else](../guide/20-authorising-actions.md#enforcing-somewhere-else-call-preflight-not-evaluate).
- **Runbook:** [confined fleet](../operations/confined-fleet.md), where providers are reachable only through their
  gateway and enforce anyway.
- **Design:** [`authority-at-execution.md`](../design/authority-at-execution.md).
- **Code:** [`src/agent/provider_enforcement.rs`](../../src/agent/provider_enforcement.rs) (`check`).
