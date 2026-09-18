//! **The knowledge layer, end to end** (item 3 PR 6 — the demonstration).
//!
//! Runs the whole lifecycle over one release: claims and observations arrive, an assessment judges
//! them, a reader resolves with its own policy, a basis is retracted and the correction reaches what
//! was built on it — and a competing statement survives all of it.
//!
//! Run with `cargo run --example knowledge_layer`.
//!
//! It ends by printing **what it did not demonstrate**, which for this layer matters more than
//! usual: the headline claim is easy to overstate and the record is explicit that the behavioural
//! half is research-track, not a v3 deliverable.

use mycelium::knowledge::correction::{standing, DependencyIndex, Standing};
use mycelium::knowledge::resolution::{classify, ReaderPolicy, ReleaseId, Verdict};
use mycelium::knowledge::store::KnowledgeStore;
use mycelium::knowledge::{IssuerId, KnowledgeRecord, Link, LinkKind, RecordId, RecordKind};
use std::collections::BTreeSet;

fn issuer(s: &str) -> IssuerId {
    IssuerId::new(s).expect("non-empty")
}

fn record(
    by: &str,
    kind: RecordKind,
    at_ms: u64,
    subject: &str,
    body: &str,
    links: Vec<Link>,
) -> KnowledgeRecord {
    KnowledgeRecord::new(issuer(by), kind, at_ms, subject, body.as_bytes().to_vec(), links)
        .expect("well formed")
}

fn main() {
    let release = ReleaseId::new("soil-summarizer", "1.2.0");
    let subject = release.subject();
    let provider = issuer("greenfield-coop");
    let mut store = KnowledgeStore::new();

    println!("== A release, and what is said about it ==");
    println!("release: {}/{}", release.name, release.version);
    println!("subject: {subject}\n");

    // ---- 1. The provider's own claim. Not evidence — it is what someone said about themselves.
    let claim = record(
        "greenfield-coop",
        RecordKind::Claim,
        1_000,
        &subject,
        "summarises soil reports at 94% agreement with our agronomist",
        vec![],
    );
    store.put(claim);
    println!("1. the provider claims 94% agreement — a CLAIM, which is not evidence");

    // ---- 2. An observation by a field lab. A report, still not a judgement.
    let observation = record(
        "fen-field-lab",
        RecordKind::Observation,
        1_100,
        &subject,
        "ran 200 reports; 12 disagreed with our reading",
        vec![],
    );
    let observation_id = observation.id().clone();
    store.put(observation);
    println!("2. a field lab reports what it saw — an OBSERVATION, still not a judgement");

    // ---- 3. Two independent assessments. These are judgements, with authors.
    for lab in ["fen-field-lab", "broads-soil-trust"] {
        store.put(record(
            lab,
            RecordKind::Assessment,
            1_200,
            &subject,
            "fit for drafting, not for final agronomic advice",
            vec![Link {
                kind: LinkKind::Supports,
                target: RecordId { issuer: provider.clone(), digest: [1u8; 32] },
            }],
        ));
    }
    println!("3. two independent labs ASSESS it — judgements, with authors\n");

    // ---- 4. A reader resolves, with its own policy.
    let policy = ReaderPolicy { min_supporting: 2, min_independent: 2, ..Default::default() };
    let verdict = classify(&store, &release, &provider, &policy, 1_300);
    println!("== A reader resolves ==");
    println!("policy: 2 supporting, 2 independent");
    println!("verdict: {verdict:?}");
    println!("eligible: {}\n", verdict.is_accepted());

    // ---- 5. The same reader, having learned the two labs share a funder.
    let mut group = BTreeSet::new();
    group.insert(issuer("fen-field-lab"));
    group.insert(issuer("broads-soil-trust"));
    let informed = ReaderPolicy {
        min_supporting: 2,
        min_independent: 2,
        control_groups: vec![group],
        ..Default::default()
    };
    let verdict = classify(&store, &release, &provider, &informed, 1_300);
    println!("== The same records; the reader has learned the two labs share a funder ==");
    println!("verdict: {verdict:?}");
    println!("nothing about the RECORDS changed — independence is the READER's judgement\n");

    // ---- 6. A challenge arrives. Fifty supporters would not bury it; one is plenty to show why.
    store.put(record(
        "waveney-growers",
        RecordKind::Assessment,
        1_400,
        &subject,
        "misreads waterlogged samples; we stopped using it",
        vec![Link {
            kind: LinkKind::Challenges,
            target: RecordId { issuer: provider.clone(), digest: [1u8; 32] },
        }],
    ));
    let verdict = classify(&store, &release, &provider, &policy, 1_500);
    println!("== A challenge arrives ==");
    println!("verdict: {verdict:?}");
    match verdict {
        Verdict::Conflicted { .. } => {
            println!("CONFLICTED — the disagreement is PRESERVED, not voted on\n")
        }
        other => println!("unexpected: {other:?}\n"),
    }

    // ---- 7. Correction: the field lab withdraws its observation, and something was built on it.
    let conclusion = record(
        "broads-soil-trust",
        RecordKind::AcceptanceDecision,
        1_600,
        &subject,
        "adopting for draft reports across the catchment",
        vec![Link { kind: LinkKind::DerivedFrom, target: observation_id.clone() }],
    );
    let conclusion_id = conclusion.id().clone();
    store.put(conclusion);

    let held_before = store.len();
    store.put(record(
        "fen-field-lab",
        RecordKind::Observation,
        1_700,
        &subject,
        "withdrawn: our reference meter was out of calibration",
        vec![Link { kind: LinkKind::Retracts, target: observation_id.clone() }],
    ));

    let index = DependencyIndex::build(&store);
    println!("== The field lab withdraws its observation ==");
    println!("records held before: {held_before}, after: {} — nothing erased", store.len());
    println!(
        "the withdrawn observation: {:?}",
        standing(&store, &index, &observation_id, 1_700, u64::MAX).expect("held")
    );

    let downstream = standing(&store, &index, &conclusion_id, 1_700, u64::MAX).expect("held");
    match downstream {
        Standing::BasisWithdrawn { ref basis, hops } => {
            println!("what was built on it:   BasisWithdrawn (basis by {}, {hops} hop)", basis.issuer);
            println!(
                "  → a fact about its SUPPORT, not a verdict on it.\n    \
                 fen-field-lab has no standing to withdraw broads-soil-trust's decision.\n"
            );
        }
        other => println!("what was built on it:   unexpected: {other:?}\n"),
    }

    // ---- What this did not show.
    println!("== What this example does NOT demonstrate ==");
    println!("- that evidence-aware selection picks BETTER providers. That is the behavioural");
    println!("  claim, it is research-track (§13), and it is not a v3 deliverable. This example");
    println!("  shows only that certain things are impossible.");
    println!("- any transport. Every record here is local; heads in the gossip medium and records");
    println!("  in an authorized store is §2's shape, and none of it is exercised.");
    println!("- signature verification. The records are well formed, not signed and checked.");
    println!("- an aggregated reputation score. There is none, deliberately — §9.");
    println!("- that the disagreement in step 6 gets RESOLVED. It does not. It is preserved, and");
    println!("  coordination is expected to continue around it.");
}
