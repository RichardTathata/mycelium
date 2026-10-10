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
}

impl TransportBounds {
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
