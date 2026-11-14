//! **A hosted component at an enforcing provider** (the maintainer's decision of 2026-11-14): its serve kind,
//! `cap.invoke/{ns}/{name}`, is protected work there, so a member's direct call is decided before the component
//! runs — and the MCP bridge of a `tool/{name}` component still works, **decided once, as the client**.
//!
//! One node is the gateway, the provider and the stem's runtime, with two real components installed by a
//! `Provisioner`: the calculator fixture as `tool/calculate` — bridged as the MCP tool `calculate` — and the echo
//! fixture as `text/echo`. The runtime's own invocation log counts what ran; the node's evidence journal says who
//! each decision was for.
//!
//! Twice: with the authority attached in code (`with_provider_enforcement`), and from an `[authority]` table, as
//! the published stem takes it — where the unit's bare `tool:calculate` and `cap:text/echo` are qualified to this
//! node.
//!
//! **Seen failing first**, both ways round:
//! - with the provider deciding the family and the bridge as it was (an `rpc_call` to this node), the `/mcp` call
//!   came back without the component's answer — the inner leg was decided a second time, as `node:{self}`, and
//!   refused — and the component's log stayed empty;
//! - with the bridge as it is and the family outside the provider's protected set (the code as it was), the member's
//!   direct call of `cap.invoke/tool/calculate` ran the component with no decision.

#![cfg(feature = "gateway")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mycelium::{alloc_port, AeEvidence, CapFilter, Capability, DecisionKind, GossipAgent, GossipConfig, NodeId, RecordKind};
use mycelium_wasm_host::{cap_invoke_kind, InMemorySource, InstallableCatalog, InstallableEntry, Provisioner, WasmHost};

const ECHO_COMPONENT: &[u8] = include_bytes!("fixtures/echo_component.wasm");
const CALCULATE_COMPONENT: &[u8] = include_bytes!("fixtures/calculate_tool_component.wasm");
/// The gateway client: named token `ops` under issuer `gw`.
const CLIENT: &str = "token:gw/ops";

struct Host {
    agent: Arc<GossipAgent>,
    prov: Provisioner,
    http: u16,
    journal: PathBuf,
    dir: PathBuf,
}

impl Drop for Host {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("hosted-enforcement-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// How the node gets its authority.
#[derive(Clone, Copy, PartialEq)]
enum Authority {
    /// `with_action_evaluator` + `with_provider_enforcement`, as an embedder's program attaches them.
    InCode,
    /// An `[authority]` table over a unit file, as the published stem takes it.
    Declared,
}

fn config_toml(dir: &Path, port: u16, http: u16, authority: bool) -> String {
    let mut toml = format!(
        "bind_address = \"127.0.0.1\"\nbind_port = {port}\nhttp_port = {http}\ngateway_identity_issuer = \"gw\"\n\n\
         [[gateway_named_tokens]]\nname = \"ops\"\ntoken = \"ops-secret\"\nscopes = [\"*\"]\n\n\
         [tls]\nauto_cert_dir = \"{tls}\"\n\n",
        tls = dir.join("tls").display(),
    );
    if authority {
        toml.push_str(&format!(
            "[persistence]\nbase_path = \"{data}\"\n\n[authority]\ndeclarations = \"{units}\"\nunit = \"stem-unit\"\njournal = \"{journal}\"\n",
            data = dir.join("data").display(), units = dir.join("units").display(), journal = dir.join("evidence.journal").display(),
        ));
    }
    toml
}

/// The unit's rules: the client may call the tool `calculate`; anyone may call the capability `text/echo`. Bare
/// resources — `[authority]` qualifies each to the node it is attached on.
const UNIT: &str = "principal = \"stem-unit\"\n\n\
    [[rule]]\nactor = \"token:gw/ops\"\noperation = \"tools/call\"\nresource = \"tool:calculate\"\n\n\
    [[rule]]\nactor = \"*\"\noperation = \"cap.invoke\"\nresource = \"cap:text/echo\"\n";

async fn host(how: Authority) -> Host {
    let (port, http) = (alloc_port(), alloc_port());
    let dir = std::env::temp_dir().join(format!("hosted-enforcement-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("units")).unwrap();
    std::fs::write(dir.join("units").join("stem-unit.toml"), UNIT).unwrap();
    let cfg_path = dir.join("node.toml");
    std::fs::write(&cfg_path, config_toml(&dir, port, http, how == Authority::Declared)).unwrap();
    let cfg = GossipConfig::load_from_file(&cfg_path).expect("the configuration loads");
    cfg.validate().expect("the configuration is valid");
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    let journal = dir.join("evidence.journal");
    if how == Authority::InCode {
        use mycelium::{EvidenceJournal, EvidenceProfile, ReferenceEvaluator, Rule, HOSTED_CAPABILITY_OPERATION};
        agent.with_evidence_journal(EvidenceJournal::open(&journal, EvidenceProfile::Strict).unwrap());
        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-hosted")
                .with_catalogue("cat-hosted", "1")
                .map_action("tools/call", "*")
                .map_action(HOSTED_CAPABILITY_OPERATION, "*")
                .allow(Rule::new(CLIENT, "tools/call", format!("tool:calculate@{id}")))
                .allow(Rule::new("*", HOSTED_CAPABILITY_OPERATION, format!("cap:text/echo@{id}"))),
        ));
        agent.with_provider_enforcement();
    }
    agent.start().await.expect("the node starts");

    let wasm = Arc::new(WasmHost::new().expect("engine"));
    let mut source = InMemorySource::new();
    let calculate = source.insert(CALCULATE_COMPONENT.to_vec());
    let echo = source.insert(ECHO_COMPONENT.to_vec());
    let mut catalog = InstallableCatalog::new();
    catalog.add(InstallableEntry::new(Capability::new("tool", "calculate"), calculate));
    catalog.add(InstallableEntry::new(Capability::new("text", "echo"), echo));
    let mut prov = Provisioner::new(Arc::clone(&agent), wasm, catalog, Arc::new(source), 1.0);
    prov.supervise(CapFilter::new("tool", "calculate"), 1);
    prov.supervise(CapFilter::new("text", "echo"), 1);
    let tool_key = format!("tools/calculate/{id}");
    let mut live = false;
    for _ in 0..400 {
        prov.provision_round();
        if prov.hosted_count() == 2 && prov.installing_count() == 0 && agent.kv().get(&tool_key).is_some() {
            live = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(live, "both components are hosted and the tool is bridged ({} hosted)", prov.hosted_count());
    for _ in 0..200 {
        if reqwest::Client::new().get(format!("http://127.0.0.1:{http}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Host { agent, prov, http, journal, dir }
}

impl Host {
    /// `POST /mcp` `tools/call` as the client. `Ok` is the tool's text; `Err` the status and body.
    async fn mcp_call(&self, tool: &str, args: serde_json::Value) -> Result<String, String> {
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
        let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/mcp", self.http)).bearer_auth("ops-secret").json(&body).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let reply: serde_json::Value = resp.json().await.unwrap_or_default();
        if status != 200 || reply.get("error").is_some() {
            return Err(format!("{status} {reply}"));
        }
        Ok(reply["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
    }
    /// The node's own direct call of a hosted capability: a member's `rpc_call`.
    async fn direct(&self, ns: &str, name: &str, payload: &[u8]) -> String {
        let reply = self.agent.service().rpc_call(self.agent.node_id().clone(), cap_invoke_kind(ns, name), payload.to_vec(), Duration::from_secs(20)).await.expect("a reply");
        String::from_utf8_lossy(&reply).into_owned()
    }
    /// The provider's decisions on `resource`, as `(subject, decision)`.
    fn provider_decisions(&self, resource: &str) -> Vec<(String, DecisionKind)> {
        mycelium::read_evidence_journal(&self.journal).unwrap().iter().map(|b| serde_json::from_slice::<AeEvidence>(b).unwrap())
            .filter(|e| e.enforcement_point == "provider" && e.kind == RecordKind::Decided && e.resource == resource)
            .map(|e| (e.subject, e.decision))
            .collect()
    }
    fn ran(&self) -> usize {
        self.prov.invocations().len()
    }
}

async fn the_bridge_is_decided_once_and_a_direct_call_is_decided_as_the_member(how: Authority) {
    let h = host(how).await;
    let me = h.agent.node_id().to_string();
    let node = format!("node:{me}");
    let tool = format!("tool:calculate@{me}");

    // `/mcp` → the MCP tool loop → the component. Decided at the provider once, as the client.
    let text = h.mcp_call("calculate", serde_json::json!({"expression": "330 * 1024"})).await.expect("BRIDGE BROKEN: the tool call was refused");
    assert!(text.contains("337920"), "BRIDGE BROKEN: the component's answer did not come back through /mcp: {text}");
    assert_eq!(h.ran(), 1, "the component ran once");
    assert_eq!(h.provider_decisions(&tool), vec![(CLIENT.to_string(), DecisionKind::Permit)], "DECIDED TWICE, or not as the client");

    // A member's direct call of the same component: decided as the member, and the tool is the client's alone.
    let refused = h.direct("tool", "calculate", br#"{"expression": "2 * 3"}"#).await;
    assert_eq!(h.ran(), 1, "RAN UNDECIDED: a member's direct call ran the component: {refused}");
    let reply: serde_json::Value = serde_json::from_str(&refused).unwrap_or_else(|_| panic!("a refusal, as JSON: {refused}"));
    assert!(reply["reason"].as_str().is_some_and(|r| !r.is_empty()), "{reply}");
    let decided = h.provider_decisions(&tool);
    assert_eq!(decided.len(), 2, "{decided:?}");
    assert!(decided[1].0 == node && decided[1].1 != DecisionKind::Permit, "decided as the node, and refused: {decided:?}");

    // Outside the `tool` namespace: `cap.invoke` on `cap:text/echo@{node}`, which the rule opens to anyone. The
    // payload is not JSON, as a component's may be.
    let out = h.direct("text", "echo", b"plain bytes").await;
    assert!(out.contains("plain bytes"), "the permitted capability answers: {out}");
    assert_eq!(h.ran(), 2);
    assert_eq!(h.provider_decisions(&format!("cap:text/echo@{me}")), vec![(node, DecisionKind::Permit)]);

    // And the bridge again, after the refusals: still the client's, still once per call.
    let text = h.mcp_call("calculate", serde_json::json!({"expression": "6 * 7"})).await.expect("the tool call is admitted");
    assert!(text.contains("42"), "{text}");
    assert_eq!(h.ran(), 3);
    assert_eq!(h.provider_decisions(&tool).iter().filter(|(s, d)| s == CLIENT && *d == DecisionKind::Permit).count(), 2);
    h.agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_provider_enforcement_the_bridge_is_decided_once_as_the_client_and_a_direct_call_as_the_member() {
    the_bridge_is_decided_once_and_a_direct_call_is_decided_as_the_member(Authority::InCode).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_an_authority_table_the_bridge_is_decided_once_as_the_client_and_a_direct_call_as_the_member() {
    the_bridge_is_decided_once_and_a_direct_call_is_decided_as_the_member(Authority::Declared).await;
}
