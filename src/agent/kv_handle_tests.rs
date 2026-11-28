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
    let mut cfg = GossipConfig::default();
    cfg.bind_address = "127.0.0.1".parse().unwrap();
    cfg.bind_port = port;
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
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
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
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
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

// ── The live log subscription (engineering review ER8, R8-01 and R8-09) ─────
//
// A sibling key is written directly — `log/{stream}/{hlc:016x}/{node}`, as a peer's append arrives — so
// each case is deterministic on one node. A sentinel written after the late entry is the progress
// marker: the next entry received must be the late one, not the sentinel.

fn put_log(a: &GossipAgent, stream: &str, hlc: u64, node: &str, value: &str) {
    assert!(a.kv().set(format!("log/{stream}/{hlc:016x}/{node}"), value.as_bytes().to_vec()));
}

async fn next_value(rx: &mut tokio::sync::mpsc::Receiver<mycelium_core::kv_handle::LogEntry>) -> (u64, String) {
    let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await
        .expect("an entry within 5 s").expect("the subscription is open");
    (e.hlc, String::from_utf8(e.value.to_vec()).unwrap())
}

/// The prefix watcher's receiver count for `log/{stream}/` — 0 once every subscription task has ended
/// (or no sender is registered).
fn log_watchers(a: &GossipAgent, stream: &str) -> usize {
    a.kv().core().kv_state.prefix_watchers.pin()
        .get(format!("log/{stream}/").as_str())
        .map_or(0, |tx| tx.receiver_count())
}

async fn until_no_log_watchers(a: &GossipAgent, stream: &str, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while log_watchers(a, stream) > 0 {
        if tokio::time::Instant::now() >= deadline { return false; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

/// R8-01: the subscription advanced to `hlc + 1` after each entry, so a sibling at the same stamp that
/// replicated after the first scan was below the next scan's floor and never sent.
#[tokio::test]
async fn a_live_subscription_delivers_a_same_stamp_sibling_that_arrives_later() {
    let a = make_started_agent(alloc_port()).await;
    put_log(&a, "er8.sib", 0x100, "aa", "first");
    let mut rx = a.kv().subscribe_log("er8.sib", 0);
    assert_eq!(next_value(&mut rx).await, (0x100, "first".into()));
    put_log(&a, "er8.sib", 0x100, "zz", "second");
    put_log(&a, "er8.sib", 0x101, "aa", "sentinel");
    assert_eq!(next_value(&mut rx).await, (0x100, "second".into()), "the same-stamp sibling, before the sentinel");
    assert_eq!(next_value(&mut rx).await, (0x101, "sentinel".into()));
    a.shutdown().await;
}

/// R8-01, the likelier case: an entry stamped below the last one delivered — a lagging writer, a healed
/// partition — arrives late. It is delivered when it arrives, so a subscriber may see a lower stamp.
#[tokio::test]
async fn a_live_subscription_delivers_a_late_append_with_a_lower_stamp() {
    let a = make_started_agent(alloc_port()).await;
    put_log(&a, "er8.low", 0x200, "aa", "first");
    let mut rx = a.kv().subscribe_log("er8.low", 0);
    assert_eq!(next_value(&mut rx).await, (0x200, "first".into()));
    put_log(&a, "er8.low", 0x1ff, "bb", "late");
    put_log(&a, "er8.low", 0x201, "aa", "sentinel");
    assert_eq!(next_value(&mut rx).await, (0x1ff, "late".into()), "the late, lower-stamped entry");
    assert_eq!(next_value(&mut rx).await, (0x201, "sentinel".into()));
    a.shutdown().await;
}

/// Control: every rescan (one per write under the prefix) sends only what was not yet sent.
#[tokio::test]
async fn each_entry_is_delivered_once_across_rescans() {
    let a = make_started_agent(alloc_port()).await;
    for (h, v) in [(0x100, "a"), (0x101, "b"), (0x102, "c")] { put_log(&a, "er8.once", h, "n", v); }
    let mut rx = a.kv().subscribe_log("er8.once", 0);
    for (h, v) in [(0x100, "a"), (0x101, "b"), (0x102, "c")] { assert_eq!(next_value(&mut rx).await, (h, v.into())); }
    put_log(&a, "er8.once", 0x103, "n", "d");
    assert_eq!(next_value(&mut rx).await, (0x103, "d".into()));
    // A write under the prefix that is not an entry (an unparsable key) rescans and sends nothing.
    assert!(a.kv().set("log/er8.once/not-an-entry", b"x".to_vec()));
    put_log(&a, "er8.once", 0x104, "n", "e");
    assert_eq!(next_value(&mut rx).await, (0x104, "e".into()), "no repeat of a..d");
    a.shutdown().await;
}

/// Control: `since` is a floor — an entry below it is never sent, even when it arrives late.
#[tokio::test]
async fn since_is_a_floor() {
    let a = make_started_agent(alloc_port()).await;
    for (h, v) in [(0x100, "a"), (0x200, "b"), (0x300, "c")] { put_log(&a, "er8.floor", h, "n", v); }
    let mut rx = a.kv().subscribe_log("er8.floor", 0x200);
    assert_eq!(next_value(&mut rx).await, (0x200, "b".into()), "inclusive");
    assert_eq!(next_value(&mut rx).await, (0x300, "c".into()));
    put_log(&a, "er8.floor", 0x1ff, "late", "below");
    put_log(&a, "er8.floor", 0x400, "n", "d");
    assert_eq!(next_value(&mut rx).await, (0x400, "d".into()), "the late entry below the floor is not sent");
    a.shutdown().await;
}

/// The core twin of the gateway's crafted-stamp test: a `log/{stream}/ffffffffffffffff/x` key (any member can
/// write one). Before ER8 the core never overflowed on it — its scan was `scan_log(.., last_seen, u64::MAX)`,
/// whose exclusive upper bound left the entry out — so it was never delivered (seen failing first: no entry
/// within 5 s). It is an entry like any other now, delivered once, and the stream goes on without a repeat.
#[tokio::test]
async fn a_crafted_max_stamp_entry_does_not_end_or_reset_a_live_subscription() {
    let a = make_started_agent(alloc_port()).await;
    assert!(a.kv().set("log/er8.max/ffffffffffffffff/x", b"crafted".to_vec()));
    let mut rx = a.kv().subscribe_log("er8.max", 0);
    assert_eq!(next_value(&mut rx).await, (u64::MAX, "crafted".into()));
    put_log(&a, "er8.max", 0x100, "n", "after");
    assert_eq!(next_value(&mut rx).await, (0x100, "after".into()), "the subscription goes on");
    put_log(&a, "er8.max", 0x101, "n", "after-2");
    assert_eq!(next_value(&mut rx).await, (0x101, "after-2".into()), "and does not re-send what it sent");
    a.shutdown().await;
}

/// R8-09: the task awaited only the prefix watcher, so a dropped receiver was noticed by the next failed
/// send — on a quiet stream, never — and its watch receiver stayed registered. It now ends on the drop.
#[tokio::test]
async fn dropping_an_idle_log_receiver_ends_its_task() {
    let a = make_started_agent(alloc_port()).await;
    let mut rx = a.kv().subscribe_log("er8.idle", 0);
    a.kv().append("er8.idle", Bytes::from_static(b"ready"));
    assert_eq!(next_value(&mut rx).await.1, "ready");
    assert_eq!(log_watchers(&a, "er8.idle"), 1, "the task holds its watch receiver");
    drop(rx);
    assert!(until_no_log_watchers(&a, "er8.idle", Duration::from_secs(3)).await,
        "the task must end with no further append (receivers left: {})", log_watchers(&a, "er8.idle"));
    a.shutdown().await;
}

/// R8-01 across two nodes, as the reviewer reproduced it: two nodes append in one millisecond and take
/// one stamp; a running subscriber on the first has delivered its own row before the second's arrives.
/// It must deliver the second, then a later sentinel.
#[tokio::test]
async fn a_running_log_subscription_receives_a_same_stamp_append_from_a_peer() {
    let (p1, p2) = (alloc_port(), alloc_port());
    let id1 = NodeId::new("127.0.0.1", p1).unwrap();
    let id2 = NodeId::new("127.0.0.1", p2).unwrap();
    let mut c1 = GossipConfig::default();
    c1.bind_address = "127.0.0.1".parse().unwrap();
    c1.bind_port = p1;
    let mut c2 = GossipConfig::default();
    c2.bind_address = "127.0.0.1".parse().unwrap();
    c2.bind_port = p2;
    c2.bootstrap_peers = vec![id1.clone()];
    let first  = GossipAgent::new(id1, c1);
    let second = GossipAgent::new(id2, c2);
    // Sample a pair of appends that took one stamp, before the nodes connect.
    let mut sample = None;
    for i in 0..64 {
        let stream = format!("er8.peer-{i}");
        let a = first.kv().append(&stream, Bytes::from_static(b"first"));
        let b = second.kv().append(&stream, Bytes::from_static(b"second"));
        if a == b { sample = Some((stream, a)); break; }
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let (stream, stamp) = sample.expect("two nodes appending in one millisecond take one stamp");
    let mut live = first.kv().subscribe_log(&stream, stamp);
    assert_eq!(next_value(&mut live).await, (stamp, "first".into()));
    first.start().await.unwrap();
    second.start().await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while first.kv().scan_log(&stream, 0, u64::MAX).len() < 2 {
        assert!(tokio::time::Instant::now() < deadline, "the second node's append never replicated");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let sentinel = first.kv().append(&stream, Bytes::from_static(b"sentinel"));
    assert!(sentinel > stamp);
    assert_eq!(next_value(&mut live).await, (stamp, "second".into()), "the peer's same-stamp append");
    assert_eq!(next_value(&mut live).await, (sentinel, "sentinel".into()));
    first.shutdown().await;
    second.shutdown().await;
}


/// **The replica gate, through a restart that keeps only the WAL** (external review 2026-10-31, R2). The gate above
/// restarts B through `shutdown()`, whose exit snapshot is taken from the store — so it proved the exit-snapshot path.
/// Here B's exit snapshot is made to fail (`test_util::sabotage_exit_snapshot`), the start snapshot is put back, and
/// B's restart replays start snapshot + WAL tail: the acknowledged record is back only if the inbound path appended it.
#[tokio::test]
async fn an_acknowledged_replica_still_holds_the_record_after_a_restart_that_keeps_only_the_wal() {
    // `Async` is the gate's own mode: the inbound append is a try-send there, which is the path to prove.
    wal_only_replica_restart(crate::SyncMode::Async).await;
}
#[tokio::test]
async fn an_acknowledged_replica_still_holds_the_record_after_a_restart_that_keeps_only_the_wal_under_flush() {
    wal_only_replica_restart(crate::SyncMode::Flush).await;
}
async fn wal_only_replica_restart(sync_mode: crate::SyncMode) {
    use crate::test_util::{kv_dir_under, restore_start_snapshot, sabotage_exit_snapshot};
    use crate::PersistenceConfig;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let base_a = std::env::temp_dir().join(format!("myc-rsw-a-{port_a}"));
    let base_b = std::env::temp_dir().join(format!("myc-rsw-b-{port_b}"));
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
    let mk = |port: u16, id: NodeId, peer: NodeId, base: std::path::PathBuf| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![peer];
        cfg.health_check_max_jitter_ms = 50;
        cfg.persistence = Some(PersistenceConfig {
            base_path: base,
            sync_mode,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
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
    let first = a.set_with_replica_sync("rs/wal-only", b"v".to_vec(), Duration::from_secs(10)).await.expect("local write");
    assert!(first.replica_sync.persisted_by.contains(&id_b), "B must acknowledge first; missing={:?}", first.replica_sync.missing);

    let kv_dir = kv_dir_under(&base_b);
    sabotage_exit_snapshot(&kv_dir);
    b1.shutdown().await;
    drop(b1);
    restore_start_snapshot(&kv_dir);
    // The record is in B's WAL as bytes, or it is nowhere B's restart can find it.
    let wal = std::fs::read(kv_dir.join("wal.bin")).unwrap();
    assert!(wal.windows(b"rs/wal-only".len()).any(|w| w == b"rs/wal-only"), "the inbound path appended the acknowledged record to B's WAL");
    // A is gone before B comes back, so nothing can re-gossip or anti-entropy the record to B: what B holds, it replayed.
    a.shutdown().await;
    drop(a);
    let b2 = mk(port_b, id_b.clone(), id_a.clone(), base_b.clone());
    b2.start().await.unwrap();
    assert_eq!(
        b2.kv().get("rs/wal-only"), Some(Bytes::from_static(b"v")),
        "the restarted peer must have replayed the acknowledged record from its own WAL, with no exit snapshot and no peer to carry it",
    );
    b2.shutdown().await;
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
}

/// **A replica-sync answer covers the apply-to-append window** (external review 2026-10-31, F3; row R8). The receive
/// path applies an inbound record to the store and then hands it to the WAL. B is paused between the two — the
/// interleaving the review's probe drove — and asked about the record: it holds the value, has dropped no append and its
/// WAL syncs, so before R8 it answered `Persisted` for a record a crash at that moment would lose. It must answer
/// `Failed` until the append has been made, then `Persisted`.
#[tokio::test]
async fn a_record_applied_but_not_appended_is_not_reported_persisted() {
    apply_to_append_window(crate::SyncMode::Async).await;
}
#[tokio::test]
async fn a_record_applied_but_not_appended_is_not_reported_persisted_under_flush() {
    apply_to_append_window(crate::SyncMode::Flush).await;
}
async fn apply_to_append_window(sync_mode: crate::SyncMode) {
    use crate::agent::replica_sync::{answer, Answer, Query};
    use crate::PersistenceConfig;
    use mycelium_core::receipt::{AttemptId, OperationId};

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let base_a = std::env::temp_dir().join(format!("myc-r8-a-{port_a}"));
    let base_b = std::env::temp_dir().join(format!("myc-r8-b-{port_b}"));
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
    let mk = |port: u16, id: NodeId, peer: NodeId, base: std::path::PathBuf| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![peer];
        cfg.health_check_max_jitter_ms = 50;
        cfg.persistence = Some(PersistenceConfig {
            base_path: base,
            sync_mode,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
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

    let key = "r8/window";
    let pause = b.service().ctx.wal.get().expect("B has a WAL").pause_inbound_after_apply(key);

    let a_ctx = Arc::clone(&a.service().ctx);
    let op = OperationId::generate(a.node_id());
    let attempt = AttemptId::fresh(&op);
    let receipt = mycelium_core::ops::kv_set_with_receipt(
        &a_ctx, op, attempt, Arc::from(key), Bytes::from_static(b"v"), None,
    ).await.expect("local write");
    tokio::time::timeout(Duration::from_secs(10), pause.reached())
        .await
        .expect("B applied the record and reached the pause before its append");

    let q = Query { stamp: receipt.stamp, content_hash: receipt.content_hash, key: Arc::from(key) };
    let b_ctx = Arc::clone(&b.service().ctx);
    assert_eq!(
        b.kv().get(key), Some(Bytes::from_static(b"v")),
        "the pause sits after the apply: B's store holds the value",
    );
    assert_eq!(
        answer(&b_ctx, &q).await, Answer::Failed,
        "a record applied but not yet handed to the WAL must not be reported persisted",
    );
    // Not asserted through the origin's `collect` here: A's query would reach B over the very
    // connection whose receive task is paused, so B could not answer whatever its state, and the
    // assertion held with the fix removed (R8's review round 1). The receipt is built only from
    // `answer()` — `persisted_by` names exactly the peers whose `answer()` returned `Persisted`
    // (`collect`'s doc) — so the direct call above is the receipt's claim.

    pause.release();
    let mut last = Answer::Failed;
    for _ in 0..200 {
        last = answer(&b_ctx, &q).await;
        if last == Answer::Persisted { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(last, Answer::Persisted, "once the append has been made the answer is `Persisted`");
    // Not `inbound_in_flight() == 0` here: B keeps receiving the fleet's own records (heartbeats,
    // advertisements), so the count is non-zero again whenever one is in flight — under `flush` each
    // waits on an fsync. `Persisted` above already read it as zero.
    assert_eq!(b_ctx.wal.get().unwrap().dropped_appends(), 0, "a completed append is not a dropped one");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base_a);
    let _ = std::fs::remove_dir_all(&base_b);
}

/// **The answer reads the in-flight count before the dropped-append count** (R8's review round 1). An inbound
/// record's window that closes without its append counts a dropped append and *then* releases the in-flight count. An
/// answer that loaded the dropped count first could see none, then — the window closing in between — see nothing in
/// flight, and answer `Persisted` for the record that was dropped. B holds a record whose window is open (applied, not
/// appended); the answer is paused between its two loads; the window is abandoned in that gap.
#[tokio::test]
async fn an_append_dropped_between_the_answers_two_reads_is_not_reported_persisted() {
    use crate::agent::replica_sync::{answer, Answer, Query};
    use crate::PersistenceConfig;
    use mycelium_core::receipt::content_hash;

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-r8-order-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.persistence = Some(PersistenceConfig {
        base_path: base.clone(),
        sync_mode: crate::SyncMode::Async,
        snapshot_wal_threshold: 1_000_000,
        snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
    });
    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    let ctx = Arc::clone(&b.service().ctx);
    let wal = Arc::clone(ctx.wal.get().expect("a WAL"));

    // A peer's record, applied as the receive path applies it: the window opened first.
    let peer = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let key = "r8/order";
    let window = wal.begin_inbound();
    let upd = mycelium_core::framing::make_gossip_update(
        &peer, 4, Arc::from(key), Bytes::from_static(b"v"), false, &ctx.hlc,
    );
    mycelium_core::store::apply_and_notify(&ctx.kv_state, &upd);
    let q = Query { stamp: upd.timestamp, content_hash: content_hash(key, b"v", false), key: Arc::from(key) };

    let pause = wal.pause_answer_between_counts();
    let asking = tokio::spawn({
        let ctx = Arc::clone(&ctx);
        async move { answer(&ctx, &q).await }
    });
    // Either the answer reaches the gap between its two loads, or it has already declined (it read the
    // in-flight count first, which is the fix). In the gap, the window closes without its append.
    let mut window = Some(window);
    for _ in 0..1000 {
        if asking.is_finished() { break; }
        tokio::select! {
            _ = pause.reached() => {
                drop(window.take());
                pause.release();
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
    }
    drop(window);
    let got = asking.await.unwrap();
    assert_eq!(wal.dropped_appends(), 1, "the abandoned window counted a dropped append");
    assert_eq!(got, Answer::Failed, "a record whose append was dropped must not be reported persisted");

    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}
