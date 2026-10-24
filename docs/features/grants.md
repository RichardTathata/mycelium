# Grants

## What it is

A **grant** is approval that is default-deny: for a tool the agent's policy lists in `require_grant_for`, the agent's
`Invoking` transition waits for a grant and is refused on silence — the [grant](../guide/00-concepts.md#grant) entry.
A grant is a [scoped mandate](../guide/00-concepts.md#mandate) a supervisor issues for one call, bound to the tool,
the arguments' digest, the requesting agent's identity and the policy revision it was approved under, with an expiry.
It is verified where the action is enforced, not where a button is pressed: the agent's state machine checks it before
`Invoking`, the gateway's preflight checks it again, and a provider with `with_provider_enforcement()` checks it a
third time. The veto mode (`require_approval_for`) stays what it is and says so: *veto, not grant* — an absent
supervisor approves there.

## When to use it, and when not

- **Use it when** a tool's effect is one a human or a supervising agent must approve *per call* — a payment, a
  deployment — and a missing supervisor must mean the call does not run.
- **Not when** you need a fast path that tolerates an absent supervisor (that is the veto mode, labelled as such in
  `strength_report()`), or when the node attaches no execution authority and no identity: a grant is verified by the
  node's authority with a possession proof signed by its identity, so both are needed, and the gateway issues a grant
  only where an evaluator decides who may.

## Turn it on

This feature is turned on in code.

- [`AgentPolicy`](../../src/agent/state_machine.rs) — its `require_grant_for` lists the tools, its `grant_window` how
  long the agent waits (default 30 s, read on the wall clock).
- [`AgentStateMachine::transition_invoking`](../../src/agent/state_machine.rs) — the agent's door: writes the request
  under `grant/{agent}/{request}` (the tool, the arguments' digest and a bounded rendering, the policy revision, the
  expiry), waits for `…/issued`, verifies the grant with the node's [`ExecutionAuthority`](../../src/agent/gateway_authority.rs)
  and returns it for the call's `_meta.mandate`. A plain `transition` to a grant-required tool is refused: it carries
  no arguments to bind.
- [`gw_grants_pending`](../../src/agent/http/grants.rs) and [`gw_grant_decide`](../../src/agent/http/grants.rs) — the
  supervisor's door: `GET /gateway/grants/pending` (`grant:read`), `POST /gateway/grants/{agent}/{request}`
  (`grant:issue`) with `{"decision": "issue"|"refuse", "ttl_secs", "reason"}`. Issuing is an action the evaluator
  sees (`grant.issue` on `tool:{tool}@{agent node}`, the issuer as the actor): a client holding `grant:issue` is not
  thereby an approver of everything, and a gateway with no evaluator issues nothing.
- [`GRANT_SCOPE_PREFIX`](../../src/mandate/grant.rs), [`grant_operations`](../../src/mandate/grant.rs) and
  [`entitle_grants`](../../src/mandate/grant.rs) — the grant as a mandate: scope `grant:{request}`, its binding inside
  its signed operations, the issuing node entitled under the prefix.
- [`requiring_grant`](../../src/agent/action_evaluator.rs) — a policy rule that applies only under a grant.
- `a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock`, in
  [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs): the matching digest established, another
  refused by name, a revoked term not established, the window read on the clock passed in.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests: in [`src/mandate/grant.rs`](../../src/mandate/grant.rs),
`a_grant_binds_in_its_signed_operations_and_two_grants_never_conflict`; in
[`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs),
`a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock`; in
[`src/agent/http/grant_tests.rs`](../../src/agent/http/grant_tests.rs), the loop through the doors —
`silence_refuses_and_a_plain_transition_to_a_grant_tool_is_refused`,
`an_issued_grant_lets_the_call_run_and_binds_it_to_its_arguments` and
`issuing_is_an_action_the_evaluator_decides_and_the_namespace_is_route_owned`.

## What it guarantees, and what it does not

- Silence refuses: a grant-required tool is never invoked without a grant that verified at the agent's node; the
  refusal is `GrantNotPresented`, a supervisor's refusal `GrantRefused`.
- A grant binds one call: another argument digest is refused by name (`BoundToOtherArguments`) at the agent, the
  gateway and the provider; a decision under another policy revision is refused (`BoundToOtherPolicy`); a grant past
  its window is out of window on the wall clock, however still the node's HLC stayed.
- A grant is one appointment in its own scope, so grants never conflict with each other or with the resource
  authority's appointments. Its revocation standing must be *known* to be established, so the issuing node publishes
  the grant's first checkpoint (nothing revoked) to its own authority at issue and writes it beside the grant
  (`…/checkpoint`); it is revoked as any mandate is, by the next checkpoint naming its term, and is then not
  established at every verifier that has it.
- The record of the decision carries the grant's hash (`mandate_digest` on the evidence record).
- The pending request is a KV record under a route-owned namespace (`grant/`): the raw KV routes refuse it, a
  supervisor on another node sees it by gossip, and an agent that asks again for the same tool and digest while the
  request stands waits on it rather than asking twice.
- **Veto, not grant:** `require_approval_for` approves on silence and is labelled so in `strength_report()`; its
  `ApprovalTimedOut` variant was never produced and is deprecated (`deprecations.md` §59).
- **Not built, by the plan row** ([`adopter-surface.md`](../plans/adopter-surface.md) X3): the SDKs' grant verbs and an
  SSE stream of new requests; the agent restarted while waiting re-reading its request (the record is durable in a
  persisted KV; the re-wait after a restart is not shown by a test); revocation reaching the agent's node (a checkpoint
  is offered to the node that holds the authority; carrying it across nodes is the adopter's today, as for every
  mandate); the A2A `input-required` state for a task waiting on a grant; the data-class policy over the request's
  argument rendering (rendered when under 16 KiB, else summarised with its size); the bypass matrix row and the
  `authority_drain` bound for a revoked grant stopping admitted work; a `prove_denials` reconstruction. An operator
  UI is the adopter's.

## Go deeper

- Guide: [mandates — a grant is a mandate for one call](../guide/21-mandates.md#a-grant-is-a-mandate-for-one-call).
- Operations: [RBAC](../operations/rbac.md) for `grant:read` / `grant:issue`.
- Design: [scoped mandates](../design/scoped-mandates.md); the plan row is the record for the grant itself.
- Code: [`src/agent/state_machine.rs`](../../src/agent/state_machine.rs), [`src/agent/http/grants.rs`](../../src/agent/http/grants.rs),
  [`src/mandate/grant.rs`](../../src/mandate/grant.rs), [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs).
