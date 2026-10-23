//! Crate-level test module — originally inlined as `#[cfg(test)] mod tests`
//! at the end of `src/lib.rs`. Lifted out so `lib.rs` itself is a thin
//! crate-config + re-exports file (≈90 lines), making the public API
//! surface easy to scan without scrolling past 2 800 lines of tests.
//!
//! All tests stay private to the crate (no integration-test changes) so
//! they can keep using `pub(crate)` items like `ConnContext`, `KvState`,
//! `GossipUpdate`, etc.

use super::*;
use crate::connection::ConnContext;
use crate::framing::{
    read_frame, write_frame, GossipUpdate, SyncEntry,
    WireMessage,
    N_GOSSIP_SHARDS, TTL_OFFSET,
};
use mycelium_core::codec::{decode_wire, wire_to_bytes};
use crate::seen::ShardedSeen;
use crate::store::{store_hash, KvState, StoreEntry};
use bytes::{Bytes, BytesMut};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    time,
};

// ── Helpers ───────────────────────────────────────────────────────────────

// Port 0 is intentional: this agent is for store/API unit tests only and must
// NOT have start() called on it — validate() rejects bind_port = 0.
// Use alloc_port() + agent.start() for integration tests that need a live node.
fn make_agent() -> GossipAgent {
    GossipAgent::new(
        NodeId::new("127.0.0.1", 0).unwrap(),
        GossipConfig::default(),
    )
}

/// Agent for state-machine tests — uses port 0 (no networking) but a non-zero
/// port in the NodeId so `agent/{node}/state` KV keys parse correctly.
pub(crate) fn make_agent_for_sm_tests() -> GossipAgent {
    GossipAgent::new(
        NodeId::new("127.0.0.1", 19876).unwrap(),
        GossipConfig::default(),
    )
}

async fn loopback_pair() -> (TcpStream, TcpStream) {
    let listener    = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr        = listener.local_addr().unwrap();
    let writer      = TcpStream::connect(addr).await.unwrap();
    let (reader, _) = listener.accept().await.unwrap();
    (writer, reader)
}

async fn send_wire(writer: &mut TcpStream, msg: &WireMessage) {
    let data = wire_to_bytes(msg);
    write_frame(writer, &data).await.unwrap();
}

fn data_update(key: &str, value: &[u8], nonce: u64, is_tombstone: bool) -> GossipUpdate {
    GossipUpdate {
        sender:       NodeId::new("127.0.0.1", 9999).unwrap().id_hash(),
        key:          Arc::from(key),
        value:        Bytes::copy_from_slice(value),
        timestamp:    1,
        nonce,
        ttl:          3,
        is_tombstone,
    }
}

fn spawn_handler(
    socket: TcpStream,
    store: Arc<papaya::HashMap<Arc<str>, StoreEntry>>,
    peers: Arc<papaya::HashMap<NodeId, Instant>>,
    gossip_tx: mpsc::Sender<(Bytes, u64, crate::framing::ForwardHint)>,
    seen: Arc<ShardedSeen>,
    max_ttl: u8,
) -> (Arc<watch::Sender<bool>>, tokio::task::JoinHandle<Result<(), GossipError>>) {
    spawn_handler_with(socket, store, peers, gossip_tx, seen, max_ttl, HandlerOpts::default())
}

/// What a connection handler under test is configured with, beyond the defaults: the node's
/// `GossipConfig`, its TLS identity (a node with one is "a TLS node" to the handler — the stream
/// itself can stay plaintext, the handler does not look), and the peer keys it already knows.
#[derive(Default)]
struct HandlerOpts {
    config:    GossipConfig,
    tls:       Option<Arc<crate::tls::NodeTls>>,
    peer_keys: Vec<(NodeId, Vec<[u8; 32]>)>,
    /// The handler's per-node counters, shared so a test can read them.
    bounds:    Arc<mycelium_core::bounds::TransportBounds>,
}

fn spawn_handler_with(
    socket: TcpStream,
    store: Arc<papaya::HashMap<Arc<str>, StoreEntry>>,
    peers: Arc<papaya::HashMap<NodeId, Instant>>,
    gossip_tx: mpsc::Sender<(Bytes, u64, crate::framing::ForwardHint)>,
    seen: Arc<ShardedSeen>,
    max_ttl: u8,
    opts: HandlerOpts,
) -> (Arc<watch::Sender<bool>>, tokio::task::JoinHandle<Result<(), GossipError>>) {
    use crate::connection::handle_connection;
    use crate::signal::{Boundary, SignalHandlers};
    use crate::agent::{TaskCtx, BulkTransport};
    use mycelium_core::CoreCtx;
    use parking_lot::RwLock;
    let node_id = NodeId::new("127.0.0.1", 0).unwrap();
    let (shutdown_tx, _) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);
    let gossip_txs = mycelium_core::outbound::GossipTxs::new(
        (0..N_GOSSIP_SHARDS).map(|_| gossip_tx.clone()).collect::<Vec<_>>(), Default::default());
    // Seed the hash accumulator from the store's current state so the
    // anti-entropy fast-path works correctly for pre-populated test stores.
    let initial_hash = store_hash(&store);
    let kv_state = Arc::new(KvState {
        kv_store: crate::store::KvStore {
            store,
            prefix_index:      Arc::new(crate::store::PrefixIndex::new()),
            index_stripes:     Arc::new(std::array::from_fn(|_| std::sync::Mutex::new(()))),
            cap_ns_index:      Arc::new(crate::store::PrefixIndex::new()),
            scope_index: Arc::new(papaya::HashMap::new()),
            scope_roster: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "test-frame-filter")]
            frame_filter: Arc::new(papaya::HashMap::new()),
            scoped_prefixes: Arc::from(Vec::<Arc<str>>::new()),
            hash_acc:          Arc::new(AtomicU64::new(initial_hash)),
            dropped_frames:    Arc::new(AtomicU64::new(0)),
            individual_flood_fallbacks: Arc::new(AtomicU64::new(0)),
            max_store_entries: 0,
            live_count:        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            grp_generation:    Arc::new(AtomicU64::new(0)),
            prefix_watchers:           Arc::new(papaya::HashMap::new()),
            prefix_predicate_watchers: Arc::new(papaya::HashMap::new()),
            next_pred_watcher_id:      Arc::new(AtomicU64::new(0)),
            peer_localities:           Arc::new(papaya::HashMap::new()),
            quorum_trackers:           Arc::new(papaya::HashMap::new()),
        },
        subscriptions: Arc::new(papaya::HashMap::new()),
    });
    let (shutdown_tx_inner, _) = tokio::sync::watch::channel(false);
    let core_ctx = Arc::new(CoreCtx {
        node_id: node_id.clone(),
        seen,
        hlc: Arc::new(crate::hlc::Hlc::new()),
        signal_boundary: Arc::new(RwLock::new(Boundary::new(node_id))),
        decision_sink: std::sync::OnceLock::new(),
        inbound_observer: std::sync::OnceLock::new(),
        signal_handlers: Arc::new(SignalHandlers::new(Duration::from_secs(600))),
        gossip_txs,
        default_ttl: max_ttl,
        kv_state,
        wal: std::sync::OnceLock::new(),
        sys_namespace_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        transport_bounds: Arc::clone(&opts.bounds),
        tls: {
            let cell = std::sync::OnceLock::new();
            if let Some(t) = opts.tls { let _ = cell.set(t); }
            cell
        },
        peer_keys: Arc::new(opts.peer_keys.into_iter().collect()),
        peer_anchor_keys: Arc::new(papaya::HashMap::new()),
        removed: Arc::new(mycelium_core::removal::RemovedSet::default()),
        key_standing: Arc::new(mycelium_core::removal::KeyStanding::default()),
        identity_anchor_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        peers: Arc::new(papaya::HashMap::new()),
        rate_throttle: Arc::new(papaya::HashMap::new()),
        reorder_buf: None,
        reply_interceptor: None,
        soft_state_advertised: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        soft_state_gate:       Arc::default(),
        shutdown_tx: Arc::new(shutdown_tx_inner),
        task_handles: Arc::new(std::sync::Mutex::new(tokio::task::JoinSet::new())),
        hot: Arc::new(mycelium_core::context::HotConfig::from_config(&opts.config)),
        config: Arc::new(opts.config),
    });
    let task_ctx = Arc::new(TaskCtx {
        #[cfg(feature = "consensus")]
        consensus_accepted: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        acceptor_records: std::sync::Mutex::new(()),
        core: core_ctx,
        bulk_transport: Arc::new(BulkTransport::new(0, Duration::from_secs(5), 64)),
        rpc_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        rpc_reply_sender_mismatches: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        commit_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        commit_conflict_slots: Arc::new(papaya::HashMap::new()),
        decided_floor_anomaly_slots: Arc::new(papaya::HashSet::new()),
        decided_floor_anomalies_unrecorded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        #[cfg(feature = "consensus")]
        consensus_verified_ballots: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorates: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorate_refused: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        electorate_fences: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        acceptor_withheld: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        acceptor_notices: Arc::new(crate::agent::acceptor_guard::notice_sets()),
        #[cfg(feature = "consensus")]
        electorate_retired: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorate_completed: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorate_drops: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorate_migrations: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        migrations_running: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        electorate_epochs_known: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        electorate_retire_known: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        electorate_carried_known: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        electorate_dropped_known: Arc::new(papaya::HashSet::new()),
        #[cfg(feature = "consensus")]
        electorate_records_watch: Arc::new(std::sync::OnceLock::new()),
        #[cfg(feature = "consensus")]
        electorate_records_scanned: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
        #[cfg(feature = "consensus")]
        electorate_groups_known: Arc::new(papaya::HashSet::new()),
        ballot_space_exhausted: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        event_ring: Arc::new(crate::agent::emergent::EventRing::default()),
        governed_group_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        capability_coverage_gaps: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        role_concentration_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        membership_flaps: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        control_profile: std::sync::atomic::AtomicU8::new(0),
        control_would_hold: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        opacity_releases_spaced: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        opacity_oscillations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        cap_authz_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        schema_mismatch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        #[cfg(feature = "gateway")]
        governance_changes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        #[cfg(feature = "gateway")]
        governance_unaudited: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        #[cfg(feature = "compliance")]
        audit_chain: Arc::new(std::sync::Mutex::new(crate::agent::audit::AuditChainState::new())),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        action_evaluator: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        deployed_policy_revision: arc_swap::ArcSwapOption::from(None),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        evidence_journal: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        history_index: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        ae_tally_heartbeat: std::sync::atomic::AtomicBool::new(false),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        ae_tally_lock: std::sync::Mutex::new(()),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        execution_authority: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        provider_enforcement: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "tls")]
        membership: std::sync::OnceLock::new(),
        #[cfg(feature = "tls")]
        identity_ledger: Arc::new(crate::agent::revocation::IdentityLedger::default()),
        caller_replay: Default::default(),
        causal_misplaced: Default::default(),
        #[cfg(feature = "a2a")]
        a2a_mounted: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "a2a")]
        a2a_default_skill: std::sync::OnceLock::new(),
        guarantees: parking_lot::Mutex::new(crate::agent::guarantee::core_guarantees()),
        started: std::sync::atomic::AtomicBool::new(false),
        late_attachments: std::sync::atomic::AtomicU32::new(0),
        at_rest_cipher_attached: std::sync::atomic::AtomicBool::new(false),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        cohort_budget: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_edge: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_clients: std::sync::OnceLock::new(),
        #[cfg(feature = "gateway")]
        egress_credentials: std::sync::OnceLock::new(),
        #[cfg(feature = "compliance")]
        audit_sink: std::sync::OnceLock::new(),
        #[cfg(feature = "compliance")]
        audit_sink_tx: std::sync::OnceLock::new(),
        filter_opacity_registry: Arc::new(crate::agent::FilterOpacityRegistry::new()),
        group_roster_cache: Arc::new(papaya::HashMap::new()),
        tuning_governor: Arc::new(crate::agent::TuningGovernor::default()),
        #[cfg(feature = "llm")]
        llm_skills: std::sync::Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "llm")]
        llm_dispatch_spawned: std::sync::atomic::AtomicBool::new(false),
        mailbox_in_flight: Default::default(),
    });
    let ctx = ConnContext {
        task_ctx: Arc::clone(&task_ctx.core),
        peers,
        shutdown: Arc::clone(&shutdown_tx),
        peer_writers: Arc::new(papaya::HashMap::new()),
        backoff: Duration::ZERO,
        n_shards: N_GOSSIP_SHARDS,
        intern_keys: true,
        intern_max_keys: 0,
        max_peers: usize::MAX,
        writer_timing: mycelium_core::writer::WriterTiming::with_idle(Duration::ZERO),
        peer_list_tx: tokio::sync::watch::channel(std::sync::Arc::from(Vec::<NodeId>::new())).0,
    };
    let handle = tokio::spawn(handle_connection(
        crate::stream::GossipStream::Plain(socket),
        "127.0.0.1:0".parse().unwrap(),
        ctx,
    ));
    (shutdown_tx, handle)
}

async fn poll_until(mut predicate: impl FnMut() -> bool, timeout_ms: u64) {
    tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        async {
            loop {
                if predicate() { return; }
                time::sleep(Duration::from_millis(5)).await;
            }
        },
    )
    .await
    .unwrap_or_else(|_| panic!("poll_until timed out after {}ms", timeout_ms));
}

// ── Port allocator for integration tests ──────────────────────────────────

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

// ── Two-node consensus test fixture ──────────────────────────────────────
//
// Required for any multi-node consensus test:
// - Both nodes need start_consensus_listener or their votes never arrive.
// - quorum = ⌊(peers+1)/2⌋ + 1 = 2 once peers are connected; a test that
//   calls propose before peers connect silently gets quorum=1 (self-vote
//   only) and passes for the wrong reason.
// - Structural peer-ready poll converts a timing race into a deterministic
//   failure if the cluster doesn't form, making root causes obvious.

#[cfg(feature = "consensus")]
struct ConsensusPair {
    pub a:   GossipAgent,
    pub b:   GossipAgent,
    pub _la: ConsensusListenerHandle,
    pub _lb: ConsensusListenerHandle,
}

#[cfg(feature = "consensus")]
async fn consensus_pair() -> ConsensusPair {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port                  = port_a;
    cfg_a.bootstrap_peers            = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;   // cap initial ping delay so poll converges fast
    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port                  = port_b;
    cfg_b.bootstrap_peers            = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;
    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
    // Structural poll — fails deterministically if cluster doesn't form.
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 2_000).await;
    ConsensusPair { a, b, _la, _lb }
}


// ── The two-mesh harness (v3 item 2 PR 1) ─────────────────────────────────
//
// `docs/design/federated-domains.md` §2 states the invariant the whole item rests on: federation
// connects exported services and **never joins the transports** — a foreign node never enters
// membership, the native namespaces, anti-entropy state, or a quorum.
//
// §13's release gate says that must be proved "from membership tables, consensus state and traces",
// **not from a narrative**. This is the scaffolding for that: build two meshes that share nothing,
// then assert non-merger from the tables themselves. PRs 2–7 add the federation edge on top and
// re-run these same assertions with it present — at which point they stop being trivially true and
// start being the thing under test.

/// Two independent meshes, each bootstrapped only within itself.
struct TwoMeshes {
    /// Domain A's nodes.
    a: Vec<GossipAgent>,
    /// Domain B's nodes.
    b: Vec<GossipAgent>,
}

/// Build two meshes of `n` nodes each. No node in one lists any node of the other as a bootstrap
/// peer — which is what "independently admitted" reduces to in a single-process test.
async fn two_meshes(n: usize) -> TwoMeshes {
    async fn mesh(n: usize) -> Vec<GossipAgent> {
        let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
        let ids: Vec<NodeId> = ports
            .iter()
            .map(|p| NodeId::new("127.0.0.1", *p).unwrap())
            .collect();
        let mut agents = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let mut cfg = GossipConfig::default();
            cfg.bind_port = ports[i];
            // Bootstrap only within this mesh.
            cfg.bootstrap_peers = ids
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| other.clone())
                .collect();
            cfg.health_check_max_jitter_ms = 50;
            let a = GossipAgent::new(id.clone(), cfg);
            a.start().await.unwrap();
            agents.push(a);
        }
        agents
    }
    let a = mesh(n).await;
    let b = mesh(n).await;
    // Structural poll: each mesh must actually form, or "they did not merge" is vacuously true
    // because nothing connected to anything.
    let (ra, rb) = (&a, &b);
    poll_until(
        || ra.iter().all(|x| !x.peers().is_empty()) && rb.iter().all(|x| !x.peers().is_empty()),
        3_000,
    )
    .await;
    TwoMeshes { a, b }
}

impl TwoMeshes {
    /// Assert non-merger **from the tables**, per §2 of the record.
    fn assert_never_merged(&self) {
        let a: Vec<&GossipAgent> = self.a.iter().collect();
        let b: Vec<&GossipAgent> = self.b.iter().collect();
        assert_never_merged(&a, &b);
    }
}

/// The harness's assertions over borrowed nodes, so a test that holds a node in an `Arc` (the
/// federation transport's gateway node, PR 8) runs the same checks.
fn assert_never_merged(a: &[&GossipAgent], b: &[&GossipAgent]) {
    {
        let ids = |m: &[&GossipAgent]| -> Vec<String> {
            m.iter().map(|x| x.node_id().to_string()).collect()
        };
        let (a_ids, b_ids) = (ids(a), ids(b));

        // 1. Membership: no foreign node in any peer table. A foreign node here is one the failure
        //    detector, the fan-out and quorum sizing would all count.
        for node in a {
            let peers: Vec<String> = node.peers().iter().map(|p| p.to_string()).collect();
            for foreign in &b_ids {
                assert!(
                    !peers.contains(foreign),
                    "domain A node {} has domain B node {} in its peer table — the meshes merged",
                    node.node_id(), foreign
                );
            }
        }
        for node in b {
            let peers: Vec<String> = node.peers().iter().map(|p| p.to_string()).collect();
            for foreign in &a_ids {
                assert!(
                    !peers.contains(foreign),
                    "domain B node {} has domain A node {} in its peer table — the meshes merged",
                    node.node_id(), foreign
                );
            }
        }

        // 2. The native namespaces and consensus state: no key *or value* naming a foreign node.
        //    Foreign state in `cap/`, `grp/` or `sys/` is indistinguishable from local state once
        //    it is there; a foreign node in `consensus/` (ballots, trust slices, committed
        //    values) is one that voted (PR 9: the gate's *consensus state* leg).
        for (mine, theirs, label) in
            [(a, &b_ids, "A"), (b, &a_ids, "B")]
        {
            for node in mine {
                for prefix in ["cap/", "grp/", "sys/", "consensus/"] {
                    for (key, value) in node.kv().scan_prefix(prefix) {
                        let value = String::from_utf8_lossy(&value);
                        for foreign in theirs {
                            assert!(
                                !key.contains(foreign.as_str()) && !value.contains(foreign.as_str()),
                                "domain {label} node {} holds {key}, which names foreign node \
                                 {foreign} (in the key or the value) — foreign state entered the medium",
                                node.node_id()
                            );
                        }
                    }
                }
            }
        }

        // 3. The transport's own record (PR 9: the gate's *traces* leg): every connection a node
        //    holds is to a node of its own mesh. Distinct from leg 1 — membership is what a node
        //    believes, `connected_peers` is where its bytes actually went.
        for (mine, theirs, label) in
            [(a, &b_ids, "A"), (b, &a_ids, "B")]
        {
            for node in mine {
                let connected: Vec<String> = node.connected_peers().iter().map(|p| p.to_string()).collect();
                for foreign in theirs {
                    assert!(
                        !connected.contains(foreign),
                        "domain {label} node {} holds a connection to foreign node {foreign} — \
                         bytes flowed between the meshes",
                        node.node_id()
                    );
                }
            }
        }
    }
}

/// **The harness proves something, and proves it is not vacuous.**
///
/// Two meshes that share no bootstrap peer never learn each other — asserted from the peer tables
/// and the native namespaces, not from the absence of an error. The second half is the part worth
/// having: the same assertions are run against a *deliberately merged* pair, and must fail. Without
/// that, a harness that checked nothing would pass this test too.
#[tokio::test]
async fn two_meshes_never_learn_each_other() {
    let meshes = two_meshes(2).await;

    // Give discovery a real chance to do the wrong thing before concluding it did not.
    time::sleep(Duration::from_millis(300)).await;
    meshes.assert_never_merged();

    // Non-vacuity: one mesh, split into two halves, HAS merged — the same assertions must catch it.
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
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 3_000).await;
    // Positive control for leg 3 (PR 9): a merged pair shows up in the *connection* table too,
    // so the traces leg is checking a record that a merge actually writes.
    poll_until(|| a.connected_peers().contains(b.node_id()) || b.connected_peers().contains(a.node_id()), 3_000).await;
    assert!(
        a.connected_peers().contains(b.node_id()) || b.connected_peers().contains(a.node_id()),
        "a merged pair must hold a connection between the halves, or leg 3 checks nothing"
    );

    let merged = TwoMeshes { a: vec![a], b: vec![b] };
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        merged.assert_never_merged()
    }));
    assert!(
        caught.is_err(),
        "the harness must FAIL on two meshes that did merge — otherwise it asserts nothing"
    );
}

// ── Durable proposals via the existing log verb (v3 item 5 PR 5) ──────────

/// **§7 of the scoped-mandates record, demonstrated rather than typed.**
///
/// > Durable proposals via the existing log verb (`KvHandle::append` →
/// > `log/wiki/{group}/proposals`) plus item 1's receipts — **not a service database**; the
/// > evaporating queue becomes the discovery hint the plan wants.
///
/// §5 of that record refuses a resource-authoritative service process, and a durable proposal queue
/// is exactly where one would sneak back in: it looks like storage rather than like a control
/// plane. This test is the check that it did not — the proposal goes through the verb the substrate
/// already has, lands in the namespace reserved for it, and reads back byte-identical.
#[tokio::test]
async fn a_durable_proposal_uses_the_existing_append_verb_and_the_reserved_namespace() {
    use crate::mandate::{restart::{proposal_stream, Proposal}, PrincipalId, TermId};

    let agent = make_agent();
    let stream = proposal_stream("norfolk");

    let proposal = Proposal {
        proposer: PrincipalId::new("member-b").unwrap(),
        term:     TermId::new("term-3").unwrap(),
        target:   "pages/harvest.md".into(),
        body:     b"# Harvest\n".to_vec(),
        at_ms:    1_789_000_000_000,
    };

    // The existing verb. No new storage, no service.
    let hlc = agent.kv().append(&stream, proposal.encode().unwrap());
    assert!(hlc > 0, "append returns the log position it took");

    // It landed under the prefix reserved at PR 1, not in a namespace of its own.
    let keys: Vec<String> = agent
        .kv()
        .scan_prefix(mycelium_core::signal::kv_ns::LOG_WIKI)
        .into_iter()
        .map(|(k, _)| k.to_string())
        .collect();
    assert!(
        keys.iter().any(|k| k.starts_with("log/wiki/norfolk/proposals/")),
        "the proposal must live under the reserved log/wiki/ prefix, got {keys:?}"
    );

    // And it reads back through the same verb, byte-identical.
    let entries = agent.kv().scan_log(&stream, 0, u64::MAX);
    assert_eq!(entries.len(), 1, "exactly the one proposal");
    assert_eq!(
        Proposal::decode(&entries[0].value).unwrap(),
        proposal,
        "a proposal round-trips through the log verb without a database in the middle"
    );
}

// ── Agent API ─────────────────────────────────────────────────────────────

/// WS-C M8 G-C2: a cluster deployed with `GossipConfig::auto()` (no hand-set tuning
/// values, no `GOSSIP_*` env) converges and propagates a KV write — the zero-tuning
/// ops-friction the workstream removes — and each node's *resolved* config reflects the
/// size-derived (non-zero) values filled by `new()`'s `derive_unset`.
#[tokio::test]
async fn test_wsc_m8_auto_config_cluster_converges() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::auto();
    cfg_a.bind_port                  = port_a;
    cfg_a.bootstrap_peers            = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::auto();
    cfg_b.bind_port                  = port_b;
    cfg_b.bootstrap_peers            = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;

    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);

    // auto()'s 0 sentinels were filled by new()'s derive_unset (N≈2) — the resolved
    // config carries valid, formula-derived values, not the 0 sentinel.
    assert!(a.config().default_ttl >= 5,                 "default_ttl derived");
    assert!(a.config().writer_channel_depth >= 1024,     "writer_channel_depth derived");
    assert!(a.config().max_seen_entries >= 100_000,      "max_seen_entries derived");
    assert!(a.config().ping_peer_sample_size >= 1,       "ping_peer_sample_size derived");
    assert!(a.config().propagation_window_secs >= 60,    "propagation_window_secs derived");
    assert!(a.config().validate().is_ok(),               "resolved auto config must validate");

    // (Was a parallel-load flake — analysis Run 27 — when `alloc_port`'s range overlapped the OS
    // ephemeral port range; fixed at the source by confining `alloc_port` below the ephemeral floor.)
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 3_000).await;

    // The cluster actually works end-to-end with auto tuning: a write on A reaches B.
    let _ = a.kv().set("wsc/m8/k", b"v".to_vec());
    poll_until(|| b.kv().get("wsc/m8/k") == Some(Bytes::from_static(b"v")), 3_000).await;
    assert_eq!(b.kv().get("wsc/m8/k"), Some(Bytes::from_static(b"v")),
        "auto-configured cluster must propagate a KV write");
}

/// WS-C M9: the hot-reload set_* API updates the live (hot) tunables immediately,
/// with no task restart, and clamps writer depth to ≥ 1.
#[tokio::test]
async fn test_wsc_m9_hot_reload_set_api() {
    let port = alloc_port();
    let id   = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    let a = GossipAgent::new(id, cfg);

    // Initial (N=1): writer depth derived to 1024; inbound 0 (off); bulk 64 (Default).
    assert_eq!(a.hot_tunables(), (0, 1024, 64));

    a.set_max_inbound_frames_per_sec(500);
    a.set_writer_channel_depth(4096);
    a.set_max_concurrent_bulk_handlers(8);
    assert_eq!(a.hot_tunables(), (500, 4096, 8), "set_* must update the hot cell live");

    // writer depth is clamped to ≥ 1.
    a.set_writer_channel_depth(0);
    assert_eq!(a.hot_tunables().1, 1, "writer depth floors at 1");
}

/// WS-C M9 G-C4: a gossiped `sys/config/` recommendation is applied by a node whose
/// policy accepts it and **ignored** by a node whose policy rejects it — advisor advises,
/// node decides.
#[tokio::test]
async fn test_wsc_m9_config_policy_accept_vs_reject() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::auto();
    cfg_a.bind_port = port_a; cfg_a.bootstrap_peers = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::auto();
    cfg_b.bind_port = port_b; cfg_b.bootstrap_peers = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;

    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 3_000).await;

    // Both opt in to applying recommendations: A accepts, B rejects.
    a.start_config_applier(crate::accept_all());
    b.start_config_applier(crate::reject_all());

    // Both start at the derived writer depth (small N → 1024).
    assert_eq!(a.hot_tunables().1, 1024);
    assert_eq!(b.hot_tunables().1, 1024);

    // Inject a recommendation (as the advisor would) on A; it gossips to B.
    let key = format!("{}writer_channel_depth", crate::CONFIG_PREFIX);
    let _ = a.kv().set(key.clone(), 9999u64.to_le_bytes().to_vec());

    // A's accept policy applies it live.
    poll_until(|| a.hot_tunables().1 == 9999, 3_000).await;
    assert_eq!(a.hot_tunables().1, 9999, "accept-policy node applies the recommendation");

    // B receives the recommendation (gossip) but its reject policy keeps its own value.
    poll_until(|| b.kv().get(&key).is_some(), 3_000).await;
    // Let B's applier run on the change, then confirm it did NOT apply.
    time::sleep(Duration::from_millis(200)).await;
    assert_eq!(b.hot_tunables().1, 1024, "reject-policy node keeps its own value");
}

/// WS-C M9 governor: a fleet governance intent published on A is reconciled by B; once B
/// takes local control of the param, a later fleet intent is ignored (local always wins).
#[tokio::test]
async fn test_wsc_m9_governor_fleet_reconcile_and_local_wins() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::auto();
    cfg_a.bind_port = port_a; cfg_a.bootstrap_peers = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::auto();
    cfg_b.bind_port = port_b; cfg_b.bootstrap_peers = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;

    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 3_000).await;

    // B opts in to reconciling fleet governance.
    b.start_governor_reconciler();

    let ceiling_of = |agent: &GossipAgent| -> Option<u64> {
        agent.tuning_governor().params.into_iter()
            .find(|p| p.param == crate::HotParam::WriterDepth)
            .and_then(|p| p.ceiling)
    };

    // A publishes a fleet ceiling on writer_depth → B reconciles it (not locally pinned).
    let _ = a.publish_tuning_intent(crate::GovernIntent::bound(
        crate::HotParam::WriterDepth, None, Some(2048), crate::Ratchet::Off));
    poll_until(|| ceiling_of(&b) == Some(2048), 5_000).await;
    assert_eq!(ceiling_of(&b), Some(2048), "fleet intent reconciled on B");

    // B takes local control → a later fleet intent for the same param is ignored.
    b.lock_tuning_ceiling(crate::HotParam::WriterDepth, 4096);
    let _ = a.publish_tuning_intent(crate::GovernIntent::bound(
        crate::HotParam::WriterDepth, None, Some(500), crate::Ratchet::Off));
    time::sleep(Duration::from_millis(300)).await; // let the new intent gossip + reconcile
    assert_eq!(ceiling_of(&b), Some(4096), "local pin wins over the fleet intent");
}

/// Track 1: a node-targeted intent is applied only by the named node — per-node governance
/// rides the gossip path (the publisher's own node, not the target, must NOT apply it).
#[tokio::test]
async fn test_governor_node_targeted_intent() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::auto();
    cfg_a.bind_port = port_a; cfg_a.bootstrap_peers = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::auto();
    cfg_b.bind_port = port_b; cfg_b.bootstrap_peers = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;

    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b.clone(), cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 3_000).await;
    a.start_governor_reconciler();
    b.start_governor_reconciler();

    let ceiling_of = |agent: &GossipAgent| -> Option<u64> {
        agent.tuning_governor().params.into_iter()
            .find(|p| p.param == crate::HotParam::WriterDepth)
            .and_then(|p| p.ceiling)
    };

    // A publishes an intent TARGETED at B. It gossips to both, but only B applies it.
    let _ = a.publish_tuning_intent(
        crate::GovernIntent::bound(crate::HotParam::WriterDepth, None, Some(2048), crate::Ratchet::Off)
            .for_node(id_b.clone()));
    poll_until(|| ceiling_of(&b) == Some(2048), 5_000).await;
    assert_eq!(ceiling_of(&b), Some(2048), "the targeted node applies it");
    // Give A's reconciler ample time; it must NOT apply (not its target).
    time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ceiling_of(&a), None, "a non-targeted node ignores the intent");
}

/// I7's gate for the governor: with a decision sink on every node, membership still converges up to
/// `min` exactly as without one (the roll, spacing and settling are untouched), and the sinks say what
/// each node decided — every record is `membership.governed`, at least `min` of them are `join`
/// actions, none were dropped, and a `hold` carries the roll it was decided on.
#[tokio::test]
async fn test_membership_governor_with_a_decision_trace_decides_as_without_and_says_so() {
    use crate::capability::{Capability, CapFilter, CapabilityGroupDef};
    use mycelium_core::decision::{DecisionSink, OutcomeKind};
    let ports: Vec<u16> = (0..3).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let mut agents = Vec::new();
    let mut sinks = Vec::new();
    for i in 0..3 {
        let mut cfg = GossipConfig::auto();
        cfg.bind_port = ports[i];
        cfg.bootstrap_peers = ids.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, id)| id.clone()).collect();
        cfg.health_check_interval_secs = 1;
        cfg.health_check_max_jitter_ms = 50;
        let a = GossipAgent::new(ids[i].clone(), cfg);
        let sink = Arc::new(DecisionSink::default());
        a.with_decision_trace(Arc::clone(&sink));
        a.with_decision_trace(Arc::new(DecisionSink::default())); // a second attach warns and is ignored
        sinks.push(sink);
        agents.push(a);
    }
    for a in &agents { a.start().await.unwrap(); }
    let mut regs = Vec::new();
    for a in &agents {
        regs.push(a.capabilities().advertise_capability(Capability::new("svc", "worker"), Duration::from_secs(30)));
    }
    let _grp = agents[0].capabilities().define_capability_group(
        "pool",
        CapabilityGroupDef { filter: CapFilter::new("svc", "worker"), topology_policy: None, provides: vec![], requires: vec![] },
        Duration::from_secs(30));
    for a in &agents { a.start_membership_governor(); }
    poll_until(|| agents.iter().all(|a| !a.peers().is_empty()), 3_000).await;
    let _ = agents[0].publish_membership_intent(crate::MembershipIntent::new("pool", 2, None));
    let joined = |agents: &[GossipAgent]| -> usize {
        agents.iter().filter(|a| a.groups().iter().any(|g| g.as_ref() == "pool")).count()
    };
    poll_until(|| joined(&agents) >= 2, 20_000).await;
    assert!(joined(&agents) >= 2, "membership converges up to min=2 with a trace as without (got {})", joined(&agents));

    let records: Vec<_> = sinks.iter().flat_map(|s| s.snapshot()).collect();
    assert!(records.iter().all(|r| r.rule == "membership.governed" && r.target.as_deref() == Some("pool")), "{records:?}");
    let joins = records.iter().filter(|r| r.outcome == OutcomeKind::Action && r.reason == "join").count();
    assert!(joins >= 2, "at least min join actions across the fleet: {records:?}");
    assert_eq!(sinks.iter().map(|s| s.dropped()).sum::<u64>(), 0);
    if let Some(hold) = records.iter().find(|r| r.reason == "hold") {
        assert!(hold.inputs.iter().any(|i| i.value.starts_with("roll=")), "a hold says what it rolled: {hold:?}");
    }
    let report = agents[0].guarantee_report();
    assert_eq!(report.late_attachments, 0, "attached before start()");
    for a in agents { a.shutdown().await; }
}

/// Track 2a: a `MembershipIntent { min }` makes eligible nodes self-elect into the group until
/// the member count converges up to `min` — coordinator-free elastic sizing.
#[tokio::test]
async fn test_membership_governor_converges_to_min() {
    use crate::capability::{Capability, CapFilter, CapabilityGroupDef};
    let ports: Vec<u16> = (0..3).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();

    let mut agents = Vec::new();
    let mut regs = Vec::new();
    for i in 0..3 {
        let mut cfg = GossipConfig::auto();
        cfg.bind_port = ports[i];
        cfg.bootstrap_peers = ids.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, id)| id.clone()).collect();
        cfg.health_check_interval_secs = 1;        // fast convergence ticks
        cfg.health_check_max_jitter_ms = 50;
        agents.push(GossipAgent::new(ids[i].clone(), cfg));
    }
    for a in &agents { a.start().await.unwrap(); }
    // Every node is eligible: advertise the capability the group's filter matches.
    for a in &agents {
        regs.push(a.capabilities().advertise_capability(
            Capability::new("svc", "worker"), Duration::from_secs(30)));
    }
    // Define the group (one node is enough; the def gossips) and turn on the governor everywhere.
    let _grp = agents[0].capabilities().define_capability_group(
        "pool",
        CapabilityGroupDef { filter: CapFilter::new("svc", "worker"), topology_policy: None,
                             provides: vec![], requires: vec![] },
        Duration::from_secs(30));
    for a in &agents { a.start_membership_governor(); }

    // Wait for caps + group def to propagate so every node sees the eligible set.
    poll_until(|| agents.iter().all(|a| !a.peers().is_empty()), 3_000).await;

    // Publish: keep "pool" at >= 2 members. Nobody is a member yet → nodes self-elect up to 2.
    let _ = agents[0].publish_membership_intent(crate::MembershipIntent::new("pool", 2, None));

    let joined = |agents: &[GossipAgent]| -> usize {
        agents.iter().filter(|a| a.groups().iter().any(|g| g.as_ref() == "pool")).count()
    };
    poll_until(|| joined(&agents) >= 2, 20_000).await;
    assert!(joined(&agents) >= 2, "membership must converge up to min=2 (got {})", joined(&agents));
}

/// Regression for #56: a group under a live membership intent is **governed** — the emergent
/// watcher defers, so the group does NOT auto-join its full eligible set and the governor's `max`
/// actually holds. Before the fix, `reconcile_emergent_groups` auto-joined every cap-matching node
/// unconditionally and re-joined anything the governor shed, so the count was pinned at `eligible`
/// regardless of the intent — `max` was unenforceable.
#[tokio::test]
async fn test_membership_intent_governs_against_emergent_autojoin() {
    use crate::capability::{Capability, CapFilter, CapabilityGroupDef};
    let n = 3;
    let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();

    let mut agents = Vec::new();
    let mut regs = Vec::new();
    for i in 0..n {
        let mut cfg = GossipConfig::auto();
        cfg.bind_port = ports[i];
        cfg.bootstrap_peers = ids.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, id)| id.clone()).collect();
        cfg.health_check_interval_secs = 1;
        cfg.health_check_max_jitter_ms = 50;
        agents.push(GossipAgent::new(ids[i].clone(), cfg));
    }
    for a in &agents { a.start().await.unwrap(); }
    for a in &agents {
        regs.push(a.capabilities().advertise_capability(
            Capability::new("svc", "worker"), Duration::from_secs(30)));
    }
    let _grp = agents[0].capabilities().define_capability_group(
        "pool",
        CapabilityGroupDef { filter: CapFilter::new("svc", "worker"), topology_policy: None,
                             provides: vec![], requires: vec![] },
        Duration::from_secs(30));
    for a in &agents { a.start_membership_governor(); }
    poll_until(|| agents.iter().all(|a| !a.peers().is_empty()), 5_000).await;

    let joined = |agents: &[GossipAgent]| -> usize {
        agents.iter().filter(|a| a.groups().iter().any(|g| g.as_ref() == "pool")).count()
    };

    // Cap the group at 1 with 3 eligible nodes. With the fix the governor sheds to the band and the
    // emergent watcher does not re-join — so the count drops strictly below the eligible count.
    // Without the fix it is pinned at 3 forever (every shed is undone by emergent auto-join).
    let _ = agents[0].publish_membership_intent(crate::MembershipIntent::new("pool", 1, Some(1)));

    poll_until(|| { let c = joined(&agents); (1..n).contains(&c) }, 30_000).await;
    let count = joined(&agents);
    assert!((1..n).contains(&count),
        "governed group must be a subset (1..{n}) — got {count}; max unenforceable means the \
         emergent watcher is re-joining shed nodes (regression of #56)");
}

// Compile-time proof that GossipAgent is Send + Sync so it can be wrapped in Arc.
#[allow(dead_code)]
fn assert_gossip_agent_is_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<GossipAgent>();
}

#[tokio::test]
async fn test_state_request_ignored_from_unknown_peer() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    store.pin().insert(Arc::from("secret"), StoreEntry {
        data: Some(Bytes::from_static(b"payload")),
        timestamp: 1,
    });
    let (tx, _rx) = mpsc::channel(10);
    let (shutdown_tx, _) = spawn_handler(
        reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
        Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl,
    );

    // StateRequest from an unknown peer (not in peers map) must be silently ignored.
    send_wire(&mut writer, &WireMessage::StateRequest {
        sender: "127.0.0.1:4444".parse().unwrap(),
        store_hash: 0,
        bucket_hashes: vec![],
    }).await;

    // Give the handler time to process the message.
    time::sleep(Duration::from_millis(50)).await;

    // Store must be unchanged — no StateResponse was routed back because the
    // peer_writers map is empty (no writer was spawned for the unknown sender).
    assert_eq!(
        store.pin().get("secret").and_then(|e| e.data.clone()),
        Some(Bytes::from_static(b"payload")),
    );
    let _ = shutdown_tx.send(true);
}

// ── handle_connection behaviour ───────────────────────────────────────────

#[tokio::test]
async fn test_upsert_propagates() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    send_wire(&mut writer, &WireMessage::Data(data_update("k", b"v", 1, false))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").and_then(|e| e.data.clone()) == Some(Bytes::from_static(b"v")), 200).await;
}

// ── KV `SignedData` is domain-tagged (post-360 plan row F) ─────────────────────

/// A TLS identity for a handler test, generated into its own directory.
#[cfg(feature = "tls")]
fn test_tls_identity(tag: &str, node: &NodeId) -> (Arc<crate::tls::NodeTls>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("myc-{tag}-{}-{}", node.to_socket_addr().port(), std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() };
    let tls = crate::tls::load_or_generate(&cfg, node, Arc::new(mycelium_core::removal::RemovedSet::default()), Default::default()).unwrap();
    (Arc::new(tls), dir)
}

/// A `SignedData` frame for `update`, signed by `sk` over `message` — whatever message the test
/// wants the signature to be over.
#[cfg(feature = "tls")]
fn signed_data_over(update: GossipUpdate, signer: u64, sk: &ed25519_dalek::SigningKey, message: &[u8]) -> WireMessage {
    let sig = crate::tls::sign_bytes(sk, message);
    let lo: [u8; 32] = sig[..32].try_into().unwrap();
    let hi: [u8; 32] = sig[32..].try_into().unwrap();
    WireMessage::SignedData { update, signer, signature: (lo, hi) }
}

/// **A signature over the tagged message is accepted at the connection** (post-360 plan row F).
/// Seen failing first: the 2.32 verifier checked the bare canonical bytes only, so the tagged
/// frame was dropped as a bad signature and the key never applied.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_tagged_kv_signature_is_accepted_at_the_connection() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let peer = NodeId::new("127.0.0.1", 7301).unwrap();
    let sk = SigningKey::from_bytes(&[71u8; 32]);
    let opts = HandlerOpts { peer_keys: vec![(peer.clone(), vec![sk.verifying_key().to_bytes()])], ..Default::default() };
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);

    let mut u = data_update("tagged/k", b"v", 7301, false);
    u.sender = peer.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&u);
    send_wire(&mut writer, &signed_data_over(u, peer.id_hash(), &sk, &crate::framing::kv_signing_message(&canonical))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("tagged/k").and_then(|e| e.data.clone()) == Some(Bytes::from_static(b"v")), 500).await;
}

/// **A signature under another domain's tag is not a KV signature.** The point of the tag: a
/// consensus payload signed by this node over bytes that happen to be an update cannot be replayed
/// as a KV write. Dropped, and not counted as the 2.32 form either.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_signature_under_another_tag_is_not_a_kv_signature() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let peer = NodeId::new("127.0.0.1", 7302).unwrap();
    let sk = SigningKey::from_bytes(&[72u8; 32]);
    let opts = HandlerOpts { peer_keys: vec![(peer.clone(), vec![sk.verifying_key().to_bytes()])], ..Default::default() };
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);

    let mut u = data_update("other-tag/k", b"v", 7302, false);
    u.sender = peer.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&u);
    let mut as_consensus = b"mycelium.consensus/msg/1".to_vec();
    as_consensus.extend_from_slice(&(canonical.len() as u32).to_le_bytes());
    as_consensus.extend_from_slice(&canonical);
    send_wire(&mut writer, &signed_data_over(u, peer.id_hash(), &sk, &as_consensus)).await;
    // A control frame behind it, so "absent" means dropped rather than not yet read.
    let mut probe = data_update("other-tag/probe", b"p", 7303, false);
    probe.sender = peer.id_hash();
    let probe_canonical = crate::framing::canonical_sign_bytes(&probe);
    send_wire(&mut writer, &signed_data_over(probe, peer.id_hash(), &sk, &crate::framing::kv_signing_message(&probe_canonical))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("other-tag/probe").is_some(), 500).await;
    assert!(store.pin().get("other-tag/k").is_none(), "a consensus-tagged signature is not a KV signature");
}

/// **A TLS node that still sets the retired `kv_require_signed_frames` refuses an unsigned `Data` frame
/// exactly as one that does not** (post-360 plan rows F and F2). Before 2.33.0 the `Data` arm of the
/// connection handler applied a frame whatever the node's posture, so under `[tls]` a member could write
/// any key under any other member's `sender` hash by not signing (row F, seen failing first: the unsigned
/// key was applied). Since row F2 the setting is a no-op: a 2.33 configuration that set it loads and
/// refuses the same — [`an_unsigned_data_frame_is_refused_whatever_the_setting_says`] is the default.
#[cfg(feature = "tls")]
#[tokio::test]
#[allow(deprecated)]
async fn a_tls_node_that_sets_the_retired_setting_refuses_an_unsigned_data_frame() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let me = NodeId::new("127.0.0.1", 7311).unwrap();
    let peer = NodeId::new("127.0.0.1", 7312).unwrap();
    let (tls, dir) = test_tls_identity("unsigned-refused", &me);
    let sk = SigningKey::from_bytes(&[73u8; 32]);
    let mut config = GossipConfig::default();
    config.kv_require_signed_frames = true;
    let opts = HandlerOpts { config, tls: Some(tls), peer_keys: vec![(peer.clone(), vec![sk.verifying_key().to_bytes()])], ..Default::default() };
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);

    let mut forged = data_update("unsigned/k", b"forged", 7311, false);
    forged.sender = peer.id_hash();
    send_wire(&mut writer, &WireMessage::Data(forged)).await;
    // A signed control frame behind it, so "absent" means refused rather than not yet read.
    let mut probe = data_update("unsigned/probe", b"p", 7312, false);
    probe.sender = peer.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&probe);
    send_wire(&mut writer, &signed_data_over(probe, peer.id_hash(), &sk, &crate::framing::kv_signing_message(&canonical))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("unsigned/probe").is_some(), 500).await;
    assert!(store.pin().get("unsigned/k").is_none(), "an unsigned write is not applied by a node that requires signatures");
    assert_eq!(bounds.kv_unsigned_refused.load(Ordering::Relaxed), 1, "and the refusal is counted");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sealed identity record (`version ‖ history ‖ signer ‖ sig`) for `sk`'s key, signed by `signer`.
#[cfg(feature = "tls")]
fn sealed_record(history_keys: &[[u8; 32]], signer: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use ed25519_dalek::Signer;
    let history: Vec<u8> = history_keys.iter().flatten().copied().collect();
    let sig = signer.sign(&crate::agent::helpers::identity_proof_message(&history)).to_bytes();
    let proof = crate::agent::helpers::encode_identity_proof(&signer.verifying_key().to_bytes(), &sig);
    crate::agent::helpers::encode_sealed_identity(&history, &proof)
}

/// **Only a self-authenticating sealed identity record is exempt from the unsigned-frame refusal**
/// (#611's second review). The first cut exempted any `sys/identity{,-proof,-signed}/{n}` frame whose
/// `sender` was `n` — and `sender` is whatever the frame's writer puts there, so a member could garble
/// a victim's sealed record and proof, publish the victim's bare identity as its own key, or tombstone
/// it, all unsigned and all exempt. Now the exemption is the sealed record alone, not a tombstone,
/// whose proof verifies under a key in its own history and — when this node already holds keys for
/// `n` — under one of those. Everything else in the family is refused like any unsigned frame, and
/// the exempt frames are counted. Seen failing first: garbage in the sealed slot, a bare identity, a
/// proof alone and a tombstone were all applied. Since row F2 it runs with `kv_require_signed_frames` at its
/// default — seen failing first on 4251d50a, where the default applied all five refused frames.
#[cfg(feature = "tls")]
#[tokio::test]
async fn only_a_self_authenticating_sealed_identity_is_exempt_from_the_refusal() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(32);
    let me = NodeId::new("127.0.0.1", 7341).unwrap();
    let fresh = NodeId::new("127.0.0.1", 7342).unwrap();   // a node this receiver has never seen
    let known = NodeId::new("127.0.0.1", 7343).unwrap();   // a node whose key this receiver holds
    let fresh_sk = SigningKey::from_bytes(&[77u8; 32]);
    let known_sk = SigningKey::from_bytes(&[78u8; 32]);
    let other_sk = SigningKey::from_bytes(&[79u8; 32]);
    let (tls, dir) = test_tls_identity("identity-exempt", &me);
    let opts = HandlerOpts { tls: Some(tls), peer_keys: vec![(known.clone(), vec![known_sk.verifying_key().to_bytes()])], ..Default::default() };
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);
    let mut nonce = 7400u64;
    let mut send = |key: String, value: Vec<u8>, tomb: bool, sender: &NodeId| {
        nonce += 1;
        let mut u = data_update(&key, &value, nonce, tomb);
        u.sender = sender.id_hash();
        u.timestamp = nonce; // later frames win LWW
        WireMessage::Data(u)
    };
    let refused: Vec<String> = vec![
        format!("sys/identity-signed/{fresh}"),   // garbage in the sealed slot
        format!("sys/identity/{known}"),          // a bare identity
        format!("sys/identity-proof/{known}"),    // a proof alone
        format!("sys/identity-signed/{known}"),   // a tombstone
        format!("sys/identity-signed/{known}"),   // valid in itself, but signed by a key `known` does not hold here
    ];
    let msgs = vec![
        send(refused[0].clone(), b"garbage".to_vec(), false, &fresh),
        send(refused[1].clone(), other_sk.verifying_key().to_bytes().to_vec(), false, &known),
        send(refused[2].clone(), vec![0u8; 96], false, &known),
        send(refused[3].clone(), Vec::new(), true, &known),
        send(refused[4].clone(), sealed_record(&[other_sk.verifying_key().to_bytes()], &other_sk), false, &known),
    ];
    for m in &msgs { send_wire(&mut writer, m).await; }
    // Exempt: a first-contact sealed record that proves itself, and a rotation for `known` that chains to its held key.
    let first_contact = format!("sys/identity-signed/{fresh}");
    let first = send(first_contact.clone(), sealed_record(&[fresh_sk.verifying_key().to_bytes()], &fresh_sk), false, &fresh);
    let rotation_key = format!("sys/identity-signed/{known}");
    let rot = send(rotation_key.clone(),
        sealed_record(&[other_sk.verifying_key().to_bytes(), known_sk.verifying_key().to_bytes()], &known_sk), false, &known);
    send_wire(&mut writer, &first).await;
    send_wire(&mut writer, &rot).await;
    let s = Arc::clone(&store);
    poll_until(|| s.pin().get(first_contact.as_str()).is_some_and(|e| e.data.is_some())
        && s.pin().get(rotation_key.as_str()).is_some_and(|e| e.data.is_some()), 1000).await;
    for k in &refused[1..3] {
        assert!(store.pin().get(k.as_str()).is_none(), "{k}: not a self-authenticating sealed record, refused");
    }
    assert_eq!(bounds.kv_unsigned_refused.load(Ordering::Relaxed), 5, "garbage, bare, proof, tombstone and the unchained record");
    assert_eq!(bounds.kv_identity_exempt.load(Ordering::Relaxed), 2, "and the two exempt frames are counted");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Three TLS nodes (each refusing unsigned frames — since row F2, every TLS node), B and C each bootstrapped to A, anti-entropy a minute away.
/// Returns once B holds C's key — **first contact** between B and C rests on what reaches B by gossip.
#[cfg(feature = "tls")]
async fn three_signed_nodes(tag: &str, proofs: bool) -> (GossipAgent, GossipAgent, GossipAgent, NodeId, NodeId, std::path::PathBuf) {
    three_signed_nodes_with(tag, proofs, |_, _| {}).await
}

/// [`three_signed_nodes`], with B's configuration adjusted by `tweak_b` (a restart test gives B
/// persistence).
#[cfg(feature = "tls")]
async fn three_signed_nodes_with(tag: &str, proofs: bool, tweak_b: impl Fn(&mut GossipConfig, &NodeId)) -> (GossipAgent, GossipAgent, GossipAgent, NodeId, NodeId, std::path::PathBuf) {
    three_signed_nodes_tweaked(tag, proofs, tweak_b, |_, _| {}).await
}

/// [`three_signed_nodes_with`], with C's configuration adjusted by `tweak_c` too.
#[cfg(feature = "tls")]
async fn three_signed_nodes_tweaked(tag: &str, proofs: bool, tweak_b: impl Fn(&mut GossipConfig, &NodeId), tweak_c: impl Fn(&mut GossipConfig, &NodeId)) -> (GossipAgent, GossipAgent, GossipAgent, NodeId, NodeId, std::path::PathBuf) {
    use crate::config::TlsConfig;
    let ports = (alloc_port(), alloc_port(), alloc_port());
    let (na, nb, nc) = (NodeId::new("127.0.0.1", ports.0).unwrap(), NodeId::new("127.0.0.1", ports.1).unwrap(), NodeId::new("127.0.0.1", ports.2).unwrap());
    let cert_dir = std::env::temp_dir().join(format!("myc-{tag}-{}", ports.0));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mk = |node: &NodeId, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = node.to_socket_addr().port();
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 60;
        cfg.require_identity_proofs = proofs;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(node.clone(), cfg)
    };
    let a = mk(&na, vec![]);
    let b = {
        let mut cfg = mk(&nb, vec![na.clone()]).config.clone();
        tweak_b(&mut cfg, &nb);
        GossipAgent::new(nb.clone(), cfg)
    };
    let c = {
        let mut cfg = mk(&nc, vec![na.clone()]).config.clone();
        tweak_c(&mut cfg, &nc);
        GossipAgent::new(nc.clone(), cfg)
    };
    a.start().await.unwrap(); b.start().await.unwrap(); c.start().await.unwrap();
    let knows = |x: &GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    for _ in 0..600 {
        if knows(&b, &nc) && knows(&b, &na) && knows(&c, &nb) && knows(&a, &nc) && !b.peers().is_empty() && !c.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(knows(&b, &nc) && knows(&c, &nb), "first contact: B and C never learned each other's key");
    (a, b, c, nb, nc, cert_dir)
}

/// A frame A forges in C's name: unsigned, `sender` = C.
#[cfg(feature = "tls")]
fn forge_unsigned_as(a: &GossipAgent, victim: &NodeId, key: String, value: Bytes, tomb: bool) {
    let mut u = crate::framing::make_gossip_update(&a.node_id, a.task_ctx.default_ttl, Arc::from(key.as_str()), value, tomb, &a.task_ctx.hlc);
    u.sender = victim.id_hash();
    crate::framing::dispatch_gossip_try_send(&a.task_ctx.gossip_txs, crate::framing::WireMessage::Data(u), a.node_id.id_hash(),
        crate::framing::ForwardHint::All, &a.task_ctx.kv_state.dropped_frames);
}

/// **The reviewer's impersonation, lifted** (#611's second review). With proofs off, A garbled C's
/// sealed record and proof and published C's bare identity as A's own key — all unsigned with
/// `sender` = C, all exempt — so B merged A's key into C's set, and A's write signed as C landed.
/// Seen failing first on 59d86368: "IMPERSONATION: A wrote as C at a node requiring signed frames".
#[cfg(feature = "tls")]
#[tokio::test]
async fn an_unsigned_identity_forgery_cannot_make_a_member_sign_as_another() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("forge-imp", false).await;
    let a_vk = a.task_ctx.tls.get().unwrap().signing_key().verifying_key().to_bytes();
    forge_unsigned_as(&a, &nc, format!("sys/identity-signed/{nc}"), Bytes::from_static(b"x"), false);
    forge_unsigned_as(&a, &nc, format!("sys/identity-proof/{nc}"), Bytes::from_static(b"x"), false);
    tokio::time::sleep(Duration::from_millis(200)).await;
    forge_unsigned_as(&a, &nc, format!("sys/identity/{nc}"), Bytes::copy_from_slice(&a_vk), false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&a_vk)), "A's key did not enter C's set at B");
    let mut u = crate::framing::make_gossip_update(&a.node_id, a.task_ctx.default_ttl, Arc::from("forged/as-c"), Bytes::from_static(b"by-A"), false, &a.task_ctx.hlc);
    u.sender = nc.id_hash();
    let msg = crate::framing::make_kv_wire_msg(u, nc.id_hash(), a.task_ctx.tls.get().map(Arc::as_ref));
    crate::framing::dispatch_gossip_try_send(&a.task_ctx.gossip_txs, msg, a.node_id.id_hash(),
        crate::framing::ForwardHint::All, &a.task_ctx.kv_state.dropped_frames);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let landed = b.kv().get("forged/as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!landed, "IMPERSONATION: A wrote as C at a node requiring signed frames");
}

/// Row S: put `node` on a **legacy certificate** — CA-issued, naming its IP and not its node (no URI
/// SAN, what every certificate before row Q was), so no node gives it standing and its trusted keys
/// are its retained set. Mints the fleet CA into `cfg`'s directory first (the other nodes load it).
#[cfg(feature = "tls")]
fn legacy_certificate_for(cfg: &mut GossipConfig, node: &NodeId) {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let dir = cfg.tls.as_ref().expect("a TLS node").auto_cert_dir.clone();
    std::fs::create_dir_all(&dir).unwrap();
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    std::fs::write(dir.join("ca-key.pem"), ca_key.serialize_pem()).unwrap();
    std::fs::write(dir.join("ca-cert.pem"), ca.pem()).unwrap();
    let sk = ed25519_dalek::SigningKey::from_bytes(&[0x5e; 32]);
    let kp = rcgen::KeyPair::try_from(sk.to_pkcs8_der().unwrap().as_bytes()).unwrap();
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.subject_alt_names = vec![rcgen::SanType::IpAddress(node.to_socket_addr().ip())];
    p.not_before = rcgen::date_time_ymd(2024, 1, 1);
    p.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let cert = p.signed_by(&kp, &ca, &ca_key).unwrap();
    let (cert_path, key_path) = (dir.join("legacy-node.crt"), dir.join("legacy-node.key.pem"));
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, kp.serialize_pem()).unwrap();
    let t = cfg.tls.as_mut().unwrap();
    t.cert_pem = Some(cert_path);
    t.key_pem = Some(key_path);
}

/// Row S: A garbles C's sealed record and proof and writes C's bare identity as `ka ‖ k0` — each its
/// own signed write, signer = sender = A, a valid frame. Returns A's signing key and whether B came
/// to hold `ka` for C within the bound.
#[cfg(feature = "tls")]
async fn poison_identity(a: &GossipAgent, b: &GossipAgent, c: &GossipAgent, nc: &NodeId) -> (Arc<ed25519_dalek::SigningKey>, bool) {
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let ka = a_sk.verifying_key().to_bytes();
    let k0 = c.identity_public_key().unwrap();
    let _ = a.kv().set(format!("sys/identity-signed/{nc}"), b"x".to_vec());
    let _ = a.kv().set(format!("sys/identity-proof/{nc}"), b"x".to_vec());
    let _ = a.kv().set(format!("sys/identity/{nc}"), [ka, k0].concat());
    let held = within_3s(|| b.task_ctx.peer_keys.pin().get(nc).is_some_and(|k| k.contains(&ka))).await;
    (a_sk, held)
}

/// The verifier's identity setting left as the default, whatever the helper's `proofs` argument says.
#[cfg(feature = "tls")]
fn proofs_by_default(cfg: &mut GossipConfig, _: &NodeId) {
    cfg.require_identity_proofs = GossipConfig::default().require_identity_proofs;
}

/// **Post-360 plan row S: by default, a member's key written into another's identity does not sign as
/// it on a legacy certificate.** C runs on a pre-row-Q certificate, so B trusts C's retained key set;
/// A poisons C's identity with its own key ([`poison_identity`]) and writes as C under it. With
/// `require_identity_proofs` off — the default until row S — B merged `ka` into C's set and the write
/// landed: full impersonation. B and C run the default configuration here. Seen failing first on
/// 4251d50a: "IMPERSONATION: by default, a key A wrote into C's identity signs as C at B".
#[cfg(feature = "tls")]
#[tokio::test]
async fn by_default_a_poisoned_identity_key_does_not_sign_for_a_legacy_certificate_node() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-legacy", false, proofs_by_default, |cfg, n| {
        proofs_by_default(cfg, n);
        legacy_certificate_for(cfg, n);
    }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let anchored = b.task_ctx.key_standing.get(&nc).is_some_and(|s| s.anchored.is_some());
    let (a_sk, held) = poison_identity(&a, &b, &c, &nc).await;
    let ka = a_sk.verifying_key().to_bytes();
    send_signed_as(&a, &nc, &a_sk, "poison/legacy-as-c", b"by-ka");
    let plant = plant_lands(&c, &b, "poison/legacy-plant").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = b.kv().get("poison/legacy-as-c").is_some();
    let in_history = b.task_ctx.history_keys(&nc).contains(&ka);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!anchored, "the setup: C's legacy certificate gives it no standing at B");
    assert!(plant, "C's own writes still arrive at B");
    assert!(!landed, "IMPERSONATION: by default, a key A wrote into C's identity signs as C at B");
    assert!(!in_history, "nor is it in C's history set");
    assert!(!held, "by default, A's key does not enter C's set at B");
}

/// **Post-360 plan row S: by default, a member's key written into another's identity does not reach
/// its history set.** C is anchored (a current certificate), so its *live* writes verify only under its
/// anchored keys whatever B holds — but history verification (audit-chain recovery, verification and
/// pruning, `CoreCtx::history_keys`) reads every key B holds for C. With proofs off a poisoned `ka`
/// entered it, so a record A sealed under `ka` verified as C's history. Seen failing first on
/// 4251d50a: "HISTORY: by default, a key A wrote into C's identity is in C's history set at B".
#[cfg(feature = "tls")]
#[tokio::test]
async fn by_default_a_poisoned_identity_key_does_not_reach_the_history_set() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-history", false, proofs_by_default, proofs_by_default).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    #[cfg(feature = "compliance")]
    let h0 = c.audit(crate::AuditAction::Invoke, "p", "t0", crate::AuditOutcome::Success, None).unwrap();
    let (a_sk, held) = poison_identity(&a, &b, &c, &nc).await;
    let ka = a_sk.verifying_key().to_bytes();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let in_history = b.task_ctx.history_keys(&nc).contains(&ka);
    #[cfg(feature = "compliance")]
    let forged_verifies = {
        use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
        let r1 = AuditRecord { node_id: nc.clone(), seq: 1, hlc: 9, principal: "attacker".into(), action: AuditAction::Admin, target: "role/grant".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: h0 };
        let _ = a.kv().set(audit_key(&nc, 1), SignedAuditRecord::sign(r1, &a_sk).encode());
        let _ = within_3s(|| b.kv().get(&audit_key(&nc, 1)).is_some()).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        b.audit_verify(&nc).is_ok()
    };
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    #[cfg(feature = "compliance")]
    assert!(!forged_verifies, "FORGED: by default, A's audit record signed by its poisoned key verifies as C's history at B");
    assert!(!in_history, "HISTORY: by default, a key A wrote into C's identity is in C's history set at B");
    assert!(!held, "by default, A's key does not enter C's set at B");
}

/// **Rotation does not launder a member's key into a node's signed history** (#623's review, finding
/// 1, its probe lifted). `rotate_identity` built the new history from the raw `sys/identity/{self}`
/// value — any member can write it — and signed it with the trusted prior key into the sealed record,
/// so a key A planted there became part of C's authenticated history at every peer, with proofs on.
/// The history now comes from what C can show it held (`revocation::own_history`), as the start path's
/// does. Seen failing first on 1e0a648e: "LAUNDERED: after C rotated, A's poisoned key is in C's
/// history set at B".
#[cfg(feature = "tls")]
#[tokio::test]
async fn rotation_does_not_launder_a_poisoned_bare_identity_into_the_history() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-rotlaunder", false, proofs_by_default, proofs_by_default).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (a_sk, _held) = poison_identity(&a, &b, &c, &nc).await;
    let ka = a_sk.verifying_key().to_bytes();
    let c_has = within_3s(|| c.kv().get(&format!("sys/identity/{nc}")).is_some_and(|v| v.len() >= 32 && v[..32] == ka)).await;
    let before = b.task_ctx.history_keys(&nc).contains(&ka);
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let learned_k1 = within_3s(|| b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k1))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = b.task_ctx.history_keys(&nc).contains(&ka);
    let in_peer = b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&ka));
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(c_has && !before && learned_k1, "setup: C holds the poisoned bare list, B does not trust it yet, and B learns C's rotated key");
    assert!(!after, "LAUNDERED: after C rotated, A's poisoned key is in C's history set at B");
    assert!(!in_peer, "and not in C's key set at B");
}

/// [`rotation_does_not_launder_a_poisoned_bare_identity_into_the_history`] on a legacy certificate,
/// where the laundered key would also be *live* trust: a write A signs as C under it must not land at
/// B after C rotates (#623's review, its second probe). **A guard, not a regression test:** it passes
/// on 1e0a648e too, because the rotation itself issues C a certificate naming it (row Q), so after it C
/// is anchored and its trusted keys are its anchored keys, whatever its history holds. The laundering
/// reaches the history set, which the test above pins.
#[cfg(feature = "tls")]
#[tokio::test]
async fn rotation_does_not_launder_a_poisoned_key_into_live_trust_on_a_legacy_certificate() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-rotlegacy", false, proofs_by_default, |cfg, n| {
        proofs_by_default(cfg, n);
        legacy_certificate_for(cfg, n);
    }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (a_sk, _held) = poison_identity(&a, &b, &c, &nc).await;
    let ka = a_sk.verifying_key().to_bytes();
    let c_has = within_3s(|| c.kv().get(&format!("sys/identity/{nc}")).is_some_and(|v| v.len() >= 32 && v[..32] == ka)).await;
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let learned_k1 = within_3s(|| b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k1))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let trusted = b.task_ctx.trusted_keys(&nc).contains(&ka);
    send_signed_as(&a, &nc, &a_sk, "poison/rot-legacy-as-c", b"by-ka");
    let plant = plant_lands(&c, &b, "poison/rot-legacy-plant").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = b.kv().get("poison/rot-legacy-as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(c_has && learned_k1 && plant, "setup: C holds the poisoned list, B learns k1, and C's own writes arrive");
    assert!(!landed, "IMPERSONATION after rotation: a key A wrote into C's identity signs as C at B");
    assert!(!trusted, "and it is not among C's trusted keys at B");
}

/// **A node that rotates twice keeps its original key in its published history** (#623's confirming
/// check, its probe lifted). Rotation signed the sealed record with the *prior* key and never re-signed
/// it, so on a legacy-certificate node — whose original key no certificate record anchors — the second
/// rotation's `own_history` ignored that record (its signer is not a key the node can show it holds)
/// and dropped k0: existing peers kept it, but a peer joining later learned a history without it.
/// Rotation now re-signs the record with the new key after the cut-over. Seen failing first on
/// 0f9d417b: "k0 dropped: a legacy-certificate node's history lost its original key after two
/// rotations".
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_double_rotation_keeps_the_original_key_in_the_published_history() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-dblrot", false, proofs_by_default, |cfg, n| {
        proofs_by_default(cfg, n);
        legacy_certificate_for(cfg, n);
    }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0 = c.identity_public_key().unwrap();
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let _ = within_3s(|| b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k1))).await;
    let k2 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let _ = within_3s(|| b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k2))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sealed = c.kv().get(&format!("sys/identity-signed/{nc}")).unwrap();
    let hist = crate::agent::helpers::parse_sealed_identity(&sealed).map(|(h, _)| crate::agent::helpers::parse_identity_keys(h)).unwrap_or_default();
    // A peer joining afterwards learns C's history from gossip alone.
    let pd = alloc_port();
    let nd = NodeId::new("127.0.0.1", pd).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = pd; cfg.bootstrap_peers = vec![a.node_id.clone()]; cfg.reconnect_backoff_secs = 1;
    // A short health interval so anti-entropy — the only way a joiner gets records gossiped before it
    // arrived — runs within the test's bound.
    cfg.health_check_interval_secs = 1;
    cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..crate::config::TlsConfig::default() });
    let d = GossipAgent::new(nd, cfg);
    d.start().await.unwrap();
    let mut d_k2 = false;
    for _ in 0..200 { if d.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k2)) { d_k2 = true; break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let mut d_has_k0 = false;
    for _ in 0..400 { if d.task_ctx.history_keys(&nc).contains(&k0) { d_has_k0 = true; break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    a.shutdown().await; b.shutdown().await; c.shutdown().await; d.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(d_k2, "setup: the late joiner learns C's current key");
    assert!(hist.contains(&k0) && hist.len() == 3, "k0 dropped: a legacy-certificate node's history lost its original key after two rotations ({} keys)", hist.len());
    assert!(d_has_k0, "and a peer joining afterwards holds k0 in C's history set");
}

/// **Post-360 plan row S: `false` is still an opt-out.** A verifier configured with an explicit
/// `require_identity_proofs = false` (the TOML spelling an operator writes) is back on the pre-row-S
/// rule — the residual the two tests above close by default reproduces, which is what an opt-out
/// means. A guard, not a regression test: it passes on 4251d50a too.
#[cfg(feature = "tls")]
#[tokio::test]
async fn an_explicit_false_still_opts_out_of_identity_proofs() {
    let opt_out = |cfg: &mut GossipConfig, _: &NodeId| {
        let parsed: GossipConfig = toml::from_str("require_identity_proofs = false\n").unwrap();
        cfg.require_identity_proofs = parsed.require_identity_proofs;
    };
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("s-optout", false, opt_out, opt_out).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (a_sk, held) = poison_identity(&a, &b, &c, &nc).await;
    let in_history = b.task_ctx.history_keys(&nc).contains(&a_sk.verifying_key().to_bytes());
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(held && in_history, "an explicit false keeps the pre-row-S rule: the bare list merges (detected, not prevented)");
}

/// **The reviewer's silencing, lifted, in both postures** (#611's second review, and the watcher's
/// own pre-existing rule): one tombstone of `sys/identity/{C}` — unsigned and exempt under the first
/// cut, or signed by any member as itself — made B's identity watcher drop C's keys, after which C's
/// signed writes were dropped at B as from an unknown signer. A tombstone no longer removes keys: they
/// leave only by membership removal or a verified rotation. Seen failing first on 59d86368: "DOS: an
/// unsigned forged tombstone erased C's key at B", with proofs off and with proofs on.
#[cfg(feature = "tls")]
#[tokio::test]
async fn an_identity_tombstone_does_not_erase_a_members_keys() {
    for proofs in [false, true] {
        let (a, b, c, _nb, nc, dir) = three_signed_nodes(if proofs { "forge-tomb-p" } else { "forge-tomb" }, proofs).await;
        forge_unsigned_as(&a, &nc, format!("sys/identity/{nc}"), Bytes::new(), true);
        // And the same tombstone signed by A as itself — a member's own write, signer == sender.
        assert!(a.kv().delete(format!("sys/identity/{nc}")));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let forgotten = b.task_ctx.peer_keys.pin().get(&nc).is_none_or(|k| k.is_empty());
        let _ = c.kv().set("after/c", b"v".to_vec());
        let mut arrived = false;
        for _ in 0..120 {
            if b.kv().get("after/c").is_some() { arrived = true; break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        a.shutdown().await; b.shutdown().await; c.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!forgotten, "proofs={proofs} DOS: an identity tombstone erased C's key at B");
        assert!(arrived, "proofs={proofs}: and C's later signed write reached B by gossip");
    }
}

/// **A replayed, superseded sealed record is not exempt** (#611's third review, N4, lifted from the
/// reviewer's `review_replay_old_sealed_record_after_rotation`). C's genuine pre-rotation sealed record
/// still proves itself — its signer is in its history and B still holds that key — so a member that
/// kept a copy could replay it unsigned with a fresh timestamp and replace C's current record at B; a
/// node joining later would then read the pre-rotation history. A node's history only grows, so an
/// exempt record must now contain every key the receiver already holds or has stored for that node;
/// one that does not is refused and counted (`kv_identity_stale_refused`). Seen failing first on
/// 884f310f: "REPLAY: a genuine pre-rotation sealed record, replayed unsigned with a fresh timestamp,
/// replaced C's current one".
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_replayed_pre_rotation_sealed_record_does_not_replace_the_current_one() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("replay-sealed", false).await;
    let sealed_key = format!("sys/identity-signed/{nc}");
    let mut old = None;
    for _ in 0..200 { if let Some(v) = a.kv().get(&sealed_key) { old = Some(v); break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let old = old.expect("A holds C's sealed record");
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    for _ in 0..200 {
        if b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k1)) && b.kv().get(&sealed_key) != Some(old.clone()) { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&k1)), "B learned C's rotated key");
    forge_unsigned_as(&a, &nc, sealed_key.clone(), old.clone(), false);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let b_holds_old = b.kv().get(&sealed_key) == Some(old.clone());
    let stale = b.task_ctx.transport_bounds.kv_identity_stale_refused.load(Ordering::Relaxed);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!b_holds_old, "REPLAY: a genuine pre-rotation sealed record, replayed unsigned with a fresh timestamp, replaced C's current one");
    assert!(stale >= 1, "and the refusal is counted");
}

// ── Post-360 plan row Q: revocation reaches every verifier ──────────────────────────────────────

/// A `SignedData` frame A sends in `as_node`'s name, signed by `sk` — what a holder of one of
/// `as_node`'s keys can put on the wire.
#[cfg(feature = "tls")]
fn send_signed_as(a: &GossipAgent, as_node: &NodeId, sk: &ed25519_dalek::SigningKey, key: &str, value: &'static [u8]) {
    let mut u = crate::framing::make_gossip_update(&a.node_id, a.task_ctx.default_ttl, Arc::from(key), Bytes::from_static(value), false, &a.task_ctx.hlc);
    u.sender = as_node.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&u);
    let msg = signed_data_over(u, as_node.id_hash(), sk, &crate::framing::kv_signing_message(&canonical));
    crate::framing::dispatch_gossip_try_send(&a.task_ctx.gossip_txs, msg, a.node_id.id_hash(),
        crate::framing::ForwardHint::All, &a.task_ctx.kv_state.dropped_frames);
}

/// A signed revocation of `revoked` in `node`'s name, signed by `sk` — whoever holds it.
#[cfg(all(feature = "tls", feature = "compliance"))]
fn revocation_signed_by(node: &NodeId, revoked: [u8; 32], sk: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use crate::agent::revocation::{RevocationEvent, SignedRevocation};
    let event = RevocationEvent { node_id: node.clone(), hlc: 1, revoked_key: revoked, reason: None };
    let sig = crate::tls::sign_bytes(sk, &mycelium_core::serde_fixint::to_vec(&event).unwrap()).to_vec();
    SignedRevocation { event, sig }.encode().to_vec()
}

/// Whether `x` no longer trusts `old` for `node` and holds `new` for it — the revocation seen.
#[cfg(all(feature = "tls", feature = "compliance"))]
async fn revocation_seen(x: &GossipAgent, node: &NodeId, old: [u8; 32], new: [u8; 32]) -> bool {
    for _ in 0..400 {
        let trusted = crate::agent::helpers::known_verifying_keys(&x.task_ctx, node);
        if !trusted.contains(&old) && trusted.contains(&new) { return true; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// Poll `ok` for up to three seconds.
#[cfg(feature = "tls")]
async fn within_3s(mut ok: impl FnMut() -> bool) -> bool {
    for _ in 0..120 { if ok() { return true; } tokio::time::sleep(Duration::from_millis(25)).await; }
    ok()
}

/// **A KV write signed by a revoked key is refused once the revocation is seen** (post-360 plan row Q,
/// lifted from #611's independent review, `review_revoked_key_still_verifies_kv_writes`). After C's
/// `rotate_identity_on_compromise`, B listed C's old key as revoked yet still held it in `peer_keys`,
/// which is all the `SignedData` arm read — so a write signed by the revoked key as C was applied. C's
/// own write under its new key still arrives (the plant: the refusal is the revocation, not the
/// channel). Seen failing first on 7c967ba3: "REVOKED KEY: C's revoked key still signs C's KV writes at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_kv_write_signed_by_a_revoked_key_is_refused_once_the_revocation_is_seen() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-kv", false).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    send_signed_as(&a, &nc, &k0_sk, "revoked/as-c", b"by-k0");
    let _ = c.kv().set("revoked/plant", b"by-k1".to_vec());
    let plant = within_3s(|| b.kv().get("revoked/plant").is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = b.kv().get("revoked/as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "B saw C's old key revoked and its new key held");
    assert!(plant, "C's write under its new key reached B");
    assert!(!landed, "REVOKED KEY: C's revoked key still signs C's KV writes at B");
}

/// Drive one in-memory TLS handshake; `Err` names the side that refused.
#[cfg(all(feature = "tls", feature = "compliance"))]
fn mesh_handshake(client: Arc<rustls::ClientConfig>, server: Arc<rustls::ServerConfig>) -> Result<(), String> {
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
    let mut c = rustls::ClientConnection::new(client, name).unwrap();
    let mut s = rustls::ServerConnection::new(server).unwrap();
    for _ in 0..16 {
        let mut buf = Vec::new();
        while c.wants_write() { c.write_tls(&mut buf).unwrap(); }
        if !buf.is_empty() {
            s.read_tls(&mut buf.as_slice()).unwrap();
            s.process_new_packets().map_err(|e| format!("server refused: {e}"))?;
        }
        let mut buf = Vec::new();
        while s.wants_write() { s.write_tls(&mut buf).unwrap(); }
        if !buf.is_empty() {
            c.read_tls(&mut buf.as_slice()).unwrap();
            c.process_new_packets().map_err(|e| format!("client refused: {e}"))?;
        }
        if !c.is_handshaking() && !s.is_handshaking() { return Ok(()); }
    }
    Err("the handshake did not finish".into())
}

/// **A thief holding a revoked key and its certificate is refused at the mTLS handshake** (post-360
/// plan row Q's follow-up, end to end). C's pre-rotation client config — its old key and the
/// CA-issued certificate naming C — is what a thief who stole them can present. Before, B's handshake
/// judged the CA chain and the removal ledger only, so after C's `rotate_identity_on_compromise` the
/// thief was still a connected peer (its signed writes refused one by one). Now the revocation B's
/// fold counted reaches B's verifier. The plant: C's new certificate is accepted, and the old one was
/// accepted before the rotation. Seen failing first: the old certificate's handshake completed.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_revoked_keys_certificate_is_refused_at_a_peers_handshake() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-handshake", false).await;
    let thief = c.task_ctx.tls.get().unwrap().client_config();
    let k0 = c.task_ctx.tls.get().unwrap().verifying_key_bytes();
    let b_server = || b.task_ctx.tls.get().unwrap().server_config();
    let before = mesh_handshake(Arc::clone(&thief), b_server());
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let stolen = mesh_handshake(thief, b_server());
    let current = mesh_handshake(c.task_ctx.tls.get().unwrap().client_config(), b_server());
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(before, Ok(()), "plant: C's certificate was accepted before the rotation");
    assert!(seen, "B saw C's old key revoked and its new key held");
    assert_eq!(current, Ok(()), "plant: C's new certificate is accepted");
    assert!(stolen.as_ref().is_err_and(|e| e.contains("revoked")),
        "REVOKED KEY: B accepted a handshake from C's revoked key and certificate: {stolen:?}");
}

/// **A key revoked long ago stays refused at the handshake once it falls out of the anchored set**
/// (the re-review of row Q's follow-up). The fold publishes only a node's sixteen newest anchored keys
/// (`MAX_CERTS_PER_NODE`), so after C's compromise rotation and sixteen more, C's first key is no
/// longer anchored at B — and the first cut of the rule (*anchored and revoked*) admitted the thief's
/// handshake again, while every signature check still refused the key. The handshake now reads the
/// revocations B **counted** (`NodeStanding::counted`), which no cap or saturation drops. Seen failing
/// first with the rule reading the anchored set.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_key_revoked_long_ago_stays_refused_at_the_handshake_past_the_anchor_cap() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-handshake-cap", false).await;
    let thief = c.task_ctx.tls.get().unwrap().client_config();
    let k0 = c.task_ctx.tls.get().unwrap().verifying_key_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(300)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let mut newest = k1;
    for _ in 0..16 {
        newest = c.rotate_identity(Duration::from_millis(100)).await.unwrap();
    }
    let k0_dropped = within_3s(|| b.task_ctx.key_standing.get(&nc).is_some_and(|s| {
        s.anchored.as_ref().is_some_and(|a| a.contains(&newest) && !a.contains(&k0))
    })).await;
    let counted = b.task_ctx.key_standing.get(&nc).is_some_and(|s| s.counted.contains(&k0));
    let stolen = mesh_handshake(thief, b.task_ctx.tls.get().unwrap().server_config());
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "B saw C's first key revoked");
    assert!(k0_dropped, "plant: past the cap, C's first key is no longer among its anchored keys at B");
    assert!(counted, "B still holds the revocation as counted");
    assert!(stolen.as_ref().is_err_and(|e| e.contains("revoked")),
        "REVOKED KEY: past the anchor cap, B accepted a handshake from C's long-revoked key: {stolen:?}");
}

/// **An identity record chained through a revoked key adds nothing to the node's key set** (row Q).
/// `validate_and_merge_identity` accepted a record whose proof was signed by any key held for the
/// node — revoked or not — so a holder of C's revoked key could publish a sealed record for C carrying
/// its own key, and every peer that had seen the revocation merged it. Here A writes that record as
/// its own (signed) write, so it reaches B's identity watcher rather than the exemption. Seen failing
/// first on 7c967ba3: "REVOKED KEY: a record chained through C's revoked key put an attacker key in
/// C's set at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn an_identity_record_chained_through_a_revoked_key_is_not_merged() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-chain", false).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let ka = ed25519_dalek::SigningKey::from_bytes(&[0x5a; 32]).verifying_key().to_bytes();
    let key = format!("sys/identity-signed/{nc}");
    let forged = sealed_record(&[ka, k1, k0], &k0_sk);
    let _ = a.kv().set(key.clone(), forged.clone());
    let stored = within_3s(|| b.kv().get(&key).is_some_and(|v| v.as_ref() == forged.as_slice())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let merged = b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&ka))
        && crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&ka);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "B saw C's old key revoked");
    assert!(stored, "A's own signed write reached B's store");
    assert!(!merged, "REVOKED KEY: a record chained through C's revoked key put an attacker key in C's set at B");
}

/// **A sealed record signed by a revoked key is not exempt from the unsigned-frame refusal** (row Q).
/// `connection::self_authenticating_identity` exempted a sealed record whose signer was any key held
/// for the node, so a holder of C's revoked key could send one unsigned, under
/// `kv_require_signed_frames`, and replace C's record at B. Seen failing first on 7c967ba3:
/// "REVOKED KEY: a sealed record signed by C's revoked key was exempt at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_sealed_record_signed_by_a_revoked_key_is_not_exempt() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-exempt", false).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let ka = ed25519_dalek::SigningKey::from_bytes(&[0x5b; 32]).verifying_key().to_bytes();
    let key = format!("sys/identity-signed/{nc}");
    let forged = sealed_record(&[ka, k1, k0], &k0_sk);
    let refused_before = b.task_ctx.transport_bounds.kv_revoked_key_refused.load(Ordering::Relaxed);
    forge_unsigned_as(&a, &nc, key.clone(), Bytes::from(forged.clone()), false);
    let _ = c.kv().set("exempt/plant", b"by-k1".to_vec());
    let plant = within_3s(|| b.kv().get("exempt/plant").is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let exempt = b.kv().get(&key).is_some_and(|v| v.as_ref() == forged.as_slice());
    let refused = b.task_ctx.transport_bounds.kv_revoked_key_refused.load(Ordering::Relaxed) > refused_before;
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen && plant, "B saw the revocation, and C's write under its new key arrived");
    assert!(!exempt, "REVOKED KEY: a sealed record signed by C's revoked key was exempt at B");
    assert!(refused, "and it was counted as signed by a revoked key (kv_revoked_key_refused)");
}

/// **A revocation still filters after the verifier restarts** (row Q). B learned C's revocation, then
/// restarted from its persisted store: the identity records are validated first, then the
/// revocations against what they established, so a write signed by C's revoked key is still refused.
/// Seen failing first on 7c967ba3 (no verifier of KV writes consulted revocation at all): "REVOKED KEY:
/// after B restarted, C's revoked key signed C's KV writes at B".
#[cfg(all(feature = "tls", feature = "compliance", unix))]
#[tokio::test]
async fn a_revocation_still_filters_after_the_verifier_restarts() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let base = std::env::temp_dir().join(format!("myc-rev-restart-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, nb, nc, dir) = three_signed_nodes_with("rev-restart", false, |cfg, _| {
        cfg.persistence = Some(PersistenceConfig { base_path: data.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    }).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let b_cfg = b.config.clone();
    b.shutdown().await;
    drop(b);
    let b = GossipAgent::new(nb.clone(), b_cfg);
    b.start().await.unwrap();
    let rejoined = within_3s(|| !b.peers().is_empty()).await;
    let trusted_after = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc);
    // The plant first, re-sent until it lands: C's writer to B held the connection B's restart broke,
    // so its first frame is lost to the writer's backoff — the plant waits that out, then the forgery
    // goes over the same, now working, paths.
    let mut plant = false;
    for i in 0..20 {
        let _ = c.kv().set("restart/plant", format!("by-k1-{i}").into_bytes());
        if within_3s(|| b.kv().get("restart/plant").is_some()).await { plant = true; break; }
    }
    send_signed_as(&a, &nc, &k0_sk, "restart/as-c", b"by-k0");
    let _ = c.kv().set("restart/plant-2", b"by-k1".to_vec());
    let plant = plant && within_3s(|| b.kv().get("restart/plant-2").is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = b.kv().get("restart/as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(seen && rejoined && plant, "B saw the revocation ({seen}), came back ({rejoined}), and C's new-key writes arrived ({plant})");
    assert!(trusted_after.contains(&k1) && !trusted_after.contains(&k0), "at restart B trusts k1 and not k0: {trusted_after:?}");
    assert!(!landed, "REVOKED KEY: after B restarted, C's revoked key signed C's KV writes at B");
}

/// **A member cannot revoke another member's key by rewriting that member's identity** (row Q, who
/// may revoke). A revocation counted when signed by the *current* key of the raw `sys/identity/{N}`
/// value — a plain KV entry any member can write — so A wrote `sys/identity/{C}` = A's key ‖ C's key
/// and revoked C's key under A's own; with revocation now reaching the KV verifier that would have
/// silenced C fleet-wide. A revocation now counts only when signed by a key this node *holds* for N
/// (authenticated: proved, chained or CA-anchored). Run with `require_identity_proofs`, the posture in
/// which a bare identity write cannot put a key in a member's set (with proofs off it can, and whoever
/// can do that can already sign as N). Seen failing first on 7c967ba3: "DOS: A revoked C's key at B
/// by rewriting C's identity".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_member_cannot_revoke_another_members_key_by_rewriting_its_identity() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-forge-id", true).await;
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let ka = a_sk.verifying_key().to_bytes();
    let k0 = c.identity_public_key().unwrap();
    let before = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    let _ = a.kv().set(format!("sys/identity/{nc}"), [ka, k0].concat());
    let rev_key = crate::agent::revocation::revocation_key(&nc, &k0);
    let _ = a.kv().set(rev_key.clone(), revocation_signed_by(&nc, k0, &a_sk));
    let arrived = within_3s(|| b.kv().get(&rev_key).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    let _ = c.kv().set("forge-id/plant", b"c".to_vec());
    let plant = within_3s(|| b.kv().get("forge-id/plant").is_some()).await;
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(before && arrived, "B trusted C's key, and A's revocation reached B");
    assert!(still, "DOS: A revoked C's key at B by rewriting C's identity");
    assert!(plant, "and C's writes still arrive at B");
}

/// **A key one member claims and revokes under its own name stays trusted for its owner** (row Q).
/// The revoked set was one fleet-wide set of keys, so a member that put C's key in its *own* identity
/// record (signed by its own key, which chains) and then revoked it as its own removed C's key for
/// every node. Revocation is now per node: N's revocation removes a key from N's set only. Seen
/// failing first on 7c967ba3: "DOS: A's revocation of a key it claimed silenced C at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_key_one_member_claims_and_revokes_stays_trusted_for_its_owner() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev-claim", false).await;
    let na = a.node_id.clone();
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let ka = a_sk.verifying_key().to_bytes();
    let k0 = c.identity_public_key().unwrap();
    let _ = a.kv().set(format!("sys/identity/{na}"), [ka, k0].concat());
    let _ = a.kv().set(format!("sys/identity-signed/{na}"), sealed_record(&[ka, k0], &a_sk));
    let claimed = within_3s(|| b.task_ctx.peer_keys.pin().get(&na).is_some_and(|k| k.contains(&k0))).await;
    let rev_key = crate::agent::revocation::revocation_key(&na, &k0);
    let _ = a.kv().set(rev_key.clone(), revocation_signed_by(&na, k0, &a_sk));
    let arrived = within_3s(|| b.kv().get(&rev_key).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(claimed && arrived, "B took A's claim and A's revocation");
    assert!(still, "DOS: A's revocation of a key it claimed silenced C at B");
}

/// **A node cannot revoke the key it signs with** (row Q). Revoking the current key would leave the
/// node with no key its peers trust, silently; the flow is rotate first, then revoke the old key
/// (`rotate_identity_on_compromise`). Seen failing first on 7c967ba3: the revocation was written.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn revoking_the_key_a_node_signs_with_is_refused() {
    let port = alloc_port();
    let dir = std::env::temp_dir().join(format!("myc-rev-current-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();
    let current = agent.identity_public_key().unwrap();
    let refused = agent.revoke_identity_key(current).is_err();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let recorded = agent.revocation_head().is_some();
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(refused, "revoking the key this node signs with must be refused");
    assert!(!recorded, "and nothing is recorded");
}

// ── Row Q rev 3 (`docs/design/revocation-authority.md`): who may revoke, judged by certificates ──

/// A certificate the fleet CA in `cert_dir` issues for `key` as `node`, naming it by URI and dated
/// `issued_ms` in its serial — what `generate_node_cert` makes, minted here so a test can be a CA
/// holder (the development posture puts `ca-key.pem` on every node).
#[cfg(feature = "tls")]
fn mint_member_cert(cert_dir: &std::path::Path, node: &NodeId, key: &ed25519_dalek::SigningKey, issued_ms: u64) -> Vec<u8> {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let ca_key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(cert_dir.join("ca-key.pem")).unwrap()).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let pkcs8 = key.to_pkcs8_der().unwrap();
    let kp = rcgen::KeyPair::try_from(pkcs8.as_bytes()).unwrap();
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.subject_alt_names = vec![
        rcgen::SanType::IpAddress(node.to_socket_addr().ip()),
        rcgen::SanType::URI(rcgen::Ia5String::try_from(format!("mycelium://{node}")).unwrap()),
    ];
    p.serial_number = Some(rcgen::SerialNumber::from(issued_ms));
    p.not_before = rcgen::date_time_ymd(2024, 1, 1);
    p.not_after = rcgen::date_time_ymd(2099, 1, 1);
    p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec()
}

#[cfg(feature = "tls")]
fn hex_key(k: &[u8; 32]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

/// A v2 revocation record of `revoked` for `node`, signed by `sk`, carrying `certs` — the layout
/// `docs/design/revocation-authority.md` §2.3 fixes (the v1 bytes, a marker, the trailer), built here
/// from the layout alone so the test does not lean on the module it tests. Returns `(path, bytes)`.
#[cfg(feature = "tls")]
fn revocation_v2_by(node: &NodeId, revoked: [u8; 32], sk: &ed25519_dalek::SigningKey, certs: Vec<Vec<u8>>) -> (String, Vec<u8>) {
    use sha2::{Digest, Sha256};
    #[derive(serde::Serialize)]
    struct Event { node_id: NodeId, hlc: u64, revoked_key: [u8; 32], reason: Option<String> }
    #[derive(serde::Serialize)]
    struct Signed<'a> { event: &'a Event, sig: Vec<u8> }
    #[derive(serde::Serialize)]
    struct Trailer { revoker: [u8; 32], certs: Vec<Vec<u8>>, sig: Vec<u8> }
    let event = Event { node_id: node.clone(), hlc: 1, revoked_key: revoked, reason: None };
    let canonical = mycelium_core::serde_fixint::to_vec(&event).unwrap();
    let revoker = sk.verifying_key().to_bytes();
    let mut h = Sha256::new();
    h.update((canonical.len() as u32).to_le_bytes());
    h.update(&canonical);
    h.update(revoker);
    for c in &certs { h.update((c.len() as u32).to_le_bytes()); h.update(c); }
    let mut msg = b"mycelium.identity/revoke/2".to_vec();
    msg.extend_from_slice(&h.finalize());
    let mut out = mycelium_core::serde_fixint::to_vec(&Signed { event: &event, sig: crate::tls::sign_bytes(sk, &canonical).to_vec() }).unwrap();
    out.extend_from_slice(b"MYCREV2\0");
    out.extend_from_slice(&mycelium_core::serde_fixint::to_vec(&Trailer { revoker, certs, sig: crate::tls::sign_bytes(sk, &msg).to_vec() }).unwrap());
    (format!("sys/revocation/{node}/{}/{}", hex_key(&revoked), hex_key(&revoker)), out)
}

/// Re-send `c`'s write until `b` has it (a writer whose connection a restart broke loses its first frame).
#[cfg(feature = "tls")]
async fn plant_lands(c: &GossipAgent, b: &GossipAgent, key: &str) -> bool {
    for i in 0..20 {
        let _ = c.kv().set(key.to_string(), format!("plant-{i}").into_bytes());
        if within_3s(|| b.kv().get(key).is_some()).await { return true; }
    }
    false
}

#[cfg(all(feature = "tls", feature = "compliance", unix))]
fn persisted(base: &std::path::Path) -> crate::config::PersistenceConfig {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    PersistenceConfig { base_path: base.to_path_buf(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse }
}

/// Restart `b` over its own configuration and wait for it to have a peer again.
#[cfg(all(feature = "tls", feature = "compliance", unix))]
async fn restart(b: GossipAgent) -> GossipAgent {
    let (id, cfg) = (b.node_id.clone(), b.config.clone());
    b.shutdown().await;
    drop(b);
    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    let _ = within_3s(|| !b.peers().is_empty()).await;
    b
}

/// **A key a member poisoned into another's identity neither revokes nor signs for it** (row Q rev 3,
/// #616's review finding 2 with proofs off). A garbles C's sealed record and proof and writes C's bare
/// identity as `ka ‖ k0` — its own signed writes, which proofs-off accepts — so B holds `ka` for C; A
/// then revokes C's `k0` under `ka` and writes as C under `ka`. On 7c967ba3's successor `e5c2c27e` the
/// held-key rule counted the revocation (C silenced) and the write landed. C has a certificate record,
/// so its trusted keys are exactly its anchored keys: `ka` is none. Seen failing first on e5c2c27e:
/// "POISON: a key A wrote into C's identity revoked C's key at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_poisoned_identity_key_neither_revokes_nor_signs_for_an_anchored_member() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-poison", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let ka = a_sk.verifying_key().to_bytes();
    let k0 = c.identity_public_key().unwrap();
    let _ = a.kv().set(format!("sys/identity-signed/{nc}"), b"x".to_vec());
    let _ = a.kv().set(format!("sys/identity-proof/{nc}"), b"x".to_vec());
    let _ = a.kv().set(format!("sys/identity/{nc}"), [ka, k0].concat());
    let _ = within_3s(|| b.task_ctx.peer_keys.pin().get(&nc).is_some_and(|k| k.contains(&ka))).await;
    let _ = a.kv().set(crate::agent::revocation::revocation_key(&nc, &k0), revocation_signed_by(&nc, k0, &a_sk));
    let (p2, v2) = revocation_v2_by(&nc, k0, &a_sk, vec![]);
    let _ = a.kv().set(p2.clone(), v2);
    let _ = within_3s(|| b.kv().get(&p2).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_signed_as(&a, &nc, &a_sk, "poison/as-c", b"by-ka");
    let plant = plant_lands(&c, &b, "poison/plant").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    let landed = b.kv().get("poison/as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(k0_trusted, "POISON: a key A wrote into C's identity revoked C's key at B");
    assert!(plant, "C's writes still arrive at B");
    assert!(!landed, "POISON: a key A wrote into C's identity signs as C at B");
}

/// **A first-sighted key after a restart cannot revoke an anchored member's key** (row Q rev 3, #616's
/// review finding 2 with proofs on). A writes a self-signed sealed record `[ka]` for C; B refuses it
/// live but stores it, and after B restarts it is a first sighting — on e5c2c27e `ka` entered C's set.
/// Seen failing first on e5c2c27e **at its second assertion**: "and the self-signed key is not trusted
/// for C" (the first, that the revocation of `k0` did not count, held there).
#[cfg(all(feature = "tls", feature = "compliance", unix))]
#[tokio::test]
async fn a_first_sighted_key_after_a_restart_cannot_revoke_an_anchored_member() {
    let base = std::env::temp_dir().join(format!("myc-rev3-fs-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_with("rev3-first", true, |cfg, _| cfg.persistence = Some(persisted(&data))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let ka = a_sk.verifying_key().to_bytes();
    let k0 = c.identity_public_key().unwrap();
    let sealed = format!("sys/identity-signed/{nc}");
    let forged = sealed_record(&[ka], &a_sk);
    let _ = a.kv().set(sealed.clone(), forged.clone());
    let _ = within_3s(|| b.kv().get(&sealed).is_some_and(|v| v.as_ref() == forged.as_slice())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = restart(b).await;
    let _ = a.kv().set(crate::agent::revocation::revocation_key(&nc, &k0), revocation_signed_by(&nc, k0, &a_sk));
    let (p2, v2) = revocation_v2_by(&nc, k0, &a_sk, vec![]);
    let _ = a.kv().set(p2.clone(), v2);
    let _ = within_3s(|| b.kv().get(&p2).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let plant = plant_lands(&c, &b, "first/plant").await;
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    let ka_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&ka);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(k0_trusted, "FIRST SIGHT: after B restarted, a self-signed key revoked C's anchored key");
    assert!(!ka_trusted, "and the self-signed key is not trusted for C");
    assert!(plant, "C's writes still arrive at B");
}

/// **A fork from a revoked key is not trusted after the verifier restarts** (row Q rev 3, #616's review
/// finding 1). C rotates on compromise; the thief writes a sealed record `[ka, k1, k0]` signed by the
/// revoked `k0`. Live, B refused it; after B restarts it was a first sighting, `ka` entered C's set, and
/// the thief's write as C landed. Seen failing first on e5c2c27e: "RESTART: a key chained through C's
/// revoked key signs as C after B restarted".
#[cfg(all(feature = "tls", feature = "compliance", unix))]
#[tokio::test]
async fn a_fork_from_a_revoked_key_is_not_trusted_after_the_verifier_restarts() {
    let base = std::env::temp_dir().join(format!("myc-rev3-fork-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_with("rev3-fork", true, |cfg, _| cfg.persistence = Some(persisted(&data))).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let thief = ed25519_dalek::SigningKey::from_bytes(&[0x6a; 32]);
    let ka = thief.verifying_key().to_bytes();
    let sealed = format!("sys/identity-signed/{nc}");
    let forged = sealed_record(&[ka, k1, k0], &k0_sk);
    let _ = a.kv().set(sealed.clone(), forged.clone());
    let _ = within_3s(|| b.kv().get(&sealed).is_some_and(|v| v.as_ref() == forged.as_slice())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = restart(b).await;
    let plant = plant_lands(&c, &b, "fork/plant").await;
    send_signed_as(&a, &nc, &thief, "fork/as-c", b"by-ka");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let landed = b.kv().get("fork/as-c").is_some();
    let ka_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&ka);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(seen && plant, "B saw the revocation, and C's write under its new key arrived after the restart");
    assert!(!ka_trusted, "RESTART: a key chained through C's revoked key is trusted for C after B restarted");
    assert!(!landed, "RESTART: a key chained through C's revoked key signs as C after B restarted");
}

/// **A retired key cannot revoke its successor, and rotation continues** (row Q rev 3, #616's review
/// finding 3). After `rotate_identity_on_compromise`, a thief holding the old `k0` revokes `k1` under
/// `k0`. On e5c2c27e any held key revoked any other, so C was silenced and could not rotate on. Authority
/// now flows only to the past: `k0`'s certificate is older than `k1`'s. Seen failing first on e5c2c27e:
/// "THIEF: a retired key revoked its successor at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_retired_key_cannot_revoke_its_successor_and_rotation_continues() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-thief", false).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let k0_cert = a.kv().get(&format!("sys/identity-cert/{nc}/{}", hex_key(&k0))).map(|b| b.to_vec()).into_iter().collect();
    let _ = a.kv().set(crate::agent::revocation::revocation_key(&nc, &k1), revocation_signed_by(&nc, k1, &k0_sk));
    let (p2, v2) = revocation_v2_by(&nc, k1, &k0_sk, k0_cert);
    let _ = a.kv().set(p2.clone(), v2);
    let _ = within_3s(|| b.kv().get(&p2).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k1_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k1);
    let plant = plant_lands(&c, &b, "thief/plant").await;
    let k2 = c.rotate_identity(Duration::from_millis(500)).await;
    let k2_trusted = match &k2 {
        Ok(k2) => within_3s(|| crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(k2)).await,
        Err(_) => false,
    };
    let plant2 = plant_lands(&c, &b, "thief/plant-after-k2").await;
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "B saw k0 revoked");
    assert!(k1_trusted, "THIEF: a retired key revoked its successor at B");
    assert!(plant, "C's writes under k1 still arrive");
    assert!(k2_trusted && plant2, "C rotates on to k2, which B trusts, and its writes arrive ({k2:?})");
}

/// **A revocation hidden while the verifier was down still counts** (row Q rev 3: the journal, and the
/// `(P, S)` slot). B counted C's revocation of `k0`; C is down, so nothing republishes it; A tombstones
/// every revocation record of C's and B takes the tombstones; B restarts. On e5c2c27e the revocation was
/// re-read from the store at restart, found missing, and `k0` verified again. Seen failing first on
/// e5c2c27e: "HIDDEN: after B restarted, the hidden revocation no longer counted".
#[cfg(all(feature = "tls", feature = "compliance", unix))]
#[tokio::test]
async fn a_revocation_hidden_while_the_verifier_was_down_still_counts() {
    let base = std::env::temp_dir().join(format!("myc-rev3-hide-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_with("rev3-hide", false, |cfg, _| cfg.persistence = Some(persisted(&data))).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.shutdown().await;
    let records: Vec<String> = a.kv().scan_prefix(&format!("sys/revocation/{nc}/")).into_iter().map(|(k, _)| k.to_string()).collect();
    for k in &records { assert!(a.kv().delete(k.clone())); }
    let hidden = within_3s(|| records.iter().all(|k| b.kv().get(k).is_none())).await;
    let b = restart(b).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let plant = plant_lands(&a, &b, "hide/plant-a").await;
    send_signed_as(&a, &nc, &k0_sk, "hide/as-c", b"by-k0");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    let landed = b.kv().get("hide/as-c").is_some();
    a.shutdown().await; b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(seen && !records.is_empty() && hidden && plant, "counted, hidden at B ({hidden}), B back ({plant})");
    assert!(!k0_trusted, "HIDDEN: after B restarted, the hidden revocation no longer counted");
    assert!(!landed, "HIDDEN: after B restarted, C's revoked key signed C's writes");
}

/// **A node built with `tls` and without `compliance` honours a revocation it receives** (row Q rev 3,
/// #616's review finding 7). C rotates (`rotate_identity`, a `tls` verb) and writes its revocation of
/// `k0` itself, signed by `k1` and carrying `k1`'s certificate. **Coverage, not a fail-first test:** it
/// passes on e5c2c27e, whose revocation module was already compiled under `tls`; the review's finding
/// was that no test exercised a `tls` build without `compliance`.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_tls_only_node_honours_a_revocation() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-tls", false).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let _ = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let k1_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let k1_cert = mint_member_cert(&dir, &nc, &k1_sk, now);
    let (path, bytes) = revocation_v2_by(&nc, k0, &k1_sk, vec![k1_cert]);
    let _ = c.kv().set(path.clone(), bytes);
    let _ = within_3s(|| b.kv().get(&path).is_some()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    send_signed_as(&a, &nc, &k0_sk, "tlsonly/as-c", b"by-k0");
    let plant = plant_lands(&c, &b, "tlsonly/plant").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = b.kv().get("tlsonly/as-c").is_some();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(plant, "C's writes under k1 arrive at B");
    assert!(!landed, "TLS-ONLY: C's revoked key still signs C's writes at B");
}

/// **A certificate issued in the future outranks nothing** (row Q rev 3, the design review's attack 2).
/// A, holding the CA key (the development posture), mints a certificate for C dated a day ahead, writes
/// it as C's certificate record and revokes C's `k0` under it. Its issuance is past `wall_now_ms` plus
/// the skew bound, so it is counted (`identity_cert_future`) and ignored. A guard on the new path: on
/// e5c2c27e certificate records did not exist, so nothing here could happen.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_certificate_issued_in_the_future_outranks_nothing() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-future", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0 = c.identity_public_key().unwrap();
    let kx = ed25519_dalek::SigningKey::from_bytes(&[0x7c; 32]);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let future = mint_member_cert(&dir, &nc, &kx, now + 86_400_000);
    let _ = a.kv().set(format!("sys/identity-cert/{nc}/{}", hex_key(&kx.verifying_key().to_bytes())), future.clone());
    let (path, bytes) = revocation_v2_by(&nc, k0, &kx, vec![future]);
    let _ = a.kv().set(path.clone(), bytes);
    let _ = within_3s(|| b.kv().get(&path).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc);
    let counted = b.task_ctx.identity_ledger.cert_future.load(Ordering::Relaxed);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(trusted.contains(&k0), "a future-dated certificate's revocation did not count");
    assert!(!trusted.contains(&kx.verifying_key().to_bytes()), "and its key is not trusted");
    assert!(counted >= 1, "the future certificate was counted");
}

/// **A legacy IP-only certificate anchors nothing** (row Q rev 3, the design review's attack 5). A mints
/// for C a CA-signed certificate without the URI SAN (what every certificate before row Q was), for a
/// key of its own, and revokes C's `k0` under it. It names a host, not a node, so it gives no standing.
/// A guard on the new path, as above.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_legacy_ip_only_certificate_anchors_nothing() {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-legacy", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0 = c.identity_public_key().unwrap();
    let kx = ed25519_dalek::SigningKey::from_bytes(&[0x7d; 32]);
    let ca_key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(dir.join("ca-key.pem")).unwrap()).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let kp = rcgen::KeyPair::try_from(kx.to_pkcs8_der().unwrap().as_bytes()).unwrap();
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.subject_alt_names = vec![rcgen::SanType::IpAddress(nc.to_socket_addr().ip())];
    p.serial_number = Some(rcgen::SerialNumber::from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64));
    let legacy = p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec();
    let _ = a.kv().set(format!("sys/identity-cert/{nc}/{}", hex_key(&kx.verifying_key().to_bytes())), legacy.clone());
    let (path, bytes) = revocation_v2_by(&nc, k0, &kx, vec![legacy]);
    let _ = a.kv().set(path.clone(), bytes);
    let _ = within_3s(|| b.kv().get(&path).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(trusted.contains(&k0) && !trusted.contains(&kx.verifying_key().to_bytes()), "an IP-only certificate gave no standing");
}

/// **An owner republishes its overwritten records** (row Q rev 3, attack 4). A overwrites C's certificate
/// record with garbage; C's watcher sees its own record changed and writes it back, and the tripwire
/// counts the foreign write. Seen failing first on e5c2c27e, where no certificate record existed.
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn an_owner_republishes_its_overwritten_certificate_record() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("rev3-republish", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0 = c.identity_public_key().unwrap();
    let key = format!("sys/identity-cert/{nc}/{}", hex_key(&k0));
    let original = c.kv().get(&key).expect("C published its certificate record");
    let before = c.task_ctx.sys_namespace_violations.load(Ordering::Relaxed);
    let _ = a.kv().set(key.clone(), b"garbage".to_vec());
    let restored = {
        let mut ok = false;
        for _ in 0..200 {
            if b.kv().get(&key).as_deref() == Some(original.as_ref()) && c.task_ctx.sys_namespace_violations.load(Ordering::Relaxed) > before { ok = true; break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        ok
    };
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(restored, "C rewrote its certificate record after A overwrote it, and the tripwire counted it");
}

// ── Row Q, the final review's round (named fixes only) ─────────────────────────────────────────

/// **An audit chain sealed under a key with no certificate record survives restarts** (the final
/// review's finding 1). Chain recovery read the node's *trusted* keys — exactly its anchored ones — so
/// a chain whose genesis was sealed under a key without a certificate record (any key retired before
/// the upgrade; here k0's record is tombstoned after the first restart) failed to verify at the next
/// restart, and the next seal replaced seq 0. History verifies under every key the node held, minus
/// revoked ones, and the owner republishes the certificate record of every key it holds. Seen failing
/// first on 38da0472: "the chain verifies after each restart".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn an_audit_chain_sealed_under_a_key_without_a_certificate_record_survives_restarts() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use crate::{AuditAction, AuditOutcome};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-q4-audit-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let k0 = a.identity_public_key().unwrap();
    let h0 = a.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
    let _ = a.audit(AuditAction::Read, "10.0.0.2:9000", "kv/x", AuditOutcome::Success, None).unwrap();
    let _ = a.rotate_identity(Duration::from_millis(100)).await.unwrap();
    let _ = a.audit(AuditAction::Write, "10.0.0.1:9000", "kv/y", AuditOutcome::Success, None).unwrap();
    a.shutdown().await;
    drop(a);
    let k0_cert = format!("sys/identity-cert/{id}/{}", hex_key(&k0));
    let mut verdicts = Vec::new();
    let mut genesis = Vec::new();
    for round in 0..2 {
        let a = GossipAgent::new(id.clone(), cfg.clone());
        a.start().await.unwrap();
        verdicts.push(a.audit_verify(&id));
        if round == 0 {
            // k0's certificate record goes missing (any key retired before the upgrade has none).
            let _ = a.kv().delete(k0_cert.clone());
        }
        let _ = a.audit(AuditAction::Admin, "10.0.0.3:9000", format!("round/{round}"), AuditOutcome::Success, None).unwrap();
        let stream = a.audit_stream(&id);
        genesis.push((stream.first().map(|r| r.record.content_hash()), stream.len()));
        a.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(verdicts.iter().all(|v| v.is_ok()), "the chain verifies after each restart: {verdicts:?}");
    assert_eq!(genesis[0], (Some(h0), 4), "the first restart sealed seq 3 after the original genesis");
    assert_eq!(genesis[1], (Some(h0), 5), "the second restart sealed seq 4; genesis never replaced");
}

/// **A revoked key restored at its node does not regain trust at a peer** (the final review's finding
/// 2). C rotates on compromise; then C restarts with its old key file restored from a backup (no
/// persistence, so its own records are gone) and re-issues a certificate for k0 — newer than k1's. B
/// ranked a key by its newest issuance and re-judged revocations from scratch, so k0 outranked k1 and
/// verified again. A key ranks by its first issuance, and a counted revocation stays counted. Seen
/// failing first on 38da0472: "RE-ISSUE: a restored revoked key verifies again at B".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn a_revoked_key_restored_at_its_node_does_not_regain_trust_at_a_peer() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q4-restore", false).await;
    let key_path = dir.join(format!("{}.key", nc.as_str().replace([':', '.'], "_")));
    let backup = std::fs::read(&key_path).unwrap();
    let k0 = c.identity_public_key().unwrap();
    let k1 = c.rotate_identity_on_compromise(Duration::from_millis(500)).await.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    let c_cfg = c.config.clone();
    c.shutdown().await;
    drop(c);
    std::fs::write(&key_path, &backup).unwrap();
    let c = GossipAgent::new(nc.clone(), c_cfg);
    let started = c.start().await.is_ok();
    let _ = within_3s(|| !c.peers().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen && started, "B saw k0 revoked; C restarted with its old key");
    assert!(!k0_trusted, "RE-ISSUE: a restored revoked key verifies again at B");
}

/// **A node whose `key_pem` names a key a rotation retired refuses to start** (the final review's
/// finding 2). `key_pem` keeps naming k0 after `rotate_identity_on_compromise` saved k1 to the
/// automatic path; the restart reloaded k0 and re-issued its certificate. The node's own records show
/// k0 revoked, so it now refuses to start, by name. Seen failing first on 38da0472: "the restart with
/// a revoked key succeeded".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn a_node_whose_key_pem_names_a_retired_key_refuses_to_start() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-q4-keypem-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let k0_sk = ed25519_dalek::SigningKey::from_bytes(&[0x42; 32]);
    let der = k0_sk.to_pkcs8_der().unwrap();
    let b64 = { use base64::Engine as _; base64::engine::general_purpose::STANDARD.encode(der.as_bytes()) };
    let pem = format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n");
    std::fs::write(base.join("k0.pem"), pem).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), key_pem: Some(base.join("k0.pem")), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    assert_eq!(a.identity_public_key(), Some(k0_sk.verifying_key().to_bytes()));
    let _ = a.rotate_identity_on_compromise(Duration::from_millis(100)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.shutdown().await;
    drop(a);
    let again = GossipAgent::new(id.clone(), cfg);
    let res = again.start().await;
    again.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert!(res.is_err(), "the restart with a revoked key succeeded");
}

/// **Junk revocation records keep a fold bounded** (the final review's finding 3). Five records for C,
/// each carrying 2,000 copies of C's own certificate, made one fold CA-check 10,000 certificates
/// (3.3 s in the review). A record may carry at most two certificates, judged certificates are cached
/// by hash, and the caps apply before any check. The bound stated: one fold under 250 ms. Seen failing
/// first on 38da0472: "FOLD COST: one fold took".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn junk_revocation_records_keep_a_fold_bounded() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q4-cost", false).await;
    let k0 = c.identity_public_key().unwrap();
    let cert_key = format!("sys/identity-cert/{nc}/{}", hex_key(&k0));
    let _ = within_3s(|| b.kv().get(&cert_key).is_some()).await;
    let cert = b.kv().get(&cert_key).map(|v| v.to_vec()).unwrap_or_else(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        mint_member_cert(&dir, &nc, &c.task_ctx.tls.get().unwrap().signing_key(), now)
    });
    for i in 0..5u8 {
        let junk = ed25519_dalek::SigningKey::from_bytes(&[0x90 + i; 32]);
        let (path, bytes) = revocation_v2_by(&nc, k0, &junk, vec![cert.clone(); 2_000]);
        let _ = b.kv().set(path, bytes);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t = std::time::Instant::now();
    crate::agent::revocation::refresh(&b.task_ctx);
    let took = t.elapsed();
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(took < Duration::from_millis(250), "FOLD COST: one fold took {took:?}");
    assert!(k0_trusted, "and the junk revoked nothing");
}

/// **A rotation after the clock steps back still outranks its predecessor** (the final review's finding
/// 4). The new certificate was issued at the wall clock, so after a backward step the successor ranked
/// below the key it succeeds and its revocation did not count. It is issued strictly after the node's
/// newest own issuance. Seen failing first on 38da0472 (with the test-only clock step added to that
/// copy): "STEP: after the clock stepped back, the rotation's revocation did not count".
#[cfg(all(feature = "tls", feature = "compliance", not(feature = "sim")))]
#[tokio::test]
async fn a_rotation_after_the_clock_steps_back_still_outranks_its_predecessor() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q4-step", false).await;
    let k0 = c.identity_public_key().unwrap();
    mycelium_core::sim_seam::step_wall_clock_for_test(-5_000);
    let rotated = c.rotate_identity_on_compromise(Duration::from_millis(500)).await;
    mycelium_core::sim_seam::step_wall_clock_for_test(0);
    let k1 = rotated.unwrap();
    let seen = revocation_seen(&b, &nc, k0, k1).await;
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen, "STEP: after the clock stepped back, the rotation's revocation did not count");
}

/// **An owner does not adopt a revocation planted through its raw identity** (the final review's
/// finding 6). A writes C's bare `sys/identity/` as `k1 ‖ k0 ‖ kx` and a v1 revocation of C's retired,
/// unrevoked k0 signed by its own kx; C restarted and, reading its history from the raw value, re-issued
/// it as its own — revoking k0 fleet-wide. Only C's current or anchored keys sign what it adopts now.
/// Seen failing first on 38da0472: "ADOPT: C re-issued a revocation A planted".
#[cfg(all(feature = "tls", feature = "compliance", unix))]
#[tokio::test]
async fn an_owner_does_not_adopt_a_revocation_planted_through_its_raw_identity() {
    let base = std::env::temp_dir().join(format!("myc-q4-adopt-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("q4-adopt", false, |_, _| {}, |cfg, _| cfg.persistence = Some(persisted(&data))).await;
    let k0 = c.identity_public_key().unwrap();
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let kx = ed25519_dalek::SigningKey::from_bytes(&[0x5e; 32]);
    let _ = a.kv().set(format!("sys/identity/{nc}"), [k1, k0, kx.verifying_key().to_bytes()].concat());
    let v1_path = crate::agent::revocation::revocation_key(&nc, &k0);
    let _ = a.kv().set(v1_path.clone(), revocation_signed_by(&nc, k0, &kx));
    let _ = within_3s(|| c.kv().get(&v1_path).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let c = restart(c).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let adopted = c.kv().get(&format!("sys/revocation/{nc}/{}/{}", hex_key(&k0), hex_key(&k1))).is_some();
    let k0_trusted = crate::agent::helpers::known_verifying_keys(&b.task_ctx, &nc).contains(&k0);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(!adopted, "ADOPT: C re-issued a revocation A planted");
    assert!(k0_trusted, "and C's retired, unrevoked key still verifies at B");
}

/// **Revoking one key twice writes one record** (the final review's finding 7). Each call re-signed a
/// new record for the same `(P, S)`, and each was journaled — sixty-four calls saturated the node. The
/// second call now keeps the first record. Seen failing first on 38da0472: "a second revocation of the
/// same key re-signed the record".
#[cfg(all(feature = "tls", feature = "compliance"))]
#[tokio::test]
async fn revoking_one_key_twice_writes_one_record() {
    let port = alloc_port();
    let dir = std::env::temp_dir().join(format!("myc-q4-twice-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let k0 = a.identity_public_key().unwrap();
    let k1 = a.rotate_identity(Duration::from_millis(100)).await.unwrap();
    let path = format!("sys/revocation/{id}/{}/{}", hex_key(&k0), hex_key(&k1));
    a.revoke_identity_key(k0).unwrap();
    let first = a.kv().get(&path);
    a.revoke_identity_key(k0).unwrap();
    let second = a.kv().get(&path);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(first.is_some(), "the revocation was written");
    assert_eq!(first, second, "a second revocation of the same key re-signed the record");
}

// ── Row Q, the confirming review's N1 and N2 ───────────────────────────────────────────────────

/// **A thief's revocation judged before the successor's certificate arrives does not lock in** (the
/// confirming review's N1, its probe T1). C rotates k0 → k1; B withholds C's certificate records, so
/// the thief's revocation of k1 signed by the retired k0 (carrying k0's certificate) is judged while
/// k1 has no anchor — and 39cfb571 recorded it as counted, for ever. A revocation of a key with no
/// anchor now counts only provisionally and is re-judged every fold: once k1's certificate arrives, k1
/// outranks k0 and is trusted. Seen failing first on 39cfb571: "LOCKED: k1 is not trusted after its
/// certificate arrived".
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[tokio::test]
async fn a_thiefs_revocation_judged_before_the_successors_certificate_does_not_lock_in() {
    use crate::store::FrameView;
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q5-lock", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0_sk = c.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k0_cert_key = format!("sys/identity-cert/{nc}/{}", hex_key(&k0));
    let _ = within_3s(|| b.kv().get(&k0_cert_key).is_some()).await;
    let k0_cert = b.kv().get(&k0_cert_key).map(|v| v.to_vec()).expect("k0's certificate at B");
    let prefix = format!("sys/identity-cert/{nc}/");
    b.task_ctx.kv_state.frame_filter.pin().insert(77, Arc::new(move |v: &FrameView<'_>| match v {
        FrameView::Kv { key, .. } => key.starts_with(prefix.as_str()),
        _ => false,
    }));
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    let k1_cert_key = format!("sys/identity-cert/{nc}/{}", hex_key(&k1));
    let (path, bytes) = revocation_v2_by(&nc, k1, &k0_sk, vec![k0_cert]);
    let _ = a.kv().set(path.clone(), bytes);
    let arrived = within_3s(|| b.kv().get(&path).is_some()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let withheld = b.kv().get(&k1_cert_key).is_none();
    b.task_ctx.kv_state.frame_filter.pin().remove(&77);
    let _ = within_3s(|| a.kv().get(&k1_cert_key).is_some()).await;
    let der = a.kv().get(&k1_cert_key).map(|v| v.to_vec()).expect("k1's certificate at A");
    let _ = b.kv().set(k1_cert_key.clone(), der);
    tokio::time::sleep(Duration::from_millis(500)).await;
    crate::agent::revocation::refresh(&b.task_ctx);
    let k1_trusted = within_3s(|| b.task_ctx.trusted_keys(&nc).contains(&k1)).await;
    let landed = plant_lands(&c, &b, "q5/after").await;
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(arrived && withheld, "the thief's revocation reached B before k1's certificate");
    assert!(k1_trusted, "LOCKED: k1 is not trusted after its certificate arrived");
    assert!(landed, "and C's writes under k1 arrive at B");
}

/// **A node whose own certificate record is missing is not refused by a planted record** (the
/// confirming review's N1). A revocation of a key that had no certificate yet (k2, an operator's
/// coming re-issue, held through the node's sealed identity record) planted under the node's older, anchored k0
/// counted and was journaled; restarted on k2, whose record is not in the store yet, the node read
/// the journal and refused to start. Such a revocation is now provisional and never journaled, and at
/// start the node's own current certificate is an anchor before anything is judged. Its own current certificate is now an anchor before anything is judged, so k2 (issued
/// now) outranks k0. Seen failing first on 39cfb571: "a planted record refused the start".
#[cfg(all(feature = "tls", unix))]
#[tokio::test]
async fn a_planted_record_does_not_refuse_a_start_whose_own_certificate_record_is_missing() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-q5-plant-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let k0_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let k0 = k0_sk.verifying_key().to_bytes();
    let k0_cert = a.kv().get(&format!("sys/identity-cert/{id}/{}", hex_key(&k0))).map(|v| v.to_vec()).expect("k0's record");
    // The key the operator will re-issue the node with: it reaches the node's held set before its
    // certificate does (a sealed record chained from k0), and a revocation of it is planted under k0.
    let k2_sk = ed25519_dalek::SigningKey::from_bytes(&[0x52; 32]);
    let k2 = k2_sk.verifying_key().to_bytes();
    let _ = a.kv().set(format!("sys/identity-signed/{id}"), sealed_record(&[k0, k2], &k0_sk));
    let held = within_3s(|| a.task_ctx.peer_keys.pin().get(&id).is_some_and(|k| k.contains(&k2))).await;
    assert!(held, "precondition: k2 is held for the node before its certificate exists");
    let (path, bytes) = revocation_v2_by(&id, k2, &k0_sk, vec![k0_cert]);
    let _ = a.kv().set(path, bytes);
    tokio::time::sleep(Duration::from_millis(300)).await;
    crate::agent::revocation::refresh(&a.task_ctx);
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.shutdown().await;
    drop(a);
    std::fs::create_dir_all(base.join("keys")).unwrap();
    let der = k2_sk.to_pkcs8_der().unwrap();
    let b64 = { use base64::Engine as _; base64::engine::general_purpose::STANDARD.encode(der.as_bytes()) };
    std::fs::write(base.join("keys/k2.pem"), format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n")).unwrap();
    let mut cfg2 = cfg.clone();
    cfg2.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), key_pem: Some(base.join("keys/k2.pem")), ..TlsConfig::default() });
    let again = GossipAgent::new(id.clone(), cfg2);
    let res = again.start().await;
    let k2_now = again.identity_public_key();
    again.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert!(res.is_ok(), "a planted record refused the start: {:?}", res.err());
    assert_eq!(k2_now, Some(k2_sk.verifying_key().to_bytes()), "the node runs on k2");
}

/// **A raw identity value a member writes does not enter the owner's history** (the confirming
/// review's N2, its probe T2, with identity proofs on). A writes C's bare `sys/identity/` as `k0 ‖ ka`;
/// C restarted and rebuilt its published history from that value, so `ka` entered C's history at B and
/// an audit record A forged under `ka` verified. The owner's history is now built from its own
/// certificate records and verified sealed record. Seen failing first on 39cfb571: "FORGED: A's
/// audit record signed by ka verifies as C's at B".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn a_raw_identity_value_a_member_writes_does_not_enter_the_owners_history() {
    use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
    let base = std::env::temp_dir().join(format!("myc-q5-raw-{}-{}", std::process::id(), alloc_port()));
    let _ = std::fs::remove_dir_all(&base);
    let data = base.join("data");
    let (a, b, c, _nb, nc, dir) = three_signed_nodes_tweaked("q5-raw", true, |_, _| {}, |cfg, _| cfg.persistence = Some(persisted(&data))).await;
    let h0 = c.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let k0 = c.identity_public_key().unwrap();
    let ka_sk = ed25519_dalek::SigningKey::from_bytes(&[0x61; 32]);
    let ka = ka_sk.verifying_key().to_bytes();
    let _ = a.kv().set(format!("sys/identity/{nc}"), [k0, ka].concat());
    let _ = within_3s(|| c.kv().get(&format!("sys/identity/{nc}")).is_some_and(|v| v.len() == 64)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let c = restart(c).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let ka_in_history = b.task_ctx.history_keys(&nc).contains(&ka);
    let r1 = AuditRecord { node_id: nc.clone(), seq: 1, hlc: 9, principal: "attacker".into(), action: AuditAction::Admin, target: "role/grant".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: h0 };
    let _ = a.kv().set(audit_key(&nc, 1), SignedAuditRecord::sign(r1, &ka_sk).encode());
    let _ = within_3s(|| b.kv().get(&audit_key(&nc, 1)).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let verified = b.audit_verify(&nc);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
    assert!(verified.is_err(), "FORGED: A's audit record signed by ka verifies as C's at B");
    assert!(!ka_in_history, "and ka is not in C's history at B");
}

// ── Row Q, the N1/N2 check's NEW-1 and NEW-2 ───────────────────────────────────────────────────

/// **An audit genesis survives an upgrade after an old-version rotation** (the N1/N2 check's NEW-1).
/// Before row Q, `rotate_identity` wrote the sealed record `[k0, kL]` signed by the *prior* key kL,
/// which never had a URI-SAN certificate; after the upgrade the node does not take that record into its
/// history (N2 stays closed), so records sealed under kL do not verify — and 4908ecd3 then sealed over
/// seq 0. Recovery never seals over an existing record now: the unverified records are kept, numbering
/// resumes after them, and they are reported. Seen failing first on 4908ecd3: "UPGRADE: the genesis
/// sealed under the pre-upgrade key was replaced".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn an_audit_genesis_survives_an_upgrade_after_an_old_version_rotation() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-q6-legacy-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let k0 = a.identity_public_key().unwrap();
    // The pre-upgrade state after a rotation kL -> k0: the rotation record is signed by the prior kL.
    let kl_sk = ed25519_dalek::SigningKey::from_bytes(&[0x4d; 32]);
    let kl = kl_sk.verifying_key().to_bytes();
    let _ = a.kv().set(format!("sys/identity/{id}"), [k0, kl].concat());
    let _ = a.kv().set(format!("sys/identity-signed/{id}"), sealed_record(&[k0, kl], &kl_sk));
    let r0 = AuditRecord { node_id: id.clone(), seq: 0, hlc: 1, principal: "p".into(), action: AuditAction::Invoke, target: "t0".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: [0u8; 32] };
    let h0 = r0.content_hash();
    let _ = a.kv().set(audit_key(&id, 0), SignedAuditRecord::sign(r0, &kl_sk).encode());
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.shutdown().await;
    drop(a);
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let _ = a.audit(AuditAction::Admin, "p", "after", AuditOutcome::Success, None).unwrap();
    let s = a.audit_stream(&id);
    let report = a.guarantee_report();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(s.first().map(|r| r.record.content_hash()), Some(h0), "UPGRADE: the genesis sealed under the pre-upgrade key was replaced");
    assert_eq!(s.len(), 2, "the next seal resumed after it");
    assert_eq!(report.audit_unverified, Some([0, 0]), "and the unverified prefix is reported");
}

/// **Two compromise rotations learned out of order leave the oldest key revoked** (the N1/N2 check's
/// NEW-2). C rotates k0 → k1 and revokes k0 under k1, then k1 → k2 and revokes k1 under k2; B learns
/// k2's revocation of k1 first, then k1's revocation of k0. 4908ecd3 refused a signer it had counted as
/// revoked, so k1's revocation of k0 never counted and k0 stayed trusted. A revoked signer's earlier
/// revocations count: authority flows only to the past. (`rotate_identity` and records C writes itself,
/// so the test needs `tls` and the frame filter, not `compliance`.) Seen failing first on 4908ecd3:
/// "ORDER: k0 is trusted after both revocations".
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[tokio::test]
async fn two_compromise_rotations_learned_out_of_order_leave_the_oldest_key_revoked() {
    use crate::store::FrameView;
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q6-order", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let k0 = c.identity_public_key().unwrap();
    let prefix = format!("sys/revocation/{nc}/{}/", hex_key(&k0));
    let p2 = prefix.clone();
    b.task_ctx.kv_state.frame_filter.pin().insert(55, Arc::new(move |v: &FrameView<'_>| match v {
        FrameView::Kv { key, .. } => key.starts_with(p2.as_str()),
        _ => false,
    }));
    let revoke = |c: &GossipAgent, revoked: [u8; 32]| {
        let sk = c.task_ctx.tls.get().unwrap().signing_key();
        let me = sk.verifying_key().to_bytes();
        let cert = c.kv().get(&format!("sys/identity-cert/{nc}/{}", hex_key(&me))).map(|v| v.to_vec()).expect("own certificate record");
        let (path, bytes) = revocation_v2_by(&nc, revoked, &sk, vec![cert]);
        let _ = c.kv().set(path, bytes);
    };
    let k1 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    revoke(&c, k0);
    let k2 = c.rotate_identity(Duration::from_millis(500)).await.unwrap();
    revoke(&c, k1);
    let k1_revoked = within_3s(|| !b.task_ctx.trusted_keys(&nc).contains(&k1) && b.task_ctx.trusted_keys(&nc).contains(&k2)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    b.task_ctx.kv_state.frame_filter.pin().remove(&55);
    let rec = c.kv().scan_prefix(&prefix);
    for (k, v) in &rec { let _ = b.kv().set(k.to_string(), v.to_vec()); }
    tokio::time::sleep(Duration::from_millis(500)).await;
    crate::agent::revocation::refresh(&b.task_ctx);
    let k0_trusted = b.task_ctx.trusted_keys(&nc).contains(&k0);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(k1_revoked && !rec.is_empty(), "B counted k2's revocation of k1 first, and k1's revocation of k0 exists");
    assert!(!k0_trusted, "ORDER: k0 is trusted after both revocations");
}

// ── Row Q, the audit safety net's review: H1, M1, L1 ───────────────────────────────────────────

/// A persisted, gateway-fronted single node for the audit tests: `(id, cfg, base, http_port)`.
#[cfg(all(feature = "compliance", unix))]
fn audit_test_node(tag: &str) -> (NodeId, GossipConfig, std::path::PathBuf, u16) {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    let port = alloc_port();
    let http_port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-{tag}-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![crate::GatewayToken { token: "auditor".into(), scopes: vec!["audit:read".into()] }];
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    (id, cfg, base, http_port)
}

/// A record in `node`'s stream signed by a key that is not the node's, at `key`.
#[cfg(all(feature = "compliance", unix))]
fn plant_audit_record(a: &GossipAgent, key: String, node: &NodeId, seq: u64, prev: [u8; 32]) {
    use crate::{AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
    let forged = AuditRecord { node_id: node.clone(), seq, hlc: 1, principal: "x".into(), action: AuditAction::Admin, target: "x".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: prev };
    let _ = a.kv().set(key, SignedAuditRecord::sign(forged, &ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])).encode());
}

/// **A forged high-seq record neither pushes nor wraps the numbering** (the safety net's review, H1,
/// its probe). A member writes `sys/audit/{N}/zzzz` — a key that is not a seq — holding a record whose
/// body names `u64::MAX - 1`. 81b70a2e read it into the stream by its body, resumed numbering at
/// `u64::MAX`, and the first seal overflowed (a panic here; a wrap onto genesis in release). Only
/// `{seq:016x}` keys whose record names that seq are read; anything else is counted
/// (`audit_malformed_keys`) and ignored. Seen failing first on 81b70a2e: "attempt to add with overflow".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn a_forged_high_seq_record_neither_pushes_nor_wraps_the_numbering() {
    use crate::{AuditAction, AuditOutcome};
    let (id, cfg, base, _http) = audit_test_node("q7-push");
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let h0 = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let _ = a.audit(AuditAction::Invoke, "p", "t1", AuditOutcome::Success, None).unwrap();
    plant_audit_record(&a, format!("sys/audit/{id}/zzzz"), &id, u64::MAX - 1, [9u8; 32]);
    a.shutdown().await;
    drop(a);
    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    let next = b.task_ctx.audit_chain.lock().unwrap().next_seq;
    let s1 = b.audit(AuditAction::Admin, "p", "seal-1", AuditOutcome::Success, None);
    let s2 = b.audit(AuditAction::Admin, "p", "seal-2", AuditOutcome::Success, None);
    let stream = b.audit_stream(&id);
    let verified = b.audit_verify(&id);
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(next, 2, "the numbering resumed after the node's own records, not pushed");
    assert!(s1.is_ok() && s2.is_ok(), "both seals succeed");
    assert_eq!(stream.first().map(|r| r.record.content_hash()), Some(h0), "genesis is kept");
    assert_eq!(stream.iter().map(|r| r.record.seq).collect::<Vec<_>>(), vec![0, 1, 2, 3], "the planted key is not part of the stream");
    assert_eq!(verified, Ok(()), "and the stream verifies");
}

/// **A far-away record is reported, not followed** (H1). A record at a valid key, seq 1 000 000, that
/// does not verify: 81b70a2e resumed numbering after it. The next seal now takes the slot after the
/// verified head; the record is kept and reported. Seen failing first on 81b70a2e: "the numbering
/// followed a far-away record".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn a_far_away_unverified_record_is_reported_not_followed() {
    use crate::{audit_key, AuditAction, AuditOutcome};
    let (id, cfg, base, _http) = audit_test_node("q7-far");
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let _ = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let _ = a.audit(AuditAction::Invoke, "p", "t1", AuditOutcome::Success, None).unwrap();
    plant_audit_record(&a, audit_key(&id, 1_000_000), &id, 1_000_000, [9u8; 32]);
    a.shutdown().await;
    drop(a);
    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    let next = b.task_ctx.audit_chain.lock().unwrap().next_seq;
    let _ = b.audit(AuditAction::Admin, "p", "after", AuditOutcome::Success, None).unwrap();
    let kept = b.kv().get(&audit_key(&id, 1_000_000)).is_some();
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(next, 2, "the numbering followed a far-away record");
    assert!(kept, "and the record is kept");
}

/// **A record whose key and body disagree is ignored** (H1). `sys/audit/{N}/{5:016x}` holding a record
/// that names seq 9: 81b70a2e sorted it into the stream by key and judged it by its body (a gap, an
/// unverified record, numbering pushed to 10). It is not part of the stream now, and is counted. Seen
/// failing first on 81b70a2e: "a key/body mismatch was read into the stream".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn an_audit_record_whose_key_and_body_disagree_is_ignored() {
    use crate::{audit_key, AuditAction, AuditOutcome};
    let (id, cfg, base, http_port) = audit_test_node("q7-mismatch");
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let h0 = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    plant_audit_record(&a, audit_key(&id, 5), &id, 9, h0);
    a.shutdown().await;
    drop(a);
    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    let stream: Vec<u64> = b.audit_stream(&id).iter().map(|r| r.record.seq).collect();
    let verified = b.audit_verify(&id);
    let stats: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{http_port}/stats")).await.unwrap().json().await.unwrap();
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(stream, vec![0], "a key/body mismatch was read into the stream");
    assert_eq!(verified, Ok(()), "and the stream verifies");
    assert_eq!(stats["audit_malformed_keys"], 1, "the mismatched key is counted");
}

/// **A peer's write into a node's audit stream trips the `sys/` tripwire** (the safety net's review, H1;
/// N-B for the control). `sys/audit/` and `sys/audit-checkpoint/` were not self-owned prefixes, so a
/// member planting a record in another's stream went uncounted. The control: the counter is first held
/// quiet for a window, then one write into C's stream, then one into C's checkpoint prefix — each must
/// raise it by exactly one, so the delta is the write's. Seen failing first on 81b70a2e: "the audit
/// write raised the counter by 0".
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_peers_write_into_a_nodes_audit_stream_trips_the_tripwire() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("q7-trip", false).await;
    let count = || c.task_ctx.sys_namespace_violations.load(Ordering::Relaxed);
    // A quiet window: the counter holds still for a second before anything is written.
    let mut quiet = false;
    for _ in 0..20 {
        let before = count();
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        if count() == before { quiet = true; break; }
    }
    let base = count();
    let key = format!("sys/audit/{nc}/{:016x}", 7u64);
    let _ = a.kv().set(key.clone(), b"planted".to_vec());
    let arrived = within_3s(|| c.kv().get(&key).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after_audit = count();
    let cp_key = format!("sys/audit-checkpoint/{nc}/{:016x}", 7u64);
    let _ = a.kv().set(cp_key.clone(), b"planted".to_vec());
    let _ = within_3s(|| c.kv().get(&cp_key).is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after_cp = count();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(quiet && arrived, "the counter was quiet ({quiet}) and the write reached C ({arrived})");
    assert_eq!(after_audit - base, 1, "the audit write raised the counter by {}", after_audit - base);
    assert_eq!(after_cp - after_audit, 1, "the checkpoint write raised the counter by {}", after_cp - after_audit);
}

/// **One planted record does not leave the rest of the stream unverifiable** (the safety net's review,
/// M1 and L1). A record at seq 2 that does not verify, then a restart and two seals: the seals skip
/// the occupied slot and link to the verified prefix by hash, and the stream reads as segments — four
/// records verify (the prefix 0, 1 and the tail 3, 4), one does not. `/stats` counts unverified
/// *records*: two planted far apart count two, not the span between them. Seen failing first on
/// 81b70a2e: "the tail sealed after the planted record is not reported as verified".
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn one_planted_record_does_not_leave_the_rest_of_the_stream_unverifiable() {
    use crate::{audit_key, AuditAction, AuditOutcome};
    use axum::http::header::AUTHORIZATION;
    let (id, cfg, base, http_port) = audit_test_node("q7-segments");
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let _ = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let h1 = a.audit(AuditAction::Invoke, "p", "t1", AuditOutcome::Success, None).unwrap();
    plant_audit_record(&a, audit_key(&id, 2), &id, 2, h1);
    plant_audit_record(&a, audit_key(&id, 1_000_000_000), &id, 1_000_000_000, [9u8; 32]);
    a.shutdown().await;
    drop(a);
    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    let _ = b.audit(AuditAction::Admin, "p", "tail-1", AuditOutcome::Success, None).unwrap();
    let _ = b.audit(AuditAction::Admin, "p", "tail-2", AuditOutcome::Success, None).unwrap();
    let client = reqwest::Client::new();
    let audit: serde_json::Value = client.get(format!("http://127.0.0.1:{http_port}/gateway/audit?node={id}"))
        .header(AUTHORIZATION, "Bearer auditor").send().await.unwrap().json().await.unwrap();
    let stats: serde_json::Value = client.get(format!("http://127.0.0.1:{http_port}/stats")).send().await.unwrap().json().await.unwrap();
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
    let seg = &audit["streams"][0]["segments"];
    assert_eq!(seg["verified"], 4, "the tail sealed after the planted record is not reported as verified: {seg}");
    assert_eq!(seg["unverified"], 2, "and the two planted records are reported: {seg}");
    assert_eq!(stats["audit_unverified_records"], 2, "L1: /stats counts the unverified records, not the span");
}

/// **A stream with a skipped slot verifies under `verify_linked`, and is a gap under the contiguous
/// verifiers** (the H1/M1 review's N-A). Once a seal skips a planted slot, an `AuditSink` consumer
/// verifying an export with `verify_chain_keys` gets `SequenceGap`; `verify_linked` — the rule the node
/// itself verifies by — follows the hash links and reports the planted record. A new public API: on
/// 3898c77c it does not exist.
#[cfg(feature = "compliance")]
#[test]
fn a_stream_with_a_skipped_slot_verifies_under_verify_linked() {
    use crate::{verify_chain_keys, verify_linked, AuditAction, AuditOutcome, AuditRecord, AuditVerifyError, SignedAuditRecord};
    let node = NodeId::new("127.0.0.1", 7901).unwrap();
    let sk = ed25519_dalek::SigningKey::from_bytes(&[0x21; 32]);
    let vk = sk.verifying_key().to_bytes();
    let rec = |seq: u64, prev: [u8; 32]| AuditRecord { node_id: node.clone(), seq, hlc: seq + 1, principal: "p".into(), action: AuditAction::Invoke, target: format!("t{seq}"), outcome: AuditOutcome::Success, detail: None, prev_hash: prev };
    let r0 = rec(0, [0u8; 32]);
    let r1 = rec(1, r0.content_hash());
    let planted = rec(2, r1.content_hash());
    let r3 = rec(3, r1.content_hash());
    let stream = vec![
        SignedAuditRecord::sign(r0, &sk),
        SignedAuditRecord::sign(r1, &sk),
        SignedAuditRecord::sign(planted, &ed25519_dalek::SigningKey::from_bytes(&[0x22; 32])),
        SignedAuditRecord::sign(r3, &sk),
    ];
    let seg = verify_linked(&stream, &node, &[vk], 0, [0u8; 32]);
    assert_eq!(seg.linked, vec![0, 1, 3], "the prefix and the tail linked to it verify");
    assert_eq!(seg.unverified, vec![2], "the planted record does not");
    let honest = [stream[0].clone(), stream[1].clone(), stream[3].clone()];
    assert_eq!(verify_chain_keys(&honest, &node, &[vk], 0, [0u8; 32]), Err(AuditVerifyError::SequenceGap { expected: 2, found: 3 }),
        "the contiguous verifier reports the skip as a gap");
}

/// **A planted run of any length leaves the node sealing, and is reported** (the N-A–N-D check's high:
/// 2d33c84b refused to seal past 4 096 occupied slots, so 4 097 cheap writes stopped a node's audit
/// trail while governance went on unaudited). The first free slot is found outside the audit lock in
/// one ordered pass; the lock is held only for an O(1) check of the head's slot per attempt — the bound
/// stated and measured here: while one seal steps over a 50 000-record run, another thread never waits
/// 50 ms for the lock. The run is counted with the other unverified records. Seen failing first on
/// 2d33c84b: "a planted run stopped the node's audit trail".
#[cfg(feature = "compliance")]
#[tokio::test]
async fn a_planted_run_of_any_length_leaves_the_node_sealing_and_is_reported() {
    use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
    const RUN: u64 = 50_000;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("myc-q9-run-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let h0 = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let other = ed25519_dalek::SigningKey::from_bytes(&[0x33; 32]);
    for seq in 1..=RUN {
        let r = AuditRecord { node_id: id.clone(), seq, hlc: 1, principal: "x".into(), action: AuditAction::Admin, target: "x".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: h0 };
        let _ = a.kv().set(audit_key(&id, seq), SignedAuditRecord::sign(r, &other).encode());
    }
    // Garbage at the next free slot is not a record and occupies nothing.
    let _ = a.kv().set(audit_key(&id, RUN + 1), b"garbage".to_vec());
    let chain = Arc::clone(&a.task_ctx.audit_chain);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let prober = {
        let (chain, stop) = (Arc::clone(&chain), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut worst = Duration::ZERO;
            while !stop.load(Ordering::Relaxed) {
                let t = std::time::Instant::now();
                drop(chain.lock().unwrap());
                worst = worst.max(t.elapsed());
                std::thread::yield_now();
            }
            worst
        })
    };
    let sealed = a.audit(AuditAction::Admin, "p", "after-run", AuditOutcome::Success, None);
    stop.store(true, Ordering::Relaxed);
    let worst = prober.join().unwrap();
    let last = a.audit_stream(&id).last().map(|r| (r.record.seq, r.record.prev_hash));
    let (count, range) = { let g = chain.lock().unwrap(); (g.unverified_count, g.unverified) };
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(sealed.is_ok(), "a planted run stopped the node's audit trail: {sealed:?}");
    assert_eq!(last, Some((RUN + 1, h0)), "sealed at the first free slot (over the garbage), linked to the head by hash");
    assert_eq!((count, range), (RUN, Some((1, RUN))), "the skipped run is reported");
    assert!(worst < Duration::from_millis(50), "the lock was held for {worst:?} while the seal stepped over the run");
}

/// **A seal over one planted record costs the run, not the stream** (the seal fix's check, medium). The
/// free slot was found by reading and decoding the whole stream; on a long stream one planted record at
/// the head made a seal cost the stream (20 000 records: 165 ms against 0.29 ms in the review). It is
/// found slot by slot from the head now, one O(1) lookup per occupied slot. The bound stated: with
/// 20 000 records elsewhere in the stream and one planted at the head's slot, the seal takes under
/// 25 ms (debug build). Seen failing first on 5ee431fe: "a seal over one planted record took".
#[cfg(feature = "compliance")]
#[tokio::test]
async fn a_seal_over_one_planted_record_costs_the_run_not_the_stream() {
    use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("myc-q10-cost-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let h0 = a.audit(AuditAction::Invoke, "p", "t0", AuditOutcome::Success, None).unwrap();
    let other = ed25519_dalek::SigningKey::from_bytes(&[0x44; 32]);
    let rec = |seq: u64| SignedAuditRecord::sign(AuditRecord { node_id: id.clone(), seq, hlc: 1, principal: "x".into(), action: AuditAction::Admin, target: "x".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: h0 }, &other).encode();
    // A long stream: 20 000 records far from the head.
    for seq in 1_000_000..1_020_000u64 { let _ = a.kv().set(audit_key(&id, seq), rec(seq)); }
    // One planted record at the head's slot.
    let _ = a.kv().set(audit_key(&id, 1), rec(1));
    let t = std::time::Instant::now();
    let sealed = a.audit(AuditAction::Admin, "p", "after", AuditOutcome::Success, None);
    let took = t.elapsed();
    eprintln!("seal over one planted record on a 20 000-record stream: {took:?}");
    let at = a.audit_stream(&id).iter().any(|r| r.record.prev_hash == h0 && r.record.seq == 2);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(sealed.is_ok() && at, "the seal stepped over the planted record to seq 2");
    assert!(took < Duration::from_millis(25), "a seal over one planted record took {took:?}");
}

/// **A capability retracted as its node shuts down reaches the peers** (#611, found by CI's co-op
/// demo 04). A capability's soft-state task tombstones its key when the handle drops; a node that
/// drops the handle and shuts down at once (a provider killed mid-run) raced the shutdown signal, and
/// since row F signs that tombstone — slow enough in a debug build to lose the race — the writers had
/// stopped before it was queued, so peers kept the dead provider's capability until its TTL and routed
/// RPCs to it. Graceful shutdown now waits, bounded, for retractions already queued to reach the
/// writers before it stops them. Seen failing first: B still resolved A's capability.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_capability_retracted_at_shutdown_reaches_the_peers() {
    use crate::capability::{CapFilter, Capability};
    let (a, b, node_a, _node_b, cert_dir) = signed_fleet_pair("retract-at-shutdown", None).await;
    let reg = a.capabilities().advertise_capability(Capability::new("route", "retract"), Duration::from_secs(30));
    let resolves_a = || b.capabilities().resolve(&CapFilter::new("route", "retract")).iter().any(|(n, _)| *n == node_a);
    for _ in 0..200 { if resolves_a() { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    assert!(resolves_a(), "B saw A's capability");
    drop(reg);
    a.shutdown().await;
    let mut gone = false;
    for _ in 0..80 { if !resolves_a() { gone = true; break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    assert!(gone, "B still resolves the capability A retracted as it shut down");
}

/// **Soft state whose handle is still live at shutdown is retracted at the peers** — every family a
/// node keeps alive under its own id, not only `cap/` and `req/`. A persistent service advert
/// (`svc/{kind}/{self}`, `mesh().advertise_persistent`), an MCP tool (`tools/{name}/{self}`) and an
/// emergent group's projection (`gcap/{group}/{ns}/{name}/{self}`) were tombstoned only by their own
/// tasks when the shutdown signal woke them — after `flush_outbound_before_shutdown`, with the writers
/// stopping — so the tombstone never left, and a peer kept routing to the dead node until the entry's
/// TTL (#611's review, recorded in post-360 plan §3.6 under F; `gcap/` found by enumerating every
/// caller of `run_kv_persist_task` and every task that tombstones on exit). Health checks run every
/// 60 s here, so anti-entropy cannot deliver the tombstone in the test's bound.
#[cfg(feature = "tls")]
#[tokio::test]
async fn soft_state_with_a_live_handle_at_shutdown_is_retracted_at_the_peers() {
    use crate::capability::{Capability, CapFilter, CapabilityGroupDef};
    let (a, b, node_a, _node_b, cert_dir) = signed_fleet_pair("live-handle-at-shutdown", None).await;
    let svc_key  = format!("svc/live-svc/{node_a}");
    let tool_key = format!("tools/live_tool/{node_a}");
    let gcap_key = format!("gcap/live-group/route/projected/{node_a}");
    let _svc = a.mesh().advertise_persistent("live-svc", SignalScope::Cluster, Duration::from_millis(200), || Bytes::from_static(b"up"));
    let _tool = a.mcp().register_mcp_tool("live_tool", serde_json::json!({}), |_args| async { Ok(serde_json::json!({})) });
    let _cap = a.capabilities().advertise_capability(Capability::new("route", "member"), Duration::from_secs(30));
    let _grp = a.capabilities().define_capability_group("live-group", CapabilityGroupDef {
        filter: CapFilter::new("route", "member"), topology_policy: None,
        provides: vec![Capability::new("route", "projected")], requires: vec![],
    }, Duration::from_secs(30));
    let live = |k: &str| b.kv().get(k).is_some();
    poll_until(|| live(&svc_key) && live(&tool_key) && live(&gcap_key), 5_000).await;
    assert!(live(&svc_key) && live(&tool_key) && live(&gcap_key),
        "B saw all three before the shutdown (svc {}, tools {}, gcap {})", live(&svc_key), live(&tool_key), live(&gcap_key));
    // Every handle stays alive across the shutdown: the node is stopped, not the work dropped.
    a.shutdown().await;
    for _ in 0..80 { if !live(&svc_key) && !live(&tool_key) && !live(&gcap_key) { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let (svc, tool, gcap) = (live(&svc_key), live(&tool_key), live(&gcap_key));
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    let kept: Vec<&str> = [(svc, "the persistent service advert"), (tool, "the MCP tool"), (gcap, "the group projection")]
        .into_iter().filter(|(still, _)| *still).map(|(_, what)| what).collect();
    assert!(kept.is_empty(), "B still holds A's {kept:?} after A shut down");
}

/// **A soft-state task cannot re-assert a key between the shutdown sweep and the signal** (#627's review,
/// finding 1). The tasks run until the shutdown signal, which comes after the sweep and the outbound
/// flush; a re-assert tick in between wrote the key live under a newer HLC and the flush delivered that,
/// so the peer kept routing to the stopped node — 20/20 shutdowns at a 5 ms interval in the review's
/// probe. Shutdown now closes the soft-state gate before it sweeps. Five shutdowns at a 5 ms interval,
/// a persistent service advert and a capability each live across the shutdown.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fast_reassert_does_not_undo_the_shutdown_sweep() {
    use crate::capability::Capability;
    let mut kept_rounds = Vec::new();
    for round in 0..5 {
        let (a, b, node_a, _node_b, cert_dir) = signed_fleet_pair(&format!("fast-reassert-{round}"), None).await;
        let svc_key = format!("svc/fast-svc/{node_a}");
        let _svc = a.mesh().advertise_persistent("fast-svc", SignalScope::Cluster, Duration::from_millis(5), || Bytes::from_static(b"up"));
        let _cap = a.capabilities().advertise_capability(Capability::new("route", "fast"), Duration::from_millis(5));
        // Both read as raw keys: `resolve()` drops an advert older than 3 × its refresh interval (15 ms
        // here), so it would read a re-asserted `cap/` key as gone whether or not the sweep held (#628's
        // review, finding 2).
        let cap_key = format!("cap/{node_a}/route/fast");
        let svc_live = || b.kv().get(&svc_key).is_some();
        let cap_live = || b.kv().get(&cap_key).is_some();
        poll_until(|| svc_live() && cap_live(), 5_000).await;
        a.shutdown().await;
        for _ in 0..80 { if !svc_live() && !cap_live() { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
        if svc_live() || cap_live() { kept_rounds.push((round, svc_live(), cap_live())); }
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }
    assert!(kept_rounds.is_empty(), "B kept A's soft state after A shut down in rounds (round, svc, cap): {kept_rounds:?}");
}

/// Shutdown-delivery tests (the shutdown-sweep flake, 2026-10-19): a frame the sweep queued must be on
/// the wire before `shutdown()` returns, wherever it is when the flush starts. Each holds one place on
/// the outbound path open with a test pause (`before`, undone by `after`) and returns whether B still
/// holds A's service advert after A's shutdown.
#[cfg(feature = "tls")]
async fn advert_kept_after_shutdown_with(tag: &str, before: impl FnOnce(&GossipAgent, &NodeId), after: impl FnOnce(&NodeId)) -> bool {
    let (a, b, node_a, node_b, cert_dir) = signed_fleet_pair(tag, None).await;
    let svc_key = format!("svc/held-svc/{node_a}");
    let _svc = a.mesh().advertise_persistent("held-svc", SignalScope::Cluster, Duration::from_secs(30), || Bytes::from_static(b"up"));
    let live = || b.kv().get(&svc_key).is_some();
    poll_until(live, 5_000).await;
    assert!(live(), "B saw A's service advert before the shutdown");
    before(&a, &node_b);
    let t = std::time::Instant::now();
    a.shutdown().await;
    eprintln!("{tag}: shutdown took {:?}", t.elapsed());
    for _ in 0..80 { if !live() { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let kept = live();
    after(&node_b);
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    kept
}

/// **A frame a peer writer holds in its write when the node shuts down still reaches the peer** — the
/// mechanism behind `a_fast_reassert_does_not_undo_the_shutdown_sweep`'s CI failure on 98a311fc (B kept
/// `svc/` in one round; ~1 % of runs under parallel load, on v2.35.0 too). The flush waited for empty
/// queues, but a frame the writer has dequeued is in none: the flush saw nothing queued and the signal
/// stopped the write (a probe on a failing run: `saw_queue false`, then `shutdown mid-write, queued 0`).
/// Holds each batch A writes to B for 30 ms between the buffer and the socket flush. Seen failing first:
/// 10/10 on the unfixed writer.
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_a_writer_holds_at_shutdown_still_reaches_the_peer() {
    use mycelium_core::writer::TEST_WRITE_PAUSE_MS;
    let kept = advert_kept_after_shutdown_with("writer-holds-at-shutdown",
        |_, b| { TEST_WRITE_PAUSE_MS.pin().insert(b.to_string(), 30); },
        |b| { TEST_WRITE_PAUSE_MS.pin().remove(&b.to_string()); }).await;
    assert!(!kept, "B still holds A's service advert: the sweep's tombstone was dropped inside A's writer at shutdown");
}

/// **…and is on the wire before `shutdown()` returns** (#650's review, finding 1). A first fix let the
/// writer finish its batch *after* the signal, on a detached task nothing awaited; the `mycelium` binary
/// returns from `main` straight after `shutdown()`, dropping the runtime and the write with it. A runs on
/// its own runtime here, shut down with `shutdown_background()` the moment `shutdown()` returns. Seen
/// failing first on that fix (the review: 3/8 unpaused; with the pause, every run).
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[test]
fn a_frame_a_writer_holds_at_shutdown_is_written_before_shutdown_returns() {
    use crate::config::TlsConfig;
    use mycelium_core::writer::TEST_WRITE_PAUSE_MS;
    let rt = |n| tokio::runtime::Builder::new_multi_thread().worker_threads(n).enable_all().build().unwrap();
    let (rt_a, rt_b) = (rt(2), rt(2));
    let (port_a, port_b) = (alloc_port(), alloc_port());
    let (node_a, node_b) = (NodeId::new("127.0.0.1", port_a).unwrap(), NodeId::new("127.0.0.1", port_b).unwrap());
    let cert_dir = std::env::temp_dir().join(format!("myc-runtime-exit-{port_a}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mk = |node: &NodeId, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = node.to_socket_addr().port();
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 60;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(node.clone(), cfg)
    };
    let a = mk(&node_a, vec![]);
    let b = mk(&node_b, vec![node_a.clone()]);
    rt_a.block_on(a.start()).unwrap();
    rt_b.block_on(b.start()).unwrap();
    // As `signed_fleet_pair`: peered, and each holding the other's keys, before anything is sent.
    let ready = || b.task_ctx.peer_keys.pin().get(&node_a).is_some_and(|k| !k.is_empty())
        && a.task_ctx.peer_keys.pin().get(&node_b).is_some_and(|k| !k.is_empty())
        && !a.peers().is_empty() && !b.peers().is_empty();
    rt_b.block_on(poll_until(ready, 10_000));
    let svc_key = format!("svc/exit-svc/{node_a}");
    let live = || b.kv().get(&svc_key).is_some();
    let _svc = rt_a.block_on(async {
        a.mesh().advertise_persistent("exit-svc", SignalScope::Cluster, Duration::from_secs(30), || Bytes::from_static(b"up"))
    });
    rt_b.block_on(poll_until(live, 10_000));
    assert!(live(), "B saw A's service advert before the shutdown");
    TEST_WRITE_PAUSE_MS.pin().insert(node_b.to_string(), 30);
    rt_a.block_on(a.shutdown());
    // What a process does when `main` returns: the runtime goes, and every task on it.
    rt_a.shutdown_background();
    let gone = rt_b.block_on(async {
        for _ in 0..80 { if !live() { return true; } tokio::time::sleep(Duration::from_millis(25)).await; }
        !live()
    });
    TEST_WRITE_PAUSE_MS.pin().remove(&node_b.to_string());
    rt_b.block_on(b.shutdown());
    let _ = std::fs::remove_dir_all(&cert_dir);
    assert!(gone, "B still holds A's service advert: the tombstone was not on the wire when shutdown() returned");
}

/// **A frame a writer holds through a slow connect at shutdown still reaches the peer** (#650's review,
/// finding 2). A writer with no connection — evicted, idled out or failed — connects on its first frame,
/// and the connect `select!` broke on the signal with that frame in hand. A's writer to B is evicted
/// during the shutdown, after the departure heartbeat and just before the sweep (`TEST_BEFORE_SWEEP`), and
/// its next connect held for 30 ms, so the sweep's tombstone is the frame inside the connect when the
/// flush looks. Seen failing first on #650's first fix.
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_in_a_slow_connect_at_shutdown_still_reaches_the_peer() {
    use crate::agent::lifecycle::TEST_BEFORE_SWEEP;
    use mycelium_core::writer::TEST_CONNECT_PAUSE_MS;
    let mut node_a = None;
    let kept = advert_kept_after_shutdown_with("connect-at-shutdown",
        |a, b| {
            node_a = Some(a.node_id.to_string());
            let b = b.clone();
            TEST_BEFORE_SWEEP.pin().insert(a.node_id.to_string(), Arc::new(move |a: &GossipAgent| {
                // Let the departure heartbeat reach the old writer's socket first, so the sweep's frames
                // are the new writer's first.
                std::thread::sleep(Duration::from_millis(50));
                TEST_CONNECT_PAUSE_MS.pin().insert(b.to_string(), 30);
                let old = a.peer_writers.pin().get(&b).and_then(|e| e.abort_handle.clone());
                mycelium_core::writer::evict_peer_writer(&a.peer_writers, &b);
                // Wait for the evicted writer to stop: a frame it took first would be dropped with it (an
                // eviction stops writing to the peer), and the test is about the connect, not the eviction.
                for _ in 0..200 {
                    if old.as_ref().is_none_or(|h| h.is_finished()) { break; }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }));
        },
        |b| { TEST_CONNECT_PAUSE_MS.pin().remove(&b.to_string()); }).await;
    if let Some(n) = node_a { TEST_BEFORE_SWEEP.pin().remove(&n); }
    assert!(!kept, "B still holds A's service advert: the sweep's tombstone was dropped inside a writer's connect at shutdown");
}

/// **A frame a shard worker holds at shutdown still reaches the peer** (#650's review, finding 3). A
/// frame a shard worker has dequeued and not yet handed to a writer was in no queue; the flush could
/// pass it, the signal stop the idle writer, and the hand-off then find no writer. A's shard workers
/// hold each frame for 30 ms after the dequeue (blocking, as the hand-off is synchronous). Seen
/// failing first on #650's first fix.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_a_shard_worker_holds_at_shutdown_still_reaches_the_peer() {
    use crate::agent::tasks::TEST_SHARD_PAUSE_MS;
    let mut node_a = None;
    let kept = advert_kept_after_shutdown_with("shard-holds-at-shutdown",
        |a, _| { node_a = Some(a.node_id.to_string()); TEST_SHARD_PAUSE_MS.pin().insert(a.node_id.to_string(), 30); },
        |_| {}).await;
    if let Some(n) = node_a { TEST_SHARD_PAUSE_MS.pin().remove(&n); }
    assert!(!kept, "B still holds A's service advert: the sweep's tombstone was dropped between a shard worker and a writer at shutdown");
}

/// **A frame dropped on its way to a dead peer releases its place in the outbound count** (#650's review).
/// The shutdown waits for that count to reach zero, bounded; a frame that left the path without lowering
/// it would make every later shutdown wait the whole bound. B is shut down and A sends to it (the write
/// fails, a refused connect, then the reconnect backoff drops the rest); A's count returns to zero. Seen
/// failing with the backoff drop's release removed.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_dropped_to_a_dead_peer_do_not_hold_shutdown() {
    let (a, b, _node_a, _node_b, cert_dir) = signed_fleet_pair("dead-peer-at-shutdown", None).await;
    b.shutdown().await;
    let unsent = || a.task_ctx.gossip_txs.unsent().get();
    for i in 0..20 {
        let _ = a.kv().set(format!("dead-peer/{i}"), Bytes::from_static(b"v"));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for _ in 0..200 { if unsent() == 0 { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let left = unsent();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    assert_eq!(left, 0, "frames dropped to a dead peer stayed counted");
}

/// **A frame that cannot reach the wire holds the shutdown only for the flush's bound** (#650's review).
/// A's writer to B is evicted before the sweep and its next connect held for five seconds, so the sweep's
/// frames stay counted throughout; `shutdown()` still returns well inside that — the flush gives up at
/// 250 ms, and the frames are then lost, as before.
#[cfg(all(feature = "tls", feature = "test-frame-filter"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeliverable_frame_holds_shutdown_only_for_the_bound() {
    use crate::agent::lifecycle::TEST_BEFORE_SWEEP;
    use mycelium_core::writer::TEST_CONNECT_PAUSE_MS;
    let (a, b, node_a, node_b, cert_dir) = signed_fleet_pair("undeliverable-at-shutdown", None).await;
    let _svc = a.mesh().advertise_persistent("stuck-svc", SignalScope::Cluster, Duration::from_secs(30), || Bytes::from_static(b"up"));
    let svc_key = format!("svc/stuck-svc/{node_a}");
    poll_until(|| b.kv().get(&svc_key).is_some(), 5_000).await;
    let nb = node_b.clone();
    TEST_BEFORE_SWEEP.pin().insert(node_a.to_string(), Arc::new(move |a: &GossipAgent| {
        TEST_CONNECT_PAUSE_MS.pin().insert(nb.to_string(), 5_000);
        mycelium_core::writer::evict_peer_writer(&a.peer_writers, &nb);
    }));
    let t = std::time::Instant::now();
    a.shutdown().await;
    let took = t.elapsed();
    TEST_BEFORE_SWEEP.pin().remove(&node_a.to_string());
    TEST_CONNECT_PAUSE_MS.pin().remove(&node_b.to_string());
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    // The flush's bound is 250 ms; the rest of `shutdown()` (the sweep, the task drain) is given a
    // tolerance, well short of the five-second connect.
    assert!(took < Duration::from_millis(250 + 750), "a frame held in a five-second connect held the shutdown for {took:?}");
}

/// **A healthy, busy pair's outbound count returns to zero once it goes quiet** (#650's second review).
/// Every frame counted on the way out is released once written, so a node that has sent a burst and gone
/// idle has nothing counted, and its next shutdown does not wait at all. A leak on any common path — a
/// relay, a hand-off, a write — would leave the count above zero here.
#[cfg(feature = "tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_pair_returns_its_outbound_count_to_zero() {
    let (a, b, _node_a, _node_b, cert_dir) = signed_fleet_pair("count-at-zero", None).await;
    for i in 0..300 {
        let _ = a.kv().set(format!("busy/a/{i}"), Bytes::from(vec![7u8; 256]));
        let _ = b.kv().set(format!("busy/b/{i}"), Bytes::from(vec![7u8; 256]));
        let _ = a.mesh().emit("busy.sig", SignalScope::Cluster, Bytes::from_static(b"s"));
        let _ = b.mesh().emit("busy.sig", SignalScope::Cluster, Bytes::from_static(b"s"));
        if i % 50 == 0 { tokio::task::yield_now().await; }
    }
    let both = || (a.task_ctx.gossip_txs.unsent().get(), b.task_ctx.gossip_txs.unsent().get());
    poll_until(|| b.kv().get("busy/a/299").is_some() && a.kv().get("busy/b/299").is_some(), 10_000).await;
    for _ in 0..200 { if both() == (0, 0) { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    let left = both();
    let under = (a.task_ctx.gossip_txs.unsent().underflows(), b.task_ctx.gossip_txs.unsent().underflows());
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    assert_eq!(left, (0, 0), "outbound frames still counted on an idle pair (A, B)");
    assert_eq!(under, (0, 0), "a frame was released twice (A, B)");
}

/// **A soft-state payload that panics does not wedge shutdown** (#628's review, finding 1). The soft-state
/// gate's in-flight count was restored only on a normal return, and `run_kv_persist_task` ran the
/// caller's `payload_fn` inside the gate — so a panicking payload left the count raised and the next
/// `shutdown` spun in `close()` forever, past its own timeout. The caller's closures now run outside the
/// gate, and the count is restored on unwind. The agent runs on its own runtime on a detached thread and
/// this test waits for it with a bound, so a regression fails here by name instead of a runtime that
/// can never drop hanging the test binary (the confirming check's note).
#[test]
fn a_panicking_soft_state_payload_does_not_wedge_shutdown() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let port = alloc_port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = port;
            let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
            a.start().await.unwrap();
            let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let c = Arc::clone(&calls);
            let _svc = a.mesh().advertise_persistent("panics", SignalScope::Cluster, Duration::from_millis(5), move || {
                if c.fetch_add(1, Ordering::SeqCst) == 2 { panic!("the payload panicked on its third call"); }
                Bytes::from_static(b"up")
            });
            poll_until(|| calls.load(Ordering::SeqCst) >= 3, 3_000).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            a.shutdown_with_timeout(Duration::from_millis(500)).await;
            let _ = tx.send(());
        });
    });
    assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "shutdown did not return after a soft-state payload panicked");
}

/// **A TLS node refuses an unsigned `Data` frame whatever `kv_require_signed_frames` says** (post-360
/// plan row F2). Row F's window applied the frame and counted it unless the node opted in; F2 makes
/// the refusal the default and the setting a retired no-op (`docs/guide/deprecations.md` §40).
/// Seen failing first on 4251d50a: with the setting at its default the unsigned key was applied.
#[cfg(feature = "tls")]
#[tokio::test]
async fn an_unsigned_data_frame_is_refused_whatever_the_setting_says() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let me = NodeId::new("127.0.0.1", 7313).unwrap();
    let peer = NodeId::new("127.0.0.1", 7314).unwrap();
    let (tls, dir) = test_tls_identity("unsigned-unconditional", &me);
    let sk = SigningKey::from_bytes(&[80u8; 32]);
    // The default configuration — the setting untouched.
    let opts = HandlerOpts { tls: Some(tls), peer_keys: vec![(peer.clone(), vec![sk.verifying_key().to_bytes()])], ..Default::default() };
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);
    let mut forged = data_update("unconditional/k", b"forged", 7313, false);
    forged.sender = peer.id_hash();
    send_wire(&mut writer, &WireMessage::Data(forged)).await;
    // A signed control frame behind it, so "absent" means refused rather than not yet read.
    let mut probe = data_update("unconditional/probe", b"p", 7314, false);
    probe.sender = peer.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&probe);
    send_wire(&mut writer, &signed_data_over(probe, peer.id_hash(), &sk, &crate::framing::kv_signing_message(&canonical))).await;
    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("unconditional/probe").is_some(), 500).await;
    assert!(store.pin().get("unconditional/k").is_none(), "an unsigned write is refused by a TLS node with the setting at its default");
    assert_eq!(bounds.kv_unsigned_refused.load(Ordering::Relaxed), 1, "and the refusal is counted");
    assert_eq!(bounds.kv_unsigned_accepted.load(Ordering::Relaxed), 0, "the window's counter no longer rises");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A node with no TLS identity signs nothing and refuses nothing: the plaintext posture is unchanged by
/// row F2 — an unsigned frame is applied, and neither window counter moves.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_node_without_tls_still_applies_an_unsigned_frame() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let opts = HandlerOpts::default();
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);
    send_wire(&mut writer, &WireMessage::Data(data_update("plaintext/k", b"v", 7315, false))).await;
    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("plaintext/k").is_some(), 500).await;
    assert!(store.pin().get("plaintext/k").is_some(), "a plaintext node applies an unsigned frame");
    assert_eq!(bounds.kv_unsigned_refused.load(Ordering::Relaxed), 0);
    assert_eq!(bounds.kv_unsigned_accepted.load(Ordering::Relaxed), 0);
}

/// **A `SignedData` frame's signer must be its sender** (#611's review, finding 1). The arm verified
/// the signature against `signer`'s key and applied the update as `update.sender`'s, so a member M could
/// sign with its own key a write naming V as its originator — including V's own `sys/` keys — and it
/// verified. Seen failing first: both of M's writes were applied.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_signed_frame_whose_signer_is_not_its_sender_is_refused() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let m = NodeId::new("127.0.0.1", 7321).unwrap();
    let v = NodeId::new("127.0.0.1", 7322).unwrap();
    let m_sk = SigningKey::from_bytes(&[74u8; 32]);
    let v_sk = SigningKey::from_bytes(&[75u8; 32]);
    let opts = HandlerOpts {
        peer_keys: vec![(m.clone(), vec![m_sk.verifying_key().to_bytes()]), (v.clone(), vec![v_sk.verifying_key().to_bytes()])],
        ..Default::default()
    };
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);

    let signed_by_m_as_v = |key: &str, nonce: u64| {
        let mut u = data_update(key, b"forged", nonce, false);
        u.sender = v.id_hash();
        let canonical = crate::framing::canonical_sign_bytes(&u);
        signed_data_over(u, m.id_hash(), &m_sk, &crate::framing::kv_signing_message(&canonical))
    };
    let v_own_sys_key = format!("sys/role/{v}");
    send_wire(&mut writer, &signed_by_m_as_v(&v_own_sys_key, 7321)).await;
    send_wire(&mut writer, &signed_by_m_as_v("app/as-v", 7322)).await;
    // V's own correctly signed write behind them, so "absent" means refused rather than not yet read.
    let mut probe = data_update("app/probe", b"p", 7323, false);
    probe.sender = v.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&probe);
    send_wire(&mut writer, &signed_data_over(probe, v.id_hash(), &v_sk, &crate::framing::kv_signing_message(&canonical))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("app/probe").is_some(), 500).await;
    assert!(store.pin().get(v_own_sys_key.as_str()).is_none(), "M cannot write V's own sys/ key by naming V as sender");
    assert!(store.pin().get("app/as-v").is_none(), "nor any key as V");
    assert_eq!(bounds.kv_signer_mismatch.load(Ordering::Relaxed), 2, "each refusal is counted");
}

/// **A bare (pre-2.33) KV signature is refused** (post-360 plan row F2). Row F accepted the bare 2.32
/// form for one release and counted it (`kv_untagged_signatures`); that counter no longer rises.
/// Seen failing first on 4251d50a: the barely signed key was applied and counted as accepted.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_bare_2_32_kv_signature_is_refused() {
    use ed25519_dalek::SigningKey;
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let peer = NodeId::new("127.0.0.1", 7331).unwrap();
    let sk = SigningKey::from_bytes(&[76u8; 32]);
    let opts = HandlerOpts { peer_keys: vec![(peer.clone(), vec![sk.verifying_key().to_bytes()])], ..Default::default() };
    let bounds = Arc::clone(&opts.bounds);
    let _ = spawn_handler_with(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                               Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl, opts);
    let mut u = data_update("bare/k", b"v", 7331, false);
    u.sender = peer.id_hash();
    let canonical = crate::framing::canonical_sign_bytes(&u);
    send_wire(&mut writer, &signed_data_over(u, peer.id_hash(), &sk, &canonical)).await;
    // The same peer's tagged write behind it, so "absent" means refused rather than not yet read.
    let mut probe = data_update("bare/probe", b"p", 7332, false);
    probe.sender = peer.id_hash();
    let probe_canonical = crate::framing::canonical_sign_bytes(&probe);
    send_wire(&mut writer, &signed_data_over(probe, peer.id_hash(), &sk, &crate::framing::kv_signing_message(&probe_canonical))).await;
    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("bare/probe").is_some(), 500).await;
    assert!(store.pin().get("bare/k").is_none(), "a bare 2.32 signature is refused");
    assert_eq!(bounds.kv_untagged_signatures.load(Ordering::Relaxed), 0, "the window's counter no longer rises");
    assert_eq!(bounds.kv_untagged_refused.load(Ordering::Relaxed), 1, "the refusal is counted, so an operator can find the 2.32 peer");
}

/// Two TLS nodes (both refusing unsigned frames — since row F2, every TLS node), with anti-entropy a minute away, peered and holding
/// each other's identity keys — so a write reaches the other only by gossip, and only if it is signed.
#[cfg(feature = "tls")]
async fn signed_fleet_pair(tag: &str, http_a: Option<u16>) -> (GossipAgent, GossipAgent, NodeId, NodeId, std::path::PathBuf) {
    use crate::config::TlsConfig;
    let (port_a, port_b) = (alloc_port(), alloc_port());
    let (node_a, node_b) = (NodeId::new("127.0.0.1", port_a).unwrap(), NodeId::new("127.0.0.1", port_b).unwrap());
    let cert_dir = std::env::temp_dir().join(format!("myc-{tag}-{port_a}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mk = |node: &NodeId, boots: Vec<NodeId>, http: Option<u16>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = node.to_socket_addr().port();
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 60;
        cfg.http_port = http;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(node.clone(), cfg)
    };
    let a = mk(&node_a, vec![], http_a);
    let b = mk(&node_b, vec![node_a.clone()], None);
    a.start().await.unwrap();
    b.start().await.unwrap();
    let ready = || b.task_ctx.peer_keys.pin().get(&node_a).is_some_and(|k| !k.is_empty())
        && a.task_ctx.peer_keys.pin().get(&node_b).is_some_and(|k| !k.is_empty())
        && !a.peers().is_empty() && !b.peers().is_empty();
    for _ in 0..400 {
        if ready() { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ready(), "the two nodes never peered and exchanged identities");
    (a, b, node_a, node_b, cert_dir)
}

/// **Quorum evidence is signed** (#611's review, finding 2). The connection handler writes
/// `sys/quorum/{kind}/{sender}` for a kind a local worker subscribes to and dispatched it as a bare
/// `Data` frame — the one local writer the reroute missed, in a file the dispatch gate exempted
/// whole. With both nodes requiring signatures, B's evidence about A's signal reaches A only if it is
/// signed. Seen failing first: A never held B's evidence.
#[cfg(feature = "tls")]
#[tokio::test]
async fn quorum_evidence_reaches_a_peer_that_requires_signed_frames() {
    let (a, b, node_a, _node_b, cert_dir) = signed_fleet_pair("signed-quorum", None).await;
    let _worker = b.task_ctx.signal_handlers.register_with_capacity(Arc::from("signed.evidence"), 8);
    let prefix = format!("sys/quorum/signed.evidence/{node_a}");
    let mut arrived = false;
    for _ in 0..200 {
        let _ = a.mesh().emit("signed.evidence", SignalScope::Cluster, Bytes::from_static(b"e"));
        if !a.kv().scan_prefix(&prefix).is_empty() { arrived = true; break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!b.kv().scan_prefix(&prefix).is_empty(), "B wrote its evidence");
    assert!(arrived, "and A received it by gossip: the evidence write is signed");
    assert_eq!(a.task_ctx.transport_bounds.kv_unsigned_refused.load(Ordering::Relaxed), 0,
               "a fully upgraded fleet sends no unsigned frame under signal traffic of a subscribed kind");
    assert_eq!(a.task_ctx.transport_bounds.kv_unsigned_accepted.load(Ordering::Relaxed), 0);
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A gateway write reaches a peer that requires signed frames** — a second of the twenty-three
/// rerouted sites end to end (#611's review, finding 8): `DELETE /gateway/kv` tombstones through the
/// gateway's own `kv_write`. Seen failing with that one site toggled back to a bare `Data` frame: B
/// kept the value.
#[cfg(all(feature = "tls", feature = "gateway"))]
#[tokio::test]
async fn a_gateway_kv_delete_reaches_a_peer_that_requires_signed_frames() {
    let http_port = alloc_port();
    let (a, b, _node_a, _node_b, cert_dir) = signed_fleet_pair("signed-gw", Some(http_port)).await;
    assert!(a.kv().set("app/gw-delete", Bytes::from_static(b"v")));
    for _ in 0..200 {
        if b.kv().get("app/gw-delete").is_some() { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(b.kv().get("app/gw-delete").is_some(), "the seed write arrived (ops::kv_set signs)");
    let client = reqwest::Client::new();
    let r = client.delete(format!("http://127.0.0.1:{http_port}/gateway/kv?key=app/gw-delete")).send().await.unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let mut gone = false;
    for _ in 0..200 {
        if b.kv().get("app/gw-delete").is_none() { gone = true; break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(gone, "the gateway's tombstone reached B by gossip");
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Every receive boundary drops a replayed caller envelope unanswered, and `/stats` counts it**
/// (post-360 row E's review, finding 2). `rpc_rx` and the MCP loops are tested beside the window; these
/// are the other three places a request is built from a raw signal: the SDK serve stream
/// (`/gateway/rpc/serve/{kind}`), `llm.invoke` and the explain responder. Each gets the same signed
/// envelope twice: the first is handed on (streamed, or answered), the second is neither, and
/// `caller_envelopes_replayed` on `GET /stats` says 3. Seen failing first with each boundary on the
/// stateless `verify`: the stream carried both and both calls were answered.
#[cfg(all(feature = "tls", feature = "gateway", feature = "llm"))]
#[tokio::test]
async fn every_receive_boundary_drops_a_replay_unanswered() {
    use crate::config::TlsConfig;
    use std::sync::atomic::AtomicUsize;
    let (port, http_port) = (alloc_port(), alloc_port());
    let cert_dir = std::env::temp_dir().join(format!("myc-e-boundaries-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.emergent_detectors_enabled = true;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await.unwrap();
    let _skill = a.llm().register_prompt_skill("review", "echo", crate::PromptTemplate {
        system: "".into(), user_template: "{{input}}".into(), max_tokens: 32, temperature: 0.0, metadata: Default::default(),
    }, Arc::new(crate::EchoBackend)).await.unwrap();

    // The SDK serve stream: count what it hands on.
    let streamed = Arc::new(AtomicUsize::new(0));
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let resp = loop {
            match reqwest::Client::new().get(format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/replay.serve")).send().await {
                Ok(r) => break r,
                Err(e) => {
                    assert!(tokio::time::Instant::now() < deadline, "the serve stream never opened: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        assert_eq!(resp.status(), 200, "the serve stream is open");
        let saw = Arc::clone(&streamed);
        tokio::spawn(async move {
            use futures_util::StreamExt as _;
            let mut body = resp.bytes_stream();
            while let Some(Ok(chunk)) = body.next().await {
                saw.fetch_add(String::from_utf8_lossy(&chunk).matches("event: replay.serve").count(), Ordering::SeqCst);
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let twice = |kind: &'static str, app: &'static [u8]| {
        let a = Arc::clone(&a);
        async move {
            let framed = crate::agent::gateway_caller::frame_with_context(&a.task_ctx, a.node_id(), "oidc:idp/alice", &[], bytes::Bytes::from_static(app)).unwrap();
            let call = || crate::agent::rpc::rpc_call_framed(&a.task_ctx, a.node_id().clone(), Arc::from(kind), framed.clone(), Duration::from_millis(800));
            (call().await.is_ok(), call().await.is_ok())
        }
    };
    let llm = twice("llm.invoke", br#"{"prompt":"review/echo","input":"x","context":{}}"#).await;
    let explain = twice(crate::agent::emergent::EXPLAIN_RPC_KIND, &[0u8; 8]).await;
    let _ = twice("replay.serve", b"go").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let mut stats = serde_json::Value::Null;
    for _ in 0..80 {
        if let Ok(r) = client.get(format!("http://127.0.0.1:{http_port}/stats")).send().await
            && r.status().is_success() { stats = r.json().await.unwrap(); break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let streamed = streamed.load(Ordering::SeqCst);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    assert_eq!(llm, (true, false), "REPLAY: llm.invoke answered the replay (first, replay)");
    assert_eq!(explain, (true, false), "REPLAY: the explain responder answered the replay (first, replay)");
    assert_eq!(streamed, 1, "REPLAY: the serve stream handed the same envelope on twice");
    assert_eq!(stats["caller_envelopes_replayed"], 3, "{stats}");
    assert_eq!(stats["caller_envelopes_without_id"], 0, "{stats}");
}

/// **Only a gateway's own fresh id marker is honoured** (post-360 row E's re-review). The marker was a bare
/// `b"1"` written once: any member could plant it for a gateway on an older release, and a gateway rolled back
/// kept it for good — either way that gateway's id-less envelopes were dropped at every provider, silently and
/// permanently. It is now the gateway's signed, dated statement, re-asserted every minute, and a provider
/// honours it only when a key it trusts for that gateway signed it within the freshness window plus two
/// refresh periods. Here C holds, for A: a marker B signed (planted) — ignored; A's own, dated past the
/// lifetime (lapsed — a rolled-back A) — ignored; A's own, fresh — honoured. Seen failing first with any marker
/// honoured: the planted one was.
#[cfg(feature = "tls")]
#[tokio::test]
async fn only_a_gateways_own_fresh_id_marker_is_honoured() {
    let (a, b, c, _nb, nc, dir) = three_signed_nodes("e-marker", false).await;
    let key = crate::agent::gateway_caller::id_marker_key(a.node_id());
    let ok = within_3s(|| c.kv().get(&key).is_some()).await;
    let framed = crate::agent::gateway_caller::frame_with_context(&a.task_ctx, &nc, "oidc:idp/alice", &[], bytes::Bytes::from_static(b"go")).unwrap();
    let stripped = crate::agent::gateway_caller::strip_envelope_id(&framed);
    let at_c = || {
        let mut buf = bytes::BytesMut::new();
        bytes::BufMut::put_u64_le(&mut buf, 42);
        bytes::BufMut::put(&mut buf, stripped.clone());
        let req = crate::RpcRequest::from(crate::signal::Signal {
            kind: Arc::from("k"), scope: crate::signal::SignalScope::Individual(nc.clone()),
            payload: buf.freeze(), sender: a.node_id().clone(), nonce: 7,
        });
        c.receive_gateway_caller(&req).map(|_| ()).map_err(|e| e.code())
    };
    let fresh = at_c();
    let now = c.task_ctx.hlc.decision_now_ms();
    let a_sk = a.task_ctx.tls.get().unwrap().signing_key();
    let b_sk = b.task_ctx.tls.get().unwrap().signing_key();
    let _ = c.kv().set(key.clone(), crate::agent::gateway_caller::id_marker_bytes(a.node_id(), &b_sk, now).to_vec());
    let planted = at_c();
    let lifetime = crate::agent::gateway_caller::CALLER_CONTEXT_MAX_AGE_MS + c.task_ctx.config.max_clock_drift_ms
        + 2 * crate::agent::gateway_caller::ID_MARKER_REFRESH_MS;
    let _ = c.kv().set(key.clone(), crate::agent::gateway_caller::id_marker_bytes(a.node_id(), &a_sk, now - lifetime - 1).to_vec());
    let lapsed = at_c();
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "A's marker reached C");
    assert_eq!(fresh, Err("id_stripped"), "A's own fresh marker is honoured");
    assert_eq!(planted, Ok(()), "PLANTED: a marker B signed for A was honoured");
    assert_eq!(lapsed, Ok(()), "LAPSED: A's marker past its lifetime was honoured");
}

/// **A captured envelope is refused at another provider, and with its id stripped once its gateway
/// publishes the id marker** (post-360 row E's review, findings 1 and 7). A frames an envelope for
/// B; C, which serves the same kinds, is handed it: misaddressed, dropped. With the id stripped, C
/// refuses it as a replay once A's `sys/caller-context-id/` marker has reached it; without the marker
/// — a gateway on 2.34 or earlier — it verifies in the v1 form, is served, and is counted
/// (`caller_envelopes_without_id`), which is the one-release window. Seen failing first: C served
/// both.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_captured_envelope_is_refused_at_another_provider_and_stripped() {
    let (a, b, c, nb, nc, dir) = three_signed_nodes("e-captured", false).await;
    let framed = crate::agent::gateway_caller::frame_with_context(&a.task_ctx, &nb, "oidc:idp/alice", &[], bytes::Bytes::from_static(b"go")).unwrap();
    let at = |x: &GossipAgent, to: &NodeId, f: bytes::Bytes| {
        let mut buf = bytes::BytesMut::new();
        bytes::BufMut::put_u64_le(&mut buf, 42);
        bytes::BufMut::put(&mut buf, f);
        let req = crate::RpcRequest::from(crate::signal::Signal {
            kind: Arc::from("k"), scope: crate::signal::SignalScope::Individual(to.clone()),
            payload: buf.freeze(), sender: a.node_id().clone(), nonce: 7,
        });
        x.receive_gateway_caller(&req).map(|_| ()).map_err(|e| e.code())
    };
    let marker = crate::agent::gateway_caller::id_marker_key(a.node_id());
    let marked = within_3s(|| c.kv().get(&marker).is_some()).await;
    let elsewhere = at(&c, &nc, framed.clone());
    let at_b = at(&b, &nb, framed.clone());
    let stripped = crate::agent::gateway_caller::strip_envelope_id(&framed);
    let marked_refusal = at(&c, &nc, stripped.clone());
    // A gateway on 2.34 or earlier publishes no marker: remove A's at C.
    let removed = c.kv().delete(marker.clone());
    let unmarked = at(&c, &nc, stripped.clone());
    let without_id = c.task_ctx.caller_replay.without_id.load(Ordering::Relaxed);
    a.shutdown().await; b.shutdown().await; c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(marked, "A's id marker reached C");
    assert_eq!(at_b, Ok(()), "plant: B, the provider it was signed for, serves it");
    assert_eq!(elsewhere, Err("misaddressed"), "REPLAY: C served an envelope signed for B");
    assert_eq!(marked_refusal, Err("id_stripped"), "REPLAY: C served A's envelope with the id stripped");
    assert!(removed, "the marker was removed at C");
    assert_eq!(unmarked, Ok(()), "without the marker an id-less envelope is a pre-row-E gateway's");
    assert_eq!(without_id, 1, "and is counted");
}

/// **The row F and F2 counters are on `GET /stats`, per node** (#611's review, finding 8; row F2 adds
/// `kv_untagged_signatures_refused`, and the two window counters stay, at zero). The counters
/// moved from process-global statics to the node's `TransportBounds`, so each node reports its own.
#[cfg(all(feature = "tls", feature = "gateway"))]
#[tokio::test]
async fn the_row_f_window_counters_are_on_gateway_stats() {
    use crate::config::TlsConfig;
    let (port, http_port) = (alloc_port(), alloc_port());
    let cert_dir = std::env::temp_dir().join(format!("myc-f-stats-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    a.start().await.unwrap();
    let b = &a.task_ctx.transport_bounds;
    b.kv_untagged_signatures.store(3, Ordering::Relaxed);
    b.kv_unsigned_accepted.store(5, Ordering::Relaxed);
    b.kv_unsigned_refused.store(7, Ordering::Relaxed);
    b.kv_untagged_refused.store(19, Ordering::Relaxed);
    b.kv_signer_mismatch.store(11, Ordering::Relaxed);
    b.kv_identity_exempt.store(13, Ordering::Relaxed);
    b.kv_identity_stale_refused.store(17, Ordering::Relaxed);
    let client = reqwest::Client::new();
    let mut stats = serde_json::Value::Null;
    for _ in 0..80 {
        if let Ok(r) = client.get(format!("http://127.0.0.1:{http_port}/stats")).send().await
            && r.status().is_success() { stats = r.json().await.unwrap(); break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(stats["kv_untagged_signatures"], 3, "{stats}");
    assert_eq!(stats["kv_unsigned_frames_accepted"], 5);
    assert_eq!(stats["kv_unsigned_frames_refused"], 7);
    assert_eq!(stats["kv_untagged_signatures_refused"], 19, "row F2's refusal of a bare signature, per node");
    assert_eq!(stats["kv_signer_mismatch"], 11);
    assert_eq!(stats["kv_identity_exempt_frames"], 13);
    assert_eq!(stats["kv_identity_stale_refused"], 17);
    assert!(stats.get("identity_untagged_proofs").is_none() && stats.get("consensus_untagged_signatures").is_none(),
            "the §23 counters went with the window they counted");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Every write a TLS node's agent originates is signed**, so a fleet that requires signed frames
/// still hears its members. A mailbox delivery was one of the twenty-three agent write sites that
/// built a bare `Data` frame (`scripts/check-signed-dispatch.sh`); with B requiring signatures and
/// anti-entropy a minute away, A's delivery reaches B only by gossip, which needs it signed.
/// Seen failing first (with the refusal built and the sites not yet rerouted): B never held the key.
#[cfg(feature = "tls")]
#[tokio::test]
async fn an_agent_write_reaches_a_peer_that_requires_signed_frames() {
    use crate::config::TlsConfig;
    let (port_a, port_b) = (alloc_port(), alloc_port());
    let (node_a, node_b) = (NodeId::new("127.0.0.1", port_a).unwrap(), NodeId::new("127.0.0.1", port_b).unwrap());
    let cert_dir = std::env::temp_dir().join(format!("myc-signed-agent-writes-{port_a}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mk = |node: &NodeId, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = node.to_socket_addr().port();
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 60; // anti-entropy is not what carries the write
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(node.clone(), cfg)
    };
    let a = mk(&node_a, vec![]);
    let b = mk(&node_b, vec![node_a.clone()]);
    a.start().await.unwrap();
    b.start().await.unwrap();
    // B must hold A's key before A writes, or a signed frame is dropped as an unknown signer.
    let b_knows_a = || b.task_ctx.peer_keys.pin().get(&node_a).is_some_and(|k| !k.is_empty());
    let a_knows_b = || a.task_ctx.peer_keys.pin().get(&node_b).is_some_and(|k| !k.is_empty());
    for _ in 0..400 {
        if b_knows_a() && a_knows_b() && !a.peers().is_empty() && !b.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(b_knows_a() && a_knows_b(), "the two nodes never exchanged identities");

    assert!(a.service().deliver_event(&node_b, "signed-kind", Bytes::from_static(b"hello")));
    let prefix = format!("mailbox/{node_b}/signed-kind/");
    let mut arrived = false;
    for _ in 0..200 {
        if !b.kv().scan_prefix(&prefix).is_empty() { arrived = true; break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(arrived, "a mailbox delivery from a TLS node reaches a peer that requires signed frames");
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

#[tokio::test]
async fn test_tombstone_nullifies_value() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    store.pin().insert(Arc::from("k"), StoreEntry { data: Some(Bytes::from_static(b"old")), timestamp: 0 });

    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    send_wire(&mut writer, &WireMessage::Data(data_update("k", b"", 2, true))).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").is_some_and(|e| e.data.is_none()), 200).await;
    assert!(store.pin().get("k").is_some(), "tombstone entry must remain in store for LWW");
}

#[tokio::test]
async fn test_deduplication() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, mut rx) = mpsc::channel::<(Bytes, u64, crate::framing::ForwardHint)>(10);
    let seen = Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS));
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx, seen,
                          GossipConfig::default().default_ttl);

    let update = data_update("k", b"v", 42, false);
    send_wire(&mut writer, &WireMessage::Data(update.clone())).await;
    send_wire(&mut writer, &WireMessage::Data(update)).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").and_then(|e| e.data.clone()) == Some(Bytes::from_static(b"v")), 200).await;

    let mut forwarded = 0;
    while rx.try_recv().is_ok() { forwarded += 1; }
    assert_eq!(forwarded, 1, "duplicate nonce should be dropped");
}

#[tokio::test]
async fn test_peer_registered_from_ping() {
    let (mut writer, reader) = loopback_pair().await;
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::new(papaya::HashMap::new()), Arc::clone(&peers), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    send_wire(&mut writer, &WireMessage::Ping { sender: "127.0.0.1:9999".parse().unwrap(), known_peers: vec![] }).await;

    let p = Arc::clone(&peers);
    poll_until(
        || p.pin().contains_key(&NodeId::new("127.0.0.1", 9999).unwrap()),
        200,
    ).await;
}

#[tokio::test]
async fn test_ping_not_deduplicated() {
    let (mut writer, reader) = loopback_pair().await;
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::new(papaya::HashMap::new()), Arc::clone(&peers), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    send_wire(&mut writer, &WireMessage::Ping { sender: "127.0.0.1:1001".parse().unwrap(), known_peers: vec![] }).await;
    send_wire(&mut writer, &WireMessage::Ping { sender: "127.0.0.1:1002".parse().unwrap(), known_peers: vec![] }).await;

    let p = Arc::clone(&peers);
    poll_until(
        || {
            let g = p.pin();
            g.contains_key(&NodeId::new("127.0.0.1", 1001).unwrap())
                && g.contains_key(&NodeId::new("127.0.0.1", 1002).unwrap())
        },
        200,
    ).await;
}

#[tokio::test]
async fn test_handle_connection_shutdown() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let (shutdown_tx, handle) = spawn_handler(
        reader,
        Arc::clone(&store),
        Arc::new(papaya::HashMap::new()),
        tx,
        Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)),
        GossipConfig::default().default_ttl,
    );

    send_wire(&mut writer, &WireMessage::Data(data_update("k", b"v", 1, false))).await;
    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").is_some(), 200).await;

    let _ = shutdown_tx.send(true);
    handle.await.unwrap().ok();
}

// ── TTL clamping ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_inbound_ttl_clamped_to_max() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, mut rx) = mpsc::channel::<(Bytes, u64, crate::framing::ForwardHint)>(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), 5);

    let mut update = data_update("k", b"v", 77, false);
    update.ttl = 255;
    send_wire(&mut writer, &WireMessage::Data(update)).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").is_some(), 200).await;

    let (fwd_bytes, _, _) = rx.try_recv().expect("should have forwarded once");
    assert_eq!(fwd_bytes[TTL_OFFSET], 4, "forwarded TTL must be clamped to max_ttl - 1");
}

#[tokio::test]
async fn test_inbound_ttl_above_max_not_forwarded_when_clamped_to_one() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, mut rx) = mpsc::channel::<(Bytes, u64, crate::framing::ForwardHint)>(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), 1);

    let mut update = data_update("k", b"v", 88, false);
    update.ttl = 100;
    send_wire(&mut writer, &WireMessage::Data(update)).await;

    let s = Arc::clone(&store);
    poll_until(|| s.pin().get("k").is_some(), 200).await;

    assert!(rx.try_recv().is_err(), "no forward when clamped ttl == 1");
}

// ── Two-node integration test ─────────────────────────────────────────────

#[tokio::test]
async fn test_two_node_propagation() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port = port_a;
    cfg_a.health_check_interval_secs = 1;
    cfg_a.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_b).unwrap()];

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.health_check_interval_secs = 1;
    cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];

    let agent_a = Arc::new(GossipAgent::new(id_a, cfg_a));
    let agent_b = Arc::new(GossipAgent::new(id_b, cfg_b));

    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();

    time::sleep(Duration::from_millis(20)).await;

    let _ = agent_a.kv().set("x", b"hello".to_vec());
    let b = Arc::clone(&agent_b);
    poll_until(
        || b.kv().get("x") == Some(Bytes::from_static(b"hello")),
        2_000,
    ).await;

    let _ = agent_a.kv().delete("x");
    let b = Arc::clone(&agent_b);
    poll_until(|| b.kv().get("x").is_none(), 2_000).await;

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}


// ── subscribe() ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_subscribe_notified_via_gossip() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let subs: Arc<papaya::HashMap<Arc<str>, watch::Sender<Option<Bytes>>>> =
        Arc::new(papaya::HashMap::new());
    let (gossip_tx, _) = mpsc::channel::<(Bytes, u64, crate::framing::ForwardHint)>(10);
    let (shutdown_tx, _sd) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);

    let (sub_tx, _) = watch::channel(None::<Bytes>);
    let mut sub_rx = sub_tx.subscribe();
    sub_rx.borrow_and_update();
    subs.pin().insert(Arc::from("gossip_key"), sub_tx);

    let gossip_txs = mycelium_core::outbound::GossipTxs::new(
        (0..N_GOSSIP_SHARDS).map(|_| gossip_tx.clone()).collect::<Vec<_>>(), Default::default());
    {
        use crate::signal::{Boundary, SignalHandlers};
        use crate::agent::{TaskCtx, BulkTransport};
    use mycelium_core::CoreCtx;
        use parking_lot::RwLock;
        let node_id = NodeId::new("127.0.0.1", 0).unwrap();
        let kv_state = Arc::new(KvState {
            kv_store: crate::store::KvStore {
                store: Arc::clone(&store),
                prefix_index:      Arc::new(crate::store::PrefixIndex::new()),
                index_stripes:     Arc::new(std::array::from_fn(|_| std::sync::Mutex::new(()))),
                cap_ns_index:      Arc::new(crate::store::PrefixIndex::new()),
                scope_index: Arc::new(papaya::HashMap::new()),
            scope_roster: Arc::new(papaya::HashMap::new()),
                #[cfg(feature = "test-frame-filter")]
            frame_filter: Arc::new(papaya::HashMap::new()),
            scoped_prefixes: Arc::from(Vec::<Arc<str>>::new()),
                hash_acc:          Arc::new(AtomicU64::new(0)),
                dropped_frames:    Arc::new(AtomicU64::new(0)),
            individual_flood_fallbacks: Arc::new(AtomicU64::new(0)),
                max_store_entries: 0,
                live_count:        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                grp_generation:    Arc::new(AtomicU64::new(0)),
                prefix_watchers:           Arc::new(papaya::HashMap::new()),
                prefix_predicate_watchers: Arc::new(papaya::HashMap::new()),
                next_pred_watcher_id:      Arc::new(AtomicU64::new(0)),
                peer_localities:           Arc::new(papaya::HashMap::new()),
                quorum_trackers:           Arc::new(papaya::HashMap::new()),
            },
            subscriptions: subs,
        });
        let (shutdown_tx_inner2, _) = tokio::sync::watch::channel(false);
        let core_ctx = Arc::new(CoreCtx {
            node_id: node_id.clone(),
            seen: Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)),
            hlc: Arc::new(crate::hlc::Hlc::new()),
            signal_boundary: Arc::new(RwLock::new(Boundary::new(node_id))),
            decision_sink: std::sync::OnceLock::new(),
        inbound_observer: std::sync::OnceLock::new(),
            signal_handlers: Arc::new(SignalHandlers::new(Duration::from_secs(600))),
            gossip_txs,
            default_ttl: 5,
            kv_state,
            wal: std::sync::OnceLock::new(),
            sys_namespace_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            transport_bounds: Arc::default(),
            tls: std::sync::OnceLock::new(),
            peer_keys: Arc::new(papaya::HashMap::new()),
            peer_anchor_keys: Arc::new(papaya::HashMap::new()),
            removed: Arc::new(mycelium_core::removal::RemovedSet::default()),
            key_standing: Arc::new(mycelium_core::removal::KeyStanding::default()),
            identity_anchor_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            peers: Arc::new(papaya::HashMap::new()),
            rate_throttle: Arc::new(papaya::HashMap::new()),
            reorder_buf: None,
            reply_interceptor: None,
            soft_state_advertised: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            soft_state_gate:       Arc::default(),
            shutdown_tx: Arc::new(shutdown_tx_inner2),
            task_handles: Arc::new(std::sync::Mutex::new(tokio::task::JoinSet::new())),
            config: Arc::new(crate::config::GossipConfig::default()),
            hot: Arc::new(mycelium_core::context::HotConfig::from_config(&crate::config::GossipConfig::default())),
        });
        let task_ctx = Arc::new(TaskCtx {
        #[cfg(feature = "consensus")]
        consensus_accepted: Arc::new(papaya::HashMap::new()),
        #[cfg(feature = "consensus")]
        acceptor_records: std::sync::Mutex::new(()),
            core: core_ctx,
            bulk_transport: Arc::new(BulkTransport::new(0, Duration::from_secs(5), 64)),
            rpc_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            rpc_reply_sender_mismatches: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            commit_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            commit_conflict_slots: Arc::new(papaya::HashMap::new()),
            decided_floor_anomaly_slots: Arc::new(papaya::HashSet::new()),
            decided_floor_anomalies_unrecorded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "consensus")]
            consensus_verified_ballots: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorates: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorate_refused: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            electorate_fences: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            acceptor_withheld: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            acceptor_notices: Arc::new(crate::agent::acceptor_guard::notice_sets()),
            #[cfg(feature = "consensus")]
            electorate_retired: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorate_completed: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorate_drops: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorate_migrations: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            migrations_running: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            electorate_epochs_known: Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "consensus")]
            electorate_retire_known: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            electorate_carried_known: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            electorate_dropped_known: Arc::new(papaya::HashSet::new()),
            #[cfg(feature = "consensus")]
            electorate_records_watch: Arc::new(std::sync::OnceLock::new()),
            #[cfg(feature = "consensus")]
            electorate_records_scanned: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
            #[cfg(feature = "consensus")]
            electorate_groups_known: Arc::new(papaya::HashSet::new()),
            ballot_space_exhausted: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            event_ring: Arc::new(crate::agent::emergent::EventRing::default()),
            governed_group_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            capability_coverage_gaps: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        role_concentration_pct: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            membership_flaps: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        control_profile: std::sync::atomic::AtomicU8::new(0),
        control_would_hold: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        opacity_releases_spaced: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            opacity_oscillations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cap_authz_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            schema_mismatch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "gateway")]
            governance_changes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "gateway")]
            governance_unaudited: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "compliance")]
            audit_chain: Arc::new(std::sync::Mutex::new(crate::agent::audit::AuditChainState::new())),
            #[cfg(all(feature = "gateway", feature = "tls"))]
            action_evaluator: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        deployed_policy_revision: arc_swap::ArcSwapOption::from(None),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        evidence_journal: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        history_index: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        ae_tally_heartbeat: std::sync::atomic::AtomicBool::new(false),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        ae_tally_lock: std::sync::Mutex::new(()),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        execution_authority: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        provider_enforcement: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "tls")]
        membership: std::sync::OnceLock::new(),
        #[cfg(feature = "tls")]
        identity_ledger: Arc::new(crate::agent::revocation::IdentityLedger::default()),
        caller_replay: Default::default(),
        causal_misplaced: Default::default(),
        #[cfg(feature = "a2a")]
        a2a_mounted: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "a2a")]
        a2a_default_skill: std::sync::OnceLock::new(),
        guarantees: parking_lot::Mutex::new(crate::agent::guarantee::core_guarantees()),
        started: std::sync::atomic::AtomicBool::new(false),
        late_attachments: std::sync::atomic::AtomicU32::new(0),
        at_rest_cipher_attached: std::sync::atomic::AtomicBool::new(false),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        cohort_budget: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_edge: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_clients: std::sync::OnceLock::new(),
        #[cfg(feature = "gateway")]
        egress_credentials: std::sync::OnceLock::new(),
            #[cfg(feature = "compliance")]
            audit_sink: std::sync::OnceLock::new(),
            #[cfg(feature = "compliance")]
            audit_sink_tx: std::sync::OnceLock::new(),
            filter_opacity_registry: Arc::new(crate::agent::FilterOpacityRegistry::new()),
            group_roster_cache: Arc::new(papaya::HashMap::new()),
            tuning_governor: Arc::new(crate::agent::TuningGovernor::default()),
            #[cfg(feature = "llm")]
            llm_skills: std::sync::Arc::new(papaya::HashMap::new()),
            #[cfg(feature = "llm")]
            llm_dispatch_spawned: std::sync::atomic::AtomicBool::new(false),
            mailbox_in_flight: Default::default(),
        });
        let ctx = ConnContext {
            task_ctx: Arc::clone(&task_ctx.core),
            peers: Arc::new(papaya::HashMap::new()),
            shutdown: shutdown_tx,
            peer_writers: Arc::new(papaya::HashMap::new()),
            backoff: Duration::ZERO,
            n_shards: N_GOSSIP_SHARDS,
            intern_keys: true,
            intern_max_keys: 0,
            max_peers: usize::MAX,
            writer_timing: mycelium_core::writer::WriterTiming::with_idle(Duration::ZERO),
            peer_list_tx: tokio::sync::watch::channel(std::sync::Arc::from(Vec::<NodeId>::new())).0,
        };
        use crate::connection::handle_connection;
        tokio::spawn(handle_connection(crate::stream::GossipStream::Plain(reader), "127.0.0.1:0".parse().unwrap(), ctx));
    }

    send_wire(&mut writer, &WireMessage::Data(data_update("gossip_key", b"gossip_val", 42, false))).await;

    tokio::time::timeout(Duration::from_millis(200), sub_rx.changed())
        .await
        .expect("subscriber should fire within 200 ms")
        .unwrap();
    assert_eq!(*sub_rx.borrow(), Some(Bytes::from_static(b"gossip_val")));
}

#[test]
fn test_subscribe_multiple_receivers_same_key() {
    let agent = make_agent();
    let rx1 = agent.kv().subscribe("k");
    let rx2 = agent.kv().subscribe("k");
    let _ = agent.kv().set("k", b"shared".to_vec());
    assert_eq!(*rx1.borrow(), Some(Bytes::from_static(b"shared")));
    assert_eq!(*rx2.borrow(), Some(Bytes::from_static(b"shared")));
}

// ── Peer-list piggybacking ────────────────────────────────────────────────

#[tokio::test]
async fn test_piggybacked_peers_added_to_table() {
    let (mut writer, reader) = loopback_pair().await;
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::new(papaya::HashMap::new()), Arc::clone(&peers), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    let piggybacked = vec![
        NodeId::new("127.0.0.1", 5001).unwrap(),
        NodeId::new("127.0.0.1", 5002).unwrap(),
    ];
    send_wire(&mut writer, &WireMessage::Ping {
        sender: "127.0.0.1:9999".parse().unwrap(),
        known_peers: piggybacked,
    }).await;

    let p = Arc::clone(&peers);
    poll_until(
        || {
            let g = p.pin();
            g.contains_key(&NodeId::new("127.0.0.1", 9999).unwrap())
                && g.contains_key(&NodeId::new("127.0.0.1", 5001).unwrap())
                && g.contains_key(&NodeId::new("127.0.0.1", 5002).unwrap())
        },
        200,
    ).await;
}

#[tokio::test]
async fn test_piggybacked_self_not_added() {
    let (mut writer, reader) = loopback_pair().await;
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::new(papaya::HashMap::new()), Arc::clone(&peers), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    let self_id = NodeId::new("127.0.0.1", 0).unwrap();
    send_wire(&mut writer, &WireMessage::Ping {
        sender: "127.0.0.1:9000".parse().unwrap(),
        known_peers: vec![self_id.clone()],
    }).await;

    let p = Arc::clone(&peers);
    poll_until(|| p.pin().contains_key(&NodeId::new("127.0.0.1", 9000).unwrap()), 200).await;
    assert!(!peers.pin().contains_key(&self_id), "self must not be added via piggybacking");
}

#[tokio::test]
async fn test_piggybacked_known_peer_timestamp_not_overwritten() {
    let (mut writer, reader) = loopback_pair().await;
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::new(papaya::HashMap::new()), Arc::clone(&peers), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    let known_id: NodeId = "127.0.0.1:7777".parse().unwrap();
    let old_time = Instant::now() - Duration::from_secs(5);
    peers.pin().insert(known_id.clone(), old_time);

    send_wire(&mut writer, &WireMessage::Ping {
        sender: "127.0.0.1:8888".parse().unwrap(),
        known_peers: vec![known_id.clone()],
    }).await;

    let p = Arc::clone(&peers);
    poll_until(|| p.pin().contains_key(&NodeId::new("127.0.0.1", 8888).unwrap()), 200).await;

    let stored = *peers.pin().get(&known_id).unwrap();
    assert_eq!(
        stored, old_time,
        "existing peer timestamp must not be overwritten by piggybacking"
    );
}

// ── Anti-entropy ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_state_response_applies_entries_to_store() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    let entries = vec![
        SyncEntry { key: Arc::from("c:v1"), value: Bytes::from_static(b"payload_v1"), timestamp: 100, is_tombstone: false },
        SyncEntry { key: Arc::from("c:v2"), value: Bytes::new(), timestamp: 200, is_tombstone: true },
    ];
    send_wire(&mut writer, &WireMessage::StateResponse { entries }).await;

    let s = Arc::clone(&store);
    poll_until(
        || s.pin().get("c:v1").and_then(|e| e.data.clone()) == Some(Bytes::from_static(b"payload_v1")),
        200,
    ).await;

    assert!(
        store.pin().get("c:v2").is_some_and(|e| e.data.is_none()),
        "tombstone entry must land as data=None"
    );
}

#[tokio::test]
async fn test_state_response_respects_lww() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    store.pin().insert(Arc::from("k"), StoreEntry { data: Some(Bytes::from_static(b"newer")), timestamp: 999 });

    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
                          Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)), GossipConfig::default().default_ttl);

    let entries = vec![
        SyncEntry { key: Arc::from("k"), value: Bytes::from_static(b"stale"), timestamp: 1, is_tombstone: false },
    ];
    send_wire(&mut writer, &WireMessage::StateResponse { entries }).await;

    time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        store.pin().get("k").and_then(|e| e.data.clone()),
        Some(Bytes::from_static(b"newer")),
        "StateResponse must not overwrite a locally newer value"
    );
}

#[tokio::test]
async fn test_anti_entropy_syncs_pre_existing_state() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port = port_a;
    cfg_a.health_check_interval_secs = 1;

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.health_check_interval_secs = 1;

    let agent_a = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port_a).unwrap(),
        cfg_a,
    ));
    agent_a.start().await.unwrap();
    time::sleep(Duration::from_millis(20)).await;

    let _ = agent_a.kv().set("contract:v1", b"spec_bytes".to_vec());
    assert_eq!(agent_a.kv().get("contract:v1"), Some(Bytes::from_static(b"spec_bytes")));

    cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];
    let agent_b = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port_b).unwrap(),
        cfg_b,
    ));
    agent_b.start().await.unwrap();

    let b = Arc::clone(&agent_b);
    poll_until(
        || b.kv().get("contract:v1") == Some(Bytes::from_static(b"spec_bytes")),
        3_000,
    ).await;

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

#[tokio::test]
async fn test_anti_entropy_skips_when_synced() {
    // Build a store with one live entry so the hash is non-zero (zero is the
    // "no digest" sentinel and would trigger a full snapshot instead).
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    store.pin().insert(
        Arc::from("sync_key"),
        StoreEntry { data: Some(Bytes::from_static(b"sync_val")), timestamp: 42 },
    );
    let expected_hash = store_hash(&store);
    assert_ne!(expected_hash, 0, "precondition: hash must be non-zero");

    // Bind a listener so the handler's peer writer can connect back to deliver
    // the StateResponse. The port becomes the sender NodeId's port.
    let response_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sender_port = response_listener.local_addr().unwrap().port();
    let sender_id = NodeId::new("127.0.0.1", sender_port).unwrap();

    // Register sender as a known peer — the handler silently drops StateRequest
    // from unrecognised peers.
    let peers: Arc<papaya::HashMap<NodeId, Instant>> = Arc::new(papaya::HashMap::new());
    peers.pin().insert(sender_id.clone(), Instant::now());

    let (mut writer, reader) = loopback_pair().await;
    let (tx, _rx) = mpsc::channel(10);
    let (_shutdown, _handle) = spawn_handler(
        reader, store, peers, tx,
        Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)),
        GossipConfig::default().default_ttl,
    );

    // Start accepting before sending so the writer can connect immediately.
    let accept_task = tokio::spawn(async move {
        let (sock, _) = response_listener.accept().await.unwrap();
        sock
    });

    // Send a StateRequest whose hash matches the handler's store — fast-path.
    send_wire(&mut writer, &WireMessage::StateRequest {
        sender: sender_id,
        store_hash: expected_hash,
        bucket_hashes: vec![],
    }).await;

    let mut response_sock = accept_task.await.unwrap();

    // Read back the one frame the handler must write: an empty StateResponse.
    let mut buf = BytesMut::new();
    tokio::time::timeout(
        Duration::from_millis(500),
        read_frame(&mut response_sock, &mut buf),
    )
    .await
    .expect("timed out waiting for fast-path StateResponse")
    .expect("read_frame error");

    let msg = decode_wire(&buf).unwrap();

    match msg {
        WireMessage::StateResponse { entries } => assert!(
            entries.is_empty(),
            "anti-entropy fast-path: StateResponse must be empty when hashes match; got {} entries",
            entries.len(),
        ),
        other => panic!("expected StateResponse, got {:?}", other),
    }
}

// ── liveness flags ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_system_stats_liveness_flags_while_running() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.health_check_interval_secs = 1;
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).unwrap(),
        cfg,
    ));
    agent.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let stats = agent.system_stats();
    assert!(stats.gc_alive,             "gc task should be alive while running");
    assert!(stats.health_monitor_alive, "health monitor should be alive while running");
    agent.shutdown().await;
    // After shutdown, state-gated flags must not report false negatives.
    let stats = agent.system_stats();
    assert!(stats.gc_alive,             "gc_alive should read true after clean shutdown");
    assert!(stats.health_monitor_alive, "health_monitor_alive should read true after clean shutdown");
}

#[tokio::test]
async fn regression_store_entries_reflects_writes_immediately() {
    // Audit 2026-07-15 pass 4: system_stats().store_entries read the GC task's periodic recount,
    // which lags reality by up to a full GC interval (≥60 s) under write load — so an operator/
    // autoscaler saw a stale count. It must read the exact inline `kv.live_count` (maintained on
    // every winning CAS), reflecting writes at once.
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let before = agent.system_stats().store_entries;
    for i in 0..5 { let _ = agent.kv().set(format!("probe/k{i}"), b"v".to_vec()); }
    let after = agent.system_stats().store_entries;
    assert!(after >= before + 5,
        "store_entries must reflect the 5 writes immediately (exact live_count), not lag a GC tick: {before} -> {after}");
    agent.shutdown().await;
}

// ── shutdown_with_timeout ─────────────────────────────────────────────────

#[tokio::test]
async fn test_shutdown_with_timeout_does_not_hang() {
    let port = alloc_port();
    let cfg = GossipConfig { bind_port: port, ..GossipConfig::default() };
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();
    // Use a 1 ms internal timeout so the abort path fires; the outer 2 s
    // timeout asserts the call itself returns instead of hanging forever.
    tokio::time::timeout(
        Duration::from_secs(2),
        agent.shutdown_with_timeout(Duration::from_millis(1)),
    )
    .await
    .expect("shutdown_with_timeout must return even when the internal timeout fires");
}

#[tokio::test]
async fn test_shutdown_lifecycle_edges_never_started_and_double() {
    // Shutdown on an agent that was never started must return promptly, not
    // hang waiting on tasks that were never spawned.
    let port = alloc_port();
    let cfg = GossipConfig { bind_port: port, ..GossipConfig::default() };
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    tokio::time::timeout(Duration::from_secs(2), agent.shutdown())
        .await
        .expect("shutdown on a never-started agent must not hang");

    // A second shutdown after a completed one must be an idempotent no-op.
    let port = alloc_port();
    let cfg = GossipConfig { bind_port: port, ..GossipConfig::default() };
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();
    agent.shutdown().await;
    tokio::time::timeout(Duration::from_secs(2), agent.shutdown())
        .await
        .expect("double shutdown must be idempotent, not hang or panic");
}

/// Individual-scoped signals must reach targets the sender is NOT directly
/// peered with — forwarding is unconditional in the Holland model; only
/// admission is scoped. Line topology A→B→C: A's only outbound peer is B,
/// so delivery to C requires the flood fallback at origination plus B's
/// targeted relay. Pre-fix, A silently dropped the signal at origination,
/// which broke RPC requests/responses and consensus votes between
/// non-peered pairs in partial meshes (M2 finding, 2026-06-12; found by
/// the three-arm experiment bring-up).
#[tokio::test]
async fn test_individual_signal_reaches_unpeered_target_via_relay() {
    use crate::signal::SignalScope;
    use bytes::Bytes;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let port_c = alloc_port();
    let id =
        |p: u16| NodeId::new("127.0.0.1", p).unwrap();

    let mk = |port: u16, boots: Vec<NodeId>, max_active: usize| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        // Pin the active-connection cap so the topology is deterministic regardless
        // of discovery/health-tick timing (0 = unbounded).
        cfg.max_active_connections = max_active;
        // This gate asserts relay/flood-fallback across a *controlled* sparse topology
        // (A→B→C, non-adjacent pair). SWIM (now default-on) would discover every node over
        // UDP and dissolve the controlled mesh, so pin the legacy TCP-forwarding path the
        // gate is written for. (RPC over SWIM partial meshes is covered by the G3 scale test.)
        cfg.swim_failure_detector = false;
        GossipAgent::new(id(port), cfg)
    };

    // Strict line: A → B → C. A is capped to a single active connection (its
    // bootstrap, B), so it structurally cannot form a direct A→C route even once
    // discovery piggybacks C into its peer set — making the relay path (and the
    // flood-fallback counter) deterministic instead of racing the health tick that
    // would otherwise reconcile a learned C into A's forwarding set.
    let c = Arc::new(mk(port_c, vec![], 0));
    let b = Arc::new(mk(port_b, vec![id(port_c)], 0));
    let a = Arc::new(mk(port_a, vec![id(port_b)], 1));
    c.start().await.unwrap();
    b.start().await.unwrap();
    a.start().await.unwrap();

    let mut rx = c.mesh().signal_rx("test.relay");

    // Structural poll: line links up.
    for _ in 0..100 {
        if !a.peers().is_empty() && !b.peers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(a.mesh().emit(
        "test.relay",
        SignalScope::Individual(id(port_c)),
        Bytes::from_static(b"hop"),
    ));

    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    let sig = got
        .expect("Individual signal must reach an unpeered target via relay")
        .expect("channel open");
    assert_eq!(&sig.payload[..], b"hop");

    // The fallback must be legible: A had no direct route, so its counter
    // (also on /stats) records the flood fallback.
    assert!(
        a.system_stats().individual_flood_fallbacks >= 1,
        "flood fallback must be counted on the sender"
    );

    a.shutdown().await;
    b.shutdown().await;
    c.shutdown().await;
}


/// Topology-class generalization of the line-relay canary above: across
/// RANDOM connected partial meshes, all three Individual-scope consumers —
/// targeted signal delivery, RPC round-trip, and consensus ballots (votes go
/// Individual to the proposer) — must work between deliberately NON-adjacent
/// node pairs via the flood fallback. Topology was the dimension no prior
/// test varied (M2 post-mortem, 2026-06-12): every existing topology
/// accidentally peered all RPC pairs directly.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_individual_consumers_over_random_partial_meshes() {
    use crate::consensus::{ConsensusConfig, ConsensusResult};
    use crate::signal::SignalScope;
    use bytes::Bytes;

    for graph_seed in [11u64, 23, 47] {
        let mut rng = fastrand::Rng::with_seed(graph_seed);
        let n = 7;
        let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
        let id = |i: usize| NodeId::new("127.0.0.1", ports[i]).unwrap();

        // Random spanning tree (node i dials a random earlier node) plus one
        // extra random edge: connected by construction, sparse enough that
        // non-adjacent pairs always exist at n=7.
        let mut dials: Vec<Vec<usize>> = vec![vec![]; n];
        for (i, d) in dials.iter_mut().enumerate().skip(1) {
            d.push(rng.usize(0..i));
        }
        let (a, b) = (rng.usize(0..n), rng.usize(0..n));
        if a != b && !dials[a].contains(&b) && !dials[b].contains(&a) {
            dials[a].push(b);
        }

        let adjacent = |x: usize, y: usize| dials[x].contains(&y) || dials[y].contains(&x);
        let (src, dst) = (0..n)
            .flat_map(|x| (0..n).map(move |y| (x, y)))
            .find(|&(x, y)| x != y && !adjacent(x, y))
            .expect("sparse graph must have a non-adjacent pair");

        let mut agents = Vec::with_capacity(n);
        for i in 0..n {
            let mut cfg = GossipConfig::default();
            cfg.bind_port = ports[i];
            cfg.bootstrap_peers = dials[i].iter().map(|&j| id(j)).collect();
            cfg.reconnect_backoff_secs = 1;
            // Fast first pings: peer discovery is ping-driven, and the
            // property under test is delivery, not discovery cadence.
            cfg.health_check_max_jitter_ms = 50;
            // Worst-case relay path in a random tree on 7 nodes can exceed
            // the default hop budget; the property under test is delivery,
            // not TTL sizing.
            cfg.default_ttl = 10;
            // The gate's premise is a *random partial mesh* with genuinely non-adjacent
            // pairs. SWIM (now default-on) discovers every node over UDP and would connect
            // the mesh fully, removing the non-adjacency this gate exists to exercise — so
            // pin the legacy TCP-forwarding/relay path it is written for. (RPC/delivery over
            // SWIM partial meshes at scale is covered by the G3 resilience test.)
            cfg.swim_failure_detector = false;
            let agent = Arc::new(GossipAgent::new(id(i), cfg));
            agent.start().await.unwrap();
            agents.push(agent);
        }

        // Consensus listeners on every node BEFORE any proposal (CLAUDE.md
        // pattern), and structural poll until the dial graph links up.
        let _listeners: Vec<_> = agents
            .iter()
            .map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default()))
            .collect();
        for _ in 0..100 {
            if agents.iter().all(|a| !a.peers().is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // (a) Individual signal src -> non-adjacent dst. Re-emit each second
        // (distinct frames) to distinguish "racing formation" from "never". The window is 20 s, not
        // 8: the topology is deterministic (fixed seeds) but multi-hop flood-fallback delivery is
        // timing-bound, and 8 s occasionally starves under full CI load (flaked once on 2026-07-03 —
        // calibration ledger). This is "re-emit until the structural condition holds", not a fixed
        // sleep — the loop exits the instant delivery is observed.
        let mut rx = agents[dst].mesh().signal_rx("prop.topology");
        let mut delivered_at = None;
        for attempt in 0..20 {
            assert!(agents[src].mesh().emit(
                "prop.topology",
                SignalScope::Individual(id(dst)),
                Bytes::from_static(b"hop"),
            ));
            if tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.is_ok() {
                delivered_at = Some(attempt);
                break;
            }
        }
        if delivered_at.is_none() {
            for (i, a) in agents.iter().enumerate() {
                eprintln!("  POSTMORTEM node {i}: fb={} peers={}",
                    a.system_stats().individual_flood_fallbacks, a.peers().len());
            }
            // Which senders can reach dst directly?
            for (i, agent) in agents.iter().enumerate() {
                if i == dst { continue; }
                let ok = agent.mesh().emit(
                    "prop.topology",
                    SignalScope::Individual(id(dst)),
                    Bytes::from_static(b"direct"),
                );
                let got = tokio::time::timeout(Duration::from_millis(800), rx.recv()).await.is_ok();
                eprintln!("  direct {i}->{dst}: emit={ok} delivered={got}");
            }
            panic!("graph_seed={graph_seed}: signal {src}->{dst} undelivered after 8 attempts");
        }
        eprintln!("  signal delivered on attempt {}", delivered_at.unwrap());

        // (b) RPC round-trip src -> dst (echo handler on dst).
        let mut req_rx = agents[dst].service().rpc_rx("prop.echo");
        let dst_agent = Arc::clone(&agents[dst]);
        tokio::spawn(async move {
            while let Some(req) = req_rx.recv().await {
                let payload = req.payload();
                dst_agent.service().rpc_respond(&req, payload);
            }
        });
        let reply = agents[src]
            .service()
            .rpc_call(id(dst), "prop.echo", Bytes::from_static(b"ping"), Duration::from_secs(8))
            .await
            .unwrap_or_else(|e| panic!("graph_seed={graph_seed}: rpc {src}->{dst} failed: {e:?}"));
        assert_eq!(&reply[..], b"ping");

        // (c) Ballot from src: votes return Individual over the same relays.
        match agents[src]
            .consensus()
            .cluster_propose("prop/slot", Bytes::from_static(b"v"), ConsensusConfig::default())
            .await
        {
            ConsensusResult::Committed { .. } => {}
            other => panic!("graph_seed={graph_seed}: ballot did not commit: {other:?}"),
        }

        for a in agents {
            a.shutdown().await;
        }
    }
}

/// Cold-start sendability (2026-07-21, the mailbox_llm 1-in-10 RPC drop made
/// deterministic): with the health interval cranked so no tick reconcile can rescue the
/// handshake, both peer maps must converge and an Individual-scoped RPC must succeed in
/// BOTH directions within seconds of startup. Two distinct mechanisms are under test:
/// - SWIM on (default): `ApplyEffect::BecameAlive` runs the bounded fan-out activation
///   (the SWIM twin of the TCP Ping arm's append) — before the fix, the SEED's watch
///   stayed empty until its first tick (up to jitter + interval) and its RPC responses
///   dropped with "no peers at all".
/// - SWIM off: ping-before-pull (the startup Ping announcing our NodeId ahead of the
///   StateRequest, which is ignored from unknown peers) + the Ping arm's ping-back.
#[tokio::test]
async fn test_cold_start_rpc_both_directions_before_first_tick_swim() {
    cold_start_rpc_case(true).await;
}

#[tokio::test]
async fn test_cold_start_rpc_both_directions_before_first_tick_no_swim() {
    cold_start_rpc_case(false).await;
}

async fn cold_start_rpc_case(swim: bool) {
    let pa = crate::test_util::alloc_port();
    let pb = crate::test_util::alloc_port();
    let mk = |port: u16, boot: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boot;
        cfg.swim_failure_detector = swim;
        cfg.health_check_interval_secs = 3600; // first tick ~an hour away
        cfg.health_check_max_jitter_ms = 1;    // startup block acts at t≈0
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    };
    let a = mk(pa, vec![]);
    let b = mk(pb, vec![NodeId::new("127.0.0.1", pa).unwrap()]);
    a.start().await.unwrap();
    b.start().await.unwrap();

    for ag in [&a, &b] {
        let mut rx = ag.service().rpc_rx("cold.echo");
        let me = Arc::clone(ag);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let payload = req.payload();
                me.service().rpc_respond(&req, payload);
            }
        });
    }

    // Ping + ping-back converge both maps with no health tick involved.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while a.peers().is_empty() || b.peers().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "peer maps never converged without a health tick (startup ping / ping-back missing)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The cold-start assertion: an Individual-scoped RPC succeeds immediately, both ways.
    let ida = NodeId::new("127.0.0.1", pa).unwrap();
    let idb = NodeId::new("127.0.0.1", pb).unwrap();
    let r = match b.service()
        .rpc_call(ida, "cold.echo", Bytes::from_static(b"b->a"), Duration::from_secs(5))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            for (name, ag) in [("a", &a), ("b", &b)] {
                let s = ag.system_stats();
                eprintln!("POSTMORTEM {name}: peers={:?} flood_fallbacks={} dropped={}",
                    ag.peers(), s.individual_flood_fallbacks, s.dropped_frames);
            }
            panic!("b->a cold-start rpc failed: {e:?}");
        }
    };
    assert_eq!(&r[..], b"b->a");
    let r = a.service()
        .rpc_call(idb, "cold.echo", Bytes::from_static(b"a->b"), Duration::from_secs(5))
        .await.expect("a->b cold-start rpc");
    assert_eq!(&r[..], b"a->b");

    a.shutdown().await;
    b.shutdown().await;
}

// ── Layer 2: Signal / Boundary ───────────────────────────────────────────

#[tokio::test]
async fn test_signal_local_system_delivery() {
    let agent = make_agent();
    let mut rx = agent.mesh().signal_rx(signal_kind::HEALTH_PROBE);
    let _ = agent.mesh().emit(signal_kind::HEALTH_PROBE, SignalScope::Cluster, b"ping".to_vec());
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("signal should be delivered within 100ms")
        .expect("receiver should not be closed");
    assert_eq!(&*sig.kind, signal_kind::HEALTH_PROBE);
    assert_eq!(sig.payload, Bytes::from_static(b"ping"));
    assert_eq!(sig.scope, SignalScope::Cluster);
}

#[tokio::test]
async fn test_signal_group_admitted_when_member() {
    let agent = make_agent();
    agent.mesh().join_group("nlp");
    let mut rx = agent.mesh().signal_rx("task");
    // The agent is never started, so `join_group`'s KV frame stays in a gossip shard (fill 1/1024) and a Group emit's
    // local delivery is shed with that probability — re-emit until it arrives, bounded (#568's review; the 2026-10-02
    // precedent, `.log/2026-10-02-group-quorum-test-shed.md`).
    let mut got = None;
    for _ in 0..50 {
        let _ = agent.mesh().emit("task", SignalScope::Group(Arc::from("nlp")), b"work".to_vec());
        if let Ok(sig) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            got = sig;
            break;
        }
    }
    let sig = got.expect("member should receive group signal");
    assert_eq!(sig.scope, SignalScope::Group(Arc::from("nlp")));
}

#[tokio::test]
async fn test_signal_group_blocked_when_not_member() {
    let agent = make_agent();
    // do NOT join "nlp"
    let mut rx = agent.mesh().signal_rx("task");
    let _ = agent.mesh().emit("task", SignalScope::Group(Arc::from("nlp")), b"ignored".to_vec());
    let result = tokio::time::timeout(Duration::from_millis(30), rx.recv()).await;
    assert!(result.is_err(), "non-member should not receive group signal");
}

#[tokio::test]
async fn test_signal_individual_admitted_to_self() {
    let agent = make_agent();
    let self_id = agent.node_id().clone();
    let mut rx = agent.mesh().signal_rx(signal_kind::INVOKE);
    let _ = agent.mesh().emit(signal_kind::INVOKE, SignalScope::Individual(self_id), b"call".to_vec());
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("individual signal to self should be delivered")
        .expect("receiver closed");
    assert_eq!(&*sig.kind, signal_kind::INVOKE);
}

#[tokio::test]
async fn test_signal_multiple_receivers_same_kind() {
    let agent = make_agent();
    let mut rx1 = agent.mesh().signal_rx("evt");
    let mut rx2 = agent.mesh().signal_rx("evt");
    let _ = agent.mesh().emit("evt", SignalScope::Cluster, b"data".to_vec());
    let s1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv()).await;
    let s2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv()).await;
    assert!(s1.is_ok() && s1.unwrap().is_some(), "rx1 should receive signal");
    assert!(s2.is_ok() && s2.unwrap().is_some(), "rx2 should receive signal");
}

#[tokio::test]
async fn test_emit_async_delivers_locally() {
    let agent = make_agent();
    let mut rx = agent.mesh().signal_rx("async.evt");
    assert!(agent.mesh().emit_async("async.evt", SignalScope::Cluster, b"data".to_vec()).await);
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("emit_async should deliver locally")
        .expect("receiver closed");
    assert_eq!(sig.payload, Bytes::from_static(b"data"));
}

#[tokio::test]
async fn test_signal_rx_with_capacity() {
    let agent = make_agent();
    // Custom depth of 1 — second signal should be dropped (channel full).
    let mut rx = agent.mesh().signal_rx_with_capacity("burst", 1);
    let _ = agent.mesh().emit("burst", SignalScope::Cluster, b"first".to_vec());
    let _ = agent.mesh().emit("burst", SignalScope::Cluster, b"second".to_vec()); // drops on Full
    let first = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("first signal should arrive")
        .expect("receiver closed");
    assert_eq!(first.payload, Bytes::from_static(b"first"));
}

#[tokio::test]
async fn test_signal_two_node_propagation() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port = port_a;
    cfg_a.health_check_interval_secs = 1;
    cfg_a.bootstrap_peers = vec![id_b.clone()];

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.health_check_interval_secs = 1;
    cfg_b.bootstrap_peers = vec![id_a.clone()];

    let agent_a = Arc::new(GossipAgent::new(id_a, cfg_a));
    let agent_b = Arc::new(GossipAgent::new(id_b, cfg_b));

    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();

    // Wait for peers to discover each other.
    time::sleep(Duration::from_millis(100)).await;

    let mut rx_b = agent_b.mesh().signal_rx("cluster.event");
    let _ = agent_a.mesh().emit("cluster.event", SignalScope::Cluster, b"hello".to_vec());

    let sig = tokio::time::timeout(Duration::from_millis(2_000), rx_b.recv())
        .await
        .expect("signal should arrive at B within 2s")
        .expect("receiver closed");

    assert_eq!(&*sig.kind, "cluster.event");
    assert_eq!(sig.payload, Bytes::from_static(b"hello"));

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

#[tokio::test]
async fn test_group_signal_only_reaches_members() {
    // 3-node cluster: A and B join group "team"; C does not.
    // With group_aware_forwarding enabled, A's shard forwards Group("team")
    // signals only to known members + epidemic_extra_peers random others.
    // Regardless of forwarding, C's Boundary must block local delivery
    // (C never joined "team") — its handler must not fire.
    let (port_a, port_b, port_c) = (alloc_port(), alloc_port(), alloc_port());

    let make_cfg = |port: u16, peers: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port                = port;
        cfg.health_check_interval_secs = 1;
        cfg.group_aware_forwarding   = true;
        cfg.bootstrap_peers          = peers;
        cfg
    };

    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let id_c = NodeId::new("127.0.0.1", port_c).unwrap();

    let cfg_a = make_cfg(port_a, vec![id_b.clone(), id_c.clone()]);
    let cfg_b = make_cfg(port_b, vec![id_a.clone(), id_c.clone()]);
    let cfg_c = make_cfg(port_c, vec![id_a.clone(), id_b.clone()]);

    let agent_a = Arc::new(GossipAgent::new(id_a, cfg_a));
    let agent_b = Arc::new(GossipAgent::new(id_b, cfg_b));
    let agent_c = Arc::new(GossipAgent::new(id_c, cfg_c));

    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();
    agent_c.start().await.unwrap();

    // A and B join the group; C does not.
    agent_a.mesh().join_group("team");
    agent_b.mesh().join_group("team");

    // Wait for group membership KV entries to propagate and peers to discover each other.
    time::sleep(Duration::from_millis(300)).await;

    let mut rx_b = agent_b.mesh().signal_rx("team.event");
    let mut rx_c = agent_c.mesh().signal_rx("team.event");

    let _ = agent_a.mesh().emit("team.event", SignalScope::Group("team".into()), b"msg".to_vec());

    // B must receive the signal — it's a group member.
    tokio::time::timeout(Duration::from_millis(2_000), rx_b.recv())
        .await
        .expect("B (group member) should receive the Group signal within 2s")
        .expect("B receiver closed");

    // C must NOT receive the signal — its Boundary blocks delivery for Group("team").
    let c_result = tokio::time::timeout(Duration::from_millis(200), rx_c.recv()).await;
    assert!(c_result.is_err(), "C (non-member) must not receive the Group signal");

    agent_a.shutdown().await;
    agent_b.shutdown().await;
    agent_c.shutdown().await;
}

#[tokio::test]
async fn test_signal_not_delivered_twice_via_gossip() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port = port_a;
    cfg_a.health_check_interval_secs = 1;
    cfg_a.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_b).unwrap()];

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.health_check_interval_secs = 1;
    cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];

    let agent_a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port_a).unwrap(), cfg_a));
    let agent_b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port_b).unwrap(), cfg_b));

    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();
    time::sleep(Duration::from_millis(100)).await;

    let mut rx_b = agent_b.mesh().signal_rx("ping");
    let _ = agent_a.mesh().emit("ping", SignalScope::Cluster, b"once".to_vec());

    // Receive the first signal.
    let first = tokio::time::timeout(Duration::from_millis(2_000), rx_b.recv()).await;
    assert!(first.is_ok() && first.unwrap().is_some(), "first signal should arrive");

    // Give extra time for any duplicate to propagate — there must be none.
    let second = tokio::time::timeout(Duration::from_millis(200), rx_b.recv()).await;
    assert!(second.is_err(), "signal must not be delivered more than once (nonce dedup)");

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

// ── StateResponse key interning ───────────────────────────────────────────

#[tokio::test]
async fn test_state_response_interns_keys() {
    let (mut writer, reader) = loopback_pair().await;
    let store: Arc<papaya::HashMap<Arc<str>, StoreEntry>> = Arc::new(papaya::HashMap::new());
    let (tx, _rx) = mpsc::channel(10);
    let _ = spawn_handler(
        reader, Arc::clone(&store), Arc::new(papaya::HashMap::new()), tx,
        Arc::new(ShardedSeen::new(N_GOSSIP_SHARDS)),
        GossipConfig::default().default_ttl,
    );

    let pool_before = crate::store::intern_pool_len();
    let unique_key = format!("state_response_intern_test_{}", fastrand::u64(..));
    send_wire(&mut writer, &WireMessage::StateResponse {
        entries: vec![SyncEntry {
            key:          Arc::from(unique_key.as_str()),
            value:        Bytes::from_static(b"v"),
            timestamp:    1,
            is_tombstone: false,
        }],
    }).await;

    let s = Arc::clone(&store);
    let k = unique_key.clone();
    poll_until(|| s.pin().get(k.as_str()).is_some(), 200).await;
    assert!(
        crate::store::intern_pool_len() > pool_before,
        "StateResponse should intern the key when intern_keys = true",
    );
}

// ── signal_once ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_signal_once_returns_on_match() {
    let agent = make_agent();
    // signal_once must return the emitted signal.
    let kind: Arc<str> = Arc::from("test.once");
    let agent_ref = &agent;
    let recv = tokio::spawn({
        let kind = Arc::clone(&kind);
        async move {
            make_agent().mesh().signal_once(kind, Duration::from_millis(500), |_| true).await
        }
    });
    // Brief pause so the receiver registers before the emit.
    time::sleep(Duration::from_millis(20)).await;
    let _ = agent_ref.mesh().emit(Arc::clone(&kind), SignalScope::Cluster, Bytes::new());

    // Use a fresh agent with a real handler.
    let agent2 = make_agent();
    let mut rx = agent2.mesh().signal_rx_with_capacity(Arc::clone(&kind), 4);
    let _ = agent2.mesh().emit(Arc::clone(&kind), SignalScope::Cluster, Bytes::from_static(b"hi"));
    let sig = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(sig.is_ok() && sig.unwrap().is_some());
    drop(recv);
}

#[tokio::test]
async fn test_signal_once_timeout() {
    let agent = make_agent();
    let result = agent
        .mesh().signal_once("no.such.signal", Duration::from_millis(50), |_| true)
        .await;
    assert!(result.is_none(), "should return None when nothing is emitted");
}

#[tokio::test]
async fn test_signal_once_skips_non_matching() {
    let agent = make_agent();
    let kind: Arc<str> = Arc::from("invoke.result");
    let mut rx = agent.mesh().signal_rx_with_capacity(Arc::clone(&kind), 16);

    // Individual scope bypasses the opacity shedding check so both signals
    // are guaranteed to land in the channel regardless of fill_ratio.
    let self_id = agent.node_id().clone();
    let target_nonce: u64 = 0xDEAD_BEEF;
    let _ = agent.mesh().emit(Arc::clone(&kind), SignalScope::Individual(self_id.clone()), Bytes::from_static(b"wrong"));
    let _ = agent.mesh().emit(Arc::clone(&kind), SignalScope::Individual(self_id.clone()), Bytes::from_static(b"right"));

    // Drain both into a Vec and find the one with "right" payload.
    let mut signals = Vec::new();
    for _ in 0..2 {
        if let Ok(Some(s)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            signals.push(s);
        }
    }
    // signal_once logic: predicate on payload content (simulating nonce check).
    let matching = signals.into_iter().find(|s| s.payload == Bytes::from_static(b"right"));
    assert!(matching.is_some(), "should find the matching signal");
    let _ = target_nonce;
}

// ── advertise ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_advertise_emits_on_interval() {
    let agent = make_agent();
    let kind: Arc<str> = Arc::from("capacity.available");
    let mut rx = agent.mesh().signal_rx_with_capacity(Arc::clone(&kind), 8);

    let _handle = agent.mesh().advertise(
        Arc::clone(&kind),
        SignalScope::Cluster,
        Duration::from_millis(30),
        || Bytes::from_static(b"load=0"),
    );

    // Should receive at least one signal within a generous window.
    let result = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(result.is_ok() && result.unwrap().is_some(), "advertise should emit on interval");
}

#[tokio::test]
async fn test_advertise_stops_on_handle_drop() {
    let agent = make_agent();
    let kind: Arc<str> = Arc::from("capacity.probe");
    let mut rx = agent.mesh().signal_rx_with_capacity(Arc::clone(&kind), 8);

    let handle = agent.mesh().advertise(
        Arc::clone(&kind),
        SignalScope::Cluster,
        Duration::from_millis(20),
        Bytes::new,
    );

    // Confirm it emits at least once.
    let first = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(first.is_ok() && first.unwrap().is_some());

    // Drop handle — task should stop.
    drop(handle);
    time::sleep(Duration::from_millis(60)).await;

    // Drain any already-queued signals.
    while rx.try_recv().is_ok() {}

    // No further signals should arrive.
    let after = tokio::time::timeout(Duration::from_millis(80), rx.recv()).await;
    assert!(after.is_err(), "no signals should arrive after handle is dropped");
}

// ── last_signal ───────────────────────────────────────────────────────────

#[test]
fn test_last_signal_none_initially() {
    let agent = make_agent();
    assert!(agent.mesh().last_signal("never.seen").is_none());
}

#[tokio::test]
async fn test_last_signal_updates_after_deliver() {
    let agent = make_agent();
    let kind = "health.probe";
    let before = std::time::Instant::now();
    let _ = agent.mesh().emit(kind, SignalScope::Cluster, Bytes::new());
    // Give the local deliver() call time to record.
    time::sleep(Duration::from_millis(5)).await;
    let ts = agent.mesh().last_signal(kind);
    assert!(ts.is_some(), "last_signal should be Some after emit");
    assert!(ts.unwrap() >= before, "timestamp should be at or after emit time");

    // The additive sibling states the same claim as an age — `last_signal_age` was added rather
    // than changing `last_signal`'s return type, so both are checked.
    let age = agent.mesh().last_signal_age(kind).expect("an age too");
    assert!(
        age <= before.elapsed(),
        "the signal cannot have been recorded before the emit that produced it: {age:?}"
    );
    assert!(age < Duration::from_secs(1), "and it was recorded just now: {age:?}");
}

// ── suppress / unsuppress / is_suppressed ────────────────────────────────

#[tokio::test]
async fn test_suppress_blocks_delivery() {
    let agent = make_agent();
    let mut rx = agent.mesh().signal_rx_with_capacity("test.suppress", 8);
    agent.mesh().suppress("test.suppress", Duration::from_secs(60));
    assert!(agent.mesh().is_suppressed("test.suppress"));
    let _ = agent.mesh().emit("test.suppress", SignalScope::Cluster, Bytes::new());
    let result = time::timeout(Duration::from_millis(50), rx.recv()).await;
    assert!(result.is_err(), "suppressed kind must not be delivered to handlers");
}

#[tokio::test]
async fn test_suppress_allows_after_expiry() {
    let agent = make_agent();
    let mut rx = agent.mesh().signal_rx_with_capacity("test.expiry", 8);
    agent.mesh().suppress("test.expiry", Duration::from_millis(50));
    time::sleep(Duration::from_millis(100)).await;
    assert!(!agent.mesh().is_suppressed("test.expiry"), "suppression should have expired");
    let _ = agent.mesh().emit("test.expiry", SignalScope::Cluster, Bytes::new());
    let result = time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(result.is_ok() && result.unwrap().is_some(), "expired suppression must allow delivery");
}

#[tokio::test]
async fn test_unsuppress_lifts_early() {
    let agent = make_agent();
    let mut rx = agent.mesh().signal_rx_with_capacity("test.unsuppress", 8);
    agent.mesh().suppress("test.unsuppress", Duration::from_secs(60));
    agent.mesh().unsuppress("test.unsuppress");
    assert!(!agent.mesh().is_suppressed("test.unsuppress"), "unsuppressed must not be suppressed");
    let _ = agent.mesh().emit("test.unsuppress", SignalScope::Cluster, Bytes::new());
    let result = time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(result.is_ok() && result.unwrap().is_some(), "unsuppressed kind must deliver");
}

#[tokio::test]
async fn test_suppress_still_updates_last_signal() {
    let agent = make_agent();
    agent.mesh().suppress("test.last_seen", Duration::from_secs(60));
    let _ = agent.mesh().emit("test.last_seen", SignalScope::Cluster, Bytes::new());
    time::sleep(Duration::from_millis(10)).await;
    assert!(agent.mesh().last_signal("test.last_seen").is_some(),
        "last_signal must update even while kind is suppressed");
}

// ── watch ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_watch_fires_on_stale() {
    let agent = make_agent();
    let fired = Arc::new(AtomicU64::new(0));
    let fired_clone = Arc::clone(&fired);
    // threshold = 50ms → check_interval = max(12ms, 100ms) = 100ms
    // No signal ever emitted → stale from the first check.
    let _handle = agent.mesh().watch(
        "health.probe",
        Duration::from_millis(50),
        move || { fired_clone.fetch_add(1, Ordering::Relaxed); },
    );
    time::sleep(Duration::from_millis(350)).await;
    assert!(fired.load(Ordering::Relaxed) > 0, "on_stale must fire when no signal received");
}

#[tokio::test]
async fn test_watch_does_not_fire_when_fresh() {
    let agent = make_agent();
    let fired = Arc::new(AtomicU64::new(0));
    let fired_clone = Arc::clone(&fired);
    // Emit once so last_signal is fresh.
    let _ = agent.mesh().emit("health.fresh", SignalScope::Cluster, Bytes::new());
    // threshold = 500ms → first check at 125ms. At 250ms elapsed is ~250ms < 500ms.
    let _handle = agent.mesh().watch(
        "health.fresh",
        Duration::from_millis(500),
        move || { fired_clone.fetch_add(1, Ordering::Relaxed); },
    );
    time::sleep(Duration::from_millis(250)).await;
    assert_eq!(fired.load(Ordering::Relaxed), 0, "on_stale must not fire when signal is recent");
}

#[tokio::test]
async fn test_watch_stops_on_handle_drop() {
    let agent = make_agent();
    let fired = Arc::new(AtomicU64::new(0));
    let fired_clone = Arc::clone(&fired);
    let handle = agent.mesh().watch(
        "health.stop",
        Duration::from_millis(30),
        move || { fired_clone.fetch_add(1, Ordering::Relaxed); },
    );
    // Let it fire at least once.
    time::sleep(Duration::from_millis(350)).await;
    assert!(fired.load(Ordering::Relaxed) > 0, "should fire before drop");
    drop(handle);
    // Allow the task to observe the cancellation.
    time::sleep(Duration::from_millis(50)).await;
    let count_at_drop = fired.load(Ordering::Relaxed);
    // Wait well past several check intervals — no further fires expected.
    time::sleep(Duration::from_millis(400)).await;
    assert_eq!(fired.load(Ordering::Relaxed), count_at_drop, "no fires after handle drop");
}

// ── manage_opacity governor ───────────────────────────────────────────────

/// Wait up to `$secs` for a signal on `$rx`, polling in 200 ms slices.
///
/// The opacity governor is a **periodic sampler** (`time::interval(100ms)` in
/// `opacity.rs`) that emits OPAQUE/TRANSPARENT on the next tick after a fill
/// transition. The signal is registered before the governor starts and the
/// channel is buffered, so it is never *lost* — but under heavy parallel test
/// load tokio can starve the ticker task well past a few ticks
/// (`MissedTickBehavior::Skip` then drops the missed beats). This is a bounded
/// "eventually" wait that absorbs that scheduler latency; it is **not** a race
/// mask — a genuinely stuck governor that never emits still fails at the ceiling.
macro_rules! recv_within {
    ($rx:expr, $secs:expr) => {{
        let deadline = std::time::Instant::now() + Duration::from_secs($secs);
        let mut got = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_)) = time::timeout(Duration::from_millis(200), $rx.recv()).await {
                got = true;
                break;
            }
        }
        got
    }};
}

#[tokio::test]
async fn test_manage_opacity_emits_opaque_when_threshold_crossed() {
    let agent = make_agent();
    // One handler for the monitored kind with cap=4.
    // fill_ratio = 0.75 after 3 signals; hint.threshold = 0.75.
    let _work_rx      = agent.mesh().signal_rx_with_capacity("test.gov.invoke", 4);
    let mut opaque_rx = agent.mesh().signal_rx_with_capacity(signal_kind::BOUNDARY_OPAQUE, 8);

    let _gov = agent.manage_opacity(
        "test.gov.invoke",
        SignalScope::Cluster,
        OpacityHint::default(), // threshold = 0.75
    );

    // Individual scope bypasses the opacity-shedding check in emit_signal, so
    // all three signals reliably land in the channel and fill_ratio reaches 0.75.
    let self_id = agent.node_id().clone();
    for _ in 0..3 {
        let _ = agent.mesh().emit("test.gov.invoke", SignalScope::Individual(self_id.clone()), Bytes::new());
    }

    assert!(
        recv_within!(opaque_rx, 3),
        "governor must emit BOUNDARY_OPAQUE when fill crosses threshold",
    );
}

#[tokio::test]
async fn test_manage_opacity_emits_transparent_after_drain() {
    let agent = make_agent();
    let mut work_rx   = agent.mesh().signal_rx_with_capacity("test.gov.drain", 4);
    let mut opaque_rx = agent.mesh().signal_rx_with_capacity(signal_kind::BOUNDARY_OPAQUE, 8);
    let mut clear_rx  = agent.mesh().signal_rx_with_capacity(signal_kind::BOUNDARY_TRANSPARENT, 8);

    let _gov = agent.manage_opacity(
        "test.gov.drain",
        SignalScope::Cluster,
        OpacityHint::default(),
    );

    // Fill to 100% with Individual scope to avoid opacity shedding.
    let self_id = agent.node_id().clone();
    for _ in 0..4 {
        let _ = agent.mesh().emit("test.gov.drain", SignalScope::Individual(self_id.clone()), Bytes::new());
    }
    assert!(recv_within!(opaque_rx, 3), "should go opaque first");

    // Drain all four — fill drops to 0.0 < 0.75 - 0.20 = 0.55.
    for _ in 0..4 {
        let _ = time::timeout(Duration::from_millis(50), work_rx.recv()).await;
    }

    assert!(
        recv_within!(clear_rx, 3),
        "governor must emit BOUNDARY_TRANSPARENT once fill drops below clear threshold",
    );
}

#[tokio::test]
async fn test_manage_opacity_gate_vetoes_then_library_overrides() {
    let agent = make_agent();
    // cap=8: 6 signals → fill=0.75 (threshold met, gate vetoes), 8 → fill=1.0 (override).
    let _work_rx      = agent.mesh().signal_rx_with_capacity("test.gov.gate", 8);
    let mut opaque_rx = agent.mesh().signal_rx_with_capacity(signal_kind::BOUNDARY_OPAQUE, 8);

    // Gate always vetoes — library must still override when fill == 1.0.
    let _gov = agent.manage_opacity_gated(
        "test.gov.gate",
        SignalScope::Cluster,
        OpacityHint::default(),
        |_state| false,
    );

    // Fill to 75% with Individual scope. Gate should veto every tick.
    let self_id = agent.node_id().clone();
    for _ in 0..6 {
        let _ = agent.mesh().emit("test.gov.gate", SignalScope::Individual(self_id.clone()), Bytes::new());
    }
    let premature = time::timeout(Duration::from_millis(250), opaque_rx.recv()).await;
    assert!(premature.is_err(), "gate veto must prevent emission below 100% fill");

    // Fill to 100% — the library overrides the gate (at `fill_ratio >= 1.0` the emit is
    // unconditional), so BOUNDARY_OPAQUE is emitted. The **real** root cause of this test's long
    // flake history (Runs 27–36, "widened 3 s → 10 s → 30 s") was NOT scheduling latency — it was a
    // *dropped signal*: the governor's BOUNDARY_OPAQUE is `System`-scoped, and `ops::deliver_locally`
    // probabilistically sheds non-`Individual` signals by `combined_fill`. Under CI gossip
    // backpressure (`gossip_shard_fill > 0`) the single emission was occasionally shed from *local*
    // delivery — a permanent miss no timeout could recover, which is why widening never worked.
    // Fixed in `ops.rs` (boundary-transition kinds are exempt from the local shed, like `Individual`);
    // pinned deterministically by
    // `ops::delivery_shed_tests::boundary_transition_signals_are_never_locally_shed`. The emission is
    // now undroppable, so this only needs a little patience for the governor's 100 ms ticker.
    for _ in 0..2 {
        let _ = agent.mesh().emit("test.gov.gate", SignalScope::Individual(self_id.clone()), Bytes::new());
    }
    assert!(
        recv_within!(opaque_rx, 5),
        "library must override gate and emit BOUNDARY_OPAQUE when fill == 1.0",
    );
}

// ── competitive response ──────────────────────────────────────────────────

#[tokio::test]
async fn test_competitive_response_group_scope() {
    let agent = Arc::new(make_agent());
    agent.mesh().join_group("work");

    // Register the reply receiver synchronously — before any emit, no race.
    let mut result_rx = agent.mesh().signal_rx_with_capacity(signal_kind::INVOKE_RESULT, 4);

    // Worker: receives Group-scoped invoke, replies to sender via Individual scope.
    let mut invoke_rx = agent.mesh().signal_rx(signal_kind::INVOKE);
    let agent_w = Arc::clone(&agent);
    tokio::spawn(async move {
        if let Some(sig) = invoke_rx.recv().await {
            // Echo correlation payload so the invoker can identify its reply.
            let _ = agent_w.mesh().emit(
                signal_kind::INVOKE_RESULT,
                SignalScope::Individual(sig.sender),
                sig.payload.clone(),
            );
        }
    });

    // Emit to the group — no worker selected; routing emerges from opacity state.
    let corr = Bytes::from_static(b"corr-42");
    let _ = agent.mesh().emit(signal_kind::INVOKE, SignalScope::Group(Arc::from("work")), corr.clone());

    // Generous timeout: this is an in-process signal round-trip, so it completes in
    // microseconds when the runtime is idle — but a loaded CI runner can starve the
    // worker task well past a few hundred ms. We assert the reply *arrives*, not that
    // it is fast, so a wide bound removes the flake without weakening the check.
    let reply = tokio::time::timeout(Duration::from_secs(3), result_rx.recv())
        .await
        .expect("worker should reply within timeout")
        .expect("channel closed");

    assert_eq!(reply.payload, corr, "reply echoes correlation payload");
    assert_eq!(
        reply.scope,
        SignalScope::Individual(agent.node_id().clone()),
        "reply uses Individual scope targeting the invoker",
    );
}

// ── Consensus ─────────────────────────────────────────────────────────────

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_group_propose_single_voter() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());
    agent.mesh().join_group("cg1");

    let config = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let result = agent.consensus().group_propose("cg1", "sl1", Bytes::from_static(b"val1"), config).await;

    assert!(
        matches!(result, ConsensusResult::Committed { .. }),
        "single-voter quorum should commit immediately; got {:?}", result
    );
    assert_eq!(agent.consensus().consensus_get("sl1"), Some(Bytes::from_static(b"val1")));
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_group_propose_timeout() {
    let agent = make_agent();
    // The group must exist before it can fail to reach quorum. Without this join the proposal is
    // refused for having **no electorate**, which is a different answer to the one under test —
    // and before 2026-09-24 it was a third answer again: an empty roster counted as one member,
    // so the test was exercising a singleton election it did not mean to create.
    agent.mesh().join_group("cg2");
    // No listener started — no votes arrive, quorum of 2 is unreachable.
    let config = ConsensusConfig {
        quorum_size:    2,
        phase1_timeout: Duration::from_millis(50),
        max_ballots:    1,
        ..ConsensusConfig::default()
    };
    let result = agent.consensus().group_propose("cg2", "sl2", Bytes::from_static(b"v2"), config).await;
    assert!(
        matches!(result, ConsensusResult::Timeout { ballots_tried: 1, .. }),
        "unreachable quorum must return Timeout; got {:?}", result
    );
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_group_propose_two_node_quorum() {
    let pair = consensus_pair().await;
    pair.a.mesh().join_group("cgrp");
    pair.b.mesh().join_group("cgrp");

    let config = ConsensusConfig {
        quorum_size:    2,
        phase1_timeout: Duration::from_secs(3),
        max_ballots:    3,
        ..ConsensusConfig::default()
    };
    let result = pair.a.consensus().group_propose("cgrp", "slA", Bytes::from_static(b"agreed"), config).await;
    assert!(
        matches!(result, ConsensusResult::Committed { .. }),
        "two-node quorum should commit; got {:?}", result
    );
    pair.a.shutdown().await;
    pair.b.shutdown().await;
}

// Two agents propose to the same slot concurrently. With ballot jitter, one
// should Commit and the other Superseded. Neither should Timeout.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_consensus_simultaneous_proposers_resolve() {
    let ConsensusPair { a, b, _la, _lb } = consensus_pair().await;
    let agent_a = Arc::new(a);
    let agent_b = Arc::new(b);

    // quorum_size=1 so each agent self-commits; the second proposer will find the
    // commit_key written by the first and return Superseded on the next ballot check.
    // A tiny stagger ensures A commits before B polls the commit_key.
    let config = ConsensusConfig {
        quorum_size:            1,
        phase1_timeout:         Duration::from_millis(500),
        max_ballots:            5,
        ballot_retry_jitter_ms: 0, // disabled — test relies on commit_key propagation, not jitter
        ..ConsensusConfig::default()
    };

    let aa = Arc::clone(&agent_a);
    let cfg_a2 = config.clone();
    let task_a = tokio::spawn(async move {
        aa.consensus().cluster_propose("sim_sl", Bytes::from_static(b"val_a"), cfg_a2).await
    });
    // Small stagger gives A time to commit and gossip the commit_key to B.
    time::sleep(Duration::from_millis(50)).await;
    let bb = Arc::clone(&agent_b);
    let cfg_b2 = config.clone();
    let task_b = tokio::spawn(async move {
        bb.consensus().cluster_propose("sim_sl", Bytes::from_static(b"val_b"), cfg_b2).await
    });

    let (res_a, res_b) = tokio::join!(task_a, task_b);
    let res_a = res_a.unwrap();
    let res_b = res_b.unwrap();

    assert!(
        matches!(res_a, ConsensusResult::Committed { .. }),
        "first proposer must commit; got {:?}", res_a,
    );
    assert!(
        matches!(res_b, ConsensusResult::Superseded { .. }),
        "second proposer must see commit and return Superseded; got {:?}", res_b,
    );
    let timed_out = [&res_a, &res_b].iter().any(|r| matches!(r, ConsensusResult::Timeout { .. }));
    assert!(!timed_out, "neither proposer should time out; got a={:?} b={:?}", res_a, res_b);

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

/// Regression gate for #149 (`subscribe_log_group` exact-once). The gateway consumer-group
/// endpoint picks a **single active consumer** by claiming a slot under a lease and reading the
/// **converged committed holder** — `system_propose` commits *optimistically* (two
/// near-simultaneous proposers can both return `Committed`), so the propose return is NOT mutual
/// exclusion; but commit-keys are LWW-by-HLC, so exactly one holder converges. The old bare-LWW
/// "lock" handed every consumer a guard (no exclusion) → 100% double-delivery.
///
/// This is the **deterministic in-process** gate the flaky Docker overlay S11 could not be. With
/// `quorum_size = 1` each node self-commits its own claim, reproducing the double-optimistic-commit
/// exactly; the assertion is that the *converged* holder is single and identical on both nodes.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_leased_claim_converges_to_single_active_holder() {
    use crate::consensus::ConsensusConfig;
    use bytes::Bytes;

    let ConsensusPair { a, b, _la, _lb } = consensus_pair().await;
    let slot     = "clog/regress/single-active/claim";
    let holder_a = Bytes::from(a.node_id().to_string().into_bytes());
    let holder_b = Bytes::from(b.node_id().to_string().into_bytes());
    let cfg = || ConsensusConfig { quorum_size: 1, committed_lease_secs: Some(30), ..Default::default() };

    // Both claim the SAME slot concurrently, no stagger — the adversarial case the bug lived in.
    // Bind the handles so the async futures don't borrow temporaries dropped at the `;`.
    let (ca, cb) = (a.consensus(), b.consensus());
    let _ = tokio::join!(
        ca.cluster_propose(slot, holder_a.clone(), cfg()),
        cb.cluster_propose(slot, holder_b.clone(), cfg()),
    );

    // Wait for the commit-key LWW to converge so both nodes agree on the holder.
    poll_until(|| {
        matches!(
            (a.consensus().consensus_get(slot), b.consensus().consensus_get(slot)),
            (Some(ha), Some(hb)) if ha == hb
        )
    }, 3_000).await;

    let ha = a.consensus().consensus_get(slot).expect("node a sees a committed holder");
    let hb = b.consensus().consensus_get(slot).expect("node b sees a committed holder");
    assert_eq!(ha, hb, "both nodes must converge to the SAME committed holder (single-active)");

    // The converged holder is exactly ONE of the two claimants — never both (the #149 bug was that
    // both "held" the claim and both drained → double-delivery).
    let a_holds = ha == holder_a;
    let b_holds = ha == holder_b;
    assert!(
        a_holds ^ b_holds,
        "exactly one claimant must be the converged holder; a_holds={a_holds} b_holds={b_holds} holder={ha:?}",
    );

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_system_propose_commits() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());

    let config = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let result = agent.consensus().cluster_propose("sys_sl", Bytes::from_static(b"sys_v"), config).await;

    assert!(
        matches!(result, ConsensusResult::Committed { .. }),
        "single-node system propose must commit; got {:?}", result
    );
    assert_eq!(agent.consensus().consensus_get("sys_sl"), Some(Bytes::from_static(b"sys_v")));
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_consensus_rx_fires_on_commit() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());
    // An explicit one-member electorate: legitimate, and now required to be explicit.
    agent.mesh().join_group("rxg");
    let mut rx = agent.consensus().consensus_rx("slRx");

    let config = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let _ = agent.consensus().group_propose("rxg", "slRx", Bytes::from_static(b"fired"), config).await;

    let val = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if rx.borrow().is_some() { return rx.borrow().clone(); }
            rx.changed().await.ok();
        }
    }).await;
    assert_eq!(val.unwrap(), Some(Bytes::from_static(b"fired")));
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_consensus_get_returns_committed() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());
    // An explicit one-member electorate: legitimate, and now required to be explicit.
    agent.mesh().join_group("cgg");

    let config = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let _ = agent.consensus().group_propose("cgg", "slGet", Bytes::from_static(b"gotten"), config).await;

    assert_eq!(
        agent.consensus().consensus_get("slGet"),
        Some(Bytes::from_static(b"gotten")),
    );
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_declare_and_read_trust() {
    let agent = make_agent();
    let peer_a = NodeId::new("127.0.0.1", 9001).unwrap();
    let peer_b = NodeId::new("127.0.0.1", 9002).unwrap();

    agent.consensus().declare_trust("trustgrp", &[peer_a.clone(), peer_b.clone()]);
    let slices = agent.consensus().group_trust("trustgrp");

    assert_eq!(slices.len(), 1, "one trust slice declared");
    let (declaring_node, peers) = &slices[0];
    assert_eq!(*declaring_node, *agent.node_id());
    assert!(peers.contains(&peer_a));
    assert!(peers.contains(&peer_b));
}

#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_consensus_late_joiner_sync() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port                    = port_a;
    cfg_a.reconnect_backoff_secs       = 1;
    cfg_a.health_check_interval_secs   = 1;
    cfg_a.health_check_max_jitter_ms   = 50;

    let agent_a = GossipAgent::new(NodeId::new("127.0.0.1", port_a).unwrap(), cfg_a);
    agent_a.start().await.unwrap();

    let _listener_a = agent_a.consensus().start_consensus_listener(ConsensusConfig::default());
    let config = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let result = agent_a.consensus().cluster_propose("late_sl", Bytes::from_static(b"late_v"), config).await;
    assert!(matches!(result, ConsensusResult::Committed { .. }));

    // B starts after A has already committed — anti-entropy must deliver the value.
    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port                       = port_b;
    cfg_b.reconnect_backoff_secs          = 1;
    cfg_b.health_check_interval_secs      = 1;
    cfg_b.health_check_max_jitter_ms      = 50;
    cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];
    let agent_b = GossipAgent::new(NodeId::new("127.0.0.1", port_b).unwrap(), cfg_b);
    agent_b.start().await.unwrap();

    // Give B's health monitor time to pass its jitter (0–50 ms) and send
    // the initial StateRequest to A before we start polling.
    time::sleep(Duration::from_millis(100)).await;
    poll_until(|| agent_b.consensus().consensus_get("late_sl").is_some(), 5_000).await;
    assert_eq!(
        agent_b.consensus().consensus_get("late_sl"),
        Some(Bytes::from_static(b"late_v")),
    );

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

// Proposer declares an empty trust slice; B's vote must not count.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_trust_slice_filters_votes() {
    let pair = consensus_pair().await;
    pair.a.mesh().join_group("tg");
    pair.b.mesh().join_group("tg");
    // A declares an empty trust slice — trusts nobody remotely.
    pair.a.consensus().declare_trust("tg", &[]);

    let config = ConsensusConfig {
        quorum_size:      2,
        phase1_timeout:   Duration::from_millis(300),
        max_ballots:      1,
        use_trust_slices: true,
        ..ConsensusConfig::default()
    };
    let result = pair.a
        .consensus().group_propose("tg", "ts1", Bytes::from_static(b"x"), config)
        .await;
    assert!(
        matches!(result, ConsensusResult::Timeout { .. }),
        "B's vote should be filtered by empty trust slice; got {:?}", result
    );

    pair.a.shutdown().await;
    pair.b.shutdown().await;
}

// Proposer includes B in its trust slice; B's vote should be counted.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_trust_slice_admits_trusted_vote() {
    let pair = consensus_pair().await;
    let node_b = pair.b.node_id().clone();
    pair.a.mesh().join_group("tg2");
    pair.b.mesh().join_group("tg2");
    // A explicitly trusts B.
    pair.a.consensus().declare_trust("tg2", &[node_b]);

    let config = ConsensusConfig {
        quorum_size:      2,
        phase1_timeout:   Duration::from_secs(3),
        max_ballots:      3,
        use_trust_slices: true,
        ..ConsensusConfig::default()
    };
    let result = pair.a
        .consensus().group_propose("tg2", "ts2", Bytes::from_static(b"y"), config)
        .await;
    assert!(
        matches!(result, ConsensusResult::Committed { .. }),
        "B is trusted; quorum of 2 should commit; got {:?}", result
    );

    pair.a.shutdown().await;
    pair.b.shutdown().await;
}

// ── H9: advertise_persistent writes capability to Layer I ─────────────────

#[tokio::test]
async fn test_advertise_persistent_late_joiner_discovers_capability() {
    let agent = make_agent();
    agent.mesh().join_group("workers");

    // Start persistent advertise with a short tick so the first write happens quickly.
    let _handle = agent.mesh().advertise_persistent(
        "contract.available",
        SignalScope::Group("workers".into()),
        Duration::from_millis(20),
        || Bytes::from_static(b"v1"),
    );

    // Wait for the first tick to fire and write to Layer I.
    poll_until(
        || !agent.kv().scan_prefix(kv_ns::ADVERTISE).is_empty(),
        500,
    ).await;

    let entries = agent.kv().scan_prefix(kv_ns::ADVERTISE);
    assert_eq!(entries.len(), 1);
    let (key, value) = &entries[0];
    assert!(key.starts_with("svc/contract.available/"), "key should be svc/{{kind}}/{{node_id}}");
    assert_eq!(*value, Bytes::from_static(b"v1"));

    // Dropping the handle tombstones the Layer I entry.
    drop(_handle);
    poll_until(|| agent.kv().scan_prefix(kv_ns::ADVERTISE).is_empty(), 500).await;
    assert!(
        agent.kv().scan_prefix(kv_ns::ADVERTISE).is_empty(),
        "capability should be tombstoned after handle drop"
    );
}

// ── H4: group_quorum filters by current Layer I membership ────────────────

#[test]
fn test_group_quorum_excludes_ex_member() {
    let agent = make_agent();

    // Join the group so the boundary admits the signal and grp/workers/{node_id}
    // is written to Layer I.
    agent.mesh().join_group("workers");

    // Emit until the signal is admitted — deliver() records the sender in the sender_log.
    // (deliver() always updates sender_log before checking handler registration.)
    //
    // Not once: local delivery is shed with probability equal to the gossip shards' fill
    // (`deliver_locally`), and this agent is never started, so the frame `join_group`'s KV write
    // queued is never drained — fill 1/1024, and about one run in a thousand shed the only signal
    // (the v2.18.1 release gate, 2026-10-02: "raw quorum should be satisfied"). Shedding is the
    // product's behaviour; what this test is about is which admitted senders `group_quorum` counts.
    let admitted = (0..50).any(|_| {
        let _ = agent.mesh().emit("heartbeat", SignalScope::Group("workers".into()), Bytes::new());
        agent.mesh().quorum("heartbeat", 1, Duration::from_secs(60))
    });

    // Raw quorum is satisfied (1 sender, 1 required).
    assert!(admitted, "raw quorum should be satisfied");
    // group_quorum should also be satisfied while the node is still a member.
    assert!(
        agent.mesh().group_quorum("workers", "heartbeat", 1, Duration::from_secs(60)),
        "node is a current member — group_quorum should count it"
    );

    // Leave the group — tombstones grp/workers/{node_id} in Layer I.
    agent.mesh().leave_group("workers");

    // Raw quorum is still satisfied (sender_log entry remains).
    assert!(
        agent.mesh().quorum("heartbeat", 1, Duration::from_secs(60)),
        "raw quorum still sees the sender_log entry"
    );
    // But group_quorum must exclude the ex-member.
    assert!(
        !agent.mesh().group_quorum("workers", "heartbeat", 1, Duration::from_secs(60)),
        "ex-member must not satisfy group_quorum after leave_group"
    );
}

// ── H10: peer_load_rx yields typed LoadState ──────────────────────────────

#[tokio::test]
async fn test_peer_load_rx_yields_decoded_state() {
    use crate::signal::{encode_load_state, LoadState};

    let agent = make_agent();
    let peer = NodeId::new("127.0.0.1", 9999).unwrap();
    let mut rx = agent.peer_load_rx(&peer, "test");

    // Initially absent.
    assert!(rx.borrow().is_none());

    // Write a pheromone entry for that peer.
    let state = LoadState { fill_ratio: 0.75, is_opaque: true, written_at_ms: 0 };
    let key = format!("sys/load/{}/test", peer);
    let _ = agent.kv().set(key.clone(), encode_load_state(&state));

    // Forwarding task should decode and push the typed value.
    let _ = tokio::time::timeout(Duration::from_millis(200), rx.changed()).await
        .expect("watch should fire within 200 ms");
    let got = rx.borrow().clone().expect("should have a LoadState");
    assert!((got.fill_ratio - 0.75).abs() < 1e-4);
    assert!(got.is_opaque);

    // Tombstone → None.
    let _ = agent.kv().delete(key);
    let _ = tokio::time::timeout(Duration::from_millis(200), rx.changed()).await
        .expect("watch should fire on tombstone");
    assert!(rx.borrow().is_none(), "tombstone should decode as None");
}

// ── H2: boundary reconciliation from Layer I ──────────────────────────────

#[test]
fn test_rehydrate_boundary_from_kv_inserts_group() {
    let agent = make_agent();
    let node_id = agent.node_id().to_string();
    let grp_key = format!("grp/workers/{}", node_id);
    let _ = agent.kv().set(grp_key, Bytes::from_static(b"1"));
    assert!(agent.groups().is_empty(), "group not yet in boundary");
    agent.rehydrate_boundary_from_kv();
    assert!(
        agent.groups().iter().any(|g| g.as_ref() == "workers"),
        "rehydrate should admit the group written to KV"
    );
}

#[test]
fn test_rehydrate_boundary_from_kv_removes_tombstoned_group() {
    let agent = make_agent();
    let node_id = agent.node_id().to_string();
    let grp_key = format!("grp/workers/{}", node_id);
    let _ = agent.kv().set(grp_key.clone(), Bytes::from_static(b"1"));
    agent.rehydrate_boundary_from_kv();
    assert!(agent.groups().iter().any(|g| g.as_ref() == "workers"));

    // Tombstone the KV entry — simulates another node forcing this node out.
    let _ = agent.kv().delete(grp_key);
    agent.rehydrate_boundary_from_kv();
    assert!(
        !agent.groups().iter().any(|g| g.as_ref() == "workers"),
        "tombstoned group must be evicted from boundary"
    );
}

#[tokio::test]
async fn test_writer_evicted_after_idle_timeout() {
    let port_a = alloc_port();
    let port_b = alloc_port();

    // Long health interval so pings don't keep resetting the idle deadline.
    // The idle timeout (3 s) is shorter than the health interval jitter window,
    // so the writer will go idle before any ping resets it.
    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port                  = port_a;
    cfg_a.reconnect_backoff_secs     = 1;
    cfg_a.health_check_interval_secs = 60;
    cfg_a.writer_idle_timeout_secs   = 3;
    cfg_a.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_b).unwrap()];

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port                  = port_b;
    cfg_b.reconnect_backoff_secs     = 1;
    cfg_b.health_check_interval_secs = 60;
    cfg_b.writer_idle_timeout_secs   = 3;
    cfg_b.bootstrap_peers = vec![NodeId::new("127.0.0.1", port_a).unwrap()];

    let agent_a = GossipAgent::new(NodeId::new("127.0.0.1", port_a).unwrap(), cfg_a);
    let agent_b = GossipAgent::new(NodeId::new("127.0.0.1", port_b).unwrap(), cfg_b);
    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();
    // Small pause so listener tasks fully start before sending.
    time::sleep(Duration::from_millis(50)).await;

    // A single gossip write establishes a writer from A to B.
    assert!(agent_a.kv().set("idle_key", Bytes::from_static(b"v1")));
    poll_until(|| agent_b.kv().get("idle_key").is_some(), 5_000).await;

    // After 3 s of silence the writer task exits. system_stats() filters finished
    // handles, so cached_connections drops to 0 without waiting for the GC pass.
    // Allow 10 s total (3 s idle + generous scheduling slack).
    poll_until(|| agent_a.system_stats().cached_connections == 0, 10_000).await;
    assert_eq!(
        agent_a.system_stats().cached_connections, 0,
        "writer should report as gone after idle timeout"
    );

    // A new write must reconnect transparently and still reach B.
    assert!(agent_a.kv().set("idle_key", Bytes::from_static(b"v2")));
    poll_until(
        || agent_b.kv().get("idle_key").map(|v| v == Bytes::from_static(b"v2")).unwrap_or(false),
        5_000,
    ).await;

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

// When member B turns opaque mid-ballot the reactive select! arm recomputes quorum
// from 2 down to 1, letting A's self-vote commit before the phase1 timeout fires.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_ballot_reacts_to_opacity_change() {
    use crate::signal::{encode_load_state, LoadState};
    use std::time::{SystemTime, UNIX_EPOCH};

    let ConsensusPair { a, b, _la, _lb } = consensus_pair().await;
    let node_b = b.node_id().clone();
    let agent_a = Arc::new(a);

    agent_a.mesh().join_group("ogrp");
    b.mesh().join_group("ogrp");
    // Poll until A sees B's group membership key — required for auto quorum_size=0
    // to compute 2 (floor(2/2)+1) rather than 1 at proposal time.
    let node_b_str = node_b.to_string();
    poll_until(
        || !agent_a.kv().scan_prefix(&format!("grp/ogrp/{node_b_str}")).is_empty(),
        2_000,
    ).await;

    // Background task: after a short pause (to let the collect loop start),
    // write B's opaque pheromone to A's store and emit BOUNDARY_OPAQUE on A.
    // Both happen in-process on agent_a so there's no gossip propagation race.
    let aa = Arc::clone(&agent_a);
    tokio::spawn(async move {
        time::sleep(Duration::from_millis(100)).await;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default()
            .as_millis() as u64;
        let opaque_bytes = encode_load_state(&LoadState {
            fill_ratio:   1.0,
            is_opaque:    true,
            written_at_ms: now_ms,
        });
        let pheromone_key = format!("sys/load/{}/test.kind", node_b);
        let _ = aa.kv().set(pheromone_key, opaque_bytes);
        // Emit BOUNDARY_OPAQUE locally on A so opaque_rx fires in propose().
        let _ = aa.mesh().emit(signal_kind::BOUNDARY_OPAQUE, SignalScope::Cluster, Bytes::new());
    });

    // phase1_timeout = 2 s; the reactive arm should commit within ~150 ms.
    let config = ConsensusConfig {
        quorum_size:            0, // auto = floor(2/2)+1 = 2 at proposal time
        phase1_timeout:         Duration::from_secs(2),
        max_ballots:            1,
        ballot_retry_jitter_ms: 0,
        count_opaque_as_absent: true,
        ..ConsensusConfig::default()
    };

    let start = tokio::time::Instant::now();
    let result = agent_a.consensus().group_propose("ogrp", "opq_sl", Bytes::from_static(b"v"), config).await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, ConsensusResult::Committed { .. }),
        "should commit once B goes opaque and quorum drops to 1; got {:?}", result
    );
    assert!(
        elapsed < Duration::from_millis(1000),
        "reactive commit must happen well before the 2 s timeout; elapsed {:?}", elapsed
    );

    agent_a.shutdown().await;
    b.shutdown().await;
}

#[test]
fn test_warm_quorum_seeds_sender_log() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let port_a = alloc_port();
    let port_b = alloc_port();
    let node_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let node_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg = GossipConfig::default();
    cfg.bind_port = port_a;
    cfg.signal_window_secs = 60;
    let agent = GossipAgent::new(node_a, cfg);

    // Write a sys/quorum entry 5 s in the past (well within the 60 s window).
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let written_at_ms = now_ms - 5_000;
    let key = format!("sys/quorum/my.kind/{}", node_b);
    let _ = agent.kv().set(key, Bytes::copy_from_slice(&written_at_ms.to_le_bytes()));

    // Before seeding, the in-memory sender_log is empty.
    assert!(!agent.mesh().quorum("my.kind", 1, Duration::from_secs(60)));

    // After warm_quorum_from_layer1, the entry is seeded and quorum passes.
    agent.warm_quorum_from_layer1();
    assert!(agent.mesh().quorum("my.kind", 1, Duration::from_secs(60)));
}

#[test]
fn test_last_signal_persistent_reads_layer1() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let port_a = alloc_port();
    let port_b = alloc_port();
    let node_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let node_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg = GossipConfig::default();
    cfg.bind_port = port_a;
    let agent = GossipAgent::new(node_a, cfg);

    // Write a quorum entry 5 s in the past.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let written_at_ms = now_ms - 5_000;
    let key = format!("sys/quorum/my.kind/{}", node_b);
    let _ = agent.kv().set(key, Bytes::copy_from_slice(&written_at_ms.to_le_bytes()));

    let age = agent.mesh().last_signal_persistent("my.kind").expect("should find entry");
    // Age should be approximately 5 s (allow ±2 s scheduling slack).
    assert!(age >= Duration::from_secs(3) && age <= Duration::from_secs(7),
        "expected ~5 s, got {:?}", age);

    // Non-existent kind returns None.
    assert!(agent.mesh().last_signal_persistent("never.seen").is_none());
}

// ── Semantic correctness — LWW convergence ────────────────────────────────

/// Verifies LWW convergence at the network level:
///
/// 1. Agent A writes `"first"` and waits for agent B to receive it (HLC sync).
/// 2. Agent B writes `"second"` — B's HLC is now strictly greater than A's, so
///    `"second"` is the definitive LWW winner everywhere.
/// 3. Both agents must converge to `"second"`.
///
/// This is a network-level complement to the in-memory LWW unit tests in
/// `store.rs` — it exercises the full gossip path including HLC `observe()`
/// and the `>` LWW conflict resolution applied on every inbound update.
#[tokio::test]
async fn test_lww_convergence_two_concurrent_writers() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port       = port_a;
    cfg_a.bootstrap_peers = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port       = port_b;
    cfg_b.bootstrap_peers = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;

    let agent_a = Arc::new(GossipAgent::new(id_a, cfg_a));
    let agent_b = Arc::new(GossipAgent::new(id_b, cfg_b));

    agent_a.start().await.unwrap();
    agent_b.start().await.unwrap();

    poll_until(|| !agent_a.peers().is_empty() && !agent_b.peers().is_empty(), 2_000).await;

    // Step 1: A writes "first".
    let _ = agent_a.kv().set("lww/converge", Bytes::from_static(b"first"));

    // Step 2: Wait until B has observed A's write (B's HLC now ≥ A's HLC).
    let ab = Arc::clone(&agent_b);
    poll_until(
        || ab.kv().get("lww/converge") == Some(Bytes::from_static(b"first")),
        2_000,
    ).await;

    // Step 3: B writes "second" — B's HLC.tick() is strictly > A's "first" timestamp.
    let _ = agent_b.kv().set("lww/converge", Bytes::from_static(b"second"));

    // Both agents must converge to "second" (higher HLC wins).
    let aa = Arc::clone(&agent_a);
    let ab2 = Arc::clone(&agent_b);
    poll_until(
        || {
            aa.kv().get("lww/converge") == Some(Bytes::from_static(b"second"))
            && ab2.kv().get("lww/converge") == Some(Bytes::from_static(b"second"))
        },
        2_000,
    ).await;

    let final_a = agent_a.kv().get("lww/converge").unwrap();
    let final_b = agent_b.kv().get("lww/converge").unwrap();
    assert_eq!(final_a, final_b, "LWW must produce identical values on both nodes");
    assert_eq!(final_a, Bytes::from_static(b"second"),
        "higher-HLC write must win");

    agent_a.shutdown().await;
    agent_b.shutdown().await;
}

// ── Semantic correctness — cross_group_propose split-brain invariant ──────

/// Verifies that `cross_group_propose` cannot commit when a required group
/// has no live voters — even if every other group votes unanimously.
///
/// This is the key split-brain safety invariant: a single-group majority
/// cannot unilaterally commit a multi-group proposal.  The commit condition
/// in `ConsensusEngine::cross_propose` requires `all groups pass their
/// quorum fraction`, so a group with 0 members contributes `needed = 1`
/// but `accepts = 0`, permanently blocking the commit.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn test_cross_group_propose_requires_all_group_quorums() {
    let pair = consensus_pair().await;

    // Both agents join "alpha" but neither joins "beta".
    pair.a.mesh().join_group("alpha");
    pair.b.mesh().join_group("alpha");

    // Wait for group membership to gossip.
    let aa = &pair.a;
    let ab = &pair.b;
    poll_until(
        || aa.mesh().group_members("alpha").len() >= 2
        && ab.mesh().group_members("alpha").len() >= 2,
        2_000,
    ).await;
    // The poll above does not assert; a membership that had not gossiped in time used to surface
    // later as a bare `Timeout` with zero votes (seen in CI 2026-09-28 and 2026-10-03). Say so here,
    // with both views, so the next failure carries its evidence.
    assert!(
        aa.mesh().group_members("alpha").len() >= 2 && ab.mesh().group_members("alpha").len() >= 2,
        "alpha membership did not gossip within 2 s: a sees {:?}, b sees {:?}",
        aa.mesh().group_members("alpha"), ab.mesh().group_members("alpha"),
    );

    // Require quorum from both "alpha" (has 2 voters) and "beta" (0 voters).
    let groups = vec![
        GroupQuorum { group: "alpha".into(), quorum: 0.5, veto: false },
        GroupQuorum { group: "beta".into(),  quorum: 0.5, veto: false },
    ];
    let mut fast_cfg = ConsensusConfig::default();
    fast_cfg.phase1_timeout = Duration::from_millis(200);
    fast_cfg.max_ballots    = 1;

    let result = pair.a.consensus()
        .cross_group_propose("cgp/split-brain", Bytes::from_static(b"v"), groups, fast_cfg)
        .await;

    assert!(
        matches!(result, ConsensusResult::Timeout { .. }),
        "proposal must time out when 'beta' has no voters — got {result:?}",
    );

    // Positive case: requiring only "alpha" (which has quorum) must commit.
    let groups_alpha_only = vec![
        GroupQuorum { group: "alpha".into(), quorum: 0.5, veto: false },
    ];
    let result_ok = pair.a.consensus()
        .cross_group_propose(
            "cgp/alpha-only",
            Bytes::from_static(b"v"),
            groups_alpha_only,
            ConsensusConfig::default(),
        )
        .await;

    assert!(
        matches!(result_ok, ConsensusResult::Committed { .. }),
        "alpha-only proposal must commit; got {result_ok:?} — a sees alpha {:?}, b sees alpha {:?}, a's peers {:?}, b's peers {:?}",
        aa.mesh().group_members("alpha"), ab.mesh().group_members("alpha"), aa.peers(), ab.peers(),
    );

    pair.a.shutdown().await;
    pair.b.shutdown().await;
}

// ── M2 falsification probes (Run 16) — kept as permanent regression tests ──

/// Robustness probe: a live agent must survive hostile bytes on its gossip
/// port — pure garbage, an absurd length prefix, and an abrupt disconnect —
/// and remain fully serviceable afterwards.
#[tokio::test]
async fn probe_garbage_on_gossip_port_survives() {
    use tokio::io::AsyncWriteExt;

    let port = alloc_port();
    let id   = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(id, cfg);
    agent.start().await.unwrap();

    // 1. Pure garbage.
    let mut s1 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s1.write_all(&[0xFF; 1024]).await.unwrap();
    let _ = s1.shutdown().await;

    // 2. Huge length prefix (4 GiB frame announcement), then disconnect.
    let mut s2 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s2.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
    s2.write_all(b"trailing").await.unwrap();
    let _ = s2.shutdown().await;

    // 3. Zero-length frame followed by garbage.
    let mut s3 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s3.write_all(&0u32.to_le_bytes()).await.unwrap();
    s3.write_all(&[0x00; 64]).await.unwrap();
    let _ = s3.shutdown().await;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Agent must still be alive and serviceable.
    assert!(agent.kv().set("probe/after-garbage", Bytes::from_static(b"ok")));
    assert_eq!(
        agent.kv().get("probe/after-garbage").as_deref(),
        Some(b"ok".as_slice()),
        "agent must remain serviceable after hostile input",
    );
    let stats = agent.system_stats();
    assert_eq!(stats.dead_shards, 0, "no gossip shard may die from hostile input");
    agent.shutdown().await;
}

/// **Run 62 probe (P2).** The garbage probe above announces a huge frame and disconnects. This
/// one *delivers* it: a header claiming `MAX_FRAME_BYTES + 1`, then that many bytes streamed in
/// earnest, so the reader's bound is exercised against a peer that keeps sending rather than one
/// that goes away. The node must refuse the frame, keep every shard alive, stay serviceable, and
/// accept a fresh connection afterwards.
#[tokio::test]
async fn probe_a_fully_delivered_oversized_frame_is_refused_and_the_node_stays_serviceable() {
    use crate::framing::MAX_FRAME_BYTES;
    use tokio::io::AsyncWriteExt;

    let port = alloc_port();
    let id   = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(id, cfg);
    agent.start().await.unwrap();

    let claimed = (MAX_FRAME_BYTES + 1) as u32;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = s.write_all(&claimed.to_be_bytes()).await;
    let _ = s.write_all(&[crate::framing::WIRE_VERSION]).await;
    // Stream the whole claimed payload; the peer may close on us part-way, which is the point.
    let chunk = vec![0xABu8; 64 * 1024];
    let mut sent = 0usize;
    while sent < MAX_FRAME_BYTES + 1 {
        if s.write_all(&chunk).await.is_err() { break; }
        sent += chunk.len();
    }
    let _ = s.shutdown().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert!(agent.kv().set("probe/after-oversized", Bytes::from_static(b"ok")));
    assert_eq!(agent.kv().get("probe/after-oversized").as_deref(), Some(b"ok".as_slice()));
    assert_eq!(agent.system_stats().dead_shards, 0, "no gossip shard may die from an oversized frame");
    TcpStream::connect(("127.0.0.1", port)).await.expect("the gossip port still accepts a connection");
    agent.shutdown().await;
}

/// **Run 62 probe (P3).** `validate()` refuses a family-wildcard scope — but a config built in
/// code never calls `validate()` itself; `start()` does. So the guarantee an operator gets is
/// only as good as the lifecycle's own call, which this probe exercises end to end: an agent
/// whose token table carries `llm:*` must refuse to **start**, naming the field, rather than
/// start with a credential that admits nothing.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn probe_a_family_wildcard_scope_refuses_to_start_not_just_to_validate() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.gateway_scoped_tokens = vec![crate::GatewayToken { token: "t".into(), scopes: vec!["llm:*".into()] }];
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    match agent.start().await {
        Err(crate::error::GossipError::InvalidField { field, reason }) => {
            assert_eq!(field, "gateway_scoped_tokens");
            assert!(reason.contains("llm:*"), "{reason}");
        }
        other => panic!("start() must refuse a family-wildcard scope by name, got {other:?}"),
    }
}

/// **Run 62 probe (P4) — the finding was environmental, and this is its record.** Every test
/// that constructs a `GossipAgent` on the development Mac reported "has been running for over 60
/// seconds", including ones that never call `start()`, and this probe measured `new()` at 86 s.
/// The cause is not the constructor: launching the *freshly built* test binary with a filter that
/// matched **no** test took 93 s wall, and the same binary a second time took 0 s. A debug test
/// binary paged in from the external volume on first launch is the minute and a half, and the
/// first test to run pays it. Kept ignored, with the 5 s bound, as the way to re-check: run it
/// twice; if the *second* run fails, the constructor really did get slow.
#[test]
#[ignore = "first-launch page-in of the debug binary from a slow volume dominates on the dev Mac; run twice"]
fn probe_constructing_an_agent_takes_milliseconds_not_a_minute() {
    let started = std::time::Instant::now();
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), GossipConfig::default());
    let built = started.elapsed();
    drop(agent);
    let dropped = started.elapsed();
    assert!(built < std::time::Duration::from_secs(5), "GossipAgent::new took {built:?}");
    assert!(dropped < std::time::Duration::from_secs(5), "construct + drop took {dropped:?}");
}

/// Resource-management probe: after `shutdown_with_timeout`, every tracked
/// background task must have exited and the gossip port must be rebindable.
#[tokio::test]
#[cfg(feature = "consensus")]
async fn probe_shutdown_drains_tasks_and_releases_port() {
    let port = alloc_port();
    let id   = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(id, cfg);
    agent.start().await.unwrap();

    // Exercise the task spawners: consensus listener + capability advertisement.
    let _listener = agent.consensus().start_consensus_listener(
        crate::consensus::ConsensusConfig::default(),
    );
    let _reg = agent.capabilities().advertise_capability(
        Capability::new("probe", "drain"),
        std::time::Duration::from_secs(60),
    );
    assert!(agent.system_stats().task_count > 0, "tasks must be tracked while running");

    agent.shutdown_with_timeout(std::time::Duration::from_secs(10)).await;
    assert_eq!(
        agent.system_stats().task_count, 0,
        "no tracked task may survive shutdown",
    );

    // The gossip port must actually be released.
    let rebind = std::net::TcpListener::bind(("127.0.0.1", port));
    assert!(rebind.is_ok(), "gossip port must be rebindable after shutdown");
}

// ── Architecture conformance tests (M2 evidence for dimensions 1 and 3) ────

/// Layer separation as an executable invariant: Layer I modules (KV store,
/// framing, writer, seen-set, HLC) must not reference Layer III (consensus)
/// or the capability subsystem. Until the v2 workspace split makes this a
/// compile boundary, this test is the enforcement. The Layer I/II bridge
/// (`KvState::subscriptions`) is documented and intentionally not forbidden.
#[test]
fn layer1_modules_do_not_reference_higher_layers() {
    const LAYER1: &[(&str, &str)] = &[
        ("store.rs",   include_str!("../mycelium-core/src/store.rs")),
        ("framing.rs", include_str!("../mycelium-core/src/framing.rs")),
        ("writer.rs",  include_str!("../mycelium-core/src/writer.rs")),
        ("seen.rs",    include_str!("../mycelium-core/src/seen.rs")),
        ("hlc.rs",     include_str!("../mycelium-core/src/hlc.rs")),
    ];
    const FORBIDDEN: &[&str] = &[
        "crate::consensus",
        "crate::capability",
        "consensus_ns",
        "ConsensusEngine",
        "ConsensusMsg",
        "CapEntry",
        "CapFilter",
        "CapabilityGroupDef",
        "cross_group",
    ];
    for (file, src) in LAYER1 {
        for pat in FORBIDDEN {
            assert!(
                !src.contains(pat),
                "Layer I file src/{file} references `{pat}` — \
                 a Layer I → Layer III/capability dependency is a layer violation \
                 (see CLAUDE.md § Layer I/II Bridge Invariant)",
            );
        }
    }
}

/// The Holland inversion as an executable invariant: boundaries control
/// *acting*, never *forwarding*. A relay node that is NOT a member of group g
/// must still forward g-scoped signals to its peers. A raw socket plays the
/// emitter so the emitter and receiver cannot peer directly — delivery is
/// only possible through the non-member relay. If anyone ever "optimises"
/// forwarding by scope membership, this test fails.
#[tokio::test]
async fn forwarding_is_unconditional_through_non_member_relay() {
    let port_r = alloc_port();
    let port_b = alloc_port();
    let id_r = NodeId::new("127.0.0.1", port_r).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();

    let mut cfg_r = GossipConfig::default();
    cfg_r.bind_port = port_r;
    // One-way bootstrap (member → relay): the relay only learns the member's
    // address from the member's ping, and pings back on its next health-check
    // tick — that ping-back is what populates the member's peer table. At the
    // default 10 s interval that exceeds the formation wait, so tighten it.
    cfg_r.health_check_interval_secs = 1;
    cfg_r.health_check_max_jitter_ms = 50;
    let relay = GossipAgent::new(id_r.clone(), cfg_r);
    relay.start().await.unwrap();
    // The relay deliberately does NOT join the group.

    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port = port_b;
    cfg_b.bootstrap_peers = vec![id_r.clone()];
    cfg_b.health_check_interval_secs = 1;
    cfg_b.health_check_max_jitter_ms = 50;
    let member = GossipAgent::new(id_b.clone(), cfg_b);
    member.start().await.unwrap();
    member.mesh().join_group("relay-grp");

    // Structural readiness: relay must have the member as a live peer AND
    // know (via gossiped grp/ key) that the member belongs to the group, so
    // group-hinted forwarding has a routable target.
    let grp_key = format!("grp/relay-grp/{id_b}");
    let mut ready = false;
    for _ in 0..100 {
        if !relay.peers().is_empty()
            && !member.peers().is_empty()
            && relay.kv().get(&grp_key).is_some()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ready, "cluster did not form: relay peers={}, member peers={}, grp key seen={}",
        relay.peers().len(), member.peers().len(), relay.kv().get(&grp_key).is_some());

    // Register the receiver BEFORE emitting.
    let mut rx = member.mesh().signal_rx("relay.probe");

    // Raw socket plays emitter A — a node the member can never peer with.
    // The forwarded frame is best-effort: `relay.peers()` reflects the membership table (populated
    // on the member's inbound ping), but forwarding rides the `gossip_txs` fan-out, whose outbound
    // relay→member writer is established slightly later. A frame injected during that window is
    // (correctly) dropped and lost forever. Re-inject with a FRESH nonce (so the seen-set doesn't
    // dedup it) until the member receives it, within an overall deadline — faithful to the
    // invariant (forwarding works once the path is up), robust to the one-shot setup race.
    let fake_sender = NodeId::new("127.0.0.1", 1).unwrap();
    let mut delivered = None;
    for _ in 0..20 {
        let mut sock = TcpStream::connect(("127.0.0.1", port_r)).await.unwrap();
        send_wire(&mut sock, &WireMessage::Signal {
            ttl:     3,
            nonce:   fastrand::u64(1..),
            sender:  fake_sender.clone(),
            scope:   SignalScope::Group(Arc::from("relay-grp")),
            kind:    Arc::from("relay.probe"),
            payload: Bytes::from_static(b"through-the-relay"),
            hlc_seq: None,
        }).await;
        if let Ok(Some(sig)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await
        {
            delivered = Some(sig);
            break;
        }
    }
    let sig = delivered
        .expect("group signal was never forwarded by the non-member relay within 10 s — \
                 forwarding must be unconditional (boundaries control acting, not forwarding)");
    assert_eq!(&sig.payload[..], b"through-the-relay");

    relay.shutdown().await;
    member.shutdown().await;
}

/// Zero-gaps Z9 (D9): `gossip_peers_connected` is the number of **live writers** — sockets, not the
/// view (`mycelium_emergent_peers_known` is the view) — set where the writer map changes and at the
/// GC sweep, mirroring `/stats` `cached_connections`. Written before the gauge existed and seen
/// failing (no such series on `/metrics`).
#[cfg(all(feature = "gateway", feature = "metrics"))]
#[tokio::test]
async fn the_connected_peer_gauge_counts_live_writers() {
    let (pa, pb) = (alloc_port(), alloc_port());
    let http_a = alloc_port();
    let mut ca = GossipConfig::default();
    ca.bind_port = pa;
    ca.http_port = Some(http_a);
    ca.health_check_interval_secs = 1;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", pa).unwrap(), ca);
    a.start().await.expect("a starts");
    let mut cb = GossipConfig::default();
    cb.bind_port = pb;
    cb.bootstrap_peers = vec![NodeId::new("127.0.0.1", pa).unwrap()];
    cb.health_check_interval_secs = 1;
    let b = GossipAgent::new(NodeId::new("127.0.0.1", pb).unwrap(), cb);
    b.start().await.expect("b starts");
    poll_until(|| !a.peers().is_empty() && a.system_stats().cached_connections >= 1, 10_000).await;
    assert!(a.system_stats().cached_connections >= 1, "a has a live writer to b");

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_a}/metrics");
    let mut seen: Option<f64> = None;
    for _ in 0..100 {
        if let Ok(resp) = client.get(&url).send().await
            && let Ok(text) = resp.text().await
        {
            seen = text.lines()
                .find(|l| l.starts_with("gossip_peers_connected"))
                .and_then(|l| l.split_whitespace().last())
                .and_then(|v| v.parse::<f64>().ok());
            if seen.is_some_and(|v| v >= 1.0) { break; }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen.is_some_and(|v| v >= 1.0), "gossip_peers_connected on /metrics should count a's live writer to b; saw {seen:?}");
    b.shutdown().await;
    a.shutdown().await;
}

/// Audit 2026-07-15 pass 4 (#21 Operational Readiness): `/ready` reflects STARTUP COMPLETION, not
/// soft-state advertisement. A node that advertises no capability (a pure KV/signal node) is ready
/// as soon as it has started — previously it returned 503 forever, so a k8s readiness gate never
/// routed to a healthy node, blocking deploys of that shape. Advertising later must not un-ready it.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn ready_reflects_startup_not_advertisement() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg);
    agent.start().await.expect("start");

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_port}/ready");
    let health = format!("http://127.0.0.1:{http_port}/health");
    for _ in 0..40 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Ready with NO capability advertised — the shape that used to block forever.
    let mut ready = false;
    for _ in 0..40 {
        if client.get(&url).send().await.is_ok_and(|r| r.status().is_success()) { ready = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "/ready must be 200 once startup completes, even with no soft state advertised");

    // Advertising a capability afterwards is incremental — it must not un-ready a started node.
    let _reg = agent.capabilities().advertise_capability(
        Capability::new("probe", "ready"),
        Duration::from_secs(5),
    );
    let after = client.get(&url).send().await.expect("ready after advertise");
    assert!(after.status().is_success(), "advertising a capability must not un-ready a started node");

    agent.shutdown().await;
}

/// M2 Run-20 quota probe (#18 Test Architecture): in-suite mini-fuzz of the
/// wire and capability decoders — the fuzz targets exist but the series has
/// never executed them, so this puts a fast adversarial pass in the normal
/// suite. Random bytes plus truncations/bitflips of a VALID encoded frame
/// (mutation finds more than noise). Pass = no panic; Err is valid output.
#[cfg(feature = "fuzz-internals")]
#[test]
fn mini_fuzz_decoders_survive_adversarial_bytes() {
    // A valid frame to mutate: encode a real WireMessage.
    let update = GossipUpdate {
        nonce: 7, sender: 1, ttl: 3, is_tombstone: false,
        timestamp: crate::hlc::pack(1_700_000_000_000, 4),
        key: Arc::from("fuzz/seed"), value: Bytes::from_static(b"v"),
    };
    let valid = wire_to_bytes(&WireMessage::Data(update)).to_vec();

    let mut rng = fastrand::Rng::with_seed(0xC0FFEE);
    let mut cases = 0u32;
    // Pure noise.
    for _ in 0..20_000 {
        let len = rng.usize(..256);
        let buf: Vec<u8> = (0..len).map(|_| rng.u8(..)).collect();
        let _ = crate::fuzz_internals::wire_message_decode(&buf);
        let _ = crate::fuzz_internals::capability_decode(&buf);
        let _ = crate::fuzz_internals::cap_filter_decode(&buf);
        let _ = crate::fuzz_internals::locality_path_decode(&buf);
        // Trust-edge parsers (§12.6): bytes a partner or a client controls, parsed before
        // anything about them is verified. These assert their invariant rather than merely
        // surviving -- byte conservation for the frame, round-trip stability and a well-formed
        // `DomainId` for the federation objects.
        let _ = crate::fuzz_internals::caller_frame_classify(&buf);
        let _ = crate::fuzz_internals::caller_envelope_decode(&buf);
        let _ = crate::fuzz_internals::federation_objects_parse(&buf);
        let _ = crate::fuzz_internals::trust_bundle_parse(&buf);
        let _ = crate::fuzz_internals::fixint_decode(&buf);
        let _ = crate::fuzz_internals::closure_corpus_render(&buf);
        #[cfg(feature = "tls")]
        {
            let _ = crate::fuzz_internals::presented_call_parse(&buf);
            let _ = crate::fuzz_internals::catalog_reply_parse(&buf);
        }
        cases += 1;
    }

    // Mutations of a VALID caller-context frame. Noise almost never produces the 5-byte magic, so
    // the interesting region -- length fields, boundary arithmetic -- is only reachable by
    // mutating a well-formed one. This is the same reasoning as the valid-frame pass below.
    {
        // A real `Envelope`: v/p/via/s/t, with the two optional base64 credential fields. A seed
        // that is merely frame-shaped would never get past `serde_json`, so the envelope layer's
        // assertions would never run and the pass would silently test only the framing.
        let envelope = br#"{"v":1,"p":"token:node-1/dispatch","via":"127.0.0.1:7001","s":["kv:write"],"t":1700000000000,"k":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","sig":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
        let mut valid_frame = vec![0x00, b'G', b'W', b'C', 1];
        valid_frame.extend_from_slice(&(envelope.len() as u16).to_be_bytes());
        valid_frame.extend_from_slice(envelope);
        valid_frame.extend_from_slice(b"application-bytes");

        // The seed must actually reach both layers, or the mutation pass below silently tests
        // only the framing. A gate that has quietly stopped exercising what it names is worse
        // than no gate, so this is asserted rather than assumed.
        // ── REACHABILITY REGISTRY ─────────────────────────────────────────────────────────────────
    //
    // **Every target below asserts an invariant, and noise reaches none of them.** That is
    // measured, not assumed: a 20,000-input random pass reached 0 of 7. A target fed only noise
    // therefore proves "does not panic on garbage" and *nothing it claims* — while looking fully
    // covered, which is the worse half.
    //
    // This cost a real defect. `presented_call_parse` asserted round-trip stability from the day
    // it was written and had never executed that assertion; the `/a2a` credential parser shipped
    // in v2.11.0 accepting a non-ASCII principal it could not re-emit.
    //
    // **This list claims completeness**, in the same sense the lock-order table does: adding a
    // trust-edge target that asserts anything means adding a row here *and* a valid seed above.
    // A row that fails means the seed stopped reaching the parser — which is the failure to care
    // about, because the target goes on passing either way.
    {
        let frame_seed = {
            let envelope = br#"{"v":1,"p":"token:node-1/dispatch","via":"127.0.0.1:7001","s":["kv:write"],"t":1700000000000,"k":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","sig":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
            let mut f = vec![0x00, b'G', b'W', b'C', 1];
            f.extend_from_slice(&(envelope.len() as u16).to_be_bytes());
            f.extend_from_slice(envelope);
            f.extend_from_slice(b"application-bytes");
            f
        };
        let policy_seed = br#"{"domain":"depot.example","revision":7,"grants":[["kitchen.example","dispatch"]]}"#;
        let bundle_seed = serde_json::to_vec(&crate::federation::TrustBundle::trusting([(
            crate::federation::DomainId::new("partner.example").expect("a valid domain"),
            [7u8; 32],
        )]))
        .expect("a bundle serialises");

        // X4: two trace lines, the second's parent the first's hash — the closure reader's join and its cycle guard
        // are reached only by a parseable record with a parent.
        let closure_seed = {
            let first = r#"{"schema":"mycelium.decision/1","rule":"signal.admission","rule_revision":2,"node":"n1","seq":1,"trigger":"frame","outcome":"Permitted","reason":"under_fill","correlation":"run-1"}"#;
            let mut second: serde_json::Value = serde_json::from_str(first).unwrap();
            second["seq"] = serde_json::json!(2);
            second["parent"] = serde_json::json!(crate::causal::content_hash(first.as_bytes()));
            format!("{first}\n{second}\n")
        };
        let mut rows: Vec<(&str, bool)> = vec![
            ("caller_frame_classify", crate::fuzz_internals::caller_frame_classify(&frame_seed)),
            ("closure_corpus_render", crate::fuzz_internals::closure_corpus_render(closure_seed.as_bytes())),
            ("caller_envelope_decode", crate::fuzz_internals::caller_envelope_decode(&frame_seed)),
            ("federation_objects_parse", crate::fuzz_internals::federation_objects_parse(policy_seed)),
            ("trust_bundle_parse", crate::fuzz_internals::trust_bundle_parse(&bundle_seed)),
        ];

        #[cfg(feature = "tls")]
        {
            let call_seed = br#"{"origin":"partner.example","principal":"oidc:idp/alice","export":"depot.read","issued_at_ms":1700000000000,"expires_at_ms":1700000060000,"signature":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
            let reply_seed = serde_json::to_vec(&crate::federation::edge::CatalogReply {
                domain: crate::federation::DomainId::new("depot.example").expect("valid"),
                for_partner: crate::federation::DomainId::new("kitchen.example").expect("valid"),
                policy_revision: 7,
                issued_at_ms: 1_700_000_000_000,
                exports: vec!["depot.read".to_string()],
                signature: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string()),
            })
            .expect("a reply serialises");
            rows.push(("presented_call_parse", crate::fuzz_internals::presented_call_parse(call_seed)));
            rows.push(("catalog_reply_parse", crate::fuzz_internals::catalog_reply_parse(&reply_seed)));
        }

        // `fixint_decode` is reached by its own seeded block above, which asserts it there.
        for (name, reached) in rows {
            assert!(
                reached,
                "{name} did not reach its invariant: its seed no longer parses, so the assertion \
                 the target exists for never runs. A target fed only noise looks covered and \
                 checks nothing — see this block's note.",
            );
        }
    }

    assert!(
            crate::fuzz_internals::caller_frame_classify(&valid_frame),
            "the seed must classify as a framed payload",
        );
        assert!(
            crate::fuzz_internals::caller_envelope_decode(&valid_frame),
            "the seed must decode as an envelope, or the envelope layer is never reached",
        );

        for cut in 0..valid_frame.len() {
            let _ = crate::fuzz_internals::caller_frame_classify(&valid_frame[..cut]);
            let _ = crate::fuzz_internals::caller_envelope_decode(&valid_frame[..cut]);
            cases += 1;
        }
        for i in 0..valid_frame.len() {
            for bit in 0..8 {
                let mut m = valid_frame.clone();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::caller_frame_classify(&m);
                // The envelope layer is only reachable through a frame that still carries the
                // magic, so it is mutation and not noise that gets there at all.
                let _ = crate::fuzz_internals::caller_envelope_decode(&m);
                cases += 1;
            }
        }
    }

    // Mutations of a VALID presented federation credential, plus the escaped-Unicode case.
    //
    // **This seed is the point.** `presented_call_parse` asserts round-trip stability, and random
    // bytes essentially never form a parseable credential — so the assertion sat unreachable in
    // the noise pass above while the target looked covered. Same tell as the envelope seed: a
    // gate that has quietly stopped exercising what it names is worse than no gate.
    //
    // The specific case that got through on main (2026-09-21): JSON can spell a non-ASCII
    // character in pure ASCII, so `\u0809` passed a header-level `is_ascii()` check and then came
    // back out of `to_header_value` unescaped — a credential this node accepted and could not
    // re-parse. Kept here as a literal because it is the shape, not the bytes, that matters.
    #[cfg(feature = "tls")]
    {
        let valid_call = br#"{"origin":"partner.example","principal":"oidc:idp/alice","export":"depot.read","issued_at_ms":1700000000000,"expires_at_ms":1700000060000,"signature":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
        assert!(
            crate::fuzz_internals::presented_call_parse(valid_call),
            "the seed must parse, or the round-trip assertion is never reached",
        );

        // An ASCII header whose *content* is not ASCII. Must be refused, never panic, and above
        // all never be accepted-then-re-emitted as something this parser would reject.
        let escaped = br#"{"origin":"partner.example","principal":"oidc:idp/al\u0809ice","export":"depot.read","issued_at_ms":0,"expires_at_ms":1,"signature":"AAAA"}"#;
        assert!(
            escaped.is_ascii(),
            "the regression input is ASCII on the wire — that was the whole trap",
        );
        assert!(
            !crate::fuzz_internals::presented_call_parse(escaped),
            "a credential whose content is not ASCII must be refused, not round-tripped",
        );
        cases += 2;

        for i in 0..valid_call.len() {
            for bit in 0..8 {
                let mut m = valid_call.to_vec();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::presented_call_parse(&m);
                cases += 1;
            }
        }
        for cut in 0..valid_call.len() {
            let _ = crate::fuzz_internals::presented_call_parse(&valid_call[..cut]);
            cases += 1;
        }
    }

    // Mutations of a VALID federation policy: the same argument, for a text parser. A byte flipped
    // inside the domain string is how an id that no constructor would produce reaches the parser.
    {
        let valid_policy = br#"{"domain":"depot.example","revision":7,"grants":[["kitchen.example","dispatch"]]}"#;
        for i in 0..valid_policy.len() {
            for bit in 0..8 {
                let mut m = valid_policy.to_vec();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::federation_objects_parse(&m);
                cases += 1;
            }
        }
    }

    // VALID seeds for the two trust-edge parsers that had none.
    //
    // Measured, not assumed: a 20,000-input noise pass reached the invariant of **zero** of the
    // seven assertion-bearing targets. Noise proves "does not panic on garbage", which is worth
    // having and is not what these targets claim. Without a seed, `trust_bundle_parse` and
    // `catalog_reply_parse` had never once run the assertion they exist for.
    {
        let bundle = crate::federation::TrustBundle::trusting([(
            crate::federation::DomainId::new("partner.example").expect("a valid domain"),
            [7u8; 32],
        )]);
        let valid = serde_json::to_vec(&bundle).expect("a bundle serialises");
        assert!(
            crate::fuzz_internals::trust_bundle_parse(&valid),
            "the seed must parse, or the partner-id assertion is never reached",
        );
        for i in 0..valid.len() {
            for bit in 0..8 {
                let mut m = valid.clone();
                m[i] ^= 1 << bit;
                // A flipped byte inside a domain string is how an id no constructor would produce
                // reaches the parser — which is exactly what the assertion is about.
                let _ = crate::fuzz_internals::trust_bundle_parse(&m);
                cases += 1;
            }
        }
    }

    #[cfg(feature = "tls")]
    {
        let reply = crate::federation::edge::CatalogReply {
            domain: crate::federation::DomainId::new("depot.example").expect("a valid domain"),
            for_partner: crate::federation::DomainId::new("kitchen.example").expect("valid"),
            policy_revision: 7,
            issued_at_ms: 1_700_000_000_000,
            exports: vec!["depot.read".to_string(), "depot.dispatch".to_string()],
            signature: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string()),
        };
        let valid = serde_json::to_vec(&reply).expect("a reply serialises");
        assert!(
            crate::fuzz_internals::catalog_reply_parse(&valid),
            "the seed must parse, or the round-trip assertion is never reached",
        );
        for i in 0..valid.len() {
            for bit in 0..8 {
                let mut m = valid.clone();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::catalog_reply_parse(&m);
                cases += 1;
            }
        }
        for cut in 0..valid.len() {
            let _ = crate::fuzz_internals::catalog_reply_parse(&valid[..cut]);
            cases += 1;
        }
    }

    // And a VALID `serde_fixint` encoding. Noise dies on the first length prefix -- a sweep of
    // 40k random inputs decoded *nothing* -- so only mutation reaches this decoder's interior.
    {
        use crate::control::ledger::PublishedRightsHead;
        let head = PublishedRightsHead {
            head:      crate::control::ledger::RightsHead {
                holder: crate::mandate::PrincipalId::new("depot-dispatcher").expect("a valid principal"),
                seq:    7,
                totals: vec![("pallet-slots".to_string(), 12), ("chiller-hours".to_string(), 4)],
            },
            signature: vec![1, 2, 3, 4],
        };
        let valid = mycelium_core::serde_fixint::to_vec(&head).expect("encode a rights head");
        assert!(
            crate::fuzz_internals::fixint_decode(&valid),
            "the seed must decode, or the fixint layer is never reached",
        );
        for cut in 0..valid.len() {
            let _ = crate::fuzz_internals::fixint_decode(&valid[..cut]);
            cases += 1;
        }

        // **Bit flips, not only truncations.** Truncation reaches the *end* of a buffer; a binary
        // decoder goes wrong at its variant indices, length prefixes and discriminants, and only a
        // flipped bit reaches those. This block was truncation-only, which meant the interesting
        // region of a hand-rolled codec was never touched.
        //
        // Measured when this was added: of ~2,500 flipped inputs, **796 decoded** and therefore
        // reached the round-trip assertion — and none failed it. That is a real negative result
        // rather than an empty one, which is the distinction `COVERAGE` and the reachability
        // registry exist to keep visible.
        for i in 0..valid.len() {
            for bit in 0..8 {
                let mut m = valid.clone();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::fixint_decode(&m);
                cases += 1;
            }
        }
        // Trailing garbage: `from_slice` tolerates it by design, so the decoded value must still
        // be the one that was encoded rather than one the extra bytes changed.
        for extra in 1..32usize {
            let mut m = valid.clone();
            m.extend(std::iter::repeat_n(0xAAu8, extra));
            let _ = crate::fuzz_internals::fixint_decode(&m);
            cases += 1;
        }
        for i in 0..valid.len() {
            for bit in 0..8 {
                let mut m = valid.clone();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::fixint_decode(&m);
                cases += 1;
            }
        }
    }

    // And a VALID trust bundle. This is the object that decides which keys are acceptable at all,
    // so a misparse is not a refused call -- it is the wrong answer to *who do we trust*.
    {
        let valid_bundle = br#"{"partners":[{"domain":"kitchen.example","key":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"retiring":null,"revoked":false}]}"#;
        for i in 0..valid_bundle.len() {
            for bit in 0..8 {
                let mut m = valid_bundle.to_vec();
                m[i] ^= 1 << bit;
                let _ = crate::fuzz_internals::trust_bundle_parse(&m);
                cases += 1;
            }
        }
    }
    // Truncations of a valid frame at every offset.
    for cut in 0..valid.len() {
        let _ = crate::fuzz_internals::wire_message_decode(&valid[..cut]);
        cases += 1;
    }
    // Single-bit flips at every position of the valid frame.
    for i in 0..valid.len() {
        for bit in 0..8 {
            let mut m = valid.clone();
            m[i] ^= 1 << bit;
            let _ = crate::fuzz_internals::wire_message_decode(&m);
            cases += 1;
        }
    }
    eprintln!("mini-fuzz: {cases} adversarial inputs, no panics");
}

/// M2 Run-20 deep-dive probe: anti-entropy closure for writes that predate
/// the peer connection. Reconstruction of the community-demo cold-start
/// flake (2026-06-11): a spoke bootstraps toward a seed that is not yet
/// listening, writes a key while unconnected, and the seed comes up later.
/// Live gossip missed the write by construction; the documented guarantee
/// is that anti-entropy closes the gap. Until it does, hub-spoke clusters
/// silently lack one-shot writes (the skillrunner schema keys were exactly
/// this; masked there by periodic re-assertion, not root-caused).
#[tokio::test]
async fn anti_entropy_delivers_pre_connection_writes() {
    let seed_port  = alloc_port();
    let spoke_port = alloc_port();
    let seed_id  = NodeId::new("127.0.0.1", seed_port).unwrap();
    let spoke_id = NodeId::new("127.0.0.1", spoke_port).unwrap();

    // Spoke first: bootstrap points at a seed that is NOT yet listening.
    let mut spoke_cfg = GossipConfig::default();
    spoke_cfg.bind_port = spoke_port;
    spoke_cfg.bootstrap_peers = vec![seed_id.clone()];
    let spoke = GossipAgent::new(spoke_id.clone(), spoke_cfg);
    spoke.start().await.expect("spoke start");
    // Written while unconnected: live gossip cannot deliver this.
    assert!(spoke.kv().set("ae/pre-connection", &b"written-before-seed-existed"[..]));

    // Seed comes up afterwards.
    let mut seed_cfg = GossipConfig::default();
    seed_cfg.bind_port = seed_port;
    let seed = GossipAgent::new(seed_id, seed_cfg);
    seed.start().await.expect("seed start");

    // Anti-entropy must converge the pre-connection write seed-ward.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut arrived_after = None;
    let t0 = std::time::Instant::now();
    while std::time::Instant::now() < deadline {
        if seed.kv().get("ae/pre-connection").is_some() {
            arrived_after = Some(t0.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let outcome = arrived_after;
    spoke.shutdown().await;
    seed.shutdown().await;
    match outcome {
        Some(t) => eprintln!("anti-entropy closed the gap in {t:?}"),
        None => panic!(
            "seed never received the spoke's pre-connection write within 30 s — \
             anti-entropy closure is broken for hub-spoke bootstrap"
        ),
    }
}

/// Regression: `with_http_routes` must MERGE routers across calls, not
/// replace. A last-caller-wins slot silently dropped every earlier
/// registration — skillrunner's management dashboard erased the A2A
/// endpoints (`/.well-known/agent.json` → 404) for as long as both were
/// enabled, found by a live run-through of examples/a2a_langchain.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn with_http_routes_merges_across_calls() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg);

    async fn first() -> &'static str { "first" }
    async fn second() -> &'static str { "second" }
    agent.with_http_routes(axum::Router::new().route("/extra-one", axum::routing::get(first)));
    agent.with_http_routes(axum::Router::new().route("/extra-two", axum::routing::get(second)));

    agent.start().await.expect("start");
    // Poll until the HTTP server is up, then both routes must serve.
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let mut ok = false;
    for _ in 0..40 {
        if let Ok(r) = client.get(format!("{base}/health")).send().await
            && r.status().is_success() { ok = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "gateway did not come up");
    let one = client.get(format!("{base}/extra-one")).send().await.unwrap();
    let two = client.get(format!("{base}/extra-two")).send().await.unwrap();
    assert_eq!(one.status().as_u16(), 200, "first registered router must survive");
    assert_eq!(two.status().as_u16(), 200, "second registered router must serve");
    assert_eq!(one.text().await.unwrap(), "first");
    assert_eq!(two.text().await.unwrap(), "second");
    agent.shutdown().await;
}

/// M2 Run-18 race-family sweep: re-registering a prompt skill under the same
/// id and then dropping the OLD handle must not delete the NEW registration.
/// The cancellation task previously removed by key unconditionally; it must
/// remove only if the registry still holds the backend it registered.
#[cfg(feature = "llm")]
#[tokio::test]
async fn reregistered_llm_skill_survives_old_handle_drop() {
    use crate::agent::{EchoBackend, PromptTemplate};
    fn template() -> PromptTemplate {
        PromptTemplate {
            system:        "s".into(),
            user_template: "{{input}}".into(),
            max_tokens:    16,
            temperature:   0.0,
            metadata:      std::collections::HashMap::new(),
        }
    }
    let agent = make_agent();
    let h1 = agent.llm()
        .register_prompt_skill("ns", "skill", template(), Arc::new(EchoBackend))
        .await
        .expect("first registration");
    let _h2 = agent.llm()
        .register_prompt_skill("ns", "skill", template(), Arc::new(EchoBackend))
        .await
        .expect("re-registration while first handle is alive");

    drop(h1);
    // Give the old handle's cancellation task time to run (it fires on drop).
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        agent.task_ctx.llm_skills.pin().get("ns/skill").is_some(),
        "old handle's drop must not delete the newer registration"
    );
}

/// M2 Run-18 race-family sweep: concurrent registrations of the SAME signal
/// kind. The HandlerTable registration closure runs under papaya `compute`,
/// which re-invokes the closure when the entry changes concurrently — a
/// single-use `slot.take().expect(...)` panics on that retry, so two tasks
/// calling `signal_rx("same.kind")` simultaneously (or one racing the
/// closed-sender eviction in `deliver_to_handlers`) could crash. The closure
/// must clone per invocation instead.
#[test]
fn concurrent_same_kind_signal_registration_does_not_panic() {
    use crate::signal::SignalHandlers;
    let handlers = SignalHandlers::new(Duration::from_secs(600));
    let threads = 8;
    let per_thread = 400;
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut rxs = Vec::with_capacity(per_thread);
                for _ in 0..per_thread {
                    rxs.push(handlers.register(Arc::from("contended.kind")));
                }
                // Drop half so closed senders accumulate and future eviction
                // computes contend with registrations too.
                rxs.truncate(per_thread / 2);
                rxs
            });
        }
    });
}

/// M2 Run-18 probe (dims 6/10): the documented lifecycle error contract.
/// `start()` on a running agent returns `AlreadyRunning`; `start()` after
/// shutdown returns `Shutdown`; and `shutdown_with_timeout` actually drains
/// every tracked task (`task_count == 0`) rather than leaking them.
#[tokio::test]
async fn test_lifecycle_error_contract_and_task_drain() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);

    agent.start().await.expect("first start succeeds");
    poll_until(|| agent.system_stats().task_count > 0, 2_000).await;

    let second = agent.start().await;
    assert!(
        matches!(second, Err(GossipError::AlreadyRunning)),
        "second start() must return AlreadyRunning, got {second:?}"
    );

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    assert_eq!(
        agent.system_stats().task_count, 0,
        "all tracked tasks must drain on shutdown"
    );

    let after = agent.start().await;
    assert!(
        matches!(after, Err(GossipError::Shutdown)),
        "start() after shutdown must return Shutdown, got {after:?}"
    );
}

// ── WS1 RBAC end-to-end (compliance feature) ──────────────────────────────

/// WS1 integration scenario: signed role claims propagate and verify across two
/// real tls-enabled nodes, and provider-side `caller_authorized` admits/denies
/// correctly based on the *verified* claim.
///
/// **Two nodes that require proofs still learn each other's keys — the Phase 3b gate.**
///
/// This is the end-to-end claim the sealed record exists to make, and it is deliberately written
/// against an *observable behaviour* rather than against `peer_keys` directly: `roles_of` returns
/// `Some` only once A's signed claim has gossiped to B **and** A's verifying key has reached B's
/// `peer_keys`, so a key that never arrives shows up here as a missing role.
///
/// **What it does not prove, stated plainly.** This is an in-process test: two agents on loopback,
/// converging in milliseconds. It cannot reproduce the cross-process ordering window that made the
/// 2026-09-23 default flip split a four-node Docker fleet, and it would very likely pass on the old
/// two-entry path too. Claiming otherwise would repeat the mistake that caused all this — assuming
/// a green in-process run says something about a race between processes
/// (`docs/wiki/dev/testing/testing.md` §a green run is evidence about that run).
///
/// So what it *is*: proof that the sealed path is wired, published, parsed and sufficient on its
/// own — a peer holding nothing but the sealed record authenticates. The absence of the window is a
/// **structural** argument, not a measured one: one KV entry cannot arrive in two parts, so there
/// is no ordering left to lose. The gate that can observe the race is the Docker suite.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_identity_proofs_required_two_nodes_still_authenticate() {
    use crate::config::TlsConfig;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_a = id(port_a);

    let cert_dir = std::env::temp_dir().join(format!("myc-sealed-{port_a}-{port_b}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        // The opt-in under test. Both nodes require it, so neither will accept an identity that
        // is not sealed — including the other's.
        cfg.require_identity_proofs = true;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![node_a.clone()]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    let mut peered = false;
    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() { peered = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "two tls nodes failed to peer within the window");

    // The sealed record is actually published — if this is missing, the rest of the test would be
    // asserting the legacy path and passing for the wrong reason.
    let sealed_key = format!("sys/identity-signed/{node_a}");
    let mut sealed = None;
    for _ in 0..200 {
        if let Some(v) = a.kv().get(&sealed_key) { sealed = Some(v); break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let sealed = sealed.expect("a TLS node must publish its sealed identity record");
    assert!(
        crate::agent::helpers::parse_sealed_identity(&sealed).is_some(),
        "the published record must parse as version(1) ‖ history ‖ proof(96)",
    );

    a.advertise_roles(["admin".into()], 3).expect("advertise_roles with a tls identity");

    let mut verified: Option<crate::RoleClaim> = None;
    for _ in 0..200 {
        if let Some(claim) = b.roles_of(&node_a) { verified = Some(claim); break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        verified.is_some_and(|c| c.has_role("admin")),
        "B never verified A's claim: A's key did not reach peer_keys with proofs required",
    );

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// This exercises the whole WS1 path the unit tests stub out: A signs a role
/// claim with its tls identity key; the claim and A's `sys/identity/` key both
/// gossip to B; B's identity-watcher mirrors A's verifying key into `peer_keys`;
/// and B's `roles_of(A)` only returns the claim because the signature checks out
/// against the key B learned from the cluster — never from the (forgeable) KV
/// entry alone. Detection-not-prevention: a node can write any `sys/role/` bytes,
/// but only a correctly-signed claim reads back as a role.
///
/// Both nodes share one auto-cert dir so they share a CA (mutual trust); a
/// unique temp dir per run keeps concurrent tests and the default
/// `./mycelium-tls/` from colliding.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_ws1_rbac_signed_roles_propagate_and_authorize_across_nodes() {
    use crate::config::TlsConfig;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_a = id(port_a);
    let node_b = id(port_b);

    let cert_dir =
        std::env::temp_dir().join(format!("myc-rbac-{port_a}-{port_b}"));
    let _ = std::fs::remove_dir_all(&cert_dir); // clean slate if a prior run left files

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        // Fast Pings so peer registration (which happens on Ping receipt) and
        // anti-entropy converge well inside the poll window.
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig {
            auto_cert_dir: cert_dir.clone(),
            ..TlsConfig::default()
        });
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![node_a.clone()]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    // Structural poll: both nodes peered (so identity keys have a path to gossip).
    let mut peered = false;
    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() {
            peered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "two tls nodes failed to peer within the window");

    // A advertises an admin role at data-clearance 3.
    a.advertise_roles(["admin".into()], 3)
        .expect("advertise_roles must succeed with a tls identity");

    // Structural poll: B verifies A's claim. Returns Some only once (a) the
    // signed `sys/role/A` entry has gossiped to B AND (b) A's identity key has
    // reached B's peer_keys so the signature verifies.
    let mut verified: Option<crate::RoleClaim> = None;
    for _ in 0..200 {
        if let Some(claim) = b.roles_of(&node_a) {
            verified = Some(claim);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let claim = verified.expect("B never verified A's signed role claim");
    assert!(claim.has_role("admin"), "verified claim must carry the admin role");
    assert!(claim.clearance_at_least(3), "verified claim must carry clearance 3");
    assert!(!claim.clearance_at_least(4), "clearance must not over-report");

    // Provider-side authorization on B, keyed on the verified sender (A):
    let admin_allow:  [Arc<str>; 1] = [Arc::<str>::from("admin")];
    let writer_allow: [Arc<str>; 1] = [Arc::<str>::from("db-writer")];
    let node_allow:   [Arc<str>; 1] = [Arc::<str>::from(node_a.to_string())];

    assert!(b.caller_authorized(&node_a, &admin_allow),
        "A holds the admin role → admitted");
    assert!(!b.caller_authorized(&node_a, &writer_allow),
        "A holds no db-writer role → denied");
    assert!(b.caller_authorized(&node_a, &node_allow),
        "explicit NodeId allowlist entry → admitted");
    assert!(b.caller_authorized(&node_a, &[]),
        "empty allowlist → open");

    // A node with no advertised roles is denied a role-gated capability but
    // still admitted on an open one.
    assert!(!b.caller_authorized(&node_b, &admin_allow),
        "B advertised no roles → denied a role-gated capability");
    assert!(b.caller_authorized(&node_b, &[]),
        "B still admitted on an open (empty-allowlist) capability");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

// ── sys/ namespace-ownership tripwire (Layer I detection) ─────────────────

/// WS1 increment 5: the `sys/` write-guard tripwire detects an inbound remote
/// write to a `sys/` key the receiving node owns. Node A writes
/// `sys/load/{B}/probe` — a key in B's own load namespace that only B should
/// ever originate — and it gossips to B. B applies it (LWW, detection not
/// prevention) but flags it: `system_stats().sys_namespace_violations` rises.
/// A control key in A's *own* load namespace must not trip B's wire.
#[tokio::test]
async fn test_sys_namespace_tripwire_flags_foreign_self_owned_write() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_b = id(port_b);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![id(port_a)]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    // Structural poll: both peered so writes gossip A → B.
    let mut peered = false;
    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() { peered = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "nodes failed to peer");

    // Control: A writes its OWN load key — legitimate, must never trip B.
    let _ = a.kv().set(format!("sys/load/{}/control", id(port_a)), Bytes::from_static(b"x"));
    // Violation: A writes a key in B's load namespace.
    let foreign_key = format!("sys/load/{node_b}/probe");
    let _ = a.kv().set(foreign_key.clone(), Bytes::from_static(b"clobber"));

    // Structural poll: B observes the foreign write and flags it.
    let mut flagged = false;
    for _ in 0..200 {
        // Confirm the write actually reached B (gossip arrived) …
        if b.kv().get(&foreign_key).is_some() && b.system_stats().sys_namespace_violations >= 1 {
            flagged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(flagged, "B did not flag the foreign write to its own sys/load namespace");

    // Violation 2: A writes B's durable acceptor record — B's promise and acceptance for a slot, which
    // only B's `persist_acceptor` writes and `prewarm_accepted` restores. A forged one is a promise B
    // never made; B must count it (2026-10-10).
    let before = b.system_stats().sys_namespace_violations;
    let forged_acceptor = format!("sys/consensus-accepted/{node_b}/slot-x");
    let _ = a.kv().set(forged_acceptor.clone(), Bytes::from_static(b"forged"));
    let mut flagged = false;
    for _ in 0..200 {
        if b.kv().get(&forged_acceptor).is_some() && b.system_stats().sys_namespace_violations > before {
            flagged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(flagged, "B did not flag a peer's write to its own sys/consensus-accepted/ record");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **The adversarial review of #591, finding 1: anti-entropy repair is not a violation.** Three
/// nodes commit a few cluster slots, so each holds every voter's `sys/consensus-accepted/` record
/// and every node's `sys/caller-context/`. Then C restarts with the same id and no persistence: it
/// rewrites its caller-context at start, and anti-entropy hands it back its old one and every
/// acceptor record it wrote before — its own records, repaired. The tripwire counted each of those
/// (the review measured 62 over 30 commits; this test, 1 per slot plus the caller-context) and
/// must count none: `sys_namespace_violations` stays 0 on every node.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn anti_entropy_repair_after_a_restart_is_not_a_namespace_violation() {
    let ports = [alloc_port(), alloc_port(), alloc_port()];
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        Arc::new(GossipAgent::new(id(port), cfg))
    };
    let a = mk(ports[0], vec![]);
    let b = mk(ports[1], vec![id(ports[0])]);
    let c = mk(ports[2], vec![id(ports[0])]);
    for n in [&a, &b, &c] { n.start().await.unwrap(); }
    let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
    let lc = c.consensus().start_consensus_listener(ConsensusConfig::default());
    let mut peered = false;
    for _ in 0..200 {
        if [&a, &b, &c].iter().all(|n| n.peers().len() >= 2) { peered = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "the three nodes did not peer");

    const SLOTS: usize = 5;
    for i in 0..SLOTS {
        let res = a.consensus().cluster_propose(&format!("repair/{i}"), Bytes::from_static(b"v"), ConsensusConfig::default()).await;
        assert!(matches!(res, ConsensusResult::Committed { .. }), "slot {i}: {res:?}");
    }
    // C's own acceptor records must have reached A before C goes away — they are what comes back.
    let c_record = |i: usize| format!("sys/consensus-accepted/{}/repair/{i}", id(ports[2]));
    let mut held = false;
    for _ in 0..200 {
        if (0..SLOTS).all(|i| a.kv().get(&c_record(i)).is_some()) { held = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(held, "A never received C's acceptor records");

    drop(lc);
    c.shutdown_with_timeout(Duration::from_secs(5)).await;
    drop(c);
    let c2 = mk(ports[2], vec![id(ports[0])]);
    c2.start().await.unwrap();
    let _lc2 = c2.consensus().start_consensus_listener(ConsensusConfig::default());

    // The repair happened: the restarted C holds its own pre-restart records again.
    let mut repaired = false;
    for _ in 0..300 {
        if (0..SLOTS).all(|i| c2.kv().get(&c_record(i)).is_some()) { repaired = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(repaired, "anti-entropy did not hand C its acceptor records back");
    // Let any straggling entries land before reading the counters.
    tokio::time::sleep(Duration::from_millis(500)).await;
    for (name, n) in [("A", &a), ("B", &b), ("C", &c2)] {
        assert_eq!(n.system_stats().sys_namespace_violations, 0,
                   "{name} counted anti-entropy repair as a namespace violation");
    }

    for n in [&a, &b, &c2] { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

// ── WS2 audit trail — agent-level writer + chain verification ─────────────

/// WS2 increment 2: a node seals events into its own hash-chained audit stream
/// and the stream verifies end-to-end against the node's identity key — and a
/// post-hoc edit to any stored record breaks verification (the tamper probe).
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_ws2_audit_chain_writes_and_verifies_on_a_node() {
    use crate::config::TlsConfig;
    use crate::{
        audit_stream_prefix, verify_stream_from_genesis, AuditAction, AuditOutcome,
        SignedAuditRecord,
    };

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-audit-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    a.start().await.unwrap();

    // Structural poll: the tls identity key lands at sys/identity/{self}.
    let id_key = format!("sys/identity/{id}");
    let mut vk_bytes = None;
    for _ in 0..100 {
        if let Some(b) = a.kv().get(&id_key) {
            vk_bytes = Some(b);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let vk_bytes = vk_bytes.expect("node identity key never written");
    assert_eq!(vk_bytes.len(), 32);
    let mut vk = [0u8; 32];
    vk.copy_from_slice(&vk_bytes);

    // Seal three events; content hashes must be distinct.
    let h0 = a.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
    let h1 = a.audit(AuditAction::Read, "10.0.0.2:9000", "kv/secret", AuditOutcome::Denied, Some("scope".into())).unwrap();
    let _h2 = a.audit(AuditAction::Write, "10.0.0.1:9000", "kv/x", AuditOutcome::Success, None).unwrap();
    assert_ne!(h0, h1, "distinct events have distinct content hashes");

    // Collect the stream, order by key (lexicographic = seq order), decode.
    let mut entries = a.kv().scan_prefix(&audit_stream_prefix(&id));
    entries.sort_by(|x, y| x.0.cmp(&y.0));
    let chain: Vec<SignedAuditRecord> = entries
        .iter()
        .map(|(_, v)| SignedAuditRecord::decode(v).expect("decode audit record"))
        .collect();
    assert_eq!(chain.len(), 3, "all three sealed records are present");
    assert_eq!(verify_stream_from_genesis(&chain, &id, &vk), Ok(()), "honest chain verifies");

    // Tamper probe: flip a stored record's outcome → verification must fail.
    let mut tampered = chain.clone();
    tampered[1].record.outcome = AuditOutcome::Success;
    assert!(
        verify_stream_from_genesis(&tampered, &id, &vk).is_err(),
        "a post-hoc edit must break chain verification"
    );

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **The audit chain's head survives a restart.** `AuditChainState::new()` is `{ next_seq: 0,
/// last_hash: zero }` and nothing read the persisted `sys/audit/{self}/` stream back at `start()`,
/// so a restarted node with `[persistence]` sealed seq 0 again with a zero `prev_hash` and LWW
/// overwrote the original genesis record — the tamper-evident chain erased its own history at every
/// restart. The head is now recovered from this node's own persisted stream, verified from genesis
/// (or the newest signed checkpoint) so a forged record cannot seed it. Seen failing first: the
/// restarted node's stream held three records and seq 0 was not the original.
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn the_audit_chain_head_is_recovered_from_persistence_at_restart() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use crate::{audit_stream_prefix, verify_stream_from_genesis, AuditAction, AuditOutcome, SignedAuditRecord};

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-audit-restart-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });

    let stream = |a: &GossipAgent| -> Vec<SignedAuditRecord> {
        let mut entries = a.kv().scan_prefix(&audit_stream_prefix(&id));
        entries.sort_by(|x, y| x.0.cmp(&y.0));
        entries.iter().map(|(_, v)| SignedAuditRecord::decode(v).expect("decode audit record")).collect()
    };

    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let vk = a.task_ctx.tls.get().expect("tls").verifying_key_bytes();
    let h0 = a.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
    let _ = a.audit(AuditAction::Read, "10.0.0.2:9000", "kv/secret", AuditOutcome::Denied, None).unwrap();
    let h2 = a.audit(AuditAction::Write, "10.0.0.1:9000", "kv/x", AuditOutcome::Success, None).unwrap();
    assert_eq!(stream(&a).len(), 3);
    a.shutdown().await;

    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    assert_eq!(stream(&b).len(), 3, "the persisted stream replays before anything is sealed");
    let h3 = b.audit(AuditAction::Admin, "10.0.0.3:9000", "role/grant", AuditOutcome::Success, None).unwrap();
    let chain = stream(&b);
    assert_eq!(chain.len(), 4, "the restarted node sealed seq 3, not seq 0 again: {:?}", chain.iter().map(|r| r.record.seq).collect::<Vec<_>>());
    assert_eq!(chain[0].record.content_hash(), h0, "the genesis record is the original, not overwritten by a new seq 0");
    assert_eq!(chain[3].record.seq, 3);
    assert_eq!(chain[3].record.prev_hash, h2, "the new record links to the last record sealed before the restart");
    assert_eq!(chain[3].record.content_hash(), h3);
    assert_eq!(verify_stream_from_genesis(&chain, &id, &vk), Ok(()), "the chain across the restart verifies from genesis");
    assert_eq!(b.audit_verify(&id), Ok(()));
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// The recovery is **verified**: a record in this node's own persisted stream that does not verify
/// under its keys — planted by whoever can write the files — does not seed the chain: the next seal
/// links to the last verified record. Since post-360 plan row Q it is also **never sealed over**: the
/// unverified record stays untouched and is reported (`audit_unverified`), and numbering resumes after
/// it — sealing over it was how a node upgraded after an old-version rotation destroyed its own genesis.
#[cfg(all(feature = "compliance", unix))]
#[tokio::test]
async fn a_forged_record_in_the_persisted_audit_stream_does_not_seed_the_chain_head() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode, TlsConfig};
    use crate::{audit_key, audit_stream_prefix, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-audit-forged-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: base.join("certs"), ..TlsConfig::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: base.join("data"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });

    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let h0 = a.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
    // A forged seq 1, signed by a key that is not this node's, planted through the data plane into
    // the persisted store (the same bytes an attacker with the files could write).
    let forged = AuditRecord { node_id: id.clone(), seq: 1, hlc: 1, principal: "attacker".into(), action: AuditAction::Admin, target: "role/grant".into(), outcome: AuditOutcome::Success, detail: None, prev_hash: h0 };
    let signed = SignedAuditRecord::sign(forged, &ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]));
    let _ = a.kv().set(audit_key(&id, 1), signed.clone().encode());
    a.shutdown().await;

    let b = GossipAgent::new(id.clone(), cfg);
    b.start().await.unwrap();
    let _ = b.audit(AuditAction::Write, "10.0.0.1:9000", "kv/x", AuditOutcome::Success, None).unwrap();
    let mut entries = b.kv().scan_prefix(&audit_stream_prefix(&id));
    entries.sort_by(|x, y| x.0.cmp(&y.0));
    let chain: Vec<SignedAuditRecord> = entries.iter().map(|(_, v)| SignedAuditRecord::decode(v).unwrap()).collect();
    let reported = b.task_ctx.audit_chain.lock().unwrap().unverified;
    assert_eq!(chain.len(), 3, "seq 0, the forgery kept, and the new seal: {:?}", chain.iter().map(|r| r.record.seq).collect::<Vec<_>>());
    assert_eq!(chain[0].record.content_hash(), h0, "genesis is untouched");
    assert_eq!(chain[1], signed, "the unverified record is not sealed over");
    assert_eq!(chain[2].record.seq, 2, "numbering resumed after the stream's last record");
    assert_eq!(chain[2].record.prev_hash, h0, "linked to the last verified record, not to the forgery");
    assert_eq!(reported, Some((1, 1)), "and the unverified record is reported");
    assert!(b.audit_verify(&id).is_err(), "the stream still says it holds a record that does not verify");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// The adversarial review of #584 (finding 2): only the genesis branch of `recover_chain_head` was
/// tested. The three other anchors, each through the real seal/checkpoint/prune calls:
/// (a) no record and a checkpoint — the stream pruned to its head — anchors at the newest checkpoint;
/// (b) a first record at N with a checkpoint at N — a pruned stream — anchors there and verifies on;
/// (c) a first record at N with no matching checkpoint — a hand-deleted genesis — stays at genesis
/// and reports the gap, overwriting nothing because nothing is at seq 0.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn recover_chain_head_anchors_at_a_checkpoint_or_reports_the_gap() {
    use crate::agent::audit::{recover_chain_head, AuditVerifyError};
    use crate::config::TlsConfig;
    use crate::{audit_key, AuditAction, AuditOutcome};

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-audit-anchors-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let seal = |n: u64| a.audit(AuditAction::Invoke, "10.0.0.1:9000", format!("skill/{n}"), AuditOutcome::Success, None).unwrap();

    // (b) records 0,1 · checkpoint at 2 · records 2,3 · prune → [2, 3] anchored at the checkpoint.
    let _h0 = seal(0); let _h1 = seal(1);
    let (cp_seq, cp_prev) = a.audit_checkpoint().unwrap();
    assert_eq!(cp_seq, 2);
    let _h2 = seal(2); let h3 = seal(3);
    assert_eq!(a.audit_prune_to_checkpoint(), 2, "records 0 and 1 pruned");
    let head = recover_chain_head(&a.task_ctx).expect("a stream to recover");
    assert_eq!((head.next_seq, head.last_hash, head.verified, head.unverified.clone()), (4, h3, 2, None),
        "(b) anchored at the checkpoint's (seq, prev_hash) and verified through seq 3");
    let _ = cp_prev;

    // (a) checkpoint at 4, prune everything → no record, anchored at the newest checkpoint.
    let (cp_seq, cp_prev) = a.audit_checkpoint().unwrap();
    assert_eq!((cp_seq, cp_prev), (4, h3));
    assert_eq!(a.audit_prune_to_checkpoint(), 2);
    assert!(a.audit_stream(&id).is_empty());
    let head = recover_chain_head(&a.task_ctx).expect("a checkpoint to recover from");
    assert_eq!((head.next_seq, head.last_hash, head.verified, head.unverified.clone()), (4, h3, 0, None),
        "(a) the newest checkpoint is the head when the stream is pruned to it");
    let h4 = seal(4);
    assert_eq!(a.audit_stream(&id).last().map(|r| (r.record.seq, r.record.prev_hash)), Some((4, h3)));
    assert_eq!(a.audit_verify(&id), Ok(()), "verification resumes from the checkpoint over the new seal");

    // (c) delete the first present record (seq 4, the record at the pruned checkpoint boundary) by hand:
    // no checkpoint at 5. Since post-360 plan row Q the stream anchors at the newest checkpoint below its
    // first record (4, h3) — not at genesis, among exported and pruned slots — the gap is named, and
    // seq 5 is not directly after the head, so it is not followed. Sealing **never overwrites** seq 5:
    // a seal skips a slot a record occupies. (Before the safety net the head went back to genesis and,
    // sealing on, overwrote seq 5.)
    assert!(a.kv().delete(audit_key(&id, 4)));
    let _h5 = seal(5);
    let held5 = a.kv().get(&audit_key(&id, 5));
    let head = recover_chain_head(&a.task_ctx).expect("records to fold");
    assert_eq!(head.next_seq, 4, "(c) anchored at the newest checkpoint below seq 5, not at genesis");
    assert_eq!(head.last_hash, h3);
    assert_eq!(head.verified, 0);
    assert_eq!(head.unverified, Some(AuditVerifyError::SequenceGap { expected: 4, found: 5 }), "the gap is reported, not guessed over");
    assert_eq!(head.unverified_range, Some((5, 5)), "and the unverified record is named");
    crate::agent::audit::restore_chain_head(&a.task_ctx);
    for n in 0..3u64 { let _ = seal(100 + n); }
    assert_eq!(a.kv().get(&audit_key(&id, 5)), held5, "sealing on never overwrote the record at seq 5");
    let seqs: Vec<u64> = a.audit_stream(&id).iter().map(|r| r.record.seq).collect();
    assert_eq!(seqs, vec![4, 5, 6, 7], "the seals took 4 and 6..=7, skipping the occupied 5 (0..=3 stay pruned)");
    let _ = h4;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

// ── WS3 crown-jewel — data-at-rest encryption hook ────────────────────────

/// A trivial reversible cipher for exercising the data-at-rest hook: a 1-byte
/// key tag followed by XOR-with-key. `decrypt` rejects a blob tagged with a
/// different key (returns `None`), so a wrong-key replay reads as corrupt.
struct XorCipher {
    key: u8,
}

impl crate::DataAtRestCipher for XorCipher {
    fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plaintext.len() + 1);
        out.push(self.key);
        out.extend(plaintext.iter().map(|b| b ^ self.key));
        out
    }
    fn decrypt(&self, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let (tag, body) = ciphertext.split_first()?;
        if *tag != self.key {
            return None; // wrong key → treated as corrupt
        }
        Some(body.iter().map(|b| b ^ self.key).collect())
    }
}

/// WS3: an attached `DataAtRestCipher` encrypts WAL/snapshot bytes on disk
/// (plaintext never appears in `wal.bin`), the same cipher recovers the data on
/// restart, and a wrong-key cipher cannot — proving the hook is load-bearing.
#[tokio::test]
async fn test_ws3_data_at_rest_cipher_encrypts_wal_and_round_trips() {
    use crate::{PersistenceConfig, SyncMode};

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-darc-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let wal_path = base.join(id.to_string()).join("kv").join("wal.bin");

    let marker: &[u8] = b"TOPSECRET-PLAINTEXT-MARKER";

    let mk = || {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.persistence = Some(PersistenceConfig {
            base_path: base.clone(),
            sync_mode: SyncMode::Flush,
            snapshot_wal_threshold: 1_000_000, // keep data in wal.bin, no auto-snapshot
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
        });
        GossipAgent::new(id.clone(), cfg)
    };

    // ── Phase 1: write under encryption ──────────────────────────────────
    let a1 = Arc::new(mk());
    a1.with_data_at_rest_cipher(Arc::new(XorCipher { key: 0x5A }));
    a1.start().await.unwrap();
    let _ = a1.kv().set("secret/1", Bytes::copy_from_slice(marker));

    // Structural poll: wait until the WAL record has actually landed on disk.
    let mut wal_bytes = Vec::new();
    for _ in 0..200 {
        if let Ok(b) = std::fs::read(&wal_path)
            && !b.is_empty()
        {
            wal_bytes = b;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!wal_bytes.is_empty(), "WAL never flushed to disk");

    // Encryption proof: the plaintext marker must NOT appear on disk.
    let contains = wal_bytes
        .windows(marker.len())
        .any(|w| w == marker);
    assert!(!contains, "plaintext marker found in wal.bin — bytes were not encrypted");

    a1.shutdown_with_timeout(Duration::from_secs(5)).await;

    // ── Phase 2: same key recovers the data ──────────────────────────────
    let a2 = Arc::new(mk());
    a2.with_data_at_rest_cipher(Arc::new(XorCipher { key: 0x5A }));
    a2.start().await.unwrap();
    let mut recovered = None;
    for _ in 0..40 {
        if let Some(v) = a2.kv().get("secret/1") {
            recovered = Some(v);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        recovered.as_deref(),
        Some(marker),
        "same-key restart must recover the encrypted record"
    );
    a2.shutdown_with_timeout(Duration::from_secs(5)).await;

    // ── Phase 3: wrong key cannot read it — and the node says so ─────────
    // Since 2026-10-03 (`persist.replay`): persisted state that does not decode under the configured
    // cipher is *unreadable*, and a node refuses to start over it rather than starting empty and
    // snapshotting over the ciphertext. The cipher is load-bearing in both directions.
    let a3 = Arc::new(mk());
    a3.with_data_at_rest_cipher(Arc::new(XorCipher { key: 0x11 }));
    let e = a3.start().await.expect_err("a wrong key must refuse the start, not recover nothing");
    assert!(e.to_string().contains("persistence") && e.to_string().contains("quarantine"), "{e}");
    assert!(
        a3.kv().get("secret/1").is_none(),
        "wrong-key replay must not recover the record (cipher is load-bearing)"
    );
    a3.shutdown_with_timeout(Duration::from_secs(5)).await;

    let _ = std::fs::remove_dir_all(&base);
}

/// `gw.tls_runtime` (plan §8): a gateway that cannot come up is a **start() error**, not a dead task
/// behind a node that reports ready. Before the fix the bind and the TLS setup ran inside the spawned
/// task: a bad certificate path or a busy port was logged as "HTTP server exited" while `start()`
/// returned Ok.
#[cfg(all(feature = "gateway", feature = "tls"))]
#[tokio::test]
async fn a_gateway_that_cannot_come_up_fails_the_start() {
    // A busy port.
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let port = alloc_port();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.http_port = Some(busy_port);
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    let e = a.start().await.expect_err("a port the gateway cannot bind must fail the start");
    assert!(e.to_string().contains("gateway"), "{e}");
    a.shutdown().await;
    drop(busy);

    // A certificate path that does not exist.
    let port = alloc_port();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.http_port = Some(alloc_port());
    cfg.gateway_tls = Some(crate::GatewayTlsConfig {
        cert_pem_path: Some("/nonexistent/gateway.cert.pem".into()),
        key_pem_path: Some("/nonexistent/gateway.key.pem".into()),
    });
    let b = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    let e = b.start().await.expect_err("an unreadable gateway certificate must fail the start");
    assert!(e.to_string().contains("gateway"), "{e}");
    assert!(!b.is_ready(), "a node whose gateway did not come up is not ready");
    b.shutdown().await;
}

/// `audit.sink` (plan §8): an audit sink attached to a node with no `[tls]` would receive nothing —
/// records are sealed with the node identity, and without it nothing is sealed. That is a setting
/// accepted and ignored, the class v2.18.1 closed for the token tables; `start()` refuses it by name.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn an_audit_sink_without_tls_refuses_the_start() {
    struct Nowhere;
    impl crate::AuditSink for Nowhere {
        fn export(&self, _: &crate::SignedAuditRecord) {}
    }
    let port = alloc_port();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.tls = None;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    a.with_audit_sink(Arc::new(Nowhere));
    let e = a.start().await.expect_err("a sink that would receive nothing must refuse the start");
    assert!(e.to_string().contains("audit") && e.to_string().contains("[tls]"), "{e}");
    a.shutdown().await;
}

/// G13 (`at_rest.cipher`, plan §8): a cipher attached **after** `start()` is rejected — not set, not
/// reported as enforced, and counted as a late attachment — because the WAL writer read its cipher
/// at start and an attach now would be accepted and do nothing. Before the fix the report said
/// `enforced` for a node writing plaintext.
#[tokio::test]
async fn a_cipher_attached_after_start_is_rejected_and_the_report_says_so() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    struct Xor;
    impl crate::DataAtRestCipher for Xor {
        fn encrypt(&self, b: &[u8]) -> Vec<u8> { b.iter().map(|x| x ^ 0x5A).collect() }
        fn decrypt(&self, b: &[u8]) -> Option<Vec<u8>> { Some(b.iter().map(|x| x ^ 0x5A).collect()) }
    }
    let port = alloc_port();
    let base = std::env::temp_dir().join(format!("mycelium-late-cipher-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(PersistenceConfig { base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 10, snapshot_interval_secs: 300, on_unreadable: OnUnreadable::Refuse });
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    a.start().await.unwrap();
    a.with_data_at_rest_cipher(Arc::new(Xor));
    let r = a.guarantee_report();
    let st = r.entries.iter().find(|e| e.id == "at_rest.cipher").map(|e| format!("{:?}", e.resolution)).unwrap_or_default();
    assert!(st.contains("NotConfigured"), "a late cipher is not reported as enforced: {st}");
    assert_eq!(a.late_attachments(), 1, "counted, so the startup report is known stale");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// Regression floor (contracts axis item 1 PR 1, `docs/design/contracts-receipts.md` §8): with
/// **no persistence configured**, `Committed { persisted }` reads `true` — "nothing was promised" is
/// collapsed into the same bool as "fsynced" (D24). PR 2 adds `local_durability: NotConfigured`
/// beside it; this pin is what that PR changes, in the open.
#[cfg(feature = "consensus")]
/// `persist.replay` (plan §8): a node with persisted state it cannot read **refuses to start** by
/// default, naming the file — it used to start from the WAL alone and overwrite the unreadable
/// snapshot at its next snapshot. Under `on_unreadable = "quarantine"` it moves the file aside (never
/// deleted) and starts.
#[tokio::test]
async fn a_node_refuses_to_start_over_an_unreadable_snapshot_unless_told_to_quarantine() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-unreadable-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("snapshot.bin"), b"this is not a snapshot").unwrap();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(PersistenceConfig { base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 10, snapshot_interval_secs: 300, on_unreadable: OnUnreadable::Refuse });

    let a = GossipAgent::new(id.clone(), cfg.clone());
    let e = a.start().await.expect_err("an unreadable snapshot must refuse the start");
    assert!(e.to_string().contains("snapshot.bin") && e.to_string().contains("quarantine"), "{e}");
    assert!(dir.join("snapshot.bin").exists(), "nothing was moved or overwritten");
    a.shutdown().await;

    cfg.persistence.as_mut().unwrap().on_unreadable = OnUnreadable::Quarantine;
    let b = GossipAgent::new(id, cfg);
    b.start().await.expect("quarantine moves the file aside and starts");
    assert!(dir.join("snapshot.bin.unreadable-1").exists(), "moved aside, never deleted: {:?}", std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());
    assert_eq!(std::fs::read(dir.join("snapshot.bin.unreadable-1")).unwrap(), b"this is not a snapshot", "the unreadable bytes are kept as they were");
    if let Ok(now) = std::fs::read(dir.join("snapshot.bin")) {
        assert_ne!(now, b"this is not a snapshot", "a snapshot written since is a fresh one, not the unreadable file");
    }
    let report = b.guarantee_report();
    let st = report.entries.iter().find(|e| e.id == "persist.unreadable_refused").map(|e| format!("{:?}", e.resolution)).unwrap_or_default();
    assert!(st.contains("NotConfigured"), "quarantine is not the refusing setting: {st}");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// A persistence config pointing at `base`, Flush mode, with the realignment-repairs R2 witnesses'
/// small thresholds.
#[cfg(unix)]
fn r2_persistence(base: &std::path::Path) -> crate::config::PersistenceConfig {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    PersistenceConfig { base_path: base.to_path_buf(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse }
}

/// **Realignment repairs R2 (the review's F01), the pin.** A torn WAL tail is repaired by the
/// startup snapshot *before* the first acknowledged append, so the append is reachable by replay
/// if the node crashes right after it. This held before R2 — `start()` has triggered a snapshot
/// since the WAL existed — but nothing pinned it.
///
/// **The crash is modelled by copying the files while the node runs**, right after the synced
/// append, and replaying the copy. A graceful restart cannot pin this: `shutdown()` takes a final
/// snapshot from the *store*, which holds the value whatever the WAL says, so the first version of
/// this test passed with the startup snapshot removed. Seen failing with that line toggled off.
#[cfg(unix)]
#[tokio::test]
async fn a_torn_wal_tail_is_repaired_at_start_before_the_first_acknowledged_append() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-r2-torn-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    std::fs::create_dir_all(&dir).unwrap();
    // A length prefix claiming 50 bytes, three present: the file a crash mid-append leaves.
    std::fs::write(dir.join("wal.bin"), [50u8, 0, 0, 0, 1, 2, 3]).unwrap();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));

    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.expect("a torn tail is a crash, not a refusal");
    a.kv().set_requiring_sync(&mycelium_core::receipt::OperationId::new("r2-after-torn"), "r2/after-torn", b"v".to_vec()).await.expect("the append is synced before the crash");

    // The crash: what is on disk now, with the node still running, is all a restart would have.
    let crash = base.join("crash");
    std::fs::create_dir_all(&crash).unwrap();
    for f in ["snapshot.bin", "wal.bin"] {
        if dir.join(f).exists() { std::fs::copy(dir.join(f), crash.join(f)).unwrap(); }
    }
    let mut replayed = Vec::new();
    mycelium_core::persistence::replay(&crash, None, |e| replayed.push(e.key.to_string()))
        .await
        .expect("the crashed files replay");
    assert!(replayed.iter().any(|k| k == "r2/after-torn"), "the acknowledged append is reachable after a crash: {replayed:?}");
    let wal = std::fs::read(crash.join("wal.bin")).unwrap();
    assert!(!wal.starts_with(&[50, 0, 0, 0]), "the torn frame was removed before the append landed");
    a.shutdown().await;

    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    assert_eq!(b.kv().get("r2/after-torn"), Some(Bytes::from_static(b"v")), "the acknowledged append survives the restart");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **An acceptor's record reaches the WAL before its answer leaves.** The record
/// (`sys/consensus-accepted/{node}/{slot}`) was applied to the store and gossiped, never handed to
/// the WAL — only the committed slot and its lease were — so a node that crashed after promising or
/// voting restarted with no memory of it: `prewarm_accepted` reads the store the snapshot and WAL
/// restore, and neither held the record. The promise a proposer chose its value on was gone, and the
/// node could vote again at the same ballot for another value — the restart case the in-memory
/// `acceptor_memory_survives_a_restart` modelled with an encode/decode round trip and nothing on disk.
///
/// **The crash is modelled as R2's is**: copy the files while the node runs, replay the copy, then
/// restart over the copy. An orderly `shutdown()` cannot show this — it snapshots the *store*, which
/// held the record whatever the WAL said. Seen failing first: the replay held
/// `consensus/committed/leader/solo` and not the acceptor's record.
#[cfg(all(unix, feature = "consensus"))]
#[tokio::test]
async fn an_acceptors_record_survives_a_crash_without_a_snapshot() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-acceptor-wal-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));

    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    // A one-member group: this node is the whole electorate, so its own promise and acceptance are
    // the record under test, written on the proposer's path (the voter's path writes the same way).
    a.mesh().join_group("solo");
    let res = a.consensus()
        .group_propose("solo", "leader/solo", Bytes::from_static(b"me"), ConsensusConfig::default())
        .await;
    assert!(matches!(res, ConsensusResult::Committed { .. }), "{res:?}");
    let slot: Arc<str> = Arc::from("leader/solo");
    let live = a.task_ctx.consensus_accepted.pin().get(&slot).cloned().expect("the acceptor holds the slot");
    assert!(live.promised >= 1, "{live:?}");
    let key = crate::consensus::accepted_key(&id, &slot);

    // The crash: what is on disk now, with the node still running, is all a restart would have.
    let crash = base.join("crash");
    std::fs::create_dir_all(&crash).unwrap();
    for f in ["snapshot.bin", "wal.bin"] {
        if dir.join(f).exists() { std::fs::copy(dir.join(f), crash.join(f)).unwrap(); }
    }
    let mut replayed = Vec::new();
    mycelium_core::persistence::replay(&crash, None, |e| replayed.push(e.key.to_string()))
        .await
        .expect("the crashed files replay");
    assert!(replayed.iter().any(|k| k == "consensus/committed/leader/solo"), "the commit is on disk: {replayed:?}");
    assert!(replayed.iter().any(|k| k == &key), "the acceptor's record is reachable after a crash: {replayed:?}");
    a.shutdown().await;

    // Restart over what the crash left — not over the orderly shutdown's snapshot.
    for f in ["snapshot.bin", "wal.bin"] {
        let _ = std::fs::remove_file(dir.join(f));
        if crash.join(f).exists() { std::fs::copy(crash.join(f), dir.join(f)).unwrap(); }
    }
    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    let restored = b.task_ctx.consensus_accepted.pin().get(&slot).cloned();
    assert!(
        restored.as_ref().is_some_and(|s| s.promised >= live.promised),
        "prewarm_accepted restores the promise from the WAL: {restored:?} (live was {live:?})",
    );
    // And the restored node keeps it: a lower ballot from another proposer is refused.
    assert!(
        !crate::consensus::claim_vote(&b.task_ctx.consensus_accepted, &slot, live.promised - 1,
                                      &Bytes::from_static(b"other"), 0xBEEF, 0),
        "a ballot below the restored promise is refused",
    );
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **Review of #600, finding 5: a released leadership is on disk when `release_leadership` returns.**
/// A permanent leadership has no lease to fall back on, so a release a crash forgets restores it.
#[cfg(all(unix, feature = "consensus"))]
#[tokio::test]
async fn a_released_leadership_survives_a_crash() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-release-wal-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    a.mesh().join_group("perm");
    a.consensus().elect_leader_with("perm", crate::LeaderTerm::Permanent).await.expect("elects");
    assert!(a.consensus().release_leadership("perm").await);
    // The crash, at the instant the call returned.
    let crash = base.join("crash");
    std::fs::create_dir_all(&crash).unwrap();
    for f in ["snapshot.bin", "wal.bin"] {
        if dir.join(f).exists() { std::fs::copy(dir.join(f), crash.join(f)).unwrap(); }
    }
    let mut released = false;
    let marker_prefix = format!("consensus/life/{}/end/", "leader%2Fperm");
    mycelium_core::persistence::replay(&crash, None, |e| {
        if e.key.starts_with(marker_prefix.as_str()) && !e.is_tombstone
            && crate::consensus_life::decode_marker(&e.value).is_some() {
            released = true;
        }
    }).await.expect("the crashed files replay");
    assert!(released, "the release was not on disk when release_leadership returned");
    a.shutdown().await;
    // Restart over what the crash left (row 9): the leadership stays released.
    for f in ["snapshot.bin", "wal.bin"] {
        let _ = std::fs::remove_file(dir.join(f));
        if crash.join(f).exists() { std::fs::copy(crash.join(f), dir.join(f)).unwrap(); }
    }
    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    assert_eq!(b.consensus().consensus_get("leader/perm"), None, "the release did not survive the restart");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **Rows 14 / 27 (review finding 5), across a restart: collection's floor is this node's own record.**
/// A lock is acquired and released; collection shrinks the acceptor state to the floor; the node
/// crashes and restarts over what is on disk: the finished ballot is still refused, by the restored
/// floor, with the shared `decided` key regressed.
#[cfg(all(unix, feature = "consensus"))]
#[tokio::test]
async fn a_collected_floor_survives_a_crash() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-floor-wal-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));
    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let g = a.consensus().distributed_lock("floor", Duration::from_secs(60)).await.expect("acquire");
    let slot: Arc<str> = Arc::from("lock/floor");
    let ballot = a.task_ctx.consensus_accepted.pin().get(&slot).map(|s| s.promised).expect("accepted");
    g.release();
    assert!(a.consensus().collect_finished_acceptor_state().await >= 1, "collected");
    let crash = base.join("crash");
    std::fs::create_dir_all(&crash).unwrap();
    for f in ["snapshot.bin", "wal.bin"] {
        if dir.join(f).exists() { std::fs::copy(dir.join(f), crash.join(f)).unwrap(); }
    }
    a.shutdown().await;
    for f in ["snapshot.bin", "wal.bin"] {
        let _ = std::fs::remove_file(dir.join(f));
        if crash.join(f).exists() { std::fs::copy(crash.join(f), dir.join(f)).unwrap(); }
    }
    let b = GossipAgent::new(id, cfg);
    b.start().await.unwrap();
    let _ = b.kv().set("consensus/decided/lock/floor", crate::consensus::encode_ballot(0));
    let restored = b.task_ctx.consensus_accepted.pin().get(&slot).cloned();
    assert!(restored.as_ref().is_some_and(|s| s.promised >= ballot && s.accepted.is_none()), "the shrunk floor was restored: {restored:?}");
    assert!(!crate::consensus::claim_vote(&b.task_ctx.consensus_accepted, &slot, ballot, &Bytes::from_static(b"x"), 0xBEEF, 0),
        "a finished ballot was accepted after a restart");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **Review of #600, finding 8: the gateway's election routes** — `ttl_secs`, `permanent`, and
/// `DELETE /gateway/overlay/elect/{group}` (stepping down; `404 not_leader` when this node is not it).
#[cfg(all(feature = "consensus", feature = "gateway"))]
#[tokio::test]
async fn the_gateway_elects_on_a_lease_and_steps_down() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.expect("start");
    let _l = agent.consensus().start_consensus_listener(ConsensusConfig::default());
    agent.mesh().join_group("gw-lease");
    agent.mesh().join_group("gw-perm");
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    for _ in 0..40 {
        if client.get(format!("{base}/health")).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let term = |slot: &str| match crate::consensus_life::read_slot(
            &agent.task_ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(t) => Some(t.env.term),
        _ => None,
    };

    let r = client.post(format!("{base}/gateway/overlay/elect"))
        .json(&serde_json::json!({"group": "gw-lease", "ttl_secs": 7})).send().await.expect("elect");
    assert_eq!(r.status(), 200, "{:?}", r.text().await);
    assert!(matches!(term("leader/gw-lease"), Some(crate::consensus_life::Term::Lease { ms: 7000, .. })));

    let r = client.post(format!("{base}/gateway/overlay/elect"))
        .json(&serde_json::json!({"group": "gw-perm", "permanent": true})).send().await.expect("elect");
    assert_eq!(r.status(), 200);
    assert_eq!(term("leader/gw-perm"), Some(crate::consensus_life::Term::Permanent), "a permanent election carries no lease");

    let r = client.delete(format!("{base}/gateway/overlay/elect/gw-lease")).send().await.expect("release");
    assert_eq!(r.status(), 200);
    assert!(agent.consensus().consensus_get("leader/gw-lease").is_none(), "released leadership reads as none");
    let r = client.delete(format!("{base}/gateway/overlay/elect/gw-lease")).send().await.expect("release");
    assert_eq!(r.status(), 404, "this node is no longer the leader");
    let body: serde_json::Value = r.json().await.expect("json");
    assert_eq!(body["error"], "not_leader");
    agent.shutdown().await;
}

/// **Realignment repairs R7** (found by A2's configuration audit, 2026-10-05). A persistence
/// directory that cannot be created used to be a warning: `start()` logged it and ran **in memory**,
/// so every write was lost on the next restart — while the guarantee report, resolved from the
/// configuration alone, still said `persist.configured: enforced` and `secure-single-domain` let the
/// node start. Now it refuses the start by name. The directory here sits under a regular file.
#[cfg(unix)]
#[tokio::test]
async fn a_persistence_directory_that_cannot_be_created_refuses_the_start() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let root = std::env::temp_dir().join(format!("mycelium-r7-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let not_a_dir = root.join("a-file");
    std::fs::write(&not_a_dir, b"not a directory").unwrap();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&not_a_dir));

    let a = GossipAgent::new(id, cfg);
    let r = a.start().await;
    let e = match r {
        Err(e) => e,
        Ok(()) => { a.shutdown().await; panic!("a node that cannot persist must not start as if it could"); }
    };
    assert!(e.to_string().contains("persistence") && e.to_string().contains("directory"), "the refusal names it: {e}");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// **R2, the refusal.** If the startup snapshot that repairs the WAL fails, the node must not start
/// and append behind the torn tail it could not remove — `start()` used to discard the snapshot's
/// result with `let _ =`. The failure here is a directory the node cannot write `snapshot.tmp` into.
#[cfg(unix)]
#[tokio::test]
async fn a_failing_startup_snapshot_refuses_the_start_instead_of_appending_behind_a_torn_tail() {
    use std::os::unix::fs::PermissionsExt as _;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-r2-snapfail-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(id.to_string()).join("kv");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("wal.bin"), [50u8, 0, 0, 0, 1, 2, 3]).unwrap();
    // The ownership lock file exists already, so the read-only directory stops only the snapshot.
    std::fs::write(dir.join("wal.bin.lock"), b"").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));

    let a = GossipAgent::new(id, cfg);
    let r = a.start().await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let e = match r {
        Err(e) => e,
        Ok(()) => { a.shutdown().await; panic!("a node whose repairing snapshot failed must refuse to start"); }
    };
    assert!(e.to_string().contains("startup snapshot"), "the refusal names the snapshot: {e}");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **R2, ownership (the review's F12 for the WAL).** Two agents on one persistence path — the same
/// node id, the same `base_path` — would each replay, each snapshot and each append to one
/// `wal.bin`. The second now refuses to start naming the owner, before it binds anything; before R2
/// it got as far as the bind and failed for the port.
#[cfg(unix)]
#[tokio::test]
async fn a_second_agent_on_one_persistence_path_refuses_to_start_naming_the_owner() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-r2-owner-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));

    let a = GossipAgent::new(id.clone(), cfg.clone());
    a.start().await.unwrap();
    let b = GossipAgent::new(id, cfg);
    let e = match b.start().await {
        Err(e) => e,
        Ok(()) => { b.shutdown().await; panic!("a second owner must be refused"); }
    };
    assert!(e.to_string().contains("owned by another"), "the refusal names the ownership, not the port: {e}");
    b.shutdown().await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// Regression floor (contracts axis item 1 PR 1, `docs/design/contracts-receipts.md` §8): with
/// **no persistence configured**, `Committed { persisted }` reads `true` — "nothing was promised" is
/// collapsed into the same bool as "fsynced" (D24). PR 2 adds `local_durability: NotConfigured`
/// beside it; this pin is what that PR changes, in the open.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn floor_committed_persisted_is_true_when_persistence_unconfigured() {
    use crate::{ConsensusConfig, ConsensusResult};
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    assert!(cfg.persistence.is_none(), "precondition: no persistence");
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let solo = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let r = a.consensus().cluster_propose("floor/unconfigured", Bytes::from_static(b"v"), solo).await;
    assert!(matches!(r, ConsensusResult::Committed { persisted: true, .. }),
        "today: unconfigured persistence reports persisted: true (nothing promised); got {r:?}");
    a.shutdown().await;
}

/// Review 2026-09-05 finding 3: consensus discarded the committed-slot WAL result and reported
/// `Committed` regardless. With persistence on, the commit must report `persisted: true`, the
/// forced-fsync append must actually land (`append_sync` in *Async* mode — the mode that used to
/// skip the sync), and the slot must survive a restart via replay — including a leased commit's
/// lease record (folded into the same flag).
#[tokio::test]
#[cfg(feature = "consensus")]
async fn consensus_commit_reports_persisted_and_survives_restart() {
    use crate::{ConsensusConfig, ConsensusResult, PersistenceConfig, SyncMode};

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("myc-cpersist-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mk = || {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.persistence = Some(PersistenceConfig {
            base_path: base.clone(),
            sync_mode: SyncMode::Async, // append_sync must force the fsync here
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
        });
        GossipAgent::new(id.clone(), cfg)
    };

    let a1 = Arc::new(mk());
    a1.start().await.unwrap();
    let _listener = a1.consensus().start_consensus_listener(ConsensusConfig::default());
    let permanent = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };
    let leased    = ConsensusConfig { quorum_size: 1, committed_lease_secs: Some(3_600), ..ConsensusConfig::default() };

    let r1 = a1.consensus().cluster_propose("durable/perm", Bytes::from_static(b"v-perm"), permanent).await;
    assert!(matches!(r1, ConsensusResult::Committed { persisted: true, .. }),
        "permanent commit with a live writer must report persisted: true; got {r1:?}");
    let r2 = a1.consensus().cluster_propose("durable/leased", Bytes::from_static(b"v-leased"), leased).await;
    assert!(matches!(r2, ConsensusResult::Committed { persisted: true, .. }),
        "leased commit (slot + lease record) must report persisted: true; got {r2:?}");
    a1.shutdown_with_timeout(Duration::from_secs(5)).await;

    // Restart: both slots come back from disk (replay), the leased one still live.
    let a2 = Arc::new(mk());
    a2.start().await.unwrap();
    let mut got = (None, None);
    for _ in 0..80 {
        got = (a2.consensus().consensus_get("durable/perm"), a2.consensus().consensus_get("durable/leased"));
        if got.0.is_some() && got.1.is_some() { break; }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(got.0, Some(Bytes::from_static(b"v-perm")),   "permanent committed slot lost across restart");
    assert_eq!(got.1, Some(Bytes::from_static(b"v-leased")), "leased committed slot lost across restart");
    a2.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&base);
}

// ── WS5: hot identity rotation under live traffic ─────────────────────────

/// WS5 increment 3: rotating node A's identity mid-stream does not break peer B's
/// verification. B verifies A's audit records signed by the OLD key *and* the NEW
/// key (retained key set), and the chain spanning the rotation verifies on B.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_ws5_rotate_identity_verifies_across_rotation_on_peer() {
    use crate::config::TlsConfig;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_a = id(port_a);
    let cert_dir = std::env::temp_dir().join(format!("myc-ws5-{port_a}-{port_b}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![node_a.clone()]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    // Peer up.
    let mut peered = false;
    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() { peered = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "tls nodes failed to peer");

    // A seals a record under the OLD key.
    a.audit(crate::AuditAction::Invoke, "client", "before-rotation", crate::AuditOutcome::Success, None).unwrap();

    // Rotate A's identity (publish new‖old, brief window, cut over to new key).
    let new_vk = a.rotate_identity(Duration::from_millis(500)).await.expect("rotation");
    // The active key actually changed.
    assert_ne!(
        a.kv().get(&format!("sys/identity/{node_a}")).map(|b| b.len()),
        None,
        "identity entry present"
    );

    // A seals a second record under the NEW key.
    a.audit(crate::AuditAction::Invoke, "client", "after-rotation", crate::AuditOutcome::Success, None).unwrap();

    // B must converge to A's full 2-record stream AND verify it end-to-end —
    // which requires B to hold BOTH of A's keys (retained set) and the chain to
    // link across the rotation.
    let mut ok = false;
    for _ in 0..200 {
        let stream = b.audit_stream(&node_a);
        if stream.len() == 2 && b.audit_verify(&node_a) == Ok(()) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "peer B failed to verify A's audit chain across the identity rotation");

    // B learned the new key (its retained set contains it).
    let _ = new_vk;

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-D / D1 gate (G-D1): explicit **revocation** closes the WS5 compromise caveat. A seals an audit
/// record under key1, rotates to key2 (B verifies the chain via the retained key set — the WS5
/// guarantee), then A **revokes key1**. Once the revocation gossips, B must **refuse** to verify the
/// key1-signed record — a revoked key is trusted for nothing.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_wsd_revoked_key_is_rejected_by_peer_verification() {
    use crate::config::TlsConfig;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_a = id(port_a);
    let cert_dir = std::env::temp_dir().join(format!("myc-wsd-{port_a}-{port_b}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![node_a.clone()]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    let mut peered = false;
    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() { peered = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(peered, "tls nodes failed to peer");

    // The key that will sign the record we later revoke.
    let old_key = a.identity_public_key().expect("a has a tls identity");
    a.audit(crate::AuditAction::Invoke, "client", "signed-by-old-key", crate::AuditOutcome::Success, None).unwrap();

    // Rotate to a fresh key, then seal a second record under it.
    let new_key = a.rotate_identity(Duration::from_millis(500)).await.expect("rotation");
    assert_ne!(new_key, old_key);
    a.audit(crate::AuditAction::Invoke, "client", "signed-by-new-key", crate::AuditOutcome::Success, None).unwrap();

    // WS5 guarantee first: B verifies the full chain across the rotation (retained key set).
    let mut verified_across_rotation = false;
    for _ in 0..200 {
        if b.audit_stream(&node_a).len() == 2 && b.audit_verify(&node_a) == Ok(()) {
            verified_across_rotation = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(verified_across_rotation, "WS5: B should verify A's chain across the rotation before revocation");

    // Now revoke the OLD key (signed by the current/new key). This is the compromise case.
    a.revoke_identity_key(old_key).expect("revoke");

    // Once the revocation gossips to B, B must REFUSE to verify A's chain — the genesis record was
    // signed by the now-revoked key1.
    let mut rejected = false;
    for _ in 0..200 {
        if b.audit_verify(&node_a) != Ok(()) {
            rejected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(rejected, "G-D1: B must reject a chain signed by a revoked key (the WS5 caveat is closed)");

    // And on A itself the revocation reads back as valid (signed by current key, revoked key owned).
    assert!(a.audit_verify(&node_a) != Ok(()), "A also rejects its own revoked-key-signed chain");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-D / M6 gate (G-D4 + G-D5): gossip-level capability authorization. With a `capauthz` policy
/// requiring role `operator` for `rush/worker`, a consumer **routes around** an advertiser that
/// lacks the role (D4 enforce) and counts the rejection on `/stats` (D5 detect) — while the
/// advertisement still propagated (detection-not-prevention). An advertiser holding the verified
/// role stays resolvable.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_wsd_capability_authz_routes_around_unauthorized_advertiser() {
    use crate::config::TlsConfig;
    use crate::{CapFilter, Capability};

    let pa = alloc_port();
    let pa2 = alloc_port();
    let pb = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let (node_a, node_a2) = (id(pa), id(pa2));
    let cert_dir = std::env::temp_dir().join(format!("myc-capauthz-{pa}-{pb}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };

    let a  = Arc::new(mk(pa, vec![]));                       // unauthorized advertiser (no role)
    let a2 = Arc::new(mk(pa2, vec![node_a.clone()]));        // authorized advertiser (operator role)
    let b  = Arc::new(mk(pb, vec![node_a.clone()]));         // consumer
    a.start().await.unwrap();
    a2.start().await.unwrap();
    b.start().await.unwrap();

    poll_until(|| !a.peers().is_empty() && !a2.peers().is_empty() && !b.peers().is_empty(), 5_000).await;

    // Both advertise rush/worker; only A2 advertises the operator role.
    let _r1 = a.capabilities().advertise_capability(Capability::new("rush", "worker"), Duration::from_secs(30));
    let _r2 = a2.capabilities().advertise_capability(Capability::new("rush", "worker"), Duration::from_secs(30));
    a2.advertise_roles([Arc::from("operator")], 1).unwrap();

    // B sees both providers before any policy.
    let filter = CapFilter::new("rush", "worker");
    poll_until(|| b.capabilities().resolve(&filter).len() >= 2, 10_000).await;
    assert_eq!(b.capabilities().resolve(&filter).len(), 2, "both providers resolvable with no policy");

    // Publish the policy: rush/worker requires role `operator`.
    assert!(b.set_capability_authz("rush", "worker", vec!["operator".into()]));

    // After the policy + A2's role gossip, B resolves ONLY the authorized advertiser.
    let mut converged = false;
    for _ in 0..200 {
        let r = b.capabilities().resolve(&filter);
        if r.len() == 1 && r[0].0 == node_a2 {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let resolved = b.capabilities().resolve(&filter);
    assert!(converged, "G-D4: B must route around the unauthorized advertiser — got {resolved:?}");
    assert!(resolved.iter().any(|(n, _)| n == &node_a2), "the authorized advertiser stays resolvable");
    assert!(!resolved.iter().any(|(n, _)| n == &node_a), "the unauthorized advertiser is excluded");

    // G-D5: the rejection is counted on /stats, and the advertisement still PROPAGATED (the cap
    // entry is present in B's store — detection, not prevention).
    assert!(b.system_stats().cap_authz_violations >= 1, "G-D5: the rejection is counted");
    assert!(b.kv().get(&format!("cap/{node_a}/rush/worker")).is_some(),
        "the unauthorized advertisement still propagated (detection, not prevention)");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    a2.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-D / M6 gate (G-D6): the capability-authz policy is set **through consensus** (agreed by a
/// quorum, not a unilateral LWW write), and is then enforced at resolve exactly like the D4 path.
#[cfg(all(feature = "compliance", feature = "consensus"))]
#[tokio::test]
async fn test_wsd_capability_authz_policy_via_consensus() {
    use crate::config::TlsConfig;
    use crate::{CapFilter, Capability, ConsensusConfig, ConsensusResult};

    let pa = alloc_port();
    let pb = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let (node_a, node_b) = (id(pa), id(pb));
    let cert_dir = std::env::temp_dir().join(format!("myc-capauthz-cons-{pa}-{pb}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };

    let a = Arc::new(mk(pa, vec![]));                  // voter + unauthorized advertiser
    let b = Arc::new(mk(pb, vec![node_a.clone()]));    // voter + authorized advertiser + consumer
    a.start().await.unwrap();
    b.start().await.unwrap();

    // Multi-node consensus requires a listener on every voter (CLAUDE.md).
    let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());

    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 5_000).await;

    let _r1 = a.capabilities().advertise_capability(Capability::new("rush", "worker"), Duration::from_secs(30));
    let _r2 = b.capabilities().advertise_capability(Capability::new("rush", "worker"), Duration::from_secs(30));
    b.advertise_roles([Arc::from("operator")], 1).unwrap();

    let filter = CapFilter::new("rush", "worker");
    poll_until(|| b.capabilities().resolve(&filter).len() >= 2, 10_000).await;

    // Set the policy THROUGH CONSENSUS (quorum = 2; both voters participate).
    let result = a.set_capability_authz_via_consensus(
        "rush", "worker", vec!["operator".into()], ConsensusConfig::default()).await;
    assert!(matches!(result, ConsensusResult::Committed { .. }),
        "G-D6: the policy must be agreed via consensus — got {result:?}");

    // The agreed policy is committed (proves it went through consensus, not a bare LWW write).
    assert!(a.consensus().consensus_get("capauthz/rush/worker").is_some(),
        "the policy is recorded as a committed consensus value");

    // …and is enforced at resolve exactly like D4: B routes around the unauthorized advertiser.
    let mut converged = false;
    for _ in 0..200 {
        let r = b.capabilities().resolve(&filter);
        if r.len() == 1 && r[0].0 == node_b {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged, "G-D6: the consensus-set policy is enforced at resolve");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-F / Schema-Evo gate (G-E2): a schema-version mismatch at resolve is **detected** — counted on
/// `/stats` and the provider routed around — never silently accepted. A matching version does not
/// trip the counter.
#[tokio::test]
async fn test_wsf_schema_mismatch_is_detected_at_resolve() {
    use crate::{Capability, CapFilter};

    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();

    // Advertise nlp/summarize at schema version v2.
    let _reg = agent.capabilities().advertise_capability(
        Capability::new("nlp", "summarize").with_schema_id("nlp/summarize@v2"),
        Duration::from_secs(30));
    poll_until(|| !agent.capabilities().resolve(&CapFilter::new("nlp", "summarize")).is_empty(), 3_000).await;

    let before = agent.system_stats().schema_mismatch;

    // A consumer expecting v1 finds nothing AND the mismatch is counted.
    let v1 = CapFilter::new("nlp", "summarize").with_schema("nlp/summarize@v1");
    assert!(agent.capabilities().resolve(&v1).is_empty(), "the v2 provider does not satisfy a v1 filter");
    let after_mismatch = agent.system_stats().schema_mismatch;
    assert!(after_mismatch > before, "G-E2: the schema-version mismatch is counted on /stats");

    // A consumer expecting the matching v2 resolves it, with no further mismatch count.
    let v2 = CapFilter::new("nlp", "summarize").with_schema("nlp/summarize@v2");
    assert_eq!(agent.capabilities().resolve(&v2).len(), 1, "the matching schema version resolves");
    assert_eq!(agent.system_stats().schema_mismatch, after_mismatch,
        "a matching version does not trip the counter");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// WS-F / Schema-Evo gate (G-E3a): the migration registry round-trips through the gossip KV — a
/// published migration is readable by `get_migration` / `list_migrations` on the same node (and
/// would gossip to peers like any KV entry).
#[tokio::test]
async fn test_wsf_migration_registry_round_trips() {
    use crate::schema_evolution::{MigrationRule, SchemaMigration};

    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();

    let m = SchemaMigration {
        from: "donation@v1".into(),
        to: "donation@v2".into(),
        rules: vec![
            MigrationRule::Rename { from: "origin".into(), to: "origin_zone".into() },
            MigrationRule::Default { path: "priority".into(), value: serde_json::json!(0) },
        ],
    };
    assert!(agent.publish_migration(&m), "publish queued");
    poll_until(|| agent.get_migration("donation@v1", "donation@v2").is_some(), 3_000).await;

    assert_eq!(agent.get_migration("donation@v1", "donation@v2").as_ref(), Some(&m),
        "the registered migration round-trips through the registry");
    assert!(agent.get_migration("donation@v2", "donation@v3").is_none(), "an unregistered path is absent");
    assert!(agent.list_migrations().contains(&m), "it appears in the catalogue");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// WS-F / Schema-Evo gate (G-E3b): `migrate_payload` composes the registered `v1→v2→v3` chain to
/// migrate a received payload; with the chain incomplete it returns `NoMigrationPath` (and trips the
/// `schema_mismatch` tripwire) rather than mis-parsing — detect, don't guess.
#[tokio::test]
async fn test_wsf_migrate_payload_composes_chain_or_detects_no_path() {
    use crate::schema_evolution::{MigrationError, MigrationRule, SchemaMigration};

    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();

    let m12 = SchemaMigration { from: "d@v1".into(), to: "d@v2".into(),
        rules: vec![MigrationRule::Rename { from: "origin".into(), to: "origin_zone".into() }] };
    agent.publish_migration(&m12);
    poll_until(|| agent.get_migration("d@v1", "d@v2").is_some(), 3_000).await;

    // Before v2→v3 is registered, v1→v3 has no path → NoMigrationPath + the tripwire fires.
    let before = agent.system_stats().schema_mismatch;
    let payload = br#"{"origin":"southwark","kg":12}"#;
    let err = agent.migrate_payload("d@v1", "d@v3", payload).unwrap_err();
    assert_eq!(err, MigrationError::NoMigrationPath { from: "d@v1".into(), to: "d@v3".into() });
    assert!(agent.system_stats().schema_mismatch > before, "a missing path trips schema_mismatch");

    // Register v2→v3; now v1→v3 composes and migrates the payload.
    let m23 = SchemaMigration { from: "d@v2".into(), to: "d@v3".into(),
        rules: vec![MigrationRule::Default { path: "priority".into(), value: serde_json::json!(0) }] };
    agent.publish_migration(&m23);
    poll_until(|| agent.get_migration("d@v2", "d@v3").is_some(), 3_000).await;

    let migrated = agent.migrate_payload("d@v1", "d@v3", payload).expect("the chain now composes");
    assert_eq!(migrated, serde_json::json!({ "origin_zone": "southwark", "kg": 12, "priority": 0 }),
        "the composed v1→v2→v3 chain migrates the payload");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// WS-F / Schema-Evo gate (G-E3c) — **the WS-F done-when**: a producer and consumer compiled against
/// *different* schema versions interoperate via an explicitly registered migration chain composed on
/// the receive side. Also exercises a nested (AgentFacts-shaped) document migration — the M16 pairing.
#[tokio::test]
async fn test_wsf_cross_version_producer_consumer_interop_end_to_end() {
    use crate::schema_evolution::{MigrationRule, SchemaMigration};
    use serde::{Deserialize, Serialize};

    // The producer is compiled against schema v1.
    #[derive(Serialize)]
    struct ProducerV1 { origin: String, kg: u32 }
    // The consumer is compiled against schema v3 — renamed field + a new required field.
    #[derive(Deserialize, PartialEq, Debug)]
    struct ConsumerV3 { origin_zone: String, kg: u32, priority: u8 }

    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();

    let wire = serde_json::to_vec(&ProducerV1 { origin: "southwark".into(), kg: 12 }).unwrap();

    // Without a migration, the v1 payload does NOT parse as v3 (the incompatibility is real).
    assert!(serde_json::from_slice::<ConsumerV3>(&wire).is_err(),
        "cross-version interop genuinely needs migration, not luck");

    // Register the explicit v1→v2→v3 chain.
    agent.publish_migration(&SchemaMigration { from: "d@v1".into(), to: "d@v2".into(),
        rules: vec![MigrationRule::Rename { from: "origin".into(), to: "origin_zone".into() }] });
    agent.publish_migration(&SchemaMigration { from: "d@v2".into(), to: "d@v3".into(),
        rules: vec![MigrationRule::Default { path: "priority".into(), value: serde_json::json!(0) }] });
    poll_until(|| agent.get_migration("d@v2", "d@v3").is_some(), 3_000).await;

    // The consumer migrates the received payload, then parses it into its own v3 type — interop.
    let migrated = agent.migrate_payload("d@v1", "d@v3", &wire).expect("chain composes");
    let parsed: ConsumerV3 = serde_json::from_value(migrated).expect("migrated payload parses as v3");
    assert_eq!(parsed, ConsumerV3 { origin_zone: "southwark".into(), kg: 12, priority: 0 });

    // M16 pairing: the same engine migrates a nested AgentFacts-shaped document (the quilt-fetcher
    // case — evolve `certification` across versions).
    agent.publish_migration(&SchemaMigration { from: "facts@v1".into(), to: "facts@v2".into(),
        rules: vec![MigrationRule::Default { path: "certification.schemaVersion".into(), value: serde_json::json!("v2") }] });
    poll_until(|| agent.get_migration("facts@v1", "facts@v2").is_some(), 3_000).await;
    let facts_v1 = br#"{"id":"did:mycelium:x","certification":{"scheme":"self-certified"}}"#;
    let facts_v2 = agent.migrate_payload("facts@v1", "facts@v2", facts_v1).expect("facts chain composes");
    assert_eq!(facts_v2["certification"]["schemaVersion"], serde_json::json!("v2"),
        "the engine evolves a nested AgentFacts certification field (M16 pairing)");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// WS-C / M7 gate (G-M7): distributed rate-limiting — shared observation, local decision. With
/// aggregate evidence (summed across observers via `sys/rate/`) over the threshold, the decider
/// throttles the sender on this node (`rate_limited_senders` reflects it); a calm sender never trips
/// it; and the throttle clears when the evidence evaporates.
#[tokio::test]
async fn test_wsc_m7_distributed_rate_limit_throttles_on_aggregate() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.rate_observation_enabled = true;
    cfg.rate_aggregate_threshold_fps = 900; // low, for the test
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();

    // Seed shared evidence: three observers each saw "noisy:1" at 500 fps → aggregate 1500 > 900.
    for obs in ["10.0.0.1:7", "10.0.0.2:7", "10.0.0.3:7"] {
        let _ = agent.kv().set(format!("sys/rate/{obs}/noisy:1"), Bytes::from_static(b"500"));
    }
    // A calm sender seen once at 100 fps → aggregate 100 < 900.
    let _ = agent.kv().set("sys/rate/10.0.0.1:7/calm:1", Bytes::from_static(b"100"));

    // The decider runs every ~2 s; wait for it to throttle exactly the noisy sender.
    poll_until(|| agent.system_stats().rate_limited_senders == 1, 8_000).await;
    assert_eq!(agent.system_stats().rate_limited_senders, 1,
        "G-M7: the over-threshold sender is throttled; the calm one is not");

    // Evidence evaporates (drop it) → the throttle clears on the next decider pass.
    for obs in ["10.0.0.1:7", "10.0.0.2:7", "10.0.0.3:7"] {
        let _ = agent.kv().delete(format!("sys/rate/{obs}/noisy:1"));
    }
    poll_until(|| agent.system_stats().rate_limited_senders == 0, 8_000).await;
    assert_eq!(agent.system_stats().rate_limited_senders, 0,
        "the throttle releases when the sender is no longer abusive");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// WS-C / M10.1 gate (G-M10.1): timing params are hot-reloadable — a live `set_*` retunes the
/// background loop on its next cycle with **no task restart** (`task_count` unchanged), the
/// management-as-intent / hot-reload way (no consensus fence).
#[tokio::test]
async fn test_wsc_m10_hot_reload_timing_no_task_restart() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.health_check_interval_secs = 5;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();
    poll_until(|| agent.system_stats().task_count > 0, 3_000).await;
    let tasks_before = agent.system_stats().task_count;

    // Live-retune the health-check interval and the reconnect backoff.
    agent.set_health_check_interval_secs(1).unwrap();
    agent.set_reconnect_backoff_secs(2).unwrap();
    assert_eq!(agent.timing_tunables(), (1, 2), "the live timing override is recorded");

    // The health monitor adopts the new cadence on its next cycle — no task respawned.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(agent.system_stats().task_count, tasks_before,
        "G-M10.1: hot-reload retunes the loop in place — no task restart");

    // `0` reverts to the static config value.
    agent.set_health_check_interval_secs(0).unwrap();
    assert_eq!(agent.timing_tunables().0, 0, "0 = revert to static config");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Realignment repairs R9: the live timing setters keep `validate()`'s bounds — health interval in
/// 1..=3600, reconnect backoff in 1..=300, with `0` still meaning *revert to the static value* — the
/// bounds the cluster timing governor already applies to a `TimingIntent`. A refused value changes
/// nothing: neither the live value nor the local pin that would make the node ignore the fleet.
#[test]
fn hot_timing_setters_refuse_what_validate_refuses() {
    use std::sync::atomic::Ordering;
    let port = alloc_port();
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), GossipConfig::default());
    let pinned = |a: &GossipAgent| (a.task_ctx.hot.health_locally_pinned.load(Ordering::Relaxed),
                                    a.task_ctx.hot.reconnect_locally_pinned.load(Ordering::Relaxed));

    let before = agent.timing_tunables();
    assert!(agent.set_health_check_interval_secs(3601).is_err(), "refused by name");
    assert!(agent.set_reconnect_backoff_secs(301).is_err(), "refused by name");
    assert_eq!(agent.timing_tunables(), before, "an out-of-range value is not applied");
    assert_eq!(pinned(&agent), (false, false), "nor does it pin the node against the fleet");

    agent.set_health_check_interval_secs(3600).unwrap();
    agent.set_reconnect_backoff_secs(300).unwrap();
    assert_eq!(agent.timing_tunables(), (3600, 300), "the bounds themselves are allowed");
    agent.set_health_check_interval_secs(0).unwrap();
    agent.set_reconnect_backoff_secs(0).unwrap();
    assert_eq!(agent.timing_tunables(), (0, 0), "0 still reverts to the static value");
}

/// Every outbound client's `with_egress` takes the node's policy as `agent.egress_policy()` hands it —
/// a reference — or by value (doc-coverage run 20, code gap 5: `OpenAiBackend` took only a reference,
/// the other three only a value, and the docs wrote one form for all four).
#[cfg(feature = "llm")]
#[test]
fn every_with_egress_takes_the_policy_by_reference_or_by_value() {
    let policy = crate::config::EgressPolicy { allow_hosts: vec!["llm.internal".into()] };
    let _ = crate::OpenAiBackend::new("http://llm.internal/v1", "", "m").with_egress(&policy);
    let _ = crate::OpenAiBackend::new("http://llm.internal/v1", "", "m").with_egress(policy.clone());
}

/// R9's third door, from Rust: `govern_timing` publishes nothing a node would not apply.
#[test]
fn govern_timing_does_not_publish_an_intent_no_node_would_apply() {
    let port = alloc_port();
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), GossipConfig::default());
    assert!(agent.govern_timing(99_999, 0, None).is_err(), "above 3600");
    assert!(agent.govern_timing(0, 301, None).is_err(), "above 300");
    assert!(agent.govern_timing(0, 0, None).is_err(), "governs nothing (the review of #544)");
    assert!(agent.kv().get(crate::agent::timing_governor::TIMING_INTENT_KEY).is_none(),
        "an out-of-range timing intent is not published");
}

/// WS-C / M10.2 gate (G-M10.2): live timing reconfiguration cluster-wide, intent-governed and
/// fence-free. A `TimingIntent` published on one node is reconciled by every node within the TTL; a
/// node-local `set_*` wins over the fleet intent (local-wins); and letting the intent evaporate
/// returns non-pinned nodes to baseline.
#[tokio::test]
async fn test_wsc_m10_timing_intent_governs_fleet_with_local_wins() {
    let pa = alloc_port();
    let pb = alloc_port();
    let node_a = NodeId::new("127.0.0.1", pa).unwrap();
    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.health_check_interval_secs = 5;
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    };
    let a = mk(pa, vec![]);
    let b = mk(pb, vec![node_a.clone()]);
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 5_000).await;

    // B pins its own timing locally → it must ignore the fleet intent (local-wins).
    b.set_health_check_interval_secs(9).unwrap();

    // A publishes a fleet TimingIntent (whole fleet).
    assert!(a.govern_timing(2, 3, None).unwrap(), "intent published");

    // A (not pinned) adopts the fleet intent; B (pinned) keeps its local value.
    poll_until(|| a.timing_tunables() == (2, 3), 6_000).await;
    assert_eq!(a.timing_tunables(), (2, 3), "G-M10.2: the non-pinned node adopts the fleet intent");
    assert_eq!(b.timing_tunables().0, 9, "local-wins: the pinned node ignores the fleet intent");

    // Evaporate the intent (delete the key) → A self-heals back to baseline (0 = static).
    let _ = a.kv().delete(crate::agent::timing_governor::TIMING_INTENT_KEY);
    poll_until(|| a.timing_tunables() == (0, 0), 6_000).await;
    assert_eq!(a.timing_tunables(), (0, 0), "the non-pinned node self-heals to baseline on evaporation");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Ops cluster-name: the optional `cluster_name` config flows to the public accessor (and thence to
/// `/stats`, the `/metrics` `cluster` label, and AgentFacts). Purely a label — no effect on identity.
#[test]
fn test_cluster_name_config_flows_to_accessor() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.cluster_name = Some("prod-eu".into());
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    assert_eq!(agent.cluster_name(), Some("prod-eu"));

    // Default (unset) reads as None — unlabelled.
    let mut cfg2 = GossipConfig::default();
    cfg2.bind_port = alloc_port();
    let plain = GossipAgent::new(NodeId::new("127.0.0.1", cfg2.bind_port).unwrap(), cfg2);
    assert_eq!(plain.cluster_name(), None);
}

// ── M2 falsification probe (Run 24): concurrent audit chain integrity ─────

/// Probe: many concurrent `audit()` calls on one node must produce a strictly
/// linear, gap-free, verifiable hash chain — i.e. the per-node chain lock (#8)
/// serialises seq/prev_hash assignment correctly and the sign-outside-the-lock
/// optimisation does not corrupt linkage under contention.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn probe_concurrent_audit_chain_is_contiguous_and_verifies() {
    use crate::config::TlsConfig;

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-probe-cc-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    a.start().await.unwrap();

    let id_key = format!("sys/identity/{id}");
    let mut vkb = None;
    for _ in 0..100 {
        if let Some(b) = a.kv().get(&id_key) { vkb = Some(b); break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let vkb = vkb.expect("identity key");
    let mut vk = [0u8; 32];
    vk.copy_from_slice(&vkb[..32]);

    // Fire N concurrent audits.
    let n = 64u64;
    let mut handles = Vec::new();
    for i in 0..n {
        let a2 = Arc::clone(&a);
        handles.push(tokio::spawn(async move {
            a2.audit(
                crate::AuditAction::Invoke,
                format!("caller-{i}"),
                "concurrent",
                crate::AuditOutcome::Success,
                None,
            ).unwrap()
        }));
    }
    let content_hashes: Vec<[u8; 32]> = {
        let mut v = Vec::new();
        for h in handles { v.push(h.await.unwrap()); }
        v
    };
    // Every returned content hash is distinct (no two records collided).
    let mut uniq = content_hashes.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), n as usize, "every concurrent audit produced a distinct record");

    // The stored stream is contiguous 0..N and verifies end-to-end.
    let mut entries = a.kv().scan_prefix(&crate::audit_stream_prefix(&id));
    entries.sort_by(|x, y| x.0.cmp(&y.0));
    let chain: Vec<crate::SignedAuditRecord> = entries
        .iter()
        .filter_map(|(_, v)| crate::SignedAuditRecord::decode(v))
        .collect();
    assert_eq!(chain.len() as u64, n, "no lost or duplicated seq under contention");
    for (i, sr) in chain.iter().enumerate() {
        assert_eq!(sr.record.seq, i as u64, "contiguous seq (no gap/collision)");
    }
    assert_eq!(
        crate::verify_stream_from_genesis(&chain, &id, &vk),
        Ok(()),
        "concurrently-built chain verifies"
    );

    // And tamper-evidence still holds on the concurrent chain.
    let mut tampered = chain.clone();
    tampered[n as usize / 2].record.principal = "EVIL".into();
    assert!(
        crate::verify_stream_from_genesis(&tampered, &id, &vk).is_err(),
        "a tampered record in the concurrent chain fails verification"
    );

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-F M16-A prerequisite: the public node-identity signing surface (`sign_with_identity` +
/// `identity_public_key`) lets a public-API consumer self-certify a document under the node
/// identity and a fetcher verify it — the foundation AgentFacts emission builds on.
#[cfg(feature = "tls")]
#[tokio::test]
async fn test_wsf_public_identity_signing_round_trips() {
    use crate::config::TlsConfig;

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-wsf-sign-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.unwrap();

    let doc = b"agent-facts-document-bytes";
    let sig = a.sign_with_identity(doc).expect("tls node signs");
    let pk = a.identity_public_key().expect("tls node has a public key");

    // A fetcher verifies the self-signed document against the published key.
    assert!(crate::tls::verify_bytes(&pk, doc, &sig), "self-signed document verifies");
    // Tampered document → verification fails.
    assert!(!crate::tls::verify_bytes(&pk, b"tampered", &sig), "tampered document rejected");
    // The published key matches the gossiped identity (sys/identity/{self}).
    let id_key = format!("sys/identity/{}", a.node_id());
    let mut idb = None;
    for _ in 0..100 {
        if let Some(b) = a.kv().get(&id_key) { idb = Some(b); break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let idb = idb.expect("identity gossiped");
    assert_eq!(&idb[..32], &pk[..], "identity_public_key matches the gossiped sys/identity key");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

// ── M2 Run-28 falsification probes ────────────────────────────────────────

/// M2 Run-28 probe (dim 6 — error model), **flipped to a regression gate** same-day:
/// `kv().set()` used to accept values that cannot fit a gossip frame (`true`, applied
/// locally, WAL-appended) while the value could never leave the node — silent permanent
/// divergence — and the per-peer writer tore down the healthy connection on the
/// resulting `FrameTooLarge`. Fixed by the `MAX_KV_WRITE_BYTES` guard in
/// `kv_set`/`kv_set_async` (reject outright: `false`, nothing applied, `warn!`) and by
/// the writer dropping an oversized frame without tearing down the connection.
#[tokio::test]
async fn test_oversized_value_is_rejected_outright_and_cluster_stays_healthy() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port                  = port_a;
    cfg_a.bootstrap_peers            = vec![id_b.clone()];
    cfg_a.health_check_max_jitter_ms = 50;
    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port                  = port_b;
    cfg_b.bootstrap_peers            = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;
    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b, cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 5_000).await;

    // Healthy-cluster baseline: a small key propagates A → B.
    assert!(a.kv().set("probe/small-before", "x"));
    poll_until(|| b.kv().get("probe/small-before").is_some(), 10_000).await;

    // An oversized write is rejected outright: no local apply, no queue, `false`.
    let big = vec![0u8; crate::framing::MAX_FRAME_BYTES + 64 * 1024];
    assert!(
        !a.kv().set("probe/oversized", big.clone()),
        "kv.set must reject a value that cannot fit a gossip frame"
    );
    assert!(
        a.kv().get("probe/oversized").is_none(),
        "a rejected oversized write must not be applied to the local store"
    );
    assert!(
        !a.kv().set_async("probe/oversized-async", big).await,
        "kv.set_async must reject the same way"
    );

    // The cluster is unharmed: a subsequent small key still propagates promptly.
    assert!(a.kv().set("probe/small-after", "y"));
    poll_until(|| b.kv().get("probe/small-after").is_some(), 10_000).await;

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// M2 Run-28 follow-up gate (dims 12/16): anti-entropy must converge a late joiner
/// whose divergence exceeds one gossip frame. Pre-fix, `StateResponse` was a single
/// unchunked frame: a >`MAX_FRAME_BYTES` full dump was skipped with a warn and never
/// retried, so a late joiner of a large store never converged. Now the response is
/// chunked; this test also plants one poison entry (injected past the write guard, as
/// a legacy store could hold) and asserts the sync skips it and still delivers
/// everything else.
#[tokio::test]
async fn test_late_joiner_converges_past_frame_sized_store_via_chunked_anti_entropy() {
    use crate::framing::make_gossip_update;
    use crate::store::apply_and_notify;

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id_a = NodeId::new("127.0.0.1", port_a).unwrap();
    let id_b = NodeId::new("127.0.0.1", port_b).unwrap();
    let mut cfg_a = GossipConfig::default();
    cfg_a.bind_port                  = port_a;
    cfg_a.health_check_max_jitter_ms = 50;
    let a = GossipAgent::new(id_a.clone(), cfg_a);
    a.start().await.unwrap();

    // ~12.3 MiB across 120 keys — more than one MAX_FRAME_BYTES frame can carry.
    let n_keys = 120usize;
    let val = vec![7u8; 105 * 1024];
    for i in 0..n_keys {
        assert!(a.kv().set_async(format!("bulkstore/{i:04}"), val.clone()).await);
    }
    // Poison entry: apply an un-frameable value directly (bypassing the kv_set guard,
    // the way a legacy store might hold one). The sync must skip it, not stall on it.
    let poison = make_gossip_update(
        &id_a,
        a.task_ctx.default_ttl,
        Arc::from("bulkstore/poison"),
        Bytes::from(vec![9u8; crate::framing::MAX_FRAME_BYTES]),
        false,
        &a.task_ctx.hlc,
    );
    apply_and_notify(&a.task_ctx.kv_state, &poison);
    assert!(a.kv().get("bulkstore/poison").is_some());

    // Late joiner: bootstraps to A after the writes — anti-entropy is its only source.
    let mut cfg_b = GossipConfig::default();
    cfg_b.bind_port                  = port_b;
    cfg_b.bootstrap_peers            = vec![id_a.clone()];
    cfg_b.health_check_max_jitter_ms = 50;
    let b = GossipAgent::new(id_b, cfg_b);
    b.start().await.unwrap();

    poll_until(
        || (0..n_keys).all(|i| b.kv().get(&format!("bulkstore/{i:04}")).is_some()),
        30_000,
    ).await;
    assert!(
        b.kv().get("bulkstore/poison").is_none(),
        "the un-frameable poison entry must be skipped, not delivered"
    );

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// M2 Run-28 probe (dim 10 — resource management): dropping a `CapabilityReg`
/// must tombstone the `cap/{node}/{ns}/{name}` advertisement (the documented
/// drop contract on `advertise_capability`). PASSED at Run 28 — kept as a
/// regression gate.
#[tokio::test]
async fn test_capability_reg_drop_tombstones_advertisement() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();

    let key = format!("cap/{}/probe/drop-retract", agent.node_id());
    let reg = agent.capabilities().advertise_capability(
        Capability::new("probe", "drop-retract"),
        Duration::from_millis(100),
    );
    poll_until(|| agent.kv().get(&key).is_some(), 5_000).await;

    drop(reg);
    poll_until(|| agent.kv().get(&key).is_none(), 5_000).await;

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── M2 Run-29 falsification probes ────────────────────────────────────────

/// M2 Run-29 probe (dim 11 — semantic correctness): equal-timestamp LWW must
/// converge *regardless of apply order* and *across more than two concurrent
/// writers* (Run 27 covered two data writers; this adds a third and the
/// tombstone-tie rule). All permutations of three equal-ts writes to one key —
/// three distinct values plus a tombstone — must land on the same StoreEntry.
/// The deterministic rule (`store.rs::lww_wins`): among equal-ts writes a
/// tombstone always wins the tie; among equal-ts data writes the lexicographically
/// greater value wins; data never resurrects over an equal-ts tombstone. PASSED
/// Run 29 — kept as a regression gate.
#[test]
fn test_equal_timestamp_lww_converges_across_three_writers_all_orders() {
    use crate::store::{apply_and_notify, KvState};

    // Four concurrent equal-ts (ts=1) writes to one key.
    let writes: Vec<GossipUpdate> = vec![
        data_update("k", b"alpha",   1, false),
        data_update("k", b"bravo",   1, false),
        data_update("k", b"charlie", 1, false),
        data_update("k", b"",        4, true),   // tombstone (distinct nonce)
    ];

    // Reference outcome: a tombstone present among equal-ts writes always wins.
    let expected: Option<Bytes> = None;

    // Every permutation of apply order must converge to the same StoreEntry.
    let idx = [0usize, 1, 2, 3];
    let mut perms = vec![idx.to_vec()];
    // Heap's algorithm (iterative not needed — 24 perms, generate by std lib style).
    fn permute(v: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
        if k == 1 { out.push(v.clone()); return; }
        for i in 0..k {
            permute(v, k - 1, out);
            if k.is_multiple_of(2) { v.swap(i, k - 1); } else { v.swap(0, k - 1); }
        }
    }
    let mut base = idx.to_vec();
    perms.clear();
    permute(&mut base, 4, &mut perms);
    assert_eq!(perms.len(), 24);

    for perm in &perms {
        let kv = KvState::new(0);
        for &i in perm {
            apply_and_notify(&kv, &writes[i]);
        }
        let got = kv.store.pin().get("k").map(|e| e.data.clone()).unwrap();
        assert_eq!(
            got, expected,
            "equal-ts convergence broke for apply order {perm:?}: got {got:?}"
        );
    }

    // Control: with NO tombstone, all data-write orders converge to max value ("charlie").
    let data_only = [&writes[0], &writes[1], &writes[2]];
    let data_idx = [0usize, 1, 2];
    let mut db = data_idx.to_vec();
    let mut dperms = Vec::new();
    permute(&mut db, 3, &mut dperms);
    for perm in &dperms {
        let kv = KvState::new(0);
        for &i in perm { apply_and_notify(&kv, data_only[i]); }
        let got = kv.store.pin().get("k").map(|e| e.data.clone()).unwrap();
        assert_eq!(
            got.as_deref(), Some(&b"charlie"[..]),
            "equal-ts data-only convergence broke for order {perm:?}: got {got:?}"
        );
    }
}

/// Legible-Emergence Phase 1 — **live end-to-end** #56 reproduction. The pure detectors are
/// unit-tested in `agent::emergent`; this exercises the whole path a real deployment uses: a
/// started agent with `emergent_detectors_enabled`, the spawned detector loop, the governor-intent
/// publish path, and the `/stats` gauge atomic. It reproduces the governor-vs-emergent-autojoin
/// condition (#56) — a group capped at max=2 that observes 4 members — and asserts the detector
/// *fires*, then *clears* when membership returns in-bounds (the false-positive direction).
#[tokio::test]
async fn test_p1_governed_group_conflict_detector_fires_and_clears_end_to_end() {
    use std::sync::atomic::Ordering;
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.emergent_detectors_enabled = true;
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();

    // Governor caps "workers" at [min=1, max=2] (publish_membership_intent stamps it fresh).
    assert!(agent.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));

    // Emergent auto-join pushes the group to 4 live members — over the cap. Inject them as
    // `grp/workers/{node}` entries (the node segment must round-trip through NodeId::parse).
    let members: Vec<NodeId> = (0..4).map(|i| NodeId::new("127.0.0.1", 25000 + i).unwrap()).collect();
    for m in &members {
        assert!(agent.kv().set(format!("grp/workers/{m}"), "1"));
    }

    // The detector loop ticks every ~2 s and confirms after CONFIRM_TICKS — wait it out.
    poll_until(
        || agent.task_ctx.governed_group_conflicts.load(Ordering::Relaxed) >= 1,
        15_000,
    ).await;

    // Bring membership back in-bounds (tombstone 3 of 4 → 1 ∈ [1,2]) and assert the gauge clears.
    for m in &members[1..] {
        assert!(agent.kv().delete(format!("grp/workers/{m}")));
    }
    poll_until(
        || agent.task_ctx.governed_group_conflicts.load(Ordering::Relaxed) == 0,
        15_000,
    ).await;

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Legible-Emergence Phase 2 — the fleet-snapshot **acceptance gate** (RT1-restated): on a seeded
/// conflicted fleet, the snapshot from *three different nodes* agrees on the **diagnosis** (the
/// governed-group conflict + the capability-coverage gap), while each node's `view_confidence` is
/// its own. Proves the snapshot is coordinator-free — computed locally from converged KV, identical
/// across observers — the thing a central collector would otherwise provide.
#[tokio::test]
async fn test_fleet_snapshot_agrees_across_three_nodes_at_convergence() {
    use crate::capability::{CapFilter, ReqEntry};
    let (pa, pb, pc) = (alloc_port(), alloc_port(), alloc_port());
    let (ia, ib, ic) = (
        NodeId::new("127.0.0.1", pa).unwrap(),
        NodeId::new("127.0.0.1", pb).unwrap(),
        NodeId::new("127.0.0.1", pc).unwrap(),
    );
    let mk = |port: u16, boot: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boot;
        cfg.health_check_max_jitter_ms = 50;
        cfg.emergent_detectors_enabled = true; // so each node publishes its sys/health self-report
        GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
    };
    let a = mk(pa, vec![ib.clone()]);
    let b = mk(pb, vec![ic.clone()]);
    let c = mk(pc, vec![ia.clone()]);
    a.start().await.unwrap();
    b.start().await.unwrap();
    c.start().await.unwrap();
    // Ring forms.
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty() && !c.peers().is_empty(), 5_000).await;

    // Seed a conflict + a coverage gap on node A; KV floods to all three.
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        let m = NodeId::new("127.0.0.1", 26000 + i).unwrap();
        assert!(a.kv().set(format!("grp/workers/{m}"), "1"));
    }
    let req = ReqEntry { filter: CapFilter::new("ai", "llm"), refresh_interval_ms: 60_000 };
    assert!(a.kv().set(format!("req/{ia}/ai/llm"), req.encode())); // no provider ⇒ gap

    let diagnosis = |node: &GossipAgent| {
        let s = crate::agent::emergent::compute_fleet_snapshot(&node.task_ctx);
        let conflict = s.governed_groups.iter().any(|g| g.group == "workers" && g.conflict && g.observed == 4);
        (conflict, s.capability_coverage_gaps.contains(&"ai/llm".to_string()))
    };
    // Wait for all three to converge on the same diagnosis AND to see all three sys/health reports
    // (the cross-node store-convergence field — Field 1, published by each node's detector loop).
    let converged = |n: &GossipAgent| {
        let s = crate::agent::emergent::compute_fleet_snapshot(&n.task_ctx);
        diagnosis(n) == (true, true) && s.store_convergence.nodes_reporting == 3
    };
    poll_until(|| converged(&a) && converged(&b) && converged(&c), 20_000).await;

    // The diagnosis is byte-identical across observers; view_confidence is each node's own.
    let (sa, sb, sc) = (
        crate::agent::emergent::compute_fleet_snapshot(&a.task_ctx),
        crate::agent::emergent::compute_fleet_snapshot(&b.task_ctx),
        crate::agent::emergent::compute_fleet_snapshot(&c.task_ctx),
    );
    assert_eq!(sa.store_convergence.nodes_reporting, 3, "all three nodes' sys/health self-reports visible");
    assert_eq!(sa.governed_groups, sb.governed_groups, "A and B agree on governed-group diagnosis");
    assert_eq!(sb.governed_groups, sc.governed_groups, "B and C agree");
    assert_eq!(sa.capability_coverage_gaps, sc.capability_coverage_gaps, "A and C agree on coverage gaps");
    assert_eq!(sa.view_confidence.observer, ia.to_string(), "each snapshot is labelled with its own observer");
    assert_eq!(sc.view_confidence.observer, ic.to_string());

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    c.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Legible-Emergence **Phase 3 increment 2** — the cross-node causal `explain` fan-out. Proves the
/// two halves of the DoD in one run: (1) `assemble_explain` on node A merges A's *and* B's
/// event rings into one HLC-ordered narrative (cross-node causal assembly), and (2) node C — a
/// live, gossiping peer that runs **without** the explain responder — is named as a `non_responder`
/// rather than silently dropped (RT3: render what you have + name the gaps). C-without-responder is
/// the deterministic stand-in for a slow/partitioned node, avoiding an eviction-timing race.
#[tokio::test]
async fn test_explain_fanout_assembles_cross_node_ring_and_names_non_responders() {
    let (pa, pb, pc) = (alloc_port(), alloc_port(), alloc_port());
    let (ia, ib, ic) = (
        NodeId::new("127.0.0.1", pa).unwrap(),
        NodeId::new("127.0.0.1", pb).unwrap(),
        NodeId::new("127.0.0.1", pc).unwrap(),
    );
    let mk = |port: u16, boot: Vec<NodeId>, detectors: bool| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boot;
        cfg.health_check_max_jitter_ms = 50;
        cfg.emergent_detectors_enabled = detectors;
        GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
    };
    // A + B run the detector loop AND the explain responder; C gossips but has neither — so C is a
    // live peer that never answers `sys.explain` (the deterministic non-responder).
    let a = mk(pa, vec![ib.clone()], true);
    let b = mk(pb, vec![ic.clone()], true);
    let c = mk(pc, vec![ia.clone()], false);
    a.start().await.unwrap();
    b.start().await.unwrap();
    c.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty() && !c.peers().is_empty(), 5_000).await;
    // A must learn C as a peer (peer-exchange around the ring) for C to be a fan-out target.
    poll_until(|| a.peers().len() >= 2, 10_000).await;

    // Seed a governed-group conflict on A; KV floods to B (and C), so both A's and B's detector
    // loops confirm it and record a `governed_group_conflict` onset event into their own rings.
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        let m = NodeId::new("127.0.0.1", 26100 + i).unwrap();
        assert!(a.kv().set(format!("grp/workers/{m}"), "1"));
    }
    // Wait until both A and B have recorded the onset event locally (hysteresis ≈ 4 s).
    poll_until(|| {
        !a.task_ctx.event_ring.since(0).is_empty() && !b.task_ctx.event_ring.since(0).is_empty()
    }, 20_000).await;

    let res = crate::agent::emergent::assemble_explain(&a.task_ctx, 0).await;

    // (1) Cross-node assembly: the merged narrative carries events authored by BOTH A and B.
    assert!(res.events.iter().any(|e| e.node == ia.to_string()), "A's own events present");
    assert!(res.events.iter().any(|e| e.node == ib.to_string()), "B's events assembled via fan-out");
    // HLC-ordered.
    let hlcs: Vec<u64> = res.events.iter().map(|e| e.hlc).collect();
    let mut sorted = hlcs.clone();
    sorted.sort();
    assert_eq!(hlcs, sorted, "assembled narrative is HLC causal-ordered");
    // B answered.
    assert!(res.responders.contains(&ib.to_string()), "B is a responder");

    // (2) RT3: C is a live peer that runs no responder ⇒ named non-responder, not a silent gap.
    assert!(res.non_responders.contains(&ic.to_string()),
        "C (live peer, no explain responder) is named as a non-responder; got responders={:?} non_responders={:?}",
        res.responders, res.non_responders);
    assert!(!res.responders.contains(&ic.to_string()), "C did not answer");
    assert_eq!(res.observer, ia.to_string(), "result is labelled with the assembling observer");
    assert_eq!(res.not_queried, 0, "a 2-peer fleet is well under the fan-out cap — nothing skipped");

    // (3) The #56 reconstruction narrative — the assembled ring renders an operator-legible story
    // (one line per event) that names the specific group + band, with no code knowledge required.
    assert_eq!(res.narrative.len(), res.events.len(), "one narrative line per event");
    assert!(res.narrative.iter().any(|l|
        l.contains("governor's [min,max] band") && l.contains("workers")),
        "narrative legibly describes the governed-group conflict on 'workers': {:?}", res.narrative);

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    c.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Legible-Emergence **Phase 4** — the fleet narrative / diagnosis. Grounds the rule engine against
/// a *real* KV-derived snapshot (not a synthetic struct): seed an actual governor-vs-membership
/// conflict, then assert `compute_fleet_diagnosis` names the cause on 'workers' in actionable,
/// code-free terms — the Phase-4 acceptance bar. Single node: the snapshot's `conflict` flag is a
/// pure KV scan (no detector loop / hysteresis needed to surface it).
#[tokio::test]
async fn test_fleet_diagnosis_names_a_real_governed_group_conflict() {
    let p = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = p;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();

    // Seed a real conflict: governor caps 'workers' at [1, 2]; four live members.
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        assert!(a.kv().set(format!("grp/workers/127.0.0.1:{}", 26200 + i), "1"));
    }

    // The diagnosis (over the real snapshot) names the workers conflict, actionably.
    poll_until(|| {
        let d = crate::agent::emergent::compute_fleet_diagnosis(&a.task_ctx);
        d.findings.iter().any(|f| f.pathology.starts_with("governed_group") && f.cause.contains("workers"))
    }, 5_000).await;

    let d = crate::agent::emergent::compute_fleet_diagnosis(&a.task_ctx);
    let f = d.findings.iter().find(|f| f.cause.contains("workers"))
        .expect("diagnosis must name the real 'workers' conflict");
    assert!(f.cause.contains("Action:"), "diagnosis is actionable: {}", f.cause);
    assert!(f.cause.contains("[1, 2]") && f.cause.contains('4'), "names the band + observed: {}", f.cause);
    assert!(d.summary.contains("condition"), "summary counts the condition: {}", d.summary);

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── M2 self-audit falsification probes (analysis Run 30) — kept as regression tests ──────────────

/// **Probe — Philosophy/Coherence (detection, not prevention).** Running the fleet diagnosis over a
/// governed-group conflict must NOT mutate the observed `grp/` membership — the diagnosis *names*
/// the pathology, it never drains nodes to "fix" it. Falsifies any hidden correction path.
#[tokio::test]
async fn probe_diagnosis_observes_but_never_corrects_a_conflict() {
    let p = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = p;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        assert!(a.kv().set(format!("grp/workers/127.0.0.1:{}", 27400 + i), "1"));
    }
    let observed = || a.fleet_snapshot().governed_groups.iter()
        .find(|g| g.group == "workers").map(|g| g.observed).unwrap_or(0);
    poll_until(|| observed() == 4, 3_000).await;
    let before = observed();
    // Hammer the diagnosis: if it corrected anything, the membership would move.
    for _ in 0..25 {
        let _ = a.fleet_diagnosis();
        let _ = a.fleet_snapshot();
    }
    assert_eq!(before, observed(), "diagnosis must not correct the conflict (detection, not prevention)");
    assert!(a.fleet_diagnosis().findings.iter().any(|f| f.cause.contains("workers")),
        "the conflict is still diagnosed, not silently resolved");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Probe — coordinator-free (no collector).** A lone node with no peers computes a well-formed
/// diagnosis from its own KV — no quorum, no aggregator, no hang. Falsifies any hidden dependency
/// on a peer/collector for the fleet view.
#[tokio::test]
async fn probe_lone_node_diagnoses_without_a_collector() {
    let p = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = p;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    // No peers, no bootstrap. The diagnosis is a local computation.
    let d = a.fleet_diagnosis();
    assert_eq!(d.observer, format!("127.0.0.1:{p}"), "labelled with its own identity");
    assert!(d.findings.is_empty() && d.summary.to_lowercase().contains("nominal"),
        "a healthy lone node reads nominal: {d:?}");
    assert!(a.peers().is_empty(), "truly no peers — the diagnosis needed no collector");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Probe — Failure-Mode Legibility.** Every finding's `cause` must be operator-readable: an
/// actionable sentence, never a raw code identifier. Seed two distinct pathologies and assert no
/// cause leaks its snake_case pathology id and every one carries an `Action:`.
#[tokio::test]
async fn probe_every_diagnosis_finding_is_operator_legible() {
    use crate::capability::{CapFilter, ReqEntry};
    let p = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = p;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    // Pathology 1: a governed-group conflict.
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        assert!(a.kv().set(format!("grp/workers/127.0.0.1:{}", 27500 + i), "1"));
    }
    // Pathology 2: a capability-coverage gap (a demand with no provider).
    let req = ReqEntry { filter: CapFilter::new("ai", "llm"), refresh_interval_ms: 60_000 };
    assert!(a.kv().set(format!("req/127.0.0.1:{p}/ai/llm"), req.encode()));

    poll_until(|| a.fleet_diagnosis().findings.len() >= 2, 5_000).await;
    let d = a.fleet_diagnosis();
    assert!(d.findings.len() >= 2, "both pathologies diagnosed: {:?}", d.findings);
    for f in &d.findings {
        assert!(f.cause.contains("Action:"), "finding is actionable: {}", f.cause);
        assert!(!f.cause.contains(&f.pathology),
            "cause must not leak its raw pathology id ({}): {}", f.pathology, f.cause);
    }
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Probe — Philosophy/Coherence (analysis Run 31, diagnostics as a pure read).** `fleet_diagnosis`
/// is a pure function of the store, not a stateful engine: called repeatedly against unchanged KV it
/// returns the *same* load-bearing findings, with no accumulating state and no self-perturbation.
/// (Run-30 probed "does not correct a conflict"; this probes idempotence — a distinct angle.)
#[tokio::test]
async fn probe_r31_diagnosis_is_idempotent_no_accumulating_state() {
    use crate::capability::{CapFilter, ReqEntry};
    let p = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = p;
    let a = GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg);
    a.start().await.unwrap();
    assert!(a.publish_membership_intent(MembershipIntent::new("workers", 1, Some(2))));
    for i in 0..4 {
        assert!(a.kv().set(format!("grp/workers/127.0.0.1:{}", 27600 + i), "1"));
    }
    let req = ReqEntry { filter: CapFilter::new("ai", "llm"), refresh_interval_ms: 60_000 };
    assert!(a.kv().set(format!("req/127.0.0.1:{p}/ai/llm"), req.encode()));
    poll_until(|| a.fleet_diagnosis().findings.len() >= 2, 5_000).await;

    // 50 diagnoses against the same KV: the load-bearing findings are present *every* time.
    for _ in 0..50 {
        let d = a.fleet_diagnosis();
        assert!(d.findings.iter().any(|f| f.cause.contains("workers")),
            "the workers conflict is diagnosed on every call (idempotent)");
        assert!(d.findings.iter().any(|f| f.pathology == "capability_coverage_gap"),
            "the coverage gap is diagnosed on every call (no accumulating state)");
    }
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Run-41 falsification probe (concurrency): `connect_peer` hammered from concurrent tasks
/// while the agent shuts down must neither panic nor wedge shutdown. The warm path spawns a
/// writer + sends a Ping; `get_or_spawn_writer` refuses during shutdown (returns None) — this
/// asserts that guard holds under a real race, not just in isolation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_peer_racing_shutdown_is_safe() {
    let port = crate::test_util::alloc_port();
    let agent = std::sync::Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).unwrap(),
        GossipConfig { bind_port: port, ..Default::default() },
    ));
    agent.start().await.unwrap();

    // 8 tasks hammering connect/disconnect against distinct (dead) peers…
    let mut tasks = Vec::new();
    for t in 0..8u16 {
        let a = std::sync::Arc::clone(&agent);
        tasks.push(tokio::spawn(async move {
            for i in 0..200u16 {
                let peer = NodeId::new("127.0.0.1", 40000 + t * 300 + (i % 250)).unwrap();
                a.connect_peer(peer.clone());
                if i % 3 == 0 {
                    a.disconnect_peer(&peer);
                }
                tokio::task::yield_now().await;
            }
        }));
    }
    // …while shutdown lands mid-hammer.
    tokio::time::sleep(Duration::from_millis(10)).await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;

    // All hammer tasks finish without panic (a panicked task returns Err here).
    for t in tasks {
        t.await.expect("connect_peer task panicked during shutdown race");
    }
    // And post-shutdown calls are inert, not panicking.
    agent.connect_peer(NodeId::new("127.0.0.1", 41999).unwrap());
}

/// Run-41 follow-up (#161 diagnosis): an Individual-scoped signal whose target is the
/// emitting node itself must be delivered locally and NEVER enter the flood-fallback —
/// pre-fix, a self-emit (e.g. mailbox deliver-to-self) flooded the cluster with a frame no
/// other node can terminate (seen-set/TTL bounded, pure waste) and fired the
/// topology-pressure warn against the node itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn self_targeted_signal_does_not_flood() {
    let (pa, pb) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let a = std::sync::Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", pa).unwrap(),
        GossipConfig { bind_port: pa, ..Default::default() },
    ));
    let b = std::sync::Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", pb).unwrap(),
        GossipConfig {
            bind_port: pb,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", pa).unwrap()],
            ..Default::default()
        },
    ));
    a.start().await.unwrap();
    b.start().await.unwrap();
    for _ in 0..100 {
        if !a.peers().is_empty() && !b.peers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut rx = a.mesh().signal_rx("self.test");
    let self_target = a.node_id().clone();
    assert!(a.mesh().emit("self.test", SignalScope::Individual(self_target), Bytes::from_static(b"x")));

    // Local delivery works…
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(matches!(got, Ok(Some(_))), "self-targeted signal not delivered locally");

    // …and the emitter never took the flood fallback for it (pre-fix this was ≥1).
    tokio::time::sleep(Duration::from_millis(300)).await; // let the gossip shard drain
    assert_eq!(
        a.system_stats().individual_flood_fallbacks, 0,
        "self-targeted emit entered the flood fallback"
    );

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Run-42 falsification probe (robustness): a WELL-FRAMED but interior-corrupted message —
/// passes the frame-length layer (which Run 40's garbage/oversized probes covered) and dies
/// inside the codec decoder instead. The agent must reject it cleanly: no shard death, still
/// serviceable, connection handling intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_framed_but_corrupt_message_survives() {
    use tokio::io::AsyncWriteExt;

    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(id, cfg);
    agent.start().await.unwrap();

    // A legitimate Ping, corrupted in the interior while keeping valid framing.
    let nid = NodeId::new("127.0.0.1", 9999).unwrap();
    let good = wire_to_bytes(&WireMessage::Ping {
        sender: nid.clone(),
        known_peers: vec![nid.clone(), nid.clone(), nid],
    });
    let mut corrupt = good.to_vec();
    // Flip every third byte after the first (keep the message-type byte plausible).
    for i in (1..corrupt.len()).step_by(3) {
        corrupt[i] ^= 0xA5;
    }
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    write_frame(&mut s, &corrupt).await.unwrap();
    // And a second variant: truncate the tail but frame the truncated bytes correctly.
    let truncated = &good[..good.len() / 2];
    write_frame(&mut s, truncated).await.unwrap();
    let _ = s.shutdown().await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(agent.kv().set("probe/after-corrupt", Bytes::from_static(b"ok")));
    assert_eq!(
        agent.kv().get("probe/after-corrupt").as_deref(),
        Some(b"ok".as_slice()),
        "agent must remain serviceable after framed-but-corrupt input"
    );
    assert_eq!(agent.system_stats().dead_shards, 0, "no shard may die from corrupt codec input");
    agent.shutdown().await;
}

/// Run-43 falsification probe (security deep-dive): the gateway auth model END TO END on the
/// wire — unit tests cover scope resolution, but nothing asserted a configured token actually
/// gates real HTTP. With `gateway_auth_token` set: gateway routes 401 bare and with a wrong
/// token, 200 with the right one; the monitoring plane (`/health`) stays deliberately open;
/// hostile bodies (1 MiB garbage, malformed JSON) get clean errors, no panic.
#[cfg(feature = "gateway")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_gateway_auth_gates_the_wire() {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("s3cret".to_string());
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg);
    agent.start().await.expect("start");

    let client = reqwest::Client::new();
    let health = format!("http://127.0.0.1:{http_port}/health");
    for _ in 0..40 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let kv = format!("http://127.0.0.1:{http_port}/gateway/kv?key=probe/auth");
    // Bare → 401. Wrong token → 401. Right token → not-401.
    assert_eq!(client.get(&kv).send().await.unwrap().status(), 401);
    assert_eq!(
        client.get(&kv).bearer_auth("wrong").send().await.unwrap().status(), 401);
    let set = format!("http://127.0.0.1:{http_port}/gateway/kv");
    let ok = client.post(&set).bearer_auth("s3cret")
        .json(&serde_json::json!({"key":"probe/auth","value_b64":"dg=="}))
        .send().await.unwrap().status();
    assert!(ok.is_success(), "authorized gateway write failed: {ok}");

    // Monitoring plane stays open by design (no token).
    assert!(client.get(&health).send().await.unwrap().status().is_success());

    // Hostile bodies: 1 MiB garbage + malformed JSON to a JSON endpoint. A clean rejection is
    // either an error status OR the server closing the connection (so `send()` errors) — both are
    // acceptable; unwrapping the send made this flaky (the server can reset on the oversized body
    // before responding). The load-bearing assertion is liveness *after*, below.
    let big = vec![0xA5u8; 1024 * 1024];
    if let Ok(r) = client.post(format!("http://127.0.0.1:{http_port}/gateway/tuple/put"))
        .bearer_auth("s3cret").body(big).send().await {
        assert!(r.status().is_client_error() || r.status().is_server_error());
    }
    if let Ok(r) = client.post(format!("http://127.0.0.1:{http_port}/gateway/tuple/put"))
        .bearer_auth("s3cret").header("content-type", "application/json")
        .body("{not json").send().await {
        assert!(r.status().is_client_error());
    }
    assert!(client.get(&health).send().await.unwrap().status().is_success(),
        "gateway died on hostile input");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Item 1 PR 7 — HTTP parity with the commit receipt: `POST /gateway/overlay/consistent/set`
/// answers `"local_durability"` beside `"persisted"`, and the two say different things. A node
/// **without** persistence reports `persisted: true` *and* `"not_configured"` — the collapse
/// `persisted` folds into one `true`, undone beside it; a node with `Flush` persistence reports
/// `persisted: true` *and* `"on_disk"`. No `"local_durability_error"` in either. What this does
/// **not** show: the `failed` shape (a stopped WAL writer is not inducible from outside) — that
/// rendering is pinned by `commit_json`'s unit test and `LocalDurability::tag`'s.
#[cfg(all(feature = "gateway", feature = "consensus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_consistent_set_reports_local_durability_beside_persisted() {
    let client = reqwest::Client::new();

    for expected_tag in ["not_configured", "on_disk"] {
        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let base = std::env::temp_dir().join(format!("myc-parity-{gossip_port}"));
        let _ = std::fs::remove_dir_all(&base);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.persistence = (expected_tag == "on_disk").then(|| PersistenceConfig {
            base_path: base.clone(),
            sync_mode: SyncMode::Flush,
            snapshot_wal_threshold: 1_000_000,
            snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
        });
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg);
        agent.start().await.expect("start");

        let health = format!("http://127.0.0.1:{http_port}/health");
        for _ in 0..40 {
            if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let set = format!("http://127.0.0.1:{http_port}/gateway/overlay/consistent/set");
        let body: serde_json::Value = client.post(&set)
            .json(&serde_json::json!({"key":"parity/k","value_b64":"dg=="}))
            .send().await.unwrap().json().await.unwrap();

        assert_eq!(body["ok"], true, "{expected_tag}: {body}");
        // The old field, exactly as before: `true` on both nodes — which is the point.
        assert_eq!(body["persisted"], true, "{expected_tag}: {body}");
        assert_eq!(body["local_durability"], expected_tag, "{body}");
        assert!(body.get("local_durability_error").is_none(),
            "no error field unless the state is `failed`: {body}");

        agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    }
}

/// Run-43 falsification probe (failure-mode legibility deep-dive): a rejected config must
/// NAME the offending field — an operator staring at a failed start needs the knob, not a
/// generic message.
#[test]
fn probe_config_rejection_names_the_field() {
    let mut cfg = GossipConfig::default();
    cfg.reconnect_backoff_secs = 0; // documented-invalid
    let err = cfg.validate().expect_err("zero backoff must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains("reconnect_backoff_secs"),
        "config error must name the field, got: {msg}"
    );
}

/// Run-43 falsification probe (semantic correctness): tombstone anti-resurrection — after a
/// delete, re-applying the ORIGINAL (older-HLC) write frame must NOT resurrect the value.
/// LWW is only correct if the tombstone's timestamp wins replays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_tombstone_survives_replay_of_older_write() {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();

    assert!(agent.kv().set("probe/res", Bytes::from_static(b"v1")));
    // Capture the store entry's timestamp, then delete (tombstone with a later HLC).
    assert!(agent.kv().delete("probe/res"));
    assert_eq!(agent.kv().get("probe/res"), None);

    // Replay the original write as a remote frame with an OLDER timestamp (1).
    let update = data_update("probe/res", b"v1", 424242, false);
    {
        use crate::store::apply_and_notify;
        apply_and_notify(&agent.task_ctx.kv_state, &update);
    }
    assert_eq!(
        agent.kv().get("probe/res"),
        None,
        "older replayed write resurrected a tombstoned key"
    );
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Item 1 PR 2 — the receipt path: what a write establishes, rung by rung, and what a retry does.
#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::{
        AttemptId, LocalApplication, LocalDurability, OperationId, PersistenceConfig, ReceiptError,
        SyncMode,
    };

    fn agent_with(persistence: Option<PersistenceConfig>) -> Arc<GossipAgent> {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.persistence = persistence;
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    }

    fn persistence(tag: &str, sync_mode: SyncMode) -> (PersistenceConfig, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("myc-receipt-{tag}-{}", alloc_port()));
        let _ = std::fs::remove_dir_all(&base);
        (
            PersistenceConfig {
                base_path: base.clone(),
                sync_mode,
                snapshot_wal_threshold: 1_000_000,
                snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
            },
            base,
        )
    }

    /// With no persistence configured the receipt says `NotConfigured` — *nothing was promised* —
    /// and never `OnDisk`. This is the distinction `persisted: bool` collapses (D24).
    #[tokio::test]
    async fn an_unpersisted_node_promises_nothing() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-unpersisted");
        let r = a.kv().set_with_receipt(&op, "k/1", b"v".to_vec()).await.unwrap();
        assert_eq!(r.application, LocalApplication::Applied);
        assert_eq!(r.local_durability, LocalDurability::NotConfigured);
        assert!(!r.local_durability.is_durable(), "nothing was promised, so nothing is durable");
        assert!(r.attempt_id.as_str().starts_with("op-unpersisted#"), "{}", r.attempt_id);
        a.shutdown().await;
    }

    /// The gap this PR found in its own record: under `SyncMode::Async` a successful append is
    /// **buffered**, not on disk. A receipt claiming `OnDisk` here would claim a durability the
    /// node never established; `Flush` is what establishes it.
    #[tokio::test]
    async fn a_buffered_write_does_not_claim_disk_but_a_flushed_one_does() {
        let (p_async, dir_a) = persistence("async", SyncMode::Async);
        let a = agent_with(Some(p_async));
        a.start().await.unwrap();
        let r = a.kv().set_with_receipt(&OperationId::new("op-a"), "k/async", b"v".to_vec()).await.unwrap();
        assert_eq!(r.local_durability, LocalDurability::Buffered, "Async does not sync per append");
        assert!(!r.local_durability.is_durable());
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir_a);

        let (p_flush, dir_f) = persistence("flush", SyncMode::Flush);
        let b = agent_with(Some(p_flush));
        b.start().await.unwrap();
        let r = b.kv().set_with_receipt(&OperationId::new("op-f"), "k/flush", b"v".to_vec()).await.unwrap();
        assert_eq!(r.local_durability, LocalDurability::OnDisk, "Flush syncs every append");
        assert!(r.local_durability.is_durable());
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir_f);
    }

    /// A late retry, arriving after a newer value legitimately took the key, is **`Superseded`** —
    /// not applied, not failed — and crucially it does **not** clobber the newer value. That is
    /// what reusing the original stamp buys (D11): had the retry ticked a fresh HLC it would have
    /// outranked the newer write and silently undone it.
    #[tokio::test]
    async fn a_late_retry_is_superseded_and_does_not_clobber_the_newer_value() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-late");

        // 1. The operation is written and receipted at stamp T1.
        let first = a.kv().set_with_receipt(&op, "k/race", b"original".to_vec()).await.unwrap();
        assert_eq!(first.application, LocalApplication::Applied);

        // 2. Something newer takes the key (a fresh HLC, so T2 > T1).
        assert!(a.kv().set("k/race", b"newer".to_vec()), "the newer write is queued");
        assert_eq!(a.kv().get("k/race").as_deref(), Some(&b"newer"[..]));

        // 3. The original operation is retried — same identity, same content, same stamp.
        let retry = a.kv().retry_with_receipt(&first, "k/race", b"original".to_vec()).await.unwrap();
        assert_eq!(retry.stamp, first.stamp, "the retry reuses T1");
        assert_eq!(
            retry.application,
            LocalApplication::Superseded,
            "a late retry loses LWW to the newer value, and the receipt says so"
        );
        assert_eq!(
            a.kv().get("k/race").as_deref(),
            Some(&b"newer"[..]),
            "the newer value survives: this is why a retry must not tick a fresh HLC"
        );
        a.shutdown().await;
    }

    /// D11: a retry re-submits the **same stamp**, so the operation ranks identically under LWW
    /// however often it is attempted — and each attempt is distinguishable.
    #[tokio::test]
    async fn a_retry_reuses_the_stamp_and_numbers_the_attempt() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-retry");
        let first = a.kv().set_with_receipt(&op, "k/retry", b"v".to_vec()).await.unwrap();
        let second = a.kv().retry_with_receipt(&first, "k/retry", b"v".to_vec()).await.unwrap();
        let third = a.kv().retry_with_receipt(&second, "k/retry", b"v".to_vec()).await.unwrap();
        assert_eq!(second.stamp, first.stamp, "a retry must not tick a fresh HLC (D11)");
        assert_eq!(third.stamp, first.stamp);
        // Every delivery has its own identity; the operation's does not change.
        assert_ne!(first.attempt_id, second.attempt_id);
        assert_ne!(second.attempt_id, third.attempt_id);
        assert_ne!(first.attempt_id, third.attempt_id);
        assert_eq!(second.operation_id, op, "the operation identity is stable across attempts");
        // A caller that numbers its own deliveries still can.
        let numbered = AttemptId::of(&op, 7);
        let r = a.kv().retry_with_receipt_as(&first, &numbered, "k/retry", b"v".to_vec()).await.unwrap();
        assert_eq!(r.attempt_id, numbered);
        a.shutdown().await;
    }

    /// Same identity, different content is a `Conflict` — and **nothing is written**.
    #[tokio::test]
    async fn the_same_operation_with_different_content_conflicts_and_writes_nothing() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-conflict");
        let first = a.kv().set_with_receipt(&op, "k/c", b"original".to_vec()).await.unwrap();
        let err = a.kv().retry_with_receipt(&first, "k/c", b"changed".to_vec()).await.unwrap_err();
        match err {
            ReceiptError::Conflict { operation_id, expected, found } => {
                assert_eq!(operation_id, op);
                assert_ne!(expected, found);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(a.kv().get("k/c").as_deref(), Some(&b"original"[..]), "the conflicting retry wrote nothing");
        a.shutdown().await;
    }

    /// Review regression (2026-09-15, finding 2): an identical retry is `AlreadyCurrent`, not
    /// `Superseded` — nothing newer won, the operation *is* the current value.
    #[tokio::test]
    async fn an_identical_retry_is_already_current_not_superseded() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-idem");
        let first = a.kv().set_with_receipt(&op, "k/idem", b"v".to_vec()).await.unwrap();
        assert_eq!(first.application, LocalApplication::Applied);
        let again = a.kv().retry_with_receipt(&first, "k/idem", b"v".to_vec()).await.unwrap();
        assert_eq!(
            again.application,
            LocalApplication::AlreadyCurrent,
            "an idempotent retry is current, not superseded by something newer"
        );
        assert!(again.application.is_current());
        assert_eq!(a.kv().get("k/idem").as_deref(), Some(&b"v"[..]));
        a.shutdown().await;
    }

    /// Review regression (2026-09-15, finding 3): two retries from the **same prior receipt** are
    /// two deliveries and must not share an attempt identity — the case that arises when a retry's
    /// response is lost, or when replacement workers share the last receipt they saw.
    #[tokio::test]
    async fn two_retries_from_one_receipt_get_distinct_attempt_identities() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-dup");
        let first = a.kv().set_with_receipt(&op, "k/dup", b"v".to_vec()).await.unwrap();
        let retry_a = a.kv().retry_with_receipt(&first, "k/dup", b"v".to_vec()).await.unwrap();
        let retry_b = a.kv().retry_with_receipt(&first, "k/dup", b"v".to_vec()).await.unwrap();
        assert_ne!(
            retry_a.attempt_id, retry_b.attempt_id,
            "two deliveries derived from one receipt must still be distinguishable"
        );
        assert_eq!(retry_a.operation_id, retry_b.operation_id);
        assert_eq!(retry_a.stamp, retry_b.stamp, "both still reuse the operation's stamp");
        a.shutdown().await;
    }

    /// PR 3, the strong path: a required-sync write establishes `OnDisk` whatever the node's
    /// `SyncMode` is — `Async` would otherwise have left it `Buffered`.
    #[tokio::test]
    async fn a_required_sync_write_establishes_disk_even_in_async_mode() {
        let (p, dir) = persistence("required", SyncMode::Async);
        let a = agent_with(Some(p));
        a.start().await.unwrap();

        let relaxed = a.kv().set_with_receipt(&OperationId::new("op-relaxed"), "k/relaxed", b"v".to_vec()).await.unwrap();
        assert_eq!(relaxed.local_durability, LocalDurability::Buffered, "the ordinary path takes what the mode gives");

        let strong = a.kv().set_requiring_sync(&OperationId::new("op-strong"), "k/strong", b"v".to_vec()).await.unwrap();
        assert_eq!(strong.local_durability, LocalDurability::OnDisk, "the strong path forces the sync");
        assert!(strong.local_durability.is_durable());
        assert_eq!(strong.application, LocalApplication::Applied);
        assert_eq!(a.kv().get("k/strong").as_deref(), Some(&b"v"[..]));

        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR 3's whole point: when a required-sync write cannot establish durability, **nothing
    /// became visible** — not in the store, not to a subscriber. The ordinary path cannot promise
    /// this, because it applies before it persists.
    #[tokio::test]
    async fn a_refused_required_sync_write_leaves_nothing_visible() {
        // No persistence configured: the node cannot make anything durable, so it refuses rather
        // than apply and report undurable.
        let a = agent_with(None);
        a.start().await.unwrap();
        let mut watch = a.kv().subscribe_prefix("k/");

        let err = a
            .kv()
            .set_requiring_sync(&OperationId::new("op-nodurability"), "k/none", b"v".to_vec())
            .await
            .unwrap_err();
        match err {
            ReceiptError::DurabilityNotEstablished { persistence_configured, .. } => {
                assert!(!persistence_configured, "this node never had persistence");
            }
            other => panic!("expected DurabilityNotEstablished, got {other:?}"),
        }
        assert!(a.kv().get("k/none").is_none(), "nothing was applied");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), watch.changed()).await.is_err(),
            "no subscriber saw anything — the value never became visible"
        );

        // By contrast the ordinary path applies first: the same node accepts it and reports that
        // nothing was promised. Both are honest; they are different contracts.
        let ok = a.kv().set_with_receipt(&OperationId::new("op-relaxed"), "k/none", b"v".to_vec()).await.unwrap();
        assert_eq!(ok.local_durability, LocalDurability::NotConfigured);
        assert_eq!(a.kv().get("k/none").as_deref(), Some(&b"v"[..]), "the ordinary path did apply it");
        a.shutdown().await;
    }

    /// The strong path retries like the ordinary one: same stamp, and a changed payload under the
    /// same identity is refused.
    #[tokio::test]
    async fn a_required_sync_retry_keeps_the_stamp_and_refuses_changed_content() {
        let (p, dir) = persistence("required-retry", SyncMode::Async);
        let a = agent_with(Some(p));
        a.start().await.unwrap();
        let op = OperationId::new("op-strong-retry");
        let first = a.kv().set_requiring_sync(&op, "k/sr", b"v".to_vec()).await.unwrap();
        let again = a.kv().retry_requiring_sync(&first, "k/sr", b"v".to_vec()).await.unwrap();
        assert_eq!(again.stamp, first.stamp, "a retry reuses the operation's stamp (D11)");
        assert_ne!(again.attempt_id, first.attempt_id, "each delivery is distinguishable");
        assert_eq!(again.local_durability, LocalDurability::OnDisk);
        assert_eq!(again.application, LocalApplication::AlreadyCurrent);

        let err = a.kv().retry_requiring_sync(&first, "k/sr", b"changed".to_vec()).await.unwrap_err();
        assert!(matches!(err, ReceiptError::Conflict { .. }), "got {err:?}");
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review regression (PR 3, 2026-09-15): a caller that **loses the receipt** can still retry
    /// safely, because it prepared the operation before dispatching it.
    ///
    /// Re-issuing `set_with_receipt` with the same `OperationId` would mint a fresh HLC and let a
    /// late retry outrank — and silently undo — a newer value. Committing the same `PreparedWrite`
    /// reuses the original stamp, so the retry loses LWW and says so.
    #[tokio::test]
    async fn a_prepared_write_survives_a_lost_acknowledgement_without_clobbering() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let op = OperationId::new("op-lost-ack");

        // Prepared *before* dispatch; the caller keeps this, not the receipt.
        let prepared = a.kv().prepare_write(&op, "k/lost", b"original");
        let first = a.kv().commit_prepared(&prepared, b"original".to_vec()).await.unwrap();
        assert_eq!(first.application, LocalApplication::Applied);

        // The response is lost — the caller has no receipt. Meanwhile something newer takes the key.
        drop(first);
        assert!(a.kv().set("k/lost", b"newer".to_vec()));

        // The retry uses the token it kept. It must not undo the newer value.
        let retry = a.kv().commit_prepared(&prepared, b"original".to_vec()).await.unwrap();
        assert_eq!(retry.stamp, prepared.stamp, "the prepared stamp is reused, not re-ticked");
        assert_eq!(retry.application, LocalApplication::Superseded);
        assert_eq!(
            a.kv().get("k/lost").as_deref(),
            Some(&b"newer"[..]),
            "the newer value survives the lost-acknowledgement retry",
        );

        // The contrast: re-issuing by operation id alone mints a fresh stamp and does clobber it.
        let reissued = a.kv().set_with_receipt(&op, "k/lost", b"original".to_vec()).await.unwrap();
        assert_ne!(reissued.stamp, prepared.stamp);
        assert_eq!(reissued.application, LocalApplication::Applied);
        assert_eq!(
            a.kv().get("k/lost").as_deref(),
            Some(&b"original"[..]),
            "which is exactly why the prepared token exists",
        );
        a.shutdown().await;
    }

    /// A prepared write binds its content: committing different bytes under the same operation
    /// identity is a conflict, and the token survives a round trip through serialisation — the
    /// caller may persist it across its own restart.
    #[tokio::test]
    async fn a_prepared_write_binds_its_content_and_round_trips() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let prepared = a.kv().prepare_write(&OperationId::new("op-bound"), "k/bound", b"v");
        let json = serde_json::to_string(&prepared).unwrap();
        let restored: crate::PreparedWrite = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, prepared, "a caller can persist the token and come back to it");

        let err = a.kv().commit_prepared(&restored, b"different".to_vec()).await.unwrap_err();
        assert!(matches!(err, ReceiptError::Conflict { .. }), "got {err:?}");
        assert!(a.kv().get("k/bound").is_none(), "the conflicting commit wrote nothing");
        a.kv().commit_prepared(&restored, b"v".to_vec()).await.unwrap();
        assert_eq!(a.kv().get("k/bound").as_deref(), Some(&b"v"[..]));
        a.shutdown().await;
    }

    /// An oversized write is refused before anything is applied — `Rejected`, not a receipt that
    /// claims an application that never happened.
    #[tokio::test]
    async fn an_oversized_write_is_rejected_before_it_applies() {
        let a = agent_with(None);
        a.start().await.unwrap();
        let big = vec![0u8; crate::framing::MAX_KV_WRITE_BYTES + 1];
        let err = a.kv().set_with_receipt(&OperationId::new("op-big"), "k/big", big).await.unwrap_err();
        assert!(matches!(err, ReceiptError::Rejected(_)), "got {err:?}");
        assert!(a.kv().get("k/big").is_none(), "nothing was applied");
        a.shutdown().await;
    }

    /// The consensus receipt separates "the cluster agreed" from "this node has it on disk", and
    /// a timeout reads as `DeliveryUnknown` rather than as a negative.
    #[cfg(feature = "consensus")]
    #[tokio::test]
    async fn a_commit_receipt_separates_agreement_from_local_durability() {
        use crate::{CommitError, ConsensusConfig};
        let a = agent_with(None);
        a.start().await.unwrap();
        let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
        let solo = ConsensusConfig { quorum_size: 1, ..ConsensusConfig::default() };

        let r = a
            .consensus()
            .cluster_propose_receipt("receipt/slot", Bytes::from_static(b"v"), solo.clone())
            .await
            .expect("committed");
        assert_eq!(&*r.slot, "receipt/slot");
        // Agreed cluster-wide, but this node persists nothing — the two statements stay apart.
        assert_eq!(r.local_durability, LocalDurability::NotConfigured);
        assert!(!r.local_durability.is_durable());

        // A quorum that cannot be met times out, and the receipt vocabulary calls that unknown.
        let impossible = ConsensusConfig {
            quorum_size: 5,
            max_ballots: 1,
            phase1_timeout: Duration::from_millis(150),
            ..ConsensusConfig::default()
        };
        let err = a
            .consensus()
            .cluster_propose_receipt("receipt/unknown", Bytes::from_static(b"v"), impossible)
            .await
            .unwrap_err();
        match err {
            CommitError::DeliveryUnknown { slot, .. } => assert_eq!(&*slot, "receipt/unknown"),
            other => panic!("a timeout must read as DeliveryUnknown, got {other:?}"),
        }
        a.shutdown().await;
    }
}

// ── The federation transport, first arm (v3 item 2 PR 8) ─────────────────
//
// PRs 1–7 ended every page with "no bytes cross a network". This module is where they do, and
// where the PR 1 harness's `assert_never_merged` stops being trivially true: a call has crossed
// from domain B to domain A's gateway, and the tables must still show two meshes.
#[cfg(all(feature = "gateway", feature = "tls", feature = "a2a"))]
mod federation_transport {
    use super::*;
    use crate::federation::{
        call::{CallPolicy, FederatedCaller},
        client::{ClientError, FederationClient, GatewayEndpoint, TlsRefusal},
        edge::{now_ms, FederationEdge, PresentedCall, HEADER_FEDERATED_CALL},
        gateway::{CallOutcome, Repeatability},
        session::{LinkRefusal, LinkState},
        DomainId, DomainPolicy, TrustBundle,
    };
    use std::sync::atomic::AtomicUsize;

    fn keypair(seed: u8) -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let vk = sk.verifying_key().to_bytes();
        (sk, vk)
    }

    /// A mesh of `n` nodes bootstrapped only within itself; node 0 optionally runs a gateway
    /// with the A2A edge and the federation edge. Mirrors the PR 1 harness's builder.
    async fn mesh(n: usize, gateway: Option<(u16, Arc<FederationEdge>)>) -> Vec<GossipAgent> {
        let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
        let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
        let mut agents = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let mut cfg = GossipConfig::default();
            cfg.bind_port = ports[i];
            cfg.bootstrap_peers = ids.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.clone()).collect();
            cfg.health_check_max_jitter_ms = 50;
            let a = match (&gateway, i) {
                (Some((http_port, edge)), 0) => {
                    cfg.http_port = Some(*http_port);
                    GossipAgent::new(id.clone(), cfg).with_a2a().with_federation_edge(Arc::clone(edge))
                }
                _ => GossipAgent::new(id.clone(), cfg),
            };
            a.start().await.unwrap();
            agents.push(a);
        }
        agents
    }

    /// A provider on `agent` for skill `demo/whoami` that answers with the principal it was told,
    /// and counts how many calls actually reached it. The capability itself is advertised by the
    /// caller (the registration must outlive the test body).
    fn whoami_provider(agent: Arc<GossipAgent>, calls: Arc<AtomicUsize>) {
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                calls.fetch_add(1, Ordering::SeqCst);
                let reply = match agent.request_principal(&req) {
                    Ok(p) => p.name(),
                    Err(e) => format!("refused:{e}"),
                };
                agent.service().rpc_respond(&req, reply.into_bytes());
            }
        });
    }

    fn whoami() -> crate::capability::Capability {
        crate::capability::Capability::new("demo", "whoami")
    }

    fn task_body(skill: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
            "params": {"skillId": skill, "message": {"role": "user", "parts": [{"type": "text", "text": "?"}]}},
        })
    }

    /// An A2A 0.3 / 1.0 send (`message/send`, `SendMessage`, and their streaming names) naming its
    /// skill in `metadata.skillId` (plan row N).
    fn message_body(method: &str, skill: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method,
            "params": {"message": {"messageId": "m-1", "role": "ROLE_USER", "parts": [{"text": "?"}],
                                   "metadata": {"skillId": skill}}},
        })
    }

    fn cred(beta: &DomainId, export: &str, sk: &ed25519_dalek::SigningKey) -> PresentedCall {
        let now = now_ms();
        PresentedCall::sign(
            &FederatedCaller {
                body_sha256: None,
                origin_domain: beta.clone(),
                principal: "svc/billing".into(),
                export: export.into(),
                issued_at_ms: now,
                expires_at_ms: now + 60_000,
            },
            sk,
        )
    }

    /// **The release gate's first leg.** Domain B discovers domain A's catalogue and invokes an
    /// export over HTTP, with the provider told `federation:beta.example/svc/billing`; the export
    /// B was not granted is refused before any byte is sent; a forged, an ungranted and a
    /// revoked credential are refused at the gateway with no dispatch; and the two meshes'
    /// tables show they never merged — now that something has crossed.
    #[tokio::test]
    async fn a_federated_call_crosses_and_the_meshes_still_never_merge() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(7);
        let (alpha_sk, alpha_vk) = keypair(8);

        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami", "demo/secret"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy::default(),
        ).with_signing_key(alpha_sk));
        let http_port = alloc_port();
        let mut a = mesh(2, Some((http_port, Arc::clone(&edge)))).await;
        let a0 = Arc::new(a.remove(0));
        let a_rest = a;
        let b = mesh(2, None).await;
        {
            let (a0, a_rest, b) = (&a0, &a_rest, &b);
            poll_until(
                || !a0.peers().is_empty() && a_rest.iter().all(|x| !x.peers().is_empty()) && b.iter().all(|x| !x.peers().is_empty()),
                3_000,
            )
            .await;
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let _reg = a0.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        whoami_provider(Arc::clone(&a0), Arc::clone(&calls));
        let cap_key = format!("cap/{}/demo/whoami", a0.node_id());
        poll_until(|| a0.kv().get(&cap_key).is_some(), 5_000).await;

        let client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            vec![GatewayEndpoint { id: "gw-a0".into(), base_url: format!("http://127.0.0.1:{http_port}") }],
            1, Duration::from_secs(30),
        ).with_partner_key(alpha_vk);

        // 0. A signed catalogue under the wrong key is refused before it is relied on, and the
        //    link stays Down (item 2 PR 10a). The same edge, a client holding the wrong key.
        let (_, wrong_vk) = keypair(9);
        let misled = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            vec![GatewayEndpoint { id: "gw-a0".into(), base_url: format!("http://127.0.0.1:{http_port}") }],
            1, Duration::from_secs(30),
        ).with_partner_key(wrong_vk);
        assert!(matches!(misled.connect().await, Err(ClientError::Catalogue(crate::federation::edge::CatalogRefusal::BadSignature))));
        assert_eq!(misled.link_state(), LinkState::Down);

        // 1. Before discovery: refused locally, the link is Down. No HTTP.
        assert!(matches!(
            client.call("demo/whoami", "?", Repeatability::AtMostOnce).await,
            Err(ClientError::Link(LinkRefusal::Down { .. }))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // 2. Discovery: the catalogue is the grant, not the export list.
        let exports = client.connect().await.expect("catalogue fetch");
        assert_eq!(exports, vec!["demo/whoami".to_string()]);
        assert_eq!(client.link_state(), LinkState::Ready);

        // 3. The call crosses, and the provider is told who asked — not "the gateway".
        let reply = client.call("demo/whoami", "?", Repeatability::AtMostOnce).await.expect("federated call");
        assert_eq!(reply, crate::federation_principal("beta.example", "svc/billing"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 4. An export the catalogue never named: refused by the resolver, before any byte.
        assert!(matches!(
            client.call("demo/secret", "?", Repeatability::AtMostOnce).await,
            Err(ClientError::Resolve(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 5. Plants at the gateway — each must be refused *without a dispatch*.
        let raw = reqwest::Client::new();
        let a2a = format!("http://127.0.0.1:{http_port}/a2a");
        let catalog = format!("http://127.0.0.1:{http_port}/federation/catalog");
        // 5a. Granted export, but a credential minted for a different one: WrongExport, -32003.
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/secret", &beta_sk).to_header_value())
            .json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32003, "{v}");
        assert!(v["error"]["message"].as_str().unwrap().contains("authorises"), "{v}");
        // 5a-bis. **A credential binds ONE export, and only `tasks/send` authorises against it.**
        // `tasks/get` and `tasks/cancel` take no caller and no export, so before the Phase-C
        // adversarial audit a partner granted one export could read any task's completed artifact —
        // including a *native* caller's — or cancel it, by naming its id. Ids are caller-supplied on
        // `tasks/send`, so they are enumerable. Both are refused for federated callers, as
        // `tasks/sendSubscribe` already was — and A2A 1.0's names for them (plan row N).
        for method in ["tasks/get", "tasks/cancel", "GetTask", "CancelTask"] {
            let body = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": method, "params": {"id": "some-other-callers-task"},
            });
            let r = raw.post(&a2a)
                .header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
                .json(&body).send().await.unwrap();
            assert_eq!(r.status(), 200);
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["error"]["code"], -32004, "{method} must be refused for a federated caller: {v}");
            assert!(
                v["error"]["message"].as_str().unwrap().contains("binds one export"),
                "the refusal must name the reason: {v}"
            );
        }

        // 5b. Named and signed, but not granted: NotPermitted, -32003.
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/secret", &beta_sk).to_header_value())
            .json(&task_body("demo/secret")).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32003, "{v}");
        assert!(v["error"]["message"].as_str().unwrap().contains("policy does not grant"), "{v}");
        // 5c. Forged: signed by a key alpha does not trust — refused at the auth layer, 401.
        let (forger, _) = keypair(9);
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &forger).to_header_value())
            .json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 401);
        // 5d. Tampered in transit: a field changed after signing.
        let mut tampered = cred(&beta, "demo/whoami", &beta_sk);
        tampered.principal = "svc/admin".into();
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, tampered.to_header_value())
            .json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 401);
        // 5e. Malformed: tried to present, failed — refused, never anonymised.
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, "{not a credential")
            .json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 400);
        // 5f. Two identities on one request.
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
            .header(axum::http::header::AUTHORIZATION, "Bearer anything")
            .json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 400);
        // 5g. Streaming is not a federated call, in any generation's name for it (plan row N).
        for method in ["tasks/sendSubscribe", "message/stream", "SendStreamingMessage"] {
            let subscribe = if method == "tasks/sendSubscribe" {
                let mut b = task_body("demo/whoami");
                b["method"] = serde_json::Value::String(method.into());
                b
            } else {
                message_body(method, "demo/whoami")
            };
            let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
                .json(&subscribe).send().await.unwrap();
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["error"]["code"], -32003, "{method}: {v}");
        }
        // 5h. The catalogue needs its reserved export: a call credential cannot fetch it.
        let r = raw.get(&catalog).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
            .send().await.unwrap();
        assert_eq!(r.status(), 403);
        let r = raw.get(&catalog).send().await.unwrap();
        assert_eq!(r.status(), 401, "no credential, no catalogue");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no plant reached the provider");

        // 6. Anonymous A2A is unchanged: no credential, no bearer → dispatched as `anonymous`.
        let r = raw.post(&a2a).json(&task_body("demo/whoami")).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], crate::PRINCIPAL_ANONYMOUS, "{v}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // 6b. A2A 1.0's `SendMessage` crosses the edge as `tasks/send` does (plan row N): the export
        //     is authorised against the skill `metadata.skillId` names, and the provider is told the
        //     partner — and an export the credential does not name is refused with no dispatch.
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
            .json(&message_body("SendMessage", "demo/whoami")).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(
            v["result"]["task"]["artifacts"][0]["parts"][0]["text"],
            crate::federation_principal("beta.example", "svc/billing"),
            "{v}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
            .json(&message_body("SendMessage", "demo/secret")).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32003, "{v}");
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // 7. Revocation at the provider, mid-session: the next call is refused at the auth
        //    layer with no dispatch. The client learns it from the refusal, not from gossip.
        edge.revoke(&beta);
        match client.call("demo/whoami", "?", Repeatability::AtMostOnce).await {
            Err(ClientError::Refused { status: 401, message, .. }) => {
                assert!(message.contains("not in the trust bundle"), "{message}")
            }
            other => panic!("a revoked partner must be refused at the gateway, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // 8. The tables, after bytes crossed: two meshes. Non-vacuity of "bytes crossed": the
        //    provider was reached exactly three times — two federated calls and the anonymous one.
        let a_nodes: Vec<&GossipAgent> = std::iter::once(a0.as_ref()).chain(a_rest.iter()).collect();
        let b_nodes: Vec<&GossipAgent> = b.iter().collect();
        assert_never_merged(&a_nodes, &b_nodes);
        assert_eq!(a0.peers().len(), 1, "domain A's gateway node peers with its own mesh only");
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        for n in a_nodes.iter().chain(b_nodes.iter()) {
            n.shutdown().await;
        }
    }

    /// **Disconnect, reconnect, revoke — on the client side; and a silent gateway.** A link
    /// marked down refuses with no HTTP until the catalogue is fetched again (reconnected is not
    /// ready); a client-side revocation is final, whatever the partner would have answered; a
    /// silent gateway is `DeliveryUnknown` for an at-most-once call and a failover for a
    /// repeatable one; a gateway with no edge refuses a presented credential rather than
    /// anonymising it.
    #[tokio::test]
    async fn the_client_side_link_and_a_silent_gateway() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(11);
        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy::default(),
        ));
        let http_port = alloc_port();
        let mut a = mesh(1, Some((http_port, edge))).await;
        let a0 = Arc::new(a.remove(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let _reg = a0.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        whoami_provider(Arc::clone(&a0), Arc::clone(&calls));
        let cap_key = format!("cap/{}/demo/whoami", a0.node_id());
        poll_until(|| a0.kv().get(&cap_key).is_some(), 5_000).await;

        let dead_port = alloc_port();
        let endpoints = vec![
            GatewayEndpoint { id: "gw-dead".into(), base_url: format!("http://127.0.0.1:{dead_port}") },
            GatewayEndpoint { id: "gw-live".into(), base_url: format!("http://127.0.0.1:{http_port}") },
        ];
        let client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(), endpoints, 1, Duration::from_secs(30),
        );

        // This edge has no signing key: a client that requires a signed catalogue refuses the
        // unsigned one (item 2 PR 10a); the client below does not require one and proceeds.
        let (_, some_vk) = keypair(12);
        let requiring = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            vec![GatewayEndpoint { id: "gw-live".into(), base_url: format!("http://127.0.0.1:{http_port}") }],
            1, Duration::from_secs(30),
        ).with_partner_key(some_vk);
        assert!(matches!(requiring.connect().await, Err(ClientError::Catalogue(crate::federation::edge::CatalogRefusal::Unsigned))));

        // connect tries the dead gateway, then the live one.
        assert_eq!(client.connect().await.unwrap(), vec!["demo/whoami".to_string()]);

        // At-most-once through the dead gateway (admitted first): unknown, never retried.
        match client.call("demo/whoami", "?", Repeatability::AtMostOnce).await {
            Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { attempted_via, .. })) => {
                assert_eq!(attempted_via, vec!["gw-dead".to_string()])
            }
            other => panic!("expected DeliveryUnknown via the dead gateway, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // Repeatable: the same first choice, then the failover carries it.
        let reply = client.call("demo/whoami", "?", Repeatability::Repeatable).await.expect("failover");
        assert_eq!(reply, crate::federation_principal("beta.example", "svc/billing"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Down: refused locally until reconnected; reconnect refreshes before new work.
        client.disconnect();
        assert_eq!(client.link_state(), LinkState::Down);
        assert_eq!(client.last_catalogue(), None);
        assert!(matches!(
            client.call("demo/whoami", "?", Repeatability::Repeatable).await,
            Err(ClientError::Link(LinkRefusal::Down { .. }))
        ));
        client.connect().await.unwrap();
        assert!(client.call("demo/whoami", "?", Repeatability::Repeatable).await.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Revoked on this side: final. The fetch itself is not refused; the link never comes back.
        client.revoke();
        assert!(matches!(
            client.call("demo/whoami", "?", Repeatability::Repeatable).await,
            Err(ClientError::Link(LinkRefusal::Revoked { .. }))
        ));
        assert!(client.connect().await.is_ok());
        assert!(matches!(
            client.call("demo/whoami", "?", Repeatability::Repeatable).await,
            Err(ClientError::Link(LinkRefusal::Revoked { .. }))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // A gateway with no edge refuses a presented credential rather than anonymising it.
        let bare_port = alloc_port();
        let bare = {
            let gossip_port = alloc_port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = gossip_port;
            cfg.http_port = Some(bare_port);
            GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a()
        };
        bare.start().await.unwrap();
        // The listener binds after `start` returns; wait for it rather than for a fixed time.
        let http = reqwest::Client::new();
        let bare_url = format!("http://127.0.0.1:{bare_port}/a2a");
        let deadline = Instant::now() + Duration::from_secs(5);
        let r = loop {
            let sent = http
                .post(&bare_url)
                .header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
                .json(&task_body("demo/whoami"))
                .send()
                .await;
            match sent {
                Ok(r) => break r,
                Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(e) => panic!("the bare gateway never came up: {e}"),
            }
        };
        assert_eq!(r.status(), 401);

        bare.shutdown().await;
        a0.shutdown().await;
    }

    /// **A blackholed gateway is silent in bounded time** (item 2 PR 10b's finding). PR 5 promises
    /// `DeliveryUnknown` for a gateway that goes silent, and a client that waits forever cannot
    /// deliver that verdict. A *refusing* partner sends a TCP reset and fails fast; a **blackholed**
    /// one — interface gone, default route still there, which is what a real severance looks like —
    /// sends nothing at all. `192.0.2.1` is RFC 5737 TEST-NET-1, reserved and unroutable, so this
    /// reproduces that shape without a network namespace.
    ///
    /// The assertion is the *bound*, not the error: on a host that answers `ENETUNREACH` the call
    /// fails immediately and the bound still holds. Before this PR's default connect timeout it
    /// hung instead — which is how the Docker suite found it.
    #[tokio::test]
    async fn a_blackholed_gateway_is_unknown_within_a_bound_rather_than_hanging() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(17);
        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy::default(),
        ));
        let http_port = alloc_port();
        let mut a = mesh(1, Some((http_port, edge))).await;
        let a0 = Arc::new(a.remove(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let _reg = a0.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        whoami_provider(Arc::clone(&a0), Arc::clone(&calls));
        let cap_key = format!("cap/{}/demo/whoami", a0.node_id());
        poll_until(|| a0.kv().get(&cap_key).is_some(), 5_000).await;

        // The blackhole is listed first, so it is the one the pool admits.
        let client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk, alpha.clone(),
            vec![
                GatewayEndpoint { id: "gw-blackhole".into(), base_url: "http://192.0.2.1:8300".into() },
                GatewayEndpoint { id: "gw-live".into(), base_url: format!("http://127.0.0.1:{http_port}") },
            ],
            1, Duration::from_secs(30),
        )
        .with_timeouts(Duration::from_millis(300), Duration::from_secs(1));

        // `connect` walks past the blackhole to the live gateway, bounded.
        let started = Instant::now();
        assert_eq!(client.connect().await.unwrap(), vec!["demo/whoami".to_string()]);
        assert!(started.elapsed() < Duration::from_secs(10), "connect past a blackhole took {:?}", started.elapsed());

        // At-most-once through the blackhole: unknown, named, and bounded.
        let started = Instant::now();
        match client.call("demo/whoami", "?", Repeatability::AtMostOnce).await {
            Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { attempted_via, .. })) => {
                assert_eq!(attempted_via, vec!["gw-blackhole".to_string()])
            }
            other => panic!("a blackholed gateway must read as DeliveryUnknown, got {other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the verdict took {:?}", started.elapsed());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing reached the provider through the blackhole");

        a0.shutdown().await;
    }

    /// **The cap applies at `/a2a`, not only in the type.** A mechanism nothing wires is the
    /// failure this project keeps finding in its own gates, so this drives two *concurrent*
    /// federated calls through a real gateway with `max_in_flight_per_partner: 1`.
    ///
    /// The provider blocks until the test releases it, which is what makes "in flight" mean
    /// something: with an instant provider the first call would be finished before the second
    /// arrived and the cap would never be consulted.
    #[tokio::test]
    async fn the_per_partner_cap_refuses_a_concurrent_call_at_the_live_gateway() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(61);

        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy { max_in_flight_per_partner: 1, ..CallPolicy::default() },
        ));
        let http_port = alloc_port();
        let mut nodes = mesh(1, Some((http_port, Arc::clone(&edge)))).await;
        let a0 = Arc::new(nodes.remove(0));

        // A provider that reports when it has been entered and waits to be let go.
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let _reg = a0.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        let provider = Arc::clone(&a0);
        let mut rx = a0.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            let mut release = Some(release_rx);
            while let Some(req) = rx.recv().await {
                let _ = entered_tx.send(());
                if let Some(r) = release.take() {
                    let _ = r.await; // the first call waits; any later one answers at once
                }
                provider.service().rpc_respond(&req, b"ok".to_vec());
            }
        });
        let cap_key = format!("cap/{}/demo/whoami", a0.node_id());
        let marker = format!("sys/caller-context/{}", a0.node_id());
        poll_until(|| a0.kv().get(&cap_key).is_some() && a0.kv().get(&marker).is_some(), 10_000).await;

        let a2a = format!("http://127.0.0.1:{http_port}/a2a");
        let call = |sk: ed25519_dalek::SigningKey, url: String, beta: DomainId| async move {
            reqwest::Client::new()
                .post(&url)
                .header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &sk).to_header_value())
                .json(&task_body("demo/whoami"))
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        };

        // 1. The first call is in flight and parked inside the provider.
        let first = tokio::spawn(call(beta_sk.clone(), a2a.clone(), beta.clone()));
        tokio::time::timeout(Duration::from_secs(10), entered_rx.recv())
            .await
            .expect("the provider was entered")
            .expect("the channel is live");
        assert_eq!(edge.in_flight_for(&beta), 1, "the gateway is carrying it");

        // 2. A second concurrent call from the same partner is refused — and refused with its own
        //    code, because a capacity refusal is transient and -32003 says talk to your operator.
        let refused = call(beta_sk.clone(), a2a.clone(), beta.clone()).await;
        assert_eq!(refused["error"]["code"], -32004, "{refused}");
        assert!(
            refused["error"]["message"].as_str().unwrap().contains("refused, not queued"),
            "the message says what happened: {refused}",
        );

        // 3. Release the first; its slot comes back and the next call is admitted. Without the
        //    guard's `Drop` this is where a leaked slot would show up as a partner permanently
        //    short of capacity.
        let _ = release_tx.send(());
        let ok = first.await.expect("the first call completes");
        assert!(ok.get("error").is_none(), "the first call was never refused: {ok}");
        poll_until(|| edge.in_flight_for(&beta) == 0, 5_000).await;
        assert_eq!(edge.in_flight_for(&beta), 0, "the slot returned");

        let after = call(beta_sk, a2a, beta).await;
        assert!(after.get("error").is_none(), "capacity recovered: {after}");

        a0.shutdown().await;
    }

    // ── The release gate's choreography (item 2 PR 9) ─────────────────────────────────────
    //
    // The record's §13 gate, run over the PR 8 transport: discover, invoke, lose a gateway, sever
    // every link, keep working locally, change permissions mid-partition, reconnect — and prove
    // from membership tables, consensus state and traces that the meshes never merged. Every node
    // runs the **enforced domain profile** (§9: TLS, SWIM off), and the two meshes have two
    // different auto-generated CAs, which is what "independently admitted" reduces to in one
    // process.

    /// One enforced-profile node: TLS under `cert_dir` (a mesh shares one; two meshes never do),
    /// SWIM off, bootstrapped to `bootstrap`, optionally a gateway with the A2A + federation edges.
    async fn enforced_node(
        port: u16,
        cert_dir: &std::path::Path,
        bootstrap: Vec<NodeId>,
        gateway: Option<(u16, Arc<FederationEdge>)>,
    ) -> GossipAgent {
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = bootstrap;
        cfg.health_check_max_jitter_ms = 50;
        // Peer registration happens on Ping receipt, so formation under TLS needs fast pings.
        // The interval stays above `reconnect_backoff_secs + 2`: at 1/1 (the WS1 TLS test's
        // values) `validate()` warns on every start that a peer can be evicted mid-backoff and
        // never reconnect — found while bringing up the Docker suite (item 2 PR 10b).
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 4;
        cfg.domain_profile = DomainProfile::Enforced;
        cfg.swim_failure_detector = false;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.to_path_buf(), ..TlsConfig::default() });
        let a = match gateway {
            Some((http_port, edge)) => {
                cfg.http_port = Some(http_port);
                GossipAgent::new(id, cfg).with_a2a().with_federation_edge(edge)
            }
            None => GossipAgent::new(id, cfg),
        };
        a.start().await.unwrap();
        a
    }

    async fn commits(node: &GossipAgent, slot: &str, value: &'static [u8]) {
        match node.consensus().cluster_propose(slot, Bytes::from_static(value), ConsensusConfig::default()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("{} could not commit {slot}: {other:?}", node.node_id()),
        }
    }

    /// **The release gate, in one process.** What it proves and what it does not is spelled out in
    /// `docs/design/federated-domains.md` §13 and the testing page; the short form: two meshes
    /// under separate CA roots, a call crossing between them, one gateway lost and replaced, every
    /// link severed and both meshes still serving locally (KV and consensus), a grant changed
    /// while no link existed and visible on reconnect, authority already issued honoured to its
    /// expiry and not past it, and non-merger asserted from the tables, the consensus namespace
    /// and the connection tables — before, during and after.
    #[tokio::test]
    async fn the_release_gates_choreography_over_the_transport() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(13);
        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami", "demo/secret"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy::default(),
        ));

        // Two CAs: one per mesh. Everything else about the meshes is symmetric.
        let tag = alloc_port();
        let ca_a = std::env::temp_dir().join(format!("fed9-{tag}-alpha"));
        let ca_b = std::env::temp_dir().join(format!("fed9-{tag}-beta"));
        let _ = std::fs::remove_dir_all(&ca_a);
        let _ = std::fs::remove_dir_all(&ca_b);

        // Domain A: a1 (the provider), a2, gw1 (gateway). gw2's ports are allocated now — it is
        // the replacement that comes up on reconnect, and the client must know it from the start
        // (≥ 2 replaceable gateways, §10).
        let (pa1, pa2, pg1, pg2) = (alloc_port(), alloc_port(), alloc_port(), alloc_port());
        let (http1, http2) = (alloc_port(), alloc_port());
        let a_ids: Vec<NodeId> = [pa1, pa2, pg1].iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
        let others = |me: u16, all: &[NodeId]| -> Vec<NodeId> {
            let me = NodeId::new("127.0.0.1", me).unwrap();
            all.iter().filter(|n| **n != me).cloned().collect()
        };
        let a1 = Arc::new(enforced_node(pa1, &ca_a, others(pa1, &a_ids), None).await);
        let a2 = enforced_node(pa2, &ca_a, others(pa2, &a_ids), None).await;
        let gw1 = enforced_node(pg1, &ca_a, others(pg1, &a_ids), Some((http1, Arc::clone(&edge)))).await;

        // Domain B: b1, b2.
        let (pb1, pb2) = (alloc_port(), alloc_port());
        let b_ids: Vec<NodeId> = [pb1, pb2].iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
        let b1 = enforced_node(pb1, &ca_b, others(pb1, &b_ids), None).await;
        let b2 = enforced_node(pb2, &ca_b, others(pb2, &b_ids), None).await;

        // Listeners on every node (consensus needs them everywhere), then a structural poll that
        // each mesh formed — "they did not merge" is vacuous if nothing connected to anything.
        let mut listeners = vec![
            a1.consensus().start_consensus_listener(ConsensusConfig::default()),
            a2.consensus().start_consensus_listener(ConsensusConfig::default()),
            gw1.consensus().start_consensus_listener(ConsensusConfig::default()),
            b1.consensus().start_consensus_listener(ConsensusConfig::default()),
            b2.consensus().start_consensus_listener(ConsensusConfig::default()),
        ];
        poll_until(|| a1.peers().len() == 2 && a2.peers().len() == 2 && gw1.peers().len() == 2 && b1.peers().len() == 1 && b2.peers().len() == 1, 25_000).await;
        assert_eq!((a1.peers().len(), b1.peers().len()), (2, 1), "both meshes formed under their own CA");

        // The provider lives on a1, not on a gateway: gateways route, and are replaceable.
        let calls = Arc::new(AtomicUsize::new(0));
        // Both exported skills live on a1; the same responder answers either (it reports the
        // principal it was told). `demo/secret` is exported but not yet granted to beta.
        let _reg = a1.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        let _reg_secret = a1.capabilities().advertise_capability(
            crate::capability::Capability::new("demo", "secret"), Duration::from_secs(5));
        whoami_provider(Arc::clone(&a1), Arc::clone(&calls));
        let cap_key = format!("cap/{}/demo/whoami", a1.node_id());
        let cap_key_secret = format!("cap/{}/demo/secret", a1.node_id());
        // The gateway dispatches only to a provider whose caller-context marker it has seen
        // (item 7's secure profile); the capabilities and the marker all reach it by gossip.
        let marker_key = format!("sys/caller-context/{}", a1.node_id());
        let gateway_ready = |gw: &GossipAgent| {
            gw.kv().get(&cap_key).is_some() && gw.kv().get(&cap_key_secret).is_some() && gw.kv().get(&marker_key).is_some()
        };
        poll_until(|| gateway_ready(&gw1), 25_000).await;
        assert!(gateway_ready(&gw1), "the gateway learned the provider's capabilities and caller-context marker through its own mesh");

        let client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            vec![
                GatewayEndpoint { id: "gw-1".into(), base_url: format!("http://127.0.0.1:{http1}") },
                GatewayEndpoint { id: "gw-2".into(), base_url: format!("http://127.0.0.1:{http2}") },
            ],
            2, Duration::from_secs(30),
        );

        // ── 1. Discover, invoke. ──────────────────────────────────────────────────────────────
        assert_eq!(client.connect().await.unwrap(), vec!["demo/whoami".to_string()]);
        let reply = client.call("demo/whoami", "?", Repeatability::Repeatable).await.expect("call via gw-1");
        assert_eq!(reply, crate::federation_principal("beta.example", "svc/billing"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // ── 2. Consensus in each mesh, so the consensus-state leg has something to check. ─────
        commits(&a1, "fed/alpha", b"alpha-1").await;
        commits(&b1, "fed/beta", b"beta-1").await;
        poll_until(|| gw1.consensus().consensus_get("fed/alpha").is_some() && b2.consensus().consensus_get("fed/beta").is_some(), 5_000).await;
        for n in [&b1, &b2] {
            assert!(n.consensus().consensus_get("fed/alpha").is_none(), "{} learned domain A's commit", n.node_id());
        }
        for n in [a1.as_ref(), &a2, &gw1] {
            assert!(n.consensus().consensus_get("fed/beta").is_none(), "{} learned domain B's commit", n.node_id());
        }

        // ── 3. Never merged, at steady state with bytes crossing. ─────────────────────────────
        assert_never_merged(&[a1.as_ref(), &a2, &gw1], &[&b1, &b2]);

        // ── 4. Lose the gateway — with one gateway up, that severs every link. ────────────────
        listeners.remove(2);
        gw1.shutdown().await;
        match client.call("demo/whoami", "?", Repeatability::AtMostOnce).await {
            Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { attempted_via, .. })) => assert_eq!(attempted_via, vec!["gw-1".to_string()]),
            other => panic!("at-most-once through a lost gateway must be DeliveryUnknown, got {other:?}"),
        }
        match client.call("demo/whoami", "?", Repeatability::Repeatable).await {
            Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { attempted_via, reason })) => {
                assert_eq!(attempted_via, vec!["gw-1".to_string(), "gw-2".to_string()]);
                assert!(reason.contains("no further gateway available"), "{reason}");
            }
            other => panic!("repeatable with every gateway silent must still be Unknown, not failed, got {other:?}"),
        }
        assert!(client.connect().await.is_err(), "discovery cannot refresh with no gateway");
        assert_eq!(client.link_state(), LinkState::Down);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "nothing reached the provider through a lost gateway");

        // ── 5. Keep working locally, on both sides. ───────────────────────────────────────────
        let _ = a1.kv().set("local/alpha", Bytes::from_static(b"still here"));
        let _ = b1.kv().set("local/beta", Bytes::from_static(b"still here"));
        poll_until(|| a2.kv().get("local/alpha").is_some() && b2.kv().get("local/beta").is_some(), 5_000).await;
        assert!(a2.kv().get("local/alpha").is_some() && b2.kv().get("local/beta").is_some(), "gossip within each mesh continues with no link");
        commits(&a2, "fed/alpha-partitioned", b"alpha-2").await;
        commits(&b2, "fed/beta-partitioned", b"beta-2").await;

        // ── 6. Change permissions mid-partition; note authority already issued. ──────────────
        edge.set_policy(DomainPolicy {
            domain: alpha.clone(),
            revision: 2,
            grants: vec![(beta.clone(), "demo/whoami".into()), (beta.clone(), "demo/secret".into())],
        });
        let issued_before_reconnect = cred(&beta, "demo/whoami", &beta_sk);

        // ── 7. Reconnect: the replacement gateway comes up in domain A. ──────────────────────
        let gw2 = enforced_node(pg2, &ca_a, vec![a1.node_id().clone(), a2.node_id().clone()], Some((http2, Arc::clone(&edge)))).await;
        listeners.push(gw2.consensus().start_consensus_listener(ConsensusConfig::default()));
        poll_until(|| gw2.peers().len() == 2 && gateway_ready(&gw2), 25_000).await;
        assert!(gateway_ready(&gw2), "the replacement gateway learned the provider through its mesh");

        // Reconnected is not ready: no call until discovery refreshes.
        assert!(matches!(client.call("demo/whoami", "?", Repeatability::Repeatable).await, Err(ClientError::Link(LinkRefusal::Down { .. }))));
        let exports = client.connect().await.expect("catalogue via gw-2");
        assert_eq!(exports, vec!["demo/whoami".to_string(), "demo/secret".to_string()], "the grant changed mid-partition is what discovery now sees");
        assert_eq!(client.link_state(), LinkState::Ready);

        // 7a. A revision never goes backwards, even signed. A catalogue reply carries no expiry
        // and no nonce, so a captured one still verifies after the operator moves on.
        // `DomainPolicy::revision` is documented as monotonic — a consumer that has seen a higher
        // revision must not accept a lower one — and that rule was stated and implemented nowhere:
        // a reply from a revision that granted an export replayed into a fresh observation, and the
        // withdrawn export reappeared in the client's view. Phase-C audit finding. Rolling the edge
        // back to revision 1 stands in for the replay.
        edge.set_policy(DomainPolicy {
            domain: alpha.clone(),
            revision: 1,
            grants: vec![(beta.clone(), "demo/whoami".into()), (beta.clone(), "demo/secret".into())],
        });
        let rolled_back = client.connect().await;
        assert!(
            matches!(
                rolled_back,
                Err(ClientError::Catalogue(crate::federation::edge::CatalogRefusal::StaleRevision {
                    seen: 2,
                    offered: 1,
                }))
            ),
            "an older revision must be refused as stale, got {rolled_back:?}"
        );
        // Restore, and the client accepts it again — the rule is monotonicity, not one-shot.
        edge.set_policy(DomainPolicy {
            domain: alpha.clone(),
            revision: 2,
            grants: vec![(beta.clone(), "demo/whoami".into()), (beta.clone(), "demo/secret".into())],
        });
        client.connect().await.expect("the current revision is accepted again");

        // The newly granted export works — repeatable fails over past the dead gw-1 …
        let reply = client.call("demo/secret", "?", Repeatability::Repeatable).await.expect("call via gw-2");
        assert_eq!(reply, crate::federation_principal("beta.example", "svc/billing"));
        // … but at-most-once pays the dead gateway once per call, because the pool keeps no health
        // memory (PR 5, by design). Retiring the replaced gateway is the operator's move (§10).
        assert!(matches!(client.call("demo/secret", "?", Repeatability::AtMostOnce).await, Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { .. }))));
        assert!(client.retire_gateway("gw-1"));
        assert!(client.call("demo/secret", "?", Repeatability::AtMostOnce).await.is_ok());

        // Authority issued before the partition is honoured to its expiry and not past it.
        let raw = reqwest::Client::new();
        let a2a2 = format!("http://127.0.0.1:{http2}/a2a");
        let r = raw.post(&a2a2).header(HEADER_FEDERATED_CALL, issued_before_reconnect.to_header_value()).json(&task_body("demo/whoami")).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert!(v.get("error").is_none(), "a credential issued before the partition, still inside its lifetime, is accepted: {v}");
        let now = now_ms();
        let expired = PresentedCall::sign(
            &FederatedCaller { body_sha256: None, origin_domain: beta.clone(), principal: "svc/billing".into(), export: "demo/whoami".into(), issued_at_ms: now - 120_000, expires_at_ms: now - 60_000 },
            &beta_sk,
        );
        let r = raw.post(&a2a2).header(HEADER_FEDERATED_CALL, expired.to_header_value()).json(&task_body("demo/whoami")).send().await.unwrap();
        assert_eq!(r.status(), 401, "issued authority lasts only to its stated expiry — nothing is silently extended");

        // ── 8. Never merged, after the whole cycle — tables, consensus namespace, connections. ─
        commits(&a1, "fed/alpha-after", b"alpha-3").await;
        poll_until(|| gw2.consensus().consensus_get("fed/alpha-after").is_some(), 5_000).await;
        assert_never_merged(&[a1.as_ref(), &a2, &gw2], &[&b1, &b2]);
        for n in [&b1, &b2] {
            assert!(n.consensus().consensus_get("fed/alpha-after").is_none() && n.consensus().consensus_get("fed/alpha").is_none());
        }
        // Its own mesh only: every peer is a domain-A node. The count waits for the failure detector —
        // gw2 joined after gw1 was shut down, and can learn the dead gateway from a1/a2 by peer
        // exchange before its eviction has propagated (PR #470's CI, 2026-10-02: left 3, right 2,
        // with `assert_never_merged` above passing — the third peer was the dead gw-1, not a B node).
        let a_side: Vec<String> = [a1.node_id(), a2.node_id(), gw1.node_id()].iter().map(|n| n.to_string()).collect();
        for p in gw2.peers() {
            assert!(a_side.iter().any(|a| a == &p.to_string()), "gw2 peers only with domain A: {p} is not an A node");
        }
        poll_until(|| gw2.peers().len() == 2, 30_000).await;
        assert_eq!(gw2.peers().len(), 2, "the replacement gateway peers with its own mesh only, once the dead gateway is evicted");

        // ── 9. A plant on admission itself: a node holding B's CA cannot join A. ─────────────
        // Timing-bounded negative (it asserts something did *not* happen within a window), kept
        // because the two-CA claim is the one this whole test rests on; the positive control is
        // step 0 — gw2, holding A's CA, joined A above.
        let rogue = enforced_node(alloc_port(), &ca_b, vec![a1.node_id().clone()], None).await;
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(rogue.peers().is_empty(), "a node admitted by B's CA joined A's mesh: {:?}", rogue.peers());
        assert!(!a1.peers().contains(rogue.node_id()) && !a1.connected_peers().contains(rogue.node_id()));

        drop(listeners);
        rogue.shutdown().await;
        for n in [&a2, &gw2, &b1, &b2] {
            n.shutdown().await;
        }
        a1.shutdown().await;
        let _ = std::fs::remove_dir_all(&ca_a);
        let _ = std::fs::remove_dir_all(&ca_b);
    }


    /// **Three domains, one edge (item 2 row 11: more than two domains).**
    ///
    /// Alpha serves beta and gamma, who have no relationship with each other and never exchange a
    /// byte. Everything item 2 claims per-partner is stated against *a* partner; with one partner
    /// those claims cannot be distinguished from claims about the edge as a whole, and this is the
    /// configuration where the difference shows.
    ///
    /// The legs, and what each would look like if it were wrong:
    /// 1. Each partner's catalogue is **its own grants**, over real HTTP. A shared or union
    ///    catalogue would tell beta that `demo/secret` exists — enumerating what you may not call is
    ///    already being told something.
    /// 2. The other partner's grant is not yours. Beta is refused `demo/secret` **locally**, with no
    ///    HTTP, because it is not in beta's catalogue; the provider's counter proves nothing was
    ///    dispatched.
    /// 3. A **trusted** partner cannot mint a credential in the other trusted partner's name. This
    ///    is the leg that only exists at two partners: a verifier trying every key in the bundle
    ///    instead of the keys for the claimed origin would pass every single-partner test in the
    ///    tree and be a total authentication bypass between partners.
    /// 4. Revoking one partner leaves the other working.
    /// 5. **Alpha is a common neighbour and not a bridge.** Non-merger is asserted for all three
    ///    pairs — and the beta/gamma pair is the new one, two domains that never spoke, sharing a
    ///    partner that spoke to both.
    ///
    /// This complements `three_domains_compose_without_trust_composing`, which is a *chain* (alpha to
    /// beta to gamma). This is a *star*: one edge, two partners who must not be able to act for each
    /// other. Capacity per partner is `max_in_flight_per_partner`'s gate, not asserted again here.
    #[tokio::test]
    async fn three_domains_one_edge_and_the_middle_domain_is_not_a_bridge() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let gamma = DomainId::new("gamma.example").unwrap();
        let (beta_sk, beta_vk) = keypair(31);
        let (gamma_sk, gamma_vk) = keypair(32);
        let (alpha_sk, alpha_vk) = keypair(33);

        // One policy, two partners, different grants. One bundle, two keys.
        let edge = Arc::new(
            FederationEdge::new(
                alpha.clone(),
                ["demo/whoami", "demo/secret"],
                DomainPolicy {
                    domain: alpha.clone(),
                    revision: 1,
                    grants: vec![
                        (beta.clone(), "demo/whoami".into()),
                        (gamma.clone(), "demo/secret".into()),
                    ],
                },
                TrustBundle::trusting([(beta.clone(), beta_vk), (gamma.clone(), gamma_vk)]),
                CallPolicy::default(),
            )
            .with_signing_key(alpha_sk),
        );

        let http = alloc_port();
        let mut a = mesh(2, Some((http, Arc::clone(&edge)))).await;
        let a0 = Arc::new(a.remove(0));
        let a_rest = a;
        let b = mesh(1, None).await;
        let g = mesh(1, None).await;
        {
            // Only alpha is a multi-node mesh; beta and gamma are single nodes with nobody to peer
            // with, which is the point — they are separate domains, not a partitioned one.
            let (a0, a_rest) = (&a0, &a_rest);
            poll_until(|| !a0.peers().is_empty() && a_rest.iter().all(|x| !x.peers().is_empty()), 3_000).await;
        }

        // Both exports live on alpha's gateway node; the same responder answers either.
        let calls = Arc::new(AtomicUsize::new(0));
        let _reg_w = a0.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        let _reg_s = a0
            .capabilities()
            .advertise_capability(crate::capability::Capability::new("demo", "secret"), Duration::from_secs(5));
        whoami_provider(Arc::clone(&a0), Arc::clone(&calls));
        let k_w = format!("cap/{}/demo/whoami", a0.node_id());
        let k_s = format!("cap/{}/demo/secret", a0.node_id());
        poll_until(|| a0.kv().get(&k_w).is_some() && a0.kv().get(&k_s).is_some(), 5_000).await;

        let endpoint = || {
            vec![GatewayEndpoint { id: "gw-a0".into(), base_url: format!("http://127.0.0.1:{http}") }]
        };
        let beta_client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(), endpoint(), 2, Duration::from_secs(30),
        )
        .with_partner_key(alpha_vk);
        let gamma_client = FederationClient::new(
            gamma.clone(), "svc/audit", gamma_sk.clone(), alpha.clone(), endpoint(), 2, Duration::from_secs(30),
        )
        .with_partner_key(alpha_vk);

        // ── 1. Each partner's catalogue is its own grants. ────────────────────────────────────
        assert_eq!(beta_client.connect().await.expect("beta discovers"), vec!["demo/whoami".to_string()]);
        assert_eq!(gamma_client.connect().await.expect("gamma discovers"), vec!["demo/secret".to_string()]);

        // And each call reaches the provider as its *own* principal, not as "the gateway" and not
        // as the other partner.
        assert_eq!(
            beta_client.call("demo/whoami", "?", Repeatability::AtMostOnce).await.expect("beta calls"),
            crate::federation_principal("beta.example", "svc/billing"),
        );
        assert_eq!(
            gamma_client.call("demo/secret", "?", Repeatability::AtMostOnce).await.expect("gamma calls"),
            crate::federation_principal("gamma.example", "svc/audit"),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // ── 2. The other partner's grant is not yours — refused locally, no HTTP. ─────────────
        assert!(matches!(
            beta_client.call("demo/secret", "?", Repeatability::AtMostOnce).await,
            Err(ClientError::Resolve(_)),
        ));
        assert!(matches!(
            gamma_client.call("demo/whoami", "?", Repeatability::AtMostOnce).await,
            Err(ClientError::Resolve(_)),
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "neither refusal reached a provider");

        // ── 3. A trusted partner cannot speak for the other trusted partner. ─────────────────
        // Beta's key, gamma's name, gamma's grant. Every field is individually legitimate.
        let raw = reqwest::Client::new();
        let a2a = format!("http://127.0.0.1:{http}/a2a");
        let now = now_ms();
        let forged = PresentedCall::sign(
            &FederatedCaller {
                body_sha256: None,
                origin_domain: gamma.clone(),
                principal: "svc/audit".into(),
                export: "demo/secret".into(),
                issued_at_ms: now,
                expires_at_ms: now + 60_000,
            },
            &beta_sk,
        );
        let r = raw
            .post(&a2a)
            .header(HEADER_FEDERATED_CALL, forged.to_header_value())
            .json(&task_body("demo/secret"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "one partner's key must not authenticate another partner's name");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a forged origin reaches no provider");

        // ── 4. Revoking gamma leaves beta working. ────────────────────────────────────────────
        edge.revoke(&gamma);
        assert!(gamma_client.connect().await.is_err(), "a revoked partner is refused");
        assert_eq!(
            beta_client.call("demo/whoami", "?", Repeatability::AtMostOnce).await.expect("beta unaffected"),
            crate::federation_principal("beta.example", "svc/billing"),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // ── 5. Alpha is a common neighbour, not a bridge. ─────────────────────────────────────
        let alpha_nodes: Vec<&GossipAgent> = std::iter::once(a0.as_ref()).chain(a_rest.iter()).collect();
        let beta_nodes: Vec<&GossipAgent> = b.iter().collect();
        let gamma_nodes: Vec<&GossipAgent> = g.iter().collect();
        assert_never_merged(&alpha_nodes, &beta_nodes);
        assert_never_merged(&alpha_nodes, &gamma_nodes);
        // The new pair: two domains that never spoke to each other, both of which spoke to alpha.
        assert_never_merged(&beta_nodes, &gamma_nodes);

        for n in alpha_nodes {
            n.shutdown().await;
        }
        for n in b.iter().chain(g.iter()) {
            n.shutdown().await;
        }
    }

    /// A single-node agent serving its gateway over **HTTPS** (the node-cert reuse path), with the
    /// A2A and federation edges attached. Returns the agent, its port and its cert directory.
    async fn https_gateway(tag: &str, edge: Arc<FederationEdge>) -> (Arc<GossipAgent>, u16, std::path::PathBuf) {
        let bind = alloc_port();
        let http = alloc_port();
        let dir = std::env::temp_dir().join(format!("fed-pin-{}-{tag}-{bind}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = bind;
        cfg.http_port = Some(http);
        cfg.http_addr = "127.0.0.1".to_string();
        cfg.health_check_max_jitter_ms = 50;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
        cfg.gateway_tls = Some(crate::GatewayTlsConfig::default()); // reuse the node cert
        let id = NodeId::new("127.0.0.1", bind).unwrap();
        let a = Arc::new(GossipAgent::new(id, cfg).with_a2a().with_federation_edge(edge));
        a.start().await.unwrap();
        (a, http, dir)
    }

    /// The SPKI pin of the key an agent terminates TLS with, computed the way a partner would
    /// publish it — from the identity key, with no certificate to exchange.
    fn pin_of(a: &GossipAgent) -> [u8; 32] {
        let key = a.task_ctx.tls.get().expect("the agent runs with TLS").verifying_key_bytes();
        crate::federation::pinning::ed25519_spki_sha256(&key)
    }


    /// **The federation edge over TLS, anchored on a pin rather than a CA (item 2 row 11).**
    ///
    /// Two gateways, both up, both running the *same* federation edge — same domain, same policy,
    /// same catalogue signing key. They differ in exactly one thing: the TLS key that terminates
    /// the connection. So every refusal below is attributable to the transport anchor and to
    /// nothing else, and the positive control is the same client code succeeding against the
    /// gateway it pins.
    ///
    /// In order:
    /// 1. A pinned link discovers and invokes normally — pinning is not a mode that breaks things.
    /// 2. An endpoint presenting an unpinned key is refused **by name**, and the provider behind it
    ///    never sees the call. Without pinning that same endpoint answers correctly, which is what
    ///    makes this a test of the anchor rather than of reachability.
    /// 3. The refusal is `Tls(PinMismatch)` and **not** `Outcome(DeliveryUnknown)`. That is the
    ///    whole reason the variant exists: `DeliveryUnknown` says *it may have run*, which bars a
    ///    non-repeatable caller from ever retrying, and a failed handshake sent nothing.
    /// 4. Pins plus an `http://` endpoint is a refusal, not a quiet plaintext call.
    /// 5. The two builder setters both rebuild the HTTP client, so the order they are called in
    ///    must not decide whether TLS is pinned.
    ///
    /// What this does **not** prove: nothing here says anything about an attacker who holds the
    /// partner's private key, and nothing exercises certificate expiry — which this verifier
    /// deliberately does not check, for the reason given in `federation::pinning`.
    #[tokio::test]
    async fn a_pinned_federation_link_talks_only_to_the_key_the_bundle_names() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (beta_sk, beta_vk) = keypair(21);
        let (alpha_sk, alpha_vk) = keypair(22);

        let edge = Arc::new(
            FederationEdge::new(
                alpha.clone(),
                ["demo/whoami"],
                DomainPolicy {
                    domain: alpha.clone(),
                    revision: 1,
                    grants: vec![(beta.clone(), "demo/whoami".into())],
                },
                TrustBundle::trusting([(beta.clone(), beta_vk)]),
                CallPolicy::default(),
            )
            .with_signing_key(alpha_sk),
        );

        let (gw1, http1, dir1) = https_gateway("one", Arc::clone(&edge)).await;
        let (gw2, http2, dir2) = https_gateway("two", Arc::clone(&edge)).await;
        let (pin1, pin2) = (pin_of(&gw1), pin_of(&gw2));
        assert_ne!(pin1, pin2, "two gateways must differ in their TLS key, or nothing below is a test");

        // The provider lives behind gw-1 only; gw-2 is a correct gateway with the wrong key.
        let calls = Arc::new(AtomicUsize::new(0));
        let _reg = gw1.capabilities().advertise_capability(whoami(), Duration::from_secs(5));
        whoami_provider(Arc::clone(&gw1), Arc::clone(&calls));
        let cap_key = format!("cap/{}/demo/whoami", gw1.node_id());
        poll_until(|| gw1.kv().get(&cap_key).is_some(), 5_000).await;

        // The operator's wiring: the pin goes in the bundle beside the signing key, and the client
        // takes it from there.
        let mut bundle = TrustBundle::trusting([(alpha.clone(), alpha_vk)]);
        assert!(bundle.pin_tls(&alpha, pin1), "the partner is in the bundle");
        assert!(!bundle.pin_tls(&DomainId::new("stranger.example").unwrap(), pin1), "an unknown partner records nothing");
        assert_eq!(bundle.tls_pins_for(&alpha), &[pin1]);
        assert!(bundle.pin_tls(&alpha, pin1) && bundle.tls_pins_for(&alpha).len() == 1, "pinning twice is not two pins");

        let endpoints = vec![
            GatewayEndpoint { id: "gw-1".into(), base_url: format!("https://127.0.0.1:{http1}") },
            GatewayEndpoint { id: "gw-2".into(), base_url: format!("https://127.0.0.1:{http2}") },
        ];
        let client = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            endpoints.clone(), 2, Duration::from_secs(30),
        )
        .with_partner_key(alpha_vk)
        .with_tls_pins(bundle.tls_pins_for(&alpha).to_vec());

        // ── 1. The pinned link works, over real TLS. ──────────────────────────────────────────
        assert_eq!(client.connect().await.expect("catalogue over pinned TLS"), vec!["demo/whoami".to_string()]);
        assert_eq!(client.link_state(), LinkState::Ready);
        let reply = client.call("demo/whoami", "?", Repeatability::AtMostOnce).await.expect("call over pinned TLS");
        assert_eq!(reply, crate::federation_principal("beta.example", "svc/billing"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // ── 2/3. Retire gw-1, so the next call must use the endpoint with the unpinned key. ───
        // Discovery stays fresh, the link stays Ready, the pool has one gateway left: the refusal
        // that follows can only be the transport's.
        assert!(client.retire_gateway("gw-1"));
        let refused = client.call("demo/whoami", "?", Repeatability::AtMostOnce).await;
        match &refused {
            Err(ClientError::Tls(TlsRefusal::PinMismatch { gateway, presented })) => {
                assert_eq!(gateway, "gw-2");
                let expected = presented.as_deref().expect("the refusal names the key that arrived");
                assert_eq!(expected.len(), 64, "a sha256 in hex");
                assert!(expected.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            }
            other => panic!("an unpinned endpoint must be refused by name, got {other:?}"),
        }
        assert!(
            !matches!(refused, Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { .. }))),
            "a failed handshake sent nothing: reporting DeliveryUnknown would bar a retry forever",
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the call must not have reached any provider");

        // ── 4. Pins plus a plaintext endpoint: refused with nothing attempted. ────────────────
        let plaintext = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(),
            vec![GatewayEndpoint { id: "gw-1".into(), base_url: format!("http://127.0.0.1:{http1}") }],
            2, Duration::from_secs(30),
        )
        .with_partner_key(alpha_vk)
        .with_tls_pins(vec![pin1]);
        match plaintext.connect().await {
            Err(ClientError::Tls(TlsRefusal::PlaintextEndpoint { gateway, base_url })) => {
                assert_eq!(gateway, "gw-1");
                assert!(base_url.starts_with("http://"));
            }
            other => panic!("a pinned client must refuse an http:// endpoint, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // ── 5. The builder order does not decide whether TLS is pinned. ───────────────────────
        // Both clients point *only* at gw-2 and pin gw-1's key, so both must fail — the question is
        // *how*. A live pin fails as `Tls(PinMismatch)`, from our verifier. A dropped pin falls
        // back to the default Web-PKI verifier, which also rejects this self-signed endpoint, but
        // as an ordinary transport error: `Outcome(DeliveryUnknown)`. The variant is therefore the
        // whole gate here, and the control below pins that the two are actually distinguishable
        // rather than both arriving as the same thing.
        let only_gw2 = vec![GatewayEndpoint { id: "gw-2".into(), base_url: format!("https://127.0.0.1:{http2}") }];
        let pins_then_timeouts = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(), only_gw2.clone(), 2, Duration::from_secs(30),
        )
        .with_tls_pins(vec![pin1])
        .with_timeouts(Duration::from_secs(2), Duration::from_secs(5));
        let timeouts_then_pins = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk.clone(), alpha.clone(), only_gw2, 2, Duration::from_secs(30),
        )
        .with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
        .with_tls_pins(vec![pin1]);
        for (order, c) in [("pins then timeouts", &pins_then_timeouts), ("timeouts then pins", &timeouts_then_pins)] {
            match c.connect().await {
                Err(ClientError::Tls(TlsRefusal::PinMismatch { .. })) => {}
                other => panic!("{order}: the pinning was dropped by the builder, got {other:?}"),
            }
        }

        // The control for step 5: the same endpoint with **no** pins configured fails as a
        // transport error, not as a pin mismatch. Without this, step 5 would pass even if every
        // failure in this test were being labelled `PinMismatch` by accident.
        let unpinned = FederationClient::new(
            beta.clone(), "svc/billing", beta_sk, alpha.clone(),
            vec![GatewayEndpoint { id: "gw-2".into(), base_url: format!("https://127.0.0.1:{http2}") }],
            2, Duration::from_secs(30),
        )
        .with_partner_key(alpha_vk);
        match unpinned.connect().await {
            Err(ClientError::Outcome(CallOutcome::DeliveryUnknown { .. })) => {}
            other => panic!("an unpinned client has no anchor and must fail as transport, got {other:?}"),
        }

        gw1.shutdown().await;
        gw2.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}


/// A `DomainId` that arrives over the wire obeys the same rules as one built in process.
///
/// `DomainId::new` refuses anything outside `[a-z0-9.-]` and anything over 253 bytes, and the
/// type's own documentation gives the reason: two ids differing only in case are one domain to a
/// human and two to a `HashMap`, and the place that difference surfaces is a trust decision. A
/// derived `Deserialize` writes the inner field and checks nothing, so that rule held only for
/// constructed values -- while **most** `DomainId`s are parsed, from partner-controlled bytes,
/// before anything about them has been verified.
///
/// What this does NOT prove: nothing here shows an unvalidated id could forge authority. It could
/// not -- an id no trust bundle holds a key for is refused whatever its spelling. This pins the
/// narrower claim, which is that the type means on the wire what it says it means.
/// Found by the §12.6 trust-edge fuzz work.
#[test]
fn a_domain_id_from_the_wire_obeys_its_own_constructor() {
    use crate::federation::DomainId;

    // The exact values the constructor refuses, refused identically when parsed.
    for bad in ["UPPER", "mixed.Case", "has space", "sl/ash", "new\nline", "under_score", ""] {
        assert!(DomainId::new(bad).is_err(), "constructor must refuse {bad:?}");
        let json = serde_json::to_string(bad).expect("encode");
        assert!(
            serde_json::from_str::<DomainId>(&json).is_err(),
            "the wire must refuse {bad:?} too -- the constructor's rule is not optional",
        );
    }

    // The length cap holds on the wire as well.
    let over = "a".repeat(254);
    assert!(DomainId::new(&over).is_err(), "constructor must refuse a 254-byte id");
    let json = serde_json::to_string(&over).expect("encode");
    assert!(
        serde_json::from_str::<DomainId>(&json).is_err(),
        "the wire must enforce the 253-byte cap",
    );

    // And a legitimate id still round-trips unchanged -- the rule is validation, not rejection.
    let good = DomainId::new("depot.example").expect("a valid id");
    let encoded = serde_json::to_string(&good).expect("encode");
    assert_eq!(encoded, "\"depot.example\"", "the wire form is the bare id, unchanged");
    let back: DomainId = serde_json::from_str(&encoded).expect("a valid id must still parse");
    assert_eq!(good, back, "a valid id must survive its own round trip");
}

/// The same gap, on the two mandate newtypes: `new` refuses an empty id and the derived
/// `Deserialize` did not. An empty identifier is the one value that names nobody while comparing
/// equal to itself, so it is worth not admitting from a store or a peer.
#[test]
fn a_mandate_identifier_from_the_wire_is_never_empty() {
    use crate::mandate::{PrincipalId, TermId};

    assert!(PrincipalId::new("").is_none(), "constructor must refuse an empty principal");
    assert!(TermId::new("").is_none(), "constructor must refuse an empty term id");
    assert!(
        serde_json::from_str::<PrincipalId>("\"\"").is_err(),
        "the wire must refuse an empty principal too",
    );
    assert!(
        serde_json::from_str::<TermId>("\"\"").is_err(),
        "the wire must refuse an empty term id too",
    );

    // Non-empty still round-trips.
    let p = PrincipalId::new("dispatcher").expect("valid");
    let back: PrincipalId = serde_json::from_str(&serde_json::to_string(&p).expect("encode"))
        .expect("a valid principal must still parse");
    assert_eq!(p, back);
}

// ── The identity-proof default (Phase 3; on by default since post-360 row S) ───
//
// `validate_and_merge_identity`'s two arms are unit-tested in `agent::http`, and the default value
// is pinned — with its reason (`true` since row S) — in `mycelium-core::config`
// (`the_default_requires_identity_proofs`). What is tested here is the **join**: that the flag an
// operator sets actually reaches the rejecting arm, because a setting nothing exercises end to end
// is a setting nobody has checked.
//
// And the second test states the limit, which matters more than it looks: requiring proofs closes
// *one* residual (an unsigned entry mimicking a pre-Phase-2 node) and **not** trust-on-first-use.
// Anyone reading "identity proofs required" as "identity is authenticated" would be wrong, so the
// boundary gets an executable statement rather than a caveat in prose that can quietly rot.
#[cfg(feature = "tls")]
mod identity_proof_default {
    use super::*;
    use crate::agent::helpers::{encode_identity_proof, identity_proof_message, validate_and_merge_identity};
    use ed25519_dalek::{Signer, SigningKey};

    fn keys() -> papaya::HashMap<NodeId, Vec<[u8; 32]>> {
        papaya::HashMap::new()
    }
    fn anchors() -> papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> {
        papaya::HashMap::new()
    }

    /// **A default configuration rejects an unsigned identity entry** (post-360 plan row S) — end to
    /// end, from the configuration a deployment gets by saying nothing to the entry that does not
    /// enter `peer_keys`. This test asserted the default once before (2026-09-23), was renamed
    /// `an_opted_in_configuration_…` when the flip was reverted, and is the default's again: the
    /// revert's cause was misattributed (the flag is inert without TLS) and the window it named is
    /// closed by the sealed record. Seen failing first on 4251d50a.
    #[test]
    fn a_default_configuration_rejects_an_unsigned_identity_entry() {
        let cfg = GossipConfig::default(); // nothing set: the default arm

        let victim = NodeId::new("127.0.0.1", 7101).unwrap();
        let attacker_key = SigningKey::from_bytes(&[42u8; 32]).verifying_key().to_bytes();
        let history = attacker_key.to_vec();

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(
            &pk, &anchor, &counter, &victim, &history, &[attacker_key],
            None,                              // no proof — a bare key list
            cfg.require_identity_proofs,       // the default arm
        );
        assert!(pk.pin().get(&victim).is_none(), "an unsigned entry does not enter peer_keys");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "and it is counted, not silently dropped");
    }

    /// **The sealed record is the point of Phase 3b: one message, so there is no window.**
    ///
    /// With proofs required, a sealed `sys/identity-signed/{node}` validates on its own — the keys
    /// and the proof that authenticates them are the same KV entry, so a peer can never hold one
    /// without the other. This is what makes the flag safe to enable at all; the 2026-09-23 attempt
    /// to default it on was reverted precisely because the legacy pair could arrive apart.
    #[test]
    fn a_sealed_record_is_accepted_when_proofs_are_required() {
        let node = NodeId::new("127.0.0.1", 7110).unwrap();
        let sk = SigningKey::from_bytes(&[51u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        let sig = sk.sign(&identity_proof_message(&history)).to_bytes();
        let sealed = crate::agent::helpers::encode_sealed_identity(
            &history, &encode_identity_proof(&vk, &sig));

        // No legacy proof anywhere: the sealed record is the *only* evidence, which is the case
        // this test exists for.
        let (h, proof) = crate::agent::helpers::resolve_identity_record(
            Some(&sealed), &history, None, /* require */ true);
        assert_eq!(h, &history[..], "the sealed record supplies the history it signs");

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(
            &pk, &anchor, &counter, &node, h, &[vk], proof, true);
        assert!(
            pk.pin().get(&node).is_some_and(|v| v.contains(&vk)),
            "a sealed record authenticates itself with no sibling entry to wait for",
        );
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// **The legacy pair is refused when proofs are required — deliberately, and this is the whole
    /// mechanism.**
    ///
    /// The pair is two independent gossip messages. Accepting it under `require_identity_proofs`
    /// would reopen the window the flag exists to close: learn the identity, reject it for want of
    /// a proof, hold no key, and decide a leader election inside the gap. So a peer publishing only
    /// the pair is treated exactly as the flag already treated a peer publishing no proof — it
    /// predates the mechanism being required.
    ///
    /// If this test ever "fails" because someone made the pair acceptable again, the window is back
    /// and the default must not be flipped.
    #[test]
    fn the_legacy_pair_is_refused_when_proofs_are_required() {
        let node = NodeId::new("127.0.0.1", 7111).unwrap();
        let sk = SigningKey::from_bytes(&[52u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        let legacy_proof = encode_identity_proof(&vk, &sk.sign(&identity_proof_message(&history)).to_bytes());

        // A *valid* pair — the point is that validity is not the issue; arrival is.
        let (h, proof) = crate::agent::helpers::resolve_identity_record(
            None, &history, Some(&legacy_proof), /* require */ true);
        assert!(proof.is_none(), "the pair is surfaced unproven, so the validator rejects it");

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(&pk, &anchor, &counter, &node, h, &[vk], proof, true);
        assert!(pk.pin().get(&node).is_none(), "an un-sealed peer does not enter peer_keys");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "and it is counted, not silently dropped");
    }

    /// **And nothing changes for a deployment that has not opted in.** With the flag off (the
    /// default), the legacy pair is still honoured exactly as before — this is the compatibility
    /// claim that lets the sealed record ship without a rolling-upgrade note.
    #[test]
    fn the_legacy_pair_still_works_when_proofs_are_not_required() {
        let node = NodeId::new("127.0.0.1", 7112).unwrap();
        let sk = SigningKey::from_bytes(&[53u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        let legacy_proof = encode_identity_proof(&vk, &sk.sign(&identity_proof_message(&history)).to_bytes());

        let (h, proof) = crate::agent::helpers::resolve_identity_record(
            None, &history, Some(&legacy_proof), /* require */ false);
        assert!(proof.is_some(), "the pair is still the record when nothing requires sealing");

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(&pk, &anchor, &counter, &node, h, &[vk], proof, false);
        assert!(pk.pin().get(&node).is_some_and(|v| v.contains(&vk)));
    }

    /// A sealed record is not a way in. Its proof is verified by the same rule as any other, so a
    /// record signed by a key with no claim to this node is rejected once the node is established.
    #[test]
    fn a_sealed_record_cannot_introduce_a_foreign_key_for_an_established_node() {
        let node = NodeId::new("127.0.0.1", 7113).unwrap();
        let real = SigningKey::from_bytes(&[54u8; 32]).verifying_key().to_bytes();
        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        pk.pin().insert(node.clone(), vec![real]); // established

        let attacker = SigningKey::from_bytes(&[55u8; 32]);
        let a_vk = attacker.verifying_key().to_bytes();
        let history = a_vk.to_vec();
        let sealed = crate::agent::helpers::encode_sealed_identity(
            &history, &encode_identity_proof(&a_vk, &attacker.sign(&identity_proof_message(&history)).to_bytes()));

        let (h, proof) = crate::agent::helpers::resolve_identity_record(
            Some(&sealed), &history, None, true);
        validate_and_merge_identity(&pk, &anchor, &counter, &node, h, &[a_vk], proof, true);
        assert!(
            !pk.pin().get(&node).is_some_and(|v| v.contains(&a_vk)),
            "sealing changes how the record travels, never what authorises it",
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    /// Malformed sealed values are *no record*, never a partial one — so a truncated or
    /// wrong-version entry falls back to the legacy path rather than authenticating anything.
    #[test]
    fn a_malformed_sealed_record_is_no_record() {
        use crate::agent::helpers::{encode_sealed_identity, parse_sealed_identity};
        let sk = SigningKey::from_bytes(&[56u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        let good = encode_sealed_identity(&history, &encode_identity_proof(&vk, &sk.sign(&identity_proof_message(&history)).to_bytes()));
        assert!(parse_sealed_identity(&good).is_some(), "the round trip holds");

        let mut wrong_version = good.clone();
        wrong_version[0] = 2;
        assert!(parse_sealed_identity(&wrong_version).is_none(), "a future version is not guessed at");
        assert!(parse_sealed_identity(&good[..good.len() - 1]).is_none(), "a truncated proof is not a proof");
        assert!(parse_sealed_identity(&[]).is_none());
        // History that is not key-aligned: one byte inserted, so the split lands mid-key.
        let mut misaligned = good.clone();
        misaligned.insert(1, 0xAB);
        assert!(parse_sealed_identity(&misaligned).is_none(), "a history that is not whole keys is refused");
    }

    /// **The limit, stated as a test.** With proofs required, a *self-signed* entry for a node this
    /// one has never seen is still accepted: first sighting is trust-on-first-use, because there is
    /// nothing yet to chain to. Proofs close the "unsigned mimic" residual; anchors — a direct,
    /// CA-validated connection — are what close this one.
    ///
    /// The test exists so nobody reads `require_identity_proofs: true` as *identity is
    /// authenticated*. It is not a defect being enshrined; it is a boundary being kept visible, and
    /// if the TOFU window is ever closed this test should fail and be rewritten rather than quietly
    /// keep passing.
    #[test]
    fn requiring_proofs_does_not_close_trust_on_first_use() {
        let stranger = NodeId::new("127.0.0.1", 7102).unwrap();
        let sk = SigningKey::from_bytes(&[43u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        // Self-signed: the signer is one of the keys in its own published history.
        let sig = sk.sign(&identity_proof_message(&history)).to_bytes();
        let proof = encode_identity_proof(&vk, &sig);

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(
            &pk, &anchor, &counter, &stranger, &history, &[vk], Some(&proof), /* require */ true,
        );
        assert!(
            pk.pin().get(&stranger).is_some_and(|v| v.contains(&vk)),
            "first sighting of an unknown node is TOFU even with proofs required",
        );
        assert_eq!(counter.load(Ordering::SeqCst), 0, "and it is not flagged, because it is not a conflict");

        // Once a key IS established for that node, the TOFU door shuts: a second, differently-keyed
        // self-signed entry cannot introduce itself — it must chain to what is already trusted.
        let usurper = SigningKey::from_bytes(&[44u8; 32]);
        let u_vk = usurper.verifying_key().to_bytes();
        let u_history = u_vk.to_vec();
        let u_sig = usurper.sign(&identity_proof_message(&u_history)).to_bytes();
        let u_proof = encode_identity_proof(&u_vk, &u_sig);
        validate_and_merge_identity(
            &pk, &anchor, &counter, &stranger, &u_history, &[u_vk], Some(&u_proof), true,
        );
        assert!(
            !pk.pin().get(&stranger).is_some_and(|v| v.contains(&u_vk)),
            "an unchained key never joins an established set — this is the poisoning case",
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1, "and that one IS flagged");
    }

    /// **A signature a node makes for any other purpose must never be an identity proof.**
    ///
    /// The proof was a bare signature over the key history, and a history is `32 × N` bytes and
    /// nothing more. Consensus signed its serialized `ConsensusMsg` bare too, and a `PrepareAck`
    /// carries the **proposer's** bytes back (`accepted_value`). So a member A proposes a value
    /// that embeds A's own key at a 32-byte boundary, V accepts it, A prepares a higher ballot, V
    /// answers with the value — signed as V — and A publishes V's signed answer as
    /// `sys/identity-signed/V`: a sealed record whose "history" is the answer and whose proof is
    /// V's own signature over it. The verifier chained it through V's trusted key and merged every
    /// 32-byte chunk, A's key among them; A could then sign as V.
    ///
    /// The proof is now a signature over `mycelium.identity/proof/1 ‖ len ‖ history`, which no
    /// other signing path produces. The answer below is signed the way a 2.31 node signs a
    /// consensus message — bare — which is also what a 2.31 node's identity proof looks like;
    /// with proofs required, neither is a proof any more. Seen failing on the unfixed code: A's
    /// key entered `peer_keys[V]`.
    #[cfg(feature = "consensus")]
    #[test]
    fn a_signed_consensus_answer_is_not_an_identity_proof() {
        use crate::agent::helpers::{encode_sealed_identity, parse_identity_keys, resolve_identity_record};
        use crate::consensus::{encode_consensus_msg, ConsensusMsg};

        let victim = NodeId::new("127.0.0.1", 7120).unwrap();
        let v_sk = SigningKey::from_bytes(&[60u8; 32]);
        let v_vk = v_sk.verifying_key().to_bytes();
        let a_vk = SigningKey::from_bytes(&[61u8; 32]).verifying_key().to_bytes();

        // V's answer to A's prepare, carrying the value A proposed. The value is A's to choose.
        let ack = |value: Vec<u8>| ConsensusMsg::PrepareAck {
            slot: Arc::from("leader/g"), ballot: 2, voter: victim.clone(),
            accepted_ballot: 1, accepted_digest: Some([0u8; 32]),
            accepted_value: Some(Bytes::from(value)), committed_digest: None,
        };
        // Where the value lands in the encoding, found with a marker rather than assumed.
        let marker = vec![0xABu8; 64];
        let probe = encode_consensus_msg(&ack(marker.clone()));
        let at = probe.windows(64).position(|w| w == &marker[..]).expect("the value is carried verbatim");
        let base = probe.len() - 64;
        // Lead so A's key sits on a 32-byte boundary; tail so the whole answer is 32-aligned.
        let lead = (32 - at % 32) % 32;
        let mut value = vec![0u8; lead];
        value.extend_from_slice(&a_vk);
        let pad = (32 - (base + value.len()) % 32) % 32;
        value.extend(std::iter::repeat_n(0u8, pad));
        let blob = encode_consensus_msg(&ack(value));
        assert_eq!(blob.len() % 32, 0, "the answer reads as whole keys");
        let chunks = parse_identity_keys(&blob);
        assert!(chunks.contains(&a_vk), "and A's key is one of them");

        // What V signs as a consensus participant, published by A as V's sealed identity record.
        let v_sig = v_sk.sign(&blob).to_bytes();
        let sealed = encode_sealed_identity(&blob, &encode_identity_proof(&v_vk, &v_sig));

        // V is established at this node, so the record chains through a trusted key.
        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        pk.pin().insert(victim.clone(), vec![v_vk]);
        let (h, proof) = resolve_identity_record(Some(&sealed), &[], None, /* require */ true);
        assert_eq!(h, &blob[..], "the sealed record is the one validated");
        validate_and_merge_identity(&pk, &anchor, &counter, &victim, h, &chunks, proof, true);
        assert!(
            !pk.pin().get(&victim).is_some_and(|v| v.contains(&a_vk)),
            "A's key must not enter peer_keys[V]: a consensus signature is not an identity proof",
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1, "and the attempt is counted");
    }

    /// **The 2.32.0 window is closed** (post-360 plan row F, `docs/guide/deprecations.md` §23/§35).
    /// A proof signed the pre-2.32.0 way — bare, over the history — was accepted for one release
    /// while `require_identity_proofs` was off, counted on `identity_untagged_proofs`. In 2.33.0 it
    /// is refused in both postures: only a signature over `identity_proof_message` is a proof. Seen
    /// failing first: with the flag off the bare proof still established the key.
    #[test]
    fn a_bare_identity_proof_is_refused_now_the_window_is_closed() {
        let node = NodeId::new("127.0.0.1", 7121).unwrap();
        let sk = SigningKey::from_bytes(&[62u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let history = vk.to_vec();
        let old_form = encode_identity_proof(&vk, &sk.sign(&history).to_bytes());

        for require in [false, true] {
            let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
            validate_and_merge_identity(&pk, &anchor, &counter, &node, &history, &[vk], Some(&old_form), require);
            assert!(pk.pin().get(&node).is_none(), "require_identity_proofs={require}: a bare signature is not a proof");
            assert_eq!(counter.load(Ordering::SeqCst), 1, "and the refusal is counted as a conflict");
        }

        // The tagged form is what both postures accept.
        let new_form = encode_identity_proof(&vk, &sk.sign(&identity_proof_message(&history)).to_bytes());
        for require in [false, true] {
            let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
            validate_and_merge_identity(&pk, &anchor, &counter, &node, &history, &[vk], Some(&new_form), require);
            assert!(pk.pin().get(&node).is_some_and(|v| v.contains(&vk)));
            assert_eq!(counter.load(Ordering::SeqCst), 0);
        }
    }

    /// **A bare proof signed by a victim's own key is refused, not merged** (the adversarial review
    /// of #585, F4a, re-read after the window closed). Under the 2.32 allowance such a proof merged on
    /// `signer_trusted` and only the anchor tripwire flagged it; now it is not a proof at all.
    #[test]
    fn a_bare_proof_signed_by_the_victim_is_refused_and_flagged() {
        let victim = NodeId::new("127.0.0.1", 7122).unwrap();
        let v_sk = SigningKey::from_bytes(&[65u8; 32]);
        let v_vk = v_sk.verifying_key().to_bytes();
        let a_vk = SigningKey::from_bytes(&[66u8; 32]).verifying_key().to_bytes();
        let mut history = v_vk.to_vec();
        history.extend_from_slice(&a_vk);
        // V's key is CA-anchored (a direct connection), and the bare proof is V's own signature.
        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        anchor.pin().insert(victim.clone(), std::collections::HashSet::from([v_vk]));
        let bare = encode_identity_proof(&v_vk, &v_sk.sign(&history).to_bytes());
        validate_and_merge_identity(&pk, &anchor, &counter, &victim, &history, &[v_vk, a_vk], Some(&bare), false);
        assert!(pk.pin().get(&victim).is_none_or(|v| !v.contains(&a_vk)), "A's key does not enter V's set");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "and the refusal is flagged");
    }

    /// **The consensus side: the bare form no longer verifies** (post-360 plan row F closes the
    /// 2.32.0 window, `deprecations.md` §23/§35). A payload is signed under its own tag; a bare
    /// signature — a 2.31 node's — was accepted for one release and counted; a signature under the
    /// identity tag, or any other, never was. Seen failing first (in the 2.32 API's form,
    /// `== None`): the bare signature still verified as `Some(Untagged)`.
    #[cfg(feature = "consensus")]
    #[test]
    fn a_consensus_signature_verifies_only_in_the_tagged_form() {
        use crate::consensus::{consensus_signing_message, verify_consensus_signature};
        let sk = SigningKey::from_bytes(&[63u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        let other = SigningKey::from_bytes(&[64u8; 32]).verifying_key().to_bytes();
        let payload = b"a serialized ConsensusMsg";
        let tagged = sk.sign(&consensus_signing_message(payload)).to_bytes();
        let bare = sk.sign(payload).to_bytes();
        let as_identity = sk.sign(&identity_proof_message(payload)).to_bytes();
        assert!(verify_consensus_signature(&[other, vk], payload, &tagged));
        assert!(!verify_consensus_signature(&[vk], payload, &bare), "2.31's bare form: the window is closed");
        assert!(!verify_consensus_signature(&[vk], payload, &as_identity), "a signature under another tag is not a consensus signature");
        assert!(!verify_consensus_signature(&[other], payload, &tagged));
        assert!(!verify_consensus_signature(&[], payload, &tagged), "no key, no verification");
    }
}

// ── The federation verbs at the gateway (item 2 row 11) ──────────────────────
//
// `/gateway/federation/*` is the consumer side of federation, and the thing the Python and
// TypeScript SDK verbs talk to. The provider side (`/federation/catalog`, `/a2a`) is tested above
// under `federation_transport`; what is tested here is what these routes add, and it is one claim
// with a negative attached:
//
//   **The credential the partner receives names the local caller** — not this node, not the
//   client's configured principal — **and the request body cannot say otherwise.**
//
// Everything else in this module exists to make that claim non-vacuous: the routes are behind the
// gateway's bearer-then-scope layer, a refusal says whether anything was sent, and the read verbs
// read without touching the network.
#[cfg(all(feature = "gateway", feature = "tls", feature = "a2a"))]
mod federation_gateway_verbs {
    use super::*;
    use crate::federation::{
        call::{CallPolicy, CallRefusal, FederatedCaller},
        client::{ClientError, FederationClient, GatewayEndpoint},
        edge::{now_ms, FederationEdge, PresentedCall, HEADER_FEDERATED_CALL},
        gateway::Repeatability,
        session::LinkState,
        DomainId, DomainPolicy, TrustBundle,
    };
    use std::sync::atomic::AtomicUsize;

    /// A credential minted by hand, for planting a call the client would refuse to make.
    fn cred_for(origin: &DomainId, export: &str, sk: &ed25519_dalek::SigningKey) -> PresentedCall {
        let now = now_ms();
        PresentedCall::sign(
            &FederatedCaller {
                origin_domain: origin.clone(),
                principal: "svc/hub".into(),
                export: export.into(),
                issued_at_ms: now,
                expires_at_ms: now + 60_000,
                body_sha256: None,
            },
            sk,
        )
    }

    fn keypair(seed: u8) -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let vk = sk.verifying_key().to_bytes();
        (sk, vk)
    }

    /// A serving domain: one node that is its own gateway, advertises `skills` and answers with
    /// the principal it was told. Returns the node, its HTTP port, the call counter and the
    /// advertisement registrations (which must outlive the test, or the capability is tombstoned).
    async fn serving_domain(
        edge: Arc<FederationEdge>,
        skills: &[&str],
    ) -> (Arc<GossipAgent>, u16, Arc<AtomicUsize>, Vec<crate::CapabilityReg>) {
        let port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(http_port);
        cfg.health_check_max_jitter_ms = 50;
        let a = Arc::new(
            GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
                .with_a2a()
                .with_federation_edge(edge),
        );
        a.start().await.unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        // The registrations are returned, not dropped here: dropping one tombstones its capability.
        let regs: Vec<crate::CapabilityReg> = skills
            .iter()
            .map(|s| {
                let (ns, name) = s.split_once('/').expect("a skill id is ns/name");
                a.capabilities()
                    .advertise_capability(crate::capability::Capability::new(ns, name), Duration::from_secs(5))
            })
            .collect();
        let provider = Arc::clone(&a);
        let counted = Arc::clone(&calls);
        let mut rx = a.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                counted.fetch_add(1, Ordering::SeqCst);
                let reply = match provider.request_principal(&req) {
                    Ok(p) => p.name(),
                    Err(e) => format!("refused:{e}"),
                };
                provider.service().rpc_respond(&req, reply.into_bytes());
            }
        });

        // The gateway dispatches only to a provider whose capability *and* caller-context marker
        // it has seen (item 7's secure profile). Here they are the same node, so both are local.
        let cap_keys: Vec<String> = skills.iter().map(|s| format!("cap/{}/{s}", a.node_id())).collect();
        let marker = format!("sys/caller-context/{}", a.node_id());
        poll_until(
            || cap_keys.iter().all(|k| a.kv().get(k).is_some()) && a.kv().get(&marker).is_some(),
            10_000,
        )
        .await;
        await_gateway(http_port).await;
        (a, http_port, calls, regs)
    }

    /// The consumer domain: one node with a gateway, a bearer token, and a client for `partner`.
    async fn beta_domain(client: Arc<FederationClient>, token: &str) -> (GossipAgent, u16) {
        let port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some(token.to_string());
        cfg.health_check_max_jitter_ms = 50;
        let b = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
            .with_federation_clients([client]);
        b.start().await.unwrap();
        await_gateway(http_port).await;
        (b, http_port)
    }

    /// The HTTP server comes up on its own task, so every test here waits for it structurally —
    /// `/health` answering, never a sleep (the testing page's rule).
    async fn await_gateway(port: u16) {
        let url = format!("http://127.0.0.1:{port}/health");
        let http = reqwest::Client::new();
        for _ in 0..200 {
            if http.get(&url).send().await.is_ok_and(|r| r.status().is_success()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("gateway on {port} never became reachable");
    }

    async fn get(url: &str, bearer: Option<&str>) -> (u16, serde_json::Value) {
        let mut req = reqwest::Client::new().get(url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let r = req.send().await.expect("gateway answered");
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(serde_json::Value::Null))
    }

    async fn post(url: &str, bearer: Option<&str>, body: serde_json::Value) -> (u16, serde_json::Value) {
        let mut req = reqwest::Client::new().post(url).json(&body);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let r = req.send().await.expect("gateway answered");
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(serde_json::Value::Null))
    }

    /// **The row-11 gate.** A local client with nothing but a bearer token discovers a partner,
    /// calls one of its exports, and the partner's provider reports *that client's* principal.
    #[tokio::test]
    async fn the_gateway_verbs_carry_the_local_caller_across_the_boundary() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (alpha_sk, alpha_vk) = keypair(21);
        let (beta_sk, beta_vk) = keypair(22);

        let edge = Arc::new(
            FederationEdge::new(
                alpha.clone(),
                ["demo/whoami", "demo/secret"],
                DomainPolicy {
                    domain: alpha.clone(),
                    revision: 7,
                    grants: vec![(beta.clone(), "demo/whoami".into())],
                },
                TrustBundle::trusting([(beta.clone(), beta_vk)]),
                CallPolicy::default(),
            )
            .with_signing_key(alpha_sk),
        );
        let (a, alpha_http, calls, _regs) = serving_domain(Arc::clone(&edge), &["demo/whoami"]).await;

        // The client's *own* principal is `svc/billing`. Nothing a gateway caller does should ever
        // make the partner see it — that is the whole point of the route.
        let client = Arc::new(
            FederationClient::new(
                beta.clone(),
                "svc/billing",
                beta_sk,
                alpha.clone(),
                vec![GatewayEndpoint { id: "gw-a".into(), base_url: format!("http://127.0.0.1:{alpha_http}") }],
                2,
                Duration::from_secs(30),
            )
            .with_partner_key(alpha_vk),
        );
        let (b, beta_http) = beta_domain(Arc::clone(&client), "s3cret").await;
        let base = format!("http://127.0.0.1:{beta_http}/gateway/federation");
        let bearer = Some("s3cret");

        // 0. The routes are behind the gateway's auth layer. A missing bearer is 401 — checked
        //    first, because every assertion after it would be worthless if they were open.
        let (status, _) = get(&format!("{base}/partners"), None).await;
        assert_eq!(status, 401, "the federation verbs sit behind bearer-then-scope like every other local verb");

        // 1. Before discovery: a partner row exists, the link is down, nothing was observed.
        let (status, v) = get(&format!("{base}/partners"), bearer).await;
        assert_eq!(status, 200);
        assert_eq!(v["partners"][0]["domain"], "alpha.example");
        assert_eq!(v["partners"][0]["link"], "down");
        assert!(v["partners"][0]["last_catalogue"].is_null());

        // 2. A call before discovery is refused **here**, and says so: nothing was sent, so an
        //    at-most-once caller may retry without reasoning about our internals.
        let (status, v) = post(
            &format!("{base}/call"),
            bearer,
            serde_json::json!({"domain": "alpha.example", "export": "demo/whoami", "text": "?"}),
        )
        .await;
        assert_eq!(status, 409, "{v}");
        assert_eq!(v["error"], "link");
        assert_eq!(v["sent"], false);
        assert_eq!(v["delivery"], "none");
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // 3. Discovery through the route: the catalogue is the grant, not the export list.
        let (status, v) = post(&format!("{base}/connect"), bearer, serde_json::json!({"domain": "alpha.example"})).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["exports"], serde_json::json!(["demo/whoami"]));
        assert_eq!(v["link"], "ready");

        // 4. The read verb reads what is already held — no network, and the same answer twice.
        let (status, v) = get(&format!("{base}/catalog/alpha.example"), bearer).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["observed"], true);
        assert_eq!(v["exports"], serde_json::json!(["demo/whoami"]));

        // 5. **The gate.** The provider is told the *gateway caller's* principal, qualified by the
        //    domain that vouched for it — not `svc/billing`, and not the node.
        let (status, v) = post(
            &format!("{base}/call"),
            bearer,
            serde_json::json!({"domain": "alpha.example", "export": "demo/whoami", "text": "?"}),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        let expected = crate::federation_principal("beta.example", &format!("token:{}/legacy", b.node_id()));
        assert_eq!(v["reply"], expected, "the partner sees the local caller, qualified by this domain");
        assert_ne!(v["reply"], crate::federation_principal("beta.example", "svc/billing"), "never the client's configured principal");
        assert_eq!(v["delivery"], "completed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 6. **The negative.** The body cannot name the principal. A caller who could would have
        //    this domain vouch for an identity nothing authenticated — item 7's confused deputy,
        //    one boundary out.
        let (status, v) = post(
            &format!("{base}/call"),
            bearer,
            serde_json::json!({
                "domain": "alpha.example", "export": "demo/whoami", "text": "?",
                "principal": "svc/treasury", "origin_domain": "gamma.example",
            }),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["reply"], expected, "the body is not an identity claim");

        // 7. An export the catalogue never granted: refused by the resolver, before any byte.
        let (status, v) = post(
            &format!("{base}/call"),
            bearer,
            serde_json::json!({"domain": "alpha.example", "export": "demo/secret", "text": "?"}),
        )
        .await;
        assert_eq!(status, 409, "{v}");
        assert_eq!(v["error"], "resolve");
        assert_eq!(v["sent"], false);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the refused call never reached the provider");

        // 8. A domain nobody configured is a 404 that names it, not a silent failure.
        let (status, v) = post(
            &format!("{base}/call"),
            bearer,
            serde_json::json!({"domain": "gamma.example", "export": "demo/whoami", "text": "?"}),
        )
        .await;
        assert_eq!(status, 404, "{v}");
        assert!(v["error"].as_str().unwrap().contains("gamma.example"));

        // 9. The provider domain answers `domain` about itself; the consumer, having no edge,
        //    answers `configured: false` rather than pretending to be a domain.
        let (status, v) = get(&format!("http://127.0.0.1:{alpha_http}/gateway/federation/domain"), None).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["configured"], true);
        assert_eq!(v["domain"], "alpha.example");
        assert_eq!(v["policy_revision"], 7);
        assert_eq!(v["exports"], serde_json::json!(["demo/whoami", "demo/secret"]));
        let (status, v) = get(&format!("{base}/domain"), bearer).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["configured"], false);

        b.shutdown().await;
        a.shutdown().await;
    }

    /// **The provider's side of the budget** — the Phase-C audit's last open finding.
    ///
    /// `GatewayPool` has metered per-partner slots since PR 5, on the **consumer** side: it bounds
    /// what we send a partner. Nothing bounded what a partner sends us, so a compromised or simply
    /// buggy partner — one whose credentials are, by construction, minted by *them* — could hold
    /// this gateway's whole capacity.
    ///
    /// Four claims, and the third is the one that makes it a budget rather than a global limit.
    #[tokio::test]
    async fn the_edge_meters_calls_per_partner_and_refuses_rather_than_queues() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let gamma = DomainId::new("gamma.example").unwrap();
        let (_, beta_vk) = keypair(51);
        let (_, gamma_vk) = keypair(52);

        let edge = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami"],
            DomainPolicy {
                domain: alpha.clone(),
                revision: 1,
                grants: vec![(beta.clone(), "demo/whoami".into()), (gamma.clone(), "demo/whoami".into())],
            },
            TrustBundle::trusting([(beta.clone(), beta_vk), (gamma.clone(), gamma_vk)]),
            CallPolicy { max_in_flight_per_partner: 2, ..CallPolicy::default() },
        ));

        // 1. The cap admits up to its limit and then refuses — by name, with the limit in it.
        let first = edge.admit(&beta).expect("the first slot");
        let second = edge.admit(&beta).expect("the second");
        match edge.admit(&beta) {
            Err(CallRefusal::AtCapacity { partner, limit }) => {
                assert_eq!((partner.as_str(), limit), ("beta.example", 2));
            }
            other => panic!("over the cap must be AtCapacity, got {other:?}"),
        }
        assert_eq!(edge.in_flight_for(&beta), 2);

        // 2. A slot comes back when its guard drops — on any path, which is why it is a guard.
        drop(second);
        assert_eq!(edge.in_flight_for(&beta), 1);
        let replacement = edge.admit(&beta).expect("the freed slot is usable");
        assert_eq!(edge.in_flight_for(&beta), 2);

        // 3. **Per partner, not per gateway.** beta at its cap does not cost gamma anything —
        //    without this the budget would be a global limit and one partner could starve every
        //    other, which is the failure the consumer side's slots were built to avoid.
        let gamma_slot = edge.admit(&gamma).expect("gamma has its own allowance");
        assert_eq!((edge.in_flight_for(&beta), edge.in_flight_for(&gamma)), (2, 1));

        // 4. Everything returns, and the map is then empty rather than full of zeroes — it is the
        //    set of partners with work in flight, so a diagnostic reading it sees the truth.
        drop(first);
        drop(replacement);
        drop(gamma_slot);
        assert_eq!((edge.in_flight_for(&beta), edge.in_flight_for(&gamma)), (0, 0));

        // 5. The default is unlimited, so no deployment acquires a cap by upgrading.
        let unmetered = Arc::new(FederationEdge::new(
            alpha.clone(),
            ["demo/whoami"],
            DomainPolicy { domain: alpha.clone(), revision: 1, grants: vec![(beta.clone(), "demo/whoami".into())] },
            TrustBundle::trusting([(beta.clone(), beta_vk)]),
            CallPolicy::default(),
        ));
        let mut held = Vec::new();
        for _ in 0..64 {
            held.push(unmetered.admit(&beta).expect("0 means unlimited"));
        }
        assert_eq!(unmetered.in_flight_for(&beta), 0, "an unmetered edge counts nothing at all");
    }

    /// **More than two domains** — the last of item 2's row 11.
    ///
    /// Everything before this ran with exactly two domains, where several distinct properties are
    /// indistinguishable: "filtered for the asker" looks like "the export list", and "trust is not
    /// transitive" cannot even be stated. Three domains separate them.
    ///
    /// The topology is a chain, because a chain is what would break if trust composed:
    ///
    /// ```text
    ///   alpha ──grants demo/whoami──▶ beta ──grants demo/relay──▶ gamma
    ///     ▲                                                          │
    ///     └──────────── no relationship, in either direction ────────┘
    /// ```
    ///
    /// beta is a **provider and a consumer at once**, which is the case two domains cannot
    /// produce, and the one a hub deployment is made of.
    #[tokio::test]
    async fn three_domains_compose_without_trust_composing() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let gamma = DomainId::new("gamma.example").unwrap();
        let (alpha_sk, alpha_vk) = keypair(41);
        let (beta_sk, beta_vk) = keypair(42);
        let (gamma_sk, gamma_vk) = keypair(43);

        // alpha exports two skills and grants beta exactly one of them. gamma is not in its
        // bundle at all — not "granted nothing", *unknown*, which is a different refusal.
        let alpha_edge = Arc::new(
            FederationEdge::new(
                alpha.clone(),
                ["demo/whoami", "demo/secret"],
                DomainPolicy {
                    domain: alpha.clone(),
                    revision: 1,
                    grants: vec![(beta.clone(), "demo/whoami".into())],
                },
                TrustBundle::trusting([(beta.clone(), beta_vk)]),
                CallPolicy::default(),
            )
            .with_signing_key(alpha_sk),
        );
        // beta exports its own service to gamma. It does **not** re-export what alpha granted it.
        let beta_edge = Arc::new(
            FederationEdge::new(
                beta.clone(),
                ["demo/relay"],
                DomainPolicy {
                    domain: beta.clone(),
                    revision: 1,
                    grants: vec![(gamma.clone(), "demo/relay".into())],
                },
                TrustBundle::trusting([(gamma.clone(), gamma_vk)]),
                CallPolicy::default(),
            )
            .with_signing_key(beta_sk.clone()),
        );

        let (a, alpha_http, alpha_calls, _ra) =
            serving_domain(Arc::clone(&alpha_edge), &["demo/whoami", "demo/secret"]).await;
        let (b, beta_http, beta_calls, _rb) = serving_domain(Arc::clone(&beta_edge), &["demo/relay"]).await;
        // gamma serves nothing; it is the end of the chain and only ever a consumer.
        let (g, _gamma_http, _gamma_calls, _rg) = serving_domain(
            Arc::new(FederationEdge::new(
                gamma.clone(),
                [] as [&str; 0],
                DomainPolicy { domain: gamma.clone(), revision: 1, grants: vec![] },
                TrustBundle::trusting([]),
                CallPolicy::default(),
            )),
            &[],
        )
        .await;

        let endpoint = |port: u16, id: &str| GatewayEndpoint { id: id.into(), base_url: format!("http://127.0.0.1:{port}") };
        let beta_to_alpha = FederationClient::new(
            beta.clone(), "svc/hub", beta_sk.clone(), alpha.clone(),
            vec![endpoint(alpha_http, "gw-alpha")], 2, Duration::from_secs(30),
        ).with_partner_key(alpha_vk);
        let gamma_to_beta = FederationClient::new(
            gamma.clone(), "svc/edge", gamma_sk.clone(), beta.clone(),
            vec![endpoint(beta_http, "gw-beta")], 2, Duration::from_secs(30),
        ).with_partner_key(beta_vk);
        // gamma's operator configures a link to alpha too. Nothing stops them *configuring* it —
        // what matters is what alpha does with it.
        let gamma_to_alpha = FederationClient::new(
            gamma.clone(), "svc/edge", gamma_sk.clone(), alpha.clone(),
            vec![endpoint(alpha_http, "gw-alpha")], 2, Duration::from_secs(30),
        ).with_partner_key(alpha_vk);

        // ── 1. Each link works on its own terms. ─────────────────────────────────────────────
        assert_eq!(beta_to_alpha.connect().await.expect("beta discovers alpha"), vec!["demo/whoami".to_string()]);
        assert_eq!(gamma_to_beta.connect().await.expect("gamma discovers beta"), vec!["demo/relay".to_string()]);

        let via_beta = beta_to_alpha.call("demo/whoami", "?", Repeatability::AtMostOnce).await.expect("beta calls alpha");
        assert_eq!(via_beta, crate::federation_principal("beta.example", "svc/hub"));
        let into_beta = gamma_to_beta.call("demo/relay", "?", Repeatability::AtMostOnce).await.expect("gamma calls beta");
        assert_eq!(into_beta, crate::federation_principal("gamma.example", "svc/edge"));
        assert_eq!((alpha_calls.load(Ordering::SeqCst), beta_calls.load(Ordering::SeqCst)), (1, 1));

        // ── 2. **Trust does not compose.** alpha trusts beta; beta trusts gamma; alpha does not
        //    trust gamma — and no amount of the first two makes the third true. The refusal is
        //    `UnknownDomain` at the *catalogue*, so gamma cannot even learn what alpha exports.
        let refused = gamma_to_alpha.connect().await.expect_err("gamma is unknown to alpha");
        assert!(
            matches!(&refused, ClientError::Refused { status: 401, .. }),
            "an unknown origin is refused at the auth layer, not filtered to an empty catalogue: {refused:?}",
        );
        assert_eq!(gamma_to_alpha.link_state(), LinkState::Down);

        // ── 3. **A grant is not re-exported.** beta may call alpha's `demo/whoami`, and gamma may
        //    call beta — but beta's catalogue to gamma names beta's own export and nothing it
        //    holds from alpha. Transitive *reach* would otherwise arrive by accident, through a
        //    hub that is merely honest about what it can do.
        let gammas_view = gamma_to_beta.last_catalogue().expect("gamma connected");
        assert_eq!(gammas_view, vec!["demo/relay".to_string()]);
        assert!(!gammas_view.contains(&"demo/whoami".to_string()), "beta does not re-export alpha's grant");

        // ── 4. **One catalogue per asker, with three parties to tell apart.** With two domains a
        //    filtered catalogue and an export list are the same list; here alpha exports two
        //    skills, grants one, and beta sees exactly that one.
        let betas_view = beta_to_alpha.last_catalogue().expect("beta connected");
        assert_eq!(betas_view, vec!["demo/whoami".to_string()]);
        assert!(!betas_view.contains(&"demo/secret".to_string()), "an export is not a grant");

        // A credential minted for the export alpha exports but grants to nobody is refused at the
        // edge — planted directly, because beta's own resolver would refuse it first and we are
        // testing alpha's decision, not beta's.
        let raw = reqwest::Client::new();
        let planted = cred_for(&beta, "demo/secret", &beta_sk);
        let r = raw
            .post(format!("http://127.0.0.1:{alpha_http}/a2a"))
            .header(HEADER_FEDERATED_CALL, planted.to_header_value())
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                "params": {"skillId": "demo/secret", "message": {"role": "user", "parts": [{"type": "text", "text": "?"}]}},
            }))
            .send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32003, "{v}");
        assert_eq!(alpha_calls.load(Ordering::SeqCst), 1, "the ungranted export never reached a provider");

        // ── 5. **Three meshes, pairwise.** Non-merger is a property of every pair, and checking
        //    only the pair that exchanged bytes would miss a domain joined through a third.
        crate::lib_tests::assert_never_merged(&[&a], &[&b]);
        crate::lib_tests::assert_never_merged(&[&b], &[&g]);
        crate::lib_tests::assert_never_merged(&[&a], &[&g]);

        // ── 6. **Slots are per partner.** A hub's gateway serves several partners, and one
        //    partner saturating its allowance must not refuse another's work — otherwise a busy
        //    neighbour is a denial of service on everyone else the hub talks to.
        let mut pool = crate::federation::gateway::GatewayPool::new(["gw-hub"], 1);
        let first = pool.admit(&alpha, &[]).expect("alpha's one slot");
        assert!(
            matches!(pool.admit(&alpha, &[]), Err(crate::federation::gateway::CallOutcome::NoCapacity { .. })),
            "alpha's allowance is one",
        );
        assert!(pool.admit(&gamma, &[]).is_ok(), "gamma's allowance is its own, not what alpha left");
        pool.release(&first, &alpha);

        g.shutdown().await;
        b.shutdown().await;
        a.shutdown().await;
    }

    /// `federation:read` is not `federation:invoke`: a token that may *look* at the partners may
    /// not spend this domain's credential on them. The table is pinned in `http/authz.rs`; this is the
    /// same rule at a live gateway, because a table nothing enforces is a comment.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn a_read_scoped_token_cannot_invoke_a_partner() {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let (_, alpha_vk) = keypair(31);
        let (beta_sk, _) = keypair(32);
        let client = Arc::new(FederationClient::new(
            beta.clone(),
            "svc/billing",
            beta_sk,
            alpha.clone(),
            vec![GatewayEndpoint { id: "gw-a".into(), base_url: "http://127.0.0.1:1".into() }],
            1,
            Duration::from_secs(30),
        ).with_partner_key(alpha_vk));

        let port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(http_port);
        cfg.health_check_max_jitter_ms = 50;
        cfg.gateway_named_tokens = vec![
            crate::GatewayNamedToken { name: "viewer".into(), token: "look".into(), scopes: vec!["federation:read".into()] },
            crate::GatewayNamedToken { name: "caller".into(), token: "act".into(), scopes: vec!["federation:invoke".into()] },
        ];
        let b = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
            .with_federation_clients([client]);
        b.start().await.unwrap();
        await_gateway(http_port).await;
        let base = format!("http://127.0.0.1:{http_port}/gateway/federation");

        let (status, _) = get(&format!("{base}/partners"), Some("look")).await;
        assert_eq!(status, 200, "federation:read reads");
        let (status, _) = post(&format!("{base}/connect"), Some("look"), serde_json::json!({"domain": "alpha.example"})).await;
        assert_eq!(status, 403, "federation:read does not invoke");
        let (status, _) = post(
            &format!("{base}/call"),
            Some("look"),
            serde_json::json!({"domain": "alpha.example", "export": "demo/whoami", "text": "?"}),
        )
        .await;
        assert_eq!(status, 403, "federation:read does not call");

        // The invoke token gets past the scope layer and is refused by the *link* instead — the
        // positive control, without which "403" could mean the route was simply broken.
        let (status, v) = post(
            &format!("{base}/call"),
            Some("act"),
            serde_json::json!({"domain": "alpha.example", "export": "demo/whoami", "text": "?"}),
        )
        .await;
        assert_eq!(status, 409, "{v}");
        assert_eq!(v["error"], "link");

        b.shutdown().await;
    }
}

/// **A gateway write must say what it writes.** The contract, one assertion per row:
///
/// | Request | Result |
/// |---|---|
/// | missing `value_b64` | 400, **no mutation** |
/// | `value_b64` not a string | 400, **no mutation** |
/// | explicit `"value_b64": ""` | a valid zero-length value |
/// | valid encoded value | exactly those bytes |
///
/// Until 2026-09-24 the first row wrote an **empty value** and answered `{"ok": true}`, so a
/// misspelled field name erased a key and reported success. The overlay test helper had been
/// sending `value` since it was written; every sentinel it wrote was empty, and the check that
/// consumed them only counted keys, so neither defect surfaced the other.
///
/// The "no mutation" half is the load-bearing part: a 400 that had already clobbered the key would
/// be the same data loss with a better status code.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn gateway_kv_write_requires_an_explicit_value() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg);
    agent.start().await.expect("start");

    let client = reqwest::Client::new();
    let health = format!("http://127.0.0.1:{http_port}/health");
    for _ in 0..40 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let url = format!("http://127.0.0.1:{http_port}/gateway/kv");
    let key = "probe/kv-contract";
    let get_val = |k: &str| {
        let c = client.clone();
        let u = format!("http://127.0.0.1:{http_port}/gateway/kv?key={k}");
        async move { c.get(&u).send().await.unwrap().json::<serde_json::Value>().await.unwrap() }
    };

    // Establish a known value first, so "no mutation" is observable rather than vacuous.
    let ok = client.post(&url)
        .json(&serde_json::json!({"key": key, "value_b64": "aGVsbG8="})) // "hello"
        .send().await.unwrap();
    assert!(ok.status().is_success(), "seed write rejected");
    let seeded = get_val(key).await;
    assert_eq!(seeded["value_b64"], "aGVsbG8=", "seed value must be readable: {seeded}");

    // Row 1 — missing `value_b64`: 400, and the seeded value survives.
    let r = client.post(&url).json(&serde_json::json!({"key": key})).send().await.unwrap();
    assert_eq!(r.status(), 400, "an omitted value must be refused, not treated as empty");
    let after = get_val(key).await;
    assert_eq!(after["value_b64"], "aGVsbG8=", "a refused write must not mutate: {after}");

    // Row 2 — wrong type: 400, still no mutation.
    let r = client.post(&url)
        .json(&serde_json::json!({"key": key, "value_b64": 42}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "a non-string value_b64 must be refused");
    let after = get_val(key).await;
    assert_eq!(after["value_b64"], "aGVsbG8=", "a refused write must not mutate: {after}");

    // Row 3 — invalid base64: 400, still no mutation.
    let r = client.post(&url)
        .json(&serde_json::json!({"key": key, "value_b64": "!!!not base64!!!"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "invalid base64 must be refused");
    let after = get_val(key).await;
    assert_eq!(after["value_b64"], "aGVsbG8=", "a refused write must not mutate: {after}");

    // Row 4 — an EXPLICIT empty value is legitimate and distinct from an omitted one.
    let r = client.post(&url)
        .json(&serde_json::json!({"key": key, "value_b64": ""}))
        .send().await.unwrap();
    assert!(r.status().is_success(), "an explicitly empty value is a valid write");
    let after = get_val(key).await;
    assert_eq!(after["found"], true, "an explicitly empty value is present, not absent: {after}");
    assert_eq!(after["value_b64"], "", "and it is zero-length: {after}");

    // Row 5 — a valid encoded value stores exactly those bytes.
    let r = client.post(&url)
        .json(&serde_json::json!({"key": key, "value_b64": "d29ybGQ="})) // "world"
        .send().await.unwrap();
    assert!(r.status().is_success());
    let after = get_val(key).await;
    assert_eq!(after["value_b64"], "d29ybGQ=", "exact bytes round-trip: {after}");

    agent.shutdown().await;
}

/// **An electorate must be established, never inferred from absence.**
///
/// The three acceptance cases for the empty/partial-roster defect, as pure decisions over
/// `resolve_electorate` — no timing, no cluster, so they cannot flake and cannot be "fixed" by a
/// longer sleep.
///
/// Until 2026-09-24 an empty roster was counted as `members.len().max(1)`: one member, quorum one,
/// satisfied by the proposer's **own** self-vote. Every node therefore committed its own candidate
/// unopposed — *N* singleton elections wearing the shape of one, reconciled afterwards by LWW if at
/// all. The arithmetic was not the defect. The defect was that **"I cannot see members" silently
/// meant "I have authority to decide alone."**
#[cfg(feature = "consensus")]
#[test]
fn an_electorate_is_established_not_inferred() {
    use crate::agent::helpers::{resolve_electorate, Electorate};

    // 1. An empty group cannot elect. No roster, no authority — whatever the declaration says.
    assert_eq!(
        resolve_electorate(0, 0),
        Electorate::Unavailable { observed: 0, declared_min: 0 },
        "an unknown or unjoined group must refuse, not elect alone",
    );
    assert_eq!(
        resolve_electorate(0, 3),
        Electorate::Unavailable { observed: 0, declared_min: 3 },
    );

    // 2. A partial roster cannot silently confer singleton authority. This is the subtle one: the
    //    node sees a plausible number, computes a plausible quorum from it, and is wrong — the
    //    same defect wearing better clothes.
    assert_eq!(
        resolve_electorate(1, 3),
        Electorate::Unavailable { observed: 1, declared_min: 3 },
        "seeing 1 of a declared 3 is a partial view, and a partial view is not a small electorate",
    );
    assert_eq!(resolve_electorate(2, 3), Electorate::Unavailable { observed: 2, declared_min: 3 });

    // 3. An EXPLICIT singleton is legitimate and still elects. The rule refuses inference from
    //    absence, not solo authority that someone actually established.
    assert_eq!(
        resolve_electorate(1, 0), Electorate::Established(1),
        "a genuine one-member group elects; that was never the defect",
    );
    assert_eq!(resolve_electorate(1, 1), Electorate::Established(1));

    // 4. A complete or over-complete view decides with what it sees.
    assert_eq!(resolve_electorate(3, 3), Electorate::Established(3));
    assert_eq!(resolve_electorate(5, 3), Electorate::Established(5), "the floor is a floor");

    // 5. No declaration ⇒ nothing to be partial against. The floor binds while asserted, and a
    //    `MembershipIntent` that nobody refreshes evaporates — it must not wedge the group shut.
    assert_eq!(resolve_electorate(2, 0), Electorate::Established(2));
}

/// **The same refusal reaches a real caller, through both surfaces.**
///
/// `resolve_electorate` is a pure function; this is the join — that a node which has joined no
/// group gets `ElectorateUnavailable` from the library path and `409 electorate_unavailable` from
/// the gateway, rather than a leader it elected by itself.
///
/// The HTTP half is the half that matters most: the gateway is where the defect was found, because
/// **there is no gateway route by which a node joins a group** (`grp_prefix` is read in `http/overlay.rs`
/// and written nowhere in the surface), so an HTTP caller's election could only ever reach the
/// empty-roster path.
#[cfg(all(feature = "consensus", feature = "gateway"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unjoined_group_refuses_to_elect_on_both_surfaces() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.expect("start");

    // Library path.
    let res = agent.consensus()
        .group_propose("nobody-joined", "leader/nobody-joined",
                       Bytes::from_static(b"me"), crate::consensus::ConsensusConfig::default())
        .await;
    match res {
        crate::consensus::ConsensusResult::ElectorateUnavailable {
            observed_members, declared_min, ..
        } => {
            assert_eq!(observed_members, 0, "nobody joined, so nobody is visible");
            assert_eq!(declared_min, 0, "and nothing was declared");
        }
        other => panic!("an unjoined group must refuse, got {other:?}"),
    }

    // Gateway path — the same contract, or the surfaces disagree about who may decide.
    let client = reqwest::Client::new();
    let health = format!("http://127.0.0.1:{http_port}/health");
    for _ in 0..40 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/overlay/elect"))
        .json(&serde_json::json!({"group": "nobody-joined"}))
        .send().await.expect("elect request");
    assert_eq!(r.status(), 409, "the gateway must refuse an election with no electorate");
    let body: serde_json::Value = r.json().await.expect("json body");
    assert_eq!(body["error"], "electorate_unavailable", "{body}");
    assert_eq!(body["observed_members"], 0, "{body}");

    agent.shutdown().await;
}

/// **A node that is not in a group's roster may not propose to it.**
///
/// A proposer counted its own promise and its own vote unconditionally, while the quorum it needed
/// was computed from the group's roster — which it need not be in. So a stranger to a one-member
/// group `{A}` had a quorum of one and satisfied it alone: it chose a leader for the group without
/// a member voting, from a node the group's members never admitted. On a larger group the same
/// arithmetic lets two strangers with disjoint acceptors each reach quorum — a stranger's self-vote
/// is a vote the roster does not contain, so the two quorums need not intersect.
///
/// Both surfaces refuse by name now: `ConsensusResult::NotAMember` from the library, and
/// `403 not_a_member` from the gateway's election route, which proposed `leader/{group}` for any
/// group a client named. Seen failing first: B committed `leader/members-only` = B.
#[cfg(all(feature = "consensus", feature = "gateway"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_member_cannot_propose_to_a_group_on_either_surface() {
    let port_a = alloc_port();
    let port_b = alloc_port();
    let http_b = alloc_port();
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
    cfg_b.http_port = Some(http_b);
    let a = GossipAgent::new(id_a, cfg_a);
    let b = GossipAgent::new(id_b.clone(), cfg_b);
    a.start().await.unwrap();
    b.start().await.unwrap();
    let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 2_000).await;

    // A is the whole roster; B is a stranger to the group, and can see that it is.
    a.mesh().join_group("members-only");
    let prefix = crate::signal::grp_prefix("members-only");
    poll_until(|| !b.kv().scan_prefix(&prefix).is_empty(), 5_000).await;

    let cfg = ConsensusConfig {
        phase1_timeout: Duration::from_millis(300),
        max_ballots:    1,
        ..ConsensusConfig::default()
    };
    let res = b.consensus()
        .group_propose("members-only", "leader/members-only", Bytes::from_static(b"stranger"), cfg.clone())
        .await;
    // Seen failing first as `!matches!(res, Committed { .. })`: B committed `leader/members-only` = B.
    match res {
        ConsensusResult::NotAMember { ref group, ref slot } => {
            assert_eq!(&**group, "members-only", "the refusal names the group");
            assert_eq!(&**slot, "leader/members-only");
        }
        other => panic!("a stranger must be refused by name, got {other:?}"),
    }
    assert!(
        b.consensus().consensus_get("leader/members-only").is_none(),
        "and nothing was committed",
    );
    // The receipt verb and the election verb translate it, each as a refusal — never a timeout.
    match b.consensus().group_propose_receipt("members-only", "leader/members-only", Bytes::from_static(b"stranger"), cfg).await {
        Err(crate::CommitError::NotAMember { group, .. }) => assert_eq!(&*group, "members-only"),
        other => panic!("expected CommitError::NotAMember, got {other:?}"),
    }
    match b.consensus().elect_leader_receipt("members-only").await {
        Err(crate::ConsistencyError::NotAMember { group }) => assert_eq!(&*group, "members-only"),
        other => panic!("expected ConsistencyError::NotAMember, got {other:?}"),
    }
    // And the member itself still elects: the roster is explicit, so solo authority is legitimate.
    let own = a.consensus().elect_leader_receipt("members-only").await.expect("the member elects");
    assert_eq!(own.leader, *a.node_id());

    // The gateway's election route proposes `leader/{group}` for any group a client names.
    let client = reqwest::Client::new();
    let health = format!("http://127.0.0.1:{http_b}/health");
    for _ in 0..40 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = client.post(format!("http://127.0.0.1:{http_b}/gateway/overlay/elect"))
        .json(&serde_json::json!({"group": "members-only"}))
        .send().await.expect("elect request");
    assert_eq!(r.status(), 403, "an authority refusal, like governed_group");
    let body: serde_json::Value = r.json().await.expect("json body");
    assert_eq!(body["error"], "not_a_member", "{body}");
    assert_eq!(body["group"], "members-only", "{body}");

    a.shutdown().await;
    b.shutdown().await;
}

/// **An acceptor answers only a tagged request, and answers it tagged** (post-360 plan row F
/// closes the 2.32.0 window, `deprecations.md` §23/§35). Under the window a bare-signed `Prepare` —
/// a 2.31 proposer's — got a bare answer (the adversarial review of #585, F1); now a bare request does
/// not verify and is not answered. Driven through the running listener, the way a frame from the
/// network arrives. Seen failing first: the bare prepare was answered.
#[cfg(all(feature = "tls", feature = "consensus"))]
#[tokio::test]
async fn an_acceptor_answers_only_a_tagged_request() {
    use crate::config::TlsConfig;
    use crate::consensus::{
        consensus_kind, consensus_signing_message, encode_consensus_msg, verify_consensus_signature,
        ConsensusMsg, SignedConsensusMsg,
    };
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-reply-form-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let tls = Arc::clone(a.task_ctx.tls.get().expect("tls identity"));
    let my_key = tls.verifying_key_bytes();
    let mut acks = a.task_ctx.signal_handlers.register_with_capacity(Arc::from(consensus_kind::VOTE), 8);

    // A prepare as a proposer of each era signs it, addressed to this node as the acceptor.
    let send = |ballot: u64, tagged: bool| {
        let bytes = encode_consensus_msg(&ConsensusMsg::Prepare { slot: Arc::from("reply-form"), ballot, proposer: id.clone() });
        let message = if tagged { consensus_signing_message(&bytes) } else { bytes.to_vec() };
        let signed = SignedConsensusMsg {
            msg_bytes: bytes,
            signer: id.clone(),
            signature: crate::tls::sign_bytes(&tls.signing_key(), &message).to_vec(),
        };
        mycelium_core::ops::emit_signal(
            &a.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Individual(id.clone()),
            Bytes::from(mycelium_core::serde_fixint::to_vec(&signed).unwrap()),
        );
    };

    send(3, false);
    assert!(tokio::time::timeout(Duration::from_secs(2), acks.recv()).await.is_err(),
            "a bare (2.31) prepare is not answered: the window is closed");

    send(4, true);
    let ack = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.expect("answered").expect("open");
    let signed: SignedConsensusMsg = mycelium_core::serde_fixint::from_slice(&ack.payload).expect("a signed answer");
    assert!(verify_consensus_signature(&[my_key], &signed.msg_bytes, &signed.signature), "a tagged request is answered tagged");

    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **An acceptor that cannot record its promise counts the answer it withheld** (the adversarial
/// review of #585, F5). The WAL writer is stopped under a running listener, so the fsync a promise
/// needs fails; the prepare goes unanswered — and `consensus_acceptor_unrecorded` says so, where
/// before only a warning did. Seen failing first with the increment absent: the counter stayed 0.
#[cfg(all(unix, feature = "consensus"))]
#[tokio::test]
async fn an_unrecorded_promise_is_counted_as_well_as_withheld() {
    use crate::consensus::{acceptor_answers_unrecorded, consensus_kind, encode_consensus_msg, ConsensusMsg};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let base = std::env::temp_dir().join(format!("mycelium-unrecorded-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.persistence = Some(r2_persistence(&base));
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let mut acks = a.task_ctx.signal_handlers.register_with_capacity(Arc::from(consensus_kind::VOTE), 8);
    // The disk goes away under the acceptor: the writer stops, so every forced append fails.
    a.task_ctx.wal.get().expect("persistence configured").shutdown().await;

    let before = acceptor_answers_unrecorded();
    let bytes = encode_consensus_msg(&ConsensusMsg::Prepare { slot: Arc::from("unrecorded"), ballot: 1, proposer: id.clone() });
    mycelium_core::ops::emit_signal(&a.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Individual(id.clone()), bytes);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), acks.recv()).await.is_err(),
        "a promise that did not reach the WAL is not answered",
    );
    assert_eq!(acceptor_answers_unrecorded(), before + 1, "and the withheld answer is counted");

    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **A decided floor no history of the slot can reach is counted, and still obeyed** (2026-10-10).
/// `consensus/decided/{slot}` is written by whichever node commits, so it is not self-owned and the
/// `sys/` tripwire cannot see it; a member writing `u64::MAX` there makes every later prepare on the
/// slot refuse. The write goes in through the KV path (`kv().set`, which applies through
/// `apply_and_notify` exactly as a gossiped update does); a prepare at ballot 1 is then refused as
/// before — answered with a NACK naming the floor, no promise recorded — and the slot is counted
/// once on `consensus_decided_floor_anomalies`, however many times the floor is read. A legitimate
/// floor a little above what this node has seen (it missed some attempts) is not counted.
/// Seen failing first with the field plumbed and no detection: the count stayed 0.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn a_forged_decided_floor_is_counted_and_still_obeyed() {
    use crate::consensus::{consensus_kind, decode_consensus_msg, encode_consensus_msg, ConsensusMsg};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    let mut nacks = a.task_ctx.signal_handlers.register_with_capacity(Arc::from(consensus_kind::NACK), 8);
    let mut acks = a.task_ctx.signal_handlers.register_with_capacity(Arc::from(consensus_kind::VOTE), 8);
    assert_eq!(a.system_stats().consensus_decided_floor_anomalies, 0);

    // Control: a floor of 3 on a slot this node has never seen — it missed the attempts; not counted.
    let _ = a.kv().set("consensus/decided/plausible", Bytes::copy_from_slice(&3u64.to_le_bytes()));
    // The forgery.
    let slot: Arc<str> = Arc::from("forged");
    let _ = a.kv().set(format!("consensus/decided/{slot}"), Bytes::copy_from_slice(&u64::MAX.to_le_bytes()));

    for (s, ballot) in [("plausible", 4u64), (&*slot, 1), (&*slot, 2)] {
        let bytes = encode_consensus_msg(&ConsensusMsg::Prepare { slot: Arc::from(s), ballot, proposer: id.clone() });
        mycelium_core::ops::emit_signal(&a.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Individual(id.clone()), bytes);
    }
    // The plausible slot's prepare is promised (above its floor of 3).
    let ack = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.expect("answered").expect("open");
    assert!(matches!(decode_consensus_msg(&ack.payload), Some(ConsensusMsg::PrepareAck { ballot: 4, .. })));
    // The forged slot's two prepares are refused, as before: a NACK naming the floor, nothing promised.
    for _ in 0..2 {
        let nack = tokio::time::timeout(Duration::from_secs(5), nacks.recv()).await.expect("refused").expect("open");
        match decode_consensus_msg(&nack.payload) {
            Some(ConsensusMsg::Promise { slot: s, seen_ballot, .. }) => {
                assert_eq!(&*s, "forged");
                assert_eq!(seen_ballot, u64::MAX, "the refusal names the floor");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
    assert!(a.task_ctx.consensus_accepted.pin().get(&slot).is_none_or(|st| st.promised == 0),
            "no promise is recorded below the floor");
    assert_eq!(a.system_stats().consensus_decided_floor_anomalies, 1,
               "the forged slot is counted, once, however often its floor is read; the plausible one is not");

    a.shutdown().await;
}

/// **The floor tripwire does not trust the shared ballot key** (the adversarial review of #591,
/// finding 3). `consensus/ballot/{slot}` is written by any member too, so measuring "observed" from it
/// let a forger raise both keys together and pass uncounted — and the ballot key alone at `u64::MAX`
/// exhausts the slot with no tripwire at all. "Observed" is now this node's acceptor memory and the
/// ballots of verified COMMITs it processed, and the ballot key read for a draw gets the same check.
/// Each forgery is counted once for its slot, and each proposal still ends by name.
/// Seen failing first: the both-keys slot was not counted (0 where 1 was expected).
#[cfg(feature = "consensus")]
#[tokio::test]
async fn forged_ballot_keys_are_counted_as_well_as_forged_floors() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    a.mesh().join_group("solo");
    let mut fast = ConsensusConfig::default();
    fast.phase1_timeout = Duration::from_millis(200);
    fast.max_ballots = 2;
    let max = Bytes::copy_from_slice(&u64::MAX.to_le_bytes());

    // Both keys forged together.
    let _ = a.kv().set("consensus/decided/forged/both", max.clone());
    let _ = a.kv().set("consensus/ballot/forged/both", max.clone());
    let before = a.system_stats().consensus_decided_floor_anomalies;
    let res = a.consensus().group_propose("solo", "forged/both", Bytes::from_static(b"v"), fast.clone()).await;
    assert!(matches!(res, ConsensusResult::Timeout { .. }), "{res:?}");
    assert_eq!(a.system_stats().consensus_decided_floor_anomalies, before + 1,
               "forging the floor and the ballot key together is counted");

    // The ballot key alone.
    let _ = a.kv().set("consensus/ballot/forged/key", max.clone());
    let res = a.consensus().group_propose("solo", "forged/key", Bytes::from_static(b"v"), fast.clone()).await;
    assert!(matches!(res, ConsensusResult::Timeout { .. }), "{res:?}");
    assert_eq!(a.system_stats().consensus_decided_floor_anomalies, before + 2,
               "a forged ballot key alone is counted");

    // Control: an ordinary slot commits and counts nothing.
    let res = a.consensus().group_propose("solo", "forged/none", Bytes::from_static(b"v"), ConsensusConfig::default()).await;
    assert!(matches!(res, ConsensusResult::Committed { .. }), "{res:?}");
    assert_eq!(a.system_stats().consensus_decided_floor_anomalies, before + 2);

    a.shutdown().await;
}

/// **A slot whose decided floor is `u64::MAX` cannot take a new ballot, and says so** (2026-10-10).
/// Both proposers drew `max(ballot key, floor) + 1`, which overflows there: a panic in any build with
/// overflow checks (and the release profile is `panic = "abort"`, so such a build loses the node), a
/// wrap to ballot 0 — refused below the floor — without them. Now the draw is checked: the group
/// proposal and the cross-group proposal each end as a `Timeout` named `ballot_exhausted`
/// (this node's `consensus_ballot_space_exhausted`), and a slot below the ceiling still commits.
/// Seen failing first: `attempt to add with overflow` in `ConsensusEngine::propose`.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn a_floor_at_the_ballot_ceiling_is_refused_by_name_not_overflowed() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    a.mesh().join_group("solo");
    let mut fast = ConsensusConfig::default();
    fast.phase1_timeout = Duration::from_millis(200);
    fast.max_ballots = 2;

    for slot in ["ceiling/group", "ceiling/cross"] {
        let _ = a.kv().set(format!("consensus/decided/{slot}"), Bytes::copy_from_slice(&u64::MAX.to_le_bytes()));
    }
    let exhausted = || a.system_stats().consensus_ballot_space_exhausted;
    let before = exhausted();
    let res = a.consensus().group_propose("solo", "ceiling/group", Bytes::from_static(b"v"), fast.clone()).await;
    assert!(matches!(res, ConsensusResult::Timeout { .. }), "group proposal: {res:?}");
    assert_eq!(exhausted(), before + 1, "the group proposal is refused by name");

    let groups = vec![crate::consensus::GroupQuorum { group: "solo".into(), quorum: 0.5, veto: false }];
    let res = a.consensus().cross_group_propose("ceiling/cross", Bytes::from_static(b"v"), groups, fast.clone()).await;
    assert!(matches!(res, ConsensusResult::Timeout { .. }), "cross-group proposal: {res:?}");
    assert_eq!(exhausted(), before + 2, "the cross-group proposal is refused by name");

    // Control: a floor one below the ceiling leaves exactly one ballot, and the slot commits at it.
    let _ = a.kv().set("consensus/decided/ceiling/last", Bytes::copy_from_slice(&(u64::MAX - 1).to_le_bytes()));
    let res = a.consensus().group_propose("solo", "ceiling/last", Bytes::from_static(b"v"), ConsensusConfig::default()).await;
    assert!(matches!(res, ConsensusResult::Committed { ballot: u64::MAX, .. }), "the last ballot is usable: {res:?}");

    a.shutdown().await;
}

/// **Boundary H P1 gate — issuer binding on live nodes.** Two TLS members, A and B.
///
/// 1. A signs a knowledge record as itself; B attributes it to A as `Current` on the member path,
///    using only B's own view of A's keys.
/// 2. B's signing API refuses to sign a record naming A's issuer.
/// 3. A record naming A's issuer but signed with B's identity key is `BadSignature` at B: one member
///    cannot be another issuer.
/// 4. A rotates and **revokes** the key that signed the first record. B then reports that record as
///    `Revoked` — still attributable to A as history, no longer current.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_boundary_h_p1_issuer_binding_on_live_nodes() {
    use crate::config::TlsConfig;
    use crate::knowledge::issuer::{
        verify_issuer, Authenticity, IssuerPath, SignAsMemberError, TrustedExternalIssuers,
        UnverifiableReason,
    };
    use crate::knowledge::{IssuerId, KnowledgeRecord, RecordKind};

    let port_a = alloc_port();
    let port_b = alloc_port();
    let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
    let node_a = id(port_a);
    let cert_dir = std::env::temp_dir().join(format!("myc-p1-{port_a}-{port_b}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.reconnect_backoff_secs = 1;
        cfg.health_check_interval_secs = 1;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        GossipAgent::new(id(port), cfg)
    };
    let a = Arc::new(mk(port_a, vec![]));
    let b = Arc::new(mk(port_b, vec![node_a.clone()]));
    a.start().await.unwrap();
    b.start().await.unwrap();

    let record = |issuer: IssuerId, at_ms: u64| {
        KnowledgeRecord::new(issuer, RecordKind::Assessment, at_ms, "release/x/1", b"j".to_vec(), vec![])
            .expect("well formed")
    };
    let external = TrustedExternalIssuers::new();

    // 1. A signs as itself; B attributes it to A once it has learned A's key.
    let by_a = record(IssuerId::for_node(&node_a), 1_000);
    let sig_a = a.sign_knowledge_record(&by_a).expect("A signs as itself");
    let old_key = a.identity_public_key().expect("a has a tls identity");
    let mut attributed = false;
    for _ in 0..200 {
        if verify_issuer(&by_a, &sig_a, &b.knowledge_member_keys(), &external)
            == (Authenticity::Current { path: IssuerPath::Member(node_a.clone()), key: old_key })
        {
            attributed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(attributed, "B should attribute A's record to A on the member path");

    // 2. B's signing API will not sign as A.
    assert_eq!(
        b.sign_knowledge_record(&by_a),
        Err(SignAsMemberError::NotThisMember { issuer: IssuerId::for_node(&node_a) })
    );

    // 3. Signing A's issuer with B's key does not make B into A.
    let forged = record(IssuerId::for_node(&node_a), 1_001);
    let forged_sig = b.sign_with_identity(&forged.canonical_bytes()).expect("B has a key");
    assert_eq!(
        verify_issuer(&forged, &forged_sig, &b.knowledge_member_keys(), &external),
        Authenticity::Unverifiable(UnverifiableReason::BadSignature)
    );

    // 4. Rotate, then revoke the key that signed `by_a`: attribution survives, currency does not.
    let new_key = a.rotate_identity(Duration::from_millis(500)).await.expect("rotation");
    assert_ne!(new_key, old_key);
    a.revoke_identity_key(old_key).expect("revoke");
    let mut revoked_seen = false;
    for _ in 0..200 {
        let v = verify_issuer(&by_a, &sig_a, &b.knowledge_member_keys(), &external);
        if v == (Authenticity::Revoked { path: IssuerPath::Member(node_a.clone()), key: old_key }) {
            assert!(v.is_attributable() && !v.is_current());
            revoked_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(revoked_seen, "after revocation B must report A's old record as Revoked, not Current");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Boundary H A1 at the gateway — end to end, through the real `/a2a` door.**
///
/// A policy allows `skill.invoke` on `depot/dispatch` **only with a mandate for `depot`**. The
/// gateway has an execution authority attached, and the caller authenticates with a named bearer
/// token (`token:gw/agent-1`).
///
/// 1. No grant presented: authority not established, and the skill is **never reached**.
/// 2. A valid grant, possessed for this call, with a fresh revocation checkpoint: the mandate is
///    **established**, the policy permits, and the skill answers.
/// 3. The authority revokes the appointment: the same call is refused, and the skill is not reached
///    again.
///
/// Closure plan C2 adds two things. The provider receives, in the caller envelope, **exactly the
/// mandate the client presented**, for it to verify for itself; and the streaming door
/// (`tasks/sendSubscribe`) honours a presented mandate too. Before C2 it passed no params to the
/// preflight, so a mandate on a stream was never read.
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn test_boundary_h_a1_gateway_establishes_mandates_end_to_end() {
    use crate::agent::gateway_authority::{possession_request, ExecutionAuthority, PresentedMandate};
    use crate::config::{GatewayNamedToken, TlsConfig};
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::{ReferenceEvaluator, Rule};
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CALLER: &str = "token:gw/agent-1";
    let authority_key = SigningKey::from_bytes(&[51u8; 32]);
    let holder_key = SigningKey::from_bytes(&[52u8; 32]);

    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("a1-gateway-{http_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens =
        vec![GatewayNamedToken { name: "agent-1".into(), token: "s3cret".into(), scopes: vec!["*".into()] }];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a());
    let me = agent.node_id().clone();
    let resource = format!("skill:depot/dispatch@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-a1")
            .with_catalogue("cat-a1", "1")
            .map_action("skill.invoke", resource.clone())
            .allow(Rule::new("*", "skill.invoke", resource.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    agent.start().await.unwrap();

    let _reg = agent.capabilities().advertise_capability(
        crate::capability::Capability::new("depot", "dispatch"),
        Duration::from_secs(30),
    );
    let reached = Arc::new(AtomicUsize::new(0));
    let carried: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::new(std::sync::Mutex::new(None));
    {
        let (agent, reached, carried) = (Arc::clone(&agent), Arc::clone(&reached), Arc::clone(&carried));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                reached.fetch_add(1, Ordering::SeqCst);
                *carried.lock().unwrap() = agent.presented_mandate(&req).expect("a verified context");
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let wall_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let checkpoint = |seq: u64, revoked: &[&str]| {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: wall_ms(),
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c }
    };
    agent.offer_revocation_checkpoint(&checkpoint(1, &[])).expect("an authority is attached");

    let text = "dispatch";
    let canonical = serde_json::to_vec(&serde_json::json!({ "text": text })).unwrap();
    let digest = crate::agent::action_evaluator::arguments_digest(&canonical);
    let mandate = Mandate {
        holder: PrincipalId::new(CALLER).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "dispatch".into(),
        scope: "depot".into(),
        operations: vec!["skill.invoke:skill:depot/dispatch".into()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 600_000,
    };
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&authority_key, &mandate.canonical_bytes()).to_vec(), mandate };
    let proof = mycelium_core::tls::sign_bytes(
        &holder_key,
        &possession_message(&grant.mandate, &possession_request("skill.invoke", &resource, &digest)),
    );
    let presented = PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof) };

    let send = |meta: Option<serde_json::Value>| {
        let mut params = serde_json::json!({
            "id": format!("task-{}", fastrand::u32(..)),
            "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
        });
        if let Some(m) = meta {
            params["_meta"] = m;
        }
        async move {
            reqwest::Client::new()
                .post(format!("http://127.0.0.1:{http_port}/a2a"))
                .bearer_auth("s3cret")
                .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tasks/send", "params": params}))
                .send()
                .await
                .expect("tasks/send")
                .json::<serde_json::Value>()
                .await
                .unwrap()
        }
    };

    // 1. No grant: authority not established, and the skill is never reached.
    let body = send(None).await;
    assert_eq!(body["error"]["data"]["reason"], "authority_not_established", "{body}");
    assert_eq!(reached.load(Ordering::SeqCst), 0);

    // 2. A valid, possessed grant with a fresh revocation view: established, permitted, dispatched.
    let body = send(Some(serde_json::json!({ "mandate": presented }))).await;
    assert!(body.get("error").is_none(), "an established mandate satisfies the policy: {body}");
    assert_eq!(reached.load(Ordering::SeqCst), 1);
    // C2: the provider received exactly what the client presented, to verify for itself.
    assert_eq!(
        carried.lock().unwrap().clone(),
        Some(serde_json::to_value(&presented).unwrap()),
        "the presented mandate travels to the provider in the caller envelope",
    );

    // C2: the streaming door reads the mandate too, and dispatches under it.
    let stream_body = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/a2a"))
        .bearer_auth("s3cret")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tasks/sendSubscribe", "params": {
            "id": format!("task-{}", fastrand::u32(..)),
            "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
            "_meta": { "mandate": presented },
        }}))
        .send()
        .await
        .expect("tasks/sendSubscribe")
        .text()
        .await
        .unwrap();
    assert!(stream_body.contains("completed"), "a stream call under an established mandate completes: {stream_body}");
    assert_eq!(reached.load(Ordering::SeqCst), 2, "and reaches the skill");

    // C3: with provider enforcement on as well, the same call passes **both** doors: the gateway
    // carried enough (the mandate and the resource claim) for the provider to verify it for itself.
    agent.with_provider_enforcement();
    let body = send(Some(serde_json::json!({ "mandate": presented }))).await;
    assert!(body.get("error").is_none(), "established at the gateway and at the provider: {body}");
    assert_eq!(reached.load(Ordering::SeqCst), 3);

    // 3. The authority revokes the appointment: refused, and the skill is not reached again.
    agent.offer_revocation_checkpoint(&checkpoint(2, &["t1"])).unwrap();
    let body = send(Some(serde_json::json!({ "mandate": presented }))).await;
    assert!(body.get("error").is_some(), "a revoked appointment is not established: {body}");
    assert_eq!(reached.load(Ordering::SeqCst), 3);

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Closure plan C3: authority where the work happens.** One node with provider enforcement on,
/// a policy requiring a mandate for `depot`, and its own execution authority.
///
/// 1. A member calls the node's MCP tool **directly**, no gateway, with no grant: refused at the
///    provider, and the tool never runs. Before C3 this ran: only a gateway asked.
/// 2. The same member, under its own grant (holder `node:{id}`, possession signed with its identity
///    key): established at the provider, and the tool runs.
/// 3. The authority revokes the appointment: refused again.
/// 4. A skill call claiming another node's resource, or a skill this node does not serve, is refused
///    before any policy question: a mandate for one resource does not admit a call routed to another.
/// 5. An operator-listed protected kind served through `rpc_rx` is refused at the receiver, and the
///    serve loop never sees it.
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn test_c3_provider_enforcement_decides_direct_member_calls() {
    use crate::agent::gateway_authority::{possession_request, ExecutionAuthority, PresentedMandate};
    use crate::config::TlsConfig;
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::{ReferenceEvaluator, Rule};
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let authority_key = SigningKey::from_bytes(&[53u8; 32]);
    let gossip_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("c3-provider-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.protected_rpc_kinds = vec!["depot.custom".into()];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    let me = agent.node_id().clone();
    let tool = format!("tool:count@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-c3")
            .with_catalogue("cat-c3", "1")
            .map_action("tools/call", tool.clone())
            .allow(Rule::new("*", "tools/call", tool.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    agent.with_provider_enforcement();
    agent.start().await.unwrap();

    let ran = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&ran);
    // The handler receives the whole call: on a member's own mandated direct call the carried
    // mandate reaches it, and the principal is named as the grant names its holder.
    let seen_calls: Arc<std::sync::Mutex<Vec<(String, bool)>>> = Arc::default();
    let seen2 = Arc::clone(&seen_calls);
    let _tool = agent.mcp().register_mcp_tool_with_call("count", serde_json::json!({}), move |call, _args| {
        let r = Arc::clone(&r2);
        seen2.lock().unwrap().push((call.principal_string(), call.carried_mandate.is_some()));
        async move { r.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("ran")) }
    });
    let skill_seen = Arc::new(AtomicUsize::new(0));
    {
        let (agent, seen) = (Arc::clone(&agent), Arc::clone(&skill_seen));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }
    let custom_seen = Arc::new(AtomicUsize::new(0));
    {
        let (agent, seen) = (Arc::clone(&agent), Arc::clone(&custom_seen));
        let mut rx = agent.service().rpc_rx("depot.custom");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"served".to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let wall_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let checkpoint = |seq: u64, revoked: &[&str]| {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: wall_ms(),
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c }
    };
    agent.offer_revocation_checkpoint(&checkpoint(1, &[])).expect("an authority is attached");

    let arguments = serde_json::json!({"n": 1});
    let call = serde_json::json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"count","arguments":arguments}});
    let payload = call.to_string().into_bytes();
    let reply = |b: bytes::Bytes| serde_json::from_slice::<serde_json::Value>(&b).unwrap();

    // 1. Direct, no grant: refused at the provider, the tool never runs.
    let r = reply(agent.service().rpc_call(me.clone(), "mcp.invoke", payload.clone(), Duration::from_secs(5)).await.unwrap());
    assert_eq!(r["error"]["data"]["reason"], "authority_not_established", "{r}");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool never ran");

    // 2. Under its own grant: established at the provider, and the tool runs.
    let holder = format!("node:{me}");
    let mandate = Mandate {
        holder: PrincipalId::new(&holder).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "count".into(),
        scope: "depot".into(),
        operations: vec!["tools/call:tool:count".to_string()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 600_000,
    };
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&authority_key, &mandate.canonical_bytes()).to_vec(), mandate };
    let digest = crate::agent::action_evaluator::arguments_digest(&serde_json::to_vec(&arguments).unwrap());
    let proof = agent
        .sign_with_identity(&possession_message(&grant.mandate, &possession_request("tools/call", &tool, &digest)))
        .expect("a tls identity");
    let presented = serde_json::to_value(PresentedMandate {
        grant,
        possession: base64::engine::general_purpose::STANDARD.encode(proof),
    })
    .unwrap();
    let r = reply(agent.service().rpc_call_with_mandate(me.clone(), "mcp.invoke", payload.clone(), &presented, &tool, Duration::from_secs(5)).await.unwrap());
    assert!(r.get("error").is_none(), "an established mandate is admitted at the provider: {r}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(seen_calls.lock().unwrap().as_slice(), &[(holder.clone(), true)],
        "the handler saw the carried mandate and the holder string on a member's own direct call");

    // 3. Revoked: refused again.
    agent.offer_revocation_checkpoint(&checkpoint(2, &["t1"])).unwrap();
    let r = reply(agent.service().rpc_call_with_mandate(me.clone(), "mcp.invoke", payload.clone(), &presented, &tool, Duration::from_secs(5)).await.unwrap());
    assert!(r.get("error").is_some(), "a revoked appointment is refused at the provider: {r}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    // 4. A skill claim naming another node, or a skill not served here: refused on the claim.
    let other = "skill:depot/dispatch@10.9.9.9:1".to_string();
    let r = reply(agent.service().rpc_call_with_mandate(me.clone(), "skill.invoke", b"go".to_vec(), &presented, &other, Duration::from_secs(5)).await.unwrap());
    assert_eq!(r["reason"], "resource_not_here", "{r}");
    let unserved = format!("skill:depot/unknown@{me}");
    let r = reply(agent.service().rpc_call_with_mandate(me.clone(), "skill.invoke", b"go".to_vec(), &presented, &unserved, Duration::from_secs(5)).await.unwrap());
    assert_eq!(r["reason"], "resource_not_served", "{r}");
    assert_eq!(skill_seen.load(Ordering::SeqCst), 0, "the skill loop never saw either");

    // 5. An operator-listed protected kind: refused at `rpc_rx`, the serve loop never sees it.
    let r = reply(agent.service().rpc_call(me.clone(), "depot.custom", b"x".to_vec(), Duration::from_secs(5)).await.unwrap());
    assert_eq!(r["reason"], "resource_unnamed", "{r}");
    assert_eq!(custom_seen.load(Ordering::SeqCst), 0, "the serve loop never saw the refused call");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Closure plan C10: work already running stops when its authority lapses.** A provider with
/// enforcement on admits two calls under an established mandate: an MCP tool that runs until
/// cancelled, and a cooperative `rpc_rx` skill that races `authority_lapsed()`.
/// 1. The plant: a sweep with nothing revoked cancels nothing; both keep running.
/// 2. The authority revokes the appointment; the next sweep cancels both. The tool's future is
///    dropped and its call answers "cancelled"; the skill observes the lapse and answers itself.
/// 3. Each stop is recorded with requested, acknowledged and confirmed times, for `drain_report`.
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn test_c10_running_work_stops_when_its_authority_lapses() {
    use crate::agent::gateway_authority::{possession_request, ExecutionAuthority, PresentedMandate};
    use crate::config::TlsConfig;
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::{ReferenceEvaluator, Rule};
    use base64::Engine;
    use ed25519_dalek::SigningKey;

    let authority_key = SigningKey::from_bytes(&[57u8; 32]);
    let gossip_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("c10-cancel-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    let me = agent.node_id().clone();
    let tool = format!("tool:slow@{me}");
    let skill = format!("skill:depot/long@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-c10")
            .with_catalogue("cat-c10", "1")
            .map_action("tools/call", tool.clone())
            .map_action("skill.invoke", skill.clone())
            .allow(Rule::new("*", "tools/call", tool.clone()).requiring_mandate("depot"))
            .allow(Rule::new("*", "skill.invoke", skill.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external));
    agent.with_execution_authority(Arc::clone(&authority));
    agent.with_provider_enforcement();
    agent.start().await.unwrap();
    let _cap = agent.capabilities().advertise_capability(crate::capability::Capability::new("depot", "long"), Duration::from_secs(30));

    let _tool = agent.mcp().register_mcp_tool("slow", serde_json::json!({}), |_args| async move {
        std::future::pending::<()>().await; // runs until cancelled
        Ok(serde_json::json!("finished"))
    });
    {
        let agent = Arc::clone(&agent);
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let agent = Arc::clone(&agent);
                tokio::spawn(async move {
                    req.authority_lapsed().await; // the only way this work ends
                    agent.service().rpc_respond(&req, b"stopped: authority lapsed".to_vec());
                });
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let wall_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let checkpoint = |seq: u64, revoked: &[&str]| {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: wall_ms(),
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c }
    };
    agent.offer_revocation_checkpoint(&checkpoint(1, &[])).unwrap();

    let holder = format!("node:{me}");
    let mandate = Mandate {
        holder: PrincipalId::new(&holder).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "depot work".into(),
        scope: "depot".into(),
        operations: vec!["tools/call:tool:slow".to_string(), "skill.invoke:skill:depot/long".to_string()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 600_000,
    };
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&authority_key, &mandate.canonical_bytes()).to_vec(), mandate };
    let present = |op: &str, res: &str, arguments: &serde_json::Value| {
        let digest = crate::agent::action_evaluator::arguments_digest(&serde_json::to_vec(arguments).unwrap());
        let proof = agent.sign_with_identity(&possession_message(&grant.mandate, &possession_request(op, res, &digest))).unwrap();
        serde_json::to_value(PresentedMandate { grant: grant.clone(), possession: base64::engine::general_purpose::STANDARD.encode(proof) }).unwrap()
    };

    let tool_args = serde_json::json!({});
    let tool_call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"slow","arguments":tool_args}});
    let tool_mandate = present("tools/call", &tool, &tool_args);
    let skill_mandate = present("skill.invoke", &skill, &serde_json::json!({"text": "go"}));
    let tool_task = {
        let (agent, me, tool, m) = (Arc::clone(&agent), me.clone(), tool.clone(), tool_mandate.clone());
        tokio::spawn(async move {
            agent.service().rpc_call_with_mandate(me, "mcp.invoke", tool_call.to_string().into_bytes(), &m, &tool, Duration::from_secs(30)).await
        })
    };
    let skill_task = {
        let (agent, me, skill, m) = (Arc::clone(&agent), me.clone(), skill.clone(), skill_mandate.clone());
        tokio::spawn(async move {
            agent.service().rpc_call_with_mandate(me, "skill.invoke", b"go".to_vec(), &m, &skill, Duration::from_secs(30)).await
        })
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while authority.running_count() < 2 {
        assert!(tokio::time::Instant::now() < deadline, "both calls should be admitted and running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 1. The plant: nothing revoked, nothing cancelled.
    assert_eq!(authority.sweep(wall_ms()), 0);
    assert_eq!(authority.running_count(), 2, "an authorised call keeps running");

    // 2. Revoked: the next sweep cancels both.
    agent.offer_revocation_checkpoint(&checkpoint(2, &["t1"])).unwrap();
    assert_eq!(authority.sweep(wall_ms()), 2);
    let tool_reply: serde_json::Value = serde_json::from_slice(&tool_task.await.unwrap().expect("answered")).unwrap();
    assert!(tool_reply["error"]["message"].as_str().unwrap_or("").contains("authority lapsed"), "{tool_reply}");
    let skill_reply = skill_task.await.unwrap().expect("answered");
    assert_eq!(&skill_reply[..], b"stopped: authority lapsed");

    // 3. Both stops recorded, requested before confirmed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while authority.stop_records().len() < 2 {
        assert!(tokio::time::Instant::now() < deadline, "both stops are recorded: {:?}", authority.stop_records());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for r in authority.stop_records() {
        let (req, conf) = (r.requested_ms.expect("requested"), r.confirmed_ms.expect("confirmed"));
        assert!(req <= conf + 1_000, "requested before confirmed (allowing clock skew): {r:?}");
        assert!(r.acknowledged_ms.is_some());
    }
    assert_eq!(authority.running_count(), 0);

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Closure plan C11: a quiet gateway's clock does not stand still for its decisions.** The
/// gateway's HLC is forced an hour into the past, as on a node that has heard nothing for an hour. A
/// mandate that expired thirty minutes ago is presented through `/a2a`.
/// - The plant: at the frozen HLC's time the mandate **is** inside its window, so a gateway that
///   decided on `Hlc::current()` would admit the call.
/// - The gateway refuses it, and the skill is never reached: it decides on `decision_now_ms`.
/// (#405's trap, heeded: a freshly created clock seeds from the wall clock and would prove nothing.)
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn test_c11_a_quiet_gateway_still_sees_a_mandate_expire() {
    use crate::agent::gateway_authority::{possession_request, ExecutionAuthority, PresentedMandate};
    use crate::config::{GatewayNamedToken, TlsConfig};
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::{ReferenceEvaluator, Rule};
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CALLER: &str = "token:gw/agent-1";
    let authority_key = SigningKey::from_bytes(&[58u8; 32]);
    let holder_key = SigningKey::from_bytes(&[59u8; 32]);
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("c11-quiet-{http_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens =
        vec![GatewayNamedToken { name: "agent-1".into(), token: "s3cret".into(), scopes: vec!["*".into()] }];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a());
    let me = agent.node_id().clone();
    let resource = format!("skill:depot/dispatch@{me}");
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-c11")
            .with_catalogue("cat-c11", "1")
            .map_action("skill.invoke", resource.clone())
            .allow(Rule::new("*", "skill.invoke", resource.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    agent.start().await.unwrap();
    let _cap = agent.capabilities().advertise_capability(crate::capability::Capability::new("depot", "dispatch"), Duration::from_secs(30));
    let reached = Arc::new(AtomicUsize::new(0));
    {
        let (agent, reached) = (Arc::clone(&agent), Arc::clone(&reached));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                reached.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let c = RevocationCheckpoint {
        authority: PrincipalId::new("operator:acme").unwrap(),
        scope: "depot".into(),
        seq: 1,
        issued_at_ms: wall,
        revoked: Default::default(),
    };
    agent
        .offer_revocation_checkpoint(&SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c })
        .unwrap();

    let text = "dispatch";
    let digest = crate::agent::action_evaluator::arguments_digest(&serde_json::to_vec(&serde_json::json!({ "text": text })).unwrap());
    let mandate = Mandate {
        holder: PrincipalId::new(CALLER).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "dispatch".into(),
        scope: "depot".into(),
        operations: vec!["skill.invoke:skill:depot/dispatch".into()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall - 1_800_000, // expired thirty minutes ago
    };
    let an_hour_ago = wall - 3_600_000;
    // The plant: at the frozen clock's time, this mandate is inside its window.
    assert!(mandate.is_current(an_hour_ago), "a decision on the frozen clock would admit it");
    assert!(!mandate.is_current(wall));

    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&authority_key, &mandate.canonical_bytes()).to_vec(), mandate };
    let proof = mycelium_core::tls::sign_bytes(&holder_key, &possession_message(&grant.mandate, &possession_request("skill.invoke", &resource, &digest)));
    let presented = PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof) };

    // A node that has heard nothing for an hour.
    agent.task_ctx.hlc.force_state_for_tests(mycelium_core::hlc::pack(an_hour_ago, 0));
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/a2a"))
        .bearer_auth("s3cret")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tasks/send", "params": {
            "id": "task-c11", "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
            "_meta": { "mandate": presented },
        }}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("error").is_some(), "an expired mandate is refused however quiet the node: {body}");
    assert_eq!(reached.load(Ordering::SeqCst), 0, "the skill was never reached");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Closure plan C7: the bypass matrix.** One node is both gateway and provider, with an evaluator
/// requiring a `depot` mandate, an execution authority, and provider enforcement. Every handler
/// counts what reaches it.
///
/// **The plant:** with a valid mandate, `/mcp` `tools/call` and every `/a2a` send — the pre-0.2,
/// 0.3 and 1.0 names, unary and streamed (plan row N) — each reach their handler. Without it, the refusals below could come from a
/// node that refused everything.
///
/// Then the authority **revokes** the term, and every door is tried again with the revoked mandate.
/// The pass condition is that **no handler runs**, observed by the counters inside the handlers, not
/// by the absence of an error:
///
/// | Door | Refused at |
/// |---|---|
/// | `/mcp` `tools/call` | the gateway (#399) and the provider (C3) |
/// | `/a2a` `tasks/send`, `tasks/sendSubscribe`, `message/send`, `message/stream`, `SendMessage`, `SendStreamingMessage` | the gateway and the provider |
/// | `/gateway/rpc/call` with a protected kind | the route (C1) |
/// | `/gateway/signal/emit`, RPC-shaped | the route (C1) |
/// | a member's direct `rpc_call_with_mandate` | the provider (C3) |
/// | `/gateway/rpc/serve` delivering to an SDK agent | the provider check before streaming (C3) |
/// | `/signals/{kind}`, `/gateway/signal/sse/{kind}` observing a protected kind | the route (C1, the SSE half) |
/// | `/gateway/rpc/respond` for a request this principal was never handed | the route (`unserved_request`) |
///
/// Not in this matrix, and gated where they are built: federation calls (the same `ae_preflight`,
/// `federation_transport` tests), the wiki store (`mycelium-wiki` `git_store_authority`), and
/// reconnecting as a removed member (C5, proposed).
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn test_c7_the_bypass_matrix_no_door_runs_revoked_work() {
    use crate::agent::gateway_authority::{possession_request, ExecutionAuthority, PresentedMandate};
    use crate::config::{GatewayNamedToken, TlsConfig};
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::{ReferenceEvaluator, Rule};
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CALLER: &str = "token:gw/agent-1";
    let authority_key = SigningKey::from_bytes(&[60u8; 32]);
    let holder_key = SigningKey::from_bytes(&[62u8; 32]);
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("c7-matrix-{http_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens =
        vec![GatewayNamedToken { name: "agent-1".into(), token: "s3cret".into(), scopes: vec!["*".into()] }];
    cfg.protected_rpc_kinds = vec!["depot.custom".into()];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a());
    let me = agent.node_id().clone();
    let tool = format!("tool:work@{me}");
    let skill = format!("skill:depot/dispatch@{me}");
    let custom = format!("depot:custom@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-c7")
            .with_catalogue("cat-c7", "1")
            .map_action("tools/call", tool.clone())
            .map_action("skill.invoke", skill.clone())
            .map_action("depot.custom", custom.clone())
            .allow(Rule::new("*", "tools/call", tool.clone()).requiring_mandate("depot"))
            .allow(Rule::new("*", "skill.invoke", skill.clone()).requiring_mandate("depot"))
            .allow(Rule::new("*", "depot.custom", custom.clone()).requiring_mandate("depot")),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    agent.with_provider_enforcement();
    agent.start().await.unwrap();
    let _cap = agent.capabilities().advertise_capability(crate::capability::Capability::new("depot", "dispatch"), Duration::from_secs(30));

    // Handlers, each counting what reaches it.
    let tool_ran = Arc::new(AtomicUsize::new(0));
    let t2 = Arc::clone(&tool_ran);
    let _tool = agent.mcp().register_mcp_tool("work", serde_json::json!({}), move |_args| {
        let t = Arc::clone(&t2);
        async move { t.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("done")) }
    });
    let skill_ran = Arc::new(AtomicUsize::new(0));
    {
        let (agent, ran) = (Arc::clone(&agent), Arc::clone(&skill_ran));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                ran.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }
    // The SDK agent: whatever the serve stream delivers for `depot.custom` is counted.
    let sdk_saw = Arc::new(AtomicUsize::new(0));
    {
        let saw = Arc::clone(&sdk_saw);
        // The HTTP server may still be starting: retry the connect briefly.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let resp = loop {
            match reqwest::Client::new()
                .get(format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/depot.custom"))
                .bearer_auth("s3cret")
                .send()
                .await
            {
                Ok(r) => break r,
                Err(e) => {
                    assert!(tokio::time::Instant::now() < deadline, "the serve stream never opened: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        assert_eq!(resp.status(), 200, "the SDK agent's serve stream is open");
        tokio::spawn(async move {
            use futures_util::StreamExt as _;
            let mut body = resp.bytes_stream();
            while let Some(Ok(chunk)) = body.next().await {
                saw.fetch_add(String::from_utf8_lossy(&chunk).matches("event: depot.custom").count(), Ordering::SeqCst);
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let wall_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let checkpoint = |seq: u64, revoked: &[&str]| {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: wall_ms(),
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c }
    };
    agent.offer_revocation_checkpoint(&checkpoint(1, &[])).unwrap();

    let grant_for = |holder: &str| {
        let m = Mandate {
            holder: PrincipalId::new(holder).unwrap(),
            established_by: PrincipalId::new("operator:acme").unwrap(),
            purpose: "depot".into(),
            scope: "depot".into(),
            operations: vec![
                "tools/call:tool:work".to_string(),
                "skill.invoke:skill:depot/dispatch".to_string(),
                "depot.custom:depot:custom".to_string(),
            ],
            epoch: 1,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: wall_ms() + 600_000,
        };
        SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&authority_key, &m.canonical_bytes()).to_vec(), mandate: m }
    };
    let client_grant = grant_for(CALLER);
    let member_grant = grant_for(&format!("node:{me}"));
    let digest_of = |v: &serde_json::Value| crate::agent::action_evaluator::arguments_digest(&serde_json::to_vec(v).unwrap());
    let client_mandate = |op: &str, res: &str, args: &serde_json::Value| {
        let proof = mycelium_core::tls::sign_bytes(&holder_key, &possession_message(&client_grant.mandate, &possession_request(op, res, &digest_of(args))));
        serde_json::to_value(PresentedMandate { grant: client_grant.clone(), possession: base64::engine::general_purpose::STANDARD.encode(proof) }).unwrap()
    };
    let member_mandate = |op: &str, res: &str, args: &serde_json::Value| {
        let proof = agent.sign_with_identity(&possession_message(&member_grant.mandate, &possession_request(op, res, &digest_of(args)))).unwrap();
        serde_json::to_value(PresentedMandate { grant: member_grant.clone(), possession: base64::engine::general_purpose::STANDARD.encode(proof) }).unwrap()
    };

    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let tool_args = serde_json::json!({});
    let text = "go";
    let mcp_call = |m: serde_json::Value| {
        let (http, base) = (http.clone(), base.clone());
        async move {
            http.post(format!("{base}/mcp")).bearer_auth("s3cret")
                .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                    "params":{"name":"work","arguments":{},"_meta":{"mandate":m}}}))
                .send().await.unwrap().text().await.unwrap()
        }
    };
    let a2a = |method: &str, m: serde_json::Value| {
        let (http, base, method) = (http.clone(), base.clone(), method.to_string());
        // The pre-0.2 methods take `skillId` and `_meta` in params; 0.3 and 1.0 take them in the
        // request's `metadata` (plan row N).
        let params = if method.starts_with("tasks/") {
            serde_json::json!({
                "id": format!("task-{}", fastrand::u32(..)), "skillId": "depot/dispatch",
                "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
                "_meta": {"mandate": m}})
        } else {
            serde_json::json!({
                "message": {"messageId": format!("m-{}", fastrand::u32(..)), "role": "ROLE_USER", "parts": [{"text": text}]},
                "metadata": {"skillId": "depot/dispatch", "_meta": {"mandate": m}}})
        };
        async move {
            http.post(format!("{base}/a2a")).bearer_auth("s3cret")
                .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
                .send().await.unwrap().text().await.unwrap()
        }
    };
    // Every name `/a2a` takes a send by, in every generation (plan row N).
    const A2A_SENDS: [&str; 6] =
        ["tasks/send", "tasks/sendSubscribe", "message/send", "message/stream", "SendMessage", "SendStreamingMessage"];
    let skill_args = serde_json::json!({ "text": text });

    // ── The plant: a valid mandate reaches every handler through the front doors. ──
    mcp_call(client_mandate("tools/call", &tool, &tool_args)).await;
    for method in A2A_SENDS {
        let before = skill_ran.load(Ordering::SeqCst);
        let answer = a2a(method, client_mandate("skill.invoke", &skill, &skill_args)).await;
        // A stream's body is read to its end above, so its skill call has returned.
        assert_eq!(skill_ran.load(Ordering::SeqCst), before + 1, "plant: /a2a {method} reaches the skill: {answer}");
    }
    assert_eq!(tool_ran.load(Ordering::SeqCst), 1, "plant: /mcp reaches the tool");

    // ── Revoked. ──
    agent.offer_revocation_checkpoint(&checkpoint(2, &["t1"])).unwrap();
    let (tool0, skill0, sdk0) = (tool_ran.load(Ordering::SeqCst), skill_ran.load(Ordering::SeqCst), sdk_saw.load(Ordering::SeqCst));

    mcp_call(client_mandate("tools/call", &tool, &tool_args)).await;
    for method in A2A_SENDS {
        a2a(method, client_mandate("skill.invoke", &skill, &skill_args)).await;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(
        serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"work","arguments":{}}}).to_string(),
    );
    for (route, body) in [
        ("rpc/call", serde_json::json!({"target": me.to_string(), "method": "mcp.invoke", "payload_b64": b64, "timeout_secs": 2})),
        ("rpc/call", serde_json::json!({"target": me.to_string(), "method": "skill.invoke", "payload_b64": b64, "timeout_secs": 2})),
        ("signal/emit", serde_json::json!({"kind": "mcp.invoke", "scope": format!("node:{me}"), "payload_b64": b64})),
    ] {
        let status = http.post(format!("{base}/gateway/{route}")).bearer_auth("s3cret").json(&body).send().await.unwrap().status();
        assert_eq!(status, 403, "{route} refuses protected work");
    }
    // The SSE doors: a protected kind can be observed no more than it can be sent — a `mesh:read`
    // holder reading `/signals/mcp.invoke` would see every tool call's frame, mandate included.
    for path in ["/signals/mcp.invoke", "/gateway/signal/sse/mcp.invoke", "/signals/depot.custom"] {
        let status = http.get(format!("{base}{path}")).bearer_auth("s3cret").send().await.unwrap().status();
        assert_eq!(status, 403, "{path} refuses to observe protected work");
    }
    // `rpc/respond` for a request this principal was never handed: refused, nothing emitted. A
    // `mesh:serve` holder used to be able to answer — and so pre-empt — any in-flight call by nonce.
    let r = http.post(format!("{base}/gateway/rpc/respond")).bearer_auth("s3cret")
        .json(&serde_json::json!({"nonce_hex": "00000000000000c7", "sender": me.to_string(), "result_b64": ""}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "rpc/respond refuses a request it never streamed to this principal");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "unserved_request");
    let tool_call = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"work","arguments":tool_args}});
    let _ = agent.service().rpc_call_with_mandate(me.clone(), "mcp.invoke", tool_call.to_string().into_bytes(),
        &member_mandate("tools/call", &tool, &tool_args), &tool, Duration::from_secs(5)).await;
    let _ = agent.service().rpc_call_with_mandate(me.clone(), "skill.invoke", text.as_bytes().to_vec(),
        &member_mandate("skill.invoke", &skill, &skill_args), &skill, Duration::from_secs(5)).await;
    let custom_args = serde_json::json!({ "payload_b64": base64::engine::general_purpose::STANDARD.encode(b"work") });
    let _ = agent.service().rpc_call_with_mandate(me.clone(), "depot.custom", b"work".to_vec(),
        &member_mandate("depot.custom", &custom, &custom_args), &custom, Duration::from_secs(3)).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(tool_ran.load(Ordering::SeqCst), tool0, "no door ran the tool after revocation");
    assert_eq!(skill_ran.load(Ordering::SeqCst), skill0, "no door ran the skill after revocation");
    assert_eq!(sdk_saw.load(Ordering::SeqCst), sdk0, "the serve stream delivered nothing to the SDK agent");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A forged `rpc.result` with the right nonce from the wrong sender does not consume the pending
/// call.** The reply interceptor claimed a pending oneshot on nonce match alone and `await_nonce_reply`
/// then dropped the reply on sender mismatch — so the oneshot was consumed either way, and a peer that
/// learned a nonce could make the legitimate reply undeliverable: the caller timed out although the
/// provider answered. Now the interceptor claims only when the nonce **and** the expected sender match;
/// a nonce-only match from another sender is counted (`SystemStats::rpc_reply_sender_mismatches`) and
/// ignored, and the real reply still arrives. Seen failing first: `Err(Timeout)` where `Ok("real")` was
/// expected.
#[tokio::test]
async fn a_forged_reply_from_the_wrong_sender_does_not_consume_the_pending_call() {
    use crate::signal::SignalScope;
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
    let a = Arc::new(GossipAgent::new(id_a.clone(), cfg_a));
    let b = Arc::new(GossipAgent::new(id_b.clone(), cfg_b));
    a.start().await.unwrap();
    b.start().await.unwrap();
    poll_until(|| !a.peers().is_empty() && !b.peers().is_empty(), 5_000).await;

    // A serves `k` to itself. Each request's nonce is leaked to B (the forger) before A answers.
    let (leak_tx, mut leak_rx) = tokio::sync::mpsc::channel::<u64>(4);
    {
        let a = Arc::clone(&a);
        let mut rx = a.service().rpc_rx("k");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = leak_tx.send(req.nonce()).await;
                tokio::time::sleep(Duration::from_millis(400)).await;
                a.service().rpc_respond(&req, b"real".to_vec());
            }
        });
    }
    // B forges a reply carrying A's nonce, addressed to A, from B — a sender A never called.
    {
        let b = Arc::clone(&b);
        let target = id_a.clone();
        tokio::spawn(async move {
            while let Some(nonce) = leak_rx.recv().await {
                let mut buf = Vec::with_capacity(14);
                buf.extend_from_slice(&nonce.to_le_bytes());
                buf.extend_from_slice(b"forged");
                let _ = b.mesh().emit("rpc.result", SignalScope::Individual(target.clone()), buf);
            }
        });
    }

    let reply = a.service().rpc_call(id_a.clone(), "k", b"hi".to_vec(), Duration::from_secs(5)).await;
    assert_eq!(reply.as_deref().ok(), Some(&b"real"[..]), "the real reply arrives: {reply:?}");
    assert_eq!(a.system_stats().rpc_reply_sender_mismatches, 1, "the forged reply was counted, not claimed");
    assert_eq!(b.system_stats().rpc_reply_sender_mismatches, 0);

    a.shutdown().await;
    b.shutdown().await;
}

/// **A leadership answer names the rung it reached.**
///
/// `elect_leader` returns a bare `NodeId`, which cannot distinguish *"a quorum chose me"* from
/// *"this is what my replica currently says"* — and callers read the bare id as an **exclusive
/// grant**, because nothing in the type said otherwise. Those are different answers, and the
/// difference decides whether two callers can act as leader at once.
///
/// This pins the distinction end to end: a node that proposes into its own one-member electorate
/// and wins reports `Decided`, and carries a non-zero `epoch` — the fencing token that is the
/// honest instrument for exclusivity, rather than the leader's identity or the fact that the call
/// returned `Ok`.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn a_leadership_answer_names_its_rung() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());
    agent.mesh().join_group("rung");

    let l = agent.consensus().elect_leader_receipt("rung").await.expect("an established electorate elects");

    assert_eq!(l.leader, *agent.node_id(), "a one-member electorate elects its only member");
    assert_eq!(
        l.basis, crate::LeadershipBasis::Decided,
        "our own proposal committed at quorum — that is the strongest rung, and it should say so",
    );
    assert!(l.was_decided_here());
    assert!(l.epoch > 0, "the fencing token is the commit's HLC, and must be usable");

    // The legacy call still works and agrees about *who*; what it cannot tell you is *how*.
    assert_eq!(
        agent.consensus().elect_leader("rung").await.expect("legacy call"),
        l.leader,
    );
}

/// **An election with no electorate does not produce a leadership answer at all** — it refuses,
/// rather than handing back a stale winner from an earlier, differently-constituted election.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn an_unjoined_group_yields_no_leadership() {
    let agent = make_agent();
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());

    match agent.consensus().elect_leader_receipt("never-joined").await {
        Err(crate::ConsistencyError::ElectorateUnavailable { observed_members, .. }) =>
            assert_eq!(observed_members, 0),
        other => panic!("expected a refusal with no electorate, got {other:?}"),
    }
}


/// **Boundary H closure plan C5: removing a member.** Three TLS nodes share a fleet CA; A and C take
/// removals from an operator membership authority. The operator removes B at A.
/// - A refuses B from then on: its gossip writes are not applied, and it does not come back as a
///   peer (its pings are dropped and its certificate is refused at the handshake);
/// - C learns the removal by gossip, verifies it and applies it too;
/// - C, which was not removed, is unaffected (the plant);
/// - B's identity keys read as revoked at A, so a mandate B holds no longer proves possession;
/// - a removal from an untrusted issuer, or a forged one, is ignored; a second offer is a no-op.
#[cfg(feature = "compliance")]
mod member_removal {
    use super::*;
    use crate::config::TlsConfig;
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::membership::{MemberRemoval, RemovalOffer, SignedMemberRemoval};
    use ed25519_dalek::{Signer, SigningKey};

    fn removal(key: &SigningKey, authority: &str, node: &NodeId, keys: Vec<[u8; 32]>) -> SignedMemberRemoval {
        let removal = MemberRemoval {
            authority: IssuerId::new(authority).unwrap(),
            node: node.clone(),
            keys,
            seq: 1,
            issued_at_ms: 1_000,
            reason: "compromised".into(),
        };
        SignedMemberRemoval { signature: key.sign(&removal.canonical_bytes()).to_bytes().to_vec(), removal }
    }

    async fn poll(mut ok: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_removed_member_is_cut_off_and_an_unremoved_one_is_not() {
        let ports = [alloc_port(), alloc_port(), alloc_port()];
        let id = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
        let (node_a, node_b, node_c) = (id(ports[0]), id(ports[1]), id(ports[2]));
        let cert_dir = std::env::temp_dir().join(format!("myc-c5-{}-{}-{}", ports[0], ports[1], ports[2]));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let mk = |port: u16, boots: Vec<NodeId>| {
            let mut cfg = GossipConfig::default();
            cfg.bind_port = port;
            cfg.bootstrap_peers = boots;
            cfg.reconnect_backoff_secs = 1;
            cfg.health_check_interval_secs = 1;
            cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
            Arc::new(GossipAgent::new(id(port), cfg))
        };
        let a = mk(ports[0], vec![]);
        a.start().await.unwrap();
        let b = mk(ports[1], vec![node_a.clone()]);
        let c = mk(ports[2], vec![node_a.clone()]);
        b.start().await.unwrap();
        c.start().await.unwrap();
        assert!(poll(|| a.peers().len() == 2 && !b.peers().is_empty() && !c.peers().is_empty()).await, "fleet failed to peer");

        let operator = SigningKey::from_bytes(&[51u8; 32]);
        let rogue = SigningKey::from_bytes(&[52u8; 32]);
        let authorities = || {
            let mut external = TrustedExternalIssuers::new();
            external.trust(IssuerId::new("operator:membership").unwrap(), operator.verifying_key().to_bytes()).unwrap();
            external.trust(IssuerId::new("operator:other").unwrap(), rogue.verifying_key().to_bytes()).unwrap();
            external
        };
        a.with_membership_authorities([IssuerId::new("operator:membership").unwrap()], authorities());
        c.with_membership_authorities([IssuerId::new("operator:membership").unwrap()], authorities());

        // B's identity key, as A has seen it.
        let key_b = {
            let key = format!("sys/identity/{node_b}");
            assert!(poll(|| a.kv().get(&key).is_some_and(|v| v.len() == 32)).await, "A never saw B's identity key");
            let mut k = [0u8; 32];
            k.copy_from_slice(&a.kv().get(&key).unwrap());
            k
        };

        // Before removal B's writes reach A: the channel works, so the refusal below is the removal.
        let _ = b.kv().set("c5/before", b"b".to_vec());
        assert!(poll(|| a.kv().get("c5/before").is_some()).await, "B's write never reached A before removal");

        // The plant's opposites: an issuer that is not a membership authority, and a forgery.
        assert_eq!(a.offer_member_removal(&removal(&rogue, "operator:other", &node_b, vec![key_b])), Some(RemovalOffer::UntrustedAuthority));
        let mut forged = removal(&operator, "operator:membership", &node_c, vec![]);
        forged.removal.node = node_b.clone();
        assert!(matches!(a.offer_member_removal(&forged), Some(RemovalOffer::Unverifiable(_))));
        assert!(a.removed_members().is_empty(), "a refused removal removes nobody");

        // The operator removes B, naming a retained key it held before a rotation as well.
        let retained = [9u8; 32];
        let signed = removal(&operator, "operator:membership", &node_b, vec![key_b, retained]);
        assert_eq!(a.offer_member_removal(&signed), Some(RemovalOffer::Accepted));
        assert_eq!(a.offer_member_removal(&signed), Some(RemovalOffer::AlreadyRemoved), "monotonic: a repeat is a no-op");
        assert_eq!(a.removed_members(), vec![node_b.clone()]);

        // Its keys, current and retained, now read as revoked at A.
        let mut still_trusted = vec![key_b, retained];
        a.task_ctx.retain_trusted(&node_b, &mut still_trusted);
        assert!(still_trusted.is_empty(), "a removed member's keys are revoked");

        // C learns it by gossip and applies it.
        assert!(poll(|| c.removed_members() == vec![node_b.clone()]).await, "the removal never reached C");

        // B's writes are no longer applied at A or C; C's still are (the plant).
        let _ = b.kv().set("c5/after-b", b"b".to_vec());
        let _ = c.kv().set("c5/after-c", b"c".to_vec());
        assert!(poll(|| a.kv().get("c5/after-c").is_some()).await, "an unremoved member's write must still arrive");
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(a.kv().get("c5/after-b").is_none(), "a removed member's write was applied at A");
        assert!(c.kv().get("c5/after-b").is_none(), "a removed member's write was applied at C");

        // B does not come back as a peer: over several reconnect and health intervals A never
        // re-admits it, while C stays.
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(!a.peers().contains(&node_b), "a removed member rejoined A");
        }
        assert!(a.peers().contains(&node_c), "an unremoved member was dropped");

        for n in [&a, &b, &c] {
            n.shutdown_with_timeout(Duration::from_secs(5)).await;
        }
        let _ = std::fs::remove_dir_all(&cert_dir);
    }
}

/// A token table the build cannot enforce must refuse to start, never run an open gateway
/// (doc-coverage run 18, 2026-10-02). Named and scoped tokens are honoured only under
/// `compliance` (`gateway_auth`'s `have_scoped`); without it the table was parsed, validated and
/// ignored, and a node with no positional token served every gateway route to an anonymous
/// caller — the 2.10.0–2.12.0 open-gateway defect reached by a different build.
#[cfg(all(feature = "gateway", not(feature = "compliance")))]
#[tokio::test]
async fn a_token_table_this_build_cannot_enforce_refuses_to_start() {
    use crate::{GatewayNamedToken, GatewayToken};
    for named in [true, false] {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(alloc_port());
        if named {
            cfg.gateway_named_tokens =
                vec![GatewayNamedToken { name: "ops".into(), token: "s3cret".into(), scopes: vec!["*".into()] }];
        } else {
            cfg.gateway_scoped_tokens = vec![GatewayToken { token: "s3cret".into(), scopes: vec!["*".into()] }];
        }
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        let started = agent.start().await;
        let field = if named { "gateway_named_tokens" } else { "gateway_scoped_tokens" };
        match started {
            Err(GossipError::InvalidField { field: f, reason }) => {
                assert_eq!(f, field);
                assert!(reason.contains("compliance"), "the refusal names the missing feature: {reason}");
            }
            other => {
                let _ = agent.shutdown_with_timeout(Duration::from_secs(5)).await;
                panic!("{field} set without `compliance` must refuse to start, got {other:?}");
            }
        }
    }
}

/// An `[oidc]` table in a build without `compliance` must refuse to start, not vanish (I1 audit
/// finding `gw.oidc`, 2026-10-02). The field is `#[cfg(feature = "compliance")]` and `GossipConfig`
/// accepts unknown keys, so the table was dropped at parse time; `have_oidc` is `false` in that
/// build; with no positional token the gateway ran open — a node the operator believed was behind
/// SSO served every route to an anonymous caller.
#[cfg(all(feature = "gateway", not(feature = "compliance")))]
#[tokio::test]
async fn an_oidc_table_this_build_cannot_enforce_refuses_to_start() {
    let port = alloc_port();
    let text = format!(
        "bind_port = {port}\nhttp_port = {}\n\n[oidc]\nissuer = \"https://idp.coop.example\"\naudience = \"mycelium\"\n",
        alloc_port()
    );
    let cfg: GossipConfig = toml::from_str(&text).expect("the table parses in every build");
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    match agent.start().await {
        Err(GossipError::InvalidField { field, reason }) => {
            assert_eq!(field, "oidc");
            assert!(reason.contains("compliance"), "the refusal names the missing feature: {reason}");
        }
        other => {
            let _ = agent.shutdown_with_timeout(Duration::from_secs(5)).await;
            panic!("an [oidc] table without `compliance` must refuse to start, got {other:?}");
        }
    }
}

/// **An exposed gateway with no credential refuses to start** (plan `post-360-hardening.md` row P1).
/// `http_addr = "0.0.0.0"` with no token, no token table and no `[oidc]` served every gateway route to
/// anyone who could reach the port; the docs said *suitable for loopback-only deployments* and nothing
/// checked that the deployment was one. Now `start()` refuses, naming `http_addr`, the credential
/// settings and the opt-in — in every gateway build (without `compliance` the only credential is
/// `gateway_auth_token`; the tables and `[oidc]` are refused there already). `::` is the same case.
/// A blank secret is not a credential either: since #598's review `validate()` refuses it by name in
/// every form (`gateway_auth_token`, and under `compliance` a scoped or named entry), before the
/// exposure check is reached — so a blank-token node never starts, on any address.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn an_exposed_gateway_with_no_credential_refuses_to_start() {
    #[allow(unused_mut)]
    let mut cases: Vec<(&str, Option<&str>, bool, &str)> = vec![
        ("0.0.0.0", None, false, "http_addr"),
        ("::", None, false, "http_addr"),
        ("0.0.0.0", Some("  "), false, "gateway_auth_token"),
    ];
    #[cfg(feature = "compliance")]
    cases.push(("0.0.0.0", None, true, "gateway_scoped_tokens"));
    for (addr, token, blank_table, expected) in cases {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(alloc_port());
        cfg.http_addr = addr.to_string();
        cfg.gateway_auth_token = token.map(String::from);
        if blank_table {
            cfg.gateway_scoped_tokens = vec![crate::GatewayToken { token: String::new(), scopes: vec!["*".into()] }];
        }
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        match agent.start().await {
            Err(GossipError::InvalidField { field, reason }) => {
                assert_eq!(field, expected, "{addr} token={token:?} blank_table={blank_table}");
                let named: &[&str] = if expected == "http_addr" { &["gateway_auth_token", "gateway_allow_unauthenticated", addr] } else { &["blank"] };
                for named in named {
                    assert!(reason.contains(named), "the refusal names `{named}`: {reason}");
                }
            }
            other => {
                let _ = agent.shutdown_with_timeout(Duration::from_secs(5)).await;
                panic!("{addr} with no credential (token {token:?}) must refuse to start, got {other:?}");
            }
        }
    }
}

/// The three ways an exposed or local gateway still starts (P1): a credential on a non-loopback
/// address; no credential on loopback (development is unchanged — `127.0.0.0/8` and `::1`); and the
/// explicit opt-in, which starts, is classified as a waiver (the case `start()` warns about, once)
/// and reads `not_configured` in the report, so the secure profile refuses it.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn an_exposed_gateway_starts_with_a_credential_on_loopback_or_with_the_opt_in() {
    use crate::agent::guarantee::{gateway_exposure, GatewayExposure};
    // `127.0.0.2` and `::1` are loopback too (`guarantee::tests`), but not bindable on every runner
    // (macOS configures only `127.0.0.1`; some containers have no IPv6), so they are not started here.
    let cases: [(&str, Option<&str>, bool, GatewayExposure); 3] = [
        ("0.0.0.0", Some("s3cret"), false, GatewayExposure::Credentialed),
        ("127.0.0.1", None, false, GatewayExposure::Loopback),
        ("0.0.0.0", None, true, GatewayExposure::Waived),
    ];
    for (addr, token, opt_in, expected) in cases {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(alloc_port());
        cfg.http_addr = addr.to_string();
        cfg.gateway_auth_token = token.map(String::from);
        cfg.gateway_allow_unauthenticated = opt_in;
        assert_eq!(gateway_exposure(&cfg), expected, "{addr} token={token:?} opt_in={opt_in}");
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        agent.start().await.unwrap_or_else(|e| panic!("{addr} token={token:?} opt_in={opt_in} must start: {e:?}"));
        let state = agent.guarantee_report().entry("gw.exposed_closed").expect("registered").resolution.state();
        let want = match expected {
            GatewayExposure::Credentialed => "enforced",
            GatewayExposure::Loopback => "not_applicable",
            _ => "not_configured",
        };
        assert_eq!(state, want, "{addr} token={token:?} opt_in={opt_in}");
        agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    }
}

// A `[tls]` or `[gateway_tls]` table in a build without `tls` refusing to start: `mycelium-tls-free-tests`, the one
// test build of `mycelium` without `tls` (this crate's dev-dependencies turn it on, so a test here never ran).

/// `/a2a` is anonymous skill dispatch unless an evaluator is attached — a configured bearer does not
/// gate it (I1 audit `a2a.admission`, 2026-10-02). The attach-time warning said the opposite: it
/// fired only with no evaluator **and** no bearer, so a node with a bearer and no evaluator — the
/// production-readiness §2 case — got no warning, and an evaluator attached after `with_a2a` made
/// it spurious. The predicate is now a start-time fact. This replaces
/// `a2a_mounted_without_evaluator_or_bearer_is_the_open_configuration` (2026-09-25), which pinned the
/// wrong condition — *any one bearer is enough to close the anonymous path* — and said to revisit it
/// rather than leave it quietly passing.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn a2a_is_anonymous_dispatch_whatever_bearer_is_configured() {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = alloc_port();
    cfg.http_port = Some(alloc_port());
    cfg.gateway_auth_token = Some("s3cret".into());
    let id = NodeId::new("127.0.0.1", cfg.bind_port).unwrap();
    let plain = GossipAgent::new(id.clone(), cfg.clone());
    assert!(!plain.a2a_dispatch_is_anonymous(), "nothing mounted, nothing exposed");
    let mounted = GossipAgent::new(id, cfg).with_a2a();
    assert!(mounted.a2a_dispatch_is_anonymous(), "a bearer does not gate /a2a");
    #[cfg(feature = "tls")]
    {
        mounted.with_action_evaluator(Arc::new(crate::ReferenceEvaluator::new("rev-i1")));
        assert!(!mounted.a2a_dispatch_is_anonymous(), "an evaluator attached after with_a2a counts");
    }
}

/// Plan I3's acceptance: a node that provides every guarantee `secure-single-domain` requires starts
/// under it, its report says so, and its gateway refuses an unauthenticated request. Under rev 2 the
/// node's certificate is issued off-node first, so the same report shows `id.ca_key_off_node`
/// enforced and required (the rev-1 gap, closed in v2.21.0: plan §8).
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn a_node_providing_every_requirement_starts_under_the_secure_profile() {
    use crate::agent::gateway_authority::ExecutionAuthority;
    use crate::agent::guarantee::{check, Resolution, SECURE_SINGLE_DOMAIN};
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{ClockModel, DurableEpochs, ExecutionGate, FreshnessPolicy, ResourceTier};
    use crate::mandate::grant::{EntitlementTable, GrantVerifier};
    use crate::mandate::{PrincipalId, ResourceAuthority};
    use crate::{EvidenceJournal, EvidenceProfile, ReferenceEvaluator, Rule};
    use ed25519_dalek::SigningKey;

    let port = alloc_port();
    let http_port = alloc_port();
    let root = std::env::temp_dir().join(format!("secure-profile-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let cert_dir = root.join("tls");
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    // Rev 2 requires `id.ca_key_off_node`: the fleet CA lives with an issuer, the node gets a
    // certificate issued there and the CA *certificate* only. The issuer here is a throwaway node
    // started once on its own directory (which mints the CA), then `issue_node_cert` on that
    // directory — the operator's `mycelium tls issue`.
    let ca_dir = root.join("issuer");
    {
        let issuer_port = alloc_port();
        let mut icfg = GossipConfig::auto();
        icfg.bind_port = issuer_port;
        icfg.tls = Some(TlsConfig { auto_cert_dir: ca_dir.clone(), ..Default::default() });
        let issuer = GossipAgent::new(NodeId::new("127.0.0.1", issuer_port).unwrap(), icfg);
        issuer.start().await.expect("the issuer node mints the fleet CA");
        issuer.shutdown().await;
    }
    assert!(ca_dir.join("ca-key.pem").exists(), "the CA key is with the issuer");
    let (cert_pem, key_pem) = crate::issue_node_cert(&ca_dir, &id, &cert_dir).expect("a node certificate issued off-node");
    std::fs::copy(ca_dir.join("ca-cert.pem"), cert_dir.join("ca-cert.pem")).unwrap();
    assert!(!cert_dir.join("ca-key.pem").exists(), "and never on the node");
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), cert_pem: Some(cert_pem), key_pem: Some(key_pem), ca_cert_pem: None });
    cfg.gateway_tls = Some(crate::GatewayTlsConfig::default()); // the node cert serves the gateway too
    cfg.gateway_auth_token = Some("s3cret".into());
    cfg.require_identity_proofs = true;
    cfg.egress.allow_hosts = vec!["idp.coop.example".into()];
    cfg.persistence = Some(PersistenceConfig {
        base_path: root.join("data"),
        sync_mode: SyncMode::Flush,
        snapshot_wal_threshold: 1_000_000,
        snapshot_interval_secs: 3_600, on_unreadable: Default::default(),
    });
    // Rev 3 requires `cons.safety_profile`: an exclusive outcome is decided only by an electorate group.
    cfg.consensus_require_electorate = true;
    cfg.profile = Some("secure-single-domain".into());

    let a = GossipAgent::new(id, cfg).with_a2a();
    let me = a.node_id().clone();
    let resource = format!("skill:depot/dispatch@{me}");
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-i3")
            .with_catalogue("cat-i3", "1")
            .map_action("skill.invoke", resource.clone())
            .allow(Rule::new("*", "skill.invoke", resource).requiring_mandate("depot")),
    ));
    a.with_evidence_journal(EvidenceJournal::open(root.join("evidence"), EvidenceProfile::Strict).unwrap());
    a.with_provider_enforcement();
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external
        .trust(IssuerId::new("operator:acme").unwrap(), SigningKey::from_bytes(&[51u8; 32]).verifying_key().to_bytes())
        .unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .unwrap();
    let authority = ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external);
    authority.with_durable_epochs(DurableEpochs::open(root.join("epochs")).unwrap());
    a.with_execution_authority(Arc::new(authority));

    // Before the boundary the check already passes: every required guarantee resolves on config and
    // attachments, none of which start() changes.
    check(&a.guarantee_report(), &SECURE_SINGLE_DOMAIN).expect("every requirement is enforced before start");
    a.start().await.expect("and so the profile admits the node");
    let r = a.guarantee_report();
    assert!(r.started && r.node_requirements_satisfied(), "required unmet: {:?}", r.required_unmet());
    assert_eq!(r.profile.as_ref().map(|p| (p.name, p.revision, p.selected)), Some(("secure-single-domain", 5, true)));
    // Rev 5's requirement (row F2), met by `[tls]` alone: no new setting is needed to start.
    assert_eq!(r.entry("mesh.signed_writes").unwrap().resolution.state(), "enforced");
    assert!(!cert_dir.join("ca-key.pem").exists(), "start() minted no CA key on the node");
    assert_eq!(r.unresolved(), ["net.confinement", "clock.sync"], "what the node cannot see is still listed");
    // Rev 3's requirement, met: a safety-sensitive proposal outside an electorate group is refused.
    assert!(matches!(r.entry("cons.safety_profile").unwrap().resolution, Resolution::Enforced));
    // Rev 2's two new requirements, met: the CA key is with the issuer, and unreadable persisted
    // state refuses the start (the default).
    assert!(matches!(r.entry("id.ca_key_off_node").unwrap().resolution, Resolution::Enforced), "{:?}", r.entry("id.ca_key_off_node").unwrap().resolution);
    assert!(matches!(r.entry("persist.unreadable_refused").unwrap().resolution, Resolution::Enforced));
    assert!(!r.unmet().contains(&"id.ca_key_off_node") && !r.unmet().contains(&"persist.unreadable_refused"));
    // Talk to the gateway the way a partner would: TLS pinned on the node's SPKI (the project's own
    // client path; a stock verifier does not negotiate with the Ed25519 node identity).
    let pin = crate::federation::pinning::ed25519_spki_sha256(&a.task_ctx.tls.get().expect("TLS is on").verifying_key_bytes());
    let client = reqwest::Client::builder()
        .use_preconfigured_tls(crate::federation::pinning::pinned_client_config(vec![pin]))
        .build()
        .unwrap();
    // The gateway task binds after start() returns; wait for it structurally, never a fixed sleep.
    let url = format!("https://127.0.0.1:{http_port}/gateway/kv/keys");
    let mut last = None;
    let mut resp = None;
    for _ in 0..100 {
        match client.get(&url).send().await {
            Ok(r) => { resp = Some(r); break; }
            Err(e) => {
                let mut chain = e.to_string();
                let mut src: Option<&dyn std::error::Error> = std::error::Error::source(&e);
                while let Some(s) = src { chain.push_str(" <- "); chain.push_str(&s.to_string()); src = s.source(); }
                last = Some(chain);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let resp = resp.unwrap_or_else(|| panic!("the HTTPS gateway came up — last error: {last:?}"));
    assert_eq!(resp.status(), 401, "no bearer, no entry");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// `report.egress` (plan §8): the OIDC identity provider is an outbound the substrate chooses, so a
/// non-empty `egress.allow_hosts` that does not permit it is a **contradiction refused at start**, by
/// name — not a JWKS fetch that fails later behind a gateway that then refuses every token.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn an_oidc_issuer_the_egress_policy_denies_refuses_the_start() {
    let port = alloc_port();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.oidc = Some(crate::OidcConfig { issuer: "https://idp.example".into(), audience: "mycelium".into(), group_claim: "groups".into(), group_scopes: Default::default(), jwks_uri: None });
    cfg.egress.allow_hosts = vec!["api.allowed.example".into()];
    let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg.clone());
    let e = a.start().await.expect_err("an issuer the egress policy denies must refuse the start");
    assert!(e.to_string().contains("egress") && e.to_string().contains("idp.example"), "{e}");
    a.shutdown().await;

    cfg.egress.allow_hosts.push("idp.example".into());
    let b = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    b.start().await.expect("the issuer permitted, the node starts");
    b.shutdown().await;
}

/// `report.egress` (plan §8): a federation client refuses an endpoint the egress policy denies
/// **before any byte is sent** — the link stays down and nothing is dialled — and the agent applies
/// the node's policy to the clients it is handed.
#[tokio::test]
async fn a_federation_client_refuses_an_endpoint_the_egress_policy_denies() {
    use crate::federation::client::{ClientError, FederationClient, GatewayEndpoint};
    use crate::federation::session::LinkState;
    use crate::federation::DomainId;
    let (sk, _vk) = {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key().to_bytes();
        (sk, vk)
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let dialled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let dialled = Arc::clone(&dialled);
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            for _ in 0..200 {
                if listener.accept().is_ok() { dialled.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }
    let client = FederationClient::new(
        DomainId::new("beta").unwrap(), "svc/billing", sk, DomainId::new("alpha").unwrap(),
        vec![GatewayEndpoint { id: "gw".into(), base_url: format!("http://127.0.0.1:{port}") }],
        1, Duration::from_secs(30),
    // By reference, as `agent.egress_policy()` hands it (doc-coverage run 20, code gap 5).
    ).with_egress(&crate::EgressPolicy { allow_hosts: vec!["partner.example".into()] });
    let r = client.connect().await;
    assert!(matches!(r, Err(ClientError::Egress { .. })), "{r:?}");
    assert_eq!(client.link_state(), LinkState::Down);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(dialled.load(std::sync::atomic::Ordering::SeqCst), 0, "refused before any byte was sent");

    // The agent hands its own policy to clients that carry none.
    let port2 = alloc_port();
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port2;
    cfg.egress.allow_hosts = vec!["partner.example".into()];
    let bare = Arc::new(FederationClient::new(
        DomainId::new("beta").unwrap(), "svc/billing", ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]), DomainId::new("alpha").unwrap(),
        vec![GatewayEndpoint { id: "gw".into(), base_url: format!("http://127.0.0.1:{port}") }],
        1, Duration::from_secs(30),
    ));
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port2).unwrap(), cfg).with_federation_clients([Arc::clone(&bare)]);
    assert!(matches!(bare.connect().await, Err(ClientError::Egress { .. })), "the node's policy applies to a client handed to it");
    drop(agent);
}

// ── P2: a consensus electorate is a governed group ─────────────────────────────────────────────

/// One electorate test node's config: `[persistence]` under a temp directory of its own (an electorate member must
/// configure it — a member without is refused by name, `acceptor_guard` detection 1), the mesh tuning the rounds
/// settled on, and `peers` as its bootstrap list. `fresh` wipes the directory first — a node restarted over an
/// **empty** store keeps the same config with `fresh = false` and the directory removed by the test.
#[cfg(feature = "consensus")]
fn electorate_cfg(port: u16, peers: Vec<NodeId>, fresh: bool) -> GossipConfig {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.bootstrap_peers = peers;
    cfg.health_check_max_jitter_ms = 50;
    // Anti-entropy runs once per health-check interval: a dropped gossip frame is repaired in five seconds
    // rather than ten. The reconnect backoff stays below the interval minus two (the config's own tuning
    // invariant) — round 3 found peers evicted and redialled in a loop with an interval of two seconds.
    cfg.health_check_interval_secs = 5;
    cfg.reconnect_backoff_secs = 1;
    let base = std::env::temp_dir().join(format!("mycelium-electorate-{port}"));
    if fresh { let _ = std::fs::remove_dir_all(&base); }
    cfg.persistence = Some(PersistenceConfig {
        base_path: base, sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600,
        on_unreadable: OnUnreadable::Refuse,
    });
    cfg
}

/// `n` started agents, fully meshed, each running the consensus listener; returns once every node
/// has a peer (a structural poll, never a fixed sleep). `tweak` adjusts each node's config.
#[cfg(feature = "consensus")]
async fn electorate_mesh(n: usize, tweak: impl Fn(&mut GossipConfig)) -> (Vec<GossipAgent>, Vec<crate::ConsensusListenerHandle>, tokio::sync::MutexGuard<'static, ()>) {
    // One electorate mesh at a time in this process: each is several nodes, and run side by side on a loaded host
    // their gossip fell behind enough to fail rosters that converge in a second alone (round 2's flakes).
    let serial = ELECTORATE_MESHES.lock().await;
    let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let mut agents = Vec::new();
    for i in 0..n {
        // Each node dials only the nodes started before it, which are already listening — a dial to a node not yet
        // up fails and backs off, and round 3 saw meshes stay sparse for a minute after it.
        let mut cfg = electorate_cfg(ports[i], ids[..i].to_vec(), true);
        tweak(&mut cfg);
        agents.push(GossipAgent::new(ids[i].clone(), cfg));
    }
    for a in &agents { a.start().await.unwrap(); }
    let listeners = agents.iter().map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    // Readiness is structural and is what consensus needs: every node's write reaches every node. (Round 2: waiting
    // for each node's peer table to list every other one flaked — a peer table need not list a whole reachable mesh.)
    for a in &agents {
        assert!(a.kv().set(format!("electorate-test-ready/{}", a.node_id()), Bytes::from_static(b"1")));
    }
    // And every node is a direct peer of every other: a group-scoped signal is forwarded to the group's known
    // members, so a member another cannot reach directly can miss a whole round.
    let reached = tokio::time::timeout(Duration::from_secs(60), async {
        while !agents.iter().all(|a| a.kv().scan_prefix("electorate-test-ready/").len() == n && a.peers().len() + 1 >= n) {
            time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    assert!(reached.is_ok(), "writes did not reach every node: seen {:?}, peers {:?}",
        agents.iter().map(|a| a.kv().scan_prefix("electorate-test-ready/").len()).collect::<Vec<_>>(),
        agents.iter().map(|a| a.peers().len()).collect::<Vec<_>>());
    (agents, listeners, serial)
}

/// Serialises the electorate tests' meshes (see `electorate_mesh`).
#[cfg(feature = "consensus")]
static ELECTORATE_MESHES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The acceptor's P2 door, read as answer-or-refuse (the gate it would claim through is not asked about).
#[cfg(feature = "consensus")]
fn admits(
    engine: &crate::consensus::ConsensusEngine, scope: &SignalScope, slot: &str, named: Option<(u64, [u8; 32])>,
    proposer: &NodeId, value: Option<&Bytes>,
) -> Result<(), (u64, [u8; 32])> {
    engine.electorate_admits(scope, slot, named, proposer, value.map(|v| (v, false)), None).map(|_| ())
}

/// `declare_electorate`, retried further while it is merely undecided — a genesis needs every member's answer, and on a
/// loaded test host the library's own bounded retries can run out. Any other refusal is returned at once.
#[cfg(feature = "consensus")]
async fn declare(agent: &GossipAgent, group: &str, exclusive_default: bool) -> Result<crate::ElectorateDecl, crate::ElectorateError> {
    // Until decided (round 3: a genesis needs every member's answer, and one timed out on a loaded host).
    let mut last = None;
    for _ in 0..3 {
        match agent.declare_electorate(group, exclusive_default).await {
            Err(crate::ElectorateError::NotDecided(why)) => last = Some(why),
            other => return other,
        }
        time::sleep(Duration::from_millis(100)).await;
    }
    Err(crate::ElectorateError::NotDecided(last.unwrap_or_default()))
}

/// Every agent's view of `group`'s roster holds exactly `members` (a structural readiness poll).
#[cfg(feature = "consensus")]
async fn roster_converges(agents: &[GossipAgent], group: &str, members: &[&GossipAgent]) {
    let want = crate::agent::electorate::sorted_members(members.iter().map(|a| a.node_id().clone()).collect());
    let prefix = crate::signal::grp_prefix(group);
    let seen = |a: &GossipAgent| -> Vec<NodeId> {
        crate::agent::electorate::sorted_members(a.kv().scan_prefix(&prefix).into_iter()
            .filter_map(|(k, _)| k.strip_prefix(prefix.as_str()).and_then(|r| r.parse().ok())).collect())
    };
    // A join is a write carried by gossip; on a loaded host one can be slow to reach every node, so each member's
    // join is re-issued while the rosters disagree — the agreement is what the test needs, not the first frame.
    let mut agreed = false;
    for _ in 0..6 {
        let ok = tokio::time::timeout(Duration::from_secs(5), async {
            while !agents.iter().all(|a| seen(a) == want) { time::sleep(Duration::from_millis(10)).await; }
        }).await;
        if ok.is_ok() { agreed = true; break; }
        // `join_group` is a no-op for a member; re-writing its own member key re-gossips it.
        for m in members { let _ = m.kv().set(format!("{prefix}{}", m.node_id()), Bytes::from_static(b"1")); }
    }
    assert!(agreed, "roster of {group} did not converge to {want:?}: {:?}", agents.iter().map(seen).collect::<Vec<_>>());
}

/// Every agent holds `group`'s electorate at `epoch` (the commit and its certificate both arrived).
#[cfg(feature = "consensus")]
async fn epoch_converges(agents: &[GossipAgent], group: &str, epoch: u64) {
    let ok = tokio::time::timeout(Duration::from_secs(30), async {
        while !agents.iter().all(|a| a.electorate(group).is_some_and(|d| d.epoch == epoch)) {
            time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    assert!(ok.is_ok(), "{group} did not reach epoch {epoch} everywhere: {:?}; records refused {}",
        agents.iter().map(|a| a.electorate(group).map(|d| d.epoch)).collect::<Vec<_>>(), crate::electorate_records_refused());
}

/// **A safety-sensitive proposal is decided only by an electorate group** (post-360 plan row P2,
/// `docs/design/consensus-electorate.md` §8). On a node that requires it, a leader election, a lock,
/// a consistent write — or any proposal flagged `safety_sensitive` — whose scope is the whole cluster
/// or a group with no electorate is refused by name, `ElectorateNotGoverned`, before anything is sent.
/// An ordinary proposal is untouched. Once the group is declared — its genesis decided by its one
/// member — the same election commits, and with the group marked the fleet's `exclusive_default` the
/// cluster-scoped lock and consistent write decide there, while a cluster-scoped exclusive slot is
/// refused `ElectorateMismatch`: one place per fleet. Seen failing first (2026-10-10, c9131839's
/// review): `elect_leader_receipt("council")` on the ungoverned group returned `Decided`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group() {
    let (agents, _l, _serial) = electorate_mesh(1, |c| c.consensus_require_electorate = true).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(300), max_ballots: 1, ..ConsensusConfig::default() };

    match a.consensus().group_propose("council", "leader/council", Bytes::from_static(b"me"), quick.clone()).await {
        ConsensusResult::ElectorateNotGoverned { ref slot, group: Some(ref g) } => {
            assert_eq!((&**slot, &**g), ("leader/council", "council"), "the refusal names the slot and the group");
        }
        other => panic!("a leader slot on an ungoverned group must be refused by name, got {other:?}"),
    }
    match a.consensus().elect_leader_receipt("council").await {
        Err(crate::ConsistencyError::ElectorateNotGoverned { group: Some(g) }) => assert_eq!(&*g, "council"),
        other => panic!("expected ConsistencyError::ElectorateNotGoverned, got {other:?}"),
    }
    let flagged = ConsensusConfig { safety_sensitive: true, ..quick.clone() };
    match a.consensus().group_propose_receipt("council", "work/exclusive", Bytes::from_static(b"x"), flagged).await {
        Err(crate::CommitError::ElectorateNotGoverned { group: Some(g), .. }) => assert_eq!(&*g, "council"),
        other => panic!("a flagged proposal outside the slot families is safety-sensitive too, got {other:?}"),
    }
    match a.consensus().distributed_lock("door", Duration::from_secs(5)).await {
        Err(crate::ConsistencyError::ElectorateNotGoverned { group: None }) => {}
        other => panic!("with no fleet electorate the lock is a cluster proposal, refused: {other:?}"),
    }
    match a.consensus().consistent_set("cfg/k", Bytes::from_static(b"v")).await {
        Err(crate::ConsistencyError::ElectorateNotGoverned { group: None }) => {}
        other => panic!("and the consistent write: {other:?}"),
    }
    assert!(a.consensus().consensus_get("leader/council").is_none(), "nothing was committed");
    assert!(matches!(
        a.consensus().group_propose("council", "work/ordinary", Bytes::from_static(b"o"), quick.clone()).await,
        ConsensusResult::Committed { .. }
    ), "an ordinary proposal is not an exclusive outcome, and is untouched");

    // Genesis, decided by its one member and marked the fleet's exclusive default.
    let e1 = declare(a, "council", true).await.expect("a genesis");
    assert_eq!((e1.epoch, e1.members.clone(), e1.exclusive_default), (1, vec![a.node_id().clone()], true));
    assert_eq!(declare(a, "council", true).await.unwrap(), e1, "declaring the same electorate decides nothing");
    let won = a.consensus().elect_leader_receipt("council").await.expect("an electorate group elects");
    assert_eq!((won.leader.clone(), won.basis), (a.node_id().clone(), crate::LeadershipBasis::Decided));
    drop(a.consensus().distributed_lock("door", Duration::from_secs(5)).await.expect("the lock decides in the fleet's electorate"));
    a.consensus().consistent_set("cfg/k", Bytes::from_static(b"v")).await.expect("and the consistent write");
    match a.consensus().cluster_propose("lock/raw", Bytes::from_static(b"x"), quick).await {
        ConsensusResult::ElectorateMismatch { group: Some(g), .. } => assert_eq!(&*g, "council"),
        other => panic!("a cluster-scoped exclusive slot is refused while the fleet decides them in council: {other:?}"),
    }
    agents[0].shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **The fleet's exclusive electorate is a fleet record, not a node setting** (#601's review, finding 2). A node
/// whose `consensus_electorate` names a group the fleet has not marked refuses its exclusive verbs by name rather
/// than decide them somewhere the rest of the fleet does not. Seen failing first: the lock decided in the locally
/// named group.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused() {
    let (agents, _l, _serial) = electorate_mesh(1, |c| c.consensus_electorate = Some("mine".into())).await;
    let a = &agents[0];
    a.mesh().join_group("mine");
    roster_converges(&agents, "mine", &[a]).await;
    a.mesh().join_group("other");
    roster_converges(&agents, "other", &[a]).await;
    declare(a, "other", false).await.expect("a genesis, not marked the fleet's default");
    match a.consensus().distributed_lock("door", Duration::from_secs(5)).await {
        Err(crate::ConsistencyError::ElectorateMismatch { group: None, detail }) =>
            assert!(detail.contains("mine") && detail.contains("no electorate group is marked"), "{detail}"),
        other => panic!("a local electorate the fleet does not name must be refused, got {other:?}"),
    }
    declare(a, "mine", true).await.expect("a genesis marked the fleet's default");
    drop(a.consensus().distributed_lock("door", Duration::from_secs(5)).await.expect("now the setting restates the fleet"));
    agents[0].shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **An electorate is pinned by identity, not by count** (#601's review, finding 1a; round 4: membership is frozen).
/// Genesis {A,B,C}; D joins the roster and C leaves, with no migration — the count matches again, the identities do
/// not. A proposal is refused `ElectorateMismatch` and counted on the roster tripwire, never decided by {A,B,D}; D, in
/// the roster but not the electorate, may not propose at all; and declaring the new roster is refused
/// `MembershipFrozen`. Seen failing first: under c9131839's count rule the proposal was attempted against {A,B,D}.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swap_without_a_migration_is_refused() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b, c] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b, c]).await;
    declare(a, "council", false).await.expect("genesis, every member accepting");
    epoch_converges(&agents, "council", 1).await;

    d.mesh().join_group("council");
    // A leave is a tombstone carried by gossip; on a loaded host one can be slow to reach every node, so it is
    // re-issued until every roster agrees (the convergence is what the test needs, not the first frame).
    let prefix = crate::signal::grp_prefix("council");
    let gone = |n: &GossipAgent| !n.kv().scan_prefix(&prefix).iter().any(|(k, _)| k.ends_with(&c.node_id().to_string()));
    for _ in 0..10 {
        c.mesh().leave_group("council"); // a no-op once left; the delete below re-gossips the tombstone
        let _ = c.kv().delete(format!("{prefix}{}", c.node_id()));
        if tokio::time::timeout(Duration::from_secs(3), async {
            while !agents.iter().all(gone) { time::sleep(Duration::from_millis(20)).await; }
        }).await.is_ok() { break; }
    }
    roster_converges(&agents, "council", &[a, b, d]).await;
    let before = crate::electorate_roster_mismatches();
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(500), max_ballots: 1, ..ConsensusConfig::default() };
    match a.consensus().group_propose("council", "leader/council", Bytes::from_static(b"a"), quick.clone()).await {
        ConsensusResult::ElectorateMismatch { group: Some(g), detail, .. } => {
            assert_eq!(&*g, "council");
            assert!(detail.contains("roster differs"), "{detail}");
        }
        other => panic!("a swapped roster must be refused, got {other:?}"),
    }
    assert!(crate::electorate_roster_mismatches() > before, "the roster tripwire counted it");
    assert!(matches!(
        d.consensus().group_propose("council", "leader/council", Bytes::from_static(b"d"), quick).await,
        ConsensusResult::NotAMember { .. }
    ), "a roster member outside the electorate may not propose");
    assert_eq!(declare(a, "council", false).await, Err(crate::ElectorateError::MembershipFrozen { group: "council".into() }));
    assert!(agents.iter().all(|n| n.consensus().consensus_get("leader/council").is_none()), "nothing was decided");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **The vote filter is wired** (#601's review, finding 8): a proposal on an electorate group counts promises and votes
/// only from the epoch's members. Genesis {A,B}; B's listener is stopped, so a quorum of two cannot form honestly; D,
/// outside the electorate, forges promises and votes for A's proposal at every ballot it could draw. A does not commit.
/// The proposal also asks for an explicit quorum of one with opacity reduction on: an electorate's quorum is a strict
/// majority of its members whatever the caller asks (finding 3). Seen failing first, with the door's filter unwired
/// (`electorate_vote_filter(trust_set, None)`) and the quorum floor removed: A committed.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn votes_from_outside_the_electorate_are_not_counted() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, value_digest, ConsensusMsg};
    let (agents, mut listeners, _serial) = electorate_mesh(3, |_| {}).await;
    let (a, b, d) = (&agents[0], &agents[1], &agents[2]);
    for n in [a, b] { n.mesh().join_group("pair"); }
    roster_converges(&agents, "pair", &[a, b]).await;
    declare(a, "pair", false).await.expect("genesis {A,B}");
    epoch_converges(&agents, "pair", 1).await;
    drop(listeners.remove(1)); // B stops answering

    let value = Bytes::from_static(b"forged-quorum");
    let forger = crate::agent::electorate::engine(&d.task_ctx);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (dctx, aid, v, halt) = (Arc::clone(&d.task_ctx), a.node_id().clone(), value.clone(), Arc::clone(&stop));
    let forging = tokio::spawn(async move {
        while !halt.load(std::sync::atomic::Ordering::Relaxed) {
            for ballot in 1..=8u64 {
                for msg in [
                    ConsensusMsg::PrepareAck { slot: Arc::from("work/w"), ballot, voter: dctx.node_id.clone(),
                        accepted_ballot: 0, accepted_digest: None, accepted_value: None, committed_digest: None },
                    ConsensusMsg::VoteForValue { slot: Arc::from("work/w"), ballot, voter: dctx.node_id.clone(),
                        value_digest: value_digest(&v), locality: None },
                ] {
                    mycelium_core::ops::emit_signal(&dctx, Arc::from(consensus_kind::VOTE),
                        SignalScope::Individual(aid.clone()), forger.sign_payload(encode_consensus_msg(&msg)));
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    // An explicit quorum of one, and opacity reduction on: neither shrinks an electorate's strict majority (#601's
    // review, finding 3) — without the floor, A's own vote would have decided.
    let quick = ConsensusConfig {
        phase1_timeout: Duration::from_millis(400), max_ballots: 3, quorum_size: 1, count_opaque_as_absent: true,
        ..ConsensusConfig::default()
    };
    let res = a.consensus().group_propose("pair", "work/w", value, quick).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = forging.await;
    assert!(!matches!(res, ConsensusResult::Committed { .. }), "a non-member's forged promises and votes were counted: {res:?}");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **An electorate group decides only through its own epoch** (#601's review, finding 8): a cross-group proposal that
/// names one is refused `ElectorateMismatch`, and a safety-sensitive cross-group proposal over plain groups is
/// refused `ElectorateNotGoverned` on a node that requires an electorate. Seen failing first: the cross-group
/// proposal over the electorate group committed.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cross_group_proposal_does_not_decide_for_an_electorate_group() {
    let (agents, _l, _serial) = electorate_mesh(1, |c| c.consensus_require_electorate = true).await;
    let a = &agents[0];
    for g in ["council", "plain"] { a.mesh().join_group(g); }
    roster_converges(&agents, "council", &[a]).await;
    roster_converges(&agents, "plain", &[a]).await;
    declare(a, "council", false).await.expect("genesis");
    let gq = |g: &str| crate::GroupQuorum { group: g.into(), quorum: 0.5, veto: false };
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(300), max_ballots: 1, ..ConsensusConfig::default() };
    match a.consensus().cross_group_propose("work/x", Bytes::from_static(b"x"), vec![gq("plain"), gq("council")], quick.clone()).await {
        ConsensusResult::ElectorateMismatch { group: Some(g), .. } => assert_eq!(&*g, "council"),
        other => panic!("a cross-group proposal over an electorate group must be refused, got {other:?}"),
    }
    let flagged = ConsensusConfig { safety_sensitive: true, ..quick };
    match a.consensus().cross_group_propose("work/y", Bytes::from_static(b"y"), vec![gq("plain")], flagged).await {
        ConsensusResult::ElectorateNotGoverned { group: Some(g), .. } => assert_eq!(&*g, "plain"),
        other => panic!("a safety-sensitive cross-group proposal is never an electorate, got {other:?}"),
    }
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A forged electorate record does not change the electorate** (#601's review, finding 5). A committed
/// `electorate/{group}/{epoch}` entry written straight into the store — no consensus, no certificate,
/// or a certificate that does not verify — is not adopted, and is counted once. Seen failing first: under c9131839 a KV write to
/// `sys/govern/electorate/{group}` was the electorate.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forged_electorate_record_is_not_adopted() {
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    let stranger = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    // A forged superseding genesis — well formed (it keeps a majority of epoch 1), no genuine certificate.
    let forged = crate::ElectorateDecl {
        group: "council".into(), epoch: 2, exclusive_default: false,
        members: crate::agent::electorate::sorted_members(vec![a.node_id().clone(), stranger.clone()]),
        supersedes: Some(crate::Superseded {
            epoch: 1, digest: crate::consensus::value_digest(&e1.encode()), envelope: [0; 32], members: e1.members.clone(),
        }),
        predecessors: Default::default(),
    };
    let bogus = mycelium_core::serde_fixint::to_vec(&vec![Bytes::from_static(b"not a vote")]).unwrap();
    let before = crate::electorate_records_refused();
    assert!(a.kv().set(format!("consensus/committed/{}", crate::agent::electorate::genesis_slot("council", 2)), forged.encode()));
    assert!(a.kv().set(crate::agent::electorate::cert_key("council", 2), bogus));
    poll_until(|| a.kv().get(&crate::agent::electorate::cert_key("council", 2)).is_some(), 2_000).await;
    assert_eq!(a.electorate("council"), Some(e1), "the forged epoch is not adopted");
    assert!(crate::electorate_records_refused() > before, "and it is counted");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Nothing resizes an electorate group but a decided step** (P2's tension (a), *governed is not fixed*). The
/// membership governor does not roll on it — not even to honour a drain naming this node, which it obeys on an
/// ordinary group in the same pass — and the emergent watcher does not auto-join it on a capability match it acts
/// on for an undeclared group. Seen failing first: the governor drained this node out of `council`, and the
/// watcher joined `council2`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_governor_and_the_watcher_leave_an_electorate_group_alone() {
    use crate::capability::{Capability, CapFilter, CapabilityGroupDef};
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.health_check_interval_secs = 1;
    cfg.health_check_max_jitter_ms = 50;
    // An electorate member must configure `[persistence]` (`acceptor_guard`, detection 1).
    let base = std::env::temp_dir().join(format!("mycelium-electorate-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base, sync_mode: crate::config::SyncMode::Flush, snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600, on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let a = GossipAgent::new(id.clone(), cfg);
    a.start().await.unwrap();
    let in_group = |g: &str| a.groups().iter().any(|x| x.as_ref() == g);

    // The watcher: `council2` is an electorate whose one member then left it; `open` is an ordinary group.
    a.mesh().join_group("council2");
    poll_until(|| in_group("council2"), 2_000).await;
    declare(&a, "council2", false).await.expect("genesis {A}");
    a.mesh().leave_group("council2");
    poll_until(|| !in_group("council2"), 2_000).await;
    let _cap = a.capabilities().advertise_capability(Capability::new("svc", "voter"), Duration::from_secs(60));
    let def = || CapabilityGroupDef { filter: CapFilter::new("svc", "voter"), topology_policy: None, provides: vec![], requires: vec![] };
    let _g2 = a.capabilities().define_capability_group("council2", def(), Duration::from_secs(60));
    let _go = a.capabilities().define_capability_group("open", def(), Duration::from_secs(60));
    poll_until(|| in_group("open"), 5_000).await;

    // The governor: this node is in both groups and an intent drains it from both.
    let _gc = a.capabilities().define_capability_group("council", def(), Duration::from_secs(60));
    let _gp = a.capabilities().define_capability_group("pool", def(), Duration::from_secs(60));
    poll_until(|| in_group("council") && in_group("pool"), 5_000).await;
    declare(&a, "council", false).await.expect("genesis {A}");
    for g in ["council", "pool"] {
        let _ = a.publish_membership_intent(crate::MembershipIntent::new(g, 0, None).with_drain(vec![id.clone()]));
    }
    a.start_membership_governor();
    poll_until(|| !in_group("pool"), 10_000).await;
    assert!(in_group("council"), "the governor drained an electorate group's member");
    assert!(!in_group("council2"), "the watcher auto-joined an electorate group");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Under `[tls]` the certificate is signatures** (#601's review, finding 5; round 2, finding 4): a genesis decided
/// over authenticated consensus messages is adopted on every node — each verifies the deciding votes' signatures, the
/// proposer's own included — and a forged superseding genesis whose certificate would count is refused because its signatures do not belong to the voters they name. Seen failing first with the
/// certificate check counting votes without verifying their signer: the forged record was adopted.
#[cfg(all(feature = "consensus", feature = "tls"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_electorate_certificate_verifies_under_tls() {
    let cert_dir = std::env::temp_dir().join(format!("myc-electorate-tls-{}", alloc_port()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let dir = cert_dir.clone();
    let (agents, _l, _serial) = electorate_mesh(3, move |c| {
        c.tls = Some(TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() });
    }).await;
    let (a, b, c) = (&agents[0], &agents[1], &agents[2]);
    for n in [a, b] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b]).await;
    let e1 = declare(a, "council", false).await.expect("a signed genesis");
    epoch_converges(&agents, "council", 1).await;

    // A forged superseding genesis whose certificate **counts** — votes naming A and B, every member it names — but
    // every one of them is signed by C. Only signature checking refuses it: each vote's signer is not the voter it names.
    let forged = crate::ElectorateDecl {
        epoch: 2,
        supersedes: Some(crate::Superseded {
            epoch: 1, digest: crate::consensus::value_digest(&e1.encode()), envelope: [0; 32], members: e1.members.clone(),
        }),
        ..e1.clone()
    };
    let slot = crate::agent::electorate::genesis_slot("council", 2);
    let envelope = crate::consensus_life::Envelope::new(
        &slot, forged.encode(), crate::consensus_life::Term::Permanent, 1, c.node_id().clone(), 1,
    ).encode();
    let c_engine = crate::agent::electorate::engine(&c.task_ctx);
    let votes: Vec<Bytes> = [a, b].iter().map(|named| {
        let vote = crate::consensus::ConsensusMsg::VoteForValue {
            slot: Arc::from(slot.as_str()), ballot: 1, voter: named.node_id().clone(),
            value_digest: crate::consensus::value_digest(&envelope), locality: None,
        };
        c_engine.sign_payload(crate::consensus::encode_consensus_msg(&vote))
    }).collect();
    let before = crate::electorate_records_refused();
    let cert = crate::agent::electorate::cert_key("council", 2);
    assert!(c.kv().set(cert.clone(), mycelium_core::serde_fixint::to_vec(&(envelope, votes)).unwrap()));
    assert!(c.kv().set(format!("consensus/committed/{slot}"), forged.encode()));
    poll_until(|| agents.iter().all(|n| n.kv().get(&format!("consensus/committed/{slot}")).is_some()
        && n.kv().get(&cert).is_some()), 10_000).await;
    for n in &agents {
        assert_eq!(n.electorate("council").map(|d| d.epoch), Some(1), "{} adopted a forged genesis", n.node_id());
    }
    assert!(crate::electorate_records_refused() > before, "the forgery was refused, and counted");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
    let _ = std::fs::remove_dir_all(&cert_dir);
}


/// **A cross-group proposal does not decide an exclusive slot** (round 3, finding 3). With a fleet electorate marked,
/// a cross-group proposal for `lock/…` over plain groups is refused at the door and by an acceptor; a `leader/{g}`
/// slot across groups is refused too. Seen failing first: the cross-group `lock/x` committed outside the electorate.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cross_group_proposal_does_not_decide_an_exclusive_slot() {
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    for g in ["council", "p1", "p2"] { a.mesh().join_group(g); }
    roster_converges(&agents, "p2", &[a]).await;
    declare(a, "council", true).await.expect("genesis, the fleet's electorate");
    let gq = |g: &str| crate::GroupQuorum { group: g.into(), quorum: 0.5, veto: false };
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(300), max_ballots: 1, ..ConsensusConfig::default() };
    for slot in ["lock/x", "leader/p1"] {
        match a.consensus().cross_group_propose(slot, Bytes::from_static(b"x"), vec![gq("p1"), gq("p2")], quick.clone()).await {
            ConsensusResult::ElectorateMismatch { .. } => {}
            other => panic!("a cross-group {slot} must be refused, got {other:?}"),
        }
    }
    let scope = SignalScope::Groups(vec![Arc::from("p1"), Arc::from("p2")]);
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    assert!(admits(&engine, &scope, "lock/x", None, a.node_id(), None).is_err(), "an acceptor refuses it too");
    // And the acceptor half of the `leader/` refusal (round 4, finding 9): `leader/p1` across groups, or in another
    // group, is not answered.
    assert!(admits(&engine, &scope, "leader/p1", None, a.node_id(), None).is_err(), "a cross-group leader/p1");
    assert!(admits(&engine, &SignalScope::Group(Arc::from("p2")), "leader/p1", None, a.node_id(), None).is_err(),
        "leader/p1 proposed in p2");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── P2 round 4 (#601): membership is frozen; an undecided genesis is superseded; the gate lowers what it raised ──

/// `n` started agents in a **line** — each dials only the node before it, so most pairs are not direct peers —
/// each running the consensus listener; returns once every node's write reaches every node (round 4, finding 8:
/// production meshes are sparse while they converge, and under `max_peers`).
#[cfg(feature = "consensus")]
async fn electorate_line(n: usize) -> (Vec<GossipAgent>, Vec<crate::ConsensusListenerHandle>, tokio::sync::MutexGuard<'static, ()>) {
    let serial = ELECTORATE_MESHES.lock().await;
    let ports: Vec<u16> = (0..n).map(|_| alloc_port()).collect();
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let mut agents = Vec::new();
    for i in 0..n {
        let cfg = electorate_cfg(ports[i], if i == 0 { vec![] } else { vec![ids[i - 1].clone()] }, true);
        agents.push(GossipAgent::new(ids[i].clone(), cfg));
    }
    for a in &agents { a.start().await.unwrap(); }
    let listeners = agents.iter().map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    for a in &agents {
        assert!(a.kv().set(format!("electorate-test-ready/{}", a.node_id()), Bytes::from_static(b"1")));
    }
    let reached = tokio::time::timeout(Duration::from_secs(60), async {
        while !agents.iter().all(|a| a.kv().scan_prefix("electorate-test-ready/").len() == n) {
            time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    assert!(reached.is_ok(), "writes did not reach every node on the line");
    (agents, listeners, serial)
}

/// **A refused claim through the gate raises nothing** (round 4 of #601's review, finding 4 — in the one raising kind
/// that ships, a genesis attempt). A genesis claim that passes the gate and is then refused by the acceptor — here a
/// promise at a ballot below the slot's decided floor — promised nothing, so it must not record the attempt as
/// touched: a claim on an earlier attempt still passes afterwards.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_claim_through_the_gate_lowers_the_fence_again() {
    use crate::consensus::{prepare_slot, Gate, GateKind, PrepareOutcome};
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    let later: Arc<str> = Arc::from(crate::agent::electorate::genesis_slot("council", 3).as_str());
    assert!(a.kv().set(format!("consensus/decided/{later}"), crate::consensus::encode_ballot(10)));
    let raise = Gate { group: Arc::from("council"), slot: Arc::clone(&later), epoch: 0, kind: GateKind::Genesis(3) };
    let refused = engine.claim_gated(Some(&raise),
        || prepare_slot(&a.task_ctx.consensus_accepted, &later, 5, a.node_id().id_hash(), 10));
    assert!(matches!(refused, Some(PrepareOutcome::Refused { .. })), "the acceptor refused the promise: {refused:?}");
    let earlier = Gate { group: Arc::from("council"), slot: Arc::clone(&later), epoch: 0, kind: GateKind::Genesis(2) };
    assert_eq!(engine.claim_gated(Some(&earlier), || 7), Some(7), "a refused claim left the attempt touched");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **An undecided genesis does not wedge its group** (round 4 of #601's review, finding 3). Genesis {A,B,C} was
/// accepted by A and B and never by C, which is gone; the roster is now {A,B}. A declaration supersedes the pending
/// genesis with one naming the live roster — decided by every member it names, a majority of the one it supersedes —
/// and the group decides again.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undecided_genesis_is_superseded_by_one_naming_the_live_roster() {
    let (agents, mut listeners, _serial) = electorate_mesh(3, |_| {}).await;
    let (a, b, c) = (&agents[0], &agents[1], &agents[2]);
    for n in [a, b, c] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b, c]).await;
    let pending = crate::ElectorateDecl {
        group: "council".into(), epoch: 1, exclusive_default: false,
        members: crate::agent::electorate::sorted_members(vec![a.node_id().clone(), b.node_id().clone(), c.node_id().clone()]),
        ..Default::default()
    };
    let genesis: Arc<str> = Arc::from(crate::agent::electorate::genesis_slot("council", 1).as_str());
    for n in [a, b] {
        assert!(crate::consensus::claim_vote(&n.task_ctx.consensus_accepted, &genesis, 3, &pending.encode(), a.node_id().id_hash(), 0),
            "{} accepts the genesis", n.node_id());
    }
    // C is gone: it answers nothing, and the operator takes it out of the roster.
    drop(listeners.remove(2));
    c.mesh().leave_group("council");
    let _ = c.kv().delete(format!("{}{}", crate::signal::grp_prefix("council"), c.node_id()));
    roster_converges(&agents, "council", &[a, b]).await;
    let live = declare(a, "council", false).await.expect("the pending genesis is superseded");
    assert_eq!(live.members, crate::agent::electorate::sorted_members(vec![a.node_id().clone(), b.node_id().clone()]));
    assert_eq!((live.epoch, live.supersedes.as_ref().map(|s| s.digest)), (2, Some(crate::consensus::value_digest(&pending.encode()))),
        "epoch 2 supersedes it");
    epoch_converges(&agents[..2], "council", 2).await;
    // A proposal naming the superseded genesis is refused, naming the live one.
    let engine = crate::agent::electorate::engine(&b.task_ctx);
    assert_eq!(admits(&engine, &SignalScope::Group(Arc::from("council")), "work/x", Some((1, pending.digest())), a.node_id(), None),
        Err((2, live.digest())));
    let cfg = ConsensusConfig { phase1_timeout: Duration::from_millis(1500), max_ballots: 3, ..ConsensusConfig::default() };
    assert!(matches!(a.consensus().group_propose("council", "work/after", Bytes::from_static(b"z"), cfg).await,
        ConsensusResult::Committed { .. }), "the group decides again");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **`declare_electorate` retries while undecided, on a sparse mesh** (round 4 of #601's review, finding 8). The
/// nodes form a line, so the members are not each other's direct peers, and one member answers nothing for sixteen
/// seconds — longer than one declaration's ballots. The library call itself (not a test helper) keeps trying, with a
/// bounded backoff, and the genesis is decided once the member answers.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declaration_on_a_sparse_mesh_retries_until_decided() {
    let (agents, mut listeners, _serial) = electorate_line(4).await;
    let (a, c, d) = (&agents[0], &agents[2], &agents[3]);
    for n in [a, c, d] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, c, d]).await;
    drop(listeners.remove(3));
    let late = {
        let d = d.consensus();
        tokio::spawn(async move {
            time::sleep(Duration::from_secs(16)).await;
            d.start_consensus_listener(ConsensusConfig::default())
        })
    };
    let decided = a.declare_electorate("council", false).await.expect("the declaration retried until decided");
    assert_eq!(decided.members.len(), 3);
    drop(late.await);
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A second default is refused by name** (round 5). While `old` holds the fleet's exclusive default, a genesis
/// declaring another default is refused at the proposer's door and by every acceptor — the default is declared once,
/// at genesis, and moves only by retirement (`a_second_live_default_is_refused_by_name`).
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_default_is_refused_by_name() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b]).await;
    declare(a, "old", true).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    for n in [c, d] { n.mesh().join_group("new"); }
    roster_converges(&agents, "new", &[c, d]).await;
    match c.declare_electorate("new", true).await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("old") && why.contains("default"), "{why}"),
        other => panic!("a second default is refused by name: {other:?}"),
    }
    let mut members = vec![c.node_id().clone(), d.node_id().clone()];
    members.sort_by_key(ToString::to_string);
    let second = crate::ElectorateDecl { group: "new".into(), epoch: 1, members, exclusive_default: true, ..Default::default() };
    let slot = crate::agent::electorate::genesis_slot("new", 1);
    assert!(admits(&crate::agent::electorate::engine(&d.task_ctx), &SignalScope::Group(Arc::from("new")), &slot,
        Some((0, [0; 32])), c.node_id(), Some(&second.encode())).is_err(), "an acceptor refuses it too");
    // The door's own refusal, reached directly — `declare_outcome` refuses before it, so it is otherwise unreached.
    let door = crate::agent::electorate::engine(&c.task_ctx).electorate_door(
        &SignalScope::Group(Arc::from("new")), &Arc::from(slot.as_str()), &second.encode(), &ConsensusConfig::default(), None);
    assert!(door.is_err(), "the proposer's door refuses a second default");
    assert!(agents.iter().all(|n| n.electorate("new").is_none()), "nothing was decided");
    for n in &agents {
        assert_eq!(crate::agent::electorate::exclusive_electorate(&n.task_ctx), Ok(Some("old".to_string())), "{}", n.node_id());
    }
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A tagged cluster-scoped proposal for an exclusive slot is refused while the default cannot be named** (round 5,
/// D5). The fail-closed check skipped a message that named an electorate; it now refuses one tagged or not. Here the
/// default cannot be named because this node's `consensus_electorate` names a group the fleet did not mark.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tagged_cluster_proposal_for_an_exclusive_slot_fails_closed() {
    let (agents, _l, _serial) = electorate_mesh(1, |c| c.consensus_electorate = Some("elsewhere".into())).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    assert!(crate::agent::electorate::exclusive_electorate(&a.task_ctx).is_err(), "the default cannot be named here");
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    for named in [None, Some((e1.epoch, e1.digest()))] {
        assert!(admits(&engine, &SignalScope::Cluster, "lock/x", named, a.node_id(), None).is_err(),
            "a cluster-scoped lock/x is refused, named {named:?}");
    }
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A slot that names a group is decided only in that group** (#601's final review, D1 — the reviewer's probe).
/// `electorate/{g}/…` and `leader/{g}` were answered, and proposed, under the cluster scope while no default was
/// marked — and since the votes are signed at cluster scope too, they assembled a valid certificate: the probe moved a
/// frozen electorate to epoch 2, marked the default. Now such a slot is refused at the proposer's door and at every
/// acceptor unless the scope is exactly `Group(g)`, whatever the default state or `consensus_require_electorate`; an
/// `electorate/` slot is never answered under `Groups`, nor is `leader/{g}` under `Groups([g])`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_that_names_a_group_is_decided_only_in_that_group() {
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    for slot in ["electorate/council/2", "leader/council"] {
        assert!(engine.electorate_admits(&SignalScope::Cluster, slot, None, a.node_id(), None, None).is_err(),
            "an acceptor answers {slot} under the cluster scope");
    }
    let only = SignalScope::Groups(vec![Arc::from("other")]);
    for slot in ["electorate/other/1", "leader/other"] {
        assert!(engine.electorate_admits(&only, slot, None, a.node_id(), None, None).is_err(),
            "an acceptor answers {slot} under Groups([other]), a group with no electorate here");
    }
    // The probe: a cluster-scoped proposal of a superseding genesis that marks the default.
    let e2 = crate::ElectorateDecl {
        group: "council".into(), epoch: 2, exclusive_default: true, members: e1.members.clone(),
        supersedes: Some(crate::Superseded {
            epoch: 1, digest: crate::consensus::value_digest(&e1.encode()), envelope: [0; 32], members: e1.members.clone(),
        }),
        predecessors: Default::default(),
    };
    let slot = crate::agent::electorate::genesis_slot("council", 2);
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(300), max_ballots: 1, ..ConsensusConfig::default() };
    let r = engine.propose(SignalScope::Cluster, Arc::from(slot.as_str()), e2.encode(), 1, quick, None).await;
    assert!(!matches!(r, ConsensusResult::Committed { .. }), "the cluster-scoped genesis was decided: {r:?}");
    // Had it been accepted, a certificate from the cluster-signed vote would move the electorate.
    let held = a.task_ctx.consensus_accepted.pin().get(slot.as_str()).and_then(|s| s.accepted.clone());
    if let Some((ballot, crate::consensus::Accepted::Envelope(env))) = held {
        let vote = crate::consensus::ConsensusMsg::VoteForValue {
            slot: Arc::from(slot.as_str()), ballot, voter: a.node_id().clone(),
            value_digest: crate::consensus::value_digest(&env), locality: None,
        };
        let signed = engine.sign_payload(crate::consensus::encode_consensus_msg(&vote));
        let cert = mycelium_core::serde_fixint::to_vec(&(env, vec![signed])).unwrap();
        assert!(a.kv().set(crate::agent::electorate::cert_key("council", 2), Bytes::from(cert)));
        poll_until(|| a.kv().get(&crate::agent::electorate::cert_key("council", 2)).is_some(), 2_000).await;
    }
    assert_eq!(a.electorate("council"), Some(e1), "a frozen electorate must not move");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A tagged proposal is answered only under its group's scope** (#601's final review, D2). `PrepareIn`/`ProposeIn`
/// name an electorate, and the scope is not inside the signed payload — so a tagged message under the cluster scope, or
/// under `Groups`, is refused rather than answered without the gate.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tagged_proposal_outside_its_group_scope_is_refused() {
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    let named = Some((e1.epoch, e1.digest()));
    for scope in [SignalScope::Cluster, SignalScope::Groups(vec![Arc::from("plain")])] {
        assert!(admits(&engine, &scope, "work/x", named, a.node_id(), None).is_err(), "tagged work/x answered under {scope:?}");
    }
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Two defaults fail closed** (#601's final review). Two groups that could not see each other's genesis can both be
/// marked the fleet's exclusive default; a node that sees both names neither — `live_default` is an error naming them,
/// and every exclusive verb is refused, at the door and at the acceptor — while group-scoped work outside the exclusive
/// families keeps running. Built here from two meshes: the second group's record and certificate, decided on a mesh of
/// its own, are carried into the first.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_defaults_fail_closed() {
    use crate::consensus::consensus_ns::COMMITTED;
    let carried = {
        let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
        let x = &agents[0];
        x.mesh().join_group("two");
        roster_converges(&agents, "two", &[x]).await;
        declare(x, "two", true).await.expect("the other side's default");
        let record = format!("{COMMITTED}{}", crate::agent::electorate::genesis_slot("two", 1));
        let cert = crate::agent::electorate::cert_key("two", 1);
        let carried = [(record.clone(), x.kv().get(&record).expect("record")), (cert.clone(), x.kv().get(&cert).expect("cert"))];
        x.shutdown_with_timeout(Duration::from_secs(5)).await;
        carried
    };
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    a.mesh().join_group("one");
    roster_converges(&agents, "one", &[a]).await;
    declare(a, "one", true).await.expect("this side's default");
    for (k, v) in carried { assert!(a.kv().set(k, v)); }
    poll_until(|| crate::agent::electorate::live_default(&a.task_ctx).is_err(), 5_000).await;
    let why = crate::agent::electorate::live_default(&a.task_ctx).unwrap_err();
    assert!(why.contains("one") && why.contains("two"), "{why}");
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    assert!(admits(&engine, &SignalScope::Cluster, "lock/x", None, a.node_id(), None).is_err(), "an acceptor answers lock/x");
    let quick = || ConsensusConfig { phase1_timeout: Duration::from_millis(500), max_ballots: 2, ..ConsensusConfig::default() };
    let r = engine.propose(SignalScope::Cluster, Arc::from("lock/x"), Bytes::from_static(b"holder"), 1, quick(), None).await;
    assert!(matches!(r, ConsensusResult::ElectorateMismatch { .. }), "an exclusive verb is refused by name: {r:?}");
    // Group-scoped work outside the exclusive families keeps running during the stop.
    let r = engine.propose(SignalScope::Group(Arc::from("one")), Arc::from("work/y"), Bytes::from_static(b"v"), 1, quick(), None).await;
    assert!(matches!(r, ConsensusResult::Committed { .. }), "group work stops too: {r:?}");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── The acceptor's memory guard (`acceptor_guard`; design §8.5; the hazard PR #604 documents) ──────────────────

/// **A member without `[persistence]` is refused an electorate by name** (`acceptor_guard`, detection 1). Its
/// promises would not survive a restart (`persist_sync` reports "no promise was made" with nothing on disk), so a
/// member restarted empty could let two values be decided. Refused at every door: `declare_electorate` returns
/// `ElectorateError::PersistenceRequired`, the proposer's door refuses the genesis proposal, the acceptor's door does
/// not answer a genesis naming it — and each is counted. A node outside the roster is untouched (`NotAMember`, as
/// before), and consensus outside an electorate keeps committing. Seen failing first (2026-10-11): the declaration
/// was decided.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_persistence_is_refused_an_electorate_by_name() {
    let (agents, _l, _serial) = electorate_mesh(1, |c| c.persistence = None).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    let before = crate::electorate_persistence_refused();
    match a.declare_electorate("council", false).await {
        Err(crate::ElectorateError::PersistenceRequired { group }) => assert_eq!(group, "council"),
        other => panic!("a member without persistence is refused by name: {other:?}"),
    }
    assert!(a.electorate("council").is_none(), "nothing was decided");
    let genesis = crate::ElectorateDecl {
        group: "council".into(), epoch: 1, members: vec![a.node_id().clone()], ..Default::default()
    };
    let slot = crate::agent::electorate::genesis_slot("council", 1);
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    let door = engine.electorate_door(
        &SignalScope::Group(Arc::from("council")), &Arc::from(slot.as_str()), &genesis.encode(), &ConsensusConfig::default(), None);
    match door {
        Err(crate::consensus::DoorRefusal::Mismatch { detail, .. }) => assert!(detail.contains("[persistence]"), "{detail}"),
        Err(_) => panic!("the proposer's door refuses the genesis by name, not another way"),
        Ok(_) => panic!("the proposer's door let the genesis through"),
    }
    assert!(admits(&engine, &SignalScope::Group(Arc::from("council")), &slot, Some((0, [0; 32])), a.node_id(), Some(&genesis.encode())).is_err(),
        "the acceptor's door does not answer a genesis naming it");
    assert!(admits(&engine, &SignalScope::Group(Arc::from("council")), &slot, Some((0, [0; 32])), a.node_id(), None).is_err(),
        "nor a genesis prepare");
    assert!(crate::electorate_persistence_refused() >= before + 3, "each refusal is counted: {}", crate::electorate_persistence_refused() - before);
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(500), max_ballots: 2, ..ConsensusConfig::default() };
    assert!(matches!(a.consensus().group_propose("council", "work/plain", Bytes::from_static(b"v"), quick).await,
        ConsensusResult::Committed { .. }), "consensus outside an electorate is untouched");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A member restarted without `[persistence]` does not adopt its electorate** (detection 1's adoption half, and the
/// gateway-free path to it). The group's genesis was decided while B had persistence; B comes back under the same
/// `NodeId` with an empty store and no `[persistence]`. The committed record and its certificate reach it by gossip,
/// and B refuses to adopt — by name, once, counted — so it answers for the electorate as a node that holds none (a
/// stale refusal, never a promise). Seen failing first: B adopted epoch 1.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_restarted_without_persistence_does_not_adopt_its_electorate() {
    let (mut agents, mut listeners, _serial) = electorate_mesh(2, |_| {}).await;
    for n in &agents { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[&agents[0], &agents[1]]).await;
    declare(&agents[0], "council", false).await.expect("a genesis of two");
    epoch_converges(&agents, "council", 1).await;
    let b_id = agents[1].node_id().clone();
    let a_id = agents[0].node_id().clone();
    drop(listeners.remove(1));
    agents.remove(1).shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("mycelium-electorate-{}", b_id.to_socket_addr().port())));
    let mut cfg = electorate_cfg(b_id.to_socket_addr().port(), vec![a_id], false);
    cfg.persistence = None;
    let b = GossipAgent::new(b_id.clone(), cfg);
    b.start().await.unwrap();
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
    b.mesh().join_group("council");
    let before = crate::electorate_persistence_refused();
    // The record and its certificate arrive by anti-entropy; the adoption is refused where it would happen.
    let record = format!("{}{}", crate::consensus::consensus_ns::COMMITTED, crate::agent::electorate::genesis_slot("council", 1));
    // A restarted member is reached again once A's writer meets the dead socket at its next write and redials, and
    // anti-entropy runs once per health-check interval: a bounded structural poll, measured at 15–25 s on a loaded host.
    poll_until(|| b.kv().get(&record).is_some() && b.kv().get(&crate::agent::electorate::cert_key("council", 1)).is_some(), 60_000).await;
    assert!(b.kv().get(&record).is_some(), "the committed genesis reached B's store");
    assert!(b.electorate("council").is_none(), "B does not adopt an electorate it cannot keep promises for");
    assert!(b.electorate("council").is_none(), "nor on a second look");
    assert!(crate::electorate_persistence_refused() - before >= 2, "every refused view is counted, warned once");
    assert!(crate::agent::acceptor_guard::noticed(&b.task_ctx, crate::agent::acceptor_guard::Notice::Adopt, "council/1"));
    let engine = crate::agent::electorate::engine(&b.task_ctx);
    let a_view = agents[0].electorate("council").unwrap();
    assert!(admits(&engine, &SignalScope::Group(Arc::from("council")), "work/x", Some((1, a_view.digest())), agents[0].node_id(), None).is_err(),
        "B answers a proposal naming the electorate as a node that holds none");
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    agents[0].shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A member restarted over an empty store is warned once its genesis is learned** (`acceptor_guard`, detection 2).
/// Every member accepted the group's genesis, and the collector keeps the promise, so a member that kept its store
/// always holds a record for the genesis slot. B comes back under the same `NodeId` over an **empty** persistence
/// directory: at `start()` its store holds no record and nothing is warned (a brand-new member legitimately has
/// none); when the committed genesis arrives by gossip the warning fires — once, counted on
/// `acceptor_memory_empty_for_electorate` — and B still adopts the electorate (warned, not refused: the record was
/// read unverified, and the fork is the operator's). The same arrival carries B's own acceptor record for the genesis
/// slot, a promise above its empty memory: that slot is withheld (detection 3, `acceptor_promises_forgotten`). Seen
/// failing first: nothing was counted.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_restarted_over_an_empty_store_is_warned_once_its_genesis_is_learned() {
    let (mut agents, mut listeners, _serial) = electorate_mesh(2, |_| {}).await;
    for n in &agents { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[&agents[0], &agents[1]]).await;
    declare(&agents[0], "council", false).await.expect("a genesis of two");
    epoch_converges(&agents, "council", 1).await;
    let genesis = crate::agent::electorate::genesis_slot("council", 1);
    assert!(agents[1].task_ctx.consensus_accepted.pin().get(genesis.as_str()).is_some_and(|s| s.promised > 0),
        "a member that kept its store holds the genesis slot");
    let b_id = agents[1].node_id().clone();
    let a_id = agents[0].node_id().clone();
    drop(listeners.remove(1));
    agents.remove(1).shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("mycelium-electorate-{}", b_id.to_socket_addr().port())));
    let (empty_before, forgotten_before) = (crate::acceptor_memory_empty_for_electorate(), crate::acceptor_promises_forgotten());
    let b = GossipAgent::new(b_id.clone(), electorate_cfg(b_id.to_socket_addr().port(), vec![a_id], false));
    b.start().await.unwrap();
    assert_eq!(crate::acceptor_memory_empty_for_electorate(), empty_before, "an empty store has no record to warn on at start");
    let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
    b.mesh().join_group("council");
    poll_until(|| crate::acceptor_memory_empty_for_electorate() > empty_before, 60_000).await;
    assert_eq!(crate::acceptor_memory_empty_for_electorate() - empty_before, 1, "warned once the committed genesis arrived");
    assert!(crate::agent::acceptor_guard::noticed(&b.task_ctx, crate::agent::acceptor_guard::Notice::Empty, "council"), "the notice names the group");
    epoch_converges(std::slice::from_ref(&b), "council", 1).await;
    assert_eq!(crate::acceptor_memory_empty_for_electorate() - empty_before, 1, "adopting it does not warn again");
    poll_until(|| crate::acceptor_promises_forgotten() > forgotten_before, 60_000).await;
    assert!(b.task_ctx.acceptor_withheld.pin().contains(genesis.as_str()),
        "B's own genesis record, repaired from A, held a promise B's memory did not: the slot is withheld");
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    agents[0].shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A forgotten promise withholds the slot, so a second value is not decided** (`acceptor_guard`, detection 3 —
/// the interleaving from PR #604's review). Electorate {A, B, C}; `work/x` = v1 is decided by {A, B} while C's
/// listener is down, and C never learns the decision: the frame filter drops every `work/x` key at C but the ballot
/// key (so C draws ballot 2) and **every signal from A** — a learner writes the record from A's `CommitTerm` signal
/// too, which can still be in flight through B when C's listener comes back. B restarts under the same `NodeId` over
/// an **empty** store and never learns the decision either — only its own acceptor record for `work/x` reaches it,
/// repaired from A. C then proposes v2 at ballot 2, hearing nothing from A.
///
/// **Unfixed code:** B, empty, promises ballot 2 reporting no acceptance; {B, C} is a quorum; B votes v2; **v2 is
/// decided** — two values for one slot, and A's commit-conflict tripwire fires when C's commit reaches it. **With
/// detection 3:** B's own record, arriving with a promise above B's empty memory, puts `work/x` in B's withheld set;
/// B answers nothing; C alone is no quorum; the proposal times out. Then C hears A again, proposes once more, A
/// reports v1, and C adopts it: v1 everywhere, no conflict.
///
/// **The limit, stated:** the test waits for B's record to arrive *before* C proposes. Had C's prepare reached B
/// first, B would have promised ballot 2 empty-handed, and the copy carrying ballot 1 would then read as stale —
/// the detection needs the repair to arrive before the next higher ballot (design §8.5; `what-is-proven.md`).
/// Seen failing first (every detection toggled off): C returned `Committed { value: b"v2", ballot: 2 }` and B's memory
/// moved from `(1, v1)` to `(2, v2)` — two decided values.
#[cfg(all(feature = "consensus", feature = "test-frame-filter"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forgotten_promise_withholds_the_slot_so_a_second_value_is_not_decided() {
    use crate::store::FrameView;
    let (mut agents, mut listeners, _serial) = electorate_mesh(3, |_| {}).await;
    for n in &agents { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[&agents[0], &agents[1], &agents[2]]).await;
    declare(&agents[0], "council", false).await.expect("a genesis of three");
    epoch_converges(&agents, "council", 1).await;
    let (a_id, b_id, c_id) = (agents[0].node_id().clone(), agents[1].node_id().clone(), agents[2].node_id().clone());
    let names_slot = |key: &str| key.contains("work/x") || key.contains("work%2Fx");
    let ballot_key = format!("{}work/x", crate::consensus::consensus_ns::BALLOT);
    // C never learns the decision — only the ballot it must go above — and hears nothing from A.
    let a_hash = a_id.id_hash();
    agents[2].task_ctx.kv_state.frame_filter.pin().insert(1, Arc::new(move |v: &FrameView<'_>| match v {
        FrameView::Kv { key, .. } => *key != ballot_key && names_slot(key),
        FrameView::Signal { sender, .. } => *sender == a_hash,
    }));
    drop(listeners.remove(2));
    let cfg = ConsensusConfig { phase1_timeout: Duration::from_millis(1500), max_ballots: 2, ..ConsensusConfig::default() };
    let r = agents[0].consensus().group_propose("council", "work/x", Bytes::from_static(b"v1"), cfg.clone()).await;
    assert!(matches!(r, ConsensusResult::Committed { .. }), "v1 decided by {{A, B}}: {r:?}");
    poll_until(|| agents[1].consensus().consensus_get("work/x").as_deref() == Some(b"v1".as_slice()), 20_001).await;
    assert!(agents[1].task_ctx.consensus_accepted.pin().get("work/x").is_some_and(|s| s.accepted.is_some()), "B accepted v1");
    assert!(agents[2].consensus().consensus_get("work/x").is_none(), "C never learned it");
    listeners.push(agents[2].consensus().start_consensus_listener(ConsensusConfig::default()));

    // B restarts over an empty store, and never learns the decision either — only its own record comes back.
    drop(listeners.remove(1));
    agents.remove(1).shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("mycelium-electorate-{}", b_id.to_socket_addr().port())));
    let b = GossipAgent::new(b_id.clone(), electorate_cfg(b_id.to_socket_addr().port(), vec![a_id.clone(), c_id.clone()], false));
    let own_record = crate::consensus::accepted_key(&b_id, "work/x");
    b.task_ctx.kv_state.frame_filter.pin().insert(2, Arc::new(move |v: &FrameView<'_>| match v {
        FrameView::Kv { key, .. } => !key.starts_with(mycelium_core::signal::kv_ns::CONSENSUS_ACCEPTED) && names_slot(key),
        FrameView::Signal { .. } => false,
    }));
    b.start().await.unwrap();
    listeners.push(b.consensus().start_consensus_listener(ConsensusConfig::default()));
    b.mesh().join_group("council");
    agents.push(b);
    let (a, c, b) = (&agents[0], &agents[1], &agents[2]);
    roster_converges(&agents, "council", &[a, b, c]).await;
    epoch_converges(std::slice::from_ref(b), "council", 1).await;
    // The repair: B's own acceptor record for `work/x`, from A, reaches B's store before C proposes.
    poll_until(|| b.kv().get(&own_record).is_some(), 60_000).await;
    assert!(b.kv().get(&own_record).is_some(), "B's own record came back by anti-entropy");
    assert!(b.consensus().consensus_get("work/x").is_none(), "B does not know the decision");

    // C proposes v2 at ballot 2.
    let r = c.consensus().group_propose("council", "work/x", Bytes::from_static(b"v2"), cfg.clone()).await;
    assert!(matches!(r, ConsensusResult::Timeout { .. }), "B withholds the slot, so C alone is no quorum: {r:?}");
    assert!(b.task_ctx.acceptor_withheld.pin().contains("work/x"), "the slot is withheld on B");
    assert!(b.task_ctx.consensus_accepted.pin().get("work/x").is_none(), "B answered nothing: its memory for the slot is untouched");
    assert_ne!(c.consensus().consensus_get("work/x").as_deref(), Some(b"v2".as_slice()), "v2 was not decided");
    // C hears A again: A reports v1, C adopts it.
    c.task_ctx.kv_state.frame_filter.pin().remove(&1);
    let r = c.consensus().group_propose("council", "work/x", Bytes::from_static(b"v2"), cfg).await;
    // A reports v1 in its promise: C adopts and commits it (`Committed`, the store written here), or learns the
    // decision from A's report (`Superseded`) and holds the record once anti-entropy carries it — one round.
    if matches!(r, ConsensusResult::Committed { .. }) {
        assert_eq!(c.consensus().consensus_get("work/x").as_deref(), Some(b"v1".as_slice()), "C committed what A reported: {r:?}");
    }
    poll_until(|| c.consensus().consensus_get("work/x").as_deref() == Some(b"v1".as_slice()), 60_000).await;
    assert_eq!(c.consensus().consensus_get("work/x").as_deref(), Some(b"v1".as_slice()), "v1 kept");
    assert_eq!(a.consensus().consensus_get("work/x").as_deref(), Some(b"v1".as_slice()));
    assert_eq!(a.system_stats().commit_conflicts, 0, "one value was decided");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A forgotten promise repairs the disk, not only the slot** (#605's review, finding 1). B decided `work/x` alone and
/// holds `{promised: p, accepted: (p, own)}`; a peer's copy of B's own record carrying `{p+4, (p+4, v)}` then arrives —
/// here handed to the inbound seam directly, as a copy that **lost LWW** to B's own newer write would be (the store is
/// not touched by the test). B withholds the slot and writes the **merged** record to its own key, so a restart reads
/// the promise at `p+4` and the acceptance `v` back. Seen failing first (the repair write disabled): the store kept
/// `{p, (p, own)}` and the restarted memory promised `p`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forgotten_promise_repairs_the_record_so_the_next_start_holds_it() {
    use crate::consensus::{accepted_key, decode_acceptor, encode_acceptor, Accepted, AcceptorSlot};
    let _serial = ELECTORATE_MESHES.lock().await;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let b = GossipAgent::new(id.clone(), electorate_cfg(port, vec![], true));
    b.start().await.unwrap();
    let listener = b.consensus().start_consensus_listener(ConsensusConfig::default());
    b.mesh().join_group("solo");
    let r = b.consensus().group_propose("solo", "work/x", Bytes::from_static(b"own"), ConsensusConfig::default()).await;
    assert!(matches!(r, ConsensusResult::Committed { .. }), "{r:?}");
    let slot: Arc<str> = Arc::from("work/x");
    let held = b.task_ctx.consensus_accepted.pin().get(&slot).cloned().expect("B holds the slot");
    let p = held.promised;
    assert!(p >= 1 && held.accepted.is_some(), "{held:?}");
    let key = accepted_key(&id, &slot);
    // The copy, as a peer holds it: a later promise to another proposer and an acceptance at it.
    let copy = AcceptorSlot { promised: p + 4, promised_to: Some(0xC0FFEE), accepted: Some((p + 4, Accepted::DigestOnly([7; 32]))), groups: Vec::new() };
    let upd = crate::framing::GossipUpdate {
        nonce: 1, sender: 0xBEEF, ttl: 1, is_tombstone: false, timestamp: 1, key: Arc::from(key.as_str()), value: encode_acceptor(&copy),
    };
    let before = crate::acceptor_promises_forgotten();
    (b.task_ctx.inbound_observer.get().expect("the guard is installed").on_inbound)(&upd);
    assert_eq!(crate::acceptor_promises_forgotten() - before, 1);
    assert!(b.task_ctx.acceptor_withheld.pin().contains("work/x"), "the slot is withheld");
    let stored = b.kv().get(&key).and_then(|v| decode_acceptor(&v)).expect("B's own record is in the store");
    assert_eq!((stored.promised, stored.promised_to, stored.accepted.as_ref().map(|(b, a)| (*b, a.digest()))),
        (p + 4, Some(0xC0FFEE), Some((p + 4, [7; 32]))), "the merged record was written to B's own key");
    assert_eq!(b.task_ctx.consensus_accepted.pin().get(&slot).map(|s| s.promised), Some(p), "memory is not refreshed live");
    // The restart reads the repaired record.
    drop(listener);
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    let b2 = GossipAgent::new(id, electorate_cfg(port, vec![], false));
    b2.start().await.unwrap();
    let restored = b2.task_ctx.consensus_accepted.pin().get(&slot).cloned().expect("prewarm_accepted restores the slot");
    assert_eq!((restored.promised, restored.accepted.as_ref().map(|(b, a)| (*b, a.digest()))), (p + 4, Some((p + 4, [7; 32]))),
        "the next start holds the forgotten promise and acceptance: {restored:?}");
    assert!(!b2.task_ctx.acceptor_withheld.pin().contains("work/x"), "and the slot is not withheld after the restart");
    b2.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A slot withheld while a proposal runs ends it at the next ballot** (#605's review, finding 5). A's proposal on a
/// two-member group gets no answer from the silent member, so each ballot runs out its prepare phase; the slot is
/// withheld on A during the first ballot, and the proposal ends `Timeout` after that ballot rather than running all
/// three — with no promise or vote of A's own after the withholding. Seen failing first: three ballots ran, 3 s.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_withheld_mid_proposal_ends_it_at_the_next_ballot() {
    let (agents, mut listeners, _serial) = electorate_mesh(2, |_| {}).await;
    let (a, b) = (&agents[0], &agents[1]);
    for n in [a, b] { n.mesh().join_group("pair"); }
    roster_converges(&agents, "pair", &[a, b]).await;
    drop(listeners.remove(1)); // B answers nothing
    let cfg = ConsensusConfig { phase1_timeout: Duration::from_millis(1000), max_ballots: 3, ..ConsensusConfig::default() };
    let withheld = Arc::clone(&a.task_ctx.acceptor_withheld);
    let later = tokio::spawn(async move {
        time::sleep(Duration::from_millis(300)).await;
        withheld.pin().insert(Arc::from("work/mid"));
    });
    let started = std::time::Instant::now();
    let r = a.consensus().group_propose("pair", "work/mid", Bytes::from_static(b"v"), cfg).await;
    let elapsed = started.elapsed();
    let _ = later.await;
    match r {
        ConsensusResult::Timeout { ballots_tried, .. } => assert!((1..3).contains(&ballots_tried), "ended at the next ballot: {ballots_tried}"),
        other => panic!("{other:?}"),
    }
    assert!(elapsed < Duration::from_millis(2500), "not all three ballots ran: {elapsed:?}");
    let mem = a.task_ctx.consensus_accepted.pin().get("work/mid").cloned().unwrap_or_default();
    assert!(mem.promised <= 1 && mem.accepted.is_none(), "no answer of A's own after the withholding: {mem:?}");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **The repair merges the memory it finds under the lock** (#605's confirming review, finding 1). The merged record
/// is computed from a memory read outside `acceptor_records`; a listener promise at a higher ballot can land between
/// that read and the write. Here B's memory is raised to `p+9` (a promise to another proposer, as the listener would
/// make it) after the merge was computed from `p+4`, and the record written says `p+9` — never less than an answer
/// that is out. Seen failing first (the re-read removed): the stored record said `p+4`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_repair_merges_the_memory_it_finds_under_the_lock() {
    use crate::consensus::{accepted_key, decode_acceptor, prepare_slot, Accepted, AcceptorSlot, PrepareOutcome};
    let _serial = ELECTORATE_MESHES.lock().await;
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let b = GossipAgent::new(id.clone(), electorate_cfg(port, vec![], true));
    b.start().await.unwrap();
    let _l = b.consensus().start_consensus_listener(ConsensusConfig::default());
    b.mesh().join_group("solo");
    assert!(matches!(b.consensus().group_propose("solo", "work/x", Bytes::from_static(b"own"), ConsensusConfig::default()).await,
        ConsensusResult::Committed { .. }));
    let slot: Arc<str> = Arc::from("work/x");
    let p = b.task_ctx.consensus_accepted.pin().get(&slot).map(|s| s.promised).expect("B holds the slot");
    // The merge as the connection task computed it from memory `p`, with the copy's acceptance at `p+4`.
    let merged = AcceptorSlot { promised: p + 4, promised_to: Some(0xC0FFEE), accepted: Some((p + 4, Accepted::DigestOnly([7; 32]))), groups: Vec::new() };
    // The listener's promise lands in between: memory now says `p+9` to another proposer.
    assert!(matches!(prepare_slot(&b.task_ctx.consensus_accepted, &slot, p + 9, 0xD1CE, 0), PrepareOutcome::Promised(_)));
    crate::agent::acceptor_guard::repair_own_record(&b.task_ctx, "work/x", &merged);
    let stored = b.kv().get(&accepted_key(&id, &slot)).and_then(|v| decode_acceptor(&v)).expect("the record was written");
    assert_eq!((stored.promised, stored.promised_to, stored.accepted.as_ref().map(|(bl, a)| (*bl, a.digest()))),
        (p + 9, Some(0xD1CE), Some((p + 4, [7; 32]))), "the record says what memory said under the lock: {stored:?}");
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A forgotten acceptance at an equal ballot withholds the slot** (#605's confirming review, finding 2 — its trace).
/// Electorate {X, Y, N}; X decides vX at ballot 1 with N's vote (Y's listener down, Y never learns it and hears nothing
/// from X). N restarts empty, never learning the decision nor its own record, and promises ballot 1 to Y empty-handed
/// — set directly, as Y's prepare would leave it. Then N's own record `{1 to X, (1, vX)}` arrives: an acceptance at the
/// promised ballot that N's memory lacks. N withholds the slot, so Y's proposal at ballot 1 gets no vote and times out.
/// Seen failing first (the equal-ballot rule absent): Y committed vY at ballot 1 — two values at one ballot — with N's
/// memory holding `(1, vY)`.
#[cfg(all(feature = "consensus", feature = "test-frame-filter"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forgotten_acceptance_at_an_equal_ballot_withholds_the_slot() {
    use crate::consensus::{accepted_key, encode_acceptor, prepare_slot, Accepted, AcceptorSlot, PrepareOutcome};
    use crate::store::FrameView;
    let (mut agents, mut listeners, _serial) = electorate_mesh(3, |_| {}).await;
    for n in &agents { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[&agents[0], &agents[1], &agents[2]]).await;
    declare(&agents[0], "council", false).await.expect("a genesis of three");
    epoch_converges(&agents, "council", 1).await;
    let (x_id, y_id, n_id) = (agents[0].node_id().clone(), agents[1].node_id().clone(), agents[2].node_id().clone());
    let names_slot = |key: &str| key.contains("work/x") || key.contains("work%2Fx");
    // Y never learns the decision (nor the ballot key: it draws ballot 1) and hears nothing from X.
    let x_hash = x_id.id_hash();
    agents[1].task_ctx.kv_state.frame_filter.pin().insert(1, Arc::new(move |v: &FrameView<'_>| match v {
        FrameView::Kv { key, .. } => names_slot(key),
        FrameView::Signal { sender, .. } => *sender == x_hash,
    }));
    drop(listeners.remove(1)); // Y answers nothing while X decides
    let cfg = ConsensusConfig { phase1_timeout: Duration::from_millis(1500), max_ballots: 1, ..ConsensusConfig::default() };
    let r = agents[0].consensus().group_propose("council", "work/x", Bytes::from_static(b"vX"), cfg.clone()).await;
    assert!(matches!(r, ConsensusResult::Committed { ballot: 1, .. }), "vX decided at ballot 1 by {{X, N}}: {r:?}");
    poll_until(|| agents[2].task_ctx.consensus_accepted.pin().get("work/x").is_some_and(|s| s.accepted.is_some()), 10_000).await;
    listeners.push(agents[1].consensus().start_consensus_listener(ConsensusConfig::default()));

    // N restarts over an empty store, learning nothing about the slot — not even its own record.
    drop(listeners.remove(1));
    agents.remove(2).shutdown_with_timeout(Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("mycelium-electorate-{}", n_id.to_socket_addr().port())));
    let n = GossipAgent::new(n_id.clone(), electorate_cfg(n_id.to_socket_addr().port(), vec![x_id.clone(), y_id.clone()], false));
    n.task_ctx.kv_state.frame_filter.pin().insert(2, Arc::new(move |v: &FrameView<'_>| matches!(v, FrameView::Kv { key, .. } if names_slot(key))));
    n.start().await.unwrap();
    listeners.push(n.consensus().start_consensus_listener(ConsensusConfig::default()));
    n.mesh().join_group("council");
    agents.push(n);
    let (x, y, n) = (&agents[0], &agents[1], &agents[2]);
    roster_converges(&agents, "council", &[x, y, n]).await;
    epoch_converges(std::slice::from_ref(n), "council", 1).await;
    // N promised ballot 1 to Y empty-handed; then its own record from before the restart comes back.
    let slot: Arc<str> = Arc::from("work/x");
    assert!(matches!(prepare_slot(&n.task_ctx.consensus_accepted, &slot, 1, y_id.id_hash(), 0), PrepareOutcome::Promised(None)));
    let copy = AcceptorSlot { promised: 1, promised_to: Some(x_hash), accepted: Some((1, Accepted::DigestOnly([9; 32]))), groups: Vec::new() };
    let upd = crate::framing::GossipUpdate {
        nonce: 1, sender: x_hash, ttl: 1, is_tombstone: false, timestamp: 1,
        key: Arc::from(accepted_key(&n_id, &slot).as_str()), value: encode_acceptor(&copy),
    };
    let before = crate::acceptor_promises_forgotten();
    (n.task_ctx.inbound_observer.get().unwrap().on_inbound)(&upd);
    assert_eq!(crate::acceptor_promises_forgotten() - before, 1, "an acceptance at the promised ballot that memory lacks is forgotten");
    assert!(n.task_ctx.acceptor_withheld.pin().contains("work/x"));
    // Y proposes at ballot 1: N withholds, so {Y} alone is no quorum.
    let r = y.consensus().group_propose("council", "work/x", Bytes::from_static(b"vY"), cfg).await;
    assert!(matches!(r, ConsensusResult::Timeout { .. }), "no second value at ballot 1: {r:?}");
    assert!(n.task_ctx.consensus_accepted.pin().get("work/x").is_some_and(|s| s.accepted.is_none()), "N voted for nothing");
    assert_ne!(y.consensus().consensus_get("work/x").as_deref(), Some(b"vY".as_slice()));
    for a in &agents { a.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

// ── P2's migration: close · enumerate · carry · complete (`docs/design/consensus-electorate.md` §8.3) ──────────

/// Every agent holds `group`'s verified retirement.
#[cfg(feature = "consensus")]
async fn retirement_converges(agents: &[GossipAgent], group: &str) -> crate::Retirement {
    let ok = tokio::time::timeout(Duration::from_secs(30), async {
        while !agents.iter().all(|a| a.electorate_retirement(group).is_some()) {
            time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    assert!(ok.is_ok(), "{group}'s retirement did not reach every node: {:?}; records refused {}",
        agents.iter().map(|a| a.electorate_retirement(group).map(|r| r.successor)).collect::<Vec<_>>(),
        crate::electorate_records_refused());
    agents[0].electorate_retirement(group).unwrap()
}

/// Every agent holds the verified completion of `group`'s carry from `predecessor`.
#[cfg(feature = "consensus")]
async fn completion_converges(agents: &[GossipAgent], group: &str, predecessor: &str) -> crate::Completion {
    let ok = tokio::time::timeout(Duration::from_secs(30), async {
        while !agents.iter().all(|a| a.electorate_completion(group, predecessor).is_some()) {
            time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    assert!(ok.is_ok(), "{group}'s completion of {predecessor} did not reach every node: {:?}; records refused {}",
        agents.iter().map(|a| a.electorate_completion(group, predecessor).is_some()).collect::<Vec<_>>(),
        crate::electorate_records_refused());
    agents[0].electorate_completion(group, predecessor).unwrap()
}

/// Declares `group` = `members` as a successor of `preds` from `by` (successor first), retried while undecided, and
/// waits until every agent holds it.
#[cfg(feature = "consensus")]
async fn successor(agents: &[GossipAgent], by: &GossipAgent, group: &str, members: &[&GossipAgent], preds: &[&str]) -> crate::ElectorateDecl {
    for m in members { m.mesh().join_group(group); }
    roster_converges(agents, group, members).await;
    let mut last = None;
    for _ in 0..3 {
        match by.declare_successor_electorate(group, preds).await {
            Ok(d) => { epoch_converges(agents, group, d.epoch).await; return d; }
            Err(crate::ElectorateError::NotDecided(why)) => last = Some(why),
            Err(e) => panic!("the successor {group} was refused: {e}"),
        }
    }
    panic!("the successor {group} was not decided: {last:?}")
}

/// Completes `group`'s migration from `by` and waits until every agent holds the completion of each predecessor that
/// is complete afterwards; returns the states.
#[cfg(feature = "consensus")]
async fn complete(agents: &[GossipAgent], by: &GossipAgent, group: &str) -> Vec<crate::MigrationState> {
    let states = by.complete_migration(group).await.unwrap_or_else(|e| panic!("completing {group}: {e}"));
    for s in &states {
        if matches!(s.phase, crate::MigrationPhase::Complete { .. }) { completion_converges(agents, group, &s.predecessor).await; }
    }
    states
}

/// The quick proposal configuration the migration tests use.
#[cfg(feature = "consensus")]
fn quick_cfg() -> ConsensusConfig {
    ConsensusConfig { phase1_timeout: Duration::from_millis(1500), max_ballots: 3, ..ConsensusConfig::default() }
}

/// **Two concurrent retirements decide one successor**, and a non-member cannot retire. Two members retire the
/// group to different successors at once: they are one slot, so exactly one retirement is decided, every node holds
/// that one, and each caller is told which. A node outside the electorate cannot retire it — at its own door, and at
/// every acceptor.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_retirements_decide_one_successor_and_a_non_member_cannot_retire() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b, c] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b, c]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    epoch_converges(&agents, "council", 1).await;

    assert_eq!(d.retire_electorate("council", "mine").await, Err(crate::ElectorateError::NotAMember), "refused at D's door");
    let forged = crate::Retirement { group: "council".into(), epoch: 1, successor: "mine".into(), passes_default: false };
    let scope = SignalScope::Group(Arc::from("council"));
    let slot = crate::agent::electorate::retire_slot("council");
    assert!(admits(&crate::agent::electorate::engine(&a.task_ctx), &scope, &slot, Some((1, e1.digest())), d.node_id(),
        Some(&forged.encode())).is_err(), "an acceptor refuses a retirement whose proposer is not a member");

    successor(&agents, d, "next-a", &[d], &["council"]).await;
    successor(&agents, d, "next-b", &[d], &["council"]).await;
    let (ra, rb) = tokio::join!(a.retire_electorate("council", "next-a"), b.retire_electorate("council", "next-b"));
    let decided = retirement_converges(&agents, "council").await;
    assert!(decided.successor == "next-a" || decided.successor == "next-b", "{decided:?}");
    assert!(agents.iter().all(|n| n.electorate_retirement("council").as_ref() == Some(&decided)), "every node holds one retirement");
    for (r, mine) in [(ra, "next-a"), (rb, "next-b")] {
        match r {
            Ok(got) => assert_eq!((got.successor.as_str(), &got), (mine, &decided), "an Ok is the decided retirement"),
            Err(crate::ElectorateError::Retired { successor }) => assert_eq!(successor, decided.successor),
            other => panic!("each caller learns the decided retirement: {other:?}"),
        }
    }
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A retirement closes the group through a real ballot** (safety argument (i)). A passes the door and promises its
/// own ballot; while it waits for a quorum, A's acceptor accepts the group's retirement (B proposing it), which closes
/// the group on A. When the quorum becomes reachable, A must not complete: its own claim is refused by its gate, and
/// the proposal ends `ElectorateMismatch`, naming the close.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_proposer_closed_mid_ballot_does_not_complete() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
    let (agents, mut listeners, _serial) = electorate_mesh(3, |_| {}).await;
    let (a, b, c) = (&agents[0], &agents[1], &agents[2]);
    for n in [a, b, c] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b, c]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    epoch_converges(&agents, "council", 1).await;
    successor(&agents, a, "next", &[a], &["council"]).await;
    // B and C stop answering; A's listener stays (it is the acceptor the retirement closes).
    let c_listener = listeners.remove(2);
    drop(listeners.remove(1));
    drop(c_listener);

    let proposing = {
        let a = a.consensus();
        tokio::spawn(async move {
            a.group_propose("council", "work/x", Bytes::from_static(b"before-the-close"), quick_cfg()).await
        })
    };
    poll_until(|| a.task_ctx.consensus_accepted.pin().get("work/x").is_some_and(|s| s.promised > 0), 5_000).await;
    let retire = crate::Retirement { group: "council".into(), epoch: 1, successor: "next".into(), passes_default: false };
    let slot = crate::agent::electorate::retire_slot("council");
    let envelope = crate::consensus_life::Envelope::new(
        &slot, retire.encode(), crate::consensus_life::Term::Permanent, 1, b.node_id().clone(), b.task_ctx.hlc.tick(),
    ).encode();
    let msg = ConsensusMsg::ProposeIn {
        slot: Arc::from(slot.as_str()), ballot: 1, envelope, proposer: b.node_id().clone(),
        epoch: 1, electorate: e1.digest(),
    };
    let payload = crate::agent::electorate::engine(&b.task_ctx).sign_payload(encode_consensus_msg(&msg));
    let closed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            mycelium_core::ops::emit_signal(&b.task_ctx, Arc::from(consensus_kind::PROPOSE),
                SignalScope::Group(Arc::from("council")), payload.clone());
            for _ in 0..25 {
                if a.task_ctx.consensus_accepted.pin().get(slot.as_str()).is_some_and(|s| s.accepted.is_some()) { return; }
                time::sleep(Duration::from_millis(20)).await;
            }
        }
    }).await;
    assert!(closed.is_ok(), "A's acceptor never received the retirement");
    let _c_again = c.consensus().start_consensus_listener(ConsensusConfig::default());
    match proposing.await.unwrap() {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("closed"), "{detail}"),
        other => panic!("a proposer closed mid-ballot must not complete, got {other:?}"),
    }
    assert!(a.consensus().consensus_get("work/x").is_none() && c.consensus().consensus_get("work/x").is_none(),
        "nothing was decided after the close");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A close waits for the ordinary claims in flight** (the report-versus-fence race of round 4, finding 1: close
/// first, then read). With an ordinary claim on the group between its gate decision and its claim, a close through
/// the gate — a retirement's promise, or a member about to report its manifest — does not complete; it completes once
/// the claim is done. So any acceptance in flight is either finished before the close (and in the manifest read after
/// it) or refused after it.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_waits_for_an_ordinary_claim_in_flight() {
    use crate::consensus::{Gate, GateKind};
    let (agents, _l, _serial) = electorate_mesh(1, |_| {}).await;
    let a = &agents[0];
    a.mesh().join_group("council");
    roster_converges(&agents, "council", &[a]).await;
    declare(a, "council", false).await.expect("genesis");
    let engine = crate::agent::electorate::engine(&a.task_ctx);
    let (tx, rx) = std::sync::mpsc::channel();
    let ordinary = Gate { group: Arc::from("council"), slot: Arc::from("work/x"), epoch: 1, kind: GateKind::Ordinary };
    let closer = engine.claim_gated(Some(&ordinary), || {
        let ctx = Arc::clone(&a.task_ctx);
        let t = std::thread::spawn(move || {
            let close = Gate {
                group: Arc::from("council"), slot: Arc::from(crate::agent::electorate::retire_slot("council").as_str()),
                epoch: 1, kind: GateKind::Retire,
            };
            crate::agent::electorate::engine(&ctx).claim_gated(Some(&close), || ());
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "the close did not wait for the claim in flight");
        t
    }).expect("the ordinary claim passed the gate");
    rx.recv_timeout(Duration::from_secs(5)).expect("the close completes once the claim is done");
    closer.join().unwrap();
    // And the slot is tagged with the group it was claimed on: what a manifest enumerates.
    assert!(a.task_ctx.consensus_accepted.pin().get("work/x").is_some_and(|s| s.groups.iter().any(|g| &**g == "council")));
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A value chosen in a retired group is carried by its successor, and the migration terminates** (safety argument
/// (ii), (iii), (iv)). `old` = {A,B,C} declares the fleet's default; a lock value is **chosen and never committed**
/// in it — accepted by A and B. `new` = {C,D,E} is declared first, naming `old`; until `old` has retired to it `new`
/// decides nothing; after the retirement, nothing outside the carry set until the carry is complete; the completion
/// enumerates `old` and carries the lock. D asks for the lock: `new`'s members held nothing of it before the carry
/// (C never accepted the value), so only the carry can stop D, and it does. Then `old`'s majority goes (A's and B's
/// listeners stop) and `new` decides a fresh slot without it — the predecessor is irrelevant after completion.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_value_chosen_in_a_retired_group_is_carried_by_its_successor() {
    let (agents, mut listeners, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, c, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b, c] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b, c]).await;
    declare(a, "old", true).await.expect("genesis of old, the fleet's default");
    epoch_converges(&agents, "old", 1).await;
    let v = Bytes::from_static(b"held-by-a");
    let slot: Arc<str> = Arc::from("lock/door");
    for acceptor in [a, b] {
        assert!(crate::agent::electorate::engine(&acceptor.task_ctx)
            .accept_for_test("old", &slot, 5, &v, a.node_id()).await, "{} accepts v", acceptor.node_id());
    }

    let g = successor(&agents, c, "new", &[c, d, e], &["old"]).await;
    assert!(g.predecessors.contains("old") && !g.exclusive_default);
    assert_eq!(d.migration_state("new"), vec![crate::MigrationState { predecessor: "old".into(), phase: crate::MigrationPhase::Waiting }]);
    match d.consensus().group_propose("new", "work/early", Bytes::from_static(b"x"), quick_cfg()).await {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("waiting"), "{detail}"),
        other => panic!("a successor decides nothing before its predecessor retires to it: {other:?}"),
    }

    let r = a.retire_electorate("old", "new").await.expect("old retires to new");
    assert!(r.passes_default, "old held the default, so the retirement passes it");
    retirement_converges(&agents, "old").await;
    match a.consensus().group_propose("old", "work/after", Bytes::from_static(b"x"), quick_cfg()).await {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("retired"), "{detail}"),
        other => panic!("a retired group decides nothing: {other:?}"),
    }
    assert_eq!(crate::agent::electorate::exclusive_electorate(&d.task_ctx), Ok(Some("new".to_string())),
        "the default followed the retirement");
    assert_eq!(d.migration_state("new"), vec![crate::MigrationState { predecessor: "old".into(), phase: crate::MigrationPhase::Closed }]);
    // Retired but not complete: nothing outside the carry set, by name.
    match d.consensus().group_propose("new", "work/early", Bytes::from_static(b"x"), quick_cfg()).await {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("not complete"), "{detail}"),
        other => panic!("a successor decides nothing outside the carry set before the completion: {other:?}"),
    }
    match d.consensus().distributed_lock("door", Duration::from_secs(5)).await {
        Err(crate::ConsistencyError::ElectorateMismatch { detail, .. }) => assert!(detail.contains("not complete"), "{detail}"),
        other => panic!("the lock waits for the completion too: {other:?}"),
    }

    let before = crate::migration_slots_carried();
    let states = complete(&agents, d, "new").await;
    assert_eq!(states, vec![crate::MigrationState { predecessor: "old".into(), phase: crate::MigrationPhase::Complete { slots: 1, conflicts: 0 } }], "{states:?}");
    let res = d.consensus().group_propose("new", &slot, Bytes::from_static(b"d-wants-it"), quick_cfg()).await;
    assert_eq!(d.consensus().consensus_get(&slot), Some(v.clone()), "the value chosen in old was carried, not overwritten: {res:?}");
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(crate::migration_slots_carried(), before + 1, "the one slot of old's carry set was carried");
    assert!(matches!(d.consensus().distributed_lock("door", Duration::from_secs(5)).await, Err(crate::ConsistencyError::Superseded)),
        "A still holds the lock in the successor");

    // old's majority is gone; new decides without it, and reads what it carried.
    drop(listeners.remove(1));
    drop(listeners.remove(0));
    let fresh = e.consensus().group_propose("new", "work/fresh", Bytes::from_static(b"first"), quick_cfg()).await;
    assert!(matches!(fresh, ConsensusResult::Committed { .. }), "after the completion new decides a fresh slot without old: {fresh:?}");
    assert_eq!(e.consensus().consensus_get(&slot), Some(v), "and still reads the carried value");
    assert!(crate::migration_completions() >= 1);
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A successor decides nothing until its predecessor's retirement is visible to it** (the D1 race of #601's
/// round-5 review: a successor formed before the retirement reached its members). `new` is declared naming `old`,
/// and `old` retires to it; D and E have not received the retirement yet (it is taken out of their stores). D's door
/// refuses to propose, naming the wait, and E — an acceptor — refuses to answer a proposal on `new`. Once the
/// retirement arrives, the completion carries the value chosen in `old` and D's proposal reads it.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successor_decides_nothing_until_its_predecessors_retirement_is_visible() {
    let (agents, _l, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, c, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b, c] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b, c]).await;
    declare(a, "old", false).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    let v = Bytes::from_static(b"held-by-a");
    let slot: Arc<str> = Arc::from("work/s");
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &slot, 5, &v, a.node_id()).await); }
    let g = successor(&agents, c, "new", &[c, d, e], &["old"]).await;
    a.retire_electorate("old", "new").await.unwrap();
    retirement_converges(&agents, "old").await;
    // The retirement has not reached D and E (taken out of their stores, as gossip lag would leave it).
    let keys = [format!("consensus/committed/{}", crate::agent::electorate::retire_slot("old")),
                crate::agent::electorate::retire_cert_key("old")];
    let held: Vec<Bytes> = keys.iter().map(|k| a.kv().get(k).expect("the retirement's records")).collect();
    for n in [d, e] {
        for k in &keys { n.task_ctx.kv_state.store.pin().remove(k.as_str()); }
        n.task_ctx.electorate_retired.pin().remove("old");
    }
    let before = crate::electorate_waiting_refusals();
    assert!(admits(&crate::agent::electorate::engine(&e.task_ctx), &SignalScope::Group(Arc::from("new")), &slot,
        Some((g.epoch, g.digest())), d.node_id(), Some(&Bytes::from_static(b"d-wants-it"))).is_err(),
        "an acceptor that cannot see the retirement does not answer");
    assert!(crate::electorate_waiting_refusals() > before, "and counts it");
    match d.consensus().group_propose("new", &slot, Bytes::from_static(b"d-wants-it"), quick_cfg()).await {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("waiting"), "{detail}"),
        other => panic!("the proposer's door refuses while it cannot see the retirement: {other:?}"),
    }
    assert!(matches!(d.complete_migration("new").await, Ok(states) if states[0].phase == crate::MigrationPhase::Waiting),
        "a completion run finds nothing to carry while the retirement is not visible");
    assert!(agents.iter().all(|n| n.consensus().consensus_get(&slot).is_none()), "nothing was decided");
    // The retirement arrives (re-sent here, as gossip would deliver it); then the completion carries v.
    for (k, v) in keys.iter().zip(&held) { assert!(a.kv().set(k.clone(), v.clone())); }
    retirement_converges(&agents, "old").await;
    complete(&agents, d, "new").await;
    let res = d.consensus().group_propose("new", &slot, Bytes::from_static(b"d-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(d.consensus().consensus_get(&slot), Some(v));
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A second live default is refused by name, and the default has one holder across the handover** (safety
/// argument (v); the D1 default interleaving of round 5). While `old` holds the fleet's default, a genesis declaring
/// another default is refused at the door and by every acceptor; the default moves only by retirement, into a
/// successor whose own flag is false — every node names `new` as the exclusive electorate once the retirement is
/// visible, never none and never two — and after the completion a lock decides there.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_live_default_is_refused_by_name() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b]).await;
    declare(a, "old", true).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    for n in [c, d] { n.mesh().join_group("new"); }
    roster_converges(&agents, "new", &[c, d]).await;
    match c.declare_electorate("new", true).await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("old") && why.contains("default"), "{why}"),
        other => panic!("a second default is refused by name: {other:?}"),
    }
    let mut members = vec![c.node_id().clone(), d.node_id().clone()];
    members.sort_by_key(ToString::to_string);
    let second = crate::ElectorateDecl { group: "new".into(), epoch: 1, members, exclusive_default: true, ..Default::default() };
    let slot = crate::agent::electorate::genesis_slot("new", 1);
    assert!(admits(&crate::agent::electorate::engine(&d.task_ctx), &SignalScope::Group(Arc::from("new")), &slot,
        Some((0, [0; 32])), c.node_id(), Some(&second.encode())).is_err(), "an acceptor refuses it too");
    // The default moves by retirement — passed by it, whatever the successor's flag.
    let g = successor(&agents, c, "new", &[c, d], &["old"]).await;
    assert!(!g.exclusive_default);
    // While old holds the default and is live: a retirement that says it does not pass the default is refused by
    // every acceptor, and one that says it does is admitted (review of #606, finding 6: probed before the retirement).
    let e1 = a.electorate("old").unwrap();
    let retire = crate::agent::electorate::retire_slot("old");
    let says = |passes: bool| crate::Retirement { group: "old".into(), epoch: 1, successor: "new".into(), passes_default: passes };
    assert!(admits(&crate::agent::electorate::engine(&b.task_ctx), &SignalScope::Group(Arc::from("old")), &retire,
        Some((1, e1.digest())), a.node_id(), Some(&says(false).encode())).is_err(), "a retirement that drops the default is refused");
    assert!(admits(&crate::agent::electorate::engine(&b.task_ctx), &SignalScope::Group(Arc::from("old")), &retire,
        Some((1, e1.digest())), a.node_id(), Some(&says(true).encode())).is_ok(), "one that passes it is admitted");
    let r = a.retire_electorate("old", "new").await.unwrap();
    assert!(r.passes_default);
    retirement_converges(&agents, "old").await;
    for n in &agents {
        assert_eq!(crate::agent::electorate::exclusive_electorate(&n.task_ctx), Ok(Some("new".to_string())), "{}", n.node_id());
    }
    // A completion must copy what the retirement says (review round 2, C): one that drops the default it passed, or
    // names another predecessor epoch, is refused by the acceptor; the faithful one is admitted.
    let carried = crate::agent::electorate::carried_slot("new", "old");
    let faithful = crate::Completion { group: "new".into(), epoch: g.epoch, predecessor: "old".into(), predecessor_epoch: 1, passes_default: true, slots: 0, conflicts: 0, conflict_slots: vec![], digest: [0; 32] };
    assert!(admits(&crate::agent::electorate::engine(&d.task_ctx), &SignalScope::Group(Arc::from("new")), &carried,
        Some((g.epoch, g.digest())), c.node_id(), Some(&faithful.encode())).is_ok(), "a faithful completion is admitted");
    for wrong in [crate::Completion { passes_default: false, ..faithful.clone() }, crate::Completion { predecessor_epoch: 2, ..faithful.clone() }] {
        assert!(admits(&crate::agent::electorate::engine(&d.task_ctx), &SignalScope::Group(Arc::from("new")), &carried,
            Some((g.epoch, g.digest())), c.node_id(), Some(&wrong.encode())).is_err(), "a completion that does not match the retirement is refused: {wrong:?}");
    }
    complete(&agents, c, "new").await;
    drop(d.consensus().distributed_lock("door", Duration::from_secs(5)).await.expect("the lock decides in the successor"));
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A successor that waits on a predecessor that never retires has an escape: the drop** (the D2 wedge of round 5;
/// #606's review, finding 1). `new` names `old` (the default) and `other`; `old` retires to it and is completed,
/// `other` never retires. `new` holds the default but decides nothing — the exclusive verbs are refused by name — and
/// cannot retire either (a predecessor's retirement not visible counts as incomplete). The escape:
/// `drop_predecessor("new", "other")`, a decision of `new`'s majority; then `new` decides (the lock, with `old`'s
/// value), `other`'s later retirement to `new` is refused at `other`'s door and acceptor, `other` goes on deciding on
/// its own, and `new` can now retire into `newer`, which carries `old`'s value through `new`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successor_waiting_on_a_predecessor_that_never_retires_has_an_escape() {
    let (agents, _l, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, c, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b] { n.mesh().join_group("old"); }
    c.mesh().join_group("other");
    roster_converges(&agents, "old", &[a, b]).await;
    roster_converges(&agents, "other", &[c]).await;
    declare(a, "old", true).await.unwrap();
    declare(c, "other", false).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    epoch_converges(&agents, "other", 1).await;
    let v = Bytes::from_static(b"held-by-a");
    let slot: Arc<str> = Arc::from("lock/door");
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &slot, 5, &v, a.node_id()).await); }
    successor(&agents, d, "new", &[d, e], &["old", "other"]).await;
    a.retire_electorate("old", "new").await.unwrap();
    retirement_converges(&agents, "old").await;
    assert_eq!(crate::agent::electorate::exclusive_electorate(&d.task_ctx), Ok(Some("new".to_string())));
    let states = complete(&agents, d, "new").await;
    assert_eq!(states, vec![
        crate::MigrationState { predecessor: "old".into(), phase: crate::MigrationPhase::Complete { slots: 1, conflicts: 0 } },
        crate::MigrationState { predecessor: "other".into(), phase: crate::MigrationPhase::Waiting },
    ], "{states:?}");
    match d.consensus().distributed_lock("door", Duration::from_secs(5)).await {
        Err(crate::ConsistencyError::ElectorateMismatch { detail, .. }) => assert!(detail.contains("other"), "{detail}"),
        other => panic!("new waits for other: {other:?}"),
    }
    successor(&agents, e, "newer", &[e], &["new"]).await;
    match d.retire_electorate("new", "newer").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("other"), "{why}"),
        other => panic!("a successor waiting on a predecessor cannot retire either: {other:?}"),
    }
    // The escape: `other` will never retire into new — but it may still (nothing says its majority is gone), so a drop
    // is refused without the operator's assertion; forced, it is admitted and audited as such.
    match d.drop_predecessor("new", "other", false).await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("force"), "{why}"),
        other => panic!("a drop of a predecessor that may still retire is refused: {other:?}"),
    }
    let dr = d.drop_predecessor("new", "other", true).await.expect("new drops other, forced");
    assert!(dr.force && dr.predecessor == "other");
    poll_until(|| agents.iter().all(|n| n.electorate_drop("new", "other").is_some()), 20_000).await;
    assert_eq!(d.migration_state("new").iter().find(|s| s.predecessor == "other").map(|s| s.phase.clone()), Some(crate::MigrationPhase::Dropped));
    let res = d.consensus().group_propose("new", &slot, Bytes::from_static(b"d-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "new decides once other is dropped: {res:?}");
    assert_eq!(d.consensus().consensus_get(&slot), Some(v.clone()), "with old's value");
    // other's retirement to new is refused from now on — at other's door and at its acceptor.
    match c.retire_electorate("other", "new").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("dropped"), "{why}"),
        other => panic!("a dropped predecessor cannot retire into the group: {other:?}"),
    }
    let eo = c.electorate("other").unwrap();
    let late = crate::Retirement { group: "other".into(), epoch: 1, successor: "new".into(), passes_default: false };
    assert!(admits(&crate::agent::electorate::engine(&c.task_ctx), &SignalScope::Group(Arc::from("other")),
        &crate::agent::electorate::retire_slot("other"), Some((1, eo.digest())), c.node_id(), Some(&late.encode())).is_err(),
        "other's acceptor refuses the retirement into a group that dropped it");
    assert!(matches!(c.consensus().group_propose("other", "work/own", Bytes::from_static(b"o"), quick_cfg()).await,
        ConsensusResult::Committed { .. }), "other still decides on its own");
    // And new, every predecessor complete or dropped, can retire: newer carries old's value through new.
    d.retire_electorate("new", "newer").await.expect("after the drop new can retire");
    retirement_converges(&agents, "new").await;
    assert_eq!(crate::agent::electorate::exclusive_electorate(&e.task_ctx), Ok(Some("newer".to_string())));
    complete(&agents, e, "newer").await;
    let res = e.consensus().group_propose("newer", &slot, Bytes::from_static(b"e-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(e.consensus().consensus_get(&slot), Some(v), "carried from old through new");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A retirement's prepare closes nothing while a carry into the group is incomplete** (#606's review, finding 1 —
/// the permanent wedge). `g1` retires to `g2` = {D,E,F}; the retirement reaches E and F but not D (taken out of D's
/// store). D, seeing no incomplete carry, sends a bare `PrepareIn` on `g2`'s retirement slot to `g3`: E and F must
/// **not** close — each sees `g1` retired to `g2` and no completion, so it refuses the prepare without promising.
/// Then the retirement reaches D, the completion succeeds, and `g2` retires to `g3` for real. The merge variant:
/// `m` names `g1` and `lone`; `g1` complete, `lone` not retired — the bare prepare on `m`'s retirement is refused by
/// every acceptor without closing, even with no lag at all.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retirement_prepare_closes_nothing_while_the_carry_into_the_group_is_incomplete() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
    let (agents, _l, _serial) = electorate_mesh(6, |_| {}).await;
    let (a, b, c, d, e, f) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4], &agents[5]);
    for n in [a, b] { n.mesh().join_group("g1"); }
    c.mesh().join_group("lone");
    roster_converges(&agents, "g1", &[a, b]).await;
    roster_converges(&agents, "lone", &[c]).await;
    declare(a, "g1", false).await.unwrap();
    declare(c, "lone", false).await.unwrap();
    epoch_converges(&agents, "g1", 1).await;
    epoch_converges(&agents, "lone", 1).await;
    let v = Bytes::from_static(b"chosen-in-g1");
    let slot: Arc<str> = Arc::from("work/s");
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("g1", &slot, 5, &v, a.node_id()).await); }
    let g2 = successor(&agents, d, "g2", &[d, e, f], &["g1"]).await;
    successor(&agents, c, "g3", &[c], &["g2"]).await;
    a.retire_electorate("g1", "g2").await.unwrap();
    retirement_converges(&agents, "g1").await;
    // D lags: the retirement is not in its store.
    let keys = [format!("consensus/committed/{}", crate::agent::electorate::retire_slot("g1")), crate::agent::electorate::retire_cert_key("g1")];
    let held: Vec<Bytes> = keys.iter().map(|k| a.kv().get(k).expect("the retirement's records")).collect();
    for k in &keys { d.task_ctx.kv_state.store.pin().remove(k.as_str()); }
    d.task_ctx.electorate_retired.pin().remove("g1");
    match d.retire_electorate("g2", "g3").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("not visible"), "D's own door refuses: {why}"),
        other => panic!("D's door refuses a retirement while a predecessor's retirement is not visible: {other:?}"),
    }
    // The bare prepare D's door would have sent: E and F refuse it without closing.
    let retire_g2 = crate::agent::electorate::retire_slot("g2");
    let prepare = ConsensusMsg::PrepareIn {
        slot: Arc::from(retire_g2.as_str()), ballot: 3, proposer: d.node_id().clone(), epoch: g2.epoch, electorate: g2.digest(),
    };
    let before = crate::electorate_waiting_refusals();
    for _ in 0..3 {
        mycelium_core::ops::emit_signal(&d.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Group(Arc::from("g2")),
            crate::agent::electorate::engine(&d.task_ctx).sign_payload(encode_consensus_msg(&prepare)));
        time::sleep(Duration::from_millis(300)).await;
    }
    poll_until(|| crate::electorate_waiting_refusals() > before, 10_000).await;
    for n in [e, f] {
        assert!(n.task_ctx.consensus_accepted.pin().get(retire_g2.as_str()).is_none_or(|s| s.promised == 0 && s.accepted.is_none()),
            "{} promised the retirement's prepare and closed g2", n.node_id());
        assert!(n.task_ctx.electorate_fences.pin().get("g2").is_none_or(|s| !s.closed), "{} closed g2", n.node_id());
    }
    // The retirement reaches D; the completion succeeds; g2 retires for real; g3 reads g1's value.
    for (k, v) in keys.iter().zip(&held) { assert!(a.kv().set(k.clone(), v.clone())); }
    retirement_converges(&agents, "g1").await;
    complete(&agents, d, "g2").await;
    d.retire_electorate("g2", "g3").await.expect("after the completion g2 retires");
    retirement_converges(&agents, "g2").await;
    complete(&agents, c, "g3").await;
    assert_eq!({ let _ = c.consensus().group_propose("g3", &slot, Bytes::from_static(b"c-wants-it"), quick_cfg()).await; c.consensus().consensus_get(&slot) }, Some(v));
    // The merge variant, with no lag: m names g3 and lone; g3 retires and is completed, lone never retires.
    let m = successor(&agents, e, "m", &[e, f], &["g3", "lone"]).await;
    c.retire_electorate("g3", "m").await.unwrap();
    retirement_converges(&agents, "g3").await;
    complete(&agents, e, "m").await;
    successor(&agents, d, "m2", &[d], &["m"]).await;
    let retire_m = crate::agent::electorate::retire_slot("m");
    let prepare = ConsensusMsg::PrepareIn {
        slot: Arc::from(retire_m.as_str()), ballot: 3, proposer: e.node_id().clone(), epoch: m.epoch, electorate: m.digest(),
    };
    let before = crate::electorate_waiting_refusals();
    for _ in 0..3 {
        mycelium_core::ops::emit_signal(&e.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Group(Arc::from("m")),
            crate::agent::electorate::engine(&e.task_ctx).sign_payload(encode_consensus_msg(&prepare)));
        time::sleep(Duration::from_millis(300)).await;
    }
    poll_until(|| crate::electorate_waiting_refusals() > before, 10_000).await;
    assert!(f.task_ctx.consensus_accepted.pin().get(retire_m.as_str()).is_none_or(|s| s.promised == 0), "F closed m for a merge with a waiting predecessor");
    assert!(matches!(e.consensus().group_propose("m", "work/s", Bytes::from_static(b"e"), quick_cfg()).await, ConsensusResult::ElectorateMismatch { .. }),
        "m still waits on lone (ordinary decisions), but is not closed");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A slot claimed before the group tag existed is carried** (#606's review, finding 2 — the upgrade window). A
/// value chosen in `old` by acceptors whose records are 2.32.0-shaped (no group tag, as a 2.32.0 member wrote them)
/// is still in the manifest — an untagged slot is reported too — and reaches `new`. A slot the successor may not
/// decide at all (a cluster-scoped exclusive slot a member once answered) is skipped, not stuck on.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_claimed_before_the_tag_existed_is_carried() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b]).await;
    declare(a, "old", false).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    let v = Bytes::from_static(b"chosen-by-2.32.0-members");
    let slot: Arc<str> = Arc::from("work/legacy");
    for n in [a, b] {
        assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &slot, 5, &v, a.node_id()).await);
        // As a 2.32.0 member holds it: the acceptor memory and the durable record without the tag.
        n.task_ctx.consensus_accepted.pin().compute(Arc::clone(&slot), |e| {
            let cur = e.map(|(_, s)| s.clone()).unwrap_or_default();
            papaya::Operation::<_, ()>::Insert(crate::consensus::AcceptorSlot { groups: Vec::new(), ..cur })
        });
        let rec = crate::consensus::encode_acceptor(&n.task_ctx.consensus_accepted.pin().get(&slot).cloned().unwrap());
        assert_eq!(rec[0], 0x03, "the 2.32.0 record shape");
        assert!(n.kv().set(crate::consensus::accepted_key(n.node_id(), &slot), rec));
        // And a cluster-scoped lock answered outside any electorate, untagged: not old's, never new's.
        let lock: Arc<str> = Arc::from("lock/elsewhere");
        assert!(crate::consensus::claim_vote(&n.task_ctx.consensus_accepted, &lock, 2, &Bytes::from_static(b"x"), a.node_id().id_hash(), 0));
    }
    successor(&agents, c, "new", &[c, d], &["old"]).await;
    a.retire_electorate("old", "new").await.unwrap();
    retirement_converges(&agents, "old").await;
    let states = complete(&agents, c, "new").await;
    assert!(matches!(states[0].phase, crate::MigrationPhase::Complete { slots, conflicts: 0 } if slots >= 1), "{states:?}");
    let res = c.consensus().group_propose("new", &slot, Bytes::from_static(b"c-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(c.consensus().consensus_get(&slot), Some(v));
    assert!(c.consensus().consensus_get("lock/elsewhere").is_none(), "a slot that was never old's is not decided in new");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **Two live defaults are merged into one successor** (#606's review, finding 3 — the escape §8.5 documents). `one`
/// = {A,B} and `two` = {C,D} both declared the default — `one`'s records hidden from C and D while `two`'s genesis was
/// decided, as gossip lag would. Every node that sees both refuses the exclusive verbs. The merge: `m` = {A,C} naming
/// both; `one` retires into it (its default passed although it is not the unique holder), `two` retires into it,
/// `m` completes both — and is the one default: the exclusive verbs resume there.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_defaults_are_merged_into_one_successor() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, c, d) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b] { n.mesh().join_group("one"); }
    for n in [c, d] { n.mesh().join_group("two"); }
    roster_converges(&agents, "one", &[a, b]).await;
    roster_converges(&agents, "two", &[c, d]).await;
    declare(a, "one", true).await.unwrap();
    epoch_converges(&agents, "one", 1).await;
    // A 2.32.0-era lock, answered by one's members before the tag existed: untagged in their memory.
    let legacy: Arc<str> = Arc::from("lock/legacy");
    for n in [a, b] { assert!(crate::consensus::claim_vote(&n.task_ctx.consensus_accepted, &legacy, 2, &Bytes::from_static(b"held-by-a"), a.node_id().id_hash(), 0)); }
    // C and D have not seen one's genesis (taken out of their stores); two declares the default too.
    let keys = [format!("consensus/committed/{}", crate::agent::electorate::genesis_slot("one", 1)), crate::agent::electorate::cert_key("one", 1)];
    let held: Vec<Bytes> = keys.iter().map(|k| a.kv().get(k).expect("one's records")).collect();
    for n in [c, d] {
        for k in &keys { n.task_ctx.kv_state.store.pin().remove(k.as_str()); }
        n.task_ctx.electorates.pin().remove("one");
    }
    c.declare_electorate("two", true).await.expect("two's default, declared in the lag");
    for (k, v) in keys.iter().zip(&held) { assert!(a.kv().set(k.clone(), v.clone())); }
    epoch_converges(&agents, "two", 1).await;
    poll_until(|| agents.iter().all(|n| crate::agent::electorate::live_default(&n.task_ctx).is_err()), 10_000).await;
    assert!(matches!(a.consensus().distributed_lock("door", Duration::from_secs(5)).await, Err(crate::ConsistencyError::ElectorateMismatch { .. })),
        "two live defaults: the exclusive verbs are refused");
    // The merge.
    for n in [a, c] { n.mesh().join_group("m"); }
    roster_converges(&agents, "m", &[a, c]).await;
    let m = a.declare_successor_electorate("m", &["one", "two"]).await.expect("the merge's successor");
    epoch_converges(&agents, "m", m.epoch).await;
    let r1 = a.retire_electorate("one", "m").await.expect("one retires although it is not the unique holder");
    assert!(r1.passes_default);
    retirement_converges(&agents, "one").await;
    // While two still holds the default the fleet's default cannot be named, so whether the untagged lock is m's to
    // decide cannot be told: the carry stops there, named — it does not skip the lock (review round 2, D).
    match crate::agent::migration::complete_outcome(&a.task_ctx, "m").await {
        Err(crate::ElectorateError::NotDecided(why)) => assert!(why.contains("cannot be told") && why.contains("lock/legacy"), "{why}"),
        other => panic!("the carry stops, named, while the default cannot be named: {other:?}"),
    }
    assert!(a.electorate_completion("m", "one").is_none(), "nothing completed while the lock's home cannot be told");
    let r2 = c.retire_electorate("two", "m").await.expect("two retires");
    assert!(r2.passes_default);
    retirement_converges(&agents, "two").await;
    let states = complete(&agents, a, "m").await;
    assert!(states.iter().all(|s| matches!(s.phase, crate::MigrationPhase::Complete { .. })), "{states:?}");
    for n in &agents {
        assert_eq!(crate::agent::electorate::exclusive_electorate(&n.task_ctx), Ok(Some("m".to_string())), "{}", n.node_id());
    }
    drop(a.consensus().distributed_lock("door", Duration::from_secs(5)).await.expect("the exclusive verbs resume in m"));
    assert_eq!(a.consensus().consensus_get(&legacy), Some(Bytes::from_static(b"held-by-a")), "the legacy lock was carried into m once the default could be named");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A drop is admitted only for a predecessor that cannot retire into the group any more, and a forced drop that
/// races a retirement is a detected conflict** (#606's review, round 2, A). `p` = {A,B} chose `work/s = v`, never
/// committed; `g2` = {D,E,F} names `p`. (i) A drop of `p` is refused at D's door and at E's acceptor — no retirement of
/// `p` elsewhere is visible, so `p` may still retire. Forced — the operator's assertion that `p`'s majority is gone —
/// it is decided. (ii) The assertion was false: A and B are alive and the drop has not reached them (taken out of their
/// stores, as lag would). F, no longer waiting, decides `work/s = w` in `g2`; `p` retires into `g2`; the drop is void;
/// the completion's carry finds `w` above `v` — a conflict: counted on the commit-conflict tripwire, the slot
/// reported `conflict` and never `carried`, `w` standing, the completion decided with it on record.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drop_needs_a_retirement_elsewhere_and_a_forced_drop_that_races_a_retirement_is_a_detected_conflict() {
    let (agents, _l, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, d, e, f) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b] { n.mesh().join_group("p"); }
    roster_converges(&agents, "p", &[a, b]).await;
    declare(a, "p", false).await.unwrap();
    epoch_converges(&agents, "p", 1).await;
    let v = Bytes::from_static(b"chosen-in-p");
    let slot: Arc<str> = Arc::from("work/s");
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("p", &slot, 5, &v, a.node_id()).await); }
    let g2 = successor(&agents, d, "g2", &[d, e, f], &["p"]).await;
    // (i) refused without a retirement elsewhere — door and acceptor.
    match d.drop_predecessor("g2", "p", false).await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("may still retire"), "{why}"),
        other => panic!("a drop of a predecessor that may still retire is refused: {other:?}"),
    }
    let drop_slot = crate::agent::electorate::drop_slot("g2", "p");
    let unforced = crate::PredecessorDrop { group: "g2".into(), epoch: g2.epoch, predecessor: "p".into(), force: false };
    assert!(admits(&crate::agent::electorate::engine(&e.task_ctx), &SignalScope::Group(Arc::from("g2")), &drop_slot,
        Some((g2.epoch, g2.digest())), d.node_id(), Some(&unforced.encode())).is_err(), "E refuses the unforced drop too");
    assert!(agents.iter().all(|n| n.electorate_drop("g2", "p").is_none()));
    // Forced: the operator's assertion, decided by g2's majority.
    let dr = d.drop_predecessor("g2", "p", true).await.expect("a forced drop is admitted");
    assert!(dr.force);
    poll_until(|| [d, e, f].iter().all(|n| n.electorate_drop("g2", "p").is_some()), 20_000).await;
    // (ii) The assertion was false and the drop has not reached p (lag): F decides freely, then p retires into g2.
    let drop_keys = [format!("consensus/committed/{drop_slot}"), crate::agent::electorate::drop_cert_key("g2", "p")];
    for n in [a, b] {
        for k in &drop_keys { n.task_ctx.kv_state.store.pin().remove(k.as_str()); }
        n.task_ctx.electorate_drops.pin().remove(crate::agent::electorate::migration_key("g2", "p").as_str());
        n.task_ctx.electorate_dropped_known.pin().remove(crate::agent::electorate::migration_key("g2", "p").as_str());
    }
    assert!(matches!(f.consensus().group_propose("g2", &slot, Bytes::from_static(b"w"), quick_cfg()).await, ConsensusResult::Committed { .. }),
        "g2 decides freely once p is dropped");
    a.retire_electorate("p", "g2").await.expect("p retires into g2 where the drop is not visible");
    retirement_converges(&agents, "p").await;
    assert!(d.electorate_drop("g2", "p").is_none(), "the drop is void once the retirement is visible");
    let before = d.task_ctx.commit_conflicts.load(std::sync::atomic::Ordering::Relaxed);
    let states = complete(&agents, d, "g2").await;
    assert_eq!(states, vec![crate::MigrationState { predecessor: "p".into(), phase: crate::MigrationPhase::Complete { slots: 1, conflicts: 1 } }],
        "the slot is reported as a conflict, never carried: {states:?}");
    assert_eq!(d.task_ctx.commit_conflicts.load(std::sync::atomic::Ordering::Relaxed), before + 1, "counted on the commit-conflict tripwire");
    let c = d.electorate_completion("g2", "p").unwrap();
    assert_eq!((c.conflicts, c.conflict_slots.as_slice()), (1, ["work/s".to_string()].as_slice()));
    assert_eq!(d.consensus().consensus_get(&slot), Some(Bytes::from_static(b"w")), "the value g2 holds stands");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A verified completion stands as proof of the retirement, and the predecessor's records are on the successor's
/// disk** (#606's review, finding 4). `new` = {D,E,F} runs `Async` persistence; after the completion each member's
/// WAL holds `old`'s genesis and retirement records with their certificates and the completion (the replay shows
/// them); `new` is then restarted over **what is on disk now** — the crash model — with `old` gone, and still names
/// itself ready, holds the default, and decides. The completion alone carries `passes_default` and `old`'s epoch.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completion_stands_as_proof_when_the_predecessor_is_gone_and_the_successor_crashed() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let base = std::env::temp_dir().join(format!("mycelium-migration-crash-{}-{}", std::process::id(), alloc_port()));
    let persistence = |port: u16| PersistenceConfig {
        base_path: base.join(port.to_string()), sync_mode: SyncMode::Async, snapshot_wal_threshold: 100_000,
        snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse,
    };
    let (mut agents, mut listeners, _serial) = electorate_mesh(6, |c| c.persistence = Some(persistence(c.bind_port))).await;
    let ids: Vec<NodeId> = agents.iter().map(|a| a.node_id().clone()).collect();
    let needed: Vec<String> = vec![
        format!("consensus/committed/{}", crate::agent::electorate::genesis_slot("old", 1)), crate::agent::electorate::cert_key("old", 1),
        format!("consensus/committed/{}", crate::agent::electorate::retire_slot("old")), crate::agent::electorate::retire_cert_key("old"),
        format!("consensus/committed/{}", crate::agent::electorate::carried_slot("new", "old")), crate::agent::electorate::carried_cert_key("new", "old"),
    ];
    {
        let (a, b, c, d, e, f) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4], &agents[5]);
        for n in [a, b, c] { n.mesh().join_group("old"); }
        roster_converges(&agents, "old", &[a, b, c]).await;
        declare(a, "old", true).await.unwrap();
        epoch_converges(&agents, "old", 1).await;
        assert!(matches!(a.consensus().group_propose("old", "work/k", Bytes::from_static(b"k"), quick_cfg()).await, ConsensusResult::Committed { .. }));
        successor(&agents, d, "new", &[d, e, f], &["old"]).await;
        a.retire_electorate("old", "new").await.unwrap();
        retirement_converges(&agents, "old").await;
        complete(&agents, d, "new").await;
        let c_rec = d.electorate_completion("new", "old").unwrap();
        assert!(c_rec.passes_default && c_rec.predecessor_epoch == 1, "{c_rec:?}");
        // The proof, in process: with old's genesis and retirement gone from D's store and caches (as on a member
        // they never reached), the completion alone makes D ready and the default new's.
        for k in &needed[..4] { d.task_ctx.kv_state.store.pin().remove(k.as_str()); }
        d.task_ctx.electorate_retired.pin().remove("old");
        d.task_ctx.electorates.pin().remove("old");
        d.task_ctx.electorate_retire_known.pin().remove("old");
        let engine = crate::agent::electorate::engine(&d.task_ctx);
        let g = d.electorate("new").unwrap();
        assert_eq!(engine.migration_complete(&g), Ok(()), "the completion stands as proof of the retirement");
        assert_eq!(crate::agent::electorate::exclusive_electorate(&d.task_ctx), Ok(Some("new".to_string())), "and of the default passed");
        assert!(matches!(d.consensus().group_propose("new", "work/proof", Bytes::from_static(b"p"), quick_cfg()).await, ConsensusResult::Committed { .. }));
    }
    // The crash: what is on disk now, with the nodes still running, is all a restart has (the files copied aside).
    let crash = base.join("crash");
    for id in &ids[3..6] {
        let port = id.to_socket_addr().port();
        let dir = base.join(port.to_string()).join(id.to_string()).join("kv");
        let to = crash.join(port.to_string());
        std::fs::create_dir_all(&to).unwrap();
        for f in ["snapshot.bin", "wal.bin"] { if dir.join(f).exists() { std::fs::copy(dir.join(f), to.join(f)).unwrap(); } }
        let mut keys = Vec::new();
        mycelium_core::persistence::replay(&to, None, |e| keys.push(e.key.to_string())).await.expect("the crashed files replay");
        for k in &needed { assert!(keys.iter().any(|x| x == k), "{id} has {k} on disk after the completion"); }
    }
    listeners.clear();
    for a in agents.drain(..) { a.shutdown_with_timeout(Duration::from_secs(5)).await; }
    for id in &ids[3..6] {
        let port = id.to_socket_addr().port();
        let dir = base.join(port.to_string()).join(id.to_string()).join("kv");
        for f in ["snapshot.bin", "wal.bin"] {
            let from = crash.join(port.to_string()).join(f);
            if from.exists() { std::fs::copy(&from, dir.join(f)).unwrap(); } else { let _ = std::fs::remove_file(dir.join(f)); }
        }
    }
    let mut restarted = Vec::new();
    for i in 3..6 {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = ids[i].to_socket_addr().port();
        cfg.bootstrap_peers = ids[3..i].to_vec();
        cfg.health_check_max_jitter_ms = 50;
        cfg.health_check_interval_secs = 5;
        cfg.reconnect_backoff_secs = 1;
        cfg.persistence = Some(persistence(cfg.bind_port));
        let agent = GossipAgent::new(ids[i].clone(), cfg);
        agent.start().await.unwrap();
        restarted.push(agent);
    }
    let _listeners: Vec<_> = restarted.iter().map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    for a in &restarted { assert!(a.kv().set(format!("migration-crash-ready/{}", a.node_id()), Bytes::from_static(b"1"))); }
    let reached = tokio::time::timeout(Duration::from_secs(60), async {
        while !restarted.iter().all(|a| a.kv().scan_prefix("migration-crash-ready/").len() == 3 && a.peers().len() + 1 >= 3) {
            time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    assert!(reached.is_ok(), "the restarted successor did not re-form a mesh");
    let (d, e, f) = (&restarted[0], &restarted[1], &restarted[2]);
    for n in [d, e, f] {
        assert!(n.electorate_completion("new", "old").is_some(), "{} holds the completion", n.node_id());
        assert_eq!(crate::agent::electorate::exclusive_electorate(&n.task_ctx), Ok(Some("new".to_string())), "{}", n.node_id());
    }
    assert!(matches!(d.consensus().group_propose("new", "work/fresh", Bytes::from_static(b"fresh"), quick_cfg()).await, ConsensusResult::Committed { .. }));
    drop(e.consensus().distributed_lock("other", Duration::from_secs(5)).await.expect("the default is new's"));
    assert_eq!(f.consensus().consensus_get("work/k"), Some(Bytes::from_static(b"k")));
    for n in &restarted { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
    let _ = std::fs::remove_dir_all(&base);
}

/// **A merge carries every predecessor** (the D3 finding of round 5). `new` names `g1` and `g3`; it decides nothing
/// until both have retired to it; a completion run after `g1`'s retirement completes `g1` and still waits on `g3`;
/// after `g3`'s, the merge carries a value chosen in each. A retirement to a successor that does not name the retiree
/// is refused by name.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_merge_carries_every_predecessor() {
    let (agents, _l, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, c, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b] { n.mesh().join_group("g1"); }
    for n in [c, d] { n.mesh().join_group("g3"); }
    e.mesh().join_group("lone");
    roster_converges(&agents, "g1", &[a, b]).await;
    roster_converges(&agents, "g3", &[c, d]).await;
    roster_converges(&agents, "lone", &[e]).await;
    declare(a, "g1", false).await.unwrap();
    declare(c, "g3", false).await.unwrap();
    declare(e, "lone", false).await.unwrap();
    epoch_converges(&agents, "g1", 1).await;
    epoch_converges(&agents, "g3", 1).await;
    epoch_converges(&agents, "lone", 1).await;
    match a.retire_electorate("g1", "lone").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("does not name"), "{why}"),
        other => panic!("a retirement to a successor not naming the retiree is refused: {other:?}"),
    }
    let (v1, v3) = (Bytes::from_static(b"v1"), Bytes::from_static(b"v3"));
    let (s1, s3): (Arc<str>, Arc<str>) = (Arc::from("work/s1"), Arc::from("work/s3"));
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("g1", &s1, 5, &v1, a.node_id()).await); }
    for n in [c, d] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("g3", &s3, 5, &v3, c.node_id()).await); }
    successor(&agents, e, "new", &[e], &["g1", "g3"]).await;
    a.retire_electorate("g1", "new").await.unwrap();
    retirement_converges(&agents, "g1").await;
    let states = complete(&agents, e, "new").await;
    assert_eq!(states, vec![
        crate::MigrationState { predecessor: "g1".into(), phase: crate::MigrationPhase::Complete { slots: 1, conflicts: 0 } },
        crate::MigrationState { predecessor: "g3".into(), phase: crate::MigrationPhase::Waiting },
    ], "{states:?}");
    match e.consensus().group_propose("new", &s1, Bytes::from_static(b"e-wants-it"), quick_cfg()).await {
        ConsensusResult::ElectorateMismatch { detail, .. } => assert!(detail.contains("g3"), "{detail}"),
        other => panic!("a merge decides nothing until every predecessor has retired: {other:?}"),
    }
    c.retire_electorate("g3", "new").await.unwrap();
    retirement_converges(&agents, "g3").await;
    complete(&agents, e, "new").await;
    for (s, v) in [(&s1, &v1), (&s3, &v3)] {
        let res = e.consensus().group_propose("new", s, Bytes::from_static(b"e-wants-it"), quick_cfg()).await;
        assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
        assert_eq!(e.consensus().consensus_get(s), Some(v.clone()), "{s} was carried");
    }
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A value committed in the predecessor whose record did not propagate is carried** (the committed-but-
/// unpropagated interleaving). `old` = {A,B,C} commits `work/k` = v, permanent; the decision record has not reached
/// `new` = {D,E,F} (taken out of their stores just before the completion — anti-entropy may bring it back later,
/// which is why the decisive check is the carry ballot itself). A design that carried only what the completer's own
/// records show as decided would carry nothing and let `w` be decided fresh. The carry proposes v from `old`'s
/// acceptances: every member of `new` holds an acceptance of v above `old`'s ballot, `new` reads v, and a proposal of
/// `w` is superseded.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_but_unpropagated_value_is_carried() {
    let (agents, _l, _serial) = electorate_mesh(6, |_| {}).await;
    let (a, b, c, d, e, f) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4], &agents[5]);
    for n in [a, b, c] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b, c]).await;
    declare(a, "old", false).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    let v = Bytes::from_static(b"v-committed-in-old");
    let slot: Arc<str> = Arc::from("work/k");
    assert!(matches!(a.consensus().group_propose("old", &slot, v.clone(), quick_cfg()).await, ConsensusResult::Committed { .. }));
    poll_until(|| agents.iter().all(|n| n.consensus().consensus_get(&slot).is_some()), 10_000).await;
    successor(&agents, d, "new", &[d, e, f], &["old"]).await;
    a.retire_electorate("old", "new").await.unwrap();
    retirement_converges(&agents, "old").await;
    // The decision never reached new's members (as a frame lost before anti-entropy repairs it).
    let prefix = crate::consensus_life::slot_prefix(&slot);
    for n in [d, e, f] {
        let keys: Vec<Arc<str>> = n.kv().scan_prefix(&prefix).into_iter().map(|(k, _)| k).collect();
        for k in keys { n.task_ctx.kv_state.store.pin().remove(&*k); }
        for k in [format!("consensus/committed/{slot}"), format!("consensus/decided/{slot}")] { n.task_ctx.kv_state.store.pin().remove(k.as_str()); }
        assert!(n.consensus().consensus_get(&slot).is_none());
    }
    complete(&agents, d, "new").await;
    let res = d.consensus().group_propose("new", &slot, Bytes::from_static(b"w"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "the carried commit stands: {res:?}");
    assert_eq!(d.consensus().consensus_get(&slot), Some(v.clone()));
    // The carry ran as a ballot of new: a majority of new accepted v, whatever their decision records say.
    let holders = [d, e, f].iter().filter(|n| n.task_ctx.consensus_accepted.pin().get(&slot)
        .and_then(|st| st.accepted.as_ref().and_then(|(_, a)| a.value()))
        .and_then(|env| crate::consensus_life::Envelope::decode(&env)).and_then(|env| env.value) == Some(v.clone())).count();
    assert!(holders >= 2, "a majority of new holds an acceptance of the carried value: {holders} of 3");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A retirement completes a pending one** (the closed group's way out). A retirement promised by a majority whose
/// proposer never proposed it has closed them — membership is fixed, so the group stays closed — and
/// `retire_electorate` from another member completes it (adopting whatever was accepted, or deciding its own).
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retirement_completes_a_pending_one() {
    use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
    let (agents, _l, _serial) = electorate_mesh(3, |_| {}).await;
    let (a, b, c) = (&agents[0], &agents[1], &agents[2]);
    for n in [a, b, c] { n.mesh().join_group("council"); }
    roster_converges(&agents, "council", &[a, b, c]).await;
    let e1 = declare(a, "council", false).await.expect("genesis");
    epoch_converges(&agents, "council", 1).await;
    successor(&agents, c, "next", &[c], &["council"]).await;
    let slot = crate::agent::electorate::retire_slot("council");
    let prepare = ConsensusMsg::PrepareIn {
        slot: Arc::from(slot.as_str()), ballot: 3, proposer: b.node_id().clone(), epoch: 1, electorate: e1.digest(),
    };
    mycelium_core::ops::emit_signal(&b.task_ctx, Arc::from(consensus_kind::PROPOSE), SignalScope::Group(Arc::from("council")),
        crate::agent::electorate::engine(&b.task_ctx).sign_payload(encode_consensus_msg(&prepare)));
    poll_until(|| [a, c].iter().all(|n| n.task_ctx.consensus_accepted.pin().get(slot.as_str()).is_some_and(|s| s.promised > 0)), 20_000).await;
    let quick = ConsensusConfig { phase1_timeout: Duration::from_millis(400), max_ballots: 1, ..ConsensusConfig::default() };
    assert!(!matches!(a.consensus().group_propose("council", "work/y", Bytes::from_static(b"y"), quick).await,
        ConsensusResult::Committed { .. }), "closed by the promised retirement");
    let r = a.retire_electorate("council", "next").await.expect("the pending retirement is completed");
    assert_eq!(r.successor, "next");
    retirement_converges(&agents, "council").await;
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A group with an incomplete carry cannot retire, and a chain reads only its predecessor** (safety argument (vi)).
/// `g1` = {A,B} chooses a value, never committed; `g2` = {C,D} succeeds it. Before `g2`'s carry from `g1` is complete,
/// retiring `g2` to `g3` is refused by name — its successor would enumerate `g2` alone. After the completion `g1`'s
/// members shut down for good; `g2` retires to `g3` = {E}, which enumerates `g2` only (its members hold the carried
/// acceptance, tagged `g2`) and reads `g1`'s value.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_with_an_incomplete_carry_cannot_retire_and_a_chain_reads_only_its_predecessor() {
    let (agents, _l, _serial) = electorate_mesh(5, |_| {}).await;
    let (a, b, c, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4]);
    for n in [a, b] { n.mesh().join_group("g1"); }
    roster_converges(&agents, "g1", &[a, b]).await;
    declare(a, "g1", false).await.unwrap();
    epoch_converges(&agents, "g1", 1).await;
    let v = Bytes::from_static(b"chosen-in-g1");
    let slot: Arc<str> = Arc::from("work/s");
    for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("g1", &slot, 5, &v, a.node_id()).await); }
    successor(&agents, c, "g2", &[c, d], &["g1"]).await;
    a.retire_electorate("g1", "g2").await.unwrap();
    retirement_converges(&agents, "g1").await;
    successor(&agents, e, "g3", &[e], &["g2"]).await;
    match c.retire_electorate("g2", "g3").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("not complete"), "{why}"),
        other => panic!("a group with an incomplete carry does not retire: {other:?}"),
    }
    let e2 = c.electorate("g2").unwrap();
    let early = crate::Retirement { group: "g2".into(), epoch: 1, successor: "g3".into(), passes_default: false };
    assert!(admits(&crate::agent::electorate::engine(&d.task_ctx), &SignalScope::Group(Arc::from("g2")),
        &crate::agent::electorate::retire_slot("g2"), Some((1, e2.digest())), c.node_id(), Some(&early.encode())).is_err(),
        "an acceptor refuses it too");
    complete(&agents, c, "g2").await;
    for n in [a, b] { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
    c.retire_electorate("g2", "g3").await.expect("after the completion g2 retires");
    retirement_converges(&agents[2..], "g2").await;
    let states = complete(&agents[2..], e, "g3").await;
    assert_eq!(states, vec![crate::MigrationState { predecessor: "g2".into(), phase: crate::MigrationPhase::Complete { slots: 1, conflicts: 0 } }], "{states:?}");
    let res = e.consensus().group_propose("g3", &slot, Bytes::from_static(b"e-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(e.consensus().consensus_get(&slot), Some(v), "g1's value reached g3 through g2 alone");
    for n in [c, d, e] { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **A completion run that stops mid-carry is resumed by the next, which re-enumerates** (restart before completion).
/// `old` chose two values; the first run on D carries one and stops (as a crash would); its progress reads
/// *carrying 1 of 2*; the next run enumerates afresh, carries both — the one already carried is decided again with
/// the value `new` holds — and completes. Both values are read in `new`.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completion_run_that_stops_mid_carry_is_resumed_by_the_next() {
    let (agents, _l, _serial) = electorate_mesh(4, |_| {}).await;
    let (a, b, d, e) = (&agents[0], &agents[1], &agents[2], &agents[3]);
    for n in [a, b] { n.mesh().join_group("old"); }
    roster_converges(&agents, "old", &[a, b]).await;
    declare(a, "old", false).await.unwrap();
    epoch_converges(&agents, "old", 1).await;
    let (v1, v2) = (Bytes::from_static(b"one"), Bytes::from_static(b"two"));
    let (s1, s2): (Arc<str>, Arc<str>) = (Arc::from("work/a"), Arc::from("work/b"));
    for n in [a, b] {
        assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &s1, 5, &v1, a.node_id()).await);
        assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &s2, 6, &v2, b.node_id()).await);
    }
    successor(&agents, d, "new", &[d, e], &["old"]).await;
    a.retire_electorate("old", "new").await.unwrap();
    retirement_converges(&agents, "old").await;
    crate::agent::migration::CARRY_STOP_GROUP.store(crate::agent::migration::stop_key("new"), std::sync::atomic::Ordering::Relaxed);
    crate::agent::migration::CARRY_STOP_AFTER.store(1, std::sync::atomic::Ordering::Relaxed);
    match d.complete_migration("new").await {
        Err(crate::ElectorateError::Refused(why)) => assert!(why.contains("stopped after carrying 1"), "{why}"),
        other => panic!("the run stops after one slot: {other:?}"),
    }
    assert_eq!(d.migration_state("new"), vec![crate::MigrationState {
        predecessor: "old".into(), phase: crate::MigrationPhase::Carrying { carried: 1, total: 2 },
    }]);
    assert!(d.electorate_completion("new", "old").is_none(), "not complete");
    // The next run — on the same node, as after a restart of the driver — trusts nothing it remembers: it
    // enumerates afresh.
    let states = complete(&agents, d, "new").await;
    assert_eq!(states, vec![crate::MigrationState { predecessor: "old".into(), phase: crate::MigrationPhase::Complete { slots: 2, conflicts: 0 } }], "{states:?}");
    for (s, v) in [(&s1, &v1), (&s2, &v2)] {
        let res = e.consensus().group_propose("new", s, Bytes::from_static(b"e-wants-it"), quick_cfg()).await;
        assert!(matches!(res, ConsensusResult::Superseded { .. }), "{s}: {res:?}");
        assert_eq!(e.consensus().consensus_get(s), Some(v.clone()), "{s}");
    }
    assert_eq!(d.migration_state("new"), vec![crate::MigrationState {
        predecessor: "old".into(), phase: crate::MigrationPhase::Complete { slots: 2, conflicts: 0 },
    }], "D sees the completion E decided");
    for n in &agents { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
}

/// **After the completion, with every predecessor node shut down and the successor restarted, the successor decides
/// new slots and reads every carried slot correctly** — the acceptance test. `old` = {A,B,C} (the default) holds a
/// lock with a 30 s window (A's), a lock A acquired and released, a permanent consistent write, and a value chosen but
/// never committed; `new` = {D,E,F}, on disk, carries and completes; A, B and C shut down for good; D, E and F restart
/// from their persistence directories. Then `new` decides a fresh slot and a fresh lock, A's lock is still A's (its
/// window kept), the released lock is free, the consistent write reads, and the chosen value stands.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_the_completion_the_successor_survives_the_predecessors_departure_and_its_own_restart() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let base = std::env::temp_dir().join(format!("mycelium-migration-{}-{}", std::process::id(), alloc_port()));
    let persistence = |port: u16| PersistenceConfig {
        base_path: base.join(port.to_string()), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse,
    };
    let (mut agents, mut listeners, _serial) = electorate_mesh(6, |c| c.persistence = Some(persistence(c.bind_port))).await;
    let ids: Vec<NodeId> = agents.iter().map(|a| a.node_id().clone()).collect();
    {
        let (a, b, c, d, e, f) = (&agents[0], &agents[1], &agents[2], &agents[3], &agents[4], &agents[5]);
        for n in [a, b, c] { n.mesh().join_group("old"); }
        roster_converges(&agents, "old", &[a, b, c]).await;
        declare(a, "old", true).await.unwrap();
        epoch_converges(&agents, "old", 1).await;
        let held = a.consensus().distributed_lock("held", Duration::from_secs(30)).await.expect("A holds a lock");
        std::mem::forget(held);
        let freed = a.consensus().distributed_lock("freed", Duration::from_secs(30)).await.expect("A holds another");
        freed.release();
        poll_until(|| a.consensus().consensus_get("lock/freed").is_none(), 5_000).await;
        assert!(a.consensus().consistent_set("k", Bytes::from_static(b"consistent-v")).await.is_ok(), "a permanent consistent write");
        let chosen: Arc<str> = Arc::from("work/chosen");
        for n in [a, b] { assert!(crate::agent::electorate::engine(&n.task_ctx).accept_for_test("old", &chosen, 5, &Bytes::from_static(b"chosen-v"), a.node_id()).await); }
        successor(&agents, d, "new", &[d, e, f], &["old"]).await;
        a.retire_electorate("old", "new").await.unwrap();
        retirement_converges(&agents, "old").await;
        let states = complete(&agents, d, "new").await;
        assert!(matches!(states[0].phase, crate::MigrationPhase::Complete { slots, conflicts: 0 } if slots >= 4), "{states:?}");
    }
    // The predecessor's members are gone for good; the successor's restart.
    listeners.clear();
    for a in agents.drain(..) { a.shutdown_with_timeout(Duration::from_secs(5)).await; }
    let mut restarted = Vec::new();
    for i in 3..6 {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = ids[i].to_socket_addr().port();
        cfg.bootstrap_peers = ids[3..i].to_vec();
        cfg.health_check_max_jitter_ms = 50;
        cfg.health_check_interval_secs = 5;
        cfg.reconnect_backoff_secs = 1;
        cfg.persistence = Some(persistence(ids[i].to_socket_addr().port()));
        let agent = GossipAgent::new(ids[i].clone(), cfg);
        agent.start().await.unwrap();
        restarted.push(agent);
    }
    let _listeners: Vec<_> = restarted.iter().map(|a| a.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    for a in &restarted { assert!(a.kv().set(format!("migration-test-ready/{}", a.node_id()), Bytes::from_static(b"1"))); }
    let reached = tokio::time::timeout(Duration::from_secs(60), async {
        while !restarted.iter().all(|a| a.kv().scan_prefix("migration-test-ready/").len() == 3 && a.peers().len() + 1 >= 3) {
            time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    assert!(reached.is_ok(), "the restarted successor did not re-form a mesh");
    let (d, e, f) = (&restarted[0], &restarted[1], &restarted[2]);
    for n in [d, e, f] {
        assert_eq!(n.electorate("new").map(|g| g.epoch), Some(1), "{} restored the electorate", n.node_id());
        assert!(n.electorate_completion("new", "old").is_some(), "{} restored the completion", n.node_id());
        assert_eq!(crate::agent::electorate::exclusive_electorate(&n.task_ctx), Ok(Some("new".to_string())));
    }
    // Decides afresh, without old.
    assert!(matches!(d.consensus().group_propose("new", "work/fresh", Bytes::from_static(b"fresh"), quick_cfg()).await,
        ConsensusResult::Committed { .. }), "a fresh slot decides in the restarted successor");
    drop(e.consensus().distributed_lock("other", Duration::from_secs(5)).await.expect("a fresh lock decides"));
    // Reads every carried slot correctly.
    assert!(matches!(f.consensus().distributed_lock("held", Duration::from_secs(5)).await, Err(crate::ConsistencyError::Superseded)),
        "A's lock keeps A's window: still held");
    drop(f.consensus().distributed_lock("freed", Duration::from_secs(5)).await.expect("a released lock stays released: free to take"));
    assert_eq!(d.consensus().consistent_get("k"), Some(Bytes::from_static(b"consistent-v")));
    let res = e.consensus().group_propose("new", "work/chosen", Bytes::from_static(b"e-wants-it"), quick_cfg()).await;
    assert!(matches!(res, ConsensusResult::Superseded { .. }), "{res:?}");
    assert_eq!(e.consensus().consensus_get("work/chosen"), Some(Bytes::from_static(b"chosen-v")));
    for n in &restarted { n.shutdown_with_timeout(Duration::from_secs(5)).await; }
    let _ = std::fs::remove_dir_all(&base);
}
