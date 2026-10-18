//! Declarative blob activation for a stem (design-time-tooling.md §16, X2): the unit file's
//! `[[activation]]` sections become [`BlobRuntime::with_entry_activation`] — after a blob that
//! provides `ns/name` is placed, run the declared command, and gate the capability on the declared
//! probe. The probe is re-run by a background task ([`spawn_reprobe`]) that flips a per-install
//! flag; the provisioner's probe only reads the flag, so a slow `ollama show` never runs under its
//! lock. A failed re-probe withdraws the install and the next round reinstalls — restart ≡
//! provisioning, exactly as for a wasm component whose serve task died.
//!
//! Unconfined by default: the commands are the operator's, written in a reviewed unit file and run
//! with the stem's own privileges — the same trust as the stem's command line. An activation whose
//! unit names an `[activation.confine]` profile runs its command **and** its probe under that profile,
//! or is refused by name and runs nothing (post-360 row M, [`crate::activation_sandbox`]). The profile
//! is resolved once, when the activation runs, and its re-probes reuse it — re-resolved, every check
//! repeated, when a declared path was replaced; only a refused re-resolution withdraws the install. A profile
//! naming `[activation.confine.egress]` runs confined to its egress proxy's port in a cleared environment, or —
//! its proxy not running — is refused by name (rows L and M, [`crate::activation_egress`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mycelium::ActivationDecl;

use crate::activation_sandbox::{run_confined, Profile};
use crate::catalog::InstallableEntry;
use crate::runtime::{ActivationCtx, TraceCtx};
use mycelium::decision::OutcomeKind;

/// One live activation the re-probe task keeps current. Lock-order table row 51: the list is a
/// leaf, taken by the activation (install task) to push and by the re-probe task to snapshot and
/// prune — never while any other lock is held, never across `await`.
#[derive(Clone)]
pub struct Watched {
    decl:   ActivationDecl,
    entry:  InstallableEntry,
    path:   PathBuf,
    health: Arc<AtomicBool>,
    /// The declaration's index and the hook's resolved profiles: each re-probe takes the profile from
    /// there, re-resolved — every check repeated — when a declared path was replaced (plan §3.6, M's
    /// follow-up), so a legitimate replacement does not withdraw the install and a stale profile never runs.
    index:    usize,
    resolved: Arc<ResolvedProfiles>,
    /// The activation's egress proxy, when its profile names one — every re-resolution confines to its port
    /// and its cleared environment (rows L and M).
    proxy:    Option<Arc<crate::EgressProxy>>,
    /// The install's token and trace, so a re-probe's verdict records under the same token.
    token:  u64,
    trace:  Option<Arc<TraceCtx>>,
}

/// The re-probe list, shared by the activation hook and the re-probe task.
pub type WatchList = Arc<Mutex<Vec<Watched>>>;

/// The most resolved confinement profiles one stem keeps (review follow-up 1, #636).
const RESOLVED_PROFILES_CAP: usize = 1024;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

fn hex_of(entry: &InstallableEntry) -> String {
    entry.artifact.to_hex()
}

fn expand(argv: &[String], entry: &InstallableEntry, path: &Path, rendered: Option<&Path>) -> Vec<String> {
    let dir = path.parent().map(|d| d.display().to_string()).unwrap_or_default();
    argv.iter()
        .map(|a| {
            let mut s = a
                .replace("{path}", &path.display().to_string())
                .replace("{dir}", &dir)
                .replace("{artifact}", &hex_of(entry))
                .replace("{ns}", &entry.provides.namespace)
                .replace("{name}", &entry.provides.name);
            if let Some(r) = rendered {
                s = s.replace("{rendered}", &r.display().to_string());
            }
            s
        })
        .collect()
}

/// Run `argv` unconfined, or under `profile` (row M).
fn run_under(argv: &[String], timeout: Duration, profile: Option<&Profile>) -> Result<(), String> {
    match profile {
        None => run(argv, timeout),
        Some(p) => run_confined(argv, timeout, p),
    }
}

/// The runner's message for a command killed at its deadline — matched by the hook to record
/// `command_timed_out`, so the two cannot drift apart again.
const DID_NOT_FINISH: &str = "did not finish within";

/// How much of a failing command's stderr its error keeps — the tail, where the reason usually is.
const STDERR_TAIL_BYTES: usize = 4096;

/// Run `argv` to completion within `timeout`; `Err` names the command and why.
///
/// Stderr is **drained while the command runs**, on its own thread, keeping only the last
/// [`STDERR_TAIL_BYTES`]: a command that writes more than a pipe buffer would otherwise block on
/// the write and never exit, and be reported as a timeout (360 review F3, 2026-10-02). On timeout
/// the command is killed and reaped. A descendant that inherited the pipe and outlives the command
/// cannot hold the result hostage either: the tail is awaited only briefly after exit.
pub fn run(argv: &[String], timeout: Duration) -> Result<(), String> {
    run_with_env(argv, timeout, None)
}

/// [`run`], with the command's **whole** environment given (`Some`: cleared, then exactly these — an
/// egress-proxied activation, rows L and M) or inherited from the stem (`None`).
pub fn run_with_env(argv: &[String], timeout: Duration, env: Option<&[(String, String)]>) -> Result<(), String> {
    let (cmd, args) = argv.split_first().ok_or("empty command")?;
    let mut command = std::process::Command::new(cmd);
    if let Some(env) = env {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    let mut child = command
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;
    let tail = child.stderr.take().map(|mut pipe| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut kept: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 { break; }
                kept.extend(&buf[..n]);
                let excess = kept.len().saturating_sub(STDERR_TAIL_BYTES);
                kept.drain(..excess);
            }
            let _ = tx.send(String::from_utf8_lossy(kept.make_contiguous()).into_owned());
        });
        rx
    });
    let read_tail = |rx: Option<std::sync::mpsc::Receiver<String>>| -> String {
        rx.and_then(|rx| rx.recv_timeout(Duration::from_secs(1)).ok()).unwrap_or_default()
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|e| format!("{cmd}: {e}"))? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                let err = read_tail(tail);
                return Err(format!("{cmd} exited {status}: {}", err.trim()));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{cmd} {DID_NOT_FINISH} {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Write `{path}.rendered`: every `artifact:<64 hex>` reference replaced by that artifact's placed
/// path in the same placement root. A reference not placed yet is an error — the next round retries.
pub fn render(path: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("render {}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find("artifact:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "artifact:".len()..];
        let hex: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() == 64 {
            let placed = dir.join(hex.to_ascii_lowercase());
            if !placed.exists() {
                return Err(format!("referenced artifact {} not placed yet — retrying next round", &hex[..12]));
            }
            out.push_str(&placed.display().to_string());
            rest = &after[64..];
        } else {
            out.push_str("artifact:");
            rest = after;
        }
    }
    out.push_str(rest);
    let rendered = PathBuf::from(format!("{}.rendered", path.display()));
    std::fs::write(&rendered, out).map_err(|e| format!("render {}: {e}", rendered.display()))?;
    Ok(rendered)
}

/// Each `[[activation]]`'s egress proxy, by its index in the unit's list, as the stem started it — or why it
/// could not start (post-360 rows L and M). An activation naming `[activation.confine.egress]` with no
/// entry here is refused.
pub type EgressProxies = Arc<std::collections::HashMap<usize, Result<Arc<crate::EgressProxy>, String>>>;

/// The hook a stem gives its [`BlobRuntime`](crate::BlobRuntime): dispatch by the entry's
/// capability to its `[[activation]]`; no matching section means placement is the whole install.
/// An activation naming `[activation.confine.egress]` is refused here — it needs its proxy
/// ([`hook_with_egress`]).
pub fn hook(
    decls: Vec<ActivationDecl>,
    watch: WatchList,
) -> impl Fn(&InstallableEntry, &Path, &ActivationCtx) -> Result<Option<Arc<AtomicBool>>, String> + Send + Sync + 'static {
    hook_with_egress(decls, watch, EgressProxies::default())
}

/// [`hook`], with each activation's egress proxy (post-360 rows L and M): an activation whose
/// `[activation.confine.egress]` proxy is missing or could not start is refused by name
/// (`egress_proxy_unavailable`) and nothing runs; one whose proxy is up runs confined to that proxy's port,
/// in the proxy's cleared environment.
pub fn hook_with_egress(
    decls: Vec<ActivationDecl>,
    watch: WatchList,
    proxies: EgressProxies,
) -> impl Fn(&InstallableEntry, &Path, &ActivationCtx) -> Result<Option<Arc<AtomicBool>>, String> + Send + Sync + 'static {
    // Review finding 5 (#636): each activation's profile is resolved once — per declaration and placed
    // file — and kept for the stem's life, its paths held open, re-probes included. Lock-order table row
    // 63: a leaf, held for a lookup or an insert, never while a command runs.
    let resolved = Arc::new(ResolvedProfiles::with_capacity(RESOLVED_PROFILES_CAP));
    move |entry, path, actx| {
        let target = format!("{}/{}", entry.provides.namespace, entry.provides.name);
        let trigger = format!("install token {}", actx.install_token);
        // `prov.activation` / `prov.probe` records, from the outcome each step already produced,
        // under the install's token (the link the plan's I5 left open).
        let rec = |rule: &str, outcome: OutcomeKind, reason: &str, effect: Option<String>| {
            if let Some(t) = &actx.trace {
                t.record(rule, trigger.clone(), outcome, reason, target.clone(), Vec::new(), effect);
            }
        };
        let Some((index, decl)) = decls
            .iter()
            .enumerate()
            .find(|(_, d)| d.ns == entry.provides.namespace.as_ref() && d.name == entry.provides.name.as_ref())
        else {
            rec("prov.activation", OutcomeKind::NoAction, "no_declaration", None);
            return Ok(None);
        };
        // Row M: the profile first — a profile this node cannot apply refuses the activation by name and
        // nothing runs, not even the render. Resolved once, here; the re-probes take it from the same
        // resolved profiles, re-resolved only when a declared path was replaced.
        let profile = match &decl.confine {
            None => None,
            Some(c) => match proxy_for(&proxies, index, c).and_then(|proxy| resolved.get_or_resolve(index, c, path, proxy.as_deref())) {
                Ok(p) => Some(p),
                Err(refusal) => {
                    rec("prov.activation", OutcomeKind::Refusal, refusal.reason(), None);
                    return Err(format!("activation {target}: its [activation.confine] profile cannot be applied — {refusal}; refused, not run unconfined"));
                }
            },
        };
        // The proxy the profile was resolved with (rows L and M); the probes re-resolve with it.
        let proxy = decl.confine.as_ref().and_then(|c| proxy_for(&proxies, index, c).ok().flatten());
        let rendered = if decl.resolve_artifact_refs {
            match render(path) {
                Ok(r) => Some(r),
                Err(e) => {
                    rec("prov.activation", OutcomeKind::Refusal, if e.contains("not placed") { "reference_not_placed_yet" } else { "render_failed" }, None);
                    return Err(e);
                }
            }
        } else { None };
        let timeout = decl.timeout_secs.map(Duration::from_secs).unwrap_or(DEFAULT_TIMEOUT);
        if let Err(e) = run_under(&expand(&decl.command, entry, path, rendered.as_deref()), timeout, profile.as_ref()) {
            rec("prov.activation", OutcomeKind::Refusal, if e.contains(DID_NOT_FINISH) { "command_timed_out" } else { "command_failed" }, None);
            return Err(e);
        }
        rec("prov.activation", OutcomeKind::Action, "activated", Some(decl.command.join(" ")));
        if decl.probe.is_empty() {
            rec("prov.probe", OutcomeKind::NoAction, "no_probe_declared", None);
            return Ok(None);
        }
        // The probe gates the capability (D21): a failing initial probe is an activation error, so the
        // install fails at stage `activation`, nothing is advertised, and the next round retries —
        // never a live install that the health pass withdraws a round later.
        // Review of #641, finding 4: the initial probe takes the re-probe's both-sides currency check. The
        // command before it may replace its own declared paths (a setup step that recreates a scratch
        // directory); the probe re-resolves them, and a path that changes under the probe itself is not trusted.
        let probe_argv = expand(&decl.probe, entry, path, rendered.as_deref());
        let probed = match &decl.confine {
            None => run_under(&probe_argv, PROBE_TIMEOUT, None).map_err(|e| ("initial_probe_failed", e)),
            Some(c) => probe_confined(&resolved, index, c, path, &probe_argv, proxy.as_deref()).map_err(|(reason, e)| {
                (if reason == "probe_failed" { "initial_probe_failed" } else { reason }, e)
            }),
        };
        if let Err((reason, e)) = probed {
            rec("prov.probe", OutcomeKind::Refusal, reason, None);
            return Err(format!("initial probe failed after activation: {e}"));
        }
        rec("prov.probe", OutcomeKind::Action, "healthy", Some("health flag set; re-probed on the stem's tick".into()));
        let health = Arc::new(AtomicBool::new(true));
        watch.lock().unwrap_or_else(|e| e.into_inner()).push(Watched {
            decl: decl.clone(),
            entry: entry.clone(),
            path: path.to_path_buf(),
            health: Arc::clone(&health),
            index,
            resolved: Arc::clone(&resolved),
            proxy,
            token: actx.install_token,
            trace: actx.trace.clone(),
        });
        Ok(Some(health))
    }
}

/// The running egress proxy an activation's profile names, `None` when it names none, or the refusal.
fn proxy_for(proxies: &EgressProxies, index: usize, c: &mycelium::ConfineDecl) -> Result<Option<Arc<crate::EgressProxy>>, crate::activation_sandbox::Refusal> {
    use crate::activation_sandbox::Refusal;
    if c.egress.is_none() {
        return Ok(None);
    }
    match proxies.get(&index) {
        Some(Ok(p)) => Ok(Some(Arc::clone(p))),
        Some(Err(why)) => Err(Refusal::EgressUnavailable(format!("{why} — refused, not run without it"))),
        None => Err(Refusal::EgressUnavailable("its [activation.confine.egress] proxy was never started — refused, not run without it".into())),
    }
}

/// Each activation's resolved confinement profile, per declaration and placed file (review finding 5,
/// #636). Lock-order table row 63: a leaf, held for a lookup or an insert, never while a command runs.
pub(crate) struct ResolvedProfiles {
    map: Mutex<std::collections::HashMap<(usize, PathBuf), Profile>>,
    cap: usize,
}

impl ResolvedProfiles {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        ResolvedProfiles { map: Mutex::default(), cap }
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, index: usize, placed: &Path) -> bool {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&(index, placed.to_path_buf()))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The profile resolved for `(index, placed)`, kept while every path it pinned still names what it
    /// pinned; re-resolved — with every check `prepare` makes, symlinks included — when one was removed,
    /// recreated or replaced (review follow-up 1, #636), never used stale. A placed file that is gone
    /// (`BlobRuntime::uninstall` removed it) is forgotten, and at most `cap` profiles are kept.
    pub(crate) fn get_or_resolve(&self, index: usize, c: &mycelium::ConfineDecl, placed: &Path, proxy: Option<&crate::EgressProxy>) -> Result<Profile, crate::activation_sandbox::Refusal> {
        let key = (index, placed.to_path_buf());
        let cached = self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
        // Checked outside the lock (it reads the filesystem).
        if let Some(p) = cached.filter(|p| p.is_current()) {
            return Ok(p);
        }
        let fresh = crate::activation_sandbox::prepare_with(c, placed, proxy)?;
        let gone: Vec<(usize, PathBuf)> = {
            let g = self.map.lock().unwrap_or_else(|e| e.into_inner());
            g.keys().filter(|(_, p)| p != placed).cloned().collect()
        };
        let gone: Vec<(usize, PathBuf)> = gone.into_iter().filter(|(_, p)| !p.exists()).collect();
        let mut g = self.map.lock().unwrap_or_else(|e| e.into_inner());
        for k in &gone {
            g.remove(k);
        }
        while g.len() >= self.cap && !g.contains_key(&key) {
            let Some(victim) = g.keys().next().cloned() else { break };
            g.remove(&victim);
        }
        g.insert(key, fresh.clone());
        Ok(fresh)
    }
}

/// How many probes one re-probe round runs at once (review of #641 round 2, finding D).
const REPROBE_CONCURRENCY: usize = 8;

/// How many times a probe runs when the profile changed under the run, before it refuses.
const REPROBE_ATTEMPTS: usize = 2;

/// Run a confined probe under a **current** profile and trust its verdict only if the profile is still
/// current after it — the both-sides rule (plan §3.6, M's follow-up; review of #641, findings 1 and 4).
/// The profile comes from `resolved`, re-resolved with every check when a declared path was replaced; one
/// not current before the run (a path swapped since it was resolved) is not run, one not current after it
/// is not trusted, and either is re-resolved once more. `Err((reason, why))`: a refused resolution's
/// reason, `probe_failed`, or `confinement_changed`.
///
/// Not retried: a declared path **missing** when it is resolved — a rotation that is not atomic (`mv out
/// out.old && mkdir out`) caught between its two steps — refuses as `confinement_failed`, and the install is
/// withdrawn and reinstalled. Rotate by `rename(2)` over the old path, or by flipping a symlink above it.
fn probe_confined(
    resolved: &ResolvedProfiles,
    index: usize,
    c: &mycelium::ConfineDecl,
    path: &Path,
    argv: &[String],
    proxy: Option<&crate::EgressProxy>,
) -> Result<(), (&'static str, String)> {
    for _ in 0..REPROBE_ATTEMPTS {
        let profile = resolved.get_or_resolve(index, c, path, proxy).map_err(|refusal| {
            (refusal.reason(), format!("its [activation.confine] profile cannot be re-resolved — {refusal}; refused, not run"))
        })?;
        if !profile.is_current() {
            continue;
        }
        let ran = run_under(argv, PROBE_TIMEOUT, Some(&profile));
        if profile.is_current() {
            return ran.map_err(|e| ("probe_failed", e));
        }
    }
    Err(("confinement_changed", format!(
        "a declared path of its [activation.confine] profile changed around each of {REPROBE_ATTEMPTS} probes; the verdict is not trusted"
    )))
}

/// One re-probe of one live activation: run its probe, set its health flag, and record a change of
/// verdict under the install's token. Blocking (it runs the probe); `spawn_reprobe` calls it off the
/// async workers.
///
/// A confined probe takes its profile from the hook's resolved profiles, which re-resolve — every check
/// `prepare` makes, symlinks, the canonical comparison and existence included — when a declared path was
/// removed, recreated or replaced (plan §3.6, M's follow-up): a path replaced legitimately (a rotated
/// directory, an atomic rename) keeps the install live, and only a refused re-resolution withdraws it, the
/// refusal named. A profile that is no longer current **after** the run ran stale for some of it (a path
/// swapped between the currency check and the run): its verdict is not trusted, the profile is re-resolved
/// and the probe run again, and a change under every attempt refuses (`confinement_changed`).
fn reprobe_one(w: &Watched) -> bool {
    let target = format!("{}/{}", w.entry.provides.namespace, w.entry.provides.name);
    let rendered = w.decl.resolve_artifact_refs.then(|| PathBuf::from(format!("{}.rendered", w.path.display())));
    let argv = expand(&w.decl.probe, &w.entry, &w.path, rendered.as_deref());
    let was = w.health.load(Ordering::Relaxed);
    // `Err((reason, why))`: the trace's reason and what the warning and the record's effect name.
    let verdict: Result<(), (&'static str, String)> = match &w.decl.confine {
        None => run_under(&argv, PROBE_TIMEOUT, None).map_err(|e| ("probe_failed", e)),
        Some(c) => probe_confined(&w.resolved, w.index, c, &w.path, &argv, w.proxy.as_deref()),
    };
    let ok = verdict.is_ok();
    w.health.store(ok, Ordering::Relaxed);
    if let Err((reason, why)) = &verdict {
        tracing::warn!(activation = %target, reason = *reason, "activation re-probe failed — the provisioner will withdraw and reinstall: {why}");
    }
    // A re-probe records only a **change** of verdict (the steady state is the install's record), under
    // the install's token.
    if ok != was && let Some(t) = &w.trace {
        let (outcome, reason, effect) = match &verdict {
            Ok(()) => (OutcomeKind::Action, "healthy", "health flag set".to_string()),
            Err((reason, why)) => (OutcomeKind::Refusal, *reason, format!("health flag cleared; the next health pass withdraws — {why}")),
        };
        t.record("prov.probe", format!("install token {}", w.token), outcome, reason, target, Vec::new(), Some(effect));
    }
    ok
}

/// Re-run every live activation's probe each `every`, flipping its flag; drop the ones whose
/// install is gone (no holder but this list).
pub fn spawn_reprobe(watch: WatchList, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            let live: Vec<Watched> = {
                let mut g = watch.lock().unwrap_or_else(|e| e.into_inner());
                g.retain(|w| Arc::strong_count(&w.health) > 1);
                g.clone()
            };
            reprobe_round(live).await;
        }
    })
}

/// One re-probe round over `live`: the entries' probes concurrently, each on its own blocking thread, at
/// most [`REPROBE_CONCURRENCY`] at once — so one slow or churning entry (at most `REPROBE_ATTEMPTS` ×
/// `PROBE_TIMEOUT`) holds one slot and delays no other entry's verdict beyond the queue for a slot, and a
/// stem with hundreds of installs does not fork every probe at one instant (review of #641, findings 7 and
/// D). The next round starts when the slowest finishes.
async fn reprobe_round(live: Vec<Watched>) {
    let slots = Arc::new(tokio::sync::Semaphore::new(REPROBE_CONCURRENCY));
    let probes: Vec<_> = live
        .into_iter()
        .map(|w| {
            let slots = Arc::clone(&slots);
            tokio::spawn(async move {
                let _slot = slots.acquire_owned().await;
                tokio::task::spawn_blocking(move || reprobe_one(&w)).await
            })
        })
        .collect();
    for p in probes {
        // `reprobe_one` warns on a failed probe, naming the activation and why.
        match p.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) | Err(e) => tracing::warn!("activation re-probe task failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    /// A verbose activation command must not stall on its own diagnostics (360 review F3,
    /// 2026-10-02): the runner read stderr only after exit, so a child writing more than a pipe
    /// buffer blocked on the write and was reported as a timeout.
    #[test]
    fn a_verbose_command_that_succeeds_is_not_reported_as_a_timeout() {
        let started = Instant::now();
        let got = run(&sh("head -c 1048576 /dev/zero | tr '\\0' x >&2; exit 0"), Duration::from_secs(10));
        assert_eq!(got, Ok(()), "1 MiB of stderr then exit 0 must succeed");
        assert!(started.elapsed() < Duration::from_secs(5), "and promptly: {:?}", started.elapsed());
    }

    /// A failing command's error carries the tail of what it said, bounded however much it said.
    #[test]
    fn a_failing_command_reports_a_bounded_tail_of_its_stderr() {
        let err = run(
            &sh("head -c 1048576 /dev/zero | tr '\\0' x >&2; echo ' the last words' >&2; exit 3"),
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(err.contains("the last words"), "the tail survives: {}", &err[err.len().saturating_sub(80)..]);
        assert!(err.len() <= STDERR_TAIL_BYTES + 200, "bounded: {} bytes", err.len());
    }

    /// A failing *initial* probe is an activation error, not a live install: `[[activation]]` says the
    /// probe gates the capability (D21), and until 2026-10-03 the hook returned `Ok(Some(false))` —
    /// the install completed, the capability was advertised and counted, and only the next round's
    /// health pass withdrew it (plan I4 reconnaissance; the I5 exit gate "an activation failure is
    /// never recorded as success").
    #[test]
    fn a_failing_initial_probe_is_an_activation_error() {
        let dir = std::env::temp_dir().join(format!("act-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pack");
        std::fs::write(&path, b"bytes").unwrap();
        let decl = ActivationDecl {
            ns: "data".into(),
            name: "pack".into(),
            command: vec!["true".into()],
            probe: vec!["false".into()],
            timeout_secs: Some(5),
            resolve_artifact_refs: false,
            confine: None,
        };
        let entry = crate::InstallableEntry::new(mycelium::Capability::new("data", "pack"), crate::ArtifactId::from_bytes([7u8; 32]));
        let watch: WatchList = Default::default();
        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let actx = ActivationCtx { install_token: 7, trace: Some(Arc::new(TraceCtx { sink: Arc::clone(&sink), node: "n".into(), build: "t".into(), config_digest: "d".into(), profile: None })) };
        let got = hook(vec![decl], Arc::clone(&watch))(&entry, &path, &actx);
        // The link the plan's I5 left open: activation and probe record under the install's token.
        let recs: Vec<(String, String, String)> = sink.snapshot().into_iter().map(|r| (r.rule, r.reason, r.trigger)).collect();
        assert_eq!(recs, [("prov.activation".to_string(), "activated".to_string(), "install token 7".to_string()),
                          ("prov.probe".to_string(), "initial_probe_failed".to_string(), "install token 7".to_string())], "{recs:?}");
        match got {
            Err(e) => assert!(e.contains("probe"), "names the probe: {e}"),
            Ok(h) => panic!("a failing initial probe must be an activation error, got Ok({:?})", h.map(|h| h.load(std::sync::atomic::Ordering::Relaxed))),
        }
        assert!(watch.lock().unwrap().is_empty(), "nothing is watched for a capability that never went live");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Post-360 row M: confined activations ────────────────────────────────────────────────────

    struct Scene {
        root:   std::path::PathBuf,
        placed: std::path::PathBuf,
    }

    impl Scene {
        /// `root/placed/pack` (the placed blob), `root/outside/secret`, `root/models/m`, `root/out/`. Every
        /// path canonical, so a profile names what the kernel sees (`/tmp` is `/private/tmp` on macOS).
        fn new(tag: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!("act-confine-{tag}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&root);
            for d in ["placed", "outside", "models", "out"] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            let root = root.canonicalize().unwrap();
            std::fs::write(root.join("placed/pack"), b"the placed bytes").unwrap();
            std::fs::write(root.join("outside/secret"), b"not yours").unwrap();
            std::fs::write(root.join("models/m"), b"a declared model").unwrap();
            Scene { placed: root.join("placed/pack"), root }
        }
        fn p(&self, rel: &str) -> String {
            self.root.join(rel).display().to_string()
        }
    }

    impl Drop for Scene {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn confined(command: Vec<String>, probe: Vec<String>, confine: mycelium::ConfineDecl) -> ActivationDecl {
        ActivationDecl {
            ns: "data".into(),
            name: "pack".into(),
            command,
            probe,
            timeout_secs: Some(10),
            resolve_artifact_refs: false,
            confine: Some(confine),
        }
    }

    fn pack_entry() -> InstallableEntry {
        crate::InstallableEntry::new(mycelium::Capability::new("data", "pack"), crate::ArtifactId::from_bytes([7u8; 32]))
    }

    /// Run one activation through the stem's hook; the result and what the trace recorded.
    fn activate(decl: ActivationDecl, path: &Path) -> (Result<(), String>, Vec<(String, String)>) {
        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let actx = ActivationCtx { install_token: 11, trace: Some(Arc::new(TraceCtx { sink: Arc::clone(&sink), node: "n".into(), build: "t".into(), config_digest: "d".into(), profile: None })) };
        let got = hook(vec![decl], Default::default())(&pack_entry(), path, &actx).map(|_| ());
        (got, sink.snapshot().into_iter().map(|r| (r.rule, r.reason)).collect())
    }

    /// The backend this platform cannot apply: Landlock on macOS, Seatbelt everywhere else.
    fn foreign_backend() -> mycelium::ConfineBackend {
        if cfg!(target_os = "macos") { mycelium::ConfineBackend::Landlock } else { mycelium::ConfineBackend::Seatbelt }
    }

    /// A profile the platform cannot apply refuses the activation **by name** and never runs it
    /// unconfined (row M). Runs on every platform: it forces the backend this one does not have. Seen
    /// failing first: the profile was ignored, the command ran unconfined and the activation succeeded.
    #[test]
    fn a_profile_the_platform_cannot_apply_refuses_the_activation_by_name() {
        let s = Scene::new("foreign");
        let ran = s.p("placed/ran");
        let foreign = foreign_backend();
        let decl = confined(sh(&format!("touch {ran}")), Vec::new(), mycelium::ConfineDecl { backend: foreign, ..Default::default() });
        let (got, recs) = activate(decl, &s.placed);
        let e = got.expect_err("a profile this platform cannot apply must refuse the activation");
        assert!(e.contains(foreign.as_str()) && e.contains("not run unconfined") && e.contains("data/pack"), "names the backend, the activation and the refusal: {e}");
        assert!(!Path::new(&ran).exists(), "the command never ran");
        assert_eq!(recs, [("prov.activation".to_string(), "confinement_unavailable".to_string())]);
    }

    /// Filesystem confinement, shown on this platform's own backend: a confined command reads the placed
    /// blob and its declared paths, writes only its declared write paths, and reads nothing else — and
    /// the probe runs under the same profile. On Linux without Landlock the test cannot show anything and
    /// says so — unless `MYCELIUM_CONFINE_LIVE_REQUIRED` is set (CI sets it), when it fails. Seen failing
    /// first: unconfined, the secret was read and every write landed.
    #[test]
    fn a_confined_activation_reads_and_writes_only_what_its_profile_names() {
        if !crate::activation_sandbox::native_backend_or_skip("a_confined_activation_reads_and_writes_only_what_its_profile_names") {
            return;
        }
        let s = Scene::new("fs");
        let profile = mycelium::ConfineDecl { read: vec![s.p("models")], write: vec![s.p("out")], ..Default::default() };
        let run = |cmd: String, probe: Vec<String>| activate(confined(sh(&cmd), probe, profile.clone()), &s.placed).0;

        // What the profile names: the placed blob ({path}, in the readable placement directory), a declared
        // read path, a declared write path.
        let ok = run(format!("cat {{path}} > {out}/blob && cat {m} > {out}/model", out = s.p("out"), m = s.p("models/m")), Vec::new());
        assert_eq!(ok, Ok(()), "the declared paths are usable");
        assert_eq!(std::fs::read(s.root.join("out/blob")).unwrap(), b"the placed bytes");
        assert_eq!(std::fs::read(s.root.join("out/model")).unwrap(), b"a declared model");

        // What it does not.
        let e = run(format!("cat {}", s.p("outside/secret")), Vec::new()).expect_err("a path outside the profile is not readable");
        assert!(e.contains("exited"), "{e}");
        assert!(run(format!("echo x > {}", s.p("placed/new")), Vec::new()).is_err(), "the placement directory is read-only");
        assert!(!s.root.join("placed/new").exists());
        assert!(run(format!("echo x > {}", s.p("models/new")), Vec::new()).is_err(), "a read path is read-only");
        assert!(!s.root.join("models/new").exists());
        assert!(run(format!("echo x > {}", s.p("outside/new")), Vec::new()).is_err(), "an undeclared path is not writable");
        assert!(!s.root.join("outside/new").exists());

        // The probe runs under the same profile: a probe reading outside it fails the activation.
        let e = run("true".into(), vec!["cat".into(), s.p("outside/secret")]).expect_err("the probe is confined too");
        assert!(e.contains("probe"), "{e}");
    }

    /// Network confinement, shown on this platform's own backend: with `connect` empty a confined command
    /// reaches nothing; with `*:<port>` it reaches that TCP port and no other, and no UDP at all. Seen
    /// failing first: unconfined, every connection opened.
    #[test]
    fn a_confined_activation_reaches_only_its_declared_tcp_ports() {
        if !crate::activation_sandbox::native_backend_or_skip("a_confined_activation_reaches_only_its_declared_tcp_ports") {
            return;
        }
        let s = Scene::new("net");
        // Two listening ports, so a refused connection is the profile's doing, not an absent listener.
        let open = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (a, b) = (open.local_addr().unwrap().port(), other.local_addr().unwrap().port());
        let tcp = |port: u16| vec!["bash".to_string(), "-c".into(), format!("exec 3<>/dev/tcp/127.0.0.1/{port}")];
        let udp = |port: u16| vec!["bash".to_string(), "-c".into(), format!("exec 3<>/dev/udp/127.0.0.1/{port} && echo x >&3")];
        let run = |argv: Vec<String>, connect: Vec<String>| {
            activate(confined(argv, Vec::new(), mycelium::ConfineDecl { connect, ..Default::default() }), &s.placed).0
        };
        // Unconfined, both ports open: the plant that shows the listeners answer.
        assert_eq!(super::run(&tcp(a), Duration::from_secs(5)), Ok(()));
        assert_eq!(super::run(&tcp(b), Duration::from_secs(5)), Ok(()));

        assert!(run(tcp(a), Vec::new()).is_err(), "no `connect`: no network");
        let declared = vec![format!("*:{a}")];
        assert_eq!(run(tcp(a), declared.clone()), Ok(()), "the declared port is reachable");
        assert!(run(tcp(b), declared.clone()).is_err(), "an undeclared port is not");
        assert!(run(udp(a), declared.clone()).is_err(), "UDP is not, whatever the port");

        // `localhost:<port>`: Seatbelt holds it; Landlock limits ports, not addresses, so it refuses by name.
        let local = run(tcp(a), vec![format!("localhost:{a}")]);
        if cfg!(target_os = "macos") {
            assert_eq!(local, Ok(()), "Seatbelt limits to localhost");
        } else {
            let e = local.expect_err("Landlock cannot hold `localhost:<port>`");
            assert!(e.contains("landlock") && e.contains("localhost") && e.contains("not run unconfined"), "{e}");
        }
        drop((open, other));
    }

    /// `python3` for the probes bash cannot make; under `MYCELIUM_CONFINE_LIVE_REQUIRED` its absence fails.
    fn python3_or_skip(test: &str) -> bool {
        if run(&["python3".to_string(), "-c".into(), "pass".into()], Duration::from_secs(10)).is_ok() {
            return true;
        }
        if std::env::var_os("MYCELIUM_CONFINE_LIVE_REQUIRED").is_some() {
            panic!("{test}: MYCELIUM_CONFINE_LIVE_REQUIRED is set and python3 is not on this host");
        }
        eprintln!("{test}: SKIPPED — no python3");
        false
    }

    /// A probe written in Python: `mode` against 127.0.0.1:`port`, exit 0 if the call succeeded.
    const NET_PROBE: &str = r#"import socket,sys
mode,port=sys.argv[1],int(sys.argv[2])
s=socket.socket(socket.AF_INET,socket.SOCK_STREAM)
try:
    if mode=='fastopen': s.sendto(b'x',socket.MSG_FASTOPEN,('127.0.0.1',port))
    elif mode=='fastopen_msg': s.sendmsg([b'x'],[],socket.MSG_FASTOPEN,('127.0.0.1',port))
    elif mode=='listen': s.listen(1)
    elif mode=='connect': s.connect(('127.0.0.1',port))
except OSError as e:
    print(e,file=sys.stderr); sys.exit(3)
"#;

    /// Where this host's `python3` lives (`sys.base_prefix`, canonical), for a profile's `read`: on macOS
    /// it is under `/opt/homebrew` or `/Library`, outside the always-readable set.
    fn python_prefix() -> String {
        let out = std::process::Command::new("python3").args(["-c", "import sys,os;print(os.path.realpath(sys.base_prefix))"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn net_probe(mode: &str, port: u16) -> Vec<String> {
        vec!["python3".into(), "-I".into(), "-S".into(), "-c".into(), NET_PROBE.into(), mode.into(), port.to_string()]
    }

    /// Review finding 1 (#636): a confined command with one declared port reached any other by TCP Fast
    /// Open (`sendto`/`sendmsg` with `MSG_FASTOPEN` connects without `connect(2)`, past Landlock's hook),
    /// and could `listen()` on an auto-bound port. Both refused now, by the syscalls themselves — not
    /// bash's `/dev/tcp`, which only ever calls `connect`. Seen failing first on Linux (both succeeded).
    #[test]
    fn a_confined_activation_cannot_reach_an_undeclared_port_by_fast_open_or_listen() {
        const T: &str = "a_confined_activation_cannot_reach_an_undeclared_port_by_fast_open_or_listen";
        if !crate::activation_sandbox::native_backend_or_skip(T) || !python3_or_skip(T) {
            return;
        }
        let s = Scene::new("tfo");
        let open = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (a, b) = (open.local_addr().unwrap().port(), other.local_addr().unwrap().port());
        let profile = mycelium::ConfineDecl { read: vec![python_prefix()], connect: vec![format!("*:{a}")], ..Default::default() };
        let run_c = |argv: Vec<String>| activate(confined(argv, Vec::new(), profile.clone()), &s.placed).0;
        // The plants: unconfined, each call succeeds; confined, the declared port is still reachable.
        assert_eq!(super::run(&net_probe("listen", 0), Duration::from_secs(10)), Ok(()));
        assert_eq!(run_c(net_probe("connect", a)), Ok(()), "the declared port");
        let modes: &[&str] = if cfg!(target_os = "linux") {
            assert_eq!(super::run(&net_probe("fastopen", b), Duration::from_secs(10)), Ok(()));
            assert_eq!(super::run(&net_probe("fastopen_msg", b), Duration::from_secs(10)), Ok(()));
            &["fastopen", "fastopen_msg"]
        } else {
            &[] // macOS's Python has no MSG_FASTOPEN; Seatbelt checks the connection, not the call
        };
        for mode in modes {
            assert!(run_c(net_probe(mode, b)).is_err(), "{mode} to an undeclared port must be refused");
        }
        assert!(run_c(net_probe("listen", 0)).is_err(), "listen() must be refused");
        drop((open, other));
    }

    /// Review finding 4 (#636): on x86_64 an x32 syscall number (`nr | 0x40000000`) matched no rule of
    /// the seccomp filter, whose default is allow, so `socket(2)` through the x32 table passed it. Now
    /// refused with the filter's own `EACCES`. Not applicable off x86_64 (aarch64 has no x32 table).
    /// Seen failing first in CI (x86_64): the errno was not the filter's.
    #[test]
    fn an_x32_syscall_is_refused_by_the_filter() {
        const T: &str = "an_x32_syscall_is_refused_by_the_filter";
        if !(cfg!(target_os = "linux") && cfg!(target_arch = "x86_64")) {
            eprintln!("{T}: not applicable — x32 is an x86_64 Linux table");
            return;
        }
        if !crate::activation_sandbox::native_backend_or_skip(T) || !python3_or_skip(T) {
            return;
        }
        let s = Scene::new("x32");
        let script = "import ctypes,sys\nlibc=ctypes.CDLL(None,use_errno=True)\nr=libc.syscall(41|0x40000000,1,1,0)\ne=ctypes.get_errno()\nprint(r,e,file=sys.stderr)\nsys.exit(0 if r==-1 and e==13 else 3)\n";
        let profile = mycelium::ConfineDecl { read: vec![python_prefix()], ..Default::default() };
        let got = activate(confined(vec!["python3".into(), "-I".into(), "-S".into(), "-c".into(), script.into()], Vec::new(), profile), &s.placed).0;
        assert_eq!(got, Ok(()), "an x32 socket(2) is refused with the filter's EACCES");
    }

    /// Review finding 2 (#636): the always-readable set held `/etc` (Linux) and `/private/etc` and all of
    /// `/usr` (macOS) — the node key's documented home is `/etc/mycelium/node.key`. Now only what loading,
    /// resolving and TLS need. A world-readable `/etc` file outside that list stands in for the key. Seen
    /// failing first: it was read.
    #[test]
    fn a_confined_activation_cannot_read_the_hosts_unlisted_configuration() {
        const T: &str = "a_confined_activation_cannot_read_the_hosts_unlisted_configuration";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let candidates: &[&str] = if cfg!(target_os = "macos") {
            &["/private/etc/shells", "/private/etc/profile", "/private/etc/bashrc"]
        } else {
            &["/etc/hostname", "/etc/debian_version", "/etc/issue", "/etc/environment", "/etc/shells"]
        };
        let readable = candidates.iter().find(|p| run(&sh(&format!("cat {p} >/dev/null")), Duration::from_secs(5)).is_ok());
        let Some(file) = readable else {
            if std::env::var_os("MYCELIUM_CONFINE_LIVE_REQUIRED").is_some() {
                panic!("{T}: no readable stand-in among {candidates:?}");
            }
            eprintln!("{T}: SKIPPED — no readable stand-in");
            return;
        };
        let s = Scene::new("etc");
        let got = activate(confined(sh(&format!("cat {file}")), Vec::new(), mycelium::ConfineDecl::default()), &s.placed).0;
        assert!(got.is_err(), "{file} is outside the always-readable set and must not be readable");
        // What a loader, a resolver and TLS need still is.
        let ok = activate(confined(sh("cat /etc/hosts >/dev/null"), Vec::new(), mycelium::ConfineDecl::default()), &s.placed).0;
        assert_eq!(ok, Ok(()), "/etc/hosts stays readable");
    }

    /// Review finding 5 (#636): a confined run with `write = ["{dir}"]` replaced a declared read path
    /// under it with a symlink to a directory the profile never named, and the next run — which resolved
    /// the profile afresh — was granted it. Now the first resolution is kept for the activation's life,
    /// and a declared path that passes through a symlink inside a writable root is refused. Seen failing
    /// first: the second run read the secret.
    #[test]
    fn a_symlink_planted_by_one_run_does_not_widen_the_next() {
        const T: &str = "a_symlink_planted_by_one_run_does_not_widen_the_next";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("symlink");
        std::fs::create_dir_all(s.root.join("placed/models")).unwrap();
        std::fs::write(s.root.join("placed/models/m"), b"declared").unwrap();
        let outside = s.p("outside");
        let profile = mycelium::ConfineDecl { read: vec!["{dir}/models".into()], write: vec!["{dir}".into()], ..Default::default() };
        // One command: the first run plants the symlink, every later run reads through it.
        let cmd = format!(
            "if [ -e {{dir}}/planted ]; then cat {{dir}}/models/secret; else rm -rf {{dir}}/models && ln -s {outside} {{dir}}/models && touch {{dir}}/planted; fi"
        );
        let actx = ActivationCtx { install_token: 1, trace: None };
        // One hook for every run, as one stem's provisioner holds it.
        let h = hook(vec![confined(sh(&cmd), Vec::new(), profile.clone())], Default::default());
        assert!(h(&pack_entry(), &s.placed, &actx).is_ok(), "the first run may rewrite its writable {{dir}}");
        assert!(std::fs::symlink_metadata(s.root.join("placed/models")).unwrap().file_type().is_symlink(), "the plant");
        let got = h(&pack_entry(), &s.placed, &actx);
        assert!(got.is_err(), "the same stem's next run must not read through the planted symlink: {got:?}");
        // A stem started afresh resolves anew — and refuses a declared path through a symlink in a writable root.
        let fresh = hook(vec![confined(sh(&cmd), Vec::new(), profile)], Default::default());
        let e = fresh(&pack_entry(), &s.placed, &actx).expect_err("a fresh resolution through the symlink is refused");
        assert!(e.contains("symlink") && e.contains("not run unconfined"), "{e}");
    }

    /// Review follow-up 1 (#636): a declared path removed and recreated between two runs — a scratch
    /// directory cleaned, a re-heal's new inode at the same path — must be the recreated one at the next
    /// run, never a stale pin that refuses everything until the stem restarts. Seen failing first on
    /// Linux: the second run's write was denied (the rules came from the first directory's descriptor).
    #[test]
    fn a_declared_path_removed_and_recreated_between_runs_is_the_new_one() {
        const T: &str = "a_declared_path_removed_and_recreated_between_runs_is_the_new_one";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("recreate");
        let out = s.p("out");
        let profile = mycelium::ConfineDecl { write: vec![out.clone()], ..Default::default() };
        let h = hook(vec![confined(sh(&format!("echo run > {out}/f")), Vec::new(), profile)], Default::default());
        let actx = ActivationCtx { install_token: 1, trace: None };
        assert!(h(&pack_entry(), &s.placed, &actx).is_ok(), "the first run writes");
        std::fs::remove_dir_all(&out).unwrap();
        std::fs::create_dir(&out).unwrap();
        let got = h(&pack_entry(), &s.placed, &actx);
        assert!(got.is_ok(), "the recreated directory is the declared one: {got:?}");
        assert_eq!(std::fs::read(s.root.join("out/f")).unwrap(), b"run\n");
    }

    /// Activate through one hook with a trace, and hand back the watched entry the re-probe task keeps,
    /// its health flag and the trace.
    fn activate_watched(decl: ActivationDecl, path: &Path) -> (Watched, Arc<AtomicBool>, Arc<mycelium::decision::DecisionSink>) {
        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let actx = ActivationCtx { install_token: 13, trace: Some(Arc::new(TraceCtx { sink: Arc::clone(&sink), node: "n".into(), build: "t".into(), config_digest: "d".into(), profile: None })) };
        let watch: WatchList = Default::default();
        let health = hook(vec![decl], Arc::clone(&watch))(&pack_entry(), path, &actx)
            .expect("the activation runs")
            .expect("a declared probe yields a health flag");
        let w = watch.lock().unwrap()[0].clone();
        (w, health, sink)
    }

    /// The re-probe records after the activation's own (`prov.probe`, reason, effect).
    fn probe_records(sink: &mycelium::decision::DecisionSink) -> Vec<(String, String, String)> {
        sink.snapshot()
            .into_iter()
            .filter(|r| r.rule == "prov.probe" && r.reason != "healthy")
            .map(|r| (r.rule, r.reason, r.effect.unwrap_or_default()))
            .collect()
    }

    /// Plan §3.6, M's follow-up: a declared path replaced legitimately — a write directory rotated, a
    /// read file replaced by an atomic rename — is re-resolved inside the re-probe, every check repeated,
    /// and the probe runs under the fresh profile: the install stays live. Seen failing first: the
    /// re-probe found the profile stale and failed, so the install was withdrawn and reinstalled.
    #[test]
    fn a_replaced_write_directory_keeps_the_install_live_across_a_reprobe() {
        const T: &str = "a_replaced_write_directory_keeps_the_install_live_across_a_reprobe";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("rotate");
        let (out, model) = (s.p("out"), s.p("models/m"));
        let profile = mycelium::ConfineDecl { read: vec![model.clone()], write: vec![out.clone()], ..Default::default() };
        let (w, health, sink) = activate_watched(confined(sh("true"), sh(&format!("cat {model} > {out}/probe")), profile), &s.placed);
        assert!(reprobe_one(&w), "the plant: an unchanged profile re-probes healthy");

        // The write directory rotated: a new one renamed into place, the old one moved aside.
        std::fs::create_dir(s.root.join("out.new")).unwrap();
        std::fs::rename(&out, s.root.join("out.old")).unwrap();
        std::fs::rename(s.root.join("out.new"), &out).unwrap();
        // The read file replaced by an atomic rename.
        std::fs::write(s.root.join("models/m.tmp"), b"a new model").unwrap();
        std::fs::rename(s.root.join("models/m.tmp"), &model).unwrap();

        assert!(reprobe_one(&w), "a legitimately replaced path is re-resolved, not a failed probe: {:?}", probe_records(&sink));
        assert!(health.load(Ordering::Relaxed), "the install stays live");
        assert_eq!(std::fs::read(s.root.join("out/probe")).unwrap(), b"a new model", "the probe ran under the fresh profile, writing the new directory");
        assert!(reprobe_one(&w), "and the next re-probe uses the re-resolved profile");
        assert_eq!(probe_records(&sink), [], "no failed probe recorded");
    }

    /// A declared path replaced by a symlink is refused at the re-probe's re-resolution, never followed:
    /// the install is withdrawn and the refusal named in the trace. Seen failing first: the probe failed
    /// as `probe_failed`, the symlink unnamed.
    #[test]
    fn a_declared_path_replaced_by_a_symlink_is_refused_at_the_reprobe_by_name() {
        const T: &str = "a_declared_path_replaced_by_a_symlink_is_refused_at_the_reprobe_by_name";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("relink");
        let out = s.p("out");
        let profile = mycelium::ConfineDecl { write: vec![out.clone()], ..Default::default() };
        let (w, health, sink) = activate_watched(confined(sh("true"), sh(&format!("echo ok > {out}/probe")), profile), &s.placed);
        std::fs::rename(&out, s.root.join("out.old")).unwrap();
        std::os::unix::fs::symlink(s.root.join("outside"), &out).unwrap();

        assert!(!reprobe_one(&w), "a declared path that is now a symlink is refused");
        assert!(!health.load(Ordering::Relaxed), "the install is withdrawn");
        assert!(!s.root.join("outside/probe").exists(), "the symlink was not followed");
        let recs = probe_records(&sink);
        assert_eq!(recs.len(), 1, "{recs:?}");
        let (_, reason, effect) = &recs[0];
        assert_eq!(reason, "confinement_failed", "the refusal is named: {recs:?}");
        assert!(effect.contains("symlink") && effect.contains(&out), "the effect names the path and why: {effect}");
    }

    /// A declared path swapped under the run — after the currency check — fails closed: the verdict of a
    /// probe whose profile changed while it ran is not trusted, the profile is re-resolved and the probe
    /// run once more, and a second change refuses by name. Shown with a probe that swaps a declared read
    /// path inside its write root on every run, the deterministic form of the race. Seen failing first:
    /// the probe passed and the install stayed live on a profile that no longer named its paths.
    #[test]
    fn a_declared_path_swapped_during_the_probe_fails_closed() {
        const T: &str = "a_declared_path_swapped_during_the_probe_fails_closed";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("swap");
        let out = s.p("out");
        std::fs::create_dir(s.root.join("out/models")).unwrap();
        let profile = mycelium::ConfineDecl { read: vec![format!("{out}/models")], write: vec![out.clone()], ..Default::default() };
        let (w, health, sink) = activate_watched(confined(sh("true"), sh("true"), profile), &s.placed);
        let mut w = w;
        w.decl.probe = sh(&format!("mv {out}/models {out}/models.$$ && mkdir {out}/models"));

        assert!(!reprobe_one(&w), "a profile that changed under each run is refused, not trusted");
        assert!(!health.load(Ordering::Relaxed));
        let recs = probe_records(&sink);
        assert_eq!(recs.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["confinement_changed"], "{recs:?}");
    }

    /// Review of #641, finding 4: the activation's **initial** probe takes the same both-sides currency
    /// check as a re-probe — a declared path that changes under it fails the activation as
    /// `confinement_changed`, never a live install on a profile that no longer names its paths. Seen
    /// failing first: the activation succeeded.
    #[test]
    fn a_declared_path_swapped_during_the_initial_probe_fails_the_activation() {
        const T: &str = "a_declared_path_swapped_during_the_initial_probe_fails_the_activation";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        let s = Scene::new("swap0");
        let out = s.p("out");
        std::fs::create_dir(s.root.join("out/models")).unwrap();
        let profile = mycelium::ConfineDecl { read: vec![format!("{out}/models")], write: vec![out.clone()], ..Default::default() };
        let probe = sh(&format!("mv {out}/models {out}/models.$$ && mkdir {out}/models"));
        let (got, recs) = activate(confined(sh("true"), probe, profile), &s.placed);
        let e = got.expect_err("an initial probe under a profile that changed under it is not trusted");
        assert!(e.contains("changed"), "{e}");
        assert_eq!(recs.last().map(|r| (r.0.as_str(), r.1.as_str())), Some(("prov.probe", "confinement_changed")), "{recs:?}");
    }

    /// Review of #641, finding 7: a re-probe round runs its entries' probes concurrently, so one slow or
    /// churning entry (up to two probe timeouts) does not delay every other install's verdict. Seen failing
    /// first: the second entry's probe waited for the first's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_slow_reprobe_does_not_delay_the_others() {
        let s = Scene::new("concurrent");
        let marker = s.root.join("out/second");
        let unconfined = |name: &str, probe: Vec<String>| ActivationDecl {
            ns: "data".into(), name: name.into(), command: vec!["true".into()], probe, timeout_secs: Some(10),
            resolve_artifact_refs: false, confine: None,
        };
        let watch: WatchList = Default::default();
        let actx = ActivationCtx { install_token: 1, trace: None };
        // The slow probe passes at activation (the gate file is absent) and sleeps at the re-probe.
        let gate = s.root.join("out/slow");
        let h = hook(vec![unconfined("slow", sh(&format!("[ ! -e {g} ] || sleep 3", g = gate.display()))),
                          unconfined("fast", sh(&format!("[ ! -e {g} ] || touch {m}", g = gate.display(), m = marker.display())))], Arc::clone(&watch));
        let slow = crate::InstallableEntry::new(mycelium::Capability::new("data", "slow"), crate::ArtifactId::from_bytes([1u8; 32]));
        let fast = crate::InstallableEntry::new(mycelium::Capability::new("data", "fast"), crate::ArtifactId::from_bytes([2u8; 32]));
        let _h1 = h(&slow, &s.placed, &actx).unwrap();
        let _h2 = h(&fast, &s.placed, &actx).unwrap();
        std::fs::write(&gate, b"").unwrap();
        let live = watch.lock().unwrap().clone();
        let started = Instant::now();
        let round = tokio::spawn(reprobe_round(live));
        while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let waited = started.elapsed();
        round.await.unwrap();
        assert!(marker.exists(), "the fast entry was re-probed");
        assert!(waited < Duration::from_millis(2500), "the fast entry's probe did not wait for the slow one: {waited:?}");
    }

    /// Review of #641 round 2, finding D: a round runs at most `REPROBE_CONCURRENCY` probes at once, so a
    /// stem hosting hundreds of installs does not fork them all at one instant. Each probe records how many
    /// were running when it started. Seen failing first: all twenty ran together.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reprobe_round_runs_a_bounded_number_of_probes_at_once() {
        let s = Scene::new("bounded");
        let (running, seen, gate) = (s.root.join("out/running"), s.root.join("out/seen"), s.root.join("out/gate"));
        std::fs::create_dir_all(&running).unwrap();
        std::fs::create_dir_all(&seen).unwrap();
        let n = 20;
        let decls: Vec<ActivationDecl> = (0..n)
            .map(|i| ActivationDecl {
                ns: "data".into(), name: format!("e{i}"), command: vec!["true".into()], timeout_secs: Some(10),
                probe: sh(&format!(
                    "[ ! -e {g} ] && exit 0; touch {r}/e{i}; ls {r} | wc -l > {s}/e{i}; sleep 0.4; rm {r}/e{i}",
                    g = gate.display(), r = running.display(), s = seen.display()
                )),
                resolve_artifact_refs: false, confine: None,
            })
            .collect();
        let watch: WatchList = Default::default();
        let actx = ActivationCtx { install_token: 1, trace: None };
        let h = hook(decls, Arc::clone(&watch));
        let healths: Vec<_> = (0..n)
            .map(|i| {
                let e = crate::InstallableEntry::new(mycelium::Capability::new("data", format!("e{i}").as_str()), crate::ArtifactId::from_bytes([i as u8; 32]));
                h(&e, &s.placed, &actx).unwrap()
            })
            .collect();
        std::fs::write(&gate, b"").unwrap();
        let live = watch.lock().unwrap().clone();
        reprobe_round(live).await;
        let peak = (0..n)
            .map(|i| std::fs::read_to_string(seen.join(format!("e{i}"))).unwrap().trim().parse::<usize>().unwrap())
            .max()
            .unwrap();
        drop(healths);
        assert!(peak <= REPROBE_CONCURRENCY, "at most {REPROBE_CONCURRENCY} probes at once, saw {peak}");
    }

    /// Review follow-up 1 (#636): the resolved profiles are bounded and forget a placed file that is gone
    /// (`BlobRuntime::uninstall` removes it), so the cache does not grow with every artifact ever placed.
    #[test]
    fn resolved_profiles_forget_a_removed_placement_and_are_bounded() {
        if !crate::activation_sandbox::native_backend_or_skip("resolved_profiles_forget_a_removed_placement_and_are_bounded") {
            return;
        }
        let s = Scene::new("cache");
        let cache = ResolvedProfiles::with_capacity(2);
        let c = mycelium::ConfineDecl::default();
        let mut placed = Vec::new();
        for i in 0..3 {
            let p = s.root.join(format!("placed/blob{i}"));
            std::fs::write(&p, b"x").unwrap();
            cache.get_or_resolve(0, &c, &p, None).unwrap();
            placed.push(p);
        }
        assert!(cache.len() <= 2, "bounded: {}", cache.len());
        assert!(cache.contains(0, &placed[2]), "the newest is kept");
        std::fs::remove_file(&placed[2]).unwrap();
        let next = s.root.join("placed/blob3");
        std::fs::write(&next, b"x").unwrap();
        cache.get_or_resolve(0, &c, &next, None).unwrap();
        assert!(!cache.contains(0, &placed[2]), "a removed placement is forgotten at the next resolution");
        assert!(cache.len() <= 2, "still bounded: {}", cache.len());
    }

    /// A command that genuinely hangs still times out, and is killed and reaped.
    #[test]
    fn a_hung_command_times_out() {
        let started = Instant::now();
        let err = run(&sh("sleep 30"), Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "killed at the deadline: {:?}", started.elapsed());
    }

    /// A hung activation is recorded as `command_timed_out`, the reason the rule catalogue lists. Found
    /// while enumerating row M's doors: the hook matched `"timed out"` against the runner's error, which
    /// says `did not finish`, so every timeout was recorded as `command_failed`. Seen failing first.
    #[test]
    fn a_hung_activation_is_recorded_as_a_timeout() {
        let decl = ActivationDecl {
            ns: "data".into(),
            name: "pack".into(),
            command: sh("sleep 30"),
            probe: Vec::new(),
            timeout_secs: Some(1),
            resolve_artifact_refs: false,
            confine: None,
        };
        let (got, recs) = activate(decl, Path::new("/nonexistent/pack"));
        assert!(got.is_err());
        assert_eq!(recs, [("prov.activation".to_string(), "command_timed_out".to_string())]);
    }

    // ── Post-360 rows L and M: the activation egress proxy ──────────────────────────────────────────

    /// A loopback upstream that records every request head and answers `200 ok`.
    async fn upstream() -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    let mut buf = vec![0u8; 16384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_string());
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
                });
            }
        });
        (port, seen)
    }

    /// What a confined command does through its proxy, in Python (no curl on every runner): `get URL
    /// [Header: value]…` through the environment's proxy (urllib reads `http_proxy`, its userinfo the
    /// token); `connect HOST PORT` — a `CONNECT` tunnel, then a request inside it; `direct HOST PORT` — a TCP
    /// connection past the proxy; `env FILE` — the whole environment, written out. Exit 0 on success.
    const EGRESS_PROBE: &str = r#"import os,sys,socket,base64,urllib.request,urllib.parse
mode=sys.argv[1]
try:
    if mode=='get':
        req=urllib.request.Request(sys.argv[2])
        for h in sys.argv[3:]:
            k,v=h.split(':',1); req.add_header(k.strip(),v.strip())
        r=urllib.request.urlopen(req,timeout=10); r.read()
        sys.exit(0 if r.status==200 else 4)
    elif mode=='connect':
        u=urllib.parse.urlsplit(os.environ['MYCELIUM_EGRESS_PROXY'])
        s=socket.create_connection((u.hostname,u.port),timeout=10)
        auth=base64.b64encode((u.username+':'+u.password).encode()).decode()
        s.sendall(('CONNECT %s:%s HTTP/1.1\r\nHost: %s:%s\r\nProxy-Authorization: Basic %s\r\n\r\n'%(sys.argv[2],sys.argv[3],sys.argv[2],sys.argv[3],auth)).encode())
        head=s.recv(4096)
        if not head.startswith(b'HTTP/1.1 200'):
            print(head,file=sys.stderr); sys.exit(3)
        s.sendall(b'GET /tunnelled HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n')
        data=b''
        while True:
            c=s.recv(4096)
            if not c: break
            data+=c
        sys.exit(0 if data.startswith(b'HTTP/1.1 200') else 5)
    elif mode=='direct':
        socket.create_connection((sys.argv[2],int(sys.argv[3])),timeout=5)
    elif mode=='env':
        open(sys.argv[2],'w').write('\n'.join(k+'='+v for k,v in os.environ.items()))
except Exception as e:
    print(e,file=sys.stderr); sys.exit(3)
"#;

    fn egress_probe(args: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = vec!["python3".into(), "-I".into(), "-S".into(), "-c".into(), EGRESS_PROBE.into()];
        v.extend(args.iter().map(|a| a.to_string()));
        v
    }

    fn proxied(egress: mycelium::EgressDecl, write: Vec<String>) -> mycelium::ConfineDecl {
        mycelium::ConfineDecl { read: vec![python_prefix()], write, egress: Some(egress), ..Default::default() }
    }

    fn proxy_for_test(label: &str, egress: &mycelium::EgressDecl) -> Arc<crate::EgressProxy> {
        Arc::new(
            crate::EgressProxy::start(crate::activation_egress::ProxyOptions {
                label: label.into(),
                decl: egress,
                node_egress: &mycelium::EgressPolicy::default(),
                bind: crate::EgressProxy::DEFAULT_BIND,
                limits: Default::default(),
                peer_uid: None,
                #[cfg(any(feature = "gateway", feature = "llm"))]
                gate: None,
            })
            .expect("the proxy starts"),
        )
    }

    type TestHook = Arc<dyn Fn(&InstallableEntry, &Path, &ActivationCtx) -> Result<Option<Arc<AtomicBool>>, String> + Send + Sync>;

    /// One activation run through a hook that holds `proxy` for declaration 0, off the runtime (the hook
    /// blocks; the proxy serves on the runtime's other threads).
    async fn activate_via(h: TestHook, placed: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || h(&pack_entry(), &placed, &ActivationCtx { install_token: 3, trace: None }).map(|_| ()))
            .await
            .unwrap()
    }

    fn egress_hook(command: Vec<String>, confine: mycelium::ConfineDecl, proxy: Arc<crate::EgressProxy>) -> TestHook {
        let proxies: EgressProxies = Arc::new([(0usize, Ok(proxy))].into_iter().collect());
        Arc::new(hook_with_egress(vec![confined(command, Vec::new(), confine)], Default::default(), proxies))
    }

    /// Rows L and M — **limits by hostname**: a confined command reaches an allowed host through its
    /// stem's egress proxy and is refused another — another port, and the same address under another name
    /// — while a connection past the proxy, even to the allowed upstream, is refused by the confinement
    /// itself. `CONNECT` tunnels to the allowed host and is refused elsewhere. Plants: unconfined, the
    /// direct connection succeeds, so its refusal is the profile's doing. Seen failing first with the proxy
    /// forwarding every host (`hosts` ignored): the other host was reached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_confined_command_reaches_an_allowed_host_through_its_proxy_and_is_refused_another() {
        const T: &str = "a_confined_command_reaches_an_allowed_host_through_its_proxy_and_is_refused_another";
        if !crate::activation_sandbox::native_backend_or_skip(T) || !python3_or_skip(T) {
            return;
        }
        let s = Scene::new("egress-host");
        let ((a, at_a), (b, at_b)) = (upstream().await, upstream().await);
        let egress = mycelium::EgressDecl { hosts: vec![format!("http://127.0.0.1:{a}")], ..Default::default() };
        let proxy = proxy_for_test("data/pack", &egress);
        let confine = proxied(egress, Vec::new());
        let run = |args: Vec<String>| activate_via(egress_hook(egress_probe(&args.iter().map(String::as_str).collect::<Vec<_>>()), confine.clone(), Arc::clone(&proxy)), s.placed.clone());

        assert_eq!(run(vec!["get".into(), format!("http://127.0.0.1:{a}/through")]).await, Ok(()), "the allowed host, through the proxy");
        assert!(at_a.lock().unwrap().iter().any(|h| h.starts_with("GET /through")), "{:?}", at_a.lock().unwrap());
        assert!(run(vec!["get".into(), format!("http://127.0.0.1:{b}/no")]).await.is_err(), "another host:port is refused");
        assert!(run(vec!["get".into(), format!("http://localhost:{a}/no")]).await.is_err(), "the same address under another name is refused: by name");
        assert!(at_b.lock().unwrap().is_empty(), "the refused host saw nothing");
        assert!(!at_a.lock().unwrap().iter().any(|h| h.starts_with("GET /no")), "nor did the allowed one, asked by another name");

        // CONNECT: a tunnel to the listed host:port; refused to another.
        assert_eq!(run(vec!["connect".into(), "127.0.0.1".into(), a.to_string()]).await, Ok(()), "a tunnel to the allowed host");
        assert!(at_a.lock().unwrap().iter().any(|h| h.starts_with("GET /tunnelled")));
        assert!(run(vec!["connect".into(), "127.0.0.1".into(), b.to_string()]).await.is_err(), "no tunnel to another");
        assert!(at_b.lock().unwrap().is_empty());

        // Past the proxy: refused by the confinement (on Seatbelt by address; on Landlock by port).
        assert_eq!(super::run(&egress_probe(&["direct", "127.0.0.1", &a.to_string()]), Duration::from_secs(10)), Ok(()), "plant: unconfined it connects");
        assert!(run(vec!["direct".into(), "127.0.0.1".into(), a.to_string()]).await.is_err(), "confined, only the proxy's port is open");
        let c = proxy.counts();
        assert!(c.forwarded >= 1 && c.tunnelled >= 1 && c.refused >= 3, "{c:?}");
    }

    /// Row M's live allow-list: the list changes while the stem runs — same proxy, same port, the same
    /// hook with its profile resolved once — and the next request meets the new list; a bad list is
    /// refused and the old one stands. Seen failing first with `set_hosts` a no-op: the added host stayed
    /// refused.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_allow_list_change_takes_effect_without_a_restart() {
        const T: &str = "an_allow_list_change_takes_effect_without_a_restart";
        if !crate::activation_sandbox::native_backend_or_skip(T) || !python3_or_skip(T) {
            return;
        }
        let s = Scene::new("egress-live");
        let ((a, _), (b, at_b)) = (upstream().await, upstream().await);
        let egress = mycelium::EgressDecl { hosts: vec![format!("http://127.0.0.1:{a}")], ..Default::default() };
        let proxy = proxy_for_test("data/pack", &egress);
        let port = proxy.port();
        let to_b = format!("http://127.0.0.1:{b}/live");
        // One hook for every run, as one stem's provisioner holds it.
        let h = egress_hook(egress_probe(&["get", &to_b]), proxied(egress, Vec::new()), Arc::clone(&proxy));
        assert!(activate_via(Arc::clone(&h), s.placed.clone()).await.is_err(), "b is not listed yet");
        assert!(at_b.lock().unwrap().is_empty());

        proxy.set_hosts(&[format!("http://127.0.0.1:{a}"), format!("http://127.0.0.1:{b}")]).expect("a valid list");
        assert_eq!(activate_via(Arc::clone(&h), s.placed.clone()).await, Ok(()), "listed live: reached, no restart");
        assert_eq!(at_b.lock().unwrap().len(), 1);
        assert_eq!(proxy.port(), port, "the same proxy, the same port");

        let e = proxy.set_hosts(&["https://*.example.com".into()]).expect_err("a bad list is refused");
        assert!(e.contains("wildcard"), "{e}");
        assert_eq!(proxy.hosts(), [format!("http://127.0.0.1:{a}"), format!("http://127.0.0.1:{b}")], "and the old one stands");

        proxy.set_hosts(&[]).expect("an empty list");
        assert!(activate_via(h, s.placed.clone()).await.is_err(), "removed live: refused again");
        assert_eq!(at_b.lock().unwrap().len(), 1);
    }

    /// Row L's activation-command half: a placeholder is replaced **only at the bound host** — the node's
    /// gate attaches the value there; another listed host gets a refusal, not the value; a binding the
    /// activation was not given is refused — and **the value is never in the command's environment**: the
    /// command starts with a cleared environment, so the binding's `value_env` variable, set in the stem's,
    /// is not in it. Seen failing first with the proxy forwarding headers as written and the environment
    /// inherited: the upstream received the placeholder, and the command's environment held the variable.
    #[cfg(any(feature = "gateway", feature = "llm"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_placeholder_is_replaced_only_at_the_bound_host_and_the_value_is_never_in_the_commands_environment() {
        const T: &str = "a_placeholder_is_replaced_only_at_the_bound_host_and_the_value_is_never_in_the_commands_environment";
        if !crate::activation_sandbox::native_backend_or_skip(T) || !python3_or_skip(T) {
            return;
        }
        const VAR: &str = "MYCELIUM_TEST_ACT_EGRESS_KEY_7F";
        const SECRET: &str = "sk-act-egress-7f3a-never-in-the-env";
        // SAFETY: see `stem::tests::a_served_skill_reads_its_key_from_the_environment` — test code, a
        // variable unique to this test, written once; the residual is a libc `getenv` race on another test
        // thread, whose worst outcome is a crashed test run.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var(VAR, SECRET); }
        let s = Scene::new("egress-cred");
        let ((a, at_a), (b, at_b)) = (upstream().await, upstream().await);
        let bind = |name: &str, port: u16| mycelium::CredentialBinding {
            name: name.into(), host: "127.0.0.1".into(), port: Some(port), path_prefix: None,
            form: mycelium::CredentialForm::Bearer, header: None, username: None,
            value_env: Some(VAR.into()), value_file: None, allow_plaintext: false,
        };
        let gate = Arc::new(mycelium::EgressCredentials::load(&[bind("model", a), bind("other", b)], &mycelium::EgressPolicy::default()).expect("the gate loads"));
        let egress = mycelium::EgressDecl {
            hosts: vec![format!("http://127.0.0.1:{a}"), format!("http://127.0.0.1:{b}")],
            credentials: vec!["model".into()],
        };
        let proxy = Arc::new(
            crate::EgressProxy::start(crate::activation_egress::ProxyOptions {
                label: "data/pack".into(),
                decl: &egress,
                node_egress: &mycelium::EgressPolicy::default(),
                bind: crate::EgressProxy::DEFAULT_BIND,
                limits: Default::default(),
                peer_uid: None,
                gate: Some(Arc::clone(&gate)),
            })
            .expect("the proxy starts"),
        );
        let confine = proxied(egress, vec![s.p("out")]);
        let run = |args: Vec<String>| activate_via(egress_hook(egress_probe(&args.iter().map(String::as_str).collect::<Vec<_>>()), confine.clone(), Arc::clone(&proxy)), s.placed.clone());
        let bearer = |name: &str| format!("Authorization: Bearer {}", mycelium::credential_placeholder(name));

        assert_eq!(run(vec!["get".into(), format!("http://127.0.0.1:{a}/v1/chat"), bearer("model")]).await, Ok(()), "the bound host");
        let got = at_a.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert!(got[0].to_ascii_lowercase().contains(&format!("authorization: bearer {SECRET}")), "the value attached at the bound host: {}", got[0]);
        assert!(!got[0].contains("mycelium-credential:") && !got[0].to_ascii_lowercase().contains("proxy-authorization"), "nor the placeholder, nor the proxy token: {}", got[0]);

        assert!(run(vec!["get".into(), format!("http://127.0.0.1:{b}/v1/chat"), bearer("model")]).await.is_err(), "another listed host: the binding does not authorise it");
        assert!(run(vec!["get".into(), format!("http://127.0.0.1:{b}/v1/chat"), bearer("other")]).await.is_err(), "a binding this activation was not given");
        assert!(at_b.lock().unwrap().is_empty(), "the other host saw neither the value nor the request");
        assert_eq!(gate.uses().iter().map(|u| (u.name.as_str(), u.attached)).collect::<Vec<_>>(), [("model", 1), ("other", 0)]);

        // The command's whole environment: no value, no variable — the proxy's address, the base set.
        let dump = s.root.join("out/env");
        assert_eq!(run(vec!["env".into(), dump.display().to_string()]).await, Ok(()));
        let env = std::fs::read_to_string(&dump).unwrap();
        assert!(!env.contains(SECRET) && !env.contains(VAR), "the value and its variable are not in the command's environment:\n{env}");
        assert!(env.contains("HTTP_PROXY=http://mycelium:") && env.contains(&format!("@127.0.0.1:{}", proxy.port())), "{env}");
        assert!(std::env::var(VAR).is_ok(), "plant: the stem's own environment holds it");
    }

    /// #644's review, finding 1: **every** confined command starts with a cleared environment, egress table or
    /// not — a variable in the stem's environment (a binding's `value_env` would be one) is not in it, the base
    /// set and the declared `env` are. Seen failing first: the confined command's environment was the stem's.
    #[test]
    fn a_confined_activation_starts_with_a_cleared_environment() {
        const T: &str = "a_confined_activation_starts_with_a_cleared_environment";
        if !crate::activation_sandbox::native_backend_or_skip(T) {
            return;
        }
        // SAFETY: as `stem::tests::a_served_skill_reads_its_key_from_the_environment` — test code, unique
        // variables, each written once.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("MYCELIUM_TEST_STEM_ONLY_7F", "sk-stem-only-7f");
            std::env::set_var("MYCELIUM_TEST_PASSED_7F", "passed-7f");
        }
        let s = Scene::new("cleared");
        let dump = s.p("out/env");
        let profile = mycelium::ConfineDecl { write: vec![s.p("out")], env: vec!["MYCELIUM_TEST_PASSED_7F".into()], ..Default::default() };
        let got = activate(confined(sh(&format!("env > {dump}")), Vec::new(), profile), &s.placed).0;
        assert_eq!(got, Ok(()));
        let env = std::fs::read_to_string(&dump).unwrap();
        assert!(!env.contains("MYCELIUM_TEST_STEM_ONLY_7F") && !env.contains("sk-stem-only-7f"), "the stem's variable is not passed:\n{env}");
        assert!(env.contains("MYCELIUM_TEST_PASSED_7F=passed-7f") && env.contains("PATH="), "the declared and base variables are:\n{env}");
        assert!(std::env::var("MYCELIUM_TEST_STEM_ONLY_7F").is_ok(), "plant: the stem holds it");
    }

    /// No silent fallback: an activation whose egress proxy could not start — here its address is taken —
    /// is refused by name (`egress_proxy_unavailable`) and its command never runs; so is one whose proxy was
    /// never started (the plain `hook`). Runs on every platform: the refusal comes before any backend. Seen
    /// failing first with the hook ignoring the missing proxy: the command ran.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_proxy_that_cannot_start_refuses_the_activation() {
        let s = Scene::new("egress-down");
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let egress = mycelium::EgressDecl { hosts: vec!["https://api.example.com".into()], ..Default::default() };
        let failed = crate::EgressProxy::start(crate::activation_egress::ProxyOptions {
            label: "data/pack".into(),
            decl: &egress,
            node_egress: &mycelium::EgressPolicy::default(),
            bind: taken.local_addr().unwrap(),
            limits: Default::default(),
            peer_uid: None,
            #[cfg(any(feature = "gateway", feature = "llm"))]
            gate: None,
        })
        .expect_err("the address is in use");
        assert!(failed.contains("data/pack") && failed.contains("egress proxy could not start"), "{failed}");
        let ran = s.p("placed/ran");
        let decl = confined(sh(&format!("touch {ran}")), Vec::new(), mycelium::ConfineDecl { egress: Some(egress), ..Default::default() });
        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let actx = ActivationCtx { install_token: 5, trace: Some(Arc::new(TraceCtx { sink: Arc::clone(&sink), node: "n".into(), build: "t".into(), config_digest: "d".into(), profile: None })) };
        let proxies: EgressProxies = Arc::new([(0usize, Err(failed))].into_iter().collect());
        let (placed, d) = (s.placed.clone(), decl.clone());
        let e = tokio::task::spawn_blocking(move || hook_with_egress(vec![d], Default::default(), proxies)(&pack_entry(), &placed, &actx).map(|_| ()))
            .await
            .unwrap()
            .expect_err("refused");
        assert!(e.contains("data/pack") && e.contains("egress proxy") && e.contains("not run without it"), "{e}");
        assert!(!Path::new(&ran).exists(), "the command never ran");
        assert_eq!(sink.snapshot().into_iter().map(|r| r.reason).collect::<Vec<_>>(), ["egress_proxy_unavailable"]);
        // Never started (a hook without proxies): refused the same way.
        let (got, recs) = activate(decl, &s.placed);
        assert!(got.expect_err("refused").contains("never started"));
        assert_eq!(recs, [("prov.activation".to_string(), "egress_proxy_unavailable".to_string())]);
        assert!(!Path::new(&ran).exists());
        drop(taken);
    }
}
