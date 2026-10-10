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
/// forced by an unknown `kid`, and a retry after a fetch that failed.
///
/// Without it, every bearer the gateway sees is offered to OIDC first, so an unauthenticated client
/// that minted JWT headers with random `kid`s made the node issue one outbound request to the IdP per
/// gateway request — a free amplifier aimed at someone else's identity provider. Thirty seconds is
/// short against how IdPs rotate (a new key is published before it signs, typically hours or days
/// ahead), so a genuine rotation is picked up by the first token signed under it at most one
/// cooldown late, and long enough that the IdP sees at most two requests a minute from this node
/// whatever the traffic.
const JWKS_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

/// The deadline on each discovery and JWKS request (the whole request: connect, send, body).
///
/// The fetch runs under the cache's write guard (single-flight), so a request with no deadline to
/// an IdP that accepts and never answers would hold every verify — and every gateway handler
/// awaiting one — indefinitely. Ten seconds is generous for a small JSON document from an IdP, and
/// bounded: a handler waits at most this long for keys before the token is refused.
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct CachedKeys {
    /// When the current `keys` were fetched; `None` until a fetch first succeeds.
    fetched_at: Option<std::time::Instant>,
    /// When a fetch was last attempted, whatever its outcome — the cooldown's reference point.
    attempted_at: Option<std::time::Instant>,
    /// Whether that attempt came back with no keys (unreachable, refused by egress, unparsable).
    last_attempt_failed: bool,
    keys: Vec<(String, DecodingKey)>,
}

impl CachedKeys {
    /// Keys fetched within the TTL.
    fn fresh(&self) -> bool {
        self.fetched_at.is_some_and(|at| sim_seam::mono_elapsed(&at) < JWKS_TTL)
    }

    fn knows(&self, kid: &str) -> bool {
        self.keys.iter().any(|(k, _)| k == kid)
    }
}

/// Holds the OIDC config + a cached JWKS, and validates tokens against it. One
/// per gateway; `verify` is cheap on the hot path (a read-lock + cached keys),
/// fetching only on cold cache, TTL expiry, or an unknown `kid` — single-flight, the forced and
/// retried fetches at most once per [`JWKS_REFRESH_COOLDOWN`], each within [`JWKS_FETCH_TIMEOUT`].
pub(crate) struct OidcVerifier {
    cfg:    OidcConfig,
    http:   reqwest::Client,
    /// The node's outbound allow-list (`report.egress`, plan §8): discovery and the JWKS fetch are
    /// outbound calls the substrate chooses, and a host the policy denies is never dialled — the
    /// verifier then holds no keys and refuses every token. `start()` refuses the contradiction
    /// first, so this is the runtime's belt to that brace.
    egress: crate::config::EgressPolicy,
    /// Lock-order row 17 — the one `tokio::sync` lock whose write guard is held across I/O: the
    /// JWKS fetch runs under it, so concurrent verifies needing a refresh wait for one fetch rather
    /// than each issuing their own.
    cache:  tokio::sync::RwLock<CachedKeys>,
    refresh_cooldown: Duration,
}

impl OidcVerifier {
    pub(crate) fn new(cfg: OidcConfig, egress: crate::config::EgressPolicy) -> Self {
        Self::with_limits(cfg, egress, JWKS_REFRESH_COOLDOWN, JWKS_FETCH_TIMEOUT)
    }

    /// `new` with the cooldown and the request deadline given — the tests' door to both.
    fn with_limits(
        cfg: OidcConfig,
        egress: crate::config::EgressPolicy,
        refresh_cooldown: Duration,
        fetch_timeout: Duration,
    ) -> Self {
        // Discovery and JWKS are gated by URL below; every redirect is re-checked as well, or keys
        // served by a denied host would be trusted (realignment repairs R3).
        let http = crate::agent::egress_client::build_or_none(
            crate::agent::egress_client::with_policy(&egress).timeout(fetch_timeout),
        );
        Self { cfg, http, egress, cache: tokio::sync::RwLock::new(CachedKeys::default()), refresh_cooldown }
    }

    /// The IdP issuer every accepted JWT was validated against — the authority that qualifies an
    /// OIDC caller principal (`oidc:{issuer}/{subject}`, item 7 review finding 2).
    pub(crate) fn issuer(&self) -> &str {
        &self.cfg.issuer
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
        validate_token(&self.cfg, &keys, token).ok()
    }

    /// The key set to validate a token signed under `kid` against.
    ///
    /// Hot path: fresh keys that know `kid`, under the read guard. Otherwise the write guard is
    /// taken and **held across the fetch** (single-flight), and the state is re-read under it, so a
    /// caller that queued behind a refresh uses that refresh's keys instead of fetching again. A
    /// fetch then happens when the keys are cold or past the TTL, or when `kid` is unknown — but the
    /// last case, and a retry after a failed fetch, only once per cooldown: inside it the current
    /// keys are returned as they are and an unknown `kid` is refused by `validate_token`. A failed
    /// fetch keeps the previous good set (no flapping to empty).
    async fn keys_for(&self, kid: &str) -> Vec<(String, DecodingKey)> {
        {
            let guard = self.cache.read().await;
            if guard.fresh() && guard.knows(kid) {
                return guard.keys.clone();
            }
        }
        let mut guard = self.cache.write().await;
        let fresh = guard.fresh();
        if fresh && guard.knows(kid) {
            return guard.keys.clone(); // a refresh we queued behind brought the key
        }
        let in_cooldown = guard.attempted_at.is_some_and(|at| sim_seam::mono_elapsed(&at) < self.refresh_cooldown);
        // Fresh keys without `kid` → a forced refresh; stale keys whose last attempt failed → a retry.
        // Both wait out the cooldown. A first fetch, and the first attempt past the TTL, do not.
        let retry_after_failure = !fresh && guard.last_attempt_failed;
        if in_cooldown && (fresh || retry_after_failure) {
            return guard.keys.clone();
        }
        let now = sim_seam::mono_instant();
        guard.attempted_at = Some(now);
        let fetched = self.fetch_keys().await;
        guard.last_attempt_failed = fetched.is_empty();
        if !fetched.is_empty() {
            guard.fetched_at = Some(now);
            guard.keys = fetched;
        }
        guard.keys.clone()
    }

    /// Resolve the JWKS URI (explicit, or via `.well-known/openid-configuration`),
    /// fetch it, and build `(kid, DecodingKey)` pairs. Returns empty on any failure
    /// (logged) — `verify` then simply finds no matching key and rejects.
    async fn fetch_keys(&self) -> Vec<(String, DecodingKey)> {
        let jwks_uri = match self.resolve_jwks_uri().await {
            Some(u) => u,
            None => return Vec::new(),
        };
        if !self.egress.permits_url(&jwks_uri) {
            tracing::warn!(%jwks_uri, "oidc: the egress policy does not permit the JWKS host; no keys, every token refused");
            return Vec::new();
        }
        let jwks: jsonwebtoken::jwk::JwkSet = match self.http.get(&jwks_uri).send().await {
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

    async fn resolve_jwks_uri(&self) -> Option<String> {
        if let Some(u) = &self.cfg.jwks_uri {
            return Some(u.clone());
        }
        // OIDC discovery.
        let disco = format!("{}/.well-known/openid-configuration", self.cfg.issuer.trim_end_matches('/'));
        if !self.egress.permits_url(&disco) {
            tracing::warn!(%disco, "oidc: the egress policy does not permit the issuer host; discovery skipped");
            return None;
        }
        let doc: serde_json::Value = self.http.get(&disco).send().await.ok()?.json().await.ok()?;
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

    /// **The forced refresh is bounded.** An unauthenticated client choosing random `kid`s used to
    /// force one outbound JWKS fetch per gateway request. Inside the cooldown an unknown `kid` is now
    /// refused without a fetch; once the cooldown has passed one more forced refresh is allowed (key
    /// rotation is still picked up).
    #[tokio::test]
    async fn an_unknown_kid_forces_at_most_one_refresh_per_cooldown() {
        use std::sync::atomic::Ordering;
        let (port, hits) = crate::test_util::spawn_counting_listener(JWKS_BODY).await;
        let cooldown = Duration::from_millis(300);
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), cooldown, JWKS_FETCH_TIMEOUT);
        // Warm the cache with a known kid: one fetch.
        assert!(v.verify(&mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV)).await.is_some());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Unknown kids inside the cooldown that began with that fetch: refused, no fetch.
        for kid in ["random-1", "random-2", "random-3"] {
            assert!(v.verify(&mint(valid_claims(), Algorithm::RS256, kid, TEST_PRIV)).await.is_none());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1, "no forced refresh inside the cooldown");
        // After the cooldown: exactly one forced refresh, then the cooldown applies again.
        tokio::time::sleep(cooldown + Duration::from_millis(100)).await;
        assert!(v.verify(&mint(valid_claims(), Algorithm::RS256, "random-4", TEST_PRIV)).await.is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one forced refresh once the cooldown has passed");
        assert!(v.verify(&mint(valid_claims(), Algorithm::RS256, "random-5", TEST_PRIV)).await.is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 2, "and the next unknown kid waits out a new cooldown");
    }

    /// **The fetch has a deadline.** The client set no timeout, so an IdP that accepts the connection
    /// and never answers held the verify — and the gateway handler awaiting it — indefinitely.
    #[tokio::test]
    async fn a_jwks_endpoint_that_never_answers_does_not_hold_verify() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock); // accept, read nothing, answer nothing
            }
        });
        let v = OidcVerifier::with_limits(stub_cfg(port), open_egress(), Duration::from_secs(30), Duration::from_millis(500));
        let token = mint(valid_claims(), Algorithm::RS256, "test-kid", TEST_PRIV);
        let out = tokio::time::timeout(Duration::from_secs(5), v.verify(&token)).await;
        assert!(matches!(out, Ok(None)), "verify returns (refusing) within the fetch timeout, got {out:?}");
    }

    /// **A failed fetch is retried on the cooldown's spacing**, not per request: an IdP serving
    /// garbage (or down) used to be asked twice per verify — the cold fetch, then the refresh the
    /// resulting unknown `kid` forced.
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
}
