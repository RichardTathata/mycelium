//! The node side of **issuer binding** (Boundary H plan item P1).
//!
//! [`crate::knowledge::issuer`] is pure: it verifies a record against a reader's view of member keys
//! and configured external issuers. This supplies that view from a live node, and lets a node sign a
//! knowledge record **only as itself**.
//!
//! Gated on `compliance` because the member path's two inputs live there: the retained identity-key
//! set (`peer_keys`, learned from `sys/identity/`) and the validated revocations (`sys/revocation/`).
//! Its strength rests on `require_identity_proofs`, which is **on by default** since post-360 plan
//! row S: with it explicitly off, the retained set can include a key injected into `sys/identity/` by
//! another admitted member (the identity-poisoning residual).

use std::collections::HashMap;

use super::GossipAgent;
use crate::knowledge::issuer::{MemberKeys, SignAsMemberError};
use crate::knowledge::{IssuerId, KnowledgeRecord};
use crate::node_id::NodeId;

impl GossipAgent {
    /// This node's current view of every member's identity keys, for
    /// [`crate::knowledge::issuer::verify_issuer`].
    ///
    /// A **snapshot**: retained keys (current and historical, so a signature made before a routine
    /// rotation still verifies) and, separately, the subset this node has seen **validly revoked**.
    /// Revoked keys are kept in `retained` on purpose — a record signed under one is still
    /// attributable as history, and verification reports it as `Revoked` rather than failing it.
    ///
    /// Includes this node itself, with its current key, even before its own `sys/identity` write has
    /// cycled back through the watcher.
    pub fn knowledge_member_keys(&self) -> HashMap<NodeId, MemberKeys> {
        member_keys_of(&self.task_ctx)
    }
    /// Sign `record` with this node's identity key — **only if the record is issued by this node**
    /// ([`IssuerId::for_node`] of this node's id).
    ///
    /// The refusal is the point: this API never produces a signature a reader would check against
    /// another member's keys. (A key holder can of course sign arbitrary bytes with
    /// [`sign_with_identity`](Self::sign_with_identity); such a signature still verifies only under
    /// this node's keys, so it cannot make this node another issuer.)
    pub fn sign_knowledge_record(
        &self,
        record: &KnowledgeRecord,
    ) -> Result<[u8; 64], SignAsMemberError> {
        let mine = IssuerId::for_node(&self.node_id);
        if record.issuer() != &mine {
            return Err(SignAsMemberError::NotThisMember { issuer: record.issuer().clone() });
        }
        self.sign_with_identity(&record.canonical_bytes()).ok_or(SignAsMemberError::NoIdentity)
    }
}

/// The member-key snapshot behind [`GossipAgent::knowledge_member_keys`], from a task context.
pub(crate) fn member_keys_of(ctx: &super::TaskCtx) -> HashMap<NodeId, MemberKeys> {
    let mut out: HashMap<NodeId, MemberKeys> = HashMap::new();
    for (node, keys) in ctx.peer_keys.pin().iter() {
        let entry = out.entry(node.clone()).or_default();
        for k in keys {
            if !entry.retained.contains(k) {
                entry.retained.push(*k);
            }
        }
    }
    // Row Q: an anchored node's certified keys are retained (attributable) too, even when no identity
    // record put them in `peer_keys`.
    for node in ctx.key_standing.nodes() {
        if let Some(s) = ctx.key_standing.get(&node) {
            let entry = out.entry(node.clone()).or_default();
            for k in s.anchored.iter().flatten() {
                if !entry.retained.contains(k) {
                    entry.retained.push(*k);
                }
            }
        }
    }
    if let Some(t) = ctx.tls.get() {
        let entry = out.entry(ctx.node_id.clone()).or_default();
        let cur = t.verifying_key_bytes();
        if !entry.retained.contains(&cur) {
            entry.retained.insert(0, cur);
        }
    }
    // Post-360 plan row Q: a key is revoked *for the node that revoked it* (and a removed member's
    // keys for everyone) — the same answer every other verifier of a node's key reads.
    for (node, entry) in out.iter_mut() {
        let mut trusted = entry.retained.clone();
        ctx.retain_trusted(node, &mut trusted);
        if node == &ctx.node_id && let Some(t) = ctx.tls.get() {
            let cur = t.verifying_key_bytes();
            if !trusted.contains(&cur) { trusted.push(cur); }
        }
        entry.revoked = entry.retained.iter().filter(|k| !trusted.contains(*k)).copied().collect();
    }
    out
}
