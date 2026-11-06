//! The gateway's accept loop, one for both listeners (external review 2026-10-31, F5;
//! `docs/plans/external-review-2026-10-31.md` row R5).
//!
//! Before R5 the plaintext listener was `axum::serve` and the HTTPS one a hand-rolled loop, and neither
//! bounded a connection that never spoke. The HTTPS loop spawned a task per socket that awaited the TLS
//! handshake with no deadline and no cap. On both, hyper's HTTP/1 header-read timer was inert: it needs a
//! `Timer`, which neither builder set, and even with one it starts only after hyper-util has read enough of
//! the stream to choose HTTP/1 or HTTP/2 — up to the 24-byte HTTP/2 preface — so a socket that sends
//! nothing, or stops inside `PRI * HTTP/2…`, was never timed at all.
//!
//! Each connection now:
//! 1. takes one of `gateway_max_connections` permits **before** it is accepted, so at the bound the gateway
//!    stops accepting and a further client waits in the listen backlog until a connection closes;
//! 2. under `gateway_tls`, completes its TLS handshake within `handshake_timeout_ms`;
//! 3. delivers its preface within `handshake_timeout_ms` of being ready to ([`PrefaceDeadline`]): on HTTP/1 the
//!    first byte that tells hyper-util it is not HTTP/2, on HTTP/2 the 24-byte connection preface **and the
//!    header of the frame after it** (the client's SETTINGS) — a bare preface then silence held a permit forever
//!    before #719's review;
//! 4. is served by hyper with a timer, so HTTP/1's header-read bound (hyper's default, 30 s) is live for the
//!    head of every request after that, and HTTP/2 sends a keep-alive PING after `handshake_timeout_ms` of
//!    quiet and closes a connection that does not answer it within `handshake_timeout_ms`.
//!
//! On shutdown the loop stops accepting; a connection in its handshake is dropped, and one past it is shut
//! down gracefully, as `axum::serve` did (a connection waiting on its preface or between requests closes at
//! once; a request in flight completes). The loop returns when every connection has closed. The node's
//! shutdown drain bounds that wait and aborts the server task after it, which drops the [`JoinSet`] and
//! so every connection still open.
//!
//! An accept error backs off as `axum::serve` did: a connection error (the client went away before it was
//! accepted) is skipped at once; any other (`EMFILE`, `ENFILE`) waits a second, so a node out of descriptors does
//! not spin a core on its accept loop.
//!
//! **Not bounded, stated:** a connection that keeps talking holds its permit, as on the gossip listener — an
//! HTTP/2 client that answers PINGs and sends no request is one; a long-running stream (an SSE route) holds its
//! permit while it streams, so enough of them fill the cap and `/health` and `/ready` wait with every other
//! request (no slot is reserved for the probes). The deadlines are real-socket deadlines outside the replay
//! kernel, like the socket they bound (replay inventory §2.4).
//!
//! [`JoinSet`]: tokio::task::JoinSet

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use mycelium_core::bounds::TransportBounds;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{watch, Semaphore};
use tracing::{debug, warn};

/// The gateway's accept bounds, read from `GossipConfig` when the gateway starts.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    /// `gateway_max_connections`.
    pub(super) max_connections: usize,
    /// `handshake_timeout_ms`: for the TLS handshake, then again for the preface.
    pub(super) preface: Duration,
}

/// The HTTP/2 connection preface (RFC 9113 §3.4): what hyper-util reads before it chooses a protocol.
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// An HTTP/2 frame header (RFC 9113 §4.1): after the preface the deadline waits for one — the client's SETTINGS,
/// which RFC 9113 §3.4 says must follow — before it hands the connection to hyper's keep-alive.
const H2_FRAME_HEADER: usize = 9;

/// How long the accept loop waits after an accept error that is not the client's (`axum::serve`'s figure).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_secs(1);

/// The gateway's source of connections: a `TcpListener`, or a test's stand-in that fails on demand.
pub(super) trait Accept: Send + 'static {
    fn next_connection(&self) -> impl Future<Output = io::Result<tokio::net::TcpStream>> + Send;
}

impl Accept for tokio::net::TcpListener {
    async fn next_connection(&self) -> io::Result<tokio::net::TcpStream> {
        self.accept().await.map(|(tcp, _peer)| tcp)
    }
}

/// How long to wait before the next accept after `e`: `None` for an error on one client's connection (it
/// reset or aborted before it was accepted — the next one is unaffected), [`ACCEPT_ERROR_BACKOFF`] for any other,
/// which is the process's (out of descriptors, out of memory) and recurs at once if retried at once.
fn accept_error_backoff(e: &io::Error) -> Option<Duration> {
    match e.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset => None,
        _ => Some(ACCEPT_ERROR_BACKOFF),
    }
}

/// Holds one open gateway connection on the `gateway_connections_open` gauge.
struct OpenConnection(Arc<TransportBounds>);

impl OpenConnection {
    fn new(bounds: &Arc<TransportBounds>) -> Self {
        bounds.gateway_connection_opened();
        Self(Arc::clone(bounds))
    }
}

impl Drop for OpenConnection {
    fn drop(&mut self) { self.0.gateway_connection_closed(); }
}

/// Serve `app` on `listener` until `shutdown_rx` turns `true`, over TLS when `tls` is set.
pub(super) async fn serve<L: Accept>(
    listener: L,
    app: axum::Router,
    #[cfg(feature = "tls")] tls: Option<Arc<rustls::ServerConfig>>,
    bounds: Bounds,
    counters: Arc<TransportBounds>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    #[cfg(feature = "tls")]
    let acceptor = tls.map(tokio_rustls::TlsAcceptor::from);
    let permits = Arc::new(Semaphore::new(bounds.max_connections));
    let mut conns = tokio::task::JoinSet::new();
    loop {
        // Reap finished connections so the set holds only open ones.
        while conns.try_join_next().is_some() {}
        let permit = match Arc::clone(&permits).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // Every permit is held: the loop stops accepting until one returns (counted once per wait).
                counters.count_gateway_cap_wait();
                tokio::select! {
                    _ = shutdown_rx.wait_for(|v| *v) => break,
                    p = Arc::clone(&permits).acquire_owned() => p.expect("the gateway's permits are never closed"),
                }
            }
        };
        let accepted = tokio::select! {
            _ = shutdown_rx.wait_for(|v| *v) => break,
            accepted = listener.next_connection() => accepted,
        };
        let tcp = match accepted {
            Ok(tcp) => tcp,
            Err(e) => {
                if let Some(wait) = accept_error_backoff(&e) {
                    warn!("gateway accept error: {e}; accepting again in {wait:?}");
                    tokio::select! {
                        _ = shutdown_rx.wait_for(|v| *v) => break,
                        _ = tokio::time::sleep(wait) => {}
                    }
                } else {
                    debug!("gateway accept: a client's connection failed before it was accepted: {e}");
                }
                continue;
            }
        };
        let app = app.clone();
        let srx = shutdown_rx.clone();
        let counters = Arc::clone(&counters);
        #[cfg(feature = "tls")]
        let acceptor = acceptor.clone();
        conns.spawn(async move {
            let _permit = permit;
            let _open = OpenConnection::new(&counters);
            #[cfg(feature = "tls")]
            if let Some(acceptor) = acceptor {
                let mut handshake_rx = srx.clone();
                let stream = tokio::select! {
                    _ = handshake_rx.wait_for(|v| *v) => return,
                    r = tokio::time::timeout(bounds.preface, acceptor.accept(tcp)) => match r {
                        Ok(Ok(s))  => s,
                        Ok(Err(_)) => return, // a failed handshake — drop quietly, as before
                        Err(_)     => {
                            counters.count_gateway_timeout();
                            debug!(timeout = ?bounds.preface,
                                   "gateway TLS handshake timed out (handshake_timeout_ms)");
                            return;
                        }
                    },
                };
                return serve_connection(stream, app, bounds.preface, &counters, srx).await;
            }
            serve_connection(tcp, app, bounds.preface, &counters, srx).await
        });
    }
    drop(listener);
    while conns.join_next().await.is_some() {}
    Ok(())
}

/// Serve one connection whose transport is ready: its preface under `preface`, the rest under hyper's
/// own bounds; shut down gracefully when `shutdown_rx` turns `true`.
async fn serve_connection<IO>(
    io: IO,
    app: axum::Router,
    preface: Duration,
    counters: &Arc<TransportBounds>,
    mut shutdown_rx: watch::Receiver<bool>,
)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;
    use hyper_util::service::TowerToHyperService;

    let mut builder = ConnBuilder::new(TokioExecutor::new());
    // The timer makes `header_read_timeout` (hyper's default, 30 s) live on HTTP/1.
    builder.http1().timer(TokioTimer::new());
    // As `axum::serve` configures it: the CONNECT protocol, for WebSockets over HTTP/2. And a keep-alive from
    // the same bound (#719's review, finding 1): HTTP/2 has no header-read timer, so a client past its SETTINGS
    // that never sends a request is found by a PING it does not answer.
    builder.http2()
        .timer(TokioTimer::new())
        .enable_connect_protocol()
        .keep_alive_interval(Some(preface))
        .keep_alive_timeout(preface);
    let io = TokioIo::new(PrefaceDeadline::new(io, preface, Arc::clone(counters)));
    let conn = builder.serve_connection_with_upgrades(io, TowerToHyperService::new(app));
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => return,
        _ = shutdown_rx.wait_for(|v| *v) => conn.as_mut().graceful_shutdown(),
    }
    let _ = conn.await;
}

/// A transport whose reads fail `TimedOut` if the connection's preface has not arrived by a deadline. It
/// disarms where hyper's own bounds take over: on HTTP/1 at the first byte that departs from the HTTP/2 preface
/// (hyper-util chooses HTTP/1 there and the header-read timer runs); on HTTP/2 only once the 24-byte preface
/// **and the 9-byte header of the frame after it** have arrived, after which the keep-alive PING bounds a client
/// that goes quiet. Disarming at the preface's last byte left a bare preface unbounded (#719's review, finding 1).
/// Writes pass through.
struct PrefaceDeadline<IO> {
    inner: IO,
    /// `None` once disarmed.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    /// How many bytes read so far match the HTTP/2 preface, then (past 24) how many of the first frame's header.
    matched: usize,
    /// Counts a deadline that fires (`gateway_connections_timed_out`); `None` in the unit tests.
    counters: Option<Arc<TransportBounds>>,
}

impl<IO> PrefaceDeadline<IO> {
    fn new(inner: IO, within: Duration, counters: Arc<TransportBounds>) -> Self {
        Self { inner, deadline: Some(Box::pin(tokio::time::sleep(within))), matched: 0, counters: Some(counters) }
    }

    /// Feed newly read bytes; disarm once hyper's own bound for the chosen protocol has taken over.
    fn observe(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.matched < H2_PREFACE.len() && b != H2_PREFACE[self.matched] {
                self.deadline = None; // HTTP/1: the header-read timer runs from here
                return;
            }
            self.matched += 1;
            if self.matched == H2_PREFACE.len() + H2_FRAME_HEADER {
                self.deadline = None; // HTTP/2: the preface and a frame header; the keep-alive runs from here
                return;
            }
        }
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for PrefaceDeadline<IO> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if this.deadline.is_some() {
                    let fresh = buf.filled()[before..].to_vec();
                    this.observe(&fresh);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Pending => {
                if let Some(deadline) = this.deadline.as_mut()
                    && deadline.as_mut().poll(cx).is_ready()
                {
                    this.deadline = None;
                    if let Some(c) = &this.counters { c.count_gateway_timeout(); }
                    debug!("gateway connection sent no preface within handshake_timeout_ms; closing it");
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut,
                        "no request preface within handshake_timeout_ms")));
                }
                Poll::Pending
            }
            other => other,
        }
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for PrefaceDeadline<IO> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool { self.inner.is_write_vectored() }
}

#[cfg(test)]
mod preface_tests {
    use super::*;

    fn armed() -> PrefaceDeadline<()> {
        PrefaceDeadline {
            inner: (), deadline: Some(Box::pin(tokio::time::sleep(Duration::from_secs(60)))), matched: 0, counters: None,
        }
    }

    #[tokio::test]
    async fn an_http1_request_disarms_at_its_first_byte() {
        let mut p = armed();
        p.observe(b"G");
        assert!(p.deadline.is_none(), "'G' departs from the HTTP/2 preface: hyper-util chooses HTTP/1 here");
    }

    #[tokio::test]
    async fn a_post_disarms_at_its_second_byte() {
        let mut p = armed();
        p.observe(b"P");
        assert!(p.deadline.is_some(), "'P' is still a prefix of the HTTP/2 preface");
        p.observe(b"O");
        assert!(p.deadline.is_none(), "'PO' is not");
    }

    #[tokio::test]
    async fn a_partial_http2_preface_stays_armed() {
        let mut p = armed();
        p.observe(&H2_PREFACE[..23]);
        assert!(p.deadline.is_some(), "23 of the preface's 24 bytes: hyper-util is still reading");
    }

    /// #719's review, finding 1: the whole preface alone must not disarm — nothing bounds the HTTP/2 handshake
    /// after it until a frame arrives.
    #[tokio::test]
    async fn the_whole_preface_stays_armed_until_a_frame_header_follows() {
        let mut p = armed();
        p.observe(H2_PREFACE);
        assert!(p.deadline.is_some(), "the preface alone: no frame yet, the keep-alive has nothing to bound");
        p.observe(&[0, 0, 0, 0x4, 0, 0, 0, 0]);
        assert!(p.deadline.is_some(), "8 of the frame header's 9 bytes");
        p.observe(&[0]);
        assert!(p.deadline.is_none(), "the preface and a whole frame header: hyper's HTTP/2 bounds take over");
    }

    #[tokio::test]
    async fn bytes_after_the_preface_are_not_compared_with_it() {
        let mut p = armed();
        let mut bytes = H2_PREFACE.to_vec();
        bytes.extend_from_slice(&[0xff; 9]); // any frame header, not a continuation of the preface
        p.observe(&bytes);
        assert!(p.deadline.is_none());
    }

    #[test]
    fn a_client_side_accept_error_is_skipped_and_a_process_one_backs_off() {
        for kind in [io::ErrorKind::ConnectionAborted, io::ErrorKind::ConnectionReset, io::ErrorKind::ConnectionRefused] {
            assert_eq!(accept_error_backoff(&io::Error::from(kind)), None, "{kind:?} is one client's");
        }
        let emfile = io::Error::from_raw_os_error(24); // EMFILE on Linux and macOS
        assert_eq!(accept_error_backoff(&emfile), Some(ACCEPT_ERROR_BACKOFF), "out of descriptors: back off");
        assert_eq!(accept_error_backoff(&io::Error::other("anything else")), Some(ACCEPT_ERROR_BACKOFF));
    }

    /// A listener whose every accept fails: with `kind` a process error, the loop must not spin on it.
    struct Failing { calls: Arc<std::sync::atomic::AtomicU64>, kind: io::ErrorKind }

    impl Accept for Failing {
        async fn next_connection(&self) -> io::Result<tokio::net::TcpStream> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tokio::task::yield_now().await;
            Err(io::Error::from(self.kind))
        }
    }

    async fn accepts_in(kind: io::ErrorKind, window: Duration) -> u64 {
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (tx, rx) = watch::channel(false);
        let listener = Failing { calls: Arc::clone(&calls), kind };
        let bounds = Bounds { max_connections: 4, preface: Duration::from_secs(1) };
        let server = tokio::spawn(serve(
            listener, axum::Router::new(),
            #[cfg(feature = "tls")] None,
            bounds, Arc::new(TransportBounds::default()), rx,
        ));
        tokio::time::sleep(window).await;
        let n = calls.load(std::sync::atomic::Ordering::Relaxed);
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), server).await
            .expect("shutdown ends the loop, during its backoff too").unwrap().unwrap();
        n
    }

    /// #719's review, finding 2: an `EMFILE` loop retried at once spins a core.
    #[tokio::test]
    async fn an_accept_loop_out_of_descriptors_backs_off() {
        let n = accepts_in(io::ErrorKind::Other, Duration::from_millis(500)).await;
        assert!(n <= 2, "a process error waits {ACCEPT_ERROR_BACKOFF:?} between accepts; {n} in 500 ms");
    }

    #[tokio::test]
    async fn an_accept_loop_skips_a_client_error_at_once() {
        let n = accepts_in(io::ErrorKind::ConnectionAborted, Duration::from_millis(200)).await;
        assert!(n > 10, "a client's aborted connection is skipped without waiting; {n} in 200 ms");
    }
}
