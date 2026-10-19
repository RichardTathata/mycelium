//! **Outbound frames not yet on the wire**, counted node-wide, so a graceful shutdown can wait for them.
//!
//! A frame a node sends crosses three hands: a gossip shard queue (`GossipTxs`), the shard worker that
//! fans it out, and each peer writer's queue and write (`WriterTx` → `run_peer_writer`). Reading queue
//! emptiness cannot tell whether that is done — a frame a shard worker or a writer has dequeued is in
//! no queue — so `flush_outbound_before_shutdown` used to end while the shutdown sweep's last tombstone
//! was still in a writer's hand, and the signal stopped the writer (the shutdown-sweep flake,
//! 2026-10-19). [`Unsent`] closes that: a count of frames handed to the outbound path and not yet
//! written to a socket or dropped.
//!
//! **The protocol.** The count is raised *before* a frame becomes visible to its consumer and lowered
//! only after the consumer is done with it, so it can never read zero while a frame is in flight:
//! - a producer raises it by one before it enqueues (to a shard, [`GossipTxs::try_send_counted`] /
//!   [`GossipTxs::send_counted`]; to a writer, [`WriterTx::try_send_counted`] /
//!   [`WriterTx::send_counted`]) and lowers it again if the enqueue fails;
//! - the shard worker raises it once per writer it hands the frame to (through [`WriterTx`]) and then
//!   lowers it for its own copy ([`Held`], dropped at the end of the frame's handling);
//! - the writer lowers it for each frame once the frame is flushed to the socket, or dropped — oversize,
//!   reconnect backoff, a failed write, a failed connect, eviction, shutdown, or the task's abort
//!   ([`Held`] and [`CountedRx`]'s drop, which closes the queue and lowers it for every frame left).
//!
//! No path may keep a count it does not hold a frame for: a leak makes every later shutdown wait its
//! whole bound (never forever — the wait is bounded). The senders are not reachable raw, so a send
//! that skips the count does not compile.
//!
//! Lock-free (one `AtomicUsize`), so it adds no row to the lock-order table. Cost: one or two atomic
//! read-modify-writes per frame at each hand.

use crate::framing::ForwardHint;
use crate::sim_seam::ChanVerdict;
use bytes::Bytes;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;

/// One item on a gossip shard queue: the encoded frame, the sender's id hash, and how to forward it.
pub type GossipItem = (Bytes, u64, ForwardHint);

/// The node-wide count of outbound frames not yet on the wire (module docs).
#[derive(Clone, Debug, Default)]
pub struct Unsent(Arc<AtomicUsize>);

impl Unsent {
    /// Frames counted now.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    fn add(&self, n: usize) {
        self.0.fetch_add(n, Ordering::AcqRel);
    }

    fn sub(&self, n: usize) {
        if n == 0 { return; }
        let before = self.0.fetch_sub(n, Ordering::AcqRel);
        debug_assert!(before >= n, "outbound count lowered below zero ({before} - {n}): a frame was released twice");
    }

    /// Hold `n` frames already counted, releasing them when the guard drops.
    pub fn held(&self, n: usize) -> Held {
        Held { unsent: self.clone(), n }
    }
}

/// Frames a consumer holds, already counted; released (the count lowered) on [`release`](Self::release)
/// or drop — so a `break`, an early return or an abort releases them too.
#[must_use]
#[derive(Debug)]
pub struct Held {
    unsent: Unsent,
    n:      usize,
}

impl Held {
    /// One more frame held.
    pub fn add_one(&mut self) {
        self.n += 1;
    }

    /// Release every frame held now.
    pub fn release(&mut self) {
        self.unsent.sub(std::mem::take(&mut self.n));
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.release();
    }
}

/// A receiver of counted frames. Dropping it closes the queue and releases every frame left in it,
/// so a consumer that stops (or is aborted) leaves no count behind; a send after the close fails and
/// its producer lowers the count itself.
#[derive(Debug)]
pub struct CountedRx<T> {
    rx:     mpsc::Receiver<T>,
    unsent: Unsent,
}

impl<T> CountedRx<T> {
    /// Wrap `rx`, whose items were counted into `unsent` by their producers.
    pub fn new(rx: mpsc::Receiver<T>, unsent: Unsent) -> Self {
        Self { rx, unsent }
    }

    /// The next frame, held: the guard releases it.
    pub async fn recv(&mut self) -> Option<(T, Held)> {
        let item = self.rx.recv().await?;
        Some((item, self.unsent.held(1)))
    }

    /// The next frame if one is queued, without holding it: the caller adds it to a [`Held`] it owns.
    pub fn try_recv_into(&mut self, held: &mut Held) -> Option<T> {
        let item = self.rx.try_recv().ok()?;
        held.add_one();
        Some(item)
    }

    /// Frames queued now.
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    /// Whether no frame is queued.
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

impl<T> Drop for CountedRx<T> {
    fn drop(&mut self) {
        self.rx.close();
        let mut left = 0;
        while self.rx.try_recv().is_ok() {
            left += 1;
        }
        self.unsent.sub(left);
    }
}

/// The gossip shard queues, with the count every send through them raises.
///
/// The senders are not exposed: a send goes through [`try_send_counted`](Self::try_send_counted) or
/// [`send_counted`](Self::send_counted), so none skips the count.
#[derive(Clone, Debug)]
pub struct GossipTxs {
    txs:    Arc<[mpsc::Sender<GossipItem>]>,
    unsent: Unsent,
}

impl GossipTxs {
    /// The shard queues, counted into `unsent`.
    pub fn new(txs: Vec<mpsc::Sender<GossipItem>>, unsent: Unsent) -> Self {
        Self { txs: txs.into(), unsent }
    }

    /// The count these queues raise.
    pub fn unsent(&self) -> &Unsent {
        &self.unsent
    }

    /// Number of shards.
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Whether there are no shards.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// `(capacity left, max capacity)` per shard, for fill and depth readings.
    pub fn capacities(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.txs.iter().map(|tx| (tx.capacity(), tx.max_capacity()))
    }

    /// Enqueue `item` on `shard` without waiting, through the channel-readiness seam, counted.
    pub fn try_send_counted(&self, shard: usize, stream: &str, item: GossipItem) -> ChanVerdict {
        self.unsent.add(1);
        let verdict = crate::sim_seam::chan_try_send(stream, &self.txs[shard], item);
        if verdict != ChanVerdict::Sent {
            self.unsent.sub(1);
        }
        verdict
    }

    /// Enqueue `item` on `shard`, waiting for room, counted. `false` when the shard has stopped.
    pub async fn send_counted(&self, shard: usize, item: GossipItem) -> bool {
        self.unsent.add(1);
        let sent = self.txs[shard].send(item).await.is_ok();
        if !sent {
            self.unsent.sub(1);
        }
        sent
    }
}

/// A peer writer's queue, with the count every send through it raises. The sender is not exposed.
#[derive(Clone, Debug)]
pub struct WriterTx {
    tx:     mpsc::Sender<Bytes>,
    unsent: Unsent,
}

impl WriterTx {
    /// A writer queue counted into `unsent`.
    pub fn new(tx: mpsc::Sender<Bytes>, unsent: Unsent) -> Self {
        Self { tx, unsent }
    }

    /// Enqueue `frame` without waiting, through the channel-readiness seam, counted.
    pub fn try_send_counted(&self, stream: &str, frame: Bytes) -> ChanVerdict {
        self.unsent.add(1);
        let verdict = crate::sim_seam::chan_try_send(stream, &self.tx, frame);
        if verdict != ChanVerdict::Sent {
            self.unsent.sub(1);
        }
        verdict
    }

    /// Enqueue `frame`, waiting for room, counted. `false` when the writer has stopped.
    pub async fn send_counted(&self, frame: Bytes) -> bool {
        self.unsent.add(1);
        let sent = self.tx.send(frame).await.is_ok();
        if !sent {
            self.unsent.sub(1);
        }
        sent
    }

    /// Room left in the queue.
    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }

    /// The queue's bound.
    pub fn max_capacity(&self) -> usize {
        self.tx.max_capacity()
    }

    /// Whether the writer has stopped.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every way a frame leaves the path lowers the count exactly once: delivered, refused at the
    /// enqueue, or left in a queue whose consumer stopped.
    #[tokio::test]
    async fn every_exit_lowers_the_count_once() {
        let unsent = Unsent::default();
        let (tx, rx) = mpsc::channel(2);
        let wtx = WriterTx::new(tx, unsent.clone());
        let mut rx = CountedRx::new(rx, unsent.clone());
        assert_eq!(wtx.try_send_counted("t", Bytes::from_static(b"a")), ChanVerdict::Sent);
        assert_eq!(wtx.try_send_counted("t", Bytes::from_static(b"b")), ChanVerdict::Sent);
        assert_eq!(wtx.try_send_counted("t", Bytes::from_static(b"c")), ChanVerdict::Full, "a full queue refuses");
        assert_eq!(unsent.get(), 2, "a refused enqueue is not counted");
        let (_a, mut held) = rx.recv().await.unwrap();
        assert_eq!(unsent.get(), 2, "a dequeued frame stays counted while held");
        held.release();
        assert_eq!(unsent.get(), 1);
        drop(rx);
        assert_eq!(unsent.get(), 0, "a frame left in a stopped writer's queue is released");
        assert!(!wtx.send_counted(Bytes::from_static(b"d")).await, "a send after the stop fails");
        assert_eq!(unsent.get(), 0, "and is not counted");
    }

    /// A held frame is released when its guard drops — a `break` or an abort releases it.
    #[test]
    fn a_dropped_guard_releases() {
        let unsent = Unsent::default();
        unsent.add(3);
        {
            let mut held = unsent.held(1);
            held.add_one();
        }
        assert_eq!(unsent.get(), 1);
    }
}
