//! The side-effect contract's **compensation and evidence** (adopter-surface X5), under the attesting
//! build: a compensation is refused without the evaluator's permit, refused when the evaluator denies,
//! and when recorded carries the compensated record's hash as its parent — in the evidence journal, where
//! `mycelium explain --closure` reads it. Seen failing first under a plant that skipped the permit check.
#![cfg(feature = "envelope")]

mod mock_http;

use std::sync::Arc;
use std::time::Duration;

use mock_http::MockHttp;
use mycelium::{read_evidence_journal, ActionEvaluator, AeEvidence, EvidenceJournal, EvidenceProfile, Execution, NodeId, RecordKind, ReferenceEvaluator, Rule};
use mycelium_effects::{
    AttemptId, CompensationPermit, DedupOutcome, Effect, EffectClass, EffectRefusal, EffectState, EgressPolicy, ExternalEffect,
    ExternalRefusal, HttpDestination, OperationId, ProbeBudget, Reconciler,
};

fn temp(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!("mycelium-x5e-{tag}-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&p);
    let _ = std::fs::remove_file(&p);
    p
}

fn external(op: &str, payload: &str, class: EffectClass) -> ExternalEffect {
    let op = OperationId::new(op);
    ExternalEffect {
        effect: Effect::new(op.clone(), AttemptId::of(&op, 1), payload.as_bytes().to_vec()),
        class,
        probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 60_000 },
        composition: None,
    }
}

fn records(path: &std::path::Path) -> Vec<(AeEvidence, String)> {
    read_evidence_journal(path)
        .unwrap()
        .into_iter()
        .map(|bytes| {
            let ev: AeEvidence = serde_json::from_slice(&bytes).unwrap();
            let hash = {
                use sha2::{Digest, Sha256};
                Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
            };
            (ev, hash)
        })
        .collect()
}

#[tokio::test]
async fn a_compensation_needs_the_evaluators_permit_and_names_the_record_it_compensates() {
    let mock = MockHttp::start();
    let dest = Arc::new(HttpDestination::new("payments", "127.0.0.1", mock.port, "", Duration::from_millis(700)));
    let dir = temp("comp");
    std::fs::create_dir_all(&dir).unwrap();
    let journal_path = dir.join("evidence.log");
    let journal = EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).unwrap();
    let via = NodeId::new("127.0.0.1", 7001).unwrap();
    let r = Reconciler::open(dir.join("ledger.sqlite"), Arc::clone(&dest), EgressPolicy::default())
        .unwrap()
        .with_evidence(Arc::clone(&journal), via.clone());

    // The effect, committed and recorded.
    let paid = external("pay-20", "20", EffectClass::Compensatable { compensation: "refund".into() });
    assert_eq!(r.apply(&paid, None).await.unwrap().dedup, DedupOutcome::Fresh);
    let paid_record = r.record(&paid.effect.operation_id).unwrap().unwrap();
    assert_eq!(paid_record.state, EffectState::Committed);
    let paid_hash = paid_record.evidence_hash.clone().expect("the execution record's hash is on the ledger");
    let journal_records = records(&journal_path);
    assert!(journal_records.iter().any(|(ev, h)| *h == paid_hash && ev.kind == RecordKind::Execution && ev.execution == Execution::Completed && ev.operation == "effect.apply" && ev.destination.as_deref() == Some("payments")), "the apply's execution record, hashed as the journal hashes it: {journal_records:?}");

    // Without a permit: refused, nothing sent.
    let refund = external("refund-20", "-20", EffectClass::Idempotent { compensation: None });
    let reached = mock.reached();
    assert!(matches!(r.compensate(&paid.effect.operation_id, &refund, None).await, Err(ExternalRefusal::CompensationNotPermitted(_))));
    assert_eq!(mock.reached(), reached);
    assert_eq!(r.counts().external.compensation_not_permitted, 1);

    // The evaluator denies: no permit exists to present.
    let denying: Arc<dyn ActionEvaluator> = Arc::new(ReferenceEvaluator::new("rev-1").map_action("refund", "*"));
    let now_ms = mycelium::sim_seam::wall_now_ms();
    assert!(matches!(
        CompensationPermit::decide(&denying, "token:gw/clerk", via.clone(), "payments", "refund", &paid_hash, now_ms),
        Err(ExternalRefusal::CompensationNotPermitted(_))
    ));

    // The evaluator permits: the compensation is sent, and its record's parent is the compensated record's hash.
    let permitting: Arc<dyn ActionEvaluator> = Arc::new(
        ReferenceEvaluator::new("rev-1").map_action("refund", "*").allow(Rule::new("token:gw/clerk", "refund", "destination:payments")),
    );
    let permit = CompensationPermit::decide(&permitting, "token:gw/clerk", via.clone(), "payments", "refund", &paid_hash, now_ms).expect("permitted");
    assert_eq!(permit.policy_revision(), "rev-1");
    // A permit for something else does not permit this compensation (#674's review, D4): another operation,
    // another destination, another parent — each refused by name, nothing sent. Seen failing first under a
    // plant that skipped the binding.
    let wide: Arc<dyn ActionEvaluator> = Arc::new(
        ReferenceEvaluator::new("rev-1")
            .map_action("weigh", "*")
            .map_action("refund", "*")
            .allow(Rule::new("token:gw/clerk", "weigh", "destination:payments"))
            .allow(Rule::new("token:gw/clerk", "refund", "destination:invoices"))
            .allow(Rule::new("token:gw/clerk", "refund", "destination:payments")),
    );
    let reached = mock.reached();
    for (op, dest, parent, why) in [
        ("weigh", "payments", paid_hash.as_str(), "another operation"),
        ("refund", "invoices", paid_hash.as_str(), "another destination"),
        ("refund", "payments", "0000000000000000000000000000000000000000000000000000000000000000", "another parent"),
    ] {
        let wrong = CompensationPermit::decide(&wide, "token:gw/clerk", via.clone(), dest, op, parent, now_ms).expect("the evaluator permits it, for what it is");
        assert!(matches!(r.compensate(&paid.effect.operation_id, &refund, Some(&wrong)).await, Err(ExternalRefusal::CompensationNotPermitted(_))), "{why}");
    }
    assert_eq!(mock.reached(), reached, "nothing left for a permit that is not for this compensation");
    assert_eq!(r.counts().external.compensation_not_permitted, 4);
    let commit = r.compensate(&paid.effect.operation_id, &refund, Some(&permit)).await.expect("compensated");
    assert_eq!(commit.dedup, DedupOutcome::Fresh);
    assert_eq!(mock.applied(), 2, "the compensation is an effect of its own at the destination");
    assert_eq!(r.record(&paid.effect.operation_id).unwrap().unwrap().state, EffectState::Compensated);
    assert_eq!(r.record(&paid.effect.operation_id).unwrap().unwrap().compensated_by.as_deref(), Some("refund-20"));
    let journal_records = records(&journal_path);
    let decision = journal_records.iter().find(|(ev, _)| ev.kind == RecordKind::Decided && ev.operation == "refund").expect("the permit's decision is recorded");
    assert_eq!(decision.0.parent.as_deref(), Some(paid_hash.as_str()), "the decision names what it compensates");
    let execution = journal_records.iter().find(|(ev, _)| ev.kind == RecordKind::Execution && ev.operation == "refund" && ev.operation_id == "refund-20").expect("the compensation's execution record, under the permit's own envelope");
    assert_eq!(execution.0.parent.as_deref(), Some(paid_hash.as_str()), "the compensation's record names the compensated record's hash");
    assert_eq!(execution.0.execution, Execution::Completed);
    assert_eq!((execution.0.decision, execution.0.policy_revision.as_str()), (mycelium::DecisionKind::Permit, "rev-1"), "the evaluator's decision, not a fabricated one (#674's review, Q1)");
    let apply = journal_records.iter().find(|(ev, _)| ev.operation_id == "pay-20" && ev.operation == "effect.apply").unwrap();
    assert_eq!(apply.0.decision, mycelium::DecisionKind::Indeterminate, "an uncomposed effect: no policy decided it, and the record says so");
    assert!(apply.0.effect_durability.is_none(), "a remote destination's commit is not this node's rung");
    // The closure reader follows the parent: the compensated record and its compensation are one closure.
    let corpus = mycelium::closure::Corpus::load(std::slice::from_ref(&journal_path)).unwrap();
    let members = corpus.closure_members(&paid_hash);
    assert!(members.iter().any(|m| m == &execution.1), "the compensation is in the compensated record's closure: {members:?}");

    // A second compensation of the same record is refused by state.
    assert!(matches!(r.compensate(&paid.effect.operation_id, &refund, Some(&permit)).await, Err(ExternalRefusal::State { state: EffectState::Compensated, .. })));

    // A compensation whose answer is lost does not strand the record it compensates (#674's review, D5): the
    // link is kept, a second compensation is refused meanwhile, and when a probe settles the compensation the
    // original becomes `Compensated`.
    let paid2 = external("pay-22", "22", EffectClass::Compensatable { compensation: "refund".into() });
    assert_eq!(r.apply(&paid2, None).await.unwrap().dedup, DedupOutcome::Fresh);
    let paid2_hash = r.record(&paid2.effect.operation_id).unwrap().unwrap().evidence_hash.unwrap();
    let permit2 = CompensationPermit::decide(&permitting, "token:gw/clerk", via.clone(), "payments", "refund", &paid2_hash, now_ms).unwrap();
    let lost_refund = external("refund-22", "lose:-22", EffectClass::Idempotent { compensation: None });
    assert!(matches!(r.compensate(&paid2.effect.operation_id, &lost_refund, Some(&permit2)).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    let linked = r.record(&paid2.effect.operation_id).unwrap().unwrap();
    assert_eq!((linked.state, linked.compensated_by.as_deref()), (EffectState::Committed, Some("refund-22")), "linked, not yet compensated");
    assert!(matches!(r.compensate(&paid2.effect.operation_id, &lost_refund, Some(&permit2)).await, Err(ExternalRefusal::State { .. })), "a second compensation while one is in flight is refused");
    assert_eq!(r.probe(&lost_refund.effect.operation_id).await, Ok(mycelium_effects::ProbeAnswer::Done));
    assert_eq!(r.record(&paid2.effect.operation_id).unwrap().unwrap().state, EffectState::Compensated, "the probe that settled the compensation compensated the original");
    // An unknown effect on a lost answer is recorded `Attempted`, and an unresolved one `Unresolved`.
    let mut lost = external("pay-21", "lose:21", EffectClass::Idempotent { compensation: None });
    lost.probe_budget = ProbeBudget { max_probes: 0, max_age_ms: 60_000 };
    assert!(matches!(r.apply(&lost, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert!(matches!(r.probe(&lost.effect.operation_id).await, Err(ExternalRefusal::ProbeBudgetExhausted { .. })));
    let journal_records = records(&journal_path);
    assert!(journal_records.iter().any(|(ev, _)| ev.operation_id == "pay-21" && ev.execution == Execution::Attempted && ev.operation == "effect.apply"));
    let unresolved = journal_records.iter().find(|(ev, _)| ev.operation_id == "pay-21" && ev.execution == Execution::Unresolved).expect("the unresolved record");
    assert_eq!(unresolved.0.operation, "effect.probe");
    assert!(unresolved.0.parent.is_some(), "the escalation names the effect it escalates");
    let _ = std::fs::remove_dir_all(&dir);
}
