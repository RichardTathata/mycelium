//! Typed knowledge records (v3 contracts axis, item 3 PR 2).
//!
//! The record is [`docs/design/knowledge-layer.md`](../../docs/design/knowledge-layer.md). This is
//! its first code: the four record types, the six link kinds, and the identity scheme that makes
//! the layer's central rule checkable.
//!
//! # Four types, because judging is not recording
//!
//! `Claim` · `Observation` · **`Assessment`** · `AcceptanceDecision`. The split that matters is
//! assessment from observation: collapsing them is how *"three nodes reported an error"* silently
//! becomes *"the service is broken"*, with nobody having said so and nobody accountable for the
//! inference.
//!
//! # Immutability is structural, not a convention
//!
//! A record's [`RecordId`] is **derived from its content**. Change any field and you have a
//! different id — so *"there is no edit, only a further record"* is not a rule someone must
//! remember, it is arithmetic. There are no setters and no `&mut` accessors here.
//!
//! # Why an id names its issuer
//!
//! The layer's sharpest rule is that **an issuer retracts only its own statements** — retraction is
//! not moderation, and a record that could retract another issuer's statement would make this a
//! consensus mechanism over truth, which the record refuses.
//!
//! Enforcing that needs to know who issued the *target*. If the id were a bare digest, a reader
//! would have to fetch the target to find out, which means the rule could be skipped by any reader
//! that had not. So an id is `(issuer, digest)`: the check is **local**, from the retracting record
//! alone, and cannot be bypassed by not looking.
//!
//! # What is not here
//!
//! No store, no resolution, no gossip. `knowledge/head/{issuer}/{stream}` is reserved
//! (`kv_ns::KNOWLEDGE_HEAD`) and carries **bounded signed heads only** — pointers, never records —
//! because LWW may move a pointer, and moving a pointer cannot erase a competing statement.

pub mod correction;
pub mod durable;
pub mod gate;
pub mod heads;
pub mod issuer;
pub mod resolution;
pub mod store;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Who issued a record. Opaque: an issuer's internal naming is its own.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IssuerId(Arc<str>);

impl IssuerId {
    /// Wrap an issuer name. Empty is refused — an unattributed record is not a record.
    pub fn new(s: impl AsRef<str>) -> Option<Self> {
        let s = s.as_ref();
        (!s.is_empty()).then(|| Self(Arc::from(s)))
    }

    /// The issuer as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IssuerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a record is.
///
/// Part of the signed bytes, so an observation's signature can never authenticate an assessment —
/// the same domain-separation rule the federation objects use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordKind {
    /// An issuer asserts something.
    Claim,
    /// An issuer reports what it saw.
    Observation,
    /// An issuer **judges** a claim or observation. Not a report — a judgement, with an author.
    Assessment,
    /// A reader decides what to do about something.
    AcceptanceDecision,
}

impl RecordKind {
    /// The tag that goes into the signed bytes.
    fn tag(self) -> &'static str {
        match self {
            Self::Claim => "mycelium.knowledge/claim/1",
            Self::Observation => "mycelium.knowledge/observation/1",
            Self::Assessment => "mycelium.knowledge/assessment/1",
            Self::AcceptanceDecision => "mycelium.knowledge/acceptance/1",
        }
    }
}

/// How one record relates to another. Exactly six, and no others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    /// This record supports the target.
    Supports,
    /// This record disputes the target. **Crossing issuers is the point** — disagreement is what
    /// the layer preserves.
    Challenges,
    /// This record was derived from the target. Asserted explicitly; never inferred from timestamp
    /// adjacency, which is an ordering and not a cause.
    DerivedFrom,
    /// This record replaces the target.
    Supersedes,
    /// This record withdraws the target. **Same issuer only** — see the module docs.
    Retracts,
    /// This record adopts the target as a basis for action.
    Adopts,
}

impl LinkKind {
    fn code(self) -> u8 {
        match self {
            Self::Supports => 1,
            Self::Challenges => 2,
            Self::DerivedFrom => 3,
            Self::Supersedes => 4,
            Self::Retracts => 5,
            Self::Adopts => 6,
        }
    }
}

/// A content-derived identity that **names its issuer**.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RecordId {
    /// Who issued it. Present so `Retracts` is checkable without fetching the target.
    pub issuer: IssuerId,
    /// SHA-256 over the record's canonical bytes.
    pub digest: [u8; 32],
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/", self.issuer)?;
        for b in &self.digest[..8] {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// One typed link to another record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// What kind of relation.
    pub kind: LinkKind,
    /// Which record.
    pub target: RecordId,
}

/// Why a record is not well formed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    /// A `Retracts` link targets another issuer's record.
    ///
    /// The single most important refusal in this module: retraction is not moderation.
    ForeignRetraction {
        /// Who tried.
        by: IssuerId,
        /// Whose record they tried to withdraw.
        target_issuer: IssuerId,
    },
    /// A record links to itself, which no relation can sensibly mean.
    SelfLink,
    /// The subject is empty — a record about nothing.
    EmptySubject,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignRetraction { by, target_issuer } => write!(
                f,
                "{by} cannot retract a record issued by {target_issuer}: an issuer retracts only \
                 its own statements"
            ),
            Self::SelfLink => write!(f, "a record cannot link to itself"),
            Self::EmptySubject => write!(f, "a record must have a subject"),
        }
    }
}

impl std::error::Error for RecordError {}

/// An immutable, attributable statement.
///
/// Construct with [`KnowledgeRecord::new`], which validates and derives the id. Fields are readable
/// and there is no way to change one — mutating a record would mean recomputing its identity, which
/// is the same as making a different record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRecord {
    id: RecordId,
    issuer: IssuerId,
    kind: RecordKind,
    at_ms: u64,
    subject: String,
    body: Vec<u8>,
    links: Vec<Link>,
}

fn put_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl KnowledgeRecord {
    /// Build and validate a record, deriving its id from its content.
    ///
    /// Refuses a foreign retraction, a self-link, and an empty subject. Everything else is the
    /// issuer's business — this module decides *well-formedness*, never truth.
    #[cfg(feature = "tls")]
    pub fn new(
        issuer: IssuerId,
        kind: RecordKind,
        at_ms: u64,
        subject: impl Into<String>,
        body: Vec<u8>,
        links: Vec<Link>,
    ) -> Result<Self, RecordError> {
        let subject = subject.into();
        if subject.is_empty() {
            return Err(RecordError::EmptySubject);
        }
        for l in &links {
            if l.kind == LinkKind::Retracts && l.target.issuer != issuer {
                return Err(RecordError::ForeignRetraction {
                    by: issuer.clone(),
                    target_issuer: l.target.issuer.clone(),
                });
            }
        }

        let mut record = Self {
            // Placeholder; replaced once the bytes it hashes over are complete.
            id: RecordId { issuer: issuer.clone(), digest: [0u8; 32] },
            issuer,
            kind,
            at_ms,
            subject,
            body,
            links,
        };
        record.id = RecordId {
            issuer: record.issuer.clone(),
            digest: record.content_digest(),
        };

        // Checked after the id exists, because a self-link can only be spotted once it does — and
        // a record whose own digest appears in its links could not have been built honestly anyway.
        if record.links.iter().any(|l| l.target == record.id) {
            return Err(RecordError::SelfLink);
        }
        Ok(record)
    }

    /// Its content-derived identity.
    pub fn id(&self) -> &RecordId {
        &self.id
    }
    /// Who issued it.
    pub fn issuer(&self) -> &IssuerId {
        &self.issuer
    }
    /// What kind of record it is.
    pub fn kind(&self) -> RecordKind {
        self.kind
    }
    /// When the issuer says it was made, in epoch milliseconds.
    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }
    /// What it is about.
    pub fn subject(&self) -> &str {
        &self.subject
    }
    /// Its payload, opaque to this module.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    /// Its links.
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// The exact bytes a signature is taken over, and which the id hashes.
    ///
    /// Begins with the **kind's** tag, so an observation's signature can never authenticate an
    /// assessment. Length-prefixed throughout for the same reason the federation objects are: two
    /// distinct records must not be able to encode identically.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_lp(&mut out, self.kind.tag().as_bytes());
        put_lp(&mut out, self.issuer.as_str().as_bytes());
        out.extend_from_slice(&self.at_ms.to_le_bytes());
        put_lp(&mut out, self.subject.as_bytes());
        put_lp(&mut out, &self.body);
        out.extend_from_slice(&(self.links.len() as u32).to_le_bytes());
        for l in &self.links {
            out.push(l.kind.code());
            put_lp(&mut out, l.target.issuer.as_str().as_bytes());
            out.extend_from_slice(&l.target.digest);
        }
        out
    }

    #[cfg(feature = "tls")]
    fn content_digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.canonical_bytes());
        h.finalize().into()
    }

    /// Does `signature` verify over this record under `key`?
    #[cfg(feature = "tls")]
    pub fn verify(&self, key: &[u8; 32], signature: &[u8]) -> bool {
        mycelium_core::tls::verify_bytes(key, &self.canonical_bytes(), signature)
    }
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use super::*;

    fn iss(s: &str) -> IssuerId {
        IssuerId::new(s).unwrap()
    }

    fn record(issuer: &str, kind: RecordKind, subject: &str) -> KnowledgeRecord {
        KnowledgeRecord::new(iss(issuer), kind, 1_789_000_000_000, subject, b"body".to_vec(), vec![])
            .expect("well formed")
    }

    // ── the rule the module exists to enforce ────────────────────────────────────────────────

    /// **An issuer retracts only its own statements**, and the check is *local* — no store lookup,
    /// so it cannot be skipped by a reader that has not fetched the target.
    #[test]
    fn an_issuer_cannot_retract_another_issuers_record() {
        let theirs = record("node-b", RecordKind::Claim, "svc/health");
        let attempt = KnowledgeRecord::new(
            iss("node-a"),
            RecordKind::Claim,
            1_789_000_000_001,
            "svc/health",
            b"withdrawn".to_vec(),
            vec![Link { kind: LinkKind::Retracts, target: theirs.id().clone() }],
        );
        assert_eq!(
            attempt,
            Err(RecordError::ForeignRetraction {
                by: iss("node-a"),
                target_issuer: iss("node-b"),
            }),
            "retraction is not moderation"
        );
    }

    #[test]
    fn an_issuer_may_retract_its_own_record() {
        let mine = record("node-a", RecordKind::Claim, "svc/health");
        KnowledgeRecord::new(
            iss("node-a"),
            RecordKind::Claim,
            1_789_000_000_001,
            "svc/health",
            b"withdrawn".to_vec(),
            vec![Link { kind: LinkKind::Retracts, target: mine.id().clone() }],
        )
        .expect("an issuer may withdraw its own statement");
    }

    /// Challenging **across** issuers is the point — disagreement is what the layer preserves, and
    /// only retraction is issuer-bound.
    #[test]
    fn challenging_another_issuer_is_allowed_and_is_the_whole_point() {
        let theirs = record("node-b", RecordKind::Observation, "svc/health");
        KnowledgeRecord::new(
            iss("node-a"),
            RecordKind::Observation,
            1_789_000_000_001,
            "svc/health",
            b"i saw otherwise".to_vec(),
            vec![Link { kind: LinkKind::Challenges, target: theirs.id().clone() }],
        )
        .expect("disagreement across issuers is permitted");
    }

    // ── identity and immutability ────────────────────────────────────────────────────────────

    /// **Immutability is arithmetic, not a convention.** Any content change is a different id, so
    /// "no edit, only a further record" cannot be violated by forgetting it.
    #[test]
    fn any_content_change_is_a_different_record() {
        let base = record("node-a", RecordKind::Claim, "svc/health");
        let later = KnowledgeRecord::new(
            iss("node-a"),
            RecordKind::Claim,
            base.at_ms() + 1,
            "svc/health",
            b"body".to_vec(),
            vec![],
        )
        .unwrap();
        assert_ne!(base.id(), later.id(), "a different timestamp is a different record");

        let other_body = KnowledgeRecord::new(
            iss("node-a"),
            RecordKind::Claim,
            base.at_ms(),
            "svc/health",
            b"different".to_vec(),
            vec![],
        )
        .unwrap();
        assert_ne!(base.id(), other_body.id(), "a different body is a different record");
    }

    /// **An observation and an assessment of the same thing are different records**, because the
    /// kind's tag is in the signed bytes. Otherwise a signature over "what I saw" would
    /// authenticate "what I concluded".
    #[test]
    fn an_observation_and_an_assessment_never_share_bytes_or_identity() {
        let observed = record("node-a", RecordKind::Observation, "svc/health");
        let assessed = record("node-a", RecordKind::Assessment, "svc/health");
        assert_ne!(observed.canonical_bytes(), assessed.canonical_bytes());
        assert_ne!(observed.id(), assessed.id(), "judging is not recording");
    }

    /// The id names its issuer, which is what makes the retraction check local.
    #[test]
    fn an_id_names_its_issuer() {
        let r = record("node-a", RecordKind::Claim, "svc/health");
        assert_eq!(r.id().issuer, iss("node-a"));
        assert!(format!("{}", r.id()).starts_with("node-a/"));
    }

    /// **Equivocation is preserved.** Two issuers contradicting each other produce two records that
    /// both exist; nothing here resolves them, and that is deliberate.
    #[test]
    fn two_issuers_disagreeing_produces_two_records_not_a_winner() {
        let a = KnowledgeRecord::new(
            iss("node-a"), RecordKind::Observation, 1_000, "svc/health", b"up".to_vec(), vec![],
        ).unwrap();
        let b = KnowledgeRecord::new(
            iss("node-b"), RecordKind::Observation, 1_000, "svc/health", b"down".to_vec(), vec![],
        ).unwrap();
        assert_ne!(a.id(), b.id());
        assert_eq!(a.subject(), b.subject(), "about the same thing, and both survive");
    }

    // ── well-formedness ─────────────────────────────────────────────────────────────────────

    #[test]
    fn a_record_must_have_a_subject_and_an_issuer() {
        assert_eq!(
            KnowledgeRecord::new(iss("node-a"), RecordKind::Claim, 1, "", vec![], vec![]),
            Err(RecordError::EmptySubject)
        );
        assert_eq!(IssuerId::new(""), None, "an unattributed record is not a record");
    }

    /// Length prefixes stop two different records encoding identically — the same argument as the
    /// federation objects, and it matters more here because a digest *is* the identity.
    #[test]
    fn the_encoding_is_unambiguous() {
        let a = KnowledgeRecord::new(
            iss("node-a"), RecordKind::Claim, 1, "ab", b"c".to_vec(), vec![],
        ).unwrap();
        let b = KnowledgeRecord::new(
            iss("node-a"), RecordKind::Claim, 1, "a", b"bc".to_vec(), vec![],
        ).unwrap();
        assert_ne!(a.canonical_bytes(), b.canonical_bytes());
        assert_ne!(a.id(), b.id(), "a concatenation-ambiguous encoding would collide identities");
    }

    // ── signing ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_signed_record_verifies_and_a_re_signed_variant_does_not_pass_as_it() {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[5u8; 32]);
        let pk = sk.verifying_key().to_bytes();

        let r = record("node-a", RecordKind::Assessment, "svc/health");
        let sig = mycelium_core::tls::sign_bytes(&sk, &r.canonical_bytes());
        assert!(r.verify(&pk, &sig));

        // Same issuer, same subject, different kind — the signature must not carry over.
        let other = record("node-a", RecordKind::Observation, "svc/health");
        assert!(
            !other.verify(&pk, &sig),
            "an assessment's signature must not authenticate an observation"
        );
    }
}
