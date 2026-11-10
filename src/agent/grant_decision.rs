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
    /// Base64 of the decider's client-certificate Ed25519 key (32 bytes).
    pub k: String,
    /// Base64 of the Ed25519 signature (64 bytes) over [`grant_decision_message`].
    pub sig: String,
}

/// The bytes a decider signs: [`GRANT_DECISION_TAG`], then the decider, the addressed node, the holder, the request,
/// the decision (`issue`, `refuse` or `revoke`), an issue's `ttl_secs` in decimal (empty when absent) and the reason
/// (empty when absent) — each as a `u32` little-endian length and its bytes — then `issued_at_ms` as a `u64`
/// little-endian. **Published format**: the SDKs compute it, and `tests/fixtures/decision/vector.json` pins it in all
/// three languages.
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
    let ttl = ttl_secs.map(|t| t.to_string()).unwrap_or_default();
    let mut out = GRANT_DECISION_TAG.to_vec();
    for part in [decider, node, agent, request, decision, ttl.as_str(), reason.unwrap_or("")] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part.as_bytes());
    }
    out.extend_from_slice(&issued_at_ms.to_le_bytes());
    out
}

impl SignedDecision {
    /// Whether `sig` is a signature under `k` over this decision — the signed fields and the decision's terms — for
    /// re-verifying a record afterwards (an evidence record's `signed_decision`, a refusal's or revocation's
    /// `signed`). It checks the signature only: whether `k` was the decider's current client key at `t` is read from
    /// the decider's `sys/client-cert/` records.
    #[cfg(feature = "tls")]
    pub fn verify_signature(&self, decision: &str, ttl_secs: Option<u64>, reason: Option<&str>) -> bool {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (Some(key), Some(sig)) = (
            b64.decode(&self.k).ok().and_then(|v| <[u8; 32]>::try_from(v).ok()),
            b64.decode(&self.sig).ok().and_then(|v| <[u8; 64]>::try_from(v).ok()),
        ) else {
            return false;
        };
        let msg = grant_decision_message(&self.decider, &self.node, &self.agent, &self.request, decision, ttl_secs, reason, self.t);
        crate::tls::verify_bytes(&key, &msg, &sig)
    }
}

/// Sign a decision with `key` as `decider`, at `issued_at_ms`.
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
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let msg = grant_decision_message(decider, node, agent, request, decision, ttl_secs, reason, issued_at_ms);
    let b64 = base64::engine::general_purpose::STANDARD;
    SignedDecision {
        decider: decider.to_string(),
        node: node.to_string(),
        agent: agent.to_string(),
        request: request.to_string(),
        t: issued_at_ms,
        k: b64.encode(key.verifying_key().to_bytes()),
        sig: b64.encode(key.sign(&msg).to_bytes()),
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
        }
    }

    pub(crate) fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::Malformed => StatusCode::BAD_REQUEST,
            Self::NotAddressedHere(_) | Self::IssuerUnset => StatusCode::CONFLICT,
            _ => StatusCode::FORBIDDEN,
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::Malformed => "signed.k is not a base64 32-byte key, or signed.sig not a base64 64-byte signature".into(),
            Self::NotTheDecider(why) => why.clone(),
            Self::Retargeted => "the signed agent or request is not this route's: a decision is signed for one request".into(),
            Self::NotAddressedHere(node) => format!("the decision is addressed to {node}, not this node; deliver it there"),
            Self::Stale => "the decider's clock is outside the caller envelope's window".into(),
            Self::DeciderLacksScope => "the decider holds no grant:issue at this gateway".into(),
            Self::Unsigned => "a bearer holding only grant:relay carries a decision a decider signed (`signed`)".into(),
            Self::Unverifiable => "this node has no identity ([tls]) and cannot judge a client certificate".into(),
            Self::IssuerUnset => "a signed decision needs this gateway's gateway_identity_issuer set: without it principals are qualified by the node id, which no client certificate names".into(),
        }
    }
}

/// Verify `signed` for the decision a route holds: `holder` and `request` the route's path, `decision`, `ttl_secs`
/// and `reason` its body. Checked in the module's order; `Ok` means the decider signed exactly this, here, now.
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
) -> Result<(), DecisionRefusal> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64.decode(&signed.k).ok().and_then(|v| v.try_into().ok()).ok_or(DecisionRefusal::Malformed)?;
    let sig: [u8; 64] = b64.decode(&signed.sig).ok().and_then(|v| v.try_into().ok()).ok_or(DecisionRefusal::Malformed)?;
    if ctx.tls.get().is_none() {
        return Err(DecisionRefusal::Unverifiable);
    }
    let msg = grant_decision_message(&signed.decider, &signed.node, &signed.agent, &signed.request, decision, ttl_secs, reason, signed.t);
    if !crate::tls::verify_bytes(&key, &msg, &sig) {
        return Err(DecisionRefusal::NotTheDecider(format!("the signature is not {}'s over this decision", signed.decider)));
    }
    // After the signature, so a forgery answers as one.
    match super::client_cert::client_key_standing(ctx, &signed.decider, &key) {
        super::client_cert::ClientKeyStanding::Current => {}
        super::client_cert::ClientKeyStanding::Superseded => {
            return Err(DecisionRefusal::NotTheDecider(format!("the key is one {} held before a newer certificate", signed.decider)))
        }
        _ => return Err(DecisionRefusal::NotTheDecider(format!("the key is not {}'s client key on record here", signed.decider))),
    }
    if signed.agent != holder || signed.request != request {
        return Err(DecisionRefusal::Retargeted);
    }
    if signed.node != ctx.node_id.to_string() {
        return Err(DecisionRefusal::NotAddressedHere(signed.node.clone()));
    }
    if !super::gateway_caller::client_issue_time_fresh(ctx, signed.t, now_ms) {
        return Err(DecisionRefusal::Stale);
    }
    Ok(())
}
