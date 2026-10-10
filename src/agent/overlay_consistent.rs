use std::sync::Arc;

use bytes::Bytes;

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
            LeaderTerm::Lease(d) => Some(d.as_secs().max(1)),
            LeaderTerm::Permanent => None,
        }
    }
}

/// RAII guard for a distributed lock acquired via [`ConsensusHandle::distributed_lock`].
///
/// On drop (or [`release`](Self::release)) it releases the lock's consensus slot — writes
/// `consensus/lease/lock/{name}` as *released at this guard's ballot* (row A) — **but only if this
/// guard is still the converged holder** (#164). `token` is a monotonic fencing token (the commit's HLC).
pub struct LockGuard {
    pub(super) ctx:      Arc<TaskCtx>,
    pub(super) name:     Arc<str>,
    /// The exact committed value this guard holds (`{holder}:{nonce}`). Release only clears the
    /// slot if the converged value still equals this — so a stale guard (lease lapsed, another
    /// acquire won, or even the same node re-acquiring under a fresh nonce) is a safe no-op.
    pub(super) value:    Bytes,
    /// The ballot this guard's commit was decided at — what its release record names as ended, so a
    /// later prepare sets that acceptance aside and a late COMMIT at it is stale (row A).
    pub(super) ballot:   u64,
    /// Fencing token: the **HLC timestamp** of the winning commit. Monotonic across
    /// successive holders of this lock name — stamp resource writes with it and have the
    /// resource reject a lower token (Kleppmann fencing). (The consensus *ballot* is NOT used —
    /// it regresses under gossip lag; #164.)
    pub token: u64,
    pub(super) released: bool,
}

impl LockGuard {
    /// Explicitly release the lock. Equivalent to dropping the guard.
    pub fn release(mut self) { self.do_release(); }

    fn do_release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let slot = format!("lock/{}", self.name);

        // #164 bug B: release the AUTHORITATIVE consensus slot, and only if the converged committed
        // value is still EXACTLY ours (`{holder}:{nonce}`) — a stale guard (lease lapsed, another
        // acquire won, or the same node re-acquiring under a fresh nonce) no-ops, so it can never
        // clear the live holder's claim (#149/#151).
        //
        // Row A: the release is the slot's **lifecycle record** written as released at this
        // guard's ballot (`consensus::release_decision`), not tombstones of `committed` and the
        // lease. Tombstones are collected after ~3000 s, and a slot with no committed entry looked
        // never decided, so the next acquirer adopted this guard's value from the acceptors' memory
        // and handed the lock back to its old holder (K1); and a late COMMIT re-stamped the entry
        // over the tombstone (K3). The record names the ballot that ended and is never collected.
        let _ = crate::consensus::release_decision(&self.ctx, &slot, &self.value, self.ballot);
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


    /// **Row A, K1: a released lock whose tombstone was garbage-collected is not re-committed to
    /// its old holder.** Release used to tombstone `consensus/committed/lock/{name}` and its lease
    /// and leave `consensus/decided/` and the acceptor's memory of `(ballot, holder's value)`. Once
    /// tombstone GC removed the entry (~3000 s with defaults), a slot with no entry is not *over* —
    /// the commit "may not have arrived yet" — so the next acquirer's prepare adopted the old
    /// holder's value, committed it at a fresh ballot and was told `Superseded`: the old holder held
    /// the lock again for a TTL, with no guard to release it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_released_lock_is_not_recommitted_to_its_holder_after_tombstone_gc() {
        let a = make_agent(alloc_port(), &[]).await;
        let _la = a.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
        let g1 = a.consensus().distributed_lock("gc", std::time::Duration::from_secs(60)).await
            .expect("first acquire");
        let old = g1.value.clone();
        g1.release();
        // Tombstone GC, run as if the GC horizon had long passed: every tombstone goes.
        crate::store::sweep_stale_tombstones(&a.task_ctx.kv_state.store, u64::MAX);

        let g2 = a.consensus().distributed_lock("gc", std::time::Duration::from_secs(60)).await;
        let live = crate::consensus::live_committed_value(
            &a.task_ctx.kv_state, "lock/gc", mycelium_core::sim_seam::wall_now_ms());
        assert!(g2.is_ok(), "re-acquire after a released lock's tombstones were collected: {:?}", g2.err());
        assert_ne!(live.as_deref(), Some(old.as_ref()), "the old holder's value was committed again");
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
