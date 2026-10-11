//! **Electorate groups** — post-360 plan row P2, `docs/design/consensus-electorate.md` §8.
//!
//! Discovery and the electorate are separate capabilities (D1): a group's roster is dynamic, and a
//! consensus electorate is a fixed set for the life of a decision. An **electorate group** is a group
//! whose electorate is pinned by **identity**: an [`ElectorateDecl`] `{ group, epoch, members }` that is
//! itself a **consensus decision** — the slot `electorate/{group}/{epoch}` — decided by **every** member it
//! names, each checking the set is the group roster it sees. Each commit carries a **certificate** — the
//! deciding votes, signed — under `consensus/electorate-cert/{group}/{epoch}`, and a node adopts a record
//! only when its certificate verifies; one that does not is counted ([`electorate_records_refused`]).
//!
//! **Membership is fixed at genesis** (the maintainer's decision, 2026-10-11): once adopted, the group's members
//! never change. Changing who decides is a **migration** to another group, which **terminates** — close,
//! enumerate, carry, complete (the maintainer's decision of the same day; design record §8.3):
//!
//! - the successor's genesis **names its predecessors** (a set — a merge when two or more;
//!   [`ElectorateDecl::predecessors`]), and the successor decides nothing — at the proposer's door or any
//!   acceptor — until each has retired to it and that retirement is visible there;
//! - each predecessor **retires** — a decision of a strict majority of its members on `electorate/{group}/retire`,
//!   naming a successor that already names it ([`Retirement`]) and passing the fleet's default when it holds it.
//!   Every member **closes** the group, durably, at the retirement's promise, so once the retirement is chosen no
//!   value of the retired group can be chosen;
//! - a successor member **enumerates**: it asks a strict majority of the retired group for a **manifest** of every
//!   slot each holds state for in that group, and the union is the carry set `S` — complete, because a value chosen
//!   in the retired group is held by a majority of it, which the manifest majority intersects;
//! - it **carries** each slot in `S` with one ordinary ballot of the successor, proposing the highest acceptance the
//!   manifests reported (the decision envelope whole — the holder's window, lineage and token kept); and
//! - it **completes**: a decision of the successor on `electorate/{successor}/carried/{predecessor}`
//!   ([`Completion`]), after which the successor decides any slot without the predecessor, whose members may be
//!   removed. Until then the successor decides nothing outside `S`.
//!
//! The fleet's exclusive default is declared once, by a genesis, and moves only by retirement (`live_default`).
//! A dead member still lowers a group's fault tolerance, and a lost majority stops it: migrating needs a
//! reachable majority of the retired group (to retire it and to enumerate it) and of the successor.
//!
//! A genesis epoch counts **attempts**: a genesis whose members no longer all answer can be superseded by
//! one at the next epoch naming the live roster — a majority of the one it supersedes, every one of which
//! held it and none of which adopted it (round 4 of #601's review, finding 3).
//!
//! Every proposal for a slot on an electorate group carries the proposer's epoch and the electorate's
//! digest (`ConsensusMsg::PrepareIn` / `ProposeIn`); an acceptor answers only for its own electorate and
//! refuses any other by name (`ElectorateStale`). The rules and why they are safe are stated once, in the
//! design record §8.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{GossipAgent, TaskCtx};
use crate::node_id::NodeId;

/// The slot family an electorate's own decisions use: `electorate/{group}/{epoch}` (a genesis attempt),
/// `electorate/{group}/retire` (its retirement) and `electorate/{group}/carried/{predecessor}` (a migration's
/// completion).
pub const ELECTORATE_SLOT_PREFIX: &str = "electorate/";
/// `consensus/electorate-cert/{group}/{epoch}`, `…/{group}/retire` and `…/{group}/carried/{predecessor}` — the
/// decided envelope and the signed votes that decided a genesis, a retirement or a completion.
pub const ELECTORATE_CERT_PREFIX: &str = "consensus/electorate-cert/";

/// The last segment of a retirement's slot, and the segment before a completion's predecessor.
#[cfg(feature = "consensus")]
const RETIRE: &str = "retire";
#[cfg(feature = "consensus")]
const CARRIED: &str = "carried";
#[cfg(feature = "consensus")]
const DROP: &str = "drop";

/// Slot families the substrate's exclusive verbs propose in — safety-sensitive **by definition**:
/// `distributed_lock` and the lock route (`lock/`), `elect_leader` and the election route (`leader/`),
/// `consistent_set` and its route (`consistent/`), and an electorate's own decisions (`electorate/`). With
/// [`is_fleet_exclusive_slot`]'s `capauthz/` and commitment awards, these are what every acceptor refuses
/// across the cluster once the fleet marks an electorate.
pub const SAFETY_SLOT_FAMILIES: &[&str] = &["lock/", "leader/", "consistent/", "capauthz/", ELECTORATE_SLOT_PREFIX];

static RECORDS_REFUSED: AtomicU64 = AtomicU64::new(0);

/// [`ElectorateError::PersistenceRequired`]'s text — one sentence, the same at every door (`acceptor_guard`, detection 1).
pub(crate) const PERSISTENCE_REQUIRED_DETAIL: &str =
    "this node is a member of the electorate and has no [persistence]: its promises and acceptances would not survive \
     a restart, and a member restarted empty can let two values be decided (guide 04 § Changing an electorate's members)";

/// Electorate records this process refused to adopt — a committed `electorate/{group}/…` entry that is not
/// a well-formed genesis, retirement or completion, or whose certificate did not verify (a forgery, or a write that
/// did not come through consensus). Each distinct record is counted once.
pub fn electorate_records_refused() -> u64 {
    RECORDS_REFUSED.load(Ordering::Relaxed)
}

/// The pending genesis a superseding one replaces (round 4 of #601's review, finding 3).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Superseded {
    /// Its epoch — the one before the superseding genesis.
    pub epoch: u64,
    /// The digest of its record — SHA-256 of the `ElectorateDecl`'s encoding, the value inside the decision
    /// envelope that was accepted.
    pub digest: [u8; 32],
    /// The digest of the decision envelope the declaring member accepted it in — what a member that restarted
    /// still holds (its acceptor record keeps the digest only), so it can check it holds exactly this genesis.
    pub envelope: [u8; 32],
    /// Its members. The superseding genesis names a strict majority of them.
    pub members: Vec<NodeId>,
}

/// An electorate: the group, the genesis attempt that decided it, and **who** its members are. Frozen:
/// nothing changes it once adopted.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ElectorateDecl {
    /// The group.
    pub group: String,
    /// The genesis attempt: 1, or one more than the pending genesis it supersedes.
    pub epoch: u64,
    /// The members, sorted and without duplicates. Votes are counted only from these.
    pub members: Vec<NodeId>,
    /// Whether this genesis **declares** the fleet's default electorate for the cluster-scoped exclusive verbs
    /// (`distributed_lock`, `consistent_set`, the lock and consistent routes, the log claim) — accepted only while
    /// no live default is visible anywhere. After that the default moves only by retirement: a successor **holds**
    /// it when a predecessor that held it retired to it (`Retirement::passes_default`), whatever its own flag says
    /// (`electorate::holds_default`). `consensus_electorate` may only restate it.
    pub exclusive_default: bool,
    /// The pending genesis this one supersedes (epoch > 1 only).
    pub supersedes: Option<Superseded>,
    /// The groups this electorate **succeeds** — declared before they retire, so a retirement can only name a
    /// successor that already names it (design record §8.3). Two or more is a merge. The successor decides nothing
    /// until each of them has retired to it, then only the slots it carries from them until each carry is
    /// **complete** (`electorate/{group}/carried/{predecessor}`), and after that anything, without them.
    pub predecessors: BTreeSet<String>,
}

impl ElectorateDecl {
    /// A strict majority of the members.
    pub fn quorum(&self) -> usize {
        self.members.len() / 2 + 1
    }

    /// Whether `node` is a member.
    pub fn contains(&self, node: &NodeId) -> bool {
        self.members.iter().any(|m| m == node)
    }

    /// The electorate's digest — what a proposal names and an acceptor compares with its own.
    pub fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"mycelium.electorate/1\0");
        h.update(self.group.as_bytes());
        h.update([0]);
        h.update(self.epoch.to_le_bytes());
        for m in &self.members {
            h.update(m.to_string().as_bytes());
            h.update([0]);
        }
        h.update([u8::from(self.exclusive_default)]);
        match &self.supersedes {
            None => h.update([0]),
            Some(s) => {
                h.update([1]);
                h.update(s.epoch.to_le_bytes());
                h.update(s.digest);
                h.update(s.envelope);
                for m in &s.members {
                    h.update(m.to_string().as_bytes());
                    h.update([0]);
                }
            }
        }
        for p in &self.predecessors {
            h.update([1]);
            h.update(p.as_bytes());
            h.update([0]);
        }
        h.finalize().into()
    }

    /// The bytes a genesis proposes. A genesis with no predecessors is encoded in the 2.32.0 layout — byte for byte
    /// what a 2.32.0 node proposes and verifies — and one with predecessors in the layout that carries them; the
    /// codec is not self-describing, so the two layouts are told apart by [`decode`](Self::decode).
    #[cfg(feature = "consensus")]
    pub(crate) fn encode(&self) -> bytes::Bytes {
        let v = if self.predecessors.is_empty() {
            mycelium_core::serde_fixint::to_vec(&DeclV1::of(self))
        } else {
            mycelium_core::serde_fixint::to_vec(self)
        };
        bytes::Bytes::from(v.unwrap_or_default())
    }

    /// Decodes either layout: the one with predecessors, else the 2.32.0 one (which reads as none). A 2.32.0 node
    /// reading a record with predecessors ignores them — and then computes another digest, so it never answers a
    /// successor's proposals and a successor never answers its (the electorate's digest names the predecessors).
    #[cfg(feature = "consensus")]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        if let Ok(d) = mycelium_core::serde_fixint::from_slice::<Self>(bytes) {
            return Some(d);
        }
        mycelium_core::serde_fixint::from_slice::<DeclV1>(bytes).ok().map(DeclV1::into_decl)
    }
}

/// The 2.32.0 layout of a genesis: no predecessors.
#[cfg(feature = "consensus")]
#[derive(Serialize, Deserialize)]
struct DeclV1 {
    group: String,
    epoch: u64,
    members: Vec<NodeId>,
    exclusive_default: bool,
    supersedes: Option<Superseded>,
}

#[cfg(feature = "consensus")]
impl DeclV1 {
    fn of(d: &ElectorateDecl) -> Self {
        Self {
            group: d.group.clone(), epoch: d.epoch, members: d.members.clone(),
            exclusive_default: d.exclusive_default, supersedes: d.supersedes.clone(),
        }
    }
    fn into_decl(self) -> ElectorateDecl {
        ElectorateDecl {
            group: self.group, epoch: self.epoch, members: self.members, exclusive_default: self.exclusive_default,
            supersedes: self.supersedes, predecessors: BTreeSet::new(),
        }
    }
}

/// **A group's retirement** — a decision of a strict majority of its members, on `electorate/{group}/retire`,
/// naming the group that succeeds it. Once chosen, the retired group decides nothing again; its members only
/// report what they hold to the successor's enumeration (design record §8.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Retirement {
    /// The retired group.
    pub group: String,
    /// The retired electorate's epoch.
    pub epoch: u64,
    /// The successor group — already declared, naming this group among its predecessors.
    pub successor: String,
    /// Whether this retirement **passes the fleet's exclusive default** to the successor: `true` exactly when the
    /// retired group held it. Checked by the proposer and every acceptor against what they see, so the default has
    /// one holder across the handover, whatever the successor's own genesis flag says.
    pub passes_default: bool,
}

impl Retirement {
    #[cfg(feature = "consensus")]
    pub(crate) fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    #[cfg(feature = "consensus")]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// **A migration's completion** — a decision of the successor, on `electorate/{group}/carried/{predecessor}`,
/// recording that every slot in the carry set `S` enumerated from `predecessor` has been decided in `group`. After
/// it, `group` decides any slot without consulting `predecessor`, and `predecessor`'s members may be removed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Completion {
    /// The successor that carried.
    pub group: String,
    /// The successor's electorate epoch.
    pub epoch: u64,
    /// The retired predecessor whose state was carried.
    pub predecessor: String,
    /// The predecessor's electorate epoch, and whether its retirement passed the fleet's default — copied from the
    /// retirement, so a verified completion **stands as proof of the retirement** on a successor member that no
    /// longer holds the predecessor's records (they reach it by gossip; the completer fsyncs them, but a member that
    /// crashed before they reached its disk, with the predecessor gone, has only this).
    pub predecessor_epoch: u64,
    pub passes_default: bool,
    /// How many slots the carry set held.
    pub slots: u64,
    /// Slots whose carry found a **different value already decided above the carried one** in the successor — a
    /// detected conflict (two values chosen for one slot: a forced drop of a predecessor that then retired, a 2.32.0
    /// member that voted for the retirement without closing, two defaults merged that both held the slot). Counted on
    /// the commit-conflict tripwire; the value the successor holds stands; the first [`CONFLICTS_NAMED`] are named.
    pub conflicts: u64,
    pub conflict_slots: Vec<String>,
    /// SHA-256 over the sorted carry set — `slot\0ballot LE` per slot — so two completers who enumerated the same
    /// closed group propose the same value.
    pub digest: [u8; 32],
}

/// How many conflicting slots a completion names (the count is always exact).
#[cfg(feature = "consensus")]
pub const CONFLICTS_NAMED: usize = 64;

impl Completion {
    #[cfg(feature = "consensus")]
    pub(crate) fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    #[cfg(feature = "consensus")]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// **A dropped predecessor** — a decision of the successor on `electorate/{group}/drop/{predecessor}`, allowed only
/// while the predecessor has not retired to it: the successor stops waiting for that predecessor, and the
/// predecessor's retirement to this successor is refused from then on (its acceptors check for the drop). The escape
/// from a predecessor named by mistake, one whose majority is gone, or one that retired elsewhere (a losing
/// successor). Nothing of the predecessor is carried.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct PredecessorDrop {
    /// The successor.
    pub group: String,
    /// The successor's electorate epoch.
    pub epoch: u64,
    /// The predecessor dropped.
    pub predecessor: String,
    /// **An operator's assertion, outside the protocol's guarantee**: the predecessor's majority is gone for good, so
    /// it will never retire anywhere. Without it a drop is admitted only for a predecessor whose verified retirement
    /// to *another* successor is visible (it cannot retire into this group any more). A forced drop of a predecessor
    /// whose majority is in fact alive, and which later retires into this group, is the race §8.3 names: values can
    /// be lost — detected by the carry's conflict tripwire, never prevented. Audited at the gateway.
    pub force: bool,
}

impl PredecessorDrop {
    #[cfg(feature = "consensus")]
    pub(crate) fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    #[cfg(feature = "consensus")]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// Why a declaration, a retirement, a completion or a drop did not take effect.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElectorateError {
    /// A group name is empty or contains `/`, or a group names itself as its predecessor or successor.
    InvalidGroup,
    /// This node is not in the roster it would declare (a genesis is decided only by every member it names), or not
    /// a member of the electorate it would retire or complete.
    NotAMember,
    /// **Membership is fixed at genesis**: the group has an electorate, and the roster (or the default marking, or
    /// the predecessors) now differs from it. Nothing changes an adopted electorate; changing who decides is a
    /// migration (`docs/design/consensus-electorate.md` §8.3).
    MembershipFrozen {
        /// The group.
        group: String,
    },
    /// The group is retired to `successor`; it decides nothing again.
    Retired {
        /// The successor its retirement named.
        successor: String,
    },
    /// Nothing was decided: no quorum answered in time, another proposal won, a record has not arrived here yet, or
    /// a carry is still in progress. The reason says which. [`GossipAgent::declare_electorate`],
    /// [`GossipAgent::retire_electorate`] and [`GossipAgent::complete_migration`] retry this with a bounded backoff
    /// before returning it.
    NotDecided(String),
    /// Refused: a precondition no retry can change — a door refusal, a second default while one is held, a
    /// superseding genesis that keeps no majority, a successor that does not name the retiree, a predecessor retired
    /// elsewhere, a retirement while a carry into the group is incomplete, or this node cannot name the genesis it
    /// would supersede. The reason says which. Not retried.
    Refused(String),
    /// This node would be a member and has no `[persistence]`: its promises and acceptances would not survive a
    /// restart, and a member restarted empty can let two values be decided (`docs/design/consensus-electorate.md`
    /// §8.5, PR #604). Refused at every door — declaration, adoption, the genesis proposal and the genesis answer —
    /// and counted on [`crate::electorate_persistence_refused`]. `409 persistence_required` at the gateway. Not retried.
    PersistenceRequired {
        /// The group.
        group: String,
    },
}

impl std::fmt::Display for ElectorateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGroup => write!(f, "a group must be a non-empty name without '/', and not its own predecessor or successor"),
            Self::NotAMember => write!(f, "this node is not in the roster it would declare, or not a member of the electorate it would retire or complete"),
            Self::MembershipFrozen { group } => write!(
                f,
                "electorate group {group}'s membership is fixed at genesis: the roster differs from its electorate — \
                 changing who decides is a migration: declare a successor naming {group}, retire {group} into it, complete the carry"
            ),
            Self::Retired { successor } => write!(f, "the electorate group is retired; its successor is {successor}"),
            Self::NotDecided(why) => write!(f, "the electorate decision was not decided: {why}"),
            Self::Refused(why) => write!(f, "the electorate decision was refused: {why}"),
            Self::PersistenceRequired { group } => write!(
                f, "electorate group {group}: {PERSISTENCE_REQUIRED_DETAIL}"),
        }
    }
}

impl std::error::Error for ElectorateError {}

/// `electorate/{group}/{epoch}` — a genesis attempt.
#[cfg(feature = "consensus")]
pub(crate) fn genesis_slot(group: &str, epoch: u64) -> String {
    format!("{ELECTORATE_SLOT_PREFIX}{group}/{epoch}")
}

/// `(group, epoch)` for a genesis slot.
#[cfg(feature = "consensus")]
pub(crate) fn parse_genesis_slot(slot: &str) -> Option<(&str, u64)> {
    let rest = slot.strip_prefix(ELECTORATE_SLOT_PREFIX)?;
    let (group, epoch) = rest.split_once('/')?;
    if group.is_empty() || epoch.contains('/') { return None; }
    Some((group, epoch.parse().ok()?))
}

/// The certificate key for a genesis at `(group, epoch)`.
#[cfg(feature = "consensus")]
pub(crate) fn cert_key(group: &str, epoch: u64) -> String {
    format!("{ELECTORATE_CERT_PREFIX}{group}/{epoch}")
}

/// `electorate/{group}/retire`.
#[cfg(feature = "consensus")]
pub(crate) fn retire_slot(group: &str) -> String {
    format!("{ELECTORATE_SLOT_PREFIX}{group}/{RETIRE}")
}

/// The group a retirement slot retires.
#[cfg(feature = "consensus")]
pub(crate) fn parse_retire_slot(slot: &str) -> Option<&str> {
    let rest = slot.strip_prefix(ELECTORATE_SLOT_PREFIX)?;
    let (group, last) = rest.split_once('/')?;
    (!group.is_empty() && last == RETIRE).then_some(group)
}

/// The certificate key for `group`'s retirement.
#[cfg(feature = "consensus")]
pub(crate) fn retire_cert_key(group: &str) -> String {
    format!("{ELECTORATE_CERT_PREFIX}{group}/{RETIRE}")
}

/// `electorate/{group}/carried/{predecessor}` — the completion of `group`'s carry from `predecessor`.
#[cfg(feature = "consensus")]
pub(crate) fn carried_slot(group: &str, predecessor: &str) -> String {
    format!("{ELECTORATE_SLOT_PREFIX}{group}/{CARRIED}/{predecessor}")
}

/// `(group, predecessor)` for a completion slot.
#[cfg(feature = "consensus")]
pub(crate) fn parse_carried_slot(slot: &str) -> Option<(&str, &str)> {
    let rest = slot.strip_prefix(ELECTORATE_SLOT_PREFIX)?;
    let (group, rest) = rest.split_once('/')?;
    let (mid, pred) = rest.split_once('/')?;
    (!group.is_empty() && mid == CARRIED && valid_group(pred)).then_some((group, pred))
}

/// The certificate key for `group`'s completion of its carry from `predecessor`.
#[cfg(feature = "consensus")]
pub(crate) fn carried_cert_key(group: &str, predecessor: &str) -> String {
    format!("{ELECTORATE_CERT_PREFIX}{group}/{CARRIED}/{predecessor}")
}

/// `electorate/{group}/drop/{predecessor}` — `group` dropping `predecessor`.
#[cfg(feature = "consensus")]
pub(crate) fn drop_slot(group: &str, predecessor: &str) -> String {
    format!("{ELECTORATE_SLOT_PREFIX}{group}/{DROP}/{predecessor}")
}

/// `(group, predecessor)` for a drop slot.
#[cfg(feature = "consensus")]
pub(crate) fn parse_drop_slot(slot: &str) -> Option<(&str, &str)> {
    let rest = slot.strip_prefix(ELECTORATE_SLOT_PREFIX)?;
    let (group, rest) = rest.split_once('/')?;
    let (mid, pred) = rest.split_once('/')?;
    (!group.is_empty() && mid == DROP && valid_group(pred)).then_some((group, pred))
}

/// The certificate key for `group`'s drop of `predecessor`.
#[cfg(feature = "consensus")]
pub(crate) fn drop_cert_key(group: &str, predecessor: &str) -> String {
    format!("{ELECTORATE_CERT_PREFIX}{group}/{DROP}/{predecessor}")
}

/// Whether `group` is a usable group name.
#[cfg(feature = "consensus")]
pub(crate) fn valid_group(group: &str) -> bool {
    !group.is_empty() && !group.contains('/')
}

/// **The genesis rule**, pure: `next` is a well-formed genesis of `group` — a non-empty sorted member set, epoch
/// 1 with nothing superseded, or a later epoch superseding the one before it with a strict majority of its
/// members.
#[cfg(feature = "consensus")]
pub(crate) fn check_genesis(group: &str, next: &ElectorateDecl) -> Result<(), ElectorateError> {
    if !valid_group(group) || next.group != group { return Err(ElectorateError::InvalidGroup); }
    if next.members.is_empty() { return Err(ElectorateError::NotAMember); }
    if next.members.windows(2).any(|w| w[0].to_string() >= w[1].to_string()) {
        return Err(ElectorateError::Refused("the member set is not sorted and unique".into()));
    }
    if next.predecessors.iter().any(|p| !valid_group(p) || p == group) {
        return Err(ElectorateError::InvalidGroup);
    }
    match (next.epoch, &next.supersedes) {
        (0, _) => Err(ElectorateError::Refused("a genesis starts at epoch 1".into())),
        (1, None) => Ok(()),
        (1, Some(_)) => Err(ElectorateError::Refused("epoch 1 supersedes nothing".into())),
        (_, None) => Err(ElectorateError::Refused(format!("genesis epoch {} names no genesis it supersedes", next.epoch))),
        (e, Some(s)) => {
            if s.epoch + 1 != e {
                return Err(ElectorateError::Refused(format!("genesis epoch {e} supersedes epoch {}, not the one before it", s.epoch)));
            }
            let kept = s.members.iter().filter(|m| next.contains(m)).count();
            if kept * 2 <= s.members.len() {
                return Err(ElectorateError::Refused(format!(
                    "a superseding genesis names a strict majority of the one it supersedes: {kept} of {}", s.members.len())));
            }
            Ok(())
        }
    }
}

/// A member list in the electorate's canonical order: sorted by the node's address, without duplicates.
#[cfg(feature = "consensus")]
pub(crate) fn sorted_members(mut members: Vec<NodeId>) -> Vec<NodeId> {
    members.sort_by_key(ToString::to_string);
    members.dedup();
    members
}

/// Seeds each group's gate key with the highest genesis attempt this node's acceptor memory touched (at start,
/// after the memory is recovered), so the check for an electorate in formation is a lookup, not a walk.
#[cfg(feature = "consensus")]
pub(crate) fn prewarm_genesis_touched(ctx: &TaskCtx) {
    for (slot, s) in ctx.consensus_accepted.pin().iter() {
        let Some((group, epoch)) = parse_genesis_slot(slot) else { continue };
        if s.promised == 0 && s.accepted.is_none() { continue; }
        ctx.electorate_fences.pin().compute(Arc::from(group), |cur| {
            let f = cur.map(|(_, v)| *v).unwrap_or_default();
            if f.touched >= epoch { papaya::Operation::Abort(()) }
            else { papaya::Operation::Insert(crate::consensus::FenceState { touched: epoch, ..f }) }
        });
    }
}

/// The verified electorate this node holds for `group`, without refreshing it.
#[cfg(feature = "consensus")]
pub(crate) fn cached(ctx: &TaskCtx, group: &str) -> Option<Arc<ElectorateDecl>> {
    ctx.electorates.pin().get(group).cloned()
}

/// Rescans `consensus/committed/electorate/` when it changed since the last scan (a prefix subscription's
/// generation), recording which groups have records, the highest genesis epoch committed per group, which groups
/// have a retirement record and which completions — so a proposal does not walk every `consensus/` key (round 2,
/// finding 5).
#[cfg(feature = "consensus")]
pub(crate) fn refresh_records(ctx: &TaskCtx) {
    use std::sync::atomic::Ordering::{Acquire, Release};
    let prefix = format!("{}{ELECTORATE_SLOT_PREFIX}", crate::consensus::consensus_ns::COMMITTED);
    let rx = ctx.electorate_records_watch.get_or_init(|| {
        mycelium_core::ops::kv_subscribe_prefix(ctx, Arc::from(prefix.as_str()))
    });
    let generation = *rx.borrow();
    // Marked scanned only **after** the scan: a caller that finds the generation already marked knows a scan of it
    // completed, so it never reads the caches while another thread is still filling them (round 4: a swap before the
    // scan let a concurrent caller return early and read a stale epoch). Two callers may both scan; it is idempotent.
    if ctx.electorate_records_scanned.load(Acquire) != generation {
        for (k, _) in crate::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
            let Some((g, last)) = k.strip_prefix(prefix.as_str()).and_then(|r| r.split_once('/')) else { continue };
            let group: Arc<str> = Arc::from(g);
            ctx.electorate_groups_known.pin().insert(Arc::clone(&group));
            if last == RETIRE {
                ctx.electorate_retire_known.pin().insert(group);
            } else if let Some(pred) = last.strip_prefix(CARRIED).and_then(|r| r.strip_prefix('/')) {
                if valid_group(pred) {
                    ctx.electorate_carried_known.pin().insert(Arc::from(migration_key(g, pred).as_str()));
                }
            } else if let Some(pred) = last.strip_prefix(DROP).and_then(|r| r.strip_prefix('/')) {
                if valid_group(pred) {
                    ctx.electorate_dropped_known.pin().insert(Arc::from(migration_key(g, pred).as_str()));
                }
            } else if let Ok(epoch) = last.parse::<u64>() {
                ctx.electorate_epochs_known.pin().compute(group, |cur| match cur {
                    Some((_, e)) if *e >= epoch => papaya::Operation::Abort(()),
                    _ => papaya::Operation::Insert(epoch),
                });
            }
        }
        ctx.electorate_records_scanned.store(generation, Release);
    }
}

/// Every group that has a committed electorate record this node can see (verified or not — the caller
/// verifies through [`view`]). Without `consensus` there are none.
pub(crate) fn groups_with_records(ctx: &TaskCtx) -> Vec<String> {
    #[cfg(feature = "consensus")]
    {
        refresh_records(ctx);
        let mut groups: Vec<String> = ctx.electorate_groups_known.pin().iter().map(|g| g.to_string()).collect();
        groups.sort();
        groups
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = ctx;
        Vec::new()
    }
}

/// The slots of the cluster-scoped exclusive verbs — `lock/`, `consistent/`, `capauthz/`, a commitment award
/// (`cn/{requirement}/award`) and a consumer-group log claim (`clog/{stream}/{group}/claim`). They are decided in the fleet's electorate group (or the whole cluster
/// when none is marked) and **nowhere else**: a group-scoped proposal for one on any other group is refused,
/// so two groups never decide the same `consensus/committed/{slot}` (round 2, finding 2c).
#[cfg(feature = "consensus")]
pub fn is_fleet_exclusive_slot(slot: &str) -> bool {
    ["lock/", "consistent/", "capauthz/"].iter().any(|f| slot.starts_with(f))
        || (slot.starts_with("cn/") && slot.ends_with("/award"))
        || (slot.starts_with("clog/") && slot.ends_with("/claim"))
}

/// The group a slot names itself, when its family does: `leader/{group}` and `electorate/{group}/…` are
/// decided only by that group — and never carried from a predecessor.
#[cfg(feature = "consensus")]
pub(crate) fn slot_names_group(slot: &str) -> Option<&str> {
    if let Some(g) = slot.strip_prefix("leader/") { return Some(g); }
    slot.strip_prefix(ELECTORATE_SLOT_PREFIX).and_then(|r| r.split_once('/')).map(|(g, _)| g)
}

/// The key a migration `(group, predecessor)` is filed under in the per-node caches: `{group}\0{predecessor}`.
#[cfg(feature = "consensus")]
pub(crate) fn migration_key(group: &str, predecessor: &str) -> String {
    format!("{group}\u{0}{predecessor}")
}

/// The verified electorate for `group` — refreshed from the committed records. `None` when the group is
/// not an electorate group (or nothing about it verifies). Without `consensus` there are none.
pub(crate) fn view(ctx: &Arc<TaskCtx>, group: &str) -> Option<Arc<ElectorateDecl>> {
    #[cfg(feature = "consensus")]
    {
        crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None).electorate_view(group)
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = (ctx, group);
        None
    }
}

/// Every verified electorate this node can see.
pub(crate) fn all_views(ctx: &Arc<TaskCtx>) -> Vec<Arc<ElectorateDecl>> {
    groups_with_records(ctx).iter().filter_map(|g| view(ctx, g)).collect()
}

/// `group`'s verified retirement, if it is retired. Without `consensus` there is none.
pub(crate) fn retirement(ctx: &Arc<TaskCtx>, group: &str) -> Option<Arc<Retirement>> {
    #[cfg(feature = "consensus")]
    {
        crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None).retirement_view(group)
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = (ctx, group);
        None
    }
}

/// The verified completion of `group`'s carry from `predecessor`, if decided. Without `consensus` there is none.
pub(crate) fn completion(ctx: &Arc<TaskCtx>, group: &str, predecessor: &str) -> Option<Arc<Completion>> {
    #[cfg(feature = "consensus")]
    {
        crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None).completion_view(group, predecessor)
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = (ctx, group, predecessor);
        None
    }
}

#[cfg(feature = "consensus")]
fn refused_key(slot: &str, digest: &[u8; 32]) -> Arc<str> {
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Arc::from(format!("{slot}#{tag}"))
}

/// Whether this exact record (and certificate) was already refused — it is not verified again.
#[cfg(feature = "consensus")]
pub(crate) fn already_refused(ctx: &TaskCtx, slot: &str, digest: &[u8; 32]) -> bool {
    ctx.electorate_refused.pin().contains(&refused_key(slot, digest))
}

/// Counts a refused record once per distinct `(slot, digest)` and warns once.
#[cfg(feature = "consensus")]
pub(crate) fn note_refused(ctx: &TaskCtx, slot: &str, digest: &[u8; 32], why: &str) {
    let key = refused_key(slot, digest);
    let seen = ctx.electorate_refused.pin();
    if seen.contains(&key) || seen.len() >= 4096 {
        return;
    }
    seen.insert(key);
    RECORDS_REFUSED.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_electorate_records_refused_total").increment(1);
    tracing::warn!(slot, why, "electorate: a committed record was not adopted — it did not come through consensus \
        (see mycelium::electorate_records_refused)");
}

/// Whether `slot` is safety-sensitive under `config`: flagged, or in a [`SAFETY_SLOT_FAMILIES`] family.
#[cfg(feature = "consensus")]
pub(crate) fn is_safety_sensitive(config: &crate::consensus::ConsensusConfig, slot: &str) -> bool {
    config.safety_sensitive || SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f)) || is_fleet_exclusive_slot(slot)
}

/// Whether `d` **holds** the fleet's default: it declared it at genesis, or a predecessor it names retired to it
/// passing the default (`Retirement::passes_default` — or the verified completion's copy of it, which stands as
/// proof once the predecessor's records are gone) — the default follows the retirement whatever the successor's
/// own flag says. One level, never a chain: a successor's retirement passes what it holds, so a chain of migrations
/// needs only the last retirement.
#[cfg(feature = "consensus")]
pub(crate) fn holds_default(ctx: &Arc<TaskCtx>, d: &ElectorateDecl) -> bool {
    d.exclusive_default
        || d.predecessors.iter().any(|p| retirement(ctx, p).is_some_and(|r| r.successor == d.group && r.passes_default)
            || completion(ctx, &d.group, p).is_some_and(|c| c.passes_default))
}

/// The verified drop of `predecessor` by `group`, if decided. Without `consensus` there is none.
pub(crate) fn dropped(ctx: &Arc<TaskCtx>, group: &str, predecessor: &str) -> Option<Arc<PredecessorDrop>> {
    #[cfg(feature = "consensus")]
    {
        crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None).drop_view(group, predecessor)
    }
    #[cfg(not(feature = "consensus"))]
    {
        let _ = (ctx, group, predecessor);
        None
    }
}

/// **The live default**, as the fleet's records say (no local setting): the one electorate group that holds the
/// default and is not retired, or none when no group ever declared one. `Err` — fail closed — while a holder retired
/// to a successor this node cannot see yet (gossip), or when two live groups hold it (two default declarations raced
/// within gossip lag; the escape is to merge them: declare one successor naming both, retire both into it, complete).
#[cfg(feature = "consensus")]
pub(crate) fn live_default(ctx: &Arc<TaskCtx>) -> Result<Option<String>, String> {
    let views = all_views(ctx);
    let mut live = Vec::new();
    for d in views.iter().filter(|d| holds_default(ctx, d)) {
        match retirement(ctx, &d.group) {
            None => live.push(d.group.clone()),
            Some(r) if !r.passes_default => return Err(format!(
                "the fleet's exclusive electorate {} retired to {} without passing the default", d.group, r.successor)),
            Some(r) => if !views.iter().any(|s| s.group == r.successor) {
                return Err(format!(
                    "the fleet's exclusive electorate {} retired to {}, which is not visible here yet", d.group, r.successor));
            },
        }
    }
    if live.len() > 1 {
        return Err(format!("more than one live electorate group holds the fleet's exclusive default: {} — merge them: \
                            declare one successor naming every one of them, retire each into it, complete the carry", live.join(", ")));
    }
    Ok(live.into_iter().next())
}

/// **Where the fleet's exclusive verbs decide** (finding 2 of #601's review; round 5): the fleet's default
/// ([`live_default`]) — a fleet record, so every node reads the same answer once gossip has carried it — or the whole
/// cluster when no group ever declared one. `consensus_electorate` may only restate it: a local setting that names
/// another group, or names one when the fleet names none, is refused by name.
#[cfg(feature = "consensus")]
pub(crate) fn exclusive_electorate(ctx: &Arc<TaskCtx>) -> Result<Option<String>, String> {
    let fleet = live_default(ctx)?;
    match (ctx.config.consensus_electorate.as_deref(), fleet) {
        (None, fleet) => Ok(fleet),
        (Some(local), Some(fleet)) if local == fleet => Ok(Some(fleet)),
        (Some(local), Some(fleet)) => Err(format!(
            "this node's consensus_electorate names {local}, but the fleet's exclusive default is {fleet}")),
        (Some(local), None) => Err(format!(
            "this node's consensus_electorate names {local}, but no electorate group is marked the fleet's exclusive \
             default — declare it with exclusive_default, so every node decides in the same place")),
    }
}

/// What a declaration did.
#[cfg(feature = "consensus")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Declared {
    /// The electorate is as the roster says: nothing to decide.
    Unchanged,
    /// This node's genesis was decided.
    Decided,
    /// Another member's genesis was decided — this declaration completed or lost to it.
    Adopted,
}

/// The configuration one attempt at an electorate decision runs under: three ballots, two seconds each.
#[cfg(feature = "consensus")]
pub(crate) fn decision_config() -> crate::consensus::ConsensusConfig {
    crate::consensus::ConsensusConfig {
        safety_sensitive: true,
        phase1_timeout: std::time::Duration::from_secs(2),
        ..crate::consensus::ConsensusConfig::default()
    }
}

/// How many times [`retrying`] tries, and the longest pause between tries.
#[cfg(feature = "consensus")]
const DECIDE_ATTEMPTS: u32 = 8;
#[cfg(feature = "consensus")]
const DECIDE_BACKOFF_CAP_MS: u64 = 4_000;

/// Runs `attempt` until it is decided or refused, retrying `NotDecided` with a bounded exponential backoff —
/// 250 ms doubling to [`DECIDE_BACKOFF_CAP_MS`], [`DECIDE_ATTEMPTS`] tries (round 4 of #601's review, finding 8:
/// a genesis needs every member's answer, and a mesh still converging, or sparse under `max_peers`, can miss
/// one round). Any other outcome returns at once.
#[cfg(feature = "consensus")]
pub(crate) async fn retrying<T, F, Fut>(mut attempt: F) -> Result<T, ElectorateError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ElectorateError>>,
{
    let mut last = String::new();
    for i in 0..DECIDE_ATTEMPTS {
        match attempt().await {
            Err(ElectorateError::NotDecided(why)) => last = why,
            other => return other,
        }
        if i + 1 < DECIDE_ATTEMPTS {
            let pause = (250u64 << i.min(5)).min(DECIDE_BACKOFF_CAP_MS);
            mycelium_core::sim_seam::sleep_ms("electorate.decide_retry", pause).await;
        }
    }
    Err(ElectorateError::NotDecided(last))
}

/// **Declare** `group`'s electorate from the roster this node sees — one attempt. See
/// [`GossipAgent::declare_electorate`].
///
/// - The group has an electorate: if the roster, the default marking and the predecessors match it, nothing to
///   decide; otherwise [`ElectorateError::MembershipFrozen`] — changing who decides is a migration.
/// - It has none: a genesis naming the roster, decided by every member named, naming `predecessors` (each an
///   electorate this node can see, not retired to another group). If this node holds a pending genesis (accepted,
///   never adopted) naming another set, the new genesis **supersedes** it at the next epoch; one naming this set is
///   completed at its own epoch.
#[cfg(feature = "consensus")]
pub(crate) async fn declare_outcome(
    ctx: &Arc<TaskCtx>, group: &str, exclusive_default: bool, predecessors: &BTreeSet<String>,
) -> Result<(ElectorateDecl, Declared), ElectorateError> {
    use crate::consensus::ConsensusResult;
    if !valid_group(group) || predecessors.iter().any(|p| !valid_group(p) || p == group) {
        return Err(ElectorateError::InvalidGroup);
    }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let roster = sorted_members(crate::agent::helpers::group_members_ctx(ctx, group));
    if let Some(r) = engine.retirement_view(group) {
        return Err(ElectorateError::Retired { successor: r.successor.clone() });
    }
    if let Some(cur) = engine.electorate_view(group) {
        if cur.members == roster && cur.exclusive_default == exclusive_default && cur.predecessors == *predecessors {
            return Ok(((*cur).clone(), Declared::Unchanged));
        }
        return Err(ElectorateError::MembershipFrozen { group: group.to_string() });
    }
    if !roster.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    // A member with no `[persistence]` declares nothing (`acceptor_guard`, detection 1): refused by name before anything is sent.
    if ctx.config.persistence.is_none() {
        crate::agent::acceptor_guard::note_persistence_refused(ctx, group, 0, "declare");
        return Err(ElectorateError::PersistenceRequired { group: group.to_string() });
    }
    // Successor first (design record §8.3): each predecessor is an electorate this node can see, not yet retired to
    // another group. Not visible yet is gossip — retried; retired elsewhere is a precondition no retry changes.
    for p in predecessors {
        if engine.electorate_view(p).is_none() {
            return Err(ElectorateError::NotDecided(format!("predecessor {p} has no electorate visible here yet")));
        }
        if let Some(r) = engine.retirement_view(p) && r.successor != group {
            return Err(ElectorateError::Refused(format!("predecessor {p} is already retired to {}", r.successor)));
        }
    }
    // One live default: a genesis may declare it only while no group holds it (round 5, D1/D2); it moves by
    // retirement, so a successor of the default's holder does not declare it.
    if exclusive_default {
        match live_default(ctx) {
            Ok(None) => {}
            Ok(Some(g)) => return Err(ElectorateError::Refused(format!(
                "{g} already holds the fleet's exclusive default; the default moves only by retirement — declare {group} \
                 naming {g} as a predecessor, retire {g} into it, complete the carry"))),
            Err(e) => return Err(ElectorateError::Refused(e)),
        }
    }
    let base = ElectorateDecl {
        group: group.to_string(), epoch: 1, members: roster, exclusive_default, supersedes: None,
        predecessors: predecessors.clone(),
    };
    let (touched, pending) = engine.pending_genesis(group);
    let same_set = |t: &ElectorateDecl| t.members == base.members && t.exclusive_default == base.exclusive_default
        && t.predecessors == base.predecessors;
    let next = match pending {
        // The pending genesis names this set, at the highest epoch this node touched: complete it.
        Some((t, _)) if t.epoch >= touched && same_set(&t) => t,
        // It names another set (or this node promised the epoch after it): supersede it at the next epoch
        // (round 4, finding 3).
        Some((t, held)) if t.epoch + 1 >= touched => ElectorateDecl {
            epoch: t.epoch + 1,
            supersedes: Some(Superseded {
                epoch: t.epoch, digest: crate::consensus::value_digest(&t.encode()), envelope: held, members: t.members.clone(),
            }),
            ..base
        },
        None if touched <= 1 => base,
        _ => return Err(ElectorateError::Refused(format!(
            "this node promised genesis epoch {touched} of {group} without holding the genesis before it; \
             declare from a member that holds the pending genesis"))),
    };
    check_genesis(group, &next)?;
    let slot = genesis_slot(group, next.epoch);
    let result = engine.propose(
        crate::signal::SignalScope::Group(Arc::from(group)), Arc::from(slot.as_str()), next.encode(),
        next.members.len(), decision_config(), None,
    ).await;
    let after = engine.electorate_view(group);
    match result {
        ConsensusResult::Committed { .. } => match after {
            Some(v) if *v == next => Ok((next, Declared::Decided)),
            Some(v) => Ok(((*v).clone(), Declared::Adopted)),
            None => Err(ElectorateError::NotDecided(
                "committed, but its certificate did not reach the store — the record was not adopted yet".into())),
        },
        ConsensusResult::Superseded { .. } if after.is_some() =>
            Ok(((*after.unwrap_or_else(|| Arc::new(next.clone()))).clone(), Declared::Adopted)),
        ConsensusResult::NotAMember { .. } => Err(ElectorateError::NotAMember),
        // A door refusal is a precondition a retry does not change (round 5, Q3).
        ConsensusResult::ElectorateMismatch { detail, .. } => Err(ElectorateError::Refused(detail.to_string())),
        ConsensusResult::ElectorateNotGoverned { .. } => Err(ElectorateError::Refused("not an electorate".into())),
        other => Err(ElectorateError::NotDecided(format!("{other:?}"))),
    }
}

static RETIREMENTS_UNDECIDED: AtomicU64 = AtomicU64::new(0);

/// Retirement attempts that ended undecided after this node's acceptor had promised the retirement — so this node
/// (and any member that promised it) has closed the group, which decides nothing until a retirement completes it
/// (`retire_electorate` again, from any member). Counted and warned once per attempt (round 5, Q4).
pub fn electorate_retirements_undecided() -> u64 {
    RETIREMENTS_UNDECIDED.load(Ordering::Relaxed)
}

/// **Retire** `group` to `successor` — one attempt. See [`GossipAgent::retire_electorate`].
#[cfg(feature = "consensus")]
pub(crate) async fn retire_outcome(ctx: &Arc<TaskCtx>, group: &str, successor: &str) -> Result<Retirement, ElectorateError> {
    use crate::consensus::ConsensusResult;
    if !valid_group(group) || !valid_group(successor) || group == successor { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    // A retry that finds the retirement already decided reports it (round 5, Q3).
    if let Some(r) = engine.retirement_view(group) {
        return if r.successor == successor { Ok((*r).clone()) } else { Err(ElectorateError::Retired { successor: r.successor.clone() }) };
    }
    let Some(cur) = engine.electorate_view(group) else {
        return Err(ElectorateError::NotDecided(format!("{group} has no electorate visible here yet")));
    };
    if !cur.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    // Successor first: the successor is declared, naming this group, before this group retires into it. Not visible
    // yet is gossip — retried; visible and not naming it, or itself retired, is refused.
    match engine.electorate_view(successor) {
        None => return Err(ElectorateError::NotDecided(format!(
            "successor {successor} has no electorate visible here yet — declare it first, naming {group} among its predecessors"))),
        Some(s) if !s.predecessors.contains(group) => return Err(ElectorateError::Refused(format!(
            "successor {successor} does not name {group} among its predecessors"))),
        Some(_) if engine.retirement_view(successor).is_some() => return Err(ElectorateError::Refused(format!(
            "successor {successor} is itself retired"))),
        Some(_) => {}
    }
    // A group retires only once every predecessor it names is complete or dropped — a retirement not visible here
    // counts as incomplete: its successor would otherwise enumerate this group alone and miss what a predecessor still
    // holds (design record §8.3, chains). Checked again by every acceptor, at the prepare, before it closes.
    if let Err(why) = engine.retirable(&cur) { return Err(ElectorateError::Refused(why)); }
    // The default follows the retirement: passed exactly when this group holds it. Uniqueness is not required — two
    // live holders (declared within gossip lag) are merged exactly this way, each retiring into one successor.
    let passes_default = holds_default(ctx, &cur);
    let r = Retirement { group: group.to_string(), epoch: cur.epoch, successor: successor.to_string(), passes_default };
    let slot = retire_slot(group);
    let result = engine.propose(
        crate::signal::SignalScope::Group(Arc::from(group)), Arc::from(slot.as_str()), r.encode(),
        cur.quorum(), decision_config(), None,
    ).await;
    let after = engine.retirement_view(group);
    let undecided = |why: String| {
        if ctx.consensus_accepted.pin().get(slot.as_str()).is_some_and(|s| s.promised > 0) {
            RETIREMENTS_UNDECIDED.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_electorate_retirements_undecided_total").increment(1);
            tracing::warn!(group, successor, why, "electorate: a retirement attempt ended undecided after this node promised \
                it — the group is closed here and decides nothing until a retirement completes (retire again from any member)");
        }
        ElectorateError::NotDecided(why)
    };
    match (result, after) {
        (_, Some(done)) if done.successor == successor => Ok((*done).clone()),
        (_, Some(other)) => Err(ElectorateError::Retired { successor: other.successor.clone() }),
        (ConsensusResult::NotAMember { .. }, None) => Err(ElectorateError::NotAMember),
        (ConsensusResult::ElectorateMismatch { detail, .. }, None) => Err(ElectorateError::Refused(detail.to_string())),
        (ConsensusResult::Committed { .. }, None) => Err(undecided(
            "committed, but its certificate did not reach the store — the retirement was not adopted yet".into())),
        (other, None) => Err(undecided(format!("{other:?}"))),
    }
}

/// **Drop `predecessor` from `group`** — one attempt. See [`GossipAgent::drop_predecessor`].
#[cfg(feature = "consensus")]
pub(crate) async fn drop_outcome(ctx: &Arc<TaskCtx>, group: &str, predecessor: &str, force: bool) -> Result<PredecessorDrop, ElectorateError> {
    use crate::consensus::ConsensusResult;
    if !valid_group(group) || !valid_group(predecessor) || group == predecessor { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    if let Some(d) = engine.drop_view(group, predecessor) { return Ok((*d).clone()); }
    let Some(cur) = engine.electorate_view(group) else {
        return Err(ElectorateError::NotDecided(format!("{group} has no electorate visible here yet")));
    };
    if !cur.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    if !cur.predecessors.contains(predecessor) {
        return Err(ElectorateError::Refused(format!("{predecessor} is not a predecessor of {group}")));
    }
    let d = PredecessorDrop { group: group.to_string(), epoch: cur.epoch, predecessor: predecessor.to_string(), force };
    if let Err(why) = engine.drop_acceptable(&cur, predecessor, Some(&d)) { return Err(ElectorateError::Refused(why)); }
    let slot = drop_slot(group, predecessor);
    let result = engine.propose(
        crate::signal::SignalScope::Group(Arc::from(group)), Arc::from(slot.as_str()), d.encode(),
        cur.quorum(), decision_config(), None,
    ).await;
    match (result, engine.drop_view(group, predecessor)) {
        (_, Some(done)) => Ok((*done).clone()),
        (ConsensusResult::NotAMember { .. }, None) => Err(ElectorateError::NotAMember),
        (ConsensusResult::ElectorateMismatch { detail, .. }, None) => Err(ElectorateError::Refused(detail.to_string())),
        (other, None) => Err(ElectorateError::NotDecided(format!("{other:?}"))),
    }
}

/// A consensus engine over `ctx`, for tests that drive the acceptor's door directly.
#[cfg(all(test, feature = "consensus"))]
pub(crate) fn engine(ctx: &Arc<TaskCtx>) -> crate::consensus::ConsensusEngine {
    crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None)
}

impl GossipAgent {
    /// **Declare `group`'s electorate** — a governance act, decided by consensus (P2,
    /// `docs/design/consensus-electorate.md` §8). Reads the group's roster as this node sees it and proposes
    /// a **genesis** naming it, decided by **every** member named, each checking the set is the roster it sees.
    /// `exclusive_default` declares the fleet's default electorate — refused while any group holds it.
    ///
    /// **Membership is fixed at genesis**: on a group that already has an electorate this decides nothing — it returns
    /// the electorate when the roster still matches, and [`ElectorateError::MembershipFrozen`] when it does not.
    /// Changing who decides is a **migration** (design record §8.3): declare the successor naming this group
    /// ([`declare_successor_electorate`](Self::declare_successor_electorate)), retire this group into it
    /// ([`retire_electorate`](Self::retire_electorate)), complete the carry
    /// ([`complete_migration`](Self::complete_migration)). A dead member lowers the group's fault tolerance; a lost
    /// majority stops the group. A pending genesis that can no longer complete (a member it names is gone) is
    /// superseded by declaring the live roster from a member that holds it.
    ///
    /// Retries an undecided attempt with a bounded backoff (eight tries, 250 ms doubling to 4 s); a refusal no retry
    /// changes returns at once ([`ElectorateError::Refused`]). Requires the consensus listener on every member.
    #[cfg(feature = "consensus")]
    pub async fn declare_electorate(&self, group: &str, exclusive_default: bool) -> Result<ElectorateDecl, ElectorateError> {
        let none = BTreeSet::new();
        retrying(|| async { declare_outcome(&self.task_ctx, group, exclusive_default, &none).await.map(|(d, _)| d) }).await
    }

    /// **Declare a successor electorate** — the first step of a migration (design record §8.3): a genesis of
    /// `group`, from its roster, naming the groups it succeeds (two or more is a merge). Each predecessor must be an
    /// electorate this node can see and not retired elsewhere. The successor **decides nothing** — no slot, at the
    /// proposer or any acceptor — until every predecessor has retired to it
    /// ([`retire_electorate`](Self::retire_electorate)) and that retirement is visible there; then only the slots
    /// it carries, until the carry is complete ([`complete_migration`](Self::complete_migration)). When a
    /// predecessor holds the fleet's default, the successor holds it from that retirement on.
    #[cfg(feature = "consensus")]
    pub async fn declare_successor_electorate(&self, group: &str, predecessors: &[&str]) -> Result<ElectorateDecl, ElectorateError> {
        let preds: BTreeSet<String> = predecessors.iter().map(|p| p.to_string()).collect();
        retrying(|| async { declare_outcome(&self.task_ctx, group, false, &preds).await.map(|(d, _)| d) }).await
    }

    /// **Retire `group` to `successor`** — the second step of a migration (design record §8.3). `successor` must
    /// already be declared, naming `group` among its predecessors (not yet visible here: retried; visible and not
    /// naming it: refused). A decision of a strict majority of `group`'s electorate, on `electorate/{group}/retire`.
    /// Every member **closes** `group`, durably, when it promises the retirement, so once it is chosen `group`
    /// decides nothing again; its members then answer only the successor's enumeration. When `group` holds the
    /// fleet's default, the retirement passes it to `successor`. A group whose own carry from a predecessor is
    /// incomplete is refused: complete it first.
    ///
    /// Needs a reachable strict majority of `group`. Retries an undecided attempt as `declare_electorate` does; a
    /// retry that finds the retirement decided reports it.
    #[cfg(feature = "consensus")]
    pub async fn retire_electorate(&self, group: &str, successor: &str) -> Result<Retirement, ElectorateError> {
        retrying(|| async { retire_outcome(&self.task_ctx, group, successor).await }).await
    }

    /// **Drop `predecessor` from `group`'s wait** — a decision of a strict majority of `group` on
    /// `electorate/{group}/drop/{predecessor}`, admitted at the door and at every acceptor only for a predecessor that
    /// **cannot retire into `group` any more**: its verified retirement to another successor is visible (a losing
    /// successor's way on). After it `group` no longer waits on `predecessor`. Nothing of `predecessor` is carried.
    ///
    /// `force` is **an operator's assertion outside the protocol's guarantee** — the predecessor's majority is gone
    /// for good (disaster-recovery territory): the drop is then admitted without a retirement elsewhere, and
    /// `predecessor`'s later retirement into `group` is refused by its acceptors where the drop is visible. If that
    /// majority is in fact alive and retires into `group` before the drop reaches it, the retirement stands, the drop
    /// is void, `group` carries after all — and any slot `group` decided meanwhile is a **detected conflict** on the
    /// carry (counted on the commit-conflict tripwire, named in the completion), not a prevented one (design record
    /// §8.3, §8.5). Refused without `force` while the predecessor may still retire.
    #[cfg(feature = "consensus")]
    pub async fn drop_predecessor(&self, group: &str, predecessor: &str, force: bool) -> Result<PredecessorDrop, ElectorateError> {
        retrying(|| async { drop_outcome(&self.task_ctx, group, predecessor, force).await }).await
    }

    /// **Complete `group`'s migration** — the third step, from any member of `group`, idempotent (design record
    /// §8.3): for each predecessor that has retired to `group`, **enumerate** the carry set `S` from a strict majority
    /// of its members, **carry** every slot in `S` with one ballot of `group`, and decide the **completion**
    /// `electorate/{group}/carried/{predecessor}`. After it `group` decides any slot without the predecessor, whose
    /// members may be removed. A restart before completion re-enumerates; a slot already carried is decided again
    /// with the value `group` holds (its own phase 1 finds it), which is safe and costs one ballot.
    ///
    /// Needs a reachable strict majority of each retired predecessor and of `group`. Returns what it found per
    /// predecessor; a carry that could not finish — a predecessor majority unreachable, a value known only by
    /// digest that no record here resolves — is `NotDecided`, naming the slot, and is retried with a bounded
    /// backoff before it is returned. One completion runs per node at a time.
    #[cfg(feature = "consensus")]
    pub async fn complete_migration(&self, group: &str) -> Result<Vec<MigrationState>, ElectorateError> {
        retrying(|| async { super::migration::complete_outcome(&self.task_ctx, group).await }).await
    }

    /// **Where `group`'s migration stands**, per predecessor it names, as this node sees it: waiting for the
    /// retirement, closed (retired, not yet enumerated here), carrying (`carried` of `total` slots, as far as this
    /// node's own completion run got), or complete.
    #[cfg(feature = "consensus")]
    pub fn migration_state(&self, group: &str) -> Vec<MigrationState> {
        super::migration::state(&self.task_ctx, group)
    }

    /// The verified electorate this node holds for `group`, if any.
    pub fn electorate(&self, group: &str) -> Option<ElectorateDecl> {
        view(&self.task_ctx, group).map(|d| (*d).clone())
    }

    /// `group`'s verified retirement, if it is retired.
    pub fn electorate_retirement(&self, group: &str) -> Option<Retirement> {
        retirement(&self.task_ctx, group).map(|r| (*r).clone())
    }

    /// The verified completion of `group`'s carry from `predecessor`, if decided.
    pub fn electorate_completion(&self, group: &str, predecessor: &str) -> Option<Completion> {
        completion(&self.task_ctx, group, predecessor).map(|c| (*c).clone())
    }

    /// The verified drop of `predecessor` by `group`, if decided.
    pub fn electorate_drop(&self, group: &str, predecessor: &str) -> Option<PredecessorDrop> {
        dropped(&self.task_ctx, group, predecessor).map(|d| (*d).clone())
    }
}

/// Where a successor's carry from one predecessor stands, as one node sees it ([`GossipAgent::migration_state`]).
#[cfg(feature = "consensus")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MigrationState {
    /// The predecessor.
    pub predecessor: String,
    /// Its phase (flattened in JSON: `"phase": "waiting" | "closed" | "carrying" | "complete"`, with `carried` /
    /// `total` or `slots` beside it).
    #[serde(flatten)]
    pub phase: MigrationPhase,
}

/// A migration's phase, per predecessor.
#[cfg(feature = "consensus")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MigrationPhase {
    /// The predecessor's retirement to this group is not visible here: the successor decides nothing.
    Waiting,
    /// Retired to this group; the carry set has not been enumerated by a completion run on this node.
    Closed,
    /// A completion run on this node enumerated `total` slots and has carried `carried` of them.
    Carrying {
        /// Slots carried so far.
        carried: u64,
        /// Slots in the carry set.
        total: u64,
    },
    /// The completion is decided and visible here: the predecessor is no longer consulted.
    Complete {
        /// Slots the carry set held.
        slots: u64,
        /// Slots whose carry found another value already decided above the carried one — detected conflicts, counted
        /// on the commit-conflict tripwire and named in the completion record.
        conflicts: u64,
    },
    /// The group dropped this predecessor (it never retired to it): no longer waited on, nothing carried.
    Dropped,
}

#[cfg(all(test, feature = "consensus"))]
mod tests {
    use super::*;

    fn n(port: u16) -> NodeId { NodeId::new("127.0.0.1", port).unwrap() }
    fn decl(epoch: u64, ports: &[u16]) -> ElectorateDecl {
        let members = sorted_members(ports.iter().map(|p| n(*p)).collect());
        ElectorateDecl { group: "e".into(), epoch, members, ..ElectorateDecl::default() }
    }
    fn superseding(prev: &ElectorateDecl, ports: &[u16]) -> ElectorateDecl {
        ElectorateDecl {
            supersedes: Some(Superseded {
                epoch: prev.epoch, digest: crate::consensus::value_digest(&prev.encode()), envelope: [0; 32], members: prev.members.clone(),
            }),
            ..decl(prev.epoch + 1, ports)
        }
    }

    /// A genesis is well formed: epoch 1 supersedes nothing; a later epoch supersedes the one before it with a
    /// strict majority of its members (round 4, finding 3).
    #[test]
    fn a_genesis_supersedes_only_the_epoch_before_it_with_a_majority_of_it() {
        let g1 = decl(1, &[1, 2, 3]);
        assert_eq!(check_genesis("e", &g1), Ok(()));
        assert_eq!(check_genesis("e", &superseding(&g1, &[1, 2])), Ok(()), "two of three, the dead member dropped");
        assert_eq!(check_genesis("e", &superseding(&g1, &[1, 2, 4, 5])), Ok(()), "a majority kept, members added");
        assert!(check_genesis("e", &superseding(&g1, &[1, 4, 5])).is_err(), "one of three is not a majority");
        assert!(check_genesis("e", &superseding(&decl(1, &[1, 2, 3, 4]), &[1, 2])).is_err(), "two of four is not a strict majority");
        assert!(check_genesis("e", &decl(2, &[1, 2])).is_err(), "epoch 2 must name what it supersedes");
        let mut skip = superseding(&g1, &[1, 2]);
        skip.epoch = 3;
        assert!(check_genesis("e", &skip).is_err(), "an epoch cannot be skipped");
        let mut both = g1.clone();
        both.supersedes = superseding(&g1, &[1, 2]).supersedes;
        assert!(check_genesis("e", &both).is_err(), "epoch 1 supersedes nothing");
        assert_eq!(check_genesis("a/b", &g1), Err(ElectorateError::InvalidGroup));
        let mut unsorted = decl(1, &[1, 2]);
        unsorted.members.reverse();
        assert!(check_genesis("e", &unsorted).is_err());
        let mut own = g1.clone();
        own.predecessors.insert("e".into());
        assert_eq!(check_genesis("e", &own), Err(ElectorateError::InvalidGroup), "a group does not succeed itself");
        let mut slash = g1.clone();
        slash.predecessors.insert("a/b".into());
        assert_eq!(check_genesis("e", &slash), Err(ElectorateError::InvalidGroup));
    }

    /// The digest commits to everything a record says — the superseded genesis and the predecessors included; a
    /// retirement and a completion round-trip, and a 2.32.0 genesis (no `predecessors` field) still decodes.
    #[test]
    fn the_digest_commits_to_what_is_superseded_and_succeeded() {
        let g1 = decl(1, &[1, 2, 3]);
        let g2 = superseding(&g1, &[1, 2]);
        let mut other = g2.clone();
        other.supersedes.as_mut().unwrap().members.pop();
        assert_ne!(g2.digest(), other.digest());
        assert_eq!(ElectorateDecl::decode(&g2.encode()), Some(g2));
        let mut succ = g1.clone();
        succ.predecessors.insert("old".into());
        assert_ne!(g1.digest(), succ.digest());
        let mut merge = g1.clone();
        merge.predecessors.extend(["a".to_string(), "b".to_string()]);
        let mut one = g1.clone();
        one.predecessors.insert("a".into());
        assert_ne!(merge.digest(), one.digest(), "the digest commits to every predecessor");
        assert_eq!(ElectorateDecl::decode(&merge.encode()), Some(merge));
        let r = Retirement { group: "old".into(), epoch: 1, successor: "new".into(), passes_default: true };
        assert_eq!(Retirement::decode(&r.encode()), Some(r.clone()));
        let c = Completion { group: "new".into(), epoch: 1, predecessor: "old".into(), predecessor_epoch: 1, passes_default: true, slots: 3, conflicts: 1, conflict_slots: vec!["lock/door".into()], digest: [7; 32] };
        assert_eq!(Completion::decode(&c.encode()), Some(c));
        let dr = PredecessorDrop { group: "new".into(), epoch: 1, predecessor: "other".into(), force: true };
        assert_eq!(PredecessorDrop::decode(&dr.encode()), Some(dr));
        // A 2.32.0 genesis record carries no `predecessors`: it reads as an electorate with none, and a genesis with
        // none is encoded in that layout, byte for byte.
        #[derive(serde::Serialize)]
        struct Old<'a> { group: &'a str, epoch: u64, members: &'a [NodeId], exclusive_default: bool, supersedes: Option<Superseded> }
        let old = mycelium_core::serde_fixint::to_vec(&Old {
            group: "e", epoch: 1, members: &g1.members, exclusive_default: false, supersedes: None,
        }).unwrap();
        assert_eq!(ElectorateDecl::decode(&old), Some(g1.clone()));
        assert_eq!(g1.encode().as_ref(), old.as_slice(), "a genesis without predecessors is the 2.32.0 bytes");
        assert_eq!(ElectorateDecl::decode(&old).map(|d| d.digest()), Some(g1.digest()), "and digests as before");
    }

    #[test]
    fn electorate_slots_round_trip() {
        assert_eq!(genesis_slot("g", 7), "electorate/g/7");
        assert_eq!(parse_genesis_slot("electorate/g/7"), Some(("g", 7)));
        assert_eq!(parse_genesis_slot("electorate/g/x"), None);
        assert_eq!(parse_genesis_slot("electorate/g/retire"), None);
        assert_eq!(parse_genesis_slot("electorate/g/carried/p"), None);
        assert_eq!(parse_genesis_slot("leader/g"), None);
        assert_eq!(slot_names_group("electorate/g/7"), Some("g"));
        assert_eq!(retire_slot("g"), "electorate/g/retire");
        assert_eq!(parse_retire_slot("electorate/g/retire"), Some("g"));
        assert_eq!(parse_retire_slot("electorate/g/7"), None);
        assert_eq!(parse_retire_slot("electorate/g/carried/p"), None);
        assert_eq!(slot_names_group("electorate/g/retire"), Some("g"));
        assert_eq!(carried_slot("g", "p"), "electorate/g/carried/p");
        assert_eq!(parse_carried_slot("electorate/g/carried/p"), Some(("g", "p")));
        assert_eq!(parse_carried_slot("electorate/g/carried/"), None);
        assert_eq!(parse_carried_slot("electorate/g/carried/a/b"), None);
        assert_eq!(parse_carried_slot("electorate/g/retire"), None);
        assert_eq!(slot_names_group("electorate/g/carried/p"), Some("g"));
        assert_eq!(drop_slot("g", "p"), "electorate/g/drop/p");
        assert_eq!(parse_drop_slot("electorate/g/drop/p"), Some(("g", "p")));
        assert_eq!(parse_drop_slot("electorate/g/carried/p"), None);
        assert_eq!(parse_carried_slot("electorate/g/drop/p"), None);
        assert_eq!(migration_key("g", "p"), "g\u{0}p");
    }

    #[test]
    fn the_exclusive_slot_families_are_safety_sensitive_by_definition() {
        let plain = crate::consensus::ConsensusConfig::default();
        for slot in ["lock/x", "leader/g", "consistent/k", "electorate/g/2", "electorate/g/retire", "electorate/g/carried/p"] {
            assert!(is_safety_sensitive(&plain, slot), "{slot}");
        }
        assert!(!is_safety_sensitive(&plain, "work/item-7"));
        let flagged = crate::consensus::ConsensusConfig { safety_sensitive: true, ..plain };
        assert!(is_safety_sensitive(&flagged, "work/item-7"));
    }
}
