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
// Prefer NAMED tokens (2.13.0): the principal a provider sees is `token:{issuer}/{name}`
// instead of a positional `token:{issuer}/#{i}`, so rotating one token does not renumber
// the rest. Fields: name · token · scopes (`GatewayNamedToken`, mycelium-core/src/config.rs).
cfg.gateway_named_tokens = vec![
    GatewayNamedToken { name: "ci-bot".into(), token: "…".into(),
                        scopes: vec!["mcp:invoke".into()] },
    GatewayNamedToken { name: "skill-server".into(), token: "…".into(),
                        scopes: vec!["mesh:serve".into()] },   // serves RPC kinds; cannot call them
];
// From the environment: GOSSIP_GATEWAY_NAMED_TOKENS="ci-bot|…|mcp:invoke;skill-server|…|mesh:serve"
// (entries `;`, fields `|`, scopes `,`; a malformed entry refuses the whole variable at startup).
// Or the TOML file `GossipConfig::load_from_file` reads. The issuer prefix is
// `gateway_identity_issuer` (default: this node's id). Scopes match exactly or "*": a scope such
// as `llm:*` is refused by `validate()` rather than silently admitting nothing.
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
| *(none)* — `/a2a` | **No scope is required on this route.** Auth is *optional*: a federation credential names the partner; a bearer resolves to a principal but its **scopes are dropped**; nothing presented is anonymous. Authority here comes from an `ActionEvaluator`, not the scope table — and with no evaluator attached an anonymous caller reaches skill dispatch. `with_a2a()` warns in that configuration |
| `mesh:serve` | the RPC serve stream (`/gateway/rpc/serve/{kind}`) and `/gateway/rpc/respond`: **serving without the power to call**. Added 2026-09-25 (closure plan C1). For one release a token holding `mesh:read` or `mesh:write` is still admitted here, with a warning; reissue it |
| `mesh:read` / `mesh:write` | signal SSE (`/gateway/signal/sse/{kind}` **and** the node-level `/signals/{kind}`), mailbox subscribe, demand / signal emit, rpc call, scatter, **group membership** (`GET`/`POST`/`DELETE /gateway/mesh/group` — a node joins or leaves *itself*; there is no verb for enrolling another node) |
| `consensus:read` / `consensus:write` | overlay log scan, consistent get, **`/consensus/{*slot}` inspection** / consistent set, lock, elect, log append, cross-group propose |
| `mcp:invoke` | `POST /mcp` — the MCP JSON-RPC bridge (`initialize`, `tools/list`, `tools/call`) |
| `llm:read` / `llm:write` / `llm:invoke` | prompt get/list / prompt put,delete / llm call,stream |
| `audit:read` / `transparency:read` | audit-trail query / revocation transparency log |
| `identity:write` | key revocation (`POST /gateway/identity/revoke`) |
| `federation:read` / `federation:invoke` | federation's **consumer** side (item 2 row 11): `GET /gateway/federation/domain`, `/partners`, `/catalog/{domain}` / `POST /gateway/federation/connect`, `/call`. Split because they are different powers — reading which partners exist is operator information; `connect` and `call` spend this domain's credential on a partner's gateway, under the caller's own name |
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

> **Since 2026-09-23** a deployment whose **only** credential model was `gateway_named_tokens`
> was running an *open* gateway: the auth layer's "is a token model configured?" test counted
> `gateway_auth_token` and `gateway_scoped_tokens` and not the named table, so no bearer was
> required at all and every route was granted exactly the scope it asked for. The tokens
> themselves worked, which is what hid it — presenting one was admitted, and so was presenting
> nothing. Shipped in 2.10.0 and fixed here; **check any deployment on 2.10.0–2.12.0 that
> configures named tokens only** (a request with no `Authorization` header should now be 401).
> `gateway_scoped_tokens`-only and `gateway_auth_token` deployments were never affected.

**Protected RPC kinds are refused on the raw routes** (closure plan C1, 2026-09-25). `rpc/call`, `scatter`,
`signal/emit`, `mailbox/deliver`, `shard/emit` and `overlay/emit_reliable` take an RPC kind from the request body.
`mcp.invoke`, `skill.invoke`, `llm.invoke`, and any kind in `protected_rpc_kinds` (`GOSSIP_PROTECTED_RPC_KINDS`), are
refused there `403` with `{"error": "protected_kind", "kind": …, "message": …}` naming the door to use, whatever the
token's scopes and whether or not `compliance` is built. Before this, a client with `mesh:write` could send
`mcp.invoke` or `skill.invoke` straight to a provider and skip the action evaluator and mandate checks that `/mcp`
and `/a2a` run. The SDKs raise `ProtectedKindError`.

**Public, never scope-gated** (M16 edge criterion): `/health`, `/ready`, `/stats`, `/metrics`,
the A2A descriptor (`/.well-known/agent.json`), `POST /a2a` (an A2A peer needs no Mycelium
credential — but a bearer *presented* on it is resolved, and an unrecognised one is 401; §7), and
`GET /bulk/{id}` — a **capability URL**: the 64-bit random per-call nonce is the credential, and
the serving peer fetches it node-to-node with no shared bearer. **That is the whole public
surface**; the routing code asserts the same list.

---

**Scopes match exactly, or `*`.** `scope_admits` accepts the literal scope or the single wildcard
`"*"`; a token scoped `llm:*` admits **nothing** — the runbooks' phrase *"the `llm:*` family"* is
prose for `llm:read` · `llm:write` · `llm:invoke`, each listed by name.

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
| `principal` | the originating client, **qualified by its issuing authority** | `oidc:{idp issuer}/{subject}` (OIDC bearer) · `token:{issuer}/{name}` (a `gateway_named_tokens` entry — prefer these) · `token:{issuer}/#{i}` (the i-th `gateway_scoped_tokens` entry — by position, so reordering moves the identity) · `token:{issuer}/legacy` (`gateway_auth_token`) · `anonymous` (open gateway, or `/a2a` without a bearer — carries no issuer). `{issuer}` is `gateway_identity_issuer`, or this gateway's node id: two gateways never mint the same identity unless the operator gives them the same issuer on purpose |
| `via` | the gateway node acting for it | this node; the provider checks it equals the frame's signature-verified sender |
| `scopes` | the authority granted for *this* request | the credential's scopes ∩ the route's required scope — a `*` token yields exactly the route's scope, never `*`; empty on `/a2a` (no scope required) |
| `attestation` | how it was verified | `Signed { signer }` — Ed25519 by the gateway's identity key over `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)`, verified against the keys the provider knows for `via` (`sys/identity`, anchors, minus revocations) — under `tls`; `UnauthenticatedMesh` on a mesh without a `tls` identity (then the context is exactly as strong as the frame's sender: honoured, promise-strength) |
| `mandate` | the mandate the caller presented, **carried, not verified** (Boundary H closure plan C2) | a gateway client's `params._meta.mandate` on `/mcp` `tools/call` and `/a2a` (send and stream), or a member's own grant sent with `rpc_call_with_mandate`. Not covered by the gateway's signature, and needs not be: the grant is authority-signed and the possession proof holder-signed over this call, so stripping it only causes a refusal and swapping it fails verification. Read it with `agent.presented_mandate(&req)` and **verify it before relying on it** (closure plan C3 does this for you). Absent from the envelope when none was presented |
| `resource` | the resource the call claims to act on (closure plan C3), e.g. `skill:depot/dispatch@{provider}` | set by the gateway from what it resolved, or named by a member in `rpc_call_with_mandate`. A **claim**: a provider with enforcement on uses it only where the payload cannot name the resource (skills, operator kinds), and only after confirming it names this node and something this node serves |

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
(gateway clients): `["oidc:https://idp.example/alice", "token:gw-a/ci-bot", "orchestrator"]`. To admit
unauthenticated gateway clients on an open gateway, list `anonymous` explicitly (it admits every
gateway's anonymous clients). An empty list stays open.

**The receive boundary verifies for you.** `ServiceHandle::rpc_rx` verifies every request's caller
context before yielding it: a forged, unsigned-on-a-`tls`-mesh, malformed, mis-attributed or *missing*
context is answered with `{"error":"caller context refused: …"}` and never reaches the loop — in this
crate and in every companion. The built-in MCP tool task, the LLM prompt skills and the explain
responder verify the same way. What stays the loop's job is authorising the **verified** principal
(`request_authorized`).

**A gateway node's own RPCs carry a self envelope.** A node that runs a gateway in the secure profile
publishes `sys/caller-context/{self} = "2"` ("every RPC I originate carries an envelope") and wraps
its own `rpc_call`s in a signed `node:{self}` envelope, which providers map back to the node. So a
**raw emission** through `/gateway/signal/emit`, `/gateway/shard/emit` or `/gateway/mailbox/deliver`
that happens to be RPC-shaped arrives with **no** envelope from a node that promised one — refused as
`CallerError::Missing`, never admitted as the node (an external review found that bypass before this
shipped). Plain signal consumers are unaffected: those routes emit exactly the bytes given. A node
without a gateway publishes `"1"` ("I verify on receive") and its bare RPCs are its own actions, as
before.

**The secure profile refuses rather than impersonates** (`gateway_caller_profile = secure`, the
default; env `GOSSIP_GATEWAY_CALLER_PROFILE`). Four cases are CI-gated
(`gateway_caller_tests`, `src/agent/http.rs`):

| Case | Where | Answer |
|---|---|---|
| a client-supplied (forged) context — anything in `params`, `_meta`, a `caller` block | ignored at the gateway; a forged envelope from another node fails verification at the provider | provider sees the auth layer's principal; a mesh forgery is a `CallerError` denial (MCP `-32022`) |
| a missing context falling back to the node | the dispatch site | `-32020` / HTTP `412` `caller_context_missing` — never dispatched as the node |
| a gateway asserting more scope than the credential holds | the auth layer | `scopes` is the intersection; `*` is never carried |
| an older provider that cannot enforce the context | the gateway, before dispatch: no `sys/caller-context/{provider}` marker | `-32021` / HTTP `412` `provider_without_caller_context`, naming the provider; `/gateway/scatter` lists such targets under `refused` |
| a malformed or oversized frame (truncated header, length over 8 KiB, unsupported version, bad envelope) | the provider's receive boundary | `CallerError::Malformed` — refused, never read as an unframed node call; the producer refuses a principal or scope set that would not fit (`-32023` / HTTP `413` `caller_context_too_large`) rather than truncating |
| RPC-shaped bytes through a raw emission route | the provider's receive boundary | `CallerError::Missing` — the gateway node promised an envelope on every RPC; a bare frame from it is not its action |

Every node running this release writes `sys/caller-context/{self} = b"1"` at start (self-owned under the
`sys/` tripwire, §5). The marker is what a secure gateway checks; a node without it is a pre-item-7
provider that would hand the framed bytes to its handler as if they were the payload.

**Rolling upgrade.** Upgrade providers before gateways, or run the gateways with
`gateway_caller_profile = legacy` (node-as-caller, exactly the old behaviour; `warn!` at start) until every
provider publishes its marker, then switch to `secure`. `legacy` is a `3.0.0` removal-ledger entry
(plan §6.6). Metric: `mycelium_gateway_caller_refusals_total{reason}`.

**Deployments to re-check:** any provider allowlist that named the *gateway node* to admit HTTP clients.
That listing was the impersonation; replace it with the clients' principals.

## 8. The action evaluator — authorising *what* a verified caller may do

Section 7 establishes **who** is asking. This is the separate question of whether *this* operation on
*this* resource is allowed, checked as a preflight before dispatch. Developer view:
[guide 20](../guide/20-authorising-actions.md); decision record
[`design/action-envelope-ae0.md`](../design/action-envelope-ae0.md).

**Three attachments, and a node is misconfigured without all three.**

| Attach | If you don't |
|---|---|
| an `ActionEvaluator` | nothing is evaluated; the gateway is inert, exactly as before |
| an `EvidenceJournal` | **the node enforces and records nothing**, and warns at attach time. Treat that warning as a failed deployment |
| the deployed `policy.revision` | the stale-policy check **has nothing to compare against and never fires** — so a gateway running a superseded policy is undetectable, which is the condition the check exists for |

Set the revision to the same string your **deployment report** carries. The report is attributable
testimony about activation — policy digest, enforcement points, activation and effective times,
issuer, route coverage — and it is **never proof of coverage**. Missing, conflicting or stale reports
are explicit rather than assumed away.

### What the three verdicts do at the gateway

- **`Permit`** — dispatch proceeds, and the permit is recorded. Recording only refusals would leave
  the interesting half invisible: an evidence stream that omits its permits cannot support any
  statement about what an agent was *allowed* to do.
- **`Deny`** — refused as `-32030` `action_denied`. The policy establishes the refusal.
- **`Indeterminate`** — refused as `-32031` `authority_not_established`. **Never treated as permit.**
  Nothing says the action is forbidden; only that nothing says it is allowed.

**`Indeterminate` is also what an evaluator *error* produces**, along with an unsupported policy
clause, an unmapped operation, a stale revision and an expired envelope. A `Decision` carrying
evaluation errors alongside a `Permit` verdict is a contradiction, and the seam resolves it to
`Indeterminate` rather than letting it through. Operationally: **a rising
`authority_not_established` is a policy or deployment problem**, not an attack — a stale revision, an
operation outside your reviewed catalogue, or a declared argument callers are not sending.

### The refusal that surprises people

`-32032` `evidence_not_recorded` refuses the action **even when the policy permitted it**, because
the decision could not be written down. Enforcement without attribution is an unlogged gate, so
refusing is the honest failure mode — and it is visible rather than silent.

Which failure you get is the **evidence profile**:

| Profile | When evidence cannot be recorded |
|---|---|
| `Strict` | the dispatch is refused. Enforcement and attribution stand or fall together |
| `Lenient` | the dispatch proceeds, and the evidence records that it was produced under a profile that does not gate — weaker, and **legible as weaker** |

Alert on `mycelium_ae_preflight_refusals_total{reason="evidence_not_recorded"}`. It is a storage or
journal fault presenting as an authorisation outage, and it will not look like one in a dashboard of
denials.

### What a gateway check does and does not promise

It is **self-imposed prevention for the routes this gateway fronts, and nothing for routes it does
not**. Only a check inside the resource's own effect boundary earns hard prevention. The evidence
says so itself: `coverage.complete: false` names the routes the enforcement point cannot see. **Never
read silence as an all-clear.**

