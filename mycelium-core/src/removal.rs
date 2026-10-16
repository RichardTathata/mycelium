//! **Removed members** (Boundary H closure plan C5, [`docs/design/member-removal.md`]).
//!
//! The operator's statement that an identity is no longer one of us, applied. The root crate
//! verifies an operator-signed removal and records it here; this crate's transport consults it on
//! the per-message path and at the TLS handshake:
//!
//! - a ping, a signal or a gossip update **originated** by a removed node is dropped;
//! - a peer presenting a removed identity key is refused at the handshake, so it cannot rejoin by
//!   reconnecting.
//!
//! Monotonic: there is no un-removal. A removed identity is readmitted only as a **new** identity.
//! Lock-free (`papaya`), because the per-message path reads it.
//!
//! [`docs/design/member-removal.md`]: ../../../docs/design/member-removal.md

use crate::node_id::NodeId;

/// The set of removed members, by node id, by `id_hash` (what a gossip update names as its
/// origin), and by identity key (what a TLS certificate and a signature carry).
#[derive(Default)]
pub struct RemovedSet {
    nodes: papaya::HashSet<NodeId>,
    hashes: papaya::HashSet<u64>,
    keys: papaya::HashSet<[u8; 32]>,
}

impl std::fmt::Debug for RemovedSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemovedSet").field("nodes", &self.nodes.len()).field("keys", &self.keys.len()).finish()
    }
}

impl RemovedSet {
    /// Record `node` as removed, with every identity `key` it is known to hold. Idempotent.
    pub fn remove(&self, node: &NodeId, keys: impl IntoIterator<Item = [u8; 32]>) {
        self.nodes.pin().insert(node.clone());
        self.hashes.pin().insert(node.id_hash());
        let k = self.keys.pin();
        for key in keys {
            k.insert(key);
        }
    }

    /// Has `node` been removed?
    pub fn is_node_removed(&self, node: &NodeId) -> bool {
        !self.nodes.is_empty() && self.nodes.pin().contains(node)
    }

    /// Has the node with this `id_hash` been removed? (Gossip updates carry only the hash.)
    pub fn is_hash_removed(&self, id_hash: u64) -> bool {
        !self.hashes.is_empty() && self.hashes.pin().contains(&id_hash)
    }

    /// Is `key` a removed member's identity key?
    pub fn is_key_removed(&self, key: &[u8; 32]) -> bool {
        !self.keys.is_empty() && self.keys.pin().contains(key)
    }

    /// Every removed identity key.
    pub fn keys(&self) -> Vec<[u8; 32]> {
        self.keys.pin().iter().copied().collect()
    }

    /// Every removed node.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.nodes.pin().iter().cloned().collect()
    }

    /// Has anyone been removed? (A fast path: the common case is an empty set.)
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// One node's key standing, as this node has judged it (post-360 plan row Q,
/// `docs/design/revocation-authority.md`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeStanding {
    /// `Some` once the node has an in-window certificate record naming it (it is **anchored**): its
    /// trusted keys are then exactly these, and no key without standing verifies as it — not a bare
    /// or first-sighted identity key, not a legacy handshake anchor. `None` for a node without one
    /// (a legacy node): its keys are judged as before row Q, from `peer_keys`.
    pub anchored: Option<std::collections::HashSet<[u8; 32]>>,
    /// The keys the node has revoked, by a revocation this node counted.
    pub revoked: std::collections::HashSet<[u8; 32]>,
    /// The keys of [`revoked`](Self::revoked) whose revocation was **counted against a key with
    /// standing** — never a provisional one (of a key whose certificate record this node has not
    /// judged). Kept for good, even once the key leaves [`anchored`](Self::anchored) (the newest-16 cap,
    /// a saturated node). What the mTLS handshake refuses (post-360 plan row Q's follow-up).
    pub counted: std::collections::HashSet<[u8; 32]>,
}

/// Every node's [`NodeStanding`], read by every verifier of a node's key through
/// [`crate::CoreCtx::trusted_keys`] / [`crate::CoreCtx::retain_trusted`]. Written only by the root
/// crate's fold (`src/agent/revocation.rs`), which replaces a node's standing as a whole. Lock-free,
/// because the per-message path reads it.
#[derive(Default)]
pub struct KeyStanding {
    by_node: papaya::HashMap<NodeId, std::sync::Arc<NodeStanding>>,
}

impl std::fmt::Debug for KeyStanding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyStanding").field("nodes", &self.by_node.len()).finish()
    }
}

impl KeyStanding {
    /// Replace `node`'s standing.
    pub fn set(&self, node: &NodeId, standing: NodeStanding) {
        self.by_node.pin().insert(node.clone(), std::sync::Arc::new(standing));
    }

    /// `node`'s standing, if this node has judged one.
    pub fn get(&self, node: &NodeId) -> Option<std::sync::Arc<NodeStanding>> {
        if self.by_node.is_empty() {
            return None;
        }
        self.by_node.pin().get(node).cloned()
    }

    /// The node with this `id_hash` among those judged (a `SignedData` frame names its signer by hash).
    pub fn node_by_hash(&self, id_hash: u64) -> Option<NodeId> {
        if self.by_node.is_empty() {
            return None;
        }
        self.by_node.pin().iter().find(|(n, _)| n.id_hash() == id_hash).map(|(n, _)| n.clone())
    }

    /// Has `node` revoked `key`?
    pub fn is_revoked(&self, node: &NodeId, key: &[u8; 32]) -> bool {
        self.get(node).is_some_and(|s| s.revoked.contains(key))
    }

    /// Has `node` revoked `key` by a revocation counted against a key with standing
    /// ([`NodeStanding::counted`])? What the mTLS handshake refuses (post-360 plan row Q's follow-up).
    pub fn is_counted_revoked(&self, node: &NodeId, key: &[u8; 32]) -> bool {
        self.get(node).is_some_and(|s| s.counted.contains(key))
    }

    /// A node that has revoked `key` by a counted revocation, if any. A revocation counts only against
    /// a key anchored for the revoking node — a CA-issued certificate naming it — so this finds the
    /// key's owner and no other node's revocation.
    pub fn counted_revoker_of(&self, key: &[u8; 32]) -> Option<NodeId> {
        if self.by_node.is_empty() {
            return None;
        }
        self.by_node.pin().iter().find(|(_, s)| s.counted.contains(key)).map(|(n, _)| n.clone())
    }

    /// Is `key` **not** a key this node trusts for `node`, whatever else holds it — revoked, or, for an
    /// anchored node, not one of its anchored keys?
    pub fn is_distrusted(&self, node: &NodeId, key: &[u8; 32]) -> bool {
        self.get(node).is_some_and(|s| {
            s.revoked.contains(key) || s.anchored.as_ref().is_some_and(|a| !a.contains(key))
        })
    }

    /// Every node with a judged standing.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.by_node.pin().iter().map(|(n, _)| n.clone()).collect()
    }

    /// Has anything been judged? (A fast path: the common case before row Q's records exist.)
    pub fn is_empty(&self) -> bool {
        self.by_node.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_removal_is_seen_by_node_hash_and_key_and_nothing_else() {
        let r = RemovedSet::default();
        let gone = NodeId::new("127.0.0.1", 7001).unwrap();
        let stays = NodeId::new("127.0.0.1", 7002).unwrap();
        assert!(r.is_empty() && !r.is_node_removed(&gone));
        r.remove(&gone, [[1u8; 32], [2u8; 32]]);
        assert!(r.is_node_removed(&gone) && r.is_hash_removed(gone.id_hash()));
        assert!(r.is_key_removed(&[1u8; 32]) && r.is_key_removed(&[2u8; 32]));
        assert!(!r.is_node_removed(&stays) && !r.is_hash_removed(stays.id_hash()) && !r.is_key_removed(&[3u8; 32]));
    }

    #[test]
    fn a_standing_is_per_node_and_an_anchored_node_trusts_only_its_anchors() {
        let k = KeyStanding::default();
        let owner = NodeId::new("127.0.0.1", 7011).unwrap();
        let other = NodeId::new("127.0.0.1", 7012).unwrap();
        assert!(k.is_empty() && !k.is_distrusted(&owner, &[1u8; 32]));
        k.set(&owner, NodeStanding {
            anchored: Some([[2u8; 32], [3u8; 32]].into_iter().collect()),
            revoked: [[3u8; 32]].into_iter().collect(),
            counted: [[3u8; 32]].into_iter().collect(),
        });
        assert!(k.is_distrusted(&owner, &[1u8; 32]), "not anchored");
        assert!(!k.is_distrusted(&owner, &[2u8; 32]));
        assert!(k.is_distrusted(&owner, &[3u8; 32]) && k.is_revoked(&owner, &[3u8; 32]), "anchored, revoked");
        assert!(!k.is_distrusted(&other, &[3u8; 32]), "the owner's standing says nothing about another node");
        assert_eq!(k.node_by_hash(owner.id_hash()), Some(owner.clone()));
    }
}
