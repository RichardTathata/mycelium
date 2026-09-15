//! `GossipAgent`-driven tests for [`KvHandle`](mycelium_core::KvHandle) and the
//! [`KvQuorumExt`](crate::KvQuorumExt) overlay.
//!
//! The handle itself lives in `mycelium-core` (v2 M3); these exercise it through a
//! live `GossipAgent`, and the `set_with_min_acks` cases additionally cover the
//! upper-crate quorum-durability extension.

use crate::{GossipAgent, GossipConfig, KvQuorumExt, NodeId};
use bytes::Bytes;
use std::{sync::Arc, time::Duration};

fn make_agent() -> GossipAgent {
    GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), GossipConfig::default())
}

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

async fn make_started_agent(port: u16) -> GossipAgent {
    let id  = NodeId::new("127.0.0.1", port).unwrap();
    let cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: port, ..GossipConfig::default() };
    let a   = GossipAgent::new(id, cfg);
    a.start().await.unwrap();
    a
}

// ── Basic KV ─────────────────────────────────────────────────────────────

#[test]
fn set_get() {
    let a = make_agent();
    let _ = a.kv().set("hello", b"world".to_vec());
    assert_eq!(a.kv().get("hello"), Some(Bytes::from_static(b"world")));
}

#[test]
fn set_returns_true_when_channel_has_capacity() {
    assert!(make_agent().kv().set("k", b"v".to_vec()));
}

#[test]
fn delete_local() {
    let a = make_agent();
    let _ = a.kv().set("key", b"val".to_vec());
    let _ = a.kv().delete("key");
    assert_eq!(a.kv().get("key"), None);
}

#[test]
fn keys_returns_live_keys_only() {
    let a = make_agent();
    let _ = a.kv().set("a", b"1".to_vec());
    let _ = a.kv().set("b", b"2".to_vec());
    let _ = a.kv().set("c", b"3".to_vec());
    let _ = a.kv().delete("b");
    let mut keys = a.kv().keys();
    keys.sort();
    assert_eq!(keys, vec![Arc::from("a"), Arc::from("c")]);
}

#[test]
fn keys_empty_on_new_agent() {
    assert!(make_agent().kv().keys().is_empty());
}

#[test]
fn scan_prefix_returns_matching_live_entries() {
    let a = make_agent();
    let _ = a.kv().set("load/node-a", b"state-a".to_vec());
    let _ = a.kv().set("load/node-b", b"state-b".to_vec());
    let _ = a.kv().set("other/key",   b"other".to_vec());
    let mut entries = a.kv().scan_prefix("load/");
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    assert_eq!(entries.len(), 2);
    assert_eq!(&*entries[0].0, "load/node-a");
    assert_eq!(entries[0].1, Bytes::from_static(b"state-a"));
    assert_eq!(&*entries[1].0, "load/node-b");
    assert_eq!(entries[1].1, Bytes::from_static(b"state-b"));
}

#[test]
fn scan_prefix_excludes_tombstones() {
    let a = make_agent();
    let _ = a.kv().set("load/node-a", b"alive".to_vec());
    let _ = a.kv().set("load/node-b", b"alive".to_vec());
    let _ = a.kv().delete("load/node-a");
    let entries = a.kv().scan_prefix("load/");
    assert_eq!(entries.len(), 1);
    assert_eq!(&*entries[0].0, "load/node-b");
}

#[test]
fn scan_prefix_no_match_returns_empty() {
    let a = make_agent();
    let _ = a.kv().set("load/node-a", b"x".to_vec());
    assert_eq!(a.kv().scan_prefix("grp/").len(), 0);
}

#[tokio::test]
async fn set_async_stores_and_queues() {
    let a = make_agent();
    assert!(a.kv().set_async("k", b"v".to_vec()).await);
    assert_eq!(a.kv().get("k"), Some(Bytes::from_static(b"v")));
}

#[tokio::test]
async fn delete_async_tombstones_key() {
    let a = make_agent();
    assert!(a.kv().set_async("k", b"v".to_vec()).await);
    assert!(a.kv().delete_async("k").await);
    assert_eq!(a.kv().get("k"), None);
}

#[tokio::test]
async fn subscribe_initial_value_absent() {
    let rx = make_agent().kv().subscribe("missing");
    assert_eq!(*rx.borrow(), None);
}

#[tokio::test]
async fn subscribe_initial_value_present() {
    let a = make_agent();
    let _ = a.kv().set("k", b"hello".to_vec());
    let rx = a.kv().subscribe("k");
    assert_eq!(*rx.borrow(), Some(Bytes::from_static(b"hello")));
}

#[tokio::test]
async fn subscribe_notified_on_set() {
    let a = make_agent();
    let mut rx = a.kv().subscribe("k");
    rx.borrow_and_update();
    let _ = a.kv().set("k", b"world".to_vec());
    tokio::time::timeout(std::time::Duration::from_millis(100), rx.changed())
        .await.expect("should fire within 100 ms").unwrap();
    assert_eq!(*rx.borrow(), Some(Bytes::from_static(b"world")));
}

#[tokio::test]
async fn subscribe_notified_on_delete() {
    let a = make_agent();
    let _ = a.kv().set("k", b"v".to_vec());
    let mut rx = a.kv().subscribe("k");
    rx.borrow_and_update();
    let _ = a.kv().delete("k");
    tokio::time::timeout(std::time::Duration::from_millis(100), rx.changed())
        .await.expect("should fire within 100 ms").unwrap();
    assert_eq!(*rx.borrow(), None);
}

#[tokio::test]
async fn subscribe_prefix_with_predicate_skips_non_matching_keys() {
    let a = make_agent();
    let mut rx = a.kv().subscribe_prefix_with_predicate(
        Arc::<str>::from("cap/"),
        |k: &str| k.ends_with("/compute/gpu"),
    );
    let mark = *rx.borrow();
    let _ = a.kv().set("cap/127.0.0.1:1/storage/disk", b"x".to_vec());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(*rx.borrow(), mark, "predicate must suppress non-matching keys");
    let _ = a.kv().set("cap/127.0.0.1:1/compute/gpu", b"y".to_vec());
    tokio::time::timeout(std::time::Duration::from_millis(100), rx.changed())
        .await.expect("predicate-matching write should fire within 100 ms").unwrap();
    assert_ne!(*rx.borrow(), mark);
}

#[test]
fn gossip_channel_capacity_respected() {
    let mut cfg = GossipConfig::default();
    cfg.gossip_channel_capacity = 1;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), cfg);
    assert!( a.kv().set("k1", b"v1".to_vec()), "first send fits");
    assert!(!a.kv().set("k1", b"v2".to_vec()), "second send to same shard fails");
}

// ── set_with_min_acks (KvQuorumExt) ───────────────────────────────────────

#[tokio::test]
async fn set_with_min_acks_zero() {
    let a = make_agent();
    let r = a.kv().set_with_min_acks("sq-key", b"val".to_vec(), 0, Duration::from_secs(5)).await;
    assert_eq!(r, Ok(0));
    assert_eq!(a.kv().get("sq-key"), Some(Bytes::from_static(b"val")));
}

/// The regression floor for item 1 PR 4b (contracts axis, `docs/design/contracts-receipts.md` §8).
///
/// Pins the structural gap PR 4a **measured but could not close**: a peer that has received and
/// applied the write produces no acknowledgement, so `set_with_min_acks` times out in a healthy
/// two-node cluster. Three facts compose into it — a `GossipUpdate`'s `sender` is its *originating*
/// node and survives every hop; fan-out excludes the origin, so the relayed copy never returns to
/// us; and anti-entropy re-attributes delivered entries to the receiving node. No inbound frame
/// says "a peer holds your write".
///
/// This is deliberately asserted as a **failure**, not skipped: the verb's documentation now states
/// that it cannot succeed, and a claim like that has to be executable. PR 4b's persisted-by-peer
/// protocol flips this test, in the open, which is what the floor is for.
#[tokio::test]
async fn a_peer_holding_the_write_still_produces_no_acknowledgement() {
    use crate::agent::kv_quorum::QuorumError;
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port = port_a;
    cfg_a.bootstrap_peers = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.bootstrap_peers = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;
    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    // Structural poll, never a fixed sleep.
    for _ in 0..300 {
        if !a.peers().is_empty() && !b.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!a.peers().is_empty() && !b.peers().is_empty(), "the pair must form a cluster");

    let r = a.kv().set_with_min_acks("mk/held", b"v".to_vec(), 1, Duration::from_secs(2)).await;

    // The peer really does hold it — that is the whole point of the pin.
    for _ in 0..300 {
        if b.kv().get("mk/held").is_some() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        b.kv().get("mk/held"), Some(Bytes::from_static(b"v")),
        "the write must have reached the peer",
    );
    match r {
        Err(QuorumError::Timeout { acks_received }) => assert_eq!(
            acks_received, 0,
            "no inbound frame can attribute our own write's propagation to a peer",
        ),
        Ok(n) => panic!(
            "PR 4b appears to have landed: {n} ack(s) while this pin still asserts none. \
             Update the pin and the `set_with_min_acks` documentation together."
        ),
    }
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn set_with_min_acks_timeout_no_peers() {
    use crate::agent::kv_quorum::QuorumError;
    let a = make_agent();
    let r = a.kv().set_with_min_acks("sq-key2", b"val".to_vec(), 1, Duration::from_millis(50)).await;
    match r {
        Err(QuorumError::Timeout { acks_received }) => assert_eq!(acks_received, 0),
        Ok(n) => panic!("expected Timeout, got Ok({n})"),
    }
}

// ── Log overlay ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_append_scan_compact() {
    let a = make_started_agent(alloc_port()).await;

    let h1 = a.kv().append("events", Bytes::from_static(b"e1"));
    let h2 = a.kv().append("events", Bytes::from_static(b"e2"));
    let _h3 = a.kv().append("events", Bytes::from_static(b"e3"));
    let h4 = a.kv().append("events", Bytes::from_static(b"e4"));
    let h5 = a.kv().append("events", Bytes::from_static(b"e5"));

    let all = a.kv().scan_log("events", 0, u64::MAX);
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].value, Bytes::from_static(b"e1"));
    assert_eq!(all[4].value, Bytes::from_static(b"e5"));

    let mid = a.kv().scan_log("events", h2, h4);
    assert_eq!(mid.len(), 2);

    a.kv().compact_log("events", h4);
    let after = a.kv().scan_log("events", 0, u64::MAX);
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|e| e.hlc >= h4));

    let _ = (h1, h5);
    a.shutdown().await;
}

#[tokio::test]
async fn test_subscribe_log_receives_live_append() {
    let a  = make_started_agent(alloc_port()).await;
    let mut rx = a.kv().subscribe_log("live", 0);

    a.kv().append("live", Bytes::from_static(b"msg1"));

    let entry = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        rx.recv(),
    ).await.expect("timeout").expect("channel closed");

    assert_eq!(entry.value, Bytes::from_static(b"msg1"));
    a.shutdown().await;
}

