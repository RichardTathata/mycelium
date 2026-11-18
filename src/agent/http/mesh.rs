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

/// Identity-and-lifecycle I4, PR 3b: was `(sender, nonce)` streamed to `principal` within the window — and is it
/// answered? `None`: not streamed to it. Reads only; `answer_served_rpc` is the one that marks.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(super) fn served_rpc_answered(ctx: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64, principal: &str) -> Option<bool> {
    let now = ctx.agent_ctx.hlc.decision_now_ms();
    let served = ctx.served_rpcs.lock().unwrap_or_else(|e| e.into_inner());
    match served.map.get(&(sender.clone(), nonce, principal.to_string())) {
        Some((at, _)) if now.saturating_sub(*at) > SERVED_RPC_TTL_MS => None,
        Some((_, answered)) => Some(*answered),
        None => None,
    }
}

/// `POST /gateway/rpc/call`
///
/// Sends a blocking RPC call to a named node. `payload_b64` is base64.
/// Returns `{ "ok": true, "result_b64": "…" }` or `{ "ok": false, "error": "timeout" }`.
/// What a raw dispatch did, for a door's record (Y11): a reply is completed; a refusal before sending is nothing; a
/// timeout or a transport error is unknown — never a negative.
pub(super) fn door_outcome(dispatched: &Result<Bytes, GatewayDispatchError>) -> crate::agent::door::DoorOutcome {
    use crate::agent::door::DoorOutcome;
    match dispatched {
        Ok(_) => DoorOutcome::Completed,
        Err(GatewayDispatchError::Rpc(_)) => DoorOutcome::Unknown,
        Err(_) => DoorOutcome::Nothing,
    }
}

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
    // Closure plan C1: a raw route never carries protected work around the door that checks it. A companion's kind
    // was already refused by the auth layer unless the token holds the companion's family scope (external review
    // 2026-10-31 F1, `authz::companion_kind_gate`): a kind sent here can reach an `rpc_rx` receiver for it.
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

    // Adopter-surface Y11 (#708's review, finding 1): a raw call of a companion's door kind — a tuple take, a board
    // claim — runs that door's decision, selected or untouched exactly as the door is, when this node hosts the
    // companion that registered the kind; otherwise the call passes undecided (pinned in tests/exposure_doors.rs).
    let door = match crate::agent::door::decide_raw(&ctx.agent_ctx, caller.as_ref(), &method, &payload, &body, "gateway:rpc/call").await {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    let dispatched = gateway_caller::gateway_rpc_call(&ctx.agent_ctx, caller.as_ref(), target, method, payload, timeout).await;
    if let Some(pass) = door {
        pass.finish(door_outcome(&dispatched)).await;
    }
    match dispatched {
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
///
/// **Cancellation** (identity-and-lifecycle I4, PR 3b; `served_call` has who promises what). Two additive query
/// parameters, each the subscriber's own word: `cancel_events=true` — it reads a `cancel` event on this stream
/// (`{"cancel": true, "nonce_hex", "sender", "reason": "cancelled", …}`), sent when the node stops waiting for a call
/// it streamed; `stops_on_cancel=true` — it **declares that it stops** a call on that event (and so reads them). Only
/// a declaring subscriber is served a call under a cancellable rule; an undeclared one is refused it by name
/// (`door_cannot_stop`, `data.door` `sdk_serve`). A stream opened with neither is sent no `cancel` event, since an
/// older client would read one as a request.
pub(super) async fn gw_rpc_serve(
    Path(kind):  Path<String>,
    Query(cancel): Query<ServeCancel>,
    State(ctx):  State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
) -> axum::response::Response {
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if let Err(message) = cancel.window() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": "invalid_read_ahead", "message": message }))).into_response();
    }
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

    // The serve loop runs on its own task and hands events to the stream through a small channel, so a provider check
    // that settles a seat claim runs beside it — bounded, answered when the stream ends — and one claim never holds
    // up the requests behind it (#642's review, round 3, finding 1). Closure plan C3: a refused protected call is
    // answered here and never streamed.
    // **Pulled, never pushed** (#642's review, round 4, finding 4): the stream asks for one event at a time, and the
    // loop takes a request off the channel only while an ask is open — so it never runs ahead of its client, and a
    // request it has taken is either streamed or answered when the loop ends.
    let (pull_tx, pull_rx) = tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<Event>>(1);
    let loop_ctx = Arc::clone(&ctx);
    let loop_evicted = evicted.clone();
    // This stream's identifier (I4, PR 3b): in the `Mycelium-Serve-Stream` header and in every event it carries.
    let stream_id = new_stream_id();
    ctx.agent_ctx.spawn_task(serve_stream_loop(loop_ctx, rx, pull_rx, principal, loop_evicted, cancel, stream_id));
    let stream = futures_util::stream::unfold(pull_tx, |pull_tx| async move {
        let (want, event) = tokio::sync::oneshot::channel();
        pull_tx.send(want).await.ok()?;
        let event = event.await.ok()?;
        Some((Ok::<_, Infallible>(event), pull_tx))
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
    let mut response = Sse::new(stream).keep_alive(KeepAlive::default()).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&stream_name(stream_id)) {
        response.headers_mut().insert("mycelium-serve-stream", v);
    }
    response
}

/// `rpc/serve`'s cancellation parameters (identity-and-lifecycle I4, PR 3b): what the subscriber says of itself.
#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
pub(super) struct ServeCancel {
    /// The subscriber reads `cancel` events.
    #[serde(default)]
    cancel_events: bool,
    /// The subscriber declares that it stops a call on its `cancel` event — its promise (implies `cancel_events`).
    #[serde(default)]
    stops_on_cancel: bool,
    /// For a declaring stream: how many requests it may hold unanswered before the node stops handing it more
    /// (1..=[`SERVE_WINDOW`], which is the default). Read as text and checked by the route ([`ServeCancel::window`]):
    /// a declaring stream's value outside the range, or not a number, is refused by name; an undeclared stream's is
    /// ignored, since it has no window.
    #[serde(default)]
    read_ahead: Option<String>,
}

impl ServeCancel {
    /// A declaring stream's window, or the refusal of a `read_ahead` that is not a whole number in
    /// 1..=[`SERVE_WINDOW`] (#806's second confirm round, D2: refused, never clamped). `Ok(None)`: undeclared.
    #[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
    fn window(&self) -> Result<Option<usize>, String> {
        if !self.stops_on_cancel {
            return Ok(None);
        }
        match self.read_ahead.as_deref() {
            None => Ok(Some(SERVE_WINDOW)),
            Some(text) => match text.parse::<usize>() {
                Ok(n) if (1..=SERVE_WINDOW).contains(&n) => Ok(Some(n)),
                _ => Err(format!("`read_ahead` must be a whole number from 1 to {SERVE_WINDOW}; got `{text}`")),
            },
        }
    }
}

/// **The most requests a declaring serve stream holds unanswered** (#806's confirm round, C1): the depth of the
/// stream's own channel. A declaring SDK reads its stream without pause, so a `cancel` event is never stuck behind
/// requests; the backpressure that pause used to give is the node's own instead — it counts what it handed the
/// stream and the stream has not ended (`rpc/respond` naming the stream; or 300 s), and past this window hands it no
/// further request. Requests then wait in the stream's channel, whose fill the node's admission reads
/// (`SignalHandlers::fill_ratio`), as for any slow worker.
#[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
pub(super) const SERVE_WINDOW: usize = 256;

/// A stream's name on the wire: its 128-bit id, 32 hex digits.
pub(super) fn stream_name(stream: u128) -> String {
    format!("{stream:032x}")
}

/// A stream's id from its name.
pub(super) fn stream_from_name(name: &str) -> Option<u128> {
    (name.len() == 32).then(|| u128::from_str_radix(name, 16).ok()).flatten()
}

/// **A new serve stream's id: 128 random bits** (#806's confirm round, C3), so a process that holds the same token
/// cannot name a sibling's stream: each stream speaks only for its own work by construction, not by convention. Drawn
/// through the sim seam as an external input (recorded, and not redrawn, under a replay): from the operating system
/// in a build with `tls` — the only build with principals and watchers — and from the seam's own generator otherwise.
fn new_stream_id() -> u128 {
    let name = mycelium_core::sim_seam::input("rpc-serve/stream-id", "", || {
        #[cfg(feature = "tls")]
        let id = {
            let mut bytes = [0u8; 16];
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
            u128::from_be_bytes(bytes)
        };
        #[cfg(not(feature = "tls"))]
        let id = (u128::from(mycelium_core::sim_seam::rng_u64_from("nonce", 0)) << 64) | u128::from(mycelium_core::sim_seam::rng_u64_from("nonce", 0));
        stream_name(id)
    });
    stream_from_name(&name).unwrap_or_default()
}

/// What `rpc/serve` streams a checked request as, or `None` for one it drops: the caller context verified and handed
/// on, the request recorded as served to this principal.
fn serve_event(http_ctx: &HttpCtx, req: &super::rpc::RpcRequest, caller: Option<serde_json::Value>, principal: &str, stream: u128) -> Event {
    use base64::Engine as _;
    // Handed to this principal: the only one `rpc/respond` will take an answer from.
    record_served_rpc(http_ctx, req.sender(), req.nonce(), principal);
    let payload_b64 = base64::engine::general_purpose::STANDARD.encode(req.payload());
    let mut data = json!({
        "nonce_hex":   format!("{:016x}", req.nonce()),
        "sender":      req.sender().to_string(),
        "payload_b64": payload_b64,
        // The stream this request went out on (I4, PR 3b; the maintainer's decision of 2026-11-17): `rpc/respond`
        // names it, so a stream's answer and its confirmation speak only for its own work.
        "stream":      stream_name(stream),
    });
    if let Some(c) = caller {
        data["caller"] = c;
    }
    Event::default().event(req.kind().as_ref()).data(data.to_string())
}

/// **The serve loop's queue pass**: take out of `queue`, in order, every checked call whose authority has lapsed (to be
/// answered `cancelled` without being handed out). It runs **whatever the stream's window** — `window_full` is taken
/// only so that this is said where it is decided (#806's second confirm round, D3: the pass once sat inside the
/// hand-out loop, which a full window skips) — and not while a test holds the queue.
#[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
fn take_lapsed<T>(queue: &mut std::collections::VecDeque<T>, held: bool, window_full: bool, lapsed: impl Fn(&T) -> bool) -> Vec<T> {
    let _ = window_full;
    if held {
        return Vec::new();
    }
    let mut taken = Vec::new();
    for _ in 0..queue.len() {
        let Some(item) = queue.pop_front() else { break };
        if lapsed(&item) {
            taken.push(item);
        } else {
            queue.push_back(item);
        }
    }
    taken
}

/// The `rpc/serve` loop: receive, verify the caller context, check — inline, or beside the loop when the check settles
/// a seat claim — and stream, **one event per ask of the stream**. It takes a request off the channel only while an
/// ask is open and nothing checked is waiting to be streamed, so it never runs ahead of its client. It ends when the
/// client goes, on eviction or at shutdown; a request it took and did not stream — checked and waiting, or still
/// settling a claim — is then answered, never dropped.
async fn serve_stream_loop(
    ctx: Arc<HttpCtx>,
    mut rx: tokio::sync::mpsc::Receiver<crate::signal::Signal>,
    mut pull: tokio::sync::mpsc::Receiver<tokio::sync::oneshot::Sender<Event>>,
    principal: Arc<str>,
    mut evicted: tokio::sync::watch::Receiver<bool>,
    #[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(unused_variables))] cancel: ServeCancel,
    stream: u128,
) {
    let agent_ctx = Arc::clone(&ctx.agent_ctx);
    // A declaring stream's window ([`SERVE_WINDOW`]): what it was handed and has not ended, by `(sender, nonce)` and
    // when; `rpc/respond` tells this loop of each end through `HttpCtx::serve_streams`.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let window = cancel.window().unwrap_or(None);
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let mut outstanding: HashMap<(crate::node_id::NodeId, u64), u64> = HashMap::new();
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let (ended_tx, mut ended_rx) = tokio::sync::mpsc::unbounded_channel::<(crate::node_id::NodeId, u64)>();
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if window.is_some() {
        ctx.serve_streams.pin().insert(stream, (Arc::clone(&principal), ended_tx));
    }
    // The `cancel` events the node's own stops send this stream (`served_call`), if its subscriber reads them; streamed
    // ahead of any request.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let hears_cancel = (cancel.cancel_events || cancel.stops_on_cancel).then(|| cancel_tx.clone());
    let mut shutdown = agent_ctx.shutdown_tx.subscribe();
    // An open ask of the stream, and the requests checked and admitted while none was open.
    let mut want: Option<tokio::sync::oneshot::Sender<Event>> = None;
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let mut ready: std::collections::VecDeque<(super::rpc::RpcRequest, Option<serde_json::Value>, super::provider_enforcement::Admission)> =
        std::collections::VecDeque::new();
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let mut pending: super::provider_enforcement::DeferredChecks<(super::rpc::RpcRequest, Option<serde_json::Value>)> =
        super::provider_enforcement::DeferredChecks::new(&agent_ctx);
    loop {
        // Stream what is ready while the stream asks.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let mut gone = false;
        #[cfg(all(feature = "gateway", feature = "tls"))]
        while want.is_some() && let Ok(event) = cancel_rx.try_recv() {
            if want.take().map(|w| w.send(event)).is_none_or(|r| r.is_err()) {
                gone = true;
                break;
            }
        }
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let held = test_hooks::streaming_held(&agent_ctx);
        // The window: full, nothing more is handed out (and nothing more taken off the channel) until the stream ends
        // something or it expires. `cancel` events, above, are never held back.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let mut full = window.is_some_and(|w| outstanding.len() >= w);
        #[cfg(all(feature = "gateway", feature = "tls"))]
        if full {
            let now = agent_ctx.hlc.decision_now_ms();
            outstanding.retain(|_, at| now.saturating_sub(*at) <= SERVED_RPC_TTL_MS);
            full = window.is_some_and(|w| outstanding.len() >= w);
        }
        // A call whose authority lapsed while it waited here was never handed to anyone, so the node's stop is the
        // whole of it: answered `cancelled` and its registration ended, a stop the node saw complete whatever the
        // stream declared (#806's review, L7). **When:** on this loop's next turn ([`take_lapsed`]), whether or not the
        // window has room — within a second with a full window (its timer turns the loop); with room in the window and
        // a stream that is not asking, when the stream next asks or closes, since nothing else turns the loop then.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        for (req, _, admission) in take_lapsed(&mut ready, held, full, |(_, _, admission)| admission.is_lapsed()) {
            super::rpc::rpc_respond_ctx(&agent_ctx, &req, super::served_call::cancelled_body("sdk_serve"));
            super::provider_enforcement::count_door_stop(&agent_ctx, "sdk_serve");
            admission.release_confirmed_by_node();
        }
        #[cfg(all(feature = "gateway", feature = "tls"))]
        while !gone && !held && !full && want.is_some() && let Some((req, caller, mut admission)) = ready.pop_front() {
            // A watcher is needed and none can be held: refused before streaming, never streamed unwatched.
            if admission.holds_anything() && super::served_call::full(&ctx) {
                refuse_served(&agent_ctx, &req, &super::provider_enforcement::served_calls_full());
                continue;
            }
            admission.promised_stop = cancel.stops_on_cancel;
            // The watcher is placed before the event goes out, so an answer that comes at once finds it (#806's review,
            // L6).
            let placed = admission.holds_anything().then(|| super::served_call::place(&ctx, &req, &principal, stream, cancel.stops_on_cancel));
            let event = serve_event(&ctx, &req, caller.clone(), &principal, stream);
            let sent = want.take().map(|w| w.send(event));
            if matches!(sent, Some(Ok(()))) && let Some(w) = window {
                outstanding.insert((req.sender().clone(), req.nonce()), agent_ctx.hlc.decision_now_ms());
                full = outstanding.len() >= w;
            }
            match (sent, placed) {
                (Some(Ok(())), Some(placed)) => placed.start(&ctx, &req, Arc::clone(&principal), admission, hears_cancel.clone()),
                (Some(Ok(())), None) => {}
                (_, placed) => {
                    if let Some(placed) = placed {
                        placed.withdraw(&ctx);
                    }
                    ready.push_front((req, caller, admission));
                    gone = true;
                    break;
                }
            }
        }
        #[cfg(all(feature = "gateway", feature = "tls"))]
        if gone {
            break;
        }
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let take = want.is_some() && ready.is_empty() && !pending.is_full() && !full;
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let take = want.is_some();
        let asking = want.is_none();
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let sig = tokio::select! { biased;
            _ = async { let _ = shutdown.wait_for(|v| *v).await; } => break,
            _ = async { let _ = evicted.wait_for(|e| *e).await; } => break,
            _ = async { if let Some(w) = want.as_mut() { w.closed().await } }, if !asking => break,
            ask = pull.recv(), if asking => match ask {
                Some(w) => { want = Some(w); continue; }
                None => break,
            },
            // The stream ended a request it was handed: room in its window.
            Some(key) = ended_rx.recv() => {
                outstanding.remove(&key);
                continue;
            }
            // A full window is looked at again each second, for what has expired.
            _ = mycelium_core::sim_seam::sleep_ms("rpc-serve/window", 1_000), if full => continue,
            // Test hook: streaming held, the loop wakes to see it released.
            _ = mycelium_core::sim_seam::sleep_ms("rpc-serve/test-hold", 20), if held => continue,
            // A `cancel` event to send: taken while an ask is open, and sent at the top of the loop.
            Some(event) = cancel_rx.recv(), if !asking => {
                if want.take().map(|w| w.send(event)).is_none_or(|r| r.is_err()) {
                    break;
                }
                continue;
            }
            Some(((req, caller), checked)) = pending.next(), if !pending.is_empty() => {
                match checked {
                    Ok(admission) => ready.push_back((req, caller, admission)),
                    Err(refusal) => refuse_served(&agent_ctx, &req, &refusal),
                }
                continue;
            }
            sig = rx.recv(), if take => match sig {
                Some(sig) => sig,
                None => break,
            },
        };
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let sig = tokio::select! { biased;
            _ = async { let _ = shutdown.wait_for(|v| *v).await; } => break,
            _ = async { let _ = evicted.wait_for(|e| *e).await; } => break,
            _ = async { if let Some(w) = want.as_mut() { w.closed().await } }, if !asking => break,
            ask = pull.recv(), if asking => match ask {
                Some(w) => { want = Some(w); continue; }
                None => break,
            },
            sig = rx.recv(), if take => match sig {
                Some(sig) => sig,
                None => break,
            },
        };
        if sig.payload.len() < 8 {
            continue;
        }
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
                continue;
            }
        };
        #[cfg(all(feature = "gateway", feature = "tls"))]
        match super::provider_enforcement::check(&agent_ctx, &req, true, super::provider_enforcement::Door::SdkServe { stops: cancel.stops_on_cancel }).await {
            super::provider_enforcement::Checked::Done(Ok(admission)) => ready.push_back((req, caller, admission)),
            super::provider_enforcement::Checked::Done(Err(refusal)) => refuse_served(&agent_ctx, &req, &refusal),
            super::provider_enforcement::Checked::Waiting(waiting) => pending.spawn(waiting, (req, caller)),
        }
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        {
            let event = serve_event(&ctx, &req, caller, &principal, stream);
            if want.take().map(|w| w.send(event)).is_none_or(|r| r.is_err()) {
                break;
            }
        }
    }
    // A request this loop took and did not stream is answered: checked and waiting for an ask, or still settling.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        if window.is_some() {
            ctx.serve_streams.pin().remove(&stream);
        }
        for (req, _, admission) in ready.drain(..) {
            // Cancelled while it waited, and never handed out: answered `cancelled`, the node's own confirmed stop —
            // by what happened, as in the queue (#806's confirm round, on L7).
            if admission.is_lapsed() {
                super::rpc::rpc_respond_ctx(&agent_ctx, &req, super::served_call::cancelled_body("sdk_serve"));
                super::provider_enforcement::count_door_stop(&agent_ctx, "sdk_serve");
                admission.release_confirmed_by_node();
                continue;
            }
            refuse_served(&agent_ctx, &req, &super::provider_enforcement::withdrawn());
        }
        for (req, _) in pending.abandon() {
            refuse_served(&agent_ctx, &req, &super::provider_enforcement::withdrawn());
        }
    }
}

/// Answer a served request the provider check refused.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn refuse_served(
    agent_ctx: &Arc<super::TaskCtx>,
    req: &super::rpc::RpcRequest,
    refusal: &super::provider_enforcement::ProviderRefusal,
) {
    warn!(kind = %req.kind(), sender = %req.sender(), reason = %refusal.reason,
          "rpc/serve: refused by provider enforcement");
    super::rpc::rpc_respond_ctx(agent_ctx, req, Bytes::from(refusal.rpc_body()));
}

/// `POST /gateway/rpc/respond` — send a reply to an in-flight RPC request.
///
/// Body: `{"nonce_hex": "…", "sender": "IP:PORT", "result_b64": "…"}`, or — identity-and-lifecycle I4, PR 3b — `{"nonce_hex",
/// "sender", "stopped": true}`: the subscriber's confirmation that it stopped a call the node cancelled (`served_call`),
/// answered `{"ok": true, "stopped": true}`, and refused `409 not_cancelled` for any other request. An answer to a call
/// the node cancelled is discarded and answered `{"ok": true, "cancelled": true}`.
/// Returns `{"ok": true}`. Only a request that `rpc/serve` streamed to **this principal** and
/// that is not yet answered is accepted; anything else — another principal's request, a nonce never
/// streamed, a second answer, one past the gateway's RPC ceiling — is `403 unserved_request` and
/// nothing is emitted or released.
///
/// **The window's one release rule** (identity-and-lifecycle I4, PR 3b; #806's third confirm round, N1): *every
/// answer of this route in the 2xx class, to a respond that names a stream its own principal opened, ends that
/// `(sender, nonce)` on that stream; any other answer ends nothing.* It is applied here, in one place, after the
/// status is decided — whatever the 2xx says (`ok`, `duplicate`, `cancelled`, `stopped`).
pub(super) async fn gw_rpc_respond(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> axum::response::Response {
    let principal = caller.map(|Extension(c)| c.principal).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let names = (
        body["stream"].as_str().and_then(stream_from_name),
        body["sender"].as_str().and_then(|s| s.parse::<crate::node_id::NodeId>().ok()),
        body["nonce_hex"].as_str().and_then(|n| u64::from_str_radix(n, 16).ok()),
    );
    let response = respond(&ctx, &principal, body);
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if response.status().is_success()
        && let (Some(stream), Some(sender), Some(nonce)) = names
        && let Some((opened_by, tx)) = ctx.serve_streams.pin().get(&stream)
        && opened_by.as_ref() == principal
    {
        let _ = tx.send((sender, nonce));
    }
    response
}

/// `rpc/respond`, judged: the answer, with nothing done to any stream's window ([`gw_rpc_respond`] does that, by the
/// answer's status).
fn respond(ctx: &Arc<HttpCtx>, principal: &str, body: serde_json::Value) -> axum::response::Response {
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
    let principal = principal.to_string();
    let ctx = Arc::clone(ctx);

    // Identity-and-lifecycle I4, PR 3b: the subscriber's own word that it stopped a call the node cancelled
    // (`"stopped": true`) — the only thing a confirmed stop is recorded on. Accepted only for a call streamed to this
    // principal that the node stopped waiting for; anything else is refused `not_cancelled` and nothing moves.
    // The stream the answer comes from (the maintainer's decision of 2026-11-17): a confirmation speaks only for that
    // stream's work, never a sibling's. Absent: a client before streams were named.
    #[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(unused_variables))]
    let stream: Option<u128> = body["stream"].as_str().and_then(stream_from_name);
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let served = super::served_call::find(&ctx, &sender, nonce);
    if body["stopped"].as_bool() == Some(true) {
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let refused = match (served.as_ref(), served_rpc_answered(&ctx, &sender, nonce, &principal)) {
            // Never streamed to this principal (#806's review, M5): not its call to confirm.
            (_, None) => super::served_call::NotConfirmed::NotYourStream,
            (Some(s), Some(true)) => match s.confirm(stream, &principal) {
                Ok(()) => {
                    s.assess(&ctx.agent_ctx);
                    return Json(json!({ "ok": true, "stopped": true })).into_response();
                }
                Err(why) => why,
            },
            _ => super::served_call::NotConfirmed::NotCancelled,
        };
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let refused = "not_cancelled";
        #[cfg(all(feature = "gateway", feature = "tls"))]
        let refused = refused.label();
        return (StatusCode::CONFLICT, Json(json!({
            "ok": false,
            "error": refused,
            "message": "\"stopped\" confirms a stop of a call this node cancelled and streamed to you, on the stream it names (\"stream\", the one the call went out on); this request is not one",
        }))).into_response();
    }
    match answer_served_rpc(&ctx, &sender, nonce, &principal) {
        ServedAnswer::First => {}
        // I4, PR 3b: the node stopped waiting for this call (its authority lapsed) and answered the requester
        // `cancelled`: a later answer is discarded, never sent — and assessed, if the subscriber declared it stops.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        ServedAnswer::Duplicate if served.as_ref().is_some_and(|s| s.cancelled()) => {
            if let Some(s) = &served {
                s.assess(&ctx.agent_ctx);
                s.answered(stream, &principal);
            }
            return Json(json!({ "ok": true, "cancelled": true })).into_response();
        }
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

    // Closure plan C4: the SDK agent has replied, so the call is no longer in flight — its watcher releases what it
    // holds (I4, PR 3b: `served_call`).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if let Some(s) = &served {
        s.answered(stream, &principal);
    }

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
    // Closure plan C1: a raw route never carries protected work around the door that checks it. A companion's kind
    // was already refused by the auth layer unless the token holds the companion's family scope (external review
    // 2026-10-31 F1, `authz::companion_kind_gate`): a kind sent here can reach an `rpc_rx` receiver for it.
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

    // Adopter-surface Y11 (#708's review, finding 1): a scatter of a companion's door kind runs that door's decision
    // once, before any target is called — when this node hosts the companion that registered the kind.
    let door = match crate::agent::door::decide_raw(&ctx.agent_ctx, caller.as_ref(), &method, &payload, &body, "gateway:scatter").await {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };

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
    if let Some(pass) = door {
        use crate::agent::door::DoorOutcome;
        // A reply is a take done somewhere; no reply, and every target refused before dispatch, is nothing; otherwise
        // a target may have acted without answering.
        pass.finish(if !replies.is_empty() {
            DoorOutcome::Completed
        } else if refused.is_empty() {
            DoorOutcome::Unknown
        } else {
            DoorOutcome::Nothing
        }).await;
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
///
/// A kind `mailbox/deliver` refuses is refused here the same way (2026-11-13): protected work, an RPC reply kind,
/// one of the substrate's own, a hosted capability's. The gateway's `mailbox/deliver` cannot have filled such a
/// mailbox, but a **member** can — `ServiceHandle::deliver_event` checks no kind — so a client that read one a
/// member fills (an operator-listed protected kind, an application's `sys.alerts`) is now refused, and the
/// entries stay under `mailbox/` until they expire: rename the mailbox's kind. The refusal keeps the two halves
/// of the gateway's mailbox one rule.
pub(super) async fn gw_mailbox_subscribe(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> axum::response::Response {
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let events = mailbox_events(Arc::clone(&ctx.agent_ctx), Arc::from(kind.as_str()), ctx.shutdown_rx.clone());
    Sse::new(events.map(|data| {
        let kind = data["kind"].as_str().unwrap_or_default().to_string();
        Ok::<_, Infallible>(Event::default().event(kind).data(data.to_string()))
    }))
    .keep_alive(KeepAlive::default())
    .into_response()
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
    caller: Option<axum::Extension<crate::agent::ResolvedPrincipal>>,
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
    // Closure plan C1: a raw route never carries protected work around the door that checks it. A companion's kind
    // was already refused by the auth layer unless the token holds the companion's family scope (external review
    // 2026-10-31 F1, `authz::companion_kind_gate`). A delivery writes a `mailbox/` key and reaches no handler, so a
    // companion's kind could not reach its `rpc_rx` this way; the gate covers every kind-carrying raw route alike.
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

    // Adopter-surface Y11: decided only when an exposure rule selects `mailbox.deliver` on this kind.
    let caller = caller.map(|axum::Extension(c)| c);
    let door = crate::agent::door::decide(
        &ctx.agent_ctx, crate::agent::door::DoorActor::Caller(caller.as_ref()), crate::agent::door::OP_MAILBOX_DELIVER,
        &crate::agent::door::signal_resource(&kind), &body, "gateway:mailbox/deliver",
    ).await;
    let pass = match door {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    let ok = super::mailbox::deliver_event_ctx(
        &ctx.agent_ctx,
        &ctx.agent_ctx.node_id,
        &target,
        kind,
        payload,
    );
    // Written locally either way; `false` is a full gossip queue, so whether it reached the target is unknown.
    pass.finish(if ok { crate::agent::door::DoorOutcome::Completed } else { crate::agent::door::DoorOutcome::Unknown }).await;

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
    caller: Option<axum::Extension<crate::agent::ResolvedPrincipal>>,
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
    // Closure plan C1: a raw route never carries protected work around the door that checks it. A companion's kind
    // was already refused by the auth layer unless the token holds the companion's family scope (external review
    // 2026-10-31 F1, `authz::companion_kind_gate`): a kind sent here can reach an `rpc_rx` receiver for it.
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

    // Adopter-surface Y11: decided only when an exposure rule selects `shard.emit` on this kind.
    let caller = caller.map(|axum::Extension(c)| c);
    let door = crate::agent::door::decide(
        &ctx.agent_ctx, crate::agent::door::DoorActor::Caller(caller.as_ref()), crate::agent::door::OP_SHARD_EMIT,
        &crate::agent::door::signal_resource(&kind), &body, "gateway:shard/emit",
    ).await;
    let pass = match door {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    match shard_owner(&shard_key, &providers) {
        Some(owner) => {
            let ok = super::helpers::emit_signal_async(
                &ctx.agent_ctx, kind, SignalScope::Individual(owner.clone()), payload,
            ).await;
            // `false`: handed to no route; local delivery, if any, done — unknown, as the emit door records it.
            pass.finish(if ok { crate::agent::door::DoorOutcome::Completed } else { crate::agent::door::DoorOutcome::Unknown }).await;
            Json(json!({ "ok": true, "owner": owner.to_string() })).into_response()
        }
        None => {
            pass.finish(crate::agent::door::DoorOutcome::Nothing).await;
            (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "no providers" }))).into_response()
        }
    }
}

/// Test hooks for the serve loop.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) mod test_hooks {
    /// Nodes whose serve loops hold what they have checked instead of streaming it (#806's review, L7: a call that
    /// lapses while it waits in the queue).
    #[cfg(test)]
    pub(crate) static HOLD_STREAMING: std::sync::LazyLock<papaya::HashSet<String>> = std::sync::LazyLock::new(papaya::HashSet::new);

    #[cfg(test)]
    pub(super) fn streaming_held(ctx: &crate::agent::TaskCtx) -> bool {
        HOLD_STREAMING.pin().contains(&ctx.node_id.to_string())
    }

    #[cfg(not(test))]
    pub(super) fn streaming_held(_: &crate::agent::TaskCtx) -> bool {
        false
    }
}

#[cfg(test)]
mod queue_pass_tests {
    use super::take_lapsed;
    use std::collections::VecDeque;

    /// **A cancelled call waiting in the serve loop's queue is taken out whether or not the window has room** (#806's
    /// second confirm round, D3; tested directly in the third). Of three queued calls the first and last have lapsed:
    /// with a **full** window both are taken, in order, and the live one stays; the same with room; and nothing is
    /// taken while a test holds the queue. Seen failing first with the pass skipped while the window is full.
    #[test]
    fn the_queue_pass_takes_lapsed_calls_whatever_the_window() {
        let queue = || VecDeque::from([("a", true), ("b", false), ("c", true)]);
        for window_full in [true, false] {
            let mut q = queue();
            let taken = take_lapsed(&mut q, false, window_full, |(_, lapsed)| *lapsed);
            assert_eq!(taken, vec![("a", true), ("c", true)], "window_full {window_full}");
            assert_eq!(q, VecDeque::from([("b", false)]), "window_full {window_full}");
        }
        let mut q = queue();
        assert!(take_lapsed(&mut q, true, false, |(_, lapsed)| *lapsed).is_empty());
        assert_eq!(q, queue());
    }
}
