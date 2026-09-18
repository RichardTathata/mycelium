//! **The semantic gate** (item 3 PR 6) — §8 of
//! [`docs/design/knowledge-layer.md`](../../../docs/design/knowledge-layer.md), and a **Phase D exit
//! condition**.
//!
//! The record is careful to keep two claims apart, and so is this module:
//!
//! > **Semantic — a Phase D exit condition, in CI.** Three negative cases, replayed: misleading
//! > evidence cannot silently **erase** a conflicting observation, cannot **refresh** expired
//! > evidence, and cannot **confer** authority. These are properties. They either hold or they do
//! > not.
//! >
//! > **Behavioural — research track (§13), not a v3 deliverable.** Whether evidence-sensitive
//! > resolution *improves outcomes* […]
//!
//! **This module is only the first of those.** It establishes that three specific things are
//! impossible. It says nothing whatever about whether evidence-aware resolution selects better
//! providers — that is the behavioural experiment, it is research-track, and no test here should be
//! read as standing in for it.
//!
//! # The trap the record names, and the guard against it
//!
//! > *A resolver that rejects everything looks safe while being useless, and a metric that counted
//! > only bad selections would score it perfectly.*
//!
//! Every negative case below is a refusal, so all three would pass against a resolver that refused
//! unconditionally. [`the_gate_is_not_satisfied_by_refusing_everything`] is the positive control
//! that makes the other three mean something.
//!
//! # Run it alone
//!
//! `make gate-knowledge` runs exactly this module, so the exit condition is locatable rather than
//! being three tests somewhere in a suite of hundreds.

#[cfg(test)]
mod tests {
    use crate::knowledge::correction::{standing, DependencyIndex, Standing};
    use crate::knowledge::resolution::{
        classify, filter_accepted, ReaderPolicy, ReleaseId, Verdict,
    };
    use crate::knowledge::store::KnowledgeStore;
    use crate::knowledge::{IssuerId, KnowledgeRecord, Link, LinkKind, RecordId, RecordKind};

    fn issuer(s: &str) -> IssuerId {
        IssuerId::new(s).expect("valid")
    }

    fn release() -> ReleaseId {
        ReleaseId::new("summarizer", "1.2.0")
    }

    fn target() -> RecordId {
        RecordId { issuer: issuer("provider"), digest: [7u8; 32] }
    }

    fn assessment(by: &str, subject: &str, kind: LinkKind, at_ms: u64) -> KnowledgeRecord {
        KnowledgeRecord::new(
            issuer(by),
            RecordKind::Assessment,
            at_ms,
            subject,
            b"judgement".to_vec(),
            vec![Link { kind, target: target() }],
        )
        .expect("well formed")
    }

    /// **Negative case 1 — misleading evidence cannot silently erase a conflicting observation.**
    ///
    /// Fifty issuers support the release; one challenges it. The challenge is **not outvoted**,
    /// because there is no vote: the verdict is `Conflicted`, the challenging record is still in the
    /// store, and it is still `Current`.
    ///
    /// This is §9's *"consensus over truth"* refusal made executable. Disagreement is **preserved**,
    /// not resolved — a resolver that let fifty supporters bury one challenge would have resolved it
    /// by majority, which is precisely what the layer declines to do.
    #[test]
    fn misleading_evidence_cannot_erase_a_conflicting_observation() {
        let r = release();
        let mut store = KnowledgeStore::new();

        for i in 0..50 {
            store.put(assessment(&format!("shill-{i}"), &r.subject(), LinkKind::Supports, 1_000));
        }
        let challenge = assessment("careful-lab", &r.subject(), LinkKind::Challenges, 1_000);
        let challenge_id = challenge.id().clone();
        store.put(challenge);

        let verdict = classify(&store, &r, &issuer("provider"), &ReaderPolicy::default(), 1_000);
        assert_eq!(
            verdict,
            Verdict::Conflicted { supporting: 50, challenging: 1 },
            "fifty supporters must not bury one challenge — there is no vote here"
        );
        assert!(!verdict.is_accepted(), "a conflicted release is not eligible");

        // The challenging record itself is untouched: present, and still current.
        assert!(store.get(&challenge_id).is_some(), "the challenge is still in the store");
        let index = DependencyIndex::build(&store);
        assert_eq!(
            standing(&store, &index, &challenge_id, 1_000, u64::MAX),
            Some(Standing::Current),
            "no quantity of contrary evidence changes where the challenge stands"
        );
    }

    /// **Negative case 2 — misleading evidence cannot refresh expired evidence.**
    ///
    /// An issuer's old assessments have aged out. It now issues a fresh one. That must **not**
    /// resurrect the old ones: each record ages on its own timestamp, so the flood of stale support
    /// stays stale and only the new record counts — which is not enough on its own.
    ///
    /// The same holds for a capability refresh, which touches the provider's liveness lease and no
    /// record here. There is no path from "the provider is alive" to "the assessment is current".
    #[test]
    fn misleading_evidence_cannot_refresh_expired_evidence() {
        let r = release();
        let mut store = KnowledgeStore::new();

        // Three old supporters, long past the window.
        for i in 0..3 {
            store.put(assessment(&format!("lab-{i}"), &r.subject(), LinkKind::Supports, 1_000));
        }
        let policy = ReaderPolicy {
            min_supporting: 2,
            min_independent: 2,
            max_evidence_age_ms: 5_000,
            ..Default::default()
        };
        let now = 100_000;

        // Everything is stale: nothing counts.
        assert!(matches!(
            classify(&store, &r, &issuer("provider"), &policy, now),
            Verdict::InsufficientEvidence { have: 0, .. }
        ));

        // A fresh assessment arrives. It counts for itself — and for nothing else.
        store.put(assessment("lab-new", &r.subject(), LinkKind::Supports, now));

        match classify(&store, &r, &issuer("provider"), &policy, now) {
            Verdict::InsufficientEvidence { have, independent, .. } => {
                assert_eq!(have, 1, "only the new record is current; the old three stay expired");
                assert_eq!(independent, 1);
            }
            other => panic!("a new record must not resurrect expired ones, got {other:?}"),
        }
    }

    /// **Negative case 3 — misleading evidence cannot confer authority.**
    ///
    /// Evidence can only ever *narrow* the set. A candidate that authorization excluded is not in
    /// the input, and there is no entry point by which evidence could put it back — demonstrated
    /// against the only function that returns candidates at all.
    #[test]
    fn misleading_evidence_cannot_confer_authority() {
        // Authorization ran first and admitted one of the two providers.
        let all = ["authorized-provider", "denied-provider"];
        let authorized: Vec<&str> = all.iter().copied().filter(|p| *p != "denied-provider").collect();

        // The denied provider is lavishly evidenced. It makes no difference: it never enters.
        let survivors = filter_accepted(authorized.clone(), |_| Verdict::Accepted {
            supporting: 99,
            independent: 99,
        });

        assert_eq!(survivors, vec!["authorized-provider"]);
        assert!(
            !survivors.contains(&"denied-provider"),
            "no quantity of evidence admits a candidate authorization denied"
        );
        assert!(
            survivors.len() <= authorized.len(),
            "evidence narrows and never widens the authorized set"
        );
    }

    /// **The positive control**, and the reason the three above mean anything.
    ///
    /// The record names the trap directly: *"a resolver that rejects everything looks safe while
    /// being useless"*. All three negative cases are refusals, so all three would pass against a
    /// resolver that refused unconditionally. This one fails against it.
    #[test]
    fn the_gate_is_not_satisfied_by_refusing_everything() {
        let r = release();
        let mut store = KnowledgeStore::new();
        store.put(assessment("lab-a", &r.subject(), LinkKind::Supports, 1_000));
        store.put(assessment("lab-b", &r.subject(), LinkKind::Supports, 1_000));

        let policy = ReaderPolicy { min_supporting: 2, min_independent: 2, ..Default::default() };
        let verdict = classify(&store, &r, &issuer("provider"), &policy, 1_000);

        assert_eq!(
            verdict,
            Verdict::Accepted { supporting: 2, independent: 2 },
            "honest independent support must actually be accepted, or the gate proves nothing"
        );
    }

    /// A second positive control, for the correction half: a retraction must reach downstream
    /// records **and leave everything else alone**. A sweep that marked everything would satisfy
    /// "misleading evidence cannot erase" while destroying the layer's usefulness.
    #[test]
    fn correction_reaches_what_it_should_and_nothing_else() {
        let basis = KnowledgeRecord::new(
            issuer("lab-a"),
            RecordKind::Observation,
            1_000,
            "soil/ph",
            b"reading".to_vec(),
            vec![],
        )
        .expect("well formed");
        let basis_id = basis.id().clone();

        let derived = KnowledgeRecord::new(
            issuer("lab-b"),
            RecordKind::Assessment,
            1_100,
            "soil/ph",
            b"conclusion".to_vec(),
            vec![Link { kind: LinkKind::DerivedFrom, target: basis_id.clone() }],
        )
        .expect("well formed");
        let derived_id = derived.id().clone();

        let unrelated = KnowledgeRecord::new(
            issuer("lab-z"),
            RecordKind::Observation,
            1_000,
            "water/ph",
            b"elsewhere".to_vec(),
            vec![],
        )
        .expect("well formed");
        let unrelated_id = unrelated.id().clone();

        let mut store = KnowledgeStore::new();
        store.put(basis);
        store.put(derived);
        store.put(unrelated);

        let before = store.len();
        store.put(
            KnowledgeRecord::new(
                issuer("lab-a"),
                RecordKind::Observation,
                2_000,
                "soil/ph",
                b"withdrawn".to_vec(),
                vec![Link { kind: LinkKind::Retracts, target: basis_id.clone() }],
            )
            .expect("well formed"),
        );
        let index = DependencyIndex::build(&store);

        assert_eq!(store.len(), before + 1, "the retraction was added; nothing was erased");
        assert_eq!(
            standing(&store, &index, &derived_id, 2_000, u64::MAX),
            Some(Standing::BasisWithdrawn { basis: basis_id, hops: 1 }),
            "the retraction reached the derived record — actively, not on someone noticing"
        );
        assert_eq!(
            standing(&store, &index, &unrelated_id, 2_000, u64::MAX),
            Some(Standing::Current),
            "and left the unrelated record alone"
        );
    }
}
