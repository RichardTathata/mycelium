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
///
/// `#[non_exhaustive]` as of the release that added `Tls`. A new refusal here is a refusal that already existed and was
/// being reported as something less precise — `Tls` was exactly that — so the honest expectation is
/// that more will arrive, and a `_` arm now means the next one is not a breaking change. The arm
/// should **fail closed**: an unrecognised refusal is *the call did not happen for a reason this
/// code does not know*, never *the call is fine*.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The transport was refused before any request crossed it (item 2 row 11).
    ///
    /// **Not a [`CallOutcome::DeliveryUnknown`].** That distinction is the reason this variant
    /// exists rather than reusing the silent-gateway path: `DeliveryUnknown` means *it may have
    /// run*, which for a non-repeatable call is a permanent stain — the caller may never retry. A
    /// TLS refusal means the handshake did not complete, so the partner received no request and the
    /// call is safe to retry once the endpoint or the pin is fixed.
    Tls(TlsRefusal),
}

/// Why the transport was refused before it carried anything. See [`ClientError::Tls`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsRefusal {
    /// The endpoint completed no handshake this client would accept: it presented a certificate
    /// whose `SubjectPublicKeyInfo` is not pinned for this partner.
    ///
    /// Either the partner rotated its TLS key without publishing the new pin, or the endpoint is
    /// not the partner. Both are an operator's decision, not something to retry around.
    /// `presented` is the digest that arrived, lower-case hex, so it can be compared against a
    /// bundle by eye — it is *not* a value to paste in without asking the partner first.
    PinMismatch { gateway: String, presented: Option<String> },
    /// Pins are configured and this endpoint's URL is not `https://`, so nothing would be encrypted
    /// and no certificate would be presented to check. Refused with no connection attempted.
    PlaintextEndpoint { gateway: String, base_url: String },
}

impl std::fmt::Display for TlsRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PinMismatch { gateway, presented } => match presented {
                Some(p) => write!(f, "gateway {gateway} presented an unpinned key (spki sha256 {p})"),
                None => write!(f, "gateway {gateway} presented an unpinned key"),
            },
            Self::PlaintextEndpoint { gateway, base_url } => {
                write!(f, "gateway {gateway} is pinned but its endpoint {base_url} is not https")
            }
        }
    }
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
            Self::Tls(r) => write!(f, "tls: {r}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Find a pin mismatch inside a transport error, if that is what it was.
///
/// The refusal is raised deep inside rustls and arrives wrapped by hyper and reqwest, so it is
/// recovered by **walking the error graph and downcasting** rather than by matching on a message —
/// a string match would be a gate that silently stops working the next time any layer rewords
/// itself. Returning `None` costs only precision: the caller then reports the failure as a silent
/// gateway, which is the conservative reading.
///
/// `source()` alone is not enough, and that is not a guess — it is what the wrapping actually does:
///
/// ```text
/// reqwest::Error → hyper_util Error → io::Error(Custom) → io::Error(Custom) → rustls::Error
///                                     ^ source() stops here
/// ```
///
/// `io::Error` does not report a custom payload through `source()`; it exposes it through
/// `get_ref()`, and here there are **two** such layers nested. So both edges are followed. The
/// depth bound is not defending against anything in particular — the chain is four deep — it is
/// there so a future wrapper that made the graph cyclic could not hang a caller's request path.
fn pin_mismatch(err: &(dyn std::error::Error + 'static)) -> Option<crate::federation::pinning::PinMismatch> {
    use crate::federation::pinning::PinMismatch;
    let mut pending: Vec<&(dyn std::error::Error + 'static)> = vec![err];
    for _ in 0..16 {
        let Some(e) = pending.pop() else { break };
        if let Some(m) = e.downcast_ref::<PinMismatch>() {
            return Some(m.clone());
        }
        // rustls does not expose the wrapped error through `source()` either, so the one variant
        // that can carry ours is opened by hand.
        if let Some(rustls::Error::InvalidCertificate(rustls::CertificateError::Other(other))) =
            e.downcast_ref::<rustls::Error>()
        {
            let inner: &(dyn std::error::Error + 'static) = &*other.0;
            if let Some(m) = inner.downcast_ref::<PinMismatch>() {
                return Some(m.clone());
            }
        }
        if let Some(inner) = e.downcast_ref::<std::io::Error>().and_then(|io| io.get_ref()) {
            pending.push(inner);
        }
        if let Some(src) = e.source() {
            pending.push(src);
        }
    }
    None
}

struct ClientState {
    link: PartnerLink,
    pool: GatewayPool,
    resolver: RemoteResolver,
    last_catalogue: Option<Vec<String>>,
    /// The highest policy revision accepted from this partner. `DomainPolicy::revision` is
    /// documented as monotonic; this is what makes that rule true of a *consumer* rather than only
    /// of the producer. See [`CatalogRefusal::StaleRevision`].
    highest_revision: Option<u64>,
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
    /// The transport anchor (item 2 row 11). Empty: no pinning, and `http://` endpoints are read by
    /// anyone on the path. Non-empty: only an endpoint holding one of these keys is talked to.
    ///
    /// Held here rather than only inside `http` because **two builder methods rebuild the client**,
    /// and a setting that lives only in the built object is a setting the next builder call drops.
    /// `the_builder_order_does_not_decide_whether_tls_is_pinned` is the gate on that.
    tls_pins: Vec<[u8; 32]>,
    connect_timeout: Duration,
    request_timeout: Duration,
    http: reqwest::Client,
    /// Lock-order row 39: leaf, µs, never held across the HTTP await (two critical sections
    /// around it, by construction of `call`).
    state: Mutex<ClientState>,
}

/// How long a single attempt may take before the gateway counts as silent.
///
/// **This is a contract obligation, not a tuning knob.** PR 5 says a silent gateway yields
/// `DeliveryUnknown` — and a client that waits forever cannot deliver that verdict. A partner whose
/// network is *blackholed* rather than refusing (the interface is gone but a default route remains,
/// which is what a real severance looks like) never sends a TCP reset, so without a bound the
/// connect simply hangs. Found by the two-mesh Docker suite (item 2 PR 10b): with the edge network
/// disconnected, calls through a gateway with no pooled connection hung instead of being refused.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The whole-request bound, for a partner that accepts a connection and then stops talking.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn http_client(connect: Duration, request: Duration, tls_pins: &[[u8; 32]]) -> reqwest::Client {
    let builder = reqwest::Client::builder().connect_timeout(connect).timeout(request);
    let builder = if tls_pins.is_empty() {
        builder
    } else {
        builder.use_preconfigured_tls(super::pinning::pinned_client_config(tls_pins.to_vec()))
    };
    builder
        .build()
        // A builder failure here is a TLS-backend problem, not a per-call condition; the default
        // client is still better than refusing to construct the whole client.
        //
        // **Except when pins were asked for.** Falling back to a default client would silently
        // discard the pinning and dial the partner with the ordinary Web-PKI verifier — the one
        // shape of failure this whole module exists to refuse. A client that cannot honour its pins
        // must not be a client that quietly does not.
        .unwrap_or_else(|e| {
            assert!(tls_pins.is_empty(), "a pinned TLS client could not be built, and falling back would drop the pinning: {e}");
            reqwest::Client::new()
        })
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
            tls_pins: Vec::new(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            http: http_client(DEFAULT_CONNECT_TIMEOUT, DEFAULT_REQUEST_TIMEOUT, &[]),
            state: Mutex::new(ClientState {
                link: PartnerLink::new(partner),
                pool,
                resolver: RemoteResolver::new(freshness),
                last_catalogue: None,
                highest_revision: None,
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

    /// Bound how long one attempt may take before the gateway counts as silent (defaults: 5 s to
    /// connect, 30 s in total). See [`DEFAULT_CONNECT_TIMEOUT`] for why this is a contract
    /// obligation rather than a tuning knob: a blackholed partner never refuses, and an unbounded
    /// wait cannot produce the `DeliveryUnknown` the contract promises.
    pub fn with_timeouts(mut self, connect: Duration, request: Duration) -> Self {
        self.connect_timeout = connect;
        self.request_timeout = request;
        self.rebuild_http();
        self
    }

    /// Dial this partner over TLS terminated by a key the trust bundle pins (item 2 row 11).
    ///
    /// ```ignore
    /// let client = FederationClient::new(..).with_tls_pins(bundle.tls_pins_for(&partner).to_vec());
    /// ```
    ///
    /// With pins set, the partner's certificate is trusted **only** if its `SubjectPublicKeyInfo`
    /// hashes to one of them — no certificate authority is consulted, because this design has none
    /// to consult. See [`crate::federation::pinning`] for why the bundle is the anchor, what a
    /// completed handshake does and does not prove, and how to compute the value.
    ///
    /// An empty list is *no pinning*, and leaves the client exactly as it was. A **non-empty** list
    /// makes an `http://` endpoint a refusal rather than a plaintext call: see
    /// [`TlsRefusal::PlaintextEndpoint`].
    pub fn with_tls_pins(mut self, pins: Vec<[u8; 32]>) -> Self {
        self.tls_pins = pins;
        self.rebuild_http();
        self
    }

    /// Rebuild the HTTP client from every setting that shapes it.
    ///
    /// One function, called by both setters, because the alternative — each setter building from
    /// its own arguments — made the result depend on the order they were called in, and dropped
    /// whichever setting was applied first.
    fn rebuild_http(&mut self) {
        self.http = http_client(self.connect_timeout, self.request_timeout, &self.tls_pins);
    }

    /// The first endpoint that pinning cannot protect: pins are configured and the URL is
    /// plaintext, so the transport would carry the call in the clear and no certificate would ever
    /// be presented to check.
    ///
    /// Refused rather than downgraded. An operator who configured a pin has stated what they want;
    /// silently dialling `http://` would give them the ceremony of pinning and none of the
    /// confidentiality, which is worse than not offering it.
    fn insecure_endpoint(&self) -> Option<&GatewayEndpoint> {
        if self.tls_pins.is_empty() {
            return None;
        }
        self.endpoints.iter().find(|e| !e.base_url.starts_with("https://"))
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

    /// Mint a credential for `export`, binding `body` when there is one.
    ///
    /// `None` is for a request that carries no payload — the catalogue `GET`. There is nothing to
    /// bind there, and the reply is separately signed, so the field is honestly absent rather than
    /// bound to an empty string that would look like a guarantee.
    fn present(&self, export: &str, body: Option<&[u8]>) -> PresentedCall {
        let now = now_ms();
        PresentedCall::sign(
            &FederatedCaller {
                origin_domain: self.origin.clone(),
                principal: self.principal.clone(),
                export: export.to_string(),
                issued_at_ms: now,
                expires_at_ms: now + self.credential_lifetime.as_millis() as u64,
                body_sha256: body.map(FederatedCaller::digest_of),
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
        if let Some(ep) = self.insecure_endpoint() {
            return Err(ClientError::Tls(TlsRefusal::PlaintextEndpoint {
                gateway: ep.id.clone(),
                base_url: ep.base_url.clone(),
            }));
        }
        let presented = self.present(CATALOG_EXPORT, None);
        let mut last: Option<ClientError> = None;
        for gw in &self.endpoints {
            let url = format!("{}{}", gw.base_url, CATALOG_PATH);
            let sent = self.http.get(&url).header(HEADER_FEDERATED_CALL, presented.to_header_value()).send().await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) if pin_mismatch(&e).is_some() => {
                    // Not tried elsewhere and not reported as silence: an endpoint presenting a key
                    // this domain does not pin is either the partner having rotated without saying
                    // so, or not the partner. Moving quietly to the next gateway would turn an
                    // authentication failure into a latency blip in a log nobody reads.
                    self.state.lock().unwrap_or_else(|e| e.into_inner()).link.disconnected();
                    return Err(ClientError::Tls(TlsRefusal::PinMismatch {
                        gateway: gw.id.clone(),
                        presented: pin_mismatch(&e).map(|m| crate::federation::pinning::hex32(&m.presented)),
                    }));
                }
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

            // A signed reply carries no expiry and no nonce, so an old one still verifies. The
            // revision is what orders them, and the rule is the consumer's to keep: never accept a
            // lower one than it has already accepted. Checked *after* the signature and the domain,
            // so a replay is reported as a replay rather than as whichever check came first.
            if let Some(seen) = s.highest_revision
                && reply.policy_revision < seen
            {
                s.link.disconnected();
                return Err(ClientError::Catalogue(CatalogRefusal::StaleRevision {
                    seen,
                    offered: reply.policy_revision,
                }));
            }
            s.highest_revision = Some(reply.policy_revision.max(s.highest_revision.unwrap_or(0)));

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
        if let Some(ep) = self.insecure_endpoint() {
            return Err(ClientError::Tls(TlsRefusal::PlaintextEndpoint {
                gateway: ep.id.clone(),
                base_url: ep.base_url.clone(),
            }));
        }
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
            let body = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                "params": {
                    "skillId": export,
                    "message": { "role": "user", "parts": [{ "type": "text", "text": text }] },
                },
            });
            // **Serialised once.** The digest has to be over the bytes that actually go on the
            // wire, so the body is encoded here and those same bytes are both signed over and
            // sent. Handing the `Value` to `.json()` would re-encode it, and the receiver would
            // then be comparing our digest against a different serialisation of the same JSON —
            // which is how a binding becomes a source of false refusals instead of a guarantee.
            let body_bytes = serde_json::to_vec(&body).expect("a request we constructed serialises");
            let presented = self.present(export, Some(&body_bytes));
            let sent = self
                .http
                .post(format!("{}/a2a", ep.base_url))
                .header(HEADER_FEDERATED_CALL, presented.to_header_value())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body_bytes)
                .send()
                .await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) if pin_mismatch(&e).is_some() => {
                    // The slot comes back — the gateway is not busy, it is not the partner — and the
                    // call ends here rather than becoming `DeliveryUnknown`, because the handshake
                    // failed: nothing was sent, so a non-repeatable call stays retryable.
                    self.state.lock().unwrap_or_else(|e| e.into_inner()).pool.release(&gateway, &self.partner);
                    return Err(ClientError::Tls(TlsRefusal::PinMismatch {
                        gateway: gateway.clone(),
                        presented: pin_mismatch(&e).map(|m| crate::federation::pinning::hex32(&m.presented)),
                    }));
                }
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
