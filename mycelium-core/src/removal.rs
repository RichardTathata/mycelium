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
}
