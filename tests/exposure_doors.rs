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
    agent.with_action_evaluator(Arc::new(evaluator()));
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
    assert_eq!((c.held, c.independent), (14, 14), "{c:?}");
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
    let c = n.agent.exposure_counts();
    assert_eq!((c.would_hold, c.held), (7, 0), "{c:?}");
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
