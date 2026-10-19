//! [`CoreCtx`] — the shared Layers I + II infrastructure bundle.
//!
//! Carries identity/config, the Layer I KV substrate, the Layer II signal mesh,
//! transport security, networking, and lifecycle handles — everything the gossip
//! connection handler and writer need. The full `mycelium` crate wraps this in its
//! `TaskCtx` (adding Layer III+) and derefs to it, so existing `ctx.<core-field>`
//! sites are unchanged.
//!
//! **Invariant:** `CoreCtx` never references a higher-layer type (philosophy §5a —
//! the substrate is never aware of the layers above it).

use crate::config::GossipConfig;
use crate::node_id::NodeId;
use crate::seen::ShardedSeen;
use crate::signal::{Boundary, Signal, SignalHandlers};
use crate::store::KvState;
use parking_lot::RwLock;
use std::sync::{atomic::{AtomicU64, AtomicUsize, Ordering}, Arc};
use tokio::{sync::watch, task::JoinSet};

/// Runtime-tunable ("hot") subset of [`GossipConfig`] (WS-C M9). These parameters are
/// sampled from an atomic cell **on each use** (or each writer / bulk-handler spawn)
/// rather than captured at task-spawn time, so a live cluster can retune them with **no
/// task restart**. Initialised from `GossipConfig` at agent construction (after M8
/// auto-derivation), then updated either by an operator (`GossipAgent::set_*`) or by the
/// `ClusterTuner` advisor over `sys/config/` — in both cases the node applies the change
/// itself (advisor advises, node decides — Core Principle 1).
#[derive(Debug)]
pub struct HotConfig {
    /// Per-peer inbound frame-rate cap (`0` = unlimited). Sampled per inbound frame in
    /// the connection handler.
    pub max_inbound_frames_per_sec: AtomicU64,
    /// Depth of each *new* per-peer writer channel. Sampled at writer spawn — existing
    /// writers keep their channel; new / reconnecting peers pick up the new depth.
    pub writer_channel_depth: AtomicUsize,
    /// Concurrent bulk-handler cap (`0` = unlimited). Sampled per bulk-serve admission.
    pub max_concurrent_bulk_handlers: AtomicUsize,
    /// **Timing params (WS-C / M10), live-reconfigurable.** The background loops re-read these each
    /// cycle (via a dynamic sleep, not a fixed `interval`), so a change takes effect on the next tick
    /// with **no task restart**. Cluster-wide changes propagate via an evaporating `TimingIntent`
    /// (management-as-intent — newest-wins, local-wins, no consensus fence; see
    /// `docs/plans/v2-wsc-m7-m10.md` for why a fence would import a coordinator the self-healing
    /// substrate doesn't need). `0` means "leave at the static config value" for that param.
    pub health_check_interval_secs: AtomicU64,
    pub reconnect_backoff_secs: AtomicU64,
    /// Per-param local pins, set by the corresponding node-local setter
    /// (`set_health_check_interval_secs` / `set_reconnect_backoff_secs`). While a param is pinned, a
    /// cluster-wide `TimingIntent` does **not** govern THAT param — local always wins (M10.2) — but
    /// pinning one no longer freezes the other (a single shared flag did; audit 2026-07-15 pass 5).
    /// The node owns its own config; an operator's explicit override is sovereign over fleet
    /// governance, per param.
    pub health_locally_pinned:    std::sync::atomic::AtomicBool,
    pub reconnect_locally_pinned: std::sync::atomic::AtomicBool,
}

impl HotConfig {
    /// Snapshot the hot subset from a (already M8-derived) config.
    pub fn from_config(c: &GossipConfig) -> Self {
        Self {
            max_inbound_frames_per_sec:   AtomicU64::new(c.max_inbound_frames_per_sec),
            writer_channel_depth:         AtomicUsize::new(c.writer_channel_depth),
            max_concurrent_bulk_handlers: AtomicUsize::new(c.max_concurrent_bulk_handlers),
            health_check_interval_secs:   AtomicU64::new(c.health_check_interval_secs),
            reconnect_backoff_secs:       AtomicU64::new(c.reconnect_backoff_secs),
            health_locally_pinned:        std::sync::atomic::AtomicBool::new(false),
            reconnect_locally_pinned:     std::sync::atomic::AtomicBool::new(false),
        }
    }
    #[inline] pub fn inbound_fps(&self) -> u64 { self.max_inbound_frames_per_sec.load(Ordering::Relaxed) }
    #[inline] pub fn writer_depth(&self) -> usize { self.writer_channel_depth.load(Ordering::Relaxed) }
    #[inline] pub fn bulk_handlers(&self) -> usize { self.max_concurrent_bulk_handlers.load(Ordering::Relaxed) }
    /// Live health-check interval (secs); falls back to `fallback` (the static config) when unset (`0`).
    #[inline] pub fn health_interval_secs(&self, fallback: u64) -> u64 {
        let v = self.health_check_interval_secs.load(Ordering::Relaxed);
        if v == 0 { fallback } else { v }
    }
    /// Live reconnect backoff (secs); falls back to `fallback` when unset (`0`).
    #[inline] pub fn reconnect_backoff_secs(&self, fallback: u64) -> u64 {
        let v = self.reconnect_backoff_secs.load(Ordering::Relaxed);
        if v == 0 { fallback } else { v }
    }
}

/// Opt-in pre-delivery signal interceptor registered by the upper service layer.
/// Given a delivered [`Signal`], it claims correlated `rpc.result` / `bulk.result`
/// replies — firing the waiting oneshot — and returns `true` to skip the
/// `signal_handlers` fan-out. Core knows nothing about RPC: the connection handler
/// only asks "did anything claim this signal?" The RPC correlation law lives in the
/// closure the service layer registers (mechanism in core; agency above).
pub type ReplyInterceptor = Arc<dyn Fn(&Signal) -> bool + Send + Sync>;

/// What [`CoreCtx::inbound_observer`] holds: the key prefixes the upper layer asked to see, and the
/// callback that sees each inbound update under them. Called inline on the connection task, so it must
/// be cheap and must not block — a decode, a lock-free lookup, a counter, a log line.
#[derive(Clone)]
pub struct InboundObserver {
    /// Key prefixes (`sys/consensus-accepted/{me}/`, …); an update is reported when its key starts
    /// with any of them.
    pub prefixes: Vec<String>,
    /// The callback, with the update as it arrived.
    pub on_inbound: Arc<dyn Fn(&crate::framing::GossipUpdate) + Send + Sync>,
}

impl InboundObserver {
    /// Whether `key` is under one of the observed prefixes.
    pub fn watches(&self, key: &str) -> bool {
        self.prefixes.iter().any(|p| key.starts_with(p.as_str()))
    }
}

/// The shared Layers I + II infrastructure bundle. The full `mycelium` crate's
/// `TaskCtx` holds this as `core: Arc<CoreCtx>` and `Deref`s to it.
pub struct CoreCtx {
    // ── Identity + config ────────────────────────────────────────────────────────
    pub node_id:          NodeId,
    /// Shared copy of the agent configuration. Available to typed handles so they
    /// can access `signal_window_secs`, `health_check_interval_secs`, `locality_path`,
    /// and `topology_policies` without borrowing `GossipAgent`.
    pub config:           Arc<GossipConfig>,
    /// Runtime-tunable subset of `config` (WS-C M9). Hot-path code reads tuning values
    /// from here (atomic, live) instead of the immutable `config` snapshot.
    pub hot:              Arc<HotConfig>,
    pub default_ttl:      u8,

    // ── Layer I — KV substrate ───────────────────────────────────────────────────
    pub seen:             Arc<ShardedSeen>,
    /// Hybrid Logical Clock for causal LWW ordering. `make_gossip_update`
    /// calls `tick()` for every locally-originated write; the connection
    /// handler calls `observe()` for every incoming timestamp so the local
    /// clock dominates any remote stamp it has seen.
    pub hlc:              Arc<crate::hlc::Hlc>,
    /// The gossip shard queues; every send through them is counted (`outbound`).
    pub gossip_txs:       crate::outbound::GossipTxs,
    pub kv_state:         Arc<KvState>,
    /// WAL handle for durable KV writes. Unset when persistence is disabled.
    /// Written once by `start()` after replay; read-only afterwards.
    pub wal: std::sync::OnceLock<Arc<crate::persistence::WalHandle>>,

    // ── Layer II — Signal mesh ───────────────────────────────────────────────────
    pub signal_boundary:  Arc<RwLock<Boundary>>,
    /// The decision trace's sink (`crate::decision`, plan I7): set once, before start, by the
    /// agent's `with_decision_trace`; absent means no decision point records.
    pub decision_sink:    std::sync::OnceLock<Arc<crate::decision::DecisionSink>>,
    pub signal_handlers:  Arc<SignalHandlers>,
    /// Receiver-side causal reorder buffer for `emit_ordered` signals.
    /// `None` when `config.signal_ordered_delivery = false` (the default).
    pub reorder_buf: Option<Arc<std::sync::Mutex<crate::signal::SignalReorderBuffer>>>,
    /// Opt-in pre-delivery signal interceptor (see [`ReplyInterceptor`]). `None`
    /// for pure KV/signal embeds (zero overhead); the upper service layer sets it
    /// to claim correlated RPC/bulk replies. Core stays RPC-agnostic.
    pub reply_interceptor: Option<ReplyInterceptor>,

    // ── Security (transport) ─────────────────────────────────────────────────────
    /// TLS context (server + client configs + signing key). Unset when the
    /// `tls` feature is disabled or when `GossipConfig::tls` is `None`.
    /// Written once by `start()` before any task is spawned; read-only afterwards.
    pub tls: std::sync::OnceLock<Arc<crate::tls::NodeTls>>,
    /// Retained verifying-key **set** per node (WS5 option B): every key a node
    /// has published at `sys/identity/{node}`, accumulated across rotations so
    /// historical signatures (audit, consensus, roles) keep verifying. Verify
    /// paths try all keys.
    #[cfg_attr(not(feature = "tls"), allow(dead_code))]
    pub peer_keys: Arc<papaya::HashMap<NodeId, Vec<[u8; 32]>>>,

    /// **CA-authenticated** identity keys, harvested from each *directly-connected* peer's
    /// validated mTLS cert (identity-auth Phase 1b). Distinct from `peer_keys` (which mirrors
    /// the unauthenticated `sys/identity/` KV) so the merge path can tell a CA-anchored key from
    /// a KV-asserted one — the basis of the conflict tripwire now and the signed-proof chain in
    /// Phase 2. Populated on the outbound writer path (see `writer::run_peer_writer`); a set per
    /// peer because a peer's anchor accumulates across rotations/reconnects.
    #[cfg_attr(not(feature = "tls"), allow(dead_code))]
    pub peer_anchor_keys: Arc<papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>>>,

    /// Cumulative identity-anchor conflicts (see `SystemStats::identity_anchor_conflicts`): a
    /// `sys/identity/{V}` KV entry introduced a key for a `V` whose CA-anchored key is known and
    /// differs. Detection-only in Phase 1b (poisoning signal; may briefly trip on a legitimate
    /// rotation until the new key is re-anchored). Relaxed — diagnostic.
    #[cfg_attr(not(feature = "tls"), allow(dead_code))]
    pub identity_anchor_conflicts: Arc<AtomicU64>,

    /// Cumulative `sys/` namespace-ownership violations (see
    /// `SystemStats::sys_namespace_violations`). Incremented by the connection
    /// handler's inbound-apply tripwire when a remote write targets a `sys/`
    /// key this node owns; Relaxed ordering — purely diagnostic. Core because the
    /// connection handler (Layer I transport) is the sole writer.
    pub sys_namespace_violations: Arc<AtomicU64>,

    /// An observer of **inbound** updates under named key prefixes ([`InboundObserver`]), installed
    /// once before start by the upper layer (`OnceLock`, like `decision_sink`). The connection handler
    /// calls it for every inbound gossip frame or anti-entropy entry whose key starts with one of its
    /// prefixes, after the update was applied — whatever LWW decided about it. Core reports the
    /// arrival; it compares nothing and refuses nothing (a consensus acceptor comparing a peer's copy
    /// of its own record against its memory lives in `mycelium::agent::acceptor_guard`). Absent for a
    /// pure KV/signal embed: one prefix test per inbound update, nothing else.
    pub inbound_observer: std::sync::OnceLock<InboundObserver>,

    /// Transport resource bounds (row B): the counters behind `SystemStats::{inbound_connections_timed_out,
    /// anti_entropy_replies_skipped}` and the per-peer anti-entropy reply slots.
    pub transport_bounds: Arc<crate::bounds::TransportBounds>,

    /// **Removed members** (closure plan C5): consulted on the per-message path and by the TLS
    /// handshake. Empty until an operator removes someone.
    pub removed: Arc<crate::removal::RemovedSet>,

    /// **Key standing, per node** (post-360 plan row Q, `docs/design/revocation-authority.md`): which
    /// keys a certificate record anchors for a node, and which it has revoked. Every verifier of a
    /// node's key reads it through [`CoreCtx::trusted_keys`]; written by the root crate's fold. Empty
    /// until a certificate record or a revocation is judged.
    pub key_standing: Arc<crate::removal::KeyStanding>,

    // ── Networking ───────────────────────────────────────────────────────────────
    /// Live peer table shared with the HTTP gateway for peer-count-based quorum sizing. The value
    /// is when the peer was last heard from.
    ///
    /// The type stays `Instant`: what a replay must reproduce is the staleness *decision*, and
    /// every such decision asks "how long since this", so the **interval** is what the kernel owns
    /// (`sim_seam::mono_elapsed`). Changing the stored type would have been an API break that
    /// bought nothing.
    pub peers: Arc<papaya::HashMap<NodeId, std::time::Instant>>,

    /// **M7 distributed rate-limiting** (WS-C): per-sender locally-decided throttle budget (fps).
    /// Empty unless `rate_observation_enabled`. The rate-decider task sets a fair-share budget for a
    /// sender whose *aggregate* observed rate (summed across all observers via `sys/rate/`) crosses
    /// the threshold; the connection read loop reads it and clamps the sender's effective inbound
    /// limit. A node-local decision on shared evidence — never a cluster eviction verdict.
    pub rate_throttle: Arc<papaya::HashMap<Arc<str>, std::sync::atomic::AtomicU64>>,

    /// Set to `true` by the first tick of any [`run_kv_persist_task`](crate::kv_persist::run_kv_persist_task)
    /// — the substrate's generic soft-state advertisement loop (capability,
    /// locality, `advertise_persistent`, …). Until this is `true`, soft-state KV
    /// keys have not yet been written after a (re)start, so the gateway `/ready`
    /// probe returns 503. Substrate-level readiness, not a Layer III concept:
    /// the persist loop that flips it is pure Layer I. Stored with Release;
    /// loaded with Acquire (see the memory-ordering policy in CLAUDE.md).
    pub soft_state_advertised: Arc<std::sync::atomic::AtomicBool>,
    /// The gate every soft-state re-assert passes through; shutdown closes it before sweeping the
    /// node's own soft state, so no tick re-writes a swept key (#627's review).
    pub soft_state_gate: Arc<crate::kv_persist::SoftStateGate>,

    // ── Lifecycle ────────────────────────────────────────────────────────────────
    /// Shutdown broadcast — sending `true` cancels all background tasks.
    pub shutdown_tx: Arc<watch::Sender<bool>>,
    /// All spawned background tasks. Reaping is automatic via `JoinSet`.
    pub task_handles: Arc<std::sync::Mutex<JoinSet<()>>>,
}

impl CoreCtx {
    /// Spawns a background task onto the shared [`JoinSet`](Self::task_handles).
    ///
    /// The guard is released before the future is polled; this is the single
    /// substrate task-spawn entry point used by the typed handles' task helpers
    /// (`advertise`, `watch`, `subscribe_log`, …) and the gossip lifecycle tasks.
    ///
    /// Tasks that have already finished are reaped first: a `JoinSet` keeps a finished task's entry until it is
    /// joined, and nothing joins this set before shutdown, so a caller that spawns one short task per event (row K2's
    /// seat release, one per admitted permit) would otherwise grow it for the node's life.
    pub fn spawn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut handles = self.task_handles.lock().unwrap_or_else(|e| e.into_inner());
        while handles.try_join_next().is_some() {}
        handles.spawn(fut);
    }
}

impl CoreCtx {
    /// The keys this node **currently trusts** to have signed for `node` (post-360 plan row Q,
    /// `docs/design/revocation-authority.md`). For a node with a certificate record naming it
    /// (anchored): exactly its anchored keys that it has not revoked. For a legacy node: the retained
    /// set in `peer_keys`, as before row Q. Removed members' keys are never trusted. The one answer
    /// every verifier of a node's key uses — the KV `SignedData` arm, the unsigned identity exemption,
    /// identity chaining, consensus, caller envelopes, `known_verifying_keys`, knowledge records.
    ///
    /// A revocation seen *after* a write was verified does not un-apply that write (detection, not
    /// prevention); every verification after it is seen refuses.
    pub fn trusted_keys(&self, node: &NodeId) -> Vec<[u8; 32]> {
        let mut keys: Vec<[u8; 32]> = match self.key_standing.get(node) {
            Some(s) if s.anchored.is_some() => {
                s.anchored.iter().flatten().filter(|k| !s.revoked.contains(*k)).copied().collect()
            }
            _ => self.peer_keys.pin().get(node).cloned().unwrap_or_default(),
        };
        self.retain_trusted(node, &mut keys);
        keys
    }

    /// The keys this node accepts as **having signed history** for `node` (post-360 plan row Q, the
    /// final review's finding 1): every key it holds for the node — its retained set in `peer_keys` and
    /// its anchored keys — minus the keys the node revoked and removed members' keys. **Verify-only**:
    /// audit-chain recovery and verification read it, so a record sealed under a key retired before
    /// the node was re-anchored still verifies; live writes and signatures are admitted only under
    /// [`Self::trusted_keys`]. A key reaches `peer_keys` only through an identity record the identity
    /// watcher validated — and a node's own record is built from its certificate records and its own
    /// verified sealed record, never the raw `sys/identity/{node}` value (the confirming review's N2) —
    /// so with `require_identity_proofs` (the default since post-360 row S) a member cannot add a key
    /// to another's history. With proofs explicitly off a bare identity record still can, as before
    /// row Q: the identity-poisoning residual, and it reaches history verification.
    pub fn history_keys(&self, node: &NodeId) -> Vec<[u8; 32]> {
        let mut keys: Vec<[u8; 32]> = self.peer_keys.pin().get(node).cloned().unwrap_or_default();
        if let Some(s) = self.key_standing.get(node) {
            for k in s.anchored.iter().flatten() {
                if !keys.contains(k) {
                    keys.push(*k);
                }
            }
            keys.retain(|k| !s.revoked.contains(k));
        }
        if !self.removed.is_empty() {
            keys.retain(|k| !self.removed.is_key_removed(k));
        }
        keys
    }

    /// Drop from `keys` (offered for `node`) every key this node does not trust for it: revoked by
    /// `node`, not among an anchored node's anchored keys, or a removed member's. This node's own
    /// current key is always kept for itself (its certificate record may not have been judged yet).
    pub fn retain_trusted(&self, node: &NodeId, keys: &mut Vec<[u8; 32]>) {
        if !self.key_standing.is_empty() {
            #[cfg(feature = "tls")]
            let own = (node == &self.node_id).then(|| self.tls.get().map(|t| t.verifying_key_bytes())).flatten();
            #[cfg(not(feature = "tls"))]
            let own: Option<[u8; 32]> = None;
            keys.retain(|k| Some(*k) == own || !self.key_standing.is_distrusted(node, k));
        }
        if !self.removed.is_empty() {
            keys.retain(|k| !self.removed.is_key_removed(k));
        }
    }

    /// The attached decision sink, if any (plan I7).
    pub fn decision_sink(&self) -> Option<&crate::decision::DecisionSink> {
        self.decision_sink.get().map(|a| a.as_ref())
    }
}
