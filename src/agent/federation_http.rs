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

use super::gateway_caller::{federation_principal, ResolvedPrincipal};
use crate::agent::TaskCtx;
use crate::federation::edge::{now_ms, FederationEdge, PresentedCall, CATALOG_PATH, HEADER_FEDERATED_CALL};
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
