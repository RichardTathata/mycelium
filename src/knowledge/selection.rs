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
//! for its subject, if any — computed over the records this selection admits whose standing is
//! current, so a withheld, withdrawn, expired or unverified record never reaches a count. From the last two it derives one [`VerdictClass`]. The ranking is the adopter's —
//! a closure that scores a candidate or says it is not relevant — and this module only orders by it;
//! there is no vector store and no relevance model here. Verified records only, by default, under keys
//! not revoked when they were stored — and a record derived from one the selection does not admit is
//! withheld, as one derived from an unreadable record is. Expiry is carried down `DerivedFrom`: a
//! derivation is no fresher than its oldest source.
//!
//! # Compression stays inside one class
//!
//! If the adopter condenses a selection — an LLM shortening twelve records into a paragraph — the
//! paragraph is a statement someone made, so it goes back as a record of its own:
//! [`Selector::write_condensed`] accepts it only
//!
//! - **signed by its own issuer** under the signature presented now, under a key the reader does not
//!   hold revoked (an unsigned condensation is a record nobody signed, which is what D14 exists to
//!   refuse);
//! - as a [`RecordKind::Claim`], so it never counts as evidence for anything: a condensation of
//!   assessments is not an assessment;
//! - linked by `DerivedFrom` to **every** record it drew on, and by nothing else, so a retraction of
//!   any source reaches it as `BasisWithdrawn` through the existing correction index;
//! - when **every source is in one class**, re-derived here at the moment of writing rather than taken
//!   from the selection that proposed it — an `InsufficientEvidence` record and a `Rejected` one are
//!   two classes, *we do not know* and *we looked and it is bad*, and a `Retracted` or
//!   `BasisWithdrawn` record is never in the class of a live one;
//! - **about one subject — every source's**: a condensation's class is then its sources' class at
//!   every later read, whatever the verdict for that subject becomes and whoever reads it, since both
//!   take the one subject's verdict and the condensation's standing follows its sources'. A summary
//!   over several subjects is a selection, returned as one; written as a record it is not built;
//! - with **no retracted source**: a condensation of it would be basis-withdrawn, not retracted, and
//!   only its issuer retracts. Basis-withdrawn sources give a basis-withdrawn condensation — and
//!   withdrawn records collapse into one class, whatever the verdict they were about;
//! - with **no expired source**, and an `at_ms` no later than the reader's clock plus the HLC's drift
//!   bound and inside the window: a condensation of expired records would be a fresh statement of
//!   stale ones, which the semantic gate forbids, and one dated ahead would outlive its sources.
//!
//! # The mandate scopes what can be read, and the scope is one rule
//!
//! A selection is taken through a [`ReadScope`], built from the caller's [`Mandate`]: enumerating
//! [`KNOWLEDGE_READ`], with a scope that is a plain path, and used only inside the mandate's window —
//! checked when the scope is built, again when a [`Selector`] is built and on every [`ReadScope::view`].
//! A record is readable under it when its subject is the mandate's scope or lies beneath it
//! (`scope/…`, by path segment; a subject holding an empty, `.` or `..` segment is never covered),
//! **and** every record it was derived from is held and readable too. The hierarchical match is a
//! selection-only rule: everywhere else a mandate's scope is matched exactly. The second half is
//! what stops a condensation laundering content: a record derived from something the caller cannot
//! read is withheld, because its text may be that something. A source the store does not hold is
//! unreadable by the same rule — unknown is not readable — and the selection reports both cases with
//! one reason, so withholding does not tell the caller whether an out-of-scope record exists.
//!
//! [`ScopedView`] is the direct read under the same rule ([`ScopedView::get`], [`ScopedView::about`]),
//! and the selector reads only through it, so the selection cannot see a record the caller could not
//! read directly. The [`KnowledgeStore`]'s own methods stay unscoped: they are the store holder's,
//! and no door — no HTTP route, no SDK verb — reads knowledge records today. A selected record is
//! returned unchanged, so a non-`DerivedFrom` link names its target's id even when the target is not
//! readable ([`Selected`]). Standing is computed over the whole store, so a retraction filed under
//! another subject still reaches the record it withdraws: the selector is never more optimistic than
//! the store.
//!
//! Subjects are opaque strings: the path rule is literal (`/` separates, an empty, `.` or `..`
//! segment is refused), so `%2e%2e` or a Unicode dot leader is an ordinary segment. An adopter that
//! decodes subjects must normalise them before filing.
//!
//! # Not here
//!
//! A condensation across subjects (see above). The wiki: its pages carry no issuer, verdict or standing, so selecting one with its provenance
//! would mean filing it as a record first. A read mandate's epoch and revocation standing: a
//! knowledge store installs no epoch, so a read scope checks the window and the operation only.
//! Re-verifying a selected record against the reader's current key view, which
//! [`classify_eligible`](super::resolution::classify_eligible) does for evidence.

use std::collections::{BTreeMap, BTreeSet};

use crate::mandate::{Mandate, MandateRefusal};

use super::correction::{standing, DependencyIndex, Standing};
use super::issuer::{verify_issuer, Authenticity, MemberKeySource, TrustedExternalIssuers};
use super::resolution::Verdict;
use super::store::{Attribution, KnowledgeStore, PutRefusal, SignedRecord};
use super::{IssuerId, KnowledgeRecord, Link, LinkKind, RecordError, RecordId, RecordKind};

/// The operation a mandate enumerates to read knowledge records into context.
pub const KNOWLEDGE_READ: &str = "knowledge.read";

/// Why a mandate gives no read scope.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadScopeRefusal {
    /// The mandate is out of its window, or does not enumerate [`KNOWLEDGE_READ`].
    Mandate(MandateRefusal),
    /// The mandate's scope is not a usable path: empty, `/`, or holding an empty, `.` or `..` segment.
    UnusableScope(String),
}

impl std::fmt::Display for ReadScopeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mandate(r) => write!(f, "{r}"),
            Self::UnusableScope(s) => write!(f, "the mandate's scope {s:?} is not a usable read scope"),
        }
    }
}

impl std::error::Error for ReadScopeRefusal {}

/// Does every `/`-separated segment of `path` name something — none empty, none `.` or `..`?
fn plain_segments(path: &str) -> bool {
    path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// What the caller's mandate lets it read. Built only from a mandate that enumerates
/// [`KNOWLEDGE_READ`], and usable only inside that mandate's window — checked again at every use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadScope {
    /// The mandate's scope, without a trailing `/`.
    base: String,
    valid_from_ms: u64,
    valid_until_ms: u64,
}

impl ReadScope {
    /// The read scope of `mandate` at `now_ms`, or the refusal the mandate earns.
    ///
    /// Outside its window → [`MandateRefusal::OutOfWindow`]; not enumerating [`KNOWLEDGE_READ`] →
    /// [`MandateRefusal::NotEnumerated`]; a scope that is empty, `/`, or holds an empty, `.` or `..`
    /// segment → [`ReadScopeRefusal::UnusableScope`]. The window is kept and re-checked by
    /// [`view`](Self::view) and [`Selector::new`], so a scope does not outlive its mandate.
    pub fn from_mandate(mandate: &Mandate, now_ms: u64) -> Result<Self, ReadScopeRefusal> {
        if !mandate.permits(KNOWLEDGE_READ) {
            return Err(ReadScopeRefusal::Mandate(MandateRefusal::NotEnumerated {
                operation: KNOWLEDGE_READ.to_string(),
            }));
        }
        let base = mandate.scope.strip_suffix('/').unwrap_or(&mandate.scope);
        if !plain_segments(base) {
            return Err(ReadScopeRefusal::UnusableScope(mandate.scope.clone()));
        }
        let scope = Self {
            base: base.to_string(),
            valid_from_ms: mandate.valid_from_ms,
            valid_until_ms: mandate.valid_until_ms,
        };
        scope.check_window(now_ms).map_err(ReadScopeRefusal::Mandate)?;
        Ok(scope)
    }

    /// The mandate's scope, without a trailing `/`.
    pub fn scope(&self) -> &str {
        &self.base
    }

    /// Is `now_ms` inside the mandate's window?
    pub fn check_window(&self, now_ms: u64) -> Result<(), MandateRefusal> {
        if now_ms >= self.valid_from_ms && now_ms <= self.valid_until_ms {
            Ok(())
        } else {
            Err(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: self.valid_until_ms })
        }
    }

    /// Is `subject` the scope, or beneath it by path segment? `svc` covers `svc` and `svc/health`,
    /// never `svcx`, and never a subject holding an empty, `.` or `..` segment (`svc/../payroll`).
    ///
    /// **A selection-only rule.** Everywhere else a mandate's scope is matched exactly (a resource's
    /// [`WrongScope`](MandateRefusal::WrongScope), the action evaluator's mandate check); here a
    /// knowledge subject is a path, and a mandate over `svc` reads what is filed beneath it.
    pub fn covers(&self, subject: &str) -> bool {
        plain_segments(subject)
            && (subject == self.base
                || subject.strip_prefix(self.base.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }

    /// The store, as this scope may read it at `now_ms`.
    pub fn view<'a>(&self, store: &'a KnowledgeStore, now_ms: u64) -> Result<ScopedView<'a>, MandateRefusal> {
        self.view_admitting(store, now_ms, &|_| true)
    }

    /// The view, counting only records `admit` accepts — as sources too: a record derived from one
    /// `admit` refuses is withheld, so a selector that admits only verified records cannot be handed
    /// unverified content inside a verified record.
    fn view_admitting<'a>(
        &self,
        store: &'a KnowledgeStore,
        now_ms: u64,
        admit: &dyn Fn(&RecordId) -> bool,
    ) -> Result<ScopedView<'a>, MandateRefusal> {
        self.check_window(now_ms)?;
        // A fixpoint over the covered records: drop any whose `DerivedFrom` source is not held or not
        // (still) readable, until nothing changes. Cycle-safe: a cycle of covered records whose
        // sources are all covered stays readable, and every member of it was checked for coverage.
        // Rebuilt per call; at worst quadratic in the covered records (a derivation chain dropped one
        // link per pass).
        let mut readable: BTreeSet<RecordId> = store
            .records()
            .filter(|r| self.covers(r.subject()) && admit(r.id()))
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
        Ok(ScopedView { store, readable, withheld })
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

    /// A store holding only the readable records, with their attributions and signatures.
    pub fn to_store(&self) -> KnowledgeStore {
        self.store_where(|_| true)
    }

    /// A store holding the readable records `keep` accepts, with their attributions and signatures.
    fn store_where(&self, keep: impl Fn(&KnowledgeRecord) -> bool) -> KnowledgeStore {
        let mut out = KnowledgeStore::new();
        for record in self.records().filter(|r| keep(r)) {
            let attribution = self.store.attribution(record.id());
            match (attribution, self.store.signature(record.id())) {
                (Some(a @ Attribution::Verified { .. }), Some(sig)) => {
                    out.insert_verified(SignedRecord { record: record.clone(), signature: sig.to_vec() }, a.clone());
                }
                _ => out.put(record.clone()),
            }
        }
        out
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
///
/// The record is returned **unchanged**, so its links are too: a `Supports`, `Challenges` or
/// `Retracts` link names its target's id (issuer and content digest) even where the target is not
/// readable under the scope. The id is not the target's content, and the target itself cannot be read
/// through the view; a `DerivedFrom` link to an unreadable target withholds the record instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    /// The record, unchanged — its issuer in it and in its id.
    pub record: KnowledgeRecord,
    /// How it was attributed when stored.
    pub attribution: Attribution,
    /// Where it stands now.
    pub standing: Standing,
    /// The reader's verdict for its subject, if any, computed over the readable records only.
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
    /// No admissible path attributes the condensation to its issuer under the signature presented:
    /// unsigned, signed by someone else, or by an issuer the reader does not know. A condensation
    /// nobody signed is not a record.
    Unattributed(PutRefusal),
    /// Signed by its issuer under a key the reader holds validly revoked: authentic, with no present
    /// standing to say anything new.
    RevokedKey,
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
    /// A source is older than the reader's window. A condensation of expired records would be a
    /// fresh statement of stale ones — expired evidence refreshed, which the semantic gate forbids.
    ExpiredSource(RecordId),
    /// A source's subject is not the condensation's. One subject per condensation, so its class is its
    /// sources' class at every read; a summary over several subjects is a selection.
    SubjectMismatch(RecordId),
    /// A source was withdrawn by its issuer. A condensation of it would be basis-withdrawn, not
    /// retracted, and only its issuer retracts.
    RetractedSource(RecordId),
    /// Its sources are in more than one class.
    AcrossVerdicts(Vec<(RecordId, VerdictClass)>),
    /// Its `at_ms` is later than the reader's clock plus the drift bound, or older than the window: a
    /// condensation dated in the future would outlive its sources, one dated past the window would be
    /// expired while they are not.
    UnusableTime {
        /// The condensation's `at_ms`.
        at_ms: u64,
    },
    /// The record itself is malformed (building a draft).
    Malformed(RecordError),
    /// The selector's mandate is outside its window.
    Mandate(MandateRefusal),
}

impl std::fmt::Display for CondenseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unattributed(r) => write!(f, "a condensation must be signed by its own issuer: {r}"),
            Self::RevokedKey => write!(f, "the condensation is signed under a revoked key"),
            Self::NotAClaim(k) => write!(f, "a condensation is a claim, never {k:?}"),
            Self::NoSources => write!(f, "a condensation must cite every record it drew on"),
            Self::NotDerivedFrom(k) => write!(f, "a condensation links to its sources by DerivedFrom only, not {k:?}"),
            Self::SubjectOutsideScope => write!(f, "the condensation's subject is outside the caller's scope"),
            Self::SourceUnreadable(id) => write!(f, "source {id} is not readable under the caller's scope"),
            Self::ExpiredSource(id) => write!(f, "source {id} is expired; a condensation does not refresh it"),
            Self::AcrossVerdicts(classes) => {
                write!(f, "a condensation draws from one class; its sources are in")?;
                let distinct: BTreeSet<VerdictClass> = classes.iter().map(|(_, c)| *c).collect();
                for c in distinct {
                    write!(f, " {c:?}")?;
                }
                Ok(())
            }
            Self::SubjectMismatch(id) => write!(f, "source {id} is about another subject than the condensation"),
            Self::RetractedSource(id) => write!(f, "source {id} was retracted by its issuer"),
            Self::UnusableTime { at_ms } => write!(f, "the condensation's at_ms {at_ms} is outside the reader's window"),
            Self::Malformed(e) => write!(f, "malformed condensation: {e}"),
            Self::Mandate(r) => write!(f, "{r}"),
        }
    }
}

impl std::error::Error for CondenseRefusal {}

/// The reader's verdict for a subject, computed over the store it is handed — the readable records
/// only, never the store the selector was given.
pub type VerdictSource<'v> = dyn Fn(&KnowledgeStore, &str) -> Option<Verdict> + 'v;

/// A reader's selector: one scope, one clock, one window, one verdict source.
pub struct Selector<'v> {
    scope: ReadScope,
    now_ms: u64,
    max_age_ms: u64,
    require_verified: bool,
    max_clock_drift_ms: u64,
    verdicts: &'v VerdictSource<'v>,
}

impl<'v> Selector<'v> {
    /// A selector reading through `scope` at `now_ms`, records older than `max_age_ms` being
    /// `Expired`. Refused when `now_ms` is outside the scope's mandate window.
    ///
    /// `verdicts` is the reader's verdict for a subject — typically
    /// [`classify_eligible`](super::resolution::classify_eligible) over the release the subject names
    /// — or `None` where the reader has none. It is called with a store holding **only the readable
    /// records** ([`ScopedView::to_store`]), so a withheld record cannot reach a count or a
    /// `Challenged { by }`; a closure that ignores that store and reads another one defeats this.
    ///
    /// Verified records only, by default ([`admit_unchecked`](Self::admit_unchecked) to widen).
    pub fn new(
        scope: ReadScope,
        now_ms: u64,
        max_age_ms: u64,
        verdicts: &'v VerdictSource<'v>,
    ) -> Result<Self, MandateRefusal> {
        scope.check_window(now_ms)?;
        Ok(Self {
            scope,
            now_ms,
            max_age_ms,
            require_verified: true,
            max_clock_drift_ms: mycelium_core::hlc::DEFAULT_MAX_CLOCK_DRIFT_MS,
            verdicts,
        })
    }

    /// Also select records stored without verification ([`Attribution::Unchecked`]), and let them
    /// retract and derive. Off by default: an unchecked record's issuer is a claim nobody verified, and
    /// "issuer intact" means an issuer the reader established.
    pub fn admit_unchecked(mut self) -> Self {
        self.require_verified = false;
        self
    }

    /// How far ahead of this selector's clock a condensation's `at_ms` may be — the HLC's drift bound by
    /// default ([`DEFAULT_MAX_CLOCK_DRIFT_MS`](mycelium_core::hlc::DEFAULT_MAX_CLOCK_DRIFT_MS)); a node
    /// configured with another `max_clock_drift_ms` passes its own.
    pub fn with_max_clock_drift_ms(mut self, ms: u64) -> Self {
        self.max_clock_drift_ms = ms;
        self
    }

    /// Does this attribution have present standing here? Verified, under a key not revoked at
    /// storage — or anything, once unchecked records are admitted.
    fn admits(&self, attribution: Option<&Attribution>) -> bool {
        match attribution {
            Some(Attribution::Verified { revoked_at_storage, .. }) => !*revoked_at_storage || !self.require_verified,
            Some(_) => !self.require_verified,
            None => false,
        }
    }

    fn index(&self, store: &KnowledgeStore) -> DependencyIndex {
        if self.require_verified {
            DependencyIndex::build_filtered(store, |r| self.admits(store.attribution(r.id())))
        } else {
            DependencyIndex::build(store)
        }
    }

    fn view<'a>(&self, store: &'a KnowledgeStore) -> Result<ScopedView<'a>, MandateRefusal> {
        self.scope.view_admitting(store, self.now_ms, &|id| self.admits(store.attribution(id)))
    }

    /// A record's standing, with expiry carried down `DerivedFrom`: a derivation is no fresher than its
    /// basis. Its own `at_ms` is the issuer's claim; its sources' are the only times it can be bound to,
    /// so once any record it derives from (transitively, held in the store) is past the window, so is it.
    fn standing_of(&self, store: &KnowledgeStore, index: &DependencyIndex, id: &RecordId) -> Option<Standing> {
        let st = standing(store, index, id, self.now_ms, self.max_age_ms)?;
        if st != Standing::Current {
            return Some(st);
        }
        let mut seen: BTreeSet<RecordId> = BTreeSet::new();
        let mut stack: Vec<RecordId> = vec![id.clone()];
        while let Some(current) = stack.pop() {
            let Some(record) = store.get(&current) else { continue };
            for l in record.links().iter().filter(|l| l.kind == LinkKind::DerivedFrom) {
                if !seen.insert(l.target.clone()) {
                    continue;
                }
                if let Some(basis) = store.get(&l.target) {
                    if self.now_ms.saturating_sub(basis.at_ms()) > self.max_age_ms {
                        return Some(Standing::Expired { at_ms: basis.at_ms() });
                    }
                    stack.push(l.target.clone());
                }
            }
        }
        Some(Standing::Current)
    }

    /// The store a verdict is computed over: the records this selection admits whose standing, over the
    /// whole store, is current — so a withheld, withdrawn, expired or (verified-only) unchecked record
    /// never counts toward a verdict or names a challenger.
    fn verdict_store(&self, store: &KnowledgeStore, view: &ScopedView<'_>, index: &DependencyIndex) -> KnowledgeStore {
        view.store_where(|r| self.standing_of(store, index, r.id()) == Some(Standing::Current))
    }

    /// **Select records for a run.** Reads only through this selector's [`ScopedView`]; scores each
    /// readable record with `rank` (`None` = not relevant) and returns the top `request.limit`.
    pub fn select(
        &self,
        store: &KnowledgeStore,
        request: &SelectionRequest,
        rank: impl Fn(&Candidate<'_>) -> Option<u64>,
    ) -> Selection {
        // `new` checked the window at this selector's `now_ms`, which does not move.
        let Ok(view) = self.view(store) else { return Selection::default() };
        let index = self.index(store);
        let scoped = self.verdict_store(store, &view, &index);
        let mut verdicts: BTreeMap<&str, Option<Verdict>> = BTreeMap::new();
        let mut out: Vec<Selected> = Vec::new();
        for record in view.records() {
            let Some(attribution) = store.attribution(record.id()) else { continue };
            if !self.admits(Some(attribution)) {
                continue;
            }
            let Some(st) = self.standing_of(store, &index, record.id()) else { continue };
            let verdict = verdicts
                .entry(record.subject())
                .or_insert_with(|| (self.verdicts)(&scoped, record.subject()))
                .clone();
            let class = VerdictClass::of(&st, verdict.as_ref());
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

    /// Is `at_ms` a time this reader can file a condensation at: no later than its clock plus the drift
    /// bound, and inside the window?
    fn usable_time(&self, at_ms: u64) -> Result<(), CondenseRefusal> {
        let future = at_ms > self.now_ms.saturating_add(self.max_clock_drift_ms);
        let stale = self.now_ms.saturating_sub(at_ms) > self.max_age_ms;
        if future || stale { Err(CondenseRefusal::UnusableTime { at_ms }) } else { Ok(()) }
    }

    /// The class rules over a condensation's sources, given each source's id, subject and class: every
    /// source about `subject`, none retracted or expired, all in one class.
    fn sources_check(&self, subject: &str, sources: &[(RecordId, &str, VerdictClass)]) -> Result<(), CondenseRefusal> {
        if sources.is_empty() {
            return Err(CondenseRefusal::NoSources);
        }
        for (id, source_subject, class) in sources {
            if *source_subject != subject {
                return Err(CondenseRefusal::SubjectMismatch(id.clone()));
            }
            match class {
                VerdictClass::Retracted => return Err(CondenseRefusal::RetractedSource(id.clone())),
                VerdictClass::Expired => return Err(CondenseRefusal::ExpiredSource(id.clone())),
                _ => {}
            }
        }
        let distinct: BTreeSet<VerdictClass> = sources.iter().map(|(_, _, c)| *c).collect();
        if distinct.len() > 1 {
            return Err(CondenseRefusal::AcrossVerdicts(sources.iter().map(|(id, _, c)| (id.clone(), *c)).collect()));
        }
        Ok(())
    }

    /// Build the unsigned draft of a condensation over `sources` (from a selection): a claim by
    /// `issuer`, linked by `DerivedFrom` to each source once, under the same rules as
    /// [`check_condensed`](Self::check_condensed) — in scope, every source about `subject`, none
    /// retracted or expired, one class, `at_ms` usable. Advisory: the write re-derives everything.
    pub fn condensation_draft(
        &self,
        sources: &[&Selected],
        issuer: IssuerId,
        at_ms: u64,
        subject: impl Into<String>,
        body: Vec<u8>,
    ) -> Result<KnowledgeRecord, CondenseRefusal> {
        let subject = subject.into();
        if !self.scope.covers(&subject) {
            return Err(CondenseRefusal::SubjectOutsideScope);
        }
        self.usable_time(at_ms)?;
        let checked: Vec<(RecordId, &str, VerdictClass)> =
            sources.iter().map(|s| (s.record.id().clone(), s.record.subject(), s.class)).collect();
        self.sources_check(&subject, &checked)?;
        let targets: BTreeSet<RecordId> = sources.iter().map(|s| s.record.id().clone()).collect();
        let links = targets.into_iter().map(|target| Link { kind: LinkKind::DerivedFrom, target }).collect();
        KnowledgeRecord::new(issuer, RecordKind::Claim, at_ms, subject, body, links).map_err(CondenseRefusal::Malformed)
    }

    /// **Check a condensation without writing it.** Every rule in the module docs, re-derived now
    /// against `store`: the presented signature its own issuer's under a key not revoked, a claim, its
    /// subject in scope, its `at_ms` usable, linked by `DerivedFrom` only and to at least one source,
    /// every source readable and admitted, about the condensation's own subject, none retracted or
    /// expired, all in one class.
    pub fn check_condensed(
        &self,
        store: &KnowledgeStore,
        signed: &SignedRecord,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> Result<(), CondenseRefusal> {
        // Integrity (the id is the content's digest) — and, for a record not yet held, attribution.
        store.verify_signed(signed, members, external).map_err(CondenseRefusal::Unattributed)?;
        // The signature presented now, whether or not the store already holds the record: a held
        // condensation re-presented unsigned is not a write by its issuer.
        match verify_issuer(&signed.record, &signed.signature, members, external) {
            Authenticity::Current { .. } => {}
            Authenticity::Revoked { .. } => return Err(CondenseRefusal::RevokedKey),
            Authenticity::Unverifiable(reason) => {
                return Err(CondenseRefusal::Unattributed(PutRefusal::Unverifiable(reason)))
            }
        }
        let record = &signed.record;
        if record.kind() != RecordKind::Claim {
            return Err(CondenseRefusal::NotAClaim(record.kind()));
        }
        if !self.scope.covers(record.subject()) {
            return Err(CondenseRefusal::SubjectOutsideScope);
        }
        self.usable_time(record.at_ms())?;
        if record.links().is_empty() {
            return Err(CondenseRefusal::NoSources);
        }
        if let Some(l) = record.links().iter().find(|l| l.kind != LinkKind::DerivedFrom) {
            return Err(CondenseRefusal::NotDerivedFrom(l.kind));
        }
        let view = self.view(store).map_err(CondenseRefusal::Mandate)?;
        let index = self.index(store);
        let scoped = self.verdict_store(store, &view, &index);
        let mut checked: Vec<(RecordId, &str, VerdictClass)> = Vec::new();
        for link in record.links() {
            let unreadable = || CondenseRefusal::SourceUnreadable(link.target.clone());
            let source = view.get(&link.target).ok_or_else(unreadable)?;
            let st = self.standing_of(store, &index, source.id()).ok_or_else(unreadable)?;
            let class = VerdictClass::of(&st, (self.verdicts)(&scoped, source.subject()).as_ref());
            checked.push((source.id().clone(), source.subject(), class));
        }
        self.sources_check(record.subject(), &checked)
    }

    /// **Write a condensation back** into `store`: [`check_condensed`](Self::check_condensed), then
    /// [`KnowledgeStore::put_signed`]. For a durable store, check here and then put through
    /// `DurableKnowledgeStore::put_signed`, which verifies again before it persists — the store can
    /// change between the two calls (a source retracted), so a caller wanting no window holds its store
    /// exclusively across both.
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
    fn verdicts(_readable: &KnowledgeStore, subject: &str) -> Option<Verdict> {
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
            Err(ReadScopeRefusal::Mandate(MandateRefusal::NotEnumerated { operation: KNOWLEDGE_READ.into() }))
        );
        assert!(matches!(
            ReadScope::from_mandate(&mandate("svc", &[KNOWLEDGE_READ]), NOW + 2 * DAY),
            Err(ReadScopeRefusal::Mandate(MandateRefusal::OutOfWindow { .. }))
        ));
        let s = read("svc");
        assert!(s.covers("svc") && s.covers("svc/health") && s.covers("svc/a/b"));
        assert!(!s.covers("svcx") && !s.covers("other/svc") && !s.covers(""));
        assert!(read("svc/").covers("svc/health"), "a trailing slash names the same scope");
        for unusable in ["", "/", "svc/../payroll", "./svc", "svc//a"] {
            assert_eq!(
                ReadScope::from_mandate(&mandate(unusable, &[KNOWLEDGE_READ]), NOW),
                Err(ReadScopeRefusal::UnusableScope(unusable.into())),
                "{unusable:?}"
            );
        }
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

        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let selection = selector.select(&w.store, &request(), all);
        let ids: Vec<&RecordId> = selection.records.iter().map(|s| s.record.id()).collect();
        assert_eq!(ids, vec![&inside], "only the record the mandate covers, with its own sources readable");
        let mut withheld = vec![laundered.clone(), orphan.clone()];
        withheld.sort();
        assert_eq!(selection.withheld, withheld, "one reason for both, so it does not say which");

        // The direct read under the same scope agrees with the selection.
        let view = read("svc").view(&w.store, NOW).unwrap();
        assert!(view.get(&outside).is_none() && view.get(&laundered).is_none() && view.get(&orphan).is_none());
        assert_eq!(view.about("svc/a").len(), 1);

        // A scope that covers payroll reads the out-of-scope record, and nothing under `svc`.
        let payroll = read("payroll").view(&w.store, NOW).unwrap();
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

        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
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
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
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
        // Someone with the store put an unchecked "retraction" of the verified record.
        let fake_retraction = KnowledgeRecord::new(iss("observer"), RecordKind::Claim, NOW, "svc/a", vec![],
            vec![Link { kind: LinkKind::Retracts, target: verified.clone() }]).unwrap();
        w.store.put(fake_retraction);
        // Verified only is the default (review question 7).
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let s = selector.select(&w.store, &request(), all);
        assert_eq!(s.records.len(), 1);
        assert_eq!(s.records[0].standing, Standing::Current, "an unchecked record retracts nothing here");
        // Admitting unchecked records is a choice the caller makes by name.
        let wide = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap().admit_unchecked();
        let s = wide.select(&w.store, &request(), all);
        assert_eq!(s.records.len(), 2);
        let v = s.records.iter().find(|x| x.record.id() == &verified).unwrap();
        assert_eq!(v.standing, Standing::Retracted { by: iss("observer") });
    }

    // ── the review of #683 ──────────────────────────────────────────────────────────────────────

    /// Review finding 1: a condensation's subject was free, so filing a `Rejected` record's summary under an
    /// `InsufficientEvidence` subject crossed the boundary in two writes. Since the confirmation round the
    /// rule is one subject per condensation, which makes its class its sources' at every read.
    #[test]
    fn a_condensation_is_refused_unless_its_own_class_is_its_sources_class() {
        let mut w = World::new();
        let bad = w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]); // Rejected
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let switched = w.condensation("condenser", "svc/b", &[&bad]); // would be InsufficientEvidence
        assert_eq!(
            w.write(&selector, switched),
            Err(CondenseRefusal::SubjectMismatch(bad.clone())),
            "a Rejected source filed under an InsufficientEvidence subject"
        );
        let s = selector.select(&w.store, &request(), all);
        let bad_s = s.records.iter().find(|x| x.record.id() == &bad).unwrap();
        assert!(matches!(
            selector.condensation_draft(&[bad_s], iss("condenser"), NOW, "svc/b", vec![]),
            Err(CondenseRefusal::SubjectMismatch(_))
        ), "the draft builder applies the same rule");
        let same = w.condensation("condenser", "svc/a", &[&bad]);
        w.write(&selector, same).expect("filed under its own class's subject");

        // Retracted sources: the condensation would be basis-withdrawn, not retracted — only their issuer
        // retracts — so it is refused; basis-withdrawn sources give a basis-withdrawn condensation.
        let gone = w.put("observer", RecordKind::Observation, "svc/a", "down", vec![]);
        w.put("observer", RecordKind::Claim, "svc/a", "retract", vec![Link { kind: LinkKind::Retracts, target: gone.clone() }]);
        assert_eq!(
            w.write(&selector, w.condensation("condenser", "svc/a", &[&gone])),
            Err(CondenseRefusal::RetractedSource(gone.clone()))
        );
        let downstream = w.put("auditor-2", RecordKind::Assessment, "svc/a", "therefore",
            vec![Link { kind: LinkKind::DerivedFrom, target: gone.clone() }]);
        w.write(&selector, w.condensation("condenser", "svc/a", &[&downstream])).expect("withdrawn stays withdrawn");
    }

    /// Review finding 1, expiry: a condensation over expired records was a new current record — expired evidence
    /// refreshed, which the semantic gate forbids.
    #[test]
    fn a_condensation_over_expired_records_is_refused() {
        let mut w = World::new();
        let old = KnowledgeRecord::new(iss("observer"), RecordKind::Observation, NOW - 3 * DAY, "svc/c", b"old".to_vec(), vec![]).unwrap();
        let old_id = old.id().clone();
        let signed = w.sign(old);
        w.store.put_signed(signed, &w.members, &w.external).unwrap();
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        assert_eq!(
            w.write(&selector, w.condensation("condenser", "svc/c", &[&old_id])),
            Err(CondenseRefusal::ExpiredSource(old_id.clone()))
        );
    }

    /// Review finding 2: a key revoked at storage still wrote a condensation, selected under `require_verified`.
    #[test]
    fn a_condensation_signed_under_a_revoked_key_is_refused() {
        let mut w = World::new();
        let a = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let sk = SigningKey::from_bytes(&[99; 32]);
        let pk = sk.verifying_key().to_bytes();
        let n = NodeId::new("127.0.0.1", 7001).unwrap();
        w.members.insert(n.clone(), MemberKeys { retained: vec![pk], revoked: [pk].into_iter().collect() });
        let rec = KnowledgeRecord::new(IssuerId::for_node(&n), RecordKind::Claim, NOW, "svc/a", b"x".to_vec(),
            vec![Link { kind: LinkKind::DerivedFrom, target: a.clone() }]).unwrap();
        let sig = mycelium_core::tls::sign_bytes(&sk, &rec.canonical_bytes()).to_vec();
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let before = w.store.len();
        assert_eq!(
            selector.write_condensed(&mut w.store, SignedRecord { record: rec, signature: sig }, &w.members, &w.external),
            Err(CondenseRefusal::RevokedKey)
        );
        assert_eq!(w.store.len(), before);
    }

    /// Review finding 9: an already-held condensation presented again with no signature passed as a write.
    #[test]
    fn a_held_condensation_presented_unsigned_is_refused() {
        let mut w = World::new();
        let a = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let draft = w.condensation("condenser", "svc/a", &[&a]);
        w.write(&selector, draft.clone()).unwrap();
        let unsigned = SignedRecord { record: draft, signature: vec![] };
        assert!(matches!(
            selector.write_condensed(&mut w.store, unsigned, &w.members, &w.external),
            Err(CondenseRefusal::Unattributed(_))
        ));
    }

    /// Review finding 6: `svc` covered `svc/../payroll/q3`.
    #[test]
    fn a_dot_segment_is_never_covered() {
        let s = read("svc");
        assert!(!s.covers("svc/../payroll/q3") && !s.covers("svc/./a") && !s.covers("svc//a"));
    }

    /// Review finding 4: a read scope outlived its mandate's window.
    #[test]
    fn a_selection_past_the_mandates_window_reads_nothing() {
        let mut w = World::new();
        w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        assert!(matches!(
            Selector::new(read("svc"), NOW + 365 * DAY, 1000 * DAY, &verdicts),
            Err(MandateRefusal::OutOfWindow { .. })
        ));
        assert!(read("svc").view(&w.store, NOW + 365 * DAY).is_err(), "the direct read re-checks the window too");
    }

    /// Review finding 3: the verdict leg was computed over the unscoped store, so a withheld record counted.
    #[test]
    fn a_verdict_is_computed_over_what_the_scope_can_read() {
        let mut w = World::new();
        let outside = w.put("observer", RecordKind::Observation, "payroll/q3", "secret", vec![]);
        w.put("auditor-1", RecordKind::Assessment, "svc/a", "about payroll",
            vec![Link { kind: LinkKind::DerivedFrom, target: outside.clone() }]);
        let inside = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        // The verdict source counts assessments over the store it is handed — the readable records.
        let v = |readable: &KnowledgeStore, subject: &str| -> Option<Verdict> {
            let n = readable.records().filter(|r| r.subject() == subject && r.kind() == RecordKind::Assessment).count();
            Some(Verdict::InsufficientEvidence { have: n, need: 2, independent: n, need_independent: 2 })
        };
        let selector = Selector::new(read("svc"), NOW, DAY, &v).unwrap();
        let s = selector.select(&w.store, &request(), all);
        let got = s.records.iter().find(|x| x.record.id() == &inside).unwrap();
        assert!(matches!(got.verdict, Some(Verdict::InsufficientEvidence { have: 0, .. })), "{:?}", got.verdict);
    }

    // ── the confirmation round on #683 ──────────────────────────────────────────────────────────

    /// A verdict source that mirrors `classify`: standing over the store it is handed; a current
    /// assessment whose body is `bad` makes its subject Rejected, any other current assessment
    /// InsufficientEvidence, none no verdict.
    fn live_verdict(st: &KnowledgeStore, subject: &str) -> Option<Verdict> {
        let idx = DependencyIndex::build(st);
        let cur: Vec<&KnowledgeRecord> = st
            .records()
            .filter(|r| r.subject() == subject && r.kind() == RecordKind::Assessment)
            .filter(|r| matches!(standing(st, &idx, r.id(), NOW, 1000 * DAY), Some(Standing::Current)))
            .collect();
        if cur.iter().any(|r| r.body() == b"bad") {
            Some(rejected())
        } else if !cur.is_empty() {
            Some(insufficient())
        } else {
            None
        }
    }

    /// Confirmation A: a condensation filed under another subject took that subject's verdict at every
    /// later read, so a verdict drift after the write re-opened the two-write blend. One subject per
    /// condensation makes its class its sources' class at every read.
    #[test]
    fn a_condensation_is_filed_under_its_sources_subject() {
        let mut w = World::new();
        let bad_a = w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]);
        w.put("auditor-2", RecordKind::Assessment, "svc/b", "bad", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &live_verdict).unwrap();
        // Both subjects Rejected now, so a class comparison alone accepted this.
        let elsewhere = w.write(&selector, w.condensation("condenser", "svc/b", &[&bad_a]));
        assert!(elsewhere.is_err(), "a condensation under another subject than its source's: {elsewhere:?}");
        w.write(&selector, w.condensation("condenser", "svc/a", &[&bad_a])).expect("under its source's subject");
    }

    /// Confirmation B: a retraction filed under another subject reached the record's standing but not the
    /// verdict, so a withdrawn record still made its subject Rejected.
    #[test]
    fn a_withdrawn_record_never_counts_toward_a_verdict() {
        let mut w = World::new();
        let bad_b = w.put("auditor-2", RecordKind::Assessment, "svc/b", "bad", vec![]);
        let obs = w.put("observer", RecordKind::Observation, "svc/b", "fine", vec![]);
        w.put("auditor-2", RecordKind::Claim, "other/x", "retract",
            vec![Link { kind: LinkKind::Retracts, target: bad_b.clone() }]);
        let selector = Selector::new(read("svc"), NOW, DAY, &live_verdict).unwrap();
        let s = selector.select(&w.store, &request(), all);
        let get = |id: &RecordId| s.records.iter().find(|x| x.record.id() == id).unwrap();
        assert_eq!(get(&bad_b).class, VerdictClass::Retracted);
        assert_eq!(get(&obs).class, VerdictClass::Unjudged, "a retracted assessment decides nothing");
    }

    /// Confirmation C: an unchecked record reached the verdict under the verified-only default.
    #[test]
    fn an_unchecked_record_never_counts_toward_a_verdict_under_verified_only() {
        let mut w = World::new();
        let obs = w.put("observer", RecordKind::Observation, "svc/c", "fine", vec![]);
        let forged = KnowledgeRecord::new(iss("auditor-1"), RecordKind::Assessment, NOW - 10, "svc/c", b"bad".to_vec(), vec![]).unwrap();
        w.store.put(forged);
        let selector = Selector::new(read("svc"), NOW, DAY, &live_verdict).unwrap();
        let s = selector.select(&w.store, &request(), all);
        assert_eq!(s.records.iter().find(|x| x.record.id() == &obs).unwrap().class, VerdictClass::Unjudged);
    }

    /// Confirmation D: a verified condensation of an unchecked source, written under `admit_unchecked`,
    /// was selected by a default selector — unverified content laundered by a verified signature.
    #[test]
    fn verified_only_withholds_a_record_derived_from_an_unchecked_one() {
        let mut w = World::new();
        let forged = KnowledgeRecord::new(iss("auditor-1"), RecordKind::Assessment, NOW - 10, "svc/a", b"bad".to_vec(), vec![]).unwrap();
        let fid = forged.id().clone();
        w.store.put(forged);
        let wide = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap().admit_unchecked();
        let c = w.write(&wide, w.condensation("condenser", "svc/a", &[&fid])).expect("written where unchecked records are admitted");
        let strict = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let s = strict.select(&w.store, &request(), all);
        assert!(!s.records.iter().any(|x| x.record.id() == &c), "derived from a record verified-only does not admit");
        assert!(s.withheld.contains(&c));
    }

    /// Confirmation E: a condensation dated in the future outlived its sources.
    #[test]
    fn a_future_dated_condensation_is_refused() {
        let mut w = World::new();
        let live = w.put("observer", RecordKind::Observation, "svc/c", "now", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let rec = KnowledgeRecord::new(iss("condenser"), RecordKind::Claim, NOW + 3000 * DAY, "svc/c", vec![],
            vec![Link { kind: LinkKind::DerivedFrom, target: live.clone() }]).unwrap();
        assert!(w.write(&selector, rec).is_err());
    }

    /// Confirmation E: a derivation is no fresher than its basis — once a source ages out of the window,
    /// so does what was derived from it.
    #[test]
    fn a_derived_record_expires_with_its_oldest_source() {
        let mut w = World::new();
        let src = KnowledgeRecord::new(iss("observer"), RecordKind::Observation, NOW - DAY / 2, "svc/c", b"x".to_vec(), vec![]).unwrap();
        let src_id = src.id().clone();
        let signed = w.sign(src);
        w.store.put_signed(signed, &w.members, &w.external).unwrap();
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let c = w.write(&selector, w.condensation("condenser", "svc/c", &[&src_id])).unwrap();
        let later = Selector::new(read("svc"), NOW + 3 * DAY / 4, DAY, &verdicts).unwrap();
        let s = later.select(&w.store, &request(), all);
        let class = |id: &RecordId| s.records.iter().find(|x| x.record.id() == id).unwrap().class;
        assert_eq!(class(&src_id), VerdictClass::Expired);
        assert_eq!(class(&c), VerdictClass::Expired, "the condensation is no fresher than its source");
    }

    // ── writing a condensation back ─────────────────────────────────────────────────────────────

    /// **The exit's first case:** a condensation nobody signed is refused, and so is one signed by
    /// someone other than the issuer it names.
    #[test]
    fn a_condensed_record_is_refused_without_its_issuers_signature() {
        let mut w = World::new();
        let a1 = w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]);
        let a2 = w.put("auditor-2", RecordKind::Assessment, "svc/a", "bad too", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
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
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();

        // Rejected and InsufficientEvidence are two subjects' verdicts, so one subject per condensation
        // keeps them apart; within a subject the standing still divides.
        for (pair, want, why) in [
            ([&bad, &unknown], CondenseRefusal::SubjectMismatch(unknown.clone()), "rejected with insufficient evidence"),
            ([&live, &withdrawn], CondenseRefusal::RetractedSource(withdrawn.clone()), "a retracted record into a live one"),
        ] {
            let draft = w.condensation("condenser", "svc/a", &pair);
            assert_eq!(w.write(&selector, draft), Err(want), "{why}");
        }
        let got = w.write(&selector, w.condensation("condenser", "svc/a", &[&live, &derived]));
        assert!(matches!(got, Err(CondenseRefusal::AcrossVerdicts(_))), "a basis-withdrawn record into a live one: {got:?}");

        // The draft builder refuses the same, from the selection's classes.
        let s = selector.select(&w.store, &request(), all);
        let pick = |id: &RecordId| s.records.iter().find(|x| x.record.id() == id).unwrap();
        assert!(matches!(
            selector.condensation_draft(&[pick(&bad), pick(&unknown)], iss("condenser"), NOW, "svc/a", vec![]),
            Err(CondenseRefusal::SubjectMismatch(_))
        ));

        // Within one class it is written.
        let groups = s.by_class();
        let rejected: Vec<&Selected> = groups[&VerdictClass::Rejected].clone();
        assert!(rejected.iter().any(|x| x.record.id() == &bad) && rejected.iter().any(|x| x.record.id() == &live));
        let draft = selector.condensation_draft(&rejected, iss("condenser"), NOW, "svc/a", b"both bad".to_vec()).unwrap();
        w.write(&selector, draft).unwrap();
    }

    /// The class is re-derived when the condensation is written, not taken from the selection: a
    /// source retracted between selecting and writing moves it out of its class.
    #[test]
    fn the_class_is_re_derived_at_write_time() {
        let mut w = World::new();
        let one = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        w.put("auditor-1", RecordKind::Assessment, "svc/a", "bad", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
        let s = selector.select(&w.store, &request(), all);
        let draft = selector.condensation_draft(&s.records.iter().collect::<Vec<_>>(), iss("condenser"), NOW, "svc/a", vec![])
            .unwrap();
        w.put("observer", RecordKind::Claim, "svc/a", "retract",
            vec![Link { kind: LinkKind::Retracts, target: one.clone() }]);
        let got = w.write(&selector, draft);
        assert_eq!(got, Err(CondenseRefusal::RetractedSource(one.clone())), "retracted since the selection");
    }

    #[test]
    fn a_condensation_is_a_claim_in_scope_derived_only_from_readable_sources() {
        let mut w = World::new();
        let a = w.put("observer", RecordKind::Observation, "svc/a", "slow", vec![]);
        let secret = w.put("observer", RecordKind::Observation, "payroll/q3", "secret", vec![]);
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
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
        let selector = Selector::new(read("svc"), NOW, DAY, &verdicts).unwrap();
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
