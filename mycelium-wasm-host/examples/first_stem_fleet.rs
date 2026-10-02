//! First stem fleet: a signed, checked-in echo component, three generic hosts,
//! a floor of two providers, invocation by discovery, and graceful recovery.
//! Run: cargo run -p mycelium-wasm-host --example first_stem_fleet
//! Tutorial: docs/guide/tutorials/01-first-stem-fleet.md
use ed25519_dalek::SigningKey;
use mycelium::test_util::alloc_port;
use mycelium::{CapFilter, Capability, GossipAgent, NodeCapabilityConfig, NodeId};
use mycelium_wasm_host::{
    FsLibrarySource, InstallableEntry, LibrarianConfig, MANIFEST_FILE, Manifest, Stem, StemOptions,
    StemSource, cap_invoke_kind, spawn_librarian,
};
use std::{sync::Arc, time::Duration};
const ECHO_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/echo_component.wasm");
async fn agent(port: u16, bootstrap: Option<u16>) -> Arc<GossipAgent> {
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cfg = mycelium::GossipConfig {
        bind_port: port,
        bootstrap_peers: bootstrap
            .map(|b| vec![NodeId::new("127.0.0.1", b).unwrap()])
            .unwrap_or_default(),
        health_check_interval_secs: 2,
        ..Default::default()
    };
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("agent start");
    a
}

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

#[tokio::main]
async fn main() {
    // ── the library and its librarian ───────────────────────────────────────
    let lib_dir = std::env::temp_dir().join(format!(
        "mycelium-first-fleet-{}-{}",
        std::process::id(),
        alloc_port()
    ));
    let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let artifact = lib.store(ECHO_COMPONENT).unwrap();
    let entry = InstallableEntry::new(Capability::new("demo", "echo"), artifact)
        .with_cost(ECHO_COMPONENT.len() as u64, 1)
        .signed_by(&key);
    Manifest::from_entries(vec![entry])
        .save(&lib_dir.join(MANIFEST_FILE))
        .unwrap();
    let seed = agent(alloc_port(), None).await;
    let _librarian = spawn_librarian(
        Arc::clone(&seed),
        Arc::clone(&lib) as Arc<_>,
        LibrarianConfig {
            manifest_path: lib_dir.join(MANIFEST_FILE),
            publisher: key.verifying_key().to_bytes(),
            sync_interval: Duration::from_millis(200),
            manifest_source: None,
        },
    );
    let publisher_hex: String = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // ── one unit file, three stems ──────────────────────────────────────────
    let units_toml = format!(
        r#"principal = "stem"

[hosts]
kinds = ["wasm-component"]
trusted_publishers = ["ed25519:{publisher_hex}"]

[[presence]]
ns = "demo"
name = "echo"
min_providers = 2
max_providers = 2

[[requirement]]
ns = "demo"
name = "echo"
"#
    );
    println!("Declaration (demo key; never use in production):\n{units_toml}");
    let units = NodeCapabilityConfig::from_toml_str(&units_toml).unwrap();
    let opts = StemOptions {
        source: StemSource::Mesh {
            timeout: Duration::from_secs(5),
        },
        tick: Duration::from_millis(300),
        self_elect_p: 1.0,
        declare_interval: Duration::from_secs(2),
        reprobe_every: Duration::from_secs(1),
    };
    let mut fleet: Vec<(Arc<GossipAgent>, Stem)> = Vec::new();
    for _ in 0..3 {
        let a = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let s = Stem::start(Arc::clone(&a), &units, opts.clone()).unwrap();
        fleet.push((a, s));
    }
    let filter = CapFilter::new("demo", "echo");
    let live = |n: &Arc<GossipAgent>| n.capabilities().resolve(&filter).len();

    let converged = wait_until(90, || live(&seed) == 2).await;
    assert!(
        converged,
        "the fleet converges to the presence floor: {} live",
        live(&seed)
    );
    // A stem publishes its hosted count on its own tick, after the advertisement the seed
    // already saw — so this is waited for too, not read once.
    let hosts_of = |fleet: &Vec<(Arc<GossipAgent>, Stem)>| -> Vec<usize> {
        fleet
            .iter()
            .enumerate()
            .filter(|(_, (_, s))| s.hosted_count() > 0)
            .map(|(i, _)| i)
            .collect()
    };
    assert!(
        wait_until(30, || hosts_of(&fleet).len() == 2).await,
        "exactly two stems host: {:?}",
        hosts_of(&fleet)
    );
    let hosting = hosts_of(&fleet);
    println!("PASS: two generic hosts installed demo/echo; one remains available");
    let provider = seed.capabilities().resolve(&filter)[0].0.clone();
    let reply = seed
        .service()
        .rpc_call(
            provider,
            cap_invoke_kind("demo", "echo"),
            b"hello stem".to_vec(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(reply.as_ref(), b"hello stem");
    println!("PASS: discovered provider echoed hello stem");

    // ── the declared-versus-observed comparison ─────────────────────────────
    let description = mycelium::wire_check::ArtifactDescription::from_toml_str(
        "kind=\"wasm-component\"\n[provides]\nns=\"demo\"\nname=\"echo\"\n",
    )
    .unwrap();
    let declared: Vec<mycelium::wire_check::Unit> = (0..3)
        .map(|i| mycelium::wire_check::Unit {
            name: format!("stem-{i}"),
            config: units.clone(),
        })
        .collect();
    let report = mycelium::wire_check::check(
        &declared,
        &[("echo-component".to_string(), description)],
        &mycelium::wire_check::CheckOptions::default(),
    );
    assert_eq!(
        report.exit_code(),
        0,
        "the declarations say the floor is hostable: {}",
        report.render_text()
    );
    let mut observed = Vec::new();
    for (node, _cap) in seed.capabilities().resolve(&filter) {
        let (i, _) = fleet
            .iter()
            .enumerate()
            .find(|(_, (a, _))| a.node_id() == &node)
            .expect("a provider is one of the stems");
        observed.push(serde_json::json!({"node":node.to_string(), "unit":format!("stem-{i}"), "capability":"demo/echo"}));
        assert!(
            report.units[i]
                .hosts_kinds
                .contains(&"wasm-component".to_string()),
            "what bound was declared hostable"
        );
    }

    // Optional explicit export for the consumer tutorial. Refuse an existing directory.
    if let Some(dir) = std::env::args().nth(1) {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir(&dir).expect("export directory must not exist");
        std::fs::write(dir.join("declared.json"), report.render_json()).unwrap();
        std::fs::write(
            dir.join("observed.json"),
            serde_json::to_vec_pretty(&observed).unwrap(),
        )
        .unwrap();
        println!(
            "Exported declarations and one observer's snapshot to {}",
            dir.display()
        );
    }
    println!("PASS: observed providers map to units that declare WASM hosting");

    // ── stop one host: the standby brings the floor back ────────────────────
    let victim = hosting[0];
    let (dead_agent, dead_stem) = fleet.remove(victim);
    let removed = dead_agent.node_id().clone();
    dead_stem.stop().await;
    dead_agent.shutdown().await;
    // Recovery may happen between observations; do not require seeing a transient gap.
    let rehealed = wait_until(90, || {
        let providers = seed.capabilities().resolve(&filter);
        providers.len() == 2 && providers.iter().all(|(node, _)| node != &removed)
    })
    .await;
    assert!(rehealed, "the standby re-provisions: {} live", live(&seed));
    assert!(
        wait_until(30, || hosts_of(&fleet).len() == 2).await,
        "both remaining stems now host: {:?}",
        hosts_of(&fleet)
    );

    println!("PASS: remaining hosts restored the declared floor after graceful removal");
    for (a, s) in fleet {
        s.stop().await;
        a.shutdown().await;
    }
    seed.shutdown().await;
    let _ = std::fs::remove_dir_all(&lib_dir);
}
