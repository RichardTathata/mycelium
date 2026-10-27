//! `/gateway/audit`, `/gateway/transparency`, `/gateway/identity/revoke`, `/gateway/records/{decision,evidence}/{sha256}`
//! — the `compliance` routes.

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
        let seg     = super::audit::segments(tc, &node);
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
            // Post-360 plan row Q: which records verify (the prefix and the tail linked to it) and
            // which do not — kept, never sealed over.
            "segments": {
                "verified": seg.linked.len(),
                "unverified": seg.unverified.len(),
                "unverified_seqs": seg.unverified.iter().take(64).collect::<Vec<_>>(),
                "malformed_keys": seg.malformed,
                "head_seq": seg.head.map(|(s, _)| s),
            },
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
/// node's **current** key) which every verifier of this node's key — KV writes, identity records,
/// roles, audit, **and consensus** — then excludes at each node that validates it (post-360 plan row
/// Q). Only this node's own keys can be revoked, and not the one it signs with (422), so this
/// remediates *this* node's compromised key: rotate to a fresh key first, then revoke the old one
/// (or use `rotate_identity_on_compromise`, which does both).
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

// ── A record by its hash (adopter-surface Y4) ─────────────────────────────────────────────────────

/// The schema id of a `/gateway/records/…` answer; a change to its shape is a new one.
#[cfg(feature = "compliance")]
pub(crate) const RECORD_READ_SCHEMA: &str = "mycelium.record/1";

/// How many times one by-hash read retries a page of the decision sink whose lock was held before it answers 503.
#[cfg(feature = "compliance")]
const SINK_PAGE_ATTEMPTS: usize = 256;

/// The lowercase hex SHA-256 a span names, or `None`: 64 characters of `0-9a-f`, nothing else (a prefix is
/// `mycelium explain --closure`'s, not this route's — a span always carries the whole hash).
#[cfg(feature = "compliance")]
fn record_hash(given: &str) -> Option<&str> {
    (given.len() == 64 && given.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some(given)
}

#[cfg(feature = "compliance")]
fn bad_record_hash() -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({
        "schema": RECORD_READ_SCHEMA,
        "error": "bad_hash",
        "detail": "a record is asked for by its full content hash: 64 lowercase hex characters, as a span's \
                   mycelium.record_sha256 carries it",
    }))).into_response()
}

/// The body of a record that was found: the exact bytes the hash is over (`record_b64` — what a holder verifies), the
/// record parsed for reading, and what this node can say about it.
#[cfg(feature = "compliance")]
fn found_record(hash: &str, kind: &str, source: &str, bytes: &[u8], chain: serde_json::Value) -> Response {
    use base64::Engine as _;
    let matches = crate::causal::content_hash(bytes) == hash;
    Json(json!({
        "schema": RECORD_READ_SCHEMA,
        "sha256": hash,
        "kind": kind,
        "found": true,
        "source": source,
        "record_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "record": serde_json::from_slice::<serde_json::Value>(bytes).ok(),
        "verification": {
            "hash": if matches { "matches" } else { "mismatch" },
            "chain": chain,
        },
    })).into_response()
}

/// A record that was not found: the one meaning this node can give, or `undetermined` with every meaning it cannot
/// rule out, and what it searched.
#[cfg(feature = "compliance")]
fn record_not_found(hash: &str, kind: &str, cannot_rule_out: Vec<&'static str>, searched: serde_json::Value) -> Response {
    let meaning = match cannot_rule_out.as_slice() {
        [one] => *one,
        _ => "undetermined",
    };
    (StatusCode::NOT_FOUND, Json(json!({
        "schema": RECORD_READ_SCHEMA,
        "sha256": hash,
        "kind": kind,
        "found": false,
        "not_found": meaning,
        "cannot_rule_out": cannot_rule_out,
        "searched": searched,
    }))).into_response()
}

/// `GET /gateway/records/decision/{sha256}` (scope `audit:read`, `compliance`) — one **decision record** by its content
/// hash (adopter-surface Y4): the hash a span's `mycelium.record_sha256` carries, the hash of the record's trace line.
///
/// **Where it looks.** The decision sink this node holds (paged under `try_lock`, never waiting: a page whose lock is
/// held is retried, and after [`SINK_PAGE_ATTEMPTS`] the answer is 503 `sink_busy`), then the trace the embedder
/// declared it writes ([`GossipAgent::with_decision_trace_file`](crate::GossipAgent::with_decision_trace_file)).
///
/// **Found:** 200 with the line's bytes; `verification.hash` is `matches` when SHA-256 over them is the hash asked, and
/// `verification.chain` is `not_chained` — a decision record is best-effort trace, not evidence, and nothing seals it.
///
/// **Not found:** 404, `not_found` one of —
/// - `not_written`: this node keeps no decision trace (no sink attached and no trace declared);
/// - `never_existed`: nothing else is possible — every record this incarnation's sink kept was searched, and the
///   sink dropped none;
/// - `undetermined`, with `cannot_rule_out` naming each meaning still possible: `not_written` when the sink dropped
///   records (a dropped decision was never kept anywhere), `evicted` when records the sink kept were not searched — drained
///   before or during the read (or kept after it finished, counted the same way so the count is never too low) —
///   and the declared trace, if any, did not hold this one, and `never_existed`, which is always possible.
///
/// Scope of `never_existed`: **this incarnation**. The sink lives in memory, so a record an earlier incarnation kept
/// and never wrote to the declared trace is not known here.
#[cfg(feature = "compliance")]
pub(super) async fn gw_record_decision(
    State(ctx): State<Arc<HttpCtx>>,
    Path(given): Path<String>,
) -> Response {
    let Some(hash) = record_hash(&given) else { return bad_record_hash() };
    let tc = &ctx.agent_ctx;
    let mut sink_searched = json!({ "attached": false });
    let mut cannot_rule_out: Vec<&'static str> = Vec::new();
    if let Some(sink) = tc.decision_sink() {
        let (mut after, mut held) = (0u64, 0usize);
        loop {
            let mut page = None;
            for _ in 0..SINK_PAGE_ATTEMPTS {
                page = sink.try_read_after(after, mycelium_core::decision::MAX_PAGE_RECORDS);
                if page.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let Some(page) = page else {
                return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({
                    "schema": RECORD_READ_SCHEMA,
                    "sha256": hash,
                    "error": "sink_busy",
                    "detail": "the decision sink's lock was held on every attempt; this route never waits for it",
                }))).into_response();
            };
            let Some(last) = page.last().map(|r| r.seq) else { break };
            for r in &page {
                let line = r.to_string_line();
                if crate::causal::content_hash(line.as_bytes()) == hash {
                    return found_record(hash, "decision", "sink", line.as_bytes(), json!({ "state": "not_chained" }));
                }
            }
            held += page.len();
            after = last;
        }
        let drops = sink.drop_counts();
        // Every kept record has one `seq` in `1..=last_seq`, so the kept records this read did not see are the count
        // less the ones it did: drained before or during the read, or kept after its last page — conservative, never
        // fewer than have left the ring.
        let left = sink.last_seq().saturating_sub(held as u64);
        if drops.total() > 0 {
            cannot_rule_out.push("not_written");
        }
        if left > 0 {
            cannot_rule_out.push("evicted");
        }
        sink_searched = json!({ "attached": true, "searched": held, "not_searched": left, "dropped": drops });
    }
    let mut trace_searched = json!({ "declared": false });
    if let Some(path) = tc.decision_trace_file.get().cloned() {
        let wanted = hash.to_string();
        let shown = path.display().to_string();
        let read = tokio::task::spawn_blocking(move || crate::closure::find_in_trace(&path, &wanted)).await;
        match read {
            Ok(Ok(found)) => {
                if let Some(line) = found.line {
                    return found_record(hash, "decision", "trace", line.as_bytes(), json!({ "state": "not_chained" }));
                }
                trace_searched = json!({ "declared": true, "records": found.records });
            }
            Ok(Err(e)) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                    "schema": RECORD_READ_SCHEMA, "sha256": hash, "error": "trace_unreadable",
                    "detail": format!("{shown}: {e}"),
                }))).into_response();
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                    "schema": RECORD_READ_SCHEMA, "sha256": hash, "error": "trace_unreadable", "detail": e.to_string(),
                }))).into_response();
            }
        }
    }
    let attached = sink_searched["attached"] == true || trace_searched["declared"] == true;
    if !attached {
        cannot_rule_out = vec!["not_written"];
    } else {
        cannot_rule_out.push("never_existed");
    }
    record_not_found(hash, "decision", cannot_rule_out, json!({ "sink": sink_searched, "trace": trace_searched }))
}

/// `GET /gateway/records/evidence/{sha256}` (scope `audit:read`, `compliance`) — one **evidence record** by its content
/// hash (adopter-surface Y4): the hash a span's `mycelium.record_sha256` carries, the journal frame's, the hash the
/// audit chain's reference record cites as `journal_sha256`.
///
/// **Where it looks.** This node's evidence journal, page by page, off the async workers. The journal never drops a
/// record it acknowledged.
///
/// **Found:** 200 with the frame's bytes, and the record **verified here** two ways — `verification.hash`: SHA-256 over
/// the bytes is the hash asked (`matches`), which a holder recomputes from `record_b64`; `verification.chain`: the
/// reference record in this node's signed, hash-linked audit stream that cites the hash — `referenced` with its `seq`
/// and whether it is in the stream's verified segment (`linked`), or `not_referenced` (no reference cites it: pruned
/// to a checkpoint, or a journal the chain never referenced). The stream itself is read with `GET /gateway/audit`.
///
/// **Not found:** 404, `not_found` one of — `not_written` (no evidence journal is attached: this node writes no
/// evidence); `never_existed` (the journal holds no such record and lost none when it was opened); `undetermined` with
/// `cannot_rule_out: ["evicted", "never_existed"]` when the journal truncated records at open (a torn tail or a damaged
/// frame repaired away), since the record may have been one of them. A hash from another node's span is
/// `never_existed` **here**: each node's journal is its own.
#[cfg(feature = "compliance")]
pub(super) async fn gw_record_evidence(
    State(ctx): State<Arc<HttpCtx>>,
    Path(given): Path<String>,
) -> Response {
    let Some(hash) = record_hash(&given) else { return bad_record_hash() };
    let tc = &ctx.agent_ctx;
    let Some(journal) = tc.evidence_journal.get() else {
        return record_not_found(hash, "evidence", vec!["not_written"], json!({ "journal": { "attached": false } }));
    };
    let (path, truncated) = (journal.path().to_path_buf(), journal.truncated_at_open());
    let wanted = hash.to_string();
    let scan = tokio::task::spawn_blocking(move || -> std::io::Result<(Option<Vec<u8>>, u64)> {
        let mut cursor = evidence_journal::EvidenceCursor::default();
        let mut records = 0u64;
        loop {
            let page = evidence_journal::read_evidence_journal_from(&path, cursor, 256, 1 << 20)?;
            for entry in page.entries {
                if super::action_evaluator::hex32(&entry.content_hash) == wanted {
                    return Ok((Some(entry.bytes), records));
                }
                records += 1;
            }
            if !page.more || page.next == cursor {
                return Ok((None, records));
            }
            cursor = page.next;
        }
    })
    .await;
    let (found, records) = match scan {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "schema": RECORD_READ_SCHEMA, "sha256": hash, "error": "journal_unreadable", "detail": e.to_string(),
            }))).into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "schema": RECORD_READ_SCHEMA, "sha256": hash, "error": "journal_unreadable", "detail": e.to_string(),
            }))).into_response();
        }
    };
    match found {
        Some(bytes) => found_record(hash, "evidence", "journal", &bytes, chain_reference(tc, hash)),
        None => {
            let cannot = if truncated { vec!["evicted", "never_existed"] } else { vec!["never_existed"] };
            record_not_found(hash, "evidence", cannot, json!({
                "journal": { "attached": true, "records": records, "truncated_at_open": truncated },
            }))
        }
    }
}

/// The reference record in this node's own audit stream that cites `hash` as its `journal_sha256`, and whether that
/// record is in the stream's verified segment.
#[cfg(feature = "compliance")]
fn chain_reference(tc: &TaskCtx, hash: &str) -> serde_json::Value {
    let records = super::audit::read_stream(tc, &tc.node_id);
    let cited = records.iter().find(|sr| {
        sr.record.detail.as_deref().and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok()).is_some_and(|v| {
            v["schema"] == super::action_evaluator::AE_REFERENCE_SCHEMA && v["journal_sha256"] == hash
        })
    });
    match cited {
        None => json!({ "state": "not_referenced" }),
        Some(sr) => {
            let linked = super::audit::segments(tc, &tc.node_id).linked.contains(&sr.record.seq);
            json!({
                "state": "referenced",
                "seq": sr.record.seq,
                "linked": linked,
                "reference_content_hash": hex32(&sr.record.content_hash()),
            })
        }
    }
}
