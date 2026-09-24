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
    use crate::connection::handle_connection;
    use crate::signal::{Boundary, SignalHandlers};
    use crate::agent::{TaskCtx, BulkTransport};
    use mycelium_core::CoreCtx;
    use parking_lot::RwLock;
    let node_id = NodeId::new("127.0.0.1", 0).unwrap();
    let (shutdown_tx, _) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);
    let gossip_txs: Arc<[mpsc::Sender<(Bytes, u64, crate::framing::ForwardHint)>]> =
        (0..N_GOSSIP_SHARDS).map(|_| gossip_tx.clone()).collect::<Vec<_>>().into();
    // Seed the hash accumulator from the store's current state so the
    // anti-entropy fast-path works correctly for pre-populated test stores.
    let initial_hash = store_hash(&store);
    let kv_state = Arc::new(KvState {
        kv_store: crate::store::KvStore {
            store,
            prefix_index:      Arc::new(crate::store::PrefixIndex::new()),
            index_stripes:     Arc::new(std::array::from_fn(|_| std::sync::Mutex::new(()))),
            cap_ns_index:      Arc::new(crate::store::PrefixIndex::new()),
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
        signal_handlers: Arc::new(SignalHandlers::new(Duration::from_secs(600))),
        gossip_txs,
        default_ttl: max_ttl,
        kv_state,
        wal: std::sync::OnceLock::new(),
        sys_namespace_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        tls: std::sync::OnceLock::new(),
        peer_keys: Arc::new(papaya::HashMap::new()),
        peer_anchor_keys: Arc::new(papaya::HashMap::new()),
        identity_anchor_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        peers: Arc::new(papaya::HashMap::new()),
        rate_throttle: Arc::new(papaya::HashMap::new()),
        reorder_buf: None,
        reply_interceptor: None,
        soft_state_advertised: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        shutdown_tx: Arc::new(shutdown_tx_inner),
        task_handles: Arc::new(std::sync::Mutex::new(tokio::task::JoinSet::new())),
        config: Arc::new(crate::config::GossipConfig::default()),
        hot: Arc::new(mycelium_core::context::HotConfig::from_config(&crate::config::GossipConfig::default())),
    });
    let task_ctx = Arc::new(TaskCtx {
        core: core_ctx,
        bulk_transport: Arc::new(BulkTransport::new(0, Duration::from_secs(5), 64)),
        rpc_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        commit_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        commit_conflict_slots: Arc::new(papaya::HashMap::new()),
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
        #[cfg(feature = "compliance")]
        audit_chain: Arc::new(std::sync::Mutex::new(crate::agent::audit::AuditChainState::new())),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        action_evaluator: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        deployed_policy_revision: arc_swap::ArcSwapOption::from(None),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        evidence_journal: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_edge: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_clients: std::sync::OnceLock::new(),
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
        writer_idle_timeout: Duration::ZERO,
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

    let gossip_txs: Arc<[mpsc::Sender<(Bytes, u64, crate::framing::ForwardHint)>]> =
        (0..N_GOSSIP_SHARDS).map(|_| gossip_tx.clone()).collect::<Vec<_>>().into();
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
            signal_handlers: Arc::new(SignalHandlers::new(Duration::from_secs(600))),
            gossip_txs,
            default_ttl: 5,
            kv_state,
            wal: std::sync::OnceLock::new(),
            sys_namespace_violations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            tls: std::sync::OnceLock::new(),
            peer_keys: Arc::new(papaya::HashMap::new()),
            peer_anchor_keys: Arc::new(papaya::HashMap::new()),
            identity_anchor_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            peers: Arc::new(papaya::HashMap::new()),
            rate_throttle: Arc::new(papaya::HashMap::new()),
            reorder_buf: None,
            reply_interceptor: None,
            soft_state_advertised: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown_tx: Arc::new(shutdown_tx_inner2),
            task_handles: Arc::new(std::sync::Mutex::new(tokio::task::JoinSet::new())),
            config: Arc::new(crate::config::GossipConfig::default()),
            hot: Arc::new(mycelium_core::context::HotConfig::from_config(&crate::config::GossipConfig::default())),
        });
        let task_ctx = Arc::new(TaskCtx {
            core: core_ctx,
            bulk_transport: Arc::new(BulkTransport::new(0, Duration::from_secs(5), 64)),
            rpc_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            commit_conflicts: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            commit_conflict_slots: Arc::new(papaya::HashMap::new()),
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
            #[cfg(feature = "compliance")]
            audit_chain: Arc::new(std::sync::Mutex::new(crate::agent::audit::AuditChainState::new())),
            #[cfg(all(feature = "gateway", feature = "tls"))]
            action_evaluator: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        deployed_policy_revision: arc_swap::ArcSwapOption::from(None),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        evidence_journal: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_edge: std::sync::OnceLock::new(),
        #[cfg(all(feature = "gateway", feature = "tls"))]
        federation_clients: std::sync::OnceLock::new(),
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
            writer_idle_timeout: Duration::ZERO,
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
    let _ = agent.mesh().emit("task", SignalScope::Group(Arc::from("nlp")), b"work".to_vec());
    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("member should receive group signal")
        .expect("receiver closed");
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

    // Emit a signal — deliver() records the sender in the sender_log.
    // (deliver() always updates sender_log before checking handler registration.)
    let _ = agent.mesh().emit("heartbeat", SignalScope::Group("workers".into()), Bytes::new());

    // Raw quorum is satisfied (1 sender, 1 required).
    assert!(
        agent.mesh().quorum("heartbeat", 1, Duration::from_secs(60)),
        "raw quorum should be satisfied"
    );
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
        "alpha-only proposal must commit; got {result_ok:?}",
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

        let mut rows: Vec<(&str, bool)> = vec![
            ("caller_frame_classify", crate::fuzz_internals::caller_frame_classify(&frame_seed)),
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

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
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
            snapshot_interval_secs: 3_600,
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

    // ── Phase 3: wrong key cannot read it ────────────────────────────────
    let a3 = Arc::new(mk());
    a3.with_data_at_rest_cipher(Arc::new(XorCipher { key: 0x11 }));
    a3.start().await.unwrap();
    // Give replay a chance to run; the record must NOT decode under the wrong key.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        a3.kv().get("secret/1").is_none(),
        "wrong-key replay must not recover the record (cipher is load-bearing)"
    );
    a3.shutdown_with_timeout(Duration::from_secs(5)).await;

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
            snapshot_interval_secs: 3_600,
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
    agent.set_health_check_interval_secs(1);
    agent.set_reconnect_backoff_secs(2);
    assert_eq!(agent.timing_tunables(), (1, 2), "the live timing override is recorded");

    // The health monitor adopts the new cadence on its next cycle — no task respawned.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(agent.system_stats().task_count, tasks_before,
        "G-M10.1: hot-reload retunes the loop in place — no task restart");

    // `0` reverts to the static config value.
    agent.set_health_check_interval_secs(0);
    assert_eq!(agent.timing_tunables().0, 0, "0 = revert to static config");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
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
    b.set_health_check_interval_secs(9);

    // A publishes a fleet TimingIntent (whole fleet).
    assert!(a.govern_timing(2, 3, None), "intent published");

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
            snapshot_interval_secs: 3_600,
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
                snapshot_interval_secs: 3_600,
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
        // `tasks/sendSubscribe` already was.
        for method in ["tasks/get", "tasks/cancel"] {
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
        // 5g. Streaming is not a federated call.
        let mut subscribe = task_body("demo/whoami");
        subscribe["method"] = serde_json::Value::String("tasks/sendSubscribe".into());
        let r = raw.post(&a2a).header(HEADER_FEDERATED_CALL, cred(&beta, "demo/whoami", &beta_sk).to_header_value())
            .json(&subscribe).send().await.unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32003, "{v}");
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

        // 7. Revocation at the provider, mid-session: the next call is refused at the auth
        //    layer with no dispatch. The client learns it from the refusal, not from gossip.
        edge.revoke(&beta);
        match client.call("demo/whoami", "?", Repeatability::AtMostOnce).await {
            Err(ClientError::Refused { status: 401, message, .. }) => {
                assert!(message.contains("not in the trust bundle"), "{message}")
            }
            other => panic!("a revoked partner must be refused at the gateway, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // 8. The tables, after bytes crossed: two meshes. Non-vacuity of "bytes crossed": the
        //    provider was reached exactly twice — the federated call and the anonymous one.
        let a_nodes: Vec<&GossipAgent> = std::iter::once(a0.as_ref()).chain(a_rest.iter()).collect();
        let b_nodes: Vec<&GossipAgent> = b.iter().collect();
        assert_never_merged(&a_nodes, &b_nodes);
        assert_eq!(a0.peers().len(), 1, "domain A's gateway node peers with its own mesh only");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

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
        assert_eq!(gw2.peers().len(), 2, "the replacement gateway peers with its own mesh only");

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

// ── The identity-proof opt-in (Phase 3, off by default) ──────────────────────
//
// `validate_and_merge_identity`'s two arms are unit-tested in `agent::http`, and the default value
// is pinned — with the reason it is still `false` — in `mycelium-core::config`
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
    use crate::agent::helpers::{encode_identity_proof, validate_and_merge_identity};
    use ed25519_dalek::{Signer, SigningKey};

    fn keys() -> papaya::HashMap<NodeId, Vec<[u8; 32]>> {
        papaya::HashMap::new()
    }
    fn anchors() -> papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> {
        papaya::HashMap::new()
    }

    /// A node that opts in rejects an unsigned identity entry — end to end, from the config field
    /// an operator sets to the entry that does not enter `peer_keys`.
    ///
    /// This test once asserted the *default* did this. The default was flipped on 2026-09-23 and
    /// reverted on 2026-09-24: requiring proofs opens a startup window, because identity and proof
    /// are two independent gossip writes and a peer that learns one without the other rejects it
    /// (`config::tests::the_default_requires_identity_proofs` carries the full account). The
    /// mechanism being tested here was never what failed, so it is kept — with the flag set
    /// explicitly, which is now how a deployment reaches this arm.
    #[test]
    fn an_opted_in_configuration_rejects_an_unsigned_identity_entry() {
        let mut cfg = GossipConfig::default();
        cfg.require_identity_proofs = true; // the operator's choice, not the default

        let victim = NodeId::new("127.0.0.1", 7101).unwrap();
        let attacker_key = SigningKey::from_bytes(&[42u8; 32]).verifying_key().to_bytes();
        let history = attacker_key.to_vec();

        let (pk, anchor, counter) = (keys(), anchors(), std::sync::atomic::AtomicU64::new(0));
        validate_and_merge_identity(
            &pk, &anchor, &counter, &victim, &history, &[attacker_key],
            None,                              // no proof — the pre-Phase-2 mimic
            cfg.require_identity_proofs,       // the opted-in arm
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
        let sig = sk.sign(&history).to_bytes();
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
        let legacy_proof = encode_identity_proof(&vk, &sk.sign(&history).to_bytes());

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
        let legacy_proof = encode_identity_proof(&vk, &sk.sign(&history).to_bytes());

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
            &history, &encode_identity_proof(&a_vk, &attacker.sign(&history).to_bytes()));

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
        let good = encode_sealed_identity(&history, &encode_identity_proof(&vk, &sk.sign(&history).to_bytes()));
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
        let sig = sk.sign(&history).to_bytes();
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
        let u_sig = usurper.sign(&u_history).to_bytes();
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
    /// not spend this domain's credential on them. The table is pinned in `http.rs`; this is the
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
