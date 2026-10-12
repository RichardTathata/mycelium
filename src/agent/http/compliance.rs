//! `/gateway/audit`, `/gateway/transparency`, `/gateway/identity/revoke` — the `compliance` routes.

use super::*;

/// `GET /gateway/audit` — query the tamper-evident audit trail (compliance, scope
/// `audit:read`). Optional `?node=` selects one stream (default: all known
/// streams); optional `?limit=` caps the records returned per stream (most
/// recent), while verification always runs over the full stream. Each stream
/// reports `verified` + any `verify_error`, the chain-tip `head_hash`, and each
/// record's stable `content_hash` (the M16-citable identifier).
#[cfg(feature = "compliance")]
#[derive(Deserialize)]
pub(super) struct AuditQuery {
    node:  Option<String>,
    limit: Option<usize>,
}

#[cfg(feature = "compliance")]
pub(super) fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(feature = "compliance")]
pub(super) async fn gw_audit(
    State(ctx): State<Arc<HttpCtx>>,
    Query(q): Query<AuditQuery>,
) -> Response {
    let tc = &ctx.agent_ctx;
    let nodes: Vec<crate::node_id::NodeId> = match q.node.as_deref() {
        Some(s) => match s.parse() {
            Ok(n) => vec![n],
            Err(_) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid node id"})))
                    .into_response();
            }
        },
        None => super::audit::stream_nodes(tc),
    };
    let limit = q.limit.unwrap_or(usize::MAX);

    let mut streams = Vec::with_capacity(nodes.len());
    for node in nodes {
        let records = super::audit::read_stream(tc, &node);
        let verify  = super::audit::verify_stream(tc, &node);
        let head_hash = records.last().map(|sr| hex32(&sr.record.content_hash()));
        let shown: Vec<_> = records
            .iter()
            .rev()
            .take(limit)
            .rev()
            .map(|sr| {
                let r = &sr.record;
                json!({
                    "seq":          r.seq,
                    "hlc":          r.hlc,
                    "principal":    r.principal,
                    "action":       format!("{:?}", r.action),
                    "target":       r.target,
                    "outcome":      format!("{:?}", r.outcome),
                    "detail":       r.detail,
                    "content_hash": hex32(&r.content_hash()),
                })
            })
            .collect();
        streams.push(json!({
            "node":         node.to_string(),
            "count":        records.len(),
            "verified":     verify.is_ok(),
            "verify_error": verify.err().map(|e| format!("{e:?}")),
            "head_hash":    head_hash,
            "records":      shown,
        }));
    }
    Json(json!({ "streams": streams })).into_response()
}

/// `GET /gateway/transparency` (scope `transparency:read`) — the revocation transparency log
/// (WS-D / D2). With no query: each node's Merkle `root` + `count` (the head). With `?node=&key=`
/// (key = 64-hex of a revoked verifying key): a **client-checkable inclusion proof** that the
/// revocation is in that node's log — `leaf`, `index`, the Merkle audit `proof`, and the `root` to
/// verify against (run `transparency::verify_inclusion` locally; no trust in this server needed).
#[cfg(feature = "compliance")]
#[derive(Deserialize)]
pub(super) struct TransparencyQuery {
    node: Option<String>,
    key:  Option<String>,
}

#[cfg(feature = "compliance")]
pub(super) fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    // `s.len()` is a BYTE length; the loop below byte-slices `&s[i*2..i*2+2]` assuming 1 byte/char.
    // Without the ASCII guard a 64-byte string containing a multibyte UTF-8 char panics on a
    // non-char-boundary slice — and with `panic = "abort"` (release) that aborts the node. Hex is
    // ASCII, so reject non-ASCII up front (audit 2026-07-15 pass 2).
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// `POST /gateway/identity/revoke` (scope `identity:write`, `compliance`) — SOC 2 WS-B.
///
/// Operator-facing key revocation, the compromise-remediation trigger. Body:
/// `{ "revoked_key": "<64 hex>", "reason": "..."? }`. Writes a signed revocation (signed by this
/// node's **current** key) which all verify paths — roles, audit, **and consensus** — then exclude
/// cluster-wide. Only this node's own historical keys can be revoked (the signer must hold the
/// current key), so this remediates *this* node's compromised key: rotate to a fresh key first,
/// then revoke the old one (or use `rotate_identity_on_compromise`, which does both).
#[cfg(feature = "compliance")]
pub(super) async fn gw_identity_revoke(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> Response {
    let key_s = match body["revoked_key"].as_str() {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing revoked_key (64 hex chars)"}))).into_response(),
    };
    let Some(revoked_key) = parse_hex32(key_s) else {
        return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "revoked_key must be 64 hex chars"}))).into_response();
    };
    let reason = body["reason"].as_str().map(|s| s.to_string());
    match super::revocation::revoke_key(&ctx.agent_ctx, revoked_key, reason) {
        Ok(())  => {
            audit_govern(&ctx.agent_ctx, "identity/revoke", body.to_string());
            Json(json!({"ok": true, "revoked_key": key_s})).into_response()
        }
        Err(e)  => (StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[cfg(feature = "compliance")]
pub(super) async fn gw_transparency(
    State(ctx): State<Arc<HttpCtx>>,
    Query(q): Query<TransparencyQuery>,
) -> Response {
    let tc = &ctx.agent_ctx;

    // Inclusion-proof mode: ?node=&key=.
    if let (Some(node_s), Some(key_s)) = (q.node.as_deref(), q.key.as_deref()) {
        let Ok(node) = node_s.parse::<crate::node_id::NodeId>() else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid node id"}))).into_response();
        };
        let Some(revoked_key) = parse_hex32(key_s) else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "key must be 64 hex chars"}))).into_response();
        };
        return match super::transparency::inclusion_proof(tc, &node, &revoked_key) {
            Some((leaf, index, proof, root)) => Json(json!({
                "node":        node.to_string(),
                "revoked_key": key_s,
                "included":    true,
                "root":        hex32(&root),
                "leaf":        hex32(&leaf),
                "index":       index,
                "proof":       proof.iter().map(|s| json!({
                    "sibling":  hex32(&s.sibling),
                    "on_right": s.on_right,
                })).collect::<Vec<_>>(),
            })).into_response(),
            None => Json(json!({
                "node": node.to_string(), "revoked_key": key_s, "included": false,
            })).into_response(),
        };
    }

    // Head mode: every node's revocation-log root + count.
    let nodes = super::revocation::revocation_nodes(tc);
    let heads: Vec<_> = nodes.iter().map(|node| {
        let (root, count) = super::transparency::revocation_head(tc, node);
        json!({ "node": node.to_string(), "root": hex32(&root), "count": count })
    }).collect();
    Json(json!({ "nodes": heads })).into_response()
}
