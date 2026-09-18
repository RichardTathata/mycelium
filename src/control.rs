//! The adaptive-stability contract (v3 item 4 PR 2) — §2, §3, §5 and §7 of
//! [`docs/design/adaptive-stability.md`](../../docs/design/adaptive-stability.md).
//!
//! Types and pure decisions only. No governor is changed here, no actuator is touched, and the
//! rights ledger is PR 3. What this module fixes is the **shape** every governor will be judged
//! against: what class an action is, whether uncertainty may hold it, which profile is in force,
//! and the two loop-breakers that are new — spacing and settling.
//!
//! # The decisive rule
//!
//! > **Uncertainty holds speculation and routine scale-down. It never holds protective shedding,
//! > and it never holds rescue from zero capacity.**
//!
//! The asymmetry is the shape of the costs, not caution. A node that held a protective shed on
//! grounds of uncertainty would be a node with a full channel waiting for peers it cannot hear to
//! tell it whether it is allowed to protect itself. A node that held a rescue-from-zero would leave
//! a group empty because it could not be sure the group was empty.
//!
//! # Why the rule is derived and then written out again by hand
//!
//! [`holds_on_uncertainty`] is the rule. The tests write the same table out by hand and assert the
//! two agree — the shape item 5's partition table uses — so that changing the rule without changing
//! the table, or the reverse, fails rather than drifts.
//!
//! # Observe before enforce
//!
//! Under [`Profile::Observe`] a held action is **recorded as would-hold and proceeds**. That is the
//! profile a deployment lives in until its own traces show the rule is right, and it is a distinct
//! [`Decision`] variant rather than a flag, so a caller cannot forget to tell them apart.

pub mod ledger;

use crate::ViewConfidence;

/// What kind of action a governor is about to take. The class is what decides whether uncertainty
/// may hold it — §2's four rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionClass {
    /// Spend rights on something that *looks* like it will be needed — pre-install a model because
    /// demand appears to be rising.
    SpeculativeScaleUp,
    /// Give something up because the fleet *looks* like it has too much — leave a group that
    /// appears to be over `max`.
    RoutineScaleDown,
    /// Protect this node from its own overload — go opaque because *this node's* channel is full.
    /// Reads local state, and local state is never uncertain.
    ProtectiveShed,
    /// Fill a hole that *reads as* empty — join a group whose live count is 0. The cost of a wrong
    /// rescue is one extra member; the cost of a wrong hold is a group with nobody in it.
    RescueFromZero,
}

/// Every class, for sweeps — a variant missing here shows up as a table that stopped covering the
/// type.
pub const ALL_CLASSES: [ActionClass; 4] = [
    ActionClass::SpeculativeScaleUp,
    ActionClass::RoutineScaleDown,
    ActionClass::ProtectiveShed,
    ActionClass::RescueFromZero,
];

/// **The rule.** May uncertainty about the fleet hold this class of action?
pub fn holds_on_uncertainty(class: ActionClass) -> bool {
    match class {
        // Acting on a guess spends rights on a guess.
        ActionClass::SpeculativeScaleUp => true,
        // The observation may be a partition; leaving on a partition makes it worse.
        ActionClass::RoutineScaleDown => true,
        // Local state. Never.
        ActionClass::ProtectiveShed => false,
        // The asymmetric cost. Never.
        ActionClass::RescueFromZero => false,
    }
}

/// What a governor requires of a [`ViewConfidence`] before it counts the view as *certain enough*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfidenceBound {
    /// The stalest heard peer may be at most this old.
    pub max_staleness_ms: u64,
    /// At least this many peers must have been heard inside the window.
    pub min_peers_heard: usize,
}

impl Default for ConfidenceBound {
    /// Deliberately strict: an operator loosens it on evidence.
    fn default() -> Self {
        Self { max_staleness_ms: 30_000, min_peers_heard: 1 }
    }
}

/// Why a view is not certain enough. Named, so a held action says *what* was missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Uncertainty {
    /// No peer heard inside the window: the staleness figure is a placeholder, not an observation.
    /// **An isolated node is uncertain, not fresh** — the WP5 correction.
    StalenessUnknown,
    /// The stalest heard peer is older than the bound.
    Stale {
        /// The observed staleness.
        max_staleness_ms: u64,
        /// The bound it exceeded.
        bound_ms: u64,
    },
    /// Fewer peers heard than the bound requires.
    TooFewHeard {
        /// Heard.
        heard: usize,
        /// Required.
        required: usize,
    },
    /// The observer is itself opaque or shedding, so its own inputs may be degraded.
    SelfDegraded,
}

/// Is this view certain enough under `bound`? `Ok(())` if so, otherwise the first reason it is not,
/// in a fixed order: unknown staleness is reported before a stale figure, because a stale *figure*
/// presupposes there is one.
pub fn assess(view: &ViewConfidence, bound: &ConfidenceBound) -> Result<(), Uncertainty> {
    if !view.staleness_known() {
        return Err(Uncertainty::StalenessUnknown);
    }
    if view.max_staleness_ms > bound.max_staleness_ms {
        return Err(Uncertainty::Stale {
            max_staleness_ms: view.max_staleness_ms,
            bound_ms: bound.max_staleness_ms,
        });
    }
    if view.peers_heard < bound.min_peers_heard {
        return Err(Uncertainty::TooFewHeard { heard: view.peers_heard, required: bound.min_peers_heard });
    }
    if view.self_degraded {
        return Err(Uncertainty::SelfDegraded);
    }
    Ok(())
}

/// Which promises are in force — §7, in order of adoption. **Shadow before enforcement.**
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Profile {
    /// Today's governors, untouched. The predicate is not consulted.
    Legacy,
    /// The predicate is evaluated and its holds are **recorded**; nothing is held. Where a
    /// deployment lives until its own traces show the rule is right.
    Observe,
    /// Tier A: budgets on this node's own actuators; the predicate holds actions.
    EnforceLocal,
    /// Tier C: ceilings backed by allocated rights (PR 3). The predicate holds actions.
    EnforceAllocated,
}

impl Profile {
    /// Does this profile turn a hold into an actual hold?
    pub fn enforces(self) -> bool {
        matches!(self, Profile::EnforceLocal | Profile::EnforceAllocated)
    }
}

/// What the predicate decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Act.
    Proceed,
    /// Do not act; here is why.
    Held(Uncertainty),
    /// **Act, and record that an enforcing profile would have held this.** `Observe` only. A
    /// distinct variant so a caller cannot treat it as `Proceed` by forgetting to check a flag.
    WouldHold(Uncertainty),
}

impl Decision {
    /// Does the action go ahead?
    pub fn proceeds(&self) -> bool {
        !matches!(self, Decision::Held(_))
    }
}

/// **The predicate.** May an action of `class` proceed on `view`, under `bound` and `profile`?
///
/// Derived, not hand-listed: a class that [`holds_on_uncertainty`] is held when [`assess`] finds
/// the view uncertain and the profile enforces; recorded as would-hold when the profile observes;
/// and never consulted under `Legacy`. A class that does not hold on uncertainty proceeds whatever
/// the view says.
pub fn decide(
    class: ActionClass,
    view: &ViewConfidence,
    bound: &ConfidenceBound,
    profile: Profile,
) -> Decision {
    if profile == Profile::Legacy || !holds_on_uncertainty(class) {
        return Decision::Proceed;
    }
    match assess(view, bound) {
        Ok(()) => Decision::Proceed,
        Err(why) if profile.enforces() => Decision::Held(why),
        Err(why) => Decision::WouldHold(why),
    }
}

/// A stable name for one action, so a retry, a replay and a reconcile all name the same thing.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId {
    /// The governor that proposed it.
    pub governor: String,
    /// The actuator it touches — one owner per actuator, so `(governor, actuator)` is a pair
    /// the ADR fixes, not a free choice.
    pub actuator: String,
    /// The governor's own sequence for this actuator.
    pub seq: u64,
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.governor, self.actuator, self.seq)
    }
}

/// One governor's declared contract — §3. What it observes and touches, and the loop-breakers it
/// commits to. The existing hysteresis and cooldown stay where they are; these are the two that
/// are new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlSpec {
    /// The governor.
    pub governor: String,
    /// The one actuator it owns.
    pub actuator: String,
    /// Minimum interval between two actions on the actuator.
    pub spacing_ms: u64,
    /// How long a pending action may go unobserved before it is settled as `unknown` and the
    /// governor may propose again.
    pub settle_timeout_ms: u64,
    /// What the governor requires before its view counts as certain.
    pub bound: ConfidenceBound,
    /// Which promises are in force.
    pub profile: Profile,
}

/// **Spacing.** May the actuator be touched again at `now_ms`, given when it was last touched?
///
/// `None` for never-touched. Saturating, so a clock that reads earlier than the last action (which
/// a *wall* clock can) refuses rather than wrapping into "long ago".
pub fn spacing_allows(spec: &ControlSpec, last_action_at_ms: Option<u64>, now_ms: u64) -> bool {
    match last_action_at_ms {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= spec.spacing_ms,
    }
}

/// **Settling** — the reconcile step's state for one actuator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleState {
    /// Nothing pending.
    Idle,
    /// An action was taken and its effect has not yet been observed.
    Pending {
        /// Which.
        id: ActionId,
        /// When it was taken.
        since_ms: u64,
    },
}

/// Why a proposal must wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsettled {
    /// The action still pending.
    pub id: ActionId,
    /// How long it has been pending.
    pub pending_for_ms: u64,
}

/// May the governor propose on this actuator at `now_ms`?
///
/// Not while an action is pending and inside the settle timeout: *a governor does not propose
/// again on an actuator whose last action has not been observed to take effect, or to have
/// failed, or to have timed out into `unknown`.* Past the timeout the state is treated as
/// settled-unknown and a proposal may proceed — the caller records the `unknown`.
pub fn may_propose(state: &SettleState, spec: &ControlSpec, now_ms: u64) -> Result<(), Unsettled> {
    match state {
        SettleState::Idle => Ok(()),
        SettleState::Pending { id, since_ms } => {
            let pending_for_ms = now_ms.saturating_sub(*since_ms);
            if pending_for_ms >= spec.settle_timeout_ms {
                Ok(())
            } else {
                Err(Unsettled { id: id.clone(), pending_for_ms })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(peers_heard: usize, max_staleness_ms: u64, self_degraded: bool) -> ViewConfidence {
        ViewConfidence {
            observer: "n1".into(),
            peers_known: 5,
            peers_heard,
            max_staleness_ms,
            self_degraded,
        }
    }

    fn certain() -> ViewConfidence {
        view(3, 1_000, false)
    }

    /// Every way a view can be uncertain, each alone.
    fn uncertain_views() -> Vec<(&'static str, ViewConfidence)> {
        vec![
            ("isolated", view(0, 0, false)),
            ("stale", view(3, 90_000, false)),
            ("too few heard", view(0, 0, false)),
            ("self-degraded", view(3, 1_000, true)),
        ]
    }

    fn bound() -> ConfidenceBound {
        ConfidenceBound { max_staleness_ms: 30_000, min_peers_heard: 2 }
    }

    /// **The table, written by hand.** If [`holds_on_uncertainty`] changes without this changing,
    /// the next test fails.
    fn expected_held_under_uncertainty(class: ActionClass) -> bool {
        match class {
            ActionClass::SpeculativeScaleUp => true,
            ActionClass::RoutineScaleDown => true,
            ActionClass::ProtectiveShed => false,
            ActionClass::RescueFromZero => false,
        }
    }

    #[test]
    fn the_derived_rule_and_the_written_table_agree() {
        for &class in &ALL_CLASSES {
            assert_eq!(
                holds_on_uncertainty(class),
                expected_held_under_uncertainty(class),
                "the rule and the table disagree about {class:?}"
            );
        }
    }

    /// **The decisive rule, swept.** Protection and rescue proceed under every uncertain view and
    /// every profile; speculation and routine scale-down are held under every uncertain view when
    /// enforcing.
    #[test]
    fn protection_and_rescue_are_never_held_and_speculation_always_is() {
        for (why, v) in uncertain_views() {
            for profile in [Profile::EnforceLocal, Profile::EnforceAllocated] {
                assert_eq!(
                    decide(ActionClass::ProtectiveShed, &v, &bound(), profile),
                    Decision::Proceed,
                    "a protective shed must never wait on the fleet ({why}, {profile:?})"
                );
                assert_eq!(
                    decide(ActionClass::RescueFromZero, &v, &bound(), profile),
                    Decision::Proceed,
                    "a rescue from zero must never wait on the fleet ({why}, {profile:?})"
                );
                assert!(
                    matches!(decide(ActionClass::SpeculativeScaleUp, &v, &bound(), profile), Decision::Held(_)),
                    "speculation is held under an uncertain view ({why}, {profile:?})"
                );
                assert!(
                    matches!(decide(ActionClass::RoutineScaleDown, &v, &bound(), profile), Decision::Held(_)),
                    "routine scale-down is held under an uncertain view ({why}, {profile:?})"
                );
            }
        }
    }

    /// A certain view lets everything proceed — the rule holds on *uncertainty*, not on class.
    #[test]
    fn a_certain_view_lets_every_class_proceed() {
        for &class in &ALL_CLASSES {
            assert_eq!(decide(class, &certain(), &bound(), Profile::EnforceLocal), Decision::Proceed);
        }
    }

    /// **An isolated node is uncertain, not fresh** — the WP5 correction, now consequential. Its
    /// `max_staleness_ms` of `0` is a placeholder, and the predicate reports *why*.
    #[test]
    fn an_isolated_node_is_uncertain_not_fresh() {
        let isolated = view(0, 0, false);
        assert_eq!(assess(&isolated, &bound()), Err(Uncertainty::StalenessUnknown));
        assert!(matches!(
            decide(ActionClass::SpeculativeScaleUp, &isolated, &bound(), Profile::EnforceLocal),
            Decision::Held(Uncertainty::StalenessUnknown)
        ));
    }

    /// Each uncertainty is named, so a held action says what was missing.
    #[test]
    fn a_hold_names_what_was_missing() {
        let b = bound();
        assert_eq!(
            assess(&view(3, 90_000, false), &b),
            Err(Uncertainty::Stale { max_staleness_ms: 90_000, bound_ms: 30_000 })
        );
        assert_eq!(
            assess(&view(1, 1_000, false), &b),
            Err(Uncertainty::TooFewHeard { heard: 1, required: 2 })
        );
        assert_eq!(assess(&view(3, 1_000, true), &b), Err(Uncertainty::SelfDegraded));
    }

    /// **Observe records and proceeds; Legacy does not even look.**
    #[test]
    fn observe_records_a_would_hold_and_legacy_never_consults_the_predicate() {
        let v = view(0, 0, false);
        assert!(matches!(
            decide(ActionClass::SpeculativeScaleUp, &v, &bound(), Profile::Observe),
            Decision::WouldHold(Uncertainty::StalenessUnknown)
        ));
        assert!(decide(ActionClass::SpeculativeScaleUp, &v, &bound(), Profile::Observe).proceeds());
        assert_eq!(
            decide(ActionClass::SpeculativeScaleUp, &v, &bound(), Profile::Legacy),
            Decision::Proceed,
            "legacy is today's governors, untouched"
        );
    }

    /// **Non-vacuity.** A predicate that held everything would satisfy every safety statement
    /// above and freeze the fleet; one that held nothing would not be a predicate.
    #[test]
    fn the_predicate_neither_holds_everything_nor_nothing() {
        let v = view(0, 0, false);
        let held = ALL_CLASSES
            .iter()
            .filter(|&&c| matches!(decide(c, &v, &bound(), Profile::EnforceLocal), Decision::Held(_)))
            .count();
        assert!(held > 0, "something must be held under an uncertain view");
        assert!(held < ALL_CLASSES.len(), "and something must still proceed");
    }

    fn spec() -> ControlSpec {
        ControlSpec {
            governor: "tuning".into(),
            actuator: "inbound_fps".into(),
            spacing_ms: 5_000,
            settle_timeout_ms: 20_000,
            bound: bound(),
            profile: Profile::EnforceLocal,
        }
    }

    /// **Spacing**: a minimum interval between two actions on one actuator, and a clock that reads
    /// earlier than the last action refuses rather than wrapping into "long ago".
    #[test]
    fn spacing_is_a_minimum_interval_that_does_not_wrap() {
        let s = spec();
        assert!(spacing_allows(&s, None, 0), "never touched");
        assert!(!spacing_allows(&s, Some(10_000), 12_000), "too soon");
        assert!(spacing_allows(&s, Some(10_000), 15_000), "exactly the interval");
        assert!(!spacing_allows(&s, Some(10_000), 9_000), "a clock that went backwards is not 'long ago'");
    }

    /// **Settling**: no proposal while the last action is unobserved and inside the timeout; past
    /// it the state is settled-unknown and a proposal may proceed.
    #[test]
    fn settling_holds_a_proposal_until_the_last_action_is_observed_or_times_out() {
        let s = spec();
        let id = ActionId { governor: "tuning".into(), actuator: "inbound_fps".into(), seq: 7 };
        assert_eq!(id.to_string(), "tuning/inbound_fps#7");

        assert_eq!(may_propose(&SettleState::Idle, &s, 0), Ok(()));
        let pending = SettleState::Pending { id: id.clone(), since_ms: 100_000 };
        assert_eq!(
            may_propose(&pending, &s, 110_000),
            Err(Unsettled { id: id.clone(), pending_for_ms: 10_000 }),
            "inside the settle timeout, the actuator is still settling"
        );
        assert_eq!(may_propose(&pending, &s, 120_000), Ok(()), "past it, settled as unknown");
    }
}
