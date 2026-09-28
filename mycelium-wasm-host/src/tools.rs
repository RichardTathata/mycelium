//! The artifact tool — `mycelium-artifact publish | list | verify`
//! (`docs/plans/design-time-tooling.md` §10, D10 and A1).
//!
//! The signed line-hex **manifest stays the library's truth**; a reviewable **description**
//! (`artifacts/*.toml`, the same TOML `mycelium wire-check --library` reads) is the input, and this
//! tool derives the one from the other:
//!
//! - [`publish`] stores the description's bytes in the library (content-addressed), builds the
//!   entry — kind, the capability it provides, cost hints, the **signed** footprint — signs it
//!   with the publisher's key, and appends the manifest line. A running librarian picks the
//!   change up on its next reconcile pass.
//! - [`list`] renders a manifest back into the description shape, with the content address and
//!   the signer shown, so a reviewer can read what a line declares.
//! - [`verify`] checks every manifest line: provenance against the trusted publisher keys, the
//!   bytes present in the library and hashing to the entry's address, and — given the
//!   descriptions directory — that each description's bytes still hash to the address the
//!   manifest carries for its capability (a description whose bytes changed under an unchanged
//!   manifest is named).
//!
//! Keys: a publisher's signing key is a 32-byte Ed25519 seed as 64 hex characters (the form the
//! repository's federation fixtures use); [`signing_key_from_hex`] parses it. In production the
//! seed comes from a secret store, never from a unit file (plan D7).

use std::path::Path;

use ed25519_dalek::SigningKey;
use mycelium::wire_check::ArtifactDescription;
use mycelium::{CapValue, Capability};

use crate::artifact::{verify_artifact, ArtifactId, ArtifactKind, ArtifactSource, FsLibrarySource};
use crate::catalog::{InstallableEntry, Manifest, MANIFEST_FILE};

/// The artifact kinds by the names a description uses.
pub fn kind_from_name(name: &str) -> Option<ArtifactKind> {
    match name {
        "wasm-component" => Some(ArtifactKind::WasmComponent),
        "blob" => Some(ArtifactKind::Blob),
        _ => None,
    }
}

/// The description name of a kind.
pub fn kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::WasmComponent => "wasm-component",
        ArtifactKind::Blob => "blob",
    }
}

/// A 32-byte seed as 64 hex characters (surrounding whitespace ignored).
pub fn signing_key_from_hex(text: &str) -> Result<SigningKey, String> {
    let hex = text.trim();
    if hex.len() != 64 {
        return Err(format!("a signing key is 64 hex characters (a 32-byte seed); got {}", hex.len()));
    }
    let mut seed = [0u8; 32];
    for (i, b) in seed.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| "a signing key is hex".to_string())?;
    }
    Ok(SigningKey::from_bytes(&seed))
}

/// A verifying key as `ed25519:<64 hex>`.
pub fn publisher_from_str(s: &str) -> Result<[u8; 32], String> {
    let hex = s.strip_prefix("ed25519:").ok_or_else(|| format!("{s:?} must be ed25519:<64 hex>"))?;
    if hex.len() != 64 {
        return Err(format!("{s:?} must carry 64 hex characters"));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| format!("{s:?} is not hex"))?;
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What `publish` did.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub artifact:   ArtifactId,
    pub provides:   Capability,
    pub kind:       ArtifactKind,
    pub size_bytes: u64,
    pub signer:     String,
}

/// Store the description's bytes, build and sign the entry, append the manifest line.
pub fn publish(description: &Path, library: &Path, key: &SigningKey) -> Result<PublishOutcome, String> {
    let text = std::fs::read_to_string(description).map_err(|e| format!("{}: {e}", description.display()))?;
    let d = ArtifactDescription::from_toml_str(&text).map_err(|e| format!("{}: {e}", description.display()))?;
    let kind = kind_from_name(&d.kind).ok_or_else(|| format!("unknown artifact kind {:?}", d.kind))?;
    let bytes_rel = d.bytes.as_deref().ok_or_else(|| format!("{}: `bytes` is required to publish", description.display()))?;
    let bytes_path = description.parent().unwrap_or_else(|| Path::new(".")).join(bytes_rel);
    let bytes = std::fs::read(&bytes_path).map_err(|e| format!("{}: {e}", bytes_path.display()))?;
    let lib = FsLibrarySource::open(library).map_err(|e| format!("{}: {e}", library.display()))?;
    let artifact = lib.store(&bytes).map_err(|e| format!("store: {e}"))?;
    let provides = d.provides.to_capability();
    let entry = InstallableEntry::new(provides.clone(), artifact)
        .with_kind(kind)
        .with_cost(bytes.len() as u64, d.est_install_secs.unwrap_or(0))
        .with_requirements(d.requires.disk_bytes, d.requires.mem_bytes)
        .signed_by(key);
    Manifest::append_entry(&library.join(MANIFEST_FILE), entry).map_err(|e| format!("manifest: {e}"))?;
    Ok(PublishOutcome {
        artifact,
        provides,
        kind,
        size_bytes: bytes.len() as u64,
        signer: format!("ed25519:{}", hex(&key.verifying_key().to_bytes())),
    })
}

fn toml_value(v: &CapValue) -> String {
    match v {
        CapValue::Text(s) => format!("{:?}", s.as_ref()),
        CapValue::Integer(n) => n.to_string(),
        CapValue::Float(f) => format!("{f:?}"),
        CapValue::Bool(b) => b.to_string(),
        CapValue::Version([a, b, c]) => format!("{{ version = \"{a}.{b}.{c}\" }}"),
    }
}

/// Render one entry in the description shape, headed by what the description cannot carry:
/// the content address, the size, and the signer.
pub fn render_entry(e: &InstallableEntry) -> String {
    let mut out = String::new();
    let signer = if e.signer.is_empty() { "unsigned".to_string() } else { format!("ed25519:{}", hex(&e.signer)) };
    out.push_str(&format!("# artifact {} — {} B — {}\n", e.artifact.to_hex(), e.size_bytes, signer));
    out.push_str(&format!("kind = {:?}\n", kind_name(e.kind)));
    out.push_str(&format!("est_install_secs = {}\n", e.est_install_secs));
    out.push_str("\n[provides]\n");
    out.push_str(&format!("ns = {:?}\nname = {:?}\n", e.provides.namespace.as_ref(), e.provides.name.as_ref()));
    if let Some(sid) = &e.provides.schema_id {
        out.push_str(&format!("schema_id = {:?}\n", sid.as_ref()));
    }
    if !e.provides.attributes.is_empty() {
        out.push_str("\n[provides.attrs]\n");
        for (k, v) in &e.provides.attributes {
            out.push_str(&format!("{} = {}\n", k.as_ref(), toml_value(v)));
        }
    }
    out.push_str("\n[requires]\n");
    out.push_str(&format!("disk_bytes = {}\nmem_bytes = {}\n", e.requires.disk_bytes, e.requires.mem_bytes));
    out
}

/// The whole manifest in the description shape, one block per entry, blank-line separated.
pub fn list(library: &Path) -> Result<String, String> {
    let m = Manifest::load(&library.join(MANIFEST_FILE)).map_err(|e| format!("manifest: {e}"))?;
    Ok(m.entries().iter().map(render_entry).collect::<Vec<_>>().join("\n"))
}

/// What `verify` found: every problem names its line and its reason.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub checked:  usize,
    pub problems: Vec<String>,
}

/// Check every manifest line: provenance against `trusted` (when given), bytes present and
/// hashing to the address; and, given `descriptions`, that each description's bytes still hash
/// to the address the manifest carries for its capability.
pub fn verify(library: &Path, trusted: &[[u8; 32]], descriptions: Option<&Path>) -> Result<VerifyReport, String> {
    let m = Manifest::load(&library.join(MANIFEST_FILE)).map_err(|e| format!("manifest: {e}"))?;
    let lib = FsLibrarySource::open(library).map_err(|e| format!("{}: {e}", library.display()))?;
    let mut r = VerifyReport::default();
    for (i, e) in m.entries().iter().enumerate() {
        r.checked += 1;
        let who = format!("line {} ({}/{}, {})", i + 1, e.provides.namespace, e.provides.name, &e.artifact.to_hex()[..12]);
        if !trusted.is_empty() && !e.verify_provenance(trusted) {
            r.problems.push(format!("{who}: provenance does not verify against any trusted publisher"));
        }
        match lib.fetch(&e.artifact) {
            None => r.problems.push(format!("{who}: bytes are not in the library")),
            Some(bytes) => {
                if let Err(err) = verify_artifact(&bytes, &e.artifact) {
                    r.problems.push(format!("{who}: {err}"));
                }
            }
        }
    }
    if let Some(dir) = descriptions {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let d = ArtifactDescription::from_toml_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let Some(bytes_rel) = d.bytes.as_deref() else { continue };
            let bytes_path = path.parent().unwrap_or_else(|| Path::new(".")).join(bytes_rel);
            let Ok(bytes) = std::fs::read(&bytes_path) else {
                r.problems.push(format!("{}: bytes at {} cannot be read", path.display(), bytes_path.display()));
                continue;
            };
            let now = ArtifactId::of(&bytes);
            let cap = d.provides.to_capability();
            match m.entries().iter().find(|e| e.provides.namespace == cap.namespace && e.provides.name == cap.name) {
                None => r.problems.push(format!("{}: {}/{} is not in the manifest — publish it", path.display(), cap.namespace, cap.name)),
                Some(e) if e.artifact != now => r.problems.push(format!(
                    "{}: bytes changed under an unchanged manifest — the manifest names {} and the description's bytes hash to {}; publish again",
                    path.display(), &e.artifact.to_hex()[..12], &now.to_hex()[..12]
                )),
                Some(_) => {}
            }
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "mycelium-tools-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const DESCRIPTION: &str = r#"
kind = "wasm-component"
bytes = "optimizer.wasm"
est_install_secs = 1

[provides]
ns = "route"
name = "optimize"
schema_id = "route.optimize.v1"
  [provides.attrs]
  engine = { version = "1.4.0" }
  region = "north"
  slots = 4

[requires]
disk_bytes = 0
mem_bytes = 67108864
"#;

    /// A1's exit gate, first half: `list` of a manifest written by `publish` reproduces the
    /// description — the same kind, capability, attributes and footprint parse back.
    #[test]
    fn list_of_a_published_manifest_reproduces_the_description() {
        let dir = scratch("roundtrip");
        std::fs::write(dir.join("optimizer.toml"), DESCRIPTION).unwrap();
        std::fs::write(dir.join("optimizer.wasm"), b"pretend component bytes").unwrap();
        let lib = dir.join("library");
        let key = signing_key_from_hex(&"ab".repeat(32)).unwrap();

        let out = publish(&dir.join("optimizer.toml"), &lib, &key).unwrap();
        assert_eq!(out.kind, ArtifactKind::WasmComponent);
        assert_eq!(out.size_bytes, 23);
        assert!(out.signer.starts_with("ed25519:"));
        assert!(lib.join(out.artifact.to_hex()).exists(), "bytes stored content-addressed");

        let listed = list(&lib).unwrap();
        assert!(listed.contains(&format!("# artifact {} — 23 B — {}", out.artifact.to_hex(), out.signer)), "{listed}");
        let back = ArtifactDescription::from_toml_str(&listed).expect("the listing parses as a description");
        let want = ArtifactDescription::from_toml_str(DESCRIPTION).unwrap();
        assert_eq!(back.kind, want.kind);
        assert_eq!(back.provides.to_capability(), want.provides.to_capability(), "capability, schema and attributes round-trip");
        assert_eq!(back.requires, want.requires);
        assert_eq!(back.est_install_secs, want.est_install_secs);

        // Publishing again with the same bytes is idempotent: one line, one blob.
        publish(&dir.join("optimizer.toml"), &lib, &key).unwrap();
        assert_eq!(Manifest::load(&lib.join(MANIFEST_FILE)).unwrap().entries().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A1's exit gate, second half: a description whose bytes changed under an unchanged
    /// manifest fails `verify` by name; so does an untrusted signer and a missing blob.
    #[test]
    fn verify_names_changed_bytes_untrusted_signers_and_missing_blobs() {
        let dir = scratch("verify");
        let descriptions = dir.join("artifacts");
        std::fs::create_dir_all(&descriptions).unwrap();
        std::fs::write(descriptions.join("optimizer.toml"), DESCRIPTION).unwrap();
        std::fs::write(descriptions.join("optimizer.wasm"), b"v1 bytes").unwrap();
        let lib = dir.join("library");
        let key = signing_key_from_hex(&"cd".repeat(32)).unwrap();
        let trusted = key.verifying_key().to_bytes();
        let out = publish(&descriptions.join("optimizer.toml"), &lib, &key).unwrap();

        let r = verify(&lib, &[trusted], Some(&descriptions)).unwrap();
        assert_eq!(r.checked, 1);
        assert!(r.problems.is_empty(), "{:?}", r.problems);

        // The bytes change; the manifest did not.
        std::fs::write(descriptions.join("optimizer.wasm"), b"v2 bytes").unwrap();
        let r = verify(&lib, &[trusted], Some(&descriptions)).unwrap();
        assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
        assert!(r.problems[0].contains("bytes changed under an unchanged manifest"), "{}", r.problems[0]);

        // An untrusted publisher.
        let other = signing_key_from_hex(&"ef".repeat(32)).unwrap().verifying_key().to_bytes();
        let r = verify(&lib, &[other], None).unwrap();
        assert!(r.problems.iter().any(|p| p.contains("provenance does not verify")), "{:?}", r.problems);

        // The blob goes missing.
        std::fs::remove_file(lib.join(out.artifact.to_hex())).unwrap();
        let r = verify(&lib, &[trusted], None).unwrap();
        assert!(r.problems.iter().any(|p| p.contains("not in the library")), "{:?}", r.problems);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_and_kinds_are_parsed_by_name() {
        assert!(signing_key_from_hex("abc").unwrap_err().contains("64 hex"));
        assert!(signing_key_from_hex(&"zz".repeat(32)).unwrap_err().contains("hex"));
        assert_eq!(publisher_from_str(&format!("ed25519:{}", "01".repeat(32))).unwrap(), [1u8; 32]);
        assert!(publisher_from_str("rsa:00").unwrap_err().contains("ed25519:<64 hex>"));
        assert_eq!(kind_from_name("blob"), Some(ArtifactKind::Blob));
        assert_eq!(kind_from_name("native"), None);
        assert_eq!(kind_name(ArtifactKind::WasmComponent), "wasm-component");
    }
}
