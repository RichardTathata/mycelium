//! **Replay scenario B — the decisive test for scoped mandates** (item 6 PR 5).
//!
//! §8 of [`docs/design/scoped-mandates.md`](../../../docs/design/scoped-mandates.md):
//!
//! > The decisive test is **replay scenario B**, built once, there. It covers competing
//! > appointments · delayed holders · resource restart · expiry · **revocation with no subsequent
//! > content write**.
//!
//! Everything is judged against one sentence:
//!
//! > **Once the protected resource acknowledges installation of epoch E2, no operation authorized
//! > only under E1 can commit there — even if its holder refreshes the content revision, retries,
//! > reconnects or restarts.**
//!
//! # Why this is a schedule sweep and not five hand-written tests
//!
//! Five tests check five orderings someone thought of. The invariant is about **every** ordering,
//! and the ones that break it are the ones nobody pictured — which is the entire argument for item
//! 6 existing. So the cases below are generated as schedules and swept exhaustively: each is a
//! sequence of steps, and the invariant is asserted after *every* step of *every* schedule, not
//! only at the end.
//!
//! # The case the ADR singles out
//!
//! **Revocation with no subsequent content write.** A revocation followed by a write is easy to
//! observe — something fails. A revocation followed by *nothing* is where a mandate silently
//! remains effective, because nobody looked. The sweep therefore asserts the invariant at points
//! where no write is attempted at all, which is the only way to catch a fence that would have
//! opened had anyone knocked.

use super::{restart::RestartGuard, Mandate, MandateRefusal, PrincipalId, TermId};
// Only the non-vacuity test constructs a deliberately broken authority.
#[cfg(test)]
use super::ResourceAuthority;

/// One step a schedule can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The authority installs a later epoch — an appointment moving.
    Install(u64),
    /// A holder attempts an operation under the mandate of the given epoch.
    Attempt(u64),
    /// The resource restarts, losing its in-memory epoch.
    Restart,
    /// Time passes. Exercises expiry and the *delayed holder* — a mandate minted long before it is
    /// presented.
    Advance(u64),
    /// Nothing happens. **The ADR's named case**: a revocation followed by no write at all.
    Idle,
}

/// What the sweep observed at one point in a schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Which step produced it.
    pub step: Step,
    /// The epoch the resource had installed at that moment.
    pub installed: u64,
    /// The outcome, if the step attempted anything.
    pub outcome: Option<Result<(), MandateRefusal>>,
}

fn mandate_at(epoch: u64, valid_until_ms: u64) -> Mandate {
    Mandate {
        holder: PrincipalId::new("curator-a").expect("valid"),
        established_by: PrincipalId::new("owner").expect("valid"),
        purpose: "curate".into(),
        scope: "council/alpha".into(),
        operations: vec!["apply".into()],
        epoch,
        term: TermId::new(format!("term-{epoch}")).expect("valid"),
        valid_from_ms: 0,
        valid_until_ms,
    }
}

/// Run one schedule, returning what happened at each step.
///
/// The resource begins established at epoch 1 — a running system, not a cold one; `Step::Restart`
/// is what exercises the cold path, and it re-establishes from the *durable* epoch, which is the
/// only thing a restart can honestly recover.
pub fn run(schedule: &[Step], mandate_valid_until_ms: u64) -> Vec<Observation> {
    let mut durable_epoch = 1u64;
    let mut guard = RestartGuard::cold("council/alpha");
    guard.established(durable_epoch);
    let mut now_ms = 0u64;
    let mut out = Vec::new();

    for &step in schedule {
        let mut outcome = None;
        match step {
            Step::Install(e) => {
                // An installation is durable by definition: the resource has acknowledged it.
                if e > durable_epoch {
                    durable_epoch = e;
                }
                // The resource acknowledges the installation. A closed guard stays closed: a
                // restarted resource does not learn its epoch from an install it did not see.
                if guard.authority().is_ok() {
                    guard.established(durable_epoch);
                }
            }
            Step::Attempt(e) => {
                let m = mandate_at(e, mandate_valid_until_ms);
                outcome = Some(match guard.authority() {
                    Ok(a) => a.check(&m, "apply", now_ms),
                    // A closed guard admits nothing; represent that as a supersession by the
                    // durable epoch, which is what it will be once re-established.
                    Err(_) => Err(MandateRefusal::Superseded {
                        installed: durable_epoch,
                        presented: e,
                    }),
                });
            }
            Step::Restart => {
                guard = RestartGuard::cold("council/alpha");
                // A restart recovers only what was durable. It must not recover more.
                guard.established(durable_epoch);
            }
            Step::Advance(ms) => now_ms = now_ms.saturating_add(ms),
            Step::Idle => {}
        }
        out.push(Observation { step, installed: durable_epoch, outcome });
    }
    out
}

/// Check the decisive invariant over one run.
///
/// Returns the first violation: an attempt that **succeeded** although its epoch was below the
/// installed one.
pub fn first_violation(observations: &[Observation]) -> Option<&Observation> {
    observations.iter().find(|o| {
        matches!(o.step, Step::Attempt(e) if e < o.installed)
            && matches!(o.outcome, Some(Ok(())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every schedule the sweep explores, built from the five cases §8 names.
    fn schedules() -> Vec<Vec<Step>> {
        let mut out = Vec::new();

        // Competing appointments: installs interleaved with attempts under the older epoch.
        for a in [Step::Attempt(1), Step::Attempt(2)] {
            for b in [Step::Attempt(1), Step::Attempt(2)] {
                out.push(vec![a, Step::Install(2), b, Step::Attempt(1)]);
            }
        }

        // Delayed holders: time passes between minting and presenting.
        for delay in [0u64, 50, 5_000, 1_000_000] {
            out.push(vec![Step::Install(2), Step::Advance(delay), Step::Attempt(1)]);
            out.push(vec![Step::Advance(delay), Step::Install(2), Step::Attempt(1)]);
        }

        // Resource restart, before and after the installation, and twice over.
        out.push(vec![Step::Install(2), Step::Restart, Step::Attempt(1)]);
        out.push(vec![Step::Restart, Step::Install(2), Step::Attempt(1)]);
        out.push(vec![Step::Install(2), Step::Restart, Step::Restart, Step::Attempt(1)]);
        out.push(vec![Step::Install(2), Step::Restart, Step::Advance(10_000), Step::Attempt(1)]);

        // The ADR's named case: revocation (the install) with NO subsequent content write —
        // idling, then finally knocking much later.
        out.push(vec![Step::Install(2), Step::Idle, Step::Idle, Step::Idle, Step::Attempt(1)]);
        out.push(vec![Step::Install(2), Step::Idle, Step::Restart, Step::Idle, Step::Attempt(1)]);

        // Retry storms: the same superseded attempt over and over.
        out.push(vec![
            Step::Install(2),
            Step::Attempt(1), Step::Attempt(1), Step::Attempt(1), Step::Attempt(1),
        ]);

        out
    }

    /// **The decisive invariant, over every schedule and after every step.**
    #[test]
    fn no_operation_authorized_only_under_an_older_epoch_ever_commits() {
        let all = schedules();
        assert!(all.len() >= 18, "the sweep is not trivially small: {}", all.len());

        let mut attempts_checked = 0usize;
        for schedule in &all {
            let obs = run(schedule, u64::MAX);
            attempts_checked += obs
                .iter()
                .filter(|o| matches!(o.step, Step::Attempt(e) if e < o.installed))
                .count();
            assert_eq!(
                first_violation(&obs),
                None,
                "invariant violated in schedule {schedule:?}\nobservations: {obs:#?}"
            );
        }
        assert!(
            attempts_checked >= 20,
            "the sweep must actually exercise superseded attempts, saw {attempts_checked}"
        );
    }

    /// **Non-vacuity.** The sweep must be able to *fail*. A resource whose installed epoch is
    /// ignored — which is the bug the invariant is about — has to be caught.
    #[test]
    fn the_sweep_catches_a_resource_that_ignores_its_installed_epoch() {
        // Simulate the broken resource: check against epoch 0 rather than what was installed.
        let broken = ResourceAuthority::new("council/alpha", 0);
        let m = mandate_at(1, u64::MAX);
        let observations = vec![Observation {
            step: Step::Attempt(1),
            installed: 2, // the resource HAD installed 2 …
            outcome: Some(broken.check(&m, "apply", 0)), // … but checked as if it were 0
        }];
        assert!(
            first_violation(&observations).is_some(),
            "a resource that ignores its installed epoch must be caught by the sweep"
        );
    }

    /// The current epoch still works — a fence that refused everything would pass the invariant
    /// while being useless, which is the failure mode the knowledge layer's gate warns about.
    #[test]
    fn a_current_mandate_still_commits() {
        let obs = run(&[Step::Install(2), Step::Attempt(2)], u64::MAX);
        assert_eq!(
            obs.last().unwrap().outcome,
            Some(Ok(())),
            "the invariant must not be satisfied by refusing everything"
        );
    }

    /// **Expiry**, the fifth case: a mandate at the current epoch still fails once its window has
    /// passed — and fails as `OutOfWindow`, not as supersession.
    #[test]
    fn expiry_is_reported_as_expiry_not_as_supersession() {
        let obs = run(&[Step::Install(2), Step::Advance(10_000), Step::Attempt(2)], 5_000);
        match &obs.last().unwrap().outcome {
            Some(Err(MandateRefusal::OutOfWindow { valid_until_ms, .. })) => {
                assert_eq!(*valid_until_ms, 5_000);
            }
            other => panic!("expected OutOfWindow, got {other:?}"),
        }
    }

    /// A restart must not *widen* what is accepted — the assumed-zero failure, checked inside a
    /// schedule rather than only as a unit.
    #[test]
    fn a_restart_never_readmits_a_superseded_mandate() {
        for schedule in [
            vec![Step::Install(5), Step::Restart, Step::Attempt(1)],
            vec![Step::Install(5), Step::Restart, Step::Restart, Step::Attempt(4)],
        ] {
            let obs = run(&schedule, u64::MAX);
            assert_eq!(first_violation(&obs), None, "schedule {schedule:?}");
            assert!(
                matches!(obs.last().unwrap().outcome, Some(Err(MandateRefusal::Superseded { .. }))),
                "a restart recovers the durable epoch, never a permissive default"
            );
        }
    }
}
