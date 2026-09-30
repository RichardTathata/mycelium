//! The stem-examples driver (design-time-tooling.md §16, X2): the roles a co-op demo keeps as code
//! when its dynamic roles are stem nodes fed the demo's declaration directory — the buffer, the
//! seeder, the worker, and the asserts. It prints the **same markers** the code run prints, so
//! `make examples-both-ways` can grep one contract twice.
//!
//! It is also the fleet's bootstrap seed (the stems are started with `-r <driver>:57000`), a plain
//! node on the suite's private network — no TLS, unlike the co-op's `spawn_depot`, which binds
//! 127.0.0.1 under a shared CA and cannot join a container fleet.
//!
//! Run by `docker/docker-compose.stem-examples.yml`; not meant to be run by hand.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use coop::common::Donation;
use mycelium::{CapFilter, GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};

const LANE: &str = "optimize";
const DONE: &str = "done";
const N: u64 = 4;
const INVOKE: &str = "cap.invoke/route/optimize";

async fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    cond()
}

fn live_optimizers(agent: &GossipAgent) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = agent
        .capabilities()
        .resolve(&CapFilter::new("route", "optimize").with_max_age(Duration::from_secs(20)))
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.sort_by_key(|id| id.to_string());
    ids
}

async fn optimize(agent: &Arc<GossipAgent>, node: &NodeId, payload: Bytes) -> Result<Bytes, Box<dyn std::error::Error>> {
    let mut last = None;
    for attempt in 1..=5u32 {
        match agent.service().rpc_call(node.clone(), INVOKE, payload.clone(), Duration::from_secs(5)).await {
            Ok(b) => return Ok(b),
            Err(e) => {
                println!("[driver] optimize attempt {attempt} failed ({e}); retrying");
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Err(last.expect("an error").into())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).init();
    let demo = std::env::args().nth(1).unwrap_or_default();
    let host = std::env::var("STEM_DRIVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("STEM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57000);
    let cfg = GossipConfig { bind_address: host.clone(), bind_port: port, ..Default::default() };
    let agent = Arc::new(GossipAgent::new(NodeId::new(&host, port)?, cfg));
    agent.start().await?;
    println!("[driver] up at {host}:{port} — the fleet's bootstrap seed; demo {demo:?}");

    match demo.as_str() {
        "provisioning" => provisioning(agent).await,
        "catalog" => catalog(agent).await,
        other => Err(format!("unknown demo {other:?} (provisioning | catalog)").into()),
    }
}

/// The provisioning demo with provider-a and provider-b as stem nodes: buffer, seed, drain, kill
/// the active provider for real, drain again. Same markers as `examples/coop/src/bin/provisioning.rs`.
async fn provisioning(agent: Arc<GossipAgent>) -> Result<(), Box<dyn std::error::Error>> {
    let providers: HashMap<String, String> = std::env::var("STEM_PROVIDERS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| kv.split_once('=').map(|(ip, name)| (ip.to_string(), name.to_string())))
        .collect();
    let ts = TupleSpace::new(Arc::clone(&agent), TupleConfig {
        namespace: Arc::from("rescue"), role: TupleRole::Primary, persist: false, ..Default::default()
    }).await?;
    assert!(wait_until(60, || agent.peers().len() >= 2).await, "both provider stems must peer with the driver");
    println!("[phase 1] two stem providers peered; seeding {N} donations into lane '{LANE}' — no route/optimize provider exists yet");
    for id in 1..=N {
        ts.put(LANE, Donation::new(id, "borough-market", "surplus produce", "southwark").to_bytes()).await?;
    }
    let _req = agent.capabilities().declare_requirement(CapFilter::new("route", "optimize"), Duration::from_secs(600));
    assert!(wait_until(90, || !live_optimizers(&agent).is_empty()).await, "a stem must self-provision route/optimize from the declared requirement");
    println!("[phase 2] route/optimize self-provisioned by a stem node (WASM pulled from the library, verified, serving)");
    for _ in 0..N {
        let (id, payload) = ts.take(LANE, Duration::from_secs(10)).await?;
        let node = live_optimizers(&agent).into_iter().next().ok_or("no route/optimize provider")?;
        let out = optimize(&agent, &node, payload.clone()).await?;
        assert_eq!(out, payload, "the optimizer echoes (the fixture component)");
        ts.complete(id, DONE, out).await?;
    }
    let done1 = ts.depth(Some(DONE)).await?.first().map(|s| s.depth).unwrap_or(0);
    assert_eq!(done1 as u64, N, "wave 1 fully optimized");
    println!("[phase 2] worker drained wave 1 → {done1} donations optimized");

    let active = live_optimizers(&agent).into_iter().next().expect("a live optimizer");
    let active_ip = active.to_socket_addr().ip().to_string();
    let container = providers.get(&active_ip).cloned().unwrap_or_else(|| panic!("no container mapped for {active_ip} in STEM_PROVIDERS"));
    println!("[phase 3] killing the active optimizer ({container}); its capability evaporates …");
    let st = std::process::Command::new("docker").args(["kill", &container]).status()?;
    assert!(st.success(), "docker kill {container}");
    for id in (N + 1)..=(2 * N) {
        ts.put(LANE, Donation::new(id, "spitalfields", "surplus bread", "tower-hamlets").to_bytes()).await?;
    }
    assert!(wait_until(120, || live_optimizers(&agent).iter().any(|id| *id != active)).await,
        "the standby stem must re-provision route/optimize after the active one dies");
    println!("[phase 3] standby re-provisioned route/optimize — capability restored with no coordinator");
    for _ in 0..N {
        let (id, payload) = ts.take(LANE, Duration::from_secs(10)).await?;
        let node = live_optimizers(&agent).into_iter().find(|n| *n != active).ok_or("no live standby")?;
        let out = optimize(&agent, &node, payload).await?;
        ts.complete(id, DONE, out).await?;
    }
    let done2 = ts.depth(Some(DONE)).await?.first().map(|s| s.depth).unwrap_or(0);
    assert_eq!(done2 as u64, 2 * N, "both waves fully optimized across the failover");
    println!("[phase 3] worker drained wave 2 → {done2} donations optimized in total");
    println!("\nAll assertions passed — buffered, self-provisioned (WASM, from stem nodes fed the declaration directory), drained, and self-healed across a provider death.");
    agent.shutdown().await;
    Ok(())
}

/// The catalog demo with the librarian and the installer as stem nodes: the driver is the caller.
/// Phases 1–4 of `examples/coop/src/bin/catalog.rs`; the late joiner from a peer cache (phase 6)
/// has no stem equivalent yet.
async fn catalog(agent: Arc<GossipAgent>) -> Result<(), Box<dyn std::error::Error>> {
    assert!(wait_until(60, || agent.peers().len() >= 2).await, "the librarian and installer stems must peer with the driver");
    assert!(wait_until(60, || !agent.capabilities().resolve(&CapFilter::new("artifact", "librarian")).is_empty()).await,
        "the librarian stem advertises artifact/librarian");
    println!("[phase 1] librarian stem up (serving the signed library); installer stem up (hosting, no library of its own)");
    let _req = agent.capabilities().declare_requirement(CapFilter::new("route", "optimize"), Duration::from_secs(600));
    assert!(wait_until(120, || !live_optimizers(&agent).is_empty()).await,
        "the installer stem must discover the librarian, pull route/optimize over the mesh, verify and serve it");
    let node = live_optimizers(&agent).into_iter().next().expect("a provider");
    println!("[phase 2] route/optimize provisioned on {node} — pulled from the librarian over the mesh, verified by content address and provenance");
    let out = optimize(&agent, &node, Bytes::from_static(b"route me")).await?;
    assert_eq!(out.as_ref(), b"route me", "the installed component serves");
    println!("[phase 3] the caller invoked the installed optimizer and got its answer");
    println!("\nAll assertions passed — runtime-read bytes → signed library → librarian stem → discovered pull → provisioned on an installer stem → served. (The late joiner's peer-cache install, phase 6 of the code run, is not recut.)");
    agent.shutdown().await;
    Ok(())
}
