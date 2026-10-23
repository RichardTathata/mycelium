//! The node-level routes: the public probes (`/health`, `/ready`, `/stats`, `/metrics`, `/bulk/{id}`) and
//! the gated `/consensus/{*slot}`.

use super::*;

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /metrics` — Prometheus text-format scrape endpoint.
///
/// Available when the `metrics` cargo feature is enabled. Returns
/// `text/plain; version=0.0.4` as expected by Prometheus scrapers.
/// When the feature is disabled, returns 404.
pub(super) async fn metrics_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    #[cfg(feature = "metrics")]
    {
        let body = ctx.prometheus.render();
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        ).into_response()
    }
    #[cfg(not(feature = "metrics"))]
    {
        let _ = ctx;
        (StatusCode::NOT_FOUND, "metrics feature not enabled").into_response()
    }
}

pub(super) async fn health_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    // The WAL writer's state, so a writer refusing appends after a failed write is visible here and
    // not only in the log (the adversarial review of #584). Liveness is unchanged: the process is
    // alive, and the body says what its disk is doing.
    let persistence = match ctx.agent_ctx.wal.get() {
        None => json!({ "configured": false }),
        Some(wal) => {
            let reason = wal.refusing_appends();
            json!({
                "configured": true,
                "wal_refusing_appends": reason.is_some(),
                "reason": reason,
                "dropped_appends": wal.dropped_appends(),
            })
        }
    };
    Json(json!({
        "status":  "ok",
        "node_id": ctx.agent_ctx.node_id.to_string(),
        "persistence": persistence,
    }))
}

/// Readiness probe: returns 200 once soft-state keys (capabilities, locality)
/// have been written to the local store after startup or restart.
/// Returns 503 while WAL replay is still pending or the first advertisement
/// tick has not yet fired.
///
/// Use `/health` for liveness; use `/ready` before routing traffic. Ready = the node has completed
/// startup and serves KV/signals/membership; capability discovery gossips independently and does not
/// gate readiness (so a node advertising no soft state is still ready — audit 2026-07-15 pass 4).
pub(super) async fn ready_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    if ctx.agent_ctx.soft_state_advertised.load(std::sync::atomic::Ordering::Acquire) {
        (StatusCode::OK, Json(json!({ "status": "ready", "node_id": ctx.agent_ctx.node_id.to_string() }))).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "starting", "node_id": ctx.agent_ctx.node_id.to_string() }))).into_response()
    }
}

/// `GET /bulk/{corr_id}`
///
/// Serves a staged bulk-call payload by nonce (hex-encoded 16-char string).
/// Used by the `bulk_serve` target to fetch the caller's staged data over HTTP.
/// Returns 200 + raw bytes on hit, 404 when the nonce is not found.
pub(super) async fn bulk_staging_handler(
    Path(corr_id): Path<String>,
    State(ctx):    State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let nonce = match u64::from_str_radix(corr_id.trim_start_matches("0x"), 16) {
        Ok(n)  => n,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    match ctx.agent_ctx.bulk_transport.get(nonce) {
        Some(bytes) => (StatusCode::OK, bytes.to_vec()).into_response(),
        None        => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /consensus/{*slot}` — inspect the committed value and current ballot for a slot.
///
/// Returns `{"slot": "…", "committed": "<base64>" | null, "ballot": <u64>,
/// "lease_ms": <u64> | null, "lease_expired": <bool>}`.
/// `committed` is the **live** value: `null` when nothing has been committed
/// yet *or* when an epoch lease has expired (the slot has reopened —
/// `lease_expired: true` distinguishes the two). `ballot` reflects the highest
/// ballot number seen for that slot (0 = never proposed).
///
/// This endpoint is public (no auth) and is intended for operational debugging.
#[cfg(feature = "consensus")]
pub(super) async fn consensus_slot_handler(
    Path(slot):   Path<String>,
    State(ctx):   State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let committed_key = format!("{}{}", crate::consensus::consensus_ns::COMMITTED, slot);
    let lease_key     = format!("{}{}", crate::consensus::consensus_ns::LEASE,     slot);
    let ballot_key    = format!("{}{}", crate::consensus::consensus_ns::BALLOT,    slot);
    let live = crate::consensus::live_committed_value(
        &ctx.agent_ctx.kv_state, &slot, crate::consensus::causal_now_ms(&ctx.agent_ctx.hlc),
    );
    let store = ctx.agent_ctx.kv_state.store.pin();
    let raw_present = store.get(committed_key.as_str())
        .and_then(|e| e.data.as_ref())
        .is_some();
    let committed_b64 = live
        .map(|b| base64::engine::general_purpose::STANDARD.encode(&b));
    // Row A: a decision whose record has ended (released, or its lease past) reads `lease_expired`;
    // the raw committed key is consulted only for a slot with no decision record.
    // Row A: a slot with a decision record says `lease_expired` exactly when its top record has ended
    // (released, or its lease past) — not when a live top's large value has not arrived, and not for a
    // slot whose top is not here yet (`Unknown`). Only a slot with no record reads the raw committed key.
    let lease_expired = match crate::consensus_life::read_slot(
            &ctx.agent_ctx.kv_state, &slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(t) => t.ended,
        crate::consensus_life::SlotView::Unknown => false,
        crate::consensus_life::SlotView::NoRecord => raw_present && committed_b64.is_none(),
    };
    let lease_ms = store.get(lease_key.as_str())
        .and_then(|e| e.data.clone())
        .and_then(|b| crate::consensus::decode_lease_ms(&b));
    let ballot: u64 = store.get(ballot_key.as_str())
        .and_then(|e| e.data.clone())
        .map(|b| crate::consensus::decode_ballot(&b))
        .unwrap_or(0);
    Json(json!({
        "slot":          slot,
        "committed":     committed_b64,
        "ballot":        ballot,
        "lease_ms":      lease_ms,
        "lease_expired": lease_expired,
    }))
}

pub(super) async fn stats_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    let kv = &ctx.agent_ctx.kv_state;
    let task_count = ctx.agent_ctx.task_handles
        .lock().unwrap_or_else(|e| e.into_inner())
        .len();
    let mut body = json!({
        "node_id":       ctx.agent_ctx.node_id.to_string(),
        "cluster_name":  ctx.agent_ctx.config.cluster_name,
        "store_entries": kv.store.pin().len(),
        // SWIM membership-view size — the map the de-pin reads; the direct diagnostic
        // for connection-fan-out / scale behaviour (small here ⇒ de-pin precondition unmet).
        "peers":         ctx.agent_ctx.peers.pin().len(),
        "dropped_frames": kv.dropped_frames.load(std::sync::atomic::Ordering::Relaxed),
        "individual_flood_fallbacks": kv.individual_flood_fallbacks.load(std::sync::atomic::Ordering::Relaxed),
        "task_count":    task_count,
        "commit_conflicts": ctx.agent_ctx.commit_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "consensus_decided_floor_anomalies": ctx.agent_ctx.decided_floor_anomaly_slots.pin().len() as u64
            + ctx.agent_ctx.decided_floor_anomalies_unrecorded.load(std::sync::atomic::Ordering::Relaxed),
        "consensus_ballot_space_exhausted": ctx.agent_ctx.ballot_space_exhausted
            .load(std::sync::atomic::Ordering::Relaxed),
        "sys_namespace_violations": ctx.agent_ctx.sys_namespace_violations
            .load(std::sync::atomic::Ordering::Relaxed),
        "identity_anchor_conflicts": ctx.agent_ctx.identity_anchor_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "cap_authz_violations": ctx.agent_ctx.cap_authz_violations
            .load(std::sync::atomic::Ordering::Relaxed),
        "schema_mismatch": ctx.agent_ctx.schema_mismatch
            .load(std::sync::atomic::Ordering::Relaxed),
        "governance_changes": ctx.agent_ctx.governance_changes
            .load(std::sync::atomic::Ordering::Relaxed),
        "governance_unaudited": ctx.agent_ctx.governance_unaudited
            .load(std::sync::atomic::Ordering::Relaxed),
        "rate_limited_senders": mycelium_core::rate::throttled_sender_count(&ctx.agent_ctx.core),
        "rpc_reply_sender_mismatches": ctx.agent_ctx.rpc_reply_sender_mismatches
            .load(std::sync::atomic::Ordering::Relaxed),
        "rpc_respond_duplicates": ctx.rpc_respond_duplicates.load(std::sync::atomic::Ordering::Relaxed),
        // Legible-Emergence Phase 1 (emergent detectors). The conflict gauge is always present
        // (0 unless the detector loop is running); `view_confidence` — the RT1/RT2 "this is a
        // per-node estimate, not fleet truth" header — is attached only when detectors are enabled.
        "governed_group_conflicts": ctx.agent_ctx.governed_group_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "capability_coverage_gaps": ctx.agent_ctx.capability_coverage_gaps
            .load(std::sync::atomic::Ordering::Relaxed),
        "membership_flaps": ctx.agent_ctx.membership_flaps
            .load(std::sync::atomic::Ordering::Relaxed),
        // P10 — the largest share of the fleet's single-writer roles on one node. A **percentage,
        // not a count**: the reading is present whether or not it trips, so an operator can watch
        // it climb rather than learn about it at the threshold.
        "role_concentration_pct": ctx.agent_ctx.role_concentration_pct
            .load(std::sync::atomic::Ordering::Relaxed),
        "opacity_oscillations": ctx.agent_ctx.opacity_oscillations
            .load(std::sync::atomic::Ordering::Relaxed),
        "opaque_node_pct": ctx.agent_ctx.config.emergent_detectors_enabled
            .then(|| super::emergent::compute_opaque_node_pct(&ctx.agent_ctx)),
        "view_confidence": ctx.agent_ctx.config.emergent_detectors_enabled
            .then(|| super::emergent::compute_view_confidence(&ctx.agent_ctx)),
    });
    for (name, value) in feature_gated_counters() {
        body[name] = json!(value);
    }
    // Post-360 row R: client serve streams closed because an in-process receiver took their kind.
    body["signal_client_evictions"] = json!(ctx.agent_ctx.signal_handlers.client_evictions());
    // Post-360 row E: signed caller envelopes accepted without an id (a pre-row-E gateway's — the
    // one-release window; `deprecations.md` §45), and envelopes dropped as replays.
    {
        let r = &ctx.agent_ctx.caller_replay;
        body["caller_envelopes_without_id"] = json!(r.without_id.load(std::sync::atomic::Ordering::Relaxed));
        body["caller_envelopes_replayed"] = json!(r.replayed.load(std::sync::atomic::Ordering::Relaxed));
    }
    // Adopter-surface X4: a `correlation` or `parent` carried as a header or in the caller envelope — never read.
    body["causal_fields_misplaced"] = json!(ctx.agent_ctx.causal_misplaced.load(std::sync::atomic::Ordering::Relaxed));
    // Post-360 plan rows F and F2 (`deprecations.md` §35, §40), per node: unsigned KV `Data` frames and
    // bare (pre-2.33) KV signatures a TLS node refused, and signed frames refused because their signer
    // was not their sender. Frames, not peers. `kv_untagged_signatures` and `kv_unsigned_frames_accepted`
    // counted row F's window and stay at zero since F2 (kept so a 2.x reader does not break). (2.32.0's
    // `identity_untagged_proofs` and `consensus_untagged_signatures` went with the §23 window.)
    #[cfg(feature = "tls")]
    {
        let b = &ctx.agent_ctx.transport_bounds;
        let load = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
        body["kv_untagged_signatures"] = json!(load(&b.kv_untagged_signatures));
        body["kv_unsigned_frames_accepted"] = json!(load(&b.kv_unsigned_accepted));
        body["kv_unsigned_frames_refused"] = json!(load(&b.kv_unsigned_refused));
        body["kv_untagged_signatures_refused"] = json!(load(&b.kv_untagged_refused));
        body["kv_signer_mismatch"] = json!(load(&b.kv_signer_mismatch));
        body["kv_identity_exempt_frames"] = json!(load(&b.kv_identity_exempt));
        body["kv_identity_stale_refused"] = json!(load(&b.kv_identity_stale_refused));
        body["kv_revoked_key_refused"] = json!(load(&b.kv_revoked_key_refused));
        // Post-360 plan row Q: certificate records issued past the skew bound (ignored), records past a
        // cap (skipped), and counted revocations the journal could not take (the node saturated).
        let l = &ctx.agent_ctx.identity_ledger;
        body["identity_cert_future"] = json!(load(&l.cert_future));
        body["identity_records_over_cap"] = json!(load(&l.over_cap));
        body["revocation_journal_refused"] = json!(load(&l.journal_refused));
        // Row Q: this node's own persisted audit records that did not verify at start (kept, never
        // sealed over).
        #[cfg(feature = "compliance")]
        {
            let (unverified, count, malformed) = {
                let g = ctx.agent_ctx.audit_chain.lock().unwrap_or_else(|e| e.into_inner());
                (g.unverified, g.unverified_count, g.malformed)
            };
            body["audit_unverified_records"] = json!(count);
            body["audit_unverified_from_seq"] = json!(unverified.map(|(a, _)| a));
            body["audit_malformed_keys"] = json!(malformed);
        }
    }
    Json(body)
}

/// The `/stats` counters that exist only in some builds — each present exactly when the code that
/// increments it is compiled, so a reader never sees a `0` for a counter this build cannot raise.
/// Returned as a list (possibly empty) so the caller's loop is the same in every build.
pub(super) fn feature_gated_counters() -> Vec<(&'static str, u64)> {
    // Answers this node's acceptor withheld because its promise or acceptance did not reach the WAL.
    let acceptor_unrecorded: Option<(&'static str, u64)> = {
        #[cfg(feature = "consensus")]
        { Some(("consensus_acceptor_unrecorded", crate::consensus::acceptor_answers_unrecorded())) }
        #[cfg(not(feature = "consensus"))]
        { None }
    };
    // The acceptor's memory guard (`acceptor_guard`): persistence refusals, an empty store, forgotten promises.
    let acceptor_guard: [Option<(&'static str, u64)>; 3] = {
        #[cfg(feature = "consensus")]
        { [Some(("electorate_persistence_refused", crate::electorate_persistence_refused())),
           Some(("acceptor_memory_empty_for_electorate", crate::acceptor_memory_empty_for_electorate())),
           Some(("acceptor_promises_forgotten", crate::acceptor_promises_forgotten()))] }
        #[cfg(not(feature = "consensus"))]
        { [None, None, None] }
    };
    [acceptor_unrecorded].into_iter().chain(acceptor_guard).flatten().collect()
}
