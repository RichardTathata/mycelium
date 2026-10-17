//! **A node of the Strands fleet demonstration** (post-360 plan row P). The driver,
//! `examples/strands_fleet/run_demo.py`, starts one operator and three nodes from this binary and runs Strands
//! agents against them; its README says what the run shows and what it does not.
//!
//! | `ROLE` | What runs |
//! |---|---|
//! | `operator` | The mandate authority of the food co-op: appoints one courier to dispatch — one grant, signed at start for the holder its configuration names (`APPOINT`), served to that holder only (`GET /grant`) — and issues a revocation checkpoint every second (`GET /checkpoint`) |
//! | `node` | A fleet member: a gateway with `/a2a`, the action evaluator, the mandate authority's verifier, an evidence journal and a consensus voter, serving one skill |
//!
//! and three helpers the driver runs: `ca-init <dir>` (the fleet CA), `pubkey <seed-hex>` (the authority's public
//! key), `evidence <journal>` (this node's evidence records, one JSON object a line).
//!
//! ```bash
//! cargo build --example strands_fleet_node --features tls,a2a,compliance
//! python examples/strands_fleet/run_demo.py
//! ```
//!
//! A node serves the one skill in `SKILL` — `skill.invoke` carries only the message text, so one handler per node
//! keeps the answer the skill's. The co-op's policy, the same on every node: asking the bakery or the kitchen is
//! allowed to the fleet's own agents (each a principal named by its bearer); **dispatching a van is a mandated
//! role** — `depot/dispatch` is allowed to anyone only with an established mandate in scope `dispatch`, and the
//! gateway establishes one only from a grant the operator signed for the calling principal, presented with the
//! holder's proof for this very call. A mandate is an appointment: at each gateway's verifier, a scope has one holder
//! at an epoch (two grants at one scope and epoch are `ConflictingAppointments` there — each node's verifier keeps its
//! own memory, so one holder across the fleet is the authority's to keep, by issuing one grant). So the operator
//! appoints one courier and signs nothing else. A call without the grant is refused, and the refusal is in the
//! evidence journal of the gateway that decided it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use mycelium::capability::Capability;
use mycelium::config::{GatewayNamedToken, PersistenceConfig, TlsConfig};
use mycelium::knowledge::issuer::TrustedExternalIssuers;
use mycelium::knowledge::IssuerId;
use mycelium::mandate::authority::{
    CheckpointOffer, ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
};
use mycelium::mandate::grant::{EntitlementTable, GrantVerifier, SignedMandateGrant};
use mycelium::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use mycelium::{
    AeEvidence, EvidenceJournal, EvidenceProfile, ExecutionAuthority, GossipAgent, GossipConfig, NodeId,
    ReferenceEvaluator, Rule,
};

/// The co-op's mandate authority, the mandated role's scope, and the skill it covers.
const AUTHORITY: &str = "operator:food-coop";
const SCOPE: &str = "dispatch";
const MANDATED_SKILL: &str = "depot/dispatch";
/// The principals the gateway names its token holders by: `token:{ISSUER}/{name}`.
const ISSUER: &str = "fleet";

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

fn key_from_hex(hex: &str) -> [u8; 32] {
    let hex = hex.trim();
    assert_eq!(hex.len(), 64, "a 32-byte key in hex");
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("pubkey") => println!("{}", to_hex(&SigningKey::from_bytes(&key_from_hex(&args[2])).verifying_key().to_bytes())),
        Some("ca-init") => ca_init(&args[2]).await,
        Some("evidence") => evidence(&args[2]),
        _ => {
            let shutdown = mycelium::shutdown::ShutdownSignal::install().expect("signal handlers");
            let role = async {
                match env("ROLE").as_str() {
                    "operator" => operator().await,
                    "node" => node().await,
                    other => panic!("unknown ROLE {other}"),
                }
            };
            tokio::select! {
                _ = role => {}
                _ = shutdown.wait() => {}
            }
        }
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

/// This node's evidence records, one JSON object a line — read from the node-local journal, which never gossips.
fn evidence(path: &str) {
    for record in mycelium::read_evidence_journal(std::path::Path::new(path)).expect("the evidence journal") {
        let record: AeEvidence = serde_json::from_slice(&record).expect("an evidence record");
        println!("{}", serde_json::to_string(&record).expect("encode"));
    }
}

// ── The operator ─────────────────────────────────────────────────────────────

struct Operator {
    key: SigningKey,
    seq: AtomicU64,
    latest: Mutex<Option<SignedRevocationCheckpoint>>,
    /// The one appointment: signed once, at start. Re-signing on request would make each fetch a second grant at
    /// the same scope and epoch, which a verifier refuses as a conflicting appointment — the holder's own included.
    appointment: SignedMandateGrant,
}

impl Operator {
    /// A checkpoint revoking nothing: the members' standing for every grant stays fresh while it arrives.
    fn issue(&self) {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new(AUTHORITY).unwrap(),
            scope: SCOPE.into(),
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            issued_at_ms: wall_ms(),
            revoked: Default::default(),
        };
        let signed = SignedRevocationCheckpoint { signature: self.key.sign(&c.canonical_bytes()).to_bytes().to_vec(), checkpoint: c };
        *self.latest.lock().unwrap() = Some(signed);
    }
}

async fn operator() {
    let key = SigningKey::from_bytes(&key_from_hex(&env("AUTHORITY_SEED")));
    // Whom the co-op appoints, and to what: configuration, not whoever asks.
    let mandate = Mandate {
        holder: PrincipalId::new(format!("token:{ISSUER}/{}", env("APPOINT"))).unwrap(),
        established_by: PrincipalId::new(AUTHORITY).unwrap(),
        purpose: "dispatch tonight's redistribution runs".into(),
        scope: SCOPE.into(),
        operations: vec![format!("skill.invoke:skill:{MANDATED_SKILL}")],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 12 * 3_600_000,
    };
    let appointment = SignedMandateGrant { signature: key.sign(&mandate.canonical_bytes()).to_bytes().to_vec(), mandate };
    let op = Arc::new(Operator { key, seq: AtomicU64::new(0), latest: Mutex::new(None), appointment });
    op.issue();
    {
        let op = Arc::clone(&op);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1_000)).await;
                op.issue();
            }
        });
    }
    let app = Router::new()
        .route("/checkpoint", get(|State(op): State<Arc<Operator>>| async move { Json(op.latest.lock().unwrap().clone()) }))
        .route(
            "/grant",
            // The appointment, to its holder: anyone else is told no — the authority signs nothing on request.
            get(|State(op): State<Arc<Operator>>, Query(q): Query<HashMap<String, String>>| async move {
                if q.get("holder").map(String::as_str) == Some(op.appointment.mandate.holder.as_str()) {
                    Ok(Json(op.appointment.clone()))
                } else {
                    Err((axum::http::StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "not_appointed" }))))
                }
            }),
        )
        .with_state(op);
    let port: u16 = env("OPERATOR_PORT").parse().expect("OPERATOR_PORT");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    println!("ready operator :{port}");
    axum::serve(listener, app).await.unwrap();
}

// ── A node ───────────────────────────────────────────────────────────────────

/// The skill's deterministic answer: the demonstration is about where a call goes and who may make it, not
/// about what a model would say.
fn answer(skill: &str, text: &str) -> String {
    match skill {
        "bakery/surplus" => "surplus tonight: 40 loaves at Rye Lane bakery; 12 trays of pastries at Market Street bakery".into(),
        "kitchen/needs" => "needs tonight: Northside shelter, 30 meals; Eastgate community kitchen, 20 meals".into(),
        "depot/dispatch" => format!("dispatched: {text}"),
        other => format!("{other}: {text}"),
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
    // A voter persists its promises: one that forgot them on a restart could let a claim be decided twice.
    cfg.persistence = Some(PersistenceConfig {
        base_path: data.join("kv"),
        sync_mode: Default::default(),
        snapshot_wal_threshold: 10_000,
        snapshot_interval_secs: 300,
        on_unreadable: Default::default(),
    });
    cfg.http_port = Some(env("HTTP_PORT").parse().expect("HTTP_PORT"));
    cfg.gateway_identity_issuer = Some(ISSUER.into());
    // Each agent's bearer names its principal, `token:fleet/{agent}`. The scopes are the session store's: its rows
    // (`kv:*`) and its claims (`consensus:*`). `/a2a` takes the principal; the mandate, not a scope, decides there.
    let agents: Vec<(String, String, String)> = env("AGENTS")
        .split(',')
        .map(|a| {
            let mut it = a.splitn(3, ':');
            (it.next().unwrap().to_string(), it.next().expect("token").to_string(), it.next().expect("public key").to_string())
        })
        .collect();
    cfg.gateway_named_tokens = agents
        .iter()
        .map(|(agent, token, _)| GatewayNamedToken {
            name: agent.clone(),
            token: token.clone(),
            scopes: ["kv:read", "kv:write", "consensus:read", "consensus:write"].map(String::from).to_vec(),
        })
        .collect();
    let agent = Arc::new(GossipAgent::new(me.clone(), cfg).with_a2a());

    // The co-op's policy, the same on every node: the fleet's agents may ask; only a mandate in `dispatch` sends
    // a van. A principal the policy does not name — `anonymous`, say — is allowed nothing.
    let mut policy = ReferenceEvaluator::new("rev-food-coop").with_catalogue("cat-food-coop", "1");
    for provided in env("PROVIDERS").split(',') {
        let (skill, provider) = provided.split_once('@').expect("PROVIDERS is ns/name@host:port,…");
        let resource = format!("skill:{skill}@{provider}");
        policy = policy.map_action("skill.invoke", resource.clone());
        if skill == MANDATED_SKILL {
            policy = policy.allow(Rule::new("*", "skill.invoke", resource).requiring_mandate(SCOPE));
        } else {
            for (agent_name, _, _) in &agents {
                policy = policy.allow(Rule::new(format!("token:{ISSUER}/{agent_name}"), "skill.invoke", resource.clone()));
            }
        }
    }
    agent.with_action_evaluator(Arc::new(policy));
    let journal_path = data.join("evidence.log");
    agent.with_evidence_journal(EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).expect("the evidence journal"));

    // Who may grant in `dispatch`, and whose keys sign what: the operator's grants, each agent's possession proofs.
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle(SCOPE, PrincipalId::new(AUTHORITY).unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new(AUTHORITY).unwrap(), key_from_hex(&env("AUTHORITY_PUB"))).unwrap();
    for (agent_name, _, public) in &agents {
        external.trust(IssuerId::new(format!("token:{ISSUER}/{agent_name}")).unwrap(), key_from_hex(public)).unwrap();
    }
    let freshness = FreshnessPolicy { freshness_ms: 5_000, interval_ms: 1_000, delivery_ms: 1_000 };
    let gate = ExecutionGate::strict(ResourceAuthority::new(SCOPE, 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, freshness)
        .expect("a valid profile");
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));

    agent.start().await.expect("start");
    // The node votes: the session store claims every message and record version by consensus.
    let _voter = agent.consensus().start_consensus_listener(Default::default());

    // The operator's checkpoints keep each grant's revocation standing fresh. Only one issued after this node
    // started counts (closure plan C8), so the first accepted one is part of being ready: before it, a mandated call
    // is refused with "revocation standing unknown".
    let standing = Arc::new(AtomicBool::new(false));
    {
        let (agent, url, standing) = (Arc::clone(&agent), format!("{}/checkpoint", env("OPERATOR_URL")), Arc::clone(&standing));
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            loop {
                if let Ok(r) = http.get(&url).timeout(Duration::from_secs(2)).send().await
                    && let Ok(Some(c)) = r.json::<Option<SignedRevocationCheckpoint>>().await
                    && matches!(agent.offer_revocation_checkpoint(&c), Some(CheckpointOffer::Accepted))
                {
                    standing.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
    }

    // The one skill this node serves.
    let skill = env("SKILL");
    let (ns, skill_name) = skill.split_once('/').expect("SKILL is ns/name");
    {
        let (agent, skill) = (Arc::clone(&agent), skill.clone());
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let reply = answer(&skill, &String::from_utf8_lossy(&req.payload()));
                agent.service().rpc_respond(&req, reply.into_bytes());
            }
        });
    }
    let _cap = agent.capabilities().advertise_capability(Capability::new(ns, skill_name), Duration::from_secs(30));

    // Ready when a send dispatches anywhere in the fleet: every skill advertised and every member's caller-context
    // marker here (the gateway refuses to route to a node whose marker it cannot see).
    let mut keys = vec![];
    for provided in env("PROVIDERS").split(',') {
        let (skill, provider) = provided.split_once('@').unwrap();
        keys.push(format!("cap/{provider}/{skill}"));
        keys.push(format!("sys/caller-context/{provider}"));
    }
    for key in keys {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while agent.kv().get(&key).is_none() {
            assert!(tokio::time::Instant::now() < deadline, "{key} never reached {name}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !standing.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < deadline, "{name} never accepted a revocation checkpoint");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("ready {name} gossip={port} http={} skill={skill}", env("HTTP_PORT"));
    std::future::pending::<()>().await;
}
