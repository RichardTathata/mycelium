//! The authorized store, and the heads that point into it (item 3 PR 3).
//!
//! §2 of [`docs/design/knowledge-layer.md`](../../../docs/design/knowledge-layer.md) carries the
//! decision this module exists to make true:
//!
//! > **LWW moves a pointer, and moving a pointer cannot erase a competing statement.**
//!
//! The gossip KV namespace carries **bounded signed discovery heads** — `knowledge/head/{issuer}/
//! {stream}`, reserved as `kv_ns::KNOWLEDGE_HEAD`. The records themselves live here.
//!
//! # Why that split is the whole design
//!
//! Put records in KV and last-write-wins becomes **last-writer-is-right**: two issuers who disagree
//! resolve to whichever had the later HLC, and the losing statement is *gone*. That is a clock
//! race, not a truth procedure.
//!
//! With heads only, an advancing head moves a pointer and nothing else. Both statements survive, so
//! the layer can answer the question it exists to answer — *"these two issuers disagree"* — instead
//! of quietly reporting the later one. [`KnowledgeStore::about`] is that answer, and
//! `advancing_a_head_does_not_remove_what_it_pointed_at` is the test that keeps it true.
//!
//! # "Authorized" means two things, both checked here
//!
//! 1. **An issuer publishes only its own heads.** The same shape as retraction in the parent
//!    module: a head names its issuer, so the check is local and cannot be skipped by a reader that
//!    has not fetched anything.
//! 2. **A head only advances.** A replayed older head is refused rather than accepted as news —
//!    otherwise a stale copy re-delivered by anti-entropy would silently roll a stream backwards.
//!
//! # A head may point at a record we do not have
//!
//! That is normal, not an error: a head is a *discovery* pointer, and discovering something is how
//! you learn to go and fetch it. The store says so through [`KnowledgeStore::is_resolvable`] rather
//! than refusing the head — refusing would make discovery depend on having already discovered.

use super::issuer::{
    verify_issuer, Authenticity, IssuerPath, MemberKeySource, TrustedExternalIssuers,
    UnverifiableReason,
};
use super::{IssuerId, KnowledgeRecord, LinkKind, RecordError, RecordId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A bounded, signed pointer to an issuer's latest record in one stream.
///
/// This is the only knowledge-layer object that belongs in the gossip KV namespace, and it is a
/// pointer by construction — there is nowhere in it to put a statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// Whose stream.
    pub issuer: IssuerId,
    /// Which stream.
    pub stream: String,
    /// The record it points at.
    pub record: RecordId,
    /// Monotonic within `(issuer, stream)`. This is what makes "only advances" checkable without a
    /// clock — two heads from the same issuer are ordered by the issuer's own counter, not by
    /// whichever arrived later.
    pub seq: u64,
}

impl Head {
    /// The KV key this head would occupy: `knowledge/head/{issuer}/{stream}`.
    pub fn kv_key(&self) -> String {
        format!(
            "{}{}/{}",
            mycelium_core::signal::kv_ns::KNOWLEDGE_HEAD,
            self.issuer,
            self.stream
        )
    }
}

/// Why the store refused something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// A head was published by someone other than its issuer.
    ForeignHead {
        /// Who tried to publish it.
        by: IssuerId,
        /// Whose stream it claims to be.
        issuer: IssuerId,
    },
    /// A head did not advance. Carries both sequence numbers, because *"we already had a newer
    /// one"* and *"this is a duplicate"* look identical without them.
    StaleHead {
        /// What the store holds.
        held: u64,
        /// What was offered.
        offered: u64,
    },
    /// The head's record belongs to a different issuer than the head's stream.
    MismatchedRecord {
        /// The stream's issuer.
        issuer: IssuerId,
        /// The record's issuer.
        record_issuer: IssuerId,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignHead { by, issuer } =>
                write!(f, "{by} cannot publish a head for {issuer}'s stream"),
            Self::StaleHead { held, offered } =>
                write!(f, "head does not advance: holding seq {held}, offered {offered}"),
            Self::MismatchedRecord { issuer, record_issuer } =>
                write!(f, "{issuer}'s stream cannot point at a record issued by {record_issuer}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A record together with its issuer's signature over [`KnowledgeRecord::canonical_bytes`].
///
/// The signature is **not** part of the record or its id: a record's identity is its content, and
/// the same content signed twice is the same record. This is the form a record travels in, and the
/// only form [`KnowledgeStore::put_signed`] accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRecord {
    /// The record.
    pub record: KnowledgeRecord,
    /// Its issuer's Ed25519 signature over the record's canonical bytes.
    pub signature: Vec<u8>,
}

/// How a stored record came to be attributed to its issuer (Boundary H item K1).
///
/// A **snapshot taken at storage**. It settles authenticity, which is historical and does not
/// change. It does not settle whether the record should count *now*: a key can be revoked after
/// storage, and present eligibility is re-derived at resolution from the retained signature (plan
/// item K1b).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribution {
    /// Stored through [`KnowledgeStore::put`], without verification. Trusted by construction by
    /// whoever put it there — tests, a node's own freshly built records — and by **no one else**.
    /// A reader that did not put it has no evidence of who wrote it.
    Unchecked,
    /// Verified on storage through one of issuer binding's two admissible paths.
    Verified {
        /// Which path verified it.
        path: IssuerPath,
        /// The key that verified it.
        key: [u8; 32],
        /// Whether that key was already validly revoked when the record was stored. Such a record is
        /// authentic history — its issuer wrote it — with no present standing.
        revoked_at_storage: bool,
    },
}

impl Attribution {
    /// Verified on storage, whatever the key's status.
    pub fn is_verified(&self) -> bool {
        matches!(self, Attribution::Verified { .. })
    }
}

/// Why [`KnowledgeStore::put_signed`] refused a record. Every refusal is counted
/// ([`KnowledgeStore::refusal_counts`]); none is a silent drop.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PutRefusal {
    /// The record's id does not match its content. A record that arrived by deserialisation never
    /// passed through [`KnowledgeRecord::new`], so this is checked again here.
    IdMismatch,
    /// The record breaks a rule [`KnowledgeRecord::new`] enforces: a foreign retraction, a
    /// self-link, an empty subject.
    Malformed(RecordError),
    /// No admissible path attributes the record to its issuer.
    Unverifiable(UnverifiableReason),
}

impl PutRefusal {
    /// A stable label for counting.
    pub fn label(&self) -> &'static str {
        match self {
            Self::IdMismatch => "id_mismatch",
            Self::Malformed(RecordError::ForeignRetraction { .. }) => "malformed_foreign_retraction",
            Self::Malformed(RecordError::SelfLink) => "malformed_self_link",
            Self::Malformed(RecordError::EmptySubject) => "malformed_empty_subject",
            Self::Unverifiable(UnverifiableReason::UnknownMember(_)) => "unknown_member",
            Self::Unverifiable(UnverifiableReason::MalformedMemberIssuer) => "malformed_member_issuer",
            Self::Unverifiable(UnverifiableReason::UntrustedExternal) => "untrusted_external",
            Self::Unverifiable(UnverifiableReason::BadSignature) => "bad_signature",
        }
    }
}

impl std::fmt::Display for PutRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdMismatch => write!(f, "the record's id does not match its content"),
            Self::Malformed(e) => write!(f, "malformed record: {e}"),
            Self::Unverifiable(r) => write!(f, "no admissible path attributes this record: {r:?}"),
        }
    }
}

impl std::error::Error for PutRefusal {}

/// Re-check what [`KnowledgeRecord::new`] checks, and that the id is the content's digest.
///
/// `new` is the only constructor, but `Deserialize` is derived, and a record from the wire never
/// went through `new`. Integrity is therefore checked where records enter a store from outside.
fn check_integrity(record: &KnowledgeRecord) -> Result<(), PutRefusal> {
    if record.id.issuer != record.issuer || record.id.digest != record.content_digest() {
        return Err(PutRefusal::IdMismatch);
    }
    if record.subject.is_empty() {
        return Err(PutRefusal::Malformed(RecordError::EmptySubject));
    }
    for l in &record.links {
        if l.kind == LinkKind::Retracts && l.target.issuer != record.issuer {
            return Err(PutRefusal::Malformed(RecordError::ForeignRetraction {
                by: record.issuer.clone(),
                target_issuer: l.target.issuer.clone(),
            }));
        }
        if l.target == record.id {
            return Err(PutRefusal::Malformed(RecordError::SelfLink));
        }
    }
    Ok(())
}

/// Records, and the heads that point into them.
///
/// In-memory here. Durability, authorisation of *readers*, and the transport that carries heads are
/// later PRs; what this settles is the shape, and the shape is the part that decides whether a
/// competing statement can be erased.
#[derive(Debug, Default)]
pub struct KnowledgeStore {
    records: HashMap<RecordId, KnowledgeRecord>,
    heads: HashMap<(IssuerId, String), Head>,
    /// How each record came to be attributed. Every record has an entry.
    attributions: HashMap<RecordId, Attribution>,
    /// The signature a verified record was stored with, retained so present eligibility can be
    /// re-derived later against the reader's then-current key view (plan item K1b).
    signatures: HashMap<RecordId, Vec<u8>>,
    /// Refusals by [`PutRefusal::label`]. Nothing is refused silently.
    refusals: BTreeMap<&'static str, u64>,
}

impl KnowledgeStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a record **without verifying it**, attributed [`Attribution::Unchecked`].
    ///
    /// For records trusted by construction by whoever puts them: tests, and a node's own freshly
    /// built records. A record received from anyone else belongs in [`put_signed`](Self::put_signed).
    /// Idempotent: a record's id is its content, so storing it twice is storing the same thing, and
    /// an existing verified attribution is never downgraded.
    pub fn put(&mut self, record: KnowledgeRecord) {
        let id = record.id().clone();
        self.records.insert(id.clone(), record);
        self.attributions.entry(id).or_insert(Attribution::Unchecked);
    }

    /// **Verify, then keep** (Boundary H item K1). The only way a record from outside should enter.
    ///
    /// 1. Integrity: the id must be the content's digest, and the record must satisfy the rules
    ///    [`KnowledgeRecord::new`] enforces. A deserialised record never went through `new`.
    /// 2. Attribution: through issuer binding's two admissible paths, with the key taken from the
    ///    reader's own view (`members`, `external`), never from the record.
    ///
    /// On success the record is stored with an [`Attribution::Verified`] snapshot and its signature
    /// is retained. A match under a since-revoked key is **stored**, as authentic history, and
    /// flagged `revoked_at_storage`. Everything else is refused, counted, and not stored.
    ///
    /// Idempotent. Re-storing a record already verified keeps its first attribution. A record
    /// previously stored unchecked is upgraded.
    pub fn put_signed(
        &mut self,
        signed: SignedRecord,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> Result<Attribution, PutRefusal> {
        let SignedRecord { record, signature } = signed;
        let outcome = check_integrity(&record).and_then(|()| {
            match verify_issuer(&record, &signature, members, external) {
                Authenticity::Current { path, key } => {
                    Ok(Attribution::Verified { path, key, revoked_at_storage: false })
                }
                Authenticity::Revoked { path, key } => {
                    Ok(Attribution::Verified { path, key, revoked_at_storage: true })
                }
                Authenticity::Unverifiable(reason) => Err(PutRefusal::Unverifiable(reason)),
            }
        });
        let attribution = match outcome {
            Ok(a) => a,
            Err(refusal) => {
                *self.refusals.entry(refusal.label()).or_insert(0) += 1;
                return Err(refusal);
            }
        };

        let id = record.id().clone();
        if let Some(existing @ Attribution::Verified { .. }) = self.attributions.get(&id) {
            return Ok(existing.clone());
        }
        self.records.insert(id.clone(), record);
        self.signatures.insert(id.clone(), signature);
        self.attributions.insert(id, attribution.clone());
        Ok(attribution)
    }

    /// How a record came to be attributed, if it is held.
    pub fn attribution(&self, id: &RecordId) -> Option<&Attribution> {
        self.attributions.get(id)
    }

    /// The signature a verified record was stored with. `None` for an unchecked record.
    pub fn signature(&self, id: &RecordId) -> Option<&[u8]> {
        self.signatures.get(id).map(Vec::as_slice)
    }

    /// Refusals by [`PutRefusal::label`], since this store was created.
    pub fn refusal_counts(&self) -> &BTreeMap<&'static str, u64> {
        &self.refusals
    }

    /// Fetch a record.
    pub fn get(&self, id: &RecordId) -> Option<&KnowledgeRecord> {
        self.records.get(id)
    }

    /// How many records are held. Used by the tests that pin "nothing was erased".
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Every record held, in unspecified order.
    ///
    /// The order is a `HashMap`'s and must not be depended on — anything that needs a stable order
    /// sorts by [`RecordId`], which is content-derived and therefore the same on every node.
    pub fn records(&self) -> impl Iterator<Item = &KnowledgeRecord> {
        self.records.values()
    }

    /// Is the store empty?
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Publish or advance a head.
    ///
    /// `by` is the issuer doing the publishing — checked against the head's own issuer, so an
    /// issuer cannot move someone else's stream.
    pub fn advance_head(&mut self, by: &IssuerId, head: Head) -> Result<(), StoreError> {
        if by != &head.issuer {
            return Err(StoreError::ForeignHead { by: by.clone(), issuer: head.issuer.clone() });
        }
        if head.record.issuer != head.issuer {
            return Err(StoreError::MismatchedRecord {
                issuer: head.issuer.clone(),
                record_issuer: head.record.issuer.clone(),
            });
        }
        let key = (head.issuer.clone(), head.stream.clone());
        if let Some(held) = self.heads.get(&key)
            && head.seq <= held.seq
        {
            return Err(StoreError::StaleHead { held: held.seq, offered: head.seq });
        }
        self.heads.insert(key, head);
        Ok(())
    }

    /// The current head of `(issuer, stream)`, if any.
    pub fn head(&self, issuer: &IssuerId, stream: &str) -> Option<&Head> {
        self.heads.get(&(issuer.clone(), stream.to_string()))
    }

    /// Can the head's record be read from here yet?
    ///
    /// `false` is ordinary — a head is a discovery pointer, and not having fetched the record yet
    /// is the state discovery exists to get you out of.
    pub fn is_resolvable(&self, head: &Head) -> bool {
        self.records.contains_key(&head.record)
    }

    /// **Every record about `subject`, from every issuer** — including ones no head points at.
    ///
    /// This is what "equivocation is preserved" *means* operationally. A reader asks what is known
    /// about something and is handed the disagreement, rather than the most recent writer's
    /// version of it.
    ///
    /// Sorted by record id so the answer is stable: an unordered answer would make two callers
    /// disagree about a set they both hold in full.
    pub fn about(&self, subject: &str) -> Vec<&KnowledgeRecord> {
        let mut out: Vec<&KnowledgeRecord> =
            self.records.values().filter(|r| r.subject() == subject).collect();
        out.sort_by(|a, b| a.id().cmp(b.id()));
        out
    }

    /// The distinct issuers who have said anything about `subject`.
    pub fn issuers_about(&self, subject: &str) -> Vec<&IssuerId> {
        let mut out: Vec<&IssuerId> = self.about(subject).into_iter().map(|r| r.issuer()).collect();
        out.dedup();
        out
    }
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeRecord, RecordKind};

    fn iss(s: &str) -> IssuerId {
        IssuerId::new(s).unwrap()
    }

    fn rec(issuer: &str, subject: &str, body: &[u8], at_ms: u64) -> KnowledgeRecord {
        KnowledgeRecord::new(
            iss(issuer),
            RecordKind::Observation,
            at_ms,
            subject,
            body.to_vec(),
            vec![],
        )
        .expect("well formed")
    }

    fn head_for(r: &KnowledgeRecord, stream: &str, seq: u64) -> Head {
        Head {
            issuer: r.issuer().clone(),
            stream: stream.to_string(),
            record: r.id().clone(),
            seq,
        }
    }

    // ── K1: verify on storage ────────────────────────────────────────────────────────────────

    mod k1 {
        use super::*;
        use crate::knowledge::issuer::MemberKeys;
        use crate::knowledge::Link;
        use crate::node_id::NodeId;
        use ed25519_dalek::SigningKey;
        use std::collections::HashMap;

        fn keypair(seed: u8) -> (SigningKey, [u8; 32]) {
            let sk = SigningKey::from_bytes(&[seed; 32]);
            let pk = sk.verifying_key().to_bytes();
            (sk, pk)
        }

        fn node(port: u16) -> NodeId {
            NodeId::new("127.0.0.1", port).expect("valid node id")
        }

        fn member_rec(n: &NodeId, at_ms: u64) -> KnowledgeRecord {
            KnowledgeRecord::new(
                IssuerId::for_node(n),
                RecordKind::Assessment,
                at_ms,
                "release/x",
                b"j".to_vec(),
                vec![],
            )
            .expect("well formed")
        }

        fn signed(sk: &SigningKey, r: KnowledgeRecord) -> SignedRecord {
            let signature = mycelium_core::tls::sign_bytes(sk, &r.canonical_bytes()).to_vec();
            SignedRecord { record: r, signature }
        }

        fn view(n: &NodeId, retained: Vec<[u8; 32]>, revoked: Vec<[u8; 32]>) -> HashMap<NodeId, MemberKeys> {
            HashMap::from([(n.clone(), MemberKeys { retained, revoked: revoked.into_iter().collect() })])
        }

        /// A member's signed record is stored, attributed to the member, with its signature kept.
        #[test]
        fn a_verified_record_is_stored_with_its_attribution_and_signature() {
            let (sk, pk) = keypair(1);
            let a = node(7101);
            let r = member_rec(&a, 1_000);
            let mut s = KnowledgeStore::new();

            let got = s.put_signed(signed(&sk, r.clone()), &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new());
            let want = Attribution::Verified { path: IssuerPath::Member(a), key: pk, revoked_at_storage: false };
            assert_eq!(got, Ok(want.clone()));
            assert_eq!(s.attribution(r.id()), Some(&want));
            assert!(s.signature(r.id()).is_some());
            assert!(s.refusal_counts().is_empty());
        }

        /// **Refused, counted, not stored.** Member B signing a record that names member A.
        #[test]
        fn a_forged_record_is_refused_counted_and_not_stored() {
            let (_sk_a, pk_a) = keypair(1);
            let (sk_b, _pk_b) = keypair(2);
            let a = node(7101);
            let r = member_rec(&a, 1_000);
            let mut s = KnowledgeStore::new();

            assert_eq!(
                s.put_signed(signed(&sk_b, r.clone()), &view(&a, vec![pk_a], vec![]), &TrustedExternalIssuers::new()),
                Err(PutRefusal::Unverifiable(UnverifiableReason::BadSignature))
            );
            assert!(s.get(r.id()).is_none(), "a refused record is not stored");
            assert_eq!(s.refusal_counts().get("bad_signature"), Some(&1));
        }

        /// An invented issuer name gets nothing: refused as untrusted, not stored.
        #[test]
        fn an_invented_issuer_is_refused() {
            let (sk, _pk) = keypair(1);
            let r = KnowledgeRecord::new(iss("sock-puppet-7"), RecordKind::Assessment, 1_000, "release/x", b"j".to_vec(), vec![]).unwrap();
            let mut s = KnowledgeStore::new();
            let empty: HashMap<NodeId, MemberKeys> = HashMap::new();
            assert_eq!(
                s.put_signed(signed(&sk, r), &empty, &TrustedExternalIssuers::new()),
                Err(PutRefusal::Unverifiable(UnverifiableReason::UntrustedExternal))
            );
            assert_eq!(s.refusal_counts().get("untrusted_external"), Some(&1));
            assert!(s.is_empty());
        }

        /// **Authentic is not current.** A record signed under a since-revoked key is stored as
        /// history, flagged, rather than refused.
        #[test]
        fn a_revoked_key_record_is_stored_as_history_and_flagged() {
            let (old_sk, old_pk) = keypair(1);
            let (_new_sk, new_pk) = keypair(2);
            let a = node(7101);
            let r = member_rec(&a, 1_000);
            let mut s = KnowledgeStore::new();
            let got = s.put_signed(signed(&old_sk, r.clone()), &view(&a, vec![new_pk, old_pk], vec![old_pk]), &TrustedExternalIssuers::new());
            assert_eq!(got, Ok(Attribution::Verified { path: IssuerPath::Member(a), key: old_pk, revoked_at_storage: true }));
            assert!(s.get(r.id()).is_some());
        }

        /// **A record from the wire never went through `new`.** Changing a field after
        /// serialisation leaves a stale id, and the store refuses it even under a valid signature
        /// over the altered content.
        #[test]
        fn a_deserialised_record_with_a_stale_id_is_refused() {
            let (sk, pk) = keypair(1);
            let a = node(7101);
            let mut v = serde_json::to_value(member_rec(&a, 1_000)).unwrap();
            v["at_ms"] = serde_json::json!(9_999);
            let tampered: KnowledgeRecord = serde_json::from_value(v).unwrap();
            let mut s = KnowledgeStore::new();
            assert_eq!(
                s.put_signed(signed(&sk, tampered), &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new()),
                Err(PutRefusal::IdMismatch)
            );
            assert_eq!(s.refusal_counts().get("id_mismatch"), Some(&1));
        }

        /// A foreign retraction smuggled in by deserialisation, with a recomputed id so it passes
        /// the id check, is still refused.
        #[test]
        fn a_deserialised_foreign_retraction_is_refused() {
            let (sk, pk) = keypair(1);
            let a = node(7101);
            let own = member_rec(&a, 1_000);
            let retraction = KnowledgeRecord::new(
                IssuerId::for_node(&a),
                RecordKind::Claim,
                2_000,
                "release/x",
                b"withdrawn".to_vec(),
                vec![Link { kind: LinkKind::Retracts, target: own.id().clone() }],
            )
            .unwrap();
            let mut v = serde_json::to_value(&retraction).unwrap();
            v["links"][0]["target"]["issuer"] = serde_json::json!("someone-else");
            let mut smuggled: KnowledgeRecord = serde_json::from_value(v).unwrap();
            smuggled.id.digest = smuggled.content_digest();
            let mut s = KnowledgeStore::new();
            assert!(matches!(
                s.put_signed(signed(&sk, smuggled), &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new()),
                Err(PutRefusal::Malformed(RecordError::ForeignRetraction { .. }))
            ));
        }

        /// `put` stays, and says what it is: unchecked. Signing the same record later upgrades it;
        /// a verified attribution is never downgraded by a later unchecked `put`.
        #[test]
        fn unchecked_put_is_marked_and_upgraded_but_never_downgrades_a_verified_record() {
            let (sk, pk) = keypair(1);
            let a = node(7101);
            let r = member_rec(&a, 1_000);
            let mut s = KnowledgeStore::new();

            s.put(r.clone());
            assert_eq!(s.attribution(r.id()), Some(&Attribution::Unchecked));
            assert!(s.signature(r.id()).is_none());

            s.put_signed(signed(&sk, r.clone()), &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new()).unwrap();
            assert!(s.attribution(r.id()).unwrap().is_verified());

            s.put(r.clone());
            assert!(s.attribution(r.id()).unwrap().is_verified(), "a later unchecked put must not downgrade");
            assert_eq!(s.len(), 1);
        }
    }

    // ── the decision this module exists to make true ─────────────────────────────────────────

    /// **Moving a pointer cannot erase a competing statement.**
    ///
    /// Put records in KV and last-write-wins becomes last-writer-is-right. With heads only, an
    /// advancing head moves a pointer and the superseded record is still there to be read.
    #[test]
    fn advancing_a_head_does_not_remove_what_it_pointed_at() {
        let mut s = KnowledgeStore::new();
        let first = rec("node-a", "svc/health", b"up", 1_000);
        let second = rec("node-a", "svc/health", b"degraded", 2_000);
        s.put(first.clone());
        s.put(second.clone());

        s.advance_head(&iss("node-a"), head_for(&first, "health", 1)).unwrap();
        s.advance_head(&iss("node-a"), head_for(&second, "health", 2)).unwrap();

        assert_eq!(s.head(&iss("node-a"), "health").unwrap().record, *second.id());
        assert!(
            s.get(first.id()).is_some(),
            "the superseded record must still be readable — the head moved, nothing was erased"
        );
        assert_eq!(s.len(), 2);
    }

    /// The operational meaning of "equivocation is preserved": a reader is handed the
    /// disagreement, not the later writer's version of it.
    #[test]
    fn two_issuers_disagreeing_are_both_returned() {
        let mut s = KnowledgeStore::new();
        let a = rec("node-a", "svc/health", b"up", 1_000);
        let b = rec("node-b", "svc/health", b"down", 1_000);
        s.put(a.clone());
        s.put(b.clone());

        let about = s.about("svc/health");
        assert_eq!(about.len(), 2, "both survive; neither wins");
        let mut issuers: Vec<String> =
            about.iter().map(|r| r.issuer().to_string()).collect();
        issuers.sort();
        assert_eq!(issuers, vec!["node-a".to_string(), "node-b".to_string()]);
    }

    /// One issuer advancing its own head must not disturb another's stream.
    #[test]
    fn heads_are_independent_per_issuer() {
        let mut s = KnowledgeStore::new();
        let a = rec("node-a", "svc/health", b"up", 1_000);
        let b = rec("node-b", "svc/health", b"down", 1_000);
        s.put(a.clone());
        s.put(b.clone());
        s.advance_head(&iss("node-a"), head_for(&a, "health", 1)).unwrap();
        s.advance_head(&iss("node-b"), head_for(&b, "health", 1)).unwrap();

        let a2 = rec("node-a", "svc/health", b"recovered", 2_000);
        s.put(a2.clone());
        s.advance_head(&iss("node-a"), head_for(&a2, "health", 2)).unwrap();

        assert_eq!(s.head(&iss("node-b"), "health").unwrap().record, *b.id(),
            "node-b's stream is untouched by node-a advancing");
    }

    // ── "authorized" ─────────────────────────────────────────────────────────────────────────

    /// Same shape as retraction in the parent module: an issuer publishes only its own heads, and
    /// the check is local.
    #[test]
    fn an_issuer_cannot_publish_another_issuers_head() {
        let mut s = KnowledgeStore::new();
        let b = rec("node-b", "svc/health", b"down", 1_000);
        s.put(b.clone());
        assert_eq!(
            s.advance_head(&iss("node-a"), head_for(&b, "health", 1)),
            Err(StoreError::ForeignHead { by: iss("node-a"), issuer: iss("node-b") })
        );
    }

    /// A stream may not point at another issuer's record — otherwise an issuer could adopt
    /// someone else's statement as its own by pointing at it.
    #[test]
    fn a_stream_cannot_point_at_another_issuers_record() {
        let mut s = KnowledgeStore::new();
        let b = rec("node-b", "svc/health", b"down", 1_000);
        s.put(b.clone());
        let smuggled = Head {
            issuer: iss("node-a"),
            stream: "health".into(),
            record: b.id().clone(),
            seq: 1,
        };
        assert_eq!(
            s.advance_head(&iss("node-a"), smuggled),
            Err(StoreError::MismatchedRecord {
                issuer: iss("node-a"),
                record_issuer: iss("node-b"),
            })
        );
    }

    /// **A head only advances.** A stale copy re-delivered by anti-entropy must not roll a stream
    /// backwards, and the refusal carries both sequence numbers because *"we have newer"* and
    /// *"this is a duplicate"* are indistinguishable without them.
    #[test]
    fn a_replayed_older_head_is_refused_with_both_sequence_numbers() {
        let mut s = KnowledgeStore::new();
        let first = rec("node-a", "svc/health", b"up", 1_000);
        let second = rec("node-a", "svc/health", b"degraded", 2_000);
        s.put(first.clone());
        s.put(second.clone());
        s.advance_head(&iss("node-a"), head_for(&first, "health", 1)).unwrap();
        s.advance_head(&iss("node-a"), head_for(&second, "health", 2)).unwrap();

        assert_eq!(
            s.advance_head(&iss("node-a"), head_for(&first, "health", 1)),
            Err(StoreError::StaleHead { held: 2, offered: 1 })
        );
        assert_eq!(
            s.advance_head(&iss("node-a"), head_for(&second, "health", 2)),
            Err(StoreError::StaleHead { held: 2, offered: 2 }),
            "the same seq is not an advance either"
        );
        assert_eq!(s.head(&iss("node-a"), "health").unwrap().record, *second.id());
    }

    // ── discovery ────────────────────────────────────────────────────────────────────────────

    /// A head pointing at a record we have not fetched is **ordinary**, not an error — refusing it
    /// would make discovery depend on having already discovered.
    #[test]
    fn a_head_may_point_at_a_record_we_do_not_hold_yet() {
        let mut s = KnowledgeStore::new();
        let unfetched = rec("node-a", "svc/health", b"up", 1_000);
        let h = head_for(&unfetched, "health", 1);
        s.advance_head(&iss("node-a"), h.clone()).expect("a head is a discovery pointer");
        assert!(!s.is_resolvable(&h), "and the store says plainly that it cannot read it yet");

        s.put(unfetched);
        assert!(s.is_resolvable(&h));
    }

    /// The head's KV key is the reserved namespace and nothing else.
    #[test]
    fn a_heads_kv_key_is_the_reserved_prefix() {
        let r = rec("node-a", "svc/health", b"up", 1_000);
        let key = head_for(&r, "health", 1).kv_key();
        assert_eq!(key, "knowledge/head/node-a/health");
        assert!(key.starts_with(mycelium_core::signal::kv_ns::KNOWLEDGE_HEAD));
    }

    /// `about` is stable across calls — an unordered answer would let two callers holding the same
    /// set disagree about it.
    #[test]
    fn about_is_stably_ordered() {
        let mut s = KnowledgeStore::new();
        for (i, who) in ["node-c", "node-a", "node-b"].iter().enumerate() {
            s.put(rec(who, "svc/health", b"x", 1_000 + i as u64));
        }
        let once: Vec<String> = s.about("svc/health").iter().map(|r| r.id().to_string()).collect();
        let twice: Vec<String> = s.about("svc/health").iter().map(|r| r.id().to_string()).collect();
        assert_eq!(once, twice);
        assert_eq!(once.len(), 3);
    }
}
