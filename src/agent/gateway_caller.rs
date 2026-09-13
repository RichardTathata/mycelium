//! Gateway caller identity inside a domain — v3 contracts axis **item 7**
//! (`docs/plans/v3-contracts-axis.md` §6.4, decision D27).
//!
//! **The gap.** Every gateway-originated dispatch — `POST /mcp` `tools/call`, `POST /a2a`,
//! `/gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable`, `/gateway/llm/*` —
//! ran under the **node's** identity: the RPC frame's `sender` was the gateway node, so a
//! provider's `authorized_callers` saw the node and never the HTTP client. A confused deputy,
//! not specific to MCP (the 2026-09-05 `/mcp` fix put the route behind auth; it did not tell the
//! provider who called).
//!
//! **The contract.** On every gateway dispatch the auth layer constructs a [`GatewayCaller`] —
//! three things a provider can verify:
//! 1. the **originating principal** (the resolved bearer / scoped-token / OIDC principal — never
//!    a client-supplied string; `anonymous` on an open gateway);
//! 2. the **gateway acting on its behalf** (this node, `via`, bound in and checked against the
//!    frame's signature-verified `sender`);
//! 3. the **authority granted for this request** (the scopes the credential holds, intersected
//!    with what the route needs — never more than the credential holds).
//!
//! The context is **constructed only by the auth layer** (`#[non_exhaustive]`, `pub(crate)`
//! builders) and **attested by the node over the request digest**: under the `tls` identity
//! the envelope carries an Ed25519 signature by the gateway's identity key over
//! `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)`. A struct with a principal name and
//! a digest supplied by anyone else is not evidence — the provider verifies the signer against
//! the keys it knows for `via` (`sys/identity`, anchors, minus revocations) and rejects a
//! mismatch, an unknown signer, an unsigned envelope on an authenticated mesh, or a `via` that is
//! not the frame's sender. **Any rejection denies; nothing ever falls back to the node.**
//!
//! **Where it rides.** Inside the RPC payload, after the 8-byte nonce — the wire (v12) is
//! unchanged: `[0x00 'G' 'W' 'C' 0x01] ‖ u16be len ‖ envelope JSON ‖ application payload`.
//! [`RpcRequest::payload`](super::rpc::RpcRequest::payload) strips it, so every existing
//! provider loop on a node running this code sees exactly the application bytes; the context is
//! read through [`GossipAgent::gateway_caller`] / [`GossipAgent::request_principal`].
//!
//! **The older-provider case.** A node that does not strip the envelope would hand the framed
//! bytes to its handler as if they were the application payload. Every node that enforces the
//! context writes `sys/caller-context/{self} = b"1"` at start; a secure-profile gateway
//! dispatches to a provider **only** when that marker is present and answers an explicit error
//! otherwise (`GatewayDispatchError::ProviderWithoutContext`). The `legacy` profile
//! ([`GatewayCallerProfile::Legacy`](crate::config::GatewayCallerProfile)) is the one way to get
//! node-as-caller dispatch back, for a rolling-upgrade window, logged at `warn!`.
//!
//! **Promise strength** (guardrails tier vocabulary). Under `tls`: *HardPrevention* at the
//! provider — a forged or missing attestation is refused where the work happens. Without a `tls`
//! identity the mesh's own sender identity is unauthenticated, so the context is exactly as
//! strong as the frame's `sender`: carried, honoured by `authorized_callers`, but
//! *SelfImposedPrevention* — the envelope says [`CallerAttestation::UnauthenticatedMesh`].
//!
//! **Five-part statement.** *Guarantee:* a provider on a `tls` mesh authorises a gateway-
//! originated call as the resolved client principal, never as the gateway node. *Assumptions:*
//! the gateway node's identity key is the one peers know for it (identity-auth), and the auth
//! layer is the only constructor. *Enforcing component:* [`verify`] on the provider's receive
//! path, reached through `request_principal` / `request_authorized`. *Failure behaviour:* deny
//! with a [`CallerError`]; the secure gateway refuses a provider without the marker. *Detecting
//! test:* `gateway_caller_tests` in `src/agent/http.rs` — the four negative cases and the
//! `authorized_callers` gate.

use crate::node_id::NodeId;
use bytes::Bytes;
#[cfg(any(feature = "gateway", test))]
use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "gateway", feature = "compliance", test))]
use std::sync::Arc;
#[cfg(any(feature = "gateway", test))]
use std::time::Duration;

#[cfg(any(feature = "gateway", test))]
use super::rpc::RpcError;
use super::rpc::RpcRequest;
use super::TaskCtx;

/// Envelope version carried in the frame magic and in `sys/caller-context/{node}`.
pub const CALLER_CONTEXT_VERSION: u8 = 1;
/// The principal an open gateway (no token model configured, or `/a2a` without a bearer)
/// resolves to. Listing it in `authorized_callers` admits unauthenticated gateway clients.
pub const PRINCIPAL_ANONYMOUS: &str = "anonymous";
/// Principal of the legacy single `gateway_auth_token`.
pub const PRINCIPAL_LEGACY_TOKEN: &str = "token:legacy";

/// Frame magic after the RPC nonce: a leading NUL (no JSON or text payload starts with one),
/// then `GWC`, then the version.
const FRAME_MAGIC: [u8; 5] = [0x00, b'G', b'W', b'C', CALLER_CONTEXT_VERSION];
/// Domain separator for the signed message.
#[cfg(feature = "tls")]
const DOMAIN_SEP: &[u8] = b"mycelium:gateway-caller:v1\n";
/// Upper bound on an envelope: a principal, a node id, a few scopes.
const MAX_ENVELOPE_BYTES: usize = 8 * 1024;

// ── Public types ─────────────────────────────────────────────────────────────

/// Who a gateway request came from, as the provider sees it after verification.
///
/// Constructed only by the gateway's auth layer and read through
/// [`GossipAgent::gateway_caller`](crate::GossipAgent::gateway_caller) /
/// [`GossipAgent::request_principal`](crate::GossipAgent::request_principal).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayCaller {
    /// The originating principal: `oidc:{subject}`, `token:#{index}` (a `gateway_scoped_tokens`
    /// entry by position), [`PRINCIPAL_LEGACY_TOKEN`], or [`PRINCIPAL_ANONYMOUS`]. Never the
    /// credential itself.
    pub principal: String,
    /// The gateway node acting on the principal's behalf — equal to the RPC frame's
    /// signature-verified sender (checked).
    pub via: NodeId,
    /// The authority granted for this request: the credential's scopes intersected with what
    /// the route required. Empty on a route that requires no scope (`/a2a`).
    pub scopes: Vec<String>,
    /// Gateway HLC physical time (ms) when the context was issued.
    pub issued_at_ms: u64,
    /// How the context was attested.
    pub attestation: CallerAttestation,
}

/// How a [`GatewayCaller`] was attested.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallerAttestation {
    /// Ed25519 signature by the gateway node's identity key, verified by this provider against
    /// the keys it knows for `via`. *HardPrevention* strength.
    Signed {
        /// The verifying key that signed.
        signer: [u8; 32],
    },
    /// This node runs without a `tls` identity, so it holds no key to verify against and the
    /// frame's `sender` is itself unauthenticated. The context is as trustworthy as that sender:
    /// carried and honoured, *SelfImposedPrevention* strength.
    UnauthenticatedMesh,
}

/// Why a caller context was refused. **Every variant denies** — a provider never falls back to
/// treating the call as the gateway node's own action.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallerError {
    /// The envelope did not parse (bad JSON, bad version, bad key/signature encoding).
    Malformed(String),
    /// The envelope names a gateway that is not the frame's sender.
    ViaMismatch {
        /// The `via` the envelope claims.
        claimed: String,
        /// The signature-verified sender of the frame.
        sender: NodeId,
    },
    /// This node has a `tls` identity (the mesh is authenticated) and the envelope is unsigned.
    Unsigned,
    /// The signer is not a key this node trusts for `via` (never learned, or revoked).
    UnknownSigner,
    /// The signature does not verify over the request digest.
    BadSignature,
}

impl std::fmt::Display for CallerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallerError::Malformed(why) => write!(f, "caller context malformed: {why}"),
            CallerError::ViaMismatch { claimed, sender } => {
                write!(f, "caller context names gateway {claimed} but the frame came from {sender}")
            }
            CallerError::Unsigned => f.write_str("caller context is unsigned on an authenticated mesh"),
            CallerError::UnknownSigner => f.write_str("caller context signer is not a trusted key for the gateway"),
            CallerError::BadSignature => f.write_str("caller context signature does not verify"),
        }
    }
}

impl std::error::Error for CallerError {}

/// The principal an RPC request should be authorised **as**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestPrincipal {
    /// No gateway context: the sending node acts for itself (a direct `rpc_call` from Rust).
    Node(NodeId),
    /// A gateway dispatched the call for a client; authorise the client, not the gateway.
    Client(GatewayCaller),
}

impl RequestPrincipal {
    /// The string an allowlist entry or an audit record names: the node id, or the client
    /// principal.
    pub fn name(&self) -> String {
        match self {
            RequestPrincipal::Node(n) => n.to_string(),
            RequestPrincipal::Client(c) => c.principal.clone(),
        }
    }

    /// `true` when a gateway client is behind the request.
    pub fn is_client(&self) -> bool {
        matches!(self, RequestPrincipal::Client(_))
    }
}

// ── Auth layer → dispatch ────────────────────────────────────────────────────

/// What the gateway auth layer resolved for one HTTP request. Crate-private on purpose: the
/// only constructors are the auth middleware paths in `http.rs`.
#[cfg(any(feature = "gateway", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedPrincipal {
    pub(crate) principal: String,
    pub(crate) scopes: Vec<String>,
}

#[cfg(any(feature = "gateway", test))]
impl ResolvedPrincipal {
    pub(crate) fn anonymous(scopes: Vec<String>) -> Self {
        Self { principal: PRINCIPAL_ANONYMOUS.to_string(), scopes }
    }
}

/// The authority a request is granted: what the credential holds, intersected with what the
/// route needs. `"*"` in `held` grants exactly the required scope (never `"*"` itself);
/// an empty `required` (a route with no scope) grants nothing.
#[cfg(any(feature = "gateway", test))]
pub(crate) fn granted_scopes(held: &[String], required: Option<&str>) -> Vec<String> {
    match required {
        Some(r) if held.iter().any(|s| s == "*" || s == r) => vec![r.to_string()],
        _ => Vec::new(),
    }
}

/// KV key of a node's caller-context marker.
pub(crate) fn marker_key(node: &NodeId) -> String {
    format!("{}{}", crate::signal::kv_ns::CALLER_CONTEXT, node)
}

/// Does `node` enforce the caller context — i.e. did it publish its marker? This node always
/// does (it is running this code).
#[cfg(any(feature = "gateway", test))]
pub(crate) fn provider_enforces_context(ctx: &TaskCtx, node: &NodeId) -> bool {
    if *node == ctx.node_id {
        return true;
    }
    mycelium_core::ops::kv_get(ctx, &marker_key(node)).is_some()
}

/// Why a gateway dispatch was refused before (or while) reaching the provider.
#[cfg(any(feature = "gateway", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GatewayDispatchError {
    /// The provider replied nothing in time.
    Rpc(RpcError),
    /// Secure profile, but no caller context reached the dispatch site — a handler outside the
    /// auth layer tried to dispatch. Refused rather than run as the node.
    MissingContext,
    /// Secure profile, and the target has no `sys/caller-context/{node}` marker: it would run
    /// the call as this node. Refused; the message names the provider.
    ProviderWithoutContext(NodeId),
}

#[cfg(any(feature = "gateway", test))]
impl GatewayDispatchError {
    /// JSON-RPC error code for the MCP / A2A surfaces.
    pub(crate) fn json_rpc_code(&self) -> i32 {
        match self {
            GatewayDispatchError::Rpc(_) => -32000,
            GatewayDispatchError::MissingContext => -32020,
            GatewayDispatchError::ProviderWithoutContext(_) => -32021,
        }
    }

    /// Short machine-readable reason for JSON bodies and metrics.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            GatewayDispatchError::Rpc(_) => "timeout",
            GatewayDispatchError::MissingContext => "caller_context_missing",
            GatewayDispatchError::ProviderWithoutContext(_) => "provider_without_caller_context",
        }
    }
}

#[cfg(any(feature = "gateway", test))]
impl std::fmt::Display for GatewayDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayDispatchError::Rpc(e) => write!(f, "{e}"),
            GatewayDispatchError::MissingContext => f.write_str(
                "gateway caller context missing: the secure profile refuses to dispatch as the node",
            ),
            GatewayDispatchError::ProviderWithoutContext(n) => write!(
                f,
                "provider {n} does not enforce the gateway caller context (no sys/caller-context marker): \
                 upgrade it, or set gateway_caller_profile = legacy for the rolling-upgrade window"
            ),
        }
    }
}

/// The one dispatch path for every gateway-originated RPC.
///
/// `Secure` (default): requires a resolved principal, refuses a provider without the marker,
/// frames and attests the context, then calls. `Legacy`: node-as-caller, as before item 7.
#[cfg(any(feature = "gateway", test))]
pub(crate) async fn gateway_rpc_call(
    ctx: &TaskCtx,
    caller: Option<&ResolvedPrincipal>,
    target: NodeId,
    kind: Arc<str>,
    payload: Bytes,
    timeout: Duration,
) -> Result<Bytes, GatewayDispatchError> {
    use crate::config::GatewayCallerProfile;
    match ctx.config.gateway_caller_profile {
        GatewayCallerProfile::Legacy => super::rpc::rpc_call_ctx(ctx, target, kind, payload, timeout)
            .await
            .map_err(GatewayDispatchError::Rpc),
        GatewayCallerProfile::Secure => {
            let Some(caller) = caller else {
                refused("caller_context_missing");
                return Err(GatewayDispatchError::MissingContext);
            };
            if !provider_enforces_context(ctx, &target) {
                refused("provider_without_caller_context");
                return Err(GatewayDispatchError::ProviderWithoutContext(target));
            }
            let framed = frame_with_context(ctx, caller, payload);
            super::rpc::rpc_call_ctx(ctx, target, kind, framed, timeout)
                .await
                .map_err(GatewayDispatchError::Rpc)
        }
    }
}

#[cfg(any(feature = "gateway", test))]
#[allow(unused_variables)]
fn refused(reason: &'static str) {
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_caller_refusals_total", "reason" => reason).increment(1);
}

// ── Wire envelope ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u8,
    /// principal
    p: String,
    /// gateway node id
    via: String,
    /// granted scopes
    s: Vec<String>,
    /// issued_at_ms
    t: u64,
    /// base64 signer verifying key (32 bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    k: Option<String>,
    /// base64 Ed25519 signature (64 bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sig: Option<String>,
}

/// Build the framed payload: magic ‖ len ‖ envelope ‖ application payload. Signs under a `tls`
/// identity; otherwise the envelope is unsigned (see [`CallerAttestation::UnauthenticatedMesh`]).
#[cfg(any(feature = "gateway", test))]
pub(crate) fn frame_with_context(ctx: &TaskCtx, caller: &ResolvedPrincipal, app: Bytes) -> Bytes {
    let issued_at_ms = crate::hlc::physical_ms(ctx.hlc.current());
    let via = ctx.node_id.to_string();
    let (k, sig) = attest(ctx, &caller.principal, &via, &caller.scopes, issued_at_ms, &app);
    let env = Envelope {
        v: CALLER_CONTEXT_VERSION,
        p: caller.principal.clone(),
        via,
        s: caller.scopes.clone(),
        t: issued_at_ms,
        k,
        sig,
    };
    let env_bytes = serde_json::to_vec(&env).unwrap_or_default();
    let len = u16::try_from(env_bytes.len()).unwrap_or(u16::MAX);
    let mut buf = BytesMut::with_capacity(FRAME_MAGIC.len() + 2 + env_bytes.len() + app.len());
    buf.put_slice(&FRAME_MAGIC);
    buf.put_u16(len);
    buf.put_slice(&env_bytes[..len as usize]);
    buf.put(app);
    buf.freeze()
}

/// The `(signer, signature)` pair for an envelope: Ed25519 by this node's identity key under a
/// `tls` identity, `(None, None)` otherwise (an unauthenticated mesh has nothing to sign with).
#[cfg(any(feature = "gateway", test))]
#[cfg(feature = "tls")]
fn attest(
    ctx: &TaskCtx,
    principal: &str,
    via: &str,
    scopes: &[String],
    issued_at_ms: u64,
    app: &[u8],
) -> (Option<String>, Option<String>) {
    use base64::Engine as _;
    let Some(tls) = ctx.tls.get() else { return (None, None) };
    let sk = tls.signing_key();
    let signer = sk.verifying_key().to_bytes();
    let msg = signed_message(principal, via, scopes, issued_at_ms, &digest(app));
    let sig = crate::tls::sign_bytes(&sk, &msg);
    (
        Some(base64::engine::general_purpose::STANDARD.encode(signer)),
        Some(base64::engine::general_purpose::STANDARD.encode(sig)),
    )
}

#[cfg(any(feature = "gateway", test))]
#[cfg(not(feature = "tls"))]
fn attest(
    _ctx: &TaskCtx,
    _principal: &str,
    _via: &str,
    _scopes: &[String],
    _issued_at_ms: u64,
    _app: &[u8],
) -> (Option<String>, Option<String>) {
    (None, None)
}

/// Split a nonce-stripped RPC payload into `(envelope, application payload)`. A payload that
/// does not start with the frame magic, or whose declared length does not fit, is returned
/// whole as the application payload with no envelope.
pub(crate) fn split_frame(after_nonce: &Bytes) -> (Option<Bytes>, Bytes) {
    let hdr = FRAME_MAGIC.len() + 2;
    if after_nonce.len() < hdr || after_nonce[..FRAME_MAGIC.len()] != FRAME_MAGIC {
        return (None, after_nonce.clone());
    }
    let len = u16::from_be_bytes([after_nonce[FRAME_MAGIC.len()], after_nonce[FRAME_MAGIC.len() + 1]]) as usize;
    if len > MAX_ENVELOPE_BYTES || after_nonce.len() < hdr + len {
        return (None, after_nonce.clone());
    }
    (Some(after_nonce.slice(hdr..hdr + len)), after_nonce.slice(hdr + len..))
}

#[cfg(feature = "tls")]
fn digest(app: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(app).into()
}

#[cfg(feature = "tls")]
fn signed_message(principal: &str, via: &str, scopes: &[String], issued_at_ms: u64, digest: &[u8; 32]) -> Vec<u8> {
    fn put(buf: &mut Vec<u8>, field: &[u8]) {
        buf.extend_from_slice(&(field.len() as u32).to_le_bytes());
        buf.extend_from_slice(field);
    }
    let mut buf = Vec::with_capacity(96 + principal.len() + via.len());
    buf.extend_from_slice(DOMAIN_SEP);
    put(&mut buf, principal.as_bytes());
    put(&mut buf, via.as_bytes());
    buf.extend_from_slice(&(scopes.len() as u32).to_le_bytes());
    for s in scopes {
        put(&mut buf, s.as_bytes());
    }
    buf.extend_from_slice(&issued_at_ms.to_le_bytes());
    buf.extend_from_slice(digest);
    buf
}

/// Keys this node trusts to have signed for `node`: the retained identity set (`sys/identity`
/// and CA anchors) minus revocations under `compliance`, plus this node's own current key when
/// `node` is self.
#[cfg(feature = "tls")]
fn verifying_keys_for(ctx: &TaskCtx, node: &NodeId) -> Vec<[u8; 32]> {
    #[cfg(feature = "compliance")]
    {
        super::helpers::known_verifying_keys(ctx, node)
    }
    #[cfg(not(feature = "compliance"))]
    {
        let mut keys: Vec<[u8; 32]> = ctx.peer_keys.pin().get(node).cloned().unwrap_or_default();
        if node == &ctx.node_id
            && let Some(t) = ctx.tls.get()
        {
            let cur = t.verifying_key_bytes();
            if !keys.contains(&cur) {
                keys.push(cur);
            }
        }
        keys
    }
}

/// Verify the caller context carried by `req`, if any.
///
/// `Ok(None)`: no context — the sender acts for itself. `Ok(Some(_))`: a verified context.
/// `Err(_)`: a context was present and must be **refused**; never treat the call as the node's.
pub(crate) fn verify(ctx: &TaskCtx, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
    let Some(env_bytes) = req.caller_envelope() else {
        return Ok(None);
    };
    let env: Envelope =
        serde_json::from_slice(&env_bytes).map_err(|e| CallerError::Malformed(e.to_string()))?;
    if env.v != CALLER_CONTEXT_VERSION {
        return Err(CallerError::Malformed(format!("unsupported envelope version {}", env.v)));
    }
    let via: NodeId = env
        .via
        .parse()
        .map_err(|_| CallerError::Malformed(format!("bad via node id '{}'", env.via)))?;
    if via != *req.sender() {
        return Err(CallerError::ViaMismatch { claimed: env.via, sender: req.sender().clone() });
    }

    #[cfg(feature = "tls")]
    let attestation = if ctx.tls.get().is_some() {
        use base64::Engine as _;
        let (Some(k), Some(sig)) = (env.k.as_deref(), env.sig.as_deref()) else {
            return Err(CallerError::Unsigned);
        };
        let signer: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(k)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| CallerError::Malformed("signer key is not 32 bytes".into()))?;
        let sig: [u8; 64] = base64::engine::general_purpose::STANDARD
            .decode(sig)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| CallerError::Malformed("signature is not 64 bytes".into()))?;
        if !verifying_keys_for(ctx, &via).contains(&signer) {
            return Err(CallerError::UnknownSigner);
        }
        let msg = signed_message(&env.p, &env.via, &env.s, env.t, &digest(&req.payload()));
        if !crate::tls::verify_bytes(&signer, &msg, &sig) {
            return Err(CallerError::BadSignature);
        }
        CallerAttestation::Signed { signer }
    } else {
        CallerAttestation::UnauthenticatedMesh
    };
    #[cfg(not(feature = "tls"))]
    let attestation = {
        let _ = ctx;
        CallerAttestation::UnauthenticatedMesh
    };

    Ok(Some(GatewayCaller {
        principal: env.p,
        via,
        scopes: env.s,
        issued_at_ms: env.t,
        attestation,
    }))
}

/// Resolve the principal `req` should be authorised as. `Err` means refuse.
pub(crate) fn request_principal(ctx: &TaskCtx, req: &RpcRequest) -> Result<RequestPrincipal, CallerError> {
    Ok(match verify(ctx, req)? {
        Some(c) => RequestPrincipal::Client(c),
        None => RequestPrincipal::Node(req.sender().clone()),
    })
}

/// The provider-side allowlist decision for a gateway client: an empty allowlist admits
/// everyone; otherwise the client's **principal** must be listed. Listing the gateway *node*
/// does not admit its clients — that is the confused deputy this module closes.
#[cfg(any(feature = "compliance", test))]
pub(crate) fn client_admitted(allow: &[Arc<str>], caller: &GatewayCaller) -> bool {
    allow.is_empty() || allow.iter().any(|entry| entry.as_ref() == caller.principal)
}

// ── GossipAgent surface ──────────────────────────────────────────────────────

impl crate::GossipAgent {
    /// The verified gateway caller context behind `req`, if a gateway dispatched it.
    ///
    /// `Ok(None)` — no context: the sender node acts for itself. `Err` — a context was present
    /// and failed verification: **refuse the call**; never run it as the node.
    pub fn gateway_caller(&self, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
        verify(&self.task_ctx, req)
    }

    /// The principal `req` should be authorised as — the gateway client when a verified
    /// context is present, otherwise the sending node. `Err` means refuse.
    pub fn request_principal(&self, req: &RpcRequest) -> Result<RequestPrincipal, CallerError> {
        request_principal(&self.task_ctx, req)
    }
}

#[cfg(feature = "compliance")]
impl crate::GossipAgent {
    /// Provider-side authorization for a request, **caller-context aware** — the replacement
    /// for [`caller_authorized`](Self::caller_authorized) in any serve loop the gateway can
    /// reach. Empty `allow` admits everyone. A direct node call is admitted if the node is
    /// listed or holds a listed role; a gateway client is admitted only if its *principal* is
    /// listed (the gateway node being listed does not admit its clients). `Err` — the context
    /// failed verification — must be treated as a denial.
    pub fn request_authorized(&self, req: &RpcRequest, allow: &[Arc<str>]) -> Result<bool, CallerError> {
        Ok(match self.request_principal(req)? {
            RequestPrincipal::Node(n) => self.caller_authorized(&n, allow),
            RequestPrincipal::Client(c) => client_admitted(allow, &c),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use bytes::Bytes;

    fn agent(tls: bool) -> Arc<GossipAgent> {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        #[cfg(feature = "tls")]
        if tls {
            let dir = std::env::temp_dir().join(format!("gwcaller-{port}"));
            let _ = std::fs::remove_dir_all(&dir);
            cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir, ..Default::default() });
        }
        #[cfg(not(feature = "tls"))]
        let _ = tls;
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    }

    fn request_from(sender: &NodeId, kind: &str, framed: Bytes) -> RpcRequest {
        let mut buf = BytesMut::with_capacity(8 + framed.len());
        buf.put_u64_le(42);
        buf.put(framed);
        RpcRequest::from(crate::signal::Signal {
            kind: Arc::from(kind),
            scope: crate::signal::SignalScope::Individual(sender.clone()),
            payload: buf.freeze(),
            sender: sender.clone(),
            nonce: 7,
        })
    }

    #[test]
    fn granted_scopes_never_exceed_the_credential() {
        // `*` grants exactly the required scope, never `*`.
        assert_eq!(granted_scopes(&["*".into()], Some("mcp:invoke")), vec!["mcp:invoke".to_string()]);
        // A held scope is granted only when required.
        assert_eq!(
            granted_scopes(&["kv:read".into(), "mcp:invoke".into()], Some("mcp:invoke")),
            vec!["mcp:invoke".to_string()]
        );
        // Not held → nothing (the route would have been refused upstream anyway).
        assert!(granted_scopes(&["kv:read".into()], Some("mcp:invoke")).is_empty());
        // No requirement → nothing granted, whatever is held.
        assert!(granted_scopes(&["*".into()], None).is_empty());
    }

    #[test]
    fn split_frame_leaves_plain_payloads_alone() {
        let plain = Bytes::from_static(b"{\"jsonrpc\":\"2.0\"}");
        let (env, app) = split_frame(&plain);
        assert!(env.is_none());
        assert_eq!(app, plain);
        // Magic with an impossible length is not an envelope either.
        let mut bogus = FRAME_MAGIC.to_vec();
        bogus.extend_from_slice(&[0xff, 0xff, b'x']);
        let bogus = Bytes::from(bogus);
        let (env, app) = split_frame(&bogus);
        assert!(env.is_none());
        assert_eq!(app, bogus);
        let (env, app) = split_frame(&Bytes::new());
        assert!(env.is_none());
        assert!(app.is_empty());
    }

    #[tokio::test]
    async fn context_round_trips_and_payload_is_stripped() {
        let a = agent(false);
        let draft = ResolvedPrincipal { principal: "token:#0".into(), scopes: vec!["mcp:invoke".into()] };
        let app = Bytes::from_static(b"hello");
        let framed = frame_with_context(&a.task_ctx, &draft, app.clone());
        let req = request_from(a.node_id(), "k", framed);
        assert_eq!(req.payload(), app, "the provider sees exactly the application bytes");
        assert!(req.has_caller_context());
        let c = verify(&a.task_ctx, &req).unwrap().expect("a context");
        assert_eq!(c.principal, "token:#0");
        assert_eq!(c.scopes, vec!["mcp:invoke".to_string()]);
        assert_eq!(&c.via, a.node_id());
        assert_eq!(c.attestation, CallerAttestation::UnauthenticatedMesh, "no tls identity ⇒ unsigned");
        match request_principal(&a.task_ctx, &req).unwrap() {
            RequestPrincipal::Client(c2) => assert_eq!(c2, c),
            other => panic!("expected a client principal, got {other:?}"),
        }
        // No envelope ⇒ the node itself.
        let bare = request_from(a.node_id(), "k", app.clone());
        assert!(!bare.has_caller_context());
        assert_eq!(request_principal(&a.task_ctx, &bare).unwrap(), RequestPrincipal::Node(a.node_id().clone()));
    }

    #[tokio::test]
    async fn via_must_be_the_frame_sender() {
        let a = agent(false);
        let draft = ResolvedPrincipal::anonymous(vec![]);
        let framed = frame_with_context(&a.task_ctx, &draft, Bytes::from_static(b"x"));
        // Replayed by a different node: the envelope still names `a` as via.
        let other = NodeId::new("127.0.0.1", 1).unwrap();
        let req = request_from(&other, "k", framed);
        assert!(matches!(verify(&a.task_ctx, &req), Err(CallerError::ViaMismatch { .. })));
    }

    #[tokio::test]
    async fn malformed_envelope_is_refused_not_ignored() {
        let a = agent(false);
        let mut framed = FRAME_MAGIC.to_vec();
        let junk = b"not json";
        framed.extend_from_slice(&(junk.len() as u16).to_be_bytes());
        framed.extend_from_slice(junk);
        framed.extend_from_slice(b"payload");
        let req = request_from(a.node_id(), "k", Bytes::from(framed));
        assert_eq!(req.payload(), Bytes::from_static(b"payload"));
        assert!(matches!(verify(&a.task_ctx, &req), Err(CallerError::Malformed(_))));
    }

    #[test]
    fn client_admission_is_by_principal_never_by_gateway_node() {
        let via = NodeId::new("127.0.0.1", 9).unwrap();
        let c = GatewayCaller {
            principal: "oidc:alice".into(),
            via: via.clone(),
            scopes: vec![],
            issued_at_ms: 0,
            attestation: CallerAttestation::UnauthenticatedMesh,
        };
        assert!(client_admitted(&[], &c), "empty allowlist is open");
        assert!(client_admitted(&["oidc:alice".into()], &c));
        assert!(!client_admitted(&[via.to_string().into()], &c), "listing the gateway node admits nothing");
        assert!(!client_admitted(&["oidc:bob".into()], &c));
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn tls_signed_context_verifies_and_forgeries_fail() {
        let a = agent(true);
        a.start().await.unwrap();
        let draft = ResolvedPrincipal { principal: "oidc:alice".into(), scopes: vec!["mesh:write".into()] };
        let app = Bytes::from_static(b"payload");
        let framed = frame_with_context(&a.task_ctx, &draft, app.clone());
        let req = request_from(a.node_id(), "k", framed.clone());
        let c = verify(&a.task_ctx, &req).unwrap().expect("context");
        let own = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
        assert_eq!(c.attestation, CallerAttestation::Signed { signer: own });

        // Tampered payload: the digest no longer matches.
        let (env, _) = split_frame(&framed);
        let mut tampered = FRAME_MAGIC.to_vec();
        let env = env.unwrap();
        tampered.extend_from_slice(&(env.len() as u16).to_be_bytes());
        tampered.extend_from_slice(&env);
        tampered.extend_from_slice(b"PAYLOAD");
        let req = request_from(a.node_id(), "k", Bytes::from(tampered));
        assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::BadSignature));

        // Unsigned envelope on an authenticated node: refused.
        let unsigned = serde_json::to_vec(&Envelope {
            v: 1, p: "oidc:mallory".into(), via: a.node_id().to_string(), s: vec![], t: 0, k: None, sig: None,
        }).unwrap();
        let mut f = FRAME_MAGIC.to_vec();
        f.extend_from_slice(&(unsigned.len() as u16).to_be_bytes());
        f.extend_from_slice(&unsigned);
        let req = request_from(a.node_id(), "k", Bytes::from(f));
        assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::Unsigned));

        // Signed by a key nobody trusts for `via`: refused.
        {
            use base64::Engine as _;
            let rogue = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
            let msg = signed_message("oidc:mallory", &a.node_id().to_string(), &[], 0, &digest(b"payload"));
            let sig = crate::tls::sign_bytes(&rogue, &msg);
            let forged = serde_json::to_vec(&Envelope {
                v: 1, p: "oidc:mallory".into(), via: a.node_id().to_string(), s: vec![], t: 0,
                k: Some(base64::engine::general_purpose::STANDARD.encode(rogue.verifying_key().to_bytes())),
                sig: Some(base64::engine::general_purpose::STANDARD.encode(sig)),
            }).unwrap();
            let mut f = FRAME_MAGIC.to_vec();
            f.extend_from_slice(&(forged.len() as u16).to_be_bytes());
            f.extend_from_slice(&forged);
            f.extend_from_slice(b"payload");
            let req = request_from(a.node_id(), "k", Bytes::from(f));
            assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::UnknownSigner));
        }
        a.shutdown().await;
    }
}
