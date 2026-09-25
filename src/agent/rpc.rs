//! Formalised point-to-point RPC primitive built on top of the signal mesh.
//!
//! `rpc_call` / `rpc_respond` codify the `signal_once + nonce` pattern that was
//! previously implicit in `INVOKE` / `INVOKE_RESULT` usage. Callers and responders
//! work with typed `Bytes` — the 8-byte correlation nonce is prepended by
//! `rpc_call` and echoed back by `rpc_respond` without either side managing it
//! directly.
//!
//! All replies flow through the single `RPC_RESULT` signal kind; the nonce
//! distinguishes concurrent in-flight calls.

use crate::node_id::NodeId;
use crate::signal::{Signal, SignalScope, signal_kind};
use bytes::{BufMut, Bytes, BytesMut};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

use super::TaskCtx;
use super::emit_signal;

// ── RpcRequest newtype ────────────────────────────────────────────────────────

/// A received RPC request signal with the 8-byte correlation nonce hidden.
///
/// Obtained from [`ServiceHandle::rpc_rx`] or by wrapping a [`Signal`] with
/// `RpcRequest::from`. The nonce is used internally by [`ServiceHandle::rpc_respond`];
/// callers work only with `payload()` and `sender()`.
#[derive(Clone, Debug)]
pub struct RpcRequest(
    pub(crate) Signal,
    // Held for its `Drop`, never read; only set when the provider check is built in.
    #[allow(dead_code)] pub(crate) Held,
);

/// Something an admitted request keeps alive until the last copy of it is dropped: closure plan
/// C4's cohort-budget slot, so a call counts as in flight for exactly as long as its serve loop
/// holds it. Opaque to applications.
#[derive(Clone, Default)]
pub(crate) struct Held(#[allow(dead_code)] Option<Arc<dyn std::any::Any + Send + Sync>>);

impl Held {
    /// Hold `value` for the request's lifetime.
    #[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(dead_code))]
    pub(crate) fn new(value: impl std::any::Any + Send + Sync) -> Self {
        Self(Some(Arc::new(value)))
    }
}

/// Closure plan C10: cooperative cancellation for work served through `rpc_rx`.
#[cfg(all(feature = "gateway", feature = "tls"))]
impl RpcRequest {
    /// **Resolves when the authority this request runs under lapses** (Boundary H closure plan C10):
    /// its mandate expired, was revoked, went stale or was superseded, as the node's authority sweep
    /// found. Never resolves for a request that acts under no established mandate, or on a node
    /// without provider enforcement. A serve loop that does long work should race it:
    ///
    /// ```ignore
    /// tokio::select! {
    ///     out = do_the_work(&req) => agent.service().rpc_respond(&req, out),
    ///     _ = req.authority_lapsed() => agent.service().rpc_respond(&req, b"stopped: authority lapsed".to_vec()),
    /// }
    /// ```
    ///
    /// Awaiting it records that the work **acknowledged** the cancellation; dropping the request
    /// records the stop as **confirmed**.
    pub async fn authority_lapsed(&self) {
        let work = self
            .1
            .0
            .as_ref()
            .and_then(|a| a.downcast_ref::<super::provider_enforcement::Admission>())
            .and_then(|a| a.work());
        match work {
            Some(w) => w.cancelled().await,
            None => std::future::pending().await,
        }
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Held(slot)" } else { "Held(none)" })
    }
}

impl RpcRequest {
    /// Application payload with the 8-byte nonce prefix stripped — and, when a gateway
    /// dispatched this call for a client, the caller-context envelope stripped too (item 7:
    /// [`gateway_caller`](super::gateway_caller)). A provider loop always sees exactly the
    /// application bytes; the context is read via
    /// [`GossipAgent::request_principal`](crate::GossipAgent::request_principal).
    pub fn payload(&self) -> Bytes {
        match self.frame() {
            super::gateway_caller::Frame::Unframed(app) => app,
            super::gateway_caller::Frame::Framed { app, .. } => app,
            // A recognised but malformed frame has no defined application payload: never hand a
            // handler the raw bytes as if they were one (review finding 4). Verification refuses
            // the request before any handler behind `rpc_rx` sees it.
            super::gateway_caller::Frame::Malformed(_) => Bytes::new(),
        }
    }
    /// The classified frame after the nonce (crate-private: verification reads it through
    /// [`gateway_caller::verify`](super::gateway_caller::verify)).
    pub(crate) fn frame(&self) -> super::gateway_caller::Frame {
        let after_nonce = self.0.payload.slice(8.min(self.0.payload.len())..);
        super::gateway_caller::split_frame(&after_nonce)
    }
    /// `true` when a caller-context frame rides on this request (well-formed or not — use
    /// [`GossipAgent::gateway_caller`](crate::GossipAgent::gateway_caller) to verify it).
    pub fn has_caller_context(&self) -> bool {
        !matches!(self.frame(), super::gateway_caller::Frame::Unframed(_))
    }
    /// NodeId of the node that sent the request.
    pub fn sender(&self)  -> &NodeId { &self.0.sender }
    /// Signal kind (e.g. `"mcp.invoke"`).
    pub fn kind(&self)    -> &Arc<str> { &self.0.kind }
    /// RPC correlation nonce (the 8 bytes prepended by `rpc_call`). Useful as
    /// a per-invocation trace correlator in audit records.
    pub fn nonce(&self) -> u64 {
        let b: [u8; 8] = self.0.payload.slice(..8).as_ref().try_into()
            .unwrap_or([0u8; 8]);
        u64::from_le_bytes(b)
    }
}

impl From<Signal> for RpcRequest {
    fn from(s: Signal) -> Self { RpcRequest(s, Held::default()) }
}

/// A signal receiver that yields [`RpcRequest`] values.
///
/// Returned by [`ServiceHandle::rpc_rx`]. Wraps `mpsc::Receiver<Signal>` and **verifies the
/// caller context at the receive boundary** (item 7, review finding 3): a request whose context
/// is forged, unsigned on an authenticated mesh, malformed, mis-attributed, or missing from a
/// node that promises one is answered with an error reply and never yielded — so every serve
/// loop, in this crate or a companion, gets the refusal without calling anything. Authorising
/// the *verified* principal against an allowlist stays the loop's job
/// (`request_authorized`).
pub struct RpcRequestRx {
    pub(crate) rx:  mpsc::Receiver<Signal>,
    pub(crate) ctx: Arc<TaskCtx>,
}

impl RpcRequestRx {
    /// Receives the next **verified** RPC request. Returns `None` when the agent shuts down.
    pub async fn recv(&mut self) -> Option<RpcRequest> {
        loop {
            #[allow(unused_mut)] // mutated only when the provider check can attach a budget slot
            let mut req = RpcRequest::from(self.rx.recv().await?);
            match super::gateway_caller::verify(&self.ctx, &req) {
                Ok(_) => {
                    // Closure plan C3: protected work is decided at this boundary, so every serve
                    // loop built on `rpc_rx`, in this crate or a companion, gets it without calling
                    // anything. Inert unless provider enforcement is on.
                    #[cfg(all(feature = "gateway", feature = "tls"))]
                    match super::provider_enforcement::check(&self.ctx, &req).await {
                        // C4: the admission (a cohort-budget slot, if a budget is attached) lives as
                        // long as the request does, so the call counts as in flight until the serve
                        // loop drops it.
                        Ok(admission) => req.1 = Held::new(admission),
                        Err(refusal) => {
                            tracing::warn!(kind = %req.kind(), sender = %req.sender(), reason = %refusal.reason,
                                "rpc_rx: refused by provider enforcement");
                            rpc_respond_ctx(&self.ctx, &req, Bytes::from(refusal.rpc_body()));
                            continue;
                        }
                    }
                    return Some(req);
                }
                Err(e) => {
                    tracing::warn!(kind = %req.kind(), sender = %req.sender(),
                        "rpc_rx: caller context refused, answering with an error: {e}");
                    let err = format!("{{\"error\":\"caller context refused: {e}\"}}");
                    rpc_respond_ctx(&self.ctx, &req, Bytes::from(err.into_bytes()));
                }
            }
        }
    }
}

/// Error returned by [`ServiceHandle::rpc_call`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    /// No reply arrived before the timeout elapsed.
    Timeout,
    /// The caller envelope, with the mandate it carries, would exceed the size a provider accepts;
    /// nothing was sent ([`ServiceHandle::rpc_call_with_mandate`](super::service_handle::ServiceHandle::rpc_call_with_mandate)).
    ContextTooLarge,
}

/// Registers a one-shot receiver in `ctx.rpc_pending` and awaits the first
/// reply signal whose correlation nonce (first 8 bytes of payload, LE) matches
/// `nonce` and whose sender matches `target`.
///
/// Registration happens synchronously in the first poll — before any yield
/// point — so it is safe to call `emit_signal` immediately before this
/// without missing a co-located reply.
///
/// Returns `Some(payload)` with the 8-byte nonce prefix stripped, or `None`
/// on timeout (including sender mismatch, which is astronomically rare with
/// 64-bit nonces).
pub(crate) async fn await_nonce_reply(
    ctx:      &TaskCtx,
    nonce:    u64,
    target:   &NodeId,
    deadline: tokio::time::Instant,
) -> Option<Bytes> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    ctx.rpc_pending.lock().unwrap_or_else(|e| e.into_inner()).insert(nonce, tx);
    let result = match tokio::time::timeout_at(deadline, rx).await {
        Ok(Ok(sig)) if sig.sender == *target => Some(sig.payload.slice(8..)),
        _ => None,
    };
    ctx.rpc_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&nonce);
    result
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Timeout => f.write_str("rpc call timed out — no reply from target"),
            RpcError::ContextTooLarge => f.write_str("rpc call not sent — the caller envelope with its mandate is over the size limit"),
        }
    }
}

impl std::error::Error for RpcError {}

/// `rpc_respond` logic operating on [`TaskCtx`] directly.
///
/// Used by callers that hold an `Arc<TaskCtx>` rather than a full `GossipAgent`
/// (e.g. MCP task functions). [`ServiceHandle::rpc_respond`] delegates here.
pub(crate) fn rpc_respond_ctx(ctx: &TaskCtx, request: &RpcRequest, result: impl Into<Bytes>) {
    // `Bytes::slice` **asserts** `end <= len`, so an under-length payload panicked here — and the
    // panic was remotely reachable through item 7's own refusal path: a 0–7 byte signal on a kind a
    // provider serves fails `verify` with `Missing`, and the refusal branch in `RpcRequestRx::recv`
    // answers by calling this, killing the provider's serve task for good. A correlation nonce we
    // never received cannot be echoed, so there is nothing to answer to: drop it rather than die.
    // Found by the Phase-C adversarial audit (items 1+2+7).
    if request.0.payload.len() < 8 {
        tracing::warn!(
            kind = %request.kind(), sender = %request.sender(), len = request.0.payload.len(),
            "rpc_respond: payload is shorter than the 8-byte correlation nonce; dropping the reply"
        );
        return;
    }
    let nonce_bytes = request.0.payload.slice(..8);
    let result_bytes: Bytes = result.into();
    let mut buf = BytesMut::with_capacity(8 + result_bytes.len());
    buf.put_slice(&nonce_bytes);
    buf.put(result_bytes);
    emit_signal(
        ctx,
        Arc::from(signal_kind::RPC_RESULT),
        SignalScope::Individual(request.0.sender.clone()),
        buf.freeze(),
    );
}

/// Core `rpc_call` logic operating on [`TaskCtx`] directly — **the node's own action**.
///
/// On a node that promises envelopes (a gateway in the secure profile,
/// [`gateway_caller::promises_envelopes`](super::gateway_caller::promises_envelopes)) the payload
/// is wrapped in the node's signed *self* envelope first, so a provider can tell this call from a
/// raw gateway emission shaped like one (review finding 1). [`ServiceHandle::rpc_call`] delegates
/// here; gateway dispatch on a client's behalf uses [`rpc_call_framed`] with the client's envelope.
pub(crate) async fn rpc_call_ctx(
    ctx:     &TaskCtx,
    target:  NodeId,
    kind:    Arc<str>,
    payload: Bytes,
    timeout: Duration,
) -> Result<Bytes, RpcError> {
    let payload = if super::gateway_caller::promises_envelopes(&ctx.config) {
        super::gateway_caller::frame_self(ctx, payload)
    } else {
        payload
    };
    rpc_call_framed(ctx, target, kind, payload, timeout).await
}

/// `rpc_call` over an already-framed payload (a client envelope from the gateway, or a self
/// envelope from [`rpc_call_ctx`]); prepends only the correlation nonce.
pub(crate) async fn rpc_call_framed(
    ctx:     &TaskCtx,
    target:  NodeId,
    kind:    Arc<str>,
    payload: Bytes,
    timeout: Duration,
) -> Result<Bytes, RpcError> {
    let nonce: u64 = fastrand::u64(1..);

    let mut buf = BytesMut::with_capacity(8 + payload.len());
    buf.put_u64_le(nonce);
    buf.put(payload);

    emit_signal(ctx, kind, SignalScope::Individual(target.clone()), buf.freeze());

    let deadline = tokio::time::Instant::now() + timeout;
    #[cfg(feature = "metrics")]
    let rpc_start = std::time::Instant::now();
    let result = match await_nonce_reply(ctx, nonce, &target, deadline).await {
        Some(b) => Ok(b),
        None    => Err(RpcError::Timeout),
    };
    #[cfg(feature = "metrics")]
    metrics::histogram!("gossip_rpc_latency_ms").record(rpc_start.elapsed().as_secs_f64() * 1000.0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use bytes::Bytes;
    use std::{sync::Arc, time::Duration};

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    fn agent_pair() -> (Arc<GossipAgent>, Arc<GossipAgent>) {
        let port_a = alloc_port();
        let port_b = alloc_port();
        let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
        let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

        let mut cfg_a = GossipConfig::default();
        cfg_a.bind_port = port_a;
        cfg_a.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_b).unwrap()];

        let mut cfg_b = GossipConfig::default();
        cfg_b.bind_port = port_b;
        cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];

        (
            Arc::new(GossipAgent::new(id_a, cfg_a)),
            Arc::new(GossipAgent::new(id_b, cfg_b)),
        )
    }

    #[tokio::test]
    async fn test_rpc_round_trip() {
        let (agent_a, agent_b) = agent_pair();
        agent_a.start().await.unwrap();
        agent_b.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let node_b = agent_b.node_id().clone();

        let responder = Arc::clone(&agent_b);
        tokio::spawn(async move {
            let mut rx = responder.service().rpc_rx("echo");
            if let Some(req) = rx.recv().await {
                responder.service().rpc_respond(&req, req.payload());
            }
        });

        let result = agent_a.service().rpc_call(
            node_b,
            "echo",
            Bytes::from_static(b"hello"),
            Duration::from_secs(2),
        ).await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(result.unwrap(), Bytes::from_static(b"hello"));

        agent_a.shutdown().await;
        agent_b.shutdown().await;
    }

    #[tokio::test]
    async fn test_rpc_timeout() {
        let port = alloc_port();
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();

        let ghost = NodeId::new("127.0.0.1", 19999).unwrap();
        let result = agent.service().rpc_call(
            ghost,
            "noop",
            Bytes::from_static(b"ping"),
            Duration::from_millis(150),
        ).await;

        assert_eq!(result, Err(RpcError::Timeout));
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_rpc_nonce_isolation() {
        let (agent_a, agent_b) = agent_pair();
        agent_a.start().await.unwrap();
        agent_b.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let node_b = agent_b.node_id().clone();

        let responder = Arc::clone(&agent_b);
        tokio::spawn(async move {
            let mut rx = responder.service().rpc_rx("tagged");
            while let Some(req) = rx.recv().await {
                responder.service().rpc_respond(&req, req.payload());
            }
        });

        let b1 = node_b.clone();
        let b2 = node_b.clone();
        let a1 = Arc::clone(&agent_a);
        let a2 = Arc::clone(&agent_a);
        let (r1, r2) = tokio::join!(
            async move { a1.service().rpc_call(b1, "tagged", Bytes::from_static(b"call-one"), Duration::from_secs(2)).await },
            async move { a2.service().rpc_call(b2, "tagged", Bytes::from_static(b"call-two"), Duration::from_secs(2)).await },
        );

        assert_eq!(r1.unwrap(), Bytes::from_static(b"call-one"));
        assert_eq!(r2.unwrap(), Bytes::from_static(b"call-two"));

        agent_a.shutdown().await;
        agent_b.shutdown().await;
    }
}
