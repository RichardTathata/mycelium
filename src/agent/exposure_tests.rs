//! Adopter-surface Y5's promises: only work that extends an unresolved closure is held, on two thresholds that fire on
//! their own; a compensation is admitted while a dependent call is held; the shadow rung counts and holds nothing;
//! unknown coverage fails closed; the fleet tally's coverage and collection rules. The pure half runs in every build;
//! the doors' half, under `gateway` and `tls`, drives `ae_preflight` — the function every door runs — on a real node.

use super::*;

const H: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const RUN: &str = "run-7";

fn rule(max_unresolved: Option<u32>, max_age: Option<u64>, scope: &str) -> ExposureRule {
    ExposureRule { actor: "*".into(), operation: "tools/call".into(), resource: "*".into(), limit: ExposureLimit::new(max_unresolved, max_age, scope).unwrap() }
}

fn view(unresolved: u32, oldest_at_ms: Option<u64>) -> Exposure {
    Exposure::View { exposure: ClosureExposure { unresolved, oldest_at_ms }, scope: ExposureScope::EnforcementPoint, publishers: vec!["me".into()] }
}

fn record(hash: &str, correlation: Option<&str>, t: u64) -> UnresolvedRecord {
    UnresolvedRecord { record_hash: hash.into(), evidence_hashes: Vec::new(), correlation: correlation.map(str::to_string), prepared_at_ms: t }
}

const PARENT_H: ClosureKey<'static> = ClosureKey { parent: Some(H), correlation: None };

/// **Exit: the aged-record threshold fires with the size threshold unmet.** One unresolved effect against a size
/// threshold of ten, prepared five seconds before a one-second age threshold: held, naming the age. The same record a
/// half-second old is clear. Seen failing first with the age arm removed from `judge` (the call was clear).
#[test]
fn the_aged_record_threshold_fires_with_the_size_threshold_unmet() {
    let r = rule(Some(10), Some(1_000), "enforcement_point");
    let now = 100_000;
    match judge(&r, &view(1, Some(now - 5_000)), now) {
        Judgement::Hold(why) => assert!(why.contains("5000 ms old") && why.contains("age threshold of 1000 ms"), "{why}"),
        other => panic!("an aged record past its threshold did not hold: {other:?}"),
    }
    assert_eq!(judge(&r, &view(1, Some(now - 500)), now), Judgement::Clear, "inside both thresholds");
    assert!(matches!(judge(&r, &view(10, Some(now)), now), Judgement::Hold(w) if w.contains("size threshold is 10")));
    assert_eq!(judge(&r, &view(9, Some(now)), now), Judgement::Clear);
    assert_eq!(judge(&r, &view(0, None), now), Judgement::Clear);
}

#[test]
fn a_closure_that_cannot_be_read_is_not_established_never_empty() {
    let r = rule(Some(1), None, "enforcement_point");
    assert!(matches!(judge(&r, &Exposure::Unavailable("no source".into()), 1), Judgement::NotEstablished(w) if w.contains("no source")));
}

#[test]
fn a_limit_is_refused_by_name_without_a_threshold_at_zero_or_at_consensus() {
    assert_eq!(ExposureLimit::new(None, None, "enforcement_point"), Err(ExposureRuleError::NoThreshold));
    assert_eq!(ExposureLimit::new(Some(0), None, "enforcement_point"), Err(ExposureRuleError::ZeroSize));
    assert_eq!(ExposureLimit::new(None, Some(0), "enforcement_point"), Err(ExposureRuleError::ZeroAge));
    let e = ExposureLimit::new(Some(1), None, "fleet_consensus").unwrap_err();
    assert!(e.to_string().contains("not built"), "{e}");
    assert!(ExposureLimit::new(Some(1), None, "fleet").is_err(), "an unknown scope is refused");
    assert_eq!(ExposureLimit::new(Some(1), Some(5), "fleet_tally").unwrap().scope, ExposureScope::FleetTally);
}

/// **A request extends a closure only by naming it** (#701's review, finding 1): its parent an unresolved record's
/// prepared hash or evidence hash, or its correlation that record's run. Anything else is independent.
#[test]
fn a_request_extends_a_closure_only_by_its_causal_fields() {
    let r = UnresolvedRecord { evidence_hashes: vec!["5555".into(), OTHER.into()], ..record(H, Some(RUN), 5) };
    assert!(r.extended_by(&ClosureKey { parent: Some(H), correlation: None }), "the prepared record");
    assert!(r.extended_by(&ClosureKey { parent: Some(OTHER), correlation: None }), "the latest execution evidence");
    assert!(r.extended_by(&ClosureKey { parent: Some("5555"), correlation: None }), "an earlier execution evidence");
    assert!(r.extended_by(&ClosureKey { parent: None, correlation: Some(RUN) }), "the run");
    assert!(!r.extended_by(&ClosureKey { parent: Some("3333"), correlation: Some("run-8") }));
    assert!(!r.extended_by(&ClosureKey::default()));
    assert!(ClosureKey::default().is_root());
}

/// **The decisive rule at a door:** a rule that selects every operation still never holds the reconciler's moves that
/// resolve a closure or a grant — named exactly, so `effect.apply` and `effect.retry` are dependent work (#701's
/// review, question 9).
#[test]
fn the_reconcilers_own_work_and_a_grant_are_never_held() {
    let any = ExposurePolicy::new(vec![ExposureRule { operation: "*".into(), ..rule(Some(1), None, "enforcement_point") }]);
    for op in EXEMPT_OPERATIONS {
        let g = gate(&any, "a", op, "destination:pay", &PARENT_H, true, 1, |_| view(100, Some(0)));
        assert!(matches!(g, Gate::Exempt(_)), "{op}: {g:?}");
    }
    for op in ["effect.apply", "effect.retry", "effect.probes", "grant.issuer", "tools/call"] {
        assert!(matches!(gate(&any, "a", op, "x", &PARENT_H, true, 1, |_| view(100, Some(0))), Gate::Hold(_)), "{op} is dependent work");
    }
}

/// **In shadow a hold proceeds** — the pure half: not enforcing, a threshold reached is `ShadowWouldHold`; unknown
/// coverage is `ShadowWouldNotEstablish`; nothing is held. And the gate counts nothing itself.
#[test]
fn in_shadow_a_hold_proceeds() {
    let p = ExposurePolicy::new(vec![rule(Some(2), None, "enforcement_point")]);
    match gate(&p, "a", "tools/call", "t", &PARENT_H, false, 1, |_| view(5, Some(0))) {
        Gate::ShadowWouldHold(note) => assert!(note.starts_with("shadow") && note.contains("size threshold is 2"), "{note}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(gate(&p, "a", "tools/call", "t", &PARENT_H, false, 1, |_| Exposure::Unavailable("x".into())), Gate::ShadowWouldNotEstablish(_)));
}

/// A root request — no parent, no correlation — and one whose fields name no unresolved effect are independent.
#[test]
fn a_request_naming_no_unresolved_effect_is_independent() {
    let p = ExposurePolicy::new(vec![rule(Some(1), None, "enforcement_point")]);
    let mut reads = 0;
    assert_eq!(gate(&p, "a", "tools/call", "t", &ClosureKey::default(), true, 1, |_| { reads += 1; view(9, Some(0)) }), Gate::Independent);
    assert_eq!(reads, 0, "a root reads nothing");
    assert_eq!(gate(&p, "a", "tools/call", "t", &PARENT_H, true, 1, |_| view(0, None)), Gate::Independent);
}

#[test]
fn a_rule_selects_by_actor_operation_and_resource() {
    let p = ExposurePolicy::new(vec![ExposureRule { actor: "agent:planner".into(), resource: "tool:post".into(), ..rule(Some(1), None, "enforcement_point") }]);
    let full = |_| view(9, Some(0));
    assert!(matches!(gate(&p, "agent:planner", "tools/call", "tool:post", &PARENT_H, true, 1, full), Gate::Hold(_)));
    assert_eq!(gate(&p, "agent:other", "tools/call", "tool:post", &PARENT_H, true, 1, full), Gate::Untouched);
    assert_eq!(gate(&p, "agent:planner", "tools/call", "tool:read", &PARENT_H, true, 1, full), Gate::Untouched);
}

/// Two rules: a hold read at one scope outranks unknown coverage at another — a threshold read is a fact.
#[test]
fn a_hold_outranks_unknown_coverage_elsewhere() {
    let p = ExposurePolicy::new(vec![rule(Some(1), None, "fleet_tally"), rule(Some(1), None, "enforcement_point")]);
    let g = gate(&p, "a", "tools/call", "t", &PARENT_H, true, 1, |s| match s {
        ExposureScope::FleetTally => Exposure::Unavailable("a peer is stale".into()),
        _ => view(3, Some(0)),
    });
    assert!(matches!(g, Gate::Hold(_)), "{g:?}");
}

/// **One set of names** (#701's review, finding 10): the catalogue's `ae.exposure` reasons are exactly the gate's
/// labels, which are the metric's.
#[test]
fn the_catalogues_reasons_are_the_gates_labels() {
    let entry = crate::rules::RULES.iter().find(|r| r.id == "ae.exposure").expect("ae.exposure is catalogued");
    let mut reasons: Vec<&str> = entry.outcomes.iter().flat_map(|o| o.reasons.iter().copied()).collect();
    let mut labels: Vec<&str> = GATE_LABELS.to_vec();
    reasons.sort_unstable();
    labels.sort_unstable();
    assert_eq!(reasons, labels);
    for g in [Gate::Exempt(""), Gate::Independent, Gate::Clear, Gate::ShadowWouldHold(String::new()), Gate::ShadowWouldNotEstablish(String::new()), Gate::Hold(String::new()), Gate::NotEstablished(String::new())] {
        assert!(GATE_LABELS.contains(&g.label().unwrap()), "{g:?}");
    }
    assert_eq!(Gate::Untouched.label(), None);
}

#[test]
fn counting_is_per_gate_and_untouched_counts_nothing() {
    let c = ExposureCounters::default();
    for g in [Gate::Untouched, Gate::Exempt(""), Gate::Independent, Gate::Clear, Gate::ShadowWouldHold(String::new()), Gate::Hold(String::new()), Gate::NotEstablished(String::new()), Gate::ShadowWouldNotEstablish(String::new())] {
        c.record(&g);
    }
    let s = c.snapshot();
    assert_eq!((s.exempt, s.independent, s.judged, s.would_hold, s.held, s.not_established, s.would_not_establish), (1, 1, 5, 1, 1, 1, 1));
}

#[cfg(all(feature = "gateway", feature = "tls"))]
#[test]
fn the_local_horizon_is_the_history_horizon() {
    assert_eq!(HISTORY_HORIZON_MS, crate::agent::action_evaluator::MAX_FLEET_WINDOW_MS);
}

// ── The fleet tally's coverage and collection ────────────────────────────────

fn summary(as_of: u64, records: &[UnresolvedRecord]) -> Summary {
    Summary { as_of, source: true, records: records.iter().map(SummaryRecord::of).collect(), ..Summary::default() }
}

fn peer(node: &str, s: Option<Summary>) -> NodeSummary {
    NodeSummary { node: node.into(), peer: true, member: true, summary: s.map(Ok) }
}

fn gone(node: &str, member: bool, s: Option<Result<Summary, ()>>) -> NodeSummary {
    NodeSummary { node: node.into(), peer: false, member, summary: s }
}

const NOW: u64 = 1_000_000_000;
const STALE: u64 = 3_000;
const DRIFT: u64 = 500;

#[test]
fn the_tally_sums_what_the_request_extends_across_fresh_peers() {
    let mine = ClosureExposure { unresolved: 1, oldest_at_ms: Some(NOW - 10) };
    let others = [
        peer("b", Some(summary(NOW - 100, &[record(H, None, NOW - 900), record(OTHER, None, 1)]))),
        peer("c", Some(summary(NOW, &[record("4444", Some(RUN), NOW - 50)]))),
    ];
    match sum_fleet("a", mine, &ClosureKey { parent: Some(H), correlation: Some(RUN) }, &others, NOW, STALE, DRIFT) {
        Exposure::View { exposure, scope, publishers } => {
            assert_eq!(exposure, ClosureExposure { unresolved: 3, oldest_at_ms: Some(NOW - 900) }, "H at b and the run at c, not OTHER");
            assert_eq!(scope, ExposureScope::FleetTally);
            assert_eq!(publishers, ["a", "b", "c"]);
        }
        other => panic!("{other:?}"),
    }
}

/// Unknown coverage fails closed at a peer: no summary, a stale one, one from the future, an unreadable or a truncated
/// one. **A peer's `departed` mark is not read** (#701's review, finding 4): a node in view has not left, and the mark
/// would excuse it from the staleness bound. Seen failing first with the stale check removed, and with a stale
/// departed peer counted.
#[test]
fn unknown_coverage_in_the_tally_is_not_established() {
    let mine = ClosureExposure::default();
    let cases: Vec<(&str, NodeSummary)> = vec![
        ("no summary", peer("b", None)),
        ("stale", peer("b", Some(summary(NOW - STALE - 1, &[])))),
        ("stale", peer("b", Some(Summary { departed: true, ..summary(NOW - STALE * 10, &[record(H, None, 5)]) }))),
        ("in the future", peer("b", Some(summary(NOW + DRIFT + 1, &[])))),
        ("cannot read", NodeSummary { node: "b".into(), peer: true, member: true, summary: Some(Err(())) }),
        ("truncated", peer("b", Some(Summary { truncated: true, ..summary(NOW, &[]) }))),
    ];
    for (why, s) in cases {
        match sum_fleet("a", mine, &PARENT_H, &[s], NOW, STALE, DRIFT) {
            Exposure::Unavailable(w) => assert!(w.contains(why), "{why}: {w}"),
            other => panic!("{why}: not established was read as {other:?}"),
        }
    }
}

/// **Out of view** (#701's review, finding 4): a departed member's last summary is counted; a member that did not depart
/// and lists what the request extends is unknown; one listing nothing of it is not waited for; an id no member holds,
/// an unreadable summary and a future-dated one are not read at all — the collector drops them.
#[test]
fn a_node_out_of_view_is_counted_if_it_departed_and_never_read_if_it_is_no_member() {
    let mine = ClosureExposure::default();
    let lists = || summary(NOW - 100, &[record(H, None, 1)]);
    assert!(matches!(sum_fleet("a", mine, &PARENT_H, &[gone("z", true, Some(Ok(lists())))], NOW, STALE, DRIFT), Exposure::Unavailable(w) if w.contains("out of view")));
    let departed = gone("z", true, Some(Ok(Summary { departed: true, ..lists() })));
    assert!(matches!(sum_fleet("a", mine, &PARENT_H, &[departed], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 1));
    let unrelated = gone("z", true, Some(Ok(summary(1, &[record(OTHER, None, 1)]))));
    assert!(matches!(sum_fleet("a", mine, &PARENT_H, &[unrelated], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 0));
    for (why, o) in [
        ("an invented id", gone("invented", false, Some(Ok(Summary { departed: true, ..summary(NOW, &vec![record(H, None, 1); 9]) })))),
        ("unreadable", gone("z", true, Some(Err(())))),
        ("from the future", gone("z", true, Some(Ok(summary(NOW + DRIFT + 1, &[record(H, None, 1)]))))),
    ] {
        assert!(matches!(sum_fleet("a", mine, &PARENT_H, &[o], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 0), "{why}");
    }
}

/// **What the collector removes** (#701's review, finding 4): an out-of-view node's unreadable, future-dated,
/// non-member (when this node can verify) or past-the-horizon summary — never a peer's, never a live member's.
/// Seen failing first with no collector (each `true` below read as unknown coverage for good).
#[test]
fn the_collector_drops_unreadable_forged_and_expired_summaries_and_never_a_peers() {
    let horizon = summary_horizon_ms(STALE, DRIFT);
    let c = |o: &NodeSummary, verify| collectable(o, verify, NOW, STALE, DRIFT);
    assert!(c(&gone("z", true, Some(Err(()))), false), "unreadable");
    assert!(c(&gone("z", true, Some(Ok(summary(NOW + DRIFT + 1, &[])))), false), "from the future");
    assert!(c(&gone("z", false, Some(Ok(summary(NOW, &[])))), true), "no member, and this node can verify");
    assert!(!c(&gone("z", false, Some(Ok(summary(NOW, &[])))), false), "without [tls] nothing is ruled out");
    assert!(c(&gone("z", true, Some(Ok(summary(NOW - horizon - 1, &[record(H, None, 1)])))), true), "past the horizon");
    assert!(!c(&gone("z", true, Some(Ok(summary(NOW - horizon + 1, &[record(H, None, 1)])))), true), "inside it");
    assert!(!c(&NodeSummary { node: "b".into(), peer: true, member: true, summary: Some(Err(())) }, true), "a peer's is never collected");
}

// ── The declaration ──────────────────────────────────────────────────────────

#[test]
fn an_exposure_rule_parses_is_no_allowance_and_is_refused_by_name_when_malformed() {
    use crate::capability_config::NodeCapabilityConfig;
    let ok = "[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"*\"\nexposure = { max_unresolved = 3, max_oldest_age_ms = 60000 }\n";
    let cfg = NodeCapabilityConfig::from_toml_str(ok).expect("an exposure rule parses");
    let r = &cfg.rules[0];
    assert!(r.is_exposure());
    assert_eq!(r.to_exposure_rule().unwrap().unwrap().limit, ExposureLimit::new(Some(3), Some(60_000), "enforcement_point").unwrap());
    assert!(!r.admits_grant_issue_for("tool:anything"), "an exposure rule admits nothing");
    #[cfg(all(feature = "gateway", feature = "tls"))]
    assert!(r.to_rule().unwrap_err().contains("not an allowance"));
    for (bad, says) in [
        ("exposure = { max_unresolved = 3, max_unresolve = 4 }", "max_unresolve"),
        ("exposure = { }", "neither holds nothing"),
        ("exposure = { max_unresolved = 0 }", "the least is 1"),
        ("exposure = { max_unresolved = 1, scope = \"fleet_consensus\" }", "not built"),
        ("exposure = { max_unresolved = 1 }\nrequires_scopes = [\"mesh:write\"]", "no requires_* clause"),
    ] {
        let text = format!("[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"*\"\n{bad}\n");
        let err = NodeCapabilityConfig::from_toml_str(&text).expect_err(bad).to_string();
        assert!(err.contains(says), "{bad}: {err}");
    }
}

/// The unit-file reference's exposure snippet, as written, loads — so the page cannot drift from the loader.
#[test]
fn the_unit_file_references_exposure_snippet_loads_as_written() {
    let page = include_str!("../../docs/reference/unit-file.md");
    let start = page.find("# an exposure rule (adopter-surface Y5)").expect("the snippet is on the page");
    let body = &page[start..];
    let end = body.find("```").expect("the snippet's block ends");
    let cfg = crate::capability_config::NodeCapabilityConfig::from_toml_str(&body[..end]).expect("the snippet loads");
    let r = cfg.rules[0].to_exposure_rule().unwrap().unwrap();
    assert_eq!(r.limit, ExposureLimit::new(Some(3), Some(600_000), "fleet_tally").unwrap());
}

// ── The doors ────────────────────────────────────────────────────────────────

#[cfg(all(feature = "gateway", feature = "tls"))]
mod doors {
    use super::*;
    use crate::agent::action_evaluator::PreflightRefusal;
    use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
    use crate::agent::http::{ae_preflight, Preflight};
    use crate::control::Profile;
    use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
    use std::sync::{Arc, Mutex};

    const POINT: &str = "gateway:mcp/tools/call";

    /// A source the test sets: its unresolved records, or an error.
    struct Fixed(Mutex<Result<Vec<UnresolvedRecord>, String>>);
    impl Fixed {
        fn of(records: Vec<UnresolvedRecord>) -> Arc<Fixed> {
            Arc::new(Fixed(Mutex::new(Ok(records))))
        }
    }
    impl ExposureSource for Fixed {
        fn unresolved(&self) -> Result<Vec<UnresolvedRecord>, String> {
            self.0.lock().unwrap().clone()
        }
    }

    fn evaluator() -> ReferenceEvaluator {
        ReferenceEvaluator::new("rev-y5")
            .with_catalogue("cat-y5", "1")
            .map_action("tools/call", "*")
            .map_action("refund", "*")
            .map_action("skill.invoke", "*")
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", "tool:post"))
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", "tool:limited").at_most(100, 600_000))
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "refund", "*"))
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "skill.invoke", "*"))
    }

    async fn node_with(tag: &str, policy: ExposurePolicy, source: Option<Arc<dyn ExposureSource>>, tune: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
        let p = crate::test_util::alloc_port();
        let mut cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: p, ..GossipConfig::default() };
        tune(&mut cfg);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg));
        let dir = std::env::temp_dir().join(format!("y5-exposure-{tag}-{p}"));
        let _ = std::fs::remove_dir_all(&dir);
        agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
        agent.with_action_evaluator(Arc::new(evaluator()));
        agent.with_exposure_policy(Arc::new(policy));
        if let Some(s) = source {
            agent.with_exposure_source(s);
        }
        agent.start().await.unwrap();
        agent
    }

    async fn node(tag: &str, policy: ExposurePolicy, source: Arc<dyn ExposureSource>) -> Arc<GossipAgent> {
        node_with(tag, policy, Some(source), |_| {}).await
    }

    async fn call_with(a: &Arc<GossipAgent>, resource: &str, meta: serde_json::Value) -> Preflight {
        ae_preflight(&a.task_ctx, None, "tools/call", resource, &serde_json::json!({}), &serde_json::json!({ "_meta": meta }), POINT).await
    }

    async fn call(a: &Arc<GossipAgent>, meta: serde_json::Value) -> Preflight {
        call_with(a, "tool:post", meta).await
    }

    fn size_one(scope: &str) -> ExposurePolicy {
        ExposurePolicy::new(vec![ExposureRule { actor: "*".into(), operation: "*".into(), resource: "*".into(), limit: ExposureLimit::new(Some(1), None, scope).unwrap() }])
    }

    fn held(p: &Preflight) -> bool {
        matches!(p, Preflight::Refuse(PreflightRefusal::Denied(d)) if d.reason.contains("exposure"))
    }

    /// **Exit: a plant shows a compensation admitted while a new dependent call is held.** A closure holding one
    /// unresolved effect, under a size threshold of one, on an enforcing rung: a new call naming that effect as its
    /// parent is held, naming the threshold, and recorded; the compensation the reconciler asks the same evaluator for
    /// is permitted. **The plant:** the gate also run in the shared seam (`preflight_with_history`) — the compensation
    /// is then refused and this test fails, which is what makes the rule structural rather than configured.
    #[tokio::test]
    async fn a_compensation_is_admitted_while_a_new_dependent_call_is_held() {
        let a = node("comp", size_one("enforcement_point"), Fixed::of(vec![record(H, None, 0)])).await;
        a.set_control_profile(Profile::EnforceLocal);
        match call(&a, serde_json::json!({ "parent": H })).await {
            Preflight::Refuse(PreflightRefusal::Denied(d)) => {
                assert!(d.reason.contains("exposure") && d.reason.contains("size threshold is 1"), "{}", d.reason);
                assert!(d.checked.iter().any(|c| c == "exposure"));
            }
            other => panic!("a dependent call past the threshold was not held: {}", describe(&other)),
        }
        let evaluator: Arc<dyn crate::ActionEvaluator> = Arc::clone(a.task_ctx.action_evaluator.get().unwrap());
        let now = a.task_ctx.hlc.decision_now_ms();
        let permit = crate::effects::external::CompensationPermit::decide(&evaluator, crate::PRINCIPAL_ANONYMOUS, a.node_id().clone(), "pay", "refund", H, now);
        assert!(permit.is_ok(), "the compensation was refused while the closure was exposed: {:?}", permit.err());
        assert_eq!(a.exposure_counts().held, 1);
        a.shutdown().await;
    }

    /// **Only dependent work is held** (#701's review, finding 1): an actor with an unresolved closure has an
    /// independent call admitted — a root, or one whose parent and run name no unresolved effect — while one whose
    /// parent or run is in the closure is held. Seen failing first with the classing per actor (every call of a
    /// selected actor read against all its effects): the independent calls were held.
    #[tokio::test]
    async fn independent_work_passes_while_work_extending_the_closure_is_held() {
        let a = node("cone", size_one("enforcement_point"), Fixed::of(vec![record(H, Some(RUN), 0)])).await;
        a.set_control_profile(Profile::EnforceLocal);
        for meta in [serde_json::json!({}), serde_json::json!({ "parent": OTHER }), serde_json::json!({ "correlation": "run-8" })] {
            let p = call(&a, meta.clone()).await;
            assert!(matches!(p, Preflight::Proceed { .. }), "{meta}: independent work was not admitted: {}", describe(&p));
        }
        for meta in [serde_json::json!({ "parent": H }), serde_json::json!({ "correlation": RUN }), serde_json::json!({ "parent": OTHER, "correlation": RUN })] {
            assert!(held(&call(&a, meta.clone()).await), "{meta}: work extending the closure was admitted");
        }
        let c = a.exposure_counts();
        assert_eq!((c.independent, c.held), (3, 3), "{c:?}");
        a.shutdown().await;
    }

    /// **Exit: the shadow profile counts and holds nothing.** The same dependent call under `observe`: it proceeds, is
    /// counted `would_hold`, none `held`, and its decision record says what an enforcing rung would have done. Under
    /// `legacy` the policy is neither read nor counted (#701's review, finding 11). Seen failing first with the shadow
    /// arm holding.
    #[tokio::test]
    async fn the_shadow_profile_counts_and_holds_nothing() {
        let a = node("shadow", size_one("enforcement_point"), Fixed::of(vec![record(H, None, 0); 5])).await;
        a.set_control_profile(Profile::Observe);
        match call(&a, serde_json::json!({ "parent": H })).await {
            Preflight::Proceed { evidence, .. } => assert!(evidence.checked.iter().any(|c| c.contains("shadow") && c.contains("would hold")), "{:?}", evidence.checked),
            other => panic!("the shadow rung held: {}", describe(&other)),
        }
        a.set_control_profile(Profile::Legacy);
        match call(&a, serde_json::json!({ "parent": H })).await {
            Preflight::Proceed { evidence, .. } => assert!(!evidence.checked.iter().any(|c| c.contains("exposure")), "{:?}", evidence.checked),
            other => panic!("legacy held: {}", describe(&other)),
        }
        let c = a.exposure_counts();
        assert_eq!((c.judged, c.would_hold, c.held, c.not_established), (1, 1, 0, 0), "legacy counted: {c:?}");
        a.shutdown().await;
    }

    /// **Counted only when the evaluator permitted** (#701's review, finding 5): a call the policy denies leaves `held`
    /// and `would_hold` at 0, on either rung. Seen failing first with the count taken before the evaluator decided.
    #[tokio::test]
    async fn a_policy_denial_counts_nothing() {
        let a = node("denied", size_one("enforcement_point"), Fixed::of(vec![record(H, None, 0)])).await;
        for profile in [Profile::EnforceLocal, Profile::Observe] {
            a.set_control_profile(profile);
            let p = call_with(&a, "tool:unlisted", serde_json::json!({ "parent": H })).await;
            assert!(matches!(p, Preflight::Refuse(_)) && !held(&p), "{profile:?}: the evaluator, not exposure, refuses: {}", describe(&p));
        }
        assert_eq!(a.exposure_counts(), ExposureCounts::default());
        a.shutdown().await;
    }

    /// Unknown coverage fails closed on an enforcing rung: a source that cannot read its records, and an
    /// `enforcement_point` rule on a node with no source (#701's review, finding 6) — each *not established*.
    #[tokio::test]
    async fn an_unreadable_closure_is_not_established_on_an_enforcing_rung() {
        let src = Arc::new(Fixed(Mutex::new(Err("the ledger is closed".into()))));
        let a = node("unread", size_one("enforcement_point"), src).await;
        a.set_control_profile(Profile::EnforceLocal);
        match call(&a, serde_json::json!({ "parent": H })).await {
            Preflight::Refuse(PreflightRefusal::NotEstablished(d)) => assert!(d.reason.contains("the ledger is closed"), "{}", d.reason),
            other => panic!("{}", describe(&other)),
        }
        a.shutdown().await;
        let b = node_with("nosource", size_one("enforcement_point"), None, |_| {}).await;
        b.set_control_profile(Profile::EnforceLocal);
        match call(&b, serde_json::json!({ "parent": H })).await {
            Preflight::Refuse(PreflightRefusal::NotEstablished(d)) => assert!(d.reason.contains("no exposure source"), "{}", d.reason),
            other => panic!("an enforcement_point rule with no source cleared: {}", describe(&other)),
        }
        b.shutdown().await;
    }

    fn put(a: &Arc<GossipAgent>, node: &str, bytes: Vec<u8>) {
        let _ = mycelium_core::ops::kv_set(&a.task_ctx, Arc::from(summary_key(node).as_str()), bytes::Bytes::from(bytes));
    }

    /// **The fleet scope's node side** (#701's review, finding 6): what `fleet` sums — every other node's summary this
    /// node holds (`others`) — never reads this node's own key and never a removed member's; a departed out-of-view
    /// member's is counted. (`fleet` asks the governors' confidence bound first, which a lone node never meets; that
    /// gate is row K2's and tested there.)
    #[tokio::test]
    async fn the_fleet_scope_reads_others_skips_its_own_key_and_removed_members() {
        let a = node_with("fleet", size_one("fleet_tally"), None, |_| {}).await;
        let now = a.task_ctx.hlc.decision_now_ms();
        let me = a.node_id().to_string();
        let read = |a: &Arc<GossipAgent>| {
            let others: Vec<NodeSummary> = node_others(&a.task_ctx).0.into_iter().map(|(o, _)| o).collect();
            sum_fleet(&a.node_id().to_string(), ClosureExposure::default(), &PARENT_H, &others, now, 3_000, 500)
        };
        assert!(matches!(read(&a), Exposure::View { exposure, .. } if exposure.unresolved == 0), "no source, no summaries: an empty fleet");
        put(&a, &me, serde_json::to_vec(&summary(now, &vec![record(H, None, 1); 7])).unwrap());
        assert!(matches!(read(&a), Exposure::View { exposure, .. } if exposure.unresolved == 0), "its own key is not read");
        let ghost = NodeId::new("127.0.0.1", 9).unwrap();
        put(&a, &ghost.to_string(), serde_json::to_vec(&Summary { departed: true, ..summary(now, &[record(H, None, 1)]) }).unwrap());
        assert!(matches!(read(&a), Exposure::View { exposure, .. } if exposure.unresolved == 1), "a departed member out of view is counted");
        a.task_ctx.removed.remove(&ghost, []);
        assert!(matches!(read(&a), Exposure::View { exposure, .. } if exposure.unresolved == 0), "a removed member is not consulted");
        a.shutdown().await;
    }

    /// `publish_summary` lists the source's unresolved records; a source that cannot list them publishes `truncated`;
    /// `depart` marks the last word departed; the collector drops an out-of-view node's unreadable summary and counts it.
    #[tokio::test]
    async fn publishing_departing_and_collecting_write_what_readers_rely_on() {
        let src = Fixed::of(vec![record(H, Some(RUN), 3)]);
        let a = node_with("publish", size_one("fleet_tally"), Some(Arc::clone(&src) as Arc<dyn ExposureSource>), |_| {}).await;
        let me = a.node_id().to_string();
        let now = a.task_ctx.hlc.decision_now_ms();
        let own = |a: &Arc<GossipAgent>| -> Summary {
            serde_json::from_slice(&mycelium_core::ops::kv_get(&a.task_ctx, &summary_key(&a.node_id().to_string())).unwrap()).unwrap()
        };
        publish_summary(&a.task_ctx, now);
        let s = own(&a);
        assert!(s.source && !s.truncated && !s.departed && s.records == vec![SummaryRecord::of(&record(H, Some(RUN), 3))], "{s:?}");
        *src.0.lock().unwrap() = Err("closed".into());
        publish_summary(&a.task_ctx, now);
        assert!(own(&a).truncated, "a source error is published as truncated");
        assert!(serde_json::from_slice::<Summary>(&summary_bytes(&a.task_ctx, now, true).unwrap()).unwrap().departed);
        depart(&a.task_ctx);
        assert!(own(&a).departed, "the last word is marked departed");
        let ghost = NodeId::new("127.0.0.1", 9).unwrap().to_string();
        put(&a, &ghost, b"not a summary".to_vec());
        assert_eq!(collect_summaries(&a.task_ctx, now), 1);
        assert!(mycelium_core::ops::kv_get(&a.task_ctx, &summary_key(&ghost)).is_none(), "the unreadable summary is gone");
        assert_eq!(a.exposure_counts().collected, 1);
        assert_ne!(me, ghost);
        a.shutdown().await;
    }

    /// A `protected_rpc_kinds` entry named as an exempt operation is refused at start (#701's review, question 9): a
    /// provider checks a protected kind under its own name, so its work would pass every exposure rule unheld.
    #[tokio::test]
    async fn a_protected_kind_named_as_an_exempt_operation_is_refused_at_start() {
        let p = crate::test_util::alloc_port();
        let cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: p, protected_rpc_kinds: vec!["effect.compensate".into()], ..GossipConfig::default() };
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
        let err = agent.start().await.expect_err("refused").to_string();
        assert!(err.contains("protected_rpc_kinds") && err.contains("effect.compensate"), "{err}");
    }

    /// The reconciler is the source: an effect whose destination never answers is unresolved, named by its prepared
    /// record's hash and its run.
    #[tokio::test]
    async fn the_reconciler_reports_its_unresolved_effects_by_their_names() {
        use crate::effects::external::{EffectClass, ExternalDestination, ExternalEffect, IdempotencyKey, ProbeAnswer, ProbeBudget, Reconciler};
        use crate::effects::{Effect, EffectRefusal};
        use crate::DestinationCommit;
        struct Silent;
        impl ExternalDestination for Silent {
            fn identity(&self) -> &str { "silent" }
            fn dedup_scope(&self) -> &str { "silent" }
            fn host(&self) -> Option<&str> { None }
            fn send(&self, _: &IdempotencyKey, _: &Effect) -> Result<DestinationCommit, EffectRefusal> { Err(EffectRefusal::DeliveryUnknown) }
            fn observe(&self, _: &IdempotencyKey) -> ProbeAnswer { ProbeAnswer::Unknown }
        }
        let path = std::env::temp_dir().join(format!("y5-ledger-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.lock", path.display()));
        let rec = Reconciler::open(&path, Arc::new(Silent), crate::EgressPolicy::default()).unwrap();
        let effect = |op: &str| {
            let op = crate::OperationId::new(op);
            ExternalEffect { effect: Effect::new(op.clone(), crate::AttemptId::of(&op, 1), b"x".to_vec()), class: EffectClass::Idempotent { compensation: None }, probe_budget: ProbeBudget { max_probes: 2, max_age_ms: 60_000 }, composition: None }
        };
        let causal = crate::causal::CausalFields { correlation: Some(RUN.into()), parent: None };
        let _ = rec.apply_caused(&effect("op-1"), None, causal).await;
        let _ = rec.apply(&effect("op-2"), None).await;
        let all = rec.unresolved().unwrap();
        assert_eq!(all.len(), 2, "{all:?}");
        let hash = rec.record(&crate::OperationId::new("op-1")).unwrap().unwrap().record_hash;
        assert_eq!(rec.exposure_of(&ClosureKey { parent: Some(&hash), correlation: None }).unwrap().unresolved, 1);
        assert_eq!(rec.exposure_of(&ClosureKey { parent: None, correlation: Some(RUN) }).unwrap().unresolved, 1);
        assert_eq!(rec.exposure_of(&ClosureKey { parent: Some(OTHER), correlation: None }).unwrap().unresolved, 0);
    }

    /// **Every evidence hash an unresolved record has had** (#701's confirmation round, N1): a probe writes a second
    /// execution evidence, and a request naming the first still extends the record's closure. Seen failing first with
    /// `set_evidence_hash` overwriting (the earlier hash named nothing).
    #[tokio::test]
    async fn a_request_naming_an_earlier_evidence_hash_still_extends_the_closure() {
        use crate::effects::external::{EffectClass, ExternalDestination, ExternalEffect, IdempotencyKey, ProbeAnswer, ProbeBudget, Reconciler};
        use crate::effects::{Effect, EffectRefusal};
        use crate::DestinationCommit;
        struct Silent;
        impl ExternalDestination for Silent {
            fn identity(&self) -> &str { "silent" }
            fn dedup_scope(&self) -> &str { "silent" }
            fn host(&self) -> Option<&str> { None }
            fn send(&self, _: &IdempotencyKey, _: &Effect) -> Result<DestinationCommit, EffectRefusal> { Err(EffectRefusal::DeliveryUnknown) }
            fn observe(&self, _: &IdempotencyKey) -> ProbeAnswer { ProbeAnswer::Unknown }
        }
        let dir = std::env::temp_dir().join(format!("y5-evidence-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let journal = EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap();
        let via = NodeId::new("127.0.0.1", 1).unwrap();
        let rec = Reconciler::open(dir.join("ledger.journal"), Arc::new(Silent), crate::EgressPolicy::default()).unwrap().with_evidence(journal, via);
        let op = crate::OperationId::new("op-e");
        let ext = ExternalEffect { effect: Effect::new(op.clone(), crate::AttemptId::of(&op, 1), b"x".to_vec()), class: EffectClass::Idempotent { compensation: None }, probe_budget: ProbeBudget { max_probes: 5, max_age_ms: 600_000 }, composition: None };
        let _ = rec.apply(&ext, None).await;
        let e1 = rec.record(&op).unwrap().unwrap().evidence_hash.expect("the apply wrote evidence");
        let _ = rec.probe(&op).await;
        let e2 = rec.record(&op).unwrap().unwrap().evidence_hash.expect("the probe wrote evidence");
        assert_ne!(e1, e2, "the probe wrote a new execution evidence");
        for e in [&e1, &e2] {
            assert_eq!(rec.exposure_of(&ClosureKey { parent: Some(e), correlation: None }).unwrap().unresolved, 1, "{e}");
        }
    }

    /// **Where the causal fields reach** (#701's confirmation round, N2): at the provider, `mcp.invoke` carries them
    /// in its JSON-RPC `params._meta` and a dependent call is held there; a `skill.invoke` payload is text and an
    /// operator kind's opaque, so neither carries any — and a dependent skill call is held at the `/a2a` door that
    /// dispatches it, which reads the request's own `_meta`.
    #[tokio::test]
    async fn a_dependent_call_is_held_where_its_causal_fields_reach() {
        use crate::agent::provider_enforcement::{causal_meta, ENFORCEMENT_POINT_PROVIDER};
        let tools = serde_json::to_vec(&serde_json::json!({ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "post", "_meta": { "parent": H, "correlation": RUN } } })).unwrap();
        let meta = causal_meta(crate::signal::signal_kind::MCP_INVOKE, &tools);
        assert_eq!((meta.get("parent").and_then(|v| v.as_str()), meta.get("correlation").and_then(|v| v.as_str())), (Some(H), Some(RUN)));
        assert!(causal_meta("skill.invoke", br#"{"_meta":{"parent":"x"}}"#).is_empty(), "a skill's payload is its text");
        assert!(causal_meta("depot.custom", b"opaque").is_empty(), "an operator kind's payload is opaque");

        let a = node("reach", size_one("enforcement_point"), Fixed::of(vec![record(H, None, 0)])).await;
        a.set_control_profile(Profile::EnforceLocal);
        let at_provider = ae_preflight(&a.task_ctx, None, "tools/call", "tool:post", &serde_json::json!({}), &serde_json::json!({ "_meta": meta }), ENFORCEMENT_POINT_PROVIDER).await;
        assert!(held(&at_provider), "a dependent tools/call at the provider: {}", describe(&at_provider));
        let at_a2a = ae_preflight(&a.task_ctx, None, "skill.invoke", "skill:depot/dispatch@x", &serde_json::json!({ "text": "go" }), &serde_json::json!({ "_meta": { "parent": H } }), "gateway:a2a").await; // `ENFORCEMENT_POINT_A2A`, re-exported only under `a2a`
        assert!(held(&at_a2a), "a dependent skill call at /a2a: {}", describe(&at_a2a));
        a.shutdown().await;
    }

    /// **Counted from the final decision** (#701's confirmation round, item 5): a permit the exposure gate was applied
    /// to, which ends refused as not established because the actor's history kept changing, counts nothing. Seen
    /// failing first with the count taken from the evaluator's last permit (`would_hold` 1).
    #[tokio::test]
    async fn a_permit_refused_after_the_history_retries_counts_nothing() {
        let a = node("retries", size_one("enforcement_point"), Fixed::of(vec![record(H, None, 0)])).await;
        a.set_control_profile(Profile::Observe);
        // The history index read, so the evaluator's history limit can permit and the retry loop is reached.
        let ctx = Arc::clone(&a.task_ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !ctx.history_index.get().is_some_and(|i| matches!(i.view("gateway", "probe", crate::HistoryRequest { window_ms: 1, max_entries: 1 }, 1).0, crate::History::View(_))) {
            assert!(std::time::Instant::now() < deadline, "the history index was never read");
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        crate::agent::http::TEST_HISTORY_REFUSES_ADMIT.pin().insert(a.node_id().to_string());
        let p = call_with(&a, "tool:limited", serde_json::json!({ "parent": H })).await;
        crate::agent::http::TEST_HISTORY_REFUSES_ADMIT.pin().remove(&a.node_id().to_string());
        assert!(matches!(&p, Preflight::Refuse(PreflightRefusal::NotEstablished(d)) if d.reason.contains("kept changing")), "{}", describe(&p));
        assert_eq!(a.exposure_counts(), ExposureCounts::default());
        a.shutdown().await;
    }

    fn describe(p: &Preflight) -> String {
        match p {
            Preflight::Inert => "inert".into(),
            Preflight::Proceed { evidence, .. } => format!("proceed ({})", evidence.reason),
            Preflight::Refuse(r) => format!("refused: {r}"),
        }
    }
}
