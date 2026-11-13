//! **An exchanged token at the provider** (identity-and-lifecycle I5, D6; spike S-exchange,
//! `docs/plans/spikes/2026-11-11-s-exchange.md`).
//!
//! A gateway exchanges an OIDC user's gateway-audience token at the IdP (RFC 8693) for a token whose audience is
//! `oidc.exchange_audience`, DPoP-bound (RFC 9449) to the gateway's Ed25519 node key, and carries it in the caller
//! envelope as `ot`, under a signature of its own (`so`). A provider whose `[oidc]` table sets `exchange_audience`
//! verifies it ([`check`]), in this order — the first failing step names the refusal (#759's review, L3; pinned by
//! `the_first_failing_check_names_the_refusal`):
//! 1. the header: a JWS with an allowed asymmetric `alg` ([`ALLOWED_ALGS`]; `none` and `HS*` are `ot_malformed`), a
//!    `typ`, if any, of an access token (`JWT` or `at+jwt`, else `ot_wrong_type`), and a `kid` (`ot_malformed`);
//! 2. the keys: the issuer's JWKS from **this node's** `[oidc]` table, from the one verifier the node's gateway uses
//!    for bearers too ([`OidcVerifier::keys_now`], never waiting on a fetch; none held, or not the `kid`:
//!    `ot_keys_unavailable`, retriable);
//! 3. the signature (`ot_signature_invalid`);
//! 4. `iss` this node's issuer (`ot_wrong_issuer`, absent included);
//! 5. `aud` holding `exchange_audience`, as a string or in an array (`ot_wrong_audience`, absent included);
//! 6. `azp` the gateway's client, `exchange_client_id` (`ot_wrong_client`);
//! 7. **sender binding**: `cnf.jkt` present (`ot_unbound`), and the RFC 7638 thumbprint of the Ed25519 key that
//!    verified this envelope's gateway signature ([`ed25519_jwk_thumbprint`]; `ot_bound_to_another_key`);
//! 8. the subject: the envelope's principal is `oidc:{iss}/{sub}` (`ot_subject_mismatch`);
//! 9. the clocks, against the **same** `now_ms` the replay window prunes with (#759's review, M1) and under the leeway
//!    of `max_clock_drift_ms`: `iat` present (`ot_without_iat`), `exp` present (`ot_without_exp`), neither `iat` nor
//!    `nbf` in the future (`ot_not_yet_valid`), `exp` not passed (`ot_expired`), and `exp − iat` within
//!    `exchange_max_lifetime_secs` (`ot_lifetime_too_long`);
//! 10. a `jti` (`ot_without_jti`);
//! 11. replay: `(iss, jti)` admitted once at this provider, whichever gateway carries it, while unexpired
//!     ([`OtReplayWindows`], at the receive boundary).
//!
//! And an `oidc:` principal's action — a kind [`needs_exchanged_token`] names — without an `ot` is refused.

use std::collections::HashMap;

use jsonwebtoken::{decode, decode_header, errors::ErrorKind, Validation};
use sha2::Digest as _;

use super::gateway_caller::ExchangedToken;
use super::oidc::{OidcVerifier, ALLOWED_ALGS};

/// The kind family a hosted capability is invoked on: `cap.invoke/{namespace}/{name}`
/// (`mycelium_wasm_host::runtime::cap_invoke_kind`, which this crate cannot name — the host depends on it).
pub(crate) const CAP_INVOKE_PREFIX: &str = super::HOSTED_CAPABILITY_KIND_PREFIX;

/// Whether `kind` is an **action** for which a verifying provider requires an `oidc:` principal to present an `ot`
/// (#757's confirm round, N4; the maintainer, 2026-11-11, on #759's Q1): a kind [`is_protected_kind`](super::is_protected_kind)
/// names (`mcp.invoke`, `skill.invoke`, `llm.invoke`, and the operator's `protected_rpc_kinds`); a companion kind
/// [`companion_kind_scope`](super::companion_kind_scope) gives a `:write` scope (every `blackboard.*`, `tuple.*` and
/// `wiki.*` kind but their read operations, an unknown verb included, as that table treats it); and a hosted
/// capability's invoke, the [`CAP_INVOKE_PREFIX`] family, because it runs code. The artifact kinds and
/// `reason.blob.fetch` are not actions here.
pub(crate) fn needs_exchanged_token(cfg: &crate::config::GossipConfig, kind: &str) -> bool {
    super::is_protected_kind(cfg, kind)
        || super::companion_kind_scope(kind).is_some_and(|s| s.ends_with(":write"))
        || kind.starts_with(CAP_INVOKE_PREFIX)
}

/// The RFC 7638 thumbprint of an Ed25519 public key's JWK — `{"crv":"Ed25519","kty":"OKP","x":…}` with its required
/// members in lexicographic order and no whitespace — in base64url: what `cnf.jkt` names on a token DPoP-bound to it.
pub(crate) fn ed25519_jwk_thumbprint(key: &[u8; 32]) -> String {
    use base64::Engine as _;
    let b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwk = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#, b64u.encode(key));
    b64u.encode(sha2::Sha256::digest(jwk.as_bytes()))
}

/// Verify the exchanged token `ot` carried in an envelope for `principal`, whose gateway signature `signer`
/// verified, at `now_ms` — the instant the receive boundary admits it at. `Err` names the refusal. Steps 1–10 of the
/// module's order; step 11, replay, is the receive boundary's ([`OtReplayWindows::admit`]). Reads no clock of its own.
pub(crate) fn check(verifier: &OidcVerifier, ot: &str, principal: &str, signer: &[u8; 32], now_ms: u64, leeway_ms: u64) -> Result<ExchangedToken, &'static str> {
    let cfg = verifier.config();
    let (Some(audience), Some(client)) = (cfg.exchange_audience.as_deref(), cfg.exchange_client_id.as_deref()) else {
        return Err("ot_not_verified_here");
    };
    // 1. The header.
    let header = decode_header(ot).map_err(|_| "ot_malformed")?;
    if !ALLOWED_ALGS.contains(&header.alg) {
        return Err("ot_malformed");
    }
    if header.typ.as_deref().is_some_and(|t| !is_access_token_typ(t)) {
        return Err("ot_wrong_type");
    }
    let kid = header.kid.ok_or("ot_malformed")?;
    // 2. The keys this node holds — never waiting on a fetch. A `kid` they lack has started the refresh (spaced by the
    // cooldown); retriable.
    let keys = verifier.keys_now(&kid).ok_or("ot_keys_unavailable")?;
    let key = keys.iter().find(|(k, _)| *k == kid).map(|(_, k)| k).ok_or("ot_keys_unavailable")?;
    // 3. The signature, and only the signature: every claim is read below, in the module's order, against `now_ms`
    // rather than the system clock `jsonwebtoken` would read.
    let mut validation = Validation::new(header.alg);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    let claims = match decode::<serde_json::Value>(ot, key, &validation) {
        Ok(d) => d.claims,
        Err(e) if matches!(e.kind(), ErrorKind::InvalidSignature) => return Err("ot_signature_invalid"),
        Err(_) => return Err("ot_malformed"),
    };
    let str_claim = |name: &str| claims.get(name).and_then(|v| v.as_str());
    // 4. The issuer.
    if str_claim("iss") != Some(cfg.issuer.as_str()) {
        return Err("ot_wrong_issuer");
    }
    // 5. The audience: a gateway-audience token, forwarded rather than exchanged, is refused here by name.
    let aud_holds = match claims.get("aud") {
        Some(serde_json::Value::String(a)) => a == audience,
        Some(serde_json::Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(audience)),
        _ => false,
    };
    if !aud_holds {
        return Err("ot_wrong_audience");
    }
    // 6. The client that exchanged it.
    if str_claim("azp") != Some(client) {
        return Err("ot_wrong_client");
    }
    // 7. Sender binding: bound, and to the key that signed this envelope.
    let Some(jkt) = claims.get("cnf").and_then(|c| c.get("jkt")).and_then(|v| v.as_str()) else {
        return Err("ot_unbound");
    };
    if jkt != ed25519_jwk_thumbprint(signer) {
        return Err("ot_bound_to_another_key");
    }
    // 8. The subject is the envelope's principal.
    let sub = str_claim("sub").unwrap_or_default();
    if sub.is_empty() || principal != super::gateway_caller::oidc_principal(&cfg.issuer, sub) {
        return Err("ot_subject_mismatch");
    }
    // 9. The clocks, at `now_ms`: accepted while `now_ms < exp + leeway`, the instant the window stops holding it.
    let secs = |name: &str| claims.get(name).and_then(|v| v.as_u64()).map(|s| s.saturating_mul(1_000));
    let iat = secs("iat").ok_or("ot_without_iat")?;
    let exp = secs("exp").ok_or("ot_without_exp")?;
    let latest_start = now_ms.saturating_add(leeway_ms);
    if iat > latest_start || secs("nbf").is_some_and(|nbf| nbf > latest_start) {
        return Err("ot_not_yet_valid");
    }
    if now_ms >= exp.saturating_add(leeway_ms) {
        return Err("ot_expired");
    }
    if exp.saturating_sub(iat) > cfg.exchange_max_lifetime().saturating_mul(1_000) {
        return Err("ot_lifetime_too_long");
    }
    // 10. An id to admit once.
    let jti = str_claim("jti").filter(|j| !j.is_empty()).ok_or("ot_without_jti")?;
    Ok(ExchangedToken { iss: cfg.issuer.clone(), jti: jti.to_string(), exp_ms: exp })
}

/// A JOSE `typ` an access token may carry: `JWT` (what Keycloak 26.8.2 emits — the spike's tokens) or RFC 9068's
/// `at+jwt`, with or without the `application/` prefix, in any case. Anything else — `dpop+jwt`, `logout+jwt`, … — is
/// another kind of token.
fn is_access_token_typ(typ: &str) -> bool {
    let t = typ.get(..12).filter(|p| p.eq_ignore_ascii_case("application/")).map_or(typ, |_| &typ[12..]);
    t.eq_ignore_ascii_case("JWT") || t.eq_ignore_ascii_case("at+jwt")
}

/// The most exchanged-token ids one provider holds for one gateway (#757's confirm round, N1).
pub(crate) const OT_WINDOW_PER_GATEWAY: usize = 65_536;
/// The most of those one principal may hold, so one user cannot fill its gateway's share.
pub(crate) const OT_WINDOW_PER_PRINCIPAL: usize = 16_384;
/// The most this provider holds in all (#759's review, L2): four gateways' worth, so the memory is bounded however
/// many gateways a fleet has, and a gateway that left frees its entries as they expire.
pub(crate) const OT_WINDOW_TOTAL: usize = 4 * OT_WINDOW_PER_GATEWAY;

/// Why an exchanged token was not admitted at the receive boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OtAdmit {
    /// This `(iss, jti)` was already admitted here, by any gateway, and is unexpired.
    Replayed,
    /// The provider holds [`OT_WINDOW_TOTAL`] unexpired ids.
    ProviderFull,
    /// The gateway holds [`OT_WINDOW_PER_GATEWAY`] of them.
    GatewayFull,
    /// The principal holds [`OT_WINDOW_PER_PRINCIPAL`] of them.
    PrincipalFull,
}

impl OtAdmit {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Replayed => "ot_replayed",
            Self::ProviderFull => "ot_provider_window_full",
            Self::GatewayFull => "ot_window_full",
            Self::PrincipalFull => "ot_principal_window_full",
        }
    }
}

/// One admitted token: whose shares it counts against.
struct Held {
    gateway: crate::NodeId,
    principal: String,
}

/// The admitted exchanged tokens, keyed by `(iss, jti)` **across gateways** (#759's review, Q2: the token is bound to
/// one key, and one key under two node ids must not get two admissions), each until its expiry plus the leeway, with
/// each gateway's and each principal's count. **It refuses rather than evicts** (#757's confirm round, N1): an entry is
/// never forgotten before its token could no longer verify, so a full share is a hard ceiling — 65 536 / (lifetime +
/// drift) tokens a second per gateway, 16 384 / (lifetime + drift) per principal, 262 144 / (lifetime + drift) in all.
/// Expired entries leave in expiry order from a queue (#759's review, L2), so an admission removes only what has
/// expired, and a departed gateway's entries and count go as theirs do.
#[derive(Default)]
struct OtWindow {
    held: HashMap<(String, String), Held>,
    by_expiry: std::collections::BinaryHeap<std::cmp::Reverse<(u64, (String, String))>>,
    per_gateway: HashMap<crate::NodeId, usize>,
    per_principal: HashMap<String, usize>,
}

fn release<K: std::hash::Hash + Eq>(counts: &mut HashMap<K, usize>, k: &K) {
    if let Some(n) = counts.get_mut(k) {
        *n -= 1;
        if *n == 0 {
            counts.remove(k);
        }
    }
}

impl OtWindow {
    fn prune(&mut self, now_ms: u64) {
        while let Some(std::cmp::Reverse((until, _))) = self.by_expiry.peek()
            && *until <= now_ms
        {
            let Some(std::cmp::Reverse((_, key))) = self.by_expiry.pop() else { break };
            if let Some(h) = self.held.remove(&key) {
                release(&mut self.per_gateway, &h.gateway);
                release(&mut self.per_principal, &h.principal);
            }
        }
    }
}

/// Every admitted exchanged token at this provider (identity-and-lifecycle I5): in memory, so empty after a restart —
/// a token presented before it can be presented once more after it, while unexpired, and only in an envelope signed by
/// the gateway it is bound to. Lock-order row 78: one leaf lock, held for one admission.
pub(crate) struct OtReplayWindows {
    window: parking_lot::Mutex<OtWindow>,
    total: usize,
    per_gateway: usize,
    per_principal: usize,
    /// Refusals at the window, by reason: `GET /stats` `exchanged_tokens_refused`.
    pub(crate) replayed: std::sync::atomic::AtomicU64,
    pub(crate) provider_full: std::sync::atomic::AtomicU64,
    pub(crate) gateway_full: std::sync::atomic::AtomicU64,
    pub(crate) principal_full: std::sync::atomic::AtomicU64,
}

impl Default for OtReplayWindows {
    fn default() -> Self {
        Self::with_bounds(OT_WINDOW_TOTAL, OT_WINDOW_PER_GATEWAY, OT_WINDOW_PER_PRINCIPAL)
    }
}

impl OtReplayWindows {
    pub(crate) fn with_bounds(total: usize, per_gateway: usize, per_principal: usize) -> Self {
        Self {
            window: Default::default(),
            total,
            per_gateway,
            per_principal,
            replayed: Default::default(),
            provider_full: Default::default(),
            gateway_full: Default::default(),
            principal_full: Default::default(),
        }
    }

    /// How many ids are held now, and for how many gateways (tests).
    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, usize) {
        let w = self.window.lock();
        (w.held.len(), w.per_gateway.len())
    }

    /// Admit `token`, carried by `gateway` for `principal`, at `now_ms` — the instant [`check`] verified it at: once,
    /// and only while the provider's, its gateway's and its principal's shares have room. Held until
    /// `exp_ms + leeway_ms`, the instant [`check`] stops accepting it.
    pub(crate) fn admit(&self, gateway: &crate::NodeId, principal: &str, token: &ExchangedToken, now_ms: u64, leeway_ms: u64) -> Result<(), OtAdmit> {
        use std::sync::atomic::Ordering;
        let mut w = self.window.lock();
        w.prune(now_ms);
        let key = (token.iss.clone(), token.jti.clone());
        let refused = if w.held.contains_key(&key) {
            Some(OtAdmit::Replayed)
        } else if w.held.len() >= self.total {
            Some(OtAdmit::ProviderFull)
        } else if w.per_gateway.get(gateway).copied().unwrap_or(0) >= self.per_gateway {
            Some(OtAdmit::GatewayFull)
        } else if w.per_principal.get(principal).copied().unwrap_or(0) >= self.per_principal {
            Some(OtAdmit::PrincipalFull)
        } else {
            None
        };
        if let Some(r) = refused {
            match r {
                OtAdmit::Replayed => &self.replayed,
                OtAdmit::ProviderFull => &self.provider_full,
                OtAdmit::GatewayFull => &self.gateway_full,
                OtAdmit::PrincipalFull => &self.principal_full,
            }
            .fetch_add(1, Ordering::Relaxed);
            return Err(r);
        }
        let until_ms = token.exp_ms.saturating_add(leeway_ms);
        w.by_expiry.push(std::cmp::Reverse((until_ms, key.clone())));
        *w.per_gateway.entry(gateway.clone()).or_default() += 1;
        *w.per_principal.entry(principal.to_string()).or_default() += 1;
        w.held.insert(key, Held { gateway: gateway.clone(), principal: principal.to_string() });
        Ok(())
    }
}
