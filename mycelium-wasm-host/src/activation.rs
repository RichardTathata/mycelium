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

/// Run `argv` to completion within `timeout`; `Err` names the command and why.
pub fn run(argv: &[String], timeout: Duration) -> Result<(), String> {
    let (cmd, args) = argv.split_first().ok_or("empty command")?;
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|e| format!("{cmd}: {e}"))? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
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
        let healthy = run(&expand(&decl.probe, entry, path, rendered.as_deref()), PROBE_TIMEOUT).is_ok();
        let health = Arc::new(AtomicBool::new(healthy));
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
