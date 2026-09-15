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

// Pins the deprecated verb's behaviour on purpose: it still ships, so what it does still
// needs a test. `set_with_replica_sync` is the replacement and has its own gates.
#[allow(deprecated)]
#[tokio::test]
async fn set_with_min_acks_zero() {
    let a = make_agent();
    let r = a.kv().set_with_min_acks("sq-key", b"val".to_vec(), 0, Duration::from_secs(5)).await;
    assert_eq!(r, Ok(0));
    assert_eq!(a.kv().get("sq-key"), Some(Bytes::from_static(b"val")));
}

/// Item 1 PR 4b — **the flip**. This replaces
/// `a_peer_holding_the_write_still_produces_no_acknowledgement`, the pin PR 4a planted to record a
/// gap it measured but could not close: a peer that had received and applied the write produced no
/// acknowledgement, so the verb timed out in a healthy two-node cluster.
///
/// PR 4a's pin asserted the timeout **as the contract** precisely so this PR would have to come and
/// change it in the open rather than quietly make a documented impossibility possible. That is what
/// this test is. The gap is closed by asking the peer instead of watching the gossip stream, which
/// is the only thing that could close it — no predicate over inbound updates can work, because no
/// inbound frame carries the evidence (`docs/design/contracts-receipts.md` §1a).
#[tokio::test]
async fn a_peer_holding_the_write_acknowledges_it() {
    use crate::{PersistenceConfig, SyncMode};

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let base_a = std::env::temp_dir().join(format!("myc-rs-a-{port_a}"));
    let base_b = std::env::temp_dir().join(format!("myc-rs-b-{port_b}"));
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);

    let mk = |port: u16, id: NodeId, peer: NodeId, base: std::path::PathBuf| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![peer];
        cfg.health_check_max_jitter_ms = 50;
        // `Async` on purpose: the answer must come from the peer's own forced sync, not from the
        // node happening to run in Flush mode.
        cfg.persistence = Some(PersistenceConfig {
            base_path: base,
            sync_mode: SyncMode::Async,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600,
        });
        GossipAgent::new(id, cfg)
    };
    let a = mk(port_a, id_a.clone(), id_b.clone(), base_a.clone());
    let b = mk(port_b, id_b.clone(), id_a.clone(), base_b.clone());
    a.start().await.unwrap();
    b.start().await.unwrap();
    for _ in 0..300 {
        if !a.peers().is_empty() && !b.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!a.peers().is_empty() && !b.peers().is_empty(), "the pair must form a cluster");

    let receipt = a
        .set_with_replica_sync("rs/held", b"v".to_vec(), Duration::from_secs(10))
        .await
        .expect("the local write must succeed");

    assert!(
        receipt.replica_sync.persisted_by.contains(&id_b),
        "B must answer that it holds this operation on disk; persisted_by={:?} missing={:?}",
        receipt.replica_sync.persisted_by, receipt.replica_sync.missing,
    );
    assert!(
        !receipt.replica_sync.persisted_by.contains(&id_a),
        "the origin is never counted as a replica",
    );
    assert!(receipt.replica_sync.missing.is_empty(), "every peer answered");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
}

/// Item 1 PR 4b — the Phase B exit gate: **a peer that acknowledged persistence holds the record
/// across its own crash and restart** (plan §2, Phase B).
///
/// The acknowledgement is only worth having if it survives the event it is insurance against. B
/// answers `persisted`, B is then stopped and started again from the same directory, and the same
/// question is asked: it must still answer yes, from the replayed WAL, without the value ever being
/// re-gossiped to it. A weaker mechanism — one that answered from memory, or from "I saw that
/// update once" — would pass the first assertion and fail this one.
#[tokio::test]
async fn an_acknowledged_replica_still_holds_the_record_after_restart() {
    use crate::{PersistenceConfig, SyncMode};

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let base_a = std::env::temp_dir().join(format!("myc-rsr-a-{port_a}"));
    let base_b = std::env::temp_dir().join(format!("myc-rsr-b-{port_b}"));
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);

    let mk = |port: u16, id: NodeId, peer: NodeId, base: std::path::PathBuf| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![peer];
        cfg.health_check_max_jitter_ms = 50;
        cfg.persistence = Some(PersistenceConfig {
            base_path: base,
            sync_mode: SyncMode::Async,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600,
        });
        GossipAgent::new(id, cfg)
    };

    let a = mk(port_a, id_a.clone(), id_b.clone(), base_a.clone());
    let b1 = mk(port_b, id_b.clone(), id_a.clone(), base_b.clone());
    a.start().await.unwrap();
    b1.start().await.unwrap();
    for _ in 0..300 {
        if !a.peers().is_empty() && !b1.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let first = a
        .set_with_replica_sync("rs/survives", b"v".to_vec(), Duration::from_secs(10))
        .await
        .expect("local write");
    assert!(
        first.replica_sync.persisted_by.contains(&id_b),
        "B must acknowledge before the restart means anything; missing={:?}",
        first.replica_sync.missing,
    );

    // B crashes and comes back on the same directory. Nothing re-gossips the value to it: A does
    // not rewrite the key, so the only way B can answer yes again is its own replayed WAL.
    b1.shutdown().await;
    drop(b1);
    let b2 = mk(port_b, id_b.clone(), id_a.clone(), base_b.clone());
    b2.start().await.unwrap();
    for _ in 0..300 {
        if !a.peers().is_empty() && !b2.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        b2.kv().get("rs/survives"), Some(Bytes::from_static(b"v")),
        "the restarted peer must have replayed the record from its own WAL",
    );

    // Isolate the gate's actual claim from transport. What must survive the restart is B's ability
    // to answer truthfully about its own disk; whether A can currently reach B is a different
    // property, and conflating them would make this gate fail for reasons it is not about.
    let q = crate::agent::replica_sync::Query {
        stamp: first.stamp,
        content_hash: first.content_hash,
        key: std::sync::Arc::from("rs/survives"),
    };
    let local = crate::agent::replica_sync::answer(&b2.service().ctx, &q).await;
    assert_eq!(
        local, crate::agent::replica_sync::Answer::Persisted,
        "the restarted peer must answer `persisted` about its own replayed record",
    );

    // 30 s, and the number is not a tolerance for slowness. A restarted peer keeps the same
    // NodeId, so the origin still holds a writer entry for the connection that died and must work
    // through reconnect backoff before any RPC reaches it. The deadline has to exceed that
    // reconnection budget or the answer cannot arrive at all — the distinction drawn in
    // testing.md §"A client deadline below the server's budget is a defect, not a flake". At 10 s
    // this failed for exactly that reason while the assertion above already passed.
    let after = crate::agent::replica_sync::collect(
        &a.service().ctx,
        q,
        Duration::from_secs(30),
    )
    .await;
    assert!(
        after.persisted_by.contains(&id_b),
        "an acknowledged replica must still hold the record after its own restart; \
         persisted_by={:?} missing={:?}",
        after.persisted_by, after.missing,
    );

    a.shutdown().await;
    b2.shutdown().await;
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
}

// Pins the deprecated verb's behaviour on purpose: it still ships, so what it does still
// needs a test. `set_with_replica_sync` is the replacement and has its own gates.
#[allow(deprecated)]
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

