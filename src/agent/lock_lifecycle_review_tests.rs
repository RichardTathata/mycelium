//! **The implementation review of row A** (PR #600, "Adversarial review of the implementation"): one
//! test per finding, posed with **real frames withheld** by the test-only frame filter
//! (`mycelium_core::store::FrameView`, feature `test-support`) rather than by editing a node's store.

use crate::consensus::{decode_consensus_msg, ConsensusConfig, ConsensusMsg, ConsensusResult};
use crate::consensus_life::{self as life, Envelope, SlotView, Term};
use crate::store::FrameView;
use crate::{ConsistencyError, GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;

fn port() -> u16 { crate::test_util::alloc_port() }

async fn node_with(p: u16, peers: &[u16], edit: impl FnOnce(&mut GossipConfig)) -> GossipAgent {
    let mut cfg = GossipConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        bind_port: p,
        bootstrap_peers: peers.iter().map(|q| NodeId::new("127.0.0.1", *q).unwrap()).collect(),
        ..GossipConfig::default()
    };
    edit(&mut cfg);
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    a
}

async fn mesh(n: usize) -> (Vec<GossipAgent>, Vec<crate::ConsensusListenerHandle>) {
    let ports: Vec<u16> = (0..n).map(|_| port()).collect();
    let mut nodes = Vec::new();
    for (i, p) in ports.iter().enumerate() {
        // Each node dials only the nodes started before it (the P2 author's finding: dialling a node
        // not yet listening puts the dialer into reconnect backoff longer than a test waits).
        let others: Vec<u16> = ports[..i].to_vec();
        nodes.push(node_with(*p, &others, |_| {}).await);
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

fn wall() -> u64 { mycelium_core::sim_seam::wall_now_ms() }

/// Withhold from `a` every inbound frame `drop` matches (filter `id`).
fn withhold(a: &GossipAgent, id: u64, drop: impl Fn(&FrameView<'_>) -> bool + Send + Sync + 'static) {
    a.task_ctx.kv_state.frame_filter.pin().insert(id, Arc::new(drop));
}

fn heal(a: &GossipAgent, id: u64) { a.task_ctx.kv_state.frame_filter.pin().remove(&id); }

fn esc(slot: &str) -> String { slot.replace('%', "%25").replace('/', "%2F") }

/// A frame belongs to `slot`'s decision: its record namespace, its legacy keys, or a COMMIT for it.
fn of_slot(slot: &str, kinds: &'static [&'static str]) -> impl Fn(&FrameView<'_>) -> bool + Send + Sync + 'static {
    let life = format!("consensus/life/{}/", esc(slot));
    let legacy = [format!("consensus/committed/{slot}"), format!("consensus/lease/{slot}"), format!("consensus/decided/{slot}")];
    let slot = slot.to_string();
    move |f| match f {
        FrameView::Kv { key, .. } =>
            (kinds.contains(&"life") && key.starts_with(life.as_str()))
            || (kinds.contains(&"committed") && *key == legacy[0])
            || (kinds.contains(&"lease") && *key == legacy[1])
            || (kinds.contains(&"decided") && *key == legacy[2])
            || (kinds.contains(&"marker") && key.starts_with(format!("{life}end/").as_str())),
        FrameView::Signal { kind, payload, .. } if *kind == crate::consensus::consensus_kind::COMMIT && kinds.contains(&"commit") =>
            matches!(decode_consensus_msg(&Bytes::copy_from_slice(payload)),
                Some(ConsensusMsg::Commit { slot: s, .. } | ConsensusMsg::CommitTerm { slot: s, .. }) if *s == *slot),
        _ => false,
    }
}

/// Everything from `peers` (a partition, one direction).
fn from(peers: Vec<u64>) -> impl Fn(&FrameView<'_>) -> bool + Send + Sync + 'static {
    move |f| match f {
        FrameView::Kv { sender, .. } | FrameView::Signal { sender, .. } => peers.contains(sender),
    }
}

fn decided(a: &GossipAgent, slot: &str) -> u64 {
    a.task_ctx.kv_state.store.pin().get(format!("consensus/decided/{slot}").as_str())
        .and_then(|e| e.data.clone()).map(|b| crate::consensus::decode_ballot(&b)).unwrap_or(0)
}

/// **D1 (S): a renewal decides on the full report set.** P leads; C has accepted W at b′ and is
/// partitioned; P renews at b″ > b′ on {P, X}; then, with X partitioned and C back, P renews again on
/// {P, C}: it must keep its own leadership, not adopt W.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn d1_a_renewal_never_adopts_a_lower_value_over_its_own() {
    let (n, _l) = mesh(3).await;
    let (p, x, c) = (&n[0], &n[1], &n[2]);
    for a in [p, x, c] { a.mesh().join_group("d1"); }
    assert!(until(|| [p, x, c].iter().all(|a| a.mesh().group_members("d1").len() >= 3), 30_000).await);
    let slot = "leader/d1";
    let first = p.consensus().elect_leader_receipt("d1").await.expect("P elects");
    assert_eq!(first.leader, *p.node_id());
    assert!(until(|| c.consensus().consensus_get(slot).is_some(), 5_000).await);
    // C accepts W at b′ (another proposer's round that never committed), and the ballot is published.
    let b_prime = decided(c, slot) + 3;
    let w = Envelope::new(slot, Bytes::from(x.node_id().to_string().into_bytes()), Term::Lease { ms: 60_000, expires_at_ms: wall() + 60_000 },
        b_prime, x.node_id().clone(), 1);
    let cs: Arc<str> = Arc::from(slot);
    assert!(crate::consensus::claim_envelope(&c.task_ctx.consensus_accepted, &cs, b_prime, &w.encode(), x.node_id().id_hash(), 0));
    let _ = c.kv().set(format!("consensus/ballot/{slot}"), crate::consensus::encode_ballot(b_prime));
    assert!(until(|| p.task_ctx.kv_state.store.pin().get(format!("consensus/ballot/{slot}").as_str())
        .and_then(|e| e.data.clone()).map(|b| crate::consensus::decode_ballot(&b)) == Some(b_prime), 5_000).await);
    // C partitioned; P renews on {P, X}.
    withhold(c, 1, from(vec![p.node_id().id_hash(), x.node_id().id_hash()]));
    withhold(p, 1, from(vec![c.node_id().id_hash()]));
    withhold(x, 1, from(vec![c.node_id().id_hash()]));
    let r = p.consensus().elect_leader_receipt("d1").await.expect("P renews on {P, X}");
    assert_eq!(r.leader, *p.node_id());
    // C back, X partitioned; P renews on {P, C}.
    heal(c, 1);
    heal(p, 1);
    withhold(p, 2, from(vec![x.node_id().id_hash()]));
    withhold(c, 2, from(vec![x.node_id().id_hash()]));
    withhold(x, 2, from(vec![p.node_id().id_hash(), c.node_id().id_hash()]));
    let again = p.consensus().elect_leader_receipt("d1").await;
    assert!(matches!(&again, Ok(l) if l.leader == *p.node_id() && l.basis == crate::LeadershipBasis::Decided),
        "the renewal adopted W over its own higher decided acceptance: {again:?}");
    for a in [p, x, c] { a.shutdown().await; }
}

/// **D2 (S, mixed fleet): an upgraded proposer never sets acceptances aside by `decided`.** U and O
/// hold an older node's 1 s legacy lease for the slot; B commits; U and O receive B's `committed` and
/// `decided` but neither B's record, its lease, nor its COMMIT — so neither reads B's decision as live,
/// and U's promise quorum {U, O} reports B's acceptance with no live commit beside it. After the old
/// window, U must adopt B's acceptance and be refused, not set it aside by `decided`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn d2_an_upgraded_proposer_never_sets_aside_by_decided() {
    let (n, _l) = mesh(3).await;
    let (o, b, u) = (&n[0], &n[1], &n[2]);
    let slot = "lock/d2";
    // An older node's decision: the legacy keys only.
    let _ = o.kv().set(format!("consensus/committed/{slot}"), Bytes::from_static(b"old"));
    let _ = o.kv().set(format!("consensus/lease/{slot}"), crate::consensus::encode_lease_ms(1_000));
    assert!(until(|| u.task_ctx.kv_state.store.pin().get(format!("consensus/lease/{slot}").as_str()).is_some(), 5_000).await);
    withhold(u, 1, of_slot(slot, &["life", "lease", "commit"]));
    withhold(o, 1, of_slot(slot, &["life", "lease", "commit"]));
    tokio::time::sleep(Duration::from_millis(1_200)).await; // the older decision's window passes
    let gb = b.consensus().distributed_lock("d2", Duration::from_secs(60)).await.expect("B acquires");
    std::mem::forget(gb);
    assert!(until(|| decided(u, slot) > 0 && decided(o, slot) > 0, 5_000).await, "U and O received B's decided");
    // B is gone: U's quorum is {U, O}.
    b.shutdown().await;
    tokio::time::sleep(Duration::from_millis(1_200)).await; // B's committed entry is older than 1 s on U
    let r = u.consensus().distributed_lock("d2", Duration::from_secs(60)).await;
    assert!(matches!(r, Err(ConsistencyError::Superseded)), "a second holder beside B: {r:?}");
    std::mem::forget(r);
    for a in [o, u] { a.shutdown().await; }
}

/// **D5: a handoff whose release marker lags the new holder's COMMIT is written.** L holds A's live
/// record; A's release marker and B's record frames are withheld from L; B's COMMIT reaches L. L must
/// read B as the holder.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn d5_a_commit_ahead_of_its_release_marker_is_written() {
    let (n, _l) = mesh(3).await;
    let (a, b, l) = (&n[0], &n[1], &n[2]);
    let slot = "lock/d5";
    let ga = a.consensus().distributed_lock("d5", Duration::from_secs(60)).await.expect("A acquires");
    assert!(until(|| l.consensus().consensus_get(slot).is_some(), 5_000).await);
    let b_hash = b.node_id().id_hash();
    let life = format!("consensus/life/{}/", esc(slot));
    // Every record-namespace frame except the sentinel and A's own record (L holds those): A's release
    // marker and B's record never reach L, by any path or relay. B's COMMIT does. L also never sees
    // B's proposals, so its stale view of A cannot refuse them: B's quorum is {A, B}.
    let keep: Vec<String> = l.task_ctx.kv_state.store.pin().iter()
        .filter(|(k, v)| k.starts_with(life.as_str()) && v.data.is_some())
        .map(|(k, _)| k.to_string()).collect();
    withhold(l, 1, move |f| match f {
        FrameView::Kv { key, .. } => key.starts_with(life.as_str()) && !keep.iter().any(|k| k == key),
        FrameView::Signal { kind, sender, .. } =>
            *kind == crate::consensus::consensus_kind::PROPOSE && *sender == b_hash,
    });
    ga.release();
    let gb = b.consensus().locks().lock("d5", Duration::from_secs(60), Duration::from_secs(10)).await.expect("B acquires");
    let vb = b.consensus().consensus_get(slot).expect("B holds");
    std::mem::forget(gb);
    assert!(until(|| l.consensus().consensus_get(slot).as_ref() == Some(&vb), 5_000).await,
        "L still reads the released holder: {:?}", l.consensus().consensus_get(slot));
    for x in [a, b, l] { x.shutdown().await; }
}

/// **D4: a guard never outlives its lease.** A 1.9 s TTL: the lease must run to at least the guard's
/// deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d4_a_guard_never_outlives_its_lease() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let start = wall();
    let g = a.consensus().distributed_lock("d4", Duration::from_millis(1_900)).await.expect("acquire");
    let exp = g.expires_at_ms().expect("leased");
    assert!(exp >= start + 1_900, "the lease ends {} ms after the start, before the 1.9 s deadline", exp - start);
    std::mem::forget(g);
    a.shutdown().await;
}

/// **Growth: a thousand acquisitions leave a constant number of keys per slot after collection.**
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn growth_a_thousand_acquisitions_leave_constant_keys() {
    let a = node_with(port(), &[], |c| c.max_clock_drift_ms = 1).await;
    let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let engine = super::helpers::make_consensus_engine_ctx(&a.task_ctx, false, false, 0, None);
    for i in 0..1_000 {
        let g = a.consensus().distributed_lock("grow", Duration::from_secs(1)).await
            .unwrap_or_else(|e| panic!("acquire {i}: {e:?}"));
        g.release();
        if i % 100 == 99 { engine.collect_records().await; } // the collector's periodic pass
    }
    tokio::time::sleep(Duration::from_millis(1_200)).await; // every lease and the drift bound pass
    for _ in 0..4 { engine.collect_records().await; }
    crate::store::sweep_stale_tombstones(&a.task_ctx.kv_state.store, u64::MAX);
    let prefix = format!("consensus/life/{}/", esc("lock/grow"));
    let keys = a.task_ctx.kv_state.store.pin().iter().filter(|(k, _)| k.starts_with(prefix.as_str())).count();
    assert!(keys <= 3, "{keys} keys remain for one slot after 1,000 decisions and collection");
    a.shutdown().await;
}

/// **D6: the gateway's consistent read agrees with the library.**
#[cfg(feature = "gateway")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d6_the_gateway_consistent_get_reads_the_decision() {
    let gp = port();
    let hp = port();
    let a = node_with(gp, &[], |c| c.http_port = Some(hp)).await;
    let c = a.consensus();
    c.consistent_set("d6", Bytes::from_static(b"decided")).await.expect("set");
    let _ = a.kv().set("consensus/committed/consistent/d6", Bytes::from_static(b"raw-other"));
    assert_eq!(c.consistent_get("d6").as_deref(), Some(b"decided".as_slice()));
    let client = reqwest::Client::new();
    for _ in 0..40 {
        if client.get(format!("http://127.0.0.1:{hp}/health")).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let body: serde_json::Value = client.get(format!("http://127.0.0.1:{hp}/gateway/overlay/consistent/get?key=d6"))
        .send().await.expect("get").json().await.expect("json");
    use base64::Engine as _;
    assert_eq!(body["value_b64"], base64::engine::general_purpose::STANDARD.encode(b"decided"), "{body}");
    a.shutdown().await;
}

/// **D7: a record key spelled in uppercase hex is not a record.**
#[test]
fn d7_an_uppercase_ballot_is_not_a_record() {
    let kv = crate::consensus_life::test_kv(1024);
    let n = NodeId::new("127.0.0.1", 1).unwrap();
    let real = Envelope::new("s", Bytes::from_static(b"real"), Term::Permanent, 1, n.clone(), 1);
    let fake = Envelope::new("s", Bytes::from_static(b"fake"), Term::Permanent, 2, n, 2);
    let upd = |k: String, v: Bytes| crate::framing::GossipUpdate { nonce: 1, sender: 1, ttl: 1, is_tombstone: false, timestamp: 1, key: Arc::from(k.as_str()), value: v };
    crate::store::apply_and_notify(&kv, &upd(life::record_key("s", 1), real.encode()));
    crate::store::apply_and_notify(&kv, &upd(format!("{}000000000000000A", life::slot_prefix("s")), fake.encode()));
    match life::read_slot(&kv, "s", 0) {
        SlotView::Top(t) => assert_eq!(t.value.as_deref(), Some(b"real".as_slice()), "an uppercase-hex key was read as a record"),
        other => panic!("{other:?}"),
    }
}

/// **Row 21 (replay): collection's writes come in one order whatever the insertion order.**
#[test]
fn row21_collection_writes_in_sorted_order() {
    let build = |order: &[u64]| {
        let kv = crate::consensus_life::test_kv(1024);
        let n = NodeId::new("127.0.0.1", 1).unwrap();
        for b in order {
            let e = Envelope::new("s", Bytes::from(format!("v{b}").into_bytes()), Term::Lease { ms: 1, expires_at_ms: 1 }, *b, n.clone(), *b);
            crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
                nonce: *b, sender: 1, ttl: 1, is_tombstone: false, timestamp: *b, key: Arc::from(life::record_key("s", *b).as_str()), value: e.encode() });
        }
        kv
    };
    let a = build(&[1, 2, 3, 4, 5, 9]);
    let b = build(&[9, 4, 1, 5, 3, 2]);
    let ka = life::collectable(&a, "s", 10_000, 0).expect("top").dead_keys;
    let kb = life::collectable(&b, "s", 10_000, 0).expect("top").dead_keys;
    assert_eq!(ka, kb, "collection order depends on insertion order");
    let mut sorted = ka.clone();
    sorted.sort();
    assert_eq!(ka, sorted);
}

/// **D3: a slot's read costs O(its own records).** Not a timing assertion (CI hosts vary): the read
/// must not depend on other slots' history — measured in `measure_read_slot_cost`.
#[test]
fn d3_a_slot_read_ignores_other_slots() {
    let kv = crate::consensus_life::test_kv(1 << 20);
    let n = NodeId::new("127.0.0.1", 1).unwrap();
    for s in 0..200 {
        for b in 1..=20u64 {
            let slot = format!("other/{s}");
            let e = Envelope::new(&slot, Bytes::from_static(b"x"), Term::Permanent, b, n.clone(), b);
            crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
                nonce: b, sender: 1, ttl: 1, is_tombstone: false, timestamp: b, key: Arc::from(life::record_key(&slot, b).as_str()), value: e.encode() });
        }
    }
    assert!(matches!(life::read_slot(&kv, "mine", 0), SlotView::NoRecord));
    let scope = format!("consensus/life/{}", esc("other/3"));
    assert_eq!(mycelium_core::store::scan_scope(&kv, &scope).len(), 20, "one slot's scope holds its own keys only");
}

/// The measurement the review asked for: a slot read at 5,000 slots × 100 decisions, with the prefix
/// bucket scan (before) and the scope index (after). Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn measure_read_slot_cost() {
    let kv = crate::consensus_life::test_kv(1 << 22);
    let n = NodeId::new("127.0.0.1", 1).unwrap();
    for s in 0..5_000u64 {
        let slot = format!("lock/m{s}");
        for b in 1..=100u64 {
            let e = Envelope::new(&slot, Bytes::from_static(b"holder"), Term::Permanent, b, n.clone(), b);
            crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
                nonce: s * 1_000 + b, sender: 1, ttl: 1, is_tombstone: false, timestamp: b,
                key: Arc::from(life::record_key(&slot, b).as_str()), value: e.encode() });
        }
    }
    let t = std::time::Instant::now();
    for i in 0..20 { let _ = mycelium_core::store::scan_kv_prefix(&kv, &life::slot_prefix(&format!("lock/m{i}"))); }
    let before = t.elapsed() / 20;
    let t = std::time::Instant::now();
    for i in 0..2_000 { let _ = life::read_slot(&kv, &format!("lock/m{}", i % 5_000), 0); }
    let after = t.elapsed() / 2_000;
    eprintln!("MEASURE 500,000 records: prefix-bucket scan {before:?} per read; scope-index read_slot {after:?} per read");
}

/// Keeps `ConsensusResult` in use for the file's imports across feature sets.
#[allow(dead_code)]
fn _uses(_: ConsensusResult) {}

/// **The review of `ee3d9e4b`, finding 1 (the reviewer's probe, ported):** a 2.31 permanent
/// leadership, released the legacy way, must not be resurrected as an unreleasable permanent decision
/// by an upgraded proposer adopting the legacy acceptance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f1_a_legacy_released_permanent_is_not_resurrected() {
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    a.mesh().join_group("pz");
    assert!(until(|| !a.mesh().group_members("pz").is_empty(), 5_000).await);
    let slot = "leader/pz";
    let x = NodeId::new("127.0.0.1", 1).unwrap();
    let xv = Bytes::from(x.to_string().into_bytes());
    let b = 7u64;
    // The 2.31 decision: legacy committed (no lease: permanent), decided b, acceptor memory at b.
    let _ = a.kv().set(format!("consensus/committed/{slot}"), xv.clone());
    let _ = a.kv().set(format!("consensus/decided/{slot}"), crate::consensus::encode_ballot(b));
    let _ = a.kv().set(format!("consensus/ballot/{slot}"), crate::consensus::encode_ballot(b));
    let cs: Arc<str> = Arc::from(slot);
    assert!(crate::consensus::claim_vote(&a.task_ctx.consensus_accepted, &cs, b, &xv, x.id_hash(), 0));
    // The legacy release (what a 2.31 X's release_leadership did): committed + lease tombstoned.
    let _ = a.kv().delete(format!("consensus/committed/{slot}"));
    let _ = a.kv().delete(format!("consensus/lease/{slot}"));
    assert!(a.consensus().consensus_get(slot).is_none(), "released before the upgrade");
    // An upgraded node elects itself, permanently.
    let r = a.consensus().elect_leader_with("pz", crate::LeaderTerm::Permanent).await;
    assert!(matches!(&r, Ok(l) if l.leader == *a.node_id()),
        "the released 2.31 leader was resurrected: {r:?}");
    a.shutdown().await;
}

/// **The review of `ee3d9e4b`, finding 2, black-box:** a renewal whose own lineage is reported
/// **below** a foreign acceptance adopts the foreign one, as any proposer must (it may be chosen). P
/// leads at b1; C has accepted W at b′ > b1; P renews on {P, C}: P must not renew its own lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn f2_a_renewal_adopts_a_higher_foreign_acceptance() {
    let (n, _l) = mesh(3).await;
    let (p, x, c) = (&n[0], &n[1], &n[2]);
    for a in [p, x, c] { a.mesh().join_group("f2"); }
    assert!(until(|| [p, x, c].iter().all(|a| a.mesh().group_members("f2").len() >= 3), 30_000).await);
    let slot = "leader/f2";
    p.consensus().elect_leader_receipt("f2").await.expect("P elects");
    assert!(until(|| c.consensus().consensus_get(slot).is_some(), 5_000).await);
    let b_prime = decided(c, slot) + 3;
    let w = Envelope::new(slot, Bytes::from(x.node_id().to_string().into_bytes()), Term::Lease { ms: 60_000, expires_at_ms: wall() + 60_000 },
        b_prime, x.node_id().clone(), 1);
    let cs: Arc<str> = Arc::from(slot);
    assert!(crate::consensus::claim_envelope(&c.task_ctx.consensus_accepted, &cs, b_prime, &w.encode(), x.node_id().id_hash(), 0));
    let _ = c.kv().set(format!("consensus/ballot/{slot}"), crate::consensus::encode_ballot(b_prime));
    assert!(until(|| p.task_ctx.kv_state.store.pin().get(format!("consensus/ballot/{slot}").as_str())
        .and_then(|e| e.data.clone()).map(|b| crate::consensus::decode_ballot(&b)) == Some(b_prime), 5_000).await);
    // X partitioned: P's promise quorum is {P, C}.
    withhold(p, 1, from(vec![x.node_id().id_hash()]));
    withhold(c, 1, from(vec![x.node_id().id_hash()]));
    withhold(x, 1, from(vec![p.node_id().id_hash(), c.node_id().id_hash()]));
    // P adopts W (it may have been chosen). Committing it is then refused by P's own live decision,
    // so P reports what it reads, `Observed`; what it must never report is a renewal of its own lower
    // lineage, `Decided`, over W.
    let r = p.consensus().elect_leader_receipt("f2").await;
    assert!(!matches!(&r, Ok(l) if l.leader == *p.node_id() && l.basis == crate::LeadershipBasis::Decided),
        "the renewal kept its own lower lineage over a higher foreign acceptance: {r:?}");
    for a in [p, x, c] { a.shutdown().await; }
}

/// **The review of `ee3d9e4b`, finding 7: a learner never overwrites a tombstone.** A record key this
/// node holds as a tombstone (collected below a higher top) is not re-written by a late COMMIT of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f7_a_learner_never_overwrites_a_tombstone() {
    use crate::consensus::{consensus_kind, encode_consensus_msg};
    let (n, _l) = mesh(1).await;
    let a = &n[0];
    let slot = "f7";
    let low = Envelope::new(slot, Bytes::from_static(b"low"), Term::Permanent, 3, NodeId::new("127.0.0.1", 1).unwrap(), 1);
    let key = life::record_key(slot, 3);
    let _ = a.kv().delete(key.clone());
    assert!(a.task_ctx.kv_state.store.pin().get(key.as_str()).is_some_and(|e| e.data.is_none()), "a tombstone");
    let late = ConsensusMsg::CommitTerm { slot: Arc::from(slot), ballot: 3, envelope: low.encode() };
    let probe = Envelope::new("probe/f7", Bytes::from_static(b"p"), Term::Permanent, 1, NodeId::new("127.0.0.1", 1).unwrap(), 1);
    let probe_msg = ConsensusMsg::CommitTerm { slot: Arc::from("probe/f7"), ballot: 1, envelope: probe.encode() };
    let probe_key = life::record_key("probe/f7", 1);
    let mut applied = false;
    for _ in 0..200 {
        let _ = a.mesh().emit(consensus_kind::COMMIT, crate::signal::SignalScope::Cluster, encode_consensus_msg(&late));
        let _ = a.mesh().emit(consensus_kind::COMMIT, crate::signal::SignalScope::Cluster, encode_consensus_msg(&probe_msg));
        tokio::time::sleep(Duration::from_millis(25)).await;
        applied = a.task_ctx.kv_state.store.pin().get(probe_key.as_str()).is_some_and(|e| e.data.is_some());
        if applied { break; }
    }
    assert!(applied, "the listener processed the COMMITs");
    assert!(a.task_ctx.kv_state.store.pin().get(key.as_str()).is_some_and(|e| e.data.is_none()),
        "a late COMMIT overwrote a collected record's tombstone");
    a.shutdown().await;
}
