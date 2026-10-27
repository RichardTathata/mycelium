//! **Cross-origin reads, opt-in** (adopter-surface Y3, decision D28).
//!
//! A browser page served from another origin — the Ops Console, read from a static file host — can
//! read a gateway's answers only if the gateway names its origin. `gateway_cors_origins` lists the
//! exact origins; with it empty (the default) this layer is not installed at all, so no response
//! carries `Access-Control-Allow-Origin` and a cross-origin page reads nothing.
//!
//! With it set, the layer answers **only** for [`CORS_READ_PATHS`] and only for `GET`/`HEAD`: a
//! listed origin's read gets `Access-Control-Allow-Origin: <that origin>` (never `*`) and
//! `Vary: Origin`; its preflight for `GET`/`HEAD` is answered `204` with `Authorization` admitted, so
//! the page can present a bearer and the gateway's own `gateway_auth` still decides what it may read.
//! Everything else — a write, a stream, a call, an unlisted origin, a preflight asking for another
//! method — passes through untouched and so carries no CORS header: the browser keeps the answer
//! from the page. CORS limits what a page may *read*; it never authenticates anyone, which is why the
//! bearer stays the boundary.
//!
//! The layer wraps the whole router, so a preflight is answered before `gateway_auth` (a browser
//! sends no credential on a preflight) — but only the preflight: the read itself goes through
//! `gateway_auth` like any other request.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The routes a listed origin may read across origins: the node's probes and the observing routes the
/// Ops Console renders (`examples/ops_console/README.md` names each panel's route). Read-only, each
/// one — no write, no SSE stream, no call. `/gateway/kv` is listed for its `GET` (a value peek); its
/// `POST`/`DELETE` are not, because only `GET`/`HEAD` are ever answered. `/gateway/audit` and
/// `/gateway/tuple/overview` answer only where mounted (`compliance`; the tuple-space companion).
pub(crate) const CORS_READ_PATHS: &[&str] = &[
    "/health",
    "/ready",
    "/stats",
    "/metrics",
    "/gateway/fleet",
    "/gateway/diagnose",
    "/gateway/guarantees",
    "/gateway/govern",
    "/gateway/kv/keys",
    "/gateway/kv",
    "/gateway/audit",
    "/gateway/tuple/overview",
];

/// How long a browser may cache an answered preflight.
const PREFLIGHT_MAX_AGE_SECS: &str = "600";

/// The listed origin, if this request comes from one and asks for a read-only path.
fn admitted<'a>(origins: &[String], req: &'a Request) -> Option<&'a HeaderValue> {
    let origin = req.headers().get(header::ORIGIN)?;
    let o = origin.to_str().ok()?;
    (origins.iter().any(|a| a == o) && CORS_READ_PATHS.contains(&req.uri().path())).then_some(origin)
}

fn is_read(m: &Method) -> bool {
    m == Method::GET || m == Method::HEAD
}

pub(super) async fn cors_reads(State(origins): State<Arc<[String]>>, req: Request, next: Next) -> Response {
    let origin = admitted(&origins, &req).cloned();
    if req.method() == Method::OPTIONS {
        let asks_read = req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|m| m.to_str().ok())
            .is_some_and(|m| m == "GET" || m == "HEAD");
        if let (Some(origin), true) = (origin, asks_read) {
            let mut resp = StatusCode::NO_CONTENT.into_response();
            let h = resp.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, HEAD"));
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization"));
            h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static(PREFLIGHT_MAX_AGE_SECS));
            h.append(header::VARY, HeaderValue::from_static("Origin"));
            return resp;
        }
        // Not a preflight this layer answers: the router answers it (405, 404 or 401), with no CORS header.
        return next.run(req).await;
    }
    let read = is_read(req.method());
    let on_read_path = CORS_READ_PATHS.contains(&req.uri().path());
    let mut resp = next.run(req).await;
    if on_read_path {
        // A cache in front of the gateway must not serve one origin's answer to another.
        resp.headers_mut().append(header::VARY, HeaderValue::from_static("Origin"));
    }
    if let (Some(origin), true) = (origin, read) {
        resp.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    resp
}
