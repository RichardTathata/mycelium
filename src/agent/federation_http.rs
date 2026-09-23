//! The federation edge on the gateway (item 2 PR 8): the auth-layer step that reads the
//! credential header, the catalogue route, and the identity handed on to the A2A handler.
//!
//! The decisions live in `crate::federation::edge`; this module is the HTTP around them. Two
//! rules it enforces that are its own:
//!
//! - **A presented credential is never anonymised.** `/a2a` admits anonymous callers (item 7's
//!   optional auth), but a request that *tried* to present a federation credential and failed —
//!   malformed, untrusted, expired, or presented at a gateway with no edge — is refused, not
//!   treated as one that presented nothing. Otherwise a revoked partner would quietly become an
//!   anonymous one.
//! - **One identity per request.** A bearer and a federation credential on the same request is a
//!   400: the gateway will not choose which of two callers it is dispatching for.
//!
//! # The other direction (item 2 row 11)
//!
//! Everything above is the **provider** side: a partner's call arriving here. The second half of
//! this module is the **consumer** side — the `/gateway/federation/*` routes a *local* client uses
//! to reach a partner, and so the thing the Python and TypeScript SDK verbs talk to. Its decisions
//! live in [`crate::federation::client`]; what is this module's own is the vocabulary a refusal
//! comes back in, and that vocabulary answers two questions rather than one:
//!
//! - **`sent`** — did any byte reach the partner? A local refusal (link down, export not in a
//!   fresh catalogue, no slot, a pin mismatch) sent nothing, and saying so is what lets an
//!   at-most-once caller retry without reasoning about our internals.
//! - **`delivery`** — `none`, `refused`, `completed` or **`unknown`**. `unknown` is not an error
//!   dressed up: it is the hot invariant (*a timeout is `DeliveryUnknown`, never a negative*)
//!   carried to an SDK, and a caller that treats it as failure will double-run effects.

use super::gateway_caller::{federation_principal, ResolvedPrincipal};
use crate::agent::TaskCtx;
use crate::federation::client::{ClientError, FederationClient};
use crate::federation::edge::{now_ms, FederationEdge, PresentedCall, CATALOG_PATH, HEADER_FEDERATED_CALL};
use crate::federation::gateway::CallOutcome;
use crate::federation::session::LinkState;
use crate::federation::DomainId;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Extension, Json, Router,
};
use serde_json::json;
use std::sync::Arc;

/// An authenticated federated caller, inserted into the request by the auth layer. The handler
/// still authorises the export against `presented` — identity is not a grant.
#[derive(Clone, Debug)]
pub(crate) struct FederatedIdentity {
    pub(crate) presented: PresentedCall,
    /// `federation:{origin}/{principal}` — what the provider is told.
    pub(crate) principal: String,
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

/// Read and authenticate the credential header. `Ok(None)`: no header. `Err`: a response that
/// refuses the request — the header was present and could not be accepted.
pub(crate) fn authenticate_presented(ctx: &TaskCtx, headers: &HeaderMap) -> Result<Option<FederatedIdentity>, Box<Response>> {
    let raw = match headers.get(HEADER_FEDERATED_CALL) {
        None => None,
        Some(v) => Some(v.to_str().map_err(|_| Box::new(refuse(StatusCode::BAD_REQUEST, "federation credential header is not ASCII")))?),
    };
    let presented = match PresentedCall::from_header_value(raw) {
        Ok(None) => return Ok(None),
        Ok(Some(p)) => p,
        Err(e) => return Err(Box::new(refuse(StatusCode::BAD_REQUEST, e))),
    };
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(Box::new(refuse(StatusCode::BAD_REQUEST, "present a bearer or a federation credential, not both")));
    }
    let Some(edge) = ctx.federation_edge.get() else {
        return Err(Box::new(refuse(StatusCode::UNAUTHORIZED, "federation is not enabled at this gateway")));
    };
    match edge.authenticate(&presented, now_ms()) {
        Ok(credential) => Ok(Some(FederatedIdentity {
            principal: federation_principal(credential.origin_domain.as_str(), &credential.principal),
            presented,
        })),
        Err(refusal) => Err(Box::new(refuse(StatusCode::UNAUTHORIZED, format!("federation credential refused: {refusal}")))),
    }
}

/// Insert the identity the handler and the dispatch will see.
pub(crate) fn insert_identity(request: &mut Request, identity: FederatedIdentity) {
    request.extensions_mut().insert(ResolvedPrincipal { principal: identity.principal.clone(), scopes: Vec::new() });
    request.extensions_mut().insert(identity);
}

/// The auth layer for `/federation/*`: a credential is required, not optional.
pub(crate) async fn federation_auth(ctx: Arc<TaskCtx>, mut request: Request, next: Next) -> Response {
    match authenticate_presented(&ctx, request.headers()) {
        Ok(Some(identity)) => {
            insert_identity(&mut request, identity);
            next.run(request).await
        }
        Ok(None) => refuse(StatusCode::UNAUTHORIZED, "a federation credential is required"),
        Err(response) => *response,
    }
}

/// `GET /federation/catalog`: the exports the presenting partner has been granted.
async fn catalog_handler(
    State(edge): State<Arc<FederationEdge>>,
    identity: Option<Extension<FederatedIdentity>>,
) -> Response {
    let Some(Extension(identity)) = identity else {
        return refuse(StatusCode::UNAUTHORIZED, "a federation credential is required");
    };
    match edge.catalog_for(&identity.presented, now_ms()) {
        Ok(reply) => Json(reply).into_response(),
        Err(refusal) => refuse(StatusCode::FORBIDDEN, format!("catalogue refused: {refusal}")),
    }
}

pub(crate) fn federation_router(edge: Arc<FederationEdge>) -> Router {
    Router::new().route(CATALOG_PATH, get(catalog_handler)).with_state(edge)
}

// ── The consumer side: `/gateway/federation/*` (item 2 row 11) ────────────────

/// How a link reads to an operator and an SDK. Lower-case and stable, because it is a wire value:
/// `Debug` formatting would make renaming a Rust variant a breaking change to every SDK.
pub(crate) fn link_word(state: LinkState) -> &'static str {
    match state {
        LinkState::Down => "down",
        LinkState::Refreshing => "refreshing",
        LinkState::Ready => "ready",
    }
}

/// The client configured for `domain`, or a response saying why there is none.
///
/// Two distinct refusals, kept apart on purpose: a gateway with **no** consumer side at all is a
/// deployment that never configured one, and a gateway that has clients but not *this* partner is a
/// name the operator got wrong. Folding them into one 404 would make the second look like the first.
pub(crate) fn client_for(ctx: &TaskCtx, domain: &str) -> Result<Arc<FederationClient>, Box<Response>> {
    let Some(clients) = ctx.federation_clients.get() else {
        return Err(Box::new(refuse(StatusCode::NOT_FOUND, "this gateway has no federation consumer configured")));
    };
    if DomainId::new(domain).is_err() {
        return Err(Box::new(refuse(StatusCode::BAD_REQUEST, format!("{domain:?} is not a domain id"))));
    }
    clients
        .iter()
        .find(|c| c.partner().as_str() == domain)
        .cloned()
        .ok_or_else(|| Box::new(refuse(StatusCode::NOT_FOUND, format!("no federation client is configured for domain {domain}"))))
}

/// `GET /gateway/federation/domain` — what this node *is*, federally: the edge's identity, what it
/// exports and the policy revision those exports are filtered by. `configured: false` when no edge
/// is attached, rather than a 404: "this gateway serves no domain" is an answer, not a missing page.
pub(crate) fn domain_json(ctx: &TaskCtx) -> Response {
    match ctx.federation_edge.get() {
        None => Json(json!({ "configured": false })).into_response(),
        Some(edge) => Json(json!({
            "configured": true,
            "domain": edge.domain().as_str(),
            "exports": edge.exports(),
            "policy_revision": edge.policy_revision(),
            "signs_catalogue": edge.signs_catalogue(),
        }))
        .into_response(),
    }
}

/// `GET /gateway/federation/partners` — one row per configured partner: the link state and the
/// exports the **last** successful discovery was granted.
///
/// `last_catalogue` is remembered, not fresh, and the field name says so. Whether an export may be
/// called now is the resolver's freshness rule at call time — this row is what an operator looks at,
/// never what a caller should branch on.
pub(crate) fn partners_json(ctx: &TaskCtx) -> Response {
    let rows: Vec<serde_json::Value> = ctx
        .federation_clients
        .get()
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    json!({
                        "domain": c.partner().as_str(),
                        "link": link_word(c.link_state()),
                        "last_catalogue": c.last_catalogue(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!({ "partners": rows })).into_response()
}

/// The refusal vocabulary, and the two fields that make it usable: see the module docs.
///
/// **No `_` arm, deliberately.** [`ClientError`] is `#[non_exhaustive]` to *other* crates, but in
/// this one the compiler still demands every variant — so a new refusal stops the build here, at
/// the place that has to decide what it means for `sent` and `delivery`, rather than being folded
/// into a default. The fail-closed rule that a `_` arm would carry is instead applied to each
/// value: where this gateway cannot establish that nothing ran, it says `unknown`, because the one
/// thing worse than a caller that cannot retry is a caller that retries an effect which already
/// happened.
pub(crate) fn call_refusal(e: &ClientError) -> Response {
    let (status, kind, sent, delivery) = match e {
        // Refused here, before any byte left this process.
        ClientError::Link(_) => (StatusCode::CONFLICT, "link", false, "none"),
        ClientError::Resolve(_) => (StatusCode::CONFLICT, "resolve", false, "none"),
        ClientError::Principal(_) => (StatusCode::BAD_REQUEST, "principal", false, "none"),
        ClientError::Outcome(CallOutcome::NoCapacity { .. }) => (StatusCode::TOO_MANY_REQUESTS, "capacity", false, "none"),
        ClientError::Tls(_) => (StatusCode::BAD_GATEWAY, "tls", false, "none"),
        // The partner answered.
        ClientError::Refused { .. } => (StatusCode::BAD_GATEWAY, "refused", true, "refused"),
        ClientError::Catalogue(_) => (StatusCode::BAD_GATEWAY, "catalogue", true, "refused"),
        // Nobody can say what happened.
        ClientError::Outcome(CallOutcome::DeliveryUnknown { .. }) => (StatusCode::GATEWAY_TIMEOUT, "delivery-unknown", true, "unknown"),
        // A reply arrived and could not be read: the partner ran something. Reporting this as a
        // clean failure would be the overclaim item 1 exists to remove.
        ClientError::Transport(_) => (StatusCode::BAD_GATEWAY, "transport", true, "unknown"),
        ClientError::Outcome(_) => (StatusCode::BAD_GATEWAY, "outcome", true, "unknown"),
    };
    let mut body = json!({ "error": kind, "detail": e.to_string(), "sent": sent, "delivery": delivery });
    if let ClientError::Outcome(CallOutcome::DeliveryUnknown { attempted_via, .. }) = e {
        body["attempted_via"] = json!(attempted_via);
    }
    if let ClientError::Refused { status: partner_status, code, .. } = e {
        body["partner_status"] = json!(partner_status);
        body["partner_code"] = json!(code);
    }
    (status, Json(body)).into_response()
}
