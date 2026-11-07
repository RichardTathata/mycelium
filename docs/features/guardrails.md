# Guardrails

## What it is

`mycelium-guardrails` compiles one declared [`Policy`](../../mycelium-guardrails/src/policy.rs) onto a node: which
[groups](../guide/00-concepts.md#group) it acts within, which tools and budgets gate its agent's state transitions, and which callers may invoke the
capabilities it serves. Each clause is labelled with the strength it actually has. Only `authorized_callers` is hard
prevention: an off-list caller is refused at the provider and, under `compliance`, the denial is sealed into the
audit chain with the verified caller. Group and transition clauses are self-imposed by an honest node. There is no
remote authority: a policy configures the node that applies it, and no path sets another node's policy.

## When to use it, and when not

- **Use it when** an agent's own node should constrain what it does, and a provider must refuse callers outside a
  list and be able to prove it stopped them.
- **Not when** you need content moderation, or a guarantee against a malicious node: Tiers A and B are self-imposed,
  and the chain is per node ([guide 16 § Honest limits](../guide/16-guardrails.md#honest-limits-state-them-dont-gloss-them)).

## Turn it on

This feature is turned on in code, from the `mycelium-guardrails` crate. Its features: `compliance` for the Tier C
gate and the sealed denial (it forwards `mycelium/compliance`, which needs `tls`); `gateway` for the HTTP edge;
`metrics-export` for Prometheus metrics on it.

- [`Policy`](../../mycelium-guardrails/src/policy.rs): the declaration — `act_within_groups`, `allow_tools`,
  `tool_budget`, `require_grant_for`, `authorized_callers` and the rest; `strength_report` lists each active
  clause's strength.
- [`apply`](../../mycelium-guardrails/src/apply.rs): compiles the policy onto this node and returns the applied
  policy.
- [`guarded_rpc_serve`](../../mycelium-guardrails/src/guard.rs): serves a capability behind the caller check.
- [`prove_denials`](../../mycelium-guardrails/src/verify.rs): reads the sealed denials back and verifies them.
- `tier_c_denies_unauthorized_caller_and_seals_the_denial`, in
  [`mycelium-guardrails/tests/guardrails.rs`](../../mycelium-guardrails/tests/guardrails.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`guardrail_fleet`](../../examples/README.md#the-capability-matrix) and
[`guardrail_wedge`](../../examples/README.md#the-capability-matrix), both run in CI by
`mycelium-guardrails/ci_smoke.sh`; [`guardrail_viz`](../../examples/README.md#the-capability-matrix) is the browser showcase.

## What it guarantees, and what it does not

- No guarantee or rule id: the companion registers none in the
  [guarantee catalogue](../reference/guarantee-catalogue.md); a sealed denial is an
  [`audit.chain`](../reference/guarantee-catalogue.md#audit.chain) record, and its strength is the clause's own
  report.
- **Not built:** Tiers A and B do not bind a node that ignores its own boundary; the proof is provable stopping per
  provider, not "could not have acted anywhere"; the failure model is crash-fault, not Byzantine
  ([guide 16 § Honest limits](../guide/16-guardrails.md#honest-limits-state-them-dont-gloss-them)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger's
  [companion-suite row](../operations/what-is-proven.md#companion-crate-s-suite-both-sdks).

## Go deeper

- **Guide:** [16, guardrails](../guide/16-guardrails.md).
- **Runbook:** [audit § 8, Proving a guardrail stopped an agent](../operations/audit.md#8-proving-a-guardrail-stopped-an-agent);
  [companions § mycelium-guardrails](../operations/companions.md#mycelium-guardrails--the-structural-gate).
- **Design:** [`plans/mycelium-guardrails.md`](../plans/mycelium-guardrails.md).
- **Code:** [`mycelium-guardrails/src/`](../../mycelium-guardrails/src/lib.rs); the examples are
  [`mycelium-guardrails/examples/`](../../mycelium-guardrails/examples/README.md).
