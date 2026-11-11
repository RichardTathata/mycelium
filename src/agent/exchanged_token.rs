//! **An exchanged token at the provider** (identity-and-lifecycle I5, D6; spike S-exchange,
//! `docs/plans/spikes/2026-11-11-s-exchange.md`).
//!
//! A gateway exchanges an OIDC user's gateway-audience token at the IdP (RFC 8693) for a token whose audience is
//! `oidc.exchange_audience`, DPoP-bound (RFC 9449) to the gateway's Ed25519 node key, and carries it in the caller
//! envelope as `ot`, under a signature of its own (`so`). A provider whose `[oidc]` table sets `exchange_audience`
//! verifies it ([`check`]), in the row's order:
//! 1. the signature, against the issuer's JWKS from **this provider's** `[oidc]` table (the verifier the gateway uses
//!    for bearers, [`OidcVerifier::keys_now`], never waiting on a fetch);
//! 2. `aud` holding `exchange_audience`, and `azp` the gateway's client, `exchange_client_id`;
//! 3. **sender binding**: `cnf.jkt` present, and the RFC 7638 thumbprint of the Ed25519 key that verified this
//!    envelope's gateway signature ([`ed25519_jwk_thumbprint`]);
//! 4. the subject: the envelope's principal is `oidc:{iss}/{sub}`;
//! 5. the lifetime and the clocks: `exp`, `iat` and `nbf` under the leeway of `max_clock_drift_ms`, and `exp − iat`
//!    within `exchange_max_lifetime_secs`;
//! 6. replay: `(iss, jti)` admitted once at this provider while unexpired ([`OtReplayWindows`], at the receive
//!    boundary).
//!
//! And an `oidc:` principal's action — a kind [`needs_exchanged_token`] names — without an `ot` is refused.

use std::collections::HashMap;

use jsonwebtoken::{decode, decode_header, errors::ErrorKind, Validation};
use sha2::Digest as _;

use super::gateway_caller::ExchangedToken;
use super::oidc::{OidcVerifier, ALLOWED_ALGS};

/// Whether `kind` is an **action** for which a verifying provider requires an `oidc:` principal to present an `ot`
/// (#757's confirm round, N4): a kind [`is_protected_kind`](super::is_protected_kind) names (`mcp.invoke`,
/// `skill.invoke`, `llm.invoke`, and the operator's `protected_rpc_kinds`), or a companion kind
/// [`companion_kind_scope`](super::companion_kind_scope) gives a `:write` scope (every `blackboard.*`, `tuple.*` and
/// `wiki.*` kind but their read operations, an unknown verb included, as that table treats it).
pub(crate) fn needs_exchanged_token(cfg: &crate::config::GossipConfig, kind: &str) -> bool {
    super::is_protected_kind(cfg, kind) || super::companion_kind_scope(kind).is_some_and(|s| s.ends_with(":write"))
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
/// verified, at `now_ms`. `Err` names the refusal. Steps 1–5 of the module's order; step 6, replay, is the receive
/// boundary's ([`OtReplayWindows::admit`]).
pub(crate) fn check(verifier: &OidcVerifier, ot: &str, principal: &str, signer: &[u8; 32], now_ms: u64, leeway_ms: u64) -> Result<ExchangedToken, &'static str> {
    let cfg = verifier.config();
    let (Some(audience), Some(client)) = (cfg.exchange_audience.as_deref(), cfg.exchange_client_id.as_deref()) else {
        return Err("ot_not_verified_here");
    };
    // 1. The signature, against the keys this provider holds — never waiting on a fetch.
    let header = decode_header(ot).map_err(|_| "ot_malformed")?;
    if !ALLOWED_ALGS.contains(&header.alg) {
        return Err("ot_malformed");
    }
    let kid = header.kid.ok_or("ot_malformed")?;
    let keys = verifier.keys_now(&kid).ok_or("ot_keys_unavailable")?;
    // A `kid` the held keys lack: the refresh has started (spaced by the cooldown); retriable.
    let key = keys.iter().find(|(k, _)| *k == kid).map(|(_, k)| k).ok_or("ot_keys_unavailable")?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[cfg.issuer.as_str()]);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = leeway_ms / 1_000;
    let claims = match decode::<serde_json::Value>(ot, key, &validation) {
        Ok(d) => d.claims,
        Err(e) => {
            return Err(match e.kind() {
                ErrorKind::InvalidSignature => "ot_signature_invalid",
                // 2. A gateway-audience token, forwarded rather than exchanged, is refused here by name.
                ErrorKind::InvalidAudience => "ot_wrong_audience",
                ErrorKind::MissingRequiredClaim(c) if c == "aud" => "ot_wrong_audience",
                ErrorKind::InvalidIssuer => "ot_wrong_issuer",
                ErrorKind::ExpiredSignature | ErrorKind::ImmatureSignature => "ot_expired",
                _ => "ot_malformed",
            });
        }
    };
    // 2. The client that exchanged it.
    if claims.get("azp").and_then(|v| v.as_str()) != Some(client) {
        return Err("ot_wrong_client");
    }
    // 3. Sender binding: bound, and to the key that signed this envelope.
    let Some(jkt) = claims.get("cnf").and_then(|c| c.get("jkt")).and_then(|v| v.as_str()) else {
        return Err("ot_unbound");
    };
    if jkt != ed25519_jwk_thumbprint(signer) {
        return Err("ot_bound_to_another_key");
    }
    // 4. The subject is the envelope's principal.
    let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or_default();
    if sub.is_empty() || principal != super::gateway_caller::oidc_principal(&cfg.issuer, sub) {
        return Err("ot_subject_mismatch");
    }
    // 5. The lifetime (`exp` and `nbf` were read under the leeway above).
    let (Some(exp), Some(iat)) = (claims.get("exp").and_then(|v| v.as_u64()), claims.get("iat").and_then(|v| v.as_u64())) else {
        return Err("ot_lifetime_too_long");
    };
    if exp.saturating_sub(iat) > cfg.exchange_max_lifetime() {
        return Err("ot_lifetime_too_long");
    }
    if iat.saturating_mul(1_000) > now_ms.saturating_add(leeway_ms) {
        return Err("ot_expired");
    }
    // 6's precondition: an id to admit once.
    let jti = claims.get("jti").and_then(|v| v.as_str()).filter(|j| !j.is_empty()).ok_or("ot_without_jti")?;
    Ok(ExchangedToken { iss: cfg.issuer.clone(), jti: jti.to_string(), exp_ms: exp.saturating_mul(1_000) })
}

/// The most exchanged-token ids one provider holds for one gateway (#757's confirm round, N1).
pub(crate) const OT_WINDOW_PER_GATEWAY: usize = 65_536;
/// The most of those one principal may hold, so one user cannot fill its gateway's window.
pub(crate) const OT_WINDOW_PER_PRINCIPAL: usize = 16_384;

/// Why an exchanged token was not admitted at the receive boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OtAdmit {
    /// This `(iss, jti)` was already admitted here and is unexpired.
    Replayed,
    /// The gateway's window holds [`OT_WINDOW_PER_GATEWAY`] unexpired ids.
    GatewayFull,
    /// The principal holds [`OT_WINDOW_PER_PRINCIPAL`] of its gateway's.
    PrincipalFull,
}

impl OtAdmit {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Replayed => "ot_replayed",
            Self::GatewayFull => "ot_window_full",
            Self::PrincipalFull => "ot_principal_window_full",
        }
    }
}

/// One gateway's admitted exchanged tokens: each `(iss, jti)` until its expiry plus the leeway, and each principal's
/// count. **It refuses rather than evicts** (#757's confirm round, N1): an entry is never forgotten before its token
/// could no longer verify, so a full window is a hard ceiling — 65 536 / (lifetime + drift) tokens a second per
/// gateway, 16 384 / (lifetime + drift) per principal — not a floor that refuses honest tokens.
#[derive(Default)]
struct OtWindow {
    until: HashMap<(String, String), (String, u64)>,
    per_principal: HashMap<String, usize>,
}

impl OtWindow {
    fn prune(&mut self, now_ms: u64) {
        let per = &mut self.per_principal;
        self.until.retain(|_, (principal, until)| {
            let keep = *until > now_ms;
            if !keep
                && let Some(n) = per.get_mut(principal)
            {
                *n -= 1;
                if *n == 0 {
                    per.remove(principal);
                }
            }
            keep
        });
    }
}

/// Every gateway's admitted exchanged tokens at this provider (identity-and-lifecycle I5): in memory, so empty after a
/// restart — a token presented before it can be presented once more after it, while unexpired, and only in an envelope
/// signed by the gateway it is bound to. Lock-order row 78: one leaf lock per gateway, held for one admission.
pub(crate) struct OtReplayWindows {
    by_gateway: papaya::HashMap<crate::NodeId, std::sync::Arc<parking_lot::Mutex<OtWindow>>>,
    per_gateway: usize,
    per_principal: usize,
    /// Refusals at the window, by reason: `GET /stats` `exchanged_tokens_refused`.
    pub(crate) replayed: std::sync::atomic::AtomicU64,
    pub(crate) gateway_full: std::sync::atomic::AtomicU64,
    pub(crate) principal_full: std::sync::atomic::AtomicU64,
}

impl Default for OtReplayWindows {
    fn default() -> Self {
        Self::with_bounds(OT_WINDOW_PER_GATEWAY, OT_WINDOW_PER_PRINCIPAL)
    }
}

impl OtReplayWindows {
    pub(crate) fn with_bounds(per_gateway: usize, per_principal: usize) -> Self {
        Self {
            by_gateway: papaya::HashMap::new(),
            per_gateway,
            per_principal,
            replayed: Default::default(),
            gateway_full: Default::default(),
            principal_full: Default::default(),
        }
    }

    /// Admit `token`, carried by `gateway` for `principal`, at `now_ms`: once, and only while its principal's and its
    /// gateway's shares have room. Entries are held until `exp_ms + leeway_ms`.
    pub(crate) fn admit(&self, gateway: &crate::NodeId, principal: &str, token: &ExchangedToken, now_ms: u64, leeway_ms: u64) -> Result<(), OtAdmit> {
        use std::sync::atomic::Ordering;
        let window = std::sync::Arc::clone(self.by_gateway.pin().get_or_insert_with(gateway.clone(), Default::default));
        let mut w = window.lock();
        w.prune(now_ms);
        let key = (token.iss.clone(), token.jti.clone());
        let refused = if w.until.contains_key(&key) {
            Some(OtAdmit::Replayed)
        } else if w.until.len() >= self.per_gateway {
            Some(OtAdmit::GatewayFull)
        } else if w.per_principal.get(principal).copied().unwrap_or(0) >= self.per_principal {
            Some(OtAdmit::PrincipalFull)
        } else {
            None
        };
        if let Some(r) = refused {
            match r {
                OtAdmit::Replayed => self.replayed.fetch_add(1, Ordering::Relaxed),
                OtAdmit::GatewayFull => self.gateway_full.fetch_add(1, Ordering::Relaxed),
                OtAdmit::PrincipalFull => self.principal_full.fetch_add(1, Ordering::Relaxed),
            };
            return Err(r);
        }
        w.until.insert(key, (principal.to_string(), token.exp_ms.saturating_add(leeway_ms)));
        *w.per_principal.entry(principal.to_string()).or_default() += 1;
        Ok(())
    }
}
