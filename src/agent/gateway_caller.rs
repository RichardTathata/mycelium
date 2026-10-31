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
//! `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)`, and (post-360 row E) a second one over
//! those fields plus the envelope's **id** (the gateway's HLC at framing) and the node it is addressed
//! to. A provider admits each `(gateway, issue time, id)` once, at the place it receives the request
//! ([`verify_received`]: `rpc_rx`, the MCP tool loop and bridge, the SDK serve stream, `llm.invoke`, the
//! explain responder), and drops a replay — repeated, misaddressed, or with the id stripped from a
//! gateway that publishes `sys/caller-context-id/` — without answering. A struct with a principal name and
//! a digest supplied by anyone else is not evidence — the provider verifies the signer against
//! the keys it knows for `via` (`sys/identity`, anchors, minus revocations) and rejects a
//! mismatch, an unknown signer, an unsigned envelope on an authenticated mesh, a `via` that is
//! not the frame's sender, or an envelope issued outside the freshness window
//! ([`CALLER_CONTEXT_MAX_AGE_MS`] plus the drift bound behind, the drift bound ahead — post-360 row E).
//! **Any rejection denies; nothing ever falls back to the node.**
//!
//! **Where it rides.** Inside the RPC payload, after the 8-byte nonce — the wire (v12) is
//! unchanged: `[0x00 'G' 'W' 'C' 0x01] ‖ u16be len ‖ envelope JSON ‖ application payload`.
//! [`RpcRequest::payload`](super::rpc::RpcRequest::payload) strips it, so every existing
//! provider loop on a node running this code sees exactly the application bytes; the context is
//! read through [`GossipAgent::gateway_caller`] / [`GossipAgent::request_principal`], and
//! **verified at the receive boundary** by `ServiceHandle::rpc_rx`, which answers a refusal itself
//! and never yields a request whose context failed (so every companion loop is covered).
//!
//! **A gateway node's own RPCs carry a self envelope.** A node that runs a gateway in the secure
//! profile publishes marker [`MARKER_PROMISES_ENVELOPES`] and wraps its own `rpc_call`s in a signed
//! `node:{self}` envelope ([`frame_self`]); providers map it back to [`RequestPrincipal::Node`]. That
//! is what makes RPC-shaped bytes sent through a *raw* emission route (`/gateway/signal/emit`)
//! distinguishable from the node's action: they arrive bare from a node that promised an envelope
//! — [`CallerError::Missing`]. Principals are qualified by their issuing authority
//! ([`legacy_token_principal`], [`named_token_principal`], [`positional_token_principal`],
//! [`oidc_principal`]) so two gateways never mint one identity by accident.
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
//! test:* `gateway_caller_tests` in `src/agent/http/gateway_caller_tests.rs` — the four negative cases and the
//! `authorized_callers` gate.

use crate::node_id::NodeId;
use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "gateway", feature = "compliance", test))]
use std::sync::Arc;
#[cfg(any(feature = "gateway", test))]
use std::time::Duration;

#[cfg(any(feature = "gateway", test))]
use super::rpc::RpcError;
use super::rpc::RpcRequest;
use super::TaskCtx;

/// Envelope version carried in the frame magic.
pub const CALLER_CONTEXT_VERSION: u8 = 1;
/// The principal an open gateway (no token model configured, or `/a2a` without a bearer)
/// resolves to. Listing it in `authorized_callers` admits unauthenticated gateway clients — of
/// **every** gateway; anonymity carries no issuer.
pub const PRINCIPAL_ANONYMOUS: &str = "anonymous";

/// `sys/caller-context/{node}` value: this node strips and **verifies** the envelope on its RPC
/// receive path.
pub const MARKER_VERIFIES: &[u8] = b"1";
/// `sys/caller-context/{node}` value: as [`MARKER_VERIFIES`], **and** every RPC this node originates
/// carries an envelope — a client's, or its own signed *self* envelope. A bare RPC-shaped frame from
/// such a node is therefore never the node's own action (review finding 1: a raw `/gateway/signal/emit`
/// of nonce-prefixed bytes used to reach a provider as the gateway node). Written by nodes that run a
/// gateway in the secure profile.
pub const MARKER_PROMISES_ENVELOPES: &[u8] = b"2";

/// Frame magic after the RPC nonce: a leading NUL (no JSON or text payload starts with one),
/// then `GWC`; the fifth byte is the version.
const FRAME_MAGIC_PREFIX: [u8; 4] = [0x00, b'G', b'W', b'C'];
const FRAME_MAGIC: [u8; 5] = [0x00, b'G', b'W', b'C', CALLER_CONTEXT_VERSION];

/// The principal of a bearer that matched the legacy `gateway_auth_token`, qualified by the
/// gateway's identity issuer: `token:{issuer}/legacy`.
pub fn legacy_token_principal(issuer: &str) -> String { format!("token:{issuer}/legacy") }
/// The principal of the `i`-th `gateway_scoped_tokens` entry: `token:{issuer}/#{i}` — positional,
/// so reordering the list moves the identity; prefer named tokens.
pub fn positional_token_principal(issuer: &str, index: usize) -> String { format!("token:{issuer}/#{index}") }
/// The principal of a `gateway_named_tokens` entry: `token:{issuer}/{name}`.
pub fn named_token_principal(issuer: &str, name: &str) -> String { format!("token:{issuer}/{name}") }
/// The principal of a validated OIDC bearer: `oidc:{idp issuer}/{subject}` — two IdPs' subjects
/// never collide.
pub fn oidc_principal(issuer: &str, subject: &str) -> String { format!("oidc:{issuer}/{subject}") }
/// The principal a node's **own** RPC carries in its self envelope: `node:{node id}`. A provider
/// maps it back to [`RequestPrincipal::Node`].
pub fn node_principal(node: &NodeId) -> String { format!("node:{node}") }

/// A federated caller (item 2 PR 8): the partner domain is the issuer, the principal is the one
/// the partner's credential named. `federation:beta.example/svc/billing` — never the gateway that
/// carried it, which is item 7's fix one boundary further out.
pub fn federation_principal(origin_domain: &str, principal: &str) -> String {
    format!("federation:{origin_domain}/{principal}")
}
/// Domain separator for the signed message.
#[cfg(feature = "tls")]
const DOMAIN_SEP: &[u8] = b"mycelium:gateway-caller:v1\n";
/// Domain separator for the signature that also covers the envelope's id (post-360 row E's
/// replay window). A gateway signs both: the v1 form for a provider that predates the id, this one
/// for every provider that keeps a replay window.
#[cfg(feature = "tls")]
const DOMAIN_SEP_ID: &[u8] = b"mycelium:gateway-caller:v2\n";
/// The most envelope ids a provider keeps per gateway inside the freshness window. Past it the
/// oldest is evicted and becomes the window's floor: an envelope at or below the floor is refused
/// (it might be a replay of one forgotten), so the bound fails closed. Ids are kept for the
/// freshness window — 300 s plus `max_clock_drift_ms`, 600 s at the default drift bound — so the floor
/// engages past about 109 calls a second, sustained, from one gateway to one provider.
pub(crate) const REPLAY_WINDOW_PER_GATEWAY: usize = 65_536;
/// Upper bound on an envelope: a principal, a node id, a few scopes.
pub(crate) const MAX_ENVELOPE_BYTES: usize = 8 * 1024;
/// How old a caller envelope may be when a provider verifies it, before the clock-drift allowance:
/// 300 s, the longest any gateway door waits for an answer (`SERVED_RPC_TTL_MS`), so an envelope
/// older than this is answering no *gateway* caller that still waits (post-360 hardening row E). An
/// in-process caller (`rpc_call`, `rpc_call_with_mandate`) names its own timeout, and the check runs
/// when the provider reads the request from `rpc_rx`: behind a backlog longer than this, such a request
/// is refused `stale` though its caller still waits. [`verify`] refuses one issued more than this
/// **plus** the node's `max_clock_drift_ms` ago, or more than `max_clock_drift_ms` ahead of its own
/// clock ([`CallerError::Stale`]); with `max_clock_drift_ms = 0` (no drift bound) the age bound is this
/// alone, and an envelope may be at most this far ahead.
pub const CALLER_CONTEXT_MAX_AGE_MS: u64 = 300_000;

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
    /// A `correlation` or `parent` sat in the envelope beside `m` — outside every signature, never read
    /// (adopter-surface X4). Set by [`verify`] after the signature held, counted once per request by the receive
    /// boundary ([`verify_received`]) or by the provider check when it is the first to verify.
    pub(crate) misplaced_causal: bool,
    /// The mandate the caller presented (`params._meta.mandate` at the gateway, or a member's own
    /// grant on a direct call), **carried, not verified** (closure plan C2). It is not covered by the
    /// gateway's signature and needs not be: the grant is signed by its establishing authority and the
    /// possession proof by the holder over this call's operation, resource and arguments, so a relay
    /// that strips it only causes a refusal, and one that swaps it fails verification. A provider
    /// verifies it for itself (C3); it never takes a gateway's word that a mandate was established.
    pub mandate: Option<serde_json::Value>,
    /// The resource the caller says this call acts on (closure plan C3), e.g.
    /// `skill:depot/dispatch@{provider}`: what the gateway resolved, or what a member names with
    /// [`rpc_call_with_mandate`](crate::ServiceHandle::rpc_call_with_mandate). A **claim**: a
    /// provider checking mandates uses it only where the payload cannot name the resource itself,
    /// and only after confirming it names this node and something this node serves.
    pub resource: Option<String>,
    /// The envelope's id, unique per gateway (its HLC at issue), signed with the rest under a `tls`
    /// identity (post-360 row E). `None` from a gateway that predates it: such an envelope is accepted
    /// and counted for one release (`GET /stats` `caller_envelopes_without_id`), unprotected against
    /// replay inside the freshness window.
    pub envelope_id: Option<u64>,
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
    /// No envelope, from a sender whose `sys/caller-context` marker promises one on every RPC it
    /// originates (a secure-profile gateway node): a raw emission shaped like an RPC, never the
    /// node's own action.
    Missing,
    /// The sending node has been removed from the mesh by an operator (closure plan C5).
    Removed,
    /// The envelope's issue time is outside the window this node accepts: older than
    /// [`CALLER_CONTEXT_MAX_AGE_MS`] plus the drift bound, or further ahead than the drift bound
    /// (post-360 hardening row E). Under `tls` the time is inside the signature, so a captured
    /// envelope cannot be re-dated; without it the time is carried, as the rest of the envelope is.
    Stale {
        /// The envelope's `issued_at_ms`.
        issued_at_ms: u64,
        /// This node's decision clock when it verified (`Hlc::decision_now_ms`).
        now_ms: u64,
    },
    /// This envelope — its gateway, issue time and id — was already received inside the freshness
    /// window, or falls at or below the window's floor after an eviction (post-360 row E). A replay:
    /// the receive boundary drops it **without answering**, because a refusal sent back under the
    /// original's correlation nonce could reach the gateway before the real answer.
    Replayed {
        /// The envelope's `issued_at_ms`.
        issued_at_ms: u64,
        /// The envelope's id.
        id: u64,
    },
    /// The envelope is addressed (signed) to another node (post-360 row E's review): a capture
    /// replayed at a provider it was not sent to. Dropped without an answer, as a replay.
    Misaddressed {
        /// The node the envelope was signed for.
        to: String,
    },
    /// A signed envelope without an id from a gateway that issues one on every envelope (its
    /// `sys/caller-context-id/` marker, or this node itself): one of its envelopes with the id
    /// stripped — a replay reaching for the pre-row-E path. Dropped without an answer.
    IdStripped,
}

impl CallerError {
    /// A stable machine-readable name, carried in `rpc_rx`'s refusal reply so a requester can act on it: the signer
    /// not yet known and the marker not yet seen pass as gossip converges; the rest hold until something changes.
    pub fn code(&self) -> &'static str {
        match self {
            CallerError::Malformed(_) => "malformed",
            CallerError::ViaMismatch { .. } => "via_mismatch",
            CallerError::Unsigned => "unsigned",
            CallerError::UnknownSigner => "unknown_signer",
            CallerError::BadSignature => "bad_signature",
            CallerError::Missing => "missing",
            CallerError::Removed => "removed",
            CallerError::Stale { .. } => "stale",
            CallerError::Replayed { .. } => "replayed",
            CallerError::Misaddressed { .. } => "misaddressed",
            CallerError::IdStripped => "id_stripped",
        }
    }

    /// Whether a receive boundary must drop the request **without answering** (post-360 row E): a
    /// replay in any of its shapes. A refusal sent back under a captured request's correlation nonce
    /// could reach its gateway before the real answer.
    pub fn unanswered(&self) -> bool {
        matches!(self, CallerError::Replayed { .. } | CallerError::Misaddressed { .. } | CallerError::IdStripped)
    }
}

impl std::fmt::Display for CallerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallerError::Removed => write!(f, "the sending node has been removed from the mesh"),
            CallerError::Stale { issued_at_ms, now_ms } => write!(
                f,
                "caller context issued at {issued_at_ms} ms is outside the accepted window at {now_ms} ms (at most \
                 {CALLER_CONTEXT_MAX_AGE_MS} ms old plus the clock-drift bound, never beyond that bound ahead; \
                 with a drift bound of 0, at most that age either way)"
            ),
            CallerError::Replayed { issued_at_ms, id } => write!(
                f,
                "caller context {id} issued at {issued_at_ms} ms was already received (a replay inside the freshness window)"
            ),
            CallerError::Misaddressed { to } => write!(f, "caller context is addressed to {to}, not this node (a replay)"),
            CallerError::IdStripped => f.write_str(
                "caller context has no id, from a gateway that issues one on every envelope (a replay with the id stripped)",
            ),
            CallerError::Malformed(why) => write!(f, "caller context malformed: {why}"),
            CallerError::ViaMismatch { claimed, sender } => {
                write!(f, "caller context names gateway {claimed} but the frame came from {sender}")
            }
            CallerError::Unsigned => f.write_str("caller context is unsigned on an authenticated mesh"),
            CallerError::UnknownSigner => f.write_str("caller context signer is not a trusted key for the gateway"),
            CallerError::BadSignature => f.write_str("caller context signature does not verify"),
            CallerError::Missing => f.write_str(
                "caller context missing from a node that promises one on every RPC (a raw gateway emission is not the node's action)",
            ),
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
    #[deprecated(since = "2.33.0", note = "no caller, test, SDK or document in the repository references it (post-360 hardening row J, 2026-10-12); kept on the 2.x line — open an issue if you use it and it will be undeprecated")]
    pub fn is_client(&self) -> bool {
        matches!(self, RequestPrincipal::Client(_))
    }
}

// ── Auth layer → dispatch ────────────────────────────────────────────────────

/// What the gateway auth layer resolved for one HTTP request: the principal and the authority
/// granted for the request. **Constructed only by the auth middleware in `http/authz.rs`** — the fields
/// are crate-private, so a companion can read it (axum's `Extension<ResolvedPrincipal>` on a route
/// merged with `with_http_routes`) and hand it on (`ServiceHandle::rpc_call_as`,
/// `GossipAgent::gateway_preflight`), never make one up.
#[cfg(any(feature = "gateway", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPrincipal {
    pub(crate) principal: String,
    pub(crate) scopes: Vec<String>,
}

#[cfg(any(feature = "gateway", test))]
impl ResolvedPrincipal {
    /// The issuer-qualified principal (`token:{issuer}/{name}`, `oidc:{issuer}/{subject}`, …), or
    /// [`PRINCIPAL_ANONYMOUS`].
    pub fn principal(&self) -> &str {
        &self.principal
    }
    /// The scopes granted for this request: the credential's, intersected with what the route
    /// required.
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
}

#[cfg(any(feature = "gateway", test))]
impl ResolvedPrincipal {
    pub(crate) fn anonymous(scopes: Vec<String>) -> Self {
        Self { principal: PRINCIPAL_ANONYMOUS.to_string(), scopes }
    }

    /// This node as the actor of a door its own code opens (adopter-surface Y11's Rust verbs): `node:{id}`, no scopes.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub(crate) fn node(node: &NodeId) -> Self {
        Self { principal: node_principal(node), scopes: Vec::new() }
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

/// Is this node one whose every originated RPC carries an envelope — a gateway in the secure
/// profile? Decides the marker value it publishes and whether `rpc_call` wraps a self envelope.
pub(crate) fn promises_envelopes(config: &crate::config::GossipConfig) -> bool {
    config.http_port.is_some() && config.gateway_caller_profile == crate::config::GatewayCallerProfile::Secure
}

/// The `sys/caller-context/{self}` value this node publishes.
pub(crate) fn marker_value(config: &crate::config::GossipConfig) -> &'static [u8] {
    if promises_envelopes(config) { MARKER_PROMISES_ENVELOPES } else { MARKER_VERIFIES }
}

/// How often a node re-asserts its caller-envelope id marker (post-360 row E).
#[cfg(feature = "tls")]
pub(crate) const ID_MARKER_REFRESH_MS: u64 = 60_000;
/// Domain separator for the id marker's signature.
#[cfg(feature = "tls")]
const ID_MARKER_SEP: &[u8] = b"mycelium:caller-context-id:v1\n";

/// The id marker's value: `issued_at_ms (8, LE) ‖ signer (32) ‖ Ed25519 signature (64)` over
/// [`ID_MARKER_SEP`] `‖ node ‖ issued_at_ms` — the node's own dated statement that every envelope it
/// frames carries an id. `None` without a `tls` identity (an unsigned envelope keeps no window anyway).
#[cfg(feature = "tls")]
pub(crate) fn id_marker_value(ctx: &TaskCtx) -> Option<Bytes> {
    let tls = ctx.tls.get()?;
    Some(id_marker_bytes(&ctx.node_id, &tls.signing_key(), ctx.hlc.decision_now_ms()))
}

/// An id marker for `node`, issued at `at`, signed by `sk`.
#[cfg(feature = "tls")]
pub(crate) fn id_marker_bytes(node: &NodeId, sk: &ed25519_dalek::SigningKey, at: u64) -> Bytes {
    let sig = crate::tls::sign_bytes(sk, &id_marker_message(node, at));
    let mut v = BytesMut::with_capacity(104);
    v.put_u64_le(at);
    v.put_slice(&sk.verifying_key().to_bytes());
    v.put_slice(&sig);
    v.freeze()
}

#[cfg(feature = "tls")]
fn id_marker_message(node: &NodeId, at: u64) -> Vec<u8> {
    let mut m = ID_MARKER_SEP.to_vec();
    m.extend_from_slice(node.to_string().as_bytes());
    m.extend_from_slice(&at.to_le_bytes());
    m
}

/// Does `node` put an id on every caller envelope it frames (post-360 row E)? This node does (it runs
/// this code). Another node does if its `sys/caller-context-id/` marker is **its own** — signed by a
/// key this node trusts for it, so a member cannot plant one for a gateway on an older release — and
/// **fresh**: issued within the freshness window plus two refresh periods. A node that stops
/// re-asserting it (rolled back to a release without ids, or stopped) is treated as id-less once its
/// last envelopes can no longer be fresh, so a rollback is not cut off for good, and a stopped node's
/// stripped envelopes are refused for as long as they could be replayed.
#[cfg(feature = "tls")]
fn issues_envelope_ids(ctx: &TaskCtx, node: &NodeId) -> bool {
    if *node == ctx.node_id {
        return true;
    }
    let Some(v) = mycelium_core::ops::kv_get(ctx, &id_marker_key(node)) else { return false };
    if v.len() != 104 {
        return false;
    }
    let at = u64::from_le_bytes(v[..8].try_into().expect("8 bytes"));
    let signer: [u8; 32] = v[8..40].try_into().expect("32 bytes");
    let now = ctx.hlc.decision_now_ms();
    let drift = ctx.config.max_clock_drift_ms;
    let ttl = CALLER_CONTEXT_MAX_AGE_MS.saturating_add(drift).saturating_add(2 * ID_MARKER_REFRESH_MS);
    if now.saturating_sub(at) > ttl || at.saturating_sub(now) > drift.max(ID_MARKER_REFRESH_MS) {
        return false;
    }
    verifying_keys_for(ctx, node).contains(&signer) && crate::tls::verify_bytes(&signer, &id_marker_message(node, at), &v[40..])
}

/// KV key of a node's caller-envelope id marker (post-360 row E).
#[cfg(feature = "tls")]
pub(crate) fn id_marker_key(node: &NodeId) -> String {
    format!("{}{}", crate::signal::kv_ns::CALLER_CONTEXT_ID, node)
}

/// Did `sender` promise an envelope on every RPC it originates (its marker is
/// [`MARKER_PROMISES_ENVELOPES`])? For self, the local config answers.
fn sender_promises_envelopes(ctx: &TaskCtx, sender: &NodeId) -> bool {
    if *sender == ctx.node_id {
        return promises_envelopes(&ctx.config);
    }
    mycelium_core::ops::kv_get(ctx, &marker_key(sender))
        .is_some_and(|v| v.as_ref() == MARKER_PROMISES_ENVELOPES)
}

/// Why a gateway dispatch was refused before (or while) reaching the provider.
#[cfg(any(feature = "gateway", test))]
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayDispatchError {
    /// The provider replied nothing in time.
    Rpc(RpcError),
    /// Secure profile, but no caller context reached the dispatch site — a handler outside the
    /// auth layer tried to dispatch. Refused rather than run as the node.
    MissingContext,
    /// Secure profile, and the target has no `sys/caller-context/{node}` marker: it would run
    /// the call as this node. Refused; the message names the provider.
    ProviderWithoutContext(NodeId),
    /// The resolved principal or scopes do not fit the envelope bound a receiver accepts
    /// (`MAX_ENVELOPE_BYTES`); refused before dispatch rather than truncated (review finding 4).
    ContextTooLarge,
}

#[cfg(any(feature = "gateway", test))]
impl GatewayDispatchError {
    /// JSON-RPC error code for the MCP / A2A surfaces.
    pub fn json_rpc_code(&self) -> i32 {
        match self {
            GatewayDispatchError::Rpc(_) => -32000,
            GatewayDispatchError::MissingContext => -32020,
            GatewayDispatchError::ProviderWithoutContext(_) => -32021,
            GatewayDispatchError::ContextTooLarge => -32023,
        }
    }

    /// Short machine-readable reason for JSON bodies and metrics.
    pub fn reason(&self) -> &'static str {
        match self {
            GatewayDispatchError::Rpc(_) => "timeout",
            GatewayDispatchError::MissingContext => "caller_context_missing",
            GatewayDispatchError::ProviderWithoutContext(_) => "provider_without_caller_context",
            GatewayDispatchError::ContextTooLarge => "caller_context_too_large",
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
            GatewayDispatchError::ContextTooLarge => f.write_str(
                "gateway caller context exceeds the envelope bound (principal or scopes too long)",
            ),
        }
    }
}

#[cfg(any(feature = "gateway", test))]
impl std::error::Error for GatewayDispatchError {}

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
    gateway_rpc_call_with_mandate(ctx, caller, target, kind, payload, timeout, None, None).await
}

/// [`gateway_rpc_call`], carrying the mandate the caller presented to the provider (closure plan C2).
/// Under the `Legacy` profile there is no envelope, so nothing is carried.
#[cfg(any(feature = "gateway", test))]
#[allow(clippy::too_many_arguments)] // one call's facts: who, where, what, and under which mandate
pub(crate) async fn gateway_rpc_call_with_mandate(
    ctx: &TaskCtx,
    caller: Option<&ResolvedPrincipal>,
    target: NodeId,
    kind: Arc<str>,
    payload: Bytes,
    timeout: Duration,
    mandate: Option<&serde_json::Value>,
    resource: Option<&str>,
) -> Result<Bytes, GatewayDispatchError> {
    use crate::config::GatewayCallerProfile;
    match ctx.config.gateway_caller_profile {
        // Legacy: a bare frame — the node's own action, as before item 7. `rpc_call_framed` (not
        // `rpc_call_ctx`) so no self envelope is attached either: legacy is legacy.
        GatewayCallerProfile::Legacy => super::rpc::rpc_call_framed(ctx, target, kind, payload, timeout)
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
            let Some(framed) = frame_with_context_and_mandate(ctx, &target, &caller.principal, &caller.scopes, payload, mandate, resource) else {
                refused("caller_context_too_large");
                return Err(GatewayDispatchError::ContextTooLarge);
            };
            super::rpc::rpc_call_framed(ctx, target, kind, framed, timeout)
                .await
                .map_err(GatewayDispatchError::Rpc)
        }
    }
}

/// The mandate a caller presented in `params._meta.mandate`, if any, to carry to the provider
/// (closure plan C2). Carried as sent; the provider verifies it.
#[cfg(feature = "gateway")]
pub(crate) fn presented_mandate(params: &serde_json::Value) -> Option<&serde_json::Value> {
    params.get("_meta").and_then(|m| m.get("mandate")).filter(|v| !v.is_null())
}

#[cfg(any(feature = "gateway", test))]
#[allow(unused_variables)]
fn refused(reason: &'static str) {
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_caller_refusals_total", "reason" => reason).increment(1);
}

// ── Wire envelope ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "fuzz-internals", derive(Debug, PartialEq))]
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
    /// The presented mandate, as JSON (closure plan C2). Absent on the wire when there is none, so
    /// an envelope without one is byte-identical to before, and an older provider ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    m: Option<serde_json::Value>,
    /// The resource the call claims to act on (closure plan C3). Absent when not named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    r: Option<String>,
    /// The envelope's id: the issuing gateway's HLC at framing, so unique per gateway (post-360 row
    /// E). An older provider ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u64>,
    /// The node the envelope is addressed to — inside the v2 signature, so a capture cannot be replayed
    /// at another provider. Present whenever `n` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    o: Option<String>,
    /// base64 Ed25519 signature over the v2 message — the v1 fields, `n` and `o`. An older provider reads
    /// `sig`; a provider that keeps a replay window requires this one whenever `n` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sn: Option<String>,
    /// Fields this version does not define, kept so a verifier can see them: a `correlation` or `parent` placed here
    /// (adopter-surface X4) sits outside every signature and is never read — counted, and the record's field stays
    /// empty. Never produced by this crate; absent on the wire when empty, so the envelope's bytes are unchanged.
    #[serde(default, flatten, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    x: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Build the framed payload: magic ‖ len ‖ envelope ‖ application payload. Signs under a `tls`
/// identity; otherwise the envelope is unsigned (see [`CallerAttestation::UnauthenticatedMesh`]).
/// `None` when the envelope would exceed the bound a receiver accepts (`MAX_ENVELOPE_BYTES`) —
/// the producer never truncates (review finding 4).
pub(crate) fn frame_with_context(ctx: &TaskCtx, target: &NodeId, principal: &str, scopes: &[String], app: Bytes) -> Option<Bytes> {
    frame_with_context_and_mandate(ctx, target, principal, scopes, app, None, None)
}

/// [`frame_with_context`], carrying a presented mandate for the provider to verify (closure plan C2).
/// `None` when the envelope, mandate included, would exceed `MAX_ENVELOPE_BYTES`.
pub(crate) fn frame_with_context_and_mandate(
    ctx: &TaskCtx,
    target: &NodeId,
    principal: &str,
    scopes: &[String],
    app: Bytes,
    mandate: Option<&serde_json::Value>,
    resource: Option<&str>,
) -> Option<Bytes> {
    // A caller envelope's issue time is checked against a lifetime by whoever receives it, so it
    // must come from a clock that moves. See `Hlc::decision_now_ms`.
    frame_with_context_at(ctx, target, principal, scopes, app, mandate, resource, ctx.hlc.decision_now_ms())
}

/// [`frame_with_context_and_mandate`] at a given issue time — the one place an envelope is built.
#[allow(clippy::too_many_arguments)] // the envelope's fields, each one
pub(crate) fn frame_with_context_at(
    ctx: &TaskCtx,
    target: &NodeId,
    principal: &str,
    scopes: &[String],
    app: Bytes,
    mandate: Option<&serde_json::Value>,
    resource: Option<&str>,
    issued_at_ms: u64,
) -> Option<Bytes> {
    let via = ctx.node_id.to_string();
    let id = ctx.hlc.tick();
    let to = target.to_string();
    let (k, sig, sn) = attest(ctx, principal, &via, scopes, issued_at_ms, id, &to, &app);
    let env = Envelope {
        v: CALLER_CONTEXT_VERSION,
        p: principal.to_string(),
        via,
        s: scopes.to_vec(),
        t: issued_at_ms,
        k,
        sig,
        m: mandate.cloned(),
        r: resource.map(str::to_string),
        n: Some(id),
        o: Some(to),
        sn,
        x: Default::default(),
    };
    let env_bytes = serde_json::to_vec(&env).ok()?;
    if env_bytes.len() > MAX_ENVELOPE_BYTES {
        return None;
    }
    let len = u16::try_from(env_bytes.len()).ok()?;
    let mut buf = BytesMut::with_capacity(FRAME_MAGIC.len() + 2 + env_bytes.len() + app.len());
    buf.put_slice(&FRAME_MAGIC);
    buf.put_u16(len);
    buf.put_slice(&env_bytes);
    buf.put(app);
    buf.freeze()
    .into()
}

/// The **self envelope** a secure-profile gateway node wraps around its own RPCs
/// (`principal = node:{self}`, no scopes): what makes a bare RPC-shaped frame from that node
/// distinguishable from its own action. Falls back to the bare payload only if the envelope
/// cannot be built (it cannot exceed the bound: a node id is short).
pub(crate) fn frame_self(ctx: &TaskCtx, target: &NodeId, app: Bytes) -> Bytes {
    frame_with_context(ctx, target, &node_principal(&ctx.node_id), &[], app.clone()).unwrap_or(app)
}

/// The `(signer, signature, id signature)` triple for an envelope: Ed25519 by this node's identity
/// key under a `tls` identity — the v1 signature an older provider verifies, and the v2 one over the
/// same fields and the envelope's id — `(None, None, None)` otherwise (an unauthenticated mesh has
/// nothing to sign with).
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)] // the signed fields, each one
fn attest(
    ctx: &TaskCtx,
    principal: &str,
    via: &str,
    scopes: &[String],
    issued_at_ms: u64,
    id: u64,
    to: &str,
    app: &[u8],
) -> (Option<String>, Option<String>, Option<String>) {
    use base64::Engine as _;
    let Some(tls) = ctx.tls.get() else { return (None, None, None) };
    let sk = tls.signing_key();
    let signer = sk.verifying_key().to_bytes();
    let d = digest(app);
    let sig = crate::tls::sign_bytes(&sk, &signed_message(principal, via, scopes, issued_at_ms, &d));
    let sig_id = crate::tls::sign_bytes(&sk, &signed_message_with_id(principal, via, scopes, issued_at_ms, id, to, &d));
    let b64 = base64::engine::general_purpose::STANDARD;
    (Some(b64.encode(signer)), Some(b64.encode(sig)), Some(b64.encode(sig_id)))
}

#[cfg(not(feature = "tls"))]
#[allow(clippy::too_many_arguments)] // mirrors the `tls` signature
fn attest(
    _ctx: &TaskCtx,
    _principal: &str,
    _via: &str,
    _scopes: &[String],
    _issued_at_ms: u64,
    _id: u64,
    _to: &str,
    _app: &[u8],
) -> (Option<String>, Option<String>, Option<String>) {
    (None, None, None)
}

/// A nonce-stripped RPC payload, classified. Three outcomes, never two (review finding 4): a
/// recognised frame that is malformed is **not** an unframed payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// No frame magic: the whole payload is the application payload.
    Unframed(Bytes),
    /// A well-formed frame: the envelope bytes and the application payload after it.
    Framed { envelope: Bytes, app: Bytes },
    /// The frame magic prefix is present but the frame cannot be used: an unsupported version, a
    /// truncated header, a declared length beyond the bound or beyond the payload. The
    /// application payload is undefined; the request must be refused.
    Malformed(&'static str),
}

/// Does a **client-supplied** payload carry a caller-context frame where one would be read?
///
/// An RPC payload is `[8-byte nonce][frame][application bytes]`, so a client that controls every
/// byte of a raw emission can prepend its own envelope. The gateway emits those bytes verbatim with
/// itself as the sender, so `via` matches by construction and the envelope verifies as this node's.
/// The pre-existing `CallerError::Missing` guard only catches *bare* bytes; a client that supplies a
/// frame walked straight past it.
///
/// Raw-emission routes call this and refuse. A legitimate caller context is constructed by the auth
/// layer and never by a request body.
/// Found by the Phase-C adversarial audit (items 1+2+7).
///
/// Gated: the raw-emission routes it guards exist only with the gateway. (A minimal build has no
/// route through which a client could supply bytes at all.)
#[cfg(any(feature = "gateway", test))]
pub(crate) fn carries_caller_frame(payload: &[u8]) -> bool {
    let start = 8; // past the RPC nonce
    payload.len() >= start + FRAME_MAGIC_PREFIX.len()
        && payload[start..start + FRAME_MAGIC_PREFIX.len()] == FRAME_MAGIC_PREFIX
}

/// Classify a nonce-stripped RPC payload (see [`Frame`]).
pub(crate) fn split_frame(after_nonce: &Bytes) -> Frame {
    let prefix = FRAME_MAGIC_PREFIX.len();
    if after_nonce.len() < prefix || after_nonce[..prefix] != FRAME_MAGIC_PREFIX {
        return Frame::Unframed(after_nonce.clone());
    }
    let hdr = FRAME_MAGIC.len() + 2;
    if after_nonce.len() < hdr {
        return Frame::Malformed("truncated frame header");
    }
    if after_nonce[prefix] != CALLER_CONTEXT_VERSION {
        return Frame::Malformed("unsupported caller-context frame version");
    }
    let len = u16::from_be_bytes([after_nonce[hdr - 2], after_nonce[hdr - 1]]) as usize;
    if len > MAX_ENVELOPE_BYTES {
        return Frame::Malformed("envelope length exceeds the bound");
    }
    if after_nonce.len() < hdr + len {
        return Frame::Malformed("envelope truncated");
    }
    Frame::Framed { envelope: after_nonce.slice(hdr..hdr + len), app: after_nonce.slice(hdr + len..) }
}

/// Fuzz hook (§12.6): drive the caller-context **envelope** decode exactly as [`verify`] does, up
/// to but not including the signature check.
///
/// [`split_frame`] only classifies; everything that actually reads the client's envelope happens
/// after it and **before** `verify_bytes` — the JSON decode, the `via` node id, and the two base64
/// fields. Fuzzing the classification alone stops one layer short of the bytes an attacker shapes.
///
/// The invariant asserted is **signing-field stability**: the verifier rebuilds the signed message
/// from these parsed fields, so an envelope that parses one way and re-renders another would have
/// one credential verified and a different one acted on.
#[cfg(feature = "fuzz-internals")]
pub(crate) fn fuzz_envelope_decode(after_nonce: &Bytes) -> bool {
    let Frame::Framed { envelope, .. } = split_frame(after_nonce) else { return false };
    let Ok(env) = serde_json::from_slice::<Envelope>(&envelope) else { return false };

    let rendered = serde_json::to_vec(&env).expect("a parsed envelope must re-render");
    let again: Envelope =
        serde_json::from_slice(&rendered).expect("a rendered envelope must re-parse");
    assert_eq!(env, again, "an envelope did not survive its own round trip");

    // A `via` that is not a node id is a refusal, never a panic.
    let _ = env.via.parse::<NodeId>();

    // Drive both credential fields exactly as `verify` does. There is no assertion here on
    // purpose: that a fixed-size `try_into` refuses a wrong length is a property of the standard
    // library, and asserting it would dress a tautology up as a gate. What this buys is **coverage**
    // -- the base64 decoders run on attacker-shaped input under the fuzzer's instrumentation.
    {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let _: Option<[u8; 32]> =
            env.k.as_deref().and_then(|k| b64.decode(k).ok()).and_then(|v| v.try_into().ok());
        let _: Option<[u8; 64]> =
            env.sig.as_deref().and_then(|s| b64.decode(s).ok()).and_then(|v| v.try_into().ok());
        let _: Option<[u8; 64]> =
            env.sn.as_deref().and_then(|s| b64.decode(s).ok()).and_then(|v| v.try_into().ok());
    }
    true
}

#[cfg(feature = "tls")]
fn digest(app: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(app).into()
}

#[cfg(feature = "tls")]
fn signed_message(principal: &str, via: &str, scopes: &[String], issued_at_ms: u64, digest: &[u8; 32]) -> Vec<u8> {
    signed_fields(DOMAIN_SEP, principal, via, scopes, issued_at_ms, digest)
}

/// The v2 message: the v1 fields under their own domain separator, then the envelope's id and the
/// node it is addressed to.
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)] // the signed fields, each one
fn signed_message_with_id(principal: &str, via: &str, scopes: &[String], issued_at_ms: u64, id: u64, to: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut buf = signed_fields(DOMAIN_SEP_ID, principal, via, scopes, issued_at_ms, digest);
    buf.extend_from_slice(&id.to_le_bytes());
    buf.extend_from_slice(&(to.len() as u32).to_le_bytes());
    buf.extend_from_slice(to.as_bytes());
    buf
}

#[cfg(feature = "tls")]
fn signed_fields(sep: &[u8], principal: &str, via: &str, scopes: &[String], issued_at_ms: u64, digest: &[u8; 32]) -> Vec<u8> {
    fn put(buf: &mut Vec<u8>, field: &[u8]) {
        buf.extend_from_slice(&(field.len() as u32).to_le_bytes());
        buf.extend_from_slice(field);
    }
    let mut buf = Vec::with_capacity(104 + principal.len() + via.len());
    buf.extend_from_slice(sep);
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
/// and CA anchors) plus this node's own current key when `node` is self, minus the keys `node` has
/// revoked (in every `tls` build since post-360 plan row Q).
#[cfg(feature = "tls")]
fn verifying_keys_for(ctx: &TaskCtx, node: &NodeId) -> Vec<[u8; 32]> {
    #[cfg(feature = "compliance")]
    {
        super::helpers::known_verifying_keys(ctx, node)
    }
    #[cfg(not(feature = "compliance"))]
    {
        let mut keys: Vec<[u8; 32]> = ctx.trusted_keys(node);
        if node == &ctx.node_id
            && let Some(t) = ctx.tls.get()
        {
            let cur = t.verifying_key_bytes();
            if !keys.contains(&cur) {
                keys.push(cur);
            }
        }
        // Post-360 plan row Q: revocations reach this build too.
        ctx.retain_trusted(node, &mut keys);
        keys
    }
}

/// Verify the caller context carried by `req`, if any.
///
/// `Ok(None)`: no context — the sender acts for itself. `Ok(Some(_))`: a verified context.
/// `Err(_)`: a context was present and must be **refused**; never treat the call as the node's.
pub(crate) fn verify(ctx: &TaskCtx, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
    // Closure plan C5: a removed member is refused before any serve path, whatever it frames.
    if ctx.removed.is_node_removed(req.sender()) {
        return Err(CallerError::Removed);
    }
    let env_bytes = match req.frame() {
        Frame::Unframed(_) => {
            if sender_promises_envelopes(ctx, req.sender()) {
                return Err(CallerError::Missing);
            }
            return Ok(None);
        }
        Frame::Malformed(why) => return Err(CallerError::Malformed(why.to_string())),
        Frame::Framed { envelope, .. } => envelope,
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
        let decode_sig = |s: &str| -> Result<[u8; 64], CallerError> {
            base64::engine::general_purpose::STANDARD
                .decode(s)
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| CallerError::Malformed("signature is not 64 bytes".into()))
        };
        let sig = decode_sig(sig)?;
        if !verifying_keys_for(ctx, &via).contains(&signer) {
            return Err(CallerError::UnknownSigner);
        }
        let d = digest(&req.payload());
        // Post-360 row E: an envelope with an id must carry the signature that covers it — the id is
        // what the replay window keys on, so an unsigned one would let anyone mint fresh ids. One
        // without an id is a pre-row-E gateway's, verified in the v1 form (counted at the boundary).
        let (msg, sig) = match (env.n, env.o.as_deref(), env.sn.as_deref()) {
            (Some(id), Some(to), Some(sn)) => (signed_message_with_id(&env.p, &env.via, &env.s, env.t, id, to, &d), decode_sig(sn)?),
            (Some(_), _, _) => {
                return Err(CallerError::Malformed("envelope id without its target and the signature that covers them".into()))
            }
            (None, _, _) => (signed_message(&env.p, &env.via, &env.s, env.t, &d), sig),
        };
        if !crate::tls::verify_bytes(&signer, &msg, &sig) {
            return Err(CallerError::BadSignature);
        }
        // Checked after the signature, so a forgery answers as one.
        match env.o.as_deref() {
            Some(to) if to != ctx.node_id.to_string() => {
                return Err(CallerError::Misaddressed { to: to.to_string() });
            }
            None if issues_envelope_ids(ctx, &via) => return Err(CallerError::IdStripped),
            _ => {}
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

    // Post-360 row E: the envelope binds freshness. Its issue time is inside the signature (under
    // `tls`), and it is read here — before, it was signed and never checked, so a captured envelope and
    // payload verified for ever. Checked after the signature, so a forgery still answers as one.
    // `max_clock_drift_ms = 0` means *no drift bound* (`hlc.rs`, `configuration.md`): then there is no
    // future bound, and the age bound is `CALLER_CONTEXT_MAX_AGE_MS` alone (#613's review, finding 1 —
    // an RPC frame does not make this node observe the sender's clock, so a zero tolerance refused
    // every envelope from a gateway a few ms ahead).
    let now_ms = ctx.hlc.decision_now_ms();
    let drift = ctx.config.max_clock_drift_ms;
    // With no drift bound the future is still bounded — by the age bound, so a far-future envelope
    // (a re-dated replay on an unsigned mesh) is refused (#613's final review).
    let ahead_bound = if drift == 0 { CALLER_CONTEXT_MAX_AGE_MS } else { drift };
    let too_far_ahead = env.t.saturating_sub(now_ms) > ahead_bound;
    if now_ms.saturating_sub(env.t) > CALLER_CONTEXT_MAX_AGE_MS.saturating_add(drift) || too_far_ahead {
        return Err(CallerError::Stale { issued_at_ms: env.t, now_ms });
    }

    Ok(Some(GatewayCaller {
        misplaced_causal: env.x.contains_key("correlation") || env.x.contains_key("parent"),
        principal: env.p,
        via,
        scopes: env.s,
        issued_at_ms: env.t,
        attestation,
        mandate: env.m,
        resource: env.r,
        envelope_id: env.n,
    }))
}

/// [`verify`] at a **receive boundary** — the one place a received request is checked against the
/// replay window (post-360 row E). Call it exactly once per received frame (`rpc_rx`, the SDK serve
/// stream, `llm.invoke`, the explain responder); everything that re-reads the request later calls
/// [`verify`], which keeps no state. [`CallerError::Replayed`] must be dropped without an answer.
pub(crate) fn verify_received(ctx: &TaskCtx, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
    let verified = verify(ctx, req).inspect_err(|e| {
        if e.unanswered() {
            ctx.caller_replay.count_replay();
        }
    })?;
    if let Some(c) = &verified {
        ctx.caller_replay.admit(c, ctx.hlc.decision_now_ms(), ctx.config.max_clock_drift_ms)?;
        // X4: counted here, once per request, after the envelope verified — a forged one moves nothing.
        if c.misplaced_causal {
            ctx.note_misplaced_causal("caller_envelope");
        }
    }
    Ok(verified)
}

/// Every gateway's recent envelope ids, as one provider has received them (post-360 row E). Kept only
/// for **signed** envelopes: without a `tls` identity an envelope can be forged outright, and a window
/// keyed on forgeable gateways would be memory anyone could fill. Each window holds at most
/// [`REPLAY_WINDOW_PER_GATEWAY`] ids, and a window left empty past the freshness bound — a gateway that
/// went quiet, left or was removed — is dropped by a sweep every [`REPLAY_SWEEP_EVERY`] admissions.
#[derive(Default)]
pub(crate) struct ReplayWindows {
    by_gateway: papaya::HashMap<NodeId, std::sync::Arc<parking_lot::Mutex<ReplayWindow>>>,
    /// Signed envelopes accepted without an id from a gateway that has not published the id marker —
    /// a pre-row-E gateway's, unprotected inside the window. `GET /stats` `caller_envelopes_without_id`.
    pub(crate) without_id: std::sync::atomic::AtomicU64,
    /// Envelopes dropped as replays — repeated, misaddressed, or with the id stripped.
    /// `GET /stats` `caller_envelopes_replayed`.
    pub(crate) replayed: std::sync::atomic::AtomicU64,
    /// Admissions since the last sweep.
    since_sweep: std::sync::atomic::AtomicU64,
}

/// How many admissions pass between sweeps of empty windows.
pub(crate) const REPLAY_SWEEP_EVERY: u64 = 4_096;

/// One gateway's envelopes inside the freshness window, ordered by `(issued_at_ms, id)`, and the
/// floor below which an envelope is refused because an eviction forgot what was there.
#[derive(Default)]
struct ReplayWindow {
    seen: std::collections::BTreeSet<(u64, u64)>,
    floor: Option<(u64, u64)>,
}

impl ReplayWindow {
    /// Forget what `verify` would refuse as stale anyway; a floor below the horizon refuses nothing
    /// `verify` would not, so it goes too.
    fn prune(&mut self, horizon: u64) {
        while self.seen.first().is_some_and(|&(t, _)| t < horizon) {
            self.seen.pop_first();
        }
        if self.floor.is_some_and(|(t, _)| t < horizon) {
            self.floor = None;
        }
    }
}

impl ReplayWindows {
    /// Admit `caller`'s envelope once. Unsigned envelopes pass untouched (see the type's doc); a signed
    /// one without an id passes and is counted (`verify` has already refused one from a gateway that
    /// issues ids).
    pub(crate) fn admit(&self, caller: &GatewayCaller, now_ms: u64, drift_ms: u64) -> Result<(), CallerError> {
        use std::sync::atomic::Ordering;
        if !matches!(caller.attestation, CallerAttestation::Signed { .. }) {
            return Ok(());
        }
        let Some(id) = caller.envelope_id else {
            self.without_id.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        let key = (caller.issued_at_ms, id);
        let horizon = now_ms.saturating_sub(CALLER_CONTEXT_MAX_AGE_MS.saturating_add(drift_ms));
        let outcome = loop {
            let window = std::sync::Arc::clone(self.by_gateway.pin().get_or_insert_with(caller.via.clone(), Default::default));
            let mut w = window.lock();
            // A sweep may have removed this window between the lookup and the lock; an entry added to
            // a removed window would be forgotten, so look again.
            if !self.by_gateway.pin().get(&caller.via).is_some_and(|cur| std::sync::Arc::ptr_eq(cur, &window)) {
                continue;
            }
            w.prune(horizon);
            if w.floor.is_some_and(|f| key <= f) || !w.seen.insert(key) {
                break Err(CallerError::Replayed { issued_at_ms: caller.issued_at_ms, id });
            }
            if w.seen.len() > REPLAY_WINDOW_PER_GATEWAY
                && let Some(oldest) = w.seen.pop_first()
            {
                w.floor = Some(w.floor.map_or(oldest, |f| f.max(oldest)));
            }
            break Ok(());
        };
        if outcome.is_err() {
            self.replayed.fetch_add(1, Ordering::Relaxed);
        }
        if self.since_sweep.fetch_add(1, Ordering::Relaxed) + 1 >= REPLAY_SWEEP_EVERY {
            self.since_sweep.store(0, Ordering::Relaxed);
            self.sweep(horizon);
        }
        outcome
    }

    /// Count a replay refused before the window (misaddressed, or with the id stripped).
    pub(crate) fn count_replay(&self) {
        self.replayed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Drop every window left empty at `horizon`, one window lock at a time.
    fn sweep(&self, horizon: u64) {
        let windows: Vec<(NodeId, std::sync::Arc<parking_lot::Mutex<ReplayWindow>>)> =
            self.by_gateway.pin().iter().map(|(k, v)| (k.clone(), std::sync::Arc::clone(v))).collect();
        for (via, window) in windows {
            let mut w = window.lock();
            w.prune(horizon);
            if w.seen.is_empty() && w.floor.is_none() {
                // Under the window's lock, and only if the map still holds this window: an admission
                // that fetched it waits on the lock and then finds it gone, and looks again.
                self.by_gateway.pin().remove_if(&via, |_, cur| std::sync::Arc::ptr_eq(cur, &window)).ok();
            }
        }
    }

    /// How many gateways have a window (tests).
    #[cfg(test)]
    pub(crate) fn gateways(&self) -> usize {
        self.by_gateway.len()
    }
}

/// Resolve the principal `req` should be authorised as. `Err` means refuse.
pub(crate) fn request_principal(ctx: &TaskCtx, req: &RpcRequest) -> Result<RequestPrincipal, CallerError> {
    request_principal_and_mandate(ctx, req).map(|(p, _)| p)
}

/// [`request_principal`] plus the mandate the request **carried** (not verified), whichever
/// principal it resolves to. A member's own mandated direct call (`rpc_call_with_mandate`) frames a
/// self envelope with the mandate; mapping that to [`RequestPrincipal::Node`] is right for
/// authorisation but used to drop the mandate on the floor, so a tool that commits at a
/// destination could not compose its effect on that path (`composed-effect.md` §9).
pub(crate) fn request_principal_and_mandate(
    ctx: &TaskCtx,
    req: &RpcRequest,
) -> Result<(RequestPrincipal, Option<serde_json::Value>), CallerError> {
    let verified = verify(ctx, req)?;
    let carried = verified.as_ref().and_then(|c| c.mandate.clone());
    Ok((principal_of(req, verified), carried))
}

/// [`request_principal_and_mandate`] at a **receive boundary**: through [`verify_received`], so the
/// replay window sees the request once. For a loop that reads raw signals itself (the MCP tool loop).
pub(crate) fn received_principal_and_mandate(
    ctx: &TaskCtx,
    req: &RpcRequest,
) -> Result<(RequestPrincipal, Option<serde_json::Value>), CallerError> {
    let verified = verify_received(ctx, req)?;
    let carried = verified.as_ref().and_then(|c| c.mandate.clone());
    Ok((principal_of(req, verified), carried))
}

fn principal_of(req: &RpcRequest, verified: Option<GatewayCaller>) -> RequestPrincipal {
    match verified {
        // A verified self envelope (`node:{via}`, via == sender) is the sending node's own action.
        //
        // NOTE (Phase-C audit): requiring `CallerAttestation::Signed` here was tried and reverted —
        // on a mesh with no `tls` identity a node's own self envelope is legitimately unsigned, and
        // refusing it broke `a_promising_node_is_never_the_bare_sender`. The impersonation route the
        // audit found is closed where it actually opens: a client can no longer *supply* a frame,
        // because the raw-emission routes refuse a payload carrying one
        // (`carries_caller_frame`). This mapping is only reachable from a peer node's own frame.
        Some(c) if c.principal == node_principal(&c.via) => RequestPrincipal::Node(c.via),
        Some(c) => RequestPrincipal::Client(c),
        None => RequestPrincipal::Node(req.sender().clone()),
    }
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
    ///
    /// Stateless: it can be called any number of times on a request, and it does **not** check the
    /// replay window. A request from [`rpc_rx`](crate::ServiceHandle::rpc_rx) has already been
    /// through the window; a loop that builds requests from raw signals itself
    /// (`mesh().signal_rx(kind)` + [`RpcRequest::from`]) calls [`receive_gateway_caller`](Self::receive_gateway_caller) instead.
    pub fn gateway_caller(&self, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
        verify(&self.task_ctx, req)
    }

    /// [`gateway_caller`](Self::gateway_caller) **at a receive boundary** (post-360 row E): verifies
    /// `req` and admits its envelope against this node's replay window, so a captured envelope is
    /// served once. Call it **exactly once per received signal** — a second call on the same request
    /// is refused as a replay — and drop the request without answering when the error
    /// [`unanswered`](CallerError::unanswered) says so. [`rpc_rx`](crate::ServiceHandle::rpc_rx) does
    /// this for you.
    pub fn receive_gateway_caller(&self, req: &RpcRequest) -> Result<Option<GatewayCaller>, CallerError> {
        verify_received(&self.task_ctx, req)
    }

    /// The principal `req` should be authorised as — the gateway client when a verified
    /// context is present, otherwise the sending node. `Err` means refuse.
    pub fn request_principal(&self, req: &RpcRequest) -> Result<RequestPrincipal, CallerError> {
        request_principal(&self.task_ctx, req)
    }

    /// The mandate presented with `req`, **as carried and not yet verified** (Boundary H closure
    /// plan C2): from a gateway client's `params._meta.mandate`, or from a member's
    /// [`rpc_call_with_mandate`](crate::ServiceHandle::rpc_call_with_mandate). `Ok(None)`: none was
    /// presented. `Err`: the caller context itself failed verification, so refuse.
    ///
    /// Verify it before relying on it: the grant's holder must be the request's principal, and the
    /// grant and its possession proof must verify (P2), and the appointment must be current (A1).
    pub fn presented_mandate(&self, req: &RpcRequest) -> Result<Option<serde_json::Value>, CallerError> {
        Ok(verify(&self.task_ctx, req)?.and_then(|c| c.mandate))
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

/// `framed` with its envelope's id, target and v2 signature removed — what a replayer reaching for the
/// pre-row-E path sends (tests).
#[cfg(test)]
pub(crate) fn strip_envelope_id(framed: &Bytes) -> Bytes {
    let Frame::Framed { envelope, app } = split_frame(framed) else { panic!("framed") };
    let mut env: Envelope = serde_json::from_slice(&envelope).expect("an envelope");
    (env.n, env.o, env.sn) = (None, None, None);
    let env = serde_json::to_vec(&env).expect("serialises");
    let mut buf = BytesMut::new();
    buf.put_slice(&FRAME_MAGIC);
    buf.put_u16(env.len() as u16);
    buf.put_slice(&env);
    buf.put(app);
    buf.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use bytes::Bytes;

    /// **A client-supplied caller-context frame is detected where one would be read.**
    ///
    /// A raw emission carries the client's bytes verbatim with the gateway as sender, so a frame the
    /// client prepended verifies as the gateway's own envelope. The pre-existing guard only caught
    /// *bare* bytes. Phase-C audit finding.
    #[test]
    fn a_client_supplied_caller_frame_is_detected() {
        // 8-byte nonce, then the frame magic — exactly where `split_frame` looks.
        let mut forged = vec![0u8; 8];
        forged.extend_from_slice(&FRAME_MAGIC_PREFIX);
        forged.extend_from_slice(b"{\"v\":1}");
        assert!(carries_caller_frame(&forged), "a prepended envelope must be refused, not emitted");

        // Ordinary payloads are untouched: bare bytes, and bytes that merely contain the magic
        // somewhere that is not the frame position.
        assert!(!carries_caller_frame(&[0u8; 8]));
        assert!(!carries_caller_frame(b"a short body"));
        let mut elsewhere = vec![0u8; 20];
        elsewhere.extend_from_slice(&FRAME_MAGIC_PREFIX);
        assert!(!carries_caller_frame(&elsewhere), "the magic only counts at the frame position");
    }

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

    fn raw_frame(env: &[u8], app: &[u8]) -> Bytes {
        let mut f = FRAME_MAGIC.to_vec();
        f.extend_from_slice(&(env.len() as u16).to_be_bytes());
        f.extend_from_slice(env);
        f.extend_from_slice(app);
        Bytes::from(f)
    }

    #[test]
    fn granted_scopes_never_exceed_the_credential() {
        assert_eq!(granted_scopes(&["*".into()], Some("mcp:invoke")), vec!["mcp:invoke".to_string()]);
        assert_eq!(
            granted_scopes(&["kv:read".into(), "mcp:invoke".into()], Some("mcp:invoke")),
            vec!["mcp:invoke".to_string()]
        );
        assert!(granted_scopes(&["kv:read".into()], Some("mcp:invoke")).is_empty());
        assert!(granted_scopes(&["*".into()], None).is_empty());
    }

    /// Review finding 4: three outcomes, never two. A recognised frame that is malformed is
    /// `Malformed`, not "unframed and therefore the node".
    #[test]
    fn split_frame_distinguishes_unframed_framed_and_malformed() {
        let plain = Bytes::from_static(b"{\"jsonrpc\":\"2.0\"}");
        assert_eq!(split_frame(&plain), Frame::Unframed(plain.clone()));
        assert_eq!(split_frame(&Bytes::new()), Frame::Unframed(Bytes::new()));
        // A well-formed frame.
        let good = raw_frame(b"{}", b"app");
        assert!(matches!(split_frame(&good), Frame::Framed { app, .. } if app == Bytes::from_static(b"app")));
        // Truncated header: magic present, no length.
        let mut t = FRAME_MAGIC.to_vec(); t.push(0x00);
        assert!(matches!(split_frame(&Bytes::from(t)), Frame::Malformed(_)));
        // Declared length beyond the bound.
        let mut big = FRAME_MAGIC.to_vec(); big.extend_from_slice(&[0xff, 0xff]); big.extend_from_slice(b"x");
        assert!(matches!(split_frame(&Bytes::from(big)), Frame::Malformed(_)));
        // Declared length beyond the payload (envelope truncated).
        let mut short = FRAME_MAGIC.to_vec(); short.extend_from_slice(&50u16.to_be_bytes()); short.extend_from_slice(b"{}");
        assert!(matches!(split_frame(&Bytes::from(short)), Frame::Malformed(_)));
        // Unsupported version.
        let mut v2 = FRAME_MAGIC_PREFIX.to_vec(); v2.push(2); v2.extend_from_slice(&2u16.to_be_bytes()); v2.extend_from_slice(b"{}x");
        assert!(matches!(split_frame(&Bytes::from(v2)), Frame::Malformed(_)));
    }

    /// **Closure plan C2.** A presented mandate travels in the envelope and comes out of `verify`
    /// exactly as sent; an envelope without one carries no `m` key at all, so it is byte-for-byte
    /// what it was before C2 and an older provider sees nothing new.
    #[tokio::test]
    async fn a_mandate_travels_in_the_envelope_and_one_without_is_unchanged() {
        let a = agent(false);
        let mandate = serde_json::json!({"grant": {"mandate": {"holder": "token:gw/a"}, "signature": [1, 2, 3]},
                                         "possession": "cHJvb2Y="});
        let framed = frame_with_context_and_mandate(&a.task_ctx, a.node_id(), "token:gw/a", &[], Bytes::from_static(b"app"), Some(&mandate), Some("skill:depot/dispatch@x")).unwrap();
        let req = request_from(a.node_id(), "skill.invoke", framed);
        let c = verify(&a.task_ctx, &req).unwrap().unwrap();
        assert_eq!(c.mandate, Some(mandate), "carried exactly as presented");
        assert_eq!(c.resource.as_deref(), Some("skill:depot/dispatch@x"), "the named resource travels too (C3)");
        assert_eq!(req.payload(), Bytes::from_static(b"app"), "the application payload is untouched");

        let plain = frame_with_context(&a.task_ctx, a.node_id(), "token:gw/a", &[], Bytes::from_static(b"app")).unwrap();
        let Frame::Framed { envelope, .. } = split_frame(&plain) else { panic!("framed") };
        let env: serde_json::Value = serde_json::from_slice(&envelope).unwrap();
        assert!(env.get("m").is_none(), "no mandate, no key: the pre-C2 envelope, unchanged");
        let req = request_from(a.node_id(), "skill.invoke", plain);
        assert_eq!(verify(&a.task_ctx, &req).unwrap().unwrap().mandate, None);
    }

    /// A mandate that would push the envelope over `MAX_ENVELOPE_BYTES` is refused at the producer,
    /// never truncated (the same rule as an oversized principal).
    #[tokio::test]
    async fn an_oversized_mandate_is_refused_not_truncated() {
        let a = agent(false);
        let huge = serde_json::json!({"grant": "x".repeat(MAX_ENVELOPE_BYTES), "possession": ""});
        assert!(frame_with_context_and_mandate(&a.task_ctx, a.node_id(), "token:gw/a", &[], Bytes::new(), Some(&huge), None).is_none());
    }

    /// **A member acting under its own mandate** calls a provider directly: the provider sees the
    /// member itself as the principal (`RequestPrincipal::Node`) and the mandate it presented.
    #[tokio::test]
    async fn a_member_call_carries_its_mandate_as_the_node() {
        let (pa, pb) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
        let (ida, idb) = (NodeId::new("127.0.0.1", pa).unwrap(), NodeId::new("127.0.0.1", pb).unwrap());
        let mut ca = GossipConfig::default();
        ca.bind_port = pa;
        ca.bootstrap_peers = vec![idb.clone()];
        let mut cb = GossipConfig::default();
        cb.bind_port = pb;
        cb.bootstrap_peers = vec![ida.clone()];
        let (a, b) = (Arc::new(GossipAgent::new(ida, ca)), Arc::new(GossipAgent::new(idb, cb)));
        a.start().await.unwrap();
        b.start().await.unwrap();
        let mandate = serde_json::json!({"grant": {"mandate": {"holder": format!("node:{}", a.node_id())}}, "possession": "cA=="});
        type Seen = Option<(RequestPrincipal, Option<serde_json::Value>)>;
        let seen: Arc<std::sync::Mutex<Seen>> = Arc::default();
        {
            let (b2, seen) = (Arc::clone(&b), Arc::clone(&seen));
            let mut rx = b.service().rpc_rx("depot.dispatch");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    *seen.lock().unwrap() = Some((b2.request_principal(&req).unwrap(), b2.presented_mandate(&req).unwrap()));
                    b2.service().rpc_respond(&req, b"ok".to_vec());
                }
            });
        }
        let mut reply = None;
        for _ in 0..50 {
            if let Ok(r) = a.service().rpc_call_with_mandate(b.node_id().clone(), "depot.dispatch", b"go".to_vec(), &mandate, &format!("depot:dispatch@{}", b.node_id()), std::time::Duration::from_millis(500)).await {
                reply = Some(r);
                break;
            }
        }
        assert_eq!(reply.as_deref(), Some(&b"ok"[..]), "the call completes");
        let (principal, carried) = seen.lock().unwrap().clone().expect("the provider was reached");
        assert!(matches!(principal, RequestPrincipal::Node(ref n) if n == a.node_id()), "the member is the principal: {principal:?}");
        assert_eq!(carried, Some(mandate));
        a.shutdown().await;
        b.shutdown().await;
    }

    /// Review finding 4 (the parser downgrade): every malformed shape is refused by `verify`
    /// and yields an **empty** application payload — never admitted as the node's own call.
    #[tokio::test]
    async fn malformed_frames_are_refused_never_treated_as_the_node() {
        let a = agent(false);
        let node_allow: Vec<Arc<str>> = vec![Arc::from(a.node_id().to_string().as_str())];
        let cases: Vec<(&str, Bytes)> = vec![
            ("truncated header", { let mut t = FRAME_MAGIC.to_vec(); t.push(0); Bytes::from(t) }),
            ("oversized length", { let mut b = FRAME_MAGIC.to_vec(); b.extend_from_slice(&[0xff, 0xff]); b.extend_from_slice(b"x"); Bytes::from(b) }),
            ("truncated envelope", { let mut b = FRAME_MAGIC.to_vec(); b.extend_from_slice(&50u16.to_be_bytes()); b.extend_from_slice(b"{}"); Bytes::from(b) }),
            ("unsupported version", { let mut b = FRAME_MAGIC_PREFIX.to_vec(); b.push(9); b.extend_from_slice(&2u16.to_be_bytes()); b.extend_from_slice(b"{}"); Bytes::from(b) }),
            ("malformed json", raw_frame(b"not json", b"payload")),
        ];
        for (name, framed) in cases {
            let req = request_from(a.node_id(), "k", framed);
            assert!(matches!(verify(&a.task_ctx, &req), Err(CallerError::Malformed(_))), "{name}: must be Malformed");
            if name != "malformed json" {
                // A frame that fails structurally has no defined application payload; a
                // well-formed frame whose envelope is bad JSON does (and is still refused).
                assert!(req.payload().is_empty(), "{name}: no application payload is defined");
            }
            assert!(request_principal(&a.task_ctx, &req).is_err(), "{name}: never a principal");
            #[cfg(feature = "compliance")]
            assert!(a.request_authorized(&req, &node_allow).is_err(), "{name}: never admitted as the node");
            #[cfg(not(feature = "compliance"))]
            let _ = &node_allow;
        }
    }

    /// Review finding 4 (the producer side): an envelope over the bound is refused before
    /// dispatch, never truncated into something a receiver would misread.
    #[tokio::test]
    async fn producer_refuses_an_oversized_envelope() {
        let a = agent(false);
        let long = format!("oidc:{}", "a".repeat(9_000));
        assert!(frame_with_context(&a.task_ctx, a.node_id(), &long, &[], Bytes::from_static(b"x")).is_none(),
            "a 9 KB principal does not fit the 8 KiB envelope bound");
        assert!(frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mesh:write".into()], Bytes::from_static(b"x")).is_some());
    }

    #[tokio::test]
    async fn context_round_trips_and_payload_is_stripped() {
        let a = agent(false);
        let app = Bytes::from_static(b"hello");
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "token:gw/#0", &["mcp:invoke".into()], app.clone()).unwrap();
        let req = request_from(a.node_id(), "k", framed);
        assert_eq!(req.payload(), app, "the provider sees exactly the application bytes");
        assert!(req.has_caller_context());
        let c = verify(&a.task_ctx, &req).unwrap().expect("a context");
        assert_eq!(c.principal, "token:gw/#0");
        assert_eq!(c.scopes, vec!["mcp:invoke".to_string()]);
        assert_eq!(&c.via, a.node_id());
        assert_eq!(c.attestation, CallerAttestation::UnauthenticatedMesh, "no tls identity ⇒ unsigned");
        match request_principal(&a.task_ctx, &req).unwrap() {
            RequestPrincipal::Client(c2) => assert_eq!(c2, c),
            other => panic!("expected a client principal, got {other:?}"),
        }
        // No envelope from a node that promises none ⇒ the node itself.
        let bare = request_from(a.node_id(), "k", app.clone());
        assert!(!bare.has_caller_context());
        assert_eq!(request_principal(&a.task_ctx, &bare).unwrap(), RequestPrincipal::Node(a.node_id().clone()));
    }

    /// Review finding 1: a node that promises envelopes (a secure-profile gateway) is judged
    /// only by them — its own RPCs carry a self envelope that maps back to `Node`; a bare
    /// RPC-shaped frame from it (what a raw `/gateway/signal/emit` produces) is `Missing`, never
    /// the node's own action.
    #[tokio::test]
    async fn a_promising_node_is_never_the_bare_sender() {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(crate::test_util::alloc_port()); // a gateway node, secure by default
        let g = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        assert!(promises_envelopes(&g.task_ctx.config));
        assert_eq!(marker_value(&g.task_ctx.config), MARKER_PROMISES_ENVELOPES);

        // Its own action: the self envelope ⇒ Node.
        let own = frame_self(&g.task_ctx, g.node_id(), Bytes::from_static(b"payload"));
        let req = request_from(g.node_id(), "k", own);
        assert_eq!(req.payload(), Bytes::from_static(b"payload"));
        assert_eq!(request_principal(&g.task_ctx, &req).unwrap(), RequestPrincipal::Node(g.node_id().clone()));

        // A bare frame from it (a raw emission): refused as Missing.
        let mut raw = Vec::new(); raw.extend_from_slice(b"x");
        let req = request_from(g.node_id(), "k", Bytes::from(raw));
        assert_eq!(verify(&g.task_ctx, &req), Err(CallerError::Missing));
        assert!(request_principal(&g.task_ctx, &req).is_err());

        // A non-gateway node promises nothing: bare is its own action.
        let n = agent(false);
        assert!(!promises_envelopes(&n.task_ctx.config));
        assert_eq!(marker_value(&n.task_ctx.config), MARKER_VERIFIES);
    }

    #[tokio::test]
    async fn via_must_be_the_frame_sender() {
        let a = agent(false);
        let framed = frame_with_context(&a.task_ctx, a.node_id(), PRINCIPAL_ANONYMOUS, &[], Bytes::from_static(b"x")).unwrap();
        let other = NodeId::new("127.0.0.1", 1).unwrap();
        let req = request_from(&other, "k", framed);
        assert!(matches!(verify(&a.task_ctx, &req), Err(CallerError::ViaMismatch { .. })));
    }

    /// Review finding 2: gateway-local names are qualified by the issuing gateway, so two
    /// gateways' first tokens are distinct identities, and only an explicit shared issuer makes
    /// them one. OIDC subjects are qualified by the IdP issuer.
    #[test]
    fn principals_are_qualified_by_their_issuer() {
        let g1 = NodeId::new("127.0.0.1", 9001).unwrap();
        let g2 = NodeId::new("127.0.0.1", 9002).unwrap();
        let mk = |via: &NodeId, p: String| GatewayCaller {
            misplaced_causal: false,
            principal: p, via: via.clone(), scopes: vec![], issued_at_ms: 0,
            attestation: CallerAttestation::UnauthenticatedMesh, mandate: None, resource: None, envelope_id: None,
        };
        let a = mk(&g1, positional_token_principal(&g1.to_string(), 0));
        let b = mk(&g2, positional_token_principal(&g2.to_string(), 0));
        assert_ne!(a.principal, b.principal, "the same list position on two gateways is two identities");
        let allow_a: Vec<Arc<str>> = vec![Arc::from(a.principal.as_str())];
        assert!(client_admitted(&allow_a, &a));
        assert!(!client_admitted(&allow_a, &b), "gateway 2's first token is not gateway 1's");
        // A deliberately shared issuer is one namespace — explicit configuration, not inference.
        let s1 = mk(&g1, named_token_principal("fleet-gw", "ci-bot"));
        let s2 = mk(&g2, named_token_principal("fleet-gw", "ci-bot"));
        assert_eq!(s1.principal, s2.principal);
        // Named tokens survive reordering; positional ones do not — by construction of the name.
        assert_eq!(named_token_principal("gw", "ci-bot"), "token:gw/ci-bot");
        assert_eq!(legacy_token_principal("gw"), "token:gw/legacy");
        assert_ne!(oidc_principal("https://idp-a", "alice"), oidc_principal("https://idp-b", "alice"));
        // Listing the gateway node admits nothing (unchanged).
        assert!(!client_admitted(&[Arc::from(g1.to_string().as_str())], &a));
        assert!(client_admitted(&[], &a), "empty allowlist is open");
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn tls_signed_context_verifies_and_forgeries_fail() {
        let a = agent(true);
        a.start().await.unwrap();
        let app = Bytes::from_static(b"payload");
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mesh:write".into()], app.clone()).unwrap();
        let req = request_from(a.node_id(), "k", framed.clone());
        let c = verify(&a.task_ctx, &req).unwrap().expect("context");
        let own = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
        assert_eq!(c.attestation, CallerAttestation::Signed { signer: own });

        // Tampered payload: the digest no longer matches.
        let Frame::Framed { envelope, .. } = split_frame(&framed) else { panic!() };
        let req = request_from(a.node_id(), "k", raw_frame(&envelope, b"PAYLOAD"));
        assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::BadSignature));

        // Unsigned envelope on an authenticated node: refused.
        let unsigned = serde_json::to_vec(&Envelope {
            v: 1, p: "oidc:idp/mallory".into(), via: a.node_id().to_string(), s: vec![], t: 0, k: None, sig: None,
            m: None,
            r: None, n: None, o: None, sn: None, x: Default::default(),
        }).unwrap();
        let req = request_from(a.node_id(), "k", raw_frame(&unsigned, b""));
        assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::Unsigned));

        // Signed by a key nobody trusts for `via`: refused.
        {
            use base64::Engine as _;
            let rogue = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
            let msg = signed_message("oidc:idp/mallory", &a.node_id().to_string(), &[], 0, &digest(b"payload"));
            let sig = crate::tls::sign_bytes(&rogue, &msg);
            let forged = serde_json::to_vec(&Envelope {
                v: 1, p: "oidc:idp/mallory".into(), via: a.node_id().to_string(), s: vec![], t: 0,
                k: Some(base64::engine::general_purpose::STANDARD.encode(rogue.verifying_key().to_bytes())),
                sig: Some(base64::engine::general_purpose::STANDARD.encode(sig)),
                m: None,
                r: None, n: None, o: None, sn: None, x: Default::default(),
            }).unwrap();
            let req = request_from(a.node_id(), "k", raw_frame(&forged, b"payload"));
            assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::UnknownSigner));
        }
        a.shutdown().await;
    }

    /// Post-360 row E: the envelope binds freshness. Its issue time was signed but never read, so a
    /// captured envelope and payload verified for ever. Now `verify` refuses one issued further back
    /// than [`CALLER_CONTEXT_MAX_AGE_MS`] plus the node's drift bound, or further ahead than the drift
    /// bound — signed (the time is the gateway's) and unsigned (the time is carried) alike.
    #[tokio::test]
    async fn a_stale_or_future_envelope_is_refused() {
        for tls in [false, true] {
            #[cfg(not(feature = "tls"))]
            if tls { continue; }
            let a = agent(tls);
            a.start().await.unwrap();
            let now = a.task_ctx.hlc.decision_now_ms();
            let drift = a.task_ctx.config.max_clock_drift_ms;
            let app = Bytes::from_static(b"payload");
            let at = |t: u64| {
                let framed = frame_with_context_at(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mesh:write".into()], app.clone(), None, None, t).unwrap();
                verify(&a.task_ctx, &request_from(a.node_id(), "k", framed)).map(|c| c.map(|c| c.issued_at_ms)).map_err(|e| e.code())
            };
            assert_eq!(at(now), Ok(Some(now)), "tls={tls}: a fresh envelope verifies");
            assert_eq!(at(now - 60_000), Ok(Some(now - 60_000)), "tls={tls}: a minute in flight is fine");
            assert_eq!(at(now - 2 * 3_600_000 - drift), Err("stale"), "tls={tls}: two hours old");
            assert_eq!(at(now + drift + 60_000), Err("stale"), "tls={tls}: from beyond the drift bound");
            assert_eq!(at(0), Err("stale"), "tls={tls}: issued at the epoch");
            a.shutdown().await;
        }
    }

    /// #613's review, finding 1: `max_clock_drift_ms = 0` means *no drift bound* (`hlc.rs`,
    /// `configuration.md`), and RPC frames do not make the provider observe the sender's clock — so a
    /// provider a few ms behind the gateway refused nearly every envelope as `stale`. With 0 there is no
    /// future bound, and the age bound is `CALLER_CONTEXT_MAX_AGE_MS` alone.
    #[tokio::test]
    async fn a_zero_drift_bound_bounds_only_the_age() {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.max_clock_drift_ms = 0;
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        a.start().await.unwrap();
        let now = a.task_ctx.hlc.decision_now_ms();
        let app = Bytes::from_static(b"payload");
        let at = |t: u64| {
            let framed = frame_with_context_at(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], app.clone(), None, None, t).unwrap();
            verify(&a.task_ctx, &request_from(a.node_id(), "k", framed)).map(|c| c.is_some()).map_err(|e| e.code())
        };
        assert_eq!(at(now + 50), Ok(true), "a gateway 50 ms ahead of this provider");
        assert_eq!(at(now + CALLER_CONTEXT_MAX_AGE_MS - 1_000), Ok(true), "ahead, inside the age bound");
        assert_eq!(at(now + 3_600_000), Err("stale"), "far ahead: with no drift bound the future is bounded by the age bound");
        assert_eq!(at(now - CALLER_CONTEXT_MAX_AGE_MS + 1_000), Ok(true), "inside the age bound");
        assert_eq!(at(now - CALLER_CONTEXT_MAX_AGE_MS - 1_000), Err("stale"), "the age bound alone");
        a.shutdown().await;
    }

    /// Review finding 3: the built-in LLM provider refuses an unsigned context on a `tls` node
    /// instead of stripping it and executing.
    #[cfg(all(feature = "tls", feature = "llm"))]
    #[tokio::test]
    async fn llm_provider_refuses_an_unverified_context() {
        let a = agent(true);
        a.start().await.unwrap();
        let handle = a.llm().register_prompt_skill("review", "echo", crate::PromptTemplate {
            system: "".into(), user_template: "{{input}}".into(), max_tokens: 32, temperature: 0.0, metadata: Default::default(),
        }, Arc::new(crate::EchoBackend)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let env = serde_json::to_vec(&Envelope {
            v: 1, p: "oidc:idp/admin".into(), via: a.node_id().to_string(), s: vec![], t: 0, k: None, sig: None,
            m: None,
            r: None, n: None, o: None, sn: None, x: Default::default(),
        }).unwrap();
        let bytes = raw_frame(&env, br#"{"prompt":"review/echo","input":"unverified call","context":{}}"#);
        let req = request_from(a.node_id(), "llm.invoke", bytes.clone());
        assert_eq!(verify(&a.task_ctx, &req), Err(CallerError::Unsigned));
        // The real dispatch: `rpc_call_framed` sends the forged bytes as-is (the node's own
        // `rpc_call` would wrap a self envelope around them, which is the point).
        let reply = super::super::rpc::rpc_call_framed(&a.task_ctx, a.node_id().clone(), Arc::from("llm.invoke"), bytes, std::time::Duration::from_secs(3)).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(v["error"], "caller_context_refused", "{v}");
        assert!(v.get("output").is_none(), "the backend must not run");
        drop(handle);
        a.shutdown().await;
    }

    /// Re-frame `framed`'s envelope with `edit` applied, keeping the application bytes.
    #[cfg(feature = "tls")]
    fn reframe(framed: &Bytes, edit: impl FnOnce(&mut serde_json::Value)) -> Bytes {
        let Frame::Framed { envelope, app } = split_frame(framed) else { panic!("framed") };
        let mut v: serde_json::Value = serde_json::from_slice(&envelope).unwrap();
        edit(&mut v);
        raw_frame(&serde_json::to_vec(&v).unwrap(), &app)
    }

    /// **A caller envelope is served once** (post-360 row E: the nonce cache). The freshness bound
    /// alone let a captured envelope and its payload be served again for five minutes — re-emitted under
    /// a fresh RPC nonce by anyone who saw it. Each envelope now carries an id, signed with the rest,
    /// and a provider's receive boundary admits each `(gateway, issue time, id)` once. The plant: two
    /// calls identical in principal, scopes, payload **and issue time** are both admitted — they differ
    /// in id — so the window keys on the envelope, not on its contents. Seen failing first: the replay
    /// was admitted.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn a_replayed_envelope_is_refused_at_the_receive_boundary() {
        let a = agent(true);
        a.start().await.unwrap();
        let app = Bytes::from_static(b"payload");
        let t = a.task_ctx.hlc.decision_now_ms();
        let first = frame_with_context_at(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mesh:write".into()], app.clone(), None, None, t).unwrap();
        let twin = frame_with_context_at(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mesh:write".into()], app.clone(), None, None, t).unwrap();
        let receive = |f: &Bytes| verify_received(&a.task_ctx, &request_from(a.node_id(), "k", f.clone())).map(|_| ()).map_err(|e| e.code());
        assert_eq!(receive(&first), Ok(()), "the first receipt is served");
        assert_eq!(receive(&twin), Ok(()), "plant: an identical call at the same instant is another envelope");
        assert_eq!(receive(&first), Err("replayed"), "REPLAY: the same envelope was served twice");
        // `verify` keeps no state: a handler re-reading the request it was handed is not a replay.
        assert!(verify(&a.task_ctx, &request_from(a.node_id(), "k", first.clone())).is_ok());
        assert_eq!(a.task_ctx.caller_replay.replayed.load(std::sync::atomic::Ordering::Relaxed), 1);
        a.shutdown().await;
    }

    /// **The id and the target are signed, so a replay can be neither re-numbered nor re-addressed;
    /// stripping them is refused from a gateway that issues ids** (post-360 row E and its review). A
    /// changed id fails the v2 signature; an id without its target or signature is malformed; an
    /// envelope addressed to another node is misaddressed; and an envelope with the id stripped, in the
    /// name of a gateway that issues ids (here this node itself), is a replay — each dropped unanswered.
    /// The v1 signature is still produced, so a provider that predates the id verifies a new gateway's
    /// envelope. Seen failing first: a re-numbered replay was admitted (id unsigned), and a stripped one
    /// was served twice (before the id marker).
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn an_envelope_id_and_target_are_signed_and_stripping_them_is_refused() {
        let a = agent(true);
        a.start().await.unwrap();
        let app = Bytes::from_static(b"payload");
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], app.clone()).unwrap();
        let receive = |f: Bytes| verify_received(&a.task_ctx, &request_from(a.node_id(), "k", f)).map(|_| ()).map_err(|e| e.code());
        assert_eq!(receive(framed.clone()), Ok(()));
        let renumbered = reframe(&framed, |v| { v["n"] = serde_json::json!(v["n"].as_u64().unwrap() + 1); });
        assert_eq!(receive(renumbered), Err("bad_signature"), "REPLAY: a re-numbered envelope verified");
        let readdressed = reframe(&framed, |v| { v["o"] = serde_json::json!("127.0.0.1:1"); });
        assert_eq!(receive(readdressed), Err("bad_signature"), "REPLAY: a re-addressed envelope verified");
        for field in ["sn", "o"] {
            let partial = reframe(&framed, |v| { v.as_object_mut().unwrap().remove(field); });
            assert_eq!(receive(partial), Err("malformed"), "an id without `{field}`");
        }
        let elsewhere = frame_with_context(&a.task_ctx, &NodeId::new("127.0.0.1", 1).unwrap(), "oidc:idp/alice", &[], app.clone()).unwrap();
        assert_eq!(receive(elsewhere), Err("misaddressed"), "REPLAY: an envelope signed for another provider was served here");
        let stripped = reframe(&framed, |v| { let o = v.as_object_mut().unwrap(); o.remove("n"); o.remove("o"); o.remove("sn"); });
        assert_eq!(receive(stripped), Err("id_stripped"), "REPLAY: an id-less envelope from a gateway that issues ids was served");
        assert!(CallerError::IdStripped.unanswered() && CallerError::Misaddressed { to: String::new() }.unanswered());
        assert_eq!(a.task_ctx.caller_replay.replayed.load(std::sync::atomic::Ordering::Relaxed), 2, "both counted as replays");

        // What a provider that predates the id checks: the v1 signature over the v1 fields.
        let Frame::Framed { envelope, .. } = split_frame(&framed) else { panic!() };
        let env: Envelope = serde_json::from_slice(&envelope).unwrap();
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let k: [u8; 32] = b64.decode(env.k.unwrap()).unwrap().try_into().unwrap();
        let sig = b64.decode(env.sig.unwrap()).unwrap();
        assert!(crate::tls::verify_bytes(&k, &signed_message(&env.p, &env.via, &env.s, env.t, &digest(&app)), &sig),
            "the v1 signature a 2.34 provider verifies is still produced");
        a.shutdown().await;
    }

    /// **A causal field in the envelope is never read; one in the payload is under the signature** (adopter-surface
    /// X4). A `parent` added to the envelope beside `m` verifies — the field is unknown to the envelope and sits
    /// outside every signature — and is counted at the door, so the record built from this call carries no parent
    /// from it. A `parent` inside the application payload is covered by the digest the signature binds, so altering
    /// it after signing is refused as any altered byte is. Seen failing first: the counter did not exist.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn a_parent_in_the_envelope_is_counted_not_read_and_one_in_the_payload_is_signed() {
        let a = agent(true);
        a.start().await.unwrap();
        let hash = crate::causal::content_hash(b"a decision");
        let app = Bytes::from(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "greet", "arguments": {}, "_meta": {"parent": hash}}}).to_string());
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], app.clone()).unwrap();
        let receive = |f: Bytes| verify_received(&a.task_ctx, &request_from(a.node_id(), "k", f)).map(|_| ()).map_err(|e| e.code());
        // In the envelope, beside `m`: verifies, counted, never read (the envelope's own fields are unchanged).
        let in_envelope = reframe(&framed, |v| { v["parent"] = serde_json::json!(hash); v["correlation"] = serde_json::json!("run-1"); });
        assert_eq!(receive(in_envelope.clone()), Ok(()), "an unknown envelope field is not a refusal");
        assert_eq!(a.task_ctx.misplaced_causal_counts().get("caller_envelope").copied(), Some(1), "counted against the envelope door, by name");
        // A second verification of the same request (the provider check after the receive boundary) counts nothing more.
        assert!(verify(&a.task_ctx, &request_from(a.node_id(), "k", in_envelope)).is_ok());
        assert_eq!(a.task_ctx.misplaced_causal_counts().get("caller_envelope").copied(), Some(1), "once per request, not per verification");
        // In the payload, altered after signing: refused by the digest the signature covers.
        let Frame::Framed { envelope, .. } = split_frame(&framed) else { panic!("framed") };
        let altered_app = Bytes::from(app.iter().map(|&b| if b == b'a' { b'b' } else { b }).collect::<Vec<u8>>());
        assert_ne!(altered_app, app);
        let altered = raw_frame(&envelope, &altered_app);
        assert_eq!(receive(altered), Err("bad_signature"), "a parent altered after signing verified");
        // A forged envelope moves nothing: the count follows a signature that held.
        let forged = reframe(&framed, |v| { v["parent"] = serde_json::json!(hash); v["sig"] = serde_json::json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="); v.as_object_mut().unwrap().remove("sn"); v.as_object_mut().unwrap().remove("n"); v.as_object_mut().unwrap().remove("o"); });
        assert!(receive(forged).is_err(), "a forged envelope is refused");
        assert_eq!(a.task_ctx.misplaced_causal_counts().get("caller_envelope").copied(), Some(1), "an altered payload or a forgery is not a misplacement");
        a.shutdown().await;
    }

    /// **A window left empty is swept** (post-360 row E's review, finding 4): a gateway that sent a
    /// burst and left kept its window at every provider for good. Every [`REPLAY_SWEEP_EVERY`]
    /// admissions the windows empty at the freshness bound are dropped; a live gateway's is kept.
    #[test]
    fn an_empty_replay_window_is_swept() {
        let w = ReplayWindows::default();
        let (gone, live) = (NodeId::new("127.0.0.1", 9611).unwrap(), NodeId::new("127.0.0.1", 9612).unwrap());
        let caller = |via: &NodeId, t: u64, id: u64| GatewayCaller {
            misplaced_causal: false,
            principal: "p".into(), via: via.clone(), scopes: vec![], issued_at_ms: t,
            attestation: CallerAttestation::Signed { signer: [1u8; 32] }, mandate: None, resource: None, envelope_id: Some(id),
        };
        let t0 = 10_000_000;
        assert!(w.admit(&caller(&gone, t0, 1), t0, 0).is_ok());
        let later = t0 + CALLER_CONTEXT_MAX_AGE_MS + 1;
        for id in 0..REPLAY_SWEEP_EVERY {
            assert!(w.admit(&caller(&live, later, id), later, 0).is_ok());
        }
        assert_eq!(w.gateways(), 1, "the departed gateway's window was swept; the live one kept");
        assert!(matches!(w.admit(&caller(&live, later, 7), later, 0), Err(CallerError::Replayed { .. })), "and still remembers");
    }

    /// **The window is bounded per gateway and fails closed** (post-360 row E). Past
    /// [`REPLAY_WINDOW_PER_GATEWAY`] ids the oldest is evicted and becomes the floor: a replay of it, or
    /// anything older, is refused rather than forgotten; a newer envelope is still served. And ids older
    /// than the freshness bound are pruned, since `verify` refuses those as stale anyway.
    #[test]
    fn the_replay_window_is_bounded_and_fails_closed() {
        let w = ReplayWindows::default();
        let via = NodeId::new("127.0.0.1", 9601).unwrap();
        let caller = |t: u64, id: u64| GatewayCaller {
            misplaced_causal: false,
            principal: "oidc:idp/alice".into(), via: via.clone(), scopes: vec![], issued_at_ms: t,
            attestation: CallerAttestation::Signed { signer: [1u8; 32] }, mandate: None, resource: None, envelope_id: Some(id),
        };
        let now = 10_000_000;
        let t = now - 1_000;
        for id in 0..=REPLAY_WINDOW_PER_GATEWAY as u64 {
            assert!(w.admit(&caller(t, id), now, 0).is_ok());
        }
        assert!(matches!(w.admit(&caller(t, 0), now, 0), Err(CallerError::Replayed { .. })), "the evicted oldest is the floor");
        assert!(matches!(w.admit(&caller(t - 1, 99_999_999), now, 0), Err(CallerError::Replayed { .. })), "anything below the floor");
        assert!(w.admit(&caller(t + 1, 0), now, 0).is_ok(), "a newer envelope is served");
        assert!(matches!(w.admit(&caller(t, 5), now, 0), Err(CallerError::Replayed { .. })), "a kept id is a replay");

        // Pruning: past the freshness bound the window empties, and an unsigned envelope is never kept.
        let later = t + CALLER_CONTEXT_MAX_AGE_MS + 10;
        assert!(w.admit(&caller(later, 1), later + 1, 0).is_ok());
        let kept = w.by_gateway.pin().get(&via).unwrap().lock().seen.len();
        assert_eq!(kept, 1, "everything older than the freshness bound was pruned");
        let mut unsigned = caller(later, 1);
        unsigned.attestation = CallerAttestation::UnauthenticatedMesh;
        assert!(w.admit(&unsigned, later + 1, 0).is_ok(), "an unsigned envelope has no window to fill");
    }

    /// **A replay is dropped unanswered at `rpc_rx`** (post-360 row E). The same signed envelope sent
    /// twice — as an attacker re-emits a captured frame — reaches the serving loop once; the second
    /// gets no reply (an answer under the original's correlation nonce could reach the gateway first),
    /// and is counted. Seen failing first: the serving loop ran twice.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn rpc_rx_drops_a_replay_unanswered() {
        let a = agent(true);
        a.start().await.unwrap();
        let mut rx = a.service().rpc_rx("echo.replay");
        let served = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (ctx, n) = (Arc::clone(&a.task_ctx), Arc::clone(&served));
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                super::super::rpc::rpc_respond_ctx(&ctx, &req, Bytes::from_static(b"ok"));
            }
        });
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], Bytes::from_static(b"go")).unwrap();
        let call = || super::super::rpc::rpc_call_framed(&a.task_ctx, a.node_id().clone(), Arc::from("echo.replay"), framed.clone(), std::time::Duration::from_millis(800));
        assert_eq!(call().await.as_deref(), Ok(&b"ok"[..]), "the first is served");
        let replay = call().await;
        let replayed = a.task_ctx.caller_replay.replayed.load(std::sync::atomic::Ordering::Relaxed);
        let count = served.load(std::sync::atomic::Ordering::SeqCst);
        a.shutdown().await;
        assert_eq!(count, 1, "REPLAY: the serving loop ran the same envelope twice");
        assert_eq!(replay, Err(super::super::rpc::RpcError::Timeout), "the replay is not answered");
        assert_eq!(replayed, 1, "and is counted");
    }

    /// **The MCP tool loop is a receive boundary too** (post-360 row E): it reads raw `mcp.invoke`
    /// signals itself, not through `rpc_rx`, so it admits the envelope against the replay window on its
    /// own. The same signed `tools/call` sent twice runs the tool once and leaves the second unanswered.
    /// Seen failing first: the tool ran twice.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn the_mcp_tool_loop_drops_a_replay_unanswered() {
        let a = agent(true);
        a.start().await.unwrap();
        let ran = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let n = Arc::clone(&ran);
        let _tool = a.mcp().register_mcp_tool("count", serde_json::json!({"type": "object"}), move |_args| {
            let n = Arc::clone(&n);
            async move { n.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok(serde_json::json!("ok")) }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "count", "arguments": {}}});
        let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &["mcp:invoke".into()], Bytes::from(body.to_string())).unwrap();
        let call = || super::super::rpc::rpc_call_framed(&a.task_ctx, a.node_id().clone(),
            Arc::from(crate::signal::signal_kind::MCP_INVOKE), framed.clone(), std::time::Duration::from_millis(800));
        let first = call().await;
        let replay = call().await;
        let count = ran.load(std::sync::atomic::Ordering::SeqCst);
        a.shutdown().await;
        assert!(first.is_ok(), "the first call is answered: {first:?}");
        assert_eq!(count, 1, "REPLAY: the MCP tool ran the same envelope twice");
        assert_eq!(replay, Err(super::super::rpc::RpcError::Timeout), "the replay is not answered");
    }
}
