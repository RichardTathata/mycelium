## [2026-09-13] ingest | v3 item 7 — gateway caller identity inside a domain

Up: [dev](../dev.md) · page: [security](../security.md) §WS1.5 · plan
[§6.4 / D27](../../../plans/v3-contracts-axis.md) · runbook `docs/operations/rbac.md` §7.

**Finding.** The 2026-09-05 fix put `/mcp` behind auth but left the deputy confused: every
gateway-originated dispatch (`tools/call`, `/a2a`, `rpc/call`, `scatter`, `emit_reliable`, `llm/*`)
still ran as the **node**, so a provider's `authorized_callers` saw the gateway and never the
client. Listing the gateway node in an allowlist admitted every client behind it. Rev 1.10 put this
first in the v3 queue because everything commercially visible (AE-T: "enforce your remits where the
agents act") presumes the provider can see who acted.

**Change** (`src/agent/gateway_caller.rs`, wire v12 unchanged). The auth layer constructs a
`GatewayCaller` on every dispatch — principal (`oidc:{sub}` · `token:#{i}` · `token:legacy` ·
`anonymous`; never the credential) · `via` (this node) · `scopes` (credential ∩ route, never `*`)
— and the node attests it (Ed25519 over `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)`
under `tls`). It rides inside the RPC payload after the nonce: `[0 G W C 1] ‖ u16 len ‖ JSON ‖
app`. `RpcRequest::payload()` strips it (existing loops unchanged); providers read
`request_principal` / `gateway_caller` and authorise with `request_authorized` (a client by
*principal*; a node by id/roles). Guardrails `check_caller` / `guarded_rpc_serve` and SkillRunner
switched; denial seals carry the principal + `via`. `McpHandle::register_mcp_tool_with_principal`;
`/gateway/rpc/serve` events gain `caller`; `/a2a` resolves an optional bearer (unrecognised ⇒ 401,
never downgraded). Every node writes `sys/caller-context/{self} = b"1"` at start (tripwire prefix
added). `gateway_caller_profile` (`secure` default / `legacy`, env
`GOSSIP_GATEWAY_CALLER_PROFILE`).

**The four negative cases + the gate, all in CI** (`gateway_caller_tests`, `src/agent/http.rs`):
forged client context ignored and a mesh forgery refused as `CallerError`; a missing context is
`-32020`, never node-as-caller; `scopes` is the intersection by construction (a `*` token yields
exactly the route scope); a provider without the marker is `-32021` / HTTP 412 naming it, while
the explicit `legacy` profile still dispatches as the node. Gate: a provider listing the gateway
*node* rejects a gateway client though the node's own direct call is admitted; listing the client's
principal admits it, and under `tls` the provider verified the gateway's signature.

**Design decisions worth keeping.** (1) *Marker, not wire bump*: an older provider would hand the
framed bytes to its handler as payload, so the secure gateway refuses before dispatch on the
absence of `sys/caller-context/{provider}`; wire v12 stays. (2) *Strength follows the mesh*:
without a `tls` identity the frame's sender is unauthenticated already, so an unsigned envelope is
accepted there as `UnauthenticatedMesh` (promise-strength) and refused on a `tls` node
(`Unsigned`). (3) *Principal for scoped tokens is positional* (`token:#i`): `GatewayToken` is a
plain public struct, and a new field would break struct literals (plan §9 versioning rule); a
name field waits for a builder. (4) *`/a2a` stays public* — the A2A peer needs no Mycelium
credential — but the context is still auth-layer-constructed: presented ⇒ resolved or 401.
(5) Consensus `propose` routes are not RPC dispatches and carry no context — out of item 7's scope
(item 5 / AE own the mutation-side attribution).

**A core hazard found on the way (not fixed here — a follow-up).** The marker was first written
inside `start()`; `distributed_lock_grants_single_holder_under_race` then failed deterministically
with **two** lock holders, and the trace showed the first-started node's KV writes (its commit and
lease keys) never reaching its peer while the reverse direction worked. Mechanism: a gossip write
fans out to the **bootstrap** peers as well (`src/agent/tasks.rs`, `targets.extend(bootstrap_peers)`),
so a write before a bootstrap peer is listening creates the outbound writer to it, whose connect
fails and which then **drops every later frame during reconnect backoff** (`writer.rs`, "Dropping
frame … during reconnect backoff"). Any application `kv().set` immediately after `start()` with a
not-yet-up bootstrap peer hits the same window; the lock test only exposes it because its
correctness rides on the commit key converging within one second. The marker is now written lazily
(first peer connected, or a 5 s grace) so it is never the write that trips it; the hazard itself
belongs to item 6's nondeterminism inventory / a writer fix (queue-then-flush, or exclude
never-connected bootstrap peers from data fan-out). Also observed: the same test's wall time swings
between ~1 s and ~60 s on an unchanged `main` depending on which node's bootstrap connect fails
first — a timing signature worth a structural gate, not a widened timeout.

**External review before merge (2026-09-13/14) — four P1 findings, all real, all closed.** (1) *Raw
emission bypass:* `/gateway/signal/emit` sends arbitrary bytes as the node and an RPC request is just a
nonce-prefixed signal, so a client reached any `rpc_rx` provider *as the gateway node*. Closed by making a
secure-profile gateway node **promise an envelope on every RPC it originates** (marker value `"2"`; its own
`rpc_call`s wrap a signed `node:{self}` envelope mapped back to `Node`), so a bare RPC-shaped frame from it is
`CallerError::Missing`. Plain-signal semantics untouched: the raw routes still emit exactly the given bytes;
the enforceable distinction lives at the RPC receive path. Residual: a plain (non-RPC) signal handler
acting on a gateway emission still attributes it to the sender node — plain signals carry no principal, by
design. (2) *Positional identities:* `token:#0` on two gateways was one string. Closed by qualifying every
principal with its issuing authority (`gateway_identity_issuer`, default the node id; `oidc:{idp}/{sub}`)
and adding `gateway_named_tokens` (a new struct beside the old — `GatewayToken` cannot grow a field under
the compatibility rule). (3) *LLM provider stripped without verifying:* closed twice — `rpc_rx` now
verifies at the receive boundary and answers refusals itself (every companion loop covered without a code
change), and the raw-registration receivers (LLM, MCP, explain) verify directly. (4) *Malformed frames
failed open:* `split_frame` returned "no envelope" for a truncated or over-8 KiB frame, which read as a node
call. Closed with a three-way `Frame`, refusal on `Malformed`, an empty payload for structurally broken
frames, and a producer bound (`-32023` / HTTP 413) instead of truncation. The reviewer's four probes were
converted into refusal tests; 18 gates now.

**Behaviour note for adopters.** An allowlist that named the gateway node to admit HTTP clients
must now name the clients' principals (that node-listing *was* the impersonation).
