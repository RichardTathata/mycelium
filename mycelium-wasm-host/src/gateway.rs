//! A3 — `POST /gateway/artifacts/publish`: a named, checked door into the gossiped catalogue
//! (`docs/plans/design-time-tooling.md` §10, D10).
//!
//! The body is an **already-signed** catalogue line — the hex of [`InstallableEntry::encode`],
//! which is exactly one line of a library manifest — so the publisher's key never reaches a
//! gateway; the route verifies provenance against the node's trusted publishers and writes the
//! line with [`publish_installable`]. The bytes are not uploaded here: they live at the library
//! the librarians mirror. Refusals are by name and the plan's exit gate says **403**, not 422:
//! an unsigned or untrusted entry is a *permission* failure, not a shape failure.
//!
//! Two things this route is honest about. First, it is *a* door, not *the* door: the raw KV routes
//! refuse `installable/` since 2.27.0, but a peer can still write it through Layer I, so the defence
//! that holds in every case is the provisioner's own `require_provenance`. Second, a librarian
//! tombstones every catalogue entry under its publisher key that its manifest does not carry —
//! so an entry published here under a librarian-managed key would be deleted at the next sync,
//! and the route refuses that by name (**409**) rather than letting it vanish.
//!
//! Mounted with `agent.with_http_routes(artifact_router(..))` **before** `start()`; the stem
//! binary does this from `[hosts].trusted_publishers` when built with `gateway`. The scope family
//! (`artifact:publish`) is enforced by the node's auth layer, which is the `compliance` feature of
//! `mycelium`: a node built without it checks the bearer and nothing narrower, for this route as
//! for every other — the provenance checks below hold either way.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use mycelium::GossipAgent;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::catalog::{publish_installable, InstallableEntry};
use crate::tools::kind_name;

/// The route's state: whose entries it admits, and which key a local librarian owns.
pub struct ArtifactGateway {
    agent:               Arc<GossipAgent>,
    trusted:             Vec<[u8; 32]>,
    librarian_publisher: Option<[u8; 32]>,
}

/// The router carrying `POST /gateway/artifacts/publish` (scope family `artifact:publish`).
/// `trusted` is the node's trusted publisher list (`[hosts].trusted_publishers`); an empty list
/// refuses every entry, so nothing passes by default. `librarian_publisher` is the key a
/// librarian on this node manages, if any — entries under it are refused (409).
pub fn artifact_router(agent: Arc<GossipAgent>, trusted: Vec<[u8; 32]>, librarian_publisher: Option<[u8; 32]>) -> Router {
    let state = Arc::new(ArtifactGateway { agent, trusted, librarian_publisher });
    Router::new().route("/gateway/artifacts/publish", post(gw_publish)).with_state(state)
}

#[derive(Deserialize)]
struct PublishBody {
    /// One manifest line: lowercase hex of `InstallableEntry::encode()`.
    entry_hex: String,
}

fn hex_line(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn refuse(status: StatusCode, error: &str, detail: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error, "detail": detail.into() })))
}

async fn gw_publish(State(gw): State<Arc<ArtifactGateway>>, Json(body): Json<PublishBody>) -> (StatusCode, Json<Value>) {
    let Some(bytes) = hex_line(&body.entry_hex) else {
        return refuse(StatusCode::BAD_REQUEST, "malformed entry", "entry_hex is not hex");
    };
    let Some(entry) = InstallableEntry::decode(&bytes) else {
        return refuse(StatusCode::BAD_REQUEST, "malformed entry", "entry_hex does not decode as a catalogue line (unknown format version or kind, or a truncated line)");
    };
    if gw.trusted.is_empty() {
        return refuse(StatusCode::FORBIDDEN, "no trusted publishers configured", "this node's [hosts].trusted_publishers is empty, so no entry can be vouched for here");
    }
    if entry.signer.len() != 32 || entry.signature.len() != 64 {
        return refuse(StatusCode::FORBIDDEN, "unsigned entry", "the catalogue takes only entries a publisher signed");
    }
    let signer: [u8; 32] = entry.signer.as_slice().try_into().expect("checked");
    if !gw.trusted.contains(&signer) {
        return refuse(StatusCode::FORBIDDEN, "untrusted publisher", format!("ed25519:{} is not in this node's trusted publishers", hex(&signer)));
    }
    if !entry.verify_provenance(&gw.trusted) {
        return refuse(StatusCode::FORBIDDEN, "provenance does not verify", "the signature does not cover this entry as presented — altered after signing, or forged");
    }
    if gw.librarian_publisher == Some(signer) {
        return refuse(
            StatusCode::CONFLICT,
            "librarian-managed signer",
            format!("ed25519:{} is a librarian's key on this node; its manifest is the truth for that key and would tombstone this line at the next sync — publish to the library instead", hex(&signer)),
        );
    }
    if !publish_installable(&gw.agent.kv(), &entry) {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "not written", "the KV write was refused (size gate or a stopped node)");
    }
    (
        StatusCode::OK,
        Json(json!({
            "key": entry.kv_key(),
            "artifact": entry.artifact.to_hex(),
            "signer": format!("ed25519:{}", hex(&signer)),
            "kind": kind_name(entry.kind),
            "provides": { "ns": entry.provides.namespace.as_ref(), "name": entry.provides.name.as_ref() },
        })),
    )
}
