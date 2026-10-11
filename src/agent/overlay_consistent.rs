use std::sync::Arc;


use super::TaskCtx;

// ── Public types ─────────────────────────────────────────────────────────────

/// Error returned when a consensus round does not commit.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ConsistencyError {
    /// All ballot attempts timed out without reaching quorum.
    Timeout { ballots_tried: u32 },
    /// The slot was decided for another value — another node committed first, or (2.30.0) this
    /// call's prepare phase adopted a value a quorum had already accepted.
    Superseded,
    /// Quorum met in headcount but the Hard topology gate was not satisfied.
    TopologyUnsatisfied,
    /// **No electorate could be established, so nothing was decided** — the group roster is empty
    /// (unknown or unjoined), or smaller than a fresh `MembershipIntent { min }` declares, meaning
    /// this node's view is partial. An explicit one-member group still elects; what is refused is
    /// inferring authority from absence. See `ConsensusResult::ElectorateUnavailable`.
    ElectorateUnavailable { observed_members: usize, declared_min: usize },
    /// **This node is not in the group's roster**, so it may not propose to the group — its own
    /// vote would be one the electorate does not contain. Nothing was decided; join the group
    /// first. See `ConsensusResult::NotAMember`.
    NotAMember { group: Arc<str> },
}

impl std::fmt::Display for ConsistencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { ballots_tried } =>
                write!(f, "consensus timed out after {ballots_tried} ballot(s)"),
            Self::Superseded =>
                write!(f, "the slot was decided for another value"),
            Self::TopologyUnsatisfied =>
                write!(f, "quorum met but Hard topology gate not satisfied"),
            Self::ElectorateUnavailable { observed_members: 0, .. } =>
                write!(f, "no electorate: the group roster is empty (unknown or unjoined group) — \
                           an election needs members, and absence is not authority"),
            Self::ElectorateUnavailable { observed_members, declared_min } =>
                write!(f, "no electorate: this node sees {observed_members} member(s) but the \
                           group declares at least {declared_min} — the view is partial"),
            Self::NotAMember { group } =>
                write!(f, "not a member: this node is not in the roster of group {group}, so it \
                           may not propose to it — join the group first"),
        }
    }
}

impl std::error::Error for ConsistencyError {}

/// **How this node came to believe in a leader** — the rung a [`Leadership`] reached, and nothing
/// above it.
///
/// This substrate's rule for acknowledgements is that *a receipt names its rung and nothing above
/// it* (`docs/design/contracts-receipts.md`). `elect_leader` returned a bare `NodeId`, which names
/// no rung at all, and callers read it as an **exclusive grant** because nothing in the type said
/// otherwise. These two answers are different things and always were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadershipBasis {
    /// **This node's own proposal committed at a quorum.** The strongest rung the protocol offers:
    /// a quorum of the electorate voted for this value, at this ballot, bound to it by digest, and
    /// no other value can have been committed at that ballot.
    Decided,
    /// **Read from the converged slot** — somebody else decided, and this is what this node sees.
    ///
    /// Sound for *following* a leader. It is **not** evidence that the cluster currently agrees:
    /// what this node reads is its own replica, and a newer decision may be in flight. A node that
    /// needs exclusivity must fence on [`Leadership::epoch`] at the resource rather than trust the
    /// read.
    Observed,
}

/// The answer to "who leads this group", **with the rung it reached and a fencing token**.
///
/// Replaces the bare `NodeId` that `elect_leader` returns, which could not distinguish *"a quorum
/// chose me"* from *"this is what my replica currently says"* — a distinction that decides whether
/// two callers can act as leader at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leadership {
    /// The node this answer names as leader.
    pub leader: crate::node_id::NodeId,
    /// The commit's HLC — a **monotonic fencing token**, the same one `LockGuard` uses.
    ///
    /// Monotonic across successive holders (each observes the prior release), so a resource that
    /// refuses a lower token is genuinely fenced. The **ballot is not** usable for this: it
    /// regresses under gossip lag (#164). If you need exclusivity, this is the field that provides
    /// it — not the leader's identity, and not the fact that the call returned `Ok`.
    pub epoch: u64,
    /// Which rung this answer reached.
    pub basis: LeadershipBasis,
}

impl Leadership {
    /// `true` only for [`LeadershipBasis::Decided`] — a quorum chose this node's value.
    ///
    /// Deliberately not named `is_leader`: the question *"am I the leader"* has no
    /// coordinator-free answer that stays true for any length of time, and a method promising one
    /// would be the same overclaim in a shorter form.
    pub fn was_decided_here(&self) -> bool {
        self.basis == LeadershipBasis::Decided
    }
}

/// **The lease an election carries unless it asks for another** — 30 s (row A, C1).
///
/// A leadership is a role, and in this substrate roles evaporate (philosophy § *Anderson — More Is
/// Different*: a layer may never demand "roles that escape evaporation"). Until 2.32.0 `elect_leader`
/// committed permanently, so a leader that died was reported for ever. Why 30 s:
///
/// - it is the default anti-entropy interval (`GossipConfig::anti_entropy_interval_secs`), so a node
///   that missed the commit's gossip learns of it within one lease — a shorter lease could lapse on a
///   node before that node ever saw it;
/// - it is the lease the log-consumer claim and the gateway lock already default to, so the
///   substrate's leased roles fail over on one time scale;
/// - it is long against the ~1 s an election round takes, so renewing every `lease / 3` costs one
///   round per 10 s, and short against the failover an operator expects from a dead leader.
///
/// **Renewal is calling again**: re-electing while the lease is live re-commits the same value and
/// refreshes it (`ConsensusConfig::committed_lease_secs`). Stepping down is
/// [`release_leadership`](super::ConsensusHandle::release_leadership).
pub const DEFAULT_LEADER_LEASE: std::time::Duration = std::time::Duration::from_secs(30);

/// **How long an election holds** — [`elect_leader_with`](super::ConsensusHandle::elect_leader_with).
///
/// Leased by default ([`DEFAULT_LEADER_LEASE`]); permanence is an explicit opt-in, for a decision
/// that is genuinely ledger-shaped. A permanent leadership still has a release path
/// ([`release_leadership`](super::ConsensusHandle::release_leadership)) — what it lacks is a lapse, so
/// a permanent leader that dies is reported until someone releases it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderTerm {
    /// The leadership lapses `Duration` after its commit unless renewed (whole seconds, at least 1).
    Lease(std::time::Duration),
    /// The pre-2.32.0 behaviour: committed with no lease.
    Permanent,
}

impl Default for LeaderTerm {
    fn default() -> Self { LeaderTerm::Lease(DEFAULT_LEADER_LEASE) }
}

impl LeaderTerm {
    /// `ConsensusConfig::committed_lease_secs` for this term.
    pub(crate) fn lease_secs(self) -> Option<u64> {
        match self {
            LeaderTerm::Lease(d) => Some(crate::agent::consensus_handle::lease_secs_ceil(d)),
            LeaderTerm::Permanent => None,
        }
    }
}

/// RAII guard for a distributed lock acquired via [`ConsensusHandle::distributed_lock`].
///
/// On drop (or [`release`](Self::release)) it releases **the decision it was granted** — writes the
/// release marker for that decision's lineage (row A, `docs/design/lock-lifecycle.md` §5). A marker
/// ends only the decision it names, so a stale guard (its lease lapsed, another acquire won) can never
/// end the live holder's (#164). `token` is the decision's fencing token.
///
/// **The lease is the holder's to keep track of.** [`expires_at_ms`](Self::expires_at_ms) is the
/// lease's end on the wall clock every reader judges it by; [`deadline`](Self::deadline) is a
/// **monotonic** local bound — the instant this node started the proposal plus the TTL — by which the
/// holder must stop, whatever any clock says (review finding 4). Past it the lock may already be
/// someone else's; fence the resource on `token`.
pub struct LockGuard {
    pub(super) ctx:      Arc<TaskCtx>,
    pub(super) name:     Arc<str>,
    /// The decision this guard was granted — what its release names (row A).
    pub(super) env:      crate::consensus_life::Envelope,
    /// The monotonic local deadline: the proposal's start plus the TTL.
    pub(super) deadline: std::time::Instant,
    /// Fencing token: fixed by the decision's original proposer, after observing the prior decision's
    /// token (`docs/design/lock-lifecycle.md` §2.6). Stamp resource writes with it and have the
    /// resource reject a lower token (Kleppmann fencing). (The consensus *ballot* is NOT used — #164.)
    pub token: u64,
    pub(super) released: bool,
}

impl LockGuard {
    /// Explicitly release the lock. Equivalent to dropping the guard.
    pub fn release(mut self) { self.do_release(); }

    /// The lease's end, on the wall clock every reader judges it by (milliseconds since the epoch).
    pub fn expires_at_ms(&self) -> Option<u64> { self.env.expires_at_ms() }

    /// The monotonic local deadline by which this holder must stop: the instant the acquisition
    /// started plus the TTL. Earlier than the readers' wall-clock end, never later.
    pub fn deadline(&self) -> std::time::Instant { self.deadline }

    /// Whether [`deadline`](Self::deadline) has passed.
    pub fn is_expired(&self) -> bool {
        mycelium_core::sim_seam::mono_before(&self.deadline, &mycelium_core::sim_seam::mono_instant())
    }

    fn do_release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let slot = format!("lock/{}", self.name);

        // Row A: the release is the marker for this guard's decision (`release_envelope`) — it names
        // the lineage, proposer and value, so it ends this decision and no other, whatever this node
        // currently reads; nothing is tombstoned, so nothing can be collected out from under it (K1),
        // and no record write can remove it (K3).
        crate::consensus::release_envelope_try(&self.ctx, &slot, &self.env);
    }
}

impl Drop for LockGuard { fn drop(&mut self) { self.do_release(); } }

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockGuard")
            .field("name", &self.name)
            .field("token", &self.token)
            .field("released", &self.released)
            .finish_non_exhaustive()
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

    struct ConsensusPair {
        a:   GossipAgent,
        b:   GossipAgent,
        _la: crate::ConsensusListenerHandle,
        _lb: crate::ConsensusListenerHandle,
    }

    async fn consensus_pair() -> ConsensusPair {
        use crate::consensus::ConsensusConfig;
        let p1 = alloc_port();
        let p2 = alloc_port();
        let a = make_agent(p1, &[p2]).await;
        let b = make_agent(p2, &[p1]).await;
        let _la = a.consensus().start_consensus_listener(ConsensusConfig::default());
        let _lb = b.consensus().start_consensus_listener(ConsensusConfig::default());
        for _ in 0..40 {
            if !a.peers().is_empty() && !b.peers().is_empty() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        ConsensusPair { a, b, _la, _lb }
    }

    #[tokio::test]
    async fn test_consistent_set_and_get_two_nodes() {
        let pair = consensus_pair().await;

        pair.a.consensus().consistent_set("cfg/x", Bytes::from_static(b"hello")).await.unwrap();

        assert_eq!(pair.a.consensus().consistent_get("cfg/x").as_deref(), Some(b"hello".as_slice()));

        pair.a.shutdown().await;
        pair.b.shutdown().await;
    }

    #[tokio::test]
    async fn test_consistent_set_single_node_succeeds() {
        // Single node: quorum auto-computes to 1 (majority of 1), so it commits.
        let a = make_agent(alloc_port(), &[]).await;
        let r = a.consensus().consistent_set("cfg/solo", Bytes::from_static(b"ok")).await;
        assert!(r.is_ok(), "single-node consistent_set should succeed: {r:?}");
        assert_eq!(a.consensus().consistent_get("cfg/solo").as_deref(), Some(b"ok".as_slice()));
        a.shutdown().await;
    }

    #[tokio::test]
    async fn test_consistent_set_timeout_unreachable_quorum() {
        // Require quorum > 1 by proposing with an explicit ConsensusConfig that
        // sets max_ballots = 1 and quorum_size = 2. Use cluster_propose directly.
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


    /// #164 bug A regression gate: two nodes racing for the same lock must yield exactly one
    /// holder. Pre-fix this observed `winners == 2` (no mutual exclusion — both got a guard).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn distributed_lock_grants_single_holder_under_race() {
        let pair = consensus_pair().await;
        let (ca, cb) = (pair.a.consensus(), pair.b.consensus());
        let ta = tokio::spawn(async move { ca.distributed_lock("x", std::time::Duration::from_secs(60)).await });
        let tb = tokio::spawn(async move { cb.distributed_lock("x", std::time::Duration::from_secs(60)).await });
        let (ra, rb) = (ta.await.unwrap(), tb.await.unwrap());
        let winners = [ra.is_ok(), rb.is_ok()].iter().filter(|x| **x).count();
        std::mem::forget(ra);
        std::mem::forget(rb);
        assert_eq!(winners, 1, "lock granted to {winners} holders concurrently — not mutually exclusive");
        pair.a.shutdown().await;
        pair.b.shutdown().await;
    }

    /// #164 bug B regression gate: a released lock must be re-acquirable. Pre-fix, release
    /// tombstoned the wrong key (no-op), so re-acquire returned `Superseded` forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn distributed_lock_release_frees_for_reacquire() {
        let a = make_agent(alloc_port(), &[]).await;
        let _la = a.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
        let g1 = a.consensus().distributed_lock("y", std::time::Duration::from_secs(60)).await
            .expect("first acquire");
        g1.release();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let g2 = a.consensus().distributed_lock("y", std::time::Duration::from_secs(60)).await;
        assert!(g2.is_ok(), "re-acquire after release failed: {:?}", g2.err());
        std::mem::forget(g2);
        a.shutdown().await;
    }

    /// #164 bug B token-guard: a stale guard whose lease lapsed must NOT clear a later holder's
    /// claim on drop. Acquire with a short-but-safe lease (must exceed the ~1 s acquire settle),
    /// let it lapse, re-acquire (higher ballot), then drop the stale guard and assert the lock is
    /// still held. Same node re-acquires here, so the ballot/token guard — not holder-match — is
    /// what must save it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn distributed_lock_stale_release_does_not_clobber() {
        let a = make_agent(alloc_port(), &[]).await;
        let _la = a.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
        // 2 s lease: survives the 1 s acquire-settle, then lapses within the test.
        let g1 = a.consensus().distributed_lock("z", std::time::Duration::from_secs(2)).await
            .expect("first acquire");
        // Wait past the 2 s lease so the slot reopens.
        tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
        // Re-acquire (same node) — wins a fresh nonce, distinct from g1's.
        let g2 = a.consensus().distributed_lock("z", std::time::Duration::from_secs(60)).await
            .expect("re-acquire after lease lapse");
        // Drop the STALE first guard — must no-op (its value/nonce is no longer the converged one).
        drop(g1);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let held = crate::consensus::live_committed_value(
            &a.task_ctx.kv_state, "lock/z", mycelium_core::sim_seam::wall_now_ms()).is_some();
        assert!(held, "stale guard's drop cleared the live holder's claim");
        std::mem::forget(g2);
        a.shutdown().await;
    }


    // Suppress unused variant warning — ConsistencyError::Timeout is tested above.
    #[allow(dead_code)]
    fn _assert_consistency_error_variants() {
        let _ = ConsistencyError::Superseded;
        let _ = ConsistencyError::TopologyUnsatisfied;
    }
}
