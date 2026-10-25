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
    node_with(http_port, Opts::default()).await
}

/// What a test node differs in (adopter-surface X3, second PR).
#[derive(Default, Clone)]
struct Opts {
    /// Fixed ports (a restart over the same identity); fresh ones otherwise.
    gossip_port: Option<u16>,
    /// `[persistence]` under the node's directory, and the directory kept across a restart.
    persist: bool,
    /// The gate's freshness window: the sweep runs every twentieth of it (the drain bound).
    freshness_ms: Option<u64>,
    /// The tool sleeps this long before it counts — work that a revocation can stop.
    slow_ms: u64,
    /// A shared certificate directory, so two nodes trust one another; and the peers to bootstrap to.
    cert_dir: Option<std::path::PathBuf>,
    bootstrap: Vec<NodeId>,
    /// Entitle grants from every fleet member, not only this node (the agent's node in the cross-node test).
    fleet_grants: bool,
    /// `/a2a` mounted.
    #[cfg_attr(not(feature = "a2a"), allow(dead_code))]
    a2a: bool,
}

async fn node_with(http_port: u16, opts: Opts) -> (Arc<GossipAgent>, std::path::PathBuf, Arc<AtomicUsize>) {
    let gossip_port = opts.gossip_port.unwrap_or_else(alloc_port);
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let dir = std::env::temp_dir().join(format!("x3-grant-{gossip_port}"));
    if opts.gossip_port.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.bootstrap_peers = opts.bootstrap.clone();
    cfg.reconnect_backoff_secs = 1;
    cfg.tls = Some(TlsConfig { auto_cert_dir: opts.cert_dir.clone().unwrap_or_else(|| dir.join("tls")), ..Default::default() });
    if opts.persist {
        use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
        cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    }
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into(), "mcp:invoke".into()] },
        GatewayNamedToken { name: "bob".into(), token: "bob-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into(), "kv:write".into()] },
    ];
    let a = GossipAgent::new(id.clone(), cfg);
    #[cfg(feature = "a2a")]
    let a = if opts.a2a { a.with_a2a() } else { a };
    let a = Arc::new(a);
    let me = id.to_string();
    let tool = format!("tool:deploy@{me}");
    let node_principal = crate::agent::gateway_caller::node_principal(&id);
    let journal = dir.join("evidence.log");
    a.with_evidence_journal(EvidenceJournal::open(journal.clone(), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-grant")
            .with_catalogue("cat-grant", "1")
            .map_action("tools/call", "*")
            .map_action("skill.invoke", "*")
            .allow(Rule::new("*", "skill.invoke", format!("skill:depot/dispatch@{me}")))
            .map_action("grant.issue", "*")
            // The agent (this node) may call the tool only under a grant.
            .allow(Rule::new(&node_principal, "tools/call", &tool).requiring_grant())
            // Only the supervisor may grant — or refuse — it.
            .map_action("grant.refuse", "*")
            .allow(Rule::new(SUPERVISOR, "grant.issue", &tool))
            .allow(Rule::new(SUPERVISOR, "grant.refuse", &tool))
            // The supervisor may revoke what they issued — and, in the cross-node test, grant and revoke for the
            // other node's tool (the reference evaluator matches a resource exactly or as `*`).
            .map_action("grant.revoke", "*")
            .allow(Rule::new(SUPERVISOR, "grant.revoke", &tool))
            .allow(Rule::new(SUPERVISOR, "grant.issue", "*"))
            .allow(Rule::new(SUPERVISOR, "grant.revoke", "*")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(&node_principal).unwrap());
    if opts.fleet_grants {
        entitlements.entitle_grants_from_fleet();
    }
    let freshness_ms = opts.freshness_ms.unwrap_or(120_000);
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("unused", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 100 },
        FreshnessPolicy { freshness_ms, interval_ms: freshness_ms / 4, delivery_ms: freshness_ms / 12 },
    )
    .unwrap();
    a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new())));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let (ran2, slow_ms) = (Arc::clone(&ran), opts.slow_ms);
    let handle = a.mcp().register_mcp_tool("deploy", serde_json::json!({"type": "object"}), move |_| {
        let ran = Arc::clone(&ran2);
        async move {
            if slow_ms > 0 {
                tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            }
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
    // An agent on a node this one holds no key of is no member's: refused by name (a member's agent is served since
    // the second PR, `a_grant_is_issued_at_one_node_for_an_agent_on_another_and_revoked_there`).
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/node:127.0.0.1:1/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_a_member")), "{body}");
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

/// The grant and its request for a tool call, as a test gets them: the agent asks in the background, the supervisor
/// issues at `http_port`, the agent's wait returns the presented grant.
async fn granted(a: &Arc<GossipAgent>, http_port: u16, sm: &Arc<crate::agent::state_machine::AgentStateMachine>, args: &serde_json::Value) -> (serde_json::Value, String) {
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let (sm2, args2) = (Arc::clone(sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    let request = issue_pending(Arc::clone(a), http_port, agent_principal).await;
    let mandate = waiting.await.unwrap().expect("the grant verified at the agent").expect("a grant-required tool returns its grant");
    (mandate, request)
}

/// One `tools/call` under a presented grant at a door: a member's direct call (the provider alone) or `POST /mcp`
/// (the gateway, then the provider). `Ok` is the tool's answer; `Err` the refusal's text.
async fn call_deploy(a: &Arc<GossipAgent>, http_port: Option<u16>, mandate: &serde_json::Value, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let resource = format!("tool:deploy@{}", a.node_id());
    match http_port {
        None => {
            let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "deploy", "arguments": args}}).to_string();
            let reply = a.service().rpc_call_with_mandate(a.node_id().clone(), "mcp.invoke", payload.into_bytes(), mandate, &resource, Duration::from_secs(10)).await.map_err(|e| e.to_string())?;
            let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            match reply.get("error") { Some(e) => Err(e.to_string()), None => Ok(reply["result"].clone()) }
        }
        Some(port) => {
            let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "deploy", "arguments": args, "_meta": {"mandate": mandate}}});
            let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{port}/mcp")).bearer_auth("alice-secret").json(&body).send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let reply: serde_json::Value = resp.json().await.unwrap_or_default();
            if status != 200 || reply.get("error").is_some() { Err(format!("{status} {reply}")) } else { Ok(reply["result"].clone()) }
        }
    }
}

/// **A grant admits one call at each door** (the second PR). The grant the agent's wait returned is presented twice
/// on a member's direct call: the first runs the tool, the second is refused `GrantUsed` at the provider and the tool
/// does not run again; the marker under the request names the door. The gateway's door is the same claim in the same
/// preflight under its own tag (a node-held grant is presented by the node, so the provider is the door it reaches;
/// a client-held grant at `/mcp` claims `used/gateway` the same way). Seen failing first: before the claim, the
/// second direct call ran the tool a second time under one approval.
#[tokio::test]
async fn a_grant_admits_one_call_at_each_door() {
    let http_port = alloc_port();
    let (a, journal, ran) = node(http_port).await;
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let (mandate, request) = granted(&a, http_port, &sm, &args).await;
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    assert!(call_deploy(&a, None, &mandate, &args).await.is_ok());
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    let again = call_deploy(&a, None, &mandate, &args).await.unwrap_err();
    assert!(again.contains("already used at provider"), "the second direct call is refused by name: {again}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the tool did not run twice under one grant");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{request}/used/provider")).is_some(), "the provider's marker");
    // The marker alone refuses too: a restart forgets the authority's set, not the WAL — the authority's set is cleared
    // by a fresh authority in the restart test; here the marker is read first, as the preflight does.
    let third = call_deploy(&a, None, &mandate, &args).await.unwrap_err();
    assert!(third.contains("already used at provider"), "{third}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // The refusals are **denials**, sealed into the audit chain — exactly the records `prove_denials`
    // (mycelium-guardrails) reconstructs a refused grant from: the sealed reference names the operation, and the
    // evidence record it cites carries the reason.
    #[cfg(feature = "compliance")]
    {
        let denied: Vec<String> = a.audit_stream(a.node_id()).into_iter()
            .filter(|r| r.record.action == crate::AuditAction::Invoke && r.record.outcome == crate::AuditOutcome::Denied && r.record.principal == agent_principal)
            .filter_map(|r| serde_json::from_str::<serde_json::Value>(r.record.detail.as_deref()?).ok()?["operation_id"].as_str().map(str::to_string))
            .collect();
        assert!(!denied.is_empty(), "a sealed denial for the agent");
        let evidence = records(&journal);
        assert!(denied.iter().any(|op| evidence.iter().any(|e| e.operation_id == *op && e.reason.contains("already used"))), "the sealed denial's evidence record names the used grant: {denied:?}");
    }
    a.shutdown().await;
}

/// **A revoked grant is refused at every door, and stops work already admitted under it** (the second PR). A slow
/// tool is called under a grant; the supervisor revokes at the issuing node; the authority's sweep — judging a
/// grant by its issuer's checkpoint, not the resource gate — cancels the running call within the drain bound, the
/// tool never counts, a stop is recorded. A second grant, revoked before use, is refused `revoked` on a member's
/// direct call (the door a node-held grant reaches; `/mcp` refuses a node's grant to a bearer client as not the
/// holder's, before revocation is asked) and the raw RPC route refuses the protected kind; the tool count does not
/// move. The provider's denials are sealed into the audit chain, which is what `prove_denials` reconstructs from.
/// Seen failing first: the sweep cancelled granted work at its first tick, revoked or not (the resource gate's scope
/// rule).
#[tokio::test]
async fn a_revoked_grant_is_refused_at_every_door_and_stops_admitted_work() {
    let http_port = alloc_port();
    let (a, journal, ran) = node_with(http_port, Opts { freshness_ms: Some(2_000), slow_ms: 3_000, ..Opts::default() }).await;
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let (mandate, request) = granted(&a, http_port, &sm, &args).await;
    // The slow call starts under the grant; the grant is revoked while it runs.
    let (a2, m2, args2) = (Arc::clone(&a), mandate.clone(), args.clone());
    let running = tokio::spawn(async move { call_deploy(&a2, None, &m2, &args2).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Still running at the revocation: the sweep has ticked three or four times and left granted work alone (the
    // defect this test found: the resource gate's scope rule cancelled it at the first tick).
    assert!(!running.is_finished(), "the granted call was stopped before anything revoked it");
    let started = std::time::Instant::now();
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}/revoke"), Some(serde_json::json!({"reason": "freeze"}))).await;
    assert_eq!(status, 200, "{body}");
    let outcome = running.await.unwrap();
    let stopped_after = started.elapsed();
    assert!(outcome.is_err(), "the admitted call was stopped, not completed: {outcome:?}");
    assert!(stopped_after < Duration::from_millis(1_500), "stopped within the drain bound (sweep every 100 ms, plus the handler's own stop): {stopped_after:?}");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool never counted: it was cancelled before its sleep ended");
    let authority = a.task_ctx.execution_authority.get().unwrap();
    assert!(!authority.stop_records().is_empty(), "the stop is recorded for T_drain");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{request}/revoked")).is_some());
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}/revoke"), None).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("already_revoked")), "{body}");
    // A second grant, revoked before it is used: every door refuses it as revoked.
    sm.transition(ExecutionState::Planning).await.ok();
    let (grant2, request2) = granted(&a, http_port, &sm, &args).await;
    let (status, _) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request2}/revoke"), None).await;
    assert_eq!(status, 200);
    let direct = call_deploy(&a, None, &grant2, &args).await.unwrap_err();
    assert!(direct.contains("revoked"), "the provider: {direct}");
    // A bearer presenting the node's grant at /mcp is refused before revocation is asked: the grant is not theirs.
    let gateway = call_deploy(&a, Some(http_port), &grant2, &args).await.unwrap_err();
    assert!(!gateway.contains("deployed"), "{gateway}");
    let raw = reqwest::Client::new().post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call")).bearer_auth("bob-secret")
        .json(&serde_json::json!({"target": a.node_id().to_string(), "method": "mcp.invoke", "payload_b64": "e30=", "timeout_secs": 2})).send().await.unwrap();
    assert_eq!(raw.status().as_u16(), 403, "the raw route refuses the protected kind whatever is carried");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "no door ran the tool");
    // A bystander may not revoke: the evaluator decides, as for issuing.
    sm.transition(ExecutionState::Planning).await.ok();
    let (_, request3) = granted(&a, http_port, &sm, &args).await;
    let (status, body) = supervisor(http_port, "bob-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request3}/revoke"), None).await;
    assert_eq!(status, 403, "{body}");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent_principal}/{request3}/revoked")).is_none());
    // A revoked grant is *authority not established* — the AE vocabulary for revocation, as for any revoked mandate —
    // sealed into the audit chain as an error outcome, not a denial; `prove_denials` lists denials (a used or
    // mis-bound grant, the test above), not this. Stated on the feature page.
    #[cfg(feature = "compliance")]
    {
        let errored: Vec<String> = a.audit_stream(a.node_id()).into_iter()
            .filter(|r| r.record.action == crate::AuditAction::Invoke && r.record.outcome == crate::AuditOutcome::Error && r.record.principal == agent_principal)
            .filter_map(|r| serde_json::from_str::<serde_json::Value>(r.record.detail.as_deref()?).ok()?["operation_id"].as_str().map(str::to_string))
            .collect();
        let evidence = records(&journal);
        // The record is *indeterminate* and names the grant (its hash); the standing's own words ("the grant is
        // revoked") are in the door's answer (asserted above) and in the decision's missing facts, which the evidence
        // record does not carry — a legibility gap recorded on the feature page and the plan row, not closed here.
        let grant2_hash = crate::agent::action_evaluator::hex32(&serde_json::from_value::<crate::agent::gateway_authority::PresentedMandate>(grant2.clone()).unwrap().grant.mandate.digest());
        let indeterminate = |e: &AeEvidence| e.decision == crate::agent::action_evaluator::DecisionKind::Indeterminate && e.mandate_digest.as_deref() == Some(grant2_hash.as_str());
        assert!(errored.iter().any(|op| evidence.iter().any(|e| e.operation_id == *op && indeterminate(e))), "the revoked call sealed as not established, its record indeterminate with the grant's hash: {:?}", evidence.iter().filter(|e| errored.contains(&e.operation_id)).map(|e| (e.decision, e.reason.clone(), e.mandate_digest.clone())).collect::<Vec<_>>());
    }
    a.shutdown().await;
}

/// **A grant is issued at one node for an agent on another, and revoked there** (the second PR). Node A is the
/// supervisor's gateway; node B runs the agent, entitles grants from the fleet, and trusts A's key as a member's.
/// B's agent asks; A's pending list shows the request by gossip; the supervisor issues at A; B's wait reads A's
/// grant and A's checkpoint from the KV, verifies both against A's member key, and runs the call at B. A second
/// request, issued and then revoked at A: B's authority is fed the revoking checkpoint, and the agent's wait refuses
/// `GrantRefused … revoked`. Revoking at B is refused `not_the_issuer`.
#[tokio::test]
async fn a_grant_is_issued_at_one_node_for_an_agent_on_another_and_revoked_there() {
    let (http_a, http_b) = (alloc_port(), alloc_port());
    let cert_dir = std::env::temp_dir().join(format!("x3-grant-fleet-{http_a}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let (a, _ja, _ran_a) = node_with(http_a, Opts { cert_dir: Some(cert_dir.clone()), ..Opts::default() }).await;
    let (b, _jb, ran_b) = node_with(http_b, Opts { cert_dir: Some(cert_dir.clone()), bootstrap: vec![a.node_id().clone()], fleet_grants: true, ..Opts::default() }).await;
    let knows = |x: &GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    assert!(until(|| knows(&a, b.node_id()) && knows(&b, a.node_id()), Duration::from_secs(15)).await, "the two nodes learned each other's keys");
    let sm = b.agent_state_machine(policy(15_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let agent_b = crate::agent::gateway_caller::node_principal(b.node_id());
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    // The request reaches A by gossip; the supervisor issues at A.
    let request = issue_pending(Arc::clone(&a), http_a, agent_b.clone()).await;
    let mandate = waiting.await.unwrap().expect("B's agent verified A's grant").expect("a grant");
    assert_eq!(sm.state(), ExecutionState::Invoking { tool: "deploy".into() });
    assert!(call_deploy(&b, None, &mandate, &args).await.is_ok(), "the call runs at B under A's grant");
    assert_eq!(ran_b.load(Ordering::SeqCst), 1);
    // Revoking at B: not the issuer.
    let (status, body) = supervisor(http_b, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_b}/{request}/revoke"), None).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_the_issuer")), "{body}");
    // A second request: issued at A, revoked at A before B's wait reads it — B's authority is fed the revocation.
    sm.transition(ExecutionState::Planning).await.ok();
    let b_kv = Arc::clone(&b);
    let (sm3, args3) = (Arc::clone(&sm), args.clone());
    // Hold the agent's wait until the revocation has landed: the issue and the revoke are written at A back to back.
    let waiting = tokio::spawn(async move { sm3.transition_invoking("deploy", &args3).await });
    let prefix = format!("grant/{agent_b}/");
    let a_kv = Arc::clone(&a);
    let fresh = move || {
        let entries = crate::store::scan_kv_prefix(&a_kv.task_ctx.kv_state, &prefix);
        entries.iter().find_map(|(k, _)| {
            let rest = k.strip_prefix(prefix.as_str())?;
            if rest.contains('/') || rest == request.as_str() { return None; }
            Some(rest.to_string())
        })
    };
    assert!(until(|| fresh().is_some(), Duration::from_secs(10)).await, "B's second request reached A");
    let request2 = fresh().unwrap();
    // Issue and revoke at A in one breath: B's wait may see the grant first and the revocation a moment later, or
    // both at once; either way the outcome below is a refusal, because B's door re-feeds the checkpoint on every
    // assessment and the agent's wait checks the standing before it commits.
    let (status, _) = supervisor(http_a, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_b}/{request2}"), Some(serde_json::json!({"decision": "issue", "ttl_secs": 60}))).await;
    assert_eq!(status, 200);
    let (status, body) = supervisor(http_a, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_b}/{request2}/revoke"), Some(serde_json::json!({"reason": "freeze"}))).await;
    assert_eq!(status, 200, "{body}");
    let outcome = waiting.await.unwrap();
    match outcome {
        Err(PolicyViolation::GrantRefused(why)) => assert!(why.contains("revoked"), "{why}"),
        Ok(Some(m)) => {
            // The wait read the grant before the revocation reached B: the doors refuse it once the checkpoint lands.
            assert!(until(|| mycelium_core::ops::kv_get(&b_kv.task_ctx, &format!("grant/{agent_b}/{request2}/revoked")).is_some(), Duration::from_secs(10)).await);
            let err = call_deploy(&b_kv, None, &m, &args).await.unwrap_err();
            assert!(err.contains("revoked"), "the revocation reached B's authority: {err}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(ran_b.load(Ordering::SeqCst), 1, "the revoked grant ran nothing");
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A restarted agent re-waits on its pending request; an expired one asks again** (the row's rev 0.4 exit). The
/// request is on the WAL: the agent asks, the supervisor **issues**, the node is shut down before the agent read the
/// grant and restarted over the same directory and identity; a new state machine asks for the same tool and arguments,
/// finds the one request — it does not ask twice — reads the grant issued against it, feeds its authority the grant's
/// checkpoint (issued before this authority started, which the start rule exempts a grant from — #678's review,
/// finding 1: before, every grant issued before a restart was refused after it), and commits once. With the window
/// past at the restart, the expired request is not re-waited: a second request is written, and silence refuses it.
#[tokio::test]
async fn a_restarted_agent_re_waits_on_its_pending_request_and_an_expired_one_asks_again() {
    let (http_port, gossip_port) = (alloc_port(), alloc_port());
    let opts = Opts { gossip_port: Some(gossip_port), persist: true, ..Opts::default() };
    let dir = std::env::temp_dir().join(format!("x3-grant-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let (a, _j, _ran) = node_with(http_port, opts.clone()).await;
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let prefix = format!("grant/{agent_principal}/");
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let requests = |a: &GossipAgent| crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).into_iter().filter(|(k, _)| k.matches('/').count() == 2).count();
    // Ask with a long window, then crash mid-wait.
    let sm = a.agent_state_machine(policy(20_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    assert!(until(|| requests(&a) == 1, Duration::from_secs(5)).await);
    // The agent is stopped before it can read the answer; the supervisor issues against the standing request.
    waiting.abort();
    let _ = waiting.await;
    let request = issue_pending(Arc::clone(&a), http_port, agent_principal.clone()).await;
    // The restart's authority must not see this checkpoint as "before its start": let the clock move past two skews.
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.shutdown().await;
    // Every handle on the node's context goes: the state machine holds one, and with it the journal's lock.
    drop(sm);
    drop(a);
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The restart: the same identity, the same directory; the request and the grant are back from the WAL.
    let (a, _j, _ran) = node_with(http_port, opts.clone()).await;
    assert_eq!(requests(&a), 1, "the request survived the restart");
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("{prefix}{request}/issued")).is_some(), "the grant survived the restart");
    let sm = a.agent_state_machine(policy(20_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let mandate = sm.transition_invoking("deploy", &args).await.expect("the grant issued before the restart verified after it").expect("a grant");
    assert_eq!(requests(&a), 1, "the restarted agent did not ask twice");
    assert_eq!(sm.state(), ExecutionState::Invoking { tool: "deploy".into() });
    assert!(mycelium_core::ops::kv_get(&a.task_ctx, &format!("{prefix}{request}/consumed")).is_some(), "the one request, consumed once");
    assert!(call_deploy(&a, None, &mandate, &args).await.is_ok(), "one execution");
    drop(sm);
    // Expired: a short window, a crash, a restart past it — a new request, and silence refuses.
    let sm = a.agent_state_machine(policy(300));
    sm.transition(ExecutionState::Planning).await.ok();
    let other = serde_json::json!({"env": "staging"});
    let (sm2, other2) = (Arc::clone(&sm), other.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &other2).await });
    assert!(until(|| requests(&a) == 2, Duration::from_secs(5)).await);
    waiting.abort();
    let _ = waiting.await;
    a.shutdown().await;
    drop(sm);
    drop(a);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (a, _j, _ran) = node_with(http_port, opts).await;
    let sm = a.agent_state_machine(policy(300));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let err = sm.transition_invoking("deploy", &other).await.unwrap_err();
    assert!(matches!(err, PolicyViolation::GrantNotPresented(_)), "{err}");
    assert_eq!(requests(&a), 3, "the expired request was not re-waited: a new one was written");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Read SSE events from `url` under the supervisor's bearer until `want` events arrived or `within` passed: each as
/// `(event name, data)`.
async fn sse_events(url: String, want: usize, within: Duration) -> Vec<(String, serde_json::Value)> {
    use futures_util::StreamExt as _;
    let resp = reqwest::Client::new().get(url).bearer_auth("alice-secret").send().await.expect("stream");
    assert_eq!(resp.status().as_u16(), 200);
    let mut body = resp.bytes_stream();
    let (mut buf, mut out) = (String::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + within;
    while out.len() < want {
        let chunk = tokio::select! {
            c = body.next() => match c { Some(Ok(c)) => c, _ => break },
            _ = tokio::time::sleep_until(deadline) => break,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find("\n\n") {
            let frame = buf[..end].to_string();
            buf = buf[end + 2..].to_string();
            let (mut name, mut data) = (String::new(), String::new());
            for line in frame.lines() {
                if let Some(e) = line.strip_prefix("event: ") { name = e.to_string(); }
                if let Some(d) = line.strip_prefix("data: ") { data = d.to_string(); }
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                out.push((name, v));
            }
        }
    }
    out
}

/// **The stream reports a request once, then its answer** (the third PR). A supervisor on the stream before the
/// agent asks sees the request as it lands, with the arguments a human reads; a supervisor connecting after sees the
/// standing request as a backlog; the issue reaches both as `grant_answered`. Nothing is reported twice.
#[tokio::test]
async fn the_grants_stream_reports_a_request_once_and_then_its_answer() {
    let http_port = alloc_port();
    let (a, _journal, _ran) = node(http_port).await;
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let url = format!("http://127.0.0.1:{http_port}/gateway/grants/stream");
    let early = tokio::spawn(sse_events(url.clone(), 2, Duration::from_secs(10)));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let args = serde_json::json!({"env": "prod", "replicas": 3});
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    let prefix = format!("grant/{agent_principal}/");
    assert!(until(|| !crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).is_empty(), Duration::from_secs(5)).await);
    // A late supervisor: the standing request is the backlog.
    let late = sse_events(url.clone(), 1, Duration::from_secs(5)).await;
    assert_eq!(late.len(), 1, "{late:?}");
    assert_eq!(late[0].0, "grant_request");
    assert_eq!(late[0].1["tool"], "deploy");
    assert_eq!(late[0].1["arguments"], args, "a human reads the arguments");
    let request = late[0].1["request"].as_str().unwrap().to_string();
    let (status, _) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200);
    waiting.await.unwrap().expect("granted");
    let early = early.await.unwrap();
    assert_eq!(early.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["grant_request", "grant_answered"], "{early:?}");
    assert_eq!(early[0].1["request"], request);
    assert_eq!(early[1].1["outcome"], "issued");
    assert_eq!(early[1].1["issuer"], SUPERVISOR);
    // A request nobody answers lapses: the stream says so once, as `grant_expired` (#679's review, finding 6).
    let watching = tokio::spawn(sse_events(url.clone(), 2, Duration::from_secs(10)));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let short = a.agent_state_machine(policy(600));
    short.transition(ExecutionState::Planning).await.unwrap();
    let other = serde_json::json!({"env": "staging"});
    let err = short.transition_invoking("deploy", &other).await.unwrap_err();
    assert!(matches!(err, PolicyViolation::GrantNotPresented(_)));
    let seen = watching.await.unwrap();
    assert_eq!(seen.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["grant_request", "grant_expired"], "{seen:?}");
    assert_eq!(seen[1].1["outcome"], "expired");
    a.shutdown().await;
}

/// **A streamed A2A task waiting for a grant reports `input-required` with the request's id, and `working` once it is
/// answered** (the row's design question, decided: yes, the id and nothing else). The skill's handler asks for a grant
/// while serving the task; the supervisor, reading the status, issues it; the task completes.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn a_streamed_a2a_task_waiting_for_a_grant_reports_input_required_with_the_request_id() {
    let http_port = alloc_port();
    let (a, _journal, ran) = node_with(http_port, Opts { a2a: true, ..Opts::default() }).await;
    let _reg = a.capabilities().advertise_capability(crate::capability::Capability::new("depot", "dispatch"), Duration::from_secs(30));
    let sm = a.agent_state_machine(policy(10_000));
    sm.transition(ExecutionState::Planning).await.unwrap();
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    // The skill's handler: ask for the grant, run the tool under it, answer.
    {
        let (agent, sm) = (Arc::clone(&a), Arc::clone(&sm));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let args = serde_json::json!({"env": "prod"});
                let outcome = match sm.transition_invoking("deploy", &args).await {
                    Ok(Some(mandate)) => match call_deploy(&agent, None, &mandate, &args).await { Ok(_) => "deployed".to_string(), Err(e) => format!("refused: {e}") },
                    other => format!("no grant: {other:?}"),
                };
                agent.service().rpc_respond(&req, outcome.into_bytes());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The supervisor issues the first request that appears.
    let (a2, principal2) = (Arc::clone(&a), agent_principal.clone());
    let issuing = tokio::spawn(async move { issue_pending(a2, http_port, principal2).await });
    let stream_body = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/a2a"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tasks/sendSubscribe", "params": {
            "id": "task-grant-1", "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": "deploy prod"}]}}}))
        .send().await.expect("tasks/sendSubscribe").text().await.unwrap();
    let request = issuing.await.unwrap();
    let states: Vec<&str> = stream_body.match_indices("\"state\":\"").map(|(i, _)| { let rest = &stream_body[i + 9..]; &rest[..rest.find('"').unwrap_or(0)] }).collect();
    assert!(states.contains(&"input-required"), "the task reported the wait: {states:?}\n{stream_body}");
    assert!(stream_body.contains(&format!("grant request {request}")), "the status names the request and nothing else: {stream_body}");
    let input_at = states.iter().position(|s| *s == "input-required").unwrap();
    assert!(states[input_at..].contains(&"working") && states.last() == Some(&"completed"), "working again after the answer, then completed: {states:?}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the tool ran once, under the grant");
    // The same wait on the 1.0 door: the state is `TASK_STATE_INPUT_REQUIRED`, an enum value a 1.0 client accepts
    // (#679's review, finding 1: it was `TASK_STATE_INPUT-REQUIRED`).
    sm.transition(ExecutionState::Planning).await.ok();
    let (a2, principal2) = (Arc::clone(&a), agent_principal.clone());
    let issuing = tokio::spawn(async move { issue_pending(a2, http_port, principal2).await });
    let v10 = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/a2a"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 8, "method": "SendStreamingMessage", "params": {
            "message": {"messageId": "m-8", "role": "ROLE_USER", "parts": [{"text": "deploy prod again"}]},
            "metadata": {"skillId": "depot/dispatch"}}}))
        .send().await.expect("SendStreamingMessage").text().await.unwrap();
    let request2 = issuing.await.unwrap();
    assert!(v10.contains("TASK_STATE_INPUT_REQUIRED") && !v10.contains("INPUT-REQUIRED"), "the 1.0 name: {v10}");
    assert!(v10.contains(&format!("grant request {request2}")), "{v10}");
    assert!(v10.contains("TASK_STATE_COMPLETED"), "{v10}");
    assert_eq!(ran.load(Ordering::SeqCst), 2);
    a.shutdown().await;
}

/// **Arguments of a class the policy does not render are withheld, with the class and the size; the grant still binds
/// their digest** (the third PR's rendering policy). Unclassified arguments are rendered as before.
#[tokio::test]
async fn a_request_withholds_arguments_of_a_class_the_policy_does_not_render() {
    let http_port = alloc_port();
    let (a, _journal, ran) = node(http_port).await;
    let mut p = policy(10_000);
    p.grant_arguments_class = Some("confidential".into());
    p.grant_renderable_classes = vec!["public".into()];
    let sm = a.agent_state_machine(p);
    sm.transition(ExecutionState::Planning).await.unwrap();
    let args = serde_json::json!({"env": "prod", "card": "4111-1111-1111-1111"});
    let agent_principal = crate::agent::gateway_caller::node_principal(a.node_id());
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("deploy", &args2).await });
    let prefix = format!("grant/{agent_principal}/");
    assert!(until(|| !crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).is_empty(), Duration::from_secs(5)).await);
    let (status, body) = supervisor(http_port, "alice-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert_eq!(status, 200);
    let rec = &body["pending"][0];
    assert_eq!(rec["arguments_class"], "confidential");
    assert_eq!(rec["arguments"]["class"], "confidential", "withheld, naming the class: {rec}");
    assert!(rec["arguments"]["withheld"].as_str().unwrap().contains("not rendered"), "{rec}");
    assert!(!rec.to_string().contains("4111-1111"), "the value never entered the gossiped record: {rec}");
    assert_eq!(rec["arguments_digest"].as_str().unwrap().len(), 64, "the digest is there for the grant to bind");
    let request = rec["request"].as_str().unwrap().to_string();
    let (status, _) = supervisor(http_port, "alice-secret", reqwest::Method::POST, &format!("/gateway/grants/{agent_principal}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200);
    let mandate = waiting.await.unwrap().unwrap().unwrap();
    assert!(call_deploy(&a, None, &mandate, &args).await.is_ok(), "the grant binds the digest of the withheld arguments");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
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
