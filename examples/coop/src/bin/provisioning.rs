//! Example 04 ⭐ — **the autonomic loop: buffer while peers self-provision**.
//!
//! The flagship. A surge of donations needs a `route/optimize` capability **no depot has yet**.
//! The donations **buffer in a tuple-space lane** while a depot **self-provisions** the optimizer —
//! a real WASM component pulled, content-verified, and instantiated — then advertises and serves
//! it. The moment it is live, the worker drains the backlog. Nothing predicted who would run the
//! optimizer: it was unmet demand, satisfied by a node electing to provision.
//!
//!   • `buffer`     — the tuple-space Primary (the rendezvous point).
//!   • `seeder`     — fills the `optimize` lane with donations (a client).
//!   • `provider-a` / `provider-b` — each runs a `Provisioner`: on unmet `route/optimize` demand,
//!                    one self-elects to pull+verify+instantiate the WASM optimizer, advertise it,
//!                    and serve it over RPC; the other idles as a standby.
//!   • `worker`     — declares it needs `route/optimize` (the demand), then drains the lane: take →
//!                    invoke the provisioned optimizer → complete to `done`.
//!
//! Then the **active optimizer is killed**: its capability evaporates, a second wave of donations
//! buffers, and the **standby self-provisions** to restore the capability — restart ≡ provisioning,
//! no coordinator. Both waves drain.
//!
//! **Wave 3 — shadow, then accept (D20).** An *agent* publishes a **proposed** v2 optimizer under
//! its own key. The provisioners load it only into the **shadow lane** — advertised as
//! `route/optimize.shadow`, callable by name for comparison, never resolved by the worker's
//! `route/optimize` filter — so the incumbent serves the whole wave and the proposal takes no
//! demand. A third depot comes online for this wave: it shadows v2 too, and never loads v1,
//! because that demand is already met. A **forged** acceptance (a stranger's key) changes nothing. The **reviewer's**
//! acceptance co-signs the same catalogue line; the shadow is withdrawn, the operator retires v1,
//! the incumbent dies, and the accepted v2 is what the last standby provisions to serve wave 3b.
//! *An agent proposed and a reviewer accepted* is a fact in the manifest, not a policy in
//! someone's head.
//!
//! The WASM artifacts are committed fixtures standing in for optimizers so CI needs no wasm
//! toolchain: v1 is `echo_component.wasm` (echoes its input — a deterministic "optimized route");
//! the proposed v2 is `unit_convert_component.wasm` (a different contract: `{"kg": n}` →
//! `{"tonnes": n/1000}`), so what serves wave 3b is visible in the replies.
//!
//! ## Loads
//! - **Content** — route/optimize — a WASM component (echo_component.wasm fixture)
//! - **Type** — `ArtifactKind::WasmComponent`
//! - **From** — InMemorySource (build-embedded fixture, single-process shortcut — see catalog for the cross-node path) → catalogue → autonomic Provisioner
//!
//! Run:  cargo run -p mycelium-coop-examples --features wasm --bin provisioning

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use coop::common::{alloc_ports, announce_loads, spawn_depot, Donation, DepotOpts, Loads};
use ed25519_dalek::SigningKey;
use mycelium::{CapFilter, Capability, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};
use mycelium_wasm_host::{
    cap_invoke_kind, publish_installable, shadow_name, InMemorySource, InstallableCatalog, InstallableEntry,
    Provisioner, WasmHost,
};

/// The committed WASM fixture — a route optimizer stand-in (echoes its input).
const OPTIMIZER_WASM: &[u8] =
    include_bytes!("../../../../mycelium-wasm-host/tests/fixtures/echo_component.wasm");
/// The proposed v2 optimizer (D20) — a different component with a visibly different contract.
const OPTIMIZER_V2_WASM: &[u8] =
    include_bytes!("../../../../mycelium-wasm-host/tests/fixtures/unit_convert_component.wasm");

/// The operator's publishing key, the agent's key (the v2 author), the reviewer, and a stranger.
fn keys() -> (SigningKey, SigningKey, SigningKey, SigningKey) {
    (
        SigningKey::from_bytes(&[41u8; 32]),
        SigningKey::from_bytes(&[42u8; 32]),
        SigningKey::from_bytes(&[43u8; 32]),
        SigningKey::from_bytes(&[44u8; 32]),
    )
}

const LANE: &str = "optimize";
const DONE: &str = "done";
const N_DONATIONS: u64 = 4;

async fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    cond()
}

/// Run an autonomic `Provisioner` on `agent`: each tick, re-read the gossiped catalogue and
/// provision `route/optimize` **only while demand is unmet** (no live provider). A standby thus
/// idles until the active provider dies and demand reappears — restart ≡ first-time provisioning.
/// Provenance is required from the operator and the agent; only the reviewer's acceptance makes a
/// proposal loadable for real (D20). Returns the loop task.
fn spawn_provisioner(agent: Arc<mycelium::GossipAgent>) -> tokio::task::JoinHandle<()> {
    let (operator, author, reviewer, _) = keys();
    let mut source = InMemorySource::new();
    source.insert(OPTIMIZER_WASM.to_vec());
    source.insert(OPTIMIZER_V2_WASM.to_vec());
    let host = Arc::new(WasmHost::new().expect("wasm engine"));
    let catalog = InstallableCatalog::from_kv(&agent.kv());
    let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
    prov.require_provenance(vec![operator.verifying_key().to_bytes(), author.verifying_key().to_bytes()]);
    prov.require_reviewers(vec![reviewer.verifying_key().to_bytes()]);
    tokio::spawn(async move {
        loop {
            prov.refresh_catalog(InstallableCatalog::from_kv(&agent.kv()));
            let _ = prov.provision_round();
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    })
}

/// The v1 optimizer's catalogue line, signed by the operator.
fn v1_entry() -> InstallableEntry {
    let (operator, ..) = keys();
    InstallableEntry::new(Capability::new("route", "optimize"), mycelium_wasm_host::ArtifactId::of(OPTIMIZER_WASM))
        .with_cost(OPTIMIZER_WASM.len() as u64, 1)
        .signed_by(&operator)
}

/// The proposed v2 optimizer's catalogue line, signed by the agent that authored it (D20).
fn v2_proposed() -> InstallableEntry {
    let (_, author, ..) = keys();
    InstallableEntry::new(Capability::new("route", "optimize"), mycelium_wasm_host::ArtifactId::of(OPTIMIZER_V2_WASM))
        .with_cost(OPTIMIZER_V2_WASM.len() as u64, 1)
        .as_proposed()
        .signed_by(&author)
}

/// Drain one donation from the lane: take → invoke the live optimizer (RPC → WASM) → complete.
/// Returns the node that served the call and its reply (wave 3 counts who took the demand).
async fn process_one(
    worker: &Arc<mycelium::GossipAgent>,
    ts: &mycelium_tuple_space::TupleSpace,
    invoke_kind: &Arc<str>,
) -> Result<(NodeId, Bytes), Box<dyn std::error::Error>> {
    let (id, payload) = ts.take(LANE, Duration::from_secs(10)).await?;
    let providers = worker.capabilities().resolve(&CapFilter::new("route", "optimize"));
    let (opt_node, _) = providers.into_iter().next().ok_or("no route/optimize provider")?;
    // Retried per testing.md's deliverability corollary: the worker→provider leg's first
    // exercise can precede the sendable-set convergence on a cold start.
    let mut optimized: Option<Bytes> = None;
    for attempt in 1..=3u32 {
        match worker.service()
            .rpc_call(opt_node.clone(), Arc::clone(invoke_kind), payload.clone(),
                      Duration::from_secs(5))
            .await
        {
            Ok(b) => { optimized = Some(b); break; }
            Err(e) if attempt < 3 =>
                println!("[worker] optimize RPC attempt {attempt} failed ({e}); retrying"),
            Err(e) => return Err(e.into()),
        }
    }
    let optimized = optimized.expect("optimize reply after retries");
    ts.complete(id, DONE, optimized.clone()).await?;
    Ok((opt_node, optimized))
}

const LOADS: &[Loads] = &[
    Loads {
        content: "route/optimize — a WASM component (echo_component.wasm fixture), signed by the operator",
        kind: "ArtifactKind::WasmComponent",
        from: "InMemorySource (build-embedded fixture, single-process shortcut — see catalog for \
               the cross-node path) → gossiped catalogue (installable/) → autonomic Provisioner",
    },
    Loads {
        content: "route/optimize v2 — a PROPOSED WASM component (unit_convert_component.wasm fixture), \
                  signed by an agent; shadow lane until the reviewer accepts it (D20)",
        kind: "ArtifactKind::WasmComponent",
        from: "the same path, the catalogue line rewritten in place by the acceptance",
    },
];

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    announce_loads(LOADS);
    tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).init();

    let cert_dir = std::env::temp_dir().join(format!("coop-provisioning-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let p = alloc_ports(12); // buffer, seeder, provider-a, provider-b, provider-c, worker × (gossip, http)

    // ── buffer: tuple-space Primary (the rendezvous point) ──────────────────────
    let buffer = spawn_depot(DepotOpts {
        name: "buffer".into(), gossip_port: p[0], http_port: p[1],
        zone: "hub".into(), bootstrap: vec![], cert_dir: cert_dir.clone(), health_secs: None,
    }).await?;
    let seed = buffer.gossip_port;
    let _buf_ts = TupleSpace::new(Arc::clone(&buffer.agent), TupleConfig {
        namespace: Arc::from("rescue"), role: TupleRole::Primary, persist: false, ..Default::default()
    }).await?;
    println!("[buffer] up — tuple-space primary (ns rescue)");

    // ── seeder, provider, worker (all bootstrap the buffer) ─────────────────────
    let mk = |name: &str, gp: u16, hp: u16, zone: &str| DepotOpts {
        name: name.into(), gossip_port: gp, http_port: hp,
        zone: zone.into(), bootstrap: vec![seed], cert_dir: cert_dir.clone(), health_secs: None,
    };
    let seeder     = spawn_depot(mk("seeder",     p[2], p[3], "borough")).await?;
    let provider_a = spawn_depot(mk("provider-a", p[4], p[5], "depot-a")).await?;
    let provider_b = spawn_depot(mk("provider-b", p[6], p[7], "depot-b")).await?;
    let provider_c = spawn_depot(mk("provider-c", p[8], p[9], "depot-c")).await?;
    let worker     = spawn_depot(mk("worker",     p[10], p[11], "depot-d")).await?;
    println!("[seeder|provider-a|provider-b|provider-c|worker] up");
    // The operator publishes the v1 optimizer into the gossiped catalogue (signed).
    assert!(publish_installable(&buffer.agent.kv(), &v1_entry()));

    // Wait for the cluster to peer and the tuple-space primary to be resolvable.
    let seeder_ts = TupleSpace::new(Arc::clone(&seeder.agent), TupleConfig {
        namespace: Arc::from("rescue"), role: TupleRole::Client, persist: false, ..Default::default()
    }).await?;
    let worker_ts = TupleSpace::new(Arc::clone(&worker.agent), TupleConfig {
        namespace: Arc::from("rescue"), role: TupleRole::Client, persist: false, ..Default::default()
    }).await?;
    // Peers + identity keys (TLS: signed frames from a not-yet-known signer are dropped —
    // testing.md's identity-readiness gate; 5 nodes ⇒ 5 sys/identity/ entries everywhere).
    wait_until(30, || {
        !worker.agent.peers().is_empty() && !seeder.agent.peers().is_empty()
            && [&buffer.agent, &seeder.agent, &provider_a.agent, &provider_b.agent, &provider_c.agent, &worker.agent]
                .iter().all(|a| a.kv().scan_prefix("sys/identity/").len() >= 6)
    }).await;
    // Structural: wait until the client can reach the tuple-space primary (depth() succeeds).
    // Generous budget (30 s) — a constrained CI runner peers + elects the primary more slowly.
    let mut primary_ready = false;
    for _ in 0..300 {
        if seeder_ts.depth(None).await.is_ok() && worker_ts.depth(None).await.is_ok() {
            primary_ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(primary_ready, "clients must reach the tuple-space primary before seeding");

    // ── Phase 1 — wave 1 of donations buffers in the lane (no optimizer yet) ─────
    for id in 1..=N_DONATIONS {
        let d = Donation::new(id, "borough-market", "surplus produce", "southwark");
        seeder_ts.put(LANE, d.to_bytes()).await?;
    }
    let buffered = seeder_ts.depth(Some(LANE)).await?.first().map(|s| s.depth).unwrap_or(0);
    println!("[phase 1] {buffered} donations buffered in lane '{LANE}' — no route/optimize provider exists yet");
    assert!(buffered as u64 == N_DONATIONS, "all donations buffered while unprovisioned");

    // The worker declares it NEEDS route/optimize (the demand signal; a long TTL — wave 3 needs
    // the demand to still stand for the shadow lane to open). All three providers run an
    // autonomic provisioner each, self-electing to satisfy unmet demand (one wins the round; the
    // other idles as a standby until demand reappears — restart ≡ provisioning).
    let _req = worker.agent.capabilities()
        .declare_requirement(CapFilter::new("route", "optimize"), Duration::from_secs(600));
    let prov_a = spawn_provisioner(Arc::clone(&provider_a.agent));
    let prov_b = spawn_provisioner(Arc::clone(&provider_b.agent));
    // provider-c's provisioner starts in wave 3: the depot that comes online late, and the one
    // left to serve the accepted v2.

    // ── Phase 2 — the optimizer self-provisions; the worker drains wave 1 ───────
    let provisioned = wait_until(40, || {
        !worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize")).is_empty()
    }).await;
    assert!(provisioned, "a provider must self-provision route/optimize from unmet demand");
    println!("[phase 2] route/optimize self-provisioned (WASM pulled + verified + serving)");

    let invoke_kind: Arc<str> = Arc::from(cap_invoke_kind("route", "optimize").as_str());
    for _ in 0..N_DONATIONS {
        let (_, reply) = process_one(&worker.agent, &worker_ts, &invoke_kind).await?;
        assert!(!reply.starts_with(b"component-error") && !reply.starts_with(b"host-error"), "v1 echoes: {reply:?}");
    }
    let done1 = worker_ts.depth(Some(DONE)).await?.first().map(|s| s.depth).unwrap_or(0);
    println!("[phase 2] worker drained wave 1 → {done1} donations optimized");
    assert_eq!(done1 as u64, N_DONATIONS, "wave 1 fully optimized once the capability went live");

    // Which node won the provisioning? (The live provider of route/optimize.)
    let active = worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize"))
        .into_iter().next().map(|(id, _)| id).expect("a live optimizer");
    let name_of = |id: &NodeId| -> &'static str {
        if *id == provider_a.node_id() { "provider-a" } else if *id == provider_b.node_id() { "provider-b" } else { "provider-c" }
    };
    println!("[phase 2] active optimizer: {}", name_of(&active));

    // ── Phase 3 — kill the active optimizer; a standby self-heals (restart ≡ provisioning) ──
    println!("[phase 3] killing the active optimizer; its capability evaporates …");
    let (mut provs, mut depots): (Vec<Option<tokio::task::JoinHandle<()>>>, Vec<Option<&coop::common::Depot>>) = (
        vec![Some(prov_a), Some(prov_b), None],
        vec![Some(&provider_a), Some(&provider_b), Some(&provider_c)],
    );
    let slot = |id: &NodeId| if *id == provider_a.node_id() { 0 } else if *id == provider_b.node_id() { 1 } else { 2 };
    let i = slot(&active);
    if let Some(h) = provs[i].take() { h.abort(); }
    if let Some(d) = depots[i].take() { d.shutdown().await; }

    // Seed wave 2 — a fresh backlog that can only be served after the standby re-provisions.
    for id in (N_DONATIONS + 1)..=(2 * N_DONATIONS) {
        let d = Donation::new(id, "spitalfields", "surplus bread", "tower-hamlets");
        seeder_ts.put(LANE, d.to_bytes()).await?;
    }

    // The surviving provider sees the demand return (the dead node's cap/ evaporated) and re-provisions.
    let healed = wait_until(45, || {
        worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize"))
            .iter().any(|(id, _)| *id != active)
    }).await;
    assert!(healed, "the standby provider must re-provide route/optimize after the active one dies");
    println!("[phase 3] standby re-provisioned route/optimize — capability restored with no coordinator");

    for _ in 0..N_DONATIONS {
        process_one(&worker.agent, &worker_ts, &invoke_kind).await?;
    }
    let done_total = worker_ts.depth(Some(DONE)).await?.first().map(|s| s.depth).unwrap_or(0);
    println!("[phase 3] worker drained wave 2 → {done_total} donations optimized in total");
    assert_eq!(done_total as u64, 2 * N_DONATIONS, "both waves fully optimized across the failover");

    // ── Phase 4 — wave 3: shadow, then accept (D20) ──────────────────────────────
    let incumbent = worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize"))
        .into_iter().next().map(|(id, _)| id).expect("the healed optimizer");
    // Liveness-aware resolution for wave 3: the dead provider's advertisement lingers until its
    // TTL, and "resolves the incumbent only" is a claim about the shadow, not about that window.
    let live_optimizers = || {
        let mut ids: Vec<NodeId> = worker.agent.capabilities()
            .resolve(&CapFilter::new("route", "optimize").with_max_age(Duration::from_secs(20)))
            .into_iter().map(|(id, _)| id).collect();
        ids.sort_by_key(|id| id.to_string());
        ids
    };
    let settled = wait_until(60, || live_optimizers() == vec![incumbent.clone()]).await;
    assert!(settled, "exactly one live optimizer before wave 3: {:?}", live_optimizers());
    println!("[phase 4] incumbent optimizer: {}; provider-c comes online; an agent publishes a PROPOSED v2 under its own key …", name_of(&incumbent));
    provs[2] = Some(spawn_provisioner(Arc::clone(&provider_c.agent)));
    assert!(publish_installable(&buffer.agent.kv(), &v2_proposed()));
    let shadow = CapFilter::new("route", shadow_name("optimize"));
    let shadowed = wait_until(40, || !worker.agent.capabilities().resolve(&shadow).is_empty()).await;
    assert!(shadowed, "the proposal must load into the shadow lane (route/optimize.shadow)");
    assert_eq!(live_optimizers(), vec![incumbent.clone()], "the worker's filter resolves the incumbent only — a shadow is never a provider");
    println!("[phase 4] v2 is live in the shadow lane on {}; route/optimize still resolves only {}",
        worker.agent.capabilities().resolve(&shadow).into_iter().map(|(id, _)| name_of(&id)).collect::<Vec<_>>().join("+"),
        name_of(&incumbent));

    // Wave 3a: the incumbent serves every donation; the proposal takes none of the demand.
    for id in (2 * N_DONATIONS + 1)..=(3 * N_DONATIONS) {
        let d = Donation::new(id, "new-covent-garden", "surplus veg", "wandsworth");
        seeder_ts.put(LANE, d.to_bytes()).await?;
    }
    let mut served_by_incumbent = 0u64;
    for _ in 0..N_DONATIONS {
        let (node, _) = process_one(&worker.agent, &worker_ts, &invoke_kind).await?;
        if node == incumbent { served_by_incumbent += 1; }
    }
    assert_eq!(served_by_incumbent, N_DONATIONS, "the incumbent's call count rose by the whole wave");
    // The shadow's output is recorded and compared — by calling it by name, which takes no demand.
    let (shadow_node, _) = worker.agent.capabilities().resolve(&shadow).into_iter().next().expect("a shadow");
    let sample = br#"{"kg": 1500}"#.to_vec();
    let shadow_kind: Arc<str> = Arc::from(cap_invoke_kind("route", &shadow_name("optimize")).as_str());
    let shadow_out = worker.agent.service().rpc_call(shadow_node, shadow_kind, sample.clone(), Duration::from_secs(5)).await?;
    let incumbent_out = worker.agent.service().rpc_call(incumbent.clone(), Arc::clone(&invoke_kind), sample.clone(), Duration::from_secs(5)).await?;
    println!("[phase 4] wave 3a: {served_by_incumbent}/{N_DONATIONS} served by the incumbent; shadow compared on a sample — incumbent {:?} vs shadow {:?}",
        String::from_utf8_lossy(&incumbent_out), String::from_utf8_lossy(&shadow_out));
    assert!(shadow_out.starts_with(b"{\"tonnes\""), "the shadow is the v2 component: {shadow_out:?}");

    // A forged acceptance — a stranger's key — changes nothing.
    let (_, _, reviewer, stranger) = keys();
    assert!(publish_installable(&buffer.agent.kv(), &v2_proposed().accepted_by(&stranger).expect("a signed proposal")));
    tokio::time::sleep(Duration::from_millis(1200)).await; // three provisioner ticks
    assert_eq!(live_optimizers(), vec![incumbent.clone()], "a forged acceptance does not promote the proposal");
    assert!(!worker.agent.capabilities().resolve(&shadow).is_empty(), "still a shadow under a forged acceptance");
    println!("[phase 4] a stranger's acceptance was published: the provisioners were not fooled — still a shadow, incumbent unchanged");

    // The reviewer accepts (the same catalogue line, co-signed); the operator retires v1.
    assert!(publish_installable(&buffer.agent.kv(), &v2_proposed().accepted_by(&reviewer).expect("a signed proposal")));
    assert!(buffer.agent.kv().delete(v1_entry().kv_key()));
    let withdrawn = wait_until(20, || worker.agent.capabilities().resolve(&shadow).is_empty()).await;
    assert!(withdrawn, "accepted: the shadow is withdrawn");
    println!("[phase 4] the reviewer accepted v2 and the operator retired v1: shadow withdrawn; killing the incumbent …");
    let i = slot(&incumbent);
    if let Some(h) = provs[i].take() { h.abort(); }
    if let Some(d) = depots[i].take() { d.shutdown().await; }
    for id in (3 * N_DONATIONS + 1)..=(4 * N_DONATIONS) {
        seeder_ts.put(LANE, Bytes::from(format!(r#"{{"kg": {}}}"#, id * 250))).await?;
    }
    let promoted = wait_until(45, || {
        worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize")).iter().any(|(id, _)| *id != incumbent)
    }).await;
    assert!(promoted, "the last standby must provision the ACCEPTED v2 to serve wave 3b");
    let mut tonnes = 0u64;
    for _ in 0..N_DONATIONS {
        let (node, reply) = process_one(&worker.agent, &worker_ts, &invoke_kind).await?;
        assert_ne!(node, incumbent);
        if reply.starts_with(b"{\"tonnes\"") { tonnes += 1; }
    }
    assert_eq!(tonnes, N_DONATIONS, "wave 3b is served by the accepted v2 — its replies say so");
    let done_all = worker_ts.depth(Some(DONE)).await?.first().map(|s| s.depth).unwrap_or(0);
    assert_eq!(done_all as u64, 4 * N_DONATIONS);
    println!("[phase 4] wave 3b: {tonnes}/{N_DONATIONS} served by the accepted v2 on {} — shadow-then-accept complete", 
        worker.agent.capabilities().resolve(&CapFilter::new("route", "optimize")).into_iter().map(|(id, _)| name_of(&id)).collect::<Vec<_>>().join("+"));

    println!("\nAll assertions passed — buffered, self-provisioned (WASM), drained, self-healed across a provider death, and a proposed v2 shadowed then accepted (D20).");

    for h in provs.into_iter().flatten() { h.abort(); }
    worker.shutdown().await;
    for d in depots.into_iter().flatten() { d.shutdown().await; }
    seeder.shutdown().await;
    buffer.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    Ok(())
}
