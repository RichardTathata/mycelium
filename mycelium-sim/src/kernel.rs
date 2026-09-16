//! The event kernel — one decision at a time, recorded or checked.
//!
//! # What makes this a replay harness rather than a re-seeder
//!
//! In [`Mode::Replay`] the kernel does **not** re-run the production code's own source of
//! randomness and hope it agrees. It **checks each request against the recorded next entry** and
//! then *supplies* the recorded result. A request for `rng/jitter` when the trace says `rng/nonce`,
//! an `fs` append whose content digest differs, a different offset, a flipped `sync` flag — each is
//! a divergence, and the run stops with both sides printed.
//!
//! The distinction is the whole point of item 6. A harness that re-seeds reproduces a run *only
//! while the code is unchanged*, which is precisely when you do not need it. One that checks
//! requests tells you **where** changed code first departed from the recorded run, which is the
//! question a reviewer actually has.
//!
//! PR 2's gate is the test that separates the two: record a run, replay it with one WAL record's
//! bytes changed **at equal length**, and require a divergence. A harness that passes that
//! unchanged is not detecting divergence.
//!
//! # Five-part statement
//!
//! *Guarantee:* in `Replay`, every decision the production code asks for is compared with the
//! recorded one by node, kind, stream and canonical request, and the run stops at the first
//! mismatch; the value returned is always the recorded one, never a freshly computed one.
//! *Assumptions:* the production code reaches the kernel for every nondeterministic decision — a
//! call that bypasses it is invisible here, which is what PR 3's static forbidden-call check exists
//! to catch. *Enforcing component:* [`Kernel::decide`]. *Failure behaviour:* a mismatch or an
//! exhausted trace returns [`Divergence`] and the caller stops; the kernel never guesses a result.
//! *Detecting tests:* this module's `tests`, and the same-length/different-content gate in
//! `tests/divergence.rs`. *Strength:* it governs this harness's own replay; it prevents nothing in
//! production, which does not link it.

use crate::trace::{Choice, ChoiceKind, Trace};
use std::fmt;

/// How the kernel is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run the production code for real and write down what it asked and what it got.
    Record,
    /// Check each request against the trace and supply the recorded result.
    Replay,
}

/// Where a replay departed from the recorded run.
///
/// Carries **both sides** because the useful question is never "did it diverge" but "what changed",
/// and an error that prints only the expectation makes the reader go and find the other half.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    /// The position in the trace at which this happened.
    pub seq: u64,
    /// What the recorded run did here. `None` when the trace ran out — the replay asked for more
    /// decisions than the recording contained, which is itself a divergence.
    ///
    /// Boxed so `Divergence` stays small: it is the `Err` of every seam call, and a fat error type
    /// makes the *success* path pay for the failure path.
    pub expected: Option<Box<Choice>>,
    /// What the replayed code asked for.
    pub actual: Box<Choice>,
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "replay diverged at seq {}", self.seq)?;
        match &self.expected {
            Some(e) => writeln!(f, "  recorded: {}", e.to_line())?,
            None => writeln!(f, "  recorded: <trace exhausted>")?,
        }
        write!(f, "  replayed: {}", self.actual.to_line())
    }
}
impl std::error::Error for Divergence {}

/// The kernel. Single-threaded by construction: one decision at a time, in order.
pub struct Kernel {
    mode:   Mode,
    trace:  Trace,
    cursor: usize,
}

impl Kernel {
    /// A kernel that records a fresh run.
    pub fn recording() -> Self {
        Self { mode: Mode::Record, trace: Trace::new(), cursor: 0 }
    }

    /// A kernel that replays `trace`, checking every request against it.
    pub fn replaying(trace: Trace) -> Self {
        Self { mode: Mode::Replay, trace, cursor: 0 }
    }

    /// Which mode this kernel is in.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The trace — the recording, or the one being replayed.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// How many decisions have been made.
    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Whether a replay consumed the whole trace.
    ///
    /// Worth checking at the end of a replay: a run that stopped early diverged by *omission*, and
    /// nothing else would have noticed.
    pub fn is_exhausted(&self) -> bool {
        self.cursor >= self.trace.len()
    }

    /// Make one decision.
    ///
    /// In `Record`, `produce` is called and its answer written down. In `Replay`, `produce` is
    /// **not** called: the request is checked against the trace and the recorded result returned.
    /// That asymmetry is deliberate — calling `produce` in replay and comparing afterwards would
    /// re-run the very nondeterminism the trace exists to remove.
    pub fn decide<F>(
        &mut self,
        node: Option<&str>,
        kind: ChoiceKind,
        stream: &str,
        request: &str,
        produce: F,
    ) -> Result<String, Divergence>
    where
        F: FnOnce() -> String,
    {
        let asking = Choice {
            seq:     self.cursor as u64 + 1,
            node:    node.map(str::to_string),
            kind,
            stream:  stream.to_string(),
            request: request.to_string(),
            result:  String::new(),
        };

        match self.mode {
            Mode::Record => {
                let result = produce();
                self.trace.push(Choice { result: result.clone(), ..asking });
                self.cursor += 1;
                Ok(result)
            }
            Mode::Replay => {
                let Some(recorded) = self.trace.entries().get(self.cursor).cloned() else {
                    // Asking for more decisions than were recorded is a divergence, not an end.
                    return Err(Divergence {
                        seq:      asking.seq,
                        expected: None,
                        actual:   Box::new(asking),
                    });
                };
                if !recorded.same_request(&asking) {
                    return Err(Divergence {
                        seq:      asking.seq,
                        expected: Some(Box::new(recorded)),
                        actual:   Box::new(asking),
                    });
                }
                self.cursor += 1;
                Ok(recorded.result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(k: &mut Kernel, stream: &str, request: &str, produce: &str) -> Result<String, Divergence> {
        k.decide(Some("n1"), ChoiceKind::Rng, stream, request, || produce.to_string())
    }

    #[test]
    fn recording_returns_what_production_produced_and_writes_it_down() {
        let mut k = Kernel::recording();
        assert_eq!(run(&mut k, "nonce", "draw(u64)", "0x9f3a").unwrap(), "0x9f3a");
        assert_eq!(run(&mut k, "jitter", "draw(u64)", "0x0011").unwrap(), "0x0011");
        assert_eq!(k.trace().len(), 2);
        assert_eq!(k.trace().entries()[0].result, "0x9f3a");
    }

    /// The asymmetry that makes replay deterministic: `produce` is never called, so the
    /// nondeterminism the trace exists to remove cannot leak back in.
    #[test]
    fn replay_supplies_the_recorded_result_and_never_runs_production() {
        let mut rec = Kernel::recording();
        run(&mut rec, "nonce", "draw(u64)", "0x9f3a").unwrap();
        let trace = rec.trace().clone();

        let mut k = Kernel::replaying(trace);
        let called = std::cell::Cell::new(false);
        let got = k
            .decide(Some("n1"), ChoiceKind::Rng, "nonce", "draw(u64)", || {
                called.set(true);
                "0xdifferent".into()
            })
            .unwrap();
        assert_eq!(got, "0x9f3a", "the recorded value, not a fresh one");
        assert!(!called.get(), "production must not run during replay");
    }

    #[test]
    fn a_different_stream_is_a_divergence_naming_both_sides() {
        let mut rec = Kernel::recording();
        run(&mut rec, "nonce", "draw(u64)", "0x1").unwrap();
        let mut k = Kernel::replaying(rec.trace().clone());

        let d = run(&mut k, "jitter", "draw(u64)", "0x1").unwrap_err();
        assert_eq!(d.seq, 1);
        assert_eq!(d.expected.as_ref().unwrap().stream, "nonce");
        assert_eq!(d.actual.stream, "jitter");
        // Both halves are printed, because "what changed" is the question a reviewer has.
        let shown = d.to_string();
        assert!(shown.contains("nonce") && shown.contains("jitter"), "{shown}");
    }

    #[test]
    fn a_different_kind_at_the_same_stream_is_a_divergence() {
        let mut rec = Kernel::recording();
        run(&mut rec, "clock", "draw(u64)", "0x1").unwrap();
        let mut k = Kernel::replaying(rec.trace().clone());
        assert!(k
            .decide(Some("n1"), ChoiceKind::Wall, "clock", "draw(u64)", || "0x1".into())
            .is_err());
    }

    /// Asking for more decisions than were recorded is a divergence, not an end — the replayed code
    /// did something the recording never did.
    #[test]
    fn running_past_the_end_of_the_trace_diverges() {
        let mut rec = Kernel::recording();
        run(&mut rec, "nonce", "draw(u64)", "0x1").unwrap();
        let mut k = Kernel::replaying(rec.trace().clone());
        run(&mut k, "nonce", "draw(u64)", "0x1").unwrap();

        let d = run(&mut k, "nonce", "draw(u64)", "0x1").unwrap_err();
        assert_eq!(d.expected, None);
        assert!(d.to_string().contains("trace exhausted"));
    }

    /// A replay that stops early diverged by **omission**, and nothing else would notice.
    #[test]
    fn a_replay_that_stops_early_is_visible_as_an_unexhausted_trace() {
        let mut rec = Kernel::recording();
        run(&mut rec, "a", "draw(u64)", "0x1").unwrap();
        run(&mut rec, "b", "draw(u64)", "0x2").unwrap();
        let mut k = Kernel::replaying(rec.trace().clone());
        run(&mut k, "a", "draw(u64)", "0x1").unwrap();

        assert!(!k.is_exhausted(), "one decision short, and only this says so");
        assert_eq!(k.position(), 1);
    }

    #[test]
    fn a_replay_of_the_recording_it_came_from_is_exhausted_and_never_diverges() {
        let mut rec = Kernel::recording();
        for (s, v) in [("nonce", "0x1"), ("jitter", "0x2"), ("nonce", "0x3")] {
            run(&mut rec, s, "draw(u64)", v).unwrap();
        }
        let trace = rec.trace().clone();

        let mut k = Kernel::replaying(trace.clone());
        for e in trace.entries() {
            let got = k
                .decide(e.node.as_deref(), e.kind, &e.stream, &e.request, || {
                    unreachable!("production must not run")
                })
                .expect("no divergence");
            assert_eq!(got, e.result);
        }
        assert!(k.is_exhausted());
    }
}
