//! Capabilities: advertise / heartbeat / drop / resolve, `units/declare`, `demand`, and the provider scan the
//! shard and LLM doors share.

use super::*;

/// One gateway-advertised capability. In-process advertisers get liveness
/// coupling for free (their refresh loop dies with them); a *bridged*
/// advertiser's refresh loop runs in this node, so its advert would outlive
/// the client process — the lease closes that gap.
pub(super) struct GatewayCapHandle {
    /// Dropped on map removal → the persist task exits and tombstones the
    /// KV entry. Never read; its `Drop` is the retraction mechanism.
    _cancel:   oneshot::Sender<()>,
    /// `Some` when the advertiser passed `lease_secs`: each notification
    /// restarts the lease watchdog's window; a full window with no beat
    /// retracts the advert exactly as `DELETE /gateway/capability/{id}` would.
    heartbeat: Option<Arc<Notify>>,
    /// Whatever else this handle keeps alive (`POST /gateway/units/declare`: the unit's capability
    /// registrations, requirement and group handles). Dropped with the entry, which retracts them.
    _held:     Option<Box<dyn std::any::Any + Send>>,
}

// ── Language-bridge gateway handlers ─────────────────────────────────────────
//
// These seven endpoints form the HTTP sidecar API for Python/TypeScript agents.
// All inputs and outputs use JSON. Binary payloads are base64-encoded.

/// `POST /gateway/capability/advertise`
///
/// Advertises a capability on behalf of a language-bridge agent. The
/// returned `handle_id` must be supplied to `DELETE /gateway/capability/{id}`
/// to retract the advertisement (tombstone the KV entry).
///
/// Request body:
/// ```json
/// { "ns": "compute", "name": "gpu",
///   "interval_secs": 30,
///   "lease_secs": 90,
///   "attributes": { "model": "A100" },
///   "authorized_callers": ["orchestrator"] }
/// ```
/// Response: `{ "handle_id": "<uuid>" }`
///
/// `lease_secs` (optional) binds the advertisement to the *client's* liveness,
/// not this node's: the caller must `POST /gateway/capability/{handle_id}/heartbeat`
/// within every `lease_secs` window or the advert is retracted (tombstoned) as if
/// DELETEd. Beat at `lease_secs / 3` for margin — mirroring the mesh's own 3×
/// evaporation convention. Without it, the refresh task keeps the advert fresh
/// until DELETE or node shutdown — which outlives a crashed bridge client.
/// Post-360 row R: a model's `llm-meta/{model}` ad — its `locality` above all — is what an inference-routing policy
/// decides on, and it is the **serving node's** declaration (`mycelium_reason::serve_model`). A gateway client writes
/// ads under this node's key, so any `llm-meta` ad it wrote would either declare a model's locality for it or, written
/// without one, replace this node's own ad (last writer wins on one key) and drop its locality. The namespace is
/// therefore refused at both doors that write an ad for a client — `capability/advertise` and `units/declare` — with
/// or without a `locality` attribute (the review's finding 2: the first cut refused only the attribute).
const MODEL_META_NS: &str = "llm-meta";

fn refuse_model_meta(ns: &str, name: &str) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(json!({
        "ok": false, "error": "reserved_namespace",
        "message": format!("{ns}/{name}: `{MODEL_META_NS}` ads are the serving node's own declaration of its model \
                            (where its inference runs, post-360 row R); a gateway client cannot write one"),
    }))).into_response()
}

fn refuse_reserved(ns: &str, name: &str, problem: &str) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(json!({
        "ok": false, "error": "reserved_namespace", "message": format!("{ns}/{name}: {problem}"),
    }))).into_response()
}

pub(super) async fn gw_cap_advertise(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::capability::{Capability, CapValue};

    let ns   = match body["ns"].as_str()   { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing ns"}))).into_response() };
    let name = match body["name"].as_str() { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing name"}))).into_response() };
    if let Some(problem) = crate::capability::capability_name_problem(&ns, &name) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "invalid_capability_name",
            "message": format!("{ns:?}/{name:?}: {problem}"),
        }))).into_response();
    }
    if ns == MODEL_META_NS {
        return refuse_model_meta(&ns, &name);
    }
    if let Some(problem) = crate::capability::reserved_capability_namespace(&ns) {
        return refuse_reserved(&ns, &name, problem);
    }
    let (interval_secs, lease_secs) = match (timer_secs(&body, "interval_secs"), timer_secs(&body, "lease_secs")) {
        (Ok(i), Ok(l)) => (i.unwrap_or(DEFAULT_INTERVAL_SECS), l),
        (Err(refused), _) | (_, Err(refused)) => return *refused,
    };

    let mut cap = Capability::new(ns.as_str(), name.as_str());

    if let Some(attrs) = body["attributes"].as_object() {
        for (k, v) in attrs {
            let cv = match v {
                serde_json::Value::String(s) => CapValue::Text(Arc::from(s.as_str())),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() { CapValue::Integer(i) }
                    else if let Some(f) = n.as_f64() { CapValue::Float(f) }
                    else { continue }
                }
                serde_json::Value::Bool(b) => CapValue::Bool(*b),
                _ => continue,
            };
            cap = cap.with(k.as_str(), cv);
        }
    }

    if let Some(callers) = body["authorized_callers"].as_array() {
        let list: Vec<Arc<str>> = callers.iter()
            .filter_map(|v| v.as_str())
            .map(Arc::from)
            .collect();
        cap = cap.with_authorized_callers(list);
    }

    let interval = Duration::from_secs(interval_secs);
    let kv_key: Arc<str> = Arc::from(
        format!("cap/{}/{}/{}", ctx.agent_ctx.node_id, cap.namespace, cap.name).as_str()
    );
    // The advert carries the period it is refreshed at, as an in-process advert does
    // (`CapabilitiesHandle::advertise_capability`): a bare `Capability` is timed by every reader
    // with the legacy 60 s default, so a slower refresh read as stale between beats and a faster one
    // lingered three minutes after its refresher died (post-360 row E).
    let entry = Arc::new(crate::capability::CapEntry { capability: cap, refresh_interval_ms: interval_secs * 1_000 });
    let payload_fn: mycelium_core::kv_persist::PersistPayloadFn = {
        let entry = Arc::clone(&entry);
        Arc::new(move || entry.encode())
    };

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let shutdown_rx = ctx.shutdown_rx.clone();
    tokio::spawn(mycelium_core::kv_persist::run_kv_persist_task(
        Arc::clone(&ctx.agent_ctx.core), cancel_rx, shutdown_rx, kv_key, interval, payload_fn, None,
    ));

    let handle_id = new_handle_id();

    // Lease mode: the watchdog retracts through the same path as DELETE (map
    // removal drops the cancel sender). `remove` returning `None` means the
    // caller already retracted — exit without noise.
    let heartbeat = lease_secs.map(|secs| lease_watchdog(&ctx, &handle_id, secs));

    ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner())
        .insert(handle_id.clone(), GatewayCapHandle { _cancel: cancel_tx, heartbeat, _held: None });

    Json(json!({ "handle_id": handle_id })).into_response()
}

/// `interval_secs` when a request names none.
pub(super) const DEFAULT_INTERVAL_SECS: u64 = 30;

/// A client-supplied timer period in whole seconds (post-360 hardening row E): `Ok(None)` when the
/// field is absent or `null`, `Ok(Some(n))` for an integer in `1..=MAX_TIMER_PERIOD_SECS`, and a 400
/// naming the field otherwise. Before this, any `u64` was a period and a value that was present but not
/// a whole number (`0.5`, `"30"`) fell back to the default without a word.
pub(super) fn timer_secs(body: &serde_json::Value, field: &str) -> Result<Option<u64>, Box<axum::response::Response>> {
    use crate::capability_config::MAX_TIMER_PERIOD_SECS;
    match &body[field] {
        serde_json::Value::Null => Ok(None),
        // `30.0` is a whole number too; `0.5` and `"30"` are not.
        v => match v.as_u64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && (0.0..=MAX_TIMER_PERIOD_SECS as f64).contains(f)).map(|f| f as u64)) {
            Some(n) if (1..=MAX_TIMER_PERIOD_SECS).contains(&n) => Ok(Some(n)),
            _ => Err(Box::new((StatusCode::BAD_REQUEST, Json(json!({
                "error": "timer_out_of_range",
                "field": field,
                "message": format!("{field} must be a whole number of seconds in 1..={MAX_TIMER_PERIOD_SECS}, got {v}"),
            }))).into_response())),
        },
    }
}

/// An opaque gateway handle id. One mint for every handle kind, so the replay inventory counts
/// one nondeterministic site, not one per route.
pub(super) fn new_handle_id() -> String {
    format!("{:x}", fastrand::u128(..))
}

/// The lease watchdog every gateway handle with `lease_secs` shares: a full window without a
/// heartbeat removes the handle from `gateway_caps`, which retracts whatever it holds — exactly as
/// `DELETE` would.
pub(super) fn lease_watchdog(ctx: &HttpCtx, handle_id: &str, secs: u64) -> Arc<Notify> {
    let lease = Duration::from_secs(secs.max(1));
    let hb = Arc::new(Notify::new());
    let watchdog_hb = Arc::clone(&hb);
    let caps = Arc::clone(&ctx.gateway_caps);
    let hid = handle_id.to_string();
    let mut wshutdown = ctx.shutdown_rx.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = wshutdown.wait_for(|v| *v) => return,
                beat = tokio::time::timeout(lease, watchdog_hb.notified()) => {
                    if beat.is_ok() { continue; }
                    if caps.lock().unwrap_or_else(|e| e.into_inner()).remove(&hid).is_some() {
                        warn!(handle = %hid, "gateway capability lease expired without heartbeat — retracting");
                    }
                    return;
                }
            }
        }
    });
    hb
}

/// `POST /gateway/units/declare` — an SDK agent's unit file (design-time-tooling.md Q2).
///
/// Body: `{"toml": "<the unit file>", "interval_secs"?: n, "lease_secs"?: n}`. The node parses and
/// validates the file with its own loader — the SDKs carry text, never a second parser — then
/// declares its `[[capability]]`, `[[requirement]]` and `[[group]]` sections under **one handle**
/// that `DELETE /gateway/capability/{handle_id}` retracts and the heartbeat route renews (with
/// `lease_secs`, a missed window retracts it). Refused by name: an unparsable or invalid file
/// (400), and the hosting sections — `[hosts]`, `[[presence]]`, `[[activation]]`, `[[serve]]` (422) — which are
/// a stem's: an SDK agent hosts nothing. `[[lane]]`, `[[mandate]]` and `[[rule]]` are accepted as
/// declarations and reported in `not_enforced`, as the stem reports them.
pub(super) async fn gw_units_declare(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(text) = body["toml"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing toml"}))).into_response();
    };
    let units = match crate::NodeCapabilityConfig::from_toml_str(text) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e.to_string()}))).into_response(),
    };
    let mut hosting = Vec::new();
    if units.hosts.is_some() { hosting.push("[hosts]"); }
    if !units.presence.is_empty() { hosting.push("[[presence]]"); }
    if !units.activations.is_empty() { hosting.push("[[activation]]"); }
    if !units.serves.is_empty() { hosting.push("[[serve]]"); }
    if !hosting.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
            "error": "hosting sections",
            "detail": format!("{} belong to a stem (mycelium-stem): an SDK agent hosts nothing", hosting.join(", ")),
        }))).into_response();
    }
    if !units.schedules.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
            "error": "schedule section",
            "detail": "[[schedule]] is not declared with a unit: a schedule runs while something supplies its work — an SDK agent opens GET /gateway/schedules/run with the section and receives its runs (scope schedule:run); an application embedding the node calls GossipAgent::run_schedule",
        }))).into_response();
    }
    // Row R: a served model's ad is not a client's to write (see `refuse_model_meta`).
    if let Some(c) = units.capabilities.iter().find(|c| c.ns == MODEL_META_NS) {
        return refuse_model_meta(&c.ns, &c.name);
    }
    // The unit loader refuses a reserved namespace already; said again here, where the ad would be written.
    if let Some((c, problem)) = units.capabilities.iter().find_map(|c| crate::capability::reserved_capability_namespace(&c.ns).map(|p| (c, p))) {
        return refuse_reserved(&c.ns, &c.name, problem);
    }
    // Convert everything before declaring anything: a refusal leaves nothing half-declared.
    let filters = match units.requirements.iter().map(|r| r.to_filter()).collect::<Result<Vec<_>, _>>() {
        Ok(f) => f,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e}))).into_response(),
    };
    // A group under a membership intent is governed: its definition (its eligibility filter) decides which nodes
    // the governor can elect into it, so `cap:write` does not redefine it (#572's review).
    let governed: Vec<&str> = units.groups.iter().map(|g| g.name.as_str()).filter(|n| is_governed_group(&ctx.agent_ctx, n)).collect();
    if !governed.is_empty() {
        return (StatusCode::FORBIDDEN, Json(json!({
            "ok": false, "error": "governed_group",
            "message": format!("{} under a membership intent: a governed group's definition is not redefined through units/declare",
                               governed.join(", ")),
        }))).into_response();
    }
    let defs = match units.groups.iter().map(|g| g.to_def().map(|d| (g.name.clone(), d))).collect::<Result<Vec<_>, _>>() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e}))).into_response(),
    };
    let (interval, lease_secs) = match (timer_secs(&body, "interval_secs"), timer_secs(&body, "lease_secs")) {
        (Ok(i), Ok(l)) => (Duration::from_secs(i.unwrap_or(DEFAULT_INTERVAL_SECS)), l),
        (Err(refused), _) | (_, Err(refused)) => return *refused,
    };
    let caps = crate::agent::CapabilitiesHandle { ctx: Arc::clone(&ctx.agent_ctx) };
    let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
    for c in &units.capabilities {
        held.push(Box::new(caps.advertise_capability(c.build_capability_public(), Duration::from_secs(c.ttl_secs))));
    }
    for f in filters {
        held.push(Box::new(caps.declare_requirement(f, interval)));
    }
    for (name, def) in defs {
        held.push(Box::new(caps.define_capability_group(name.as_str(), def, interval)));
    }
    let mut not_enforced = Vec::new();
    if !units.lanes.is_empty() { not_enforced.push("[[lane]]"); }
    if !units.mandates.is_empty() { not_enforced.push("[[mandate]]"); }
    if !units.rules.is_empty() { not_enforced.push("[[rule]]"); }

    let handle_id = new_handle_id();
    let heartbeat = lease_secs.map(|secs| lease_watchdog(&ctx, &handle_id, secs));
    let (cancel_tx, _cancel_rx) = oneshot::channel::<()>();
    ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner())
        .insert(handle_id.clone(), GatewayCapHandle { _cancel: cancel_tx, heartbeat, _held: Some(Box::new(held)) });
    Json(json!({
        "handle_id": handle_id,
        "principal": units.principal,
        "declared": {
            "capabilities": units.capabilities.len(),
            "requirements": units.requirements.len(),
            "groups": units.groups.len(),
        },
        "not_enforced": not_enforced,
    })).into_response()
}

/// `POST /gateway/capability/{handle_id}/heartbeat`
///
/// Renews the lease on a capability advertised with `lease_secs`. `404` for an
/// unknown or already-retracted handle (a client seeing this should re-advertise);
/// `409` when the handle was advertised without `lease_secs` and has no lease.
pub(super) async fn gw_cap_heartbeat(
    Path(handle_id): Path<String>,
    State(ctx):      State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let guard = ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(&handle_id) {
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "handle not found" }))).into_response(),
        Some(GatewayCapHandle { heartbeat: None, .. }) =>
            (StatusCode::CONFLICT, Json(json!({ "error": "handle has no lease (advertised without lease_secs)" }))).into_response(),
        Some(GatewayCapHandle { heartbeat: Some(hb), .. }) => {
            hb.notify_one();
            Json(json!({ "ok": true })).into_response()
        }
    }
}

/// `DELETE /gateway/capability/{handle_id}`
///
/// Retracts a previously-advertised capability. Drops the cancel sender,
/// which causes the persist task to tombstone the KV entry.
pub(super) async fn gw_cap_drop(
    Path(handle_id): Path<String>,
    State(ctx):      State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let removed = ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle_id).is_some();
    if removed {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "error": "handle not found" }))).into_response()
    }
}

/// `GET /gateway/capability/resolve?ns=X&name=Y[&caller_id=Z]`
///
/// Snapshot filter-match over the local `cap/` KV view. If `caller_id` is
/// supplied, capabilities with non-empty `authorized_callers` are filtered
/// to only those that list the caller's identity.
#[derive(Deserialize)]
pub(super) struct ResolveQuery {
    ns:        String,
    name:      String,
    caller_id: Option<String>,
}

pub(super) async fn gw_cap_resolve(
    Query(q):   Query<ResolveQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::{CallerContext, CapFilter, Capability};

    let filter     = CapFilter::new(q.ns.as_str(), q.name.as_str());
    let caller_ctx = match q.caller_id {
        Some(id) => CallerContext::for_caller(id.as_str()),
        None     => CallerContext::unrestricted(),
    };

    let mut results = Vec::new();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, "cap/") {
        if super::capability_ops::is_cap_locality_key(&key) { continue; }
        let Some((node_id, _ns, _name)) =
            super::capability_ops::parse_cap_key_or_warn("cap/", &key)
            else { continue };
        let Some(cap) = Capability::decode(&bytes) else { continue };
        if filter.matches(&cap) && caller_ctx.can_see(&cap) {
            let attrs: serde_json::Map<String, serde_json::Value> = cap.attributes.iter()
                .map(|(k, v)| (k.as_ref().to_string(), capvalue_to_json(v)))
                .collect();
            results.push(json!({
                "node_id":    node_id.to_string(),
                "ns":         cap.namespace.as_ref(),
                "name":       cap.name.as_ref(),
                "attributes": attrs,
            }));
        }
    }

    Json(json!({ "providers": results })).into_response()
}

pub(super) fn capvalue_to_json(v: &crate::capability::CapValue) -> serde_json::Value {
    use crate::capability::CapValue;
    match v {
        CapValue::Text(s)    => serde_json::Value::String(s.as_ref().to_string()),
        CapValue::Integer(n) => json!(n),
        CapValue::Float(f)   => json!(f),
        CapValue::Bool(b)    => json!(b),
        CapValue::Version(v) => serde_json::Value::String(format!("{}.{}.{}", v[0], v[1], v[2])),
    }
}

/// `GET /gateway/demand?ns=X&name=Y`
///
/// Returns the demand-pressure snapshot for a capability filter.
#[derive(Deserialize)]
pub(super) struct DemandQuery { ns: String, name: String }

pub(super) async fn gw_demand(
    Query(q):   Query<DemandQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;

    let filter   = CapFilter::new(q.ns.as_str(), q.name.as_str());
    let kv       = &ctx.agent_ctx.kv_state;

    let providers = crate::store::scan_kv_prefix(kv, "cap/")
        .into_iter()
        .filter(|(k, v)| {
            if super::capability_ops::is_cap_locality_key(k) { return false; }
            crate::capability::Capability::decode(v)
                .map(|c| filter.matches(&c))
                .unwrap_or(false)
        })
        .count();

    let requirers = crate::store::scan_kv_prefix(kv, "req/")
        .into_iter()
        .filter(|(_, v)| {
            crate::capability::CapFilter::decode(v)
                .map(|f| f.namespace == filter.namespace && f.name == filter.name)
                .unwrap_or(false)
        })
        .count();

    let pressure = (requirers as f64) / (providers.max(1) as f64);

    Json(json!({
        "ns":              q.ns,
        "name":            q.name,
        "providers":       providers,
        "requirers":       requirers,
        "demand_pressure": pressure,
    })).into_response()
}

/// Shared helper: scan `cap/` KV and return providers matching `filter`.
/// Mirrors the scan in `gw_cap_resolve` (no freshness check — same as the HTTP resolve endpoint).
pub(super) fn resolve_cap_providers(
    kv_state: &crate::store::KvState,
    filter:   &crate::capability::CapFilter,
) -> Vec<(crate::node_id::NodeId, crate::capability::Capability)> {
    use crate::capability::Capability;
    use crate::store::scan_kv_prefix;
    use super::capability_ops::{is_cap_locality_key, parse_cap_key_or_warn};

    let mut out = Vec::new();
    for (key, bytes) in scan_kv_prefix(kv_state, "cap/") {
        if is_cap_locality_key(&key) { continue; }
        let Some((node_id, _ns, _name)) = parse_cap_key_or_warn("cap/", &key) else { continue };
        let Some(cap) = Capability::decode(&bytes) else { continue };
        if filter.matches(&cap) {
            out.push((node_id, cap));
        }
    }
    out
}
