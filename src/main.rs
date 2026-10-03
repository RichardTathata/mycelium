use mycelium::{GossipAgent, NodeId};
use mycelium::config::GossipConfig;
use mycelium::error::GossipError;
use std::{error::Error, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // `mycelium wire-check <units-dir> …` — the offline check over a directory of unit files
    // (docs/plans/design-time-tooling.md §4). A pure function over declarations; it starts no
    // node and reads no mesh, so it runs before the runtime is built.
    if std::env::args().nth(1).as_deref() == Some("wire-check") {
        std::process::exit(wire_check_cli(std::env::args().skip(2).collect()));
    }
    // `mycelium rules` and `mycelium explain` — the rule catalogue this binary registers, and a
    // decision trace rendered as an explanation (docs/plans/guarantees-and-rule-catalogue.md I6).
    // Both are pure over files; neither starts a node.
    match std::env::args().nth(1).as_deref() {
        Some("rules") => std::process::exit(rules_cli(std::env::args().skip(2).collect())),
        Some("explain") => std::process::exit(explain_cli(std::env::args().skip(2).collect())),
        // `mycelium tls issue` — a node certificate signed where the CA key is, so the node never
        // holds it (`id.ca_key_off_node`). Runs on the issuer's host, starts no node.
        Some("tls") => std::process::exit(tls_cli(std::env::args().skip(2).collect())),
        _ => {}
    }

    let config = parse_args()?;

    // `GOSSIP_RECORD_BUNDLE_DIR` (builds with `sim` only): run under the replay seams on a
    // current-thread runtime and write a bundle at shutdown — the operator's capture path that
    // diagnostics.md used to imply and the tree did not have (doc-coverage run 17, code gap 3).
    #[cfg(feature = "sim")]
    if let Ok(dir) = std::env::var("GOSSIP_RECORD_BUNDLE_DIR") {
        return record::run_recorded(config, std::path::PathBuf::from(dir));
    }

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(config, None))
}

async fn run(config: GossipConfig, trace: Option<Arc<mycelium::decision::DecisionSink>>) -> Result<(), Box<dyn Error>> {
    let node_id = NodeId::new(&config.bind_address, config.bind_port)?;

    let agent = Arc::new(GossipAgent::new(node_id, config));
    // A recording carries the decision trace (plan I6): attached before start(), written into the
    // bundle as `decisions.jsonl` beside `coverage.json` when the run ends.
    if let Some(sink) = trace {
        agent.with_decision_trace(sink);
    }

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

/// `mycelium wire-check <units-dir> [--library <artifacts-dir>] [--format text|json|dot]
/// [--strict-deployed] [--revision <rev>]`. Reads every `*.toml` in `units-dir` as a unit (its
/// file stem is the unit's name) and every `*.toml` in `artifacts-dir` as an artifact description,
/// runs [`mycelium::wire_check::check`], prints the report, and exits 0 with no errors, 1 with
/// any, 2 when a file does not load. The revision defaults to `git rev-parse HEAD` in `units-dir`
/// when that succeeds, so the JSON document names the commit it describes.
fn wire_check_cli(args: Vec<String>) -> i32 {
    use mycelium::wire_check::{check, ArtifactDescription, CheckOptions, Unit};

    let mut dir: Option<String> = None;
    let mut library: Option<String> = None;
    let mut schemas: Option<String> = None;
    let mut format = "text".to_string();
    let mut opts = CheckOptions::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--library" => library = it.next(),
            "--format" => format = it.next().unwrap_or_default(),
            "--strict-deployed" => opts.strict_deployed = true,
            "--no-authority" => opts.authority = false,
            "--revision" => opts.revision = it.next(),
            "--schemas" => schemas = it.next(),
            "-h" | "--help" => {
                eprintln!("Usage: mycelium wire-check <units-dir> [--library <artifacts-dir>] [--schemas <schemas-dir>] [--format text|json|dot] [--strict-deployed] [--no-authority] [--revision <rev>]\n\
                           \n\
                           Applies the mesh's own match rule (CapFilter::matches) to a directory of unit files and reports\n\
                           what could not bind. It says *would bind under these declarations*, never *is bound*: liveness,\n\
                           ranking over runtime attributes, locality, membership now, a mandate's currency and load are\n\
                           runtime facts the mesh reports itself. Exit 0 with no errors, 1 with any, 2 if a file does not load.");
                return 0;
            }
            other if other.starts_with('-') => {
                eprintln!("wire-check: unknown option {other}");
                return 2;
            }
            other => dir = Some(other.to_string()),
        }
    }
    let Some(dir) = dir else {
        eprintln!("wire-check: a units directory is required");
        return 2;
    };
    if !matches!(format.as_str(), "text" | "json" | "dot") {
        eprintln!("wire-check: --format must be text, json or dot");
        return 2;
    }

    fn toml_files(dir: &str) -> Result<Vec<(String, std::path::PathBuf)>, String> {
        let mut files: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(dir)
            .map_err(|e| format!("{dir}: {e}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .filter_map(|p| p.file_stem().map(|s| (s.to_string_lossy().into_owned(), p.clone())))
            .collect();
        files.sort();
        Ok(files)
    }

    let units: Vec<Unit> = match toml_files(&dir) {
        Ok(files) => {
            let mut units = Vec::new();
            for (name, path) in files {
                match mycelium::NodeCapabilityConfig::load_from_file(&path) {
                    Ok(config) => units.push(Unit { name, config }),
                    Err(e) => {
                        eprintln!("wire-check: {}: {e}", path.display());
                        return 2;
                    }
                }
            }
            units
        }
        Err(e) => {
            eprintln!("wire-check: {e}");
            return 2;
        }
    };
    let mut artifacts = Vec::new();
    if let Some(lib) = &library {
        match toml_files(lib) {
            Ok(files) => {
                for (name, path) in files {
                    let text = match std::fs::read_to_string(&path) {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("wire-check: {}: {e}", path.display());
                            return 2;
                        }
                    };
                    match ArtifactDescription::from_toml_str(&text) {
                        Ok(a) => artifacts.push((name, a)),
                        Err(e) => {
                            eprintln!("wire-check: {}: {e}", path.display());
                            return 2;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("wire-check: {e}");
                return 2;
            }
        }
    }
    if let Some(dir) = &schemas {
        // Guide 12's rule: each `.json` file's path relative to the directory, without the extension.
        fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) -> Result<(), String> {
            for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
                if path.is_dir() {
                    walk(root, &path, out)?;
                } else if path.extension().is_some_and(|x| x == "json")
                    && let Ok(rel) = path.strip_prefix(root)
                {
                    let id = rel.with_extension("").components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                    out.insert(id);
                }
            }
            Ok(())
        }
        let root = std::path::Path::new(dir);
        let mut ids = std::collections::BTreeSet::new();
        if let Err(e) = walk(root, root, &mut ids) {
            eprintln!("wire-check: {e}");
            return 2;
        }
        opts.known_schemas = Some(ids);
    }
    if opts.revision.is_none() {
        opts.revision = std::process::Command::new("git")
            .args(["-C", &dir, "rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
    }

    let report = check(&units, &artifacts, &opts);
    match format.as_str() {
        "json" => println!("{}", report.render_json()),
        "dot" => print!("{}", report.render_dot()),
        _ => print!("{}", report.render_text()),
    }
    report.exit_code()
}

/// `mycelium rules [--format md|json]` — the rule catalogue this binary registers
/// (`mycelium::rules::RULES`), generated from the descriptors. The fleet's whole catalogue, with the
/// stem host's provisioning rules, is the checked-in `docs/reference/rule-catalogue.md`.
fn rules_cli(args: Vec<String>) -> i32 {
    use mycelium::rule::Catalogue;
    const USAGE: &str = "Usage: mycelium rules [--format md|json]";
    let mut format = "md".to_string();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--format" => format = it.next().unwrap_or_default(),
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    let (cat, external) = match Catalogue::gather_partial(&[mycelium::rules::RULES]) {
        Ok(c) => c,
        Err(errs) => { for e in errs { eprintln!("{e}"); } return 1; }
    };
    if !external.is_empty() {
        eprintln!("note: {} relation(s) name rules another crate registers (the stem host's provisioning rules); the fleet's whole catalogue is docs/reference/rule-catalogue.md", external.len());
    }
    match format.as_str() {
        "json" => println!("{}", cat.to_json()),
        "md" => print!("{}", cat.to_markdown()),
        other => { eprintln!("unknown format `{other}` (md|json)"); return 2; }
    }
    0
}

/// `mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>] [--target <t>]` — a
/// decision trace (a stem's `--trace-dir`, or a bundle's `decisions.jsonl`) rendered as what each
/// target went through: the round, the rule, how it ended and why, what it read and caused; with a
/// catalogue, each rule used summarised. Exits 2 when a file does not load.
fn explain_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>] [--target <target>]";
    let (mut file, mut catalogue, mut target): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--catalogue" => catalogue = it.next(),
            "--target" => target = it.next(),
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            _ if file.is_none() => file = Some(a),
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    let Some(file) = file else { eprintln!("{USAGE}"); return 2; };
    let jsonl = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => { eprintln!("{file}: {e}"); return 2; }
    };
    let catalogue = match catalogue.map(std::fs::read_to_string) {
        None => None,
        Some(Ok(s)) => Some(s),
        Some(Err(e)) => { eprintln!("catalogue: {e}"); return 2; }
    };
    print!("{}", mycelium::decision::explain(&jsonl, catalogue.as_deref(), target.as_deref()));
    0
}

/// `mycelium tls issue --ca-dir <dir> --node <ip:port> --out <dir>` — issue a node certificate off-node:
/// the CA (`ca-cert.pem` + `ca-key.pem`) stays in `--ca-dir` on this host; `<out>/<node>.cert.pem` and
/// `<out>/<node>.key.pem` go to the node, with `ca-cert.pem` (the certificate only), as `[tls]
/// cert_pem` / `key_pem`. Exits 2 on usage or a file that does not load.
fn tls_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium tls issue --ca-dir <dir> --node <ip:port> --out <dir>";
    let mut it = args.into_iter();
    if it.next().as_deref() != Some("issue") { eprintln!("{USAGE}"); return 2; }
    let (mut ca_dir, mut node, mut out): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ca-dir" => ca_dir = it.next(),
            "--node" => node = it.next(),
            "--out" => out = it.next(),
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    let (Some(ca_dir), Some(node), Some(out)) = (ca_dir, node, out) else { eprintln!("{USAGE}"); return 2; };
    #[cfg(feature = "tls")]
    {
        let node_id: mycelium::NodeId = match node.parse() { Ok(n) => n, Err(e) => { eprintln!("--node: {e}"); return 2; } };
        match mycelium::issue_node_cert(std::path::Path::new(&ca_dir), &node_id, std::path::Path::new(&out)) {
            Ok((cert, key)) => {
                println!("issued {} and {}", cert.display(), key.display());
                println!("copy both to the node with {ca_dir}/ca-cert.pem (the certificate only — never ca-key.pem), and set [tls] cert_pem / key_pem");
                0
            }
            Err(e) => { eprintln!("{e}"); 2 }
        }
    }
    #[cfg(not(feature = "tls"))]
    {
        let _ = (ca_dir, node, out);
        eprintln!("mycelium tls issue needs a binary built with the `tls` feature");
        2
    }
}

fn print_usage() {
    eprintln!(
        "Usage: mycelium [OPTIONS]\n       mycelium wire-check <units-dir> [--library <dir>] [--schemas <dir>] [--format text|json|dot] [--strict-deployed] [--no-authority]\n       mycelium rules [--format md|json]\n       mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>] [--target <target>]\n       mycelium tls issue --ca-dir <dir> --node <ip:port> --out <dir>\n\
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

        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let outcome = rt.block_on(run(config, Some(Arc::clone(&sink))));
        drop(rt);

        match take() {
            Some(ctx) => {
                std::fs::create_dir_all(&dir)?;
                let coverage = mycelium::rule::Catalogue::gather_partial(&[mycelium::rules::RULES])
                    .map(|(cat, _)| mycelium::decision::coverage_manifest(&cat.rules))
                    .unwrap_or_else(|_| "{}".into());
                Bundle::new(ctx.kernel.trace().clone())
                    .with_attachment(mycelium_sim::bundle::DECISION_ATTACHMENT, sink.to_jsonl().into_bytes())
                    .with_attachment(mycelium_sim::bundle::COVERAGE_MANIFEST, coverage.into_bytes())
                    .with_attachment("decisions.stats.json", sink.stats_json().into_bytes())
                    .write(&dir)
                    .map_err(|e| format!("writing the bundle: {e:?}"))?;
                tracing::info!(dir = %dir.display(), "bundle written — add a witness before you trust a replay");
            }
            None => tracing::warn!("no recording context to write"),
        }
        outcome
    }
}

