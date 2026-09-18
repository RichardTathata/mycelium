//! `federation_node` — the one binary of the two-mesh Docker suite (v3 item 2 PR 10b).
//!
//! ```text
//! make test-federation          # docker compose: two meshes, a real network severance
//! ```
//!
//! The in-process choreography (`lib_tests.rs` → `the_release_gates_choreography_over_the_transport`)
//! meets item 2's release gate with one caveat the record states: the meshes share an address space
//! and "severing every link" is a shut-down gateway. This binary exists so the same choreography runs
//! with **process isolation** (one container per node) and a **real network severance** (`docker
//! network disconnect`). It decides nothing new: every role below is the library's own API driven by
//! environment variables, plus a small *admin* surface the runner script asserts against.
//!
//! # Roles (`FED_ROLE`)
//!
//! | Role | What it is | Extra env |
//! |---|---|---|
//! | `member` | a node of a domain under the enforced profile; advertises `FED_EXPORTS` as skills and answers with the caller principal | — |
//! | `gateway` | a member that also runs the A2A + federation edges | `FED_GRANTS`, `FED_TRUST`, `FED_SIGNING_SEED` |
//! | `probe` | the *consumer* side: one `FederationClient` behind a control API (not a mesh node) | `FED_ORIGIN`, `FED_PRINCIPAL`, `FED_PARTNER`, `FED_PARTNER_KEY`, `FED_GATEWAYS`, `FED_CONTROL_PORT` |
//! | `keys` | prints the public half of `FED_SIGNING_SEED` and exits — the suite's keys are derived, never transcribed by hand (`make federation-keys`) | `FED_SIGNING_SEED` |
//!
//! Common to `member`/`gateway`: `FED_DOMAIN`, `MYCELIUM_HOSTNAME` (this node's IP **on its mesh
//! network** — a gateway sits on two networks, so it is passed explicitly), `MYCELIUM_PORT`,
//! `MYCELIUM_HTTP_PORT`, `MYCELIUM_PEERS` (`host:port,…`), `FED_CA_DIR` (one shared volume per mesh
//! → one auto-generated CA per mesh), `FED_EXPORTS` (`ns/name,…`).
//!
//! # The admin surface (test image only)
//!
//! Mounted with `with_http_routes` **outside** `/gateway/`, so it is unauthenticated by construction
//! (the library's rule for merged routers). It exists for a runner on a private test network and is
//! not something to ship: `GET /fed-admin/tables` (the three tables the gate is proved from),
//! `POST /fed-admin/propose`, `GET /fed-admin/committed/{slot}`, `POST|GET /fed-admin/kv`,
//! `POST /fed-admin/policy` and `POST /fed-admin/revoke` (gateway only).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use mycelium::federation::{
    call::CallPolicy,
    client::{ClientError, FederationClient, GatewayEndpoint},
    edge::FederationEdge,
    gateway::Repeatability,
    DomainId, DomainPolicy, TrustBundle,
};
use mycelium::{ConsensusConfig, ConsensusResult, DomainProfile, GossipAgent, GossipConfig, NodeId, TlsConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_or(key: &str, default: &str) -> String {
    env(key).unwrap_or_else(|| default.to_string())
}

fn must(key: &str) -> String {
    env(key).unwrap_or_else(|| panic!("{key} is required for FED_ROLE={}", env_or("FED_ROLE", "?")))
}

fn hex32(s: &str) -> [u8; 32] {
    let s = s.trim();
    assert_eq!(s.len(), 64, "expected 64 hex chars, got {}", s.len());
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn domain(s: &str) -> DomainId {
    DomainId::new(s).unwrap_or_else(|e| panic!("bad domain id {s:?}: {e:?}"))
}

async fn resolve_ip(host: &str) -> Result<String, String> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(host.to_string());
    }
    let addrs = tokio::net::lookup_host((host, 0)).await.map_err(|e| e.to_string())?;
    addrs
        .filter_map(|a| match a.ip() {
            std::net::IpAddr::V4(v4) => Some(v4.to_string()),
            _ => None,
        })
        .next()
        .ok_or_else(|| format!("no IPv4 address for {host}"))
}

/// `host:port,…` → node ids, with DNS retries (Docker's resolver may lag a peer's start).
async fn resolve_peers(list: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    for entry in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((host, port)) = entry.rsplit_once(':') else { warn!(peer = entry, "no port"); continue };
        let Ok(port) = port.parse::<u16>() else { warn!(peer = entry, "bad port"); continue };
        for attempt in 0u32..10 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            match resolve_ip(host).await {
                Ok(ip) => {
                    if let Ok(id) = NodeId::new(&ip, port) {
                        out.push(id);
                    }
                    break;
                }
                Err(e) if attempt == 9 => warn!(peer = entry, "unresolved after retries: {e}"),
                Err(_) => {}
            }
        }
    }
    out
}

// ── Member / gateway ─────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Admin {
    agent: Arc<GossipAgent>,
    edge: Option<Arc<FederationEdge>>,
}

fn admin_router(state: Admin) -> Router {
    Router::new()
        .route("/fed-admin/tables", get(tables))
        .route("/fed-admin/propose", post(propose))
        .route("/fed-admin/committed/{*slot}", get(committed))
        .route("/fed-admin/kv", post(kv_put))
        .route("/fed-admin/kv/{*key}", get(kv_get))
        .route("/fed-admin/policy", post(set_policy))
        .route("/fed-admin/revoke", post(revoke))
        .with_state(state)
}

/// The three tables the gate is proved from: membership, the native + consensus namespaces (keys
/// and values), and the connection table.
async fn tables(State(s): State<Admin>) -> Json<Value> {
    let a = &s.agent;
    let mut entries = Vec::new();
    for prefix in ["cap/", "grp/", "sys/", "consensus/"] {
        for (k, v) in a.kv().scan_prefix(prefix) {
            entries.push(json!({ "key": k.to_string(), "value": String::from_utf8_lossy(&v) }));
        }
    }
    Json(json!({
        "node_id": a.node_id().to_string(),
        "peers": a.peers().iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        "connected_peers": a.connected_peers().iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        "entries": entries,
    }))
}

#[derive(Deserialize)]
struct ProposeReq {
    slot: String,
    value: String,
}

async fn propose(State(s): State<Admin>, Json(req): Json<ProposeReq>) -> Json<Value> {
    let outcome = s
        .agent
        .consensus()
        .cluster_propose(&req.slot, Bytes::from(req.value.into_bytes()), ConsensusConfig::default())
        .await;
    let committed = matches!(outcome, ConsensusResult::Committed { .. });
    Json(json!({ "committed": committed, "detail": format!("{outcome:?}") }))
}

async fn committed(State(s): State<Admin>, Path(slot): Path<String>) -> Json<Value> {
    let v = s.agent.consensus().consensus_get(&slot).map(|b| String::from_utf8_lossy(&b).to_string());
    Json(json!({ "slot": slot, "value": v }))
}

#[derive(Deserialize)]
struct KvReq {
    key: String,
    value: String,
}

async fn kv_put(State(s): State<Admin>, Json(req): Json<KvReq>) -> Json<Value> {
    let _ = s.agent.kv().set(req.key.as_str(), Bytes::from(req.value.into_bytes()));
    Json(json!({ "ok": true }))
}

async fn kv_get(State(s): State<Admin>, Path(key): Path<String>) -> Json<Value> {
    let v = s.agent.kv().get(&key).map(|b| String::from_utf8_lossy(&b).to_string());
    Json(json!({ "key": key, "value": v }))
}

#[derive(Deserialize)]
struct PolicyReq {
    revision: u64,
    grants: Vec<(String, String)>,
}

async fn set_policy(State(s): State<Admin>, Json(req): Json<PolicyReq>) -> Response {
    let Some(edge) = &s.edge else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not a gateway" }))).into_response();
    };
    let grants = req.grants.into_iter().map(|(p, e)| (domain(&p), e)).collect();
    edge.set_policy(DomainPolicy { domain: edge.domain().clone(), revision: req.revision, grants });
    Json(json!({ "ok": true, "policy_revision": edge.policy_revision() })).into_response()
}

#[derive(Deserialize)]
struct RevokeReq {
    partner: String,
}

async fn revoke(State(s): State<Admin>, Json(req): Json<RevokeReq>) -> Response {
    let Some(edge) = &s.edge else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not a gateway" }))).into_response();
    };
    edge.revoke(&domain(&req.partner));
    Json(json!({ "ok": true })).into_response()
}

fn parse_grants(spec: &str) -> Vec<(DomainId, String)> {
    // `partner=export;partner=export`
    spec.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (p, e) = s.split_once('=').unwrap_or_else(|| panic!("FED_GRANTS entry {s:?}: expected partner=export"));
            (domain(p.trim()), e.trim().to_string())
        })
        .collect()
}

fn parse_trust(spec: &str) -> TrustBundle {
    // `partner=hex32;partner=hex32`
    TrustBundle::trusting(spec.split(';').map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        let (p, k) = s.split_once('=').unwrap_or_else(|| panic!("FED_TRUST entry {s:?}: expected partner=hex"));
        (domain(p.trim()), hex32(k))
    }))
}

async fn run_node(role: &str) {
    let dom = must("FED_DOMAIN");
    let my_ip = must("MYCELIUM_HOSTNAME");
    let port: u16 = env_or("MYCELIUM_PORT", "57000").parse().expect("MYCELIUM_PORT");
    let http_port: u16 = env_or("MYCELIUM_HTTP_PORT", "8300").parse().expect("MYCELIUM_HTTP_PORT");
    let peers = resolve_peers(&env_or("MYCELIUM_PEERS", "")).await;
    let exports: Vec<String> =
        env_or("FED_EXPORTS", "").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();

    let mut cfg = GossipConfig::default();
    cfg.cluster_name = Some(dom.clone());
    cfg.bind_address = my_ip.clone();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "0.0.0.0".to_string();
    cfg.bootstrap_peers = peers;
    // Fast pings, because peer registration happens on Ping receipt and the suite's formation
    // polls are bounded. `health_check_interval_secs` stays above `reconnect_backoff_secs + 2`:
    // below that, config `validate()` warns that a peer can be evicted mid-backoff and never
    // reconnect — a warning on every node start is noise that hides a real one later.
    cfg.reconnect_backoff_secs = 1;
    cfg.health_check_interval_secs = 4;
    cfg.health_check_max_jitter_ms = 200;
    // The enforced domain profile (record §9): TLS under this mesh's CA, SWIM off.
    cfg.domain_profile = DomainProfile::Enforced;
    cfg.swim_failure_detector = false;
    cfg.tls = Some(TlsConfig { auto_cert_dir: must("FED_CA_DIR").into(), ..TlsConfig::default() });

    let id = NodeId::new(&my_ip, port).expect("self node id");
    let mut agent = GossipAgent::new(id, cfg);
    let mut edge = None;
    if role == "gateway" {
        let signing = ed25519_dalek::SigningKey::from_bytes(&hex32(&must("FED_SIGNING_SEED")));
        let e = Arc::new(
            FederationEdge::new(
                domain(&dom),
                exports.clone(),
                DomainPolicy { domain: domain(&dom), revision: 1, grants: parse_grants(&env_or("FED_GRANTS", "")) },
                parse_trust(&env_or("FED_TRUST", "")),
                CallPolicy::default(),
            )
            .with_signing_key(signing),
        );
        agent = agent.with_a2a().with_federation_edge(Arc::clone(&e));
        edge = Some(e);
    }
    let agent = Arc::new(agent);
    agent.with_http_routes(admin_router(Admin { agent: Arc::clone(&agent), edge }));
    agent.start().await.expect("start");
    info!(role, domain = %dom, node = %agent.node_id(), "up under the enforced profile");

    // Every node listens for consensus (the gate's consensus-state leg needs rounds to commit).
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());

    // Members (and gateways, harmlessly) provide the exported skills, answering with the principal
    // they were told — the check that origin survives the hop.
    let mut regs = Vec::new();
    if role == "member" {
        for export in &exports {
            let (ns, name) = export.split_once('/').unwrap_or((export.as_str(), "default"));
            regs.push(agent.capabilities().advertise_capability(
                mycelium::Capability::new(ns, name),
                Duration::from_secs(5),
            ));
        }
        let a = Arc::clone(&agent);
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let reply = match a.request_principal(&req) {
                    Ok(p) => p.name(),
                    Err(e) => format!("refused:{e}"),
                };
                a.service().rpc_respond(&req, reply.into_bytes());
            }
        });
    }

    tokio::signal::ctrl_c().await.ok();
    drop(regs);
    agent.shutdown().await;
}

// ── Probe (the consumer side) ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CallReq {
    export: String,
    #[serde(default = "default_text")]
    text: String,
    #[serde(default)]
    repeatable: bool,
}

fn default_text() -> String {
    "?".to_string()
}

#[derive(Deserialize)]
struct RetireReq {
    id: String,
}

fn client_error(e: &ClientError) -> Value {
    let kind = match e {
        ClientError::Link(_) => "link",
        ClientError::Resolve(_) => "resolve",
        ClientError::Outcome(_) => "outcome",
        ClientError::Refused { .. } => "refused",
        ClientError::Transport(_) => "transport",
        ClientError::Catalogue(_) => "catalogue",
    };
    json!({ "error": kind, "detail": e.to_string(), "debug": format!("{e:?}") })
}

async fn probe_connect(State(c): State<Arc<FederationClient>>) -> Response {
    match c.connect().await {
        Ok(exports) => Json(json!({ "exports": exports, "link": format!("{:?}", c.link_state()) })).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(client_error(&e))).into_response(),
    }
}

async fn probe_call(State(c): State<Arc<FederationClient>>, Json(req): Json<CallReq>) -> Response {
    let rep = if req.repeatable { Repeatability::Repeatable } else { Repeatability::AtMostOnce };
    match c.call(&req.export, &req.text, rep).await {
        Ok(reply) => Json(json!({ "reply": reply })).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(client_error(&e))).into_response(),
    }
}

async fn probe_retire(State(c): State<Arc<FederationClient>>, Json(req): Json<RetireReq>) -> Json<Value> {
    Json(json!({ "retired": c.retire_gateway(&req.id) }))
}

async fn probe_link(State(c): State<Arc<FederationClient>>) -> Json<Value> {
    Json(json!({ "link": format!("{:?}", c.link_state()), "last_catalogue": c.last_catalogue() }))
}

async fn run_probe() {
    let origin = domain(&must("FED_ORIGIN"));
    let partner = domain(&must("FED_PARTNER"));
    let signing = ed25519_dalek::SigningKey::from_bytes(&hex32(&must("FED_SIGNING_SEED")));
    let endpoints: Vec<GatewayEndpoint> = must("FED_GATEWAYS")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (id, url) = s.split_once('=').unwrap_or_else(|| panic!("FED_GATEWAYS entry {s:?}: expected id=url"));
            GatewayEndpoint { id: id.trim().to_string(), base_url: url.trim().trim_end_matches('/').to_string() }
        })
        .collect();
    let mut client = FederationClient::new(
        origin,
        env_or("FED_PRINCIPAL", "svc/billing"),
        signing,
        partner,
        endpoints,
        env_or("FED_SLOTS", "2").parse().expect("FED_SLOTS"),
        Duration::from_secs(env_or("FED_FRESHNESS_SECS", "60").parse().expect("FED_FRESHNESS_SECS")),
    );
    if let Some(k) = env("FED_PARTNER_KEY") {
        client = client.with_partner_key(hex32(&k));
    }
    let client = Arc::new(client);
    let app = Router::new()
        .route("/ready", get(|| async { "ok" }))
        .route("/connect", post(probe_connect))
        .route("/call", post(probe_call))
        .route("/retire", post(probe_retire))
        .route("/link", get(probe_link))
        .with_state(client);
    let port: u16 = env_or("FED_CONTROL_PORT", "8400").parse().expect("FED_CONTROL_PORT");
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind control port");
    info!(port, "probe control API up");
    axum::serve(listener, app).await.expect("serve");
}

/// Print the public half of `FED_SIGNING_SEED`, hex. The compose file needs each domain's public
/// key (alpha's to verify catalogues, beta's for alpha's trust bundle), and deriving it beats
/// transcribing it: a wrong byte here would fail as `BadSignature`, which reads like a defect in
/// the thing under test rather than a typo in its fixture.
fn print_public_key() {
    let seed = hex32(&must("FED_SIGNING_SEED"));
    let vk = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    println!("{}", vk.iter().map(|b| format!("{b:02x}")).collect::<String>());
}

#[tokio::main]
async fn main() {
    let role = env_or("FED_ROLE", "member");
    if role == "keys" {
        print_public_key();
        return;
    }
    tracing_subscriber::fmt().with_env_filter(env_or("RUST_LOG", "info")).init();
    match role.as_str() {
        "member" | "gateway" => run_node(&role).await,
        "probe" => run_probe().await,
        other => panic!("unknown FED_ROLE {other:?}: member | gateway | probe | keys"),
    }
}
