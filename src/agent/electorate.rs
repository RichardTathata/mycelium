//! **Electorate groups** — post-360 plan row P2, `docs/design/consensus-electorate.md` §8.
//!
//! Discovery and the electorate are separate capabilities (D1): a group's roster is dynamic, and a
//! consensus electorate is a fixed set for the life of a decision. An **electorate group** is a group
//! with an [`ElectorateDecl`] at `sys/govern/electorate/{group}`, written only by a governance act
//! ([`GossipAgent::declare_electorate`], `POST /gateway/govern/electorate`). It names the electorate as a
//! **group**, never as node identities; which nodes are in it is governance.
//!
//! What the declaration changes, each enforced where it is read:
//!
//! - it **does not evaporate** — unlike a `MembershipIntent`, it holds until retired;
//! - the membership governor and the emergent watcher **do not move** an electorate group, and the
//!   gateway's data-plane group routes refuse it (`governed_group`);
//! - a proposal to it is refused unless the roster this node sees holds **exactly** `size` members, its
//!   votes are counted only from that roster, and its quorum is at least a strict majority of `size`
//!   (the engine's door, `consensus.rs`);
//! - with `consensus_require_electorate`, a **safety-sensitive** proposal to anything else is refused
//!   `ElectorateNotGoverned`.
//!
//! Detection, not prevention: an embedded caller can still write `grp/` or this prefix. Layer I is not
//! taught the rule; the proposal-time check refuses what it finds.

use serde::{Deserialize, Serialize};

use super::GossipAgent;

/// `sys/govern/electorate/{group}` — one declaration per electorate group.
pub const ELECTORATE_PREFIX: &str = "sys/govern/electorate/";

/// The governance record that makes a group an electorate: its name and how many members it holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ElectorateDecl {
    /// The group.
    pub group: String,
    /// The number of members the electorate holds — exact, not a floor. A proposal to the group is
    /// refused while the roster a node sees differs from it.
    pub size: usize,
    /// When the declaration was written (Unix ms, informational — a declaration does not evaporate).
    pub written_at_ms: u64,
}

/// Why a declaration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElectorateError {
    /// The group name is empty or contains `/`.
    InvalidGroup,
    /// `size` is zero: an electorate of nobody decides nothing.
    ZeroSize,
    /// The re-declaration moves `size` by more than one member. Two strict majorities of electorates
    /// that differ by one member intersect; a larger step has no such argument. Step one at a time,
    /// or retire and re-form.
    StepTooLarge {
        /// The size currently declared.
        from: usize,
        /// The size asked for.
        to: usize,
    },
}

impl std::fmt::Display for ElectorateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGroup => write!(f, "the group must be a non-empty name without '/'"),
            Self::ZeroSize => write!(f, "an electorate's size must be at least 1"),
            Self::StepTooLarge { from, to } => write!(
                f,
                "an electorate changes one member at a time: declared {from}, asked {to} — step by one, \
                 or retire the declaration and re-form the group"
            ),
        }
    }
}

impl std::error::Error for ElectorateError {}

fn decl_key(group: &str) -> String {
    format!("{ELECTORATE_PREFIX}{group}")
}

/// The declaration for `group`, if it is an electorate group on this node's view.
pub(crate) fn electorate_decl(kv_state: &crate::store::KvState, group: &str) -> Option<ElectorateDecl> {
    let bytes = kv_state.store.pin().get(decl_key(group).as_str()).and_then(|e| e.data.clone())?;
    mycelium_core::serde_fixint::from_slice::<ElectorateDecl>(&bytes).ok().filter(|d| d.group == group)
}

/// The declared size of `group`'s electorate, or `None` when it is not an electorate group.
pub(crate) fn electorate_size(kv_state: &crate::store::KvState, group: &str) -> Option<usize> {
    electorate_decl(kv_state, group).map(|d| d.size)
}

/// Every electorate declaration this node sees.
pub(crate) fn electorate_decls(kv_state: &crate::store::KvState) -> Vec<ElectorateDecl> {
    super::capability_ops::scan_prefix_kv(kv_state, ELECTORATE_PREFIX)
        .into_iter()
        .filter_map(|(k, b)| {
            let d = mycelium_core::serde_fixint::from_slice::<ElectorateDecl>(&b).ok()?;
            (k.strip_prefix(ELECTORATE_PREFIX) == Some(d.group.as_str())).then_some(d)
        })
        .collect()
}

/// Every group this node sees an electorate declaration for.
pub(crate) fn electorate_groups(kv_state: &crate::store::KvState) -> Vec<String> {
    electorate_decls(kv_state).into_iter().map(|d| d.group).collect()
}

/// Slot families the substrate's exclusive verbs propose in — safety-sensitive **by definition**:
/// `distributed_lock` and the lock route (`lock/`), `elect_leader` and the election route (`leader/`),
/// `consistent_set` and its route (`consistent/`).
pub const SAFETY_SLOT_FAMILIES: &[&str] = &["lock/", "leader/", "consistent/"];

/// Whether a proposal for `slot` under `config` is safety-sensitive: flagged, or in a
/// [`SAFETY_SLOT_FAMILIES`] family.
#[cfg(feature = "consensus")]
pub(crate) fn is_safety_sensitive(config: &crate::consensus::ConsensusConfig, slot: &str) -> bool {
    config.safety_sensitive || SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f))
}

/// The pure rule a declaration obeys: a non-empty name without `/`, a size of at least one, and a
/// change of at most one member from what is declared now.
pub(crate) fn check_declaration(group: &str, size: usize, current: Option<usize>) -> Result<(), ElectorateError> {
    if group.is_empty() || group.contains('/') { return Err(ElectorateError::InvalidGroup); }
    if size == 0 { return Err(ElectorateError::ZeroSize); }
    if let Some(from) = current && from.abs_diff(size) > 1 {
        return Err(ElectorateError::StepTooLarge { from, to: size });
    }
    Ok(())
}

/// Write a declaration — the one path both [`GossipAgent::declare_electorate`] and the gateway's
/// `POST /gateway/govern/electorate` take.
pub(crate) fn declare(ctx: &super::TaskCtx, group: &str, size: usize) -> Result<ElectorateDecl, ElectorateError> {
    check_declaration(group, size, electorate_size(&ctx.kv_state, group))?;
    let decl = ElectorateDecl {
        group: group.to_string(),
        size,
        written_at_ms: mycelium_core::sim_seam::wall_now_ms(),
    };
    if let Ok(bytes) = mycelium_core::serde_fixint::to_vec(&decl) {
        let _ = super::helpers::kv_set(ctx, std::sync::Arc::from(decl_key(group)), bytes::Bytes::from(bytes));
    }
    Ok(decl)
}

/// Retire a declaration; whether there was one.
pub(crate) fn retire(ctx: &super::TaskCtx, group: &str) -> bool {
    let had = electorate_decl(&ctx.kv_state, group).is_some();
    if had {
        let _ = super::helpers::kv_delete(ctx, std::sync::Arc::from(decl_key(group)));
    }
    had
}

impl GossipAgent {
    /// **Declare `group` an electorate group of `size` members** — a governance act (P2,
    /// `docs/design/consensus-electorate.md` §8). Gossips `sys/govern/electorate/{group}`; it does not
    /// evaporate. From then on the governor and the emergent watcher leave the group alone, the
    /// gateway's data-plane group routes refuse it, and a proposal to it is decided by exactly `size`
    /// members — refused while the roster a node sees holds any other number.
    ///
    /// A re-declaration may move `size` by one member at most ([`ElectorateError::StepTooLarge`]);
    /// to change it further, step, or [`retire_electorate`](Self::retire_electorate) and re-form.
    /// Changing an electorate is two acts: move the node (`/gateway/govern/group`), then re-declare.
    /// Between them the group decides nothing.
    pub fn declare_electorate(&self, group: &str, size: usize) -> Result<ElectorateDecl, ElectorateError> {
        declare(&self.task_ctx, group, size)
    }

    /// **Retire `group`'s electorate declaration** — it becomes an ordinary group again. Returns
    /// whether there was one. While retired, a safety-sensitive proposal to it is refused under
    /// `consensus_require_electorate`.
    pub fn retire_electorate(&self, group: &str) -> bool {
        retire(&self.task_ctx, group)
    }

    /// The electorate declaration this node sees for `group`, if any.
    pub fn electorate(&self, group: &str) -> Option<ElectorateDecl> {
        electorate_decl(&self.task_ctx.kv_state, group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A declaration names a group, has members, and moves one member at a time.
    #[test]
    fn a_declaration_moves_one_member_at_a_time() {
        assert_eq!(check_declaration("", 3, None), Err(ElectorateError::InvalidGroup));
        assert_eq!(check_declaration("a/b", 3, None), Err(ElectorateError::InvalidGroup));
        assert_eq!(check_declaration("e", 0, None), Err(ElectorateError::ZeroSize));
        assert_eq!(check_declaration("e", 5, None), Ok(()), "a first declaration may be any size");
        assert_eq!(check_declaration("e", 4, Some(3)), Ok(()));
        assert_eq!(check_declaration("e", 2, Some(3)), Ok(()));
        assert_eq!(check_declaration("e", 3, Some(3)), Ok(()));
        assert_eq!(check_declaration("e", 5, Some(3)), Err(ElectorateError::StepTooLarge { from: 3, to: 5 }));
    }

    #[cfg(feature = "consensus")]
    #[test]
    fn the_exclusive_slot_families_are_safety_sensitive_by_definition() {
        let plain = crate::consensus::ConsensusConfig::default();
        for slot in ["lock/x", "leader/g", "consistent/k"] {
            assert!(is_safety_sensitive(&plain, slot), "{slot}");
        }
        assert!(!is_safety_sensitive(&plain, "work/item-7"));
        let flagged = crate::consensus::ConsensusConfig { safety_sensitive: true, ..plain };
        assert!(is_safety_sensitive(&flagged, "work/item-7"));
    }
}
