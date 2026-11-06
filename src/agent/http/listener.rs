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
//! 3. delivers its preface — the bytes hyper-util reads to choose a protocol — within `handshake_timeout_ms`
//!    of being ready to ([`PrefaceDeadline`]);
//! 4. is served by hyper with a timer, so HTTP/1's header-read bound (hyper's default, 30 s) is live for the
//!    head of every request after that.
//!
//! On shutdown the loop stops accepting; a connection in its handshake is dropped, and one past it is shut
//! down gracefully, as `axum::serve` did (a connection waiting on its preface or between requests closes at
//! once; a request in flight completes). The loop returns when every connection has closed. The node's
//! shutdown drain bounds that wait and aborts the server task after it, which drops the [`JoinSet`] and
//! so every connection still open.
//!
//! **Not bounded, stated:** a connection that keeps talking holds its permit, as on the gossip listener; an
//! HTTP/2 connection past its preface is bounded by the cap, not by a timer (hyper has no HTTP/2 header
//! timer); a long-running stream (an SSE route) holds its permit while it streams. The deadlines are real-socket
//! deadlines outside the replay kernel, like the socket they bound (replay inventory §2.4).
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

/// Serve `app` on `listener` until `shutdown_rx` turns `true`, over TLS when `tls` is set.
pub(super) async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    #[cfg(feature = "tls")] tls: Option<Arc<rustls::ServerConfig>>,
    bounds: Bounds,
    mut shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    #[cfg(feature = "tls")]
    let acceptor = tls.map(tokio_rustls::TlsAcceptor::from);
    let permits = Arc::new(Semaphore::new(bounds.max_connections));
    let mut conns = tokio::task::JoinSet::new();
    loop {
        // Reap finished connections so the set holds only open ones.
        while conns.try_join_next().is_some() {}
        let permit = tokio::select! {
            _ = shutdown_rx.wait_for(|v| *v) => break,
            p = Arc::clone(&permits).acquire_owned() => p.expect("the gateway's permits are never closed"),
        };
        let tcp = tokio::select! {
            _ = shutdown_rx.wait_for(|v| *v) => break,
            accepted = listener.accept() => match accepted {
                Ok((tcp, _peer)) => tcp,
                Err(e) => { warn!("gateway accept error: {e}"); continue; }
            },
        };
        let app = app.clone();
        let srx = shutdown_rx.clone();
        #[cfg(feature = "tls")]
        let acceptor = acceptor.clone();
        conns.spawn(async move {
            let _permit = permit;
            #[cfg(feature = "tls")]
            if let Some(acceptor) = acceptor {
                let mut handshake_rx = srx.clone();
                let stream = tokio::select! {
                    _ = handshake_rx.wait_for(|v| *v) => return,
                    r = tokio::time::timeout(bounds.preface, acceptor.accept(tcp)) => match r {
                        Ok(Ok(s))  => s,
                        Ok(Err(_)) => return, // a failed handshake — drop quietly, as before
                        Err(_)     => {
                            debug!(timeout = ?bounds.preface,
                                   "gateway TLS handshake timed out (handshake_timeout_ms)");
                            return;
                        }
                    },
                };
                return serve_connection(stream, app, bounds.preface, srx).await;
            }
            serve_connection(tcp, app, bounds.preface, srx).await
        });
    }
    drop(listener);
    while conns.join_next().await.is_some() {}
    Ok(())
}

/// Serve one connection whose transport is ready: its preface under `preface`, the rest under hyper's
/// own bounds; shut down gracefully when `shutdown_rx` turns `true`.
async fn serve_connection<IO>(io: IO, app: axum::Router, preface: Duration, mut shutdown_rx: watch::Receiver<bool>)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;
    use hyper_util::service::TowerToHyperService;

    let mut builder = ConnBuilder::new(TokioExecutor::new());
    // The timer makes `header_read_timeout` (hyper's default, 30 s) live on HTTP/1.
    builder.http1().timer(TokioTimer::new());
    // As `axum::serve` configures it: the CONNECT protocol, for WebSockets over HTTP/2.
    builder.http2().timer(TokioTimer::new()).enable_connect_protocol();
    let io = TokioIo::new(PrefaceDeadline::new(io, preface));
    let conn = builder.serve_connection_with_upgrades(io, TowerToHyperService::new(app));
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => return,
        _ = shutdown_rx.wait_for(|v| *v) => conn.as_mut().graceful_shutdown(),
    }
    let _ = conn.await;
}

/// A transport whose reads fail `TimedOut` if the connection's preface — the bytes hyper-util reads to
/// choose HTTP/1 or HTTP/2 — has not arrived by a deadline. It disarms at the byte where hyper-util decides:
/// the first byte that departs from the HTTP/2 preface (hyper's HTTP/1 header timer takes over), or the
/// preface's last byte (HTTP/2). Writes pass through.
struct PrefaceDeadline<IO> {
    inner: IO,
    /// `None` once disarmed.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    /// How many bytes read so far match the HTTP/2 preface.
    matched: usize,
}

impl<IO> PrefaceDeadline<IO> {
    fn new(inner: IO, within: Duration) -> Self {
        Self { inner, deadline: Some(Box::pin(tokio::time::sleep(within))), matched: 0 }
    }

    /// Feed newly read bytes; disarm once hyper-util would have chosen a protocol.
    fn observe(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b != H2_PREFACE[self.matched] {
                self.deadline = None;
                return;
            }
            self.matched += 1;
            if self.matched == H2_PREFACE.len() {
                self.deadline = None;
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
        PrefaceDeadline { inner: (), deadline: Some(Box::pin(tokio::time::sleep(Duration::from_secs(60)))), matched: 0 }
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
    async fn a_partial_http2_preface_stays_armed_and_the_whole_one_disarms() {
        let mut p = armed();
        p.observe(&H2_PREFACE[..23]);
        assert!(p.deadline.is_some(), "23 of the preface's 24 bytes: hyper-util is still reading");
        p.observe(&H2_PREFACE[23..]);
        assert!(p.deadline.is_none(), "the whole preface: hyper-util chooses HTTP/2");
    }
}
