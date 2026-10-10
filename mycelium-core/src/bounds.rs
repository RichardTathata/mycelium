//! Transport resource bounds (row B, post-360 hardening, 2026-10-10): the counters that make each
//! bound legible, and the per-peer anti-entropy reply slot.
//!
//! **Why a slot and not a cooldown.** A `StateRequest` is answered with a dump of every divergent
//! entry — up to the whole store — handed to the requester's writer channel. The per-connection
//! cooldown limits how often one *connection* is answered, and a reconnect resets it; a peer that
//! accepts and never reads parks its writer, so the dumps queued for it piled up, one whole-store
//! copy per reconnect. A [`ReplySlot`] is held by every frame of one reply (through
//! [`bytes::Bytes::from_owner`]) and released when the last of them has been written or dropped, so
//! a peer has **at most one** reply in flight — queued, being written, or waiting on the writer —
//! and a request that arrives while it is answered is skipped and counted. The writer's own write
//! progress bound (`peer_write_stall_timeout_ms`) is what guarantees a held
//! slot is released.

use crate::node_id::NodeId;
use bytes::Bytes;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Counters for the transport bounds, plus the per-peer anti-entropy reply slots. One per node,
/// on [`CoreCtx::transport_bounds`](crate::CoreCtx::transport_bounds).
#[derive(Default)]
pub struct TransportBounds {
    /// Inbound gossip connections this node closed because a bound elapsed: the TLS handshake or
    /// the first frame's first byte did not arrive within `handshake_timeout_ms`, or an established connection
    /// was silent for `inbound_idle_timeout_secs`. See `SystemStats::inbound_connections_timed_out`.
    pub inbound_timed_out: AtomicU64,
    /// Inbound gossip connections closed because a frame **in progress** stalled: no byte for
    /// `peer_read_stall_timeout_ms`, or below `peer_min_rate_bytes_per_sec` (#602's review, finding 1) —
    /// counted apart from `inbound_timed_out`, which is silence between frames.
    pub inbound_frames_stalled: AtomicU64,
    /// Inbound connections closed to give their permit to a newcomer at `max_connections`: the one
    /// whose last complete frame is oldest (#602's round 3, finding 1). See
    /// `SystemStats::inbound_connections_preempted`.
    pub inbound_preempted: AtomicU64,
    /// Anti-entropy chunks applied to the store whose WAL batch has not been enqueued yet (#602's
    /// round 4, finding 4). While non-zero, replica sync does not answer `Persisted`: a record the
    /// store holds may not have a WAL record behind it yet.
    pub ae_unbatched: AtomicU64,
    /// Outbound writer connections failed because the peer accepted no byte for
    /// `peer_write_stall_timeout_ms` (#602's re-review, finding 9). See `SystemStats::outbound_stalls`.
    pub outbound_stalls: Arc<AtomicU64>,
    /// `StateRequest`s this node did not answer because a reply to the same peer was still in
    /// flight. See `SystemStats::anti_entropy_replies_skipped`.
    pub anti_entropy_replies_skipped: AtomicU64,
    /// Peers with an anti-entropy reply in flight. Bounded by the peer table: an entry is only
    /// claimed for a sender the connection handler has already found in `peers`, and is removed when
    /// the reply's last frame is gone.
    replying_to: Arc<papaya::HashMap<NodeId, ()>>,
    /// Live inbound connections and when each last completed a frame (#602's round 3, finding 1).
    inbound: Arc<papaya::HashMap<u64, Arc<InboundSlot>>>,
    next_inbound: AtomicU64,
    /// When the last preemption happened (mono ns; 0 = never), for the rate limit.
    last_preempt_ns: AtomicU64,
    /// Inbound connections per source address — kept only when `max_connections_per_source` is set.
    per_source: Arc<papaya::HashMap<std::net::IpAddr, Arc<AtomicU64>>>,
}

/// One inbound connection's standing for preemption: when it last **completed** a frame (monotonic
/// ns; its accept until it does), and the signal that closes it.
pub struct InboundSlot {
    last_frame_ns: AtomicU64,
    /// A frame is being read: never a victim (#602's round 4, Q-a) — a slow frame is the progress
    /// bound's and the floor's business, not preemption's.
    in_frame:      std::sync::atomic::AtomicBool,
    close:         tokio::sync::Notify,
}

impl InboundSlot {
    /// Records that a frame completed now.
    pub fn frame_completed(&self) {
        self.last_frame_ns.store(crate::sim_seam::mono_now_ns(), Ordering::Relaxed);
    }

    /// Marks whether a frame is being read.
    pub fn set_in_frame(&self, v: bool) {
        self.in_frame.store(v, Ordering::Relaxed);
    }

    /// Resolves when this connection has been chosen for preemption.
    pub async fn preempted(&self) {
        self.close.notified().await;
    }
}

/// One connection's place under the per-source cap; given back when dropped.
pub struct SourceGuard {
    ip:  std::net::IpAddr,
    map: Option<Arc<papaya::HashMap<std::net::IpAddr, Arc<AtomicU64>>>>,
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        let Some(map) = &self.map else { return };
        let map = map.pin();
        if let Some(c) = map.get(&self.ip)
            && c.fetch_sub(1, Ordering::AcqRel) == 1
        {
            // Last one from this source: drop the entry if it is still at zero, so the table holds
            // only sources with a connection.
            map.compute(self.ip, |e| match e {
                Some((_, c)) if c.load(Ordering::Acquire) == 0 => papaya::Operation::Remove,
                _ => papaya::Operation::Abort(()),
            });
        }
    }
}

/// A connection's registration; removed from the table when dropped.
pub struct InboundRegistration {
    id:   u64,
    map:  Arc<papaya::HashMap<u64, Arc<InboundSlot>>>,
    /// The slot the connection updates and listens on.
    pub slot: Arc<InboundSlot>,
}

impl Drop for InboundRegistration {
    fn drop(&mut self) {
        self.map.pin().remove(&self.id);
    }
}

impl TransportBounds {
    /// Admits one more inbound connection from `ip` under an opt-in per-source cap (`0` = off), and
    /// returns the guard that gives the slot back. `None` = the source is at its cap (#602's round 4).
    pub fn admit_source(&self, ip: std::net::IpAddr, cap: usize) -> Option<SourceGuard> {
        if cap == 0 {
            return Some(SourceGuard { ip, map: None });
        }
        let map = self.per_source.pin();
        let count = match map.get(&ip) {
            Some(c) => Arc::clone(c),
            None => {
                let fresh = Arc::new(AtomicU64::new(0));
                let mut held = None;
                map.compute(ip, |e| match e {
                    Some((_, c)) => { held = Some(Arc::clone(c)); papaya::Operation::Abort(()) }
                    None => { held = Some(Arc::clone(&fresh)); papaya::Operation::Insert(Arc::clone(&fresh)) }
                });
                held.expect("compute sets it on both arms")
            }
        };
        if count.fetch_add(1, Ordering::AcqRel) >= cap as u64 {
            count.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(SourceGuard { ip, map: Some(Arc::clone(&self.per_source)) })
    }

    /// Registers an inbound connection accepted now.
    pub fn register_inbound(&self) -> InboundRegistration {
        let id = self.next_inbound.fetch_add(1, Ordering::Relaxed);
        let slot = Arc::new(InboundSlot {
            last_frame_ns: AtomicU64::new(crate::sim_seam::mono_now_ns()),
            in_frame:      std::sync::atomic::AtomicBool::new(false),
            close:         tokio::sync::Notify::new(),
        });
        self.inbound.pin().insert(id, Arc::clone(&slot));
        InboundRegistration { id, map: Arc::clone(&self.inbound), slot }
    }

    /// Preemption at the permit cap (#602's rounds 3–4). Closes the inbound connection whose last
    /// complete frame is oldest, provided that frame is at least `min_quiet` old and the connection is
    /// not mid-frame, and returns whether one was chosen — at most one per `min_interval` across the
    /// listener (hysteresis, round 4 Q-b). The caller is a newcomer that has **already completed its
    /// handshake and a valid first frame** (round 4, finding 2: preempting at accept let a bare connect
    /// close a member's link), and `min_quiet` is `writer_idle_timeout_secs + handshake_timeout_ms`
    /// (finding 1): an honest inbound link quiet that long is one whose remote writer has idle-closed
    /// it or is about to, so preemption never takes a link an honest peer is still using. What it
    /// cannot take: an attacker link that talks more often than `min_quiet` — it holds the slot it got
    /// from a free permit; that residual is stated in the threat model, and the opt-in per-source cap
    /// (`max_connections_per_source`) is the remaining mitigation.
    pub fn preempt_oldest(&self, min_quiet: std::time::Duration, min_interval: std::time::Duration) -> bool {
        let now = crate::sim_seam::mono_now_ns();
        let interval = min_interval.as_nanos().min(u64::MAX as u128) as u64;
        let last = self.last_preempt_ns.load(Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < interval {
            return false;
        }
        let min_quiet = min_quiet.as_nanos().min(u64::MAX as u128) as u64;
        let map = self.inbound.pin();
        let victim = map.iter()
            .filter(|(_, slot)| !slot.in_frame.load(Ordering::Relaxed))
            .map(|(id, slot)| (*id, slot.last_frame_ns.load(Ordering::Relaxed)))
            .filter(|(_, last)| now.saturating_sub(*last) >= min_quiet)
            .min_by_key(|(_, last)| *last);
        let Some((id, _)) = victim else { return false };
        // Take it out of the table so a second newcomer does not choose it too; the connection's
        // registration drop finds nothing to remove.
        let Some(slot) = map.remove(&id).cloned() else { return false };
        slot.close.notify_one();
        self.last_preempt_ns.store(now.max(1), Ordering::Relaxed);
        self.inbound_preempted.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_inbound_connections_preempted_total").increment(1);
        true
    }

    /// Claims `peer`'s reply slot, or returns `None` (and counts the skip) when a reply to `peer`
    /// is still in flight.
    pub fn claim_reply(&self, peer: &NodeId) -> Option<Arc<ReplySlot>> {
        let claimed = self.replying_to.pin().try_insert(peer.clone(), ()).is_ok();
        if !claimed {
            self.anti_entropy_replies_skipped.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("gossip_anti_entropy_replies_skipped_total").increment(1);
            return None;
        }
        Some(Arc::new(ReplySlot { peer: peer.clone(), map: Arc::clone(&self.replying_to) }))
    }

    /// Whether a reply to `peer` is in flight (tests and diagnostics).
    pub fn reply_in_flight(&self, peer: &NodeId) -> bool {
        self.replying_to.pin().contains_key(peer)
    }

    /// Counts one inbound connection closed because a frame in progress stalled.
    pub fn count_frame_stalled(&self) {
        self.inbound_frames_stalled.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_inbound_frames_stalled_total").increment(1);
    }

    /// Counts one inbound connection closed by a handshake, first-frame or idle bound.
    pub fn count_inbound_timeout(&self) {
        self.inbound_timed_out.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_inbound_connections_timed_out_total").increment(1);
    }
}

/// One peer's claimed anti-entropy reply slot; released when the last clone drops.
pub struct ReplySlot {
    peer: NodeId,
    map:  Arc<papaya::HashMap<NodeId, ()>>,
}

impl Drop for ReplySlot {
    fn drop(&mut self) {
        self.map.pin().remove(&self.peer);
    }
}

/// A frame that holds `slot` until the frame itself is dropped — after the writer wrote it, dropped
/// it during a reconnect backoff, or exited with it queued.
pub fn hold_slot(frame: Bytes, slot: &Arc<ReplySlot>) -> Bytes {
    Bytes::from_owner(SlotFrame { frame, _slot: Arc::clone(slot) })
}

struct SlotFrame {
    frame: Bytes,
    _slot: Arc<ReplySlot>,
}

impl AsRef<[u8]> for SlotFrame {
    fn as_ref(&self) -> &[u8] { self.frame.as_ref() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    #[test]
    fn a_peer_has_one_reply_in_flight_until_its_last_frame_is_gone() {
        let b = TransportBounds::default();
        let slot = b.claim_reply(&id(1)).expect("first claim");
        let f1 = hold_slot(Bytes::from_static(b"one"), &slot);
        let f2 = hold_slot(Bytes::from_static(b"two"), &slot);
        drop(slot);
        assert!(b.claim_reply(&id(1)).is_none(), "a reply's frames are still queued");
        assert!(b.claim_reply(&id(2)).is_some(), "another peer is not affected");
        assert_eq!(&f1[..], b"one");
        drop(f1);
        assert!(b.reply_in_flight(&id(1)), "one frame is still queued");
        drop(f2);
        assert!(!b.reply_in_flight(&id(1)), "the last frame released the slot");
        assert!(b.claim_reply(&id(1)).is_some());
        assert_eq!(b.anti_entropy_replies_skipped.load(Ordering::Relaxed), 1);
    }
}
