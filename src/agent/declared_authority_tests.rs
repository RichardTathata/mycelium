//! Identity-and-lifecycle I0 at the live doors: a node whose authority comes from its `[authority]` table alone —
//! no `with_*` call in code, as the `mycelium` binary builds a node — over the `governed-call` scaffold's unit.
//!
//! Seen failing first: with [`super::declared_authority::attach`] planted to return before attaching anything (the
//! node as the binary built it until this row — the table read, nothing enforced), the call no rule permits runs at
//! the gateway door and the call under a revoked grant runs at the provider door.

use crate::agent::action_evaluator::AeEvidence;
use crate::agent::state_machine::{AgentPolicy, ExecutionState};
use crate::config::{AuthorityConfig, AuthorityTier, GatewayNamedToken, GrantEntitlement, TlsConfig};
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The scaffold's unit, as `mycelium init` writes it.
const PURCHASER: &str = include_str!("../../templates/governed-call/units/purchaser.toml");
/// The treasurer, as this gateway names it: a named token's principal is `token:{issuer}/{name}`.
const TREASURER: &str = "token:gw/treasurer";

/// The scaffold's unit as the doors can decide it: the issuer written as its gateway principal (a rule's actor is
/// matched exactly, and the scaffold's bare `treasurer` is no principal a gateway presents), and the treasurer
/// allowed to revoke what they issue (the scaffold declares no `grant.revoke` rule).
fn purchaser_at_the_gateway() -> String {
    let unit = PURCHASER.replace("actor     = \"treasurer\"", &format!("actor     = \"{TREASURER}\""));
    assert!(unit.contains(TREASURER), "the scaffold's issuer rule changed shape; update this test");
    format!("{unit}\n[[rule]]\nactor     = \"{TREASURER}\"\noperation = \"grant.revoke\"\nresource  = \"tool:purchase.approve\"\n")
}

/// A second unit in the same directory whose rule would admit the call the purchaser's rule refuses.
const OTHER: &str = "principal = \"auditor\"\n\n[[rule]]\nactor     = \"*\"\noperation = \"tools/call\"\nresource  = \"tool:purchase.place\"\n";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("i0-{tag}-{}", alloc_port()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn units(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
    let units = dir.join("units");
    std::fs::create_dir_all(&units).unwrap();
    for (name, body) in files {
        std::fs::write(units.join(format!("{name}.toml")), body).unwrap();
    }
    units
}

fn authority(dir: &Path, declarations: PathBuf, unit: &str) -> AuthorityConfig {
    AuthorityConfig {
        declarations,
        unit: unit.into(),
        journal: dir.join("evidence.log"),
        tier: AuthorityTier::Serialised,
        freshness_ms: 120_000,
        skew_ms: 1_000,
        entitle_grants: GrantEntitlement::Node,
        scope: None,
    }
}

/// One node — gateway, agent and provider — configured, never attached to in code.
fn config(dir: &Path, http_port: u16, auth: Option<AuthorityConfig>) -> (NodeId, GossipConfig) {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "treasurer".into(), token: "treasurer-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] },
    ];
    cfg.authority = auth;
    (id, cfg)
}

/// Start the node and register the scaffold's two tools, counting their runs.
async fn node(dir: &Path, http_port: u16, auth: AuthorityConfig) -> (Arc<GossipAgent>, Arc<AtomicUsize>) {
    let (id, cfg) = config(dir, http_port, Some(auth));
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
    let ran = Arc::new(AtomicUsize::new(0));
    for tool in ["purchase.place", "purchase.approve"] {
        let ran = Arc::clone(&ran);
        let handle = a.mcp().register_mcp_tool(tool, serde_json::json!({"type": "object"}), move |_| {
            let ran = Arc::clone(&ran);
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!("done"))
            }
        });
        std::mem::forget(handle);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    (a, ran)
}

fn records(journal: &Path) -> Vec<AeEvidence> {
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

/// `POST /mcp` `tools/call` as alice. `Ok` is the tool's answer; `Err` the status and body.
async fn mcp_call(http_port: u16, tool: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
    let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{http_port}/mcp")).bearer_auth("alice-secret").json(&body).send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let reply: serde_json::Value = resp.json().await.unwrap_or_default();
    if status != 200 || reply.get("error").is_some() { Err(format!("{status} {reply}")) } else { Ok(reply["result"].clone()) }
}

/// **The gateway door decides by the unit's own rules — and only this unit's.** The scaffold's rule admits
/// `purchase.place` on the petty-cash ledger: that call runs, decided at the gateway and again at the provider; the
/// capital ledger is refused before the tool runs,
/// and a second unit in the same directory whose rule admits any `purchase.place` admits nothing here. Each decision
/// is on the record, and no answer or record says `no_evaluator`.
#[tokio::test]
async fn the_gateway_door_decides_by_this_units_rules_and_records_each_decision() {
    let dir = scratch("gateway");
    let declarations = units(&dir, &[("purchaser", PURCHASER), ("auditor", OTHER)]);
    let http_port = alloc_port();
    let auth = authority(&dir, declarations, "purchaser");
    let journal = auth.journal.clone();
    let (a, ran) = node(&dir, http_port, auth).await;

    let refused = mcp_call(http_port, "purchase.place", serde_json::json!({"ledger": "capital"})).await.unwrap_err();
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the call no rule permits did not run: {refused}");
    assert!(!refused.contains("no_evaluator"), "{refused}");
    let permitted = mcp_call(http_port, "purchase.place", serde_json::json!({"ledger": "petty-cash"})).await;
    assert!(permitted.is_ok(), "the scaffold's rule admits the petty-cash ledger: {permitted:?}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    assert!(until(|| records(&journal).len() >= 2, Duration::from_secs(5)).await, "both decisions are journaled");
    let evidence = records(&journal);
    // No allowance matches the capital ledger: the reference evaluator's answer is *indeterminate* (only a prohibition
    // denies), and indeterminate is never a permit.
    assert!(evidence.iter().any(|e| e.decision == crate::agent::action_evaluator::DecisionKind::Indeterminate && e.reason.contains("no allowance matches")), "the refusal is recorded: {:?}", evidence.iter().map(|e| (e.decision, e.reason.clone())).collect::<Vec<_>>());
    assert!(evidence.iter().any(|e| e.decision == crate::agent::action_evaluator::DecisionKind::Permit), "the permitted call is recorded");
    assert!(evidence.iter().all(|e| !e.reason.contains("no_evaluator")), "{:?}", evidence.iter().map(|e| e.reason.clone()).collect::<Vec<_>>());
    a.shutdown().await;
}

/// **The provider door refuses a call under a revoked grant.** The purchaser's `purchase.approve` waits for the
/// treasurer's grant; the treasurer issues it at this gateway, then revokes it; a member's direct call presenting the
/// grant is refused `revoked` at the provider and the tool does not run. The refusal is on the record.
#[tokio::test]
async fn the_provider_door_refuses_a_call_under_a_revoked_grant() {
    let dir = scratch("provider");
    let body = purchaser_at_the_gateway();
    let declarations = units(&dir, &[("purchaser", body.as_str())]);
    let http_port = alloc_port();
    let auth = authority(&dir, declarations, "purchaser");
    let journal = auth.journal.clone();
    let (a, ran) = node(&dir, http_port, auth).await;
    let agent = crate::agent::gateway_caller::node_principal(a.node_id());

    let sm = a.agent_state_machine(AgentPolicy { require_grant_for: vec!["purchase.approve".into()], grant_window: Some(Duration::from_secs(10)), ..AgentPolicy::default() });
    sm.transition(ExecutionState::Planning).await.unwrap();
    let args = serde_json::json!({"amount": 40});
    let (sm2, args2) = (Arc::clone(&sm), args.clone());
    let waiting = tokio::spawn(async move { sm2.transition_invoking("purchase.approve", &args2).await });
    let prefix = format!("grant/{agent}/");
    let pending = |a: &GossipAgent| -> Option<String> {
        crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix).iter().find_map(|(k, _)| k.strip_prefix(prefix.as_str()).filter(|r| !r.contains('/')).map(str::to_string))
    };
    assert!(until(|| pending(&a).is_some(), Duration::from_secs(5)).await, "the agent asks");
    let request = pending(&a).unwrap();
    let treasurer = |method: reqwest::Method, path: String, body: Option<serde_json::Value>| async move {
        let mut req = reqwest::Client::new().request(method, format!("http://127.0.0.1:{http_port}{path}")).bearer_auth("treasurer-secret");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.json::<serde_json::Value>().await.unwrap_or_default())
    };
    let (status, issued) = treasurer(reqwest::Method::POST, format!("/gateway/grants/{agent}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200, "the declared issuer issues: {issued}");
    let mandate = waiting.await.unwrap().expect("the grant verifies at the agent").expect("a grant-required tool returns its grant");
    let (status, revoked) = treasurer(reqwest::Method::POST, format!("/gateway/grants/{agent}/{request}/revoke"), Some(serde_json::json!({"reason": "freeze"}))).await;
    assert_eq!(status, 200, "the declared issuer revokes: {revoked}");

    let resource = format!("tool:purchase.approve@{}", a.node_id());
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "purchase.approve", "arguments": args}}).to_string();
    let reply = a.service().rpc_call_with_mandate(a.node_id().clone(), "mcp.invoke", payload.into_bytes(), &mandate, &resource, Duration::from_secs(10)).await;
    let text = match reply {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => e.to_string(),
    };
    assert!(text.contains("revoked"), "the provider refuses the revoked grant by name: {text}");
    assert!(!text.contains("no_evaluator"), "{text}");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool never ran under the revoked grant");
    let digest = crate::agent::action_evaluator::hex32(&serde_json::from_value::<crate::agent::gateway_authority::PresentedMandate>(mandate).unwrap().grant.mandate.digest());
    assert!(until(|| records(&journal).iter().any(|e| e.enforcement_point == "provider" && e.mandate_digest.as_deref() == Some(digest.as_str()) && e.decision != crate::agent::action_evaluator::DecisionKind::Permit), Duration::from_secs(5)).await,
        "the provider's refusal is journaled with the grant's hash: {:?}", records(&journal).iter().map(|e| (e.enforcement_point.clone(), e.decision, e.reason.clone())).collect::<Vec<_>>());
    a.shutdown().await;
}

/// The reason `start()` gives for `cfg`, which must refuse.
async fn refusal(cfg: GossipConfig, id: NodeId) -> String {
    let a = GossipAgent::new(id, cfg);
    let e = a.start().await.expect_err("the start is refused");
    let text = e.to_string();
    assert!(text.contains("authority"), "the refusal names the table: {text}");
    text
}

/// **Every declaration that cannot be built refuses the start by name** — before a port is bound: an empty
/// directory, a directory that does not exist, an undeclared unit, two units with one principal, an exposure rule
/// the table cannot attach, an evaluator already attached in code, a gate the strict profile refuses, and a journal
/// that cannot be opened.
#[tokio::test]
async fn each_unbuildable_declaration_refuses_the_start_by_name() {
    let dir = scratch("refusals");
    let http_port = alloc_port();
    let cases: Vec<(&str, AuthorityConfig, &str)> = vec![
        ("empty", authority(&dir, units(&dir.join("empty"), &[]), "purchaser"), "holds no unit file"),
        ("missing", authority(&dir, dir.join("no-such-dir"), "purchaser"), "declarations"),
        ("undeclared", authority(&dir, units(&dir.join("undeclared"), &[("purchaser", PURCHASER)]), "treasurer"), "no unit in"),
        ("two", authority(&dir, units(&dir.join("two"), &[("a", PURCHASER), ("b", PURCHASER)]), "purchaser"), "2 units"),
        ("exposure", authority(&dir, units(&dir.join("exposure"), &[("purchaser", &format!("{PURCHASER}\n[[rule]]\nactor = \"*\"\noperation = \"tuple.take\"\nresource = \"*\"\n  [rule.exposure]\n  max_unresolved = 3\n  scope = \"enforcement_point\"\n"))]), "purchaser"), "exposure rule"),
        ("freshness", AuthorityConfig { freshness_ms: 1, ..authority(&dir, units(&dir.join("freshness"), &[("purchaser", PURCHASER)]), "purchaser") }, "execution gate"),
        ("journal", AuthorityConfig { journal: dir.join("journal-is-a-dir"), ..authority(&dir, units(&dir.join("journal"), &[("purchaser", PURCHASER)]), "purchaser") }, "journal"),
    ];
    std::fs::create_dir_all(dir.join("journal-is-a-dir")).unwrap();
    for (name, auth, words) in cases {
        let (id, cfg) = config(&dir, http_port, Some(auth));
        let text = refusal(cfg, id).await;
        assert!(text.contains(words), "case {name}: {text}");
    }
    // An evaluator attached in code beside the table: one source of authority per node.
    let (id, cfg) = config(&dir, http_port, Some(authority(&dir, units(&dir.join("attached"), &[("purchaser", PURCHASER)]), "purchaser")));
    let a = GossipAgent::new(id, cfg);
    a.with_action_evaluator(Arc::new(crate::ReferenceEvaluator::new("code")));
    let text = a.start().await.expect_err("refused").to_string();
    assert!(text.contains("already attached in code"), "{text}");
    // The advisory tier is refused by `validate()`, before the declaration is read.
    let (id, cfg) = config(&dir, http_port, Some(AuthorityConfig { tier: AuthorityTier::Advisory, ..authority(&dir, dir.join("never-read"), "purchaser") }));
    assert!(refusal(cfg, id).await.contains("advisory"));
}

/// **A door the evaluator leaves unmapped refuses the start by name** — the check `start()` runs over every door
/// this build serves, shown on an evaluator missing one door's mapping.
#[test]
fn a_door_the_evaluator_leaves_unmapped_is_named() {
    let cfg = GossipConfig::default();
    let doors = super::declared_authority::doors(&cfg);
    assert!(doors.iter().any(|(p, o)| p == "gateway:mcp/tools/call" && o == "tools/call"), "{doors:?}");
    assert!(doors.iter().any(|(p, o)| p == "provider" && o == "tools/call"), "{doors:?}");
    assert!(doors.iter().any(|(p, o)| p == "gateway:grants/issue" && o == "grant.revoke"), "{doors:?}");
    let mut ev = crate::ReferenceEvaluator::new("r");
    for (_, op) in doors.iter().filter(|(_, o)| o != "grant.revoke") {
        ev = ev.map_action(op.clone(), "*");
    }
    let err = super::declared_authority::check_doors(&ev, &doors).unwrap_err().to_string();
    assert!(err.contains("gateway:grants/issue (grant.revoke)"), "{err}");
    let ev = ev.map_action("grant.revoke", "*");
    assert!(super::declared_authority::check_doors(&ev, &doors).is_ok());
    // A protected kind the operator adds is a door at the provider, under its own name.
    let mut extra = GossipConfig::default();
    extra.protected_rpc_kinds = vec!["ledger.post".into()];
    assert!(super::declared_authority::doors(&extra).iter().any(|(p, o)| p == "provider" && o == "ledger.post"));
}

/// **A misspelt key is refused when the file loads**, never left at its default.
#[test]
fn an_unknown_key_in_the_table_is_refused_at_load() {
    let bad = "[authority]\ndeclarations = \"units\"\nunit = \"purchaser\"\njournal = \"e.log\"\nfreshnes_ms = 5\n";
    let err = toml::from_str::<GossipConfig>(bad).unwrap_err().to_string();
    assert!(err.contains("freshnes_ms"), "{err}");
    let good = "[authority]\ndeclarations = \"units\"\nunit = \"purchaser\"\njournal = \"e.log\"\n";
    let cfg: GossipConfig = toml::from_str(good).unwrap();
    let auth = cfg.authority.unwrap();
    assert_eq!((auth.tier, auth.freshness_ms, auth.skew_ms, auth.entitle_grants, auth.scope), (AuthorityTier::Serialised, 120_000, 1_000, GrantEntitlement::Node, None));
}
