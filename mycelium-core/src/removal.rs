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

/// Keys **revoked by the node that held them** (post-360 plan row Q), as this node has validated
/// them: node `N`'s revocation of key `k` removes `k` from `N`'s trusted set and from no other node's.
/// The root crate validates a signed `sys/revocation/{N}/{k}` record (signed by a key this node holds
/// for `N`, naming a key it holds for `N`) and records it here; every verifier of a node's key reads
/// it through [`crate::CoreCtx::trusted_keys`] / [`crate::CoreCtx::retain_trusted`].
///
/// Per node, not one fleet-wide set of keys: a member that claims another's key in its own identity
/// record and then revokes it under its own name must not strip that key from its owner. Monotonic,
/// like [`RemovedSet`]: a revocation, once validated here, is never undone by a later tombstone or an
/// overwrite of the record that carried it. Lock-free, because the per-message path reads it.
#[derive(Default)]
pub struct RevokedKeys {
    by_node: papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>>,
}

impl std::fmt::Debug for RevokedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevokedKeys").field("nodes", &self.by_node.len()).finish()
    }
}

impl RevokedKeys {
    /// Record that `node` revoked `key`. Idempotent; returns whether it was new here.
    pub fn revoke(&self, node: &NodeId, key: [u8; 32]) -> bool {
        if self.is_revoked(node, &key) {
            return false;
        }
        // Retry-safe: the closure derives the new set from the current one every time it runs.
        self.by_node.pin().compute(node.clone(), |existing| {
            let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
            if existing.is_some() && set.contains(&key) {
                papaya::Operation::Abort(())
            } else {
                set.insert(key);
                papaya::Operation::Insert(set)
            }
        });
        true
    }

    /// Has `node` revoked `key`?
    pub fn is_revoked(&self, node: &NodeId, key: &[u8; 32]) -> bool {
        !self.by_node.is_empty() && self.by_node.pin().get(node).is_some_and(|s| s.contains(key))
    }

    /// Every key `node` has revoked.
    pub fn of(&self, node: &NodeId) -> std::collections::HashSet<[u8; 32]> {
        self.by_node.pin().get(node).cloned().unwrap_or_default()
    }

    /// Has anyone revoked anything? (A fast path: the common case is an empty set.)
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
    fn a_revocation_is_scoped_to_the_node_that_made_it_and_is_monotonic() {
        let r = RevokedKeys::default();
        let owner = NodeId::new("127.0.0.1", 7011).unwrap();
        let other = NodeId::new("127.0.0.1", 7012).unwrap();
        assert!(r.is_empty() && !r.is_revoked(&owner, &[1u8; 32]));
        assert!(r.revoke(&owner, [1u8; 32]));
        assert!(!r.revoke(&owner, [1u8; 32]), "idempotent");
        assert!(r.is_revoked(&owner, &[1u8; 32]));
        assert!(!r.is_revoked(&other, &[1u8; 32]), "the owner's revocation says nothing about another node's set");
        assert_eq!(r.of(&owner).len(), 1);
    }
}
