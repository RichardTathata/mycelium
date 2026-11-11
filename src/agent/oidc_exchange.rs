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

impl OidcExchanger {
    /// The exchanger a node's configuration asks for, built at `start()`. `Ok(None)` when this node does not exchange:
    /// no `[oidc]`, no `exchange_audience`, or no HTTP gateway (a provider only verifies). A gateway with
    /// `exchange_audience` refuses to start, by name, when its token endpoint, its client id or its client secret is
    /// missing, or when its egress allow-list does not permit the token endpoint.
    pub(crate) fn load(cfg: &GossipConfig) -> Result<Option<Self>, GossipError> {
        Self::load_with(cfg, |var| std::env::var(var).ok())
    }

    /// [`load`](Self::load), the environment read through `env` (a test names the value it sees).
    pub(crate) fn load_with(cfg: &GossipConfig, env: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GossipError> {
        let Some(oidc) = &cfg.oidc else { return Ok(None) };
        let Some(audience) = oidc.exchange_audience.clone() else { return Ok(None) };
        if cfg.http_port.is_none() {
            return Ok(None);
        }
        let Some(endpoint) = oidc.exchange_token_endpoint.clone().filter(|e| !e.trim().is_empty()) else {
            return Err(refused("oidc.exchange_token_endpoint", "oidc_exchange_endpoint_unset: a gateway with exchange_audience exchanges at the IdP's token endpoint, which must be named".into()));
        };
        let Some(client_id) = oidc.exchange_client_id.clone().filter(|c| !c.trim().is_empty()) else {
            return Err(refused("oidc.exchange_client_id", "oidc_exchange_client_unset: a gateway with exchange_audience exchanges as a client of the IdP, which must be named".into()));
        };
        let var = oidc.exchange_secret_env();
        let secret = env(var).filter(|s| !s.is_empty()).ok_or_else(|| {
            refused("oidc.exchange_client_secret_env", format!("oidc_exchange_secret_unset: the variable {var}, which holds the gateway's client secret at the IdP, is unset or empty"))
        })?;
        if !cfg.egress.permits_url(&endpoint) {
            return Err(refused("oidc.exchange_token_endpoint", format!("egress.allow_hosts does not permit the token endpoint `{endpoint}`: no exchange could reach it — add its host, or unset exchange_audience")));
        }
        let http = crate::agent::egress_client::build_or_none(crate::agent::egress_client::with_policy(&cfg.egress));
        Ok(Some(Self { endpoint, client_id, secret, audience, http, egress: cfg.egress.clone() }))
    }

    /// Exchange `subject`, an OIDC user's own bearer, for a token bound to `key` (this gateway's node key). `Err` names
    /// the refusal the dispatch answers: the IdP unreachable within [`EXCHANGE_TIMEOUT`], refusing the exchange, or
    /// answering something that is not a token.
    pub(crate) async fn exchange(&self, subject: &str, key: &ed25519_dalek::SigningKey) -> Result<String, &'static str> {
        if !self.egress.permits_url(&self.endpoint) {
            return Err("oidc_exchange_unreachable");
        }
        let iat = mycelium_core::sim_seam::wall_now_ms() / 1_000;
        let jti = mycelium_core::sim_seam::rng_u64_from("oidc-exchange/dpop-jti", 0);
        let proof = dpop_proof(key, &self.endpoint, iat, jti);
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
            ("subject_token", subject),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("audience", self.audience.as_str()),
        ];
        let response = self
            .http
            .post(&self.endpoint)
            .basic_auth(&self.client_id, Some(&self.secret))
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
        if !status.is_success() {
            tracing::warn!(%status, "oidc: the IdP refused the token exchange");
            return Err("oidc_exchange_refused");
        }
        let body: serde_json::Value = response.json().await.map_err(|_| "oidc_exchange_malformed")?;
        body.get("access_token").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(str::to_string).ok_or("oidc_exchange_malformed")
    }
}

/// A DPoP proof (RFC 9449) for a `POST` to `htu`, signed with `key` (EdDSA), issued at `iat` (seconds) under the id
/// `jti`: `{"typ":"dpop+jwt","alg":"EdDSA","jwk":{"kty":"OKP","crv":"Ed25519","x":…}}` over
/// `{"jti":…,"htm":"POST","htu":…,"iat":…}`. The IdP binds the token it issues to the JWK's RFC 7638 thumbprint.
pub(crate) fn dpop_proof(key: &ed25519_dalek::SigningKey, htu: &str, iat: u64, jti: u64) -> String {
    use base64::Engine as _;
    let b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let x = b64u.encode(key.verifying_key().to_bytes());
    let header = serde_json::json!({ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": { "kty": "OKP", "crv": "Ed25519", "x": x } });
    let payload = serde_json::json!({ "jti": format!("{jti:016x}"), "htm": "POST", "htu": htu, "iat": iat });
    let signing_input = format!("{}.{}", b64u.encode(header.to_string()), b64u.encode(payload.to_string()));
    let sig = crate::tls::sign_bytes(key, signing_input.as_bytes());
    format!("{signing_input}.{}", b64u.encode(sig))
}
