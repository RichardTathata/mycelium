//! The `replay` class of `mycelium test --faults` (adopter-surface X6), which exists only under `sim`: a fleet
//! built from a declaration, recorded under the kernel, replayed with the clock paused, and compared decision for
//! decision. Its own binary because `sim` turns the seams on for the whole crate.
#![cfg(feature = "sim")]

use mycelium::faults::{run, Class, ClassState, Options};
use mycelium::wire_check::{CheckOptions, Unit};

#[test]
fn the_replay_class_replays_the_fleet_decision_for_decision() {
    let units = vec![Unit {
        name: "p".into(),
        config: mycelium::NodeCapabilityConfig::from_toml_str("principal = \"p\"\n[[capability]]\nns = \"demo\"\nname = \"thing\"\nttl_secs = 5\n").unwrap(),
    }];
    let opts = Options { classes: vec![Class::Replay], seed: 11, ..Default::default() };
    let report = run(&units, &[], &CheckOptions::default(), &opts).expect("the run");
    let c = &report.classes[0];
    assert_eq!(c.state, ClassState::Passed, "{}", report.render());
    assert_eq!(c.checks.len(), 2, "two checks for the one unit: {}", report.render());
    assert!(c.checks.iter().all(|k| k.ok == Some(true)), "{}", report.render());
    assert_eq!(report.exit_code(), 0);
}
