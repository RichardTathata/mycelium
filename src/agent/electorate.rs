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
/// `consistent_set` and its route (`consistent/`), and an electorate's own steps (`electorate/`). With
/// [`is_fleet_exclusive_slot`]'s `capauthz/` and commitment awards, these are what every acceptor refuses
/// across the cluster once the fleet marks an electorate.
pub const SAFETY_SLOT_FAMILIES: &[&str] = &["lock/", "leader/", "consistent/", "capauthz/", ELECTORATE_SLOT_PREFIX];

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
    /// The step was not decided: the electorate moved under it, or no quorum answered in time. Nothing
    /// changed unless the reason says so.
    NotDecided(String),
    /// The step's drain could not complete, so the step was refused: a member reports more slots of the
    /// group than the drain carries (`drain_too_large`), a slot's highest acceptance is known only by its
    /// digest (`drain_blocked`), or a slot did not complete (`drain_incomplete`). The electorate did not
    /// move; reduce the group's open slots, or let them complete, and declare again.
    DrainRefused(String),
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
            Self::DrainRefused(why) => write!(f, "the electorate step was refused: its drain did not complete ({why})"),
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
/// verifies through [`view`]). Rescanned only when `consensus/committed/electorate/` changed since the last
/// scan (a prefix subscription's generation), so a proposal does not walk every `consensus/` key (round 2
/// of #601's review, finding 5). Without `consensus` there are none.
pub(crate) fn groups_with_records(ctx: &TaskCtx) -> Vec<String> {
    #[cfg(feature = "consensus")]
    {
        use std::sync::atomic::Ordering::{Acquire, Release};
        let prefix = format!("{}{ELECTORATE_SLOT_PREFIX}", crate::consensus::consensus_ns::COMMITTED);
        let rx = ctx.electorate_records_watch.get_or_init(|| {
            mycelium_core::ops::kv_subscribe_prefix(ctx, Arc::from(prefix.as_str()))
        });
        let generation = *rx.borrow();
        if ctx.electorate_records_scanned.swap(generation, Release) != generation {
            for (k, _) in crate::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
                if let Some((g, _)) = k.strip_prefix(prefix.as_str()).and_then(|r| r.split_once('/')) {
                    ctx.electorate_groups_known.pin().insert(Arc::from(g));
                }
            }
        }
        let _ = ctx.electorate_records_scanned.load(Acquire);
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

/// Restores which electorate group each slot this node answered for belongs to, from its durable records.
#[cfg(feature = "consensus")]
pub(crate) fn prewarm_slot_groups(ctx: &TaskCtx) {
    let prefix = format!("{}{}/", mycelium_core::signal::kv_ns::CONSENSUS_SLOT_GROUP, ctx.node_id);
    for (k, v) in crate::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
        if let (Some(slot), Ok(group)) = (k.strip_prefix(prefix.as_str()), std::str::from_utf8(&v)) {
            ctx.electorate_slot_groups.pin().insert(Arc::from(slot), Arc::from(group));
        }
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

/// The group a slot names itself, when its family does: `leader/{group}` and `electorate/{group}/{epoch}`
/// are decided only by that group.
#[cfg(feature = "consensus")]
pub(crate) fn slot_names_group(slot: &str) -> Option<&str> {
    if let Some(g) = slot.strip_prefix("leader/") { return Some(g); }
    slot.strip_prefix(ELECTORATE_SLOT_PREFIX).and_then(|r| r.split_once('/')).map(|(g, _)| g)
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
    tracing::warn!(slot, why, "electorate: a committed record was not adopted — it did not come through the step protocol \
        (see mycelium::electorate_records_refused)");
}

/// Whether `slot` is safety-sensitive under `config`: flagged, or in a [`SAFETY_SLOT_FAMILIES`] family.
#[cfg(feature = "consensus")]
pub(crate) fn is_safety_sensitive(config: &crate::consensus::ConsensusConfig, slot: &str) -> bool {
    config.safety_sensitive || SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f)) || is_fleet_exclusive_slot(slot)
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
    declare_outcome(ctx, group, exclusive_default).await.map(|(d, _)| d)
}

/// What a declaration did.
#[cfg(feature = "consensus")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Declared {
    /// Nothing to decide, and no step pending: the electorate is as it was.
    Unchanged,
    /// This node's step was decided.
    Decided,
    /// Another member's step was decided — this declaration completed it (round 2, finding 3).
    Adopted,
}

/// Whether a step out of `epoch` is pending: this node, or any member whose durable acceptor record has
/// reached this node, has promised the step slot without the electorate having moved. A pending step
/// fences the members that promised it, so the group decides nothing until it completes.
#[cfg(feature = "consensus")]
fn step_pending(ctx: &TaskCtx, current: &ElectorateDecl) -> bool {
    let slot = step_slot(&current.group, current.epoch + 1);
    if ctx.consensus_accepted.pin().get(slot.as_str()).is_some_and(|s| s.promised > 0) { return true; }
    current.members.iter().any(|m| {
        ctx.kv_state.store.pin().get(crate::consensus::accepted_key(m, &slot).as_str()).is_some_and(|e| e.data.is_some())
    })
}

/// [`declare`], saying what it did.
#[cfg(feature = "consensus")]
pub(crate) async fn declare_outcome(
    ctx: &Arc<TaskCtx>, group: &str, exclusive_default: bool,
) -> Result<(ElectorateDecl, Declared), ElectorateError> {
    use crate::consensus::ConsensusResult;
    if group.is_empty() || group.contains('/') { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let current = engine.electorate_view(group);
    let members = sorted_members(crate::agent::helpers::group_members_ctx(ctx, group));
    let mut next = ElectorateDecl {
        group: group.to_string(),
        epoch: current.as_ref().map_or(1, |c| c.epoch + 1),
        members,
        exclusive_default,
    };
    if let Some(c) = &current
        && c.members == next.members && c.exclusive_default == next.exclusive_default {
            if !step_pending(ctx, c) {
                return Ok(((**c).clone(), Declared::Unchanged)); // nothing to decide
            }
            // A step promised and never decided fences its members: complete it. Phase 1 adopts whatever
            // was accepted for the slot; if nothing was, a no-op step (the same members) decides it.
            next = ElectorateDecl { epoch: c.epoch + 1, ..(**c).clone() };
        }
    check_step(group, current.as_deref(), &next)?;
    let proposer_set = current.as_ref().map_or(&next.members, |c| &c.members);
    if !proposer_set.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    let result = propose_step(ctx, current.as_deref(), &next).await;
    let after = engine.electorate_view(group);
    match result {
        ConsensusResult::Committed { .. } => match after {
            Some(v) if *v == next => Ok((next, Declared::Decided)),
            _ => Err(ElectorateError::NotDecided(
                "committed, but its certificate did not reach the store — the record was not adopted".into())),
        },
        // The slot was decided for another member's step: this declaration completed it.
        ConsensusResult::Superseded { .. } if after.as_ref().is_some_and(|v| v.epoch == next.epoch) =>
            Ok(((*after.unwrap_or_else(|| Arc::new(next.clone()))).clone(), Declared::Adopted)),
        ConsensusResult::NotAMember { .. } => Err(ElectorateError::NotAMember),
        ConsensusResult::ElectorateMismatch { detail, .. } if detail.starts_with("drain_") =>
            Err(ElectorateError::DrainRefused(detail.to_string())),
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
