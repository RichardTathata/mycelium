//! The failure bundle — what a failing run leaves behind and what a reviewer replays.
//!
//! Layout fixed by PR 1 (`docs/design/replay-nondeterminism-inventory.md` §5):
//!
//! ```text
//! bundle/
//!   build.json        # crate versions, git commit, features, target triple, rustc
//!   config.json       # every GossipConfig field per node (secrets replaced by placeholders)
//!   initial/          # disk images per node, or the fixture ids they were built from
//!   inputs/           # every external input, in arrival order, redacted
//!   choices.trace     # the ordered choice log — the reproduction itself
//!   witness.json      # the assertion that failed, and the toggle that must make it fail again
//! ```
//!
//! # The witness is the part people skip
//!
//! A bundle that replays but cannot *fail* proves nothing: you have reproduced a run in which
//! everything went fine. The witness names the assertion **and the `cfg(test)` toggle that must
//! make it fail again** — because the 2026-09-05 snapshot fix was verified by hand-disabling the
//! merge, and a fix whose proof is a manual edit is a fix nobody can re-check.
//!
//! # Secrets
//!
//! `config.json` carries every field with secrets replaced by placeholders, and `inputs/` is
//! redacted (threat model §6). A bundle travels to whoever is debugging; a bundle carrying a JWT
//! signing key is an incident, not an artefact.

use crate::trace::{Trace, TraceError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The assertion that failed, and how to make it fail again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    /// What failed, in words a reviewer can search for — usually the test name.
    pub assertion: String,
    /// The `cfg(test)` toggle that must be set for the failure to reappear, if any.
    ///
    /// `None` means the failure needs no toggle. It does **not** mean "we did not check": a bundle
    /// whose witness is unknown is a bundle that cannot prove its own fix.
    pub toggle: Option<String>,
}

/// Build identity — what the recording ran on.
///
/// Recorded because *exact* replay is only meaningful against a pinned build. A trace replayed on a
/// different commit is a **scenario** replay, which is a different and weaker claim, and the
/// difference has to be visible rather than assumed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Build {
    /// The commit the recording ran at.
    pub commit: String,
    /// The crate version.
    pub version: String,
    /// Cargo features enabled.
    pub features: Vec<String>,
    /// Target triple.
    pub target: String,
    /// The compiler.
    pub rustc: String,
}

impl Build {
    /// This build's identity, as far as it is honestly known at compile time: the crate version,
    /// the commit when the build ran under GitHub Actions (`GITHUB_SHA` is in the compiler's
    /// environment there; `unknown` elsewhere), the architecture and OS, and the features the
    /// caller names. The compiler version is not known without a build script, and is not
    /// guessed. A corpus entry (item 6 PR 7) carries this so a reviewer can tell an exact replay
    /// from a scenario replay.
    pub fn current(features: &[&str]) -> Self {
        Self {
            commit:   option_env!("GITHUB_SHA").unwrap_or("unknown").to_string(),
            version:  env!("CARGO_PKG_VERSION").to_string(),
            features: features.iter().map(|f| f.to_string()).collect(),
            target:   format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            rustc:    "unknown".to_string(),
        }
    }
}

/// A recorded run, complete enough to reproduce exactly.
#[derive(Clone, Debug, Default)]
pub struct Bundle {
    /// What it ran on.
    pub build: Build,
    /// Per-node configuration, secrets already replaced.
    pub config: BTreeMap<String, String>,
    /// Per-node initial disk images, or the fixture ids they were built from.
    pub initial: BTreeMap<String, Vec<u8>>,
    /// External inputs in arrival order, redacted.
    pub inputs: Vec<(String, Vec<u8>)>,
    /// The reproduction itself.
    pub trace: Trace,
    /// What failed, and the toggle that makes it fail again.
    pub witness: Option<Witness>,
}

impl Bundle {
    /// A bundle around `trace`.
    pub fn new(trace: Trace) -> Self {
        Self { trace, ..Default::default() }
    }

    /// Name the assertion that failed and the toggle that reproduces it.
    pub fn witnessed_by(mut self, assertion: impl Into<String>, toggle: Option<String>) -> Self {
        self.witness = Some(Witness { assertion: assertion.into(), toggle });
        self
    }

    /// Is this bundle able to demonstrate the failure it was written for?
    ///
    /// A bundle without a witness replays a run in which nothing went wrong — reproducing it proves
    /// only that the harness works.
    pub fn can_prove_its_failure(&self) -> bool {
        self.witness.is_some()
    }

    /// Write the bundle to `dir`.
    pub fn write(&self, dir: &Path) -> Result<(), BundleError> {
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(dir.join("initial"))?;
        std::fs::create_dir_all(dir.join("inputs"))?;

        std::fs::write(dir.join("build.json"), json_object(&build_fields(&self.build)))?;
        std::fs::write(dir.join("config.json"), json_object(&self.config))?;
        std::fs::write(dir.join("choices.trace"), self.trace.to_text())?;

        for (node, image) in &self.initial {
            std::fs::write(dir.join("initial").join(node), image)?;
        }
        for (i, (name, bytes)) in self.inputs.iter().enumerate() {
            // Numbered, because arrival order is part of the reproduction.
            std::fs::write(dir.join("inputs").join(format!("{i:04}-{name}")), bytes)?;
        }
        if let Some(w) = &self.witness {
            let mut fields = BTreeMap::new();
            fields.insert("assertion".to_string(), w.assertion.clone());
            fields.insert("toggle".to_string(), w.toggle.clone().unwrap_or_default());
            std::fs::write(dir.join("witness.json"), json_object(&fields))?;
        }
        Ok(())
    }

    /// Read back the parts a replay needs: the trace, the build, the witness.
    ///
    /// Deliberately partial — `initial/` and `inputs/` are handed to the scenario that knows what
    /// they mean, not interpreted here.
    pub fn read(dir: &Path) -> Result<Self, BundleError> {
        let trace = Trace::parse(&std::fs::read_to_string(dir.join("choices.trace"))?)?;
        let build = parse_build(&read_optional(&dir.join("build.json"))?);
        let witness = {
            let text = read_optional(&dir.join("witness.json"))?;
            let fields = parse_object(&text);
            fields.get("assertion").filter(|a| !a.is_empty()).map(|a| Witness {
                assertion: a.clone(),
                toggle:    fields.get("toggle").filter(|t| !t.is_empty()).cloned(),
            })
        };
        let config = parse_object(&read_optional(&dir.join("config.json"))?);
        Ok(Self { build, config, trace, witness, ..Default::default() })
    }

    /// Where a bundle for `name` conventionally lives under `root`.
    pub fn path_for(root: &Path, name: &str) -> PathBuf {
        root.join(name)
    }
}

/// Why a bundle could not be read or written.
#[derive(Debug)]
pub enum BundleError {
    /// The filesystem said no.
    Io(std::io::Error),
    /// The trace inside it was malformed.
    Trace(TraceError),
}

impl From<std::io::Error> for BundleError {
    fn from(e: std::io::Error) -> Self {
        BundleError::Io(e)
    }
}
impl From<TraceError> for BundleError {
    fn from(e: TraceError) -> Self {
        BundleError::Trace(e)
    }
}
impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleError::Io(e) => write!(f, "bundle io: {e}"),
            BundleError::Trace(e) => write!(f, "bundle trace: {e}"),
        }
    }
}
impl std::error::Error for BundleError {}

fn build_fields(b: &Build) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("commit".into(), b.commit.clone());
    m.insert("version".into(), b.version.clone());
    m.insert("features".into(), b.features.join(","));
    m.insert("target".into(), b.target.clone());
    m.insert("rustc".into(), b.rustc.clone());
    m
}

fn parse_build(text: &str) -> Build {
    let m = parse_object(text);
    let get = |k: &str| m.get(k).cloned().unwrap_or_default();
    Build {
        commit:   get("commit"),
        version:  get("version"),
        features: get("features").split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
        target:   get("target"),
        rustc:    get("rustc"),
    }
}

fn read_optional(p: &Path) -> Result<String, std::io::Error> {
    match std::fs::read_to_string(p) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// A flat string→string JSON object. Flat on purpose: a bundle's metadata is read by people and by
/// `jq`, and a schema nobody can eyeball is a schema nobody checks.
fn json_object(fields: &BTreeMap<String, String>) -> String {
    let body: Vec<String> =
        fields.iter().map(|(k, v)| format!("  {}: {}", quote(k), quote(v))).collect();
    format!("{{\n{}\n}}\n", body.join(",\n"))
}

fn parse_object(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches(',');
        let Some((k, v)) = line.split_once(business_separator()) else { continue };
        let (k, v) = (unquote(k.trim()), unquote(v.trim()));
        if !k.is_empty() {
            out.insert(k, v);
        }
    }
    out
}

fn business_separator() -> &'static str {
    "\": \""
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').replace("\\\"", "\"").replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{Choice, ChoiceKind};

    fn trace() -> Trace {
        let mut t = Trace::new();
        t.push(Choice {
            seq:     0,
            node:    Some("n1".into()),
            kind:    ChoiceKind::Fs,
            stream:  "wal.bin".into(),
            request: "append d=sha256:4f1c len=214 sync=true off=8192".into(),
            result:  "Ok(214)".into(),
        });
        t
    }

    fn tmp(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "sim-bundle-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_bundle_round_trips_its_trace_build_and_witness() {
        let dir = tmp("roundtrip");
        let mut b = Bundle::new(trace())
            .witnessed_by("snapshot_install_syncs_the_directory", Some("sim_disable_merge".into()));
        b.build = Build {
            commit:   "abc123".into(),
            version:  "2.7.0".into(),
            features: vec!["tls".into(), "compliance".into()],
            target:   "aarch64-apple-darwin".into(),
            rustc:    "1.88.0".into(),
        };
        b.config.insert("n1".into(), "bind_port=7000 tls=<placeholder>".into());
        b.write(&dir).expect("write");

        let read = Bundle::read(&dir).expect("read");
        assert_eq!(read.trace, b.trace);
        assert_eq!(read.build, b.build);
        assert_eq!(read.witness, b.witness);
        assert_eq!(read.config.get("n1").map(String::as_str), Some("bind_port=7000 tls=<placeholder>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bundle that replays but cannot *fail* reproduces a run in which nothing went wrong.
    #[test]
    fn a_bundle_without_a_witness_cannot_prove_its_failure() {
        assert!(!Bundle::new(trace()).can_prove_its_failure());
        assert!(Bundle::new(trace()).witnessed_by("x", None).can_prove_its_failure());
    }

    /// The toggle is what stops a fix being verified by hand-editing the source — which is how the
    /// 2026-09-05 snapshot fix was checked, and why it could not be re-checked afterwards.
    #[test]
    fn the_witness_carries_the_toggle_that_makes_the_failure_reappear() {
        let dir = tmp("witness");
        Bundle::new(trace())
            .witnessed_by("wal_tail_merge", Some("sim_disable_merge".into()))
            .write(&dir)
            .expect("write");
        let read = Bundle::read(&dir).expect("read");
        assert_eq!(read.witness.unwrap().toggle.as_deref(), Some("sim_disable_merge"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inputs_keep_their_arrival_order_in_the_filename() {
        let dir = tmp("inputs");
        let mut b = Bundle::new(trace());
        b.inputs.push(("oidc-verify".into(), b"<redacted>".to_vec()));
        b.inputs.push(("llm-reply".into(), b"<redacted>".to_vec()));
        b.write(&dir).expect("write");

        let mut names: Vec<String> = std::fs::read_dir(dir.join("inputs"))
            .expect("inputs")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["0000-oidc-verify", "0001-llm-reply"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_bundle_trace_is_an_error_not_an_empty_run() {
        let dir = tmp("missing");
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(Bundle::read(&dir).is_err(), "no trace is not the same as no decisions");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
