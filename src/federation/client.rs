//! The federation client — the transport's consumer side (item 2 PR 8).
//!
//! A [`FederationClient`] is one domain's view of one partner: the link state PR 6 defined
//! ([`PartnerLink`]), the gateway slots PR 5 defined ([`GatewayPool`]), and the catalogue
//! observations PR 3 defined ([`RemoteResolver`]), driven by real HTTP. It makes no decision those
//! modules do not already make; it sequences them, and it is the place where a transport failure
//! becomes one of their outcomes rather than an error string.
//!
//! # The sequence a call goes through
//!
//! 1. `link.admit()` — the link is `Ready`, not `Down`, not `Refreshing`, not revoked. Fails with
//!    no HTTP.
//! 2. `resolver.resolve(partner, export)` — a fresh observation says the partner exports it to us.
//!    Fails with no HTTP.
//! 3. `pool.admit(partner)` — a gateway with a free slot for this partner. Fails with no HTTP.
//! 4. The HTTP call, with a credential minted for *this export* and *this lifetime*.
//! 5. The slot is released; a silent gateway goes through [`on_gateway_silent`], which is where
//!    repeatability decides whether another gateway may be tried.
//!
//! Steps 1–3 and step 5 each hold the client's one lock for microseconds; step 4 holds nothing.
//! The lock is never held across the await.
//!
//! # What "connected" means
//!
//! [`FederationClient::connect`] fetches the catalogue. On success the link goes `Refreshing` →
//! `Ready` in one step, because the fetch *is* the refresh; on a transport failure it goes `Down`.
//! Reconnected is not ready: a client that lost its link must `connect` again before a call is
//! admitted, and the catalogue it gets may differ from the one it had.

use super::call::FederatedCaller;
use super::catalog::{CatalogObservation, RemoteResolver, ResolveFailure};
use super::edge::{now_ms, CatalogRefusal, CatalogReply, PresentedCall, CATALOG_EXPORT, CATALOG_PATH, HEADER_FEDERATED_CALL};
use super::gateway::{on_gateway_silent, CallOutcome, GatewayPool, Repeatability};
use super::session::{LinkRefusal, LinkState, PartnerLink};
use super::DomainId;
use std::sync::Mutex;
use std::time::Duration;

/// One of the partner's gateways: the id the pool meters slots by, and where it answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    pub id: String,
    /// `http://host:port` or `https://host:port`, no trailing slash.
    pub base_url: String,
}

/// Why a federated call did not complete. Every variant is one the contract already names;
/// `Transport` is the one this module adds, for a reply that was received but could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// Refused locally, before any byte was sent: the link is not ready.
    Link(LinkRefusal),
    /// Refused locally, before any byte was sent: the export is not in a fresh catalogue.
    Resolve(ResolveFailure),
    /// A pool or delivery outcome: no capacity, or delivery unknown after a silent gateway.
    Outcome(CallOutcome),
    /// The partner's gateway answered and refused: the auth layer's status, or the JSON-RPC error.
    Refused { status: u16, code: Option<i64>, message: String },
    /// A reply arrived but was not the shape of an A2A task.
    Transport(String),
    /// The catalogue arrived but is not one this client will rely on: unsigned when a signature is
    /// required, forged, for another domain, or filtered for someone else (item 2 PR 10a).
    Catalogue(CatalogRefusal),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Link(r) => write!(f, "link: {r:?}"),
            Self::Resolve(r) => write!(f, "resolve: {r:?}"),
            Self::Outcome(o) => write!(f, "outcome: {o:?}"),
            Self::Refused { status, code, message } => write!(f, "refused ({status}, {code:?}): {message}"),
            Self::Transport(s) => write!(f, "transport: {s}"),
            Self::Catalogue(r) => write!(f, "catalogue: {r}"),
        }
    }
}

impl std::error::Error for ClientError {}

struct ClientState {
    link: PartnerLink,
    pool: GatewayPool,
    resolver: RemoteResolver,
    last_catalogue: Option<Vec<String>>,
}

/// One domain's client for one partner. See the module docs for the sequence.
pub struct FederationClient {
    origin: DomainId,
    principal: String,
    signing_key: ed25519_dalek::SigningKey,
    partner: DomainId,
    /// The partner's public key (its descriptor's `public_key`). Present: a catalogue is relied
    /// on only if it verifies under it; absent: attributed to the gateway asked, as PR 3's
    /// *observation*, and nothing more.
    partner_key: Option<[u8; 32]>,
    endpoints: Vec<GatewayEndpoint>,
    credential_lifetime: Duration,
    http: reqwest::Client,
    /// Lock-order row 39: leaf, µs, never held across the HTTP await (two critical sections
    /// around it, by construction of `call`).
    state: Mutex<ClientState>,
}

impl FederationClient {
    /// `slots_per_partner` is the fixed per-gateway quota (PR 5); `freshness` is how long a
    /// catalogue observation may be relied on (PR 3).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: DomainId,
        principal: impl Into<String>,
        signing_key: ed25519_dalek::SigningKey,
        partner: DomainId,
        endpoints: Vec<GatewayEndpoint>,
        slots_per_partner: usize,
        freshness: Duration,
    ) -> Self {
        let pool = GatewayPool::new(endpoints.iter().map(|e| e.id.clone()), slots_per_partner);
        Self {
            origin,
            principal: principal.into(),
            signing_key,
            partner: partner.clone(),
            partner_key: None,
            endpoints,
            credential_lifetime: Duration::from_secs(60),
            http: reqwest::Client::new(),
            state: Mutex::new(ClientState {
                link: PartnerLink::new(partner),
                pool,
                resolver: RemoteResolver::new(freshness),
                last_catalogue: None,
            }),
        }
    }

    /// How long a minted credential is valid for. Must be within the partner's
    /// `CallPolicy::max_lifetime` or every call is `LifetimeTooLong`.
    pub fn with_credential_lifetime(mut self, lifetime: Duration) -> Self {
        self.credential_lifetime = lifetime;
        self
    }

    /// Require every catalogue to be signed under the partner's key (item 2 PR 10a). An unsigned,
    /// forged, misaddressed or wrong-domain catalogue is refused and leaves the link `Down`.
    pub fn with_partner_key(mut self, key: [u8; 32]) -> Self {
        self.partner_key = Some(key);
        self
    }

    pub fn partner(&self) -> &DomainId {
        &self.partner
    }

    pub fn link_state(&self) -> LinkState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).link.state()
    }

    /// The exports the last successful `connect` was granted, or `None` before one (and after a
    /// `disconnect`, which forgets discovery). A remembered list, not a fresh one: whether an
    /// export may be *called* is the resolver's freshness rule at call time, not this.
    pub fn last_catalogue(&self) -> Option<Vec<String>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_catalogue.clone()
    }

    fn present(&self, export: &str) -> PresentedCall {
        let now = now_ms();
        PresentedCall::sign(
            &FederatedCaller {
                origin_domain: self.origin.clone(),
                principal: self.principal.clone(),
                export: export.to_string(),
                issued_at_ms: now,
                expires_at_ms: now + self.credential_lifetime.as_millis() as u64,
            },
            &self.signing_key,
        )
    }

    fn endpoint(&self, id: &str) -> Option<&GatewayEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    /// Fetch the catalogue from the first gateway that answers, and bring the link to `Ready`.
    /// Returns the exports this domain has been granted. A transport failure on every gateway
    /// leaves the link `Down`; a refusal (revoked, untrusted, expired) leaves it `Down` too and
    /// says why.
    pub async fn connect(&self) -> Result<Vec<String>, ClientError> {
        let presented = self.present(CATALOG_EXPORT);
        let mut last: Option<ClientError> = None;
        for gw in &self.endpoints {
            let url = format!("{}{}", gw.base_url, CATALOG_PATH);
            let sent = self.http.get(&url).header(HEADER_FEDERATED_CALL, presented.to_header_value()).send().await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) => {
                    last = Some(ClientError::Outcome(CallOutcome::DeliveryUnknown {
                        attempted_via: vec![gw.id.clone()],
                        reason: e.to_string(),
                    }));
                    continue;
                }
            };
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| ClientError::Transport(e.to_string()))?;
            if status != 200 {
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
                    .unwrap_or(body);
                let err = ClientError::Refused { status, code: None, message };
                self.state.lock().unwrap_or_else(|e| e.into_inner()).link.disconnected();
                return Err(err);
            }
            let reply: CatalogReply =
                serde_json::from_str(&body).map_err(|e| ClientError::Transport(format!("catalogue reply: {e}")))?;
            if let Some(key) = &self.partner_key
                && let Err(refusal) = reply.verify(&self.partner, &self.origin, key)
            {
                self.state.lock().unwrap_or_else(|e| e.into_inner()).link.disconnected();
                return Err(ClientError::Catalogue(refusal));
            }
            if reply.domain != self.partner {
                let err = ClientError::Transport(format!(
                    "gateway {} answered for domain {}, expected {}",
                    gw.id, reply.domain, self.partner
                ));
                self.state.lock().unwrap_or_else(|e| e.into_inner()).link.disconnected();
                return Err(err);
            }
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.link.connected();
            s.resolver.observe(CatalogObservation {
                partner: self.partner.clone(),
                observed_by: gw.id.clone(),
                // A stored instant, constructed by the seam; its age is only ever read through
                // `mono_elapsed` in the resolver.
                observed_at: mycelium_core::sim_seam::mono_instant(),
                exports: reply.exports.clone(),
            });
            s.link.discovery_refreshed();
            s.last_catalogue = Some(reply.exports.clone());
            return Ok(reply.exports);
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).link.disconnected();
        Err(last.unwrap_or(ClientError::Transport("no gateway configured".to_string())))
    }

    /// Mark the link down: discovery is forgotten, and the next call is refused with no HTTP until
    /// `connect` succeeds again.
    pub fn disconnect(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.link.disconnected();
        s.resolver.forget(&self.partner);
        s.last_catalogue = None;
    }

    /// Retire one of the partner's gateways (it was replaced, or is known dead): it is never
    /// admitted again. Its endpoint stays configured so an in-flight release for it is harmless.
    /// Returns whether it was in the pool.
    pub fn retire_gateway(&self, gateway_id: &str) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pool.retire(gateway_id)
    }

    /// Revoke the partner on this side: no call is ever admitted again, whatever it answers.
    pub fn revoke(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.link.revoke();
        s.resolver.forget(&self.partner);
    }

    /// Invoke `export` on the partner with `text` as the A2A message, and return the task's first
    /// text artifact. See the module docs for the sequence and where each refusal comes from.
    pub async fn call(&self, export: &str, text: &str, repeatability: Repeatability) -> Result<String, ClientError> {
        // Critical section 1: admit, resolve, take a slot. No HTTP has happened if this fails.
        let first = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.link.admit().map_err(ClientError::Link)?;
            s.resolver.resolve(&self.partner, export).map_err(ClientError::Resolve)?;
            s.pool.admit(&self.partner, &[]).map_err(ClientError::Outcome)?
        };
        let mut attempted = vec![first.clone()];
        let mut gateway = first;
        loop {
            let Some(ep) = self.endpoint(&gateway) else {
                // A pool id with no endpoint is a configuration fault, treated as a silent gateway.
                match self.silent(repeatability, &mut attempted, "gateway id has no endpoint") {
                    Ok(next) => { gateway = next; continue; }
                    Err(e) => return Err(e),
                }
            };
            let presented = self.present(export);
            let body = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                "params": {
                    "skillId": export,
                    "message": { "role": "user", "parts": [{ "type": "text", "text": text }] },
                },
            });
            let sent = self
                .http
                .post(format!("{}/a2a", ep.base_url))
                .header(HEADER_FEDERATED_CALL, presented.to_header_value())
                .json(&body)
                .send()
                .await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) => match self.silent(repeatability, &mut attempted, &e.to_string()) {
                    Ok(next) => { gateway = next; continue; }
                    Err(err) => return Err(err),
                },
            };
            // Critical section 2: the slot comes back whatever the answer was.
            self.state.lock().unwrap_or_else(|e| e.into_inner()).pool.release(&gateway, &self.partner);
            let status = resp.status().as_u16();
            let value: serde_json::Value = match resp.text().await {
                Ok(t) => serde_json::from_str(&t).unwrap_or(serde_json::Value::String(t)),
                Err(e) => return Err(ClientError::Transport(e.to_string())),
            };
            if status != 200 {
                let message = value.get("error").and_then(|e| e.as_str()).map(str::to_owned).unwrap_or_else(|| value.to_string());
                return Err(ClientError::Refused { status, code: None, message });
            }
            if let Some(err) = value.get("error") {
                return Err(ClientError::Refused {
                    status,
                    code: err.get("code").and_then(|c| c.as_i64()),
                    message: err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                });
            }
            return value["result"]["artifacts"][0]["parts"][0]["text"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ClientError::Transport(format!("reply is not an A2A task: {value}")));
        }
    }

    fn silent(&self, repeatability: Repeatability, attempted: &mut Vec<String>, reason: &str) -> Result<String, ClientError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        on_gateway_silent(&mut s.pool, &self.partner, repeatability, attempted, reason).map_err(ClientError::Outcome)
    }
}
