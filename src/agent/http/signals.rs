//! The signal doors: `/signals/{kind}`, `/gateway/signal/emit`, `/gateway/signal/sse/{kind}`.

use super::*;

/// SSE endpoint — streams admitted signals of the requested `kind`.
///
/// Each event carries:
/// - `event` field: the signal kind
/// - `data` field: JSON `{"kind","sender","payload_b64","nonce","payload"}` — the gateway stream's shape
///   (`kind` since 2.24.0; `payload_b64` and `nonce` since 2.26.0), plus `payload`, the same base64 under
///   the name this route used first, kept for existing readers
///
/// The subscription is torn down automatically when the client disconnects.
pub(super) async fn signal_sse_handler(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Response {
    // Observing protected work is refused like sending it (closure plan C1, the SSE half): this
    // stream registers on the same table `rpc/serve` and the native MCP tools register on, and a
    // signal fans to every receiver — so a `mesh:read` holder would read each protected request's
    // whole frame (caller envelope, carried mandate and possession proof, correlation nonce).
    if let Some(refused) = refuse_unobservable_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    // A tap: an observer, never counted in the kind's fill, so a stalled or fast SSE client neither
    // vetoes nor hides the kind's workers (#602's review, finding 4).
    let rx = ctx.agent_ctx.signal_handlers.register_tap(
        std::sync::Arc::from(kind.as_str()),
        256,
    );

    let stream = ReceiverStream::new(rx).map(|sig: crate::signal::Signal| {
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&sig.payload);
        // The gateway stream's shape, so one parser reads both routes (doc-coverage run 20): `payload_b64`
        // and the `nonce` added; `payload` kept, the same bytes, for readers of the original shape.
        let data = json!({
            "kind":        sig.kind.as_ref(),
            "sender":      sig.sender.to_string(),
            "payload_b64": payload_b64.clone(),
            "nonce":       sig.nonce,
            "payload":     payload_b64,
        });
        Ok::<_, Infallible>(Event::default()
            .event(sig.kind.as_ref())
            .data(data.to_string()))
    });

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// `POST /gateway/signal/emit`
///
/// Fires a signal into the mesh. `scope` is `"cluster"` (every node; default), `"group:NAME"`,
/// or `"node:IP:PORT"`. `"system"` is still accepted as a deprecated alias for `"cluster"`.
/// `payload_b64` is the base64-encoded signal payload.
pub(super) async fn gw_signal_emit(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<axum::Extension<crate::agent::ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
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

    let scope_str = body["scope"].as_str().unwrap_or("cluster");
    // "cluster" is the name; "system" stays accepted as a deprecated alias (2026-07-10 rename).
    let scope = if scope_str == "cluster" || scope_str == "system" {
        SignalScope::Cluster
    } else if let Some(rest) = scope_str.strip_prefix("group:") {
        SignalScope::Group(Arc::from(rest))
    } else if let Some(rest) = scope_str.strip_prefix("node:") {
        match rest.parse::<crate::node_id::NodeId>() {
            Ok(nid) => SignalScope::Individual(nid),
            Err(_)  => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid node id"}))).into_response(),
        }
    } else {
        // Reject an unrecognized scope instead of silently widening it to a cluster-wide broadcast:
        // a typo'd prefix (`grp:`, `individual:`) would otherwise emit to the WHOLE cluster rather
        // than the intended narrow scope (audit 2026-07-15 pass 2).
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "unknown scope; expected \"cluster\" | \"group:<name>\" | \"node:<id>\""
        }))).into_response();
    };

    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // A raw emission carries the client's bytes verbatim with *this node* as the sender, so a
    // client-supplied caller-context frame would verify as this gateway's own envelope. The frame is
    // constructed by the auth layer and never by a request body: refuse rather than emit.
    // Found by the Phase-C adversarial audit (items 1+2+7).
    if super::gateway_caller::carries_caller_frame(&payload) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "payload carries a caller-context frame; that context is constructed by the gateway, not supplied"
        }))).into_response();
    }

    // Adopter-surface Y11: decided only when an exposure rule selects `signal.emit` on this kind; untouched otherwise.
    let caller = caller.map(|axum::Extension(c)| c);
    let door = crate::agent::door::decide(
        &ctx.agent_ctx, crate::agent::door::DoorActor::Caller(caller.as_ref()), crate::agent::door::OP_SIGNAL_EMIT,
        &crate::agent::door::signal_resource(&kind), &body, "gateway:signal/emit",
    ).await;
    let pass = match door {
        Ok(p) => p,
        Err(r) => return crate::agent::door::refusal_response(&r),
    };
    // Same code path as GossipAgent::emit — local delivery + gossip fan-out
    let ok = super::helpers::emit_signal(&ctx.agent_ctx, kind, scope, payload);
    // `false` is a full gossip queue with local delivery done: not nothing, and not known to have reached the fleet.
    pass.finish(if ok { crate::agent::door::DoorOutcome::Completed } else { crate::agent::door::DoorOutcome::Unknown }).await;
    Json(json!({ "ok": ok })).into_response()
}

/// `GET /gateway/signal/sse/{kind}` — SSE stream of admitted signals for a kind.
///
/// Each event has `event: <kind>` and `data: {"kind":"…","sender":"…","payload_b64":"…","nonce":…}`;
/// `kind` repeats the event name (since 2.24.0) so a client reading the body alone is not wrong.
pub(super) async fn gw_signal_sse(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Response {
    // Closure plan C1, the SSE half: a protected kind can be observed no more than it can be sent
    // on a raw route (see `signal_sse_handler`).
    if let Some(refused) = refuse_unobservable_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    // A tap: an observer, never counted in the kind's fill, so a stalled or fast SSE client neither
    // vetoes nor hides the kind's workers (#602's review, finding 4).
    let rx = ctx.agent_ctx.signal_handlers.register_tap(
        Arc::from(kind.as_str()),
        256,
    );

    let stream = ReceiverStream::new(rx).map(|sig: crate::signal::Signal| {
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&sig.payload);
        let data = json!({
            "kind":        sig.kind.as_ref(),
            "sender":      sig.sender.to_string(),
            "payload_b64": payload_b64,
            "nonce":       sig.nonce,
        });
        Ok::<_, Infallible>(Event::default()
            .event(sig.kind.as_ref())
            .data(data.to_string()))
    });

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
