//! **Row A's interleavings, black-box** — `docs/design/lock-lifecycle.md` §9.
//!
//! Each test drives the public consensus API across real nodes (a listener on every node, structural
//! readiness polls) and asserts the exact outcome. They use only what both the old reading rules and
//! the decision record expose, so each was run against the branch before the record existed — the
//! row table in the PR says which failed there. A frame that must be *withheld* from a node is
//! modelled by removing, on that node, every key the decision would have written (`forget`), or by
//! restoring the keys it held before (`restore`): the substrate has no partition primitive, and these
//! are the states a withheld frame leaves.

use crate::consensus::{ConsensusConfig, ConsensusResult};
use crate::store::StoreEntry;
use crate::{ConsistencyError, GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;

fn port() -> u16 { crate::test_util::alloc_port() }

async fn node(p: u16, peers: &[u16]) -> GossipAgent {
    let cfg = GossipConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        bind_port: p,
        bootstrap_peers: peers.iter().map(|q| NodeId::new("127.0.0.1", *q).unwrap()).collect(),
        ..GossipConfig::default()
    };
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    a
}

/// `n` fully peered nodes, each with a listener, each seeing `n - 1` peers.
async fn mesh(n: usize) -> (Vec<GossipAgent>, Vec<crate::ConsensusListenerHandle>) {
    let ports: Vec<u16> = (0..n).map(|_| port()).collect();
    let mut nodes = Vec::new();
    for (i, p) in ports.iter().enumerate() {
        let others: Vec<u16> = ports.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, q)| *q).collect();
        nodes.push(node(*p, &others).await);
    }
    let ls = nodes.iter().map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    assert!(until(|| nodes.iter().all(|a| a.peers().len() >= n - 1), 30_000).await, "the mesh never peered");
    (nodes, ls)
}

async fn until(cond: impl Fn() -> bool, ms: u64) -> bool {
    for _ in 0..(ms / 25).max(1) {
        if cond() { return true; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

/// The slot's escaped name, as the decision-record namespace spells it (absent before row A).
fn esc(slot: &str) -> String { slot.replace('%', "%25").replace('/', "%2F") }

/// Every key a decision of `slot` writes, as `node` holds them: the legacy three and the record
/// namespace.
fn slot_keys(a: &GossipAgent, slot: &str) -> Vec<(Arc<str>, StoreEntry)> {
    let life = format!("consensus/life/{}/", esc(slot));
    let legacy = [
        format!("consensus/committed/{slot}"), format!("consensus/lease/{slot}"), format!("consensus/decided/{slot}"),
    ];
    a.task_ctx.kv_state.store.pin().iter()
        .filter(|(k, _)| k.starts_with(life.as_str()) || legacy.iter().any(|l| l.as_str() == k.as_ref()))
        .map(|(k, v)| (Arc::clone(k), v.clone()))
        .collect()
}

/// Remove, on `a`, every key a decision of `slot` writes whose kind is in `kinds` (`committed`,
/// `lease`, `decided`, `life`) — the state a withheld frame leaves.
fn forget(a: &GossipAgent, slot: &str, kinds: &[&str]) {
    let life = format!("consensus/life/{}/", esc(slot));
    let pin = a.task_ctx.kv_state.store.pin();
    for (k, _) in slot_keys(a, slot) {
        let kind = if k.starts_with(life.as_str()) { "life" }
            else if k.starts_with("consensus/committed/") { "committed" }
            else if k.starts_with("consensus/lease/") { "lease" }
            else { "decided" };
        if kinds.contains(&kind) { pin.remove(&k); }
    }
}

/// Put back, on `a`, the keys of `kinds` it held in `snapshot`, removing any it holds now that the
/// snapshot did not — a node that kept an older frame and never received the newer one.
fn restore(a: &GossipAgent, slot: &str, snapshot: &[(Arc<str>, StoreEntry)], kinds: &[&str]) {
    forget(a, slot, kinds);
    let life = format!("consensus/life/{}/", esc(slot));
    let pin = a.task_ctx.kv_state.store.pin();
    for (k, v) in snapshot {
        let kind = if k.starts_with(life.as_str()) { "life" }
            else if k.starts_with("consensus/committed/") { "committed" }
            else if k.starts_with("consensus/lease/") { "lease" }
            else { "decided" };
        if kinds.contains(&kind) { pin.insert(Arc::clone(k), v.clone()); }
    }
}

fn decided(a: &GossipAgent, slot: &str) -> u64 {
    a.task_ctx.kv_state.store.pin().get(format!("consensus/decided/{slot}").as_str())
        .and_then(|e| e.data.clone()).map(|b| crate::consensus::decode_ballot(&b)).unwrap_or(0)
}

fn leased(secs: u64) -> ConsensusConfig {
    ConsensusConfig { committed_lease_secs: Some(secs), ..ConsensusConfig::default() }
}

/// Row 1 (K1): a released lock whose old state was collected is not re-committed to its old holder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row01_a_released_lock_is_not_recommitted_after_collection() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let g = a.consensus().distributed_lock("r01", Duration::from_secs(60)).await.expect("acquire");
    let old = a.consensus().consensus_get("lock/r01").expect("held");
    g.release();
    crate::store::sweep_stale_tombstones(&a.task_ctx.kv_state.store, u64::MAX);
    let g2 = a.consensus().distributed_lock("r01", Duration::from_secs(60)).await;
    assert!(g2.is_ok(), "re-acquire after a release and collection: {:?}", g2.err());
    assert_ne!(a.consensus().consensus_get("lock/r01"), Some(old), "the old holder's value was committed again");
    std::mem::forget(g2);
    a.shutdown().await;
}

/// Row 2 (K2): a lapsed lease is reopened by a node without the decided floor; the new value commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row02_a_reopened_lease_does_not_adopt_the_expired_value() {
    let a = node(port(), &[]).await; // no listener: its own COMMIT does not re-write the floor
    let c = a.consensus();
    assert!(matches!(c.cluster_propose("r02", Bytes::from_static(b"old"), leased(1)).await, ConsensusResult::Committed { .. }));
    forget(&a, "r02", &["decided"]);
    assert!(until(|| c.consensus_get("r02").is_none(), 5_000).await, "the 1 s lease lapsed");
    match c.cluster_propose("r02", Bytes::from_static(b"new"), leased(60)).await {
        ConsensusResult::Committed { value, .. } => assert_eq!(value.as_ref(), b"new"),
        other => panic!("the reopened slot adopted the expired value: {other:?}"),
    }
    a.shutdown().await;
}

/// Row 3 (K3): a late COMMIT of a released lock, at a node missing the floor and the entry, does not
/// revive it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row03_a_late_commit_does_not_revive_a_released_lock() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let g = a.consensus().distributed_lock("r03", Duration::from_secs(60)).await.expect("acquire");
    let value = a.consensus().consensus_get("lock/r03").expect("held");
    let ballot = decided(a, "lock/r03");
    g.release();
    forget(a, "lock/r03", &["committed", "decided"]);
    let late = ConsensusMsg::Commit { slot: Arc::from("lock/r03"), ballot, value };
    let probe = ConsensusMsg::Commit { slot: Arc::from("probe/r03"), ballot: 1, value: Bytes::from_static(b"p") };
    for _ in 0..20 {
        let _ = a.mesh().emit(consensus_kind::COMMIT, crate::signal::SignalScope::Cluster, encode_consensus_msg(&late));
        let _ = a.mesh().emit(consensus_kind::COMMIT, crate::signal::SignalScope::Cluster, encode_consensus_msg(&probe));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(a.consensus().consensus_get("lock/r03"), None, "a late COMMIT revived a released lock");
    a.shutdown().await;
}

/// Row 4 (R1-1a): a lapsed guard's release, on a node that kept its own state and never received the
/// newer holder's, does not end the newer holder's lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row04_a_stale_guards_release_does_not_end_a_newer_holder() {
    let (n, _l) = mesh(2).await;
    let (a, b) = (&n[0], &n[1]);
    let ga = a.consensus().distributed_lock("r04", Duration::from_secs(2)).await.expect("A acquires");
    let a_view = slot_keys(a, "lock/r04");
    let gb = b.consensus().locks().lock("r04", Duration::from_secs(60), Duration::from_secs(15)).await
        .expect("B acquires once A's lease lapses");
    let vb = b.consensus().consensus_get("lock/r04").expect("B holds");
    assert!(until(|| a.consensus().consensus_get("lock/r04").as_ref() == Some(&vb), 5_000).await);
    // A never received B's decision: it holds what it held before.
    restore(a, "lock/r04", &a_view, &["committed", "lease", "decided", "life"]);
    drop(ga);
    // Whatever A's release wrote reaches B; B still holds, for the whole wait.
    for _ in 0..40 {
        assert_eq!(b.consensus().consensus_get("lock/r04").as_ref(), Some(&vb), "a stale release ended B's lock");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    std::mem::forget(gb);
    a.shutdown().await;
    b.shutdown().await;
}

/// Row 11 (R2-1): the learners hold B's commit beside A's older window or record; a third proposer
/// must be refused, `Superseded`, while B holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn row11_a_newer_commit_beside_an_older_window_is_not_ended() {
    let (n, _l) = mesh(5).await;
    let (a, b) = (&n[0], &n[1]);
    let learners = &n[2..];
    let ga = a.consensus().distributed_lock("r11", Duration::from_secs(2)).await.expect("A acquires");
    assert!(until(|| learners.iter().all(|l| l.consensus().consensus_get("lock/r11").is_some()), 5_000).await);
    let views: Vec<_> = learners.iter().map(|l| slot_keys(l, "lock/r11")).collect();
    std::mem::forget(ga);
    let gb = b.consensus().locks().lock("r11", Duration::from_secs(60), Duration::from_secs(15)).await
        .expect("B acquires once A's lease lapses");
    let vb = b.consensus().consensus_get("lock/r11").expect("B holds");
    std::mem::forget(gb);
    assert!(until(|| learners.iter().all(|l| l.consensus().consensus_get("lock/r11").as_ref() == Some(&vb)), 5_000).await);
    a.shutdown().await;
    b.shutdown().await;
    // B's window — its lease frame, its decision record — never reached the learners.
    for (l, v) in learners.iter().zip(&views) {
        restore(l, "lock/r11", v, &["lease", "life"]);
    }
    tokio::time::sleep(Duration::from_millis(2500)).await; // A's old window passes
    let c = learners[0].consensus().distributed_lock("r11", Duration::from_secs(60)).await;
    assert!(matches!(c, Err(ConsistencyError::Superseded)), "a second holder: {c:?}");
    std::mem::forget(c);
    for l in learners { l.shutdown().await; }
}

/// Row 12 (R2-2): a permanent commit stays live on a node that also holds an older lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row12_a_permanent_commit_beside_a_stale_lease_stays_live() {
    let (n, _l) = mesh(2).await;
    let (a, l) = (&n[0], &n[1]);
    assert!(matches!(a.consensus().cluster_propose("r12", Bytes::from_static(b"vA"), leased(1)).await, ConsensusResult::Committed { .. }));
    assert!(until(|| l.consensus().consensus_get("r12").is_some(), 5_000).await);
    let stale = slot_keys(l, "r12");
    assert!(until(|| a.consensus().consensus_get("r12").is_none(), 5_000).await);
    assert!(matches!(a.consensus().cluster_propose("r12", Bytes::from_static(b"vP"), ConsensusConfig::default()).await, ConsensusResult::Committed { .. }));
    assert!(until(|| l.consensus().consensus_get("r12").as_deref() == Some(b"vP".as_slice()), 5_000).await);
    a.shutdown().await;
    restore(l, "r12", &stale, &["lease"]);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(l.consensus().consensus_get("r12").as_deref(), Some(b"vP".as_slice()), "a permanent commit expired");
    l.shutdown().await;
}

/// Row 13 (R2-3): a live permanent leader releases even when a stale-view node re-committed its value
/// higher (modelled by a higher decided ballot).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row13_a_permanent_leader_releases_after_a_higher_recommit() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    a.mesh().join_group("r13");
    a.consensus().elect_leader_with("r13", crate::LeaderTerm::Permanent).await.expect("elects");
    let b = decided(a, "leader/r13");
    let _ = a.kv().set("consensus/decided/leader/r13", crate::consensus::encode_ballot(b + 3));
    assert!(a.consensus().release_leadership("r13").await, "a live permanent leader was refused its release");
    assert_eq!(a.consensus().consensus_get("leader/r13"), None);
    a.shutdown().await;
}

/// Row 17 (R3-1): B's decision record carries a later HLC (B's clock is ahead); B dies; the others
/// lose B's commit and floor but keep its record. They must not wedge: once B's TTL has passed, the
/// lock is free.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn row17_a_skewed_holders_decision_does_not_wedge_the_lock() {
    let (n, _l) = mesh(4).await;
    let b = &n[0];
    let others = &n[1..];
    b.task_ctx.hlc.observe(crate::hlc::pack(mycelium_core::sim_seam::wall_now_ms() + 200_000, 0));
    let gb = b.consensus().distributed_lock("r17", Duration::from_secs(2)).await.expect("B acquires");
    std::mem::forget(gb);
    assert!(until(|| others.iter().all(|o| o.consensus().consensus_get("lock/r17").is_some()), 5_000).await);
    b.shutdown().await;
    for o in others { forget(o, "lock/r17", &["committed", "decided"]); }
    // C proposes now: it adopts B's acceptance (or sees B's live decision) — never commits its own.
    let c = others[0].consensus().distributed_lock("r17", Duration::from_secs(60)).await;
    assert!(matches!(c, Err(ConsistencyError::Superseded)), "C took B's live lock: {c:?}");
    std::mem::forget(c);
    // B's 2 s window passes: D acquires.
    let mut got = None;
    for _ in 0..40 {
        if let Ok(g) = others[1].consensus().distributed_lock("r17", Duration::from_secs(60)).await {
            got = Some(g);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(got.is_some(), "the lock is wedged for ever after B's window");
    std::mem::forget(got);
    for o in others { o.shutdown().await; }
}

/// Row 20 (R3-4): a released leadership does not read live again when a higher decided ballot arrives
/// before the newer decision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row20_an_ended_decision_does_not_revive_on_a_higher_floor() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    a.mesh().join_group("r20");
    a.consensus().elect_leader_receipt("r20").await.expect("elects");
    assert!(a.consensus().release_leadership("r20").await);
    let b = decided(a, "leader/r20");
    let _ = a.kv().set("consensus/decided/leader/r20", crate::consensus::encode_ballot(b + 5));
    assert_eq!(a.consensus().consensus_get("leader/r20"), None, "an ended leadership read live again");
    a.shutdown().await;
}

/// Row 22 (Q1): an adopter with a shorter TTL does not shorten the holder's lease. B holds `q1` for
/// 60 s and dies; C, which never saw B's commit, proposes with a 1 s TTL and adopts B's acceptance;
/// two seconds later D must still be refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn row22_an_adoption_keeps_the_holders_lease() {
    let (n, _l) = mesh(4).await;
    let b = &n[0];
    let others = &n[1..];
    let gb = b.consensus().distributed_lock("r22", Duration::from_secs(60)).await.expect("B acquires");
    std::mem::forget(gb);
    assert!(until(|| others.iter().all(|o| o.consensus().consensus_get("lock/r22").is_some()), 5_000).await);
    b.shutdown().await;
    for o in others { forget(o, "lock/r22", &["committed", "lease", "decided", "life"]); }
    let c = others[0].consensus().distributed_lock("r22", Duration::from_secs(1)).await;
    assert!(matches!(c, Err(ConsistencyError::Superseded)), "C took B's lock: {c:?}");
    std::mem::forget(c);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let d = others[1].consensus().distributed_lock("r22", Duration::from_secs(60)).await;
    assert!(matches!(d, Err(ConsistencyError::Superseded)), "D acquired inside B's 60 s lease: {d:?}");
    std::mem::forget(d);
    for o in others { o.shutdown().await; }
}

/// Row 23 (Q2): a late COMMIT of an older decision, at a node holding neither the floor nor any
/// record of the slot, does not displace the permanent decision fleet-wide.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row23_a_late_commit_does_not_displace_a_permanent_decision() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
    let (n, _l) = mesh(2).await;
    let (a, l) = (&n[0], &n[1]);
    assert!(matches!(a.consensus().cluster_propose("r23", Bytes::from_static(b"vA"), leased(1)).await, ConsensusResult::Committed { .. }));
    let b1 = decided(a, "r23");
    assert!(until(|| a.consensus().consensus_get("r23").is_none(), 5_000).await);
    assert!(matches!(a.consensus().cluster_propose("r23", Bytes::from_static(b"vP"), ConsensusConfig::default()).await, ConsensusResult::Committed { .. }));
    assert!(until(|| l.consensus().consensus_get("r23").as_deref() == Some(b"vP".as_slice()), 5_000).await);
    forget(l, "r23", &["committed", "lease", "decided", "life"]);
    let late = ConsensusMsg::Commit { slot: Arc::from("r23"), ballot: b1, value: Bytes::from_static(b"vA") };
    for _ in 0..20 {
        let _ = l.mesh().emit(consensus_kind::COMMIT, crate::signal::SignalScope::Individual(l.node_id().clone()), encode_consensus_msg(&late));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for _ in 0..60 {
        assert_eq!(a.consensus().consensus_get("r23").as_deref(), Some(b"vP".as_slice()), "a late COMMIT displaced the permanent decision");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    a.shutdown().await;
    l.shutdown().await;
}

/// Row 26 (review finding 4): a peer pulling this node's HLC forward does not end a live lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row26_hlc_drift_does_not_end_a_lease_early() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let g = a.consensus().distributed_lock("r26", Duration::from_secs(60)).await.expect("acquire");
    a.task_ctx.hlc.observe(crate::hlc::pack(mycelium_core::sim_seam::wall_now_ms() + 120_000, 0));
    assert!(a.consensus().consensus_get("lock/r26").is_some(), "HLC drift ended a 60 s lease early");
    std::mem::forget(g);
    a.shutdown().await;
}

/// Row 27 (review finding 5): after collection, the refusal floor is this node's own record — a
/// regressed shared `decided` key does not let a finished ballot be accepted again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row27_collection_keeps_a_node_owned_floor() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let g = a.consensus().distributed_lock("r27", Duration::from_secs(60)).await.expect("acquire");
    let ballot = decided(a, "lock/r27");
    g.release();
    a.consensus().collect_finished_acceptor_state().await;
    let _ = a.kv().set("consensus/decided/lock/r27", crate::consensus::encode_ballot(0));
    let slot: Arc<str> = Arc::from("lock/r27");
    assert!(!crate::consensus::claim_vote(&a.task_ctx.consensus_accepted, &slot, ballot,
        &Bytes::from_static(b"x"), 0xBEEF, 0), "a finished ballot was accepted after collection");
    a.shutdown().await;
}
