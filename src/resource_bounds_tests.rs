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
    cx.peer_stall_timeout_ms = 300;
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
