//! End-to-end: a real WASM guest component (`tests/fixtures/echo_component.wasm`, built from
//! `tests/fixtures/echo-component/`) is instantiated against a live node and invoked. This proves
//! the host⇄component boundary actually *executes* — the guest's `kv` import crosses into the
//! node's store, confined to its subtree — not merely that the wiring type-checks.
//!
//! The `.wasm` is committed so CI needs no wasm toolchain; regenerate with the fixture's `build.sh`.

use mycelium::{CapFilter, Capability, GossipAgent, GossipConfig, NodeId};
use mycelium_wasm_host::{
    ArtifactId, HostState, InMemorySource, InstallableCatalog, InstallableEntry, WasmHost,
};
use std::sync::Arc;

const ECHO_COMPONENT: &[u8] = include_bytes!("fixtures/echo_component.wasm");

/// The core's bind-verified, process-unique loopback allocator (`mycelium::test_util::alloc_port`,
/// the `test-util` feature) — retires the old bind-:0-and-drop TOCTOU flake class.
fn alloc_port() -> u16 {
    mycelium::test_util::alloc_port()
}

async fn live_agent() -> Arc<GossipAgent> {
    // Retry on bind failure (alloc_port has a TOCTOU window; parallel tests can race for a freed
    // port). A fresh port per attempt removes the AddrInUse flake.
    for _ in 0..16 {
        let port = alloc_port();
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let agent = Arc::new(GossipAgent::new(id, cfg));
        if agent.start().await.is_ok() {
            return agent;
        }
    }
    panic!("could not bind a gossip port after 16 attempts");
}

#[tokio::test]
async fn echo_component_executes_and_its_kv_import_crosses_the_confined_boundary() {
    let agent = live_agent().await;
    let host = WasmHost::new().expect("engine");
    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());

    let mut instance = host.instantiate(ECHO_COMPONENT, state).expect("instantiate real component");

    // The guest stores the payload via kv.set, reads it back, and echoes it.
    let out = instance
        .invoke("greet", b"hello from the host".to_vec())
        .expect("no host/ABI trap")
        .expect("guest returned success");
    assert_eq!(out, b"hello from the host", "component echoed the payload");

    // The guest's kv.set actually crossed into the node's store — confined to comp/{node}/nlp/.
    let abs = format!("comp/{}/nlp/last-input", agent.node_id());
    assert_eq!(
        agent.kv().get(&abs).map(|b| b.to_vec()),
        Some(b"hello from the host".to_vec()),
        "guest write landed in the confined component subtree"
    );

    agent.shutdown().await;
}

#[tokio::test]
async fn provision_pulls_verifies_and_runs_a_real_component() {
    let agent = live_agent().await;
    let host = WasmHost::new().expect("engine");

    // Publish the component into a content-addressed source, then provision by id.
    let mut catalog = InMemorySource::new();
    let id: ArtifactId = catalog.insert(ECHO_COMPONENT.to_vec());

    let state = HostState::new(agent.node_id().clone(), "vision", agent.kv(), agent.mesh());
    let mut instance = host.provision(&catalog, &id, state).expect("provision (fetch+verify+instantiate)");

    let out = instance.invoke("ping", b"42".to_vec()).expect("no trap").expect("guest ok");
    assert_eq!(out, b"42");

    agent.shutdown().await;
}

#[tokio::test]
async fn fuel_budget_bounds_a_component_invocation() {
    let agent = live_agent().await;

    // A tiny fuel budget: the component's handle runs past it and traps (no hang).
    let starved = WasmHost::with_fuel_per_call(1).expect("engine");
    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let mut inst = starved.instantiate(ECHO_COMPONENT, state).expect("instantiate (unlimited init)");
    let err = inst.invoke("go", b"x".to_vec());
    assert!(
        matches!(err, Err(mycelium_wasm_host::WasmHostError::FuelExhausted { budget: 1 })),
        "a starved invocation is stopped at its budget, by name (D19), got {err:?}"
    );

    // An ample budget: the same call completes normally.
    let ample = WasmHost::with_fuel_per_call(1_000_000_000).expect("engine");
    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let mut inst = ample.instantiate(ECHO_COMPONENT, state).expect("instantiate");
    let out = inst.invoke("go", b"plenty".to_vec()).expect("no trap").expect("guest ok");
    assert_eq!(out, b"plenty");

    agent.shutdown().await;
}

#[tokio::test]
async fn provision_for_resolves_a_requirement_then_pulls_and_runs_the_match() {
    // The full M15→M12 path: a requirement (CapFilter) is resolved against an installable
    // catalog to pick an ArtifactId, which is then pulled + verified + instantiated.
    let agent = live_agent().await;
    let host = WasmHost::new().expect("engine");

    // The artifact bytes live in a content-addressed source...
    let mut source = InMemorySource::new();
    let id: ArtifactId = source.insert(ECHO_COMPONENT.to_vec());

    // ...and the catalog declares what installing it would provide, keyed by that id.
    let mut catalog = InstallableCatalog::new();
    catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id).with_cost(52_266, 1));

    let state = HostState::new(agent.node_id().clone(), "text", agent.kv(), agent.mesh());

    // Resolve "I need text/echo" → pick the artifact → pull + verify + instantiate.
    let mut instance = host
        .provision_for(&catalog, &CapFilter::new("text", "echo"), &source, state)
        .expect("provision_for ok")
        .expect("a catalog entry satisfied the requirement");
    let out = instance.invoke("run", b"resolved!".to_vec()).expect("no trap").expect("guest ok");
    assert_eq!(out, b"resolved!");

    // A requirement nothing provides → Ok(None), no error (the loop simply doesn't fire).
    let state2 = HostState::new(agent.node_id().clone(), "text", agent.kv(), agent.mesh());
    let none = host
        .provision_for(&catalog, &CapFilter::new("audio", "transcribe"), &source, state2)
        .expect("provision_for ok");
    assert!(none.is_none(), "unsatisfiable requirement yields Ok(None)");

    agent.shutdown().await;
}

/// D19 at the host level: the budget is an instance property on a metered engine, so the same
/// component bytes run stopped under one budget and unbounded under none — it is the key that
/// decides, not the component. And a budget on an unmetered host is refused by name.
#[tokio::test]
async fn a_metered_host_gives_each_instance_its_own_budget() {
    let agent = live_agent().await;
    let host = WasmHost::metered().expect("engine");
    assert!(host.is_metered());

    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let mut starved = host.instantiate_with_fuel(ECHO_COMPONENT, state, Some(1)).expect("instantiate");
    let err = starved.invoke("go", b"x".to_vec());
    assert!(
        matches!(err, Err(mycelium_wasm_host::WasmHostError::FuelExhausted { budget: 1 })),
        "stopped at its budget, by name: {err:?}"
    );
    assert_eq!(starved.fuel_per_call(), Some(1));

    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let mut unbounded = host.instantiate_with_fuel(ECHO_COMPONENT, state, None).expect("instantiate");
    let out = unbounded.invoke("go", b"plenty".to_vec()).expect("no trap").expect("guest ok");
    assert_eq!(out, b"plenty");
    assert_eq!(unbounded.fuel_per_call(), None);

    let plain = WasmHost::new().expect("engine");
    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let err = plain.instantiate_with_fuel(ECHO_COMPONENT, state, Some(1_000)).err().expect("refused");
    assert!(err.to_string().contains("metered host"), "{err}");

    agent.shutdown().await;
}

/// Row D: a guest call is bounded in **time**, not only in instructions — an unmetered host's
/// call into a guest that never returns is interrupted at its deadline (wasmtime epoch
/// interruption) and the error names it. Seen failing first: with no epoch deadline on the
/// engine the call never returned (`the call returned within its bound` timed out).
#[tokio::test]
async fn a_guest_call_past_its_deadline_is_stopped_by_name() {
    const SPIN_COMPONENT: &[u8] = include_bytes!("fixtures/spin_component.wasm");
    let agent = live_agent().await;
    let host = WasmHost::new().expect("engine").with_call_deadline(Some(std::time::Duration::from_millis(200)));
    let state = HostState::new(agent.node_id().clone(), "nlp", agent.kv(), agent.mesh());
    let mut inst = host.instantiate(SPIN_COMPONENT, state).expect("instantiate");

    // On its own thread, so a call that never returns fails this test instead of hanging it.
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(inst.invoke("spin", b"x".to_vec()));
    });
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the call returned within its bound");
    let took = started.elapsed();
    assert!(
        matches!(res, Err(mycelium_wasm_host::WasmHostError::DeadlineExceeded { deadline_ms: 200 })),
        "a call past its deadline is stopped by name, got {res:?}"
    );
    assert!(took >= std::time::Duration::from_millis(200), "stopped before its deadline: {took:?}");
    assert!(took < std::time::Duration::from_secs(3), "stopped well after its deadline: {took:?}");
    agent.shutdown().await;
}
