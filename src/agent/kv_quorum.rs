//! Per-write ACK tracker for [`GossipAgent::set_with_min_acks`].
//!
//! `QuorumAckTracker` is installed by `set_with_min_acks` just before the write and
//! removed after the wait completes (success or timeout). It lives in
//! `KvState::quorum_trackers` keyed by the key string.  `apply_and_notify`
//! calls `observe` for every incoming update so each tracker learns when
//! distinct peers have confirmed the value.
//!
//! The tracker is per-write, not per-key: each key maps to a copy-on-write
//! *list* of trackers, so concurrent `set_with_min_acks` calls on the same
//! key coexist — every inbound update is observed by all of them, and each
//! caller removes exactly its own tracker (by `Arc` identity) on completion.
//! (A previous single-slot design let the second caller overwrite the first
//! tracker, and the first caller's unconditional cleanup then deleted the
//! second's — both callers could time out spuriously. M2 Run-18 sweep
//! finding.)

use std::sync::Arc;
use papaya::HashMap;
use tokio::sync::watch;

/// Per-key tracker list type now lives in core as
/// [`crate::store::QuorumTrackerList`] (`Arc<Vec<Arc<dyn QuorumObserver>>>`) so the
/// substrate's `KvState` never names this upper type. `QuorumAckTracker` implements
/// the core `QuorumObserver` trait; the list holds it as a trait object.
use crate::store::{QuorumObserver, QuorumTrackerList};

/// Adds `tracker` to `key`'s list, coexisting with any concurrent callers'
/// trackers on the same key. The closure is retry-safe (clones per
/// invocation — papaya re-invokes it under CAS contention).
pub(crate) fn install_tracker(
    map:     &HashMap<Arc<str>, QuorumTrackerList>,
    key:     Arc<str>,
    tracker: &Arc<QuorumAckTracker>,
) {
    let obs_concrete = Arc::clone(tracker);
    let obs: Arc<dyn QuorumObserver> = obs_concrete;
    map.pin().compute(key, |existing| -> papaya::Operation<QuorumTrackerList, ()> {
        match existing {
            None => papaya::Operation::Insert(Arc::new(vec![Arc::clone(&obs)])),
            Some((_, list)) => {
                let mut v = (**list).clone();
                v.push(Arc::clone(&obs));
                papaya::Operation::Insert(Arc::new(v))
            }
        }
    });
}

/// Removes exactly `tracker` (by `Arc` identity) from `key`'s list — never a
/// concurrent caller's tracker. Drops the map entry when the list empties.
pub(crate) fn remove_tracker(
    map:     &HashMap<Arc<str>, QuorumTrackerList>,
    key:     &Arc<str>,
    tracker: &Arc<QuorumAckTracker>,
) {
    let needle_concrete = Arc::clone(tracker);
    let needle: Arc<dyn QuorumObserver> = needle_concrete;
    map.pin().compute(Arc::clone(key), |existing| -> papaya::Operation<QuorumTrackerList, ()> {
        match existing {
            None => papaya::Operation::Abort(()),
            Some((_, list)) => {
                let v: Vec<Arc<dyn QuorumObserver>> = list
                    .iter()
                    .filter(|t| !Arc::ptr_eq(t, &needle))
                    .cloned()
                    .collect();
                if v.len() == list.len() {
                    papaya::Operation::Abort(())
                } else if v.is_empty() {
                    papaya::Operation::Remove
                } else {
                    papaya::Operation::Insert(Arc::new(v))
                }
            }
        }
    });
}

/// Tracks how many distinct peers have confirmed a particular KV write.
///
/// Created by `set_with_min_acks` and observed by `apply_and_notify`.
pub(crate) struct QuorumAckTracker {
    /// Lower bound on the tracked write's HLC stamp, taken immediately before the
    /// write. Evidence older than this cannot be about this write.
    pub(crate) write_ts:  u64,
    /// `id_hash()` of this node — used to filter out loopback `apply_and_notify`
    /// calls that originate from our own local write.
    pub(crate) self_hash: u64,
    /// [`content_hash`](mycelium_core::receipt::content_hash) of the exact payload this
    /// write carries. An update that does not hash to this is **a different value**, and
    /// counting it was the overclaim PR 4a removes.
    pub(crate) write_content: u64,
    /// Set of peer `id_hash` values that have confirmed the write.
    pub(crate) acked_by:  HashMap<u64, ()>,
    /// Notifies the waiter whenever `acked_by.len()` increases.
    pub(crate) notify_tx: watch::Sender<usize>,
}

impl QuorumAckTracker {
    pub(crate) fn new(
        write_ts: u64,
        self_hash: u64,
        write_content: u64,
    ) -> (Arc<Self>, watch::Receiver<usize>) {
        let (tx, rx) = watch::channel(0usize);
        let tracker = Arc::new(Self {
            write_ts,
            self_hash,
            write_content,
            acked_by:  HashMap::new(),
            notify_tx: tx,
        });
        (tracker, rx)
    }
}

impl QuorumObserver for QuorumAckTracker {
    /// The identity-less observation establishes nothing and therefore **never counts**.
    ///
    /// Before PR 4a this was the whole mechanism, and `sender != self && timestamp >= write_ts`
    /// was enough to call an update an acknowledgement. It is not: a *newer overwrite* of the
    /// same key satisfies both while carrying a payload the peer never received from us. An ack
    /// needs the payload's identity, which only [`observe_update`](QuorumObserver::observe_update)
    /// carries.
    fn observe(&self, _sender: u64, _timestamp: u64) {}

    /// Called by `apply_and_notify` for every incoming update on the tracked key. Counts the
    /// update only when it is **this exact payload**, from an origin that is not us, at or after
    /// the tracked write.
    ///
    /// **What this can establish, and what it structurally cannot.** `sender` is the update's
    /// *originating* node, preserved across every forwarding hop, and the fan-out excludes the
    /// origin — so a peer relaying our own write is both attributed to us and never sent back to
    /// us. Anti-entropy re-attributes the entries it delivers to the receiving node. There is
    /// therefore **no inbound frame on today's substrate that says "a peer holds your write"**,
    /// and this counter cannot reach a non-zero value from our own write's propagation, however
    /// widely it propagates. What it *can* count is a distinct origin independently gossiping this
    /// same payload under this key. The persisted-by-peer protocol that makes a replica-sync
    /// receipt obtainable is item 1 PR 4b; until it lands, `set_with_min_acks` cannot succeed and
    /// says so.
    fn observe_update(&self, sender: u64, timestamp: u64, _nonce: u64, content_hash: u64) {
        if sender != self.self_hash
            && timestamp >= self.write_ts
            && content_hash == self.write_content
        {
            let n = {
                let guard = self.acked_by.pin();
                guard.insert(sender, ());
                guard.len()
            };
            let _ = self.notify_tx.send(n);
        }
    }
}

#[cfg(test)]
mod floor_tests {
    //! The regression floor (contracts axis item 1, `docs/design/contracts-receipts.md` §8):
    //! executable pins of the min-acks tracker's ack semantics. PR 1 pinned the `>=` propagation
    //! rule; **PR 4a changed that meaning by changing this pin**, which is the discipline the
    //! floor exists for.
    use super::*;
    use mycelium_core::receipt::content_hash;

    /// PR 4a (2026-09-15) — replaces `floor_observe_counts_any_update_at_or_after_write_ts`.
    ///
    /// The old pin held that any update from a distinct origin at or after the write's HLC counted
    /// as an ack, **including a newer overwrite carrying a payload that peer never received from
    /// us**. That was the documented overclaim (plan D9). An ack now requires the payload's
    /// identity to match, so a newer overwrite counts for nothing.
    #[test]
    fn observe_counts_only_this_exact_payload() {
        let mine = content_hash("k", b"mine", false);
        let theirs = content_hash("k", b"theirs", false);
        let (tracker, rx) = QuorumAckTracker::new(1_000, /* self */ 1, mine);

        tracker.observe_update(1, 1_000, 1, mine);   // loopback from self: never an ack
        assert_eq!(*rx.borrow(), 0);
        tracker.observe_update(2, 999, 2, mine);     // older than the write: not evidence of it
        assert_eq!(*rx.borrow(), 0);
        tracker.observe_update(2, 1_000, 3, mine);   // this payload, from a peer: an ack
        assert_eq!(*rx.borrow(), 1);
        tracker.observe_update(2, 1_000, 4, mine);   // the same peer again: one distinct peer
        assert_eq!(*rx.borrow(), 1);

        // The flip. A *newer* value from another origin no longer counts: that peer holds a
        // different payload, and saying otherwise was the overclaim.
        tracker.observe_update(3, 5_000, 5, theirs);
        assert_eq!(
            *rx.borrow(), 1,
            "a newer overwrite is not evidence that any peer holds THIS payload (PR 4a)",
        );
    }

    /// The identity-less `observe` establishes nothing, so it must never count — not even for a
    /// sender and timestamp that would have satisfied the pre-4a rule. Implementations that only
    /// have this method keep compiling (the trait's default forwards to it); they simply cannot
    /// produce an acknowledgement, which is the honest outcome.
    #[test]
    fn the_identity_less_observation_is_never_an_ack() {
        let (tracker, rx) = QuorumAckTracker::new(1_000, 1, content_hash("k", b"v", false));
        tracker.observe(2, 5_000);
        assert_eq!(*rx.borrow(), 0, "no payload identity, no acknowledgement");
    }
}

/// Error returned by [`GossipAgent::set_with_min_acks`] when the durability threshold
/// is not reached within the timeout.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum QuorumError {
    /// The write propagated to fewer peers than requested within the deadline.
    Timeout {
        /// How many distinct peers had confirmed the write before the timeout.
        acks_received: usize,
    },
}

impl std::fmt::Display for QuorumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuorumError::Timeout { acks_received } =>
                write!(f, "set_with_min_acks timed out ({acks_received} peer(s) acknowledged)"),
        }
    }
}

impl std::error::Error for QuorumError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::GossipUpdate;
    use crate::store::{apply_and_notify, KvState};
    use bytes::Bytes;

    /// M2 Run-18 race-family sweep: concurrent `set_with_min_acks` callers on the
    /// SAME key must coexist. The previous single-slot tracker map let the second
    /// caller overwrite the first tracker, and the first caller's unconditional
    /// cleanup then deleted the second's — both could time out spuriously despite
    /// the acks arriving. (Relocated from `store.rs` with the quorum overlay in the
    /// ROADMAP §v2.0 M1 Stage 3b crate split; core holds only the `QuorumObserver`
    /// trait, the install/remove/tracker machinery lives here.)
    #[test]
    fn concurrent_quorum_trackers_coexist_and_remove_only_self() {
        let kv = KvState::new(0);
        let key: Arc<str> = Arc::from("q/k");
        // Both callers track the same payload, so the inbound updates below are evidence for
        // both (PR 4a: an ack requires the payload's identity to match).
        let content = mycelium_core::receipt::content_hash("q/k", b"v1", false);
        let (t1, rx1) = QuorumAckTracker::new(100, 1, content);
        let (t2, rx2) = QuorumAckTracker::new(100, 1, content);
        install_tracker(&kv.quorum_trackers, Arc::clone(&key), &t1);
        install_tracker(&kv.quorum_trackers, Arc::clone(&key), &t2);

        // One inbound peer update acks BOTH in-flight callers.
        apply_and_notify(&kv, &GossipUpdate {
            sender: 7, key: Arc::clone(&key), value: Bytes::from_static(b"v1"),
            timestamp: 150, nonce: 1, ttl: 1, is_tombstone: false,
        });
        assert_eq!(*rx1.borrow(), 1, "first caller sees the ack");
        assert_eq!(*rx2.borrow(), 1, "second caller sees the ack");

        // First caller completes: removes ONLY its own tracker.
        remove_tracker(&kv.quorum_trackers, &key, &t1);
        apply_and_notify(&kv, &GossipUpdate {
            sender: 8, key: Arc::clone(&key), value: Bytes::from_static(b"v1"),
            timestamp: 151, nonce: 2, ttl: 1, is_tombstone: false,
        });
        assert_eq!(*rx2.borrow(), 2, "surviving caller keeps receiving acks");
        assert_eq!(*rx1.borrow(), 1, "removed tracker is no longer observed");

        remove_tracker(&kv.quorum_trackers, &key, &t2);
        assert!(
            kv.quorum_trackers.pin().get(&key).is_none(),
            "entry drops when the last tracker is removed"
        );
    }
}
