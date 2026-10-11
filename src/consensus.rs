//! Consensus — single-decree agreement (prepare, propose, commit) built on the signal mesh.
//!
//! Lightweight Group-level and System-level agreement built on top of the
//! epidemic signal layer. Single-decree Paxos over gossip:
//!
//! ```text
//! Prepare → (promises, each reporting what it accepted) →
//!   Propose (the highest-ballot reported value, or the proposer's own) →
//!   (votes bound to that value) → Commit → KV committed/{slot}
//! ```
//!
//! The prepare phase (2.30.0) is what makes a value a quorum accepted reach every later proposer:
//! before it, a proposer learned an accepted value only from a refusal, and a strictly higher ballot
//! is never refused — so two proposers could commit different values for one slot on a stable
//! roster. See [`ConsensusMsg::Prepare`].
//!
//! Committed values are written to the Layer 1 KV store at
//! `consensus/committed/{slot}` and anti-entropy synced to late joiners.
//!
//! See [`crate::ConsensusHandle::group_propose`] and [`crate::ConsensusHandle::cluster_propose`] for
//! the entry points. [`GossipAgent::start_consensus_listener`] must be called
//! on every node that should participate as a voter.
//!
//! # Design notes
//!
//! - **Ballot numbering** (from SCP §6.2): monotonic counter stored at
//!   `consensus/ballot/{slot}`, kept across commits; `consensus/decided/{slot}` is the floor a commit sets.
//! - **Group-scoped votes**: votes are broadcast to the group, but only the proposer counts them and
//!   commits; if it crashes after a quorum accepted, the next proposer's prepare phase finishes it.
//! - **Signing**: with `tls`, every consensus payload is Ed25519-signed and a vote or proposal must be
//!   signed by the node it names; without it, trusted-domain only. Byzantine fault tolerance is out of
//!   scope.
//! - **Quorum slices** (optional, SCP §3.1): nodes may declare trust sets via
//!   [`GossipAgent::declare_trust`]. With `use_trust_slices` the proposer's tally counts only
//!   votes from its declared set — a fixed *eligible* voter set. The quorum size is still simple
//!   majority (or `quorum_size`) over the observed roster; slice-based quorum *intersection* is a
//!   future extension. **Safety-sensitive use** therefore means: `quorum_size` fixed,
//!   `use_trust_slices` on with every voter declaring the same set, `count_opaque_as_absent`
//!   off, and membership changes outside the supported profile — the quorum is derived from
//!   what this node *observes*, so intersection across a membership change is an assumption,
//!   not a consequence of value-bound votes. The eligibility filter is pinned by
//!   `lib_tests::test_trust_slice_filters_votes` (a vote from outside the declared set is not
//!   counted) and `test_trust_slice_admits_trusted_vote`.

use crate::agent::{emit_signal, emit_signal_async, make_gossip_update, TaskCtx};
use crate::config::{GroupTopologyPolicy, TopologyEnforcement};
use crate::framing::{
    dispatch_gossip_send, dispatch_gossip_try_send, make_kv_wire_msg,
    sync_entry_from, ForwardHint, GossipUpdate,
};
use crate::locality::LocalityPath;
use crate::node_id::NodeId;
use crate::signal::{grp_prefix, signal_kind, Signal, SignalScope};
use crate::store::{apply_and_notify, scan_kv_prefix};
use ahash::{AHashMap, AHashSet};
use bytes::Bytes;
use std::{
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time,
};

/// Configuration for a single consensus round.
///
/// Use [`ConsensusConfig::default`] and override only what you need. When
/// `quorum_size` is 0, the library computes majority from the live group
/// membership or peer count at proposal time.
#[derive(Clone, Debug)]
pub struct ConsensusConfig {
    /// Minimum number of distinct voters needed to commit.
    ///
    /// `0` = auto: `floor(N / 2) + 1` where N is the group member count
    /// (for [`group_propose`](crate::ConsensusHandle::group_propose)) or the
    /// known peer count + 1 (for
    /// [`cluster_propose`](crate::ConsensusHandle::cluster_propose)).
    pub quorum_size:    usize,
    /// How long each phase of a ballot attempt waits — the promises of the prepare phase (2.30.0),
    /// then the votes — before the attempt is declared failed. A ballot can take up to twice this.
    pub phase1_timeout: Duration,
    /// Maximum number of ballot attempts before returning [`ConsensusResult::Timeout`].
    pub max_ballots:    u32,
    /// Maximum random sleep (ms) before each ballot retry. Breaks lock-step livelock
    /// when two proposers increment their ballots in unison and repeatedly Nack each
    /// other. Two proposers sleeping for independent durations in `[0, N)` ms will
    /// rarely collide on the next retry; the first to wake succeeds.
    ///
    /// `0` disables jitter (not recommended outside tests). Default: `50`.
    pub ballot_retry_jitter_ms: u64,

    /// When `true`, group members that have a fresh `is_opaque: true` pheromone
    /// entry in Layer I (`load/{node_id}/{any kind}`) are excluded from the
    /// member count used to compute quorum. Prevents ballots from timing out
    /// waiting for overloaded voters.
    ///
    /// Requires `manage_opacity` writing pheromone trails (Fix B) to be effective.
    /// Default: `false`.
    ///
    /// **Availability trade-off**: the effective quorum is floored at 1 (a single
    /// transparent node can commit). When all members are simultaneously opaque, a
    /// lone transparent node satisfies quorum. Set `quorum_size` explicitly to
    /// prevent this if your correctness model requires a minimum voter count
    /// regardless of opacity.
    pub count_opaque_as_absent: bool,

    /// When `true`, this node will not vote in consensus rounds while any of its
    /// managed `load/{node_id}/*` entries show `is_opaque: true`. The node neither
    /// votes nor nacks — it silently drops `PROPOSE` messages while overloaded.
    /// Default: `false`.
    ///
    /// **Liveness risk**: if all nodes are simultaneously opaque, every ballot
    /// times out indefinitely. Set `max_abstain_ballots > 0` to automatically
    /// relax the abstain rule after that many consecutive abstentions, guaranteeing
    /// liveness at the cost of accepting votes from temporarily overloaded nodes.
    pub abstain_when_opaque: bool,

    /// When `true`, the proposer counts only votes from nodes in its own trust
    /// slice declared via [`GossipAgent::declare_trust`]. If no slice is declared
    /// for the group, all votes are counted (same as `false`). Default: `false`.
    pub use_trust_slices: bool,

    /// When `true`, `group_propose` calls [`suggest_leader`](crate::GossipAgent::suggest_leader)
    /// before entering the ballot loop. If the suggested leader is not this node, an additional
    /// deferral of `ballot_retry_jitter_ms` is applied, giving the healthier peer a window to
    /// win the first ballot unopposed.
    ///
    /// Uses [`SENDER_LOG_WINDOW`](crate::signal::SENDER_LOG_WINDOW) as the `max_age` for
    /// pheromone freshness. Default: `false`.
    ///
    /// **Note**: in `group_propose`, suggestion is based on pheromone load + trust counts
    /// within the group. In `cluster_propose`, suggestion defers this node if it is not the
    /// lowest-load proposer among all peers that have written a `consensus.propose` trail.
    pub use_suggest_leader: bool,

    /// Maximum consecutive ballot attempts during which this node may abstain due to
    /// `abstain_when_opaque`. After this many consecutive abstentions, the node votes
    /// regardless of its opacity state, guaranteeing liveness even when all nodes are
    /// simultaneously overloaded.
    ///
    /// `0` = no limit (always abstain when opaque). Default: `0`.
    pub max_abstain_ballots: u32,

    /// When `Some(secs)`, the commitment is **epoch-leased** rather than permanent:
    /// readers ([`consensus_get`](crate::ConsensusHandle::consensus_get),
    /// `GET /consensus/{slot}`) treat the committed value as absent once
    /// `now − commit_time > secs`, and the slot reopens for re-proposal.
    ///
    /// The lease window is written to `consensus/lease/{slot}` and gossips like any
    /// other key; since 2.32.0 (row A) it is a lifecycle record ([`LeaseRecord`]) that also names
    /// the decision's ballot and value digest and is measured from its own HLC timestamp (an older
    /// node's 8-byte record is measured from the committed entry's) — the same evaporation
    /// convention capability entries use
    /// ([`CapEntry::is_fresh`](crate::CapEntry::is_fresh)). No background task, no
    /// renewal RPC: an expired lease is simply no longer acted upon.
    ///
    /// **Renewal** is a fresh quorum round: re-propose the *same* value while the
    /// lease is live (allowed; refreshes the commit timestamp), or any value after
    /// expiry (the slot has reopened). Proposing a *different* value while the lease
    /// is live returns [`ConsensusResult::Superseded`], same as a permanent commit.
    ///
    /// `None` (default) = permanent commitment; behaviour is unchanged.
    pub committed_lease_secs: Option<u64>,

    /// **This proposal decides an exclusive outcome** — a lock, a leader, a single writer, anything a
    /// second holder would corrupt (post-360 plan row P2). A slot in a
    /// [`SAFETY_SLOT_FAMILIES`](crate::SAFETY_SLOT_FAMILIES) family (`lock/`, `leader/`, `consistent/`)
    /// is safety-sensitive whatever this says. On a node with `consensus_require_electorate`, a
    /// safety-sensitive proposal whose scope is not an electorate group is refused
    /// [`ConsensusResult::ElectorateNotGoverned`]. Default `false`.
    pub safety_sensitive: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            quorum_size:             0,
            phase1_timeout:          Duration::from_secs(5),
            max_ballots:             3,
            ballot_retry_jitter_ms:  50,
            count_opaque_as_absent:  false,
            abstain_when_opaque:     false,
            use_trust_slices:        false,
            use_suggest_leader:      false,
            max_abstain_ballots:     0,
            committed_lease_secs:    None,
            safety_sensitive:        false,
        }
    }
}

/// Per-group quorum requirement for [`crate::ConsensusHandle::cross_group_propose`].
///
/// Each entry describes one named capability group and the fraction of its
/// members that must accept before the proposal can commit across all groups.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GroupQuorum {
    /// Name of the capability group (matches the name used in [`GossipAgent::join_group`]).
    pub group: String,
    /// Fraction of group members required to accept. `0.5` means strict majority.
    /// Clamped to `(0.0, 1.0]` at runtime.
    pub quorum: f32,
    /// When `true`, this group acts as a ratification / compliance gate: it must
    /// reach its quorum fraction independently of all other groups. No additional
    /// wire semantics — the effect is simply that this group cannot be outweighed
    /// by others (the commit condition already requires all groups to pass).
    pub veto: bool,
}

/// Outcome of a [`group_propose`](crate::ConsensusHandle::group_propose) or
/// [`cluster_propose`](crate::ConsensusHandle::cluster_propose) call.
///
/// **`#[non_exhaustive]` since 2026-09-24.** A `_` arm is required, and it must **fail closed**:
/// a future refusal variant read as success is exactly the class of bug
/// [`ElectorateUnavailable`](Self::ElectorateUnavailable) exists to end. Same discipline as
/// `CallRefusal` and `CommitmentRefusal` (v2.13.0).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ConsensusResult {
    /// Quorum reached and the value was committed to the KV store.
    Committed {
        slot:   Arc<str>,
        value:  Bytes,
        ballot: u64,
        /// `true` when the committed slot (and its lease, if any) reached stable
        /// storage on this node — the forced-`fdatasync` WAL append succeeded, or
        /// persistence is not configured (nothing was promised). `false` means the
        /// value **is committed cluster-wide and applied locally**, but its local
        /// durability was **not established** (writer stopped, write or sync error):
        /// the WAL record is written before it is synced, so the bytes may or may not
        /// be on disk — a restart may restore it by local replay or via anti-entropy
        /// from peers, and nothing is promised. Never read `false` as "absent from the
        /// WAL". Logged at `error` with the slot; a caller that requires local
        /// durability must check this flag (`docs/design/contracts-receipts.md` §1).
        persisted: bool,
    },
    /// All ballot attempts timed out without reaching quorum.
    Timeout {
        slot:          Arc<str>,
        ballots_tried: u32,
        /// Votes counted in the latest attempt whose vote phase ran out without a commit or a refusal.
        ///
        /// In `propose` (one group) the proposer's own vote is included; `0` if no attempt got that
        /// far. Since 2.30.0 a partition or a promise shortfall ends an attempt in the prepare phase,
        /// so this does **not** tell them apart — it may read `0` or an earlier attempt's count. The cause of a
        /// timeout is the `reason` label on `mycelium_consensus_timeouts_total` (`metrics`): `no_voters`,
        /// `promise_short`, `contended`, `blocked`, `quorum_short`. In `cross_propose` it is the sum of
        /// the last attempt's accepts across the groups.
        votes_last_ballot: usize,
        /// Quorum size that was required (as computed at proposal time).
        ///
        /// Compare to `votes_last_ballot` to understand how far off quorum was.
        quorum_required: usize,
    },
    /// The slot was decided for **another** value: another proposer committed first, or (2.30.0) this
    /// proposal's prepare phase found a value a quorum had already accepted and committed that instead.
    /// The committed value is readable via [`consensus_get`](crate::GossipAgent::consensus_get).
    Superseded {
        slot:   Arc<str>,
        ballot: u64,
    },
    /// **The electorate could not be established, so no decision was attempted.**
    ///
    /// Either the group roster this node can see is **empty** — an unknown group, or one nobody
    /// has joined — or it holds **fewer members than the group declares** through a fresh
    /// `MembershipIntent { min }`, meaning this node's view is partial.
    ///
    /// **Why this is a refusal and not a singleton election.** Until 2026-09-24 an empty roster
    /// was counted as `members.len().max(1)` — one member, quorum one — and the proposer's own
    /// self-vote satisfied it. Every node therefore committed its own candidate unopposed: *N*
    /// singleton elections wearing the shape of one, converging afterwards by LWW if they
    /// converged at all. The defect was not the arithmetic; it was that **"I cannot see members"
    /// silently meant "I have authority to decide alone."**
    ///
    /// A singleton election remains entirely legitimate — when the roster **explicitly** holds one
    /// member. What is refused is inferring that authority from *absence*.
    ///
    /// The caller's options are to join the group, wait for the roster to converge, or (if it
    /// genuinely intends solo authority) establish a one-member group explicitly.
    ElectorateUnavailable {
        slot:  Arc<str>,
        group: Arc<str>,
        /// Members visible to this node in `grp/{group}/` at proposal time. `0` = unknown or
        /// unjoined group.
        observed_members: usize,
        /// The floor a fresh `MembershipIntent` declares for this group, or `0` when none is
        /// declared. `observed_members < declared_min` means this node's view is partial.
        declared_min: usize,
    },
    /// **This node is not in the group's roster, so it may not propose to the group.**
    ///
    /// A proposer counts its own promise and its own vote, and the quorum it needs is computed
    /// from the roster — which, until 2.32.0, it need not have been in. A stranger's self-vote is
    /// a vote the electorate does not contain: on a one-member group it decided alone, and on a
    /// larger group two strangers with disjoint acceptors could each reach quorum, since the
    /// quorums need not intersect in a member. Refused by name at the engine's door, before
    /// anything leaves, so nothing is in flight; `cluster_propose` has no roster and is unaffected.
    /// Join the group first — `mesh().join_group`, or `/gateway/govern/group` for a governed one.
    NotAMember {
        slot:  Arc<str>,
        group: Arc<str>,
    },
    /// **A safety-sensitive proposal named no electorate group, so nothing was proposed** (post-360
    /// plan row P2, `docs/design/consensus-electorate.md` §8).
    ///
    /// The node requires (`consensus_require_electorate`) that an exclusive outcome be decided by a
    /// group with an electorate declaration — a fixed set named as a group, whose membership moves only
    /// through governance — and this proposal's scope was the whole cluster (`group: None`), whose
    /// electorate is discovery, or a group with no declaration. Refused at the engine's door, before
    /// anything leaves. Declare the electorate (`declare_electorate`, `/gateway/govern/electorate`),
    /// or name one for the cluster-scoped verbs (`consensus_electorate`).
    ElectorateNotGoverned {
        slot:  Arc<str>,
        group: Option<Arc<str>>,
    },
    /// **This proposal's electorate is not the acceptors'** (P2, #601's review). A proposal on an
    /// electorate group carries the proposer's epoch and digest, and an acceptor answers only for its
    /// own — so a proposer still holding a pending genesis that was superseded cannot gather a quorum
    /// from members that adopted the later one. `epoch` is this proposer's, `seen_epoch` the one a member
    /// of its electorate reported. Nothing was decided by this proposal; re-read the electorate and retry.
    ElectorateStale {
        slot:       Arc<str>,
        group:      Arc<str>,
        epoch:      u64,
        seen_epoch: u64,
    },
    /// **The electorate and what this node sees disagree, so nothing was proposed** (P2). Either the
    /// group roster differs from the member set (a member joined or left an electorate whose membership is
    /// frozen — counted as a tripwire, never counted as a vote), the group is retired or closing (its
    /// retirement promised here), a genesis or retirement value does not fit, the slot is not this group's
    /// to decide, a cross-group proposal names an electorate group, a successor cannot read its predecessor
    /// yet, or this node's `consensus_electorate` disagrees with the fleet's exclusive default. `detail` says
    /// which.
    ElectorateMismatch {
        slot:   Arc<str>,
        group:  Option<Arc<str>>,
        detail: Arc<str>,
    },

    /// Quorum size was met but the Hard topology gate was not satisfied — too
    /// few distinct domains at `spread_depth`. The proposal is **not** committed.
    /// The caller decides whether to retry, wait for more diverse voters to
    /// come online, or surface the failure.
    ///
    /// Hard enforcement never silently degrades: it is better to refuse a
    /// fault-isolated write than to commit one that doesn't satisfy the
    /// operator-stated redundancy contract.
    TopologyUnsatisfied {
        slot:             Arc<str>,
        ballot:           u64,
        voters_seen:      usize,
        quorum_required:  usize,
        distinct_domains: usize,
        domains_required: usize,
        spread_depth:     usize,
    },
}

use crate::agent::electorate::{ElectorateDecl, Retirement};

/// What a proposal on an electorate group carries through the ballot loop (P2).
#[derive(Clone, Debug)]
pub(crate) struct ElectorateTag {
    /// The proposer's epoch (0 for a genesis).
    pub(crate) epoch:  u64,
    /// The electorate's digest (zero for a genesis).
    pub(crate) digest: [u8; 32],
    /// Votes are counted only from these members (`NodeId::id_hash`).
    pub(crate) voters: AHashSet<u64>,
    /// A strict majority of the members; every member for a genesis.
    pub(crate) quorum: usize,
    /// What the proposal is — an ordinary slot, the group's retirement, or a genesis attempt — with the
    /// group, as the fence gate needs. A retirement's or a genesis's commit carries a certificate.
    pub(crate) gate:   Gate,
    /// **The lazy carry** (round 4; design record §8): the retired predecessors whose majorities phase 1
    /// also reads, nearest first — empty once this group has decided the slot, or when it has none.
    pub(crate) predecessors: Vec<Arc<ElectorateDecl>>,
}

/// Which way a claim on an electorate group passes the gate (round 2 of #601's review, finding 1; round 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateKind {
    /// An ordinary slot of the group: refused once this node has closed the group.
    Ordinary,
    /// The group's retirement — or a carry report read from it: **closes the group for good** first, once no
    /// ordinary claim is in flight. A refused claim leaves only what is durable closed (round 4, finding 4).
    Retire,
    /// A genesis attempt at this epoch: refused once this node adopted an electorate for the group or touched a
    /// later attempt; it records the attempt it touched, which an adoption of an earlier one then refuses.
    Genesis(u64),
}

/// A claim's electorate: the group, the epoch it is made at, and its kind.
#[derive(Clone, Debug)]
pub(crate) struct Gate {
    pub(crate) group: Arc<str>,
    pub(crate) epoch: u64,
    pub(crate) kind:  GateKind,
}

/// **The gate's key**, per electorate group (`TaskCtx::electorate_fences`): every claim, adoption and carry
/// report on the group passes one `compute` on it, so their order is the order the safety argument needs. Pure
/// data, rewritten whole by each `compute`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FenceState {
    /// The group is closed for ordinary claims: this node promised its retirement or reported to a successor.
    pub(crate) closed:   bool,
    /// Ordinary claims between their gate decision and their claim.
    pub(crate) inflight: u32,
    /// Retirement claims (and carry reports) between raising `closed` and settling it.
    pub(crate) closing:  u32,
    /// Genesis claims between their gate decision and their claim.
    pub(crate) opening:  u32,
    /// The highest genesis attempt this node's acceptor promised or accepted.
    pub(crate) touched:  u64,
    /// The genesis attempt this node adopted (0: none).
    pub(crate) adopted:  u64,
}

impl ElectorateTag {
    /// Whether a member's refusal says this proposer is behind: the member is in this electorate and
    /// names a later epoch under another electorate's digest (a genesis that superseded this one). A member
    /// that closed the group names this digest at the next epoch and is not evidence of staleness; a member
    /// behind names an earlier epoch. Either simply does not vote.
    fn knows_a_later_electorate(&self, voter: &NodeId, epoch: u64, digest: &[u8; 32]) -> bool {
        self.voters.contains(&voter.id_hash()) && epoch > self.epoch && *digest != self.digest
    }

    fn of(d: &ElectorateDecl, kind: GateKind) -> Self {
        Self {
            epoch: d.epoch, digest: d.digest(), quorum: d.quorum(),
            voters: d.members.iter().map(NodeId::id_hash).collect(),
            gate: Gate { group: Arc::from(d.group.as_str()), epoch: d.epoch, kind },
            predecessors: Vec::new(),
        }
    }

    /// Whether this proposal's commit carries a certificate: a genesis or a retirement.
    fn certified(&self) -> bool {
        matches!(self.gate.kind, GateKind::Genesis(_) | GateKind::Retire)
    }

    /// Whether a refusal's sender may move this proposal: a member of its electorate (round 2, finding 5).
    fn sent_by_member(&self, sender: &NodeId) -> bool {
        self.voters.contains(&sender.id_hash())
    }
}

/// Why the proposer's P2 door refused, before anything was sent.
pub(crate) enum DoorRefusal {
    NotGoverned(Option<Arc<str>>),
    NotAMember(Arc<str>),
    Mismatch { group: Option<Arc<str>>, detail: Arc<str> },
}

impl DoorRefusal {
    fn mismatch(group: Option<&Arc<str>>, detail: &str) -> Self {
        Self::Mismatch { group: group.cloned(), detail: Arc::from(detail) }
    }
    fn into_result(self, slot: Arc<str>) -> ConsensusResult {
        match self {
            Self::NotGoverned(group) => ConsensusResult::ElectorateNotGoverned { slot, group },
            Self::NotAMember(group) => ConsensusResult::NotAMember { slot, group },
            Self::Mismatch { group, detail } => ConsensusResult::ElectorateMismatch { slot, group, detail },
        }
    }
}

static ROSTER_MISMATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Proposals refused because an electorate group's roster differed from its members — a member that joined
/// or left an electorate whose membership is frozen (P2's tripwire).
pub fn electorate_roster_mismatches() -> u64 {
    ROSTER_MISMATCHES.load(std::sync::atomic::Ordering::Relaxed)
}

fn note_roster_mismatch(slot: &str, group: &str, epoch: u64) {
    ROSTER_MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_electorate_roster_mismatch_total").increment(1);
    tracing::warn!(slot, group, epoch, "electorate: the group roster differs from its electorate; not proposing \
        (membership is frozen — migrate to a successor group to change it)");
}

/// `ElectorateStale` for a proposal a member of its electorate refused for another epoch.
fn stale_result(slot: Arc<str>, scope: &SignalScope, tag: Option<&ElectorateTag>, seen: u64) -> ConsensusResult {
    let group = match scope {
        SignalScope::Group(g) => Arc::clone(g),
        SignalScope::Groups(gs) => gs.first().cloned().unwrap_or_else(|| Arc::from("")),
        _ => Arc::from(""),
    };
    ConsensusResult::ElectorateStale { slot, group, epoch: tag.map_or(0, |t| t.epoch), seen_epoch: seen }
}

/// `ElectorateMismatch` for a claim this node's own gate refused after the door: the group closed (its
/// retirement was promised here) or the genesis attempt was overtaken while the proposal ran.
fn gate_refused(slot: Arc<str>, tag: Option<&ElectorateTag>) -> ConsensusResult {
    let detail = match tag.map(|t| t.gate.kind) {
        Some(GateKind::Genesis(e)) => format!("genesis attempt {e} was overtaken here: this node adopted an electorate or touched a later attempt"),
        _ => "the group was closed here while this proposal ran: a member promised its retirement".to_string(),
    };
    ConsensusResult::ElectorateMismatch { slot, group: tag.map(|t| Arc::clone(&t.gate.group)), detail: Arc::from(detail.as_str()) }
}

/// The voters a proposal counts: an electorate group's roster as it stood when the proposal began,
/// narrowed by a declared trust slice when both apply (P2's tension (b): the electorate is named by the
/// group; a trust slice stays available beneath it). Neither ⇒ no filter.
fn electorate_vote_filter(trust: Option<AHashSet<u64>>, roster: Option<AHashSet<u64>>) -> Option<AHashSet<u64>> {
    match (trust, roster) {
        (Some(t), Some(r)) => Some(t.intersection(&r).copied().collect()),
        (None, Some(r)) => Some(r),
        (t, None) => t,
    }
}

/// Wire payload carried inside `Signal.payload` for all consensus messages.
///
/// Encoded with `bincode_cfg()` (fixed-int, same as the rest of the wire format).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum ConsensusMsg {
    Propose {
        slot:     Arc<str>,
        ballot:   u64,
        value:    Bytes,
        proposer: NodeId,
    },
    Vote {
        slot:   Arc<str>,
        ballot: u64,
        voter:  NodeId,
    },
    Commit {
        slot:   Arc<str>,
        ballot: u64,
        value:  Bytes,
    },
    Nack {
        slot:        Arc<str>,
        seen_ballot: u64,
    },
    /// Voter response carrying the voter's `LocalityPath` so the proposer can
    /// evaluate topology gates (Hard enforcement). Voters that have no
    /// configured locality send `locality: None`; their vote still counts
    /// toward quorum but contributes zero topology diversity.
    ///
    /// Added after `Nack` so old proposers (which decode unknown variants as
    /// `None` and drop the message) silently lose these votes rather than
    /// misinterpreting them. Mixed-version clusters cannot run Hard policies.
    VoteWithLocality {
        slot:     Arc<str>,
        ballot:   u64,
        voter:    NodeId,
        locality: Option<LocalityPath>,
    },
    /// Voter response **bound to the value it voted for** — the only form a proposer counts.
    ///
    /// ## Why this variant exists
    ///
    /// [`Vote`](Self::Vote) and [`VoteWithLocality`](Self::VoteWithLocality) name a `(slot,
    /// ballot)` and a voter, and **nothing else**. A proposer collecting votes matched on `(slot,
    /// ballot)` alone and then committed **its own** value. Ballots are drawn from a shared KV key
    /// (`read_ballot + 1`), so concurrent proposers pick the *same* ballot by construction, and
    /// votes are emitted to the group scope — a **broadcast**, not a unicast to the proposer.
    ///
    /// Put together: A proposes `v_A` at ballot 1, B proposes `v_B` at ballot 1, C votes once for
    /// whichever it saw first, and **both A and B count C's vote**. Each reaches quorum. Each
    /// commits a different value at the same ballot — a single-decree safety violation.
    ///
    /// The acceptor already refused to *accept* two values at one ballot (`may_cast_vote`, audit
    /// 2026-07-15), and that was not enough: it stopped C voting twice, but C's single vote did not
    /// say **what C accepted**, so it could not stop the counting. The NACK C sends the second
    /// proposer does not save it either — a proposer only acts on `seen_ballot > ballot`, and this
    /// NACK carries an *equal* ballot.
    ///
    /// `value_digest` closes it: a vote is evidence for **one** value, and a proposer counts only
    /// evidence for the value it proposed.
    ///
    /// ## Compatibility
    ///
    /// Added **after** `VoteWithLocality`, so a proposer predating it decodes an unknown variant as
    /// `None` and drops the message — the same rolling-upgrade shape that variant used. Voters emit
    /// this **and** the legacy form, so old proposers keep working exactly as before. New proposers
    /// count only this one, which means a mixed cluster **times out rather than committing two
    /// values**: fail-closed, and visible.
    VoteForValue {
        slot:         Arc<str>,
        ballot:       u64,
        voter:        NodeId,
        /// SHA-256 over the exact value bytes this voter accepted.
        value_digest: [u8; 32],
        locality:     Option<LocalityPath>,
    },
    /// A refusal that **reports what this acceptor has already accepted**, so a proposer moving to
    /// a higher ballot can adopt it instead of overwriting it.
    ///
    /// ## Why a bare `Nack` was not enough
    ///
    /// [`Nack`](Self::Nack) carries a `seen_ballot` and nothing else, and the proposer's retry is
    /// `ballot = …max(ballot) + 1` while **still proposing its own `value`**, which never changes
    /// across attempts. So a value a quorum had already accepted at ballot *N* could be replaced by
    /// a different value at *N+1* — the acceptors allow it, because `may_cast_vote` returns `true`
    /// for any strictly greater ballot.
    ///
    /// The commit record guards the *committed* case (`try_commit_if_ready` refuses when
    /// `live_committed` holds a different value), but only once it has propagated; inside that
    /// window the overwrite stands. A refusal is not enough to close it — a strictly higher ballot
    /// is never refused, so no `Promise` is sent — which is why [`Prepare`](Self::Prepare) exists
    /// (2.30.0): the closing is phase 1's, and this refusal remains a hint that moves a proposer up.
    ///
    /// This is the classic promise: the acceptor answers a refusal with its highest accepted
    /// `(ballot, value)`, and the proposer adopts the one with the **highest accepted ballot**
    /// before retrying. Appended last, so older proposers drop it as an unknown variant and fall
    /// back to the `Nack` that is still sent alongside.
    Promise {
        slot:           Arc<str>,
        /// The ballot this acceptor has seen — the same field `Nack` carries.
        seen_ballot:    u64,
        /// The ballot at which `accepted_value` was accepted, or `0` when nothing is accepted.
        accepted_ballot: u64,
        /// The value this acceptor has accepted, if any.
        accepted_value: Option<Bytes>,
    },
    /// **Phase 1: ask before proposing.** A proposer asks the scope's acceptors to *promise*
    /// `ballot` and to report what they have already accepted, and proposes only once a quorum has
    /// promised — carrying the highest-ballot value any of them reported.
    ///
    /// ## Why this variant exists
    ///
    /// Without it a proposer learned an accepted value only from a **refusal** (`Promise`), and a
    /// strictly higher ballot is never refused: the acceptor granted it and overwrote what it held.
    /// So A could choose `v1` at ballot 1 with `{A, 1, 2}` while B, which had not yet seen A's
    /// COMMIT, chose `v2` at ballot 2 with `{B, 2, 3}` — two values for one slot on a stable
    /// roster, the only guard being B's local view of the committed key (2026-10-08 review).
    /// A promise quorum intersects every accept quorum, so the value a quorum accepted is reported
    /// to every later proposer before it may propose anything.
    ///
    /// Ballots come from a shared KV key, so two proposers can draw the same one. An acceptor
    /// therefore promises a ballot to **one** proposer and refuses another's accept at that ballot,
    /// which keeps *one value per ballot* — the property the adoption rule depends on.
    ///
    /// ## Compatibility
    ///
    /// Sent under the `PROPOSE` kind and appended last: an acceptor predating it decodes an unknown
    /// variant as `None` and ignores it, so an upgraded proposer **times out rather than commits**
    /// until a quorum is upgraded — fail-closed, as `VoteForValue` was. A proposer predating it
    /// sends no `Prepare`; upgraded acceptors still accept its higher ballots, so a fleet keeps
    /// working mid-upgrade, and has the guarantee only once every proposer is upgraded.
    Prepare {
        slot:     Arc<str>,
        ballot:   u64,
        proposer: NodeId,
    },
    /// An acceptor's answer to a [`Prepare`](Self::Prepare) it granted: it will refuse any lower
    /// ballot and any other proposer at `ballot`, and it reports what it holds.
    PrepareAck {
        slot:            Arc<str>,
        ballot:          u64,
        voter:           NodeId,
        /// The ballot `accepted_digest` was accepted at; `0` when nothing is accepted.
        accepted_ballot: u64,
        /// Digest of the accepted value — present whenever something is accepted, including after
        /// a restart, when the value itself is not known.
        accepted_digest: Option<[u8; 32]>,
        /// The accepted value, when this acceptor still has it.
        accepted_value:  Option<Bytes>,
        /// Digest of the live committed value for the slot on this acceptor, if any — a digest,
        /// because the proposer only asks whether it is its own value, and a full value here would
        /// double the reply for a large slot.
        committed_digest: Option<[u8; 32]>,
    },
    // ── Row A (design `docs/design/lock-lifecycle.md` §3): the decision envelope on the wire ──────
    //
    // Appended after every older variant, so a node predating them decodes an unknown variant as
    // `None` and drops it — the 2.30.0 `Prepare` shape: an upgraded proposer times out rather than
    // commits until a quorum is upgraded.
    /// Phase 1 from an upgraded proposer. An upgraded acceptor answers it with
    /// [`PrepareAckTerm`](Self::PrepareAckTerm); an older one ignores it.
    PrepareTerm {
        slot:     Arc<str>,
        ballot:   u64,
        proposer: NodeId,
    },
    /// An upgraded acceptor's promise: what it accepted, **flagged** as an envelope or a legacy value,
    /// so the proposer can adopt the envelope whole (its window included) or wrap a legacy value.
    PrepareAckTerm {
        slot:                 Arc<str>,
        ballot:               u64,
        voter:                NodeId,
        accepted_ballot:      u64,
        accepted_digest:      Option<[u8; 32]>,
        accepted:             Option<Bytes>,
        accepted_is_envelope: bool,
        /// Digest of the live committed **value** (not envelope) on this acceptor, if any.
        committed_digest:     Option<[u8; 32]>,
    },
    /// Phase 2 with the decision envelope (`consensus_life::Envelope`) as the value.
    ProposeTerm {
        slot:     Arc<str>,
        ballot:   u64,
        envelope: Bytes,
        proposer: NodeId,
    },
    /// A refusal to an envelope proposal, reporting what this acceptor holds, flagged.
    PromiseTerm {
        slot:                 Arc<str>,
        seen_ballot:          u64,
        accepted_ballot:      u64,
        accepted:             Option<Bytes>,
        accepted_is_envelope: bool,
    },
    /// **The whole decision**: a learner writes the decision record from it, so it never holds a
    /// decision without its window (design §3).
    CommitTerm {
        slot:     Arc<str>,
        ballot:   u64,
        envelope: Bytes,
    },
    /// [`Prepare`](Self::Prepare) for a slot on an **electorate group** (P2), naming the proposer's
    /// electorate: its `epoch` and `electorate` digest (`ElectorateDecl::digest`; zero for a genesis,
    /// epoch 0). An acceptor answers only when both are its own; otherwise it refuses with
    /// [`StaleElectorate`](Self::StaleElectorate).
    ///
    /// ## Compatibility
    ///
    /// Appended last, like `Prepare` in 2.30.0: a node predating it decodes an unknown variant as
    /// `None` and ignores it, so a mixed electorate **times out rather than commits** until its members
    /// are upgraded. Answered as [`PrepareTerm`](Self::PrepareTerm) is: [`PrepareAckTerm`](Self::PrepareAckTerm),
    /// [`PromiseTerm`](Self::PromiseTerm).
    PrepareIn {
        slot:       Arc<str>,
        ballot:     u64,
        proposer:   NodeId,
        epoch:      u64,
        electorate: [u8; 32],
    },
    /// [`ProposeTerm`](Self::ProposeTerm) — the decision envelope — for a slot on an electorate group,
    /// naming the proposer's electorate as [`PrepareIn`](Self::PrepareIn) does.
    ProposeIn {
        slot:       Arc<str>,
        ballot:     u64,
        envelope:   Bytes,
        proposer:   NodeId,
        epoch:      u64,
        electorate: [u8; 32],
    },
    /// An acceptor's refusal of a proposal whose electorate is not its own — the epoch and digest it
    /// holds (or the next epoch under the same digest, when it has closed the group for its retirement and
    /// so answers it no more). Sent under `NACK`.
    StaleElectorate {
        slot:       Arc<str>,
        voter:      NodeId,
        epoch:      u64,
        electorate: [u8; 32],
    },
    /// A successor's request to a member of a **retired** electorate group for a read-only report of what it
    /// holds for `slot` — the lazy carry (P2, round 4 of #601's review; design record §8). Answered only by a
    /// member of the retired `group`, only when the retirement verifies there, and only for a proposer in a
    /// group that succeeds it; the member closes `group` for good (through its gate, after any ordinary claim
    /// in flight) and puts the retirement on stable storage before it reports. It promises nothing. Sent under
    /// `PROPOSE`, to each member individually. Appended last, like [`PrepareIn`](Self::PrepareIn).
    CarryPrepare {
        slot:     Arc<str>,
        ballot:   u64,
        proposer: NodeId,
        group:    Arc<str>,
    },
    /// A retired group member's carry report: its promise for `slot` and to whom, and its latest acceptance,
    /// read from its acceptor memory after it closed the group. Under `VOTE`, to the proposer.
    CarryReport {
        slot:             Arc<str>,
        ballot:           u64,
        voter:            NodeId,
        group:            Arc<str>,
        promised:         u64,
        promised_to:      Option<u64>,
        accepted_ballot:  u64,
        accepted_digest:  Option<[u8; 32]>,
        accepted_value:   Option<Bytes>,
        /// Whether `accepted_value` is a decision envelope (row A) or a legacy value.
        accepted_is_envelope: bool,
    },
}

/// Releases an ordinary claim's in-flight registration on its group's gate key when the claim is done.
struct InFlight<'a> {
    fences: &'a papaya::HashMap<Arc<str>, FenceState>,
    group:  &'a Arc<str>,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.fences.pin().compute(Arc::clone(self.group), |cur| {
            let s = cur.map(|(_, v)| *v).unwrap_or_default();
            papaya::Operation::<FenceState, ()>::Insert(FenceState { inflight: s.inflight.saturating_sub(1), ..s })
        });
    }
}

thread_local! {
    /// How deep this thread is in nested electorate verifications (`ViewDepth`).
    static VIEW_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One level of nested electorate verification; `enter` refuses past `CHAIN_LIMIT` levels.
struct ViewDepth;

impl ViewDepth {
    fn enter() -> Option<Self> {
        VIEW_DEPTH.with(|d| {
            if d.get() >= crate::agent::electorate::CHAIN_LIMIT { return None; }
            d.set(d.get() + 1);
            Some(Self)
        })
    }
}

impl Drop for ViewDepth {
    fn drop(&mut self) {
        VIEW_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Two digests combined — the key a refused record and its certificate are remembered under.
fn xor_digests(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = *a;
    for (o, x) in out.iter_mut().zip(b.iter()) { *o ^= *x; }
    out
}

/// SHA-256 of a proposal's value — what a [`VoteForValue`](ConsensusMsg::VoteForValue) commits its
/// voter to, and what a proposer checks its votes against.
pub(crate) fn value_digest(value: &Bytes) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(value);
    h.finalize().into()
}

/// Cancels the consensus listener task on drop.
///
/// Obtain from [`ConsensusHandle::start_consensus_listener`].
/// The task also exits when the agent shuts down even if this handle is live.
pub struct ConsensusListenerHandle {
    pub(crate) _cancel: oneshot::Sender<()>,
    /// Stops the acceptor collector started beside the listener (row A, C2).
    pub(crate) _cancel_collector: oneshot::Sender<()>,
}

/// Well-known signal kind strings for consensus messages.
pub mod consensus_kind {
    /// The proposer's messages: phase 1's `Prepare` and phase 2's `Propose` (the candidate value).
    pub const PROPOSE: &str = "consensus.propose";
    /// The acceptor's answers: phase 1's `PrepareAck` (to the proposer) and phase 2's votes.
    pub const VOTE:    &str = "consensus.vote";
    /// Phase 2: any node broadcasts that quorum has been reached.
    pub const COMMIT:  &str = "consensus.commit";
    /// Phase 1: voter rejects a stale ballot (higher already seen).
    pub const NACK:    &str = "consensus.nack";
}

/// KV key namespace prefixes used by the consensus layer.
pub mod consensus_ns {
    /// Durable committed values. Key: `consensus/committed/{slot}`.
    /// Written on commit; anti-entropy syncs to late joiners automatically.
    pub const COMMITTED: &str = "consensus/committed/";
    /// Highest ballot seen for a slot. Key: `consensus/ballot/{slot}`.
    /// Prevents stale commits from overwriting fresh ones.
    pub const BALLOT:    &str = "consensus/ballot/";
    /// Quorum trust slice declarations (optional, SCP §3.1 inspired).
    /// Key: `consensus/trust/{group}/{node_id}`. Value: bincode-encoded
    /// `Vec<NodeId>` of trusted peers.
    pub const TRUST:     &str = "consensus/trust/";
    /// The slot's lifecycle record. Key: `consensus/lease/{slot}`. Value: u64 LE milliseconds, then
    /// (since 2.32.0, row A) the ballot and value digest of the decision it bounds and whether its
    /// holder released it ([`LeaseRecord`]). Written at commit time when
    /// [`ConsensusConfig::committed_lease_secs`] is set, and as *released* by a lock or leadership
    /// release ([`release_decision`]); absent (or tombstoned) for permanent commitments. Expiry is
    /// evaluated read-side — see [`ConsensusConfig::committed_lease_secs`].
    pub const LEASE:     &str = "consensus/lease/";
    /// The ballot a slot's most recent commit was decided at. Key: `consensus/decided/{slot}`,
    /// value `u64` LE. A ballot at or below it belongs to a finished decision: acceptors refuse
    /// it, and a proposer reopening an expired leased slot ignores acceptances at or below it.
    pub const DECIDED:   &str = "consensus/decided/";
}

// ── Lease helpers ─────────────────────────────────────────────────────────────

/// The legacy lease record, `u64` LE ms — still written for readers older than row A until a later,
/// governed MINOR (design §7, Q3); an upgraded reader consults it only for a slot with no decision
/// record (§2.3).
pub(crate) fn encode_lease_ms(ms: u64) -> Bytes {
    Bytes::copy_from_slice(&ms.to_le_bytes())
}

/// `None` on malformed bytes — readers treat a malformed lease as *permanent*
/// (never silently expire a commitment because a lease entry was corrupted).
pub(crate) fn decode_lease_ms(bytes: &Bytes) -> Option<u64> {
    (bytes.len() >= 8).then(|| u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8])))
}

/// The reader's **causal now** on the HLC physical domain: `max(wall clock, HLC physical)` — what the
/// **legacy** lease is read against. A decision record's lease is read against the wall clock instead
/// (row A, review finding 4).
pub(crate) fn causal_now_ms(hlc: &crate::hlc::Hlc) -> u64 {
    // The same formula as `Hlc::decision_now_ms`, which now exists for exactly this (C11).
    hlc.decision_now_ms()
}

/// Returns the **live** committed value for `slot`, or `None` — see [`live_committed_with_hlc`].
pub(crate) fn live_committed_value(
    kv:     &crate::store::KvState,
    slot:   &str,
    now_ms: u64,
) -> Option<Bytes> {
    live_committed_with_hlc(kv, slot, now_ms).map(|(v, _)| v)
}

/// The live committed value for `slot` and its **fencing token**.
///
/// Row A (`docs/design/lock-lifecycle.md` §2.3): the decision record with the highest ballot this
/// node holds decides it — live unless a release marker names it or its lease is past on **this
/// node's wall clock** — and the token is the one the original proposer fixed in the envelope. A slot
/// that has had records (its sentinel is here) but holds none reads as not live. Only a slot with no
/// record and no sentinel falls back to the legacy reading below, with `now_ms` (the causal now) against the committed
/// entry's timestamp and an 8-byte lease — the reading a node older than row A uses.
pub(crate) fn live_committed_with_hlc(
    kv:     &crate::store::KvState,
    slot:   &str,
    now_ms: u64,
) -> Option<(Bytes, u64)> {
    match crate::consensus_life::read_slot(kv, slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(top) =>
            if top.ended { None } else { top.value.map(|v| (v, top.env.token)) },
        crate::consensus_life::SlotView::Unknown => None,
        crate::consensus_life::SlotView::NoRecord => legacy_live(kv, slot, now_ms),
    }
}

/// The legacy reading, unchanged from before row A: `consensus/committed/{slot}`, expired by an 8-byte
/// `consensus/lease/{slot}` measured from the committed entry's HLC timestamp (BOUNDED-CLOCK-SKEW, audit
/// 2026-07-15 BUG 8); the token is the committed entry's HLC.
fn legacy_live(kv: &crate::store::KvState, slot: &str, now_ms: u64) -> Option<(Bytes, u64)> {
    let commit_key = format!("{}{}", consensus_ns::COMMITTED, slot);
    let guard = kv.store.pin();
    let entry = guard.get(commit_key.as_str())?;
    let data  = entry.data.clone()?;
    let hlc   = entry.timestamp;
    let lease_key = format!("{}{}", consensus_ns::LEASE, slot);
    let Some(lease_bytes) = guard.get(lease_key.as_str()).and_then(|e| e.data.clone()) else {
        return Some((data, hlc)); // no lease (or tombstoned lease) → permanent
    };
    let Some(lease_ms) = decode_lease_ms(&lease_bytes) else {
        return Some((data, hlc)); // malformed lease → treat as permanent
    };
    let written_ms = crate::hlc::physical_ms(entry.timestamp);
    (now_ms.saturating_sub(written_ms) <= lease_ms).then_some((data, hlc))
}

/// The digest of `slot`'s live decision's value — see [`ConsensusEngine::live_digest`].
pub(crate) fn live_digest(kv: &crate::store::KvState, slot: &str, now_ms: u64) -> Option<[u8; 32]> {
    match crate::consensus_life::read_slot(kv, slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(top) => (!top.ended).then_some(top.env.value_digest),
        crate::consensus_life::SlotView::Unknown => None,
        crate::consensus_life::SlotView::NoRecord => legacy_live(kv, slot, now_ms).map(|(v, _)| value_digest(&v)),
    }
}

/// **The ballot at or below which `slot`'s latest decision is known to be over**: the top decision
/// record's ballot when it is released or its lease is past on this node's wall clock (design §2.3).
/// `None` when it is live, unknown here, or the slot has no decision record (the legacy reading's
/// `decision_over` covers that case in the engine).
pub(crate) fn ended_at(kv: &crate::store::KvState, slot: &str) -> Option<u64> {
    match crate::consensus_life::read_slot(kv, slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(top) if top.ended => Some(top.ballot),
        _ => None,
    }
}

/// **Release the decision `env` names** (design §5): writes its release marker — the only thing that
/// ends it — and, while readers older than row A exist and this node still reads `env` as the slot's
/// live decision, the legacy release form (an 8-byte lease of 0, which such a reader reads as expired).
/// It names no ballot and tombstones nothing; a marker ends only the decision whose lineage, proposer
/// and value it names, so it can never end another (a stale guard's release writes a marker no reader
/// of a newer decision consults). Returns the applied updates for the caller to hand to the WAL.
pub(crate) fn release_envelope(ctx: &TaskCtx, slot: &str, env: &crate::consensus_life::Envelope) -> Vec<GossipUpdate> {
    let mut out = Vec::new();
    let marker = make_gossip_update(
        &ctx.node_id, ctx.default_ttl,
        Arc::from(crate::consensus_life::marker_key(slot, env.lineage, &env.proposer).as_str()),
        crate::consensus_life::encode_marker(env), false, &ctx.hlc,
    );
    // The legacy form only for the decision this node still reads as live — a stale release must not
    // end a newer holder for older readers either.
    let still_top = matches!(
        crate::consensus_life::read_slot(&ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()),
        crate::consensus_life::SlotView::Top(t) if !t.ended && t.env.identity() == env.identity()
    );
    out.push(marker);
    if still_top {
        out.push(make_gossip_update(
            &ctx.node_id, ctx.default_ttl,
            Arc::from(format!("{}{}", consensus_ns::LEASE, slot).as_str()),
            encode_lease_ms(0), false, &ctx.hlc,
        ));
    }
    let tls = ctx.tls.get().map(std::sync::Arc::as_ref);
    for upd in &out {
        apply_and_notify(&ctx.kv_state, upd);
        dispatch_gossip_try_send(
            &ctx.gossip_txs, make_kv_wire_msg(upd.clone(), ctx.node_id.id_hash(), tls),
            ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
        );
    }
    out
}

/// A lock's release from `Drop`: [`release_envelope`], handed to the WAL fire-and-forget — the lease
/// is the backstop if a crash loses it.
pub(crate) fn release_envelope_try(ctx: &TaskCtx, slot: &str, env: &crate::consensus_life::Envelope) {
    for upd in release_envelope(ctx, slot, env) {
        if let Some(wal) = ctx.wal.get() {
            wal.append_try(sync_entry_from(&upd));
        }
    }
}

/// **Step `value` down from `slot`** — `release_leadership`'s path (design §5). Releases iff this
/// node reads a live decision of `value` **originally proposed by this node**, and puts the release on
/// stable storage (`append_sync`) before answering `Released`. A slot with no decision record (decided
/// by a node older than row A) is released the legacy way — its committed entry and lease tombstoned —
/// which keeps K1's residual for that decision only (design §7).
pub(crate) async fn release_decision_durable(ctx: &TaskCtx, slot: &str, value: &Bytes) -> ReleaseOutcome {
    use crate::consensus_life::SlotView;
    let updates = match crate::consensus_life::read_slot(&ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()) {
        SlotView::Top(top) => {
            if top.ended || top.value.as_ref() != Some(value) || top.env.proposer != ctx.node_id {
                return ReleaseOutcome::Refused;
            }
            release_envelope(ctx, slot, &top.env)
        }
        SlotView::Unknown => return ReleaseOutcome::Refused,
        SlotView::NoRecord => {
            if legacy_live(&ctx.kv_state, slot, causal_now_ms(&ctx.hlc)).map(|(v, _)| v).as_ref() != Some(value) {
                return ReleaseOutcome::Refused;
            }
            // The legacy release: the committed entry and its lease tombstoned, as an older node
            // releases — K1's residual stays for this decision only (design §7).
            let tls = ctx.tls.get().map(std::sync::Arc::as_ref);
            [consensus_ns::COMMITTED, consensus_ns::LEASE].iter().map(|ns| {
                let upd = make_gossip_update(
                    &ctx.node_id, ctx.default_ttl,
                    Arc::from(format!("{ns}{slot}").as_str()), Bytes::new(), true, &ctx.hlc,
                );
                apply_and_notify(&ctx.kv_state, &upd);
                dispatch_gossip_try_send(
                    &ctx.gossip_txs, make_kv_wire_msg(upd.clone(), ctx.node_id.id_hash(), tls),
                    ctx.node_id.id_hash(), ForwardHint::All, &ctx.kv_state.dropped_frames,
                );
                upd
            }).collect()
        }
    };
    let Some(wal) = ctx.wal.get() else { return ReleaseOutcome::Released };
    for upd in &updates {
        if let Err(e) = wal.append_sync(sync_entry_from(upd)).await {
            tracing::error!(slot = %slot, error = %e, "consensus: a release did not reach stable storage on this node");
            return ReleaseOutcome::Unrecorded;
        }
    }
    ReleaseOutcome::Released
}

/// What [`release_decision_durable`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseOutcome {
    /// Released, and on stable storage (or no persistence is configured).
    Released,
    /// Nothing was written: this node does not read a live decision of the value that it proposed.
    Refused,
    /// Applied and gossiped, but the WAL did not acknowledge it: a crash may restore the leadership
    /// on this node (the gateway answers 500 `release_unrecorded`, not 404).
    Unrecorded,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Counts how many distinct segment values appear at `depth` across `voters`.
/// Voters whose locality is `None`, or whose path is shorter than `depth + 1`,
/// contribute zero. Order-independent and unbiased.
pub(crate) fn distinct_domains_at_depth(
    voters: &AHashMap<NodeId, Option<LocalityPath>>,
    depth: usize,
) -> usize {
    let mut seen: AHashSet<&str> = AHashSet::new();
    for loc in voters.values().flatten() {
        if let Some(seg) = loc.value_at(depth) {
            seen.insert(seg.as_ref());
        }
    }
    seen.len()
}

/// Returns whether a Hard topology policy is satisfied by the current voter
/// set. Soft and policy-less calls always pass. The returned `usize` is the
/// current `distinct_domains` count — useful for populating
/// `ConsensusResult::TopologyUnsatisfied`.
pub(crate) fn evaluate_topology_gate(
    voters: &AHashMap<NodeId, Option<LocalityPath>>,
    policy: &GroupTopologyPolicy,
) -> (bool, usize) {
    if policy.enforcement != TopologyEnforcement::Hard {
        return (true, 0);
    }
    let Some(depth) = policy.spread_depth else {
        // Hard with no spread_depth is invalid config; validate() rejects this
        // at startup. Treat as pass to avoid stalling production after a
        // hot-reload bug.
        return (true, 0);
    };
    let distinct = distinct_domains_at_depth(voters, depth);
    (distinct >= policy.spread_min_distinct, distinct)
}

/// Context for mid-ballot opaque-member recomputation.
///
/// Injected by `group_propose` / `cluster_propose` at the signal-mesh call site so
/// `propose` does not read `KvState` directly — the opacity query strategy is an
/// injected dependency, not a consensus-engine concern.
pub(crate) struct OpaqueRecompute {
    /// Total group/system member count at proposal time (before opacity exclusions).
    pub(crate) total_members: usize,
    /// Operator-configured quorum size (0 = auto-majority from active count).
    pub(crate) config_quorum: usize,
    /// Callback that returns the current number of opaque members on each call.
    pub(crate) count_opaque:  Arc<dyn Fn() -> usize + Send + Sync>,
}

// ── ConsensusEngine ──────────────────────────────────────────────────────────
//
// Shared context for both the voter/listener task and the proposer.
// Constructed by GossipAgent::start_consensus_listener and
// ConsensusHandle::group_propose / cluster_propose, then either spawned
// (spawn_listener) or driven directly (propose).

/// Bundles the Arc fields needed for consensus tasks.
///
/// Replaces the former `ConsensusListenerCtx` that was private to `agent.rs`.
/// Infrastructure fields are shared via `Arc<TaskCtx>` to avoid cloning them
/// individually at each spawn site.
pub(crate) struct ConsensusEngine {
    pub(crate) task_ctx:            Arc<TaskCtx>,
    /// When `true`, this node silently abstains from voting while any pheromone
    /// trail under `sys/load/{node_id}/` shows `is_opaque: true`.
    pub(crate) abstain_when_opaque: bool,
    /// When `true`, the proposer filters incoming votes against its declared
    /// trust slice for the group (`consensus/trust/{group}/{node_id}`).
    pub(crate) use_trust_slices:    bool,
    pub(crate) max_abstain_ballots: u32,
    /// This node's locality, captured at engine-construction time from
    /// `GossipConfig::locality_path`. Used by the voter task to populate
    /// `VoteWithLocality`. `None` when the node has no configured locality.
    pub(crate) self_locality:       Option<LocalityPath>,
    /// Per-group topology policy for the proposer side. `None` means no
    /// policy: ballots commit on quorum without a diversity check.
    /// `Some(Soft)` policies still skip the gate — they affect fan-out
    /// scoring only. `Some(Hard)` enables the topology gate in `propose`.
    pub(crate) topology_policy:     Option<GroupTopologyPolicy>,
}

impl ConsensusEngine {
    // ── KV helpers ───────────────────────────────────────────────────────────
    //
    // ConsensusEngine writes directly to the KV substrate (`apply_and_notify`,
    // `dispatch_gossip_*`, `make_gossip_update`) for its own `consensus/ballot/*`
    // and `consensus/committed/*` namespace. This is intentional, not a layer violation:
    //
    // 1. `GossipAgent::set` cannot be used here — it would create a reference cycle
    //    (`ConsensusEngine` → `GossipAgent` → task handles → `ConsensusEngine`),
    //    so `ConsensusEngine` receives only `Arc<TaskCtx>` with no agent back-reference.
    //
    // 2. Ownership of `consensus/*` is analogous to the opacity governor owning
    //    `sys/load/*`: the module writes directly to its documented KV prefix because
    //    the data is architectural state, not user data.
    //
    // 3. Consensus uses exactly the same KV primitives (`make_gossip_update` +
    //    `apply_and_notify`) that every other subsystem uses — no additional coupling
    //    to the transport layer is introduced.

    fn get(&self, key: &str) -> Option<Bytes> {
        self.task_ctx.kv_state.store.pin().get(key).and_then(|e| e.data.clone())
    }

    /// The slot's shared ballot key (`consensus/ballot/{slot}`), as this node holds it — read by every
    /// ballot draw. Any member writes it, so it gets the floor's tripwire too
    /// ([`note_implausible`](Self::note_implausible)): a ballot key at `u64::MAX` exhausts the slot
    /// as surely as a floor there (the adversarial review of #591, finding 3). Nothing about what is
    /// returned changes.
    fn read_ballot(&self, ballot_key: &str) -> u64 {
        let ballot = self.get(ballot_key).map(|b| decode_ballot(&b)).unwrap_or(0);
        if ballot > DECIDED_FLOOR_ANOMALY_MARGIN
            && let Some(slot) = ballot_key.strip_prefix(consensus_ns::BALLOT)
        {
            self.note_implausible(slot, ballot, "ballot key");
        }
        ballot
    }

    /// The **digest** of the slot's live decision, for every refusal (row A): a live decision whose
    /// large value has not arrived yet is still a live decision (review D7), so the engine compares
    /// digests, never "is a value here".
    fn live_digest(&self, slot: &str) -> Option<[u8; 32]> {
        live_digest(&self.task_ctx.kv_state, slot, causal_now_ms(&self.task_ctx.hlc))
    }

    /// Applies a KV update from within a consensus task (`try_send` gossip; dropped frames recovered
    /// by anti-entropy), returning the applied update so the caller can WAL-append the
    /// exact entry — apply to the store first, then hand the record to the WAL, never the reverse.
    fn kv_set_returning(&self, key: String, value: Bytes) -> GossipUpdate {
        let tc  = &self.task_ctx;
        let upd = make_gossip_update(&tc.node_id, tc.default_ttl, Arc::from(key.as_str()), value, false, &tc.hlc);
        apply_and_notify(&tc.kv_state, &upd);
        let tls = tc.tls.get().map(std::sync::Arc::as_ref);
        let msg = make_kv_wire_msg(upd.clone(), tc.node_id.id_hash(), tls);
        dispatch_gossip_try_send(
            &tc.gossip_txs, msg,
            tc.node_id.id_hash(), ForwardHint::All, &tc.kv_state.dropped_frames,
        );
        upd
    }

    /// The ballot this slot's most recent commit was decided at, as this node knows it; `0` when
    /// none — see [`consensus_ns::DECIDED`].
    ///
    /// Every reader of the floor comes through here — the acceptor's prepare and vote, the
    /// proposer's ballot draw and phase-1 collection, the commit path, `record_decided` — so this is
    /// where the floor's tripwire sits ([`note_implausible`](Self::note_implausible)).
    /// It changes nothing about what is returned or refused.
    fn decided_floor(&self, slot: &str) -> u64 {
        let floor = self.get(&format!("{}{}", consensus_ns::DECIDED, slot)).map(|b| decode_ballot(&b)).unwrap_or(0);
        // Cheap guard first: no floor at or below the margin can be implausible.
        if floor > DECIDED_FLOOR_ANOMALY_MARGIN {
            self.note_implausible(slot, floor, "decided floor");
        }
        floor
    }

    /// The ballot tripwire — **detection, not prevention**, in Layer III (core's apply path never
    /// learns consensus's conventions). `consensus/decided/{slot}` is written by whichever node
    /// commits and `consensus/ballot/{slot}` by any proposer or voter, so neither is self-owned and
    /// the `sys/` tripwire cannot cover them; but a member that writes `u64::MAX` into either makes the
    /// slot refuse every later ballot. A `value` (the floor or the ballot key, named by `what`) more
    /// than [`DECIDED_FLOOR_ANOMALY_MARGIN`] above the highest ballot this node has **itself**
    /// observed for the slot is counted once per slot (`SystemStats::consensus_decided_floor_anomalies`,
    /// `mycelium_consensus_decided_floor_anomalies_total`) and warned about once. The value is still
    /// obeyed.
    ///
    /// "Observed" is this node's acceptor memory — the ballot it promised (never below what it
    /// accepted) — and the ballots of verified COMMITs it processed
    /// ([`note_verified_ballot`](Self::note_verified_ballot)). **Not** the shared ballot key: a
    /// member writes that too, so measuring against it let a forger raise both keys together and pass
    /// uncounted (the adversarial review of #591, finding 3).
    ///
    /// Bounded: at most [`ANOMALY_SLOTS_CAP`] slots are remembered; an anomaly on a slot past the cap
    /// is counted on `decided_floor_anomalies_unrecorded` instead (each read, since it cannot be
    /// deduplicated), and still warned about.
    fn note_implausible(&self, slot: &str, value: u64, what: &str) {
        let promised = self.task_ctx.consensus_accepted.pin().get(slot).map(|s| s.promised).unwrap_or(0);
        let verified = self.task_ctx.consensus_verified_ballots.pin().get(slot).copied().unwrap_or(0);
        let observed = promised.max(verified);
        if !decided_floor_is_implausible(value, observed) {
            return;
        }
        let slots = self.task_ctx.decided_floor_anomaly_slots.pin();
        let first = if slots.contains(slot) {
            false
        } else if slots.len() >= ANOMALY_SLOTS_CAP {
            self.task_ctx.decided_floor_anomalies_unrecorded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            slots.insert(Arc::from(slot))
        };
        if first {
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_consensus_decided_floor_anomalies_total").increment(1);
            tracing::warn!(
                slot = %slot, value, observed, what,
                "consensus: the slot's {what} is far above every ballot this node has observed — a forged \
                 consensus/ entry? Obeyed as before; counted once per slot \
                 (see SystemStats::consensus_decided_floor_anomalies)"
            );
        }
    }

    /// Records the ballot of a verified consensus message this node processed (a COMMIT) as
    /// observed for its slot — the evidence [`note_implausible`](Self::note_implausible) measures a
    /// floor or ballot key against. Kept as the maximum; bounded at [`VERIFIED_BALLOTS_CAP`] slots
    /// (past it a new slot is not recorded, which can only make the tripwire more sensitive there).
    /// `compute` with a pure closure, so a CAS retry is harmless.
    fn note_verified_ballot(&self, slot: &Arc<str>, ballot: u64) {
        let map = self.task_ctx.consensus_verified_ballots.pin();
        if !map.contains_key(slot) && map.len() >= VERIFIED_BALLOTS_CAP {
            return;
        }
        map.compute(Arc::clone(slot), |existing| match existing {
            Some((_, &seen)) if seen >= ballot => papaya::Operation::Abort(()),
            _ => papaya::Operation::Insert(ballot),
        });
    }

    /// **The legacy reading says the slot's decision is over**: the slot has no decision record (no
    /// record, no sentinel — design §2.3), and its committed entry is present, data or tombstone, and
    /// not live. An absent entry is *not* over: the commit may not have arrived yet.
    fn legacy_over(&self, slot: &str) -> bool {
        if !matches!(crate::consensus_life::read_slot(&self.task_ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()),
                     crate::consensus_life::SlotView::NoRecord) {
            return false;
        }
        let present = self.task_ctx.kv_state.store.pin()
            .get(format!("{}{}", consensus_ns::COMMITTED, slot).as_str()).is_some();
        present && legacy_live(&self.task_ctx.kv_state, slot, causal_now_ms(&self.task_ctx.hlc)).is_none()
    }

    /// The ballot of `slot`'s top decision record when it has ended (design §2.3); `0` when none. A
    /// member can write a record too, so a ballot far above every one this node observed is counted by
    /// the floor's tripwire ([`note_implausible`](Self::note_implausible)) — detection only.
    fn ended(&self, slot: &str) -> u64 {
        let ended = ended_at(&self.task_ctx.kv_state, slot).unwrap_or(0);
        if ended > DECIDED_FLOOR_ANOMALY_MARGIN {
            self.note_implausible(slot, ended, "decision record's ballot");
        }
        ended
    }

    /// **Build this proposal's decision envelope** (design §2.1, §4): the term from the config — a
    /// lease's `expires_at_ms` on this node's **wall clock** now (Q1); the fencing token — `tick()`
    /// after observing the top record's token; the lineage — this proposal's first ballot, or, for a
    /// **renewal**, the lineage of this node's own live decision of the same value (Q2: only the
    /// original proposer renews).
    fn build_carry(&self, slot: &str, raw: &Bytes, lease_ms: Option<u64>, first_ballot: u64) -> Carry {
        use crate::consensus_life::{Envelope, SlotView, Term};
        let me = self.task_ctx.node_id.clone();
        let term = match lease_ms {
            Some(ms) => Term::Lease { ms, expires_at_ms: mycelium_core::sim_seam::wall_now_ms().saturating_add(ms) },
            None => Term::Permanent,
        };
        let view = crate::consensus_life::read_slot(&self.task_ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms());
        let mut renewing = None;
        if let SlotView::Top(top) = &view {
            self.task_ctx.hlc.observe(top.env.token);
            if !top.ended && lease_ms.is_some() && top.value.as_ref() == Some(raw) && top.env.proposer == me {
                renewing = Some(top.env.lineage);
            }
        }
        let token = self.task_ctx.hlc.tick();
        let lineage = renewing.unwrap_or(first_ballot);
        let legacy_term = match term {
            Term::Lease { .. } => term,
            Term::Permanent => Term::Lease {
                ms: LEGACY_ADOPTION_LEASE_MS,
                expires_at_ms: mycelium_core::sim_seam::wall_now_ms().saturating_add(LEGACY_ADOPTION_LEASE_MS),
            },
        };
        Carry { own: Envelope::new(slot, raw.clone(), term, lineage, me, token), renewing, legacy_term }
    }

    /// The ballot at or below which this node refuses `slot`'s prepares and accepts, and above which
    /// it draws: the decided floor, or an ended decision's ballot if that is higher (row A).
    fn floor(&self, slot: &str) -> u64 {
        self.decided_floor(slot).max(self.ended(slot))
    }

    /// Records that `slot` was decided at `ballot`, never lowering what is recorded — on stable
    /// storage (`persist_sync`): the floor is what makes acceptors refuse a ballot at or below a
    /// decision after a restart, so a floor a restart forgets is a decision a stale proposal can
    /// re-open. Returns whether it reached the WAL (`true` when nothing changed or nothing was
    /// promised).
    async fn record_decided(&self, slot: &str, ballot: u64) -> bool {
        if ballot <= self.decided_floor(slot) { return true; }
        let upd = self.kv_set_returning(format!("{}{}", consensus_ns::DECIDED, slot), encode_ballot(ballot));
        self.persist_sync(slot, &upd, "decided ballot").await
    }

    /// Writes `ballot` to the slot's shared ballot key unless a higher one is already there, so
    /// proposers drawing their next ballot start above it. Handed to the WAL fire-and-forget
    /// (`append_try`): the key is a liveness aid — a proposer that restarts without it draws a
    /// ballot an acceptor's promise refuses, and moves up — so it is not worth an fsync per ballot.
    async fn raise_ballot(&self, ballot_key: &str, ballot: u64) {
        if ballot > self.read_ballot(ballot_key) {
            let upd = self.set_async(ballot_key, encode_ballot(ballot)).await;
            self.wal_try(&upd);
        }
    }

    /// Hands an applied update to the WAL fire-and-forget (`append_try`) — the ballot key's rung,
    /// from the proposer's `raise_ballot` and the voter's write alike (the voter's used to reach no
    /// WAL at all; the adversarial review of #585, F7).
    fn wal_try(&self, upd: &GossipUpdate) {
        if let Some(wal) = self.task_ctx.wal.get() {
            wal.append_try(sync_entry_from(upd));
        }
    }

    /// Writes this node's acceptor state for `slot` to its durable record — the promise and the
    /// acceptance together, read back from the shared memory so the record is never older than
    /// the change that prompted it.
    ///
    /// The listener and this node's proposer can both change a slot's state; each writes what it
    /// reads *after* its change, and re-reads after writing — if the state moved meanwhile, the
    /// newer one is written again, so the record cannot be left behind a vote or promise that has
    /// already left. Bounded: a slot changing faster than four writes is written as of the last.
    ///
    /// **On stable storage before this returns `true`** (`persist_sync`, a forced `fdatasync`):
    /// the record is what `prewarm_accepted` restores at the next start, and until 2.32.0 it was
    /// applied to the store and gossiped but never handed to the WAL, so a node that crashed after
    /// promising or voting restarted with no memory of it — `lib_tests::
    /// an_acceptors_record_survives_a_crash_without_a_snapshot`. Every caller refuses to let the
    /// promise or vote leave on `false`: an unrecorded acceptance is one a restart lets this node
    /// contradict. `true` when there is nothing to record or nothing was promised (no WAL).
    async fn persist_acceptor(&self, slot: &Arc<str>) -> bool {
        let key = accepted_key(&self.task_ctx.node_id, slot);
        let read = || self.task_ctx.consensus_accepted.pin().get(slot).map(encode_acceptor);
        let mut written: Option<GossipUpdate> = None;
        {
            // Serialised with every other writer of this node's acceptor records — the listener, the
            // proposer and the collector's shrink (row A, C2) — so each write carries the memory as it
            // is at that write, and a shrunk record cannot land after, and erase, a promise made since.
            let _records = self.task_ctx.acceptor_records.lock().unwrap_or_else(|e| e.into_inner());
            let mut current = read();
            for _ in 0..4 {
                let Some(bytes) = current else { break };
                written = Some(self.kv_set_returning(key.clone(), bytes.clone()));
                let after = read();
                if after.as_ref() == Some(&bytes) { break; }
                current = after;
            }
        }
        match written {
            Some(upd) => self.persist_sync(slot, &upd, "acceptor record (promise, acceptance)").await,
            None => true,
        }
    }

    /// **Collect acceptor state whose decision is over** (row A, C2: *mandate TTL applies to decisions
    /// too*; design §8). Returns how many slots were collected.
    ///
    /// The exact condition, per slot: this node's top decision record for it has **ended** at ballot
    /// `e` (released, or its lease past on this node's wall clock), and the acceptor promises nothing
    /// above `e` — re-checked inside the shrink's compare-and-set, so a promise to a ballot that may be
    /// in flight is never touched. The node first puts the top record (and its marker) on stable
    /// storage — a learner's copy is otherwise only gossip-durable — then **shrinks** its memory and its
    /// own durable acceptor record to `{promised: e}`: no proposer, no acceptance. The floor that refuses
    /// every ballot `≤ e` is therefore this node's **own** record, fsynced (`persist_acceptor`), never
    /// the shared `consensus/decided/` key, which last-writer-wins can regress (review finding 5).
    ///
    /// A permanent decision is never collected until released. Bounded per pass
    /// ([`ACCEPTOR_COLLECT_BUDGET`]), candidates sorted by slot and rotated by a seam-drawn offset so
    /// failing slots cannot starve the rest and a replay selects the same ones.
    pub(crate) async fn collect_finished(&self) -> usize {
        // Cheap filter first (review D3): a shrunk state (no acceptance) is never walked again, and
        // `ended_at` — a scan of the slot's records — runs only for the rotated window, at most
        // `ACCEPTOR_COLLECT_SCAN` slots a pass.
        let mut walk: Vec<(Arc<str>, u64)> = self.task_ctx.consensus_accepted.pin().iter()
            .filter(|(_, state)| state.accepted.is_some())
            .map(|(slot, state)| (Arc::clone(slot), state.promised))
            .collect();
        walk.sort_by(|a, b| a.0.cmp(&b.0));
        if !walk.is_empty() {
            let start = mycelium_core::sim_seam::rng_u64_below("consensus/collect", walk.len() as u64) as usize;
            walk.rotate_left(start);
        }
        let candidates: Vec<(Arc<str>, u64)> = walk.into_iter()
            .take(ACCEPTOR_COLLECT_SCAN)
            .filter_map(|(slot, promised)| {
                let ended = ended_at(&self.task_ctx.kv_state, &slot)?;
                (promised <= ended).then_some((slot, ended))
            })
            .take(ACCEPTOR_COLLECT_BUDGET)
            .collect();
        let mut collected = 0;
        for (slot, ended) in candidates {
            if !self.sync_top_record(&slot).await {
                continue;
            }
            if self.shrink_acceptor(&slot, ended) {
                if !self.persist_acceptor(&slot).await {
                    continue;
                }
                collected += 1;
            }
        }
        collected
    }

    /// Hands this node's top decision record for `slot`, and its release marker if it holds one, to
    /// the WAL with `append_sync` (review finding 5): collection must not rely on a record a crash
    /// could take back. `true` when nothing was promised (no WAL).
    async fn sync_top_record(&self, slot: &str) -> bool {
        let Some(wal) = self.task_ctx.wal.get() else { return true };
        let crate::consensus_life::SlotView::Top(top) =
            crate::consensus_life::read_slot(&self.task_ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms())
        else { return false };
        let keys = [
            crate::consensus_life::sentinel_key(slot),
            crate::consensus_life::record_key(slot, top.ballot),
            crate::consensus_life::marker_key(slot, top.env.lineage, &top.env.proposer),
        ];
        for key in keys {
            let Some(entry) = self.task_ctx.kv_state.store.pin().get(key.as_str()).cloned() else { continue };
            let Some(value) = entry.data else { continue };
            let sync = crate::framing::SyncEntry {
                key: Arc::from(key.as_str()), value, timestamp: entry.timestamp, is_tombstone: false,
            };
            if wal.append_sync(sync).await.is_err() { return false; }
        }
        true
    }

    /// Shrinks `slot`'s acceptor memory to `{promised: ended}` if it still promises nothing above
    /// `ended` — a compare-and-set, retry-safe. `true` when it shrank; the caller then writes the
    /// durable record from memory (`persist_acceptor`, under `acceptor_records`), so whatever is in
    /// memory when the record is written — the shrunk state, or a promise made since — is what lands.
    fn shrink_acceptor(&self, slot: &Arc<str>, ended: u64) -> bool {
        let memory = self.task_ctx.consensus_accepted.pin();
        matches!(
            memory.compute(Arc::clone(slot), |entry| match entry {
                Some((_, state)) if state.promised <= ended && state.accepted.is_some() =>
                    papaya::Operation::Insert(AcceptorSlot { promised: ended, promised_to: Some(FLOOR_SENTINEL), accepted: None }),
                _ => papaya::Operation::Abort(()),
            }),
            papaya::Compute::Updated { .. },
        )
    }

    /// [`shrink_acceptor`](Self::shrink_acceptor), for the re-check test.
    #[cfg(test)]
    pub(crate) fn shrink_acceptor_for_test(&self, slot: &Arc<str>, ended: u64) -> bool {
        self.shrink_acceptor(slot, ended)
    }

    /// **Collect decision records and markers** (design §8), on the collector's tick: for up to
    /// [`ACCEPTOR_COLLECT_BUDGET`] slots (sorted, seam-rotated), tombstone every decision record below
    /// the top (the tombstone sweep then removes them), content-addressed values no remaining record
    /// names, and **lease** markers whose expiry plus `max_clock_drift_ms` has passed and whose lineage
    /// is not the top's — in sorted key order, so a replay writes the same sequence. Never the top
    /// record, the slot's sentinel, a permanent marker (review finding 1) or the top lineage's marker.
    /// Steady state per slot: the sentinel, the top record, the top's marker.
    pub(crate) async fn collect_records(&self) -> usize {
        let wall = mycelium_core::sim_seam::wall_now_ms();
        let drift = self.task_ctx.config.max_clock_drift_ms;
        let mut slots = crate::consensus_life::slots_with_records(&self.task_ctx.kv_state);
        if slots.len() > ACCEPTOR_COLLECT_BUDGET {
            let start = mycelium_core::sim_seam::rng_u64_below("consensus/collect-records", slots.len() as u64) as usize;
            slots.rotate_left(start);
            slots.truncate(ACCEPTOR_COLLECT_BUDGET);
        }
        let mut touched = 0;
        for slot in slots {
            let Some(c) = crate::consensus_life::collectable(&self.task_ctx.kv_state, &slot, wall, drift) else { continue };
            if c.dead_keys.is_empty() { continue; }
            // The top, its marker and the sentinel on stable storage **before** anything below them is
            // tombstoned (the review of `ee3d9e4b`, finding 5): a crash between would otherwise leave
            // this node holding neither — `Unknown` until anti-entropy repaired it.
            if !self.sync_top_record(&slot).await { continue; }
            for key in c.dead_keys {
                let upd = self.kv_delete(&key);
                self.wal_try(&upd);
                touched += 1;
            }
        }
        touched
    }

    /// Tombstones `key` in the KV store and gossips the deletion.
    /// Returns the applied tombstone so the caller can hand it to the WAL.
    fn kv_delete(&self, key: &str) -> GossipUpdate {
        let tc  = &self.task_ctx;
        let upd = make_gossip_update(&tc.node_id, tc.default_ttl, Arc::from(key), Bytes::new(), true, &tc.hlc);
        apply_and_notify(&tc.kv_state, &upd);
        let tls = tc.tls.get().map(std::sync::Arc::as_ref);
        let msg = make_kv_wire_msg(upd.clone(), tc.node_id.id_hash(), tls);
        dispatch_gossip_try_send(
            &tc.gossip_txs, msg,
            tc.node_id.id_hash(), ForwardHint::All, &tc.kv_state.dropped_frames,
        );
        upd
    }

    /// Like `kv_set` but awaits channel capacity (used by the proposer).
    /// Returns the applied update so the caller can WAL-append the exact entry.
    async fn set_async(&self, key: &str, value: Bytes) -> GossipUpdate {
        let tc  = &self.task_ctx;
        let upd = make_gossip_update(&tc.node_id, tc.default_ttl, Arc::from(key), value, false, &tc.hlc);
        apply_and_notify(&tc.kv_state, &upd);
        let tls = tc.tls.get().map(std::sync::Arc::as_ref);
        let msg = make_kv_wire_msg(upd.clone(), tc.node_id.id_hash(), tls);
        dispatch_gossip_send(
            &tc.gossip_txs, msg,
            tc.node_id.id_hash(), ForwardHint::All,
        ).await;
        upd
    }

    // ── Signal-mesh bridge helpers ───────────────────────────────────────────

    /// True if any `sys/load/{node_id}/*` pheromone entry is `is_opaque`.
    ///
    /// Delegates to the opacity helper in `agent::opacity` so this type
    /// does not scan `KvState` directly.
    fn is_overloaded(&self) -> bool {
        crate::agent::opacity::is_self_opaque(&self.task_ctx.kv_state, &self.task_ctx.node_id)
    }

    fn emit(&self, kind: Arc<str>, scope: SignalScope, payload: Bytes) {
        emit_signal(&self.task_ctx, kind, scope, payload);
    }

    /// Evaluates the Hard topology gate against the current voter set, honouring
    /// any `sys/topology-override/{group}` operator override. Returns:
    /// - `passes`: whether the proposer may commit on this voter set
    /// - `distinct_domains`: count at `spread_depth` (0 when no Hard policy)
    /// - `policy_meta`: `Some((spread_depth, spread_min_distinct))` only when a
    ///   Hard policy is actually being enforced — used to populate
    ///   `ConsensusResult::TopologyUnsatisfied`.
    ///
    /// **`sys/topology-override/{group}` value format**: the override is active
    /// when the KV value is exactly the ASCII bytes `b"true"`. Any other value
    /// (including absent) leaves Hard enforcement in effect. Operators writing
    /// `b"false"` or empty bytes will *not* disable enforcement — this guards
    /// against fat-finger overrides where the presence of the key alone would
    /// otherwise be load-bearing.
    fn topology_check(
        &self,
        voters:     &AHashMap<NodeId, Option<LocalityPath>>,
        group_name: Option<&str>,
    ) -> (bool, usize, Option<(usize, usize)>) {
        let Some(policy) = self.topology_policy.as_ref() else { return (true, 0, None); };
        if policy.enforcement != TopologyEnforcement::Hard { return (true, 0, None); }
        if let Some(name) = group_name {
            let override_key = format!("sys/topology-override/{}", name);
            if let Some(value) = self.get(&override_key)
                && value.as_ref() == b"true" {
                    // Operator override — degrade to no-gate behaviour.
                    return (true, 0, None);
                }
        }
        let (passes, distinct) = evaluate_topology_gate(voters, policy);
        let meta = (policy.spread_depth.unwrap_or(0), policy.spread_min_distinct);
        (passes, distinct, Some(meta))
    }

    async fn emit_async(&self, kind: Arc<str>, scope: SignalScope, payload: Bytes) -> bool {
        emit_signal_async(&self.task_ctx, kind, scope, payload).await
    }

    // ── Payload signing / verification ───────────────────────────────────────

    /// Wraps `bytes` in a `SignedConsensusMsg` when TLS is active; returns
    /// `bytes` unchanged when TLS is disabled (zero overhead on the non-TLS path).
    /// The signature is over [`consensus_signing_message`] of `bytes` — domain-tagged — while
    /// `msg_bytes` carries `bytes` as they are, so the frame is unchanged (wire v12).
    pub(crate) fn sign_payload(&self, bytes: Bytes) -> Bytes {
        self.sign_payload_as(bytes, SignatureForm::Tagged)
    }

    /// [`sign_payload`](Self::sign_payload) in the given form: `Untagged` signs the payload bare,
    /// as a 2.31 node does, for an answer to a request that arrived bare (see [`SignatureForm`]).
    /// Only an acceptor's answers take `Untagged`; a proposer's own messages are always tagged.
    fn sign_payload_as(&self, bytes: Bytes, form: SignatureForm) -> Bytes {
        #[cfg(not(feature = "tls"))]
        let _ = form;
        #[cfg(feature = "tls")]
        if let Some(tls) = self.task_ctx.tls.get() {
            let sig = match form {
                SignatureForm::Tagged => crate::tls::sign_bytes(&tls.signing_key(), &consensus_signing_message(&bytes)),
                SignatureForm::Untagged => crate::tls::sign_bytes(&tls.signing_key(), &bytes),
            };
            let signed = SignedConsensusMsg {
                msg_bytes:  bytes.clone(),
                signer:     self.task_ctx.node_id.clone(),
                signature:  sig.to_vec(),
            };
            if let Ok(encoded) = mycelium_core::serde_fixint::to_vec(&signed) {
                return Bytes::from(encoded);
            }
        }
        bytes
    }

    /// Decodes `payload` as a `ConsensusMsg`, verifying its Ed25519 signature
    /// first when TLS is enabled. Returns `None` on bad signature or decode failure.
    fn decode_verify(&self, payload: &Bytes) -> Option<ConsensusMsg> {
        self.decode_verify_form(payload).map(|(msg, _)| msg)
    }

    /// [`decode_verify`](Self::decode_verify), also saying which [`SignatureForm`] verified — so an
    /// acceptor can answer in the same form. Without TLS every message reads as `Tagged`.
    fn decode_verify_form(&self, payload: &Bytes) -> Option<(ConsensusMsg, SignatureForm)> {
        #[cfg(feature = "tls")]
        if self.task_ctx.tls.get().is_some() {
            let signed: SignedConsensusMsg =
                mycelium_core::serde_fixint::from_slice(payload).ok()?;
            // Look up the sender's verifying key SET (WS5 retained keys): the
            // in-memory cache first, else parse the `sys/identity/` KV entry
            // (32 = one key, 64 = current‖previous). Verify against any so a
            // rotated key still validates in-flight/historical consensus msgs.
            let mut key_set: Vec<[u8; 32]> =
                self.task_ctx.peer_keys.pin().get(&signed.signer).cloned().unwrap_or_default();
            if key_set.is_empty() {
                let kv_key = format!("{}{}", crate::signal::kv_ns::IDENTITY, signed.signer);
                if let Some(b) = self.task_ctx.kv_state.store.pin()
                    .get(kv_key.as_str()).and_then(|e| e.data.clone())
                {
                    key_set = crate::agent::helpers::parse_identity_keys(&b);
                }
            }
            // WS-D: exclude validly-revoked keys on the consensus verify path too. The revocation
            // module's guarantee — "a key signed by a revoked key fails verification cluster-wide" —
            // was applied only by `helpers::known_verifying_keys` (role-claim / audit paths); consensus
            // built its own `key_set` and skipped it, so a key an operator rotated away from AND revoked
            // still validated `Vote`/`Propose` messages, defeating the operator's remediation (audit
            // 2026-07-15 pass 3). Mirror `known_verifying_keys` here.
            #[cfg(feature = "compliance")]
            {
                let revoked = crate::agent::revocation::revoked_key_set(&self.task_ctx);
                if !revoked.is_empty() {
                    key_set.retain(|k| !revoked.contains(k));
                }
            }
            let form = verify_consensus_signature(&key_set, &signed.msg_bytes, &signed.signature);
            match form {
                Some(SignatureForm::Tagged) => {}
                Some(SignatureForm::Untagged) => {
                    // A peer not yet on 2.32.0. One release, counted — see `SignatureForm`.
                    UNTAGGED_CONSENSUS_SIGNATURES_ACCEPTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    #[cfg(feature = "metrics")]
                    metrics::counter!("mycelium_consensus_untagged_signatures_total").increment(1);
                    tracing::debug!(
                        signer = %signed.signer,
                        "accepted an untagged (pre-2.32.0) consensus signature; this allowance closes \
                         in the next MINOR (docs/guide/deprecations.md §23)"
                    );
                }
                None => {
                    tracing::warn!("dropping consensus msg: bad/unknown signature from {}", signed.signer);
                    return None;
                }
            }
            let msg = decode_consensus_msg(&signed.msg_bytes)?;
            // Bind the message's CLAIMED authority to the VERIFIED signer. The signature proves only
            // that `signed.signer` produced the bytes — not that the `voter`/`proposer` named INSIDE
            // is that same node. Without this, one node holding one valid identity key could sign N
            // votes each naming a different `voter` and forge a quorum with zero peers agreeing
            // (audit 2026-07-15 pass 2). This closes the vote/propose impersonation vector.
            if !signer_authorized(&msg, &signed.signer) {
                tracing::warn!(
                    signer = %signed.signer,
                    "dropping consensus msg: signer does not match the vote/propose identity it claims",
                );
                return None;
            }
            return Some((msg, form?));
        }
        decode_consensus_msg(payload).map(|msg| (msg, SignatureForm::Tagged))
    }

    // ── Proposer ─────────────────────────────────────────────────────────────

    /// Runs one full proposal attempt sequence for `slot`.
    ///
    /// Called by `ConsensusHandle::group_propose` and `ConsensusHandle::cluster_propose`.
    ///
    /// `opaque_recompute` — when `Some`, `propose` registers for `BOUNDARY_OPAQUE` signals
    /// and re-evaluates the effective quorum size mid-ballot when any member transitions.
    /// The callback is built by the call site so `propose` does not read `KvState`
    /// directly — the opacity query strategy is an injected dependency.
    /// [`propose_inner`](Self::propose_inner), with one translation: a commit of a value **other
    /// than the caller's** — adopted in phase 1 from what a quorum had already accepted — is
    /// reported as [`Superseded`](ConsensusResult::Superseded). The slot was decided, and not for
    /// what the caller asked; a caller acting on `Committed` would otherwise act on its own value
    /// while the slot holds another (second review, M3).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn propose(
        &self,
        scope:             SignalScope,
        slot:              Arc<str>,
        value:             Bytes,
        quorum_size:       usize,
        config:            ConsensusConfig,
        opaque_recompute:  Option<OpaqueRecompute>,
    ) -> ConsensusResult {
        let asked = value.clone();
        own_or_superseded(
            self.propose_inner(scope, slot, value, quorum_size, config, opaque_recompute).await,
            &asked,
        )
    }

    async fn propose_inner(
        &self,
        scope:             SignalScope,
        slot:              Arc<str>,
        value:             Bytes,
        quorum_size:       usize,
        config:            ConsensusConfig,
        opaque_recompute:  Option<OpaqueRecompute>,
    ) -> ConsensusResult {
        let ballot_key = format!("{}{}", consensus_ns::BALLOT, &*slot);
        let commit_key = format!("{}{}", consensus_ns::COMMITTED, &*slot);

        // **A proposer counts itself, so it must be in the roster it counts itself toward.** The
        // promise and the vote below are inserted for this node unconditionally; the quorum came
        // from the group's roster. Checked here, at the one door both the library and the gateway
        // reach (`ConsensusHandle::group_propose`, `http::overlay_group_propose`), so no caller can
        // reach the counting for a group this node has not joined. A cluster proposal has no
        // roster: every node is in it. `cross_propose` never counts itself (its claim is a gate).
        if let SignalScope::Group(group) = &scope {
            let my_key = format!("{}{}", grp_prefix(group), self.task_ctx.node_id);
            if self.get(&my_key).is_none() {
                tracing::warn!(slot = %slot, group = %group, "consensus: refusing to propose to a group this node is not in");
                return ConsensusResult::NotAMember { slot, group: Arc::clone(group) };
            }
        }

        // The value this proposer is currently carrying. It starts as the caller's, and is
        // **replaced** by any higher-balloted accepted value an acceptor reports — accepted-value
        // preservation, without which a retry at `ballot + 1` can overwrite what a quorum already
        // accepted. `adopted_from` is the ballot the current value was accepted at (0 = ours).
        let mut adopted_from: u64 = 0;

        let mut quorum_size = quorum_size;
        // **P2: an exclusive outcome is decided by an electorate group, pinned by identity** (decision record
        // §8), checked here for the same reason as the membership above: the library and the gateway both reach
        // this door. It reads the caller's value, before it is wrapped in the decision envelope.
        let mut opaque_recompute = opaque_recompute;
        let tag = match self.electorate_door(&scope, &slot, &value, &config) {
            Ok(tag) => tag,
            Err(refused) => return refused.into_result(slot),
        };
        if let Some(t) = &tag {
            // The quorum is a strict majority of the members (a genesis: all of them), whatever the caller
            // computed — and opacity does not shrink it (#601's review, finding 3).
            quorum_size = quorum_size.max(t.quorum);
            opaque_recompute = None;
        }
        // Register for BOUNDARY_TRANSPARENT signals so we can re-evaluate quorum
        // mid-ballot when previously-opaque members become available.
        // Watch for BOUNDARY_OPAQUE: a member going opaque shrinks active_members,
        // which may lower the quorum threshold below votes already collected.
        let mut opaque_rx = if opaque_recompute.is_some() {
            Some(self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(signal_kind::BOUNDARY_OPAQUE), 8,
            ))
        } else {
            None
        };

        // Build trust set once before the ballot loop — it doesn't change mid-round.
        let trust_set: Option<AHashSet<u64>> = if self.use_trust_slices {
            if let SignalScope::Group(ref group_name) = scope {
                let key = format!("{}{}/{}", consensus_ns::TRUST, group_name, self.task_ctx.node_id);
                self.get(&key).and_then(|b| {
                    mycelium_core::serde_fixint::from_slice::<Vec<NodeId>>(&b).ok()
                }).map(|peers| peers.iter().map(|p| p.id_hash()).collect())
            } else {
                None
            }
        } else {
            None
        };
        // An electorate group's roster is the vote filter; a declared trust slice narrows it further.
        let trust_set = electorate_vote_filter(trust_set, tag.as_ref().map(|t| t.voters.clone()));

        // Above the slot's decided ballot too: the ballot key is no longer reset at commit, but it is
        // gossiped and may lag what this node knows was decided.
        let Some(mut ballot) = next_ballot(self.read_ballot(&ballot_key).max(self.floor(&slot))) else {
            return self.ballots_exhausted(slot, 0, quorum_size);
        };
        let mut votes_last_ballot: usize = 0;
        // How the latest attempt ended — what a timeout is labelled by (doc-coverage run 22, gap 3).
        let mut last = LastAttempt::None;
        // Captured topology-gate failure from the most recent ballot that
        // reached quorum-by-count but failed the Hard gate. Used to surface
        // `TopologyUnsatisfied` after all ballots are exhausted.
        let mut topology_failed: Option<(usize, usize, usize)> = None;

        // Epoch lease window (ms) for this proposal, if configured.
        let lease_ms = config.committed_lease_secs.map(|s| s.saturating_mul(1000));
        // A slot already committed with the *same* value under a lease may be
        // re-proposed while live — a successful re-commit refreshes the commit
        // timestamp (lease renewal). Any other live commitment supersedes us.
        // Takes the current value explicitly: the proposer may **adopt** a reported accepted value
        // between ballots, so a closure capturing `value` would pin it and go stale.
        let superseded_by_live = |existing: &[u8; 32], current: &Bytes| -> bool {
            !(lease_ms.is_some() && raw_of(current).map(|r| value_digest(&r)).as_ref() == Some(existing))
        };
        // Row A: the value Paxos decides is the decision envelope — the caller's value with its
        // window, lineage, proposer and token (design §2.1). Built once, at the first ballot.
        let carry = self.build_carry(&slot, &value, lease_ms, ballot);
        let mut value = carry.own.encode();

        // Extract the group name once for `sys/topology-override/{group}` lookups
        // and for completing the TopologyUnsatisfied return.
        let group_name: Option<Arc<str>> = match &scope {
            SignalScope::Group(g) => Some(Arc::clone(g)),
            _                     => None,
        };

        for _attempt in 0..config.max_ballots {
            if let Some(existing) = self.live_digest(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded {
                        slot,
                        ballot: self.read_ballot(&ballot_key),
                    };
                }

            // Early exit when all members are opaque — waiting for votes is futile.
            if let Some(ref or) = opaque_recompute {
                let opaque = (or.count_opaque)();
                if opaque >= or.total_members {
                    #[cfg(feature = "metrics")]
                    metrics::counter!("mycelium_consensus_timeouts_total", "reason" => "all_opaque")
                        .increment(1);
                    return ConsensusResult::Timeout {
                        slot,
                        ballots_tried: 0,
                        votes_last_ballot: 0,
                        quorum_required: quorum_size,
                    };
                }
            }

            // Register before emitting so no reply can arrive before we listen — one pair for both
            // phases, so a vote racing ahead of the last promise is not lost between them.
            let mut vote_rx = self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(consensus_kind::VOTE), 512,
            );
            let mut nack_rx = self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(consensus_kind::NACK), 64,
            );

            // **Phase 1: ask a quorum what it has accepted before proposing anything** — see
            // `ConsensusMsg::Prepare`. Whatever the promise quorum reports at its highest ballot
            // is what this ballot must carry.
            let needed = quorum_size;
            let phase1 = self.prepare_phase(
                &mut vote_rx, &mut nack_rx, &scope, &slot, ballot, &value,
                &|p: &AHashSet<NodeId>| p.len() >= needed,
                trust_set.as_ref(), tag.as_ref(), config.phase1_timeout, &carry,
            ).await;
            let retry_floor = match phase1 {
                Phase1::Ready(Phase1Choice::Keep) => None,
                Phase1::Ready(Phase1Choice::Adopt(ab, v)) => {
                    adopted_from = ab;
                    value = v;
                    None
                }
                Phase1::DecidedOtherwise => {
                    return ConsensusResult::Superseded {
                        slot,
                        ballot: self.read_ballot(&ballot_key),
                    };
                }
                Phase1::Ready(Phase1Choice::Blocked) => {
                    last = LastAttempt::Blocked;
                    Some(0)
                }
                Phase1::Short(others) => {
                    last = LastAttempt::Prepare { others };
                    Some(0)
                }
                Phase1::Refused(seen) => {
                    last = LastAttempt::Contended;
                    Some(seen)
                }
                Phase1::Unrecorded => return self.unrecorded(slot, _attempt + 1, quorum_size),
                Phase1::Stale(seen) => return stale_result(slot, &scope, tag.as_ref(), seen),
                Phase1::Closed => return gate_refused(slot, tag.as_ref()),
            };
            if let Some(floor) = retry_floor {
                ballot_retry_pause(config.ballot_retry_jitter_ms).await;
                let Some(next) = next_ballot(floor.max(self.read_ballot(&ballot_key)).max(ballot)) else {
                    return self.ballots_exhausted(slot, _attempt + 1, quorum_size);
                };
                ballot = next;
                continue;
            }
            last = LastAttempt::Vote;

            // Phase 2 listens on a fresh refusal channel: a refusal of this ballot's *prepare*, still
            // queued after the promise quorum formed, would otherwise abort a ballot already won.
            nack_rx = self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(consensus_kind::NACK), 64,
            );

            // **Claim this node's vote before proposing, not after.** Proposing a value *is*
            // accepting it, so it goes through this node's shared acceptor memory — the same gate
            // every other vote passes. Order matters as much as the check: claiming after the
            // broadcast would let this node propose a value it then discovers it may not vote for,
            // and the acceptors that had already accepted it would be holding that ballot against
            // the value this node actually owes its vote to.
            if let Some(existing) = self.live_digest(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
                }
            let floor = self.floor(&slot);
            // Through the electorate's gate, inside the same `compute` as the claim (P2 round 2, finding 1): a
            // retirement this node's acceptor promised since the door ends this proposal, by name.
            let gate = tag.as_ref().map(|t| &t.gate);
            let claimed = self.claim_gated(gate, || {
                claim_envelope(&self.task_ctx.consensus_accepted, &slot, ballot, &value, self.task_ctx.node_id.id_hash(), floor)
            });
            if claimed.is_none() && tag.is_some() {
                return gate_refused(slot, tag.as_ref());
            }
            if claimed != Some(true) {
                // Already committed to a different value at this ballot. Cannot win here; move up
                // rather than emit a proposal we are not entitled to support.
                last = LastAttempt::Contended;
                let Some(next) = next_ballot(ballot.max(self.read_ballot(&ballot_key))) else {
                    return self.ballots_exhausted(slot, _attempt + 1, quorum_size);
                };
                ballot = next;
                continue;
            }
            // Durable before the proposal leaves, for the same reason the voter records before its
            // vote leaves: a proposal is an acceptance, and an acceptance a restart forgets is one
            // this node can contradict.
            if !self.persist_acceptor(&slot).await {
                return self.unrecorded(slot, _attempt + 1, quorum_size);
            }

            self.raise_ballot(&ballot_key, ballot).await;

            let propose_msg = match &tag {
                Some(t) => ConsensusMsg::ProposeIn {
                    slot: Arc::clone(&slot), ballot, envelope: value.clone(),
                    proposer: self.task_ctx.node_id.clone(), epoch: t.epoch, electorate: t.digest,
                },
                None => ConsensusMsg::ProposeTerm {
                    slot: Arc::clone(&slot), ballot, envelope: value.clone(),
                    proposer: self.task_ctx.node_id.clone(),
                },
            };
            self.emit_async(
                Arc::from(consensus_kind::PROPOSE), scope.clone(), self.sign_payload(encode_consensus_msg(&propose_msg)),
            ).await;
            // A genesis's or a retirement's commit carries its certificate: the signed votes that decided it,
            // this node's own included.
            let mut cert: Option<Vec<Bytes>> = tag.as_ref().filter(|t| t.certified()).map(|_| {
                let own = ConsensusMsg::VoteForValue {
                    slot: Arc::clone(&slot), ballot, voter: self.task_ctx.node_id.clone(),
                    value_digest: value_digest(&value), locality: None,
                };
                vec![self.sign_payload(encode_consensus_msg(&own))]
            });

            // Voter dedup map per (slot, ballot). NodeId-keyed so each voter contributes
            // exactly once even if they re-emit; the Option<LocalityPath> value is required
            // for Hard topology gate evaluation. Cannot use signal_handlers.quorum_for_group
            // here: the sender_log records (sender, received_at) without slot/ballot
            // correlation, so it would conflate votes from different rounds.
            //
            // Membership changes mid-ballot are intentionally ignored. The group member
            // set is captured once before the ballot loop (in group_propose) and the quorum
            // threshold is fixed for that ballot's lifetime. A joining member's votes do not
            // count toward this ballot; a leaving member's existing vote remains counted.
            let mut voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
            voters.insert(self.task_ctx.node_id.clone(), self.self_locality.clone());

            // Single-node quorum check before entering the collect loop.
            if let Some(res) = self.try_commit_if_ready(
                &voters, quorum_size, group_name.as_deref(),
                &scope, &slot, ballot, &value, &ballot_key, &commit_key, lease_ms, cert.as_deref(),
            ).await {
                return res;
            }

            // Drive one ballot attempt.
            let deadline = time::Instant::now() + config.phase1_timeout;
            let outcome = self.collect_one_ballot(
                &mut voters, &mut quorum_size,
                &mut vote_rx, &mut nack_rx, &mut opaque_rx,
                deadline,
                &slot, ballot, &value, &scope,
                &ballot_key, &commit_key,
                group_name.as_deref(),
                trust_set.as_ref(),
                opaque_recompute.as_ref(),
                lease_ms,
                tag.as_ref(),
                cert.as_mut(),
            ).await;

            let mut nack_ballot = 0u64;
            match outcome {
                BallotOutcome::Committed(res) => return res,
                BallotOutcome::Stale(seen) => return stale_result(slot, &scope, tag.as_ref(), seen),
                BallotOutcome::NackHigher(b, reported) => {
                    // Refused at this ballot: another proposer is ahead — contention, however few
                    // votes had arrived (the review of #579).
                    last = LastAttempt::Contended;
                    nack_ballot = b;
                    // **Accepted-value preservation.** An acceptor that refused us reported what
                    // it already holds; the next ballot must carry *that* value, not ours. Without
                    // this the retry is `ballot + 1` with the proposer's original value, which can
                    // replace a value a quorum already accepted — acceptors permit it, because any
                    // strictly greater ballot passes `may_cast_vote`.
                    if let Some((ab, v, is_env)) = reported
                        && ab >= adopted_from
                        && let (_, _, Some(env)) = carry.report(&slot, ab, None, Some(v), is_env)
                        && !carry.renews(&env) {
                            adopted_from = ab;
                            value = env;
                        }
                }
                BallotOutcome::Timeout => {}
            }

            if let Some(existing) = self.live_digest(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded {
                        slot,
                        ballot: self.read_ballot(&ballot_key),
                    };
                }

            votes_last_ballot = voters.len();
            // If this ballot reached quorum-by-count but failed the Hard gate,
            // remember it for the final TopologyUnsatisfied return.
            if voters.len() >= quorum_size {
                let (passes, distinct, meta) = self.topology_check(&voters, group_name.as_deref());
                if !passes
                    && let Some((depth, required)) = meta {
                        topology_failed = Some((distinct, required, depth));
                    }
            }

            ballot_retry_pause(config.ballot_retry_jitter_ms).await;
            let Some(next) = next_ballot(nack_ballot.max(self.read_ballot(&ballot_key)).max(ballot)) else {
                return self.ballots_exhausted(slot, _attempt + 1, quorum_size);
            };
            ballot = next;
        }

        // After exhausting max_ballots: prefer TopologyUnsatisfied over Timeout
        // when the last attempt reached quorum but failed the gate — the caller
        // needs to distinguish "not enough voters" from "voters not diverse enough."
        if let Some((distinct, required, depth)) = topology_failed {
            return ConsensusResult::TopologyUnsatisfied {
                slot,
                ballot,
                voters_seen:      votes_last_ballot,
                quorum_required:  quorum_size,
                distinct_domains: distinct,
                domains_required: required,
                spread_depth:     depth,
            };
        }

        // This proposer's own vote is in `voters`; the label asks whether anyone *else* answered.
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_timeouts_total",
            "reason" => timeout_reason(last, votes_last_ballot.saturating_sub(1)))
            .increment(1);
        #[cfg(not(feature = "metrics"))]
        let _ = last;
        ConsensusResult::Timeout {
            slot,
            ballots_tried: config.max_ballots,
            votes_last_ballot,
            quorum_required: quorum_size,
        }
    }

    // ── P2: electorate groups (`docs/design/consensus-electorate.md` §8) ─────────────────────────

    /// The verified electorate for `group`: the highest genesis attempt whose committed record is well formed,
    /// names the retirement that names this group (when it succeeds one), and whose certificate — the signed
    /// votes of **every** member it names, at one ballot — verifies. Adopted only through the gate, so a node
    /// whose acceptor touched a later attempt never adopts an earlier one (round 4, finding 3). A record that
    /// does not verify is refused and counted ([`crate::electorate_records_refused`]); one whose certificate
    /// or predecessor's retirement has not arrived yet is simply not adopted yet. Membership is frozen: once
    /// adopted, a later attempt is adopted only by a node that never adopted, never voted for, this one.
    pub(crate) fn electorate_view(&self, group: &str) -> Option<Arc<ElectorateDecl>> {
        use crate::agent::electorate::{already_refused, cached, cert_key, check_genesis, genesis_slot, note_refused, refresh_records};
        refresh_records(&self.task_ctx);
        let cur = cached(&self.task_ctx, group);
        // Verifying a successor's genesis reads its predecessor's electorate, which reads its own predecessor: a
        // forged cycle of records must not recurse without end.
        let Some(_depth) = ViewDepth::enter() else { return cur };
        let have = cur.as_ref().map_or(0, |d| d.epoch);
        let top = self.task_ctx.electorate_epochs_known.pin().get(group).copied().unwrap_or(0);
        for epoch in (have + 1..=top).rev() {
            let slot = genesis_slot(group, epoch);
            let Some(raw) = self.get(&format!("{}{}", consensus_ns::COMMITTED, slot)) else { continue };
            let digest = value_digest(&raw);
            let Some(next) = ElectorateDecl::decode(&raw).filter(|d| d.epoch == epoch) else {
                note_refused(&self.task_ctx, &slot, &digest, "the record does not decode as a genesis of this epoch");
                continue;
            };
            if let Err(e) = check_genesis(group, &next) {
                note_refused(&self.task_ctx, &slot, &digest, &format!("the record is not a well-formed genesis: {e}"));
                continue;
            }
            match self.predecessor_holds(group, &next) {
                Ok(true) => {}
                Ok(false) => continue, // the predecessor's retirement has not arrived here yet
                Err(why) => { note_refused(&self.task_ctx, &slot, &digest, &why); continue; }
            }
            // A certificate already refused is not verified again (round 2, finding 5): keyed by the record and the
            // certificate's own bytes, so a genuine certificate arriving later is still checked.
            let Some(cert) = self.get(&cert_key(group, epoch)) else { continue };
            let key = xor_digests(&digest, &value_digest(&cert));
            if already_refused(&self.task_ctx, &slot, &key) { continue; }
            if !self.certificate_holds(&slot, &digest, &cert, &next.members, next.members.len()) {
                note_refused(&self.task_ctx, &slot, &key, "its certificate does not verify");
                continue;
            }
            if !self.adopt_through_gate(group, epoch) {
                return cur; // this node touched a later attempt: it never adopts this one
            }
            let next = Arc::new(next);
            let adopted = Arc::clone(&next);
            self.task_ctx.electorates.pin().compute(Arc::from(group), |existing| match existing {
                Some((_, e)) if e.epoch >= adopted.epoch => papaya::Operation::Abort(()),
                _ => papaya::Operation::Insert(Arc::clone(&adopted)),
            });
            return cached(&self.task_ctx, group);
        }
        cur
    }

    /// Whether a genesis record names its predecessor correctly: when it names one, the predecessor's verified
    /// retirement names this group, has this digest and passed the default this record carries. `Ok(false)` while
    /// that retirement has not arrived (or verified) here yet; `Err` when it contradicts the record.
    fn predecessor_holds(&self, group: &str, next: &ElectorateDecl) -> Result<bool, String> {
        let Some(p) = &next.predecessor else { return Ok(true) };
        let Some(r) = self.retirement_view(&p.group) else { return Ok(false) };
        if r.successor != group || r.digest() != p.retirement || r.passes_default != next.exclusive_default {
            return Err(format!("the genesis names {}'s retirement, which names {} (default passed: {})",
                p.group, r.successor, r.passes_default));
        }
        Ok(true)
    }

    /// `group`'s verified retirement: its committed record (`electorate/{group}/retire`) names this electorate's
    /// epoch and a successor, passes the default exactly when the group held it, and its certificate — signed
    /// votes of a strict majority of the electorate, at one ballot — verifies. Cached once verified.
    pub(crate) fn retirement_view(&self, group: &str) -> Option<Arc<Retirement>> {
        use crate::agent::electorate::{already_refused, note_refused, refresh_records, retire_cert_key, retire_slot, valid_group};
        if let Some(r) = self.task_ctx.electorate_retired.pin().get(group) { return Some(Arc::clone(r)); }
        refresh_records(&self.task_ctx);
        if !self.task_ctx.electorate_retire_known.pin().contains(group) { return None; }
        let d = self.electorate_view(group)?;
        let slot = retire_slot(group);
        let raw = self.get(&format!("{}{}", consensus_ns::COMMITTED, slot))?;
        let digest = value_digest(&raw);
        let Some(r) = Retirement::decode(&raw) else {
            note_refused(&self.task_ctx, &slot, &digest, "the record does not decode as a retirement");
            return None;
        };
        if r.group != group || r.epoch != d.epoch || !valid_group(&r.successor) || r.successor == group
            || r.passes_default != d.exclusive_default {
            note_refused(&self.task_ctx, &slot, &digest, "the retirement does not match the electorate it retires");
            return None;
        }
        let cert = self.get(&retire_cert_key(group))?;
        let key = xor_digests(&digest, &value_digest(&cert));
        if already_refused(&self.task_ctx, &slot, &key) { return None; }
        if !self.certificate_holds(&slot, &digest, &cert, &d.members, d.quorum()) {
            note_refused(&self.task_ctx, &slot, &key, "its certificate does not verify");
            return None;
        }
        let r = Arc::new(r);
        self.task_ctx.electorate_retired.pin().insert(Arc::from(group), Arc::clone(&r));
        Some(r)
    }

    /// The verified retirement that names `group` its successor, if any (the first by group name, should two).
    pub(crate) fn predecessor_of(&self, group: &str) -> Option<Arc<Retirement>> {
        crate::agent::electorate::refresh_records(&self.task_ctx);
        let mut retired: Vec<Arc<str>> = self.task_ctx.electorate_retire_known.pin().iter().cloned().collect();
        retired.sort();
        retired.iter().filter_map(|g| self.retirement_view(g)).find(|r| r.successor == group)
    }

    /// The retired electorates `d` succeeds, nearest first, each verified with the retirement `d`'s chain names.
    /// `Err` when one is not known here — a successor that cannot read its predecessor cannot carry from it.
    fn predecessor_chain(&self, d: &ElectorateDecl) -> Result<Vec<Arc<ElectorateDecl>>, String> {
        let mut chain = Vec::new();
        let mut at = d.predecessor.clone();
        while let Some(p) = at {
            if chain.len() >= crate::agent::electorate::CHAIN_LIMIT {
                return Err(format!("{}'s predecessor chain is longer than {}", d.group, crate::agent::electorate::CHAIN_LIMIT));
            }
            let pred = self.electorate_view(&p.group)
                .ok_or_else(|| format!("{} succeeds {}, whose electorate is not known here yet", d.group, p.group))?;
            if self.retirement_view(&p.group).is_none_or(|r| r.digest() != p.retirement) {
                return Err(format!("{}'s retirement is not known here yet", p.group));
            }
            at = pred.predecessor.clone();
            chain.push(pred);
        }
        Ok(chain)
    }

    /// The electorates that succeed `group`, nearest first — those whose members a retired member reports to.
    fn successors_of(&self, group: &str) -> Vec<Arc<ElectorateDecl>> {
        let mut out = Vec::new();
        let mut at = group.to_string();
        while out.len() < crate::agent::electorate::CHAIN_LIMIT {
            let Some(r) = self.retirement_view(&at) else { break };
            let Some(s) = self.electorate_view(&r.successor) else { break };
            at = r.successor.clone();
            out.push(s);
        }
        out
    }

    /// The highest genesis attempt of `group` this node's acceptor promised or accepted, from its memory (which
    /// survives a restart), and the highest one it accepted a readable value for — with the digest of what it
    /// holds (the decision envelope, as accepted).
    pub(crate) fn pending_genesis(&self, group: &str) -> (u64, Option<(ElectorateDecl, [u8; 32])>) {
        let mut touched = 0;
        let mut pending: Option<(ElectorateDecl, [u8; 32])> = None;
        for (slot, s) in self.task_ctx.consensus_accepted.pin().iter() {
            let Some((g, epoch)) = crate::agent::electorate::parse_genesis_slot(slot) else { continue };
            if g != group || (s.promised == 0 && s.accepted.is_none()) { continue; }
            touched = touched.max(epoch);
            if let Some((_, a)) = &s.accepted
                && let Some(d) = raw_accepted(a).and_then(|v| ElectorateDecl::decode(&v))
                && pending.as_ref().is_none_or(|(p, _)| p.epoch < epoch) {
                    pending = Some((d, a.digest()));
                }
        }
        (touched, pending)
    }

    /// Adopts genesis attempt `epoch` of `group` through the gate: refused when this node's acceptor touched a
    /// later attempt; waits out a genesis claim in flight, so a claim and an adoption are ordered on one key.
    fn adopt_through_gate(&self, group: &str, epoch: u64) -> bool {
        let durable = self.pending_genesis(group).0;
        let fences = self.task_ctx.electorate_fences.pin();
        loop {
            let r = fences.compute(Arc::from(group), |cur| {
                let s = cur.map(|(_, v)| *v).unwrap_or_default();
                if s.opening > 0 { papaya::Operation::Abort(true) }
                else if s.touched.max(durable) > epoch { papaya::Operation::Abort(false) }
                else { papaya::Operation::Insert(FenceState { adopted: s.adopted.max(epoch), ..s }) }
            });
            match r {
                papaya::Compute::Aborted(true) => std::thread::yield_now(),
                papaya::Compute::Aborted(false) => return false,
                _ => return true,
            }
        }
    }

    /// Whether this node touched a genesis attempt of `group` without adopting an electorate — an electorate in
    /// formation. A lookup on the hot path of every untagged proposal: the gate key's `touched` (seeded at start by
    /// `prewarm_genesis_touched`, raised by every genesis claim), and the first attempt's acceptor memory.
    fn forming(&self, group: &str) -> bool {
        self.task_ctx.electorate_fences.pin().get(group).is_some_and(|s| s.touched > 0)
            || self.task_ctx.consensus_accepted.pin().get(crate::agent::electorate::genesis_slot(group, 1).as_str())
                .is_some_and(|s| s.promised > 0 || s.accepted.is_some())
    }

    /// Whether this node's acceptor durably closed `group`: it promised (or accepted) the group's retirement, or
    /// holds the verified retirement. Read inside the gate's `compute`, so it must stay a pure read.
    fn durably_closed(&self, group: &str) -> bool {
        let slot = crate::agent::electorate::retire_slot(group);
        self.task_ctx.consensus_accepted.pin().get(slot.as_str()).is_some_and(|s| s.promised > 0 || s.accepted.is_some())
            || self.task_ctx.electorate_retired.pin().contains_key(group)
    }

    /// Whether `group` is closed here — retired, or its retirement promised by this node's acceptor.
    fn closed(&self, group: &str) -> bool {
        let _ = self.retirement_view(group); // verifies (and caches) a retirement that has arrived
        self.task_ctx.electorate_fences.pin().get(group).is_some_and(|s| s.closed || s.closing > 0) || self.durably_closed(group)
    }

    /// Why `group` is closed, for a refusal.
    fn closed_detail(&self, group: &str) -> String {
        match self.retirement_view(group) {
            Some(r) => format!("electorate group {group} is retired; its successor is {}", r.successor),
            None => format!("electorate group {group} is closing: this node promised its retirement"),
        }
    }

    /// **Claim through the gate** (round 2 of #601's review, finding 1; made retry-safe in round 3, finding 5;
    /// round 4). The `compute` on the group's key only **decides**; the claim runs after it committed, with
    /// nothing inside a closure papaya may re-run:
    ///
    /// - `Ordinary` passes while the group is not closed and this node holds `epoch`, and registers itself as
    ///   **in flight** on the key; the claim runs, then the registration is released (a drop guard).
    /// - `Retire` closes the group only when **no** ordinary claim is in flight (it yields and retries otherwise),
    ///   then claims. So every ordinary claim either completed before the close or is refused after it — the
    ///   order the retirement's fence and the carry's reports rest on (decision record §8). Afterwards the key
    ///   keeps only what is durable: a claim the acceptor refused promised nothing, and leaves the group open
    ///   (round 4, finding 4).
    /// - `Genesis(e)` passes while this node adopted no electorate for the group and touched no later attempt,
    ///   registered as opening (an adoption waits it out); afterwards it records `e` as touched if the acceptor
    ///   now holds a promise or acceptance for it.
    ///
    /// `None` when the gate refused (nothing was claimed); otherwise what `claim` returned.
    pub(crate) fn claim_gated<T>(&self, gate: Option<&Gate>, claim: impl FnOnce() -> T) -> Option<T> {
        let Some(g) = gate else { return Some(claim()) };
        let fences = self.task_ctx.electorate_fences.pin();
        match g.kind {
            GateKind::Retire => {
                loop {
                    let raised = fences.compute(Arc::clone(&g.group), |cur| {
                        let s = cur.map(|(_, v)| *v).unwrap_or_default();
                        if s.inflight > 0 { papaya::Operation::Abort(()) }
                        else { papaya::Operation::Insert(FenceState { closed: true, closing: s.closing + 1, ..s }) }
                    });
                    if !matches!(raised, papaya::Compute::Aborted(())) { break; }
                    std::thread::yield_now();
                }
                let out = claim();
                let durable = self.durably_closed(&g.group);
                fences.compute(Arc::clone(&g.group), |cur| {
                    let s = cur.map(|(_, v)| *v).unwrap_or_default();
                    let closing = s.closing.saturating_sub(1);
                    papaya::Operation::<FenceState, ()>::Insert(FenceState { closed: durable || closing > 0, closing, ..s })
                });
                Some(out)
            }
            GateKind::Genesis(epoch) => {
                let durable = self.pending_genesis(&g.group).0;
                let entered = fences.compute(Arc::clone(&g.group), |cur| {
                    let s = cur.map(|(_, v)| *v).unwrap_or_default();
                    if s.adopted > 0 || s.touched.max(durable) > epoch { papaya::Operation::Abort(()) }
                    else { papaya::Operation::Insert(FenceState { opening: s.opening + 1, ..s }) }
                });
                if matches!(entered, papaya::Compute::Aborted(())) { return None; }
                let out = claim();
                let slot = crate::agent::electorate::genesis_slot(&g.group, epoch);
                let holds = self.task_ctx.consensus_accepted.pin().get(slot.as_str())
                    .is_some_and(|s| s.promised > 0 || s.accepted.is_some());
                fences.compute(Arc::clone(&g.group), |cur| {
                    let s = cur.map(|(_, v)| *v).unwrap_or_default();
                    let touched = if holds { s.touched.max(epoch) } else { s.touched };
                    papaya::Operation::<FenceState, ()>::Insert(FenceState { opening: s.opening.saturating_sub(1), touched, ..s })
                });
                Some(out)
            }
            GateKind::Ordinary => {
                let entered = fences.compute(Arc::clone(&g.group), |cur| {
                    let s = cur.map(|(_, v)| *v).unwrap_or_default();
                    let closed = s.closed || s.closing > 0 || self.durably_closed(&g.group);
                    let current = crate::agent::electorate::cached(&self.task_ctx, &g.group).map(|d| d.epoch) == Some(g.epoch);
                    if closed || !current { papaya::Operation::Abort(()) }
                    else { papaya::Operation::Insert(FenceState { inflight: s.inflight + 1, ..s }) }
                });
                if matches!(entered, papaya::Compute::Aborted(())) { return None; }
                let _leave = InFlight { fences: &self.task_ctx.electorate_fences, group: &g.group };
                Some(claim())
            }
        }
    }

    /// Closes the retired electorate `d` for good on this node — through the gate, after any ordinary claim in
    /// flight — and puts its retirement (record and certificate) on stable storage, so a restart still finds the
    /// group closed. What a member does before it reports to a successor's carry. `false` when the retirement did
    /// not reach the disk: the member then reports nothing.
    async fn close_for_carry(&self, d: &ElectorateDecl) -> bool {
        use crate::agent::electorate::{retire_cert_key, retire_slot};
        if self.retirement_view(&d.group).is_none() { return false; }
        let gate = Gate { group: Arc::from(d.group.as_str()), epoch: d.epoch, kind: GateKind::Retire };
        let _ = self.claim_gated(Some(&gate), || ());
        let Some(wal) = self.task_ctx.wal.get() else { return true };
        for key in [format!("{}{}", consensus_ns::COMMITTED, retire_slot(&d.group)), retire_cert_key(&d.group)] {
            let Some((value, timestamp)) = self.task_ctx.kv_state.store.pin().get(key.as_str())
                .and_then(|e| e.data.clone().map(|v| (v, e.timestamp))) else { return false };
            let entry = crate::framing::SyncEntry { key: Arc::from(key.as_str()), value, timestamp, is_tombstone: false };
            if let Err(e) = wal.append_sync(entry).await {
                tracing::error!(group = %d.group, error = %e, "consensus: a retirement did not reach stable storage; not reporting");
                return false;
            }
        }
        true
    }

    /// Whether this node has seen `group` decide `slot` — after which its proposals for it no longer carry from
    /// a predecessor (the decision is held by a majority of `group`, at a ballot above every predecessor's).
    /// In memory: after a restart a proposal carries again, which is safe (§8) and costs only the reads.
    fn carried(&self, group: &str, slot: &str) -> bool {
        self.task_ctx.electorate_carried.pin().contains(format!("{group}\u{0}{slot}").as_str())
    }

    /// Records that `group` decided `slot`, when `group` succeeds a retired one (see [`carried`](Self::carried)).
    fn note_decided_in(&self, group: &str, slot: &str) {
        if crate::agent::electorate::slot_names_group(slot).is_some() { return; }
        if crate::agent::electorate::cached(&self.task_ctx, group).is_some_and(|d| d.predecessor.is_some()) {
            self.task_ctx.electorate_carried.pin().insert(Arc::from(format!("{group}\u{0}{slot}").as_str()));
        }
    }

    /// Whether a certificate for an electorate record verifies. A certificate is the decided **decision envelope**
    /// (row A: the value Paxos decided, whose `value` is the record) and the signed `VoteForValue`s for it: the
    /// envelope must be for `slot` and carry the record whose digest is `digest`, and the votes must be for the
    /// envelope's digest, at one ballot, from at least `need` of `electors` — every member for a genesis, a
    /// strict majority of the retired electorate for a retirement. Without `[tls]` nothing is signed, and the
    /// check is a shape.
    fn certificate_holds(&self, slot: &str, digest: &[u8; 32], raw: &Bytes, electors: &[NodeId], need: usize) -> bool {
        let Ok((envelope, votes)) = mycelium_core::serde_fixint::from_slice::<(Bytes, Vec<Bytes>)>(raw) else { return false };
        let Some(env) = crate::consensus_life::Envelope::decode(&envelope) else { return false };
        if *env.slot != *slot || env.value.as_ref().map(value_digest).as_ref() != Some(digest) { return false; }
        let want = value_digest(&envelope);
        let mut by_ballot: AHashMap<u64, AHashSet<NodeId>> = AHashMap::new();
        for v in &votes {
            if let Some(ConsensusMsg::VoteForValue { slot: s, ballot, voter, value_digest: d, .. }) = self.decode_verify(v)
                && &*s == slot && d == want && electors.contains(&voter) {
                    by_ballot.entry(ballot).or_default().insert(voter);
                }
        }
        by_ballot.values().any(|v| v.len() >= need)
    }

    /// Test-only: this node's acceptor accepts `value` for `slot` at `ballot` on electorate `group`, exactly as
    /// its listener would on a `ProposeIn` — the claim gated, the record durable. Lets a test choose a value on a
    /// chosen set of acceptors without a proposer committing it.
    #[cfg(test)]
    pub(crate) async fn accept_for_test(&self, group: &str, slot: &Arc<str>, ballot: u64, value: &Bytes, proposer: &NodeId) -> bool {
        let Some(d) = self.electorate_view(group) else { return false };
        let gate = Gate { group: Arc::from(group), epoch: d.epoch, kind: GateKind::Ordinary };
        let floor = self.floor(slot);
        // One envelope for one proposal, whichever acceptor accepts it: its token is the ballot, not this node's clock,
        // so two acceptors given the same (slot, ballot, value, proposer) hold the same bytes, as they would from one
        // proposer.
        let envelope = crate::consensus_life::Envelope::new(
            slot, value.clone(), crate::consensus_life::Term::Permanent, ballot, proposer.clone(), ballot,
        ).encode();
        self.claim_gated(Some(&gate), || claim_envelope(&self.task_ctx.consensus_accepted, slot, ballot, &envelope, proposer.id_hash(), floor))
            == Some(true) && self.persist_acceptor(slot).await
    }

    /// **The proposer's door for P2.** `Ok(Some(tag))` for a proposal on an electorate group (carrying
    /// its epoch, digest, vote filter, quorum and the predecessors to carry from), `Ok(None)` for one outside
    /// any, or a refusal by name.
    fn electorate_door(
        &self, scope: &SignalScope, slot: &Arc<str>, value: &Bytes, config: &ConsensusConfig,
    ) -> Result<Option<ElectorateTag>, DoorRefusal> {
        use crate::agent::electorate::{check_genesis, parse_genesis_slot, parse_retire_slot, slot_names_group, valid_group};
        let me = &self.task_ctx.node_id;
        match scope {
            SignalScope::Group(group) => {
                self.check_slot_family(slot, group).map_err(|d| DoorRefusal::mismatch(Some(group), &d))?;
                let view = self.electorate_view(group);
                if let Some((_, epoch)) = parse_genesis_slot(slot).filter(|(g, _)| *g == &**group) {
                    if view.is_some() {
                        return Err(DoorRefusal::mismatch(Some(group), &format!(
                            "membership is frozen: {group} has an electorate, and a genesis decides nothing more — \
                             migrate to a successor group")));
                    }
                    let Some(next) = ElectorateDecl::decode(value).filter(|d| d.epoch == epoch) else {
                        return Err(DoorRefusal::mismatch(Some(group), "a genesis's value is not a genesis of its epoch"));
                    };
                    check_genesis(group, &next).map_err(|e| DoorRefusal::mismatch(Some(group), &e.to_string()))?;
                    if !next.contains(me) { return Err(DoorRefusal::NotAMember(Arc::clone(group))); }
                    return Ok(Some(ElectorateTag {
                        epoch: 0, digest: [0; 32], quorum: next.members.len(),
                        voters: next.members.iter().map(NodeId::id_hash).collect(),
                        gate: Gate { group: Arc::clone(group), epoch: 0, kind: GateKind::Genesis(epoch) },
                        predecessors: Vec::new(),
                    }));
                }
                if parse_retire_slot(slot).is_some_and(|g| g == &**group) {
                    let Some(d) = view else { return Err(DoorRefusal::mismatch(Some(group), "no electorate to retire")) };
                    if !d.contains(me) { return Err(DoorRefusal::NotAMember(Arc::clone(group))); }
                    if let Some(r) = self.retirement_view(group) {
                        return Err(DoorRefusal::mismatch(Some(group), &format!("{group} is already retired to {}", r.successor)));
                    }
                    let fits = Retirement::decode(value).is_some_and(|r| r.group == **group && r.epoch == d.epoch
                        && valid_group(&r.successor) && r.successor != **group && r.passes_default == d.exclusive_default);
                    if !fits { return Err(DoorRefusal::mismatch(Some(group), "a retirement's value does not retire this electorate")); }
                    // No roster check: the roster moving away from a frozen electorate is what a retirement is for.
                    return Ok(Some(ElectorateTag::of(&d, GateKind::Retire)));
                }
                match view {
                    Some(d) => {
                        if !d.contains(me) { return Err(DoorRefusal::NotAMember(Arc::clone(group))); }
                        if self.closed(group) { return Err(DoorRefusal::mismatch(Some(group), &self.closed_detail(group))); }
                        let roster = crate::agent::electorate::sorted_members(
                            crate::agent::helpers::group_members_ctx(&self.task_ctx, group));
                        if roster != d.members {
                            note_roster_mismatch(slot, group, d.epoch);
                            return Err(DoorRefusal::mismatch(Some(group),
                                "the group roster differs from its electorate — membership is frozen; a member joined or \
                                 left without a migration"));
                        }
                        let mut tag = ElectorateTag::of(&d, GateKind::Ordinary);
                        if d.predecessor.is_some() && slot_names_group(slot).is_none() && !self.carried(group, slot) {
                            tag.predecessors = self.predecessor_chain(&d).map_err(|why| DoorRefusal::mismatch(Some(group), &why))?;
                        }
                        Ok(Some(tag))
                    }
                    None => match self.refuse_ungoverned(config, slot, Some(group)) {
                        Some(_) => Err(DoorRefusal::NotGoverned(Some(Arc::clone(group)))),
                        None => Ok(None),
                    },
                }
            }
            SignalScope::Groups(groups) => {
                if let Some(g) = groups.iter().find(|g| self.electorate_view(g).is_some()) {
                    return Err(DoorRefusal::mismatch(Some(g),
                        "an electorate group decides only through its own electorate, never inside a cross-group proposal"));
                }
                // An exclusive slot's home is never a cross-group proposal (round 3, finding 3).
                if let Some(why) = self.cross_group_slot_refusal(slot, groups) {
                    return Err(DoorRefusal::mismatch(groups.first(), &why));
                }
                match self.refuse_ungoverned(config, slot, groups.first()) {
                    Some(_) => Err(DoorRefusal::NotGoverned(groups.first().cloned())),
                    None => Ok(None),
                }
            }
            _ => {
                if crate::agent::electorate::is_safety_sensitive(config, slot) {
                    match crate::agent::electorate::exclusive_electorate(&self.task_ctx) {
                        Ok(Some(g)) => return Err(DoorRefusal::Mismatch {
                            group: Some(Arc::from(g.as_str())),
                            detail: Arc::from(format!("the fleet decides exclusive outcomes in electorate group {g}, \
                                                       not across the whole cluster").as_str()),
                        }),
                        Err(detail) => return Err(DoorRefusal::Mismatch { group: None, detail: Arc::from(detail.as_str()) }),
                        Ok(None) => {}
                    }
                }
                match self.refuse_ungoverned(config, slot, None) {
                    Some(_) => Err(DoorRefusal::NotGoverned(None)),
                    None => Ok(None),
                }
            }
        }
    }

    /// Why a cross-group proposal may not decide `slot`: a slot that names its group, or a fleet-exclusive slot
    /// while the fleet decides those in an electorate (or the setting disagrees). `None` when it may.
    fn cross_group_slot_refusal(&self, slot: &str, groups: &[Arc<str>]) -> Option<String> {
        use crate::agent::electorate::{exclusive_electorate, is_fleet_exclusive_slot, slot_names_group};
        if let Some(named) = slot_names_group(slot) && !groups.iter().all(|g| &**g == named) {
            return Some(format!("slot {slot} belongs to group {named}"));
        }
        if is_fleet_exclusive_slot(slot) {
            return match exclusive_electorate(&self.task_ctx) {
                Ok(None) => None,
                Ok(Some(g)) => Some(format!("slot {slot} is decided in the fleet's electorate {g}, never by a cross-group proposal")),
                Err(e) => Some(e),
            };
        }
        None
    }

    /// **Which group may decide a slot** (round 2, finding 2c): a fleet-exclusive slot (`lock/`, `consistent/`,
    /// `capauthz/`, a commitment award) only in the fleet's exclusive electorate, and a slot that names its
    /// group (`leader/{g}`, `electorate/{g}/…`) only in that group — so two groups never decide one
    /// `consensus/committed/{slot}`. `Err` says why not.
    fn check_slot_family(&self, slot: &str, group: &str) -> Result<(), String> {
        use crate::agent::electorate::{exclusive_electorate, is_fleet_exclusive_slot, slot_names_group};
        if let Some(named) = slot_names_group(slot) && named != group {
            return Err(format!("slot {slot} belongs to group {named}, not {group}"));
        }
        if is_fleet_exclusive_slot(slot) {
            return match exclusive_electorate(&self.task_ctx) {
                Ok(Some(g)) if g == group => Ok(()),
                Ok(Some(g)) => Err(format!("slot {slot} is decided in the fleet's electorate {g}, not {group}")),
                Ok(None) => Err(format!("slot {slot} is decided across the cluster (no electorate is the fleet's \
                                         exclusive default), never in group {group}")),
                Err(e) => Err(e),
            };
        }
        Ok(())
    }

    /// **The acceptor's door for P2**: may this node answer a proposal for `slot` arriving under `scope`
    /// that names electorate `(epoch, digest)` — `None` for a legacy message, which names none? `Ok(gate)` to
    /// answer, the claim then made through that gate; `Err((epoch, digest))` — this node's own electorate (the
    /// next epoch, under its own digest, when the group is closed here) — to refuse with `StaleElectorate`.
    ///
    /// `proposed` is the proposal's value as it arrived — a decision envelope (`true`, row A) or a legacy value —
    /// and `None` for a prepare.
    pub(crate) fn electorate_admits(
        &self, scope: &SignalScope, slot: &str, named: Option<(u64, [u8; 32])>, proposer: &NodeId, proposed: Option<(&Bytes, bool)>,
    ) -> Result<Option<Gate>, (u64, [u8; 32])> {
        let raw = proposed.map(|(b, env)| if env { raw_of(b) } else { Some(b.clone()) });
        use crate::agent::electorate::{is_fleet_exclusive_slot, parse_genesis_slot, parse_retire_slot, valid_group};
        let me = &self.task_ctx.node_id;
        let group = match scope {
            SignalScope::Group(g) => Arc::clone(g),
            SignalScope::Groups(gs) => {
                // A cross-group proposal over an electorate group this node holds is not answered, nor one for an
                // exclusive slot whose home is the fleet's electorate (round 3, finding 3).
                if let Some(d) = gs.iter().find_map(|g| self.electorate_view(g)) {
                    return Err((d.epoch, d.digest()));
                }
                return match self.cross_group_slot_refusal(slot, gs) {
                    Some(_) => Err((0, [0; 32])),
                    None => Ok(None),
                };
            }
            _ => {
                // A cluster-scoped proposal for an exclusive slot — every family, `capauthz/` and commitment
                // awards included (round 2, finding 2b) — while the fleet decides those in an electorate group,
                // is not answered: a lagging or pre-P2 proposer cannot decide one across the cluster.
                let exclusive = crate::agent::electorate::SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f))
                    || is_fleet_exclusive_slot(slot);
                // While the fleet's exclusive electorate cannot be named here — a retired default whose successor has not
                // formed, or two marked — such a slot has no home, and is refused too (fail closed, round 4).
                if exclusive && named.is_none() {
                    match crate::agent::electorate::exclusive_electorate(&self.task_ctx) {
                        Ok(Some(g)) => if let Some(d) = self.electorate_view(&g) { return Err((d.epoch, d.digest())); },
                        Err(_) => return Err((0, [0; 32])),
                        Ok(None) => {}
                    }
                }
                return Ok(None);
            }
        };
        let view = self.electorate_view(&group);
        let own = view.as_ref().map_or((0, [0; 32]), |d| (d.epoch, d.digest()));
        if self.check_slot_family(slot, &group).is_err() { return Err(own); }
        if let Some((_, epoch)) = parse_genesis_slot(slot).filter(|(g, _)| *g == &*group) {
            // A genesis attempt: answered by the members it names, none of which holds an electorate for the group.
            if view.is_some() || named != Some((0, [0; 32])) { return Err(own); }
            let gate = Gate { group: Arc::clone(&group), epoch: 0, kind: GateKind::Genesis(epoch) };
            return match (raw, proposed) {
                (None, _) => Ok(Some(gate)), // a prepare carries no value
                (Some(Some(v)), Some((as_proposed, _))) if self.genesis_acceptable(&group, epoch, &v, as_proposed, proposer) => Ok(Some(gate)),
                _ => Err((0, [0; 32])),
            };
        }
        let retiring = parse_retire_slot(slot).is_some_and(|g| g == &*group);
        match (view, named) {
            // Not an electorate group here, and a legacy message: nothing changes — unless this node touched one of
            // the group's genesis attempts. Then it is an electorate in formation, and an untagged proposal is refused
            // until the genesis is adopted (round 3, finding 4).
            (None, None) => if retiring || self.forming(&group) { Err((0, [0; 32])) } else { Ok(None) },
            (None, Some(_)) | (Some(_), None) => Err(own),
            (Some(d), Some((epoch, digest))) => {
                if epoch != d.epoch || digest != own.1 || !d.contains(me) || !d.contains(proposer) {
                    return Err(own);
                }
                let gate = |kind| Some(Gate { group: Arc::clone(&group), epoch: d.epoch, kind });
                if retiring {
                    // The retirement's value must retire this electorate, to a successor that has none yet (so the
                    // successor's genesis can name it).
                    return match raw.map(|v| v.and_then(|v| Retirement::decode(&v))) {
                        None => Ok(gate(GateKind::Retire)),
                        Some(Some(r)) if r.group == *group && r.epoch == d.epoch && valid_group(&r.successor)
                            && r.successor != *group && r.passes_default == d.exclusive_default
                            && self.electorate_view(&r.successor).is_none() => Ok(gate(GateKind::Retire)),
                        Some(_) => Err(own),
                    };
                }
                if self.closed(&group) { return Err((d.epoch + 1, own.1)); }
                Ok(gate(GateKind::Ordinary))
            }
        }
    }

    /// Whether this node's acceptor may accept `value` as genesis attempt `epoch` of `group` from `proposer`
    /// (round 4): a well-formed genesis of that epoch naming both; the roster this node sees is its member set; it
    /// names the retirement that names this group, if one does, and carries the default it passed; a genesis it
    /// supersedes is the one this node accepted, when this node is among its members; and this node has accepted
    /// no other value for the attempt.
    fn genesis_acceptable(&self, group: &str, epoch: u64, value: &Bytes, as_proposed: &Bytes, proposer: &NodeId) -> bool {
        use crate::agent::electorate::{check_genesis, genesis_slot, sorted_members};
        let me = &self.task_ctx.node_id;
        let Some(next) = ElectorateDecl::decode(value).filter(|d| d.epoch == epoch) else { return false };
        if check_genesis(group, &next).is_err() || !next.contains(me) || !next.contains(proposer) { return false; }
        if sorted_members(crate::agent::helpers::group_members_ctx(&self.task_ctx, group)) != next.members { return false; }
        let named_right = match (self.predecessor_of(group), &next.predecessor) {
            (Some(r), Some(p)) => p.group == r.group && p.retirement == r.digest() && next.exclusive_default == r.passes_default,
            (None, None) => true,
            _ => false,
        };
        if !named_right { return false; }
        let held = |e: u64| self.task_ctx.consensus_accepted.pin().get(genesis_slot(group, e).as_str())
            .and_then(|s| s.accepted.as_ref().map(|(_, a)| a.clone()));
        // A member the superseded genesis names must hold exactly it: its record (by digest), or — after a restart,
        // when only the digest of what it accepted survives — the very envelope the declaring member holds.
        if let Some(s) = &next.supersedes && s.members.contains(me) {
            let holds = match held(s.epoch) {
                Some(Accepted::DigestOnly(d)) => d == s.envelope,
                Some(a) => raw_accepted(&a).is_some_and(|r| value_digest(&r) == s.digest),
                None => false,
            };
            if !holds { return false; }
        }
        // One value per attempt: the same record (in any envelope), or after a restart the same envelope.
        match held(epoch) {
            None => true,
            Some(Accepted::DigestOnly(d)) => d == value_digest(as_proposed),
            Some(a) => raw_accepted(&a).is_some_and(|r| r == *value),
        }
    }

    /// A safety-sensitive proposal to a scope that is not an electorate group: refused
    /// [`ConsensusResult::ElectorateNotGoverned`] when the node requires an electorate
    /// (`consensus_require_electorate`), otherwise counted and let through as before. `None` for a
    /// proposal that is not safety-sensitive.
    fn refuse_ungoverned(&self, config: &ConsensusConfig, slot: &Arc<str>, group: Option<&Arc<str>>) -> Option<ConsensusResult> {
        if !crate::agent::electorate::is_safety_sensitive(config, slot) {
            return None;
        }
        if self.task_ctx.config.consensus_require_electorate {
            tracing::warn!(slot = %slot, group = ?group.map(|g| &**g),
                "consensus: refusing a safety-sensitive proposal outside an electorate group");
            return Some(ConsensusResult::ElectorateNotGoverned { slot: Arc::clone(slot), group: group.cloned() });
        }
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_ungoverned_safety_total").increment(1);
        None
    }

    /// Proposes `value` for `slot` requiring independent quorum from each group in `groups`.
    ///
    /// Commits only when every group reaches its configured [`GroupQuorum::quorum`] fraction.
    /// Uses a single ballot round so the commit is atomic — no group can commit without
    /// all others also committing.
    ///
    /// Called by [`crate::ConsensusHandle::cross_group_propose`].
    pub(crate) async fn cross_propose(
        &self,
        slot:   Arc<str>,
        value:  Bytes,
        groups: &[GroupQuorum],
        config: ConsensusConfig,
    ) -> ConsensusResult {
        let asked = value.clone();
        own_or_superseded(self.cross_propose_inner(slot, value, groups, config).await, &asked)
    }

    async fn cross_propose_inner(
        &self,
        slot:   Arc<str>,
        value:  Bytes,
        groups: &[GroupQuorum],
        config: ConsensusConfig,
    ) -> ConsensusResult {
        if groups.is_empty() {
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_consensus_timeouts_total", "reason" => "empty_groups")
                .increment(1);
            return ConsensusResult::Timeout {
                slot,
                ballots_tried: 0, votes_last_ballot: 0, quorum_required: 0,
            };
        }

        // P2, as at `propose_inner`'s door: an electorate group decides only through its own electorate, so a
        // cross-group proposal naming one is refused by name; a safety-sensitive cross-group proposal is not an
        // electorate, and is refused when the node requires one.
        let scope_groups = SignalScope::Groups(groups.iter().map(|g| Arc::from(g.group.as_str())).collect());
        if let Err(refused) = self.electorate_door(&scope_groups, &slot, &value, &config) {
            return refused.into_result(slot);
        }

        let ballot_key = format!("{}{}", consensus_ns::BALLOT,    &*slot);
        let commit_key = format!("{}{}", consensus_ns::COMMITTED, &*slot);

        // The value this proposer carries; replaced by any higher-balloted accepted value an
        // acceptor reports. `adopted_from` is the ballot it was accepted at (0 = ours).
        let mut value = value;
        let mut adopted_from: u64 = 0;

        // Epoch lease window (ms), with the same renewal exception as `propose`:
        // a live same-value leased commitment may be re-proposed to refresh it.
        let lease_ms = config.committed_lease_secs.map(|s| s.saturating_mul(1000));
        let superseded_by_live = |existing: &[u8; 32], current: &Bytes| -> bool {
            !(lease_ms.is_some() && raw_of(current).map(|r| value_digest(&r)).as_ref() == Some(existing))
        };

        if let Some(existing) = self.live_digest(&slot)
            && !(lease_ms.is_some() && existing == value_digest(&value)) {
                return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
            }

        // Per-group state (rebuilt from KV once before the ballot loop).

        let mut group_states: AHashMap<Arc<str>, CrossState> = AHashMap::new();
        for gq in groups {
            let prefix  = grp_prefix(&gq.group);
            let entries = scan_kv_prefix(&self.task_ctx.kv_state, &prefix);
            let members: ahash::AHashSet<NodeId> = entries.iter()
                .filter_map(|(key, _)| key.strip_prefix(&prefix).and_then(|s| s.parse().ok()))
                .collect();
            group_states.insert(Arc::from(gq.group.as_str()), CrossState {
                members,
                quorum_frac: gq.quorum.clamp(0.001, 1.0),
                accepts: 0,
                seen: ahash::AHashSet::new(),
            });
        }

        // Pre-compute voter → group membership for O(1) lookup during vote collection.
        let mut node_groups: AHashMap<NodeId, Vec<Arc<str>>> = AHashMap::new();
        for (gname, gs) in &group_states {
            for nid in &gs.members {
                node_groups.entry(nid.clone()).or_default().push(Arc::clone(gname));
            }
        }

        let scope = SignalScope::Groups(
            groups.iter().map(|g| Arc::from(g.group.as_str())).collect(),
        );

        let mut vote_rx = self.task_ctx.signal_handlers.register_with_capacity(
            Arc::from(consensus_kind::VOTE), 512,
        );
        let mut nack_rx = self.task_ctx.signal_handlers.register_with_capacity(
            Arc::from(consensus_kind::NACK), 64,
        );

        let Some(mut ballot) = next_ballot(self.read_ballot(&ballot_key).max(self.floor(&slot))) else {
            return self.ballots_exhausted(slot, 0, 0);
        };
        // Row A: the decision envelope (design §2.1), as in `propose`.
        let carry = self.build_carry(&slot, &value, lease_ms, ballot);
        value = carry.own.encode();
        let mut last = LastAttempt::None;

        for _attempt in 0..config.max_ballots {
            for gs in group_states.values_mut() { gs.begin_attempt(); }

            // Phase 1, with the same per-group quorum the commit needs — see `propose`.
            let ready = |p: &AHashSet<NodeId>| group_states.values().all(|gs| {
                gs.members.iter().filter(|m| p.contains(*m)).count()
                    >= cross_group_quorum(gs.members.len(), gs.quorum_frac)
            });
            let phase1 = self.prepare_phase(
                &mut vote_rx, &mut nack_rx, &scope, &slot, ballot, &value, &ready, None, None,
                config.phase1_timeout, &carry,
            ).await;
            let retry_floor = match phase1 {
                Phase1::Ready(Phase1Choice::Keep) => None,
                Phase1::Ready(Phase1Choice::Adopt(ab, v)) => {
                    adopted_from = ab;
                    value = v;
                    None
                }
                Phase1::DecidedOtherwise => {
                    return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
                }
                Phase1::Ready(Phase1Choice::Blocked) => {
                    last = LastAttempt::Blocked;
                    Some(0)
                }
                Phase1::Short(others) => {
                    last = LastAttempt::Prepare { others };
                    Some(0)
                }
                Phase1::Refused(seen) => {
                    last = LastAttempt::Contended;
                    Some(seen)
                }
                Phase1::Unrecorded => return self.unrecorded(slot, _attempt + 1, 0),
                // Unreachable: a cross-group proposal carries no electorate. Refused all the same.
                Phase1::Stale(seen) => return stale_result(slot, &scope, None, seen),
                Phase1::Closed => return gate_refused(slot, None),
            };
            if let Some(floor) = retry_floor {
                ballot_retry_pause(config.ballot_retry_jitter_ms).await;
                let Some(next) = next_ballot(floor.max(self.read_ballot(&ballot_key)).max(ballot)) else {
                    return self.ballots_exhausted(slot, _attempt + 1, 0);
                };
                ballot = next;
                continue;
            }
            last = LastAttempt::Vote;

            // A fresh refusal channel for phase 2 — see `propose`.
            nack_rx = self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(consensus_kind::NACK), 64,
            );

            // The node-level gate `propose` passes too: one value per ballot from this node, however
            // many proposals it runs for the slot at once. This proposer does not count its own vote
            // here; the claim is the gate, not a vote.
            let floor = self.floor(&slot);
            if !claim_envelope(&self.task_ctx.consensus_accepted, &slot, ballot, &value, self.task_ctx.node_id.id_hash(), floor) {
                last = LastAttempt::Contended;
                let Some(next) = next_ballot(ballot.max(self.read_ballot(&ballot_key))) else {
                    return self.ballots_exhausted(slot, _attempt + 1, 0);
                };
                ballot = next;
                continue;
            }
            if !self.persist_acceptor(&slot).await {
                return self.unrecorded(slot, _attempt + 1, 0);
            }
            self.raise_ballot(&ballot_key, ballot).await;

            let propose_msg = ConsensusMsg::ProposeTerm {
                slot: Arc::clone(&slot), ballot, envelope: value.clone(),
                proposer: self.task_ctx.node_id.clone(),
            };
            self.emit_async(
                Arc::from(consensus_kind::PROPOSE),
                scope.clone(),
                self.sign_payload(encode_consensus_msg(&propose_msg)),
            ).await;

            let deadline  = time::Instant::now() + config.phase1_timeout;
            let sleep_fut = time::sleep_until(deadline);
            tokio::pin!(sleep_fut);
            let mut nack_ballot = 0u64;
            // The highest `(accepted_ballot, value)` any acceptor reported this attempt. A
            // proposer that learns of an accepted value must adopt it rather than retry its own.
            let mut learned: Option<(u64, Bytes, bool)> = None;

            'collect: loop {
                tokio::select! { biased;
                    _ = &mut sleep_fut => break 'collect,
                    Some(sig) = vote_rx.recv() => {
                        // **Only a vote bound to this value counts.** An unbound legacy vote
                        // names a (slot, ballot) and nothing else, so a proposer that counted one
                        // could be counting a vote cast for a *different* value at the same
                        // ballot — which two concurrent proposers reach by construction, because
                        // ballots come from a shared key. See `ConsensusMsg::VoteForValue`.
                        let want = value_digest(&value);
                        let (s, b, voter) = match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::VoteForValue {
                                slot: s, ballot: b, voter, value_digest: d, ..
                            }) if d == want => (s, b, voter),
                            _ => continue 'collect,
                        };
                        if s != slot || b != ballot { continue 'collect; }

                        if let Some(gnames) = node_groups.get(&voter) {
                            for gname in gnames {
                                if let Some(gs) = group_states.get_mut(gname) {
                                    // Count each DISTINCT voter once — a re-delivered vote
                                    // (gossip re-flood / duplicate PROPOSE) must not inflate
                                    // the tally (the sibling `propose` path is NodeId-keyed too).
                                    gs.count(&voter);
                                }
                            }
                        }

                        // Commit when every group has independently reached its fraction.
                        let all_ready = group_states.values().all(|gs| {
                            gs.accepts >= cross_group_quorum(gs.members.len(), gs.quorum_frac)
                        });
                        if all_ready {
                            // Lost race with another proposer mid-ballot: refuse
                            // to clobber a different live commitment.
                            if let Some(existing) = self.live_digest(&slot)
                                && raw_of(&value).map(|r| value_digest(&r)) != Some(existing) {
                                    return ConsensusResult::Superseded {
                                        slot, ballot: self.read_ballot(&ballot_key),
                                    };
                                }
                            return self.commit_decision(&scope, &slot, ballot, &value, &commit_key, lease_ms).await;
                        }
                    }
                    Some(sig) = nack_rx.recv() => {
                        match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::PromiseTerm {
                                slot: s, seen_ballot, accepted_ballot, accepted, accepted_is_envelope,
                            }) if s == slot && seen_ballot >= ballot => {
                                nack_ballot = seen_ballot;
                                last = LastAttempt::Contended;
                                if let Some(v) = accepted
                                    && accepted_ballot >= learned.as_ref().map(|(b, _, _)| *b).unwrap_or(0) {
                                        learned = Some((accepted_ballot, v, accepted_is_envelope));
                                    }
                                break 'collect;
                            }
                            Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                                if s == slot && seen_ballot >= ballot => {
                                nack_ballot = seen_ballot;
                                last = LastAttempt::Contended;
                                break 'collect;
                            }
                            _ => {}
                        }
                    }
                }
            }

            if let Some(existing) = self.live_digest(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
                }

            ballot_retry_pause(config.ballot_retry_jitter_ms).await;
            // Adopt before retrying: a value an acceptor already holds outranks ours.
            if let Some((ab, v, is_env)) = learned.take()
                && ab >= adopted_from
                && let (_, _, Some(env)) = carry.report(&slot, ab, None, Some(v), is_env)
                && !carry.renews(&env) {
                    adopted_from = ab;
                    value = env;
                }
            let Some(next) = next_ballot(nack_ballot.max(self.read_ballot(&ballot_key)).max(ballot)) else {
                return self.ballots_exhausted(slot, _attempt + 1, 0);
            };
            ballot = next;
        }

        let votes_last_ballot: usize = group_states.values().map(|gs| gs.accepts).sum();
        // The cross tally counts every member's vote, this node's own included when it is a member that
        // runs a listener (its vote loops back) — so a lone member can read `quorum_short` here.
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_timeouts_total",
            "reason" => timeout_reason(last, votes_last_ballot))
            .increment(1);
        #[cfg(not(feature = "metrics"))]
        let _ = last;
        ConsensusResult::Timeout {
            slot,
            ballots_tried:     config.max_ballots,
            votes_last_ballot,
            quorum_required:   groups.len(),
        }
    }

    /// **Phase 1.** Promise `ballot` locally, ask `scope` to promise it, and collect promises until
    /// `ready` says the promisers form a quorum, a refusal arrives, or `timeout` elapses.
    ///
    /// Counts only [`PrepareAck`](ConsensusMsg::PrepareAck)s for this slot and ballot, from distinct
    /// acceptors (and, with trust slices, only declared ones), and decides the value with
    /// [`choose_after_prepare`]. A refusal at or above `ballot` ends the attempt: an acceptor has
    /// promised that ballot or a higher one to someone else.
    #[allow(clippy::too_many_arguments)]
    async fn prepare_phase(
        &self,
        vote_rx:   &mut mpsc::Receiver<Signal>,
        nack_rx:   &mut mpsc::Receiver<Signal>,
        scope:     &SignalScope,
        slot:      &Arc<str>,
        ballot:    u64,
        current:   &Bytes,
        ready:     &(dyn Fn(&AHashSet<NodeId>) -> bool + Sync),
        trust_set: Option<&AHashSet<u64>>,
        tag:       Option<&ElectorateTag>,
        timeout:   Duration,
        carry:     &Carry,
    ) -> Phase1 {
        let me = &self.task_ctx.node_id;
        let mut reports: Vec<AcceptReport> = Vec::new();
        // The ballots whose acceptance was reported **raw** (a legacy value, before it is wrapped) —
        // the only acceptances the legacy reading may set aside (below).
        let mut raw_ballots: Vec<u64> = Vec::new();
        let floor = self.floor(slot);
        // **The lazy carry** (P2 round 4; design record §8): a successor's phase 1 also reads a strict majority of
        // every retired predecessor. A predecessor this node belongs to is closed here first — before this node's
        // own promise reads its memory, which is shared across the groups it serves — so its own report is
        // complete.
        let preds: &[Arc<ElectorateDecl>] = tag.map_or(&[], |t| t.predecessors.as_slice());
        let mut heard: Vec<AHashSet<NodeId>> = vec![AHashSet::new(); preds.len()];
        for (p, got) in preds.iter().zip(heard.iter_mut()) {
            if p.contains(me) {
                if !self.close_for_carry(p).await { return Phase1::Unrecorded; }
                got.insert(me.clone());
            }
        }
        // This node's own promise passes the electorate's gate like any other claim (P2 round 2, finding 1).
        let gate = tag.map(|t| &t.gate);
        match self.claim_gated(gate, || prepare_slot(&self.task_ctx.consensus_accepted, slot, ballot, me.id_hash(), floor)) {
            None => return Phase1::Closed,
            Some(PrepareOutcome::Refused { promised, .. }) => return Phase1::Refused(promised),
            Some(PrepareOutcome::Promised(acc)) => {
                if let Some((b, a)) = acc {
                    let is_env = matches!(a, Accepted::Envelope(_));
                    if !is_env && a.value().is_some() { raw_ballots.push(b); }
                    reports.push(carry.report(slot, b, Some(a.digest()), a.value(), is_env));
                }
            }
        }
        if !self.persist_acceptor(slot).await { return Phase1::Unrecorded; }
        let mut promisers: AHashSet<NodeId> = AHashSet::new();
        promisers.insert(me.clone());
        let carried = |heard: &[AHashSet<NodeId>]| preds.iter().zip(heard).all(|(p, got)| got.len() >= p.quorum());

        if !(ready(&promisers) && carried(&heard)) {
            // Publish the ballot before asking, so a proposer drawing its next one starts above it
            // rather than colliding on it.
            self.raise_ballot(&format!("{}{}", consensus_ns::BALLOT, &**slot), ballot).await;
            let prepare = match tag {
                Some(t) => ConsensusMsg::PrepareIn {
                    slot: Arc::clone(slot), ballot, proposer: me.clone(), epoch: t.epoch, electorate: t.digest,
                },
                None => ConsensusMsg::PrepareTerm { slot: Arc::clone(slot), ballot, proposer: me.clone() },
            };
            self.emit_async(
                Arc::from(consensus_kind::PROPOSE), scope.clone(),
                self.sign_payload(encode_consensus_msg(&prepare)),
            ).await;
            // Each retired predecessor's members are asked one by one: they are not this group's members.
            for p in preds {
                let ask = self.sign_payload(encode_consensus_msg(&ConsensusMsg::CarryPrepare {
                    slot: Arc::clone(slot), ballot, proposer: me.clone(), group: Arc::from(p.group.as_str()),
                }));
                for m in p.members.iter().filter(|m| *m != me) {
                    self.emit_async(Arc::from(consensus_kind::PROPOSE), SignalScope::Individual(m.clone()), ask.clone()).await;
                }
            }
            let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            let sleep = mycelium_core::sim_seam::sleep_ms("consensus.prepare", ms);
            tokio::pin!(sleep);
            loop {
                tokio::select! { biased;
                    _ = &mut sleep => return Phase1::Short(promisers.len().saturating_sub(1)),
                    Some(sig) = vote_rx.recv() => {
                        match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::PrepareAckTerm {
                                slot: s, ballot: b, voter, accepted_ballot, accepted_digest,
                                accepted, accepted_is_envelope, committed_digest,
                            }) => {
                                if s != *slot || b != ballot { continue; }
                                if let Some(ts) = trust_set
                                    && !ts.contains(&voter.id_hash()) { continue; }
                                // A live commit of a different value ends this proposal. The same value — a
                                // leased slot being renewed — is one promise like any other: no shortcut
                                // past the quorum, whose reports still decide.
                                let current_raw = raw_of(current);
                                if committed_digest.is_some_and(|d| Some(d) != current_raw.as_ref().map(value_digest)) {
                                    return Phase1::DecidedOtherwise;
                                }
                                if accepted_ballot > 0 {
                                    // A digest is always sent with an acceptance; a value without one is
                                    // checked against its own digest by `choose_after_prepare`.
                                    if accepted_digest.is_none() && accepted.is_none() {
                                        // An acceptance we cannot identify constrains us without telling
                                        // us how: do not count this promiser.
                                        continue;
                                    }
                                    if !accepted_is_envelope && accepted.is_some() { raw_ballots.push(accepted_ballot); }
                                    reports.push(carry.report(slot, accepted_ballot, accepted_digest, accepted, accepted_is_envelope));
                                }
                                promisers.insert(voter);
                            }
                            // A retired predecessor's member reports what it holds (the lazy carry). Counted once per
                            // member of the predecessor it names; a promise it holds above this ballot, or at it to
                            // another proposer, means a ballot of the predecessor may sit above ours — move up first.
                            Some(ConsensusMsg::CarryReport {
                                slot: s, ballot: b, voter, group, promised, promised_to, accepted_ballot, accepted_digest,
                                accepted_value, accepted_is_envelope,
                            }) => {
                                if s != *slot || b != ballot { continue; }
                                let Some(i) = preds.iter().position(|p| *p.group == *group && p.contains(&voter)) else { continue };
                                if promised > ballot || (promised == ballot && promised_to != Some(me.id_hash())) {
                                    return Phase1::Refused(promised);
                                }
                                if accepted_ballot > 0 {
                                    if accepted_digest.is_none() && accepted_value.is_none() { continue; }
                                    if !accepted_is_envelope && accepted_value.is_some() { raw_ballots.push(accepted_ballot); }
                                    reports.push(carry.report(slot, accepted_ballot, accepted_digest, accepted_value, accepted_is_envelope));
                                }
                                heard[i].insert(voter);
                            }
                            _ => continue,
                        }
                        if ready(&promisers) && carried(&heard) { break; }
                    }
                    Some(sig) = nack_rx.recv() => {
                        // On an electorate group a refusal moves this proposal only from a member (P2 round 2,
                        // finding 5): the sender is bound to the electorate, as votes are.
                        if tag.is_some_and(|t| !t.sent_by_member(&sig.sender)) { continue; }
                        match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::Promise { slot: s, seen_ballot, .. })
                            | Some(ConsensusMsg::PromiseTerm { slot: s, seen_ballot, .. })
                                if s == *slot && seen_ballot >= ballot =>
                                return Phase1::Refused(seen_ballot),
                            Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                                if s == *slot && seen_ballot > ballot =>
                                return Phase1::Refused(seen_ballot),
                            // A member of this proposal's electorate holds a **later** electorate (a genesis that
                            // superseded this one): this proposer is stale — stop, by name. A member behind it, or
                            // one that closed the group (it names this electorate's own digest), simply does not
                            // answer; a non-member's refusal is not evidence about this electorate at all.
                            Some(ConsensusMsg::StaleElectorate { slot: s, voter, epoch, electorate })
                                if s == *slot && tag.is_some_and(|t| t.knows_a_later_electorate(&voter, epoch, &electorate)) =>
                                return Phase1::Stale(epoch),
                            _ => {}
                        }
                    }
                }
            }
        }

        // `current` is the proposer's own value, or one adopted from a refusal; the reports decide
        // whether it may still be carried. Acceptances at or below the decided ballot belong to the
        // previous decision — but are set aside **only when this node can see that decision is
        // over**: it holds the committed entry and it is not live (a lease expired, a lock released).
        // `decided` and `committed` travel as separate keys, so a node can learn the floor before
        // the commit; filtering then would hide a live decision and let a second value commit
        // (second review, M1). Without the filter the worst case is re-committing the old value.
        //
        // Row A: a slot whose top decision record has ended — released, or its lease past — sets
        // aside every acceptance at or below that record's ballot (design §2.3). The record carries
        // its own ballot, so this needs neither the committed entry nor the decided floor.
        set_aside_finished(&mut reports, 0, false, self.ended(slot));
        // `decided` sets aside **only acceptances reported raw** — a legacy proposer's — and only when
        // the legacy reading says that legacy decision is over (no decision record, the committed entry
        // present and not live). An envelope is never set aside by `decided` (review D2: `decided`
        // arrives with a commit's legacy keys ahead of its record and says nothing about whether *that*
        // decision ended). Without this, a legacy decision released the legacy way came back on the next
        // proposal, adopted and re-committed (the review of `ee3d9e4b`, finding 1).
        if self.legacy_over(slot) {
            let floor = self.decided_floor(slot);
            reports.retain(|r| !(r.0 <= floor && raw_ballots.contains(&r.0)));
        }
        Phase1::Ready(phase1_choice(current, &reports, carry))
    }

    /// Evaluates quorum-by-count + topology gate. When both pass, dispatches
    /// the commit (emit `COMMIT`, write `consensus/committed/{slot}` and the
    /// `consensus/lease/{slot}` window when leased, record the decided ballot under
    /// `consensus/decided/{slot}`) and returns `Some(ConsensusResult::Committed)`.
    /// Returns `None` when either gate fails — caller keeps collecting.
    ///
    /// Refuses to clobber a *different* live commitment that landed between the
    /// caller's supersession check and quorum being reached (a lost race with
    /// another proposer) — returns `Superseded` instead of overwriting.
    #[allow(clippy::too_many_arguments)]
    async fn try_commit_if_ready(
        &self,
        voters:      &AHashMap<NodeId, Option<LocalityPath>>,
        quorum_size: usize,
        group_name:  Option<&str>,
        scope:       &SignalScope,
        slot:        &Arc<str>,
        ballot:      u64,
        value:       &Bytes,
        ballot_key:  &str,
        commit_key:  &str,
        lease_ms:    Option<u64>,
        cert:        Option<&[Bytes]>,
    ) -> Option<ConsensusResult> {
        if voters.len() < quorum_size { return None; }
        let (passes, _, _) = self.topology_check(voters, group_name);
        if !passes { return None; }

        if let Some(existing) = self.live_digest(slot)
            && raw_of(value).map(|r| value_digest(&r)) != Some(existing) {
                return Some(ConsensusResult::Superseded {
                    slot:   Arc::clone(slot),
                    ballot: self.read_ballot(ballot_key),
                });
            }
        // A genesis or a retirement: its certificate — the decided envelope and the signed votes for it — goes into
        // the store **before** the COMMIT leaves, so a node that learns the commit can verify it (P2).
        let cert_at = crate::agent::electorate::parse_genesis_slot(slot).map(|(g, e)| crate::agent::electorate::cert_key(g, e))
            .or_else(|| crate::agent::electorate::parse_retire_slot(slot).map(crate::agent::electorate::retire_cert_key));
        if let (Some(votes), Some(key)) = (cert, cert_at) {
            let Ok(encoded) = mycelium_core::serde_fixint::to_vec(&(value.clone(), votes.to_vec())) else { return None; };
            let upd = self.set_async(&key, Bytes::from(encoded)).await;
            let _ = self.persist_sync(slot, &upd, "electorate certificate").await;
        }
        let res = self.commit_decision(scope, slot, ballot, value, commit_key, lease_ms).await;
        // A successor has decided this slot: its proposals stop carrying from its predecessors (P2 round 4).
        if let SignalScope::Group(g) = scope { self.note_decided_in(g, slot); }
        Some(res)
    }

    /// **Commit the decision envelope `envelope` at `ballot`** (design §3) — both commit paths
    /// (`propose`, `cross_propose`) come through here.
    ///
    /// The decision record (and a large value's content-addressed key) is applied and put on stable
    /// storage **first**; then the legacy keys older readers read (`committed` = the caller's value, the
    /// 8-byte `lease`, `decided`); then the `CommitTerm` signal, which carries the whole envelope, and
    /// the legacy `Commit` for older learners. A learner therefore never holds the decision without
    /// its window: the window is a field of the record, and every way the decision travels carries it.
    /// Returns `Committed` with the **envelope** as its value; `propose` / `cross_propose` translate.
    async fn commit_decision(
        &self,
        scope:      &SignalScope,
        slot:       &Arc<str>,
        ballot:     u64,
        envelope:   &Bytes,
        commit_key: &str,
        lease_ms:   Option<u64>,
    ) -> ConsensusResult {
        let env = crate::consensus_life::Envelope::decode(envelope);
        let raw = env.as_ref().and_then(|e| e.value.clone()).unwrap_or_default();
        let mut persisted = true;
        if let Some(env) = &env {
            let (record, external) = env.stored();
            if self.get(&crate::consensus_life::sentinel_key(slot)).is_none() {
                let upd = self.set_async(&crate::consensus_life::sentinel_key(slot),
                    Bytes::from_static(&crate::consensus_life::SENTINEL)).await;
                persisted &= self.persist_sync(slot, &upd, "decision sentinel").await;
            }
            if let Some(v) = external {
                let upd = self.set_async(&crate::consensus_life::value_key(slot, &env.value_digest), v).await;
                persisted &= self.persist_sync(slot, &upd, "decision value").await;
            }
            let upd = self.set_async(&crate::consensus_life::record_key(slot, ballot), record).await;
            persisted &= self.persist_sync(slot, &upd, "decision record").await;
        }
        // The legacy keys, for readers older than row A (design §7). An adopted envelope keeps its
        // original window, so the legacy lease follows the envelope, not this proposer's config.
        let legacy_lease = match env.as_ref().map(|e| e.term) {
            Some(crate::consensus_life::Term::Lease { ms, .. }) => Some(ms),
            Some(crate::consensus_life::Term::Permanent) => None,
            None => lease_ms,
        };
        let committed_upd = self.set_async(commit_key, raw.clone()).await;
        persisted &= self.persist_sync(slot, &committed_upd, "committed slot").await;
        persisted &= self.write_lease(slot, legacy_lease).await;
        persisted &= self.record_decided(slot, ballot).await;
        let commit_term = ConsensusMsg::CommitTerm {
            slot: Arc::clone(slot), ballot, envelope: envelope.clone(),
        };
        self.emit_async(
            Arc::from(consensus_kind::COMMIT), scope.clone(), self.sign_payload(encode_consensus_msg(&commit_term)),
        ).await;
        let commit = ConsensusMsg::Commit {
            slot: Arc::clone(slot), ballot, value: raw,
        };
        self.emit_async(
            Arc::from(consensus_kind::COMMIT), scope.clone(), self.sign_payload(encode_consensus_msg(&commit)),
        ).await;
        ConsensusResult::Committed {
            slot:   Arc::clone(slot),
            value:  envelope.clone(),
            ballot,
            persisted,
        }
    }

    /// Forces an already-applied record to stable storage (`append_sync` — `fdatasync` in every
    /// `SyncMode`); `what` names it in the log. Returns `false` on failure, which a commit
    /// surfaces as `Committed { persisted: false }` rather than swallowing: the cluster commit
    /// already happened (COMMIT emitted, value applied), so the honest report is "committed, not
    /// locally durable" — and which an acceptor answers by **not answering** (`persist_acceptor`).
    /// `true` when persistence is not configured — no promise was made.
    async fn persist_sync(&self, slot: &str, upd: &crate::framing::GossipUpdate, what: &str) -> bool {
        let Some(wal) = self.task_ctx.wal.get() else { return true; };
        match wal.append_sync(sync_entry_from(upd)).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(
                    slot = %slot, error = %e,
                    "consensus: {what} did not reach stable storage on this node",
                );
                false
            }
        }
    }

    /// The proposal ends here: the slot's next ballot would be above `u64::MAX` — its decided floor
    /// or ballot key is at the ceiling, which no history of a slot reaches by drawing one ballot per
    /// attempt, and a forged `consensus/decided/` entry does (the floor tripwire counts that). Until
    /// 2026-10-10 the draw was `… + 1`: a panic in a build with overflow checks — under the release
    /// profile's `panic = "abort"`, the node — and a wrap to ballot 0, refused below the floor,
    /// without them. A `Timeout`, named `ballot_exhausted` on the timeout metric and counted on
    /// this node's `SystemStats::consensus_ballot_space_exhausted`, with one `warn!`.
    fn ballots_exhausted(&self, slot: Arc<str>, ballots_tried: u32, quorum_required: usize) -> ConsensusResult {
        self.task_ctx.ballot_space_exhausted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_timeouts_total", "reason" => "ballot_exhausted").increment(1);
        tracing::warn!(slot = %slot, "consensus: the slot's ballot space is exhausted (the next ballot would \
            exceed u64::MAX — a decided floor or ballot key at the ceiling); the proposal ends here");
        ConsensusResult::Timeout { slot, ballots_tried, votes_last_ballot: 0, quorum_required }
    }

    /// The attempt ends here: this node's own acceptor record did not reach stable storage, so
    /// nothing left this node and nothing can be in flight. Reported as a `Timeout` with reason
    /// `unrecorded` — the attempt was not made — rather than retried against a failing disk.
    fn unrecorded(&self, slot: Arc<str>, ballots_tried: u32, quorum_required: usize) -> ConsensusResult {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("mycelium_consensus_timeouts_total", "reason" => "unrecorded").increment(1);
            metrics::counter!("mycelium_consensus_unrecorded_total", "role" => "proposer").increment(1);
        }
        ConsensusResult::Timeout { slot, ballots_tried, votes_last_ballot: 0, quorum_required }
    }

    /// Writes (or clears) the epoch-lease window for `slot` at commit time.
    ///
    /// The **legacy** lease, for readers older than row A (design §7). `Some(ms)` → write
    /// `consensus/lease/{slot}` (WAL-appended alongside the
    /// committed value so a restart cannot resurrect an expired slot as
    /// permanent). `None` → tombstone any stale lease left by a previous
    /// leased commitment, so the new permanent commit cannot be expired by it.
    ///
    /// Returns whether the lease record reached stable storage (`true` when there
    /// is nothing to persist) — folded into `Committed::persisted`.
    async fn write_lease(&self, slot: &Arc<str>, lease_ms: Option<u64>) -> bool {
        let lease_key = format!("{}{}", consensus_ns::LEASE, &**slot);
        match lease_ms {
            Some(ms) => {
                let upd = self.set_async(&lease_key, encode_lease_ms(ms)).await;
                self.persist_sync(slot, &upd, "lease for committed slot").await
            }
            None => {
                // The tombstone is on the same footing as the lease it clears: a restart that
                // replayed the old lease without it would expire the permanent commit for a reader
                // of the legacy keys. Written whether or not this node holds a lease.
                let upd = self.kv_delete(&lease_key);
                self.persist_sync(slot, &upd, "lease tombstone for committed slot").await
            }
        }
    }

    /// Drives one ballot's `'collect` loop: accept votes (Vote or
    /// VoteWithLocality, with optional trust-slice filtering), accept
    /// nacks, re-evaluate quorum on opacity recompute, all until
    /// `deadline` expires. Returns the outcome.
    ///
    /// `voters` and `quorum_size` are passed `&mut` so callers can read
    /// final state after a `Timeout` outcome (for the
    /// `TopologyUnsatisfied`-vs-`Timeout` decision at the end of
    /// `propose`).
    #[allow(clippy::too_many_arguments)]
    async fn collect_one_ballot(
        &self,
        voters:           &mut AHashMap<NodeId, Option<LocalityPath>>,
        quorum_size:      &mut usize,
        vote_rx:          &mut mpsc::Receiver<Signal>,
        nack_rx:          &mut mpsc::Receiver<Signal>,
        opaque_rx:        &mut Option<mpsc::Receiver<Signal>>,
        deadline:         time::Instant,
        slot:             &Arc<str>,
        ballot:           u64,
        value:            &Bytes,
        scope:            &SignalScope,
        ballot_key:       &str,
        commit_key:       &str,
        group_name:       Option<&str>,
        trust_set:        Option<&AHashSet<u64>>,
        opaque_recompute: Option<&OpaqueRecompute>,
        lease_ms:         Option<u64>,
        tag:              Option<&ElectorateTag>,
        mut cert:         Option<&mut Vec<Bytes>>,
    ) -> BallotOutcome {
        let sleep = time::sleep_until(deadline);
        tokio::pin!(sleep);

        loop {
            tokio::select! { biased;
                _ = &mut sleep => return BallotOutcome::Timeout,
                Some(sig) = vote_rx.recv() => {
                    // **Only a vote bound to this value counts.** `Vote` and `VoteWithLocality`
                    // name a (slot, ballot) and a voter and nothing else, so counting one risks
                    // counting a vote cast for a *different* value at the same ballot — which two
                    // concurrent proposers reach by construction, because ballots are drawn from a
                    // shared KV key and votes are broadcast to the group rather than sent to the
                    // proposer. The acceptor's `may_cast_vote` stops a voter accepting twice; it
                    // cannot stop the counting, because an unbound vote does not say what was
                    // accepted. See `ConsensusMsg::VoteForValue`.
                    let want = value_digest(value);
                    let (s, b, voter, locality) = match self.decode_verify(&sig.payload) {
                        Some(ConsensusMsg::VoteForValue {
                            slot: s, ballot: b, voter, value_digest: d, locality,
                        }) if d == want => (s, b, voter, locality),
                        _ => continue,
                    };
                    if s == *slot && b == ballot {
                        // Trust-slice filtering: only count votes from declared peers.
                        if let Some(ts) = trust_set
                            && !ts.contains(&voter.id_hash()) { continue; }
                        if let Some(c) = cert.as_deref_mut() && !voters.contains_key(&voter) {
                            c.push(sig.payload.clone());
                        }
                        voters.insert(voter, locality);
                        if let Some(res) = self.try_commit_if_ready(
                            voters, *quorum_size, group_name,
                            scope, slot, ballot, value, ballot_key, commit_key, lease_ms, cert.as_deref().map(Vec::as_slice),
                        ).await {
                            return BallotOutcome::Committed(res);
                        }
                        // Quorum count met but topology gate failed — keep
                        // collecting until timeout in case a more diverse voter
                        // arrives. (try_commit_if_ready returned None.)
                    }
                }
                Some(sig) = nack_rx.recv() => {
                    // On an electorate group only a member's refusal moves this ballot (P2 round 2, finding 5).
                    if tag.is_some_and(|t| !t.sent_by_member(&sig.sender)) { continue; }
                    match self.decode_verify(&sig.payload) {
                        // A `Promise` refuses **and reports**: the proposer must adopt the accepted
                        // value at the next ballot instead of retrying with its own.
                        Some(ConsensusMsg::PromiseTerm {
                            slot: s, seen_ballot, accepted_ballot, accepted, accepted_is_envelope,
                        }) if s == *slot && seen_ballot >= ballot =>
                            return BallotOutcome::NackHigher(
                                seen_ballot, accepted.map(|v| (accepted_ballot, v, accepted_is_envelope))),
                        Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                            if s == *slot && seen_ballot >= ballot =>
                            return BallotOutcome::NackHigher(seen_ballot, None),
                        Some(ConsensusMsg::StaleElectorate { slot: s, voter, epoch, electorate })
                            if s == *slot && tag.is_some_and(|t| t.knows_a_later_electorate(&voter, epoch, &electorate)) =>
                            return BallotOutcome::Stale(epoch),
                        _ => {}
                    }
                }
                // Re-evaluate quorum when a member turns opaque mid-ballot.
                // BOUNDARY_OPAQUE means the sender is now excluded from active_members,
                // shrinking the quorum threshold. If we already have enough votes, commit.
                Some(_) = async {
                    match opaque_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(or) = opaque_recompute {
                        let opaque = (or.count_opaque)();
                        let active = or.total_members.saturating_sub(opaque).max(1);
                        *quorum_size = if or.config_quorum > 0 { or.config_quorum } else { active / 2 + 1 };
                        if let Some(res) = self.try_commit_if_ready(
                            voters, *quorum_size, group_name,
                            scope, slot, ballot, value, ballot_key, commit_key, lease_ms, cert.as_deref().map(Vec::as_slice),
                        ).await {
                            return BallotOutcome::Committed(res);
                        }
                    }
                }
            }
        }
    }
}

/// What a proposer carries into phase 1 besides its current value (row A): its **own** envelope —
/// whose term and token wrap a legacy acceptance it must adopt (design §4, §7) — and the lineage it is
/// renewing, if any.
pub(crate) struct Carry {
    pub(crate) own:      crate::consensus_life::Envelope,
    pub(crate) renewing: Option<u64>,
    /// The term a **legacy** acceptance is wrapped in when adopted: this proposal's own lease, or —
    /// when this proposal is permanent — a lease of [`LEGACY_ADOPTION_LEASE_MS`]. Never permanent: an
    /// adopted legacy value has no original proposer in the scheme to release it, so a permanent wrap
    /// would make it unreleasable (the review of `ee3d9e4b`, finding 1). Fixed once per proposal, so two
    /// promisers reporting one legacy acceptance wrap it identically.
    pub(crate) legacy_term: crate::consensus_life::Term,
}

/// The window an adopted legacy value gets when the adopter asked for permanence — the default leader
/// lease (30 s): long enough for the value's holder to re-elect itself the upgraded way.
pub(crate) const LEGACY_ADOPTION_LEASE_MS: u64 = 30_000;

impl Carry {
    /// One acceptor's report as an [`AcceptReport`] over envelope bytes: an envelope as it is; a legacy
    /// value wrapped in an envelope with this proposer's term and token, lineage = the ballot it was
    /// accepted at — deterministic, so two promisers reporting one legacy acceptance agree; an
    /// acceptance known only by digest stays a digest (and blocks unless it is this proposal's).
    fn report(&self, slot: &str, ballot: u64, digest: Option<[u8; 32]>, value: Option<Bytes>, is_envelope: bool) -> AcceptReport {
        match (value, is_envelope) {
            (Some(env), true) => (ballot, value_digest(&env), Some(env)),
            (Some(raw), false) => {
                let wrapped = crate::consensus_life::Envelope::wrap_legacy(
                    slot, raw, self.legacy_term, self.own.proposer.clone(), self.own.token, ballot,
                ).encode();
                (ballot, value_digest(&wrapped), Some(wrapped))
            }
            (None, _) => (ballot, digest.unwrap_or([0u8; 32]), None),
        }
    }
}

impl Carry {
    /// Whether `env` is an acceptance of the lineage this proposal renews — this proposer's own live
    /// decision, which a renewal replaces rather than adopts (design §4).
    fn renews(&self, env: &Bytes) -> bool {
        self.renewing.is_some_and(|l| crate::consensus_life::Envelope::decode(env)
            .is_some_and(|e| e.lineage == l && e.proposer == self.own.proposer))
    }
}

/// **The phase-1 rule, as this proposer applies it** — pure, so it is tested on its own.
///
/// A renewal (design §4, Q2) replaces this proposer's own live decision with a new one of the same
/// lineage, but only when the **highest** acceptance the promise quorum reports is that lineage. It is
/// decided on the full report set (review D1): dropping the own-lineage reports and adopting the best of
/// the rest let a renewal adopt a *lower* value over its own higher decided acceptance; and a renewal
/// whose own lineage is reported **below** a foreign acceptance must adopt the foreign one, as any
/// proposer does. Otherwise [`choose_after_prepare`].
pub(crate) fn phase1_choice(current: &Bytes, reports: &[AcceptReport], carry: &Carry) -> Phase1Choice {
    if carry.renewing.is_some() && top_is_renewed_lineage(reports, carry) {
        return Phase1Choice::Keep;
    }
    choose_after_prepare(current, reports)
}

/// Whether every report at the highest reported ballot is an acceptance of the lineage `carry` renews
/// (and there is at least one) — the only case in which a renewal keeps its own new envelope.
fn top_is_renewed_lineage(reports: &[AcceptReport], carry: &Carry) -> bool {
    let Some(top) = reports.iter().map(|r| r.0).max() else { return false };
    reports.iter().filter(|r| r.0 == top).all(|r| r.2.as_ref().is_some_and(|b| carry.renews(b)))
}

/// The caller's value inside a decision envelope; `None` for bytes that are not an envelope.
pub(crate) fn raw_of(envelope: &Bytes) -> Option<Bytes> {
    crate::consensus_life::Envelope::decode(envelope).and_then(|e| e.value)
}

/// Phase 1's reports, with the previous decision's acceptances (at or below `floor`) set aside —
/// **only** when this node sees that decision is `over`; see `ConsensusEngine::prepare_phase`.
///
/// `ended` is the ballot of a decision this node's lifecycle record says is over ([`ended_at`], row
/// A); acceptances at or below it are set aside whatever `floor` and `over` say.
fn set_aside_finished(reports: &mut Vec<AcceptReport>, floor: u64, over: bool, ended: u64) {
    let finished = ended.max(if over { floor } else { 0 });
    reports.retain(|r| r.0 > finished);
}


/// How a proposal's latest ballot attempt ended — what its timeout is labelled by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "metrics", test)), allow(dead_code))]
enum LastAttempt {
    /// No attempt ran (`max_ballots = 0`).
    None,
    /// The prepare phase timed out with `others` acceptors besides this node promising.
    Prepare { others: usize },
    /// A quorum promised, but its highest acceptance is known only by digest and is not this value.
    Blocked,
    /// An acceptor had promised this ballot or a higher one to someone else, or this node's own acceptor
    /// refused the claim — another proposer is ahead.
    Contended,
    /// The voting phase ran; the timeout's count of other voters decides.
    Vote,
}

/// Why a proposal timed out, as `mycelium_consensus_timeouts_total` labels it — by **how many other
/// acceptors answered**, not by which phase the attempt ended in (the review of #579: since 2.30.0 every
/// attempt starts with a prepare phase, so a partition ends there too). `others` is the count for a
/// `Vote` attempt; a `Prepare` attempt carries its own.
///
/// `no_voters` — nobody else answered: a partition, or every acceptor older than 2.30.0 (they ignore
/// `Prepare`). `promise_short` — some promised, too few. `quorum_short` — some voted, too few.
/// `contended` — another proposer is ahead. `blocked` — the slot's top acceptance is known only by digest.
#[cfg(any(feature = "metrics", test))]
fn timeout_reason(last: LastAttempt, others: usize) -> &'static str {
    match last {
        LastAttempt::Prepare { others: 0 } | LastAttempt::None => "no_voters",
        LastAttempt::Prepare { .. } => "promise_short",
        LastAttempt::Blocked => "blocked",
        LastAttempt::Contended => "contended",
        LastAttempt::Vote if others == 0 => "no_voters",
        LastAttempt::Vote => "quorum_short",
    }
}

/// A `Committed` for a value other than `asked` becomes `Superseded` — see `ConsensusEngine::propose`.
///
/// Row A: the committed value is a decision envelope; what the caller receives is the value inside it,
/// and "its own" means that value is the one it asked for (an adoption of another's value is
/// `Superseded`; a bare value — a test's — is compared as it is).
fn own_or_superseded(result: ConsensusResult, asked: &Bytes) -> ConsensusResult {
    match result {
        ConsensusResult::Committed { slot, value, ballot, persisted } => {
            let raw = raw_of(&value).unwrap_or(value);
            if raw != *asked {
                ConsensusResult::Superseded { slot, ballot }
            } else {
                ConsensusResult::Committed { slot, value: raw, ballot, persisted }
            }
        }
        other => other,
    }
}

/// One group's tally in a cross-group proposal, per ballot attempt.
struct CrossState {
    members:     ahash::AHashSet<NodeId>,
    quorum_frac: f32,
    accepts:     usize,
    seen:        ahash::AHashSet<NodeId>, // distinct voters this attempt — each counts once
}

impl CrossState {
    /// A new ballot: its votes start from nothing. Clearing only `accepts` — as before the review of
    /// #579 — left last ballot's voters in `seen`, so a retry whose voters had voted before could never
    /// reach quorum, and its timeout read as nobody answering.
    fn begin_attempt(&mut self) {
        self.accepts = 0;
        self.seen.clear();
    }

    /// Counts `voter` once per attempt — a re-delivered vote must not inflate the tally.
    fn count(&mut self, voter: &NodeId) {
        if self.seen.insert(voter.clone()) {
            self.accepts += 1;
        }
    }
}

/// The random pause before a ballot retry — breaks lock-step livelock between proposers that
/// increment their ballots in unison. Through the replay seam, so a recording replays its draws.
async fn ballot_retry_pause(jitter_ms: u64) {
    if jitter_ms > 0 {
        let jitter = mycelium_core::sim_seam::rng_u64_below("consensus.ballot_jitter", jitter_ms);
        mycelium_core::sim_seam::sleep_ms("consensus.ballot_jitter", jitter).await;
    }
}

/// Outcome of `ConsensusEngine::prepare_phase`.
enum Phase1 {
    /// A quorum promised; the choice says what this ballot may carry.
    Ready(Phase1Choice),
    /// A promiser holds a live commit of a different value for the slot.
    DecidedOtherwise,
    /// An acceptor has promised this ballot (to another proposer) or a higher one.
    Refused(u64),
    /// No quorum promised within the timeout; carries how many acceptors **other than this node** did.
    Short(usize),
    /// This node's own promise did not reach stable storage, so the attempt stops before anything
    /// leaves: a promise a restart forgets is one this node can break.
    Unrecorded,
    /// A member of the proposal's electorate answered for another epoch (P2): carries the epoch it holds.
    Stale(u64),
    /// This node's own promise was refused by its electorate gate (P2): the group closed here, or the genesis
    /// attempt was overtaken, since the door.
    Closed,
}

/// Outcome of `ConsensusEngine::collect_one_ballot`.
enum BallotOutcome {
    /// Quorum + topology gate satisfied; commit dispatched.
    Committed(ConsensusResult),
    /// A refusal arrived with a strictly higher seen_ballot. Caller must
    /// re-issue at `>= seen_ballot + 1` to win on the next attempt — and, if the refusal was a
    /// [`Promise`](ConsensusMsg::Promise) reporting an accepted value, must **adopt that value**
    /// rather than retry with its own. Carries the highest `(accepted_ballot, accepted_value)`
    /// this attempt learned of, or `None` when nothing was reported.
    NackHigher(u64, Option<(u64, Bytes, bool)>),
    /// `phase1_timeout` elapsed. Caller may retry or surface TopologyUnsatisfied
    /// based on whether voters reached quorum-by-count.
    Timeout,
    /// A member of the proposal's electorate answered for another epoch (P2).
    Stale(u64),
}

// ── Wire encoding ─────────────────────────────────────────────────────────────

pub(crate) fn encode_consensus_msg(msg: &ConsensusMsg) -> Bytes {
    Bytes::from(mycelium_core::serde_fixint::to_vec(msg).unwrap_or_default())
}

pub(crate) fn decode_consensus_msg(bytes: &Bytes) -> Option<ConsensusMsg> {
    mycelium_core::serde_fixint::from_slice(bytes).ok()
}

/// Binds a signed consensus message's claimed authority to the verified signer: a node may only
/// PROPOSE and VOTE **as itself**. A signature proves who produced the bytes, not that the identity
/// named *inside* the message is that node — so without this check one node holding one valid key
/// could sign N votes each naming a different `voter` and forge a quorum (audit 2026-07-15 pass 2).
/// `Commit`/`Nack` carry no per-node authority field and are not bound here: a fully Byzantine-safe
/// commit needs a quorum certificate, which is out of scope — Mycelium is CFT, not BFT. This closes
/// the vote/propose *impersonation* vector, not all insider misbehaviour.
///
/// LIMITATION (audit 2026-07-15 pass 3): this bind is only as strong as the `signer → key` map it
/// verifies against, and that map (`peer_keys` / the `sys/identity/{node}` KV entry) is populated
/// from *unauthenticated* Layer-I gossip under the detection-not-prevention model. A **compromised
/// admitted** node can LWW-overwrite `sys/identity/{victim}` to append its own key to the victim's
/// verifying set and then sign votes as the victim — defeating this bind. That is a Byzantine-insider
/// attack, outside the CFT-not-BFT threat model; closing it needs authenticated identity-rotation
/// writes (mirroring how `sys/revocation/` validates signatures at read) and is tracked as a
/// follow-up. The bind still prevents impersonation among honest nodes and by any node that does not
/// hold a key present in the (possibly poisoned) target set — do not read it as insider-proof.
///
/// Only the `tls` decode path calls this (signed frames exist only with `tls`); it stays
/// compiled under `test` so the impersonation gates run in every feature set.
#[cfg(any(feature = "tls", test))]
fn signer_authorized(msg: &ConsensusMsg, signer: &NodeId) -> bool {
    match msg {
        ConsensusMsg::Vote { voter, .. }             => voter == signer,
        ConsensusMsg::VoteWithLocality { voter, .. } => voter == signer,
        ConsensusMsg::VoteForValue { voter, .. }     => voter == signer,
        ConsensusMsg::Propose { proposer, .. }       => proposer == signer,
        ConsensusMsg::Prepare { proposer, .. }       => proposer == signer,
        ConsensusMsg::PrepareAck { voter, .. }       => voter == signer,
        ConsensusMsg::PrepareTerm { proposer, .. }   => proposer == signer,
        ConsensusMsg::PrepareAckTerm { voter, .. }   => voter == signer,
        ConsensusMsg::ProposeTerm { proposer, .. }   => proposer == signer,
        ConsensusMsg::PrepareIn { proposer, .. }     => proposer == signer,
        ConsensusMsg::ProposeIn { proposer, .. }     => proposer == signer,
        ConsensusMsg::StaleElectorate { voter, .. }  => voter == signer,
        ConsensusMsg::CarryPrepare { proposer, .. }  => proposer == signer,
        ConsensusMsg::CarryReport { voter, .. }      => voter == signer,
        ConsensusMsg::Commit { .. } | ConsensusMsg::Nack { .. } | ConsensusMsg::Promise { .. }
        | ConsensusMsg::PromiseTerm { .. } | ConsensusMsg::CommitTerm { .. } => true,
    }
}

/// Acceptor anti-equivocation rule (single-decree safety): a voter must never accept two DIFFERENT
/// values at the same ballot. Returns `true` if this node may cast a vote for `(ballot, value)` given
/// the highest ballot it has voted at (`local_ballot`) and the value it accepted there (`prior_value`,
/// `None` if it has not voted this slot). At a strictly higher ballot it may always vote; at the same
/// ballot only for the *same* value (idempotent re-vote); at a lower ballot never. Without this, two
/// proposers racing the same fresh slot both get a ballot-1 vote from an overlapping voter and both
/// reach quorum with different values (audit 2026-07-15 pass 2).
/// The durable acceptor record's key for `slot` on this node.
#[cfg(feature = "consensus")]
pub(crate) fn accepted_key(node: &NodeId, slot: &str) -> String {
    format!("{}{}/{}", mycelium_core::signal::kv_ns::CONSENSUS_ACCEPTED, node, slot)
}

/// The ballot after `above`, or `None` when `above` is `u64::MAX` — every ballot draw goes through
/// here, so the ballot space ends in a named refusal ([`ConsensusEngine::ballots_exhausted`]) rather
/// than an overflow.
#[cfg(feature = "consensus")]
pub(crate) fn next_ballot(above: u64) -> Option<u64> {
    above.checked_add(1)
}

/// How far a decided floor may sit above every ballot this node has observed for its slot before
/// the floor's tripwire counts it: `2^32`.
///
/// Why this bound. A ballot is drawn one attempt at a time — `max(ballot key, floor) + 1` — so a
/// legitimate floor can exceed what this node has seen only by the attempts it missed: a node that
/// joined late, or whose copy of the ballot key lags the decided key it arrived with (two gossip
/// messages, no ordering between them). Missing `2^32` attempts on one slot is ~49 days of one
/// attempt per millisecond, which no slot's history contains; a forged floor near `u64::MAX`, the
/// value that exhausts the ballot space, is `2^63` or more past anything observed. A forged floor
/// *inside* the margin is not counted — and is also harmless: proposers draw above it.
#[cfg(feature = "consensus")]
pub(crate) const DECIDED_FLOOR_ANOMALY_MARGIN: u64 = 1 << 32;

/// The `promised_to` of a shrunk acceptor state (row A, C2): no proposer's `id_hash`, so the floor
/// refuses a prepare **and** an accept at the ended ballot itself — a promise to "nobody" at `e`
/// (a `None` there would admit any proposer at `e`, which is how a pre-2.30.0 record reads).
#[cfg(feature = "consensus")]
pub(crate) const FLOOR_SENTINEL: u64 = u64::MAX;

/// How many slots one collection pass examines (a record scan each) — the review's D3 bound on the
/// pass's synchronous work, whatever the history.
#[cfg(feature = "consensus")]
pub(crate) const ACCEPTOR_COLLECT_SCAN: usize = 1024;

/// How often a consensus listener collects acceptor state whose decision is over (row A, C2).
#[cfg(feature = "consensus")]
pub(crate) const ACCEPTOR_COLLECT_INTERVAL_MS: u64 = 60_000;

/// How many finished slots one collection pass collects at most (each costs up to two fsyncs); the
/// rest wait for the next tick (the adversarial review of #600, finding 4).
#[cfg(feature = "consensus")]
pub(crate) const ACCEPTOR_COLLECT_BUDGET: usize = 64;

/// How many slots the ballot tripwire remembers as anomalous (one `warn!` and one count each).
#[cfg(feature = "consensus")]
pub(crate) const ANOMALY_SLOTS_CAP: usize = 4096;

/// How many slots' verified COMMIT ballots this node keeps as tripwire evidence.
#[cfg(feature = "consensus")]
pub(crate) const VERIFIED_BALLOTS_CAP: usize = 65_536;

/// Whether `floor` is further than [`DECIDED_FLOOR_ANOMALY_MARGIN`] above `observed`, the highest
/// ballot this node has seen for the slot.
#[cfg(feature = "consensus")]
pub(crate) fn decided_floor_is_implausible(floor: u64, observed: u64) -> bool {
    floor.saturating_sub(observed) > DECIDED_FLOOR_ANOMALY_MARGIN
}

/// Encode the pre-2.30.0 acceptance-only record, `ballot(8, LE) ‖ digest(32)` — kept so the tests
/// can write one and check that [`decode_acceptor`] still reads it.
#[cfg(all(feature = "consensus", test))]
pub(crate) fn encode_accepted(ballot: u64, digest: [u8; 32]) -> Bytes {
    let mut v = Vec::with_capacity(40);
    v.extend_from_slice(&ballot.to_le_bytes());
    v.extend_from_slice(&digest);
    Bytes::from(v)
}

/// Decode a durable acceptor record. Anything malformed is **no record** — never a partial one,
/// because a half-understood memory is worse than an absent one: it would refuse votes it cannot
/// justify.
#[cfg(feature = "consensus")]
pub(crate) fn decode_accepted(bytes: &[u8]) -> Option<(u64, [u8; 32])> {
    if bytes.len() != 40 { return None; }
    let ballot = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    let mut d = [0u8; 32];
    d.copy_from_slice(&bytes[8..40]);
    Some((ballot, d))
}

/// Encode the whole acceptor state for a slot — the promise as well as the acceptance:
/// `0x02 ‖ promised(8) ‖ has_proposer(1) ‖ promised_to(8) ‖ has_accepted(1) ‖ accepted_ballot(8) ‖
/// digest(32) ‖ value(rest, optional)`.
///
/// A promise a restart forgets is one the acceptor can break: it would accept a lower ballot after
/// telling a higher proposer it would not, and that proposer's choice rests on the promise.
#[cfg(feature = "consensus")]
pub(crate) fn encode_acceptor(state: &AcceptorSlot) -> Bytes {
    let mut v = Vec::with_capacity(ACCEPTOR_RECORD_LEN + 1);
    // `0x03` (row A) is `0x02` with one byte after the digest saying whether the acceptance is a
    // decision envelope; written only when it is, so a record for a legacy acceptance stays `0x02`.
    let envelope = matches!(&state.accepted, Some((_, Accepted::Envelope(_))));
    v.push(if envelope { 0x03 } else { 0x02 });
    v.extend_from_slice(&state.promised.to_le_bytes());
    v.push(u8::from(state.promised_to.is_some()));
    v.extend_from_slice(&state.promised_to.unwrap_or(0).to_le_bytes());
    v.push(u8::from(state.accepted.is_some()));
    let (ab, d) = state.accepted.as_ref().map(|(b, a)| (*b, a.digest())).unwrap_or((0, [0u8; 32]));
    v.extend_from_slice(&ab.to_le_bytes());
    v.extend_from_slice(&d);
    if envelope { v.push(1); }
    // The value itself when it is small, so a restarted acceptor can still hand it to a proposer;
    // without it, a value known only by digest blocks every other proposal for the slot. Small,
    // because the record is gossiped cluster-wide and kept: a large value would be paid for on
    // every node, per acceptor, for good.
    if let Some((_, Accepted::Full(value) | Accepted::Envelope(value))) = &state.accepted
        && value.len() <= ACCEPTOR_RECORD_VALUE_CAP {
            v.extend_from_slice(value);
        }
    Bytes::from(v)
}

#[cfg(feature = "consensus")]
const ACCEPTOR_RECORD_LEN: usize = 1 + 8 + 1 + 8 + 1 + 8 + 32;

/// The largest accepted value the durable record carries; a larger one is recorded by digest only.
#[cfg(feature = "consensus")]
pub(crate) const ACCEPTOR_RECORD_VALUE_CAP: usize = 4096;

/// Decode a durable acceptor record in either shape: the current one ([`encode_acceptor`]) or the
/// 40-byte acceptance-only record written before 2.30.0, read as *promised at the accepted ballot,
/// to no named proposer* — which refuses a lower ballot and a different value at that ballot, the
/// most the old record can justify. Anything else is **no record**, never a partial one.
#[cfg(feature = "consensus")]
pub(crate) fn decode_acceptor(bytes: &[u8]) -> Option<AcceptorSlot> {
    if let Some((ballot, digest)) = decode_accepted(bytes) {
        return Some(AcceptorSlot {
            promised:    ballot,
            promised_to: None,
            accepted:    Some((ballot, Accepted::DigestOnly(digest))),
        });
    }
    let envelope = match bytes.first() { Some(0x02) => false, Some(0x03) => true, _ => return None };
    let head = ACCEPTOR_RECORD_LEN + usize::from(envelope);
    if bytes.len() < ACCEPTOR_RECORD_LEN { return None; }
    let u64_at = |i: usize| bytes[i..i + 8].try_into().ok().map(u64::from_le_bytes);
    let flag = |i: usize| match bytes[i] { 0 => Some(false), 1 => Some(true), _ => None };
    let promised = u64_at(1)?;
    let promised_to = flag(9)?.then_some(u64_at(10)?);
    let accepted = if flag(18)? {
        let ab = u64_at(19)?;
        let mut d = [0u8; 32];
        d.copy_from_slice(&bytes[27..59]);
        if ab > promised { return None; }
        if bytes.len() < head || (envelope && bytes[ACCEPTOR_RECORD_LEN] != 1) { return None; }
        let rest = &bytes[head..];
        if rest.is_empty() {
            Some((ab, Accepted::DigestOnly(d)))
        } else {
            let value = Bytes::copy_from_slice(rest);
            if value_digest(&value) != d { return None; }
            Some((ab, if envelope { Accepted::Envelope(value) } else { Accepted::Full(value) }))
        }
    } else {
        if envelope || bytes.len() != ACCEPTOR_RECORD_LEN { return None; }
        None
    };
    Some(AcceptorSlot { promised, promised_to, accepted })
}

/// **Recover this node's acceptor memory from its durable records.**
///
/// Called at startup **before any listener can vote**. Without it the acceptor's guarantee — *at
/// most one value per ballot* — would hold only for a process lifetime: a node that restarted
/// mid-ballot would forget what it accepted and could vote again, for a different value, at the
/// same ballot.
///
/// Recovered entries are [`Accepted::DigestOnly`]: enough to **refuse** a conflicting vote, which
/// is the safety property, and not enough to report a value in a `Promise`, which is only a
/// liveness aid. Malformed records are skipped rather than guessed at.
#[cfg(feature = "consensus")]
pub(crate) fn prewarm_accepted(
    kv_state: &crate::store::KvState,
    node:     &NodeId,
    accepted: &AcceptorMemory,
) -> usize {
    let prefix = format!("{}{}/", mycelium_core::signal::kv_ns::CONSENSUS_ACCEPTED, node);
    let mut recovered = 0;
    let guard = kv_state.store.pin();
    for (key, entry) in guard.iter() {
        let Some(slot) = key.strip_prefix(prefix.as_str()) else { continue };
        let Some(bytes) = entry.data.as_ref() else { continue };
        let Some(state) = decode_acceptor(bytes) else { continue };
        accepted.pin().insert(Arc::from(slot), state);
        recovered += 1;
    }
    recovered
}

/// This node's acceptor memory, one entry per slot, shared by its acceptor and proposer roles.
#[cfg(feature = "consensus")]
pub(crate) type AcceptorMemory = papaya::HashMap<Arc<str>, AcceptorSlot>;

/// What this node's acceptor holds for one slot: the ballot it has **promised** not to go below,
/// whom it promised it to, and what it has **accepted**.
///
/// `promised` is never below the accepted ballot: accepting at a ballot is also promising it.
#[cfg(feature = "consensus")]
#[derive(Clone, Debug, Default)]
pub(crate) struct AcceptorSlot {
    /// The highest ballot promised or accepted; `0` for a fresh slot.
    pub(crate) promised:    u64,
    /// The proposer (`NodeId::id_hash`) `promised` was promised to. `None` for a record recovered
    /// from the pre-2.30.0 shape, which did not name one.
    pub(crate) promised_to: Option<u64>,
    /// The ballot and value most recently accepted.
    pub(crate) accepted:    Option<(u64, Accepted)>,
}

/// Outcome of [`prepare_slot`].
#[cfg(feature = "consensus")]
#[derive(Clone, Debug)]
pub(crate) enum PrepareOutcome {
    /// The promise is recorded; carries what this acceptor had accepted.
    Promised(Option<(u64, Accepted)>),
    /// Refused: this acceptor has promised `promised` (to another proposer, when equal).
    Refused { promised: u64, accepted: Option<(u64, Accepted)> },
}

/// **Promise `ballot` to `proposer`, or refuse.** A ballot at or below `floor` — the slot's decided
/// ballot ([`consensus_ns::DECIDED`]) — belongs to a finished decision and is always refused.
/// Otherwise granted for a ballot above anything promised, and
/// for a re-sent prepare at the ballot already promised **to the same proposer**; refused for a
/// lower ballot or for a second proposer at an equal one — which is what keeps a ballot to one
/// proposer, and so to one value, when ballots are drawn from a shared key.
///
/// The `compute` closure re-reads the entry on every attempt and is retry-safe.
#[cfg(feature = "consensus")]
pub(crate) fn prepare_slot(
    memory:   &AcceptorMemory,
    slot:     &Arc<str>,
    ballot:   u64,
    proposer: u64,
    floor:    u64,
) -> PrepareOutcome {
    use papaya::Operation;
    let mut outcome = PrepareOutcome::Refused { promised: 0, accepted: None };
    memory.pin().compute(Arc::clone(slot), |entry| {
        let cur = entry.map(|(_, s)| s.clone()).unwrap_or_default();
        if ballot <= floor {
            outcome = PrepareOutcome::Refused { promised: cur.promised.max(floor), accepted: cur.accepted };
            return Operation::Abort(());
        }
        let granted = ballot > cur.promised
            || (ballot == cur.promised && cur.promised_to == Some(proposer));
        if granted {
            outcome = PrepareOutcome::Promised(cur.accepted.clone());
            Operation::Insert(AcceptorSlot { promised: ballot, promised_to: Some(proposer), ..cur })
        } else {
            outcome = PrepareOutcome::Refused { promised: cur.promised, accepted: cur.accepted };
            Operation::Abort(())
        }
    });
    outcome
}

/// One acceptor's report in a promise quorum: `(accepted_ballot, digest, value if known)`.
#[cfg(feature = "consensus")]
pub(crate) type AcceptReport = (u64, [u8; 32], Option<Bytes>);

/// What a proposer may propose after a promise quorum answered — [`choose_after_prepare`].
#[cfg(feature = "consensus")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase1Choice {
    /// No acceptor in the quorum had accepted anything: any value is safe, so keep the current one.
    Keep,
    /// Carry this value, accepted at this ballot — the highest any promiser reported.
    Adopt(u64, Bytes),
    /// The highest acceptance is known only by digest (its acceptors restarted), and it is not the
    /// current value: proposing anything would risk overwriting a chosen value, so do not.
    Blocked,
}

/// **The phase-1 rule.** Among the acceptances reported by acceptors that promised this ballot,
/// the one at the highest ballot decides: carry it. Classic Paxos, stated over digests so an
/// acceptor that restarted — and holds only the digest — still constrains the proposer.
///
/// Note what is *not* consulted: a value adopted earlier from a refusal. A value accepted outside
/// the promise quorum at a higher ballot was not chosen, and preferring it over the quorum's
/// highest could overwrite one that was.
#[cfg(feature = "consensus")]
pub(crate) fn choose_after_prepare(current: &Bytes, reports: &[AcceptReport]) -> Phase1Choice {
    let Some(top) = reports.iter().filter(|r| r.0 > 0).map(|r| r.0).max() else {
        return Phase1Choice::Keep;
    };
    let at_top = || reports.iter().filter(move |r| r.0 == top);
    // One value per ballot is what the rule rests on; two digests at one ballot means it did not
    // hold (a pre-2.30.0 proposer), and either choice could overwrite the chosen one.
    let first = at_top().next().map(|r| r.1);
    if at_top().any(|r| Some(r.1) != first) { return Phase1Choice::Blocked; }
    if let Some(v) = at_top().find_map(|r| r.2.clone().filter(|v| value_digest(v) == r.1)) {
        return Phase1Choice::Adopt(top, v);
    }
    let mine = value_digest(current);
    if at_top().all(|r| r.1 == mine) { Phase1Choice::Keep } else { Phase1Choice::Blocked }
}

/// What this node accepted at a ballot.
///
/// Two shapes, because two requirements pull apart. `Promise` must report the **value** so a
/// proposer at a higher ballot can adopt it, which needs the bytes. Surviving a restart needs a
/// record small enough to write on the voting path, which wants a digest. So the live entry keeps
/// the value, and the durable record keeps the digest — and a node that has restarted holds
/// [`DigestOnly`](Self::DigestOnly): still able to **refuse** a conflicting vote, which is the
/// safety property, but unable to help a proposer adopt, which is only liveness.
#[cfg(feature = "consensus")]
#[derive(Clone, Debug)]
pub(crate) enum Accepted {
    /// Accepted in this process: the value is known and can be reported in a `Promise`.
    Full(Bytes),
    /// Recovered from the durable record after a restart: the digest is known, the value is not.
    DigestOnly([u8; 32]),
    /// A decision envelope (`consensus_life::Envelope`, row A) accepted from a `ProposeTerm`: the
    /// value Paxos decided is the envelope, so reporting it carries the decision's window, lineage,
    /// proposer and token to an adopter.
    Envelope(Bytes),
}

#[cfg(feature = "consensus")]
impl Accepted {
    /// The digest of whatever was accepted — the only thing equality is ever asked about.
    pub(crate) fn digest(&self) -> [u8; 32] {
        match self {
            Accepted::Full(v) | Accepted::Envelope(v) => value_digest(v),
            Accepted::DigestOnly(d) => *d,
        }
    }
    /// The value for an **older** proposer: a legacy acceptance's value, never an envelope's.
    pub(crate) fn legacy_value(&self) -> Option<Bytes> {
        match self {
            Accepted::Full(v) => Some(v.clone()),
            Accepted::Envelope(_) | Accepted::DigestOnly(_) => None,
        }
    }

    /// The value, when this node still has it. `None` after a restart.
    pub(crate) fn value(&self) -> Option<Bytes> {
        match self {
            Accepted::Full(v) | Accepted::Envelope(v) => Some(v.clone()),
            Accepted::DigestOnly(_) => None,
        }
    }
}

/// **Claim this node's single vote at `ballot` for `value`, or refuse.**
///
/// The one place both of this node's roles pass through. A node is an acceptor when it answers
/// someone else's `Propose`, and *also* an acceptor when it proposes — proposing a value **is**
/// accepting it — so both must consult and update the same memory, or the node equivocates with
/// itself: accept `v_X` at ballot 1 as a voter, self-vote `v_Y` at ballot 1 as a proposer, and two
/// proposers can each reach quorum with a different value at one ballot.
///
/// Returns `true` when the claim is recorded and the caller may vote. The `compute` closure is a
/// compare-and-set and therefore **retry-safe**: it re-reads the current entry on every attempt and
/// never acts on a value captured outside the closure.
///
/// Since 2.30.0 the claim also honours the **promise**: a ballot below the one promised is refused,
/// and at the promised ballot only the proposer it was promised to may claim — see
/// [`ConsensusMsg::Prepare`]. A ballot at or below `floor`, the slot's decided ballot, is refused.
///
/// The memory is **not erased on commit**. It used to be, to bound the record prefix, and that
/// dropped promises: a delayed lower-ballot proposal reached acceptors that had forgotten what they
/// promised and could commit a second value (2026-10-08 review). Since row A (2.32.0) it is
/// **collected once its decision is over** — released, or its lease lapsed — and only when it
/// promises nothing above that decision's ballot, after the decided floor has been raised to it on
/// stable storage ([`ConsensusEngine::collect_finished`]). A permanent decision's memory is kept.
#[cfg(feature = "consensus")]
pub(crate) fn claim_vote(
    memory:   &AcceptorMemory,
    slot:     &Arc<str>,
    ballot:   u64,
    value:    &Bytes,
    proposer: u64,
    floor:    u64,
) -> bool {
    claim(memory, slot, ballot, Accepted::Full(value.clone()), proposer, floor)
}

/// [`claim_vote`] for a decision envelope (row A): the same rule, recorded as
/// [`Accepted::Envelope`] so it is reported flagged.
#[cfg(feature = "consensus")]
pub(crate) fn claim_envelope(
    memory:   &AcceptorMemory,
    slot:     &Arc<str>,
    ballot:   u64,
    envelope: &Bytes,
    proposer: u64,
    floor:    u64,
) -> bool {
    claim(memory, slot, ballot, Accepted::Envelope(envelope.clone()), proposer, floor)
}

#[cfg(feature = "consensus")]
fn claim(
    memory:   &AcceptorMemory,
    slot:     &Arc<str>,
    ballot:   u64,
    accept:   Accepted,
    proposer: u64,
    floor:    u64,
) -> bool {
    use papaya::Operation;
    let want = accept.digest();
    let mut granted = false;
    memory.pin().compute(Arc::clone(slot), |entry| {
        // Recomputed from scratch on every retry — never from a prior attempt's result.
        let cur = entry.map(|(_, s)| s.clone()).unwrap_or_default();
        granted = ballot > floor && may_accept(&cur, ballot, want, proposer);
        if granted {
            Operation::Insert(AcceptorSlot {
                promised:    ballot,
                promised_to: Some(proposer),
                accepted:    Some((ballot, accept.clone())),
            })
        } else {
            Operation::Abort(())
        }
    });
    granted
}

/// The accept rule over a slot's state: above the promise, always; below it, never; at it, only
/// for the proposer it was promised to (or no named one — a pre-2.30.0 record) **and** only for the
/// value already accepted there, if any. Comparison is by digest, so a value recovered from the
/// durable record after a restart is as decisive as one accepted in this process.
#[cfg(feature = "consensus")]
fn may_accept(cur: &AcceptorSlot, ballot: u64, digest: [u8; 32], proposer: u64) -> bool {
    use std::cmp::Ordering::*;
    match ballot.cmp(&cur.promised) {
        Less    => false,
        Greater => true,
        Equal   => {
            let same_proposer = cur.promised_to.is_none_or(|p| p == proposer);
            let prior = cur.accepted.as_ref().filter(|(b, _)| *b == ballot).map(|(_, a)| a.digest());
            same_proposer && may_cast_vote_digest(ballot, prior, ballot, digest)
        }
    }
}

/// `may_cast_vote` over digests — the form both roles and the restart path share.
#[cfg(feature = "consensus")]
fn may_cast_vote_digest(
    local_ballot: u64,
    prior:        Option<[u8; 32]>,
    ballot:       u64,
    digest:       [u8; 32],
) -> bool {
    use std::cmp::Ordering::*;
    match ballot.cmp(&local_ballot) {
        Less    => false,
        Greater => true,
        Equal   => prior.is_none_or(|p| p == digest),
    }
}
pub(crate) fn encode_ballot(ballot: u64) -> Bytes {
    Bytes::copy_from_slice(&ballot.to_le_bytes())
}

pub(crate) fn decode_ballot(bytes: &Bytes) -> u64 {
    if bytes.len() >= 8 {
        u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]))
    } else {
        0
    }
}

/// Wrapper used when `tls` is enabled: the raw `ConsensusMsg` bytes plus an
/// Ed25519 signature over them, so forged ballots can be detected and dropped.
/// Encoded/decoded with the same `bincode_cfg()` as `ConsensusMsg` itself.
///
/// The TLS transport already prevents unauthenticated TCP connections; this adds a second layer that
/// authenticates the *origin* of each consensus message and binds a `Vote`/`Propose` to the signer's
/// identity (`signer_authorized`) — so a node cannot impersonate another when voting, and a relayed
/// or replayed ballot with a bad/unknown signature is dropped. It is **not** full Byzantine
/// tolerance: it does not require a quorum certificate on `Commit`, so a compromised insider can
/// still emit other malformed protocol messages. Mycelium is CFT, not BFT (`framing.rs` signing
/// doc); this raises the bar against impersonation, it does not make consensus BFT.
#[cfg(feature = "tls")]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct SignedConsensusMsg {
    pub msg_bytes: Bytes,
    pub signer:    NodeId,
    pub signature: Vec<u8>,
}

/// The domain tag a consensus message is signed under.
#[cfg(feature = "tls")]
pub(crate) const CONSENSUS_MSG_DOMAIN: &[u8] = b"mycelium.consensus/msg/1";

/// The message a node signs for a consensus payload: `mycelium.consensus/msg/1 ‖ len(u32 LE) ‖
/// bytes`. The payload used to be signed bare, and a `PrepareAck` or `Promise` carries a
/// proposer-chosen value back, so a signed answer was a signature over bytes another node shaped —
/// which the identity proof, also bare over `32 × N` bytes, accepted as a proof of whatever keys
/// those bytes held. The identity proof now requires its own tag (`helpers::identity_proof_message`),
/// which closes that on its own; this tag is defence in depth, so no consensus signature verifies
/// as anything but a consensus signature. The frame is unchanged: `msg_bytes` carries the payload
/// bare, only the signed message gains the prefix.
#[cfg(feature = "tls")]
pub(crate) fn consensus_signing_message(bytes: &[u8]) -> Vec<u8> {
    let mut out = CONSENSUS_MSG_DOMAIN.to_vec();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Which form a verified consensus signature took.
///
/// `Untagged` is the pre-2.32.0 form — a bare signature over the payload — accepted for **one
/// release** so a fleet can upgrade node by node (a 2.31 acceptor's votes still count at a 2.32
/// proposer, and a 2.31 proposer's proposals are still answered); counted in
/// [`untagged_consensus_signatures_accepted`] and closed in the next MINOR
/// (`docs/guide/deprecations.md` §23). A 2.31 node cannot verify a tagged signature, so while any
/// node is un-upgraded a 2.32 proposer's messages are dropped there — the same shape as 2.30.0's
/// `Prepare` window.
/// An acceptor **answers in the form of the request it verified** (`sign_payload_as`): a 2.31
/// proposer verifies bare only, so its bare `Prepare`/`Propose` gets a bare answer and its rounds
/// complete; a tagged request gets a tagged answer. Found by the adversarial review of #585 — the
/// first cut answered everything tagged, which dropped a 2.32 acceptor out of every un-upgraded
/// proposer's rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "tls"), allow(dead_code))] // `Untagged` is only ever verified under `tls`
pub(crate) enum SignatureForm { Tagged, Untagged }

/// Verifies `sig` over `msg_bytes` against any key in `keys`: the tagged form first, then the
/// untagged one. `None` when neither verifies, or when there is no key to verify against.
#[cfg(feature = "tls")]
pub(crate) fn verify_consensus_signature(keys: &[[u8; 32]], msg_bytes: &[u8], sig: &[u8]) -> Option<SignatureForm> {
    if keys.is_empty() { return None; }
    let tagged = consensus_signing_message(msg_bytes);
    if keys.iter().any(|k| crate::tls::verify_bytes(k, &tagged, sig)) {
        return Some(SignatureForm::Tagged);
    }
    if keys.iter().any(|k| crate::tls::verify_bytes(k, msg_bytes, sig)) {
        return Some(SignatureForm::Untagged);
    }
    None
}

/// Answers this node's acceptor **withheld** because its record did not reach the WAL — a prepare
/// not acked, a proposal not voted (`persist_acceptor`). Beside the proposer's own
/// `mycelium_consensus_timeouts_total{reason="unrecorded"}`; both also count on
/// `mycelium_consensus_unrecorded_total{role}`. Non-zero means this node's disk is refusing the
/// fsync a promise needs, and it is silently absent from every round it is asked into.
#[cfg(feature = "consensus")]
static ACCEPTOR_ANSWERS_UNRECORDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many answers this node's acceptor withheld because its record did not reach the WAL.
#[cfg(feature = "consensus")]
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // read by the gateway's stats route
pub(crate) fn acceptor_answers_unrecorded() -> u64 {
    ACCEPTOR_ANSWERS_UNRECORDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Consensus signatures this process accepted in the untagged (pre-2.32.0) form — see
/// [`SignatureForm`]. Non-zero means a peer is not yet upgraded.
#[cfg(feature = "tls")]
static UNTAGGED_CONSENSUS_SIGNATURES_ACCEPTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many untagged consensus signatures this process has accepted under the mixed-fleet allowance.
#[cfg(feature = "tls")]
#[cfg_attr(not(feature = "gateway"), allow(dead_code))] // read by the gateway's stats route
pub(crate) fn untagged_consensus_signatures_accepted() -> u64 {
    UNTAGGED_CONSENSUS_SIGNATURES_ACCEPTED.load(std::sync::atomic::Ordering::Relaxed)
}

// ── Voter task ───────────────────────────────────────────────────────────────

/// Answers a [`Prepare`](ConsensusMsg::Prepare): promise and report, or refuse and say what was
/// promised. The promise is **on stable storage before the answer leaves** — applied to the store,
/// then handed to the WAL and fsynced (`persist_acceptor`), the same rung a vote's record and a
/// commit reach — because a proposer chooses its value on the strength of it, and a restart must
/// not let this node break it. Until 2.32.0 this said "handed to the WAL" and was not: the record
/// was applied and gossiped only. A promise that does not reach the WAL is **not answered**, and
/// the withheld answer is counted (`acceptor_answers_unrecorded`). The answer is signed in `form`,
/// the form the prepare arrived in — see [`SignatureForm`].
#[cfg(feature = "consensus")]
async fn answer_prepare(
    ctx: &ConsensusEngine, slot: Arc<str>, ballot: u64, proposer: NodeId, form: SignatureForm, term: bool, gate: Option<Gate>,
) {
    let floor = ctx.floor(&slot);
    // P2: the promise passes the electorate's gate, which only decides; a retirement's promise closes the group
    // first. A gate that refused sends no promise (round 3, finding 5).
    let outcome = ctx.claim_gated(gate.as_ref(), || prepare_slot(&ctx.task_ctx.consensus_accepted, &slot, ballot, proposer.id_hash(), floor));
    let Some(outcome) = outcome else {
        if let Some(g) = &gate {
            let digest = crate::agent::electorate::cached(&ctx.task_ctx, &g.group).map_or([0; 32], |d| d.digest());
            refuse_stale(ctx, slot, proposer, (g.epoch + 1, digest), form);
        }
        return;
    };
    match outcome {
        PrepareOutcome::Promised(acc) => {
            if !ctx.persist_acceptor(&slot).await {
                note_answer_unrecorded(&slot, ballot, "not answering the prepare");
                return;
            }
            let accepted_ballot = acc.as_ref().map(|(b, _)| *b).unwrap_or(0);
            let accepted_digest = acc.as_ref().map(|(_, a)| a.digest());
            let committed_digest = ctx.live_digest(&slot);
            let ack = if term {
                ConsensusMsg::PrepareAckTerm {
                    slot: Arc::clone(&slot), ballot, voter: ctx.task_ctx.node_id.clone(),
                    accepted_ballot, accepted_digest,
                    accepted_is_envelope: matches!(acc, Some((_, Accepted::Envelope(_)))),
                    accepted: acc.and_then(|(_, a)| a.value()),
                    committed_digest,
                }
            } else {
                // An older proposer: an envelope acceptance is reported **by digest only**, so it is
                // blocked rather than committing envelope bytes as a value (design §7).
                ConsensusMsg::PrepareAck {
                    slot: Arc::clone(&slot), ballot, voter: ctx.task_ctx.node_id.clone(),
                    accepted_ballot, accepted_digest,
                    accepted_value: acc.and_then(|(_, a)| a.legacy_value()),
                    committed_digest,
                }
            };
            ctx.emit(
                Arc::from(consensus_kind::VOTE),
                SignalScope::Individual(proposer),
                ctx.sign_payload_as(encode_consensus_msg(&ack), form),
            );
        }
        PrepareOutcome::Refused { promised, accepted } => {
            emit_refusal(ctx, &slot, promised, accepted, proposer, form, term, false);
        }
    }
}

/// A refusal reporting what this acceptor holds: `PromiseTerm` (flagged) to an upgraded proposer,
/// the legacy `Promise` (an envelope withheld) to an older one — and, refusing a *proposal*, the
/// legacy `Nack` too, for a proposer that predates `Promise` (a prepare's refusal never had one).
#[cfg(feature = "consensus")]
#[allow(clippy::too_many_arguments)]
fn emit_refusal(
    ctx: &ConsensusEngine, slot: &Arc<str>, seen: u64, held: Option<(u64, Accepted)>,
    proposer: NodeId, form: SignatureForm, term: bool, with_nack: bool,
) {
    let accepted_ballot = held.as_ref().map(|(b, _)| *b).unwrap_or(0);
    let refusal = if term {
        ConsensusMsg::PromiseTerm {
            slot: Arc::clone(slot), seen_ballot: seen, accepted_ballot,
            accepted_is_envelope: matches!(held, Some((_, Accepted::Envelope(_)))),
            accepted: held.and_then(|(_, a)| a.value()),
        }
    } else {
        ConsensusMsg::Promise {
            slot: Arc::clone(slot), seen_ballot: seen, accepted_ballot,
            accepted_value: held.and_then(|(_, a)| a.legacy_value()),
        }
    };
    ctx.emit(
        Arc::from(consensus_kind::NACK),
        SignalScope::Individual(proposer.clone()),
        ctx.sign_payload_as(encode_consensus_msg(&refusal), form),
    );
    if !with_nack { return; }
    let nack = ConsensusMsg::Nack { slot: Arc::clone(slot), seen_ballot: seen };
    ctx.emit(
        Arc::from(consensus_kind::NACK),
        SignalScope::Individual(proposer),
        ctx.sign_payload_as(encode_consensus_msg(&nack), form),
    );
}

/// A retired group's member answering a successor's [`CarryPrepare`](ConsensusMsg::CarryPrepare) (P2, round 4;
/// design record §8): only a member of the retired `group`, only when the retirement verifies here, only for a
/// proposer in a group that succeeds it, and never for a slot that names a group. The member closes the group for
/// good and puts the retirement on stable storage first (`close_for_carry`), then reports its acceptor memory for
/// the slot — read after the close, so an ordinary acceptance in flight is either in the report or refused.
#[cfg(feature = "consensus")]
async fn answer_carry(ctx: &ConsensusEngine, slot: Arc<str>, ballot: u64, proposer: NodeId, group: Arc<str>, form: SignatureForm) {
    let me = &ctx.task_ctx.node_id;
    let Some(d) = ctx.electorate_view(&group) else { return };
    if !d.contains(me) || ctx.retirement_view(&group).is_none() { return; }
    if crate::agent::electorate::slot_names_group(&slot).is_some() { return; }
    if !ctx.successors_of(&group).iter().any(|s| s.contains(&proposer)) { return; }
    if !ctx.close_for_carry(&d).await {
        note_answer_unrecorded(&slot, ballot, "not reporting to a successor");
        return;
    }
    let state = ctx.task_ctx.consensus_accepted.pin().get(&slot).cloned().unwrap_or_default();
    let (accepted_ballot, held) = state.accepted.map_or((0, None), |(b, a)| (b, Some(a)));
    let report = ConsensusMsg::CarryReport {
        slot, ballot, voter: me.clone(), group,
        promised: state.promised, promised_to: state.promised_to, accepted_ballot,
        accepted_digest: held.as_ref().map(Accepted::digest),
        accepted_is_envelope: matches!(held, Some(Accepted::Envelope(_))),
        accepted_value: held.and_then(|a| a.value()),
    };
    ctx.emit(
        Arc::from(consensus_kind::VOTE),
        SignalScope::Individual(proposer),
        ctx.sign_payload_as(encode_consensus_msg(&report), form),
    );
}

/// The caller's value an acceptance holds — an envelope's value, a legacy value as it is; `None` when only the
/// digest survived a restart.
#[cfg(feature = "consensus")]
fn raw_accepted(a: &Accepted) -> Option<Bytes> {
    match a {
        Accepted::Envelope(e) => raw_of(e),
        Accepted::Full(v) => Some(v.clone()),
        Accepted::DigestOnly(_) => None,
    }
}

/// Refuses a proposal whose electorate is not this node's own, naming the electorate it holds (P2).
#[cfg(feature = "consensus")]
fn refuse_stale(ctx: &ConsensusEngine, slot: Arc<str>, proposer: NodeId, own: (u64, [u8; 32]), form: SignatureForm) {
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_electorate_stale_refusals_total").increment(1);
    tracing::debug!(slot = %slot, proposer = %proposer, epoch = own.0, "consensus: refusing a proposal for another electorate");
    let refusal = ConsensusMsg::StaleElectorate {
        slot, voter: ctx.task_ctx.node_id.clone(), epoch: own.0, electorate: own.1,
    };
    ctx.emit(
        Arc::from(consensus_kind::NACK),
        SignalScope::Individual(proposer),
        ctx.sign_payload_as(encode_consensus_msg(&refusal), form),
    );
}

/// An acceptor's record did not reach the WAL, so its answer is withheld: counted and warned.
#[cfg(feature = "consensus")]
fn note_answer_unrecorded(slot: &str, ballot: u64, what: &str) {
    ACCEPTOR_ANSWERS_UNRECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_consensus_unrecorded_total", "role" => "acceptor").increment(1);
    tracing::warn!(slot = %slot, ballot, "consensus: acceptor record not on stable storage; {what}");
}

/// **The acceptor collector** (row A, C2): every [`ACCEPTOR_COLLECT_INTERVAL_MS`], through the timer
/// seam, collects up to [`ACCEPTOR_COLLECT_BUDGET`] slots whose decision is over
/// ([`ConsensusEngine::collect_finished`]). A task of its own, started beside the listener and stopped
/// with it, so its fsyncs never delay a vote (the adversarial review of #600, finding 4). It runs
/// concurrently with the listener's and the proposer's acceptor writes; `acceptor_records` and the
/// removal's compare-and-set serialise them.
#[cfg(feature = "consensus")]
pub(crate) async fn run_acceptor_collector(
    ctx:             ConsensusEngine,
    mut cancel:      oneshot::Receiver<()>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut collect = mycelium_core::sim_seam::interval_ms(
        "consensus/collect", ACCEPTOR_COLLECT_INTERVAL_MS, time::MissedTickBehavior::Skip,
    );
    // The interval's first tick is immediate; the first pass runs one interval after the listener
    // starts, so a just-started node is not collecting while it is still catching up.
    collect.tick().await;
    loop {
        tokio::select! { biased;
            _ = &mut cancel => break,
            _ = async { let _ = shutdown_rx.wait_for(|v| *v).await; } => break,
            _ = collect.tick() => {
                let _ = ctx.collect_finished().await;
                let _ = ctx.collect_records().await;
            }
        }
    }
}

/// Background voter task — processes incoming consensus signals and emits
/// votes, nacks, and KV commit writes on behalf of this node.
///
/// Spawned by [`GossipAgent::start_consensus_listener`] via [`ConsensusEngine::spawn_listener`].
///
/// `rx_propose` / `rx_commit` are registered **synchronously by the caller**
/// (`start_consensus_listener`) before this task is spawned, so a proposal or
/// commit that arrives in the window between `start_consensus_listener`
/// returning and this task's first poll is queued rather than silently
/// dropped. Registering here instead would reintroduce that race: this node
/// would not vote on (or endorse) signals that arrive before the scheduler
/// first polls the task.
pub(crate) async fn run_consensus_listener(
    ctx:             ConsensusEngine,
    mut cancel:      oneshot::Receiver<()>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut rx_propose:  mpsc::Receiver<Signal>,
    mut rx_commit:   mpsc::Receiver<Signal>,
) {
    // The acceptor's anti-equivocation memory lives on `TaskCtx`, **shared with this node's
    // proposer role** — it used to be task-local here, which let the node accept one value as a
    // voter and self-vote for another as a proposer at the same ballot. See
    // `TaskCtx::consensus_accepted` and `claim_vote`.
    let accepted = Arc::clone(&ctx.task_ctx.consensus_accepted);
    let mut consecutive_abstains: u32 = 0;

    loop {
        tokio::select! { biased;
            _ = &mut cancel                  => break,
            // The `watch::Ref` is dropped inside the block: the arms below await (the acceptor's
            // record is fsynced before it answers), and a `select!` output holding the guard would
            // make this future `!Send`.
            _ = async { let _ = shutdown_rx.wait_for(|v| *v).await; } => break,
            Some(sig) = rx_propose.recv() => {
                // Silent abstain: overloaded node neither votes nor nacks.
                // max_abstain_ballots > 0 caps how many ballots can be skipped in a row.
                if ctx.abstain_when_opaque {
                    let at_cap = ctx.max_abstain_ballots > 0
                        && consecutive_abstains >= ctx.max_abstain_ballots;
                    if !at_cap && ctx.is_overloaded() {
                        consecutive_abstains += 1;
                        continue;
                    }
                }
                consecutive_abstains = 0;

                // `form` is how the request was signed; every answer below is signed the same way.
                // `term`: the request came from an upgraded proposer (row A) — the value is a decision
                // envelope, and every answer is the flagged form.
                // P2: a proposal is answered only for this node's own electorate (`electorate_admits`) — a message
                // without one on an electorate group, or one naming another, is refused by name.
                let (slot, ballot, value, proposer, form, term, gate) = match ctx.decode_verify_form(&sig.payload) {
                    Some((ConsensusMsg::Prepare { slot, ballot, proposer }, form)) => {
                        match ctx.electorate_admits(&sig.scope, &slot, None, &proposer, None) {
                            Ok(gate) => answer_prepare(&ctx, slot, ballot, proposer, form, false, gate).await,
                            Err(own) => refuse_stale(&ctx, slot, proposer, own, form),
                        }
                        continue;
                    }
                    Some((ConsensusMsg::PrepareTerm { slot, ballot, proposer }, form)) => {
                        match ctx.electorate_admits(&sig.scope, &slot, None, &proposer, None) {
                            Ok(gate) => answer_prepare(&ctx, slot, ballot, proposer, form, true, gate).await,
                            Err(own) => refuse_stale(&ctx, slot, proposer, own, form),
                        }
                        continue;
                    }
                    Some((ConsensusMsg::PrepareIn { slot, ballot, proposer, epoch, electorate }, form)) => {
                        match ctx.electorate_admits(&sig.scope, &slot, Some((epoch, electorate)), &proposer, None) {
                            Ok(gate) => answer_prepare(&ctx, slot, ballot, proposer, form, true, gate).await,
                            Err(own) => refuse_stale(&ctx, slot, proposer, own, form),
                        }
                        continue;
                    }
                    Some((ConsensusMsg::CarryPrepare { slot, ballot, proposer, group }, form)) => {
                        answer_carry(&ctx, slot, ballot, proposer, group, form).await;
                        continue;
                    }
                    Some((ConsensusMsg::Propose { slot, ballot, value, proposer }, form)) => {
                        match ctx.electorate_admits(&sig.scope, &slot, None, &proposer, Some((&value, false))) {
                            Ok(gate) => (slot, ballot, value, proposer, form, false, gate),
                            Err(own) => { refuse_stale(&ctx, slot, proposer, own, form); continue; }
                        }
                    }
                    Some((ConsensusMsg::ProposeTerm { slot, ballot, envelope, proposer }, form)) => {
                        // An envelope for another slot is not a proposal for this one.
                        if crate::consensus_life::Envelope::decode(&envelope).is_none_or(|e| *e.slot != *slot) {
                            continue;
                        }
                        match ctx.electorate_admits(&sig.scope, &slot, None, &proposer, Some((&envelope, true))) {
                            Ok(gate) => (slot, ballot, envelope, proposer, form, true, gate),
                            Err(own) => { refuse_stale(&ctx, slot, proposer, own, form); continue; }
                        }
                    }
                    Some((ConsensusMsg::ProposeIn { slot, ballot, envelope, proposer, epoch, electorate }, form)) => {
                        if crate::consensus_life::Envelope::decode(&envelope).is_none_or(|e| *e.slot != *slot) {
                            continue;
                        }
                        match ctx.electorate_admits(&sig.scope, &slot, Some((epoch, electorate)), &proposer, Some((&envelope, true))) {
                            Ok(gate) => (slot, ballot, envelope, proposer, form, true, gate),
                            Err(own) => { refuse_stale(&ctx, slot, proposer, own, form); continue; }
                        }
                    }
                    _ => continue,
                };
                // Claim this node's single vote at this ballot. Refuses a stale ballot OR an
                // equal-ballot proposal for a DIFFERENT value than we already accepted: casting a
                // second vote at one ballot for another value lets two proposers each reach quorum
                // with an overlapping voter and both Commit different values — a single-decree
                // safety violation (audit 2026-07-15 pass 2). The claim goes through the *shared*
                // memory, so this node's proposer role cannot cast a second, conflicting vote.
                // A live commit of a different value refuses the proposal outright: the slot is
                // decided, whatever ballot the proposal carries.
                let proposed_raw = if term { raw_of(&value) } else { Some(value.clone()) };
                let decided_otherwise = ctx.live_digest(&slot).is_some_and(|d| Some(d) != proposed_raw.as_ref().map(value_digest));
                let floor = ctx.floor(&slot);
                // Through the electorate's gate, in the same `compute` as the claim (P2 round 2, finding 1).
                let gated = if decided_otherwise { Some(false) } else {
                    ctx.claim_gated(gate.as_ref(), || if term {
                        claim_envelope(&accepted, &slot, ballot, &value, proposer.id_hash(), floor)
                    } else {
                        claim_vote(&accepted, &slot, ballot, &value, proposer.id_hash(), floor)
                    })
                };
                if gated.is_none() && let Some(g) = &gate {
                    let digest = crate::agent::electorate::cached(&ctx.task_ctx, &g.group).map_or([0; 32], |d| d.digest());
                    refuse_stale(&ctx, slot, proposer, (g.epoch + 1, digest), form);
                    continue;
                }
                let claimed = gated == Some(true);
                if !claimed {
                    // Report what we hold, so the proposer can **adopt** it at a higher ballot
                    // rather than overwrite it. A bare `Nack` says only "I have seen a ballot",
                    // which leaves the proposer free to retry with its own value and replace a
                    // value a quorum already accepted.
                    // `None` after a restart: the durable record carries the digest, not the value,
                    // so this node can refuse a conflicting vote but cannot help the proposer adopt.
                    let state = accepted.pin().get(&slot).cloned().unwrap_or_default();
                    emit_refusal(&ctx, &slot, state.promised.max(floor), state.accepted, proposer, form, term, true);
                } else {
                    // **Durable before the vote leaves.** A vote that reaches a proposer while this
                    // node's acceptance is unrecorded is exactly the memory a restart would lose —
                    // the proposer counts it, the node forgets it, and after a restart the node can
                    // vote again at the same ballot for another value. Same ordering rule as
                    // "apply to the store, then hand the record to the WAL", one layer up — and
                    // fsynced, so "durable" means the disk, not the store. An acceptance that does
                    // not reach the WAL is not voted; the claim stays, and a re-sent proposal for
                    // the same value re-claims it idempotently.
                    if !ctx.persist_acceptor(&slot).await {
                        note_answer_unrecorded(&slot, ballot, "not voting");
                        continue;
                    }
                    // The shared ballot key, at the same rung the proposer's `raise_ballot` gives it.
                    let ballot_upd = ctx.kv_set_returning(
                        format!("{}{}", consensus_ns::BALLOT, &*slot),
                        encode_ballot(ballot),
                    );
                    ctx.wal_try(&ballot_upd);
                    // Always emit VoteWithLocality (carrying None when locality is
                    // unspecified). Topology gates only count voters that arrived
                    // via this variant — Soft policies and gate-less proposals
                    // count every voter regardless.
                    // The **bound** vote: evidence for one value, which is the only form a
                    // current proposer counts. Emitted first so it is the one that races ahead.
                    let bound = ConsensusMsg::VoteForValue {
                        slot:         Arc::clone(&slot),
                        ballot,
                        voter:        ctx.task_ctx.node_id.clone(),
                        value_digest: value_digest(&value),
                        locality:     ctx.self_locality.clone(),
                    };
                    ctx.emit(
                        Arc::from(consensus_kind::VOTE),
                        sig.scope.clone(),
                        ctx.sign_payload_as(encode_consensus_msg(&bound), form),
                    );
                    // And the legacy form, so a proposer predating `VoteForValue` still sees a
                    // vote it understands. It is unbound and therefore unsafe to count — which is
                    // no worse than before this change, and strictly better once the proposer is
                    // upgraded, because an upgraded proposer ignores it.
                    let vote = ConsensusMsg::VoteWithLocality {
                        slot:     Arc::clone(&slot),
                        ballot,
                        voter:    ctx.task_ctx.node_id.clone(),
                        locality: ctx.self_locality.clone(),
                    };
                    ctx.emit(
                        Arc::from(consensus_kind::VOTE),
                        sig.scope,
                        ctx.sign_payload_as(encode_consensus_msg(&vote), form),
                    );
                }
            }
            Some(sig) = rx_commit.recv() => {
                let (slot, ballot, envelope) = match ctx.decode_verify(&sig.payload) {
                    Some(ConsensusMsg::CommitTerm { slot, ballot, envelope }) => (slot, ballot, envelope),
                    // An upgraded learner **ignores the legacy COMMIT** except as tripwire evidence
                    // (design §3, review finding 2): it never writes `committed` or `decided` from it,
                    // so a legacy COMMIT applied first, or a forged one, cannot bring `decided`-driven
                    // liveness back. Decisions by older proposers reach it through their own keys.
                    Some(ConsensusMsg::Commit { slot, ballot, .. }) => {
                        ctx.note_verified_ballot(&slot, ballot);
                        continue;
                    }
                    _ => continue,
                };
                let Some(env) = crate::consensus_life::Envelope::decode(&envelope) else { continue };
                if *env.slot != *slot { continue; }
                // A record ballot far above every ballot this node observed is counted (detection,
                // not prevention — review finding 9); the record is still written.
                if ballot > DECIDED_FLOOR_ANOMALY_MARGIN {
                    ctx.note_implausible(&slot, ballot, "decision record's ballot");
                }
                // A verified decision's ballot is what the ballot tripwire measures forged keys against.
                ctx.note_verified_ballot(&slot, ballot);
                let top = crate::consensus_life::read_slot(
                    &ctx.task_ctx.kv_state, &slot, mycelium_core::sim_seam::wall_now_ms());
                if let crate::consensus_life::SlotView::Top(t) = &top
                    && t.ballot > DECIDED_FLOOR_ANOMALY_MARGIN {
                        ctx.note_implausible(&slot, t.ballot, "decision record's ballot");
                    }

                // ── Commit-conflict tripwire ─────────────────────────────────
                // A COMMIT of a *newer* decision of another identity while this node's top is live is
                // counted: a raced double-commit, a buggy or forged proposer — or a legitimate handoff
                // whose release marker has not reached this node yet, which looks exactly the same. So
                // it is counted and **written** (row A, review D5): the record is the newer decision's
                // at its own ballot key, so writing it endorses nothing over another decision's key and
                // gives it no fresh HLC to win with. A COMMIT at or below the top only writes a lower
                // record, which no reader takes while the top is held (design §2.3); a renewal keeps
                // the identity. Detection only: Layers I and II stay ignorant of Layer III's laws.
                if let crate::consensus_life::SlotView::Top(t) = &top
                    && !t.ended && t.ballot < ballot && t.env.identity() != env.identity() {
                        ctx.task_ctx.commit_conflicts.fetch_add(
                            1, std::sync::atomic::Ordering::Relaxed,
                        );
                        // Legible-Emergence Phase 2: record the "hot slot" — but ONLY when detectors
                        // are enabled (RT4 zero-overhead-when-off; previously this insert ran
                        // unconditionally while only the ring below was gated) and BOUNDED, so an
                        // attacker committing conflicts to unbounded distinct slot names cannot grow
                        // the map without limit (audit 2026-07-15 pass 5). Retry-safe compute.
                        if ctx.task_ctx.config.emergent_detectors_enabled {
                            const MAX_HOT_SLOTS: usize = 4096;
                            let map = ctx.task_ctx.commit_conflict_slots.pin();
                            if map.get(&slot).is_some() || map.len() < MAX_HOT_SLOTS {
                                map.compute(std::sync::Arc::clone(&slot), |existing| {
                                    let n = existing.map(|(_, v)| *v).unwrap_or(0).saturating_add(1);
                                    papaya::Operation::<u64, ()>::Insert(n)
                                });
                            }
                        }
                        // Phase 3 explain: record the event (gated — the ring only records when
                        // the diagnostics feature is on; RT4 zero-overhead-off).
                        if ctx.task_ctx.config.emergent_detectors_enabled {
                            crate::agent::emergent::record_event(
                                &ctx.task_ctx, "commit_conflict",
                                format!("conflicting COMMIT for live slot {slot} at ballot {ballot}"),
                            );
                        }
                        // Counted, and **still written** (review D5): a legitimate handoff whose release
                        // marker lags the new holder's COMMIT looks exactly like this, and the record
                        // is the newer decision's, at a higher ballot — dropping it would leave this
                        // node reading the old holder. Detection, not prevention.
                        tracing::warn!(
                            slot = %slot, ballot,
                            "commit conflict: a COMMIT of another decision above this node's live one; \
                             written and counted (see SystemStats::commit_conflicts)"
                        );
                    }

                // The learner writes the **decision record** — the committer's bytes, so last-writer-
                // wins between copies decides nothing — and nothing else: never `committed`, never an
                // end. A record key this node holds as a tombstone (collected) stays one. The acceptor's
                // memory is kept (erasing it on commit dropped promises, 2026-10-08 review); it is
                // shrunk only once the decision is over (`collect_finished`, row A).
                // **Never overwrite a tombstone**: a record key this node holds as a tombstone was
                // collected below a higher top, and a late COMMIT must not bring it back.
                let key = crate::consensus_life::record_key(&slot, ballot);
                let collected = ctx.task_ctx.kv_state.store.pin().get(key.as_str()).is_some_and(|e| e.data.is_none());
                if collected { continue; }
                let (record, external) = env.stored();
                // Applied, then handed to the WAL (review D7: a learner's copy reached disk only with the
                // next snapshot); collection fsyncs the top before relying on it.
                if ctx.get(&crate::consensus_life::sentinel_key(&slot)).is_none() {
                    let upd = ctx.kv_set_returning(crate::consensus_life::sentinel_key(&slot),
                        Bytes::from_static(&crate::consensus_life::SENTINEL));
                    ctx.wal_try(&upd);
                }
                if let Some(v) = external {
                    let upd = ctx.kv_set_returning(crate::consensus_life::value_key(&slot, &env.value_digest), v);
                    ctx.wal_try(&upd);
                }
                let upd = ctx.kv_set_returning(key, record);
                ctx.wal_try(&upd);
                let _ = ctx.record_decided(&slot, ballot).await;
            }
        }
    }
}

#[cfg(test)]
mod topology_tests {
    use super::*;
    use crate::locality::LocalityPath;

    fn nid(port: u16) -> NodeId {
        NodeId::new("127.0.0.1", port).expect("valid loopback NodeId")
    }

    fn loc(segs: &[&str]) -> LocalityPath {
        LocalityPath::new(segs.iter().copied())
    }

    #[test]
    fn distinct_domains_counts_unique_segments() {
        let mut voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        voters.insert(nid(1), Some(loc(&["eu", "az1"])));
        voters.insert(nid(2), Some(loc(&["eu", "az2"])));
        voters.insert(nid(3), Some(loc(&["eu", "az1"])));
        // depth 0 (region): all "eu" → 1 domain
        assert_eq!(distinct_domains_at_depth(&voters, 0), 1);
        // depth 1 (az): "az1" + "az2" → 2 domains
        assert_eq!(distinct_domains_at_depth(&voters, 1), 2);
        // depth 2 (out of bounds for all): 0 domains
        assert_eq!(distinct_domains_at_depth(&voters, 2), 0);
    }

    #[test]
    fn distinct_domains_ignores_unknown_locality() {
        let mut voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        voters.insert(nid(1), Some(loc(&["eu"])));
        voters.insert(nid(2), None); // legacy Vote, no locality
        voters.insert(nid(3), Some(loc(&["us"])));
        assert_eq!(distinct_domains_at_depth(&voters, 0), 2);
    }

    #[test]
    fn evaluate_gate_soft_always_passes() {
        let voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        let policy = GroupTopologyPolicy {
            prefer_shared_depth: 0,
            spread_depth:        Some(1),
            spread_min_distinct: 3,
            enforcement:         TopologyEnforcement::Soft,
        };
        let (passes, _) = evaluate_topology_gate(&voters, &policy);
        assert!(passes, "Soft enforcement must never block");
    }

    #[test]
    fn evaluate_gate_hard_requires_distinct_domains() {
        let mut voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        voters.insert(nid(1), Some(loc(&["eu", "az1"])));
        voters.insert(nid(2), Some(loc(&["eu", "az1"])));
        let policy = GroupTopologyPolicy {
            prefer_shared_depth: 0,
            spread_depth:        Some(1),
            spread_min_distinct: 2,
            enforcement:         TopologyEnforcement::Hard,
        };
        let (passes, distinct) = evaluate_topology_gate(&voters, &policy);
        assert!(!passes, "two voters in the same AZ must fail spread_min_distinct=2");
        assert_eq!(distinct, 1);

        voters.insert(nid(3), Some(loc(&["eu", "az2"])));
        let (passes, distinct) = evaluate_topology_gate(&voters, &policy);
        assert!(passes, "adding a voter in a different AZ should satisfy the gate");
        assert_eq!(distinct, 2);
    }

    #[test]
    fn evaluate_gate_hard_missing_spread_depth_passes() {
        // Hard with spread_depth = None should never be constructable via
        // GossipConfig::validate, but if it somehow appears at runtime (hot-reload
        // bug), the gate falls through to pass rather than stall production.
        let voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        let policy = GroupTopologyPolicy {
            prefer_shared_depth: 0,
            spread_depth:        None,
            spread_min_distinct: 2,
            enforcement:         TopologyEnforcement::Hard,
        };
        let (passes, _) = evaluate_topology_gate(&voters, &policy);
        assert!(passes);
    }

    #[test]
    fn evaluate_gate_hard_legacy_vote_doesnt_contribute_diversity() {
        // A voter with locality: None (sent via legacy ConsensusMsg::Vote on a
        // mixed-version cluster) cannot contribute to the diversity count.
        let mut voters: AHashMap<NodeId, Option<LocalityPath>> = AHashMap::new();
        voters.insert(nid(1), Some(loc(&["eu"])));
        voters.insert(nid(2), None);
        voters.insert(nid(3), None);
        let policy = GroupTopologyPolicy {
            prefer_shared_depth: 0,
            spread_depth:        Some(0),
            spread_min_distinct: 2,
            enforcement:         TopologyEnforcement::Hard,
        };
        let (passes, distinct) = evaluate_topology_gate(&voters, &policy);
        assert!(!passes);
        assert_eq!(distinct, 1, "only the EU voter contributes diversity; the two legacy votes do not");
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use crate::store::{KvState, StoreEntry};

    fn put(kv: &KvState, key: &str, value: &[u8], ts_ms: u64) {
        kv.store.pin().insert(
            Arc::from(key),
            StoreEntry {
                data:      Some(Bytes::copy_from_slice(value)),
                timestamp: crate::hlc::pack(ts_ms, 0),
            },
        );
    }

    #[test]
    fn no_lease_entry_means_permanent() {
        let kv = KvState::new(1024);
        put(&kv, "consensus/committed/cfg", b"v1", 1_000);
        // Arbitrarily far in the future — still live without a lease entry.
        assert_eq!(
            live_committed_value(&kv, "cfg", u64::MAX / 2).as_deref(),
            Some(b"v1".as_slice()),
        );
    }

    #[test]
    fn lease_fresh_then_expired() {
        let kv = KvState::new(1024);
        put(&kv, "consensus/committed/cfg", b"v1", 1_000_000);
        put(&kv, "consensus/lease/cfg", &5_000u64.to_le_bytes(), 1_000_000);
        // Inside the 5 s window.
        assert!(live_committed_value(&kv, "cfg", 1_004_999).is_some());
        // Exactly at the boundary — still fresh (<=).
        assert!(live_committed_value(&kv, "cfg", 1_005_000).is_some());
        // One ms past the window — the slot has reopened.
        assert!(live_committed_value(&kv, "cfg", 1_005_001).is_none());
    }

    #[test]
    fn regression_causal_now_reads_on_hlc_domain_not_raw_wall() {
        // BUG 8 (audit 2026-07-15). A lease's written timestamp is the WRITER's HLC physical, so a
        // reader must measure elapsed lease time on that same causal-HLC domain — not its raw wall
        // clock, or two skewed nodes disagree on whether the SAME lease is still live and both can
        // briefly hold a `distributed_lock`. `causal_now_ms` folds in any peer HLC this node has
        // observed. Simulate hearing a peer ~1 h ahead and assert the reader's lease clock jumps to
        // the peer's frame rather than staying on this node's (relatively lagging) wall clock.
        use crate::hlc::{Hlc, pack, physical_ms};
        let wall = mycelium_core::sim_seam::wall_now_ms();
        let peer_ahead_ms = 3_600_000; // 1 h ahead of this node's wall clock
        let hlc = Hlc::with_max_drift(86_400_000); // 24 h drift budget: accept the observe unclamped
        hlc.observe(pack(wall + peer_ahead_ms, 0)); // hear a peer from the (near) future
        assert_eq!(
            physical_ms(hlc.current()),
            wall + peer_ahead_ms,
            "sanity: observing the peer pulled the HLC physical to the peer's time",
        );
        let now = causal_now_ms(&hlc);
        assert!(
            now >= wall + peer_ahead_ms,
            "causal_now_ms ({now}) must fold in the observed peer HLC ({}), not read raw wall ({wall})",
            wall + peer_ahead_ms,
        );

        // And it never reads BEHIND wall time — single-node lease timing is unchanged: a fresh HLC
        // that has heard from nobody still reports at least this node's own wall clock.
        assert!(
            causal_now_ms(&Hlc::new()) >= wall,
            "causal_now_ms must never read behind wall time",
        );
    }

    #[test]
    fn malformed_lease_is_treated_as_permanent() {
        // Never silently expire a commitment because a lease entry was corrupted.
        let kv = KvState::new(1024);
        put(&kv, "consensus/committed/cfg", b"v1", 1_000);
        put(&kv, "consensus/lease/cfg", b"xyz", 1_000); // < 8 bytes
        assert!(live_committed_value(&kv, "cfg", u64::MAX / 2).is_some());
    }

    #[test]
    fn tombstoned_lease_is_treated_as_permanent() {
        let kv = KvState::new(1024);
        put(&kv, "consensus/committed/cfg", b"v1", 1_000);
        kv.store.pin().insert(
            Arc::from("consensus/lease/cfg"),
            StoreEntry { data: None, timestamp: crate::hlc::pack(2_000, 0) },
        );
        assert!(live_committed_value(&kv, "cfg", u64::MAX / 2).is_some());
    }

    #[test]
    fn tombstoned_commit_reads_as_absent() {
        // LockGuard release tombstones the committed slot — must read as reopened.
        let kv = KvState::new(1024);
        kv.store.pin().insert(
            Arc::from("consensus/committed/lock/x"),
            StoreEntry { data: None, timestamp: crate::hlc::pack(1_000, 0) },
        );
        assert!(live_committed_value(&kv, "lock/x", 2_000).is_none());
    }

    #[test]
    fn decode_lease_roundtrip_and_malformed() {
        assert_eq!(decode_lease_ms(&encode_lease_ms(86_400_000)), Some(86_400_000));
        assert_eq!(decode_lease_ms(&Bytes::from_static(b"short")), None);
        assert_eq!(decode_lease_ms(&Bytes::new()), None);
    }
}

/// Distinct accepts a group of `n` members needs at fraction `frac` for a **safe
/// (intersecting)** quorum. `floor(n·frac)+1`, floored at strict majority so a small
/// fraction can never yield two disjoint quorums (which would split-brain the slot).
/// At `frac == 0.5` this is exactly `n/2 + 1` — matching the main path's
/// `compute_quorum_size`; the old `ceil(n·frac)` under-counted on even n (`ceil(4·0.5)=2`
/// allowed disjoint `{A,B}`/`{C,D}` — audit 2026-07-15).
pub(crate) fn cross_group_quorum(n: usize, frac: f32) -> usize {
    (((n as f32) * frac).floor() as usize + 1)
        .max(n / 2 + 1)
        .min(n.max(1))
}

#[cfg(test)]
mod cross_group_tests {
    use super::GroupQuorum;

    // Helper: mirrors the quorum check in ConsensusEngine::cross_propose (shared code).
    fn quorum_met(accepts: usize, member_count: usize, frac: f32) -> bool {
        accepts >= super::cross_group_quorum(member_count, frac)
    }

    /// Regression (audit 2026-07-15): a fraction-0.5 quorum on EVEN n must exceed n/2 so any
    /// two quorums intersect — the old `ceil(n·0.5)` allowed disjoint majorities to both commit.
    #[test]
    fn regression_even_n_quorum_intersects() {
        assert!(!quorum_met(2, 4, 0.5), "2 of 4 is a non-intersecting quorum (split-brain)");
        assert!( quorum_met(3, 4, 0.5), "3 of 4 is strict majority");
        assert!(!quorum_met(1, 2, 0.5), "1 of 2 is a minority");
        assert!( quorum_met(2, 2, 0.5));
        // exactly matches the main path (compute_quorum_size) at 0.5, every n:
        for n in 1..=12 { assert_eq!(super::cross_group_quorum(n, 0.5), n / 2 + 1, "n={n}"); }
        // supermajority fractions stay at or above strict majority (still safe):
        assert!(super::cross_group_quorum(6, 0.67) > 6 / 2);
    }

    #[test]
    fn strict_majority_requires_ceil_half_plus_one() {
        // 5-member group at quorum=0.5 → ceil(2.5) = 3 required
        assert!(!quorum_met(2, 5, 0.5), "2/5 must not satisfy majority");
        assert!( quorum_met(3, 5, 0.5), "3/5 must satisfy majority");
        assert!( quorum_met(5, 5, 0.5), "5/5 trivially satisfies");
    }

    #[test]
    fn unanimous_requires_all_members() {
        assert!(!quorum_met(4, 5, 1.0), "4/5 must not satisfy unanimous");
        assert!( quorum_met(5, 5, 1.0), "5/5 must satisfy unanimous");
    }

    #[test]
    fn single_member_group_always_needs_one_accept() {
        // ceil(1 * 0.5) = 1, not 0
        assert!(!quorum_met(0, 1, 0.5), "0 accepts in a 1-member group must fail");
        assert!( quorum_met(1, 1, 0.5));
    }

    #[test]
    fn empty_group_clamps_to_one_required() {
        // member_count=0 → .max(1) ensures needed=1, not 0 (no free commit)
        assert!(!quorum_met(0, 0, 0.5));
    }

    #[test]
    fn all_groups_must_individually_reach_quorum() {
        let group_a_ok = quorum_met(3, 5, 0.5); // ceil(2.5)=3 → passes
        let group_b_ok = quorum_met(2, 5, 0.5); // ceil(2.5)=3 → fails
        assert!( group_a_ok);
        assert!(!group_b_ok);
        assert!(!(group_a_ok && group_b_ok), "overall must fail when any group misses quorum");
    }

    #[test]
    fn group_quorum_struct_fields() {
        let gq = GroupQuorum { group: "compliance".into(), quorum: 0.75, veto: true };
        assert_eq!(gq.group, "compliance");
        assert!((gq.quorum - 0.75).abs() < f32::EPSILON);
        assert!(gq.veto);
    }
}

#[cfg(test)]
mod consensus_msg_auth_tests {
    use super::*;

    fn id(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    /// **A vote for A cannot authorise B** — the single-decree safety property, as a decision over
    /// the exact predicate the collector uses.
    ///
    /// The defect this pins: `Vote`/`VoteWithLocality` name a `(slot, ballot)` and a voter and
    /// nothing else, and the collector matched on `(slot, ballot)` alone before committing **its
    /// own** value. Ballots come from a shared KV key (`read_ballot + 1`), so two concurrent
    /// proposers pick the same ballot **by construction**; votes go to the group scope, so both
    /// proposers receive every vote. One voter's single vote therefore counted toward two
    /// different values at one ballot, and both proposers reached quorum.
    ///
    /// `may_cast_vote` was not enough, and it is worth being precise about why: it stopped the
    /// *voter* accepting two values at a ballot, but the vote it did cast never said **which**
    /// value it accepted, so it could not stop the *counting*. Nor does the NACK the voter sends
    /// the second proposer — a proposer acts on `seen_ballot > ballot`, and that NACK carries an
    /// **equal** ballot, so it is ignored while the broadcast vote is still counted.
    #[test]
    fn a_vote_for_one_value_cannot_authorise_another() {
        let voter = id(7);
        let v_a = Bytes::from_static(b"value-A");
        let v_b = Bytes::from_static(b"value-B");
        let slot: Arc<str> = Arc::from("leader/g");

        let vote_for_a = ConsensusMsg::VoteForValue {
            slot: Arc::clone(&slot), ballot: 1, voter: voter.clone(),
            value_digest: value_digest(&v_a), locality: None,
        };

        // The collector's predicate, stated once: a vote counts for `value` only when its digest
        // is the digest of `value`.
        let counts_for = |msg: &ConsensusMsg, value: &Bytes| -> bool {
            matches!(msg, ConsensusMsg::VoteForValue { value_digest: d, .. } if *d == value_digest(value))
        };

        assert!(counts_for(&vote_for_a, &v_a), "a vote counts for the value it names");
        assert!(
            !counts_for(&vote_for_a, &v_b),
            "a vote for A must NOT contribute to B's quorum, even at the same slot and ballot",
        );

        // The legacy forms carry no value, so they can never satisfy the predicate — which is the
        // mechanism by which an upgraded proposer refuses to count evidence it cannot bind. In a
        // mixed cluster this costs liveness (a timeout) and buys safety; the trade is deliberate.
        let unbound = ConsensusMsg::VoteWithLocality {
            slot: Arc::clone(&slot), ballot: 1, voter: voter.clone(), locality: None,
        };
        assert!(!counts_for(&unbound, &v_a), "an unbound vote is not evidence for any value");
        let ancient = ConsensusMsg::Vote { slot, ballot: 1, voter };
        assert!(!counts_for(&ancient, &v_a), "nor is the oldest form");
    }

    /// **A node casts at most one vote per ballot, whichever role it is playing.**
    ///
    /// `claim_vote` is the single gate both roles pass through. Before 2026-09-24 they had
    /// *separate* memories: the listener's `seen_ballot`/`voted_value` were task-local, and the
    /// proposer's ballot loop self-voted **unconditionally** without consulting them. A node could
    /// therefore accept `v_X` at ballot 1 as a voter and propose-and-self-vote `v_Y` at ballot 1 as
    /// a proposer — equivocating with itself through the one voter that never has to send a message
    /// to vote, and defeating `VoteForValue`'s binding by the route binding does not cover.
    #[test]
    fn one_node_casts_one_vote_per_ballot_in_either_role() {
        let accepted = AcceptorMemory::new();
        let slot: Arc<str> = Arc::from("leader/g");
        let v_x = Bytes::from_static(b"value-X");
        let v_y = Bytes::from_static(b"value-Y");
        const P: u64 = 1;

        // Acting as an acceptor: accept v_X at ballot 1.
        assert!(claim_vote(&accepted, &slot, 1, &v_x, P, 0), "first claim at a ballot is granted");

        // Acting as a proposer, same node, same ballot, different value — refused. This is the
        // self-vote that used to be unconditional.
        assert!(
            !claim_vote(&accepted, &slot, 1, &v_y, P, 0),
            "a node must not vote for a second value at a ballot it has already voted in",
        );

        // Re-claiming the SAME value at the same ballot is fine — a retransmitted proposal must not
        // look like equivocation.
        assert!(claim_vote(&accepted, &slot, 1, &v_x, P, 0), "idempotent for the same value");

        // A higher ballot is a fresh decision, and may carry a different value.
        assert!(claim_vote(&accepted, &slot, 2, &v_y, P, 0), "a higher ballot may choose anew");
        // …and the memory moved with it: ballot 1 is now stale.
        assert!(!claim_vote(&accepted, &slot, 1, &v_x, P, 0), "a stale ballot cannot be voted in again");
    }

    /// **A proposer at a higher ballot adopts what was already accepted.**
    ///
    /// The adoption rule, as the decision the retry path makes. Before 2026-09-24 a refusal carried
    /// only `Nack { seen_ballot }` — a number — and the retry was `ballot = …max(ballot) + 1` while
    /// **still proposing the proposer's own value**, which never changed across attempts. Acceptors
    /// permit that, because `may_cast_vote` returns `true` for any strictly greater ballot. So a
    /// value a quorum had already accepted at ballot *N* could be replaced at *N+1*.
    ///
    /// The commit record guards the *committed* case, but only once it has propagated; inside that
    /// window the overwrite stands. Preserving the accepted value is what makes the guard
    /// unnecessary rather than merely usually-sufficient.
    #[test]
    fn a_higher_ballot_adopts_the_accepted_value() {
        let mine = Bytes::from_static(b"mine");
        let theirs = Bytes::from_static(b"already-accepted");
        let older = Bytes::from_static(b"older");

        // The rule the retry path applies: adopt when the report's ballot is at least as high as
        // whatever we have already adopted (0 = still carrying our own value).
        let adopt = |adopted_from: u64, reported: Option<(u64, Bytes)>, current: Bytes|
            -> (u64, Bytes) {
            match reported {
                Some((ab, v)) if ab >= adopted_from => (ab, v),
                _ => (adopted_from, current),
            }
        };

        // Nothing reported → keep proposing our own value.
        assert_eq!(adopt(0, None, mine.clone()), (0, mine.clone()));

        // An acceptor reports a value accepted at ballot 3 → we carry it, not ours.
        assert_eq!(
            adopt(0, Some((3, theirs.clone())), mine.clone()),
            (3, theirs.clone()),
            "a value an acceptor already holds outranks the proposer's own",
        );

        // A *lower*-balloted report does not displace a higher one we already adopted.
        assert_eq!(
            adopt(3, Some((1, older)), theirs.clone()),
            (3, theirs.clone()),
            "adoption follows the highest accepted ballot, not the latest message",
        );

        // An equal-balloted report is accepted (idempotent — same decision, restated).
        assert_eq!(adopt(3, Some((3, theirs.clone())), theirs.clone()), (3, theirs));
    }

    /// **The acceptor's memory survives a restart.**
    ///
    /// Without the durable record, *at most one value per ballot* held only for a **process
    /// lifetime**: a node that restarted mid-ballot forgot what it accepted and could vote again,
    /// for a different value, at the same ballot. Every acceptor-side guarantee in classical
    /// consensus depends on that memory being durable; ours was not.
    ///
    /// The round trip below is what a restart actually does — write the record, lose the in-memory
    /// map, recover from the record, and find the conflicting vote still refused.
    #[test]
    fn acceptor_memory_survives_a_restart() {
        let node = id(11);
        let slot: Arc<str> = Arc::from("leader/g");
        let v_x = Bytes::from_static(b"value-X");
        let v_y = Bytes::from_static(b"value-Y");

        const P: u64 = 1;

        // Before the restart: accept v_X at ballot 4 and write the durable record.
        let live = AcceptorMemory::new();
        assert!(claim_vote(&live, &slot, 4, &v_x, P, 0));
        let record = encode_acceptor(&live.pin().get(&slot).cloned().expect("claimed"));
        assert_eq!(accepted_key(&node, &slot), format!("sys/consensus-accepted/{node}/leader/g"));

        // The restart: the map is gone.
        let recovered = AcceptorMemory::new();
        let state = decode_acceptor(&record).expect("a well-formed record");
        recovered.pin().insert(Arc::clone(&slot), state);

        // After the restart: the conflicting vote is still refused…
        assert!(
            !claim_vote(&recovered, &slot, 4, &v_y, P, 0),
            "a restarted node must not vote for a second value at a ballot it already voted in",
        );
        // …the same value is still idempotent…
        assert!(claim_vote(&recovered, &slot, 4, &v_x, P, 0), "equal digests, so the same vote stands");
        // …and a higher ballot is still a fresh decision.
        assert!(claim_vote(&recovered, &slot, 5, &v_y, P, 0));
    }

    /// A recovered record refuses, but cannot help a proposer adopt — safety survives the restart,
    /// that liveness aid does not, and the distinction is deliberate: the record holds a digest so
    /// it is small enough to write on the voting path.
    #[test]
    fn a_recovered_acceptance_reports_no_value() {
        let full = Accepted::Full(Bytes::from_static(b"v"));
        let recovered = Accepted::DigestOnly(value_digest(&Bytes::from_static(b"v")));
        assert_eq!(full.digest(), recovered.digest(), "both answer the equality question");
        assert!(full.value().is_some(), "a live acceptance can be reported in a Promise");
        assert!(recovered.value().is_none(), "a recovered one cannot, and must not invent it");
    }

    /// A malformed durable record is **no record**, never a partial one: a half-understood memory
    /// would refuse votes it cannot justify, which is worse than an absent one.
    #[test]
    fn a_malformed_acceptor_record_is_no_record() {
        let good = encode_accepted(7, [9u8; 32]);
        assert_eq!(decode_accepted(&good), Some((7, [9u8; 32])));
        assert!(decode_accepted(&good[..39]).is_none(), "truncated");
        assert!(decode_accepted(&[]).is_none(), "empty");
        let mut long = good.to_vec();
        long.push(0);
        assert!(decode_accepted(&long).is_none(), "over-long is not silently truncated");
    }

    /// The digest is over the value **bytes**, so two proposals that differ at all are
    /// distinguishable — including the empty value, which is a legitimate proposal.
    #[test]
    fn the_value_digest_separates_distinct_proposals() {
        let a = Bytes::from_static(b"x");
        let b = Bytes::from_static(b"y");
        let empty = Bytes::new();
        assert_ne!(value_digest(&a), value_digest(&b));
        assert_ne!(value_digest(&a), value_digest(&empty));
        assert_eq!(value_digest(&a), value_digest(&Bytes::from_static(b"x")),
                   "equal bytes, equal digest — a re-sent vote for the same value still counts");
    }

    /// **A bound vote is still an impersonation risk if the signer is not checked**, so the two
    /// defences compose rather than replace each other.
    #[test]
    fn a_bound_vote_still_requires_its_signer() {
        let a = id(1);
        let b = id(2);
        let msg = ConsensusMsg::VoteForValue {
            slot: Arc::from("s"), ballot: 1, voter: b.clone(),
            value_digest: value_digest(&Bytes::from_static(b"v")), locality: None,
        };
        assert!(signer_authorized(&msg, &b), "signed by its own voter");
        assert!(!signer_authorized(&msg, &a), "one key must not sign another node's bound vote");
    }

    // ── F1: signer must match the vote/propose identity (audit 2026-07-15 pass 2) ──
    #[test]
    fn regression_signer_must_match_vote_and_propose_identity() {
        let a = id(1);
        let b = id(2);
        // A vote naming `b` as voter is authorised ONLY when signed by `b` — an impersonated vote
        // (signed by `a`, claiming voter `b`) is rejected, which is what killed the forged-quorum
        // vector (one key signing N distinct voters).
        let vote = ConsensusMsg::Vote { slot: "s".into(), ballot: 1, voter: b.clone() };
        assert!(signer_authorized(&vote, &b), "a node may vote as itself");
        assert!(!signer_authorized(&vote, &a), "a node may NOT vote as another identity");

        let vwl = ConsensusMsg::VoteWithLocality { slot: "s".into(), ballot: 1, voter: b.clone(), locality: None };
        assert!(signer_authorized(&vwl, &b));
        assert!(!signer_authorized(&vwl, &a), "VoteWithLocality impersonation must be rejected too");

        let prop = ConsensusMsg::Propose { slot: "s".into(), ballot: 1, value: Bytes::from_static(b"v"), proposer: b.clone() };
        assert!(signer_authorized(&prop, &b));
        assert!(!signer_authorized(&prop, &a), "a node may not propose as another identity");

        // Commit/Nack carry no per-node authority field (not bound here — CFT, not BFT).
        let commit = ConsensusMsg::Commit { slot: "s".into(), ballot: 1, value: Bytes::from_static(b"v") };
        assert!(signer_authorized(&commit, &a));
        assert!(signer_authorized(&ConsensusMsg::Nack { slot: "s".into(), seen_ballot: 1 }, &a));
    }

    /// **Two proposers cannot choose two values for one slot** — the stable-roster case, no
    /// membership change, no restart (fail-first model of the 2026-10-08 finding).
    ///
    /// Five acceptors, quorum three. A chooses `v1` at ballot 1 with `{A, 1, 2}`. B has not seen
    /// the COMMIT, proposes `v2` at ballot 2 to `{B, 2, 3}`: its quorum shares acceptor 2 with A's.
    /// Before the prepare phase, nothing made B learn what acceptor 2 held — a strictly higher
    /// ballot was granted without reporting the accepted value — so B chose `v2` too. Seen failing
    /// on the unfixed code with the proposer of the day modelled as `claim_vote` alone (B chose
    /// `v2`); the round below is the proposer as it is now: `prepare_slot`, then
    /// `choose_after_prepare`, then `claim_vote`.
    #[test]
    fn two_proposers_cannot_choose_two_values_for_one_slot() {
        let slot: Arc<str> = Arc::from("leader/g");
        let v1 = Bytes::from_static(b"v1");
        let v2 = Bytes::from_static(b"v2");
        let acceptors: Vec<AcceptorMemory> = (0..5).map(|_| AcceptorMemory::new()).collect();
        let round = |proposer: u64, members: &[usize], ballot: u64, value: &Bytes| -> Option<Bytes> {
            let mut reports = Vec::new();
            for &i in members {
                match prepare_slot(&acceptors[i], &slot, ballot, proposer, 0) {
                    PrepareOutcome::Promised(acc) => if let Some((b, a)) = acc {
                        reports.push((b, a.digest(), a.value()));
                    },
                    PrepareOutcome::Refused { .. } => return None,
                }
            }
            let value = match choose_after_prepare(value, &reports) {
                Phase1Choice::Keep => value.clone(),
                Phase1Choice::Adopt(_, v) => v,
                Phase1Choice::Blocked => return None,
            };
            let granted = members.iter()
                .filter(|&&i| claim_vote(&acceptors[i], &slot, ballot, &value, proposer, 0))
                .count();
            (granted >= 3).then_some(value)
        };
        let a = round(100, &[0, 1, 2], 1, &v1);
        let b = round(200, &[4, 2, 3], 2, &v2);
        assert_eq!(a.as_ref(), Some(&v1), "A's quorum chose v1");
        assert_eq!(b.as_ref(), Some(&v1), "B's intersecting quorum must choose v1, not v2");
    }

    /// **A promise binds the acceptor.** After promising ballot 5 to P it refuses any lower
    /// ballot, refuses Q at ballot 5 — the same ballot drawn from the shared key — and still lets
    /// P through, both for a re-sent prepare and for P's accept.
    #[test]
    fn a_promise_refuses_lower_ballots_and_other_proposers() {
        let m = AcceptorMemory::new();
        let slot: Arc<str> = Arc::from("s");
        let v = Bytes::from_static(b"v");
        const P: u64 = 1;
        const Q: u64 = 2;
        assert!(matches!(prepare_slot(&m, &slot, 5, P, 0), PrepareOutcome::Promised(None)));
        assert!(matches!(prepare_slot(&m, &slot, 5, P, 0), PrepareOutcome::Promised(None)),
                "a re-sent prepare from the same proposer is granted again");
        assert!(matches!(prepare_slot(&m, &slot, 5, Q, 0), PrepareOutcome::Refused { promised: 5, .. }),
                "a second proposer at the promised ballot is refused");
        assert!(matches!(prepare_slot(&m, &slot, 4, Q, 0), PrepareOutcome::Refused { promised: 5, .. }));
        assert!(!claim_vote(&m, &slot, 4, &v, P, 0), "an accept below the promise is refused");
        assert!(!claim_vote(&m, &slot, 5, &v, Q, 0), "another proposer's accept at the promised ballot is refused");
        assert!(claim_vote(&m, &slot, 5, &v, P, 0), "the promised proposer's accept is granted");
        assert!(matches!(prepare_slot(&m, &slot, 6, Q, 0), PrepareOutcome::Promised(Some((5, _)))),
                "a higher prepare is granted and reports the acceptance");
        assert!(!claim_vote(&m, &slot, 5, &v, P, 0), "and P's ballot is now below the promise");
    }

    /// **Two proposers on one ballot: only one can assemble a quorum.** Three acceptors, quorum
    /// two; P and Q both draw ballot 1, and their prepares arrive in different orders. Whoever an
    /// acceptor promised first keeps it, so at most one proposer holds two promises — and the other
    /// cannot get its accept through either.
    #[test]
    fn one_ballot_goes_to_one_proposer() {
        let slot: Arc<str> = Arc::from("s");
        let acc: Vec<AcceptorMemory> = (0..3).map(|_| AcceptorMemory::new()).collect();
        const P: u64 = 1;
        const Q: u64 = 2;
        let promised = |i: usize, who: u64| matches!(prepare_slot(&acc[i], &slot, 1, who, 0), PrepareOutcome::Promised(_));
        // Arrival orders: acceptor 0 sees P first, 1 sees Q first, 2 sees P first.
        let (p0, q0) = (promised(0, P), promised(0, Q));
        let (q1, p1) = (promised(1, Q), promised(1, P));
        let (p2, q2) = (promised(2, P), promised(2, Q));
        assert_eq!([p0, p1, p2].iter().filter(|x| **x).count(), 2, "P holds a quorum");
        assert_eq!([q0, q1, q2].iter().filter(|x| **x).count(), 1, "Q holds only acceptor 1");
        let vq = Bytes::from_static(b"q");
        let q_accepts = (0..3).filter(|&i| claim_vote(&acc[i], &slot, 1, &vq, Q, 0)).count();
        assert!(q_accepts < 2, "Q cannot get a quorum to accept at P's ballot");
    }

    /// The durable record carries the promise as well as the acceptance, so a restart cannot
    /// break a promise; and a pre-2.30.0 acceptance-only record still reads, as a promise at the
    /// accepted ballot to no named proposer.
    #[test]
    fn a_promise_survives_a_restart() {
        let slot: Arc<str> = Arc::from("s");
        let live = AcceptorMemory::new();
        const P: u64 = 1;
        const Q: u64 = 2;
        assert!(matches!(prepare_slot(&live, &slot, 7, P, 0), PrepareOutcome::Promised(None)));
        let record = encode_acceptor(&live.pin().get(&slot).cloned().unwrap());
        let recovered = AcceptorMemory::new();
        recovered.pin().insert(Arc::clone(&slot), decode_acceptor(&record).expect("record"));
        let v = Bytes::from_static(b"v");
        assert!(!claim_vote(&recovered, &slot, 6, &v, Q, 0), "a lower ballot is still refused after a restart");
        assert!(!claim_vote(&recovered, &slot, 7, &v, Q, 0), "so is another proposer at the promised ballot");
        assert!(claim_vote(&recovered, &slot, 7, &v, P, 0));

        let legacy = decode_acceptor(&encode_accepted(4, value_digest(&v))).expect("legacy record");
        assert_eq!((legacy.promised, legacy.promised_to), (4, None));
        let m = AcceptorMemory::new();
        m.pin().insert(Arc::clone(&slot), legacy);
        assert!(!claim_vote(&m, &slot, 4, &Bytes::from_static(b"other"), Q, 0), "a different value at the old ballot is refused");
        assert!(claim_vote(&m, &slot, 4, &v, Q, 0), "the same value is not");
    }

    /// The current record's shape is checked, never half-read: a wrong tag, a flag that is not
    /// 0 or 1, an acceptance above the promise, or a wrong length is no record.
    #[test]
    fn a_malformed_promise_record_is_no_record() {
        let st = AcceptorSlot {
            promised: 9, promised_to: Some(3),
            accepted: Some((8, Accepted::DigestOnly(value_digest(&Bytes::from_static(b"v"))))),
        };
        let good = encode_acceptor(&st).to_vec();
        let back = decode_acceptor(&good).expect("round trip");
        assert_eq!((back.promised, back.promised_to), (9, Some(3)));
        assert_eq!(back.accepted.map(|(b, a)| (b, a.digest())), Some((8, value_digest(&Bytes::from_static(b"v")))));
        let mut bad_tag = good.clone(); bad_tag[0] = 0x03;
        assert!(decode_acceptor(&bad_tag).is_none());
        let mut bad_flag = good.clone(); bad_flag[9] = 2;
        assert!(decode_acceptor(&bad_flag).is_none());
        let mut above = good.clone(); above[19..27].copy_from_slice(&10u64.to_le_bytes());
        assert!(decode_acceptor(&above).is_none(), "an acceptance above the promise is incoherent");
        assert!(decode_acceptor(&good[..good.len() - 1]).is_none());
    }

    /// **The phase-1 rule**: the highest acceptance among the promisers decides; a value whose
    /// bytes do not match its digest is ignored; an acceptance known only by digest blocks a
    /// different value but not the same one.
    #[test]
    fn the_phase1_rule_carries_the_highest_acceptance() {
        let mine = Bytes::from_static(b"mine");
        let a = Bytes::from_static(b"a");
        let b = Bytes::from_static(b"b");
        assert_eq!(choose_after_prepare(&mine, &[]), Phase1Choice::Keep);
        assert_eq!(
            choose_after_prepare(&mine, &[(2, value_digest(&a), Some(a.clone())), (5, value_digest(&b), Some(b.clone()))]),
            Phase1Choice::Adopt(5, b.clone()),
        );
        assert_eq!(choose_after_prepare(&mine, &[(5, value_digest(&b), None)]), Phase1Choice::Blocked,
                   "a value known only by digest must not be overwritten");
        assert_eq!(choose_after_prepare(&b, &[(5, value_digest(&b), None)]), Phase1Choice::Keep,
                   "but the proposer may carry it when it is its own");
        assert_eq!(choose_after_prepare(&mine, &[(5, value_digest(&b), None), (5, value_digest(&b), Some(b.clone()))]),
                   Phase1Choice::Adopt(5, b.clone()), "one promiser with the bytes is enough");
        assert_eq!(choose_after_prepare(&mine, &[(5, value_digest(&b), Some(a))]), Phase1Choice::Blocked,
                   "bytes that do not match their digest are not adopted");
    }

    /// The record carries the value when it fits, so a restarted acceptor can still hand it to a
    /// proposer — and bytes that do not match the digest are no record, not a guess.
    #[test]
    fn the_promise_record_carries_the_value() {
        let v = Bytes::from_static(b"the-value");
        let st = AcceptorSlot { promised: 3, promised_to: Some(1), accepted: Some((3, Accepted::Full(v.clone()))) };
        let rec = encode_acceptor(&st);
        let back = decode_acceptor(&rec).expect("round trip");
        assert_eq!(back.accepted.and_then(|(_, a)| a.value()), Some(v), "a restart keeps the bytes");
        let mut bad = rec.to_vec();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(decode_acceptor(&bad).is_none(), "bytes that contradict the digest are no record");
        let none = AcceptorSlot { promised: 3, promised_to: Some(1), accepted: None };
        let mut trailing = encode_acceptor(&none).to_vec();
        trailing.push(7);
        assert!(decode_acceptor(&trailing).is_none(), "a value with no acceptance is incoherent");
    }

    /// **A delayed lower-ballot proposal cannot commit a second value** (the 2026-10-08 review's
    /// HIGH-1). D promised ballot 1 by `{4,0,1}` and sent `Propose(1, v0)`, which is delayed; A then
    /// chose `v1` at ballot 2 with `{3,1,2}` and committed. The listener used to **erase** an
    /// acceptor's memory on COMMIT, promises included, so the delayed proposal found `{0,1,2}`
    /// forgetful and assembled a quorum — seen failing on the erasing code (D held four votes for
    /// `v0`). Now nothing is erased: 1 and 2 still hold their promise at 2, and once an acceptor
    /// knows the slot was decided at 2, ballot 1 is below its floor.
    #[test]
    fn a_delayed_lower_proposal_cannot_commit_a_second_value() {
        let slot: Arc<str> = Arc::from("s");
        let acc: Vec<AcceptorMemory> = (0..5).map(|_| AcceptorMemory::new()).collect();
        let (v0, v1) = (Bytes::from_static(b"v0"), Bytes::from_static(b"v1"));
        const D: u64 = 4;
        const A: u64 = 3;
        for i in [4usize, 0, 1] { assert!(matches!(prepare_slot(&acc[i], &slot, 1, D, 0), PrepareOutcome::Promised(None))); }
        assert!(claim_vote(&acc[4], &slot, 1, &v0, D, 0));
        for i in [3usize, 1, 2] { assert!(matches!(prepare_slot(&acc[i], &slot, 2, A, 0), PrepareOutcome::Promised(None))); }
        assert_eq!([3usize, 1, 2].iter().filter(|&&i| claim_vote(&acc[i], &slot, 2, &v1, A, 0)).count(), 3);
        // The COMMIT reaches 0, 1, 2 — which now record the floor and keep their memory.
        let unaware = [0usize, 1, 2].iter().filter(|&&i| claim_vote(&acc[i], &slot, 1, &v0, D, 0)).count();
        assert!(1 + unaware < 3, "even without the floor, kept promises refuse it ({} votes)", 1 + unaware);
        let aware = [0usize, 1, 2].iter().filter(|&&i| claim_vote(&acc[i], &slot, 1, &v0, D, 2)).count();
        assert_eq!(aware, 0, "below the decided ballot, nothing is accepted");
    }

    /// The decided ballot is a floor for both phases: a prepare or accept at or below it is
    /// refused, and the refusal names at least the floor so the proposer moves above it.
    #[test]
    fn the_decided_ballot_is_a_floor() {
        let m = AcceptorMemory::new();
        let slot: Arc<str> = Arc::from("s");
        let v = Bytes::from_static(b"v");
        assert!(matches!(prepare_slot(&m, &slot, 4, 1, 4), PrepareOutcome::Refused { promised: 4, .. }));
        assert!(!claim_vote(&m, &slot, 3, &v, 1, 4));
        assert!(matches!(prepare_slot(&m, &slot, 5, 1, 4), PrepareOutcome::Promised(None)));
        assert!(claim_vote(&m, &slot, 5, &v, 1, 4));
    }

    /// The floor tripwire's bound: past the margin above what was observed, and only there.
    #[test]
    fn an_implausible_floor_is_one_past_the_margin() {
        assert!(!decided_floor_is_implausible(5, 0), "a node that missed a few attempts");
        assert!(!decided_floor_is_implausible(DECIDED_FLOOR_ANOMALY_MARGIN, 0));
        assert!(decided_floor_is_implausible(DECIDED_FLOOR_ANOMALY_MARGIN + 1, 0));
        assert!(decided_floor_is_implausible(u64::MAX, 7), "the forgery that exhausts the ballot space");
        assert!(!decided_floor_is_implausible(u64::MAX, u64::MAX - 1), "observed ballots reached it");
        assert!(!decided_floor_is_implausible(3, 10), "a floor below what was observed");
    }

    /// Two digests at the top ballot means *one value per ballot* did not hold (a proposer older
    /// than 2.30.0): adopting either could overwrite the chosen one, so the rule refuses.
    #[test]
    fn two_values_at_one_ballot_block_the_proposer() {
        let (a, b) = (Bytes::from_static(b"a"), Bytes::from_static(b"b"));
        assert_eq!(
            choose_after_prepare(&Bytes::from_static(b"mine"), &[
                (4, value_digest(&a), Some(a.clone())), (4, value_digest(&b), Some(b.clone())),
            ]),
            Phase1Choice::Blocked,
        );
    }

    /// **A floor learned before its commit must not hide the commit** (second review, M1). B has
    /// `decided = 5` but not yet the committed key; acceptor C reports `(5, v1)`. Setting it aside
    /// would let B keep its own value and commit a second one; B must adopt `v1` instead. Only
    /// when the decision is visibly over — expired or released — is the old acceptance set aside.
    #[test]
    fn a_floor_without_its_commit_sets_nothing_aside() {
        let v1 = Bytes::from_static(b"v1");
        let mine = Bytes::from_static(b"mine");
        let mut reports = vec![(5, value_digest(&v1), Some(v1.clone()))];
        set_aside_finished(&mut reports, 5, false, 0);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Adopt(5, v1.clone()),
                   "a decision not seen to be over still binds the proposer");
        set_aside_finished(&mut reports, 5, true, 0);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Keep,
                   "a decision seen to be over does not");
    }

    /// **D1 and the review of `ee3d9e4b`, finding 2: the renewal rule, both directions.** A renewal keeps
    /// its own envelope only when the highest report is its own lineage; with its lineage reported
    /// **below** a foreign acceptance it adopts the foreign one.
    #[test]
    fn a_renewal_keeps_its_lineage_only_when_it_is_the_top_report() {
        use crate::consensus_life::{Envelope, Term};
        let me = NodeId::new("127.0.0.1", 9).unwrap();
        let other = NodeId::new("127.0.0.1", 8).unwrap();
        let own = Envelope::new("s", Bytes::from_static(b"L"), Term::Lease { ms: 1, expires_at_ms: 9 }, 4, me.clone(), 1).encode();
        let renewal = Envelope::new("s", Bytes::from_static(b"L"), Term::Lease { ms: 1, expires_at_ms: 99 }, 4, me.clone(), 2);
        let w = Envelope::new("s", Bytes::from_static(b"W"), Term::Lease { ms: 1, expires_at_ms: 9 }, 6, other, 1).encode();
        let carry = Carry { own: renewal.clone(), renewing: Some(4), legacy_term: Term::Lease { ms: 1, expires_at_ms: 9 } };
        let current = renewal.encode();
        let lower_own = vec![(4, value_digest(&own), Some(own.clone())), (6, value_digest(&w), Some(w.clone()))];
        assert_eq!(phase1_choice(&current, &lower_own, &carry), Phase1Choice::Adopt(6, w.clone()),
            "a renewal whose lineage is below a foreign acceptance must adopt it");
        let top_own = vec![(6, value_digest(&w), Some(w)), (8, value_digest(&own), Some(own))];
        assert_eq!(phase1_choice(&current, &top_own, &carry), Phase1Choice::Keep,
            "a renewal whose lineage is the top report keeps its own envelope");
    }

    /// Row A, K1/K2: an ended decision's acceptances are set aside by the ballot its lifecycle
    /// record carries, with neither the floor nor the committed entry in view.
    #[test]
    fn an_ended_decision_sets_its_acceptances_aside_without_floor_or_entry() {
        let v1 = Bytes::from_static(b"v1");
        let v2 = Bytes::from_static(b"v2");
        let mine = Bytes::from_static(b"mine");
        let mut reports = vec![(5, value_digest(&v1), Some(v1.clone())), (7, value_digest(&v2), Some(v2.clone()))];
        set_aside_finished(&mut reports, 0, false, 5);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Adopt(7, v2),
                   "a later acceptance still binds; the ended one does not");
        let mut reports = vec![(5, value_digest(&v1), Some(v1))];
        set_aside_finished(&mut reports, 0, false, 5);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Keep);
    }

    /// A commit of an adopted value is the slot's decision, not the caller's (second review, M3).
    #[test]
    fn a_commit_of_an_adopted_value_reports_superseded() {
        let mine = Bytes::from_static(b"mine");
        let theirs = Bytes::from_static(b"theirs");
        let committed = |v: &Bytes| ConsensusResult::Committed {
            slot: Arc::from("s"), value: v.clone(), ballot: 4, persisted: true,
        };
        assert!(matches!(own_or_superseded(committed(&theirs), &mine), ConsensusResult::Superseded { ballot: 4, .. }));
        assert!(matches!(own_or_superseded(committed(&mine), &mine), ConsensusResult::Committed { .. }));
    }

    /// **A promise shortfall is not counted as a partition** (doc-coverage run 22, code gap 3). A
    /// proposer whose prepare phase never gathers a quorum — mid-upgrade, acceptors older than 2.30.0
    /// ignore `Prepare` — retries before any vote is asked for, so it used to time out as `no_voters`,
    /// which the runbook reads as a partition.
    #[test]
    fn a_promise_shortfall_has_its_own_timeout_reason() {
        // Nobody else answered the prepare: a partition (or acceptors that ignore `Prepare`).
        assert_eq!(timeout_reason(LastAttempt::Prepare { others: 0 }, 0), "no_voters");
        // Some promised, too few.
        assert_eq!(timeout_reason(LastAttempt::Prepare { others: 1 }, 0), "promise_short");
        assert_eq!(timeout_reason(LastAttempt::Contended, 0), "contended");
        assert_eq!(timeout_reason(LastAttempt::Blocked, 0), "blocked");
        assert_eq!(timeout_reason(LastAttempt::Vote, 0), "no_voters");
        assert_eq!(timeout_reason(LastAttempt::Vote, 2), "quorum_short");
    }

    /// **A cross-group retry counts its voters afresh** (the review of #579). The per-group tally
    /// cleared its count each attempt but kept the set of voters seen, so a voter from ballot 1 never
    /// counted again — a retry whose voters had all voted before could not reach quorum.
    #[test]
    fn a_cross_group_retry_counts_its_voters_afresh() {
        let v = NodeId::new("127.0.0.1", 9001).unwrap();
        let mut gs = CrossState {
            members: [v.clone()].into_iter().collect(), quorum_frac: 1.0, accepts: 0, seen: Default::default(),
        };
        gs.count(&v);
        gs.count(&v);
        assert_eq!(gs.accepts, 1, "a re-delivered vote counts once");
        gs.begin_attempt();
        gs.count(&v);
        assert_eq!(gs.accepts, 1, "the next ballot's vote from the same voter counts");
    }

    #[test]
    fn prepare_messages_are_bound_to_their_signer() {
        let a = id(1);
        let b = id(2);
        let prep = ConsensusMsg::Prepare { slot: "s".into(), ballot: 1, proposer: b.clone() };
        assert!(signer_authorized(&prep, &b));
        assert!(!signer_authorized(&prep, &a), "a node may not prepare as another identity");
        let ack = ConsensusMsg::PrepareAck {
            slot: "s".into(), ballot: 1, voter: b.clone(), accepted_ballot: 0,
            accepted_digest: None, accepted_value: None, committed_digest: None,
        };
        assert!(signer_authorized(&ack, &b));
        assert!(!signer_authorized(&ack, &a), "one key must not promise for another node");
    }

    // ── F2: acceptor must not equivocate at the same ballot (audit 2026-07-15 pass 2) ──
    #[test]
    fn regression_voter_never_accepts_two_values_at_one_ballot() {
        // Digests, since 2026-09-24: the rule is the same, but it is now asked of a digest so a
        // record recovered after a restart — which holds no value — answers it identically.
        let a = value_digest(&Bytes::from_static(b"A"));
        let b = value_digest(&Bytes::from_static(b"B"));
        // Never voted this slot (prior None): any ballot >= 0 is votable.
        assert!(may_cast_vote_digest(0, None, 1, a));
        // Voted A at ballot 1. A second ballot-1 proposal for a DIFFERENT value B must be refused.
        assert!(!may_cast_vote_digest(1, Some(a), 1, b), "equivocation at the same ballot must be refused");
        // Re-voting for the SAME value at the same ballot is idempotent and allowed.
        assert!(may_cast_vote_digest(1, Some(a), 1, a));
        // A strictly higher ballot supersedes — vote for whatever value it carries.
        assert!(may_cast_vote_digest(1, Some(a), 2, b));
        // A stale (lower) ballot is never votable.
        assert!(!may_cast_vote_digest(2, Some(a), 1, a));
    }
}

#[cfg(test)]
mod electorate_filter_tests {
    use super::*;

    /// P2's tension (b): an electorate group's roster is the vote filter, and a declared trust slice
    /// only narrows it — a voter outside the roster never counts, whatever a slice says.
    #[test]
    fn the_roster_is_the_vote_filter_and_a_trust_slice_only_narrows_it() {
        let set = |v: &[u64]| v.iter().copied().collect::<AHashSet<u64>>();
        assert_eq!(electorate_vote_filter(None, None), None, "no electorate, no slice: no filter");
        assert_eq!(electorate_vote_filter(Some(set(&[1, 9])), None), Some(set(&[1, 9])), "a slice alone, as before");
        assert_eq!(electorate_vote_filter(None, Some(set(&[1, 2, 3]))), Some(set(&[1, 2, 3])));
        assert_eq!(electorate_vote_filter(Some(set(&[1, 9])), Some(set(&[1, 2, 3]))), Some(set(&[1])),
            "9 is in the slice but not the electorate: it does not count");
    }
}
