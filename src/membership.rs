//! **Removing a member** (Boundary H closure plan C5, ADR
//! [`docs/design/member-removal.md`](../../docs/design/member-removal.md)).
//!
//! # The gap this closes
//!
//! An operator could not cut a member off. Identity-key revocation is **self**-revocation: an honest
//! node retires its own compromised key, and a colluding node never signs its own. It affects
//! signatures, not traffic. At the transport, any certificate chaining to the fleet CA is a member.
//!
//! # What this does
//!
//! A [`MemberRemoval`], signed by a configured **membership authority** (P1's external-issuer path),
//! names a node and every identity key it is known to hold. Once a node accepts one:
//! - the transport drops the removed node's pings, signals and gossip writes, and refuses its
//!   certificate at the TLS handshake, so it cannot rejoin by reconnecting;
//! - RPC receive refuses it before any serve path (`CallerError::Removed`);
//! - its keys read as revoked, so its signatures are attributable history, never current, and A1
//!   refuses any mandate it holds (the holder's possession proof no longer verifies).
//!
//! **Monotonic:** there is no un-removal; a removed identity is readmitted only as a new one.
//!
//! # Stale membership: split by layer (ADR §2.3, adopted)
//!
//! A node that has not heard from the membership authority keeps every peer it has not seen
//! removed. Presence (gossip, liveness, peering) fails open, so one authority outage does not
//! partition the fleet. Protected work already fails closed through A1 and C3.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::knowledge::issuer::{verify_signed_by, Authenticity, MemberKeySource, SharedExternalIssuers, UnverifiableReason};
#[cfg(test)]
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::knowledge::IssuerId;
use crate::node_id::NodeId;

/// An operator's statement: `node` is no longer a member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRemoval {
    /// The membership authority that removes it.
    pub authority: IssuerId,
    /// The node removed.
    pub node: NodeId,
    /// Every identity key the node is known to hold, current and retained, so a rotation does not
    /// escape the removal.
    pub keys: Vec<[u8; 32]>,
    /// Orders this authority's removals.
    pub seq: u64,
    /// When the authority issued it.
    pub issued_at_ms: u64,
    /// Why, in the operator's words.
    pub reason: String,
}

impl MemberRemoval {
    /// The exact bytes the authority signs. Tagged and length-prefixed.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn lp(out: &mut Vec<u8>, b: &[u8]) {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        let mut out = Vec::new();
        lp(&mut out, b"mycelium.membership/removal/1");
        lp(&mut out, self.authority.as_str().as_bytes());
        lp(&mut out, self.node.as_str().as_bytes());
        out.extend_from_slice(&(self.keys.len() as u32).to_le_bytes());
        for k in &self.keys {
            out.extend_from_slice(k);
        }
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        lp(&mut out, self.reason.as_bytes());
        out
    }
}

/// A removal with its authority's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedMemberRemoval {
    /// The removal.
    pub removal: MemberRemoval,
    /// The authority's signature over [`MemberRemoval::canonical_bytes`].
    pub signature: Vec<u8>,
}

/// What a node did with an offered removal.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalOffer {
    /// Verified and applied.
    Accepted,
    /// The node was already removed; nothing changed.
    AlreadyRemoved,
    /// Not signed by one of this node's configured membership authorities.
    UntrustedAuthority,
    /// The signature does not verify for the named authority.
    Unverifiable(UnverifiableReason),
}

/// The membership authorities a node takes removals from.
#[derive(Clone, Debug)]
pub struct MembershipAuthorities {
    authorities: BTreeSet<IssuerId>,
    external: SharedExternalIssuers,
}

impl MembershipAuthorities {
    /// Take removals from `authorities`, verified through `external` (their configured keys). `external` may be a
    /// set shared with the node's other holders; a client principal's key in it is never read here — a membership
    /// authority is configured, never a client (identity-and-lifecycle I1).
    pub fn new(authorities: impl IntoIterator<Item = IssuerId>, external: impl Into<SharedExternalIssuers>) -> Self {
        Self { authorities: authorities.into_iter().collect(), external: external.into() }
    }

    /// Is `signed` a removal from one of these authorities, with a signature that verifies?
    pub fn verify(&self, signed: &SignedMemberRemoval, members: &impl MemberKeySource) -> Result<(), RemovalOffer> {
        let r = &signed.removal;
        if !self.authorities.contains(&r.authority) {
            return Err(RemovalOffer::UntrustedAuthority);
        }
        // The configured keys alone: a client key fed to a shared set never entitles a removal.
        let external = self.external.read().configured_only();
        match verify_signed_by(&r.authority, &r.canonical_bytes(), &signed.signature, members, &external) {
            Authenticity::Current { .. } => Ok(()),
            Authenticity::Revoked { .. } => Err(RemovalOffer::Unverifiable(UnverifiableReason::BadSignature)),
            Authenticity::Unverifiable(reason) => Err(RemovalOffer::Unverifiable(reason)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::HashMap;

    fn signed(key: &SigningKey, authority: &str, node: &NodeId) -> SignedMemberRemoval {
        let removal = MemberRemoval {
            authority: IssuerId::new(authority).unwrap(),
            node: node.clone(),
            keys: vec![[7u8; 32]],
            seq: 1,
            issued_at_ms: 1_000,
            reason: "compromised".into(),
        };
        SignedMemberRemoval { signature: key.sign(&removal.canonical_bytes()).to_bytes().to_vec(), removal }
    }

    /// A removal verifies only from a configured authority, and only with its signature intact.
    #[test]
    fn a_removal_verifies_only_from_a_configured_authority_with_its_signature() {
        let op = SigningKey::from_bytes(&[41u8; 32]);
        let rogue = SigningKey::from_bytes(&[42u8; 32]);
        let mut external = TrustedExternalIssuers::new();
        external.trust(IssuerId::new("operator:membership").unwrap(), op.verifying_key().to_bytes()).unwrap();
        external.trust(IssuerId::new("operator:other").unwrap(), rogue.verifying_key().to_bytes()).unwrap();
        let auth = MembershipAuthorities::new([IssuerId::new("operator:membership").unwrap()], external);
        let members: HashMap<NodeId, MemberKeys> = HashMap::new();
        let node = NodeId::new("127.0.0.1", 7100).unwrap();

        assert_eq!(auth.verify(&signed(&op, "operator:membership", &node), &members), Ok(()));
        // The plant's opposites: a trusted issuer that is not a membership authority, and a forgery.
        assert_eq!(auth.verify(&signed(&rogue, "operator:other", &node), &members), Err(RemovalOffer::UntrustedAuthority));
        let mut forged = signed(&op, "operator:membership", &node);
        forged.removal.node = NodeId::new("127.0.0.1", 7101).unwrap();
        assert!(matches!(auth.verify(&forged, &members), Err(RemovalOffer::Unverifiable(_))));
    }

    /// **A membership authority is never a client** (identity-and-lifecycle I1): the set is shared with the node's
    /// other holders, and the client-certificate fold feeds client principals' keys into it — but a removal signed by
    /// a client key, even for a principal listed as an authority, does not verify, and one signed by the same
    /// authority's configured key does. The set is shared: a key configured after the authorities were built counts.
    /// Seen failing first with the full set read instead of the configured keys: the client-signed removal verified.
    #[test]
    fn a_client_key_in_a_shared_set_never_entitles_a_removal() {
        use crate::knowledge::issuer::SharedExternalIssuers;
        let client = SigningKey::from_bytes(&[43u8; 32]);
        let op = SigningKey::from_bytes(&[44u8; 32]);
        let shared = SharedExternalIssuers::default();
        let authority = IssuerId::new("token:fleet/ops").unwrap();
        let auth = MembershipAuthorities::new([authority.clone()], shared.clone());
        let members: HashMap<NodeId, MemberKeys> = HashMap::new();
        let node = NodeId::new("127.0.0.1", 7102).unwrap();
        shared.set_client_key(authority.clone(), client.verifying_key().to_bytes()).unwrap();
        assert!(matches!(auth.verify(&signed(&client, "token:fleet/ops", &node), &members), Err(RemovalOffer::Unverifiable(_))),
            "ENTITLED: a client key verified a removal");
        shared.trust(authority, op.verifying_key().to_bytes()).unwrap();
        assert_eq!(auth.verify(&signed(&op, "token:fleet/ops", &node), &members), Ok(()), "the configured key, added to the shared set later, counts");
    }
}
