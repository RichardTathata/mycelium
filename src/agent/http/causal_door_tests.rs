//! Adopter-surface X4, second PR: the doors the registry lists (`scripts/causal-doors.txt`) — a member's direct call,
//! the raw gateway door, `/a2a` and the LLM door — each shown to carry the two fields as its row says.

use crate::agent::action_evaluator::AeEvidence;
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn records(journal: &std::path::Path) -> Vec<AeEvidence> {
    crate::read_evidence_journal(journal).unwrap().iter().map(|b| serde_json::from_slice(b).unwrap()).collect()
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

/// One node: evaluator, strict journal, provider enforcement, a tool and a skill; the resources the policy allows.
async fn node(http_port: u16, allow: &[(&str, &str)]) -> (Arc<GossipAgent>, std::path::PathBuf, Arc<AtomicUsize>) {
    let gossip_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let a = GossipAgent::new(id.clone(), cfg);
    // `/a2a` is mounted only on a node that asks for it.
    #[cfg(feature = "a2a")]
    let a = a.with_a2a();
    let a = Arc::new(a);
    let dir = std::env::temp_dir().join(format!("x4-doors-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("evidence.log");
    a.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    let mut policy = ReferenceEvaluator::new("rev-x4b").with_catalogue("cat-x4b", "1");
    for (op, res) in allow {
        let resource = res.replace("{id}", &id.to_string());
        policy = policy.map_action(*op, "*").allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, *op, resource.as_str()));
        policy = policy.allow(Rule::new(crate::agent::gateway_caller::node_principal(&id).as_str(), *op, resource.as_str()));
    }
    a.with_action_evaluator(Arc::new(policy));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = Arc::clone(&ran);
    let handle = a.mcp().register_mcp_tool("greet", serde_json::json!({"type": "object"}), move |_| {
        let ran = Arc::clone(&ran2);
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("hi"))
        }
    });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(50)).await;
    (a, journal, ran)
}

/// **A member's direct call carries the fields in its own bytes, and the gateway's raw door refuses the kind.**
/// `rpc_call` alters nothing: the provider's check reads `params._meta` from the `mcp.invoke` payload and its record
/// carries both fields; the record's node is the provider. `POST /gateway/rpc/call` refuses `mcp.invoke` as a
/// protected kind (403 `protected_kind`), so no governed work starts through it.
#[tokio::test]
async fn a_members_direct_call_carries_the_fields_in_its_own_bytes_and_the_gateway_s_raw_door_refuses_the_kind() {
    let http_port = alloc_port();
    let (a, journal, ran) = node(http_port, &[("tools/call", "tool:greet@{id}")]).await;
    let parent = crate::causal::content_hash(b"the record this call acts on");
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "greet", "arguments": {}, "_meta": {"correlation": "run-direct", "parent": parent}}}).to_string();
    let reply = a.service().rpc_call(a.node_id().clone(), "mcp.invoke", payload.clone().into_bytes(), Duration::from_secs(5)).await.unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert!(until(|| records(&journal).iter().any(|r| r.enforcement_point == "provider"), Duration::from_secs(5)).await);
    let recs = records(&journal);
    let at_provider: Vec<_> = recs.iter().filter(|r| r.enforcement_point == "provider").collect();
    assert!(!at_provider.is_empty());
    for r in &at_provider {
        assert_eq!(r.correlation.as_deref(), Some("run-direct"));
        assert_eq!(r.parent.as_deref(), Some(parent.as_str()));
        assert_eq!(r.node.as_deref(), Some(a.node_id().to_string().as_str()));
    }
    // The raw gateway door: the kind is protected, nothing starts.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call"))
        .json(&serde_json::json!({"target": a.node_id().to_string(), "method": "mcp.invoke",
            "payload_b64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, payload.as_bytes()), "timeout_secs": 2}))
        .send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "protected_kind", "{body}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the raw door started no work");
    a.shutdown().await;
}

/// **`/a2a` carries the fields on its own record and forwards none:** a skill's payload is its text, so the provider's
/// record of the same call — the serve loop below, under provider enforcement — is a root, by node. Both the legacy
/// `params._meta` and the current generation's `metadata._meta` reach the gateway's record.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn the_a2a_door_carries_the_fields_on_its_own_record_and_forwards_none() {
    let http_port = alloc_port();
    let (a, journal, _ran) = node(http_port, &[("skill.invoke", "skill:depot/dispatch@{id}")]).await;
    let _reg = a.capabilities().advertise_capability(crate::capability::Capability::new("depot", "dispatch"), Duration::from_secs(30));
    {
        let agent = Arc::clone(&a);
        let mut rx = agent.service().rpc_rx("skill.invoke");
        let agent2 = Arc::clone(&agent);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                agent2.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let parent = crate::causal::content_hash(b"the decision this task acts on");
    let send = |params: serde_json::Value, method: &'static str| async move {
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/a2a"))
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
            .send().await.expect("a2a");
        let status = resp.status();
        let text = resp.text().await.unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|e| panic!("{method}: {status} {text:?}: {e}"))
    };
    let body = send(serde_json::json!({"id": "t-legacy", "skillId": "depot/dispatch",
        "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]},
        "_meta": {"correlation": "run-a2a", "parent": parent}}), "tasks/send").await;
    assert!(body.get("error").is_none(), "{body}");
    let body = send(serde_json::json!({"message": {"messageId": "m-1", "role": "user", "parts": [{"kind": "text", "text": "go"}]},
        "metadata": {"skillId": "depot/dispatch", "_meta": {"correlation": "run-a2a-current", "parent": parent}}}), "message/send").await;
    assert!(body.get("error").is_none(), "{body}");
    assert!(until(|| records(&journal).iter().filter(|r| r.enforcement_point == "gateway:a2a").count() >= 2, Duration::from_secs(5)).await);
    let recs = records(&journal);
    let gateway: Vec<_> = recs.iter().filter(|r| r.enforcement_point == "gateway:a2a").collect();
    let ids: Vec<Option<&str>> = gateway.iter().map(|r| r.correlation.as_deref()).collect();
    assert!(ids.contains(&Some("run-a2a")) && ids.contains(&Some("run-a2a-current")), "{ids:?}");
    assert!(gateway.iter().all(|r| r.parent.as_deref() == Some(parent.as_str())));
    let provider: Vec<_> = recs.iter().filter(|r| r.enforcement_point == "provider").collect();
    assert!(!provider.is_empty(), "the provider checked the skill call");
    assert!(provider.iter().all(|r| r.correlation.is_none() && r.parent.is_none()), "a text payload carries nothing: the provider's record is a root");
    a.shutdown().await;
}

/// **The LLM door carries the fields on its own record** from the body's `_meta`; the prompt provider's payload is
/// opaque to its check, so nothing is forwarded.
#[cfg(feature = "llm")]
#[tokio::test]
async fn the_llm_door_carries_the_fields_on_its_own_record() {
    use crate::{EchoBackend, PromptTemplate};
    let http_port = alloc_port();
    let (a, journal, _ran) = node(http_port, &[("llm.invoke", "prompt:demo/greeting@{id}")]).await;
    let skill = PromptTemplate { system: String::new(), user_template: "Greet {{input}}".into(), max_tokens: 16, temperature: 0.0, metadata: Default::default() };
    let _skill = a.llm().register_prompt_skill("demo", "greeting", skill, Arc::new(EchoBackend)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let parent = crate::causal::content_hash(b"the decision this prompt acts on");
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/gateway/llm/call"))
        .json(&serde_json::json!({"ns": "demo", "name": "greeting", "input": "x4", "_meta": {"correlation": "run-llm", "parent": parent}}))
        .send().await.unwrap();
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(until(|| records(&journal).iter().any(|r| r.enforcement_point == "gateway:llm/call"), Duration::from_secs(5)).await);
    let recs = records(&journal);
    let here: Vec<_> = recs.iter().filter(|r| r.enforcement_point == "gateway:llm/call").collect();
    assert!(!here.is_empty());
    assert!(here.iter().all(|r| r.correlation.as_deref() == Some("run-llm") && r.parent.as_deref() == Some(parent.as_str())), "{here:?}");
    a.shutdown().await;
}
