//! The raw KV routes: get / set / delete / keys / quorum, and the gossip write they share.

use super::*;

// ── KV gateway handlers ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct KvKeyQuery { pub(super) key: String }

/// `GET /gateway/kv?key=K` — read a single KV entry.
///
/// Returns `{"found": true, "value_b64": "…"}` or `{"found": false}`.
pub(super) async fn gw_kv_get(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    match ctx.agent_ctx.kv_state.store.pin().get(q.key.as_str()).and_then(|e| e.data.clone()) {
        Some(bytes) => {
            use base64::Engine as _;
            let v = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Json(json!({ "found": true, "value_b64": v })).into_response()
        }
        None => Json(json!({ "found": false })).into_response(),
    }
}

/// `POST /gateway/kv` — write a KV entry.
///
/// Body: `{"key": "…", "value_b64": "…"}`. Returns `{"ok": true, "operation_id", "local_durability",
/// "local_durability_error"?}` — the write's **receipt** (rung 1 always; rung 2 as
/// `local_durability`, the same vocabulary the consensus commits and the SDKs carry). Until
/// 2026-09-26 the route discarded the receipt and answered a bare `{"ok": true}`, so an HTTP or SDK
/// client could not learn rung 2 for an ordinary set (doc-coverage run 17). Additive: `ok` is
/// unchanged.
pub(super) async fn gw_kv_set(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;

    let key = match body["key"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing key"}))).into_response(),
    };
    if let Some(refused) = refuse_protected_key(&key) {
        return refused;
    }
    // A write must say what it writes. Until 2026-09-24 a missing `value_b64` silently wrote an
    // EMPTY value and answered `{"ok": true}` — so a misspelled field name erased a key and
    // reported success. The overlay test helper had been sending `value` since it was written, and
    // every sentinel it ever wrote was empty; nothing noticed, because the check that consumed them
    // only counted keys. An explicitly empty value is legitimate (`"value_b64": ""`); an *omitted*
    // one must not become one. Tombstoning has its own verb (`DELETE`), so silent-empty had no
    // legitimate caller.
    let value = match body.get("value_b64") {
        Some(serde_json::Value::String(b64)) => {
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(v)  => Bytes::from(v),
                Err(_) => return (StatusCode::BAD_REQUEST,
                                  Json(json!({"error":"invalid base64 in 'value_b64'"}))).into_response(),
            }
        }
        Some(_) => return (StatusCode::BAD_REQUEST,
                           Json(json!({"error":"'value_b64' must be a base64 string"}))).into_response(),
        None    => return (StatusCode::BAD_REQUEST,
                           Json(json!({"error":"missing 'value_b64' (use \"\" for an empty value, or DELETE to tombstone)"}))).into_response(),
    };

    let tc = Arc::clone(&ctx.agent_ctx);
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    match mycelium_core::ops::kv_set_with_receipt(&tc, op, attempt, key, value, None).await {
        Ok(r) => {
            let mut body = json!({
                "ok": true,
                "operation_id": r.operation_id.to_string(),
                "local_durability": r.local_durability.tag(),
            });
            if let Some(reason) = r.local_durability.failure_reason() {
                body["local_durability_error"] = json!(reason);
            }
            Json(body).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                   Json(json!({ "ok": false, "error": e.to_string() }))).into_response(),
    }
}

/// `DELETE /gateway/kv?key=K` — tombstone a KV entry.
///
/// Returns `{"ok": true}`.
pub(super) async fn gw_kv_delete(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_protected_key(&q.key) {
        return refused;
    }
    kv_write(&ctx.agent_ctx, Arc::from(q.key.as_str()), Bytes::new(), true);
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub(super) struct KvKeysQuery { prefix: Option<String> }

/// `GET /gateway/kv/keys?prefix=P` — list live KV keys, optionally filtered by prefix.
///
/// Returns `{"keys": ["key1", "key2", …]}`.
pub(super) async fn gw_kv_keys(
    Query(q):   Query<KvKeysQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let keys: Vec<String> = if let Some(ref pfx) = q.prefix {
        crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, pfx.as_str())
            .into_iter()
            .map(|(k, _)| k.as_ref().to_string())
            .collect()
    } else {
        ctx.agent_ctx.kv_state.store.pin()
            .iter()
            .filter(|(_, v)| v.data.is_some())
            .map(|(k, _)| k.as_ref().to_string())
            .collect()
    };
    Json(json!({ "keys": keys })).into_response()
}

/// `POST /gateway/kv/quorum` — write + wait for peer acknowledgements.
///
/// Since item 1 PR 4b this route **asks** each peer whether it holds the operation, rather than
/// watching the gossip stream for evidence the substrate cannot carry (`contracts-receipts.md`
/// §1a — before 4b it timed out however widely the write spread). `acks_received` counts peers that
/// answered *persisted*: their store holds this exact stamp and content and their WAL `fdatasync`
/// returned `Ok`, so they hold it across their own restart.
///
/// On timeout, `unknown_peers` counts those that did not answer *persisted* — **unknown, never
/// "did not persist"**. A peer may be unreachable, mid-restart, on a build without the handler, or
/// already holding a newer value for the key. The write itself is applied and gossiped either way,
/// so a timeout here never means the value was not written.
///
/// Request body:
/// ```json
/// { "key": "...", "value_b64": "<base64>", "min_acks": 2, "timeout_secs": 5.0 }
/// ```
/// Success: `{ "ok": true, "acks_received": 2 }`
/// Timeout: `{ "ok": false, "error": "timeout", "acks_received": 0 }`
#[derive(Deserialize)]
pub(super) struct KvQuorumBody {
    key:         String,
    #[serde(default)]
    value_b64:   String,
    min_acks:    usize,
    #[serde(default = "default_quorum_timeout")]
    timeout_secs: f64,
}

pub(super) fn default_quorum_timeout() -> f64 { 5.0 }

pub(super) async fn gw_kv_quorum(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<KvQuorumBody>,
) -> impl IntoResponse {
    use base64::Engine as _;

    if let Some(refused) = refuse_protected_key(&body.key) {
        return refused;
    }
    let value = match base64::engine::general_purpose::STANDARD.decode(&body.value_b64) {
        Ok(v)  => Bytes::from(v),
        Err(_) => return (StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid base64" }))).into_response(),
    };

    let key: Arc<str> = Arc::from(body.key.as_str());
    // `try_from_secs_f64`, NOT `from_secs_f64`: the latter PANICS on a negative, non-finite, or
    // over-large value. `timeout_secs` is an untrusted client `f64`, and with `panic = "abort"` in
    // the release profile a panic aborts the whole node — so `{"timeout_secs":-1}` on the (default
    // loopback-open) gateway was an unauthenticated single-request node kill (audit 2026-07-15 pass 2).
    let timeout = match Duration::try_from_secs_f64(body.timeout_secs) {
        Ok(d)  => d,
        Err(_) => return (StatusCode::BAD_REQUEST,
            Json(json!({ "error": "timeout_secs must be a finite, non-negative number" }))).into_response(),
    };
    let tc             = Arc::clone(&ctx.agent_ctx);

    if body.min_acks == 0 {
        kv_write(&tc, key, value, false);
        return Json(json!({ "ok": true, "acks_received": 0 })).into_response();
    }

    // Item 1 PR 4b: write, then **ask** the peers. The previous implementation installed a
    // tracker and watched the gossip stream for evidence of its own write, which this substrate
    // cannot carry (contracts-receipts.md §1a), so it timed out however widely the write spread.
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    let receipt = match mycelium_core::ops::kv_set_with_receipt(
        &tc, op, attempt, Arc::clone(&key), value, None,
    ).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": e.to_string() }))).into_response(),
    };
    let replica = super::replica_sync::collect(
        &tc,
        super::replica_sync::Query {
            stamp: receipt.stamp,
            content_hash: receipt.content_hash,
            key: Arc::clone(&key),
        },
        timeout,
    )
    .await;
    let acks = replica.persisted_by.len();
    let result: Result<usize, ()> = if acks >= body.min_acks { Ok(acks) } else { Err(()) };

    match result {
        Ok(n)  => Json(json!({ "ok": true, "acks_received": n })).into_response(),
        Err(_) => Json(json!({
            "ok": false,
            "error": "timeout",
            "acks_received": acks,
            // Peers that did not answer "persisted" — unknown, never "did not persist".
            "unknown_peers": replica.missing.len(),
        })).into_response(),
    }
}

/// Applies a KV write (set or delete) and fans out to gossip peers.
pub(super) fn kv_write(ctx: &Arc<TaskCtx>, key: Arc<str>, value: Bytes, tombstone: bool) -> bool {
    use crate::framing::{dispatch_gossip_try_send, make_gossip_update, ForwardHint};
    use crate::store::apply_and_notify;
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, value, tombstone, &ctx.hlc);
    apply_and_notify(&ctx.kv_state, &update); // apply, then persist (persistence.rs invariant 1)
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(crate::framing::sync_entry_from(&update));
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        crate::framing::local_kv_wire_msg(ctx, update),
        ctx.node_id.id_hash(),
        ForwardHint::All,
        &ctx.kv_state.dropped_frames,
    )
}
