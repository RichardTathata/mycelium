use mycelium::{GossipAgent, NodeId};
use mycelium::config::GossipConfig;
use mycelium::error::GossipError;
use std::path::PathBuf;
use std::{error::Error, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    // `mycelium doctor` prints its report on stdout — JSON when asked — so its log lines go to stderr;
    // the node keeps its logs on stdout as before (#665's review, D1). `mycelium test` likewise, and at
    // WARN: its fleet's nodes would otherwise print every start-up block over the report.
    let command = std::env::args().nth(1);
    let (doctor, test) = (command.as_deref() == Some("doctor"), command.as_deref() == Some("test"));
    let logs = tracing_subscriber::fmt().with_max_level(if test { tracing::Level::WARN } else { tracing::Level::INFO });
    if doctor || test { logs.with_writer(std::io::stderr).init(); } else { logs.init(); }

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
        // `mycelium --version`: the crate's version and nothing more, the line the release workflow's
        // clean-container smoke compares against the tag (docs/plans/adopter-surface.md, X0).
        Some("-V" | "--version") => { println!("{}", version_line()); std::process::exit(0) }
        // `mycelium doctor [config.toml] [--units <dir>] [--format text|json]` — what `start()` would print
        // and whether it would refuse, from the configuration and the environment alone; no port, no disk
        // (docs/plans/adopter-surface.md, X1).
        Some("doctor") => std::process::exit(doctor_cli(std::env::args().skip(2).collect())),
        // `mycelium init <dir> [--template <name>]` — a scaffold, written from the templates this binary embeds
        // (docs/plans/adopter-surface.md, X2). Pure over files; starts no node.
        Some("init") => std::process::exit(init_cli(std::env::args().skip(2).collect())),
        // `mycelium declare [project-dir] [--watch]` — the project's units and skills, declared to a gateway once or
        // kept current as the files change (docs/plans/adopter-surface.md, X10). Starts no node.
        #[cfg(feature = "gateway")]
        Some("declare") => std::process::exit(declare_cli(std::env::args().skip(2).collect())),
        #[cfg(not(feature = "gateway"))]
        Some("declare") => { eprintln!("declare talks to a gateway, which only a build with the `gateway` feature has"); std::process::exit(2) }
        Some("rules") => std::process::exit(rules_cli(std::env::args().skip(2).collect())),
        Some("test") => std::process::exit(test_cli(std::env::args().skip(2).collect())),
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

    // The stop signals are installed before the node binds, so a SIGTERM during startup is an orderly
    // shutdown — the bundle written — rather than the default action's kill. Not in interactive mode,
    // whose Ctrl-C must keep its default (a handler nobody awaits would swallow it).
    let interactive = std::env::args().any(|a| a == "-i" || a == "--interactive");
    let shutdown = if interactive { None } else { Some(mycelium::shutdown::ShutdownSignal::install()?) };

    agent.start().await?;
    // The node votes. A node that never started a listener answered every quorum it was asked to join
    // with silence, so a fleet of `mycelium` binaries timed out on each consistent write, lock and
    // election its own gateway serves (`/gateway/overlay/{consistent/set,lock/acquire,elect}`) — only
    // an embedder that called `start_consensus_listener` itself had a working quorum (found 2026-10-17,
    // post-360 row O). Held until shutdown; dropping the handle stops the voter.
    #[cfg(feature = "consensus")]
    let _voter = agent.consensus().start_consensus_listener(mycelium::ConsensusConfig::default());

    match shutdown {
        None => run_interactive(Arc::clone(&agent)).await?,
        Some(signal) => signal.wait().await?,
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
            "-V" | "--version" => {
                println!("{}", version_line());
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
        opts.revision = git_revision(&dir);
    }

    let report = check(&units, &artifacts, &opts);
    match format.as_str() {
        "json" => println!("{}", report.render_json()),
        "dot" => print!("{}", report.render_dot()),
        _ => print!("{}", report.render_text()),
    }
    report.exit_code()
}

/// `mycelium declare [project-dir] [--gateway <url>] [--token <bearer>] [--watch] [--poll-ms <n>]
/// [--interval-secs <n>] [--lease-secs <n>]`: declare the project's `units/*.toml` (and `skills/<ns>/<name>.md`) to
/// a gateway, once (exit 0, or 1 when wire-check or the gateway refused) or kept current under `--watch` until
/// Ctrl-C (which retracts). The token defaults to `MYCELIUM_GATEWAY_TOKEN`. Once, no lease is sent and the
/// declaration lives until the node stops or it is retracted; under `--watch` a lease (default 90 s) is sent and
/// heartbeated, so a watch that dies is retracted by the node, and the poll must be shorter than a third of it.
#[cfg(feature = "gateway")]
fn declare_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium declare [project-dir] [--gateway <url>] [--token <bearer>] [--watch] [--poll-ms <n>] [--interval-secs <n>] [--lease-secs <n>]";
    let mut project = std::path::PathBuf::from(".");
    let mut gateway = "http://127.0.0.1:8300".to_string();
    let mut token = std::env::var("MYCELIUM_GATEWAY_TOKEN").ok();
    let (mut watch, mut poll_ms, mut interval_secs, mut lease_secs) = (false, 500u64, 30u64, 90u64);
    let mut it = args.into_iter();
    let mut seen_dir = false;
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| { eprintln!("{flag} needs a value\n{USAGE}"); 2 });
        match a.as_str() {
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            "--watch" => watch = true,
            "--gateway" => match value("--gateway") { Ok(v) => gateway = v, Err(c) => return c },
            "--token" => match value("--token") { Ok(v) => token = Some(v), Err(c) => return c },
            "--poll-ms" | "--interval-secs" | "--lease-secs" => {
                let v = match value(&a) { Ok(v) => v, Err(c) => return c };
                let Ok(n) = v.parse::<u64>() else { eprintln!("{a}: not a number: {v}\n{USAGE}"); return 2; };
                if n == 0 { eprintln!("{a}: must be at least 1\n{USAGE}"); return 2; }
                match a.as_str() { "--poll-ms" => poll_ms = n, "--interval-secs" => interval_secs = n, _ => lease_secs = n }
            }
            _ if a.starts_with('-') => { eprintln!("unknown option `{a}`\n{USAGE}"); return 2; }
            _ if !seen_dir => { project = std::path::PathBuf::from(a); seen_dir = true; }
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    if watch && poll_ms >= lease_secs * 1000 / 3 {
        eprintln!("--poll-ms {poll_ms} is not shorter than a third of --lease-secs {lease_secs}: the declaration would lapse between heartbeats\n{USAGE}");
        return 2;
    }
    let cfg = mycelium::declare::WatchConfig { gateway, token, project, interval_secs, lease_secs: watch.then_some(lease_secs) };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => { eprintln!("declare: {e}"); return 2; }
    };
    rt.block_on(mycelium::declare::run(cfg, watch, std::time::Duration::from_millis(poll_ms)))
}

/// `mycelium init <dir> [--template <name>]`: write a template into a new or empty directory; `--list` names
/// the templates. Exit 0 when written, 1 when refused (the directory is not empty, the name is unknown), 2 on
/// a usage error.
fn init_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium init <dir> [--template <name>]\n       mycelium init --list";
    let mut dest: Option<std::path::PathBuf> = None;
    let mut name = "governed-call".to_string();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            "--list" => {
                for t in mycelium::init::TEMPLATES { println!("{:<18} {}", t.name, t.about); }
                return 0;
            }
            "--template" => match it.next() { Some(n) => name = n, None => { eprintln!("{USAGE}"); return 2; } },
            _ if a.starts_with('-') => { eprintln!("unknown option `{a}`\n{USAGE}"); return 2; }
            _ if dest.is_none() => dest = Some(std::path::PathBuf::from(a)),
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    let Some(dest) = dest else { eprintln!("{USAGE}"); return 2; };
    let Some(template) = mycelium::init::template(&name) else {
        eprintln!("no template named `{name}`; `mycelium init --list` names them");
        return 1;
    };
    match mycelium::init::write(template, &dest) {
        Ok(written) => {
            println!("wrote {} ({} files, pinned to {}):", dest.display(), written.len(), mycelium::init::tag());
            for p in &written { println!("  {}", p.strip_prefix(&dest).unwrap_or(p).display()); }
            println!("next: see {}/README.md", dest.display());
            0
        }
        Err(e) => { eprintln!("init: {e}"); 1 }
    }
}

/// `mycelium doctor`: exit 0 when `start()` would start (and, with `--units`, `wire-check` finds no
/// error), 1 when it would refuse or the units do not bind, 2 on a usage error or a file wire-check
/// cannot read (its own 2). The report is the library's `doctor::run`, printed as text or JSON;
/// `--units` then runs the same `wire-check` the `wire-check` subcommand runs, so the two never
/// disagree. `--format json` with `--units` is refused: the JSON on stdout is one document, and
/// wire-check has its own `--format json` (#665's review, D1).
fn doctor_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium doctor [config.toml] [--units <dir> [--library <dir>] [--schemas <dir>]] [--format text|json]";
    let mut config: Option<std::path::PathBuf> = None;
    let mut units: Vec<String> = Vec::new();
    let mut format = "text".to_string();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            "--format" => match it.next() { Some(f) => format = f, None => { eprintln!("{USAGE}"); return 2; } },
            "--units" | "--library" | "--schemas" => match it.next() {
                Some(v) => { units.push(a.clone()); units.push(v); }
                None => { eprintln!("{USAGE}"); return 2; }
            },
            _ if a.starts_with('-') => { eprintln!("unknown option `{a}`\n{USAGE}"); return 2; }
            _ if config.is_none() => config = Some(std::path::PathBuf::from(a)),
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    if format != "text" && format != "json" { eprintln!("{USAGE}"); return 2; }
    // Usage is settled before anything runs (#665's review, D6): a flag that needs `--units` without it,
    // and the JSON form with `--units`, are refused here rather than after a printed report.
    let units_dir = units.chunks(2).find(|p| p[0] == "--units").map(|p| p[1].clone());
    if !units.is_empty() && units_dir.is_none() { eprintln!("--library/--schemas need --units\n{USAGE}"); return 2; }
    if format == "json" && units_dir.is_some() {
        eprintln!("--format json with --units is not supported: the JSON on stdout is one document; run `mycelium wire-check <dir> --format json` separately\n{USAGE}");
        return 2;
    }
    // `GossipAgent::new` is built inside a runtime context, as the node binary builds it.
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => { eprintln!("doctor: {e}"); return 2; }
    };
    let report = rt.block_on(async { mycelium::doctor::run(config.as_deref()) });
    if format == "json" {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => { eprintln!("doctor: {e}"); return 2; }
        }
    } else {
        print!("{}", report.render());
    }
    let mut code = if report.refusal.is_some() { 1 } else { 0 };
    if let Some(dir) = units_dir {
        // `--units <dir> [--library ..] [--schemas ..]`: the directory is wire-check's positional argument;
        // its exit code is kept (1: a finding; 2: a file it could not read), never flattened to 1.
        let mut wc: Vec<String> = vec![dir];
        for pair in units.chunks(2) {
            if pair[0] != "--units" { wc.push(pair[0].clone()); wc.push(pair[1].clone()); }
        }
        println!();
        code = code.max(wire_check_cli(wc));
    }
    code
}

/// `mycelium test --faults [<class>[,<class>…]] <units-dir> [--config <node.toml>] [--against <units-dir>]
/// [--library <dir>] [--schemas <dir>] [--format text|json]` — the existing proofs against an adopter's
/// declaration (adopter-surface X6, `mycelium::faults`). `--faults` alone runs every class; a class list
/// (`revoke`, `restart`, `partition`, `replay`, `policy-diff`, `recovery`, `grant`) runs those and fails the run if one of them
/// cannot run here (`not_in_build`, `not_configured`, `not_applicable`). Exit 0 when every class that ran
/// passed; 1 when a check did not hold, a named class could not run, a class's fleet refused to build or
/// start (`refused`, asked or not), or `wire-check` refused the declaration (its findings printed, the same
/// words); 2 on a usage error or a file that does not load.
fn test_cli(args: Vec<String>) -> i32 {
    use mycelium::faults::{self, Class, Options, Refused};
    use mycelium::wire_check::{ArtifactDescription, CheckOptions};
    const USAGE: &str = "Usage: mycelium test --faults [<class>[,<class>…]] <units-dir> [--config <node.toml>] [--against <units-dir>] [--decisions <evidence.log>]… [--library <dir>] [--schemas <dir>] [--seed <n>] [--keep] [--format text|json]\n\
       classes: revoke · restart · partition · replay (needs a binary built with --features cli,sim) · policy-diff (needs --against) · recovery (needs [persistence] and a tls build) · grant (needs [tls], [persistence], a compliance build and a [[rule]] with requires_grant)\n\
       a units directory named like a class is written ./<name>; --config replaces the environment (GOSSIP_*) as -c does; --keep leaves the run's directory in place";
    let (mut faults, mut dir, mut config, mut against, mut library, mut schemas) = (false, None::<String>, None::<PathBuf>, None::<String>, None::<String>, None::<String>);
    let (mut classes, mut format, mut seed, mut keep, mut decisions) = (Vec::<Class>::new(), "text".to_string(), 0x5eed_u64, false, Vec::<PathBuf>::new());
    let mut it = args.into_iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            "--faults" => {
                faults = true;
                // A class list follows when a comma-separated token is a class name; a list with a stranger in it
                // is refused naming it (#675's review); a token that is no class is the directory.
                if let Some(next) = it.peek().filter(|n| !n.starts_with('-')) {
                    match Class::parse_list(next) {
                        Ok(Some(list)) => { classes.extend(list); it.next(); }
                        Ok(None) => {}
                        Err(e) => { eprintln!("test: {e}\n{USAGE}"); return 2; }
                    }
                }
            }
            "--keep" => keep = true,
            "--decisions" => match it.next() { Some(v) => decisions.push(PathBuf::from(v)), None => { eprintln!("{USAGE}"); return 2; } },
            "--config" => match it.next() { Some(v) => config = Some(PathBuf::from(v)), None => { eprintln!("{USAGE}"); return 2; } },
            "--against" => match it.next() { Some(v) => against = Some(v), None => { eprintln!("{USAGE}"); return 2; } },
            "--library" => match it.next() { Some(v) => library = Some(v), None => { eprintln!("{USAGE}"); return 2; } },
            "--schemas" => match it.next() { Some(v) => schemas = Some(v), None => { eprintln!("{USAGE}"); return 2; } },
            "--format" => match it.next() { Some(v) => format = v, None => { eprintln!("{USAGE}"); return 2; } },
            "--seed" => match it.next().and_then(|v| v.parse().ok()) { Some(v) => seed = v, None => { eprintln!("--seed takes a number\n{USAGE}"); return 2; } },
            _ if a.starts_with('-') => { eprintln!("unknown option `{a}`\n{USAGE}"); return 2; }
            _ if dir.is_none() => dir = Some(a),
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    if !faults { eprintln!("test runs fault classes only: pass --faults\n{USAGE}"); return 2; }
    let Some(dir) = dir else { eprintln!("a units directory is required\n{USAGE}"); return 2; };
    if format != "text" && format != "json" { eprintln!("{USAGE}"); return 2; }
    let units = match faults::load_units(std::path::Path::new(&dir)) {
        Ok(u) => u,
        Err(e) => { eprintln!("test: {e}"); return 2; }
    };
    let against = match against {
        None => None,
        Some(d) => match faults::load_units(std::path::Path::new(&d)) {
            Ok(u) => Some(u),
            Err(e) => { eprintln!("test: --against {e}"); return 2; }
        },
    };
    let mut artifacts = Vec::new();
    if let Some(lib) = &library {
        let files = match std::fs::read_dir(lib) {
            Ok(d) => d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect::<Vec<_>>(),
            Err(e) => { eprintln!("test: {lib}: {e}"); return 2; }
        };
        for path in files {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| ArtifactDescription::from_toml_str(&t).map_err(|e| e.to_string())) {
                Ok(a) => artifacts.push((name, a)),
                Err(e) => { eprintln!("test: {}: {e}", path.display()); return 2; }
            }
        }
    }
    let mut check_opts = CheckOptions { revision: git_revision(&dir), ..CheckOptions::default() };
    if let Some(dir) = &schemas {
        match schema_ids(std::path::Path::new(dir)) {
            Ok(ids) => check_opts.known_schemas = Some(ids),
            Err(e) => { eprintln!("test: {e}"); return 2; }
        }
    }
    // Every node is built inside a runtime context of its own; the command itself needs none.
    let report = match faults::run(&units, &artifacts, &check_opts, &Options { classes, config, against, seed, keep, decisions }) {
        Ok(r) => r,
        Err(Refused::WireCheck(r)) => {
            let r = *r;
            eprintln!("test: wire-check refused the declaration; nothing ran");
            if format == "json" { println!("{}", r.render_json()) } else { print!("{}", r.render_text()) }
            return 1;
        }
        Err(Refused::Config { path, reason }) => {
            eprintln!("test: the configuration {}: {reason}", path.map(|p| p.display().to_string()).unwrap_or_else(|| "(defaults and the environment)".into()));
            return 2;
        }
    };
    if format == "json" {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => { eprintln!("test: {e}"); return 2; }
        }
    } else {
        print!("{}", report.render());
    }
    report.exit_code()
}

/// The revision `wire-check` stamps on its document when none is given: `git rev-parse HEAD` in the units directory,
/// or none. `test` stamps the same, so its refusal prints the same report, word for word.
fn git_revision(dir: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["-C", dir, "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Guide 12's rule for a schema directory: each `.json` file's path relative to the directory, without the
/// extension — the same ids `wire-check --schemas` reads.
fn schema_ids(root: &std::path::Path) -> Result<std::collections::BTreeSet<String>, String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.extension().is_some_and(|x| x == "json")
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.insert(rel.with_extension("").components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"));
            }
        }
        Ok(())
    }
    let mut ids = std::collections::BTreeSet::new();
    walk(root, root, &mut ids)?;
    Ok(ids)
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
///
/// `mycelium explain --correlation <id> <files…>` renders one run's records as one chain across evidence journals
/// and decision traces; `--closure <hash> <files…> [--fleet <n>]` the dependency closure of one record
/// (adopter-surface X4, `mycelium::closure`).
fn explain_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>] [--target <target>]
       mycelium explain --correlation <id> <evidence.log|decisions.jsonl>…
       mycelium explain --closure <sha256 or a unique prefix of 8+> <evidence.log|decisions.jsonl>… [--fleet <nodes>]";
    let (mut file, mut catalogue, mut target): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let (mut correlation, mut closure, mut fleet): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let mut files: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--catalogue" => catalogue = it.next(),
            "--target" => target = it.next(),
            "--correlation" => correlation = it.next(),
            "--closure" => closure = it.next(),
            "--fleet" => fleet = it.next(),
            "-h" | "--help" => { eprintln!("{USAGE}"); return 0; }
            _ if correlation.is_some() || closure.is_some() => files.push(a),
            _ if file.is_none() => file = Some(a),
            _ => { eprintln!("{USAGE}"); return 2; }
        }
    }
    if correlation.is_some() || closure.is_some() {
        if (correlation.is_some() && closure.is_some()) || files.is_empty() || file.is_some() || catalogue.is_some() || target.is_some() {
            eprintln!("{USAGE}");
            return 2;
        }
        let fleet = match fleet.map(|f| f.parse::<usize>()) {
            None => None,
            Some(Ok(n)) if n > 0 => Some(n),
            Some(_) => { eprintln!("--fleet takes a positive count of nodes"); return 2; }
        };
        let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
        let corpus = match mycelium::closure::Corpus::load(&paths) {
            Ok(c) => c,
            Err(e) => { eprintln!("{e}"); return 2; }
        };
        // A full hash, or the unique prefix `--correlation`'s chain prints (doc-coverage run 24, gap 1).
        match (correlation, closure) {
            (Some(id), _) => print!("{}", corpus.chain(&id)),
            (_, Some(h)) => match corpus.closure_for(&h, fleet) {
                Ok(out) => print!("{out}"),
                Err(e) => { eprintln!("--closure: {e}"); return 2; }
            },
            _ => unreachable!(),
        }
        return 0;
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

/// `mycelium tls issue --ca-dir <dir> --node <ip:port> --out <dir> [--force]` — issue a node certificate off-node:
/// the CA (`ca-cert.pem` + `ca-key.pem`) stays in `--ca-dir` on this host; `<out>/<node>.cert.pem` and
/// `<out>/<node>.key.pem` go to the node, with `ca-cert.pem` (the certificate only), as `[tls]
/// cert_pem` / `key_pem`. Refuses to replace a node key it issued before unless `--force`. Exits 2 on usage or a
/// file that does not load.
///
/// `mycelium tls ca --out <dir>` — mint the fleet CA on the issuer's host (`ca-cert.pem` + `ca-key.pem`), the step
/// before the first `issue`; refused when `<dir>` already holds a CA, names a mint that was interrupted, and needs a
/// filesystem with hard links. Only `ca-cert.pem` ever goes to a node.
///
/// `mycelium tls pin <cert.pem>` — the hex SHA-256 of a certificate's `SubjectPublicKeyInfo`, read from its
/// structural position: the value a partner's federation client pins (`FederationClient::with_tls_pins`) for a
/// gateway that serves this certificate's key. An operator hands the partner this line, never the key
/// (adopter-surface Y8). Exits 2 on usage or a file that is not one PEM certificate.
fn tls_cli(args: Vec<String>) -> i32 {
    const USAGE: &str = "Usage: mycelium tls ca --out <dir>\n       mycelium tls issue --ca-dir <dir> --node <ip:port> --out <dir> [--force]\n       mycelium tls pin <cert.pem>";
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("issue") => {}
        Some("pin") => return tls_pin(it.collect()),
        Some("ca") => return tls_ca(it.collect()),
        _ => { eprintln!("{USAGE}"); return 2; }
    }
    let (mut ca_dir, mut node, mut out): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let mut force = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => force = true,
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
        // A node key already issued is an identity in use: a partner may pin it, a node may run on it. Issuing over
        // it replaces both, so it takes `--force` (#695's review, finding 5; deprecations § 69).
        let existing = std::path::Path::new(&out).join(format!("{}.key.pem", node_id.as_str().replace([':', '.'], "_")));
        if existing.exists() && !force {
            eprintln!(
                "{} exists: issuing over it replaces this node's key, so a node running on it must restart, and every \
                 partner that pins its gateway must be given the new pin (`mycelium tls pin`); pass --force to issue anyway",
                existing.display()
            );
            return 2;
        }
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
        let _ = (ca_dir, node, out, force);
        eprintln!("mycelium tls issue needs a binary built with the `tls` feature");
        2
    }
}

fn tls_ca(args: Vec<String>) -> i32 {
    let ["--out", dir] = args.iter().map(String::as_str).collect::<Vec<_>>()[..] else {
        eprintln!("Usage: mycelium tls ca --out <dir>");
        return 2;
    };
    #[cfg(feature = "tls")]
    match mycelium::mint_ca(std::path::Path::new(dir)) {
        Ok(cert) => {
            println!("minted the fleet CA: {} and {dir}/ca-key.pem", cert.display());
            println!("keep ca-key.pem on this host; a node gets ca-cert.pem only, with a certificate from `mycelium tls issue`");
            0
        }
        Err(e) => { eprintln!("{e}"); 2 }
    }
    #[cfg(not(feature = "tls"))]
    {
        let _ = dir;
        eprintln!("mycelium tls ca needs a binary built with the `tls` feature");
        2
    }
}

fn tls_pin(args: Vec<String>) -> i32 {
    let [path] = args.as_slice() else { eprintln!("Usage: mycelium tls pin <cert.pem>"); return 2; };
    #[cfg(feature = "tls")]
    {
        let pem = match std::fs::read_to_string(path) { Ok(p) => p, Err(e) => { eprintln!("{path}: {e}"); return 2; } };
        match pin_of_pem(&pem) {
            Ok(pin) => { println!("{pin}"); 0 }
            Err(e) => { eprintln!("{path}: {e}"); 2 }
        }
    }
    #[cfg(not(feature = "tls"))]
    {
        let _ = path;
        eprintln!("mycelium tls pin needs a binary built with the `tls` feature");
        2
    }
}

/// The pin of the one certificate in `pem`, as hex: its `SubjectPublicKeyInfo`'s SHA-256, read structurally. Refused:
/// no certificate or more than one, a file that also carries a private key (what is handed to a partner must hold
/// none), and a CA certificate — no gateway presents the CA's key, so its pin admits nothing (#695's review, finding 6).
#[cfg(feature = "tls")]
fn pin_of_pem(pem: &str) -> Result<String, String> {
    if pem.contains("PRIVATE KEY-----") {
        return Err("the file carries a private key: pin the certificate alone, and hand the partner only the pin".into());
    }
    let blocks: Vec<&str> = pem.split("-----BEGIN CERTIFICATE-----").skip(1).collect();
    let [block] = blocks.as_slice() else {
        return Err(format!("expected exactly one PEM certificate, found {}", blocks.len()));
    };
    let b64: String = block.split("-----END CERTIFICATE-----").next().unwrap_or("").split_whitespace().collect();
    let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).map_err(|e| format!("the certificate is not base64: {e}"))?;
    let pin = mycelium::federation::pinning::spki_sha256(&der).ok_or("not an X.509 certificate this build can read")?;
    if is_ca_certificate(&der).ok_or("the certificate's extensions could not be read")? {
        return Err("a CA certificate (basicConstraints cA): pin the gateway's own certificate, which the CA issued".into());
    }
    Ok(pin.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether a DER certificate says `basicConstraints cA = TRUE`, read by position: Certificate → tbsCertificate →
/// `[3]` extensions → the extension whose OID is 2.5.29.19 → its `BasicConstraints` → the leading BOOLEAN. `None` when
/// the structure is not one this walk can follow; no extensions, or no basicConstraints, is `Some(false)`.
#[cfg(feature = "tls")]
fn is_ca_certificate(der: &[u8]) -> Option<bool> {
    /// One TLV: (tag, contents, the bytes after it).
    fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let (&tag, rest) = b.split_first()?;
        let (&len0, rest) = rest.split_first()?;
        let (len, rest) = if len0 < 0x80 {
            (len0 as usize, rest)
        } else {
            let n = (len0 & 0x7f) as usize;
            if n == 0 || n > 4 || rest.len() < n { return None; }
            (rest[..n].iter().fold(0usize, |a, &x| (a << 8) | x as usize), &rest[n..])
        };
        (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
    }
    const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
    let (0x30, cert, _) = tlv(der)? else { return None };
    let (0x30, mut tbs, _) = tlv(cert)? else { return None };
    while !tbs.is_empty() {
        let (tag, body, rest) = tlv(tbs)?;
        tbs = rest;
        if tag != 0xa3 { continue; }
        let (0x30, mut exts, _) = tlv(body)? else { return None };
        while !exts.is_empty() {
            let (0x30, ext, rest) = tlv(exts)? else { return None };
            exts = rest;
            let (0x06, oid, fields) = tlv(ext)? else { return None };
            if oid != BASIC_CONSTRAINTS { continue; }
            let (mut tag, mut value, after) = tlv(fields)?;
            if tag == 0x01 { (tag, value, _) = tlv(after)?; }   // `critical`, then the OCTET STRING
            if tag != 0x04 { return None; }
            let (0x30, bc, _) = tlv(value)? else { return None };
            return Some(matches!(tlv(bc), Some((0x01, [v], _)) if *v != 0));
        }
        return Some(false);
    }
    Some(false)
}

fn print_usage() {
    eprintln!(
        "Usage: mycelium [OPTIONS]\n       mycelium init <dir> [--template <name>] | --list\n       mycelium declare [project-dir] [--gateway <url>] [--token <bearer>] [--watch]\n       mycelium doctor [config.toml] [--units <dir>] [--format text|json]\n       mycelium wire-check <units-dir> [--library <dir>] [--schemas <dir>] [--format text|json|dot] [--strict-deployed] [--no-authority]\n       mycelium rules [--format md|json]\n       mycelium test --faults [<class>,…] <units-dir> [--config <node.toml>] [--against <units-dir>] [--format text|json]\n       mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>] [--target <target>]\n       mycelium explain --correlation <id> <files…> | --closure <sha256> <files…> [--fleet <n>]\n       mycelium tls ca --out <dir> | issue --ca-dir <dir> --node <ip:port> --out <dir> [--force] | pin <cert.pem>\n\
         \n\
         Options:\n\
         -c, --config <file>      Load configuration from a TOML file\n\
         -p, --port <port>        Bind port (default: 8080)\n\
             --host <ip>          Bind IP address (default: 127.0.0.1)\n\
         -r, --peers <list>       Comma-separated bootstrap peers (IP:port,...)\n\
         -i, --interactive        Start an interactive REPL\n\
         -V, --version            Print `mycelium <version>` and exit\n\
         -h, --help               Show this message\n\
         \n\
         Profiles (the config's `profile`, or GOSSIP_PROFILE): `dev` is the default and requires nothing,\n\
         loudly; `secure-single-domain` refuses to start unless every guarantee it requires resolves on\n\
         this node, naming each unmet one; `secure-multi-domain` adds the federation family (`fed.*`). The report: GET /gateway/guarantees, and one log block at\n\
         start. Catalogue: docs/reference/guarantee-catalogue.md"
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


/// `mycelium --version`: `mycelium <crate version>`, one line, so a script can compare it to a tag.
fn version_line() -> String {
    format!("mycelium {}", env!("CARGO_PKG_VERSION"))
}

/// Adopter-surface Y8: the issuer host's three steps — a CA, a node certificate, the pin a partner is handed.
#[cfg(all(test, feature = "tls"))]
mod tls_cli_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mycelium-tls-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// `tls ca` mints a CA `tls issue` can sign with, and refuses a second over it; `tls pin` of the issued
    /// certificate is the pin a partner's client must hold for a gateway serving that node's key — the value
    /// `ed25519_spki_sha256` computes from the key alone, so the line an operator hands over is the one that admits.
    #[test]
    fn ca_issue_and_pin_are_the_issuers_three_steps() {
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        let (ca, out) = (scratch("ca"), scratch("out"));
        assert_eq!(tls_ca(vec!["--out".into(), ca.display().to_string()]), 0);
        assert!(ca.join("ca-cert.pem").exists() && ca.join("ca-key.pem").exists());
        assert_eq!(tls_ca(vec!["--out".into(), ca.display().to_string()]), 2, "a second CA over the first is refused");
        assert_eq!(tls_ca(vec!["--out".into()]), 2, "usage");
        let node: mycelium::NodeId = "127.0.0.1:7961".parse().expect("node id");
        let (cert, key) = mycelium::issue_node_cert(&ca, &node, &out).expect("issued from the minted CA");
        let pin = pin_of_pem(&std::fs::read_to_string(&cert).expect("cert")).expect("a pin");
        let pem = std::fs::read_to_string(&key).expect("key");
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).expect("PEM base64");
        let sk = ed25519_dalek::SigningKey::from_pkcs8_der(&der).expect("PKCS#8 node key");
        let from_key = mycelium::federation::pinning::ed25519_spki_sha256(&sk.verifying_key().to_bytes());
        assert_eq!(pin, from_key.iter().map(|b| format!("{b:02x}")).collect::<String>(), "the pin is the node key's");
        assert_eq!(tls_pin(vec![cert.display().to_string()]), 0);
        assert!(pin_of_pem("").is_err() && pin_of_pem(&format!("{0}{0}", std::fs::read_to_string(&cert).unwrap())).is_err(),
            "no certificate, or two, is refused");
        // A CA certificate pins no gateway, and a bundle with a key is not something to hand a partner. Seen failing
        // first: both printed a pin.
        let ca_pem = std::fs::read_to_string(ca.join("ca-cert.pem")).unwrap();
        assert!(pin_of_pem(&ca_pem).unwrap_err().contains("CA certificate"), "the CA's certificate is refused");
        let bundle = format!("{}{}", std::fs::read_to_string(&cert).unwrap(), std::fs::read_to_string(&key).unwrap());
        assert!(pin_of_pem(&bundle).unwrap_err().contains("private key"), "a bundle with its key is refused");
        let _ = (std::fs::remove_dir_all(&ca), std::fs::remove_dir_all(&out));
    }

    /// `tls issue` over a node's existing key is refused unless `--force`: the key is an identity in use, and a pin a
    /// partner holds stops matching once it is replaced. With `--force` it is replaced and the pin changes. Seen
    /// failing first: the second issue replaced the key silently (exit 0).
    #[test]
    fn issuing_over_an_existing_node_key_needs_force() {
        let (ca, out) = (scratch("reissue-ca"), scratch("reissue-out"));
        assert_eq!(tls_ca(vec!["--out".into(), ca.display().to_string()]), 0);
        let issue = |extra: &[&str]| {
            let mut a: Vec<String> = ["issue", "--ca-dir", &ca.display().to_string(), "--node", "127.0.0.1:7962", "--out", &out.display().to_string()]
                .iter().map(|s| s.to_string()).collect();
            a.extend(extra.iter().map(|s| s.to_string()));
            tls_cli(a)
        };
        let cert = out.join("127_0_0_1_7962.cert.pem");
        assert_eq!(issue(&[]), 0);
        let before = pin_of_pem(&std::fs::read_to_string(&cert).unwrap()).unwrap();
        assert_eq!(issue(&[]), 2, "a second issue over the key is refused");
        assert_eq!(pin_of_pem(&std::fs::read_to_string(&cert).unwrap()).unwrap(), before, "and changes nothing");
        assert_eq!(issue(&["--force"]), 0);
        assert_ne!(pin_of_pem(&std::fs::read_to_string(&cert).unwrap()).unwrap(), before, "--force replaces the key, and the pin with it");
        let _ = (std::fs::remove_dir_all(&ca), std::fs::remove_dir_all(&out));
    }
}

#[cfg(test)]
mod version_tests {
    /// `mycelium --version` is what the release workflow's clean-container smoke compares against the tag
    /// (`.github/workflows/release.yml`): the line is the crate's version, nothing more, so a tag cut out
    /// of order (RELEASING.md § 4 before § 7) is caught by the string and not by a reader.
    #[test]
    fn version_line_names_the_crate_version() {
        // Read from the manifest, not from the same `env!` the function uses, so a function that printed another
        // crate's version, a prefix, or a build-time string would fail here.
        let manifest = include_str!("../Cargo.toml");
        let declared = manifest
            .lines()
            .find_map(|l| l.strip_prefix("version").and_then(|r| r.trim_start().strip_prefix('=')))
            .map(|r| r.trim().trim_matches('"').to_string())
            .expect("Cargo.toml declares a version");
        let line = super::version_line();
        assert_eq!(line, format!("mycelium {declared}"));
        let v = line.strip_prefix("mycelium ").expect("the crate name, a space, the version");
        assert_eq!(v.split('.').count(), 3, "three dotted components: {v}");
        assert!(v.split('.').all(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit() || ch == '-' || ch.is_ascii_alphanumeric())), "{v}");
    }
}
