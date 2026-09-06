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
    /// to confirm receipt before returning.
    ///
    /// # What an ack is — propagation, not receipt, not durability
    ///
    /// The tracker counts an ack when a distinct peer gossips **any** update for `key` at or after this write's HLC timestamp — evidence that the write *propagated* (or was already superseded by a newer one), **not** that the peer received this exact payload and **not** that it persisted anything. A newer competing write from a peer therefore satisfies the count for a
    /// payload that peer never held. An exact-identity, persisted-by-peer receipt is the v3.0
    /// contracts axis, item 1 (`docs/plans/v3-contracts-axis.md`, PR 4a/4b). It also does **not**
    /// provide linearisability, total-order, or any consensus
    /// guarantee. Two concurrent callers writing different values to the same key will
    /// both succeed here; LWW resolves the winner silently. For a linearisable write
    /// use [`consistent_set`](crate::GossipAgent::consistent_set).
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
        let (tracker, mut rx) = QuorumAckTracker::new(write_ts_min, self_hash);
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
