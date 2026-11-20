//! **A developer's software consent key** (identity-and-lifecycle I7, P7): a P-256 key generated here, kept as PKCS#8
//! in a file only its user can read, and used to sign consent decisions (ASN.1 DER ECDSA over SHA-256, the form
//! [`verify_consent_p256`](crate::tls::verify_consent_p256) reads).
//!
//! **What this key is, and what it is not.** It is a *software* key: bytes in a same-user file. Any process running
//! as the user can read it and sign with it, with no terminal and no person (I7's residual B). The only grades such a
//! key may carry are `tty` and, in a `sim` build, `recording` — never an OS-presence grade: nothing here asks an
//! operating system for anyone's presence, and this module can promise only what it does itself. What it does:
//!
//! - it creates the key file `0600` in a `0700` directory, exclusively (`O_EXCL`, never through a link); a
//!   replacement is written beside it and renamed over it, never truncated in place;
//! - it **reads the key from a descriptor it has checked**: the file is opened without following a link
//!   (`O_NOFOLLOW`), and the open descriptor must be a regular file, **owned by this user**, that group and others
//!   cannot reach by its mode bits, with **one link** ([`ConsentKeyRefusal::KeyTooOpen`],
//!   [`ConsentKeyRefusal::NotOwned`], [`ConsentKeyRefusal::HardLinked`]) — before a byte of it is read;
//! - the directory holding it must be a directory (not a link), owned by this user, that group and others cannot
//!   reach ([`ConsentKeyRefusal::DirTooOpen`]); [`guarded_ancestors`] checks the directories above it up to a base
//!   the caller names;
//! - it never writes the private half anywhere but that file, and its `Debug` prints no private byte;
//! - the PKCS#8 bytes this module holds are zeroed on drop. **Not zeroed:** `ring`'s own copies — the document
//!   `generate` returns and the scalar inside each `EcdsaKeyPair` a signature builds — which this module cannot reach.
//!
//! **What the checks are, exactly.** Owner and mode bits, read with `fstat` (the file) and `lstat` (the directory).
//! **An access-control list is not read.** On macOS a file's ACL can grant another user access while its mode reads
//! `0600`, and a directory's inheritable ACL can put one on every file created in it; reading an ACL needs a call
//! into the platform's C library (`acl_get_fd_np`), which `rustix` does not wrap and this crate's no-`unsafe` rule
//! keeps out. So "only its user can reach it" is true of the mode bits and the owner, not of ACLs. The directory is
//! checked by path, not through a descriptor: swapping it needs write access to its parent, which
//! [`guarded_ancestors`] is for.
//!
//! **The provisional gate.** A build that reads no consent certificate
//! ([`consent_grade_oid_open`](crate::tls::consent_grade_oid_open) false — every shipped build while the grade
//! extension's OID is provisional) creates, loads and signs with no consent key: each entry refuses
//! [`ConsentKeyRefusal::ProvisionalOid`] before it touches a file.
//!
//! **Not built:** a Windows ACL standing for the POSIX modes — off unix, [`SoftwareConsentKey::store`] and
//! [`SoftwareConsentKey::load`] refuse ([`ConsentKeyRefusal::PlatformNotBuilt`]) rather than keep a key this module
//! cannot say is guarded (those arms are compiled by no CI job); reading a macOS ACL (above); the macOS Secure
//! Enclave key, which is the Swift shim's (I7's later PR).
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
    /// The file or directory belongs to another user (its owner's uid, then this process's).
    NotOwned(PathBuf, u32, u32),
    /// The key file has more than one name (its link count): the other name can be anywhere, a worktree included.
    HardLinked(PathBuf, u64),
    /// A directory above the key's can be written by group or others, or belongs to another user: whoever can write
    /// it can replace the directory below it.
    AncestorTooOpen(PathBuf, String),
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
            Self::NotOwned(..) => "consent_key_not_owned",
            Self::HardLinked(..) => "consent_key_hard_linked",
            Self::AncestorTooOpen(..) => "consent_dir_ancestor_too_open",
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
            Self::NotOwned(p, owner, me) => write!(f, "{} belongs to uid {owner}, and this is uid {me}: a consent key is read only from a file and a directory its own user owns", p.display()),
            Self::HardLinked(p, n) => write!(f, "the consent key file {} has {n} names (hard links): its other name can be anywhere. Remove the other link, or replace the key", p.display()),
            Self::AncestorTooOpen(p, why) => write!(f, "{} is above the consent key and is refused: {why}. Whoever can write it can replace the directory below it", p.display()),
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

    /// Write the key to `path`, `0600`, creating its directory `0700` (and any missing parent the same way).
    ///
    /// Without `replace` the file is created exclusively (`O_EXCL`): a key, or anything else, already at the path is
    /// refused and left as it is. With `replace`, whatever is there must itself be a key file this module would read
    /// (or nothing); the new key is written to a new file beside it and **renamed over it**, so the old key is never
    /// truncated in place and a crash leaves one whole key or the other. Refused when the directory — created here or
    /// found — is not a private directory of this user. Off unix: [`ConsentKeyRefusal::PlatformNotBuilt`], nothing
    /// written.
    pub fn store(&self, path: &Path, replace: bool) -> Result<(), ConsentKeyRefusal> {
        gate()?;
        #[cfg(unix)]
        {
            let dir = dir_of(path)?;
            crate::tls::create_private_dir(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
            private_dir(dir, me())?;
            if !replace {
                return create_exclusive(path, &self.pkcs8, 0o600).map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => ConsentKeyRefusal::KeyExists(path.to_path_buf()),
                    _ => ConsentKeyRefusal::Io(path.to_path_buf(), e),
                });
            }
            match open_nofollow(path) {
                Ok(existing) => private_regular(&existing, path, me())?,
                Err(ConsentKeyRefusal::Io(_, e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(refusal) => return Err(refusal),
            }
            write_then_rename(path, &self.pkcs8, 0o600)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, replace);
            Err(ConsentKeyRefusal::PlatformNotBuilt)
        }
    }

    /// Read the key at `path`, from a descriptor checked first (the module doc): the directory a private directory
    /// of this user; the file opened without following a link; the open file regular, owned by this user, closed to
    /// group and others, with one link — each refused by name **before a byte of the key is read**. Off unix:
    /// [`ConsentKeyRefusal::PlatformNotBuilt`].
    pub fn load(path: &Path) -> Result<Self, ConsentKeyRefusal> {
        gate()?;
        #[cfg(unix)]
        {
            Self::load_owned_by(path, me())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(ConsentKeyRefusal::PlatformNotBuilt)
        }
    }

    #[cfg(unix)]
    fn load_owned_by(path: &Path, owner: u32) -> Result<Self, ConsentKeyRefusal> {
        use std::io::Read as _;
        private_dir(dir_of(path)?, owner)?;
        let file = open_nofollow(path)?;
        private_regular(&file, path, owner)?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        // A PKCS#8 P-256 key is 138 bytes; the bound keeps a file that is not one from being read whole.
        (&file).take(16 * 1024).read_to_end(&mut bytes).map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))?;
        Self::parse(&bytes).ok_or_else(|| ConsentKeyRefusal::NotAKey(path.to_path_buf()))
    }
}

/// Create the directory a consent key is kept in, `0700` (and any missing parent the same way), and check that it —
/// created here or found — is a private directory of this user. Nothing else is written. A caller that must ask
/// something of the directory once it exists (is its real path inside a worktree?) asks between this and
/// [`SoftwareConsentKey::store`]. Off unix: [`ConsentKeyRefusal::PlatformNotBuilt`].
pub fn create_dir(dir: &Path) -> Result<(), ConsentKeyRefusal> {
    gate()?;
    #[cfg(unix)]
    {
        crate::tls::create_private_dir(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
        private_dir(dir, me())
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Err(ConsentKeyRefusal::PlatformNotBuilt)
    }
}

/// Write a **public** file — the key's SPKI PEM, or the consent certificate the fleet CA issued over it — at `path`,
/// `0644`, in a private directory of this user (created `0700` when missing). Written beside the path and renamed
/// over it, so nothing is written through a link standing there. Off unix: [`ConsentKeyRefusal::PlatformNotBuilt`].
pub fn store_public(path: &Path, contents: &[u8]) -> Result<(), ConsentKeyRefusal> {
    gate()?;
    #[cfg(unix)]
    {
        let dir = dir_of(path)?;
        crate::tls::create_private_dir(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
        private_dir(dir, me())?;
        write_then_rename(path, contents, 0o644)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, contents);
        Err(ConsentKeyRefusal::PlatformNotBuilt)
    }
}

/// Read a public file [`store_public`] wrote, from a private directory of this user, without following a link, at
/// most 256 KiB. `Ok(None)` when it is absent.
pub fn load_public(path: &Path) -> Result<Option<Vec<u8>>, ConsentKeyRefusal> {
    gate()?;
    #[cfg(unix)]
    {
        use std::io::Read as _;
        let dir = dir_of(path)?;
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => private_dir(dir, me())?,
        }
        let file = match open_nofollow(path) {
            Ok(file) => file,
            Err(ConsentKeyRefusal::Io(_, e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
        let io = |e| ConsentKeyRefusal::Io(path.to_path_buf(), e);
        if !file.metadata().map_err(io)?.file_type().is_file() {
            return Err(ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), "it is not a regular file".into()));
        }
        let mut bytes = Vec::new();
        (&file).take(256 * 1024).read_to_end(&mut bytes).map_err(io)?;
        Ok(Some(bytes))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(ConsentKeyRefusal::PlatformNotBuilt)
    }
}

/// `path` with every symbolic link resolved: the deepest part of it that exists is canonicalised, and the part that
/// does not exist yet is appended as written. What "is this directory inside a worktree" must be asked of — a
/// lexical path says nothing when a component is a link.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path;
    loop {
        match std::fs::canonicalize(at) {
            Ok(mut real) => {
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match (at.parent(), at.file_name()) {
                (Some(parent), Some(name)) => {
                    missing.push(name.to_owned());
                    at = parent;
                }
                _ => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
}

/// Every directory above `dir`, up to and including `base`, belongs to this user (or to root) and cannot be written
/// by group or others: whoever can write a directory can rename what is in it, and so replace the key's directory
/// with their own. **The bound is `base`** — the home directory, or the data directory the environment named:
/// nothing above it is examined. Nothing is checked when `dir` is not under `base`. Off unix: nothing.
pub fn guarded_ancestors(dir: &Path, base: &Path) -> Result<(), ConsentKeyRefusal> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let me = me();
        for above in dir.ancestors().skip(1).take_while(|a| a.starts_with(base)) {
            let meta = std::fs::metadata(above).map_err(|e| ConsentKeyRefusal::Io(above.to_path_buf(), e))?;
            let mode = meta.permissions().mode() & 0o7777;
            if mode & 0o022 != 0 {
                return Err(ConsentKeyRefusal::AncestorTooOpen(above.to_path_buf(), format!("its mode is {mode:03o}, which lets group or others write it")));
            }
            if meta.uid() != me && meta.uid() != 0 {
                return Err(ConsentKeyRefusal::AncestorTooOpen(above.to_path_buf(), format!("it belongs to uid {}, and this is uid {me}", meta.uid())));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (dir, base);
    Ok(())
}

/// This process's effective user.
#[cfg(unix)]
fn me() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(unix)]
fn dir_of(path: &Path) -> Result<&Path, ConsentKeyRefusal> {
    path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| ConsentKeyRefusal::DirTooOpen(path.to_path_buf(), "the file names no directory".into()))
}

/// `dir` is a directory, not a link, owned by `owner`, that group and others cannot reach by its mode bits.
#[cfg(unix)]
fn private_dir(dir: &Path, owner: u32) -> Result<(), ConsentKeyRefusal> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let meta = std::fs::symlink_metadata(dir).map_err(|e| ConsentKeyRefusal::Io(dir.to_path_buf(), e))?;
    if !meta.file_type().is_dir() {
        return Err(ConsentKeyRefusal::DirTooOpen(dir.to_path_buf(), "it is not a directory (a symbolic link is not followed)".into()));
    }
    if meta.uid() != owner {
        return Err(ConsentKeyRefusal::NotOwned(dir.to_path_buf(), meta.uid(), owner));
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ConsentKeyRefusal::DirTooOpen(dir.to_path_buf(), format!("its mode is {mode:03o}, which lets group or others reach it")));
    }
    Ok(())
}

/// Open `path` for reading **without following a link at its last component** (`O_NOFOLLOW`), and without waiting
/// on a FIFO or a device standing there (`O_NONBLOCK`). A link is refused by name.
#[cfg(unix)]
fn open_nofollow(path: &Path) -> Result<std::fs::File, ConsentKeyRefusal> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let flags = (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32;
    std::fs::OpenOptions::new().read(true).custom_flags(flags).open(path).map_err(|e| {
        if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
            ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), "it is not a regular file (a symbolic link is not followed)".into())
        } else {
            ConsentKeyRefusal::Io(path.to_path_buf(), e)
        }
    })
}

/// The open `file` is a regular file, owned by `owner`, that group and others cannot reach by its mode bits, with
/// one link — read from the descriptor (`fstat`), so it is the file that will be read and no other.
#[cfg(unix)]
fn private_regular(file: &std::fs::File, path: &Path, owner: u32) -> Result<(), ConsentKeyRefusal> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let meta = file.metadata().map_err(|e| ConsentKeyRefusal::Io(path.to_path_buf(), e))?;
    if !meta.file_type().is_file() {
        return Err(ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), "it is not a regular file".into()));
    }
    if meta.uid() != owner {
        return Err(ConsentKeyRefusal::NotOwned(path.to_path_buf(), meta.uid(), owner));
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ConsentKeyRefusal::KeyTooOpen(path.to_path_buf(), format!("its mode is {mode:03o}, which lets group or others reach it")));
    }
    if meta.nlink() != 1 {
        return Err(ConsentKeyRefusal::HardLinked(path.to_path_buf(), meta.nlink()));
    }
    Ok(())
}

/// Create `path` with `mode` and write `contents`, **only if nothing is there** (`O_EXCL`), never through a link
/// (`O_NOFOLLOW`); the mode is set on the descriptor before the first byte, whatever the umask.
#[cfg(unix)]
fn create_exclusive(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32).open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Write `contents` to a new file beside `path` and rename it over `path`: the file at `path` is replaced whole, never
/// truncated in place, and a link standing at `path` is replaced rather than written through.
#[cfg(unix)]
fn write_then_rename(path: &Path, contents: &[u8], mode: u32) -> Result<(), ConsentKeyRefusal> {
    let io = |at: &Path| { let at = at.to_path_buf(); move |e| ConsentKeyRefusal::Io(at, e) };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let beside = path.with_file_name(format!(".{name}.new-{}", std::process::id()));
    // A leftover of an earlier, interrupted replacement by a process of the same id: removed, then created anew.
    let _ = std::fs::remove_file(&beside);
    create_exclusive(&beside, contents, mode).map_err(io(&beside))?;
    std::fs::rename(&beside, path).map_err(|e| {
        let _ = std::fs::remove_file(&beside);
        ConsentKeyRefusal::Io(path.to_path_buf(), e)
    })
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
        let path = dir.join("consent-fleet-alex.key.p8");
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
        assert_eq!(SoftwareConsentKey::load(&linked.join("consent-fleet-alex.key.p8")).expect_err("a linked directory").code(), "consent_dir_too_open");
        // Not a key.
        let junk = dir.join("junk.p8");
        create_exclusive(&junk, b"not a key", 0o600).unwrap();
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

    /// **The key is read from a descriptor that was checked, and belongs to this user** (#813's review, F5 and F6).
    /// The file's owner and the directory's owner must be this user: each is refused by name for any other uid. A
    /// key with a second name — a hard link, which can sit anywhere, a worktree included — is refused by name, and
    /// loads again once the other name is gone (F4).
    #[cfg(unix)]
    #[test]
    fn a_consent_key_is_read_only_from_a_file_and_directory_this_user_owns_with_one_name() {
        let root = scratch("owner");
        let dir = root.join("consent");
        let path = dir.join("consent-fleet-alex.key.p8");
        let key = SoftwareConsentKey::generate().unwrap();
        key.store(&path, false).unwrap();
        let (mine, another) = (me(), me().wrapping_add(1));
        let file = open_nofollow(&path).unwrap();
        private_regular(&file, &path, mine).expect("this user's own file");
        assert_eq!(private_regular(&file, &path, another).expect_err("another user's file").code(), "consent_key_not_owned");
        private_dir(&dir, mine).expect("this user's own directory");
        assert_eq!(private_dir(&dir, another).expect_err("another user's directory").code(), "consent_key_not_owned");
        assert_eq!(SoftwareConsentKey::load_owned_by(&path, another).expect_err("loaded as another user").code(), "consent_key_not_owned");

        let elsewhere = root.join("a-second-name.p8");
        std::fs::hard_link(&path, &elsewhere).unwrap();
        let refused = SoftwareConsentKey::load(&path).expect_err("a key with two names");
        assert_eq!(refused.code(), "consent_key_hard_linked", "{refused}");
        assert_eq!(key.store(&path, true).expect_err("nor replaced while it has two names").code(), "consent_key_hard_linked");
        std::fs::remove_file(&elsewhere).unwrap();
        assert_eq!(SoftwareConsentKey::load(&path).expect("one name again").point(), key.point());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A replacement never truncates the key in place, and a key is never created over something** (#813's
    /// review, F6). `store(.., replace)` writes a new file and renames it over the old one: the path names a
    /// different inode afterwards, no temporary is left behind, and the mode is `0600`. Without `replace`, a dangling
    /// link at the path is refused and its target is never created.
    #[cfg(unix)]
    #[test]
    fn a_replacement_is_a_new_file_renamed_over_the_old_and_nothing_is_created_through_a_link() {
        use std::os::unix::fs::MetadataExt as _;
        let root = scratch("rename");
        let dir = root.join("consent");
        let path = dir.join("k.p8");
        let (first, second) = (SoftwareConsentKey::generate().unwrap(), SoftwareConsentKey::generate().unwrap());
        first.store(&path, false).unwrap();
        let before = std::fs::metadata(&path).unwrap().ino();
        second.store(&path, true).expect("replaced");
        assert_ne!(std::fs::metadata(&path).unwrap().ino(), before, "a new file was renamed over the old one: the old key was not truncated in place");
        assert_eq!((SoftwareConsentKey::load(&path).unwrap().point(), mode(&path)), (second.point(), 0o600));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary is left beside the key");

        let (dangling, target) = (dir.join("dangling.p8"), root.join("target.p8"));
        std::os::unix::fs::symlink(&target, &dangling).unwrap();
        assert_eq!(first.store(&dangling, false).expect_err("something is at the path").code(), "consent_key_exists");
        assert!(!target.exists(), "nothing was created through the link");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A directory above the key's that group or others can write is refused, up to the base** (#813's review,
    /// F5): whoever can write it can swap the directory below. Each directory from the key's parent up to and
    /// including the base is checked; above the base nothing is.
    #[cfg(unix)]
    #[test]
    fn a_writable_directory_above_the_key_is_refused_up_to_the_base() {
        let root = scratch("ancestors");
        let (base, mid) = (root.join("home"), root.join("home").join("share"));
        let dir = mid.join("mycelium").join("consent");
        std::fs::create_dir_all(&dir).unwrap();
        for d in [&root, &base, &mid, &mid.join("mycelium"), &dir] {
            chmod(d, 0o755);
        }
        guarded_ancestors(&dir, &base).expect("no directory above is writable by others");
        for (wide, at) in [(0o777, &mid), (0o775, &mid), (0o757, &base), (0o1777, &mid)] {
            chmod(at, wide);
            let refused = guarded_ancestors(&dir, &base).expect_err("a writable ancestor");
            assert_eq!(refused.code(), "consent_dir_ancestor_too_open", "{wide:o} on {}: {refused}", at.display());
            chmod(at, 0o755);
        }
        // Above the base is not examined: that is the stated bound.
        chmod(&root, 0o777);
        guarded_ancestors(&dir, &base).expect("the bound is the base");
        chmod(&root, 0o755);
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
