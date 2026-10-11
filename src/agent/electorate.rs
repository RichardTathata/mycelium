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
//! never change, and **changing them is not built in this release** — migration is the follow-up (branch
//! `fix/p2-migration`). A dead member lowers the group's fault tolerance; a lost majority stops it. The fleet's
//! exclusive default is set once, at genesis, and cannot move in this release.
//!
//! A genesis epoch counts **attempts**: a genesis whose members no longer all answer can be superseded by
//! one at the next epoch naming the live roster — a majority of the one it supersedes, every one of which
//! held it and none of which adopted it (round 4 of #601's review, finding 3).
//!
//! Every proposal for a slot on an electorate group carries the proposer's epoch and the electorate's
//! digest (`ConsensusMsg::PrepareIn` / `ProposeIn`); an acceptor answers only for its own electorate and
//! refuses any other by name (`ElectorateStale`). The rules and why they are safe are stated once, in the
//! design record §8.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{GossipAgent, TaskCtx};
use crate::node_id::NodeId;

/// The slot family an electorate's own decisions use: `electorate/{group}/{epoch}`, a genesis attempt.
pub const ELECTORATE_SLOT_PREFIX: &str = "electorate/";
/// `consensus/electorate-cert/{group}/{epoch}` — the decided envelope and the signed votes that decided a genesis.
pub const ELECTORATE_CERT_PREFIX: &str = "consensus/electorate-cert/";

/// Slot families the substrate's exclusive verbs propose in — safety-sensitive **by definition**:
/// `distributed_lock` and the lock route (`lock/`), `elect_leader` and the election route (`leader/`),
/// `consistent_set` and its route (`consistent/`), and an electorate's own decisions (`electorate/`). With
/// [`is_fleet_exclusive_slot`]'s `capauthz/` and commitment awards, these are what every acceptor refuses
/// across the cluster once the fleet marks an electorate.
pub const SAFETY_SLOT_FAMILIES: &[&str] = &["lock/", "leader/", "consistent/", "capauthz/", ELECTORATE_SLOT_PREFIX];

static RECORDS_REFUSED: AtomicU64 = AtomicU64::new(0);

/// Electorate records this process refused to adopt — a committed `electorate/{group}/…` entry that is not
/// a well-formed genesis, or whose certificate did not verify (a forgery, or a write that did not come through
/// consensus). Each distinct record is counted once.
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
    /// Whether this group is the fleet's default electorate for the cluster-scoped exclusive verbs
    /// (`distributed_lock`, `consistent_set`, the lock and consistent routes, the log claim) — accepted only while
    /// no other group holds it, and fixed for the group's life (moving it is the migration follow-up's work).
    /// `consensus_electorate` may only restate it.
    pub exclusive_default: bool,
    /// The pending genesis this one supersedes (epoch > 1 only).
    pub supersedes: Option<Superseded>,
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
        h.finalize().into()
    }

    /// The bytes a genesis proposes.
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
    /// This node is not in the roster it would declare: a genesis is decided only by every member it names.
    NotAMember,
    /// **Membership is fixed at genesis**: the group has an electorate, and the roster (or the default marking) now
    /// differs from it. Nothing changes an adopted electorate in this release — changing members is the migration
    /// follow-up's work (`docs/design/consensus-electorate.md` §8.3).
    MembershipFrozen {
        /// The group.
        group: String,
    },
    /// Nothing was decided: no quorum answered in time, or another proposal won. The reason says which.
    /// [`GossipAgent::declare_electorate`] retries this with a bounded backoff before returning it.
    NotDecided(String),
    /// Refused: a precondition no retry can change — a door refusal, a second default while one is held, a
    /// superseding genesis that keeps no majority, or this node cannot name the genesis it would supersede. The reason
    /// says which. Not retried.
    Refused(String),
}

impl std::fmt::Display for ElectorateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGroup => write!(f, "the group must be a non-empty name without '/'"),
            Self::NotAMember => write!(f, "this node is not in the roster it would declare"),
            Self::MembershipFrozen { group } => write!(
                f,
                "electorate group {group}'s membership is fixed at genesis: the roster differs from its electorate, and \
                 changing an electorate's members is not built in this release"
            ),
            Self::NotDecided(why) => write!(f, "the electorate decision was not decided: {why}"),
            Self::Refused(why) => write!(f, "the electorate decision was refused: {why}"),
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
/// generation), recording which groups have records, the highest genesis epoch committed per group, and which
/// — so a proposal does not walk every `consensus/` key (round 2, finding 5).
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
            if let Ok(epoch) = last.parse::<u64>() {
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
/// decided only by that group.
#[cfg(feature = "consensus")]
pub(crate) fn slot_names_group(slot: &str) -> Option<&str> {
    if let Some(g) = slot.strip_prefix("leader/") { return Some(g); }
    slot.strip_prefix(ELECTORATE_SLOT_PREFIX).and_then(|r| r.split_once('/')).map(|(g, _)| g)
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

/// **The fleet's default electorate**, as its records say (no local setting): the electorate group whose genesis
/// declared `exclusive_default`, or none when no group did. Set once, at genesis, and fixed for this release. `Err` —
/// fail closed — when two groups declare it: only two declarations made within gossip lag, by members that could not
/// see each other's genesis, can both be decided (each is refused once the other is visible); the exclusive verbs are
/// then refused by name, and the escape — merging them — is the migration follow-up's work.
#[cfg(feature = "consensus")]
pub(crate) fn live_default(ctx: &Arc<TaskCtx>) -> Result<Option<String>, String> {
    let marked: Vec<String> = all_views(ctx).into_iter().filter(|d| d.exclusive_default).map(|d| d.group.clone()).collect();
    if marked.len() > 1 {
        return Err(format!("more than one electorate group is marked the fleet's exclusive default: {}", marked.join(", ")));
    }
    Ok(marked.into_iter().next())
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
fn decision_config() -> crate::consensus::ConsensusConfig {
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
/// - The group has an electorate: if the roster and the default marking match it, nothing to decide;
///   otherwise [`ElectorateError::MembershipFrozen`] — changing members is not built in this release.
/// - It has none: a genesis naming the roster, decided by every member named. If this node holds a
///   pending genesis (accepted, never adopted) naming another set, the new genesis **supersedes** it at the next
///   epoch; one naming this set is completed at its own epoch.
#[cfg(feature = "consensus")]
pub(crate) async fn declare_outcome(
    ctx: &Arc<TaskCtx>, group: &str, exclusive_default: bool,
) -> Result<(ElectorateDecl, Declared), ElectorateError> {
    use crate::consensus::ConsensusResult;
    if !valid_group(group) { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let roster = sorted_members(crate::agent::helpers::group_members_ctx(ctx, group));
    if let Some(cur) = engine.electorate_view(group) {
        if cur.members == roster && cur.exclusive_default == exclusive_default {
            return Ok(((*cur).clone(), Declared::Unchanged));
        }
        return Err(ElectorateError::MembershipFrozen { group: group.to_string() });
    }
    if !roster.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    // One default: a genesis may declare it only while no group holds it (round 5, D1/D2).
    if exclusive_default {
        match live_default(ctx) {
            Ok(None) => {}
            Ok(Some(g)) => return Err(ElectorateError::Refused(format!(
                "{g} already holds the fleet's exclusive default; it is set once, at genesis, and cannot move in this \
                 release"))),
            Err(e) => return Err(ElectorateError::Refused(e)),
        }
    }
    let base = ElectorateDecl {
        group: group.to_string(), epoch: 1, members: roster, exclusive_default, supersedes: None,
    };
    let (touched, pending) = engine.pending_genesis(group);
    let same_set = |t: &ElectorateDecl| t.members == base.members && t.exclusive_default == base.exclusive_default;
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
    /// **Changing an electorate's members is not built in this release** (the migration follow-up). A dead member
    /// lowers the group's fault tolerance; a lost majority stops the group. A pending genesis that can no longer
    /// complete (a member it names is gone) is superseded by declaring the live roster from a member that holds it.
    ///
    /// Retries an undecided attempt with a bounded backoff (eight tries, 250 ms doubling to 4 s); a refusal no retry
    /// changes returns at once ([`ElectorateError::Refused`]). Requires the consensus listener on every member.
    #[cfg(feature = "consensus")]
    pub async fn declare_electorate(&self, group: &str, exclusive_default: bool) -> Result<ElectorateDecl, ElectorateError> {
        retrying(|| async { declare_outcome(&self.task_ctx, group, exclusive_default).await.map(|(d, _)| d) }).await
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
    }

    /// The digest commits to everything a record says — the superseded genesis included.
    #[test]
    fn the_digest_commits_to_what_is_superseded() {
        let g1 = decl(1, &[1, 2, 3]);
        let g2 = superseding(&g1, &[1, 2]);
        let mut other = g2.clone();
        other.supersedes.as_mut().unwrap().members.pop();
        assert_ne!(g2.digest(), other.digest());
        assert_eq!(ElectorateDecl::decode(&g2.encode()), Some(g2));
    }

    #[test]
    fn electorate_slots_round_trip() {
        assert_eq!(genesis_slot("g", 7), "electorate/g/7");
        assert_eq!(parse_genesis_slot("electorate/g/7"), Some(("g", 7)));
        assert_eq!(parse_genesis_slot("electorate/g/x"), None);
        assert_eq!(parse_genesis_slot("leader/g"), None);
        assert_eq!(slot_names_group("electorate/g/7"), Some("g"));
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
