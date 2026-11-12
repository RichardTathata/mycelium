# The HTTP gateway and its credentials

## What it is

The gateway is a node's HTTP edge: the KV, mesh, consensus and MCP routes (and A2A where it is mounted) that SDK clients, browsers and
external frameworks reach, served from `http_port` in a build with the `gateway` feature. Once a credential model is
configured — a shared bearer, or under `compliance` scoped or named tokens with a per-route scope floor, or
[single sign-on](gateway-sso.md) — every non-public route needs it. With none the gateway is open, which it may be
only on loopback: bound beyond it, it refuses to start unless that is explicitly waived. The resolved principal and its granted [scopes](../guide/00-concepts.md#scope) travel to the
handler.

## When to use it, and when not

- **Use it when** a client outside the Rust process — the Python or TypeScript SDK, a browser, an A2A or MCP
  client — needs to reach the fleet.
- **Not when** the client must be authorised per action, not per route: a token's scope admits a route, and what it
  may do there is the [action evaluator](action-authorisation.md)'s decision.

## Turn it on

Build with `gateway` (the published node has it) and set the port; credentials under `compliance` for the token
tables. The two-domain scaffold's [`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's
two-domains job starts, sets a gateway on loopback.

- [`http_port`](../reference/configuration.md#http_port) and [`http_addr`](../reference/configuration.md#http_addr):
  where it listens; a non-loopback address with no credential refuses the start.
- [`gateway_auth_token`](../reference/configuration.md#gateway_auth_token): one shared bearer; a blank one is refused.
- [`gateway_named_tokens`](../reference/configuration.md#gateway_named_tokens) (preferred) and
  [`gateway_scoped_tokens`](../reference/configuration.md#gateway_scoped_tokens): token tables with scopes, under
  `compliance`; a table in a build that cannot enforce it refuses the start.
- [`gateway_allow_unauthenticated`](../reference/configuration.md#gateway_allow_unauthenticated): the explicit,
  insecure waiver for an exposed open gateway.
- [`gateway_max_connections`](../reference/configuration.md#gateway_max_connections) and
  [`gateway_cors_origins`](../reference/configuration.md#gateway_cors_origins): the connection cap and the browser
  origins answered.
- `an_exposed_gateway_with_no_credential_refuses_to_start`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

No runnable example exists for the gateway on its own: the [example index](../../examples/README.md#the-capability-matrix)'s
examples use it as their edge. The end-to-end run of the credential model is a test in
[`src/agent/http/gateway_caller_tests.rs`](../../src/agent/http/gateway_caller_tests.rs):
`named_tokens_alone_still_close_the_gateway` — with only a named-token table, no bearer and an unknown bearer are
refused 401, a named token is admitted to what its scopes allow, and a read token's write is refused 403. That a
token resolves to its own principal is `token_identities_are_qualified_by_the_issuing_gateway`, in the same file.

## What it guarantees, and what it does not

- **Rule:** [`gateway.auth`](../reference/rule-catalogue.md#gatewayauth) — deny by default under `compliance`;
  open only on loopback.
- **Guarantees:** [`gw.not_open`](../reference/guarantee-catalogue.md#gw.not_open),
  [`gw.exposed_closed`](../reference/guarantee-catalogue.md#gw.exposed_closed) and
  [`gw.token_tables`](../reference/guarantee-catalogue.md#gw.token_tables).
- **The raw KV routes are for application keys, in both directions.** `/gateway/kv`, `/gateway/kv/quorum` and the
  consistent set and get refuse a key in a namespace the substrate or a companion owns — **403** `protected_key`,
  whatever the scopes, naming the route that owns it — so `kv:write` and `kv:read` reach nothing another scope
  guards. `/gateway/kv/keys` lists names only, and not those under `grant/`, `mailbox/`, `audit/`, `sys/audit/`,
  `sys/client-cert/`, `mandate/` or `rights/`. One table decides both directions
  (`OWNED_KV_PREFIXES`, in [`authz.rs`](../../src/agent/http/authz.rs)); the tests are
  `the_raw_kv_routes_refuse_governance_intents`, in [`http_tests.rs`](../../src/agent/http/http_tests.rs), and one
  per read door in [`kv_read_tests.rs`](../../src/agent/http/kv_read_tests.rs). The log read routes are not covered:
  `consensus:read` still reads a stream under `cn/`, `wiki/` or `reason/` ([rbac](../operations/rbac.md)).
- **Not built:** on a credentialed node `/health`, `/ready`, `/stats`, `/metrics`, `/bulk/{id}` and the A2A
  descriptor stay public, `/a2a` admits an anonymous caller, and a merged router's paths outside `/gateway/` are not
  gated (the `gw.exposed_closed` row); a bearer crosses the wire in cleartext unless the gateway serves
  [TLS](gateway-tls.md).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger's
  [companion RPC kind row](../operations/what-is-proven.md#companion-s-rpc-kind-needs-family).

## Go deeper

- **Guide:** [09, security](../guide/09-security.md) and [15, reasoning and LangGraph](../guide/15-reasoning-and-langgraph.md)
  for the SDK clients.
- **Runbook:** [rbac](../operations/rbac.md) (build, the scope vocabulary, verification, failure modes) and
  [production readiness](../operations/production-readiness.md).
- **Design:** no design record; [`plans/post-360-hardening.md`](../plans/post-360-hardening.md) row P1 is the record
  of the refusal of an exposed open gateway.
- **Code:** [`src/agent/http/`](../../src/agent/http/mod.rs), one file per route family; `gateway_auth` and
  `required_scope` in [`authz.rs`](../../src/agent/http/authz.rs).
