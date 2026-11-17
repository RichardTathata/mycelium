//! **A signed grant decision** (identity-and-lifecycle I3, D3 and D5): a supervisor holding a client certificate signs
//! its answer to a grant request — issue, refuse or revoke — and any gateway carrying it under `grant:relay` delivers
//! it to the node it is addressed to, which verifies it and records **the signer** as the decision's actor.
//!
//! The signed bytes name the decider, the node the decision is addressed to, the request's holder and id, the decision
//! and its terms (an issue's `ttl_secs`, a refusal's or revocation's reason), and the decider's clock
//! ([`grant_decision_message`]). The addressed node verifies, in order: the signature under the key it carries, that
//! key the decider's current client key here (`client_cert::client_key_standing`) — else `not_the_decider`; the
//! holder and request the same as the route's path — else `decision_retargeted`; the addressed node itself — else
//! `not_addressed_here`; the clock inside the caller envelope's window — else `stale`. A replay of a signed decision
//! inside its window finds the request already answered or already revoked, so no nonce cache is kept: the decision
//! routes take one claim per request on this node (`http/grants.rs`), so concurrent copies are answered once.
//!
//! ## The grade (identity-and-lifecycle I7: P1, P2, P4 as H2 revised it, P7)
//!
//! A decider is one of two kinds, told apart by its principal and bound to its key's kind:
//!
//! - a **service**, `token:{issuer}/{name}`, signing with its client certificate's Ed25519 key (`k` 32 bytes, `sig`
//!   64). Its decision is recorded `signed_with.grade = "service"`. Its body names no grade (a pre-I7 SDK's body, which
//!   verifies unchanged) or names `service`; any other grade is refused;
//! - a **consent principal**, `consent:{issuer}/{name}`, signing with the P-256 key its consent certificate names
//!   (`k` the 65-byte uncompressed point, `sig` an ASN.1 DER ECDSA signature over SHA-256). Its body **names its
//!   grade** — a body without one is refused before anything else is checked — and the grade must be the one in the
//!   **certificate at the principal's mark**, or the decision is refused. A key of the other kind under either
//!   principal is refused: a service key is never a consent key.
//!
//! The grade is inside the signed message ([`grant_decision_message_graded`]), so a relay cannot alter it.
//!
//! **What the grade says.** A consent grade is *what the operator who issued the certificate was told* about where
//! the key lives; the fleet CA signed that statement, and no verifier can observe it (S-presence: a software key and
//! an Enclave key verify alike). So [`SignedWith`] carries `grade_source: "operator"` beside a consent grade, and this
//! node's promise is only what it checked itself: the signature, the certificate's chain and standing, and that the
//! body's grade is the certificate's. `service` is read from the kind of certificate the key is in
//! (`grade_source: "certificate_scheme"`).
//!
//! **The provisional gate.** While the grade extension's OID is provisional
//! (`mycelium_core::tls::consent_grade_oid_open`), a build that is not a test build refuses a consent decider's
//! decision by name (`consent_grade_oid_provisional`) before it reads a key or checks a signature, and
//! `mycelium_core::tls::verify_consent_p256` verifies nothing.
//!
//! `recording` is accepted only in a `sim` build.

use serde::{Deserialize, Serialize};

/// Domain tag of the signed message.
pub const GRANT_DECISION_TAG: &[u8] = b"mycelium.gateway/grant-decision/1";

/// A decision's signature, sent as the decision body's `signed` field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedDecision {
    /// The principal who decided, as its client certificate names it (`token:{issuer}/{name}`).
    pub decider: String,
    /// The node the decision is addressed to: its node id, as `GET /health` reports it.
    pub node: String,
    /// The request's holder, as a principal (`node:…` or a client principal), never its hex key segment.
    pub agent: String,
    /// The request id.
    pub request: String,
    /// The decider's wall clock (ms) when it signed.
    pub t: u64,
    /// Base64 of the decider's key: a service's client-certificate Ed25519 key (32 bytes), or a consent principal's
    /// P-256 point (65 bytes, uncompressed X9.63 — identity-and-lifecycle I7).
    pub k: String,
    /// Base64 of the signature over [`grant_decision_message_graded`]: Ed25519 (64 bytes) under a service key, ASN.1
    /// DER ECDSA over SHA-256 under a consent key.
    pub sig: String,
    /// The grade the signer names, inside the signature (identity-and-lifecycle I7). Absent from a service's body (a
    /// pre-I7 body), or `service` there; **required** of a consent principal, and equal to its certificate's. Never
    /// empty. Serialised only when present, so a pre-I7 body is byte-for-byte what it was — and a node before I7
    /// refuses a body that carries it (unknown field), so verifiers are upgraded before a signer names a grade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
}

/// The grade a service's decision is recorded with (identity-and-lifecycle I7): a `token:` decider signing with its
/// client certificate's Ed25519 key. Not a consent grade — no consent certificate carries it.
pub const GRADE_SERVICE: &str = "service";

/// **Which key signed a decision, and the grade recorded with it** (identity-and-lifecycle I7) — on the answer's
/// record and the evidence record (`signed_with`), and on the [`ActionEnvelope`](crate::ActionEnvelope) the decision
/// is evaluated with.
///
/// `grade` is never an observation of how the decision was made. For a consent key it is **what the operator who
/// issued the consent certificate was told** about where the key lives (`grade_source: "operator"`): the fleet CA
/// signed that statement, the node that verified the decision checked that the signed body names the same grade, and
/// nobody in the fleet can check the statement itself (P1, H2) — a `tty` decision may have been signed by any process
/// running as the user, the agent whose request it approves included (P7's residual), and an `os_presence(macos,
/// unbound)` one proves at most that a person was present, not what they were shown (P5). For a service key the grade
/// is `service`, read from the kind of certificate the key is in (`grade_source: "certificate_scheme"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SignedWith {
    /// The signing key's kind: `ed25519` (a service's client-certificate key) or `p256` (a consent key).
    pub key: String,
    /// `service`, or the consent certificate's grade (`os_presence(macos, unbound)`, `tty`, `recording`).
    pub grade: String,
    /// Who the grade is the statement of: `operator` (a consent certificate's grade: the issuer's host was told it)
    /// or `certificate_scheme` (`service`: the key is in a client certificate).
    pub grade_source: String,
}

impl SignedWith {
    /// `key` value: a service's Ed25519 client-certificate key.
    pub const KEY_ED25519: &'static str = "ed25519";
    /// `key` value: a consent principal's P-256 key.
    pub const KEY_P256: &'static str = "p256";
    /// `grade_source` value: the grade is the certificate issuer's statement of what it was told.
    pub const SOURCE_OPERATOR: &'static str = "operator";
    /// `grade_source` value: the grade follows from the kind of certificate the key is in.
    pub const SOURCE_CERTIFICATE_SCHEME: &'static str = "certificate_scheme";

    /// A service decider's: Ed25519, `service`.
    pub fn service() -> Self {
        Self { key: Self::KEY_ED25519.into(), grade: GRADE_SERVICE.into(), grade_source: Self::SOURCE_CERTIFICATE_SCHEME.into() }
    }

    /// A consent decider's: P-256, the grade its certificate carries — the operator's statement.
    pub fn consent(grade: impl Into<String>) -> Self {
        Self { key: Self::KEY_P256.into(), grade: grade.into(), grade_source: Self::SOURCE_OPERATOR.into() }
    }
}

/// Whether `principal` is a consent principal (`consent:{issuer}/{name}`, identity-and-lifecycle I7).
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn is_consent_principal(principal: &str) -> bool {
    principal.starts_with("consent:")
}

/// Whether a `recording` consent grade is accepted by a build: only a `sim` build's (identity-and-lifecycle I7).
pub fn recording_grade_accepted_when(sim_build: bool) -> bool {
    sim_build
}

/// A decision's key, decoded by its kind (identity-and-lifecycle I7, P2).
#[cfg(feature = "tls")]
enum DeciderKey {
    Ed25519([u8; 32], [u8; 64]),
    P256(Box<[u8; 65]>, Vec<u8>),
}

/// The longest ASN.1 DER ECDSA P-256 signature: a SEQUENCE of two 33-byte INTEGERs.
#[cfg(feature = "tls")]
const P256_DER_SIG_MAX: usize = 72;

#[cfg(feature = "tls")]
impl DeciderKey {
    /// `k` and `sig` decoded by `k`'s length: 32 bytes is an Ed25519 key with a 64-byte signature, 65 a P-256 point
    /// with a DER signature. Anything else is `None`.
    fn decode(signed: &SignedDecision) -> Option<Self> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (k, sig) = (b64.decode(&signed.k).ok()?, b64.decode(&signed.sig).ok()?);
        match k.len() {
            32 => Some(Self::Ed25519(k.try_into().ok()?, sig.try_into().ok()?)),
            65 if !sig.is_empty() && sig.len() <= P256_DER_SIG_MAX => Some(Self::P256(Box::new(k.try_into().ok()?), sig)),
            _ => None,
        }
    }

    fn verifies(&self, msg: &[u8]) -> bool {
        match self {
            Self::Ed25519(key, sig) => crate::tls::verify_bytes(key, msg, sig),
            // Gated: `false` in a build that reads no consent certificate.
            Self::P256(point, sig) => mycelium_core::tls::verify_consent_p256(point, msg, sig),
        }
    }
}

/// The bytes a decider signs: [`GRANT_DECISION_TAG`], then the decider, the addressed node, the holder, the request,
/// the decision (`issue`, `refuse` or `revoke`), an issue's `ttl_secs` in decimal (empty when absent) and the reason
/// (empty when absent) — each as a `u32` little-endian length and its bytes — then `issued_at_ms` as a `u64`
/// little-endian. **Published format**: the SDKs compute it, and `tests/fixtures/decision/vector.json` pins it in all
/// three languages. This is the message of a decision naming **no grade**; [`grant_decision_message_graded`] is the
/// whole format.
#[allow(clippy::too_many_arguments)]
pub fn grant_decision_message(
    decider: &str,
    node: &str,
    agent: &str,
    request: &str,
    decision: &str,
    ttl_secs: Option<u64>,
    reason: Option<&str>,
    issued_at_ms: u64,
) -> Vec<u8> {
    grant_decision_message_graded(decider, node, agent, request, decision, ttl_secs, reason, issued_at_ms, None)
}

/// [`grant_decision_message`] with the signer's **grade** (identity-and-lifecycle I7): when `grade` is `Some`, a `u32`
/// little-endian length and the grade's bytes follow `issued_at_ms`; when it is `None` nothing follows, so a decision
/// naming no grade has exactly the message it had before I7 and every signature over it stands. An empty grade is no
/// grade a verifier accepts (a signed decision naming `""` is refused), and this writes it as its zero length.
/// **Published format**, pinned with and without a grade in `tests/fixtures/decision/vector.json`.
#[allow(clippy::too_many_arguments)]
pub fn grant_decision_message_graded(
    decider: &str,
    node: &str,
    agent: &str,
    request: &str,
    decision: &str,
    ttl_secs: Option<u64>,
    reason: Option<&str>,
    issued_at_ms: u64,
    grade: Option<&str>,
) -> Vec<u8> {
    let ttl = ttl_secs.map(|t| t.to_string()).unwrap_or_default();
    let mut out = GRANT_DECISION_TAG.to_vec();
    for part in [decider, node, agent, request, decision, ttl.as_str(), reason.unwrap_or("")] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part.as_bytes());
    }
    out.extend_from_slice(&issued_at_ms.to_le_bytes());
    if let Some(grade) = grade {
        out.extend_from_slice(&(grade.len() as u32).to_le_bytes());
        out.extend_from_slice(grade.as_bytes());
    }
    out
}

impl SignedDecision {
    /// Whether `sig` is a signature under `k` over this decision — the signed fields, the decision's terms and the
    /// grade the body names — for re-verifying a record afterwards (an evidence record's `signed_decision`, a
    /// refusal's or revocation's `signed`). It checks the signature only, under the key's kind (Ed25519, or P-256 for
    /// a consent key): whether `k` was the decider's current key at `t`, and whether the grade is its certificate's,
    /// are read from the decider's `sys/client-cert/` records. `false` for a body naming an empty grade, and for a
    /// consent key's signature in a build that reads no consent certificate (the provisional gate).
    #[cfg(feature = "tls")]
    pub fn verify_signature(&self, decision: &str, ttl_secs: Option<u64>, reason: Option<&str>) -> bool {
        if self.grade.as_deref() == Some("") {
            return false;
        }
        let Some(key) = DeciderKey::decode(self) else { return false };
        key.verifies(&self.message(decision, ttl_secs, reason))
    }

    /// The bytes this decision's signature is over, for `decision` and its terms.
    pub fn message(&self, decision: &str, ttl_secs: Option<u64>, reason: Option<&str>) -> Vec<u8> {
        grant_decision_message_graded(&self.decider, &self.node, &self.agent, &self.request, decision, ttl_secs, reason, self.t, self.grade.as_deref())
    }
}

/// Sign a decision with `key` as `decider`, at `issued_at_ms`, naming no grade (a service's decision is recorded
/// `service` either way).
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)]
pub fn sign_grant_decision(
    key: &ed25519_dalek::SigningKey,
    decider: &str,
    node: &str,
    agent: &str,
    request: &str,
    decision: &str,
    ttl_secs: Option<u64>,
    reason: Option<&str>,
    issued_at_ms: u64,
) -> SignedDecision {
    sign_grant_decision_graded(key, decider, node, agent, request, decision, ttl_secs, reason, issued_at_ms, None)
}

/// [`sign_grant_decision`] naming `grade` inside the signature (identity-and-lifecycle I7). An Ed25519 key is a
/// service's, whose only grade is [`GRADE_SERVICE`]: a verifier refuses any other from it. A consent key's signer is
/// `dev`'s (I7's later PR), not this function.
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)]
pub fn sign_grant_decision_graded(
    key: &ed25519_dalek::SigningKey,
    decider: &str,
    node: &str,
    agent: &str,
    request: &str,
    decision: &str,
    ttl_secs: Option<u64>,
    reason: Option<&str>,
    issued_at_ms: u64,
    grade: Option<&str>,
) -> SignedDecision {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let msg = grant_decision_message_graded(decider, node, agent, request, decision, ttl_secs, reason, issued_at_ms, grade);
    let b64 = base64::engine::general_purpose::STANDARD;
    SignedDecision {
        decider: decider.to_string(),
        node: node.to_string(),
        agent: agent.to_string(),
        request: request.to_string(),
        t: issued_at_ms,
        k: b64.encode(key.verifying_key().to_bytes()),
        sig: b64.encode(key.sign(&msg).to_bytes()),
        grade: grade.map(str::to_string),
    }
}

/// Why a decision body was refused at the decision routes.
#[cfg(all(feature = "gateway", feature = "tls"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DecisionRefusal {
    /// `signed` does not decode: a key or signature of the wrong length or not base64.
    Malformed,
    /// The signature does not verify under the key it carries, or that key is not the decider's current client key
    /// here (unknown, superseded, or no certificate on record) — the detail says which.
    NotTheDecider(String),
    /// The signed holder or request is not the route's: the decision was presented for another request.
    Retargeted,
    /// The signed node is not this one (the node it names, in the detail).
    NotAddressedHere(String),
    /// The decider's clock is outside the caller envelope's window.
    Stale,
    /// The decider does not hold `grant:issue` at this gateway.
    DeciderLacksScope,
    /// A body without `signed`, carried by a bearer holding only `grant:relay`.
    Unsigned,
    /// This node has no identity (`[tls]` unset), so it judges no client certificate.
    Unverifiable,
    /// This gateway has no `gateway_identity_issuer`: its principals are qualified by its node id, which no client
    /// certificate names, so no decider can be one of them (as I1's doors refuse a signed call).
    IssuerUnset,
    /// A consent decider's decision at a build that reads no consent certificate: the grade extension's OID is
    /// provisional and this is no test build (identity-and-lifecycle I7). Refused before a key is read.
    ConsentUnread,
    /// The body names an empty grade.
    GradeEmpty,
    /// A consent decider's body names no grade.
    GradeMissing,
    /// The body's grade is not the one recorded for the key: a service naming any grade but `service`, or a consent
    /// principal naming a grade other than its certificate's (the detail says which).
    GradeMismatch(String),
    /// A `recording` consent grade outside a `sim` build.
    RecordingOutsideSim,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl DecisionRefusal {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Malformed => "decision_signature_malformed",
            Self::NotTheDecider(_) => "not_the_decider",
            Self::Retargeted => "decision_retargeted",
            Self::NotAddressedHere(_) => "not_addressed_here",
            Self::Stale => "stale",
            Self::DeciderLacksScope => "decider_lacks_scope",
            Self::Unsigned => "decision_unsigned",
            Self::Unverifiable => "decision_unverifiable",
            Self::IssuerUnset => "gateway_identity_issuer_unset",
            Self::ConsentUnread => "consent_grade_oid_provisional",
            Self::GradeEmpty => "decision_grade_empty",
            Self::GradeMissing => "decision_grade_missing",
            Self::GradeMismatch(_) => "decision_grade_mismatch",
            Self::RecordingOutsideSim => "recording_grade_outside_sim",
        }
    }

    pub(crate) fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::Malformed | Self::GradeEmpty => StatusCode::BAD_REQUEST,
            Self::NotAddressedHere(_) | Self::IssuerUnset => StatusCode::CONFLICT,
            Self::ConsentUnread => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::FORBIDDEN,
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::Malformed => "signed.k is not a base64 32-byte Ed25519 key with a base64 64-byte signature, nor a base64 65-byte P-256 point with a base64 DER signature".into(),
            Self::NotTheDecider(why) => why.clone(),
            Self::Retargeted => "the signed agent or request is not this route's: a decision is signed for one request".into(),
            Self::NotAddressedHere(node) => format!("the decision is addressed to {node}, not this node; deliver it there"),
            Self::Stale => "the decider's clock is outside the caller envelope's window".into(),
            Self::DeciderLacksScope => "the decider holds no grant:issue at this gateway".into(),
            Self::Unsigned => "a bearer holding only grant:relay carries a decision a decider signed (`signed`)".into(),
            Self::Unverifiable => "this node has no identity ([tls]) and cannot judge a client certificate".into(),
            Self::IssuerUnset => "a signed decision needs this gateway's gateway_identity_issuer set: without it principals are qualified by the node id, which no client certificate names".into(),
            Self::ConsentUnread => "consent decisions are not read by this build: the grade extension's OID is provisional until the registered IANA Private Enterprise Number arc arrives (identity-and-lifecycle I7)".into(),
            Self::GradeEmpty => "signed.grade is empty: a decision names a grade or leaves the field out".into(),
            Self::GradeMissing => "a consent principal's decision names its grade (signed.grade), inside the signature".into(),
            Self::GradeMismatch(why) => why.clone(),
            Self::RecordingOutsideSim => "a `recording` consent grade is accepted only by a `sim` build".into(),
        }
    }
}

/// Verify `signed` for the decision a route holds: `holder` and `request` the route's path, `decision`, `ttl_secs`
/// and `reason` its body. Checked in the module's order; `Ok` means the decider signed exactly this, here, now, and
/// names which key signed and the grade recorded with it ([`SignedWith`]).
///
/// The grade's order (identity-and-lifecycle I7): a consent decider at a build that reads no consent certificate is
/// refused first, by name; then the body's grade as written (empty; absent from a consent decider; a service naming
/// anything but `service`), before any key is used; then the key's kind against the principal's; the signature; the
/// key's standing; and, for a consent key, the grade of **the certificate at the principal's mark** against the
/// body's.
#[cfg(all(feature = "gateway", feature = "tls"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn verify(
    ctx: &super::TaskCtx,
    signed: &SignedDecision,
    holder: &str,
    request: &str,
    decision: &str,
    ttl_secs: Option<u64>,
    reason: Option<&str>,
    now_ms: u64,
) -> Result<SignedWith, DecisionRefusal> {
    use super::client_cert::ClientKeyStanding;
    let consent = is_consent_principal(&signed.decider);
    // A shipped build reads no consent certificate, so it judges no consent decision: by name, before a key is read.
    if consent && !mycelium_core::tls::consent_grade_oid_open() {
        return Err(DecisionRefusal::ConsentUnread);
    }
    let key = DeciderKey::decode(signed).ok_or(DecisionRefusal::Malformed)?;
    if matches!(key, DeciderKey::P256(..)) && !mycelium_core::tls::consent_grade_oid_open() {
        return Err(DecisionRefusal::ConsentUnread);
    }
    // The grade as the body writes it, before any key is used.
    match signed.grade.as_deref() {
        Some("") => return Err(DecisionRefusal::GradeEmpty),
        None if consent => return Err(DecisionRefusal::GradeMissing),
        Some(g) if !consent && g != GRADE_SERVICE => {
            return Err(DecisionRefusal::GradeMismatch(format!("{} is a service principal, whose decisions are recorded `{GRADE_SERVICE}`: it names `{g}`", signed.decider)))
        }
        _ => {}
    }
    if ctx.tls.get().is_none() {
        return Err(DecisionRefusal::Unverifiable);
    }
    // The key's kind is the principal's: a service key is never a consent key, nor the reverse (H1).
    match (&key, consent) {
        (DeciderKey::Ed25519(..), true) => {
            return Err(DecisionRefusal::NotTheDecider(format!("{} is a consent principal, whose key is a P-256 point: an Ed25519 key is a service's, never a consent key", signed.decider)))
        }
        (DeciderKey::P256(..), false) => {
            return Err(DecisionRefusal::NotTheDecider(format!("{} is not a consent principal: a P-256 key decides only as consent:{{issuer}}/{{name}}", signed.decider)))
        }
        _ => {}
    }
    let msg = signed.message(decision, ttl_secs, reason);
    if !key.verifies(&msg) {
        return Err(DecisionRefusal::NotTheDecider(format!("the signature is not {}'s over this decision", signed.decider)));
    }
    // After the signature, so a forgery answers as one.
    let superseded = || DecisionRefusal::NotTheDecider(format!("the key is one {} held before a newer certificate", signed.decider));
    let signed_with = match &key {
        DeciderKey::Ed25519(key, _) => match super::client_cert::client_key_standing(ctx, &signed.decider, key) {
            ClientKeyStanding::Current => SignedWith::service(),
            ClientKeyStanding::Superseded => return Err(superseded()),
            _ => return Err(DecisionRefusal::NotTheDecider(format!("the key is not {}'s client key on record here", signed.decider))),
        },
        DeciderKey::P256(point, _) => match super::client_cert::consent_key_grade(ctx, &signed.decider, point) {
            (ClientKeyStanding::Current, Some(certified)) => {
                // `GradeMissing` returned above, so the body names one.
                let named = signed.grade.as_deref().unwrap_or_default();
                if named != certified.as_str() {
                    return Err(DecisionRefusal::GradeMismatch(format!(
                        "the decision names the grade `{named}`; {}'s consent certificate carries `{certified}`, the grade its issuer was told",
                        signed.decider
                    )));
                }
                if certified == mycelium_core::tls::ConsentGrade::Recording && !recording_grade_accepted_when(cfg!(feature = "sim")) {
                    return Err(DecisionRefusal::RecordingOutsideSim);
                }
                SignedWith::consent(certified.as_str())
            }
            (ClientKeyStanding::Current, None) => {
                return Err(DecisionRefusal::NotTheDecider(format!(
                    "the consent certificate at {}'s mark is not on record here, so its grade cannot be read",
                    signed.decider
                )))
            }
            (ClientKeyStanding::Superseded, _) => return Err(superseded()),
            _ => return Err(DecisionRefusal::NotTheDecider(format!("the key is not {}'s consent key on record here", signed.decider))),
        },
    };
    if signed.agent != holder || signed.request != request {
        return Err(DecisionRefusal::Retargeted);
    }
    if signed.node != ctx.node_id.to_string() {
        return Err(DecisionRefusal::NotAddressedHere(signed.node.clone()));
    }
    if !super::gateway_caller::client_issue_time_fresh(ctx, signed.t, now_ms) {
        return Err(DecisionRefusal::Stale);
    }
    Ok(signed_with)
}
