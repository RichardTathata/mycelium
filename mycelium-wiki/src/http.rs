//! HTTP gateway endpoints (Phase 4) — `/gateway/wiki/*`.
//!
//! Registered onto the Mycelium embedded gateway via
//! [`GossipAgent::with_http_routes`](mycelium::GossipAgent::with_http_routes) — the JSON edge for the
//! Python/TS `WikiClient`s. `read`/`query` are served **directly from the store** on the serving node
//! (the data-plane parallel-read property); `propose` enqueues to the curator. Bodies are plain JSON
//! (wiki content is UTF-8 text — no base64, unlike the blackboard's binary payloads).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;
use serde_json::json;

use crate::agent::Wiki;
use crate::model::{Predicate, SectionId, WikiError};
use crate::store::WikiStore;

impl<S: WikiStore + 'static> Wiki<S> {
    /// An axum `Router` with the wiki gateway endpoints, ready for `GossipAgent::with_http_routes`.
    pub fn http_router(self: Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/gateway/wiki/read", post(gw_read::<S>))
            .route("/gateway/wiki/query", post(gw_query::<S>))
            .route("/gateway/wiki/propose", post(gw_propose::<S>))
            .route("/gateway/wiki/ingest", post(gw_ingest::<S>))
            .with_state(self)
    }
}

fn err_response(e: &WikiError) -> Response {
    let status = match e {
        WikiError::BadPath(_) => StatusCode::BAD_REQUEST,
        _                     => StatusCode::BAD_GATEWAY,
    };
    (status, Json(json!({ "error": e.to_string() }))).into_response()
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

/// A request may name the group; if it does and it doesn't match this node's, reject (parity with the
/// blackboard's namespace guard).
fn group_mismatch<S: WikiStore + 'static>(w: &Wiki<S>, group: &Option<String>) -> bool {
    group.as_ref().is_some_and(|g| g.as_str() != w.group().as_ref())
}

#[derive(Deserialize)]
struct ReadBody {
    group: Option<String>,
    page:  String,
}

async fn gw_read<S: WikiStore + 'static>(State(w): State<Arc<Wiki<S>>>, Json(b): Json<ReadBody>) -> Response {
    if group_mismatch(&w, &b.group) {
        return bad_request("unknown group");
    }
    match w.read(&b.page) {
        Ok(Some(page)) => Json(json!({ "page": page })).into_response(),
        Ok(None)       => Json(json!({ "page": null })).into_response(),
        Err(e)         => err_response(&e),
    }
}

#[derive(Deserialize)]
struct QueryBody {
    group: Option<String>,
    #[serde(default)]
    equals: BTreeMap<String, String>,
}

async fn gw_query<S: WikiStore + 'static>(State(w): State<Arc<Wiki<S>>>, Json(b): Json<QueryBody>) -> Response {
    if group_mismatch(&w, &b.group) {
        return bad_request("unknown group");
    }
    let mut pred = Predicate::new();
    for (k, v) in b.equals {
        pred = pred.with(k, v);
    }
    match w.query(&pred) {
        Ok(hits) => Json(json!({ "hits": hits })).into_response(),
        Err(e)   => err_response(&e),
    }
}

#[derive(Deserialize)]
struct ProposeBody {
    group:      Option<String>,
    page:       String,
    /// Omit to mint a new section; provide an existing id to edit it.
    section:    Option<String>,
    #[serde(default)]
    heading:    String,
    body:       String,
    #[serde(default)]
    attributes: BTreeMap<String, String>,
}

async fn gw_propose<S: WikiStore + 'static>(State(w): State<Arc<Wiki<S>>>, Json(b): Json<ProposeBody>) -> Response {
    if group_mismatch(&w, &b.group) {
        return bad_request("unknown group");
    }
    let section: SectionId = match b.section {
        Some(s) => Arc::from(s.as_str()),
        None    => w.new_section_id(&b.page),
    };
    let key = w.propose(&b.page, section.clone(), b.heading, b.body, b.attributes);
    Json(json!({ "proposal": key, "section": section.as_ref() })).into_response()
}

#[derive(serde::Deserialize)]
struct IngestBody {
    group:        Option<String>,
    /// The claim-check reference (never the payload — the batch is staged in the curator's
    /// `BatchSource`; the council-substrate payload rule holds at the HTTP edge too).
    reference:    String,
    /// RPC deadline in seconds when this node must forward to the curator (default 60 — sized to
    /// the batch = one-meeting contract).
    timeout_secs: Option<u64>,
}

/// `POST /gateway/wiki/ingest` — the bulk-ingest claim-check submission for non-Rust pipelines
/// (the council-wiki extraction pipeline is Python): submit a staged batch's *reference*; the
/// group's curator fetches from its own `BatchSource`, applies (batch-atomic through the write
/// gate), publishes, and the summary comes back. Membership-gated at the curator like every
/// submission path.
async fn gw_ingest<S: WikiStore + 'static>(State(w): State<Arc<Wiki<S>>>, Json(b): Json<IngestBody>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if group_mismatch(&w, &b.group) {
        return bad_request("unknown group");
    }
    let timeout = std::time::Duration::from_secs(b.timeout_secs.unwrap_or(60));
    match w.submit_batch_with_timeout(&b.reference, timeout).await {
        Ok(summary) => Json(serde_json::json!({ "summary": summary })).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_GATEWAY,
                   Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}
