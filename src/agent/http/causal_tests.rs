//! Adopter-surface X4 at the live doors: the two causal fields travel in the application payload from `/mcp` through
//! the gateway's record to the provider's, a malformed one is refused before any policy question, and a field carried
//! as a header is counted and never read.

use crate::agent::action_evaluator::AeEvidence;
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// One node that is both the gateway and the provider: an evaluator permitting anonymous `tools/call` on `tool:greet`,
/// a strict journal, provider enforcement on. The gateway's door and the provider's check each write a record.
async fn node(http_port: u16) -> (Arc<GossipAgent>, std::path::PathBuf, Arc<AtomicUsize>) {
    let gossip_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let dir = std::env::temp_dir().join(format!("x4-causal-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("evidence.log");
    a.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    let policy = ReferenceEvaluator::new("rev-x4")
        .with_catalogue("cat-x4", "1")
        .map_action("tools/call", "*")
        .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", format!("tool:greet@{id}")));
    a.with_action_evaluator(Arc::new(policy));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = Arc::clone(&ran);
    let handle = a.mcp().register_mcp_tool("greet", serde_json::json!({"type": "object"}), move |args| {
        let ran = Arc::clone(&ran2);
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!(format!("hello, {}", args["name"].as_str().unwrap_or("?"))))
        }
    });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(50)).await;
    (a, journal, ran)
}

async fn call(http_port: u16, params: serde_json::Value, headers: &[(&str, &str)]) -> serde_json::Value {
    let mut req = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.expect("tools/call").json().await.unwrap()
}

fn records(journal: &std::path::Path) -> Vec<AeEvidence> {
    crate::read_evidence_journal(journal).unwrap().iter().map(|b| serde_json::from_slice(b).unwrap()).collect()
}

/// **The fields travel in the payload to both records.** A `tools/call` with `_meta.correlation` and `_meta.parent`
/// is decided at the gateway and re-checked at the provider; each point's decision and execution record carries both,
/// read from the request's own payload — the gateway forwards the two keys inside the application bytes the caller
/// envelope's signature digests. A `x-mycelium-parent` header on the same call is counted and never read: no record
/// carries its value.
#[tokio::test]
async fn the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records() {
    let http_port = alloc_port();
    let (a, journal, ran) = node(http_port).await;
    let parent = crate::causal::content_hash(b"the decision this call acts on");
    let misplaced = crate::causal::content_hash(b"a parent in a header");
    let body = call(
        http_port,
        serde_json::json!({"name": "greet", "arguments": {"name": "x4"}, "_meta": {"correlation": "run-x4", "parent": parent}}),
        &[("x-mycelium-parent", misplaced.as_str()), ("x-mycelium-correlation", "run-header")],
    )
    .await;
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let recs = records(&journal);
    let at = |point: &str| recs.iter().filter(|r| r.enforcement_point == point).collect::<Vec<_>>();
    // The gateway writes the decision and the execution record; the provider's check writes its decision (it does not
    // watch the tool run, so it writes no execution record — today's shape, not this row's).
    for (point, least) in [("gateway:mcp/tools/call", 2), ("provider", 1)] {
        let here = at(point);
        assert!(here.len() >= least, "{point}: at least {least} record(s), got {}", here.len());
        for r in &here {
            assert_eq!(r.correlation.as_deref(), Some("run-x4"), "{point} {:?}", r.kind);
            assert_eq!(r.parent.as_deref(), Some(parent.as_str()), "{point} {:?}", r.kind);
        }
    }
    assert!(recs.iter().all(|r| r.parent.as_deref() != Some(misplaced.as_str()) && r.correlation.as_deref() != Some("run-header")),
        "a field carried as a header was read");
    assert_eq!(a.task_ctx.causal_misplaced.load(Ordering::Relaxed), 1, "the header is counted against the door");
    // The same call with nothing under `_meta`: both records are roots, no field invented.
    let body = call(http_port, serde_json::json!({"name": "greet", "arguments": {}}), &[]).await;
    assert!(body.get("error").is_none(), "{body}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let recs = records(&journal);
    let last = recs.last().unwrap();
    assert!(last.correlation.is_none() && last.parent.is_none(), "{last:?}");
    a.shutdown().await;
}

/// **A malformed field is refused at the door, before policy and before the tool.** A `correlation` holding a `/`
/// and a `parent` that is not a hash each answer JSON-RPC `-32602` with reason `request_malformed`; the tool never
/// runs and the journal gains no record — a silently emptied parent would read as a root.
#[tokio::test]
async fn a_malformed_causal_field_is_refused_before_policy_and_never_corrected() {
    let http_port = alloc_port();
    let (a, journal, ran) = node(http_port).await;
    for meta in [
        serde_json::json!({"correlation": "run/7"}),
        serde_json::json!({"correlation": ""}),
        serde_json::json!({"parent": "not-a-hash"}),
        serde_json::json!({"parent": 7}),
    ] {
        let body = call(http_port, serde_json::json!({"name": "greet", "arguments": {}, "_meta": meta}), &[]).await;
        assert_eq!(body["error"]["code"], -32602, "{body}");
        assert_eq!(body["error"]["data"]["reason"], "request_malformed", "{body}");
        assert!(body["error"]["message"].as_str().unwrap().contains("_meta."), "{body}");
    }
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool ran on a malformed request");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(records(&journal).is_empty(), "a refusal before policy writes no decision");
    a.shutdown().await;
}
