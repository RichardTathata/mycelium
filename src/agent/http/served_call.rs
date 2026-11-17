//! **A call streamed to an SDK subscriber, held until the subscriber ends it** (identity-and-lifecycle I4, PR 3b; the
//! maintainer's decision (C) of 2026-11-16).
//!
//! # Who promises what
//!
//! The work of a call `rpc/serve` streamed runs in the SDK process — another agent. The node promises only its own
//! behaviour, and each agent speaks for itself:
//!
//! - **The node's own stop.** When the call's authority lapses — the caller's mandate, or the gateway cancelled the
//!   call (its `call:` term revoked) — the node stops waiting: it answers the requester `cancelled`, tells the
//!   subscriber with a `cancel` event if the subscriber said it reads them (`cancel_events`), and discards the
//!   subscriber's answer if one comes later (`rpc/respond` answers `{"ok": true, "cancelled": true}` and sends
//!   nothing). It promises nothing about the work in the SDK process.
//! - **The subscriber's promise.** A subscriber that opened its stream with `stops_on_cancel` declared that it stops a
//!   call on its `cancel` event — as an `rpc_rx` receiver declares with `stops_when_authority_lapses`. Only such a
//!   subscriber is served a call under a cancellable rule (`provider_enforcement::Door::SdkServe`).
//! - **The subscriber's confirmation.** The node records a confirmed stop — the one a cancel receipt reads as
//!   `Stopped` — **only on the subscriber's own word**: `rpc/respond` with `"stopped": true` for the cancelled call.
//!   Every other end of a cancelled call is recorded as a release the node cannot confirm: the subscriber answered
//!   with a result (it may have finished rather than stopped), it said nothing before [`PARKED_TTL_MS`], or the
//!   subscriber never declared. The assessment, not enforcement: a declared subscriber's answer or confirmation more
//!   than [`LAPSE_ANSWER_GRACE_MS`](super::provider_enforcement::LAPSE_ANSWER_GRACE_MS) after the stop was asked is
//!   counted (`GET /stats` `rpc_serve_answered_after_lapse`, by kind), and nothing is refused for it.
//!
//! # What ends a streamed call's registration
//!
//! One watcher task per streamed call that holds anything (a cohort place, a running-work registration): it owns the
//! call's [`Admission`] and ends it on the first of — the subscriber's answer (`rpc/respond`), the node's own stop and
//! then the subscriber's answer or confirmation, or [`PARKED_TTL_MS`] after the call was streamed (the window in
//! which `rpc/respond` takes an answer at all). The table the respond route finds a watcher in is a papaya map keyed
//! `(sender, nonce)`, so it adds no lock; each watcher removes only its own entry (`remove_if` on the pointer), and
//! it is bounded at [`SERVED_CALLS_CAP`]: past it the serve loop refuses a call that would need a watcher, before
//! streaming it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::provider_enforcement::{answered_late, Admission, PARKED_TTL_MS};
use super::*;

/// The most streamed calls a gateway holds a watcher for at once — `SERVED_RPC_CAP`, the served-request table's own.
pub(super) const SERVED_CALLS_CAP: usize = super::mesh::SERVED_RPC_CAP;

/// A streamed call's watcher, as the respond route finds it.
pub(super) struct ServedCall {
    /// How the subscriber ended the call: `Some(true)` it confirmed it stopped; `Some(false)` it answered.
    ended: watch::Sender<Option<bool>>,
    /// When the authority asked the call's work to stop, once the node stopped waiting for it; 0 while it has not.
    stop_asked_ms: AtomicU64,
    /// A subscriber the call was streamed to declared that it stops on a `cancel` event.
    promised: AtomicBool,
    kind: Arc<str>,
}

impl ServedCall {
    /// Has the node stopped waiting for this call (its authority lapsed)?
    pub(super) fn cancelled(&self) -> bool {
        self.stop_asked_ms.load(Ordering::Acquire) != 0
    }

    /// The subscriber ended the call: `stopped` it confirmed its stop; otherwise it answered. The first word holds.
    pub(super) fn end(&self, stopped: bool) {
        self.ended.send_if_modified(|e| {
            if e.is_some() {
                return false;
            }
            *e = Some(stopped);
            true
        });
    }

    /// Count this answer if it is a declared subscriber's, sent past the grace after the stop was asked.
    pub(super) fn assess(&self, ctx: &TaskCtx) {
        let asked = match self.stop_asked_ms.load(Ordering::Acquire) {
            0 => None,
            t => Some(t),
        };
        if !answered_late(ctx, self.promised.load(Ordering::Acquire), asked) {
            return;
        }
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_rpc_serve_answered_after_lapse_total", "kind" => self.kind.to_string()).increment(1);
        ctx.rpc_serve_late_answers.pin().update_or_insert(self.kind.to_string(), |n| n + 1, 1);
    }
}

/// The watcher of the streamed call `(sender, nonce)`, if it holds anything.
pub(super) fn find(http: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64) -> Option<Arc<ServedCall>> {
    http.served_calls.pin().get(&(sender.clone(), nonce)).cloned()
}

/// Is the watcher table at its cap? Read before streaming a call that would need a watcher.
pub(super) fn full(http: &HttpCtx) -> bool {
    http.served_calls.len() >= SERVED_CALLS_CAP
}

/// The `cancel` event the stream carries for a call the node stopped waiting for.
pub(super) fn cancel_event(req: &rpc::RpcRequest) -> Event {
    Event::default().event("cancel").data(
        json!({
            "cancel": true,
            "nonce_hex": format!("{:016x}", req.nonce()),
            "sender": req.sender().to_string(),
            "reason": "cancelled",
            "message": "the call's authority lapsed: the node stopped waiting for it and discards a later answer",
        })
        .to_string(),
    )
}

/// What the requester is answered when the node stops waiting for a streamed call.
pub(super) fn cancelled_body(door: &str) -> Bytes {
    Bytes::from(
        json!({
            "error": "cancelled: the call's authority lapsed while it ran; this node stopped waiting for it and discards a later answer",
            "reason": "cancelled",
            "data": { "reason": "cancelled", "door": door },
        })
        .to_string()
        .into_bytes(),
    )
}

/// **Hold a streamed call's admission until the subscriber ends it** — the watcher (module doc). `cancels`: the stream
/// the call went out on, if its subscriber reads `cancel` events; `declared`: it declared it stops on one.
pub(super) fn watch(http: &Arc<HttpCtx>, req: rpc::RpcRequest, principal: Arc<str>, admission: Admission, cancels: Option<tokio::sync::mpsc::UnboundedSender<Event>>, declared: bool) {
    let entry = Arc::new(ServedCall { ended: watch::channel(None).0, stop_asked_ms: AtomicU64::new(0), promised: AtomicBool::new(false), kind: Arc::clone(req.kind()) });
    // A replicated serve loop streams the same request on every replica's stream: they share one entry, so one answer
    // ends them all.
    let entry = Arc::clone(http.served_calls.pin().get_or_insert((req.sender().clone(), req.nonce()), entry));
    if declared {
        entry.promised.store(true, Ordering::Release);
    }
    let http = Arc::clone(http);
    let spawner = Arc::clone(&http.agent_ctx);
    spawner.spawn_task(async move {
        let key = (req.sender().clone(), req.nonce());
        run(&http, &req, &principal, admission, cancels, &entry).await;
        http.served_calls.pin().remove_if(&key, |_, v| Arc::ptr_eq(v, &entry)).ok();
    });
}

async fn run(http: &Arc<HttpCtx>, req: &rpc::RpcRequest, principal: &str, admission: Admission, cancels: Option<tokio::sync::mpsc::UnboundedSender<Event>>, entry: &ServedCall) {
    let mut ended = entry.ended.subscribe();
    let ttl = mycelium_core::sim_seam::sleep_ms("rpc-serve/parked", PARKED_TTL_MS);
    tokio::pin!(ttl);
    // An answer, the TTL, or the node's own stop — whichever comes first.
    let stop = tokio::select! { biased;
        _ = ended.wait_for(Option::is_some) => false,
        _ = &mut ttl => false,
        _ = admission.stop_asked() => true,
    };
    if !stop {
        let confirmed = *ended.borrow() == Some(true);
        // Answered before any stop: nothing to record. A stop that raced the answer is not the subscriber's word.
        if admission.is_lapsed() && !confirmed {
            admission.release_unconfirmed();
        }
        return;
    }
    // The node's own stop. It answers the requester — unless the subscriber's answer got there first — and tells the
    // subscriber, if it reads `cancel` events.
    let asked = admission.stop_requested_ms().unwrap_or_else(|| http.agent_ctx.hlc.decision_now_ms()).max(1);
    let _ = entry.stop_asked_ms.compare_exchange(0, asked, Ordering::AcqRel, Ordering::Acquire);
    if super::mesh::answer_served_rpc(http, req.sender(), req.nonce(), principal) == super::mesh::ServedAnswer::First {
        rpc::rpc_respond_ctx(&http.agent_ctx, req, cancelled_body("sdk_serve"));
        super::provider_enforcement::count_door_stop(&http.agent_ctx, "sdk_serve");
        if let Some(tx) = &cancels {
            // One event per call the stream was handed, so the channel holds at most as many as `SERVED_CALLS_CAP`;
            // a stream that has ended is not told, and the node's own stop stands.
            let _ = tx.send(cancel_event(req));
        }
    }
    // Then the subscriber's own end of it: a confirmed stop only on its word.
    tokio::select! { biased;
        _ = ended.wait_for(Option::is_some) => {}
        _ = &mut ttl => {}
    }
    let confirmed = *ended.borrow() == Some(true);
    if confirmed {
        drop(admission);
    } else {
        admission.release_unconfirmed();
    }
}
