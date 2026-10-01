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
        "mcp_toolgrowth" => mcp_toolgrowth(agent).await,
        "model_deploy" => model_deploy(agent).await,
        other => Err(format!("unknown demo {other:?} (provisioning | catalog | mcp_toolgrowth)").into()),
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

    wave3(&agent, &ts, &providers, &active).await?;
    println!("\nAll assertions passed — buffered, self-provisioned (WASM, from stem nodes fed the declaration directory), drained, self-healed across a provider death, and a proposed v2 shadowed then accepted (D20).");
    agent.shutdown().await;
    Ok(())
}

/// Wave 3 (D20) as stems: an agent proposes v2, the stems shadow it, a stranger's acceptance
/// changes nothing, the reviewer's promotes it, the operator retires v1, the incumbents die, and
/// the late provider-c serves the accepted v2.
async fn wave3(
    agent: &Arc<GossipAgent>,
    ts: &TupleSpace,
    providers: &HashMap<String, String>,
    killed: &NodeId,
) -> Result<(), Box<dyn std::error::Error>> {
    use ed25519_dalek::SigningKey;
    use mycelium::Capability;
    use mycelium_wasm_host::{publish_installable, shadow_name, ArtifactId, FsLibrarySource, InstallableEntry, Manifest, MANIFEST_FILE};
    const V1: &[u8] = include_bytes!("../../../../mycelium-wasm-host/tests/fixtures/echo_component.wasm");
    const V2: &[u8] = include_bytes!("../../../../mycelium-wasm-host/tests/fixtures/unit_convert_component.wasm");
    let (agent_key, reviewer, stranger) =
        (SigningKey::from_bytes(&[0x43; 32]), SigningKey::from_bytes(&[0x44; 32]), SigningKey::from_bytes(&[0x45; 32]));
    let lib = std::path::Path::new("/lib");

    // Settle: the killed provider's advertisement ages out; what remains are the incumbents.
    assert!(wait_until(60, || !live_optimizers(agent).contains(killed)).await, "the killed provider ages out");
    let incumbents = live_optimizers(agent);
    assert!(!incumbents.is_empty(), "an incumbent serves route/optimize");
    let c_ip = std::env::var("STEM_PROVIDER_C_IP").unwrap_or_else(|_| "172.40.0.14".into());
    let c_node = NodeId::new(&c_ip, 57000)?;
    println!("[phase 4] incumbent(s): {}; provider-c comes online …", incumbents.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", "));
    let st = std::process::Command::new("docker")
        .args(["run", "-d", "--name", "mycelium-stem-provider-c", "--network", "mycelium-stem-net", "--ip", &c_ip,
               "-v", "mycelium-stem-lib:/lib", "mycelium-stem:test",
               "--units", "/repo/examples/units/provisioning/provider-c.toml", "--library", "/lib",
               "--host", &c_ip, "-p", "57000", "-r", &format!("{}:57000", std::env::var("STEM_DRIVER_HOST").unwrap_or_default()),
               "--tick-ms", "300"])
        .status()?;
    assert!(st.success(), "docker run provider-c");
    assert!(wait_until(60, || agent.peers().contains(&c_node)).await, "provider-c peers");

    // The agent's proposal: the bytes into the library (blobs only — the manifest is the
    // operator's), the signed proposed line into the catalogue.
    let v2_id = FsLibrarySource::open(lib)?.store(V2)?;
    assert_eq!(v2_id, ArtifactId::of(V2));
    let proposal = InstallableEntry::new(Capability::new("route", "optimize"), v2_id)
        .with_cost(V2.len() as u64, 1)
        .as_proposed()
        .signed_by(&agent_key);
    assert!(publish_installable(&agent.kv(), &proposal));
    println!("[phase 4] an agent published a PROPOSED v2 under its own key");
    let shadow = CapFilter::new("route", shadow_name("optimize"));
    assert!(wait_until(60, || !agent.capabilities().resolve(&shadow).is_empty()).await,
        "the proposal loads into the shadow lane (route/optimize.shadow)");
    assert_eq!(live_optimizers(agent), incumbents, "route/optimize resolves the incumbent(s) only — a shadow is never a provider");
    println!("[phase 4] v2 is live in the shadow lane; route/optimize still resolves only the incumbent(s)");

    // Wave 3a: the incumbents serve the whole wave; the shadow is compared on a sample by name.
    for id in (2 * N + 1)..=(3 * N) {
        ts.put(LANE, Donation::new(id, "new-covent-garden", "surplus veg", "wandsworth").to_bytes()).await?;
    }
    let mut served = 0u64;
    for _ in 0..N {
        let (id, payload) = ts.take(LANE, Duration::from_secs(10)).await?;
        let node = live_optimizers(agent).into_iter().next().ok_or("no optimizer")?;
        assert!(incumbents.contains(&node));
        let out = optimize(agent, &node, payload).await?;
        ts.complete(id, DONE, out).await?;
        served += 1;
    }
    let (shadow_node, _) = agent.capabilities().resolve(&shadow).into_iter().next().expect("a shadow");
    let sample = Bytes::from_static(br#"{"kg": 1500}"#);
    let shadow_kind = format!("cap.invoke/route/{}", shadow_name("optimize"));
    let shadow_out = agent.service().rpc_call(shadow_node, shadow_kind, sample.clone(), Duration::from_secs(5)).await?;
    assert!(shadow_out.starts_with(b"{\"tonnes\""), "the shadow is the v2 component: {shadow_out:?}");
    println!("[phase 4] wave 3a: {served}/{N} served by the incumbent(s); the shadow answered a sample with {:?}", String::from_utf8_lossy(&shadow_out));

    // A stranger's acceptance changes nothing.
    assert!(publish_installable(&agent.kv(), &proposal.clone().accepted_by(&stranger)?));
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(live_optimizers(agent), incumbents, "a forged acceptance does not promote the proposal");
    assert!(!agent.capabilities().resolve(&shadow).is_empty(), "still a shadow under a forged acceptance");
    println!("[phase 4] a stranger's acceptance was published: still a shadow, incumbent unchanged");

    // The reviewer's acceptance promotes it; the operator retires v1 from the manifest, and the
    // librarian tombstones its catalogue line.
    assert!(publish_installable(&agent.kv(), &proposal.clone().accepted_by(&reviewer)?));
    assert!(wait_until(30, || agent.capabilities().resolve(&shadow).is_empty()).await, "accepted: the shadow is withdrawn");
    let manifest_path = lib.join(MANIFEST_FILE);
    let m = Manifest::load(&manifest_path)?;
    let v1_id = ArtifactId::of(V1);
    let v1_key = m.entries().iter().find(|e| e.artifact == v1_id).map(|e| e.kv_key()).ok_or("v1 in the manifest")?;
    Manifest::from_entries(m.entries().iter().filter(|e| e.artifact != v1_id).cloned().collect()).save(&manifest_path)?;
    assert!(wait_until(30, || agent.kv().get(&v1_key).is_none()).await, "the librarian tombstones the retired v1 line");
    println!("[phase 4] the reviewer accepted v2 and the operator retired v1: shadow withdrawn; killing the incumbent(s) …");
    for node in &incumbents {
        let ip = node.to_socket_addr().ip().to_string();
        let name = providers.get(&ip).cloned().ok_or("no container mapped for an incumbent")?;
        let st = std::process::Command::new("docker").args(["kill", &name]).status()?;
        assert!(st.success(), "docker kill {name}");
    }

    // Wave 3b: only provider-c is left, and the only loadable entry is the accepted v2.
    for id in (3 * N + 1)..=(4 * N) {
        ts.put(LANE, Bytes::from(format!(r#"{{"kg": {}}}"#, id * 250))).await?;
    }
    assert!(wait_until(120, || live_optimizers(agent) == vec![c_node.clone()]).await,
        "provider-c must provision the ACCEPTED v2 to serve wave 3b");
    let mut tonnes = 0u64;
    for _ in 0..N {
        let (id, payload) = ts.take(LANE, Duration::from_secs(10)).await?;
        let out = optimize(agent, &c_node, payload).await?;
        if out.starts_with(b"{\"tonnes\"") {
            tonnes += 1;
        }
        ts.complete(id, DONE, out).await?;
    }
    assert_eq!(tonnes, N, "wave 3b is served by the accepted v2 — its replies say so");
    println!("[phase 4] wave 3b: {tonnes}/{N} served by the accepted v2 on provider-c — shadow-then-accept complete");
    Ok(())
}

/// The catalog demo with the librarian and the installer as stem nodes: the driver is the caller.
/// Phases 1–4 of `examples/coop/src/bin/catalog.rs`, then phase 6 as stems: the librarian is
/// killed and a late stem, started here on the suite's network, installs from the installer's
/// re-served cache.
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

    // Phase 6 as stems: the origin dies, a late joiner installs from the peer's cache.
    let librarian = std::env::var("STEM_LIBRARIAN_CONTAINER").unwrap_or_else(|_| "mycelium-stem-librarian".into());
    let late_ip = std::env::var("STEM_LATE_IP").unwrap_or_else(|_| "172.40.0.23".into());
    let installer_ip = std::env::var("STEM_INSTALLER_IP").unwrap_or_else(|_| "172.40.0.22".into());
    // The installer advertises itself as a cache holder once its cache holds the bytes.
    assert!(wait_until(60, || agent.capabilities().resolve(&CapFilter::new("artifact", "librarian")).len() >= 2).await,
        "the installer stem re-serves its verified cache (a second artifact/librarian holder)");
    println!("[phase 4] killing the librarian ({librarian}); the installer's cache is the only holder left …");
    let st = std::process::Command::new("docker").args(["kill", &librarian]).status()?;
    assert!(st.success(), "docker kill {librarian}");
    let st = std::process::Command::new("docker")
        .args(["run", "-d", "--name", "mycelium-stem-late", "--network", "mycelium-stem-net", "--ip", &late_ip,
               "mycelium-stem:test", "--units", "/repo/examples/units/catalog/late.toml",
               "--host", &late_ip, "-p", "57000", "-r", &format!("{installer_ip}:57000"), "--tick-ms", "300"])
        .status()?;
    assert!(st.success(), "docker run the late stem");
    println!("[phase 5] a late stem joined at {late_ip}, wanting a second optimizer");
    let late_node = NodeId::new(&late_ip, 57000)?;
    assert!(wait_until(120, || live_optimizers(&agent).contains(&late_node)).await,
        "the late joiner must install route/optimize from the installer's peer cache");
    let out = optimize(&agent, &late_node, Bytes::from_static(b"late route")).await?;
    assert_eq!(out.as_ref(), b"late route");
    println!("[late] joined after the origin died — installed from a peer cache and ran it");
    println!("\nAll assertions passed — runtime-read bytes → signed library → librarian stem → discovered pull → provisioned on an installer stem → served → origin killed → late stem installed from a peer cache.");
    agent.shutdown().await;
    Ok(())
}

/// The model demo with the librarian and the model host as stems and a pinned Ollama: the driver
/// is the app. The capability goes live only once the model host's `[[activation]]` (D21) created
/// the model in Ollama; the driver checks the governed SYSTEM prompt and generates real tokens.
async fn model_deploy(agent: Arc<GossipAgent>) -> Result<(), Box<dyn std::error::Error>> {
    let ollama = std::env::var("OLLAMA_HOST").map_err(|_| "OLLAMA_HOST must name the suite's Ollama")?;
    let post = |path: &str, body: &str| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let out = std::process::Command::new("curl")
            .args(["-sf", &format!("{ollama}{path}"), "-H", "Content-Type: application/json", "-d", body])
            .output()?;
        if !out.status.success() {
            return Err(format!("POST {path} failed: {}", String::from_utf8_lossy(&out.stderr)).into());
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    assert!(wait_until(60, || agent.peers().len() >= 2).await, "the librarian and model-host stems peer with the driver");
    let _req = agent.capabilities().declare_requirement(CapFilter::new("llm", "storyteller"), Duration::from_secs(900));
    println!("[app] needs llm/storyteller; the model host places the weights and the profile and activates them …");
    assert!(wait_until(300, || !agent.capabilities().resolve(&CapFilter::new("llm", "storyteller")).is_empty()).await,
        "llm/storyteller must go live: weights and profile placed, the profile rendered and created in Ollama, the probe passing");
    println!("[app] llm/storyteller is live — the model host's activation created the model in Ollama");
    let show = post("/api/show", r#"{"model":"coop-storyteller"}"#)?;
    let system = show["system"].as_str().unwrap_or_default().to_string();
    assert!(system.contains("newsletter storyteller"), "the deployed PROFILE's SYSTEM prompt is what Ollama runs: {show}");
    println!("[app] Ollama runs the governed SYSTEM prompt: {system:?}");
    let story = post(
        "/api/generate",
        r#"{"model":"coop-storyteller","prompt":"Once upon a time, on the night of the great surplus-bread rescue,","stream":false,"options":{"num_predict":48}}"#,
    )?;
    let text = story["response"].as_str().unwrap_or_default().trim().to_string();
    assert!(!text.is_empty(), "the deployed model generated real tokens: {story}");
    println!("\n[app] the deployed model speaks:\n      “{text}”\n");
    println!("All assertions passed — a real model (weights + governed profile) published, placed by a stem, activated into Ollama by its declaration, and generated real tokens.");
    agent.shutdown().await;
    Ok(())
}

/// The tool-growth demo with the library and the tool-host as stem nodes: the driver is the
/// agent. The installed `tool/unit-convert` component is bridged as an MCP tool by the stem's
/// runtime; the agent declares the need, finds the tool in KV, and calls it over `mcp.invoke`.
async fn mcp_toolgrowth(agent: Arc<GossipAgent>) -> Result<(), Box<dyn std::error::Error>> {
    const TOOL: &str = "unit-convert";
    assert!(wait_until(60, || agent.peers().len() >= 2).await, "the library and tool-host stems must peer with the driver");
    let tool_offered = |a: &GossipAgent| -> Option<NodeId> {
        a.kv().scan_prefix(&format!("tools/{TOOL}/")).into_iter().find_map(|(key, _)| {
            key.strip_prefix(&format!("tools/{TOOL}/")).and_then(|n| n.parse().ok())
        })
    };
    assert!(tool_offered(&agent).is_none(), "the tool is not offered before it's needed");
    println!("[llm-agent] '{TOOL}' is not in the fabric yet — declaring the requirement");
    let _req = agent.capabilities().declare_requirement(CapFilter::new("tool", TOOL), Duration::from_secs(600));
    assert!(wait_until(120, || tool_offered(&agent).is_some()).await,
        "the tool-host stem must install the component and bridge it as an MCP tool once demand appears");
    let provider = tool_offered(&agent).expect("a tool provider node");
    println!("[llm-agent] '{TOOL}' is now offered by {provider} — the converter code arrived over the mesh; invoking it over MCP");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": TOOL, "arguments": {"kg": 5000.0}},
    });
    let mut reply = None;
    for attempt in 1..=5u32 {
        match agent.service().rpc_call(provider.clone(), "mcp.invoke", call.to_string().into_bytes(), Duration::from_secs(10)).await {
            Ok(r) => { reply = Some(r); break; }
            Err(e) => { println!("[llm-agent] invoke attempt {attempt} failed ({e}); retrying"); tokio::time::sleep(Duration::from_millis(500)).await; }
        }
    }
    let reply = reply.ok_or("MCP invoke reply after retries")?;
    let resp: serde_json::Value = serde_json::from_slice(&reply)?;
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
    println!("[llm-agent] MCP tool returned: {text}");
    assert!(text.contains('5') && text.contains("tonnes"), "the converter must report 5 tonnes for 5000 kg — got {resp}");
    println!("\nAll assertions passed — the tool was missing, the need was declared, a stem installed the component and bridged it as an MCP tool, and the agent used it.");
    agent.shutdown().await;
    Ok(())
}
