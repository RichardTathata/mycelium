# Node-level HTTP routes behind gateway auth (2026-09-05)

Finding 4 of the external review that produced the persistence fixes
(`2026-09-05-persistence-durability-p1.md`). Verified against `src/agent/http.rs`: with
`gateway_auth_token` set, `POST /mcp`, `GET /signals/{kind}`, `GET /consensus/{slot}` (and
`GET /bulk/{id}`) answered without a bearer. The routing comment listed `/signals` and `/mcp` as
public; `/bulk` and `/consensus` were public with no comment; `rbac.md` and `dev/security.md`
listed only the four probes. The 2026-07 self-audit (ratings.md, "design-level items flagged for
a decision") had recorded `/mcp` as "deliberately public" and left the decision open — taken today.

## Why it is a hole, not a preference
- `tools/call` dispatches `rpc_call_ctx` **as the node**: provider-side `authorized_callers` sees
  the gateway node, not the HTTP caller — a confused deputy. `tools/list` enumerates every tool and
  provider. `/signals/{kind}` is a read of live mesh traffic; `/consensus/{slot}` discloses committed
  values (lock holders).
- Same shape as the 2026-09-04 `with_http_routes` finding that shipped as v2.4.1 (a route answering
  without a bearer while the docs claimed coverage). Exposure needs the listener bound beyond
  loopback (default off / `127.0.0.1`).

## What changed
- `http.rs`: the three routes moved into a small router carrying the **same** `gateway_auth`
  `route_layer` instance as `/gateway/*` (merged, not nested — `MatchedPath` is the bare pattern),
  `required_scope` rows `"/mcp" => mcp:invoke`, `"/signals/{kind}" => mesh:read`,
  `"/consensus/{slot}" => consensus:read`. Open when no token model is configured, like the gateway.
- `/bulk/{id}` **stays public by design**: `fastrand::u64(1..)` per call, staged only for the
  call's lifetime (`StagedGuard`), fetched by the serving *peer* over plain HTTP with no shared
  bearer — a capability URL. Documented in the router, `rbac.md`, `security.md`.
- Docs: module doc route list split public/gated, `gateway_auth` doc, `rbac.md` (scope rows +
  "the whole public surface" statement + upgrade note), wiki `security.md`, CHANGELOG `Security`.
- Gates: `regression_node_level_routes_require_bearer_when_token_set` (401 bare / 200 with token
  for all three; the public five never 401), `node_level_routes_honour_scoped_tokens`
  (compliance: 403 + `required_scope` outside the family), scope-table unit asserts. Existing MCP
  and SSE tests configure no token and are unchanged — the open-deployment path is the same.

## Reusable lesson
The public-route list is a **security claim**; keep it in exactly one place per audience (router
comment ↔ `rbac.md` ↔ wiki) and make the lint compare them. "Public by routing comment" without
a doc row is the drift signature — both 2026-09-04 and today.
