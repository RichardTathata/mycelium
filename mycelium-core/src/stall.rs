//! A progress bound on a socket (the adversarial review of #602, finding 1).
//!
//! Row B first bounded *whole frames*: the reader gave a connection's first frame — header and body —
//! `handshake_timeout_ms`, and the writer gave each frame `peer_write_timeout_ms`. A frame is up to
//! ~10 MB (an anti-entropy chunk, a large value), so a slow but healthy link was cut mid-frame on every
//! attempt and the data never crossed. What a stalled peer and a slow one differ in is **progress**,
//! so that is what [`StallGuard`] bounds, while it is armed:
//!
//! - **no progress** — no byte moved for `stall` (`peer_read_stall_timeout_ms` reading,
//!   `peer_write_stall_timeout_ms` writing);
//! - **a rate floor** — once `stall` has passed since arming, fewer than `min_rate` bytes per second
//!   moved over the time beyond it (`moved < min_rate × (elapsed − stall)`), checked at each progress.
//!   This is what stops a peer from holding a socket by trickling one byte per window; `0` disables it.
//!
//! Disarmed it is a pass-through: the reader arms it only *inside* a frame (from the first byte of a
//! header), so an idle link between frames is the idle bound's business, not this one; the writer arms
//! it for each batch it writes. Expiry is an `io::ErrorKind::TimedOut` error whose message names the
//! cause ([`is_stall`] recognises it). The timer is a real-socket deadline, outside the replay kernel
//! like the socket it bounds (`docs/design/replay-nondeterminism-inventory.md` §2.4).

use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The progress bound a [`StallGuard`] enforces while armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallBound {
    /// Longest time without a byte moved.
    pub stall:    Duration,
    /// Rate floor in bytes per second past the first `stall` (`peer_min_rate_bytes_per_sec`); `0` = none.
    pub min_rate: u64,
}

impl StallBound {
    /// The bound on frames being received: `peer_read_stall_timeout_ms` and the optional floor.
    pub fn read_from_config(cfg: &crate::config::GossipConfig) -> Self {
        Self {
            stall:    Duration::from_millis(cfg.peer_read_stall_timeout_ms),
            min_rate: cfg.peer_min_rate_bytes_per_sec,
        }
    }

    /// The bound on batches being sent: `peer_write_stall_timeout_ms`, no floor (a sender cannot tell
    /// a slow link from a receiver applying what it read).
    pub fn write_from_config(cfg: &crate::config::GossipConfig) -> Self {
        Self { stall: Duration::from_millis(cfg.peer_write_stall_timeout_ms), min_rate: 0 }
    }
}

const NO_PROGRESS: &str = "peer made no progress within its stall bound";
const BELOW_FLOOR: &str = "peer moved bytes below peer_min_rate_bytes_per_sec";

/// Whether `e` is a [`StallGuard`] expiry.
pub fn is_stall(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::TimedOut
        && e.get_ref().is_some_and(|m| { let m = m.to_string(); m == NO_PROGRESS || m == BELOW_FLOOR })
}

/// A stream whose reads and writes must make progress while armed. See the module doc.
pub struct StallGuard<S> {
    inner:   S,
    bound:   Option<StallBound>,
    started: Option<tokio::time::Instant>,
    moved:   u64,
    timer:   Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<S> StallGuard<S> {
    /// Wraps `inner`, disarmed.
    pub fn new(inner: S) -> Self {
        Self { inner, bound: None, started: None, moved: 0, timer: None }
    }

    /// Arms the guard with `bound` from now (`None` disarms). Re-arming restarts the rate floor.
    pub fn arm(&mut self, bound: Option<StallBound>) {
        self.started = bound.map(|_| tokio::time::Instant::now());
        self.bound = bound;
        self.moved = 0;
        self.timer = None;
    }

    /// The wrapped stream.
    pub fn get_ref(&self) -> &S { &self.inner }

    fn progressed(&mut self, n: usize) -> io::Result<()> {
        if n == 0 { return Ok(()); }
        self.timer = None;
        self.moved = self.moved.saturating_add(n as u64);
        if let (Some(b), Some(start)) = (self.bound, self.started)
            && b.min_rate > 0
        {
            let past = start.elapsed().saturating_sub(b.stall);
            if !past.is_zero() && (self.moved as f64) < b.min_rate as f64 * past.as_secs_f64() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, BELOW_FLOOR));
            }
        }
        Ok(())
    }

    /// Called when the inner stream returned `Pending`: arms the no-progress timer and reports expiry.
    fn pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(b) = self.bound else { return Poll::Pending };
        let timer = self.timer.get_or_insert_with(|| Box::pin(tokio::time::sleep(b.stall)));
        match timer.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, NO_PROGRESS))),
            Poll::Pending   => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StallGuard<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => Poll::Ready(this.progressed(buf.filled().len() - before)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending       => this.pending(cx),
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StallGuard<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, data) {
            Poll::Ready(Ok(n)) => Poll::Ready(this.progressed(n).map(|()| n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => this.pending(cx).map(|r| r.map(|()| 0)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_flush(cx) {
            Poll::Ready(r) => { if r.is_ok() { this.timer = None; } Poll::Ready(r) }
            Poll::Pending  => this.pending(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn bound(stall_ms: u64, min_rate: u64) -> Option<StallBound> {
        Some(StallBound { stall: Duration::from_millis(stall_ms), min_rate })
    }

    #[tokio::test]
    async fn steady_progress_outlives_any_whole_frame_budget() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut g = StallGuard::new(rx);
        g.arm(bound(200, 0));
        let w = tokio::spawn(async move {
            for _ in 0..20 {
                tx.write_all(&[1u8; 64]).await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await; // 1 s in all, 50 ms gaps
            }
        });
        let mut buf = vec![0u8; 20 * 64];
        g.read_exact(&mut buf).await.expect("progress every 50 ms is within a 200 ms stall bound");
        w.await.unwrap();
    }

    #[tokio::test]
    async fn a_stall_mid_frame_times_out() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut g = StallGuard::new(rx);
        g.arm(bound(150, 0));
        tx.write_all(&[1u8; 10]).await.unwrap();
        let mut buf = [0u8; 20];
        let e = g.read_exact(&mut buf).await.expect_err("ten bytes then silence");
        assert!(is_stall(&e), "{e}");
        drop(tx);
    }

    #[tokio::test]
    async fn a_trickle_below_the_floor_times_out() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut g = StallGuard::new(rx);
        g.arm(bound(100, 10_000)); // 10 kB/s floor past the first 100 ms
        let w = tokio::spawn(async move {
            for _ in 0..40 {
                if tx.write_all(&[1u8; 1]).await.is_err() { return; }
                tokio::time::sleep(Duration::from_millis(30)).await; // ~33 B/s, never 100 ms silent
            }
        });
        let mut buf = [0u8; 40];
        let e = g.read_exact(&mut buf).await.expect_err("a trickle under the floor");
        assert!(is_stall(&e), "{e}");
        w.abort();
    }

    #[tokio::test]
    async fn disarmed_it_waits_forever() {
        let (_tx, rx) = tokio::io::duplex(64);
        let mut g = StallGuard::new(rx);
        let mut buf = [0u8; 1];
        assert!(tokio::time::timeout(Duration::from_millis(200), g.read_exact(&mut buf)).await.is_err());
    }
}

#[cfg(test)]
mod default_tests {
    use super::*;

    /// #602's re-review, findings 2 and 3: the defaults cut healthy peers. A per-connection floor of
    /// 8 KiB/s tripped every sender of a joiner whose link is shared by eight peers at 512 kbit/s, and a
    /// 15 s no-progress bound on the writer was shorter than a peer's apply of one anti-entropy chunk
    /// into an fsync WAL (measured 2026-10-10 on a developer Mac: 41.8 s for 9 000 × 1 KiB, 329 s for
    /// 70 000 × 64 B). The defaults must not cut either.
    #[test]
    fn the_defaults_do_not_cut_a_shared_link_or_a_slow_apply() {
        let cfg = crate::config::GossipConfig::default();
        let r = StallBound::read_from_config(&cfg);
        assert_eq!(r.min_rate, 0, "no per-connection rate floor by default");
        assert!(r.stall >= Duration::from_secs(60), "{:?}", r.stall);
        let w = StallBound::write_from_config(&cfg);
        assert!(w.stall >= Duration::from_secs(600), "the writer must outlast a 329 s chunk apply; got {:?}", w.stall);
    }
}
