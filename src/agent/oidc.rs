//! WS4 — generic OIDC bearer-token validation for the gateway.
//!
//! **Human-operator authentication, not agent identity** (orthogonal to the
//! NANDA/M16 agent-identity track — no forward-design is owed here). An operator
//! presents an OIDC JWT (from Entra / Okta / Auth0 / Keycloak — all
//! OIDC-conformant) as the gateway bearer; this module validates it and maps the
//! token's IdP groups to gateway scopes, so an OIDC principal is authorized
//! exactly like a [`GatewayToken`](crate::GatewayToken) — just authenticated by
//! signature instead of a shared secret.
//!
//! **Security posture.** We validate against an **explicit allowlist of
//! asymmetric algorithms** and never trust the token header's `alg` to select
//! the verification family — this closes the classic JWT alg-confusion bypass
//! (an attacker re-signing with `HS256` using the public key as the MAC secret).
//! Issuer, audience, and expiry are all checked. A token whose `kid` is not in
//! the configured key set is rejected.
//!
//! Gated behind the `compliance` feature. Vendor differences are configuration,
//! not code: discovery is the standard `.well-known/openid-configuration` + JWKS
//! (runtime fetch/cache lives in the gateway wiring).

use serde::Deserialize;

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

/// `OidcConfig` (the plain config struct) now lives in core `config` so
/// `GossipConfig` does not name an upper type; this module owns the verifier logic.
pub use crate::config::OidcConfig;

/// The asymmetric signature algorithms we accept. Symmetric (`HS*`) and `none`
/// are deliberately excluded — accepting them is the JWT alg-confusion bypass.
const ALLOWED_ALGS: &[Algorithm] = &[
    Algorithm::RS256, Algorithm::RS384, Algorithm::RS512,
    Algorithm::ES256, Algorithm::ES384,
    Algorithm::PS256, Algorithm::PS384, Algorithm::PS512,
];

/// Why an OIDC token was rejected. Coarse on purpose — the gateway answers a flat
/// 401, and finer detail goes only to logs (never leak validation specifics to
/// an unauthenticated caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OidcError {
    /// Header unparseable, missing `kid`, or a disallowed algorithm.
    Malformed,
    /// No configured key matches the token's `kid`.
    UnknownKid,
    /// Signature, issuer, audience, or expiry check failed.
    Invalid,
}

/// A validated OIDC principal: its subject and the gateway scopes its groups grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedOidcPrincipal {
    pub subject: String,
    pub scopes:  Vec<String>,
}

#[derive(Deserialize)]
struct RawClaims {
    #[serde(default)]
    sub: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// Validate `token` against the supplied `(kid, key)` set and `cfg`.
///
/// Steps, in order: parse the header; reject any algorithm outside
/// [`ALLOWED_ALGS`]; select the key by `kid`; verify signature + `iss` + `aud` +
/// `exp` with a `Validation` pinned to the allowed algorithms (never the header's
/// claimed alg); then extract `sub` and the configured group claim and map groups
/// to scopes. Returns [`VerifiedOidcPrincipal`] only if every check passes.
pub(crate) fn validate_token(
    cfg: &OidcConfig,
    keys: &[(String, DecodingKey)],
    token: &str,
) -> Result<VerifiedOidcPrincipal, OidcError> {
    let header = decode_header(token).map_err(|_| OidcError::Malformed)?;
    if !ALLOWED_ALGS.contains(&header.alg) {
        return Err(OidcError::Malformed); // HS*/none → alg-confusion attempt
    }
    let kid = header.kid.ok_or(OidcError::Malformed)?;
    let key = keys
        .iter()
        .find(|(k, _)| *k == kid)
        .map(|(_, k)| k)
        .ok_or(OidcError::UnknownKid)?;

    // The allowlist is enforced above (HS*/none → Malformed before we get here),
    // so `header.alg` is now a vetted asymmetric algorithm. Pin verification to
    // exactly that algorithm — a single family that matches the key — rather than
    // a mixed-family list (which jsonwebtoken rejects as InvalidAlgorithm).
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[cfg.issuer.as_str()]);
    validation.set_audience(&[cfg.audience.as_str()]);
    validation.validate_exp = true;
    // `set_issuer`/`set_audience` only validate the claim WHEN PRESENT; jsonwebtoken requires only
    // `exp` by default, so a token that simply OMITS `aud`/`iss` sails past both checks — an
    // audience/issuer-confusion bypass in shared-IdP deployments (audit 2026-07-15 pass 3). Require
    // all three to be present so a missing claim is rejected, not silently skipped.
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);

    let data = decode::<RawClaims>(token, key, &validation).map_err(|_| OidcError::Invalid)?;
    let claims = data.claims;

    let groups: Vec<String> = claims
        .extra
        .get(&cfg.group_claim)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|g| g.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    Ok(VerifiedOidcPrincipal {
        subject: claims.sub,
        scopes:  cfg.scopes_for_groups(&groups),
    })
}

// ── Runtime verifier: JWKS fetch + cache (gateway wiring) ─────────────────────

use std::time::Duration;

use mycelium_core::sim_seam;

/// Re-fetch the IdP's JWKS at most this often on the TTL path (an unknown `kid` can force an
/// earlier refresh, bounded by [`JWKS_REFRESH_COOLDOWN`]).
const JWKS_TTL: Duration = Duration::from_secs(3600);

/// The shortest interval between two JWKS fetch **attempts** that are not TTL expiries: a refresh
/// forced by an unknown `kid`, and a retry after a fetch that failed while keys are held (with none
/// held the spacing is [`JWKS_EMPTY_RETRY_BACKOFF`]).
///
/// Without it, every bearer the gateway sees is offered to OIDC first, so an unauthenticated client
/// that minted JWT headers with random `kid`s made the node issue one outbound request to the IdP per
/// gateway request — a free amplifier aimed at someone else's identity provider. Thirty seconds is
/// short against how IdPs rotate (a new key is published before it signs, typically hours or days
/// ahead), so a genuine rotation is picked up by the first token signed under it at most one
/// cooldown late, and long enough that the IdP sees at most two requests a minute from this node
/// whatever the traffic.
const JWKS_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

/// The deadline on one key fetch — discovery and the JWKS request **together** (connect, send,
/// body; each request gets what is left of it).
///
/// Without it, an IdP that accepts and never answers held the verify that triggered the fetch —
/// and the gateway handler awaiting it — indefinitely. Ten seconds is generous for two small JSON
/// documents, and bounded: a verify that needs a fetch waits at most this long before its token is
/// refused. A verify whose `kid` is already cached never waits for a fetch at all.
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// While the verifier holds **no keys at all** (a cold start, or no fetch has ever succeeded), a
/// failed fetch is retried after this, not after [`JWKS_REFRESH_COOLDOWN`]: an IdP that blipped as
/// the node started would otherwise have every token refused for 30 s after it recovered. Two
/// seconds still caps the retries an unauthenticated client can cause at one per back-off.
const JWKS_EMPTY_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// The verifier's timing, one place: the constants in production, shorter values in tests.
#[derive(Debug, Clone, Copy)]
struct Limits {
    ttl:              Duration,
    refresh_cooldown: Duration,
    empty_backoff:    Duration,
    fetch_timeout:    Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ttl:              JWKS_TTL,
            refresh_cooldown: JWKS_REFRESH_COOLDOWN,
            empty_backoff:    JWKS_EMPTY_RETRY_BACKOFF,
            fetch_timeout:    JWKS_FETCH_TIMEOUT,
        }
    }
}

#[derive(Default)]
struct CachedKeys {
    /// When the current `keys` were fetched; `None` until a fetch first succeeds.
    fetched_at: Option<std::time::Instant>,
    /// When the last fetch **finished**, whatever its outcome — the cooldown's reference point.
    /// Stamped by the fetch task itself, so a verify cancelled mid-fetch spends nothing.
    attempted_at: Option<std::time::Instant>,
    /// Whether that attempt came back with no keys (unreachable, timed out, refused by egress,
    /// unparsable).
    last_attempt_failed: bool,
    keys: Vec<(String, DecodingKey)>,
}

/// What a verify does with the cache as it finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Validate against the keys held now (which may lack the `kid`: then the token is refused).
    Use,
    /// The keys know the `kid` but are past the TTL: validate against them now and refresh them
    /// in the background.
    UseAndRefresh,
    /// Wait for a fetch (the `kid` is unknown and a fetch is due).
    Fetch,
}

impl CachedKeys {
    fn plan(&self, kid: &str, l: &Limits) -> Plan {
        let fresh = self.fetched_at.is_some_and(|at| sim_seam::mono_elapsed(&at) < l.ttl);
        let knows = self.keys.iter().any(|(k, _)| k == kid);
        if fresh && knows {
            return Plan::Use;
        }
        let due = match self.attempted_at {
            None => true,
            // Past the TTL after a fetch that succeeded: the TTL refresh, never held back.
            Some(_) if !fresh && !self.last_attempt_failed => true,
            // A refresh forced by an unknown `kid`, or a retry after a failure: spaced.
            Some(at) => {
                let spacing = if self.keys.is_empty() { l.empty_backoff } else { l.refresh_cooldown };
                sim_seam::mono_elapsed(&at) >= spacing
            }
        };
        match (due, knows) {
            (false, _) => Plan::Use,
            (true, true) => Plan::UseAndRefresh,
            (true, false) => Plan::Fetch,
        }
    }
}

/// Everything the detached fetch task needs, shared with the verifier.
struct Shared {
    cfg:    OidcConfig,
    http:   reqwest::Client,
    /// The node's outbound allow-list (`report.egress`, plan §8): discovery and the JWKS fetch are
    /// outbound calls the substrate chooses, and a host the policy denies is never dialled — the
    /// verifier then holds no keys and refuses every token. `start()` refuses the contradiction
    /// first, so this is the runtime's belt to that brace.
    egress: crate::config::EgressPolicy,
    limits: Limits,
    /// Lock-order row 17. A plain `std::sync` lock: read to plan and to clone keys, written only to
    /// swap in a finished fetch's outcome — never held across an `await`.
    cache:  std::sync::RwLock<CachedKeys>,
    /// Single-flight without a second lock: set by the verify that starts a fetch, cleared by the
    /// fetch task when it has written the cache, after which it wakes `fetched`'s waiters.
    in_flight: std::sync::atomic::AtomicBool,
    fetched:   tokio::sync::Notify,
}

/// Holds the OIDC config + a cached JWKS, and validates tokens against it. One per gateway.
/// A token whose `kid` is cached never waits for the network; a fetch happens on a cold cache, an
/// unknown `kid` or TTL expiry — single-flight, in a detached task (a cancelled verify does not
/// cancel it), the forced and retried ones at most once per [`JWKS_REFRESH_COOLDOWN`] (once per
/// [`JWKS_EMPTY_RETRY_BACKOFF`] while no keys are held), each within [`JWKS_FETCH_TIMEOUT`].
pub(crate) struct OidcVerifier {
    shared: std::sync::Arc<Shared>,
}

impl OidcVerifier {
    pub(crate) fn new(cfg: OidcConfig, egress: crate::config::EgressPolicy) -> Self {
        Self::with_limits(cfg, egress, Limits::default())
    }

    fn with_limits(cfg: OidcConfig, egress: crate::config::EgressPolicy, limits: Limits) -> Self {
        // Discovery and JWKS are gated by URL below; every redirect is re-checked as well, or keys
        // served by a denied host would be trusted (realignment repairs R3).
        let http = crate::agent::egress_client::build_or_none(crate::agent::egress_client::with_policy(&egress));
        Self {
            shared: std::sync::Arc::new(Shared {
                cfg,
                http,
                egress,
                limits,
                cache: std::sync::RwLock::new(CachedKeys::default()),
                in_flight: std::sync::atomic::AtomicBool::new(false),
                fetched: tokio::sync::Notify::new(),
            }),
        }
    }

    /// The IdP issuer every accepted JWT was validated against — the authority that qualifies an
    /// OIDC caller principal (`oidc:{issuer}/{subject}`, item 7 review finding 2).
    pub(crate) fn issuer(&self) -> &str {
        &self.shared.cfg.issuer
    }

    /// Validate `token`; `Some` only if signature, issuer, audience, and expiry
    /// all check out against the (possibly just-refreshed) JWKS.
    pub(crate) async fn verify(&self, token: &str) -> Option<VerifiedOidcPrincipal> {
        let header = decode_header(token).ok()?;
        if !ALLOWED_ALGS.contains(&header.alg) {
            return None;
        }
        let kid = header.kid.clone()?;
        let keys = self.keys_for(&kid).await;
        validate_token(&self.shared.cfg, &keys, token).ok()
    }

    /// The cache's plan for `kid`, and the keys held now — one read guard, released on return.
    fn plan(&self, kid: &str) -> (Plan, Vec<(String, DecodingKey)>) {
        let c = self.shared.cache.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        (c.plan(kid, &self.shared.limits), c.keys.clone())
    }

    /// The key set to validate a token signed under `kid` against.
    ///
    /// Only a verify whose plan is [`Plan::Fetch`] waits, and it waits for **one** fetch: it starts
    /// one if none is in flight, otherwise it waits for the running one, then plans again — the
    /// finished fetch has stamped the cache, so the second plan is `Use` (the new keys, or the
    /// cooldown running). The waiter registers for the wake-up *before* it plans, so a fetch that
    /// finishes in between cannot be missed.
    ///
    /// A verify waits for at most one fetch: after it, the plan is final even if a zero cooldown
    /// (a test's) would make another fetch due.
    async fn keys_for(&self, kid: &str) -> Vec<(String, DecodingKey)> {
        let mut waited = false;
        loop {
            let fetched = self.shared.fetched.notified();
            tokio::pin!(fetched);
            fetched.as_mut().enable();
            let (plan, keys) = self.plan(kid);
            match plan {
                Plan::Use => return keys,
                Plan::UseAndRefresh => {
                    self.start_fetch();
                    return keys;
                }
                Plan::Fetch if waited => return keys,
                Plan::Fetch => {
                    self.start_fetch();
                    fetched.await;
                    waited = true;
                }
            }
        }
    }

    /// Start the fetch task unless one is already running. The task, not the caller, owns the
    /// fetch: it writes its outcome to the cache, clears the flag and wakes every waiter even if
    /// the verify that started it has gone.
    fn start_fetch(&self) {
        use std::sync::atomic::Ordering;
        if self.shared.in_flight.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let shared = std::sync::Arc::clone(&self.shared);
        tokio::spawn(async move {
            /// Clears the flag and wakes the waiters however the task ends (a panic included), so a
            /// waiter can never be left on a fetch that is not running.
            struct Done(std::sync::Arc<Shared>);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.0.in_flight.store(false, std::sync::atomic::Ordering::Release);
                    self.0.fetched.notify_waiters();
                }
            }
            let done = Done(shared);
            let fetched = done.0.fetch_keys().await;
            let now = sim_seam::mono_instant();
            let mut c = done.0.cache.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            c.attempted_at = Some(now);
            c.last_attempt_failed = fetched.is_empty();
            // A failed fetch keeps the previous good set (no flapping to empty).
            if !fetched.is_empty() {
                c.fetched_at = Some(now);
                c.keys = fetched;
            }
        });
    }

    #[cfg(test)]
    async fn fetch_keys(&self) -> Vec<(String, DecodingKey)> {
        self.shared.fetch_keys().await
    }
}

impl Shared {
    /// Resolve the JWKS URI (explicit, or via `.well-known/openid-configuration`), fetch it, and
    /// build `(kid, DecodingKey)` pairs, all within one `fetch_timeout`. Returns empty on any
    /// failure (logged) — the verify then finds no matching key and rejects.
    async fn fetch_keys(&self) -> Vec<(String, DecodingKey)> {
        let started = sim_seam::mono_instant();
        // What is left of the one deadline; a request is never sent with nothing left.
        let remaining = || self.limits.fetch_timeout.checked_sub(sim_seam::mono_elapsed(&started)).filter(|d| !d.is_zero());
        let Some(jwks_uri) = self.resolve_jwks_uri(&remaining).await else {
            return Vec::new();
        };
        if !self.egress.permits_url(&jwks_uri) {
            tracing::warn!(%jwks_uri, "oidc: the egress policy does not permit the JWKS host; no keys, every token refused");
            return Vec::new();
        }
        let Some(left) = remaining() else {
            tracing::warn!("oidc: JWKS fetch skipped: discovery used the whole fetch deadline");
            return Vec::new();
        };
        let jwks: jsonwebtoken::jwk::JwkSet = match self.http.get(&jwks_uri).timeout(left).send().await {
            Ok(r) => match r.json().await {
                Ok(j) => j,
                Err(e) => { tracing::warn!("oidc: JWKS parse failed: {e}"); return Vec::new(); }
            },
            Err(e) => { tracing::warn!("oidc: JWKS fetch failed: {e}"); return Vec::new(); }
        };
        jwks.keys
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                DecodingKey::from_jwk(jwk).ok().map(|k| (kid, k))
            })
            .collect()
    }

    async fn resolve_jwks_uri(&self, remaining: &impl Fn() -> Option<Duration>) -> Option<String> {
        if let Some(u) = &self.cfg.jwks_uri {
            return Some(u.clone());
        }
        // OIDC discovery.
        let disco = format!("{}/.well-known/openid-configuration", self.cfg.issuer.trim_end_matches('/'));
        if !self.egress.permits_url(&disco) {
            tracing::warn!(%disco, "oidc: the egress policy does not permit the issuer host; discovery skipped");
            return None;
        }
        let doc: serde_json::Value = match self.http.get(&disco).timeout(remaining()?).send().await {
            Ok(r) => r.json().await.ok()?,
            Err(e) => { tracing::warn!("oidc: discovery failed: {e}"); return None; }
        };
        doc.get("jwks_uri")?.as_str().map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    // 2048-bit RSA test keypair (test-only; never used in production).
    const TEST_PRIV: &str = include_str!("../../tests/fixtures/oidc_test.key");
    const TEST_PUB:  &str = include_str!("../../tests/fixtures/oidc_test.pub");
    const OTHER_PUB: &str = include_str!("../../tests/fixtures/oidc_other.pub");

    fn now() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    fn cfg() -> OidcConfig {
        let mut group_scopes = HashMap::new();
        group_scopes.insert("admins".to_string(), vec!["*".to_string()]);
        group_scopes.insert("readers".to_string(), vec!["kv:read".to_string()]);
        OidcConfig {
            issuer: "https://idp.example".into(),
            audience: "mycelium-cluster".into(),
            group_claim: "groups".into(),
            group_scopes,
            jwks_uri: None,
        }
    }

    fn keys() -> Vec<(String, DecodingKey)> {
        vec![("test-kid".to_string(), DecodingKey::from_rsa_pem(TEST_PUB.as_bytes()).unwrap())]
    }

    /// Mint a signed JWT with the test key under `kid` "test-kid" / RS256.
    fn mint(claims: serde_json::Value, alg: Algorithm, kid: &str, priv_pem: &str) -> String {
        let mut header = Header::new(alg);
        header.kid = Some(kid.to_string());
        let key = EncodingKey::from_rsa_pem(priv_pem.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    fn valid_claims() -> serde_json::Value {
        json!({
            "sub": "alice@example",
            "iss": "https://idp.example",
            "aud": "mycelium-cluster",
            "exp": now() + 3600,
            "groups": ["admins", "readers"],
        })
    }

    #[test]
    fn valid_token_maps_groups_to_scopes() {
        let p = validate_token(&cfg(), &keys(), &mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV)).unwrap();
        assert_eq!(p.subject, "alice@example");
        assert!(p.scopes.contains(&"*".to_string()));
        assert!(p.scopes.contains(&"kv:read".to_string()));
    }

    #[test]
    fn expired_token_is_rejected() {
        let mut c = valid_claims();
        c["exp"] = json!(now() - 7200); // well beyond jsonwebtoken's default 60s leeway
        assert_eq!(validate_token(&cfg(), &keys(), &mint(c, Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let mut c = valid_claims();
        c["iss"] = json!("https://evil.example");
        assert_eq!(validate_token(&cfg(), &keys(), &mint(c, Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let mut c = valid_claims();
        c["aud"] = json!("some-other-app");
        assert_eq!(validate_token(&cfg(), &keys(), &mint(c, Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn regression_missing_audience_is_rejected() {
        // audit 2026-07-15 pass 3: `set_audience` only validates `aud` WHEN PRESENT, and jsonwebtoken
        // requires only `exp` by default — so a token that OMITS `aud` bypassed the audience check
        // (audience-confusion in shared-IdP deployments). `set_required_spec_claims` now rejects it.
        let mut c = valid_claims();
        c.as_object_mut().unwrap().remove("aud");
        assert_eq!(validate_token(&cfg(), &keys(), &mint(c, Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn regression_missing_issuer_is_rejected() {
        // Same bypass on the issuer claim — a token omitting `iss` must be rejected.
        let mut c = valid_claims();
        c.as_object_mut().unwrap().remove("iss");
        assert_eq!(validate_token(&cfg(), &keys(), &mint(c, Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn wrong_signing_key_is_rejected() {
        // Signed with TEST_PRIV but the verifier only knows a different public key.
        let keys = vec![("test-kid".to_string(), DecodingKey::from_rsa_pem(OTHER_PUB.as_bytes()).unwrap())];
        assert_eq!(validate_token(&cfg(), &keys, &mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV)), Err(OidcError::Invalid));
    }

    #[test]
    fn unknown_kid_is_rejected() {
        assert_eq!(validate_token(&cfg(), &keys(), &mint(valid_claims(), Algorithm::RS256, "other-kid", TEST_PRIV)), Err(OidcError::UnknownKid));
    }

    #[test]
    fn hs256_alg_confusion_is_rejected() {
        // Forge an HS256 token using the RSA *public* key bytes as the HMAC secret —
        // the classic alg-confusion attack. Must be rejected as Malformed (alg not
        // in the asymmetric allowlist), never validated.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-kid".to_string());
        let secret = EncodingKey::from_secret(TEST_PUB.as_bytes());
        let forged = encode(&header, &valid_claims(), &secret).unwrap();
        assert_eq!(validate_token(&cfg(), &keys(), &forged), Err(OidcError::Malformed));
    }

    #[test]
    fn garbage_token_is_malformed() {
        assert_eq!(validate_token(&cfg(), &keys(), "not.a.jwt"), Err(OidcError::Malformed));
    }

    #[test]
    fn jwks_fixture_builds_a_working_key() {
        // The JWKS fixture must carry the test public key correctly (modulus/exp),
        // so a token signed by TEST_PRIV verifies against a key built from the JWK.
        let jwks: jsonwebtoken::jwk::JwkSet =
            serde_json::from_str(include_str!("../../tests/fixtures/oidc_jwks.json")).unwrap();
        let keys: Vec<(String, DecodingKey)> = jwks
            .keys
            .iter()
            .filter_map(|j| {
                let kid = j.common.key_id.clone()?;
                DecodingKey::from_jwk(j).ok().map(|k| (kid, k))
            })
            .collect();
        assert_eq!(keys.len(), 1, "fixture should yield one key");
        let token = mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV);
        let p = validate_token(&cfg(), &keys, &token).expect("JWK-built key must verify the token");
        assert_eq!(p.subject, "alice@example");
    }

    #[test]
    fn scopes_for_groups_unions_and_dedups() {
        let c = cfg();
        assert_eq!(c.scopes_for_groups(&["readers".into()]), vec!["kv:read".to_string()]);
        assert!(c.scopes_for_groups(&["unknown".into()]).is_empty());
        // admins → "*"; duplicate groups don't duplicate scopes.
        let s = c.scopes_for_groups(&["admins".into(), "admins".into()]);
        assert_eq!(s, vec!["*".to_string()]);
    }
    /// **Realignment repairs R3 (F03's OIDC row).** The JWKS URL was gated, and then fetched with a
    /// client that followed redirects — so keys served by a host the egress list denies would be
    /// trusted. The fetch now re-checks every hop: a redirect to a denied host reaches it zero times
    /// and the verifier holds no keys.
    #[tokio::test]
    async fn a_jwks_redirect_to_a_denied_host_is_not_followed() {
        use crate::test_util::{spawn_counting_listener, spawn_redirector};
        use std::sync::atomic::Ordering;
        let (denied, hits) = spawn_counting_listener(r#"{"keys":[]}"#).await;
        let hop = spawn_redirector(302, format!("http://127.0.0.1:{denied}/jwks")).await;
        let mut c = cfg();
        c.jwks_uri = Some(format!("http://localhost:{hop}/jwks"));
        let v = OidcVerifier::new(c, crate::config::EgressPolicy { allow_hosts: vec!["localhost".into()] });
        assert!(v.fetch_keys().await.is_empty());
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the denied host is never contacted");
    }

    // ── JWKS refresh: single-flight, a bounded forced refresh, a fetch timeout ──────────────────

    const JWKS_BODY: &str = include_str!("../../tests/fixtures/oidc_jwks.json");

    /// A config whose JWKS is the counting stub on `port` (explicit `jwks_uri`, so no discovery).
    fn stub_cfg(port: u16) -> OidcConfig {
        let mut c = cfg();
        c.jwks_uri = Some(format!("http://127.0.0.1:{port}/jwks"));
        c
    }

    fn open_egress() -> crate::config::EgressPolicy {
        crate::config::EgressPolicy { allow_hosts: Vec::new() }
    }

    /// **Single-flight (lock-order row 17).** Sixteen concurrent verifies of a token whose `kid` the
    /// IdP does not serve, on a cold cache: before the fix each one fetched (cold), then each one
    /// fetched again (forced by the unknown `kid`) — up to 32 requests to the IdP for one burst. The
    /// write guard is now held across the fetch and the forced refresh is bounded by the cooldown,
    /// so the burst costs the IdP exactly one request.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_verifies_with_an_unknown_kid_fetch_the_jwks_once() {
        use std::sync::atomic::Ordering;
        let (port, hits) = crate::test_util::spawn_counting_listener(JWKS_BODY).await;
        let v = std::sync::Arc::new(OidcVerifier::new(stub_cfg(port), open_egress()));
        let token = mint(valid_claims(), Algorithm::RS256, "rotated-away", TEST_PRIV);
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let (v, token) = (std::sync::Arc::clone(&v), token.clone());
            set.spawn(async move { v.verify(&token).await });
        }
        while let Some(r) = set.join_next().await {
            assert!(r.expect("verify task").is_none(), "an unknown kid is refused");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "one burst, one JWKS fetch");
        // The keys that one fetch stored still serve a known kid without another fetch.
        assert!(v.verify(&mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV)).await.is_some());
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a known kid inside the TTL is served from the cache");
    }

    /// **A failed fetch is retried on a spacing**, not per request (here the no-keys back-off): an
    /// IdP serving garbage (or down) used to be asked twice per verify — the cold fetch, then the
    /// refresh the resulting unknown `kid` forced.
    #[tokio::test]
    async fn a_failed_jwks_fetch_is_not_retried_inside_the_cooldown() {
        use std::sync::atomic::Ordering;
        let (port, hits) = crate::test_util::spawn_counting_listener("not a jwks").await;
        let v = OidcVerifier::new(stub_cfg(port), open_egress());
        let token = mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV);
        for _ in 0..3 {
            assert!(v.verify(&token).await.is_none(), "no keys, every token refused");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "one attempt, then the cooldown");
    }

    // ── Review findings on #590: a scripted IdP stub ─────────────────────────────────────────────

    /// What the stub does with its `n`th connection (0-based): answer `body` after `delay`, or
    /// accept and never answer (`body: None`).
    #[derive(Clone)]
    struct Reply {
        delay: Duration,
        body:  Option<String>,
    }

    fn answer(body: impl Into<String>) -> Reply {
        Reply { delay: Duration::ZERO, body: Some(body.into()) }
    }

    fn hang() -> Reply {
        Reply { delay: Duration::ZERO, body: None }
    }

    /// A listener that counts connections and answers the `n`th with `script(n)`.
    async fn spawn_scripted(
        script: impl Fn(usize) -> Reply + Send + Sync + 'static,
    ) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::Ordering;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&count);
        let script = std::sync::Arc::new(script);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                let reply = script(n);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    let mut buf = [0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    tokio::time::sleep(reply.delay).await;
                    match reply.body {
                        Some(body) => {
                            let r = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = sock.write_all(r.as_bytes()).await;
                        }
                        None => {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                            drop(sock);
                        }
                    }
                });
            }
        });
        (port, count)
    }

    /// The fixture JWKS with its one key published under `kid` instead of `test-kid`.
    fn jwks_as(kid: &str) -> String {
        JWKS_BODY.replace("\"test-kid\"", &format!("\"{kid}\""))
    }

    fn token(kid: &str) -> String {
        mint(valid_claims(), Algorithm::RS256, kid, TEST_PRIV)
    }

    fn limits(f: impl FnOnce(&mut Limits)) -> Limits {
        let mut l = Limits::default();
        f(&mut l);
        l
    }

    fn hits(h: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> usize {
        h.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// **The forced refresh is bounded.** An unauthenticated client choosing random `kid`s used to
    /// force one outbound JWKS fetch per gateway request. Inside the cooldown an unknown `kid` is
    /// refused without a fetch; once the cooldown has passed one more forced refresh is allowed.
    /// (Review finding 6: a 2 s cooldown and wide sleeps, so a slow CI runner cannot flake it.)
    #[tokio::test]
    async fn an_unknown_kid_forces_at_most_one_refresh_per_cooldown() {
        let (port, n) = crate::test_util::spawn_counting_listener(JWKS_BODY).await;
        let cooldown = Duration::from_secs(2);
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.refresh_cooldown = cooldown));
        assert!(v.verify(&token("test-kid")).await.is_some());
        assert_eq!(hits(&n), 1);
        for kid in ["random-1", "random-2", "random-3"] {
            assert!(v.verify(&token(kid)).await.is_none());
        }
        assert_eq!(hits(&n), 1, "no forced refresh inside the cooldown");
        tokio::time::sleep(cooldown + Duration::from_secs(1)).await;
        assert!(v.verify(&token("random-4")).await.is_none());
        assert_eq!(hits(&n), 2, "one forced refresh once the cooldown has passed");
        assert!(v.verify(&token("random-5")).await.is_none());
        assert_eq!(hits(&n), 2, "and the next unknown kid waits out a new cooldown");
    }

    /// **The fetch has a deadline.** An IdP that accepts and never answers held the verify — and the
    /// gateway handler awaiting it — indefinitely.
    #[tokio::test]
    async fn a_jwks_endpoint_that_never_answers_does_not_hold_verify() {
        let (port, _) = spawn_scripted(|_| hang()).await;
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.fetch_timeout = Duration::from_millis(500)));
        let out = tokio::time::timeout(Duration::from_secs(5), v.verify(&token("test-kid"))).await;
        assert!(matches!(out, Ok(None)), "verify returns (refusing) within the fetch timeout, got {out:?}");
    }

    /// **Review finding 1.** A token whose `kid` is cached and fresh never waits behind a fetch.
    /// The forced refresh below stalls on an IdP that never answers; a valid token issued meanwhile
    /// must still verify at once — it used to queue behind the write guard for the whole fetch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cached_kid_does_not_wait_behind_a_stalled_fetch() {
        let (port, n) = spawn_scripted(|i| if i == 0 { answer(JWKS_BODY) } else { hang() }).await;
        let v = std::sync::Arc::new(OidcVerifier::with_limits(
            stub_cfg(port),
            open_egress(),
            limits(|l| {
                l.refresh_cooldown = Duration::ZERO;
                l.fetch_timeout = Duration::from_secs(8);
            }),
        ));
        assert!(v.verify(&token("test-kid")).await.is_some(), "warm");
        let stalled = {
            let v = std::sync::Arc::clone(&v);
            tokio::spawn(async move { v.verify(&token("rotated")).await })
        };
        for _ in 0..100 {
            if hits(&n) == 2 { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(hits(&n), 2, "the forced refresh is in flight");
        let quick = tokio::time::timeout(Duration::from_secs(2), v.verify(&token("test-kid"))).await;
        assert!(matches!(quick, Ok(Some(_))), "a cached, fresh kid verifies while the fetch stalls, got {quick:?}");
        stalled.abort();
    }

    /// **Review finding 2.** A verify that started a fetch and was then cancelled (the client
    /// hung up) must not spend the cooldown without fetching: the fetch completes on its own and
    /// the next verify uses its keys.
    #[tokio::test]
    async fn a_cancelled_verify_does_not_cancel_its_fetch() {
        let (port, n) = spawn_scripted(|_| Reply { delay: Duration::from_millis(500), body: Some(JWKS_BODY.into()) }).await;
        let v = OidcVerifier::new(stub_cfg(port), open_egress());
        let cancelled = tokio::time::timeout(Duration::from_millis(100), v.verify(&token("test-kid"))).await;
        assert!(cancelled.is_err(), "the first verify is dropped mid-fetch");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let later = tokio::time::timeout(Duration::from_secs(2), v.verify(&token("test-kid"))).await;
        assert!(matches!(later, Ok(Some(_))), "the detached fetch's keys are used, got {later:?}");
        assert_eq!(hits(&n), 1, "and no second fetch was needed");
    }

    /// **Review finding 3.** The deadline covers discovery and the JWKS request together: a
    /// discovery answer at 0.9 × the timeout followed by a JWKS host that hangs used to take
    /// almost twice the timeout.
    #[tokio::test]
    async fn discovery_and_jwks_share_one_deadline() {
        let (jwks_port, _) = spawn_scripted(|_| hang()).await;
        let timeout = Duration::from_secs(2);
        let doc = format!(r#"{{"jwks_uri":"http://127.0.0.1:{jwks_port}/jwks"}}"#);
        let (disco_port, _) = spawn_scripted(move |_| Reply { delay: timeout.mul_f64(0.9), body: Some(doc.clone()) }).await;
        let mut c = cfg();
        c.issuer = format!("http://127.0.0.1:{disco_port}");
        let v = OidcVerifier::with_limits(c, open_egress(), limits(|l| l.fetch_timeout = timeout));
        let t0 = std::time::Instant::now();
        assert!(v.verify(&token("test-kid")).await.is_none());
        let took = t0.elapsed();
        assert!(took < timeout.mul_f64(1.4), "one deadline for the pair: took {took:?}, timeout {timeout:?}");
    }

    /// **Review finding 4.** A rotated key is accepted once the cooldown has passed.
    #[tokio::test]
    async fn a_rotated_key_is_accepted_after_the_cooldown() {
        let (port, n) = spawn_scripted(|i| answer(if i == 0 { jwks_as("test-kid") } else { jwks_as("rotated") })).await;
        let cooldown = Duration::from_secs(2);
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.refresh_cooldown = cooldown));
        assert!(v.verify(&token("test-kid")).await.is_some());
        assert!(v.verify(&token("rotated")).await.is_none(), "inside the cooldown: refused, no fetch");
        assert_eq!(hits(&n), 1);
        tokio::time::sleep(cooldown + Duration::from_secs(1)).await;
        assert!(v.verify(&token("rotated")).await.is_some(), "after it: one refresh, the new key accepted");
        assert_eq!(hits(&n), 2);
    }

    /// **Review finding 4.** A refresh that fails keeps the previous good keys.
    #[tokio::test]
    async fn a_failed_refresh_keeps_the_previous_keys() {
        let (port, n) = spawn_scripted(|i| answer(if i == 0 { JWKS_BODY.to_string() } else { "garbage".to_string() })).await;
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.refresh_cooldown = Duration::ZERO));
        assert!(v.verify(&token("test-kid")).await.is_some());
        assert!(v.verify(&token("unknown")).await.is_none(), "forces a refresh, which fails");
        assert_eq!(hits(&n), 2);
        assert!(v.verify(&token("test-kid")).await.is_some(), "the previous keys still verify");
    }

    /// **Review finding 4.** Past the TTL the keys are refreshed; a token under a cached `kid` is
    /// served from the previous keys meanwhile.
    #[tokio::test]
    async fn keys_past_the_ttl_are_refreshed() {
        let (port, n) = spawn_scripted(|_| answer(JWKS_BODY)).await;
        let ttl = Duration::from_secs(1);
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.ttl = ttl));
        assert!(v.verify(&token("test-kid")).await.is_some());
        assert!(v.verify(&token("test-kid")).await.is_some());
        assert_eq!(hits(&n), 1, "inside the TTL, cached");
        tokio::time::sleep(ttl + Duration::from_millis(500)).await;
        assert!(v.verify(&token("test-kid")).await.is_some());
        for _ in 0..100 {
            if hits(&n) == 2 { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(hits(&n), 2, "past the TTL, one refresh");
    }

    /// **Review finding 5.** With no keys at all (an IdP blip at start), the retry waits a short
    /// back-off, not the full cooldown — a node does not refuse every token for 30 s after the IdP
    /// recovers.
    #[tokio::test]
    async fn with_no_keys_a_failed_fetch_is_retried_after_the_short_backoff() {
        let (port, n) = spawn_scripted(|i| answer(if i == 0 { "garbage".to_string() } else { JWKS_BODY.to_string() })).await;
        let backoff = Duration::from_secs(1);
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), limits(|l| l.empty_backoff = backoff));
        assert!(v.verify(&token("test-kid")).await.is_none(), "the IdP blips");
        assert!(v.verify(&token("test-kid")).await.is_none(), "inside the back-off: no retry");
        assert_eq!(hits(&n), 1);
        tokio::time::sleep(backoff + Duration::from_secs(1)).await;
        assert!(v.verify(&token("test-kid")).await.is_some(), "after it: retried, accepted");
        assert_eq!(hits(&n), 2);
    }
}
