//! Row B of the post-360 hardening plan (2026-10-10): core resource bounds, end to end on real
//! sockets. Each test here was seen failing on the code before the row.

use crate::framing::{write_frame, WireMessage};
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId, SignalScope};
use bytes::Bytes;
use mycelium_core::codec::wire_to_bytes;
use std::{sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpStream};

async fn until(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if f() { return true; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    f()
}

/// Finding (22): the accept loop took a `max_connections` permit and then waited on the TLS
/// handshake — and the read loop on the first frame — with no bound, so `max_connections` sockets
/// that connect and say nothing held every permit and no peer could connect again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_sockets_that_never_speak_are_closed_and_a_new_peer_connects() {
    let (px, py) = (alloc_port(), alloc_port());
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.max_connections = 2;
    cx.handshake_timeout_ms = 300;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();

    // Two sockets that connect and never send a byte take both permits.
    let mut idle_a = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    let mut idle_b = TcpStream::connect(("127.0.0.1", px)).await.unwrap();

    // Past the handshake bound, the node closes them: a read sees end-of-stream.
    for s in [&mut idle_a, &mut idle_b] {
        let mut buf = [0u8; 16];
        let r = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await;
        assert!(matches!(r, Ok(Ok(0)) | Ok(Err(_))),
                "a socket that never spoke must be closed after handshake_timeout_ms; got {r:?}");
    }
    assert!(x.system_stats().inbound_connections_timed_out >= 2);

    // And a real peer connects: its write reaches X.
    let y = GossipAgent::new(
        NodeId::new("127.0.0.1", py).unwrap(),
        GossipConfig {
            bind_port: py,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", px).unwrap()],
            reconnect_backoff_secs: 1,
            ..Default::default()
        },
    );
    y.start().await.unwrap();
    assert!(until(Duration::from_secs(5), || !y.peers().is_empty()).await);
    assert!(y.kv().set("bounds/after-idle", Bytes::from_static(b"ok")));
    assert!(
        until(Duration::from_secs(10), || x.kv().get("bounds/after-idle").is_some()).await,
        "a peer must be able to connect once the idle sockets' permits are returned",
    );
    x.shutdown().await;
    y.shutdown().await;
}

/// Finding (23), the anti-entropy half: each honoured `StateRequest` spawned a task that queued a
/// dump of the store for the requester's writer, and a reconnect reset the per-connection cooldown;
/// against a peer that never reads, the dumps piled up one per reconnect. Now a peer has at most one
/// reply in flight and the rest are skipped and counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_never_reads_gets_one_anti_entropy_reply_at_a_time() {
    let px = alloc_port();
    let x = GossipAgent::new(
        NodeId::new("127.0.0.1", px).unwrap(),
        GossipConfig { bind_port: px, ..Default::default() },
    );
    x.start().await.unwrap();
    // ~12 MB of store, so one dump is far past the socket buffers of a peer that never reads.
    let value = Bytes::from(vec![3u8; 48 * 1024]);
    for i in 0..256 {
        assert!(x.kv().set(format!("bounds/bulk/{i}"), value.clone()));
    }

    // The stalled peer: accepts X's writer and never reads.
    let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let f = NodeId::new("127.0.0.1", stalled.local_addr().unwrap().port()).unwrap();
    let _hold = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = stalled.accept().await { held.push(s); }
    });

    // Introduce F, then ask for a full dump on ten fresh connections (each a fresh cooldown).
    let mut intro = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    write_frame(&mut intro, &wire_to_bytes(&WireMessage::Ping { sender: f.clone(), known_peers: vec![] }))
        .await.unwrap();
    assert!(until(Duration::from_secs(5), || x.peers().contains(&f)).await, "X learned F");
    let mut conns = Vec::new();
    for _ in 0..10 {
        let mut c = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
        let req = WireMessage::StateRequest { sender: f.clone(), store_hash: 0, bucket_hashes: vec![] };
        write_frame(&mut c, &wire_to_bytes(&req)).await.unwrap();
        conns.push(c);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        until(Duration::from_secs(5), || x.system_stats().anti_entropy_replies_skipped >= 8).await,
        "with F not reading, at most one reply may be in flight; skipped = {}",
        x.system_stats().anti_entropy_replies_skipped,
    );
    x.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding (25): admission of a Cluster or Group signal rolled against the MAX fill over the
/// kind's subscribers, so one subscriber that stopped reading — an SSE client, a serve stream —
/// drove the fill to 1.0 and nothing of that kind was admitted on the node, for anyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_subscriber_does_not_stop_admission_for_another() {
    let (pa, pb) = (alloc_port(), alloc_port());
    let a = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", pa).unwrap(),
        GossipConfig { bind_port: pa, ..Default::default() },
    ));
    let b = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", pb).unwrap(),
        GossipConfig {
            bind_port: pb,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", pa).unwrap()],
            ..Default::default()
        },
    ));
    a.start().await.unwrap();
    b.start().await.unwrap();
    assert!(until(Duration::from_secs(5), || !a.peers().is_empty() && !b.peers().is_empty()).await);

    let _stalled = b.mesh().signal_rx("bounds.busy"); // never read
    let mut reading = b.mesh().signal_rx("bounds.busy");
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    let drain = tokio::spawn(async move {
        while reading.recv().await.is_some() {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });

    const N: usize = 600;
    for i in 0..N {
        let _ = a.mesh().emit("bounds.busy", SignalScope::Cluster, Bytes::from(format!("{i}")));
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    until(Duration::from_secs(5), || counter.load(std::sync::atomic::Ordering::Relaxed) >= N - 20).await;
    let got = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        got >= N * 3 / 4,
        "the reading subscriber got {got} of {N}; a stalled subscriber of the same kind must not \
         veto admission",
    );
    assert!(b.system_stats().signal_handler_drops > 0, "the stalled subscriber's losses are counted");
    drain.abort();
    a.shutdown().await;
    b.shutdown().await;
}

/// Frames `msg` as `write_frame` would and trickles it onto `s` in `chunk`-byte pieces, `gap` apart.
async fn trickle(s: &mut TcpStream, msg: &WireMessage, chunk: usize, gap: Duration, stop_after: Option<usize>) {
    use tokio::io::AsyncWriteExt;
    let mut framed = Vec::new();
    write_frame(&mut framed, &wire_to_bytes(msg)).await.unwrap();
    for (i, piece) in framed.chunks(chunk).enumerate() {
        if stop_after.is_some_and(|n| i >= n) { return; }
        // A write error means the node closed the connection; the caller's assertion says why.
        if s.write_all(piece).await.is_err() || s.flush().await.is_err() { return; }
        tokio::time::sleep(gap).await;
    }
}

fn big_data(key: &str, len: usize) -> WireMessage {
    let origin = NodeId::new("127.0.0.1", 9).unwrap();
    WireMessage::Data(crate::framing::make_gossip_update(
        &origin, 1, Arc::from(key), Bytes::from(vec![5u8; len]), false, &crate::hlc::Hlc::new(),
    ))
}

/// The adversarial review of #602, finding 1: the first frame on a connection had to arrive *whole*
/// within `handshake_timeout_ms`, so a large frame (an anti-entropy chunk is up to ~10 MB) on a slow
/// but healthy link was cut mid-body. A frame that keeps making progress must be read however long
/// it takes as a whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_first_frame_trickling_steadily_is_read_past_the_handshake_bound() {
    let px = alloc_port();
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.handshake_timeout_ms = 400;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    // 256 KiB in 32 pieces 40 ms apart: ~1.3 s in all, never more than 40 ms without a byte.
    trickle(&mut s, &big_data("bounds/trickled", 256 * 1024), 8 * 1024 + 1, Duration::from_millis(40), None).await;
    assert!(
        until(Duration::from_secs(5), || x.kv().get("bounds/trickled").is_some()).await,
        "a frame making steady progress must be read even when it takes longer than handshake_timeout_ms",
    );
    x.shutdown().await;
}

/// The other half of finding 1: a frame that stops mid-body is still closed — by the progress bound,
/// and counted as a stalled frame, not as a socket that never spoke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_that_stalls_mid_body_is_closed_and_counted_apart() {
    let px = alloc_port();
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.peer_read_stall_timeout_ms = 300;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    trickle(&mut s, &big_data("bounds/stalled", 64 * 1024), 8 * 1024, Duration::from_millis(20), Some(3)).await;
    let mut buf = [0u8; 16];
    let r = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await;
    assert!(matches!(r, Ok(Ok(0)) | Ok(Err(_))), "a frame stalled mid-body must be closed; got {r:?}");
    assert_eq!(x.system_stats().inbound_frames_stalled, 1);
    assert_eq!(x.system_stats().inbound_connections_timed_out, 0, "it spoke; it is not silence");
    assert!(x.kv().get("bounds/stalled").is_none());
    x.shutdown().await;
}

/// The adversarial review of #602, findings 4 and 5: admission sheds a kind when every subscriber
/// that bears work is full — and a tap (an SSE observer) reading fast does not hide that. The tap is
/// also the witness: it receives exactly what admission let through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kind_whose_every_worker_is_full_sheds_while_a_tap_reads() {
    let px = alloc_port();
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), GossipConfig { bind_port: px, ..Default::default() });
    x.start().await.unwrap();
    let kind: Arc<str> = Arc::from("bounds.shed");
    let _worker = x.mesh().signal_rx_with_capacity(Arc::clone(&kind), 8); // never read
    let mut tap = x.task_ctx_for_tests().signal_handlers.register_tap(Arc::clone(&kind), 1024);
    const N: usize = 200;
    let mut seen = 0usize;
    for i in 0..N {
        let _ = x.mesh().emit("bounds.shed", SignalScope::Cluster, Bytes::from(format!("{i}")));
        tokio::time::sleep(Duration::from_millis(1)).await;
        while tap.try_recv().is_ok() { seen += 1; }
    }
    assert!(seen >= 8, "the worker's first eight were admitted; the tap saw {seen}");
    assert!(seen < N / 4, "with its only worker full the kind must shed; the tap saw {seen} of {N}");
    x.shutdown().await;
}

/// The measurement behind `peer_write_stall_timeout_ms`'s default (#602's re-review, finding 3): how long a
/// node takes to apply one worst-case anti-entropy chunk — ~10 MB, every entry appended to a WAL
/// that fsyncs each append (`sync_mode = "flush"`) — during which it does not read, so the sender's
/// writer sees no progress. Run by hand: `cargo test --lib worst_case_chunk_apply -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, not a check; its result is recorded in docs/reference/configuration.md"]
async fn worst_case_chunk_apply_with_an_fsync_wal() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    use crate::framing::SyncEntry;
    // (entries, value bytes, tombstones, sync mode). The last two are the worst a chunk can hold: the
    // sender's chunk budget counts `key + value + 64` per entry, so ~145 000 tombstones with short
    // keys fill one (#602's round 3).
    for (entries, size, tomb, mode) in [
        (152usize, 64 * 1024usize, false, SyncMode::Flush),
        (9_000, 1024, false, SyncMode::Flush),
        (70_000, 64, false, SyncMode::Flush),
        (145_000, 0, true, SyncMode::Flush),
        (145_000, 0, true, SyncMode::Async),
    ] {
        let px = alloc_port();
        let dir = std::env::temp_dir().join(format!("mycelium-chunk-apply-{px}"));
        let mut cfg = GossipConfig { bind_port: px, ..Default::default() };
        cfg.persistence = Some(PersistenceConfig {
            base_path: dir.clone(), sync_mode: mode,
            snapshot_wal_threshold: 1_000_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse,
        });
        let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cfg);
        x.start().await.unwrap();
        let hlc = crate::hlc::Hlc::new();
        let batch: Vec<SyncEntry> = (0..entries).map(|i| SyncEntry {
            key: Arc::from(format!("t{i}")), value: Bytes::from(vec![9u8; size]),
            timestamp: hlc.tick(), is_tombstone: tomb,
        }).collect();
        let frame = wire_to_bytes(&WireMessage::StateResponse { entries: batch });
        let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
        let started = std::time::Instant::now();
        write_frame(&mut s, &frame).await.unwrap();
        // The last entry is applied last and the WAL batch follows the store apply, so wait for the
        // node to read again: a probe frame on the same connection is answered only after the chunk.
        let last = format!("t{}", entries - 1);
        assert!(until(Duration::from_secs(1200), || x.kv_state_for_tests().store.pin().contains_key(last.as_str())).await);
        let applied = started.elapsed();
        // Wait for the WAL batch too: a ping round-trip on this connection means the loop is reading.
        write_frame(&mut s, &wire_to_bytes(&WireMessage::Ping { sender: NodeId::new("127.0.0.1", 9).unwrap(), known_peers: vec![] })).await.unwrap();
        assert!(until(Duration::from_secs(1200), || x.peers().contains(&NodeId::new("127.0.0.1", 9).unwrap())).await);
        println!("chunk of {entries} x {size} B{} ({} B framed), {mode:?}: store applied in {:?}, reading again (WAL batch done) in {:?}",
                 if tomb { " tombstones" } else { "" }, frame.len(), applied, started.elapsed());
        x.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// #602's re-review, finding 9: the one-byte-then-trickle attack on an agent. With the rate floor
/// set, a peer that starts a frame and then sends a byte per 100 ms — never silent long enough for
/// the no-progress bound — is closed by the floor and counted as a stalled frame. (With the floor
/// off, the default, the same trickle holds the socket; `configuration.md` states that cost.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_byte_trickle_is_closed_by_the_rate_floor() {
    use tokio::io::AsyncWriteExt;
    let px = alloc_port();
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.peer_read_stall_timeout_ms = 300;
    cx.peer_min_rate_bytes_per_sec = 10_000;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    let mut framed = Vec::new();
    write_frame(&mut framed, &wire_to_bytes(&big_data("bounds/trickle", 4096))).await.unwrap();
    let mut closed = false;
    for b in framed.iter().take(100) {
        if s.write_all(std::slice::from_ref(b)).await.is_err() || s.flush().await.is_err() { closed = true; break; }
        tokio::time::sleep(Duration::from_millis(100)).await; // never 300 ms silent; ~10 B/s
    }
    if !closed {
        let mut buf = [0u8; 8];
        closed = matches!(tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await, Ok(Ok(0)) | Ok(Err(_)));
    }
    assert!(closed, "a trickle below the floor must be closed");
    assert!(until(Duration::from_secs(2), || x.system_stats().inbound_frames_stalled >= 1).await);
    x.shutdown().await;
}

/// And with the floor off (the default), one byte then silence still closes — by the no-progress bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_byte_then_silence_is_closed_with_the_floor_off() {
    use tokio::io::AsyncWriteExt;
    let px = alloc_port();
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.peer_read_stall_timeout_ms = 300;
    cx.peer_min_rate_bytes_per_sec = 0;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
    s.write_all(&[0u8]).await.unwrap();
    let mut buf = [0u8; 8];
    let r = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await;
    assert!(matches!(r, Ok(Ok(0)) | Ok(Err(_))), "one byte then silence must close; got {r:?}");
    assert_eq!(x.system_stats().inbound_frames_stalled, 1);
    x.shutdown().await;
}

/// Starts a peer bootstrapped to `px` that writes `key` (re-writing until the caller sees it).
async fn healthy_peer(px: u16) -> GossipAgent {
    let py = alloc_port();
    let y = GossipAgent::new(
        NodeId::new("127.0.0.1", py).unwrap(),
        GossipConfig {
            bind_port: py,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", px).unwrap()],
            reconnect_backoff_secs: 1,
            ..Default::default()
        },
    );
    y.start().await.unwrap();
    y
}

/// Keeps writing `key` on `y` until `x` holds it, or `within` passes.
async fn syncs(x: &GossipAgent, y: &GossipAgent, key: &str, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let _ = y.kv().set(key, Bytes::from_static(b"ok"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        if x.kv().get(key).is_some() { return true; }
    }
    false
}

fn attack_config(px: u16) -> GossipConfig {
    let mut cx = GossipConfig { bind_port: px, ..Default::default() };
    cx.max_connections = 2;
    cx.handshake_timeout_ms = 300;
    cx.peer_min_rate_bytes_per_sec = 0; // preemption alone, not the floor
    cx
}

/// #602's round 3, finding 1, variant one: a trickle **inside** a frame — a byte every 200 ms, never
/// silent for the stall bound — held a permit for as long as it liked. At the cap a newcomer now
/// preempts the connection whose last complete frame is oldest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trickle_inside_a_frame_does_not_hold_the_permits() {
    use tokio::io::AsyncWriteExt;
    let px = alloc_port();
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), attack_config(px));
    x.start().await.unwrap();
    let mut attackers = Vec::new();
    for _ in 0..2 {
        let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
        attackers.push(tokio::spawn(async move {
            let mut framed = Vec::new();
            write_frame(&mut framed, &wire_to_bytes(&big_data("bounds/never", 64 * 1024))).await.unwrap();
            for b in framed {
                if s.write_all(&[b]).await.is_err() { return; }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let y = healthy_peer(px).await;
    assert!(syncs(&x, &y, "bounds/after-trickle", Duration::from_secs(10)).await,
            "a healthy peer must get a permit from connections that complete no frame");
    for a in attackers { a.abort(); }
    y.shutdown().await;
    x.shutdown().await;
}

/// Variant two: a tiny **complete** frame every idle period keeps a connection open with no floor
/// able to reach it (the floor acts only inside a frame).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tiny_complete_frames_do_not_hold_the_permits() {
    let px = alloc_port();
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), attack_config(px));
    x.start().await.unwrap();
    let mut attackers = Vec::new();
    for i in 0..2u16 {
        let mut s = TcpStream::connect(("127.0.0.1", px)).await.unwrap();
        let ping = wire_to_bytes(&WireMessage::Ping { sender: NodeId::new("127.0.0.2", 40_000 + i).unwrap(), known_peers: vec![] });
        attackers.push(tokio::spawn(async move {
            loop {
                if write_frame(&mut s, &ping).await.is_err() { return; }
                tokio::time::sleep(Duration::from_millis(700)).await;
            }
        }));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let y = healthy_peer(px).await;
    assert!(syncs(&x, &y, "bounds/after-pings", Duration::from_secs(10)).await,
            "a healthy peer must get a permit from connections that only keep themselves alive");
    assert!(x.system_stats().inbound_connections_preempted >= 1);
    for a in attackers { a.abort(); }
    y.shutdown().await;
    x.shutdown().await;
}

/// And the other side: a connect flood never preempts a healthy peer that is talking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connect_flood_never_preempts_a_talking_peer() {
    let px = alloc_port();
    let mut cx = attack_config(px);
    cx.max_connections = 3;
    let x = GossipAgent::new(NodeId::new("127.0.0.1", px).unwrap(), cx);
    x.start().await.unwrap();
    let y = healthy_peer(px).await;
    assert!(syncs(&x, &y, "bounds/flood/warm", Duration::from_secs(10)).await);
    let flood = tokio::spawn(async move {
        let mut held = Vec::new();
        for _ in 0..200 {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", px)).await { held.push(s); }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        held
    });
    for i in 0..30 {
        let key = format!("bounds/flood/{i}");
        assert!(y.kv().set(key.as_str(), Bytes::from_static(b"ok")));
        tokio::time::sleep(Duration::from_millis(100)).await; // a frame every 100 ms: never quiet
    }
    drop(flood.await);
    for i in 0..30 {
        let key = format!("bounds/flood/{i}");
        assert!(until(Duration::from_secs(5), || x.kv().get(&key).is_some()).await,
                "{key} was lost: the talking peer's connection must not be preempted");
    }
    y.shutdown().await;
    x.shutdown().await;
}
