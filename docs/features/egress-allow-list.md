# Egress allow-list

## What it is

The [egress allow-list](../guide/00-concepts.md#egress-allow-list) is a list of hostnames each node enforces on the
outbound HTTP calls the substrate itself makes: the MCP bridge, LLM backends, capability probes, the skill runner,
the wasm host, the wiki sink, the federation client and OIDC. A non-empty list is fail-closed. It is checked on the
first URL and on every redirect hop (at most five, never https to http), with the host read by the client's own
parser, and an object store is checked on the endpoint it dials, not its bucket.

## When to use it, and when not

- **Use it when** a node must not be steered into calling hosts you did not choose, through a configured URL, a
  redirect or a tool's endpoint.
- **Not when** you need address-level control: it does not resolve names, so an allowed name that resolves to a
  denied address is outside it, and a client the substrate does not make (an SDK's A2A client) is not covered.

## Turn it on

Set the list in the configuration; an empty list (the default) allows all. The two-domain scaffold's
[`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's two-domains job starts, sets
`[egress] allow_hosts` to the partner's gateway host.

- [`egress`](../reference/configuration.md#egress): `allow_hosts`, each an exact host or a `.suffix` that matches the
  host and any subdomain, case-insensitive.
- A companion type built outside an agent is gated only when built with the policy: `HttpLibrarySource` and
  `OllamaProbe` take `.with_egress(policy)`, and so does `FederationClient`
  ([crown jewel § Coverage](../operations/crown-jewel.md#coverage)).
- [`redirect_policy`](../../src/agent/egress_client.rs): the redirect check every gated client uses.
- `a_redirect_is_checked_like_a_destination`, in [`mycelium-core/src/config.rs`](../../mycelium-core/src/config.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it. The
end-to-end run is a test in [`src/agent/mcp.rs`](../../src/agent/mcp.rs):
`an_mcp_redirect_to_a_denied_host_reaches_no_connection`.

## What it guarantees, and what it does not

- **Guarantee:** [`egress.allow_list`](../reference/guarantee-catalogue.md#egress.allow_list), whose row lists every
  gated path and what is not covered.
- **Not built:** name resolution (an allowed name resolving to a denied address), a cloud identity's credential
  traffic, and redirects inside `object_store`'s own client; Azure and S3 Express object stores are refused under a
  non-empty list ([crown jewel § Coverage](../operations/crown-jewel.md#coverage)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the list on its own;
  [credentials attached at egress](credentials-at-egress.md) build on it.

## Go deeper

- **Guide:** [09, security](../guide/09-security.md).
- **Runbook:** [crown jewel § 2, Outbound egress allowlist](../operations/crown-jewel.md#2-outbound-egress-allowlist).
- **Design:** no design record; [`plans/realignment-repairs.md`](../plans/realignment-repairs.md) R3–R4 are the record
  of the redirect check and the host parser.
- **Code:** [`EgressPolicy`](../../mycelium-core/src/config.rs) (`permits_url`, `redirect_verdict`) and
  [`src/agent/egress_client.rs`](../../src/agent/egress_client.rs).
