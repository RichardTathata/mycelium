//! Consensus operations — [`ConsensusHandle`].
//!
//! Wraps the agreement primitives (Layer III — prepare, propose, commit):
//! group proposals, cluster-wide proposals, cross-group proposals,
//! distributed locks, leader election, trust-slice declarations,
//! and the consistent KV overlay.
//!
//! Obtain a handle via [`GossipAgent::consensus`](crate::GossipAgent::consensus).

use crate::consensus::{
    consensus_kind, consensus_ns, ConsensusConfig, ConsensusListenerHandle,
    ConsensusResult, OpaqueRecompute,
};
use crate::node_id::NodeId;
use crate::signal::SignalScope;
use ahash::AHashSet;
use bytes::Bytes;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

use super::TaskCtx;
use super::helpers::{
    cached_group_members_ctx, compute_quorum_size,
    kv_get, kv_scan_prefix, kv_set, kv_subscribe, make_consensus_engine_ctx,
    suggest_leader_ctx, declared_electorate_min, resolve_electorate, Electorate};
use super::opacity::{
    count_opaque_members_ctx, count_opaque_system_ctx, effective_opacity_ctx,
    peer_load_ctx, count_opaque_members_in_kv, count_opaque_all_in_kv,
};

// Re-export public types used by callers.
pub use super::overlay_consistent::{ConsistencyError, LockGuard};

/// Domain handle for consensus operations. Obtained via [`GossipAgent::consensus()`].
///
/// Provides group proposals, cluster-wide proposals, cross-group proposals,
/// distributed locks, leader election, trust-slice declarations,
/// and the consistent KV overlay.
///
/// The handle is `Clone + Send + Sync` and can be stored, moved across tasks,
/// or captured in closures.
#[derive(Clone)]
pub struct ConsensusHandle {
    pub(crate) ctx: Arc<TaskCtx>,
}

/// Translate a [`ConsensusResult`] into the receipt vocabulary (item 1 PR 2).
///
/// The one judgement here: `persisted: true` on a node with **no persistence configured** means
/// *nothing was promised*, not *on disk* — the collapse D24 names. The receipt separates them by
/// reading the node's own configuration, which is the only place that distinction exists.
pub(crate) fn receipt_from(
    result: ConsensusResult,
    ctx: &Arc<TaskCtx>,
) -> Result<crate::CommitReceipt, crate::CommitError> {
    use crate::{CommitError, CommitReceipt, LocalDurability};
    match result {
        ConsensusResult::Committed { slot, value, ballot, persisted, .. } => {
            let durability = match (ctx.config.persistence.as_ref(), persisted) {
                (None, _) => LocalDurability::NotConfigured,
                // The commit path forces `append_sync`, which fsyncs in every `SyncMode`, so a
                // `true` from a persisted node is genuinely on disk — unlike the ordinary write
                // path, where `Async`/`Os` leaves the record `Buffered`.
                (Some(_), true) => LocalDurability::OnDisk,
                (Some(_), false) => LocalDurability::Failed(
                    "the commit's WAL append did not acknowledge; durability not established".into(),
                ),
            };
            Ok(CommitReceipt::new(slot, value, ballot, durability))
        }
        ConsensusResult::Timeout { slot, ballots_tried, .. } => {
            Err(CommitError::DeliveryUnknown { slot, ballots_tried })
        }
        // No electorate, so nothing was proposed and nothing can be in flight. This is a *refusal*,
        // not a `DeliveryUnknown`: the distinction is the contract's own (`contracts-receipts.md`
        // §1) — a timeout may still commit later, a refusal never will.
        ConsensusResult::ElectorateUnavailable { slot, observed_members, declared_min, .. } =>
            Err(CommitError::ElectorateUnavailable { slot, observed_members, declared_min }),
        ConsensusResult::NotAMember { slot, group } => Err(CommitError::NotAMember { slot, group }),
        ConsensusResult::Superseded { slot, ballot } => Err(CommitError::Superseded { slot, ballot }),
        ConsensusResult::TopologyUnsatisfied { slot, distinct_domains, domains_required, .. } => {
            Err(CommitError::TopologyUnsatisfied {
                slot,
                distinct: distinct_domains,
                required: domains_required,
            })
        }
    }
}

impl ConsensusHandle {
    // ── Signal window helper ─────────────────────────────────────────────────

    fn signal_window(&self) -> Duration {
        Duration::from_secs(self.ctx.config.signal_window_secs)
    }

    // ── Consensus ops ────────────────────────────────────────────────────────

    /// Subscribes to committed values for a consensus slot.
    ///
    /// Returns a `watch::Receiver` that fires whenever the slot is committed or
    /// overwritten. Initial value is the current committed state (or `None`).
    ///
    /// **Raw KV view**: the receiver reflects the stored bytes and does not
    /// apply the epoch-lease convention — an expired leased slot still shows
    /// its last value here. Use [`consensus_get`](Self::consensus_get) for
    /// lease-aware reads.
    #[must_use]
    pub fn consensus_rx(&self, slot: &str) -> tokio::sync::watch::Receiver<Option<Bytes>> {
        kv_subscribe(&self.ctx, format!("{}{}", consensus_ns::COMMITTED, slot))
    }

    /// Returns the **live** committed value for a consensus slot, or `None`.
    ///
    /// Lease-aware: when the slot was committed with
    /// [`ConsensusConfig::committed_lease_secs`] set, a value whose lease
    /// window has elapsed reads as `None` — the slot has reopened for
    /// re-proposal. Permanent commitments (the default) never expire.
    pub fn consensus_get(&self, slot: &str) -> Option<Bytes> {
        crate::consensus::live_committed_value(
            &self.ctx.kv_state, slot, crate::consensus::causal_now_ms(&self.ctx.hlc),
        )
    }

    /// Declares this node's quorum trust slice for `group` (SCP §3.1).
    ///
    /// Stored at `consensus/trust/{group}/{node_id}` and gossip-synced to all peers.
    ///
    /// With `ConsensusConfig::use_trust_slices` set, a proposer on this node counts **only**
    /// votes from the declared peers — the ballot loop in `consensus.rs` filters the tally on
    /// this set — which makes it a fixed *eligible* voter set. The quorum **size** is unchanged:
    /// simple majority over the observed roster, or `quorum_size`. Without the flag the
    /// declaration is stored and never consulted. Slice-based quorum *intersection* (SCP §3.1
    /// proper) is not implemented. (This comment used to say the protocol ignored slices
    /// entirely; the tally filter has existed alongside it — doc drift found 2026-09-26.)
    pub fn declare_trust(&self, group: &str, trusted_peers: &[NodeId]) {
        let key = format!("{}{}/{}", consensus_ns::TRUST, group, self.ctx.node_id);
        if let Ok(encoded) = mycelium_core::serde_fixint::to_vec(trusted_peers) {
            let _ = kv_set(&self.ctx, Arc::from(key.as_str()), Bytes::from(encoded));
        }
        let member_prefix = crate::signal::grp_prefix(group);
        let members: AHashSet<String> = kv_scan_prefix(&self.ctx, &member_prefix)
            .into_iter()
            .filter_map(|(k, _)| k.strip_prefix(&member_prefix).map(str::to_string))
            .collect();
        for peer in trusted_peers {
            if !members.contains(&peer.to_string()) {
                warn!(
                    group, peer = %peer,
                    "declare_trust: peer is not a current group member; \
                     use_trust_slices=true will time out waiting for their vote"
                );
            }
        }
    }

    /// Returns all declared trust slices for `group`, keyed by declaring node.
    pub fn group_trust(&self, group: &str) -> Vec<(NodeId, Vec<NodeId>)> {
        let prefix = format!("{}{}/", consensus_ns::TRUST, group);
        kv_scan_prefix(&self.ctx, &prefix)
            .into_iter()
            .filter_map(|(key, bytes)| {
                let node_str = key.strip_prefix(&prefix)?;
                let node_id: NodeId = node_str.parse().ok()?;
                let peers = mycelium_core::serde_fixint::from_slice::<Vec<NodeId>>(&bytes).ok()?;
                Some((node_id, peers))
            })
            .collect()
    }

    /// Returns the group member with the lowest observed load for `kind`.
    ///
    /// Iterates `grp/{group}/` for member NodeIds, then reads `load/{member}/{kind}`
    /// from Layer I for each. Members with no load entry are ranked lowest
    /// (transparent). Returns the lowest-load member, or `self.node_id` when the
    /// group is empty or no members have load data within `max_age`.
    ///
    /// `max_age` is used for pheromone evaporation — entries older than this are
    /// treated as transparent. Ties are broken deterministically by `id_hash()`.
    pub fn suggest_leader(&self, group: &str, kind: &str, max_age: Duration) -> NodeId {
        suggest_leader_ctx(&self.ctx, group, kind, max_age)
    }

    /// Proposes `value` for a named `slot` within a group.
    ///
    /// Blocks until quorum commits, another node commits first, or all ballot
    /// attempts are exhausted.
    ///
    /// Quorum defaults to `floor(N/2)+1` where N is the current group member
    /// count. Set `config.quorum_size > 0` to override.
    #[tracing::instrument(level = "debug", skip(self, value), fields(node = %self.ctx.node_id))]
    pub async fn group_propose(
        &self,
        group:  &str,
        slot:   &str,
        value:  Bytes,
        config: ConsensusConfig,
    ) -> ConsensusResult {
        let local_opacity = effective_opacity_ctx(&self.ctx, consensus_kind::PROPOSE);
        if local_opacity > 0.0 && config.ballot_retry_jitter_ms > 0 {
            let defer_ms = (local_opacity * config.ballot_retry_jitter_ms as f32 * 2.0) as u64;
            mycelium_core::sim_seam::sleep_ms("consensus/defer", defer_ms).await;
        }
        if config.use_suggest_leader && config.ballot_retry_jitter_ms > 0 {
            let suggested = suggest_leader_ctx(&self.ctx, group, consensus_kind::PROPOSE, self.signal_window());
            if suggested != self.ctx.node_id {
                mycelium_core::sim_seam::sleep_ms("consensus/suggest-defer", config.ballot_retry_jitter_ms).await;
            }
        }
        let roster_ttl = Duration::from_secs(self.ctx.config.health_check_interval_secs);
        let cached = cached_group_members_ctx(&self.ctx, group, roster_ttl);
        let member_ids: AHashSet<String> = cached.members
            .iter()
            .map(NodeId::to_string)
            .collect();
        let freshness = Duration::from_millis(super::opacity::opaque_freshness_ms(&self.ctx.config));
        // The electorate must be **established**, not inferred from absence. An empty roster used
        // to be counted as one member with a quorum of one, which this proposer's own self-vote
        // satisfied — so every node committed its own candidate unopposed. See
        // `helpers::resolve_electorate` and `ConsensusResult::ElectorateUnavailable`.
        let declared_min = declared_electorate_min(&self.ctx, group);
        let raw_members = match resolve_electorate(member_ids.len(), declared_min) {
            Electorate::Established(n) => n,
            Electorate::Unavailable { observed, declared_min } =>
                return ConsensusResult::ElectorateUnavailable {
                    slot:  Arc::from(slot),
                    group: Arc::from(group),
                    observed_members: observed,
                    declared_min,
                },
        };
        let active_members = if config.count_opaque_as_absent {
            let opaque_count = count_opaque_members_ctx(&self.ctx, &member_ids, freshness);
            raw_members.saturating_sub(opaque_count).max(1)
        } else {
            raw_members
        };
        let quorum = compute_quorum_size(config.quorum_size, active_members);
        let opaque_recompute = if config.count_opaque_as_absent {
            let kv_cb  = Arc::clone(&self.ctx.kv_state);
            let ids_cb = member_ids.clone();
            let freshness_ms = freshness.as_millis() as u64;
            let count_opaque: Arc<dyn Fn() -> usize + Send + Sync> = Arc::new(move || {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                count_opaque_members_in_kv(&kv_cb, &ids_cb, freshness_ms, now_ms)
            });
            Some(OpaqueRecompute { total_members: raw_members, config_quorum: config.quorum_size, count_opaque })
        } else {
            None
        };
        let topology_policy = self.ctx.config.topology_policies.get(group).cloned();
        make_consensus_engine_ctx(
            &self.ctx,
            config.abstain_when_opaque, config.use_trust_slices, config.max_abstain_ballots,
            topology_policy,
        )
            .propose(SignalScope::Group(Arc::from(group)), Arc::from(slot), value, quorum, config, opaque_recompute)
            .await
    }

    /// Proposes `value` for **cluster-wide** consensus (all known peers vote) — the consensus
    /// mirror of [`SignalScope::Cluster`](crate::SignalScope::Cluster). For a subset, use
    /// [`group_propose`](Self::group_propose).
    ///
    /// Quorum defaults to `floor(N/2)+1` where N is `peers + 1` (including self).
    /// Set `config.quorum_size > 0` to override.
    #[tracing::instrument(level = "debug", skip(self, value), fields(node = %self.ctx.node_id))]
    /// [`cluster_propose`](Self::cluster_propose), answering with a **receipt** instead of a
    /// four-variant result (contracts axis item 1 PR 2; `docs/design/contracts-receipts.md`).
    ///
    /// The difference that matters is `Timeout`: it becomes
    /// [`CommitError::DeliveryUnknown`](crate::CommitError::DeliveryUnknown), which says what is
    /// true — the value may or may not have committed elsewhere — rather than inviting the reading
    /// that nothing happened. On success the receipt carries the **local-sync** rung explicitly, so
    /// "the cluster agreed" and "this node has it on disk" are two separate statements instead of
    /// one collapsed `bool`.
    ///
    /// This is a **new verb**, not a new field on `ConsensusResult::Committed`: that variant's
    /// fields are not `#[non_exhaustive]`, so growing it would break every exhaustive destructure
    /// and construction (D24, corrected after review). The old verb and its `persisted: bool`
    /// remain; `persisted == true` still folds *on disk* together with *nothing was promised*,
    /// which is exactly what [`LocalDurability`](crate::LocalDurability) separates.
    pub async fn cluster_propose_receipt(
        &self,
        slot:   &str,
        value:  Bytes,
        config: ConsensusConfig,
    ) -> Result<crate::CommitReceipt, crate::CommitError> {
        receipt_from(self.cluster_propose(slot, value, config).await, &self.ctx)
    }

    /// [`group_propose`](Self::group_propose), answering with a receipt. See
    /// [`cluster_propose_receipt`](Self::cluster_propose_receipt).
    pub async fn group_propose_receipt(
        &self,
        group:  &str,
        slot:   &str,
        value:  Bytes,
        config: ConsensusConfig,
    ) -> Result<crate::CommitReceipt, crate::CommitError> {
        receipt_from(self.group_propose(group, slot, value, config).await, &self.ctx)
    }

    pub async fn cluster_propose(
        &self,
        slot:   &str,
        value:  Bytes,
        config: ConsensusConfig,
    ) -> ConsensusResult {
        let local_opacity = effective_opacity_ctx(&self.ctx, consensus_kind::PROPOSE);
        if local_opacity > 0.0 && config.ballot_retry_jitter_ms > 0 {
            let defer_ms = (local_opacity * config.ballot_retry_jitter_ms as f32 * 2.0) as u64;
            mycelium_core::sim_seam::sleep_ms("consensus/defer", defer_ms).await;
        }
        if config.use_suggest_leader && config.ballot_retry_jitter_ms > 0 {
            let my_fill = local_opacity;
            let is_lightest = peer_load_ctx(&self.ctx, self.signal_window())
                .iter()
                .filter(|(_, k, _)| k.as_ref() == consensus_kind::PROPOSE)
                .all(|(_, _, s)| s.fill_ratio >= my_fill);
            if !is_lightest {
                mycelium_core::sim_seam::sleep_ms("consensus/suggest-defer", config.ballot_retry_jitter_ms).await;
            }
        }
        let n_nodes = (self.ctx.peers.len() + 1).max(1);
        let freshness_ms = super::opacity::opaque_freshness_ms(&self.ctx.config);
        let active_n = if config.count_opaque_as_absent {
            let opaque_count = count_opaque_system_ctx(
                &self.ctx,
                Duration::from_millis(freshness_ms),
            );
            n_nodes.saturating_sub(opaque_count).max(1)
        } else {
            n_nodes
        };
        let quorum = compute_quorum_size(config.quorum_size, active_n);
        let opaque_recompute = if config.count_opaque_as_absent {
            let kv_cb = Arc::clone(&self.ctx.kv_state);
            let count_opaque: Arc<dyn Fn() -> usize + Send + Sync> = Arc::new(move || {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                count_opaque_all_in_kv(&kv_cb, freshness_ms, now_ms)
            });
            Some(OpaqueRecompute { total_members: n_nodes, config_quorum: config.quorum_size, count_opaque })
        } else {
            None
        };
        make_consensus_engine_ctx(
            &self.ctx,
            config.abstain_when_opaque, config.use_trust_slices, config.max_abstain_ballots,
            None,
        )
            .propose(SignalScope::Cluster, Arc::from(slot), value, quorum, config, opaque_recompute)
            .await
    }

    /// Deprecated alias for [`cluster_propose`](Self::cluster_propose) — renamed 2026-07-10 so the
    /// consensus method matches its scope (`SignalScope::Cluster`). Will be removed in a future
    /// release.
    #[deprecated(since = "2.1.0", note = "renamed to `cluster_propose` (matches SignalScope::Cluster)")]
    pub async fn system_propose(
        &self,
        slot:   &str,
        value:  Bytes,
        config: ConsensusConfig,
    ) -> ConsensusResult {
        self.cluster_propose(slot, value, config).await
    }

    /// Proposes `value` for `slot` requiring independent quorum from each group in `groups`.
    ///
    /// Commits only when **all** specified groups individually reach their configured quorum
    /// fraction.
    pub async fn cross_group_propose(
        &self,
        slot:   &str,
        value:  Bytes,
        groups: Vec<crate::GroupQuorum>,
        config: ConsensusConfig,
    ) -> ConsensusResult {
        make_consensus_engine_ctx(&self.ctx, false, false, 0, None)
            .cross_propose(Arc::from(slot), value, &groups, config)
            .await
    }

    /// Starts the consensus voter/listener task.
    ///
    /// Nodes that call this participate as voters in all consensus rounds.
    /// Nodes that do not call this still receive committed values via anti-entropy
    /// KV sync but their votes will not be counted.
    ///
    /// Returns a [`ConsensusListenerHandle`] whose drop stops the task. The task
    /// also exits on agent shutdown.
    pub fn start_consensus_listener(&self, config: ConsensusConfig) -> ConsensusListenerHandle {
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown_rx = self.ctx.shutdown_tx.subscribe();
        let engine = make_consensus_engine_ctx(
            &self.ctx,
            config.abstain_when_opaque,
            config.use_trust_slices,
            config.max_abstain_ballots,
            None,
        );
        // Register the voter's signal handlers *before* spawning so that a
        // PROPOSE or COMMIT arriving before the task's first poll is queued
        // rather than dropped — otherwise this node silently fails to vote on
        // proposals raced against listener startup.
        let rx_propose = self.ctx.signal_handlers.register_with_capacity(
            std::sync::Arc::from(consensus_kind::PROPOSE), 512,
        );
        let rx_commit = self.ctx.signal_handlers.register_with_capacity(
            std::sync::Arc::from(consensus_kind::COMMIT), 256,
        );
        self.ctx.spawn_task(crate::consensus::run_consensus_listener(
            engine, cancel_rx, shutdown_rx, rx_propose, rx_commit,
        ));
        ConsensusListenerHandle { _cancel: cancel_tx }
    }

    // ── Consistent overlay ───────────────────────────────────────────────────

    /// Consensus-durable write: runs a ballot-voting round before committing.
    ///
    /// Broadcasts a `Propose` message, waits for `floor(N/2)+1` peer votes, then
    /// writes the value to `consensus/committed/consistent/{key}` (durable, anti-entropy-
    /// synced to all nodes) and to the raw gossip KV key.
    ///
    /// **Guarantee: single-decree agreement on a stable roster, not linearizability.** Since 2.30.0
    /// a proposer asks a quorum what it has accepted before proposing (the prepare phase), so two
    /// concurrent callers do not both return `Ok(())` for different values when their quorums
    /// intersect: the second adopts the first's value, and is told `Superseded`. Across an electorate
    /// change, or against proposers older than 2.30.0, that is not guaranteed — guide 04
    /// § *Changing an electorate*. `consistent_get` is a local read and may lag the cluster-wide
    /// committed value by up to one anti-entropy round.
    ///
    /// **Suitable for:** leader election, distributed locks, single-writer coordinator
    /// patterns where "only one writer should commit first" is sufficient. Use ballot-based
    /// fencing tokens (see `distributed_lock`) to protect downstream consumers from
    /// lower-ballot writers.
    ///
    /// **Not suitable for:** read-after-write guarantees without polling. After calling
    /// `consistent_set`, callers on other nodes should poll `consistent_get` until the
    /// expected value appears (usually within one gossip round).
    ///
    /// Use [`KvHandle::set`](crate::KvHandle::set) for ordinary eventually-consistent writes.
    #[tracing::instrument(level = "debug", skip(self, key, value), fields(node = %self.ctx.node_id))]
    pub async fn consistent_set(
        &self,
        key:   impl Into<Arc<str>>,
        value: impl Into<Bytes>,
    ) -> Result<(), ConsistencyError> {
        let key: Arc<str> = key.into();
        let value: Bytes   = value.into();
        let slot = format!("consistent/{key}");

        match self.cluster_propose(&slot, value.clone(), ConsensusConfig::default()).await {
            ConsensusResult::Committed { .. } => {
                kv_set(&self.ctx, key, value);
                Ok(())
            }
            ConsensusResult::Timeout { ballots_tried, .. } =>
                Err(ConsistencyError::Timeout { ballots_tried }),
            ConsensusResult::Superseded { .. } =>
                Err(ConsistencyError::Superseded),
            ConsensusResult::TopologyUnsatisfied { .. } =>
                Err(ConsistencyError::TopologyUnsatisfied),
            // Cluster scope has no roster to be empty, so this is unreachable today — but the arm
            // fails closed rather than falling through to success, which is the whole reason the
            // enum became `#[non_exhaustive]`.
            ConsensusResult::ElectorateUnavailable { observed_members, declared_min, .. } =>
                Err(ConsistencyError::ElectorateUnavailable { observed_members, declared_min }),
            ConsensusResult::NotAMember { group, .. } => Err(ConsistencyError::NotAMember { group }),
        }
    }

    /// Read the latest ballot-committed value for `key` visible to this node.
    ///
    /// Checks `consensus/committed/consistent/{key}` first (written on quorum commit
    /// and anti-entropy-synced to all nodes); falls back to the raw gossip KV key.
    ///
    /// **Not a read quorum.** Returns whatever has anti-entropy-propagated to this node,
    /// which may lag the cluster-wide committed value by up to one gossip round.
    /// The staleness bound is `GossipConfig::anti_entropy_interval_secs` (default 30 s);
    /// on a healthy cluster, propagation typically completes in well under one second.
    /// For read-after-write guarantees, poll until the expected value appears.
    pub fn consistent_get(&self, key: &str) -> Option<Bytes> {
        crate::consensus::live_committed_value(
            &self.ctx.kv_state, &format!("consistent/{key}"), crate::consensus::causal_now_ms(&self.ctx.hlc),
        )
            .or_else(|| kv_get(&self.ctx, key))
    }

    /// The [distributed lock **service**](super::LockService) — blocking acquire, scoped critical
    /// sections, and the when-to-use guidance — over this handle's [`distributed_lock`](Self::distributed_lock).
    ///
    /// `distributed_lock` is the raw try-lock; `locks()` is the ergonomic layer most callers want.
    pub fn locks(&self) -> super::LockService {
        super::LockService { ctx: std::sync::Arc::clone(&self.ctx) }
    }

    /// Acquire a named distributed lock via cluster consensus.
    ///
    /// A **leased, mutually-exclusive** lock: exactly one holder cluster-wide until it releases
    /// (drop / [`release`](LockGuard::release)) or `ttl` elapses (the lock auto-expires — the
    /// commit carries a consensus lease). The returned [`LockGuard::token`] is a monotonic fencing
    /// token for resource-side checks.
    ///
    /// Coarse-grained by design (a consensus round per acquire, ~1 s to let the commit converge) —
    /// suited to leader election, shard/config ownership, not high-rate fine-grained locking.
    ///
    /// Returns [`ConsistencyError::Superseded`] if another holder won the lock (or a live lease is
    /// held elsewhere). #164: the pre-2026-07-10 implementation returned on the *local* optimistic
    /// commit (no mutual exclusion under a race) and its release tombstoned the wrong key (locks
    /// were permanently unreleasable) — both fixed here via the converged-holder discipline (#151).
    #[tracing::instrument(level = "debug", skip(self), fields(node = %self.ctx.node_id))]
    pub async fn distributed_lock(
        &self,
        name: &str,
        ttl:  Duration,
    ) -> Result<LockGuard, ConsistencyError> {
        let slot = format!("lock/{name}");
        // Value = `{holder}:{nonce}` (#164). Expiry is the consensus commit-*lease* below, not a
        // JSON `expires_ms` field: the old field was never enforced (the lock never expired). The
        // per-acquire nonce makes each guard's value unique, so release can tell this acquisition
        // apart from any later one (even the same node re-acquiring after its lease lapsed).
        let value = Bytes::from(
            format!("{}:{:016x}", self.ctx.node_id, fastrand::u64(..)).into_bytes(),
        );
        let cfg = ConsensusConfig {
            committed_lease_secs: Some(ttl.as_secs().max(1)),
            ..ConsensusConfig::default()
        };

        match self.cluster_propose(&slot, value.clone(), cfg).await {
            ConsensusResult::Committed { ballot, .. } => {
                // #164 bug A: two proposers can both *optimistically* commit against their own
                // local view — the propose return is NOT mutually exclusive. Commit-keys are
                // LWW-resolved by HLC, so let the winning commit converge, then read the
                // authoritative converged value; only the node whose value survived holds the
                // lock. Losers get `Superseded` and never receive a guard.
                //
                // The duration of this wait is a correctness assumption (replay inventory §2.3),
                // so it goes through the timer seam: a replay can run it at 0, exactly 1 s, or
                // longer, and the D4 audit's model of this path can become a replay of it.
                mycelium_core::sim_seam::sleep_ms("lock/converge", 1000).await;
                match crate::consensus::live_committed_with_hlc(
                        &self.ctx.kv_state, &slot, crate::consensus::causal_now_ms(&self.ctx.hlc)) {
                    // Fencing token is the commit's HLC, not the ballot: the HLC is monotonic
                    // across successive holders (each observes the prior release), so a resource
                    // that rejects a lower token is actually fenced. The ballot regresses under
                    // gossip lag and is unsafe for fencing (#164 example finding).
                    Some((converged, hlc)) if converged.as_ref() == value.as_ref() =>
                        Ok(LockGuard {
                            ctx:      Arc::clone(&self.ctx),
                            name:     Arc::from(name),
                            value,
                            ballot,
                            token:    hlc,
                            released: false,
                        }),
                    // Not the converged holder (or nothing converged): no guard. This `_` covers an
                    // `Option`, not `ConsensusResult` — it is a legitimate catch-all.
                    _ => Err(ConsistencyError::Superseded),
                }
            }
            ConsensusResult::Timeout { ballots_tried, .. } =>
                Err(ConsistencyError::Timeout { ballots_tried }),
            ConsensusResult::Superseded { .. } =>
                Err(ConsistencyError::Superseded),
            ConsensusResult::TopologyUnsatisfied { .. } =>
                Err(ConsistencyError::TopologyUnsatisfied),
            // Nothing was proposed. Reading the slot here would hand back a *stale* winner from an
            // earlier, differently-constituted election — the failure mode this whole change is
            // about, one level down.
            ConsensusResult::ElectorateUnavailable { observed_members, declared_min, .. } =>
                Err(ConsistencyError::ElectorateUnavailable { observed_members, declared_min }),
            ConsensusResult::NotAMember { group, .. } => Err(ConsistencyError::NotAMember { group }),
        }
    }

    /// Elect a leader for `group` via consensus.
    ///
    /// If this node wins, returns its own `NodeId`. If another node committed first,
    /// reads the winner from the committed KV slot and returns it.
    ///
    /// **Leased** (since 2.32.0, row A C1): the leadership lapses
    /// [`DEFAULT_LEADER_LEASE`](crate::DEFAULT_LEADER_LEASE) (30 s) after its commit unless the
    /// leader calls again — re-electing while live renews it — so a dead leader is not reported for
    /// ever. Step down with [`release_leadership`](Self::release_leadership); ask for a different
    /// term, or for permanence, with [`elect_leader_with`](Self::elect_leader_with).
    pub async fn elect_leader(&self, group: &str) -> Result<NodeId, ConsistencyError> {
        self.elect_leader_receipt(group).await.map(|l| l.leader)
    }

    /// Elect a leader for `group`, returning a [`Leadership`] that **names the rung it reached**
    /// and carries a fencing token.
    ///
    /// Prefer this over [`elect_leader`](Self::elect_leader), which returns a bare `NodeId` and so
    /// cannot distinguish *"a quorum chose me"* ([`LeadershipBasis::Decided`]) from *"this is what
    /// my replica currently says"* ([`LeadershipBasis::Observed`]). That distinction decides
    /// whether two callers can act as leader at once, which is exactly the thing a caller wanted to
    /// know and the old signature could not express.
    ///
    /// ## What a success means, and what it does not
    ///
    /// `Decided` is the strongest rung the protocol offers: a quorum of the electorate voted for
    /// this value at this ballot, **bound to it by digest**, and no other value can have been
    /// committed at that ballot. `Observed` means only that this node's replica holds a converged
    /// value — sound for *following* a leader, and not evidence the cluster agrees right now.
    ///
    /// **Neither rung is an exclusive grant that stays true.** Leadership can be superseded at any
    /// later ballot, and no coordinator-free protocol can promise otherwise; the honest instrument
    /// for exclusivity is to **fence on [`Leadership::epoch`] at the resource**, which is monotonic
    /// across successive holders. LWW can decide which *record* survives; it cannot undo work two
    /// callers each performed after being told they had won.
    ///
    /// ## Convergence is observed, not assumed
    ///
    /// This used to sleep a fixed second and then read the local slot, which the replay inventory
    /// already flagged as *"the one whose duration is a correctness assumption"*. It now **polls**
    /// until the slot holds a value, bounded by the same budget — so a fast cluster answers
    /// immediately, a slow one still gets its second, and the answer is a value that was actually
    /// there rather than one a timer hoped for. Lengthening a sleep only changes how often the
    /// difference is visible.
    ///
    /// ## How long it holds
    ///
    /// Leased for [`DEFAULT_LEADER_LEASE`](crate::DEFAULT_LEADER_LEASE) — see
    /// [`elect_leader`](Self::elect_leader) and [`elect_leader_with`](Self::elect_leader_with).
    pub async fn elect_leader_receipt(
        &self,
        group: &str,
    ) -> Result<crate::agent::overlay_consistent::Leadership, ConsistencyError> {
        self.elect_leader_with(group, crate::agent::overlay_consistent::LeaderTerm::default()).await
    }

    /// [`elect_leader_receipt`](Self::elect_leader_receipt) with an explicit
    /// [`LeaderTerm`](crate::LeaderTerm): a lease of your choosing, or — by explicit opt-in — a
    /// permanent leadership, the pre-2.32.0 behaviour (row A, C1).
    ///
    /// A leased leadership is renewed by calling again while it is live (the same value re-commits
    /// and refreshes the lease); call every `lease / 3` or so. Once it lapses — the leader died, or
    /// stopped renewing — the slot reopens and the next election decides afresh, setting the lapsed
    /// leader's acceptance aside. A permanent leadership never lapses: if its leader dies it is
    /// reported until someone [releases](Self::release_leadership) it, which only the leader can.
    pub async fn elect_leader_with(
        &self,
        group: &str,
        term:  crate::agent::overlay_consistent::LeaderTerm,
    ) -> Result<crate::agent::overlay_consistent::Leadership, ConsistencyError> {
        use crate::agent::overlay_consistent::{Leadership, LeadershipBasis};

        let slot  = format!("leader/{group}");
        let value = Bytes::from(self.ctx.node_id.to_string().into_bytes());

        // Read the AUTHORITATIVE leader from the converged slot — never assume "I committed → I
        // won". Carries the commit's HLC, which is the fencing token.
        let read_slot = |this: &Self| -> Option<(NodeId, u64)> {
            let (raw, hlc) = crate::consensus::live_committed_with_hlc(
                &this.ctx.kv_state, &slot, crate::consensus::causal_now_ms(&this.ctx.hlc))?;
            let id = std::str::from_utf8(&raw).ok()?.parse::<NodeId>().ok()?;
            Some((id, hlc))
        };

        let cfg = ConsensusConfig { committed_lease_secs: term.lease_secs(), ..ConsensusConfig::default() };
        match self.group_propose(group, &slot, value.clone(), cfg).await {
            ConsensusResult::Committed { .. } => {
                // #164 class: an optimistic `Committed` is NOT mutually exclusive on its own — the
                // binding and one-vote-per-ballot rules are what make it decisive, and the
                // converged slot is still the authority on *which* value survived. Poll for it
                // rather than sleeping a fixed second and hoping.
                let decided = self.await_converged_slot(&read_slot).await;
                match decided {
                    // Our value survived: a quorum chose it, so this is the strongest rung.
                    Some((leader, epoch)) if leader.to_string().as_bytes() == value.as_ref() =>
                        Ok(Leadership { leader, epoch, basis: LeadershipBasis::Decided }),
                    // Someone else's did: we are reporting what we see, not what we decided.
                    Some((leader, epoch)) =>
                        Ok(Leadership { leader, epoch, basis: LeadershipBasis::Observed }),
                    None => Err(ConsistencyError::Superseded),
                }
            }
            ConsensusResult::Superseded { .. } =>
                read_slot(self)
                    .map(|(leader, epoch)| Leadership { leader, epoch, basis: LeadershipBasis::Observed })
                    .ok_or(ConsistencyError::Superseded),
            ConsensusResult::Timeout { ballots_tried, .. } =>
                Err(ConsistencyError::Timeout { ballots_tried }),
            ConsensusResult::TopologyUnsatisfied { .. } =>
                Err(ConsistencyError::TopologyUnsatisfied),
            // Nothing was proposed. Reading the slot here would hand back a *stale* winner from an
            // earlier, differently-constituted election — the failure mode this whole change is
            // about, one level down.
            ConsensusResult::ElectorateUnavailable { observed_members, declared_min, .. } =>
                Err(ConsistencyError::ElectorateUnavailable { observed_members, declared_min }),
            ConsensusResult::NotAMember { group, .. } => Err(ConsistencyError::NotAMember { group }),
        }
    }

    /// **Step down**: release this node's leadership of `group` (row A, C1).
    ///
    /// Writes the leadership slot's lifecycle record as *released* at the ballot it was decided at,
    /// so every node reads the group as leaderless, the next election sets this node's acceptance
    /// aside rather than re-electing it, and a late COMMIT of it is stale. Works for a leased and a
    /// permanent leadership alike. Returns `false` — and writes nothing — when the live leader this
    /// node sees is not itself.
    pub fn release_leadership(&self, group: &str) -> bool {
        let value = Bytes::from(self.ctx.node_id.to_string().into_bytes());
        crate::consensus::release_decision(&self.ctx, &format!("leader/{group}"), &value, 0)
    }

    /// Collect acceptor state whose decision is over — what each consensus listener does on its
    /// collection tick (`ConsensusEngine::collect_finished`, row A C2). For tests.
    #[cfg(test)]
    pub(crate) async fn collect_finished_acceptor_state(&self) -> usize {
        make_consensus_engine_ctx(&self.ctx, false, false, 0, None).collect_finished().await
    }

    /// Poll for the committed slot to hold a value, bounded by the convergence budget.
    ///
    /// Replaces `sleep(1s); read`. The budget is unchanged — what changes is that the answer is a
    /// value that was **observed to be there**, at whatever moment it arrived, rather than whatever
    /// happened to be present when a timer fired. A fast cluster answers in milliseconds; a slow
    /// one still gets its full second before giving up.
    async fn await_converged_slot(
        &self,
        read: &impl Fn(&Self) -> Option<(NodeId, u64)>,
    ) -> Option<(NodeId, u64)> {
        const BUDGET_MS: u64 = 1000;
        const STEP_MS:   u64 = 25;
        let mut waited = 0;
        loop {
            if let Some(found) = read(self) { return Some(found); }
            if waited >= BUDGET_MS { return None; }
            // Through the timer seam, so a replay can run this at 0, at the step, or longer —
            // the same treatment the fixed sleep had (`elect/converge`).
            mycelium_core::sim_seam::sleep_ms("elect/converge", STEP_MS).await;
            waited += STEP_MS;
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use super::ConsistencyError;

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    async fn make_agent(port: u16, peers: &[u16]) -> GossipAgent {
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let cfg = GossipConfig {
            bind_address:    "127.0.0.1".parse().unwrap(),
            bind_port:       port,
            bootstrap_peers: peers.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect(),
            ..GossipConfig::default()
        };
        let a = GossipAgent::new(id, cfg);
        a.start().await.unwrap();
        a
    }

    #[tokio::test]
    async fn test_consistent_set_single_node_succeeds() {
        let a = make_agent(alloc_port(), &[]).await;
        let r = a.consensus().consistent_set("cfg/solo", Bytes::from_static(b"ok")).await;
        assert!(r.is_ok(), "single-node consistent_set should succeed: {r:?}");
        assert_eq!(a.consensus().consistent_get("cfg/solo").as_deref(), Some(b"ok".as_slice()));
        a.shutdown().await;
    }

    #[tokio::test]
    async fn test_consistent_set_timeout_unreachable_quorum() {
        use crate::consensus::ConsensusConfig;
        let p   = alloc_port();
        let id  = NodeId::new("127.0.0.1", p).unwrap();
        let cfg = GossipConfig {
            bind_address: "127.0.0.1".parse().unwrap(),
            bind_port:    p,
            ..GossipConfig::default()
        };
        let a = GossipAgent::new(id, cfg);
        a.start().await.unwrap();

        let custom = ConsensusConfig { quorum_size: 2, max_ballots: 1, ..ConsensusConfig::default() };
        match a.consensus().cluster_propose("test/slot", Bytes::from_static(b"x"), custom).await {
            crate::consensus::ConsensusResult::Timeout { .. } => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
        a.shutdown().await;
    }

    #[allow(dead_code)]
    fn _assert_consistency_error_variants() {
        let _ = ConsistencyError::Superseded;
        let _ = ConsistencyError::TopologyUnsatisfied;
    }

    /// **A partition is labelled `no_voters`, through `propose` itself** (the review of #579). Since 2.30.0
    /// every attempt starts with a prepare phase, so a proposer nobody answers ends there; labelling by
    /// phase called that `promise_short`, and the runbook sends `promise_short` to the upgrade. Here one
    /// node demands a quorum of two, so no other acceptor can answer — and the label must say so.
    #[cfg(feature = "metrics")]
    #[tokio::test(flavor = "current_thread")]
    async fn a_proposer_nobody_answers_times_out_as_no_voters() {
        use crate::consensus::{ConsensusConfig, ConsensusResult};
        use std::sync::{Arc as A, Mutex};

        // Records the label set of every counter registered on this thread.
        #[derive(Default)]
        struct Labels(A<Mutex<Vec<String>>>);
        impl metrics::Recorder for Labels {
            fn describe_counter(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
            fn describe_gauge(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
            fn describe_histogram(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
            fn register_counter(&self, key: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
                if key.name() == "mycelium_consensus_timeouts_total" {
                    let reason = key.labels().find(|l| l.key() == "reason").map(|l| l.value().to_string());
                    self.0.lock().unwrap().push(reason.unwrap_or_default());
                }
                metrics::Counter::noop()
            }
            fn register_gauge(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge { metrics::Gauge::noop() }
            fn register_histogram(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Histogram { metrics::Histogram::noop() }
        }
        let seen = A::new(Mutex::new(Vec::new()));
        let recorder = Labels(A::clone(&seen));
        let _guard = metrics::set_default_local_recorder(&recorder);

        let a = make_agent(alloc_port(), &[]).await;
        let config = ConsensusConfig {
            quorum_size: 2,
            max_ballots: 1,
            phase1_timeout: std::time::Duration::from_millis(200),
            ballot_retry_jitter_ms: 0,
            ..ConsensusConfig::default()
        };
        match a.consensus().cluster_propose("partition/slot", Bytes::from_static(b"v"), config).await {
            ConsensusResult::Timeout { .. } => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
        assert_eq!(*seen.lock().unwrap(), vec!["no_voters".to_string()],
                   "nobody else answered the prepare — a partition, not a promise shortfall");
        a.shutdown().await;
    }

    #[tokio::test]
    async fn test_leased_commit_expires_and_reopens() {
        use crate::consensus::{ConsensusConfig, ConsensusResult};
        let a = make_agent(alloc_port(), &[]).await;
        let c = a.consensus();

        // Commit with a 0-second lease: expires as soon as the wall clock moves.
        let leased = ConsensusConfig { committed_lease_secs: Some(0), ..ConsensusConfig::default() };
        match c.cluster_propose("lease/slot", Bytes::from_static(b"v1"), leased).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed, got {other:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(c.consensus_get("lease/slot"), None, "expired lease must read as absent");

        // The slot has reopened: a different value commits (no Superseded), and
        // committing without a lease clears the stale lease entry → permanent.
        match c.cluster_propose("lease/slot", Bytes::from_static(b"v2"), ConsensusConfig::default()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed on reopened slot, got {other:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            c.consensus_get("lease/slot").as_deref(), Some(b"v2".as_slice()),
            "permanent re-commit must not be expired by the stale lease entry",
        );
        a.shutdown().await;
    }

    #[tokio::test]
    async fn test_leased_commit_renewal_and_supersession() {
        use crate::consensus::{ConsensusConfig, ConsensusResult};
        let a = make_agent(alloc_port(), &[]).await;
        let c = a.consensus();

        let leased = ConsensusConfig { committed_lease_secs: Some(3600), ..ConsensusConfig::default() };
        match c.cluster_propose("lease/renew", Bytes::from_static(b"leader-a"), leased.clone()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed, got {other:?}"),
        }
        // Same value while the lease is live: renewal — allowed.
        match c.cluster_propose("lease/renew", Bytes::from_static(b"leader-a"), leased.clone()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed (renewal), got {other:?}"),
        }
        // Different value while the lease is live: superseded, value unchanged.
        match c.cluster_propose("lease/renew", Bytes::from_static(b"leader-b"), leased).await {
            ConsensusResult::Superseded { .. } => {}
            other => panic!("expected Superseded while lease live, got {other:?}"),
        }
        assert_eq!(c.consensus_get("lease/renew").as_deref(), Some(b"leader-a".as_slice()));

        // Without a lease there is no renewal: even the same value is Superseded.
        match c.cluster_propose("perm/slot", Bytes::from_static(b"x"), crate::consensus::ConsensusConfig::default()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed, got {other:?}"),
        }
        match c.cluster_propose("perm/slot", Bytes::from_static(b"x"), crate::consensus::ConsensusConfig::default()).await {
            ConsensusResult::Superseded { .. } => {}
            other => panic!("permanent slots must stay commit-once, got {other:?}"),
        }
        a.shutdown().await;
    }

    /// Emit `msg` as a COMMIT until `done` holds, within 15 s. A Cluster signal's local delivery is shed with probability
    /// equal to the kind's queue fill (`ops::deliver_locally`), so one emit right after a consensus round — while the
    /// round's own frames still fill the queues — is not a delivery (#568).
    async fn emit_commit_until(a: &crate::GossipAgent, msg: &crate::consensus::ConsensusMsg, done: impl Fn() -> bool) -> bool {
        use crate::consensus::{consensus_kind, encode_consensus_msg};
        use crate::signal::SignalScope;
        for _ in 0..300 {
            if done() {
                return true;
            }
            let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, encode_consensus_msg(msg));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        done()
    }

    /// The tripwire on a node whose COMMIT queue is held `fill` full by a subscriber that never reads (0.0 = none).
    async fn tripwire_case(fill_to: f32) {
        use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusConfig, ConsensusMsg, ConsensusResult};
        use crate::signal::SignalScope;
        use std::sync::Arc;

        let a = make_agent(alloc_port(), &[]).await;
        let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());

        match a.consensus().cluster_propose("trip/slot", Bytes::from_static(b"genuine"), ConsensusConfig::default()).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed, got {other:?}"),
        }
        assert_eq!(a.system_stats().commit_conflicts, 0);

        // Load: a COMMIT subscriber that never drains, filled to `fill_to` with junk for a slot nobody holds.
        let kind: Arc<str> = Arc::from(consensus_kind::COMMIT);
        let _stalled = (fill_to > 0.0).then(|| a.task_ctx.signal_handlers.register_with_capacity(Arc::clone(&kind), 1000));
        let junk = ConsensusMsg::Commit { slot: Arc::from("stall/junk"), ballot: 1, value: Bytes::from_static(b"j") };
        while a.task_ctx.signal_handlers.fill_ratio(&kind) < fill_to {
            let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, encode_consensus_msg(&junk));
        }

        // A forged COMMIT carrying a different value for the live slot: the tripwire fires and does not endorse it.
        // Local emits self-deliver; under load one emit may be shed, so it is repeated until the tripwire sees it.
        let forged = ConsensusMsg::Commit { slot: Arc::from("trip/slot"), ballot: 42, value: Bytes::from_static(b"clobber") };
        let fired = emit_commit_until(&a, &forged, || a.system_stats().commit_conflicts >= 1).await;
        assert!(fired, "tripwire did not fire on conflicting COMMIT (fill {fill_to})");
        assert_eq!(
            a.consensus().consensus_get("trip/slot").as_deref(), Some(b"genuine".as_slice()),
            "conflicting COMMIT must not be endorsed",
        );

        // An idempotent re-COMMIT of the same value is legal and must not trip — checked once it has been seen
        // delivered, so a shed emit cannot pass this vacuously. The baseline is taken once the count is stable: a second
        // forged frame delivered before the tripwire was seen may still be queued (#569's review).
        let mut conflicts = a.system_stats().commit_conflicts;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let now = a.system_stats().commit_conflicts;
            if now == conflicts {
                break;
            }
            conflicts = now;
        }
        let idempotent = ConsensusMsg::Commit { slot: Arc::from("trip/slot"), ballot: 43, value: Bytes::from_static(b"genuine") };
        let body = encode_consensus_msg(&idempotent);
        // A second subscriber sees what the listener sees (one fan-out, `deliver`), so seeing it here means delivered.
        let mut watch = a.task_ctx.signal_handlers.register_with_capacity(Arc::clone(&kind), 256);
        let mut seen = false;
        for _ in 0..300 {
            let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, body.clone());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            while let Ok(sig) = watch.try_recv() {
                seen |= sig.payload == body;
            }
            if seen {
                break;
            }
        }
        assert!(seen, "the idempotent COMMIT was never delivered (fill {fill_to})");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(a.system_stats().commit_conflicts, conflicts, "same-value COMMIT must not count as a conflict");
        a.shutdown().await;
    }

    /// Delivers `msg` to this node's consensus listener and returns once the listener has
    /// **processed** it: a second COMMIT subscriber sees what the listener sees (one fan-out), and
    /// a probe COMMIT emitted afterwards is applied only after `msg` (the listener drains its queue
    /// in order) — so the probe's committed key appearing is the structural proof, no fixed sleep.
    async fn deliver_commit_processed(a: &crate::GossipAgent, msg: &crate::consensus::ConsensusMsg, probe: &str) {
        use crate::consensus::{consensus_kind, encode_consensus_msg, ConsensusMsg};
        use crate::signal::SignalScope;
        use std::sync::Arc;
        let kind: Arc<str> = Arc::from(consensus_kind::COMMIT);
        let body = encode_consensus_msg(msg);
        let mut watch = a.task_ctx.signal_handlers.register_with_capacity(Arc::clone(&kind), 256);
        let mut seen = false;
        for _ in 0..300 {
            let _ = a.mesh().emit(consensus_kind::COMMIT, SignalScope::Cluster, body.clone());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            while let Ok(sig) = watch.try_recv() { seen |= sig.payload == body; }
            if seen { break; }
        }
        assert!(seen, "the late COMMIT was never delivered");
        let probe_msg = ConsensusMsg::Commit { slot: Arc::from(probe), ballot: 1, value: Bytes::from_static(b"probe") };
        let probe_key = format!("consensus/committed/{probe}");
        let applied = emit_commit_until(a, &probe_msg, || {
            a.task_ctx.kv_state.store.pin().get(probe_key.as_str()).is_some_and(|e| e.data.is_some())
        }).await;
        assert!(applied, "the probe COMMIT behind the late one was never applied");
    }

    fn decided_of(a: &crate::GossipAgent, slot: &str) -> u64 {
        a.task_ctx.kv_state.store.pin()
            .get(format!("consensus/decided/{slot}").as_str())
            .and_then(|e| e.data.clone())
            .map(|b| crate::consensus::decode_ballot(&b))
            .unwrap_or(0)
    }

    /// **Row A, K2: a reopened lease does not adopt the expired holder's value before the decided
    /// floor arrives.** The commit writes `committed`, then the lease, then `decided`, and each
    /// travels as its own key. A node holding the expired commit but not yet its floor saw the
    /// decision as *over* at the old floor, so the expired holder's acceptance survived the
    /// set-aside, was adopted, and was committed again with a fresh lease.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reopened_lease_does_not_adopt_the_expired_value_before_its_floor_arrives() {
        use crate::consensus::{ConsensusConfig, ConsensusResult};
        // No listener: the proposer's own COMMIT is then not re-applied here, so the floor this
        // test removes stays removed — the state of a node whose `decided` copy has not arrived.
        let a = make_agent(alloc_port(), &[]).await;
        let c = a.consensus();
        let leased = |secs| ConsensusConfig { committed_lease_secs: Some(secs), ..ConsensusConfig::default() };
        match c.cluster_propose("k2/slot", Bytes::from_static(b"expired-holder"), leased(1)).await {
            ConsensusResult::Committed { .. } => {}
            other => panic!("expected Committed, got {other:?}"),
        }
        // This node's copy of the slot's floor has not arrived.
        a.task_ctx.kv_state.store.pin().remove("consensus/decided/k2/slot");
        for _ in 0..100 {
            if c.consensus_get("k2/slot").is_none() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(c.consensus_get("k2/slot"), None, "the 1 s lease lapsed");
        match c.cluster_propose("k2/slot", Bytes::from_static(b"new-holder"), leased(60)).await {
            ConsensusResult::Committed { value, .. } => assert_eq!(value.as_ref(), b"new-holder"),
            other => panic!("the reopened slot adopted the expired holder's value: {other:?}"),
        }
        assert_eq!(c.consensus_get("k2/slot").as_deref(), Some(b"new-holder".as_slice()));
        a.shutdown().await;
    }

    /// **Row A, K3: a late COMMIT does not re-stamp a released lock.** A learner holding the floor
    /// but not the entry, or the entry but not the floor, re-wrote `committed` with its own HLC —
    /// which wins LWW over the release tombstone fleet-wide and hands the lock back to its old
    /// holder, permanently once the lease tombstone is collected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_late_commit_does_not_restamp_a_released_lock() {
        use crate::consensus::{ConsensusConfig, ConsensusMsg};
        use std::sync::Arc;
        let a = make_agent(alloc_port(), &[]).await;
        let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
        let ttl = std::time::Duration::from_secs(60);

        // (1) The learner holds the floor but not the entry.
        let g = a.consensus().distributed_lock("k3a", ttl).await.expect("acquire");
        let (value, ballot) = (g.value.clone(), decided_of(&a, "lock/k3a"));
        assert!(ballot > 0);
        g.release();
        crate::store::sweep_stale_tombstones(&a.task_ctx.kv_state.store, u64::MAX);
        a.task_ctx.kv_state.store.pin().remove("consensus/committed/lock/k3a");
        let late = ConsensusMsg::Commit { slot: Arc::from("lock/k3a"), ballot, value };
        deliver_commit_processed(&a, &late, "probe/k3a").await;
        assert_eq!(a.consensus().consensus_get("lock/k3a"), None,
            "a late COMMIT at the released ballot resurrected the lock (floor held, entry not)");

        // (2) The learner holds the entry but not the floor.
        let g = a.consensus().distributed_lock("k3b", ttl).await.expect("acquire");
        let (value, ballot) = (g.value.clone(), decided_of(&a, "lock/k3b"));
        g.release();
        a.task_ctx.kv_state.store.pin().remove("consensus/decided/lock/k3b");
        let late = ConsensusMsg::Commit { slot: Arc::from("lock/k3b"), ballot, value };
        deliver_commit_processed(&a, &late, "probe/k3b").await;
        assert_eq!(a.consensus().consensus_get("lock/k3b"), None,
            "a late COMMIT at the released ballot resurrected the lock (entry held, floor not)");
        a.shutdown().await;
    }

    /// **Row A, C1: `elect_leader` is leased by default.** It committed permanently, so a leader
    /// that died was reported for ever — a role that escapes evaporation.
    #[tokio::test]
    async fn elect_leader_is_leased_by_default() {
        use crate::consensus::ConsensusConfig;
        let a = make_agent(alloc_port(), &[]).await;
        let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
        a.mesh().join_group("c1");
        let l = a.consensus().elect_leader_receipt("c1").await.expect("elects");
        assert_eq!(l.leader, *a.node_id());
        let lease = a.task_ctx.kv_state.store.pin()
            .get("consensus/lease/leader/c1").and_then(|e| e.data.clone())
            .and_then(|b| crate::consensus::decode_lease_ms(&b));
        assert_eq!(lease, Some(super::super::overlay_consistent::DEFAULT_LEADER_LEASE.as_millis() as u64),
            "the election carries the default lease");
        // The release path: the leader steps down and the slot reads as empty.
        assert!(a.consensus().release_leadership("c1"), "the leader may release");
        assert_eq!(a.consensus().consensus_get("leader/c1"), None, "released leadership reads as no leader");
        // Permanence is still available, by asking for it.
        a.mesh().join_group("c1-perm");
        a.consensus().elect_leader_with("c1-perm", super::super::overlay_consistent::LeaderTerm::Permanent)
            .await.expect("elects permanently");
        assert!(a.task_ctx.kv_state.store.pin().get("consensus/lease/leader/c1-perm")
            .is_none_or(|e| e.data.is_none()), "a permanent election carries no lease");
        a.shutdown().await;
    }

    /// **Row A, C1: a dead leader's lease lapses and a new election succeeds.** Three members;
    /// the leader is elected on a short lease and shut down; a survivor elects itself once the
    /// lease lapses. Before, the permanent commit named the dead node for ever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_dead_leaders_lease_lapses_and_a_new_election_succeeds() {
        use crate::consensus::ConsensusConfig;
        use super::super::overlay_consistent::{LeaderTerm, LeadershipBasis};
        let (p1, p2, p3) = (alloc_port(), alloc_port(), alloc_port());
        let a = make_agent(p1, &[p2, p3]).await;
        let b = make_agent(p2, &[p1, p3]).await;
        let c = make_agent(p3, &[p1, p2]).await;
        let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
        let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
        let _lc = c.consensus().start_consensus_listener(ConsensusConfig::default());
        for n in [&a, &b, &c] { n.mesh().join_group("c1dead"); }
        let mut ready = false;
        for _ in 0..200 {
            ready = [&a, &b, &c].iter().all(|n| n.mesh().group_members("c1dead").len() >= 3);
            if ready { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "the group's roster did not converge on three members");

        let l = a.consensus()
            .elect_leader_with("c1dead", LeaderTerm::Lease(std::time::Duration::from_secs(2))).await
            .expect("A elects");
        assert_eq!(l.leader, *a.node_id());
        // B sees A's leadership before A dies, so what follows is a lapse, not a missed commit.
        let mut seen = false;
        for _ in 0..200 {
            seen = b.consensus().consensus_get("leader/c1dead").as_deref() == Some(a.node_id().to_string().as_bytes());
            if seen { break; }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(seen, "B never saw A's leadership");
        a.shutdown().await;

        let mut won = None;
        for _ in 0..60 {
            if let Ok(l) = b.consensus().elect_leader_receipt("c1dead").await
                && l.leader == *b.node_id() && l.basis == LeadershipBasis::Decided {
                won = Some(l);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        assert!(won.is_some(), "a dead leader was reported for ever: B never won the election");
        b.shutdown().await;
        c.shutdown().await;
    }

    /// **Row A, C2: acceptor state is collected once its decision is over, and a live slot's
    /// promise survives.** Acceptor memory — and since #585 its durable record — was never erased:
    /// an output that does not decay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn acceptor_state_is_collected_once_its_decision_is_over() {
        use crate::consensus::{accepted_key, claim_vote, prepare_slot, ConsensusConfig, PrepareOutcome};
        use std::sync::Arc;
        let a = make_agent(alloc_port(), &[]).await;
        let _l = a.consensus().start_consensus_listener(ConsensusConfig::default());
        let ttl = std::time::Duration::from_secs(60);
        let mem = |slot: &str| a.task_ctx.consensus_accepted.pin().get(slot).cloned();
        let record = |slot: &str| a.task_ctx.kv_state.store.pin()
            .get(accepted_key(a.node_id(), slot).as_str()).and_then(|e| e.data.clone());

        // Finished: acquired and released.
        let g = a.consensus().distributed_lock("c2-done", ttl).await.expect("acquire");
        let (done_value, done_ballot) = (g.value.clone(), decided_of(&a, "lock/c2-done"));
        g.release();
        // Finished, but a later ballot is promised (a proposal in flight): must survive.
        let g = a.consensus().distributed_lock("c2-inflight", ttl).await.expect("acquire");
        let inflight_ballot = decided_of(&a, "lock/c2-inflight");
        g.release();
        let inflight: Arc<str> = Arc::from("lock/c2-inflight");
        assert!(matches!(
            prepare_slot(&a.task_ctx.consensus_accepted, &inflight, inflight_ballot + 1, 7, 0),
            PrepareOutcome::Promised(_)));
        // Live: held.
        let held = a.consensus().distributed_lock("c2-live", ttl).await.expect("acquire");
        for s in ["lock/c2-done", "lock/c2-inflight", "lock/c2-live"] {
            assert!(mem(s).is_some() && record(s).is_some(), "{s}: acceptor state present before collection");
        }

        let collected = a.consensus().collect_finished_acceptor_state().await;
        assert!(collected >= 1, "nothing was collected");
        assert!(mem("lock/c2-done").is_none(), "a finished slot's acceptor memory was kept");
        assert!(record("lock/c2-done").is_none(), "a finished slot's durable record was kept");
        assert_eq!(mem("lock/c2-inflight").map(|s| s.promised), Some(inflight_ballot + 1),
            "a promise to a ballot above the finished decision was forgotten");
        assert!(record("lock/c2-inflight").is_some());
        assert!(mem("lock/c2-live").is_some_and(|s| s.promised > 0) && record("lock/c2-live").is_some(),
            "a live slot's promise was forgotten");
        // What the forgotten state refused is still refused — by the floor.
        let done: Arc<str> = Arc::from("lock/c2-done");
        assert!(decided_of(&a, "lock/c2-done") >= done_ballot);
        assert!(!claim_vote(&a.task_ctx.consensus_accepted, &done, done_ballot, &done_value, 9,
            decided_of(&a, "lock/c2-done")), "the finished ballot is refused after collection");
        // And the slot is usable again.
        let g = a.consensus().distributed_lock("c2-done", ttl).await;
        assert!(g.is_ok(), "re-acquire after collection: {:?}", g.err());
        std::mem::forget(g);
        std::mem::forget(held);
        a.shutdown().await;
    }

    #[tokio::test]
    async fn test_commit_conflict_tripwire() {
        tripwire_case(0.0).await;
    }

    /// #568: the tripwire test failed once under the strict gate — its one forged COMMIT was shed: when the proposal
    /// returns, the proposer's own COMMIT still sits in the listener's 256-slot queue, so the fill is 1/256 and a single
    /// emit is shed about 0.4% of the time. Here the junk loop drives the COMMIT fill (the max over its subscribers) to
    /// 0.9, so the forged frame's first emit is shed nine times in ten, and later ones at the stalled subscriber's ~0.23;
    /// the test holds because it repeats the emit until the tripwire sees it.
    #[tokio::test]
    async fn the_tripwire_holds_under_a_loaded_signal_queue() {
        tripwire_case(0.9).await;
    }
}
