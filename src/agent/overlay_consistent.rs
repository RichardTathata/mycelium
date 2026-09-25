use std::sync::Arc;

use bytes::Bytes;

use super::{helpers::make_gossip_update, TaskCtx};
use crate::store::apply_and_notify;

// ── Public types ─────────────────────────────────────────────────────────────

/// Error returned when a consensus round does not commit.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ConsistencyError {
    /// All ballot attempts timed out without reaching quorum.
    Timeout { ballots_tried: u32 },
    /// Another node committed a value to the same slot first.
    Superseded,
    /// Quorum met in headcount but the Hard topology gate was not satisfied.
    TopologyUnsatisfied,
    /// **No electorate could be established, so nothing was decided** — the group roster is empty
    /// (unknown or unjoined), or smaller than a fresh `MembershipIntent { min }` declares, meaning
    /// this node's view is partial. An explicit one-member group still elects; what is refused is
    /// inferring authority from absence. See `ConsensusResult::ElectorateUnavailable`.
    ElectorateUnavailable { observed_members: usize, declared_min: usize },
}

impl std::fmt::Display for ConsistencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { ballots_tried } =>
                write!(f, "consensus timed out after {ballots_tried} ballot(s)"),
            Self::Superseded =>
                write!(f, "another node committed to this slot first"),
            Self::TopologyUnsatisfied =>
                write!(f, "quorum met but Hard topology gate not satisfied"),
            Self::ElectorateUnavailable { observed_members: 0, .. } =>
                write!(f, "no electorate: the group roster is empty (unknown or unjoined group) — \
                           an election needs members, and absence is not authority"),
            Self::ElectorateUnavailable { observed_members, declared_min } =>
                write!(f, "no electorate: this node sees {observed_members} member(s) but the \
                           group declares at least {declared_min} — the view is partial"),
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

/// RAII guard for a distributed lock acquired via [`ConsensusHandle::distributed_lock`].
///
/// On drop (or [`release`](Self::release)) it clears the lock's authoritative consensus slot
/// (`consensus/committed/lock/{name}` + its lease) — **but only if this guard is still the
/// converged holder** (#164). `token` is a monotonic fencing token (the commit's HLC).
pub struct LockGuard {
    pub(super) ctx:      Arc<TaskCtx>,
    pub(super) name:     Arc<str>,
    /// The exact committed value this guard holds (`{holder}:{nonce}`). Release only clears the
    /// slot if the converged value still equals this — so a stale guard (lease lapsed, another
    /// acquire won, or even the same node re-acquiring under a fresh nonce) is a safe no-op.
    pub(super) value:    Bytes,
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

        // #164 bug B: release the AUTHORITATIVE consensus slot (`consensus/committed/lock/{name}`
        // + its lease), token-guarded. The pre-fix code tombstoned the plain `lock/{name}` key —
        // which acquire never writes — so release was a total no-op and locks were permanently
        // unreleasable. Guard: only clear the slot if the converged committed value is still
        // EXACTLY ours (`{holder}:{nonce}`). A stale guard — lease lapsed, another acquire won, or
        // even the same node re-acquiring under a fresh nonce — sees a different value and no-ops,
        // so it can never clear the live holder's claim (the "losers stand by without releasing"
        // rule, #149/#151).
        let still_ours = crate::consensus::live_committed_value(
                &self.ctx.kv_state, &slot, crate::consensus::causal_now_ms(&self.ctx.hlc))
            .as_deref() == Some(self.value.as_ref());
        if !still_ours {
            return;
        }

        // Tombstone the committed value + its lease so the slot reopens for the next acquirer.
        for key in [
            format!("consensus/committed/{slot}"),
            format!("consensus/lease/{slot}"),
        ] {
            let update = make_gossip_update(
                &self.ctx.node_id,
                self.ctx.default_ttl,
                Arc::from(key.as_str()),
                Bytes::new(),
                true, // tombstone
                &self.ctx.hlc,
            );
            apply_and_notify(&self.ctx.kv_state, &update);
            crate::framing::dispatch_gossip_try_send(
                &self.ctx.gossip_txs,
                crate::framing::WireMessage::Data(update),
                self.ctx.node_id.id_hash(),
                crate::framing::ForwardHint::All,
                &self.ctx.kv_state.dropped_frames,
            );
        }
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
        // sets max_ballots = 1 and quorum_size = 2. Use system_propose directly.
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
            &a.task_ctx.kv_state, "lock/z", crate::consensus::wall_now_ms()).is_some();
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
