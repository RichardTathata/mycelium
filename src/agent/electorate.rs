//! **Electorate groups** — post-360 plan row P2, `docs/design/consensus-electorate.md` §8.
//!
//! Discovery and the electorate are separate capabilities (D1): a group's roster is dynamic, and a
//! consensus electorate is a fixed set for the life of a decision. An **electorate group** is a group
//! whose electorate is pinned by **identity and epoch**: an [`ElectorateDecl`] `{ group, epoch, members }`
//! that is itself a **consensus decision** — the slot `electorate/{group}/{epoch}` — so the electorate is
//! named as a group and moved only through the group's own agreement:
//!
//! - **genesis** (epoch 1) is decided by **every** member it names, each of which checks the member set
//!   is the group roster it sees;
//! - a **step** (epoch `e` → `e + 1`) adds or removes **one** member and is decided by the epoch-`e`
//!   electorate (a strict majority of its members) — two concurrent steps are one slot, so at most one
//!   commits, and a non-member's step is refused at the proposer and at every acceptor;
//! - each commit carries a **certificate** — the deciding votes, signed — under
//!   `consensus/electorate-cert/{group}/{epoch}`, and a node adopts an epoch only when the record follows
//!   its chain one member at a time and the certificate verifies. A record that does not is not adopted,
//!   and is counted ([`electorate_records_refused`]).
//!
//! Every proposal for a slot on an electorate group carries the proposer's epoch and the electorate's
//! digest (`ConsensusMsg::PrepareIn` / `ProposeIn`); an acceptor answers only for its own epoch and
//! refuses any other by name (`ElectorateStale`). The rules and what they guarantee are stated once,
//! in the design record §8.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{GossipAgent, TaskCtx};
use crate::node_id::NodeId;

/// The slot family an electorate's own decisions use: `electorate/{group}/{epoch}`.
pub const ELECTORATE_SLOT_PREFIX: &str = "electorate/";
/// `consensus/electorate-cert/{group}/{epoch}` — the signed votes that decided an epoch.
pub const ELECTORATE_CERT_PREFIX: &str = "consensus/electorate-cert/";

/// Slot families the substrate's exclusive verbs propose in — safety-sensitive **by definition**:
/// `distributed_lock` and the lock route (`lock/`), `elect_leader` and the election route (`leader/`),
/// `consistent_set` and its route (`consistent/`), and an electorate's own steps (`electorate/`).
pub const SAFETY_SLOT_FAMILIES: &[&str] = &["lock/", "leader/", "consistent/", ELECTORATE_SLOT_PREFIX];

static RECORDS_REFUSED: AtomicU64 = AtomicU64::new(0);

/// Electorate records this process refused to adopt — a committed `electorate/{group}/{epoch}` entry
/// that did not follow the chain one member at a time, or whose certificate did not verify (a forgery,
/// or a write that did not come through the step protocol). Each distinct record is counted once.
pub fn electorate_records_refused() -> u64 {
    RECORDS_REFUSED.load(Ordering::Relaxed)
}

/// An electorate at one epoch: the group, the epoch, and **who** its members are.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ElectorateDecl {
    /// The group.
    pub group: String,
    /// The epoch: 1 at genesis, +1 per committed step.
    pub epoch: u64,
    /// The members, sorted and without duplicates. Votes are counted only from these.
    pub members: Vec<NodeId>,
    /// Whether this group is the fleet's electorate for the cluster-scoped exclusive verbs
    /// (`distributed_lock`, `consistent_set`, the lock and consistent routes, the log claim). A fleet
    /// record, so every node agrees on it; `consensus_electorate` may only restate it.
    pub exclusive_default: bool,
}

impl ElectorateDecl {
    /// A strict majority of the members.
    pub fn quorum(&self) -> usize {
        self.members.len() / 2 + 1
    }

    /// Whether `node` is a member at this epoch.
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
        h.finalize().into()
    }

    /// The bytes a step proposes.
    #[cfg(feature = "consensus")]
    pub(crate) fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    #[cfg(feature = "consensus")]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// Why a declaration did not take effect.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElectorateError {
    /// The group name is empty or contains `/`.
    InvalidGroup,
    /// The roster this node sees is empty, or does not hold this node: a genesis is decided by the
    /// members it names, and a step by members of the current epoch.
    NotAMember,
    /// The roster differs from the current electorate by more than one member. Two strict majorities of
    /// electorates that differ by one member intersect; a larger step has no such argument. Move one
    /// node, declare, and repeat.
    StepTooLarge {
        /// The current epoch's member count.
        from: usize,
        /// The roster's member count.
        to: usize,
        /// Members added plus members removed.
        changed: usize,
    },
    /// The step was not decided: another step won the slot (re-read the electorate), the electorate
    /// moved under it, or no quorum answered in time. Nothing changed unless the reason says so.
    NotDecided(String),
}

impl std::fmt::Display for ElectorateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGroup => write!(f, "the group must be a non-empty name without '/'"),
            Self::NotAMember => write!(f, "this node is not in the group's roster (a genesis) or its current electorate (a step)"),
            Self::StepTooLarge { from, to, changed } => write!(
                f,
                "an electorate changes one member at a time: the electorate has {from}, the roster {to}, \
                 {changed} member(s) differ — move one node, declare, and repeat"
            ),
            Self::NotDecided(why) => write!(f, "the electorate step was not decided: {why}"),
        }
    }
}

impl std::error::Error for ElectorateError {}

/// `electorate/{group}/{epoch}`.
#[cfg(feature = "consensus")]
pub(crate) fn step_slot(group: &str, epoch: u64) -> String {
    format!("{ELECTORATE_SLOT_PREFIX}{group}/{epoch}")
}

/// `(group, epoch)` for a step slot.
#[cfg(feature = "consensus")]
pub(crate) fn parse_step_slot(slot: &str) -> Option<(&str, u64)> {
    let rest = slot.strip_prefix(ELECTORATE_SLOT_PREFIX)?;
    let (group, epoch) = rest.split_once('/')?;
    if group.is_empty() || epoch.contains('/') { return None; }
    Some((group, epoch.parse().ok()?))
}

/// The certificate key for `(group, epoch)`.
#[cfg(feature = "consensus")]
pub(crate) fn cert_key(group: &str, epoch: u64) -> String {
    format!("{ELECTORATE_CERT_PREFIX}{group}/{epoch}")
}

/// **The step rule**, pure: `next` follows `prev` (or is a genesis) — same group, the next epoch, a
/// non-empty sorted member set, and at most one member added or removed.
#[cfg(feature = "consensus")]
pub(crate) fn check_step(group: &str, prev: Option<&ElectorateDecl>, next: &ElectorateDecl) -> Result<(), ElectorateError> {
    if group.is_empty() || group.contains('/') || next.group != group { return Err(ElectorateError::InvalidGroup); }
    if next.members.is_empty() { return Err(ElectorateError::NotAMember); }
    if next.members.windows(2).any(|w| w[0].to_string() >= w[1].to_string()) {
        return Err(ElectorateError::NotDecided("the member set is not sorted and unique".into()));
    }
    let want_epoch = prev.map_or(1, |p| p.epoch + 1);
    if next.epoch != want_epoch {
        return Err(ElectorateError::NotDecided(format!("epoch {} does not follow {}", next.epoch, want_epoch - 1)));
    }
    if let Some(p) = prev {
        let changed = member_changes(&p.members, &next.members);
        if changed > 1 {
            return Err(ElectorateError::StepTooLarge { from: p.members.len(), to: next.members.len(), changed });
        }
    }
    Ok(())
}

/// A member list in the electorate's canonical order: sorted by the node's address, without duplicates.
#[cfg(feature = "consensus")]
pub(crate) fn sorted_members(mut members: Vec<NodeId>) -> Vec<NodeId> {
    members.sort_by_key(ToString::to_string);
    members.dedup();
    members
}

/// Members added plus members removed between two sorted sets.
#[cfg(feature = "consensus")]
pub(crate) fn member_changes(a: &[NodeId], b: &[NodeId]) -> usize {
    a.iter().filter(|m| !b.contains(m)).count() + b.iter().filter(|m| !a.contains(m)).count()
}

/// The verified electorate this node holds for `group`, without refreshing it.
#[cfg(feature = "consensus")]
pub(crate) fn cached(ctx: &TaskCtx, group: &str) -> Option<Arc<ElectorateDecl>> {
    ctx.electorates.pin().get(group).cloned()
}

/// Every group that has a committed electorate record this node can see (verified or not — the caller
/// verifies through [`view`]).
pub(crate) fn groups_with_records(ctx: &TaskCtx) -> Vec<String> {
    // `consensus_ns::COMMITTED`, spelled out: this module compiles without `consensus` too.
    let prefix = format!("consensus/committed/{ELECTORATE_SLOT_PREFIX}");
    let mut groups: Vec<String> = crate::store::scan_kv_prefix(&ctx.kv_state, &prefix)
        .into_iter()
        .filter_map(|(k, _)| k.strip_prefix(prefix.as_str()).and_then(|r| r.split_once('/')).map(|(g, _)| g.to_string()))
        .collect();
    groups.sort();
    groups.dedup();
    groups
}

/// The verified electorate for `group` — refreshed from the committed chain. `None` when the group is
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

/// Counts a refused record once per distinct `(slot, digest)` and warns once.
#[cfg(feature = "consensus")]
pub(crate) fn note_refused(ctx: &TaskCtx, slot: &str, digest: &[u8; 32], why: &str) {
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let key: Arc<str> = Arc::from(format!("{slot}#{tag}"));
    let seen = ctx.electorate_refused.pin();
    if seen.contains(&key) || seen.len() >= 4096 {
        return;
    }
    seen.insert(key);
    RECORDS_REFUSED.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_electorate_records_refused_total").increment(1);
    tracing::warn!(slot, why, "electorate: a committed record was not adopted — it did not come through the step protocol \
        (see mycelium::electorate_records_refused)");
}

/// Whether `slot` is safety-sensitive under `config`: flagged, or in a [`SAFETY_SLOT_FAMILIES`] family.
#[cfg(feature = "consensus")]
pub(crate) fn is_safety_sensitive(config: &crate::consensus::ConsensusConfig, slot: &str) -> bool {
    config.safety_sensitive || SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f))
}

/// **Where the fleet's exclusive verbs decide** (finding 2 of #601's review): the electorate group whose
/// current epoch is marked `exclusive_default` — a fleet record, so every node reads the same answer —
/// or the whole cluster when none is. `consensus_electorate` may only restate it: a local setting that
/// names another group, or names one when the fleet names none, is refused by name, as are two groups
/// both marked.
#[cfg(feature = "consensus")]
pub(crate) fn exclusive_electorate(ctx: &Arc<TaskCtx>) -> Result<Option<String>, String> {
    let marked: Vec<String> = all_views(ctx).into_iter().filter(|d| d.exclusive_default).map(|d| d.group.clone()).collect();
    if marked.len() > 1 {
        return Err(format!("more than one electorate group is marked the fleet's exclusive default: {}", marked.join(", ")));
    }
    let fleet = marked.into_iter().next();
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

/// **Declare** `group`'s electorate from the roster this node sees: a genesis when the group has none
/// (decided by every member named), otherwise a one-member step decided by the current electorate. See
/// [`GossipAgent::declare_electorate`].
#[cfg(feature = "consensus")]
pub(crate) async fn declare(ctx: &Arc<TaskCtx>, group: &str, exclusive_default: bool) -> Result<ElectorateDecl, ElectorateError> {
    use crate::consensus::ConsensusResult;
    if group.is_empty() || group.contains('/') { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let current = engine.electorate_view(group);
    let members = sorted_members(crate::agent::helpers::group_members_ctx(ctx, group));
    let next = ElectorateDecl {
        group: group.to_string(),
        epoch: current.as_ref().map_or(1, |c| c.epoch + 1),
        members,
        exclusive_default,
    };
    if let Some(c) = &current
        && c.members == next.members && c.exclusive_default == next.exclusive_default {
            return Ok((**c).clone()); // nothing to decide
        }
    check_step(group, current.as_deref(), &next)?;
    let proposer_set = current.as_ref().map_or(&next.members, |c| &c.members);
    if !proposer_set.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    match propose_step(ctx, current.as_deref(), &next).await {
        ConsensusResult::Committed { .. } => match engine.electorate_view(group) {
            Some(v) if *v == next => Ok(next),
            _ => Err(ElectorateError::NotDecided(
                "committed, but its certificate did not reach the store — the record was not adopted".into())),
        },
        ConsensusResult::NotAMember { .. } => Err(ElectorateError::NotAMember),
        other => Err(ElectorateError::NotDecided(format!("{other:?}"))),
    }
}

/// Proposes `next` as the step after `current` (a genesis when `None`) on the slot `electorate/{group}/{epoch}`:
/// decided by a strict majority of `current`'s members, or by every member `next` names for a genesis. The
/// engine's door and every acceptor check the step; this only frames it.
#[cfg(feature = "consensus")]
pub(crate) async fn propose_step(
    ctx: &Arc<TaskCtx>, current: Option<&ElectorateDecl>, next: &ElectorateDecl,
) -> crate::consensus::ConsensusResult {
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let quorum = current.map_or(next.members.len(), ElectorateDecl::quorum);
    let slot = step_slot(&next.group, next.epoch);
    let cfg = crate::consensus::ConsensusConfig { safety_sensitive: true, ..crate::consensus::ConsensusConfig::default() };
    engine
        .propose(crate::signal::SignalScope::Group(Arc::from(next.group.as_str())), Arc::from(slot.as_str()), next.encode(), quorum, cfg, None)
        .await
}

/// A consensus engine over `ctx`, for tests that drive the acceptor's door directly.
#[cfg(all(test, feature = "consensus"))]
pub(crate) fn engine(ctx: &Arc<TaskCtx>) -> crate::consensus::ConsensusEngine {
    crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None)
}

impl GossipAgent {
    /// **Declare `group`'s electorate** — a governance act, decided by consensus (P2,
    /// `docs/design/consensus-electorate.md` §8). Reads the group's roster as this node sees it:
    ///
    /// - if the group has no electorate, proposes **genesis** — epoch 1, the roster as the member set —
    ///   which every member must accept, each checking the set is the roster it sees;
    /// - otherwise proposes the **next epoch** with the roster as the member set, which must differ from
    ///   the current electorate by **one** member ([`ElectorateError::StepTooLarge`] otherwise), and is
    ///   decided by a strict majority of the **current** electorate.
    ///
    /// So changing an electorate is: move one node (`/gateway/govern/group`), then declare. Between the
    /// two the roster differs from the member set, and every proposal to the group is refused. The
    /// electorate is named by the group; which nodes are in it is what the group decided.
    /// `exclusive_default` marks the group as the fleet's electorate for the cluster-scoped exclusive
    /// verbs. Requires the consensus listener on every member.
    #[cfg(feature = "consensus")]
    pub async fn declare_electorate(&self, group: &str, exclusive_default: bool) -> Result<ElectorateDecl, ElectorateError> {
        declare(&self.task_ctx, group, exclusive_default).await
    }

    /// The verified electorate this node holds for `group`, if any.
    pub fn electorate(&self, group: &str) -> Option<ElectorateDecl> {
        view(&self.task_ctx, group).map(|d| (*d).clone())
    }
}

#[cfg(all(test, feature = "consensus"))]
mod tests {
    use super::*;

    fn n(port: u16) -> NodeId { NodeId::new("127.0.0.1", port).unwrap() }
    fn decl(epoch: u64, ports: &[u16]) -> ElectorateDecl {
        let members = sorted_members(ports.iter().map(|p| n(*p)).collect());
        ElectorateDecl { group: "e".into(), epoch, members, exclusive_default: false }
    }

    /// A step follows its chain: the next epoch, one member added or removed — a swap is two.
    #[test]
    fn a_step_moves_one_member_and_one_epoch() {
        let e1 = decl(1, &[1, 2, 3]);
        assert_eq!(check_step("e", None, &e1), Ok(()), "a genesis may name any set");
        assert_eq!(check_step("e", Some(&e1), &decl(2, &[1, 2, 3, 4])), Ok(()));
        assert_eq!(check_step("e", Some(&e1), &decl(2, &[1, 2])), Ok(()));
        assert!(matches!(check_step("e", Some(&e1), &decl(2, &[1, 2, 4])), Err(ElectorateError::StepTooLarge { changed: 2, .. })),
            "a swap is two changes");
        assert!(matches!(check_step("e", Some(&e1), &decl(3, &[1, 2, 3, 4])), Err(ElectorateError::NotDecided(_))),
            "an epoch cannot be skipped");
        assert_eq!(check_step("a/b", None, &e1), Err(ElectorateError::InvalidGroup));
        let mut unsorted = decl(1, &[1, 2]);
        unsorted.members.reverse();
        assert!(check_step("e", None, &unsorted).is_err());
    }

    #[test]
    fn step_slots_round_trip() {
        assert_eq!(step_slot("g", 7), "electorate/g/7");
        assert_eq!(parse_step_slot("electorate/g/7"), Some(("g", 7)));
        assert_eq!(parse_step_slot("electorate/g/x"), None);
        assert_eq!(parse_step_slot("leader/g"), None);
    }

    #[test]
    fn the_exclusive_slot_families_are_safety_sensitive_by_definition() {
        let plain = crate::consensus::ConsensusConfig::default();
        for slot in ["lock/x", "leader/g", "consistent/k", "electorate/g/2"] {
            assert!(is_safety_sensitive(&plain, slot), "{slot}");
        }
        assert!(!is_safety_sensitive(&plain, "work/item-7"));
        let flagged = crate::consensus::ConsensusConfig { safety_sensitive: true, ..plain };
        assert!(is_safety_sensitive(&flagged, "work/item-7"));
    }
}
