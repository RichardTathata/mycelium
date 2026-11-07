# A2A interoperability

## What it is

In a build with `a2a`, the gateway serves the Agent-to-Agent protocol: an
[AgentCard](../guide/00-concepts.md#a2a-agent-card) at `/.well-known/agent-card.json`, built live from the fleet's
advertised capabilities, and JSON-RPC task dispatch at `/a2a` in the 1.0 and 0.3 shapes. An external agent framework
discovers the fleet's skills from the card and calls them as A2A tasks, unmodified. Every send runs the same doors —
the federation edge, the action evaluator, the presented mandate, the evidence record — and a task answers only the
identity that created it.

## When to use it, and when not

- **Use it when** an agent built on an A2A client (Strands Agents' `a2a-sdk`, LangChain over A2A) should reach the
  fleet's skills without a Mycelium SDK, from a node you build or `skillrunner`.
- **Not when** the route must be closed by a bearer: `/a2a` admits an anonymous caller and drops a bearer's scopes,
  so it is gated by an evaluator that denies anonymous principals, not by a token.

## Turn it on

This feature is turned on in code: build with `a2a` (it implies `gateway`), give the node an `http_port`, and mount
the routes. The published `mycelium` binary is built with `a2a` but does not mount them (below); the `skillrunner`
binary does, when its node has an `http_port`.

- [`with_a2a`](../../src/agent/mod.rs): mounts the card and `/a2a` on the gateway; without it nothing is served.
- [`http_port`](../reference/configuration.md#http_port): the gateway that serves them.
- [`with_a2a_default_skill`](../../src/agent/mod.rs): the skill a message that names none reaches; without one such a
  message is refused `-32602`.
- [`with_action_evaluator`](../../src/agent/mod.rs): what decides each send; deny anonymous principals here to close
  the route ([authorising actions](action-authorisation.md)).
- `a2a_is_anonymous_dispatch_whatever_bearer_is_configured`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`a2a_skill_authority`](../../examples/README.md#the-capability-matrix): an evaluator deciding which skills an A2A
caller may run, at the door that matters most because a bearer does not gate it. CI runs it, and CI's `a2a-sdk` job
drives the published client library against an example node built with `a2a`. [`a2a_langchain`](../../examples/README.md#the-capability-matrix)
is a manual run with a real model.

## What it guarantees, and what it does not

- **Rule:** [`a2a.admission`](../reference/rule-catalogue.md#a2aadmission) — an absent credential is anonymous and a
  present bearer's scopes are dropped; the action preflight decides the rest.
- **Guarantee:** [`a2a.admission`](../reference/guarantee-catalogue.md#a2a.admission) — `/a2a` is not anonymous skill
  dispatch, which holds only with an evaluator that denies anonymous principals.
- **Not built:** the published `mycelium` binary does not mount `/a2a` — `src/main.rs` never calls `with_a2a` — so
  an A2A client needs a node built with it or `skillrunner`; the card is public on a credentialed node; the pre-0.2
  methods and the old card path, announced in 2.35.0 for one release
  ([deprecations §46](../guide/deprecations.md#46-a-current-a2a-client-reaches-a2a-the-card-at-agent-cardjson-the-10-and-03-methods-a-skill-in-metadataskillid-2350-post-360-plan-row-n)),
  are still served; push notifications are not served.
- **Evidence:** the ledger row
  [an unmodified A2A client reaches a node's skills](../operations/what-is-proven.md#unmodified-a2a-client-reaches-node-s).

## Go deeper

- **Guide:** [08, A2A interop](../guide/08-a2a-interop.md);
  [20 § The other door](../guide/20-authorising-actions.md#the-other-door-and-why-it-is-the-one-that-matters).
- **Runbook:** [production readiness](../operations/production-readiness.md), where `/a2a`'s exposure is stated.
- **Design:** no design record; row N of the [post-360 plan](../plans/post-360-hardening.md) is the record of the
  current protocol versions.
- **Code:** [`src/agent/a2a.rs`](../../src/agent/a2a.rs) (the card, the methods, `dispatch_send`).
