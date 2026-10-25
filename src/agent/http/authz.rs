//! The shared authorisation-and-evidence layer every route family goes through (plan row P5): the
//! bearer-then-scope middleware and the route → scope table (`required_scope`), the action-evaluator
//! preflight and its evidence records (`ae_preflight` / `ae_record`), and the per-door refusals every
//! family calls before it dispatches — a protected RPC kind, an owned KV key, an owned log stream.

use super::*;

/// Axum middleware applied to every `/gateway/**` route.
///
/// Two layers, the second feature-gated:
///
/// 1. **Authentication** (always): when `gateway_auth_token` is set, or
///    (compliance) any `gateway_scoped_tokens` are configured, every gateway
///    request must carry a valid `Authorization: Bearer <token>`. With neither
///    set the gateway is open — which `start()` permits only on a loopback
///    `http_addr` or under `gateway_allow_unauthenticated`
///    (`guarantee::gateway_exposure`, plan P1). `/health`, `/ready`,
///    `/stats`, `/metrics`, `/bulk/{id}` and the descriptor path stay public
///    regardless; the node-level `/mcp`, `/signals/{kind}` and `/consensus/{*slot}`
///    carry this same layer (2026-09-05).
///
/// 2. **OAuth2 scope authorization** (`compliance` feature): the presented
///    token resolves to a scope grant — `gateway_auth_token` ⇒ the `"*"`
///    wildcard (full access, unchanged behaviour), or a `gateway_scoped_tokens`
///    entry ⇒ its scopes. The matched route requires a `resource:verb` scope
///    ([`required_scope`]); the request is admitted only if the grant holds it
///    or `"*"`. Deny-by-default: an unmapped gateway route requires `admin`.
pub(super) async fn gateway_auth(
    State(ctx): State<Arc<HttpCtx>>,
    request: Request,
    next: Next,
) -> Response {
    let cfg = &ctx.agent_ctx.config;
    let legacy = cfg.gateway_auth_token.as_deref();

    // **Every** token table counts, not only the positional one. `gateway_named_tokens` was added
    // in 2.10.0 and `resolve_token` has honoured it since, but this predicate did not — so a
    // deployment whose *only* credential model was named tokens (the model the config docs tell an
    // operator to prefer) fell into the open-gateway branch below: no bearer required, and
    // `open_gateway_scopes` granting each route exactly the scope it asks for. The tokens worked,
    // which is what hid it: presenting one was admitted, and so was presenting nothing.
    #[cfg(feature = "compliance")]
    let have_scoped = !cfg.gateway_scoped_tokens.is_empty() || !cfg.gateway_named_tokens.is_empty();
    #[cfg(not(feature = "compliance"))]
    let have_scoped = false;

    #[cfg(feature = "compliance")]
    let have_oidc = ctx.oidc.is_some();
    #[cfg(not(feature = "compliance"))]
    let have_oidc = false;

    // The scope this route requires (compliance) — also the authority a caller context is
    // granted for the request (item 7: never more than the credential holds, never more than
    // the route needs).
    #[cfg(feature = "compliance")]
    let required_scope_for_route: &'static str = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| required_scope(request.method(), m.as_str()))
        .unwrap_or("admin");
    #[cfg(feature = "compliance")]
    let required: Option<&'static str> = Some(required_scope_for_route);
    #[cfg(not(feature = "compliance"))]
    let required: Option<&'static str> = None;

    // Open gateway: no token model and no OIDC configured. The caller context still exists —
    // principal `anonymous`, granted what the route needs (compliance) or everything (no scope
    // model exists to intersect with) — so a provider can tell a gateway client from the node.
    if legacy.is_none() && !have_scoped && !have_oidc {
        let mut request = request;
        request.extensions_mut().insert(ResolvedPrincipal::anonymous(open_gateway_scopes(required)));
        return next.run(request).await;
    }

    let presented = request.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(presented) = presented else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "authentication required"})),
        ).into_response();
    };

    let Some((principal, scopes)) = resolve_bearer(&ctx, presented).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "authentication required"})),
        ).into_response();
    };

    #[cfg(feature = "compliance")]
    if !scope_admits(&scopes, required_scope_for_route) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "insufficient scope", "required_scope": required_scope_for_route})),
        ).into_response();
    }

    // Item 7: the resolved principal and the authority granted for this request travel to the
    // dispatch site as a request extension. Constructed here and nowhere else.
    let granted = match required {
        Some(_) => gateway_caller::granted_scopes(&scopes, required),
        // Without compliance the token is authenticated but not scope-gated: what it holds.
        None => scopes,
    };
    let mut request = request;
    request.extensions_mut().insert(ResolvedPrincipal { principal, scopes: granted });

    next.run(request).await
}

/// The authority an open gateway grants a request: the route's scope under `compliance`
/// (there is a scope model, and this is what the request needed); everything otherwise (no
/// scope model exists — the deployment has decided everyone on the port is trusted).
pub(super) fn open_gateway_scopes(required: Option<&'static str>) -> Vec<String> {
    match required {
        Some(r) => vec![r.to_string()],
        None => vec!["*".to_string()],
    }
}

/// Resolve a presented bearer to `(principal, scopes)`: OIDC first (a JWT from the IdP →
/// `oidc:{subject}` + groups→scopes), then the static token table. A JWT that fails OIDC
/// validation won't match a static token either, so it correctly ends in `None` (→ 401).
pub(super) async fn resolve_bearer(ctx: &HttpCtx, presented: &str) -> Option<(String, Vec<String>)> {
    #[cfg(feature = "compliance")]
    if let Some(verifier) = &ctx.oidc
        && let Some(principal) = verifier.verify(presented).await
    {
        return Some((gateway_caller::oidc_principal(verifier.issuer(), &principal.subject), principal.scopes));
    }
    resolve_token(&ctx.agent_ctx.config, &gateway_identity_issuer(&ctx.agent_ctx), presented)
}

/// The issuer that qualifies this gateway's local principals: `gateway_identity_issuer`, or this
/// node's id (item 7, review finding 2 — a token's list position is a gateway-local name, never a
/// domain-wide identity).
pub(super) fn gateway_identity_issuer(ctx: &TaskCtx) -> String {
    ctx.config.gateway_identity_issuer.clone().unwrap_or_else(|| ctx.node_id.to_string())
}

/// Optional authentication for `POST /a2a` (item 7). The route is public by design (an A2A
/// peer needs no Mycelium credential), but the caller context must still be *constructed by
/// this layer*: a valid bearer resolves to its principal; no bearer is `anonymous`; a presented
/// but unrecognised bearer is refused (never silently downgraded to anonymous). No scope is
/// required, so none is granted.
pub(super) async fn a2a_optional_auth(ctx: Arc<HttpCtx>, mut request: Request, next: Next) -> Response {
    // Item 2 PR 8: a federation credential, if presented, is the caller's identity — authenticated
    // here, authorised for its export in the handler. Present-and-refused is a refusal, never
    // anonymous (see `federation_http`).
    #[cfg(feature = "tls")]
    match super::federation_http::authenticate_presented(&ctx.agent_ctx, request.headers()) {
        Ok(None) => {}
        Ok(Some(identity)) => {
            super::federation_http::insert_identity(&mut request, identity);
            return next.run(request).await;
        }
        Err(response) => return *response,
    }
    let presented = request.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let resolved = match presented {
        None => ResolvedPrincipal::anonymous(Vec::new()),
        Some(token) => match resolve_bearer(&ctx, &token).await {
            Some((principal, _held)) => ResolvedPrincipal { principal, scopes: Vec::new() },
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "authentication required"})),
                ).into_response();
            }
        },
    };
    request.extensions_mut().insert(resolved);
    next.run(request).await
}

/// [`gateway_auth`] for routes merged via `with_http_routes`: applied only when the request
/// path is under `/gateway/`, so a companion's gateway routes get the library's auth
/// boundary (bearer, then scope — an unmapped `/gateway/` route is deny-by-default `admin`)
/// while its intentionally public paths pass straight through.
pub(super) async fn gateway_auth_if_gateway_path(
    State(ctx): State<Arc<HttpCtx>>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/gateway/") {
        gateway_auth(State(ctx), request, next).await
    } else if request.uri().path() == "/a2a" {
        a2a_optional_auth(ctx, request, next).await
    } else if request.uri().path().starts_with("/federation/") {
        federation_path_auth(ctx, request, next).await
    } else {
        next.run(request).await
    }
}

/// `/federation/*` (item 2 PR 8): a credential is required. Without `tls` no federation route is
/// ever mounted, so the branch falls through to the router's 404.
#[cfg(feature = "tls")]
pub(super) async fn federation_path_auth(ctx: Arc<HttpCtx>, request: Request, next: Next) -> Response {
    super::federation_http::federation_auth(Arc::clone(&ctx.agent_ctx), request, next).await
}

#[cfg(not(feature = "tls"))]
pub(super) async fn federation_path_auth(_ctx: Arc<HttpCtx>, request: Request, next: Next) -> Response {
    next.run(request).await
}

/// Map a presented bearer token to `(principal, scopes)`, or `None` if unrecognised.
///
/// Every gateway-local principal is qualified by `issuer` (this gateway's identity issuer): the
/// legacy `gateway_auth_token` is `token:{issuer}/legacy` with scopes `["*"]` (deployments that
/// only set it behave exactly as before); a named token (`gateway_named_tokens`, `compliance`) is
/// `token:{issuer}/{name}`; a positional token (`gateway_scoped_tokens`) is `token:{issuer}/#{i}` —
/// the list position, which reordering moves, so prefer named tokens. Never the secret.
///
/// Comparison is constant-time in the secret's bytes (`subtle::ConstantTimeEq`): `==` returns at
/// the first differing byte, which leaks the length of the matching prefix to a timing observer.
/// Lengths still differ in time, as in every constant-time string compare.
pub(super) fn resolve_token(cfg: &crate::config::GossipConfig, issuer: &str, presented: &str) -> Option<(String, Vec<String>)> {
    use subtle::ConstantTimeEq as _;
    // A blank bearer never authenticates, whatever the table holds (#598's review): `validate()`
    // refuses a blank secret, and this holds for a config that never passed through it. HTTP/1 trims
    // `Bearer `; HTTP/2 delivers the empty string.
    if presented.trim().is_empty() {
        return None;
    }
    let same = |secret: &str| bool::from(secret.as_bytes().ct_eq(presented.as_bytes()));
    if let Some(legacy) = cfg.gateway_auth_token.as_deref()
        && same(legacy)
    {
        return Some((gateway_caller::legacy_token_principal(issuer), vec!["*".to_string()]));
    }
    #[cfg(feature = "compliance")]
    {
        for t in &cfg.gateway_named_tokens {
            if same(&t.token) {
                return Some((gateway_caller::named_token_principal(issuer, &t.name), t.scopes.clone()));
            }
        }
        for (i, t) in cfg.gateway_scoped_tokens.iter().enumerate() {
            if same(&t.token) {
                return Some((gateway_caller::positional_token_principal(issuer, i), t.scopes.clone()));
            }
        }
    }
    #[cfg(not(feature = "compliance"))]
    let _ = issuer;
    None
}

/// True if `scopes` grants `required` (exact match or the `"*"` wildcard).
#[cfg(feature = "compliance")]
pub(super) fn scope_admits(scopes: &[String], required: &str) -> bool {
    scopes.iter().any(|s| s == "*" || s == required)
}

/// The OAuth2 scope a gateway route requires, keyed on its matched-path pattern
/// and method. This is the gateway ACL policy table; deny-by-default — any route
/// not listed requires `admin`. Scopes are coarse `resource:verb` families so the
/// vocabulary stays small (`kv`, `cap`, `mesh`, `consensus`, `llm` × `read`/`write`,
/// plus `llm:invoke`).
#[cfg(feature = "compliance")]
pub(super) fn required_scope(method: &axum::http::Method, matched_path: &str) -> &'static str {
    use axum::http::Method;
    let read = method == Method::GET;
    match matched_path {
        // KV
        "/gateway/kv"          => if read { "kv:read" } else { "kv:write" },
        "/gateway/kv/keys"     => "kv:read",
        "/gateway/kv/quorum"   => "kv:write",
        // Capabilities
        "/gateway/capability/advertise"   => "cap:write",
        "/gateway/capability/{handle_id}" => "cap:write",
        "/gateway/capability/{handle_id}/heartbeat" => "cap:write",
        "/gateway/capability/resolve"     => "cap:read",
        // Q2: an SDK agent's unit file — capabilities, requirements, groups — under one handle.
        "/gateway/units/declare"          => "cap:write",
        // Adopter-surface X3: the supervisor's door.
        "/gateway/grants/pending"         => "grant:read",
        "/gateway/grants/{agent}/{request}" => "grant:issue",
        "/gateway/grants/{agent}/{request}/revoke" => "grant:issue",
        "/gateway/shard/{ns}/{name}"      => "cap:read",
        // Layer II mesh messaging
        "/gateway/signal/emit"     => "mesh:write",
        "/gateway/signal/sse/{kind}" => "mesh:read",
        "/gateway/demand"          => "mesh:read",
        "/gateway/rpc/call"        => "mesh:write",
        // Closure plan C1: serving is its own scope, so an agent that serves skills needs no power
        // to *call* (`mesh:write` also opens `rpc/call`). The 2.15.0 window that admitted `mesh:read`/
        // `mesh:write` here is closed (2.18.2).
        "/gateway/rpc/serve/{kind}" => "mesh:serve",
        "/gateway/rpc/respond"     => "mesh:serve",
        "/gateway/scatter"         => "mesh:write",
        "/gateway/mailbox/deliver" => "mesh:write",
        "/gateway/mailbox/{kind}"  => "mesh:read",
        "/gateway/shard/emit"      => "mesh:write",
        // Node-level routes behind the gate since 2026-09-05 (MatchedPath is the bare pattern —
        // these are merged, not nested under /gateway).
        "/mcp"              => "mcp:invoke",
        "/signals/{kind}"   => "mesh:read",
        "/consensus/{*slot}" => "consensus:read",
        // Layer III consensus / consistency overlay
        "/gateway/overlay/consistent/set"      => "consensus:write",
        "/gateway/overlay/consistent/get"      => "consensus:read",
        "/gateway/overlay/lock/acquire"        => "consensus:write",
        "/gateway/overlay/lock/{guard_id}"     => "consensus:write",
        "/gateway/overlay/elect"               => "consensus:write",
        "/gateway/overlay/elect/{group}"       => "consensus:write",
        "/gateway/overlay/log/append"          => "consensus:write",
        "/gateway/overlay/log/scan"            => "consensus:read",
        "/gateway/overlay/log/compact"         => "consensus:write",
        "/gateway/overlay/log/subscribe"       => "consensus:read",
        "/gateway/overlay/log/group/subscribe" => "consensus:read",
        "/gateway/overlay/emit_reliable"       => "consensus:write",
        "/gateway/consensus/cross_group_propose" => "consensus:write",
        // LLM / prompt skills
        "/gateway/prompts"             => "llm:read",
        "/gateway/prompts/{ns}/{name}" => if read { "llm:read" } else { "llm:write" },
        "/gateway/llm/call"            => "llm:invoke",
        "/gateway/llm/stream"          => "llm:invoke",
        // Audit trail (WS2)
        "/gateway/audit"               => "audit:read",
        "/gateway/transparency"        => "transparency:read",
        "/gateway/identity/revoke"     => "identity:write",
        // WS-C governance (intent publish + effective-state snapshot)
        "/gateway/govern"              => "govern:read",
        "/gateway/govern/tuning"       => "govern:write",
        "/gateway/govern/timing"       => "govern:write",
        "/gateway/govern/membership"   => "govern:write",
        "/gateway/govern/topology-override" => "govern:write",
        // Membership is a node speaking for itself: read the roster with `mesh:read`, join or
        // leave **this** node with `mesh:write`. There is deliberately no verb for enrolling
        // another node — see `gw_group_join`.
        "/gateway/mesh/group"          => if read { "mesh:read" } else { "mesh:write" },
        "/gateway/govern/group"        => "govern:write",
        "/gateway/govern/electorate"   => "govern:write",
        "/gateway/govern/profile"      => "govern:write",
        // Legible Emergence Phase 2/3: the relational fleet snapshot + causal explain.
        "/gateway/fleet"               => "fleet:read",
        "/gateway/explain"             => "fleet:read",
        "/gateway/diagnose"            => "fleet:read",
        "/gateway/guarantees"          => "fleet:read",
        // Companion-crate gateway surfaces (merged via `with_http_routes`; behind this
        // layer since 2026-09-04). Exact paths only — a companion route not listed here
        // stays deny-by-default `admin`, like any other unmapped route.
        //   mycelium-reason → the `llm` family (routed inference is an LLM call).
        "/gateway/reason/route"               => "llm:invoke",
        "/gateway/reason/v1/chat/completions" => "llm:invoke",
        "/gateway/reason/v1/models"           => "llm:read",
        "/gateway/reason/trace/{run_id}"      => "llm:read",
        "/gateway/reason/blob/{id}"           => "llm:read",
        "/gateway/reason/blob"                => "llm:write",
        //   mycelium-wiki → `wiki:*`.
        "/gateway/wiki/read"    => "wiki:read",
        "/gateway/wiki/query"   => "wiki:read",
        "/gateway/wiki/propose" => "wiki:write",
        "/gateway/wiki/ingest"  => "wiki:write",
        //   mycelium-blackboard → `board:*`.
        "/gateway/bb/read"    => "board:read",
        "/gateway/bb/depth"   => "board:read",
        "/gateway/bb/post"    => "board:write",
        "/gateway/bb/claim"   => "board:write",
        "/gateway/bb/ack"     => "board:write",
        "/gateway/bb/release" => "board:write",
        //   mycelium-wasm-host → `artifact:*` (A3): a signed catalogue line into `installable/`.
        //   Its own family, not `kv:write`, because it is a narrower power with a check the raw
        //   KV route does not make (provenance against the node's trusted publishers).
        "/gateway/artifacts/publish" => "artifact:publish",
        //   mycelium-tuple-space → `tuple:*`.
        "/gateway/tuple/depth"       => "tuple:read",
        "/gateway/tuple/overview"    => "tuple:read",
        "/gateway/tuple/put"         => "tuple:write",
        "/gateway/tuple/take"        => "tuple:write",
        "/gateway/tuple/take_by_key" => "tuple:write",
        "/gateway/tuple/complete"    => "tuple:write",
        "/gateway/tuple/ack"         => "tuple:write",
        // Federation's consumer side (item 2 row 11). `federation:invoke` is separate from
        // `federation:read` because the two are different powers: reading which partners exist and
        // what they last exported is operator information, while `connect` and `call` spend this
        // domain's credentials on a partner's gateway under the caller's own name.
        "/gateway/federation/domain"           => "federation:read",
        "/gateway/federation/partners"         => "federation:read",
        "/gateway/federation/catalog/{domain}" => "federation:read",
        "/gateway/federation/connect"          => "federation:invoke",
        "/gateway/federation/call"             => "federation:invoke",
        // Deny-by-default.
        _ => "admin",
    }
}

/// The signal SSE doors' refusal: a protected kind (as on the raw routes), or an RPC reply kind
/// (`rpc.result`, `bulk.result`). A reply the interceptor did not claim — late, or from a node the
/// call was not sent to — falls through to the handler table, so streaming the reply kinds handed
/// tool and LLM output to any `mesh:read` holder (#587 F7). Same body as the raw routes' refusal.
pub(super) fn refuse_unobservable_kind(cfg: &crate::config::GossipConfig, kind: &str) -> Option<axum::response::Response> {
    use crate::signal::signal_kind::{BULK_RESULT, RPC_RESULT};
    if kind == RPC_RESULT || kind == BULK_RESULT {
        warn!(kind, "gateway: an RPC reply kind refused on a signal stream");
        return Some((
            StatusCode::FORBIDDEN,
            Json(json!({
                "ok": false,
                "error": "protected_kind",
                "kind": kind,
                "message": format!("`{kind}` carries RPC replies and is not observable on a signal stream; a reply reaches only its caller"),
            })),
        ).into_response());
    }
    refuse_protected_kind(cfg, kind)
}

/// The serve stream's refusal (post-360 row R's review, finding 1): `GET /gateway/rpc/serve/{kind}` registers a
/// receiver on the same handler table, and `mcp.invoke`, `skill.invoke` and `llm.invoke` fan out to every receiver
/// — so a `mesh:serve` client serving one of them received every request of that kind this node was sent, a
/// confidential prompt routed in-domain included, and could answer first in the provider's place. Those three are
/// served by this node's own registrations (MCP tools, skills, prompt skills), never over a client stream. An
/// **operator-listed** protected kind (`protected_rpc_kinds`) stays servable: an SDK agent is its provider, checked
/// by provider enforcement before each request is streamed (closure plan C3). The RPC reply kinds are refused as on
/// the signal streams.
pub(super) fn refuse_unservable_kind(kind: &str) -> Option<axum::response::Response> {
    use crate::signal::signal_kind::{BULK_RESULT, RPC_RESULT};
    let builtin = crate::agent::BUILTIN_PROTECTED_RPC_KINDS.contains(&kind);
    if !builtin && kind != RPC_RESULT && kind != BULK_RESULT {
        return None;
    }
    warn!(kind, "gateway: a kind this node serves itself refused on rpc/serve");
    Some((
        StatusCode::FORBIDDEN,
        Json(json!({
            "ok": false,
            "error": "protected_kind",
            "kind": kind,
            "message": format!("`{kind}` is served by this node's own registrations and is not served over rpc/serve: \
                                a stream for it would receive every such request this node is sent"),
        })),
    ).into_response())
}

/// **Closure plan C1.** The gateway's raw routes (`rpc/call`, `scatter`, `signal/emit`,
/// `mailbox/deliver`, `shard/emit`, `overlay/emit_reliable`) take the RPC kind from the request
/// body. Before this, a client with `mesh:write` could send `mcp.invoke` or `skill.invoke` straight
/// to a provider through them, framed with its own principal, and the AE preflight that `/mcp` and
/// `/a2a` run, mandates included, never ran. `llm.invoke` the same, around `llm:invoke`.
///
/// A protected kind is refused `403` with the door to use instead. It is refused whatever the
/// token's scopes (the legacy token holds `*`), and whether or not `compliance` is built in.
///
/// The two signal SSE doors (`/signals/{kind}`, `/gateway/signal/sse/{kind}`) refuse it too: they
/// register on the same handler table the serve routes and the native MCP tools use, and a signal
/// fans to every receiver, so observing a protected kind is reading every protected request's frame.
pub(super) fn refuse_protected_kind(cfg: &crate::config::GossipConfig, kind: &str) -> Option<axum::response::Response> {
    if !is_protected_kind(cfg, kind) {
        return None;
    }
    let door = match kind {
        k if k == crate::signal::signal_kind::MCP_INVOKE => "/mcp (tools/call)",
        "skill.invoke" => "/a2a",
        k if k == crate::signal::signal_kind::LLM_INVOKE => "/gateway/llm/call",
        _ => "the route your operator publishes for it",
    };
    warn!(kind, "gateway: protected RPC kind refused on a raw route");
    Some((
        StatusCode::FORBIDDEN,
        Json(json!({
            "ok": false,
            "error": "protected_kind",
            "kind": kind,
            "message": format!("`{kind}` is protected work and is not accepted on raw mesh routes; use {door}"),
        })),
    ).into_response())
}

/// What the preflight decided, and what the caller owes afterwards.
///
/// Three-valued rather than `Option<Refusal>` because a permitted dispatch leaves an obligation: the
/// decision said what was *allowed*, and only the caller can say what then *happened*. Carrying the
/// decided record out is what lets the execution record reuse its identities instead of re-deriving
/// them — a second derivation is a second chance to disagree.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) enum Preflight {
    /// No evaluator is attached. The seam is inert and the gateway behaves as it always did.
    Inert,
    /// Permitted. The caller dispatches, then reports the outcome with
    /// [`ae_record_execution`]. `journal_sha256` is the decision record's content hash when the journal
    /// established it — what a door forwards as the `parent` of the work it starts (adopter-surface X4), so
    /// the provider's record cites this decision and a closure can be computed across the two journals.
    Proceed { evidence: Box<super::action_evaluator::AeEvidence>, journal_sha256: Option<String> },
    /// Refused. Nothing is dispatched.
    Refuse(super::action_evaluator::PreflightRefusal),
}

/// The MCP tool-call route, as it is named in every piece of evidence it produces.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) const ENFORCEMENT_POINT_MCP: &str = "gateway:mcp/tools/call";

/// What the gateway's action preflight decided for a route a **companion** mounts
/// (`with_http_routes`) — [`GossipAgent::gateway_preflight`]'s answer, carried back to
/// [`GossipAgent::gateway_record_execution`] once the dispatch has happened.
///
/// Opaque on purpose: it exists so a companion's door runs the same preflight `/mcp`, `/a2a` and
/// `/gateway/llm/call` run — evaluator, mandate assessment, evidence record — without the
/// evaluator types, which are only built with `tls`. Without `tls` every preflight is inert.
pub struct GatewayPreflight {
    #[cfg(feature = "tls")]
    inner: Preflight,
}

/// A preflight refusal in the shape every gateway door answers with: the JSON-RPC code, the
/// machine-readable reason, the sentence, and the `data` object (`reason`, `policy_revision`,
/// `checked`, …).
#[derive(Clone, Debug)]
pub struct GatewayRefusal {
    /// The JSON-RPC error code (`-32030` for a denial, …), as `/mcp` sends it.
    pub code: i32,
    /// `action_denied`, `not_established`, `evidence_not_recorded`, … — never prose.
    pub reason: String,
    /// The refusal as a sentence.
    pub message: String,
    /// The `error.data` object `/mcp` and `/a2a` send.
    pub data: serde_json::Value,
}

impl GatewayPreflight {
    /// `Some` when the dispatch must not happen. `None`: proceed (permitted, or no evaluator is
    /// attached), then report the outcome with
    /// [`GossipAgent::gateway_record_execution`].
    pub fn refusal(&self) -> Option<GatewayRefusal> {
        #[cfg(feature = "tls")]
        if let Preflight::Refuse(r) = &self.inner {
            return Some(GatewayRefusal {
                code: r.json_rpc_code(),
                reason: r.reason().to_string(),
                message: r.to_string(),
                data: r.error_data(),
            });
        }
        None
    }

    /// `true` when no evaluator is attached (or none can be in this build): nothing was decided
    /// and nothing will be recorded. A door whose guarantee needs a decision — `mycelium-reason`'s
    /// classifying router (post-360 row R, round-2 review A) — refuses on it rather than dispatch.
    /// Deprecated in 2.33.0 as unreferenced, with the promise to undeprecate it once used; used now.
    pub fn is_inert(&self) -> bool {
        #[cfg(feature = "tls")]
        return matches!(self.inner, Preflight::Inert);
        #[cfg(not(feature = "tls"))]
        true
    }
}

impl crate::GossipAgent {
    /// The action preflight a gateway door runs **between the auth layer and the dispatch**, for a
    /// route a companion mounts. The same function `/mcp`, `/a2a` and `/gateway/llm/call` call:
    /// inert unless an evaluator is attached (`with_action_evaluator`); with one, the envelope is
    /// built from `caller` (the auth layer's principal and granted scopes, never the request body),
    /// `operation` (`llm.invoke`, …), `resource` (`prompt:llm/{model}@{provider}`, …) and the
    /// `arguments` the policy declared, a presented mandate (`params._meta.mandate`) is assessed,
    /// and the decision is recorded under `enforcement_point` (name your route:
    /// `gateway:reason/route`). A refusal is answered to the client and nothing is dispatched; a
    /// permit is followed by the dispatch (`ServiceHandle::rpc_call_as`) and then
    /// [`gateway_record_execution`](Self::gateway_record_execution).
    ///
    /// A companion door that dispatches a protected kind without this walks around the evaluator
    /// — the enforcement point that can be bypassed by choosing a different door is not one.
    #[allow(clippy::too_many_arguments)]
    pub async fn gateway_preflight(
        &self,
        caller: Option<&ResolvedPrincipal>,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        params: &serde_json::Value,
        enforcement_point: &str,
    ) -> GatewayPreflight {
        #[cfg(feature = "tls")]
        {
            GatewayPreflight {
                inner: ae_preflight(&self.task_ctx, caller, operation, resource, arguments, params, enforcement_point).await,
            }
        }
        #[cfg(not(feature = "tls"))]
        {
            let _ = (caller, operation, resource, arguments, params, enforcement_point);
            GatewayPreflight {}
        }
    }

    /// [`gateway_preflight`](Self::gateway_preflight) for a door that **routes** (post-360 row R):
    /// `route` names the destination this decision is about — its name and [`Locality`](crate::Locality)
    /// — the request's [`DataClass`](crate::DataClass) when the door classifies, and the destinations
    /// already refused for this call. The evaluator sees the destination and the class in the
    /// envelope; the evidence record carries all of it. A router calls this once per candidate, in its
    /// own order, and dispatches to the first the policy permits — so a classified request reaches a
    /// permitted model when one exists and is refused, naming why, when none does.
    ///
    /// What the door puts here is what it **established**, never what the request body says: the
    /// locality from the provider's own declaration, the class from the door's classification (a
    /// client may raise a class its credential carries, never lower it). Inert, like
    /// `gateway_preflight`, unless an evaluator is attached.
    #[allow(clippy::too_many_arguments)]
    pub async fn gateway_preflight_routed(
        &self,
        caller: Option<&ResolvedPrincipal>,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        params: &serde_json::Value,
        enforcement_point: &str,
        route: &crate::agent::route_facts::RouteFacts,
    ) -> GatewayPreflight {
        #[cfg(feature = "tls")]
        {
            GatewayPreflight {
                inner: ae_preflight_routed(
                    &self.task_ctx,
                    caller,
                    operation,
                    resource,
                    arguments,
                    params,
                    enforcement_point,
                    Some(route),
                )
                .await,
            }
        }
        #[cfg(not(feature = "tls"))]
        {
            let _ = (caller, operation, resource, arguments, params, enforcement_point, route);
            GatewayPreflight {}
        }
    }

    /// Record what became of a dispatch [`gateway_preflight`](Self::gateway_preflight) permitted:
    /// a reply is `completed` (or `failed`, when it is a JSON object with an `error`), a refusal
    /// before sending is `none`, a timeout or transport error is `unknown` — never a negative.
    /// Nothing is recorded for an inert or refused preflight.
    pub async fn gateway_record_execution(
        &self,
        preflight: &GatewayPreflight,
        dispatched: &Result<Bytes, GatewayDispatchError>,
    ) {
        #[cfg(feature = "tls")]
        {
            use super::action_evaluator::Execution;
            let mut observed = observed_execution(dispatched);
            if observed == Execution::Completed && dispatched.as_ref().is_ok_and(|b| reply_reports_failure(b)) {
                observed = Execution::Failed;
            }
            ae_record_execution(&self.task_ctx, &preflight.inner, observed).await;
        }
        #[cfg(not(feature = "tls"))]
        {
            let _ = (preflight, dispatched);
        }
    }
}

/// The two LLM doors — the ones `refuse_protected_kind` sends `llm.invoke` to.
#[cfg(all(feature = "gateway", feature = "tls", feature = "llm"))]
pub(crate) const ENFORCEMENT_POINT_LLM_CALL: &str = "gateway:llm/call";
#[cfg(all(feature = "gateway", feature = "tls", feature = "llm"))]
pub(crate) const ENFORCEMENT_POINT_LLM_STREAM: &str = "gateway:llm/stream";

/// The A2A route. Gated on `a2a` as well: it is referenced only from that module, and an item
/// alive in a build that never uses it is the feature-gated dead-code trap CI checks for.
#[cfg(all(feature = "gateway", feature = "tls", feature = "a2a"))]
pub(crate) const ENFORCEMENT_POINT_A2A: &str = "gateway:a2a";

/// The outbound federated-call route (item 2 row 11). Named separately from `gateway:a2a` because
/// it *is* a different point: `/a2a` is where a partner's call arrives, this is where ours leaves.
/// An operator reading evidence has to be able to tell the two directions apart.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) const ENFORCEMENT_POINT_FEDERATION: &str = "gateway:federation/call";

/// Assemble an [`ActionEnvelope`](super::action_evaluator::ActionEnvelope) from facts this
/// gateway verified and run the evaluator over it. `Some(refusal)` means do not dispatch.
///
/// Identity note: `operation_id` is a **correlation** identity, not authority, so a client may
/// supply it (`params._meta.operation_id`) to make its own retries idempotent downstream — item 1's
/// rule. Everything that *is* authority — the actor, the granted scopes — comes from the auth layer
/// and never from the request body (item 7). The `attempt_id` is minted per dispatch here.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) async fn ae_preflight(
    ctx: &Arc<TaskCtx>,
    caller: Option<&ResolvedPrincipal>,
    operation: &str,
    resource: &str,
    arguments: &serde_json::Value,
    params: &serde_json::Value,
    enforcement_point: &str,
) -> Preflight {
    ae_preflight_routed(ctx, caller, operation, resource, arguments, params, enforcement_point, None).await
}

/// [`ae_preflight`] for a **routing** door (post-360 row R): `route` is what the door established
/// about the destination it is deciding on and the request's data class. Both enter the envelope, so
/// the evaluator decides on them; all of it, the destinations already refused for this call
/// included, enters the evidence record. `None` is [`ae_preflight`] exactly. A routed call takes part in
/// fleet counting (row K2) as any other: it is decided by [`ae_decide`] and settled by [`Settling::finish`].
#[cfg(all(feature = "gateway", feature = "tls"))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ae_preflight_routed(
    ctx: &Arc<TaskCtx>,
    caller: Option<&ResolvedPrincipal>,
    operation: &str,
    resource: &str,
    arguments: &serde_json::Value,
    params: &serde_json::Value,
    enforcement_point: &str,
    route: Option<&crate::agent::route_facts::RouteFacts>,
) -> Preflight {
    match ae_decide(ctx, caller, operation, resource, arguments, params, enforcement_point, route).await {
        Decided::Done(p) => p,
        Decided::Settle(s) => s.finish().await,
    }
}

/// What [`ae_decide`] came to: a preflight already complete, or a decision still to be **settled** — its seat claims
/// and its record (post-360 row K2). [`Settling::will_claim`] says whether settling may wait on a consensus round,
/// so an enforcement point that must not stall — a provider's serve loop — can settle it beside its loop instead.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) enum Decided {
    /// Complete: nothing is left to settle.
    Done(Preflight),
    /// Decided, the per-actor gate released; the claims and the record are still to come.
    Settle(Box<Settling>),
}

/// A decision whose seat claims and record are still to come (see [`Decided`]). Owns everything it needs, so it can
/// be settled on another task.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) struct Settling {
    ctx: Arc<TaskCtx>,
    envelope: super::action_evaluator::ActionEnvelope,
    counters: Vec<super::action_evaluator::FleetCounter>,
    outcome: Result<Option<super::action_evaluator::Decision>, super::action_evaluator::PreflightRefusal>,
    history: Option<super::action_evaluator::History>,
    admitted: bool,
    point: String,
    enforcement_point: String,
    /// Row R: what a routing door established about the destination, for the evidence record.
    route: Option<crate::agent::route_facts::RouteFacts>,
}

/// The decision half of [`ae_preflight`], from the real envelope: everything up to and including the release of the
/// per-actor history gate. What is left — seat claims and the record — is a [`Settling`].
#[cfg(all(feature = "gateway", feature = "tls"))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ae_decide(
    ctx: &Arc<TaskCtx>,
    caller: Option<&ResolvedPrincipal>,
    operation: &str,
    resource: &str,
    arguments: &serde_json::Value,
    params: &serde_json::Value,
    enforcement_point: &str,
    route: Option<&crate::agent::route_facts::RouteFacts>,
) -> Decided {
    use super::action_evaluator as ae;
    // Adopter-surface X4: the request's causal fields, read from its own payload (`params._meta`, beside
    // `operation_id` and `mandate`) and nowhere else. Checked before the evaluator question so a malformed field is
    // refused at every door, an inert gateway's included — never corrected, never dropped.
    let causal = match crate::agent::causal::causal_fields(params) {
        Ok(c) => c,
        Err(e) => {
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_ae_preflight_refusals_total", "reason" => "request_malformed").increment(1);
            return Decided::Done(Preflight::Refuse(ae::PreflightRefusal::Malformed(ae::Decision::indeterminate(e.to_string(), ""))));
        }
    };
    let Some(evaluator) = ctx.action_evaluator.get() else { return Decided::Done(Preflight::Inert) };

    // Both evaluator questions asked behind the unwind boundary (AE0 §3): a panicking adapter
    // cannot be used to build the envelope either.
    let Some((wanted, mapping, history_request)) = ae::evaluator_facts(evaluator, operation, resource) else {
        warn!(%operation, %resource, "AE preflight: evaluator panicked while reporting its facts");
        return Decided::Done(Preflight::Refuse(ae::PreflightRefusal::NotEstablished(
            ae::Decision::indeterminate("evaluator panicked while reporting its facts", ""),
        )));
    };

    // Only the argument names the policy declared cross into the envelope — and thence into the
    // evidence record. Everything else stays in the request (AE0 §5).
    let mut selected = serde_json::Map::new();
    if let Some(obj) = arguments.as_object() {
        for name in &wanted {
            if let Some(v) = obj.get(name) {
                selected.insert(name.clone(), v.clone());
            }
        }
    }

    let operation_id = params["_meta"]["operation_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("gw:{}/{:016x}", ctx.node_id, fastrand::u64(..)));
    let attempt_id = format!("{operation_id}/{:08x}", fastrand::u32(..));
    let (actor, scopes) = match caller {
        Some(c) => (c.principal.clone(), c.scopes.clone()),
        None => (gateway_caller::PRINCIPAL_ANONYMOUS.to_string(), Vec::new()),
    };
    // Canonical arguments: serde_json's object serialization is key-ordered for `Map` in its
    // default (BTreeMap) configuration, so the digest is stable for equal arguments.
    let canonical = serde_json::to_vec(arguments).unwrap_or_default();
    // `decision_now_ms`, not `current()`: this value decides whether a mandate has expired and
    // whether a checkpoint is stale. `current()` never reads the wall clock, so on a node whose
    // gossip has gone quiet — the partition case — it freezes and both checks fail OPEN.
    let now_ms = ctx.hlc.decision_now_ms();
    let arguments_digest = ae::arguments_digest(&canonical);

    // Boundary H A1: with an execution authority attached, a presented grant
    // (`params._meta.mandate`) becomes a real mandate finding for this call — checked against the
    // **authenticated** caller, P2's grant checks and A1's execution gate — instead of `None`.
    let assessment = match ctx.execution_authority.get() {
        Some(authority) => {
            let presented: Option<super::gateway_authority::PresentedMandate> =
                serde_json::from_value(params["_meta"]["mandate"].clone()).ok();
            // A presented grant's checkpoint, from the KV, before it is assessed (adopter-surface X3, second PR).
            if let Some(p) = &presented {
                super::gateway_authority::feed_grant_checkpoint(ctx, authority, &p.grant.mandate, now_ms);
            }
            authority.assess(
                &actor,
                operation,
                resource,
                &arguments_digest,
                presented.as_ref(),
                now_ms,
                &super::gateway_member_keys(ctx),
            )
        }
        None => super::gateway_authority::Assessment { binding: None, valid_until_ms: None },
    };
    // A1 rule 3: the envelope can never outlive the mandate it acts under.
    let not_after_ms = match assessment.valid_until_ms {
        Some(until) => now_ms.saturating_add(60_000).min(until),
        None => now_ms.saturating_add(60_000),
    };

    let envelope = ae::ActionEnvelope {
        operation_id,
        attempt_id,
        actor,
        via: ctx.node_id.clone(),
        scopes,
        operation: operation.to_string(),
        resource: resource.to_string(),
        arguments_digest,
        selected_arguments: selected,
        mapping,
        // The expected policy revision comes from the deployment report an operator filed, via
        // `GossipAgent::set_deployed_policy_revision` (AE0 §6). Unset, the seam has no second
        // opinion and the stale-policy check does not fire — which is why leaving it unset means a
        // gateway running a superseded policy cannot be detected.
        expected_policy_revision: ctx.deployed_policy_revision.load_full().map(|r| (*r).clone()),
        issued_at_ms: now_ms,
        not_after_ms,
        // AE1 + Boundary H A1: `None` when no execution authority is attached or no grant was
        // presented — *claims none*, which a rule requiring a mandate reads as **authority not
        // established** (`Indeterminate`), never a denial. With an authority, the finding above.
        mandate: assessment.binding,
        // Row R: established by the routing door, never read from the request body.
        destination: route.map(|r| r.destination.clone()),
        data_class: route.and_then(|r| r.data_class.clone()),
        // X4: from the request's own payload, above.
        correlation: causal.correlation,
        parent: causal.parent,
    };

    // Decide, then record, *then* dispatch. Both outcomes are sealed: a permit with no record of
    // why is precisely the gap this slice exists to close, so recording only refusals would leave
    // the interesting half invisible — and an evidence stream that omits its permits cannot support
    // any statement about what an agent was allowed to do.
    // Post-360 row K: an evaluator that asks for the actor's history gets this point's — or is told none could
    // be established. A permit decided on a view is counted into the history only if the history is still the
    // one it was decided on, so concurrent calls by one actor cannot both slip under a limit; otherwise decide
    // again on a fresh view (twice), then refuse as not established.
    //
    // Row K2: the fleet counters the evaluator names for this envelope are read into the view beside it, and a
    // permit claims a seat in each consensus counter before it is admitted.
    let counters = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| evaluator.fleet_counters(&envelope))) {
        Ok(c) => c,
        Err(_) => {
            warn!(%operation, %resource, "AE preflight: evaluator panicked while naming its fleet counters");
            return Decided::Done(Preflight::Refuse(ae::PreflightRefusal::NotEstablished(ae::Decision::indeterminate(
                "evaluator panicked while naming its fleet counters",
                envelope.expected_policy_revision.clone().unwrap_or_default(),
            ))));
        }
    };
    // A fleet counter is history too: an evaluator naming one gets a view even if it asked for no window of its own.
    let history_request = history_request.or_else(|| {
        counters.iter().map(|c| c.window_ms).max().map(|window_ms| ae::HistoryRequest { window_ms, max_entries: 0 })
    });
    let history_index = ctx.history_index.get();
    let point = super::evidence_journal::point_of(enforcement_point);
    if let (Some(_), Some(index)) = (history_request, history_index) {
        index.start_loading(now_ms); // a no-op once started; normally started when the evaluator was attached
    }
    // One actor's decisions on its history, one after another: the view, the evaluator and the count are one step.
    let _gate = match (history_request, history_index) {
        (Some(_), Some(index)) => Some(index.gate(&envelope.actor).lock().await),
        _ => None,
    };
    // The clock the history is read at is read *under* the gate: a call that read its clock before another
    // decided would otherwise count from an earlier instant than the permits admitted meanwhile.
    let history_now = ctx.hlc.decision_now_ms().max(now_ms);
    let mut tries = 0;
    let (outcome, history, admitted) = loop {
        let (history, version) = match (history_request, history_index) {
            (None, _) => (None, None),
            (Some(request), Some(index)) => {
                let (mut h, v) = index.view(&point, &envelope.actor, request, history_now);
                if let ae::History::View(view) = &mut h {
                    view.fleet = super::fleet_history::establish(ctx, &counters, &point, &envelope.actor, history_now);
                }
                (Some(h), Some(v))
            }
            (Some(_), None) => (
                Some(ae::History::Unavailable(
                    "no evidence journal is attached, so this enforcement point keeps no history".into(),
                )),
                None,
            ),
        };
        let outcome = ae::preflight_with_history(Some(evaluator), &envelope, history.as_ref(), history_now);
        // Adopter-surface X3: a grant binds the policy revision it was approved under; a permit decided under another
        // is refused **here, before it is admitted into the history or claims a seat** (#673's review, finding 2).
        let outcome = match outcome {
            Ok(Some(d)) => match envelope.mandate.as_ref().and_then(|b| b.bound_policy_revision.as_deref()) {
                Some(bound) if bound != d.policy_revision => Err(ae::PreflightRefusal::NotEstablished(
                    ae::Decision::indeterminate(
                        crate::mandate::MandateRefusal::BoundToOtherPolicy { bound: bound.to_string(), decided: d.policy_revision.clone() }.to_string(),
                        d.policy_revision.clone(),
                    )
                    .checking(["grant bound to its policy revision"]),
                )),
                _ => Ok(Some(d)),
            },
            other => other,
        };
        let (Ok(Some(_)), Some(index), Some(version)) = (&outcome, history_index, version) else {
            break (outcome, history, false);
        };
        let entry = ae::HistoryEntry {
            at_ms: history_now,
            attempt_id: envelope.attempt_id.clone(),
            operation: envelope.operation.clone(),
            resource: envelope.resource.clone(),
            decision: ae::DecisionKind::Permit,
            execution: ae::Execution::Attempted,
        };
        if index.admit(&point, &envelope.actor, version, entry) {
            break (outcome, history, true);
        }
        tries += 1;
        if tries >= 3 {
            let refused = ae::Decision::indeterminate(
                "the actor's history kept changing while this decision was made on it",
                envelope.expected_policy_revision.clone().unwrap_or_default(),
            );
            break (Err(ae::PreflightRefusal::NotEstablished(refused)), history, false);
        }
    };
    // Row K2's tally: a permit admitted under a tally rule is published under the actor's gate (a KV read and
    // write), so two permits of one actor at this point do not lose each other's update.
    if let Ok(Some(_)) = &outcome {
        for counter in counters.iter().filter(|c| c.scope == ae::HistoryScope::FleetTally) {
            super::fleet_history::publish_permit(ctx, counter, &point, &envelope.actor, history_now);
        }
    }
    // Counted: the next decision on this actor's history may proceed — before any seat claim, which can wait on a
    // quorum for seconds and must hold up no other call on this stripe (#642's review, finding 1). The permit is
    // counted locally already, provisionally: a claim that fails takes it back.
    drop(_gate);
    Decided::Settle(Box::new(Settling {
        ctx: Arc::clone(ctx),
        envelope,
        counters,
        outcome,
        history,
        admitted,
        point,
        enforcement_point: enforcement_point.to_string(),
        route: route.cloned(),
    }))
}

/// Test hook: a pause, per node, after the evidence journal acknowledges a record and before the history observes it.
#[cfg(all(test, feature = "gateway", feature = "tls"))]
pub(crate) static TEST_RECORD_PAUSE_MS: std::sync::LazyLock<papaya::HashMap<String, u64>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// Takes back a provisionally counted permit if the settling it belongs to is dropped before its decision is settled.
#[cfg(all(feature = "gateway", feature = "tls"))]
struct RetractOnDrop {
    armed: bool,
    ctx: Arc<TaskCtx>,
    point: String,
    actor: String,
    attempt: String,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl Drop for RetractOnDrop {
    fn drop(&mut self) {
        if self.armed && let Some(index) = self.ctx.history_index.get() {
            index.retract(&self.point, &self.actor, &self.attempt);
        }
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl Settling {
    /// Whether settling may wait on a consensus round: a permit relying on a fleet-consensus counter.
    pub(crate) fn will_claim(&self) -> bool {
        matches!(self.outcome, Ok(Some(_)))
            && self.counters.iter().any(|c| c.scope == super::action_evaluator::HistoryScope::FleetConsensus)
    }

    /// Claim the permit's seats and record the decision: the rest of [`ae_preflight`].
    pub(crate) async fn finish(self) -> Preflight {
        use super::action_evaluator as ae;
        let Settling { ctx, envelope, counters, mut outcome, history, admitted, point, enforcement_point, route } = self;
        let ctx = &ctx;
        let enforcement_point = enforcement_point.as_str();
        let (operation, resource) = (envelope.operation.clone(), envelope.resource.clone());
        let history_index = ctx.history_index.get();
        // Dropped before it is recorded — its serve loop abandoned it (#642's review, round 3, finding 4) — a permit
        // counted provisionally is taken back, as a refused one is.
        let mut provisional = RetractOnDrop {
            armed: admitted,
            ctx: Arc::clone(ctx),
            point: point.clone(),
            actor: envelope.actor.clone(),
            attempt: envelope.attempt_id.clone(),
        };

        // Row K2's check-then-act: a permit relying on a consensus counter holds a seat before it is admitted — the
        // seat, not the evaluator's reading of the counter, is what bounds the fleet, and consensus serialises the
        // seat's contenders, so no local gate is needed across the claim. A claim that fails takes the permit back:
        // every seat held is a denial; an undecidable or too-late claim is not established.
        let mut seats = Vec::new();
        // The decisions behind the seats, with their windows: released a window after this permit is admitted (M1).
        let mut to_release = Vec::new();
        if let Ok(Some(permit)) = &outcome {
            let mut refused = None;
            for counter in counters.iter().filter(|c| c.scope == ae::HistoryScope::FleetConsensus) {
                let scope = counter.scope.label();
                let line = format!("history scope={scope} counter={} limit={} window_ms={}", counter.key, counter.count, counter.window_ms);
                match super::fleet_history::claim_seat(ctx, counter, &point, &envelope.actor, &envelope.attempt_id).await {
                    super::fleet_history::Claim::Seat(seat, env) => {
                        to_release.push((seat.slot.clone(), env, counter.window_ms));
                        seats.push(seat);
                    }
                    super::fleet_history::Claim::Full => {
                        refused = Some(ae::PreflightRefusal::Denied(
                            ae::Decision::deny(
                                format!(
                                    "the allowance permits at most {} in {} ms and every seat of the fleet-wide counter \
                                     is held (scope {scope})",
                                    counter.count, counter.window_ms
                                ),
                                permit.policy_revision.clone(),
                            )
                            .checking([line, format!("seat claim: all {} seats held", counter.count)]),
                        ));
                        break;
                    }
                    super::fleet_history::Claim::NotEstablished(why) => {
                        refused = Some(ae::PreflightRefusal::NotEstablished(
                            ae::Decision::indeterminate(
                                format!("the fleet-wide count at scope {scope} could not be claimed, so the limit is not established"),
                                permit.policy_revision.clone(),
                            )
                            .checking([line])
                            .with_errors([format!("seat claim not established at scope {scope}: {why}")]),
                        ));
                        break;
                    }
                }
            }
            if let Some(refusal) = refused {
                if admitted && let Some(index) = history_index {
                    index.retract(&point, &envelope.actor, &envelope.attempt_id);
                }
                outcome = Err(refusal);
            }
        }
        // Adopter-surface X3 (second PR): a grant admits **one call at each door**. Claimed here — once per call,
        // after the permit's history and seats and before its record — not inside the history loop, whose retry would
        // meet its own marker (#678's review, finding 2). The authority's map is the atomic half (two concurrent
        // presentations meet there), the marker under the request (`…/used/{door}`, on the WAL) the durable half,
        // read first so a restart does not forget a use. A used grant **denies**, naming the door, and gives back the
        // history permit; a grant admitted here is spent at this door whether or not the work completes, as a nonce
        // is — a refusal earlier in this settling (a full fleet counter) spends nothing.
        let grant_use = match (&outcome, &envelope.mandate, ctx.execution_authority.get()) {
            (Ok(Some(permit)), Some(b), Some(authority))
                if b.scope.starts_with(crate::mandate::grant::GRANT_SCOPE_PREFIX) && b.state == ae::MandateState::Established =>
            {
                let marker = format!("grant/{}/{}/used/{point}", b.holder.as_str(), b.term.as_str());
                let used = mycelium_core::ops::kv_get(ctx, &marker).is_some()
                    || !authority.claim_grant_use(b.holder.as_str(), &b.scope, b.term.as_str(), &point, envelope.not_after_ms);
                Some((marker, used, permit.policy_revision.clone()))
            }
            _ => None,
        };
        match grant_use {
            Some((_, true, policy_revision)) => {
                if admitted && let Some(index) = history_index {
                    index.retract(&point, &envelope.actor, &envelope.attempt_id);
                }
                outcome = Err(ae::PreflightRefusal::Denied(
                    ae::Decision::deny(crate::mandate::MandateRefusal::GrantUsed { door: point.clone() }.to_string(), policy_revision)
                        .checking(["grant used once at each door"]),
                ));
            }
            Some((marker, false, _)) => {
                super::super::state_machine::write_owned_kv(ctx, &marker, ctx.hlc.decision_now_ms().to_string().into_bytes());
            }
            None => {}
        }
        let (decision, execution) = match &outcome {
            Ok(Some(d)) => (d.clone(), ae::Execution::Attempted),
            // No evaluator — unreachable here, one was fetched above. Nothing decided, nothing to
            // record, nothing to refuse.
            Ok(None) => return Preflight::Inert,
            Err(refusal) => (refusal.decision().clone(), ae::Execution::None),
        };

        // Every decision, permits included — the refusal counter below has no denominator on its own,
        // so a denial *rate* is not computable from it and an alert on denial volume fires on traffic
        // growth. Labels are the evidence document's own vocabulary (`DecisionKind`/`MappingKind`), so
        // a reader never has to translate between a dashboard and a record. Both label sets are small
        // closed sets: nothing here is labelled by principal, operation or resource, whose value spaces
        // grow with traffic.
        #[cfg(feature = "metrics")]
        {
            let verdict = ae::DecisionKind::from(decision.verdict).label();
            let mapping = ae::MappingKind::from(envelope.mapping.status).label();
            metrics::counter!("mycelium_ae_decisions_total", "verdict" => verdict, "mapping" => mapping)
                .increment(1);
        }

        let mut evidence = ae::AeEvidence::for_decision(&envelope, &decision, execution, enforcement_point);
        if let Some(h) = &history {
            // The history this decision read, as a decision input: a replay finds the attempts in the journal. With
            // a fleet scope, the counters' state and coverage, and the seats the permit claimed (row K2).
            evidence = evidence.with_history(ae::HistoryInput::of(h).with_seats(seats));
        }
        if let Some(r) = route {
            // Row R: the class, the destination decided on and the destinations already refused.
            evidence = evidence.with_route(r);
        }
        // Disarmed **before** the record is awaited (#642's review, round 4, finding 3): once the record is handed to
        // the journal, an abort leaves its delivery unknown — the record may be on disk — so the permit stays counted
        // rather than being taken back from under a record that exists. The retractions below are the only ones.
        provisional.armed = false;
        let recorded = ae_record(ctx, &evidence).await;
        let journal_sha256 = match recorded {
            Ok(hash) => hash,
            Err(e) => {
            // Not recorded, so not done: a permit counted into the history is taken back.
            if admitted && let Some(index) = history_index {
                index.retract(&point, &envelope.actor, &envelope.attempt_id);
            }
            warn!(actor = %envelope.actor, %operation, %resource,
                  "AE preflight: the decision could not be recorded, so the action is refused: {e}");
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_ae_preflight_refusals_total", "reason" => "evidence_not_recorded")
                .increment(1);
            return Preflight::Refuse(ae::PreflightRefusal::NotRecorded(decision));
            }
        };

        match outcome {
            Ok(_) => {
                // Admitted: each seat is held for a window from now, then released (the lease is the crash fallback).
                for (slot, env, window_ms) in to_release {
                    super::fleet_history::release_after_window(ctx, slot, env, window_ms);
                }
                Preflight::Proceed { evidence: Box::new(evidence), journal_sha256 }
            }
            Err(refusal) => {
                warn!(actor = %envelope.actor, %operation, %resource,
                      "AE preflight refused: {refusal}");
                #[cfg(feature = "metrics")]
                metrics::counter!("mycelium_ae_preflight_refusals_total", "reason" => refusal.reason()).increment(1);
                Preflight::Refuse(refusal)
            }
        }
    }
}

/// What this gateway **observed** of a dispatch, in §5's vocabulary.
///
/// The rule that matters is the last arm. A timeout — or any transport error — leaves the call's
/// fate genuinely open: the provider may have run it and failed to answer. Reporting `Failed` there
/// would claim knowledge nobody has, and a consumer reading `failed` will act on it. So the honest
/// answer is `Unknown`, which item 1 names `DeliveryUnknown` and which this project has as a hot
/// invariant: *a timeout is never a negative*.
///
/// The one case where "nothing ran" can be stated is a dispatch this gateway refused before sending.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn observed_execution(
    dispatched: &Result<Bytes, GatewayDispatchError>,
) -> super::action_evaluator::Execution {
    use super::action_evaluator::Execution;
    match dispatched {
        Ok(_) => Execution::Completed,
        Err(GatewayDispatchError::ProviderWithoutContext(_))
        | Err(GatewayDispatchError::ContextTooLarge) => Execution::None,
        Err(_) => Execution::Unknown,
    }
}

/// Did the provider's JSON-RPC reply carry an error?
///
/// A reply that arrived is a *completed* RPC; whether the tool inside it succeeded is a different
/// question, and the consumer's `effect` reads `failed` from this. Unparseable bytes count as failed:
/// something answered, and it was not an answer.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn reply_reports_failure(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .map(|v| v.get("error").is_some())
        .unwrap_or(true)
}

/// Record what became of a dispatch this gateway permitted (AE0 §5's execution record).
///
/// **Why this is not optional.** Without it every permitted call exports as `effect: unknown`, even
/// when this gateway watched the provider answer — so the evidence could say what an agent was
/// *allowed* to do and never what it *did*, which is most of what anyone wants to know.
///
/// The record is appended beside the decision, never over it: `operation_id`, `attempt_id` and the
/// principal are carried through unchanged so a consumer can correlate them, and the decision stands
/// exactly as written.
///
/// A failure to record here does **not** retract the dispatch — it already happened, and pretending
/// otherwise would be the one lie worse than silence. It is logged, and the reference record carries
/// the journal's own state.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) async fn ae_record_execution(
    ctx: &Arc<TaskCtx>,
    preflight: &Preflight,
    execution: super::action_evaluator::Execution,
) {
    let Preflight::Proceed { evidence: decided, .. } = preflight else { return };
    if let Err(e) = ae_record(ctx, &decided.as_execution(execution)).await {
        // The effect has happened. The evidence for it has not.
        warn!(
            operation_id = %decided.operation_id,
            attempt_id = %decided.attempt_id,
            "AE: the execution record could not be established: {e}"
        );
    }
}

/// Record one AE decision: **journal first, then a reference into the chain** (AE0 §5).
///
/// The decision document goes to the node-local [journal](super::evidence_journal) — durable,
/// append-only, never gossiped. What is sealed into the tamper-evident chain is an
/// [`AeReference`](super::action_evaluator::AeReference): the verdict, the identities, the policy
/// revision, the catalogue id, and the journal record's **content hash**. Nothing else, because the
/// chain reaches every node and the evidence is not for every node.
///
/// `Err` means *refuse the dispatch*. Which failures refuse is the operator's
/// [`EvidenceProfile`](super::evidence_journal::EvidenceProfile): the strict profile gates the
/// effect on durable evidence, the lenient one proceeds and the reference record says it did. `Ok` carries
/// the record's content hash when the journal established it (X4: the `parent` of the work this decision starts).
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(super) async fn ae_record(
    ctx: &Arc<TaskCtx>,
    evidence: &super::action_evaluator::AeEvidence,
) -> Result<Option<String>, String> {
    use super::action_evaluator::{AeReference, EvidenceState};
    use super::evidence_journal::{EvidenceProfile, JournalError};

    let bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;

    let (journal_sha256, state, refuse) = match ctx.evidence_journal.get() {
        // No journal: nothing is recorded, and the reference says exactly that rather than being
        // absent. `with_action_evaluator` warns at attach time so this is never a surprise.
        None => (None, EvidenceState::NotConfigured, None),
        Some(journal) => match journal.append(bytes).await {
            Ok(appended) => {
                #[cfg(test)]
                {
                    // A pause between the journal's acknowledgement and the history's update — where an abort lands
                    // after the record is on disk (#642's review, round 4, finding 3).
                    let pause = TEST_RECORD_PAUSE_MS.pin().get(&ctx.node_id.to_string()).copied();
                    if let Some(ms) = pause {
                        mycelium_core::sim_seam::sleep_ms("ae/record-test-pause", ms).await;
                    }
                }
                // On the record, so in the actor's history this point keeps (post-360 row K).
                if let Some(index) = ctx.history_index.get() {
                    index.observe(evidence);
                }
                (Some(super::action_evaluator::hex32(&appended.content_hash)), EvidenceState::OnDisk, None)
            }
            Err(e) => {
                // A lost acknowledgement is *unknown*, not failed: the record may be on disk, and
                // claiming failure would assert knowledge nobody has.
                let state = match e {
                    JournalError::DeliveryUnknown => EvidenceState::Unknown,
                    _ => EvidenceState::NotEstablished,
                };
                let refuse =
                    matches!(journal.profile(), EvidenceProfile::Strict).then(|| e.to_string());
                // Lenient proceeds without the record, so after a restart the journal will not hold this
                // decision: no history reaching back past it can be complete (post-360 row K).
                if refuse.is_none()
                    && let Some(index) = ctx.history_index.get()
                {
                    index.note_lost(evidence.at_ms);
                }
                (None, state, refuse)
            }
        },
    };

    ae_seal_reference(ctx, &AeReference::for_evidence(evidence, journal_sha256.clone(), state));
    match refuse {
        Some(why) => Err(why),
        None => Ok(journal_sha256),
    }
}

/// Seal the safe reference record into the tamper-evident chain.
///
/// Best-effort by design: the journal is what gates the effect (AE0 §5 names it, not the chain and
/// not the sink, as the ack-capable contract), and the chain adds ordering and hash-linking on top.
/// A node without a `tls` identity gets a warning, not a refused dispatch — its evidence is still
/// durable, just not chain-linked.
///
/// The audit record's `target` is the **catalogue id**, never the resource: `ae:{catalogue}` is the
/// most specific thing §5 permits to gossip.
#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
pub(super) fn ae_seal_reference(ctx: &Arc<TaskCtx>, reference: &super::action_evaluator::AeReference) {
    use super::action_evaluator::DecisionKind;
    let detail = match serde_json::to_string(reference) {
        Ok(d) => d,
        Err(e) => {
            warn!("AE: the reference record did not serialise: {e}");
            return;
        }
    };
    let outcome = match reference.decision {
        DecisionKind::Permit => super::audit::AuditOutcome::Success,
        DecisionKind::Deny => super::audit::AuditOutcome::Denied,
        DecisionKind::Indeterminate => super::audit::AuditOutcome::Error,
    };
    if let Err(e) = super::audit::seal_and_write(
        ctx,
        super::audit::AuditAction::Invoke,
        reference.principal.clone(),
        format!("ae:{}", reference.catalogue),
        outcome,
        Some(detail),
    ) {
        warn!("AE: the reference record could not be chained: {e}");
    }
}

/// Without `compliance` there is no audit chain to link into. The journal still holds the evidence.
#[cfg(all(feature = "gateway", feature = "tls", not(feature = "compliance")))]
pub(super) fn ae_seal_reference(_ctx: &Arc<TaskCtx>, _reference: &super::action_evaluator::AeReference) {}

/// HTTP shape of a refused gateway dispatch (item 7): `412 Precondition Failed` — the
/// precondition being a provider that enforces the caller context, or a context to send.
pub(super) fn dispatch_refused(e: GatewayDispatchError) -> Response {
    let provider = match &e {
        GatewayDispatchError::ProviderWithoutContext(n) => Some(n.to_string()),
        _ => None,
    };
    let status = match e {
        GatewayDispatchError::ContextTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::PRECONDITION_FAILED,
    };
    (status, Json(json!({ "ok": false, "error": e.reason(), "detail": e.to_string(), "provider": provider })))
        .into_response()
}

/// Top-level KV namespaces the substrate or a companion owns (`src/lib.rs` § KV namespace ownership). The
/// raw KV routes refuse them: each has its own door, guarded by its own scope, and a write through
/// `kv:write` would bypass it — a governance intent without `govern:write`, a prompt template without
/// `llm:write`, another node enrolled in a group (a node joins itself), a forged capability or requirement,
/// a forged mailbox sender, a planted `sys/caller-context/` marker, a deleted acceptor ballot (the reviews
/// of #544; the kv:write decision of 2026-10-07). `every_namespace_in_the_table_is_classified_for_the_raw_kv_routes`
/// keeps this in step with the table.
pub(crate) const OWNED_KV_PREFIXES: &[&str] = &[
    "sys/", "consensus/", "grp/", "audit/", "cap/", "req/", "cap-group/", "gcap/", "mailbox/", "tools/",
    "agent/", "svc/", "log/", "clog/", "lock/", "prompts/", "skills/", "installable/", "comp/", "wiki/", "tuple/",
    "facts/", "mandate/", "knowledge/", "rights/", "cn/",
    "grant/",  // adopter-surface X3: grant requests and answers (POST /gateway/grants/{agent}/{request})
    "faults/", // adopter-surface X6: `mycelium test --faults`'s own writes on the fleet it builds; no route writes it
];

/// Namespaces the table assigns to an application writing through the gateway's KV routes — the LangGraph
/// checkpointer's index rows, the Strands session store's rows (`strands-session-mycelium`), the mesh manifest (application-owned, `mesh_manifest.rs`) and the schema
/// registry (`publish_schema` from Rust; this is a non-Rust application's door). (`agent/{node}/provision/{item}/error`,
/// an application's provisioning report, is the one owned-namespace subtree the raw routes also accept.)
pub(crate) const APPLICATION_KV_PREFIXES: &[&str] = &["ckpt/", "ckptw/", "strands/", "manifest/", "schemas/"];

/// Streams under `log/` that a substrate component or companion owns: the commitment net's records, a wiki's
/// durable proposals, `mycelium-reason`'s traces. The log routes take a stream name, so without this they
/// reach those as well (the review of #549).
pub(crate) const OWNED_LOG_STREAMS: &[&str] = &["cn/", "wiki/", "reason/"];

pub(super) fn refuse_owned_stream(stream: &str) -> Option<axum::response::Response> {
    let owned = OWNED_LOG_STREAMS.iter().any(|p| stream.starts_with(p) || stream == p.trim_end_matches('/'));
    owned.then(|| (
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "protected_stream",
            "message": "this log stream belongs to a component (commitment, wiki, reason) and is written through it" })),
    ).into_response())
}

/// Whether the raw KV routes write `key`: an application key, not one the substrate or a companion owns.
pub(crate) fn raw_kv_writable(key: &str) -> bool {
    if APPLICATION_KV_PREFIXES.iter().any(|p| key.starts_with(p)) {
        return true;
    }
    if let Some(rest) = key.strip_prefix("agent/")
        && let Some((_node, sub)) = rest.split_once('/')
        && sub.starts_with("provision/")
    {
        return true;
    }
    !OWNED_KV_PREFIXES.iter().any(|p| key.starts_with(p) || key == p.trim_end_matches('/'))
}

/// The doors that take a KV key from the request — `POST`/`DELETE /gateway/kv`, `POST /gateway/kv/quorum`, and
/// `POST /gateway/overlay/consistent/set`, which writes the raw key once consensus commits — write application
/// keys only ([`raw_kv_writable`]). An owned key is refused `403` whatever the token's scopes, as
/// [`refuse_protected_kind`] does for RPC kinds, naming the door to use. Layer I still accepts these keys from
/// a peer — detection, not prevention; this is the gateway's door.
pub(super) fn refuse_protected_key(key: &str) -> Option<axum::response::Response> {
    if raw_kv_writable(key) {
        return None;
    }
    warn!(key, "gateway: an owned KV key refused on a raw KV route");
    let door = if key.starts_with("sys/govern/") {
        "governance intents are published through /gateway/govern/{tuning,timing,membership} (govern:write)"
    } else if key.starts_with("sys/topology-override/") {
        "the topology override is set through POST /gateway/govern/topology-override (govern:write)"
    } else if key.starts_with("grp/") {
        "a node joins a group itself: POST /gateway/mesh/group on that node (mesh:write), or POST /gateway/govern/group (govern:write) for a group under a membership intent"
    } else if key.starts_with("prompts/") {
        "prompt templates are written through /gateway/prompts/{ns}/{name} (llm:write)"
    } else if key.starts_with("log/") || key.starts_with("clog/") {
        "logs are appended through /gateway/overlay/log/append (consensus:write)"
    } else if key.starts_with("cap/") {
        "capabilities are advertised through /gateway/capability/advertise (cap:write)"
    } else if key.starts_with("req/") {
        "requirements are declared through POST /gateway/units/declare (cap:write)"
    } else if key.starts_with("mailbox/") {
        "mailbox entries are delivered through POST /gateway/mailbox/deliver (mesh:write), which records the sender"
    } else if key.starts_with("installable/") {
        "catalogue lines are published through POST /gateway/artifacts/publish (artifact:publish, signature-checked) where mycelium-wasm-host's routes are merged, or by a librarian"
    } else if key.starts_with("lock/") {
        "locks are taken through /gateway/overlay/lock/acquire (consensus:write)"
    } else if key.starts_with("grant/") {
        "grant requests are written by the agent's state machine and answered through POST /gateway/grants/{agent}/{request} (grant:issue)"
    } else if key.starts_with("faults/") {
        "`faults/` is written by `mycelium test --faults` on the in-process fleet it builds for one run, never on an adopter's node; no route writes it"
    } else {
        "this namespace is written by the substrate or the companion that owns it, not through the KV routes \
         (src/lib.rs § KV namespace ownership); the KV routes write application keys"
    };
    Some((
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "protected_key", "message": door })),
    ).into_response())
}
