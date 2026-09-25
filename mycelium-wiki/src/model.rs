//! The wiki record model — pages, sections, the manifest, and the query predicate. Serde-serialised
//! into the pluggable store; substrate-agnostic (no Mycelium types here).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A **stable, opaque** section id — minted once at section creation and never changed, so "edit
/// section X" and "rename/move section X" stay independent. Not derived from the heading or body.
pub type SectionId = Arc<str>;

/// One section: an editable heading + prose body, plus structured **attributes** — the *join keys*
/// (the shared id namespace with the external metrics store, e.g. `node = e_rl_rk`) and cross-cutting
/// *scope tags* (`topic = accountability`, `issue = climate`). Attributes are **not** typed facets
/// for computation (those live in the metrics store); they are how the meaning layer is located and
/// joined to the structure layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id:      SectionId,
    pub heading: String,
    pub body:    String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// Page-level metadata: the section render `order` + page attributes. The curator writes this
/// **last** (manifest-last), so a direct reader entering via the manifest never observes a
/// half-applied multi-section edit — the store's per-object atomicity does the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub order: Vec<SectionId>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// A page as read: the manifest joined with its live section bodies, in render order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page {
    pub path:       String,
    pub attributes: BTreeMap<String, String>,
    pub sections:   Vec<Section>,
}

/// A lightweight [`query`](crate::WikiStore::query) hit — which section on which page matched, without
/// the full-page join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionRef {
    pub page:       String,
    pub id:         SectionId,
    pub heading:    String,
    pub attributes: BTreeMap<String, String>,
}

/// An attribute predicate for `query`: a section matches when it carries **every** `(key, value)`
/// pair (all-of / AND). Empty predicate matches all. Mirrors the blackboard's attribute predicate;
/// the retrieval is structured filter, **not** embedding similarity (that is RAG's job).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Predicate {
    pub equals: BTreeMap<String, String>,
}

impl Predicate {
    pub fn new() -> Self { Self::default() }
    /// Add a required `key == value` constraint (builder style).
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.equals.insert(key.into(), value.into());
        self
    }
    /// Does `attributes` satisfy every constraint?
    pub fn matches(&self, attributes: &BTreeMap<String, String>) -> bool {
        self.equals.iter().all(|(k, v)| attributes.get(k) == Some(v))
    }
}

/// Errors from a [`WikiStore`](crate::WikiStore).
#[derive(Debug)]
pub enum WikiError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    /// A page path that escapes the store root (`..`, absolute) — rejected.
    BadPath(String),
    /// A compare-and-swap write lost the race: the object's on-store version moved since the
    /// `expected` version the caller read (or the object already exists for an `expected = None`
    /// create). **Not an error to log-and-drop** — the caller must re-read the committed state and
    /// re-apply (idempotently), so a concurrent writer (e.g. a transient split-brain curator) can never
    /// silently clobber a landed edit. The store is at-least-once/never-lose without a distributed lock;
    /// exactly-once *effect* comes from the caller's idempotent reconcile.
    Conflict,
}

impl std::fmt::Display for WikiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WikiError::Io(e)      => write!(f, "wiki store io: {e}"),
            WikiError::Serde(e)   => write!(f, "wiki store serde: {e}"),
            WikiError::BadPath(p) => write!(f, "wiki store: unsafe page path {p:?}"),
            WikiError::Conflict   => write!(f, "wiki store: compare-and-swap version conflict (re-read and retry)"),
        }
    }
}
impl std::error::Error for WikiError {}
impl From<std::io::Error>    for WikiError { fn from(e: std::io::Error)    -> Self { WikiError::Io(e) } }
impl From<serde_json::Error> for WikiError { fn from(e: serde_json::Error) -> Self { WikiError::Serde(e) } }

/// A write refused by a deployment **write gate** (e.g. the council-wiki Node validator run by
/// `GitStore`'s `validate_cmd` — council-substrate Phase 3). Carried inside [`WikiError::Io`] as the
/// error's inner payload rather than a new enum variant (additive, no downstream match breaks);
/// construct with [`WikiError::gate_refusal`] and detect with [`WikiError::as_gate_refusal`].
#[derive(Debug)]
pub struct GateRefusal(pub String);

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "write refused by the deployment gate: {}", self.0)
    }
}
impl std::error::Error for GateRefusal {}

/// A write refused by the **mandate fence** (v3 item 5): the appointment this writer was
/// configured with is no longer the one the resource holds.
///
/// Distinct from [`WikiError::Conflict`] because the remedy is the opposite. A conflict says
/// *another writer got there first* — re-read and re-apply, and you will land. A revoked mandate
/// says *you are no longer the curator* — re-applying will refuse forever, and the right response
/// is to stop writing and find out who holds the appointment now. Item 5's record is explicit that
/// a refusal naming the wrong thing invites a fix that does not help; before this existed, a
/// revoked curator was told to "re-read and retry".
///
/// Carried inside [`WikiError::Io`] like [`GateRefusal`], so adding it breaks no downstream match.
#[derive(Debug)]
pub struct MandateRevoked {
    /// The ref that carries the appointment, e.g. `refs/mycelium/mandate/norfolk`.
    pub refname: String,
    /// What this writer was configured to expect.
    pub expected: String,
    /// What the resource actually holds now — `None` if the ref is gone entirely.
    pub found: Option<String>,
}

impl std::fmt::Display for MandateRevoked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.found {
            Some(found) => write!(
                f,
                "write refused by the mandate fence: {} holds {found}, this writer was appointed under {}",
                self.refname, self.expected
            ),
            None => write!(
                f,
                "write refused by the mandate fence: {} does not exist; this writer was appointed under {}",
                self.refname, self.expected
            ),
        }
    }
}
impl std::error::Error for MandateRevoked {}

/// A write refused by the store's **execution authority** (Boundary H item A1,
/// `docs/design/authority-at-execution.md`): at the moment of the write, the writer could not
/// show present authority. The mandate's window closed, its epoch was superseded, it was revoked,
/// or the writer had no fresh revocation checkpoint and so could not know it was not revoked.
///
/// Distinct from both neighbours. Not a [`WikiError::Conflict`]: re-reading and re-applying will
/// not help. Not a [`GateRefusal`]: the content is not at fault, so the proposals it came from must
/// stay queued for a writer that does hold authority, never be dropped. And not a
/// [`MandateRevoked`]: that is the fence finding the appointment ref moved inside the git
/// transaction, whereas this is the time and revocation check made just before it.
///
/// Carried inside [`WikiError::Io`] like the others, so adding it breaks no downstream match.
#[derive(Debug)]
pub struct AuthorityRefused(pub String);

impl std::fmt::Display for AuthorityRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "write refused: no present authority at execution ({})", self.0)
    }
}
impl std::error::Error for AuthorityRefused {}

impl WikiError {
    /// A gate refusal carrying the validator's findings. **Not a retry signal**: unlike
    /// [`Conflict`](WikiError::Conflict), re-applying the same content will refuse again — the
    /// curator drops the refused proposals (with the findings logged) instead of re-queueing them.
    pub fn gate_refusal(findings: impl Into<String>) -> Self {
        WikiError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, GateRefusal(findings.into())))
    }

    /// The gate findings, when this error is a [`gate_refusal`](WikiError::gate_refusal). A real
    /// filesystem `PermissionDenied` carries no [`GateRefusal`] payload and returns `None` here.
    pub fn as_gate_refusal(&self) -> Option<&str> {
        match self {
            WikiError::Io(e) => e.get_ref().and_then(|r| r.downcast_ref::<GateRefusal>()).map(|g| g.0.as_str()),
            _ => None,
        }
    }

    /// A mandate-fence refusal. **Not a retry signal**, and the difference from
    /// [`Conflict`](WikiError::Conflict) is the whole point — see [`MandateRevoked`].
    pub fn mandate_revoked(refname: impl Into<String>, expected: impl Into<String>, found: Option<String>) -> Self {
        WikiError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            MandateRevoked { refname: refname.into(), expected: expected.into(), found },
        ))
    }

    /// The store's execution authority refused the write (A1). **Not a retry signal and not a
    /// content fault** — see [`AuthorityRefused`].
    pub fn authority_refused(reason: impl Into<String>) -> Self {
        WikiError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, AuthorityRefused(reason.into())))
    }

    /// The reason, when this error is an [`authority_refused`](WikiError::authority_refused).
    pub fn as_authority_refused(&self) -> Option<&str> {
        match self {
            WikiError::Io(e) => e.get_ref().and_then(|r| r.downcast_ref::<AuthorityRefused>()).map(|a| a.0.as_str()),
            _ => None,
        }
    }

    /// The fence details, when this error is a [`mandate_revoked`](WikiError::mandate_revoked).
    pub fn as_mandate_revoked(&self) -> Option<&MandateRevoked> {
        match self {
            WikiError::Io(e) => e.get_ref().and_then(|r| r.downcast_ref::<MandateRevoked>()),
            _ => None,
        }
    }
}

/// Mint a **stable, content-independent** section id: `base32(fnv1a(group ‖ page ‖ mint-clock ‖
/// nonce))`, truncated. Identity-stable after minting — derived from birth coordinates, never from
/// the (freely-editable) heading or body.
pub fn mint_section_id(group: &str, page: &str, mint_clock: u64, nonce: u64) -> SectionId {
    let mut h: u64 = 0xcbf29ce4_84222325;
    let mut feed = |bytes: &[u8]| for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    feed(group.as_bytes());
    feed(b"\0");
    feed(page.as_bytes());
    feed(&mint_clock.to_le_bytes());
    feed(&nonce.to_le_bytes());
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz"; // Crockford base32
    let mut s = String::with_capacity(13);
    s.push('s');
    let mut v = h;
    for _ in 0..12 {
        s.push(ALPHABET[(v & 0x1f) as usize] as char);
        v >>= 5;
    }
    Arc::from(s.as_str())
}
