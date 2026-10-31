//! Adopter-surface Y11: **exposure at the take and emit doors**. A tuple take (`take`, `take_by_key`), a blackboard
//! claim and the four emit doors (`signal.emit`, `mailbox.deliver`, `overlay.emit_reliable`, `shard.emit`) run the
//! gateway's decision — the action evaluator, then Y5's exposure gate — when an exposure rule selects their operation,
//! and are untouched otherwise. Each door is driven both ways it can be reached: the Rust verb (`*_in`, this node the
//! actor) and the gateway route (the body's `_meta`).
#![cfg(all(feature = "gateway", feature = "tls"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use mycelium::causal::CausalFields;
use mycelium::control::Profile;
use mycelium::door::{DoorRefusal, DOOR_OPERATIONS};
use mycelium::exposure::{ExposureLimit, ExposurePolicy, ExposureRule, ExposureSource, UnresolvedRecord, EXEMPT_OPERATIONS};
use mycelium::{CapFilter, EvidenceJournal, EvidenceProfile, GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule, ServiceHandle, SignalScope};
use mycelium_blackboard::{BoardConfig, BoardRole, Blackboard, BlackboardError, Predicate};
use mycelium_tuple_space::{TupleConfig, TupleError, TupleRole, TupleSpace};

/// Below the OS ephemeral range and bind-verified, like `mycelium::test_util::alloc_port` (which needs `test-util`, a
/// feature this target's CI step does not enable).
fn alloc_port() -> u16 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = std::process::id().wrapping_mul(2_654_435_761) % 16_000 + 1;
    let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    loop {
        let p = (16_000 + NEXT.fetch_add(1, Ordering::Relaxed) % 16_000) as u16;
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
}

/// The unresolved effect's prepared record: a request naming it as its parent extends the closure.
const H: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// A record that is not unresolved: naming it is independent work.
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixed(Mutex<Vec<UnresolvedRecord>>);
impl ExposureSource for Fixed {
    fn unresolved(&self) -> Result<Vec<UnresolvedRecord>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn evaluator() -> ReferenceEvaluator {
    let mut e = ReferenceEvaluator::new("rev-y11").with_catalogue("cat-y11", "1");
    for op in DOOR_OPERATIONS {
        e = e.map_action(*op, "*").allow(Rule::new("*", *op, "*"));
    }
    e
}

/// A policy whose one rule selects `operation` (`*` for every door) with a size threshold of one.
fn policy(operation: &str) -> ExposurePolicy {
    ExposurePolicy::new(vec![ExposureRule {
        actor: "*".into(),
        operation: operation.into(),
        resource: "*".into(),
        limit: ExposureLimit::new(Some(1), None, "enforcement_point").unwrap(),
    }])
}

struct Node {
    agent: Arc<GossipAgent>,
    ts: Arc<TupleSpace>,
    bb: Arc<Blackboard>,
    base: String,
    journal: std::path::PathBuf,
}

async fn node(tag: &str, policy: Option<ExposurePolicy>) -> Node {
    node_with(tag, policy, evaluator()).await
}

async fn node_with(tag: &str, policy: Option<ExposurePolicy>, evaluator: ReferenceEvaluator) -> Node {
    let (gossip, http) = (alloc_port(), alloc_port());
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(http);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip).unwrap(), cfg));
    let dir = std::env::temp_dir().join(format!("y11-doors-{tag}-{gossip}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("evidence.log");
    agent.with_evidence_journal(EvidenceJournal::open(&journal, EvidenceProfile::Strict).unwrap());
    agent.with_action_evaluator(Arc::new(evaluator));
    if let Some(p) = policy {
        agent.with_exposure_policy(Arc::new(p));
    }
    let source = UnresolvedRecord { record_hash: H.into(), evidence_hashes: vec![], correlation: Some("run-y11".into()), prepared_at_ms: 0 };
    agent.with_exposure_source(Arc::new(Fixed(Mutex::new(vec![source]))));
    let ts = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig { namespace: Arc::from("work"), role: TupleRole::Primary, persist: false, ..Default::default() },
    )
    .await
    .unwrap();
    let bb = Blackboard::new(
        Arc::clone(&agent),
        BoardConfig { namespace: Arc::from("board"), role: BoardRole::Primary, persist: false, ..Default::default() },
    )
    .await
    .unwrap();
    agent.with_http_routes(Arc::clone(&ts).http_router());
    agent.with_http_routes(Arc::clone(&bb).http_router());
    agent.start().await.unwrap();
    Node { agent, ts, bb, base: format!("http://127.0.0.1:{http}"), journal }
}

impl Node {
    fn service(&self) -> ServiceHandle {
        self.agent.service()
    }

    async fn fill(&self) {
        for _ in 0..2 {
            self.ts.put("in", Bytes::from_static(b"x")).await.unwrap();
            self.ts.put_keyed("in", "k", Bytes::from_static(b"x")).await.unwrap();
            self.bb.post(BTreeMap::from([("kind".to_string(), "job".to_string())]), Bytes::from_static(b"x")).await.unwrap();
        }
    }

    /// Every door through its Rust verb, on `causal`: `Some(refusal)` when the door refused, `None` when it acted.
    async fn rust_doors(&self, causal: &CausalFields) -> Vec<(&'static str, Option<DoorRefusal>)> {
        let ghost = NodeId::new("127.0.0.1", 9).unwrap();
        let mut out = Vec::new();
        out.push(("signal.emit", self.agent.emit_in("k", SignalScope::Cluster, Bytes::new(), causal).await.err()));
        out.push(("mailbox.deliver", self.service().deliver_event_in(&ghost, "k", Bytes::new(), causal).await.err()));
        out.push((
            "overlay.emit_reliable",
            self.service().emit_reliable_in(ghost.clone(), "k", Bytes::new(), Duration::from_millis(50), causal).await.err(),
        ));
        out.push((
            "shard.emit",
            match self.service().emit_sharded_in("k", "key", &CapFilter::new("no", "such"), Bytes::new(), causal).await {
                Err(mycelium::ShardError::Refused(r)) => Some(r),
                _ => None,
            },
        ));
        out.push((
            "tuple.take",
            match self.ts.take_in("in", Duration::from_secs(1), causal).await {
                Err(TupleError::Refused(r)) => Some(r),
                _ => None,
            },
        ));
        out.push((
            "tuple.take_by_key",
            match self.ts.take_by_key_in("in", "k", Duration::from_secs(1), causal).await {
                Err(TupleError::Refused(r)) => Some(r),
                _ => None,
            },
        ));
        out.push((
            "bb.claim",
            match self.bb.claim_in(&Predicate::new().eq("kind", "job"), causal).await {
                Err(BlackboardError::Refused(r)) => Some(r),
                _ => None,
            },
        ));
        out
    }

    /// Every door through its gateway route, with `meta` as the body's `_meta`: the status each answered.
    async fn gateway_doors(&self, meta: serde_json::Value) -> Vec<(&'static str, u16)> {
        let ghost = NodeId::new("127.0.0.1", 9).unwrap().to_string();
        let routes: [(&str, &str, serde_json::Value); 7] = [
            ("signal.emit", "/gateway/signal/emit", serde_json::json!({ "kind": "k" })),
            ("mailbox.deliver", "/gateway/mailbox/deliver", serde_json::json!({ "kind": "k", "target": ghost })),
            ("overlay.emit_reliable", "/gateway/overlay/emit_reliable", serde_json::json!({ "kind": "k", "target": ghost, "timeout_secs": 1 })),
            ("shard.emit", "/gateway/shard/emit", serde_json::json!({ "kind": "k", "ns": "no", "name": "such", "shard_key": "key" })),
            ("tuple.take", "/gateway/tuple/take", serde_json::json!({ "stage": "in", "timeout_secs": 1 })),
            ("tuple.take_by_key", "/gateway/tuple/take_by_key", serde_json::json!({ "stage": "in", "key": "k", "timeout_secs": 1 })),
            ("bb.claim", "/gateway/bb/claim", serde_json::json!({ "eq": { "kind": "job" } })),
        ];
        let client = reqwest::Client::new();
        let mut out = Vec::new();
        for (op, path, mut body) in routes {
            if !meta.is_null() {
                body["_meta"] = meta.clone();
            }
            let r = client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
            out.push((op, r.status().as_u16()));
        }
        out
    }

    fn records(&self) -> Vec<serde_json::Value> {
        mycelium::read_evidence_journal(&self.journal)
            .unwrap()
            .iter()
            .filter_map(|b| serde_json::from_slice(b).ok())
            .collect()
    }
}

/// A rule with this actor, operation and resource selector, a size threshold of one.
fn rule(actor: &str, operation: &str, resource: &str) -> ExposurePolicy {
    ExposurePolicy::new(vec![ExposureRule {
        actor: actor.into(),
        operation: operation.into(),
        resource: resource.into(),
        limit: ExposureLimit::new(Some(1), None, "enforcement_point").unwrap(),
    }])
}

/// A take request as the tuple space's RPC encodes it: the stage, length-prefixed, then the park in milliseconds.
fn take_req(stage: &str) -> Vec<u8> {
    let mut b = (stage.len() as u16).to_le_bytes().to_vec();
    b.extend_from_slice(stage.as_bytes());
    b.extend_from_slice(&1000u64.to_le_bytes());
    b
}

fn dependent() -> CausalFields {
    CausalFields { parent: Some(H.into()), correlation: None }
}

fn independent() -> CausalFields {
    CausalFields { parent: Some(OTHER.into()), correlation: Some("run-other".into()) }
}

/// **Exit: a take and an emit extending an unresolved closure are each held on an enforcing rung, independent ones
/// admitted.** Every door, by its Rust verb and by its gateway route: work naming the unresolved effect is refused —
/// a denial naming the threshold, nothing taken, claimed or emitted — and independent work passes. Seen failing first
/// with the doors untouched whatever a rule selects (`door::decide` returning before the decision): nothing was held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_door_holds_work_extending_an_unresolved_closure_and_admits_independent_work() {
    let n = node("enforce", Some(policy("*"))).await;
    n.agent.set_control_profile(Profile::EnforceLocal);
    n.fill().await;
    let queued = |n: &Node| {
        let ts = Arc::clone(&n.ts);
        async move { ts.depth(Some("in")).await.unwrap().iter().map(|d| d.depth).sum::<u32>() }
    };
    let before = queued(&n).await;
    for (op, refused) in n.rust_doors(&dependent()).await {
        let r = refused.unwrap_or_else(|| panic!("{op}: work extending the closure was admitted"));
        assert_eq!(r.reason, "action_denied", "{op}: {r}");
        assert!(r.message.contains("exposure") && r.message.contains("size threshold is 1"), "{op}: {r}");
    }
    assert_eq!(queued(&n).await, before, "a held take took nothing");
    // The ordered and the awaiting emit are the same door (#708's review, finding 2).
    for (verb, r) in [
        ("emit_ordered_in", n.agent.emit_ordered_in("k", SignalScope::Cluster, Bytes::new(), &dependent()).await),
        ("emit_async_in", n.agent.emit_async_in("k", SignalScope::Cluster, Bytes::new(), &dependent()).await),
    ] {
        assert!(r.is_err(), "{verb}: work extending the closure was admitted");
    }
    for (op, refused) in n.rust_doors(&independent()).await {
        assert!(refused.is_none(), "{op}: independent work was refused: {refused:?}");
    }
    for (op, status) in n.gateway_doors(serde_json::json!({ "parent": H })).await {
        assert_eq!(status, 403, "{op}: the gateway route admitted work extending the closure");
    }
    for (op, status) in n.gateway_doors(serde_json::json!({ "parent": OTHER })).await {
        assert_ne!(status, 403, "{op}: the gateway route refused independent work");
    }
    let c = n.agent.exposure_counts();
    assert_eq!((c.held, c.independent), (16, 14), "{c:?}");
    n.agent.shutdown().await;
}

/// **Exit: both counted in shadow.** Under `observe` every door admits work extending the closure and counts it
/// `would_hold`, holding none. Seen failing first with the shadow rung holding (the profile read as enforcing).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_shadow_every_door_admits_and_counts_what_it_would_hold() {
    let n = node("shadow", Some(policy("*"))).await;
    n.agent.set_control_profile(Profile::Observe);
    n.fill().await;
    for (op, refused) in n.rust_doors(&dependent()).await {
        assert!(refused.is_none(), "{op}: the shadow rung held: {refused:?}");
    }
    // Through the gateway routes too (#708's review, finding 7).
    for (op, status) in n.gateway_doors(serde_json::json!({ "parent": H })).await {
        assert_ne!(status, 403, "{op}: the shadow rung held at the route");
    }
    let c = n.agent.exposure_counts();
    assert_eq!((c.would_hold, c.held), (14, 0), "{c:?}");
    n.agent.shutdown().await;
}

/// **The shadow rung holds nothing, whatever the evaluator says** (#708's review, finding 3): with an evaluator that
/// permits none of the door operations, a selected door under `observe` admits — root work and dependent work alike,
/// by verb and by route — and its record carries the evaluator's refusal and that it was admitted; on an enforcing
/// rung the same evaluator refuses. Seen failing first with the door not admitting in shadow (`admit_in_shadow` never
/// called): the root emit answered 403 `authority_not_established`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_shadow_a_door_admits_what_a_default_deny_evaluator_refuses() {
    let deny_all = ReferenceEvaluator::new("rev-deny").with_catalogue("cat-y11", "1").map_action("tools/call", "*");
    let n = node_with("shadow-deny", Some(policy("*")), deny_all).await;
    n.agent.set_control_profile(Profile::Observe);
    n.fill().await;
    for causal in [CausalFields::default(), dependent()] {
        for (op, refused) in n.rust_doors(&causal).await {
            assert!(refused.is_none(), "{op}: the shadow rung refused: {refused:?}");
        }
    }
    for (op, status) in n.gateway_doors(serde_json::Value::Null).await {
        assert_ne!(status, 403, "{op}: the shadow rung refused at the route");
    }
    let records = n.records();
    let shadowed = records
        .iter()
        .find(|r| r["enforcement_point"] == "node:signal/emit" && r["decision"] != "permit")
        .unwrap_or_else(|| panic!("no refusal recorded at node:signal/emit: {records:?}"));
    assert!(
        shadowed["checked"].as_array().unwrap().iter().any(|c| c.as_str().unwrap().contains("door admits under the observe profile")),
        "{shadowed}"
    );
    assert_eq!(shadowed["execution"], "attempted", "{shadowed}");
    n.agent.set_control_profile(Profile::EnforceLocal);
    let r = n.agent.emit_in("k", SignalScope::Cluster, Bytes::new(), &CausalFields::default()).await;
    assert!(r.is_err(), "the enforcing rung admitted what the evaluator refused");
    n.agent.shutdown().await;
}

/// **A raw call of a door's kind runs that door's decision** (#708's review, finding 1): `rpc/call`, `scatter` and a
/// reliable emit of `tuple.work.take` or `blackboard.board.claim`, with a rule selecting only the take and claim
/// operations, hold dependent work as the take and claim doors do and pass independent work; a raw call of any other
/// kind is untouched. Seen failing first with `door::decide_raw` returning no door: the dependent raw take answered
/// 200 and took the item.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_raw_call_of_a_doors_kind_runs_that_doors_decision() {
    use base64::Engine as _;
    let p = ExposurePolicy::new(
        ["tuple.take", "bb.claim"]
            .iter()
            .map(|op| ExposureRule { actor: "*".into(), operation: (*op).into(), resource: "*".into(), limit: ExposureLimit::new(Some(1), None, "enforcement_point").unwrap() })
            .collect(),
    );
    let n = node("raw", Some(p)).await;
    n.agent.set_control_profile(Profile::EnforceLocal);
    n.fill().await;
    let me = n.agent.node_id().to_string();
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let client = reqwest::Client::new();
    let raw = |path: &str, kind: &str, payload: Vec<u8>, meta: serde_json::Value| {
        let mut body = match path {
            "/gateway/scatter" => serde_json::json!({ "targets": [me.clone()], "method": kind, "payload_b64": b64(&payload), "timeout_secs": 2 }),
            "/gateway/overlay/emit_reliable" => serde_json::json!({ "target": me.clone(), "kind": kind, "payload_b64": b64(&payload), "timeout_secs": 2 }),
            _ => serde_json::json!({ "target": me.clone(), "method": kind, "payload_b64": b64(&payload), "timeout_secs": 2 }),
        };
        body["_meta"] = meta;
        client.post(format!("{}{path}", n.base)).json(&body).send()
    };
    for path in ["/gateway/rpc/call", "/gateway/scatter", "/gateway/overlay/emit_reliable"] {
        for kind in ["tuple.work.take", "blackboard.board.claim"] {
            let r = raw(path, kind, take_req("in"), serde_json::json!({ "parent": H })).await.unwrap();
            assert_eq!(r.status().as_u16(), 403, "{path} {kind}: a raw dependent call of a door's kind was admitted");
        }
        let r = raw(path, "tuple.work.take", take_req("in"), serde_json::json!({ "parent": OTHER })).await.unwrap();
        assert_ne!(r.status().as_u16(), 403, "{path}: a raw independent take was refused");
        let r = raw(path, "some.other.kind", Vec::new(), serde_json::json!({ "parent": H })).await.unwrap();
        assert_ne!(r.status().as_u16(), 403, "{path}: a raw call of a kind no door serves was decided");
    }
    assert_eq!(n.agent.exposure_counts().held, 6);
    n.agent.shutdown().await;
}

/// **A rule selects by actor and by resource** (#708's review, finding 7). A rule naming the anonymous caller as its
/// actor holds a route's dependent emit and not this node's own verb, whose actor is `node:{id}`; a rule naming one
/// stage, `tuple:work/in`, holds a take there and not one on another stage, nor an emit. Seen failing first with the
/// selection reading the operation alone: the node's verb and the other stage were held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rule_selects_a_door_by_actor_and_by_resource() {
    let n = node("actor", Some(rule(mycelium::PRINCIPAL_ANONYMOUS, "signal.emit", "*"))).await;
    n.agent.set_control_profile(Profile::EnforceLocal);
    let route = reqwest::Client::new()
        .post(format!("{}/gateway/signal/emit", n.base))
        .json(&serde_json::json!({ "kind": "k", "_meta": { "parent": H } }))
        .send()
        .await
        .unwrap();
    assert_eq!(route.status().as_u16(), 403, "the anonymous caller's dependent emit was admitted");
    assert!(n.agent.emit_in("k", SignalScope::Cluster, Bytes::new(), &dependent()).await.is_ok(), "the node's own verb was held by a rule on another actor");
    assert!(!n.records().iter().any(|r| r["enforcement_point"] == "node:signal/emit"), "the unselected verb was decided: {:?}", n.records());
    n.agent.shutdown().await;

    let n = node("resource", Some(rule("*", "*", "tuple:work/in"))).await;
    n.agent.set_control_profile(Profile::EnforceLocal);
    n.ts.put("other", Bytes::from_static(b"x")).await.unwrap();
    assert!(matches!(n.ts.take_in("in", Duration::from_secs(1), &dependent()).await, Err(TupleError::Refused(_))), "the named stage's take was admitted");
    assert!(n.ts.take_in("other", Duration::from_secs(1), &dependent()).await.is_ok(), "another stage's take was held");
    assert!(n.agent.emit_in("k", SignalScope::Cluster, Bytes::new(), &dependent()).await.is_ok(), "an emit was held by a rule on a stage");
    let records = n.records();
    assert!(records.iter().any(|r| r["resource"] == "tuple:work/in"), "the selected take was not decided: {records:?}");
    assert!(!records.iter().any(|r| r["resource"] == "tuple:work/other" || r["resource"] == "signal:k"), "an unselected door was decided: {records:?}");
    n.agent.shutdown().await;
}

/// **Opt-in, by the rule.** A door no exposure rule selects is untouched: it acts on work extending the closure,
/// decides nothing, counts nothing and writes no record — with a rule selecting another operation, with none, and
/// under `legacy` with one that selects it. Seen failing first with the selection skipped (every door decided once an
/// evaluator is attached): the dependent work was held and the journal filled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_door_no_rule_selects_is_untouched() {
    for (tag, p, profile) in [
        ("other-op", Some(policy("tools/call")), Profile::EnforceLocal),
        ("none", None, Profile::EnforceLocal),
        ("legacy", Some(policy("*")), Profile::Legacy),
    ] {
        let n = node(tag, p).await;
        n.agent.set_control_profile(profile);
        n.fill().await;
        for (op, refused) in n.rust_doors(&dependent()).await {
            assert!(refused.is_none(), "{tag}/{op}: an unselected door refused: {refused:?}");
        }
        for (op, status) in n.gateway_doors(serde_json::json!({ "parent": H })).await {
            assert_ne!(status, 403, "{tag}/{op}: an unselected route refused");
        }
        // A malformed `_meta` on an untouched door is not read, as before.
        for (op, status) in n.gateway_doors(serde_json::json!({ "parent": "zz" })).await {
            assert_ne!(status, 400, "{tag}/{op}: an untouched route read its `_meta`");
        }
        assert_eq!(n.agent.exposure_counts(), Default::default(), "{tag}");
        assert!(n.records().is_empty(), "{tag}: an untouched door wrote a record: {:?}", n.records());
        n.agent.shutdown().await;
    }
}

/// **Door-only lineage.** A selected door's decision record carries the causal fields the request carried, under the
/// door's own enforcement point; a malformed `_meta` on a selected route is refused 400, never read as a root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selected_door_records_the_causal_fields_under_its_own_point() {
    let n = node("lineage", Some(policy("*"))).await;
    n.agent.set_control_profile(Profile::EnforceLocal);
    n.agent.emit_in("k", SignalScope::Cluster, Bytes::new(), &independent()).await.unwrap();
    let records = n.records();
    let decision = records
        .iter()
        .find(|r| r["enforcement_point"] == "node:signal/emit" && r["decision"] == "permit")
        .unwrap_or_else(|| panic!("no permit recorded at node:signal/emit: {records:?}"));
    assert_eq!((decision["correlation"].as_str(), decision["parent"].as_str()), (Some("run-other"), Some(OTHER)), "{decision}");
    assert!(records.iter().any(|r| r["enforcement_point"] == "node:signal/emit" && r["execution"] == "completed"), "{records:?}");
    for (op, status) in n.gateway_doors(serde_json::json!({ "parent": "zz" })).await {
        assert_eq!(status, 400, "{op}: a selected route did not refuse a malformed parent");
    }
    n.agent.shutdown().await;
}

/// **The collision refusal at the companions' doors** (Y5's `EXEMPT_OPERATIONS`): no door is named as an exempt
/// operation, and a door asking under one is refused by name — with or without a policy — since it would pass every
/// rule unheld. Seen failing first without the check: the exempt name was decided and admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_door_asking_under_an_exempt_operation_is_refused_by_name() {
    for op in DOOR_OPERATIONS {
        assert!(!EXEMPT_OPERATIONS.contains(op), "{op} is an exempt name");
    }
    for p in [None, Some(policy("*"))] {
        let n = node("exempt", p).await;
        for op in EXEMPT_OPERATIONS {
            let r = n.agent.exposure_door_as_node(op, "tuple:work/in", &dependent(), "tuple/take").await.err().expect("refused");
            assert_eq!(r.reason, DoorRefusal::EXEMPT_NAME, "{op}: {r}");
        }
        n.agent.shutdown().await;
    }
}
