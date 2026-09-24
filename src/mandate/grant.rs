//! **Signed, portable mandate grants** (Boundary H item P2) —
//! [`docs/design/knowledge-issuer-binding.md`](../../../docs/design/knowledge-issuer-binding.md) §5.
//!
//! # The gap this closes
//!
//! A [`Mandate`] was an unsigned struct. Its epoch was verified only inside the protected resource
//! (the wiki's fence and pre-receive hook), so a *reader* elsewhere — deciding whether a capability's
//! advertiser is authorised, say — had nothing it could check. P2 makes an appointment portable: the
//! establishing authority signs it, and any reader can verify it without reaching the resource.
//!
//! # A signature proves who issued it. Four checks decide whether it counts.
//!
//! 1. **Issuance.** The grant verifies, through issuer binding's two admissible paths, under a key
//!    the reader holds for `established_by`. A grant signed under a revoked key is refused.
//! 2. **Entitlement.** The signer is entitled for that scope, by the reader's **configured**
//!    [`EntitlementTable`]. A signature never establishes entitlement on its own. Until the
//!    consensus acceptance gate passes, configuration is the only source of entitlement: no claim
//!    here rests on exclusive authority established by election.
//! 3. **Currency.** It is within its validity window, and not superseded: the reader retains the
//!    highest epoch it has verified per scope. A lower epoch is [`GrantVerdict::Superseded`]. Two
//!    *different* grants at the same scope and epoch are [`GrantVerdict::ConflictingAppointments`]:
//!    neither backs anything exclusive, and both are reported.
//! 4. **Possession.** The presenter proves it is the holder: the holder's signature over the digest
//!    of this grant and the specific request it is presented with ([`possession_message`]). A grant
//!    presented by anyone else is [`GrantVerdict::WrongHolder`].
//!
//! # What this does not do
//!
//! It does not enforce anything at a resource: the in-store fence remains the enforcing check for
//! mandated writes. A verified grant is **evidence of an appointment**, not a bearer token, and it
//! carries no transferable credential (threat model §6). The retained epochs are in memory; a
//! reader that restarts must see grants again before it knows what supersedes what.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Mandate, PrincipalId};
use crate::knowledge::issuer::{
    verify_signed_by, Authenticity, MemberKeySource, TrustedExternalIssuers, UnverifiableReason,
};
use crate::knowledge::IssuerId;

impl Mandate {
    /// The exact bytes an establishing authority signs to grant this mandate. Tagged, so a grant
    /// signature can never authenticate a knowledge record, a head or a cohort declaration.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn lp(out: &mut Vec<u8>, b: &[u8]) {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        let mut out = Vec::new();
        lp(&mut out, b"mycelium.mandate/grant/1");
        lp(&mut out, self.holder.as_str().as_bytes());
        lp(&mut out, self.established_by.as_str().as_bytes());
        lp(&mut out, self.purpose.as_bytes());
        lp(&mut out, self.scope.as_bytes());
        out.extend_from_slice(&(self.operations.len() as u32).to_le_bytes());
        for op in &self.operations {
            lp(&mut out, op.as_bytes());
        }
        out.extend_from_slice(&self.epoch.to_le_bytes());
        lp(&mut out, self.term.as_str().as_bytes());
        out.extend_from_slice(&self.valid_from_ms.to_le_bytes());
        out.extend_from_slice(&self.valid_until_ms.to_le_bytes());
        out
    }

    /// SHA-256 over [`canonical_bytes`](Self::canonical_bytes).
    pub fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.canonical_bytes()).into()
    }
}

/// A mandate together with its establishing authority's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedMandateGrant {
    /// The appointment.
    pub mandate: Mandate,
    /// `established_by`'s signature over [`Mandate::canonical_bytes`].
    pub signature: Vec<u8>,
}

/// The message a holder signs to prove possession of `grant` for one specific `request`.
///
/// Binding the request stops a proof made for one presentation being replayed for another.
pub fn possession_message(grant: &Mandate, request: &[u8]) -> Vec<u8> {
    let mut out = b"mycelium.mandate/possession/1".to_vec();
    out.extend_from_slice(&grant.digest());
    out.extend_from_slice(&(request.len() as u32).to_le_bytes());
    out.extend_from_slice(request);
    out
}

/// Which authorities a reader accepts as entitled to grant each scope. **Configured**, never
/// inferred: a signature proves who issued a grant, and this table decides whether they may.
#[derive(Clone, Debug, Default)]
pub struct EntitlementTable {
    scopes: BTreeMap<String, BTreeSet<PrincipalId>>,
}

impl EntitlementTable {
    /// No entitlements.
    pub fn new() -> Self {
        Self::default()
    }

    /// Entitle `authority` to grant mandates for `scope`.
    pub fn entitle(&mut self, scope: impl Into<String>, authority: PrincipalId) {
        self.scopes.entry(scope.into()).or_default().insert(authority);
    }

    /// Is `authority` entitled for `scope`?
    pub fn is_entitled(&self, scope: &str, authority: &PrincipalId) -> bool {
        self.scopes.get(scope).is_some_and(|a| a.contains(authority))
    }
}

/// What a reader concluded about a presented grant.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantVerdict {
    /// Issued, entitled, current, and presented by its holder.
    Valid,
    /// The signature does not verify for `established_by` through either admissible path.
    Unverifiable(UnverifiableReason),
    /// Signed under a key its issuer has revoked.
    SignedUnderRevokedKey,
    /// Authentic, but the signer is not entitled for this scope in the reader's table.
    NotEntitled,
    /// Outside its validity window.
    OutOfWindow,
    /// A higher epoch for this scope has been verified.
    Superseded {
        /// The highest epoch this reader holds for the scope.
        held: u64,
    },
    /// Another, different grant for the same scope and epoch has been verified. Neither backs
    /// anything exclusive.
    ConflictingAppointments,
    /// No proof of possession was presented, or it is not the holder's.
    WrongHolder,
}

impl GrantVerdict {
    /// Does this grant back the presenter's authority now?
    pub fn is_valid(&self) -> bool {
        matches!(self, GrantVerdict::Valid)
    }
}

/// A reader's grant checks: its entitlement table, and the highest epoch it has verified per scope.
#[derive(Clone, Debug, Default)]
pub struct GrantVerifier {
    entitlements: EntitlementTable,
    /// scope → (highest verified epoch, the digests verified at that epoch).
    highest: BTreeMap<String, (u64, BTreeSet<[u8; 32]>)>,
}

impl GrantVerifier {
    /// A verifier over `entitlements`.
    pub fn new(entitlements: EntitlementTable) -> Self {
        Self { entitlements, highest: BTreeMap::new() }
    }

    /// **Check a presented grant** against the reader's key view, entitlement table and retained
    /// epochs, at `now_ms`. `possession` is the presenter's signature over
    /// [`possession_message`]`(grant, request)`.
    ///
    /// The order is the contract: issuance, entitlement, window, currency, possession. A grant that
    /// is authentic and entitled is recorded against its scope's epoch even if it is then found
    /// superseded or presented by the wrong holder, because it is still evidence of what the
    /// authority appointed.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &mut self,
        grant: &SignedMandateGrant,
        request: &[u8],
        possession: Option<&[u8]>,
        now_ms: u64,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> GrantVerdict {
        let m = &grant.mandate;
        let Some(authority) = IssuerId::new(m.established_by.as_str()) else {
            return GrantVerdict::Unverifiable(UnverifiableReason::UntrustedExternal);
        };
        match verify_signed_by(&authority, &m.canonical_bytes(), &grant.signature, members, external) {
            Authenticity::Current { .. } => {}
            Authenticity::Revoked { .. } => return GrantVerdict::SignedUnderRevokedKey,
            Authenticity::Unverifiable(r) => return GrantVerdict::Unverifiable(r),
        }
        if !self.entitlements.is_entitled(&m.scope, &m.established_by) {
            return GrantVerdict::NotEntitled;
        }
        if !m.is_current(now_ms) {
            return GrantVerdict::OutOfWindow;
        }

        let digest = m.digest();
        let entry = self.highest.entry(m.scope.clone()).or_insert((m.epoch, BTreeSet::new()));
        if m.epoch > entry.0 {
            *entry = (m.epoch, BTreeSet::new());
        }
        if m.epoch < entry.0 {
            return GrantVerdict::Superseded { held: entry.0 };
        }
        entry.1.insert(digest);
        if entry.1.len() > 1 {
            return GrantVerdict::ConflictingAppointments;
        }

        let Some(proof) = possession else { return GrantVerdict::WrongHolder };
        let Some(holder) = IssuerId::new(m.holder.as_str()) else { return GrantVerdict::WrongHolder };
        match verify_signed_by(&holder, &possession_message(m, request), proof, members, external) {
            Authenticity::Current { .. } => GrantVerdict::Valid,
            _ => GrantVerdict::WrongHolder,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use crate::mandate::TermId;
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

    pub(crate) struct World {
        pub(crate) authority: SigningKey,
        pub(crate) holder: SigningKey,
        pub(crate) holder_node: NodeId,
        pub(crate) members: HashMap<NodeId, MemberKeys>,
        pub(crate) external: TrustedExternalIssuers,
    }

    pub(crate) fn world() -> World {
        let authority = SigningKey::from_bytes(&[31u8; 32]);
        let holder = SigningKey::from_bytes(&[32u8; 32]);
        let holder_node = NodeId::new("127.0.0.1", 7501).unwrap();
        let members = HashMap::from([(
            holder_node.clone(),
            MemberKeys { retained: vec![holder.verifying_key().to_bytes()], ..Default::default() },
        )]);
        let mut external = TrustedExternalIssuers::new();
        external.trust(IssuerId::new("operator:acme").unwrap(), authority.verifying_key().to_bytes()).unwrap();
        World { authority, holder, holder_node, members, external }
    }

    pub(crate) fn mandate(w: &World, scope: &str, ops: &[&str], epoch: u64) -> Mandate {
        Mandate {
            holder: PrincipalId::new(IssuerId::for_node(&w.holder_node).as_str()).unwrap(),
            established_by: PrincipalId::new("operator:acme").unwrap(),
            purpose: "serve the fleet".into(),
            scope: scope.into(),
            operations: ops.iter().map(|o| o.to_string()).collect(),
            epoch,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: 10_000,
        }
    }

    pub(crate) fn grant(w: &World, m: Mandate) -> SignedMandateGrant {
        let signature = mycelium_core::tls::sign_bytes(&w.authority, &m.canonical_bytes()).to_vec();
        SignedMandateGrant { mandate: m, signature }
    }

    pub(crate) fn possess(key: &SigningKey, m: &Mandate, request: &[u8]) -> Vec<u8> {
        mycelium_core::tls::sign_bytes(key, &possession_message(m, request)).to_vec()
    }

    pub(crate) fn verifier(scope: &str) -> GrantVerifier {
        let mut t = EntitlementTable::new();
        t.entitle(scope, PrincipalId::new("operator:acme").unwrap());
        GrantVerifier::new(t)
    }

    #[test]
    fn a_grant_issued_entitled_current_and_possessed_is_valid() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:tools/search"], 1));
        let p = possess(&w.holder, &g.mandate, b"req");
        assert_eq!(verifier("cap/fleet").check(&g, b"req", Some(&p), 1_000, &w.members, &w.external), GrantVerdict::Valid);
    }

    /// An altered operation list no longer verifies: the signature covers the enumerated operations.
    #[test]
    fn an_altered_operation_list_fails() {
        let w = world();
        let mut g = grant(&w, mandate(&w, "cap/fleet", &["serve:tools/search"], 1));
        g.mandate.operations.push("serve:tools/admin".into());
        let p = possess(&w.holder, &g.mandate, b"req");
        assert_eq!(
            verifier("cap/fleet").check(&g, b"req", Some(&p), 1_000, &w.members, &w.external),
            GrantVerdict::Unverifiable(UnverifiableReason::BadSignature)
        );
    }

    #[test]
    fn an_expired_grant_fails() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 1));
        let p = possess(&w.holder, &g.mandate, b"req");
        assert_eq!(verifier("cap/fleet").check(&g, b"req", Some(&p), 20_000, &w.members, &w.external), GrantVerdict::OutOfWindow);
    }

    /// **A signature proves who issued it, not that they may.** An authentic grant from an authority
    /// not entitled for the scope is refused.
    #[test]
    fn an_authentic_grant_from_a_non_entitled_authority_fails() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/other-scope", &["serve:x/y"], 1));
        let p = possess(&w.holder, &g.mandate, b"req");
        assert_eq!(verifier("cap/fleet").check(&g, b"req", Some(&p), 1_000, &w.members, &w.external), GrantVerdict::NotEntitled);
    }

    /// A grant presented by someone other than its holder is refused, and a possession proof made for
    /// one request does not carry to another.
    #[test]
    fn a_grant_presented_by_the_wrong_holder_fails() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 1));
        let impostor = SigningKey::from_bytes(&[99u8; 32]);
        let mut v = verifier("cap/fleet");
        assert_eq!(v.check(&g, b"req", Some(&possess(&impostor, &g.mandate, b"req")), 1_000, &w.members, &w.external), GrantVerdict::WrongHolder);
        assert_eq!(v.check(&g, b"req", None, 1_000, &w.members, &w.external), GrantVerdict::WrongHolder);
        let other_request = possess(&w.holder, &g.mandate, b"a different request");
        assert_eq!(v.check(&g, b"req", Some(&other_request), 1_000, &w.members, &w.external), GrantVerdict::WrongHolder);
    }

    #[test]
    fn a_superseded_grant_fails() {
        let w = world();
        let mut v = verifier("cap/fleet");
        let newer = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 2));
        let older = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 1));
        assert!(v.check(&newer, b"r", Some(&possess(&w.holder, &newer.mandate, b"r")), 1_000, &w.members, &w.external).is_valid());
        assert_eq!(
            v.check(&older, b"r", Some(&possess(&w.holder, &older.mandate, b"r")), 1_000, &w.members, &w.external),
            GrantVerdict::Superseded { held: 2 }
        );
    }

    /// Two different grants at the same scope and epoch: neither backs anything, and it is reported.
    #[test]
    fn conflicting_equal_epoch_grants_back_nothing() {
        let w = world();
        let mut v = verifier("cap/fleet");
        let a = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 3));
        let b = grant(&w, mandate(&w, "cap/fleet", &["serve:x/z"], 3));
        assert!(v.check(&a, b"r", Some(&possess(&w.holder, &a.mandate, b"r")), 1_000, &w.members, &w.external).is_valid());
        assert_eq!(
            v.check(&b, b"r", Some(&possess(&w.holder, &b.mandate, b"r")), 1_000, &w.members, &w.external),
            GrantVerdict::ConflictingAppointments
        );
    }

    /// A grant from a **member** authority whose signing key has since been validly revoked carries
    /// no present authority.
    #[test]
    fn a_grant_signed_under_a_revoked_member_key_fails() {
        let w = world();
        let auth_key = SigningKey::from_bytes(&[34u8; 32]);
        let auth_node = NodeId::new("127.0.0.1", 7502).unwrap();
        let mut members = w.members.clone();
        let pk = auth_key.verifying_key().to_bytes();
        members.insert(auth_node.clone(), MemberKeys { retained: vec![pk], revoked: [pk].into_iter().collect() });
        let mut m = mandate(&w, "cap/fleet", &["serve:x/y"], 1);
        m.established_by = PrincipalId::new(IssuerId::for_node(&auth_node).as_str()).unwrap();
        let g = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&auth_key, &m.canonical_bytes()).to_vec(), mandate: m };
        let mut t = EntitlementTable::new();
        t.entitle("cap/fleet", g.mandate.established_by.clone());
        assert_eq!(
            GrantVerifier::new(t).check(&g, b"r", Some(&possess(&w.holder, &g.mandate, b"r")), 1_000, &members, &w.external),
            GrantVerdict::SignedUnderRevokedKey
        );
    }

    #[test]
    fn a_grant_signed_under_a_key_the_reader_does_not_hold_fails() {
        let mut w = world();
        let mut ext = TrustedExternalIssuers::new();
        let other = SigningKey::from_bytes(&[33u8; 32]);
        ext.trust(IssuerId::new("operator:acme").unwrap(), other.verifying_key().to_bytes()).unwrap();
        w.external = ext;
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:x/y"], 1));
        assert_eq!(
            verifier("cap/fleet").check(&g, b"r", Some(&possess(&w.holder, &g.mandate, b"r")), 1_000, &w.members, &w.external),
            GrantVerdict::Unverifiable(UnverifiableReason::BadSignature),
            "a key the reader does not hold for the authority does not verify"
        );
    }
}
