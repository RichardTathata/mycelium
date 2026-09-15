//! Runtime authorisation at the gateway — the **evaluator seam** of the AE slice
//! (`docs/design/action-envelope-ae0.md`; plan `docs/plans/v3-contracts-axis.md` §6.8, D36–D38).
//!
//! **What this is.** One hook between the gateway's auth layer and its dispatch: before a
//! gateway-originated call reaches a provider, an [`ActionEvaluator`] sees an [`ActionEnvelope`]
//! — who is asking to do what, to which resource, with which arguments — and answers
//! [`Verdict::Permit`], [`Verdict::Deny`] or [`Verdict::Indeterminate`]. The secure profile
//! refuses on anything but `Permit`.
//!
//! **What this is not.** A gateway decision is a **route-level preflight**, not enforcement at the
//! effect (AE0 §7, posture rule 6). A process that reaches a tool without traversing this gateway
//! is outside the guarantee, and the evidence must say so (`coverage.complete: false`). Resource-side
//! enforcement inside the effect boundary is AE2, and it is a different, stronger claim. Nothing
//! here is taught to Layer I: signal propagation and KV replication are untouched.
//!
//! **Inert until attached.** With no evaluator attached ([`GossipAgent::with_action_evaluator`])
//! the seam is a no-op and the gateway behaves exactly as it did before — so this is additive for
//! every existing deployment. Attach one and the profile's refusal rules apply.
//!
//! **No second identity scheme.** The envelope carries item 1's `operation_id` / `attempt_id` and
//! item 7's verified principal ([`GatewayCaller`](super::gateway_caller::GatewayCaller)); it never
//! mints its own. A caller-supplied principal, catalogue claim or policy revision is not evidence
//! and never enters the envelope — the enforcement point assembles it from facts it verified.
//!
//! **Five-part statement.** *Guarantee:* with an evaluator attached under the secure profile, a
//! gateway-originated call whose authority cannot be established does not reach the provider
//! through this gateway. *Assumptions:* the auth layer is the only source of the actor (item 7),
//! and the evaluator is deterministic over the envelope. *Enforcing component:* [`preflight`], at
//! the gateway dispatch. *Failure behaviour:* `Deny` and `Indeterminate` both refuse; an evaluator
//! that reports errors yields `Indeterminate`, never `Permit`; a panicking one is contained
//! *only in an unwinding build* — under the release profile's `panic = "abort"` it aborts the
//! process, and the trait says so. *Detecting test:* the negative
//! fixtures in this module's `tests` (AE0 §9). *Strength:* `SelfImposedPrevention` for the routes
//! this gateway fronts — never `HardPrevention`, which only a resource fence earns.

use crate::node_id::NodeId;
use std::sync::Arc;

// ── The envelope ─────────────────────────────────────────────────────────────

/// How a business activity maps to this native operation — carried so evidence can name it, and
/// so an unmapped operation is *said to be* unmapped rather than guessed at (AE0 §6).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionMapping {
    /// The catalogue's identity, as the evidence consumer owns it. Never re-minted here.
    pub catalogue: String,
    /// The catalogue revision this mapping came from.
    pub revision: String,
    /// Whether this operation is mapped at that revision.
    pub status: MappingStatus,
}

impl ActionMapping {
    /// A mapping at `catalogue`/`revision` with an explicit status. **The constructor external
    /// adapters use** — the type is `#[non_exhaustive]` so later fields stay additive, which
    /// without this would make it unconstructible outside this crate (E0639) and the
    /// [`ActionEvaluator`] trait unimplementable. Found by review, 2026-09-15.
    pub fn new(catalogue: impl Into<String>, revision: impl Into<String>, status: MappingStatus) -> Self {
        Self { catalogue: catalogue.into(), revision: revision.into(), status }
    }

    /// Bound to exactly one catalogue action.
    pub fn mapped(catalogue: impl Into<String>, revision: impl Into<String>) -> Self {
        Self::new(catalogue, revision, MappingStatus::Mapped)
    }

    /// Not in the catalogue at this revision — a permit cannot cover it.
    pub fn unmapped(catalogue: impl Into<String>, revision: impl Into<String>) -> Self {
        Self::new(catalogue, revision, MappingStatus::Unmapped)
    }

    /// Bound to more than one action, or to one whose preconditions are not established.
    pub fn ambiguous(catalogue: impl Into<String>, revision: impl Into<String>) -> Self {
        Self::new(catalogue, revision, MappingStatus::Ambiguous)
    }
}

/// Whether an operation could be bound to a reviewed business activity.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingStatus {
    /// Bound to exactly one catalogue action.
    Mapped,
    /// Not in the catalogue at this revision. **Never** a guess from the tool's name.
    Unmapped,
    /// Bound to more than one action, or to one whose preconditions are not established.
    Ambiguous,
}

/// Who is asking to do what, to which resource, with which arguments.
///
/// **Assembled only by the enforcement point** from facts it verified: the actor comes from item
/// 7's caller context, the identities from item 1, the mapping from the reviewed catalogue. It is
/// `#[non_exhaustive]` from birth, so later fields are additive by Rust's rules.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionEnvelope {
    /// Item 1's caller-minted operation identity, stable across retries.
    pub operation_id: String,
    /// Item 1's per-attempt identity.
    pub attempt_id: String,
    /// The verified originating principal (item 7) — never a client-supplied string.
    pub actor: String,
    /// The gateway acting on the actor's behalf.
    pub via: NodeId,
    /// The authority granted for *this* request: the credential's scopes ∩ the route's requirement.
    pub scopes: Vec<String>,
    /// The native operation, e.g. `tools/call`.
    pub operation: String,
    /// The exact target, e.g. `tool:{name}@{provider node}`.
    pub resource: String,
    /// `sha256` of the canonical arguments — the binding a decision is made against, so changing
    /// the arguments after the check is a different action (AE0 §2).
    pub arguments_digest: [u8; 32],
    /// The **security-relevant argument values** the evaluator declared it needs
    /// ([`ActionEvaluator::security_relevant_arguments`]), lifted from the request by the
    /// enforcement point.
    ///
    /// A digest establishes integrity but carries no meaning: no adapter can decide
    /// *"amount ≤ 500"* or *"destination is approved"* from a hash (review, 2026-09-15). AE0 §2
    /// asks for "security-relevant arguments **or** their canonical digest"; both travel, and they
    /// answer different questions. Only the **declared names** cross into the envelope — and thence
    /// into evidence — so the rest of the payload stays out of the record (AE0 §5). A declared name
    /// that is absent from the request is absent here, and a policy that needs it must answer
    /// [`Verdict::Indeterminate`], never guess.
    pub selected_arguments: serde_json::Map<String, serde_json::Value>,
    /// The reviewed business mapping for this operation.
    pub mapping: ActionMapping,
    /// The policy revision the enforcement point expects to be evaluated, when it knows one: a
    /// decision reporting a different revision is *stale policy* and refuses (AE0 §9).
    pub expected_policy_revision: Option<String>,
    /// Gateway HLC physical time (ms) when the envelope was assembled.
    pub issued_at_ms: u64,
    /// The envelope's validity horizon (ms). A decision consumed after it is expired.
    pub not_after_ms: u64,
}

impl ActionEnvelope {
    /// Start building an envelope. The enforcement point uses this; an **external adapter needs it
    /// too**, to write tests against its own evaluator (the type is `#[non_exhaustive]`, so a
    /// struct literal will not compile outside this crate).
    ///
    /// Defaults: no scopes, a zero digest, no selected arguments, an `unmapped` mapping (which a
    /// permit cannot cover), no expected policy revision, and a validity of `issued_at` to
    /// `issued_at + 60_000` ms.
    pub fn builder(
        actor: impl Into<String>,
        via: NodeId,
        operation: impl Into<String>,
        resource: impl Into<String>,
    ) -> ActionEnvelopeBuilder {
        ActionEnvelopeBuilder {
            inner: ActionEnvelope {
                operation_id: String::new(),
                attempt_id: String::new(),
                actor: actor.into(),
                via,
                scopes: Vec::new(),
                operation: operation.into(),
                resource: resource.into(),
                arguments_digest: [0u8; 32],
                selected_arguments: serde_json::Map::new(),
                mapping: ActionMapping::unmapped("unmapped", "0"),
                expected_policy_revision: None,
                issued_at_ms: 0,
                not_after_ms: 60_000,
            },
        }
    }

    /// Has this envelope's validity elapsed as of `now_ms`?
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms > self.not_after_ms
    }
}

/// Builder for [`ActionEnvelope`] (see [`ActionEnvelope::builder`]).
#[derive(Clone, Debug)]
pub struct ActionEnvelopeBuilder {
    inner: ActionEnvelope,
}

impl ActionEnvelopeBuilder {
    /// Item 1's operation and attempt identities.
    pub fn identities(mut self, operation_id: impl Into<String>, attempt_id: impl Into<String>) -> Self {
        self.inner.operation_id = operation_id.into();
        self.inner.attempt_id = attempt_id.into();
        self
    }
    /// The authority granted for this request.
    pub fn scopes<S: Into<String>>(mut self, scopes: impl IntoIterator<Item = S>) -> Self {
        self.inner.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }
    /// The canonical argument digest.
    pub fn arguments_digest(mut self, digest: [u8; 32]) -> Self {
        self.inner.arguments_digest = digest;
        self
    }
    /// The declared security-relevant argument values.
    pub fn selected_arguments(mut self, args: serde_json::Map<String, serde_json::Value>) -> Self {
        self.inner.selected_arguments = args;
        self
    }
    /// The reviewed catalogue mapping.
    pub fn mapping(mut self, mapping: ActionMapping) -> Self {
        self.inner.mapping = mapping;
        self
    }
    /// The policy revision this enforcement point expects a decision to come from.
    pub fn expected_policy_revision(mut self, revision: impl Into<String>) -> Self {
        self.inner.expected_policy_revision = Some(revision.into());
        self
    }
    /// Issued-at and not-after, in HLC physical milliseconds.
    pub fn validity(mut self, issued_at_ms: u64, not_after_ms: u64) -> Self {
        self.inner.issued_at_ms = issued_at_ms;
        self.inner.not_after_ms = not_after_ms;
        self
    }
    /// Finish.
    pub fn build(self) -> ActionEnvelope {
        self.inner
    }
}

// ── The decision ─────────────────────────────────────────────────────────────

/// An evaluator's answer. **`Indeterminate` is never `Permit`** (AE0 §3).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The policy establishes authority for this exact action.
    Permit,
    /// The policy establishes that this action is refused — an explicit prohibition, or an
    /// allowance the action falls outside of.
    Deny,
    /// Authority could **not be established**: a fact the policy needs is missing, a clause is not
    /// supported, the policy could not be evaluated, or the operation is unmapped. Distinct from
    /// `Deny`: nothing says the action is forbidden, only that nothing says it is allowed.
    Indeterminate,
}

/// What an evaluator decided, and what it actually checked.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The verdict.
    pub verdict: Verdict,
    /// The constraints the policy actually tested, for the evidence record.
    pub checked: Vec<String>,
    /// Human-legible reason. Never the policy text itself.
    pub reason: String,
    /// The revision of the policy that produced this decision.
    pub policy_revision: String,
    /// Policy-evaluation failures and unrecognised clauses. A non-empty list with a `Permit`
    /// verdict is a contradiction the seam resolves to `Indeterminate`.
    pub errors: Vec<String>,
}

impl Decision {
    /// A decision with an explicit verdict. **The constructor external adapters use**: `Decision`
    /// is `#[non_exhaustive]`, so without these an adapter in another crate could not return one
    /// from [`ActionEvaluator::evaluate`] at all (E0639) — it could only reach
    /// [`Decision::indeterminate`], making every foreign evaluator permanently undecided. The
    /// replaceable-evaluator premise depended on a constructor that did not exist until review
    /// found it, 2026-09-15.
    pub fn new(verdict: Verdict, reason: impl Into<String>, policy_revision: impl Into<String>) -> Self {
        Self {
            verdict,
            checked: Vec::new(),
            reason: reason.into(),
            policy_revision: policy_revision.into(),
            errors: Vec::new(),
        }
    }

    /// Authority established for this exact action.
    pub fn permit(reason: impl Into<String>, policy_revision: impl Into<String>) -> Self {
        Self::new(Verdict::Permit, reason, policy_revision)
    }

    /// The policy establishes that this action is refused.
    pub fn deny(reason: impl Into<String>, policy_revision: impl Into<String>) -> Self {
        Self::new(Verdict::Deny, reason, policy_revision)
    }

    /// A decision that establishes nothing, with a reason. The shape every failure takes.
    pub fn indeterminate(reason: impl Into<String>, policy_revision: impl Into<String>) -> Self {
        Self::new(Verdict::Indeterminate, reason, policy_revision)
    }

    /// Record the constraints the policy actually tested.
    pub fn checking<S: Into<String>>(mut self, checked: impl IntoIterator<Item = S>) -> Self {
        self.checked = checked.into_iter().map(Into::into).collect();
        self
    }

    /// Record policy-evaluation failures and unrecognised clauses.
    pub fn with_errors<S: Into<String>>(mut self, errors: impl IntoIterator<Item = S>) -> Self {
        self.errors = errors.into_iter().map(Into::into).collect();
        self
    }
}

/// A replaceable policy evaluator.
///
/// Implementations must be **deterministic** over the envelope: no clock, no network, no ambient
/// state. Time enters as the envelope's validity; facts enter as its fields. The same envelope and
/// the same policy revision must produce the same decision, so a decision can be replayed
/// (item 6) and an evidence record can be re-checked.
pub trait ActionEvaluator: Send + Sync + 'static {
    /// Decide.
    ///
    /// **Must not panic.** Report an evaluation failure as [`Verdict::Indeterminate`] with the
    /// error in [`Decision::errors`]; that is a decision the seam can refuse on and the evidence
    /// can carry. The seam wraps this call in [`std::panic::catch_unwind`], but that contains a
    /// panic **only in an unwinding build**: this crate's release profile sets `panic = "abort"`,
    /// where a panicking evaluator terminates the process and no containment is possible
    /// (review, 2026-09-15 — the first cut promised containment unconditionally, which was wrong).
    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision;

    /// The **argument names** whose values this evaluator needs in order to decide — for a
    /// condition such as *amount ≤ 500* or *destination is approved*, which no digest can answer.
    ///
    /// The enforcement point lifts exactly these from the request into
    /// [`ActionEnvelope::selected_arguments`]; nothing else crosses, so the rest of the payload
    /// stays out of the envelope and out of the evidence record (AE0 §5). Default: none, which
    /// suits a policy that decides on actor, operation and resource alone.
    fn security_relevant_arguments(&self) -> Vec<String> {
        Vec::new()
    }

    /// How this operation binds to a reviewed business activity, at the catalogue revision this
    /// evaluator was configured with.
    ///
    /// **The default is fail-closed**: [`MappingStatus::Unmapped`], which a permit cannot cover
    /// (AE0 §6 — a generic tool's business purpose cannot be inferred from its name). An evaluator
    /// that carries a reviewed catalogue overrides this; one that does not says so honestly, and
    /// its actions read as unmapped to the evidence consumer rather than as business operations.
    fn mapping(&self, _operation: &str, _resource: &str) -> ActionMapping {
        ActionMapping {
            catalogue: "unmapped".into(),
            revision: "0".into(),
            status: MappingStatus::Unmapped,
        }
    }
}

// ── The preflight ────────────────────────────────────────────────────────────

/// Why the preflight refused. Every variant refuses; none falls through to dispatch.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreflightRefusal {
    /// The policy establishes that the action is refused.
    Denied(Decision),
    /// Authority could not be established (missing facts, unsupported clause, evaluation error,
    /// unmapped operation, stale policy, expired envelope).
    NotEstablished(Decision),
}

impl PreflightRefusal {
    /// The decision behind the refusal.
    pub fn decision(&self) -> &Decision {
        match self {
            PreflightRefusal::Denied(d) | PreflightRefusal::NotEstablished(d) => d,
        }
    }

    /// Short machine-readable reason for JSON bodies, metrics and evidence.
    pub fn reason(&self) -> &'static str {
        match self {
            PreflightRefusal::Denied(_) => "action_denied",
            PreflightRefusal::NotEstablished(_) => "authority_not_established",
        }
    }

    /// JSON-RPC error code for the MCP and A2A surfaces.
    pub fn json_rpc_code(&self) -> i32 {
        match self {
            PreflightRefusal::Denied(_) => -32030,
            PreflightRefusal::NotEstablished(_) => -32031,
        }
    }
}

impl std::fmt::Display for PreflightRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = self.decision();
        match self {
            PreflightRefusal::Denied(_) => write!(f, "action denied by policy {}: {}", d.policy_revision, d.reason),
            PreflightRefusal::NotEstablished(_) => {
                write!(f, "authority not established under policy {}: {}", d.policy_revision, d.reason)
            }
        }
    }
}

/// Ask the evaluator for the facts the enforcement point needs *before* it can build the envelope
/// — the declared argument names and the catalogue mapping — behind the same unwind boundary as
/// [`preflight`], so an adapter that panics here is contained wherever containment is possible.
/// `None` means the adapter panicked and nothing it said can be used.
///
/// Returns `(security_relevant_argument_names, mapping)`.
pub fn evaluator_facts(
    evaluator: &Arc<dyn ActionEvaluator>,
    operation: &str,
    resource: &str,
) -> Option<(Vec<String>, ActionMapping)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (evaluator.security_relevant_arguments(), evaluator.mapping(operation, resource))
    }))
    .ok()
}

/// Run the evaluator over `envelope` and turn its decision into an admit-or-refuse.
///
/// `Ok(Some(decision))` — admitted, with the decision for the evidence record. `Ok(None)` — no
/// evaluator is attached, so the seam is inert. `Err(_)` — refused.
///
/// The seam, not the evaluator, enforces the invariants an adapter might get wrong:
/// a `Permit` carrying evaluation errors is downgraded to `Indeterminate`; a decision whose
/// `policy_revision` is not the one the envelope expected is stale policy; an expired envelope is
/// denied; an unmapped or ambiguous operation cannot be permitted; and a panicking evaluator is
/// caught rather than trusted.
pub fn preflight(
    evaluator: Option<&Arc<dyn ActionEvaluator>>,
    envelope: &ActionEnvelope,
    now_ms: u64,
) -> Result<Option<Decision>, PreflightRefusal> {
    let Some(evaluator) = evaluator else { return Ok(None) };

    // Expiry is the seam's own check: an evaluator is deterministic and holds no clock.
    if envelope.is_expired(now_ms) {
        return Err(PreflightRefusal::Denied(Decision {
            verdict: Verdict::Deny,
            checked: vec!["validity".into()],
            reason: format!("envelope expired at {} (now {now_ms})", envelope.not_after_ms),
            policy_revision: envelope.expected_policy_revision.clone().unwrap_or_default(),
            errors: Vec::new(),
        }));
    }

    // A panicking adapter must not become an admission — **in an unwinding build**. Under the
    // release profile's `panic = "abort"` no containment is possible and the process terminates;
    // the trait documents that an evaluator must not panic, and this is defence in depth for the
    // builds where it works (review, 2026-09-15). `AssertUnwindSafe` is sound here: the evaluator
    // is `&dyn` behind an `Arc` and the envelope is borrowed immutably, so no observable state can
    // be left inconsistent by an unwind.
    let decision = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| evaluator.evaluate(envelope))) {
        Ok(d) => d,
        Err(_) => {
            return Err(PreflightRefusal::NotEstablished(Decision::indeterminate(
                "evaluator panicked",
                envelope.expected_policy_revision.clone().unwrap_or_default(),
            )));
        }
    };

    // Stale policy: the decision did not come from the revision the enforcement point expected.
    // The **effective** verdict becomes `Indeterminate` — a refusal carrying `Permit` would be a
    // contradiction in the evidence record, which reads the decision, not the control flow
    // (review, 2026-09-15). The original verdict is preserved in `checked` so nothing is lost.
    if let Some(expected) = &envelope.expected_policy_revision
        && *expected != decision.policy_revision
    {
        let reason = format!(
            "decision came from policy {} but this enforcement point expects {expected}",
            decision.policy_revision
        );
        let mut checked = decision.checked.clone();
        checked.push(format!("superseded verdict {:?} from policy {}", decision.verdict, decision.policy_revision));
        return Err(PreflightRefusal::NotEstablished(Decision {
            verdict: Verdict::Indeterminate,
            reason,
            checked,
            ..decision
        }));
    }

    match decision.verdict {
        // A permit that reports evaluation errors, or that covers an operation the catalogue does
        // not bind, establishes nothing: the seam downgrades rather than trusts.
        Verdict::Permit if !decision.errors.is_empty() => {
            let reason = format!("evaluation errors under a permit verdict: {}", decision.errors.join("; "));
            Err(PreflightRefusal::NotEstablished(Decision { verdict: Verdict::Indeterminate, reason, ..decision }))
        }
        Verdict::Permit if envelope.mapping.status != MappingStatus::Mapped => {
            let reason = format!(
                "operation is {:?} at catalogue {} revision {} — a permit cannot name it as a business operation",
                envelope.mapping.status, envelope.mapping.catalogue, envelope.mapping.revision
            );
            Err(PreflightRefusal::NotEstablished(Decision { verdict: Verdict::Indeterminate, reason, ..decision }))
        }
        Verdict::Permit => Ok(Some(decision)),
        Verdict::Deny => Err(PreflightRefusal::Denied(decision)),
        Verdict::Indeterminate => Err(PreflightRefusal::NotEstablished(decision)),
    }
}

// ── The reference evaluator ──────────────────────────────────────────────────

/// A deterministic reference evaluator: the contract's executable meaning, and what the negative
/// fixtures run against. It is **not** a policy language — the one real adapter is Cedar,
/// in-process, and it lives in the private companion (AE0 §3, D37).
///
/// Rules, in order: an explicit prohibition denies; otherwise an allowance for
/// `(actor, operation, resource)` permits; otherwise **`Indeterminate`** — an incomplete
/// allow-list is *authority not established*, never a denial, so evidence never reads it as drift
/// (AE0 §9). A clause naming a fact this evaluator cannot establish (a mandate, a budget) is
/// reported as an error and yields `Indeterminate`.
#[derive(Clone, Debug, Default)]
pub struct ReferenceEvaluator {
    revision: String,
    catalogue: Option<(String, String)>,
    mapped: Vec<(String, String)>,
    prohibitions: Vec<Rule>,
    allowances: Vec<Rule>,
    /// Clause names this evaluator does not implement; matching a rule that requires one yields
    /// `Indeterminate` with the clause reported (the *unsupported clause* fixture).
    unsupported: Vec<String>,
}

/// One rule of the reference evaluator: exact matches, `*` for any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Actor principal, or `*`.
    pub actor: String,
    /// Operation, or `*`.
    pub operation: String,
    /// Resource, or `*`.
    pub resource: String,
    /// Scopes the actor must hold for this rule to apply.
    pub requires_scopes: Vec<String>,
    /// Facts the rule needs that this evaluator cannot establish (e.g. `mandate`, `budget`).
    pub requires_facts: Vec<String>,
    /// Argument values the rule tests, by name — the reference evaluator compares for equality;
    /// a real adapter (Cedar) evaluates richer predicates over the same
    /// [`ActionEnvelope::selected_arguments`]. A named value the request did not carry is *not
    /// established*, never a silent mismatch.
    pub requires_values: Vec<(String, serde_json::Value)>,
}

impl Rule {
    /// A rule matching `actor` for `operation` on `resource`; `*` is any.
    pub fn new(actor: impl Into<String>, operation: impl Into<String>, resource: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            operation: operation.into(),
            resource: resource.into(),
            requires_scopes: Vec::new(),
            requires_facts: Vec::new(),
            requires_values: Vec::new(),
        }
    }

    /// Require these scopes for the rule to apply.
    pub fn requiring_scopes<S: Into<String>>(mut self, scopes: impl IntoIterator<Item = S>) -> Self {
        self.requires_scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Require facts this evaluator cannot establish — the rule then yields `Indeterminate`.
    pub fn requiring_facts<S: Into<String>>(mut self, facts: impl IntoIterator<Item = S>) -> Self {
        self.requires_facts = facts.into_iter().map(Into::into).collect();
        self
    }

    /// Require a named argument to equal `value`. Absent from the request ⇒ *not established*.
    pub fn requiring_value(mut self, name: impl Into<String>, value: serde_json::Value) -> Self {
        self.requires_values.push((name.into(), value));
        self
    }

    fn matches(&self, env: &ActionEnvelope) -> bool {
        let any = |pat: &str, v: &str| pat == "*" || pat == v;
        any(&self.actor, &env.actor)
            && any(&self.operation, &env.operation)
            && any(&self.resource, &env.resource)
            && self.requires_scopes.iter().all(|s| env.scopes.iter().any(|h| h == s))
            // A declared value that is present but different means this rule is not the one; a
            // value that is *absent* still matches here, so `unestablished` can report it as a
            // missing fact rather than the rule silently falling through.
            && self.requires_values.iter().all(|(n, v)| {
                env.selected_arguments.get(n).map(|have| have == v).unwrap_or(true)
            })
    }
}

impl ReferenceEvaluator {
    /// An evaluator whose decisions report `revision`.
    pub fn new(revision: impl Into<String>) -> Self {
        Self { revision: revision.into(), ..Default::default() }
    }

    /// Add an allowance.
    pub fn allow(mut self, rule: Rule) -> Self {
        self.allowances.push(rule);
        self
    }

    /// Add a prohibition. Prohibitions are checked first and always win.
    pub fn prohibit(mut self, rule: Rule) -> Self {
        self.prohibitions.push(rule);
        self
    }

    /// Declare a clause name unsupported: a matching rule requiring it yields `Indeterminate`.
    pub fn unsupported_clause(mut self, clause: impl Into<String>) -> Self {
        self.unsupported.push(clause.into());
        self
    }

    /// Carry a reviewed catalogue's identity and revision — the evidence consumer's own object,
    /// never re-minted here (AE0 §6, handover seam 1).
    pub fn with_catalogue(mut self, catalogue: impl Into<String>, revision: impl Into<String>) -> Self {
        self.catalogue = Some((catalogue.into(), revision.into()));
        self
    }

    /// Bind `(operation, resource)` to a catalogue action. Anything not bound stays *unmapped*.
    pub fn map_action(mut self, operation: impl Into<String>, resource: impl Into<String>) -> Self {
        self.mapped.push((operation.into(), resource.into()));
        self
    }
}

impl ActionEvaluator for ReferenceEvaluator {
    fn mapping(&self, operation: &str, resource: &str) -> ActionMapping {
        let (catalogue, revision) = self
            .catalogue
            .clone()
            .unwrap_or_else(|| ("unmapped".to_string(), "0".to_string()));
        let status = if self.mapped.iter().any(|(o, r)| o == operation && (r == resource || r == "*")) {
            MappingStatus::Mapped
        } else {
            MappingStatus::Unmapped
        };
        ActionMapping { catalogue, revision, status }
    }

    fn evaluate(&self, env: &ActionEnvelope) -> Decision {
        let rev = self.revision.clone();

        // A rule whose own preconditions cannot be established decides nothing — **including a
        // prohibition**. The first cut applied this to allowances only, so a prohibition needing an
        // absent mandate returned a definite `Deny`: dispatch was still refused, but the evidence
        // claimed a prohibition had been *established* when it had not (review, 2026-09-15).
        let unestablished = |r: &Rule| -> Vec<String> {
            let mut missing: Vec<String> = r
                .requires_facts
                .iter()
                .map(|f| {
                    if self.unsupported.contains(f) {
                        format!("unsupported clause: {f}")
                    } else {
                        format!("fact not established: {f}")
                    }
                })
                .collect();
            // A declared value the request did not carry is equally unestablished.
            for (name, _) in &r.requires_values {
                if !env.selected_arguments.contains_key(name) {
                    missing.push(format!("argument value not available: {name}"));
                }
            }
            missing
        };

        if let Some(p) = self.prohibitions.iter().find(|r| r.matches(env)) {
            let missing = unestablished(p);
            if !missing.is_empty() {
                return Decision {
                    verdict: Verdict::Indeterminate,
                    checked: vec![format!("prohibition actor={} operation={}", p.actor, p.operation)],
                    reason: "a prohibition matches but needs facts this evaluator cannot establish".into(),
                    policy_revision: rev,
                    errors: missing,
                };
            }
            return Decision {
                verdict: Verdict::Deny,
                checked: vec![format!("prohibition actor={} operation={} resource={}", p.actor, p.operation, p.resource)],
                reason: "an explicit prohibition matches this action".into(),
                policy_revision: rev,
                errors: Vec::new(),
            };
        }

        if let Some(a) = self.allowances.iter().find(|r| r.matches(env)) {
            let missing = unestablished(a);
            if !missing.is_empty() {
                return Decision {
                    verdict: Verdict::Indeterminate,
                    checked: vec![format!("allowance actor={} operation={}", a.actor, a.operation)],
                    reason: "the matching allowance needs facts this evaluator cannot establish".into(),
                    policy_revision: rev,
                    errors: missing,
                };
            }
            return Decision {
                verdict: Verdict::Permit,
                checked: vec![
                    format!("allowance actor={} operation={} resource={}", a.actor, a.operation, a.resource),
                    format!("scopes {:?}", a.requires_scopes),
                    format!("values {:?}", a.requires_values.iter().map(|(n, _)| n).collect::<Vec<_>>()),
                ],
                reason: "an allowance matches this action".into(),
                policy_revision: rev,
                errors: Vec::new(),
            };
        }

        // Nothing matched. Authority is *not established* — never a denial (AE0 §9).
        Decision::indeterminate(
            "no allowance matches this action; the allow-list does not cover it",
            rev,
        )
    }
}

// ── Assembling an envelope at the gateway ────────────────────────────────────

/// `sha256` of the canonical arguments — the binding a decision is made against.
///
/// Available only where `sha2` is (the `tls` feature, which the AE profile needs anyway for
/// attestation). There is deliberately **no non-cryptographic fallback**: a digest that does not
/// bind is worse than an absent one, because a decision would appear bound while an argument
/// substitution went undetected. A build without `tls` simply has no gateway preflight.
#[cfg(feature = "tls")]
pub(crate) fn arguments_digest(canonical: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(canonical).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeId {
        NodeId::new("127.0.0.1", 9000).unwrap()
    }

    /// The tests' stand-in for the gateway's digest, so the fixtures run in every build; the
    /// gateway itself uses [`arguments_digest`] (sha256, `tls` only).
    fn digest_of(bytes: &[u8]) -> [u8; 32] {
        #[cfg(feature = "tls")]
        { arguments_digest(bytes) }
        #[cfg(not(feature = "tls"))]
        {
            let mut out = [0u8; 32];
            for (i, b) in bytes.iter().enumerate() { out[i % 32] ^= *b; }
            out
        }
    }

    fn envelope(actor: &str, operation: &str, resource: &str) -> ActionEnvelope {
        // Built through the public builder — the same path an external adapter's tests take.
        ActionEnvelope::builder(actor, node(), operation, resource)
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .arguments_digest(digest_of(b"{\"n\":1}"))
            .mapping(ActionMapping::mapped("cat-procurement", "7"))
            .expected_policy_revision("rev-1")
            .validity(1_000, 61_000)
            .build()
    }

    fn evaluator(e: ReferenceEvaluator) -> Arc<dyn ActionEvaluator> {
        Arc::new(e)
    }

    /// The seam is inert with no evaluator attached — every existing deployment is unaffected.
    #[test]
    fn no_evaluator_is_a_no_op() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        assert_eq!(preflight(None, &env, 2_000), Ok(None));
    }

    /// The happy path: an allowance permits, and the decision reaches the caller for the record.
    #[test]
    fn an_allowance_permits_and_reports_what_it_checked() {
        let ev = evaluator(ReferenceEvaluator::new("rev-1").allow(
            Rule::new("oidc:idp/alice", "tools/call", "tool:square@n1").requiring_scopes(["mcp:invoke"]),
        ));
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let d = preflight(Some(&ev), &env, 2_000).expect("permitted").expect("a decision");
        assert_eq!(d.verdict, Verdict::Permit);
        assert!(!d.checked.is_empty(), "a permit says what it checked");
        assert_eq!(d.policy_revision, "rev-1");
    }

    /// AE0 §9 — **explicit prohibition**: denied, and denial is not "authority not established".
    #[test]
    fn explicit_prohibition_denies() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "*", "*"))
                .prohibit(Rule::new("oidc:idp/mallory", "tools/call", "*")),
        );
        let env = envelope("oidc:idp/mallory", "tools/call", "tool:square@n1");
        let r = preflight(Some(&ev), &env, 2_000).expect_err("denied");
        assert!(matches!(r, PreflightRefusal::Denied(_)));
        assert_eq!(r.reason(), "action_denied");
        assert_eq!(r.decision().verdict, Verdict::Deny);
    }

    /// AE0 §9 — **incomplete allow-list**: *authority not established*, never `Deny`. Evidence
    /// must not read an uncovered action as drift.
    #[test]
    fn incomplete_allow_list_is_not_established_never_denied() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1").allow(Rule::new("oidc:idp/alice", "tools/call", "tool:square@n1")),
        );
        let env = envelope("oidc:idp/alice", "tools/call", "tool:cube@n1"); // a resource nobody named
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert!(matches!(r, PreflightRefusal::NotEstablished(_)));
        assert_eq!(r.reason(), "authority_not_established");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate, "indeterminate is never a denial");
    }

    /// AE0 §9 — **missing facts** and **unsupported clause**: both `Indeterminate`, both refuse,
    /// and each reports what it could not establish.
    #[test]
    fn missing_facts_and_unsupported_clauses_refuse() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "tools/call", "*").requiring_facts(["mandate", "odrl:duty"]))
                .unsupported_clause("odrl:duty"),
        );
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        let d = r.decision();
        assert_eq!(d.verdict, Verdict::Indeterminate);
        assert!(d.errors.iter().any(|e| e.contains("fact not established: mandate")), "{:?}", d.errors);
        assert!(d.errors.iter().any(|e| e.contains("unsupported clause: odrl:duty")), "{:?}", d.errors);
    }

    /// AE0 §9 — **unmapped operation**: a permit cannot name an unmapped action as a business
    /// operation, so the seam downgrades it.
    #[test]
    fn a_permit_cannot_cover_an_unmapped_operation() {
        let ev = evaluator(ReferenceEvaluator::new("rev-1").allow(Rule::new("*", "*", "*")));
        let mut env = envelope("oidc:idp/alice", "tools/call", "tool:execute_shell@n1");
        env.mapping.status = MappingStatus::Unmapped;
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate);
        assert!(r.decision().reason.contains("Unmapped"), "{}", r.decision().reason);
        // Ambiguous is refused on the same ground.
        env.mapping.status = MappingStatus::Ambiguous;
        assert!(preflight(Some(&ev), &env, 2_000).is_err());
    }

    /// AE0 §9 — **stale policy**: a decision from a revision this enforcement point does not
    /// expect establishes nothing.
    #[test]
    fn a_decision_from_an_unexpected_policy_revision_is_stale() {
        let ev = evaluator(ReferenceEvaluator::new("rev-2").allow(Rule::new("*", "*", "*")));
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1"); // expects rev-1
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert!(matches!(r, PreflightRefusal::NotEstablished(_)));
        assert!(r.decision().reason.contains("rev-2") && r.decision().reason.contains("rev-1"));
    }

    /// AE0 §9 — **expired validity**: denied by the seam, which owns the clock the evaluator does
    /// not have.
    #[test]
    fn an_expired_envelope_is_denied_before_the_evaluator_runs() {
        struct Boom;
        impl ActionEvaluator for Boom {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                panic!("must not be reached for an expired envelope");
            }
        }
        let ev: Arc<dyn ActionEvaluator> = Arc::new(Boom);
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1"); // not_after 61_000
        let r = preflight(Some(&ev), &env, 61_001).expect_err("denied");
        assert!(matches!(r, PreflightRefusal::Denied(_)));
        assert!(r.decision().checked.iter().any(|c| c == "validity"));
    }

    /// AE0 §9 — **substitution**: a decision is bound to the argument digest, so changed arguments
    /// are a different envelope. (The gateway recomputes the digest itself; a client cannot supply it.)
    #[test]
    fn changed_arguments_change_the_binding() {
        let a = digest_of(b"{\"amount\":10}");
        let b = digest_of(b"{\"amount\":1000000}");
        assert_ne!(a, b, "the decision's binding must change with the arguments");
        let mut env1 = envelope("oidc:idp/alice", "tools/call", "tool:pay@n1");
        env1.arguments_digest = a;
        let mut env2 = env1.clone();
        env2.arguments_digest = b;
        assert_ne!(env1, env2);
    }

    /// An evaluation error under a `Permit` is a contradiction: the seam refuses rather than trust
    /// a permit its own adapter could not fully evaluate.
    #[test]
    fn a_permit_carrying_errors_is_downgraded() {
        struct Sloppy;
        impl ActionEvaluator for Sloppy {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                Decision {
                    verdict: Verdict::Permit,
                    checked: vec![],
                    reason: "allowed".into(),
                    policy_revision: "rev-1".into(),
                    errors: vec!["clause 3 failed to evaluate".into()],
                }
            }
        }
        let ev: Arc<dyn ActionEvaluator> = Arc::new(Sloppy);
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate);
    }

    /// A panicking adapter must never become an admission.
    #[test]
    fn a_panicking_evaluator_refuses() {
        struct Boom;
        impl ActionEvaluator for Boom {
            fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                panic!("adapter bug");
            }
        }
        let ev: Arc<dyn ActionEvaluator> = Arc::new(Boom);
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the test output quiet
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        std::panic::set_hook(prev);
        assert_eq!(r.decision().verdict, Verdict::Indeterminate);
        assert!(r.decision().reason.contains("panicked"));
    }

    /// The evaluator is deterministic: the same envelope decides the same way, every time.
    #[test]
    fn the_reference_evaluator_is_deterministic() {
        let ev = ReferenceEvaluator::new("rev-1")
            .allow(Rule::new("oidc:idp/alice", "tools/call", "*").requiring_scopes(["mcp:invoke"]));
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let first = ev.evaluate(&env);
        for _ in 0..16 {
            assert_eq!(ev.evaluate(&env), first);
        }
    }

    /// Review regression (2026-09-15, finding 1): a prohibition whose own preconditions cannot be
    /// established decides **nothing**. Dispatch is still refused, but the evidence must not claim
    /// a prohibition was established when the fact it rests on was unavailable.
    #[test]
    fn a_prohibition_needing_an_unavailable_fact_is_indeterminate_not_deny() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "*", "*"))
                .prohibit(Rule::new("*", "tools/call", "*").requiring_facts(["mandate"])),
        );
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused either way");
        let d = r.decision();
        assert_eq!(d.verdict, Verdict::Indeterminate, "a prohibition on an unestablished fact is not a denial");
        assert_eq!(r.reason(), "authority_not_established");
        assert!(d.errors.iter().any(|e| e.contains("fact not established: mandate")), "{:?}", d.errors);
    }

    /// Review regression (2026-09-15, finding 2): a refusal must not carry `Permit` in its
    /// decision. The evidence consumer reads the decision, not the control flow.
    #[test]
    fn a_stale_decision_is_indeterminate_not_a_permit_inside_a_refusal() {
        let ev = evaluator(ReferenceEvaluator::new("rev-2").allow(Rule::new("*", "*", "*")));
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1"); // expects rev-1
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate, "no Permit may survive inside a refusal");
        assert!(
            r.decision().checked.iter().any(|c| c.contains("superseded verdict Permit")),
            "the original verdict is preserved, not erased: {:?}", r.decision().checked
        );
    }

    /// Review point (2026-09-15): a digest establishes integrity but carries no meaning, so the
    /// envelope also carries the argument **values** the policy declared it needs.
    #[test]
    fn declared_argument_values_travel_and_absent_ones_are_not_established() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "tools/call", "*").requiring_value("currency", serde_json::json!("GBP"))),
        );
        // Present and equal ⇒ permitted.
        let mut args = serde_json::Map::new();
        args.insert("currency".into(), serde_json::json!("GBP"));
        let env = ActionEnvelope::builder("oidc:idp/alice", node(), "tools/call", "tool:pay@n1")
            .scopes(["mcp:invoke"])
            .mapping(ActionMapping::mapped("cat", "1"))
            .selected_arguments(args.clone())
            .validity(0, 60_000)
            .build();
        assert_eq!(preflight(Some(&ev), &env, 1).unwrap().unwrap().verdict, Verdict::Permit);

        // Present and different ⇒ the rule is not the one; nothing else covers it ⇒ not established.
        let mut other = serde_json::Map::new();
        other.insert("currency".into(), serde_json::json!("USD"));
        let env_usd = ActionEnvelope::builder("oidc:idp/alice", node(), "tools/call", "tool:pay@n1")
            .scopes(["mcp:invoke"]).mapping(ActionMapping::mapped("cat", "1"))
            .selected_arguments(other).validity(0, 60_000).build();
        let r = preflight(Some(&ev), &env_usd, 1).expect_err("refused");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate);

        // Absent entirely ⇒ *not established*, and said so by name — never a silent mismatch.
        let env_none = ActionEnvelope::builder("oidc:idp/alice", node(), "tools/call", "tool:pay@n1")
            .scopes(["mcp:invoke"]).mapping(ActionMapping::mapped("cat", "1"))
            .validity(0, 60_000).build();
        let r = preflight(Some(&ev), &env_none, 1).expect_err("refused");
        assert!(
            r.decision().errors.iter().any(|e| e.contains("argument value not available: currency")),
            "{:?}", r.decision().errors
        );
    }

    /// Scopes are part of the match: an actor without the route's authority is not covered by an
    /// allowance that requires it, and that is *not established*, not a denial.
    #[test]
    fn an_allowance_requiring_scopes_does_not_cover_an_actor_without_them() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "tools/call", "*").requiring_scopes(["admin"])),
        );
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1"); // holds mcp:invoke only
        let r = preflight(Some(&ev), &env, 2_000).expect_err("refused");
        assert_eq!(r.decision().verdict, Verdict::Indeterminate);
    }
}
