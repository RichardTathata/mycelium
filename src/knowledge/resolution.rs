//! **Evidence-aware resolution** (item 3 PR 4) — §3 and §4 of
//! [`docs/design/knowledge-layer.md`](../../../docs/design/knowledge-layer.md).
//!
//! # It wraps `resolve_for_caller`; it does not replace it
//!
//! `src/agent/capability_handle.rs` already applies the native gates — `is_fresh`, then the
//! schema-id gate. This runs **after** them, on candidates that already passed, and the order is
//! fixed:
//!
//! 1. **native gates first** — an entry that fails them never reaches evidence evaluation;
//! 2. **bind to the exact release** being considered;
//! 3. **verify and classify** the evidence for that release;
//! 4. evaluate a **deterministic reader policy**;
//! 5. return [`Verdict::Accepted`] · [`Rejected`](Verdict::Rejected) ·
//!    [`InsufficientEvidence`](Verdict::InsufficientEvidence) · [`Conflicted`](Verdict::Conflicted),
//!    **with reasons**;
//! 6. **compose with load and locality** — rank *within* the accepted class, then hand the survivors
//!    to the reasoning router.
//!
//! Step 6's order is the one that would otherwise be got wrong. **Evidence decides which candidates
//! are eligible; the router decides which eligible candidate to use.** Interleaving them would let a
//! well-evidenced but overloaded provider beat a fine one, or the reverse. [`filter_accepted`] keeps
//! them apart by construction: it *filters* and never reorders.
//!
//! # Evidence never grants what authorization denies
//!
//! If `resolve_for_caller` or the AE evaluator says no, no quantity of supporting evidence changes
//! it. **Evidence can only ever narrow the set** — which is why this module takes already-authorized
//! candidates as its input and has no way to add one.
//!
//! # The four rules, as mechanisms rather than prose (§4)
//!
//! - **Competence is contextual; there is no reputation scalar.** Nothing here aggregates across
//!   subjects, and there is no score to carry from one to another. A verdict is always *about one
//!   bound subject*. "Good at X" cannot imply "good at Y" because no value spans them.
//! - **Identity is not independence.** Distinct keys may be one operator, one deployment or one
//!   upstream. Independence is therefore a **reader-configured control group**
//!   ([`ReaderPolicy::control_groups`]), never inferred from issuer identity.
//! - **Missing evidence is uncertainty, not a verdict.** [`InsufficientEvidence`] is separate from
//!   [`Rejected`] so a reader can tell *"we looked and it is bad"* from *"we do not know"*.
//! - **Refreshing an advertisement never refreshes evidence.** A capability refresh is the
//!   evaporation lease: it says the provider is *alive*, not that anything is still *true*. Evidence
//!   ages on its own record timestamps, which a refresh does not touch.
//!
//! [`InsufficientEvidence`]: Verdict::InsufficientEvidence
//! [`Rejected`]: Verdict::Rejected

use std::collections::BTreeSet;

use std::collections::BTreeMap;

use super::cohort::{CohortView, StaleRule, UndeclaredRule};
use super::correction::{standing, DependencyIndex, Standing};
use super::issuer::{
    verify_issuer, Authenticity, MemberKeySource, MemberKeys, TrustedExternalIssuers,
    UnverifiableReason,
};
use super::store::{Attribution, KnowledgeStore};
use super::{IssuerId, KnowledgeRecord, LinkKind, RecordId, RecordKind};

/// The identity of one release, and the **only** thing evidence binds to.
///
/// Evidence about `v1.2.0` says nothing about `v1.3.0`. Making the subject derive from this — rather
/// than letting a caller pass any string — is what stops evidence drifting across releases by
/// accident.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReleaseId {
    /// What is being released — a capability or service name.
    pub name: String,
    /// Which release of it.
    pub version: String,
}

impl ReleaseId {
    /// Bind a release.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self { name: name.into(), version: version.into() }
    }

    /// The subject string evidence about this release is filed under.
    ///
    /// Domain-separated so a release named `"a/1.0"` and one named `"a"` version `"1.0"` cannot
    /// collide into the same subject.
    pub fn subject(&self) -> String {
        format!("release/{}/{}/{}", self.name.len(), self.name, self.version)
    }
}

/// A reader's own policy. **Deterministic** — two readers with the same policy and the same records
/// reach the same verdict, which is what makes a verdict worth recording.
#[derive(Clone, Debug)]
pub struct ReaderPolicy {
    /// How many **distinct issuers** must support the release at all.
    ///
    /// Counted per issuer, not per record: an issuer that files five supporting assessments is one
    /// supporter. Counting records would let one issuer meet this threshold by repeating itself
    /// (threat model Boundary H, plan item H2).
    pub min_supporting: usize,
    /// How many **independent** ones, counted by control group.
    pub min_independent: usize,
    /// The reader's control groups. Issuers in the same group are **not independent of each other**;
    /// an issuer in no group is its own group (or as `undeclared` says).
    ///
    /// Groups resolve as **connected components** together with cohort declarations: overlapping
    /// groups merge, and the result does not depend on the order they are listed in (H5).
    ///
    /// This is configured, never inferred — §4's "identity is not independence".
    pub control_groups: Vec<BTreeSet<IssuerId>>,
    /// Evidence older than this is not counted. Independent of the capability's refresh lease, which
    /// is exactly the point: a liveness heartbeat must not launder a stale assessment.
    pub max_evidence_age_ms: u64,
    /// Whether records stored **without verification** ([`Attribution::Unchecked`], from
    /// [`KnowledgeStore::put`]) count as evidence (Boundary H item K1b).
    ///
    /// Defaults to [`UncheckedRule::Count`] so existing stores, built with `put`, keep their
    /// verdicts. A reader that receives records from anyone else should set
    /// [`UncheckedRule::Exclude`], and the confined-fleet profile requires it. Under `Exclude`, an
    /// unchecked record can neither support, challenge, retract nor supersede anything.
    pub unchecked: UncheckedRule,
    /// Cohort declarations from operators this reader trusts (Boundary H item H5). An issuer a
    /// declaration places in a cohort is not independent of the cohort's other members.
    pub cohorts: CohortView,
    /// What to do with an issuer no declaration and no configured group places.
    pub undeclared: UndeclaredRule,
    /// What to do with an issuer whose only placements rest on stale declarations.
    pub stale: StaleRule,
}

/// What a reader does with records stored without verification.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UncheckedRule {
    /// Count them. The behaviour before K1b, and the default for compatibility.
    #[default]
    Count,
    /// Exclude them entirely, and report each one excluded.
    Exclude,
}

impl Default for ReaderPolicy {
    /// A deliberately unhelpful default: it requires evidence and will not silently accept.
    fn default() -> Self {
        Self {
            min_supporting: 1,
            min_independent: 1,
            control_groups: Vec::new(),
            max_evidence_age_ms: u64::MAX,
            unchecked: UncheckedRule::Count,
            cohorts: CohortView::default(),
            undeclared: UndeclaredRule::OwnGroup,
            stale: StaleRule::Retain,
        }
    }
}

impl ReaderPolicy {
    /// Every configured control group `issuer` is in — all of them, not the first.
    fn groups_of<'a>(&'a self, issuer: &'a IssuerId) -> impl Iterator<Item = usize> + 'a {
        self.control_groups.iter().enumerate().filter(move |(_, g)| g.contains(issuer)).map(|(i, _)| i)
    }
}

/// A tiny union-find over string-named nodes, for grouping issuers by control. Deterministic: the
/// root of a set is its smallest member, so the grouping never depends on insertion order.
#[derive(Default)]
struct Components {
    parent: BTreeMap<String, String>,
}

impl Components {
    fn find(&mut self, x: &str) -> String {
        let p = self.parent.entry(x.to_string()).or_insert_with(|| x.to_string()).clone();
        if p == x {
            return p;
        }
        let root = self.find(&p);
        self.parent.insert(x.to_string(), root.clone());
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(hi, lo);
        }
    }
}

/// Why a candidate was refused.
///
/// `#[non_exhaustive]`: challenge admission (plan item H1) will add reasons. A `match` outside this
/// crate needs a `_` arm, and that arm must **fail safe**: an unrecognised reason is still a
/// refusal.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectionReason {
    /// An issuer challenged the release and the reader's policy treats that as disqualifying.
    Challenged {
        /// Who challenged it.
        by: IssuerId,
    },
}

/// What the reader concluded. **Four outcomes, and the distinctions between them are the point.**
///
/// `#[non_exhaustive]`: challenge admission (plan item H1) will extend it. A `match` outside this
/// crate needs a `_` arm, and that arm must **fail safe**: treat an unrecognised verdict as not
/// accepted, which is what [`Verdict::is_accepted`] already does.
///
/// Every `supporting` count is a count of **distinct issuers**, never of records.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Enough independent support, no unresolved challenge.
    Accepted {
        /// How many distinct issuers support it.
        supporting: usize,
        /// How many independent control groups they came from.
        independent: usize,
    },
    /// Looked at, and it is bad.
    Rejected {
        /// Why — never an empty vector.
        reasons: Vec<RejectionReason>,
    },
    /// **We do not know.** Distinct from `Rejected` on purpose.
    InsufficientEvidence {
        /// Distinct supporting issuers found.
        have: usize,
        /// Distinct supporting issuers the policy wants.
        need: usize,
        /// Independent groups found.
        independent: usize,
        /// Independent groups the policy wants.
        need_independent: usize,
    },
    /// Support **and** challenge, both current. Not the reader's to silently resolve.
    Conflicted {
        /// Distinct supporting issuers.
        supporting: usize,
        /// Challenging assessments, counted per record. Challenge admission (plan item H1) will
        /// change how challenges are counted; this change touches support only.
        challenging: usize,
    },
}

impl Verdict {
    /// Is this candidate eligible to be used?
    pub fn is_accepted(&self) -> bool {
        matches!(self, Verdict::Accepted { .. })
    }
}

/// Why a record that would otherwise have counted as evidence did not (Boundary H item K1b).
///
/// Three questions are kept apart, and each exclusion answers one of them:
/// - **authenticity** — is the record verifiably its issuer's? ([`Unchecked`](Self::Unchecked),
///   [`NotAttributableNow`](Self::NotAttributableNow));
/// - **present authority** — does the issuer still stand behind the key? ([`KeyRevoked`](Self::KeyRevoked));
/// - **currency for this decision** — is it still current? ([`Retracted`](Self::Retracted),
///   [`SupersededByIssuer`](Self::SupersededByIssuer), [`BasisWithdrawn`](Self::BasisWithdrawn),
///   [`Expired`](Self::Expired)).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// Stored without verification, and the reader's policy excludes unchecked records.
    Unchecked,
    /// Verified when stored, but no admissible path attributes it under the reader's key view now —
    /// for example an external issuer the reader has since stopped trusting.
    NotAttributableNow(UnverifiableReason),
    /// Signed under a key its issuer has validly revoked — at storage or since. Still attributable
    /// as history; no present standing.
    KeyRevoked {
        /// The revoked key.
        key: [u8; 32],
    },
    /// Its own issuer withdrew it.
    Retracted {
        /// Who withdrew it — necessarily its issuer.
        by: IssuerId,
    },
    /// Its own issuer replaced it with a later record.
    SupersededByIssuer {
        /// The replacing record.
        by: RecordId,
    },
    /// A record it was derived from was withdrawn.
    BasisWithdrawn {
        /// The nearest withdrawn basis.
        basis: RecordId,
        /// How many derivation hops away.
        hops: usize,
    },
    /// Older than the reader's window.
    Expired {
        /// When it was issued.
        at_ms: u64,
    },
    /// No trusted declaration and no configured group places the issuer, and the reader's
    /// `UndeclaredRule::Excluded` excludes such issuers (H5).
    Undeclared,
    /// The issuer's only placements rest on stale declarations, and the reader's
    /// `StaleRule::Exclude` excludes such evidence until the relationship is refreshed (H5).
    StaleCohortOnly,
}

/// One record excluded from a verdict, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excluded {
    /// The record.
    pub record: RecordId,
    /// Its issuer.
    pub issuer: IssuerId,
    /// Why it did not count.
    pub why: Exclusion,
}

/// A verdict together with every record that would have counted but did not, sorted by record id.
///
/// Eligibility is re-derived on every call and never cached as "eligible": a key revoked, an
/// issuer untrusted or a record retracted since the last call changes the next answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    /// The verdict over the eligible records.
    pub verdict: Verdict,
    /// What was excluded, and why.
    pub excluded: Vec<Excluded>,
}

/// **Classify the evidence for one bound release.**
///
/// `now_ms` is the reader's clock; evidence older than `policy.max_evidence_age_ms` is not counted.
///
/// Only [`RecordKind::Assessment`] counts as evidence. A claim is what someone said about
/// themselves and an observation is a report; neither is a judgement, and §1 split them precisely so
/// that a self-issued claim cannot be counted as support. **A self-assessment does not count
/// either** — an issuer supporting its own release is not evidence, and the record's issuer is in
/// its id, so that is checkable here without fetching anything.
///
/// **Support is counted per issuer.** Several supporting records from one issuer are one supporter,
/// for `min_supporting` and in every `supporting` count the verdict reports. Independence was
/// already counted by control group; this closes the remaining path by which one issuer could meet
/// a support threshold by repeating itself.
///
/// **Currency is checked** (Boundary H item K1b): a record its issuer retracted or superseded, or
/// whose basis was withdrawn, does not count, and neither does an unchecked record under
/// [`UncheckedRule::Exclude`] or a record stored under an already-revoked key. This function does
/// **not** re-verify signatures against a current key view; [`classify_eligible`] does, and also
/// reports what it excluded.
pub fn classify(
    store: &KnowledgeStore,
    release: &ReleaseId,
    provider: &IssuerId,
    policy: &ReaderPolicy,
    now_ms: u64,
) -> Verdict {
    evaluate::<NoKeyView>(store, release, provider, policy, now_ms, None).verdict
}

/// **Classify against the reader's current key view, and say what was excluded** (Boundary H
/// item K1b).
///
/// Everything [`classify`] does, plus: every verified record's retained signature is checked again,
/// now, through issuer binding's two admissible paths against `members` and `external`. A key
/// revoked since storage, or an external issuer no longer trusted, removes the record from the
/// count — while it stays in the store as history.
pub fn classify_eligible(
    store: &KnowledgeStore,
    release: &ReleaseId,
    provider: &IssuerId,
    policy: &ReaderPolicy,
    now_ms: u64,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Classification {
    evaluate(store, release, provider, policy, now_ms, Some((members, external)))
}

/// A key view that knows nobody, for [`classify`], which does not re-verify.
struct NoKeyView;

impl MemberKeySource for NoKeyView {
    fn member_keys(&self, _: &crate::node_id::NodeId) -> MemberKeys {
        MemberKeys::default()
    }
}

/// Authenticity and present authority of one record, or why not.
fn authenticity_exclusion<M: MemberKeySource>(
    store: &KnowledgeStore,
    record: &KnowledgeRecord,
    policy: &ReaderPolicy,
    keys: Option<(&M, &TrustedExternalIssuers)>,
) -> Option<Exclusion> {
    match store.attribution(record.id()) {
        None | Some(Attribution::Unchecked) => match policy.unchecked {
            UncheckedRule::Count => None,
            UncheckedRule::Exclude => Some(Exclusion::Unchecked),
        },
        Some(Attribution::Verified { key, revoked_at_storage: true, .. }) => {
            Some(Exclusion::KeyRevoked { key: *key })
        }
        Some(Attribution::Verified { .. }) => {
            let (members, external) = keys?;
            let signature = store.signature(record.id()).unwrap_or_default();
            match verify_issuer(record, signature, members, external) {
                Authenticity::Current { .. } => None,
                Authenticity::Revoked { key, .. } => Some(Exclusion::KeyRevoked { key }),
                Authenticity::Unverifiable(reason) => Some(Exclusion::NotAttributableNow(reason)),
            }
        }
    }
}

/// H5's exclusions: an undeclared issuer under `UndeclaredRule::Excluded`, or one placed only by stale
/// declarations under `StaleRule::Exclude`.
fn placement_exclusion(policy: &ReaderPolicy, issuer: &IssuerId, at_ms: u64, now_ms: u64) -> Option<Exclusion> {
    if policy.groups_of(issuer).next().is_some() {
        return None; // The reader's own configuration places it.
    }
    let placement = policy.cohorts.placement(issuer, at_ms, now_ms);
    if placement.cohorts.is_empty() {
        return (policy.undeclared == UndeclaredRule::Excluded).then_some(Exclusion::Undeclared);
    }
    (!placement.any_current && policy.stale == StaleRule::Exclude).then_some(Exclusion::StaleCohortOnly)
}

/// How many independent groups the counted supporting records come from.
///
/// Nodes are issuers, configured groups and declared cohorts; every placement is an edge. Two issuers
/// are dependent if any path joins them — so overlapping groups merge, and the answer does not
/// depend on the order anything was configured or declared in.
fn independent_groups(policy: &ReaderPolicy, records: &[(IssuerId, u64)], now_ms: u64) -> usize {
    let mut c = Components::default();
    let cohort_node = |k: &super::cohort::CohortKey| format!("c:{}/{}", k.operator.as_str(), k.cohort);
    // The whole graph, not just the supporters: a member who has said nothing still joins the two
    // cohorts (or groups) it belongs to, so their supporters are dependent through it.
    for (g, members) in policy.control_groups.iter().enumerate() {
        for m in members {
            c.union(&format!("i:{}", m.as_str()), &format!("g:{g}"));
        }
    }
    for (key, member) in policy.cohorts.members_in_force(now_ms) {
        c.union(&format!("i:{}", member.as_str()), &cohort_node(&key));
    }
    // Then each counted record's own placement, including membership at its issue time.
    for (issuer, at_ms) in records {
        let me = format!("i:{}", issuer.as_str());
        c.find(&me);
        let mut placed = false;
        for g in policy.groups_of(issuer) {
            c.union(&me, &format!("g:{g}"));
            placed = true;
        }
        for cohort in policy.cohorts.placement(issuer, *at_ms, now_ms).cohorts {
            c.union(&me, &cohort_node(&cohort));
            placed = true;
        }
        if !placed && policy.undeclared == UndeclaredRule::OneGroup {
            c.union(&me, "u:undeclared");
        }
    }
    let roots: BTreeSet<String> =
        records.iter().map(|(i, _)| c.find(&format!("i:{}", i.as_str()))).collect();
    roots.len()
}

fn evaluate<M: MemberKeySource>(
    store: &KnowledgeStore,
    release: &ReleaseId,
    provider: &IssuerId,
    policy: &ReaderPolicy,
    now_ms: u64,
    keys: Option<(&M, &TrustedExternalIssuers)>,
) -> Classification {
    let subject = release.subject();

    // Only records the reader accepts as authentic and currently authorised may withdraw or replace
    // anything. Otherwise an unverified "retraction" could suppress someone's verified support —
    // suppression is an attack too, not only over-assertion.
    let authentic = |r: &KnowledgeRecord| authenticity_exclusion(store, r, policy, keys).is_none();
    let index = DependencyIndex::build_filtered(store, authentic);
    let mut superseded: BTreeMap<RecordId, RecordId> = BTreeMap::new();
    for r in store.records().filter(|r| authentic(r)) {
        for link in r.links() {
            // Same issuer only: another issuer "superseding" a record is disagreement, not
            // replacement, and must not erase it.
            if link.kind == LinkKind::Supersedes && link.target.issuer == *r.issuer() {
                superseded.entry(link.target.clone()).or_insert_with(|| r.id().clone());
            }
        }
    }

    // A set, not a list: one issuer is one supporter however many records it files.
    let mut supporting_issuers: BTreeSet<IssuerId> = BTreeSet::new();
    // Every counted supporting record's issuer and issue time, for historical grouping (H5).
    let mut supporting_records: Vec<(IssuerId, u64)> = Vec::new();
    let mut challenges: Vec<IssuerId> = Vec::new();
    let mut excluded: Vec<Excluded> = Vec::new();

    for record in store.about(&subject) {
        // Only a judgement counts. A claim or an observation is not one.
        if record.kind() != RecordKind::Assessment {
            continue;
        }
        // The provider assessing its own release is not evidence about it.
        if record.issuer() == provider {
            continue;
        }

        let mut supports = false;
        let mut challenges_it = false;
        for link in record.links() {
            match link.kind {
                LinkKind::Supports => supports = true,
                LinkKind::Challenges => challenges_it = true,
                _ => {}
            }
        }
        if !supports && !challenges_it {
            continue;
        }

        let why = authenticity_exclusion(store, record, policy, keys)
            .or_else(|| superseded.get(record.id()).map(|by| Exclusion::SupersededByIssuer { by: by.clone() }))
            .or_else(|| {
                // Evidence ages on its own timestamp. A capability refresh does not touch this.
                match standing(store, &index, record.id(), now_ms, policy.max_evidence_age_ms)? {
                    Standing::Current => None,
                    Standing::Retracted { by } => Some(Exclusion::Retracted { by }),
                    Standing::BasisWithdrawn { basis, hops } => Some(Exclusion::BasisWithdrawn { basis, hops }),
                    Standing::Expired { at_ms } => Some(Exclusion::Expired { at_ms }),
                }
            })
            .or_else(|| placement_exclusion(policy, record.issuer(), record.at_ms(), now_ms));
        if let Some(why) = why {
            excluded.push(Excluded { record: record.id().clone(), issuer: record.issuer().clone(), why });
            continue;
        }

        // A record that both supports and challenges is a challenge: the cautious reading, and the
        // one that cannot be used to manufacture support.
        if challenges_it {
            challenges.push(record.issuer().clone());
        } else {
            supporting_issuers.insert(record.issuer().clone());
            supporting_records.push((record.issuer().clone(), record.at_ms()));
        }
    }
    excluded.sort_by(|a, b| a.record.cmp(&b.record));

    let supporting = supporting_issuers.len();
    let challenging = challenges.len();

    // Independence by control — configured or declared, never inferred — as connected components.
    let independent = independent_groups(policy, &supporting_records, now_ms);

    // Support and challenge together is a conflict, and it is not this layer's to resolve.
    let verdict = if challenging > 0 && supporting > 0 {
        Verdict::Conflicted { supporting, challenging }
    } else if challenging > 0 {
        Verdict::Rejected {
            reasons: challenges.into_iter().map(|by| RejectionReason::Challenged { by }).collect(),
        }
    } else if supporting < policy.min_supporting || independent < policy.min_independent {
        Verdict::InsufficientEvidence {
            have: supporting,
            need: policy.min_supporting,
            independent,
            need_independent: policy.min_independent,
        }
    } else {
        Verdict::Accepted { supporting, independent }
    };
    Classification { verdict, excluded }
}

/// **Step 6: compose with load and locality — by filtering, never by reordering.**
///
/// `candidates` arrive in the router's preferred order (load, locality, reservations). This returns
/// the accepted ones **in that same order**, so evidence decides eligibility and the router still
/// decides choice. It cannot promote a well-evidenced candidate past a better-placed one, because it
/// never moves anything.
pub fn filter_accepted<T>(
    candidates: Vec<T>,
    mut verdict_of: impl FnMut(&T) -> Verdict,
) -> Vec<T> {
    candidates.into_iter().filter(|c| verdict_of(c).is_accepted()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::store::KnowledgeStore;
    use crate::knowledge::{KnowledgeRecord, Link, RecordId};

    fn issuer(s: &str) -> IssuerId {
        IssuerId::new(s).expect("valid")
    }

    /// A record id to point a link at. The target's content does not matter to these tests — what is
    /// under test is how the *linking* record is counted.
    fn target_id(of: &IssuerId) -> RecordId {
        RecordId { issuer: of.clone(), digest: [7u8; 32] }
    }

    /// An assessment by `by` about `release`, supporting or challenging.
    fn assessment(
        by: &str,
        release: &ReleaseId,
        kind: LinkKind,
        at_ms: u64,
    ) -> KnowledgeRecord {
        let by = issuer(by);
        let subject_of = issuer("provider");
        KnowledgeRecord::new(
            by,
            RecordKind::Assessment,
            at_ms,
            release.subject(),
            b"judgement".to_vec(),
            vec![Link { kind, target: target_id(&subject_of) }],
        )
        .expect("well formed")
    }

    fn release() -> ReleaseId {
        ReleaseId::new("summarizer", "1.2.0")
    }

    fn store_with(records: Vec<KnowledgeRecord>) -> KnowledgeStore {
        let mut s = KnowledgeStore::new();
        for r in records {
            s.put(r);
        }
        s
    }

    /// Two independent supporters, no challenge, inside the age window.
    #[test]
    fn independent_support_is_accepted() {
        let r = release();
        let store = store_with(vec![
            assessment("lab-a", &r, LinkKind::Supports, 1_000),
            assessment("lab-b", &r, LinkKind::Supports, 1_000),
        ]);
        let policy = ReaderPolicy { min_supporting: 2, min_independent: 2, ..Default::default() };

        assert_eq!(
            classify(&store, &r, &issuer("provider"), &policy, 1_000),
            Verdict::Accepted { supporting: 2, independent: 2 }
        );
    }

    /// **"Identity is not independence."** Two issuers the reader has placed in one control group
    /// count as **one**, so a policy wanting two independent voices is not satisfied by them.
    #[test]
    fn two_issuers_in_one_control_group_are_not_two_independent_voices() {
        let r = release();
        let store = store_with(vec![
            assessment("lab-a", &r, LinkKind::Supports, 1_000),
            assessment("lab-a-sibling", &r, LinkKind::Supports, 1_000),
        ]);
        let mut group = BTreeSet::new();
        group.insert(issuer("lab-a"));
        group.insert(issuer("lab-a-sibling"));
        let policy = ReaderPolicy {
            min_supporting: 2,
            min_independent: 2,
            control_groups: vec![group],
            ..Default::default()
        };

        match classify(&store, &r, &issuer("provider"), &policy, 1_000) {
            Verdict::InsufficientEvidence { have, independent, need_independent, .. } => {
                assert_eq!(have, 2, "both assessments were counted as support");
                assert_eq!(independent, 1, "but they are one control group, so one voice");
                assert_eq!(need_independent, 2);
            }
            other => panic!("expected InsufficientEvidence, got {other:?}"),
        }
    }

    // ── K1b: present eligibility ─────────────────────────────────────────────────────────────

    mod k1b {
        use super::*;
        use crate::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
        use crate::knowledge::store::SignedRecord;
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

        fn view(n: &NodeId, retained: Vec<[u8; 32]>, revoked: Vec<[u8; 32]>) -> HashMap<NodeId, MemberKeys> {
            HashMap::from([(n.clone(), MemberKeys { retained, revoked: revoked.into_iter().collect() })])
        }

        fn signed_support(sk: &SigningKey, by: IssuerId, r: &ReleaseId) -> SignedRecord {
            let record = KnowledgeRecord::new(
                by,
                RecordKind::Assessment,
                1_000,
                r.subject(),
                b"judgement".to_vec(),
                vec![Link { kind: LinkKind::Supports, target: target_id(&issuer("provider")) }],
            )
            .unwrap();
            let signature = mycelium_core::tls::sign_bytes(sk, &record.canonical_bytes()).to_vec();
            SignedRecord { record, signature }
        }

        fn follow_up(by: &str, kind: LinkKind, target: &RecordId, r: &ReleaseId) -> KnowledgeRecord {
            KnowledgeRecord::new(issuer(by), RecordKind::Claim, 1_500, r.subject(), b"f".to_vec(), vec![Link { kind, target: target.clone() }])
                .unwrap()
        }

        fn one() -> ReaderPolicy {
            ReaderPolicy { min_supporting: 1, min_independent: 1, ..Default::default() }
        }

        /// **A retracted assessment no longer counts.** Before K1b, `classify` never consulted
        /// retraction, so a withdrawn judgement kept supporting the release.
        #[test]
        fn a_retracted_assessment_stops_counting() {
            let r = release();
            let support = assessment("lab-a", &r, LinkKind::Supports, 1_000);
            let mut store = store_with(vec![support.clone()]);
            assert!(classify(&store, &r, &issuer("provider"), &one(), 2_000).is_accepted());

            store.put(follow_up("lab-a", LinkKind::Retracts, support.id(), &r));
            let c = classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &HashMap::<NodeId, MemberKeys>::new(), &TrustedExternalIssuers::new());
            assert!(!c.verdict.is_accepted());
            assert_eq!(c.excluded[0].why, Exclusion::Retracted { by: issuer("lab-a") });
            assert_eq!(store.len(), 2, "nothing was deleted");
        }

        /// Unchecked records count by default, for compatibility, and are excluded — and reported —
        /// under `UncheckedRule::Exclude`.
        #[test]
        fn unchecked_records_are_excluded_when_the_policy_says_so() {
            let r = release();
            let store = store_with(vec![assessment("lab-a", &r, LinkKind::Supports, 1_000)]);
            assert!(classify(&store, &r, &issuer("provider"), &one(), 2_000).is_accepted());

            let strict = ReaderPolicy { unchecked: UncheckedRule::Exclude, ..one() };
            let c = classify_eligible(&store, &r, &issuer("provider"), &strict, 2_000, &HashMap::<NodeId, MemberKeys>::new(), &TrustedExternalIssuers::new());
            assert!(matches!(c.verdict, Verdict::InsufficientEvidence { have: 0, .. }));
            assert_eq!(c.excluded.len(), 1);
            assert_eq!(c.excluded[0].why, Exclusion::Unchecked);
        }

        /// **Authentic is not current, re-derived at read time.** A member's verified support counts;
        /// once its key is revoked, the same stored record stops counting, and stays in the store.
        #[test]
        fn a_key_revoked_after_storage_removes_the_record_from_the_count() {
            let (sk, pk) = keypair(1);
            let (_new_sk, new_pk) = keypair(2);
            let a = node(7201);
            let r = release();
            let mut store = KnowledgeStore::new();
            let signed = signed_support(&sk, IssuerId::for_node(&a), &r);
            let id = signed.record.id().clone();
            store.put_signed(signed, &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new()).unwrap();
            let strict = ReaderPolicy { unchecked: UncheckedRule::Exclude, ..one() };

            let before = classify_eligible(&store, &r, &issuer("provider"), &strict, 2_000, &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new());
            assert!(before.verdict.is_accepted());

            let after = classify_eligible(&store, &r, &issuer("provider"), &strict, 2_000, &view(&a, vec![new_pk, pk], vec![pk]), &TrustedExternalIssuers::new());
            assert!(!after.verdict.is_accepted());
            assert_eq!(after.excluded, vec![Excluded { record: id.clone(), issuer: IssuerId::for_node(&a), why: Exclusion::KeyRevoked { key: pk } }]);
            assert!(store.get(&id).is_some(), "history is kept");
        }

        /// An external issuer the reader stops trusting stops counting, with the reason named.
        #[test]
        fn an_external_issuer_no_longer_trusted_stops_counting() {
            let (sk, pk) = keypair(9);
            let auditor = issuer("auditor-acme");
            let r = release();
            let mut trusted = TrustedExternalIssuers::new();
            trusted.trust(auditor.clone(), pk).unwrap();
            let mut store = KnowledgeStore::new();
            store.put_signed(signed_support(&sk, auditor.clone(), &r), &HashMap::<NodeId, MemberKeys>::new(), &trusted).unwrap();

            let members = HashMap::<NodeId, MemberKeys>::new();
            assert!(classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &members, &trusted).verdict.is_accepted());
            trusted.untrust(&auditor);
            let c = classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &members, &trusted);
            assert!(!c.verdict.is_accepted());
            assert_eq!(c.excluded[0].why, Exclusion::NotAttributableNow(UnverifiableReason::UntrustedExternal));
        }

        /// **Suppression resistance.** Under `Exclude`, an unverified "retraction" of a verified
        /// support record — which any local code could `put` — does not take effect.
        #[test]
        fn an_unverified_retraction_cannot_suppress_verified_support() {
            let (sk, pk) = keypair(1);
            let a = node(7201);
            let r = release();
            let mut store = KnowledgeStore::new();
            let signed = signed_support(&sk, IssuerId::for_node(&a), &r);
            let id = signed.record.id().clone();
            store.put_signed(signed, &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new()).unwrap();
            // An unchecked record claiming A's issuer, retracting A's support.
            let forged = KnowledgeRecord::new(IssuerId::for_node(&a), RecordKind::Claim, 1_500, r.subject(), b"x".to_vec(), vec![Link { kind: LinkKind::Retracts, target: id }]).unwrap();
            store.put(forged);

            let strict = ReaderPolicy { unchecked: UncheckedRule::Exclude, ..one() };
            let c = classify_eligible(&store, &r, &issuer("provider"), &strict, 2_000, &view(&a, vec![pk], vec![]), &TrustedExternalIssuers::new());
            assert!(c.verdict.is_accepted(), "an unverified retraction must not suppress verified support: {c:?}");
        }

        /// Same-issuer supersession replaces; another issuer "superseding" is only disagreement.
        #[test]
        fn only_the_issuer_can_supersede_its_own_record() {
            let r = release();
            let support = assessment("lab-a", &r, LinkKind::Supports, 1_000);
            let none = HashMap::<NodeId, MemberKeys>::new();
            let ext = TrustedExternalIssuers::new();

            let mut store = store_with(vec![support.clone(), follow_up("lab-b", LinkKind::Supersedes, support.id(), &r)]);
            assert!(classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &none, &ext).verdict.is_accepted(), "another issuer cannot replace lab-a's record");

            let replacement = follow_up("lab-a", LinkKind::Supersedes, support.id(), &r);
            let replacement_id = replacement.id().clone();
            store.put(replacement);
            let c = classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &none, &ext);
            assert!(!c.verdict.is_accepted());
            assert_eq!(c.excluded[0].why, Exclusion::SupersededByIssuer { by: replacement_id });
        }

        /// An assessment derived from a record its issuer later withdrew is reported as
        /// basis-withdrawn — a fact about its support, not a retraction of it.
        #[test]
        fn an_assessment_whose_basis_is_withdrawn_stops_counting() {
            let r = release();
            let observation = KnowledgeRecord::new(issuer("field-lab"), RecordKind::Observation, 900, "field/x", b"o".to_vec(), vec![]).unwrap();
            let derived = KnowledgeRecord::new(
                issuer("lab-a"),
                RecordKind::Assessment,
                1_000,
                r.subject(),
                b"j".to_vec(),
                vec![
                    Link { kind: LinkKind::Supports, target: target_id(&issuer("provider")) },
                    Link { kind: LinkKind::DerivedFrom, target: observation.id().clone() },
                ],
            )
            .unwrap();
            let withdrawal = KnowledgeRecord::new(issuer("field-lab"), RecordKind::Claim, 1_200, "field/x", b"w".to_vec(), vec![Link { kind: LinkKind::Retracts, target: observation.id().clone() }]).unwrap();
            let store = store_with(vec![observation.clone(), derived, withdrawal]);
            let c = classify_eligible(&store, &r, &issuer("provider"), &one(), 2_000, &HashMap::<NodeId, MemberKeys>::new(), &TrustedExternalIssuers::new());
            assert!(!c.verdict.is_accepted());
            assert_eq!(c.excluded[0].why, Exclusion::BasisWithdrawn { basis: observation.id().clone(), hops: 1 });
        }
    }

    // ── H5: cohorts ──────────────────────────────────────────────────────────────────────────

    mod h5 {
        use super::*;
        use crate::knowledge::cohort::{CohortDeclaration, CohortView, SignedCohortDeclaration, StaleRule, UndeclaredRule};
        use crate::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
        use crate::node_id::NodeId;
        use ed25519_dalek::SigningKey;
        use std::collections::HashMap;

        fn operator() -> (SigningKey, IssuerId, TrustedExternalIssuers) {
            let sk = SigningKey::from_bytes(&[12u8; 32]);
            let op = issuer("operator:acme");
            let mut ext = TrustedExternalIssuers::new();
            ext.trust(op.clone(), sk.verifying_key().to_bytes()).unwrap();
            (sk, op, ext)
        }

        fn declare(sk: &SigningKey, op: &IssuerId, cohort: &str, seq: u64, members: &[&str], from: u64, until: u64) -> SignedCohortDeclaration {
            let declaration = CohortDeclaration {
                operator: op.clone(),
                cohort: cohort.into(),
                seq,
                members: members.iter().map(|m| issuer(m)).collect(),
                valid_from_ms: from,
                valid_until_ms: until,
            };
            let signature = mycelium_core::tls::sign_bytes(sk, &declaration.canonical_bytes()).to_vec();
            SignedCohortDeclaration { declaration, signature }
        }

        fn view(decls: &[SignedCohortDeclaration]) -> CohortView {
            let (_, op, ext) = operator();
            let mut v = CohortView::trusting([op]);
            for d in decls {
                v.offer(d, &HashMap::<NodeId, MemberKeys>::new(), &ext);
            }
            v
        }

        fn two_independent(cohorts: CohortView) -> ReaderPolicy {
            ReaderPolicy { min_supporting: 2, min_independent: 2, cohorts, ..Default::default() }
        }

        fn supports(by: &[(&str, u64)]) -> KnowledgeStore {
            let r = release();
            store_with(by.iter().map(|(i, t)| assessment(i, &r, LinkKind::Supports, *t)).collect())
        }

        fn independent(v: Verdict) -> usize {
            match v {
                Verdict::Accepted { independent, .. } => independent,
                Verdict::InsufficientEvidence { independent, .. } => independent,
                other => panic!("unexpected {other:?}"),
            }
        }

        /// **Fifty agents, one voice.** A declared cohort of fifty supporting members is one
        /// independent group.
        #[test]
        fn a_declared_cohort_of_fifty_is_one_independent_voice() {
            let (sk, op, _) = operator();
            let names: Vec<String> = (0..50).map(|i| format!("agent-{i}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let v = view(&[declare(&sk, &op, "fleet", 1, &refs, 0, 100_000)]);
            let store = supports(&refs.iter().map(|n| (*n, 1_000)).collect::<Vec<_>>());
            let verdict = classify(&store, &release(), &issuer("provider"), &two_independent(v), 2_000);
            assert_eq!(verdict, Verdict::InsufficientEvidence { have: 50, need: 2, independent: 1, need_independent: 2 });
        }

        /// **Order does not matter.** Overlapping configured groups merge as connected components,
        /// whatever order they are listed in; so do declarations, whatever order they arrive in.
        #[test]
        fn independence_does_not_depend_on_configuration_or_arrival_order() {
            let (sk, op, _) = operator();
            let d1 = declare(&sk, &op, "one", 1, &["a", "b"], 0, 100_000);
            let d2 = declare(&sk, &op, "two", 1, &["b", "c"], 0, 100_000);
            let store = supports(&[("a", 1_000), ("c", 1_000), ("d", 1_000)]);
            let g = |xs: &[&str]| xs.iter().map(|x| issuer(x)).collect::<BTreeSet<_>>();

            let mut seen = BTreeSet::new();
            for decls in [vec![d1.clone(), d2.clone()], vec![d2.clone(), d1.clone()]] {
                seen.insert(independent(classify(&store, &release(), &issuer("provider"), &two_independent(view(&decls)), 2_000)));
            }
            for groups in [vec![g(&["a", "x"]), g(&["x", "c"])], vec![g(&["x", "c"]), g(&["a", "x"])]] {
                let p = ReaderPolicy { control_groups: groups, ..two_independent(CohortView::default()) };
                seen.insert(independent(classify(&store, &release(), &issuer("provider"), &p, 2_000)));
            }
            assert_eq!(seen, BTreeSet::from([2]), "a and c merge through b (or x); d stands alone — in every order");
        }

        /// **The reviewer's case, exactly.** A and B are declared together. A's only declaration
        /// expires while B stays current in another; no refresh arrives. They remain one group.
        #[test]
        fn expiry_and_partition_cannot_manufacture_independence() {
            let (sk, op, _) = operator();
            let v = view(&[
                declare(&sk, &op, "fleet", 1, &["a", "b"], 0, 1_000),
                declare(&sk, &op, "fleet-b", 1, &["b"], 0, 100_000),
            ]);
            let store = supports(&[("a", 2_000), ("b", 2_000)]);
            assert_eq!(independent(classify(&store, &release(), &issuer("provider"), &two_independent(v.clone()), 5_000)), 1);

            // Under StaleRule::Exclude, A's evidence — resting only on a stale declaration — is
            // excluded and reported. It never falls back to looking independent.
            let strict = ReaderPolicy { stale: StaleRule::Exclude, ..two_independent(v) };
            let c = classify_eligible(&store, &release(), &issuer("provider"), &strict, 5_000, &HashMap::<NodeId, MemberKeys>::new(), &TrustedExternalIssuers::new());
            assert!(matches!(c.verdict, Verdict::InsufficientEvidence { have: 1, independent: 1, .. }), "{c:?}");
            assert_eq!(c.excluded.len(), 1);
            assert_eq!(c.excluded[0].issuer, issuer("a"));
            assert_eq!(c.excluded[0].why, Exclusion::StaleCohortOnly);
        }

        /// **Historical grouping.** A was declared with B, then removed. A's record from before the
        /// removal is still grouped with B; A's record from after it is not.
        #[test]
        fn a_removal_does_not_make_earlier_evidence_independent() {
            let (sk, op, _) = operator();
            let v = view(&[
                declare(&sk, &op, "fleet", 1, &["a", "b"], 0, 100_000),
                declare(&sk, &op, "fleet", 2, &["b"], 5_000, 100_000),
            ]);
            let before = supports(&[("a", 1_000), ("b", 1_000)]);
            assert_eq!(independent(classify(&before, &release(), &issuer("provider"), &two_independent(v.clone()), 6_000)), 1);
            let after = supports(&[("a", 6_000), ("b", 6_000)]);
            assert_eq!(independent(classify(&after, &release(), &issuer("provider"), &two_independent(v), 6_000)), 2);
        }

        /// The undeclared rule: own groups by default; one group together; or excluded and reported.
        #[test]
        fn undeclared_issuers_follow_the_readers_rule() {
            let store = supports(&[("fresh-1", 1_000), ("fresh-2", 1_000)]);
            let base = two_independent(CohortView::default());
            assert_eq!(independent(classify(&store, &release(), &issuer("provider"), &base, 2_000)), 2);

            let one = ReaderPolicy { undeclared: UndeclaredRule::OneGroup, ..base.clone() };
            assert_eq!(independent(classify(&store, &release(), &issuer("provider"), &one, 2_000)), 1);

            let ex = ReaderPolicy { undeclared: UndeclaredRule::Excluded, ..base };
            let c = classify_eligible(&store, &release(), &issuer("provider"), &ex, 2_000, &HashMap::<NodeId, MemberKeys>::new(), &TrustedExternalIssuers::new());
            assert!(matches!(c.verdict, Verdict::InsufficientEvidence { have: 0, .. }));
            assert!(c.excluded.iter().all(|e| e.why == Exclusion::Undeclared));
            assert_eq!(c.excluded.len(), 2);
        }
    }

    /// **One issuer repeating itself is one supporter** (plan item H2). Five supporting records from
    /// the same issuer do not meet `min_supporting = 2`, even though independence is satisfied. Before
    /// H2 they counted as five, and a policy asking for two supporters accepted them.
    #[test]
    fn one_issuer_repeating_itself_is_one_supporter() {
        let r = release();
        let store = store_with(
            (0..5).map(|i| assessment("lab-a", &r, LinkKind::Supports, 1_000 + i)).collect(),
        );
        let policy = ReaderPolicy { min_supporting: 2, min_independent: 1, ..Default::default() };

        assert_eq!(
            classify(&store, &r, &issuer("provider"), &policy, 2_000),
            Verdict::InsufficientEvidence { have: 1, need: 2, independent: 1, need_independent: 1 },
            "five records from one issuer are one supporter"
        );
    }

    /// Every reported `supporting` count is a count of issuers: three records from one lab and one
    /// from another are two supporters, in an acceptance and in a conflict alike.
    #[test]
    fn reported_support_counts_issuers_not_records() {
        let r = release();
        let mut records: Vec<_> =
            (0..3).map(|i| assessment("lab-a", &r, LinkKind::Supports, 1_000 + i)).collect();
        records.push(assessment("lab-b", &r, LinkKind::Supports, 1_000));
        let store = store_with(records.clone());
        let policy = ReaderPolicy { min_supporting: 2, min_independent: 2, ..Default::default() };

        assert_eq!(
            classify(&store, &r, &issuer("provider"), &policy, 2_000),
            Verdict::Accepted { supporting: 2, independent: 2 }
        );

        records.push(assessment("lab-c", &r, LinkKind::Challenges, 1_000));
        let store = store_with(records);
        assert_eq!(
            classify(&store, &r, &issuer("provider"), &policy, 2_000),
            Verdict::Conflicted { supporting: 2, challenging: 1 }
        );
    }

    /// **"Missing evidence is uncertainty, not a verdict."** Nothing found is
    /// `InsufficientEvidence`, never `Rejected` — a reader must be able to tell them apart.
    #[test]
    fn no_evidence_is_uncertainty_and_never_rejection() {
        let r = release();
        let store = store_with(vec![]);
        let v = classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000);

        assert!(
            matches!(v, Verdict::InsufficientEvidence { have: 0, .. }),
            "expected InsufficientEvidence, got {v:?}"
        );
        assert!(
            !matches!(v, Verdict::Rejected { .. }),
            "'we do not know' must never be reported as 'we looked and it is bad'"
        );
    }

    /// **"Refreshing an advertisement never refreshes evidence."** The same records, judged later:
    /// the evidence ages out on its own timestamps. A capability refresh touches the provider's
    /// liveness lease and nothing here, so there is no path by which a heartbeat launders a stale
    /// assessment into a current one.
    #[test]
    fn evidence_ages_on_its_own_record_timestamps_not_the_refresh_lease() {
        let r = release();
        let store = store_with(vec![
            assessment("lab-a", &r, LinkKind::Supports, 1_000),
            assessment("lab-b", &r, LinkKind::Supports, 1_000),
        ]);
        let policy = ReaderPolicy {
            min_supporting: 2,
            min_independent: 2,
            max_evidence_age_ms: 10_000,
            ..Default::default()
        };

        // Inside the window.
        assert!(classify(&store, &r, &issuer("provider"), &policy, 6_000).is_accepted());
        // Past it — same records, same store, no refresh could change this.
        assert!(!classify(&store, &r, &issuer("provider"), &policy, 20_000).is_accepted());
    }

    /// Support **and** challenge is `Conflicted` — not silently resolved either way.
    #[test]
    fn support_and_challenge_together_is_a_conflict() {
        let r = release();
        let store = store_with(vec![
            assessment("lab-a", &r, LinkKind::Supports, 1_000),
            assessment("lab-b", &r, LinkKind::Challenges, 1_000),
        ]);

        assert_eq!(
            classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000),
            Verdict::Conflicted { supporting: 1, challenging: 1 }
        );
    }

    /// A challenge with no support is a rejection, and it **names who challenged**.
    #[test]
    fn a_challenge_alone_is_a_rejection_that_names_its_source() {
        let r = release();
        let store = store_with(vec![assessment("lab-b", &r, LinkKind::Challenges, 1_000)]);

        match classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000) {
            Verdict::Rejected { reasons } => {
                assert_eq!(reasons, vec![RejectionReason::Challenged { by: issuer("lab-b") }]);
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    /// **A provider cannot vouch for itself.** The issuer is in the record id, so this is checkable
    /// without fetching anything.
    #[test]
    fn a_self_assessment_is_not_evidence() {
        let r = release();
        let store = store_with(vec![assessment("provider", &r, LinkKind::Supports, 1_000)]);

        let v = classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000);
        assert!(
            matches!(v, Verdict::InsufficientEvidence { have: 0, .. }),
            "the provider's own assessment must not count: {v:?}"
        );
    }

    /// Only an **assessment** is evidence. A claim is what someone said about themselves; an
    /// observation is a report. §1 split the kinds precisely so neither can be counted as judgement.
    #[test]
    fn a_claim_or_an_observation_is_not_evidence() {
        let r = release();
        let mut store = KnowledgeStore::new();
        for kind in [RecordKind::Claim, RecordKind::Observation] {
            store.put(
                KnowledgeRecord::new(
                    issuer("lab-a"),
                    kind,
                    1_000,
                    r.subject(),
                    b"not a judgement".to_vec(),
                    vec![Link { kind: LinkKind::Supports, target: target_id(&issuer("provider")) }],
                )
                .expect("well formed"),
            );
        }

        let v = classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000);
        assert!(
            matches!(v, Verdict::InsufficientEvidence { have: 0, .. }),
            "only an assessment is evidence: {v:?}"
        );
    }

    /// **Evidence binds to the exact release.** Support for `1.2.0` says nothing about `1.3.0`.
    #[test]
    fn evidence_does_not_leak_across_releases() {
        let old = ReleaseId::new("summarizer", "1.2.0");
        let new = ReleaseId::new("summarizer", "1.3.0");
        let store = store_with(vec![
            assessment("lab-a", &old, LinkKind::Supports, 1_000),
            assessment("lab-b", &old, LinkKind::Supports, 1_000),
        ]);
        let policy = ReaderPolicy { min_supporting: 2, min_independent: 2, ..Default::default() };

        assert!(classify(&store, &old, &issuer("provider"), &policy, 1_000).is_accepted());
        assert!(
            !classify(&store, &new, &issuer("provider"), &policy, 1_000).is_accepted(),
            "the new release has no evidence of its own"
        );
    }

    /// A release name containing the separator cannot collide with another release's subject — the
    /// length prefix is what prevents it.
    #[test]
    fn a_release_name_cannot_be_crafted_to_collide_with_another_subject() {
        let a = ReleaseId::new("summarizer/1.2.0", "x");
        let b = ReleaseId::new("summarizer", "1.2.0/x");
        assert_ne!(a.subject(), b.subject());
    }

    /// **Step 6: evidence filters, the router ranks.** The accepted survivors come back in the
    /// router's original order — this layer never promotes a well-evidenced candidate past a
    /// better-placed one, because it never moves anything.
    #[test]
    fn filtering_preserves_the_routers_order_and_never_reorders() {
        let candidates = vec!["fine-but-unevidenced", "loaded-and-evidenced", "fine-and-evidenced"];
        let survivors = filter_accepted(candidates, |c| {
            if c.ends_with("evidenced") && *c != "fine-but-unevidenced" {
                Verdict::Accepted { supporting: 2, independent: 2 }
            } else {
                Verdict::InsufficientEvidence {
                    have: 0,
                    need: 1,
                    independent: 0,
                    need_independent: 1,
                }
            }
        });

        assert_eq!(
            survivors,
            vec!["loaded-and-evidenced", "fine-and-evidenced"],
            "the router's order must survive filtering — evidence decides eligibility, not choice"
        );
    }

    /// **Evidence can only narrow.** There is no input by which this module could add a candidate
    /// the caller did not already authorize — demonstrated by the only entry point that returns
    /// candidates at all.
    #[test]
    fn filtering_can_only_ever_remove_candidates() {
        let candidates = vec![1, 2, 3];
        let all = filter_accepted(candidates.clone(), |_| Verdict::Accepted {
            supporting: 9,
            independent: 9,
        });
        assert_eq!(all, candidates, "unanimous support still yields exactly the input set");
        assert!(
            all.len() <= candidates.len(),
            "no quantity of evidence can produce a candidate authorization did not admit"
        );
    }
}
