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
