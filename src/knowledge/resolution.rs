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

use super::store::KnowledgeStore;
use super::{IssuerId, LinkKind, RecordKind};

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
    /// an issuer in no group is its own group.
    ///
    /// This is configured, never inferred — §4's "identity is not independence".
    pub control_groups: Vec<BTreeSet<IssuerId>>,
    /// Evidence older than this is not counted. Independent of the capability's refresh lease, which
    /// is exactly the point: a liveness heartbeat must not launder a stale assessment.
    pub max_evidence_age_ms: u64,
}

impl Default for ReaderPolicy {
    /// A deliberately unhelpful default: it requires evidence and will not silently accept.
    fn default() -> Self {
        Self {
            min_supporting: 1,
            min_independent: 1,
            control_groups: Vec::new(),
            max_evidence_age_ms: u64::MAX,
        }
    }
}

impl ReaderPolicy {
    /// Which control group `issuer` belongs to, as an index. `None` means it is its own group.
    fn group_of(&self, issuer: &IssuerId) -> Option<usize> {
        self.control_groups.iter().position(|g| g.contains(issuer))
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
pub fn classify(
    store: &KnowledgeStore,
    release: &ReleaseId,
    provider: &IssuerId,
    policy: &ReaderPolicy,
    now_ms: u64,
) -> Verdict {
    let subject = release.subject();

    // A set, not a list: one issuer is one supporter however many records it files.
    let mut supporting_issuers: BTreeSet<IssuerId> = BTreeSet::new();
    let mut challenges: Vec<IssuerId> = Vec::new();

    for record in store.about(&subject) {
        // Only a judgement counts. A claim or an observation is not one.
        if record.kind() != RecordKind::Assessment {
            continue;
        }
        // The provider assessing its own release is not evidence about it.
        if record.issuer() == provider {
            continue;
        }
        // Evidence ages on its own timestamp. A capability refresh does not touch this.
        if now_ms.saturating_sub(record.at_ms()) > policy.max_evidence_age_ms {
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

        // A record that both supports and challenges is a challenge: the cautious reading, and the
        // one that cannot be used to manufacture support.
        if challenges_it {
            challenges.push(record.issuer().clone());
        } else if supports {
            supporting_issuers.insert(record.issuer().clone());
        }
    }

    let supporting = supporting_issuers.len();
    let challenging = challenges.len();

    // Independence by control group — configured, never inferred.
    let mut groups: BTreeSet<String> = BTreeSet::new();
    for issuer in &supporting_issuers {
        match policy.group_of(issuer) {
            Some(idx) => {
                groups.insert(format!("g{idx}"));
            }
            // An issuer in no configured group is its own group.
            None => {
                groups.insert(format!("i{}", issuer.as_str()));
            }
        }
    }
    let independent = groups.len();

    // Support and challenge together is a conflict, and it is not this layer's to resolve.
    if challenging > 0 && supporting > 0 {
        return Verdict::Conflicted { supporting, challenging };
    }
    if challenging > 0 {
        return Verdict::Rejected {
            reasons: challenges.into_iter().map(|by| RejectionReason::Challenged { by }).collect(),
        };
    }
    if supporting < policy.min_supporting || independent < policy.min_independent {
        return Verdict::InsufficientEvidence {
            have: supporting,
            need: policy.min_supporting,
            independent,
            need_independent: policy.min_independent,
        };
    }

    Verdict::Accepted { supporting, independent }
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
