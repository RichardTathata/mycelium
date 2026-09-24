//! **Issuer binding** (Boundary H plan item P1) — [`docs/design/knowledge-issuer-binding.md`](../../../docs/design/knowledge-issuer-binding.md).
//!
//! # The gap this closes
//!
//! An [`IssuerId`] was an opaque string, and [`KnowledgeRecord::verify`] took the key from its
//! caller. Nothing tied an issuer to anyone, so one admitted member could be as many issuers as it
//! cared to name, and every count the resolver makes (support, independence, challenge) could be
//! inflated from inside a single member.
//!
//! # Two admissible verification paths, and nothing else
//!
//! - **Member path.** The issuer is [`IssuerId::for_node`]: `node:{node_id}`. The key comes from the
//!   *reader's* view of that node's identity — every key it has retained for the node — never from
//!   the record or its presenter.
//! - **Configured-external path.** The issuer is one the reader has listed in
//!   [`TrustedExternalIssuers`] with its key or keys: an operator, an auditor, an outside observer.
//!   Trusted by configuration, never by default, and never inside the member namespace.
//!
//! Anything else is [`Authenticity::Unverifiable`], with a reason. It is never a silent drop and
//! never a pass.
//!
//! # Authentic is not the same as current
//!
//! A signature made under a key that the issuer has since validly revoked still says *who wrote
//! this, then* — attribution outlives authority (threat model posture rule 5). It no longer says the
//! issuer stands behind it *now*. So a match under a revoked key is [`Authenticity::Revoked`], not
//! [`Authenticity::Current`] and not a failure: historical attribution holds, present eligibility
//! does not. Deciding present eligibility is the resolver's job (plan item K1b); this module reports
//! the distinction so that decision is possible.
//!
//! # What this does not do
//!
//! It does not decide whether a record is *true*, whether its issuer was *entitled* to say it, or
//! whether the reader should *count* it. It answers one question: *who, verifiably, signed these
//! bytes, and does this reader still trust that key?*

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::node_id::NodeId;

use super::{IssuerId, KnowledgeRecord};

/// The prefix that marks an issuer as an admitted member. Reserved: an external issuer can never
/// be configured under it, so the two paths cannot be confused.
pub const MEMBER_ISSUER_PREFIX: &str = "node:";

impl IssuerId {
    /// The canonical issuer for an admitted member: `node:{node_id}`.
    ///
    /// One issuer per member (plan item P1). A member may keep many streams under it, but it cannot
    /// be several issuers on the member path, because the key that verifies this issuer is the
    /// member's own identity key and nothing else.
    pub fn for_node(node: &NodeId) -> Self {
        Self(std::sync::Arc::from(format!("{MEMBER_ISSUER_PREFIX}{node}").as_str()))
    }

    /// If this issuer is on the member path, which member.
    ///
    /// `None` both for an issuer outside the member namespace and for one inside it that does not
    /// parse as a node id — see [`IssuerId::claims_member_namespace`] to tell those apart.
    pub fn member_node(&self) -> Option<NodeId> {
        self.as_str().strip_prefix(MEMBER_ISSUER_PREFIX)?.parse().ok()
    }

    /// Does this issuer claim the reserved member namespace, parseable or not?
    pub fn claims_member_namespace(&self) -> bool {
        self.as_str().starts_with(MEMBER_ISSUER_PREFIX)
    }
}

/// Which of the two admissible paths verified a record.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuerPath {
    /// An admitted member, verified against the reader's view of its identity keys.
    Member(NodeId),
    /// An issuer the reader configured, verified against the configured keys.
    External,
}

/// Why a record could not be attributed.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnverifiableReason {
    /// A member issuer this reader holds no identity keys for — not yet learned, or never admitted.
    UnknownMember(NodeId),
    /// The issuer claims the member namespace but does not name a parseable node.
    MalformedMemberIssuer,
    /// An issuer outside the member namespace that this reader has not configured.
    UntrustedExternal,
    /// The issuer is known, but the signature verifies under none of its keys.
    BadSignature,
}

/// What a reader can say about who signed a record. **Three outcomes, and the middle one is the
/// point.**
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authenticity {
    /// Signed under a key this reader currently trusts for the issuer.
    Current {
        /// Which path verified it.
        path: IssuerPath,
        /// The key that verified it.
        key: [u8; 32],
    },
    /// Signed under a key the issuer held and has since **validly revoked**. The record is
    /// authentic as history — this issuer wrote it — but carries no present authority.
    Revoked {
        /// Which path verified it.
        path: IssuerPath,
        /// The revoked key that verified it.
        key: [u8; 32],
    },
    /// Not attributable. Never a silent drop: the reason says why.
    Unverifiable(UnverifiableReason),
}

impl Authenticity {
    /// Signed under a currently trusted key.
    pub fn is_current(&self) -> bool {
        matches!(self, Authenticity::Current { .. })
    }

    /// Authentic as history: signed by this issuer, whether or not the key is still trusted.
    pub fn is_attributable(&self) -> bool {
        matches!(self, Authenticity::Current { .. } | Authenticity::Revoked { .. })
    }
}

/// One member's keys, as a reader currently holds them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberKeys {
    /// Every key the reader has retained for the member — current and historical. Retained keys
    /// are what let a signature made before a routine rotation still verify (threat model §6:
    /// *rotation is not revocation*).
    pub retained: Vec<[u8; 32]>,
    /// The subset of those the reader has seen **validly** revoked.
    pub revoked: HashSet<[u8; 32]>,
}

/// Where a reader's view of member identity keys comes from.
///
/// A live node supplies one with `GossipAgent::knowledge_member_keys` (the `compliance` feature,
/// because revocations live there) — a snapshot map, which implements this trait. Tests use a fixed
/// map the same way.
pub trait MemberKeySource {
    /// The reader's current view of `node`'s keys. Empty `retained` means *this reader does not
    /// know this member*, which verification reports as [`UnverifiableReason::UnknownMember`].
    fn member_keys(&self, node: &NodeId) -> MemberKeys;
}

impl MemberKeySource for HashMap<NodeId, MemberKeys> {
    fn member_keys(&self, node: &NodeId) -> MemberKeys {
        self.get(node).cloned().unwrap_or_default()
    }
}

/// Why a node refused to sign a knowledge record.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignAsMemberError {
    /// The node has no `tls` identity to sign with.
    NoIdentity,
    /// The record's issuer is not this node's member issuer. A node signs **only as itself**: its
    /// signing API never produces a signature for any other issuer.
    NotThisMember {
        /// The issuer the record names.
        issuer: IssuerId,
    },
}

impl std::fmt::Display for SignAsMemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoIdentity => write!(f, "this node has no tls identity to sign with"),
            Self::NotThisMember { issuer } => {
                write!(f, "this node signs knowledge records only as itself, not as {issuer}")
            }
        }
    }
}

impl std::error::Error for SignAsMemberError {}

/// Why an external issuer could not be configured.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalIssuerError {
    /// The name is inside the reserved member namespace. An operator cannot configure a key for
    /// `node:…`, because that would let configuration stand in for a member's own identity.
    MemberNamespace,
}

impl std::fmt::Display for ExternalIssuerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemberNamespace => write!(
                f,
                "an external issuer cannot be configured under the reserved member namespace \
                 `{MEMBER_ISSUER_PREFIX}`"
            ),
        }
    }
}

impl std::error::Error for ExternalIssuerError {}

/// The issuers a reader trusts **by configuration** — operators, auditors, outside observers.
///
/// Empty by default: nothing outside the member path is trusted until a reader says so.
#[derive(Clone, Debug, Default)]
pub struct TrustedExternalIssuers {
    keys: BTreeMap<IssuerId, Vec<[u8; 32]>>,
}

impl TrustedExternalIssuers {
    /// No external issuers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust `key` for `issuer`. An issuer may have several keys. Refuses the member namespace.
    pub fn trust(&mut self, issuer: IssuerId, key: [u8; 32]) -> Result<(), ExternalIssuerError> {
        if issuer.claims_member_namespace() {
            return Err(ExternalIssuerError::MemberNamespace);
        }
        let keys = self.keys.entry(issuer).or_default();
        if !keys.contains(&key) {
            keys.push(key);
        }
        Ok(())
    }

    /// Stop trusting `issuer` altogether. For an external issuer, configuration *is* its trust, so
    /// withdrawing it is removal rather than revocation.
    pub fn untrust(&mut self, issuer: &IssuerId) {
        self.keys.remove(issuer);
    }

    fn keys_for(&self, issuer: &IssuerId) -> Option<&[[u8; 32]]> {
        self.keys.get(issuer).map(Vec::as_slice)
    }
}

/// **Who, verifiably, signed this record — and does this reader still trust that key?**
///
/// Takes the key from the reader's own view (`members`, `external`), never from the record or from
/// whoever presents it. See the module docs for the two paths and the three outcomes.
pub fn verify_issuer(
    record: &KnowledgeRecord,
    signature: &[u8],
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Authenticity {
    verify_signed_by(record.issuer(), &record.canonical_bytes(), signature, members, external)
}

/// **The same two paths, for any bytes an issuer signs** — a record, a stream head (Boundary H
/// item K2), anything whose canonical bytes carry their own domain-separation tag.
///
/// The caller supplies the canonical bytes; the key always comes from the reader's view.
pub fn verify_signed_by(
    issuer: &IssuerId,
    canonical_bytes: &[u8],
    signature: &[u8],
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Authenticity {
    let verifies = |key: &[u8; 32]| mycelium_core::tls::verify_bytes(key, canonical_bytes, signature);

    if issuer.claims_member_namespace() {
        let Some(node) = issuer.member_node() else {
            return Authenticity::Unverifiable(UnverifiableReason::MalformedMemberIssuer);
        };
        let keys = members.member_keys(&node);
        if keys.retained.is_empty() {
            return Authenticity::Unverifiable(UnverifiableReason::UnknownMember(node));
        }
        // A key both current and revoked cannot happen; prefer reporting a current match if the
        // retained list somehow holds duplicates.
        let mut revoked_match = None;
        for key in &keys.retained {
            if verifies(key) {
                if keys.revoked.contains(key) {
                    revoked_match = Some(*key);
                } else {
                    return Authenticity::Current { path: IssuerPath::Member(node), key: *key };
                }
            }
        }
        return match revoked_match {
            Some(key) => Authenticity::Revoked { path: IssuerPath::Member(node), key },
            None => Authenticity::Unverifiable(UnverifiableReason::BadSignature),
        };
    }

    let Some(keys) = external.keys_for(issuer) else {
        return Authenticity::Unverifiable(UnverifiableReason::UntrustedExternal);
    };
    for key in keys {
        if verifies(key) {
            return Authenticity::Current { path: IssuerPath::External, key: *key };
        }
    }
    Authenticity::Unverifiable(UnverifiableReason::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::RecordKind;
    use ed25519_dalek::SigningKey;

    fn keypair(seed: u8) -> (SigningKey, [u8; 32]) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    fn node(port: u16) -> NodeId {
        NodeId::new("127.0.0.1", port).expect("valid node id")
    }

    fn record(issuer: IssuerId) -> KnowledgeRecord {
        KnowledgeRecord::new(issuer, RecordKind::Assessment, 1_000, "release/x", b"j".to_vec(), vec![])
            .expect("well formed")
    }

    fn sign(sk: &SigningKey, r: &KnowledgeRecord) -> Vec<u8> {
        mycelium_core::tls::sign_bytes(sk, &r.canonical_bytes()).to_vec()
    }

    /// `(member, retained keys, revoked keys)`.
    type MemberEntry = (NodeId, Vec<[u8; 32]>, Vec<[u8; 32]>);

    fn members(entries: Vec<MemberEntry>) -> HashMap<NodeId, MemberKeys> {
        entries
            .into_iter()
            .map(|(n, retained, revoked)| {
                (n, MemberKeys { retained, revoked: revoked.into_iter().collect() })
            })
            .collect()
    }

    #[test]
    fn a_member_issuer_round_trips_through_its_node_id() {
        let n = node(7001);
        let issuer = IssuerId::for_node(&n);
        assert_eq!(issuer.as_str(), format!("node:{n}"));
        assert_eq!(issuer.member_node(), Some(n));
        assert!(IssuerId::new("lab-a").unwrap().member_node().is_none());
    }

    /// The member path: a member's record, signed with its own identity key, verifies as current.
    #[test]
    fn a_member_signing_as_itself_verifies_as_current() {
        let (sk, pk) = keypair(1);
        let a = node(7001);
        let r = record(IssuerId::for_node(&a));
        let v = verify_issuer(&r, &sign(&sk, &r), &members(vec![(a.clone(), vec![pk], vec![])]), &TrustedExternalIssuers::new());
        assert_eq!(v, Authenticity::Current { path: IssuerPath::Member(a), key: pk });
    }

    /// **The gap P1 closes.** Member A signs a record claiming member B's issuer. The reader checks
    /// it against *B's* keys, which A does not hold, so it fails — A cannot be B.
    #[test]
    fn one_member_cannot_sign_as_another() {
        let (sk_a, pk_a) = keypair(1);
        let (_sk_b, pk_b) = keypair(2);
        let (a, b) = (node(7001), node(7002));
        let r = record(IssuerId::for_node(&b));
        let view = members(vec![(a, vec![pk_a], vec![]), (b, vec![pk_b], vec![])]);
        assert_eq!(
            verify_issuer(&r, &sign(&sk_a, &r), &view, &TrustedExternalIssuers::new()),
            Authenticity::Unverifiable(UnverifiableReason::BadSignature)
        );
    }

    /// **One member cannot be many issuers.** An arbitrary name on the member path must name a node
    /// the reader knows; one outside it must be configured. A member inventing `sock-puppet-7`
    /// gets neither.
    #[test]
    fn an_invented_issuer_name_is_unverifiable_not_counted() {
        let (sk, pk) = keypair(1);
        let a = node(7001);
        let view = members(vec![(a, vec![pk], vec![])]);
        let invented = record(IssuerId::new("sock-puppet-7").unwrap());
        assert_eq!(
            verify_issuer(&invented, &sign(&sk, &invented), &view, &TrustedExternalIssuers::new()),
            Authenticity::Unverifiable(UnverifiableReason::UntrustedExternal)
        );
        let unknown_member = record(IssuerId::for_node(&node(7999)));
        assert_eq!(
            verify_issuer(&unknown_member, &sign(&sk, &unknown_member), &view, &TrustedExternalIssuers::new()),
            Authenticity::Unverifiable(UnverifiableReason::UnknownMember(node(7999)))
        );
        let malformed = record(IssuerId::new("node:not-a-node").unwrap());
        assert_eq!(
            verify_issuer(&malformed, &sign(&sk, &malformed), &view, &TrustedExternalIssuers::new()),
            Authenticity::Unverifiable(UnverifiableReason::MalformedMemberIssuer)
        );
    }

    /// The configured-external path: an operator or auditor the reader trusts verifies; the same
    /// issuer unconfigured does not.
    #[test]
    fn a_configured_external_issuer_verifies_and_an_unconfigured_one_does_not() {
        let (sk, pk) = keypair(9);
        let auditor = IssuerId::new("auditor:acme").unwrap();
        let r = record(auditor.clone());
        let sig = sign(&sk, &r);
        let no_members = members(vec![]);

        let mut trusted = TrustedExternalIssuers::new();
        assert_eq!(
            verify_issuer(&r, &sig, &no_members, &trusted),
            Authenticity::Unverifiable(UnverifiableReason::UntrustedExternal)
        );
        trusted.trust(auditor.clone(), pk).unwrap();
        assert_eq!(
            verify_issuer(&r, &sig, &no_members, &trusted),
            Authenticity::Current { path: IssuerPath::External, key: pk }
        );
        trusted.untrust(&auditor);
        assert!(!verify_issuer(&r, &sig, &no_members, &trusted).is_attributable());
    }

    /// Configuration cannot stand in for a member's identity: the member namespace is refused.
    #[test]
    fn an_external_issuer_cannot_be_configured_in_the_member_namespace() {
        let (_sk, pk) = keypair(9);
        let mut trusted = TrustedExternalIssuers::new();
        assert_eq!(
            trusted.trust(IssuerId::for_node(&node(7001)), pk),
            Err(ExternalIssuerError::MemberNamespace)
        );
        assert_eq!(
            trusted.trust(IssuerId::new("node:anything").unwrap(), pk),
            Err(ExternalIssuerError::MemberNamespace)
        );
    }

    /// **Authentic is not current.** A record signed under a key the member has since validly
    /// revoked keeps its attribution — this member wrote it — and loses its present standing.
    #[test]
    fn a_revoked_key_keeps_attribution_and_loses_present_standing() {
        let (old_sk, old_pk) = keypair(1);
        let (_new_sk, new_pk) = keypair(2);
        let a = node(7001);
        let r = record(IssuerId::for_node(&a));
        let view = members(vec![(a.clone(), vec![new_pk, old_pk], vec![old_pk])]);

        let v = verify_issuer(&r, &sign(&old_sk, &r), &view, &TrustedExternalIssuers::new());
        assert_eq!(v, Authenticity::Revoked { path: IssuerPath::Member(a), key: old_pk });
        assert!(v.is_attributable(), "history: this member wrote it");
        assert!(!v.is_current(), "present: it carries no authority now");
    }

    /// Rotation is not revocation: a signature under a retained, unrevoked older key is current.
    #[test]
    fn a_retained_rotated_key_still_verifies_as_current() {
        let (old_sk, old_pk) = keypair(1);
        let (_new_sk, new_pk) = keypair(2);
        let a = node(7001);
        let r = record(IssuerId::for_node(&a));
        let view = members(vec![(a.clone(), vec![new_pk, old_pk], vec![])]);
        assert_eq!(
            verify_issuer(&r, &sign(&old_sk, &r), &view, &TrustedExternalIssuers::new()),
            Authenticity::Current { path: IssuerPath::Member(a), key: old_pk }
        );
    }

    /// A tampered record fails under the right key — the signature covers the canonical bytes.
    #[test]
    fn a_signature_over_different_content_does_not_verify() {
        let (sk, pk) = keypair(1);
        let a = node(7001);
        let signed = record(IssuerId::for_node(&a));
        let other = KnowledgeRecord::new(
            IssuerId::for_node(&a),
            RecordKind::Assessment,
            2_000,
            "release/x",
            b"j".to_vec(),
            vec![],
        )
        .unwrap();
        let view = members(vec![(a, vec![pk], vec![])]);
        assert_eq!(
            verify_issuer(&other, &sign(&sk, &signed), &view, &TrustedExternalIssuers::new()),
            Authenticity::Unverifiable(UnverifiableReason::BadSignature)
        );
    }
}
