//! Adopter-surface X4's closure over a three-node run (the row's Exit, second bullet).
//!
//! Three nodes: **A** the gateway (evaluator + journal), **B** the provider (evaluator + journal + provider
//! enforcement + a tool), **C** an independent node (evaluator + journal) that answers an unrelated call. A caller
//! reaches B's tool through A's `/mcp` under `_meta.correlation`; A's decision is recorded, and A forwards its
//! decision's hash as the `parent` of the work B does, so B's record cites A's. Then the closure of A's decision, read
//! from the two journals, names B's record and nothing of C's — shown by a plant on each side: the chain cut (B's
//! record not cited) fails the "every dependent" half, and C's record counted would fail the "no independent" half.
//! The same run under a saturated decision sink shows the hole named and counted, the journal-side closure unchanged.
#![cfg(all(feature = "gateway", feature = "tls"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycelium::closure::Corpus;
use mycelium::decision::{DecisionRecord, DecisionSink, SinkConfig};
use mycelium::rule::OutcomeKind;
use mycelium::{EvidenceJournal, EvidenceProfile, GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Node {
    agent: Arc<GossipAgent>,
    journal: std::path::PathBuf,
    dir: std::path::PathBuf,
}

async fn node(name: &str, gossip: u16, http: Option<u16>, peers: &[u16], tool: Option<Arc<AtomicUsize>>, sink: Option<Arc<DecisionSink>>) -> Node {
    let id = NodeId::new("127.0.0.1", gossip).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = http;
    cfg.bootstrap_peers = peers.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    let dir = std::env::temp_dir().join(format!("x4-closure-{name}-{gossip}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("evidence.log");
    agent.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    // Anonymous callers may call any tool anywhere: the policy is not what this test is about.
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-closure").with_catalogue("cat-closure", "1").map_action("tools/call", "*")
            .allow(Rule::new(mycelium::PRINCIPAL_ANONYMOUS, "tools/call", "*")),
    ));
    agent.with_provider_enforcement();
    if let Some(s) = sink {
        agent.with_decision_trace(s);
    }
    agent.start().await.unwrap();
    if let Some(ran) = tool {
        let handle = agent.mcp().register_mcp_tool(format!("tool-{name}"), serde_json::json!({"type": "object"}), move |_| {
            let ran = Arc::clone(&ran);
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!("done"))
            }
        });
        std::mem::forget(handle);
    }
    Node { agent, journal, dir }
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

fn journal_records(path: &std::path::Path) -> Vec<serde_json::Value> {
    mycelium::read_evidence_journal(path).unwrap().iter().map(|b| serde_json::from_slice(b).unwrap()).collect()
}

async fn call(http: u16, tool: &str, meta: serde_json::Value) -> serde_json::Value {
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {}, "_meta": meta}}))
        .send().await.expect("mcp").json().await.unwrap()
}

#[tokio::test]
async fn a_closure_over_three_nodes_names_every_dependent_record_and_no_independent_one() {
    let (ga, gb, gc) = (free_port(), free_port(), free_port());
    let (ha, hc) = (free_port(), free_port());
    let ran_b = Arc::new(AtomicUsize::new(0));
    let ran_c = Arc::new(AtomicUsize::new(0));
    // B's decision sink is saturated at one record, so its own decisions past the first are dropped and counted.
    let sink_b = Arc::new(DecisionSink::new(SinkConfig { max_records: 1, ..SinkConfig::default() }));
    let a = node("a", ga, Some(ha), &[], None, None).await;
    let b = node("b", gb, None, &[ga], Some(Arc::clone(&ran_b)), Some(Arc::clone(&sink_b))).await;
    let c = node("c", gc, Some(hc), &[ga], Some(Arc::clone(&ran_c)), None).await;
    // The mesh forms and A sees B's tool.
    assert!(until(|| a.agent.connected_peers().len() >= 2, Duration::from_secs(20)).await, "three nodes connect");
    assert!(until(|| !a.agent.kv().scan_prefix("tools/tool-b/").is_empty(), Duration::from_secs(20)).await, "B's tool reaches A by gossip");
    // A dispatches to B only once B's caller-context marker (`sys/caller-context/{B}`) has reached it.
    let marker = format!("sys/caller-context/{}", b.agent.node_id());
    assert!(until(|| a.agent.kv().get(&marker).is_some(), Duration::from_secs(20)).await, "B's caller-context marker reaches A");

    // The run: A decides, forwards its decision's hash as B's parent; B decides and runs the tool.
    let body = call(ha, "tool-b", serde_json::json!({"correlation": "run-3"})).await;
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(ran_b.load(Ordering::SeqCst), 1);
    // An independent call on C, under another correlation.
    let body = call(hc, "tool-c", serde_json::json!({"correlation": "run-other"})).await;
    assert!(body.get("error").is_none(), "{body}");
    assert!(until(|| journal_records(&b.journal).iter().any(|r| r["enforcement_point"] == "provider"), Duration::from_secs(5)).await);
    assert!(until(|| journal_records(&c.journal).iter().any(|r| r["enforcement_point"] == "provider"), Duration::from_secs(5)).await);

    // A's decision record and its hash, as the closure's root.
    let a_frames = mycelium::read_evidence_journal(&a.journal).unwrap();
    let a_decided = a_frames.iter().find(|f| {
        let v: serde_json::Value = serde_json::from_slice(f).unwrap();
        v["enforcement_point"] == "gateway:mcp/tools/call" && v["kind"] == "decided"
    }).expect("A's decision");
    let root = mycelium::causal::content_hash(a_decided);
    // B's provider record cites it.
    let b_recs = journal_records(&b.journal);
    let b_decided = b_recs.iter().find(|r| r["enforcement_point"] == "provider" && r["kind"] == "decided").unwrap();
    assert_eq!(b_decided["parent"].as_str(), Some(root.as_str()), "B's record cites A's decision: {b_decided}");
    assert_eq!(b_decided["correlation"], "run-3");

    // The closure over all three journals: every dependent record, no independent one.
    let corpus = Corpus::load(&[a.journal.clone(), b.journal.clone(), c.journal.clone()]).unwrap();
    let members = corpus.closure_members(&root);
    let b_hashes: Vec<String> = mycelium::read_evidence_journal(&b.journal).unwrap().iter()
        .filter(|f| serde_json::from_slice::<serde_json::Value>(f).unwrap()["correlation"] == "run-3")
        .map(|f| mycelium::causal::content_hash(f)).collect();
    assert!(!b_hashes.is_empty());
    for h in &b_hashes {
        assert!(members.contains(h), "every dependent record of B is in the closure (PLANT: cut the forwarded parent and this fails)");
    }
    let c_hashes: Vec<String> = mycelium::read_evidence_journal(&c.journal).unwrap().iter().map(|f| mycelium::causal::content_hash(f)).collect();
    for h in &c_hashes {
        assert!(!members.contains(h), "no independent record of C is in the closure (PLANT: a reader that follows every record fails here)");
    }
    let out = corpus.closure(&root, Some(3));
    assert!(out.contains("3 journal(s) given for a fleet of 3 nodes"), "{out}");
    assert!(out.contains("@127.0.0.1:") && out.contains("point=provider"), "{out}");

    // The saturated sink: B's sink holds one record, so decisions past it are dropped and counted. B's own decision
    // points write too few to fill it in this run, so the **test** saturates it with three admission-shaped records —
    // a plant of saturation, stated as such. A hash standing for a dropped decision (no producer can know a dropped
    // record's hash: the sink assigns the `seq` a kept line carries, so a record it dropped was never written) is
    // then cited as a parent by a further call through A; the closure of that hash is a hole, named with the sink's
    // counts, and the journal-side closure is unchanged.
    let mut dropped_line = String::new();
    for i in 0..3u64 {
        let rec = DecisionRecord::new("signal.admission", 2, format!("frame {i}"), OutcomeKind::Refusal, "shed")
            .node(b.agent.node_id().to_string(), 1)
            .target(format!("signal/{i}"))
            .no_parent()
            .no_effect();
        if sink_b.record(rec.clone()).is_err() {
            dropped_line = rec.to_string_line();
        }
    }
    let stats = sink_b.stats();
    assert!(stats.dropped_full >= 1 && !dropped_line.is_empty(), "B's sink saturated: {stats:?}");
    std::fs::write(b.dir.join("decisions.jsonl"), sink_b.to_jsonl()).unwrap();
    std::fs::write(b.dir.join("decisions.stats.json"), sink_b.stats_json()).unwrap();
    let dropped_hash = mycelium::causal::content_hash(dropped_line.as_bytes());
    let body = call(ha, "tool-b", serde_json::json!({"correlation": "run-3", "parent": dropped_hash})).await;
    assert!(body.get("error").is_none(), "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let corpus2 = Corpus::load(&[a.journal.clone(), b.journal.clone(), c.journal.clone(), b.dir.join("decisions.jsonl")]).unwrap();
    let hole = corpus2.closure(&dropped_hash, Some(3));
    assert!(hole.contains("is in none of the files given"), "{hole}");
    assert!(hole.contains("or a decision the sink dropped (the 1 trace(s) given report"), "{hole}");
    // The gateway's decision and its execution record both carry the caller's parent, so two records cite it.
    assert!(hole.contains("2 record(s) cite it as their parent"), "{hole}");
    let members2 = corpus2.closure_members(&root);
    for h in &b_hashes {
        assert!(members2.contains(h), "the journal-side closure is unchanged by the trace");
    }

    a.agent.shutdown().await;
    b.agent.shutdown().await;
    c.agent.shutdown().await;
    for d in [&a.dir, &b.dir, &c.dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}
