//! **Measuring the stop** (Boundary H closure plan C12).
//!
//! `drain_report` computed T_admit and T_drain from timestamps a test supplied; nothing had measured
//! them on a running node. This does. A real node (provider enforcement on, an action evaluator that
//! requires a mandate, an execution authority fed a checkpoint every second) serves a tool that runs
//! until it is cancelled. A caller keeps admitting calls under a mandate; after a while the authority
//! revokes the term. The node's own records then give:
//!
//! - **T_admit**: from the revocation to the last call admitted after it (the handler's own entry
//!   times, recorded inside the handler);
//! - **T_drain**: from the revocation to the last **confirmed** stop (the authority's stop records);
//! - **unconfirmed**: stops requested but never confirmed, reported, never counted as stopped.
//!
//! and checks them against the bound the class declares (`StopContract::drain_bound`: *s* + the sweep
//! interval + the declared cancellation and confirmation latencies).
//!
//! Run: `cargo run --example authority_drain --features compliance`. CI runs it.
//!
//! **What the numbers show about the node.** An MCP tool loop serves **one call at a time**: while the
//! first call runs, the others queue behind it. So the calls admitted before the revocation are the
//! ones already running (one, here), and the queued ones reach the provider check only after the
//! first is cancelled, by which time the revocation has arrived: they are refused at admission, which
//! is what T_admit measures. A handler that should serve calls concurrently spawns its own work.
//!
//! **What it is and is not.** One process, real sockets, the node's own clock: a measurement of the
//! mechanism on a live node. It is not a measurement in a deployment; the confined-fleet cluster
//! variant is recorded as open in the closure plan.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use mycelium::config::TlsConfig;
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

const SKEW_MS: u64 = 100;

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

#[tokio::main]
async fn main() {
    let authority_key = SigningKey::from_bytes(&[91u8; 32]);
    let port = pick_port();
    let cert_dir = std::env::temp_dir().join(format!("authority-drain-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    let me = agent.node_id().clone();
    let tool = format!("tool:slow@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-drain")
            .with_catalogue("cat-drain", "1")
            .map_action("tools/call", tool.clone())
            .allow(Rule::new("*", "tools/call", tool.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    // F = 2 s, I = 1 s, D = 0.5 s, s = 0.1 s: F > 4s and I + D <= F - 4s. The sweep runs every F/20.
    let freshness = FreshnessPolicy { freshness_ms: 2_000, interval_ms: 1_000, delivery_ms: 500 };
    let gate = ExecutionGate::strict(ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, freshness)
        .expect("a valid profile");
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external));
    agent.with_execution_authority(Arc::clone(&authority));
    agent.with_provider_enforcement();
    agent.start().await.expect("start");

    // The tool records when each call is admitted, then runs until it is cancelled.
    let admissions: Arc<Mutex<Vec<u64>>> = Arc::default();
    let a2 = Arc::clone(&admissions);
    let _tool = agent.mcp().register_mcp_tool("slow", serde_json::json!({}), move |_args| {
        a2.lock().unwrap().push(wall_ms());
        async move {
            std::future::pending::<()>().await;
            Ok(serde_json::json!("finished"))
        }
    });

    // The authority: a cumulative checkpoint every second, even when nothing is revoked.
    let revoked = Arc::new(AtomicU64::new(0)); // 0 = nothing revoked yet
    let seq = Arc::new(AtomicU64::new(0));
    let issue = {
        let (agent, key, revoked, seq) = (Arc::clone(&agent), authority_key.clone(), Arc::clone(&revoked), Arc::clone(&seq));
        move || {
            let c = RevocationCheckpoint {
                authority: PrincipalId::new("operator:acme").unwrap(),
                scope: "depot".into(),
                seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
                issued_at_ms: wall_ms(),
                revoked: if revoked.load(Ordering::SeqCst) == 0 { Default::default() } else { [TermId::new("t1").unwrap()].into() },
            };
            let signed = SignedRevocationCheckpoint { signature: key.sign(&c.canonical_bytes()).to_bytes().to_vec(), checkpoint: c };
            agent.offer_revocation_checkpoint(&signed);
        }
    };
    issue();
    {
        let issue = issue.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1_000)).await;
                issue();
            }
        });
    }

    // The caller: a member acting under its own grant.
    let mandate = Mandate {
        holder: PrincipalId::new(format!("node:{me}")).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "slow work".into(),
        scope: "depot".into(),
        operations: vec!["tools/call:tool:slow".into()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 600_000,
    };
    let grant = SignedMandateGrant { signature: authority_key.sign(&mandate.canonical_bytes()).to_bytes().to_vec(), mandate };
    let args = serde_json::json!({});
    let proof = agent
        .sign_with_identity(&possession_message(&grant.mandate, &possession_request("tools/call", &tool, &arguments_digest(&args))))
        .expect("a tls identity");
    let presented = serde_json::to_value(PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof) }).unwrap();
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"slow","arguments":args}}).to_string();

    let load = {
        let (agent, me, tool, presented, call) = (Arc::clone(&agent), me.clone(), tool.clone(), presented.clone(), call.clone());
        tokio::spawn(async move {
            loop {
                let (agent, me, tool, presented, call) = (Arc::clone(&agent), me.clone(), tool.clone(), presented.clone(), call.clone());
                tokio::spawn(async move {
                    let _ = agent.service().rpc_call_with_mandate(me, "mcp.invoke", call.into_bytes(), &presented, &tool, Duration::from_secs(15)).await;
                });
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let before = admissions.lock().unwrap().len();
    assert!(before > 0, "calls were admitted before the revocation (the plant)");

    // Revoke, and deliver it at once.
    let revoked_at = wall_ms();
    revoked.store(revoked_at, Ordering::SeqCst);
    issue();

    tokio::time::sleep(Duration::from_millis(1_500)).await; // the load keeps trying throughout
    load.abort();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let admitted: Vec<u64> = admissions.lock().unwrap().clone();
    let stops = authority.stop_records();
    let report = drain_report(revoked_at, &admitted, &stops);
    let contract = StopContract {
        continuation: Continuation::ReauthorizeAt { interval_ms: authority.sweep_interval_ms() },
        cancellation_latency_ms: Some(50),
        confirmation_latency_ms: Some(50),
    };
    let DrainBound::Bounded(bound) = contract.drain_bound(ClockModel { skew_ms: SKEW_MS }, 0) else {
        panic!("a cancellable class has a bound");
    };

    println!("authority_drain: {} call(s) running when revoked (an MCP tool loop serves one at a time; the rest queued), {} stop(s) recorded", before, stops.len());
    println!("  T_admit  = {:?} ms (calls admitted after the revocation)", report.t_admit_ms);
    println!("  T_drain  = {:?} ms (to the last confirmed stop)", report.t_drain_confirmed_ms);
    println!("  unconfirmed stops = {}", report.unconfirmed);
    println!("  declared bound    = {bound} ms (s + sweep interval + cancellation + confirmation)");

    assert!(report.t_admit_ms.unwrap_or(0) <= SKEW_MS, "no call is admitted after the revocation, beyond s");
    let t_drain = report.t_drain_confirmed_ms.expect("running work was stopped and confirmed");
    assert!(t_drain <= bound, "T_drain {t_drain} ms is within the declared bound {bound} ms");
    assert_eq!(report.unconfirmed, 0, "every stop was confirmed");
    assert_eq!(stops.len(), before, "every call admitted before the revocation was stopped");
    println!("authority_drain: within the declared bound");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

fn pick_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
