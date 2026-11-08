//! invoke.bulk — point-to-point RPC for large payloads.
//!
//! Regular [`ServiceHandle::rpc_call`] encodes the entire
//! payload inside the gossip signal, which floods every intermediate node.
//! `bulk_call` stages the payload in a node-local HTTP endpoint and sends
//! only a lightweight ticket (nonce + kind) over the signal mesh;
//! the responder fetches the payload directly from the caller's HTTP server.
//!
//! ## Wire format (INVOKE_BULK signal payload)
//!
//! A caller whose gateway serves plaintext sends the layout every release has read:
//!
//! ```text
//! ┌────────────────┬────────────┬───────────────────┐
//! │ nonce  (8 B LE)│ port (2 LE)│ kind (UTF-8 bytes) │
//! └────────────────┴────────────┴───────────────────┘
//! ```
//!
//! A caller whose gateway serves TLS (`GossipConfig::gateway_tls`, external review 2026-10-31 R7)
//! marks the ticket and names its scheme:
//!
//! ```text
//! ┌────────────────┬────────────┬──────┬───────────┬───────────────────┐
//! │ nonce  (8 B LE)│ port (2 LE)│ 0xFF │ flags (1) │ kind (UTF-8 bytes) │
//! └────────────────┴────────────┴──────┴───────────┴───────────────────┘
//!   flags bit 0 = the caller's gateway serves https
//! ```
//!
//! `0xFF` never begins valid UTF-8, so the two layouts cannot be confused: a responder older than
//! R7 reads a marked ticket's kind as invalid UTF-8 and drops it (the call times out, as every
//! bulk call to or from a TLS gateway did before R7), and an R7 responder reads both. A plaintext
//! fleet therefore sends exactly the bytes it sent before.
//!
//! The caller stages the payload at `GET /bulk/{nonce:016x}` on its own HTTP
//! server. The target fetches it there using the caller's IP (from the signal
//! envelope), the caller's `port` and scheme (from the ticket), processes it, and
//! replies via `bulk.result` (a dedicated signal kind, separate from
//! `rpc.result`) so bulk reply handlers do not compete with RPC reply handlers.
//!
//! The port is per-ticket (not per-node configuration) because the caller's
//! HTTP port must be communicated to the serving node; the server's own
//! `BulkTransport::http_port` is irrelevant to the fetch.
//!
//! ## Transport security (R7)
//!
//! An `https` fetch verifies the caller's gateway certificate by the caller's IP address against
//! the roots `start()` installs: `gateway_tls.bulk_ca_pem_path` when set, else the fleet CA from
//! `[tls]` with the mesh's removal and revocation checks. A responder with no roots refuses an
//! `https` ticket (warned) and never retries it in plaintext. **Not built:** an integrity check on
//! the payload beyond TLS (no per-payload signature), and the fetch target stays the caller's
//! advertised address. A plaintext ticket is still fetched over plaintext — the caller chose it.
//!
//! ## Size limit (R7)
//!
//! `GossipConfig::max_bulk_payload_bytes` bounds both sides: a caller refuses to stage a larger
//! payload, and a responder refuses a fetch whose `Content-Length` exceeds it and abandons one as
//! soon as the bytes read exceed it — the body is never collected past the cap.
//!
//! ## Endpoints
//!
//! The embedded HTTP gateway (`src/agent/http/node.rs`) exposes `GET /bulk/{id}`.
//! Applications that run their own HTTP server must add an equivalent route
//! using [`ServiceHandle::bulk_staging_get`].
//!
//! ## Transport configuration
//!
//! All bulk-transport configuration (HTTP port, scheme, fetch timeout, size cap, pooled HTTP
//! clients) is encapsulated in [`BulkTransport`], which is constructed once
//! in [`GossipAgent::new`] from [`GossipConfig`](crate::GossipConfig) and
//! stored in [`TaskCtx`](super::TaskCtx); the TLS client is installed by `start()` once the node's
//! identity is loaded. Call sites do not pass `http_port` explicitly; they read it from the
//! configured transport.

use crate::node_id::NodeId;
use crate::signal::{SignalScope, signal_kind};
#[cfg(feature = "gateway")]
use crate::signal::Signal;
use bytes::{BufMut, Bytes, BytesMut};
use std::{sync::{Arc, atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering}}, time::Duration};
use tokio::sync::oneshot;
#[cfg(feature = "gateway")]
use tracing::warn;

use super::TaskCtx;
use super::emit_signal;
use super::rpc::PendingReply;

// ── Ticket ────────────────────────────────────────────────────────────────────

/// The byte that marks an R7 ticket. It never begins valid UTF-8, so no pre-R7 kind starts with it.
const TICKET_MARK: u8 = 0xFF;
/// Ticket flag: the caller's gateway serves `https`.
const FLAG_HTTPS: u8 = 0x01;

/// A decoded `INVOKE_BULK` ticket (read by `bulk_serve`, a `gateway` build's).
#[cfg(any(test, feature = "gateway"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ticket<'a> {
    pub nonce: u64,
    pub port:  u16,
    pub https: bool,
    pub kind:  &'a str,
}

/// Encode a ticket: the pre-R7 layout for a plaintext caller, the marked layout for an `https` one.
pub(crate) fn encode_ticket(nonce: u64, port: u16, https: bool, kind: &str) -> Bytes {
    let kind_bytes = kind.as_bytes();
    let mut buf = BytesMut::with_capacity(12 + kind_bytes.len());
    buf.put_u64_le(nonce);
    buf.put_u16_le(port);
    if https {
        buf.put_u8(TICKET_MARK);
        buf.put_u8(FLAG_HTTPS);
    }
    buf.put(kind_bytes);
    buf.freeze()
}

#[cfg(any(test, feature = "gateway"))]
/// Decode either layout; `None` for a ticket too short, with an unknown flag, or a kind that is
/// not UTF-8.
pub(crate) fn decode_ticket(payload: &[u8]) -> Option<Ticket<'_>> {
    if payload.len() < 10 { return None; }
    let nonce = u64::from_le_bytes(payload[..8].try_into().ok()?);
    let port  = u16::from_le_bytes(payload[8..10].try_into().ok()?);
    let (https, kind_bytes) = if payload.get(10) == Some(&TICKET_MARK) {
        let flags = *payload.get(11)?;
        if flags & !FLAG_HTTPS != 0 { return None; } // a flag this release does not know: refuse, never guess
        (flags & FLAG_HTTPS != 0, &payload[12..])
    } else {
        (false, &payload[10..])
    };
    let kind = std::str::from_utf8(kind_bytes).ok()?;
    Some(Ticket { nonce, port, https, kind })
}

// ── Transport adapter ─────────────────────────────────────────────────────────

/// Encapsulates all bulk-transport concerns: staging map, HTTP port and scheme, the size cap, and
/// (when the `gateway` feature is enabled) the pooled HTTP clients used by `bulk_serve` to
/// retrieve staged payloads from remote callers.
pub struct BulkTransport {
    staging:           papaya::HashMap<u64, Bytes>,
    http_port:             AtomicU16,
    /// This node's gateway serves `https` (`GossipConfig::gateway_tls`): its tickets say so.
    https:                 AtomicBool,
    /// `GossipConfig::max_bulk_payload_bytes`.
    max_payload:           u64,
    /// Number of per-request handler tasks currently executing.
    /// Incremented before `tokio::spawn`; decremented when each task exits.
    /// Not part of `task_handles` (unstructured spawn); surfaced in `SystemStats`.
    pub(crate) active_handlers: Arc<AtomicU64>,
    /// Bulk tickets this node refused to fetch or whose fetch it abandoned, every reason
    /// (`SystemStats::bulk_fetches_refused`; by reason, `mycelium_bulk_fetches_refused_total`).
    pub(crate) fetches_refused: AtomicU64,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fetch_timeout:         Duration,
    #[cfg(feature = "gateway")]
    client:                reqwest::Client,
    /// The `https` fetch client, installed by `start()` with the roots it trusts. Empty = this node
    /// refuses an `https` ticket.
    #[cfg(feature = "gateway")]
    tls_client:            std::sync::OnceLock<reqwest::Client>,
}

/// Why a bulk fetch returned no payload.
#[cfg(feature = "gateway")]
#[derive(Debug)]
pub(crate) enum FetchError {
    /// The request or the response failed.
    Http(reqwest::Error),
    /// The payload is larger than `max_bulk_payload_bytes`: `seen` is the `Content-Length`, or the
    /// bytes read when the cap was passed.
    TooLarge { limit: u64, seen: u64 },
    /// An `https` ticket, and this node holds no roots to verify the caller's certificate with.
    NoTlsTrust,
    /// The caller's server answered with a status other than 2xx — a redirect (the client follows
    /// none), `204`, an error — so there is no payload to hand the handler.
    Status(u16),
}

#[cfg(feature = "gateway")]
impl FetchError {
    /// The `reason` label of `mycelium_bulk_fetches_refused_total`.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            FetchError::Http(_)          => "transport",
            FetchError::TooLarge { .. }  => "too_large",
            FetchError::NoTlsTrust       => "no_tls_trust",
            FetchError::Status(_)        => "status",
        }
    }
}

#[cfg(feature = "gateway")]
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Http(e) => write!(f, "{e}"),
            FetchError::TooLarge { limit, seen } =>
                write!(f, "the payload exceeds max_bulk_payload_bytes ({seen} > {limit}); not fetched"),
            FetchError::NoTlsTrust => f.write_str(
                "an https ticket, and this node holds no roots for a peer's gateway certificate \
                 ([tls], or gateway_tls.bulk_ca_pem_path); not fetched, and not retried in plaintext"),
            FetchError::Status(code) => write!(f, "the caller's server answered {code}, not a payload; not handled"),
        }
    }
}

impl BulkTransport {
    /// The defaults: a plaintext caller, a 64 MiB cap (test fixtures build a transport this way).
    #[cfg(test)]
    pub fn new(http_port: u16, _fetch_timeout: Duration, _max_handlers: usize) -> Self {
        Self::with_limits(http_port, _fetch_timeout, _max_handlers, false, crate::config::DEFAULT_MAX_BULK_PAYLOAD_BYTES)
    }

    /// [`new`](Self::new) with the caller's scheme and the size cap (R7).
    pub fn with_limits(http_port: u16, _fetch_timeout: Duration, _max_handlers: usize, https: bool, max_payload: u64) -> Self {
        // `_max_handlers` is the *initial* concurrency cap; it lives in `CoreCtx::hot`
        // (WS-C M9), seeded from config, and is read live per bulk admission — not stored
        // here as a fixed Semaphore.
        Self {
            staging:          papaya::HashMap::new(),
            http_port:        AtomicU16::new(http_port),
            https:            AtomicBool::new(https),
            max_payload,
            active_handlers:  Arc::new(AtomicU64::new(0)),
            fetches_refused:  AtomicU64::new(0),
            #[cfg(all(feature = "gateway", feature = "tls"))]
            fetch_timeout:    _fetch_timeout,
            #[cfg(feature = "gateway")]
            // A peer's redirect is not a peer (realignment repairs R3): the URL is built from the
            // sender's address, and a 3xx could point the fetch anywhere the node can reach.
            // No environment proxy (R7's review round 2, finding 10): a bulk fetch goes to the address
            // the caller's mesh frames came from, and an `HTTP(S)_PROXY` would put a third party
            // between two members — one that sees a plaintext payload, and in front of which an
            // `https` fetch would verify the proxy's tunnel target, not change who carries it.
            client: crate::agent::egress_client::without_redirects()
                .no_proxy()
                .timeout(_fetch_timeout)
                .build()
                .expect("reqwest::Client build should never fail with valid config"),
            #[cfg(feature = "gateway")]
            tls_client: std::sync::OnceLock::new(),
        }
    }

    pub fn http_port(&self) -> u16 {
        self.http_port.load(Ordering::Relaxed)
    }

    /// Whether this node's tickets say its staging endpoint serves `https`.
    pub fn https(&self) -> bool {
        self.https.load(Ordering::Relaxed)
    }

    /// The size cap, `GossipConfig::max_bulk_payload_bytes`.
    pub fn max_payload(&self) -> u64 {
        self.max_payload
    }

    /// Overrides the HTTP port used when advertising staged payloads to peers.
    ///
    /// Call this when using a custom HTTP server instead of the embedded gateway
    /// (`GossipConfig::http_port`). The port is stored atomically so it can be
    /// updated at any time after agent construction.
    pub fn set_http_port(&self, port: u16) {
        self.http_port.store(port, Ordering::Relaxed);
    }

    /// Overrides the scheme this node's tickets advertise (`ServiceHandle::set_bulk_serving_https`).
    pub fn set_https(&self, https: bool) {
        self.https.store(https, Ordering::Relaxed);
    }

    /// Install the `https` fetch client, verifying a peer's gateway certificate with `tls` (R7).
    /// Called once by `start()`; a second call is ignored.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub(crate) fn install_tls_trust(&self, tls: rustls::ClientConfig) -> Result<(), reqwest::Error> {
        // No idle connections (R7's review, finding 10): a pooled keep-alive connection would carry
        // the next fetch from the same caller without a handshake, so a certificate revoked or a
        // member removed since would still be accepted. With session resumption off as well
        // (`install_bulk_trust`; a resumed session runs no verifier either — the reason the mesh
        // turned it off), every https fetch is a full handshake against the live removal and
        // standing sets.
        let client = crate::agent::egress_client::without_redirects()
            .no_proxy()
            .timeout(self.fetch_timeout)
            .pool_max_idle_per_host(0)
            .use_preconfigured_tls(tls)
            .build()?;
        let _ = self.tls_client.set(client);
        Ok(())
    }

    /// Count a ticket this node did not hand to its handler, and why.
    #[cfg(feature = "gateway")]
    pub(crate) fn refused(&self, why: &FetchError) {
        self.fetches_refused.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_bulk_fetches_refused_total", "reason" => why.reason()).increment(1);
        #[cfg(not(feature = "metrics"))]
        let _ = why;
    }

    /// Whether `start()` installed roots for an `https` fetch.
    #[cfg(feature = "gateway")]
    pub(crate) fn has_tls_trust(&self) -> bool {
        self.tls_client.get().is_some()
    }

    /// Stages `payload` under `nonce` and returns a [`StagedGuard`] that
    /// removes the entry on drop (cancellation-safe cleanup).
    fn stage(&self, nonce: u64, payload: Bytes) -> StagedGuard<'_> {
        self.staging.pin().insert(nonce, payload);
        StagedGuard { nonce, staging: &self.staging }
    }

    /// Returns the staged payload for `nonce`, without removing it.
    pub fn get(&self, nonce: u64) -> Option<Bytes> {
        self.staging.pin().get(&nonce).cloned()
    }

    /// Fetches a staged payload from a remote node's HTTP endpoint, over `https` when the ticket
    /// said so, refusing anything past the size cap before or while it is read.
    #[cfg(feature = "gateway")]
    pub(crate) async fn fetch(&self, url: &str, https: bool) -> Result<Bytes, FetchError> {
        let client = if https {
            self.tls_client.get().ok_or(FetchError::NoTlsTrust)?
        } else {
            &self.client
        };
        let mut resp = client.get(url).send().await.map_err(FetchError::Http)?;
        // Any status but 2xx-with-content is not a payload (R7's review, finding 8): `error_for_status`
        // passes a 3xx (the client follows no redirect) and a 204, whose empty body was handed to the
        // handler as the caller's payload.
        let status = resp.status();
        if !status.is_success() || status == reqwest::StatusCode::NO_CONTENT {
            return Err(FetchError::Status(status.as_u16()));
        }
        let limit = self.max_payload;
        if let Some(len) = resp.content_length()
            && len > limit
        {
            return Err(FetchError::TooLarge { limit, seen: len });
        }
        let mut body = BytesMut::with_capacity(resp.content_length().unwrap_or(0).min(limit) as usize);
        while let Some(chunk) = resp.chunk().await.map_err(FetchError::Http)? {
            let seen = body.len() as u64 + chunk.len() as u64;
            if seen > limit {
                return Err(FetchError::TooLarge { limit, seen });
            }
            body.put(chunk);
        }
        Ok(body.freeze())
    }
}

// ── RAII guard ────────────────────────────────────────────────────────────────

struct StagedGuard<'a> {
    nonce:   u64,
    staging: &'a papaya::HashMap<u64, Bytes>,
}

impl Drop for StagedGuard<'_> {
    fn drop(&mut self) {
        self.staging.pin().remove(&self.nonce);
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Error returned by [`ServiceHandle::bulk_call`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkError {
    /// No reply arrived before the timeout elapsed.
    Timeout,
    /// The caller does not have an HTTP server port configured.
    ///
    /// Set `GossipConfig::http_port` before starting the agent, or call
    /// `GossipConfig::set_http_port`.
    NoHttpPort,
    /// The payload is larger than this node's `GossipConfig::max_bulk_payload_bytes`; nothing was
    /// staged or sent (external review R7).
    PayloadTooLarge {
        /// The cap.
        limit: u64,
        /// The payload's length.
        len:   u64,
    },
}

impl std::fmt::Display for BulkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BulkError::Timeout    => f.write_str("bulk_call timed out — no reply from target"),
            BulkError::NoHttpPort => f.write_str("bulk_call requires an http_port in GossipConfig"),
            BulkError::PayloadTooLarge { limit, len } =>
                write!(f, "bulk_call payload of {len} bytes exceeds max_bulk_payload_bytes ({limit}); nothing was sent"),
        }
    }
}

impl std::error::Error for BulkError {}

/// RAII guard: decrements `BulkTransport::active_handlers` when the bulk handler task exits,
/// whether it returns normally, panics, or is cancelled.
#[cfg(feature = "gateway")]
struct ActiveHandlerGuard(Arc<AtomicU64>);

#[cfg(feature = "gateway")]
impl Drop for ActiveHandlerGuard {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

/// Cancels the corresponding `bulk_serve` background task on drop.
pub struct BulkServeHandle {
    pub(crate) _cancel: oneshot::Sender<()>,
}

// ── Core logic ────────────────────────────────────────────────────────────────

/// Core `bulk_call` logic.
pub(crate) async fn bulk_call_ctx(
    ctx:     &TaskCtx,
    target:  NodeId,
    kind:    Arc<str>,
    payload: Bytes,
    timeout: Duration,
) -> Result<Bytes, BulkError> {
    let http_port = ctx.bulk_transport.http_port();
    if http_port == 0 { return Err(BulkError::NoHttpPort); }
    let limit = ctx.bulk_transport.max_payload();
    if payload.len() as u64 > limit {
        return Err(BulkError::PayloadTooLarge { limit, len: payload.len() as u64 });
    }

    // Registered before the ticket leaves, as `rpc_call_framed` does: a provider on this node can answer first.
    let reply = PendingReply::register(ctx, &target, || fastrand::u64(1..));
    let nonce = reply.nonce();

    // Stage the payload; _guard removes it on any exit (timeout, cancel, success).
    let _guard = ctx.bulk_transport.stage(nonce, payload);

    let ticket = encode_ticket(nonce, http_port, ctx.bulk_transport.https(), &kind);

    emit_signal(ctx, Arc::from(signal_kind::INVOKE_BULK), SignalScope::Individual(target.clone()), ticket);
    #[cfg(test)]
    super::rpc::after_emit::run(&ctx.node_id);

    let deadline = tokio::time::Instant::now() + timeout;
    match reply.wait(deadline).await {
        Some(b) => Ok(b),
        None    => Err(BulkError::Timeout),
    }
}

/// The `INVOKE_BULK` receiver a `bulk_serve` reads, registered by the caller before it spawns
/// [`bulk_serve_task`]: a ticket that arrives once `bulk_serve` has returned finds it. Registered
/// inside the task, it existed only once the task first ran, and a ticket delivered before then had
/// no receiver and was dropped — the caller's only answer its own `Timeout` (CI run 42371240189).
#[cfg(feature = "gateway")]
pub(super) fn bulk_serve_rx(ctx: &TaskCtx) -> tokio::sync::mpsc::Receiver<Signal> {
    ctx.signal_handlers.register_with_capacity(Arc::from(signal_kind::INVOKE_BULK), 256)
}

/// Background task that handles incoming `INVOKE_BULK` signals for a given kind, read from `rx`
/// ([`bulk_serve_rx`], registered before this task is spawned).
#[cfg(feature = "gateway")]
pub(super) async fn bulk_serve_task<F, Fut>(
    ctx:         Arc<TaskCtx>,
    kind:        Arc<str>,
    handler:     Arc<F>,
    mut rx:          tokio::sync::mpsc::Receiver<Signal>,
    mut cancel_rx:   oneshot::Receiver<()>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
)
where
    F: Fn(NodeId, Bytes) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Bytes> + Send + 'static,
{
    loop {
        tokio::select! { biased;
            _ = &mut cancel_rx => break,
            result = shutdown_rx.changed() => {
                if result.is_err() || *shutdown_rx.borrow() { break; }
            }
            msg = rx.recv() => {
                let Some(sig) = msg else { break };
                handle_bulk_signal(&ctx, &kind, &handler, sig).await;
            }
        }
    }
}

/// The URL a responder fetches a ticket's payload from: the caller's address from the envelope,
/// the port and scheme from the ticket.
#[cfg(feature = "gateway")]
pub(crate) fn fetch_url(sender: &NodeId, ticket: &Ticket<'_>) -> String {
    let scheme = if ticket.https { "https" } else { "http" };
    let ip = sender.to_socket_addr().ip();
    let host = match ip { std::net::IpAddr::V6(v6) => format!("[{v6}]"), v4 => v4.to_string() };
    format!("{scheme}://{host}:{}/bulk/{:016x}", ticket.port, ticket.nonce)
}

#[cfg(feature = "gateway")]
async fn handle_bulk_signal<F, Fut>(
    ctx:     &Arc<TaskCtx>,
    kind:    &Arc<str>,
    handler: &Arc<F>,
    sig:     Signal,
)
where
    F: Fn(NodeId, Bytes) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Bytes> + Send + 'static,
{
    let Some(ticket) = decode_ticket(&sig.payload) else { return };
    if ticket.kind != kind.as_ref() { return; }
    let nonce = ticket.nonce;
    let https = ticket.https;
    let url = fetch_url(&sig.sender, &ticket);
    let sig_kind = ticket.kind;

    if https && !ctx.bulk_transport.has_tls_trust() {
        // Refused before a handler slot is taken: nothing is fetched, and plaintext is never tried.
        // Named and counted (R7's review, finding 2): the caller sees only its own timeout.
        ctx.bulk_transport.refused(&FetchError::NoTlsTrust);
        warn!(kind=%sig_kind, %url, "bulk_serve: {}", FetchError::NoTlsTrust);
        return;
    }

    tracing::debug!(kind=%sig_kind, %url, "bulk_serve: dispatching fetch");

    // WS-C M9: concurrency cap sampled live from the hot cell (0 = unlimited) instead of a
    // fixed Semaphore, so it can be retuned on a live node. The existing active-handler
    // counter is the accounting: claim a slot with fetch_add, and back out if over the cap
    // (atomic — no overshoot past the limit).
    let limit           = ctx.hot.bulk_handlers();
    let active_handlers = Arc::clone(&ctx.bulk_transport.active_handlers);
    let prev = active_handlers.fetch_add(1, Ordering::Relaxed);
    if limit > 0 && prev as usize >= limit {
        active_handlers.fetch_sub(1, Ordering::Relaxed);
        warn!(kind=%sig_kind, "bulk_serve: handler concurrency limit reached; dropping request");
        return;
    }

    let handler_clone   = Arc::clone(handler);
    let sender          = sig.sender.clone();
    let ctx_clone       = Arc::clone(ctx);

    tokio::spawn(async move {
        let _dec = ActiveHandlerGuard(active_handlers);

        let payload = match ctx_clone.bulk_transport.fetch(&url, https).await {
            Ok(b)  => { tracing::debug!(%url, bytes=b.len(), "bulk_serve: fetch ok"); b }
            Err(e) => {
                ctx_clone.bulk_transport.refused(&e);
                warn!(%url, reason = e.reason(), "bulk_serve: fetch failed: {e}");
                return;
            }
        };

        let result = handler_clone(sender.clone(), payload).await;

        tracing::debug!(sender=%sender, result_bytes=result.len(), "bulk_serve: sending bulk.result");
        let mut buf = BytesMut::with_capacity(8 + result.len());
        buf.put_u64_le(nonce);
        buf.put(result);
        emit_signal(
            &ctx_clone,
            Arc::from(signal_kind::BULK_RESULT),
            SignalScope::Individual(sender),
            buf.freeze(),
        );
    });
}

// ── Trust for an https fetch (R7) ────────────────────────────────────────────

/// The roots in a PEM file — `gateway_tls.bulk_ca_pem_path`. A file that cannot be read or holds
/// no certificate is refused by name.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn bulk_roots_from_pem(path: &std::path::Path) -> Result<rustls::RootCertStore, crate::error::GossipError> {
    let refuse = |why: String| crate::error::GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", reason: why };
    let pem = std::fs::read(path).map_err(|e| refuse(format!("{}: {e}", path.display())))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in <rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::pem_slice_iter(&pem) {
        let cert = cert.map_err(|e| refuse(format!("{}: {e}", path.display())))?;
        roots.add(cert).map_err(|e| refuse(format!("{}: {e}", path.display())))?;
    }
    if roots.is_empty() {
        return Err(refuse(format!("{}: holds no certificate", path.display())));
    }
    Ok(roots)
}

/// `start()`'s refusal (and `mycelium doctor`'s), for a node that serves a gateway over TLS
/// (`http_port` and `gateway_tls` both set; without `http_port` the table is ignored):
///
/// - on an operator certificate (`cert_pem_path`), `bulk_ca_pem_path` is required, must hold a
///   certificate, and **this node's own gateway certificate must verify against it for the IP
///   address peers fetch from** — the node's own address, which its frames carry. A hostname-only
///   certificate, or one another CA signed, would start and then fail every fetch from this node as
///   a caller-side timeout (R7's review, finding 1); it is refused here instead;
/// - a `bulk_ca_pem_path` beside the node-cert path must hold a certificate (the node's own
///   certificate is the fleet CA's, and names its IP by construction).
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn check_bulk_trust(cfg: &crate::config::GossipConfig, node: &NodeId) -> Result<(), crate::error::GossipError> {
    use crate::error::GossipError;
    let Some(gw) = &cfg.gateway_tls else { return Ok(()) };
    if cfg.http_port.is_none() { return Ok(()) }
    let refuse = |why: String| GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", reason: why };
    match (&gw.bulk_ca_pem_path, &gw.cert_pem_path) {
        (Some(ca), Some(cert_path)) => {
            let roots = bulk_roots_from_pem(ca)?;
            let pem = std::fs::read(cert_path).map_err(|e| GossipError::InvalidField {
                field: "gateway_tls", reason: format!("{}: {e}", cert_path.display()),
            })?;
            let chain: Vec<rustls::pki_types::CertificateDer<'static>> = <rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::pem_slice_iter(&pem)
                .collect::<Result<_, _>>()
                .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("{}: {e}", cert_path.display()) })?;
            let Some((leaf, intermediates)) = chain.split_first() else {
                return Err(GossipError::InvalidField { field: "gateway_tls", reason: format!("{}: holds no certificate", cert_path.display()) });
            };
            let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots), Arc::new(rustls::crypto::ring::default_provider()),
            ).build().map_err(|e| refuse(e.to_string()))?;
            let ip = node.to_socket_addr().ip();
            let now = rustls::pki_types::UnixTime::since_unix_epoch(Duration::from_millis(crate::sim_seam::wall_now_ms()));
            use rustls::client::danger::ServerCertVerifier;
            let name = rustls::pki_types::ServerName::IpAddress(ip.into());
            // R7's review round 2, N1: a certificate the **fleet** CA issued is judged by the fleet's
            // check at the responder, which refuses any certificate that is not a mesh certificate
            // (an Ed25519 identity key, names it can read) — and that refusal is final, never
            // re-tried against `bulk_ca_pem_path`. Such a gateway would start and have every fetch
            // from it refused; it is refused here instead.
            if let Some(fleet_ca) = fleet_ca_path(cfg)
                && fleet_ca.exists()
                && let Ok(fleet_roots) = bulk_roots_from_pem(&fleet_ca)
                && let Ok(fleet) = rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(fleet_roots), Arc::new(rustls::crypto::ring::default_provider()),
                ).build()
                && chains(fleet.verify_server_cert(leaf, intermediates, &name, &[], now))
                && (mycelium_core::tls::ed25519_key_from_cert_der(leaf.as_ref()).is_none()
                    || mycelium_core::tls::cert_identity(leaf.as_ref()).is_none())
            {
                return Err(refuse(format!(
                    "this node's gateway certificate ({}) was issued by the fleet CA ({}) but is not a mesh certificate \
                     (an Ed25519 identity key): a responder judges a fleet-issued certificate by the mesh's check, which \
                     refuses it, so every bulk fetch from this node would fail. Serve the node certificate instead (leave \
                     cert_pem_path and key_pem_path unset), or issue the gateway certificate from a separate operator CA",
                    cert_path.display(), fleet_ca.display(),
                )));
            }
            verifier.verify_server_cert(leaf, intermediates, &name, &[], now)
                .map(|_| ())
                .map_err(|e| refuse(format!(
                    "this node's gateway certificate ({}) does not verify against {} for {ip}, the address peers fetch \
                     a bulk payload from: {e}. It must be signed by that CA and name {ip} in a subject alternative name, \
                     or every bulk fetch from this node would fail",
                    cert_path.display(), ca.display(),
                )))
        }
        (Some(ca), None) => bulk_roots_from_pem(ca).map(|_| ()),
        (None, Some(_)) => Err(refuse(
            "the gateway serves an operator certificate (cert_pem_path), so a bulk fetch from a peer's \
             gateway has no roots to verify it with: the fleet CA did not sign it, and a bulk fetch is \
             never retried in plaintext. Set bulk_ca_pem_path to the CA that signs the gateways' \
             certificates (each must name its node's IP address)".into(),
        )),
        (None, None) => Ok(()),
    }
}

/// The fleet CA certificate's path, as `[tls]` resolves it (`ca_cert_pem`, else
/// `{auto_cert_dir}/ca-cert.pem`). `None` without `[tls]`.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn fleet_ca_path(cfg: &crate::config::GossipConfig) -> Option<std::path::PathBuf> {
    let tls = cfg.tls.as_ref()?;
    Some(tls.ca_cert_pem.clone().unwrap_or_else(|| tls.auto_cert_dir.join("ca-cert.pem")))
}

/// Whether a verification shows the certificate chains to the roots — it verified, or failed only on
/// the name or its dates.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn chains(r: Result<rustls::client::danger::ServerCertVerified, rustls::Error>) -> bool {
    use rustls::CertificateError as C;
    match r {
        Ok(_) => true,
        Err(rustls::Error::InvalidCertificate(e)) => matches!(e,
            C::NotValidForName | C::NotValidForNameContext { .. } | C::Expired | C::ExpiredContext { .. }
            | C::NotValidYet | C::NotValidYetContext { .. }),
        Err(_) => false,
    }
}

/// Install the `https` fetch client on `start()`. Its roots are the **union** of the fleet's check
/// (the CA chain, then the removal and revocation checks — for a peer on the node-cert path) and
/// `bulk_ca_pem_path` (for a peer on an operator certificate), so a fleet mixing the two verifies
/// both (R7's review, finding 2). A node with neither refuses an `https` ticket, named and counted.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn install_bulk_trust(ctx: &TaskCtx, cfg: &crate::config::GossipConfig) -> Result<(), crate::error::GossipError> {
    use crate::error::GossipError;
    let refuse = |e: String| GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", reason: e };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let operator = match cfg.gateway_tls.as_ref().and_then(|g| g.bulk_ca_pem_path.as_ref()) {
        Some(path) => Some(
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(bulk_roots_from_pem(path)?), Arc::clone(&provider))
                .build()
                .map_err(|e| refuse(e.to_string()))?,
        ),
        None => None,
    };
    let fleet = ctx.tls.get().map(|t| t.fleet_server_verifier());
    if fleet.is_none() && operator.is_none() {
        return Ok(());
    }
    let client = bulk_tls_client_config(fleet, operator).map_err(|e| refuse(e.to_string()))?;
    ctx.bulk_transport.install_tls_trust(client).map_err(|e| GossipError::InvalidField {
        field: "gateway_tls",
        reason: format!("the bulk https client could not be built: {e}"),
    })
}

/// The bulk fetch's TLS client configuration over the union of `fleet` and `operator`.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn bulk_tls_client_config(
    fleet:    Option<Arc<dyn rustls::client::danger::ServerCertVerifier>>,
    operator: Option<Arc<rustls::client::WebPkiServerVerifier>>,
) -> Result<rustls::ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(BulkServerVerifier { fleet, operator, provider: Arc::clone(&provider) });
    let mut client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    // No session resumption, as on the mesh: a resumed TLS 1.3 session runs no certificate verifier,
    // so a fetch after the caller's key was revoked would be admitted on the session its old
    // certificate opened. With the pool off too (`install_tls_trust`), every fetch verifies afresh —
    // at the cost of a full TCP and TLS handshake per fetch.
    client.resumption = rustls::client::Resumption::disabled();
    Ok(client)
}

/// A peer's gateway certificate is accepted when the fleet's check accepts it, or — only when the
/// fleet's **chain** check refused it (`InvalidCertificate`: another issuer, a signature that does
/// not verify against the fleet CA, dates) — when `bulk_ca_pem_path`'s roots accept it on their own.
/// A certificate the fleet CA did issue but the removal or revocation check refuses (a `General`
/// error, raised only after the chain verified) is refused outright, never re-tried against the
/// operator's roots, so naming the fleet CA in `bulk_ca_pem_path` cannot launder a revoked key. The
/// issuer is not matched by name: two CAs with one subject (rcgen's default, for one) make the
/// fleet check fail on the signature rather than the issuer.
#[cfg(all(feature = "gateway", feature = "tls"))]
#[derive(Debug)]
struct BulkServerVerifier {
    fleet:    Option<Arc<dyn rustls::client::danger::ServerCertVerifier>>,
    operator: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

#[cfg(all(feature = "gateway", feature = "tls"))]
impl rustls::client::danger::ServerCertVerifier for BulkServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity:    &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name:   &rustls::pki_types::ServerName<'_>,
        ocsp:          &[u8],
        now:           rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let mut fleet_refusal = None;
        if let Some(fleet) = &self.fleet {
            match fleet.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
                Ok(v) => return Ok(v),
                Err(e @ rustls::Error::InvalidCertificate(_)) => fleet_refusal = Some(e),
                Err(e) => return Err(e),
            }
        }
        match &self.operator {
            Some(op) => op.verify_server_cert(end_entity, intermediates, server_name, ocsp, now),
            None => Err(fleet_refusal.unwrap_or(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer))),
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert:    &rustls::pki_types::CertificateDer<'_>,
        dss:     &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert:    &rustls::pki_types::CertificateDer<'_>,
        dss:     &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use bytes::Bytes;
    use std::{sync::Arc, time::Duration};

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    /// Smoke-test the staging map without a real HTTP round-trip.
    #[tokio::test]
    async fn bulk_staging_insert_remove() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();

        // Manually stage and retrieve a payload.
        let nonce: u64 = 42;
        agent.task_ctx.bulk_transport.staging.pin().insert(nonce, Bytes::from_static(b"test-payload"));
        let got = agent.service().bulk_staging_get(nonce);
        assert_eq!(got, Some(Bytes::from_static(b"test-payload")));

        // Simulate cleanup.
        agent.task_ctx.bulk_transport.staging.pin().remove(&nonce);
        assert!(agent.service().bulk_staging_get(nonce).is_none());

        agent.shutdown().await;
    }

    /// **A bulk reply to a call on this node arrives, whenever the provider answers** — `rpc_call_framed`'s #642
    /// regression, on `bulk_call_ctx`, which emitted its ticket and only then registered for the `bulk.result`. A
    /// provider on this node receives the ticket inside `emit_signal`; the hook holds the caller after the emission
    /// until the provider has answered, so on the old order the reply found no registration and the call timed out.
    /// The provider here answers without fetching: the order of registration and emission is what is under test.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bulk_reply_sent_before_the_caller_waits_still_reaches_it() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        agent.task_ctx.bulk_transport.set_http_port(port); // any non-zero port: nothing fetches

        let (answered_tx, answered_rx) = std::sync::mpsc::channel::<()>();
        let answered_rx = std::sync::Mutex::new(answered_rx);
        crate::agent::rpc::after_emit::set(&id, Arc::new(move || {
            let _ = answered_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
        }));
        let mut rx = agent.task_ctx.signal_handlers.register(Arc::from(signal_kind::INVOKE_BULK));
        let provider = Arc::clone(&agent);
        tokio::spawn(async move {
            while let Some(ticket) = rx.recv().await {
                let mut reply = BytesMut::with_capacity(12);
                reply.put_slice(&ticket.payload[..8]);
                reply.put_slice(b"done");
                emit_signal(&provider.task_ctx, Arc::from(signal_kind::BULK_RESULT),
                            SignalScope::Individual(ticket.sender.clone()), reply.freeze());
                let _ = answered_tx.send(());
            }
        });

        let result = agent.service().bulk_call(id.clone(), "self.bulk", Bytes::from_static(b"big"), Duration::from_secs(2)).await;
        crate::agent::rpc::after_emit::clear(&id);
        assert_eq!(result, Ok(Bytes::from_static(b"done")), "the provider answered before the caller waited");
        assert!(agent.task_ctx.rpc_pending.lock().unwrap().is_empty(), "nothing is left registered");
        agent.shutdown().await;
    }

    /// Two nodes on one host, each serving its gateway on `http_port`; `b` serves `echo-bulk`.
    /// With `tls_dir` both share one fleet CA (the directory holds the CA and each node's key) and
    /// serve the gateway over HTTPS on the node certificate.
    #[cfg(feature = "gateway")]
    async fn bulk_pair(tls_dir: Option<&std::path::Path>) -> (Arc<GossipAgent>, Arc<GossipAgent>, NodeId) {
        let mk = |boot: Vec<NodeId>| {
            let port = alloc_port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = port;
            cfg.http_port = Some(alloc_port());
            cfg.http_addr = "127.0.0.1".to_string();
            cfg.bootstrap_peers = boot;
            cfg.health_check_max_jitter_ms = 1;
            #[cfg(feature = "tls")]
            if let Some(dir) = tls_dir {
                cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.to_path_buf(), ..Default::default() });
                cfg.gateway_tls = Some(crate::GatewayTlsConfig::default());
            }
            #[cfg(not(feature = "tls"))]
            let _ = tls_dir;
            (NodeId::new("127.0.0.1", port).unwrap(), cfg)
        };
        let (ida, cfga) = mk(vec![]);
        let (idb, cfgb) = mk(vec![ida.clone()]);
        let a = Arc::new(GossipAgent::new(ida, cfga));
        a.start().await.unwrap();
        let b = Arc::new(GossipAgent::new(idb.clone(), cfgb));
        b.start().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while a.peers().is_empty() || b.peers().is_empty() {
            assert!(std::time::Instant::now() < deadline, "the pair never connected");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        (a, b, idb)
    }

    /// The plaintext gateway's bulk call, end to end: the control for the TLS case below, and the
    /// pin that a plaintext caller's ticket is still the 2.39.0 layout an older responder reads.
    #[cfg(feature = "gateway")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bulk_call_completes_between_two_plaintext_gateways() {
        let (a, b, idb) = bulk_pair(None).await;
        let _serve = b.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let payload = Bytes::from(vec![7u8; 300_000]);
        let got = a.service().bulk_call(idb, "echo-bulk", payload.clone(), Duration::from_secs(10)).await;
        assert_eq!(got, Ok(payload));
        a.shutdown().await;
        b.shutdown().await;
    }

    /// **A ticket that arrives as soon as `bulk_serve` returns is served.** `bulk_serve` registered
    /// its `invoke.bulk` receiver inside the task it spawned, so until that task first ran a ticket
    /// found no receiver and was dropped — no retry, the caller's only answer its own `Timeout`. The
    /// plaintext pair test above failed that way once under CI load (run 42371240189, the strict
    /// `compliance,a2a` lib run): its ticket crossed the mesh before `b`'s serve task was scheduled.
    /// A current-thread runtime makes the window certain rather than likely: the spawned task cannot
    /// run before this caller's `emit_signal` delivers its own ticket locally, without a yield.
    #[cfg(feature = "gateway")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_ticket_sent_as_soon_as_bulk_serve_returns_is_served() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(alloc_port());
        cfg.http_addr = "127.0.0.1".to_string();
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        let _serve = agent.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let payload = Bytes::from_static(b"served");
        let got = agent.service().bulk_call(id, "echo-bulk", payload.clone(), Duration::from_secs(5)).await;
        assert_eq!(got, Ok(payload), "bulk_serve's receiver is registered when it returns");
        agent.shutdown().await;
    }

    /// **F6, the exit's first half: a `gateway_tls` node completes a `bulk_call`.** Before R7 the
    /// responder built `http://` unconditionally, spoke plaintext to a TLS listener, and the call
    /// timed out.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bulk_call_completes_between_two_gateway_tls_nodes() {
        let dir = std::env::temp_dir().join(format!("bulk-tls-{}-{}", std::process::id(), alloc_port()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b, idb) = bulk_pair(Some(&dir)).await;
        let _serve = b.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let payload = Bytes::from(vec![9u8; 300_000]);
        let got = a.service().bulk_call(idb, "echo-bulk", payload.clone(), Duration::from_secs(10)).await;
        assert_eq!(got, Ok(payload), "a bulk call between two gateway_tls nodes completes");
        a.shutdown().await;
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bulk_call_returns_no_http_port_on_zero() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        let ghost = NodeId::new("127.0.0.1", 19993).unwrap();
        let err = agent.service().bulk_call(ghost, "noop", Bytes::new(), Duration::from_millis(100)).await;
        assert_eq!(err, Err(BulkError::NoHttpPort));
        agent.shutdown().await;
    }

    // ── R7: the ticket ───────────────────────────────────────────────────────

    /// A plaintext caller's ticket is byte-for-byte the pre-R7 layout, so a responder older than R7
    /// still reads it; an `https` ticket round-trips and an older responder's kind read of it fails
    /// (invalid UTF-8), which drops it rather than fetching over the wrong scheme.
    #[test]
    fn a_plaintext_ticket_is_the_old_layout_and_an_https_ticket_is_marked() {
        let plain = encode_ticket(0x0102_0304_0506_0708, 8080, false, "echo");
        let mut old = Vec::new();
        old.extend_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        old.extend_from_slice(&8080u16.to_le_bytes());
        old.extend_from_slice(b"echo");
        assert_eq!(&plain[..], &old[..], "a plaintext ticket is the 2.39.0 layout");
        assert_eq!(decode_ticket(&plain), Some(Ticket { nonce: 0x0102_0304_0506_0708, port: 8080, https: false, kind: "echo" }));

        let tls = encode_ticket(7, 8443, true, "echo");
        assert_eq!(decode_ticket(&tls), Some(Ticket { nonce: 7, port: 8443, https: true, kind: "echo" }));
        assert!(std::str::from_utf8(&tls[10..]).is_err(), "a pre-R7 responder reads the kind as invalid UTF-8 and drops it");
    }

    #[test]
    fn a_ticket_with_an_unknown_flag_or_too_short_is_refused() {
        let mut t = encode_ticket(7, 8443, true, "echo").to_vec();
        t[11] = 0x03;
        assert_eq!(decode_ticket(&t), None, "an unknown flag is refused, not guessed at");
        assert_eq!(decode_ticket(&[0u8; 9]), None);
        assert_eq!(decode_ticket(&encode_ticket(7, 1, true, "")[..11]), None, "a mark with no flags byte");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn the_fetch_url_follows_the_ticket_and_brackets_an_ipv6_sender() {
        let v4 = NodeId::new("10.0.0.5", 7000).unwrap();
        let t = Ticket { nonce: 0xab, port: 8443, https: true, kind: "k" };
        assert_eq!(fetch_url(&v4, &t), "https://10.0.0.5:8443/bulk/00000000000000ab");
        let t = Ticket { https: false, ..t };
        assert_eq!(fetch_url(&v4, &t), "http://10.0.0.5:8443/bulk/00000000000000ab");
        let v6 = NodeId::new("::1", 7000).unwrap();
        assert_eq!(fetch_url(&v6, &t), "http://[::1]:8443/bulk/00000000000000ab");
    }

    // ── R7: the size cap ─────────────────────────────────────────────────────

    /// A local server answering `/declared` with a fixed body (so a `Content-Length`) and
    /// `/streamed` with the same bytes in chunks and no length.
    #[cfg(feature = "gateway")]
    async fn body_server(len: usize) -> u16 {
        use axum::{Router, routing::get, body::Body};
        let body = Bytes::from(vec![1u8; len]);
        let streamed = body.clone();
        let app = Router::new()
            .route("/declared", get(move || { let b = body.clone(); async move { b } }))
            .route("/streamed", get(move || {
                let b = streamed.clone();
                async move {
                    let chunks: Vec<Result<Bytes, std::io::Error>> = b.chunks(1024).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
                    Body::from_stream(futures_util::stream::iter(chunks))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    /// **F6, the exit's second half: a fetch over the cap is refused** — up front when the peer
    /// declares a length, and while reading when it does not, so the body is never collected past
    /// the cap. A payload exactly at the cap is fetched.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_fetch_over_the_cap_is_refused_declared_or_streamed() {
        let port = body_server(10_000).await;
        let over = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 9_999);
        for path in ["declared", "streamed"] {
            let url = format!("http://127.0.0.1:{port}/{path}");
            match over.fetch(&url, false).await {
                Err(FetchError::TooLarge { limit: 9_999, seen }) => {
                    assert!(seen > 9_999 && seen <= 10_000, "{path}: refused as it passed the cap, at {seen}");
                }
                other => panic!("{path}: a 10 000-byte payload under a 9 999-byte cap must be refused, got {other:?}"),
            }
        }
        let at = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 10_000);
        for path in ["declared", "streamed"] {
            let got = at.fetch(&format!("http://127.0.0.1:{port}/{path}"), false).await.expect("a payload at the cap is fetched");
            assert_eq!(got.len(), 10_000);
        }
    }

    /// A streamed body is abandoned at the cap, not read to its end: the bytes read when the fetch
    /// gives up are within one chunk of the cap, however large the body is.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_streamed_fetch_stops_reading_at_the_cap() {
        let port = body_server(4 * 1024 * 1024).await;
        let t = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 64 * 1024);
        match t.fetch(&format!("http://127.0.0.1:{port}/streamed"), false).await {
            Err(FetchError::TooLarge { seen, .. }) => assert!(seen <= 64 * 1024 + 64 * 1024, "stopped near the cap, at {seen}"),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    /// The caller's side of the cap: a payload larger than its own `max_bulk_payload_bytes` is
    /// refused before anything is staged or sent.
    #[tokio::test]
    async fn a_bulk_call_over_the_cap_is_refused_before_it_is_staged() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port; cfg.max_bulk_payload_bytes = 1_000;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        agent.task_ctx.bulk_transport.set_http_port(port);
        let ghost = NodeId::new("127.0.0.1", 19994).unwrap();
        let err = agent.service().bulk_call(ghost, "noop", Bytes::from(vec![0u8; 1_001]), Duration::from_millis(100)).await;
        assert_eq!(err, Err(BulkError::PayloadTooLarge { limit: 1_000, len: 1_001 }));
        assert!(agent.task_ctx.rpc_pending.lock().unwrap().is_empty(), "nothing was registered");
        agent.shutdown().await;
    }

    /// An `https` ticket on a node with no roots is refused, never fetched in plaintext.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn an_https_fetch_without_roots_is_refused() {
        let t = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 1_000);
        assert!(matches!(t.fetch("https://127.0.0.1:1/bulk/1", true).await, Err(FetchError::NoTlsTrust)));
    }

    // ── R7: operator certificates and the start refusal ──────────────────────

    /// An operator CA and a gateway certificate it signs for 127.0.0.1, written under `dir`.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn operator_pki(dir: &std::path::Path, tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        operator_pki_with(dir, tag, vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())])
    }

    /// **R7's refusal:** a gateway on an operator certificate with no `bulk_ca_pem_path` refuses
    /// `start()` by name; one naming an unreadable file is refused by the same name.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn a_gateway_on_an_operator_certificate_without_bulk_roots_refuses_the_start() {
        let dir = std::env::temp_dir().join(format!("bulk-op-refuse-{}-{}", std::process::id(), alloc_port()));
        let (_ca, cert, key) = operator_pki(&dir, "op");
        for bulk_ca in [None, Some(dir.join("missing-ca.pem"))] {
            let port = alloc_port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = port;
            cfg.http_port = Some(alloc_port());
            cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.join("mesh"), ..Default::default() });
            cfg.gateway_tls = Some(crate::GatewayTlsConfig { cert_pem_path: Some(cert.clone()), key_pem_path: Some(key.clone()), bulk_ca_pem_path: bulk_ca.clone() });
            let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
            match a.start().await {
                Err(crate::GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", .. }) => {}
                other => panic!("bulk_ca_pem_path = {bulk_ca:?}: expected a refusal naming gateway_tls.bulk_ca_pem_path, got {other:?}"),
            }
            a.shutdown().await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Operator certificates end to end: with `bulk_ca_pem_path` naming the CA that signed the
    /// gateways' certificates, a bulk call completes; a responder trusting a different CA refuses
    /// the caller's certificate, fetches nothing, and the call times out rather than falling back.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn operator_certificates_need_the_operator_ca_and_an_untrusted_one_is_refused() {
        for trusted in [true, false] {
            let dir = std::env::temp_dir().join(format!("bulk-op-{}-{}", std::process::id(), alloc_port()));
            let op = operator_pki(&dir, "op");
            let other = operator_pki(&dir, "other");
            // B, the responder, trusts only `op`. A, the caller, is on `op` too — or, untrusted, on a CA
            // B has never heard of. Each node's own certificate verifies against its own bulk roots, so
            // both start (R7's review, finding 1).
            let (a_ca, a_cert, a_key) = if trusted { op.clone() } else { other.clone() };
            let (b_ca, b_cert, b_key) = op;
            let gw = |cert: std::path::PathBuf, key: std::path::PathBuf, ca: std::path::PathBuf| crate::GatewayTlsConfig {
                cert_pem_path: Some(cert), key_pem_path: Some(key), bulk_ca_pem_path: Some(ca),
            };
            let (ida, cfga) = tls_node(&dir, vec![], true, gw(a_cert, a_key, a_ca));
            let (idb, cfgb) = tls_node(&dir, vec![ida.clone()], true, gw(b_cert, b_key, b_ca));
            let a = Arc::new(GossipAgent::new(ida, cfga));
            a.start().await.unwrap();
            let b = Arc::new(GossipAgent::new(idb.clone(), cfgb));
            b.start().await.unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while a.peers().is_empty() || b.peers().is_empty() {
                assert!(std::time::Instant::now() < deadline, "the pair never connected");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let _serve = b.service().bulk_serve("echo-bulk", |_s, p| async move { p });
            let payload = Bytes::from(vec![3u8; 50_000]);
            let got = a.service().bulk_call(idb, "echo-bulk", payload.clone(), Duration::from_secs(if trusted { 10 } else { 3 })).await;
            let refused = b.system_stats().bulk_fetches_refused;
            a.shutdown().await;
            b.shutdown().await;
            let _ = std::fs::remove_dir_all(&dir);
            if trusted {
                assert_eq!(got, Ok(payload), "the operator CA verifies the caller's gateway");
                assert_eq!(refused, 0);
            } else {
                assert_eq!(got, Err(BulkError::Timeout), "an untrusted certificate is refused and nothing is fetched in plaintext");
                assert_eq!(refused, 1, "the responder counted the refusal");
            }
        }
    }

    // ── R7, review round 1 ───────────────────────────────────────────────────

    /// A server that answers every connection with `response`, byte for byte, then closes.
    #[cfg(feature = "gateway")]
    async fn raw_server(response: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(response).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        port
    }

    /// **Finding 3: only the up-front check can refuse this.** The server declares a length over the
    /// cap and sends a hundred bytes; the streaming check never reaches the cap, so the fetch is
    /// refused as too large only because the declared length was read before the body.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_declared_length_over_the_cap_is_refused_before_the_body_is_read() {
        static RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 20000\r\nConnection: close\r\n\r\n\
            0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789";
        let port = raw_server(RESPONSE).await;
        let t = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 10_000);
        match t.fetch(&format!("http://127.0.0.1:{port}/bulk/1"), false).await {
            Err(FetchError::TooLarge { limit: 10_000, seen: 20_000 }) => {}
            other => panic!("a declared 20 000 bytes under a 10 000-byte cap must be refused up front, got {other:?}"),
        }
    }

    /// **Finding 8:** a redirect (the client follows none) or a `204` is not a payload; before, its
    /// body — empty, or the redirect's — was handed to the handler as the caller's.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_status_other_than_2xx_with_content_is_not_a_payload() {
        static REDIRECT: &[u8] = b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmoved";
        static NO_CONTENT: &[u8] = b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        let t = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 10_000);
        for (what, resp) in [("302", REDIRECT), ("204", NO_CONTENT)] {
            let port = raw_server(resp).await;
            let got = t.fetch(&format!("http://127.0.0.1:{port}/bulk/1"), false).await;
            assert!(got.is_err(), "{what}: not a payload, got {got:?}");
        }
    }

    /// An operator CA and a gateway certificate it signs with `sans`, written under `dir`.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn operator_pki_with(dir: &std::path::Path, tag: &str, sans: Vec<rcgen::SanType>) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_p = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_p.self_signed(&ca_key).unwrap();
        let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_p = rcgen::CertificateParams::new(vec![]).unwrap();
        leaf_p.subject_alt_names = sans;
        let leaf = leaf_p.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        let (ca_path, cert_path, key_path) = (dir.join(format!("{tag}-ca.pem")), dir.join(format!("{tag}-gw.pem")), dir.join(format!("{tag}-gw.key")));
        std::fs::write(&ca_path, ca.pem()).unwrap();
        std::fs::write(&cert_path, leaf.pem()).unwrap();
        std::fs::write(&key_path, leaf_key.serialize_pem()).unwrap();
        (ca_path, cert_path, key_path)
    }

    /// A node on 127.0.0.1 serving its gateway over TLS on `gateway`, meshed through `dir/mesh`.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn tls_node(dir: &std::path::Path, boot: Vec<NodeId>, http: bool, gateway: crate::GatewayTlsConfig) -> (NodeId, GossipConfig) {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        if http { cfg.http_port = Some(alloc_port()); }
        cfg.http_addr = "127.0.0.1".to_string();
        cfg.bootstrap_peers = boot;
        cfg.health_check_max_jitter_ms = 1;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.join("mesh"), ..Default::default() });
        cfg.gateway_tls = Some(gateway);
        (NodeId::new("127.0.0.1", port).unwrap(), cfg)
    }

    /// **Finding 1:** a node whose own operator certificate would not verify for its address refuses
    /// to start, by name — a hostname-only certificate, and one another CA signed. Each would have
    /// started and failed every fetch from it as the caller's timeout.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn an_operator_certificate_that_does_not_verify_for_the_node_refuses_the_start() {
        let dir = std::env::temp_dir().join(format!("bulk-own-{}-{}", std::process::id(), alloc_port()));
        let (ca, host_cert, host_key) = operator_pki_with(&dir, "host", vec![rcgen::SanType::DnsName("gw.example.com".try_into().unwrap())]);
        let (other_ca, _, _) = operator_pki_with(&dir, "other", vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())]);
        let (op_ca, ip_cert, ip_key) = operator_pki_with(&dir, "ip", vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())]);
        let _ = op_ca;
        for (what, cert, key, bulk_ca) in [
            ("a hostname-only certificate", &host_cert, &host_key, &ca),
            ("a certificate another CA signed", &ip_cert, &ip_key, &other_ca),
        ] {
            let (id, cfg) = tls_node(&dir, vec![], true, crate::GatewayTlsConfig {
                cert_pem_path: Some(cert.clone()), key_pem_path: Some(key.clone()), bulk_ca_pem_path: Some(bulk_ca.clone()),
            });
            let a = GossipAgent::new(id, cfg);
            match a.start().await {
                Err(crate::GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", .. }) => {}
                other => panic!("{what}: expected a refusal naming gateway_tls.bulk_ca_pem_path, got {other:?}"),
            }
            a.shutdown().await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Finding 9:** without `http_port` the gateway does not run, so its table asks nothing of
    /// bulk trust and the node starts.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn an_operator_certificate_on_a_node_without_a_gateway_port_starts() {
        let dir = std::env::temp_dir().join(format!("bulk-nohttp-{}-{}", std::process::id(), alloc_port()));
        let (_ca, cert, key) = operator_pki_with(&dir, "op", vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())]);
        let (id, cfg) = tls_node(&dir, vec![], false, crate::GatewayTlsConfig {
            cert_pem_path: Some(cert), key_pem_path: Some(key), ..Default::default()
        });
        let a = GossipAgent::new(id, cfg);
        a.start().await.expect("no http_port: the gateway table is ignored and the node starts");
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Finding 2:** a fleet mixing a node-cert gateway and an operator-cert gateway completes a bulk
    /// call both ways — each side's roots are the fleet CA and the operator CA together.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mixed_fleet_of_node_and_operator_certificates_completes_bulk_both_ways() {
        let dir = std::env::temp_dir().join(format!("bulk-mixed-{}-{}", std::process::id(), alloc_port()));
        let (ca, cert, key) = operator_pki_with(&dir, "op", vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())]);
        let (ida, cfga) = tls_node(&dir, vec![], true, crate::GatewayTlsConfig { bulk_ca_pem_path: Some(ca.clone()), ..Default::default() });
        let (idb, cfgb) = tls_node(&dir, vec![ida.clone()], true, crate::GatewayTlsConfig {
            cert_pem_path: Some(cert), key_pem_path: Some(key), bulk_ca_pem_path: Some(ca),
        });
        let a = Arc::new(GossipAgent::new(ida.clone(), cfga));
        a.start().await.unwrap();
        let b = Arc::new(GossipAgent::new(idb.clone(), cfgb));
        b.start().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while a.peers().is_empty() || b.peers().is_empty() {
            assert!(std::time::Instant::now() < deadline, "the pair never connected");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _sa = a.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let _sb = b.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let payload = Bytes::from(vec![5u8; 40_000]);
        let a_to_b = a.service().bulk_call(idb, "echo-bulk", payload.clone(), Duration::from_secs(10)).await;
        let b_to_a = b.service().bulk_call(ida, "echo-bulk", payload.clone(), Duration::from_secs(10)).await;
        a.shutdown().await;
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(a_to_b, Ok(payload.clone()), "the operator-cert node verifies the node-cert caller (fleet CA)");
        assert_eq!(b_to_a, Ok(payload), "the node-cert node verifies the operator-cert caller (bulk_ca_pem_path)");
    }

    /// **Finding 2's other half:** an `https` ticket on a node with no roots at all is refused by
    /// name and counted, where before only the caller's timeout said anything. The ticket is
    /// injected as a signal from this node to itself, so nothing else can explain the count.
    #[cfg(feature = "gateway")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_https_ticket_on_a_node_without_roots_is_refused_and_counted() {
        let port = alloc_port();
        let id   = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default(); cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        assert!(!agent.task_ctx.bulk_transport.has_tls_trust(), "no [tls], no bulk_ca_pem_path: no roots");
        let _serve = agent.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ticket = encode_ticket(42, 1, true, "echo-bulk");
        emit_signal(&agent.task_ctx, Arc::from(signal_kind::INVOKE_BULK), SignalScope::Individual(id.clone()), ticket);
        let mut counted = false;
        for _ in 0..100 {
            if agent.system_stats().bulk_fetches_refused == 1 { counted = true; break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(counted, "the https ticket is counted as refused");
        assert_eq!(agent.system_stats().active_bulk_handlers, 0, "no handler slot was taken");
        agent.shutdown().await;
    }


    /// The union never launders a revocation: a certificate the fleet check refuses after its chain
    /// verified (a `General` error — removed, revoked) is refused even when `bulk_ca_pem_path`'s
    /// roots would accept it, and a chain refusal (another CA) falls through to them.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn the_union_of_roots_falls_through_only_on_a_chain_refusal() {
        use rustls::client::danger::ServerCertVerifier;
        #[derive(Debug)]
        struct Fleet(rustls::Error);
        impl ServerCertVerifier for Fleet {
            fn verify_server_cert(&self, _: &rustls::pki_types::CertificateDer<'_>, _: &[rustls::pki_types::CertificateDer<'_>],
                _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime)
                -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> { Err(self.0.clone()) }
            fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct)
                -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { unreachable!() }
            fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct)
                -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { unreachable!() }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> { vec![] }
        }
        let dir = std::env::temp_dir().join(format!("bulk-union-{}-{}", std::process::id(), alloc_port()));
        let (ca, cert, _) = operator_pki(&dir, "op");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let operator = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(bulk_roots_from_pem(&ca).unwrap()), Arc::clone(&provider)).build().unwrap();
        let leaf = <rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::pem_slice_iter(&std::fs::read(&cert).unwrap()).next().unwrap().unwrap();
        let name = rustls::pki_types::ServerName::IpAddress(std::net::IpAddr::from([127, 0, 0, 1]).into());
        let now = rustls::pki_types::UnixTime::now();
        let check = |fleet_err: rustls::Error| BulkServerVerifier {
            fleet: Some(Arc::new(Fleet(fleet_err))), operator: Some(Arc::clone(&operator)), provider: Arc::clone(&provider),
        }.verify_server_cert(&leaf, &[], &name, &[], now);
        assert!(check(rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature)).is_ok(),
            "a chain refusal falls through to bulk_ca_pem_path, which accepts this certificate");
        assert!(check(rustls::Error::General("mycelium: this identity has been removed from the mesh".into())).is_err(),
            "a removal or revocation refusal is final, whatever bulk_ca_pem_path would say");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Round 2, N1:** a gateway certificate issued by the *fleet* CA that is not a mesh certificate
    /// (an ECDSA leaf with an IP SAN) is refused at start, by name. A responder judges a fleet-issued
    /// certificate by the mesh's check, which refuses it for good; before, the node started and every
    /// fetch from it was refused, the caller seeing only its timeout.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn a_fleet_issued_gateway_certificate_that_is_not_a_mesh_certificate_refuses_the_start() {
        let dir = std::env::temp_dir().join(format!("bulk-fleetop-{}-{}", std::process::id(), alloc_port()));
        let mesh = dir.join("mesh");
        std::fs::create_dir_all(&mesh).unwrap();
        // The fleet CA, minted where the node will load it from.
        let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let mut ca_p = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_p.self_signed(&ca_key).unwrap();
        std::fs::write(mesh.join("ca-cert.pem"), ca.pem()).unwrap();
        std::fs::write(mesh.join("ca-key.pem"), ca_key.serialize_pem()).unwrap();
        // An operator-shaped leaf from that CA: ECDSA, an IP SAN, no identity key.
        let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_p = rcgen::CertificateParams::new(vec![]).unwrap();
        leaf_p.subject_alt_names = vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())];
        let leaf = leaf_p.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        let (cert, key) = (dir.join("gw.pem"), dir.join("gw.key"));
        std::fs::write(&cert, leaf.pem()).unwrap();
        std::fs::write(&key, leaf_key.serialize_pem()).unwrap();
        let (id, cfg) = tls_node(&dir, vec![], true, crate::GatewayTlsConfig {
            cert_pem_path: Some(cert), key_pem_path: Some(key), bulk_ca_pem_path: Some(mesh.join("ca-cert.pem")),
        });
        let a = GossipAgent::new(id, cfg);
        let got = a.start().await;
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
        match got {
            Err(crate::GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", reason }) => {
                assert!(reason.contains("not a mesh certificate"), "{reason}");
            }
            other => panic!("a fleet-issued, non-mesh gateway certificate must refuse the start, got {other:?}"),
        }
    }

    /// **Round 2, N2: no fetch rides an earlier handshake.** A client that has fetched once from a
    /// caller fetches again after the caller's key is judged revoked — here the responder's view is
    /// the caller's own standing set, which its fleet check reads, with the key marked revoked by a
    /// counted revocation and no rotation. The second fetch must be refused: with connection pooling
    /// on, it rode the first connection; with session resumption on, it resumed the first session;
    /// neither runs the verifier.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_fetch_after_the_callers_key_is_revoked_is_refused_on_a_client_that_fetched_before() {
        let dir = std::env::temp_dir().join(format!("bulk-revoke-{}-{}", std::process::id(), alloc_port()));
        let (id, cfg) = tls_node(&dir, vec![], true, crate::GatewayTlsConfig::default());
        let port = cfg.http_port.unwrap();
        let c = Arc::new(GossipAgent::new(id.clone(), cfg));
        c.start().await.unwrap();
        let tls = Arc::clone(c.task_ctx.tls.get().unwrap());
        let responder = BulkTransport::with_limits(0, Duration::from_secs(5), 1, false, 1 << 20);
        responder.install_tls_trust(bulk_tls_client_config(Some(tls.fleet_server_verifier()), None).unwrap()).unwrap();
        let payload = Bytes::from_static(b"staged payload");
        let _staged = c.task_ctx.bulk_transport.stage(7, payload.clone());
        let url = format!("https://127.0.0.1:{port}/bulk/{:016x}", 7u64);
        let first = responder.fetch(&url, true).await;
        let key = tls.verifying_key_bytes();
        c.task_ctx.key_standing.set(&id, mycelium_core::removal::NodeStanding {
            anchored: None,
            revoked: [key].into_iter().collect(),
            counted: [key].into_iter().collect(),
        });
        let second = responder.fetch(&url, true).await;
        c.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(first.ok(), Some(payload), "the control: the first fetch verifies and succeeds");
        assert!(second.is_err(), "the caller's key is revoked: the second fetch must not ride the first handshake, got {second:?}");
    }
}
