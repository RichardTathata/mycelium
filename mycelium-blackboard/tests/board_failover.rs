//! WS-G / G3 · Phase 3 (Gate G-G3.3): emergent primary/secondary failover.
//!
//! A fact posted to the primary is live-replicated to the secondary; the primary claims it
//! (in-flight, NOT acked); the primary is killed; the secondary promotes when the primary's
//! capability evaporates; and the in-flight claim **survives** — the fact re-queues on the new
//! primary and is re-claimable (at-least-once: a claimer that drops mid-work does not strand the
//! finite fact). Also covers the `Auto` election — that it settles on one primary that serves, and
//! (P10) that two rings over the same candidates do not both elect the same node.

use bytes::Bytes;
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_blackboard::{Blackboard, BoardConfig, BoardRole, Predicate};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// The core's bind-verified, process-unique loopback allocator (`mycelium::test_util::alloc_port`,
/// the `test-util` feature) — retires the old bind-:0-and-drop TOCTOU flake class.
fn alloc_port() -> u16 {
    mycelium::test_util::alloc_port()
}

fn alloc_two_sorted_ports() -> (u16, u16) {
    loop {
        let (a, b) = (alloc_port(), alloc_port());
        if a != b {
            return (a.min(b), a.max(b));
        }
    }
}

async fn start_agent(port: u16, bootstrap: Option<u16>) -> Arc<GossipAgent> {
    let id = NodeId::new("127.0.0.1", port).expect("node id");
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.health_check_max_jitter_ms = 50;
    cfg.bootstrap_peers = bootstrap
        .map(|b| vec![NodeId::new("127.0.0.1", b).expect("bootstrap id")])
        .unwrap_or_default();
    // Failover hinges on capability-evaporation timing on a small loopback cluster; pin the
    // legacy failure detector for determinism (as the tuple-space failover tests do).
    cfg.swim_failure_detector = false;
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.expect("agent start");
    agent
}

async fn wait_peered(agents: &[&Arc<GossipAgent>]) {
    for _ in 0..400 {
        if agents.iter().all(|a| !a.peers().is_empty()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("agents never peered");
}

fn bb_cfg(ns: &str, role: BoardRole) -> BoardConfig {
    BoardConfig {
        namespace: Arc::from(ns),
        role,
        persist: false,
        cap_refresh: Duration::from_secs(1),
        claim_timeout_secs: 300,
        ..Default::default()
    }
}

fn surplus() -> BTreeMap<String, String> {
    BTreeMap::from([("kind".to_string(), "surplus".to_string()), ("feeder".to_string(), "4".to_string())])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inflight_claim_survives_primary_failover() {
    let (p_lo, p_hi) = alloc_two_sorted_ports();
    let agent_a = start_agent(p_lo, None).await; // primary (lower port)
    let agent_b = start_agent(p_hi, Some(p_lo)).await; // secondary
    wait_peered(&[&agent_a, &agent_b]).await;

    let bb_a = Blackboard::new(Arc::clone(&agent_a), bb_cfg("microgrid", BoardRole::Primary)).await.unwrap();
    let bb_b = Blackboard::new(Arc::clone(&agent_b), bb_cfg("microgrid", BoardRole::Secondary)).await.unwrap();

    // Let the secondary discover the primary + sync.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Post a finite surplus fact; give live replication time to reach the mirror.
    let _id = bb_a.post(surplus(), Bytes::from("3.2 kWh")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    // The primary claims it (in-flight, NOT acked — a Claim is not replicated).
    let pred = Predicate::new().eq("kind", "surplus");
    let claimed = bb_a.claim(&pred).await.unwrap().expect("primary claims the fact");
    assert_eq!(claimed.payload.as_ref(), b"3.2 kWh");

    // Kill the primary. Its capability evaporates; the secondary promotes.
    bb_a.shutdown().await;
    agent_a.shutdown().await;

    // The in-flight claim re-queues on the promoted secondary and is re-claimable.
    let mut recovered = None;
    for _ in 0..120 {
        if let Ok(Some(f)) = bb_b.claim(&pred).await {
            recovered = Some(f);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let f = recovered.expect("G-G3.3: the in-flight claim must survive failover and be re-claimable");
    assert_eq!(f.payload.as_ref(), b"3.2 kWh", "the same finite fact survives under its payload");

    // And acking it on the new primary terminates it (no resurrection).
    bb_b.ack(f.id).await.unwrap();
    assert!(bb_b.claim(&pred).await.unwrap().is_none(), "an acked fact does not re-serve");

    bb_b.shutdown().await;
    agent_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_election_settles_on_one_primary_that_serves() {
    let (p_lo, p_hi) = alloc_two_sorted_ports();
    let agent_lo = start_agent(p_lo, None).await;
    let agent_hi = start_agent(p_hi, Some(p_lo)).await;
    wait_peered(&[&agent_lo, &agent_hi]).await;

    let bb_lo = Blackboard::new(Arc::clone(&agent_lo), bb_cfg("elect", BoardRole::Auto)).await.unwrap();
    let bb_hi = Blackboard::new(Arc::clone(&agent_hi), bb_cfg("elect", BoardRole::Auto)).await.unwrap();

    // After the settle window, exactly one primary exists and serves. *Which* node wins is the
    // ring's negotiated election rule's business and is asserted separately, by the P10 test below
    // — this one is about the ring settling at all.
    let pred = Predicate::new();
    let mut posted = false;
    for _ in 0..120 {
        // A client post resolves the elected primary; once election settles, this succeeds.
        if bb_lo.post(surplus(), Bytes::from("x")).await.is_ok() {
            posted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(posted, "an Auto cluster must elect a primary that serves posts");
    // The elected primary serves the fact to a reader on either node.
    let mut seen = false;
    for _ in 0..40 {
        if bb_hi.read(&pred).await.map(|v| !v.is_empty()).unwrap_or(false) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(seen, "the elected primary serves reads to the other node");

    bb_lo.shutdown().await;
    bb_hi.shutdown().await;
    agent_lo.shutdown().await;
    agent_hi.shutdown().await;
}

/// Audit 2026-07-15 pass 3: a secondary that starts before the primary's advertisement has
/// propagated must NOT promote — "evaporated" means *was there, then gone*; never-seen is startup
/// lag. The pre-fix blackboard watch promoted after two empty resolves with neither a seen-primary
/// gate nor an orphan grace, creating a split-brain primary that never demoted. Ported from the
/// tuple-space guard (`secondary_startup_lag_is_not_evaporation`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secondary_startup_lag_is_not_evaporation() {
    let fast = |ns: &str, role| BoardConfig { cap_refresh: Duration::from_millis(300), ..bb_cfg(ns, role) };
    let (pa, pb) = alloc_two_sorted_ports();
    let a1 = start_agent(pa, None).await;
    let a2 = start_agent(pb, Some(pa)).await;
    wait_peered(&[&a1, &a2]).await;

    // Secondary first, with no primary anywhere on the ring.
    let bb2 = Blackboard::new(Arc::clone(&a2), fast("lag", BoardRole::Secondary)).await.unwrap();

    // Past the pre-fix ~2×cap_refresh promotion point, well inside the 10-tick orphan grace.
    tokio::time::sleep(Duration::from_millis(300 * 4)).await;
    assert!(!bb2.is_primary(), "secondary promoted on startup propagation lag (never saw a primary)");

    // Primary appears late — the secondary must see it and stay secondary.
    let bb1 = Blackboard::new(Arc::clone(&a1), fast("lag", BoardRole::Primary)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300 * 4)).await;
    assert!(!bb2.is_primary(), "secondary promoted despite a live primary");
    assert!(bb2.is_secondary(), "secondary lost its role");

    bb1.shutdown().await;
    bb2.shutdown().await;
    a2.shutdown().await;
    a1.shutdown().await;
}

/// Audit 2026-07-15 pass 3: bounded availability for the genuine orphan — a secondary whose primary
/// NEVER appears (dead before this process started) must still promote once the orphan grace
/// (10 × cap_refresh) expires, not wait forever. Ported from the tuple-space
/// `never_seen_primary_promotes_after_orphan_grace`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_seen_primary_promotes_after_orphan_grace() {
    let fast = |ns: &str, role| BoardConfig { cap_refresh: Duration::from_millis(300), ..bb_cfg(ns, role) };
    let (pa, pb) = alloc_two_sorted_ports();
    let a1 = start_agent(pa, None).await;
    let a2 = start_agent(pb, Some(pa)).await;
    wait_peered(&[&a1, &a2]).await;

    let bb2 = Blackboard::new(Arc::clone(&a2), fast("orph", BoardRole::Secondary)).await.unwrap();

    // Grace ≈ 10 × 300 ms = 3 s; poll well past it.
    let mut promoted = false;
    for _ in 0..40 {
        if bb2.is_primary() { promoted = true; break; }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(promoted, "orphaned secondary never promoted (availability hole)");

    bb2.shutdown().await;
    a2.shutdown().await;
    a1.shutdown().await;
}

/// **The P10 fix, end to end.** Two rings, the same three candidates — and they must not both
/// elect the same node.
///
/// Under the old rule (*lowest candidate node id wins*) this test could not have passed: every
/// ring applies the same ordering to the same candidates, so every ring returns the same winner,
/// and a fleet running several companions puts every single-writer job on one node — a coordinator
/// nobody declared (`docs/design/legible-emergence-taxonomy.md`, P10).
///
/// The namespaces are **chosen at runtime** rather than hard-coded: the test asks
/// `mycelium::election` which two rings this particular set of ports lands on different nodes, then
/// runs those two. That keeps the assertion exact and deterministic — it compares against the
/// winner the rule names, not against a hope — where a fixed pair of names would be a coin flip on
/// whatever ports the allocator handed out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_rings_over_the_same_candidates_do_not_elect_the_same_node() {
    use mycelium::capability::CapFilter;
    use mycelium::election::{winner, Rule};

    let ports: Vec<u16> = (0..3).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();

    // Find two rings this candidate set splits between different nodes.
    let ring_of = |ns: &str| format!("blackboard/{ns}.primary");
    let mut pair: Option<(String, String, NodeId, NodeId)> = None;
    'outer: for i in 0..40 {
        for j in (i + 1)..40 {
            let (a, b) = (format!("ring-{i}"), format!("ring-{j}"));
            let wa = winner(&ring_of(&a), &ids, Rule::Rendezvous).unwrap().clone();
            let wb = winner(&ring_of(&b), &ids, Rule::Rendezvous).unwrap().clone();
            if wa != wb {
                pair = Some((a, b, wa, wb));
                break 'outer;
            }
        }
    }
    let (ns_a, ns_b, want_a, want_b) = pair.expect("some two rings must split three candidates");

    let agents: Vec<Arc<GossipAgent>> = {
        let mut v = Vec::new();
        for (i, p) in ports.iter().enumerate() {
            v.push(start_agent(*p, if i == 0 { None } else { Some(ports[0]) }).await);
        }
        v
    };
    wait_peered(&agents.iter().collect::<Vec<_>>()).await;

    // Every node is a candidate in BOTH rings — the homogeneous fleet that concentrates.
    let mut boards = Vec::new();
    for agent in &agents {
        for ns in [&ns_a, &ns_b] {
            boards.push(Blackboard::new(Arc::clone(agent), bb_cfg(ns, BoardRole::Auto)).await.unwrap());
        }
    }

    // Poll until both rings have settled on exactly one primary each.
    let primary_of = |ns: &str| -> Option<NodeId> {
        let f = CapFilter::new("blackboard", format!("{ns}.primary"));
        let found = agents[0].capabilities().resolve(&f);
        (found.len() == 1).then(|| found[0].0.clone())
    };
    let mut got: Option<(NodeId, NodeId)> = None;
    for _ in 0..160 {
        if let (Some(a), Some(b)) = (primary_of(&ns_a), primary_of(&ns_b)) {
            got = Some((a, b));
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (got_a, got_b) = got.expect("both rings must elect a primary");

    assert_eq!(got_a, want_a, "ring {ns_a} must land where the rendezvous rule says");
    assert_eq!(got_b, want_b, "ring {ns_b} must land where the rendezvous rule says");
    assert_ne!(got_a, got_b, "two rings over one candidate set must not hand both jobs to one node");

    for b in boards.drain(..) {
        b.shutdown().await;
    }
    for a in agents {
        a.shutdown().await;
    }
}

// ── ER6 (R11-01, R11-04): a promoted board does not bring back, or reuse, what was completed ──

fn temp_wal(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("mbb-failover-{}-{name}.wal", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn persist_cfg(ns: &str, role: BoardRole, wal: &std::path::Path) -> BoardConfig {
    BoardConfig {
        persist: true,
        wal_path: wal.to_path_buf(),
        checkpoint_every: 1,
        cap_refresh: Duration::from_millis(500),
        ..bb_cfg(ns, role)
    }
}

fn item(label: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("item".to_string(), label.to_string())])
}

async fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The marker reaches this WAL only through the mirror's own append (a replicated or snapshot `Post`).
fn wal_holds(wal: &std::path::Path, marker: &str) -> bool {
    std::fs::read(wal).is_ok_and(|b| b.windows(marker.len()).any(|w| w == marker.as_bytes()))
}

async fn claim_and_ack(bb: &Arc<Blackboard>, label: &str) -> u64 {
    let f = bb.claim(&Predicate::new().eq("item", label)).await.unwrap().expect("claimable");
    bb.ack(f.id).await.unwrap();
    f.id
}

/// R11-01: a persistent mirror holds fact R, goes away; the primary acks R and posts M; the mirror
/// returns from its own WAL, syncs (M arrives), and promotes. R was completed: it must not be
/// claimable on the new primary, and M must be.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_returning_mirror_retires_facts_completed_during_its_absence() {
    let (p_lo, p_hi) = alloc_two_sorted_ports();
    let a = start_agent(p_lo, None).await;
    let b = start_agent(p_hi, Some(p_lo)).await;
    wait_peered(&[&a, &b]).await;
    let (pwal, mwal) = (temp_wal("r1101-primary"), temp_wal("r1101-mirror"));
    let primary = Blackboard::new(Arc::clone(&a), persist_cfg("r1101", BoardRole::Primary, &pwal)).await.unwrap();
    let retired = primary.post(item("done-while-away"), Bytes::from("done-while-away")).await.unwrap();

    let mirror = Blackboard::new(Arc::clone(&b), persist_cfg("r1101", BoardRole::Secondary, &mwal)).await.unwrap();
    wait_for("the fact in the mirror's WAL", || wal_holds(&mwal, "done-while-away")).await;
    mirror.shutdown().await;
    drop(mirror);
    let secondaries = mycelium::CapFilter::new("blackboard", "r1101.secondary");
    wait_for("the primary sees no secondary", || a.capabilities().resolve(&secondaries).is_empty()).await;

    assert_eq!(claim_and_ack(&primary, "done-while-away").await, retired);
    let marker = primary.post(item("posted-while-away"), Bytes::from("posted-while-away")).await.unwrap();

    let returning = Blackboard::new(Arc::clone(&b), persist_cfg("r1101", BoardRole::Secondary, &mwal)).await.unwrap();
    wait_for("the snapshot in the returning mirror's WAL", || wal_holds(&mwal, "posted-while-away")).await;
    assert!(!returning.is_primary());
    primary.shutdown().await;
    a.shutdown().await;
    wait_for("the returning mirror promotes", || returning.is_primary()).await;

    let live: Vec<u64> = returning.read(&Predicate::new()).await.unwrap().iter().map(|f| f.id).collect();
    assert!(live.contains(&marker), "the fact posted while the mirror was away survives: {live:?}");
    let again = returning.claim(&Predicate::new().eq("item", "done-while-away")).await.unwrap();
    assert!(again.is_none(), "fact {retired} was acked on the old primary and is claimable again: {again:?}");

    returning.shutdown().await;
    b.shutdown().await;
    drop(returning);
    let _ = std::fs::remove_file(&pwal);
    let _ = std::fs::remove_file(&mwal);
}

/// R11-04: a mirror whose store's id space is older than the primary's (ER3 masks this only when the
/// mirror's store is the younger) syncs one live fact after the primary acked two higher ids, then
/// promotes. Its next id must be above every id the primary issued — and stay so across a reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_mirror_preserves_the_highest_allocated_fact_id_on_promotion() {
    let (p_lo, p_hi) = alloc_two_sorted_ports();
    let a = start_agent(p_lo, None).await;
    let b = start_agent(p_hi, Some(p_lo)).await;
    wait_peered(&[&a, &b]).await;
    let (pwal, mwal) = (temp_wal("r1104-primary"), temp_wal("r1104-mirror"));
    // The mirror node's store has history from before the primary's store was created.
    {
        let old = mycelium_blackboard::BoardStore::persistent(&mwal, 1).unwrap();
        let id = old.post(item("older-life"), Bytes::new()).unwrap();
        old.claim(&Predicate::new()).unwrap();
        old.ack(id).unwrap();
    }
    let primary = Blackboard::new(Arc::clone(&a), persist_cfg("r1104", BoardRole::Primary, &pwal)).await.unwrap();
    let marker = primary.post(item("kept"), Bytes::from("kept")).await.unwrap();
    let _first = primary.post(item("first"), Bytes::from("first")).await.unwrap();
    let _last = primary.post(item("last"), Bytes::from("last")).await.unwrap();
    claim_and_ack(&primary, "first").await;
    let last = claim_and_ack(&primary, "last").await;

    let mirror = Blackboard::new(Arc::clone(&b), persist_cfg("r1104", BoardRole::Secondary, &mwal)).await.unwrap();
    wait_for("the live fact in the mirror's WAL", || wal_holds(&mwal, "kept")).await;
    primary.shutdown().await;
    a.shutdown().await;
    wait_for("the mirror promotes", || mirror.is_primary()).await;
    let ids: Vec<u64> = mirror.read(&Predicate::new().eq("item", "kept")).await.unwrap().iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![marker]);
    let next = mirror.post(item("new-after-promotion"), Bytes::new()).await.unwrap();
    assert!(next > last, "the promoted board issued {next}, at or below {last}, an id the old primary acked");

    mirror.shutdown().await;
    b.shutdown().await;
    drop(mirror);
    let mut reopened = None;
    for _ in 0..40 {
        if let Ok(s) = mycelium_blackboard::BoardStore::persistent(&mwal, 1) {
            reopened = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let reopened = reopened.expect("the promoted board's WAL reopens");
    let after = reopened.post(item("after-reopen"), Bytes::new()).unwrap();
    assert!(after > next, "the reopened board issued {after}, at or below {next}");
    drop(reopened);
    let _ = std::fs::remove_file(&pwal);
    let _ = std::fs::remove_file(&mwal);
}
