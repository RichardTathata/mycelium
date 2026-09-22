//! The choices trace — one line per kernel decision, in order.
//!
//! Schema fixed by item 6 PR 1 (`docs/design/replay-nondeterminism-inventory.md` §5). This module
//! implements it; it does not get to redesign it.
//!
//! # The one rule that makes a trace worth having
//!
//! **A request is a canonical digest of everything the effect depends on — never a length alone.**
//! For a write that means the *content hash of the bytes*, the target, the offset, and the flags
//! that change the write's meaning. Two WAL records of equal length are a different write, and a
//! trace recording only `len=214` would accept changed content as a faithful replay. That is not a
//! detail: it is the difference between a harness that detects divergence and one that merely
//! re-seeds, and PR 2's gate is exactly that test.
//!
//! **A result is what the production code received** — `Ok(n)`, a typed error, or a partial
//! completion (`wrote=96`, the short write the durability argument turns on) — never what it then
//! did with it.

use std::fmt;

/// What kind of decision this was. The kernel names one for every entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChoiceKind {
    /// A draw from a named RNG stream.
    Rng,
    /// A wall-clock read. Separate from [`Mono`](ChoiceKind::Mono) because they fail differently:
    /// wall time jumps, monotonic time does not.
    Wall,
    /// A monotonic-clock read.
    Mono,
    /// Which branch a scheduling point took.
    Sched,
    /// A channel operation and what it returned.
    Chan,
    /// A timer deadline and whether it fired.
    Timer,
    /// A storage effect. Its request carries the content hash, target, offset and flags.
    Fs,
    /// An external input — a token verification, an LLM reply, an MCP response.
    Input,
    /// A fault the kernel injected.
    Fault,
}

impl ChoiceKind {
    /// The token used in the trace's text form.
    pub fn as_str(self) -> &'static str {
        match self {
            ChoiceKind::Rng => "rng",
            ChoiceKind::Wall => "wall",
            ChoiceKind::Mono => "mono",
            ChoiceKind::Sched => "sched",
            ChoiceKind::Chan => "chan",
            ChoiceKind::Timer => "timer",
            ChoiceKind::Fs => "fs",
            ChoiceKind::Input => "input",
            ChoiceKind::Fault => "fault",
        }
    }

    /// Parse a token from a trace file.
    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "rng" => ChoiceKind::Rng,
            "wall" => ChoiceKind::Wall,
            "mono" => ChoiceKind::Mono,
            "sched" => ChoiceKind::Sched,
            "chan" => ChoiceKind::Chan,
            "timer" => ChoiceKind::Timer,
            "fs" => ChoiceKind::Fs,
            "input" => ChoiceKind::Input,
            "fault" => ChoiceKind::Fault,
            _ => return None,
        })
    }
}

impl fmt::Display for ChoiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One kernel decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Position in the trace, from 1.
    pub seq: u64,
    /// Which node asked. `None` for decisions that belong to the run rather than a node — a
    /// scheduling branch between two nodes, for instance.
    pub node: Option<String>,
    /// What kind of decision.
    pub kind: ChoiceKind,
    /// The stream or seam: `nonce`, `wal.bin`, `gossip/shard2`, `oidc/verify`.
    pub stream: String,
    /// The canonical request — everything the effect depends on. See the module docs.
    pub request: String,
    /// What the production code received back.
    pub result: String,
}

impl Choice {
    /// Does `self` describe the same *request* as `other`?
    ///
    /// Compares everything except `result` and `seq`: a replay checks that the code is asking for
    /// the same thing, and then *supplies* the recorded answer. Comparing the result too would be
    /// checking that the replay returned what the replay returned.
    pub fn same_request(&self, other: &Choice) -> bool {
        self.node == other.node
            && self.kind == other.kind
            && self.stream == other.stream
            && self.request == other.request
    }

    /// The single tab-separated line this entry writes.
    pub fn to_line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            self.seq,
            self.node.as_deref().unwrap_or("-"),
            self.kind,
            self.stream,
            self.request,
            self.result
        )
    }

    /// Parse a line written by [`to_line`](Self::to_line).
    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split('\t');
        let seq = parts.next()?.parse().ok()?;
        let node = match parts.next()? {
            "-" => None,
            n => Some(n.to_string()),
        };
        let kind = ChoiceKind::parse(parts.next()?)?;
        let stream = parts.next()?.to_string();
        let request = parts.next()?.to_string();
        // The result may itself contain tabs in principle; take the remainder verbatim.
        let result = parts.collect::<Vec<_>>().join("\t");
        Some(Choice { seq, node, kind, stream, request, result })
    }
}

/// An ordered choice log — the reproduction itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    entries: Vec<Choice>,
}

impl Trace {
    /// An empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// The entries, in order.
    pub fn entries(&self) -> &[Choice] {
        &self.entries
    }

    /// How many decisions were recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether anything was recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append one decision, assigning its sequence.
    pub fn push(&mut self, mut choice: Choice) {
        choice.seq = self.entries.len() as u64 + 1;
        self.entries.push(choice);
    }

    /// The trace's text form — one line per decision, the order *is* the content.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&e.to_line());
            out.push('\n');
        }
        out
    }

    /// Parse a trace written by [`to_text`](Self::to_text).
    ///
    /// A line that does not parse is an error rather than a skip: a trace with a hole in it would
    /// replay as a *different* run while looking like the same one.
    pub fn parse(text: &str) -> Result<Self, TraceError> {
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            // **A carriage return is refused, because this format cannot carry one.**
            //
            // `str::lines()` strips a `\r` only when it precedes `\n`. A line ending in a *bare*
            // carriage return therefore parses with the `\r` kept in its last field; `to_text`
            // appends `\n`, making it `\r\n`; and re-parsing strips it. The trace that comes back
            // is not the trace that went in — and a trace is what a whole-node recording replays
            // from, so what it decodes to *is* the run. A silently shortened field replays a
            // different run while looking like the same one, which is the exact failure
            // `Trace::parse` refuses holes to prevent.
            //
            // Only a **trailing** `\r` breaks the round trip today; an interior one survives. The
            // rule is the broader one anyway, because the narrow one depends on `to_text` choosing
            // `\n` over `\r\n` — and a parser should not encode its writer's current taste in line
            // endings. Our recorder emits none of these, so nothing legitimate is refused.
            //
            // Found by §12.6's `replay_trace` fuzz target, by the round-trip assertion.
            if line.contains('\r') {
                return Err(TraceError::Malformed { line: i + 1 });
            }
            let choice = Choice::parse(line).ok_or(TraceError::Malformed { line: i + 1 })?;
            entries.push(choice);
        }
        Ok(Self { entries })
    }
}

/// Why a trace could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    /// A line did not parse. Named by 1-based line number.
    Malformed {
        /// The offending line.
        line: usize,
    },
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Malformed { line } => write!(f, "malformed trace at line {line}"),
        }
    }
}
impl std::error::Error for TraceError {}

#[cfg(test)]
mod tests {
    /// **A trace that parses must re-render and re-parse to itself.** Regression for the defect
    /// §12.6's `replay_trace` fuzz target found on main, 2026-09-22.
    ///
    /// `str::lines()` strips a `\r` only when it precedes `\n`. So a line ending in a *bare*
    /// carriage return parses with the `\r` kept in its last field; `to_text` then appends `\n`,
    /// making it `\r\n`; and re-parsing strips it. The trace that comes back is not the trace that
    /// went in — and a trace is the record a whole-node recording replays from, so what it decodes
    /// to *is* the run.
    #[test]
    fn a_line_ending_in_a_carriage_return_does_not_silently_lose_it() {
        let text = "1\t-\tfs\tstream\trequest\tresult\r";

        match Trace::parse(text) {
            // Refused is the correct answer: `to_text` is line-oriented and cannot represent a
            // field that ends in a carriage return, so accepting one would mean accepting what we
            // cannot re-emit.
            Err(_) => {}
            Ok(trace) => {
                let again = Trace::parse(&trace.to_text()).expect("a rendered trace must re-parse");
                assert_eq!(
                    trace, again,
                    "a trace that parses must survive its own round trip; the carriage return was \
                     absorbed, so the replayed run is not the recorded one",
                );
            }
        }
    }

    use super::*;

    fn choice(kind: ChoiceKind, stream: &str, request: &str, result: &str) -> Choice {
        Choice {
            seq: 0,
            node: Some("n1".into()),
            kind,
            stream: stream.into(),
            request: request.into(),
            result: result.into(),
        }
    }

    #[test]
    fn a_trace_round_trips_through_its_text_form() {
        let mut t = Trace::new();
        t.push(choice(ChoiceKind::Rng, "nonce", "draw(u64)", "0x9f3a"));
        t.push(Choice { node: None, ..choice(ChoiceKind::Sched, "select", "conn/n1->n2#3 ready?", "branch=recv") });
        t.push(choice(ChoiceKind::Fs, "wal.bin", "append d=sha256:4f1c len=214 sync=true off=8192", "Ok(214)"));

        let parsed = Trace::parse(&t.to_text()).expect("round trip");
        assert_eq!(parsed, t);
        assert_eq!(parsed.entries()[1].node, None, "a run-level decision has no node");
        assert_eq!(parsed.entries()[2].seq, 3, "sequence is assigned in order");
    }

    #[test]
    fn a_malformed_line_is_an_error_not_a_skip() {
        // A trace with a hole replays as a *different* run while looking like the same one.
        let err = Trace::parse("1\tn1\trng\tnonce\tdraw(u64)\t0x1\nnot a trace line\n").unwrap_err();
        assert_eq!(err, TraceError::Malformed { line: 2 });
    }

    #[test]
    fn same_request_ignores_the_result_and_the_sequence() {
        let a = choice(ChoiceKind::Fs, "wal.bin", "append d=sha256:4f1c", "Ok(214)");
        let mut b = choice(ChoiceKind::Fs, "wal.bin", "append d=sha256:4f1c", "Err(EIO) wrote=96");
        b.seq = 99;
        // A replay checks that the code asks the same question; it then *supplies* the answer.
        assert!(a.same_request(&b));

        let different = choice(ChoiceKind::Fs, "wal.bin", "append d=sha256:9ab0", "Ok(214)");
        assert!(!a.same_request(&different), "a different content digest is a different request");
    }

    #[test]
    fn every_kind_survives_its_token() {
        for k in [
            ChoiceKind::Rng,
            ChoiceKind::Wall,
            ChoiceKind::Mono,
            ChoiceKind::Sched,
            ChoiceKind::Chan,
            ChoiceKind::Timer,
            ChoiceKind::Fs,
            ChoiceKind::Input,
            ChoiceKind::Fault,
        ] {
            assert_eq!(ChoiceKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(ChoiceKind::parse("something-else"), None);
    }
}
