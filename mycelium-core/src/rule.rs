//! The **rule catalogue** — `docs/plans/guarantees-and-rule-catalogue.md`, increment I1's rule half.
//!
//! A *rule* is a decision point the substrate already has: where something is observed, checked,
//! chosen, refused or withdrawn. The catalogue makes those points **explicit, inspectable and
//! testable without changing their behaviour**: each [`RuleDescriptor`] names a decision by a stable
//! id, says what triggers it and what it reads, what it can decide and why, what it writes, which
//! guards it passes through, how it relates to other rules, where its code and its tests are, and
//! whether a decision trace (increment I4) records it.
//!
//! # What a descriptor is not
//!
//! Metadata. A descriptor executes nothing, schedules nothing and enforces nothing; the mechanism it
//! describes keeps doing exactly what it did. There is no rule engine behind this module and no
//! `evaluate_and_apply` trait (plan G2). The catalogue is generated from the descriptors and checked
//! in CI (`mycelium-wasm-host/tests/rule_catalogue.rs`), so the document cannot drift from the code
//! that owns it — but a generated document cannot establish that a description is *true*; only the
//! behavioural test each descriptor names can.
//!
//! # Four responsibilities, descriptive not architectural
//!
//! [`Responsibility`] names the question a rule answers — where information travels, whether a node
//! admits a stimulus, what action the receiving subsystem takes, whether a consequential action is
//! permitted — and a rule may answer more than one. A signal boundary is not a confidentiality
//! boundary, a group match is not an authority grant, and an observed unmet requirement is not an
//! instruction to install: the categories keep those apart in the description, as the code keeps
//! them apart in fact.
//!
//! # Where entries live
//!
//! Each crate registers its own rules as a `&'static [RuleDescriptor]` next to the code they describe
//! (`mycelium::rules`, `mycelium_wasm_host::rules`); the catalogue test gathers them. Ids are
//! `subsystem.name`, never a filename or a line number: rename the symbol, keep the id; change the
//! semantic revision when the *meaning* of the decision changes, not when its code moves.
//!
//! # Relationships are hypotheses
//!
//! `may_trigger`, `may_inhibit` and `depends_on` record possible influence. Until a trace or a test
//! shows it, an edge is a claim, and a diagram of the edges proves neither causation nor controller
//! stability (plan §6).

use serde::Serialize;

/// A stable rule id, `subsystem.name`.
pub type RuleId = &'static str;

/// The question a rule answers (plan §1 of the design note; one rule may answer several).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Responsibility {
    /// Where does information travel?
    Propagation,
    /// Does this node deliver an incoming stimulus to a handler?
    Admission,
    /// What action does the receiving subsystem take?
    Response,
    /// Is that consequential action permitted at its enforcement point?
    Authority,
}

/// How a decision ends, before its reason. A trace records the kind and a typed reason, never prose.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// The rule acted (installed, advertised, withdrew, forwarded, admitted).
    Action,
    /// The rule refused: a guard said no, by name.
    Refusal,
    /// The rule deferred: it would act and chose not to this time (a draw, a cooldown, a dedup).
    Deferral,
    /// Nothing to do: the trigger was observed and no condition for action held.
    NoAction,
}

/// One outcome a rule can produce and the typed reasons it can give for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub kind: OutcomeKind,
    /// Typed reason codes, `snake_case`; a trace carries one of these, never free text.
    pub reasons: &'static [&'static str],
}

/// A guard the decision passes through, named so a trace can say which one said no.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Guard {
    Authority,
    Provenance,
    ResourceBudget,
    Cooldown,
    Settling,
    Health,
    /// A guard specific to the rule, named in the descriptor's prose.
    Other,
}

/// Whether the decision trace (increment I4) records this rule.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePolicy {
    /// A sink attached at the decision point records every evaluation.
    Instrumented,
    /// Described here; no sink at the decision point yet.
    CatalogueOnly,
    /// Instrumented for the named subset of its outcomes only (a hot path records its refusals, not
    /// its admissions); the rest is catalogue-only, and the subset is stated here.
    Partial(&'static str),
    /// Tracing this point is not supported, for a stated reason (a hot path, a lock-sensitive site).
    Unsupported,
}

/// What a rule reads: a key family, a scope, a freshness assumption, an attached component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Input {
    /// A key family (`cap/`, `grp/`, `demand/`), an in-memory map, an attached component.
    pub source: &'static str,
    /// The scope the read is confined to, if any (`Group`, this node, the fleet as seen locally).
    pub scope: &'static str,
    /// What the rule assumes about the read's freshness ("as gossiped; may be stale", "local, atomic").
    pub freshness: &'static str,
}

/// One decision point, described (plan G1). Deliberately not `#[non_exhaustive]`: other crates build
/// these as `static` literals, so a field added here breaks an exhaustive literal — the repo's usual
/// upgrade-note class — rather than making the type unconstructible outside this crate.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct RuleDescriptor {
    /// Stable id, `subsystem.name`.
    pub id: RuleId,
    /// Semantic revision — bumped when the decision's meaning changes.
    pub revision: u32,
    /// The owning subsystem (the crate or module that implements it).
    pub subsystem: &'static str,
    pub responsibilities: &'static [Responsibility],
    /// What evaluates the rule: a tick, a frame, a demand, a probe result.
    pub trigger: &'static str,
    /// What the rule actually reads.
    pub inputs: &'static [Input],
    /// Every way the decision can end, with its typed reasons.
    pub outcomes: &'static [Outcome],
    /// What the rule writes or causes: a key family, an advertisement, a process, a frame.
    pub effects: &'static [&'static str],
    /// The guards the decision passes through, in order.
    pub guards: &'static [Guard],
    /// Rules this one may cause to evaluate (a hypothesis until shown).
    pub may_trigger: &'static [RuleId],
    /// Rules this one may stop from acting (a hypothesis until shown).
    pub may_inhibit: &'static [RuleId],
    /// Rules whose outcome this one reads.
    pub depends_on: &'static [RuleId],
    /// The implementing symbol, `module::function`, for a reader with the source open.
    pub symbol: &'static str,
    /// Where the rule is explained for a reader of the docs.
    pub docs: &'static str,
    /// The behavioural tests that pin the decision, by name — at least one.
    pub tests: &'static [&'static str],
    pub trace: TracePolicy,
    /// One sentence a reader of the catalogue can act on.
    pub summary: &'static str,
}

/// A problem in a set of descriptors, found by [`check`]; the catalogue test fails on any.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogueError {
    DuplicateId(String),
    /// `(rule, field, referenced id)`.
    UnknownReference(String, &'static str, String),
    /// A rule with no outcome, or an outcome with no reason code.
    MissingReasons(String),
    /// A rule naming no test.
    MissingTests(String),
    /// A rule naming no responsibility, or no summary.
    Incomplete(String),
    /// A reason code that is not `snake_case` ASCII.
    MalformedReason(String, String),
}

impl std::fmt::Display for CatalogueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogueError::DuplicateId(id) => write!(f, "duplicate rule id `{id}`"),
            CatalogueError::UnknownReference(r, field, to) => write!(f, "rule `{r}`: `{field}` references unknown rule `{to}`"),
            CatalogueError::MissingReasons(r) => write!(f, "rule `{r}`: an outcome without typed reasons (or no outcomes)"),
            CatalogueError::MissingTests(r) => write!(f, "rule `{r}`: names no behavioural test"),
            CatalogueError::Incomplete(r) => write!(f, "rule `{r}`: no responsibility or no summary"),
            CatalogueError::MalformedReason(r, c) => write!(f, "rule `{r}`: reason `{c}` is not snake_case"),
        }
    }
}

/// The structural checks a catalogue must pass: unique ids, every relation naming a registered rule,
/// typed reasons on every outcome, at least one test, a responsibility and a summary. These prevent
/// drift in *structure*; whether a description is true is the named test's job.
pub fn check(rules: &[&RuleDescriptor]) -> Vec<CatalogueError> {
    let mut errs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for r in rules {
        if !seen.insert(r.id) {
            errs.push(CatalogueError::DuplicateId(r.id.into()));
        }
    }
    for r in rules {
        if r.responsibilities.is_empty() || r.summary.trim().is_empty() {
            errs.push(CatalogueError::Incomplete(r.id.into()));
        }
        if r.outcomes.is_empty() || r.outcomes.iter().any(|o| o.reasons.is_empty()) {
            errs.push(CatalogueError::MissingReasons(r.id.into()));
        }
        for o in r.outcomes {
            for c in o.reasons {
                let ok = !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
                if !ok { errs.push(CatalogueError::MalformedReason(r.id.into(), (*c).into())); }
            }
        }
        if r.tests.is_empty() {
            errs.push(CatalogueError::MissingTests(r.id.into()));
        }
        for (field, ids) in [("may_trigger", r.may_trigger), ("may_inhibit", r.may_inhibit), ("depends_on", r.depends_on)] {
            for to in ids {
                if !seen.contains(to) {
                    errs.push(CatalogueError::UnknownReference(r.id.into(), field, (*to).into()));
                }
            }
        }
    }
    errs
}

/// The schema id of the generated catalogue; a change to the descriptor's serialised shape is a new one.
pub const CATALOGUE_SCHEMA: &str = "mycelium.rules/1";

/// The catalogue as one serialisable document: the schema, the descriptors in id order.
#[derive(Clone, Debug, Serialize)]
pub struct Catalogue<'a> {
    pub schema: &'static str,
    pub rules: Vec<&'a RuleDescriptor>,
}

impl<'a> Catalogue<'a> {
    /// Gather descriptors from every registering crate, sorted by id. Fails on any structural error.
    pub fn gather(sets: &[&'a [RuleDescriptor]]) -> Result<Self, Vec<CatalogueError>> {
        let mut rules: Vec<&RuleDescriptor> = sets.iter().flat_map(|s| s.iter()).collect();
        rules.sort_by_key(|r| r.id);
        let errs = check(&rules);
        if errs.is_empty() { Ok(Catalogue { schema: CATALOGUE_SCHEMA, rules }) } else { Err(errs) }
    }

    /// Gather one crate's descriptors on their own: a relation naming a rule another crate registers
    /// is returned beside the catalogue as an *external* reference rather than refusing it (the full
    /// gate, [`gather`](Self::gather), still refuses). Every other structural error still fails.
    pub fn gather_partial(sets: &[&'a [RuleDescriptor]]) -> Result<(Self, Vec<CatalogueError>), Vec<CatalogueError>> {
        let mut rules: Vec<&RuleDescriptor> = sets.iter().flat_map(|s| s.iter()).collect();
        rules.sort_by_key(|r| r.id);
        let (external, hard): (Vec<_>, Vec<_>) = check(&rules).into_iter().partition(|e| matches!(e, CatalogueError::UnknownReference(..)));
        if hard.is_empty() { Ok((Catalogue { schema: CATALOGUE_SCHEMA, rules }, external)) } else { Err(hard) }
    }

    /// The catalogue as pretty JSON (the shape of `docs/reference/rule-catalogue.json`).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// The human-readable catalogue, one section per rule, generated — never hand-edited.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Rule catalogue\n\n");
        out.push_str("**Generated** from the `RuleDescriptor`s each crate registers (`mycelium_core::rule`); do not edit. \
Regenerate with `UPDATE_RULE_CATALOGUE=1 cargo test -p mycelium-wasm-host --features stem --test rule_catalogue`. \
A rule is a decision point the substrate already has, described: what triggers it, what it reads, how it can end and why, \
what it writes, which guards it passes, how it may relate to other rules, where its code and tests are, and whether the \
decision trace records it. A descriptor executes nothing. A relationship is a hypothesis until a trace or a test shows it. \
The plan is `docs/plans/guarantees-and-rule-catalogue.md`.\n\n");
        out.push_str(&format!("Schema `{}` · {} rules.\n\n", self.schema, self.rules.len()));
        out.push_str("| Rule | Responsibilities | Trace | Summary |\n|---|---|---|---|\n");
        for r in &self.rules {
            out.push_str(&format!("| [`{}`](#{}) | {} | {} | {} |\n", r.id, anchor(r.id), join_dbg(r.responsibilities), fmt_trace(r.trace), r.summary));
        }
        for r in &self.rules {
            out.push_str(&format!("\n## `{}`\n\n", r.id));
            out.push_str(&format!("rev {} · `{}` · {} · trace: {}\n\n{}\n\n", r.revision, r.subsystem, join_dbg(r.responsibilities), fmt_trace(r.trace), r.summary));
            out.push_str(&format!("- **Trigger:** {}\n", r.trigger));
            if !r.inputs.is_empty() {
                out.push_str("- **Reads:**\n");
                for i in r.inputs { out.push_str(&format!("  - `{}` — scope: {}; freshness: {}\n", i.source, i.scope, i.freshness)); }
            }
            out.push_str("- **Outcomes:**\n");
            for o in r.outcomes { out.push_str(&format!("  - {:?}: `{}`\n", o.kind, o.reasons.join("`, `"))); }
            if !r.effects.is_empty() { out.push_str(&format!("- **Effects:** {}\n", r.effects.join(" · "))); }
            if !r.guards.is_empty() { out.push_str(&format!("- **Guards:** {}\n", join_dbg(r.guards))); }
            for (name, ids) in [("May trigger", r.may_trigger), ("May inhibit", r.may_inhibit), ("Depends on", r.depends_on)] {
                if !ids.is_empty() { out.push_str(&format!("- **{name}:** {}\n", ids.iter().map(|i| format!("[`{i}`](#{})", anchor(i))).collect::<Vec<_>>().join(", "))); }
            }
            out.push_str(&format!("- **Code:** `{}` · **Docs:** {} · **Tests:** `{}`\n", r.symbol, r.docs, r.tests.join("`, `")));
        }
        out
    }
}

fn anchor(id: &str) -> String { id.replace('.', "") }
fn join_dbg<T: std::fmt::Debug>(xs: &[T]) -> String { xs.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", ") }
fn fmt_trace(t: TracePolicy) -> String {
    match t {
        TracePolicy::Instrumented => "instrumented".into(),
        TracePolicy::CatalogueOnly => "catalogue only".into(),
        TracePolicy::Partial(what) => format!("partial — {what}"),
        TracePolicy::Unsupported => "unsupported".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn r(id: RuleId, deps: &'static [RuleId], outcomes: &'static [Outcome], tests: &'static [&'static str]) -> RuleDescriptor {
        RuleDescriptor {
            id, revision: 1, subsystem: "t", responsibilities: &[Responsibility::Response], trigger: "t", inputs: &[],
            outcomes, effects: &[], guards: &[], may_trigger: &[], may_inhibit: &[], depends_on: deps,
            symbol: "t::t", docs: "d", tests, trace: TracePolicy::CatalogueOnly, summary: "s",
        }
    }
    const OK: &[Outcome] = &[Outcome { kind: OutcomeKind::Action, reasons: &["done"] }];

    /// The structural checks: a duplicate id, a dangling relation, an outcome without reasons, a rule
    /// without a test, and a reason that is not snake_case are each a named error.
    #[test]
    fn the_structural_checks_name_each_defect() {
        static A: RuleDescriptor = r("a.one", &[], OK, &["t_a"]);
        static A2: RuleDescriptor = r("a.one", &[], OK, &["t_a"]);
        static B: RuleDescriptor = r("b.one", &["ghost.id"], OK, &["t_b"]);
        static C: RuleDescriptor = r("c.one", &[], &[Outcome { kind: OutcomeKind::Refusal, reasons: &[] }], &["t_c"]);
        static D: RuleDescriptor = r("d.one", &[], OK, &[]);
        static E: RuleDescriptor = r("e.one", &[], &[Outcome { kind: OutcomeKind::Action, reasons: &["Not-Snake"] }], &["t_e"]);
        let errs = check(&[&A, &A2, &B, &C, &D, &E]);
        assert!(errs.contains(&CatalogueError::DuplicateId("a.one".into())));
        assert!(errs.contains(&CatalogueError::UnknownReference("b.one".into(), "depends_on", "ghost.id".into())));
        assert!(errs.contains(&CatalogueError::MissingReasons("c.one".into())));
        assert!(errs.contains(&CatalogueError::MissingTests("d.one".into())));
        assert!(errs.contains(&CatalogueError::MalformedReason("e.one".into(), "Not-Snake".into())));
        assert_eq!(errs.len(), 5, "{errs:?}");
    }

    /// A partial gather keeps a relation into another crate as an external reference and still refuses
    /// a hard error.
    #[test]
    fn a_partial_gather_reports_external_references_and_refuses_hard_errors() {
        static X: RuleDescriptor = r("a.one", &["other.crate"], OK, &["t"]);
        let (c, external) = Catalogue::gather_partial(&[std::slice::from_ref(&X)]).expect("external references are not refused");
        assert_eq!((c.rules.len(), external.len()), (1, 1));
        assert!(Catalogue::gather(&[std::slice::from_ref(&X)]).is_err(), "the full gate still refuses");
        static D: RuleDescriptor = r("d.one", &[], OK, &[]);
        assert!(Catalogue::gather_partial(&[std::slice::from_ref(&D)]).is_err(), "no test is a hard error");
    }

    /// A clean set gathers, sorts by id, and renders with its schema and one section per rule.
    #[test]
    fn a_clean_set_gathers_and_renders() {
        static X: RuleDescriptor = r("z.last", &["a.first"], OK, &["t"]);
        static Y: RuleDescriptor = r("a.first", &[], OK, &["t"]);
        let (a, b): (&[RuleDescriptor], &[RuleDescriptor]) = (std::slice::from_ref(&X), std::slice::from_ref(&Y));
        let c = Catalogue::gather(&[a, b]).expect("clean");
        assert_eq!(c.rules.iter().map(|r| r.id).collect::<Vec<_>>(), ["a.first", "z.last"]);
        let md = c.to_markdown();
        assert!(md.contains("## `a.first`") && md.contains("[`a.first`](#afirst)") && md.contains(CATALOGUE_SCHEMA));
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["schema"], CATALOGUE_SCHEMA);
        assert_eq!(json["rules"][1]["depends_on"][0], "a.first");
    }
}
