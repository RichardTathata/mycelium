//! The raw mesh doors: `rpc/call`, `rpc/serve`, `rpc/respond` (with the served-request table), `scatter`,
//! the mailbox and the sharded emit.

use super::*;

/// Longest a streamed request stays answerable: the gateway's own RPC ceiling. Every gateway door
/// waits at most this long (`rpc/call`, `scatter` and `overlay/emit_reliable` clamp `timeout_secs` to
/// 300; `/gateway/llm/call` clamps `timeout_ms` through `gateway_rpc_timeout_ms`; `/gateway/llm/stream`
/// waits 30 s). An **in-process** `ServiceHandle::rpc_call` names its own timeout and is not bounded
/// by this: a request it sends to an SDK agent stays answerable through `rpc/respond` for 300 s, and
/// a longer in-process wait gets no reply after that — use 300 s as the ceiling for SDK-served kinds.
pub(super) const SERVED_RPC_TTL_MS: u64 = 300_000;
/// Hard cap on streamed requests remembered at once; past it the oldest are evicted.
pub(super) const SERVED_RPC_CAP: usize = 65_536;
/// TTL eviction runs once per this many inserts (and whenever the cap is reached), not per insert.
pub(super) const SERVED_RPC_SWEEP_EVERY: u64 = 1_024;

/// [`HttpCtx::served_rpcs`]'s contents.
#[derive(Default)]
pub(super) struct ServedRpcs {
    /// `(sender, nonce, principal)` → `(streamed at ms, answered)`.
    map: HashMap<(crate::node_id::NodeId, u64, String), (u64, bool)>,
    inserts: u64,
}

/// What `rpc/respond` may do with a request.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ServedAnswer {
    /// Streamed to this principal and not yet answered: answer it (now marked answered).
    First,
    /// Already answered by this principal: accept and drop.
    Duplicate,
    /// Never streamed to this principal, or past the window: refuse.
    Unserved,
}

/// A gateway door's RPC wait, clamped to the gateway ceiling (`SERVED_RPC_TTL_MS`) — the window in
/// which a streamed request stays answerable. Every gateway door waits at most this long.
#[cfg(any(feature = "llm", test))]
pub(super) fn gateway_rpc_timeout_ms(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(1, SERVED_RPC_TTL_MS))
}

/// `gw_rpc_serve`'s record: this `principal` was handed the request `(sender, nonce)` now. A record
/// that already exists is kept as it is (a second replica's stream must not un-answer it).
pub(super) fn record_served_rpc(ctx: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64, principal: &str) {
    let now = ctx.agent_ctx.hlc.decision_now_ms();
    let mut served = ctx.served_rpcs.lock().unwrap_or_else(|e| e.into_inner());
    served.inserts = served.inserts.wrapping_add(1);
    if served.inserts.is_multiple_of(SERVED_RPC_SWEEP_EVERY) || served.map.len() >= SERVED_RPC_CAP {
        served.map.retain(|_, (at, _)| now.saturating_sub(*at) <= SERVED_RPC_TTL_MS);
    }
    if served.map.len() >= SERVED_RPC_CAP
        && let Some(oldest) = served.map.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
    {
        served.map.remove(&oldest);
    }
    served.map.entry((sender.clone(), nonce, principal.to_string())).or_insert((now, false));
}

/// `gw_rpc_respond`'s check: was `(sender, nonce)` streamed to `principal`, within the window, and
/// is this the first answer?
pub(super) fn answer_served_rpc(ctx: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64, principal: &str) -> ServedAnswer {
    let now = ctx.agent_ctx.hlc.decision_now_ms();
    let mut served = ctx.served_rpcs.lock().unwrap_or_else(|e| e.into_inner());
    match served.map.get_mut(&(sender.clone(), nonce, principal.to_string())) {
        Some((at, _)) if now.saturating_sub(*at) > SERVED_RPC_TTL_MS => ServedAnswer::Unserved,
        Some((_, answered)) if *answered => ServedAnswer::Duplicate,
        Some((_, answered)) => {
            *answered = true;
            ServedAnswer::First
        }
        None => ServedAnswer::Unserved,
    }
}

/// `POST /gateway/rpc/call`
///
/// Sends a blocking RPC call to a named node. `payload_b64` is base64.
/// Returns `{ "ok": true, "result_b64": "…" }` or `{ "ok": false, "error": "timeout" }`.
pub(super) async fn gw_rpc_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);

    let target_str = match body["target"].as_str() {
        Some(s) => s.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing target"}))).into_response(),
    };
    let target: crate::node_id::NodeId = match target_str.parse() {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid target node id"}))).into_response(),
    };

    let method = match body["method"].as_str() {
        Some(m) => Arc::from(m),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing method"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &method) {
        return refused;
    }

    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let timeout_secs = body["timeout_secs"].as_u64().unwrap_or(30);
    let timeout      = Duration::from_secs(timeout_secs.clamp(1, 300));

    match gateway_caller::gateway_rpc_call(&ctx.agent_ctx, caller.as_ref(), target, method, payload, timeout).await {
        Ok(result) => {
            let result_b64 = base64::engine::general_purpose::STANDARD.encode(&result);
            Json(json!({ "ok": true, "result_b64": result_b64 })).into_response()
        }
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => {
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": "timeout" }))).into_response()
        }
        Err(e) => dispatch_refused(e),
    }
}

// ── RPC serve / respond gateway handlers ─────────────────────────────────────

/// `GET /gateway/rpc/serve/{kind}` — SSE stream of incoming RPC requests.
///
/// Streams requests as `{"nonce_hex": "…", "sender": "IP:PORT", "payload_b64": "…"}`.
/// The receiver must call `POST /gateway/rpc/respond` with the same `nonce_hex` and
/// `sender` to complete the round-trip — **the same principal** that opened this stream: each
/// streamed request is recorded against it (`record_served_rpc`), and `rpc/respond` answers
/// nothing else. `mesh:serve` alone bound neither a kind nor a request, so any holder could
/// answer any in-flight call it learned a nonce for.
pub(super) async fn gw_rpc_serve(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
) -> axum::response::Response {
    // Post-360 row R's review: a kind this node serves itself is never handed to a client stream.
    if let Some(refused) = refuse_unservable_kind(&kind) {
        return refused;
    }
    // Round-2 review B: a client stream never shares a kind with a receiver this node serves itself — a
    // hosted component's `cap.invoke` kind, an operator's `rpc_rx` — because a signal fans to every
    // receiver. Refused here when one is registered; evicted (the stream ends) if one registers later.
    let Some(serve) = ctx.agent_ctx.signal_handlers.register_client(Arc::from(kind.as_str()), 256) else {
        warn!(%kind, "rpc/serve: refused, this node serves the kind in process");
        return (StatusCode::FORBIDDEN, Json(json!({
            "ok": false,
            "error": "served_in_process",
            "kind": kind,
            "message": format!("`{kind}` is served by a receiver on this node; a client stream for it would \
                                receive every such request this node is sent"),
        }))).into_response();
    };
    let mut evicted = serve.evicted;
    let seen_evicted = evicted.clone();
    let evicted_kind = kind.clone();
    let rx = serve.rx;
    let principal: Arc<str> = Arc::from(
        caller.map(|Extension(c)| c.principal).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string()).as_str(),
    );

    let http_ctx = Arc::clone(&ctx);
    let agent_ctx = Arc::clone(&ctx.agent_ctx);
    // Async, because closure plan C3's provider check runs the action preflight (and its journal
    // write) before a request is streamed to the SDK agent that serves it.
    let stream = futures_util::StreamExt::filter_map(ReceiverStream::new(rx), move |sig: crate::signal::Signal| {
      let agent_ctx = Arc::clone(&agent_ctx);
      let http_ctx = Arc::clone(&http_ctx);
      let principal = Arc::clone(&principal);
      async move {
        use base64::Engine as _;
        if sig.payload.len() < 8 { return None; }
        let req = super::rpc::RpcRequest::from(sig);
        // Item 7: a gateway-dispatched request carries a caller context. Verified, it is
        // handed to the served handler as `caller`; refused, the request is dropped here (the
        // served handler never sees a forged principal, and the caller times out).
        let caller = match gateway_caller::verify_received(&agent_ctx, &req) {
            Ok(Some(c)) => Some(json!({
                "principal": c.principal,
                "via":       c.via.to_string(),
                "scopes":    c.scopes,
                "attested":  matches!(c.attestation, super::gateway_caller::CallerAttestation::Signed { .. }),
            })),
            Ok(None) => None,
            Err(e) => {
                warn!(kind = %req.kind(), sender = %req.sender(), "rpc/serve: caller context refused: {e}");
                return None;
            }
        };
        // Closure plan C3: a refused protected call is answered here and never streamed.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        match super::provider_enforcement::check(&agent_ctx, &req).await {
            // C4: the admission is parked until the SDK agent replies through `/rpc/respond`.
            Ok(admission) => super::provider_enforcement::park(&agent_ctx, req.sender(), req.nonce(), admission),
            Err(refusal) => {
                warn!(kind = %req.kind(), sender = %req.sender(), reason = %refusal.reason,
                      "rpc/serve: refused by provider enforcement");
                super::rpc::rpc_respond_ctx(&agent_ctx, &req, Bytes::from(refusal.rpc_body()));
                return None;
            }
        }
        // Handed to this principal: the only one `rpc/respond` will take an answer from.
        record_served_rpc(&http_ctx, req.sender(), req.nonce(), &principal);
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(req.payload());
        let mut data = json!({
            "nonce_hex":   format!("{:016x}", req.nonce()),
            "sender":      req.sender().to_string(),
            "payload_b64": payload_b64,
        });
        if let Some(c) = caller {
            data["caller"] = c;
        }
        Some(Ok::<_, Infallible>(Event::default()
            .event(req.kind().as_ref())
            .data(data.to_string())))
      }
    });

    // Evicted: a terminal `evicted` event naming the kind, then the stream ends — so the client is told why,
    // rather than seeing a stream that simply stopped (#643's round-3 review; the SDKs raise on it).
    let stream = futures_util::StreamExt::take_until(stream, async move {
        let _ = evicted.wait_for(|e| *e).await;
    });
    let tail = futures_util::StreamExt::filter_map(futures_util::stream::once(async move {
        (*seen_evicted.borrow()).then(|| {
            Ok::<_, Infallible>(Event::default().event("evicted").data(json!({
                "error": "evicted",
                "kind": evicted_kind,
                "message": format!("`{evicted_kind}` is now served by a receiver on this node; this serve stream is closed"),
            }).to_string()))
        })
    }), |e| async move { e });
    let stream = futures_util::StreamExt::chain(stream, tail);
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// `POST /gateway/rpc/respond` — send a reply to an in-flight RPC request.
///
/// Body: `{"nonce_hex": "…", "sender": "IP:PORT", "result_b64": "…"}`.
/// Returns `{"ok": true}`. Only a request that `rpc/serve` streamed to **this principal** and
/// that is not yet answered is accepted; anything else — another principal's request, a nonce never
/// streamed, a second answer, one past the gateway's RPC ceiling — is `403 unserved_request` and
/// nothing is emitted or released.
pub(super) async fn gw_rpc_respond(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    use crate::signal::SignalScope;

    let nonce_hex = match body["nonce_hex"].as_str() {
        Some(s) => s,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing nonce_hex"}))).into_response(),
    };
    let nonce = match u64::from_str_radix(nonce_hex, 16) {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid nonce_hex"}))).into_response(),
    };
    let sender: crate::node_id::NodeId = match body["sender"].as_str().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing or invalid sender"}))).into_response(),
    };
    let result = if let Some(b64) = body["result_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 result"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // The request must have been handed to this principal on its own serve stream. Before this
    // check, `mesh:serve` bound to neither a kind nor a request: any holder could answer — and
    // pre-empt — any in-flight call whose nonce it learned, and release its parked admission.
    let principal = caller.map(|Extension(c)| c.principal)
        .unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    match answer_served_rpc(&ctx, &sender, nonce, &principal) {
        ServedAnswer::First => {}
        // A replicated serve loop: every replica of this principal received the request and the
        // first answer was delivered. The repeat is accepted and dropped — never a second
        // `rpc.result` — so the losing replica's loop is not killed by an error (#587 F1).
        ServedAnswer::Duplicate => {
            ctx.rpc_respond_duplicates.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Json(json!({ "ok": true, "duplicate": true })).into_response();
        }
        ServedAnswer::Unserved => {
            warn!(nonce = %nonce_hex, %sender, %principal, "rpc/respond: refused — not a request this principal was handed");
            return (StatusCode::FORBIDDEN, Json(json!({
                "ok": false,
                "error": "unserved_request",
                "message": "rpc/respond answers only a request your own rpc/serve stream delivered, within the gateway's 300 s window",
            }))).into_response();
        }
    }

    // Closure plan C4: the SDK agent has replied, so the call is no longer in flight.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    super::provider_enforcement::release_parked(&ctx.agent_ctx, &sender, nonce);

    let mut buf = BytesMut::with_capacity(8 + result.len());
    buf.put_u64_le(nonce);
    buf.put(result);
    super::helpers::emit_signal(
        &ctx.agent_ctx,
        Arc::from(crate::signal::signal_kind::RPC_RESULT),
        SignalScope::Individual(sender),
        buf.freeze(),
    );

    Json(json!({ "ok": true })).into_response()
}

// ── Scatter-gather gateway handler ────────────────────────────────────────────

/// `POST /gateway/scatter` — fan-out RPC to multiple targets, collect replies.
///
/// Body:
/// ```json
/// {
///   "targets":       ["IP:PORT", …],
///   "method":        "signal-kind",
///   "payload_b64":   "…",
///   "timeout_secs":  10,
///   "min_ok":        1
/// }
/// ```
/// Returns `{"ok": true, "replies": [{"sender": "…", "result_b64": "…"}, …]}` once
/// `min_ok` replies arrive, or `{"ok": false, "error": "…", "replies": […]}` on timeout.
pub(super) async fn gw_scatter(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);

    let targets: Vec<crate::node_id::NodeId> = match body["targets"].as_array() {
        Some(arr) => arr.iter()
            .filter_map(|v| v.as_str()?.parse().ok())
            .collect(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing targets"}))).into_response(),
    };
    let method: Arc<str> = match body["method"].as_str() {
        Some(m) => Arc::from(m),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing method"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &method) {
        return refused;
    }
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    let timeout_secs = body["timeout_secs"].as_u64().unwrap_or(10).clamp(1, 300);
    let timeout      = Duration::from_secs(timeout_secs);
    let min_ok       = body["min_ok"].as_u64().unwrap_or(1) as usize;

    let mut js: tokio::task::JoinSet<(crate::node_id::NodeId, Result<Bytes, GatewayDispatchError>)>
        = tokio::task::JoinSet::new();
    for target in targets {
        let c = Arc::clone(&ctx.agent_ctx);
        let k = Arc::clone(&method);
        let p = payload.clone();
        let t = target.clone();
        let who = caller.clone();
        js.spawn(async move {
            let res = gateway_caller::gateway_rpc_call(&c, who.as_ref(), t.clone(), k, p, timeout).await;
            (t, res)
        });
    }

    let mut replies: Vec<serde_json::Value> = Vec::new();
    // Item 7: a target refused before dispatch (no caller-context marker) is reported per
    // target, not folded into "insufficient replies".
    let mut refused: Vec<serde_json::Value> = Vec::new();
    while let Some(res) = js.join_next().await {
        match res {
            Ok((nid, Ok(bytes))) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                replies.push(json!({ "sender": nid.to_string(), "result_b64": b64 }));
                if replies.len() >= min_ok {
                    js.abort_all();
                    break;
                }
            }
            Ok((nid, Err(e))) if !matches!(e, GatewayDispatchError::Rpc(_)) => {
                refused.push(json!({ "target": nid.to_string(), "error": e.reason(), "detail": e.to_string() }));
            }
            _ => {}
        }
    }

    if replies.len() >= min_ok {
        Json(json!({ "ok": true, "replies": replies, "refused": refused })).into_response()
    } else {
        (StatusCode::GATEWAY_TIMEOUT,
         Json(json!({ "ok": false, "error": "insufficient replies", "replies": replies, "refused": refused })))
            .into_response()
    }
}

// ── Mailbox gateway handlers ──────────────────────────────────────────────────

/// `GET /gateway/mailbox/{kind}` — SSE stream of mailbox events for this node.
///
/// Streams events as `{"sender": "IP:PORT", "kind": "…", "payload_b64": "…"}`.
/// The subscription is torn down when the client disconnects. Each event's entry is deleted when
/// this stream hands it to the HTTP connection — not when the client reads it: what the connection
/// (or the kernel's socket buffers) had accepted when it drops is lost, the rest stay for the next
/// stream (post-360 hardening row E).
pub(super) async fn gw_mailbox_subscribe(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let events = mailbox_events(Arc::clone(&ctx.agent_ctx), Arc::from(kind.as_str()), ctx.shutdown_rx.clone());
    Sse::new(events.map(|data| {
        let kind = data["kind"].as_str().unwrap_or_default().to_string();
        Ok(Event::default().event(kind).data(data.to_string()))
    }))
    .keep_alive(KeepAlive::default())
}

/// The event stream behind `GET /gateway/mailbox/{kind}`: each item is one event's data object.
pub(super) fn mailbox_events(
    agent_ctx:   Arc<TaskCtx>,
    kind_arc:    Arc<str>,
    shutdown_rx: watch::Receiver<bool>,
) -> impl futures_util::Stream<Item = serde_json::Value> {
    let self_id = agent_ctx.node_id.clone();
    let (handle, rx) = super::mailbox::open_mailbox_acked(
        agent_ctx,
        &self_id,
        Arc::clone(&kind_arc),
        256,
        shutdown_rx,
    );

    ReceiverStream::new(rx).map(move |super::mailbox::AckedEvent { event, ack }| {
        use base64::Engine as _;
        let _ = &handle; // keep the MailboxHandle alive for the duration of the stream
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&event.payload);
        let data = json!({
            "sender":      event.sender.to_string(),
            "kind":        event.kind.as_ref(),
            "payload_b64": payload_b64,
        });
        // Post-360 row E: the entry is deleted now, as the stream hands the event to the connection —
        // not when it entered the 256-slot channel, which lost every queued event on a drop.
        ack.ack();
        data
    })
}

/// `POST /gateway/mailbox/deliver` — deliver an event to a target node's mailbox.
///
/// Body: `{"target": "IP:PORT", "kind": "…", "payload_b64": "…"}`.
/// Returns `{"ok": true}`.
pub(super) async fn gw_mailbox_deliver(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;

    let target: crate::node_id::NodeId = match body["target"].as_str().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing or invalid target"}))).into_response(),
    };
    let kind: Arc<str> = match body["kind"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing kind"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    super::mailbox::deliver_event_ctx(
        &ctx.agent_ctx,
        &ctx.agent_ctx.node_id,
        &target,
        kind,
        payload,
    );

    Json(json!({ "ok": true })).into_response()
}

// ── Cluster sharding ──────────────────────────────────────────────────────────

/// `GET /gateway/shard/{ns}/{name}?key=<shard_key>`
///
/// Returns the consistent-hash owner NodeId for `shard_key` among providers of
/// capability `ns/name`. The result is deterministic: every node with the same
/// provider view returns the same owner for the same key.
///
/// 200 `{"owner":"ip:port"}` — owner found.
/// 404 `{"error":"no providers"}` — no live providers match the filter.
#[derive(Deserialize)]
pub(super) struct ShardOwnerQuery { key: String }

pub(super) async fn gw_shard_owner(
    Path((ns, name)): Path<(String, String)>,
    Query(q):         Query<ShardOwnerQuery>,
    State(ctx):       State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;
    use super::sharding::shard_owner;

    let filter = CapFilter::new(ns.as_str(), name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    match shard_owner(&q.key, &providers) {
        Some(owner) => Json(json!({ "owner": owner.to_string() })).into_response(),
        None        => (StatusCode::NOT_FOUND, Json(json!({ "error": "no providers" }))).into_response(),
    }
}

/// `POST /gateway/shard/emit`
///
/// Emits `kind` signal to the consistent-hash owner for `shard_key` among
/// providers of `ns/name`. Equivalent to calling `emit_sharded` from Rust.
///
/// Request body:
/// ```json
/// { "kind": "actor.msg", "ns": "actor", "name": "user",
///   "shard_key": "user-12345", "payload_b64": "<base64>" }
/// ```
/// Response 200: `{"ok":true,"owner":"ip:port"}`
/// Response 404: `{"ok":false,"error":"no providers"}`
pub(super) async fn gw_shard_emit(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    use crate::capability::CapFilter;
    use super::sharding::shard_owner;
    use crate::signal::SignalScope;

    let kind = match body["kind"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing kind"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let ns   = match body["ns"].as_str()   { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing ns"}))).into_response() };
    let name = match body["name"].as_str() { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing name"}))).into_response() };
    let shard_key = match body["shard_key"].as_str() {
        Some(s) => s.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing shard_key"}))).into_response(),
    };
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b)  => Bytes::from(b),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // As for `/gateway/signal/emit`: a client-supplied caller-context frame would verify as this
    // gateway's own envelope, because the raw emission carries the bytes verbatim with this node as
    // the sender. Found by the Phase-C adversarial audit (items 1+2+7).
    if super::gateway_caller::carries_caller_frame(&payload) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "payload carries a caller-context frame; that context is constructed by the gateway, not supplied"
        }))).into_response();
    }

    let filter    = CapFilter::new(ns.as_str(), name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    match shard_owner(&shard_key, &providers) {
        Some(owner) => {
            super::helpers::emit_signal_async(
                &ctx.agent_ctx, kind, SignalScope::Individual(owner.clone()), payload,
            ).await;
            Json(json!({ "ok": true, "owner": owner.to_string() })).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "no providers" }))).into_response(),
    }
}
