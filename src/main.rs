use mycelium::{GossipAgent, NodeId};
use mycelium::config::GossipConfig;
use mycelium::error::GossipError;
use std::{error::Error, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let config = parse_args()?;

    // `GOSSIP_RECORD_BUNDLE_DIR` (builds with `sim` only): run under the replay seams on a
    // current-thread runtime and write a bundle at shutdown — the operator's capture path that
    // diagnostics.md used to imply and the tree did not have (doc-coverage run 17, code gap 3).
    #[cfg(feature = "sim")]
    if let Ok(dir) = std::env::var("GOSSIP_RECORD_BUNDLE_DIR") {
        return record::run_recorded(config, std::path::PathBuf::from(dir));
    }

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(config))
}

async fn run(config: GossipConfig) -> Result<(), Box<dyn Error>> {
    let node_id = NodeId::new(&config.bind_address, config.bind_port)?;

    let agent = Arc::new(GossipAgent::new(node_id, config));

    agent.start().await?;

    if std::env::args().any(|a| a == "-i" || a == "--interactive") {
        run_interactive(Arc::clone(&agent)).await?;
    } else {
        await_shutdown_signal().await?;
    }

    tracing::info!("Shutting down...");
    agent.shutdown().await;

    Ok(())
}

/// Parses CLI arguments and returns the resolved config.
/// Bootstrap peers from `--peers` are stored in `config.bootstrap_peers`.
fn parse_args() -> Result<GossipConfig, GossipError> {
    let mut args = std::env::args().skip(1);

    let mut config_path: Option<String> = None;
    let mut bind_port:   Option<u16>    = None;
    let mut bind_host:   Option<String> = None;
    let mut peers_arg:   Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                config_path = Some(
                    args.next()
                        .ok_or_else(|| GossipError::InvalidField { field: "-c/--config", reason: "missing config file path".into() })?,
                );
            }
            "-p" | "--port" => {
                let s = args.next()
                    .ok_or_else(|| GossipError::InvalidField { field: "-p/--port", reason: "missing port number".into() })?;
                bind_port = Some(s.parse().map_err(GossipError::Parse)?);
            }
            "--host" => {
                bind_host = Some(
                    args.next()
                        .ok_or_else(|| GossipError::InvalidField { field: "--host", reason: "missing host address".into() })?,
                );
            }
            "-r" | "--peers" => {
                peers_arg = Some(
                    args.next()
                        .ok_or_else(|| GossipError::InvalidField { field: "-r/--peers", reason: "missing peers argument".into() })?,
                );
            }
            "-i" | "--interactive" => {} // handled in main
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(GossipError::InvalidField { field: "argument", reason: format!("unknown argument: {arg}") }),
        }
    }

    let mut config = if let Some(path) = config_path {
        GossipConfig::load_from_file(path)?
    } else {
        let mut cfg = GossipConfig::default();
        cfg.apply_env_overrides()?;
        cfg
    };

    if let Some(port) = bind_port {
        config.bind_port = port;
    }
    if let Some(host) = bind_host {
        config.bind_address = host;
    }

    // CLI --peers overrides config-file bootstrap_peers.
    if let Some(peers_str) = peers_arg {
        config.bootstrap_peers = peers_str
            .split(',')
            .map(|s| s.trim().parse::<NodeId>())
            .collect::<Result<_, _>>()?;
    }
    // Self-filtering happens inside GossipAgent::new.

    config.validate()?;
    Ok(config)
}

/// Waits for Ctrl-C (all platforms) or SIGTERM (Unix).
async fn await_shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = sigterm.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

fn print_usage() {
    eprintln!(
        "Usage: mycelium [OPTIONS]\n\
         \n\
         Options:\n\
         -c, --config <file>      Load configuration from a TOML file\n\
         -p, --port <port>        Bind port (default: 8080)\n\
             --host <ip>          Bind IP address (default: 127.0.0.1)\n\
         -r, --peers <list>       Comma-separated bootstrap peers (IP:port,...)\n\
         -i, --interactive        Start an interactive REPL\n\
         -h, --help               Show this message"
    );
}

async fn run_interactive(agent: Arc<GossipAgent>) -> Result<(), GossipError> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    println!("Interactive mode. Commands:");
    println!("  set <key> <value>   store and gossip a value");
    println!("  get <key>           retrieve a local value");
    println!("  delete <key>        remove and gossip a tombstone");
    println!("  stats               show protocol state");
    println!("  exit                shut down");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    loop {
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout()).map_err(GossipError::Io)?;

        let line = match lines.next_line().await.map_err(GossipError::Io)? {
            Some(l) => l,
            None    => break,
        };

        let mut parts = line.split_whitespace();
        let cmd = match parts.next() {
            Some(c) => c,
            None => continue,
        };

        match cmd.to_lowercase().as_str() {
            "set" => {
                let key = match parts.next() {
                    Some(k) => k,
                    None => { println!("Usage: set <key> <value>"); continue; }
                };
                let value = parts.collect::<Vec<_>>().join(" ");
                if agent.kv().set(key, value.into_bytes()) {
                    println!("Stored and queued for gossip.");
                } else {
                    println!("Stored locally (gossip channel full or not running).");
                }
            }
            "get" => {
                let key = match parts.next() {
                    Some(k) => k,
                    None => { println!("Usage: get <key>"); continue; }
                };
                match agent.kv().get(key) {
                    Some(v) => println!("{}", String::from_utf8_lossy(&v)),
                    None    => println!("(not found)"),
                }
            }
            "delete" => {
                let key = match parts.next() {
                    Some(k) => k,
                    None => { println!("Usage: delete <key>"); continue; }
                };
                if agent.kv().delete(key) {
                    println!("Deleted and tombstone queued for gossip.");
                } else {
                    println!("Deleted locally (gossip channel full or not running).");
                }
            }
            "stats" => {
                let s = agent.system_stats();
                println!("Peers        : {}", s.peers);
                println!("Entries      : {}", s.store_entries);
                println!("Conns        : {}", s.cached_connections);
                println!("Dead shards  : {}", s.dead_shards);
                println!("GC alive     : {}", s.gc_alive);
                println!("Monitor alive: {}", s.health_monitor_alive);
                println!("Intern pool  : {}", s.intern_pool_size);
                println!("Task count   : {}", s.task_count);
                let depths: Vec<String> = s.gossip_shard_queue_depths
                    .iter()
                    .enumerate()
                    .map(|(i, d)| format!("[{}]={}", i, d))
                    .collect();
                println!("Shard queues : {}", depths.join(" "));
            }
            "exit" => break,
            "" => {}
            _ => println!("Unknown command. Try: set, get, delete, stats, exit"),
        }
    }

    Ok(())
}

/// The recorded run. Off in every shipped build: the seams route through the kernel only under
/// `sim`, and this module exists only then.
#[cfg(feature = "sim")]
mod record {
    use super::*;
    use mycelium::sim_seam::{install, take, SimContext};
    use mycelium_sim::{Bundle, Kernel, Sources};

    /// Install the seams for this node, run it to shutdown on a **current-thread** runtime (the
    /// kernel is per thread; a multi-thread runtime would record a fraction of the choices), then
    /// write the bundle. The bundle has **no witness**: it reproduces the run's timing, and whoever
    /// debugs it adds the assertion that failed (`Bundle::witnessed_by`) before trusting a replay.
    pub(super) fn run_recorded(config: GossipConfig, dir: std::path::PathBuf) -> Result<(), Box<dyn Error>> {
        let seed: u64 = std::env::var("GOSSIP_RECORD_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed);
        // Through the seam, read *before* `install` so it is the real wall clock: the recording's
        // `Sources` are seeded from it, and every later read is routed and recorded.
        let wall_ms = mycelium::sim_seam::wall_now_ms();
        let node = format!("{}:{}", config.bind_address, config.bind_port);
        install(SimContext {
            kernel:  Kernel::recording(),
            sources: Sources::seeded(seed, wall_ms),
            node,
            offsets: Default::default(),
        });
        tracing::info!(dir = %dir.display(), seed, "recording this run into a replay bundle (current-thread runtime)");

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let outcome = rt.block_on(run(config));
        drop(rt);

        match take() {
            Some(ctx) => {
                std::fs::create_dir_all(&dir)?;
                Bundle::new(ctx.kernel.trace().clone())
                    .write(&dir)
                    .map_err(|e| format!("writing the bundle: {e:?}"))?;
                tracing::info!(dir = %dir.display(), "bundle written — add a witness before you trust a replay");
            }
            None => tracing::warn!("no recording context to write"),
        }
        outcome
    }
}

