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
  guards. `/gateway/kv/keys` lists names only, and not those under `grant/`, `mailbox/`, `audit/`, `sys/audit/`, `sys/audit-checkpoint/`,
  `sys/client-cert/`, `mandate/`, `rights/`, `sched/` or `call/`. One table decides both directions
  (`OWNED_KV_PREFIXES`, in [`authz.rs`](../../src/agent/http/authz.rs)); the tests are
  `the_raw_kv_routes_refuse_governance_intents`, in [`http_tests.rs`](../../src/agent/http/http_tests.rs), and one
  per read door in [`kv_read_tests.rs`](../../src/agent/http/kv_read_tests.rs).
- **The log routes are for application streams, in both directions.** Append and compact, and scan, subscribe and
  group subscribe, refuse a stream under `cn/`, `wiki/` or `reason/` — a commitment's records, a wiki's durable
  proposals, a reasoning trace — **403** `protected_stream`, whatever the scopes and whether or not the stream
  exists; a read's refusal says how the stream is read, and a subscribe is refused before any event is sent or a
  claim proposed. So `consensus:read` and `consensus:write` reach nothing `llm:read` or a component guards. One
  table decides both directions (`OWNED_LOG_STREAMS`, in [`authz.rs`](../../src/agent/http/authz.rs)); the tests
  are one per read door in [`log_read_tests.rs`](../../src/agent/http/log_read_tests.rs) and, for the writes,
  `the_raw_kv_routes_refuse_governance_intents`. The key names follow: `/gateway/kv/keys` leaves out every name
  under `log/cn/`, `log/wiki/`, `log/reason/` and the same three under `clog/`, derived from the same table
  (`the_raw_listing_hides_a_component_streams_key_names`; [deprecations §101](../guide/deprecations.md)).
- **The slot routes are for application slots, in both directions.** `GET /consensus/{slot}` refuses a slot a
  component owns — `cn/` (a commitment's award), `clog/` (a consumer group's claim), `electorate/`, `ae-seat/`,
  `capauthz/`, and `consistent/{key}` for an owned key — **403** `protected_slot`, whatever the scopes, before the
  store is read, so the answer is the same whether or not anything was decided; the refusal says how the decision
  is read (an electorate: `GET /gateway/govern`, `govern:read`). It still reads a lock's slot and a leader's — the
  operator's read of a holder — and every application slot. `POST /gateway/consensus/cross_group_propose`, the one
  route whose body names the slot, refuses a proposal in those families and in `lock/`, `leader/` and
  `consistent/`, naming the route that proposes there, before anything is proposed. So, on these two routes,
  `consensus:read` reads nothing `govern:read` or a component guards, and `consensus:write` decides no award,
  policy, seat or claim. One
  table decides both directions (`SLOT_FAMILIES`, in [`authz.rs`](../../src/agent/http/authz.rs)); the tests are
  one per door in [`slot_guard_tests.rs`](../../src/agent/http/slot_guard_tests.rs), the table's rows pinned
  against the substrate's own family lists, and `a_committed_award_is_not_read_or_proposed_through_the_slot_routes`
  in `mycelium-commitment`. **Not covered:** the slot *names* are still listed by `/gateway/kv/keys` under
  `consensus/` (critique plan row CR24). The signal routes, which take a caller-chosen *kind* and so reached a
  slot's value by another door, refuse the consensus kinds — the next item
  ([rbac](../operations/rbac.md), [deprecations §102](../guide/deprecations.md)).
- **A kind the caller names is never the substrate's own.** Ten doors take a signal kind from the request: the two
  signal streams, the serve stream, the mailbox stream and the six raw send routes. Each refuses the consensus
  protocol's kinds (`consensus.propose`, `consensus.vote`, `consensus.commit`, `consensus.nack`), the node-to-node
  `sys.explain` and `sys.replica-sync.v1`, the bulk transport's ticket `invoke.bulk`, and any kind starting
  `consensus.` or `sys.` — **403** `protected_kind`, for every token. The send routes and the mailbox stream
  refuse the reply kinds `rpc.result` and `bulk.result`; and the signal streams, the mailbox stream and the send
  routes refuse a hosted capability's `cap.invoke/…`, which is called as a tool through `/mcp` (a capability in
  the `tool` namespace; one in another namespace has no gateway route). So `mesh:read` reads no consensus
  proposal and no bulk payload, `mesh:write` sends no commit and no reply and invokes no component, and neither
  reaches a peer's event ring. One list decides (`SUBSTRATE_KINDS` and `is_substrate_kind`, in
  [`src/agent/mod.rs`](../../src/agent/mod.rs)); the tests are in
  [`substrate_kind_tests.rs`](../../src/agent/http/substrate_kind_tests.rs), five of them the proofs of what the
  doors did before, and one that classifies every kind constant as refused or open by decision and fails on a new
  one. **Not covered:** a kind outside every list is open; the supervisor signals are unsigned (any `mesh:write`
  holder can veto; it fails closed); a tap on an ordinary RPC kind shows its requests' caller envelope and
  mandate; and the mailbox stream refuses these kinds where a *member* filled the mailbox too
  ([rbac](../operations/rbac.md), [deprecations §103](../guide/deprecations.md)).
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
