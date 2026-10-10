use crate::framing::{write_frame, WireMessage};
use crate::node_id::NodeId;
use crate::store::store_hash_acc;
use crate::stream::GossipStream;
use crate::tls::NodeTls;
use bytes::Bytes;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncWriteExt, BufWriter},
    net::TcpStream,
    sync::{mpsc, watch},
    time as ttime,
};
use tracing::{debug, warn};

/// The writer channel a StateRequest goes out on; a full one skips a state sync.
const STATE_REQ_CHAN: &str = "writer/state-request";

/// The time bounds one peer writer runs under (row B, post-360 hardening): how long it may sit idle,
/// how long a connect plus TLS handshake may take, and how long one frame's write or a flush may
/// take. Built from [`GossipConfig`](crate::config::GossipConfig) by [`WriterTiming::from_config`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriterTiming {
    /// Idle eviction (`writer_idle_timeout_secs`); zero = never.
    pub idle:    Duration,
    /// TCP connect + TLS handshake (`handshake_timeout_ms`).
    pub connect: Duration,
    /// The progress bound on each batch of writes and its flush (`peer_stall_timeout_ms`,
    /// `peer_min_rate_bytes_per_sec`) — progress, not whole frames (#602's review, finding 1).
    pub stall:   crate::stall::StallBound,
}

impl WriterTiming {
    /// The bounds `cfg` states.
    pub fn from_config(cfg: &crate::config::GossipConfig) -> Self {
        Self {
            idle:    Duration::from_secs(cfg.writer_idle_timeout_secs),
            connect: Duration::from_millis(cfg.handshake_timeout_ms),
            stall:   crate::stall::StallBound::from_config(cfg),
        }
    }

    /// The defaults' connect and write bounds with the given idle timeout.
    pub fn with_idle(idle: Duration) -> Self {
        Self { idle, ..Self::from_config(&crate::config::GossipConfig::default()) }
    }
}

/// Returns a jittered backoff in `[backoff/2, backoff*3/2]`.
fn jittered(backoff: Duration) -> Duration {
    let half = backoff.as_millis() as u64 / 2;
    backoff / 2 + Duration::from_millis(fastrand::u64(0..=half * 2))
}

/// Long-lived task that owns the TCP connection to one peer.
///
/// Receives pre-serialized frames over `rx` and writes them in order.
/// After the first frame is written, the task drains any additional queued frames
/// into the `BufWriter` before flushing — coalescing multiple small gossip messages
/// into a single (or fewer) kernel write calls. Reconnects transparently after write
/// failures; backs off for `backoff` after each connect failure so a dead peer
/// doesn't cause a connect storm. Exits on global shutdown, per-peer eviction
/// signal, or when all senders drop.
#[allow(clippy::too_many_arguments)]
pub async fn run_peer_writer(
    peer: NodeId,
    mut rx: mpsc::Receiver<Bytes>,
    backoff: Duration,
    timing: WriterTiming,
    mut shutdown_rx: watch::Receiver<bool>,
    mut peer_shutdown_rx: watch::Receiver<bool>,
    dropped_frames: Arc<AtomicU64>,
    peer_dropped: Arc<AtomicU64>,
    tls: Option<Arc<NodeTls>>,
) {
    let mut conn: Option<BufWriter<crate::stall::StallGuard<GossipStream>>> = None;
    // Stores (fail_time, actual_backoff) where actual_backoff is jittered so
    // simultaneous reconnects after a partition don't all fire at the same instant.
    // (when it failed, how long to wait) — monotonic nanoseconds from the clock seam, so a replay
    // reproduces a reconnect backoff instead of spending it.
    let mut last_fail: Option<(u64, Duration)> = None;
    // Idle eviction: track when we last sent a frame. None = no timeout configured.
    let idle_timeout = timing.idle;
    let mut idle_deadline: Option<ttime::Instant> = if idle_timeout.is_zero() {
        None
    } else {
        Some(ttime::Instant::now() + idle_timeout)
    };

    loop {
        // biased: data path checked first so a burst of frames drains before shutdown.
        // The idle arm uses pending() when no timeout is configured so it never fires.
        let data: Bytes = tokio::select! { biased;
            msg = rx.recv() => match msg {
                Some(d) => d,
                None => break, // all senders dropped
            },
            _ = shutdown_rx.wait_for(|v| *v) => break,
            _ = peer_shutdown_rx.wait_for(|v| *v) => break,
            _ = async {
                match idle_deadline {
                    Some(d) => ttime::sleep_until(d).await,
                    None    => std::future::pending().await,
                }
            } => break,
        };
        if let Some(ref mut d) = idle_deadline {
            *d = ttime::Instant::now() + idle_timeout;
        }

        if let Some((fail_time, fail_backoff)) = last_fail
            && crate::sim_seam::mono_since(fail_time) < fail_backoff {
                dropped_frames.fetch_add(1, Ordering::Relaxed);
                peer_dropped.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_frames_dropped_total").increment(1);
                debug!("Dropping frame to {} during reconnect backoff", peer);
                continue;
            }

        // Lazily establish (or re-establish) the connection. The connect and the TLS handshake are
        // bounded together by `timing.connect`, and shutdown or eviction interrupts them (row B):
        // a peer that accepts TCP and never answers the handshake used to park this task.
        if conn.is_none() {
            let attempt = async {
                let s = TcpStream::connect(peer.to_socket_addr()).await
                    .map_err(|e| format!("Connect to {peer} failed: {e}"))?;
                let _ = s.set_nodelay(true);
                #[cfg(unix)]
                {
                    use socket2::{SockRef, TcpKeepalive};
                    let ka = TcpKeepalive::new()
                        .with_time(Duration::from_secs(30))
                        .with_interval(Duration::from_secs(10));
                    let _ = SockRef::from(&s).set_tcp_keepalive(&ka);
                }
                // Optional TLS upgrade before buffering.
                tls_connect(s, &peer, &tls).await.map_err(|e| format!("TLS handshake to {peer} failed: {e}"))
            };
            let outcome = tokio::select! { biased;
                _ = shutdown_rx.wait_for(|v| *v) => break,
                _ = peer_shutdown_rx.wait_for(|v| *v) => break,
                r = ttime::timeout(timing.connect, attempt) => r,
            };
            match outcome {
                Ok(Ok(gs)) => {
                    // Identity-auth Phase 1b: harvest the peer's CA-validated Ed25519 key
                    // from its cert (outbound side, so it correlates to the NodeId we
                    // dialed) and record it as an authenticated anchor. Non-fatal — a
                    // missing key never affects connectivity.
                    #[cfg(feature = "tls")]
                    if let Some(ref t) = tls
                        && let Some(anchor_key) = gs.peer_ed25519_key()
                    {
                        t.record_anchor(&peer, anchor_key);
                    }
                    // 16 KB buffer coalesces a full burst of small gossip frames into
                    // one or two kernel write calls; explicit flush sends after drain.
                    conn = Some(BufWriter::with_capacity(16_384, crate::stall::StallGuard::new(gs)));
                    last_fail = None;
                }
                Ok(Err(e)) => {
                    last_fail = Some((crate::sim_seam::mono_now_ns(), jittered(backoff)));
                    warn!("{e}");
                    continue;
                }
                Err(_) => {
                    last_fail = Some((crate::sim_seam::mono_now_ns(), jittered(backoff)));
                    warn!("Connect to {} timed out after {:?} (handshake_timeout_ms)", peer, timing.connect);
                    continue;
                }
            }
        }

        // Write this frame and any others already queued, then flush once.
        //
        // `FrameTooLarge` is a *frame* problem, not a *connection* problem: `write_frame`
        // checks the size before touching the stream, so the connection is still clean —
        // drop that frame with a warn and keep going. Treating it as a write failure
        // (pre-2026-07-02 behaviour) tore down a healthy connection and dropped every
        // queued frame behind one oversized payload.
        let frame_fits = |peer: &NodeId, res: Result<(), crate::error::GossipError>| -> Result<bool, ()> {
            match res {
                Ok(())                                                => Ok(true),
                Err(crate::error::GossipError::FrameTooLarge { size, limit }) => {
                    dropped_frames.fetch_add(1, Ordering::Relaxed);
                    peer_dropped.fetch_add(1, Ordering::Relaxed);
                    warn!("Dropping oversized frame to {} ({} B > {} B limit); connection kept", peer, size, limit);
                    Ok(false)
                }
                Err(_) => Err(()),
            }
        };
        // The writes and the flush must make progress (row B; #602's review, finding 1): a peer that
        // accepts and never reads fills the socket buffers and used to park this task in a write
        // forever — with its channel, and every anti-entropy reply queued on it, behind it. The bound
        // is on progress (`peer_stall_timeout_ms` without a byte, or below the rate floor), not on a
        // whole frame, so a large frame to a slow but healthy peer still crosses. Shutdown and eviction
        // interrupt the writes too; before, they were polled only between frames.
        let stall = timing.stall;
        let write = async {
            let c = conn.as_mut().expect("infallible: conn is Some while loop body runs; only set None after break");
            c.get_mut().arm(Some(stall));
            let mut wrote_any = false;
            match frame_fits(&peer, write_frame(c, &data).await) {
                Ok(sent)  => wrote_any |= sent,
                Err(())   => return false,
            }
            while let Ok(more) = rx.try_recv() {
                match frame_fits(&peer, write_frame(c, &more).await) {
                    Ok(sent) => wrote_any |= sent,
                    Err(())  => return false,
                }
            }
            let ok = !wrote_any || c.flush().await.is_ok();
            c.get_mut().arm(None);
            ok
        };
        let write_ok = tokio::select! { biased;
            _ = shutdown_rx.wait_for(|v| *v) => break,
            _ = peer_shutdown_rx.wait_for(|v| *v) => break,
            ok = write => ok,
        };

        if !write_ok {
            conn = None;
            // +1 for the frame that caused the write failure (already dequeued, never sent).
            let dropped = rx.len() + 1;
            last_fail = Some((crate::sim_seam::mono_now_ns(), jittered(backoff)));
            warn!("Write to {} failed; {} frame(s) will be dropped during backoff", peer, dropped);
        }
    }
}

/// Peer writer map entry. Keeps writer lifecycle co-located with peer state, bounding the
/// global task_handles vec to the small fixed set of system tasks (listener, shards, health).
///
/// `abort_handle` is `Clone` (unlike `JoinHandle`), satisfying papaya's `V: Clone` bound
/// for `compute()`. The task runs as a detached tokio task; it exits via `peer_shutdown`
/// or the global shutdown signal.
///
/// `abort_handle = None` is the *pending* sentinel: a caller has claimed the spawn slot
/// and installed the channel, but the writer task has not been spawned yet. Concurrent
/// callers that see `None` return the pre-installed `tx` directly — they share the same
/// channel and their frames will be drained once the task starts.
#[derive(Clone)]
pub struct WriterEntry {
    pub tx:            mpsc::Sender<Bytes>,
    pub peer_shutdown: Arc<watch::Sender<bool>>,
    /// `None` = spawn in progress (pending sentinel); `Some` = task running or finished.
    pub abort_handle:  Option<tokio::task::AbortHandle>,
    /// Cumulative frames dropped to this peer during reconnect backoff.
    /// Subset of the global `dropped_frames` counter; useful for identifying slow peers.
    pub dropped:       Arc<AtomicU64>,
}

impl WriterEntry {
    /// Returns `true` if the writer task is alive or its spawn is still pending.
    pub fn is_live(&self) -> bool {
        self.abort_handle.as_ref().is_none_or(|h| !h.is_finished())
    }
}

/// Returns the frame sender for `peer`'s writer task, spawning a new task on first use.
///
/// Uses a *claim-then-spawn* protocol to ensure exactly one task is spawned per peer:
///
/// 1. **Fast path** — if a live entry (or a pending spawn) exists, return its `tx`.
/// 2. **Claim** — atomically insert a pending sentinel (`abort_handle = None`) with a
///    pre-created channel. Concurrent callers that lose the CAS return the winner's `tx`.
/// 3. **Spawn** — the claim winner spawns the writer task outside `compute()` (so papaya
///    retry loops don't create duplicate tasks), then updates the entry with the real handle.
#[allow(clippy::too_many_arguments)]
pub fn get_or_spawn_writer(
    peer: &NodeId,
    writers: &papaya::HashMap<NodeId, WriterEntry>,
    chan_depth: usize,
    backoff: Duration,
    timing: WriterTiming,
    shutdown_tx: &Arc<watch::Sender<bool>>,
    dropped_frames: &Arc<AtomicU64>,
    tls: Option<Arc<NodeTls>>,
) -> Option<mpsc::Sender<Bytes>> {
    // Guard: refuse to spawn during shutdown.
    if *shutdown_tx.borrow() {
        return None;
    }

    let guard = writers.pin();

    // Fast path: live writer or pending spawn already exists.
    if let Some(entry) = guard.get(peer)
        && entry.is_live() {
            return Some(entry.tx.clone());
        }

    // Claim the spawn slot by installing a pending sentinel atomically.
    // Creating the channel here is O(1) (no OS resources); the task only runs if we win.
    let (tx, rx) = mpsc::channel(chan_depth);
    let (peer_shutdown_tx, peer_shutdown_rx) = watch::channel(false);
    let peer_shutdown = Arc::new(peer_shutdown_tx);
    let dropped = Arc::new(AtomicU64::new(0));
    let pending = WriterEntry {
        tx: tx.clone(),
        peer_shutdown: Arc::clone(&peer_shutdown),
        abort_handle: None,
        dropped: Arc::clone(&dropped),
    };

    let claim = guard.compute(peer.clone(), |existing| match existing {
        Some((_, e)) if e.is_live() => papaya::Operation::Abort(e.tx.clone()),
        _                           => papaya::Operation::Insert(pending.clone()),
    });

    if let papaya::Compute::Aborted(winner_tx) = claim {
        // Another caller already holds the slot (live writer or pending spawn). Use theirs.
        return Some(winner_tx);
    }

    // We won the claim. Spawn the task (outside compute so retries don't duplicate it).
    let join_handle = tokio::spawn(run_peer_writer(
        peer.clone(),
        rx,
        backoff,
        timing,
        shutdown_tx.subscribe(),
        peer_shutdown_rx,
        Arc::clone(dropped_frames),
        dropped,
        tls,
    ));
    let abort_handle = join_handle.abort_handle();
    drop(join_handle); // detach — task exits via peer_shutdown or global shutdown signal

    // Upgrade *my own* pending sentinel to a live entry — identified by the unique `peer_shutdown`
    // Arc this call created. Matching on `abort_handle.is_none()` alone (the old code) let a
    // concurrent evict + re-claim (a DIFFERENT caller's fresh pending, also handle == None) be
    // clobbered: the map then advertised a channel whose task was already told to die, while the
    // successor's task ran orphaned — a leaked task + a second connection to the peer (audit 2026-07-15).
    let upgraded = guard.compute(peer.clone(), |existing| match existing {
        Some((_, e)) if e.abort_handle.is_none() && Arc::ptr_eq(&e.peer_shutdown, &peer_shutdown) =>
            papaya::Operation::Insert(WriterEntry {
                tx: tx.clone(),
                peer_shutdown: Arc::clone(&peer_shutdown),
                abort_handle: Some(abort_handle.clone()),
                dropped: Arc::clone(&e.dropped),
            }),
        _ => papaya::Operation::Abort(()),
    });
    publish_connected_gauge(writers);

    if matches!(upgraded, papaya::Compute::Inserted(..) | papaya::Compute::Updated { .. }) {
        Some(tx)
    } else {
        // My pending was evicted/replaced before I could upgrade — I no longer own the slot. Tear
        // down the task I spawned so it doesn't run orphaned, and hand back the live winner's sender.
        let _ = peer_shutdown.send(true);
        abort_handle.abort();
        guard.get(peer).map(|e| e.tx.clone())
    }
}

/// Removes `peer`'s writer from the map and signals its task to exit.
pub fn evict_peer_writer(writers: &papaya::HashMap<NodeId, WriterEntry>, peer: &NodeId) {
    // Remove-and-capture atomically, then signal *exactly the entry we removed*. The old
    // `get`-then-blind-`remove` had a TOCTOU: between reading the entry (to send its shutdown) and
    // the unconditional `remove`, a concurrent `get_or_spawn_writer` could re-claim the slot with a
    // fresh LIVE writer — which `remove` then deleted from the map *without signaling its task*,
    // orphaning it (leaked task + a second connection to the peer, unreachable by any later evict or
    // shutdown drain). Mirror of the get_or_spawn_writer upgrade-clobber guard (audit 2026-07-15
    // pass 2). Killing whatever writer is currently installed is the intended semantics — evict means
    // "stop writing to this peer" — the fix only ensures the removed task is always told to die.
    let guard = writers.pin();
    if let papaya::Compute::Removed(_, entry) = guard.compute(peer.clone(), |existing| match existing {
        Some(_) => papaya::Operation::Remove,
        None    => papaya::Operation::Abort(()),
    }) {
        let _ = entry.peer_shutdown.send(true);
    }
}

/// Reaps writer entries for peers whose task has already finished (idle-exit). Re-checks liveness
/// **inside** the map's `compute` guard, so a peer re-claimed with a fresh LIVE writer between the
/// caller's collection of `finished` and this removal is never blind-removed — a blind `remove`
/// would delete the live entry from the map without signaling its task, orphaning it (leaked task +
/// a second connection to the peer). Signals nothing: a finished task needs no shutdown. This is the
/// passive-reap counterpart to [`evict_peer_writer`]'s active eviction (audit 2026-07-15 pass 2).
pub fn reap_finished_writers(
    writers:  &papaya::HashMap<NodeId, WriterEntry>,
    finished: impl IntoIterator<Item = NodeId>,
) {
    let guard = writers.pin();
    for peer in finished {
        guard.compute(peer, |existing| match existing {
            Some((_, e)) if !e.is_live() => papaya::Operation::Remove,
            _                            => papaya::Operation::Abort(()),
        });
    }
    publish_connected_gauge(writers);
}

/// Zero-gaps Z9 (D9): `gossip_peers_connected` is the number of **live writers** in `writers` —
/// sockets, not the view (`mycelium_emergent_peers_known` is the view) — published where the map
/// changes (a claim won, a reap) so it mirrors `/stats` `cached_connections` without a poll.
pub fn publish_connected_gauge(writers: &papaya::HashMap<NodeId, WriterEntry>) {
    #[cfg(feature = "metrics")]
    {
        let live = writers.pin().iter().filter(|(_, e)| e.is_live()).count();
        metrics::gauge!("gossip_peers_connected").set(live as f64);
    }
    #[cfg(not(feature = "metrics"))]
    let _ = writers;
}

/// Serialises and enqueues a `StateRequest` into `peer`'s writer channel,
/// spawning the writer task if needed.
///
/// `bucket_hashes` is the sender's per-bucket Merkle digest of its live store
/// (`store::store_bucket_hashes`), used by the receiver to send only divergent-bucket
/// entries (v12 Merkle anti-entropy). Pass `vec![]` for a full-dump request (the
/// "no digest" sentinel — e.g. when the local store is empty).
#[allow(clippy::too_many_arguments)]
pub fn request_state(
    peer: &NodeId,
    peer_writers: &papaya::HashMap<NodeId, WriterEntry>,
    writer_depth: usize,
    backoff: Duration,
    timing: WriterTiming,
    shutdown_tx: &Arc<watch::Sender<bool>>,
    sender: &NodeId,
    hash_acc: &AtomicU64,
    dropped_frames: &Arc<AtomicU64>,
    bucket_hashes: Vec<u64>,
    tls: Option<Arc<NodeTls>>,
) {
    let hash = store_hash_acc(hash_acc);
    let data: Bytes = crate::codec::wire_to_bytes(
        &WireMessage::StateRequest { sender: sender.clone(), store_hash: hash, bucket_hashes },
    );
    let Some(tx) = get_or_spawn_writer(peer, peer_writers, writer_depth, backoff, timing, shutdown_tx, dropped_frames, tls) else { return; };
    if crate::sim_seam::chan_try_send(STATE_REQ_CHAN, &tx, data) != crate::sim_seam::ChanVerdict::Sent {
        warn!("StateRequest writer for {}: channel full or closed; state sync skipped", peer);
    }
}

/// Upgrades a plain `TcpStream` to a `GossipStream`, performing a TLS client
/// handshake when `tls` is `Some`. Returns the plain stream unchanged otherwise.
async fn tls_connect(
    stream: TcpStream,
    #[allow(unused_variables)] peer: &NodeId,
    tls: &Option<Arc<NodeTls>>,
) -> Result<GossipStream, std::io::Error> {
    #[cfg(feature = "tls")]
    if let Some(node_tls) = tls {
        use rustls::pki_types::ServerName;
        let ip = peer.to_socket_addr().ip();
        let server_name = ServerName::try_from(ip.to_string().as_str())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?
            .to_owned();
        let connector = tokio_rustls::TlsConnector::from(node_tls.client_config());
        let tls_stream = connector.connect(server_name, stream).await?;
        return Ok(GossipStream::TlsClient(tls_stream));
    }
    let _ = tls; // suppress unused warning when feature is disabled
    Ok(GossipStream::Plain(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use tokio::sync::{mpsc, watch};

    fn id(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    /// A LIVE entry (pending sentinel: `abort_handle == None` ⇒ `is_live()`), returning a receiver on
    /// its `peer_shutdown` so a test can assert whether the entry's task was told to exit.
    fn live_entry() -> (WriterEntry, watch::Receiver<bool>) {
        let (tx, _rx) = mpsc::channel(1);
        let (ps, ps_rx) = watch::channel(false);
        (WriterEntry { tx, peer_shutdown: Arc::new(ps), abort_handle: None, dropped: Arc::new(AtomicU64::new(0)) }, ps_rx)
    }

    /// A NOT-live entry: the abort handle of a task that has already run to completion.
    async fn finished_entry() -> WriterEntry {
        let (tx, _rx) = mpsc::channel(1);
        let (ps, _ps_rx) = watch::channel(false);
        let jh = tokio::spawn(async {});
        let ah = jh.abort_handle();
        jh.await.unwrap();
        assert!(ah.is_finished(), "precondition: task finished ⇒ entry not live");
        WriterEntry { tx, peer_shutdown: Arc::new(ps), abort_handle: Some(ah), dropped: Arc::new(AtomicU64::new(0)) }
    }

    /// The reconnect backoff drops frames while it is running, and stops dropping once it expires.
    ///
    /// **This behaviour had no test in `mycelium-core` at all.** Breaking `sim_seam::mono_since` to
    /// return zero left all 180 core tests green and failed exactly one test in the outer crate, by
    /// a route with nothing to do with reconnecting. A conversion whose behaviour nothing checks is
    /// a conversion nobody can review — so the clock seam's first call site gets the test its
    /// absence made obvious.
    #[tokio::test]
    async fn the_reconnect_backoff_drops_frames_while_it_runs_and_stops_once_it_expires() {
        // A port nothing listens on, so `connect` is refused at once and the test is decided by the
        // backoff rather than by a network timeout.
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };

        let (tx, rx) = mpsc::channel::<Bytes>(8);
        let (_sd, sd_rx)   = watch::channel(false);
        let (_psd, psd_rx) = watch::channel(false);
        let dropped      = Arc::new(AtomicU64::new(0));
        let peer_dropped = Arc::new(AtomicU64::new(0));

        // `jittered` returns `[backoff/2, backoff*3/2]`, so 200 ms means a wait somewhere in
        // 100..300 ms. Every assertion below sits well outside that band, so none of them depends
        // on which value the draw produced.
        let handle = tokio::spawn(run_peer_writer(
            id(port), rx, Duration::from_millis(200), WriterTiming::with_idle(Duration::ZERO),
            sd_rx, psd_rx, Arc::clone(&dropped), Arc::clone(&peer_dropped), None,
        ));

        /// Wait for a condition rather than for a duration — the two fixed waits below are the
        /// backoff itself, which is the thing under test.
        async fn until(mut f: impl FnMut() -> bool) -> bool {
            for _ in 0..200 {
                if f() { return true; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            false
        }

        // Frame 1 provokes the failed connect that arms the backoff.
        tx.send(Bytes::from_static(b"one")).await.unwrap();
        assert!(until(|| tx.capacity() == 8).await, "the writer never took the first frame");

        // Frame 2 arrives inside the backoff: dropped, with no connect attempted.
        tx.send(Bytes::from_static(b"two")).await.unwrap();
        assert!(
            until(|| peer_dropped.load(Ordering::Relaxed) == 1).await,
            "a frame sent inside the reconnect backoff must be dropped"
        );

        // Past the longest the jitter can make the backoff.
        tokio::time::sleep(Duration::from_millis(400)).await;
        tx.send(Bytes::from_static(b"three")).await.unwrap();
        assert!(until(|| tx.capacity() == 8).await, "the writer never took the third frame");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            peer_dropped.load(Ordering::Relaxed), 1,
            "once the backoff has elapsed the frame must provoke a fresh connect, not a drop"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn regression_evict_signals_exactly_the_entry_it_removes() {
        // evict must remove-and-signal atomically: the removed entry's task is always told to exit.
        // The old get-then-blind-`remove` could signal one entry and remove another (a re-claimed
        // successor), leaving the removed task orphaned (audit 2026-07-15 pass 2).
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        let (e, mut ps_rx) = live_entry();
        writers.pin().insert(id(1), e);
        evict_peer_writer(&writers, &id(1));
        assert!(writers.pin().get(&id(1)).is_none(), "entry must be removed");
        assert!(*ps_rx.borrow_and_update(), "the removed entry's task must be signaled to exit");
    }

    #[tokio::test]
    async fn regression_reap_keeps_reclaimed_live_writer() {
        // The GC collects finished peers, then reaps under a fresh guard. If the slot was re-claimed
        // with a LIVE writer in between, the reap must NOT remove it — a blind remove orphaned the
        // live task (audit 2026-07-15 pass 2).
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        // 1. A finished entry, "collected" into `finished`.
        writers.pin().insert(id(1), finished_entry().await);
        let finished = vec![id(1)];
        // 2. Peer re-claimed with a fresh LIVE writer before the reap runs.
        let (live, _ps_rx) = live_entry();
        writers.pin().insert(id(1), live);
        // 3. The real reap path.
        reap_finished_writers(&writers, finished);
        assert!(writers.pin().get(&id(1)).is_some(), "reap must not remove the re-claimed LIVE writer");
        assert!(writers.pin().get(&id(1)).unwrap().is_live());
    }

    #[tokio::test]
    async fn reap_removes_a_still_finished_writer() {
        // The common case still works: a peer that is still finished at reap time is removed.
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        writers.pin().insert(id(1), finished_entry().await);
        reap_finished_writers(&writers, vec![id(1)]);
        assert!(writers.pin().get(&id(1)).is_none(), "a still-finished writer must be reaped");
    }

    /// A listener that accepts every connection and never reads from it, so a writer's socket
    /// buffers fill and its next write blocks. Returns the peer id and the task holding the sockets.
    async fn never_reading_peer() -> (NodeId, tokio::task::JoinHandle<()>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        (id(port), h)
    }

    /// Fill `tx` with ~32 MB, far past any socket buffer, so the writer is parked in a write.
    async fn flood(tx: &mpsc::Sender<Bytes>) {
        let frame = Bytes::from(vec![7u8; 512 * 1024]);
        for _ in 0..64 {
            if tx.try_send(frame.clone()).is_err() { break; }
        }
        // Let the writer connect and block in its write.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    async fn finished_within(writers: &papaya::HashMap<NodeId, WriterEntry>, peer: &NodeId,
                             ah: tokio::task::AbortHandle, within: Duration) -> bool {
        let _ = (writers, peer);
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if ah.is_finished() { return true; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ah.is_finished()
    }

    /// Finding (23): shutdown and eviction were polled only between frames, so a writer blocked in
    /// a write to a peer that accepts and never reads never saw either. The write bound here is a
    /// minute, so what ends the writer is the eviction, not the timeout.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_peer_that_never_reads_does_not_block_eviction() {
        let (peer, _hold) = never_reading_peer().await;
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        let (sd, _sd_rx) = watch::channel(false);
        let sd = Arc::new(sd);
        let timing = WriterTiming { idle: Duration::ZERO, connect: Duration::from_secs(5), stall: crate::stall::StallBound { stall: Duration::from_secs(60), min_rate: 0 } };
        let tx = get_or_spawn_writer(&peer, &writers, 4096, Duration::from_millis(100), timing,
                                     &sd, &Arc::new(AtomicU64::new(0)), None).unwrap();
        flood(&tx).await;
        let ah = writers.pin().get(&peer).unwrap().abort_handle.clone().unwrap();
        assert!(!ah.is_finished(), "precondition: the writer is running");
        evict_peer_writer(&writers, &peer);
        assert!(finished_within(&writers, &peer, ah, Duration::from_secs(3)).await,
                "an evicted writer blocked in a write to a peer that never reads must still exit");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_peer_that_never_reads_does_not_block_shutdown() {
        let (peer, _hold) = never_reading_peer().await;
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        let (sd, _sd_rx) = watch::channel(false);
        let sd = Arc::new(sd);
        let timing = WriterTiming { idle: Duration::ZERO, connect: Duration::from_secs(5), stall: crate::stall::StallBound { stall: Duration::from_secs(60), min_rate: 0 } };
        let tx = get_or_spawn_writer(&peer, &writers, 4096, Duration::from_millis(100), timing,
                                     &sd, &Arc::new(AtomicU64::new(0)), None).unwrap();
        flood(&tx).await;
        let ah = writers.pin().get(&peer).unwrap().abort_handle.clone().unwrap();
        let _ = sd.send(true);
        assert!(finished_within(&writers, &peer, ah, Duration::from_secs(3)).await,
                "a writer blocked in a write to a peer that never reads must still see shutdown");
    }

    /// The write bound: a stalled write fails the connection, and what was queued behind it is
    /// dropped (and counted) instead of waiting on the peer forever — which is what releases an
    /// anti-entropy reply's slot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stalled_write_times_out_and_the_queue_drains() {
        let (peer, _hold) = never_reading_peer().await;
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        let (sd, _sd_rx) = watch::channel(false);
        let sd = Arc::new(sd);
        let timing = WriterTiming { idle: Duration::ZERO, connect: Duration::from_secs(5), stall: crate::stall::StallBound { stall: Duration::from_millis(300), min_rate: 0 } };
        let tx = get_or_spawn_writer(&peer, &writers, 4096, Duration::from_secs(30), timing,
                                     &sd, &Arc::new(AtomicU64::new(0)), None).unwrap();
        flood(&tx).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tx.capacity() < tx.max_capacity() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(tx.capacity(), tx.max_capacity(), "the queue behind a stalled write must drain");
        let dropped = writers.pin().get(&peer).unwrap().dropped.load(Ordering::Relaxed);
        assert!(dropped > 0, "the frames behind the stalled write are dropped and counted");
        let _ = sd.send(true);
    }

    /// The adversarial review of #602, finding 1: the old `peer_write_timeout_ms` bounded each *whole* frame,
    /// so a 10 MB frame needed ≥ 2.8 Mbit/s and a large value to a slow but healthy peer was dropped
    /// on every attempt. A peer that keeps reading, however slowly, must receive the frame: ~5 s for
    /// the frame here, against a 2 s no-progress bound (the receiver's 16 KiB window makes TCP's
    /// persist timer pause the sender for a few hundred ms at a time, so the bound is not tighter).
    /// Seen failing first with the whole-frame bound at 300 ms: received 939 745 of 4 194 309 bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_reader_that_keeps_reading_receives_a_large_frame() {
        use tokio::io::AsyncReadExt;
        // A small receive buffer so the kernel cannot absorb the frame on the reader's behalf.
        let sock = tokio::net::TcpSocket::new_v4().unwrap();
        sock.set_recv_buffer_size(16 * 1024).unwrap();
        sock.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let l = sock.listen(16).unwrap();
        let peer = id(l.local_addr().unwrap().port());
        let received = Arc::new(AtomicU64::new(0));
        let r = Arc::clone(&received);
        let _reader = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => { r.fetch_add(n as u64, Ordering::Relaxed); }
                }
                tokio::time::sleep(Duration::from_millis(20)).await; // ≈ 800 KB/s
            }
        });
        let writers: papaya::HashMap<NodeId, WriterEntry> = papaya::HashMap::new();
        let (sd, _sd_rx) = watch::channel(false);
        let sd = Arc::new(sd);
        let timing = WriterTiming { idle: Duration::ZERO, connect: Duration::from_secs(5), stall: crate::stall::StallBound { stall: Duration::from_secs(2), min_rate: 0 } };
        let tx = get_or_spawn_writer(&peer, &writers, 16, Duration::from_secs(30), timing,
                                     &sd, &Arc::new(AtomicU64::new(0)), None).unwrap();
        let frame_len = 4 * 1024 * 1024;
        tx.send(Bytes::from(vec![1u8; frame_len])).await.unwrap();
        let want = (frame_len + 5) as u64;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while received.load(Ordering::Relaxed) < want && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(received.load(Ordering::Relaxed), want,
                   "a reader making steady progress must receive the whole frame");
        let _ = sd.send(true);
    }
}
