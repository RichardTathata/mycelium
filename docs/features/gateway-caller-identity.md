# Gateway caller identity

## What it is

On every gateway dispatch the provider sees the [gateway caller](../guide/00-concepts.md#gateway-caller): the client
behind the call, qualified by the authority that issued its credential (`oidc:{issuer}/{subject}`,
`token:{issuer}/{name}`), the gateway acting for it, and the scopes granted for this request — never the gateway
node itself. Under `tls` the gateway signs that context with its identity key over the request's payload, and the
provider verifies it against the keys it knows for that gateway; a signed context is admitted once and refused when
stale.

## When to use it, and when not

- **Use it when** a provider authorises callers by who they are (an allow-list, an evaluator rule), and those callers
  arrive through a gateway: listing the gateway node would admit every client behind it.
- **Not when** the mesh has no `tls` identity: the context is then exactly as strong as the frame's sender, honoured
  but not signed ([rbac § 7](../operations/rbac.md#7-gateway-caller-identity--who-a-provider-sees-behind-a-gateway-call)).

## Turn it on

It is on by default in a build with `gateway`; it is signed in a build with `tls` and a `[tls]` identity.

- [`gateway_caller_profile`](../reference/configuration.md#gateway_caller_profile): `secure` (the default) carries a
  caller context on every dispatch and refuses, naming the provider, one that does not enforce it; `legacy`
  dispatches as the node, for a rolling upgrade only, and warns at start.
- [`gateway_identity_issuer`](../reference/configuration.md#gateway_identity_issuer): the issuer that qualifies this
  gateway's token principals; without it, the gateway's node id, so two gateways never mint the same identity by
  accident.
- [`tls`](../reference/configuration.md#tls): the identity the context is signed with.
- In a provider, [`request_principal`](../../src/agent/gateway_caller.rs) reads the caller and
  [`request_authorized`](../../src/agent/gateway_caller.rs) (`compliance`) checks it against an allow-list.
- `forged_client_context_is_ignored_and_the_node_is_never_the_principal`, in
  [`src/agent/http/gateway_caller_tests.rs`](../../src/agent/http/gateway_caller_tests.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it on
its own, and [`auditor_questions`](../../examples/README.md#the-capability-matrix) asks "who actually did this?" as
one of three questions. The end-to-end run is a test in
[`src/agent/http/gateway_caller_tests.rs`](../../src/agent/http/gateway_caller_tests.rs):
`authorized_callers_judges_the_client_not_the_gateway_node`.

## What it guarantees, and what it does not

- **Guarantee:** [`gw.caller_profile`](../reference/guarantee-catalogue.md#gw.caller_profile) — a provider sees the
  gateway's client as the caller, attested by the gateway's identity.
- **Not built:** the gateway attests who its client is; it does not authenticate the client beyond the credential it
  presented, and the presented mandate rides beside the context, carried, not verified, until the provider verifies
  it ([rbac § 7](../operations/rbac.md#7-gateway-caller-identity--who-a-provider-sees-behind-a-gateway-call)).
- **Evidence:** the ledger row
  [a gateway caller envelope is served once, at its provider](../operations/what-is-proven.md#gateway-caller-envelope-served-once-provider).

## Go deeper

- **Guide:** [20, authorising actions](../guide/20-authorising-actions.md), where the principal is what a rule names.
- **Runbook:** [rbac § 7, Gateway caller identity](../operations/rbac.md#7-gateway-caller-identity--who-a-provider-sees-behind-a-gateway-call).
- **Design:** item 7 of [`plans/v3-contracts-axis.md`](../plans/v3-contracts-axis.md) §6.4, D27.
- **Code:** [`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs) ([`GatewayCaller`](../../src/agent/gateway_caller.rs),
  `attest`, `verify`).
