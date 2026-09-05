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
| `consensus:read` / `consensus:write` | overlay log scan, consistent get, **`/consensus/{slot}` inspection** / consistent set, lock, elect, log append, cross-group propose |
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

> **Since 2026-09-05** the node-level `/mcp`, `/signals/{kind}` and `/consensus/{slot}` sit behind
> the same bearer-then-scope boundary as `/gateway/*`. Before, they answered without a bearer even
> with `gateway_auth_token` set — `POST /mcp` `tools/call` invoked any tool in the cluster **with
> the node's own identity**. A scoped-token deployment must now grant `mcp:invoke` to MCP clients
> (e.g. an LLM host's `Authorization: Bearer …` header), `mesh:read` for the SSE stream and
> `consensus:read` for slot inspection; a legacy `gateway_auth_token` grants all of them.
> **SDK clients:** `mycelium-py` ≥ 0.2.4 and `mycelium-ts` ≥ 0.1.1 take the bearer at construction
> (`token=` / `{ token }`) or from `MYCELIUM_GATEWAY_TOKEN`; earlier versions cannot present one.

**Public, never scope-gated** (M16 edge criterion): `/health`, `/ready`, `/stats`, `/metrics`,
the A2A descriptor (`/.well-known/agent.json`), and `GET /bulk/{id}` — a **capability URL**: the
64-bit random per-call nonce is the credential, and the serving peer fetches it node-to-node with
no shared bearer. **That is the whole public surface**; the routing code asserts the same list.

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
