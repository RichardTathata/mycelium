//! **B1, PR 1: the control lane's mechanism** (`docs/design/control-lane.md` §5, the tests it assigns to
//! PR 1: T2, T3, T6, T6b, T7, T10, and M2's counter). No production kind uses the lane yet, so these drive
//! it with the test-only kind [`TEST_CONTROL_KIND`] through [`emit_signal_control`]. Each was seen failing
//! first with the plant its doc names. Assertions are structural — what was delivered, on which
//! connection, and what was counted — never a duration.

use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use mycelium_core::control_lane::{LaneVerdict, CONTROL_LANE_DEPTH, TEST_CONTROL_KIND};
use mycelium_core::ops::emit_signal_control;
use mycelium_core::writer::{WriteHold, TEST_CONNECT_HOLD, TEST_WRITE_HOLD};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

/// Wait for a condition, bounded (a liveness bound, not an assertion on time).
async fn until(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if f() { return true; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

fn id(port: u16) -> NodeId {
    NodeId::new("127.0.0.1", port).unwrap()
}

/// A's data writer to `peer` exists, is live and holds a connected socket.
fn data_connected(a: &GossipAgent, peer: &NodeId) -> bool {
    a.peer_writers.pin().get(peer).is_some_and(|e| e.is_live() && e.connected.load(Ordering::Acquire))
}

/// A's control lane to `peer` exists in the lane map (live or finished).
fn lane_present(a: &GossipAgent, peer: &NodeId) -> bool {
    a.task_ctx.control_lane.writers().pin().get(peer).is_some()
}

/// A's control lane to `peer` exists, is live and holds a connected socket.
fn lane_connected(a: &GossipAgent, peer: &NodeId) -> bool {
    a.task_ctx.control_lane.writers().pin().get(peer).is_some_and(|e| e.is_live() && e.connected.load(Ordering::Acquire))
}

/// One frame of the test control kind from `a` to `to`, its payload the number `i`.
fn emit(a: &GossipAgent, to: &NodeId, i: u64) -> mycelium_core::control_lane::ControlSend {
    emit_signal_control(&a.task_ctx, Arc::from(TEST_CONTROL_KIND), to.clone(), Bytes::from(i.to_le_bytes().to_vec()), TEST_CONTROL_KIND)
}

/// Collect the numbers of the test-kind frames `rx` receives until it has `want` distinct ones, bounded.
async fn collect(rx: &mut tokio::sync::mpsc::Receiver<crate::Signal>, want: usize, within: Duration) -> std::collections::BTreeSet<u64> {
    let mut got = std::collections::BTreeSet::new();
    let deadline = tokio::time::Instant::now() + within;
    while got.len() < want {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(sig)) if sig.payload.len() == 8 => { got.insert(u64::from_le_bytes(sig.payload[..8].try_into().unwrap())); }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    got
}

/// Two plaintext nodes, A bootstrapping to B, each configured by its tweak; returns once A's data writer to
/// B is connected and B has heard from A.
async fn pair(tweak_a: impl Fn(&mut GossipConfig), tweak_b: impl Fn(&mut GossipConfig)) -> (GossipAgent, GossipAgent, NodeId, NodeId) {
    let (pa, pb) = (alloc_port(), alloc_port());
    let (na, nb) = (id(pa), id(pb));
    let mut cb = GossipConfig::default();
    cb.bind_port = pb;
    cb.health_check_max_jitter_ms = 50;
    tweak_b(&mut cb);
    let b = GossipAgent::new(nb.clone(), cb);
    b.start().await.unwrap();
    let mut ca = GossipConfig::default();
    ca.bind_port = pa;
    ca.bootstrap_peers = vec![nb.clone()];
    ca.health_check_max_jitter_ms = 50;
    tweak_a(&mut ca);
    let a = GossipAgent::new(na.clone(), ca);
    a.start().await.unwrap();
    assert!(until(Duration::from_secs(15), || data_connected(&a, &nb) && b.peers().contains(&na)).await, "A and B never connected");
    (a, b, na, nb)
}

type HookMap = std::sync::LazyLock<papaya::HashMap<String, Arc<WriteHold>>>;

/// Removes the test hooks it names when dropped, so a failing test leaves no hold behind for the next.
struct Hooks(Vec<(&'static HookMap, String)>);

impl Hooks {
    fn hold(&mut self, map: &'static HookMap, key: String) -> Arc<WriteHold> {
        let h = WriteHold::new();
        map.pin().insert(key.clone(), Arc::clone(&h));
        self.0.push((map, key));
        h
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for (map, key) in &self.0 {
            if let Some(h) = map.pin().remove(key) { h.release(); }
        }
    }
}

/// **T2: a control frame is never dropped while the lane has room.** A's data writer to B is held in a
/// write and its channel is full, so the shared path delivers nothing. A's lane to B is held in its connect,
/// holding the one frame that opened it: its channel then takes exactly [`CONTROL_LANE_DEPTH`] frames, all
/// counted `sent`, and the next is `full` — counted, its shared copy offered. Released, the lane delivers
/// every frame it took, **while the data writer is still held**: they crossed on the lane's own socket.
///
/// Seen failing first with the lane copy routed into the data writer's channel (`ControlLane::send` handing
/// the frame to `peer_writers`): the first copy was `full` and nothing reached B.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_frame_is_never_dropped_while_the_lane_has_room() {
    let (a, b, _na, nb) = pair(|_| {}, |_| {}).await;
    let mut rx = b.mesh().signal_rx_with_capacity(TEST_CONTROL_KIND, 1_024);
    let mut hooks = Hooks(Vec::new());

    // Hold A's data writer to B in a write, then fill its channel.
    let data_hold = hooks.hold(&TEST_WRITE_HOLD, nb.to_string());
    let mut i = 0u64;
    assert!(until(Duration::from_secs(10), || {
        i += 1;
        let _ = a.kv().set(format!("t2/fill/{i}"), b"x".to_vec());
        data_hold.reached.load(Ordering::SeqCst) >= 1
    }).await, "A's data writer never reached its hold");
    assert!(until(Duration::from_secs(20), || {
        for _ in 0..64 { i += 1; let _ = a.kv().set(format!("t2/fill/{i}"), b"x".to_vec()); }
        a.peer_writers.pin().get(&nb).is_some_and(|e| e.tx.capacity() == 0)
    }).await, "A's data writer channel to B never filled");
    assert!(data_connected(&a, &nb), "the held data writer still holds its socket");

    // The lane, held in its connect with the first frame in hand.
    let connect_hold = hooks.hold(&TEST_CONNECT_HOLD, format!("control/{nb}"));
    assert_eq!(emit(&a, &nb, 0).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || connect_hold.reached.load(Ordering::SeqCst) == 1).await, "the lane never reached its connect");
    for n in 1..=CONTROL_LANE_DEPTH as u64 {
        assert_eq!(emit(&a, &nb, n).lane, LaneVerdict::Sent, "frame {n} was refused while the lane had room");
    }
    let stats = &a.task_ctx.control_lane.stats;
    assert_eq!(stats.sent.load(Ordering::Relaxed), CONTROL_LANE_DEPTH as u64 + 1);
    assert_eq!(stats.full.load(Ordering::Relaxed), 0, "no copy was refused while the lane had room");
    let over = emit(&a, &nb, 999);
    assert_eq!(over.lane, LaneVerdict::Full, "the frame past the lane's depth is refused by the lane");
    assert!(over.shared, "and its shared copy was offered");
    assert_eq!(stats.full.load(Ordering::Relaxed), 1);

    connect_hold.release();
    let got = collect(&mut rx, CONTROL_LANE_DEPTH + 1, Duration::from_secs(15)).await;
    let want: std::collections::BTreeSet<u64> = (0..=CONTROL_LANE_DEPTH as u64).collect();
    assert_eq!(got, want, "every frame the lane took reached B");
    assert!(data_hold.reached.load(Ordering::SeqCst) >= 1 && data_connected(&a, &nb), "…while A's data writer was still held");
    assert_eq!(stats.dropped.load(Ordering::Relaxed), 0);

    drop(hooks);
    a.shutdown().await;
    b.shutdown().await;
}

/// **T3: gossip progresses while the control lane is held.** A's lane to B is held in a write
/// (`control/{B}`) and A fills it. A's KV write still reaches B, and the data writer kept its socket: two
/// tasks, two sockets, no arbitration.
///
/// Seen failing first with the lane's write made in the data writer's task — planted as the data writer's
/// write path waiting on the lane's hold, which is what one task serving both channels (option (a) of the
/// design) does when the lane's write stalls: the KV write never reached B.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gossip_progresses_while_the_control_lane_is_held() {
    let (a, b, _na, nb) = pair(|_| {}, |_| {}).await;
    let mut hooks = Hooks(Vec::new());
    let control_hold = hooks.hold(&TEST_WRITE_HOLD, format!("control/{nb}"));

    let mut full = false;
    for n in 0..2_000u64 {
        if emit(&a, &nb, n).lane == LaneVerdict::Full { full = true; break; }
    }
    assert!(until(Duration::from_secs(10), || control_hold.reached.load(Ordering::SeqCst) >= 1).await, "the control frame's write never reached its hold");
    assert!(full, "the held lane filled");

    let _ = a.kv().set("t3/after", b"v".to_vec());
    assert!(
        until(Duration::from_secs(15), || b.kv().get("t3/after").is_some()).await,
        "HELD: A's gossip waited behind its held control lane",
    );
    assert!(data_connected(&a, &nb), "the data writer kept its socket");

    drop(hooks);
    a.shutdown().await;
    b.shutdown().await;
}

/// **T10 (M4): a lane's reader does not wait for the WAL.** B persists under `flush`, and every append of
/// quorum evidence (`sys/quorum/…`) is held — the held WAL acknowledgement. A's data writer to B is held, so
/// frames reach B only on the lane. Two control frames cross it in sequence: B delivers **both** while the
/// first one's evidence append is still held. The evidence was applied to B's store before its append.
///
/// Seen failing first with the evidence append put back inline in the reader for control kinds: the second
/// frame waited behind the first one's held append and was not delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_reader_does_not_wait_for_the_wal() {
    let base = std::env::temp_dir().join(format!("myc-b1-t10-{}", alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let b_base = base.clone();
    let (a, b, na, nb) = pair(|_| {}, move |c| {
        c.persistence = Some(mycelium_core::config::PersistenceConfig {
            base_path: b_base.clone(),
            sync_mode: mycelium_core::config::SyncMode::Flush,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600,
            on_unreadable: Default::default(),
        });
    }).await;
    let mut rx = b.mesh().signal_rx_with_capacity(TEST_CONTROL_KIND, 64);
    let wal_hold = b.task_ctx.wal.get().expect("B has a WAL").hold_appends_under("sys/quorum/");
    let mut hooks = Hooks(Vec::new());
    let data_hold = hooks.hold(&TEST_WRITE_HOLD, nb.to_string());
    let mut i = 0u64;
    assert!(until(Duration::from_secs(10), || {
        i += 1;
        let _ = a.kv().set(format!("t10/fill/{i}"), b"x".to_vec());
        data_hold.reached.load(Ordering::SeqCst) >= 1
    }).await, "A's data writer never reached its hold");

    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert_eq!(collect(&mut rx, 1, Duration::from_secs(10)).await, [1].into(), "the first frame crossed the lane");
    assert!(until(Duration::from_secs(10), || wal_hold.reached.load(Ordering::SeqCst) >= 1).await, "the first frame's evidence append never reached the held WAL");
    let evidence = format!("sys/quorum/{TEST_CONTROL_KIND}/{na}");
    assert!(b.kv().get(&evidence).is_some(), "the evidence is applied before its append: apply, then hand to the WAL");

    assert_eq!(emit(&a, &nb, 2).lane, LaneVerdict::Sent);
    assert_eq!(
        collect(&mut rx, 1, Duration::from_secs(10)).await, [2].into(),
        "WAITED: the second control frame waited behind the first one's held evidence append",
    );

    wal_hold.release();
    drop(hooks);
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// Two TLS nodes on one CA. B is configured by `tweak_b`; A bootstraps to B only when `a_boots_b`.
#[cfg(feature = "tls")]
async fn tls_pair(tag: &str, a_boots_b: bool, tweak_b: impl Fn(&mut GossipConfig)) -> (GossipAgent, GossipAgent, NodeId, NodeId, std::path::PathBuf) {
    let (pa, pb) = (alloc_port(), alloc_port());
    let (na, nb) = (id(pa), id(pb));
    let dir = std::env::temp_dir().join(format!("myc-b1-{tag}-{pa}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut c = GossipConfig::default();
        c.bind_port = port;
        c.bootstrap_peers = boots;
        c.reconnect_backoff_secs = 1;
        c.health_check_max_jitter_ms = 50;
        c.tls = Some({
            let mut t = crate::config::TlsConfig::default();
            t.auto_cert_dir = dir.clone();
            t
        });
        c
    };
    // B first: it generates the fleet CA in `dir`, which A then shares.
    let mut cb = mk(pb, vec![]);
    tweak_b(&mut cb);
    let b = GossipAgent::new(nb.clone(), cb);
    b.start().await.unwrap();
    let a = GossipAgent::new(na.clone(), mk(pa, if a_boots_b { vec![nb.clone()] } else { vec![] }));
    a.start().await.unwrap();
    (a, b, na, nb, dir)
}

/// **T6: a refused lane connect falls back, and backs off** (M1; under TLS). B takes one connection per
/// source address, and A's data connection holds it. A's lane to B is refused in its handshake: counted
/// `connect_failed`, the frame delivered by its shared copy, the data connection undisturbed. A second
/// frame inside the refused backoff opens no lane (`Backoff`, no second connect). Once the backoff is over —
/// ended by the test hook, since its clock is the seam's — the next frame tries again.
///
/// **Plaintext is not covered, stated:** without TLS an accept-and-close refusal is not seen as a failed
/// connect — the lane's first write appears to succeed — so `connect_failed` can stay 0 there and the loss
/// shows in `control_lane_dropped` instead.
///
/// Seen failing first with no refused backoff (`note_refused` recording no deadline): the second frame
/// connected again, and `connect_failed` reached 2 inside the backoff.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_lane_connect_falls_back_and_backs_off() {
    let (a, b, na, nb, dir) = tls_pair("t6", true, |c| c.max_connections_per_source = 1).await;
    assert!(until(Duration::from_secs(15), || data_connected(&a, &nb) && b.peers().contains(&na)).await, "A and B never connected");
    let mut rx = b.mesh().signal_rx_with_capacity(TEST_CONTROL_KIND, 64);
    let stats = &a.task_ctx.control_lane.stats;

    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent, "the lane opens: A's data writer is connected");
    assert!(until(Duration::from_secs(15), || stats.connect_failed.load(Ordering::Relaxed) == 1).await, "the refused lane connect was not counted");
    assert!(b.system_stats().inbound_per_source_refused >= 1, "B refused the lane under its per-source cap");
    assert_eq!(collect(&mut rx, 1, Duration::from_secs(10)).await, [1].into(), "the shared copy delivered the frame");
    assert!(data_connected(&a, &nb), "the data connection was undisturbed");

    assert_eq!(emit(&a, &nb, 2).lane, LaneVerdict::Backoff, "BACKOFF: a lane was opened again inside the refused backoff");
    assert_eq!(collect(&mut rx, 1, Duration::from_secs(10)).await, [2].into(), "the shared copy delivered the second frame");
    assert_eq!(stats.connect_failed.load(Ordering::Relaxed), 1, "no second connect inside the backoff");
    assert!(!lane_present(&a, &nb) || !lane_connected(&a, &nb));

    a.task_ctx.control_lane.end_refused_backoff_for_test(&nb);
    assert_eq!(emit(&a, &nb, 3).lane, LaneVerdict::Sent, "past the backoff the lane is tried again");
    assert!(until(Duration::from_secs(15), || stats.connect_failed.load(Ordering::Relaxed) == 2).await, "…and refused again, counted");
    assert!(data_connected(&a, &nb));

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **T6b (M1): a data writer reconnects past an open lane** (under TLS). B takes one connection per source.
/// A has no data writer to B, so its lane opens first and takes B's one slot. Then A's data writer is
/// started and refused: it sits unconnected, the lane **yields** (`yielded` = 1), B's slot frees, and the data
/// writer connects on its next attempt — gossip to B resumes. A never idles a writer out here, so without the
/// yield the lane would hold the slot for good.
///
/// Seen failing first with the yield check removed (`ControlLane::should_yield` always `false`): the data
/// writer was refused on every attempt while the lane stayed open.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_writer_reconnects_past_an_open_lane() {
    let (a, b, _na, nb, dir) = tls_pair("t6b", false, |c| c.max_connections_per_source = 1).await;
    let mut rx = b.mesh().signal_rx_with_capacity(TEST_CONTROL_KIND, 64);
    let stats = &a.task_ctx.control_lane.stats;

    assert!(a.peer_writers.pin().get(&nb).is_none(), "A starts with no data writer to B");
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent, "a peer with no data writer gets a lane (M7)");
    assert_eq!(collect(&mut rx, 1, Duration::from_secs(15)).await, [1].into(), "the frame crossed the lane: A has no other route to B");
    assert!(lane_connected(&a, &nb), "the lane holds B's one per-source slot");

    // A's data writer to B: refused while the lane holds the slot, then admitted once it yields.
    let mut data_rx = b.mesh().signal_rx_with_capacity("test.b1.data", 64);
    a.connect_peer(nb.clone());
    assert!(
        until(Duration::from_secs(30), || { a.connect_peer(nb.clone()); data_connected(&a, &nb) }).await,
        "STARVED: A's data writer never reconnected past the open lane",
    );
    assert_eq!(stats.yielded.load(Ordering::Relaxed), 1, "the lane yielded to the unconnected data writer");
    assert!(b.system_stats().inbound_per_source_refused >= 1, "the data writer was refused while the lane held the slot");
    let mut resumed = false;
    for _ in 0..75 {
        let _ = mycelium_core::ops::emit_signal(&a.task_ctx, Arc::from("test.b1.data"), crate::SignalScope::Individual(nb.clone()), Bytes::from_static(b"d"));
        if matches!(tokio::time::timeout(Duration::from_millis(200), data_rx.recv()).await, Ok(Some(_))) { resumed = true; break; }
    }
    assert!(resumed, "gossip to B resumed on the data connection");
    assert!(!lane_connected(&a, &nb), "the yielded lane stayed closed: no control frame was sent since");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **T7, the shutdown sweep:** a node's shutdown closes its lanes and empties the lane map, and the
/// outbound count has no underflow. Seen failing first with `close_all` left out of the sweep: the lane's
/// entry stayed in the map.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_writer_site_handles_the_lane_shutdown() {
    let (a, b, _na, nb) = pair(|c| c.health_check_interval_secs = 60, |_| {}).await;
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || lane_connected(&a, &nb)).await);
    let unsent = a.task_ctx.gossip_txs.unsent().clone();
    a.shutdown().await;
    assert!(!lane_present(&a, &nb), "SITE: the shutdown sweep left the lane in its map");
    assert_eq!(unsent.underflows(), 0);
    b.shutdown().await;
}

/// **T7, the GC sweep:** a lane that idled out is reaped from the lane map at the GC tick. Seen failing
/// first with `control_lane.reap()` left out of the GC task: the finished lane's entry stayed. The GC ticks at
/// `max(60 s, 10 × health_check_interval_secs)`, so this test waits for one tick (bounded at 90 s).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_writer_site_handles_the_lane_reap() {
    let (a, b, _na, nb) = pair(|c| { c.health_check_interval_secs = 1; c.writer_idle_timeout_secs = 1; }, |_| {}).await;
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || lane_present(&a, &nb)).await);
    assert!(
        until(Duration::from_secs(90), || !lane_present(&a, &nb)).await,
        "SITE: the GC sweep left an idled-out lane in its map",
    );
    a.shutdown().await;
    b.shutdown().await;
}

/// **T7, `disconnect_peer`:** dropping a peer's direct route closes its lane. Seen failing first with the
/// eviction left out of `disconnect_peer`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_writer_site_handles_the_lane_disconnect() {
    let (a, b, _na, nb) = pair(|c| c.health_check_interval_secs = 60, |_| {}).await;
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || lane_connected(&a, &nb)).await);
    a.disconnect_peer(&nb);
    assert!(!lane_present(&a, &nb), "SITE: disconnect_peer left the lane open");
    a.shutdown().await;
    b.shutdown().await;
}

/// **T7, SWIM's death of a member:** a member SWIM confirms dead loses its lane with its data writer. The GC
/// tick is a minute away, so only the eviction can empty the map. Seen failing first with the lane left out
/// of SWIM's `BecameDead`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_writer_site_handles_the_lane_swim_death() {
    let swim = |c: &mut GossipConfig| {
        c.health_check_interval_secs = 60;
        c.swim_failure_detector = true;
        c.swim_probe_interval_ms = 200;
        c.swim_probe_timeout_ms = 100;
        c.swim_suspicion_timeout_ms = 600;
    };
    let (a, b, na, nb) = pair(swim, swim).await;
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    assert!(until(Duration::from_secs(10), || a.peers().contains(&nb) && b.peers().contains(&na)).await);
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || lane_connected(&a, &nb)).await);
    b.shutdown().await;
    assert!(until(Duration::from_secs(20), || !a.peers().contains(&nb)).await, "SWIM never confirmed B dead");
    assert!(
        until(Duration::from_secs(5), || !lane_present(&a, &nb)).await,
        "SITE: SWIM's death of a member left its lane open",
    );
    a.shutdown().await;
}

/// **T7, the health monitor's evictions** (the forwarding set's de-pin and shuffle, a peer leaving the
/// set, a peer timing out): every one goes through `evict_links`, which closes the lane with the data
/// writer. Read from the source, since the de-pin and the shuffle need a fleet far larger than a test's:
/// no bare `evict_peer_writer(&peer_writers, …)` is left in the monitor. Seen failing first with one site
/// put back to the bare call.
#[test]
fn every_writer_site_handles_the_lane_health_monitor() {
    let src = include_str!("agent/tasks.rs");
    let bare = src.matches("evict_peer_writer(&peer_writers").count();
    assert_eq!(bare, 0, "SITE: a health-monitor eviction closes the data writer and not its lane");
    assert_eq!(src.matches("evict_links(&peer_writers, &control_lane, ").count(), 4, "the four eviction sites of design §4.3");
    let helper = src.split("fn evict_links(").nth(1).expect("evict_links exists");
    let body = &helper[..helper.find("\n}\n").expect("evict_links ends")];
    assert!(body.contains("evict_peer_writer(peer_writers, peer)") && body.contains("control_lane.evict(peer)"), "evict_links closes both");
}

/// **T7, the gauges and counts (M3):** a lane is not a peer connection. `cached_connections` and
/// `connected_peers` count data writers only; the lane shows in `control_lanes_open`. Seen failing first with
/// `cached_connections` counting the lane map too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_is_not_counted_as_a_peer_connection() {
    let (a, b, _na, nb) = pair(|c| c.health_check_interval_secs = 60, |_| {}).await;
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    let before = a.system_stats().cached_connections;
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(10), || lane_connected(&a, &nb)).await);
    let s = a.system_stats();
    assert_eq!(s.control_lanes_open, 1);
    assert_eq!(s.cached_connections, before, "COUNTED: the lane was counted as a peer connection");
    assert_eq!(a.connected_peers().len(), before, "connected_peers lists data writers only");
    a.shutdown().await;
    b.shutdown().await;
}

/// **T7, the gauge selector (M3):** claiming a lane publishes `gossip_control_lanes_connected` and leaves
/// `gossip_peers_connected` at the data writers' count. Two data writers and one lane, so the counts differ.
/// A local recorder on this thread sees the gauges each claim publishes, synchronously. Seen failing first
/// with the selector removed (`claim_or_spawn` publishing `gossip_peers_connected` from whichever map it
/// claimed in): the gauge read 1 after the lane's claim.
#[cfg(feature = "metrics")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lane_gauge_is_not_the_peer_gauge() {
    use metrics::{Counter, Gauge, GaugeFn, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
    #[derive(Clone, Default)]
    struct Gauges(Arc<papaya::HashMap<String, u64>>);
    struct One(String, Gauges);
    impl GaugeFn for One {
        fn increment(&self, _: f64) {}
        fn decrement(&self, _: f64) {}
        fn set(&self, v: f64) { self.1.0.pin().insert(self.0.clone(), v.to_bits()); }
    }
    impl Recorder for Gauges {
        fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn register_counter(&self, _: &Key, _: &Metadata<'_>) -> Counter { Counter::noop() }
        fn register_gauge(&self, key: &Key, _: &Metadata<'_>) -> Gauge { Gauge::from_arc(Arc::new(One(key.name().to_string(), self.clone()))) }
        fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram { Histogram::noop() }
    }
    let rec = Gauges::default();
    let a = GaugeTestAgent::new();
    let (x1, x2, y) = (id(alloc_port()), id(alloc_port()), id(alloc_port()));
    metrics::with_local_recorder(&rec, || {
        for x in [&x1, &x2] {
            let _ = mycelium_core::writer::get_or_spawn_writer(
                x, &a.0.peer_writers, 16, Duration::from_secs(5),
                &mycelium_core::writer::WriterTiming::with_idle(Duration::ZERO),
                &a.0.task_ctx.shutdown_tx, &a.0.kv_state.dropped_frames, None, a.0.task_ctx.gossip_txs.unsent(),
            );
        }
        assert_eq!(emit(&a.0, &y, 1).lane, LaneVerdict::Sent, "a peer with no data writer gets a lane");
    });
    let read = |name: &str| rec.0.pin().get(name).map(|b| f64::from_bits(*b));
    assert_eq!(read("gossip_peers_connected"), Some(2.0), "GAUGE: the lane's claim set the peer gauge");
    assert_eq!(read("gossip_control_lanes_connected"), Some(1.0));
}

/// An agent that is built and never started (its writers connect to nothing), shut down on drop.
#[cfg(feature = "metrics")]
struct GaugeTestAgent(GossipAgent);

#[cfg(feature = "metrics")]
impl GaugeTestAgent {
    fn new() -> Self {
        let mut c = GossipConfig::default();
        c.bind_port = alloc_port();
        Self(GossipAgent::new(id(c.bind_port), c))
    }
}

#[cfg(feature = "metrics")]
impl Drop for GaugeTestAgent {
    fn drop(&mut self) {
        let _ = self.0.task_ctx.shutdown_tx.send(true);
    }
}

/// **T7, C5 member removal:** removing a member closes its lane — the lane is an mTLS connection to it. Seen
/// failing first with the lane left out of the removal (a blind remove of the data writer only).
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_writer_site_handles_the_lane_member_removal() {
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::membership::{MemberRemoval, RemovalOffer, SignedMemberRemoval};
    use ed25519_dalek::{Signer, SigningKey};
    let (a, b, na, nb, dir) = tls_pair("t7-c5", true, |_| {}).await;
    assert!(until(Duration::from_secs(15), || data_connected(&a, &nb) && b.peers().contains(&na)).await, "A and B never connected");
    let _rx = b.mesh().signal_rx(TEST_CONTROL_KIND);
    assert_eq!(emit(&a, &nb, 1).lane, LaneVerdict::Sent);
    assert!(until(Duration::from_secs(15), || lane_connected(&a, &nb)).await, "the lane never connected");

    let operator = SigningKey::from_bytes(&[61u8; 32]);
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:membership").unwrap(), operator.verifying_key().to_bytes()).unwrap();
    a.with_membership_authorities([IssuerId::new("operator:membership").unwrap()], external);
    let removal = MemberRemoval {
        authority: IssuerId::new("operator:membership").unwrap(),
        node: nb.clone(),
        keys: vec![],
        seq: 1,
        issued_at_ms: 1_000,
        reason: "b1-t7".into(),
    };
    let signed = SignedMemberRemoval { signature: operator.sign(&removal.canonical_bytes()).to_bytes().to_vec(), removal };
    assert_eq!(a.offer_member_removal(&signed), Some(RemovalOffer::Accepted));
    assert!(!lane_present(&a, &nb), "SITE: a removed member kept its control lane");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **M2: a control kind's late twin is a late duplicate, not a replay.** The same signed envelope received
/// twice: for a control kind the second is counted in `control_late_duplicates` and `caller_replay` is
/// unchanged; for any other kind it is counted in `caller_replay` as before. Both are refused unanswered.
/// Seen failing first with the classification removed: the control kind's twin raised `caller_replay`.
#[cfg(all(feature = "tls", feature = "gateway"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_twin_of_a_control_request_is_a_late_duplicate_not_a_replay() {
    use crate::agent::gateway_caller::{frame_with_context, verify_received};
    let (a, b, _na, _nb, dir) = tls_pair("m2", false, |_| {}).await;
    let request = |kind: &str, framed: Bytes| {
        let mut buf = bytes::BytesMut::with_capacity(8 + framed.len());
        bytes::BufMut::put_u64_le(&mut buf, 42);
        bytes::BufMut::put(&mut buf, framed);
        crate::agent::rpc::RpcRequest::from(crate::Signal {
            kind: Arc::from(kind),
            scope: crate::SignalScope::Individual(a.node_id().clone()),
            payload: buf.freeze(),
            sender: a.node_id().clone(),
            nonce: 7,
        })
    };
    let replayed = || a.task_ctx.caller_replay.replayed.load(Ordering::Relaxed);
    let late = || a.task_ctx.control_lane.stats.late_duplicates.load(Ordering::Relaxed);

    let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], Bytes::from_static(b"cancel")).unwrap();
    assert!(verify_received(&a.task_ctx, &request(TEST_CONTROL_KIND, framed.clone())).is_ok(), "the first copy is served");
    let twin = verify_received(&a.task_ctx, &request(TEST_CONTROL_KIND, framed));
    assert!(twin.as_ref().is_err_and(|e| e.unanswered()), "the twin is refused unanswered");
    assert_eq!((late(), replayed()), (1, 0), "REPLAY: a control kind's late twin was counted as a caller replay");

    let framed = frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], Bytes::from_static(b"other")).unwrap();
    assert!(verify_received(&a.task_ctx, &request("test.b1.not-control", framed.clone())).is_ok());
    assert!(verify_received(&a.task_ctx, &request("test.b1.not-control", framed)).is_err());
    assert_eq!((late(), replayed()), (1, 1), "any other kind's replay is still a caller replay");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **D7: a relay never puts a frame it received on a lane.** A strict line A → R → P (A capped to one active
/// connection, SWIM off, so A has no route to P but R). A sends a frame of a control kind by the shared path;
/// R relays it and P receives it, and R opened no lane and sent nothing on one: nothing on the receive path
/// promotes a frame to the lane, whatever its kind (the full T4a, with the cancel-forward, is B1's PR 2).
/// Seen failing first with the relay promoting by kind (the forward in `connection.rs` also offering the
/// frame to its target's lane): R's `control_lane_sent` was 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_never_puts_a_received_frame_on_a_lane() {
    let (pa, pr, pp) = (alloc_port(), alloc_port(), alloc_port());
    let mk = |port: u16, boots: Vec<NodeId>, max_active: usize| {
        let mut c = GossipConfig::default();
        c.bind_port = port;
        c.bootstrap_peers = boots;
        c.reconnect_backoff_secs = 1;
        c.max_active_connections = max_active;
        c.swim_failure_detector = false;
        GossipAgent::new(id(port), c)
    };
    let p = mk(pp, vec![], 0);
    let r = mk(pr, vec![id(pp)], 0);
    let a = mk(pa, vec![id(pr)], 1);
    p.start().await.unwrap();
    r.start().await.unwrap();
    a.start().await.unwrap();
    let mut rx = p.mesh().signal_rx(TEST_CONTROL_KIND);
    assert!(until(Duration::from_secs(10), || !a.peers().is_empty() && !r.peers().is_empty()).await, "the line never linked");

    assert!(a.mesh().emit(TEST_CONTROL_KIND, crate::SignalScope::Individual(id(pp)), Bytes::from(7u64.to_le_bytes().to_vec())));
    assert_eq!(collect(&mut rx, 1, Duration::from_secs(10)).await, [7].into(), "P received the relayed frame");
    let rs = r.system_stats();
    assert_eq!((rs.control_lane_sent, rs.control_lanes_open), (0, 0), "RELAY: R put a frame it received on a lane");
    assert_eq!(a.system_stats().control_lane_sent, 0, "A sent by the shared path only");

    a.shutdown().await;
    r.shutdown().await;
    p.shutdown().await;
}
