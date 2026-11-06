//! `mycelium test --faults` — the existing proofs against an adopter's declaration
//! (docs/plans/adopter-surface.md, X6).
//!
//! A declaration directory and a configuration go in; a fleet of in-process nodes is built from them —
//! one node per unit file, three at least — and the fault classes the repository already proves are run
//! against **this** fleet: `revoke` (a node's key and a mandate, at row Q's verifiers), `restart`
//! (persistence replay over the fleet's own directories, and unreadable state refused), `partition` (a
//! node that goes dark: evicted, its offers gone from the view, the governed floor re-filled, and the
//! node back), `replay` (the run recorded under the kernel and replayed, the first divergence reported)
//! and `policy-diff` (two revisions of the declaration's `[[rule]]`s over the decisions the run recorded,
//! every call the new revision permits and the old refused). The report lists, per class, the guarantee
//! and rule ids it exercised with each one's resolution on the fleet's node, and the ones it could not
//! exercise and why, in the guarantee report's own vocabulary (`not_configured`, `not_in_build`,
//! `not_applicable`, `not_verifiable_here`).
//!
//! **What the fleet is, and is not.** Its nodes carry the adopter's configuration — TLS, persistence,
//! every setting but the profile, which is reported as declared and not applied — with these rewritten: loopback addresses and free ports; persistence and
//! the TLS material under a temporary directory the run removes, so no adopter directory is written;
//! no gateway port (the classes drive the nodes in process); a one-second health check and a one-second
//! governor cooldown, so a cut is seen in seconds. Each unit's node advertises the unit's
//! `[[capability]]`s and defines its `[[group]]`s, as the stem and `POST /gateway/units/declare` do.
//! An attachment the adopter makes in code — an evaluator, a journal, an authority — is not in view,
//! as it is not for `mycelium doctor`.
//!
//! **It never runs an activation** (D23). A unit's `[[activation]]` commands are counted and reported
//! `not_applicable`; none is started, confined or not.
//!
//! **A class that cannot run says so and does not fail the run** unless it was asked for by name:
//! `replay` needs the `sim` build (`cargo build --bin mycelium --features cli,sim`) and reads
//! `not_in_build` naming `sim` on any other; `revoke` needs `tls` and `compliance` in the build and
//! `[tls]` in the configuration; `restart` needs `[persistence]`; `policy-diff` needs `gateway` and
//! `tls` and a second revision (`--against <units-dir>`).
//!
//! **A finding is the fleet's, not the command's.** A check that fails names what the fleet did — a
//! revoked key still verifying at one door, a synced write gone after a restart, a dark member still
//! counted — which is the thing the adopter needed to know before the fleet carried anything real.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::wire_check::{self, ArtifactDescription, CheckOptions, Unit};
use crate::{GossipAgent, GossipConfig, NodeId};

/// The JSON report's schema id; a field added later keeps it, one removed or renamed bumps it.
pub const SCHEMA: &str = "mycelium.test-faults/1";

/// The fault classes, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Revoke,
    Restart,
    Partition,
    Replay,
    PolicyDiff,
    /// Selective recovery (adopter-surface X9): a lost acknowledgement, the closure of its record across the
    /// fleet's journals, each member recovered by its declared class.
    Recovery,
    /// A grant that is default-deny and holds across a restart (adopter-surface X3): the declaration's first
    /// grant-required `[[rule]]` driven on its own fleet — silence refuses, the declared issuer's grant admits one
    /// call, the grant is back after a graceful restart (the exit-snapshot path; the restart that keeps only the WAL
    /// is `grant_tests`').
    Grant,
}

impl Class {
    pub const ALL: [Class; 7] = [Class::Revoke, Class::Restart, Class::Partition, Class::Replay, Class::PolicyDiff, Class::Recovery, Class::Grant];

    /// The class's name as the command line and the report spell it.
    pub fn name(self) -> &'static str {
        match self {
            Class::Revoke => "revoke",
            Class::Restart => "restart",
            Class::Partition => "partition",
            Class::Replay => "replay",
            Class::PolicyDiff => "policy-diff",
            Class::Recovery => "recovery",
            Class::Grant => "grant",
        }
    }

    /// A class by name; `None` for anything else.
    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.iter().copied().find(|c| c.name() == s)
    }

    /// A comma-separated class list. `Ok(None)` when no token is a class name (the token is something else, a
    /// directory); `Err` naming the token when some are and one is not, and for the partition this command does not
    /// run — a two-sided one is the Docker suites' job (D6), and says so when asked.
    pub fn parse_list(s: &str) -> Result<Option<Vec<Class>>, String> {
        let tokens: Vec<&str> = s.split(',').collect();
        if let Some(t) = tokens.iter().find(|t| matches!(**t, "two-sided" | "two-sided-partition" | "cross-process" | "cross-process-partition" | "docker")) {
            return Err(format!("`{t}`: a two-sided partition is not a class here — it needs two processes and a network to cut, which is the Docker suites' job (tests/integration; D6); `partition` is one node gone dark"));
        }
        let parsed: Vec<Option<Class>> = tokens.iter().map(|t| Class::parse(t)).collect();
        if parsed.iter().all(|c| c.is_none()) {
            return Ok(None);
        }
        if let Some(i) = parsed.iter().position(|c| c.is_none()) {
            return Err(format!("`{}` is not a class (revoke · restart · partition · replay · policy-diff · recovery · grant)", tokens[i]));
        }
        Ok(Some(parsed.into_iter().flatten().collect()))
    }
}

/// How a class ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClassState {
    /// Every check ran and held.
    Passed,
    /// At least one check did not hold; the check says what the fleet did.
    Failed,
    /// The feature that provides the class is not compiled into this binary.
    NotInBuild { feature: &'static str },
    /// The build could run it; the configuration or the command line lacks what it needs.
    NotConfigured { missing: String },
    /// Nothing in the declaration for the class to act on.
    NotApplicable { because: String },
    /// The class could not build or start its fleet; `reason` is the refusal, in the node's words.
    Refused { reason: String },
}

/// One check a class made against the fleet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    /// `true` held, `false` did not; `None` when the check could not run (its detail says why).
    pub ok: Option<bool>,
    pub detail: String,
    pub elapsed_ms: u64,
}

/// A guarantee or rule id a class exercised, with its resolution on the fleet's node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Exercised {
    pub id: String,
    /// `guarantee` (the guarantee catalogue) or `rule` (the rule catalogue).
    pub kind: &'static str,
    /// The guarantee report's state for a guarantee (`enforced`, `not_configured`, …); `instrumented` for a
    /// rule whose decision point records to the trace, `catalogue_only` for one that does not.
    pub state: String,
    /// What the state names (the missing setting, the feature, the role fact), when it names one.
    pub detail: Option<String>,
}

/// One class's report.
#[derive(Clone, Debug, Serialize)]
pub struct ClassReport {
    pub class: Class,
    /// Whether the command line named this class (a class that cannot run fails the run only then).
    pub asked: bool,
    pub state: ClassState,
    pub checks: Vec<Check>,
    /// The ids the class exercised, each resolved on the fleet's node.
    pub exercised: Vec<Exercised>,
    /// The ids the class names but could not exercise here, and why — the resolution says.
    pub not_exercised: Vec<Exercised>,
    pub notes: Vec<String>,
    pub elapsed_ms: u64,
}

/// What the fleet was built as.
#[derive(Clone, Debug, Serialize)]
pub struct FleetFacts {
    pub nodes: usize,
    pub units: Vec<String>,
    pub tls: bool,
    pub persistence: bool,
    /// The profile the fleet ran under: always `dev`.
    pub profile: &'static str,
    /// The profile the configuration declared, when it declared one; not applied to the fleet (its requirements
    /// include a gateway and code attachments the fleet has not; `doctor` checks the profile).
    pub profile_declared: Option<String>,
    /// Always `false`: the classes drive the nodes in process; no gateway port is bound.
    pub gateway_started: bool,
}

/// The `[[activation]]` commands the declaration holds, none of them run (D23).
#[derive(Clone, Debug, Serialize)]
pub struct Activations {
    pub declared: usize,
    pub run: usize,
    pub state: &'static str,
    pub because: &'static str,
}

/// The whole report.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub config_path: Option<PathBuf>,
    pub fleet: FleetFacts,
    pub wire_check: WireCheckSummary,
    pub activations: Activations,
    pub classes: Vec<ClassReport>,
    /// The run's directory, when `--keep` kept it.
    pub kept: Option<PathBuf>,
}

/// What `wire-check` said about the declaration before anything ran.
#[derive(Clone, Debug, Serialize)]
pub struct WireCheckSummary {
    pub errors: usize,
    pub warnings: usize,
}

/// Why the command refused before any class ran.
#[derive(Debug)]
pub enum Refused {
    /// The configuration did not load or validate; `reason` is the node's own words.
    Config { path: Option<PathBuf>, reason: String },
    /// `wire-check` found errors; the report is its own, so the command prints the same findings.
    WireCheck(Box<wire_check::Report>),
}

/// What to run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The classes asked for by name; empty means every class.
    pub classes: Vec<Class>,
    /// The configuration file; `None` is the defaults and the environment, as `mycelium` with no `-c`.
    pub config: Option<PathBuf>,
    /// The second revision for `policy-diff`: the proposed declaration's units.
    pub against: Option<Vec<Unit>>,
    /// The kernel seed for `replay`.
    pub seed: u64,
    /// Keep the run's directory (its journals and persistence) instead of removing it; the report names it.
    pub keep: bool,
    /// Evidence journals written elsewhere, whose decided records `policy-diff` re-decides under the proposed
    /// revision — without the argument values the records do not carry, so a record a proposed rule reads values
    /// for is *unattributable*.
    pub decisions: Vec<PathBuf>,
}

impl Report {
    /// `0` when every class that ran passed and no class asked for by name could not run; `1` otherwise.
    pub fn exit_code(&self) -> i32 {
        let bad = self.classes.iter().any(|c| match &c.state {
            ClassState::Passed => false,
            ClassState::Failed | ClassState::Refused { .. } => true,
            ClassState::NotInBuild { .. } | ClassState::NotConfigured { .. } | ClassState::NotApplicable { .. } => c.asked,
        });
        if bad { 1 } else { 0 }
    }

    /// The text form.
    pub fn render(&self) -> String {
        let mut s = String::new();
        let f = &self.fleet;
        s.push_str(&format!(
            "test --faults — {} unit(s) ({}), {} node(s), tls {}, persistence {}, profile {}, config {}\n",
            f.units.len(),
            if f.units.is_empty() { "none".to_string() } else { f.units.join(", ") },
            f.nodes,
            if f.tls { "on" } else { "off" },
            if f.persistence { "on" } else { "off" },
            match &f.profile_declared {
                Some(d) if d != "dev" => format!("{} (declared `{d}`, not applied: a test fleet has no gateway and no code attachment; `mycelium doctor` checks the profile)", f.profile),
                _ => f.profile.to_string(),
            },
            self.config_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "defaults and the environment".into()),
        ));
        s.push_str(&format!("wire-check: {} error(s), {} warning(s)\n", self.wire_check.errors, self.wire_check.warnings));
        s.push_str(&format!(
            "activation: {} — {} command(s) declared, {} run ({})\n",
            self.activations.state, self.activations.declared, self.activations.run, self.activations.because
        ));
        if let Some(k) = &self.kept {
            s.push_str(&format!("kept: {} (journals under <class>/, persistence under <class>/persist/)\n", k.display()));
        }
        // Said once, and only of what this run had: a fleet when a class ran, a CA when it had TLS, listener errors
        // when a class cut a node (dropped its runtime without a shutdown).
        let ran = |c: &ClassReport| matches!(c.state, ClassState::Passed | ClassState::Failed);
        if self.classes.iter().any(ran) {
            let mut logs = vec!["the dev profile"];
            if f.tls {
                logs.push("a CA minted for the run");
            }
            logs.push("the one-second timers set here");
            if self.classes.iter().any(|c| ran(c) && matches!(c.class, Class::Partition | Class::Recovery)) {
                logs.push("a cut node's listener errors");
            }
            // Name only the tables this run had (#690's confirmation round, finding 4).
            let dirs = match (f.persistence, f.tls) {
                (true, true) => " — the configuration's `[persistence]` and `[tls]` turn them on, and their `base_path` and `auto_cert_dir` are replaced",
                (true, false) => " — the configuration's `[persistence]` turns them on, and its `base_path` is replaced",
                (false, true) => " — the configuration's `[tls]` turns them on, and its `auto_cert_dir` is replaced",
                (false, false) => "",
            };
            s.push_str(&format!(
                "fleet: its directories are the run's own, under a temporary directory removed at the end unless --keep{dirs}; \
                 the nodes' log lines on stderr are the fleet's own ({}) and the verdict is this report\n",
                logs.join(", ")
            ));
        }
        for c in &self.classes {
            s.push('\n');
            let state = match &c.state {
                ClassState::Passed => "passed".to_string(),
                ClassState::Failed => "FAILED".to_string(),
                ClassState::NotInBuild { feature } => format!("not_in_build — {feature}"),
                ClassState::NotConfigured { missing } => format!("not_configured — {missing}"),
                ClassState::NotApplicable { because } => format!("not_applicable — {because}"),
                ClassState::Refused { reason } => format!("refused — {reason}"),
            };
            s.push_str(&format!("{}: {state}{} ({} check(s), {:.1} s)\n", c.class.name(), if c.asked { " [asked]" } else { "" }, c.checks.len(), c.elapsed_ms as f64 / 1000.0));
            for k in &c.checks {
                let mark = match k.ok { Some(true) => "✓", Some(false) => "✗", None => "–" };
                s.push_str(&format!("  {mark} {} — {} ({:.1} s)\n", k.name, k.detail, k.elapsed_ms as f64 / 1000.0));
            }
            if !c.exercised.is_empty() {
                let list: Vec<String> = c.exercised.iter().map(|e| format!("{} {}", e.id, e.state)).collect();
                s.push_str(&format!("  exercised: {}\n", list.join(" · ")));
            }
            if !c.not_exercised.is_empty() {
                let list: Vec<String> = c
                    .not_exercised
                    .iter()
                    .map(|e| match &e.detail { Some(d) => format!("{} {} ({d})", e.id, e.state), None => format!("{} {}", e.id, e.state) })
                    .collect();
                s.push_str(&format!("  not exercised: {}\n", list.join(" · ")));
            }
            for n in &c.notes {
                s.push_str(&format!("  note: {n}\n"));
            }
        }
        s.push_str(&format!("\nexit: {}\n", self.exit_code()));
        s
    }
}

/// Every `.toml` under `dir`, as units named by file stem, in name order.
pub fn load_units(dir: &Path) -> Result<Vec<Unit>, String> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| p.file_stem().map(|s| (s.to_string_lossy().into_owned(), p.clone())))
        .collect();
    files.sort();
    let mut units = Vec::new();
    for (name, path) in files {
        let config = crate::NodeCapabilityConfig::load_from_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        units.push(Unit { name, config });
    }
    Ok(units)
}

/// Run the command: `wire-check` over the declaration first (an error refuses the run with the same
/// findings), then the fleet is built per class and the classes run in order.
pub fn run(units: &[Unit], artifacts: &[(String, ArtifactDescription)], check_opts: &CheckOptions, opts: &Options) -> Result<Report, Refused> {
    let checked = wire_check::check(units, artifacts, check_opts);
    if checked.exit_code() != 0 {
        return Err(Refused::WireCheck(Box::new(checked)));
    }
    let base = match &opts.config {
        Some(path) => GossipConfig::load_from_file(path).map_err(|e| Refused::Config { path: Some(path.clone()), reason: e.to_string() })?,
        None => {
            let mut c = GossipConfig::default();
            c.apply_env_overrides().map_err(|e| Refused::Config { path: None, reason: e.to_string() })?;
            c
        }
    };
    let declared_activations: usize = units.iter().map(|u| u.config.activations.len()).sum();
    let node_count = units.len().max(3);
    let dir = scratch_dir();
    let mut classes = Vec::new();
    for class in Class::ALL {
        let asked = opts.classes.contains(&class);
        if !opts.classes.is_empty() && !asked {
            continue;
        }
        let started = crate::sim_seam::mono_instant();
        let mut rep = ClassReport { class, asked, state: ClassState::Passed, checks: Vec::new(), exercised: Vec::new(), not_exercised: Vec::new(), notes: Vec::new(), elapsed_ms: 0 };
        let class_dir = dir.join(class.name());
        match class {
            Class::Revoke => revoke::run(&mut rep, &base, units, &class_dir),
            Class::Restart => restart::run(&mut rep, &base, units, &class_dir),
            Class::Partition => partition::run(&mut rep, &base, units, &class_dir),
            Class::Replay => replay::run(&mut rep, &base, units, &class_dir, opts.seed),
            Class::PolicyDiff => policy_diff::run(&mut rep, &base, units, &checked, opts.against.as_deref(), &opts.decisions, &class_dir),
            Class::Recovery => recovery::run(&mut rep, &base, units, &class_dir),
            Class::Grant => grant::run(&mut rep, &base, units, &class_dir),
        }
        if matches!(rep.state, ClassState::Passed) && rep.checks.iter().any(|k| k.ok == Some(false)) {
            rep.state = ClassState::Failed;
        }
        rep.elapsed_ms = crate::sim_seam::mono_elapsed(&started).as_millis() as u64;
        if !opts.keep {
            let _ = std::fs::remove_dir_all(&class_dir);
        }
        classes.push(rep);
    }
    if !opts.keep {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(Report {
        schema: SCHEMA,
        config_path: opts.config.clone(),
        fleet: FleetFacts {
            nodes: node_count,
            units: units.iter().map(|u| u.name.clone()).collect(),
            tls: base.tls.is_some(),
            persistence: base.persistence.is_some(),
            profile: "dev",
            profile_declared: base.profile.clone(),
            gateway_started: false,
        },
        wire_check: WireCheckSummary {
            errors: checked.findings.iter().filter(|f| f.severity == wire_check::Severity::Error).count(),
            warnings: checked.findings.iter().filter(|f| f.severity == wire_check::Severity::Warning).count(),
        },
        activations: Activations {
            declared: declared_activations,
            run: 0,
            state: "not_applicable",
            because: "test --faults runs no activation: a test command is not a command runner (D23)",
        },
        classes,
        kept: opts.keep.then_some(dir),
    })
}

/// A directory of this run's own for its persistence, TLS material and journals: the process, the clock and a
/// per-process counter, so two runs in one process (the tests run the classes in parallel) never share one.
fn scratch_dir() -> PathBuf {
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("mycelium-test-faults-{}-{}-{n}", std::process::id(), crate::sim_seam::wall_now_ms()))
}

/// `n` loopback ports that were free when asked and that this process has not handed out before: a cursor over
/// `16000..32000` (below the OS ephemeral floor, so a port the OS picks for someone else never collides), seeded
/// from the pid, each candidate bind-verified. A port read from `bind(0)` and released would be the OS's to give
/// again to the next `bind(0)` — two runs in one process (the tests run classes in parallel) collided on exactly
/// that. The same scheme as `test_util::alloc_port`, which is test-only by design.
/// In a test build the one cursor is `test_util::alloc_port`'s: a second cursor over the same range with the same
/// seed hands out the same sequence to a test running beside this one (#675's review, finding 2).
#[cfg(any(test, feature = "test-util"))]
fn free_ports(n: usize) -> Result<Vec<u16>, String> {
    Ok((0..n).map(|_| crate::test_util::alloc_port()).collect())
}

/// The production allocator: a cursor of this process's own, seeded apart from `alloc_port`'s.
#[cfg(not(any(test, feature = "test-util")))]
fn free_ports(n: usize) -> Result<Vec<u16>, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    const LO: u32 = 16_000;
    const SPAN: u32 = 16_000;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = (std::process::id().wrapping_mul(2_246_822_519).wrapping_add(7_919) % SPAN) + 1;
    let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    let mut out = Vec::with_capacity(n);
    for _ in 0..SPAN {
        if out.len() == n {
            break;
        }
        let p = (LO + NEXT.fetch_add(1, Ordering::Relaxed) % SPAN) as u16;
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            out.push(p);
        }
    }
    if out.len() == n { Ok(out) } else { Err("no free loopback port found".into()) }
}

/// The adopter's configuration as fleet node `index` carries it (the module doc's rewrites). Each node gets a
/// persistence directory of its own: a directory has one owner (realignment repairs R1), and the identity
/// ledger's revocation journal lives at the base path, so two nodes on one base path refuse each other.
/// Every node bootstraps to every other, so a node that comes back after a cut finds the survivors itself
/// (the survivors evicted it and no longer dial it).
fn fleet_config(base: &GossipConfig, dir: &Path, index: usize, ids: &[NodeId]) -> GossipConfig {
    let mut cfg = base.clone();
    cfg.bind_address = "127.0.0.1".into();
    // The declared profile is not applied: its requirements include a gateway and code attachments this fleet does
    // not have, and the command clears the certificate paths it would need — `doctor` checks the profile, this
    // checks the fleet under it (#675's review, finding 4). Reported as declared and not applied. The fleet runs
    // under `dev`, selected, so its nodes do not warn of the CA key the run mints for them (#690's review, finding 3).
    cfg.profile = Some("dev".into());
    cfg.bind_port = ids[index].to_socket_addr().port();
    cfg.bootstrap_peers = ids.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, id)| id.clone()).collect();
    cfg.http_port = None;
    cfg.health_check_interval_secs = 1;
    cfg.health_check_max_jitter_ms = 0;
    cfg.reconnect_backoff_secs = 1;
    cfg.membership_cooldown_secs = Some(1);
    if let Some(p) = cfg.persistence.as_mut() {
        p.base_path = persist_dir(dir, index);
    }
    if let Some(t) = cfg.tls.as_mut() {
        t.auto_cert_dir = dir.join("tls");
        t.cert_pem = None;
        t.key_pem = None;
        t.ca_cert_pem = None;
    }
    cfg
}

/// Node `index`'s persistence base path under the class directory.
fn persist_dir(dir: &Path, index: usize) -> PathBuf {
    dir.join("persist").join(format!("node-{index}"))
}

/// Poll `ok` every 25 ms for up to `timeout`, on the current runtime, through the timer seam.
async fn until(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let started = crate::sim_seam::mono_instant();
    loop {
        if ok() {
            return true;
        }
        if crate::sim_seam::mono_elapsed(&started) >= timeout {
            return ok();
        }
        crate::sim_seam::sleep_ms("faults/poll", 25).await;
    }
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

/// The fleet: one node per index, each on a runtime of its own so a node can be cut by dropping its
/// runtime — every task gone at once, no departure sent, its sockets closed by the OS — which is what a
/// node that goes dark looks like to its peers.
struct Fleet<'u> {
    units: &'u [Unit],
    nodes: Vec<Node>,
}

struct Node {
    id: NodeId,
    cfg: GossipConfig,
    rt: Option<tokio::runtime::Runtime>,
    agent: Option<Arc<GossipAgent>>,
    /// Advertisement and group handles: dropping one retracts it, so they live as long as the node.
    held: Vec<Box<dyn std::any::Any + Send>>,
    sink: Arc<crate::decision::DecisionSink>,
}

impl<'u> Fleet<'u> {
    fn plan(base: &GossipConfig, units: &'u [Unit], dir: &Path, count: usize) -> Result<Self, String> {
        let ports = free_ports(count)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut nodes = Vec::with_capacity(count);
        let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        for (i, id) in ids.iter().enumerate() {
            let cfg = fleet_config(base, dir, i, &ids);
            cfg.validate().map_err(|e| format!("the fleet's configuration for node {i} does not validate: {e}"))?;
            nodes.push(Node { id: id.clone(), cfg, rt: None, agent: None, held: Vec::new(), sink: Arc::new(crate::decision::DecisionSink::default()) });
        }
        Ok(Fleet { units, nodes })
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn runtime() -> Result<tokio::runtime::Runtime, String> {
        tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().map_err(|e| e.to_string())
    }

    /// Build and start node `i`, then apply its unit (advertisements and groups) and start its governor.
    /// `before_start` attaches what a class needs attached before the lifecycle boundary.
    fn start_with(&mut self, i: usize, before_start: impl FnOnce(&GossipAgent)) -> Result<(), String> {
        let rt = Self::runtime()?;
        let (id, cfg, sink) = (self.nodes[i].id.clone(), self.nodes[i].cfg.clone(), Arc::clone(&self.nodes[i].sink));
        let unit = self.units.get(i);
        let (agent, held) = rt.block_on(async move {
            let agent = Arc::new(GossipAgent::new(id, cfg));
            agent.with_decision_trace(sink);
            before_start(&agent);
            agent.start().await.map_err(|e| e.to_string())?;
            let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
            if let Some(unit) = unit {
                for c in &unit.config.capabilities {
                    let reg = agent.capabilities().advertise_capability(c.build_capability_public(), Duration::from_secs(c.ttl_secs.max(1)));
                    held.push(Box::new(reg));
                }
                for g in &unit.config.groups {
                    let def = g.to_def().map_err(|e| format!("unit {}: group {}: {e}", unit.name, g.name))?;
                    let handle = agent.capabilities().define_capability_group(g.name.clone(), def, Duration::from_secs(30));
                    held.push(Box::new(handle));
                }
            }
            agent.start_membership_governor();
            Ok::<_, String>((agent, held))
        })?;
        let node = &mut self.nodes[i];
        node.rt = Some(rt);
        node.agent = Some(agent);
        node.held = held;
        Ok(())
    }

    fn start(&mut self, i: usize) -> Result<(), String> {
        self.start_with(i, |_| {})
    }

    fn start_all(&mut self) -> Result<(), String> {
        for i in 0..self.len() {
            self.start(i)?;
        }
        Ok(())
    }

    fn agent(&self, i: usize) -> Arc<GossipAgent> {
        Arc::clone(self.nodes[i].agent.as_ref().expect("a started node"))
    }

    fn is_up(&self, i: usize) -> bool {
        self.nodes[i].agent.is_some()
    }

    /// Run `f` on node `i`'s runtime.
    fn on<F: std::future::Future>(&self, i: usize, f: F) -> F::Output {
        self.nodes[i].rt.as_ref().expect("a started node").block_on(f)
    }

    /// Spawn `f` on node `i`'s runtime and hand back its handle (the `grant` class's waiting call).
    #[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
    fn spawn<F>(&self, i: usize, f: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.nodes[i].rt.as_ref().expect("a started node").spawn(f)
    }

    /// Poll `ok` on node `i`'s runtime for up to `timeout`.
    fn wait(&self, i: usize, timeout: Duration, ok: impl FnMut() -> bool) -> bool {
        self.on(i, until(timeout, ok))
    }

    /// Graceful shutdown: the node's sweep, then its runtime.
    fn stop(&mut self, i: usize) {
        let node = &mut self.nodes[i];
        node.held.clear();
        if let (Some(rt), Some(agent)) = (node.rt.take(), node.agent.take()) {
            rt.block_on(agent.shutdown());
            drop(agent);
            drop(rt);
        }
    }

    /// The cut: the runtime dropped with the node on it — no sweep, no departure, the sockets closed. The
    /// runtime goes first: a retraction handle dropped while it runs would wake the re-assert task to tombstone
    /// the offer, and a node that withdraws its offers on the way out is not dark (#675's review, finding 7).
    fn cut(&mut self, i: usize) {
        let node = &mut self.nodes[i];
        let agent = node.agent.take();
        if let Some(rt) = node.rt.take() {
            drop(rt);
        }
        node.held.clear();
        drop(agent);
    }

    fn stop_all(&mut self) {
        for i in 0..self.len() {
            self.stop(i);
        }
    }

    /// Every node has every other as a peer.
    fn converge(&self, timeout: Duration) -> bool {
        let n = self.len();
        self.wait(0, timeout, || (0..n).all(|i| self.is_up(i) && self.agent(i).peers().len() >= n - 1))
    }

    /// The first node's guarantee report, resolved on the fleet as built.
    fn guarantees(&self) -> Option<crate::agent::guarantee::GuaranteeReport> {
        (0..self.len()).find(|i| self.is_up(*i)).map(|i| self.agent(i).guarantee_report())
    }
}

impl Drop for Fleet<'_> {
    fn drop(&mut self) {
        for i in 0..self.nodes.len() {
            let node = &mut self.nodes[i];
            node.held.clear();
            if let (Some(rt), Some(agent)) = (node.rt.take(), node.agent.take()) {
                rt.block_on(agent.shutdown_with_timeout(Duration::from_secs(2)));
                drop(agent);
                drop(rt);
            }
        }
        // The directory is not removed here: `run` removes each class's directory unless `--keep` asked to keep it,
        // and a fleet that removed its own made every kept run empty (#681's review, finding 1).
    }
}

/// A guarantee's resolution on the fleet's node, in the report's words. With no started node (a class that did
/// not run), the configuration is resolved over a node built and never started, as `doctor` does — never
/// `unknown` (#675's review, finding 6).
fn resolution_of(report: Option<&crate::agent::guarantee::GuaranteeReport>, id: &str) -> (String, Option<String>) {
    use crate::agent::guarantee::Resolution;
    match report.and_then(|r| r.entries.iter().find(|e| e.id == id)).map(|e| &e.resolution) {
        None => ("not_verifiable_here".to_string(), Some("no node could be built from this configuration to resolve it".to_string())),
        Some(Resolution::Enforced) => ("enforced".to_string(), None),
        Some(Resolution::NotConfigured { missing }) => ("not_configured".to_string(), Some(missing.to_string())),
        Some(Resolution::NotInBuild { feature }) => ("not_in_build".to_string(), Some(feature.to_string())),
        Some(Resolution::NotApplicable { because }) => ("not_applicable".to_string(), Some(because.to_string())),
        Some(Resolution::NotVerifiableHere { evidence }) => ("not_verifiable_here".to_string(), Some(evidence.to_string())),
    }
}

/// The guarantee report of a node built from `base` and never started — what a class that could not run resolves
/// its ids against (the same view `mycelium doctor` reports).
fn unstarted_report(base: &GossipConfig) -> Option<crate::agent::guarantee::GuaranteeReport> {
    let port = free_ports(1).ok()?[0];
    let id = NodeId::new("127.0.0.1", port).ok()?;
    let cfg = fleet_config(base, &std::env::temp_dir(), 0, std::slice::from_ref(&id));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    Some(rt.block_on(async move { GossipAgent::new(id, cfg).guarantee_report() }))
}

/// The ids a class **drove** — its checks ran against them — each with its resolution on the fleet's node
/// (`exercised`), and a rule's trace policy. A guarantee the node resolves other than `enforced` is still listed as
/// exercised when the class drove the code behind it: the state says what the node enforces, the list says what
/// the class did.
fn exercised(rep: &mut ClassReport, report: Option<&crate::agent::guarantee::GuaranteeReport>, guarantees: &[&str], rules: &[&str]) {
    for id in guarantees {
        let (state, detail) = resolution_of(report, id);
        rep.exercised.push(Exercised { id: id.to_string(), kind: "guarantee", state, detail });
    }
    for id in rules {
        let trace = crate::rules::RULES.iter().find(|r| r.id == *id).map(|r| match r.trace {
            crate::rule::TracePolicy::Instrumented => "instrumented",
            crate::rule::TracePolicy::Partial(_) => "partial",
            _ => "catalogue_only",
        });
        rep.exercised.push(Exercised { id: id.to_string(), kind: "rule", state: trace.unwrap_or("catalogue_only").to_string(), detail: None });
    }
}

/// The ids a class names but **could not drive** here, with why — the class state that stopped it, or the check
/// that did not run — each with its resolution on a node built from the configuration.
fn not_exercised(rep: &mut ClassReport, report: Option<&crate::agent::guarantee::GuaranteeReport>, guarantees: &[&str], rules: &[&str], why: &str) {
    for id in guarantees {
        let (state, _) = resolution_of(report, id);
        rep.not_exercised.push(Exercised { id: id.to_string(), kind: "guarantee", state, detail: Some(why.to_string()) });
    }
    for id in rules {
        rep.not_exercised.push(Exercised { id: id.to_string(), kind: "rule", state: "not_run".to_string(), detail: Some(why.to_string()) });
    }
}

/// The reason a class that did not run gives for every id it names.
fn class_reason(state: &ClassState) -> String {
    match state {
        ClassState::Passed | ClassState::Failed => "the class ran".to_string(),
        ClassState::NotInBuild { feature } => format!("not_in_build: {feature}"),
        ClassState::NotConfigured { missing } => format!("not_configured: {missing}"),
        ClassState::NotApplicable { because } => format!("not_applicable: {because}"),
        ClassState::Refused { reason } => format!("refused: {reason}"),
    }
}

/// Record one check.
fn check(rep: &mut ClassReport, started: &std::time::Instant, name: &str, ok: Option<bool>, detail: impl Into<String>) {
    rep.checks.push(Check { name: name.to_string(), ok, detail: detail.into(), elapsed_ms: ms(crate::sim_seam::mono_elapsed(started)) });
}

// ───────────────────────────────────────────────────────────────────────────────────── restart ──

mod restart {
    //! What this class proves: the synced write is read back after a restart through `shutdown()`, whose exit
    //! snapshot is taken from the **store** — so a record the store held and the WAL never took is carried over by
    //! that snapshot too. The WAL path alone is proved by the restart that keeps only the WAL (`test_util::
    //! sabotage_exit_snapshot`, external review 2026-10-31, R2); a hard-crash restart class — the exit snapshot
    //! denied, only the WAL replayed — is **not built** here.
    use super::*;

    const GUARANTEES: &[&str] = &["persist.configured", "persist.sync_mode", "persist.unreadable_refused"];

    pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path) {
        let Some(pcfg) = base.persistence.as_ref() else {
            rep.state = ClassState::NotConfigured { missing: "[persistence]".into() };
            let why = class_reason(&rep.state);
            not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, &["kv.propagation"], &why);
            return;
        };
        let mut fleet = match Fleet::plan(base, units, dir, units.len().max(3)) {
            Ok(f) => f,
            Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
        };
        if let Err(reason) = fleet.start_all() {
            rep.state = ClassState::Refused { reason };
            return;
        }
        exercised(rep, fleet.guarantees().as_ref(), GUARANTEES, &["kv.propagation"]);
        let n = fleet.len();

        // One synced write per node: the receipt must name rung 2.
        let t = crate::sim_seam::mono_instant();
        let mut receipts_ok = true;
        let mut rungs = Vec::new();
        for i in 0..n {
            let key = format!("faults/restart/{}", fleet.nodes[i].id);
            let agent = fleet.agent(i);
            let r = fleet.on(i, async move {
                agent.kv().set_requiring_sync(&mycelium_core::receipt::OperationId::new(format!("test-faults-restart-{i}")), key, b"before-restart".to_vec()).await
            });
            match r {
                Ok(receipt) => {
                    let on_disk = matches!(receipt.local_durability, mycelium_core::receipt::LocalDurability::OnDisk);
                    rungs.push(format!("node {i}: {:?}", receipt.local_durability));
                    receipts_ok &= on_disk;
                }
                Err(e) => { rungs.push(format!("node {i}: refused: {e}")); receipts_ok = false; }
            }
        }
        check(rep, &t, "a write requiring sync is acknowledged at rung 2, on disk, on every node", Some(receipts_ok), rungs.join("; "));

        // Restart every node over its own directory: the synced write is back.
        let t = crate::sim_seam::mono_instant();
        fleet.stop_all();
        let mut restarted = Vec::new();
        for i in 0..n {
            if let Err(e) = fleet.start(i) {
                check(rep, &t, "every node restarts over its own directory", Some(false), format!("node {i}: {e}"));
                return;
            }
            let key = format!("faults/restart/{}", fleet.nodes[i].id);
            let agent = fleet.agent(i);
            let back = agent.kv().get(&key).is_some_and(|v| v.as_ref() == b"before-restart");
            restarted.push(format!("node {i}: {}", if back { "replayed" } else { "MISSING" }));
        }
        let all_back = restarted.iter().all(|s| s.ends_with("replayed"));
        check(rep, &t, "a synced write is back after a graceful restart (the exit-snapshot path; the WAL alone is proved by the WAL-only restart test)", Some(all_back), restarted.join("; "));

        // Unreadable state: a complete record that does not decode, appended to node 0's WAL.
        let t = crate::sim_seam::mono_instant();
        fleet.stop(0);
        let wal = persist_dir(dir, 0).join(fleet.nodes[0].id.to_string()).join("kv").join("wal.bin");
        let corrupt = {
            use std::io::Write;
            std::fs::OpenOptions::new().append(true).open(&wal).and_then(|mut f| {
                f.write_all(&8u32.to_le_bytes())?;
                f.write_all(&[0xFF; 8])
            })
        };
        if let Err(e) = corrupt {
            check(rep, &t, "unreadable persisted state is refused at start, by name", None, format!("could not plant a corrupt record in {}: {e}", wal.display()));
        } else {
            let outcome = fleet.start(0);
            match pcfg.on_unreadable {
                mycelium_core::config::OnUnreadable::Refuse => {
                    let refused = outcome.as_ref().err().is_some_and(|e| e.contains("corrupt") || e.contains("unreadable"));
                    check(rep, &t, "unreadable persisted state is refused at start, by name (on_unreadable = refuse)", Some(refused), match &outcome {
                        Err(e) => format!("start() refused: {e}"),
                        Ok(()) => "start() started over a WAL holding a record that does not decode".to_string(),
                    });
                }
                mycelium_core::config::OnUnreadable::Quarantine => {
                    let moved = std::fs::read_dir(wal.parent().unwrap_or(dir)).map(|d| d.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().contains(".unreadable-"))).unwrap_or(false);
                    let ok = outcome.is_ok() && moved;
                    check(rep, &t, "unreadable persisted state is moved aside and the node starts (on_unreadable = quarantine)", Some(ok), match &outcome {
                        Ok(()) => format!("started; a *.unreadable-N file {}", if moved { "exists" } else { "does NOT exist" }),
                        Err(e) => format!("start() refused: {e}"),
                    });
                }
            }
        }
        rep.notes.push(format!("persistence relocated to {} (one directory per node) for the run; sync_mode {:?}; on_unreadable {:?}", dir.join("persist").display(), pcfg.sync_mode, pcfg.on_unreadable));
    }
}

// ──────────────────────────────────────────────────────────────────────────────────── recovery ──

mod recovery;

// ─────────────────────────────────────────────────────────────────────────────────────── grant ──

mod grant;
#[cfg(feature = "sim")]
pub use recovery::{Seamed, SeamedOutcome};

/// The `recovery` class's **seamed form** (adopter-surface X9, `sim` only): the fleet is built and the effects
/// land for real, then the two recovery phases run under `seamed.kernel` — recording what the reconcilers asked
/// the destination and the clock, or replaying it with the destination **not reached**. The report is the
/// class's, as `test --faults recovery` would print it; the outcome carries the trace. `tests/faults_recovery_replay.rs`
/// records, compares with the corpus and replays.
#[cfg(all(feature = "sim", feature = "gateway", feature = "tls"))]
pub fn run_recovery_seamed(units: &[Unit], base: &GossipConfig, dir: &Path, seamed: Seamed) -> Result<(ClassReport, SeamedOutcome), String> {
    let mut rep = ClassReport { class: Class::Recovery, asked: true, state: ClassState::Passed, checks: Vec::new(), exercised: Vec::new(), not_exercised: Vec::new(), notes: Vec::new(), elapsed_ms: 0 };
    let out = recovery::run_seamed(&mut rep, base, units, dir, seamed)?;
    if matches!(rep.state, ClassState::Passed) && rep.checks.iter().any(|k| k.ok == Some(false)) {
        rep.state = ClassState::Failed;
    }
    Ok((rep, out))
}

// ─────────────────────────────────────────────────────────────────────────────────── partition ──

mod partition {
    use super::*;

    const GUARANTEES: &[&str] = &["mesh.tls"];
    const RULES: &[&str] = &["membership.governed", "kv.expiry", "cap.match"];

    pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path) {
        let mut fleet = match Fleet::plan(base, units, dir, units.len().max(3)) {
            Ok(f) => f,
            Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
        };
        if let Err(reason) = fleet.start_all() {
            rep.state = ClassState::Refused { reason };
            return;
        }
        exercised(rep, fleet.guarantees().as_ref(), GUARANTEES, RULES);
        let n = fleet.len();
        let t = crate::sim_seam::mono_instant();
        let converged = fleet.converge(Duration::from_secs(30));
        check(rep, &t, "every node has every other as a peer", Some(converged), format!("{n} nodes"));
        if !converged {
            return;
        }

        // The governed groups: each declared group governed to a band of one — a floor of one, so a cut member
        // must be re-filled by an eligible survivor, and a ceiling of one, so exactly one node holds the seat and
        // the cut is seen as a deficit rather than absorbed by a second member that was never asked for.
        let groups: Vec<(usize, String, crate::CapFilter)> = units
            .iter()
            .enumerate()
            .flat_map(|(i, u)| u.config.groups.iter().filter_map(move |g| g.filter.to_filter().ok().map(|f| (i, g.name.clone(), f))))
            .collect();
        for (i, name, _) in &groups {
            let agent = fleet.agent(*i);
            let _ = agent.publish_membership_intent(crate::MembershipIntent::new(name.clone(), 1, Some(1)));
        }
        let member_count = |fleet: &Fleet<'_>, group: &str| (0..fleet.len()).filter(|i| fleet.is_up(*i) && fleet.agent(*i).groups().iter().any(|g| g.as_ref() == group)).count();
        for (_, name, _) in &groups {
            // Exactly one: two eligible nodes may both roll in before the ceiling is seen, and one then leaves. The
            // cut below is that seat's, so the re-fill is a deficit a survivor must fill, never a seat it held already.
            // Settled means held: two nodes that both rolled in and then both rolled out pass through one member on
            // the way, and a victim chosen in that instant is not the seat's holder a moment later. Three governor
            // periods at exactly one is the stable state (no leave at the ceiling, no join at the floor).
            let t = crate::sim_seam::mono_instant();
            let mut held_since: Option<std::time::Instant> = None;
            let settled = fleet.wait(0, Duration::from_secs(40), || {
                if member_count(&fleet, name) == 1 {
                    let since = *held_since.get_or_insert_with(crate::sim_seam::mono_instant);
                    crate::sim_seam::mono_elapsed(&since) >= Duration::from_secs(3)
                } else {
                    held_since = None;
                    false
                }
            });
            check(rep, &t, &format!("the governed group `{name}` settles to its band of one and holds it"), Some(settled), format!("{} member(s)", member_count(&fleet, name)));
        }

        // The victim: a member of a governed group, else the first node that offers something, else node 0.
        let victim = (0..n)
            .find(|i| groups.iter().any(|(_, g, _)| fleet.agent(*i).groups().iter().any(|h| h.as_ref() == g)))
            .or_else(|| (0..n).find(|i| units.get(*i).is_some_and(|u| !u.config.capabilities.is_empty())))
            .unwrap_or(0);
        let observer = (0..n).find(|i| *i != victim).expect("three nodes at least");
        let victim_id = fleet.nodes[victim].id.clone();
        let offers: Vec<(crate::CapFilter, u64)> = units
            .get(victim)
            .map(|u| u.config.capabilities.iter().map(|c| (crate::CapFilter::new(c.ns.as_str(), c.name.as_str()), c.ttl_secs.max(1))).collect())
            .unwrap_or_default();
        let victim_groups: Vec<String> = fleet.agent(victim).groups().iter().map(|g| g.to_string()).collect();

        // Plant: the victim's offers are visible at the observer before the cut.
        let t = crate::sim_seam::mono_instant();
        let visible = fleet.wait(observer, Duration::from_secs(20), || offers.iter().all(|(f, _)| fleet.agent(observer).capabilities().resolve(f).iter().any(|(id, _)| *id == victim_id)));
        check(rep, &t, "the victim's offers resolve at a peer before the cut", Some(visible || offers.is_empty()), format!("node {victim} ({victim_id}), {} offer(s), observer node {observer}", offers.len()));

        // The cut — and, in the same instant, the re-fill baseline: a survivor's join decisions so far and the seats
        // survivors hold. Sampled later (after the evaporation wait, up to 3 × ttl_secs) the survivor's re-fill is
        // already inside the baseline and the check demands a second join that never comes (#675's review,
        // finding 3: the co-op fixture's `depots` group failed exactly so).
        let eviction = Duration::from_secs(fleet.nodes[observer].cfg.health_check_interval_secs.saturating_mul(fleet.nodes[observer].cfg.peer_eviction_intervals).max(1));
        let joins = |fleet: &Fleet<'_>, group: &str| (0..n).filter(|i| *i != victim).map(|i| fleet.nodes[i].sink.snapshot().iter().filter(|r| r.rule == "membership.governed" && r.reason == "join" && r.target.as_deref() == Some(group)).count()).sum::<usize>();
        fleet.cut(victim);
        let baseline: Vec<(String, usize, usize)> = victim_groups.iter().map(|g| (g.clone(), joins(&fleet, g), member_count(&fleet, g))).collect();
        let t = crate::sim_seam::mono_instant();
        let evicted = fleet.wait(observer, eviction.saturating_mul(3) + Duration::from_secs(5), || (0..n).filter(|i| *i != victim).all(|i| !fleet.agent(i).peers().contains(&victim_id)));
        check(rep, &t, "a node that goes dark is evicted from every survivor's membership", Some(evicted), format!("eviction window {} s", eviction.as_secs()));

        let longest_ttl = offers.iter().map(|(_, ttl)| *ttl).max().unwrap_or(0);
        if !offers.is_empty() {
            let t = crate::sim_seam::mono_instant();
            let gone = fleet.wait(observer, Duration::from_secs(3 * longest_ttl + 10), || offers.iter().all(|(f, _)| !fleet.agent(observer).capabilities().resolve(f).iter().any(|(id, _)| *id == victim_id)));
            check(rep, &t, "the dark node's offers leave the view after their evaporation window", Some(gone), format!("3 × ttl_secs {longest_ttl} = {} s", 3 * longest_ttl));
        }

        for (group, before, survivors_at_cut) in &baseline {
            let (before, survivors_at_cut) = (*before, *survivors_at_cut);
            let t = crate::sim_seam::mono_instant();
            let eligible_survivor = (0..n).any(|i| i != victim && groups.iter().any(|(_, g, f)| g == group && units.get(i).is_some_and(|u| u.config.capabilities.iter().any(|c| f.matches(&c.build_capability_public())))));
            if !eligible_survivor {
                check(rep, &t, &format!("the governed group `{group}` is re-filled after its member goes dark"), None, "no surviving node is eligible for the group: the fleet cannot re-fill this floor, by its own declaration");
                continue;
            }
            // A survivor's own join decision after the cut is required, not just a member among survivors: a seat a
            // second member already held would otherwise read as a re-fill.
            if survivors_at_cut >= 1 {
                // The band moved between the settled read and the cut (two nodes can swap the seat): a survivor
                // holds it, so the floor stands and nothing need roll. Said as it is; the re-fill is not exercised.
                check(rep, &t, &format!("the governed group `{group}` is re-filled by a survivor after its member goes dark"), Some(true), format!("{survivors_at_cut} survivor(s) held a seat at the cut: the floor held through it, and no re-fill was needed (not exercised this run)"));
                continue;
            }
            let refilled = fleet.wait(observer, Duration::from_secs(3 * longest_ttl.max(10) + 15), || member_count(&fleet, group) >= 1 && joins(&fleet, group) > before);
            check(rep, &t, &format!("the governed group `{group}` is re-filled by a survivor after its member goes dark"), Some(refilled), format!("no survivor held a seat at the cut; {} live member(s) among survivors now; {} join decision(s) recorded by survivors after the cut", member_count(&fleet, group), joins(&fleet, group).saturating_sub(before)));
        }

        // The heal: the node back under the same identity.
        let t = crate::sim_seam::mono_instant();
        match fleet.start(victim) {
            Err(e) => check(rep, &t, "the dark node comes back under its identity", Some(false), e),
            Ok(()) => {
                let back = fleet.wait(observer, Duration::from_secs(30), || fleet.agent(observer).peers().contains(&victim_id) && offers.iter().all(|(f, _)| fleet.agent(observer).capabilities().resolve(f).iter().any(|(id, _)| *id == victim_id)));
                check(rep, &t, "the node back under its identity is a peer again and its offers resolve", Some(back), format!("{} offer(s)", offers.len()));
            }
        }
        rep.notes.push("the cut is the node's runtime dropped: no departure is sent, its sockets close; the dark side is not observed — a two-sided partition is not a class here (`--faults two-sided` says so): it is the Docker suites' job (D6)".into());
        if groups.is_empty() {
            rep.notes.push("the declaration defines no [[group]]: the governor half observed nothing to govern".into());
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────────────── revoke ──

mod revoke {
    use super::*;

    const GUARANTEES: &[&str] = &["id.revocation_authority", "mesh.signed_writes", "mesh.tls", "authz.execution_authority", "authz.durable_epochs"];
    #[cfg(all(feature = "tls", feature = "compliance"))]
    const KEY_GUARANTEES: &[&str] = &["id.revocation_authority", "mesh.signed_writes", "mesh.tls"];
    #[cfg(all(feature = "tls", feature = "compliance"))]
    const MANDATE_GUARANTEES: &[&str] = &["authz.execution_authority", "authz.durable_epochs"];

    #[cfg(not(all(feature = "tls", feature = "compliance")))]
    pub(super) fn run(rep: &mut ClassReport, _base: &GossipConfig, _units: &[Unit], _dir: &Path) {
        rep.state = ClassState::NotInBuild { feature: if cfg!(feature = "tls") { "compliance" } else { "tls" } };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(_base).as_ref(), GUARANTEES, &[], &why);
        rep.notes.push("the key half rotates a node's identity on compromise, which the `compliance` feature provides".into());
    }

    #[cfg(all(feature = "tls", feature = "compliance"))]
    pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path) {
        use crate::agent::fault_probes as probe;
        if base.tls.is_none() {
            rep.state = ClassState::NotConfigured { missing: "[tls]".into() };
            let why = class_reason(&rep.state);
            not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, &[], &why);
            return;
        }
        // Three nodes: C (node 0, the first unit's) is compromised and rotates; A (node 1) is the holder of
        // its old key; B (node 2) is where every verifier is read.
        let mut fleet = match Fleet::plan(base, units, dir, 3) {
            Ok(f) => f,
            Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
        };
        for i in 0..3 {
            if let Err(reason) = fleet.start(i) {
                rep.state = ClassState::Refused { reason };
                return;
            }
        }
        // The key half's ids are driven by every run; the mandate half's are listed by `mandate_half`, which
        // knows whether it ran.
        exercised(rep, fleet.guarantees().as_ref(), KEY_GUARANTEES, &[]);
        rep.notes.push("three nodes of the fleet (nodes 0, 1 and 2): the compromised node, the holder of its old key, the verifying peer".into());
        let (c, a, b) = (0usize, 1usize, 2usize);
        let (nc, nb) = (fleet.nodes[c].id.clone(), fleet.nodes[b].id.clone());

        let t = crate::sim_seam::mono_instant();
        let contact = fleet.wait(b, Duration::from_secs(30), || probe::knows(&fleet.agent(b), &nc) && probe::knows(&fleet.agent(c), &nb) && probe::knows(&fleet.agent(a), &nc) && !fleet.agent(b).peers().is_empty());
        check(rep, &t, "first contact: every node holds the others' keys", Some(contact), "three TLS nodes");
        if !contact {
            return;
        }
        let Some(stolen) = probe::steal(&fleet.agent(c)) else {
            check(rep, &t, "the compromised node has a TLS identity", Some(false), "no identity");
            return;
        };
        let k0 = stolen.verifying_key;
        let before = probe::handshake(Arc::clone(&stolen.client), &fleet.agent(b));

        // The rotation and the revocation, seen at B.
        let t = crate::sim_seam::mono_instant();
        let agent_c = fleet.agent(c);
        let rotated = fleet.on(c, async move { agent_c.rotate_identity_on_compromise(Duration::from_millis(500)).await });
        let k1 = match rotated {
            Ok(k1) => k1,
            Err(e) => { check(rep, &t, "the compromised node rotates and revokes its key", Some(false), e.to_string()); return; }
        };
        let seen = fleet.wait(b, Duration::from_secs(20), || !probe::trusts(&fleet.agent(b), &nc, &k0) && probe::trusts(&fleet.agent(b), &nc, &k1));
        check(rep, &t, "a peer sees the revocation: the old key untrusted, the new key held", Some(seen), format!("old {} new {}", hex(&k0[..4]), hex(&k1[..4])));
        if !seen {
            return;
        }

        // Verifier 1: the KV arm. A frame signed by the revoked key in C's name does not land at B; C's own
        // write under its new key does (the plant that shows the channel is open).
        let t = crate::sim_seam::mono_instant();
        probe::send_signed_as(&fleet.agent(a), &nc, &stolen.signing_key, "faults/revoke/as-c", b"by-revoked-key".to_vec());
        let agent_c = fleet.agent(c);
        let _ = fleet.on(c, async move { agent_c.kv().set("faults/revoke/plant", b"by-new-key".to_vec()) });
        let plant = fleet.wait(b, Duration::from_secs(10), || fleet.agent(b).kv().get("faults/revoke/plant").is_some());
        fleet.on(b, crate::sim_seam::sleep_ms("faults/settle", 500));
        let landed = fleet.agent(b).kv().get("faults/revoke/as-c").is_some();
        check(rep, &t, "the KV arm: a write signed by the revoked key is refused at a peer", Some(plant && !landed), format!("plant under the new key {}; the revoked key's write {}", if plant { "landed" } else { "did NOT land" }, if landed { "LANDED" } else { "was refused" }));

        // Verifier 2: the mTLS handshake. The stolen certificate and key are refused at B; the current ones accepted.
        let t = crate::sim_seam::mono_instant();
        let stolen_now = probe::handshake(Arc::clone(&stolen.client), &fleet.agent(b));
        let current = probe::steal(&fleet.agent(c)).map(|s| probe::handshake(s.client, &fleet.agent(b))).unwrap_or(Err("no identity".into()));
        let ok = before.is_ok() && current.is_ok() && stolen_now.as_ref().is_err_and(|e| e.contains("revoked"));
        check(rep, &t, "the handshake: a revoked key's certificate is refused at a peer's accept side", Some(ok), format!("before rotation {:?}; stolen now {:?}; current {:?}", before.as_ref().map(|_| "accepted"), stolen_now.as_ref().map(|_| "accepted"), current.as_ref().map(|_| "accepted")));

        // Verifier 3: identity chaining. A sealed record for C chained through the revoked key, carrying an
        // attacker's key, written as A's own signed write: B merges nothing from it.
        let t = crate::sim_seam::mono_instant();
        let ka = ed25519_dalek::SigningKey::from_bytes(&[0x5a; 32]).verifying_key().to_bytes();
        let key = probe::sealed_identity_key(&nc);
        let forged = probe::sealed_record(&[ka, k1, k0], &stolen.signing_key);
        let agent_a = fleet.agent(a);
        let (k2, f2) = (key.clone(), forged.clone());
        let _ = fleet.on(a, async move { agent_a.kv().set(k2, f2) });
        let stored = fleet.wait(b, Duration::from_secs(10), || fleet.agent(b).kv().get(&key).is_some_and(|v| v.as_ref() == forged.as_slice()));
        fleet.on(b, crate::sim_seam::sleep_ms("faults/settle", 500));
        let merged = probe::trusts(&fleet.agent(b), &nc, &ka);
        check(rep, &t, "identity chaining: a record chained through the revoked key adds no key at a peer", Some(stored && !merged), format!("the forged record {} at the peer's store; the attacker key {}", if stored { "arrived" } else { "did NOT arrive" }, if merged { "WAS MERGED" } else { "was not merged" }));

        // The mandate half: the declaration's first [[mandate]], granted, established, then revoked by a checkpoint.
        mandate_half(rep, &fleet, units);
        rep.notes.push("the key half is row Q's three verifiers an in-process fleet can reach — the KV arm, the handshake, identity chaining; consensus, caller envelopes and knowledge records are the suite's (`src/lib_tests.rs`)".into());
    }

    #[cfg(all(feature = "tls", feature = "compliance"))]
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[cfg(all(feature = "tls", feature = "compliance", not(feature = "gateway")))]
    fn mandate_half(rep: &mut ClassReport, fleet: &Fleet<'_>, _units: &[Unit]) {
        let t = crate::sim_seam::mono_instant();
        check(rep, &t, "a mandate: established at the execution authority, then refused once revoked", None, "the execution authority is built with `gateway`; not in this build");
        not_exercised(rep, fleet.guarantees().as_ref(), MANDATE_GUARANTEES, &[], "not_in_build: gateway");
    }

    #[cfg(all(feature = "tls", feature = "compliance", feature = "gateway"))]
    fn mandate_half(rep: &mut ClassReport, fleet: &Fleet<'_>, units: &[Unit]) {
        use crate::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
        use crate::knowledge::IssuerId;
        use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint};
        use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
        use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
        use crate::{mandate_operation, possession_request, ExecutionAuthority, MandateState, PresentedMandate};
        use ed25519_dalek::Signer;

        let t = crate::sim_seam::mono_instant();
        let Some((unit, decl)) = units.iter().find_map(|u| u.config.mandates.first().map(|m| (u, m))) else {
            check(rep, &t, "a mandate: established at the execution authority, then refused once revoked", None, "the declaration has no [[mandate]]");
            not_exercised(rep, fleet.guarantees().as_ref(), MANDATE_GUARANTEES, &[], "the declaration has no [[mandate]]");
            return;
        };
        exercised(rep, fleet.guarantees().as_ref(), MANDATE_GUARANTEES, &[]);
        let holder_name = format!("token:test-faults/{}", decl.holder);
        let authority_name = decl.established_by.clone().unwrap_or_else(|| "operator:test-faults".into());
        let (Some(holder), Some(authority_id)) = (PrincipalId::new(&holder_name), PrincipalId::new(&authority_name)) else {
            check(rep, &t, "a mandate: established at the execution authority, then refused once revoked", None, format!("unit {}: the mandate's holder or establishing authority is not a principal id", unit.name));
            return;
        };
        let (operation, resource) = decl
            .operations
            .first()
            .and_then(|op| op.split_once(':'))
            .map(|(o, r)| (o.to_string(), format!("{r}@{}", fleet.nodes[0].id)))
            .unwrap_or_else(|| ("tools/call".to_string(), format!("tool:faults@{}", fleet.nodes[0].id)));
        let (authority_key, holder_key) = (ed25519_dalek::SigningKey::from_bytes(&[0x61; 32]), ed25519_dalek::SigningKey::from_bytes(&[0x62; 32]));
        let mut entitled = EntitlementTable::new();
        entitled.entitle(decl.scope.clone(), authority_id.clone());
        let mut external = TrustedExternalIssuers::new();
        let trusted = IssuerId::new(&authority_name).and_then(|i| external.trust(i, authority_key.verifying_key().to_bytes()).ok()).is_some()
            && IssuerId::new(&holder_name).and_then(|i| external.trust(i, holder_key.verifying_key().to_bytes()).ok()).is_some();
        if !trusted {
            check(rep, &t, "a mandate: established at the execution authority, then refused once revoked", None, "the holder or authority name is not an issuer id");
            return;
        }
        let gate = match ExecutionGate::strict(ResourceAuthority::new(decl.scope.clone(), 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 }) {
            Ok(g) => g,
            Err(e) => { check(rep, &t, "a mandate: established at the execution authority, then refused once revoked", None, format!("{e:?}")); return; }
        };
        let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitled), external));
        // Attached to a fleet node — after start() here, which the node counts as a late attachment and the
        // report says so; the revocation view starts now, as C8 requires.
        let (node, attach) = (fleet.agent(2), Arc::clone(&authority));
        fleet.on(2, async move { node.with_execution_authority(attach) });
        let now = crate::sim_seam::wall_now_ms();
        let members: std::collections::HashMap<NodeId, MemberKeys> = std::collections::HashMap::new();
        let sign = |k: &ed25519_dalek::SigningKey, m: &[u8]| k.sign(m).to_bytes().to_vec();
        let c0 = RevocationCheckpoint { authority: authority_id.clone(), scope: decl.scope.clone(), seq: 1, issued_at_ms: now + 1_000, revoked: Default::default() };
        let signed0 = SignedRevocationCheckpoint { signature: sign(&authority_key, &c0.canonical_bytes()), checkpoint: c0 };
        let _ = authority.offer_checkpoint(&signed0, now + 1_000, &members);
        let term = TermId::new("test-faults-t1").expect("a term id");
        let mandate = Mandate {
            holder: holder.clone(),
            established_by: authority_id.clone(),
            purpose: decl.purpose.clone().unwrap_or_else(|| "test --faults".into()),
            scope: decl.scope.clone(),
            operations: vec![mandate_operation(&operation, &resource)],
            epoch: 1,
            term: term.clone(),
            valid_from_ms: 0,
            valid_until_ms: now + 600_000,
        };
        let grant = SignedMandateGrant { signature: sign(&authority_key, &mandate.canonical_bytes()), mandate };
        let digest = [7u8; 32];
        let proof = sign(&holder_key, &possession_message(&grant.mandate, &possession_request(&operation, &resource, &digest)));
        let presented = PresentedMandate { grant, possession: base64_encode(&proof) };
        let established = authority.assess(&holder_name, &operation, &resource, &digest, Some(&presented), now + 2_000, &members);
        let was_established = matches!(established.binding.as_ref().map(|b| &b.state), Some(MandateState::Established));
        let c1 = RevocationCheckpoint { authority: authority_id, scope: decl.scope.clone(), seq: 2, issued_at_ms: now + 3_000, revoked: std::iter::once(term).collect() };
        let signed1 = SignedRevocationCheckpoint { signature: sign(&authority_key, &c1.canonical_bytes()), checkpoint: c1 };
        let offered = authority.offer_checkpoint(&signed1, now + 3_000, &members);
        let after = authority.assess(&holder_name, &operation, &resource, &digest, Some(&presented), now + 4_000, &members);
        // A revoked appointment is *not established* — `Unknown`, naming the revocation — never a permit and never
        // read as a prohibition (the gate's `ExecutionDenial::Revoked`, mapped by `assess`).
        let refused = matches!(after.binding.as_ref().map(|b| &b.state), Some(MandateState::Unknown(why)) if why.contains("revoked"));
        check(
            rep,
            &t,
            "a mandate: established at the execution authority, then not established once a checkpoint revokes its term",
            Some(was_established && refused),
            format!("unit {}: holder {holder_name}, scope {}, {operation} on {resource}; established {was_established}; checkpoint {offered:?}; after: {:?}", unit.name, decl.scope, after.binding.map(|b| b.state)),
        );
    }

    #[cfg(all(feature = "tls", feature = "compliance", feature = "gateway"))]
    fn base64_encode(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}

// ────────────────────────────────────────────────────────────────────────────────── policy-diff ──

mod policy_diff {
    use super::*;

    const GUARANTEES: &[&str] = &["ae.authorised_at_seam", "ae.recorded_before_dispatch"];
    const RULES: &[&str] = &["ae.preflight"];

    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    pub(super) fn run(rep: &mut ClassReport, _base: &GossipConfig, _units: &[Unit], _checked: &wire_check::Report, _against: Option<&[Unit]>, _decisions: &[PathBuf], _dir: &Path) {
        rep.state = ClassState::NotInBuild { feature: if cfg!(feature = "gateway") { "tls" } else { "gateway" } };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(_base).as_ref(), GUARANTEES, RULES, &why);
    }

    /// The journal, opened on the node's runtime and handed back out of the start closure.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    type Opened = Arc<std::sync::Mutex<Option<Result<Arc<crate::EvidenceJournal>, String>>>>;

    /// One call the run decided, kept beside the record it produced so the second revision sees the same inputs.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    struct Recorded {
        operation_id: String,
        envelope: crate::ActionEnvelope,
        verdict: crate::Verdict,
        reason: String,
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], checked: &wire_check::Report, against: Option<&[Unit]>, decisions: &[PathBuf], dir: &Path) {
        use crate::{preflight, ActionEnvelope, ActionEvaluator, Execution, PreflightRefusal, ReferenceEvaluator, Verdict};
        let Some(against) = against else {
            rep.state = ClassState::NotConfigured { missing: "--against <units-dir> (the second revision)".into() };
            let why = class_reason(&rep.state);
            not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
            return;
        };
        let build = |units: &[Unit], revision: &str| -> Result<Arc<dyn ActionEvaluator>, String> {
            let mut ev = ReferenceEvaluator::new(revision).with_catalogue("declaration", revision);
            for u in units {
                // An exposure rule (Y5) is not an allowance: the evaluator never loads it.
                for r in u.config.rules.iter().filter(|r| !r.is_exposure()) {
                    let rule = r.to_rule().map_err(|e| format!("unit {}: [[rule]]: {e}", u.name))?;
                    ev = ev.map_action(rule.operation.clone(), "*").allow(rule);
                }
            }
            Ok(Arc::new(ev))
        };
        let (eval_a, eval_b) = match (build(units, "declared"), build(against, "proposed")) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => { rep.state = ClassState::Refused { reason: e }; return; }
        };
        let mut fleet = match Fleet::plan(base, units, dir, units.len().max(3)) {
            Ok(f) => f,
            Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
        };
        // The journal's writer is spawned where it is opened, so it is opened on the node's runtime, before start().
        let journal_path = dir.join("evidence.log");
        let opened: Opened = Arc::new(std::sync::Mutex::new(None));
        let (ev, slot, path) = (Arc::clone(&eval_a), Arc::clone(&opened), journal_path.clone());
        if let Err(reason) = fleet.start_with(0, move |a| {
            // The journal before the evaluator (#680's review, finding 6: the other order warns of unrecorded decisions).
            let j = crate::EvidenceJournal::open(&path, crate::EvidenceProfile::Strict).map_err(|e| format!("opening {}: {e}", path.display()));
            if let Ok(j) = &j {
                a.with_evidence_journal(Arc::clone(j));
            }
            a.with_action_evaluator(ev);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(j);
        }) {
            rep.state = ClassState::Refused { reason };
            return;
        }
        let journal = match opened.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(j)) => j,
            Some(Err(reason)) => { rep.state = ClassState::Refused { reason }; return; }
            None => { rep.state = ClassState::Refused { reason: "the journal was not opened".into() }; return; }
        };
        exercised(rep, fleet.guarantees().as_ref(), GUARANTEES, RULES);
        let via = fleet.nodes[0].id.clone();

        // The call set: every call a [[rule]] names (as written, and without the values it requires) and every
        // wired edge's two call shapes, from each requirer's principal.
        let mut calls: Vec<(String, String, String, serde_json::Map<String, serde_json::Value>)> = Vec::new();
        let principals: Vec<String> = units.iter().filter_map(|u| u.config.principal.clone()).collect();
        for u in units {
            for r in u.config.rules.iter().filter(|r| !r.is_exposure()) {
                let actors: Vec<String> = if r.actor == "*" { principals.iter().cloned().chain(std::iter::once("stranger".to_string())).collect() } else { vec![r.actor.clone()] };
                let (op, res) = (r.operation.clone(), if r.resource == "*" { "resource:any".to_string() } else { r.resource.clone() });
                let values: serde_json::Map<String, serde_json::Value> = r.requires_values.iter().filter_map(|(k, v)| serde_json::to_value(v).ok().map(|j| (k.clone(), j))).collect();
                for actor in actors {
                    if !values.is_empty() {
                        calls.push((actor.clone(), op.clone(), res.clone(), values.clone()));
                    }
                    calls.push((actor, op.clone(), res.clone(), serde_json::Map::new()));
                }
            }
        }
        for e in &checked.edges {
            let actor = e.principal.clone().unwrap_or_else(|| e.requirer.clone());
            for shape in &e.operations {
                calls.push((actor.clone(), shape.operation.clone(), shape.resource.clone(), serde_json::Map::new()));
            }
        }
        calls.sort_by(|a, b| (&a.0, &a.1, &a.2, serde_json::Value::Object(a.3.clone()).to_string()).cmp(&(&b.0, &b.1, &b.2, serde_json::Value::Object(b.3.clone()).to_string())));
        calls.dedup();

        // Decided at the seam, recorded before anything could dispatch (nothing dispatches here). Every record
        // carries one correlation id, so the run reads back as one chain: `mycelium explain --correlation <id>
        // <journal>` over the kept journal (`--keep`).
        let t = crate::sim_seam::mono_instant();
        let now = crate::sim_seam::wall_now_ms();
        let correlation = format!("test-faults-{now}");
        let mut recorded: Vec<Recorded> = Vec::new();
        let mut appended = 0usize;
        for (i, (actor, op, res, values)) in calls.iter().enumerate() {
            let operation_id = format!("test-faults-policy-{i}");
            let env: ActionEnvelope = ActionEnvelope::builder(actor.clone(), via.clone(), op.clone(), res.clone())
                .mapping(eval_a.mapping(op, res))
                .validity(now, now + 60_000)
                .identities(operation_id.clone(), format!("{operation_id}-1"))
                .selected_arguments(values.clone())
                .causal(crate::causal::CausalFields { correlation: Some(correlation.clone()), parent: None })
                .build();
            let decision = match preflight(Some(&eval_a), &env, now) {
                Ok(Some(d)) => d,
                Ok(None) => continue,
                Err(PreflightRefusal::Denied(d)) | Err(PreflightRefusal::NotEstablished(d)) | Err(PreflightRefusal::NotRecorded(d)) => d,
                Err(other) => { rep.notes.push(format!("{operation_id}: {other:?}")); continue; }
            };
            let record = crate::AeEvidence::for_decision(&env, &decision, Execution::None, "test-faults");
            let bytes = serde_json::to_vec(&record).unwrap_or_default();
            let j = Arc::clone(&journal);
            if fleet.on(0, async move { j.append(bytes).await }).is_ok() {
                appended += 1;
            }
            recorded.push(Recorded { operation_id, envelope: env, verdict: decision.verdict, reason: decision.reason });
        }
        check(rep, &t, "every call in the declaration's set is decided at the seam and its decision recorded", Some(appended == recorded.len() && !recorded.is_empty()), format!("{} call(s), {appended} record(s) appended to {}", recorded.len(), journal_path.display()));

        // The diff: the journal's records, each joined to its call, decided again under the proposed revision.
        let t = crate::sim_seam::mono_instant();
        fleet.stop(0);
        let frames = crate::read_evidence_journal(&journal_path).unwrap_or_default();
        let (mut permitted_now, mut refused_now, mut unchanged, mut unattributable) = (Vec::new(), 0usize, 0usize, 0usize);
        for frame in &frames {
            let Ok(record) = serde_json::from_slice::<crate::AeEvidence>(frame) else { unattributable += 1; continue; };
            let Some(call) = recorded.iter().find(|r| r.operation_id == record.operation_id) else { unattributable += 1; continue; };
            let env_b = ActionEnvelope::builder(call.envelope.actor.clone(), via.clone(), call.envelope.operation.clone(), call.envelope.resource.clone())
                .mapping(eval_b.mapping(&call.envelope.operation, &call.envelope.resource))
                .validity(now, now + 60_000)
                .identities(call.operation_id.clone(), format!("{}-2", call.operation_id))
                .selected_arguments(call.envelope.selected_arguments.clone())
                .build();
            let verdict_b = match preflight(Some(&eval_b), &env_b, now) {
                Ok(Some(d)) => d.verdict,
                Ok(None) => Verdict::Indeterminate,
                Err(PreflightRefusal::Denied(d)) | Err(PreflightRefusal::NotEstablished(d)) | Err(PreflightRefusal::NotRecorded(d)) => d.verdict,
                Err(_) => Verdict::Indeterminate,
            };
            match (call.verdict, verdict_b) {
                (Verdict::Permit, Verdict::Permit) => unchanged += 1,
                (Verdict::Permit, _) => refused_now += 1,
                (_, Verdict::Permit) => permitted_now.push(format!(
                    "{} {} {}{} — declared: {:?} ({}); proposed: permit",
                    call.envelope.actor,
                    call.envelope.operation,
                    call.envelope.resource,
                    if call.envelope.selected_arguments.is_empty() { String::new() } else { format!(" {}", serde_json::Value::Object(call.envelope.selected_arguments.clone())) },
                    call.verdict,
                    call.reason
                )),
                _ => unchanged += 1,
            }
        }
        // Records written elsewhere (`--decisions`): decided records re-decided from what they carry — subject,
        // operation, resource — and no argument values. A record a proposed rule reads values for is
        // *unattributable*: the record does not carry what the rule reads, and is never reported as unchanged.
        let needs_values = |actor: &str, op: &str, res: &str| {
            against.iter().flat_map(|u| u.config.rules.iter()).any(|r| {
                let any = |pat: &str, v: &str| pat == "*" || pat == v;
                !r.requires_values.is_empty() && any(&r.actor, actor) && any(&r.operation, op) && any(&r.resource, res)
            })
        };
        let mut external = 0usize;
        for path in decisions {
            let frames = match crate::read_evidence_journal(path) {
                Ok(f) => f,
                Err(e) => { rep.notes.push(format!("--decisions {}: {e}", path.display())); continue; }
            };
            for frame in &frames {
                let Ok(record) = serde_json::from_slice::<crate::AeEvidence>(frame) else { unattributable += 1; continue; };
                if record.kind != crate::RecordKind::Decided {
                    continue;
                }
                external += 1;
                let resource = record.resource.split('@').next().unwrap_or(&record.resource).to_string();
                if needs_values(&record.subject, &record.operation, &resource) {
                    unattributable += 1;
                    continue;
                }
                let env_b = ActionEnvelope::builder(record.subject.clone(), via.clone(), record.operation.clone(), resource.clone())
                    .mapping(eval_b.mapping(&record.operation, &resource))
                    .validity(now, now + 60_000)
                    .identities(record.operation_id.clone(), format!("{}-diff", record.attempt_id))
                    .build();
                let verdict_b = match preflight(Some(&eval_b), &env_b, now) {
                    Ok(Some(d)) => d.verdict,
                    Ok(None) => Verdict::Indeterminate,
                    Err(PreflightRefusal::Denied(d)) | Err(PreflightRefusal::NotEstablished(d)) | Err(PreflightRefusal::NotRecorded(d)) => d.verdict,
                    Err(_) => Verdict::Indeterminate,
                };
                match (record.decision, verdict_b) {
                    (crate::DecisionKind::Permit, Verdict::Permit) => unchanged += 1,
                    (crate::DecisionKind::Permit, _) => refused_now += 1,
                    (_, Verdict::Permit) => permitted_now.push(format!("{} {} {} — recorded ({}): {:?}; proposed: permit", record.subject, record.operation, resource, path.display(), record.decision)),
                    _ => unchanged += 1,
                }
            }
        }
        let regression = !permitted_now.is_empty();
        check(
            rep,
            &t,
            "no call the declared revision refused is permitted by the proposed one",
            Some(!regression),
            if regression {
                format!("{} call(s) the proposed revision permits and the declared one refused: {}", permitted_now.len(), permitted_now.join(" | "))
            } else {
                format!("{} record(s) compared ({external} from --decisions): {unchanged} unchanged, {refused_now} refused now, 0 permitted now, {unattributable} unattributable", frames.len() + external)
            },
        );
        if unattributable > 0 {
            rep.notes.push(format!("{unattributable} record(s) do not carry the inputs a proposed rule reads (argument values) or could not be joined to a call, and are reported unattributable, never as unchanged"));
        }
        rep.notes.push(format!("correlation {correlation}: every record of this run carries it; with --keep the journal is {} and `mycelium explain --correlation {correlation} <journal>` renders the run as one chain", journal_path.display()));
        rep.notes.push("the decisions are this command's own enforcement point's, over the declaration's call set, under evaluators built from each revision's [[rule]]s; a rule's resource matches the call's resource as written (no provider suffix)".into());
    }
}

// ────────────────────────────────────────────────────────────────────────────────────── replay ──

mod replay {
    use super::*;

    const GUARANTEES: &[&str] = &[];
    const RULES: &[&str] = &["membership.governed", "kv.propagation"];

    #[cfg(not(feature = "sim"))]
    pub(super) fn run(rep: &mut ClassReport, _base: &GossipConfig, _units: &[Unit], _dir: &Path, _seed: u64) {
        rep.state = ClassState::NotInBuild { feature: "sim" };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(_base).as_ref(), GUARANTEES, RULES, &why);
        rep.notes.push("the seams are off in every shipped build; build the binary with `cargo build --bin mycelium --features cli,sim` to record and replay".into());
    }

    #[cfg(feature = "sim")]
    pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path, seed: u64) {
        use mycelium_sim::Kernel;
        // One node per unit, each recorded and replayed **alone**: frame arrival between nodes is outside the
        // seams (`docs/design/replay-nondeterminism-inventory.md` §3.1.1), so a fleet recorded together on one
        // kernel diverges at its first cross-node channel choice — seen against the scaffold's three nodes at seq 14,
        // `writer/forward` to one peer recorded, to the other replayed. A node alone is the shape zero-gaps Z5
        // proved (`tests/decision_trace_replay.rs`), and what this class holds each of the declaration's nodes to.
        let count = units.len().max(1);
        let ports = match free_ports(count) {
            Ok(p) => p,
            Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            rep.state = ClassState::Refused { reason: format!("{}: {e}", dir.display()) };
            return;
        }
        exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES);
        for (i, port) in ports.iter().enumerate() {
            let name = units.get(i).map(|u| u.name.as_str()).unwrap_or("node");
            let once = Once { base, unit: units.get(i), dir, index: i, port: *port, seed };
            let t = crate::sim_seam::mono_instant();
            let recorded = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_once(&once, Kernel::recording(), false))) {
                Ok(Ok(r)) => r,
                Ok(Err(reason)) => { check(rep, &t, &format!("unit `{name}`: its node's run is recorded under the kernel"), Some(false), reason); continue; }
                Err(p) => {
                    let _ = crate::sim_seam::take();
                    check(rep, &t, &format!("unit `{name}`: its node's run is recorded under the kernel"), Some(false), format!("the recording panicked: {}", panic_text(&p)));
                    continue;
                }
            };
            check(rep, &t, &format!("unit `{name}`: its node's run is recorded under the kernel"), Some(!recorded.trace.is_empty()), format!("{} choice(s), {} decision(s)", recorded.trace.len(), recorded.decisions.len()));

            let t = crate::sim_seam::mono_instant();
            let replayed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_once(&once, Kernel::replaying(recorded.trace.clone()), true)));
            let name_check = format!("unit `{name}`: the replay consumes the whole recording and decides decision for decision");
            match replayed {
                Ok(Ok(r)) => {
                    let cmp = crate::decision::compare(&recorded.decisions, &r.decisions);
                    // Decision for decision: the kernel refused no request and the decisions reproduce. A recording
                    // may end a choice or two after the replay's run did — a choice the recorded run made while
                    // shutting down, on a wall-time race the paused clock does not have; the kernel would have
                    // refused a *swapped* request, so an unmade tail is not a path the replay took differently. It
                    // is named, with the first choice not reached.
                    let ok = cmp.reproduces();
                    let tail = if r.exhausted {
                        String::new()
                    } else {
                        let first = recorded.trace.entries().get(r.position).map(|c| c.to_line()).unwrap_or_default();
                        format!("; {} recorded choice(s) at the end not reached by the replay's run (shutdown), the first: `{first}`", recorded.trace.len().saturating_sub(r.position))
                    };
                    check(rep, &t, &name_check, Some(ok), format!(
                        "{} of {} choice(s) consumed; {} decision(s) compared, {} divergence(s), {} unattributable{tail}",
                        r.position, recorded.trace.len(), cmp.compared, cmp.divergences.len(), cmp.unattributable
                    ));
                    if let Some(d) = cmp.divergences.first() {
                        rep.notes.push(format!("unit `{name}`: first decision divergence at {}: recorded {:?}, replayed {:?}{}", d.index, d.left, d.right, if d.attributable { "" } else { " (unattributable: a gossiped input)" }));
                    }
                }
                Ok(Err(reason)) => check(rep, &t, &name_check, Some(false), reason),
                Err(p) => {
                    // The seam panicked where the replay departed; the kernel it left installed on this thread is taken
                    // down before anything else reads a clock.
                    let _ = crate::sim_seam::take();
                    check(rep, &t, &name_check, Some(false), format!("first divergence: {}", panic_text(&p)));
                }
            }
        }
        rep.notes.push(format!("seed {seed}; each node recorded and replayed alone on a current-thread runtime under one kernel, as the node binary records (`GOSSIP_RECORD_BUNDLE_DIR`); the schedule between nodes is outside the seams (inventory §3.1.1), so a fleet recorded together diverges at its first cross-node channel choice"));
    }

    #[cfg(feature = "sim")]
    struct Run {
        trace: mycelium_sim::Trace,
        decisions: Vec<crate::decision::DecisionRecord>,
        exhausted: bool,
        position: usize,
    }

    #[cfg(feature = "sim")]
    fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
        p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "a panic without a message".into())
    }

    /// One node's run, as both the recording and the replay take it.
    #[cfg(feature = "sim")]
    struct Once<'a> {
        base: &'a GossipConfig,
        unit: Option<&'a Unit>,
        dir: &'a Path,
        index: usize,
        port: u16,
        seed: u64,
    }

    /// One run of one node under the kernel: start, apply its unit, let the governor tick, one write, shut down.
    /// The same sequence records and replays.
    #[cfg(feature = "sim")]
    fn run_once(once: &Once<'_>, kernel: mycelium_sim::Kernel, replaying: bool) -> Result<Run, String> {
        use crate::sim_seam::{install, take, SimContext};
        let Once { base, unit, dir, index, port, seed } = *once;
        let wall_ms = crate::sim_seam::wall_now_ms();
        install(SimContext { kernel, sources: mycelium_sim::Sources::seeded(seed, wall_ms), node: format!("node-{index}"), offsets: Default::default() });
        let sink = Arc::new(crate::decision::DecisionSink::default());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let outcome: Result<(), String> = rt.block_on(async {
            // The scheduler seam: a replayed wait is taken on tokio's paused clock, which is paused from inside the
            // runtime (zero-gaps Z5, `tests/decision_trace_replay.rs`).
            if replaying {
                crate::sim_seam::pause_clock_for_replay();
            }
            let id = NodeId::new("127.0.0.1", port).map_err(|e| e.to_string())?;
            let mut cfg = fleet_config(base, dir, 0, std::slice::from_ref(&id));
            // One directory per run: a replay must not find the recording's files.
            if let Some(p) = cfg.persistence.as_mut() {
                p.base_path = dir.join(if replaying { "persist-replay" } else { "persist-record" }).join(format!("node-{index}"));
            }
            let agent = Arc::new(GossipAgent::new(id, cfg));
            agent.with_decision_trace(Arc::clone(&sink));
            agent.start().await.map_err(|e| e.to_string())?;
            let mut held: Vec<Box<dyn std::any::Any>> = Vec::new();
            if let Some(unit) = unit {
                for c in &unit.config.capabilities {
                    held.push(Box::new(agent.capabilities().advertise_capability(c.build_capability_public(), Duration::from_secs(c.ttl_secs.max(1)))));
                }
                for g in &unit.config.groups {
                    if let Ok(def) = g.to_def() {
                        held.push(Box::new(agent.capabilities().define_capability_group(g.name.clone(), def, Duration::from_secs(30))));
                        let _ = agent.publish_membership_intent(crate::MembershipIntent::new(g.name.clone(), 1, None));
                    }
                }
            }
            agent.start_membership_governor();
            let _ = agent.kv().set(format!("faults/replay/{index}"), b"recorded".to_vec());
            // Two governor periods, through the timer seam, so the recording holds its ticks and its rolls.
            crate::sim_seam::sleep_ms("faults/settle", 2_500).await;
            drop(held);
            agent.shutdown().await;
            if replaying {
                crate::sim_seam::resume_clock_after_replay();
            }
            Ok(())
        });
        drop(rt);
        let ctx = take().ok_or_else(|| "no kernel context after the run".to_string())?;
        outcome?;
        Ok(Run { trace: ctx.kernel.trace().clone(), decisions: sink.drain(), exhausted: ctx.kernel.is_exhausted(), position: ctx.kernel.position() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, toml: &str) -> Unit {
        Unit { name: name.into(), config: crate::NodeCapabilityConfig::from_toml_str(toml).expect("a unit file") }
    }

    /// A declaration `wire-check` refuses is refused by `test` with the same findings, before any node is built.
    #[test]
    fn a_declaration_wire_check_refuses_is_refused_with_the_same_findings() {
        let units = vec![unit("b", "principal = \"b\"\n[[requirement]]\nns = \"nobody\"\nname = \"offers\"\n")];
        let direct = wire_check::check(&units, &[], &CheckOptions::default());
        assert_eq!(direct.exit_code(), 1, "the plant: wire-check finds an error");
        match run(&units, &[], &CheckOptions::default(), &Options::default()) {
            Err(Refused::WireCheck(r)) => assert_eq!(r.render_text(), direct.render_text(), "the same findings, the same words"),
            other => panic!("expected a wire-check refusal, got {other:?}"),
        }
    }

    /// A unit's `[[activation]]` runs no command: the plant's command would write a file, and the file is not
    /// written; the report says `not_applicable` and counts the command.
    #[test]
    fn an_activation_runs_no_command() {
        let marker = std::env::temp_dir().join(format!("mycelium-test-faults-activation-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let toml = format!(
            "principal = \"p\"\n[[capability]]\nns = \"demo\"\nname = \"thing\"\nttl_secs = 5\n[hosts]\nkinds = [\"blob\"]\ntrusted_publishers = [\"ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12\"]\nplacement_root = \"/var/lib/mycelium/artifacts\"\n[[activation]]\nns = \"demo\"\nname = \"thing\"\ncommand = [\"sh\", \"-c\", \"touch {}\"]\n",
            marker.display()
        );
        let units = vec![unit("p", &toml)];
        // The restart class alone, which builds and starts the fleet; the configuration has no persistence,
        // so the class reports not_configured — the fleet-building path is still the one an activation
        // would have to run through.
        let opts = Options { classes: vec![Class::Partition], ..Default::default() };
        let report = run(&units, &[], &CheckOptions::default(), &opts).expect("the run");
        assert_eq!(report.activations.declared, 1);
        assert_eq!(report.activations.run, 0);
        assert_eq!(report.activations.state, "not_applicable");
        assert!(!marker.exists(), "ACTIVATION RAN: {} exists", marker.display());
        assert!(report.render().contains("activation: not_applicable — 1 command(s) declared, 0 run"), "{}", report.render());
    }

    /// The restart class on a configuration without `[persistence]` says `not_configured`, names the table, and
    /// fails the run only when asked for by name.
    #[test]
    fn a_class_that_cannot_run_fails_only_when_asked_by_name() {
        let units = vec![unit("p", "principal = \"p\"\n")];
        let mk = |classes: Vec<Class>| ClassReport { class: Class::Restart, asked: classes.contains(&Class::Restart), state: ClassState::NotConfigured { missing: "[persistence]".into() }, checks: vec![], exercised: vec![], not_exercised: vec![], notes: vec![], elapsed_ms: 0 };
        let report = |asked: Vec<Class>| Report {
            schema: SCHEMA,
            config_path: None,
            fleet: FleetFacts { nodes: 3, units: units.iter().map(|u| u.name.clone()).collect(), tls: false, persistence: false, profile: "dev", profile_declared: None, gateway_started: false },
            wire_check: WireCheckSummary { errors: 0, warnings: 0 },
            activations: Activations { declared: 0, run: 0, state: "not_applicable", because: "" },
            classes: vec![mk(asked)],
            kept: None,
        };
        assert_eq!(report(vec![]).exit_code(), 0, "not asked: stated, not failed");
        assert_eq!(report(vec![Class::Restart]).exit_code(), 1, "asked by name: the run fails");
        // The real path: restart over the defaults, which have no persistence.
        let opts = Options { classes: vec![Class::Restart], ..Default::default() };
        let r = run(&units, &[], &CheckOptions::default(), &opts).expect("the run");
        assert_eq!(r.classes.len(), 1);
        assert_eq!(r.classes[0].state, ClassState::NotConfigured { missing: "[persistence]".into() });
        assert_eq!(r.exit_code(), 1);
        assert!(r.classes[0].not_exercised.iter().any(|e| e.id == "persist.configured"), "{:?}", r.classes[0].not_exercised);
    }

    /// `replay` without `sim`: `not_in_build` naming `sim`; the class is stated, the run is not failed unless asked.
    #[cfg(not(feature = "sim"))]
    #[test]
    fn replay_without_sim_is_not_in_build_naming_sim() {
        let units = vec![unit("p", "principal = \"p\"\n")];
        let r = run(&units, &[], &CheckOptions::default(), &Options { classes: vec![Class::Replay], ..Default::default() }).expect("the run");
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "sim" });
        assert_eq!(r.exit_code(), 1, "asked by name");
        let r = run(&units, &[], &CheckOptions::default(), &Options { classes: vec![Class::Revoke, Class::Replay], ..Default::default() }).expect("the run");
        assert!(r.render().contains("replay: not_in_build — sim"), "{}", r.render());
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mycelium-faults-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn persisted(dir: &Path) -> GossipConfig {
        use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
        let mut cfg = GossipConfig::default();
        cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
        cfg
    }

    fn run_class(class: Class, units: &[Unit], cfg: &GossipConfig, dir: &Path, against: Option<Vec<Unit>>) -> ClassReport {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("node.toml");
        std::fs::write(&path, toml::to_string(cfg).unwrap()).unwrap();
        let opts = Options { classes: vec![class], config: Some(path), against, seed: 7, ..Default::default() };
        let report = run(units, &[], &CheckOptions::default(), &opts).expect("the run");
        assert_eq!(report.classes.len(), 1, "{}", report.render());
        report.classes.into_iter().next().unwrap()
    }

    fn all_held(c: &ClassReport) -> bool {
        matches!(c.state, ClassState::Passed) && !c.checks.is_empty() && c.checks.iter().all(|k| k.ok == Some(true))
    }

    /// The restart class against a fleet with persistence: rung-2 receipts, the writes replayed after every node
    /// restarts over its own directory, and a WAL record that does not decode refused at start by name.
    #[test]
    fn the_restart_class_replays_a_synced_write_and_refuses_unreadable_state() {
        let dir = scratch("restart");
        let units = vec![unit("p", "principal = \"p\"\n[[capability]]\nns = \"demo\"\nname = \"thing\"\nttl_secs = 5\n")];
        let c = run_class(Class::Restart, &units, &persisted(&dir), &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert_eq!(c.checks.len(), 3, "{c:?}");
        assert!(c.checks[2].detail.contains("corrupt"), "the refusal names the corrupt record: {}", c.checks[2].detail);
        assert!(c.exercised.iter().any(|e| e.id == "persist.unreadable_refused" && e.state == "enforced"), "{:?}", c.exercised);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The revoke class against a TLS fleet: the three verifiers refuse the revoked key, and the declaration's
    /// mandate is established then not established once revoked.
    #[cfg(all(feature = "tls", feature = "compliance", feature = "gateway"))]
    #[test]
    fn the_revoke_class_sees_the_three_verifiers_and_the_mandate_refuse() {
        let dir = scratch("revoke");
        let mut cfg = GossipConfig::default();
        cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.join("tls"), ..crate::config::TlsConfig::default() });
        let units = vec![unit("purchaser", "principal = \"purchaser\"\n[[capability]]\nns = \"purchase\"\nname = \"place\"\nttl_secs = 5\n[[mandate]]\nholder = \"purchaser\"\nscope = \"petty-cash\"\noperations = [\"tools/call:tool:purchase.place\"]\n")];
        let c = run_class(Class::Revoke, &units, &cfg, &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert_eq!(c.checks.len(), 6, "{c:?}");
        assert!(c.checks[5].detail.contains("the appointment is revoked"), "{}", c.checks[5].detail);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The partition class against a fleet with a governed group: the dark member is evicted, its offer leaves
    /// the view after its evaporation window, the floor is re-filled by the eligible survivor, and the node comes
    /// back. Short `ttl_secs`, so the evaporation window is seconds.
    #[test]
    fn the_partition_class_sees_a_dark_member_evicted_and_the_floor_refilled() {
        let dir = scratch("partition");
        let worker = |name: &str| unit(name, &format!("principal = \"{name}\"\n[[capability]]\nns = \"svc\"\nname = \"worker\"\nttl_secs = 2\n"));
        let units = vec![
            unit("dispatcher", "principal = \"dispatcher\"\n[[group]]\nname = \"pool\"\n  [group.filter]\n  ns = \"svc\"\n  name = \"worker\"\n"),
            worker("worker-a"),
            worker("worker-b"),
        ];
        let c = run_class(Class::Partition, &units, &GossipConfig::default(), &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert!(c.checks.iter().any(|k| k.name.contains("re-filled by a survivor") && k.ok == Some(true)), "{c:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The recovery class against a persisted three-node fleet: the lost acknowledgement probed before any retry
    /// and the destination holding one effect, the closure incomplete over two journals and complete over three,
    /// every dependent recovered by its class as a new record, independent work untouched, and the recovery
    /// explained from the records. Seen failing first under a plant that re-sent the root effect instead of probing:
    /// the first check's detail read `SENT (…)` and `the destination reached 2 time(s) for it` (the destination
    /// deduplicates, so the applications stay at one — the attempts are the number a re-send shows). And under a
    /// plant that removed `retry`'s pre-check, the second check still held: the one retry is claimed under the
    /// ledger's lock, so the concurrent second retry is refused there (#677's review, findings 4 and 8).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn the_recovery_class_recovers_the_closure_by_class_and_leaves_independent_work() {
        let dir = scratch("recovery");
        let units = vec![
            unit("orders", "principal = \"orders\"\n[[capability]]\nns = \"shop\"\nname = \"orders\"\nttl_secs = 5\n"),
            unit("fulfilment", "principal = \"fulfilment\"\n[[capability]]\nns = \"shop\"\nname = \"labels\"\nttl_secs = 5\n"),
            unit("loyalty", "principal = \"loyalty\"\n[[capability]]\nns = \"shop\"\nname = \"points\"\nttl_secs = 5\n"),
        ];
        let c = run_class(Class::Recovery, &units, &persisted(&dir), &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert_eq!(c.checks.len(), 10, "{c:?}");
        assert!(c.checks[0].detail.contains("the synced write is back") && c.checks[0].detail.contains("probe before any retry") && c.checks[0].detail.contains("reached 1 time(s) for it"), "{}", c.checks[0].detail);
        assert!(c.checks[2].detail.contains("INCOMPLETE") && c.checks[2].detail.contains("hole(s) cited elsewhere"), "{}", c.checks[2].detail);
        assert!(c.checks[8].detail.contains("order-100 1/1 1/1") && c.checks[8].detail.contains("label-101 1/1 2/2") && c.checks[8].detail.contains("points-102 0/0 1/1"), "{}", c.checks[8].detail);
        assert!(c.checks[9].detail.contains("effect.probe, effect.retry, notify-retract, effect.leave"), "{}", c.checks[9].detail);
        assert!(c.exercised.iter().any(|e| e.id == "persist.configured" && e.state == "enforced"), "{:?}", c.exercised);
        assert!(c.exercised.iter().any(|e| e.id == "persist.sync_mode"), "{:?}", c.exercised);
        assert!(c.notes.iter().any(|n| n.contains("record_permit")), "{:?}", c.notes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The grant class against a declaration whose purchaser may call one tool only under the treasurer's grant, on
    /// a fleet with TLS and persistence (adopter-surface X3, fourth PR): default-deny, the declared issuer's grant and
    /// a stranger's refusal, the restart over the WAL, the binding to the arguments, the record. Seen failing first:
    /// with #678's exemption of a grant's checkpoint from the revocation view's start rule removed, check 3 reads the
    /// grant issued before the restart as not established after it.
    #[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
    #[test]
    fn the_grant_class_refuses_on_silence_issues_by_the_declared_rule_and_holds_across_a_restart() {
        let dir = scratch("grant");
        let mut cfg = persisted(&dir);
        cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.join("tls"), ..crate::config::TlsConfig::default() });
        let units = vec![unit("purchaser", concat!(
            "principal = \"purchaser\"\n[[capability]]\nns = \"purchase\"\nname = \"place\"\nttl_secs = 5\n",
            "[[rule]]\nactor = \"purchaser\"\noperation = \"tools/call\"\nresource = \"tool:purchase.approve\"\nrequires_grant = true\n",
            "[[rule]]\nactor = \"treasurer\"\noperation = \"grant.issue\"\nresource = \"tool:purchase.approve\"\n",
        ))];
        let c = run_class(Class::Grant, &units, &cfg, &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert_eq!(c.checks.len(), 5, "{c:?}");
        assert!(c.checks[0].name.starts_with("a grant-required tool is default-deny"), "{}", c.checks[0].name);
        assert!(c.checks[1].detail.contains("test-faults-stranger's issue refused by the evaluator") && c.checks[1].detail.contains("treasurer's issue: written"), "{}", c.checks[1].detail);
        assert!(c.checks[2].detail.contains("back (the exit-snapshot path)") && c.checks[2].detail.contains("did not ask twice"), "{}", c.checks[2].detail);
        assert!(c.checks[4].detail.contains("2 permitted for treasurer, 1 refused for test-faults-stranger"), "{}", c.checks[4].detail);
        assert!(c.exercised.iter().any(|e| e.id == "mesh.tls" && e.state == "enforced"), "{:?}", c.exercised);
        assert!(c.exercised.iter().any(|e| e.id == "persist.configured" && e.state == "enforced"), "{:?}", c.exercised);
        assert!(c.notes.iter().any(|n| n.contains("decide_grant") && n.contains("treasurer")), "{:?}", c.notes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A declaration that admits anyone (`*`) to issue, through a `*` operation: the stranger's half is stated, not
    /// run, and the class passes (#680's review, finding 1 — the stranger's grant was written and the issuer's
    /// refused `already_answered`; seen failing first).
    #[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
    #[test]
    fn the_grant_class_states_an_issuer_rule_that_admits_anyone() {
        let dir = scratch("grant-anyone");
        let mut cfg = persisted(&dir);
        cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.join("tls"), ..crate::config::TlsConfig::default() });
        let units = vec![unit("purchaser", concat!(
            "principal = \"purchaser\"\n",
            "[[rule]]\nactor = \"purchaser\"\noperation = \"tools/call\"\nresource = \"tool:purchase.approve\"\nrequires_grant = true\n",
            "[[rule]]\nactor = \"*\"\noperation = \"*\"\nresource = \"*\"\n",
        ))];
        let c = run_class(Class::Grant, &units, &cfg, &dir, None);
        assert!(all_held(&c), "{c:?}");
        assert!(c.checks[1].detail.contains("admits anyone (`*`) to grant.issue") && c.checks[1].detail.contains("test-faults-supervisor's issue: written"), "{}", c.checks[1].detail);
        assert!(c.checks[4].detail.contains("2 permitted for test-faults-supervisor") && c.checks[4].detail.contains("anyone may issue here"), "{}", c.checks[4].detail);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `grant` without a grant-required rule is `not_applicable`; without an issuer rule too; without `[tls]` or
    /// `[persistence]` it is `not_configured` naming the table; a build without the features names the one it lacks.
    #[test]
    fn the_grant_class_needs_a_grant_rule_tls_and_persistence_and_says_so() {
        let plain = vec![unit("p", "principal = \"p\"\n")];
        let r = run(&plain, &[], &CheckOptions::default(), &Options { classes: vec![Class::Grant], ..Default::default() }).expect("the run");
        assert_eq!(r.classes.len(), 1);
        assert_eq!(r.exit_code(), 1, "asked by name");
        assert!(r.classes[0].not_exercised.iter().any(|e| e.id == "mesh.tls"), "{:?}", r.classes[0].not_exercised);
        #[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
        {
            assert_eq!(r.classes[0].state, ClassState::NotApplicable { because: "no [[rule]] has requires_grant = true: nothing in the declaration waits for a grant".into() });
            let unissuable = vec![unit("p", "principal = \"p\"\n[[rule]]\nactor = \"p\"\noperation = \"tools/call\"\nresource = \"tool:x\"\nrequires_grant = true\n")];
            let r = run(&unissuable, &[], &CheckOptions::default(), &Options { classes: vec![Class::Grant], ..Default::default() }).expect("the run");
            assert!(matches!(&r.classes[0].state, ClassState::NotApplicable { because } if because.contains("no [[rule]] admits anyone to grant.issue on tool:x")), "{:?}", r.classes[0].state);
            let issuable = vec![unit("p", "principal = \"p\"\n[[rule]]\nactor = \"p\"\noperation = \"tools/call\"\nresource = \"tool:x\"\nrequires_grant = true\n[[rule]]\nactor = \"q\"\noperation = \"grant.issue\"\nresource = \"*\"\n")];
            let r = run(&issuable, &[], &CheckOptions::default(), &Options { classes: vec![Class::Grant], ..Default::default() }).expect("the run");
            assert!(matches!(&r.classes[0].state, ClassState::NotConfigured { missing } if missing.starts_with("[tls]")), "{:?}", r.classes[0].state);
            let dir = scratch("grant-no-persist");
            let mut cfg = GossipConfig::default();
            cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.join("tls"), ..crate::config::TlsConfig::default() });
            let c = run_class(Class::Grant, &issuable, &cfg, &dir, None);
            assert_eq!(c.state, ClassState::NotConfigured { missing: "[persistence]".into() }, "{c:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        #[cfg(all(feature = "gateway", feature = "tls", not(feature = "compliance")))]
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "compliance" });
        #[cfg(all(feature = "gateway", not(feature = "tls")))]
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "tls" });
        #[cfg(not(feature = "gateway"))]
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "gateway" });
    }

    /// The report says where the fleet's directories are and what its nodes' log lines are, once, and only what this
    /// run had (a walk-through of tutorial 7 found the appended `base_path` and `auto_cert_dir` unused, which is by
    /// design, and ~30 WARN/ERROR lines on stderr read as a failure, which they are not; #690's review, finding 5).
    #[test]
    fn the_report_says_where_the_fleet_lives_and_what_its_log_lines_are() {
        let units = vec![unit("p", "principal = \"p\"\n")];
        let mut r = run(&units, &[], &CheckOptions::default(), &Options { classes: vec![Class::Recovery], ..Default::default() }).expect("the run");
        // Nothing ran: no fleet, no line.
        assert!(!matches!(r.classes[0].state, ClassState::Passed | ClassState::Failed), "{:?}", r.classes[0].state);
        assert!(!r.render().contains("fleet: "), "{}", r.render());
        // A run with persistence and no TLS whose class stops nodes gracefully: no CA, no cut.
        r.fleet.persistence = true;
        r.fleet.tls = false;
        r.classes[0].class = Class::Restart;
        r.classes[0].state = ClassState::Passed;
        let text = r.render();
        assert!(text.contains("fleet: its directories are the run's own"), "{text}");
        assert!(text.contains("`[persistence]` turns them on, and its `base_path` is replaced"), "{text}");
        assert!(!text.contains("[tls]") && !text.contains("auto_cert_dir"), "no TLS in this run: {text}");
        assert!(text.contains("the nodes' log lines on stderr are the fleet's own"), "{text}");
        assert!(!text.contains("a CA minted for the run"), "{text}");
        assert!(!text.contains("listener errors"), "{text}");
        // TLS, and a class that cuts a node.
        r.fleet.tls = true;
        r.classes[0].class = Class::Recovery;
        let text = r.render();
        assert!(text.contains("`[persistence]` and `[tls]` turn them on, and their `base_path` and `auto_cert_dir` are replaced"), "{text}");
        assert!(text.contains("a CA minted for the run"), "{text}");
        assert!(text.contains("a cut node's listener errors"), "{text}");
    }

    /// The fleet runs under `dev`, selected: the CA-key warning, right for a real node, does not tell a `dev` user to
    /// set `profile = "dev"` (#690's review, finding 3).
    #[cfg(feature = "tls")]
    #[test]
    fn the_fleet_runs_under_dev_selected_so_no_ca_key_warning() {
        let dir = scratch("fleet-dev");
        let mut base = GossipConfig::default();
        base.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.join("adopter-tls"), ..crate::config::TlsConfig::default() });
        let ids = vec![NodeId::new("127.0.0.1", crate::test_util::alloc_port()).expect("id")];
        let cfg = fleet_config(&base, &dir, 0, &ids);
        let a = crate::GossipAgent::new(ids[0].clone(), cfg);
        assert!(crate::agent::guarantee::ca_key_on_node_warning(&a.guarantee_report()).is_none(), "the fleet's node warns of its own CA");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `recovery` without `[persistence]`: `not_configured`, naming the table; the ids it would drive listed as not
    /// exercised with that reason.
    #[test]
    fn the_recovery_class_needs_persistence_and_says_so() {
        let units = vec![unit("p", "principal = \"p\"\n")];
        let r = run(&units, &[], &CheckOptions::default(), &Options { classes: vec![Class::Recovery], ..Default::default() }).expect("the run");
        assert_eq!(r.classes.len(), 1);
        #[cfg(all(feature = "gateway", feature = "tls"))]
        assert_eq!(r.classes[0].state, ClassState::NotConfigured { missing: "[persistence]".into() });
        #[cfg(all(feature = "gateway", not(feature = "tls")))]
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "tls" }, "the feature the build lacks, by name");
        #[cfg(not(feature = "gateway"))]
        assert_eq!(r.classes[0].state, ClassState::NotInBuild { feature: "gateway" });
        assert_eq!(r.exit_code(), 1, "asked by name");
        assert!(r.classes[0].not_exercised.iter().any(|e| e.id == "persist.configured"), "{:?}", r.classes[0].not_exercised);
    }

    /// A proposed revision that drops a rule's `requires_values` permits the call the declared one refused: the
    /// diff names it and the class fails. The same revision against itself passes.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn a_planted_policy_regression_is_reported_by_policy_diff() {
        let dir = scratch("policy-diff");
        let declared = "principal = \"purchaser\"\n[[capability]]\nns = \"purchase\"\nname = \"place\"\nttl_secs = 5\n[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"tool:purchase.place\"\nrequires_values = { ledger = \"petty-cash\" }\n";
        let widened = declared.replace("requires_values = { ledger = \"petty-cash\" }\n", "");
        let units = vec![unit("purchaser", declared)];
        let c = run_class(Class::PolicyDiff, &units, &GossipConfig::default(), &dir, Some(vec![unit("purchaser", &widened)]));
        assert_eq!(c.state, ClassState::Failed, "{c:?}");
        let diff = c.checks.iter().find(|k| k.name.starts_with("no call the declared revision refused")).expect("the diff check");
        assert_eq!(diff.ok, Some(false));
        assert!(diff.detail.contains("tools/call tool:purchase.place") && diff.detail.contains("proposed: permit"), "{}", diff.detail);
        let same = run_class(Class::PolicyDiff, &units, &GossipConfig::default(), &dir.join("same"), Some(vec![unit("purchaser", declared)]));
        assert!(all_held(&same), "{same:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Class names parse exactly and nothing else does; a list with a stranger in it is refused naming it; the
    /// partition this command does not run is answered by name.
    /// `--keep` leaves a class's directory — its persistence and journals — in place and names it (#681's review,
    /// finding 1: the fleet's own `Drop` removed the class directory whatever `--keep` said, so a kept run held
    /// nothing; seen failing first).
    #[test]
    fn keep_leaves_the_class_directory_in_place() {
        let dir = scratch("keep");
        let units = vec![unit("p", "principal = \"p\"\n")];
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.toml");
        std::fs::write(&path, toml::to_string(&persisted(&dir)).unwrap()).unwrap();
        let opts = Options { classes: vec![Class::Restart], config: Some(path), keep: true, ..Default::default() };
        let report = run(&units, &[], &CheckOptions::default(), &opts).expect("the run");
        let kept = report.kept.clone().expect("--keep names the run's directory");
        let persist = kept.join("restart").join("persist");
        let held = std::fs::read_dir(&persist).map(|d| d.count()).unwrap_or(0);
        assert!(held > 0, "the kept run holds the restart class's persistence under {}: {}", persist.display(), report.render());
        let _ = std::fs::remove_dir_all(&kept);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn class_names_round_trip() {
        for c in Class::ALL {
            assert_eq!(Class::parse(c.name()), Some(c));
        }
        assert_eq!(Class::parse("replays"), None);
        assert_eq!(Class::parse(""), None);
        assert_eq!(Class::parse_list("revoke,restart"), Ok(Some(vec![Class::Revoke, Class::Restart])));
        assert_eq!(Class::parse_list("units"), Ok(None), "not a class list: a directory");
        assert!(Class::parse_list("revoke,bogus").unwrap_err().contains("`bogus` is not a class"));
        assert!(Class::parse_list("two-sided").unwrap_err().contains("Docker suites"));
    }

    /// A configuration under the secure profile is tested under `dev`, the declared profile reported and not
    /// applied: the fleet has no gateway and no code attachment, so the profile's own refusal would say nothing
    /// about the fleet (#675's review, finding 4).
    #[test]
    fn the_declared_profile_is_reported_and_not_applied() {
        let dir = scratch("profile");
        let mut cfg = persisted(&dir);
        cfg.profile = Some("secure-single-domain".into());
        let units = vec![unit("p", "principal = \"p\"\n")];
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.toml");
        std::fs::write(&path, toml::to_string(&cfg).unwrap()).unwrap();
        let report = run(&units, &[], &CheckOptions::default(), &Options { classes: vec![Class::Restart], config: Some(path), ..Default::default() }).expect("the run");
        assert_eq!(report.fleet.profile_declared.as_deref(), Some("secure-single-domain"));
        assert_eq!(report.fleet.profile, "dev");
        assert!(all_held(&report.classes[0]), "{}", report.render());
        assert!(report.render().contains("declared `secure-single-domain`, not applied"), "{}", report.render());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A journal written elsewhere, re-decided under the proposed revision: a record a proposed rule reads values
    /// for is unattributable (the record carries none); one no rule reads values for is decided from what it
    /// carries, and a permit the recorded decision refused is reported.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn a_decisions_file_without_inputs_is_unattributable_where_a_rule_needs_them() {
        use crate::{ActionEnvelope, Decision, Execution, AeEvidence};
        let dir = scratch("decisions");
        std::fs::create_dir_all(&dir).unwrap();
        let via = NodeId::new("127.0.0.1", 7000).unwrap();
        let rec = |actor: &str, res: &str, decision: Decision| {
            let env = ActionEnvelope::builder(actor, via.clone(), "tools/call", res).identities(format!("ext-{actor}-{res}"), "a1").build();
            serde_json::to_vec(&AeEvidence::for_decision(&env, &decision, Execution::None, "elsewhere")).unwrap()
        };
        let frames = [
            rec("purchaser", "tool:purchase.place@127.0.0.1:7000", Decision::deny("refused there", "r0")),
            rec("stranger", "tool:other.thing@127.0.0.1:7000", Decision::deny("refused there", "r0")),
        ];
        let journal = dir.join("elsewhere.log");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let j = crate::EvidenceJournal::open(&journal, crate::EvidenceProfile::Strict).unwrap();
            for f in &frames { j.append(f.clone()).await.unwrap(); }
        });
        drop(rt);
        let declared = "principal = \"purchaser\"\n[[capability]]\nns = \"purchase\"\nname = \"place\"\nttl_secs = 5\n[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"tool:purchase.place\"\nrequires_values = { ledger = \"petty-cash\" }\n";
        // The proposed revision keeps the valued rule and adds a bare allowance for the other tool: the first record
        // is unattributable (values the record lacks), the second is permitted now where it was refused.
        let proposed = format!("{declared}[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"tool:other.thing\"\n");
        let units = vec![unit("purchaser", declared)];
        std::fs::write(dir.join("node.toml"), toml::to_string(&GossipConfig::default()).unwrap()).unwrap();
        let opts = Options { classes: vec![Class::PolicyDiff], config: Some(dir.join("node.toml")), against: Some(vec![unit("purchaser", &proposed)]), decisions: vec![journal], ..Default::default() };
        let report = run(&units, &[], &CheckOptions::default(), &opts).expect("the run");
        let c = &report.classes[0];
        let diff = c.checks.iter().find(|k| k.name.starts_with("no call the declared revision refused")).expect("the diff check");
        assert_eq!(diff.ok, Some(false), "{}", report.render());
        assert!(diff.detail.contains("stranger tools/call tool:other.thing") && diff.detail.contains("proposed: permit"), "{}", diff.detail);
        assert!(c.notes.iter().any(|n| n.starts_with("1 record(s) do not carry the inputs")), "{:?}", c.notes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
