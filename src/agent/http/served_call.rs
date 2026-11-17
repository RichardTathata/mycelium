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
//! One watcher task per stream a call that holds anything (a cohort place, a running-work registration) was handed to:
//! it owns that stream's [`Admission`] and ends it on the first of — any subscriber's answer before a cancel (the
//! call's one answer); after the node's own stop, **its own stream's** answer or confirmation (the maintainer's
//! decision of 2026-11-17: each stream speaks only for its own work, so a replica's `stopped` never ends a sibling's
//! registration); or [`PARKED_TTL_MS`] after the call was streamed (the window in which `rpc/respond` takes an answer
//! at all). Each stream is named, by 128 random bits ([`Words::watching`], the `stream` field the node puts on every event it streams and
//! the SDK sends back). Every end but its own stream's confirmation is a release the node cannot confirm.
//!
//! **Today an enveloped call reaches one stream only**: the envelope replay window (`gateway_caller::verify_received`,
//! node-wide) drops a second stream's copy of the same envelope, and every call that carries a registration is
//! enveloped. Calls without an envelope (an in-mesh member's direct call) reach every stream of the kind and, under a
//! cohort budget, hold one place for the call, not one per stream ([`Words::place`]; §116). The per-stream keying holds whatever that window becomes.
//!
//! The table the respond route finds an entry in is a papaya map keyed `(sender, nonce)`, so it adds no lock; the
//! entry goes with its last watcher (`remove_if` on the pointer), and it is bounded at [`SERVED_CALLS_CAP`]: past it the
//! serve loop refuses a call that would need a watcher, before streaming it. A watcher keeps only the request's
//! sender, nonce and kind ([`slim`]), never its payload.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::provider_enforcement::{answered_late, Admission, PARKED_TTL_MS};
use super::*;

/// The most streamed calls a gateway holds a watcher for at once — `SERVED_RPC_CAP`, the served-request table's own.
pub(super) const SERVED_CALLS_CAP: usize = super::mesh::SERVED_RPC_CAP;

/// What the subscribers have said of a streamed call, by stream (the maintainer's decision of 2026-11-17: each serve
/// stream speaks only for its own work).
#[derive(Default)]
pub(super) struct Words {
    /// Some stream answered the call (any answer, attributed or not): before a cancel, the call's one answer.
    any: bool,
    /// The streams that answered it with a result.
    answered: std::collections::BTreeSet<u128>,
    /// The streams that confirmed they stopped it (`"stopped": true`).
    stopped: std::collections::BTreeSet<u128>,
    /// The principals that sent an answer naming no stream (a client before streams were named). Such an answer can
    /// be no stream's confirmation, and it cannot say which of its principal's streams it came from: it ends every
    /// stream **that principal** opened, and no other principal's (#806's confirm round, C4).
    unattributed: std::collections::BTreeSet<Arc<str>>,
    /// The streams the call was handed to: the principal that opened each, and whether it declared `stops_on_cancel`.
    watching: std::collections::BTreeMap<u128, (Arc<str>, bool)>,
    /// **The call's one cohort place** (the maintainer's decision of 2026-11-17, M4): a caller's budget is the
    /// caller's, so a call holds one place however many streams the server opened. The first stream's is kept here;
    /// a later stream's is given back as it is handed the call. Released, explicitly, when the call's last stream
    /// registration ends ([`leave`]) — not left to the entry's drop, which the table's reclamation may defer.
    place: Option<crate::knowledge::cohort_budget::CohortSlot>,
}

impl Words {
    /// Has `stream` itself ended the call after the node's stop — its confirmation, its answer — or did its own
    /// principal send an answer that named no stream?
    pub(super) fn ends(&self, stream: u128) -> bool {
        self.stopped.contains(&stream)
            || self.answered.contains(&stream)
            || self.watching.get(&stream).is_some_and(|(p, _)| self.unattributed.contains(p))
    }
}

/// Why a `"stopped": true` was refused, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotConfirmed {
    /// The node has not cancelled this call.
    NotCancelled,
    /// The confirmation names no stream.
    StreamRequired,
    /// The stream it names was not handed this call, or another principal opened it, or the call was not streamed to
    /// this principal at all.
    NotYourStream,
    /// The stream did not declare `stops_on_cancel`: it made no promise to stop, so there is no stop of its to confirm.
    NotDeclared,
}

impl NotConfirmed {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NotCancelled => "not_cancelled",
            Self::StreamRequired => "stream_required",
            Self::NotYourStream => "not_your_stream",
            Self::NotDeclared => "not_declared",
        }
    }
}

/// A streamed call's watchers, as the respond route finds them: one entry per `(sender, nonce)`, one watcher per stream
/// it was handed to.
pub(super) struct ServedCall {
    /// What each stream said ([`Words`]).
    ended: watch::Sender<Words>,
    /// When the authority asked the call's work to stop, once the node stopped waiting for it; 0 while it has not.
    stop_asked_ms: AtomicU64,
    /// A subscriber the call was streamed to declared that it stops on a `cancel` event.
    promised: AtomicBool,
    kind: Arc<str>,
}

impl ServedCall {
    fn new(kind: Arc<str>) -> Self {
        Self { ended: watch::channel(Words::default()).0, stop_asked_ms: AtomicU64::new(0), promised: AtomicBool::new(false), kind }
    }

    /// Has the node stopped waiting for this call (its authority lapsed)?
    pub(super) fn cancelled(&self) -> bool {
        self.stop_asked_ms.load(Ordering::Acquire) != 0
    }

    /// **`principal` answered the call with a result**, naming its stream if it did (#806's confirm round, C4). The
    /// call is answered — before a cancel, that is its one answer, whichever stream sent it. After a cancel an answer
    /// ends only its own stream's registration: a named stream counts only if `principal` opened it and it was handed
    /// the call (another's, or an unknown one, ends nothing); an answer naming none — an older SDK — ends the streams
    /// `principal` opened, since it cannot say which, and no other principal's.
    pub(super) fn answered(&self, stream: Option<u128>, principal: &str) {
        self.ended.send_modify(|w| {
            w.any = true;
            match stream {
                Some(s) if w.watching.get(&s).is_some_and(|(p, _)| p.as_ref() == principal) => {
                    w.answered.insert(s);
                }
                Some(_) => {}
                None => {
                    w.unattributed.insert(Arc::from(principal));
                }
            }
        });
    }

    /// **A subscriber's confirmation that it stopped its own work** under `stream`. Refused by name unless the node
    /// cancelled the call, the confirmation names a stream, that stream was handed the call and opened by `principal`,
    /// and it declared that it stops: a stream speaks only for its own work, never a sibling's.
    pub(super) fn confirm(&self, stream: Option<u128>, principal: &str) -> Result<(), NotConfirmed> {
        if !self.cancelled() {
            return Err(NotConfirmed::NotCancelled);
        }
        let Some(stream) = stream else { return Err(NotConfirmed::StreamRequired) };
        match self.ended.borrow().watching.get(&stream) {
            Some((p, _)) if p.as_ref() != principal => return Err(NotConfirmed::NotYourStream),
            None => return Err(NotConfirmed::NotYourStream),
            Some((_, false)) => return Err(NotConfirmed::NotDeclared),
            Some((_, true)) => {}
        }
        self.ended.send_modify(|w| {
            w.stopped.insert(stream);
        });
        Ok(())
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

/// The watchers of the streamed call `(sender, nonce)`, if it holds anything.
pub(super) fn find(http: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64) -> Option<Arc<ServedCall>> {
    http.served_calls.pin().get(&(sender.clone(), nonce)).cloned()
}

/// Is the watcher table at its cap? Read before streaming a call that would need a watcher.
pub(super) fn full(http: &HttpCtx) -> bool {
    http.served_calls.len() >= SERVED_CALLS_CAP
}

/// The `cancel` event the stream carries for a call the node stopped waiting for.
pub(super) fn cancel_event(req: &rpc::RpcRequest, stream: u128) -> Event {
    Event::default().event("cancel").data(
        json!({
            "cancel": true,
            "nonce_hex": format!("{:016x}", req.nonce()),
            "sender": req.sender().to_string(),
            "stream": super::mesh::stream_name(stream),
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

/// **What a watcher keeps of a request** (#806's review, L5): its sender, kind and nonce — what an answer to its
/// requester needs — and not its payload, which a watcher would otherwise hold for up to [`PARKED_TTL_MS`].
pub(super) fn slim(req: &rpc::RpcRequest) -> rpc::RpcRequest {
    let mut signal = req.0.clone();
    let keep = 8.min(signal.payload.len());
    // A copy, not a slice: a slice would keep the whole payload's buffer alive.
    signal.payload = Bytes::copy_from_slice(&signal.payload[..keep]);
    rpc::RpcRequest::from(signal)
}

/// A watcher placed for `stream` before its event is sent (#806's review, L6), so an answer that arrives at once
/// finds it. [`Placed::start`] hands it the admission once the event is out; [`Placed::withdraw`] takes it back if
/// the event could not be sent.
pub(super) struct Placed {
    entry: Arc<ServedCall>,
    key: (crate::node_id::NodeId, u64),
    stream: u128,
}

/// **Place a watcher for a streamed call on `stream`** — before the event is sent. A replicated serve loop shares one
/// entry per call; each stream's watcher listens only for its own stream's word.
pub(super) fn place(http: &HttpCtx, req: &rpc::RpcRequest, principal: &Arc<str>, stream: u128, declared: bool) -> Placed {
    let key = (req.sender().clone(), req.nonce());
    let entry = Arc::clone(http.served_calls.pin().get_or_insert(key.clone(), Arc::new(ServedCall::new(Arc::clone(req.kind())))));
    entry.ended.send_modify(|w| {
        w.watching.insert(stream, (Arc::clone(principal), declared));
    });
    if declared {
        entry.promised.store(true, Ordering::Release);
    }
    Placed { entry, key, stream }
}

impl Placed {
    /// The event could not be sent: the stream was not handed the call.
    pub(super) fn withdraw(self, http: &HttpCtx) {
        leave(http, &self.entry, &self.key, self.stream);
    }

    /// **Hold the streamed call's admission until its stream ends it** — the watcher (module doc). `cancels`: the
    /// stream the call went out on, if its subscriber reads `cancel` events.
    pub(super) fn start(self, http: &Arc<HttpCtx>, req: &rpc::RpcRequest, principal: Arc<str>, mut admission: Admission, cancels: Option<tokio::sync::mpsc::UnboundedSender<Event>>) {
        // One place per call (M4): the call keeps the first stream's; this stream's, if the call has one, is released.
        if let Some(place) = admission.take_place() {
            self.entry.ended.send_modify(|w| {
                if w.place.is_none() {
                    w.place = Some(place);
                }
            });
        }
        let req = slim(req);
        let http = Arc::clone(http);
        let spawner = Arc::clone(&http.agent_ctx);
        spawner.spawn_task(async move {
            run(&http, &req, &principal, self.stream, admission, cancels, &self.entry).await;
            leave(&http, &self.entry, &self.key, self.stream);
        });
    }
}

/// A stream's watcher is done: it leaves the entry, and the entry goes with the last of them.
fn leave(http: &HttpCtx, entry: &Arc<ServedCall>, key: &(crate::node_id::NodeId, u64), stream: u128) {
    let mut place = None;
    entry.ended.send_modify(|w| {
        w.watching.remove(&stream);
        // The call's last stream registration has ended: its one place goes (M4).
        if w.watching.is_empty() {
            place = w.place.take();
        }
    });
    drop(place);
    if entry.ended.borrow().watching.is_empty() {
        http.served_calls.pin().remove_if(key, |_, v| Arc::ptr_eq(v, entry)).ok();
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(http: &Arc<HttpCtx>, req: &rpc::RpcRequest, principal: &str, stream: u128, admission: Admission, cancels: Option<tokio::sync::mpsc::UnboundedSender<Event>>, entry: &ServedCall) {
    let mut ended = entry.ended.subscribe();
    let ttl = mycelium_core::sim_seam::sleep_ms("rpc-serve/parked", PARKED_TTL_MS);
    tokio::pin!(ttl);
    enum First {
        Answered,
        Expired,
        Stop,
    }
    // An answer, the TTL, or the node's own stop — whichever comes first.
    let first = tokio::select! { biased;
        _ = ended.wait_for(|w| w.any) => First::Answered,
        _ = &mut ttl => First::Expired,
        _ = admission.stop_asked() => First::Stop,
    };
    match first {
        // An answer before any stop is the call's answer, whichever stream sent it: nothing to record — and released
        // unconfirmed, so a sweep landing between here and the drop cannot record a stop no one confirmed (#806's
        // review, L1). One that came after the authority lapsed is not this stream's word: it goes on to wait for its
        // own (below).
        First::Answered if !admission.is_lapsed() => return admission.release_unconfirmed(),
        First::Expired => return admission.release_unconfirmed(),
        First::Answered | First::Stop => {}
    }
    // The node's own stop. It answers the requester — unless a subscriber's answer got there first — and tells this
    // stream, if it reads `cancel` events.
    let asked = admission.stop_requested_ms().unwrap_or_else(|| http.agent_ctx.hlc.decision_now_ms()).max(1);
    let _ = entry.stop_asked_ms.compare_exchange(0, asked, Ordering::AcqRel, Ordering::Acquire);
    if super::mesh::answer_served_rpc(http, req.sender(), req.nonce(), principal) == super::mesh::ServedAnswer::First {
        rpc::rpc_respond_ctx(&http.agent_ctx, req, cancelled_body("sdk_serve"));
        super::provider_enforcement::count_door_stop(&http.agent_ctx, "sdk_serve");
    }
    // Every stream the call went out on is told on its own stream (#806's review, M3) — unless it had answered already.
    if !ended.borrow().answered.contains(&stream)
        && let Some(tx) = &cancels
    {
        // One event per call the stream was handed, so the channel holds at most as many as `SERVED_CALLS_CAP`; a
        // stream that has ended is not told, and the node's own stop stands.
        let _ = tx.send(cancel_event(req, stream));
    }
    // Then this stream's own end of it, and a confirmed stop only on its own word.
    tokio::select! { biased;
        _ = ended.wait_for(|w| w.ends(stream)) => {}
        _ = &mut ttl => {}
    }
    let confirmed = ended.borrow().stopped.contains(&stream);
    if confirmed {
        drop(admission);
    } else {
        admission.release_unconfirmed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancelled_call(streams: &[(u128, &str, bool)]) -> ServedCall {
        let c = ServedCall::new(Arc::from("depot.custom"));
        c.ended.send_modify(|w| {
            for (s, p, d) in streams {
                w.watching.insert(*s, (Arc::from(*p), *d));
            }
        });
        c.stop_asked_ms.store(1, Ordering::Release);
        c
    }

    /// **Each stream confirms only its own stop** (the maintainer's decision of 2026-11-17). Two declaring streams of
    /// one principal watch one call: stream 1's confirmation ends stream 1 and leaves stream 2 waiting; a confirmation
    /// naming no stream, a stream not handed the call, or another principal's stream is refused by name, and so is an
    /// undeclared stream's (#806's review, L2). Seen failing first with `Words::ends` keyed by call
    /// (`!self.stopped.is_empty()`): stream 1's word ended stream 2.
    #[test]
    fn a_streams_confirmation_ends_only_its_own_registration() {
        let c = cancelled_call(&[(1, "token:sdk", true), (2, "token:sdk", true), (3, "token:other", true), (4, "token:sdk", false)]);
        assert_eq!(c.confirm(None, "token:sdk"), Err(NotConfirmed::StreamRequired));
        assert_eq!(c.confirm(Some(9), "token:sdk"), Err(NotConfirmed::NotYourStream));
        assert_eq!(c.confirm(Some(3), "token:sdk"), Err(NotConfirmed::NotYourStream), "another principal's stream");
        assert_eq!(c.confirm(Some(4), "token:sdk"), Err(NotConfirmed::NotDeclared));
        assert!(!c.ended.borrow().ends(1) && !c.ended.borrow().ends(2), "nothing moved");
        assert_eq!(c.confirm(Some(1), "token:sdk"), Ok(()));
        assert!(c.ended.borrow().ends(1), "its own stream ends");
        assert!(!c.ended.borrow().ends(2), "the sibling's stands until it speaks for itself");
        c.answered(Some(2), "token:sdk");
        assert!(c.ended.borrow().ends(2) && !c.ended.borrow().stopped.contains(&2), "its answer ends it, unconfirmed");
        let uncancelled = ServedCall::new(Arc::from("depot.custom"));
        assert_eq!(uncancelled.confirm(Some(1), "token:sdk"), Err(NotConfirmed::NotCancelled));
    }

    /// **An answer ends only its own stream's registration** (#806's confirm round, C4). After a cancel, with streams 1
    /// and 2 of one principal and 3 of another watching: an answer naming another principal's stream, or one nobody
    /// holds, ends nothing; an answer naming stream 1 ends stream 1 only; an answer naming no stream (an older SDK)
    /// ends its own principal's streams and not the other principal's. Seen failing first with `answered` taking any
    /// named stream and an unnamed answer ending every stream.
    #[test]
    fn an_answer_ends_only_its_own_streams_registration() {
        let c = cancelled_call(&[(1, "token:sdk", true), (2, "token:sdk", true), (3, "token:other", true)]);
        c.answered(Some(3), "token:sdk");
        c.answered(Some(9), "token:sdk");
        assert!(!c.ended.borrow().ends(1) && !c.ended.borrow().ends(2) && !c.ended.borrow().ends(3), "another's stream, or none: nothing ends");
        c.answered(Some(1), "token:sdk");
        assert!(c.ended.borrow().ends(1) && !c.ended.borrow().ends(2) && !c.ended.borrow().ends(3));
        c.answered(None, "token:sdk");
        assert!(c.ended.borrow().ends(2), "an unnamed answer ends its principal's streams");
        assert!(!c.ended.borrow().ends(3), "and no other principal's");
    }

    /// **A watcher keeps no payload** (#806's review, L5): its slim request answers to the same sender and nonce, with
    /// the 8-byte nonce alone. Seen failing first with `slim` returning a clone.
    #[test]
    fn a_watcher_keeps_the_nonce_and_not_the_payload() {
        let mut payload = 0x1122_3344_5566_7788u64.to_le_bytes().to_vec();
        payload.extend(std::iter::repeat_n(7u8, 4096));
        let req = rpc::RpcRequest::from(crate::signal::Signal {
            kind: Arc::from("depot.custom"),
            scope: crate::signal::SignalScope::Cluster,
            payload: Bytes::from(payload),
            sender: crate::node_id::NodeId::new("127.0.0.1", 7001).unwrap(),
            nonce: 5,
        });
        let kept = slim(&req);
        assert_eq!((kept.nonce(), kept.sender(), kept.kind()), (req.nonce(), req.sender(), req.kind()));
        assert_eq!(kept.0.payload.len(), 8);
    }
}
