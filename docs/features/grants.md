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
- [`gw_grants_pending`](../../src/agent/http/grants.rs), [`gw_grant_decide`](../../src/agent/http/grants.rs) and
  [`gw_grant_revoke`](../../src/agent/http/grants.rs) — the supervisor's door: `GET /gateway/grants/pending`
  (`grant:read`), `POST /gateway/grants/{agent}/{request}` (`grant:issue`) with `{"decision": "issue"|"refuse",
  "ttl_secs", "reason"}`, and `POST /gateway/grants/{agent}/{request}/revoke` (`grant:issue`) with `{"reason"}` at the
  node that issued. Issuing, refusing and revoking are actions the evaluator sees (`grant.issue` · `grant.refuse` ·
  `grant.revoke` on `tool:{tool}@{agent node}`, the caller as the actor): a client holding `grant:issue` is not thereby
  an approver of everything, and a gateway with no evaluator answers nothing. A grant is issued for an agent on this
  node or on a fleet member this node holds the key of; the agent's node decides whether that issuer may
  ([`entitle_grants_from_fleet`](../../src/mandate/grant.rs)). [`gw_grants_stream`](../../src/agent/http/grants.rs) —
  `GET /gateway/grants/stream` (`grant:read`): the open requests as they come, `grant_request` once each (the standing
  ones first, as a backlog) and `grant_answered` when one this stream reported is issued, refused or revoked. The SDKs'
  verbs are `grants_pending` / `grant_decide` / `grant_revoke` and `grants_stream` (`grantsPending` / `grantDecide` /
  `grantRevoke` / `grantsStream`).
- [`AgentPolicy`](../../src/agent/state_machine.rs) — its `grant_arguments_class` and `grant_renderable_classes`: the
  rendering under the data-class policy. The arguments' class by name, in the operator's vocabulary (as row R's `DataClass` names it), and
  the classes whose arguments may be rendered into the gossiped request; a class not listed is **withheld** — the record
  says so, with the class and the size, never as empty — while the grant still binds the digest
  ([`render_arguments_under`](../../src/agent/state_machine.rs)). The default list is empty, so a classified argument
  set is withheld unless its class is listed; unclassified arguments are rendered under the 16 KiB cap as before.
- The A2A door: a streamed task whose serving agent is waiting for a grant reports **`input-required`** with the
  pending request's id in the status message and nothing else, then `working` once it is answered — the
  `InputRequired` step of [`Step`](../../src/agent/a2a.rs) (the design question, decided: yes).
- [`claim_grant_use`](../../src/agent/gateway_authority.rs) and [`feed_grant_checkpoint`](../../src/agent/gateway_authority.rs)
  — a grant admits **one call at each door** (the gateway's and the provider's preflight each claim their use, the
  marker `…/used/{door}` on the WAL), and a grant's checkpoints travel in the KV beside it, fed to every node's
  authority — at issue (standing known) and at revocation (the sweep stops work running under it).
- [`GRANT_SCOPE_PREFIX`](../../src/mandate/grant.rs), [`grant_operations`](../../src/mandate/grant.rs) and
  [`entitle_grants`](../../src/mandate/grant.rs) — the grant as a mandate: scope `grant:{request}`, its binding inside
  its signed operations, the issuing node entitled under the prefix.
- [`requiring_grant`](../../src/agent/action_evaluator.rs) — a policy rule that applies only under a grant.
- `a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock`, in
  [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs): the matching digest established, another
  refused by name, a revoked term not established, the window read on the clock passed in.
- `a_revoked_grant_is_refused_at_every_door_and_stops_admitted_work`, in
  [`src/agent/http/grant_tests.rs`](../../src/agent/http/grant_tests.rs): a slow call under a grant stopped by the
  revocation within the drain bound, a second grant revoked before use refused at the doors, a bystander's revocation
  refused by policy.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests: in [`src/mandate/grant.rs`](../../src/mandate/grant.rs),
`a_grant_binds_in_its_signed_operations_and_two_grants_never_conflict`; in
[`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs),
`a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock`; in
[`src/agent/http/grant_tests.rs`](../../src/agent/http/grant_tests.rs), the loop through the doors —
`silence_refuses_and_a_plain_transition_to_a_grant_tool_is_refused`,
`an_issued_grant_lets_the_call_run_and_binds_it_to_its_arguments`,
`issuing_is_an_action_the_evaluator_decides_and_the_namespace_is_route_owned`,
`a_grant_is_bound_to_its_policy_revision_and_consumed_by_its_commit`, and the second PR's —
`a_grant_admits_one_call_at_each_door`, `a_revoked_grant_is_refused_at_every_door_and_stops_admitted_work`,
`a_grant_is_issued_at_one_node_for_an_agent_on_another_and_revoked_there` and
`a_restarted_agent_re_waits_on_its_pending_request_and_an_expired_one_asks_again`; the third PR's —
`the_grants_stream_reports_a_request_once_and_then_its_answer`,
`a_streamed_a2a_task_waiting_for_a_grant_reports_input_required_with_the_request_id` and
`a_request_withholds_arguments_of_a_class_the_policy_does_not_render`.

## What it guarantees, and what it does not

- Silence refuses: a grant-required tool is never invoked without a grant that verified at the agent's node; the
  refusal is `GrantNotPresented`, a supervisor's refusal `GrantRefused`.
- A grant binds one call: another argument digest is refused by name (`BoundToOtherArguments`) at the agent, the
  gateway and the provider; a decision under another policy revision is refused (`BoundToOtherPolicy`); a grant past
  its window is out of window on the wall clock, however still the node's HLC stayed.
- A grant is one appointment in its own scope, so grants never conflict with each other or with the resource
  authority's appointments. Its standing is its issuer's checkpoint for the grant's scope, not the gate's freshness
  policy: the issuing node publishes the grant's first checkpoint (nothing revoked) at issue and writes it beside the
  grant (`…/checkpoint`); a grant whose issuer's checkpoint has not reached a verifier is not established there; it is
  revoked by the next checkpoint naming its term, and revocation reaches a verifier when that checkpoint does. Between
  checkpoints a grant stands for its window — that is why the window is short (default 300 s, at most 3 600 s).
- **One grant, one commit at the agent, one call at each door.** The request a grant answered is marked consumed by
  the commit it admitted, so the same tool and arguments ask again; and each enforcement point — the gateway's
  preflight, the provider's — admits a grant **once**: the authority's claim is atomic across concurrent presentations,
  the marker under the request (`…/used/{door}`) is on the WAL, and a second presentation is **denied** `GrantUsed`
  naming the door. A grant admitted at a door is spent there whether or not the work completes, as a nonce is; the claim is made once
  per call at settling — after the permit's history and seats, before its record — so a refusal earlier in that
  settling (a full fleet counter) spends nothing, and a retry of the history view cannot meet its own marker.
- **Revocation.** `POST …/revoke` at the issuing node reads the tool and the agent from the **signed** grant (its one
  enumerated operation and its holder), not from the request record, and publishes the next checkpoint for the grant's scope naming the
  term, offers it to its own authority — whose sweep judges work under a grant by that standing and its window, not by
  the resource gate, and stops it within the drain bound (sweep interval plus the handler's own stop) — and writes it
  beside the grant, where every node's authority is fed it; a revoked grant is refused at every door as
  *not established — revoked*, the AE vocabulary for revocation. Revoking is an action the evaluator decides
  (`grant.revoke`); a node that did not issue the grant answers `409 not_the_issuer`.
- **Across nodes.** A supervisor at any gateway issues for an agent on a member node whose key the gateway holds; the
  grant and its checkpoint travel in the KV, the agent's node feeds its authority the checkpoint before it assesses —
  and waits, not refuses, while the grant has landed and its checkpoint has not — and its entitlement table decides
  whether a member may issue. **The trust this asks for:** `entitle_grants_from_fleet` entitles **every** member whose
  key this node holds, so the issuing member's evaluator alone decides who may grant here — a member with a permissive
  evaluator or a rogue token holder can issue for any agent in the fleet; the narrower tool is `entitle_grants` per
  trusted gateway node, and a node that entitles only itself refuses the rest as *not established*. A principal no
  member's is `409 not_a_member`. Member keys need the `compliance` feature (`gateway_member_keys` is empty without it),
  as the grant mode has since the first PR. A grant's checkpoint is exempt from the revocation view's start rule — a
  grant reads no freshness, its window bounds its life — so a grant issued before a node's restart is held after it.
- **A restart re-waits.** The request and the grant are on the WAL at the **buffered** rung (the state machine's
  writes are fire-and-forget appends; the test's "crash" is a graceful restart); an agent restarted mid-wait that asks
  for the same tool and arguments finds the one request, does not ask twice, and the grant issued against it before
  the restart is honoured once after it; an expired request is not re-waited — a new one is written and silence
  refuses it.
- **Reconstructable:** a used or mis-bound grant's refusal is a **denial**, sealed into the audit chain with the
  operation it refused; `prove_denials` (mycelium-guardrails) reads exactly those records, and the evidence record the
  reference cites carries the reason. A *revoked* grant's refusal is sealed as *not established* (an error outcome),
  which `prove_denials` does not list — stated. **Found, not closed:** an indeterminate decision's record names the grant
  (its hash — an unknown standing carries the mandate's facts since this PR, as a refusal did) and the rule it could
  not satisfy, but not the standing's words (*the grant is revoked*), which reach the caller in the door's answer and
  the decision's missing facts only; the record alone does not say *why* not established.
- Refusing is an action the evaluator decides too (`grant.refuse`), recorded like issuing; the record of either says
  what the write did.
- The agent's own guards (budget, turn, allow and deny lists) run before it asks a supervisor; a tool in both modes
  passes the grant, then the veto window.
- The record of the decision carries the grant's hash (`mandate_digest` on the evidence record).
- The pending request is a KV record under a route-owned namespace (`grant/`), appended to the node's WAL where the
  node persists: the raw KV routes refuse it, a supervisor on another node sees it by gossip, and an agent that asks
  again for the same tool and digest while the request stands and is unconsumed waits on it rather than asking twice.
- **Veto, not grant:** `require_approval_for` approves on silence and is labelled so in `strength_report()`; its
  `ApprovalTimedOut` variant was never produced and is deprecated (`deprecations.md` §59).
- **The `grant` fault class** ([`run`](../../src/faults/grant.rs); `mycelium test --faults grant <units-dir> --config
  <node.toml>`, one of the [fault classes](../guide/00-concepts.md#test-faults)) — adopter-surface X3's exit on the
  declaration's own fleet, and the one-hour job's step 4. It exercises
  [`persist.configured`](../reference/guarantee-catalogue.md) and [`mesh.tls`](../reference/guarantee-catalogue.md) (the
  grant replayed from the WAL, the identity that signs it) and the decision point
  [`ae.preflight`](../reference/rule-catalogue.md) (who may issue). The first
  `[[rule]]` with `requires_grant = true` ([unit file](../reference/unit-file.md#rule)) is read as a **selector**: its
  resource names the tool the agent's policy requires a grant for (the default-deny the class shows is the state
  machine's own check, `require_grant_for`), its actor the agent's unit; the actor of a rule admitting `grant.issue` on
  it (`*` for either, a `@<node>` suffix not matched — one reading shared with `wire-check`,
  [`admits_grant_issue_for`](../../src/capability_config.rs)) is the issuer the class plays. Because only the agent's
  node can hold a grant, the class also admits the rule with the node's principal as actor and the resource at
  `tool:<name>@<node>`, the forms the request record and the doors use; a `*` issuer admits the stranger too, so that
  half of the check is stated, not run. On the agent's node — an evaluator
  built from every declared rule, a journal, an execution authority entitled to its own grants — a plain transition is
  refused and silence refuses within the window; a principal no rule names is refused the issue by the evaluator and
  the declared issuer's grant admits the waiting call once; the node is stopped before it reads an answer, the issuer
  answers the standing request, and after the restart the request and the grant are back from the WAL, the agent
  re-waits and does not ask twice, and the grant issued before the restart verifies; other arguments ask again; the
  decisions are on the record. It needs `[tls]`, `[persistence]` and a `compliance` build, and reads `not_applicable`
  naming the rule it lacks. It issues through the route's own decision ([`decide_grant`](../../src/agent/http/grants.rs)),
  not over HTTP — the fleet has no gateway port — and admits a declared `tool:<name>` also at `tool:<name>@<the agent's
  node>`, the form the request record and the doors use. Both scaffolds declare the tool and its issuer
  ([`purchaser.toml`](../../templates/governed-call-py/units/purchaser.toml)); `wire-check` warns `unissuable grant`
  for a grant-required rule nobody may issue. Tests: `the_grant_class_refuses_on_silence_issues_by_the_declared_rule_and_holds_across_a_restart`,
  `the_grant_class_needs_a_grant_rule_tls_and_persistence_and_says_so` ([`src/faults.rs`](../../src/faults.rs)).
- **The stream** reports each request once, each answer once (an issue followed by a revocation is two), and an
  expiry once (`grant_expired`, for a reported request that lapsed unanswered), by anyone at any node (the records
  gossip); it is woken by writes under `grant/` and by the soonest reported expiry, a late supervisor gets the
  standing requests as a backlog, and its task ends when the client goes. **`input-required`** is reported on the
  streaming send only (`tasks/sendSubscribe`, `message/stream`, `SendStreamingMessage`; A2A 1.0 names it
  `TASK_STATE_INPUT_REQUIRED`): a unary send answers at the end, and `tasks/get` on a waiting task says `submitted`.
  **The bound an A2A task can wait:** the streamed dispatch times out at 120 s (the unary path's bound), so a
  supervisor who answers later answers a task already reported `failed` — a `grant_window` longer than that is not
  honoured through this door. The watcher reads the serving node's requests (`grant/{node}/`, each new unexpired one
  reported once, `working` again when none it reported still stands), because the request record carries no task id:
  a task served by a node whose agent asks for a grant for another reason reports that too — the status is a request
  id, not an attribution. **The class is the policy's**, one name per agent policy, not row R's per-call `DataClass`
  (declared or principal-sourced): a first cut, stated. **Not built, by the plan row** ([`adopter-surface.md`](../plans/adopter-surface.md) X3, after the fourth
  PR): retention of `grant/` records; a per-issuer entitlement finer than `entitle_grants_from_fleet`; a per-call class
  for the rendering; the gateway and provider doors driven on a declaration's fleet (the `grant` class drives the state
  machine's door and the issuing door — the door tests drive the rest). **The doors a node-held grant reaches:** a grant's holder is the agent's node, so
  it is presented on the node's own call (a member's direct call, the provider's door) — a bearer client presenting a
  node's grant at `/mcp` is refused as not the holder's. **The gateway's claim (`used/gateway`) is code with no
  reachable caller today:** only a `node:` principal can hold a grant, so no client presents one at `/mcp`; the claim
  runs in the one preflight both doors share and is exercised at the provider's tag. Grant records under `grant/` are
  never reclaimed (no retention row yet). An operator UI is the adopter's.
- Metrics: `mycelium_grants_requested_total`, `_verified_total`, `_expired_total`, `_issued_total`, `_refused_total`,
  `_revoked_total` and the gauge `mycelium_grants_pending` (moved on every request and answer, not only when polled).

## Go deeper

- Guide: [mandates — a grant is a mandate for one call](../guide/21-mandates.md#a-grant-is-a-mandate-for-one-call).
- Operations: [RBAC](../operations/rbac.md) for `grant:read` / `grant:issue`.
- Design: [scoped mandates](../design/scoped-mandates.md); the plan row is the record for the grant itself.
- Code: [`src/agent/state_machine.rs`](../../src/agent/state_machine.rs), [`src/agent/http/grants.rs`](../../src/agent/http/grants.rs),
  [`src/mandate/grant.rs`](../../src/mandate/grant.rs), [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs).
