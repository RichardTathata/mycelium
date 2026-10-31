//! The overlays: consensus (cross-group, consistent KV, the lock, the election), the ordered log, reliable
//! delivery.

use super::*;

// ── Overlay gateway helpers ───────────────────────────────────────────────────

/// Build a `ConsensusEngine` from `TaskCtx`, skipping the opacity/load-balance
/// heuristics used by `ConsensusHandle::cluster_propose` — those are performance
/// hints, not correctness requirements, and are not available from `TaskCtx`.
#[cfg(feature = "consensus")]
pub(super) fn overlay_make_engine(ctx: &Arc<TaskCtx>) -> crate::consensus::ConsensusEngine {
    crate::consensus::ConsensusEngine {
        task_ctx:            Arc::clone(ctx),
        abstain_when_opaque: false,
        use_trust_slices:    false,
        max_abstain_ballots: 3,
        self_locality:       None,
        topology_policy:     None,
    }
}

/// Thin system-wide propose from `TaskCtx` (quorum = floor(N/2)+1 over live peers).
#[cfg(feature = "consensus")]
pub(super) async fn overlay_cluster_propose(
    ctx:    &Arc<TaskCtx>,
    slot:   &str,
    value:  Bytes,
    config: crate::consensus::ConsensusConfig,
) -> crate::consensus::ConsensusResult {
    let n_nodes = (ctx.peers.len() + 1).max(1);
    let quorum  = super::helpers::compute_quorum_size(config.quorum_size, n_nodes);
    overlay_make_engine(ctx)
        .propose(
            crate::signal::SignalScope::Cluster,
            Arc::from(slot),
            value,
            quorum,
            config,
            None,
        )
        .await
}

/// The gateway's mirror of `ConsensusHandle::exclusive_propose`: the lock, consistent-write and log-claim routes
/// decide in the fleet's exclusive-default electorate group, or the whole cluster when none is declared (P2).
#[cfg(feature = "consensus")]
pub(super) async fn overlay_exclusive_propose(
    ctx:    &Arc<TaskCtx>,
    slot:   &str,
    value:  Bytes,
    config: crate::consensus::ConsensusConfig,
) -> crate::consensus::ConsensusResult {
    let config = crate::consensus::ConsensusConfig { safety_sensitive: true, ..config };
    match super::electorate::exclusive_electorate(ctx) {
        Ok(Some(group)) => overlay_group_propose(ctx, &group, slot, value, config).await,
        Ok(None) => overlay_cluster_propose(ctx, slot, value, config).await,
        Err(detail) => crate::consensus::ConsensusResult::ElectorateMismatch {
            slot: Arc::from(slot), group: None, detail: Arc::from(detail.as_str()),
        },
    }
}

/// Thin group propose from `TaskCtx`.
#[cfg(feature = "consensus")]
pub(super) async fn overlay_group_propose(
    ctx:    &Arc<TaskCtx>,
    group:  &str,
    slot:   &str,
    value:  Bytes,
    config: crate::consensus::ConsensusConfig,
) -> crate::consensus::ConsensusResult {
    let prefix  = crate::signal::grp_prefix(group);
    let members = crate::store::scan_kv_prefix(ctx.kv_state.as_ref(), &prefix);
    // The electorate must be established, not inferred from absence — the HTTP surface enforces
    // the same contract as the library path (`ConsensusHandle::group_propose`), because a caller
    // must not get a different answer for reaching the same election through a socket.
    let declared_min = super::helpers::declared_electorate_min(ctx, group);
    match super::helpers::resolve_electorate(members.len(), declared_min) {
        super::helpers::Electorate::Established(_) => {}
        super::helpers::Electorate::Unavailable { observed, declared_min } =>
            return crate::consensus::ConsensusResult::ElectorateUnavailable {
                slot:  Arc::from(slot),
                group: Arc::from(group),
                observed_members: observed,
                declared_min,
            },
    }
    // NO `+ 1`: the `grp/{group}/` roster already includes self (a node joins by writing its own
    // member key), so `+ 1` double-counts self and over-sizes the quorum — a solo group `{self}`
    // then needs 2 votes and can only ever cast 1 → spurious Timeout where the library path
    // (`member_ids.len().max(1)`, consensus_handle.rs) commits at quorum 1 (audit 2026-07-15 pass 2).
    let n       = members.len().max(1);
    let quorum  = super::helpers::compute_quorum_size(config.quorum_size, n);
    overlay_make_engine(ctx)
        .propose(
            crate::signal::SignalScope::Group(Arc::from(group)),
            Arc::from(slot),
            value,
            quorum,
            config,
            None,
        )
        .await
}

// ── Cross-group consensus ─────────────────────────────────────────────────────

#[cfg(feature = "consensus")]
#[derive(serde::Deserialize)]
pub(super) struct CrossGroupProposeBody {
    slot:      String,
    value_b64: Option<String>,
    groups:    Vec<crate::consensus::GroupQuorum>,
    /// Marks the proposal safety-sensitive (P2): refused under `consensus_require_electorate`, since a
    /// cross-group proposal is never an electorate.
    #[serde(default)]
    safety_sensitive: bool,
}

/// `POST /gateway/consensus/cross_group_propose` — multi-group proposal.
///
/// Body: `{"slot": "S", "value_b64": "...", "groups": [{"group":"G","quorum":0.5,"veto":false}]}`
/// Returns `{"ok":true}` on commit, or an error status.
#[cfg(feature = "consensus")]
pub(super) async fn gw_cross_group_propose(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<CrossGroupProposeBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let value = if let Some(b64) = body.value_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    if body.groups.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"groups must not be empty"}))).into_response();
    }

    let engine = overlay_make_engine(&ctx.agent_ctx);
    let result = engine.cross_propose(
        Arc::from(body.slot.as_str()),
        value,
        &body.groups,
        crate::consensus::ConsensusConfig { safety_sensitive: body.safety_sensitive, ..crate::consensus::ConsensusConfig::default() },
    ).await;

    let persisted = committed_bool(&result);
    match super::consensus_handle::receipt_from(result, &ctx.agent_ctx) {
        Ok(receipt) => Json(commit_json(persisted, &receipt.local_durability)).into_response(),
        Err(err) => commit_error_response(err),
    }
}

/// The v2.4.2 `"persisted"` bool, read off the result **before** it becomes a receipt so the
/// old field is exactly what it always was — the receipt is added beside it, never derived back
/// into it (the regression floor pins `persisted`; PR 7 must not move that pin).
#[cfg(feature = "consensus")]
pub(super) fn committed_bool(result: &crate::consensus::ConsensusResult) -> bool {
    matches!(result, crate::consensus::ConsensusResult::Committed { persisted: true, .. })
}

/// The commit's JSON — HTTP parity with [`CommitReceipt`](crate::CommitReceipt) (item 1 PR 7,
/// `docs/design/contracts-receipts.md` §5). `"persisted"` is unchanged; `"local_durability"` is
/// [`LocalDurability::tag`](crate::LocalDurability::tag) beside it — the same four names the
/// SDKs read — and `"local_durability_error"` is present **only** when the state is `failed`.
/// JSON is additive-safe: a pre-2.8 client reads `persisted` and ignores the rest.
#[cfg(feature = "consensus")]
pub(super) fn commit_json(persisted: bool, durability: &crate::LocalDurability) -> serde_json::Value {
    let mut body = json!({ "ok": true, "persisted": persisted, "local_durability": durability.tag() });
    if let Some(reason) = durability.failure_reason() {
        body["local_durability_error"] = json!(reason);
    }
    body
}

/// A refused commit, in the statuses the two overlay verbs have always used: delivery unknown is
/// `504` (the value *may* have committed elsewhere — the body says so), a lost or untried
/// proposal is `409`.
#[cfg(feature = "consensus")]
pub(super) fn commit_error_response(err: crate::CommitError) -> axum::response::Response {
    use crate::CommitError;
    match err {
        CommitError::DeliveryUnknown { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("consensus timed out after {ballots_tried} ballot(s)") }))).into_response(),
        CommitError::Superseded { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response(),
        CommitError::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        CommitError::NotAMember { group, .. } => not_a_member_response(&group),
        CommitError::ElectorateNotGoverned { group, .. } => electorate_not_governed_response(group.as_deref()),
        CommitError::ElectorateStale { group, epoch, seen_epoch, .. } => electorate_stale_response(&group, epoch, seen_epoch),
        CommitError::ElectorateMismatch { group, detail, .. } => electorate_mismatch_response(group.as_deref(), &detail),
        // `CommitError` is `#[non_exhaustive]` in `mycelium-core`: a refusal this build does not
        // name is still a refusal, and its `Display` says what it is.
        other => (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": other.to_string() }))).into_response(),
    }
}

// ── Overlay: consistent KV ────────────────────────────────────────────────────

/// `POST /gateway/overlay/consistent/set` — consensus-durable KV write (ballot-serialized).
///
/// Body: `{"key": "K", "value_b64": "V"}`.
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_consistent_set(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let key = match body["key"].as_str() {
        Some(k) => k.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing key"}))).into_response(),
    };
    if let Some(refused) = refuse_protected_key(&key) {
        return refused;
    }
    let value = if let Some(b64) = body["value_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let slot = format!("consistent/{key}");
    let result = overlay_exclusive_propose(
        &ctx.agent_ctx, &slot, value.clone(),
        crate::consensus::ConsensusConfig::default(),
    ).await;

    let persisted = committed_bool(&result);
    match super::consensus_handle::receipt_from(result, &ctx.agent_ctx) {
        Ok(receipt) => {
            let key_arc: Arc<str> = Arc::from(key.as_str());
            let update = crate::framing::make_gossip_update(
                &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
                key_arc, value, false, &ctx.agent_ctx.hlc,
            );
            crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
            crate::framing::dispatch_gossip_try_send(
                &ctx.agent_ctx.gossip_txs,
                crate::framing::local_kv_wire_msg(&ctx.agent_ctx, update),
                ctx.agent_ctx.node_id.id_hash(),
                crate::framing::ForwardHint::All,
                &ctx.agent_ctx.kv_state.dropped_frames,
            );
            Json(commit_json(persisted, &receipt.local_durability)).into_response()
        }
        Err(err) => commit_error_response(err),
    }
}

/// `GET /gateway/overlay/consistent/get?key=K` — read latest ballot-committed value (local, eventually consistent).
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_consistent_get(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    // The same read the library's `consistent_get` makes (review D6): the slot's live decision — its
    // decision record, or the legacy reading for a slot with none — then the raw key. It read the raw
    // `consensus/committed/consistent/…` key, which disagreed with the library once decisions became
    // records (and never applied a lease).
    let value = crate::consensus::live_committed_value(
            &ctx.agent_ctx.kv_state, &format!("consistent/{}", q.key),
            crate::consensus::causal_now_ms(&ctx.agent_ctx.hlc))
        .or_else(|| {
            ctx.agent_ctx.kv_state.store.pin()
                .get(q.key.as_str())
                .and_then(|e| e.data.clone())
        });
    match value {
        Some(v) => Json(json!({ "found": true, "value_b64": base64::engine::general_purpose::STANDARD.encode(&v) })).into_response(),
        None    => Json(json!({ "found": false })).into_response(),
    }
}

// ── Overlay: distributed lock ─────────────────────────────────────────────────

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
pub(super) struct LockAcquireBody { name: String, ttl_secs: Option<u64> }

/// `POST /gateway/overlay/lock/acquire` — acquire a named distributed lock.
///
/// Body: `{"name": "N", "ttl_secs": 30}`.
/// Returns `{"guard_id": "…", "token": "N"}` — the token is a **decimal string** (the
/// fencing HLC exceeds JS safe-integer range; compare it as a big integer).
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_lock_acquire(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LockAcquireBody>,
) -> impl IntoResponse {
    let ttl_secs = body.ttl_secs.unwrap_or(30).clamp(1, 3600);
    let slot  = format!("lock/{}", body.name);
    // #164: `{holder}:{nonce}` value + a real consensus lease from ttl + converged-holder
    // confirmation — the same fix as the Rust `distributed_lock`. The pre-fix gateway lock had
    // all three bugs (no mutual exclusion, decorative ttl, unreleasable slot).
    let value = Bytes::from(
        format!("{}:{:016x}", ctx.agent_ctx.node_id, fastrand::u64(..)).into_bytes(),
    );
    let cfg = crate::consensus::ConsensusConfig {
        committed_lease_secs: Some(ttl_secs),
        ..crate::consensus::ConsensusConfig::default()
    };

    let started = mycelium_core::sim_seam::mono_instant();
    let result = overlay_exclusive_propose(&ctx.agent_ctx, &slot, value.clone(), cfg).await;

    match result {
        crate::consensus::ConsensusResult::Committed { .. } => {
            // The same guard the library issues (row A): ours only if the slot's top decision record
            // says so; the token is the decision's.
            let guard = match crate::agent::consensus_handle::lock_guard_after_commit(
                &ctx.agent_ctx, &body.name, &slot, value, started, std::time::Duration::from_secs(ttl_secs),
            ).await {
                Ok(g) => g,
                // The decision converged after the guard's own deadline (review D7): its own code.
                Err(crate::ConsistencyError::Timeout { .. }) => return (StatusCode::CONFLICT,
                    Json(json!({ "ok": false, "error": "expired_before_grant" }))).into_response(),
                Err(_) => return (StatusCode::CONFLICT,
                    Json(json!({ "ok": false, "error": "superseded" }))).into_response(),
            };
            let token = guard.token;
            let guard_id = format!("{:016x}", fastrand::u64(..));
            ctx.lock_guards.lock().unwrap_or_else(|e| e.into_inner()).insert(guard_id.clone(), guard);
            Json(json!({ "ok": true, "guard_id": guard_id, "token": token.to_string() })).into_response()
        }
        crate::consensus::ConsensusResult::Timeout { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("timeout after {ballots_tried} ballot(s)") }))).into_response(),
        crate::consensus::ConsensusResult::Superseded { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response(),
        crate::consensus::ConsensusResult::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        crate::consensus::ConsensusResult::ElectorateUnavailable {
            observed_members, declared_min, ..
        } => (StatusCode::CONFLICT, Json(json!({
            "ok": false,
            "error": "electorate_unavailable",
            "observed_members": observed_members,
            "declared_min": declared_min,
        }))).into_response(),
        crate::consensus::ConsensusResult::NotAMember { group, .. } => not_a_member_response(&group),
        crate::consensus::ConsensusResult::ElectorateNotGoverned { group, .. } => electorate_not_governed_response(group.as_deref()),
        crate::consensus::ConsensusResult::ElectorateStale { group, epoch, seen_epoch, .. } => electorate_stale_response(&group, epoch, seen_epoch),
        crate::consensus::ConsensusResult::ElectorateMismatch { group, detail, .. } => electorate_mismatch_response(group.as_deref(), &detail),
    }
}

/// `DELETE /gateway/overlay/lock/:guard_id` — release a held lock.
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_lock_release(
    Path(guard_id): Path<String>,
    State(ctx):     State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let removed = ctx.lock_guards.lock().unwrap_or_else(|e| e.into_inner()).remove(&guard_id);
    if removed.is_some() {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "guard_not_found" }))).into_response()
    }
}

// ── Overlay: leader election ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
pub(super) struct ElectBody {
    group:     String,
    /// The leadership's lease in seconds (row A, C1); default `DEFAULT_LEADER_LEASE` (30 s).
    #[serde(default)]
    ttl_secs:  Option<u64>,
    /// `true` asks for a permanent leadership — the pre-2.32.0 behaviour, now an explicit opt-in.
    #[serde(default)]
    permanent: bool,
}

/// `POST /gateway/overlay/elect` — elect a leader for `group`.
///
/// Body: `{"group": "G", "ttl_secs": 30, "permanent": false}` (both optional).
/// Returns `{"leader": "IP:PORT"}` on success. **Leased by default** (since 2.32.0, row A C1): the
/// leadership lapses `ttl_secs` after its commit unless re-elected (calling again renews it); step
/// down with `DELETE /gateway/overlay/elect/{group}`.
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_elect(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<ElectBody>,
) -> impl IntoResponse {
    let slot  = format!("leader/{}", body.group);
    let value = Bytes::from(ctx.agent_ctx.node_id.to_string().into_bytes());

    let term = if body.permanent {
        crate::agent::overlay_consistent::LeaderTerm::Permanent
    } else {
        crate::agent::overlay_consistent::LeaderTerm::Lease(body.ttl_secs.map_or(
            crate::agent::overlay_consistent::DEFAULT_LEADER_LEASE,
            |s| std::time::Duration::from_secs(s.clamp(1, 3600)),
        ))
    };
    let result = overlay_group_propose(
        &ctx.agent_ctx, &body.group, &slot, value,
        crate::consensus::ConsensusConfig { committed_lease_secs: term.lease_secs(), ..crate::consensus::ConsensusConfig::default() },
    ).await;

    // #164 class: an optimistic `Committed` is NOT mutually exclusive — never return `self`.
    // Let the winning commit converge, then read the AUTHORITATIVE leader from the committed slot
    // (mirrors `elect_leader` + `distributed_lock`; returning self split-brained — audit 2026-07-15).
    let converge = matches!(result, crate::consensus::ConsensusResult::Committed { .. });
    match result {
        crate::consensus::ConsensusResult::Committed { .. }
        | crate::consensus::ConsensusResult::Superseded { .. } => {
            if converge {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            // The live leader: a leadership whose lease lapsed or that was released is no leader.
            if let Some(raw) = crate::consensus::live_committed_value(
                    &ctx.agent_ctx.kv_state, &slot, crate::consensus::causal_now_ms(&ctx.agent_ctx.hlc))
                && let Ok(s) = std::str::from_utf8(&raw) {
                    return Json(json!({ "ok": true, "leader": s.to_string() })).into_response();
                }
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response()
        }
        crate::consensus::ConsensusResult::Timeout { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("timeout after {ballots_tried} ballot(s)") }))).into_response(),
        crate::consensus::ConsensusResult::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        // Nothing was proposed, so there is no leader. Answering from the committed slot here
        // would hand back a *stale* winner from an earlier, differently-constituted election —
        // which is the failure this refusal exists to prevent, one level down.
        crate::consensus::ConsensusResult::ElectorateUnavailable {
            observed_members, declared_min, ..
        } => (StatusCode::CONFLICT, Json(json!({
            "ok": false,
            "error": "electorate_unavailable",
            "observed_members": observed_members,
            "declared_min": declared_min,
            "detail": if observed_members == 0 {
                "the group roster is empty (unknown or unjoined group) — an election needs \
                 members, and absence is not authority"
            } else {
                "this node sees fewer members than the group declares; its view is partial"
            },
        }))).into_response(),
        // This node is not in the group: it may not elect a leader for it, least of all itself.
        crate::consensus::ConsensusResult::NotAMember { group, .. } => not_a_member_response(&group),
        crate::consensus::ConsensusResult::ElectorateNotGoverned { group, .. } => electorate_not_governed_response(group.as_deref()),
        crate::consensus::ConsensusResult::ElectorateStale { group, epoch, seen_epoch, .. } => electorate_stale_response(&group, epoch, seen_epoch),
        crate::consensus::ConsensusResult::ElectorateMismatch { group, detail, .. } => electorate_mismatch_response(group.as_deref(), &detail),
    }
}

/// `DELETE /gateway/overlay/elect/{group}` — this node steps down as `group`'s leader (row A, C1).
///
/// Returns `{"ok": true}` when this node was the live leader and its release is on stable storage;
/// `404 not_leader` when nothing was released — the live leader this node sees is not itself, or the
/// slot's lifecycle record names a newer decision or is behind a higher decided ballot; `500
/// release_unrecorded` when the release was applied and gossiped but the WAL did not acknowledge it.
///
/// **Who may call it:** any principal holding `consensus:write` may step **this node** down from any
/// group it leads, and — through `POST /gateway/overlay/elect` — elect it with any `ttl_secs` or
/// `"permanent": true`. The scope is node-wide, not per group (`rbac.md`).
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_elect_release(
    Path(group): Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let value = Bytes::from(ctx.agent_ctx.node_id.to_string().into_bytes());
    match crate::consensus::release_decision_durable(&ctx.agent_ctx, &format!("leader/{group}"), &value).await {
        crate::consensus::ReleaseOutcome::Released => Json(json!({ "ok": true })).into_response(),
        crate::consensus::ReleaseOutcome::Refused =>
            (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "not_leader" }))).into_response(),
        // Applied and gossiped, not on stable storage: a crash may restore the leadership here.
        crate::consensus::ReleaseOutcome::Unrecorded =>
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "ok": false, "error": "release_unrecorded" }))).into_response(),
    }
}

/// **403 `not_a_member`**: this node is not in the named group's roster, so it did not propose.
/// A proposer counts its own vote toward a quorum drawn from the roster; from outside it that vote
/// is one the electorate does not contain (`ConsensusResult::NotAMember`). An authority refusal,
/// so 403 like `governed_group`, not the 409 of a roster that could not be established.
#[cfg(feature = "consensus")]
pub(super) fn not_a_member_response(group: &str) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(json!({
        "ok": false,
        "error": "not_a_member",
        "group": group,
        "detail": "this node is not in the group's roster, so it may not propose to it — join the \
                   group first (POST /gateway/mesh/group, or /gateway/govern/group for a governed one)",
    }))).into_response()
}

/// **409 `electorate_stale`**: a member of the proposal's electorate answered for another epoch (P2). Nothing was
/// decided by this proposal; the electorate moved, and a retry reads the new one.
#[cfg(feature = "consensus")]
pub(super) fn electorate_stale_response(group: &str, epoch: u64, seen_epoch: u64) -> axum::response::Response {
    (StatusCode::CONFLICT, Json(json!({
        "ok": false, "error": "electorate_stale", "group": group, "epoch": epoch, "seen_epoch": seen_epoch,
    }))).into_response()
}

/// **409 `electorate_mismatch`**: the electorate and this node's view disagree (P2) — `detail` says how. Nothing was
/// proposed.
#[cfg(feature = "consensus")]
pub(super) fn electorate_mismatch_response(group: Option<&str>, detail: &str) -> axum::response::Response {
    (StatusCode::CONFLICT, Json(json!({
        "ok": false, "error": "electorate_mismatch", "group": group, "detail": detail,
    }))).into_response()
}

/// **403 `electorate_not_governed`**: an exclusive outcome (a lock, a leader, a consistent write) was asked of a
/// scope with no electorate declaration — the whole cluster, or an undeclared group — on a node that requires one
/// (`consensus_require_electorate`, post-360 plan row P2). Nothing was proposed. An authority refusal, like
/// `not_a_member`: the fix is a governance act, not a retry.
#[cfg(feature = "consensus")]
pub(super) fn electorate_not_governed_response(group: Option<&str>) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(json!({
        "ok": false,
        "error": "electorate_not_governed",
        "group": group,
        "detail": match group {
            Some(_) => "this group has no electorate declaration, and this node requires one for an exclusive \
                        outcome — declare it through POST /gateway/govern/electorate (govern:write)",
            None => "the whole cluster is not an electorate group, and this node requires one for an exclusive \
                     outcome — declare an electorate group with `exclusive_default`",
        },
    }))).into_response()
}

// ── Overlay: ordered log ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct LogAppendBody { stream: String, value_b64: Option<String> }

/// `POST /gateway/overlay/log/append` — append to `stream`.
///
/// Body: `{"stream": "S", "value_b64": "V"}`.
/// Returns `{"hlc": N}`.
pub(super) async fn gw_overlay_log_append(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LogAppendBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    if let Some(refused) = refuse_owned_stream(&body.stream) {
        return refused;
    }
    let value = if let Some(b64) = body.value_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let hlc = ctx.agent_ctx.hlc.tick();
    // Salt with the node id so two nodes appending in the same wall-ms don't collide on one key
    // and silently drop an entry via LWW (audit 2026-07-15); HLC stays the first key segment.
    let node = &ctx.agent_ctx.node_id;
    let key: Arc<str> = Arc::from(format!("log/{}/{hlc:016x}/{node}", body.stream).as_str());
    let update = crate::framing::make_gossip_update(
        &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
        key, value, false, &ctx.agent_ctx.hlc,
    );
    crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
    crate::framing::dispatch_gossip_try_send(
        &ctx.agent_ctx.gossip_txs,
        crate::framing::local_kv_wire_msg(&ctx.agent_ctx, update),
        ctx.agent_ctx.node_id.id_hash(),
        crate::framing::ForwardHint::All,
        &ctx.agent_ctx.kv_state.dropped_frames,
    );
    Json(json!({ "hlc": hlc })).into_response()
}

#[derive(Deserialize)]
pub(super) struct LogScanQuery { stream: String, from: Option<u64>, to: Option<u64> }

/// `GET /gateway/overlay/log/scan?stream=S&from=0&to=MAX` — range scan.
///
/// Returns `[{"hlc": N, "value_b64": "…"}]`.
pub(super) async fn gw_overlay_log_scan(
    Query(q):   Query<LogScanQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let from = q.from.unwrap_or(0);
    let to   = q.to.unwrap_or(u64::MAX);
    // `(hlc, key)`: the core's order (`KvHandle::scan_log`), so two same-HLC entries come back in the
    // same order on every node and every call (post-360 row E).
    let entries: Vec<LogEntry> = mycelium_core::log_cursor::rows_after(
            &ctx.agent_ctx.kv_state, &q.stream, &mycelium_core::log_cursor::LogPosition::Start)
        .into_iter()
        .filter(|r| r.hlc >= from && r.hlc < to)
        .map(|r| LogEntry { hlc: r.hlc, value: r.value })
        .collect();
    let arr: Vec<serde_json::Value> = entries.iter().map(|e| json!({
        "hlc":       e.hlc,
        "value_b64": base64::engine::general_purpose::STANDARD.encode(&e.value),
    })).collect();
    Json(arr).into_response()
}

#[derive(Deserialize)]
pub(super) struct LogCompactBody { stream: String, before_hlc: u64 }

/// `POST /gateway/overlay/log/compact` — tombstone entries with HLC < `before_hlc`.
pub(super) async fn gw_overlay_log_compact(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LogCompactBody>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_owned_stream(&body.stream) {
        return refused;
    }
    let prefix = format!("log/{}/", body.stream);
    for (k, _) in crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, &prefix) {
        let suffix = k.strip_prefix(&prefix).unwrap_or("");
        if let Some(hlc) = suffix.split('/').next().and_then(|s| u64::from_str_radix(s, 16).ok())
            && hlc < body.before_hlc {
                let update = crate::framing::make_gossip_update(
                    &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
                    k, Bytes::new(), true, &ctx.agent_ctx.hlc,
                );
                crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
                crate::framing::dispatch_gossip_try_send(
                    &ctx.agent_ctx.gossip_txs,
                    crate::framing::local_kv_wire_msg(&ctx.agent_ctx, update),
                    ctx.agent_ctx.node_id.id_hash(),
                    crate::framing::ForwardHint::All,
                    &ctx.agent_ctx.kv_state.dropped_frames,
                );
            }
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub(super) struct LogSubscribeQuery { stream: String, since: Option<u64> }

/// `GET /gateway/overlay/log/subscribe?stream=S&since=0` — SSE stream of log entries.
pub(super) async fn gw_overlay_log_subscribe(
    Query(q):   Query<LogSubscribeQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let prefix      = format!("log/{}/", q.stream);
    let prefix_arc: Arc<str> = Arc::from(prefix.as_str());
    let mut watcher  = super::capability_ops::subscribe_prefix_on_kv(&ctx.agent_ctx.kv_state, Arc::clone(&prefix_arc));
    let stream_name  = q.stream.clone();
    let kv_state     = Arc::clone(&ctx.agent_ctx.kv_state);
    let mut last_seen = q.since.unwrap_or(0);

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    tokio::spawn(async move {
        loop {
            // `(hlc, key)` order, as the core's `subscribe_log` (post-360 row E).
            let entries: Vec<LogEntry> = mycelium_core::log_cursor::rows_after(
                    &kv_state, &stream_name, &mycelium_core::log_cursor::LogPosition::Start)
                .into_iter()
                .filter(|r| r.hlc >= last_seen)
                .map(|r| LogEntry { hlc: r.hlc, value: r.value })
                .collect();
            for entry in entries {
                use base64::Engine as _;
                // `saturating_add`: the HLC is parsed from the (any-node-writable) log key, so a
                // crafted `log/{stream}/ffffffffffffffff/...` entry gives `hlc == u64::MAX`; `+ 1`
                // panicked (overflow-checks → node-abort) or wrapped to 0 (release → the cursor resets
                // and the subscriber re-floods the whole stream on every change) (audit 2026-07-15 pass 4).
                last_seen = entry.hlc.saturating_add(1);
                let data  = json!({
                    "stream":    stream_name,
                    "hlc":       entry.hlc,
                    "value_b64": base64::engine::general_purpose::STANDARD.encode(&entry.value),
                });
                if tx.send(Event::default().data(data.to_string())).await.is_err() { return; }
            }
            if watcher.changed().await.is_err() { return; }
        }
    });

    Sse::new(ReceiverStream::new(rx).map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
pub(super) struct LogGroupSubscribeQuery { stream: String, group: String }

/// `GET /gateway/overlay/log/group/subscribe?stream=S&group=G` — **single-active, exact-once**
/// ordered log consumption over SSE.
///
/// **Contract:** at most one consumer of `(stream, group)` is active at a time; it receives every
/// entry in HLC order, exactly once, and another subscriber takes over if it dies (failover). This
/// is *not* a load-balanced work queue — the group does not share entries across active consumers.
/// (For competitive, load-balanced exactly-once *work distribution*, use the `mycelium-tuple-space`
/// companion, which claims each item atomically. A single advancing offset cannot do per-item
/// competitive consumption — that is a different pattern; see the `log-group vs work-queue` note.)
///
/// **How it's achieved (#149):** the claim `clog/{stream}/{group}/claim` is a **leased consensus
/// commitment** (`committed_lease_secs`). Because two near-simultaneous proposers can both
/// *optimistically* commit — each checks only its *local* committed view at commit time (the
/// ~40% double-acquire race) — the propose return is not trusted for exclusivity. Instead, after a
/// commit we read the **converged committed holder** (`live_committed_value`): commit-keys are
/// LWW-resolved by HLC, so exactly one holder converges, and only that node proceeds; losers stand
/// by *without releasing* (their losing commit is LWW-overwritten by the winner's — a tombstone
/// would clear the winner's claim). The winner drains with a **private local offset** (exact-once
/// by construction — no cross-consumer offset hand-off) and **renews the lease** while active; on
/// its death the lease lapses and a standby takes over (failover). The earlier code used a
/// bare-LWW "lock" (no mutual exclusion → every consumer drained the whole stream).
#[cfg(feature = "consensus")]
pub(super) async fn gw_overlay_log_group_subscribe(
    Query(q):   Query<LogGroupSubscribeQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    Sse::new(log_group_events(Arc::clone(&ctx.agent_ctx), q.stream, q.group).map(|data| Ok(Event::default().data(data.to_string()))))
        .keep_alive(KeepAlive::default())
}

/// The event stream behind `GET /gateway/overlay/log/group/subscribe`: each item is one entry's data object.
#[cfg(feature = "consensus")]
pub(super) fn log_group_events(
    task_ctx:    Arc<TaskCtx>,
    stream_name: String,
    group_name:  String,
) -> impl futures_util::Stream<Item = serde_json::Value> {
    use mycelium_core::log_cursor::{self, LogPosition, LogRow};
    let kv_state    = Arc::clone(&task_ctx.kv_state);
    // False once this consumer has lost the claim: what is still queued is not handed on (nor
    // recorded) — the next holder delivers it.
    let active      = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let still_active = Arc::clone(&active);
    let (record_ctx, record_stream, record_group) = (Arc::clone(&task_ctx), stream_name.clone(), group_name.clone());
    // Checked again as each entry is handed on: a client too slow to drain the channel stalls the task's
    // send, and with it the lease renewal — a lease that lapsed meanwhile may be another consumer's now.
    let claim_check = (Arc::clone(&task_ctx), format!("clog/{stream_name}/{group_name}/claim"),
                       Bytes::from(task_ctx.node_id.to_string().into_bytes()));

    let (tx, rx) = tokio::sync::mpsc::channel::<(serde_json::Value, LogRow)>(64);
    tokio::spawn(async move {
        let claim_slot = format!("clog/{stream_name}/{group_name}/claim");
        // Stable claim value = holder id only (no expiry inside the value), so a re-propose is the
        // "same value" the lease path re-endorses; the lease governs expiry + failover.
        let holder: Bytes = Bytes::from(task_ctx.node_id.to_string().into_bytes());
        let lease_secs: u64 = 30;
        // A single active consumer is an exclusive outcome (P2): flagged, and decided where the locks are.
        let mk_cfg = || crate::consensus::ConsensusConfig {
            committed_lease_secs: Some(lease_secs),
            safety_sensitive: true,
            ..crate::consensus::ConsensusConfig::default()
        };

        // Exact-once across a consumer group = a SINGLE active consumer (issue #149). The claim is a
        // *leased* consensus commitment. Two near-simultaneous proposers can both *optimistically*
        // commit (each checks only its local committed view at commit time), so the propose return
        // cannot be trusted for exclusivity — that is the ~40% double-acquire race. But the
        // commit-keys are LWW-resolved by HLC, so the CONVERGED holder is deterministic: propose,
        // let it converge, then read the authoritative committed holder. Exactly one wins; losers
        // stand by WITHOUT releasing (their losing commit is LWW-overwritten by the winner's —
        // tombstoning would clear the winner's claim). The lease gives failover if the holder dies.
        'acquire: loop {
            if tx.is_closed() { return; }
            let won = matches!(
                overlay_exclusive_propose(&task_ctx, &claim_slot, holder.clone(), mk_cfg()).await,
                crate::consensus::ConsensusResult::Committed { .. },
            );
            if won {
                tokio::time::sleep(Duration::from_millis(1000)).await; // let the winning commit converge
                let is_me = crate::consensus::live_committed_value(
                        &kv_state, &claim_slot, crate::consensus::causal_now_ms(&task_ctx.hlc))
                    .as_deref() == Some(holder.as_ref());
                if is_me { break 'acquire; }
            }
            // Lost the race, or a live lease is held elsewhere — stand by; retry after it may lapse.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Resume from the group's recorded position (LWW read; the sole holder is the only writer),
        // `(hlc, key)` — the order the core sorts a log by (post-360 row E, `log_cursor`). The
        // position below is this task's *queued* position; the recorded one moves only as the stream
        // hands an entry on (the `map` after the channel).
        let mut position: LogPosition = log_cursor::read_position(&kv_state, &stream_name, &group_name);
        let renew_every = Duration::from_secs((lease_secs / 3).max(1));
        let mut last_renew = std::time::Instant::now();

        loop {
            // Renew the lease while active (re-propose the SAME holder value → re-endorsed while
            // live; a different proposer is superseded). If we somehow lost the claim (a partition
            // let another win), stop — a single active consumer is the invariant.
            if last_renew.elapsed() >= renew_every {
                let _ = overlay_exclusive_propose(&task_ctx, &claim_slot, holder.clone(), mk_cfg()).await;
                last_renew = std::time::Instant::now();
                let still_me = crate::consensus::live_committed_value(
                        &kv_state, &claim_slot, crate::consensus::causal_now_ms(&task_ctx.hlc))
                    .as_deref() == Some(holder.as_ref());
                if !still_me {
                    active.store(false, std::sync::atomic::Ordering::Release);
                    return;
                }
            }

            let next = log_cursor::rows_after(&kv_state, &stream_name, &position).into_iter().next();
            if let Some(row) = next {
                position = LogPosition::past(&row);
                use base64::Engine as _;
                let data = json!({
                    "stream":    stream_name,
                    "hlc":       row.hlc,
                    "value_b64": base64::engine::general_purpose::STANDARD.encode(&row.value),
                });
                if tx.send((data, row)).await.is_err() { return; }
            } else {
                // Idle: the drain loop otherwise only notices a disconnected client via a failed
                // `tx.send`, which never fires while the stream is idle — so the task looped forever
                // RENEWING the exclusive `clog/{stream}/{group}/claim` lease, permanently blocking
                // failover to a standby consumer (audit 2026-07-15 pass 4). Check the channel here so a
                // client that disconnects during an idle stream releases the claim (task returns → lease
                // stops renewing → it expires and a standby can acquire).
                if tx.is_closed() { return; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    ReceiverStream::new(rx)
        .take_while(move |_| {
            let (ctx, slot, holder) = &claim_check;
            still_active.load(std::sync::atomic::Ordering::Acquire)
                && crate::consensus::live_committed_value(&ctx.kv_state, slot, crate::consensus::causal_now_ms(&ctx.hlc))
                    .as_deref() == Some(holder.as_ref())
        })
        .map(move |(data, row)| {
            // Post-360 row E: the position moves as the stream hands the entry to the connection, not
            // when it was queued — an entry queued (up to 64) or in flight when the stream dropped is
            // the next holder's to send.
            log_cursor::record(&record_ctx.core, &record_stream, &record_group, &row);
            data
        })
}

// ── Overlay: reliable delivery ────────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct EmitReliableBody {
    target:       String,
    kind:         String,
    payload_b64:  Option<String>,
    timeout_secs: Option<u64>,
    /// Adopter-surface Y11: the causal fields (`correlation`, `parent`), read only when an exposure rule selects
    /// `overlay.emit_reliable`.
    #[serde(default, rename = "_meta")]
    meta:         Option<serde_json::Value>,
}

/// `POST /gateway/overlay/emit_reliable` — send with explicit ACK.
///
/// Body: `{"target": "IP:PORT", "kind": "K", "payload_b64": "V", "timeout_secs": 5}`, and `_meta` with the causal
/// fields where an exposure rule selects `overlay.emit_reliable` (Y11).
/// Returns `{"ack": "acknowledged" | "timeout"}`.
pub(super) async fn gw_overlay_emit_reliable(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<EmitReliableBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);
    let target: crate::node_id::NodeId = match body.target.parse() {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid target node id"}))).into_response(),
    };
    let payload = if let Some(b64) = body.payload_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    let timeout = Duration::from_secs(body.timeout_secs.unwrap_or(5).clamp(1, 300));
    let kind: Arc<str> = Arc::from(body.kind.as_str());
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }

    // Adopter-surface Y11: decided only when an exposure rule selects `overlay.emit_reliable` on this kind, the body's
    // `_meta` its causal fields.
    let params = match &body.meta {
        Some(m) => json!({ "_meta": m }),
        None => json!({}),
    };
    let door = crate::agent::door::decide(
        &ctx.agent_ctx, crate::agent::door::DoorActor::Caller(caller.as_ref()), crate::agent::door::OP_OVERLAY_EMIT_RELIABLE,
        &crate::agent::door::signal_resource(&kind), &params, "gateway:overlay/emit_reliable",
    ).await;
    let pass = match door {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    // #708's review, finding 1: a reliable emit of a companion's door kind runs that door's decision as well, always —
    // a take rule stricter than the overlay's, or an evaluator denying the take, still applies (round 3: one act, two
    // records and two counts when both doors are selected).
    let kind_door = match crate::agent::door::decide_raw(&ctx.agent_ctx, caller.as_ref(), &kind, &payload, &params, "gateway:overlay/emit_reliable").await {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    let dispatched = gateway_caller::gateway_rpc_call(&ctx.agent_ctx, caller.as_ref(), target, kind, payload, timeout).await;
    let outcome = super::mesh::door_outcome(&dispatched);
    pass.finish(outcome).await;
    if let Some(p) = kind_door {
        p.finish(outcome).await;
    }
    match dispatched {
        Ok(_) => Json(json!({ "ack": "acknowledged" })).into_response(),
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => Json(json!({ "ack": "timeout" })).into_response(),
        Err(e) => dispatch_refused(e),
    }
}
