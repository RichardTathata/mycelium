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
//! See [`GossipAgent::group_propose`] and [`GossipAgent::system_propose`] for
//! the entry points. [`GossipAgent::start_consensus_listener`] must be called
//! on every node that should participate as a voter.
//!
//! # Design notes
//!
//! - **Ballot numbering** (from SCP §6.2): monotonic counter stored at
//!   `consensus/ballot/{slot}`; higher ballot supersedes lower.
//! - **Group-scoped votes**: all group members see all votes; any member that
//!   reaches quorum may commit — proposer crash does not stall the slot.
//! - **No signing**: trusted-domain only; Byzantine fault tolerance is
//!   out of scope.
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
    /// (for [`group_propose`](crate::GossipAgent::group_propose)) or the
    /// known peer count + 1 (for
    /// [`system_propose`](crate::GossipAgent::system_propose)).
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
    /// within the group. In `system_propose`, suggestion defers this node if it is not the
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
    /// other key; expiry is evaluated read-side against the committed entry's HLC
    /// timestamp — the same evaporation convention capability entries use
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
        }
    }
}

/// Per-group quorum requirement for [`GossipAgent::cross_group_propose`].
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

/// Outcome of a [`group_propose`](crate::GossipAgent::group_propose) or
/// [`system_propose`](crate::GossipAgent::system_propose) call.
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
        /// Votes received during the final ballot attempt.
        ///
        /// Distinguishes "no voters heard at all" (likely partition) from
        /// "some voters heard but quorum was not met" (likely overloaded members
        /// or quorum set too high). `0` if no vote arrived in the last ballot.
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
    /// Epoch-lease window for a committed slot. Key: `consensus/lease/{slot}`.
    /// Value: u64 LE milliseconds. Written at commit time when
    /// [`ConsensusConfig::committed_lease_secs`] is set; absent for permanent
    /// commitments. Expiry is evaluated read-side against the committed
    /// entry's HLC timestamp — see [`ConsensusConfig::committed_lease_secs`].
    pub const LEASE:     &str = "consensus/lease/";
    /// The ballot a slot's most recent commit was decided at. Key: `consensus/decided/{slot}`,
    /// value `u64` LE. A ballot at or below it belongs to a finished decision: acceptors refuse
    /// it, and a proposer reopening an expired leased slot ignores acceptances at or below it.
    pub const DECIDED:   &str = "consensus/decided/";
}

// ── Lease helpers ─────────────────────────────────────────────────────────────

pub(crate) fn encode_lease_ms(ms: u64) -> Bytes {
    Bytes::copy_from_slice(&ms.to_le_bytes())
}

/// `None` on malformed bytes — readers treat a malformed lease as *permanent*
/// (never silently expire a commitment because a lease entry was corrupted).
pub(crate) fn decode_lease_ms(bytes: &Bytes) -> Option<u64> {
    (bytes.len() >= 8).then(|| u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8])))
}


/// The reader's **causal now** on the HLC physical domain: `max(wall clock, HLC physical)`.
///
/// This is the correct clock to compare a lease against (see [`live_committed_with_hlc`]). A lease's
/// written timestamp is the writer's *HLC physical* — the causal max of its wall clock and every peer
/// time it has observed. Comparing that against a reader's raw wall clock mixes two clock domains, so
/// two nodes can disagree on whether the same lease is live (audit 2026-07-15, BUG 8). Reading on the
/// same HLC domain — at least this node's wall clock, and at least any peer HLC it has observed —
/// puts both sides in one frame, shrinking the disagreement window to *true unsynchronised wall skew*
/// (which the fencing token then covers). It never reads *behind* wall time, so single-node lease
/// timing is unchanged; the HLC term only ever pulls it forward toward a peer the node has heard from.
pub(crate) fn causal_now_ms(hlc: &crate::hlc::Hlc) -> u64 {
    // The same formula as `Hlc::decision_now_ms`, which now exists for exactly this (C11).
    hlc.decision_now_ms()
}

/// Returns the **live** committed value for `slot`, applying the epoch-lease
/// convention: a committed entry whose `consensus/lease/{slot}` window has
/// elapsed (measured against the committed entry's HLC timestamp) reads as
/// absent — the slot has reopened. Slots without a lease entry are permanent.
///
/// This is the Layer III read-side analogue of [`CapEntry::is_fresh`]
/// (crate::CapEntry::is_fresh): expiry is a property readers apply, not a
/// store mechanism — the substrate never deletes or special-cases the key.
pub(crate) fn live_committed_value(
    kv:     &crate::store::KvState,
    slot:   &str,
    now_ms: u64,
) -> Option<Bytes> {
    live_committed_with_hlc(kv, slot, now_ms).map(|(v, _)| v)
}

/// Like [`live_committed_value`] but also returns the committed entry's **HLC timestamp** — the
/// value used as a lock's fencing token. The HLC is monotonic-respecting-causality, so successive
/// holders of a slot see strictly increasing tokens (unlike the ballot, which is per-node-local
/// and gossip-lagged — it can regress across acquisitions and must NOT be used for fencing).
pub(crate) fn live_committed_with_hlc(
    kv:     &crate::store::KvState,
    slot:   &str,
    now_ms: u64,
) -> Option<(Bytes, u64)> {
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
    // BOUNDED-CLOCK-SKEW ASSUMPTION (audit 2026-07-15, BUG 8). `now_ms` is compared against the
    // writer's HLC physical component. Production callers pass [`causal_now_ms`] (`max(wall, HLC
    // physical)`), so both sides sit on the *same* causal-HLC domain — the earlier bug, comparing a
    // reader's raw wall clock against the writer's HLC, let two nodes disagree on whether the SAME
    // lease was live and both briefly believe they held a `distributed_lock`. On the shared domain the
    // disagreement window shrinks to *true unsynchronised wall skew*. (Tests inject a raw `now_ms`
    // directly — that is the intended clock-injection seam for lease-expiry unit tests.) Like every
    // lease-based lock (Chubby, etcd), correctness for a *lease* holds only while skew < lease; the
    // **skew-proof** guard for correctness-critical writes is the FENCING TOKEN returned alongside —
    // the commit's HLC, monotonic across successive holders (each observes the prior release), so a
    // resource that rejects a lower token is fenced even if two nodes momentarily both think they hold.
    (now_ms.saturating_sub(written_ms) <= lease_ms).then_some((data, hlc))
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
/// Injected by `group_propose` / `system_propose` at the signal-mesh call site so
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
// GossipAgent::group_propose / system_propose, then either spawned
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

    fn read_ballot(&self, ballot_key: &str) -> u64 {
        self.get(ballot_key).map(|b| decode_ballot(&b)).unwrap_or(0)
    }

    /// Lease-aware committed read — see [`live_committed_value`]. An expired
    /// lease reads as `None`: the slot has reopened for re-proposal.
    fn live_committed(&self, slot: &str) -> Option<Bytes> {
        live_committed_value(&self.task_ctx.kv_state, slot, causal_now_ms(&self.task_ctx.hlc))
    }

    /// Applies a KV update from within a consensus task.
    /// Uses `try_send` for gossip dispatch — dropped frames recovered via anti-entropy.
    fn kv_set(&self, key: String, value: Bytes) {
        let tc  = &self.task_ctx;
        let upd = make_gossip_update(&tc.node_id, tc.default_ttl, Arc::from(key.as_str()), value, false, &tc.hlc);
        apply_and_notify(&tc.kv_state, &upd);
        let tls = tc.tls.get().map(std::sync::Arc::as_ref);
        let msg = make_kv_wire_msg(upd, tc.node_id.id_hash(), tls);
        dispatch_gossip_try_send(
            &tc.gossip_txs, msg,
            tc.node_id.id_hash(), ForwardHint::All, &tc.kv_state.dropped_frames,
        );
    }

    /// The ballot this slot's most recent commit was decided at, as this node knows it; `0` when
    /// none — see [`consensus_ns::DECIDED`].
    fn decided_floor(&self, slot: &str) -> u64 {
        self.get(&format!("{}{}", consensus_ns::DECIDED, slot)).map(|b| decode_ballot(&b)).unwrap_or(0)
    }

    /// Whether this node can see the slot's latest decision is **over**: it holds the committed
    /// entry — data or tombstone — and the entry is not live (its lease expired, or a lock release
    /// tombstoned it). An absent entry is *not* over: the commit may simply not have arrived yet.
    fn decision_over(&self, slot: &str) -> bool {
        let present = self.task_ctx.kv_state.store.pin()
            .get(format!("{}{}", consensus_ns::COMMITTED, slot).as_str()).is_some();
        present && self.live_committed(slot).is_none()
    }

    /// Records that `slot` was decided at `ballot`, never lowering what is recorded.
    fn record_decided(&self, slot: &str, ballot: u64) {
        if ballot > self.decided_floor(slot) {
            self.kv_set(format!("{}{}", consensus_ns::DECIDED, slot), encode_ballot(ballot));
        }
    }

    /// Writes `ballot` to the slot's shared ballot key unless a higher one is already there, so
    /// proposers drawing their next ballot start above it.
    async fn raise_ballot(&self, ballot_key: &str, ballot: u64) {
        if ballot > self.read_ballot(ballot_key) {
            self.set_async(ballot_key, encode_ballot(ballot)).await;
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
    fn persist_acceptor(&self, slot: &Arc<str>) {
        let key = accepted_key(&self.task_ctx.node_id, slot);
        let read = || self.task_ctx.consensus_accepted.pin().get(slot).map(encode_acceptor);
        let mut current = read();
        for _ in 0..4 {
            let Some(bytes) = current else { return };
            self.kv_set(key.clone(), bytes.clone());
            let after = read();
            if after.as_ref() == Some(&bytes) { return; }
            current = after;
        }
    }

    /// Tombstones `key` in the KV store and gossips the deletion.
    fn kv_delete(&self, key: &str) {
        let tc  = &self.task_ctx;
        let upd = make_gossip_update(&tc.node_id, tc.default_ttl, Arc::from(key), Bytes::new(), true, &tc.hlc);
        apply_and_notify(&tc.kv_state, &upd);
        let tls = tc.tls.get().map(std::sync::Arc::as_ref);
        let msg = make_kv_wire_msg(upd, tc.node_id.id_hash(), tls);
        dispatch_gossip_try_send(
            &tc.gossip_txs, msg,
            tc.node_id.id_hash(), ForwardHint::All, &tc.kv_state.dropped_frames,
        );
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
        crate::agent::is_self_opaque(&self.task_ctx.kv_state, &self.task_ctx.node_id)
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
    fn sign_payload(&self, bytes: Bytes) -> Bytes {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.task_ctx.tls.get() {
            let sig = crate::tls::sign_bytes(&tls.signing_key(), &bytes);
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
            if key_set.is_empty()
                || !key_set.iter().any(|k| crate::tls::verify_bytes(k, &signed.msg_bytes, &signed.signature))
            {
                tracing::warn!("dropping consensus msg: bad/unknown signature from {}", signed.signer);
                return None;
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
            return Some(msg);
        }
        decode_consensus_msg(payload)
    }

    // ── Proposer ─────────────────────────────────────────────────────────────

    /// Runs one full proposal attempt sequence for `slot`.
    ///
    /// Called by `GossipAgent::group_propose` and `GossipAgent::system_propose`.
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

        // The value this proposer is currently carrying. It starts as the caller's, and is
        // **replaced** by any higher-balloted accepted value an acceptor reports — accepted-value
        // preservation, without which a retry at `ballot + 1` can overwrite what a quorum already
        // accepted. `adopted_from` is the ballot the current value was accepted at (0 = ours).
        let mut value = value;
        let mut adopted_from: u64 = 0;

        let mut quorum_size = quorum_size;
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

        // Above the slot's decided ballot too: the ballot key is no longer reset at commit, but it is
        // gossiped and may lag what this node knows was decided.
        let mut ballot = self.read_ballot(&ballot_key).max(self.decided_floor(&slot)) + 1;
        let mut votes_last_ballot: usize = 0;
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
        let superseded_by_live = |existing: &Bytes, current: &Bytes| -> bool {
            !(lease_ms.is_some() && existing == current)
        };

        // Extract the group name once for `sys/topology-override/{group}` lookups
        // and for completing the TopologyUnsatisfied return.
        let group_name: Option<Arc<str>> = match &scope {
            SignalScope::Group(g) => Some(Arc::clone(g)),
            _                     => None,
        };

        for _attempt in 0..config.max_ballots {
            if let Some(existing) = self.live_committed(&slot)
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
                trust_set.as_ref(), config.phase1_timeout,
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
                Phase1::Ready(Phase1Choice::Blocked) | Phase1::Short => Some(0),
                Phase1::Refused(seen) => Some(seen),
            };
            if let Some(floor) = retry_floor {
                ballot_retry_pause(config.ballot_retry_jitter_ms).await;
                ballot = floor.max(self.read_ballot(&ballot_key)).max(ballot) + 1;
                continue;
            }

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
            if let Some(existing) = self.live_committed(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
                }
            let floor = self.decided_floor(&slot);
            if !claim_vote(&self.task_ctx.consensus_accepted, &slot, ballot, &value, self.task_ctx.node_id.id_hash(), floor) {
                // Already committed to a different value at this ballot. Cannot win here; move up
                // rather than emit a proposal we are not entitled to support.
                ballot = ballot.max(self.read_ballot(&ballot_key)) + 1;
                continue;
            }
            // Durable before the proposal leaves, for the same reason the voter records before its
            // vote leaves: a proposal is an acceptance, and an acceptance a restart forgets is one
            // this node can contradict.
            self.persist_acceptor(&slot);

            self.raise_ballot(&ballot_key, ballot).await;

            let propose_msg = ConsensusMsg::Propose {
                slot: Arc::clone(&slot), ballot, value: value.clone(),
                proposer: self.task_ctx.node_id.clone(),
            };
            self.emit_async(
                Arc::from(consensus_kind::PROPOSE), scope.clone(), self.sign_payload(encode_consensus_msg(&propose_msg)),
            ).await;

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
                &scope, &slot, ballot, &value, &ballot_key, &commit_key, lease_ms,
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
            ).await;

            let mut nack_ballot = 0u64;
            match outcome {
                BallotOutcome::Committed(res) => return res,
                BallotOutcome::NackHigher(b, reported) => {
                    nack_ballot = b;
                    // **Accepted-value preservation.** An acceptor that refused us reported what
                    // it already holds; the next ballot must carry *that* value, not ours. Without
                    // this the retry is `ballot + 1` with the proposer's original value, which can
                    // replace a value a quorum already accepted — acceptors permit it, because any
                    // strictly greater ballot passes `may_cast_vote`.
                    if let Some((ab, v)) = reported
                        && ab >= adopted_from {
                            adopted_from = ab;
                            value = v;
                        }
                }
                BallotOutcome::Timeout => {}
            }

            if let Some(existing) = self.live_committed(&slot)
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
            ballot = nack_ballot.max(self.read_ballot(&ballot_key)).max(ballot) + 1;
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

        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_timeouts_total",
            "reason" => if votes_last_ballot == 0 { "no_voters" } else { "quorum_short" })
            .increment(1);
        ConsensusResult::Timeout {
            slot,
            ballots_tried: config.max_ballots,
            votes_last_ballot,
            quorum_required: quorum_size,
        }
    }

    /// Proposes `value` for `slot` requiring independent quorum from each group in `groups`.
    ///
    /// Commits only when every group reaches its configured [`GroupQuorum::quorum`] fraction.
    /// Uses a single ballot round so the commit is atomic — no group can commit without
    /// all others also committing.
    ///
    /// Called by [`GossipAgent::cross_group_propose`].
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

        let ballot_key = format!("{}{}", consensus_ns::BALLOT,    &*slot);
        let commit_key = format!("{}{}", consensus_ns::COMMITTED, &*slot);

        // The value this proposer carries; replaced by any higher-balloted accepted value an
        // acceptor reports. `adopted_from` is the ballot it was accepted at (0 = ours).
        let mut value = value;
        let mut adopted_from: u64 = 0;

        // Epoch lease window (ms), with the same renewal exception as `propose`:
        // a live same-value leased commitment may be re-proposed to refresh it.
        let lease_ms = config.committed_lease_secs.map(|s| s.saturating_mul(1000));
        let superseded_by_live = |existing: &Bytes, current: &Bytes| -> bool {
            !(lease_ms.is_some() && existing == current)
        };

        if let Some(existing) = self.live_committed(&slot)
            && superseded_by_live(&existing, &value) {
                return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
            }

        // Per-group state (rebuilt from KV once before the ballot loop).
        struct CrossState {
            members:     ahash::AHashSet<NodeId>,
            quorum_frac: f32,
            accepts:     usize,
            seen:        ahash::AHashSet<NodeId>, // distinct voters — each counts once
        }

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

        let mut ballot = self.read_ballot(&ballot_key).max(self.decided_floor(&slot)) + 1;

        for _attempt in 0..config.max_ballots {
            for gs in group_states.values_mut() { gs.accepts = 0; }

            // Phase 1, with the same per-group quorum the commit needs — see `propose`.
            let ready = |p: &AHashSet<NodeId>| group_states.values().all(|gs| {
                gs.members.iter().filter(|m| p.contains(*m)).count()
                    >= cross_group_quorum(gs.members.len(), gs.quorum_frac)
            });
            let phase1 = self.prepare_phase(
                &mut vote_rx, &mut nack_rx, &scope, &slot, ballot, &value, &ready, None,
                config.phase1_timeout,
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
                Phase1::Ready(Phase1Choice::Blocked) | Phase1::Short => Some(0),
                Phase1::Refused(seen) => Some(seen),
            };
            if let Some(floor) = retry_floor {
                ballot_retry_pause(config.ballot_retry_jitter_ms).await;
                ballot = floor.max(self.read_ballot(&ballot_key)).max(ballot) + 1;
                continue;
            }

            // A fresh refusal channel for phase 2 — see `propose`.
            nack_rx = self.task_ctx.signal_handlers.register_with_capacity(
                Arc::from(consensus_kind::NACK), 64,
            );

            // The node-level gate `propose` passes too: one value per ballot from this node, however
            // many proposals it runs for the slot at once. This proposer does not count its own vote
            // here; the claim is the gate, not a vote.
            let floor = self.decided_floor(&slot);
            if !claim_vote(&self.task_ctx.consensus_accepted, &slot, ballot, &value, self.task_ctx.node_id.id_hash(), floor) {
                ballot = ballot.max(self.read_ballot(&ballot_key)) + 1;
                continue;
            }
            self.persist_acceptor(&slot);
            self.raise_ballot(&ballot_key, ballot).await;

            let propose_msg = ConsensusMsg::Propose {
                slot: Arc::clone(&slot), ballot, value: value.clone(),
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
            let mut learned: Option<(u64, Bytes)> = None;

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
                                    if gs.seen.insert(voter.clone()) {
                                        gs.accepts += 1;
                                    }
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
                            if let Some(existing) = self.live_committed(&slot)
                                && existing != value {
                                    return ConsensusResult::Superseded {
                                        slot, ballot: self.read_ballot(&ballot_key),
                                    };
                                }
                            let commit_msg = ConsensusMsg::Commit {
                                slot: Arc::clone(&slot), ballot, value: value.clone(),
                            };
                            self.emit_async(
                                Arc::from(consensus_kind::COMMIT),
                                scope.clone(),
                                self.sign_payload(encode_consensus_msg(&commit_msg)),
                            ).await;
                            let committed_upd = self.set_async(&commit_key, value.clone()).await;
                            let persisted = self.persist_committed(&slot, &committed_upd).await
                                & self.write_lease(&slot, lease_ms).await;
                            self.record_decided(&slot, ballot);
                            return ConsensusResult::Committed { slot, value: value.clone(), ballot, persisted };
                        }
                    }
                    Some(sig) = nack_rx.recv() => {
                        match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::Promise {
                                slot: s, seen_ballot, accepted_ballot, accepted_value,
                            }) if s == slot && seen_ballot >= ballot => {
                                nack_ballot = seen_ballot;
                                if let Some(v) = accepted_value
                                    && accepted_ballot >= learned.as_ref().map(|(b, _)| *b).unwrap_or(0) {
                                        learned = Some((accepted_ballot, v));
                                    }
                                break 'collect;
                            }
                            Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                                if s == slot && seen_ballot >= ballot => {
                                nack_ballot = seen_ballot;
                                break 'collect;
                            }
                            _ => {}
                        }
                    }
                }
            }

            if let Some(existing) = self.live_committed(&slot)
                && superseded_by_live(&existing, &value) {
                    return ConsensusResult::Superseded { slot, ballot: self.read_ballot(&ballot_key) };
                }

            ballot_retry_pause(config.ballot_retry_jitter_ms).await;
            // Adopt before retrying: a value an acceptor already holds outranks ours.
            if let Some((ab, v)) = learned.take()
                && ab >= adopted_from {
                    adopted_from = ab;
                    value = v;
                }
            ballot = nack_ballot.max(self.read_ballot(&ballot_key)).max(ballot) + 1;
        }

        let votes_last_ballot: usize = group_states.values().map(|gs| gs.accepts).sum();
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_consensus_timeouts_total",
            "reason" => if votes_last_ballot == 0 { "no_voters" } else { "quorum_short" })
            .increment(1);
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
        timeout:   Duration,
    ) -> Phase1 {
        let me = &self.task_ctx.node_id;
        let mut reports: Vec<AcceptReport> = Vec::new();
        let floor = self.decided_floor(slot);
        match prepare_slot(&self.task_ctx.consensus_accepted, slot, ballot, me.id_hash(), floor) {
            PrepareOutcome::Refused { promised, .. } => return Phase1::Refused(promised),
            PrepareOutcome::Promised(acc) => {
                if let Some((b, a)) = acc { reports.push((b, a.digest(), a.value())); }
            }
        }
        self.persist_acceptor(slot);
        let mut promisers: AHashSet<NodeId> = AHashSet::new();
        promisers.insert(me.clone());

        if !ready(&promisers) {
            // Publish the ballot before asking, so a proposer drawing its next one starts above it
            // rather than colliding on it.
            self.raise_ballot(&format!("{}{}", consensus_ns::BALLOT, &**slot), ballot).await;
            let prepare = ConsensusMsg::Prepare {
                slot: Arc::clone(slot), ballot, proposer: me.clone(),
            };
            self.emit_async(
                Arc::from(consensus_kind::PROPOSE), scope.clone(),
                self.sign_payload(encode_consensus_msg(&prepare)),
            ).await;
            let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            let sleep = mycelium_core::sim_seam::sleep_ms("consensus.prepare", ms);
            tokio::pin!(sleep);
            loop {
                tokio::select! { biased;
                    _ = &mut sleep => return Phase1::Short,
                    Some(sig) = vote_rx.recv() => {
                        let Some(ConsensusMsg::PrepareAck {
                            slot: s, ballot: b, voter, accepted_ballot, accepted_digest,
                            accepted_value, committed_digest,
                        }) = self.decode_verify(&sig.payload) else { continue };
                        if s != *slot || b != ballot { continue; }
                        if let Some(ts) = trust_set
                            && !ts.contains(&voter.id_hash()) { continue; }
                        // A live commit of a different value ends this proposal. The same value — a
                        // leased slot being renewed — is one promise like any other: no shortcut
                        // past the quorum, whose reports still decide.
                        if committed_digest.is_some_and(|d| d != value_digest(current)) {
                            return Phase1::DecidedOtherwise;
                        }
                        if accepted_ballot > 0 {
                            // A digest is always sent with an acceptance; a value without one is
                            // checked against its own digest by `choose_after_prepare`.
                            let digest = accepted_digest
                                .or_else(|| accepted_value.as_ref().map(value_digest));
                            match digest {
                                Some(d) => reports.push((accepted_ballot, d, accepted_value)),
                                // An acceptance we cannot identify constrains us without telling
                                // us how: do not count this promiser.
                                None => continue,
                            }
                        }
                        promisers.insert(voter);
                        if ready(&promisers) { break; }
                    }
                    Some(sig) = nack_rx.recv() => {
                        match self.decode_verify(&sig.payload) {
                            Some(ConsensusMsg::Promise { slot: s, seen_ballot, .. })
                                if s == *slot && seen_ballot >= ballot =>
                                return Phase1::Refused(seen_ballot),
                            Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                                if s == *slot && seen_ballot > ballot =>
                                return Phase1::Refused(seen_ballot),
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
        set_aside_finished(&mut reports, floor, self.decision_over(slot));
        Phase1::Ready(choose_after_prepare(current, &reports))
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
    ) -> Option<ConsensusResult> {
        if voters.len() < quorum_size { return None; }
        let (passes, _, _) = self.topology_check(voters, group_name);
        if !passes { return None; }

        if let Some(existing) = self.live_committed(slot)
            && existing != *value {
                return Some(ConsensusResult::Superseded {
                    slot:   Arc::clone(slot),
                    ballot: self.read_ballot(ballot_key),
                });
            }

        let commit = ConsensusMsg::Commit {
            slot: Arc::clone(slot), ballot, value: value.clone(),
        };
        self.emit_async(
            Arc::from(consensus_kind::COMMIT), scope.clone(), self.sign_payload(encode_consensus_msg(&commit)),
        ).await;
        let committed_upd = self.set_async(commit_key, value.clone()).await;
        let persisted = self.persist_committed(slot, &committed_upd).await
            & self.write_lease(slot, lease_ms).await;
        self.record_decided(slot, ballot);
        Some(ConsensusResult::Committed {
            slot:   Arc::clone(slot),
            value:  value.clone(),
            ballot,
            persisted,
        })
    }

    /// Forces the committed-slot record to stable storage (`append_sync` —
    /// `fdatasync` in every `SyncMode`). Returns `false` on failure, which the
    /// commit surfaces as `Committed { persisted: false }` rather than swallowing:
    /// the cluster commit already happened (COMMIT emitted, value applied), so the
    /// honest report is "committed, not locally durable". `true` when persistence
    /// is not configured — no promise was made.
    async fn persist_committed(&self, slot: &Arc<str>, upd: &crate::framing::GossipUpdate) -> bool {
        let Some(wal) = self.task_ctx.wal.get() else { return true; };
        match wal.append_sync(sync_entry_from(upd)).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(
                    slot = %slot, error = %e,
                    "consensus: committed slot did not reach stable storage on this node",
                );
                false
            }
        }
    }

    /// Writes (or clears) the epoch-lease window for `slot` at commit time.
    ///
    /// `Some(ms)` → write `consensus/lease/{slot}` (WAL-appended alongside the
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
                let Some(wal) = self.task_ctx.wal.get() else { return true; };
                match wal.append_sync(sync_entry_from(&upd)).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::error!(
                            slot = %slot, error = %e,
                            "consensus: lease for committed slot did not reach stable storage on this node",
                        );
                        false
                    }
                }
            }
            None => {
                if self.get(&lease_key).is_some() {
                    self.kv_delete(&lease_key);
                }
                true
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
                        voters.insert(voter, locality);
                        if let Some(res) = self.try_commit_if_ready(
                            voters, *quorum_size, group_name,
                            scope, slot, ballot, value, ballot_key, commit_key, lease_ms,
                        ).await {
                            return BallotOutcome::Committed(res);
                        }
                        // Quorum count met but topology gate failed — keep
                        // collecting until timeout in case a more diverse voter
                        // arrives. (try_commit_if_ready returned None.)
                    }
                }
                Some(sig) = nack_rx.recv() => {
                    match self.decode_verify(&sig.payload) {
                        // A `Promise` refuses **and reports**: the proposer must adopt the accepted
                        // value at the next ballot instead of retrying with its own.
                        Some(ConsensusMsg::Promise {
                            slot: s, seen_ballot, accepted_ballot, accepted_value,
                        }) if s == *slot && seen_ballot >= ballot =>
                            return BallotOutcome::NackHigher(
                                seen_ballot, accepted_value.map(|v| (accepted_ballot, v))),
                        Some(ConsensusMsg::Nack { slot: s, seen_ballot })
                            if s == *slot && seen_ballot >= ballot =>
                            return BallotOutcome::NackHigher(seen_ballot, None),
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
                            scope, slot, ballot, value, ballot_key, commit_key, lease_ms,
                        ).await {
                            return BallotOutcome::Committed(res);
                        }
                    }
                }
            }
        }
    }
}

/// Phase 1's reports, with the previous decision's acceptances (at or below `floor`) set aside —
/// **only** when this node sees that decision is `over`; see `ConsensusEngine::prepare_phase`.
fn set_aside_finished(reports: &mut Vec<AcceptReport>, floor: u64, over: bool) {
    if over {
        reports.retain(|r| r.0 > floor);
    }
}

/// Whether a COMMIT at `ballot` belongs to a decision this node knows has been superseded (below the
/// floor) or has ended (at the floor, and `over`) — such a COMMIT is not re-stamped.
fn commit_is_stale(ballot: u64, floor: u64, over: bool) -> bool {
    ballot < floor || (ballot == floor && over)
}

/// A `Committed` for a value other than `asked` becomes `Superseded` — see `ConsensusEngine::propose`.
fn own_or_superseded(result: ConsensusResult, asked: &Bytes) -> ConsensusResult {
    match result {
        ConsensusResult::Committed { slot, value, ballot, .. } if value != *asked =>
            ConsensusResult::Superseded { slot, ballot },
        other => other,
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
    /// No quorum promised within the timeout.
    Short,
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
    NackHigher(u64, Option<(u64, Bytes)>),
    /// `phase1_timeout` elapsed. Caller may retry or surface TopologyUnsatisfied
    /// based on whether voters reached quorum-by-count.
    Timeout,
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
        ConsensusMsg::Commit { .. } | ConsensusMsg::Nack { .. } | ConsensusMsg::Promise { .. } => true,
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
    let mut v = Vec::with_capacity(ACCEPTOR_RECORD_LEN);
    v.push(0x02);
    v.extend_from_slice(&state.promised.to_le_bytes());
    v.push(u8::from(state.promised_to.is_some()));
    v.extend_from_slice(&state.promised_to.unwrap_or(0).to_le_bytes());
    v.push(u8::from(state.accepted.is_some()));
    let (ab, d) = state.accepted.as_ref().map(|(b, a)| (*b, a.digest())).unwrap_or((0, [0u8; 32]));
    v.extend_from_slice(&ab.to_le_bytes());
    v.extend_from_slice(&d);
    // The value itself when it is small, so a restarted acceptor can still hand it to a proposer;
    // without it, a value known only by digest blocks every other proposal for the slot. Small,
    // because the record is gossiped cluster-wide and kept: a large value would be paid for on
    // every node, per acceptor, for good.
    if let Some((_, Accepted::Full(value))) = &state.accepted
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
    if bytes.len() < ACCEPTOR_RECORD_LEN || bytes[0] != 0x02 { return None; }
    let u64_at = |i: usize| bytes[i..i + 8].try_into().ok().map(u64::from_le_bytes);
    let flag = |i: usize| match bytes[i] { 0 => Some(false), 1 => Some(true), _ => None };
    let promised = u64_at(1)?;
    let promised_to = flag(9)?.then_some(u64_at(10)?);
    let accepted = if flag(18)? {
        let ab = u64_at(19)?;
        let mut d = [0u8; 32];
        d.copy_from_slice(&bytes[27..59]);
        if ab > promised { return None; }
        let rest = &bytes[ACCEPTOR_RECORD_LEN..];
        if rest.is_empty() {
            Some((ab, Accepted::DigestOnly(d)))
        } else {
            let value = Bytes::copy_from_slice(rest);
            if value_digest(&value) != d { return None; }
            Some((ab, Accepted::Full(value)))
        }
    } else {
        if bytes.len() != ACCEPTOR_RECORD_LEN { return None; }
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
}

#[cfg(feature = "consensus")]
impl Accepted {
    /// The digest of whatever was accepted — the only thing equality is ever asked about.
    pub(crate) fn digest(&self) -> [u8; 32] {
        match self {
            Accepted::Full(v)     => value_digest(v),
            Accepted::DigestOnly(d) => *d,
        }
    }
    /// The value, when this node still has it. `None` after a restart.
    pub(crate) fn value(&self) -> Option<Bytes> {
        match self {
            Accepted::Full(v)       => Some(v.clone()),
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
/// The memory is **never erased** on commit. It used to be, to bound the record prefix, and that
/// dropped promises: a delayed lower-ballot proposal reached acceptors that had forgotten what they
/// promised and could commit a second value (2026-10-08 review). The prefix grows with the number
/// of slots, not of ballots.
#[cfg(feature = "consensus")]
pub(crate) fn claim_vote(
    memory:   &AcceptorMemory,
    slot:     &Arc<str>,
    ballot:   u64,
    value:    &Bytes,
    proposer: u64,
    floor:    u64,
) -> bool {
    use papaya::Operation;
    let want = value_digest(value);
    let mut granted = false;
    memory.pin().compute(Arc::clone(slot), |entry| {
        // Recomputed from scratch on every retry — never from a prior attempt's result.
        let cur = entry.map(|(_, s)| s.clone()).unwrap_or_default();
        granted = ballot > floor && may_accept(&cur, ballot, want, proposer);
        if granted {
            Operation::Insert(AcceptorSlot {
                promised:    ballot,
                promised_to: Some(proposer),
                accepted:    Some((ballot, Accepted::Full(value.clone()))),
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

// ── Voter task ───────────────────────────────────────────────────────────────

/// Answers a [`Prepare`](ConsensusMsg::Prepare): promise and report, or refuse and say what was
/// promised. The promise is **recorded before the answer leaves** — applied to the store and handed
/// to the WAL, the same rung a vote's record reaches (not an fsync) — because a proposer chooses its
/// value on the strength of it, and a restart must not let this node break it.
#[cfg(feature = "consensus")]
fn answer_prepare(ctx: &ConsensusEngine, slot: Arc<str>, ballot: u64, proposer: NodeId) {
    let floor = ctx.decided_floor(&slot);
    match prepare_slot(&ctx.task_ctx.consensus_accepted, &slot, ballot, proposer.id_hash(), floor) {
        PrepareOutcome::Promised(acc) => {
            ctx.persist_acceptor(&slot);
            let ack = ConsensusMsg::PrepareAck {
                slot:            Arc::clone(&slot),
                ballot,
                voter:           ctx.task_ctx.node_id.clone(),
                accepted_ballot: acc.as_ref().map(|(b, _)| *b).unwrap_or(0),
                accepted_digest: acc.as_ref().map(|(_, a)| a.digest()),
                accepted_value:  acc.and_then(|(_, a)| a.value()),
                committed_digest: ctx.live_committed(&slot).map(|c| value_digest(&c)),
            };
            ctx.emit(
                Arc::from(consensus_kind::VOTE),
                SignalScope::Individual(proposer),
                ctx.sign_payload(encode_consensus_msg(&ack)),
            );
        }
        PrepareOutcome::Refused { promised, accepted } => {
            let refusal = ConsensusMsg::Promise {
                slot,
                seen_ballot:     promised,
                accepted_ballot: accepted.as_ref().map(|(b, _)| *b).unwrap_or(0),
                accepted_value:  accepted.and_then(|(_, a)| a.value()),
            };
            ctx.emit(
                Arc::from(consensus_kind::NACK),
                SignalScope::Individual(proposer),
                ctx.sign_payload(encode_consensus_msg(&refusal)),
            );
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
            _ = shutdown_rx.wait_for(|v| *v) => break,
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

                let (slot, ballot, value, proposer) = match ctx.decode_verify(&sig.payload) {
                    Some(ConsensusMsg::Prepare { slot, ballot, proposer }) => {
                        answer_prepare(&ctx, slot, ballot, proposer);
                        continue;
                    }
                    Some(ConsensusMsg::Propose { slot, ballot, value, proposer }) =>
                        (slot, ballot, value, proposer),
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
                let decided_otherwise = ctx.live_committed(&slot).is_some_and(|c| c != value);
                let floor = ctx.decided_floor(&slot);
                if decided_otherwise
                    || !claim_vote(&accepted, &slot, ballot, &value, proposer.id_hash(), floor) {
                    // Report what we hold, so the proposer can **adopt** it at a higher ballot
                    // rather than overwrite it. A bare `Nack` says only "I have seen a ballot",
                    // which leaves the proposer free to retry with its own value and replace a
                    // value a quorum already accepted.
                    let state = accepted.pin().get(&slot).cloned().unwrap_or_default();
                    let local = state.promised.max(floor);
                    let held = state.accepted;
                    let promise = ConsensusMsg::Promise {
                        slot:            Arc::clone(&slot),
                        seen_ballot:     local,
                        accepted_ballot: held.as_ref().map(|(b, _)| *b).unwrap_or(0),
                        // `None` after a restart: the durable record carries the digest, not the
                        // value, so this node can refuse a conflicting vote but cannot help the
                        // proposer adopt. Safety survives the restart; that liveness aid does not.
                        accepted_value:  held.and_then(|(_, a)| a.value()),
                    };
                    ctx.emit(
                        Arc::from(consensus_kind::NACK),
                        SignalScope::Individual(proposer.clone()),
                        ctx.sign_payload(encode_consensus_msg(&promise)),
                    );
                    // And the legacy refusal, for a proposer that predates `Promise`.
                    let nack = ConsensusMsg::Nack { slot, seen_ballot: local };
                    ctx.emit(
                        Arc::from(consensus_kind::NACK),
                        SignalScope::Individual(proposer),
                        ctx.sign_payload(encode_consensus_msg(&nack)),
                    );
                } else {
                    // **Durable before the vote leaves.** A vote that reaches a proposer while this
                    // node's acceptance is unrecorded is exactly the memory a restart would lose —
                    // the proposer counts it, the node forgets it, and after a restart the node can
                    // vote again at the same ballot for another value. Same ordering rule as
                    // "apply to the store, then hand the record to the WAL", one layer up.
                    ctx.persist_acceptor(&slot);
                    ctx.kv_set(
                        format!("{}{}", consensus_ns::BALLOT, &*slot),
                        encode_ballot(ballot),
                    );
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
                        ctx.sign_payload(encode_consensus_msg(&bound)),
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
                        ctx.sign_payload(encode_consensus_msg(&vote)),
                    );
                }
            }
            Some(sig) = rx_commit.recv() => {
                let Some(ConsensusMsg::Commit { slot, ballot, value }) =
                    ctx.decode_verify(&sig.payload)
                else { continue };

                // A COMMIT from a decision already superseded, or one this node already saw end
                // (a lease expired, a lock released), is not re-stamped: re-stamping gives the old
                // value a fresh HLC, which revives its lease and can win LWW over a newer decision.
                let floor = ctx.decided_floor(&slot);
                if commit_is_stale(ballot, floor, ctx.decision_over(&slot)) {
                    continue;
                }

                // ── Commit-conflict tripwire ─────────────────────────────────
                // Slots are commit-once (or renewed with the same value while an
                // epoch lease is live; any value once the lease has expired —
                // `live_committed` returns None for an expired lease, so a legal
                // reopen never trips this). A COMMIT carrying a *different* value
                // while the existing commitment is still live is a protocol
                // violation: a raced double-commit, a buggy proposer, or a forged
                // message. Refuse to endorse it — re-writing it here would
                // propagate the clobber with a fresh HLC from this node, actively
                // helping it win LWW everywhere. Detection only: the substrate's
                // own forwarding of the foreign frame is untouched (Layer I/II
                // stay ignorant of Layer III's laws).
                if let Some(existing) = ctx.live_committed(&slot)
                    && existing != value {
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
                        tracing::warn!(
                            slot = %slot, ballot,
                            "commit conflict: COMMIT carries a different value for a \
                             live committed slot; not endorsing \
                             (see SystemStats::commit_conflicts)"
                        );
                        continue;
                    }

                // The acceptor's memory is **kept**: erasing it here dropped promises, and a delayed
                // lower-ballot proposal could then commit a second value (2026-10-08 review). What a
                // commit changes is the floor — a ballot at or below it is refused from now on.
                ctx.kv_set(
                    format!("{}{}", consensus_ns::COMMITTED, &*slot),
                    value,
                );
                ctx.record_decided(&slot, ballot);
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
        set_aside_finished(&mut reports, 5, false);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Adopt(5, v1.clone()),
                   "a decision not seen to be over still binds the proposer");
        set_aside_finished(&mut reports, 5, true);
        assert_eq!(choose_after_prepare(&mine, &reports), Phase1Choice::Keep,
                   "a decision seen to be over does not");
    }

    /// A late COMMIT is not re-stamped when it belongs to a superseded decision, or to one this node
    /// saw end; the decision in force is re-stamped as before (second review, L2).
    #[test]
    fn a_late_commit_does_not_resurrect_a_finished_decision() {
        assert!(commit_is_stale(3, 5, false), "below the floor: superseded");
        assert!(commit_is_stale(5, 5, true), "at the floor, after it ended: released or expired");
        assert!(!commit_is_stale(5, 5, false), "at the floor while live: the decision in force");
        assert!(!commit_is_stale(6, 5, true), "above the floor: a newer decision");
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
