//! **Strict eligibility** — realignment repairs A3 (`docs/plans/realignment-repairs.md` §3.4, D4).
//!
//! ```text
//! cargo run --example strict_eligibility
//! ```
//!
//! # The claim this exists to make checkable
//!
//! *Unknown history is never eligible.* A council limits how long one curator may hold the role. The
//! lenient check (`handover::eligible`) evaluates whatever history it is handed, and a history that
//! silently lacks one of the candidate's earlier terms passes the limit. The strict check
//! (`eligibility::eligible_strict`) takes history whose coverage the **source** verified — chained
//! terms, an origin, the current head — and answers `Unknown` until the missing term is back. A
//! successor is ready only when it has read the handover journal **and** every configured rule is
//! decided in its favour.
//!
//! # What it does not demonstrate
//!
//! That the source recorded every appointment there was: a source vouches for the chain it holds.
//! Nor the resource's own authority check — the protected write's refusal is shown in
//! `mycelium-wiki/examples/curator_handover.rs`, and this example is the readiness that precedes it.

use mycelium::mandate::eligibility::{eligible_strict, ready, ChainedTerm, Origin, RuleVerdict, TermHistory};
use mycelium::mandate::handover::{eligible, EntryKind, HandoverJournal, IncumbencyRules, JournalEntry, Successor, TermRecord};
use mycelium::mandate::{PrincipalId, TermId};

const DAY: u64 = 86_400_000;

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

fn pid(s: &str) -> PrincipalId {
    PrincipalId::new(s).expect("a principal")
}

fn tid(n: u32) -> TermId {
    TermId::new(format!("term-{n}")).expect("a term id")
}

/// Terms 1..=12, a week each; ada holds every odd one.
fn the_record() -> Vec<ChainedTerm> {
    (1..=12)
        .map(|n| ChainedTerm {
            record: TermRecord {
                holder: pid(if n % 2 == 1 { "curator-ada" } else { "curator-bo" }),
                term: tid(n),
                started_ms: n as u64 * 7 * DAY,
                ended_ms: (n as u64 + 1) * 7 * DAY,
            },
            previous: if n == 1 { None } else { Some(tid(n - 1)) },
        })
        .collect()
}

fn main() {
    println!("\x1b[1mStrict eligibility — unknown history is never eligible\x1b[0m");
    let rules = IncumbencyRules { max_cumulative_ms: Some(42 * DAY), ..IncumbencyRules::default() };
    let ada = pid("curator-ada");
    let now = 100 * 7 * DAY;
    note("the council's rule: no curator holds the role for more than 42 days in total");
    note("ada has held six one-week terms (1, 3, 5, 7, 9, 11) — exactly 42 days");

    step(1, "the history the node can reach is missing term 7");
    let gapped: Vec<ChainedTerm> = the_record().into_iter().filter(|t| t.record.term != tid(7)).collect();
    let lenient: Vec<TermRecord> = gapped.iter().map(|t| t.record.clone()).collect();
    note(format!("lenient check on what it was handed: {:?}", eligible(&rules, &lenient, &ada, now)));
    note("— 35 visible days, under the limit: yes. The missing week is invisible to it.");

    step(2, "the strict check verifies the chain first");
    let history = TermHistory::from_chain(gapped, Origin::Genesis, &tid(12));
    note(format!("coverage: {:?}", history.coverage()));
    let strict = eligible_strict(&rules, &history, &ada, now);
    note(format!("cumulative tenure: {:?}", strict.cumulative));
    note("term 8 does not follow term 6, so only terms 8–12 are vouched for, and they do not start at");
    note("the role's first term: the total before them is not known. Unknown — not yes.");

    step(3, "a successor that has read its handover journal is still not ready");
    let mut journal = HandoverJournal::new();
    journal.record(JournalEntry {
        term: tid(12),
        author: pid("curator-bo"),
        at_ms: 13 * 7 * DAY,
        kind: EntryKind::Observation,
        text: "three merges failed the write gate this term".into(),
    });
    let mut successor = Successor::new();
    successor.read_through(journal.len());
    note(format!("ready: {:?}", ready(&successor, &journal, &strict).map_err(|e| e.to_string())));
    note("reading the journal and being eligible are two conditions; the first does not imply the second.");

    step(4, "the missing record is fetched");
    let whole = TermHistory::from_chain(the_record(), Origin::Genesis, &tid(12));
    let decided = eligible_strict(&rules, &whole, &ada, now);
    note(format!("cumulative tenure: {:?}", decided.cumulative));
    assert!(matches!(decided.verdict(), RuleVerdict::Ineligible(_)), "42 days meets the 42-day limit");
    assert!(matches!(strict.verdict(), RuleVerdict::Unknown(_)), "and the gapped history said so rather than yes");
    note("with term 7 back the answer is decided: ada has served 42 days and may not be reappointed.");
    note("the lenient check had said yes to the same candidate — on a history one week short.");

    step(5, "what this demonstration does not establish");
    note("· that the source recorded every appointment there was — it vouches for the chain it holds");
    note("· the protected write's refusal at the resource — curator_handover (mycelium-wiki) shows that");
    println!();
}
