//! **Context selection** (adopter-surface Y1, decision D14) — records for a run, chosen without being
//! blended.
//!
//! An agent about to run wants context: what is known about the thing it is working on. The easy
//! answer is a summary, and a summary is where the layer's distinctions go to die — *"two issuers
//! rejected this and one did not know"* becomes *"this is doubtful"*, signed by nobody. D14 decided the
//! other answer:
//!
//! > A context summary is a **selection**; a condensed text is a **new signed record**, linked to its
//! > sources, scoped by the caller's mandate.
//!
//! # What a selection is
//!
//! [`Selector::select`] returns records, not text. Each one keeps what the layer knows about it:
//! its **issuer** (in the record and its id), its **attribution** as stored, its **standing**
//! ([`Standing`]: current, retracted, basis withdrawn, expired) and the **verdict** the reader holds
//! for its subject, if any. From the last two it derives one [`VerdictClass`]. The ranking is the
//! adopter's — a closure that scores a candidate or says it is not relevant — and this module only
//! orders by it; there is no vector store and no relevance model here.
//!
//! # Compression stays inside one class
//!
//! If the adopter condenses a selection — an LLM shortening twelve records into a paragraph — the
//! paragraph is a statement someone made, so it goes back as a record of its own:
//! [`Selector::write_condensed`] accepts it only
//!
//! - **signed by its own issuer**, verified as [`KnowledgeStore::put_signed`] verifies anything
//!   (an unsigned condensation is a record nobody signed, which is what D14 exists to refuse);
//! - as a [`RecordKind::Claim`], so it never counts as evidence for anything: a condensation of
//!   assessments is not an assessment;
//! - linked by `DerivedFrom` to **every** record it drew on, and by nothing else, so a retraction of
//!   any source reaches it as `BasisWithdrawn` through the existing correction index;
//! - when **every source is in one class**, re-derived here at the moment of writing rather than taken
//!   from the selection that proposed it. An `InsufficientEvidence` record and a `Rejected` one are
//!   two classes — *we do not know* and *we looked and it is bad* — and a `Retracted`,
//!   `BasisWithdrawn` or `Expired` record is never in the class of a live one.
//!
//! # The mandate scopes what can be read, and the scope is one rule
//!
//! A selection is taken through a [`ReadScope`], built from the caller's [`Mandate`]: current, and
//! enumerating [`KNOWLEDGE_READ`]. A record is readable under it when its subject is the mandate's
//! scope or lies beneath it (`scope/…`, by path segment), **and** every record it was derived from is
//! held and readable too. The second half is what stops a condensation laundering content: a record
//! derived from something the caller cannot read is withheld, because its text may be that
//! something. A source the store does not hold is unreadable by the same rule — unknown is not
//! readable — and the selection reports both cases with one reason, so withholding does not tell the
//! caller whether an out-of-scope record exists.
//!
//! [`ScopedView`] is the direct read under the same rule ([`ScopedView::get`], [`ScopedView::about`]),
//! and the selector reads only through it, so the selection cannot see a record the caller could not
//! read directly. The [`KnowledgeStore`]'s own methods stay unscoped: they are the store holder's,
//! and no door — no HTTP route, no SDK verb — reads knowledge records today.
//!
//! # Not here
//!
//! The wiki: its pages carry no issuer, verdict or standing, so selecting one with its provenance
//! would mean filing it as a record first. A read mandate's epoch and revocation standing: a
//! knowledge store installs no epoch, so a read scope checks the window and the operation only.
//! Re-verifying a selected record against the reader's current key view, which
//! [`classify_eligible`](super::resolution::classify_eligible) does for evidence.

use std::collections::{BTreeMap, BTreeSet};

use crate::mandate::{Mandate, MandateRefusal};

use super::correction::{standing, DependencyIndex, Standing};
use super::issuer::{MemberKeySource, TrustedExternalIssuers};
use super::resolution::Verdict;
use super::store::{Attribution, KnowledgeStore, PutRefusal, SignedRecord};
use super::{IssuerId, KnowledgeRecord, Link, LinkKind, RecordError, RecordId, RecordKind};

/// The operation a mandate enumerates to read knowledge records into context.
pub const KNOWLEDGE_READ: &str = "knowledge.read";

/// What the caller's mandate lets it read. Built only from a current mandate that enumerates
/// [`KNOWLEDGE_READ`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadScope {
    scope: String,
}

impl ReadScope {
    /// The read scope of `mandate` at `now_ms`, or the refusal the mandate earns.
    ///
    /// Outside its window → [`MandateRefusal::OutOfWindow`]; not enumerating [`KNOWLEDGE_READ`] →
    /// [`MandateRefusal::NotEnumerated`]. A mandate with an empty scope builds a scope that covers
    /// nothing.
    pub fn from_mandate(mandate: &Mandate, now_ms: u64) -> Result<Self, MandateRefusal> {
        if !mandate.is_current(now_ms) {
            return Err(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: mandate.valid_until_ms });
        }
        if !mandate.permits(KNOWLEDGE_READ) {
            return Err(MandateRefusal::NotEnumerated { operation: KNOWLEDGE_READ.to_string() });
        }
        Ok(Self { scope: mandate.scope.clone() })
    }

    /// The mandate's scope.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Is `subject` the scope, or beneath it by path segment? `svc` covers `svc` and `svc/health`,
    /// never `svcx`. An empty scope covers nothing.
    pub fn covers(&self, subject: &str) -> bool {
        if self.scope.is_empty() {
            return false;
        }
        let base = self.scope.strip_suffix('/').unwrap_or(&self.scope);
        subject == base
            || subject.strip_prefix(base).is_some_and(|rest| rest.starts_with('/'))
    }

    /// The store, as this scope may read it.
    pub fn view<'a>(&self, store: &'a KnowledgeStore) -> ScopedView<'a> {
        // A fixpoint over the covered records: drop any whose `DerivedFrom` source is not held or not
        // (still) readable, until nothing changes. Cycle-safe: a cycle of covered records whose
        // sources are all covered stays readable, and every member of it was checked for coverage.
        let mut readable: BTreeSet<RecordId> = store
            .records()
            .filter(|r| self.covers(r.subject()))
            .map(|r| r.id().clone())
            .collect();
        let covered = readable.clone();
        loop {
            let unreadable: Vec<RecordId> = readable
                .iter()
                .filter(|id| {
                    store.get(id).is_some_and(|r| {
                        r.links()
                            .iter()
                            .any(|l| l.kind == LinkKind::DerivedFrom && !readable.contains(&l.target))
                    })
                })
                .cloned()
                .collect();
            if unreadable.is_empty() {
                break;
            }
            for id in unreadable {
                readable.remove(&id);
            }
        }
        let withheld = covered.difference(&readable).cloned().collect();
        ScopedView { store, readable, withheld }
    }
}

/// A [`KnowledgeStore`] as one [`ReadScope`] may read it — the direct read under the rule the
/// selector uses.
pub struct ScopedView<'a> {
    store: &'a KnowledgeStore,
    readable: BTreeSet<RecordId>,
    /// Covered by the scope, but derived from a record that is not held or not readable.
    withheld: BTreeSet<RecordId>,
}

impl<'a> ScopedView<'a> {
    /// A record, if this scope may read it.
    pub fn get(&self, id: &RecordId) -> Option<&'a KnowledgeRecord> {
        if self.readable.contains(id) { self.store.get(id) } else { None }
    }

    /// Every readable record about `subject`, sorted by record id.
    pub fn about(&self, subject: &str) -> Vec<&'a KnowledgeRecord> {
        self.records().filter(|r| r.subject() == subject).collect()
    }

    /// Every readable record, sorted by record id.
    pub fn records(&self) -> impl Iterator<Item = &'a KnowledgeRecord> + '_ {
        self.readable.iter().filter_map(|id| self.store.get(id))
    }

    /// Records the scope covers that were withheld because a source is not held or not readable,
    /// sorted. One reason for both, so the list does not say which.
    pub fn withheld(&self) -> impl Iterator<Item = &RecordId> {
        self.withheld.iter()
    }
}

/// One class a selected record belongs to. Records condense together only within one.
///
/// A record's standing decides first — a record that is not current is in its standing's class
/// whatever its subject's verdict — and a current record is in its verdict's class, or `Unjudged`
/// where the reader holds no verdict for its subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VerdictClass {
    /// Current, and its subject accepted.
    Accepted,
    /// Current, and its subject rejected: *we looked and it is bad*.
    Rejected,
    /// Current, and too little evidence about its subject: *we do not know*.
    InsufficientEvidence,
    /// Current, with support and challenge both current about its subject.
    Conflicted,
    /// Current, and the reader holds no verdict for its subject.
    Unjudged,
    /// Its own issuer withdrew it.
    Retracted,
    /// A record it was derived from was withdrawn.
    BasisWithdrawn,
    /// Older than the reader's window.
    Expired,
}

impl VerdictClass {
    /// The class of a record with this standing and this verdict for its subject.
    pub fn of(standing: &Standing, verdict: Option<&Verdict>) -> Self {
        match standing {
            Standing::Retracted { .. } => Self::Retracted,
            Standing::BasisWithdrawn { .. } => Self::BasisWithdrawn,
            Standing::Expired { .. } => Self::Expired,
            Standing::Current => match verdict {
                None => Self::Unjudged,
                Some(Verdict::Accepted { .. }) => Self::Accepted,
                Some(Verdict::Rejected { .. }) => Self::Rejected,
                Some(Verdict::InsufficientEvidence { .. }) => Self::InsufficientEvidence,
                Some(Verdict::Conflicted { .. }) => Self::Conflicted,
            },
        }
    }
}

/// What the caller is selecting for. The selector passes it to the ranking closure and uses only
/// `limit` itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRequest {
    /// The capability the run exercises, as the adopter names it.
    pub capability: String,
    /// The task, as the adopter describes it.
    pub task: String,
    /// At most this many records are returned; the rest are cut after ranking (`truncated`).
    pub limit: usize,
}

/// One record as the ranking closure sees it.
pub struct Candidate<'a> {
    /// The request.
    pub request: &'a SelectionRequest,
    /// The record.
    pub record: &'a KnowledgeRecord,
    /// How it came to be attributed.
    pub attribution: &'a Attribution,
    /// Where it stands.
    pub standing: &'a Standing,
    /// The reader's verdict for its subject, if any.
    pub verdict: Option<&'a Verdict>,
    /// Its class.
    pub class: VerdictClass,
}

/// One selected record, with everything the layer knows about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    /// The record, unchanged — its issuer in it and in its id.
    pub record: KnowledgeRecord,
    /// How it was attributed when stored.
    pub attribution: Attribution,
    /// Where it stands now.
    pub standing: Standing,
    /// The reader's verdict for its subject, if any.
    pub verdict: Option<Verdict>,
    /// Its class.
    pub class: VerdictClass,
    /// The ranking closure's score.
    pub score: u64,
}

impl Selected {
    /// Its issuer.
    pub fn issuer(&self) -> &IssuerId {
        self.record.issuer()
    }
}

/// The records chosen for a run, ranked: highest score first, ties by record id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The records, ranked.
    pub records: Vec<Selected>,
    /// Records the scope covers that were withheld because a source is not held or not readable.
    pub withheld: Vec<RecordId>,
    /// Whether `limit` cut ranked records.
    pub truncated: bool,
}

impl Selection {
    /// The selected records grouped by class, each group in rank order — the groups a condensation
    /// may draw from, one at a time.
    pub fn by_class(&self) -> BTreeMap<VerdictClass, Vec<&Selected>> {
        let mut out: BTreeMap<VerdictClass, Vec<&Selected>> = BTreeMap::new();
        for s in &self.records {
            out.entry(s.class).or_default().push(s);
        }
        out
    }
}

/// Why a condensed record was not written back.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CondenseRefusal {
    /// No admissible path attributes the condensation to its issuer: unsigned, signed by someone
    /// else, or by an issuer the reader does not know. A condensation nobody signed is not a record.
    Unattributed(PutRefusal),
    /// Not a [`RecordKind::Claim`]. A condensation is never evidence, so it cannot be an assessment.
    NotAClaim(RecordKind),
    /// It cites nothing. A condensation is linked to every record it drew on.
    NoSources,
    /// It carries a link other than `DerivedFrom`.
    NotDerivedFrom(LinkKind),
    /// Its own subject is outside the caller's scope.
    SubjectOutsideScope,
    /// A source is not held, or not readable under the caller's scope. One reason for both.
    SourceUnreadable(RecordId),
    /// Its sources are in more than one class.
    AcrossVerdicts(Vec<(RecordId, VerdictClass)>),
    /// The record itself is malformed (building a draft).
    Malformed(RecordError),
}

impl std::fmt::Display for CondenseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unattributed(r) => write!(f, "a condensation must be signed by its own issuer: {r}"),
            Self::NotAClaim(k) => write!(f, "a condensation is a claim, never {k:?}"),
            Self::NoSources => write!(f, "a condensation must cite every record it drew on"),
            Self::NotDerivedFrom(k) => write!(f, "a condensation links to its sources by DerivedFrom only, not {k:?}"),
            Self::SubjectOutsideScope => write!(f, "the condensation's subject is outside the caller's scope"),
            Self::SourceUnreadable(id) => write!(f, "source {id} is not readable under the caller's scope"),
            Self::AcrossVerdicts(classes) => {
                write!(f, "a condensation draws from one class; its sources are in")?;
                let distinct: BTreeSet<VerdictClass> = classes.iter().map(|(_, c)| *c).collect();
                for c in distinct {
                    write!(f, " {c:?}")?;
                }
                Ok(())
            }
            Self::Malformed(e) => write!(f, "malformed condensation: {e}"),
        }
    }
}

impl std::error::Error for CondenseRefusal {}

/// Build the unsigned draft of a condensation over `sources`: a claim by `issuer`, linked by
/// `DerivedFrom` to each source once. Refuses an empty source list or sources in more than one class
/// — as [`Selector::write_condensed`] will, which re-derives both and is the check that counts.
pub fn condensation_draft(
    sources: &[&Selected],
    issuer: IssuerId,
    at_ms: u64,
    subject: impl Into<String>,
    body: Vec<u8>,
) -> Result<KnowledgeRecord, CondenseRefusal> {
    if sources.is_empty() {
        return Err(CondenseRefusal::NoSources);
    }
    let classes: BTreeSet<VerdictClass> = sources.iter().map(|s| s.class).collect();
    if classes.len() > 1 {
        return Err(CondenseRefusal::AcrossVerdicts(
            sources.iter().map(|s| (s.record.id().clone(), s.class)).collect(),
        ));
    }
    let targets: BTreeSet<RecordId> = sources.iter().map(|s| s.record.id().clone()).collect();
    let links = targets.into_iter().map(|target| Link { kind: LinkKind::DerivedFrom, target }).collect();
    KnowledgeRecord::new(issuer, RecordKind::Claim, at_ms, subject, body, links).map_err(CondenseRefusal::Malformed)
}

/// A reader's selector: one scope, one clock, one window, one verdict source.
pub struct Selector<'v> {
    scope: ReadScope,
    now_ms: u64,
    max_age_ms: u64,
    require_verified: bool,
    verdicts: &'v dyn Fn(&str) -> Option<Verdict>,
}

impl<'v> Selector<'v> {
    /// A selector reading through `scope` at `now_ms`, records older than `max_age_ms` being
    /// `Expired`. `verdicts` is the reader's verdict for a subject — typically
    /// [`classify_eligible`](super::resolution::classify_eligible) over the release the subject names
    /// — or `None` where the reader has none.
    pub fn new(
        scope: ReadScope,
        now_ms: u64,
        max_age_ms: u64,
        verdicts: &'v dyn Fn(&str) -> Option<Verdict>,
    ) -> Self {
        Self { scope, now_ms, max_age_ms, require_verified: false, verdicts }
    }

    /// Leave out records stored without verification ([`Attribution::Unchecked`]), and do not let one
    /// retract or derive anything — the correction index is built over verified records only.
    pub fn require_verified(mut self) -> Self {
        self.require_verified = true;
        self
    }

    fn index(&self, store: &KnowledgeStore) -> DependencyIndex {
        if self.require_verified {
            DependencyIndex::build_filtered(store, |r| {
                store.attribution(r.id()).is_some_and(Attribution::is_verified)
            })
        } else {
            DependencyIndex::build(store)
        }
    }

    fn class_of(&self, store: &KnowledgeStore, index: &DependencyIndex, record: &KnowledgeRecord)
        -> Option<(Standing, Option<Verdict>, VerdictClass)>
    {
        let st = standing(store, index, record.id(), self.now_ms, self.max_age_ms)?;
        let verdict = (self.verdicts)(record.subject());
        let class = VerdictClass::of(&st, verdict.as_ref());
        Some((st, verdict, class))
    }

    /// **Select records for a run.** Reads only through this selector's [`ScopedView`]; scores each
    /// readable record with `rank` (`None` = not relevant) and returns the top `request.limit`.
    pub fn select(
        &self,
        store: &KnowledgeStore,
        request: &SelectionRequest,
        rank: impl Fn(&Candidate<'_>) -> Option<u64>,
    ) -> Selection {
        let view = self.scope.view(store);
        let index = self.index(store);
        let mut out: Vec<Selected> = Vec::new();
        for record in view.records() {
            let Some(attribution) = store.attribution(record.id()) else { continue };
            if self.require_verified && !attribution.is_verified() {
                continue;
            }
            let Some((st, verdict, class)) = self.class_of(store, &index, record) else { continue };
            let candidate = Candidate {
                request,
                record,
                attribution,
                standing: &st,
                verdict: verdict.as_ref(),
                class,
            };
            if let Some(score) = rank(&candidate) {
                out.push(Selected {
                    record: record.clone(),
                    attribution: attribution.clone(),
                    standing: st,
                    verdict,
                    class,
                    score,
                });
            }
        }
        out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.record.id().cmp(b.record.id())));
        let truncated = out.len() > request.limit;
        out.truncate(request.limit);
        Selection { records: out, withheld: view.withheld().cloned().collect(), truncated }
    }

    /// **Check a condensation without writing it.** Every rule in the module docs, re-derived now
    /// against `store`: attributed to its own issuer, a claim, its subject in scope, linked by
    /// `DerivedFrom` only and to at least one source, every source readable, all in one class.
    pub fn check_condensed(
        &self,
        store: &KnowledgeStore,
        signed: &SignedRecord,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> Result<(), CondenseRefusal> {
        store.verify_signed(signed, members, external).map_err(CondenseRefusal::Unattributed)?;
        let record = &signed.record;
        if record.kind() != RecordKind::Claim {
            return Err(CondenseRefusal::NotAClaim(record.kind()));
        }
        if !self.scope.covers(record.subject()) {
            return Err(CondenseRefusal::SubjectOutsideScope);
        }
        if record.links().is_empty() {
            return Err(CondenseRefusal::NoSources);
        }
        if let Some(l) = record.links().iter().find(|l| l.kind != LinkKind::DerivedFrom) {
            return Err(CondenseRefusal::NotDerivedFrom(l.kind));
        }
        let view = self.scope.view(store);
        let index = self.index(store);
        let mut classes: Vec<(RecordId, VerdictClass)> = Vec::new();
        for link in record.links() {
            let unreadable = || CondenseRefusal::SourceUnreadable(link.target.clone());
            let source = view.get(&link.target).ok_or_else(unreadable)?;
            if self.require_verified
                && !store.attribution(source.id()).is_some_and(Attribution::is_verified)
            {
                return Err(unreadable());
            }
            let (_, _, class) = self.class_of(store, &index, source).ok_or_else(unreadable)?;
            classes.push((source.id().clone(), class));
        }
        let distinct: BTreeSet<VerdictClass> = classes.iter().map(|(_, c)| *c).collect();
        if distinct.len() > 1 {
            return Err(CondenseRefusal::AcrossVerdicts(classes));
        }
        Ok(())
    }

    /// **Write a condensation back** into `store`: [`check_condensed`](Self::check_condensed), then
    /// [`KnowledgeStore::put_signed`]. For a durable store, check here and then put through
    /// `DurableKnowledgeStore::put_signed`, which verifies again before it persists.
    pub fn write_condensed(
        &self,
        store: &mut KnowledgeStore,
        signed: SignedRecord,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> Result<RecordId, CondenseRefusal> {
        self.check_condensed(store, &signed, members, external)?;
        let id = signed.record.id().clone();
        store.put_signed(signed, members, external).map_err(CondenseRefusal::Unattributed)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mandate::{PrincipalId, TermId};
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

    use crate::node_id::NodeId;
    use super::super::issuer::MemberKeys;

    const NOW: u64 = 1_800_000_000_000;
    const DAY: u64 = 86_400_000;

    fn iss(s: &str) -> IssuerId {
        IssuerId::new(s).unwrap()
    }

    fn mandate(scope: &str, operations: &[&str]) -> Mandate {
        Mandate {
            holder: PrincipalId::new("agent-a").unwrap(),
            established_by: PrincipalId::new("operator").unwrap(),
            purpose: "context for a run".into(),
            scope: scope.into(),
            operations: operations.iter().map(|s| s.to_string()).collect(),
            epoch: 1,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: NOW - DAY,
            valid_until_ms: NOW + DAY,
        }
    }

    fn read(scope: &str) -> ReadScope {
        ReadScope::from_mandate(&mandate(scope, &[KNOWLEDGE_READ]), NOW).unwrap()
    }

    /// Issuers the reader trusts, each with its key.
    struct World {
        keys: BTreeMap<&'static str, SigningKey>,
        external: TrustedExternalIssuers,
        members: HashMap<NodeId, MemberKeys>,
        store: KnowledgeStore,
    }

    impl World {
        fn new() -> Self {
            let mut w = Self {
                keys: BTreeMap::new(),
                external: TrustedExternalIssuers::new(),
                members: HashMap::new(),
                store: KnowledgeStore::new(),
            };
            for (i, name) in ["auditor-1", "auditor-2", "observer", "condenser"].into_iter().enumerate() {
                let sk = SigningKey::from_bytes(&[i as u8 + 11; 32]);
                w.external.trust(iss(name), sk.verifying_key().to_bytes()).unwrap();
                w.keys.insert(name, sk);
            }
            w
        }

        fn sign(&self, record: KnowledgeRecord) -> SignedRecord {
            let sk = &self.keys[record.issuer().as_str()];
            let signature = mycelium_core::tls::sign_bytes(sk, &record.canonical_bytes()).to_vec();
            SignedRecord { record, signature }
        }

        fn put(&mut self, issuer: &str, kind: RecordKind, subject: &str, body: &str, links: Vec<Link>) -> RecordId {
            let r = KnowledgeRecord::new(iss(issuer), kind, NOW - 1_000, subject, body.as_bytes().to_vec(), links)
                .unwrap();
            let signed = self.sign(r);
            let id = signed.record.id().clone();
            self.store.put_signed(signed, &self.members, &self.external).unwrap();
            id
        }

        fn write(&mut self, selector: &Selector<'_>, record: KnowledgeRecord) -> Result<RecordId, CondenseRefusal> {
            let signed = self.sign(record);
            selector.write_condensed(&mut self.store, signed, &self.members, &self.external)
        }

        fn condensation(&self, issuer: &str, subject: &str, sources: &[&RecordId]) -> KnowledgeRecord {
            let links = sources
                .iter()
                .map(|t| Link { kind: LinkKind::DerivedFrom, target: (*t).clone() })
                .collect();
            KnowledgeRecord::new(iss(issuer), RecordKind::Claim, NOW, subject, b"in short".to_vec(), links).unwrap()
        }
    }

    fn rejected() -> Verdict {
        Verdict::Rejected { reasons: vec![super::super::resolution::RejectionReason::Challenged { by: iss("auditor-2") }] }
    }

    fn insufficient() -> Verdict {
        Verdict::InsufficientEvidence { have: 0, need: 2, independent: 0, need_independent: 2 }
    }

    /// Two subjects with two verdicts: `svc/a` rejected, `svc/b` not known.
    fn verdicts(subject: &str) -> Option<Verdict> {
        match subject {
            "svc/a" => Some(rejected()),
            "svc/b" => Some(insufficient()),
            _ => None,
        }
    }

    fn all(_: &Candidate<'_>) -> Option<u64> {
        Some(1)
    }

    fn request() -> SelectionRequest {
        SelectionRequest { capability: "svc.health".into(), task: "triage".into(), limit: 100 }
    }

    // ── the scope ───────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_read_scope_needs_a_current_mandate_that_enumerates_knowledge_read() {
        assert_eq!(
            ReadScope::from_mandate(&mandate("svc", &["wiki.write"]), NOW),
            Err(MandateRefusal::NotEnumerated { operation: KNOWLEDGE_READ.into() })
        );
        assert!(matches!(
            ReadScope::from_mandate(&mandate("svc", &[KNOWLEDGE_READ]), NOW + 2 * DAY),
            Err(MandateRefusal::OutOfWindow { .. })
        ));
        let s = read("svc");
        assert!(s.covers("svc") && s.covers("svc/health") && s.covers("svc/a/b"));
        assert!(!s.covers("svcx") && !s.covers("other/svc") && !s.covers(""));
        assert!(read("svc/").covers("svc/health"), "a trailing slash names the same scope");
        assert!(!read("").covers("svc") && !read("").covers("/x"), "an empty scope covers nothing");
    }

    /// **The exit's plant:** a mandate-scoped selection excludes a record the mandate does not cover,
    /// and a record derived from one — while the store, read unscoped, holds both.
    #[test]
    fn a_scoped_selection_excludes_what_the_mandate_does_not_cover_and_what_derives_from_it() {
        let mut w = World::new();
        let inside = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let outside = w.put("observer", RecordKind::Observation, "payroll/q3", "secret", vec![]);
        // In scope by subject, derived from the out-of-scope record: its text may be that record's.
        let laundered = w.put(
            "auditor-1", RecordKind::Assessment, "svc/a", "about payroll",
            vec![Link { kind: LinkKind::DerivedFrom, target: outside.clone() }],
        );
        // Derived from a record the store does not hold: unknown is not readable.
        let ghost = RecordId { issuer: iss("observer"), digest: [7; 32] };
        let orphan = w.put(
            "auditor-2", RecordKind::Assessment, "svc/a", "basis absent",
            vec![Link { kind: LinkKind::DerivedFrom, target: ghost }],
        );
        assert_eq!(w.store.len(), 4, "the store holds all four, unscoped");

        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let selection = selector.select(&w.store, &request(), all);
        let ids: Vec<&RecordId> = selection.records.iter().map(|s| s.record.id()).collect();
        assert_eq!(ids, vec![&inside], "only the record the mandate covers, with its own sources readable");
        let mut withheld = vec![laundered.clone(), orphan.clone()];
        withheld.sort();
        assert_eq!(selection.withheld, withheld, "one reason for both, so it does not say which");

        // The direct read under the same scope agrees with the selection.
        let view = read("svc").view(&w.store);
        assert!(view.get(&outside).is_none() && view.get(&laundered).is_none() && view.get(&orphan).is_none());
        assert_eq!(view.about("svc/a").len(), 1);

        // A scope that covers payroll reads the out-of-scope record, and nothing under `svc`.
        let payroll = read("payroll").view(&w.store);
        assert_eq!(payroll.records().map(|r| r.id()).collect::<Vec<_>>(), vec![&outside]);
    }

    #[test]
    fn each_selected_record_keeps_its_issuer_attribution_standing_and_verdict() {
        let mut w = World::new();
        let a = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let withdrawn = w.put("observer", RecordKind::Observation, "svc/b", "down", vec![]);
        w.put("observer", RecordKind::Claim, "svc/b", "retract",
            vec![Link { kind: LinkKind::Retracts, target: withdrawn.clone() }]);
        let derived = w.put("auditor-1", RecordKind::Assessment, "svc/b", "so it is broken",
            vec![Link { kind: LinkKind::DerivedFrom, target: withdrawn.clone() }]);
        let unjudged = w.put("observer", RecordKind::Observation, "svc/c", "fine", vec![]);

        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let selection = selector.select(&w.store, &request(), all);
        let by_id: BTreeMap<&RecordId, &Selected> =
            selection.records.iter().map(|s| (s.record.id(), s)).collect();

        assert_eq!(by_id[&a].class, VerdictClass::Rejected);
        assert_eq!(by_id[&a].verdict, Some(rejected()));
        assert_eq!(by_id[&a].issuer(), &iss("observer"));
        assert!(by_id[&a].attribution.is_verified());
        assert_eq!(by_id[&withdrawn].class, VerdictClass::Retracted, "standing decides before the verdict");
        assert_eq!(by_id[&withdrawn].standing, Standing::Retracted { by: iss("observer") });
        assert_eq!(by_id[&derived].class, VerdictClass::BasisWithdrawn);
        assert_eq!(by_id[&derived].issuer(), &iss("auditor-1"), "a withdrawn basis is not a retraction of this");
        assert_eq!(by_id[&unjudged].class, VerdictClass::Unjudged);
    }

    #[test]
    fn ranking_is_the_adopters_and_the_order_is_deterministic() {
        let mut w = World::new();
        let low = w.put("observer", RecordKind::Observation, "svc/a", "x", vec![]);
        let high = w.put("observer", RecordKind::Observation, "svc/a", "yy", vec![]);
        w.put("observer", RecordKind::Observation, "svc/a", "irrelevant", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let rank = |c: &Candidate<'_>| {
            assert_eq!(c.request.task, "triage", "the request reaches the adopter's ranking");
            match c.record.body() {
                b"irrelevant" => None,
                b => Some(b.len() as u64),
            }
        };
        let s = selector.select(&w.store, &request(), rank);
        let ids: Vec<&RecordId> = s.records.iter().map(|s| s.record.id()).collect();
        assert_eq!(ids, vec![&high, &low]);
        assert!(!s.truncated);
        let s = selector.select(&w.store, &SelectionRequest { limit: 1, ..request() }, rank);
        assert_eq!(s.records.len(), 1);
        assert!(s.truncated);
    }

    #[test]
    fn require_verified_leaves_out_an_unchecked_record_and_its_retraction() {
        let mut w = World::new();
        let verified = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        // Someone with the store put an unchecked "retraction" of the verified record, and an unchecked record.
        let fake_retraction = KnowledgeRecord::new(iss("observer"), RecordKind::Claim, NOW, "svc/a", vec![],
            vec![Link { kind: LinkKind::Retracts, target: verified.clone() }]).unwrap();
        w.store.put(fake_retraction);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).require_verified();
        let s = selector.select(&w.store, &request(), all);
        assert_eq!(s.records.len(), 1);
        assert_eq!(s.records[0].standing, Standing::Current, "an unchecked record retracts nothing here");
    }

    // ── writing a condensation back ─────────────────────────────────────────────────────────────

    /// **The exit's first case:** a condensation nobody signed is refused, and so is one signed by
    /// someone other than the issuer it names.
    #[test]
    fn a_condensed_record_is_refused_without_its_issuers_signature() {
        let mut w = World::new();
        let a1 = w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]);
        let a2 = w.put("auditor-2", RecordKind::Assessment, "svc/a", "bad too", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let draft = w.condensation("condenser", "svc/a", &[&a1, &a2]);

        let unsigned = SignedRecord { record: draft.clone(), signature: vec![] };
        let before = w.store.len();
        assert!(matches!(
            selector.write_condensed(&mut w.store, unsigned, &w.members, &w.external),
            Err(CondenseRefusal::Unattributed(_))
        ));
        let forged = SignedRecord {
            record: draft.clone(),
            signature: mycelium_core::tls::sign_bytes(&w.keys["auditor-1"], &draft.canonical_bytes()).to_vec(),
        };
        assert!(matches!(
            selector.write_condensed(&mut w.store, forged, &w.members, &w.external),
            Err(CondenseRefusal::Unattributed(_))
        ));
        let stranger = KnowledgeRecord::new(iss("someone"), RecordKind::Claim, NOW, "svc/a", vec![],
            draft.links().to_vec()).unwrap();
        let stranger = SignedRecord { record: stranger, signature: vec![0; 64] };
        assert!(matches!(
            selector.write_condensed(&mut w.store, stranger, &w.members, &w.external),
            Err(CondenseRefusal::Unattributed(_))
        ));
        assert_eq!(w.store.len(), before, "nothing refused was stored");

        // Signed by its own issuer, it is a record of its own, linked to both sources.
        let id = w.write(&selector, draft).unwrap();
        let written = w.store.get(&id).unwrap();
        assert_eq!(written.issuer(), &iss("condenser"));
        assert_eq!(written.kind(), RecordKind::Claim);
        let targets: BTreeSet<&RecordId> = written.links().iter().map(|l| &l.target).collect();
        assert_eq!(targets, [&a1, &a2].into_iter().collect());
    }

    /// **The exit's second case:** a condensation citing across a verdict boundary is refused —
    /// `Rejected` and `InsufficientEvidence` are two classes — and so is one folding a `Retracted` or
    /// `BasisWithdrawn` record into live ones.
    #[test]
    fn a_condensed_record_is_refused_across_a_verdict_boundary() {
        let mut w = World::new();
        let bad = w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]); // Rejected
        let unknown = w.put("auditor-2", RecordKind::Assessment, "svc/b", "?", vec![]); // InsufficientEvidence
        let live = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]); // Rejected
        let withdrawn = w.put("observer", RecordKind::Observation, "svc/a", "down", vec![]);
        w.put("observer", RecordKind::Claim, "svc/a", "retract",
            vec![Link { kind: LinkKind::Retracts, target: withdrawn.clone() }]);
        let derived = w.put("auditor-2", RecordKind::Assessment, "svc/a", "therefore",
            vec![Link { kind: LinkKind::DerivedFrom, target: withdrawn.clone() }]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);

        for (pair, why) in [
            ([&bad, &unknown], "rejected with insufficient evidence"),
            ([&live, &withdrawn], "a retracted record into a live one"),
            ([&live, &derived], "a basis-withdrawn record into a live one"),
        ] {
            let draft = w.condensation("condenser", "svc/a", &pair);
            let got = w.write(&selector, draft);
            assert!(matches!(got, Err(CondenseRefusal::AcrossVerdicts(_))), "{why}: {got:?}");
        }

        // The draft builder refuses the same, from the selection's classes.
        let s = selector.select(&w.store, &request(), all);
        let pick = |id: &RecordId| s.records.iter().find(|x| x.record.id() == id).unwrap();
        assert!(matches!(
            condensation_draft(&[pick(&bad), pick(&unknown)], iss("condenser"), NOW, "svc/a", vec![]),
            Err(CondenseRefusal::AcrossVerdicts(_))
        ));

        // Within one class it is written.
        let groups = s.by_class();
        let rejected: Vec<&Selected> = groups[&VerdictClass::Rejected].clone();
        assert!(rejected.iter().any(|x| x.record.id() == &bad) && rejected.iter().any(|x| x.record.id() == &live));
        let draft = condensation_draft(&rejected, iss("condenser"), NOW, "svc/a", b"both bad".to_vec()).unwrap();
        w.write(&selector, draft).unwrap();
    }

    /// The class is re-derived when the condensation is written, not taken from the selection: a
    /// source retracted between selecting and writing moves it out of its class.
    #[test]
    fn the_class_is_re_derived_at_write_time() {
        let mut w = World::new();
        let one = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let s = selector.select(&w.store, &request(), all);
        let draft = condensation_draft(&s.records.iter().collect::<Vec<_>>(), iss("condenser"), NOW, "svc/a", vec![])
            .unwrap();
        w.put("observer", RecordKind::Claim, "svc/a", "retract",
            vec![Link { kind: LinkKind::Retracts, target: one.clone() }]);
        let got = w.write(&selector, draft);
        assert!(matches!(got, Err(CondenseRefusal::AcrossVerdicts(_))), "{got:?}");
    }

    #[test]
    fn a_condensation_is_a_claim_in_scope_derived_only_from_readable_sources() {
        let mut w = World::new();
        let a = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let secret = w.put("observer", RecordKind::Observation, "payroll/q3", "secret", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let try_write = |w: &mut World, r: KnowledgeRecord| w.write(&selector, r);

        let assessment = KnowledgeRecord::new(iss("condenser"), RecordKind::Assessment, NOW, "svc/a", vec![],
            vec![Link { kind: LinkKind::DerivedFrom, target: a.clone() }]).unwrap();
        assert_eq!(try_write(&mut w, assessment), Err(CondenseRefusal::NotAClaim(RecordKind::Assessment)));

        let elsewhere = w.condensation("condenser", "payroll/summary", &[&a]);
        assert_eq!(try_write(&mut w, elsewhere), Err(CondenseRefusal::SubjectOutsideScope));

        let none = KnowledgeRecord::new(iss("condenser"), RecordKind::Claim, NOW, "svc/a", vec![], vec![]).unwrap();
        assert_eq!(try_write(&mut w, none), Err(CondenseRefusal::NoSources));

        let supports = KnowledgeRecord::new(iss("condenser"), RecordKind::Claim, NOW, "svc/a", vec![],
            vec![Link { kind: LinkKind::Supports, target: a.clone() }]).unwrap();
        assert_eq!(try_write(&mut w, supports), Err(CondenseRefusal::NotDerivedFrom(LinkKind::Supports)));

        let laundering = w.condensation("condenser", "svc/a", &[&a, &secret]);
        assert_eq!(try_write(&mut w, laundering), Err(CondenseRefusal::SourceUnreadable(secret.clone())));
        let ghost = RecordId { issuer: iss("observer"), digest: [9; 32] };
        let absent = w.condensation("condenser", "svc/a", &[&ghost]);
        assert_eq!(try_write(&mut w, absent), Err(CondenseRefusal::SourceUnreadable(ghost)));
    }

    /// A written condensation stands or falls as its own claim: a retraction of a source reaches it as
    /// `BasisWithdrawn`, through the existing correction index.
    #[test]
    fn a_retracted_source_reaches_the_condensation_as_basis_withdrawn() {
        let mut w = World::new();
        let one = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts);
        let draft = w.condensation("condenser", "svc/a", &[&one]);
        let id = w.write(&selector, draft).unwrap();
        w.put("observer", RecordKind::Claim, "svc/a", "retract",
            vec![Link { kind: LinkKind::Retracts, target: one.clone() }]);
        let s = selector.select(&w.store, &request(), all);
        let mine = s.records.iter().find(|x| x.record.id() == &id).unwrap();
        assert_eq!(mine.standing, Standing::BasisWithdrawn { basis: one, hops: 1 });
        assert_eq!(mine.class, VerdictClass::BasisWithdrawn);
    }
}
