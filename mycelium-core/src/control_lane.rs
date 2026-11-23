//! **The control lane (B1)**: a second connection per peer, beside the data connection, for the few
//! frame kinds whose delay is a promise ([`CONTROL_KINDS`]). Design: `docs/design/control-lane.md`.
//!
//! **Why a connection and not a priority.** On the data connection a frame waits behind every frame the
//! writer's channel holds, every byte in its buffer and in both kernel buffers, and — at the receiver —
//! behind every frame its connection reader handles first, one at a time (`connection::handle_connection`),
//! some of which wait on the WAL. No priority inside the sender's writer reaches the receiver's reader. A
//! connection of its own has its own writer task, its own socket and its own reader at the receiver, so a
//! control frame waits only behind other control frames (at most [`CONTROL_LANE_DEPTH`] per peer).
//!
//! **What it promises, and to whom.** The lane is an *extra copy*: [`crate::ops::emit_signal_control`]
//! sends the frame by the shared path exactly as `emit_signal` does, and offers the **same encoded bytes**
//! (same nonce) to the lane. Whichever copy arrives second is dropped by the receiver's node-wide seen-set
//! before delivery and before relay. So Individual-scope forwarding stays unconditional (the lane is not a
//! carve-out): a lane that is down, full or refused costs only the speed-up.
//!
//! **Rules** (design §3.2, the review's M1):
//! - a lane opens to a peer only while that peer's **data** writer is connected, or while the peer has no
//!   data writer at all (a partial mesh: the call's own reachability decides, M7) — never while a data
//!   writer exists without a connected socket, where the lane could take the per-source slot its
//!   reconnect needs ([`ControlLane::may_open`]);
//! - an open lane **yields** — closes, counted in `yielded` — when the data writer is not connected: at
//!   once when [`ControlLane::may_open`] finds it so, and otherwise within the lane's yield tick
//!   ([`yield_tick_ms`]: a quarter of the reconnect backoff, between 50 ms and
//!   [`CONTROL_LANE_YIELD_TICK_MS`]), checked by the lane's writer ([`ControlLane::should_yield`]);
//! - a lane whose handshake is refused (the socket closed) after its TCP connect succeeded — what a
//!   per-source cap's accept-and-close looks like — is not tried again for
//!   [`CONTROL_LANE_REFUSED_BACKOFF_MS`]; a lane whose TCP connect fails (the peer down or restarting), or
//!   whose handshake times out (a slow peer), waits only the node's reconnect backoff;
//! - no lane is opened to a removed member (closure plan C5);
//! - **the bound on a data reconnect, stated** (#832's review, F2): a lane opened while the peer had no
//!   data writer (or an exited, unreaped one) holds the per-source slot when a new data writer connects,
//!   so under a cap of 1 that writer's first connect can be refused. The lane then yields within its tick,
//!   a quarter of the reconnect backoff, before the writer's retry (at least half the backoff, jittered),
//!   so **each data writer is refused at most once** and waits one reconnect backoff, dropping its frames
//!   as any writer in backoff does. Once per data writer, not once ever: a writer that idles out and is
//!   reaped lets a lane open again, and its successor can be refused once in turn. At
//!   `reconnect_backoff_secs = 0` the retry is immediate and the bound is the 50 ms tick, not "once";
//! - no lane opens for a **reply** to a peer this node has neither a data writer to nor a `peers` entry
//!   for (#832's review, F5): a reply's target is the request's `sender`, which the signal layer does not
//!   authenticate, so a forged sender must not make this node dial an address of the forger's choosing;
//! - **a relay never puts a frame it received on a lane** (D7): nothing on the receive path calls
//!   [`ControlLane::send`]; relays forward through the shard only.
//!
//! **No new lock.** The writer map and the refusal map are `papaya` maps; the counters are atomics.

use crate::node_id::NodeId;
use crate::writer::WriterEntry;
use bytes::Bytes;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// The request kinds the lane carries — **closed**: a PR that adds a kind names it and its rate bound.
///
/// - `sys.call-cancel.v1` — a gateway's cancel-forward to the provider it dispatched the call to
///   (identity-and-lifecycle I4, bound 1). Its rate is bounded by the calls under cancellable rules,
///   and #826's re-sends are at most one per attempt.
///
/// The reply to a request of one of these kinds (`rpc.result`, toward the request's sender) rides the
/// lane too, and only that reply; see [`ControlFrame`] and [`may_carry`].
pub const CONTROL_KINDS: &[&str] = &["sys.call-cancel.v1"];

/// A kind the lane admits **only in test builds**, so a test can drive the lane without a real control
/// kind's handler answering its frames. Never in a shipped build.
#[cfg(any(test, feature = "test-support"))]
pub const TEST_CONTROL_KIND: &str = "test.control-lane.v1";

/// Each lane's channel depth (D5). A full lane never blocks: the copy is counted `full`, and the shared
/// copy has already been offered.
pub const CONTROL_LANE_DEPTH: usize = 64;
/// The progress bound on a lane's writes (D5): a lane writer whose batch moves no byte for this long
/// closes, counted in `outbound_stalls`. The data connection keeps `peer_write_stall_timeout_ms`.
pub const CONTROL_LANE_WRITE_BOUND_MS: u64 = 10_000;
/// How long after a failed lane connect no lane is opened to that peer again (D5, M1).
pub const CONTROL_LANE_REFUSED_BACKOFF_MS: u64 = 300_000;
/// The longest a lane's yield tick runs (M1): an idle lane checks at least this often whether the peer's
/// data writer lost its socket. A busy lane also checks before each batch. See [`yield_tick_ms`].
pub const CONTROL_LANE_YIELD_TICK_MS: u64 = 1_000;
/// The shortest yield tick, for a reconnect backoff of 0.
pub const CONTROL_LANE_YIELD_TICK_MIN_MS: u64 = 50;

/// A lane's yield tick for a node whose reconnect backoff is `backoff`: a quarter of it, between
/// [`CONTROL_LANE_YIELD_TICK_MIN_MS`] and [`CONTROL_LANE_YIELD_TICK_MS`]. A refused data writer retries no
/// sooner than half its backoff (`writer::jittered`), so the lane has yielded by then and the writer is
/// refused at most once (#832's review, F2).
pub fn yield_tick_ms(backoff: std::time::Duration) -> u64 {
    (backoff.as_millis() as u64 / 4).clamp(CONTROL_LANE_YIELD_TICK_MIN_MS, CONTROL_LANE_YIELD_TICK_MS)
}
/// The largest frame the lane carries; a larger copy is refused (counted) and only the shared copy goes.
pub const CONTROL_LANE_MAX_FRAME_BYTES: usize = 64 * 1024;

/// The seam stream a lane's yield tick sleeps on.
pub(crate) const YIELD_STREAM: &str = "writer/control-yield";

/// Whether `kind` is one of [`CONTROL_KINDS`] (or, in test builds, [`TEST_CONTROL_KIND`]).
pub fn is_control_kind(kind: &str) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if kind == TEST_CONTROL_KIND {
        return true;
    }
    CONTROL_KINDS.contains(&kind)
}

/// What a frame offered to the lane is ([`crate::ops::emit_signal_control`]).
#[derive(Debug, Clone, Copy)]
pub enum ControlFrame<'a> {
    /// A request: its own kind must be a control kind.
    Request,
    /// The `rpc.result` reply to `request`, the request this node received: the request's kind must be a
    /// control kind — checked as a request's is — and the reply goes to the request's sender only (#832's
    /// review, F5: a caller naming a control kind could otherwise put any reply on the lane).
    ///
    /// **The request's `sender` is not authenticated by the signal layer.** So a reply never opens a lane
    /// to a peer this node has no data writer to and no `peers` entry for ([`LaneVerdict::UnknownPeer`]),
    /// and **a caller (B1's PR 2) may pass `ReplyTo` only for a request that passed its receive boundary**
    /// — its envelope verified, its `via` its sender. A refusal is answered by the shared path only: a
    /// forged request must not make this node dial an address of the forger's choosing.
    ReplyTo(&'a crate::signal::Signal),
}

/// Whether a frame of `kind` to `target` may ride the lane (the closed set, D2): a request whose kind is a
/// control kind, or the `rpc.result` reply to a received request whose kind is one, sent to that
/// request's sender.
pub fn may_carry(kind: &str, frame: ControlFrame<'_>, target: &NodeId) -> bool {
    match frame {
        ControlFrame::Request => is_control_kind(kind),
        ControlFrame::ReplyTo(req) => {
            kind == crate::signal::signal_kind::RPC_RESULT && is_control_kind(&req.kind) && req.sender == *target
        }
    }
}

/// What became of a frame's lane copy ([`crate::ops::emit_signal_control`]). The shared copy is offered
/// whatever this says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneVerdict {
    /// The copy entered the lane's channel (`sent`).
    Sent,
    /// The lane's channel was full (`full`).
    Full,
    /// `control_lane = false`: no lane.
    Off,
    /// The kind may not ride the lane, or the frame is over [`CONTROL_LANE_MAX_FRAME_BYTES`] (`refused`).
    Refused,
    /// The target is a removed member (closure plan C5): no lane is opened to it (#832's review, F1).
    Removed,
    /// A reply to a peer this node has no data writer to and no `peers` entry for: no lane is opened
    /// for it (#832's review, F5).
    UnknownPeer,
    /// The peer's data writer exists and has no connected socket: no lane opens (M1).
    DataUnconnected,
    /// A lane connect to the peer failed recently: within [`CONTROL_LANE_REFUSED_BACKOFF_MS`] of a refused
    /// handshake, or within the reconnect backoff of a failed TCP connect.
    Backoff,
    /// The node is shutting down, or the lane closed twice under the copy.
    Unavailable,
}

/// What [`crate::ops::emit_signal_control`] did with one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlSend {
    /// The shared copy entered a gossip shard (as `emit_signal`'s `bool`).
    pub shared: bool,
    /// The lane copy.
    pub lane: LaneVerdict,
}

/// The lane's counters (`SystemStats::control_lane_*`, `control_late_duplicates`; design D8).
#[derive(Debug, Default)]
pub struct ControlLaneStats {
    /// Copies that entered a lane's channel.
    pub sent:            AtomicU64,
    /// Copies refused because the lane's channel was full.
    pub full:            AtomicU64,
    /// Lane connects that failed. A failure **after** the TCP connect succeeded — the handshake refused,
    /// which is how a per-source cap's accept-and-close looks — starts the long refused backoff
    /// ([`CONTROL_LANE_REFUSED_BACKOFF_MS`]); a TCP connect that failed or timed out (the peer down or
    /// restarting) starts only the node's reconnect backoff (#832's review, F3).
    pub connect_failed:  AtomicU64,
    /// Lanes closed because the peer's data writer lost its socket (M1).
    pub yielded:         AtomicU64,
    /// Copies a lane took and did not write: lost to a failed connect, a failed write or a yield. Not in
    /// `dropped_frames`, which stays the data path's.
    pub dropped:         Arc<AtomicU64>,
    /// Copies refused before the lane: a kind that may not ride it, or a frame over the size cap.
    pub refused:         AtomicU64,
    /// Requests of a control kind that reached the replay window after the seen-set forgot their nonce —
    /// the slow twin of a dual send (M2). Counted here **instead of** the caller-replay counter, so that
    /// counter stays a replay signal. A genuine replay of a control request is counted here too: it is
    /// harmless, a repeat being answered from what the provider holds.
    pub late_duplicates: AtomicU64,
    /// How long the most recent successful lane connect took, in milliseconds (also the
    /// `gossip_control_lane_connect_ms` histogram under `metrics`).
    pub last_connect_ms: AtomicU64,
}

/// A node's control lanes: one writer per peer in its own map beside the data writers, its refusal
/// backoffs, and its counters. Held by [`crate::CoreCtx::control_lane`].
pub struct ControlLane {
    enabled: bool,
    /// The lane writers, by peer. Never counted as peers: `gossip_peers_connected` and
    /// `cached_connections` read the data map only.
    writers: papaya::HashMap<NodeId, WriterEntry>,
    /// The data writers (the agent's `peer_writers`), read for the M1 rules.
    data:    Arc<papaya::HashMap<NodeId, WriterEntry>>,
    /// Per peer, the monotonic instant (`sim_seam::mono_now_ns`) before which no lane opens to it.
    refused_until: papaya::HashMap<NodeId, u64>,
    /// The counters.
    pub stats: ControlLaneStats,
}

impl ControlLane {
    /// Lanes beside `data` (the agent's `peer_writers`), on or off (`GossipConfig::control_lane`).
    pub fn new(enabled: bool, data: Arc<papaya::HashMap<NodeId, WriterEntry>>) -> Self {
        Self {
            enabled,
            writers: papaya::HashMap::new(),
            data,
            refused_until: papaya::HashMap::new(),
            stats: ControlLaneStats::default(),
        }
    }

    /// Whether lanes are on (`GossipConfig::control_lane`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The lane writers (read by the sites that close, reap and count them).
    pub fn writers(&self) -> &papaya::HashMap<NodeId, WriterEntry> {
        &self.writers
    }

    /// The data writers this lane set reads.
    pub fn data_writers(&self) -> &Arc<papaya::HashMap<NodeId, WriterEntry>> {
        &self.data
    }

    /// Whether `peer` has a data writer that is live and **not** connected — in its reconnect backoff,
    /// or before its first connect. A finished data writer (idled out) counts as none.
    fn data_unconnected(&self, peer: &NodeId) -> bool {
        self.data.pin().get(peer).is_some_and(|e| e.is_live() && !e.connected.load(Ordering::Acquire))
    }

    /// Whether a lane may open to (or take a frame for) `peer` now (D4, M1).
    pub fn may_open(&self, peer: &NodeId) -> Result<(), LaneVerdict> {
        if !self.enabled {
            return Err(LaneVerdict::Off);
        }
        if let Some(&until) = self.refused_until.pin().get(peer) {
            if crate::sim_seam::mono_now_ns() < until {
                return Err(LaneVerdict::Backoff);
            }
            self.refused_until.pin().compute(peer.clone(), |e| match e {
                Some((_, &u)) if u == until => papaya::Operation::Remove,
                _ => papaya::Operation::Abort(()),
            });
        }
        if self.data_unconnected(peer) {
            // The lane yields now rather than at its next tick (#832's review, F2): a data writer that exists
            // without a socket is the case the yield is for.
            let lane = self.writers.pin().get(peer).map(|e| e.is_live());
            if lane == Some(true) {
                self.stats.yielded.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_control_lane_yielded_total").increment(1);
            }
            if lane.is_some() {
                self.evict(peer);
            }
            return Err(LaneVerdict::DataUnconnected);
        }
        Ok(())
    }

    /// Whether the lane to `peer` must close (M1): its data writer exists and has lost (or not yet
    /// gained) its socket. Read by the lane's writer before each batch and on its yield tick.
    pub fn should_yield(&self, peer: &NodeId) -> bool {
        self.data_unconnected(peer)
    }

    /// A lane connect to `peer` failed. `handshake_refused` (the TCP connect succeeded and the handshake
    /// did not: a per-source refusal) means no lane opens to it for [`CONTROL_LANE_REFUSED_BACKOFF_MS`];
    /// otherwise (the peer down or restarting) for `reconnect`, the node's reconnect backoff.
    pub(crate) fn note_refused(&self, peer: &NodeId, handshake_refused: bool, reconnect: std::time::Duration) {
        let ms = if handshake_refused { CONTROL_LANE_REFUSED_BACKOFF_MS } else { reconnect.as_millis() as u64 };
        let until = crate::sim_seam::mono_now_ns().saturating_add(ms.saturating_mul(1_000_000));
        self.refused_until.pin().insert(peer.clone(), until);
        self.stats.connect_failed.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("gossip_control_lane_connect_failed_total").increment(1);
    }

    /// **Test-only:** end `peer`'s refused backoff now, as if [`CONTROL_LANE_REFUSED_BACKOFF_MS`] had
    /// passed — the clock is the seam's, so a test cannot wait 300 s.
    #[cfg(any(test, feature = "test-support"))]
    pub fn end_refused_backoff_for_test(&self, peer: &NodeId) {
        self.refused_until.pin().remove(peer);
    }

    /// **Test-only:** how much of `peer`'s connect backoff is left, if one runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn backoff_left_for_test(&self, peer: &NodeId) -> Option<std::time::Duration> {
        let until = *self.refused_until.pin().get(peer)?;
        Some(std::time::Duration::from_nanos(until.saturating_sub(crate::sim_seam::mono_now_ns())))
    }

    /// Offer `frame`, already encoded, to the lane toward `target`, opening it if the rules allow. Called
    /// only by [`crate::ops::emit_signal_control`], after the shared copy was offered.
    pub(crate) fn send(self: &Arc<Self>, ctx: &crate::CoreCtx, target: &NodeId, frame: Bytes, reply: bool) -> LaneVerdict {
        // Closure plan C5: never a connection of our own to a removed member. The data path has none either:
        // a removed member is not in `peers`, so it is no forwarding target (#832's review, F1).
        if ctx.removed.is_node_removed(target) {
            return LaneVerdict::Removed;
        }
        // A reply's target is an unauthenticated `sender`: no M7 lane for it (#832's review, F5).
        if reply && self.data.pin().get(target).is_none() && ctx.peers.pin().get(target).is_none() {
            return LaneVerdict::UnknownPeer;
        }
        if let Err(v) = self.may_open(target) {
            return v;
        }
        let timing = crate::writer::WriterTiming {
            idle:    std::time::Duration::from_secs(ctx.config.writer_idle_timeout_secs),
            connect: std::time::Duration::from_millis(ctx.config.handshake_timeout_ms),
            stall:   crate::stall::StallBound {
                stall:    std::time::Duration::from_millis(CONTROL_LANE_WRITE_BOUND_MS),
                min_rate: 0,
            },
            stalls:  Some(Arc::clone(&ctx.transport_bounds.outbound_stalls)),
        };
        let backoff = std::time::Duration::from_secs(ctx.hot.reconnect_backoff_secs(ctx.config.reconnect_backoff_secs));
        let tls = ctx.tls.get().cloned();
        let spawn = || crate::writer::get_or_spawn_lane(
            target, self, backoff, &timing, &ctx.shutdown_tx, tls.clone(), ctx.gossip_txs.unsent(),
        );
        let stream = format!("writer/control/{target}");
        let Some(tx) = spawn() else { return LaneVerdict::Unavailable };
        let mut verdict = tx.try_send_counted(&stream, frame.clone());
        // A copy enqueued as an idle lane shuts down lands in a closed channel: respawn and retry once, as
        // the shard's `send_to_peer!` does, so it is not counted as sent and lost.
        if verdict == crate::sim_seam::ChanVerdict::Closed {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(hook) = TEST_ON_CLOSED.pin().get(&target.to_string()) { hook(self); }
            let Some(tx) = spawn() else { return LaneVerdict::Unavailable };
            verdict = tx.try_send_counted(&stream, frame);
        }
        match verdict {
            crate::sim_seam::ChanVerdict::Sent => {
                self.stats.sent.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_control_lane_sent_total").increment(1);
                LaneVerdict::Sent
            }
            crate::sim_seam::ChanVerdict::Full => {
                let n = self.stats.full.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "metrics")]
                metrics::counter!("gossip_control_lane_full_total").increment(1);
                if n == 0 || (n + 1).is_multiple_of(64) {
                    tracing::warn!(peer = %target, full = n + 1, "control lane full: the frame goes by the shared path only");
                }
                LaneVerdict::Full
            }
            crate::sim_seam::ChanVerdict::Closed => LaneVerdict::Unavailable,
        }
    }

    /// Close the lane to `peer`, if one is open (the eviction sites of design §4.3).
    pub fn evict(&self, peer: &NodeId) {
        crate::writer::evict_peer_writer(&self.writers, peer);
        publish_lane_gauge(&self.writers);
    }

    /// Remove every lane whose writer has finished (the GC sweep) and publish the lane gauge.
    pub fn reap(&self) {
        let finished: Vec<NodeId> = self.writers.pin().iter()
            .filter(|(_, e)| !e.is_live())
            .map(|(k, _)| k.clone())
            .collect();
        crate::writer::reap_writers_only(&self.writers, finished);
        publish_lane_gauge(&self.writers);
    }

    /// Signal every lane to exit and empty the map (the agent's shutdown sweep).
    pub fn close_all(&self) {
        let guard = self.writers.pin();
        let keys: Vec<NodeId> = guard.iter()
            .map(|(k, v)| { let _ = v.peer_shutdown.send(true); k.clone() })
            .collect();
        for key in keys { guard.remove(&key); }
        drop(guard);
        publish_lane_gauge(&self.writers);
    }

    /// Lanes whose writer is live (`SystemStats::control_lanes_open`).
    pub fn open_count(&self) -> usize {
        self.writers.pin().iter().filter(|(_, e)| e.is_live()).count()
    }
}

/// **Test-only:** run when a lane copy finds its channel closed, before the respawn — so a test can stand in
/// for the GC reaping the exited writer in that window (the idle-exit race the retry is for).
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub static TEST_ON_CLOSED: std::sync::LazyLock<papaya::HashMap<String, Arc<dyn Fn(&ControlLane) + Send + Sync>>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// `gossip_control_lanes_connected`: the live lane writers — published where the lane map changes,
/// apart from `gossip_peers_connected`, which counts data writers only (M3).
pub fn publish_lane_gauge(writers: &papaya::HashMap<NodeId, WriterEntry>) {
    #[cfg(feature = "metrics")]
    {
        let live = writers.pin().iter().filter(|(_, e)| e.is_live()).count();
        metrics::gauge!("gossip_control_lanes_connected").set(live as f64);
    }
    #[cfg(not(feature = "metrics"))]
    let _ = writers;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **F2: the yield tick is shorter than a refused data writer's soonest retry** (half its backoff), so a
    /// data writer is refused at most once. Seen failing first with the tick fixed at 1 s.
    #[test]
    fn the_yield_tick_beats_a_refused_writers_retry() {
        use std::time::Duration;
        for secs in [1u64, 2, 5, 30, 300] {
            let backoff = Duration::from_secs(secs);
            assert!(yield_tick_ms(backoff) < backoff.as_millis() as u64 / 2, "TICK: {secs} s backoff, {} ms tick", yield_tick_ms(backoff));
        }
        assert_eq!(yield_tick_ms(Duration::from_secs(5)), CONTROL_LANE_YIELD_TICK_MS);
        assert_eq!(yield_tick_ms(Duration::ZERO), CONTROL_LANE_YIELD_TICK_MIN_MS);
    }

    /// The set is closed: exactly the kinds the design names, and a test kind only in test builds.
    #[test]
    fn the_control_kinds_are_the_closed_set() {
        assert_eq!(CONTROL_KINDS, &["sys.call-cancel.v1"]);
        assert!(is_control_kind("sys.call-cancel.v1"));
        assert!(!is_control_kind("rpc.result"), "a reply rides the lane only for a control request");
        assert!(!is_control_kind("sys.replica-sync.v1"));
        let peer = NodeId::new("127.0.0.1", 7001).unwrap();
        let req = |kind: &str| crate::signal::Signal {
            kind: Arc::from(kind), scope: crate::signal::SignalScope::Individual(peer.clone()),
            payload: Bytes::new(), sender: peer.clone(), nonce: 1,
        };
        assert!(may_carry("sys.call-cancel.v1", ControlFrame::Request, &peer));
        assert!(!may_carry("mcp.invoke", ControlFrame::Request, &peer));
        assert!(!may_carry("rpc.result", ControlFrame::Request, &peer), "a reply is not a request");
        assert!(may_carry("rpc.result", ControlFrame::ReplyTo(&req("sys.call-cancel.v1")), &peer));
        assert!(!may_carry("rpc.result", ControlFrame::ReplyTo(&req("mcp.invoke")), &peer), "the reply to a non-control request");
        let elsewhere = NodeId::new("127.0.0.1", 7002).unwrap();
        assert!(!may_carry("rpc.result", ControlFrame::ReplyTo(&req("sys.call-cancel.v1")), &elsewhere), "a reply goes to the request's sender only");
        assert!(!may_carry("mcp.invoke", ControlFrame::ReplyTo(&req("sys.call-cancel.v1")), &peer), "only an `rpc.result` is a reply");
    }
}
