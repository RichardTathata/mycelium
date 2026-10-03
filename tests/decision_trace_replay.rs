//! I6's bundle path, end to end, and its comparison **attempted** (plan I6, 2026-10-03).
//!
//! The attachment half: one live node recorded under the replay kernel with a decision sink; its
//! records go into a bundle as `decisions.jsonl` beside `coverage.json`, the bundle reads back, and
//! the trace explains. The comparison half: the same node replayed from that recording. **That
//! diverges at the kernel** — on this date at choice 20, two periodic loops' timer ticks
//! (`membership/tick`, `health/tick`) swapped order between runs on a current-thread runtime with the
//! clock paused, and the kernel's replay refuses a swapped choice by design. A decision-level
//! comparison of a live node therefore waits on the scheduler seam covering every periodic loop
//! (`docs/operations/what-is-proven.md`); `decision::compare` is exercised on traces the sink
//! produced, not on a replay. The test asserts what holds and names what does not.
#![cfg(feature = "sim")]

use std::sync::Arc;
use std::time::Duration;

use mycelium::decision::{compare, DecisionRecord, DecisionSink};
use mycelium::sim_seam::{install, take, SimContext};
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_sim::bundle::{Bundle, COVERAGE_MANIFEST, DECISION_ATTACHMENT};
use mycelium_sim::{Kernel, Sources, Trace};

async fn one_node(kernel: Kernel, port: u16) -> (Trace, Vec<DecisionRecord>) {
    install(SimContext { kernel, sources: Sources::seeded(11, 1_700_000_000_000), node: "n1".into(), offsets: Default::default() });
    let sink = Arc::new(DecisionSink::default());
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.health_check_interval_secs = 1;
    cfg.health_check_max_jitter_ms = 0;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    a.with_decision_trace(Arc::clone(&sink));
    a.start().await.unwrap();
    // A group this node alone is eligible for, governed to a floor of one: the governor rolls and joins.
    let _cap = a.capabilities().advertise_capability(mycelium::Capability::new("svc", "worker"), Duration::from_secs(30));
    let _grp = a.capabilities().define_capability_group(
        "pool",
        mycelium::CapabilityGroupDef { filter: mycelium::CapFilter::new("svc", "worker"), topology_policy: None, provides: vec![], requires: vec![] },
        Duration::from_secs(30),
    );
    a.start_membership_governor();
    let _ = a.publish_membership_intent(mycelium::MembershipIntent::new("pool", 1, None));
    for _ in 0..200 {
        if sink.snapshot().iter().any(|r| r.reason == "join") { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    a.shutdown().await;
    let ctx = take().expect("the kernel this thread ran under");
    (ctx.kernel.trace().clone(), sink.drain())
}

#[tokio::test]
async fn a_recorded_node_carries_its_decisions_into_the_bundle_and_the_trace_explains() {
    let port = mycelium::test_util::alloc_port();
    let (trace, recorded) = one_node(Kernel::recording(), port).await;
    assert!(recorded.iter().any(|r| r.rule == "membership.governed" && r.reason == "join"), "the recording holds a join: {recorded:?}");

    // The bundle carries the trace, as the node binary writes it.
    let jsonl: String = recorded.iter().map(|r| serde_json::to_string(r).unwrap() + "\n").collect();
    let coverage = mycelium::rule::Catalogue::gather_partial(&[mycelium::rules::RULES]).map(|(c, _)| mycelium::decision::coverage_manifest(&c.rules)).unwrap();
    let dir = std::env::temp_dir().join(format!("mycelium-trace-bundle-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    Bundle::new(trace.clone())
        .with_attachment(DECISION_ATTACHMENT, jsonl.clone().into_bytes())
        .with_attachment(COVERAGE_MANIFEST, coverage.into_bytes())
        .write(&dir)
        .unwrap();
    let back = Bundle::read(&dir).unwrap();
    assert!(back.has_decision_trace(), "absent would mean unavailable; here it is present");
    let text = mycelium::decision::explain(std::str::from_utf8(&back.attachments[DECISION_ATTACHMENT]).unwrap(), None, Some("pool"));
    assert!(text.contains("membership.governed → action join"), "{text}");
    let manifest: serde_json::Value = serde_json::from_slice(&back.attachments[COVERAGE_MANIFEST]).unwrap();
    assert!(manifest["rules"].as_array().unwrap().iter().any(|r| r["id"] == "membership.governed" && r["trace"] == "instrumented"));

    // The comparison, on what the sink produced: a trace compares equal to itself and a changed reason
    // is an attributable divergence only when its inputs were seam-covered — the governor's are
    // gossiped, so it is reported *unattributable*, which is the honest verdict for a decision read
    // off a gossiped view.
    let same = compare(&recorded, &recorded);
    assert!(same.reproduces() && same.divergences.is_empty());
    let mut altered = recorded.clone();
    if let Some(j) = altered.iter_mut().find(|r| r.reason == "join") { j.reason = "hold".into(); }
    let cmp = compare(&recorded, &altered);
    assert_eq!(cmp.divergences.len(), 1);
    assert!(!cmp.divergences[0].attributable && cmp.reproduces(), "a gossiped-input decision's divergence is stated, not counted: {:?}", cmp.divergences);
    let _ = std::fs::remove_dir_all(&dir);
}
