//! **The gateway's token exchange** (identity-and-lifecycle I5, D6; spike S-exchange,
//! `docs/plans/spikes/2026-11-11-s-exchange.md`).
//!
//! A gateway whose `[oidc]` table sets `exchange_audience` exchanges an OIDC user's own bearer at the IdP's token
//! endpoint (RFC 8693) for a token whose audience is `exchange_audience`, under its own client credential, with a
//! DPoP proof (RFC 9449) signed by its Ed25519 node key ([`dpop_proof`]): the IdP binds the token to that key
//! (`cnf.jkt`), and the provider checks the binding against the key that signed the envelope
//! ([`exchanged_token::check`](super::exchanged_token::check)). It exchanges only for the calls that need one — an
//! `oidc:` principal's action, the kinds [`needs_exchanged_token`](super::exchanged_token::needs_exchanged_token)
//! names (decided by the maintainer, 2026-11-11) — once per call, and never forwards the user's own token.
//!
//! **The node key signs a JWS here.** A DPoP proof's signing input is `BASE64URL(header) "." BASE64URL(payload)`:
//! printable text beginning `eyJ`, with no zero byte, never `{` first — none of the three shapes every Mycelium message
//! the node key signs keeps (the plan's I5 row, #757's second confirm round), so neither verifies as the other.

use std::time::Duration;

use crate::config::{EgressPolicy, GossipConfig};
use crate::error::GossipError;

/// The deadline on one exchange (connect, send and body). An IdP that never answers holds the call at most this long,
/// then the dispatch is refused `oidc_exchange_unreachable`.
pub(crate) const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a gateway exchanges with: read and checked once, at `start()`.
pub(crate) struct OidcExchanger {
    endpoint: String,
    client_id: String,
    /// The client secret, read from the environment at `start()`.
    secret: String,
    audience: String,
    http: reqwest::Client,
    egress: EgressPolicy,
}

impl std::fmt::Debug for OidcExchanger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcExchanger").field("endpoint", &self.endpoint).field("client_id", &self.client_id).field("audience", &self.audience).finish_non_exhaustive()
    }
}

fn refused(field: &'static str, reason: String) -> GossipError {
    GossipError::InvalidField { field, reason }
}

/// The most bytes the IdP's answer to an exchange may take (#760's review, L1): a token response is one token, a
/// few KiB.
pub(crate) const EXCHANGE_RESPONSE_MAX_BYTES: usize = 64 * 1024;

impl OidcExchanger {
    /// The exchanger a node's configuration asks for, built in `preflight()` (so `mycelium doctor` reads it too) without
    /// any request. **A node exchanges iff `exchange_token_endpoint` is set** (the maintainer's decision of 2026-11-11,
    /// #760's review M1); whether it runs an HTTP port decides nothing. With the endpoint set, start is refused by name
    /// when the audience or the client is unset or the endpoint is neither https nor loopback (`validate()`, the same
    /// names here), when the secret's variable is unset or empty, or when the egress allow-list does not permit the
    /// endpoint. Under `gateway_caller_profile = legacy` a gateway frames no envelope, so it exchanges nothing: the
    /// settings are not demanded, and the start says they are inert (#760's review, L6).
    pub(crate) fn load(cfg: &GossipConfig) -> Result<Option<Self>, GossipError> {
        Self::load_with(cfg, |var| std::env::var(var).ok())
    }

    /// [`load`](Self::load), the environment read through `env` (a test names the value it sees).
    pub(crate) fn load_with(cfg: &GossipConfig, env: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GossipError> {
        let Some(oidc) = &cfg.oidc else { return Ok(None) };
        let Some(endpoint) = oidc.exchange_token_endpoint.clone() else { return Ok(None) };
        if cfg.gateway_caller_profile == crate::config::GatewayCallerProfile::Legacy {
            tracing::warn!(
                %endpoint,
                "oidc: exchange_token_endpoint is set under gateway_caller_profile = legacy, which frames no caller \
                 envelope: no token is exchanged, and the exchange settings are inert"
            );
            return Ok(None);
        }
        let Some(audience) = oidc.exchange_audience.clone() else {
            return Err(refused("oidc.exchange_audience", "oidc_exchange_audience_unset: with exchange_token_endpoint set, the audience the gateway asks the IdP for must be named".into()));
        };
        let Some(client_id) = oidc.exchange_client_id.clone().filter(|c| !c.trim().is_empty()) else {
            return Err(refused("oidc.exchange_client_id", "oidc_exchange_client_unset: with exchange_token_endpoint set, the client the gateway exchanges as must be named".into()));
        };
        if !crate::config::exchange_endpoint_is_secure(&endpoint) {
            return Err(refused("oidc.exchange_token_endpoint", format!("oidc_exchange_endpoint_insecure: `{endpoint}` is not https, and its host is not loopback")));
        }
        let var = oidc.exchange_secret_env();
        let secret = env(var).filter(|s| !s.is_empty()).ok_or_else(|| {
            refused("oidc.exchange_client_secret_env", format!("oidc_exchange_secret_unset: the variable {var}, which holds the gateway's client secret at the IdP, is unset or empty"))
        })?;
        if !cfg.egress.permits_url(&endpoint) {
            return Err(refused("oidc.exchange_token_endpoint", format!("egress.allow_hosts does not permit the token endpoint `{endpoint}`: no exchange could reach it — add its host, or unset exchange_token_endpoint")));
        }
        // No redirect is followed (#760's review, M3): the request carries the client secret, the user's token and a
        // DPoP proof for this URL, and a redirect would hand them to a host the operator never named.
        let http = crate::agent::egress_client::build_or_none(crate::agent::egress_client::without_redirects());
        Ok(Some(Self { endpoint, client_id, secret, audience, http, egress: cfg.egress.clone() }))
    }

    /// Exchange `subject`, an OIDC user's own bearer, for a token bound to `key` (this gateway's node key). `Err` names
    /// the refusal the dispatch answers: the IdP unreachable within [`EXCHANGE_TIMEOUT`] or answering a redirect
    /// (`oidc_exchange_unreachable`), refusing the exchange (`oidc_exchange_refused`), answering past
    /// [`EXCHANGE_RESPONSE_MAX_BYTES`] (`oidc_exchange_too_large`), or answering something that is not a token
    /// (`oidc_exchange_malformed`). One `use_dpop_nonce` challenge (RFC 9449 §8) is answered with the nonce it names; a
    /// second is `oidc_exchange_refused`.
    pub(crate) async fn exchange(&self, subject: &str, key: &ed25519_dalek::SigningKey) -> Result<String, &'static str> {
        if !self.egress.permits_url(&self.endpoint) {
            return Err("oidc_exchange_unreachable");
        }
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
            ("subject_token", subject),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("audience", self.audience.as_str()),
        ];
        let htu = dpop_htu(&self.endpoint);
        let mut nonce: Option<String> = None;
        for attempt in 0..2 {
            let iat = mycelium_core::sim_seam::wall_now_ms() / 1_000;
            let jti = mycelium_core::sim_seam::rng_u64_from("oidc-exchange/dpop-jti", 0);
            let proof = dpop_proof(key, &htu, iat, jti, nonce.as_deref());
            let response = self
                .http
                .post(&self.endpoint)
                .header(reqwest::header::AUTHORIZATION, client_basic(&self.client_id, &self.secret))
                .header("DPoP", proof)
                .form(&form)
                .timeout(EXCHANGE_TIMEOUT)
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("oidc: the token exchange did not reach the IdP: {e}");
                    "oidc_exchange_unreachable"
                })?;
            let status = response.status();
            if status.is_redirection() {
                tracing::warn!(%status, "oidc: the IdP answered the token exchange with a redirect, which is not followed");
                return Err("oidc_exchange_unreachable");
            }
            let challenge = response.headers().get("dpop-nonce").and_then(|v| v.to_str().ok()).map(str::to_string);
            let body = crate::agent::egress_client::read_bounded(response, EXCHANGE_RESPONSE_MAX_BYTES).await.ok_or("oidc_exchange_too_large")?;
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            if !status.is_success() {
                if attempt == 0 && body.get("error").and_then(|e| e.as_str()) == Some("use_dpop_nonce") && challenge.is_some() {
                    nonce = challenge;
                    continue;
                }
                tracing::warn!(%status, error = body.get("error").and_then(|e| e.as_str()).unwrap_or_default(), "oidc: the IdP refused the token exchange");
                return Err("oidc_exchange_refused");
            }
            return body.get("access_token").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(str::to_string).ok_or("oidc_exchange_malformed");
        }
        Err("oidc_exchange_refused")
    }
}

/// The client's `Authorization: Basic` value per RFC 6749 §2.3.1: the client id and the secret each
/// `application/x-www-form-urlencoded`, then joined by `:` and base64-encoded (#760's review, L3) — so a secret holding
/// `%`, `+`, `:` or a space reaches the IdP as it is.
pub(crate) fn client_basic(client_id: &str, secret: &str) -> String {
    use base64::Engine as _;
    let enc = |v: &str| url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>();
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", enc(client_id), enc(secret))))
}

/// The `htu` of a DPoP proof for `endpoint`: the URI without its query and fragment (RFC 9449 §4.2, #760's review L3).
pub(crate) fn dpop_htu(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => endpoint.to_string(),
    }
}

/// A DPoP proof (RFC 9449) for a `POST` to `htu`, signed with `key` (EdDSA), issued at `iat` (seconds) under the id
/// `jti`, carrying the server's `nonce` when it asked for one: `{"typ":"dpop+jwt","alg":"EdDSA","jwk":{"kty":"OKP",
/// "crv":"Ed25519","x":…}}` over `{"jti":…,"htm":"POST","htu":…,"iat":…[,"nonce":…]}`. The IdP binds the token it
/// issues to the JWK's RFC 7638 thumbprint.
pub(crate) fn dpop_proof(key: &ed25519_dalek::SigningKey, htu: &str, iat: u64, jti: u64, nonce: Option<&str>) -> String {
    use base64::Engine as _;
    let b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let x = b64u.encode(key.verifying_key().to_bytes());
    let header = serde_json::json!({ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": { "kty": "OKP", "crv": "Ed25519", "x": x } });
    let mut payload = serde_json::json!({ "jti": format!("{jti:016x}"), "htm": "POST", "htu": htu, "iat": iat });
    if let Some(n) = nonce {
        payload["nonce"] = n.into();
    }
    let signing_input = format!("{}.{}", b64u.encode(header.to_string()), b64u.encode(payload.to_string()));
    let sig = crate::tls::sign_bytes(key, signing_input.as_bytes());
    format!("{signing_input}.{}", b64u.encode(sig))
}
