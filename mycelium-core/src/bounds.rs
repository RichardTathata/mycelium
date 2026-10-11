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
    /// Inbound connections refused because their source address was at `max_connections_per_source`
    /// (the final review of #602, item 8). See `SystemStats::inbound_per_source_refused`.
    pub per_source_refused: AtomicU64,
    /// Inbound connections per source address — kept only when `max_connections_per_source` is set.
    per_source: Arc<papaya::HashMap<std::net::IpAddr, Arc<AtomicU64>>>,
}

/// One connection's place under the per-source cap; given back when dropped. It holds the very
/// counter it incremented, so the give-back never looks the address up again: a lookup could find a
/// fresh counter installed after this one's entry was removed, decrement the wrong one and
/// undercount — or wrap it to `u64::MAX` and refuse the source for good (#602's round 5, item 4).
pub struct SourceGuard {
    ip:      std::net::IpAddr,
    counter: Option<Arc<AtomicU64>>,
    map:     Option<Arc<papaya::HashMap<std::net::IpAddr, Arc<AtomicU64>>>>,
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        let (Some(counter), Some(map)) = (&self.counter, &self.map) else { return };
        if counter.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last one from this source: remove the entry only if it is still *this* counter and still
            // at zero, so the table holds only sources with a connection and a newer counter is left be.
            map.pin().compute(self.ip, |e| match e {
                Some((_, c)) if Arc::ptr_eq(c, counter) && c.load(Ordering::Acquire) == 0 => papaya::Operation::Remove,
                _ => papaya::Operation::Abort(()),
            });
        }
    }
}

impl TransportBounds {
    /// Admits one more inbound connection from `ip` under an opt-in per-source cap (`0` = off), and
    /// returns the guard that gives the slot back. `None` = the source is at its cap (#602's round 4).
    /// Keys the **exact** address: an IPv6 host varying its low 64 bits is many sources to it.
    pub fn admit_source(&self, ip: std::net::IpAddr, cap: usize) -> Option<SourceGuard> {
        if cap == 0 {
            return Some(SourceGuard { ip, counter: None, map: None });
        }
        let map = self.per_source.pin();
        loop {
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
                self.per_source_refused.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_inbound_per_source_refused_total").increment(1);
                return None;
            }
            // A give-back may have removed this counter between the lookup and the increment; count
            // against the table's current one instead, so no connection is counted where nobody looks.
            if !map.get(&ip).is_some_and(|c| Arc::ptr_eq(c, &count)) {
                count.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            return Some(SourceGuard { ip, counter: Some(count), map: Some(Arc::clone(&self.per_source)) });
        }
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

    fn ip(n: u8) -> std::net::IpAddr { std::net::IpAddr::from([10, 0, 0, n]) }

    /// The final review of #602, item 6: a guard dropped, then the source admitted again — the give-back
    /// removed the entry and the next admit installs a fresh counter, never a stale or wrapped one.
    #[test]
    fn a_source_is_readmitted_after_its_guard_drops() {
        let b = TransportBounds::default();
        let g = b.admit_source(ip(1), 1).expect("first");
        assert!(b.admit_source(ip(1), 1).is_none(), "at the cap");
        assert_eq!(b.per_source_refused.load(Ordering::Relaxed), 1);
        drop(g);
        assert!(b.per_source.pin().get(&ip(1)).is_none(), "the last give-back removes the entry");
        let g2 = b.admit_source(ip(1), 1).expect("readmitted");
        assert!(b.admit_source(ip(1), 1).is_none(), "and the cap holds again");
        drop(g2);
    }

    /// Concurrent admits and drops on one source never leave it refused for good, or over its cap.
    #[test]
    fn concurrent_admits_and_drops_keep_the_count_exact() {
        let b = Arc::new(TransportBounds::default());
        let threads: Vec<_> = (0..8).map(|_| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                for _ in 0..5_000 {
                    if let Some(g) = b.admit_source(ip(2), 3) { drop(g); }
                }
            })
        }).collect();
        for t in threads { t.join().unwrap(); }
        assert!(b.per_source.pin().get(&ip(2)).is_none_or(|c| c.load(Ordering::Acquire) == 0),
                "every guard was dropped: nothing may still be counted");
        let held: Vec<_> = (0..3).map(|_| b.admit_source(ip(2), 3).expect("the full cap is available again")).collect();
        assert!(b.admit_source(ip(2), 3).is_none());
        drop(held);
    }

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
