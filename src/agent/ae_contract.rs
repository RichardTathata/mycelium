//! **The AE contract fixtures — what the seam requires of *any* evaluator behind it.**
//!
//! AE4's gate (plan §6.8): *both scenarios and **a replacement evaluator** pass the same contract
//! fixtures.* This module is the "same contract fixtures" part, and it exists because until now
//! there was no such thing. AE0 §9's negative cases were real and were tested — but each was
//! written inline against [`ReferenceEvaluator`], constructing that evaluator's own rule types. An
//! adopter with a different evaluator could not run a single one of them, which means they were
//! tests of the reference implementation rather than statements of the contract.
//!
//! # What a contract fixture is
//!
//! A [`ContractCase`] states an intent in evaluator-neutral terms — *this policy says this*, *this
//! envelope arrives*, *the contract requires this outcome*. Each evaluator translates
//! [`PolicyIntent`] into whatever it holds internally, and the suite judges only what came out of
//! the seam.
//!
//! The vocabulary in [`PolicyClause`] is deliberately tiny. It is not a policy language and must
//! never grow into one: every clause here exists because one of AE0 §9's cases needs it, and a
//! richer vocabulary would start expressing the reference evaluator's shape again.
//!
//! # Why the checks are about identifiers, not wording
//!
//! AE0 requires a refusal to report *what it could not establish*. Two evaluators will word that
//! differently and both be right, so [`ContractCase::must_mention`] asserts the **identifier**
//! appears somewhere in the decision — the fact's name, the clause's name, the revision — and
//! never that a particular sentence was produced. A suite that matched prose would fail a
//! conforming evaluator for having its own voice.
//!
//! # Declining is not passing
//!
//! An evaluator may be unable to express a clause at all. That is allowed and is sometimes
//! correct — but it is **not** a pass, and it must be declared in advance. [`Report::conformant`]
//! requires the set of cases an evaluator declined to equal the set it declared it would decline,
//! exactly: no silent gaps, and no stale declarations for cases it can now handle. A suite that
//! skipped what an implementation found hard would certify the opposite of what it claims.
//!
//! # What passing here is worth
//!
//! It is evidence that an evaluator obeys the seam's contract on the cases AE0 enumerated. It is
//! not evidence that a policy is correct, that a catalogue is right, or that the enforcement point
//! is the only route to the effect — that last one is posture rule 6's territory and no fixture
//! here can speak to it.
//!
//! **And it is not all of AE0 §9.** Nine of its eleven rows are gated here; two are not properties
//! of a decision at all, and *substitution* is covered only in part. [`COVERAGE`] says which and
//! why, row by row, because §9 describes its fixtures as the gate "for every evaluator" and an
//! adopter reading that sentence should be able to find out exactly what passing it buys them.

use super::action_evaluator::{
    ActionEnvelope, ActionEvaluator, ActionMapping, Decision, PreflightRefusal, ReferenceEvaluator,
    Rule, Verdict, preflight,
};
use mycelium_core::NodeId;
use std::sync::Arc;

/// One statement a policy can make, in terms every evaluator can be asked to express.
///
/// Deliberately minimal — see the module doc. Each variant is here because an AE0 §9 case needs
/// it, and nothing is here because a policy language usually has it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyClause {
    /// This actor may perform this operation on this resource. `*` is any.
    Allow {
        /// Actor principal, or `*`.
        actor: String,
        /// Operation, or `*`.
        operation: String,
        /// Resource, or `*`.
        resource: String,
    },
    /// This actor may **not**. A prohibition is an authority deciding *no*, which is a different
    /// fact from nothing having been decided.
    Forbid {
        /// Actor principal, or `*`.
        actor: String,
        /// Operation, or `*`.
        operation: String,
        /// Resource, or `*`.
        resource: String,
    },
    /// An allowance conditional on facts the evaluator must establish. A fact it cannot establish
    /// makes the clause *indeterminate* — never a denial, and never a quiet permit.
    AllowRequiringFacts {
        /// Actor principal, or `*`.
        actor: String,
        /// Operation, or `*`.
        operation: String,
        /// Resource, or `*`.
        resource: String,
        /// The facts the clause depends on.
        facts: Vec<String>,
    },
    /// A clause name this policy uses that the evaluator has declared it does not support.
    ///
    /// The case that matters: an adapter must not silently ignore a clause it cannot evaluate,
    /// because ignoring a restriction turns it into permission.
    UnsupportedClause(String),
}

impl PolicyClause {
    /// `Allow`, with `&str` arguments.
    pub fn allow(actor: &str, operation: &str, resource: &str) -> Self {
        Self::Allow {
            actor: actor.into(),
            operation: operation.into(),
            resource: resource.into(),
        }
    }

    /// `Forbid`, with `&str` arguments.
    pub fn forbid(actor: &str, operation: &str, resource: &str) -> Self {
        Self::Forbid {
            actor: actor.into(),
            operation: operation.into(),
            resource: resource.into(),
        }
    }

    /// `AllowRequiringFacts`, with `&str` arguments.
    pub fn allow_requiring(actor: &str, operation: &str, resource: &str, facts: &[&str]) -> Self {
        Self::AllowRequiringFacts {
            actor: actor.into(),
            operation: operation.into(),
            resource: resource.into(),
            facts: facts.iter().map(|f| (*f).to_string()).collect(),
        }
    }
}

/// A whole policy, and the revision its decisions report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyIntent {
    /// What the evaluator must report as `policy_revision`. The seam compares this against what
    /// the enforcement point expected, so it is part of the contract rather than a label.
    pub revision: String,
    /// What the policy says.
    pub clauses: Vec<PolicyClause>,
}

impl PolicyIntent {
    /// A policy reporting `revision` and saying `clauses`.
    pub fn new(revision: &str, clauses: Vec<PolicyClause>) -> Self {
        Self { revision: revision.into(), clauses }
    }
}

/// What the contract requires the seam to do with a case.
///
/// The distinction between the last two is the whole slice: **a denial says an authority decided
/// no; not-established says nobody decided.** An evaluator that collapses them turns an uncovered
/// action into evidence of drift, or a real prohibition into a shrug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expected {
    /// The action is admitted, carrying a `Permit`.
    Admitted,
    /// Refused, because an authority decided against it.
    Denied,
    /// Refused, because the authority to do it was never established.
    NotEstablished,
    /// Refused because the **evidence** could not be recorded — even though the policy permitted
    /// it. An action allowed to proceed with no record of why is an unlogged gate, not governance.
    ///
    /// **No case in [`cases`] produces this**, and that is a property of where it is raised rather
    /// than an omission: it comes from the recording step at the enforcement point, after
    /// evaluation has already succeeded. It is named here so the mapping from
    /// [`PreflightRefusal`] stays total — an outcome quietly folded into `NotEstablished` would
    /// tell an operator that authority was missing when in fact the journal was.
    NotRecorded,
}

/// One case: an intent, an envelope, and what the contract requires.
#[derive(Clone, Debug)]
pub struct ContractCase {
    /// Stable identifier, used in reports and in an evaluator's declared limits.
    pub id: &'static str,
    /// The requirement in words, traceable to AE0 §9.
    pub requirement: &'static str,
    /// What the policy says.
    pub policy: PolicyIntent,
    /// What arrives at the enforcement point.
    pub envelope: ActionEnvelope,
    /// The enforcement point's clock for this case. The evaluator holds no clock; the seam does.
    pub now_ms: u64,
    /// What the contract requires.
    pub expect: Expected,
    /// Identifiers the decision must name somewhere — **not** wording. See the module doc.
    pub must_mention: Vec<String>,
}

/// Why an evaluator cannot express a clause.
///
/// Carrying the reason rather than a bare `None`: an operator choosing between adapters needs to
/// know *what* is not covered, and "this adapter declined three cases" is not an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unexpressible {
    /// The clause, as this evaluator sees it.
    pub clause: String,
    /// Why it cannot be expressed.
    pub because: String,
}

/// An evaluator offered to the contract suite.
///
/// Implement this beside an [`ActionEvaluator`] to check it against AE0 §9. The suite never
/// inspects the evaluator's internals — it builds, runs the seam, and judges the outcome.
pub trait EvaluatorUnderTest {
    /// A name for the report.
    fn name(&self) -> &str;

    /// Build an evaluator expressing `intent`, or say which clause cannot be expressed.
    fn build(&self, intent: &PolicyIntent) -> Result<Arc<dyn ActionEvaluator>, Unexpressible>;

    /// Case ids this evaluator declares it cannot express.
    ///
    /// Declared **in advance**, and checked for exactness by [`Report::conformant`]. Default: it
    /// claims to express everything, which is the claim most adapters should be held to.
    fn declared_limits(&self) -> &[&'static str] {
        &[]
    }
}

/// One case an evaluator got wrong.
#[derive(Clone, Debug)]
pub struct Failure {
    /// Which case.
    pub case: &'static str,
    /// The requirement it failed.
    pub requirement: &'static str,
    /// What the contract required.
    pub expected: Expected,
    /// What the seam actually did.
    pub actual: String,
    /// The decision, for a reader working out why.
    pub decision: Option<Decision>,
}

/// How an evaluator did against the suite.
#[derive(Clone, Debug)]
pub struct Report {
    /// Which evaluator.
    pub evaluator: String,
    /// Cases it satisfied.
    pub passed: Vec<&'static str>,
    /// Cases it got wrong.
    pub failed: Vec<Failure>,
    /// Cases it could not express, with the reason.
    pub declined: Vec<(&'static str, Unexpressible)>,
    /// What the subject declared it would decline, copied here so [`Report::conformant`] can
    /// check the two against each other without the caller re-supplying it.
    pub declared: Vec<&'static str>,
}

impl Report {
    /// Did this evaluator meet the contract?
    ///
    /// Requires every case to pass **and** the declined set to equal the declared set exactly. A
    /// case declined but not declared is a silent gap; a case declared but not declined is a stale
    /// claim of weakness that an operator would read as a limitation it does not have.
    pub fn conformant(&self) -> bool {
        if !self.failed.is_empty() {
            return false;
        }
        let mut declined: Vec<&str> = self.declined.iter().map(|(id, _)| *id).collect();
        declined.sort_unstable();
        let mut declared: Vec<&str> = self.declared.clone();
        declared.sort_unstable();
        declined == declared
    }

    /// A one-line summary for a CI log.
    pub fn summary(&self) -> String {
        format!(
            "{}: {} passed, {} failed, {} declined",
            self.evaluator,
            self.passed.len(),
            self.failed.len(),
            self.declined.len()
        )
    }
}

// `declared` is kept beside the outcome so `conformant` can check it without the caller
// re-supplying what the subject already stated.
impl Report {
    fn with_declared(mut self, declared: Vec<&'static str>) -> Self {
        self.declared = declared;
        self
    }
}

/// Run every contract case against `subject`.
pub fn run(subject: &dyn EvaluatorUnderTest) -> Report {
    run_cases(subject, &cases())
}

/// Run a chosen set of cases — for a suite extended with an adopter's own.
pub fn run_cases(subject: &dyn EvaluatorUnderTest, cases: &[ContractCase]) -> Report {
    let mut report = Report {
        evaluator: subject.name().to_string(),
        passed: Vec::new(),
        failed: Vec::new(),
        declined: Vec::new(),
        declared: Vec::new(),
    };

    for case in cases {
        let evaluator = match subject.build(&case.policy) {
            Ok(e) => e,
            Err(why) => {
                report.declined.push((case.id, why));
                continue;
            }
        };

        let outcome = preflight(Some(&evaluator), &case.envelope, case.now_ms);
        let (actual, decision) = match &outcome {
            Ok(Some(d)) => (Expected::Admitted, Some(d.clone())),
            // No evaluator attached is impossible here: one was just built.
            Ok(None) => (Expected::NotEstablished, None),
            Err(PreflightRefusal::Denied(d)) => (Expected::Denied, Some(d.clone())),
            Err(PreflightRefusal::NotEstablished(d)) => (Expected::NotEstablished, Some(d.clone())),
            Err(PreflightRefusal::NotRecorded(d)) => (Expected::NotRecorded, Some(d.clone())),
        };

        if actual != case.expect {
            report.failed.push(Failure {
                case: case.id,
                requirement: case.requirement,
                expected: case.expect,
                actual: format!("{actual:?}"),
                decision,
            });
            continue;
        }

        // The refusal must NAME what it could not establish. Identifiers, never wording.
        if let Some(d) = &decision {
            let said = format!("{} {} {}", d.reason, d.checked.join(" "), d.errors.join(" "));
            if let Some(missing) = case.must_mention.iter().find(|m| !said.contains(m.as_str())) {
                report.failed.push(Failure {
                    case: case.id,
                    requirement: case.requirement,
                    expected: case.expect,
                    actual: format!("reached the right outcome without naming {missing:?}"),
                    decision,
                });
                continue;
            }
        }

        report.passed.push(case.id);
    }

    report.with_declared(subject.declared_limits().to_vec())
}

// ── the cases ─────────────────────────────────────────────────────────────────────────────────

fn node() -> NodeId {
    NodeId::new("127.0.0.1", 9000).expect("a well-formed node id")
}

/// The envelope every case starts from: mapped, scoped, expecting `rev-1`, valid to 61_000.
fn base(actor: &str, operation: &str, resource: &str) -> ActionEnvelope {
    ActionEnvelope::builder(actor, node(), operation, resource)
        .identities("op-1", "op-1/1")
        .scopes(["mcp:invoke"])
        .arguments_digest([1u8; 32])
        .mapping(ActionMapping::mapped("cat-procurement", "7"))
        .expected_policy_revision("rev-1")
        .validity(1_000, 61_000)
        .build()
}

/// Which of AE0 §9's eleven rows this suite covers, and which it cannot.
///
/// §9 calls its fixtures "the CI gate for the seam **and for every evaluator**". Nine of its rows
/// are gated here. Two are not, and one only partly — not by omission but because they are not
/// properties of a decision, and a suite that quietly claimed them would be the overclaim this
/// axis exists to stop making.
///
/// | AE0 §9 row | here | why |
/// |---|---|---|
/// | impersonation | ✅ `impersonation` | |
/// | missing facts | ✅ `missing_facts` | |
/// | unsupported clause | ✅ `unsupported_clause` | |
/// | unmapped operation | ✅ `unmapped_operation` | |
/// | ambiguous mapping | ✅ `ambiguous_operation` | |
/// | stale policy | ✅ `stale_policy_revision` | |
/// | expired validity | ✅ `expired_envelope` | |
/// | explicit prohibition | ✅ `explicit_prohibition` | |
/// | incomplete allow-list | ✅ `incomplete_allow_list` | |
/// | **substitution** | ⚠️ partial | §9's row is about **reusing a decision bound to one argument digest for a different one**. The seam holds no decision cache — it evaluates each envelope once — so there is nothing here to substitute *into*. `arguments_are_evaluated_not_inherited` checks only that the digest rides in the envelope and does not change the outcome of a fresh evaluation. Anything that caches decisions must gate the reuse itself; this suite cannot do it for them. |
/// | **shared identity** | ❌ | a property of the evidence *record* (it carries no `execution_identity`), not of the decision. Gated where the record is written. |
/// | **unobserved route** | ❌ | an effect that never reached the enforcement point produces no decision to judge. This is posture rule 6's territory: it is answered by `coverage.complete: false` naming the route, and no fixture about decisions can speak to an action that made none. |
///
/// Also not covered: a permit returned **with evaluation errors**, and a **panicking** evaluator.
/// Both are seam behaviours triggered by how an evaluator misbehaves rather than by what a policy
/// says, so [`PolicyIntent`] cannot express them; they are gated in the seam's own tests.
pub const COVERAGE: &str = "9 of AE0 §9's 11 rows gated; substitution partial; shared-identity and \
unobserved-route are not decision properties — see the module doc";

/// AE0 §9's negative cases, plus the permit they are all the negative of.
///
/// Every evaluator behind the seam must reach these outcomes. They are stated once, here, so that
/// a replacement evaluator is held to the same bar as the reference one — which is the whole of
/// AE4's gate.
pub fn cases() -> Vec<ContractCase> {
    let now = 2_000;
    vec![
        ContractCase {
            id: "permit",
            requirement: "an allowance that covers the action admits it, and says what it checked",
            policy: PolicyIntent::new(
                "rev-1",
                vec![PolicyClause::allow("oidc:idp/alice", "tools/call", "tool:square@n1")],
            ),
            envelope: base("oidc:idp/alice", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::Admitted,
            must_mention: vec![],
        },
        ContractCase {
            id: "explicit_prohibition",
            requirement: "AE0 §9 — a prohibition DENIES: an authority decided no",
            policy: PolicyIntent::new(
                "rev-1",
                vec![
                    PolicyClause::allow("*", "*", "*"),
                    PolicyClause::forbid("oidc:idp/mallory", "tools/call", "*"),
                ],
            ),
            envelope: base("oidc:idp/mallory", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::Denied,
            must_mention: vec![],
        },
        ContractCase {
            id: "incomplete_allow_list",
            requirement:
                "AE0 §9 — an action no clause covers is NOT ESTABLISHED, never denied: evidence \
                 must not read an uncovered action as drift",
            policy: PolicyIntent::new(
                "rev-1",
                vec![PolicyClause::allow("oidc:idp/alice", "tools/call", "tool:square@n1")],
            ),
            envelope: base("oidc:idp/alice", "tools/call", "tool:cube@n1"),
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec![],
        },
        ContractCase {
            id: "impersonation",
            requirement:
                "AE0 §9 — an allowance naming one actor establishes nothing for another, however \
                 similar the request",
            policy: PolicyIntent::new(
                "rev-1",
                vec![PolicyClause::allow("oidc:idp/alice", "tools/call", "tool:square@n1")],
            ),
            envelope: base("oidc:idp/mallory", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec![],
        },
        ContractCase {
            id: "missing_facts",
            requirement:
                "AE0 §9 — a clause resting on a fact the evaluator cannot establish is \
                 indeterminate, and NAMES the fact",
            policy: PolicyIntent::new(
                "rev-1",
                vec![PolicyClause::allow_requiring("*", "tools/call", "*", &["mandate"])],
            ),
            envelope: base("oidc:idp/alice", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec!["mandate".into()],
        },
        ContractCase {
            id: "unsupported_clause",
            requirement:
                "AE0 §9 — a clause the adapter does not support must not be ignored: ignoring a \
                 restriction turns it into permission",
            policy: PolicyIntent::new(
                "rev-1",
                vec![
                    PolicyClause::allow_requiring("*", "tools/call", "*", &["odrl:duty"]),
                    PolicyClause::UnsupportedClause("odrl:duty".into()),
                ],
            ),
            envelope: base("oidc:idp/alice", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec!["odrl:duty".into()],
        },
        ContractCase {
            id: "unmapped_operation",
            requirement:
                "AE0 §9 — a permit cannot name an unmapped operation as a business operation; the \
                 seam downgrades whatever the evaluator said",
            policy: PolicyIntent::new("rev-1", vec![PolicyClause::allow("*", "*", "*")]),
            envelope: {
                let mut e = base("oidc:idp/alice", "tools/call", "tool:execute_shell@n1");
                e.mapping = ActionMapping::unmapped("cat-procurement", "7");
                e
            },
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec![],
        },
        ContractCase {
            id: "ambiguous_operation",
            requirement:
                "AE0 §9 — an ambiguous mapping is refused on the same ground as an unmapped one",
            policy: PolicyIntent::new("rev-1", vec![PolicyClause::allow("*", "*", "*")]),
            envelope: {
                let mut e = base("oidc:idp/alice", "tools/call", "tool:send_email@n1");
                e.mapping = ActionMapping::ambiguous("cat-procurement", "7");
                e
            },
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec![],
        },
        ContractCase {
            id: "stale_policy_revision",
            requirement:
                "AE0 §9 — a decision from a revision this enforcement point does not expect \
                 establishes nothing, and the report names both revisions",
            policy: PolicyIntent::new("rev-2", vec![PolicyClause::allow("*", "*", "*")]),
            envelope: base("oidc:idp/alice", "tools/call", "tool:square@n1"),
            now_ms: now,
            expect: Expected::NotEstablished,
            must_mention: vec!["rev-1".into(), "rev-2".into()],
        },
        ContractCase {
            id: "expired_envelope",
            requirement:
                "AE0 §9 — validity is the seam's check, not the evaluator's: an expired envelope \
                 is DENIED before any evaluator runs",
            policy: PolicyIntent::new("rev-1", vec![PolicyClause::allow("*", "*", "*")]),
            envelope: base("oidc:idp/alice", "tools/call", "tool:square@n1"),
            now_ms: 61_001,
            expect: Expected::Denied,
            must_mention: vec!["validity".into()],
        },
        ContractCase {
            id: "arguments_are_evaluated_not_inherited",
            requirement:
                "an envelope carrying a different argument digest is evaluated on its own merits. \
                 NOTE: this is *not* AE0 §9's substitution row — see COVERAGE",
            policy: PolicyIntent::new(
                "rev-1",
                vec![PolicyClause::allow("oidc:idp/alice", "tools/call", "tool:pay@n1")],
            ),
            envelope: {
                let mut e = base("oidc:idp/alice", "tools/call", "tool:pay@n1");
                e.arguments_digest = [9u8; 32];
                e
            },
            now_ms: now,
            expect: Expected::Admitted,
            must_mention: vec![],
        },
    ]
}

// ── the reference evaluator's adapter, shipped as the worked example ──────────────────────────

/// [`ReferenceEvaluator`] offered to the suite — and the worked example an adopter copies.
///
/// It is deliberately short: translating [`PolicyIntent`] is meant to be a morning's work for any
/// adapter, and if it is not, the clause vocabulary has grown past its purpose.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReferenceUnderTest;

impl EvaluatorUnderTest for ReferenceUnderTest {
    fn name(&self) -> &str {
        "ReferenceEvaluator"
    }

    fn build(&self, intent: &PolicyIntent) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
        let mut ev = ReferenceEvaluator::new(&intent.revision);
        for clause in &intent.clauses {
            ev = match clause {
                PolicyClause::Allow { actor, operation, resource } => {
                    ev.allow(Rule::new(actor, operation, resource).requiring_scopes(["mcp:invoke"]))
                }
                PolicyClause::Forbid { actor, operation, resource } => {
                    ev.prohibit(Rule::new(actor, operation, resource))
                }
                PolicyClause::AllowRequiringFacts { actor, operation, resource, facts } => ev
                    .allow(
                        Rule::new(actor, operation, resource)
                            .requiring_scopes(["mcp:invoke"])
                            .requiring_facts(facts.clone()),
                    ),
                PolicyClause::UnsupportedClause(name) => ev.unsupported_clause(name),
                // No wildcard arm, deliberately. `PolicyClause` is `#[non_exhaustive]` for
                // downstream adapters, but in this crate the match is exhaustive — so adding a
                // clause breaks the build here and forces a decision about it. A wildcard would
                // silently drop the new clause, and dropping a restriction grants it: exactly the
                // failure `UnsupportedClause` one arm up exists to prevent.
            };
        }
        Ok(Arc::new(ev))
    }
}

/// The verdict the seam reported, for a caller that wants the raw answer rather than a report.
///
/// Exposed because an adopter debugging one case should not have to re-derive the mapping from
/// [`PreflightRefusal`] to [`Expected`].
pub fn outcome_of(
    evaluator: &Arc<dyn ActionEvaluator>,
    envelope: &ActionEnvelope,
    now_ms: u64,
) -> (Expected, Option<Decision>) {
    match preflight(Some(evaluator), envelope, now_ms) {
        Ok(Some(d)) => (Expected::Admitted, Some(d)),
        Ok(None) => (Expected::NotEstablished, None),
        Err(PreflightRefusal::Denied(d)) => (Expected::Denied, Some(d)),
        Err(PreflightRefusal::NotEstablished(d)) => (Expected::NotEstablished, Some(d)),
        Err(PreflightRefusal::NotRecorded(d)) => (Expected::NotRecorded, Some(d)),
    }
}

/// True when the decision is a permit. Convenience for an adopter's own cases.
pub fn is_permit(decision: Option<&Decision>) -> bool {
    decision.map(|d| d.verdict == Verdict::Permit).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::action_evaluator::Verdict;

    // ── a genuinely independent replacement evaluator ─────────────────────────────────────────
    //
    // Written from the seam's contract rather than from `ReferenceEvaluator`: it shares no types
    // with it, scans in a different order, and words every message differently. That is the whole
    // point — a replacement that wrapped the reference would prove only that the reference calls
    // itself.

    #[derive(Clone, Debug)]
    enum Entry {
        Permit { who: String, what: String, on: String },
        Refuse { who: String, what: String, on: String },
        Conditional { who: String, what: String, on: String, upon: Vec<String> },
    }

    /// A table-driven evaluator. Prohibitions are scanned **last** here and still win, which is a
    /// different route to the same contract than the reference's "prohibitions first".
    #[derive(Clone, Debug)]
    struct TableEvaluator {
        revision: String,
        entries: Vec<Entry>,
        cannot_evaluate: Vec<String>,
    }

    fn glob(pattern: &str, value: &str) -> bool {
        pattern == "*" || pattern == value
    }

    impl ActionEvaluator for TableEvaluator {
        fn evaluate(&self, env: &ActionEnvelope) -> Decision {
            let hits = |who: &str, what: &str, on: &str| {
                glob(who, &env.actor) && glob(what, &env.operation) && glob(on, &env.resource)
            };

            // Anything conditional that matches: the condition decides, and an unmet condition is
            // *not established* rather than a refusal. Checked before permits so a conditional
            // permit cannot be short-circuited by a broader unconditional one.
            for entry in &self.entries {
                if let Entry::Conditional { who, what, on, upon } = entry
                    && hits(who, what, on)
                {
                    let unmet: Vec<String> = upon
                        .iter()
                        .map(|fact| {
                            if self.cannot_evaluate.iter().any(|c| c == fact) {
                                format!("[{fact}] is outside this adapter's vocabulary")
                            } else {
                                format!("[{fact}] was not supplied with the request")
                            }
                        })
                        .collect();
                    if !unmet.is_empty() {
                        return Decision::indeterminate(
                            "conditions attached to the matching entry could not be settled",
                            &self.revision,
                        )
                        .checking(["entry.conditional"])
                        .with_errors(unmet);
                    }
                }
            }

            // Prohibitions last, and overriding.
            for entry in &self.entries {
                if let Entry::Refuse { who, what, on } = entry
                    && hits(who, what, on)
                {
                    return Decision::deny("a refusal entry covers this request", &self.revision)
                        .checking(["entry.refuse"]);
                }
            }

            for entry in &self.entries {
                if let Entry::Permit { who, what, on } = entry
                    && hits(who, what, on)
                {
                    return Decision::permit("a permit entry covers this request", &self.revision)
                        .checking(["entry.permit", "actor", "resource"]);
                }
            }

            Decision::indeterminate(
                "no entry in this table covers the request",
                &self.revision,
            )
            .checking(["table.scanned"])
        }
    }

    struct TableUnderTest;

    impl EvaluatorUnderTest for TableUnderTest {
        fn name(&self) -> &str {
            "TableEvaluator (replacement)"
        }

        fn build(&self, intent: &PolicyIntent) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
            let mut entries = Vec::new();
            let mut cannot_evaluate = Vec::new();
            for clause in &intent.clauses {
                match clause {
                    PolicyClause::Allow { actor, operation, resource } => {
                        entries.push(Entry::Permit {
                            who: actor.clone(),
                            what: operation.clone(),
                            on: resource.clone(),
                        })
                    }
                    PolicyClause::Forbid { actor, operation, resource } => {
                        entries.push(Entry::Refuse {
                            who: actor.clone(),
                            what: operation.clone(),
                            on: resource.clone(),
                        })
                    }
                    PolicyClause::AllowRequiringFacts { actor, operation, resource, facts } => {
                        entries.push(Entry::Conditional {
                            who: actor.clone(),
                            what: operation.clone(),
                            on: resource.clone(),
                            upon: facts.clone(),
                        })
                    }
                    PolicyClause::UnsupportedClause(name) => cannot_evaluate.push(name.clone()),
                }
            }
            Ok(Arc::new(TableEvaluator {
                revision: intent.revision.clone(),
                entries,
                cannot_evaluate,
            }))
        }
    }

    // ── the gate ──────────────────────────────────────────────────────────────────────────────

    /// **The reference evaluator meets the contract**, with nothing declined.
    #[test]
    fn the_reference_evaluator_is_conformant() {
        let report = run(&ReferenceUnderTest);
        assert!(
            report.conformant(),
            "{}\nfailures: {:#?}\ndeclined: {:#?}",
            report.summary(),
            report.failed,
            report.declined,
        );
        assert_eq!(report.passed.len(), cases().len(), "every case, not a subset");
    }

    /// **AE4's gate: a replacement evaluator passes the same contract fixtures.**
    ///
    /// `TableEvaluator` shares no type with the reference, scans prohibitions in the opposite
    /// order and words every message differently. It is held to exactly the same bar — which is
    /// only possible because the fixtures state the contract rather than describe an
    /// implementation.
    #[test]
    fn a_replacement_evaluator_passes_the_same_fixtures() {
        let report = run(&TableUnderTest);
        assert!(
            report.conformant(),
            "{}\nfailures: {:#?}\ndeclined: {:#?}",
            report.summary(),
            report.failed,
            report.declined,
        );
        assert_eq!(report.passed.len(), cases().len());
    }

    /// **Both evaluators reach the same verdict on every case.**
    ///
    /// Stronger than both passing separately: it rules out a case so loose that two evaluators
    /// could satisfy it while disagreeing about what happened.
    #[test]
    fn the_two_evaluators_agree_case_by_case() {
        for case in cases() {
            let reference = ReferenceUnderTest.build(&case.policy).expect("reference builds");
            let table = TableUnderTest.build(&case.policy).expect("table builds");
            let (a, _) = outcome_of(&reference, &case.envelope, case.now_ms);
            let (b, _) = outcome_of(&table, &case.envelope, case.now_ms);
            assert_eq!(a, b, "case {} disagrees: reference {a:?}, replacement {b:?}", case.id);
        }
    }

    // ── the suite's own self-checks: a suite that passes everything is worth nothing ───────────

    /// An evaluator that collapses *not established* into *denied* must be caught.
    ///
    /// This is the slice's central distinction, and an adapter that gets it wrong reports
    /// uncovered actions as drift. If the suite let it pass, the suite would be the defect.
    #[test]
    fn an_evaluator_that_denies_instead_of_abstaining_fails_the_suite() {
        struct AlwaysDeny(String);
        impl ActionEvaluator for AlwaysDeny {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                Decision::deny("computer says no", &self.0)
            }
        }
        struct Subject;
        impl EvaluatorUnderTest for Subject {
            fn name(&self) -> &str {
                "AlwaysDeny"
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                Ok(Arc::new(AlwaysDeny(intent.revision.clone())))
            }
        }

        let report = run(&Subject);
        assert!(!report.conformant(), "a blanket denier must not pass");
        assert!(
            report.failed.iter().any(|f| f.case == "incomplete_allow_list"),
            "and it must fail on exactly the distinction it collapsed: {:#?}",
            report.failed.iter().map(|f| f.case).collect::<Vec<_>>(),
        );
    }

    /// An evaluator that permits everything must be caught — the failure that matters most.
    #[test]
    fn a_blanket_permitter_fails_the_suite() {
        struct AlwaysPermit(String);
        impl ActionEvaluator for AlwaysPermit {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                Decision::permit("sure", &self.0).checking(["nothing at all"])
            }
        }
        struct Subject;
        impl EvaluatorUnderTest for Subject {
            fn name(&self) -> &str {
                "AlwaysPermit"
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                Ok(Arc::new(AlwaysPermit(intent.revision.clone())))
            }
        }

        let report = run(&Subject);
        assert!(!report.conformant());
        for must_refuse in ["explicit_prohibition", "missing_facts", "impersonation"] {
            assert!(
                report.failed.iter().any(|f| f.case == must_refuse),
                "{must_refuse} must fail for a blanket permitter",
            );
        }
        // The seam catches the unmapped case whatever the evaluator said, so a permitter does NOT
        // fail it. Stated rather than left to look like an oversight.
        assert!(
            report.passed.contains(&"unmapped_operation"),
            "the seam refuses an unmapped permit on its own",
        );
    }

    /// **Reaching the right outcome is not enough: the refusal must name what it could not
    /// establish.** An evaluator that abstains silently leaves an operator with a refusal and no
    /// way to act on it.
    #[test]
    fn a_silent_abstention_fails_even_though_the_verdict_is_right() {
        struct Mute(String);
        impl ActionEvaluator for Mute {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                Decision::indeterminate("no", &self.0)
            }
        }
        struct Subject;
        impl EvaluatorUnderTest for Subject {
            fn name(&self) -> &str {
                "Mute"
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                Ok(Arc::new(Mute(intent.revision.clone())))
            }
        }

        let report = run(&Subject);
        let missing_facts = report
            .failed
            .iter()
            .find(|f| f.case == "missing_facts")
            .expect("must fail for not naming the fact");
        assert!(
            missing_facts.actual.contains("without naming"),
            "the report must say it was the naming, not the verdict: {}",
            missing_facts.actual,
        );
    }

    /// **Declining is not passing, and an undeclared decline is a silent gap.**
    #[test]
    fn a_case_declined_but_not_declared_is_not_conformant() {
        struct Picky;
        impl EvaluatorUnderTest for Picky {
            fn name(&self) -> &str {
                "Picky"
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                if intent
                    .clauses
                    .iter()
                    .any(|c| matches!(c, PolicyClause::UnsupportedClause(_)))
                {
                    return Err(Unexpressible {
                        clause: "UnsupportedClause".into(),
                        because: "this adapter has no notion of an unsupported clause".into(),
                    });
                }
                ReferenceUnderTest.build(intent)
            }
        }

        let report = run(&Picky);
        assert!(report.failed.is_empty(), "it got every case it attempted right");
        assert_eq!(report.declined.len(), 1);
        assert!(
            !report.conformant(),
            "but a decline it did not declare is a gap, not a pass",
        );
    }

    /// The same evaluator, declaring its limit, is conformant — and a **stale** declaration is not.
    #[test]
    fn a_declared_limit_is_honoured_and_a_stale_one_is_refused() {
        struct Honest;
        impl EvaluatorUnderTest for Honest {
            fn name(&self) -> &str {
                "Honest"
            }
            fn declared_limits(&self) -> &[&'static str] {
                &["unsupported_clause"]
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                if intent
                    .clauses
                    .iter()
                    .any(|c| matches!(c, PolicyClause::UnsupportedClause(_)))
                {
                    return Err(Unexpressible {
                        clause: "UnsupportedClause".into(),
                        because: "this adapter has no notion of an unsupported clause".into(),
                    });
                }
                ReferenceUnderTest.build(intent)
            }
        }
        assert!(run(&Honest).conformant(), "a declared limit is honest");

        // Now the same declaration against an evaluator that can in fact handle the case: an
        // operator reading the declaration would believe in a weakness that is not there.
        struct Stale;
        impl EvaluatorUnderTest for Stale {
            fn name(&self) -> &str {
                "Stale"
            }
            fn declared_limits(&self) -> &[&'static str] {
                &["unsupported_clause"]
            }
            fn build(
                &self,
                intent: &PolicyIntent,
            ) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
                ReferenceUnderTest.build(intent)
            }
        }
        assert!(
            !run(&Stale).conformant(),
            "a declaration that no longer matches the behaviour is itself a defect",
        );
    }

    /// Every case has a distinct id, since ids are what a declared limit names.
    #[test]
    fn case_ids_are_unique() {
        let mut ids: Vec<&str> = cases().iter().map(|c| c.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "a duplicate id would make a declared limit ambiguous");
    }

    /// The permit case really does permit — otherwise every "refuses correctly" case above could
    /// be satisfied by an evaluator that refuses everything, and the suite would prove nothing.
    #[test]
    fn the_suite_is_not_satisfiable_by_refusing_everything() {
        let permit = cases().into_iter().find(|c| c.id == "permit").expect("the permit case");
        let ev = ReferenceUnderTest.build(&permit.policy).expect("builds");
        let (outcome, decision) = outcome_of(&ev, &permit.envelope, permit.now_ms);
        assert_eq!(outcome, Expected::Admitted);
        assert!(is_permit(decision.as_ref()));
        assert_eq!(decision.expect("a decision").verdict, Verdict::Permit);
    }
}
