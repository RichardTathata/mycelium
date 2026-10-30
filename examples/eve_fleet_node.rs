//! **A node of the Eve fleet demonstration** (adopter-surface Y6). The driver, `examples/eve_weather/run_demo.mjs`,
//! starts three nodes from this binary, loads Eve's own tiny example — the weather assistant, its `agent/` directory
//! unmodified and pinned by version — with `mycelium-ts`'s loader, and calls its tool through `/mcp`; the example's
//! README says what the run shows and what it does not.
//!
//! Every node is a fleet member with a gateway, the action evaluator, an evidence journal and named tokens. The node
//! given `EVE_MCP_URL` bridges the loader's MCP server (`connect_mcp_server`), so the Eve tool is served from it; the
//! others reach the tool by discovery, as any member reaches a tool another node serves.
//!
//! ```bash
//! cargo build --example eve_fleet_node --features compliance,llm
//! node examples/eve_weather/run_demo.mjs
//! ```
//!
//! The policy, the same on every node: `get_weather` is allowed to the forecaster and to nobody else — a principal
//! the policy does not name is denied, and the denial is in the evidence journal of the gateway that decided it.
//! Helpers the driver runs: `ca-init <dir>` (the fleet CA).

use std::sync::Arc;
use std::time::Duration;

use mycelium::config::{GatewayNamedToken, PersistenceConfig, TlsConfig};
use mycelium::{EvidenceJournal, EvidenceProfile, GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};

/// The principals the gateway names its token holders by: `token:{ISSUER}/{name}`.
const ISSUER: &str = "fleet";
/// The Eve tool the demonstration calls, and the one principal the policy allows it to.
const TOOL: &str = "get_weather";
const ALLOWED: &str = "forecaster";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("ca-init") {
        return ca_init(&args[2]).await;
    }
    let shutdown = mycelium::shutdown::ShutdownSignal::install().expect("signal handlers");
    tokio::select! {
        _ = node() => {}
        _ = shutdown.wait() => {}
    }
}

/// Generate the fleet CA into `dir` (`ca-cert.pem`, `ca-key.pem`), by starting a node against it once.
async fn ca_init(dir: &str) {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.into(), ..Default::default() });
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), cfg);
    agent.start().await.expect("start");
    agent.shutdown().await;
    for f in ["ca-cert.pem", "ca-key.pem"] {
        assert!(std::path::Path::new(dir).join(f).exists(), "{f} was not written");
    }
}

async fn node() {
    let name = env("NAME");
    let data = std::path::PathBuf::from(env("DATA_DIR"));
    let cert_dir = data.join("tls");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let ca_dir = std::path::PathBuf::from(env("CA_DIR"));
    for f in ["ca-cert.pem", "ca-key.pem"] {
        std::fs::copy(ca_dir.join(f), cert_dir.join(f)).unwrap_or_else(|e| panic!("copy {f} from {ca_dir:?}: {e}"));
    }

    let port: u16 = env("PORT").parse().expect("PORT");
    let me = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.reconnect_backoff_secs = 1;
    cfg.health_check_interval_secs = 1;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir, ..Default::default() });
    cfg.bootstrap_peers = std::env::var("PEERS")
        .unwrap_or_default()
        .split(',')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<NodeId>().expect("PEERS is host:port,…"))
        .collect();
    cfg.persistence = Some(PersistenceConfig {
        base_path: data.join("kv"),
        sync_mode: Default::default(),
        snapshot_wal_threshold: 10_000,
        snapshot_interval_secs: 300,
        on_unreadable: Default::default(),
    });
    cfg.http_port = Some(env("HTTP_PORT").parse().expect("HTTP_PORT"));
    cfg.gateway_identity_issuer = Some(ISSUER.into());
    // `TOKENS` is `name:token:scope+scope,…`: the loader declares with `cap:write` and `llm:write`, the callers call
    // with `mcp:invoke`. A scope admits the route; the policy below decides the call.
    cfg.gateway_named_tokens = env("TOKENS")
        .split(',')
        .map(|t| {
            let mut it = t.splitn(3, ':');
            let (name, token, scopes) = (it.next().unwrap(), it.next().expect("token"), it.next().expect("scopes"));
            GatewayNamedToken { name: name.into(), token: token.into(), scopes: scopes.split('+').map(String::from).collect() }
        })
        .collect();
    let agent = Arc::new(GossipAgent::new(me.clone(), cfg));

    // The tool is served from the node `PROVIDER` names; its resource at every door is `tool:{name}@{provider}`.
    let provider = env("PROVIDER");
    let resource = format!("tool:{TOOL}@{provider}");
    let policy = ReferenceEvaluator::new("rev-eve-weather")
        .with_catalogue("cat-eve-weather", "1")
        .map_action("tools/call", resource.clone())
        .allow(Rule::new(format!("token:{ISSUER}/{ALLOWED}"), "tools/call", resource));
    agent.with_action_evaluator(Arc::new(policy));
    agent.with_evidence_journal(EvidenceJournal::open(data.join("evidence.log"), EvidenceProfile::Strict).expect("the evidence journal"));

    agent.start().await.expect("start");

    // The provider bridges the loader's MCP server; it may start after the node, so the bridge is retried.
    let _bridge = match std::env::var("EVE_MCP_URL") {
        Ok(url) => {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            loop {
                match agent.mcp().connect_mcp_server(url.clone()).await {
                    Ok(handle) => break Some(handle),
                    Err(e) => {
                        assert!(tokio::time::Instant::now() < deadline, "{name} never bridged {url}: {e}");
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
            }
        }
        Err(_) => None,
    };

    // Ready when a call can dispatch from here: the tool advertised and the provider's caller-context marker seen
    // (the gateway refuses to route to a node whose marker it cannot see).
    for key in [format!("tools/{TOOL}/{provider}"), format!("sys/caller-context/{provider}")] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while agent.kv().get(&key).is_none() {
            assert!(tokio::time::Instant::now() < deadline, "{key} never reached {name}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    println!("ready {name} gossip={port} http={}", env("HTTP_PORT"));
    std::future::pending::<()>().await;
}
