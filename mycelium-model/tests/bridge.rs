//! The bridge to the engine (`mycelium::model_internals`), held to the engine's rules the model rests on: if
//! one of these moves, the engine's decision moved, and the model's bound is restated with it.

use mycelium_model::shared::{self, Acc, Choice, Val};

fn val(proposer: u8, lineage: u64, expires: Option<u64>) -> Val {
    Val { proposer, call: 0, lineage, expires }
}

#[test]
fn a_promise_refuses_a_lower_ballot_and_a_second_proposer_at_the_same_one() {
    let (acc, promised) = shared::prepare(&Acc::default(), 2, 0, 0);
    assert_eq!(promised, Some(None), "a fresh acceptor promises, having accepted nothing");
    assert_eq!(shared::prepare(&acc, 1, 1, 0).1, None, "a lower ballot");
    assert_eq!(shared::prepare(&acc, 2, 1, 0).1, None, "the same ballot, another proposer");
    assert_eq!(shared::prepare(&acc, 2, 0, 0).1, Some(None), "the same ballot, re-sent by the same proposer");
    assert_eq!(shared::prepare(&acc, 3, 1, 3).1, None, "a ballot at the floor");
}

#[test]
fn an_acceptance_is_one_value_per_ballot_and_never_below_the_promise_or_the_floor() {
    let (a, b) = (val(0, 1, None), val(1, 1, None));
    let acc = shared::accept(&Acc::default(), 1, a, 0, 0).expect("a fresh acceptor accepts");
    assert_eq!(acc.accepted, Some((1, a)));
    assert!(shared::accept(&acc, 1, b, 0, 0).is_none(), "another value at the ballot it accepted at");
    assert!(shared::accept(&acc, 1, a, 1, 0).is_none(), "the ballot was promised to another proposer");
    assert!(shared::accept(&acc, 1, a, 0, 0).is_some(), "the same value again");
    assert!(shared::accept(&acc, 2, b, 1, 0).is_some(), "a higher ballot");
    assert!(shared::accept(&acc, 2, b, 1, 2).is_none(), "a ballot at the floor");
    let (promised, reported) = shared::prepare(&acc, 3, 1, 0);
    assert_eq!(promised.accepted, Some((1, a)), "the engine's acceptor keeps the acceptance through a promise");
    assert_eq!(reported, Some(Some((1, a))), "and the engine's promise reports it");
    assert_eq!((promised.promised, promised.promised_to), (3, Some(1)), "promised to the proposer that asked");
    assert!(shared::accept(&promised, 2, b, 1, 0).is_none(), "below the promise");
    let accepted = shared::accept(&promised, 3, b, 1, 0).expect("at the promise, by the proposer it was made to");
    assert_eq!((accepted.promised, accepted.promised_to, accepted.accepted), (3, Some(1), Some((3, b))),
        "the engine's acceptor after an acceptance");
}

#[test]
fn phase_one_carries_the_highest_acceptance_unless_its_decision_is_over() {
    let (mine, low, high) = (val(2, 3, None), val(0, 1, None), val(1, 2, None));
    assert_eq!(shared::phase1(mine, mine, &[], 0), Choice::Keep);
    assert_eq!(shared::phase1(mine, mine, &[(1, low), (2, high)], 0), Choice::Adopt(2, high));
    assert_eq!(shared::phase1(mine, mine, &[(1, low), (2, high)], 1), Choice::Adopt(2, high), "only the ended one is set aside");
    assert_eq!(shared::phase1(mine, mine, &[(1, low), (2, high)], 2), Choice::Keep, "both belong to decisions that are over");
    assert_eq!(shared::phase1(mine, mine, &[(2, low), (2, high)], 0), Choice::Blocked, "two values at one ballot");
}

#[test]
fn a_lease_ends_when_the_readers_clock_passes_it_and_a_release_ends_only_what_it_names() {
    let leased = val(0, 1, Some(4));
    let live = shared::read(&[(1, leased)], &[], 4).expect("a record is held");
    assert!(!live.ended, "at its expiry a lease is still live");
    assert!(shared::read(&[(1, leased)], &[], 5).expect("a record is held").ended, "one tick past it, over");

    let next = val(1, 2, Some(9));
    let top = shared::read(&[(1, leased), (2, next)], &[leased], 4).expect("records are held");
    assert_eq!((top.ballot, top.value, top.ended), (2, next, false), "the highest ballot is the top; another's marker does not end it");
    assert!(shared::read(&[(1, leased), (2, next)], &[next], 4).expect("records are held").ended, "its own marker does");
    assert_eq!(shared::read(&[], &[leased], 4), None, "no record, no top");
}

#[test]
fn an_electorate_counts_its_members_and_needs_a_strict_majority_of_them() {
    assert_eq!((shared::quorum(&[0, 1, 2]), shared::quorum(&[0, 1, 2, 3]), shared::quorum(&[0])), (2, 3, 1));
    assert!(shared::is_member(&[0, 1, 2], 2) && !shared::is_member(&[0, 1, 2], 3));
    let filter = shared::vote_filter(Some(&[0, 1, 2])).expect("a roster is a filter");
    assert!(shared::counted(Some(&filter), 1) && !shared::counted(Some(&filter), 3));
    assert!(shared::vote_filter(None).is_none() && shared::counted(None, 3), "no roster, no filter: anyone counts");
}

/// `model_internals::phase1` is not one engine function: it composes three, in the order the engine's
/// `prepare_phase` does — each report through `Carry::report`, then `set_aside_finished` with the ended
/// record's ballot alone, then `phase1_choice`. Nothing but text ties the two compositions together, so this
/// holds the text: if `prepare_phase` composes them differently, `model_internals::phase1` is changed with it.
/// (The engine's legacy `decided` filter sits between the last two and is left out: legacy acceptances are not
/// modelled.)
#[test]
fn the_phase_one_wrapper_composes_what_the_engines_prepare_phase_composes() {
    let engine = include_str!("../../src/consensus.rs");
    let wrapper = include_str!("../../src/model_internals.rs");
    let body = engine.split("async fn prepare_phase(").nth(1).expect("the engine has a prepare phase")
        .split("\n    }\n").next().expect("which ends");
    let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("prepare_phase no longer says `{needle}`"));
    let report = at("reports.push(carry.report(slot, accepted_ballot, accepted_digest, accepted, accepted_is_envelope));");
    let aside = at("set_aside_finished(&mut reports, 0, false, self.ended(slot));");
    let choice = at("Phase1::Ready(phase1_choice(current, &reports, carry))");
    assert!(report < aside && aside < choice, "report, set aside, choose — in that order");
    assert_eq!(body.matches("set_aside_finished(").count(), 1, "one set-aside, with the ended ballot alone");
    let at = |needle: &str| wrapper.find(needle).unwrap_or_else(|| panic!("the wrapper no longer says `{needle}`"));
    let report = at("carry.report(slot, *b, Some(value_digest(e)), Some(e.clone()), true)");
    let aside = at("set_aside_finished(&mut reported, 0, false, ended);");
    let choice = at("phase1_choice(current, &reported, &carry)");
    assert!(report < aside && aside < choice, "the wrapper: report, set aside, choose — in that order");
}
