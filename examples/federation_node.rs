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
//! | `pki` | the off-node issuer: mints a domain's CA where only it holds the key, issues each node's certificate, writes the gateways' SPKI pins, exits | `FED_CA_DIR`, `FED_CERT_OUT`, `FED_NODES`, `FED_PIN_NODES`, `FED_PIN_OUT` |
//! | `keys` | prints the public half of `FED_SIGNING_SEED` and exits — the suite's keys are derived, never transcribed by hand (`make federation-keys`) | `FED_SIGNING_SEED` |
//!
//! Common to `member`/`gateway`: `FED_DOMAIN`, `MYCELIUM_HOSTNAME` (this node's IP **on its mesh
//! network** — a gateway sits on two networks, so it is passed explicitly), `MYCELIUM_PORT`,
//! `MYCELIUM_HTTP_PORT`, `MYCELIUM_PEERS` (`host:port,…`), `FED_EXPORTS` (`ns/name,…`), and the
//! certificate: `FED_CERT_DIR` (issued off-node by `pki`) or `FED_CA_DIR` (the CA generated on-node —
//! the development posture, which the secure profile refuses).
//!
//! `FED_PROFILE` (default `secure-multi-domain`, adopter-surface Y7) — under it the node carries what
//! the profile requires: `FED_GATEWAY_TOKEN` closes an HTTPS gateway, `FED_EGRESS_HOSTS` is the
//! allow-list, `FED_DATA_DIR` (default `/data`) holds persistence, the evidence journal and the
//! revocation epochs, `FED_PERMIT_ACTOR` is the one principal the evaluator permits to invoke an
//! export, at the providers `FED_PERMIT_PROVIDERS` names. `dev` is the pre-Y7 node: an open plain-HTTP gateway and nothing else.
//!
//! # The admin surface (test image only)
//!
//! Served by its own plain-HTTP listener on **this node's mesh address only**, port
//! `FED_ADMIN_PORT` (default 8301) — never on the gateway's listener, which a gateway exposes on the
//! `edge` network to the partner. It is unauthenticated: it exists for a runner on the mesh's private
//! test network and is not something to ship (leg 0 shows the partner side cannot reach it): `GET /fed-admin/tables` (the three tables the gate is proved from),
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

/// A configuration this node will not run under: said by name on stderr, exit 3 — the code the
/// runner reads as *refused at start*, apart from a panic's 101.
fn refuse(why: &str) -> ! {
    eprintln!("federation-node: refused: {why}");
    std::process::exit(3);
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
    /// An exclusive outcome (a lock, a leader): under `consensus_require_electorate` one proposed to
    /// the whole cluster rather than an electorate group is refused (`cons.safety_profile`).
    #[serde(default)]
    safety_sensitive: bool,
}

async fn propose(State(s): State<Admin>, Json(req): Json<ProposeReq>) -> Json<Value> {
    let outcome = s
        .agent
        .consensus()
        .cluster_propose(
            &req.slot,
            Bytes::from(req.value.into_bytes()),
            ConsensusConfig { safety_sensitive: req.safety_sensitive, ..ConsensusConfig::default() },
        )
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

/// The authority half of `secure-multi-domain` (the single-domain set's `ae.*`, `prov.enforcement`,
/// `a2a.admission` and `authz.*`): an action evaluator at the gateway and at every provider, the
/// evidence journal its decisions are recorded in before dispatch, and an execution authority whose
/// revocation epochs survive a restart. The evaluator permits exactly one actor — the partner's
/// principal, `FED_PERMIT_ACTOR` — to invoke the exports `FED_PERMIT_EXPORTS` names (default: every
/// `FED_EXPORTS`) at the providers `FED_PERMIT_PROVIDERS` names; everything else is
/// `Indeterminate`, which the seam refuses. Which exports the partner may call is the edge's signed
/// catalogue; this is the second door, at the provider, that asks again.
fn attach_authority(agent: &GossipAgent, exports: &[String]) {
    use mycelium::knowledge::{issuer::TrustedExternalIssuers, IssuerId};
    use mycelium::mandate::authority::{ClockModel, DurableEpochs, ExecutionGate, FreshnessPolicy, ResourceTier};
    use mycelium::mandate::grant::{EntitlementTable, GrantVerifier};
    use mycelium::mandate::{PrincipalId, ResourceAuthority};
    use mycelium::{EvidenceJournal, EvidenceProfile, ExecutionAuthority, ReferenceEvaluator, Rule};

    let data = std::path::PathBuf::from(env_or("FED_DATA_DIR", "/data"));
    let actor = env_or("FED_PERMIT_ACTOR", "");
    if !exports.is_empty() && actor.is_empty() {
        refuse("FED_EXPORTS is set but FED_PERMIT_ACTOR is not: under the secure profile every export needs the principal the evaluator permits");
    }
    // Both doors name the skill at the provider that serves it (`skill:{ns}/{name}@{provider}`): the
    // gateway the member it routes to, the provider itself. `FED_PERMIT_PROVIDERS` is that set.
    let providers: Vec<String> =
        env_or("FED_PERMIT_PROVIDERS", "").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    if !actor.is_empty() && providers.is_empty() {
        refuse("FED_PERMIT_ACTOR is set but FED_PERMIT_PROVIDERS is not: the evaluator names a skill at the provider that serves it");
    }
    let mut evaluator = ReferenceEvaluator::new("fed-suite-1").with_catalogue("fed-suite", "1");
    let permitted: Vec<String> = match env("FED_PERMIT_EXPORTS") {
        Some(list) => list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        None => exports.to_vec(),
    };
    for export in permitted.iter().filter(|_| !actor.is_empty()) {
        for provider in &providers {
            let resource = format!("skill:{export}@{provider}");
            evaluator = evaluator.map_action("skill.invoke", resource.clone()).allow(Rule::new(actor.clone(), "skill.invoke", resource));
        }
    }
    // The journal first, so the evaluator's first decision has somewhere to be recorded.
    agent.with_evidence_journal(EvidenceJournal::open(data.join("evidence"), EvidenceProfile::Strict).expect("evidence journal"));
    agent.with_action_evaluator(Arc::new(evaluator));
    agent.with_provider_enforcement();
    // No mandate is presented in this suite, so the gate decides nothing here; it is attached so a
    // presented one would be verified at this node, and its epochs kept on disk.
    let mut external = TrustedExternalIssuers::new();
    external
        .trust(
            IssuerId::new("operator:fed-suite").expect("issuer id"),
            ed25519_dalek::SigningKey::from_bytes(&[51u8; 32]).verifying_key().to_bytes(),
        )
        .expect("trust the suite's operator");
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("fed-suite", PrincipalId::new("operator:fed-suite").expect("principal"));
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("fed-suite", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .expect("execution gate");
    let authority = ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external);
    authority.with_durable_epochs(DurableEpochs::open(data.join("epochs")).expect("durable epochs"));
    agent.with_execution_authority(Arc::new(authority));
}

async fn run_node(role: &str) {
    let dom = must("FED_DOMAIN");
    let my_ip = must("MYCELIUM_HOSTNAME");
    let port: u16 = env_or("MYCELIUM_PORT", "57000").parse().expect("MYCELIUM_PORT");
    let http_port: u16 = env_or("MYCELIUM_HTTP_PORT", "8300").parse().expect("MYCELIUM_HTTP_PORT");
    let peers = resolve_peers(&env_or("MYCELIUM_PEERS", "")).await;
    let exports: Vec<String> =
        env_or("FED_EXPORTS", "").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();

    let profile = env_or("FED_PROFILE", "secure-multi-domain");
    let secure = profile != "dev";
    let id = NodeId::new(&my_ip, port).expect("self node id");

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
    cfg.tls = Some(match env("FED_CERT_DIR") {
        // A certificate issued off-node by the `pki` role: this directory holds the node's
        // certificate and key and `ca-cert.pem`, never the CA's private key (`id.ca_key_off_node`).
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            let sanitized = id.as_str().replace([':', '.'], "_");
            TlsConfig {
                cert_pem: Some(dir.join(format!("{sanitized}.cert.pem"))),
                key_pem: Some(dir.join(format!("{sanitized}.key.pem"))),
                auto_cert_dir: dir,
                ca_cert_pem: None,
            }
        }
        // The development posture: the CA generated in (or read from) a directory this node holds,
        // private key included. The admission plant runs this way, and so does the profile plant —
        // where `start()` must refuse it by name.
        None => TlsConfig { auto_cert_dir: must("FED_CA_DIR").into(), ..TlsConfig::default() },
    });
    if secure {
        // `secure-multi-domain` (adopter-surface Y7): the single-domain twenty and the federation
        // family. A closed HTTPS gateway (the node identity's certificate), identity proofs, an
        // egress allow-list, durable persistence, and exclusive outcomes only from an electorate.
        cfg.profile = Some(profile.clone());
        cfg.gateway_auth_token = Some(must("FED_GATEWAY_TOKEN"));
        cfg.gateway_tls = Some(mycelium::GatewayTlsConfig::default());
        cfg.require_identity_proofs = true;
        cfg.egress.allow_hosts = env_or("FED_EGRESS_HOSTS", "")
            .split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
        cfg.persistence = Some(mycelium::PersistenceConfig {
            base_path: std::path::PathBuf::from(env_or("FED_DATA_DIR", "/data")).join("kv"),
            sync_mode: mycelium::SyncMode::Flush,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600,
            on_unreadable: Default::default(),
        });
        cfg.consensus_require_electorate = true;
    } else {
        // No gateway credential: the suite's driver and the partner mesh reach these routes over the
        // compose project's private bridge network, which is the boundary here. Without one or this
        // opt-in, `start()` refuses to serve an open gateway on a non-loopback address (plan
        // `post-360-hardening.md` row P1).
        cfg.gateway_allow_unauthenticated = true;
    }

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
                // Bodies bound into the credential and a per-partner slot count (`fed.body_bound`,
                // `fed.partner_slots`); the dev posture keeps the defaults.
                if secure {
                    CallPolicy { require_body_binding: true, max_in_flight_per_partner: 4, ..CallPolicy::default() }
                } else {
                    CallPolicy::default()
                },
            )
            .with_signing_key(signing),
        );
        agent = agent.with_a2a().with_federation_edge(Arc::clone(&e));
        edge = Some(e);
    }
    if secure {
        attach_authority(&agent, &exports);
    }
    let agent = Arc::new(agent);
    // The admin surface on the mesh address only (see the module doc): the partner, on `edge`,
    // reaches the gateway's listener and never this one.
    let admin_port: u16 = env_or("FED_ADMIN_PORT", "8301").parse().expect("FED_ADMIN_PORT");
    let admin_listener = match tokio::net::TcpListener::bind((my_ip.as_str(), admin_port)).await {
        Ok(l) => l,
        Err(e) => refuse(&format!("cannot bind the admin surface on {my_ip}:{admin_port} (the mesh address, FED_ADMIN_PORT): {e}")),
    };
    let admin = admin_router(Admin { agent: Arc::clone(&agent), edge });
    tokio::spawn(async move { axum::serve(admin_listener, admin).await.expect("serve admin") });
    // SIGINT or SIGTERM (`docker stop`), installed before the bind; a second signal exits at once.
    let shutdown = mycelium::shutdown::ShutdownSignal::install().expect("signal handlers");
    if let Err(e) = agent.start().await {
        // The profile plant reads this line: a refusal names every unmet guarantee.
        eprintln!("federation-node: start refused under profile {profile}: {e}");
        std::process::exit(3);
    }
    info!(role, domain = %dom, node = %agent.node_id(), profile = %profile, "up under the enforced domain profile");

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

    shutdown.wait().await.ok();
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
        ClientError::Tls(_) => "tls",
        // `ClientError` is `#[non_exhaustive]`, and this arm fails **closed**: an unrecognised
        // refusal is a refusal, never a success. Naming it `unknown-refusal` rather than folding it
        // into an existing label keeps a harness reading this JSON from reporting the wrong cause.
        _ => "unknown-refusal",
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

/// `POST /call_unbound` — one federated call whose credential carries **no body digest**, sent
/// straight to one gateway (`{"gateway":"https://…","export":"…"}`), pinned like the client. A
/// `FederationClient` always binds its body, so this is the only way the suite can put an unbound
/// credential in front of an edge that requires binding (`fed.body_bound`). Returns the status and
/// the body the gateway answered.
async fn probe_call_unbound(State(p): State<ProbeState>, Json(req): Json<UnboundReq>) -> Response {
    use mycelium::federation::{call::FederatedCaller, edge::PresentedCall};
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
        "params": { "skillId": req.export, "message": { "role": "user", "parts": [{ "type": "text", "text": "?" }] } },
    }))
    .expect("serialise");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let presented = PresentedCall::sign(
        &FederatedCaller {
            origin_domain: p.origin.clone(),
            principal: p.principal.clone(),
            export: req.export.clone(),
            issued_at_ms: now,
            expires_at_ms: now + 60_000,
            body_sha256: None,
        },
        &p.signing,
    );
    let sent = p
        .http
        .post(format!("{}/a2a", req.gateway.trim_end_matches('/')))
        .header(mycelium::federation::edge::HEADER_FEDERATED_CALL, presented.to_header_value())
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    match sent {
        Ok(r) => {
            let status = r.status().as_u16();
            let text = r.text().await.unwrap_or_default();
            Json(json!({ "status": status, "body": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)) })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": "transport", "detail": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct UnboundReq {
    gateway: String,
    export: String,
}

/// What the unbound-call verb needs beside the client: the probe's own identity and a pinned HTTP client.
#[derive(Clone)]
struct ProbeState {
    origin: DomainId,
    principal: String,
    signing: ed25519_dalek::SigningKey,
    http: reqwest::Client,
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
    let principal = env_or("FED_PRINCIPAL", "svc/billing");
    let mut probe = ProbeState {
        origin: origin.clone(),
        principal: principal.clone(),
        signing: signing.clone(),
        http: reqwest::Client::new(),
    };
    let mut client = FederationClient::new(
        origin,
        principal,
        signing,
        partner,
        endpoints,
        env_or("FED_SLOTS", "2").parse().expect("FED_SLOTS"),
        Duration::from_secs(env_or("FED_FRESHNESS_SECS", "60").parse().expect("FED_FRESHNESS_SECS")),
    );
    if let Some(k) = env("FED_PARTNER_KEY") {
        client = client.with_partner_key(hex32(&k));
    }
    // The partner's gateways serve HTTPS on their node identity; pin each one's SPKI, as the
    // partner publishes them (`fed.pinned`). The `pki` role writes one `.pin` per gateway.
    if let Some(dir) = env("FED_PINS_DIR") {
        let mut pins = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("FED_PINS_DIR {dir}: {e}")) {
            let path = entry.expect("pin entry").path();
            if path.extension().is_some_and(|x| x == "pin") {
                pins.push(hex32(&std::fs::read_to_string(&path).expect("read pin")));
            }
        }
        assert!(!pins.is_empty(), "FED_PINS_DIR {dir} holds no .pin file");
        info!(pins = pins.len(), "pinning the partner's gateways");
        probe.http = reqwest::Client::builder()
            .use_preconfigured_tls(mycelium::federation::pinning::pinned_client_config(pins.clone()))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("pinned HTTP client");
        client = client.with_tls_pins(pins);
    }
    let client = Arc::new(client);
    let app = Router::new()
        .route("/ready", get(|| async { "ok" }))
        .route("/connect", post(probe_connect))
        .route("/call", post(probe_call))
        .route("/retire", post(probe_retire))
        .route("/link", get(probe_link))
        .with_state(client)
        .merge(Router::new().route("/call_unbound", post(probe_call_unbound)).with_state(probe));
    let port: u16 = env_or("FED_CONTROL_PORT", "8400").parse().expect("FED_CONTROL_PORT");
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind control port");
    info!(port, "probe control API up");
    axum::serve(listener, app).await.expect("serve");
}

// ── PKI (where the CA key is) ────────────────────────────────────────────────────────────────

/// The off-node issuer (`id.ca_key_off_node`): mints this domain's CA in `FED_CA_DIR` if it has none,
/// then issues a certificate for each node in `FED_NODES` (`ip:port,…`) under
/// `FED_CERT_OUT/<node>/` with `ca-cert.pem` beside it — the certificate only. The CA's private key
/// stays in `FED_CA_DIR`, a volume no node mounts. For each node in `FED_PIN_NODES` it writes the
/// SPKI pin of its identity (what its HTTPS gateway serves) to `FED_PIN_OUT/<node>.pin`, for the
/// partner to pin. Exits when done; the nodes wait for it.
async fn run_pki() {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let ca_dir = std::path::PathBuf::from(must("FED_CA_DIR"));
    let out = std::path::PathBuf::from(must("FED_CERT_OUT"));
    if !ca_dir.join("ca-key.pem").exists() {
        // The CA is minted the one way the library mints one: a node started over an empty
        // certificate directory. It runs here, beside the key, and never joins a mesh.
        let mut cfg = GossipConfig::auto();
        cfg.bind_address = "127.0.0.1".into();
        cfg.bind_port = 57999;
        cfg.tls = Some(TlsConfig { auto_cert_dir: ca_dir.clone(), ..TlsConfig::default() });
        let issuer = GossipAgent::new(NodeId::new("127.0.0.1", 57999).expect("issuer id"), cfg);
        issuer.start().await.expect("the issuer mints the domain CA");
        issuer.shutdown().await;
    }
    let node_list = must("FED_NODES");
    let nodes: Vec<&str> = node_list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    // Re-issuing over a node's existing key would replace the identity of a node that may be running
    // on it; only `FED_PKI_REISSUE=1` says that is meant. Every node is checked before any is issued,
    // so a refusal leaves the volumes as they were, never half re-issued.
    if env("FED_PKI_REISSUE").as_deref() != Some("1") {
        for node in &nodes {
            let sanitized = node.replace([':', '.'], "_");
            let key = out.join(&sanitized).join(format!("{sanitized}.key.pem"));
            if key.exists() {
                refuse(&format!("{} already holds a key for {node}: set FED_PKI_REISSUE=1 to re-issue it", key.display()));
            }
        }
    }
    let pin_nodes: Vec<String> = env_or("FED_PIN_NODES", "").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    for &node in &nodes {
        let (host, port) = node.rsplit_once(':').unwrap_or_else(|| panic!("FED_NODES entry {node:?}: expected ip:port"));
        let id = NodeId::new(host, port.parse().expect("port")).expect("node id");
        let sanitized = id.as_str().replace([':', '.'], "_");
        let dir = out.join(&sanitized);
        let (_cert, key) = mycelium::issue_node_cert(&ca_dir, &id, &dir).expect("issue a node certificate");
        std::fs::copy(ca_dir.join("ca-cert.pem"), dir.join("ca-cert.pem")).expect("copy ca-cert.pem");
        if pin_nodes.iter().any(|p| p == node) {
            let pin_dir = std::path::PathBuf::from(must("FED_PIN_OUT"));
            std::fs::create_dir_all(&pin_dir).expect("pin dir");
            let pem = std::fs::read_to_string(&key).expect("read node key");
            let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
            let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).expect("PEM base64");
            let sk = ed25519_dalek::SigningKey::from_pkcs8_der(&der).expect("PKCS#8 node key");
            let pin = mycelium::federation::pinning::ed25519_spki_sha256(&sk.verifying_key().to_bytes());
            std::fs::write(pin_dir.join(format!("{sanitized}.pin")), pin.iter().map(|b| format!("{b:02x}")).collect::<String>())
                .expect("write pin");
        }
        info!(node, "issued off-node");
    }
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
        "pki" => run_pki().await,
        other => panic!("unknown FED_ROLE {other:?}: member | gateway | probe | pki | keys"),
    }
}
