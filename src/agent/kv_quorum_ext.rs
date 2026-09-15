//! [`KvQuorumExt`] — the quorum-durability write overlay on [`KvHandle`].
//!
//! The substrate `KvHandle` (in `mycelium-core`) does substrate KV: last-write-
//! wins propagation with no acknowledgement. *Durability-by-ACK-count* —
//! "block until N peers have received this write" — is a guarantee layered **on**
//! the substrate, so it lives here as an extension trait rather than as an
//! inherent method on the core handle. This is "consistency as a service, not a
//! foundation" expressed in the type system: callers opt in with
//! `use mycelium::KvQuorumExt;`.
//!
//! It is still only *durability*, not consensus — see the method docs.

use crate::KvHandle;
use bytes::Bytes;
use mycelium_core::ops::kv_set_async;
use std::{sync::Arc, time::Duration};

use super::kv_quorum::QuorumError;

/// Propagation-acknowledgement write overlay for [`KvHandle`]. Import to call
/// [`set_with_min_acks`](KvQuorumExt::set_with_min_acks).
pub trait KvQuorumExt {
    /// Writes `value` under `key` and waits for at least `min_acks` distinct peers
    /// to confirm receipt before returning — **but see the warning below: on today's substrate
    /// that confirmation cannot arrive.**
    ///
    /// # ⚠ This verb cannot succeed on today's substrate
    ///
    /// Read this before calling it. With `min_acks >= 1` it will time out even when every peer in
    /// the cluster has received and applied the write. That is not a bug in the peers; it is a
    /// structural gap, measured and pinned by
    /// `a_peer_holding_the_write_still_produces_no_acknowledgement` (`kv_handle_tests.rs`):
    /// two connected nodes, the peer demonstrably holding the value, `Err(Timeout { acks_received: 0 })`.
    ///
    /// Three facts compose into it, none of which is visible from this signature:
    ///
    /// - A `GossipUpdate`'s `sender` is its **originating** node, preserved unchanged across every
    ///   forwarding hop. A peer relaying our write is therefore attributed to *us*, and the
    ///   tracker's loopback filter discards it.
    /// - Fan-out **excludes the origin**, so the relayed copy is never sent back to us anyway.
    /// - Anti-entropy re-attributes the entries it delivers to the **receiving** node.
    ///
    /// So no inbound frame on this substrate carries "a peer holds your write". What the counter
    /// *can* observe is a distinct origin independently gossiping this same payload under this
    /// key — real evidence that some peer holds this value, but not a receipt for this operation.
    ///
    /// # What an ack is — exact payload identity, still not durability
    ///
    /// Since the contracts axis' item 1 PR 4a, an ack requires the inbound update's
    /// [`content_hash`](mycelium_core::receipt::content_hash) to equal this write's, from an origin
    /// that is not us, at or after this write's HLC. Before 4a any update at or after that stamp
    /// counted, so a **newer overwrite** — a payload that peer never received from us — satisfied
    /// the count. That was the overclaim (plan D9); it is gone, and removing it is why a stray
    /// concurrent writer can no longer produce a false `Ok`.
    ///
    /// It still establishes nothing about **durability**: no peer has promised that anything
    /// reached its disk. The persisted-by-peer protocol that makes a replica-sync receipt
    /// obtainable is item 1 PR 4b (`docs/design/contracts-receipts.md` §2.1, `docs/plans/v3-contracts-axis.md`).
    ///
    /// It is also **not** linearisability, total order, or consensus. Two concurrent callers
    /// writing different values to the same key both succeed here and LWW resolves the winner
    /// silently. For a linearisable write use
    /// [`consistent_set`](crate::GossipAgent::consistent_set).
    ///
    /// # Errors
    ///
    /// Returns [`QuorumError::Timeout`] when fewer than `min_acks` peers confirm
    /// within `timeout`. The write is **not** rolled back.
    // Public-trait `async fn`: this trait is only ever implemented for the concrete
    // `KvHandle` and called directly (never behind a generic with auto-trait bounds),
    // so the `async_fn_in_trait` Send-bound caveat does not apply.
    #[allow(async_fn_in_trait)]
    async fn set_with_min_acks(
        &self,
        key:      impl Into<Arc<str>>,
        value:    impl Into<Bytes>,
        min_acks: usize,
        timeout:  Duration,
    ) -> Result<usize, QuorumError>;
}

impl KvQuorumExt for KvHandle {
    async fn set_with_min_acks(
        &self,
        key:      impl Into<Arc<str>>,
        value:    impl Into<Bytes>,
        min_acks: usize,
        timeout:  Duration,
    ) -> Result<usize, QuorumError> {
        use super::kv_quorum::{install_tracker, remove_tracker, QuorumAckTracker};

        let ctx = self.core();
        let key:   Arc<str> = key.into();
        let value: Bytes    = value.into();

        if min_acks == 0 {
            let _ = kv_set_async(ctx, key, value).await;
            return Ok(0);
        }

        let write_ts_min = ctx.hlc.tick();
        let self_hash    = ctx.node_id.id_hash();
        // The payload's identity, so a newer overwrite of this key can never be mistaken for
        // evidence that a peer holds *this* value (PR 4a). `false` = not a tombstone: this verb
        // only writes.
        let write_content = mycelium_core::receipt::content_hash(key.as_ref(), value.as_ref(), false);
        let (tracker, mut rx) = QuorumAckTracker::new(write_ts_min, self_hash, write_content);
        install_tracker(&ctx.kv_state.quorum_trackers, Arc::clone(&key), &tracker);

        let _ = kv_set_async(ctx, Arc::clone(&key), value).await;

        let result = tokio::time::timeout(timeout, async {
            loop {
                let n = *rx.borrow();
                if n >= min_acks { return n; }
                if rx.changed().await.is_err() { return *rx.borrow(); }
            }
        })
        .await;

        remove_tracker(&ctx.kv_state.quorum_trackers, &key, &tracker);

        match result {
            Ok(n)  => Ok(n),
            Err(_) => Err(QuorumError::Timeout { acks_received: *rx.borrow() }),
        }
    }
}
