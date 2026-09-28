//! `mycelium-stem` — a stem node: starts a Mycelium node from a config file, declares
//! everything in a **unit file**, and provisions what the declarations call for
//! (`docs/plans/design-time-tooling.md` §13; `Stem` in this crate).
//!
//! ```text
//! mycelium-stem --units <unit.toml> [-c <gossip.toml>] [-p <port>] [--host <ip>] [-r <peers>]
//!               [--library <dir>] [--librarian <manifest> --publisher ed25519:<hex>]
//!               [--tick-ms <n>] [--self-elect <p>]
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
             --library <dir>            read artifact bytes from this directory (default: mesh pull)\n\
             --librarian <manifest>     also take the librarian role over --library and this manifest\n\
             --publisher ed25519:<hex>  the manifest's publisher key (with --librarian)\n\
             --tick-ms <n>              provisioner tick (default 500)\n\
             --self-elect <p>           self-election probability per round (default 0.5)\n\
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
        Some(dir) => StemSource::Library(dir.into()),
        None => StemSource::Mesh { timeout: Duration::from_secs(5) },
    };
    let opts = StemOptions {
        source,
        tick: Duration::from_millis(tick_ms.max(50)),
        self_elect_p: self_elect.clamp(0.0, 1.0),
        ..StemOptions::default()
    };

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let node = NodeId::new(&config.bind_address, config.bind_port)?;
        let agent = Arc::new(GossipAgent::new(node, config));
        agent.start().await?;

        let _librarian = match (&librarian, &library) {
            (Some(manifest), Some(dir)) => {
                let key = publisher.as_deref().and_then(|s| s.strip_prefix("ed25519:")).ok_or("--librarian needs --publisher ed25519:<hex>")?;
                let bytes = (0..32)
                    .map(|i| u8::from_str_radix(key.get(2 * i..2 * i + 2).unwrap_or("zz"), 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|_| "--publisher must be ed25519:<64 hex>")?;
                let mut publisher = [0u8; 32];
                publisher.copy_from_slice(&bytes);
                Some(spawn_librarian(
                    Arc::clone(&agent),
                    Arc::new(FsLibrarySource::open(dir)?) as Arc<_>,
                    LibrarianConfig { manifest_path: manifest.into(), publisher, sync_interval: Duration::from_secs(5) },
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
        agent.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}
