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
            #[cfg(all(feature = "gateway", feature = "tls"))]
            fetch_timeout:    _fetch_timeout,
            #[cfg(feature = "gateway")]
            // A peer's redirect is not a peer (realignment repairs R3): the URL is built from the
            // sender's address, and a 3xx could point the fetch anywhere the node can reach.
            client: crate::agent::egress_client::without_redirects()
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
        let client = crate::agent::egress_client::without_redirects()
            .timeout(self.fetch_timeout)
            .use_preconfigured_tls(tls)
            .build()?;
        let _ = self.tls_client.set(client);
        Ok(())
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
        let mut resp = client.get(url).send().await.map_err(FetchError::Http)?
            .error_for_status().map_err(FetchError::Http)?;
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

/// Background task that handles incoming `INVOKE_BULK` signals for a given kind.
#[cfg(feature = "gateway")]
pub(super) async fn bulk_serve_task<F, Fut>(
    ctx:         Arc<TaskCtx>,
    kind:        Arc<str>,
    handler:     Arc<F>,
    mut cancel_rx:   oneshot::Receiver<()>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
)
where
    F: Fn(NodeId, Bytes) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Bytes> + Send + 'static,
{
    let mut rx = ctx.signal_handlers.register_with_capacity(
        Arc::from(signal_kind::INVOKE_BULK),
        256,
    );

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
            Err(e) => { warn!(%url, "bulk_serve: fetch failed: {e}"); return; }
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
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(&pem)) {
        let cert = cert.map_err(|e| refuse(format!("{}: {e}", path.display())))?;
        roots.add(cert).map_err(|e| refuse(format!("{}: {e}", path.display())))?;
    }
    if roots.is_empty() {
        return Err(refuse(format!("{}: holds no certificate", path.display())));
    }
    Ok(roots)
}

/// `start()`'s refusal (and `mycelium doctor`'s): a gateway on an operator certificate with no
/// roots for its peers' certificates, or a `bulk_ca_pem_path` that is not usable. The node-cert
/// reuse path needs nothing here — its peers' gateways serve fleet-CA certificates, and without
/// `[tls]` the gateway itself refuses to start (`prepare_gateway`).
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn check_bulk_trust(cfg: &crate::config::GossipConfig) -> Result<(), crate::error::GossipError> {
    let Some(gw) = &cfg.gateway_tls else { return Ok(()) };
    match (&gw.bulk_ca_pem_path, &gw.cert_pem_path) {
        (Some(path), _) => bulk_roots_from_pem(path).map(|_| ()),
        (None, Some(_)) => Err(crate::error::GossipError::InvalidField {
            field: "gateway_tls.bulk_ca_pem_path",
            reason: "the gateway serves an operator certificate (cert_pem_path), so a bulk fetch from a peer's \
                     gateway has no roots to verify it with: the fleet CA did not sign it, and a bulk fetch is \
                     never retried in plaintext. Set bulk_ca_pem_path to the CA that signs the gateways' \
                     certificates (each must name its node's IP address)"
                .into(),
        }),
        (None, None) => Ok(()),
    }
}

/// Install the `https` fetch client on `start()`: `bulk_ca_pem_path`'s roots when set, else the
/// fleet's client configuration (the CA chain, the removal and revocation checks), else nothing —
/// a node with neither refuses an `https` ticket.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn install_bulk_trust(ctx: &TaskCtx, cfg: &crate::config::GossipConfig) -> Result<(), crate::error::GossipError> {
    let ca = cfg.gateway_tls.as_ref().and_then(|g| g.bulk_ca_pem_path.as_ref());
    let client = match ca {
        Some(path) => {
            let roots = bulk_roots_from_pem(path)?;
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| crate::error::GossipError::InvalidField { field: "gateway_tls.bulk_ca_pem_path", reason: e.to_string() })?
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
        None => match ctx.tls.get() {
            Some(tls) => (*tls.client_config()).clone(),
            None => return Ok(()),
        },
    };
    ctx.bulk_transport.install_tls_trust(client).map_err(|e| crate::error::GossipError::InvalidField {
        field: "gateway_tls",
        reason: format!("the bulk https client could not be built: {e}"),
    })
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
        std::fs::create_dir_all(dir).unwrap();
        let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_p = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_p.self_signed(&ca_key).unwrap();
        let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_p = rcgen::CertificateParams::new(vec![]).unwrap();
        leaf_p.subject_alt_names = vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())];
        let leaf = leaf_p.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        let (ca_path, cert_path, key_path) = (dir.join(format!("{tag}-ca.pem")), dir.join(format!("{tag}-gw.pem")), dir.join(format!("{tag}-gw.key")));
        std::fs::write(&ca_path, ca.pem()).unwrap();
        std::fs::write(&cert_path, leaf.pem()).unwrap();
        std::fs::write(&key_path, leaf_key.serialize_pem()).unwrap();
        (ca_path, cert_path, key_path)
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
            let (ca, cert, key) = operator_pki(&dir, "op");
            let (other_ca, _, _) = operator_pki(&dir, "other");
            let mk = |boot: Vec<NodeId>, bulk_ca: std::path::PathBuf| {
                let port = alloc_port();
                let mut cfg = GossipConfig::default();
                cfg.bind_port = port;
                cfg.http_port = Some(alloc_port());
                cfg.http_addr = "127.0.0.1".to_string();
                cfg.bootstrap_peers = boot;
                cfg.health_check_max_jitter_ms = 1;
                cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.join("mesh"), ..Default::default() });
                cfg.gateway_tls = Some(crate::GatewayTlsConfig { cert_pem_path: Some(cert.clone()), key_pem_path: Some(key.clone()), bulk_ca_pem_path: Some(bulk_ca) });
                (NodeId::new("127.0.0.1", port).unwrap(), cfg)
            };
            let (ida, cfga) = mk(vec![], ca.clone());
            let (idb, cfgb) = mk(vec![ida.clone()], if trusted { ca.clone() } else { other_ca.clone() });
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
            if trusted {
                assert_eq!(got, Ok(payload), "the operator CA verifies the caller's gateway");
            } else {
                assert_eq!(got, Err(BulkError::Timeout), "an untrusted certificate is refused and nothing is fetched in plaintext");
            }
            a.shutdown().await;
            b.shutdown().await;
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
