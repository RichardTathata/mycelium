//! **The partition table** (item 5) — what a curator may do while it cannot reach the enforcing
//! resource.
//!
//! §6.3 of the plan states the policy in one sentence, and derives it from the decisive invariant:
//!
//! > A disconnected curator **may prepare proposals** but **cannot promise canonical acceptance**
//! > without reaching the enforcing resource.
//!
//! # This is not a second fence
//!
//! D4 was just discharged with *"no second fence"*, so it matters to say plainly what this is. A
//! fence is an **enforcement point at the resource**: it decides what commits. This is neither at
//! the resource nor an enforcement point. It is a **client-side refusal to promise** — it stops a
//! curator making a commitment it has no way to keep, and it cannot stop a curator that ignores it.
//! [`ResourceAuthority`](super::ResourceAuthority) remains the only thing that decides what commits.
//!
//! Without it the failure is not a safety violation — the fence still refuses the write — it is a
//! **lie to a submitter**: "your content is accepted" said by someone whose authority may have been
//! withdrawn ten minutes ago, with the refusal arriving only when the partition heals.
//!
//! # Why the table is derived rather than chosen
//!
//! The root fact is an asymmetry, and it falls out of the three-way split in
//! [`LifecycleEvent`](super::LifecycleEvent):
//!
//! - **Expiry is locally decidable.** `valid_until_ms` is *in the mandate*. A partitioned curator
//!   can compute that its term has ended without reaching anyone.
//! - **Revocation is not.** [`PermissionWithdrawn`](super::LifecycleEvent::PermissionWithdrawn)
//!   happens at the establishing authority. A partitioned curator **cannot distinguish "still
//!   mandated" from "revoked ten minutes ago"**, and no amount of local reasoning closes that gap.
//!
//! So the rule is: **an action may proceed while unreachable exactly when its correctness does not
//! depend on the mandate still being current.** That is [`claims_current_authority`], and the table
//! below is *derived* from it. The tests pin the derivation and the table against each other, so
//! changing one without the other fails.

/// Whether the enforcing resource can be reached right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reachability {
    /// The resource is reachable — the mandate can be verified inside its atomic boundary.
    Reachable,
    /// Partitioned from the resource. Revocation is unobservable from here.
    Unreachable,
}

/// What a curator is trying to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CuratorAction {
    /// Serve content from the local store. Makes no claim about authority.
    ReadLocal,
    /// Append an intention to the durable proposal log (item 5 PR 5's `KvHandle::append`).
    /// A proposal **records an intention**; it is not an acceptance and promises nothing.
    PrepareProposal,
    /// Conclude that this term's window has ended. Locally decidable — `valid_until_ms` is in the
    /// mandate.
    ObserveOwnExpiry,
    /// Tell a submitter their content is canonically accepted.
    PromiseAcceptance,
    /// Write to the protected resource.
    CommitCanonical,
    /// Extend or re-establish this mandate.
    RenewOwnMandate,
    /// Transfer the appointment to a successor.
    HandOver,
}

/// Every action, for sweeps. Kept beside the enum so a new variant that is not added here shows up
/// as a table that stopped covering the type.
pub const ALL_ACTIONS: [CuratorAction; 7] = [
    CuratorAction::ReadLocal,
    CuratorAction::PrepareProposal,
    CuratorAction::ObserveOwnExpiry,
    CuratorAction::PromiseAcceptance,
    CuratorAction::CommitCanonical,
    CuratorAction::RenewOwnMandate,
    CuratorAction::HandOver,
];

/// **The rule the table is derived from.** Does this action's correctness depend on the mandate
/// still being current?
///
/// If it does, a partitioned curator cannot establish it — revocation is unobservable from there —
/// so the action must be refused rather than guessed at.
pub fn claims_current_authority(action: CuratorAction) -> bool {
    match action {
        // Reads assert nothing about who may write.
        CuratorAction::ReadLocal => false,
        // An intention is not an acceptance. Recording one stays true even if the mandate is gone;
        // what the proposal is *worth* is decided later, at the resource.
        CuratorAction::PrepareProposal => false,
        // The window is in the mandate. Concluding it has closed needs nobody's agreement — and
        // note the direction: expiry only ever *narrows* what this curator will claim.
        CuratorAction::ObserveOwnExpiry => false,

        // Each of these is false the moment the mandate is not current, and the curator has no way
        // to know whether it is.
        CuratorAction::PromiseAcceptance => true,
        CuratorAction::CommitCanonical => true,
        CuratorAction::RenewOwnMandate => true,
        CuratorAction::HandOver => true,
    }
}

/// Why an action was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionRefusal {
    /// What was attempted.
    pub action: CuratorAction,
}

impl std::fmt::Display for PartitionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} needs the mandate to still be current, and the enforcing resource is unreachable \
             — from here a withdrawn mandate is indistinguishable from a live one",
            self.action
        )
    }
}

/// **The partition table.** May `action` proceed at `reach`?
///
/// Derived from [`claims_current_authority`], not hand-listed — see the module docs.
pub fn permitted(
    action: CuratorAction,
    reach: Reachability,
) -> Result<(), PartitionRefusal> {
    match reach {
        // Reachable: the resource verifies the mandate inside its own atomic boundary, which is
        // where that decision belongs. Nothing is refused *here* on authority grounds.
        Reachability::Reachable => Ok(()),
        Reachability::Unreachable if claims_current_authority(action) => {
            Err(PartitionRefusal { action })
        }
        Reachability::Unreachable => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The table, written out by hand**, so the derivation cannot quietly redefine the policy.
    /// If [`claims_current_authority`] changes without this changing, the next test fails.
    fn expected_while_unreachable(action: CuratorAction) -> bool {
        match action {
            CuratorAction::ReadLocal => true,
            CuratorAction::PrepareProposal => true,
            CuratorAction::ObserveOwnExpiry => true,
            CuratorAction::PromiseAcceptance => false,
            CuratorAction::CommitCanonical => false,
            CuratorAction::RenewOwnMandate => false,
            CuratorAction::HandOver => false,
        }
    }

    /// The derivation and the hand-written table must agree. Two independent statements of the same
    /// policy, pinned against each other — changing one alone fails.
    #[test]
    fn the_derived_rule_and_the_written_table_agree() {
        for &action in &ALL_ACTIONS {
            let derived = permitted(action, Reachability::Unreachable).is_ok();
            assert_eq!(
                derived,
                expected_while_unreachable(action),
                "the derived rule and the partition table disagree about {action:?}"
            );
        }
    }

    /// **The policy sentence itself**, as the plan states it: a disconnected curator *may prepare
    /// proposals* but *cannot promise canonical acceptance*.
    #[test]
    fn a_disconnected_curator_may_propose_but_may_not_promise() {
        assert!(permitted(CuratorAction::PrepareProposal, Reachability::Unreachable).is_ok());

        let refusal = permitted(CuratorAction::PromiseAcceptance, Reachability::Unreachable)
            .expect_err("promising acceptance while partitioned must be refused");
        assert_eq!(refusal.action, CuratorAction::PromiseAcceptance);
        assert!(
            refusal.to_string().contains("indistinguishable"),
            "the refusal should say why, not just that: {refusal}"
        );
    }

    /// **The asymmetry the whole table rests on.** Expiry is locally decidable; revocation is not.
    ///
    /// This is the payoff for keeping the three lifecycle events apart: had `RoleExpired` and
    /// `PermissionWithdrawn` been one event, this distinction would be unstateable.
    #[test]
    fn expiry_is_locally_decidable_but_revocation_is_not() {
        // Concluding your own term has ended needs nobody.
        assert!(permitted(CuratorAction::ObserveOwnExpiry, Reachability::Unreachable).is_ok());
        // Acting as though you are still appointed does.
        assert!(permitted(CuratorAction::RenewOwnMandate, Reachability::Unreachable).is_err());
    }

    /// **Non-vacuity.** A policy that refused everything while partitioned would satisfy every
    /// safety statement above and make a partitioned curator useless. Something must still be
    /// permitted, and something must still be refused.
    #[test]
    fn the_policy_neither_refuses_everything_nor_permits_everything() {
        let permitted_count = ALL_ACTIONS
            .iter()
            .filter(|&&a| permitted(a, Reachability::Unreachable).is_ok())
            .count();
        assert!(permitted_count > 0, "a partitioned curator must still be able to do something");
        assert!(
            permitted_count < ALL_ACTIONS.len(),
            "a policy that permits everything while partitioned is not a policy"
        );
    }

    /// Reachable is not a rubber stamp *here* — it defers, which is a different thing. The mandate
    /// is still checked, by the resource, inside its atomic boundary. This module never decides
    /// what commits.
    #[test]
    fn reachable_defers_to_the_resource_rather_than_approving() {
        for &action in &ALL_ACTIONS {
            assert!(
                permitted(action, Reachability::Reachable).is_ok(),
                "{action:?}: this module refuses on reachability alone, never on authority — the \
                 resource fence decides that"
            );
        }
    }

    /// Nothing permitted while unreachable touches the protected resource. Stated separately from
    /// the derivation because it is the property a reader actually cares about.
    #[test]
    fn nothing_permitted_while_partitioned_writes_to_the_resource() {
        let writes_to_resource = |a: CuratorAction| {
            matches!(a, CuratorAction::CommitCanonical | CuratorAction::HandOver)
        };
        for &action in &ALL_ACTIONS {
            if permitted(action, Reachability::Unreachable).is_ok() {
                assert!(
                    !writes_to_resource(action),
                    "{action:?} is permitted while partitioned but writes to the resource"
                );
            }
        }
    }
}
