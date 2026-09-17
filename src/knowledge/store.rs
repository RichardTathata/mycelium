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

use super::{IssuerId, KnowledgeRecord, RecordId};
use std::collections::HashMap;

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

/// Records, and the heads that point into them.
///
/// In-memory here. Durability, authorisation of *readers*, and the transport that carries heads are
/// later PRs; what this settles is the shape, and the shape is the part that decides whether a
/// competing statement can be erased.
#[derive(Debug, Default)]
pub struct KnowledgeStore {
    records: HashMap<RecordId, KnowledgeRecord>,
    heads: HashMap<(IssuerId, String), Head>,
}

impl KnowledgeStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a record. Idempotent: a record's id is its content, so storing it twice is storing the
    /// same thing.
    pub fn put(&mut self, record: KnowledgeRecord) {
        self.records.insert(record.id().clone(), record);
    }

    /// Fetch a record.
    pub fn get(&self, id: &RecordId) -> Option<&KnowledgeRecord> {
        self.records.get(id)
    }

    /// How many records are held. Used by the tests that pin "nothing was erased".
    pub fn len(&self) -> usize {
        self.records.len()
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
