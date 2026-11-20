//! **`mycelium dev consent` — the developer's side of a consent decision** (identity-and-lifecycle I7, PR 3a): a
//! software consent key kept off the worktree, a prompt rendered from the fleet's record, a bounded queue of
//! requests, and the signer that turns a person's typed answer into a signed grant decision.
//!
//! ## Who promises what here
//!
//! `dev` runs on the developer's device, and promises only its own behaviour:
//!
//! - **How the key is guarded.** The key is a software P-256 key in a `0600` file in a `0700` directory under the
//!   user's home ([`consent_dir`]), owned by the user, with one name, and with no directory above it (up to the
//!   home) that group or others can write (`mycelium_core::consent_key`). The directory's **real path** — every
//!   symbolic link resolved — has no `.git` at or above it ([`refuse_worktree`]). That is all the guarding there is:
//!   any process running as the user can read the file and sign, with no terminal and no person (I7's residual B).
//!   So `dev` signs with this key **only under a certificate graded `tty`** ([`Refusal::GradeNotThisKeys`]): `tty`
//!   says where the key lives, and `dev` will not sign under a certificate that says more about the key than `dev`
//!   does to it. The grade on a certificate stays the issuing operator's statement; `dev` tells the operator which
//!   grade describes this key, and refuses to use one that was certified as something else. The key and its
//!   certificate are read and checked again **before every prompt** ([`serve`]).
//! - **What it showed.** The prompt is rendered from the request record a gateway served under the request's id
//!   ([`PromptRecord`]) and from nothing a caller passed: a caller hands `dev` an id and no text. A request with no
//!   record prompts nothing. `dev` checks the record against itself before it shows anything: **the arguments must
//!   hash to the digest** the record names (the digest a grant binds), and the resource must be the record's tool at
//!   its node; a record whose arguments are withheld, cut or another call's prompts nothing
//!   ([`NoRecord::ArgumentsUnverified`]). The lines an approval is about — holder, tool, resource, digest, terms —
//!   are **the last thing shown**, below everything the requester controls; every field is cut to a bound that keeps
//!   them on one screen of a terminal at least 80 columns wide (the width is not read), and every character that is
//!   not printable ASCII is written as an escape, as is a space after a space ([`shown_text`], [`shown_id`]), so a
//!   record cannot repaint the terminal, scroll those lines away, or pad itself with blanks into rows that copy
//!   them. **What it still can:** arguments filled with a visible character (dots, say) can line up rows that read
//!   like those lines — plainly filled, inside the arguments, and above the real block, which is always last.
//! - **What it signed.** Only an answer typed at the terminal it opened (`/dev/tty`, [`TtyApprover`]), **after the
//!   prompt was shown**, is signed: `approve` signs an issue, `refuse` signs a refusal, and anything else — an empty
//!   line, end of input, no terminal — signs nothing and sends nothing. Whatever was typed before the prompt, entered
//!   or not, is discarded first. `dev` approves nothing on the person's behalf, and has no flag that would. It reads
//!   the record again after the answer and signs only if what it showed is still what the gateway serves
//!   ([`Answer::RecordChanged`]).
//!
//! What `dev` does **not** promise, because it is not `dev`'s to keep:
//!
//! - **That a person typed.** A terminal is a file descriptor; a process with a pseudo-terminal, or one that can
//!   write to the developer's terminal, types as well as a person does — after the prompt is shown as readily as
//!   before. `tty` is the grade of that, and the feature page says so.
//! - **That the gateway told the truth.** The record is whatever the gateway at `--gateway` answered, over whatever
//!   transport its URL names (plain `http://` by default). `dev` checks the record against itself, not against the
//!   requester: nothing signs the record.
//! - **What the grant will bind, past the second read.** Since identity-and-lifecycle I7-S the signed decision
//!   names what the prompt showed — the tool, the resource, the record's digest string and the expiry
//!   ([`PromptRecord::shown`], `grant_decision_message_shown`, a published format) — and the node it is addressed to
//!   compares them with **its own** copy of the request record, refusing a difference by name
//!   (`decision_names_another_request`). So a member who rewrites the record after the person typed `approve` no
//!   longer moves the approval onto another tool, digest, resource or expiry. **Still open** (#813's review, F1):
//!   the node remembers an answer only in the KV, so a member who tombstones it can have the same signed decision
//!   admitted again within the freshness window — for the same request now, not another. Closed by I7-S's second PR
//!   (a spent-decision memory); `CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN` blocks the release until then.
//! - **That the decision was admitted.** `dev` reports that it signed and sent, and quotes the gateway's answer as
//!   the gateway's ([`Answer::Signed`]).
//!
//! ## The queue
//!
//! Requests reach `dev` as ids, on its command line or one per line on its standard input, and wait in a bounded
//! queue ([`queue`], [`QUEUE_BOUND`]). An id offered while the queue is full is answered `busy` at once and never
//! prompted: a caller that floods `dev` cannot build a backlog of prompts for a person to click through. Stated
//! limits: ids on the command line are all offered before the first prompt, so of more than [`QUEUE_BOUND`] the
//! surplus are `busy`; the bound is on what waits, not on how many prompts a caller that keeps feeding ids can
//! raise over time, and ids carry no source to be fair between; `busy` is printed on `dev`'s standard output only —
//! the requesting agent learns nothing until its request lapses; and any member can keep a request from being
//! prompted — `ambiguous_request` by writing a second record with its id under another holder,
//! `arguments_unverified` or `record_unreadable` by rewriting the record's arguments or resource (availability only). Two `dev`
//! processes sharing one terminal are not serialised: each shows its prompt, and a typed answer goes to whichever
//! reads first — run one `dev` per terminal. How another process hands `dev` an id while it runs (N4's and N7's
//! transport) is theirs to build; this is the queue it feeds.
//!
//! ## The provisional gate
//!
//! Everything here refuses [`Refusal::ProvisionalOid`] in a build that reads no consent certificate
//! (`mycelium_core::tls::consent_grade_oid_open`), before a directory is resolved or a file touched.
//!
//! ## Not built
//!
//! The macOS Secure Enclave key and its Swift shim (I7's PR 3b — `os_presence(macos, unbound)` is that key's grade
//! and never this one's); Windows (no user-only ACL is set, so no key is kept there; the refusing arms are compiled
//! by no CI job); reading a macOS ACL on the key file or its directory (`mycelium_core::consent_key` says why); a
//! `recording` signer for a `sim` build (E6's); verifying the certificate's chain on the developer's side (the nodes
//! judge it); a home directory tracked by a repository whose `.git` is elsewhere (a bare dotfiles repository) is
//! not detected.

use mycelium_core::consent_key::{self, ConsentKeyRefusal, SoftwareConsentKey};
use mycelium_core::tls::ConsentGrade;
use std::path::{Path, PathBuf};

/// How many requests may wait behind the one at the prompt. One more is answered `busy`.
pub const QUEUE_BOUND: usize = 8;

/// The schema id of every line `mycelium dev consent` prints on its standard output.
pub const SCHEMA: &str = "mycelium.dev.consent/1";

/// How long a grant `dev` approves stands, unless the person asks otherwise: the decision route's own default.
pub const DEFAULT_TTL_SECS: u64 = 300;

/// The longest request id `dev` takes from its standard input, in bytes.
pub const REQUEST_ID_MAX: usize = 256;

/// The most of a gateway's answer `dev` reads, in bytes.
pub const GATEWAY_ANSWER_MAX: usize = 4 * 1024 * 1024;

/// The most characters of the arguments a prompt shows (five rows of an 80-column terminal).
pub const ARGUMENTS_SHOWN_MAX: usize = 400;

/// The most characters of an identifier field — request, holder, tool, resource, gateway, node — a prompt shows.
pub const ID_SHOWN_MAX: usize = 96;

/// Why `dev` did nothing. Each has a stable [`code`](Self::code).
#[derive(Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// This build reads no consent certificate, so it keeps no consent key and signs no consent decision.
    ProvisionalOid,
    /// No home directory is named by the environment, so there is nowhere outside a worktree to keep the key.
    DirUnresolved(&'static str),
    /// The consent directory's real path lies inside a worktree (the directory named holds `.git`).
    DirInWorktree { dir: PathBuf, worktree: PathBuf },
    /// The consent directory is no longer where it was resolved to: a component of its path became a link.
    DirMoved { dir: PathBuf, real: PathBuf },
    /// The key file's refusal ([`ConsentKeyRefusal`]).
    Key(ConsentKeyRefusal),
    /// No consent key is stored for the principal.
    NoKey(PathBuf),
    /// No certificate is stored for the principal: the key was never enrolled here.
    NotEnrolled { cert: PathBuf, public_key: PathBuf, issuer: String, name: String },
    /// The file is not one PEM consent certificate this build reads.
    NotAConsentCertificate(String),
    /// The certificate names another principal than the one asked for.
    AnotherPrincipal { certificate: String, asked: String },
    /// The certificate was issued over another key than the one stored here.
    EnrolledKeyMismatch { principal: String },
    /// The certificate states a grade a software key does not carry.
    GradeNotThisKeys { principal: String, certificate: ConsentGrade },
    /// The certificate's validity ended (its `notAfter`, ms since the epoch).
    CertificateExpired { principal: String, not_after_ms: u64 },
    /// The certificate's validity has not begun (its `notBefore`, ms since the epoch).
    CertificateNotYetValid { principal: String, not_before_ms: u64 },
    /// `--issuer` or `--name` is not a valid segment.
    BadName(String),
    /// The gateway could not be asked, or answered something `dev` cannot use.
    Gateway(String),
}

impl Refusal {
    /// The refusal's stable name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProvisionalOid => "consent_grade_oid_provisional",
            Self::DirUnresolved(_) => "consent_dir_unresolved",
            Self::DirInWorktree { .. } => "consent_dir_in_worktree",
            Self::DirMoved { .. } => "consent_dir_moved",
            Self::Key(k) => k.code(),
            Self::NoKey(_) => "no_consent_key",
            Self::NotEnrolled { .. } => "not_enrolled",
            Self::NotAConsentCertificate(_) => "not_a_consent_certificate",
            Self::AnotherPrincipal { .. } => "certificate_for_another_principal",
            Self::EnrolledKeyMismatch { .. } => "enrolled_key_mismatch",
            Self::GradeNotThisKeys { .. } => "grade_not_this_keys",
            Self::CertificateExpired { .. } => "certificate_expired",
            Self::CertificateNotYetValid { .. } => "certificate_not_yet_valid",
            Self::BadName(_) => "bad_name",
            Self::Gateway(_) => "gateway",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProvisionalOid => ConsentKeyRefusal::ProvisionalOid.fmt(f),
            Self::DirUnresolved(var) => write!(f, "no consent directory: {var} is not set, and a consent key is kept only under the user's home, never in the directory dev runs in"),
            Self::DirInWorktree { dir, worktree } => write!(
                f,
                "the consent directory {} is inside a worktree ({} holds .git): a consent key is never kept in a worktree, where a commit or a coding agent's tools would reach it",
                dir.display(),
                worktree.display()
            ),
            Self::DirMoved { dir, real } => write!(f, "the consent directory {} now resolves to {}: a component of its path became a link since it was resolved, and nothing is kept or read through it", dir.display(), real.display()),
            Self::Key(k) => k.fmt(f),
            Self::NoKey(p) => write!(f, "no consent key at {}: create one with `mycelium dev consent key`", p.display()),
            Self::NotEnrolled { cert, public_key, issuer, name } => write!(
                f,
                "no consent certificate at {}: have the key certified on the host that holds the fleet CA's key — `mycelium tls issue --ca-dir <ca-dir> --client {name} --consent --public-key {} --grade tty --issuer {issuer} --out <out-dir>` — then `mycelium dev consent enrol --issuer {issuer} --name {name} --cert <out-dir>/consent-{issuer}-{name}.cert.pem`",
                cert.display(),
                sh_quoted(&public_key.display().to_string())
            ),
            Self::NotAConsentCertificate(why) => write!(f, "not a consent certificate this build reads: {why}"),
            Self::AnotherPrincipal { certificate, asked } => write!(f, "the certificate is {certificate}'s, not {asked}'s"),
            Self::EnrolledKeyMismatch { principal } => write!(
                f,
                "the certificate for {principal} was issued over another key than the one stored here: nothing is signed under a certificate that does not name this key (if the key was replaced, have the new one certified and enrol again)"
            ),
            Self::GradeNotThisKeys { principal, certificate } => write!(
                f,
                "the certificate for {principal} states the grade `{certificate}`, and the key stored here is a software key in a file: its grade is `{}` and nothing else. dev signs nothing under a certificate that says more about the key than dev does to it; have the key certified with --grade {}",
                SoftwareConsentKey::GRADE,
                SoftwareConsentKey::GRADE
            ),
            Self::CertificateExpired { principal, not_after_ms } => write!(f, "the certificate for {principal} expired (its validity ended at {not_after_ms} ms since the epoch): every node refuses a decision under it; have the key certified again"),
            Self::CertificateNotYetValid { principal, not_before_ms } => write!(f, "the certificate for {principal} is not valid yet (its validity begins at {not_before_ms} ms since the epoch, by this device's clock): every node refuses a decision under it"),
            Self::BadName(why) => f.write_str(why),
            Self::Gateway(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Refusal {}

impl From<ConsentKeyRefusal> for Refusal {
    fn from(k: ConsentKeyRefusal) -> Self {
        match k {
            ConsentKeyRefusal::ProvisionalOid => Refusal::ProvisionalOid,
            other => Refusal::Key(other),
        }
    }
}

fn gate() -> Result<(), Refusal> {
    if mycelium_core::tls::consent_grade_oid_open() { Ok(()) } else { Err(Refusal::ProvisionalOid) }
}

/// The directory a consent key is kept in and the base it is under, from the environment `var` reads (the
/// process's, in the binary):
///
/// - macOS: `$HOME/Library/Application Support/mycelium/consent`, under `$HOME`;
/// - other unix: `$XDG_DATA_HOME/mycelium/consent` under `$XDG_DATA_HOME` when that is an absolute path, else
///   `$HOME/.local/share/mycelium/consent` under `$HOME`.
///
/// Never the current directory, and never a relative path: an unset or relative `HOME` is refused
/// ([`Refusal::DirUnresolved`]). The base is the bound of the ancestor check
/// (`mycelium_core::consent_key::guarded_ancestors`). Nothing is created or resolved here; [`Store::open`] resolves
/// links. Off unix the key store itself refuses.
pub fn consent_place(var: &dyn Fn(&str) -> Option<String>) -> Result<(PathBuf, PathBuf), Refusal> {
    gate()?;
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = absolute("HOME");
    if cfg!(target_os = "macos") {
        return home.map(|h| (h.join("Library").join("Application Support").join("mycelium").join("consent"), h)).ok_or(Refusal::DirUnresolved("HOME"));
    }
    if let Some(data) = absolute("XDG_DATA_HOME") {
        return Ok((data.join("mycelium").join("consent"), data));
    }
    home.map(|h| (h.join(".local").join("share").join("mycelium").join("consent"), h)).ok_or(Refusal::DirUnresolved("HOME"))
}

/// The directory of [`consent_place`].
pub fn consent_dir(var: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, Refusal> {
    consent_place(var).map(|(dir, _)| dir)
}

/// Refuse `dir` when it, or any directory above it, holds `.git` — a repository's worktree, a linked worktree (whose
/// `.git` is a file) or a home directory with a `.git` in it. **`dir` must be a real path** (links resolved:
/// [`Store`] passes one), or a link above it hides the repository it leads into. What this does not see: a
/// repository whose `.git` is elsewhere (`git --git-dir=… --work-tree=$HOME`, a bare dotfiles repository).
pub fn refuse_worktree(dir: &Path) -> Result<(), Refusal> {
    match dir.ancestors().find(|a| a.join(".git").exists()) {
        Some(worktree) => Err(Refusal::DirInWorktree { dir: dir.to_path_buf(), worktree: worktree.to_path_buf() }),
        None => Ok(()),
    }
}

/// Where one consent principal's key, public key and certificate are kept.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
    base: PathBuf,
    issuer: String,
    name: String,
}

impl Store {
    /// The store for `consent:{issuer}/{name}` under the environment's consent directory, **with every symbolic
    /// link in its path resolved**. Refused when the real directory lies in a worktree; nothing is created.
    pub fn open(var: &dyn Fn(&str) -> Option<String>, issuer: &str, name: &str) -> Result<Self, Refusal> {
        let (dir, base) = consent_place(var)?;
        Self::under(dir, base, issuer, name)
    }

    /// The store under `dir` — [`open`](Self::open) with the directory given; the directory is its own base.
    pub fn at(dir: PathBuf, issuer: &str, name: &str) -> Result<Self, Refusal> {
        Self::under(dir.clone(), dir, issuer, name)
    }

    fn under(dir: PathBuf, base: PathBuf, issuer: &str, name: &str) -> Result<Self, Refusal> {
        gate()?;
        if !mycelium_core::tls::valid_client_segment(issuer) || !mycelium_core::tls::valid_client_segment(name) {
            return Err(Refusal::BadName(format!("--issuer and --name are each 1-128 of A-Z a-z 0-9 - _ . (got {issuer:?} and {name:?})")));
        }
        let real = |p: &Path| consent_key::real_path(p).map_err(|e| Refusal::Key(ConsentKeyRefusal::Io(p.to_path_buf(), e)));
        let (dir, base) = (real(&dir)?, real(&base)?);
        refuse_worktree(&dir)?;
        Ok(Self { dir, base, issuer: issuer.to_string(), name: name.to_string() })
    }

    /// What is asked of the directory every time it is used, once it exists: its path still resolves to itself (no
    /// component became a link), its real path is in no worktree, and no directory above it up to the base can be
    /// written by group or others.
    fn guarded(&self) -> Result<(), Refusal> {
        let real = consent_key::real_path(&self.dir).map_err(|e| Refusal::Key(ConsentKeyRefusal::Io(self.dir.clone(), e)))?;
        if real != self.dir {
            return Err(Refusal::DirMoved { dir: self.dir.clone(), real });
        }
        refuse_worktree(&self.dir)?;
        consent_key::guarded_ancestors(&self.dir, &self.base)?;
        Ok(())
    }

    /// `consent:{issuer}/{name}`.
    pub fn principal(&self) -> String {
        format!("consent:{}/{}", self.issuer, self.name)
    }

    /// The files' common stem: the issuer and the name for a person to read, then the first 16 hex digits of the
    /// SHA-256 of the principal. `+` separates the parts and neither part can hold one (`a-b`/`c` and `a`/`b-c`
    /// shared a file under a `-`); the digest tells apart two principals that differ only in case, which are one
    /// name on a case-insensitive filesystem (macOS's default — `Fleet/x` replaced `fleet/x`'s key there). So two
    /// principals share a file only if 64 bits of SHA-256 collide.
    fn stem(&self) -> String {
        let digest: String = <sha2::Sha256 as sha2::Digest>::digest(self.principal().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("consent+{}+{}+{digest}", self.issuer, self.name)
    }

    /// The private key's file.
    pub fn key_path(&self) -> PathBuf {
        self.dir.join(format!("{}.key.p8", self.stem()))
    }

    /// The public key's file (SPKI PEM), which goes to the issuer.
    pub fn public_key_path(&self) -> PathBuf {
        self.dir.join(format!("{}.pub.pem", self.stem()))
    }

    /// The certificate's file.
    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(format!("{}.cert.pem", self.stem()))
    }

    /// Create the key and write its public half beside it. Refused when a key is there unless `replace`. The
    /// directory is created first and asked again whether it is where it should be, before a key is written.
    pub fn create_key(&self, replace: bool) -> Result<SoftwareConsentKey, Refusal> {
        let key = SoftwareConsentKey::generate()?;
        consent_key::create_dir(&self.dir)?;
        self.guarded()?;
        key.store(&self.key_path(), replace)?;
        consent_key::store_public(&self.public_key_path(), key.spki_pem().as_bytes())?;
        Ok(key)
    }

    fn key(&self) -> Result<SoftwareConsentKey, Refusal> {
        if !self.dir.exists() {
            return Err(Refusal::NoKey(self.key_path()));
        }
        self.guarded()?;
        match SoftwareConsentKey::load(&self.key_path()) {
            Err(ConsentKeyRefusal::Io(p, e)) if e.kind() == std::io::ErrorKind::NotFound => Err(Refusal::NoKey(p)),
            other => other.map_err(Refusal::from),
        }
    }

    /// Take the certificate the issuer returned: checked against the stored key ([`Enrolment`]'s rules), then kept
    /// beside it — **the certificate alone, re-encoded**: anything else in the file given (another PEM block, text)
    /// is neither kept nor, later, sent to a gateway. A certificate that fails a check is not stored.
    pub fn enrol(&self, cert_pem: &str) -> Result<Enrolment, Refusal> {
        let key = self.key()?;
        let enrolment = Enrolment::check(key, &self.principal(), cert_pem)?;
        consent_key::store_public(&self.cert_path(), enrolment.cert_pem().as_bytes())?;
        Ok(enrolment)
    }

    /// The stored key under its stored certificate, read and checked again **on every use**: the directory and the
    /// key file are still this user's private ones, and the certificate still names this principal, **this key**
    /// and the grade a software key carries, and has not expired.
    pub fn enrolment(&self) -> Result<Enrolment, Refusal> {
        let key = self.key()?;
        let not_enrolled = || Refusal::NotEnrolled { cert: self.cert_path(), public_key: self.public_key_path(), issuer: self.issuer.clone(), name: self.name.clone() };
        let pem = consent_key::load_public(&self.cert_path())?.ok_or_else(not_enrolled)?;
        let pem = String::from_utf8(pem).map_err(|_| Refusal::NotAConsentCertificate("the stored certificate is not text".into()))?;
        Enrolment::check(key, &self.principal(), &pem)
    }
}

/// A consent key with the certificate it signs under.
#[derive(Debug)]
pub struct Enrolment {
    key: SoftwareConsentKey,
    principal: String,
    cert_pem: String,
}

impl Enrolment {
    /// `key` under `cert_pem` as `principal`, or the named reason it is not: the PEM holds one consent certificate
    /// this build reads; it names `principal`; it was issued over `key`'s public point; its grade is the one a
    /// software key carries, `tty` — a certificate stating any other grade says something about the key that this
    /// device does not do, and nothing is signed under it; and it is valid now by this device's clock — both bounds
    /// read, and a certificate whose validity cannot be read refused.
    /// The chain is not judged here: the nodes judge it.
    pub fn check(key: SoftwareConsentKey, principal: &str, cert_pem: &str) -> Result<Self, Refusal> {
        gate()?;
        let der = one_certificate(cert_pem).map_err(Refusal::NotAConsentCertificate)?;
        let id = mycelium_core::tls::consent_identity(&der)
            .ok_or_else(|| Refusal::NotAConsentCertificate("it names no mycelium-consent:// principal over a P-256 key with a grade".into()))?;
        if id.principal() != principal {
            return Err(Refusal::AnotherPrincipal { certificate: id.principal(), asked: principal.to_string() });
        }
        if id.point != key.point() {
            return Err(Refusal::EnrolledKeyMismatch { principal: principal.to_string() });
        }
        if id.grade != SoftwareConsentKey::GRADE {
            return Err(Refusal::GradeNotThisKeys { principal: principal.to_string(), certificate: id.grade });
        }
        // Both bounds, by this device's clock, and a bound that cannot be read is a refusal: absence fails closed.
        let now_ms = mycelium_core::sim_seam::wall_now_ms();
        match mycelium_core::tls::cert_validity_ms(&der) {
            Some((Some(not_before_ms), Some(not_after_ms))) => {
                if not_after_ms <= now_ms {
                    return Err(Refusal::CertificateExpired { principal: principal.to_string(), not_after_ms });
                }
                if not_before_ms > now_ms {
                    return Err(Refusal::CertificateNotYetValid { principal: principal.to_string(), not_before_ms });
                }
            }
            _ => return Err(Refusal::NotAConsentCertificate("its validity (notBefore, notAfter) cannot be read, so it cannot be said to be valid now".into())),
        }
        Ok(Self { key, principal: principal.to_string(), cert_pem: certificate_pem(&der) })
    }

    /// The principal the key decides as.
    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// The grade every decision of this key names: the certificate's, which [`check`](Self::check) holds to `tty`.
    pub fn grade(&self) -> ConsentGrade {
        SoftwareConsentKey::GRADE
    }

    /// The certificate and nothing else, as PEM re-encoded from its DER: what is kept, and what the publication
    /// door is sent.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Sign `decision` (`issue` or `refuse`) on `record`, addressed to `node`, at `now_ms`, naming the grade and
    /// **what the prompt showed** inside the signature (identity-and-lifecycle I7-S): the record's tool, resource,
    /// arguments digest and expiry ([`PromptRecord::shown`]). The digest is the record's own string, which
    /// [`PromptRecord::find`] checked hashes from the arguments shown — so a request recorded in the pre-I3 form is
    /// named in that form, which is what the node compares with.
    pub fn sign(&self, node: &str, record: &PromptRecord, decision: &str, ttl_secs: Option<u64>, reason: Option<&str>, now_ms: u64) -> Result<crate::SignedDecision, Refusal> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let grade = self.grade().as_str();
        let shown = record.shown();
        let msg = crate::grant_decision_message_shown(&self.principal, node, &record.holder, &record.request, decision, ttl_secs, reason, now_ms, Some(grade), Some(&shown));
        let sig = self.key.sign(&msg)?;
        Ok(crate::SignedDecision::from_parts(self.principal.as_str(), node, record.holder.as_str(), record.request.as_str(), now_ms, b64.encode(self.key.point()), b64.encode(sig), Some(grade)).naming(shown))
    }
}

/// `text` as one word of a POSIX shell: in single quotes, a single quote inside written `'\''`. A path quoted in a
/// command a person is told to run must survive being pasted (macOS's consent directory holds a space).
pub fn sh_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The DER of the one PEM certificate in `pem`.
fn one_certificate(pem: &str) -> Result<Vec<u8>, String> {
    let blocks: Vec<&str> = pem.split("-----BEGIN CERTIFICATE-----").skip(1).collect();
    let [block] = blocks.as_slice() else {
        return Err(format!("expected exactly one PEM certificate, found {}", blocks.len()));
    };
    let b64: String = block.split("-----END CERTIFICATE-----").next().unwrap_or("").split_whitespace().collect();
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).map_err(|e| format!("the certificate is not base64: {e}"))
}

/// `der` as one PEM `CERTIFICATE` block.
fn certificate_pem(der: &[u8]) -> String {
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(&String::from_utf8_lossy(chunk));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// What a grant request's record says, as a gateway served it: the only source of a prompt's content. One exists
/// only for a record that agrees with itself ([`find`](Self::find)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRecord {
    /// The request's id.
    pub request: String,
    /// The holder: the principal the grant would be issued to.
    pub holder: String,
    /// The tool the record names.
    pub tool: String,
    /// The resource: `tool:{tool}@{node}`, checked.
    pub resource: String,
    /// The digest of the call's arguments the record names — **checked here against [`arguments`](Self::arguments)**;
    /// what the node's grant binds is the digest in the node's own copy of the record.
    pub arguments_digest: String,
    /// The arguments, as JSON text: the whole of what hashes to the digest.
    pub arguments: String,
    /// When the request lapses.
    pub expires_at_ms: u64,
    /// The record's KV key, `grant/{holder segment}/{request}`: where the decision is sent.
    pub key: String,
}

/// Why no prompt was raised for a request id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoRecord {
    /// No open request has this id.
    Unknown,
    /// More than one holder has an open request with this id: `dev` does not choose between them.
    Ambiguous(usize),
    /// A record has this id and lacks a field the prompt shows, or a field contradicts another.
    Unreadable(&'static str),
    /// The record's arguments do not hash to the digest it names — withheld by a data class, over the rendering
    /// cap, or simply not this digest's — so this device cannot show what the digest stands for.
    ArgumentsUnverified(&'static str),
}

impl PromptRecord {
    /// The record with id `request` among a gateway's open requests (`GET /gateway/grants/pending`'s `pending`).
    ///
    /// Every field the prompt shows must be present, and the record must agree with itself:
    /// - its key is `grant/{segment}/{request}`;
    /// - its resource is `tool:{tool}@{node}`;
    /// - **its arguments hash to its digest** — the RFC 8785 digest a grant binds, or the pre-I3 form for as long as
    ///   the nodes still honour that one (`gateway_authority::recorded_digest_matches`, `deprecations.md` §93; when
    ///   the nodes drop it, so does this). Arguments the record withholds (a data class the policy does not render)
    ///   or cuts (over the 16 KiB rendering cap) are an object that says so, which hashes to no call's digest: such
    ///   a request is **refused, not shown with a caveat** — a person cannot approve what this device cannot relate
    ///   to the digest. So are arguments holding a number of magnitude 2^53 or more, which the digest does not pin
    ///   exactly (the doors refuse them too).
    ///
    /// A record that fails any of these prompts nothing.
    pub fn find(pending: &serde_json::Value, request: &str) -> Result<Self, NoRecord> {
        let rows: Vec<&serde_json::Value> = pending.as_array().map(|a| a.iter().filter(|r| r["request"].as_str() == Some(request)).collect()).unwrap_or_default();
        let row = match rows.as_slice() {
            [] => return Err(NoRecord::Unknown),
            [row] => *row,
            many => return Err(NoRecord::Ambiguous(many.len())),
        };
        let text = |field: &'static str| row[field].as_str().filter(|s| !s.is_empty()).map(str::to_string).ok_or(NoRecord::Unreadable(field));
        let key = text("key")?;
        // The key names the holder's segment and the request: `grant/{segment}/{request}` and nothing else.
        let mut parts = key.split('/');
        if (parts.next(), parts.nth(1), parts.next()) != (Some("grant"), Some(request), None) {
            return Err(NoRecord::Unreadable("key"));
        }
        let (tool, resource, node) = (text("tool")?, text("resource")?, text("node")?);
        if resource != format!("tool:{tool}@{node}") {
            return Err(NoRecord::Unreadable("resource"));
        }
        let arguments_digest = text("arguments_digest")?;
        let arguments = &row["arguments"];
        if arguments.is_null() {
            return Err(NoRecord::ArgumentsUnverified("the record carries no arguments"));
        }
        // An integer beyond ±(2^53 − 1) is not pinned by the digest (RFC 8785 writes it as the double it rounds to, so
        // two neighbours share one): the request door and the call door refuse such arguments, and so does `dev`.
        if crate::agent::client_sig::holds_unsafe_integer(arguments) {
            return Err(NoRecord::ArgumentsUnverified("the arguments hold a number of magnitude 2^53 or more, which the digest does not pin exactly"));
        }
        if !crate::agent::gateway_authority::recorded_digest_matches(&row["arguments_digest"], arguments) {
            let why = if arguments.get("withheld").is_some() { "the record withholds or cuts the arguments, so they cannot be checked against the digest" } else { "the arguments in the record do not hash to the digest it names" };
            return Err(NoRecord::ArgumentsUnverified(why));
        }
        Ok(Self {
            request: request.to_string(),
            holder: text("agent")?,
            tool,
            resource,
            arguments_digest,
            arguments: arguments.to_string(),
            expires_at_ms: row["expires_at_ms"].as_u64().ok_or(NoRecord::Unreadable("expires_at_ms"))?,
            key,
        })
    }

    /// The holder's segment of the record's key: the decision route's `{agent}`.
    pub fn holder_segment(&self) -> &str {
        self.key.split('/').nth(1).unwrap_or_default()
    }

    /// What a decision on this record names of it (identity-and-lifecycle I7-S): its tool, resource, arguments
    /// digest (the record's string) and expiry — the fields the prompt shows that the node compares with its record.
    pub fn shown(&self) -> crate::Shown {
        crate::Shown::new(self.tool.as_str(), self.resource.as_str(), self.arguments_digest.as_str(), self.expires_at_ms)
    }

    /// Whether `other` is the same request saying the same thing, its expiry included (the prompt shows when it
    /// lapses, and the decision names it): what `dev` compares before it signs.
    pub fn says_the_same_as(&self, other: &Self) -> bool {
        (&self.request, &self.holder, &self.tool, &self.resource, &self.arguments_digest, &self.arguments, self.expires_at_ms, &self.key)
            == (&other.request, &other.holder, &other.tool, &other.resource, &other.arguments_digest, &other.arguments, other.expires_at_ms, &other.key)
    }
}

/// Whether a character is shown as itself: printable ASCII other than the backslash (which is written as its
/// escape, so an escape in the output is always one this module wrote). Everything else — every control, format,
/// separator and combining character, and **every letter and digit beyond ASCII** — is an escape.
fn shown_raw(c: char) -> bool {
    matches!(c, ' '..='~') && c != '\\'
}

/// `text` with each character shown as itself or as its `\u{…}` escape, cut so the result is at most `max`
/// characters, with what was cut said in a marker after it. A space is an escape when `spaces` is false, and in any
/// case when it follows another space. Returns the shown text and how many of `text`'s characters were not shown.
fn shown_within(text: &str, max: usize, spaces: bool) -> (String, usize) {
    let (mut out, mut used, mut taken, mut after_space) = (String::new(), 0usize, 0usize, false);
    for c in text.chars() {
        let raw = shown_raw(c) && (c != ' ' || (spaces && !after_space));
        let piece = if raw { c.to_string() } else { format!("\\u{{{:x}}}", c as u32) };
        let width = piece.chars().count();
        if used + width > max {
            break;
        }
        used += width;
        out.push_str(&piece);
        taken += 1;
        after_space = c == ' ';
    }
    let cut = text.chars().count() - taken;
    if cut > 0 {
        out.push_str(&format!(" …[cut: {cut} more characters not shown]"));
    }
    (out, cut)
}

/// Free text (the arguments) as a prompt shows it: at most `max` characters, **printable ASCII as itself and
/// everything else as its `\u{…}` escape**, and a space that follows a space as an escape too. So the text cannot
/// move the cursor, reorder or hide what is beside it (no right-to-left letter, no combining mark, no control or
/// format character reaches the terminal), pass a look-alike of another script or width for an ASCII character, or
/// pad itself with blanks into columns that imitate the prompt's own lines. (Padded with a visible character it
/// still can line up such rows; they are visibly filled, and the prompt's own block is below them, last.) **The price, chosen:** arguments in any script but
/// ASCII are shown as escapes, which a person cannot read as words; they are shown exactly, and never as something
/// they are not. (An allow-list of letters and digits was tried first; right-to-left letters, alphabetic combining
/// marks and full-width forms passed it — #813's confirm round, C3.)
pub fn shown_text(text: &str, max: usize) -> String {
    shown_within(text, max, true).0
}

/// An identifier (request, holder, tool, resource, gateway, node) as a prompt shows it: at most `max` characters,
/// printable ASCII as itself; every space, and every character beyond ASCII, is an escape.
pub fn shown_id(text: &str, max: usize) -> String {
    shown_within(text, max, false).0
}

/// What the person is asked: every line of it from the record, the gateway's own answers and `dev`'s enrolment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// The record the prompt shows.
    pub record: PromptRecord,
    /// The text, ready for a terminal.
    pub text: String,
}

impl Prompt {
    /// The prompt for `record`, to be decided as `principal` at `grade`, addressed to `node` through `gateway`, the
    /// grant standing `ttl_secs` if approved; `now_ms` only says how long the request has left.
    ///
    /// **The order is the defence.** What the requester controls freely — the arguments — comes first, cut to
    /// [`ARGUMENTS_SHOWN_MAX`]. What an approval is about — request, holder, tool, resource, digest, terms — comes
    /// **last**, each identifier cut to [`ID_SHOWN_MAX`], directly above the question: whatever a record holds, those
    /// lines are the bottom of the screen when the answer is typed — **on a terminal at least 80 columns wide**,
    /// where the request line to the question is at most fifteen rows. The terminal's width is not read: on a
    /// narrower one the block is longer (37 rows at 40 columns with every identifier at its bound), and its first
    /// lines can be above a 24-row screen.
    pub fn render(record: &PromptRecord, principal: &str, grade: ConsentGrade, gateway: &str, node: &str, ttl_secs: u64, now_ms: u64) -> Self {
        let left = record.expires_at_ms.saturating_sub(now_ms) / 1_000;
        let (arguments, cut) = shown_within(&record.arguments, ARGUMENTS_SHOWN_MAX, true);
        let checked = match cut {
            0 => "this device checked: the arguments shown above hash to it".to_string(),
            n => format!("this device checked: the whole arguments hash to it; {n} characters of them are NOT shown above"),
        };
        let id = |text: &str| shown_id(text, ID_SHOWN_MAX);
        let text = format!(
            "\nmycelium consent: a grant request. Its record, as {} serves it:\n\
             \x20 arguments  {arguments}\n\
             \x20 lapses     in {left} s\n\
             \x20 decided as {}, by a software key in a file (grade {grade}), addressed to node {}\n\
             What `approve` signs is about these lines:\n\
             \x20 request    {}\n\
             \x20 holder     {}\n\
             \x20 tool       {}\n\
             \x20 resource   {}\n\
             \x20 digest     {}\n\
             \x20            ({checked})\n\
             \x20 `approve` signs a decision to issue a grant for {ttl_secs} s. The node issues it, for the record it holds under this id.\n\
             Type `approve` to sign that, `refuse` to sign a refusal, or anything else to leave the request unanswered: ",
            id(gateway),
            id(principal),
            id(node),
            id(&record.request),
            id(&record.holder),
            id(&record.tool),
            id(&record.resource),
            id(&record.arguments_digest),
        );
        Self { record: record.clone(), text }
    }
}

/// What the person answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// `approve`, typed.
    Approve,
    /// `refuse`, typed.
    Refuse,
    /// Anything else: an empty line, another word, end of input.
    Dismissed,
    /// No terminal could be opened to ask at.
    NoTerminal,
}

impl Approval {
    /// The answer a typed line gives: exactly `approve` or `refuse` (surrounding whitespace aside), else dismissed.
    /// Nothing shorter, no default, no `y`.
    pub fn of_line(line: &str) -> Self {
        match line.trim() {
            "approve" => Approval::Approve,
            "refuse" => Approval::Refuse,
            _ => Approval::Dismissed,
        }
    }
}

/// Who is asked. The binary has one: [`TtyApprover`].
pub trait Approver {
    /// Show `prompt` and return the answer.
    fn ask(&mut self, prompt: &Prompt) -> Approval;
}

/// The terminal `dev` was started from, opened by name (`/dev/tty`) for each prompt: never standard input, which a
/// caller may have piped. Without a controlling terminal — a daemon, a CI job, a pipe-only child — nothing is asked
/// and nothing is signed.
#[derive(Debug, Default)]
pub struct TtyApprover;

impl Approver for TtyApprover {
    fn ask(&mut self, prompt: &Prompt) -> Approval {
        #[cfg(unix)]
        {
            use std::io::{BufRead as _, Write as _};
            let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return Approval::NoTerminal };
            // An answer is one typed after the prompt is shown: whatever waits at the terminal goes first — the lines
            // already entered (counted, so the prompt can say so), then everything the terminal still holds, entered
            // or not.
            let discarded = discard_typed_ahead(&tty);
            let mut out = &tty;
            let note = match discarded {
                0 => String::new(),
                n => format!("\n({n} characters typed before this prompt was shown were discarded)"),
            };
            if out.write_all(note.as_bytes()).and_then(|()| out.write_all(prompt.text.as_bytes())).and_then(|()| out.flush()).is_err() {
                return Approval::NoTerminal;
            }
            let mut line = String::new();
            // One line: in the terminal's line mode a read returns at most the line that was entered.
            match std::io::BufReader::new(&tty).read_line(&mut line) {
                Ok(n) if n > 0 => Approval::of_line(&line),
                _ => Approval::Dismissed,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = prompt;
            Approval::NoTerminal
        }
    }
}

/// Discard everything typed at the terminal before a prompt is written, and return how many bytes of entered lines
/// that was. Two steps: the lines already entered are read (non-blocking, through a second descriptor, at most
/// 64 KiB) so they can be counted; then the terminal's input queue is flushed (`tcflush(TCIFLUSH)`), which also
/// drops what was typed and **not yet entered** — otherwise `approve` typed while an earlier request was being
/// answered would become this prompt's answer on a bare return, with an empty line on the screen.
///
/// **What this is and is not.** It keeps a person's early or doubled keystrokes from answering a prompt they have
/// not seen. It is no barrier to a process that can type at the terminal: such a process types after the prompt is
/// shown, and is signed — which is what `tty` cannot promise.
#[cfg(unix)]
fn discard_typed_ahead(tty: &std::fs::File) -> usize {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut discarded = 0usize;
    if let Ok(mut entered) = std::fs::OpenOptions::new().read(true).custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32).open("/dev/tty") {
        let mut buf = [0u8; 1024];
        while discarded < 64 * 1024 {
            match entered.read(&mut buf) {
                Ok(n) if n > 0 => discarded += n,
                _ => break,
            }
        }
    }
    let _ = rustix::termios::tcflush(tty, rustix::termios::QueueSelector::IFlush);
    discarded
}

/// One request id from `input` (`dev`'s standard input): `None` at the end of input; `Some(Ok(id))` for a line of
/// at most [`REQUEST_ID_MAX`] bytes, trimmed (an empty one is for the caller to skip); `Some(Err(start))` for a
/// longer line, of which only the start is kept and the rest read and dropped, so one endless line costs no memory.
pub fn read_request_id(input: &mut impl std::io::BufRead) -> Option<Result<String, String>> {
    use std::io::{BufRead as _, Read as _};
    let mut line = Vec::new();
    match input.by_ref().take(REQUEST_ID_MAX as u64 + 1).read_until(b'\n', &mut line) {
        Ok(0) | Err(_) => return None,
        Ok(_) => {}
    }
    if line.last() != Some(&b'\n') && line.len() > REQUEST_ID_MAX {
        loop {
            let mut rest = Vec::new();
            match input.by_ref().take(4096).read_until(b'\n', &mut rest) {
                Ok(0) | Err(_) => break,
                Ok(_) if rest.last() == Some(&b'\n') => break,
                Ok(_) => {}
            }
        }
        return Some(Err(String::from_utf8_lossy(&line[..32]).into_owned()));
    }
    Some(Ok(String::from_utf8_lossy(&line).trim().to_string()))
}

/// The sending half of the request queue: [`offer`](Self::offer) an id, or learn at once that `dev` is busy.
#[derive(Clone, Debug)]
pub struct QueueHandle(tokio::sync::mpsc::Sender<String>);

/// The queue was full: the request was not taken and will not be prompted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

impl QueueHandle {
    /// Queue `request` for a prompt, or answer [`Busy`] when [`QUEUE_BOUND`] (the queue's bound) requests already
    /// wait. Never blocks.
    pub fn offer(&self, request: &str) -> Result<(), Busy> {
        self.0.try_send(request.to_string()).map_err(|_| Busy)
    }
}

/// The receiving half: what [`serve`] drains.
#[derive(Debug)]
pub struct Queue(tokio::sync::mpsc::Receiver<String>);

/// A queue holding at most `bound` waiting requests (the binary's is [`QUEUE_BOUND`]).
pub fn queue(bound: usize) -> (QueueHandle, Queue) {
    let (tx, rx) = tokio::sync::mpsc::channel(bound.max(1));
    (QueueHandle(tx), Queue(rx))
}

/// What became of one request id.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    /// The queue was full when the id arrived. Nothing was looked up or shown.
    Busy,
    /// A line on standard input longer than [`REQUEST_ID_MAX`] bytes: no id. Nothing was looked up or shown.
    IdTooLong,
    /// The key or its certificate was refused when read for this request (the refusal's code, and its text).
    /// Nothing was shown.
    Enrolment(&'static str, String),
    /// No record: nothing was shown.
    NoRecord(NoRecord),
    /// The gateway could not be read. Nothing was shown.
    Gateway(String),
    /// The prompt was shown and not answered with `approve` or `refuse`. Nothing was signed.
    Dismissed,
    /// No terminal to ask at. Nothing was signed.
    NoTerminal,
    /// The record the gateway serves changed while the prompt was up. Nothing was signed.
    RecordChanged,
    /// The key could not sign. Nothing was sent.
    NotSigned(String),
    /// `dev` signed `decision` and sent it; `status` and `gateway` are **the gateway's answer**, quoted.
    Signed { decision: &'static str, status: u16, gateway: serde_json::Value },
}

impl Answer {
    /// The answer's stable word.
    pub fn word(&self) -> &'static str {
        match self {
            Answer::Busy => "busy",
            Answer::IdTooLong => "request_id_too_long",
            Answer::Enrolment(..) => "enrolment_refused",
            Answer::NoRecord(NoRecord::Unknown) => "unknown_request",
            Answer::NoRecord(NoRecord::Ambiguous(_)) => "ambiguous_request",
            Answer::NoRecord(NoRecord::Unreadable(_)) => "record_unreadable",
            Answer::NoRecord(NoRecord::ArgumentsUnverified(_)) => "arguments_unverified",
            Answer::Gateway(_) => "gateway",
            Answer::Dismissed => "dismissed",
            Answer::NoTerminal => "no_terminal",
            Answer::RecordChanged => "record_changed",
            Answer::NotSigned(_) => "not_signed",
            Answer::Signed { .. } => "signed",
        }
    }

    /// Whether `dev` signed and the gateway admitted the decision.
    pub fn admitted(&self) -> bool {
        matches!(self, Answer::Signed { status, .. } if (200..300).contains(status))
    }

    /// The line `mycelium dev consent approve` prints for `request`: one JSON object, [`SCHEMA`].
    pub fn line(&self, request: &str) -> serde_json::Value {
        let mut line = serde_json::json!({ "schema": SCHEMA, "request": request, "answer": self.word() });
        match self {
            Answer::NoRecord(NoRecord::Ambiguous(n)) => line["detail"] = format!("{n} holders have an open request with this id").into(),
            Answer::NoRecord(NoRecord::Unreadable(field)) => line["detail"] = format!("the record lacks `{field}`, or it contradicts the rest of the record").into(),
            Answer::NoRecord(NoRecord::ArgumentsUnverified(why)) => line["detail"] = (*why).into(),
            Answer::IdTooLong => line["detail"] = format!("a request id is at most {REQUEST_ID_MAX} bytes; the line's start is given as the request").into(),
            Answer::Enrolment(code, why) => {
                line["refusal"] = (*code).into();
                line["detail"] = why.as_str().into();
            }
            Answer::Gateway(why) | Answer::NotSigned(why) => line["detail"] = why.as_str().into(),
            Answer::Signed { decision, status, gateway } => {
                line["decision"] = (*decision).into();
                line["gateway_status"] = (*status).into();
                line["gateway_answer"] = gateway.clone();
            }
            _ => {}
        }
        line
    }
}

/// A gateway, asked over HTTP with a bearer: `grant:read` to read the open requests, `grant:relay` to carry a signed
/// decision, `identity:write` to publish a certificate. The bearer signs nothing. **`dev` trusts this gateway, and
/// the path to it, for what the record says**: nothing signs the record, and `http://` is not confidential or
/// authenticated.
#[derive(Clone, Debug)]
pub struct Gateway {
    base: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl Gateway {
    /// A gateway at `base`. Every request is bounded (10 s, and [`GATEWAY_ANSWER_MAX`] bytes of answer), so a
    /// gateway that stops answering, or never stops, is a named failure.
    pub fn new(base: &str, token: Option<String>) -> Result<Self, Refusal> {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().map_err(|e| Refusal::Gateway(e.to_string()))?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token, client })
    }

    /// The gateway's address, as the prompt names it.
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<(u16, serde_json::Value), String> {
        let req = match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        };
        let mut resp = req.send().await.map_err(|e| format!("{}: {e}", self.base))?;
        let status = resp.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("{}: {e}", self.base))? {
            if body.len() + chunk.len() > GATEWAY_ANSWER_MAX {
                return Err(format!("{}: the answer is over {GATEWAY_ANSWER_MAX} bytes, and is not read further", self.base));
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body);
        Ok((status, serde_json::from_str(&text).unwrap_or_else(|_| serde_json::Value::String(text.into_owned()))))
    }

    async fn get(&self, route: &str) -> Result<serde_json::Value, String> {
        let (status, body) = self.send(self.client.get(format!("{}{route}", self.base))).await?;
        if (200..300).contains(&status) { Ok(body) } else { Err(format!("{}{route}: HTTP {status} {body}", self.base)) }
    }

    /// The id of the node behind the gateway (`GET /health`): the node a decision is addressed to.
    pub async fn node_id(&self) -> Result<String, String> {
        self.get("/health").await?["node_id"].as_str().map(str::to_string).ok_or_else(|| format!("{}/health names no node_id", self.base))
    }

    /// The gateway's open requests and its clock (`GET /gateway/grants/pending`).
    pub async fn pending(&self) -> Result<(serde_json::Value, u64), String> {
        let body = self.get("/gateway/grants/pending").await?;
        Ok((body["pending"].clone(), body["now_ms"].as_u64().unwrap_or(0)))
    }

    /// Send a decision on `record` (`POST /gateway/grants/{holder segment}/{request}`): the gateway's status and body.
    pub async fn decide(&self, record: &PromptRecord, body: &serde_json::Value) -> Result<(u16, serde_json::Value), String> {
        let mut url = reqwest::Url::parse(&self.base).map_err(|e| format!("{}: {e}", self.base))?;
        url.path_segments_mut().map_err(|()| format!("{}: not a base URL", self.base))?.pop_if_empty().extend(["gateway", "grants", record.holder_segment(), record.request.as_str()]);
        self.send(self.client.post(url).json(body)).await
    }

    /// Publish a consent certificate through the client-certificate door (`POST /gateway/identity/client-cert`).
    pub async fn publish(&self, cert_pem: &str) -> Result<(u16, serde_json::Value), String> {
        self.send(self.client.post(format!("{}/gateway/identity/client-cert", self.base)).json(&serde_json::json!({ "cert_pem": cert_pem }))).await
    }
}

/// Answer one request: read its record, show it, and sign what the person typed.
///
/// 1. The record is read from the gateway by `request`'s id; absent, ambiguous, unreadable, or with arguments that
///    do not hash to its digest, **nothing is shown**.
/// 2. The prompt is rendered from that record and shown by `approver`.
/// 3. Anything but `approve` or `refuse` signs nothing.
/// 4. The record is read again; if it no longer says what was shown, nothing is signed.
/// 5. The decision is signed with the enrolled key, naming its grade, and sent; the gateway's answer is returned as
///    the gateway's.
pub async fn answer_one(gateway: &Gateway, enrolment: &Enrolment, approver: &mut dyn Approver, request: &str, ttl_secs: u64) -> Answer {
    let read = || async {
        let (pending, now_ms) = gateway.pending().await.map_err(Answer::Gateway)?;
        PromptRecord::find(&pending, request).map(|r| (r, now_ms)).map_err(Answer::NoRecord)
    };
    let (record, now_ms) = match read().await {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let node = match gateway.node_id().await {
        Ok(node) => node,
        Err(why) => return Answer::Gateway(why),
    };
    let prompt = Prompt::render(&record, enrolment.principal(), enrolment.grade(), gateway.base(), &node, ttl_secs, now_ms);
    let (decision, ttl, reason): (&'static str, _, _) = match approver.ask(&prompt) {
        Approval::Approve => ("issue", Some(ttl_secs), None),
        Approval::Refuse => ("refuse", None, Some("refused at the developer's terminal")),
        Approval::Dismissed => return Answer::Dismissed,
        Approval::NoTerminal => return Answer::NoTerminal,
    };
    match read().await {
        Ok((again, _)) if again.says_the_same_as(&record) => {}
        Ok(_) | Err(Answer::NoRecord(_)) => return Answer::RecordChanged,
        Err(answer) => return answer,
    }
    let signed = match enrolment.sign(&node, &record, decision, ttl, reason, mycelium_core::sim_seam::wall_now_ms()) {
        Ok(signed) => signed,
        Err(why) => return Answer::NotSigned(why.to_string()),
    };
    let mut body = serde_json::json!({ "decision": decision, "signed": signed });
    if let Some(ttl) = ttl {
        body["ttl_secs"] = ttl.into();
    }
    if let Some(reason) = reason {
        body["reason"] = reason.into();
    }
    match gateway.decide(&record, &body).await {
        Ok((status, gateway)) => Answer::Signed { decision, status, gateway },
        Err(why) => Answer::Gateway(why),
    }
}

/// Drain `queue`, one prompt at a time, until every [`QueueHandle`] is dropped; `said` hears each answer as it is
/// made. **Before every prompt the key and its certificate are read from `store` and checked again**
/// ([`Store::enrolment`]): a key file widened, replaced or re-certified while `dev` runs is refused for the requests
/// that follow ([`Answer::Enrolment`]), and no key is held between prompts. Returns how many requests were signed and
/// admitted, and how many were not.
pub async fn serve(gateway: &Gateway, store: &Store, approver: &mut dyn Approver, mut queue: Queue, ttl_secs: u64, said: &mut dyn FnMut(&str, &Answer)) -> (usize, usize) {
    let (mut admitted, mut not) = (0, 0);
    while let Some(request) = queue.0.recv().await {
        let answer = match store.enrolment() {
            Ok(enrolment) => answer_one(gateway, &enrolment, approver, &request, ttl_secs).await,
            Err(refusal) => Answer::Enrolment(refusal.code(), refusal.to_string()),
        };
        if answer.admitted() { admitted += 1 } else { not += 1 }
        said(&request, &answer);
    }
    (admitted, not)
}

#[cfg(test)]
#[path = "dev_consent_tests.rs"]
mod tests;
