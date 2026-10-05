//! Strict eligibility — a verdict per configured rule, from history whose coverage the **source**
//! established (realignment repairs A3, decision D4; `docs/plans/realignment-repairs.md` §3.4).
//!
//! # Why a second function
//!
//! [`eligible`](super::handover::eligible) evaluates the history it is handed and says so: *"if the
//! rules are lax, or the history incomplete, the answer is yes."* That is honest, and it is the wrong
//! answer for a deployment that configured incumbency limits and needs them to mean something — a
//! history that silently lacks the candidate's earlier terms passes every limit. This module keeps
//! `eligible` exactly as it is and adds the strict form beside it: **an unestablished history is
//! `Unknown`, never `Eligible`.**
//!
//! # Who establishes coverage
//!
//! Not the caller. A boolean `complete: true` passed in would only move the assumption. A
//! [`TermHistory`] is built from **chained** terms — each names its predecessor — together with the
//! source's [`Origin`] (the role's first term, or a trusted baseline at a checkpoint) and the head the
//! source says is current. [`TermHistory::from_chain`] *verifies* the chain: where a term's predecessor
//! is not the term before it, history breaks, and only the suffix after the last break is vouched for;
//! where the last term is not the head, nothing reaches the present. The verified facts are the
//! [`Coverage`]; nothing else is.
//!
//! # Sufficiency is per rule
//!
//! Each configured rule needs different history, so each gets its own verdict:
//!
//! - **consecutive terms** — a run is decided once it is broken inside the verified suffix, or the
//!   suffix starts at the role's origin; a run reaching the start of a suffix that does not is
//!   `Unknown` (an earlier term may extend it). A run already at the limit is `Ineligible` however
//!   much is missing.
//! - **cumulative tenure** — needs the origin, or a baseline naming the candidate's earlier total.
//!   Exceeding the limit inside the suffix alone is `Ineligible` regardless.
//! - **cooling-off** — needs the verified suffix to reach back across the cooling window, so a term
//!   that ended inside it cannot be missing.
//!
//! A rule the operator did not configure is not evaluated. [`StrictEligibility::verdict`] is
//! `Ineligible` if any rule is, else `Unknown` if any rule is, else `Eligible`.
//!
//! # What this does not claim
//!
//! That the source recorded every appointment there was. A source can only vouch for the chain it
//! holds; a role whose appointments were never recorded anywhere is outside every rule here. And it
//! is not readiness by itself: [`ready`] combines it with the handover journal having been read.

use super::handover::{HandoverJournal, IncumbencyRules, Ineligible, NotReady, Successor, TermRecord};
use super::{PrincipalId, TermId};
use std::collections::HashMap;

/// One appointment as a source records it: the term, and the term it followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainedTerm {
    /// The appointment.
    pub record: TermRecord,
    /// The appointment it followed; `None` for the role's first.
    pub previous: Option<TermId>,
}

/// Where a source's history begins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The source holds the role from its first appointment (that term's `previous` is `None`).
    Genesis,
    /// The source holds history after a checkpoint, with a baseline it trusts at that point: the
    /// term the checkpoint follows, and each principal's cumulative tenure up to it.
    Baseline {
        /// The term the first held term follows.
        after: TermId,
        /// Cumulative tenure per principal up to and including `after`, in milliseconds.
        cumulative_ms: HashMap<PrincipalId, u64>,
    },
    /// Nothing about what came before the first held term.
    Unknown,
}

/// What a [`TermHistory`]'s source can vouch for, as verified at construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// How many terms at the end of the history form an unbroken chain.
    pub continuous_suffix: usize,
    /// Whether that suffix begins at the role's first appointment.
    pub from_genesis: bool,
    /// Whether that suffix begins right after a trusted baseline.
    pub from_baseline: bool,
    /// Whether the last term is the head the source says is current.
    pub through_head: bool,
}

/// Appointment history, with the coverage its source established.
#[derive(Clone, Debug)]
pub struct TermHistory {
    terms: Vec<TermRecord>,
    coverage: Coverage,
    baseline: HashMap<PrincipalId, u64>,
}

impl TermHistory {
    /// Build from the chain a source holds (oldest first), the source's origin, and the head the
    /// source says is current. The chain is verified here; a break or a stale head narrows the
    /// coverage rather than failing — the rules decide what that narrowing means.
    pub fn from_chain(chain: Vec<ChainedTerm>, origin: Origin, head: &TermId) -> Self {
        // The last break: the latest index whose `previous` is not the term before it.
        let mut suffix_start = 0usize;
        for i in 1..chain.len() {
            if chain[i].previous.as_ref() != Some(&chain[i - 1].record.term) {
                suffix_start = i;
            }
        }
        let first = chain.get(suffix_start);
        let from_genesis = suffix_start == 0
            && matches!(origin, Origin::Genesis)
            && first.is_some_and(|t| t.previous.is_none());
        let (from_baseline, baseline) = match origin {
            Origin::Baseline { after, cumulative_ms } if suffix_start == 0
                && first.is_some_and(|t| t.previous.as_ref() == Some(&after)) =>
                (true, cumulative_ms),
            _ => (false, HashMap::new()),
        };
        let through_head = chain.last().is_some_and(|t| &t.record.term == head);
        let coverage = Coverage {
            continuous_suffix: chain.len() - suffix_start,
            from_genesis,
            from_baseline,
            through_head,
        };
        Self { terms: chain.into_iter().map(|c| c.record).collect(), coverage, baseline }
    }

    /// The verified coverage.
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    fn suffix(&self) -> &[TermRecord] {
        &self.terms[self.terms.len() - self.coverage.continuous_suffix..]
    }
}

/// A verdict for one rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleVerdict {
    /// The rule is satisfied, on history that suffices to say so.
    Eligible,
    /// The rule is not satisfied.
    Ineligible(Ineligible),
    /// The history does not suffice to decide; the string says what is missing.
    Unknown(String),
}

/// Each configured rule's verdict; `None` for a rule the operator did not configure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictEligibility {
    /// `max_consecutive_terms`.
    pub consecutive: Option<RuleVerdict>,
    /// `max_cumulative_ms`.
    pub cumulative: Option<RuleVerdict>,
    /// `cooling_off_ms`.
    pub cooling_off: Option<RuleVerdict>,
}

impl StrictEligibility {
    /// `Ineligible` if any configured rule is, else `Unknown` if any is, else `Eligible`.
    pub fn verdict(&self) -> RuleVerdict {
        let all = [&self.consecutive, &self.cumulative, &self.cooling_off];
        if let Some(RuleVerdict::Ineligible(i)) =
            all.iter().filter_map(|v| v.as_ref()).find(|v| matches!(v, RuleVerdict::Ineligible(_)))
        {
            return RuleVerdict::Ineligible(i.clone());
        }
        let unknown: Vec<&str> = all
            .iter()
            .filter_map(|v| match v {
                Some(RuleVerdict::Unknown(why)) => Some(why.as_str()),
                _ => None,
            })
            .collect();
        if unknown.is_empty() { RuleVerdict::Eligible } else { RuleVerdict::Unknown(unknown.join("; ")) }
    }
}

/// Decide each configured rule for `candidate` from `history`, never answering `Eligible` where the
/// history does not suffice.
pub fn eligible_strict(
    rules: &IncumbencyRules,
    history: &TermHistory,
    candidate: &PrincipalId,
    now_ms: u64,
) -> StrictEligibility {
    let same = |a: &PrincipalId, b: &PrincipalId| -> bool {
        a == b || rules.affiliated.iter().any(|g| g.contains(a) && g.contains(b))
    };
    let cov = history.coverage();
    let suffix = history.suffix();
    let stale = "the history does not reach the current head";

    let consecutive = rules.max_consecutive_terms.map(|limit| {
        let run = suffix.iter().rev().take_while(|t| same(&t.holder, candidate)).count() as u32;
        if run >= limit {
            RuleVerdict::Ineligible(Ineligible::ConsecutiveTerms { served: run, limit })
        } else if !cov.through_head {
            RuleVerdict::Unknown(format!("consecutive terms: {stale}"))
        } else if (run as usize) < suffix.len() || cov.from_genesis {
            RuleVerdict::Eligible
        } else {
            RuleVerdict::Unknown(
                "consecutive terms: the run reaches the start of the verified history, and an earlier term may extend it".into(),
            )
        }
    });

    let cumulative = rules.max_cumulative_ms.map(|limit_ms| {
        let in_suffix: u64 = suffix
            .iter()
            .filter(|t| same(&t.holder, candidate))
            .map(|t| t.ended_ms.saturating_sub(t.started_ms))
            .sum();
        let before: u64 = history
            .baseline
            .iter()
            .filter(|(p, _)| same(p, candidate))
            .map(|(_, ms)| *ms)
            .sum();
        let served_ms = in_suffix.saturating_add(before);
        if served_ms >= limit_ms {
            RuleVerdict::Ineligible(Ineligible::CumulativeTenure { served_ms, limit_ms })
        } else if !(cov.from_genesis || cov.from_baseline) {
            RuleVerdict::Unknown(
                "cumulative tenure: the verified history does not start at the role's first term or a trusted baseline".into(),
            )
        } else if !cov.through_head {
            RuleVerdict::Unknown(format!("cumulative tenure: {stale}"))
        } else {
            RuleVerdict::Eligible
        }
    });

    let cooling_off = rules.cooling_off_ms.map(|cool_ms| {
        if !cov.through_head {
            return RuleVerdict::Unknown(format!("cooling-off: {stale}"));
        }
        if let Some(last) = suffix.iter().rev().find(|t| same(&t.holder, candidate)) {
            let eligible_at_ms = last.ended_ms.saturating_add(cool_ms);
            return if now_ms < eligible_at_ms {
                RuleVerdict::Ineligible(Ineligible::CoolingOff { eligible_at_ms, now_ms })
            } else {
                RuleVerdict::Eligible
            };
        }
        // No term of the candidate in the verified suffix: that settles it only if the suffix
        // reaches back across the whole cooling window.
        let window_start = now_ms.saturating_sub(cool_ms);
        let reaches = cov.from_genesis || suffix.first().is_some_and(|t| t.started_ms <= window_start);
        if reaches {
            RuleVerdict::Eligible
        } else {
            RuleVerdict::Unknown("cooling-off: the verified history does not reach back across the cooling window".into())
        }
    });

    StrictEligibility { consecutive, cumulative, cooling_off }
}

/// Why a successor may not act yet, under strict eligibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotReadyStrict {
    /// It has not read what it inherited.
    Unread(NotReady),
    /// A configured rule says no.
    Ineligible(Ineligible),
    /// A configured rule cannot be decided on the history available.
    HistoryUnknown(String),
}

impl std::fmt::Display for NotReadyStrict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unread(n) => write!(f, "{n}"),
            Self::Ineligible(i) => write!(f, "not eligible: {i}"),
            Self::HistoryUnknown(why) => write!(f, "eligibility unknown: {why}"),
        }
    }
}
impl std::error::Error for NotReadyStrict {}

/// May this successor act: has it read what it inherited, **and** is every configured incumbency
/// rule decided in its favour? Two separate conditions — reading the journal says nothing about
/// eligibility, and eligibility says nothing about having read it.
pub fn ready(successor: &Successor, journal: &HandoverJournal, eligibility: &StrictEligibility) -> Result<(), NotReadyStrict> {
    successor.admit(journal).map_err(NotReadyStrict::Unread)?;
    match eligibility.verdict() {
        RuleVerdict::Eligible => Ok(()),
        RuleVerdict::Ineligible(i) => Err(NotReadyStrict::Ineligible(i)),
        RuleVerdict::Unknown(why) => Err(NotReadyStrict::HistoryUnknown(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mandate::handover::{eligible, EntryKind, JournalEntry};

    fn pid(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }
    fn tid(n: u32) -> TermId {
        TermId::new(format!("term-{n}")).unwrap()
    }
    const DAY: u64 = 86_400_000;

    /// Terms `from..=to`, one day each, chained, held by `holder(n)`.
    fn chain(from: u32, to: u32, holder: impl Fn(u32) -> &'static str) -> Vec<ChainedTerm> {
        (from..=to)
            .map(|n| ChainedTerm {
                record: TermRecord {
                    holder: pid(holder(n)),
                    term: tid(n),
                    started_ms: n as u64 * DAY,
                    ended_ms: (n as u64 + 1) * DAY,
                },
                previous: if n == 1 { None } else { Some(tid(n - 1)) },
            })
            .collect()
    }

    fn rules() -> IncumbencyRules {
        IncumbencyRules {
            max_consecutive_terms: Some(3),
            max_cumulative_ms: Some(10 * DAY),
            cooling_off_ms: Some(2 * DAY),
            affiliated: vec![],
        }
    }

    /// The reviewer's witness: a chain from genesis with one record missing. The lenient check says
    /// yes on the same terms; the strict one says cumulative tenure is unknown — and once the record
    /// is restored, it gives a real verdict.
    #[test]
    fn a_missing_record_leaves_cumulative_tenure_unknown_until_it_is_restored() {
        // ada holds every odd term: 1,3,5,…,15 is 8 days; with term 7 missing, 7 days are visible.
        let full = chain(1, 16, |n| if n % 2 == 1 { "ada" } else { "bo" });
        let gapped: Vec<ChainedTerm> = full.iter().filter(|t| t.record.term != tid(7)).cloned().collect();
        let only_cumulative = IncumbencyRules { max_cumulative_ms: Some(8 * DAY), ..IncumbencyRules::default() };
        let now = 17 * DAY;

        let lenient: Vec<TermRecord> = gapped.iter().map(|t| t.record.clone()).collect();
        assert!(eligible(&only_cumulative, &lenient, &pid("ada"), now).is_ok(), "the lenient check says yes on 7 visible days");

        let h = TermHistory::from_chain(gapped, Origin::Genesis, &tid(16));
        assert!(!h.coverage().from_genesis, "the break means the verified suffix no longer starts at genesis");
        assert!(matches!(eligible_strict(&only_cumulative, &h, &pid("ada"), now).cumulative, Some(RuleVerdict::Unknown(_))));

        let restored = TermHistory::from_chain(full, Origin::Genesis, &tid(16));
        assert!(matches!(
            eligible_strict(&only_cumulative, &restored, &pid("ada"), now).cumulative,
            Some(RuleVerdict::Ineligible(Ineligible::CumulativeTenure { served_ms, .. })) if served_ms == 8 * DAY
        ), "with the record back, ada's 8 days meet the 8-day limit");
    }

    /// The reviewer's second witness: terms 20–30 verified, nothing before. Consecutive terms are
    /// decided (the run breaks inside the window); cumulative tenure is not, without a baseline.
    #[test]
    fn a_verified_window_decides_consecutive_terms_but_not_cumulative_tenure() {
        let window = chain(20, 30, |n| if n >= 29 { "ada" } else { "bo" });
        let h = TermHistory::from_chain(window, Origin::Unknown, &tid(30));
        let e = eligible_strict(&rules(), &h, &pid("ada"), 40 * DAY);
        assert_eq!(e.consecutive, Some(RuleVerdict::Eligible), "two in a row, broken by bo inside the window");
        assert!(matches!(e.cumulative, Some(RuleVerdict::Unknown(_))), "ada may have held terms before 20");
        assert!(matches!(e.verdict(), RuleVerdict::Unknown(_)), "one unknown rule makes the whole answer unknown");

        // A trusted baseline at the checkpoint closes it.
        let window = chain(20, 30, |n| if n >= 29 { "ada" } else { "bo" });
        let baseline = Origin::Baseline { after: tid(19), cumulative_ms: HashMap::from([(pid("ada"), 3 * DAY)]) };
        let h = TermHistory::from_chain(window, baseline, &tid(30));
        assert_eq!(eligible_strict(&rules(), &h, &pid("ada"), 40 * DAY).cumulative, Some(RuleVerdict::Eligible), "3 + 2 days under 10");
    }

    /// A run that reaches the start of the verified history is unknown — unless that start is the
    /// role's first term. A run already at the limit is ineligible however much is missing.
    #[test]
    fn a_run_reaching_the_start_of_the_history_is_unknown_unless_that_start_is_genesis() {
        let only = IncumbencyRules { max_consecutive_terms: Some(3), ..IncumbencyRules::default() };
        let all_ada = chain(20, 21, |_| "ada");
        let h = TermHistory::from_chain(all_ada, Origin::Unknown, &tid(21));
        assert!(matches!(eligible_strict(&only, &h, &pid("ada"), 0).consecutive, Some(RuleVerdict::Unknown(_))));

        let from_start = chain(1, 2, |_| "ada");
        let h = TermHistory::from_chain(from_start, Origin::Genesis, &tid(2));
        assert_eq!(eligible_strict(&only, &h, &pid("ada"), 0).consecutive, Some(RuleVerdict::Eligible));

        let at_limit = chain(20, 22, |_| "ada");
        let h = TermHistory::from_chain(at_limit, Origin::Unknown, &tid(22));
        assert!(matches!(eligible_strict(&only, &h, &pid("ada"), 0).consecutive, Some(RuleVerdict::Ineligible(_))));
    }

    /// History that does not reach the head the source names cannot say what happened since.
    #[test]
    fn a_history_that_stops_short_of_the_head_is_unknown() {
        let h = TermHistory::from_chain(chain(1, 10, |n| if n == 10 { "bo" } else { "cy" }), Origin::Genesis, &tid(11));
        assert!(!h.coverage().through_head);
        let e = eligible_strict(&rules(), &h, &pid("ada"), 20 * DAY);
        assert!(matches!(e.consecutive, Some(RuleVerdict::Unknown(_))));
        assert!(matches!(e.cooling_off, Some(RuleVerdict::Unknown(_))));
    }

    /// Cooling-off with no term of the candidate in view is settled only when the verified history
    /// reaches back across the whole window.
    #[test]
    fn cooling_off_needs_the_history_to_cover_the_window() {
        let only = IncumbencyRules { cooling_off_ms: Some(5 * DAY), ..IncumbencyRules::default() };
        // Verified 28–30, now day 31: the window starts at day 26, before the first verified term.
        let short = TermHistory::from_chain(chain(28, 30, |_| "bo"), Origin::Unknown, &tid(30));
        assert!(matches!(eligible_strict(&only, &short, &pid("ada"), 31 * DAY).cooling_off, Some(RuleVerdict::Unknown(_))));
        // Verified 20–30 reaches back past day 26.
        let long = TermHistory::from_chain(chain(20, 30, |_| "bo"), Origin::Unknown, &tid(30));
        assert_eq!(eligible_strict(&only, &long, &pid("ada"), 31 * DAY).cooling_off, Some(RuleVerdict::Eligible));
        // ada's term ending at day 30 is inside the window: ineligible until day 35.
        let recent = TermHistory::from_chain(chain(20, 29, |n| if n == 29 { "ada" } else { "bo" }), Origin::Unknown, &tid(29));
        assert!(matches!(eligible_strict(&only, &recent, &pid("ada"), 31 * DAY).cooling_off, Some(RuleVerdict::Ineligible(_))));
    }

    /// Readiness needs both: the journal read, and every configured rule decided in favour.
    #[test]
    fn a_successor_is_ready_only_when_it_has_read_and_eligibility_is_decided() {
        let mut journal = HandoverJournal::new();
        journal.record(JournalEntry {
            term: tid(30),
            author: pid("bo"),
            at_ms: 1,
            kind: EntryKind::Observation,
            text: "three merges failed the write gate".into(),
        });
        let mut successor = Successor::new();
        let unknown = eligible_strict(&rules(), &TermHistory::from_chain(chain(20, 30, |_| "bo"), Origin::Unknown, &tid(30)), &pid("ada"), 40 * DAY);
        assert!(matches!(ready(&successor, &journal, &unknown), Err(NotReadyStrict::Unread(_))), "reading comes first");
        successor.read_through(1);
        assert!(matches!(ready(&successor, &journal, &unknown), Err(NotReadyStrict::HistoryUnknown(_))), "read, but eligibility is not decided");
        let decided = eligible_strict(&rules(), &TermHistory::from_chain(chain(1, 30, |_| "bo"), Origin::Genesis, &tid(30)), &pid("ada"), 40 * DAY);
        assert_eq!(decided.verdict(), RuleVerdict::Eligible);
        assert!(ready(&successor, &journal, &decided).is_ok());
    }
}
