# Provider enforcement

## What it is

With provider enforcement on, every protected call a node receives — `mcp.invoke`, `skill.invoke`, `llm.invoke` and
any kind in `protected_rpc_kinds` — runs the same action preflight a gateway runs, as the
[enforcement point](../guide/00-concepts.md#enforcement-point) `provider`, before any handler sees it. It applies
whichever door the call came through — a gateway, a member's direct call, or the SDK serve stream — at the five
receivers the node itself serves protected work through: `rpc_rx`, the MCP tool loop, the bridged MCP loop, the SDK
serve stream and the built-in `llm.invoke` provider that answers a prompt skill. **Not at every receiver there is:**
a hosted capability's kind, `bulk_serve` and an application's raw receiver are not decided (stated below). A presented mandate
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
- [`cancellable`](../../src/agent/action_evaluator.rs): on an allowance (`cancellable = true` in a
  [unit file's `[[rule]]`](../reference/unit-file.md)), the call it permits runs only with its gateway's `call:`
  mandate established here, registered so that a cancel stops it; this needs `compliance` as well.
- `test_c7_the_bypass_matrix_no_door_runs_revoked_work`, in [`src/lib_tests.rs`](../../src/lib_tests.rs): after a
  revocation, no door runs the work, counted by the handlers themselves.
- `a_revoked_mandate_runs_no_llm_backend`, in
  [`src/agent/llm_door_tests.rs`](../../src/agent/llm_door_tests.rs): the same question at the built-in
  `llm.invoke` provider, counted by the backend — in its own suite because the matrix is built without `llm`.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`authority_drain`](../../examples/README.md#the-capability-matrix): a node with provider enforcement on serves a
tool, the authority is revoked, and the node's own records give the time to the last admitted call and to the last
confirmed stop, checked against the class's bound. CI runs it.

## What it guarantees, and what it does not

- **Rule:** [`provider.enforcement`](../reference/rule-catalogue.md#providerenforcement) — a permit, or a refusal:
  the action preflight's (a deny, authority not established, evidence not recorded), `no_evaluator`, `caller_context_refused`, `malformed_call` (a tool call that is
  not JSON or names no tool, or an `llm.invoke` call that names no prompt skill), `resource_unnamed`, `resource_not_here`, `resource_not_served`, `at_capacity`, `call_mandate_not_established` (a
  cancellable rule permitted the call and its gateway's `call:` mandate is not established here, or its term is
  already revoked — refused under any rule: *authority not established*, with the stable cause in `data.cause`
  (`revoked`, `no_call_mandate`, `other_envelope`, …; the list is in §104), the sentence in `data.found`,
  `execution: none` on the record, the provider's own use of a grant the call presented given back (the gateway
  that claimed the grant keeps it spent, §104), and counted in
  `mycelium_provider_call_mandate_refusals_total{cause}`) or `mandate_lapsed_before_run` (the
  preflight permitted on an established mandate and the re-assessment that registers the work no longer finds it
  established — a revocation in between refuses the call rather than running it unregistered; it answers as
  *authority not established*, `data.detail` `mandate_lapsed_before_run`, and the evidence journal records that
  nothing ran, `execution: none`, beside the permit, as it does after an `at_capacity` refusal of a permit; under a
  seat claim that window can be a consensus round long). The catalogue's `mandate_lapsed_before_run` is the wire
  `detail` under the reason `authority_not_established`. With
  enforcement off no authority is checked; an attached cohort budget still verifies the caller context and admits by
  cohort.
- **Guarantee:** [`prov.enforcement`](../reference/guarantee-catalogue.md#prov.enforcement).
- **Not built:** the check-then-act window between the check and the effect is narrowed and stated, not eliminated
  ([what-is-proven § the check-then-act window](../operations/what-is-proven.md#check-then-act-window)): a revocation
  before the work is registered refuses it, one after is the sweep's to stop; a provider
  that does not turn enforcement on checks no authority, whatever the gateway did, and one that turns it on with no
  evaluator refuses every protected call (`no_evaluator`).
- **`llm.invoke` at the built-in provider** (since 2.41, [§105](../guide/deprecations.md#105-an-enforcing-provider-applies-its-rules-to-llminvoke-at-the-built-in-llm-door-2410-781s-review-f1);
  [ledger](../operations/what-is-proven.md#llm-door-not-checked-at-provider)). The call is decided on the prompt
  skill its **payload** names — operation `llm.invoke`, resource `prompt:{ns}/{name}@{provider}`, arguments
  `{"input", "context"}`, as the gateway's LLM doors decide it — never on the envelope's claim, so a member's own
  call is decided too. A refusal keeps the door's shape, `{"error": reason, "detail": sentence}`, with `code` and
  `data` beside them; the gateway's `/gateway/llm/call` answers it `403` (`429` for `at_capacity`) and
  `/gateway/llm/stream` as an error event. The backend call is raced against the call's authority: a lapse or a
  cancel drops it and answers `cancelled`, which the gateway answers `409` — a stop, not a refusal of authority.
  The provider's caller-context refusal at this door carries no `data` and is still `502` at the gateway.
- **Not stopped, at that door:** work a model endpoint has already begun when the backend's future is dropped (it
  may finish generating), and anything a custom `LlmBackend` runs on a task of its own.
- **Not built: three receivers no decision is made at** (#785's review, 2026-11-14), in the order they matter:
  1. **a hosted capability's kind, `cap.invoke/{ns}/{name}`** — served through `rpc_rx`, where the check runs and is
     inert because the kind is not protected: at an enforcing stem a member's direct `rpc_call` of it runs the
     component undecided, while `/mcp` decides the same component. **Open today; decided on 2026-11-14 to be
     protected at an enforcing provider, in its own PR**
     ([ledger](../operations/what-is-proven.md#hosted-capability-not-decided-at-provider));
  2. **`bulk_serve`** — its handler runs with no caller context and no check, even for a kind in
     `protected_rpc_kinds`; members only, and a bypass only where an application serves a protected kind over bulk
     ([ledger](../operations/what-is-proven.md#bulk-serve-runs-no-provider-check));
  3. **a raw receiver** (`mesh().signal_rx` + `RpcRequest::from`) — it can verify the caller and has no public way to
     run the check ([ledger](../operations/what-is-proven.md#raw-receiver-cannot-run-provider-check)).
- **Not built, at the `llm.invoke` door:** a check waiting on a seat claim there is not under the bound the other
  four doors keep (`MAX_DEFERRED_CHECKS`, 64): each call has a task of its own and waits in it.
- **Not built, at that door:** a class or locality rule does not permit there (neither fact travels to a
  provider); `mycelium-reason`'s router reads a provider's refusal as a failed attempt and answers `exhausted`
  (`502`) when every provider refuses; and the door has no row in the C7 matrix itself (the matrix runs on CI's
  `compliance,a2a` line, which builds no `llm`).
- **Not built, for a cancellable rule:** no gateway mints a `call:` mandate yet, so such a rule refuses every call
  it permits; a member's own unframed call carries none; the bridged MCP loop and the SDK serve stream do not stop
  a cancelled call; a revoked term under a rule that is not cancellable is best effort, since a relay that strips
  `cm` makes the call run; a broader allowance listed first makes a cancellable rule inert ([§104](../guide/deprecations.md#104-a-cancellable-rule-requires-the-calls-call-mandate-at-the-provider-cm-in-the-caller-envelope-2410-identity-and-lifecycle-i4-pr-1b)).
- **Not built:** on the deferred-check path (a check settling a seat claim), a `withdrawn` refusal (the serve loop
  ended with the check in flight) and the *authority not established* answer to a check whose task panicked can follow
  a permit, and leave its `execution: attempted` alone in the journal: neither has the permit where it is made.
- **Evidence:** the ledger rows [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing)
  and [running work stops](../operations/what-is-proven.md#running-work-stops).

## Go deeper

- **Guide:** [21 § Presenting and enforcing a mandate at the gateway and the provider](../guide/21-mandates.md#presenting-and-enforcing-a-mandate-at-the-gateway-and-the-provider-2150);
  [20 § Enforcing somewhere else](../guide/20-authorising-actions.md#enforcing-somewhere-else-call-preflight-not-evaluate).
- **Runbook:** [confined fleet](../operations/confined-fleet.md), where providers are reachable only through their
  gateway and enforce anyway.
- **Design:** [`authority-at-execution.md`](../design/authority-at-execution.md).
- **Code:** [`src/agent/provider_enforcement.rs`](../../src/agent/provider_enforcement.rs) (`check`); the built-in LLM
  door, [`src/agent/llm.rs`](../../src/agent/llm.rs) (`handle_llm_invoke`).
