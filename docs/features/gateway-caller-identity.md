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
  caller context on every dispatch and sends only to a provider whose promise to enforce it — its
  `sys/caller-context/{node}` marker — the gateway holds; `legacy` dispatches as the node, for a rolling upgrade only,
  and warns at start.
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
- **The gateway sends only to a provider whose promise it has seen** (#802, [§115](../guide/deprecations.md)). The
  provider promises what it enforces by publishing its marker before it advertises work; the gateway judges only what
  it holds. Gossip can deliver a provider's `tools/` or `cap/` entry before its marker, so until the marker arrives
  that provider is not one to send to: every door that chooses a provider (`/mcp`, the grant door, `/a2a`, the LLM
  doors, the reason router's client routes, through `gateway_sends_to`) passes it over, and with none left answers its
  own "no provider" (`-32601 tool not found`, `skill not found`, `404 no_provider`), with `data.reason`
  `provider_not_promised` and the count passed over where the answer has a data field — the gateway promises only its
  answer; whether a client retries it is the client's (neither SDK does today);
  `tools/list` and the A2A agent card leave out what no such provider offers, so they agree with the calls. A target
  the client names (`/gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable`, `rpc_call_as`) whose
  marker the gateway does not hold is refused `-32021` / HTTP `412 provider_without_caller_context`, naming it — not
  yet seen, or never (a provider from before the marker). No marker value says "does not enforce". Each node
  re-asserts its own marker every 60 s, so a member's tombstone of it hides the provider for at most one period (one
  stamped beyond `max_clock_drift_ms` ahead hides it until the wall clock catches up — detected as `kv_future_stamps`
  and by the self-owned tripwire, not prevented), and
  every choosing door counts the providers it passed over (`/stats` `providers_passed_over_unpromised`, by door).
  `/gateway/caps/resolve` lists providers whatever their marker, so a client that resolves there and names the target
  can still meet a transient `-32021`; a client that lists tools once during the gap can miss one. Tests:
  [`marker_race_tests.rs`](../../src/agent/http/marker_race_tests.rs).
- **Not built:** a provider that never publishes its marker is invisible at a choosing door, not named: its
  advertisement is held and its calls answered "no provider" (§115 says how an operator finds it).
- **Not built:** the gateway attests who its client is; it does not authenticate the client beyond the credential it
  presented, and the presented mandate rides beside the context, carried, not verified, until the provider verifies
  it ([rbac § 7](../operations/rbac.md#7-gateway-caller-identity--who-a-provider-sees-behind-a-gateway-call)).
- **Not built: the first call to a newly joined provider can be refused.** The provider verifies the envelope against
  the keys it trusts for the gateway, learned from its own connection to the gateway or the gateway's certificate
  record. Either can arrive after the gateway sees the provider's tool and caller-context marker, so the gateway's
  first call can be refused `-32022 unknown_signer` at the provider's receive boundary, before anything runs. At
  `/mcp` that refusal gives back a grant the call presented, so a retry under the same grant is safe
  (`a_provider_that_does_not_yet_trust_the_gateway_gives_the_grant_back`). Neither the retry nor an immediate fetch
  of the sender's certificate by the provider is built: the caller retries
  ([issue #753](https://github.com/RichardTathata/mycelium/issues/753)).
- **Evidence:** the ledger row
  [a gateway caller envelope is served once, at its provider](../operations/what-is-proven.md#gateway-caller-envelope-served-once-provider).

## Go deeper

- **Guide:** [20, authorising actions](../guide/20-authorising-actions.md), where the principal is what a rule names.
- **Runbook:** [rbac § 7, Gateway caller identity](../operations/rbac.md#7-gateway-caller-identity--who-a-provider-sees-behind-a-gateway-call).
- **Design:** item 7 of [`plans/v3-contracts-axis.md`](../plans/v3-contracts-axis.md) §6.4, D27.
- **Code:** [`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs) ([`GatewayCaller`](../../src/agent/gateway_caller.rs),
  `attest`, `verify`).
