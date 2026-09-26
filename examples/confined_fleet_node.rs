//! **One node image for the confined-fleet cluster** (Boundary H closure plan C7 and C12, deployment variants).
//!
//! The in-process gates (`test_c7_the_bypass_matrix_no_door_runs_revoked_work`, `examples/authority_drain`) run
//! every party in one process. This binary lets `scripts/test-confined-fleet.sh` run the same checks across pods in
//! the kind cluster, through the reference manifests and their network policy:
//!
//! | `ROLE` | What runs | Where |
//! |---|---|---|
//! | `operator` | The mandate authority: signs the grant, issues a revocation checkpoint every second, revokes on request | its own pod, reachable by members, not by agents |
//! | `provider` | A member serving the tools and the skill, with provider enforcement, fed the operator's checkpoints | its own pod |
//! | `gateway` | The member agents talk to (`/mcp`, `/a2a`, the raw routes), with enforcement, fed the same checkpoints | the reference `gateway` deployment |
//!
//! and subcommands the harness runs with `kubectl exec`:
//!
//! - `agent-doors <phase> <provider-node>` (in the agent pod): tries every door through the gateway with the
//!   grant read from stdin. Phase `plant` expects them to reach the handlers; phase `revoked` also checks that
//!   the raw routes refuse protected kinds (`403`);
//! - `agent-load <ms>` (in the agent pod): keeps admitting calls to the long-running tool for `ms`;
//! - `ca-init <dir>`, `pubkey <seed-hex>`: set-up helpers for the harness.
//!
//! **The pass condition is the provider's own records**, read from its report port (`127.0.0.1:9100`, inside the
//! pod only): how many times each handler ran, when each long-running call was admitted, and the authority's stop
//! records. Never the absence of an error at the caller.
//!
//! **What it is not.** The pods share one kind node, so they share one kernel clock: *s* is declared (100 ms) but
//! not exercised. The checkpoint reaches members by polling the operator every `POLL_MS`, so the stop is
//! measured from the operator's revocation and allowed that delivery time on top of the class's declared bound.
//! The member CA key is on the members (the development profile, `ca_key_off_node` unmet): this cluster tests
//! authority, not removal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use mycelium::capability::Capability;
use mycelium::config::{GatewayNamedToken, TlsConfig};
use mycelium::knowledge::issuer::TrustedExternalIssuers;
use mycelium::knowledge::IssuerId;
use mycelium::mandate::authority::{
    drain_report, ClockModel, Continuation, DrainBound, ExecutionGate, FreshnessPolicy, ResourceTier,
    RevocationCheckpoint, SignedRevocationCheckpoint, StopContract,
};
use mycelium::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use mycelium::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use mycelium::{
    arguments_digest, possession_request, ExecutionAuthority, GossipAgent, GossipConfig, NodeId, PresentedMandate,
    ReferenceEvaluator, Rule,
};

const GOSSIP_PORT: u16 = 57000;
const GATEWAY_PORT: u16 = 8080;
const OPERATOR_PORT: u16 = 8400;
const REPORT_PORT: u16 = 9100;
/// Members fetch the operator's latest checkpoint this often.
const POLL_MS: u64 = 200;
/// The declared clock bound. One kind node shares one clock, so this is declared, not exercised.
const SKEW_MS: u64 = 100;
/// The delivery allowance on top of the class's bound: a poll interval plus the network, rounded up to the
/// freshness policy's own `delivery_ms`.
const DELIVERY_MS: u64 = 500;
const AUTHORITY: &str = "operator:acme";
const CALLER: &str = "token:gw/agent-1";
const TOKEN: &str = "agent-1-secret";

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

fn key_from_hex(hex: &str) -> [u8; 32] {
    let hex = hex.trim();
    assert_eq!(hex.len(), 64, "a key is 32 bytes of hex");
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
        Some("agent-doors") => agent_doors(&args[2], &args[3]).await,
        Some("agent-load") => agent_load(args[2].parse().expect("milliseconds")).await,
        _ => match env("ROLE").as_str() {
            "operator" => operator().await,
            "provider" => member(Role::Provider).await,
            "gateway" => member(Role::Gateway).await,
            other => panic!("unknown ROLE {other}"),
        },
    }
}

/// Generate the fleet CA into `dir` (`ca-cert.pem`, `ca-key.pem`), by starting a node against it once.
async fn ca_init(dir: &str) {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = pick_port();
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.into(), ..Default::default() });
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), cfg);
    agent.start().await.expect("start");
    agent.shutdown().await;
    for f in ["ca-cert.pem", "ca-key.pem"] {
        assert!(std::path::Path::new(dir).join(f).exists(), "{f} was not written");
    }
}

fn pick_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

// ── The operator ─────────────────────────────────────────────────────────────

struct Operator {
    key: SigningKey,
    seq: AtomicU64,
    revoked_at: AtomicU64, // 0 = nothing revoked
    latest: Mutex<Option<SignedRevocationCheckpoint>>,
}

impl Operator {
    /// A cumulative checkpoint: `t1` is in it from the revocation on.
    fn issue(&self) {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new(AUTHORITY).unwrap(),
            scope: "depot".into(),
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            issued_at_ms: wall_ms(),
            revoked: if self.revoked_at.load(Ordering::SeqCst) == 0 { Default::default() } else { [TermId::new("t1").unwrap()].into() },
        };
        let signed = SignedRevocationCheckpoint { signature: self.key.sign(&c.canonical_bytes()).to_bytes().to_vec(), checkpoint: c };
        *self.latest.lock().unwrap() = Some(signed);
    }
}

async fn operator() {
    let op = Arc::new(Operator {
        key: SigningKey::from_bytes(&key_from_hex(&env("AUTHORITY_SEED"))),
        seq: AtomicU64::new(0),
        revoked_at: AtomicU64::new(0),
        latest: Mutex::new(None),
    });
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
            "/revoke",
            post(|State(op): State<Arc<Operator>>| async move {
                let at = wall_ms();
                let _ = op.revoked_at.compare_exchange(0, at, Ordering::SeqCst, Ordering::SeqCst);
                op.issue(); // deliver at once: members pick it up at their next poll
                Json(serde_json::json!({ "revoked_at_ms": op.revoked_at.load(Ordering::SeqCst) }))
            }),
        )
        .route(
            "/grant",
            get(|State(op): State<Arc<Operator>>, Query(q): Query<HashMap<String, String>>| async move {
                let mandate = Mandate {
                    holder: PrincipalId::new(q.get("holder").map(String::as_str).unwrap_or(CALLER)).unwrap(),
                    established_by: PrincipalId::new(AUTHORITY).unwrap(),
                    purpose: "depot".into(),
                    scope: "depot".into(),
                    operations: vec![
                        "tools/call:tool:work".into(),
                        "tools/call:tool:slow".into(),
                        "skill.invoke:skill:depot/dispatch".into(),
                    ],
                    epoch: 1,
                    term: TermId::new("t1").unwrap(),
                    valid_from_ms: 0,
                    valid_until_ms: wall_ms() + 900_000,
                };
                Json(SignedMandateGrant { signature: op.key.sign(&mandate.canonical_bytes()).to_bytes().to_vec(), mandate })
            }),
        )
        .with_state(op);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", OPERATOR_PORT)).await.unwrap();
    println!("operator: serving on :{OPERATOR_PORT}");
    axum::serve(listener, app).await.unwrap();
}

// ── The members ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Gateway,
    Provider,
}

/// What the provider's handlers saw, for the report.
#[derive(Default)]
struct Seen {
    work: AtomicUsize,
    skill: AtomicUsize,
    slow_admissions: Mutex<Vec<u64>>,
}

async fn member(role: Role) {
    // The fleet CA, from the mounted secret, into a writable certificate directory.
    let cert_dir = std::path::PathBuf::from("/var/lib/mycelium/tls");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let ca_dir = std::path::PathBuf::from(std::env::var("CA_DIR").unwrap_or_else(|_| "/etc/mycelium/tls".into()));
    for f in ["ca-cert.pem", "ca-key.pem"] {
        std::fs::copy(ca_dir.join(f), cert_dir.join(f)).unwrap_or_else(|e| panic!("copy {f} from {ca_dir:?}: {e}"));
    }

    let me = NodeId::new(env("POD_IP").as_str(), GOSSIP_PORT).unwrap();
    let provider = match role {
        Role::Provider => me.clone(),
        Role::Gateway => env("PROVIDER_NODE").parse::<NodeId>().expect("PROVIDER_NODE is host:port"),
    };
    let mut cfg = GossipConfig::default();
    cfg.bind_address = env("POD_IP"); // a node binds the address it is known by
    cfg.bind_port = GOSSIP_PORT;
    cfg.reconnect_backoff_secs = 1;
    cfg.health_check_interval_secs = 1;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir, ..Default::default() });
    if role == Role::Gateway {
        cfg.bootstrap_peers = vec![provider.clone()];
        cfg.http_port = Some(GATEWAY_PORT);
        cfg.http_addr = "0.0.0.0".into();
        cfg.gateway_identity_issuer = Some("gw".into());
        cfg.gateway_named_tokens =
            vec![GatewayNamedToken { name: "agent-1".into(), token: TOKEN.into(), scopes: vec!["*".into()] }];
    }
    let agent = GossipAgent::new(me.clone(), cfg);
    let agent = Arc::new(if role == Role::Gateway { agent.with_a2a() } else { agent });

    // The same policy and authority on both members: the gateway decides at its doors, the provider where the
    // work runs (C3).
    let (work, slow, skill) = (format!("tool:work@{provider}"), format!("tool:slow@{provider}"), format!("skill:depot/dispatch@{provider}"));
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-fleet")
            .with_catalogue("cat-fleet", "1")
            .map_action("tools/call", work.clone())
            .map_action("tools/call", slow.clone())
            .map_action("skill.invoke", skill.clone())
            .allow(Rule::new("*", "tools/call", work.clone()).requiring_mandate("depot"))
            .allow(Rule::new("*", "tools/call", slow.clone()).requiring_mandate("depot"))
            .allow(Rule::new("*", "skill.invoke", skill.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new(AUTHORITY).unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new(AUTHORITY).unwrap(), key_from_hex(&env("AUTHORITY_PUB"))).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), key_from_hex(&env("HOLDER_PUB"))).unwrap();
    // F = 2 s, I = 1 s, D = 0.5 s, s = 0.1 s, as `authority_drain`.
    let freshness = FreshnessPolicy { freshness_ms: 2_000, interval_ms: 1_000, delivery_ms: DELIVERY_MS };
    let gate = ExecutionGate::strict(ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, freshness)
        .expect("a valid profile");
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external));
    agent.with_execution_authority(Arc::clone(&authority));
    agent.with_provider_enforcement();
    agent.start().await.expect("start");

    // Feed the operator's checkpoints.
    {
        let (agent, url) = (Arc::clone(&agent), format!("{}/checkpoint", env("OPERATOR_URL")));
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            loop {
                if let Ok(r) = http.get(&url).timeout(Duration::from_secs(2)).send().await
                    && let Ok(Some(c)) = r.json::<Option<SignedRevocationCheckpoint>>().await
                {
                    let _ = agent.offer_revocation_checkpoint(&c);
                }
                tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
            }
        });
    }

    if role == Role::Gateway {
        println!("gateway {me}: serving on :{GATEWAY_PORT}, provider {provider}");
        std::future::pending::<()>().await;
    }

    // The provider's handlers, each recording what reaches it.
    let seen = Arc::new(Seen::default());
    let _cap = agent.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
    let s = Arc::clone(&seen);
    let _work = agent.mcp().register_mcp_tool("work", serde_json::json!({}), move |_args| {
        let s = Arc::clone(&s);
        async move {
            s.work.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("done"))
        }
    });
    let s = Arc::clone(&seen);
    let _slow = agent.mcp().register_mcp_tool("slow", serde_json::json!({}), move |_args| {
        s.slow_admissions.lock().unwrap().push(wall_ms());
        async move {
            std::future::pending::<()>().await; // runs until it is cancelled
            Ok(serde_json::json!("finished"))
        }
    });
    {
        let (agent, s) = (Arc::clone(&agent), Arc::clone(&seen));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                s.skill.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }

    // The report, inside the pod only.
    let app = Router::new()
        .route(
            "/report",
            get(|State((seen, authority)): State<(Arc<Seen>, Arc<ExecutionAuthority>)>, Query(q): Query<HashMap<String, u64>>| async move {
                let admissions = seen.slow_admissions.lock().unwrap().clone();
                let stops = authority.stop_records();
                let revoked_at = q.get("revoked_at").copied().unwrap_or(0);
                let contract = StopContract {
                    continuation: Continuation::ReauthorizeAt { interval_ms: authority.sweep_interval_ms() },
                    cancellation_latency_ms: Some(50),
                    confirmation_latency_ms: Some(50),
                };
                let DrainBound::Bounded(bound) = contract.drain_bound(ClockModel { skew_ms: SKEW_MS }, 0) else {
                    unreachable!("a cancellable class has a bound")
                };
                let report = (revoked_at > 0).then(|| drain_report(revoked_at, &admissions, &stops));
                Json(serde_json::json!({
                    "work": seen.work.load(Ordering::SeqCst),
                    "skill": seen.skill.load(Ordering::SeqCst),
                    "slow_admitted": admissions.len(),
                    "slow_admitted_before": admissions.iter().filter(|t| **t < revoked_at).count(),
                    "stops": stops.len(),
                    "t_admit_ms": report.as_ref().and_then(|r| r.t_admit_ms),
                    "t_drain_ms": report.as_ref().and_then(|r| r.t_drain_confirmed_ms),
                    "unconfirmed": report.as_ref().map(|r| r.unconfirmed),
                    "bound_ms": bound,
                    "delivery_ms": DELIVERY_MS,
                    "skew_ms": SKEW_MS,
                }))
            }),
        )
        .with_state((seen, authority));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", REPORT_PORT)).await.unwrap();
    println!("provider {me}: serving; report on 127.0.0.1:{REPORT_PORT}");
    axum::serve(listener, app).await.unwrap();
}

// ── The agent (untrusted; reaches only the gateway) ──────────────────────────

fn read_grant() -> SignedMandateGrant {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).unwrap();
    serde_json::from_str(&s).expect("a grant on stdin")
}

fn presented(grant: &SignedMandateGrant, holder: &SigningKey, op: &str, resource: &str, args: &serde_json::Value) -> serde_json::Value {
    let proof = holder.sign(&possession_message(&grant.mandate, &possession_request(op, resource, &arguments_digest(args))));
    serde_json::to_value(PresentedMandate {
        grant: grant.clone(),
        possession: base64::engine::general_purpose::STANDARD.encode(proof.to_bytes()),
    })
    .unwrap()
}

fn gateway() -> String {
    std::env::var("MYCELIUM_GATEWAY").unwrap_or_else(|_| format!("http://gateway.confined-fleet.svc:{GATEWAY_PORT}"))
}

async fn mcp_call(http: &reqwest::Client, tool: &str, m: serde_json::Value) -> String {
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":tool,"arguments":{},"_meta":{"mandate":m}}});
    match http.post(format!("{}/mcp", gateway())).bearer_auth(TOKEN).json(&body).send().await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => format!("transport error: {e}"),
    }
}

async fn agent_doors(phase: &str, provider: &str) {
    let grant = read_grant();
    let holder = SigningKey::from_bytes(&key_from_hex(&env("HOLDER_SEED")));
    let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap();
    let text = "go";
    let (tool, skill) = (format!("tool:work@{provider}"), format!("skill:depot/dispatch@{provider}"));
    let empty = serde_json::json!({});
    let skill_args = serde_json::json!({ "text": text });

    println!("/mcp tools/call: {}", mcp_call(&http, "work", presented(&grant, &holder, "tools/call", &tool, &empty)).await);
    for method in ["tasks/send", "tasks/sendSubscribe"] {
        let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":{
            "id": format!("task-{method}-{}", wall_ms()), "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
            "_meta": {"mandate": presented(&grant, &holder, "skill.invoke", &skill, &skill_args)}}});
        let out = match http.post(format!("{}/a2a", gateway())).bearer_auth(TOKEN).json(&body).send().await {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(e) => format!("transport error: {e}"),
        };
        println!("/a2a {method}: {}", out.chars().take(300).collect::<String>());
    }

    if phase == "revoked" {
        // The raw routes refuse protected kinds whatever the token's scopes (C1).
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"work","arguments":{}}}).to_string());
        let mut failed = false;
        for (route, body) in [
            ("rpc/call", serde_json::json!({"target": provider, "method": "mcp.invoke", "payload_b64": b64, "timeout_secs": 2})),
            ("rpc/call", serde_json::json!({"target": provider, "method": "skill.invoke", "payload_b64": b64, "timeout_secs": 2})),
            ("signal/emit", serde_json::json!({"kind": "mcp.invoke", "scope": format!("node:{provider}"), "payload_b64": b64})),
        ] {
            let status = http.post(format!("{}/gateway/{route}", gateway())).bearer_auth(TOKEN).json(&body).send().await.map(|r| r.status().as_u16());
            println!("/gateway/{route} {}: {status:?}", body.get("method").or(body.get("kind")).unwrap());
            failed |= status.ok() != Some(403);
        }
        if failed {
            println!("FAIL  a raw route did not refuse a protected kind with 403");
            std::process::exit(1);
        }
    }
}

async fn agent_load(ms: u64) {
    let grant = read_grant();
    let holder = SigningKey::from_bytes(&key_from_hex(&env("HOLDER_SEED")));
    // The resource key before `@` is what the proof binds, so the agent need not know the provider.
    let m = presented(&grant, &holder, "tools/call", "tool:slow", &serde_json::json!({}));
    let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();
    let started = wall_ms();
    let mut sent = 0;
    while wall_ms() - started < ms {
        let (http, m) = (http.clone(), m.clone());
        tokio::spawn(async move { mcp_call(&http, "slow", m).await });
        sent += 1;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!("agent-load: {sent} call(s) sent over {ms} ms");
}
