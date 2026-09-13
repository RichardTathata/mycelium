# Mycelium — RBAC / Identity Operations Runbook

Operator-facing configuration and verification for the WS1 RBAC subset
(`compliance` feature). Concept and API: [`docs/guide/09-security.md`](../guide/09-security.md)
§Role-based access control. Architecture invariants: `CLAUDE.md` §RBAC / identity.

The `compliance` feature is `["gateway", "tls"]`. **TLS is a hard prerequisite** —
roles are Ed25519-signed by the node's TLS identity key, so a node with no
`GossipConfig::tls` cannot advertise roles (`advertise_roles` returns
`InvalidField { field: "tls" }`).

---

## 1. Build & enable

```bash
cargo build --features compliance        # library / embedding
cargo build --bin skillrunner --features compliance
```

```rust
let mut cfg = GossipConfig::default();
cfg.tls = Some(TlsConfig::default());    // required: signs roles, mTLS transport
// gateway ACLs (optional):
cfg.gateway_scoped_tokens = vec![
    GatewayToken { token: "orchestrator".into(),
                   scopes: vec!["kv:read".into(), "kv:write".into(), "mesh:write".into()] },
    GatewayToken { token: "readonly".into(),
                   scopes: vec!["kv:read".into()] },
];
```

Distribute the auto-generated `./mycelium-tls/ca-cert.pem` to every node (shared
cluster CA) — see the TLS runbook in `09-security.md`.

---

## 2. Scope vocabulary (gateway ACLs)

Coarse `resource:verb` families. A route admits a token holding the required
scope **or** `"*"`. Unmapped routes require `admin` (deny-by-default).

| Scope | Grants |
|---|---|
| `kv:read` / `kv:write` | `GET /gateway/kv*` / `POST`,`DELETE /gateway/kv*`, `/kv/quorum` |
| `cap:read` / `cap:write` | capability resolve, shard owner / advertise, drop |
| `mesh:read` / `mesh:write` | signal SSE (`/gateway/signal/sse/{kind}` **and** the node-level `/signals/{kind}`), mailbox/rpc-serve, demand / signal emit, rpc call, scatter |
| `consensus:read` / `consensus:write` | overlay log scan, consistent get, **`/consensus/{*slot}` inspection** / consistent set, lock, elect, log append, cross-group propose |
| `mcp:invoke` | `POST /mcp` — the MCP JSON-RPC bridge (`initialize`, `tools/list`, `tools/call`) |
| `llm:read` / `llm:write` / `llm:invoke` | prompt get/list / prompt put,delete / llm call,stream |
| `audit:read` / `transparency:read` | audit-trail query / revocation transparency log |
| `identity:write` | key revocation (`POST /gateway/identity/revoke`) |
| `*` | everything (the legacy `gateway_auth_token` is equivalent) |
| `llm:read` / `llm:write` / `llm:invoke` (companion) | `mycelium-reason`: trace, blob GET, `/v1/models` / blob PUT / `/reason/route`, `/reason/v1/chat/completions` |
| `wiki:read` / `wiki:write` | `mycelium-wiki`: `/wiki/read`, `/wiki/query` / `/wiki/propose`, `/wiki/ingest` |
| `board:read` / `board:write` | `mycelium-blackboard`: `/bb/read`, `/bb/depth` / `/bb/post`, `claim`, `ack`, `release` |
| `tuple:read` / `tuple:write` | `mycelium-tuple-space`: `/tuple/depth` / `put`, `take`, `take_by_key`, `complete`, `ack` |
| `admin` | the deny-by-default fallback for any route not in the table — including any companion path not listed above |

> **Since 2026-09-04** companion routes (merged via `with_http_routes`) sit behind this gate at
> all. Before, they answered without a bearer even when the library's own routes demanded one.
> A scoped-token deployment that used companion routes must now grant the family scopes above.

> **Since 2026-09-05** the node-level `/mcp`, `/signals/{kind}` and `/consensus/{*slot}` sit behind
> the same bearer-then-scope boundary as `/gateway/*`. Before, they answered without a bearer even
> with `gateway_auth_token` set — `POST /mcp` `tools/call` invoked any tool in the cluster **with
> the node's own identity**. A scoped-token deployment must now grant `mcp:invoke` to MCP clients
> (e.g. an LLM host's `Authorization: Bearer …` header), `mesh:read` for the SSE stream and
> `consensus:read` for slot inspection; a legacy `gateway_auth_token` grants all of them.
> **SDK clients:** `mycelium-py` ≥ 0.2.4 and `mycelium-ts` ≥ 0.1.1 take the bearer at construction
> (`token=` / `{ token }`) or from `MYCELIUM_GATEWAY_TOKEN`; earlier versions cannot present one.

**Public, never scope-gated** (M16 edge criterion): `/health`, `/ready`, `/stats`, `/metrics`,
the A2A descriptor (`/.well-known/agent.json`), `POST /a2a` (an A2A peer needs no Mycelium
credential — but a bearer *presented* on it is resolved, and an unrecognised one is 401; §7), and
`GET /bulk/{id}` — a **capability URL**: the 64-bit random per-call nonce is the credential, and
the serving peer fetches it node-to-node with no shared bearer. **That is the whole public
surface**; the routing code asserts the same list.

---

## 3. Advertise & verify roles

```rust
agent.advertise_roles(["admin".into(), "orchestrator".into()], /* clearance L3 */ 3)?;
```

- `clearance` is the L1/L2/L3 data-classification level (0–255; 1/2/3 by convention).
- The claim persists at `sys/role/{node}` and anti-entropy-syncs like any KV entry;
  re-call to update.
- Other nodes read it **verified**: `agent.roles_of(&node)` returns `Some` only if
  the signature checks against the cluster-learned identity key. A forged write
  reads back as `None`.

Capability providers gate invocations with `authorized_callers` (empty = open):

```toml
# in a .skill.toml — SkillRunner enforces this automatically under compliance
[policy]
authorized_callers = ["orchestrator", "127.0.0.1:8080"]   # role names or NodeIds
```

---

## 4. Verification checklist

```bash
# Gateway ACL — expect 200 / 403 / 401
curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer readonly' \
     http://NODE:PORT/gateway/kv/keys            # 200 (kv:read)
curl -s -w '%{http_code}\n' -H 'Authorization: Bearer readonly' \
     -X POST http://NODE:PORT/gateway/kv -d '{"key":"k","value":"v"}'   # 403 + {"required_scope":"kv:write"}
curl -s -o /dev/null -w '%{http_code}\n' http://NODE:PORT/gateway/kv/keys   # 401 (no token)
curl -s -o /dev/null -w '%{http_code}\n' http://NODE:PORT/health             # 200 (public)
```

---

## 5. The `sys/` namespace tripwire

Core diagnostic (present even without `compliance`). A **remote** write naming
this node in a self-owned `sys/` prefix (`identity`, `load`, `role`, `tuple`)
is flagged — detection, not prevention (the write still applies per LWW).

```bash
curl -s http://NODE:PORT/stats | jq '.sys_namespace_violations'
```

- **Steady-state value: `0`.** Any non-zero value warrants investigation: a peer
  is writing keys only this node should own (misconfiguration, a buggy client,
  or a hostile node in the mesh).
- Each detection also emits a `warn!` naming the offending key.
- `sys/quorum/` is intentionally **not** flagged — peers legitimately write
  quorum evidence naming the node they observed.

Pair with `commit_conflicts` (the consensus tripwire) on the same `/stats`
endpoint as the two "promise-strength namespace violated" signals.

---

## 6. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `advertise_roles` → `InvalidField { field: "tls" }` | no `GossipConfig::tls` | enable TLS; roles require the identity key |
| `roles_of(peer)` always `None` | peer's `sys/identity/` not yet learned, or unshared CA | confirm peering + that the CA cert is distributed |
| every gateway request → 401 | token not in `gateway_scoped_tokens` / no `Bearer` header | check the token list and header |
| legitimate route → 403 | scope not granted; or route is unmapped (needs `admin`) | grant the scope shown in `required_scope`, or `"*"` |
| `sys_namespace_violations` climbing | a peer clobbering this node's owned keys | identify the source from the `warn!` log; treat as a trust-boundary incident |

---

## 7. Gateway caller identity — who a provider sees behind a gateway call

*v3 contracts axis item 7 (`docs/plans/v3-contracts-axis.md` §6.4, D27); shipped on the 2.x line, wire
v12 unchanged.*

**The gap it closes.** Every gateway-originated dispatch — `POST /mcp` `tools/call`, `POST /a2a`,
`/gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable`, `/gateway/llm/*` — used to run
under the **node's** identity: the provider's `authorized_callers` saw the gateway node, never the
Python/TypeScript client. Listing the gateway node in an allowlist therefore admitted *every* client behind
it. That is the confused deputy §2's 2026-09-05 note describes; putting `/mcp` behind auth did not tell the
provider who called.

**What a provider now sees.** On every gateway dispatch the auth layer constructs a `GatewayCaller` and
the node attests it:

| Field | Meaning | Source |
|---|---|---|
| `principal` | the originating client | `oidc:{subject}` (OIDC bearer) · `token:#{i}` (the i-th `gateway_scoped_tokens` entry — by position, never the secret) · `token:legacy` (`gateway_auth_token`) · `anonymous` (open gateway, or `/a2a` without a bearer) |
| `via` | the gateway node acting for it | this node; the provider checks it equals the frame's signature-verified sender |
| `scopes` | the authority granted for *this* request | the credential's scopes ∩ the route's required scope — a `*` token yields exactly the route's scope, never `*`; empty on `/a2a` (no scope required) |
| `attestation` | how it was verified | `Signed { signer }` — Ed25519 by the gateway's identity key over `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)`, verified against the keys the provider knows for `via` (`sys/identity`, anchors, minus revocations) — under `tls`; `UnauthenticatedMesh` on a mesh without a `tls` identity (then the context is exactly as strong as the frame's sender: honoured, promise-strength) |

The context rides inside the RPC payload after the nonce. `RpcRequest::payload()` strips it, so an
existing provider loop sees exactly the application bytes. Read it with
`agent.request_principal(&req)` (→ `RequestPrincipal::Client(GatewayCaller)` or `::Node(NodeId)` for a
direct in-mesh call) and authorise with **`agent.request_authorized(&req, &allow)`** (`compliance`):

```rust
// inside a provider's rpc_rx serve loop — replaces caller_authorized(req.sender(), …)
match agent.request_authorized(&req, &authorized_callers) {
    Ok(true)  => { /* serve */ }
    Ok(false) => { /* deny: principal not listed (a gateway node being listed admits no client) */ }
    Err(e)    => { /* deny: forged / unsigned / mis-attributed context — never fall back to the node */ }
}
```

`mycelium-guardrails` (`check_caller`, `guarded_rpc_serve`) and SkillRunner already do this; their
`Denied` seals name the client principal and carry `via`. MCP tools that need the caller register with
`McpHandle::register_mcp_tool_with_principal`; a Python/TypeScript handler served through
`/gateway/rpc/serve/{kind}` receives it as the event's optional `caller` object
(`{principal, via, scopes, attested}`).

**Allowlists.** `authorized_callers` entries name node ids or roles (direct calls) **and principals**
(gateway clients): `["oidc:alice", "token:#0", "orchestrator"]`. To admit unauthenticated gateway
clients on an open gateway, list `anonymous` explicitly. An empty list stays open.

**The secure profile refuses rather than impersonates** (`gateway_caller_profile = secure`, the
default; env `GOSSIP_GATEWAY_CALLER_PROFILE`). Four cases are CI-gated
(`gateway_caller_tests`, `src/agent/http.rs`):

| Case | Where | Answer |
|---|---|---|
| a client-supplied (forged) context — anything in `params`, `_meta`, a `caller` block | ignored at the gateway; a forged envelope from another node fails verification at the provider | provider sees the auth layer's principal; a mesh forgery is a `CallerError` denial (MCP `-32022`) |
| a missing context falling back to the node | the dispatch site | `-32020` / HTTP `412` `caller_context_missing` — never dispatched as the node |
| a gateway asserting more scope than the credential holds | the auth layer | `scopes` is the intersection; `*` is never carried |
| an older provider that cannot enforce the context | the gateway, before dispatch: no `sys/caller-context/{provider}` marker | `-32021` / HTTP `412` `provider_without_caller_context`, naming the provider; `/gateway/scatter` lists such targets under `refused` |

Every node running this release writes `sys/caller-context/{self} = b"1"` at start (self-owned under the
`sys/` tripwire, §5). The marker is what a secure gateway checks; a node without it is a pre-item-7
provider that would hand the framed bytes to its handler as if they were the payload.

**Rolling upgrade.** Upgrade providers before gateways, or run the gateways with
`gateway_caller_profile = legacy` (node-as-caller, exactly the old behaviour; `warn!` at start) until every
provider publishes its marker, then switch to `secure`. `legacy` is a `3.0.0` removal-ledger entry
(plan §6.6). Metric: `mycelium_gateway_caller_refusals_total{reason}`.

**Deployments to re-check:** any provider allowlist that named the *gateway node* to admit HTTP clients.
That listing was the impersonation; replace it with the clients' principals.

