//! **Row A's interleavings against the decision record** — `docs/design/lock-lifecycle.md` §9, the
//! rows that need the record itself (writing an envelope, delivering a `CommitTerm`, reading the top),
//! plus C1 and C2. Written first against the new API; the black-box rows are in
//! `lock_lifecycle_tests`.

use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusConfig, ConsensusMsg, ConsensusResult};
use crate::consensus_life::{self as life, Envelope, SlotView, Term};
use crate::signal::SignalScope;
use crate::{GossipAgent, GossipConfig, NodeId};
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

async fn solo() -> (GossipAgent, crate::ConsensusListenerHandle) {
    let a = node(port(), &[]).await;
    let l = a.consensus().start_consensus_listener(ConsensusConfig::default());
    (a, l)
}

async fn until(cond: impl Fn() -> bool, ms: u64) -> bool {
    for _ in 0..(ms / 25).max(1) {
        if cond() { return true; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

fn wall() -> u64 { mycelium_core::sim_seam::wall_now_ms() }

fn top(a: &GossipAgent, slot: &str) -> SlotView { life::read_slot(&a.task_ctx.kv_state, slot, wall()) }

fn top_env(a: &GossipAgent, slot: &str) -> Envelope {
    match top(a, slot) { SlotView::Top(t) => t.env, other => panic!("no top record for {slot}: {other:?}") }
}

fn decided(a: &GossipAgent, slot: &str) -> u64 {
    a.task_ctx.kv_state.store.pin().get(format!("consensus/decided/{slot}").as_str())
        .and_then(|e| e.data.clone()).map(|b| crate::consensus::decode_ballot(&b)).unwrap_or(0)
}

fn other_node() -> NodeId { NodeId::new("127.0.0.1", 1).unwrap() }

/// Deliver `msg` to `a`'s listener and return once it has been processed: a probe `CommitTerm`
/// emitted after it is applied after it (the listener drains its queue in order).
async fn deliver(a: &GossipAgent, msg: &ConsensusMsg, probe: &str) {
    let env = Envelope::new(probe, Bytes::from_static(b"p"), Term::Permanent, 1, other_node(), 1);
    let probe_msg = ConsensusMsg::CommitTerm { slot: Arc::from(probe), ballot: 1, envelope: env.encode() };
    let key = life::record_key(probe, 1);
    let done = until_emit(a, msg, &probe_msg, || a.task_ctx.kv_state.store.pin().get(key.as_str()).is_some_and(|e| e.data.is_some())).await;
    assert!(done, "the probe COMMIT behind the message was never applied");
}

async fn until_emit(a: &GossipAgent, msg: &ConsensusMsg, probe: &ConsensusMsg, done: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, encode_consensus_msg(msg));
        let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, encode_consensus_msg(probe));
        tokio::time::sleep(Duration::from_millis(25)).await;
        if done() { return true; }
    }
    done()
}

/// C1: `elect_leader` is leased by default (30 s); `release_leadership` steps down; permanence is an
/// explicit opt-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c1_elect_leader_is_leased_by_default_with_a_release_path() {
    let (a, _l) = solo().await;
    a.mesh().join_group("c1");
    let l = a.consensus().elect_leader_receipt("c1").await.expect("elects");
    assert_eq!(l.leader, *a.node_id());
    assert!(matches!(top_env(&a, "leader/c1").term, Term::Lease { ms: 30_000, .. }), "the default lease");
    assert!(a.consensus().release_leadership("c1").await, "the leader may release");
    assert_eq!(a.consensus().consensus_get("leader/c1"), None, "released leadership reads as no leader");
    assert!(!a.consensus().release_leadership("c1").await, "nothing left to release");
    a.mesh().join_group("c1-perm");
    a.consensus().elect_leader_with("c1-perm", crate::LeaderTerm::Permanent).await.expect("elects");
    assert_eq!(top_env(&a, "leader/c1-perm").term, Term::Permanent);
    a.shutdown().await;
}

/// Row 13 (R2-3): a live permanent leader whose decision a stale-view node re-committed at a higher
/// ballot (the same envelope, adopted) releases — and the release ends the re-commit too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row13_a_release_ends_an_adopted_recommit() {
    let (a, _l) = solo().await;
    a.mesh().join_group("r13b");
    a.consensus().elect_leader_with("r13b", crate::LeaderTerm::Permanent).await.expect("elects");
    let env = top_env(&a, "leader/r13b");
    let higher = decided(&a, "leader/r13b") + 3;
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("leader/r13b"), ballot: higher, envelope: env.encode() }, "probe/r13b").await;
    assert!(matches!(top(&a, "leader/r13b"), SlotView::Top(t) if t.ballot == higher), "the re-commit is the top");
    assert!(a.consensus().release_leadership("r13b").await, "a live permanent leader was refused its release");
    assert_eq!(a.consensus().consensus_get("leader/r13b"), None, "the adopted re-commit outlived the release");
    a.shutdown().await;
}

/// C1: a dead leader's lease lapses and a survivor's election succeeds, `Decided`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn c1_a_dead_leaders_lease_lapses_and_a_new_election_succeeds() {
    let (p1, p2, p3) = (port(), port(), port());
    let a = node(p1, &[p2, p3]).await;
    let b = node(p2, &[p1, p3]).await;
    let c = node(p3, &[p1, p2]).await;
    let _ls: Vec<_> = [&a, &b, &c].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    for n in [&a, &b, &c] { n.mesh().join_group("c1d"); }
    assert!(until(|| [&a, &b, &c].iter().all(|n| n.mesh().group_members("c1d").len() >= 3), 30_000).await);
    let l = a.consensus().elect_leader_with("c1d", crate::LeaderTerm::Lease(Duration::from_secs(2))).await.expect("A elects");
    assert_eq!(l.leader, *a.node_id());
    assert!(until(|| b.consensus().consensus_get("leader/c1d").is_some(), 5_000).await, "B saw A's leadership");
    a.shutdown().await;
    let mut won = false;
    for _ in 0..60 {
        if let Ok(l) = b.consensus().elect_leader_receipt("c1d").await
            && l.leader == *b.node_id() && l.basis == crate::LeadershipBasis::Decided {
            won = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(won, "a dead leader was reported for ever");
    b.shutdown().await;
    c.shutdown().await;
}

/// C2: acceptor state for a finished decision shrinks to the node-owned floor; a promise to a later
/// ballot on a finished slot, and a live slot's state, survive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c2_acceptor_state_shrinks_to_a_floor_once_its_decision_is_over() {
    use crate::consensus::{accepted_key, decode_acceptor, prepare_slot, PrepareOutcome};
    let (a, _l) = solo().await;
    let ttl = Duration::from_secs(60);
    let g = a.consensus().distributed_lock("c2-done", ttl).await.expect("acquire");
    let done_ballot = decided(&a, "lock/c2-done");
    g.release();
    let g = a.consensus().distributed_lock("c2-inflight", ttl).await.expect("acquire");
    let inflight_ballot = decided(&a, "lock/c2-inflight");
    g.release();
    let inflight: Arc<str> = Arc::from("lock/c2-inflight");
    assert!(matches!(prepare_slot(&a.task_ctx.consensus_accepted, &inflight, inflight_ballot + 1, 7, 0), PrepareOutcome::Promised(_)));
    let held = a.consensus().distributed_lock("c2-live", ttl).await.expect("acquire");
    let mem = |s: &str| a.task_ctx.consensus_accepted.pin().get(s).cloned();
    let record = |s: &str| a.task_ctx.kv_state.store.pin().get(accepted_key(a.node_id(), s).as_str())
        .and_then(|e| e.data.clone()).and_then(|b| decode_acceptor(&b));

    assert!(a.consensus().collect_finished_acceptor_state().await >= 1);
    let done = mem("lock/c2-done").expect("the floor is kept");
    assert_eq!((done.promised, done.accepted.is_none()), (done_ballot, true), "shrunk to the floor");
    let rec = record("lock/c2-done").expect("the durable record is kept");
    assert_eq!((rec.promised, rec.accepted.is_none()), (done_ballot, true), "the durable record shrank too");
    assert_eq!(mem("lock/c2-inflight").map(|s| s.promised), Some(inflight_ballot + 1), "an in-flight promise was touched");
    assert!(mem("lock/c2-live").is_some_and(|s| s.accepted.is_some()), "a live slot's acceptance was collected");
    let g = a.consensus().distributed_lock("c2-done", ttl).await;
    assert!(g.is_ok(), "re-acquire after collection: {:?}", g.err());
    std::mem::forget(g);
    std::mem::forget(held);
    a.shutdown().await;
}

/// C2 / review round 2 finding 8: the shrink's compare-and-set re-check — a promise above the ended
/// ballot that arrived after the candidate was chosen is not shrunk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c2_the_shrink_rechecks_the_promise_inside_its_compare_and_set() {
    use crate::consensus::{prepare_slot, PrepareOutcome};
    let a = node(port(), &[]).await;
    let leased = ConsensusConfig { committed_lease_secs: Some(0), ..ConsensusConfig::default() };
    let ConsensusResult::Committed { ballot, .. } =
        a.consensus().cluster_propose("cas", Bytes::from_static(b"v"), leased).await else { panic!("commits") };
    let slot: Arc<str> = Arc::from("cas");
    assert!(matches!(prepare_slot(&a.task_ctx.consensus_accepted, &slot, ballot + 1, 9, 0), PrepareOutcome::Promised(_)));
    let engine = super::helpers::make_consensus_engine_ctx(&a.task_ctx, false, false, 0, None);
    assert!(!engine.shrink_acceptor_for_test(&slot, ballot), "a newer promise was shrunk");
    assert_eq!(a.task_ctx.consensus_accepted.pin().get(&slot).map(|s| s.promised), Some(ballot + 1));
    a.shutdown().await;
}

/// C2 / round 2 finding 4: one pass collects at most the budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c2_collection_is_bounded_per_pass() {
    use crate::consensus::ACCEPTOR_COLLECT_BUDGET;
    let a = node(port(), &[]).await;
    let leased = ConsensusConfig { committed_lease_secs: Some(0), ..ConsensusConfig::default() };
    let n = ACCEPTOR_COLLECT_BUDGET + 6;
    for i in 0..n {
        let r = a.consensus().cluster_propose(&format!("budget/{i}"), Bytes::from_static(b"v"), leased.clone()).await;
        assert!(matches!(r, ConsensusResult::Committed { .. }), "{r:?}");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(a.consensus().collect_finished_acceptor_state().await, ACCEPTOR_COLLECT_BUDGET);
    assert_eq!(a.consensus().collect_finished_acceptor_state().await, n - ACCEPTOR_COLLECT_BUDGET);
    a.shutdown().await;
}

/// Row 5 (R1-1b): a late COMMIT below a newer decision writes the lower record; the top stays.
/// Row 6 / 19 (R1-1c, R3-3): a `CommitTerm` alone carries the whole decision — value and window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row05_06_19_a_commit_carries_the_whole_decision_and_a_lower_one_never_outranks() {
    let (a, _l) = solo().await;
    let b_env = Envelope::new("r05", Bytes::from_static(b"vB"), Term::Lease { ms: 60_000, expires_at_ms: wall() + 60_000 }, 9, other_node(), 5);
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("r05"), ballot: 9, envelope: b_env.encode() }, "probe/r05a").await;
    assert_eq!(a.consensus().consensus_get("r05").as_deref(), Some(b"vB".as_slice()), "the COMMIT alone carried value and window");
    assert!(matches!(top(&a, "r05"), SlotView::Top(t) if t.env.expires_at_ms() == b_env.expires_at_ms()));
    let a_env = Envelope::new("r05", Bytes::from_static(b"vA"), Term::Permanent, 5, other_node(), 3);
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("r05"), ballot: 5, envelope: a_env.encode() }, "probe/r05b").await;
    assert_eq!(a.consensus().consensus_get("r05").as_deref(), Some(b"vB".as_slice()), "a lower record outranked the top");
    a.shutdown().await;
}

/// Row 7 (R1-2): a legacy re-stamp of `committed` — live by the legacy reading — does not revive an
/// ended decision for an upgraded reader. (A reader of the legacy keys would read it live.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row07_a_legacy_restamp_of_committed_is_not_read() {
    let (a, _l) = solo().await;
    let g = a.consensus().distributed_lock("r07", Duration::from_secs(60)).await.expect("acquire");
    let held = a.consensus().consensus_get("lock/r07").expect("held");
    g.release();
    assert_eq!(a.consensus().consensus_get("lock/r07"), None);
    // An older learner's re-stamp: the committed value, live for 60 s by the legacy reading.
    let _ = a.kv().set("consensus/committed/lock/r07", held);
    let _ = a.kv().set("consensus/lease/lock/r07", crate::consensus::encode_lease_ms(60_000));
    assert_eq!(a.consensus().consensus_get("lock/r07"), None, "the legacy keys revived a released lock");
    a.shutdown().await;
}

/// Row 8 (R1-3, Q2): a release ends the lineage — a renewal of it, committing after the release, is
/// ended too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row08_a_release_ends_every_renewal_of_its_lineage() {
    let (a, _l) = solo().await;
    a.mesh().join_group("r08");
    a.consensus().elect_leader_receipt("r08").await.expect("elects");
    let first = top_env(&a, "leader/r08");
    a.consensus().elect_leader_receipt("r08").await.expect("renews");
    let renewed = top_env(&a, "leader/r08");
    assert_eq!(renewed.lineage, first.lineage, "a renewal keeps the lineage");
    assert!(renewed.expires_at_ms() >= first.expires_at_ms(), "and takes a fresh window");
    assert!(a.consensus().release_leadership("r08").await);
    // A renewal that was in flight commits after the release, at a higher ballot.
    let late = Envelope { term: Term::Lease { ms: 30_000, expires_at_ms: wall() + 30_000 }, ..renewed.clone() };
    let b = decided(&a, "leader/r08") + 1;
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("leader/r08"), ballot: b, envelope: late.encode() }, "probe/r08").await;
    assert_eq!(a.consensus().consensus_get("leader/r08"), None, "a renewal outlived the release of its lineage");
    a.shutdown().await;
}

/// Row 10 (R1-6): a decision without a record (an older proposer's) is released the legacy way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row10_a_legacy_decision_is_released_the_legacy_way() {
    let a = node(port(), &[]).await;
    let me = Bytes::from(a.node_id().to_string().into_bytes());
    let _ = a.kv().set("consensus/committed/leader/r10", me);
    assert!(a.consensus().consensus_get("leader/r10").is_some(), "the legacy reading");
    assert!(a.consensus().release_leadership("r10").await);
    assert_eq!(a.consensus().consensus_get("leader/r10"), None);
    a.shutdown().await;
}

/// Row 16 / review round 2 finding 9: a decision record far above every observed ballot is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row16_a_forged_record_ballot_is_counted() {
    let (a, _l) = solo().await;
    let forged = Envelope::new("r16", Bytes::from_static(b"f"), Term::Permanent, 1, other_node(), 1);
    let _ = a.kv().set(life::record_key("r16", u64::MAX - 1), forged.encode());
    let env = Envelope::new("r16", Bytes::from_static(b"v"), Term::Permanent, 3, other_node(), 1);
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("r16"), ballot: 3, envelope: env.encode() }, "probe/r16").await;
    assert!(a.system_stats().consensus_decided_floor_anomalies >= 1, "a forged record ballot went uncounted");
    a.shutdown().await;
}

/// Row 18 (R3-2): a released permanent leader, holding a higher decided ballot but not the newer
/// decision, cannot release again or address the newer decision — even one whose lineage number
/// equals its own (another proposer's); the newer leader reads live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row18_a_release_cannot_address_a_newer_decision() {
    let (a, _l) = solo().await;
    a.mesh().join_group("r18");
    a.consensus().elect_leader_with("r18", crate::LeaderTerm::Permanent).await.expect("elects");
    let mine = top_env(&a, "leader/r18");
    assert!(a.consensus().release_leadership("r18").await);
    let _ = a.kv().set("consensus/decided/leader/r18", crate::consensus::encode_ballot(9));
    assert!(!a.consensus().release_leadership("r18").await, "a released leader released again");
    let c = Envelope::new("leader/r18", Bytes::from_static(b"C"), Term::Permanent, mine.lineage, other_node(), 99);
    deliver(&a, &ConsensusMsg::CommitTerm { slot: Arc::from("leader/r18"), ballot: 9, envelope: c.encode() }, "probe/r18").await;
    assert_eq!(a.consensus().consensus_get("leader/r18").as_deref(), Some(b"C".as_slice()), "A's release ended the newer leader");
    a.shutdown().await;
}

/// Row 24 (review finding 1): a permanent decision's marker is never collected; a lease's only after
/// its expiry plus the drift bound; lower records are tombstoned; the sentinel keeps a slot whose
/// records are gone from ever reading the legacy way.
#[test]
fn row24_collection_never_drops_a_permanent_marker() {
    let kv = crate::store::KvState::new(1024);
    let put = |k: String, v: Bytes| crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
        nonce: fastrand::u64(..), sender: 1, ttl: 1, is_tombstone: false, timestamp: 1, key: Arc::from(k.as_str()), value: v });
    let n = other_node();
    let perm = Envelope::new("s", Bytes::from_static(b"A"), Term::Permanent, 5, n.clone(), 1);
    let lease = Envelope::new("s", Bytes::from_static(b"L"), Term::Lease { ms: 1, expires_at_ms: 1_000 }, 7, n.clone(), 2);
    let top_env = Envelope::new("s", Bytes::from_static(b"C"), Term::Permanent, 9, n.clone(), 3);
    put(life::sentinel_key("s"), Bytes::from_static(&life::SENTINEL));
    for (b, e) in [(5, &perm), (7, &lease), (9, &top_env)] {
        put(life::record_key("s", b), e.encode());
        put(life::marker_key("s", e.lineage, &e.proposer), life::encode_marker(e));
    }
    let c = life::collectable(&kv, "s", 1_000 + 300_000 + 1, 300_000).expect("a top is held");
    assert!(c.dead_keys.contains(&life::record_key("s", 5)) && c.dead_keys.contains(&life::record_key("s", 7)), "lower records go");
    assert!(!c.dead_keys.contains(&life::record_key("s", 9)), "never the top");
    assert!(c.dead_keys.contains(&life::marker_key("s", 7, &n)), "an expired lease marker past the drift bound goes");
    assert!(!c.dead_keys.contains(&life::marker_key("s", 5, &n)), "a permanent marker is never collected");
    assert!(!c.dead_keys.contains(&life::sentinel_key("s")), "never the sentinel");
    let early = life::collectable(&kv, "s", 1_000 + 1, 300_000).expect("a top is held");
    assert!(!early.dead_keys.contains(&life::marker_key("s", 7, &n)), "not before the drift bound");
    // Every record gone, the sentinel kept: unknown — never the legacy fallback.
    for b in [5u64, 7, 9] {
        crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
            nonce: fastrand::u64(..), sender: 1, ttl: 1, is_tombstone: true, timestamp: 2, key: Arc::from(life::record_key("s", b).as_str()), value: Bytes::new() });
    }
    put("consensus/committed/s".into(), Bytes::from_static(b"legacy"));
    assert!(matches!(life::read_slot(&kv, "s", 0), SlotView::Unknown));
}

/// Row 25 (review finding 2): an upgraded learner ignores the legacy COMMIT — it writes neither
/// `committed` nor `decided` from it, and a released decision stays released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row25_the_legacy_commit_writes_nothing() {
    let (a, _l) = solo().await;
    let legacy = ConsensusMsg::Commit { slot: Arc::from("r25"), ballot: 4, value: Bytes::from_static(b"x") };
    deliver(&a, &legacy, "probe/r25").await;
    assert!(a.task_ctx.kv_state.store.pin().get("consensus/committed/r25").is_none(), "committed was written");
    assert_eq!(decided(&a, "r25"), 0, "decided was raised");
    let g = a.consensus().distributed_lock("r25", Duration::from_secs(60)).await.expect("acquire");
    let held = a.consensus().consensus_get("lock/r25").expect("held");
    let ballot = decided(&a, "lock/r25");
    g.release();
    let late = ConsensusMsg::Commit { slot: Arc::from("lock/r25"), ballot: ballot + 1, value: held };
    deliver(&a, &late, "probe/r25b").await;
    assert_eq!(a.consensus().consensus_get("lock/r25"), None, "a legacy COMMIT revived a released lock");
    assert_eq!(decided(&a, "lock/r25"), ballot, "a legacy COMMIT raised the floor");
    a.shutdown().await;
}

/// Row 28 (review finding 8): a slot named `lock/a/<16 hex>` cannot inject a top record into `lock/a`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn row28_a_nested_slot_name_cannot_inject_a_record() {
    let a = node(port(), &[]).await;
    assert!(matches!(a.consensus().cluster_propose("lock/a/0000000000000009", Bytes::from_static(b"x"), ConsensusConfig::default()).await,
        ConsensusResult::Committed { .. }));
    assert!(matches!(top(&a, "lock/a"), SlotView::NoRecord), "a nested slot's record was read as lock/a's");
    assert_eq!(a.consensus().consensus_get("lock/a"), None);
    a.shutdown().await;
}
