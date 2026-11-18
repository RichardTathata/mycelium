//! **A developer's software consent key** (identity-and-lifecycle I7, P7): a P-256 key generated here, kept as PKCS#8
//! in a file only its user can read, and used to sign consent decisions (ASN.1 DER ECDSA over SHA-256, the form
//! [`verify_consent_p256`](crate::tls::verify_consent_p256) reads).
//!
//! **What this key is, and what it is not.** It is a *software* key: bytes in a same-user file. Any process running
//! as the user can read it and sign with it, with no terminal and no person (I7's residual B). The only grades such a
//! key may carry are `tty` and, in a `sim` build, `recording` — never an OS-presence grade: nothing here asks an
//! operating system for anyone's presence, and this module can promise only what it does itself. What it does:
//!
//! - it creates the key file `0600` in a `0700` directory, and **refuses to read** a key file or a directory that
//!   group or others can reach ([`ConsentKeyRefusal::KeyTooOpen`], [`ConsentKeyRefusal::DirTooOpen`]), or one that is
//!   not a regular file (a symbolic link could point anywhere, a worktree included);
//! - it never writes the private half anywhere but that file, and its `Debug` prints no private byte;
//! - it holds the private half zeroed on drop.
//!
//! **The provisional gate.** A build that reads no consent certificate
//! ([`consent_grade_oid_open`](crate::tls::consent_grade_oid_open) false — every shipped build while the grade
//! extension's OID is provisional) creates, loads and signs with no consent key: each entry refuses
//! [`ConsentKeyRefusal::ProvisionalOid`] before it touches a file.
//!
//! **Not built:** a Windows ACL standing for the POSIX modes — off unix, [`SoftwareConsentKey::store`] and
//! [`SoftwareConsentKey::load`] refuse ([`ConsentKeyRefusal::PlatformNotBuilt`]) rather than keep a key this module
//! cannot say is guarded; a check of the file's owner (the modes are read, the uid is not); the macOS Secure Enclave
//! key, which is the Swift shim's (I7's later PR).
//!
//! The clock is not read here. The randomness is `ring`'s system generator, for key generation and for ECDSA's
//! per-signature nonce: a developer's key is made and used outside a node, by `mycelium dev consent`, and no node's
//! recording contains it (`docs/design/replay-nondeterminism-inventory.md`).

use std::path::{Path, PathBuf};

/// Why a software consent key was not created, stored, loaded or used. Each has a stable [`code`](Self::code).
#[derive(Debug)]
#[non_exhaustive]
pub enum ConsentKeyRefusal {
    /// This build reads no consent certificate while the grade extension's OID is provisional, so it holds no
    /// consent key either.
    ProvisionalOid,
    /// A key is already stored at the path, and replacing it was not asked for: a new key voids the certificate the
    /// fleet CA issued over the old one.
    KeyExists(PathBuf),
    /// The key file can be reached by group or others (its mode is given), or is not a regular file.
    KeyTooOpen(PathBuf, String),
    /// The directory holding the key can be reached by group or others (its mode is given), or is not a directory.
    DirTooOpen(PathBuf, String),
    /// The file does not hold a P-256 PKCS#8 key.
    NotAKey(PathBuf),
    /// The file or directory could not be read or written.
    Io(PathBuf, std::io::Error),
    /// A platform without POSIX file modes: the user-only ACL that would stand for them is not built.
    PlatformNotBuilt,
    /// The system's random generator failed.
    Random,
}

impl ConsentKeyRefusal {
    /// The refusal's stable name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProvisionalOid => "consent_grade_oid_provisional",
            Self::KeyExists(_) => "consent_key_exists",
            Self::KeyTooOpen(..) => "consent_key_too_open",
            Self::DirTooOpen(..) => "consent_dir_too_open",
            Self::NotAKey(_) => "consent_key_unreadable",
            Self::Io(..) => "consent_key_io",
            Self::PlatformNotBuilt => "consent_key_platform_not_built",
            Self::Random => "consent_key_random",
        }
    }
}

impl std::fmt::Display for ConsentKeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProvisionalOid => f.write_str(
                "consent keys are not created or used by this build: the grade extension's OID is provisional until the registered IANA Private Enterprise Number arc arrives (identity-and-lifecycle I7)",
            ),
            Self::KeyExists(p) => write!(f, "a consent key is already stored at {}: replacing it voids the certificate issued over it (pass --force to replace it, then have the new key certified)", p.display()),
            Self::KeyTooOpen(p, why) => write!(f, "the consent key file {} is refused: {why}. A consent key is read only from a regular file that its user alone can reach (chmod 600)", p.display()),
            Self::DirTooOpen(p, why) => write!(f, "the consent key directory {} is refused: {why}. A consent key is kept only in a directory that its user alone can reach (chmod 700)", p.display()),
            Self::NotAKey(p) => write!(f, "{} does not hold a P-256 PKCS#8 key", p.display()),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::PlatformNotBuilt => f.write_str("a software consent key is not kept on this platform: the user-only ACL that would stand for the 0600 file in a 0700 directory is not built (identity-and-lifecycle I7)"),
            Self::Random => f.write_str("the system's random generator failed"),
        }
    }
}

impl std::error::Error for ConsentKeyRefusal {}

/// A P-256 consent key held in software (the module doc says what that means for its grade).
pub struct SoftwareConsentKey {
    pkcs8: zeroize::Zeroizing<Vec<u8>>,
    point: [u8; 65],
}

impl std::fmt::Debug for SoftwareConsentKey {
    /// The public point and nothing of the private half.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let point: String = self.point.iter().map(|b| format!("{b:02x}")).collect();
        f.debug_struct("SoftwareConsentKey").field("point", &point).field("private", &"<never printed>").finish()
    }
}

fn gate() -> Result<(), ConsentKeyRefusal> {
    if crate::tls::consent_grade_oid_open() { Ok(()) } else { Err(ConsentKeyRefusal::ProvisionalOid) }
}

impl SoftwareConsentKey {
    /// The grade a software key may be certified at when a person approves at a terminal: `tty`, and no other.
    pub const GRADE: crate::tls::ConsentGrade = crate::tls::ConsentGrade::Tty;

    /// A new key from the system's random generator. Nothing is written.
    pub fn generate() -> Result<Self, ConsentKeyRefusal> {
        gate()?;
        let rng = ring::rand::SystemRandom::new();
        let doc = ring::signature::EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng).map_err(|_| ConsentKeyRefusal::Random)?;
        Self::parse(doc.as_ref()).ok_or(ConsentKeyRefusal::Random)
    }

    fn parse(pkcs8: &[u8]) -> Option<Self> {
        use ring::signature::KeyPair as _;
        let rng = ring::rand::SystemRandom::new();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng).ok()?;
        let point: [u8; 65] = pair.public_key().as_ref().try_into().ok()?;
        Some(Self { pkcs8: zeroize::Zeroizing::new(pkcs8.to_vec()), point })
    }

    /// The public half: the uncompressed X9.63 point, as a consent certificate carries it.
    pub fn point(&self) -> [u8; 65] {
        self.point
    }

    /// The public half as SPKI PEM, the form `mycelium tls issue --consent --public-key` takes.
    pub fn spki_pem(&self) -> String {
        crate::tls::p256_spki_pem(&self.point)
    }

    /// An ASN.1 DER ECDSA signature over the SHA-256 of `msg`.
    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, ConsentKeyRefusal> {
        gate()?;
        let rng = ring::rand::SystemRandom::new();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &self.pkcs8, &rng).map_err(|_| ConsentKeyRefusal::Random)?;
        pair.sign(&rng, msg).map(|s| s.as_ref().to_vec()).map_err(|_| ConsentKeyRefusal::Random)
    }

    /// Write the key to `path`, `0600`, creating its directory `0700` (and any missing parent the same way). Refused
    /// when a key is already there unless `replace`; refused when the directory — created here or found — can be
    /// reached by group or others. Off unix: [`ConsentKeyRefusal::PlatformNotBuilt`], nothing written.
    pub fn store(&self, path: &Path, replace: bool) -> Result<(), ConsentKeyRefusal> {
        gate()?;
        #[cfg(unix)]
        {
            let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ConsentKeyRefusal::DirTooOpen(path.to_path_buf(), "the key file names no directory".into()))?;
            crate::tls::create_private_dir(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
            private_dir(dir)?;
            match std::fs::symlink_metadata(path) {
                Ok(_) if !replace => return Err(ConsentKeyRefusal::KeyExists(path.to_path_buf())),
                // A replaced key must itself be a file this module would read: never write through a link.
                Ok(_) => private_file(path)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ConsentKeyRefusal::Io(path.to_path_buf(), e)),
            }
            crate::tls::write_private(path, &self.pkcs8).map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))
        }
        #[cfg(not(unix))]
        {
            let _ = (path, replace);
            Err(ConsentKeyRefusal::PlatformNotBuilt)
        }
    }

    /// Read the key at `path`. The directory's and the file's modes are checked **before a byte of the key is read**:
    /// either one reachable by group or others, or the file not a regular file, is refused by name. Off unix:
    /// [`ConsentKeyRefusal::PlatformNotBuilt`].
    pub fn load(path: &Path) -> Result<Self, ConsentKeyRefusal> {
        gate()?;
        #[cfg(unix)]
        {
            let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ConsentKeyRefusal::DirTooOpen(path.to_path_buf(), "the key file names no directory".into()))?;
            private_dir(dir)?;
            private_file(path)?;
            let bytes = zeroize::Zeroizing::new(std::fs::read(path).map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))?);
            Self::parse(&bytes).ok_or_else(|| ConsentKeyRefusal::NotAKey(path.to_path_buf()))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(ConsentKeyRefusal::PlatformNotBuilt)
        }
    }
}

/// Write a **public** file — the key's SPKI PEM, or the consent certificate the fleet CA issued over it — at `path`,
/// `0644`, in a directory only its user can reach (created `0700` when missing). The consent directory holds the key,
/// so nothing is written into one group or others can reach. Off unix: [`ConsentKeyRefusal::PlatformNotBuilt`].
pub fn store_public(path: &Path, contents: &[u8]) -> Result<(), ConsentKeyRefusal> {
    gate()?;
    #[cfg(unix)]
    {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ConsentKeyRefusal::DirTooOpen(path.to_path_buf(), "the file names no directory".into()))?;
        crate::tls::create_private_dir(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
        private_dir(dir)?;
        crate::tls::write_public(path, contents).map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, contents);
        Err(ConsentKeyRefusal::PlatformNotBuilt)
    }
}

/// Read a public file [`store_public`] wrote, from a directory only its user can reach. `Ok(None)` when it is absent.
pub fn load_public(path: &Path) -> Result<Option<Vec<u8>>, ConsentKeyRefusal> {
    gate()?;
    #[cfg(unix)]
    {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ConsentKeyRefusal::DirTooOpen(path.to_path_buf(), "the file names no directory".into()))?;
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => private_dir(dir)?,
        }
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConsentKeyRefusal::Io(path.to_path_buf(), e)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(ConsentKeyRefusal::PlatformNotBuilt)
    }
}

/// `dir` is a directory, not a link, that only its user can reach.
#[cfg(unix)]
fn private_dir(dir: &Path) -> Result<(), ConsentKeyRefusal> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
    if !meta.file_type().is_dir() {
        return Err(ConsentKeyRefusal::DirTooOpen(dir.to_path_buf(), "it is not a directory (a symbolic link is not followed)".into()));
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ConsentKeyRefusal::DirTooOpen(dir.to_path_buf(), format!("its mode is {mode:03o}, which lets group or others reach it")));
    }
    Ok(())
}

/// `path` is a regular file, not a link, that only its user can reach.
#[cfg(unix)]
fn private_file(path: &Path) -> Result<(), ConsentKeyRefusal> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::symlink_metadata(path).map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))?;
    if !meta.file_type().is_file() {
        return Err(ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), "it is not a regular file (a symbolic link is not followed)".into()));
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), format!("its mode is {mode:03o}, which lets group or others reach it")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::{verify_consent_p256, with_provisional_consent_grade_closed};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mycelium-consent-key-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[cfg(unix)]
    fn chmod(p: &Path, m: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    /// **A software consent key signs what the one P-256 verifier reads** (I7, P7; exit 6's key half): generated
    /// here, its signature verifies under its own point and under no other key or message; the SPKI PEM it exports is
    /// the point the issuer reads.
    #[test]
    fn a_software_consent_key_signs_what_the_one_verifier_reads() {
        let key = SoftwareConsentKey::generate().expect("generated in a test build");
        let other = SoftwareConsentKey::generate().unwrap();
        assert_ne!(key.point(), other.point(), "two keys");
        let sig = key.sign(b"a decision").expect("signed");
        assert!(verify_consent_p256(&key.point(), b"a decision", &sig), "its own point verifies it");
        assert!(!verify_consent_p256(&key.point(), b"another decision", &sig), "not another message");
        assert!(!verify_consent_p256(&other.point(), b"a decision", &sig), "not another key");
        assert_eq!(crate::tls::p256_point_from_spki_pem(&key.spki_pem()).expect("the issuer reads the export"), key.point());
        assert_eq!(SoftwareConsentKey::GRADE.as_str(), "tty", "a software key's grade: where the key lives, never a presence");
    }

    /// **The key file is written 0600 in a 0700 directory, and a wider file or directory is refused by name before
    /// the key is read** (I7, exit 10). Each widening — group read, other read, group write, a directory others can
    /// list, a symbolic link standing for the file, a link standing for the directory — is refused; narrowed again,
    /// the key loads and is the key that was stored.
    #[cfg(unix)]
    #[test]
    fn a_consent_key_is_stored_private_and_a_wider_file_is_refused_by_name() {
        let root = scratch("modes");
        let dir = root.join("a").join("consent");
        let path = dir.join("consent-fleet-richard.key.p8");
        let key = SoftwareConsentKey::generate().unwrap();
        key.store(&path, false).expect("stored");
        assert_eq!(mode(&path), 0o600, "the key file");
        assert_eq!(mode(&dir), 0o700, "its directory");
        assert_eq!(mode(&root.join("a")), 0o700, "a missing parent, created the same way");
        assert_eq!(SoftwareConsentKey::load(&path).expect("loads").point(), key.point(), "the key that was stored");

        for wider in [0o640, 0o604, 0o620, 0o602, 0o644, 0o660, 0o666] {
            chmod(&path, wider);
            let refused = SoftwareConsentKey::load(&path).expect_err("a wider file is refused");
            assert_eq!(refused.code(), "consent_key_too_open", "mode {wider:03o}: {refused}");
            assert!(refused.to_string().contains(&format!("{wider:03o}")), "the refusal names the mode: {refused}");
        }
        chmod(&path, 0o600);
        for wider in [0o750, 0o705, 0o755, 0o770, 0o777] {
            chmod(&dir, wider);
            let refused = SoftwareConsentKey::load(&path).expect_err("a wider directory is refused");
            assert_eq!(refused.code(), "consent_dir_too_open", "mode {wider:03o}: {refused}");
            // And nothing is stored into it either, replace or not.
            assert_eq!(key.store(&path, true).expect_err("not stored into a wider directory").code(), "consent_dir_too_open");
        }
        chmod(&dir, 0o700);
        SoftwareConsentKey::load(&path).expect("narrowed again, it loads");

        // A link standing for the key file: the real file could be anywhere, a worktree included.
        let elsewhere = root.join("elsewhere.p8");
        std::fs::rename(&path, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert_eq!(SoftwareConsentKey::load(&path).expect_err("a link is not a key file").code(), "consent_key_too_open");
        assert_eq!(key.store(&path, true).expect_err("and is never written through").code(), "consent_key_too_open");
        std::fs::remove_file(&path).unwrap();
        // A link standing for the directory.
        let linked = root.join("linked");
        std::os::unix::fs::symlink(&dir, &linked).unwrap();
        std::fs::rename(&elsewhere, &path).unwrap();
        assert_eq!(SoftwareConsentKey::load(&linked.join("consent-fleet-richard.key.p8")).expect_err("a linked directory").code(), "consent_dir_too_open");
        // Not a key.
        let junk = dir.join("junk.p8");
        crate::tls::write_private(&junk, b"not a key").unwrap();
        assert_eq!(SoftwareConsentKey::load(&junk).expect_err("junk").code(), "consent_key_unreadable");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A stored key is never replaced silently**: a second `store` at the path is refused by name and leaves the
    /// first key there; with `replace` the new key is the one read back.
    #[cfg(unix)]
    #[test]
    fn a_stored_consent_key_is_replaced_only_when_asked() {
        let root = scratch("replace");
        let path = root.join("consent").join("k.p8");
        let (first, second) = (SoftwareConsentKey::generate().unwrap(), SoftwareConsentKey::generate().unwrap());
        first.store(&path, false).unwrap();
        assert_eq!(second.store(&path, false).expect_err("a key is there").code(), "consent_key_exists");
        assert_eq!(SoftwareConsentKey::load(&path).unwrap().point(), first.point(), "the first key stands");
        second.store(&path, true).expect("replaced when asked");
        assert_eq!(SoftwareConsentKey::load(&path).unwrap().point(), second.point());
        assert_eq!(mode(&path), 0o600, "still private after a replacement");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Nothing prints the private half.** `Debug` shows the public point; neither it nor any refusal's text holds
    /// the PKCS#8 bytes in hex or base64, or the private scalar.
    #[test]
    fn a_consent_keys_debug_and_refusals_print_no_private_byte() {
        use base64::Engine as _;
        let key = SoftwareConsentKey::generate().unwrap();
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let shown = format!("{key:?} {key:#?}");
        // The private scalar sits in the PKCS#8 before the public point; any 16-byte run of it would be a leak.
        let private_part = &key.pkcs8[..key.pkcs8.len() - 65];
        for window in private_part.windows(16) {
            assert!(!shown.contains(&hex(window)), "Debug prints a run of the private half: {shown}");
        }
        assert!(!shown.contains(&base64::engine::general_purpose::STANDARD.encode(&key.pkcs8[..])), "nor its base64");
        assert!(shown.contains(&hex(&key.point())), "it does print the public point: {shown}");
        assert!(shown.contains("never printed"));
    }

    /// **A build that reads no consent certificate holds no consent key** (the provisional gate): with the gate
    /// closed, as in every shipped build, `generate`, `store`, `load` and `sign` each refuse by name, and `store`
    /// creates nothing; the public files beside the key (`store_public`, `load_public`) are refused the same way.
    #[test]
    fn with_the_provisional_gate_closed_no_consent_key_is_made_stored_loaded_or_used() {
        let root = scratch("gate");
        let path = root.join("consent").join("k.p8");
        let key = SoftwareConsentKey::generate().unwrap();
        #[cfg(unix)]
        key.store(&path, false).unwrap();
        with_provisional_consent_grade_closed(|| {
            assert_eq!(SoftwareConsentKey::generate().expect_err("no key is made").code(), "consent_grade_oid_provisional");
            assert_eq!(key.sign(b"m").expect_err("nothing is signed").code(), "consent_grade_oid_provisional");
            assert_eq!(SoftwareConsentKey::load(&path).expect_err("nothing is loaded").code(), "consent_grade_oid_provisional");
            let fresh = root.join("fresh").join("k.p8");
            assert_eq!(key.store(&fresh, false).expect_err("nothing is stored").code(), "consent_grade_oid_provisional");
            assert!(!root.join("fresh").exists(), "and no directory was created");
            // The public files beside the key are consent's too: none is written or read.
            let public = root.join("public").join("k.pub.pem");
            assert_eq!(store_public(&public, b"pem").expect_err("no public file is written").code(), "consent_grade_oid_provisional");
            assert!(!root.join("public").exists(), "and no directory was created for it");
            assert_eq!(load_public(&root.join("consent").join("k.pub.pem")).expect_err("no public file is read").code(), "consent_grade_oid_provisional");
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
