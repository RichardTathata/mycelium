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

/// Fuzz hook (§12.6): **write-then-read fidelity** — what [`json_object`] writes, [`parse_object`]
/// must read back unchanged.
///
/// This is the invariant that matters, and it is strictly stronger than round-tripping arbitrary
/// *text*: `parse_object` is lossy in a stable way, so `parse → write → parse` holds even where a
/// field was already mangled. Starting from the map catches exactly what the text form hides. Two
/// defects were found this way and fixed in [`quote`] / [`unquote`].
#[doc(hidden)]
pub fn fuzz_write_read(m: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    parse_object(&json_object(m))
}

/// Fuzz entry point: build an object out of the fuzzer's bytes and assert it survives a write and
/// a read. Keys and values are taken as alternating newline-separated chunks, so the fuzzer
/// controls both sides of every pair.
#[doc(hidden)]
pub fn fuzz_object_fidelity(text: &str) -> bool {
    let mut m = BTreeMap::new();
    let mut chunks = text.split('\u{1}');
    while let (Some(k), Some(v)) = (chunks.next(), chunks.next()) {
        // An empty key is not representable in the flat form and `parse_object` drops it by
        // design, so it is out of scope for a fidelity claim.
        if !k.is_empty() {
            m.insert(k.to_string(), v.to_string());
        }
    }
    if m.is_empty() {
        return false;
    }
    let back = fuzz_write_read(&m);
    assert_eq!(m, back, "a bundle object did not survive being written and read back");
    true
}

/// Fuzz hook (§12.6): the bundle's hand-rolled JSON reader, round-tripped against its writer.
///
/// A reproduction bundle is the artefact that travels with a bug report, so in practice these are
/// third-party bytes. `Bundle::read` parses `build.json`, `config.json` and `witness.json` with
/// this reader and never verifies anything. The stake is `Build.commit` and `Build.version` — the
/// fields a replay uses to decide it is replaying against the same binary.
///
/// The invariant is that what [`json_object`] writes, this reads back unchanged.
#[doc(hidden)]
pub fn fuzz_object_roundtrip(text: &str) -> bool {
    let parsed = parse_object(text);
    if parsed.is_empty() {
        return false;
    }
    let reparsed = parse_object(&json_object(&parsed));
    assert_eq!(parsed, reparsed, "a bundle object did not survive its own round trip");
    true
}

/// Fuzz hook (§12.6): as [`fuzz_object_roundtrip`], for the `build.json` reader.
#[doc(hidden)]
pub fn fuzz_build_roundtrip(text: &str) -> bool {
    let build = parse_build(text);
    let reparsed = parse_build(&json_object(&build_fields(&build)));
    assert_eq!(build, reparsed, "a build record did not survive its own round trip");
    true
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

/// Escape one field for the flat object form.
///
/// Newlines are escaped because the reader is **line-oriented** — `parse_object` iterates
/// `text.lines()`, so a literal newline in a value truncated it at the newline and silently dropped
/// the rest. A `witness.assertion` is free text, so this was reachable: a replay would check a
/// shorter, weaker assertion than the one recorded and report success. Found by the §12.6
/// trust-edge fuzz work.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The inverse of [`quote`].
///
/// Strips **exactly one** surrounding quote rather than `trim_matches('"')`, which stripped every
/// trailing quote — so a value ending in an escaped quote came back with the quote gone and its
/// backslash left behind (`he said "hi"` read back as `he said "hi\`). Unescaping is a single
/// left-to-right pass rather than two chained `replace`s, which decoded `\\"` as an escaped quote
/// instead of a backslash followed by a quote.
fn unquote(s: &str) -> String {
    // At most one quote from each end, stripped independently: `parse_object` splits a line in
    // the middle of the pair, so a key arrives with only its opening quote and a value with only
    // its closing one. `trim_matches` removed every trailing quote, which is how a value ending in
    // an escaped quote lost it.
    let inner = s.strip_prefix('"').unwrap_or(s);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            // An escape this writer never emits is kept verbatim rather than swallowed: a byte
            // dropped here is a field that reads back shorter than it was written.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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


#[cfg(test)]
mod fidelity_tests {
    use super::*;

    /// What the bundle writer writes, the bundle reader reads back — including the two shapes that
    /// used to be silently corrupted.
    ///
    /// Both were found by the §12.6 trust-edge fuzz work. They matter because a bundle is the
    /// artefact that travels with a bug report: `Build.commit` decides whether a replay is against
    /// the same binary at all, and `witness.assertion` is the free text a replay checks. A field
    /// that reads back shorter than it was written makes a replay check something weaker than what
    /// was recorded and then report success — a silent divergence, in the crate whose whole purpose
    /// is to make divergence loud.
    ///
    /// What this does NOT prove: nothing here says the flat form is a correct JSON encoder. It is
    /// deliberately not one — it is a format two functions in this file agree on, and this pins
    /// that agreement.
    #[test]
    fn a_bundle_field_survives_being_written_and_read_back() {
        let cases: &[(&str, &str)] = &[
            ("commit", "abc123"),
            // The `trim_matches('"')` defect: a value ending in an escaped quote came back as
            // `he said "hi\` — quote gone, its backslash left behind.
            ("quote", "he said \"hi\""),
            ("trail", "v\""),
            ("lead", "\"v"),
            // The line-oriented defect: a value containing a newline was truncated at it and the
            // rest was dropped without a word.
            ("newline", "a\nb"),
            ("carriage", "a\rb"),
            ("assertion", "the depot never double-books a pallet\nand never holds one past its window"),
            ("colon", "a: b"),
            ("sep", "a\": \"b"),
            ("backslash", "a\\b"),
            ("escaped", "a\\\"b"),
            ("empty", ""),
        ];
        for (k, v) in cases {
            let mut m = BTreeMap::new();
            m.insert((*k).to_string(), (*v).to_string());
            assert_eq!(
                m,
                fuzz_write_read(&m),
                "field {k:?} did not survive being written and read back",
            );
        }
    }

    /// The same claim for a whole `Build`, which is the record a replay checks its binary against.
    #[test]
    fn a_build_record_survives_being_written_and_read_back() {
        let b = Build {
            commit:   "deadbeef".into(),
            version:  "2.9.1".into(),
            features: vec!["sim".into(), "tls".into()],
            target:   "aarch64-apple-darwin".into(),
            rustc:    "rustc 1.88.0 (\"stable\")".into(),
        };
        assert_eq!(b, parse_build(&json_object(&build_fields(&b))));
    }
}
