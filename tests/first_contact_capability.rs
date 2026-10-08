//! Issue #563: a capability advertised right after `start()` must reach a peer that starts at the same time well
//! before the advertiser's first refresh — the advertisement is a write, and a connected peer gets writes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mycelium::{CapFilter, Capability, GossipAgent, GossipConfig, NodeId};

/// Below the OS ephemeral range and bind-verified, like `mycelium::test_util::alloc_port` (which needs `test-util`, a
/// feature this target's CI step does not enable): a port from `bind(0)` can be taken as an outbound source port
/// before the agent binds it (src/test_util.rs, Run 27).
fn alloc_port() -> u16 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = std::process::id().wrapping_mul(2_654_435_761) % 16_000 + 1;
    let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    loop {
        let p = (16_000 + NEXT.fetch_add(1, Ordering::Relaxed) % 16_000) as u16;
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
}

/// `before_b`: advertise on A before B starts — the write predates the connection, as in CI where A's process starts
/// first and its blob server advertises right after `start()` (#563's recipe); otherwise after both have started.
async fn once(before_b: bool) -> Duration {
    let (pa, pb) = (alloc_port(), alloc_port());
    let a_id = NodeId::new("127.0.0.1", pa).unwrap();
    let mut ca = GossipConfig::default();
    ca.bind_port = pa;
    let mut cb = GossipConfig::default();
    cb.bind_port = pb;
    cb.bootstrap_peers = vec![a_id.clone()];
    let a = Arc::new(GossipAgent::new(a_id, ca));
    let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", pb).unwrap(), cb));
    let t0 = Instant::now();
    let _reg;
    if before_b {
        a.start().await.unwrap();
        _reg = a.capabilities().advertise_capability(Capability::new("probe", "cap"), Duration::from_secs(30));
        b.start().await.unwrap();
    } else {
        let (ra, rb) = tokio::join!(a.start(), b.start());
        ra.unwrap();
        rb.unwrap();
        _reg = a.capabilities().advertise_capability(Capability::new("probe", "cap"), Duration::from_secs(30));
    }
    let filter = CapFilter::new("probe", "cap");
    let seen = loop {
        if !b.capabilities().resolve(&filter).is_empty() {
            break t0.elapsed();
        }
        assert!(t0.elapsed() < Duration::from_secs(45), "B never resolved A's capability");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    a.shutdown().await;
    b.shutdown().await;
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capability_advertised_at_start_reaches_a_peer_starting_with_it() {
    let mut worst = Duration::ZERO;
    for i in 0..10 {
        let before_b = i % 2 == 0;
        let d = once(before_b).await;
        eprintln!("run {i} (advertised {}): B resolved A's capability after {d:?}", if before_b { "before B started" } else { "after both started" });
        worst = worst.max(d);
    }
    // Locally 0.9–4.7 s; the bound is well under one capability refresh (30 s), which is what a write lost at first
    // contact would wait for (#563).
    assert!(worst < Duration::from_secs(15), "worst first sight {worst:?} — a write lost at first contact waits for repair");
}
