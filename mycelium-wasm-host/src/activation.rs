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
//! is resolved once, when the activation runs, and its re-probes reuse it.

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
    /// The confinement the activation ran under, reused by every re-probe (row M: fixed for its life).
    profile: Option<Profile>,
    /// The install's token and trace, so a re-probe's verdict records under the same token.
    token:  u64,
    trace:  Option<Arc<TraceCtx>>,
}

/// The re-probe list, shared by the activation hook and the re-probe task.
pub type WatchList = Arc<Mutex<Vec<Watched>>>;

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
    let (cmd, args) = argv.split_first().ok_or("empty command")?;
    let mut child = std::process::Command::new(cmd)
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

/// The hook a stem gives its [`BlobRuntime`](crate::BlobRuntime): dispatch by the entry's
/// capability to its `[[activation]]`; no matching section means placement is the whole install.
pub fn hook(
    decls: Vec<ActivationDecl>,
    watch: WatchList,
) -> impl Fn(&InstallableEntry, &Path, &ActivationCtx) -> Result<Option<Arc<AtomicBool>>, String> + Send + Sync + 'static {
    // Review finding 5 (#636): each activation's profile is resolved once — per declaration and placed
    // file — and kept for the stem's life, its paths held open, re-probes included. Lock-order table row
    // 61: a leaf, held for a lookup or an insert, never while a command runs.
    let resolved: Arc<Mutex<std::collections::HashMap<(usize, PathBuf), Profile>>> = Default::default();
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
        // nothing runs, not even the render. Resolved once, here; the re-probes reuse it.
        let profile = match &decl.confine {
            None => None,
            Some(c) => match resolved_profile(&resolved, index, c, path) {
                Ok(p) => Some(p),
                Err(refusal) => {
                    rec("prov.activation", OutcomeKind::Refusal, refusal.reason(), None);
                    return Err(format!("activation {target}: its [activation.confine] profile cannot be applied — {refusal}; refused, not run unconfined"));
                }
            },
        };
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
        if let Err(e) = run_under(&expand(&decl.probe, entry, path, rendered.as_deref()), PROBE_TIMEOUT, profile.as_ref()) {
            rec("prov.probe", OutcomeKind::Refusal, "initial_probe_failed", None);
            return Err(format!("initial probe failed after activation: {e}"));
        }
        rec("prov.probe", OutcomeKind::Action, "healthy", Some("health flag set; re-probed on the stem's tick".into()));
        let health = Arc::new(AtomicBool::new(true));
        watch.lock().unwrap_or_else(|e| e.into_inner()).push(Watched {
            decl: decl.clone(),
            entry: entry.clone(),
            path: path.to_path_buf(),
            health: Arc::clone(&health),
            profile,
            token: actx.install_token,
            trace: actx.trace.clone(),
        });
        Ok(Some(health))
    }
}

/// The profile resolved for `(index, placed)` the first time, or resolve and keep it now.
fn resolved_profile(
    resolved: &Mutex<std::collections::HashMap<(usize, PathBuf), Profile>>,
    index: usize,
    c: &mycelium::ConfineDecl,
    placed: &Path,
) -> Result<Profile, crate::activation_sandbox::Refusal> {
    let key = (index, placed.to_path_buf());
    if let Some(p) = resolved.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(p.clone());
    }
    // Resolved outside the lock (it reads the filesystem); a racing first resolution keeps the earlier.
    let fresh = crate::activation_sandbox::prepare(c, placed)?;
    Ok(resolved.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_insert(fresh).clone())
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
            for w in live {
                let ok = tokio::task::spawn_blocking(move || {
                    let rendered = w.decl.resolve_artifact_refs.then(|| PathBuf::from(format!("{}.rendered", w.path.display())));
                    let was = w.health.load(Ordering::Relaxed);
                    let ok = run_under(&expand(&w.decl.probe, &w.entry, &w.path, rendered.as_deref()), PROBE_TIMEOUT, w.profile.as_ref()).is_ok();
                    w.health.store(ok, Ordering::Relaxed);
                    // A re-probe records only a **change** of verdict (the steady state is the
                    // install's record), under the install's token.
                    if ok != was && let Some(t) = &w.trace {
                        let target = format!("{}/{}", w.entry.provides.namespace, w.entry.provides.name);
                        let (outcome, reason) = if ok { (OutcomeKind::Action, "healthy") } else { (OutcomeKind::Refusal, "probe_failed") };
                        t.record("prov.probe", format!("install token {}", w.token), outcome, reason, target, Vec::new(),
                            Some(if ok { "health flag set".into() } else { "health flag cleared; the next health pass withdraws".into() }));
                    }
                    ok
                })
                .await
                .unwrap_or(false);
                if !ok {
                    tracing::warn!("activation probe failed — the provisioner will withdraw and reinstall");
                }
            }
        }
    })
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
}
