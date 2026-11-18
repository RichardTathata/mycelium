//! **`mycelium dev consent` — the developer's side of a consent decision** (identity-and-lifecycle I7, PR 3a): a
//! software consent key kept off the worktree, a prompt rendered from the fleet's record, a bounded queue of
//! requests, and the signer that turns a person's typed answer into a signed grant decision.
//!
//! ## Who promises what here
//!
//! `dev` runs on the developer's device, and promises only its own behaviour:
//!
//! - **How the key is guarded.** The key is a software P-256 key in a `0600` file in a `0700` directory under the
//!   user's home ([`consent_dir`]), never under a worktree ([`refuse_worktree`]). That is all the guarding there is:
//!   any process running as the user can read the file and sign, with no terminal and no person (I7's residual B).
//!   So `dev` signs with this key **only under a certificate graded `tty`** ([`Refusal::GradeNotThisKeys`]): `tty`
//!   says where the key lives, and `dev` will not sign under a certificate that says more about the key than `dev`
//!   does to it. The grade on a certificate stays the issuing operator's statement; `dev` tells the operator which
//!   grade describes this key, and refuses to use one that was certified as something else.
//! - **What it showed.** The prompt is rendered from the request record a gateway served under the request's id
//!   ([`PromptRecord`]) — the holder, the tool, the resource, the arguments' digest — and from nothing a caller
//!   passed: a caller hands `dev` an id and no text. A request with no record prompts nothing. The record's text is
//!   escaped before it is shown ([`shown`]), so a record cannot repaint the terminal.
//! - **What it signed.** Only an answer typed at the terminal it opened (`/dev/tty`, [`TtyApprover`]) is signed:
//!   `approve` issues, `refuse` refuses, and anything else — an empty line, end of input, no terminal — signs
//!   nothing and sends nothing. A line entered **before** a prompt was shown is discarded, not taken as its answer. `dev` approves nothing on the person's behalf, and has no flag that would.
//!   It reads the record again after the answer and signs only if what it showed is still what the gateway serves
//!   ([`Answer::RecordChanged`]).
//!
//! What `dev` does **not** promise, because it is not `dev`'s to keep:
//!
//! - **That a person typed.** A terminal is a file descriptor; a process with a pseudo-terminal types as well as a
//!   person does. `tty` is the grade of that, and the feature page says so.
//! - **What the grant will bind.** The signed decision names the request's id, holder and terms
//!   (`grant_decision_message_graded`, a published format), not the tool or the digest. The node the decision is
//!   addressed to issues the grant from **its own** copy of the request record, which is unsigned in the KV
//!   (`owner-signed-records.md`, row `grant/{agent}/{request}`, closed by plan row I4a). `dev` shows what a gateway
//!   served and re-reads it before signing; it cannot promise the addressed node reads the same.
//! - **That the decision was admitted.** `dev` reports that it signed and sent, and quotes the gateway's answer as
//!   the gateway's ([`Answer::Signed`]).
//!
//! ## The queue
//!
//! Requests reach `dev` as ids, on its command line or one per line on its standard input, and wait in a bounded
//! queue ([`queue`], [`QUEUE_BOUND`]). An id offered while the queue is full is answered `busy` at once and never
//! prompted: a caller that floods `dev` cannot build a backlog of prompts for a person to click through. How another
//! process hands `dev` an id while it runs (N4's and N7's transport) is theirs to build; this is the queue it feeds.
//!
//! ## The provisional gate
//!
//! Everything here refuses [`Refusal::ProvisionalOid`] in a build that reads no consent certificate
//! (`mycelium_core::tls::consent_grade_oid_open`), before a directory is resolved or a file touched.
//!
//! ## Not built
//!
//! The macOS Secure Enclave key and its Swift shim (I7's PR 3b — `os_presence(macos, unbound)` is that key's grade
//! and never this one's); Windows (no user-only ACL is set, so no key is kept there); a `recording` signer for a
//! `sim` build (E6's); verifying the certificate's chain on the developer's side (the nodes judge it).

use mycelium_core::consent_key::{self, ConsentKeyRefusal, SoftwareConsentKey};
use mycelium_core::tls::ConsentGrade;
use std::path::{Path, PathBuf};

/// How many requests may wait behind the one at the prompt. One more is answered `busy`.
pub const QUEUE_BOUND: usize = 8;

/// The schema id of every line `mycelium dev consent` prints on its standard output.
pub const SCHEMA: &str = "mycelium.dev.consent/1";

/// How long a grant `dev` approves stands, unless the person asks otherwise: the decision route's own default.
pub const DEFAULT_TTL_SECS: u64 = 300;

/// The longest a record's field is shown in full; the digest names the whole.
const SHOWN_MAX: usize = 2_000;

/// Why `dev` did nothing. Each has a stable [`code`](Self::code).
#[derive(Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// This build reads no consent certificate, so it keeps no consent key and signs no consent decision.
    ProvisionalOid,
    /// No home directory is named by the environment, so there is nowhere outside a worktree to keep the key.
    DirUnresolved(&'static str),
    /// The consent directory lies inside a worktree (the directory named holds `.git`).
    DirInWorktree { dir: PathBuf, worktree: PathBuf },
    /// The key file's refusal ([`ConsentKeyRefusal`]).
    Key(ConsentKeyRefusal),
    /// No consent key is stored for the principal.
    NoKey(PathBuf),
    /// No certificate is stored for the principal: the key was never enrolled here.
    NotEnrolled(PathBuf),
    /// The file is not one PEM consent certificate this build reads.
    NotAConsentCertificate(String),
    /// The certificate names another principal than the one asked for.
    AnotherPrincipal { certificate: String, asked: String },
    /// The certificate was issued over another key than the one stored here.
    EnrolledKeyMismatch { principal: String },
    /// The certificate states a grade a software key does not carry.
    GradeNotThisKeys { principal: String, certificate: ConsentGrade },
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
            Self::Key(k) => k.code(),
            Self::NoKey(_) => "no_consent_key",
            Self::NotEnrolled(_) => "not_enrolled",
            Self::NotAConsentCertificate(_) => "not_a_consent_certificate",
            Self::AnotherPrincipal { .. } => "certificate_for_another_principal",
            Self::EnrolledKeyMismatch { .. } => "enrolled_key_mismatch",
            Self::GradeNotThisKeys { .. } => "grade_not_this_keys",
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
            Self::Key(k) => k.fmt(f),
            Self::NoKey(p) => write!(f, "no consent key at {}: create one with `mycelium dev consent key`", p.display()),
            Self::NotEnrolled(p) => write!(f, "no consent certificate at {}: have the key certified (`mycelium tls issue --client <name> --consent --public-key <pub.pem> --grade tty`, on the host that holds the fleet CA's key), then `mycelium dev consent enrol --cert <cert.pem>`", p.display()),
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

/// The directory a consent key is kept in, from the environment `var` reads (the process's, in the binary):
///
/// - macOS: `$HOME/Library/Application Support/mycelium/consent`;
/// - other unix: `$XDG_DATA_HOME/mycelium/consent` when `XDG_DATA_HOME` is an absolute path, else
///   `$HOME/.local/share/mycelium/consent`.
///
/// Never the current directory, and never a relative path: an unset or relative `HOME` is refused
/// ([`Refusal::DirUnresolved`]). Nothing is created here. Off unix the key store itself refuses.
pub fn consent_dir(var: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, Refusal> {
    gate()?;
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = absolute("HOME");
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library").join("Application Support").join("mycelium").join("consent")).ok_or(Refusal::DirUnresolved("HOME"));
    }
    if let Some(data) = absolute("XDG_DATA_HOME") {
        return Ok(data.join("mycelium").join("consent"));
    }
    home.map(|h| h.join(".local").join("share").join("mycelium").join("consent")).ok_or(Refusal::DirUnresolved("HOME"))
}

/// Refuse `dir` when it, or any directory above it, holds `.git` — a repository's worktree, a linked worktree (whose
/// `.git` is a file) or a home directory kept under version control. The key is never kept where a commit could take
/// it.
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
    issuer: String,
    name: String,
}

impl Store {
    /// The store for `consent:{issuer}/{name}` under the environment's consent directory. Refused when the directory
    /// lies in a worktree; nothing is created.
    pub fn open(var: &dyn Fn(&str) -> Option<String>, issuer: &str, name: &str) -> Result<Self, Refusal> {
        let dir = consent_dir(var)?;
        Self::at(dir, issuer, name)
    }

    /// The store under `dir` — [`open`](Self::open) with the directory already resolved.
    pub fn at(dir: PathBuf, issuer: &str, name: &str) -> Result<Self, Refusal> {
        gate()?;
        if !mycelium_core::tls::valid_client_segment(issuer) || !mycelium_core::tls::valid_client_segment(name) {
            return Err(Refusal::BadName(format!("--issuer and --name are each 1-128 of A-Z a-z 0-9 - _ . (got {issuer:?} and {name:?})")));
        }
        refuse_worktree(&dir)?;
        Ok(Self { dir, issuer: issuer.to_string(), name: name.to_string() })
    }

    /// `consent:{issuer}/{name}`.
    pub fn principal(&self) -> String {
        format!("consent:{}/{}", self.issuer, self.name)
    }

    /// The private key's file.
    pub fn key_path(&self) -> PathBuf {
        self.dir.join(format!("consent-{}-{}.key.p8", self.issuer, self.name))
    }

    /// The public key's file (SPKI PEM), which goes to the issuer.
    pub fn public_key_path(&self) -> PathBuf {
        self.dir.join(format!("consent-{}-{}.pub.pem", self.issuer, self.name))
    }

    /// The certificate's file, as the issuer names it.
    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(format!("consent-{}-{}.cert.pem", self.issuer, self.name))
    }

    /// Create the key and write its public half beside it. Refused when a key is there unless `replace`.
    pub fn create_key(&self, replace: bool) -> Result<SoftwareConsentKey, Refusal> {
        let key = SoftwareConsentKey::generate()?;
        key.store(&self.key_path(), replace)?;
        consent_key::store_public(&self.public_key_path(), key.spki_pem().as_bytes())?;
        Ok(key)
    }

    fn key(&self) -> Result<SoftwareConsentKey, Refusal> {
        match SoftwareConsentKey::load(&self.key_path()) {
            Err(ConsentKeyRefusal::Io(p, e)) if e.kind() == std::io::ErrorKind::NotFound => Err(Refusal::NoKey(p)),
            other => other.map_err(Refusal::from),
        }
    }

    /// Take the certificate the issuer returned: checked against the stored key ([`Enrolment`]'s rules), then kept
    /// beside it. A certificate that fails a check is not stored.
    pub fn enrol(&self, cert_pem: &str) -> Result<Enrolment, Refusal> {
        let key = self.key()?;
        let enrolment = Enrolment::check(key, &self.principal(), cert_pem)?;
        consent_key::store_public(&self.cert_path(), cert_pem.as_bytes())?;
        Ok(enrolment)
    }

    /// The stored key under its stored certificate, checked again **on every use**: the certificate still names this
    /// principal, **this key** and the grade a software key carries.
    pub fn enrolment(&self) -> Result<Enrolment, Refusal> {
        let key = self.key()?;
        let pem = consent_key::load_public(&self.cert_path())?.ok_or_else(|| Refusal::NotEnrolled(self.cert_path()))?;
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
    /// this build reads; it names `principal`; it was issued over `key`'s public point; and its grade is the one a
    /// software key carries, `tty` — a certificate stating any other grade says something about the key that this
    /// device does not do, and nothing is signed under it. The chain is not judged here: the nodes judge it.
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
        Ok(Self { key, principal: principal.to_string(), cert_pem: cert_pem.to_string() })
    }

    /// The principal the key decides as.
    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// The grade every decision of this key names: the certificate's, which [`check`](Self::check) holds to `tty`.
    pub fn grade(&self) -> ConsentGrade {
        SoftwareConsentKey::GRADE
    }

    /// The certificate, as PEM, for the publication door.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Sign `decision` (`issue` or `refuse`) on `record`, addressed to `node`, at `now_ms`, naming the grade inside
    /// the signature.
    pub fn sign(&self, node: &str, record: &PromptRecord, decision: &str, ttl_secs: Option<u64>, reason: Option<&str>, now_ms: u64) -> Result<crate::SignedDecision, Refusal> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let grade = self.grade().as_str();
        let msg = crate::grant_decision_message_graded(&self.principal, node, &record.holder, &record.request, decision, ttl_secs, reason, now_ms, Some(grade));
        let sig = self.key.sign(&msg)?;
        Ok(crate::SignedDecision::from_parts(self.principal.as_str(), node, record.holder.as_str(), record.request.as_str(), now_ms, b64.encode(self.key.point()), b64.encode(sig), Some(grade)))
    }
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

/// What a grant request's record says, as a gateway served it: the only source of a prompt's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRecord {
    /// The request's id.
    pub request: String,
    /// The holder: the principal the grant would be issued to.
    pub holder: String,
    /// The tool the grant would admit.
    pub tool: String,
    /// The resource (`tool:{tool}@{node}`).
    pub resource: String,
    /// The digest of the call's arguments the grant would bind.
    pub arguments_digest: String,
    /// The arguments as the record renders them (JSON text), when it renders them.
    pub arguments: Option<String>,
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
    /// A record has this id and lacks a field the prompt shows.
    Unreadable(&'static str),
}

impl PromptRecord {
    /// The record with id `request` among a gateway's open requests (`GET /gateway/grants/pending`'s `pending`).
    /// Every field the prompt shows must be present and a string: a record missing one prompts nothing.
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
        Ok(Self {
            request: request.to_string(),
            holder: text("agent")?,
            tool: text("tool")?,
            resource: text("resource")?,
            arguments_digest: text("arguments_digest")?,
            arguments: (!row["arguments"].is_null()).then(|| row["arguments"].to_string()),
            expires_at_ms: row["expires_at_ms"].as_u64().ok_or(NoRecord::Unreadable("expires_at_ms"))?,
            key,
        })
    }

    /// The holder's segment of the record's key: the decision route's `{agent}`.
    pub fn holder_segment(&self) -> &str {
        self.key.split('/').nth(1).unwrap_or_default()
    }

    /// Whether `other` is the same request saying the same thing: what `dev` compares before it signs.
    pub fn says_the_same_as(&self, other: &Self) -> bool {
        (&self.request, &self.holder, &self.tool, &self.resource, &self.arguments_digest, &self.arguments, &self.key)
            == (&other.request, &other.holder, &other.tool, &other.resource, &other.arguments_digest, &other.arguments, &other.key)
    }
}

/// `text` as a prompt shows it: every control character, and every character that reorders or hides text (the
/// Unicode bidirectional and zero-width formatting characters), written as its `\u{…}` escape, so a record's content
/// cannot move the cursor, clear the screen or disguise one word as another; cut at [`SHOWN_MAX`] characters with the
/// count that was cut.
pub fn shown(text: &str) -> String {
    let hidden = |c: char| c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{00AD}');
    let mut out = String::new();
    let mut chars = text.chars();
    for c in chars.by_ref().take(SHOWN_MAX) {
        if hidden(c) {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    let cut = chars.count();
    if cut > 0 {
        out.push_str(&format!(" … ({cut} more characters not shown; the digest names the whole)"));
    }
    out
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
    pub fn render(record: &PromptRecord, principal: &str, grade: ConsentGrade, gateway: &str, node: &str, ttl_secs: u64, now_ms: u64) -> Self {
        let left = record.expires_at_ms.saturating_sub(now_ms) / 1_000;
        let arguments = match &record.arguments {
            Some(a) => shown(a),
            None => "(not rendered in the record; the digest names them)".to_string(),
        };
        let text = format!(
            "\nmycelium consent: a grant request, as {gateway} serves its record\n\
             \x20 request    {}\n\
             \x20 holder     {}\n\
             \x20 tool       {}\n\
             \x20 resource   {}\n\
             \x20 arguments  {}\n\
             \x20 digest     {}\n\
             \x20 lapses     in {left} s\n\
             \x20 decided as {}, by a software key in a file (grade {grade}), addressed to node {}\n\
             \x20 approving issues a grant for {ttl_secs} s; the node issues it for the record it holds under this id\n\
             Type `approve` to issue, `refuse` to refuse, or anything else to leave the request unanswered: ",
            shown(&record.request),
            shown(&record.holder),
            shown(&record.tool),
            shown(&record.resource),
            arguments,
            shown(&record.arguments_digest),
            shown(principal),
            shown(node),
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
            // An answer is one typed after the prompt is shown: whatever waits at the terminal is discarded first.
            let discarded = discard_typed_ahead();
            let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return Approval::NoTerminal };
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

/// Read and discard every line already entered at the terminal (`/dev/tty`, opened non-blocking for this alone), and
/// return how many bytes that was. Called before a prompt is written, so a line typed while an earlier request was
/// being answered — a second `approve`, a pasted block — is never taken as the answer to a prompt that had not been
/// shown yet. Characters typed and not yet entered (no newline) are the line discipline's and stay; they become an
/// answer only when the person presses return, with the prompt on the screen. At most 64 KiB is read.
#[cfg(unix)]
fn discard_typed_ahead() -> usize {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let Ok(mut tty) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/tty") else { return 0 };
    let (mut discarded, mut buf) = (0usize, [0u8; 1024]);
    while discarded < 64 * 1024 {
        match tty.read(&mut buf) {
            Ok(n) if n > 0 => discarded += n,
            _ => break,
        }
    }
    discarded
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
            Answer::NoRecord(NoRecord::Unknown) => "unknown_request",
            Answer::NoRecord(NoRecord::Ambiguous(_)) => "ambiguous_request",
            Answer::NoRecord(NoRecord::Unreadable(_)) => "record_unreadable",
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
            Answer::NoRecord(NoRecord::Unreadable(field)) => line["detail"] = format!("the record lacks `{field}`").into(),
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
/// decision, `identity:write` to publish a certificate. The bearer signs nothing.
#[derive(Clone, Debug)]
pub struct Gateway {
    base: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl Gateway {
    /// A gateway at `base`. Every request is bounded (10 s), so a gateway that stops answering is a named failure.
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
        let resp = req.send().await.map_err(|e| format!("{}: {e}", self.base))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))))
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
/// 1. The record is read from the gateway by `request`'s id; absent, ambiguous or unreadable, **nothing is shown**.
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
/// made. Returns how many requests were signed and admitted, and how many were not.
pub async fn serve(gateway: &Gateway, enrolment: &Enrolment, approver: &mut dyn Approver, mut queue: Queue, ttl_secs: u64, said: &mut dyn FnMut(&str, &Answer)) -> (usize, usize) {
    let (mut admitted, mut not) = (0, 0);
    while let Some(request) = queue.0.recv().await {
        let answer = answer_one(gateway, enrolment, approver, &request, ttl_secs).await;
        if answer.admitted() { admitted += 1 } else { not += 1 }
        said(&request, &answer);
    }
    (admitted, not)
}

#[cfg(test)]
#[path = "dev_consent_tests.rs"]
mod tests;
