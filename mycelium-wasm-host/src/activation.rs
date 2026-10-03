//! Declarative blob activation for a stem (design-time-tooling.md §16, X2): the unit file's
//! `[[activation]]` sections become [`BlobRuntime::with_entry_activation`] — after a blob that
//! provides `ns/name` is placed, run the declared command, and gate the capability on the declared
//! probe. The probe is re-run by a background task ([`spawn_reprobe`]) that flips a per-install
//! flag; the provisioner's probe only reads the flag, so a slow `ollama show` never runs under its
//! lock. A failed re-probe withdraws the install and the next round reinstalls — restart ≡
//! provisioning, exactly as for a wasm component whose serve task died.
//!
//! What this is not: a sandbox. The commands are the operator's, written in a reviewed unit file
//! and run with the stem's own privileges — the same trust as the stem's command line.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mycelium::ActivationDecl;

use crate::catalog::InstallableEntry;

/// One live activation the re-probe task keeps current. Lock-order table row 51: the list is a
/// leaf, taken by the activation (install task) to push and by the re-probe task to snapshot and
/// prune — never while any other lock is held, never across `await`.
#[derive(Clone)]
pub struct Watched {
    decl:   ActivationDecl,
    entry:  InstallableEntry,
    path:   PathBuf,
    health: Arc<AtomicBool>,
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
                return Err(format!("{cmd} did not finish within {}s", timeout.as_secs()));
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
) -> impl Fn(&InstallableEntry, &Path) -> Result<Option<Arc<AtomicBool>>, String> + Send + Sync + 'static {
    move |entry, path| {
        let Some(decl) = decls
            .iter()
            .find(|d| d.ns == entry.provides.namespace.as_ref() && d.name == entry.provides.name.as_ref())
        else {
            return Ok(None);
        };
        let rendered = if decl.resolve_artifact_refs { Some(render(path)?) } else { None };
        let timeout = decl.timeout_secs.map(Duration::from_secs).unwrap_or(DEFAULT_TIMEOUT);
        run(&expand(&decl.command, entry, path, rendered.as_deref()), timeout)?;
        if decl.probe.is_empty() {
            return Ok(None);
        }
        // The probe gates the capability (D21): a failing initial probe is an activation error, so the
        // install fails at stage `activation`, nothing is advertised, and the next round retries —
        // never a live install that the health pass withdraws a round later.
        run(&expand(&decl.probe, entry, path, rendered.as_deref()), PROBE_TIMEOUT)
            .map_err(|e| format!("initial probe failed after activation: {e}"))?;
        let health = Arc::new(AtomicBool::new(true));
        watch.lock().unwrap_or_else(|e| e.into_inner()).push(Watched {
            decl: decl.clone(),
            entry: entry.clone(),
            path: path.to_path_buf(),
            health: Arc::clone(&health),
        });
        Ok(Some(health))
    }
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
                    let ok = run(&expand(&w.decl.probe, &w.entry, &w.path, rendered.as_deref()), PROBE_TIMEOUT).is_ok();
                    w.health.store(ok, Ordering::Relaxed);
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
        };
        let entry = crate::InstallableEntry::new(mycelium::Capability::new("data", "pack"), crate::ArtifactId::from_bytes([7u8; 32]));
        let watch: WatchList = Default::default();
        let got = hook(vec![decl], Arc::clone(&watch))(&entry, &path);
        match got {
            Err(e) => assert!(e.contains("probe"), "names the probe: {e}"),
            Ok(h) => panic!("a failing initial probe must be an activation error, got Ok({:?})", h.map(|h| h.load(std::sync::atomic::Ordering::Relaxed))),
        }
        assert!(watch.lock().unwrap().is_empty(), "nothing is watched for a capability that never went live");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command that genuinely hangs still times out, and is killed and reaped.
    #[test]
    fn a_hung_command_times_out() {
        let started = Instant::now();
        let err = run(&sh("sleep 30"), Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "killed at the deadline: {:?}", started.elapsed());
    }
}
