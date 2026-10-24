//! Adopter-surface X3 at the live doors: a grant-required tool is refused on silence and on a plain transition; an
//! issued grant lets the call run, is bound to its arguments at the gateway and the provider, and is on the record;
//! issuing is an action the evaluator decides; the namespace is route-owned.

use crate::agent::action_evaluator::AeEvidence;
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::agent::gateway_authority::ExecutionAuthority;
use crate::agent::state_machine::{AgentPolicy, ExecutionState, PolicyViolation};
use crate::config::{GatewayNamedToken, TlsConfig};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use crate::mandate::grant::{EntitlementTable, GrantVerifier};
use crate::mandate::{PrincipalId, ResourceAuthority};
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// A named token's principal is `token:{issuer}/{name}`; the node names its issuer `gw` below.
const SUPERVISOR: &str = "token:gw/alice";

/// One node that is the agent, the gateway and the provider: TLS identity (the node signs grants and possession
/// proofs), an evaluator that permits the tool under a grant and lets only the supervisor issue one, a strict
/// journal, an execution authority entitled to this node's grants, provider enforcement, and a gateway with two
/// named tokens: the supervisor's (`grant:read`, `grant:issue`) and a bystander's (`grant:read`, `grant:issue` too —
/// the policy, not the scope, decides who may grant).
async fn node(http_port: u16) -> (Arc<GossipAgent>, std::path::PathBuf, Arc<AtomicUsize>) {
    let gossip_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let dir = std::env::temp_dir().join(format!("x3-grant-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into(), "mcp:invoke".into()] },
        GatewayNamedToken { name: "bob".into(), token: "bob-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into(), "kv:write".into()] },
    ];
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let me = id.to_string();
    let tool = format!("tool:deploy@{me}");
    let node_principal = crate::agent::gateway_caller::node_principal(&id);
    let journal = dir.join("evidence.log");
    a.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-grant")
            .with_catalogue("cat-grant", "1")
            .map_action("tools/call", "*")
            .map_action("grant.issue", "*")
            // The agent (this node) may call the tool only under a grant.
            .allow(Rule::new(&node_principal, "tools/call", &tool).requiring_grant())
            // Only the supervisor may grant — or refuse — it.
            .map_action("grant.refuse", "*")
            .allow(Rule::new(SUPERVISOR, "grant.issue", &tool))
            .allow(Rule::new(SUPERVISOR, "grant.refuse", &tool)),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(&node_principal).unwrap());
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("unused", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new())));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = Arc::clone(&ran);
    let handle = a.mcp().register_mcp_tool("deploy", serde_json::json!({"type": "object"}), move |_| {
        let ran = Arc::clone(&ran2);
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("deployed"))
        }
    });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(100)).await;
    (a, journal, ran)
}

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

fn policy(window_ms: u64) -> AgentPolicy {
    AgentPolicy { require_grant_for: vec!["deploy".into()], grant_window: Some(Duration::from_millis(window_ms)), ..AgentPolicy::default() }
}

async fn supervisor(http_port: u16, token: &str, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let mut req = reqwest::Client::new().request(method, format!("http://127.0.0.1:{http_port}{path}")).bearer_auth(token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.expect("gateway");
    (resp.status().as_u16(), resp.json().await.unwrap_or(serde_json::Value::Null))
}

/// **Silence refuses, never proceeds** — and a plain `transition` to a grant-required tool is refused outright, since
/// it carries no arguments to bind. The request record is written under the route-owned namespace with the digest,
/// the rendering and the expiry; the supervisor's list shows it while it stands and not after.
#[tokio::test]
async fn silence_refuses_and_a_plain_transition_to_a_grant_tool_is_refused() {
    let http_port = alloc_port();
    let (a, _journal, ran) = node(http_port).await;
    let sm = a.agent_state_machine(policy(400));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let err = sm.transition(ExecutionState::Invoking { tool: "deploy".into() }).await.unwrap_err();
    assert!(matches!(err, PolicyViolation::GrantNotPresented(_)), "{err}");
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let started = std::time::Instant::now();
    let err = sm.transition_invoking("deploy", &args).await.unwrap_err();
    assert!(matches!(err, PolicyViolation::GrantNotPresented(_)), "{err}");
    assert!(started.elapsed() >= Duration::from_millis(350), "the window was waited out");
    assert_eq!(sm.state(), ExecutionState::Planning, "the transition never committed");
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    // The record: under grant/{agent}/{request}, with the digest and a rendering a human reads.
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let recs = crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &format!("grant/{agent_principal}/"));
    assert_eq!(recs.len(), 1, "one request record: {recs:?}");
    let rec: serde_json::Value = serde_json::from_slice(&recs[0].1).unwrap();
    assert_eq!(rec["tool"], "deploy");
    assert_eq!(rec["arguments"], args);
    assert_eq!(rec["arguments_digest"].as_str().unwrap().len(), 64);
    // Expired now: the supervisor's list does not show it.
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["pending"].as_array().unwrap().len(), 0, "an expired request is not pending: {body}");
    a.shutdown().await;
}

/// **An issued grant lets the call run, bound to its arguments.** The agent asks; the supervisor sees the request,
/// issues; the agent's wait verifies the grant with this node's authority and commits `Invoking`; the grant presented
/// on `/mcp` is established at the gateway and at the provider, and both records carry its hash. The same grant
/// presented for other arguments is refused by name at the gateway, and the tool does not run.
#[tokio::test]
async fn an_issued_grant_lets_the_call_run_and_binds_it_to_its_arguments() {
    let http_port = alloc_port();
    let (a, journal, ran) = node(http_port).await;
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    // The agent waits in the background; the supervisor answers.
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    let prefix = format!("grant/{agent_principal}/");
    assert!(until(|| !crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).is_empty(), Duration::from_secs(5)).await, "the request is written");
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert_eq!(status, 200);
    let pending = body["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{body}");
    let request = pending[0]["request"].as_str().unwrap().to_string();
    assert_eq!(pending[0]["arguments"], args, "a human reads the arguments");
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue", "ttl_secs": 60}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["local_durability"].as_str().map(|_| true), Some(true), "the receipt names its rung: {body}");
    let mandate_json = waiting.await.unwrap().expect("the grant verified at the agent").expect("a grant-required tool returns its grant");
    let presented: crate::agent::gateway_authority::PresentedMandate = serde_json::from_value(mandate_json.clone()).unwrap();
    assert_eq!(sm.state(), ExecutionState::Invoking { tool: "deploy".into() });
    assert!(presented.grant.mandate.is_grant());
    // Answered: no longer pending; answering again is a conflict.
    let (_, body) = supervisor(http_port, "alice-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert_eq!(body["pending"].as_array().unwrap().len(), 0);
    let (status, _) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 409);

    // The call, with the grant presented: permitted at the gateway and at the provider, the hash on both records.
    // The grant's holder is the node, so the call is the agent's own — a member's direct call carrying the grant in its
    // caller envelope (`rpc_call_with_mandate`), the door the bypass matrix names; the provider verifies it there.
    let resource = format!("tool:deploy@{}", a.node_id());
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "deploy", "arguments": args}}).to_string();
    let reply = a.service().rpc_call_with_mandate(a.node_id().clone(), "mcp.invoke", payload.into_bytes(), &mandate_json, &resource, Duration::from_secs(5)).await.unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert!(until(|| records(&journal).iter().any(|r| r.enforcement_point == "provider"), Duration::from_secs(5)).await);
    let grant_hash = crate::agent::action_evaluator::hex32(&presented.grant.mandate.digest());
    let provider: Vec<_> = records(&journal).into_iter().filter(|r| r.enforcement_point == "provider").collect();
    assert!(provider.iter().any(|r| r.mandate_digest.as_deref() == Some(grant_hash.as_str())), "the grant's hash is on the record: {provider:?}");
    // Other arguments under the same grant: refused by name, the tool does not run.
    let other = serde_json::json!({"env": "prod", "replicas": 300});
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "deploy", "arguments": other}}).to_string();
    let reply = a.service().rpc_call_with_mandate(a.node_id().clone(), "mcp.invoke", payload.into_bytes(), &mandate_json, &resource, Duration::from_secs(5)).await.unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert!(reply.get("error").is_some(), "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap_or("").contains("bound to arguments"), "{reply}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the tool did not run for other arguments");
    a.shutdown().await;
}

/// **Issuing — and refusing — is an action the evaluator decides; the namespace is route-owned.** A bystander
/// holding `grant:issue` is refused by policy (403, `authority_not_established`: no allowance names them) for issuing
/// and for refusing, and nothing is written; a refusal by the supervisor reaches the agent as `GrantRefused` with the
/// reason; the raw KV route refuses a write under `grant/` (403 `protected_key`); an agent on another node is
/// refused `not_this_node` until the second PR carries grants across nodes.
#[tokio::test]
async fn issuing_is_an_action_the_evaluator_decides_and_the_namespace_is_route_owned() {
    let http_port = alloc_port();
    let (a, _journal, ran) = node(http_port).await;
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let (sm2, args) = (Arc::clone(&sm), serde_json::json!({"env": "prod"}));
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args).await });
    let prefix = format!("grant/{agent_principal}/");
    assert!(until(|| !crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).is_empty(), Duration::from_secs(5)).await);
    let (_, body) = supervisor(http_port, "bob-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    let request = body["pending"][0]["request"].as_str().unwrap().to_string();
    // The bystander may read, and holds grant:issue — the policy still says no.
    let (status, body) = supervisor(http_port, "bob-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 403, "{body}");
    // A default-deny evaluator with no allowance for the bystander answers *not established* — the honest word for an
    // approver the policy never named — never a permit.
    assert_eq!(body["error"], "authority_not_established", "{body}");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{request}/issued")).is_none(), "nothing issued");
    let (status, body) = supervisor(http_port, "bob-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "refuse", "reason": "no"}))).await;
    assert_eq!(status, 403, "a bystander cannot refuse either: {body}");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{request}/refused")).is_none(), "nothing refused");
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/node:127.0.0.1:1/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_this_node")), "{body}");
    // The raw KV route cannot forge an answer.
    let (status, body) = supervisor(http_port, "bob-secret", reqwest::Method::POST, "/gateway/kv", Some(serde_json::json!({"key": format!("grant/{agent_principal}/{request}/issued"), "value_b64": ""}))).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"], "protected_key", "{body}");
    // The supervisor refuses, with a reason the agent sees.
    let (status, _) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "refuse", "reason": "not during the freeze"}))).await;
    assert_eq!(status, 200);
    let err = waiting.await.unwrap().unwrap_err();
    assert!(matches!(&err, PolicyViolation::GrantRefused(why) if why.contains("not during the freeze") && why.contains("token:gw/alice")), "{err}");
    assert_eq!(sm.state(), ExecutionState::Planning);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    a.shutdown().await;
}

/// An evaluator that decides `tools/call` under another policy revision than the one it issues grants under — the
/// shape of a policy change between approval and call.
struct TwoRevisions(ReferenceEvaluator);

impl crate::ActionEvaluator for TwoRevisions {
    fn evaluate(&self, envelope: &crate::ActionEnvelope) -> crate::Decision {
        let mut d = self.0.evaluate(envelope);
        if envelope.operation == "tools/call" {
            d.policy_revision = "rev-call".into();
        }
        d
    }
    fn security_relevant_arguments(&self) -> Vec<String> {
        self.0.security_relevant_arguments()
    }
    fn mapping(&self, operation: &str, resource: &str) -> crate::ActionMapping {
        self.0.mapping(operation, resource)
    }
}

/// **A grant is bound to the policy revision it was approved under, and consumed by the commit it admitted.** The
/// evaluator issues under `rev-grant` and decides the call under `rev-call`: the agent's own verification does not
/// read the policy, so the transition commits; the provider's preflight refuses the call `BoundToOtherPolicy` —
/// **before** it is admitted into the actor's history — and the tool does not run. Then a second `transition_invoking`
/// for the same tool and arguments finds the first request consumed and asks again rather than re-reading its grant.
#[tokio::test]
async fn a_grant_is_bound_to_its_policy_revision_and_consumed_by_its_commit() {
    let http_port = alloc_port();
    let gossip_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let dir = std::env::temp_dir().join(format!("x3-grant-rev-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] }];
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let tool = format!("tool:deploy@{id}");
    let node_principal = crate::agent::gateway_caller::node_principal(&id);
    let journal = dir.join("evidence.log");
    a.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(TwoRevisions(
        ReferenceEvaluator::new("rev-grant")
            .with_catalogue("cat-grant", "1")
            .map_action("tools/call", "*")
            .map_action("grant.issue", "*")
            .allow(Rule::new(&node_principal, "tools/call", &tool).requiring_grant())
            .allow(Rule::new(SUPERVISOR, "grant.issue", &tool)),
    )));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(&node_principal).unwrap());
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 }).unwrap();
    a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new())));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = Arc::clone(&ran);
    let handle = a.mcp().register_mcp_tool("deploy", serde_json::json!({"type": "object"}), move |_| {
        let ran = Arc::clone(&ran2);
        async move { ran.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("deployed")) }
    });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let args = serde_json::json!({"env": "prod"});
    let agent_principal = node_principal.clone();
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    let first_request = issue_pending(Arc::clone(&a), http_port, agent_principal.clone()).await;
    let mandate_json = waiting.await.unwrap().expect("verified at the agent").expect("a grant");
    let presented: crate::agent::gateway_authority::PresentedMandate = serde_json::from_value(mandate_json.clone()).unwrap();
    assert_eq!(presented.grant.mandate.grant_binding().unwrap().policy_revision.as_deref(), Some("rev-grant"));
    // The call: the provider decides under rev-call — refused by name, before admission, and the tool does not run.
    let resource = format!("tool:deploy@{id}");
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "deploy", "arguments": args}}).to_string();
    let reply = a.service().rpc_call_with_mandate(id.clone(), "mcp.invoke", payload.into_bytes(), &mandate_json, &resource, Duration::from_secs(5)).await.unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert!(reply["error"]["message"].as_str().unwrap_or("").contains("bound to policy revision \"rev-grant\""), "{reply}");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool did not run");
    assert!(until(|| records(&journal).iter().any(|r| r.enforcement_point == "provider"), Duration::from_secs(5)).await);
    let provider: Vec<_> = records(&journal).into_iter().filter(|r| r.enforcement_point == "provider").collect();
    assert!(provider.iter().all(|r| r.decision != crate::agent::action_evaluator::DecisionKind::Permit), "no permit was recorded for a call bound to another revision: {provider:?}");
    assert!(provider.iter().any(|r| r.mandate_digest.is_some()), "the refused grant's hash is on its record");
    // Consumed: the same tool and arguments ask again — a second request, not the first grant re-read.
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{first_request}/consumed")).is_some(), "the first request is marked consumed");
    sm.transition(ExecutionState::Reflecting).await.unwrap();
    sm.transition(ExecutionState::Planning).await.unwrap();
    let (sm3, args3) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm3.transition_invoking("deploy", &args3).await });
    let second_request = issue_pending(Arc::clone(&a), http_port, agent_principal.clone()).await;
    assert_ne!(second_request, first_request, "a new request, not the consumed one");
    waiting.await.unwrap().expect("the second grant verified").expect("a grant");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Wait for a fresh (unanswered, unconsumed) request under the agent, issue it as the supervisor, return its id.
async fn issue_pending(a: Arc<GossipAgent>, http_port: u16, agent_principal: String) -> String {
    let prefix = format!("grant/{agent_principal}/");
    let fresh = |a: &GossipAgent| -> Option<String> {
        let entries = crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix);
        entries.iter().find_map(|(k, _)| {
            let rest = k.strip_prefix(prefix.as_str())?;
            if rest.contains('/') { return None; }
            let answered = entries.iter().any(|(s, _)| s.as_ref() == format!("{k}/issued") || s.as_ref() == format!("{k}/consumed") || s.as_ref() == format!("{k}/refused"));
            (!answered).then(|| rest.to_string())
        })
    };
    assert!(until(|| fresh(&a).is_some(), Duration::from_secs(5)).await, "a fresh request is written");
    let request = fresh(&a).unwrap();
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200, "{body}");
    request
}
