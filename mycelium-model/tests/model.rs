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
use stateright::{Checker, HasDiscoveries, Model, Path};

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// Explores every reachable state of `model`, depth first (the visited set is all it keeps), and holds it to its
/// properties: no invariant broken anywhere, every reachability property reached somewhere.
fn explore<M>(name: &str, model: M, states: usize)
where
    M: Model + Send + Sync + 'static,
    M::State: Clone + Debug + Hash + PartialEq + Send + Sync + 'static,
    M::Action: Clone + Debug + PartialEq + Send + Sync + 'static,
{
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
        .unwrap_or_else(|| panic!("the planted fault went unseen: no trace breaks \"{invariant}\""));
    eprintln!("planted fault found — \"{invariant}\" broken in {} steps:\n{path}", path.clone().into_actions().len());
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
    explore("acceptor", SlotModel::acceptor(), 7_842_599);
}

/// Slot reuse after a lapsed lease, every message its own step.
#[test]
fn a_slot_is_reused_after_a_lapsed_lease_and_each_life_has_one_value() {
    explore("reuse after a lapse", SlotModel::reuse_after_lapse(), 3_311_813);
}

/// Slot reuse after a release, every message its own step.
#[test]
fn a_slot_is_reused_after_a_release_and_each_life_has_one_value() {
    explore("reuse after a release", SlotModel::reuse_after_release(), 4_148_304);
}

/// Slot reuse across three lives — releases and lapses together — each ballot one step.
#[test]
fn a_slot_is_reused_across_three_lives() {
    explore("reuse, three lives", SlotModel::reuse(), 443_295);
}

/// The lease under skew, with the holder's margin as CR11 designs it: no second holder while the first acts.
#[test]
fn a_lease_under_skew_has_one_holder_with_the_margin_as_designed() {
    explore("lease under skew", SlotModel::lease(), 1_154_503);
}

/// The migration: the predecessor's ballot races its retirement; the successor carries, completes, decides.
#[test]
fn a_migration_decides_nothing_early_and_keeps_the_predecessors_value() {
    explore("migration", MigrationModel::rolling(), 4_277_735);
}

/// The migration with two proposers contending in the predecessor while it retires.
#[test]
fn a_migration_carries_the_value_two_contending_proposers_decided() {
    explore("migration, contended", MigrationModel::contended(), 7_938_958);
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
