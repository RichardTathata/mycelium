//! The model, run (critique plan row CR10): every configuration explored in full with its faults out — and,
//! with one fault planted in each area, a counterexample found. CI runs this file in release
//! (`cargo test -p mycelium-model --release`).
//!
//! Each passing run pins the number of distinct states it explored. The number is the bound: a change to a
//! configuration, to the model or to an engine function the model calls moves it, and is then made here, in
//! the open, with `docs/operations/what-is-proven.md`.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::time::Instant;

use mycelium_model::migration::{self, MigrationModel};
use mycelium_model::slot::{self, SlotModel};
use stateright::{Checker, Expectation, HasDiscoveries, Model, Path};

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// Explores every reachable state of `model`, depth first (the visited set is all it keeps), and holds it to its
/// properties: no invariant broken anywhere, every reachability property reached somewhere.
fn explore<M>(name: &str, model: M, states: usize, reaches: &[&'static str])
where
    M: Model + Send + Sync + 'static,
    M::State: Clone + Debug + Hash + PartialEq + Send + Sync + 'static,
    M::Action: Clone + Debug + PartialEq + Send + Sync + 'static,
{
    // The reachability properties a configuration carries follow from its fields; the run names the list it
    // expects, so a configuration that stopped carrying one fails here instead of passing with less.
    let carried: BTreeSet<&str> = model.properties().iter()
        .filter(|p| p.expectation == Expectation::Sometimes).map(|p| p.name).collect();
    assert_eq!(carried, reaches.iter().copied().collect(), "{name}: the reachability properties this run must carry");
    let started = Instant::now();
    let checker = model.checker().threads(threads()).spawn_dfs().join();
    eprintln!(
        "model {name}: {} distinct states, longest path {} steps, {:.1}s on {} threads",
        checker.unique_state_count(), checker.max_depth(), started.elapsed().as_secs_f32(), threads(),
    );
    checker.assert_properties();
    assert_eq!(
        checker.unique_state_count(), states,
        "{name}: the bound moved — pin the new count here and restate it in docs/operations/what-is-proven.md",
    );
}

/// The shortest trace on which `model` breaks `invariant` (breadth first, stopping at the first).
fn counterexample<M>(model: M, invariant: &'static str) -> Path<M::State, M::Action>
where
    M: Model + Send + Sync + 'static,
    M::State: Clone + Debug + Hash + PartialEq + Send + Sync + 'static,
    M::Action: Clone + Debug + PartialEq + Send + Sync + 'static,
{
    let checker = model.checker().threads(threads())
        .finish_when(HasDiscoveries::AnyOf(BTreeSet::from([invariant])))
        .spawn_bfs().join();
    let path = checker.discovery(invariant)
        .unwrap_or_else(|| panic!("no trace breaks \"{invariant}\": the fault went unseen"));
    eprintln!("counterexample — \"{invariant}\" broken in {} steps:\n{path}", path.clone().into_actions().len());
    path
}

fn decided(s: &slot::SlotState) -> BTreeSet<(u64, slot::Msg)> {
    s.net.iter().filter_map(|m| match m { slot::Msg::Commit { ballot, .. } => Some((*ballot, *m)), _ => None }).collect()
}

// ── Without the faults: the model passes ─────────────────────────────────────────────────────────────

/// Inside one life — nothing releases and no lease runs out — two proposers, retried ballots and adopted
/// reports decide one value, and the outsider's answers are never counted.
#[test]
fn the_acceptor_decides_one_value_inside_a_life() {
    explore("acceptor", SlotModel::acceptor(), 7_842_599, &[slot::DECIDED, slot::REDECIDED]);
}

/// Slot reuse after a lapsed lease, every message its own step.
#[test]
fn a_slot_is_reused_after_a_lapsed_lease_and_each_life_has_one_value() {
    explore("reuse after a lapse", SlotModel::reuse_after_lapse(), 3_311_813,
        &[slot::DECIDED, slot::REUSED_AFTER_LAPSE, slot::SECOND_HOLDER]);
}

/// Slot reuse after a release, every message its own step.
#[test]
fn a_slot_is_reused_after_a_release_and_each_life_has_one_value() {
    explore("reuse after a release", SlotModel::reuse_after_release(), 4_148_304,
        &[slot::DECIDED, slot::REUSED_AFTER_RELEASE, slot::SECOND_HOLDER]);
}

/// Slot reuse across three lives — releases and lapses together — each ballot one step.
#[test]
fn a_slot_is_reused_across_three_lives() {
    explore("reuse, three lives", SlotModel::reuse(), 443_295,
        &[slot::DECIDED, slot::REUSED_AFTER_LAPSE, slot::REUSED_AFTER_RELEASE, slot::SECOND_HOLDER, slot::THIRD_LIFE]);
}

/// The lease under skew, with the holder's margin as CR11 designs it: no second holder while the first acts.
#[test]
fn a_lease_under_skew_has_one_holder_with_the_margin_as_designed() {
    explore("lease under skew", SlotModel::lease(), 1_154_503,
        &[slot::DECIDED, slot::REUSED_AFTER_LAPSE, slot::REUSED_AFTER_RELEASE, slot::SECOND_HOLDER]);
}

/// The lease one tick from the edge: on whole ticks a margin of δ − 1 is the least that holds.
#[test]
fn a_lease_under_skew_holds_at_the_least_margin_whole_ticks_allow() {
    explore("lease, at the edge", SlotModel::lease_edge(), 159_181, &[slot::DECIDED, slot::REUSED_AFTER_LAPSE, slot::SECOND_HOLDER]);
}

/// The migration: the predecessor's ballot races its retirement; the successor carries, completes, decides.
#[test]
fn a_migration_decides_nothing_early_and_keeps_the_predecessors_value() {
    explore("migration", MigrationModel::rolling(), 4_277_735, &[migration::CARRIED_OVER, migration::SUCCESSOR_DECIDES]);
}

/// The migration with a carry ballot that stalls and is run again.
#[test]
fn a_carry_ballot_that_did_not_decide_is_run_again() {
    explore("migration, stalled carry", MigrationModel::stalled_carry(), 11_796_281,
        &[migration::CARRIED_OVER, migration::CARRY_RUN_AGAIN]);
}

/// The migration with two proposers contending in the predecessor while it retires — **ballots to 2**, so the
/// carry (which runs above every ballot the manifests name) carries a ballot-1 decision only. A ballot-2 decision
/// carried needs ballots to 3: 110,929,684 states and about eight minutes on twelve cores, run once by hand on
/// 2026-11-14 — and it is where the finding below was made.
#[test]
fn a_migration_carries_the_value_two_contending_proposers_decided() {
    explore("migration, contended", MigrationModel::contended(), 7_938_958,
        &[migration::CARRIED_OVER, migration::SUCCESSOR_DECIDES]);
}

// ── Found by the model, 2026-11-14, confirmed on the engine — no fault planted ───────────────────────────────────────────────

/// **A finding, not a plant: found by the model on 2026-11-14 and confirmed on the engine the same day.**
///
/// With overlapping electorates, the successor's carry can decide a value the predecessor did not. Node 1 — in
/// both groups — accepts `v1` in the predecessor at ballot 1, which no quorum follows; nodes 0 and 2 decide `v0`
/// at ballot 2; the manifests name `v0` as the highest acceptance and the carry runs at ballot 3 — with node 1 as
/// the rest of its quorum. Node 1's promise reports the acceptance it holds, `v1`@1; phase 1 carries the highest
/// report of its promise quorum (`choose_after_prepare`) and knows nothing of the manifests, so the successor
/// decides `v1`.
///
/// In the engine this needs `v0`'s decision record not yet at the carry's quorum (an acceptor holding a live
/// record of another value refuses), and it is counted afterwards (`commit_conflicts`, `Carried::Conflict`).
///
/// **This test asserts the counterexample is found.** The fix is decided and is its own change: the acceptor
/// record carries the group each acceptance was made in, and a carry hears only acceptances made in the
/// successor. **When that has merged, the model gives each acceptance its group and this test is inverted**:
/// `overlap()` goes through `explore` like every other configuration.
#[test]
fn found_a_carry_adopts_a_stale_acceptance_of_an_overlapping_member_and_the_groups_decide_two_values() {
    let model = MigrationModel::overlap();
    let path = counterexample(model.clone(), migration::ONE_VALUE_ACROSS);
    let steps = path.into_vec();
    let decided_in = |state: &migration::MigrationState, successor: bool| -> BTreeSet<_> {
        state.net.iter().filter_map(|m| match m {
            migration::Msg::Commit { successor: s, value, .. } if *s == successor => Some(*value),
            _ => None,
        }).collect()
    };
    // The step at which the second value is decided (the search is parallel, so the trace may run a step on).
    let at = steps.iter().position(|(state, _)| decided_in(state, false) != decided_in(state, true)
        && !decided_in(state, false).is_empty() && !decided_in(state, true).is_empty())
        .expect("a state in which the two groups have decided different values");
    let (last, _) = &steps[at];
    let (before, carry) = &steps[at - 1];
    assert!(matches!(carry, Some(migration::Action::Ballot(migration::Role::Carry, 3, _, false))),
        "the step that breaks it is the carry ballot, above the predecessor's ballots: {carry:?}");
    let decided = |successor: bool| decided_in(last, successor);
    let (v0, v1) = (decided(false), decided(true));
    assert_eq!((v0.len(), v1.len()), (1, 1), "each group decided one value: {v0:?} {v1:?}");
    assert_ne!(v0, v1, "and not the same one");
    let (v0, v1) = (*v0.first().unwrap(), *v1.first().unwrap());
    assert_eq!((v0.proposer, v1.proposer), (0, 1), "the predecessor decided node 0's value, the successor node 1's");
    assert_eq!(before.manifests.1.map(|(_, v)| v), Some(v0), "the manifests named the decided value as the one to carry");
    let quorum: Vec<usize> = (0..last.nodes.len()).filter(|n| last.nodes[*n].accepted_carry).collect();
    assert_eq!(quorum.len(), 2, "the carry's quorum: {quorum:?}");
    assert!(quorum.iter().all(|n| before.nodes[*n].acc.accepted.map(|(_, v)| v) != Some(v0)),
        "no member of the carry's quorum held the decided value");
    assert!(quorum.iter().any(|n| before.nodes[*n].acc.accepted == Some((1, v1))),
        "and one of them held the stale acceptance, made in the predecessor at ballot 1");
}

// ── With one fault planted in each area: the model fails ─────────────────────────────────────────────

/// **Planted: a vote counted from outside the electorate.** Two values are decided in one life.
#[test]
fn planted_a_vote_counted_from_outside_the_electorate_decides_two_values_in_one_life() {
    let path = counterexample(SlotModel::acceptor().with_outside_votes_counted(), slot::ONE_VALUE_PER_LIFE);
    let last = path.last_state();
    let values: BTreeSet<_> = decided(last).into_iter().map(|(_, m)| match m {
        slot::Msg::Commit { value, .. } => value,
        _ => unreachable!("decided holds commits"),
    }).collect();
    assert!(values.len() >= 2, "two values are decided: {values:?}");
    assert!(last.nodes.iter().all(|n| n.clock == 0) && !last.net.iter().any(|m| matches!(m, slot::Msg::Marker { .. })),
        "and nothing ended the first: no clock moved, nothing was released");
    assert!(path.into_actions().iter().any(|a| matches!(a, slot::Action::Outsider(_))), "through the outsider's answer");
}

/// **Planted: a lease ended with no margin** — the engine as it stands, until CR11. A second holder is granted
/// while the first is still inside its own deadline.
#[test]
fn planted_a_lease_ended_with_no_margin_grants_a_second_holder_inside_the_firsts_deadline() {
    let model = SlotModel::lease().with_no_margin();
    let path = counterexample(model.clone(), slot::ONE_HOLDER);
    let last = path.last_state();
    let holders: Vec<_> = last.nodes.iter().filter(|n| slot::holding(n, model.margin)).collect();
    assert_eq!(holders.len(), 2, "two nodes hold at once: {last:?}");
    let clocks: Vec<u64> = last.nodes.iter().map(|n| n.clock).collect();
    let apart = clocks.iter().max().unwrap() - clocks.iter().min().unwrap();
    assert!(apart <= model.skew, "with every clock inside the skew bound: {clocks:?}");
    // And with the margin, the same two nodes are not both holding: the first has stopped.
    assert!(last.nodes.iter().filter(|n| slot::holding(n, model.skew)).count() <= 1,
        "the margin as designed would have stopped the first holder by now");
}

/// **Planted: a successor deciding early.** The successor decides before the predecessor's retirement is
/// visible — and, carried through, the two groups decide two values for one slot.
#[test]
fn planted_a_successor_deciding_early_decides_before_the_retirement_is_visible() {
    let model = MigrationModel::rolling().with_successor_deciding_early();
    let path = counterexample(model.clone(), migration::RETIREMENT_FIRST);
    let last = path.last_state();
    assert!(last.nodes.iter().any(|n| n.accepted_own && !n.sees_retirement && !n.sees_completion),
        "a successor member accepted the successor's own value with no retirement visible: {last:?}");

    let path = counterexample(model, migration::ONE_VALUE_ACROSS);
    let values: BTreeSet<_> = path.last_state().net.iter().filter_map(|m| match m {
        migration::Msg::Commit { successor, value, .. } => Some((*successor, *value)),
        _ => None,
    }).collect();
    assert!(values.iter().any(|(succ, v)| !succ && values.iter().any(|(other, w)| *other && v != w)),
        "the predecessor and the successor decided different values: {values:?}");
}

/// **Planted beside the row's three: a completion that proves nothing** — a successor member votes for it with
/// neither the carry done nor the retirement visible. The two guard invariants cannot see it (no successor
/// ballot has been accepted yet, and when one is, the completion is visible); the proof invariant does.
#[test]
fn planted_a_completion_that_proves_nothing_breaks_the_proof_invariant() {
    let model = MigrationModel::rolling().with_completion_unproven();
    let path = counterexample(model.clone(), migration::COMPLETION_PROVES);
    let last = path.last_state();
    assert!(last.nodes.iter().any(|n| n.sees_completion), "a completion is visible: {last:?}");
    assert!(!(model.retired(last) && last.carried), "and the retirement is not decided, or the carry not done");
    assert!(last.nodes.iter().all(|n| !(n.accepted_carry || n.accepted_own) || n.sees_retirement || n.sees_completion),
        "while the guard invariant still holds in that state");
}

/// **Planted beside the row's three: a margin one tick short of the least** whole ticks allow (δ − 2). The
/// passing run beside it, `lease_edge` at δ − 1, and this one are what tell the two margins apart; `lease`, at
/// δ, cannot.
#[test]
fn planted_a_margin_one_tick_short_of_the_least_grants_a_second_holder() {
    let model = SlotModel { margin: 1, ..SlotModel::lease_edge() };
    let path = counterexample(model.clone(), slot::ONE_HOLDER);
    let last = path.last_state();
    assert_eq!(last.nodes.iter().filter(|n| slot::holding(n, model.margin)).count(), 2, "two nodes hold at once: {last:?}");
    let clocks: Vec<u64> = last.nodes.iter().map(|n| n.clock).collect();
    assert!(clocks.iter().max().unwrap() - clocks.iter().min().unwrap() <= model.skew, "inside the skew bound: {clocks:?}");
}
