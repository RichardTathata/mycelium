//! `mycelium-stem` — a stem node: starts a Mycelium node from a config file, declares
//! everything in a **unit file**, and provisions what the declarations call for
//! (`docs/plans/design-time-tooling.md` §13; `Stem` in this crate).
//!
//! ```text
//! mycelium-stem --units <unit.toml> [-c <gossip.toml>] [-p <port>] [--host <ip>] [-r <peers>]
//!               [--library <dir>] [--librarian <manifest> --publisher ed25519:<hex>]
//!               [--tick-ms <n>] [--self-elect <p>] [--trace-dir <dir>]
//! ```
//!
//! With `--library` the node reads artifact bytes from that directory (a mounted volume, or its
//! own store); without it, bytes are pulled over the mesh from a librarian discovered through
//! the capability ring. With `--librarian` the node also **takes the librarian role** over that
//! library and manifest, serving the bytes and reconciling the manifest to the gossiped
//! catalogue — one image, every role.

use std::sync::Arc;
use std::time::Duration;

use mycelium::{GossipAgent, GossipConfig, NodeCapabilityConfig, NodeId};
use mycelium_wasm_host::{spawn_librarian, FsLibrarySource, LibrarianConfig, Stem, StemOptions, StemSource};

fn usage() -> ! {
    eprintln!(
        "Usage: mycelium-stem --units <unit.toml> [-c <gossip.toml>] [-p <port>] [--host <ip>] [-r <peers>]\n\
         \n\
         Options:\n\
             --units <file>             the unit file to declare from (required)\n\
         -c, --config <file>            GossipConfig TOML (default: env overrides on defaults)\n\
         -p, --port <port>              bind port\n\
             --host <ip>                bind address\n\
         -r, --peers <ip:port,…>        bootstrap peers\n\
             --library <dir | url>      read artifact bytes from this directory, or from an object store by URL\n\
                                        (s3://…, gs://…, file:///…; needs the object_store feature; default: mesh pull)\n\
             --manifest-source <url>    with --librarian: read the manifest from the store at this URL, not a file\n\
             --stage-dir <dir>          where a mesh pull stages what it fetches (default: <placement_root>/stage)\n\
             --librarian <manifest>     also take the librarian role over --library and this manifest\n\
             --publisher ed25519:<hex>  the manifest's publisher key (with --librarian)\n\
             --tick-ms <n>              provisioner tick (default 500)\n\
             --self-elect <p>           self-election probability per round (default 0.5)\n\
             --trace-dir <dir>          write the decision trace (decisions.jsonl + its counters) there on shutdown\n\
         -h, --help"
    );
    std::process::exit(2)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    let mut units_path: Option<String> = None;
    let mut config_path: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut host: Option<String> = None;
    let mut peers: Option<String> = None;
    let mut library: Option<String> = None;
    let mut librarian: Option<String> = None;
    let mut publisher: Option<String> = None;
    let mut tick_ms: u64 = 500;
    let mut self_elect: f64 = 0.5;
    let mut trace_dir: Option<String> = None;
    let mut manifest_source: Option<String> = None;
    let mut stage_dir: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--units" => units_path = Some(val()),
            "-c" | "--config" => config_path = Some(val()),
            "-p" | "--port" => port = Some(val().parse().unwrap_or_else(|_| usage())),
            "--host" => host = Some(val()),
            "-r" | "--peers" => peers = Some(val()),
            "--library" => library = Some(val()),
            "--librarian" => librarian = Some(val()),
            "--publisher" => publisher = Some(val()),
            "--tick-ms" => tick_ms = val().parse().unwrap_or_else(|_| usage()),
            "--self-elect" => self_elect = val().parse().unwrap_or_else(|_| usage()),
            "--trace-dir" => trace_dir = Some(val()),
            "--manifest-source" => manifest_source = Some(val()),

            "--stage-dir" => stage_dir = Some(val()),
            _ => usage(),
        }
    }
    let Some(units_path) = units_path else { usage() };
    let units = NodeCapabilityConfig::load_from_file(&units_path)?;

    let mut config = match config_path {
        Some(p) => GossipConfig::load_from_file(p)?,
        None => {
            let mut c = GossipConfig::default();
            c.apply_env_overrides()?;
            c
        }
    };
    if let Some(p) = port {
        config.bind_port = p;
    }
    if let Some(h) = host {
        config.bind_address = h;
    }
    if let Some(p) = peers {
        config.bootstrap_peers = p.split(',').map(|s| s.trim().parse::<NodeId>()).collect::<Result<_, _>>()?;
    }
    config.validate()?;

    let source = match &library {
        Some(l) if l.contains("://") => StemSource::Store { url: l.clone() },
        Some(dir) => StemSource::Library(dir.into()),
        None => StemSource::Mesh { timeout: Duration::from_secs(5) },
    };
    // The decision trace (plan I5): off unless asked for; written as `decisions.jsonl` beside its
    // counters on shutdown, so a reader knows what the trace is missing.
    let sink = trace_dir.as_ref().map(|_| Arc::new(mycelium::decision::DecisionSink::default()));
    let opts = StemOptions {
        source,
        tick: Duration::from_millis(tick_ms.max(50)),
        self_elect_p: self_elect.clamp(0.0, 1.0),
        trace: sink.clone(),
        stage_dir: stage_dir.as_ref().map(std::path::PathBuf::from),
        ..StemOptions::default()
    };

    // The librarian's publisher key, parsed once: the librarian owns it, and (A3) the gateway
    // publish route refuses entries under it, since the librarian's manifest would tombstone them.
    let librarian_publisher: Option<[u8; 32]> = match (&librarian, &publisher) {
        (Some(_), Some(p)) => {
            let key = p.strip_prefix("ed25519:").ok_or("--librarian needs --publisher ed25519:<hex>")?;
            let bytes = (0..32)
                .map(|i| u8::from_str_radix(key.get(2 * i..2 * i + 2).unwrap_or("zz"), 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| "--publisher must be ed25519:<64 hex>")?;
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            Some(k)
        }
        (Some(_), None) => return Err("--librarian needs --publisher ed25519:<hex>".into()),
        _ => None,
    };

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let node = NodeId::new(&config.bind_address, config.bind_port)?;
        let agent = Arc::new(GossipAgent::new(node, config));
        // A3: the gateway publish route, before start(), from the hosts table's trusted keys.
        #[cfg(feature = "gateway")]
        if let Some(h) = &units.hosts {
            let trusted = h
                .trusted_publishers
                .iter()
                .map(|s| mycelium_wasm_host::publisher_from_str(s))
                .collect::<Result<Vec<_>, _>>()?;
            if !trusted.is_empty() {
                agent.with_http_routes(mycelium_wasm_host::artifact_router(Arc::clone(&agent), trusted, librarian_publisher));
            }
        }
        agent.start().await?;

        let _librarian = match (&librarian, &library) {
            (Some(manifest), Some(l)) if l.contains("://") => {
                // Zero-gaps Z1: a librarian over a store — the manifest from the store, the bytes
                // mirrored to a stage by ranged pull and served from there (D1, Q1).
                #[cfg(feature = "object_store")]
                {
                    let publisher = librarian_publisher.expect("checked above");
                    let egress = agent.egress_policy().clone();
                    let store = Arc::new(mycelium_wasm_host::ObjectStoreFetcher::from_url(l, egress.clone())?);
                    let stage_dir = stage_dir.clone().map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("mycelium-librarian-stage"));
                    let staged = Arc::new(mycelium_wasm_host::DiskStagedSource::open(Arc::clone(&store) as Arc<_>, &stage_dir)?);
                    let manifest_url = manifest_source.clone().unwrap_or_else(|| l.clone());
                    let manifest_store: Arc<dyn mycelium_wasm_host::ManifestSource> =
                        Arc::new(mycelium_wasm_host::ObjectStoreFetcher::from_url(&manifest_url, egress)?);
                    // Mirror: every sync, stage what the manifest names, so this node answers for it.
                    let (mirror_store, mirror_staged) = (Arc::clone(&store), Arc::clone(&staged));
                    tokio::spawn(async move {
                        // Through the timer seam (item 6), one stream for the mirror.
                        let mut tick = mycelium::sim_seam::interval_ms("librarian/mirror", 5_000, tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tick.tick().await;
                            if let Ok(m) = mirror_store.read_manifest().await {
                                let ids: Vec<_> = m.entries().iter().map(|e| e.artifact).collect();
                                mirror_staged.stage_all(&ids).await;
                            }
                        }
                    });
                    Some(spawn_librarian(
                        Arc::clone(&agent),
                        staged as Arc<_>,
                        LibrarianConfig { manifest_path: manifest.into(), publisher, sync_interval: Duration::from_secs(5), manifest_source: Some(manifest_store) },
                    ))
                }
                #[cfg(not(feature = "object_store"))]
                {
                    let _ = manifest;
                    return Err("a store URL needs a stem built with the `object_store` feature".into());
                }
            }
            (Some(manifest), Some(dir)) => {
                let publisher = librarian_publisher.expect("checked above");
                let manifest_store: Option<Arc<dyn mycelium_wasm_host::ManifestSource>> = match &manifest_source {
                    #[cfg(feature = "object_store")]
                    Some(url) => Some(Arc::new(mycelium_wasm_host::ObjectStoreFetcher::from_url(url, agent.egress_policy().clone())?)),
                    #[cfg(not(feature = "object_store"))]
                    Some(_) => return Err("--manifest-source needs a stem built with the `object_store` feature".into()),
                    None => None,
                };
                Some(spawn_librarian(
                    Arc::clone(&agent),
                    Arc::new(FsLibrarySource::open(dir)?) as Arc<_>,
                    LibrarianConfig { manifest_path: manifest.into(), publisher, sync_interval: Duration::from_secs(5), manifest_source: manifest_store },
                ))
            }
            (Some(_), None) => return Err("--librarian needs --library".into()),
            _ => None,
        };

        let stem = Stem::start(Arc::clone(&agent), &units, opts)?;
        tracing::info!(units = %units_path, hosting = units.hosts.is_some(), "stem node up");
        tokio::signal::ctrl_c().await?;
        tracing::info!("shutting down");
        stem.stop().await;
        if let (Some(dir), Some(sink)) = (&trace_dir, &sink) {
            let dir = std::path::Path::new(dir);
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(mycelium::decision::DECISION_ATTACHMENT), sink.to_jsonl())?;
            std::fs::write(dir.join("decisions.stats.json"), sink.stats_json())?;
            // The coverage manifest beside it (G8): which rules this stem could have recorded.
            if let Ok(cat) = mycelium::rule::Catalogue::gather(&[mycelium::rules::RULES, mycelium_wasm_host::rules::RULES]) {
                std::fs::write(dir.join("coverage.json"), mycelium::decision::coverage_manifest(&cat.rules))?;
            }
            tracing::info!(dir = %dir.display(), held = sink.stats().held, dropped = sink.dropped(), "decision trace written");
        }
        agent.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}
