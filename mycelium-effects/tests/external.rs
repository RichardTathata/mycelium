//! The external side-effect contract (adopter-surface X5), exit by exit, against a destination outside
//! the fleet that loses responses on demand (`mock_http`). Each test was seen failing first under a plant
//! that removed the rule it names (the PR records which).

mod mock_http;

use std::sync::Arc;
use std::time::Duration;

use mock_http::MockHttp;
use mycelium_effects::{
    AttemptId, DedupOutcome, Effect, EffectClass, EffectRefusal, EffectState, EgressPolicy, ExternalEffect, ExternalRefusal,
    HttpDestination, OperationId, ProbeAnswer, ProbeBudget, Reconciler,
};

fn ledger_path(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!("mycelium-x5-{tag}-{}-{}.sqlite", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_file(&p);
    p
}

fn destination(mock: &MockHttp) -> Arc<HttpDestination> {
    Arc::new(HttpDestination::new("payments", "127.0.0.1", mock.port, "", Duration::from_millis(700)))
}

fn reconciler(tag: &str, mock: &MockHttp) -> Reconciler<HttpDestination> {
    Reconciler::open(ledger_path(tag), destination(mock), EgressPolicy::default()).unwrap()
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

fn idempotent(op: &str, payload: &str) -> ExternalEffect {
    external(op, payload, EffectClass::Idempotent { compensation: None })
}

/// **A lost response yields `DeliveryUnknown`, and a retry without a probe is refused.** The destination
/// applied the effect and the answer was lost: the record is `Unknown`, a retry is refused by name and
/// counted, and the destination was reached once.
#[tokio::test]
async fn a_lost_response_is_unknown_and_a_retry_without_a_probe_is_refused() {
    let mock = MockHttp::start();
    let r = reconciler("lost", &mock);
    let ext = idempotent("pay-1", "lose:42");
    assert_eq!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown)));
    let record = r.record(&ext.effect.operation_id).unwrap().expect("prepared before the send");
    assert_eq!(record.state, EffectState::Unknown);
    assert!(record.first_unknown_at_ms.is_some());
    assert_eq!(record.class, ext.class, "the class was recorded on the prepared record");
    assert_eq!(r.retry(&ext).await, Err(ExternalRefusal::RetryWithoutProbe));
    assert_eq!(r.apply(&ext, None).await, Err(ExternalRefusal::RetryWithoutProbe), "a second apply is a retry too");
    assert_eq!(r.counts().external.retry_without_probe, 2);
    assert_eq!(r.counts().delivery_unknown, 1);
    assert_eq!(mock.reached(), 1, "the destination was reached once");
}

/// **A probe answering `Done` produces no second effect and settles the record.**
#[tokio::test]
async fn a_probe_answering_done_settles_the_record_without_a_second_effect() {
    let mock = MockHttp::start();
    let r = reconciler("done", &mock);
    let ext = idempotent("pay-2", "lose:7");
    assert!(matches!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert_eq!(r.probe(&ext.effect.operation_id).await, Ok(ProbeAnswer::Done));
    let record = r.record(&ext.effect.operation_id).unwrap().unwrap();
    assert_eq!(record.state, EffectState::Committed);
    assert_eq!(record.probes, 1);
    assert_eq!(mock.applied(), 1, "one effect at the destination");
    // Settled: a retry is refused by state, and a re-apply answers from the ledger without a send.
    assert!(matches!(r.retry(&ext).await, Err(ExternalRefusal::State { state: EffectState::Committed, .. })));
    let reached = mock.reached();
    let again = r.apply(&ext, None).await.unwrap();
    assert_eq!(again.dedup, DedupOutcome::Replayed);
    assert_eq!(mock.reached(), reached, "the ledger answered; the destination was not asked");
    assert_eq!(r.counts().commits, 1);
}

/// **A probe answering `NotDone` permits one retry under the same key, and the destination sees one
/// effect**; a second retry is refused.
#[tokio::test]
async fn a_probe_answering_not_done_permits_one_retry_and_the_destination_sees_one_effect() {
    let mock = MockHttp::start();
    let r = reconciler("notdone", &mock);
    let op = "pay-3";
    let first = idempotent(op, "vanish:9");
    assert!(matches!(r.apply(&first, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert_eq!(r.probe(&first.effect.operation_id).await, Ok(ProbeAnswer::NotDone));
    assert_eq!(r.record(&first.effect.operation_id).unwrap().unwrap().state, EffectState::NotDone);
    // The retry: the same key, a payload the destination answers.
    let mut retry = idempotent(op, "9");
    retry.effect.content_hash = first.effect.content_hash; // the same logical content under the key
    let commit = r.retry(&retry).await.expect("one retry after not-done");
    assert_eq!(commit.dedup, DedupOutcome::Fresh);
    assert_eq!(mock.applied(), 1, "the destination saw one effect");
    assert_eq!(r.record(&first.effect.operation_id).unwrap().unwrap().retries, 1);
    // A second retry, whatever happened, is refused.
    assert!(matches!(r.retry(&retry).await, Err(ExternalRefusal::State { state: EffectState::Committed, .. })));
    // And on a record probed not-done twice, the second retry is refused by count: arrange one.
    let second = idempotent("pay-3b", "vanish:1");
    assert!(matches!(r.apply(&second, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert_eq!(r.probe(&second.effect.operation_id).await, Ok(ProbeAnswer::NotDone));
    let mut lost_retry = idempotent("pay-3b", "vanish:1");
    lost_retry.effect.content_hash = second.effect.content_hash;
    assert!(matches!(r.retry(&lost_retry).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))), "the retry's answer was lost too");
    assert_eq!(r.probe(&second.effect.operation_id).await, Ok(ProbeAnswer::NotDone));
    assert_eq!(r.retry(&lost_retry).await, Err(ExternalRefusal::SecondRetry));
    assert_eq!(r.counts().external.second_retry, 1);
}

/// **A destination that declares itself non-idempotent is never retried, whatever the probe says.**
#[tokio::test]
async fn a_non_idempotent_destination_is_never_retried_whatever_the_probe_says() {
    let mock = MockHttp::start();
    let r = reconciler("nonidem", &mock);
    let ext = external("pay-4", "vanish:3", EffectClass::Compensatable { compensation: "refund".into() });
    assert!(matches!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert_eq!(r.probe(&ext.effect.operation_id).await, Ok(ProbeAnswer::NotDone), "the probe says not done");
    assert_eq!(r.retry(&ext).await, Err(ExternalRefusal::RetryNotIdempotent { class: "compensatable" }));
    assert_eq!(mock.applied(), 0);
    assert_eq!(mock.reached(), 2, "the apply and the probe; no retry left");
    assert_eq!(r.counts().external.retry_not_idempotent, 1);
}

/// **An irreversible, unobservable effect on unknown stays unknown**: no retry, no probe, the record
/// readable as such.
#[tokio::test]
async fn an_irreversible_unobservable_effect_stays_unknown() {
    let mock = MockHttp::start();
    let r = reconciler("irreversible", &mock);
    let ext = external("launch-1", "lose:go", EffectClass::Irreversible);
    assert!(matches!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert_eq!(r.probe(&ext.effect.operation_id).await, Err(ExternalRefusal::IrreversibleUnobservable));
    assert_eq!(r.retry(&ext).await, Err(ExternalRefusal::RetryNotIdempotent { class: "irreversible" }));
    let record = r.record(&ext.effect.operation_id).unwrap().unwrap();
    assert_eq!(record.state, EffectState::Unknown, "the uncertainty is kept");
    assert_eq!(record.probes, 0);
    assert_eq!(mock.reached(), 1, "nothing after the apply reached the destination");
    assert_eq!(r.counts().external.irreversible_unobservable, 1);
    assert!(r.compensate(&ext.effect.operation_id, &idempotent("undo-1", "x"), None).await.is_err(), "nothing compensates an irreversible effect");
}

/// **A probe past its budget marks the record unresolved and escalated, and a further probe is refused;
/// a probe to a host the egress list denies is refused by the gate, not by the destination.**
#[tokio::test]
async fn a_probe_past_its_budget_is_unresolved_and_egress_gates_a_probe() {
    let mock = MockHttp::start();
    let r = reconciler("budget", &mock);
    let mut ext = idempotent("pay-5", "lose:5");
    ext.probe_budget = ProbeBudget { max_probes: 2, max_age_ms: 60_000 };
    assert!(matches!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    mock.state.probes_silent.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(r.probe(&ext.effect.operation_id).await, Ok(ProbeAnswer::Unknown));
    assert_eq!(r.probe(&ext.effect.operation_id).await, Ok(ProbeAnswer::Unknown));
    let reached = mock.reached();
    assert!(matches!(r.probe(&ext.effect.operation_id).await, Err(ExternalRefusal::ProbeBudgetExhausted { probes: 2, .. })));
    let record = r.record(&ext.effect.operation_id).unwrap().unwrap();
    assert_eq!(record.state, EffectState::Unresolved);
    assert_eq!(r.probe(&ext.effect.operation_id).await, Err(ExternalRefusal::Unresolved));
    assert_eq!(r.retry(&ext).await, Err(ExternalRefusal::Unresolved));
    assert_eq!(mock.reached(), reached, "past the budget the destination is not asked again");
    assert_eq!(r.counts().external.probe_budget_exhausted, 1);
    assert_eq!(r.counts().external.unresolved, 2);

    // The age bound: a budget of zero age is exhausted at the first probe.
    let mut aged = idempotent("pay-5b", "lose:5");
    aged.probe_budget = ProbeBudget { max_probes: 9, max_age_ms: 0 };
    assert!(matches!(r.apply(&aged, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    assert!(matches!(r.probe(&aged.effect.operation_id).await, Err(ExternalRefusal::ProbeBudgetExhausted { probes: 0, .. })));

    // Egress: a gate that does not name the host refuses the probe (and the effect) before the destination.
    mock.state.probes_silent.store(false, std::sync::atomic::Ordering::SeqCst);
    let gated = Reconciler::open(ledger_path("egress"), destination(&mock), EgressPolicy { allow_hosts: vec!["payments.internal".into()] }).unwrap();
    let reached = mock.reached();
    let ext = idempotent("pay-6", "6");
    assert_eq!(gated.apply(&ext, None).await, Err(ExternalRefusal::EgressDenied { host: "127.0.0.1".into() }));
    assert!(gated.record(&ext.effect.operation_id).unwrap().is_none(), "refused before the record was even prepared");
    assert_eq!(mock.reached(), reached);
    // A record that went unknown under an allowing policy, probed under a denying one: the gate refuses.
    let shared = ledger_path("egress-open");
    let open = Reconciler::open(&shared, destination(&mock), EgressPolicy::default()).unwrap();
    let lost = idempotent("pay-7", "lose:7");
    assert!(matches!(open.apply(&lost, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown))));
    let denying = Reconciler::open(&shared, destination(&mock), EgressPolicy { allow_hosts: vec!["payments.internal".into()] }).unwrap();
    let reached = mock.reached();
    assert_eq!(denying.probe(&lost.effect.operation_id).await, Err(ExternalRefusal::EgressDenied { host: "127.0.0.1".into() }));
    assert_eq!(mock.reached(), reached, "the gate, not the destination, refused the probe");
    assert_eq!(denying.counts().external.egress_denied, 1);
}

/// **The composed legs are still enforced on the external path.** An effect attributed to someone else's
/// mandate is refused by leg before the record is prepared; an unauthorised operation likewise; a composed
/// effect without an authority to check against is refused, never sent; the one that holds commits.
#[tokio::test]
async fn the_composed_legs_are_enforced_on_the_external_path() {
    use mycelium_effects::{CompositionLeg, Composition, Mandate, PrincipalId, ResourceAuthority, TermId};
    let mock = MockHttp::start();
    let r = reconciler("composed", &mock);
    let principal = |s: &str| PrincipalId::new(s).unwrap();
    let mandate = |holder: &str, ops: &[&str]| Mandate {
        holder: principal(holder),
        established_by: principal("coop-board"),
        purpose: "pay the growers".into(),
        scope: "depot/payments".into(),
        operations: ops.iter().map(|s| s.to_string()).collect(),
        epoch: 1,
        term: TermId::new("term-1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: u64::MAX,
    };
    let authority = ResourceAuthority::new("depot/payments", 1);
    let composed = |op: &str, who: &str, m: Mandate| ExternalEffect {
        composition: Some(Composition { principal: principal(who), operation: "pay".into(), mandate: m, origin_domain: None }),
        ..idempotent(op, "10")
    };
    // Attribution: presented under another holder's mandate.
    let e = composed("pay-8", "node:a", mandate("node:b", &["pay"]));
    assert!(matches!(r.apply(&e, Some(&authority)).await, Err(ExternalRefusal::Effect(EffectRefusal::Unauthorised { leg: CompositionLeg::Attribution, .. }))));
    // Authority: the operation is not enumerated.
    let e = composed("pay-9", "node:a", mandate("node:a", &["weigh"]));
    assert!(matches!(r.apply(&e, Some(&authority)).await, Err(ExternalRefusal::Effect(EffectRefusal::Unauthorised { leg: CompositionLeg::Authority, .. }))));
    // No authority to check against: refused, not sent.
    let e = composed("pay-10", "node:a", mandate("node:a", &["pay"]));
    assert!(matches!(r.apply(&e, None).await, Err(ExternalRefusal::Effect(EffectRefusal::Unauthorised { leg: CompositionLeg::Authority, .. }))));
    assert_eq!(mock.reached(), 0, "nothing reached the destination");
    assert!(r.record(&OperationId::new("pay-8")).unwrap().is_none(), "no record was prepared for a refused composition");
    assert_eq!(r.counts().unauthorised_attribution, 1);
    assert_eq!(r.counts().unauthorised_authority, 2);
    // The one that holds.
    let commit = r.apply(&e, Some(&authority)).await.unwrap();
    assert_eq!(commit.dedup, DedupOutcome::Fresh);
    assert_eq!(mock.applied(), 1);
}

/// **A failed effect starts clean**: a 500 leaves nothing at the destination, the record is `Failed`, and
/// a later apply under the same operation is sent again (not a retry: nothing was ever unknown).
#[tokio::test]
async fn a_failed_effect_starts_clean_and_a_conflict_is_named() {
    let mock = MockHttp::start();
    let r = reconciler("failed", &mock);
    let bad = idempotent("pay-11", "boom");
    assert!(matches!(r.apply(&bad, None).await, Err(ExternalRefusal::Effect(EffectRefusal::Failed(_)))));
    assert_eq!(r.record(&bad.effect.operation_id).unwrap().unwrap().state, EffectState::Failed);
    let good = idempotent("pay-11", "11");
    assert_eq!(r.apply(&good, None).await.unwrap().dedup, DedupOutcome::Fresh);
    // Same key, other content, at the destination: a conflict, and the ledger says failed (nothing new applied).
    let other = Reconciler::open(ledger_path("conflict"), destination(&mock), EgressPolicy::default()).unwrap();
    let changed = idempotent("pay-11", "12");
    assert!(matches!(other.apply(&changed, None).await, Err(ExternalRefusal::Effect(EffectRefusal::Conflict { .. }))));
    assert_eq!(mock.applied(), 1);
}

/// **The SQLite destination is an external destination too**: local (no host, so no egress gate), its
/// probe the dedup row — `Done` after an apply, `NotDone` for a key it never saw.
#[tokio::test]
async fn the_sqlite_destination_answers_probes_from_its_dedup_row() {
    use mycelium_effects::{ExternalDestination, IdempotencyKey, SqliteDestination};
    let handler: mycelium_effects::sqlite::Handler = Arc::new(|_, _| Ok(()));
    let dest = Arc::new(SqliteDestination::open(ledger_path("sqlite-dest"), "ledger", handler).unwrap());
    let r = Reconciler::open(ledger_path("sqlite-rec"), Arc::clone(&dest), EgressPolicy { allow_hosts: vec!["nothing".into()] }).unwrap();
    let ext = idempotent("row-1", "1");
    assert_eq!(r.apply(&ext, None).await.unwrap().dedup, DedupOutcome::Fresh, "a local destination is not egress");
    assert_eq!(dest.observe(&IdempotencyKey { scope: "ledger".into(), operation_id: ext.effect.operation_id.clone() }), ProbeAnswer::Done);
    assert_eq!(dest.observe(&IdempotencyKey { scope: "ledger".into(), operation_id: OperationId::new("row-never") }), ProbeAnswer::NotDone);
    assert_eq!(dest.host(), None);
}
