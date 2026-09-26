//! **Removing a member, at the agent** (Boundary H closure plan C5). The record and its
//! verification are in [`crate::membership`]; this applies an accepted removal to the node:
//! the core `RemovedSet` (transport, handshake, RPC receive), the peer table, the outbound writer,
//! and publication under `sys/membership/removed/{node}` so the removal spreads by gossip too.

use std::sync::Arc;

use bytes::Bytes;

use super::{GossipAgent, TaskCtx};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::knowledge::IssuerId;
use crate::membership::{MembershipAuthorities, RemovalOffer, SignedMemberRemoval};
use crate::node_id::NodeId;
use crate::signal::kv_ns::MEMBERSHIP_REMOVED;

/// How often a node reads removals that arrived by gossip.
const INGEST_INTERVAL_MS: u64 = 1_000;

type Writers = Arc<papaya::HashMap<NodeId, mycelium_core::writer::WriterEntry>>;

/// Verify `signed` and, if it is new and good, apply it. `publish` writes it to the KV so it
/// spreads; a removal read from the KV is not re-published.
fn apply(ctx: &TaskCtx, auth: &MembershipAuthorities, writers: &Writers, signed: &SignedMemberRemoval, publish: bool) -> RemovalOffer {
    let node = &signed.removal.node;
    if ctx.removed.is_node_removed(node) {
        return RemovalOffer::AlreadyRemoved;
    }
    #[cfg(feature = "compliance")]
    let members = super::knowledge_keys::member_keys_of(ctx);
    #[cfg(not(feature = "compliance"))]
    let members: std::collections::HashMap<NodeId, crate::knowledge::issuer::MemberKeys> = Default::default();
    if let Err(refusal) = auth.verify(signed, &members) {
        return refusal;
    }

    // Every key the removal names, plus every key this node has seen the member hold, so a
    // rotation before or after the removal does not escape it.
    let mut keys = signed.removal.keys.clone();
    if let Some(seen) = ctx.peer_keys.pin().get(node) {
        keys.extend(seen.iter().copied());
    }
    if let Some(anchored) = ctx.peer_anchor_keys.pin().get(node) {
        keys.extend(anchored.iter().copied());
    }
    ctx.removed.remove(node, keys);
    ctx.peers.pin().remove(node);
    writers.pin().remove(node);
    tracing::warn!(%node, reason = %signed.removal.reason, "membership: member removed");

    if publish && let Ok(bytes) = serde_json::to_vec(signed) {
        let key: Arc<str> = Arc::from(format!("{MEMBERSHIP_REMOVED}{node}").as_str());
        mycelium_core::ops::kv_set(ctx, key, Bytes::from(bytes));
    }
    RemovalOffer::Accepted
}

impl GossipAgent {
    /// **Take member removals from these authorities** (closure plan C5), verified through
    /// `external` (their configured keys). Also starts reading removals that arrive by gossip
    /// under `sys/membership/removed/`, each verified before it is applied. Set once.
    pub fn with_membership_authorities(
        &self,
        authorities: impl IntoIterator<Item = IssuerId>,
        external: TrustedExternalIssuers,
    ) {
        let auth = Arc::new(MembershipAuthorities::new(authorities, external));
        if self.task_ctx.membership.set(Arc::clone(&auth)).is_err() {
            tracing::warn!("with_membership_authorities: already configured; ignoring");
            return;
        }
        let (ctx, writers) = (Arc::clone(&self.task_ctx), Arc::clone(&self.peer_writers));
        let mut shutdown = self.task_ctx.shutdown_tx.subscribe();
        self.task_ctx.spawn_task(async move {
            loop {
                tokio::select! {
                    _ = super::capability_ops::await_shutdown(&mut shutdown) => break,
                    _ = mycelium_core::sim_seam::sleep_ms("membership/ingest", INGEST_INTERVAL_MS) => {}
                }
                for (_, value) in crate::store::scan_kv_prefix(&ctx.kv_state, MEMBERSHIP_REMOVED) {
                    if let Ok(signed) = serde_json::from_slice::<SignedMemberRemoval>(&value) {
                        let _ = apply(&ctx, &auth, &writers, &signed, false);
                    }
                }
            }
        });
    }

    /// **Remove a member** (closure plan C5): verify an operator-signed removal and apply it here,
    /// and publish it so it reaches every member by gossip. `None` if no membership authorities
    /// are configured. Monotonic: there is no un-removal.
    pub fn offer_member_removal(&self, signed: &SignedMemberRemoval) -> Option<RemovalOffer> {
        let auth = self.task_ctx.membership.get()?;
        Some(apply(&self.task_ctx, auth, &self.peer_writers, signed, true))
    }

    /// The members this node has removed.
    pub fn removed_members(&self) -> Vec<NodeId> {
        self.task_ctx.removed.nodes()
    }
}
