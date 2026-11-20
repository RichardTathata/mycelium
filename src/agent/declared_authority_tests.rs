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

/// The scaffold's unit and configuration, as `mycelium init` writes them.
const PURCHASER: &str = include_str!("../../templates/governed-call/units/purchaser.toml");
const NODE_TOML: &str = include_str!("../../templates/governed-call/node.toml");
/// The scaffold's gateway issuer (`gateway_identity_issuer` in its node.toml): a named token's principal is
/// `token:{issuer}/{name}`.
const ISSUER: &str = "governed-call";

/// A second unit in the same directory whose rule would admit the call the purchaser's rule refuses — on any resource,
/// so it would admit at this node's door if it were loaded.
const OTHER: &str = "principal = \"auditor\"\n\n[[rule]]\nactor     = \"*\"\noperation = \"tools/call\"\nresource  = \"*\"\n";

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
    cfg.tls = Some({
        let mut tls_cfg = TlsConfig::default();
        tls_cfg.auto_cert_dir = dir.join("tls");
        tls_cfg
    });
    cfg.persistence = Some(persistence(dir));
    cfg.gateway_identity_issuer = Some(ISSUER.into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "treasurer".into(), token: "treasurer-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] },
    ];
    cfg.authority = auth;
    (id, cfg)
}

fn persistence(dir: &Path) -> crate::config::PersistenceConfig {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse }
}

/// Register the scaffold's two tools on a started node, counting their runs.
async fn tools(a: &Arc<GossipAgent>) -> Arc<AtomicUsize> {
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
    ran
}

/// Start the node and register the scaffold's two tools, counting their runs.
async fn node(dir: &Path, http_port: u16, auth: AuthorityConfig) -> (Arc<GossipAgent>, Arc<AtomicUsize>) {
    let (id, cfg) = config(dir, http_port, Some(auth));
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
    let ran = tools(&a).await;
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

/// **The provider door refuses a call under a revoked grant**, over the scaffold's unit as `mycelium init` writes it.
/// The purchaser's `purchase.approve` waits for the treasurer's grant; the treasurer issues it at this gateway, then
/// revokes it; a member's direct call presenting the grant is refused `revoked` at the provider and the tool does not
/// run. The refusal is on the record. Seen failing first with the scaffold as it was (a bare `treasurer`, no
/// `grant.revoke` rule): the issue was refused at the grant door.
#[tokio::test]
async fn the_provider_door_refuses_a_call_under_a_revoked_grant() {
    let dir = scratch("provider");
    let declarations = units(&dir, &[("purchaser", PURCHASER)]);
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
    // Naming what the treasurer read (identity-and-lifecycle I7-S): the request's own record.
    let record: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{agent}/{request}")).and_then(|b| serde_json::from_slice(&b).ok()).expect("the request record");
    let shown = crate::Shown::from_record(&record).expect("a block");
    let (status, issued) = treasurer(reqwest::Method::POST, format!("/gateway/grants/{agent}/{request}"), Some(serde_json::json!({"decision": "issue", "shown": shown}))).await;
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
    // The advisory tier and a zero skew are refused by `validate()`, before the declaration is read.
    let (id, cfg) = config(&dir, http_port, Some(AuthorityConfig { tier: AuthorityTier::Advisory, ..authority(&dir, dir.join("never-read"), "purchaser") }));
    assert!(refusal(cfg, id).await.contains("advisory"));
    let (id, cfg) = config(&dir, http_port, Some(AuthorityConfig { skew_ms: 0, ..authority(&dir, units(&dir.join("skew"), &[("purchaser", PURCHASER)]), "purchaser") }));
    let text = refusal(cfg, id).await;
    assert!(text.contains("skew_ms"), "case skew: {text}");
    // Without `[persistence]`: a restart would forget which grants were used.
    let (id, mut cfg) = config(&dir, http_port, Some(authority(&dir, units(&dir.join("unpersisted"), &[("purchaser", PURCHASER)]), "purchaser")));
    cfg.persistence = None;
    let text = refusal(cfg, id).await;
    assert!(text.contains("needs [persistence]"), "case persistence: {text}");
}

/// **One source of authority per node** (#733's review, finding 1): any of the evaluator, the execution authority or
/// the evidence journal attached in code beside the table refuses the start, naming it — the code's would be the live
/// one and the table's journal would record nothing. Seen failing first with the check made evaluator-only: the
/// code's execution authority and journal started beside the table.
#[tokio::test]
async fn an_authority_attached_in_code_beside_the_table_refuses_the_start() {
    use crate::agent::gateway_authority::ExecutionAuthority;
    use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
    use crate::mandate::grant::{EntitlementTable, GrantVerifier};
    let dir = scratch("attached");
    let http_port = alloc_port();
    let declarations = units(&dir, &[("purchaser", PURCHASER)]);
    let gate = || ExecutionGate::strict(crate::mandate::ResourceAuthority::new("code", 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 }).unwrap();
    for what in ["evaluator", "authority", "journal"] {
        let (id, cfg) = config(&dir, http_port, Some(authority(&dir, declarations.clone(), "purchaser")));
        let a = GossipAgent::new(id, cfg);
        match what {
            "evaluator" => a.with_action_evaluator(Arc::new(crate::ReferenceEvaluator::new("code"))),
            "authority" => a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate(), GrantVerifier::new(EntitlementTable::new()), crate::knowledge::issuer::TrustedExternalIssuers::new()))),
            _ => a.with_evidence_journal(crate::EvidenceJournal::open(dir.join(format!("code-{}.log", alloc_port())), crate::EvidenceProfile::Strict).unwrap()),
        }
        let text = a.start().await.expect_err("refused").to_string();
        assert!(text.contains("already attached in code"), "{what}: {text}");
    }
}

/// **A retried `start()` is not refused by the table's own attachments**: the one-source check runs on every
/// `preflight()`, and once the table has attached, a second `preflight()` — what a retried `start()` runs first —
/// reads the attachments as the table's. (A whole `start()` cannot be retried past persistence, which keeps its
/// directory's ownership lock; that predates this table.)
#[tokio::test]
async fn a_second_preflight_reads_the_tables_attachments_as_its_own() {
    let dir = scratch("retry");
    let (id, cfg) = config(&dir, alloc_port(), Some(authority(&dir, units(&dir, &[("purchaser", PURCHASER)]), "purchaser")));
    let a = GossipAgent::new(id, cfg);
    a.preflight().expect("planned");
    let planned = a.task_ctx.declared_authority.get().cloned().expect("a plan");
    super::declared_authority::attach(&a, &planned).expect("attached");
    a.preflight().expect("the table's own attachments are not a second source");
    assert!(Arc::ptr_eq(a.task_ctx.execution_authority.get().unwrap(), &planned.authority), "the table's authority is the live one");
}

/// **The table meets the secure profile's authority requirements** (#733's review, finding 2): with `[authority]` in
/// place of code attachments, a node configured as `a_node_providing_every_requirement_starts_under_the_secure_profile`
/// configures it passes `secure-single-domain`'s check and starts — `authz.durable_epochs` among them, the epochs
/// journaled under the persistence directory. Seen failing first with the durable epochs not attached: the check named
/// `authz.durable_epochs` unmet.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn the_table_meets_the_secure_profiles_authority_requirements() {
    use crate::agent::guarantee::{check, Resolution, SECURE_SINGLE_DOMAIN};
    let port = alloc_port();
    let root = scratch("secure");
    let cert_dir = root.join("tls");
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let ca_dir = root.join("issuer");
    {
        let issuer_port = alloc_port();
        let mut icfg = GossipConfig::auto();
        icfg.bind_port = issuer_port;
        icfg.tls = Some({
            let mut tls = TlsConfig::default();
            tls.auto_cert_dir = ca_dir.clone();
            tls
        });
        let issuer = GossipAgent::new(NodeId::new("127.0.0.1", issuer_port).unwrap(), icfg);
        issuer.start().await.expect("the issuer node mints the fleet CA");
        issuer.shutdown().await;
    }
    let (cert_pem, key_pem) = crate::issue_node_cert(&ca_dir, &id, &cert_dir).expect("a node certificate issued off-node");
    std::fs::copy(ca_dir.join("ca-cert.pem"), cert_dir.join("ca-cert.pem")).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(alloc_port());
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.tls = Some({
        let mut tls = TlsConfig::default();
        tls.auto_cert_dir = cert_dir.clone();
        tls.cert_pem = Some(cert_pem);
        tls.key_pem = Some(key_pem);
        tls.ca_cert_pem = None;
        tls
    });
    cfg.gateway_tls = Some(crate::GatewayTlsConfig::default());
    cfg.gateway_auth_token = Some("s3cret".into());
    cfg.require_identity_proofs = true;
    cfg.egress.allow_hosts = vec!["idp.coop.example".into()];
    cfg.persistence = Some(persistence(&root));
    cfg.consensus_require_electorate = true;
    cfg.profile = Some("secure-single-domain".into());
    cfg.authority = Some(authority(&root, units(&root, &[("purchaser", PURCHASER)]), "purchaser"));
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.expect("the profile admits the node: every authority requirement is met by the table");
    let r = a.guarantee_report();
    check(&r, &SECURE_SINGLE_DOMAIN).expect("the secure profile's check passes");
    assert!(matches!(r.entry("authz.durable_epochs").unwrap().resolution, Resolution::Enforced));
    assert!(root.join("persist").join(id.to_string()).join("authority").join("epochs.journal").exists(), "the epochs are journaled under the persistence directory");
    a.shutdown().await;
}

/// **A skill rule admits at this node** (#733's review, finding 5): a bare `skill:<ns>/<name>` is qualified to this
/// node, as a tool is — the door presents `skill:<ns>/<name>@<node>`. A skill on another node is not admitted.
#[test]
fn a_bare_skill_resource_is_qualified_to_this_node() {
    let dir = scratch("skill");
    let unit = "principal = \"dispatcher\"\n\n[[rule]]\nactor     = \"*\"\noperation = \"skill.invoke\"\nresource  = \"skill:depot/dispatch\"\n";
    let (id, cfg) = config(&dir, alloc_port(), None);
    let auth = authority(&dir, units(&dir, &[("dispatcher", unit)]), "dispatcher");
    let planned = super::declared_authority::plan(&cfg, &id, &auth).expect("planned");
    let decide = |resource: String| {
        let env = crate::ActionEnvelope::builder("token:governed-call/alice", id.clone(), "skill.invoke", resource)
            .identities("op-1", "op-1/1")
            .validity(1_000, u64::MAX)
            .mapping(planned.evaluator.mapping("skill.invoke", "*"))
            .build();
        planned.evaluator.evaluate(&env).verdict
    };
    assert_eq!(decide(format!("skill:depot/dispatch@{id}")), crate::Verdict::Permit, "the skill at this node");
    assert_ne!(decide("skill:depot/dispatch@127.0.0.1:1".to_string()), crate::Verdict::Permit, "not at another node");
}

/// **A prompt-skill rule admits at this node** (#785's review, F3): a bare `prompt:<ns>/<name>` is qualified to this
/// node, as a tool and a skill are — the gateway's LLM doors and the provider's `llm.invoke` door both present
/// `prompt:<ns>/<name>@<node>`. Without it a unit's rule for a prompt skill had to spell the node's address or fall
/// back on `*`, the wider rule. A prompt skill on another node is not admitted. Seen failing first: with `prompt:`
/// out of the qualification, the skill at this node is not permitted.
#[test]
fn a_bare_prompt_resource_is_qualified_to_this_node() {
    let dir = scratch("prompt");
    let unit = "principal = \"reviewer\"\n\n[[rule]]\nactor     = \"*\"\noperation = \"llm.invoke\"\nresource  = \"prompt:review/summary\"\n";
    let (id, cfg) = config(&dir, alloc_port(), None);
    let auth = authority(&dir, units(&dir, &[("reviewer", unit)]), "reviewer");
    let planned = super::declared_authority::plan(&cfg, &id, &auth).expect("planned");
    let decide = |resource: String| {
        let env = crate::ActionEnvelope::builder("token:governed-call/alice", id.clone(), "llm.invoke", resource)
            .identities("op-1", "op-1/1")
            .validity(1_000, u64::MAX)
            .mapping(planned.evaluator.mapping("llm.invoke", "*"))
            .build();
        planned.evaluator.evaluate(&env).verdict
    };
    assert_eq!(decide(format!("prompt:review/summary@{id}")), crate::Verdict::Permit, "the prompt skill at this node");
    assert_ne!(decide("prompt:review/summary@127.0.0.1:1".to_string()), crate::Verdict::Permit, "not at another node");
    assert_ne!(decide(format!("prompt:review/other@{id}")), crate::Verdict::Permit, "nor another prompt skill");
}

/// **A hosted-capability rule admits at this node** (the decision of 2026-11-14): a bare `cap:<ns>/<name>` is
/// qualified to this node, as a tool, a skill and a prompt skill are — the provider's `cap.invoke/…` door presents
/// `cap:<ns>/<name>@<node>` under the operation `cap.invoke`, which `[authority]` maps as a door of its own. A
/// capability on another node is not admitted, nor another capability here. Seen failing first: with `cap:` out
/// of the qualification, the capability at this node is not permitted.
#[test]
fn a_bare_cap_resource_is_qualified_to_this_node() {
    let dir = scratch("cap");
    let unit = "principal = \"router\"\n\n[[rule]]\nactor     = \"*\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\n";
    let (id, cfg) = config(&dir, alloc_port(), None);
    let auth = authority(&dir, units(&dir, &[("router", unit)]), "router");
    let planned = super::declared_authority::plan(&cfg, &id, &auth).expect("planned");
    assert!(planned.doors.iter().any(|(p, o)| p == "provider" && o == crate::HOSTED_CAPABILITY_OPERATION), "the family's operation is a mapped door: {:?}", planned.doors);
    let decide = |resource: String| {
        let env = crate::ActionEnvelope::builder(format!("node:{id}"), id.clone(), crate::HOSTED_CAPABILITY_OPERATION, resource)
            .identities("op-1", "op-1/1")
            .validity(1_000, u64::MAX)
            .mapping(planned.evaluator.mapping(crate::HOSTED_CAPABILITY_OPERATION, "*"))
            .build();
        planned.evaluator.evaluate(&env).verdict
    };
    assert_eq!(decide(format!("cap:route/optimize@{id}")), crate::Verdict::Permit, "the capability at this node");
    assert_ne!(decide("cap:route/optimize@127.0.0.1:1".to_string()), crate::Verdict::Permit, "not at another node");
    assert_ne!(decide(format!("cap:route/other@{id}")), crate::Verdict::Permit, "nor another capability");
}

// ── `wire-check` beside the provider's door ────────────────────────────────────────────────────────────────────
//
// Every test below sets two readings of one declaration side by side: what `wire_check::check` says of an edge, and
// what a node built from `[authority]` over the same declaration does with the call — a receiver where the hosted
// component's serve loop stands (it reads the same `rpc_rx`) counts what it is handed. The provider's half is the
// canon: the check may say less than the provider decides, never more. One node is the provider and the caller, as
// in `hosted_capability_door_tests` (a member's own unframed frame is decided as `node:{sender}`, and a remote
// member's is too: #814's review drove it over a real connection). Every call is bounded (20 s).

/// The co-op fixture's rule for `route/optimize` as it stood until 2026-11-18 (`tests/fixtures/units/coop/auditor.toml:11`).
const COOP_OLD_RULE: &str = "[[rule]]\nactor            = \"worker\"\noperation        = \"skill.invoke\"\nresource         = \"skill:route/optimize\"\nrequires_mandate = \"routing\"\n";
/// The fixture's rule for the optimizer, as `depot-a.toml` and `depot-b.toml` declare it.
const COOP_RULE: &str = "[[rule]]\nactor     = \"*\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\n";

fn coop_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/units/coop")
}

/// The co-op fixture's units, copied, with each unit's text passed through `edit(unit name, text)`. `no_rules`
/// below cuts a unit's `[[rule]]`s so a test can append its own.
fn coop_with(tag: &str, edit: impl Fn(&str, String) -> String) -> PathBuf {
    let units = scratch(tag).join("units");
    std::fs::create_dir_all(&units).unwrap();
    for entry in std::fs::read_dir(coop_fixture()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(units.join(path.file_name().unwrap()), edit(&name, std::fs::read_to_string(&path).unwrap())).unwrap();
    }
    units
}

/// `text` up to its first `[[rule]]`, then `rules`.
fn no_rules(mut text: String, rules: &str) -> String {
    if let Some(at) = text.find("[[rule]]") {
        text.truncate(at);
    }
    text.push('\n');
    text.push_str(rules);
    text
}

/// The co-op with the depots' rules replaced by `rules` (both depots: each could host the optimizer).
fn coop_depots(tag: &str, rules: &str) -> PathBuf {
    coop_with(tag, |unit, text| if unit.starts_with("depot-") { no_rules(text, rules) } else { text })
}

/// The kinds of the authority findings `wire-check` reports for `requirer`'s edge to `ns/name`, with the report.
fn reads(declarations: &Path, library: &[(String, crate::wire_check::ArtifactDescription)], requirer: &str, edge: &str) -> (Vec<String>, crate::wire_check::Report) {
    let r = crate::wire_check::check(&crate::faults::load_units(declarations).expect("the units load"), library, &crate::wire_check::CheckOptions::default());
    let kinds = r
        .findings
        .iter()
        .filter(|f| f.unit.as_deref() == Some(requirer) && f.message.contains(&format!("requirement {edge}")) && f.kind.ends_with(" edge"))
        .map(|f| f.kind.clone())
        .collect();
    (kinds, r)
}

fn coop_library() -> Vec<(String, crate::wire_check::ArtifactDescription)> {
    vec![(
        "route-optimizer".to_string(),
        crate::wire_check::ArtifactDescription::from_toml_str(include_str!("../../tests/fixtures/units/artifacts/route-optimizer.toml")).expect("the fixture's artifact"),
    )]
}

/// What the check says of the worker's edge to the optimizer.
fn reads_optimizer(declarations: &Path) -> (Vec<String>, crate::wire_check::Report) {
    reads(declarations, &coop_library(), "worker", "route/optimize")
}

/// A member's call at the provider's door.
#[derive(Clone, Copy)]
enum MemberCall {
    /// `rpc_call_with_mandate`: the kind, in the member's own envelope (`node:{self}`), **presenting a mandate** in
    /// the named scope — held by the calling node, enumerating the call, its possession proof signed with the node's
    /// own identity key over the operation, resource and arguments the provider derives, and its grant signed by an
    /// operator key. Everything a member can do to hold a mandate; what is left is whether the provider's execution
    /// authority entitles the operator.
    Mandated(&'static str, &'static str),
    /// `rpc_call`, unframed: the provider decides `node:{sender}`.
    Direct(&'static str),
    /// A skill call in the member's own envelope (`node:{self}`, no scopes, no mandate — what `rpc_call_with_mandate`
    /// frames, less the mandate), naming the skill it acts on; the provider must advertise `ns/name`.
    Skill(&'static str, &'static str),
}

/// Start a provider from `[authority]` over the declaration `make(its node id)` writes, as `unit`; make the call;
/// answer whether the receiver was handed it. A call that did not run was refused with a reason.
async fn door_decides(tag: &str, unit: &str, call: MemberCall, make: impl FnOnce(&NodeId) -> PathBuf) -> bool {
    door_answers(tag, Some(unit), call, make).await.0
}

/// [`door_decides`], with the provider's answer. `unit: None` is a node with **no** `[authority]` table: the same
/// node, enforcing nothing.
async fn door_answers(tag: &str, unit: Option<&str>, call: MemberCall, make: impl FnOnce(&NodeId) -> PathBuf) -> (bool, serde_json::Value) {
    let dir = scratch(tag);
    let (id, mut cfg) = config(&dir, alloc_port(), None);
    let declarations = make(&id);
    cfg.authority = unit.map(|unit| authority(&dir, declarations, unit));
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    a.start().await.unwrap_or_else(|e| panic!("{tag}: the provider starts from the declaration, as {unit:?}: {e}"));
    let kind: String = match call {
        MemberCall::Direct(kind) | MemberCall::Mandated(kind, _) => kind.to_string(),
        MemberCall::Skill(..) => "skill.invoke".to_string(),
    };
    let _advertised = match call {
        MemberCall::Skill(ns, name) => Some(a.capabilities().advertise_capability(crate::Capability::new(ns, name), Duration::from_secs(30))),
        MemberCall::Direct(_) | MemberCall::Mandated(..) => None,
    };
    let ran = Arc::new(AtomicUsize::new(0));
    let (svc, seen, k) = (Arc::clone(&a), Arc::clone(&ran), kind.clone());
    let mut rx = a.service().rpc_rx(kind.clone());
    let serve = tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            seen.fetch_add(1, Ordering::SeqCst);
            svc.service().rpc_respond(&req, serde_json::json!({ "ran": k }).to_string().into_bytes());
        }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let within = Duration::from_secs(20);
    let payload = br#"{"stops": 3}"#.to_vec();
    let reply = match call {
        MemberCall::Direct(_) => a.service().rpc_call(id.clone(), kind.clone(), payload, within).await,
        MemberCall::Mandated(_, scope) => {
            use crate::agent::gateway_authority::{arguments_digest, possession_request, PresentedMandate};
            use crate::mandate::grant::{possession_message, SignedMandateGrant};
            use crate::mandate::{Mandate, PrincipalId, TermId};
            let (ns, name) = crate::hosted_capability_of(&kind).expect("a hosted capability's kind");
            let (operation, key) = crate::hosted_capability_call(ns, name);
            let m = Mandate {
                holder: PrincipalId::new(crate::agent::gateway_caller::node_principal(&id)).unwrap(),
                established_by: PrincipalId::new("operator:coop").unwrap(),
                purpose: scope.into(),
                scope: scope.into(),
                operations: vec![format!("{operation}:{key}")],
                epoch: 1,
                term: TermId::new("t1").unwrap(),
                valid_from_ms: 0,
                valid_until_ms: a.task_ctx.hlc.decision_now_ms() + 3_000_000,
            };
            let resource = format!("{key}@{id}");
            let arguments: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            let node_key = a.task_ctx.tls.get().expect("the node has its identity key").signing_key();
            let proof = mycelium_core::tls::sign_bytes(&node_key, &possession_message(&m, &possession_request(operation, &resource, &arguments_digest(&arguments))));
            let operator = ed25519_dalek::SigningKey::from_bytes(&[60u8; 32]);
            let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&operator, &m.canonical_bytes()).to_vec(), mandate: m };
            let presented = serde_json::to_value(PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) }).unwrap();
            a.service().rpc_call_with_mandate(id.clone(), kind.clone(), payload, &presented, &resource, within).await
        }
        MemberCall::Skill(ns, name) => {
            let me = crate::agent::gateway_caller::node_principal(&id);
            let framed = crate::agent::gateway_caller::frame_with_context_and_mandate(&a.task_ctx, &id, &me, &[], bytes::Bytes::from(payload), None, Some(&format!("skill:{ns}/{name}@{id}")))
                .expect("the envelope fits");
            crate::agent::rpc::rpc_call_framed(&a.task_ctx, id.clone(), Arc::from(kind.as_str()), framed, within).await
        }
    }
    .unwrap_or_else(|e| panic!("{tag}: no answer within 20 s: {e:?}"));
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap_or_default();
    let ran = ran.load(Ordering::SeqCst);
    assert_eq!(ran == 1, reply["ran"] == kind.as_str(), "{tag}: the answer and the receiver agree: {reply}");
    assert!(ran == 1 || reply["reason"].as_str().is_some_and(|r| !r.is_empty()), "{tag}: a call that did not run was refused with a reason: {reply}");
    serve.abort();
    a.shutdown().await;
    (ran == 1, reply)
}

const OPTIMIZE: MemberCall = MemberCall::Direct("cap.invoke/route/optimize");

/// The optimizer's edge under `declarations`, at the door of `depot-a`.
async fn depot_a_runs(tag: &str, declarations: &Path) -> bool {
    let declarations = declarations.to_path_buf();
    door_decides(tag, "depot-a", OPTIMIZE, move |_| declarations).await
}

/// **`wire-check`'s verdict for the co-op's hosted edge is the provider's decision, in both directions**
/// (`docs/operations/what-is-proven.md#wire-check-does-not-know-the-hosted-capability-shape`, closed 2026-11-18).
///
/// 1. **the fixture as it is** (the depots' rule: any node, `cap.invoke`, `cap:route/optimize`) — the check passes
///    the edge with no finding, and the call runs;
/// 2. **the fixture's rule as it stood** (`skill.invoke` on `skill:route/optimize` for `worker`, placed in the
///    provider's own unit, the one place `[authority]` would load it from) — `unauthorisable edge`, and the provider
///    refuses: the receiver sees nothing;
/// 3. **a `cap.invoke` rule naming the unit principal `worker`** — `node-decided edge`, an error, never a clean
///    pass, and the provider refuses;
/// 4. **a rule naming the calling node itself**, `node:{id}` — clean, and the call runs.
///
/// Seen failing first: on `wire_check::check` before 2026-11-18, (1) was `unauthorisable edge` for a call the
/// provider ran — the false red, this test red at its first reading; the same overlay read (2)'s declaration clean,
/// exit 0, in `the_coops_hosted_edge_is_wired_by_a_rule_its_provider_reads` — the false green. And with the hosted
/// shape's actor check planted to accept the unit principal, (3) was passed with no finding for a call the provider
/// refused.
#[tokio::test]
async fn the_coop_fixtures_hosted_edge_is_decided_at_the_provider_as_wire_check_reads_it() {
    let fixture = coop_fixture();
    let (kinds, r) = reads_optimizer(&fixture);
    assert!(kinds.is_empty() && r.exit_code() == 0, "FALSE RED: the fixture's rule is one the provider admits:\n{}", r.render_text());
    assert!(depot_a_runs("coop-now", &fixture).await, "the provider runs the call the fixture's rule admits");

    let old = coop_depots("coop-old-units", COOP_OLD_RULE);
    assert!(std::fs::read_to_string(old.join("depot-a.toml")).unwrap().contains("skill:route/optimize"), "plant: the old rule is in the provider's own unit");
    let (kinds, r) = reads_optimizer(&old);
    assert!(kinds == ["unauthorisable edge"] && r.exit_code() == 1, "FALSE GREEN: the provider refuses this edge:\n{}", r.render_text());
    assert!(!depot_a_runs("coop-old", &old).await, "the provider never consults a skill.invoke rule for a hosted component");

    let as_unit = coop_depots("coop-unit-units", "[[rule]]\nactor     = \"worker\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\n");
    let (kinds, r) = reads_optimizer(&as_unit);
    assert!(kinds == ["node-decided edge"] && r.exit_code() == 1, "PASSED: the provider decides the calling node and refuses:\n{}", r.render_text());
    assert!(!depot_a_runs("coop-unit", &as_unit).await, "the provider decides node:{{sender}}, which a rule for the unit principal does not name");

    // The caller's own node, named: the declaration is written once the node's address is known.
    let named = std::sync::Mutex::new(None);
    let ran = door_decides("coop-node", "depot-a", OPTIMIZE, |id| {
        let units = coop_depots("coop-node-units", &format!("[[rule]]\nactor     = \"node:{id}\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize@{id}\"\n"));
        *named.lock().unwrap() = Some(units.clone());
        units
    })
    .await;
    let (kinds, r) = reads_optimizer(named.lock().unwrap().as_ref().expect("the declaration was written"));
    assert!(kinds.is_empty() && ran, "a rule naming the calling node at the provider's node: the check passes it and the call runs:\n{}", r.render_text());
}

/// **A hosted edge's rule is read where its provider reads it** (#814's review, F1). `[authority]` builds a node's
/// evaluator from **its own unit's** rules, so a rule for the optimizer declared in a unit that does not host it is
/// one no provider loads.
///
/// 1. the fixture's rule moved to `auditor.toml`, none in the depots — `unauthorisable edge`, naming the unit the
///    rule sits in and the units the provider reads; the provider refuses;
/// 2. the rule in `depot-b.toml` alone — the warning `host-dependent edge`: `depot-b` runs the call and `depot-a`
///    refuses it, and which of them hosts the component is the provisioner's choice at run time.
///
/// Seen failing first: on the overlay as it was at #814's first head (rules gathered from every unit), both
/// declarations were read clean, exit 0 with no finding, while `depot-a` refused.
#[tokio::test]
async fn a_hosted_edges_rule_admits_only_from_a_unit_that_hosts_it() {
    // The depots declare vocabulary of their own — the intake's rule — and not the optimizer's: an error.
    let intake_only = "[[rule]]\nactor     = \"*\"\noperation = \"skill.invoke\"\nresource  = \"skill:depot/intake\"\n";
    let elsewhere = coop_with("coop-auditor-units", |unit, text| match unit {
        "auditor" => format!("{text}\n{COOP_RULE}"),
        u if u.starts_with("depot-") => no_rules(text, intake_only),
        _ => text,
    });
    let (kinds, r) = reads_optimizer(&elsewhere);
    assert!(kinds == ["unauthorisable edge"] && r.exit_code() == 1, "FALSE GREEN: no depot loads the auditor's rule:\n{}", r.render_text());
    let text = r.render_text();
    assert!(text.contains("`auditor`") && text.contains("depot-a|depot-b"), "the finding says where the rule is and where the provider reads it: {text}");
    assert!(!depot_a_runs("coop-auditor", &elsewhere).await, "depot-a loads its own unit's rules, and the rule is the auditor's");

    // The depots declare **nothing** (#814's confirm round, C2). Whether a depot enforces at all is its node's
    // `[authority]` table, which no unit file shows: with one it refuses the call, with none it runs it. So the
    // check does not fail the declaration; it warns `ungoverned edge`, naming the hosts and the rule they do not read.
    let outside = coop_with("coop-outside-units", |unit, text| match unit {
        "auditor" => format!("{text}\n{COOP_RULE}"),
        u if u.starts_with("depot-") => no_rules(text, ""),
        _ => text,
    });
    let (kinds, r) = reads_optimizer(&outside);
    assert!(kinds == ["ungoverned edge"] && r.exit_code() == 0, "FALSE RED: the hosts declare no vocabulary, and one without [authority] runs the call:\n{}", r.render_text());
    let text = r.render_text();
    assert!(text.contains("`auditor`") && text.contains("depot-a|depot-b") && text.contains("[authority]"), "the warning says the hosts do not read the auditor's rule: {text}");
    assert!(!depot_a_runs("coop-outside-enforcing", &outside).await, "with [authority], depot-a loads no rule for it and refuses");
    let open = outside.clone();
    assert!(door_answers("coop-outside-open", None, OPTIMIZE, move |_| open).await.0, "with no [authority] table, the same depot runs the call");

    let one_host = coop_with("coop-one-host-units", |unit, text| if unit == "depot-a" { no_rules(text, "") } else { text });
    let (kinds, r) = reads_optimizer(&one_host);
    assert!(kinds == ["host-dependent edge"] && r.exit_code() == 0, "one host admits and one refuses:\n{}", r.render_text());
    assert!(!depot_a_runs("coop-one-host-a", &one_host).await, "depot-a declares no rule for it");
    let at_b = one_host.clone();
    assert!(door_decides("coop-one-host-b", "depot-b", OPTIMIZE, move |_| at_b).await, "depot-b declares the rule");
}

/// **A rule form no provider admits a member's call under does not pass** (#814's review, F2). Each is the
/// fixture's rule in both depots — the right place — with one change, and for each the check reports the edge and
/// the provider refuses: the receiver sees nothing.
///
/// - `cancellable = true`: a hosted kind reaches the component through `rpc_rx`, which cannot stop a begun call, so a
///   cancellable rule refuses it (`door_cannot_stop`);
/// - `requires_scopes`: a member's own call carries no scopes;
/// - `actor = "node:*"`, `"node:"`: the evaluator matches an actor exactly, and neither is a node;
/// - `resource = "cap:route/optimize@"`: no node id is empty;
/// - `signed_with`: the rule applies only to a signed grant decision;
/// - `requires_facts`: the reference evaluator `[authority]` builds cannot establish a fact;
/// - a cancellable rule **before** the plain one: the evaluator decides by the first allowance that matches;
/// - `requires_mandate`, with a mandate a node holds: `mandate-dependent edge`, an error — `[authority]` entitles
///   no authority to that scope, so the provider built from the declaration refuses.
///
/// Seen failing first: on the overlay as it was at #814's first head every one of these was read clean, exit 0.
#[tokio::test]
async fn a_rule_form_no_provider_admits_a_members_call_under_is_not_passed() {
    let base = "[[rule]]\nactor     = \"*\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\n";
    let cases: [(&str, String, &str); 10] = [
        ("cancellable", format!("{base}cancellable = true\n"), "unauthorisable edge"),
        ("scopes", format!("{base}requires_scopes = [\"routing:write\"]\n"), "unauthorisable edge"),
        ("node-star", base.replace("\"*\"", "\"node:*\""), "unauthorisable edge"),
        ("node-empty", base.replace("\"*\"", "\"node:\""), "unauthorisable edge"),
        ("at-empty", base.replace("cap:route/optimize\"", "cap:route/optimize@\""), "unauthorisable edge"),
        ("signed-with", format!("{base}signed_with = [\"service\"]\n"), "unauthorisable edge"),
        ("facts", format!("{base}requires_facts = [\"budget\"]\n"), "unauthorisable edge"),
        ("shadowed", format!("{base}cancellable = true\n\n{base}"), "unauthorisable edge"),
        ("mandate", format!("{base}requires_mandate = \"routing\"\n\n[[mandate]]\nholder = \"node:10.0.0.7:7946\"\nscope = \"routing\"\noperations = [\"cap.invoke:cap:route/optimize\"]\n"), "mandate-dependent edge"),
        ("mandate-none", format!("{base}requires_mandate = \"routing\"\n"), "unauthorisable edge"),
    ];
    for (tag, rules, kind) in cases {
        let declarations = coop_depots(&format!("coop-form-{tag}-units"), &rules);
        let (kinds, r) = reads_optimizer(&declarations);
        assert!(kinds == [kind], "{tag}: FALSE GREEN (or the wrong finding): the provider refuses this call:\n{}", r.render_text());
        assert!(!depot_a_runs(&format!("coop-form-{tag}"), &declarations).await, "{tag}: the provider refuses a member's direct call under this rule");
    }
    // The order matters, and the other order runs: the plain rule first is the one that decides.
    let plain_first = coop_depots("coop-form-plain-first-units", &format!("{base}\n{base}cancellable = true\n"));
    let (kinds, r) = reads_optimizer(&plain_first);
    assert!(kinds.is_empty(), "the first matching allowance permits: {}", r.render_text());
    assert!(depot_a_runs("coop-form-plain-first", &plain_first).await, "the plain rule is reached first, and the call runs");
}

/// **A rule that names a value stands in no later rule's way** (#814's confirm round, C1). The evaluator skips a rule
/// whose `requires_values` names a value the call carries with another value, so whether such a rule matches is a
/// run-time fact. Two declarations: a rule naming `stops = 99` that is `cancellable`, or that requires a fact, and
/// then the plain rule. The call carries `stops = 3`: the provider skips the first rule and **runs** the call on
/// the second, and the check passes the edge.
///
/// Seen failing first: on the overlay at `9fed73af` the check reported `unauthorisable edge` ("never reached") for
/// both, while the provider ran the call — a false red.
#[tokio::test]
async fn a_rule_naming_a_value_does_not_shadow_the_rule_after_it() {
    let base = "[[rule]]\nactor     = \"*\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\n";
    for (tag, refusing) in [("values-cancellable", "cancellable = true\n"), ("values-facts", "requires_facts = [\"budget\"]\n")] {
        let declarations = coop_depots(&format!("coop-{tag}-units"), &format!("{base}{refusing}requires_values = {{ stops = 99 }}\n\n{base}"));
        let (kinds, r) = reads_optimizer(&declarations);
        assert!(kinds.is_empty() && r.exit_code() == 0, "{tag}: FALSE RED: the provider runs this call:\n{}", r.render_text());
        assert!(depot_a_runs(&format!("coop-{tag}"), &declarations).await, "{tag}: the call's `stops` is 3, so the first rule is not the one, and the plain rule admits it");
    }
}

/// **A mandate a calling node presents is not established by a provider built from `[authority]`** (#814's confirm
/// round, C4) — the door half of `mandate-dependent edge`. The rule is for any node and requires the scope
/// `routing`; a declared mandate, held by a node, covers it; and the call **presents** one
/// (`rpc_call_with_mandate`): held by the calling node, enumerating `cap.invoke:cap:route/optimize`, its possession
/// proof signed with the node's identity key over the operation, resource and arguments the provider derives, its
/// grant signed by an operator. The provider refuses it — `authority_not_established`, its recorded error `grant not
/// established: … UntrustedExternal`, where a call with no mandate records only `mandate not established: scope
/// routing` — and the receiver sees nothing: `[authority]` builds an execution authority that entitles grants to this node (or the fleet) and trusts
/// no external issuer (`declared_authority::plan`), so no operator's mandate in a scope of its own is ever
/// established there. The same call with no mandate is refused too (the `mandate` case of
/// `a_rule_form_no_provider_admits_a_members_call_under_is_not_passed`).
///
/// Not red on the overlay before: it adds the half that was read from the code. Its assertion on the recorded error
/// was seen failing first with the call planted to present no mandate (`MemberCall::Direct`). What it cannot show in-process is the other side —
/// a provider whose execution authority, attached in code, entitles the operator and so runs the call — because a
/// node with `[authority]` refuses an authority attached in code beside the table
/// (`an_authority_attached_in_code_beside_the_table_refuses_the_start`); that side is
/// `a_mandate_bound_at_the_mcp_door_verifies_on_a_direct_call_of_the_same_tool_and_arguments`
/// (`hosted_capability_door_tests`), for a client holder.
#[tokio::test]
async fn a_presented_mandate_is_not_established_by_a_provider_built_from_its_declaration() {
    let rules = "[[rule]]\nactor     = \"*\"\noperation = \"cap.invoke\"\nresource  = \"cap:route/optimize\"\nrequires_mandate = \"routing\"\n\n[[mandate]]\nholder = \"node:10.0.0.7:7946\"\nscope = \"routing\"\noperations = [\"cap.invoke:cap:route/optimize\"]\n";
    let declarations = coop_depots("coop-presented-units", rules);
    let (kinds, r) = reads_optimizer(&declarations);
    assert!(kinds == ["mandate-dependent edge"] && r.exit_code() == 1, "{}", r.render_text());
    let (ran, reply) = door_answers("coop-presented", Some("depot-a"), MemberCall::Mandated("cap.invoke/route/optimize", "routing"), move |_| declarations).await;
    assert!(!ran, "RAN: a provider built from [authority] established an operator's mandate: {reply}");
    // Every refusal at this door carries the reason `authority_not_established`; what tells this one apart is the
    // error the evaluator records. With no mandate it is "mandate not established: scope routing" and nothing more;
    // with the presented one, the grant was read and its issuer is not one this authority trusts.
    assert_eq!(reply["reason"], "authority_not_established", "{reply}");
    let errors = reply["data"]["errors"].to_string();
    assert!(errors.contains("grant not established") && errors.contains("UntrustedExternal"), "refused because the presented grant's issuer is untrusted, not for want of a mandate: {reply}");
}

/// **A hosted component in the `tool` namespace is decided for the calling node too** (#814's review, F3). It is
/// decided as the tool it is — `tools/call` on `tool:{name}@{node}` — and a member's direct call of it for
/// `node:{sender}`, like any hosted capability. A `tools/call` rule naming the requirer's unit principal is the
/// error `node-decided edge` and the provider refuses; the rule for any node passes and the call runs.
///
/// Seen failing first: on the overlay as it was at #814's first head the unit-principal rule was read clean, exit 0
/// with no finding — and a test of that PR asserted it.
#[tokio::test]
async fn a_hosted_tool_is_decided_for_the_calling_node_as_wire_check_reads_it() {
    let library = vec![(
        "optimizer-tool".to_string(),
        crate::wire_check::ArtifactDescription::from_toml_str("kind = \"wasm-component\"\n[provides]\nns = \"tool\"\nname = \"optimize\"\n").unwrap(),
    )];
    let declaration = |tag: &str, actor: &str| {
        let depot = format!("principal = \"depot-a\"\n[hosts]\nkinds = [\"wasm-component\"]\naccept_unsigned = true\n\n[[rule]]\nactor = \"{actor}\"\noperation = \"tools/call\"\nresource = \"tool:optimize\"\n");
        units(&scratch(tag), &[("depot-a", depot.as_str()), ("worker", "principal = \"worker\"\n[[requirement]]\nns = \"tool\"\nname = \"optimize\"\n")])
    };
    let call = MemberCall::Direct("cap.invoke/tool/optimize");

    let as_unit = declaration("tool-unit-units", "worker");
    let (kinds, r) = reads(&as_unit, &library, "worker", "tool/optimize");
    assert!(kinds == ["node-decided edge"] && r.exit_code() == 1, "PASSED: the provider decides the calling node and refuses:\n{}", r.render_text());
    let d = as_unit.clone();
    assert!(!door_decides("tool-unit", "depot-a", call, move |_| d).await, "a member's direct call of the tool is decided as node:{{sender}}");

    let any = declaration("tool-any-units", "*");
    let (kinds, r) = reads(&any, &library, "worker", "tool/optimize");
    assert!(kinds.is_empty() && r.exit_code() == 0, "{}", r.render_text());
    assert!(door_decides("tool-any", "depot-a", call, move |_| any).await, "the rule for any node admits it");
}

/// **The co-op's deployed edge, `depot/intake`, at the provider's door** (#814's review, F4). The depots declare the
/// intake as a `[[capability]]`: a skill, which a provider decides as `skill.invoke` on `skill:depot/intake@{node}`
/// for a call that names it. The fixture's rule for it is in the depots, for any node: a member's call in its own
/// envelope runs. The rule as it stood — in `auditor.toml`, for the unit principal `worker`, under a mandate
/// `worker` holds — is one no depot loads: the provider refuses.
///
/// `wire-check` reads a deployed edge by the requirer's own vocabulary, for its unit principal, from any unit's
/// rules — not as the provider decides it (`what-is-proven.md#wire-check-reads-a-deployed-edge-for-the-unit`, open).
/// So this test asserts the door, and of the check only that it says nothing it cannot know: the worker declares
/// no vocabulary, and its deployed edge is `ungoverned edge`.
#[tokio::test]
async fn the_coop_fixtures_deployed_edge_runs_under_the_rule_its_depot_declares() {
    let intake = MemberCall::Skill("depot", "intake");
    let fixture = coop_fixture();
    let (kinds, r) = reads(&fixture, &coop_library(), "worker", "depot/intake");
    assert!(kinds == ["ungoverned edge"], "the check claims no admission for the deployed edge: {}", r.render_text());
    assert!(door_decides("coop-intake-now", "depot-a", intake, move |_| fixture).await, "the depot's own rule admits a member's call that names the skill");

    let old = coop_with("coop-intake-old-units", |unit, text| match unit {
        "auditor" => format!("{text}\n[[rule]]\nactor            = \"worker\"\noperation        = \"skill.invoke\"\nresource         = \"skill:depot/intake\"\nrequires_mandate = \"routing\"\n"),
        "worker" => format!("{text}\n[[mandate]]\nholder     = \"worker\"\nscope      = \"routing\"\noperations = [\"skill.invoke:skill:depot/intake\"]\n"),
        u if u.starts_with("depot-") => no_rules(text, COOP_RULE),
        _ => text,
    });
    assert!(!door_decides("coop-intake-old", "depot-a", intake, move |_| old).await, "no depot loads the auditor's rule: the provider refuses");
}

/// **The binary's configuration path, over the scaffold** (#733's review, the scaffold): the scaffold's `node.toml`
/// with a gateway, its tokens, `[persistence]` and `[authority]` appended, loaded by `GossipConfig::load_from_file` and
/// `validate()` as `mycelium -c` loads it, then started — the call no rule permits is refused at the gateway door and
/// the one the scaffold's rule admits runs. The binary itself is started over the same file by CI's scaffold job (which
/// builds it with `compliance`); the bare binary serves no tool, so the doors' decisions are shown here.
#[tokio::test]
async fn the_scaffolds_node_toml_with_an_authority_table_loads_and_decides_through_the_config_path() {
    let dir = scratch("toml");
    let declarations = units(&dir, &[("purchaser", PURCHASER)]);
    let (port, http_port) = (alloc_port(), alloc_port());
    let node_toml = NODE_TOML
        .replace("bind_port = 7946", &format!("bind_port = {port}\nhttp_port = {http_port}"))
        .replace("auto_cert_dir = \"data/tls\"", &format!("auto_cert_dir = \"{}\"", dir.join("tls").display()));
    assert!(node_toml.contains(&format!("bind_port = {port}")) && node_toml.contains(&dir.join("tls").display().to_string()), "the scaffold's node.toml changed shape; update this test");
    let toml = format!(
        "{node_toml}\n[[gateway_named_tokens]]\nname = \"alice\"\ntoken = \"alice-secret\"\nscopes = [\"mcp:invoke\"]\n\n[persistence]\nbase_path = \"{}\"\nsync_mode = \"Flush\"\n\n[authority]\ndeclarations = \"{}\"\nunit = \"purchaser\"\njournal = \"{}\"\n",
        dir.join("persist").display(),
        declarations.display(),
        dir.join("evidence.log").display()
    );
    let path = dir.join("node.toml");
    std::fs::write(&path, toml).unwrap();
    let cfg = GossipConfig::load_from_file(&path).expect("the binary's loader reads it");
    cfg.validate().expect("and validate() admits it");
    let a = Arc::new(GossipAgent::new(NodeId::new(&cfg.bind_address, cfg.bind_port).unwrap(), cfg));
    a.start().await.expect("the node starts from its configuration file alone");
    let ran = tools(&a).await;
    assert!(mcp_call(http_port, "purchase.place", serde_json::json!({"ledger": "capital"})).await.is_err());
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    assert!(mcp_call(http_port, "purchase.place", serde_json::json!({"ledger": "petty-cash"})).await.is_ok());
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    a.shutdown().await;
}

/// **A symlinked journal is judged as the journal will open it** (#733's final confirm): the path check follows the
/// link, as `EvidenceJournal::open` does, so a journal path that is a symlink to a writable file starts — it is not
/// refused as "not a regular file". Seen failing first with the check on the link itself (`symlink_metadata`).
#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_journal_path_is_judged_by_its_target() {
    let dir = scratch("symlink");
    let target = dir.join("real-evidence.log");
    std::fs::write(&target, b"").unwrap();
    let link = dir.join("evidence-link.log");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (id, cfg) = config(&dir, alloc_port(), Some(AuthorityConfig { journal: link, ..authority(&dir, units(&dir, &[("purchaser", PURCHASER)]), "purchaser") }));
    let a = GossipAgent::new(id, cfg);
    a.start().await.expect("a symlink to a writable journal starts");
    a.shutdown().await;
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

/// The warnings logged while `f` runs on this thread: a minimal subscriber, so the test needs no logging crate.
#[cfg(feature = "a2a")]
fn warnings_during(f: impl FnOnce()) -> Vec<String> {
    use std::sync::Mutex;
    use tracing::field::{Field, Visit};
    struct Capture(Arc<Mutex<Vec<String>>>);
    struct Message(String);
    impl Visit for Message {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }
    impl tracing::Subscriber for Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool { true }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id { tracing::span::Id::from_u64(1) }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, e: &tracing::Event<'_>) {
            if *e.metadata().level() == tracing::Level::WARN {
                let mut m = Message(String::new());
                e.record(&mut m);
                self.0.lock().unwrap().push(m.0);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(Capture(Arc::clone(&seen)), f);
    let out = seen.lock().unwrap().clone();
    #[allow(clippy::let_and_return)] // the guard must drop before `seen` does
    out
}

/// **`/a2a` with `[authority]` is not reported open** (#749's review, finding 1): the table's evaluator is attached by
/// `start()` after `preflight()` has logged its warnings, so a planned declaration counts as the evaluator — what
/// `start()` attaches before the boundary, and what `mycelium doctor` sets in memory. Neither the preflight `start()`
/// runs nor the doctor warns "NO action evaluator"; without the table both do. Seen failing first: both warned.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn the_a2a_switch_with_an_authority_table_is_not_warned_open() {
    const OPEN: &str = "NO action evaluator";
    let dir = scratch("a2a-authority");
    let declarations = units(&dir, &[("purchaser", PURCHASER)]);
    let (id, mut cfg) = config(&dir, alloc_port(), Some(authority(&dir, declarations.clone(), "purchaser")));
    cfg.gateway_a2a = true;
    let a = GossipAgent::new(id.clone(), cfg.clone());
    let warned = warnings_during(|| a.preflight().expect("plans"));
    assert!(!warned.iter().any(|w| w.contains(OPEN)), "start()'s preflight with [authority]: {warned:?}");
    assert!(!a.a2a_dispatch_is_anonymous());

    let toml_path = dir.join("node.toml");
    let body = format!(
        "bind_port = {}\nhttp_port = {}\ngateway_a2a = true\ngateway_identity_issuer = \"{ISSUER}\"\n\
         [[gateway_named_tokens]]\nname = \"alice\"\ntoken = \"alice-secret\"\nscopes = [\"mcp:invoke\"]\n\
         [tls]\nauto_cert_dir = \"{tls}\"\n[persistence]\nbase_path = \"{persist}\"\n\
         [authority]\ndeclarations = \"{decl}\"\nunit = \"purchaser\"\njournal = \"{journal}\"\n",
        alloc_port(), alloc_port(),
        tls = dir.join("tls").display(), persist = dir.join("persist").display(), decl = declarations.display(),
        journal = dir.join("evidence.log").display(),
    );
    std::fs::write(&toml_path, &body).unwrap();
    let mut d = None;
    let warned = warnings_during(|| d = Some(crate::doctor::run(Some(&toml_path))));
    let d = d.unwrap();
    assert!(d.refusal.is_none(), "{:?}", d.refusal);
    assert!(!warned.iter().any(|w| w.contains(OPEN)), "doctor with [authority]: {warned:?}");

    // The control: without the table both say so.
    cfg.authority = None;
    let open = GossipAgent::new(id, cfg);
    let warned = warnings_during(|| open.preflight().expect("preflight"));
    assert!(warned.iter().any(|w| w.contains(OPEN)), "without an evaluator start()'s preflight warns: {warned:?}");
    let bare = dir.join("bare.toml");
    std::fs::write(&bare, format!("bind_port = {}\nhttp_port = {}\ngateway_a2a = true\n", alloc_port(), alloc_port())).unwrap();
    let warned = warnings_during(|| { let _ = crate::doctor::run(Some(&bare)); });
    assert!(warned.iter().any(|w| w.contains(OPEN)), "without an evaluator the doctor warns: {warned:?}");
}
