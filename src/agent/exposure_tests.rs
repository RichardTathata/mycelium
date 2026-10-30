//! Adopter-surface Y5's promises: the rule holds only new dependent work, on two thresholds that fire on their own; a
//! compensation is admitted while a dependent call is held; the shadow rungs count and hold nothing; unknown coverage
//! fails closed; the fleet tally's coverage rules. The pure half runs in every build; the doors' half, under `gateway`
//! and `tls`, drives `ae_preflight` — the function every door runs — on a real node.

use super::*;

const ACTOR: &str = "agent:planner";

fn rule(max_unresolved: Option<u32>, max_age: Option<u64>, scope: &str) -> ExposureRule {
    ExposureRule { actor: "*".into(), operation: "tools/call".into(), resource: "*".into(), limit: ExposureLimit::new(max_unresolved, max_age, scope).unwrap() }
}

fn view(unresolved: u32, oldest_at_ms: Option<u64>) -> Exposure {
    Exposure::View { exposure: ActorExposure { unresolved, oldest_at_ms }, scope: ExposureScope::EnforcementPoint, publishers: vec!["me".into()] }
}

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
    // And the size threshold alone: ten unresolved, all fresh.
    assert!(matches!(judge(&r, &view(10, Some(now)), now), Judgement::Hold(w) if w.contains("size threshold is 10")));
    assert_eq!(judge(&r, &view(9, Some(now)), now), Judgement::Clear);
    // An empty closure is clear whatever the age threshold.
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

/// **The decisive rule at a door:** a rule that selects every operation still never holds the reconciler's own family
/// or a grant — whatever the closure holds.
#[test]
fn the_reconcilers_own_work_and_a_grant_are_never_held() {
    let any = ExposurePolicy::new(vec![ExposureRule { operation: "*".into(), ..rule(Some(1), None, "enforcement_point") }]);
    let counters = ExposureCounters::default();
    for op in ["effect.probe", "effect.compensate", "effect.leave", "grant.issue", "grant.revoke"] {
        let g = gate(&any, ACTOR, op, "destination:pay", true, 1, &counters, |_| view(100, Some(0)));
        assert!(matches!(g, Gate::Exempt(_)), "{op}: {g:?}");
    }
    assert_eq!(counters.snapshot().exempt, 5);
    assert_eq!(counters.snapshot().held, 0);
    assert!(matches!(gate(&any, ACTOR, "tools/call", "tool:post", true, 1, &counters, |_| view(100, Some(0))), Gate::Hold(_)));
}

/// **Exit: the shadow profile counts and holds nothing** — the pure half: not enforcing, a threshold reached proceeds
/// and is counted `would_hold`; unknown coverage proceeds and is counted `would_not_establish`. Nothing is held.
#[test]
fn in_shadow_a_hold_proceeds_and_is_counted() {
    let p = ExposurePolicy::new(vec![rule(Some(2), None, "enforcement_point")]);
    let c = ExposureCounters::default();
    match gate(&p, ACTOR, "tools/call", "tool:post", false, 1, &c, |_| view(5, Some(0))) {
        Gate::Proceed(Some(note)) => assert!(note.starts_with("shadow") && note.contains("size threshold is 2"), "{note}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(gate(&p, ACTOR, "tools/call", "tool:post", false, 1, &c, |_| Exposure::Unavailable("x".into())), Gate::Proceed(Some(_))));
    let s = c.snapshot();
    assert_eq!((s.would_hold, s.would_not_establish, s.held, s.not_established), (1, 1, 0, 0));
}

#[test]
fn a_rule_selects_by_actor_operation_and_resource() {
    let p = ExposurePolicy::new(vec![ExposureRule { actor: ACTOR.into(), resource: "tool:post".into(), ..rule(Some(1), None, "enforcement_point") }]);
    let c = ExposureCounters::default();
    let full = |_| view(9, Some(0));
    assert!(matches!(gate(&p, ACTOR, "tools/call", "tool:post", true, 1, &c, full), Gate::Hold(_)));
    assert_eq!(gate(&p, "agent:other", "tools/call", "tool:post", true, 1, &c, full), Gate::Untouched);
    assert_eq!(gate(&p, ACTOR, "tools/call", "tool:read", true, 1, &c, full), Gate::Untouched);
    assert_eq!(gate(&p, ACTOR, "llm.invoke", "tool:post", true, 1, &c, full), Gate::Untouched);
}

/// Two rules: a hold read at one scope outranks unknown coverage at another — a threshold read is a fact.
#[test]
fn a_hold_outranks_unknown_coverage_elsewhere() {
    let p = ExposurePolicy::new(vec![rule(Some(1), None, "fleet_tally"), rule(Some(1), None, "enforcement_point")]);
    let c = ExposureCounters::default();
    let g = gate(&p, ACTOR, "tools/call", "t", true, 1, &c, |s| match s {
        ExposureScope::FleetTally => Exposure::Unavailable("a peer is stale".into()),
        _ => view(3, Some(0)),
    });
    assert!(matches!(g, Gate::Hold(_)), "{g:?}");
}

// ── The fleet tally's coverage ───────────────────────────────────────────────

fn summary(as_of: u64, actors: &[(&str, u32, u64)]) -> Summary {
    Summary {
        as_of,
        source: true,
        actors: actors.iter().map(|(a, n, t)| (actor_digest(a), ActorExposure { unresolved: *n, oldest_at_ms: Some(*t) })).collect(),
        ..Summary::default()
    }
}

fn peer(node: &str, s: Option<Summary>) -> NodeSummary {
    NodeSummary { node: node.into(), peer: true, summary: s.map(Ok) }
}

const NOW: u64 = 1_000_000;
const STALE: u64 = 3_000;
const DRIFT: u64 = 500;

#[test]
fn the_tally_sums_fresh_peers_and_takes_the_oldest() {
    let mine = ActorExposure { unresolved: 1, oldest_at_ms: Some(NOW - 10) };
    let others = [peer("b", Some(summary(NOW - 100, &[(ACTOR, 2, NOW - 900), ("agent:x", 7, 1)]))), peer("c", Some(summary(NOW, &[])))];
    match sum_fleet("a", mine, ACTOR, &others, NOW, STALE, DRIFT) {
        Exposure::View { exposure, scope, publishers } => {
            assert_eq!(exposure, ActorExposure { unresolved: 3, oldest_at_ms: Some(NOW - 900) });
            assert_eq!(scope, ExposureScope::FleetTally);
            assert_eq!(publishers, ["a", "b", "c"]);
        }
        other => panic!("{other:?}"),
    }
}

/// Unknown coverage fails closed: a peer with no summary, a stale one, one from the future, an unreadable or a
/// truncated one — each is not established, never a zero. Seen failing first with the stale check removed.
#[test]
fn unknown_coverage_in_the_tally_is_not_established() {
    let mine = ActorExposure::default();
    let cases: Vec<(&str, NodeSummary)> = vec![
        ("no summary", peer("b", None)),
        ("stale", peer("b", Some(summary(NOW - STALE - 1, &[])))),
        ("in the future", peer("b", Some(summary(NOW + DRIFT + 1, &[])))),
        ("cannot read", NodeSummary { node: "b".into(), peer: true, summary: Some(Err(())) }),
        ("truncated", peer("b", Some(Summary { truncated: true, ..summary(NOW, &[]) }))),
    ];
    for (why, s) in cases {
        match sum_fleet("a", mine, ACTOR, &[s], NOW, STALE, DRIFT) {
            Exposure::Unavailable(w) => assert!(w.contains(why), "{why}: {w}"),
            other => panic!("{why}: not established was read as {other:?}"),
        }
    }
    // A stale peer that departed is its last word: counted, not waited for.
    let departed = peer("b", Some(Summary { departed: true, ..summary(NOW - STALE * 10, &[(ACTOR, 4, 5)]) }));
    assert!(matches!(sum_fleet("a", mine, ACTOR, &[departed], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 4));
}

/// A node out of view whose last summary lists the actor is unknown coverage (its effects are still unresolved and
/// nothing here can say they resolved) unless it departed; one that lists nothing of the actor is not waited for.
#[test]
fn a_node_out_of_view_holding_the_actor_is_unknown_unless_it_departed() {
    let mine = ActorExposure::default();
    let gone = |s: Summary| NodeSummary { node: "z".into(), peer: false, summary: Some(Ok(s)) };
    assert!(matches!(sum_fleet("a", mine, ACTOR, &[gone(summary(1, &[(ACTOR, 1, 1)]))], NOW, STALE, DRIFT), Exposure::Unavailable(w) if w.contains("out of view")));
    assert!(matches!(sum_fleet("a", mine, ACTOR, &[gone(summary(1, &[("agent:x", 1, 1)]))], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 0));
    let departed = gone(Summary { departed: true, ..summary(1, &[(ACTOR, 2, 1)]) });
    assert!(matches!(sum_fleet("a", mine, ACTOR, &[departed], NOW, STALE, DRIFT), Exposure::View { exposure, .. } if exposure.unresolved == 2));
    assert!(matches!(sum_fleet("a", mine, ACTOR, &[NodeSummary { node: "z".into(), peer: false, summary: None }], NOW, STALE, DRIFT), Exposure::View { .. }));
}

// ── The declaration ──────────────────────────────────────────────────────────

#[test]
fn an_exposure_rule_parses_is_no_allowance_and_is_refused_by_name_when_malformed() {
    use crate::capability_config::NodeCapabilityConfig;
    let ok = r#"
[[rule]]
actor = "*"
operation = "tools/call"
resource = "*"
exposure = { max_unresolved = 3, max_oldest_age_ms = 60000 }
"#;
    let cfg = NodeCapabilityConfig::from_toml_str(ok).expect("an exposure rule parses");
    let r = &cfg.rules[0];
    assert!(r.is_exposure());
    let built = r.to_exposure_rule().unwrap().unwrap();
    assert_eq!(built.limit, ExposureLimit::new(Some(3), Some(60_000), "enforcement_point").unwrap());
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
    use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
    use crate::agent::http::{ae_preflight, Preflight};
    use crate::agent::action_evaluator::PreflightRefusal;
    use crate::control::Profile;
    use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
    use std::sync::{Arc, Mutex};

    const POINT: &str = "gateway:mcp/tools/call";

    /// A source the test sets: one actor's closure.
    #[derive(Default)]
    struct Fixed(Mutex<Option<Result<ActorExposure, String>>>);
    impl ExposureSource for Fixed {
        fn exposure_of(&self, actor: &str) -> Result<ActorExposure, String> {
            match self.0.lock().unwrap().clone() {
                Some(r) if actor == crate::PRINCIPAL_ANONYMOUS => r,
                Some(Err(e)) => Err(e),
                _ => Ok(ActorExposure::default()),
            }
        }
        fn exposures(&self) -> Result<Vec<(String, ActorExposure)>, String> {
            Ok(Vec::new())
        }
    }

    fn evaluator() -> ReferenceEvaluator {
        ReferenceEvaluator::new("rev-y5")
            .with_catalogue("cat-y5", "1")
            .map_action("tools/call", "*")
            .map_action("refund", "*")
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", "tool:post"))
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "refund", "*"))
    }

    async fn node(tag: &str, policy: ExposurePolicy, source: Arc<dyn ExposureSource>) -> Arc<GossipAgent> {
        let p = crate::test_util::alloc_port();
        let cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: p, ..GossipConfig::default() };
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg));
        let dir = std::env::temp_dir().join(format!("y5-exposure-{tag}-{p}"));
        let _ = std::fs::remove_dir_all(&dir);
        agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
        agent.with_action_evaluator(Arc::new(evaluator()));
        agent.with_exposure_policy(Arc::new(policy));
        agent.with_exposure_source(source);
        agent.start().await.unwrap();
        agent
    }

    async fn call(a: &Arc<GossipAgent>) -> Preflight {
        ae_preflight(&a.task_ctx, None, "tools/call", "tool:post", &serde_json::json!({}), &serde_json::json!({}), POINT).await
    }

    fn size_one() -> ExposurePolicy {
        ExposurePolicy::new(vec![ExposureRule { actor: "*".into(), operation: "*".into(), resource: "*".into(), limit: ExposureLimit::new(Some(1), None, "enforcement_point").unwrap() }])
    }

    /// **Exit: a plant shows a compensation admitted while a new dependent call is held.** An actor whose closure
    /// holds one unresolved effect, under a size threshold of one, on an enforcing rung: a new call at the gateway's
    /// door is held, naming the threshold, and recorded; the compensation the reconciler asks the same evaluator for
    /// is permitted. **The plant:** the gate moved into the shared seam (`preflight_with_history`) — the compensation
    /// is then refused and this test fails, which is what makes the rule structural rather than configured.
    #[tokio::test]
    async fn a_compensation_is_admitted_while_a_new_dependent_call_is_held() {
        let src = Arc::new(Fixed::default());
        *src.0.lock().unwrap() = Some(Ok(ActorExposure { unresolved: 1, oldest_at_ms: Some(0) }));
        let a = node("comp", size_one(), Arc::clone(&src) as Arc<dyn ExposureSource>).await;
        a.set_control_profile(Profile::EnforceLocal);
        match call(&a).await {
            Preflight::Refuse(PreflightRefusal::Denied(d)) => {
                assert!(d.reason.contains("exposure") && d.reason.contains("size threshold is 1"), "{}", d.reason);
                assert!(d.checked.iter().any(|c| c == "exposure"));
            }
            other => panic!("a dependent call past the threshold was not held: {}", describe(&other)),
        }
        let evaluator: Arc<dyn crate::ActionEvaluator> = Arc::clone(a.task_ctx.action_evaluator.get().unwrap());
        let now = a.task_ctx.hlc.decision_now_ms();
        let permit = crate::effects::external::CompensationPermit::decide(&evaluator, crate::PRINCIPAL_ANONYMOUS, a.node_id().clone(), "pay", "refund", "parent-hash", now);
        assert!(permit.is_ok(), "the compensation was refused while the closure was exposed: {:?}", permit.err());
        let c = a.exposure_counts();
        assert_eq!((c.held, c.would_hold), (1, 0), "{c:?}");
        // And once the closure resolves, the call proceeds.
        *src.0.lock().unwrap() = Some(Ok(ActorExposure::default()));
        assert!(matches!(call(&a).await, Preflight::Proceed { .. }));
        a.shutdown().await;
    }

    /// **Exit: the shadow profile counts and holds nothing.** The same exposed actor under `legacy` (the default) and
    /// `observe`: every call proceeds, each is counted `would_hold`, none `held`, and the decision record says what an
    /// enforcing rung would have done. Seen failing first with the shadow arm holding.
    #[tokio::test]
    async fn the_shadow_profile_counts_and_holds_nothing() {
        let src = Arc::new(Fixed::default());
        *src.0.lock().unwrap() = Some(Ok(ActorExposure { unresolved: 5, oldest_at_ms: Some(0) }));
        let a = node("shadow", size_one(), src).await;
        for profile in [Profile::Legacy, Profile::Observe] {
            a.set_control_profile(profile);
            match call(&a).await {
                Preflight::Proceed { evidence, .. } => assert!(
                    evidence.checked.iter().any(|c| c.contains("shadow") && c.contains("would hold")),
                    "{profile:?}: {:?}",
                    evidence.checked
                ),
                other => panic!("{profile:?}: the shadow rung held: {}", describe(&other)),
            }
        }
        let c = a.exposure_counts();
        assert_eq!((c.judged, c.would_hold, c.held, c.not_established), (2, 2, 0, 0), "{c:?}");
        a.shutdown().await;
    }

    /// Unknown coverage fails closed on an enforcing rung: a source that cannot read its records, and a policy with no
    /// source attached, each make a selected call *not established* — never a clear closure.
    #[tokio::test]
    async fn an_unreadable_closure_is_not_established_on_an_enforcing_rung() {
        let src = Arc::new(Fixed::default());
        *src.0.lock().unwrap() = Some(Err("the ledger is closed".into()));
        let a = node("unread", size_one(), src).await;
        a.set_control_profile(Profile::EnforceLocal);
        match call(&a).await {
            Preflight::Refuse(PreflightRefusal::NotEstablished(d)) => assert!(d.reason.contains("the ledger is closed"), "{}", d.reason),
            other => panic!("{}", describe(&other)),
        }
        assert_eq!(a.exposure_counts().not_established, 1);
        a.shutdown().await;
    }

    /// The reconciler is the source: an effect applied for an actor whose destination never answers is in the actor's
    /// unresolved closure — prepared, then unknown — and an effect for another actor is not.
    #[tokio::test]
    async fn the_reconciler_reports_an_actors_unresolved_effects() {
        use crate::effects::external::{EffectClass, ExternalDestination, ExternalEffect, ProbeAnswer, ProbeBudget, Reconciler, IdempotencyKey};
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
        let me = crate::mandate::PrincipalId::new(ACTOR).unwrap();
        let other = crate::mandate::PrincipalId::new("agent:other").unwrap();
        let _ = rec.apply_as(&me, &effect("op-1"), None, Default::default()).await;
        let _ = rec.apply_as(&me, &effect("op-2"), None, Default::default()).await;
        let _ = rec.apply_as(&other, &effect("op-3"), None, Default::default()).await;
        let _ = rec.apply(&effect("op-4"), None).await; // no actor named: counted toward none
        let mine = rec.exposure_of(ACTOR).unwrap();
        assert_eq!(mine.unresolved, 2, "{mine:?}");
        assert!(mine.oldest_at_ms.is_some());
        assert_eq!(rec.exposure_of("agent:other").unwrap().unresolved, 1);
        let all = rec.exposures().unwrap();
        assert_eq!(all.len(), 2, "{all:?}");
    }

    fn describe(p: &Preflight) -> String {
        match p {
            Preflight::Inert => "inert".into(),
            Preflight::Proceed { evidence, .. } => format!("proceed ({})", evidence.reason),
            Preflight::Refuse(r) => format!("refused: {r}"),
        }
    }
}
