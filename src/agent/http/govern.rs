//! Governance (`/gateway/govern*`, `/gateway/mesh/group`) and the fleet's legible reads (`/gateway/fleet`,
//! `/explain`, `/diagnose`, `/guarantees`).

use super::*;

// ── WS-C governance: management = intent + local reconcile ───────────────────
//
// A HITL operator (or an agent with a concern) publishes an evaporating fleet
// *intent* over the gossip KV; every node reconciles it locally, local pins win,
// and the intent self-heals away if the publisher vanishes. These routes are the
// publish surface (POST) plus an effective-state snapshot (GET). They never command
// a node — they only seed soft-state that nodes choose to honour (Principles 1 & 5).

/// Record a governance change in the tamper-evident audit trail. It needs `compliance` and a `[tls]` identity
/// to seal; a change that leaves no record — either missing, or a seal that failed — is counted
/// (`governance_unaudited` on `/stats`) and, when the seal failed, warned about, rather than vanishing.
pub(super) fn audit_govern(ctx: &Arc<TaskCtx>, target: &str, detail: String) {
    use std::sync::atomic::Ordering::Relaxed;
    ctx.governance_changes.fetch_add(1, Relaxed);
    #[cfg(feature = "compliance")]
    let recorded = match super::audit::seal_and_write(
        ctx,
        super::audit::AuditAction::Admin,
        "gateway/govern",
        target,
        super::audit::AuditOutcome::Success,
        Some(detail),
    ) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(target, "governance change applied but not audited: {e}");
            false
        }
    };
    #[cfg(not(feature = "compliance"))]
    let recorded = { let _ = (target, detail); false };
    if !recorded {
        ctx.governance_unaudited.fetch_add(1, Relaxed);
    }
}

/// Parse an optional `"target"` field into a node id. `Err` carries a message the
/// caller turns into a 400 (a small error type keeps the `Result` cheap).
///
/// A `target` that is present but not a node-id string is refused: read as *no* target it governed the
/// whole fleet (the reviews of #544).
pub(super) fn parse_optional_target(body: &serde_json::Value) -> Result<Option<crate::node_id::NodeId>, &'static str> {
    match body.get("target") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some(s) => s.parse().map(Some).map_err(|_| "invalid target node id"),
            None => Err("`target` must be a node id string (host:port)"),
        },
    }
}

/// A governance body must be an object naming only the fields its route reads: a misspelled field was
/// ignored and the intent published without it.
pub(super) fn refuse_unknown_fields(body: &serde_json::Value, known: &[&str]) -> Option<axum::response::Response> {
    let Some(obj) = body.as_object() else {
        return Some((StatusCode::BAD_REQUEST, Json(json!({"error": "the body must be a JSON object"}))).into_response());
    };
    obj.keys().find(|k| !known.contains(&k.as_str())).map(|k| {
        (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown field `{k}`")}))).into_response()
    })
}

/// An optional non-negative integer field: absent or `null` is `None`; anything else that is not a `u64`
/// is refused rather than read as absent.
pub(super) fn optional_u64(v: Option<&serde_json::Value>, name: &str) -> Result<Option<u64>, String> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("{name} must be a non-negative integer")),
    }
}

pub(super) fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

/// `GET /gateway/govern` — this node's **effective** tuning-governor state (the
/// reconciled result of local pins + the current fleet intent). Per-node by design;
/// scrape every node for the fleet picture (there is no central view — Principle 1).
pub(super) async fn gw_govern_snapshot(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    let snap = ctx.agent_ctx.tuning_governor.snapshot();
    let params: Vec<_> = snap
        .params
        .iter()
        .map(|p| {
            json!({
                "param":          p.param.key(),
                "floor":          p.floor,
                "ceiling":        p.ceiling,
                "ratchet":        format!("{:?}", p.ratchet).to_lowercase(),
                "locally_pinned": p.locally_pinned,
                "pending":        p.pending,
            })
        })
        .collect();
    // Item 4 §7: the node's control profile and the tripwires an operator watches before stepping
    // it (`docs/operations/control-profiles.md`). The counters keep their names across the ladder;
    // `profile` says which reading they are.
    let relaxed = std::sync::atomic::Ordering::Relaxed;
    let control = json!({
        "profile":                 crate::control::Profile::from_u8(ctx.agent_ctx.control_profile.load(relaxed)).name(),
        "would_hold":              ctx.agent_ctx.control_would_hold.load(relaxed),
        "opacity_releases_spaced": ctx.agent_ctx.opacity_releases_spaced.load(relaxed),
        "tuning": {
            "profile":          snap.profile.name(),
            "held_by_spacing":  snap.held_by_spacing,
            "held_by_settling": snap.held_by_settling,
            "settled_unknown":  snap.settled_unknown,
        },
    });
    // P2: the electorate declarations this node sees — each group, its epoch, whether it holds the default, its
    // members (node ids: a governance read, behind `govern:read`), its predecessors, where it retired to, and where
    // its migration stands per predecessor (waiting · closed · carrying n of |S| · complete).
    let electorates: Vec<_> = super::electorate::all_views(&ctx.agent_ctx).into_iter()
        .map(|d| {
            #[cfg(feature = "consensus")]
            let migration = super::migration::state(&ctx.agent_ctx, &d.group);
            #[cfg(not(feature = "consensus"))]
            let migration: Vec<serde_json::Value> = Vec::new();
            json!({
                "group": d.group, "epoch": d.epoch, "exclusive_default": d.exclusive_default,
                "members": d.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "predecessors": d.predecessors.iter().cloned().collect::<Vec<_>>(),
                "retired_to": super::electorate::retirement(&ctx.agent_ctx, &d.group).map(|r| r.successor.clone()),
                "migration": migration,
            })
        })
        .collect();
    Json(json!({
        "node_id":      ctx.agent_ctx.node_id.to_string(),
        "auto_enabled": snap.auto_enabled,
        "params":       params,
        "control":      control,
        "electorates":  electorates,
    }))
    .into_response()
}

/// `POST /gateway/govern/profile` — step **this node's** control profile along the ADR's ladder
/// (§7: `legacy` · `observe` · `enforce-local` · `enforce-allocated`; the rollout is
/// `docs/operations/control-profiles.md`). Body `{"profile": "observe"}`. Per node, like every
/// govern route, and it takes effect on each governor's next pass. An unknown name is `400` and
/// changes nothing — never read as `legacy`. Scope `govern:write`.
pub(super) async fn gw_govern_profile(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::control::Profile;
    if let Some(refused) = refuse_unknown_fields(&body, &["profile"]) {
        return refused;
    }
    let Some(name) = body.get("profile").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing 'profile'"}))).into_response();
    };
    let Some(profile) = Profile::parse(name) else {
        let known: Vec<&str> = Profile::ALL.iter().map(|p| p.name()).collect();
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown profile '{name}'"), "known": known})))
            .into_response();
    };
    let was = Profile::from_u8(ctx.agent_ctx.control_profile.load(std::sync::atomic::Ordering::Relaxed));
    ctx.agent_ctx.set_control_profile(profile);
    audit_govern(&ctx.agent_ctx, "govern/profile", body.to_string());
    Json(json!({ "ok": true, "profile": profile.name(), "was": was.name() })).into_response()
}

/// `GET /gateway/fleet` — the Legible-Emergence Phase-2 **relational fleet snapshot**: the
/// operator's "localize" view, computed **locally** from the gossiped KV this node already holds
/// (no collector — any node answers it, and it survives killing any node; Principle 1). Governed-
/// group status (intent vs observed), capability-coverage gaps, fleet opacity, and the flap/
/// oscillation counters — each paired with the RT1/RT2 `view_confidence` header (a per-node
/// *estimate*, not fleet ground truth; at convergence the *diagnosis* agrees across nodes while
/// `view_confidence` stays each observer's own). Scope `fleet:read`.
pub(super) async fn gw_fleet_snapshot(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::emergent::compute_fleet_snapshot(&ctx.agent_ctx)).into_response()
}

/// `GET /gateway/diagnose` — the Legible-Emergence Phase-4 **fleet narrative**: the "why is the
/// fleet in this state" diagnosis. A templated rule engine over the Phase-2 snapshot (one rule per
/// Phase-0 pathology) that names each cause in code-free, actionable terms — the artifact an on-call
/// engineer who did not build the system can act on. Every diagnosis is qualified by the observer's
/// own RT1/RT2 view health (`caveat`), so a clean read from a blind node is not mistaken for a
/// healthy fleet. Scope `fleet:read`.
/// `GET /gateway/guarantees` — the node's guarantee report (plan I2), recomputed live: every
/// registered guarantee resolved to `enforced` / `not_configured` / `not_in_build` /
/// `not_applicable` (with the role fact) / `not_verifiable_here`, with the configuration digest it was
/// computed over. Read-only; scope `fleet:read`.
pub(super) async fn gw_guarantees(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::guarantee::report(&ctx.agent_ctx)).into_response()
}

pub(super) async fn gw_diagnose(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::emergent::compute_fleet_diagnosis(&ctx.agent_ctx)).into_response()
}

/// `GET /gateway/explain?since=<hlc>` — the Legible-Emergence Phase-3 causal **explain**: the
/// HLC-ordered narrative of significant fleet events (`?since` filters to `hlc >= since`; default
/// all). Fans a best-effort `sys.explain` RPC out to a **capped** subset of known peers
/// (`EXPLAIN_MAX_FANOUT`, so the query never becomes an O(N) RPC storm), merges each node's ring into
/// one causal stream, and — RT3 — names both the peers that did not answer (`non_responders`) and the
/// count skipped by the cap (`not_queried`) rather than silently dropping either. Scope `fleet:read`.
pub(super) async fn gw_explain(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let since = q.get("since").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Json(super::emergent::assemble_explain(&ctx.agent_ctx, since).await).into_response()
}

/// `POST /gateway/govern/tuning` — publish a cluster-wide (or `target`-ed) tuning
/// intent. Body:
/// ```json
/// {"enabled": true,
///  "params": [{"param": "writer_depth", "floor": 1024, "ceiling": 8192, "ratchet": "up"}],
///  "target": "10.0.0.5:9000"}
/// ```
/// All fields optional except that at least one of `enabled` / `params` must be present.
pub(super) async fn gw_govern_tuning(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use super::tuning_governor::{GovernIntent, HotParam, ParamDirective, Ratchet};

    if let Some(refused) = refuse_unknown_fields(&body, &["enabled", "params", "target"]) {
        return refused;
    }
    let enabled = match body.get("enabled") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_bool() {
            Some(b) => Some(b),
            None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "`enabled` must be a boolean"}))).into_response(),
        },
    };

    let mut params = Vec::new();
    let params_in = match body.get("params") {
        None | Some(serde_json::Value::Null) => &[][..],
        Some(serde_json::Value::Array(a)) => a.as_slice(),
        Some(_) => return bad_request("`params` must be an array of directives".into()),
    };
    {
        for d in params_in {
            if let Some(refused) = refuse_unknown_fields(d, &["param", "floor", "ceiling", "ratchet"]) {
                return refused;
            }
            let Some(pkey) = d.get("param").and_then(|v| v.as_str()) else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "param directive missing 'param'"}))).into_response();
            };
            if HotParam::from_key(pkey).is_none() {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown param '{pkey}'")}))).into_response();
            }
            let ratchet = match d.get("ratchet") {
                None | Some(serde_json::Value::Null) => Ratchet::Off,
                Some(v) => match v.as_str() {
                    Some("up")   => Ratchet::Up,
                    Some("down") => Ratchet::Down,
                    Some("off")  => Ratchet::Off,
                    Some(other)  => return bad_request(format!("unknown ratchet '{other}'")),
                    None         => return bad_request("`ratchet` must be \"up\", \"down\" or \"off\"".into()),
                },
            };
            let floor = match optional_u64(d.get("floor"), "floor") { Ok(v) => v, Err(e) => return bad_request(e) };
            let ceiling = match optional_u64(d.get("ceiling"), "ceiling") { Ok(v) => v, Err(e) => return bad_request(e) };
            params.push(ParamDirective { param: pkey.to_string(), floor, ceiling, ratchet });
        }
    }

    if enabled.is_none() && params.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "intent must set 'enabled' or 'params'"}))).into_response();
    }

    let target = match parse_optional_target(&body) {
        Ok(t)  => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };

    let intent = GovernIntent { enabled, params, written_at_ms: 0, target };
    let kv = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&ctx.agent_ctx.core));
    let ok = super::intent::publish_intent(&kv, super::tuning_governor::GOVERN_FLEET_KEY, intent);
    audit_govern(&ctx.agent_ctx, super::tuning_governor::GOVERN_FLEET_KEY, body.to_string());
    Json(json!({"ok": ok, "key": super::tuning_governor::GOVERN_FLEET_KEY})).into_response()
}

/// `POST /gateway/govern/timing` — publish a cluster-wide (or `target`-ed) **timing** intent
/// (WS-C / M10.2). Body:
/// ```json
/// {"health_check_interval_secs": 2, "reconnect_backoff_secs": 3, "target": null}
/// ```
/// `0`/absent for a field leaves it as it is; `target` `null` = whole fleet. Newest-wins,
/// local-wins (a node that called a `set_*` setter ignores it), evaporating. No consensus fence.
pub(super) async fn gw_govern_timing(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // A present field must be a non-negative integer: `as_u64().unwrap_or(0)` read `"30"` or `-5` as
    // `0`, "ungoverned", and answered success for an intent that governs nothing. The same for the shape
    // around them: a non-object, a misspelled field, or a `target` that is not a node id (read as *no*
    // target — the whole fleet) each published something other than what was asked.
    if let Some(refused) = refuse_unknown_fields(&body, &["health_check_interval_secs", "reconnect_backoff_secs", "target"]) {
        return refused;
    }
    let mut fields = [0u64; 2];
    for (slot, name) in fields.iter_mut().zip(["health_check_interval_secs", "reconnect_backoff_secs"]) {
        match body.get(name) {
            None | Some(serde_json::Value::Null) => {}
            Some(v) => match v.as_u64() {
                Some(n) => *slot = n,
                None => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("{name} must be a non-negative integer")}))).into_response(),
            },
        }
    }
    let [health, reconnect] = fields;
    let target = match parse_optional_target(&body) {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let intent = super::timing_governor::TimingIntent {
        health_check_interval_secs: health,
        reconnect_backoff_secs: reconnect,
        target,
        written_at_ms: 0,
    };
    match super::timing_governor::publish_timing_intent(&ctx.agent_ctx, intent) {
        Ok(ok) => {
            // Audited as its siblings are; it never was (the third review of #544).
            audit_govern(&ctx.agent_ctx, super::timing_governor::TIMING_INTENT_KEY, body.to_string());
            Json(json!({ "published": ok, "key": super::timing_governor::TIMING_INTENT_KEY })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// `POST /gateway/govern/topology-override` — set or release a group's Hard-topology escape hatch.
/// Body: `{"group": "G", "override": true|false}`. `true` writes `sys/topology-override/{group}` = `b"true"`
/// (the only value the consensus gate reads as active); `false` tombstones it. Scope `govern:write`, audited:
/// relaxing the failure-domain gate is a governance act, and until 2026-10-07 it was an unaudited `kv:write`.
pub(super) async fn gw_govern_topology_override(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_unknown_fields(&body, &["group", "override"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|v| v.as_str()) else {
        return bad_request("missing 'group'".into());
    };
    if group.is_empty() || group.contains('/') {
        return bad_request("`group` must be a non-empty group name without '/'".into());
    }
    let Some(engage) = body.get("override").and_then(|v| v.as_bool()) else {
        return bad_request("`override` must be a boolean".into());
    };
    let key = format!("sys/topology-override/{group}");
    if engage {
        kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::from_static(b"true"), false);
    } else {
        kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::new(), true);
    }
    audit_govern(&ctx.agent_ctx, &key, body.to_string());
    Json(json!({ "ok": true, "key": key, "override": engage })).into_response()
}

/// `POST /gateway/govern/membership` — publish an elastic-sizing intent for a group.
/// Body:
/// ```json
/// {"group": "workers", "min": 3, "max": 10, "drain": ["10.0.0.5:9000"], "target": null}
/// ```
/// `group` + `min` required; `max` `null`/absent = unbounded; `drain` cooperative self-removal.
#[derive(Deserialize)]
pub(super) struct GroupBody { group: String }

/// `POST /gateway/mesh/group` — **this node** joins signal-boundary group `{group}`.
///
/// Body: `{"group": "G"}`. Returns `{"ok": true, "group": "G", "members": [...]}`.
///
/// **A node joins itself, and there is no verb for enrolling another node.** That is the shape the
/// substrate already commits to everywhere else — an agent promises only its own behaviour — and it
/// is why this route takes no node id. Membership is published at `grp/{group}/{self}` and gossips
/// like any other soft state.
///
/// **Why this route exists (2026-09-24).** It did not, and its absence was a finding: the gateway
/// offered `POST /gateway/overlay/elect` over a group while providing no supported way for an HTTP
/// caller to *populate* one. The only roster an HTTP client could reach was the empty one — which
/// was precisely the state that used to confer solo authority
/// (`ConsensusResult::ElectorateUnavailable`). An election surface without a membership surface is
/// a surface that can only be used wrongly.
///
/// **Who may change an electorate (decided 2026-10-08).** A group under a live membership intent is *governed*: its
/// members are the roster and quorum its elections count, so moving this node in or out of one is a governance act —
/// this route refuses it **403** `governed_group`, and `POST`/`DELETE /gateway/govern/group` (`govern:write`) does it.
/// A plain group stays a `mesh:write` join. Every membership change through either route is audited (or counted on
/// `/stats` when it cannot be). *Which* membership version an election is decided against stays open
/// (`docs/wiki/dev/.log/2026-09-24-consensus-vote-binding.md`). An embedded caller holding the handle is not a
/// gateway caller and is not covered here.
#[cfg(feature = "gateway")]
pub(super) async fn gw_group_join(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<GroupBody>,
) -> impl IntoResponse {
    if body.group.is_empty() || body.group.contains('/') {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"error": "group must be non-empty and contain no '/'"}))).into_response();
    }
    if let Some(refused) = refuse_governed_group(&ctx.agent_ctx, &body.group) {
        return refused;
    }
    if !is_member(&ctx.agent_ctx, &body.group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core))
            .join_group(body.group.as_str());
        audit_membership(&ctx.agent_ctx, &format!("grp/{}/join", body.group));
    }
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, &body.group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({ "ok": true, "group": body.group, "members": members })).into_response()
}

/// `DELETE /gateway/mesh/group?group=G` — **this node** leaves `G`, tombstoning `grp/G/{self}`.
#[cfg(feature = "gateway")]
pub(super) async fn gw_group_leave(
    Query(q):   Query<GroupQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_governed_group(&ctx.agent_ctx, &q.group) {
        return refused;
    }
    if is_member(&ctx.agent_ctx, &q.group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core))
            .leave_group(q.group.as_str());
        audit_membership(&ctx.agent_ctx, &format!("grp/{}/leave", q.group));
    }
    Json(json!({ "ok": true, "group": q.group })).into_response()
}

/// Whether `group` is an electorate group (P2 — its electorate a consensus decision, governed for good), or
/// under a live membership intent (`sys/govern/membership/{group}`, fresh within
/// `MEMBERSHIP_INTENT_TTL_MS` — the governor's own reading, `emergent::detect_governed_group_conflicts`). Such a
/// group's population is governed: who belongs to it decides an election's roster and quorum, so changing it is a
/// governance act, not a data-plane write. Fleet-wide: an intent `target`ed at one node still governs the group here —
/// the electorate floor (`declared_electorate_min`) and the conflict detector read it the same way. "Live" lapses: an
/// intent not re-published within the TTL leaves the group ungoverned (and a future-dated `written_at_ms` reads as
/// fresh, as it does for the governor).
#[cfg(feature = "gateway")]
pub(super) fn is_governed_group(ctx: &Arc<TaskCtx>, group: &str) -> bool {
    use super::membership_governor::{MembershipIntent, MEMBERSHIP_INTENT_TTL_MS, MEMBERSHIP_PREFIX};
    // An electorate group (P2) is governed for good: its membership is fixed at genesis.
    if super::electorate::view(ctx, group).is_some() {
        return true;
    }
    let key = format!("{MEMBERSHIP_PREFIX}{group}");
    let Some(bytes) = ctx.kv_state.store.pin().get(key.as_str()).and_then(|e| e.data.clone()) else { return false };
    let Ok(intent) = mycelium_core::serde_fixint::from_slice::<MembershipIntent>(&bytes) else { return false };
    mycelium_core::sim_seam::wall_now_ms().saturating_sub(intent.written_at_ms) <= MEMBERSHIP_INTENT_TTL_MS
}

/// `/gateway/mesh/group`'s refusal for a governed group: **403** `governed_group`, naming the route that moves a node
/// in or out of one under `govern:write` (where the accepted change is audited; the refusal itself is not).
#[cfg(feature = "gateway")]
pub(super) fn refuse_governed_group(ctx: &Arc<TaskCtx>, group: &str) -> Option<axum::response::Response> {
    is_governed_group(ctx, group).then(|| (
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "governed_group",
            "message": format!("group {group} is governed (a membership intent or an electorate declaration), so its members \
                                decide its elections: this node joins or leaves it through POST/DELETE /gateway/govern/group \
                                (govern:write)") })),
    ).into_response())
}

/// `POST /gateway/govern/group` (`govern:write`) — **this node** joins `{group}`, governed or not; audited. The
/// governed path for what `POST /gateway/mesh/group` refuses for a group under a membership intent.
#[cfg(feature = "gateway")]
pub(super) async fn gw_govern_group_join(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_unknown_fields(&body, &["group"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|g| g.as_str()).filter(|g| !g.is_empty() && !g.contains('/')) else {
        return bad_request("`group` must be a non-empty string without '/'".into());
    };
    if !is_member(&ctx.agent_ctx, group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core)).join_group(group);
        audit_govern(&ctx.agent_ctx, &format!("grp/{group}/join"), body.to_string());
    }
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({ "ok": true, "group": group, "governed": is_governed_group(&ctx.agent_ctx, group), "members": members }))
        .into_response()
}

/// `POST /gateway/govern/electorate` (`govern:write`) — **declare `{group}`'s electorate**, **retire it**, or
/// **complete its migration** (post-360 plan row P2, `docs/design/consensus-electorate.md` §8), each decided by
/// consensus. Membership is fixed at genesis; changing who decides is a migration — the three calls in order:
///
/// - `{"group": "G", "exclusive_default": false}` declares a genesis from the roster this node sees, decided by every
///   member named; `exclusive_default` declares the fleet's default, refused while any group holds it. On a group that
///   has an electorate it decides nothing: `changed: false` when the roster matches, **409** `membership_frozen` when not.
/// - `{"group": "H", "predecessors": ["G", …]}` declares a **successor** first: it decides nothing until each named
///   group has retired to it, and outside the carried slots nothing until each carry is complete.
/// - `{"group": "G", "retire_to": "H"}` retires `G` to its successor `H`, which must already name `G` — a decision of
///   a strict majority of `G`'s electorate, closing `G`; when `G` holds the fleet's default, the retirement passes it to
///   `H`. An attempt that ends undecided is audited too (members that promised it have closed `G`).
/// - `{"group": "H", "complete": true}` **completes** `H`'s migration from a member of `H`: enumerates each retired
///   predecessor's carry set from a strict majority of it, carries every slot with one ballot of `H`, and decides the
///   completion `electorate/H/carried/G`. Idempotent; the response lists each predecessor's phase.
/// - `{"group": "H", "drop_predecessor": "G"}` **drops** a predecessor that cannot retire into `H` any more — its
///   retirement to another successor is visible (a losing successor's way on): a decision of `H`'s majority; afterwards
///   `H` no longer waits on `G`. Nothing of `G` is carried. `"force": true` asserts, as an operator and **outside the
///   protocol's guarantee**, that `G`'s majority is gone for good (DR territory): the drop is admitted without a
///   retirement elsewhere, audited with the flag; refused **409** without it while `G` may still retire.
///
/// **403** `not_a_member`, **409** `retired` (the group is retired — the successor is named), **409** `refused` (a
/// precondition no retry changes: a second default, a successor that does not name the retiree, a predecessor retired
/// elsewhere, a retirement with an incomplete carry), **409** `not_decided` (no quorum answered after the bounded
/// retries, a record not visible here yet, or a carry that could not finish — the message names the slot); without
/// `consensus`, 501.
#[cfg(feature = "gateway")]
pub(super) async fn gw_govern_electorate_declare(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_unknown_fields(&body, &["group", "exclusive_default", "retire_to", "predecessors", "complete", "drop_predecessor", "force"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|g| g.as_str()).filter(|g| !g.is_empty() && !g.contains('/')) else {
        return bad_request("`group` must be a non-empty string without '/'".into());
    };
    let exclusive_default = match body.get("exclusive_default") {
        None => false,
        Some(v) => match v.as_bool() { Some(b) => b, None => return bad_request("`exclusive_default` must be a boolean".into()) },
    };
    let retire_to = match body.get("retire_to") {
        None => None,
        Some(v) => match v.as_str().filter(|s| !s.is_empty() && !s.contains('/') && *s != group) {
            Some(s) if body.get("exclusive_default").is_none() => Some(s),
            Some(_) => return bad_request("`retire_to` retires the group; the default follows the retirement and is not given".into()),
            None => return bad_request("`retire_to` must be another group's name, without '/'".into()),
        },
    };
    let predecessors: std::collections::BTreeSet<String> = match body.get("predecessors") {
        None => std::collections::BTreeSet::new(),
        Some(v) => match v.as_array().map(|a| a.iter().map(|p| p.as_str().filter(|p| !p.is_empty() && !p.contains('/') && *p != group)
                .map(str::to_string)).collect::<Option<std::collections::BTreeSet<String>>>()) {
            Some(Some(set)) if retire_to.is_none() => set,
            Some(Some(_)) => return bad_request("`predecessors` declares a successor; `retire_to` retires one — not both".into()),
            _ => return bad_request("`predecessors` must be a list of other groups' names, without '/'".into()),
        },
    };
    let complete = match body.get("complete") {
        None => false,
        Some(v) => match v.as_bool() {
            Some(true) if retire_to.is_none() && body.get("predecessors").is_none() && body.get("exclusive_default").is_none()
                && body.get("drop_predecessor").is_none() => true,
            Some(true) => return bad_request("`complete` completes the group's migration; it takes no other field".into()),
            Some(false) => false,
            None => return bad_request("`complete` must be a boolean".into()),
        },
    };
    let force = match body.get("force") {
        None => false,
        Some(v) => match v.as_bool() {
            Some(b) if body.get("drop_predecessor").is_some() => b,
            Some(_) => return bad_request("`force` belongs to `drop_predecessor`".into()),
            None => return bad_request("`force` must be a boolean".into()),
        },
    };
    let drop_predecessor = match body.get("drop_predecessor") {
        None => None,
        Some(v) => match v.as_str().filter(|s| !s.is_empty() && !s.contains('/') && *s != group) {
            Some(s) if retire_to.is_none() && body.get("predecessors").is_none() && body.get("exclusive_default").is_none() && !complete => Some(s),
            Some(_) => return bad_request("`drop_predecessor` drops one predecessor; it takes no other field".into()),
            None => return bad_request("`drop_predecessor` must be another group's name, without '/'".into()),
        },
    };
    #[cfg(feature = "consensus")]
    {
        use super::electorate::ElectorateError as E;
        let refusal = |e: E| match e {
            E::NotAMember => (StatusCode::FORBIDDEN,
                Json(json!({ "ok": false, "error": "not_a_member", "message": e.to_string() }))).into_response(),
            E::InvalidGroup => bad_request(e.to_string()),
            E::MembershipFrozen { .. } => (StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": "membership_frozen", "message": e.to_string() }))).into_response(),
            E::Retired { ref successor } => (StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": "retired", "successor": successor, "message": e.to_string() }))).into_response(),
            E::Refused(_) => (StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": "refused", "message": e.to_string() }))).into_response(),
            E::PersistenceRequired { .. } => (StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": "persistence_required", "message": e.to_string() }))).into_response(),
            e => (StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": "not_decided", "message": e.to_string() }))).into_response(),
        };
        if let Some(predecessor) = drop_predecessor {
            let already = super::electorate::dropped(&ctx.agent_ctx, group, predecessor).is_some();
            return match super::electorate::retrying(|| super::electorate::drop_outcome(&ctx.agent_ctx, group, predecessor, force)).await {
                Ok(d) => {
                    if !already {
                        let detail = json!({ "request": body, "outcome": "dropped", "force": d.force });
                        audit_govern(&ctx.agent_ctx, &super::electorate::drop_slot(group, predecessor), detail.to_string());
                    }
                    Json(json!({ "ok": true, "changed": !already, "group": d.group, "dropped": d.predecessor, "force": d.force })).into_response()
                }
                Err(e) => refusal(e),
            };
        }
        if complete {
            let before: Vec<_> = super::migration::state(&ctx.agent_ctx, group);
            return match super::electorate::retrying(|| super::migration::complete_outcome(&ctx.agent_ctx, group)).await {
                Ok(states) => {
                    // Audited once per completion this run decided or found newly decided.
                    for s in states.iter().filter(|s| matches!(s.phase, super::electorate::MigrationPhase::Complete { .. })) {
                        let newly = !before.iter().any(|b| b.predecessor == s.predecessor
                            && matches!(b.phase, super::electorate::MigrationPhase::Complete { .. }));
                        if newly {
                            let detail = json!({ "request": body, "outcome": "complete", "predecessor": s.predecessor });
                            audit_govern(&ctx.agent_ctx, &super::electorate::carried_slot(group, &s.predecessor), detail.to_string());
                        }
                    }
                    Json(json!({ "ok": true, "group": group, "migration": states })).into_response()
                }
                Err(e) => refusal(e),
            };
        }
        if let Some(successor) = retire_to {
            return match super::electorate::retrying(|| super::electorate::retire_outcome(&ctx.agent_ctx, group, successor)).await {
                Ok(r) => {
                    let detail = json!({ "request": body, "outcome": "retired", "passes_default": r.passes_default });
                    audit_govern(&ctx.agent_ctx, &super::electorate::retire_slot(group), detail.to_string());
                    Json(json!({ "ok": true, "changed": true, "group": r.group, "retired_to": r.successor,
                                 "passes_default": r.passes_default })).into_response()
                }
                Err(e @ E::NotDecided(_)) => {
                    // An undecided attempt may still have closed the members that promised it (round 5, Q4): audited.
                    let detail = json!({ "request": body, "outcome": "undecided", "message": e.to_string() });
                    audit_govern(&ctx.agent_ctx, &super::electorate::retire_slot(group), detail.to_string());
                    refusal(e)
                }
                Err(e) => refusal(e),
            };
        }
        match super::electorate::retrying(|| super::electorate::declare_outcome(&ctx.agent_ctx, group, exclusive_default, &predecessors)).await {
            Ok((decl, outcome)) => {
                // Audited when a genesis was decided — this node's, or another member's it completed (round 2,
                // finding 3); a declaration that changes nothing is not a change, and a refused or undecided one
                // never reaches here (#601's review, finding 6).
                use super::electorate::Declared;
                let changed = outcome != Declared::Unchanged;
                if changed {
                    let detail = json!({ "request": body, "outcome": if outcome == Declared::Adopted { "adopted" } else { "decided" } });
                    audit_govern(&ctx.agent_ctx, &super::electorate::genesis_slot(group, decl.epoch), detail.to_string());
                }
                Json(json!({ "ok": true, "changed": changed, "adopted": outcome == Declared::Adopted,
                             "group": decl.group, "epoch": decl.epoch, "exclusive_default": decl.exclusive_default,
                             "predecessors": decl.predecessors.iter().cloned().collect::<Vec<_>>(),
                             "members": decl.members.iter().map(ToString::to_string).collect::<Vec<_>>() })).into_response()
            }
            Err(e) => refusal(e),
        }
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = (ctx, group, exclusive_default, retire_to, predecessors, complete, drop_predecessor, force);
        (StatusCode::NOT_IMPLEMENTED, Json(json!({ "ok": false, "error": "not_in_build", "message": "an electorate needs `consensus`" })))
            .into_response()
    }
}

/// `DELETE /gateway/govern/group?group=G` (`govern:write`) — **this node** leaves `G`, governed or not; audited.
#[cfg(feature = "gateway")]
pub(super) async fn gw_govern_group_leave(
    Query(q):   Query<std::collections::HashMap<String, String>>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    // As strict as the POST: a `target` (or any other parameter) is refused, not dropped and applied here.
    if let Some(k) = q.keys().find(|k| k.as_str() != "group") {
        return bad_request(format!("unknown parameter `{k}`"));
    }
    let Some(group) = q.get("group").filter(|g| !g.is_empty() && !g.contains('/')) else {
        return bad_request("`group` must be a non-empty name without '/'".into());
    };
    if is_member(&ctx.agent_ctx, group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core)).leave_group(group.as_str());
        audit_govern(&ctx.agent_ctx, &format!("grp/{group}/leave"), json!({"route": "govern/group"}).to_string());
    }
    Json(json!({ "ok": true, "group": group })).into_response()
}

/// Whether this node is a live member of `group` — so a join that changes nothing, or a leave of a group it is not
/// in, is not recorded as a change.
#[cfg(feature = "gateway")]
pub(super) fn is_member(ctx: &TaskCtx, group: &str) -> bool {
    crate::agent::helpers::group_members_ctx(ctx, group).contains(&ctx.node_id)
}

/// Record a plain group's membership change in the audit trail. Not a governance change — a plain group's membership is
/// `mesh:write` data plane — so the governance counters are not touched; a failed seal is a warning (#572's review).
#[cfg(feature = "gateway")]
pub(super) fn audit_membership(ctx: &Arc<TaskCtx>, target: &str) {
    #[cfg(feature = "compliance")]
    if let Err(e) = super::audit::seal_and_write(ctx, super::audit::AuditAction::Admin, "gateway/mesh", target,
                                                 super::audit::AuditOutcome::Success, None) {
        tracing::debug!(target, "group membership change not audited: {e}");
    }
    #[cfg(not(feature = "compliance"))]
    let _ = (ctx, target);
}

/// `GET /gateway/mesh/group?group=G` — the roster this node can see for `G`.
///
/// This is the read an operator needs *before* an election: it answers "does this node have an
/// electorate, and is its view complete?", which is the question a refusal will otherwise answer
/// for them.
#[cfg(feature = "gateway")]
pub(super) async fn gw_group_members(
    Query(q):   Query<GroupQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, &q.group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({
        "group":        q.group,
        "members":      members,
        "declared_min": crate::agent::helpers::declared_electorate_min(&ctx.agent_ctx, &q.group),
    })).into_response()
}

#[derive(Deserialize)]
pub(super) struct GroupQuery { group: String }

pub(super) async fn gw_govern_membership(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use super::membership_governor::{MembershipIntent, MEMBERSHIP_PREFIX};

    if let Some(refused) = refuse_unknown_fields(&body, &["group", "min", "max", "drain", "target"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing 'group'"}))).into_response();
    };
    // `min` is required (the route's contract); a `"3"` or `-1` read as 0 and published a floor of nothing.
    let min = match optional_u64(body.get("min"), "min") {
        Ok(Some(m)) => m as usize,
        Ok(None) => return bad_request("missing 'min'".into()),
        Err(e) => return bad_request(e),
    };
    let max = match optional_u64(body.get("max"), "max") { Ok(v) => v.map(|m| m as usize), Err(e) => return bad_request(e) };

    let mut drain: Vec<crate::node_id::NodeId> = Vec::new();
    let drain_in = match body.get("drain") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(a)) => Some(a),
        Some(_) => return bad_request("`drain` must be an array of node ids".into()),
    };
    if let Some(arr) = drain_in {
        for v in arr {
            let Some(s) = v.as_str() else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "drain entries must be node-id strings"}))).into_response();
            };
            match s.parse() {
                Ok(n)  => drain.push(n),
                Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid drain node id '{s}'")}))).into_response(),
            }
        }
    }

    let target = match parse_optional_target(&body) {
        Ok(t)  => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };

    let mut intent = MembershipIntent::new(group, min, max).with_drain(drain);
    if let Some(t) = target {
        intent = intent.for_node(t);
    }
    let key = format!("{MEMBERSHIP_PREFIX}{group}");
    let kv = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&ctx.agent_ctx.core));
    let ok = super::intent::publish_intent(&kv, &key, intent);
    audit_govern(&ctx.agent_ctx, &key, body.to_string());
    Json(json!({"ok": ok, "key": key})).into_response()
}
