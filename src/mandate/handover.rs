//! The handover journal and incumbency rules (v3 item 5 PR 4).
//!
//! §4 of [`docs/design/scoped-mandates.md`](../../../docs/design/scoped-mandates.md):
//!
//! > A **handover journal** the successor inherits **as history, not as conclusions**, behind a
//! > readiness gate. Incumbency rules (consecutive terms, cumulative tenure, cooling-off,
//! > eligibility, affiliated principals).
//!
//! # "As history, not as conclusions" is a type, not an instruction
//!
//! A predecessor's journal contains both things it *saw* and things it *decided*. A successor that
//! inherited the decisions would be adopting its predecessor's judgement **without its context** —
//! and, worse, would then be the apparent author of it. The next reader cannot tell whether the
//! successor concluded something or merely found it lying there.
//!
//! So [`HandoverJournal::inherit`] does not hand back entries. It hands back [`Inherited`], in which
//! a conclusion is **always attributed** — you can read *"curator-a concluded X"*, and there is no
//! way to obtain a bare `X`. The same distinction the knowledge layer draws between observation and
//! assessment, applied where it is easiest to lose: at a change of personnel.
//!
//! # The readiness gate
//!
//! A successor is not ready because it was appointed; it is ready when it has read what it
//! inherited. [`Successor::admit`] refuses until then, and says how much is unread — because *"not
//! ready"* with no number is indistinguishable from a successor that is stuck.
//!
//! # The claim these rules make, at its actual strength
//!
//! The record is explicit that claims here are held at **"enforces configured eligibility rules"**.
//! Not "prevents entrenchment", not "ensures rotation". [`eligible`] checks the rules an operator
//! configured, against the history it was given. If the rules are lax, or the history incomplete,
//! it says yes — and that is the operator's decision, not a failure of this function.

use super::{PrincipalId, TermId};
use serde::{Deserialize, Serialize};

/// What kind of thing a journal entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// Something that happened. Inheritable as-is.
    Observation,
    /// The predecessor's **judgement**. Inheritable only with its author attached.
    Conclusion,
}

/// One line a predecessor left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Which appointment wrote it.
    pub term: TermId,
    /// Who wrote it.
    pub author: PrincipalId,
    /// When, in epoch milliseconds.
    pub at_ms: u64,
    /// What kind of statement it is.
    pub kind: EntryKind,
    /// The text.
    pub text: String,
}

/// An entry as a **successor** receives it.
///
/// The variants are the whole point: there is no constructor that yields a bare conclusion, so a
/// successor cannot end up holding its predecessor's judgement as its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inherited {
    /// Something the predecessor saw. Usable directly.
    Observed {
        /// When it was seen.
        at_ms: u64,
        /// What was seen.
        text: String,
    },
    /// Something the predecessor **concluded** — always attributed, never bare.
    ConcludedBy {
        /// Who concluded it.
        author: PrincipalId,
        /// Which of their appointments.
        term: TermId,
        /// When.
        at_ms: u64,
        /// What they concluded.
        text: String,
    },
}

impl Inherited {
    /// A line a human can read, with attribution where attribution is owed.
    pub fn render(&self) -> String {
        match self {
            Self::Observed { at_ms, text } => format!("[{at_ms}] observed: {text}"),
            Self::ConcludedBy { author, term, at_ms, text } =>
                format!("[{at_ms}] {author} (term {term}) concluded: {text}"),
        }
    }

    /// Was this a judgement rather than an observation?
    pub fn is_conclusion(&self) -> bool {
        matches!(self, Self::ConcludedBy { .. })
    }
}

/// What one appointment leaves for the next.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoverJournal {
    entries: Vec<JournalEntry>,
}

impl HandoverJournal {
    /// An empty journal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an entry.
    pub fn record(&mut self, entry: JournalEntry) {
        self.entries.push(entry);
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Is it empty?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What a successor receives: history, with every conclusion attributed.
    ///
    /// There is deliberately no method returning `&[JournalEntry]` to a successor. Handing back the
    /// raw entries would let a caller read a `Conclusion`'s `text` and treat it as a fact, which is
    /// precisely what *"as history, not as conclusions"* forbids.
    pub fn inherit(&self) -> Vec<Inherited> {
        self.entries
            .iter()
            .map(|e| match e.kind {
                EntryKind::Observation => Inherited::Observed {
                    at_ms: e.at_ms,
                    text: e.text.clone(),
                },
                EntryKind::Conclusion => Inherited::ConcludedBy {
                    author: e.author.clone(),
                    term: e.term.clone(),
                    at_ms: e.at_ms,
                    text: e.text.clone(),
                },
            })
            .collect()
    }
}

/// Why a successor is not yet ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotReady {
    /// How many inherited entries remain unread. Present because *"not ready"* without a number is
    /// indistinguishable from a successor that is stuck.
    pub unread: usize,
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "successor has {} inherited entries still unread", self.unread)
    }
}

impl std::error::Error for NotReady {}

/// A successor working through what it inherited.
#[derive(Clone, Debug)]
pub struct Successor {
    read_through: usize,
}

impl Default for Successor {
    fn default() -> Self {
        Self::new()
    }
}

impl Successor {
    /// A successor that has read nothing.
    pub fn new() -> Self {
        Self { read_through: 0 }
    }

    /// Acknowledge having read through the first `n` inherited entries.
    ///
    /// Monotonic: acknowledging less than before does not un-read anything, because a successor
    /// that could walk its own progress backwards could pass the gate and then claim it had not.
    pub fn read_through(&mut self, n: usize) {
        self.read_through = self.read_through.max(n);
    }

    /// May this successor act yet?
    pub fn admit(&self, journal: &HandoverJournal) -> Result<(), NotReady> {
        let total = journal.len();
        if self.read_through >= total {
            Ok(())
        } else {
            Err(NotReady { unread: total - self.read_through })
        }
    }
}

// ── incumbency ────────────────────────────────────────────────────────────────────────────────

/// One completed appointment, as the eligibility check sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermRecord {
    /// Who held it.
    pub holder: PrincipalId,
    /// Which appointment.
    pub term: TermId,
    /// When it started, epoch milliseconds.
    pub started_ms: u64,
    /// When it ended, epoch milliseconds.
    pub ended_ms: u64,
}

impl TermRecord {
    fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }
}

/// The rules an operator configured. Every field is optional: unset means unlimited, because a
/// default limit would be this module deciding an operator's governance for them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncumbencyRules {
    /// How many terms in a row one holder may serve.
    pub max_consecutive_terms: Option<u32>,
    /// How much total time one holder may accumulate.
    pub max_cumulative_ms: Option<u64>,
    /// How long a holder must be out before returning.
    pub cooling_off_ms: Option<u64>,
    /// Principals treated as the same holder for these rules — the *affiliated principals* the
    /// record names. Without this, rotation between two identities of the same operator would
    /// satisfy every limit while changing nothing.
    pub affiliated: Vec<Vec<PrincipalId>>,
}

/// Why a candidate is not eligible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ineligible {
    /// Too many consecutive terms.
    ConsecutiveTerms {
        /// How many they have served in a row.
        served: u32,
        /// The configured limit.
        limit: u32,
    },
    /// Too much accumulated time.
    CumulativeTenure {
        /// Total held, in milliseconds.
        served_ms: u64,
        /// The configured limit.
        limit_ms: u64,
    },
    /// Still inside the cooling-off period.
    CoolingOff {
        /// When they may return.
        eligible_at_ms: u64,
        /// Now.
        now_ms: u64,
    },
}

impl std::fmt::Display for Ineligible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConsecutiveTerms { served, limit } =>
                write!(f, "has served {served} consecutive terms; the configured limit is {limit}"),
            Self::CumulativeTenure { served_ms, limit_ms } =>
                write!(f, "has accumulated {served_ms}ms; the configured limit is {limit_ms}ms"),
            Self::CoolingOff { eligible_at_ms, now_ms } =>
                write!(f, "inside the cooling-off period until {eligible_at_ms} (now {now_ms})"),
        }
    }
}

impl std::error::Error for Ineligible {}

/// Does `candidate` satisfy the configured rules, given `history`?
///
/// **This enforces configured eligibility rules and claims nothing more.** If the rules are lax, or
/// the history incomplete, the answer is yes — that is the operator's decision, not a failure here.
///
/// `history` is expected in chronological order; the consecutive-terms count walks it from the end.
pub fn eligible(
    rules: &IncumbencyRules,
    history: &[TermRecord],
    candidate: &PrincipalId,
    now_ms: u64,
) -> Result<(), Ineligible> {
    let same = |a: &PrincipalId, b: &PrincipalId| -> bool {
        a == b || rules.affiliated.iter().any(|g| g.contains(a) && g.contains(b))
    };

    if let Some(limit) = rules.max_consecutive_terms {
        let mut run = 0u32;
        for t in history.iter().rev() {
            if same(&t.holder, candidate) {
                run += 1;
            } else {
                break;
            }
        }
        if run >= limit {
            return Err(Ineligible::ConsecutiveTerms { served: run, limit });
        }
    }

    if let Some(limit_ms) = rules.max_cumulative_ms {
        let served_ms: u64 = history
            .iter()
            .filter(|t| same(&t.holder, candidate))
            .map(|t| t.duration_ms())
            .sum();
        if served_ms >= limit_ms {
            return Err(Ineligible::CumulativeTenure { served_ms, limit_ms });
        }
    }

    if let Some(cool_ms) = rules.cooling_off_ms
        && let Some(last) = history.iter().rev().find(|t| same(&t.holder, candidate))
    {
        let eligible_at = last.ended_ms.saturating_add(cool_ms);
        if now_ms < eligible_at {
            return Err(Ineligible::CoolingOff { eligible_at_ms: eligible_at, now_ms });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }
    fn tid(s: &str) -> TermId {
        TermId::new(s).unwrap()
    }

    fn journal() -> HandoverJournal {
        let mut j = HandoverJournal::new();
        j.record(JournalEntry {
            term: tid("term-1"),
            author: pid("curator-a"),
            at_ms: 1_000,
            kind: EntryKind::Observation,
            text: "three merges failed the write gate".into(),
        });
        j.record(JournalEntry {
            term: tid("term-1"),
            author: pid("curator-a"),
            at_ms: 2_000,
            kind: EntryKind::Conclusion,
            text: "the gate is misconfigured".into(),
        });
        j
    }

    // ── history, not conclusions ─────────────────────────────────────────────────────────────

    /// **The type does the work.** A successor cannot obtain a bare conclusion, so it cannot end up
    /// the apparent author of its predecessor's judgement.
    #[test]
    fn a_conclusion_is_inherited_only_with_its_author_attached() {
        let inherited = journal().inherit();
        assert_eq!(inherited.len(), 2);

        assert_eq!(
            inherited[0],
            Inherited::Observed { at_ms: 1_000, text: "three merges failed the write gate".into() },
            "an observation comes through as itself"
        );

        match &inherited[1] {
            Inherited::ConcludedBy { author, term, text, .. } => {
                assert_eq!(author, &pid("curator-a"));
                assert_eq!(term, &tid("term-1"));
                assert_eq!(text, "the gate is misconfigured");
            }
            other => panic!("a conclusion must arrive attributed, got {other:?}"),
        }
    }

    /// Rendering keeps the attribution visible — the successor reads *"X concluded Y"*, never *"Y"*.
    #[test]
    fn a_rendered_conclusion_names_who_concluded_it() {
        let rendered: Vec<String> = journal().inherit().iter().map(Inherited::render).collect();
        assert!(rendered[0].contains("observed:"));
        assert!(
            rendered[1].contains("curator-a") && rendered[1].contains("concluded:"),
            "the judgement keeps its author: {}",
            rendered[1]
        );
    }

    #[test]
    fn observations_and_conclusions_are_distinguishable_after_inheriting() {
        let inherited = journal().inherit();
        assert!(!inherited[0].is_conclusion());
        assert!(inherited[1].is_conclusion());
    }

    // ── the readiness gate ───────────────────────────────────────────────────────────────────

    /// A successor is ready when it has read what it inherited, not when it was appointed.
    #[test]
    fn a_successor_cannot_act_until_it_has_read_the_journal() {
        let j = journal();
        let mut s = Successor::new();
        assert_eq!(s.admit(&j), Err(NotReady { unread: 2 }), "and it says how much is left");

        s.read_through(1);
        assert_eq!(s.admit(&j), Err(NotReady { unread: 1 }));

        s.read_through(2);
        assert!(s.admit(&j).is_ok());
    }

    /// Progress is monotonic: a successor that could walk its own progress backwards could pass the
    /// gate and then claim it had not.
    #[test]
    fn reading_progress_never_goes_backwards() {
        let j = journal();
        let mut s = Successor::new();
        s.read_through(2);
        s.read_through(0);
        assert!(s.admit(&j).is_ok(), "acknowledging less does not un-read anything");
    }

    #[test]
    fn an_empty_journal_admits_immediately() {
        assert!(Successor::new().admit(&HandoverJournal::new()).is_ok());
    }

    // ── incumbency ───────────────────────────────────────────────────────────────────────────

    fn term(holder: &str, id: &str, start: u64, end: u64) -> TermRecord {
        TermRecord { holder: pid(holder), term: tid(id), started_ms: start, ended_ms: end }
    }

    #[test]
    fn consecutive_terms_are_counted_from_the_most_recent_backwards() {
        let rules = IncumbencyRules { max_consecutive_terms: Some(2), ..Default::default() };
        let history = vec![
            term("curator-b", "t1", 0, 100),
            term("curator-a", "t2", 100, 200),
            term("curator-a", "t3", 200, 300),
        ];
        assert_eq!(
            eligible(&rules, &history, &pid("curator-a"), 300),
            Err(Ineligible::ConsecutiveTerms { served: 2, limit: 2 })
        );
        assert!(
            eligible(&rules, &history, &pid("curator-b"), 300).is_ok(),
            "someone else's run is not theirs"
        );
    }

    #[test]
    fn cumulative_tenure_sums_every_term_not_just_recent_ones() {
        let rules = IncumbencyRules { max_cumulative_ms: Some(150), ..Default::default() };
        let history = vec![
            term("curator-a", "t1", 0, 100),
            term("curator-b", "t2", 100, 200),
            term("curator-a", "t3", 200, 260),
        ];
        assert_eq!(
            eligible(&rules, &history, &pid("curator-a"), 300),
            Err(Ineligible::CumulativeTenure { served_ms: 160, limit_ms: 150 }),
            "a gap in the middle does not reset the total"
        );
    }

    #[test]
    fn cooling_off_measures_from_the_end_of_the_last_term_held() {
        let rules = IncumbencyRules { cooling_off_ms: Some(500), ..Default::default() };
        let history = vec![term("curator-a", "t1", 0, 100)];
        assert_eq!(
            eligible(&rules, &history, &pid("curator-a"), 300),
            Err(Ineligible::CoolingOff { eligible_at_ms: 600, now_ms: 300 })
        );
        assert!(eligible(&rules, &history, &pid("curator-a"), 600).is_ok());
    }

    /// **Affiliated principals.** Without this, rotating between two identities of the same operator
    /// would satisfy every limit while changing nothing.
    #[test]
    fn affiliated_principals_count_as_one_holder() {
        let rules = IncumbencyRules {
            max_consecutive_terms: Some(2),
            affiliated: vec![vec![pid("curator-a"), pid("curator-a-alt")]],
            ..Default::default()
        };
        let history = vec![
            term("curator-a", "t1", 0, 100),
            term("curator-a-alt", "t2", 100, 200),
        ];
        assert_eq!(
            eligible(&rules, &history, &pid("curator-a"), 300),
            Err(Ineligible::ConsecutiveTerms { served: 2, limit: 2 }),
            "two identities of one operator are one holder for these rules"
        );
    }

    /// **The claim at its actual strength.** Unset rules mean unlimited — a default limit would be
    /// this module deciding an operator's governance for them.
    #[test]
    fn unconfigured_rules_permit_everything() {
        let history = vec![
            term("curator-a", "t1", 0, 100),
            term("curator-a", "t2", 100, 200),
            term("curator-a", "t3", 200, 300),
        ];
        assert!(
            eligible(&IncumbencyRules::default(), &history, &pid("curator-a"), 300).is_ok(),
            "enforcing configured rules means enforcing the ones that were configured"
        );
    }
}
