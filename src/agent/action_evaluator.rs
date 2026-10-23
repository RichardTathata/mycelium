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

use crate::mandate::{MandateRefusal, PrincipalId, TermId};
use super::route_facts::{DataClass, Destination, Locality, RouteFacts};
use mycelium_core::receipt::LocalDurability;
use crate::node_id::NodeId;
use serde::{Deserialize, Serialize};
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
    /// The **scoped mandate** this action claims to act under, and what the enforcement point
    /// established about it (item 5 — AE1).
    ///
    /// `None` means the action claims no mandate, which is not the same as claiming one that
    /// failed: see [`MandateState`]. Assembled by the enforcement point from a mandate it read and
    /// checked itself — a caller-supplied epoch is a claim, never authority.
    pub mandate: Option<MandateBinding>,
    /// Where the action would go, when a routing enforcement point established it (post-360 row R):
    /// the destination's name and [`Locality`]. `None` from a door that does not route — which a rule
    /// about locality reads as *not established*, never as in-domain.
    pub destination: Option<Destination>,
    /// The class of the data the request carries — declared, never detected — and who declared it
    /// (post-360 row R). `None` from a door that does not classify, which a rule about class reads
    /// as *not established*.
    pub data_class: Option<DataClass>,
    /// The run this action belongs to, carried from `params._meta.correlation` and minted by no record
    /// (adopter-surface X4; [`crate::agent::causal`]). `None`: the caller named no run.
    pub correlation: Option<String>,
    /// The content hash of the record this action depends on, from `params._meta.parent` (X4). `None`: a root.
    pub parent: Option<String>,
}

/// The mandate an action claims, and the enforcement point's finding about it (AE1).
///
/// The identity travels beside the finding because evidence has to answer *which appointment* was
/// relied on, not merely whether something passed. `term` is which appointment; `epoch` orders
/// authority; they are deliberately separate (see [`crate::mandate`]).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MandateBinding {
    /// Who holds the mandate.
    pub holder: PrincipalId,
    /// **Which** appointment — not the same thing as the epoch.
    pub term: TermId,
    /// What it covers.
    pub scope: String,
    /// The epoch presented. Orders authority; monotonic per scope.
    pub epoch: u64,
    /// What the enforcement point established.
    pub state: MandateState,
}

impl MandateBinding {
    /// A binding the enforcement point checked and found good.
    pub fn established(
        holder: PrincipalId,
        term: TermId,
        scope: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self { holder, term, scope: scope.into(), epoch, state: MandateState::Established }
    }

    /// A binding the enforcement point checked and the fence refused.
    pub fn refused(
        holder: PrincipalId,
        term: TermId,
        scope: impl Into<String>,
        epoch: u64,
        refusal: MandateRefusal,
    ) -> Self {
        Self { holder, term, scope: scope.into(), epoch, state: MandateState::Refused(refusal) }
    }

    /// A binding the enforcement point could **not** check — authority not established.
    pub fn unknown(
        holder: PrincipalId,
        term: TermId,
        scope: impl Into<String>,
        epoch: u64,
        why: impl Into<String>,
    ) -> Self {
        Self { holder, term, scope: scope.into(), epoch, state: MandateState::Unknown(why.into()) }
    }
}

/// What the enforcement point established about a claimed mandate.
///
/// The three are kept apart because they are three different answers and conflating any two loses
/// the difference an operator needs: *it holds*, *it was refused and here is why*, and *we could
/// not find out*. The third is *not* the second — AE0 §9's rule that absence of a fact is never a
/// denial applies here exactly as it does to an incomplete allow-list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MandateState {
    /// Checked against the resource's installed epoch and found good.
    Established,
    /// Checked and refused, carrying item 5's own refusal rather than a second vocabulary.
    ///
    /// **This is a fence, not a policy input.** A refused mandate denies the action whatever the
    /// policy says, because a policy that could permit it would launder the refusal — which is the
    /// same reason [`MandateRefusal::Superseded`] is not a `Conflict`.
    Refused(MandateRefusal),
    /// The fence could not be consulted. Authority is **not established**, which is not a denial.
    Unknown(String),
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
                mandate: None,
                destination: None,
                data_class: None,
                correlation: None,
                parent: None,
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
    /// The causal fields the request carried (X4): the run's `correlation` and the `parent` record's hash.
    pub fn causal(mut self, fields: super::causal::CausalFields) -> Self {
        self.inner.correlation = fields.correlation;
        self.inner.parent = fields.parent;
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
    /// Bind the **scoped mandate** this action claims, with what the enforcement point
    /// established about it (AE1). Never call this with a caller-supplied epoch: the point of the
    /// binding is that the enforcement point read the mandate and checked it.
    pub fn mandate(mut self, binding: MandateBinding) -> Self {
        self.inner.mandate = Some(binding);
        self
    }

    /// Where the action would go (post-360 row R) — set by a routing enforcement point from the
    /// destination it established, never from the request.
    pub fn destination(mut self, destination: Destination) -> Self {
        self.inner.destination = Some(destination);
        self
    }

    /// The class of the request's data and who declared it (post-360 row R).
    pub fn data_class(mut self, class: DataClass) -> Self {
        self.inner.data_class = Some(class);
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

    /// The history this evaluator needs to decide (post-360 row K), or `None` — the default — for a policy
    /// that decides on the request alone. Asked before the envelope is built, behind the same unwind
    /// boundary as the other facts; the enforcement point then reads the actor's records for the window.
    ///
    /// **An evaluator that wraps another forwards this and [`evaluate_with_history`](Self::evaluate_with_history).**
    /// A wrapper written before them hides the inner evaluator's request, so every limited rule behind it
    /// decides `Indeterminate` — closed, but not what was meant.
    fn history_request(&self) -> Option<HistoryRequest> {
        None
    }

    /// Decide with the actor's history beside the envelope. **What the seam calls.**
    ///
    /// The default ignores it and calls [`evaluate`](Self::evaluate), so an evaluator that decides on the
    /// request alone is unchanged. One that needs history overrides this and decides
    /// [`Verdict::Indeterminate`] on [`History::Unavailable`] — a history that cannot be read is not an
    /// empty one — and names the [`HistoryScope`] in what it reports.
    fn evaluate_with_history(&self, envelope: &ActionEnvelope, history: &History) -> Decision {
        let _ = history;
        self.evaluate(envelope)
    }

    /// The **fleet-wide counters** a decision on `envelope` would rely on (row K2): for each allowance limited
    /// at a fleet scope ([`HistoryScope::FleetConsensus`], [`HistoryScope::FleetTally`]) that would decide this
    /// envelope, its counter. The default — none — is right for every policy that counts only at the enforcement
    /// point. Asked after the envelope is built, behind the same unwind boundary as the other facts.
    ///
    /// The enforcement point reads each counter's state into [`HistoryView::fleet`] before the evaluator decides
    /// and, on a permit, **claims a seat** in every [`HistoryScope::FleetConsensus`] counter named here before the
    /// work is admitted — a claim that fails turns the permit into a denial (the counter is full) or a decision
    /// not established (no quorum). So an evaluator names here the counters of the rule that would permit, and
    /// only those: a counter named for a rule that does not decide consumes a seat for nothing.
    ///
    /// **An evaluator that wraps another forwards this too**, as it forwards
    /// [`history_request`](Self::history_request).
    fn fleet_counters(&self, envelope: &ActionEnvelope) -> Vec<FleetCounter> {
        let _ = envelope;
        Vec::new()
    }
}

// ── History (post-360 row K) ─────────────────────────────────────────────────

/// **Whose** history a count covers. Every history decision names it, because a limit means nothing until
/// it is said what was counted.
///
/// Decided for row K (plan §5): by default a count is what **this enforcement point** at this node saw — its own
/// evidence journal, node-local and never gossiped. The point is the gateway (all its doors, `/mcp`, `/a2a`, the
/// LLM and federation routes, count together) or the provider check, each counted apart, so a call the gateway
/// decided and the provider re-checks on one node is counted once at each, never twice at one. Exact there and
/// needing no quorum; an actor that spreads its calls over N gateways can be admitted up to N times a per-gateway
/// limit.
///
/// Decided for row K2 (plan §5, 2026-10-17): a rule may instead name one of **two fleet scopes**, the policy
/// choosing per rule — [`FleetConsensus`](Self::FleetConsensus), exact and unavailable without a quorum, and
/// [`FleetTally`](Self::FleetTally), approximate and available without one. A rule naming one scope is never
/// satisfied by a count at another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HistoryScope {
    /// The decisions this enforcement point recorded in its own evidence journal.
    EnforcementPoint,
    /// **Exact, fleet-wide** (row K2): a counter of `count` seats agreed by consensus. Each permit claims a seat by
    /// a consensus write — leased for the rule's window — *before* the work is admitted, so the fleet admits at most
    /// `count` in any window whichever gateways the calls arrive at. Without a quorum a claim cannot be made and the
    /// rule decides `Indeterminate`. One consensus round per limited call.
    FleetConsensus,
    /// **Approximate, fleet-wide** (row K2): every enforcement point publishes its counts per actor and rule, and a
    /// limit reads the fleet's sum over the publishers it has heard from recently. Available without a quorum.
    /// **Detection, not prevention:** concurrent calls at different gateways can exceed the limit before the tallies
    /// converge, and every decision at this scope says so and names its coverage. Coverage that cannot be
    /// established — a publisher gone stale, this node's own view of the fleet not established — is not established,
    /// never an empty tally.
    FleetTally,
}

impl HistoryScope {
    /// The scope's name, as decisions and evidence state it.
    pub fn label(&self) -> &'static str {
        match self {
            HistoryScope::EnforcementPoint => "enforcement_point",
            HistoryScope::FleetConsensus => "fleet_consensus",
            HistoryScope::FleetTally => "fleet_tally",
        }
    }

    /// Whether this scope counts across the fleet rather than at one enforcement point.
    pub fn is_fleet(&self) -> bool {
        matches!(self, HistoryScope::FleetConsensus | HistoryScope::FleetTally)
    }
}

/// The sentence every decision at [`HistoryScope::FleetTally`] carries — the scope's limit stated where it is relied
/// on, not only in its documentation.
pub const FLEET_TALLY_CAVEAT: &str = "approximate: detection, not prevention — concurrent calls at different \
     gateways can exceed the limit before the tallies converge";

/// One fleet-wide counter a decision relies on (row K2): which scope counts it, the counter's identity, and its limit.
///
/// `key` names the counter — the rule's identity, not the actor's: the enforcement point counts per **actor and
/// key**, so one rule limits each actor separately. It is digested before it reaches a consensus slot or a tally key,
/// so any string is safe here; two rules that should share a count share a key. The reference evaluator derives it
/// from the rule's actor, operation and resource patterns ([`Rule::counter_key`]) — so changing a rule's count or
/// window keeps its counter, and changing what it matches starts a new one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FleetCounter {
    /// Which fleet scope counts it.
    pub scope: HistoryScope,
    /// The counter's identity.
    pub key: String,
    /// How many permits the window may hold, the one being decided included.
    pub count: u32,
    /// How far back, in milliseconds.
    pub window_ms: u64,
}

/// **The most seats a fleet counter may hold** — the largest `count` a fleet-scoped limit may name (row K2's bounds).
///
/// The enforcement point reads every seat of a [`HistoryScope::FleetConsensus`] counter — a slot digest and a KV read
/// each — on every request the rule covers, synchronously under the actor's history gate on an async worker, and a
/// claim may propose to each in turn; a [`HistoryScope::FleetTally`] entry keeps up to `count` permit times. So the
/// count is a cost per request, and an unbounded one is a denial of service by misconfiguration (a limit of
/// `u32::MAX` pinned a worker for minutes). 1,024 matches what the evidence journal keeps per actor at a point
/// (`PER_ACTOR`) and what the private Cedar adapter accepts; a policy wanting more in a window wants a per-point limit
/// or a shorter window. A counter past it is **refused, not clamped** — the enforcement point reads none of it and a
/// permit relying on it is not established ([`FleetCounter::check_bounds`]).
pub const MAX_FLEET_SEATS: u32 = 1_024;

/// **The longest window a fleet-scoped limit may name**, in milliseconds: the history horizon (24 hours), the same
/// bound the enforcement point's own history keeps an actor's records for. A seat is leased for its window, and a tally
/// entry kept for it, so a window past the horizon would hold fleet-visible state longer than anything else the history
/// keeps; a window of zero counts nothing. A counter outside `1..=MAX_FLEET_WINDOW_MS` is refused, not clamped.
pub const MAX_FLEET_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;

/// Why a [`FleetCounter`] is outside the bounds every enforcement point holds a fleet-scoped limit to
/// ([`MAX_FLEET_SEATS`], [`MAX_FLEET_WINDOW_MS`]). Its `Display` names the bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FleetBoundsError {
    /// The limit names more seats than [`MAX_FLEET_SEATS`].
    TooManySeats {
        /// The limit the counter names.
        count: u32,
    },
    /// The window is zero: it counts nothing.
    EmptyWindow,
    /// The window is longer than the history horizon, [`MAX_FLEET_WINDOW_MS`].
    PastHorizon {
        /// The window the counter names, in milliseconds.
        window_ms: u64,
    },
}

impl std::fmt::Display for FleetBoundsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManySeats { count } => {
                write!(f, "a limit of {count} exceeds the {MAX_FLEET_SEATS} seats a fleet counter may hold")
            }
            Self::EmptyWindow => write!(f, "a window of 0 ms counts nothing"),
            Self::PastHorizon { window_ms } => write!(
                f,
                "a window of {window_ms} ms exceeds the history horizon of {MAX_FLEET_WINDOW_MS} ms"
            ),
        }
    }
}

impl std::error::Error for FleetBoundsError {}

/// Whether a fleet-scoped limit of `count` in `window_ms` is within the bounds ([`MAX_FLEET_SEATS`],
/// [`MAX_FLEET_WINDOW_MS`]).
pub fn check_fleet_bounds(count: u32, window_ms: u64) -> Result<(), FleetBoundsError> {
    if count > MAX_FLEET_SEATS {
        return Err(FleetBoundsError::TooManySeats { count });
    }
    if window_ms == 0 {
        return Err(FleetBoundsError::EmptyWindow);
    }
    if window_ms > MAX_FLEET_WINDOW_MS {
        return Err(FleetBoundsError::PastHorizon { window_ms });
    }
    Ok(())
}

impl FleetCounter {
    /// A counter at `scope`, named `key`, permitting `count` in `window_ms`.
    ///
    /// Infallible, so a counter outside the bounds can be built — and is then **refused wherever it is used**: the
    /// enforcement point reads none of its seats or tallies and claims no seat ([`FleetState::NotEstablished`]), and
    /// the seam turns a permit relying on it into *not established* ([`preflight_with_history`]). Use
    /// [`try_new`](Self::try_new) to learn at construction.
    pub fn new(scope: HistoryScope, key: impl Into<String>, count: u32, window_ms: u64) -> Self {
        Self { scope, key: key.into(), count, window_ms }
    }

    /// [`new`](Self::new), refusing a counter outside the bounds ([`MAX_FLEET_SEATS`], [`MAX_FLEET_WINDOW_MS`]).
    pub fn try_new(scope: HistoryScope, key: impl Into<String>, count: u32, window_ms: u64) -> Result<Self, FleetBoundsError> {
        check_fleet_bounds(count, window_ms)?;
        Ok(Self::new(scope, key, count, window_ms))
    }

    /// Whether this counter is within the bounds every enforcement point holds it to.
    pub fn check_bounds(&self) -> Result<(), FleetBoundsError> {
        check_fleet_bounds(self.count, self.window_ms)
    }
}

/// What the enforcement point established about one fleet counter before the evaluator decided (row K2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FleetState {
    /// [`HistoryScope::FleetConsensus`]: how many of the counter's seats this node sees held by a live decision. A
    /// count at or above the limit is a decided fact (each seat is a decision); one below it is a hint — the seat
    /// this permit needs is claimed by consensus afterwards, and the claim is what admits it.
    Seats {
        /// Seats held, as this node's replica of the decisions shows them.
        held: u32,
    },
    /// [`HistoryScope::FleetTally`]: the fleet's sum over the publishers heard, and who they were.
    Tally {
        /// Permits in the window, summed over `publishers`.
        counted: u32,
        /// The enforcement points whose tallies were summed — this node and every peer whose heartbeat is fresh.
        publishers: Vec<String>,
        /// The oldest heartbeat summed, in milliseconds before the decision.
        stalest_ms: u64,
    },
    /// The counter's state could not be established, and why. Never read as zero.
    NotEstablished(String),
}

/// One fleet counter and its state, as an evaluator is given it in [`HistoryView::fleet`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FleetCount {
    /// The counter.
    pub counter: FleetCounter,
    /// What was established about it.
    pub state: FleetState,
}

impl FleetCount {
    /// `counter` in `state`.
    pub fn new(counter: FleetCounter, state: FleetState) -> Self {
        Self { counter, state }
    }
}

/// A seat a permit claimed in a [`HistoryScope::FleetConsensus`] counter (row K2), carried on its evidence record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SeatClaim {
    /// The counter's key, as the evaluator named it.
    pub counter: String,
    /// The consensus slot the seat was decided in.
    pub slot: String,
    /// Which seat, `0..count`.
    pub seat: u32,
    /// The decision's fencing token.
    pub token: u64,
    /// When the seat's lease ends, on the proposer's wall clock (epoch ms).
    pub expires_at_ms: u64,
}

impl SeatClaim {
    /// A claim of `seat` in `slot` for `counter`.
    pub fn new(counter: impl Into<String>, slot: impl Into<String>, seat: u32, token: u64, expires_at_ms: u64) -> Self {
        Self { counter: counter.into(), slot: slot.into(), seat, token, expires_at_ms }
    }
}

/// What history an evaluator needs to decide: how far back, and at most how many of the actor's records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    /// How far back from the decision, in milliseconds.
    pub window_ms: u64,
    /// The most recent records it needs at most. A view holding fewer than the window had says so
    /// ([`HistoryView::complete`]).
    pub max_entries: usize,
}

/// One earlier decision about the same actor, as the evidence journal recorded it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Its event time, epoch milliseconds (the record's `at_ms`).
    pub at_ms: u64,
    /// The attempt it decided — the key a replay finds it by in the journal.
    pub attempt_id: String,
    /// The operation.
    pub operation: String,
    /// The resource, as the enforcement point resolved it.
    pub resource: String,
    /// What was decided.
    pub decision: DecisionKind,
    /// What this point last recorded about its execution.
    pub execution: Execution,
}

/// An actor's recent history, bounded and read-only — what [`ActionEvaluator::evaluate_with_history`] decides on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryView {
    /// Whose history this is.
    pub scope: HistoryScope,
    /// The actor it is about — the envelope's.
    pub actor: String,
    /// Which enforcement point at this node counted it: `gateway`, or the provider check (`provider`).
    pub point: String,
    /// The enforcement point's clock when it was read. The evaluator holds no clock; windows start here.
    pub as_of_ms: u64,
    /// How far back it reaches.
    pub window_ms: u64,
    /// The actor's records in the window, oldest first.
    pub entries: Vec<HistoryEntry>,
    /// Whether these are **all** of the actor's records in the window. `false` when the view was capped or
    /// the point no longer holds some of them: a count below a limit is then not established.
    pub complete: bool,
    /// The newest event time of a record this view does not hold — dropped by the point's bounds, lost to a
    /// journal repair or a lenient append, or cut by the request's cap. A rule over a window that starts at or
    /// after it sees all of its records whatever `complete` says of the widest window; one reaching back past
    /// it does not. `None`: nothing is missing.
    pub omitted_through_ms: Option<u64>,
    /// The fleet counters the evaluator named ([`ActionEvaluator::fleet_counters`]) and what the enforcement point
    /// established about each (row K2). Empty for a policy that counts only at the enforcement point.
    pub fleet: Vec<FleetCount>,
}

impl HistoryView {
    /// Whether this view holds every record in the `window_ms` ending at `as_of_ms` — for a rule whose window
    /// is not the view's.
    pub fn complete_for(&self, window_ms: u64) -> bool {
        window_ms <= self.window_ms && self.omitted_through_ms.is_none_or(|t| t <= self.as_of_ms.saturating_sub(window_ms))
    }
}

/// The history an enforcement point gives an evaluator: a view, or why none could be established —
/// **never** an empty view standing in for one it could not read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum History {
    /// What was read.
    View(HistoryView),
    /// No history could be established, and why: no evidence journal, a journal that could not be read,
    /// or a decision made without one.
    Unavailable(String),
}

/// What a decision read of its actor's history, carried on the evidence record — the history view as a
/// **decision input**: the scope, the window, and the attempts it was given, by which a replay finds them in
/// the journal and decides again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryInput {
    /// Whose history; absent when none could be established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HistoryScope>,
    /// How far back it reached.
    pub window_ms: u64,
    /// The clock it was read at.
    pub as_of_ms: u64,
    /// The attempts it held, oldest first.
    pub attempts: Vec<String>,
    /// Whether that was all of them in the window.
    pub complete: bool,
    /// Why no history could be established, when none could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// The fleet counters the decision read, with their scope, state and coverage (row K2). Absent for a policy that
    /// counts only at the enforcement point, so such a record serialises exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fleet: Vec<FleetCount>,
    /// The seats a permit claimed in [`HistoryScope::FleetConsensus`] counters before it was admitted (row K2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seats: Vec<SeatClaim>,
}

impl HistoryInput {
    /// The input `history` makes to a decision.
    pub fn of(history: &History) -> Self {
        match history {
            History::View(v) => HistoryInput {
                // A decision on a fleet counter names the fleet scope it counted at, not the point whose view
                // carried it (row K2, review finding 7).
                scope: Some(v.fleet.first().map_or(v.scope, |c| c.counter.scope)),
                window_ms: v.window_ms,
                as_of_ms: v.as_of_ms,
                attempts: v.entries.iter().map(|e| e.attempt_id.clone()).collect(),
                complete: v.complete,
                unavailable: None,
                fleet: v.fleet.clone(),
                seats: Vec::new(),
            },
            History::Unavailable(why) => HistoryInput {
                scope: None,
                window_ms: 0,
                as_of_ms: 0,
                attempts: Vec::new(),
                complete: false,
                unavailable: Some(why.clone()),
                fleet: Vec::new(),
                seats: Vec::new(),
            },
        }
    }

    /// This input, with the seats its permit claimed (row K2).
    pub fn with_seats(mut self, seats: Vec<SeatClaim>) -> Self {
        self.seats = seats;
        self
    }
}

// ── The evidence record ──────────────────────────────────────────────────────

/// The schema every AE evidence document carries in an audit record's `detail`.
///
/// A consumer matches on this exactly. An audit record without it is not an authorisation
/// decision and must not be read as one — the chain carries every kind of event, and translating
/// one of the others into a decision would be inventing authority that nothing granted.
pub const AE_EVIDENCE_SCHEMA: &str = "mycelium.ae/evidence/1";

/// The verdict, in the evidence document's own vocabulary.
///
/// Deliberately a separate type from [`Verdict`]: this one is serialised into a document other
/// systems parse, so it is a wire contract and may not drift when the seam's enum gains a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Authority established for this exact action.
    Permit,
    /// An authority decided no.
    Deny,
    /// Authority not established. Never read as a permit, never shown as a prohibition.
    Indeterminate,
}

impl From<Verdict> for DecisionKind {
    fn from(v: Verdict) -> Self {
        match v {
            Verdict::Permit => DecisionKind::Permit,
            Verdict::Deny => DecisionKind::Deny,
            // A variant we do not know is not a permit and not a prohibition: it is something we
            // cannot state, which is exactly indeterminate.
            _ => DecisionKind::Indeterminate,
        }
    }
}

impl DecisionKind {
    /// The stable label, **identical to the serialised form**, for metrics.
    ///
    /// One vocabulary: a reader correlating `mycelium_ae_decisions_total{verdict=…}` with an
    /// evidence document must not have to translate. Pinned by a test, because renaming one is a
    /// dashboard break for every operator *and* a wire change for every consumer.
    pub fn label(self) -> &'static str {
        match self {
            DecisionKind::Permit => "permit",
            DecisionKind::Deny => "deny",
            DecisionKind::Indeterminate => "indeterminate",
        }
    }
}

/// Whether the operation was bound to a reviewed business activity, on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingKind {
    /// Bound to exactly one catalogue action.
    Mapped,
    /// Not in the catalogue at this revision. Never a guess from the tool's name.
    Unmapped,
    /// Bound to more than one action, or to one whose preconditions are not established.
    Ambiguous,
}

impl From<MappingStatus> for MappingKind {
    fn from(s: MappingStatus) -> Self {
        match s {
            MappingStatus::Mapped => MappingKind::Mapped,
            MappingStatus::Ambiguous => MappingKind::Ambiguous,
            // Unknown to us => not bound to a reviewed activity => not a business operation.
            _ => MappingKind::Unmapped,
        }
    }
}

impl MappingKind {
    /// The stable label, identical to the serialised form. See [`DecisionKind::label`].
    pub fn label(self) -> &'static str {
        match self {
            MappingKind::Mapped => "mapped",
            MappingKind::Unmapped => "unmapped",
            MappingKind::Ambiguous => "ambiguous",
        }
    }
}

/// What became of the action after the decision.
///
/// `Unknown` is a first-class answer, not a gap, and [`Execution::None`] is different again:
/// *nothing ran, and this point can say so*. A refusal is `None`; a permitted dispatch whose
/// result this gateway did not watch is `Attempted`, which is the honest answer, because the
/// gateway hands the call to a provider and does not observe what the provider then does.
///
/// **`#[non_exhaustive]` for the same reason as [`RecordKind`], and more urgently.** This is the
/// vocabulary AE0 §5 expects to grow first: `not_dispatched` is proposed to the evidence consumer
/// as its contract-1.2 addition (handover seam 2). Marking it before that lands makes the addition
/// additive. A downstream exhaustive `match` needs a `_` arm — and that arm must fail **safe**: an
/// execution this reader does not recognise is *we did not look*, never *nothing ran*.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    /// Dispatched; the outcome was not observed here.
    Attempted,
    /// Ran to completion, observed by this point.
    Completed,
    /// Ran and failed, observed by this point.
    Failed,
    /// Not observed.
    Unknown,
    /// Refused before dispatch: nothing ran, and this point can say so.
    None,
}

/// Which of AE0 §5's records this is.
///
/// One dispatch produces more than one: what was *decided*, and then what became of it. Keeping
/// them as separate records rather than mutating one is the point — evidence is append-only, and a
/// record that could be revised in place could be revised after someone read it.
///
/// **`#[non_exhaustive]`, and the two variants here are not the whole list.** AE0 §5 names six
/// records — *requested*, *decided*, *blocked*, *execution attempted*, *execution
/// completed/failed/unknown*, *outcome observed*. Two are shipped because two are what AE-T needed,
/// and the rest arrive when the evidence consumer's contract can receive them (`not_dispatched` is
/// proposed as its contract-1.2 addition, handover seam 2). Minting them here first would put a
/// distinction in our records that nothing downstream could read.
///
/// So the growth is *expected*, and the attribute is here before it happens rather than after: one
/// announced break instead of a series of unannounced ones, which is the §6.6 ledger's own
/// reasoning for the config structs. A downstream exhaustive `match` needs a `_` arm.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// The evaluator's verdict for this attempt. Establishes nothing about execution.
    #[default]
    Decided,
    /// What became of the dispatch this enforcement point made. Item 1's receipt for the same
    /// `operation_id` / `attempt_id` — a reuse, not a parallel ledger.
    Execution,
}

/// One authorisation decision, as sealed into the audit chain's `detail`.
///
/// **Why this exists beside the audit record's own fields.** An [`AuditRecord`](crate::AuditRecord)
/// carries a principal, an action, a target and a *three-valued* outcome. An authorisation decision
/// has a verdict, a policy revision, the constraints actually checked, whether the action then ran,
/// and which reviewed activity it maps to. Rounding those into `Success | Denied | Error` loses the
/// distinction between *prohibited* and *not established* — precisely the distinction this slice
/// exists to protect. So the precise document travels here and the coarse fields stay a summary;
/// a reader takes the document, never the summary.
///
/// **What never enters it:** arguments, argument values and policy text. What is evidenced is what
/// the policy *checked*, and the digest binding the decision to one exact payload.
///
/// `#[non_exhaustive]`: an exporter turns this into a record another organisation parses, and a
/// field added later must not break it. Construct with [`for_decision`](Self::for_decision).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AeEvidence {
    /// Always [`AE_EVIDENCE_SCHEMA`].
    pub schema: String,
    /// Which §5 record this is. Defaults to [`RecordKind::Decided`] so a record written before this
    /// field existed still parses as what it was.
    #[serde(default)]
    pub kind: RecordKind,
    /// **Event time** — when the enforcement point assembled the envelope for this attempt, in
    /// epoch milliseconds, from the node's HLC.
    ///
    /// The time of the *decision*, not of anything downstream. An exporter needs it for the
    /// consumer's `at`, and needs it to be **stable**: record ids are derived from journal
    /// position, so an exporter that re-reads after losing its cursor re-sends the same ids — and
    /// a body that differed (because it had stamped its own read time) would be refused under the
    /// consumer's *same id, byte-identical content* rule. A timestamp the record does not carry is
    /// one the exporter has to invent, and an invented one cannot be stable. Found by the
    /// exporter's own retry test, 2026-09-16.
    ///
    /// `#[serde(default)]` so a record written before this field existed still parses; it reads as
    /// `0`, which is visibly not a time rather than quietly a plausible one.
    #[serde(default)]
    pub at_ms: u64,
    /// The logical agent — the verified principal, never a client-supplied string.
    pub subject: String,
    /// The native operation asked for.
    pub operation: String,
    /// The resource it was asked of.
    pub resource: String,
    /// Where the effect would land, when this point knows. `None` means *cannot be stated*.
    pub destination: Option<String>,
    /// The identity that executed. `None` means a shared principal, so attribution is to the
    /// group rather than to the agent.
    pub execution_identity: Option<String>,
    /// What the evaluator decided.
    pub decision: DecisionKind,
    /// What became of the action.
    pub execution: Execution,
    /// The revision of the policy that decided.
    pub policy_revision: String,
    /// The digest of the deployed policy export, when one was established.
    pub policy_digest: Option<String>,
    /// The route that enforced.
    pub enforcement_point: String,
    /// The reviewed catalogue's identity, as its owner minted it. Never re-minted here.
    pub catalogue: String,
    /// That catalogue's revision.
    pub catalogue_revision: String,
    /// Whether this operation is mapped at that revision.
    pub mapping_status: MappingKind,
    /// **The rung the effect's receipt named** — item 1's own [`LocalDurability`], carried rather
    /// than re-derived (`docs/design/composed-effect.md`).
    ///
    /// This is the join the Phase-C audit's composition finding asked for. A receipt knows how
    /// durable a write was and is then *returned and gone*; this record knows who asked and
    /// survives. Neither half could state the axis' headline claim alone.
    ///
    /// `None` means the effect's durability was never established here — which is not
    /// `Failed`, and is read as *not stated* rather than *not durable*.
    ///
    /// **Not to be confused with [`AeReference::evidence`]**, which is an [`EvidenceState`] about
    /// whether *this record* reached disk. Two durabilities, one name apart: this one is the
    /// effect's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_durability: Option<LocalDurability>,
    /// The domain the call originated in, when it crossed a federated edge (item 2).
    ///
    /// A fact on the record rather than an inference from `subject`'s spelling: a principal that
    /// *looks* federated is a string, and the axis' own rule is that unverified caller-supplied
    /// identity never becomes authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_domain: Option<String>,
    /// The **content hash of the record this one corrects**, lowercase hex, when it is a
    /// correction (AE0 §5's correction/supersession reference).
    ///
    /// A correction **supersedes; it never edits**. The reason is the one that makes evidence
    /// append-only at all: a record that could be revised in place could be revised *after someone
    /// read it*. So an observation that turns out wrong is corrected by a new record citing the old
    /// one's hash, and the old record stays exactly as it was — a reader can see both, and see
    /// which replaced which.
    ///
    /// **The hash, not a name.** A correction that pointed at "the last execution record for this
    /// attempt" would be ambiguous the moment there were two, which is precisely when a correction
    /// exists. The hash is the same one [`AeReference::journal_sha256`] cites, so the two agree by
    /// construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The caller's correlation identity, stable across retries.
    pub operation_id: String,
    /// This dispatch's identity.
    pub attempt_id: String,
    /// Lowercase hex of the canonical argument digest — binds this decision to one exact payload
    /// without carrying the payload.
    pub arguments_digest: String,
    /// The constraints the policy actually tested.
    pub checked: Vec<String>,
    /// The evaluator's human-legible reason. Never policy text.
    pub reason: String,
    /// The actor's history this decision read, when it read one (post-360 row K) — scope, window and the
    /// attempts it was given, or why none could be established. Absent for a decision made on the request
    /// alone, so a record without history serialises exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryInput>,
    /// What a routing enforcement point established for this decision (post-360 row R): the request's
    /// data class and who declared it, the destination decided on with its locality, and the
    /// destinations this call had already been refused for. Absent from a door that does not route,
    /// so a record without it serialises exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteFacts>,
    /// The run this record belongs to — the envelope's `correlation`, carried, never minted (adopter-surface X4).
    /// `mycelium explain --correlation <id>` renders every record that carries one id, across journals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<String>,
    /// The content hash of the record this action depends on — the envelope's `parent` (X4); the same hash
    /// [`AeReference::journal_sha256`] cites. Absent on a root. `mycelium explain --closure <hash>` follows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

impl AeEvidence {
    /// This record, carrying what a routing enforcement point established (post-360 row R). The
    /// destination's name also fills [`destination`](Self::destination), which until now no door
    /// could state.
    pub fn with_route(mut self, route: RouteFacts) -> Self {
        self.destination = Some(route.destination.name.clone());
        self.route = Some(route);
        self
    }

    /// This record, carrying the history its decision read.
    pub fn with_history(mut self, history: HistoryInput) -> Self {
        self.history = Some(history);
        self
    }

    /// The evidence for a **destination's** refusal of a composed effect
    /// (`docs/design/composed-effect.md` §9): a `Decided` record at enforcement point
    /// `destination`, verdict `Deny`, execution `None` — nothing ran there, and the resource can
    /// say so. `leg` is the composition leg the destination refused (`attribution` or
    /// `authority`) and `reason` its own words, so a refusal after a gateway permit is as
    /// reconstructable as a commit: the journal holds the gateway's decision and the resource's,
    /// and a reader sees which point said no.
    pub fn for_destination_refusal(
        envelope: &ActionEnvelope,
        leg: &str,
        reason: impl Into<String>,
    ) -> Self {
        let mut decision = Decision::deny(reason, envelope.expected_policy_revision.clone().unwrap_or_default());
        decision.checked = vec![format!("composition:{leg}")];
        Self::for_decision(envelope, &decision, Execution::None, "destination")
    }

    /// The evidence for one decision over one envelope.
    ///
    /// `execution` is the enforcement point's own observation, because only it knows whether it
    /// dispatched: a refusal is [`Execution::None`], a permitted dispatch this point did not watch
    /// is [`Execution::Attempted`].
    pub fn for_decision(
        envelope: &ActionEnvelope,
        decision: &Decision,
        execution: Execution,
        enforcement_point: impl Into<String>,
    ) -> Self {
        Self {
            schema: AE_EVIDENCE_SCHEMA.to_string(),
            kind: RecordKind::Decided,
            // The envelope's own assembly time, carried rather than discarded — see `at_ms`.
            at_ms: envelope.issued_at_ms,
            subject: envelope.actor.clone(),
            operation: envelope.operation.clone(),
            resource: envelope.resource.clone(),
            destination: None,
            execution_identity: Some(envelope.actor.clone()),
            decision: decision.verdict.into(),
            execution,
            policy_revision: decision.policy_revision.clone(),
            policy_digest: None,
            enforcement_point: enforcement_point.into(),
            catalogue: envelope.mapping.catalogue.clone(),
            catalogue_revision: envelope.mapping.revision.clone(),
            mapping_status: envelope.mapping.status.into(),
            // Both are set by the enforcement point after the effect, through the builders below:
            // neither is knowable when the decision is recorded.
            effect_durability: None,
            origin_domain: None,
            // A record is an original until something says otherwise.
            supersedes: None,
            operation_id: envelope.operation_id.clone(),
            attempt_id: envelope.attempt_id.clone(),
            arguments_digest: hex32(&envelope.arguments_digest),
            checked: decision.checked.clone(),
            reason: decision.reason.clone(),
            // Set by the enforcement point when the decision read a history (`with_history`).
            history: None,
            // Set by a routing enforcement point (`with_route`).
            route: None,
            // Carried from the envelope, which read them from the request's own payload (X4).
            correlation: envelope.correlation.clone(),
            parent: envelope.parent.clone(),
        }
    }

    /// The **execution** record for this same attempt.
    ///
    /// Carries the decision's identities unchanged — `operation_id`, `attempt_id`, the principal,
    /// the policy revision — because it is a statement about the same attempt, and correlating them
    /// is the consumer's whole job. What it does **not** do is revise the decision: that record
    /// stands exactly as written, and this one is appended beside it.
    ///
    /// `at_ms` is carried too: both records are about **one attempt**, and an exporter that merges
    /// them into a single observation needs one event time for it — the attempt's.
    pub fn as_execution(&self, execution: Execution) -> Self {
        // The history was an input to the decision, which stands on its own record; it is not repeated here.
        Self { kind: RecordKind::Execution, execution, history: None, ..self.clone() }
    }

    /// The coarse audit outcome this decision summarises to.
    ///
    /// `Indeterminate` becoming `Error` is the least-bad of three coarse values, and is exactly why
    /// [`AeEvidence::decision`] is carried separately: a reader taking the summary alone would see
    /// a failure where the fact is *unknown*.
    #[cfg(feature = "compliance")]
    pub fn audit_outcome(&self) -> crate::AuditOutcome {
        match self.decision {
            DecisionKind::Permit => crate::AuditOutcome::Success,
            DecisionKind::Deny => crate::AuditOutcome::Denied,
            DecisionKind::Indeterminate => crate::AuditOutcome::Error,
        }
    }

    /// Parse an AE document out of an audit record's `detail`.
    ///
    /// `None` for every record that is not one of ours — absent detail, non-JSON detail, or a
    /// different `schema`. A foreign record is skipped, never coerced.
    pub fn from_detail(detail: Option<&str>) -> Option<Self> {
        let parsed: Self = serde_json::from_str(detail?).ok()?;
        (parsed.schema == AE_EVIDENCE_SCHEMA).then_some(parsed)
    }
}

/// Whether the evidence for a decision was durably established, as the chain records it.
///
/// Carried so a reference record is never mistaken for proof that the evidence behind it exists.
/// Under [`EvidenceProfile::Lenient`](super::evidence_journal::EvidenceProfile::Lenient) a dispatch
/// may proceed without durable evidence, and this is where it says so — the alternative, a chain
/// record that looks identical either way, would make the weaker profile invisible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    /// The journal acknowledged the record as fsynced.
    OnDisk,
    /// The journal did not acknowledge in time. The record may exist; nobody knows.
    Unknown,
    /// The journal reported a failure, or its queue was full and the record was not written.
    NotEstablished,
    /// No journal is attached, so no evidence was produced at all.
    NotConfigured,
}

impl AeEvidence {
    /// Carry the rung the effect's receipt named.
    pub fn with_effect_durability(mut self, durability: LocalDurability) -> Self {
        self.effect_durability = Some(durability);
        self
    }

    /// Carry the domain the call originated in, when it crossed a federated edge.
    pub fn with_origin_domain(mut self, domain: impl Into<String>) -> Self {
        self.origin_domain = Some(domain.into());
        self
    }

    /// Mark this record as correcting the record whose content hash is `journal_sha256`.
    ///
    /// The corrected record is **retained**, not replaced: this is a citation, and the journal is
    /// append-only.
    pub fn correcting(mut self, journal_sha256: impl Into<String>) -> Self {
        self.supersedes = Some(journal_sha256.into());
        self
    }

    /// Does this record correct another?
    pub fn is_correction(&self) -> bool {
        self.supersedes.is_some()
    }

    /// Does this record describe **the same action** as `other`?
    ///
    /// A correction of an *observation* means the outcome was misread — not that the action was
    /// something else. If the subject, the operation, the resource or the identities move, this is
    /// a different action wearing a citation, and a reader that accepted it would let one attempt's
    /// evidence be replaced by another's.
    ///
    /// Checked here rather than assumed, because the citation is the only thing binding the two
    /// records together and a citation can be wrong or forged.
    pub fn corrects_the_same_action_as(&self, other: &Self) -> bool {
        self.subject == other.subject
            && self.operation == other.operation
            && self.resource == other.resource
            && self.operation_id == other.operation_id
            && self.attempt_id == other.attempt_id
    }

    /// Does this one record state the axis' composed claim — **a durable, attributed, cross-domain
    /// effect**?
    ///
    /// All four legs, from this record alone. The predicate exists so the claim is *checked* rather
    /// than assembled by eye at each reading, which is how the composition went unproved: every leg
    /// worked and nothing ever asked for them together.
    ///
    /// **`OnDisk` only.** [`LocalDurability::Buffered`] survives a process crash and is lost to a
    /// power failure — a real rung, and not this one. Item 1 added `Buffered` precisely to stop a
    /// receipt claiming a durability the node never established, and a composed claim resting on it
    /// would re-make that mistake one level up.
    ///
    /// **Completion only.** `Attempted` is the honest answer when a dispatcher did not watch, and
    /// it is not an effect that happened.
    ///
    /// What this does **not** establish: that the attribution is correct. Item 7 decides that, at
    /// its own strength; this carries that verdict rather than re-deciding it
    /// (`docs/design/composed-effect.md` §7).
    pub fn states_a_composed_effect(&self) -> bool {
        matches!(self.effect_durability, Some(LocalDurability::OnDisk))
            && self.origin_domain.is_some()
            && !self.subject.is_empty()
            && self.execution == Execution::Completed
    }
}

/// The **safe reference record**: all that may enter the gossiped audit chain (AE0 §5).
///
/// # The rule, and why it is a rule
///
/// The audit chain is an ordinary signed KV entry, so everything in it reaches every node. The
/// evidence itself therefore stays in the node-local [journal](super::evidence_journal), and the
/// chain carries only: the record's kind, the identities, the verified principal, the verdict, the
/// policy revision, the catalogue id, and the **content hash of the journal record**.
///
/// Never the exact resource, the arguments, an argument digest, the policy's reason, the checked
/// constraints, or any payload. AE0 §5 adopted this after reviewing its own first draft, and the
/// first implementation of gateway evidence then shipped the draft's shape anyway — sealing the
/// whole document into the chain and disseminating every decision's details cluster-wide. This type
/// exists so that mistake has to be made deliberately: there is no field here to put them in.
///
/// The hash is what keeps the chain meaningful despite carrying so little. The chain's ordering and
/// hash-linking cover the evidence through it, so a journal record that does not match its chained
/// hash is detectable — tamper-evidence without dissemination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AeReference {
    /// Always [`AE_REFERENCE_SCHEMA`].
    pub schema: String,
    /// Which of AE0 §5's five records this refers to. Today's enforcement point writes `decided`.
    pub kind: String,
    /// The caller's correlation identity.
    pub operation_id: String,
    /// This dispatch's identity.
    pub attempt_id: String,
    /// The verified principal (item 7).
    pub principal: String,
    /// The verdict.
    pub decision: DecisionKind,
    /// The revision of the policy that decided.
    pub policy_revision: String,
    /// The reviewed catalogue's id — the *only* mapping detail allowed here, and never the resource.
    pub catalogue: String,
    /// Lowercase hex of the journal record's content hash. `None` when no record was established,
    /// in which case there is nothing to cite and `evidence` says why.
    pub journal_sha256: Option<String>,
    /// Whether the journal established the evidence this record points at.
    pub evidence: EvidenceState,
}

/// The schema every safe reference record carries.
pub const AE_REFERENCE_SCHEMA: &str = "mycelium.ae/reference/1";

impl AeReference {
    /// The reference for one decision, given what the journal established.
    pub fn for_evidence(
        evidence: &AeEvidence,
        journal_sha256: Option<String>,
        state: EvidenceState,
    ) -> Self {
        Self {
            schema: AE_REFERENCE_SCHEMA.to_string(),
            kind: match evidence.kind {
                RecordKind::Decided => "decided",
                RecordKind::Execution => "execution",
            }
            .to_string(),
            operation_id: evidence.operation_id.clone(),
            attempt_id: evidence.attempt_id.clone(),
            principal: evidence.subject.clone(),
            decision: evidence.decision,
            policy_revision: evidence.policy_revision.clone(),
            catalogue: evidence.catalogue.clone(),
            journal_sha256,
            evidence: state,
        }
    }

    /// Parse a reference out of an audit record's `detail`.
    pub fn from_detail(detail: Option<&str>) -> Option<Self> {
        let parsed: Self = serde_json::from_str(detail?).ok()?;
        (parsed.schema == AE_REFERENCE_SCHEMA).then_some(parsed)
    }
}

/// Lowercase hex of a 32-byte digest.
pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
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
    /// The decision's evidence could not be established, so the action is refused **even when
    /// the policy permitted it**.
    ///
    /// An action allowed to proceed with no record of why is the gap this slice exists to close:
    /// enforcement without attribution is not governance, it is an unlogged gate. Refusing is
    /// therefore the honest failure mode, and it is visible rather than silent.
    NotRecorded(Decision),
    /// The request's own causal fields are malformed — a `correlation` that is not one key segment, a `parent`
    /// that is not a content hash (adopter-surface X4; [`crate::agent::causal`]). Refused before any policy
    /// question, and never corrected: a silently emptied parent would read as a root.
    Malformed(Decision),
}

impl PreflightRefusal {
    /// The decision behind the refusal.
    pub fn decision(&self) -> &Decision {
        match self {
            PreflightRefusal::Denied(d)
            | PreflightRefusal::NotEstablished(d)
            | PreflightRefusal::NotRecorded(d)
            | PreflightRefusal::Malformed(d) => d,
        }
    }

    /// Short machine-readable reason for JSON bodies, metrics and evidence.
    pub fn reason(&self) -> &'static str {
        match self {
            PreflightRefusal::Denied(_) => "action_denied",
            PreflightRefusal::NotEstablished(_) => "authority_not_established",
            PreflightRefusal::NotRecorded(_) => "evidence_not_recorded",
            PreflightRefusal::Malformed(_) => "request_malformed",
        }
    }

    /// The machine-readable `data` block a refusal carries to a caller.
    ///
    /// **One function, because two call sites drifted.** `/mcp` sent `reason`, `policy_revision`,
    /// `checked` and `errors`; `/a2a` sent the numeric code and a prose message and nothing else.
    /// The same refusal was therefore machine-readable at one enforcement point and not at the
    /// other — and `/a2a` is the federation edge, so the path a *partner domain* calls was the
    /// poorer one. A consumer that wrote refusal handling against one surface broke on the other.
    ///
    /// `policy_revision` is the field that earns its place here: without it a caller cannot tell a
    /// **stale-policy** refusal (this enforcement point expected a different artifact) from a real
    /// denial, and those call for opposite responses — retry after redeploying, versus stop.
    pub fn error_data(&self) -> serde_json::Value {
        let d = self.decision();
        serde_json::json!({
            "reason": self.reason(),
            "policy_revision": d.policy_revision,
            "checked": d.checked,
            "errors": d.errors,
        })
    }

    /// JSON-RPC error code for the MCP and A2A surfaces.
    pub fn json_rpc_code(&self) -> i32 {
        match self {
            PreflightRefusal::Denied(_) => -32030,
            PreflightRefusal::NotEstablished(_) => -32031,
            PreflightRefusal::NotRecorded(_) => -32032,
            // JSON-RPC's own *invalid params*: the fault is the request's shape, not authority.
            PreflightRefusal::Malformed(_) => -32602,
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
            PreflightRefusal::NotRecorded(_) => write!(
                f,
                "the decision under policy {} could not be recorded, so the action was refused",
                d.policy_revision
            ),
            PreflightRefusal::Malformed(_) => write!(f, "request malformed: {}", d.reason),
        }
    }
}

/// Ask the evaluator for the facts the enforcement point needs *before* it can build the envelope
/// — the declared argument names and the catalogue mapping — behind the same unwind boundary as
/// [`preflight`], so an adapter that panics here is contained wherever containment is possible.
/// `None` means the adapter panicked and nothing it said can be used.
///
/// Returns `(security_relevant_argument_names, mapping, history_request)`.
pub fn evaluator_facts(
    evaluator: &Arc<dyn ActionEvaluator>,
    operation: &str,
    resource: &str,
) -> Option<(Vec<String>, ActionMapping, Option<HistoryRequest>)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (evaluator.security_relevant_arguments(), evaluator.mapping(operation, resource), evaluator.history_request())
    }))
    .ok()
}

/// Run the evaluator over `envelope` and turn its decision into an admit-or-refuse.
///
/// **Call this rather than `ActionEvaluator::evaluate`.** This is the whole reason it is public:
/// building an enforcement point of your own — a resource that decides at its own boundary rather
/// than behind this gateway — means running the same six checks the gateway runs, and an
/// evaluator called directly performs **none** of them:
///
/// | The seam checks | Because an adapter that skipped it would |
/// |---|---|
/// | a `Permit` carrying evaluation errors is downgraded | trust a permit its own adapter could not fully evaluate |
/// | a decision from an unexpected `policy_revision` is stale | act on a policy this enforcement point never deployed |
/// | an expired envelope is denied *before* the evaluator runs | let a cached decision outlive its window — the evaluator holds no clock |
/// | an unmapped or ambiguous operation cannot be permitted | name an unreviewed action as a business operation |
/// | a panicking evaluator is caught, not trusted | turn an adapter bug into an admission |
/// | a permit relying on a fleet counter outside [`MAX_FLEET_SEATS`] / [`MAX_FLEET_WINDOW_MS`] is not established | permit on a count no enforcement point read (row K2's bounds) |
///
/// None of those is the evaluator's job, and none of them is optional. Skipping them is the
/// *"proved leg by leg and nowhere as a whole"* failure one layer out: every piece correct, and the
/// composition wrong.
///
/// Returns `Ok(Some(decision))` — admitted, with the decision for the evidence record;
/// `Ok(None)` — no evaluator is attached, so the seam is inert and the caller proceeds as before;
/// `Err(_)` — refused, and [`PreflightRefusal`] says which of the three refusals it is.
///
/// What it does **not** do: record anything. An admitted action still needs its evidence written,
/// and an enforcement point that decides without recording is an unlogged gate — which is what
/// [`PreflightRefusal::NotRecorded`] exists to refuse when the recording fails.
pub fn preflight(
    evaluator: Option<&Arc<dyn ActionEvaluator>>,
    envelope: &ActionEnvelope,
    now_ms: u64,
) -> Result<Option<Decision>, PreflightRefusal> {
    preflight_with_history(evaluator, envelope, None, now_ms)
}

/// [`preflight`], with the actor's history beside the envelope (post-360 row K) — what an enforcement point
/// that keeps one calls. `None` gives an evaluator that asked for history
/// [`History::Unavailable`]: a decision made without the history it needs is not established, never made
/// as though the actor had done nothing.
pub fn preflight_with_history(
    evaluator: Option<&Arc<dyn ActionEvaluator>>,
    envelope: &ActionEnvelope,
    history: Option<&History>,
    now_ms: u64,
) -> Result<Option<Decision>, PreflightRefusal> {
    let Some(evaluator) = evaluator else { return Ok(None) };
    let unavailable = History::Unavailable("no history was given to this decision".into());
    let history = history.unwrap_or(&unavailable);

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
    let decision = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        evaluator.evaluate_with_history(envelope, history)
    })) {
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

    // Row K2's bounds: a fleet counter past the seat cap or the history horizon was read by no enforcement point
    // (`fleet_history::establish` refuses it), so a permit relying on it — whatever the evaluator said — relies on a
    // count nobody established.
    let unbounded = match history {
        History::View(v) => v.fleet.iter().find_map(|f| f.counter.check_bounds().err().map(|e| (f.counter.scope, e))),
        History::Unavailable(_) => None,
    };
    if decision.verdict == Verdict::Permit
        && let Some((scope, out)) = unbounded
    {
        // The counter is one the enforcement point was given for this envelope; the permit may come from another rule,
        // so the reason says what was named, not what the permit relied on — closed either way (#653's review).
        let reason = format!(
            "a fleet counter named for this action at scope {} is outside the fleet bounds ({out}), and no enforcement \
             point reads one, so a permit is not established",
            scope.label()
        );
        let mut errors = decision.errors.clone();
        errors.push(format!("fleet history not established at scope {}: {out}", scope.label()));
        return Err(PreflightRefusal::NotEstablished(Decision { verdict: Verdict::Indeterminate, reason, errors, ..decision }));
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
    /// The **scope** a live mandate must cover for this rule to apply (AE1).
    ///
    /// Before AE1 a mandate could only be named in [`Rule::requires_facts`], i.e. as a fact this
    /// evaluator *cannot* establish — so every mandate clause decided `Indeterminate`. The envelope
    /// now carries the enforcement point's finding, so the clause can actually be decided.
    pub requires_mandate: Option<String>,
    /// Argument values the rule tests, by name — the reference evaluator compares for equality;
    /// a real adapter (Cedar) evaluates richer predicates over the same
    /// [`ActionEnvelope::selected_arguments`]. A named value the request did not carry is *not
    /// established*, never a silent mismatch.
    pub requires_values: Vec<(String, serde_json::Value)>,
    /// At most this many permitted actions matching the rule in a window, counted in the actor's history at
    /// the limit's scope (post-360 rows K and K2). `None`: no limit.
    pub limit: Option<HistoryLimit>,
    /// The request's data class this rule is about (post-360 row R): the rule applies only to a request
    /// of this class. A request whose class was not established matches, so the rule reports the class
    /// as *not established* rather than silently not applying. `None`: any class, or none.
    pub data_class: Option<String>,
    /// The destination locality this rule is about (post-360 row R): the rule applies only to a
    /// destination at this locality. A destination that was not established, or is
    /// [`Locality::Unknown`], matches, so the rule reports the locality as *not established*. `None`: anywhere.
    pub locality: Option<Locality>,
    /// The **counter id** a fleet-scoped limit counts under (row K2) — an opaque name the policy declares, so a
    /// fleet-visible key names the counter without naming the rule's operation or resource. `None` for a rule
    /// without a fleet limit (and, in a struct literal that leaves it out, a digest of the rule's patterns —
    /// [`Rule::counter_key`]).
    pub counter: Option<String>,
}

/// *At most `count` in `window_ms`* — a limit over the actor's history, counted at `scope`: this enforcement point
/// ([`HistoryScope::EnforcementPoint`], the default) or the fleet ([`HistoryScope::FleetConsensus`],
/// [`HistoryScope::FleetTally`] — row K2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimit {
    /// How many permitted actions matching the rule the window may hold, this one included.
    pub count: u32,
    /// How far back, in milliseconds.
    pub window_ms: u64,
    /// Where the count is kept (row K2). [`Rule::at_most`] sets [`HistoryScope::EnforcementPoint`];
    /// [`Rule::scoped`] chooses another.
    pub scope: HistoryScope,
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
            requires_mandate: None,
            requires_values: Vec::new(),
            limit: None,
            data_class: None,
            locality: None,
            counter: None,
        }
    }

    /// Apply this rule only to requests whose data is of `class` (post-360 row R) — e.g. with
    /// [`at_locality`](Self::at_locality), *confidential may not go to an external model*.
    pub fn for_class(mut self, class: impl Into<String>) -> Self {
        self.data_class = Some(class.into());
        self
    }

    /// Apply this rule only to a destination at `locality` (post-360 row R). A destination whose
    /// locality is not established is *not established* under this rule — never assumed in-domain.
    pub fn at_locality(mut self, locality: Locality) -> Self {
        self.locality = Some(locality);
        self
    }

    /// Permit at most `count` actions matching this rule in `window_ms`, counted in the actor's history at
    /// this enforcement point — e.g. *at most three posts in ten minutes*. An evaluator holding such a rule
    /// asks for history; without it the rule decides `Indeterminate`.
    pub fn at_most(mut self, count: u32, window_ms: u64) -> Self {
        self.limit = Some(HistoryLimit { count, window_ms, scope: HistoryScope::EnforcementPoint });
        self
    }

    /// Count this rule's limit at `scope` instead of at the enforcement point (row K2), under the counter id
    /// `counter` — after [`at_most`](Self::at_most): `Rule::new(..).at_most(3, TEN_MIN).scoped(HistoryScope::FleetConsensus,
    /// "c-posts-7f3a")` is *at most three in ten minutes across the fleet, exactly*; with `HistoryScope::FleetTally`
    /// the same limit read from the gossiped tallies, approximately. A rule without a limit is unchanged.
    ///
    /// **The counter id is what other members see** (digested with the kind of enforcement point): choose one that
    /// does not name what the rule covers if that is not for every member to know — it never encodes the rule's
    /// operation or resource. Two rules that should share a count share an id; the count survives a change of limit
    /// or window, and a new id starts a new count.
    pub fn scoped(mut self, scope: HistoryScope, counter: impl Into<String>) -> Self {
        if let Some(limit) = self.limit.as_mut() {
            limit.scope = scope;
            self.counter = Some(counter.into());
        }
        self
    }

    /// The identity of this rule's fleet counter (row K2): the declared counter id, or — for a rule built as a struct
    /// literal without one — a digest of its actor, operation and resource patterns, which anyone who can guess the
    /// patterns can recompute.
    pub fn counter_key(&self) -> String {
        if let Some(id) = &self.counter {
            return id.clone();
        }
        use sha2::Digest as _;
        let d = sha2::Sha256::digest(format!("{}\n{}\n{}", self.actor, self.operation, self.resource).as_bytes());
        hex32(&d.into())[..32].to_string()
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

    /// Require a live mandate covering `scope`. A missing or uncheckable mandate is *not
    /// established* (`Indeterminate`); a **refused** one denies before this rule is reached.
    pub fn requiring_mandate(mut self, scope: impl Into<String>) -> Self {
        self.requires_mandate = Some(scope.into());
        self
    }

    /// Require a named argument to equal `value`. Absent from the request ⇒ *not established*.
    pub fn requiring_value(mut self, name: impl Into<String>, value: serde_json::Value) -> Self {
        self.requires_values.push((name.into(), value));
        self
    }

    /// What this rule said about class and locality, for `checked` (row R) — `None` when nothing.
    fn route_clause(&self, env: &ActionEnvelope) -> Option<String> {
        if self.data_class.is_none() && self.locality.is_none() {
            return None;
        }
        let mut c = String::from("route");
        if let Some(class) = &self.data_class {
            c.push_str(&format!(" class={class}"));
        }
        if let Some(l) = self.locality {
            c.push_str(&format!(" locality={l}"));
        }
        if let Some(d) = &env.destination {
            c.push_str(&format!(" destination={} at {}", d.name, d.locality));
        }
        Some(c)
    }

    fn matches(&self, env: &ActionEnvelope) -> bool {
        let any = |pat: &str, v: &str| pat == "*" || pat == v;
        // A rule scoped to a mandate is about *that* scope: a binding for another scope is a
        // different appointment, so the rule simply does not apply. (A binding the fence refused
        // never reaches here — see `evaluate`.)
        let mandate_scope_matches = match (&self.requires_mandate, &env.mandate) {
            (None, _) => true,
            (Some(want), Some(b)) => &b.scope == want,
            (Some(_), None) => true, // no binding at all ⇒ let `unestablished` report it
        };
        mandate_scope_matches
            && any(&self.actor, &env.actor)
            && any(&self.operation, &env.operation)
            && any(&self.resource, &env.resource)
            && self.requires_scopes.iter().all(|s| env.scopes.iter().any(|h| h == s))
            // A declared value that is present but different means this rule is not the one; a
            // value that is *absent* still matches here, so `unestablished` can report it as a
            // missing fact rather than the rule silently falling through.
            && self.requires_values.iter().all(|(n, v)| {
                env.selected_arguments.get(n).map(|have| have == v).unwrap_or(true)
            })
            // Row R: a class or locality that is established and different means this rule is not
            // the one; one that is *not established* still matches, so `unestablished` reports it —
            // the same reading as an absent argument value above.
            && match (&self.data_class, &env.data_class) {
                (Some(want), Some(have)) => *want == have.class,
                _ => true,
            }
            && match (self.locality, &env.destination) {
                (Some(want), Some(d)) if d.locality != Locality::Unknown => want == d.locality,
                _ => true,
            }
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

    /// **The confined profile's first rule, checked** (Boundary H item A1): every allowance that
    /// could permit a protected `(operation, resource)` must require a mandate.
    ///
    /// Returns the allowances that would permit a protected operation **without** one — each a hole
    /// in the profile. An allowance matches a protected pair if its operation and resource are equal
    /// or `*`. Empty means the policy satisfies the rule. This checks the reference evaluator only;
    /// any other evaluator (the Cedar adapter) needs the same check over its own policy.
    pub fn allowances_without_mandate(&self, protected: &[(&str, &str)]) -> Vec<Rule> {
        let covers = |pattern: &str, value: &str| pattern == "*" || pattern == value;
        self.allowances
            .iter()
            .filter(|r| r.requires_mandate.is_none())
            .filter(|r| protected.iter().any(|(op, res)| covers(&r.operation, op) && covers(&r.resource, res)))
            .cloned()
            .collect()
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
    fn fleet_counters(&self, env: &ActionEnvelope) -> Vec<FleetCounter> {
        // The rule that would decide: a matching prohibition decides first and counts nothing; then the first
        // matching allowance — the same order `evaluate_with_history` takes.
        if self.prohibitions.iter().any(|r| r.matches(env)) {
            return Vec::new();
        }
        match self.allowances.iter().find(|r| r.matches(env)).and_then(|a| a.limit.map(|l| (a, l))) {
            Some((a, l)) if l.scope.is_fleet() => vec![FleetCounter::new(l.scope, a.counter_key(), l.count, l.window_ms)],
            _ => Vec::new(),
        }
    }

    fn history_request(&self) -> Option<HistoryRequest> {
        let limits: Vec<HistoryLimit> = self.allowances.iter().filter_map(|r| r.limit).collect();
        (!limits.is_empty()).then(|| HistoryRequest {
            window_ms: limits.iter().map(|l| l.window_ms).max().unwrap_or(0),
            // Every record in the window: the view is the actor's, across operations and resources, so a cap
            // near the limit would be filled by records no rule here counts. The enforcement point's own bound
            // still applies, and a view it had to cut says it is incomplete.
            max_entries: usize::MAX,
        })
    }

    fn evaluate(&self, env: &ActionEnvelope) -> Decision {
        self.evaluate_with_history(env, &History::Unavailable("evaluated without a history".into()))
    }

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

    fn evaluate_with_history(&self, env: &ActionEnvelope, history: &History) -> Decision {
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
            // A mandate the rule needs but the enforcement point could not establish. `Refused`
            // never lands here: it is a fence and has already denied, above.
            if let Some(scope) = &r.requires_mandate {
                match &env.mandate {
                    None => missing.push(format!("mandate not established: scope {scope}")),
                    Some(b) => {
                        if let MandateState::Unknown(why) = &b.state {
                            missing.push(format!(
                                "mandate not established: scope {scope} (term {}, epoch {}): {why}",
                                b.term, b.epoch
                            ));
                        }
                    }
                }
            }
            // A declared value the request did not carry is equally unestablished.
            for (name, _) in &r.requires_values {
                if !env.selected_arguments.contains_key(name) {
                    missing.push(format!("argument value not available: {name}"));
                }
            }
            // Row R: a rule about the request's class, or the destination's locality, on a request
            // whose class or destination this enforcement point did not establish.
            if let (Some(class), None) = (&r.data_class, &env.data_class) {
                missing.push(format!("data class not established: the rule is about class {class}"));
            }
            if let Some(want) = r.locality {
                match &env.destination {
                    None => missing.push(format!("destination locality not established: the rule is about {want}")),
                    Some(d) if d.locality == Locality::Unknown => missing.push(format!(
                        "destination locality not established for {}: the rule is about {want}",
                        d.name
                    )),
                    Some(_) => {}
                }
            }
            missing
        };

        // ---- The mandate fence, before policy ----------------------------------------------
        //
        // A mandate the enforcement point checked and the fence **refused** is not a fact policy
        // weighs; it is a boundary policy cannot open. Running this after the allow-list would let
        // a matching allowance turn a refusal into a permit — and for `Superseded` that is exactly
        // the laundering item 5 refuses to allow (which is why it is not a `Conflict`). The refusal
        // travels by name, because an operator told the wrong reason fixes the wrong thing.
        let refused = env.mandate.as_ref().and_then(|b| match &b.state {
            MandateState::Refused(refusal) => Some((b, refusal)),
            _ => None,
        });
        if let Some((b, refusal)) = refused {
            return Decision {
                verdict: Verdict::Deny,
                checked: vec![format!(
                    "mandate holder={} term={} scope={} epoch={}",
                    b.holder, b.term, b.scope, b.epoch
                )],
                reason: format!("the mandate fence refused this action: {refusal}"),
                policy_revision: rev,
                errors: Vec::new(),
            };
        }

        if let Some(p) = self.prohibitions.iter().find(|r| r.matches(env)) {
            let missing = unestablished(p);
            if !missing.is_empty() {
                return Decision {
                    verdict: Verdict::Indeterminate,
                    checked: std::iter::once(format!("prohibition actor={} operation={}", p.actor, p.operation))
                        .chain(p.route_clause(env))
                        .collect(),
                    reason: "a prohibition matches but needs facts this evaluator cannot establish".into(),
                    policy_revision: rev,
                    errors: missing,
                };
            }
            return Decision {
                verdict: Verdict::Deny,
                checked: std::iter::once(format!(
                    "prohibition actor={} operation={} resource={}",
                    p.actor, p.operation, p.resource
                ))
                .chain(p.route_clause(env))
                .collect(),
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
                    checked: std::iter::once(format!("allowance actor={} operation={}", a.actor, a.operation))
                        .chain(a.route_clause(env))
                        .collect(),
                    reason: "the matching allowance needs facts this evaluator cannot establish".into(),
                    policy_revision: rev,
                    errors: missing,
                };
            }
            // A limit over the actor's history (row K): counted at this enforcement point, and only on a
            // history it could read — an unreadable one is not an empty one.
            let mut counted: Option<String> = None;
            if let Some(limit) = a.limit {
                let limited = format!(
                    "allowance actor={} operation={} resource={} at most {} per {} ms",
                    a.actor, a.operation, a.resource, limit.count, limit.window_ms
                );
                let view = match history {
                    History::View(v) => v,
                    History::Unavailable(why) => {
                        return Decision {
                            verdict: Verdict::Indeterminate,
                            checked: vec![limited],
                            reason: "the matching allowance is limited over the actor's history, which was not established".into(),
                            policy_revision: rev,
                            errors: vec![format!("history not established: {why}")],
                        };
                    }
                };
                if limit.window_ms == 0 {
                    return Decision {
                        verdict: Verdict::Indeterminate,
                        checked: vec![limited],
                        reason: "the allowance is limited over an empty window, which counts nothing".into(),
                        policy_revision: rev,
                        errors: vec!["history limit with a window of 0 ms".into()],
                    };
                }
                if limit.scope.is_fleet() {
                    return match fleet_limit(a, limit, view, &rev, limited) {
                        Ok(line) => permit_of(a, env, Some(line), rev),
                        Err(decision) => decision,
                    };
                }
                let any = |pat: &str, v: &str| pat == "*" || pat == v;
                // Everything in the window *and after its start* counts — a record stamped later than this
                // view's clock (a call that read its clock after this one, decided first) is one more, not none.
                let from = view.as_of_ms.saturating_sub(limit.window_ms);
                let n = view
                    .entries
                    .iter()
                    .filter(|e| {
                        e.decision == DecisionKind::Permit
                            && e.at_ms > from
                            && any(&a.operation, &e.operation)
                            && any(&a.resource, &e.resource)
                    })
                    .count();
                let scope = view.scope.label();
                let tally = format!(
                    "history scope={scope} point={} window_ms={} counted={n} limit={}",
                    view.point, limit.window_ms, limit.count
                );
                if n >= limit.count as usize {
                    return Decision {
                        verdict: Verdict::Deny,
                        checked: vec![limited, tally],
                        reason: format!(
                            "the allowance permits at most {} in {} ms and the actor's history at scope {scope} already holds {n}",
                            limit.count, limit.window_ms
                        ),
                        policy_revision: rev,
                        errors: Vec::new(),
                    };
                }
                if !view.complete_for(limit.window_ms) {
                    return Decision {
                        verdict: Verdict::Indeterminate,
                        checked: vec![limited, tally],
                        reason: "the actor's history is incomplete, so a count below the limit is not established".into(),
                        policy_revision: rev,
                        errors: vec![format!("history incomplete at scope {scope}")],
                    };
                }
                counted = Some(tally);
            }
            return permit_of(a, env, counted, rev);
        }

        // Nothing matched. Authority is *not established* — never a denial (AE0 §9).
        Decision::indeterminate(
            "no allowance matches this action; the allow-list does not cover it",
            rev,
        )
    }
}

/// The reference evaluator's permit for allowance `a`, naming what it relied on.
fn permit_of(a: &Rule, env: &ActionEnvelope, counted: Option<String>, rev: String) -> Decision {
    Decision {
        verdict: Verdict::Permit,
        checked: {
            let mut c = vec![
                format!("allowance actor={} operation={} resource={}", a.actor, a.operation, a.resource),
                format!("scopes {:?}", a.requires_scopes),
                format!("values {:?}", a.requires_values.iter().map(|(n, _)| n).collect::<Vec<_>>()),
            ];
            // Evidence has to answer *which appointment* was relied on, not merely that
            // one was: `term` is which appointment, `epoch` orders authority.
            if let (Some(scope), Some(b)) = (&a.requires_mandate, &env.mandate) {
                c.push(format!(
                    "mandate scope={scope} holder={} term={} epoch={}",
                    b.holder, b.term, b.epoch
                ));
            }
            c.extend(a.route_clause(env));
            c.extend(counted);
            c
        },
        reason: "an allowance matches this action".into(),
        policy_revision: rev,
        errors: Vec::new(),
    }
}

/// A limit at a fleet scope (row K2), decided on the counter the enforcement point established: `Ok` with the line a
/// permit names, or the denial or undecided decision. A count at another scope never satisfies it — the point's
/// own history is not consulted.
fn fleet_limit(a: &Rule, limit: HistoryLimit, view: &HistoryView, rev: &str, limited: String) -> Result<String, Decision> {
    let scope = limit.scope.label();
    let key = a.counter_key();
    // An exact counter keeps seats per actor for good (a decision record and its sentinel each, never collected —
    // `docs/design/lock-lifecycle.md` §2, §8), so a wildcard actor would let every new principal add `count` slots
    // to every member. The reference evaluator bounds that growth by the policy: an exact fleet limit names its
    // actor (review finding 4).
    if limit.scope == HistoryScope::FleetConsensus && a.actor == "*" {
        return Err(Decision {
            verdict: Verdict::Indeterminate,
            checked: vec![limited],
            reason: format!("a limit at scope {scope} must name its actor, not `*`"),
            policy_revision: rev.to_string(),
            errors: vec![format!(
                "fleet history not established at scope {scope}: a wildcard actor would let each new principal add \
                 seats that are never collected"
            )],
        });
    }
    // A fleet limit is bounded: past the seat cap or the history horizon it is a cost on every request the rule covers,
    // not a limit (the Cedar adapter's review). Not established, as a wildcard actor is — never clamped.
    if let Err(out) = check_fleet_bounds(limit.count, limit.window_ms) {
        return Err(Decision {
            verdict: Verdict::Indeterminate,
            checked: vec![limited],
            reason: format!("a limit at scope {scope} must be within the fleet bounds: {out}"),
            policy_revision: rev.to_string(),
            errors: vec![format!("fleet history not established at scope {scope}: {out}")],
        });
    }
    let undecided = |why: String| Decision {
        verdict: Verdict::Indeterminate,
        checked: vec![limited.clone()],
        reason: format!("the matching allowance is limited at scope {scope}, whose count was not established"),
        policy_revision: rev.to_string(),
        errors: vec![format!("fleet history not established at scope {scope}: {why}")],
    };
    let Some(found) = view.fleet.iter().find(|c| c.counter.scope == limit.scope && c.counter.key == key) else {
        return Err(undecided("the enforcement point gave no count for this rule's counter".into()));
    };
    let deny = |line: String, held: u32, caveat: &str| Decision {
        verdict: Verdict::Deny,
        checked: vec![limited.clone(), line],
        reason: format!(
            "the allowance permits at most {} in {} ms and the fleet's count at scope {scope} already holds {held}{caveat}",
            limit.count, limit.window_ms
        ),
        policy_revision: rev.to_string(),
        errors: Vec::new(),
    };
    match &found.state {
        FleetState::Seats { held } => {
            let line = format!(
                "history scope={scope} counter={key} window_ms={} seats_held={held} limit={} — a permit claims a seat by consensus before it is admitted",
                limit.window_ms, limit.count
            );
            if *held >= limit.count {
                return Err(deny(line, *held, ""));
            }
            Ok(line)
        }
        FleetState::Tally { counted, publishers, stalest_ms } => {
            let line = format!(
                "history scope={scope} counter={key} window_ms={} counted={counted} limit={} publishers={publishers:?} stalest_ms={stalest_ms} — {FLEET_TALLY_CAVEAT}",
                limit.window_ms, limit.count
            );
            if *counted >= limit.count {
                return Err(deny(line, *counted, " (approximate: a gossiped tally)"));
            }
            Ok(line)
        }
        FleetState::NotEstablished(why) => Err(undecided(why.clone())),
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
    use mycelium_core::receipt::LocalDurability;

    fn node() -> NodeId {
        NodeId::new("127.0.0.1", 9000).unwrap()
    }

    /// A metric label and the evidence document's wire form are **the same string**, so nobody has
    /// to translate between a dashboard and a record — and neither can drift without the other.
    ///
    /// The literals are asserted too. Deriving the expected value from the same function the code
    /// uses would make this test pass through any rename, which is precisely what it exists to stop:
    /// a rename here breaks every operator's dashboard *and* every consumer's parser.
    #[test]
    fn decision_and_mapping_labels_are_the_wire_form_and_are_stable() {
        for kind in [DecisionKind::Permit, DecisionKind::Deny, DecisionKind::Indeterminate] {
            let wire = serde_json::to_string(&kind).expect("serialises");
            assert_eq!(wire, format!("\"{}\"", kind.label()), "label must equal the wire form");
        }
        for kind in [MappingKind::Mapped, MappingKind::Unmapped, MappingKind::Ambiguous] {
            let wire = serde_json::to_string(&kind).expect("serialises");
            assert_eq!(wire, format!("\"{}\"", kind.label()), "label must equal the wire form");
        }

        assert_eq!(DecisionKind::Permit.label(), "permit");
        assert_eq!(DecisionKind::Deny.label(), "deny");
        assert_eq!(DecisionKind::Indeterminate.label(), "indeterminate");
        assert_eq!(MappingKind::Mapped.label(), "mapped");
        assert_eq!(MappingKind::Unmapped.label(), "unmapped");
        assert_eq!(MappingKind::Ambiguous.label(), "ambiguous");
    }

    /// An unknown verdict counts as `indeterminate`, never as `permit`. The counter inherits the
    /// seam's own rule rather than restating it, so a future variant cannot widen what a dashboard
    /// reports as permitted.
    #[test]
    fn an_unknown_verdict_counts_as_indeterminate() {
        assert_eq!(DecisionKind::from(Verdict::Permit).label(), "permit");
        assert_eq!(DecisionKind::from(Verdict::Deny).label(), "deny");
        assert_eq!(DecisionKind::from(Verdict::Indeterminate).label(), "indeterminate");
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


    // ── The composed guarantee ──────────────────────────────────────────────────────────
    //
    // Design of record: `docs/design/composed-effect.md`.

    /// **The gate the Phase-C audit asked for.**
    ///
    /// The axis' headline claim is *a durable, attributed, cross-domain effect*. Posture rule 6
    /// says a composed guarantee is established by argument **and** gate, never by naming — and the
    /// audit found it *"proved leg by leg and nowhere as a whole"*.
    ///
    /// So: from **one execution record**, with no access to the receipt that was returned, the
    /// caller's memory or the federation client's state, reconstruct the whole sentence.
    ///
    /// What this does NOT prove: that the attribution is *correct*. Item 7 establishes that, at its
    /// own strength; this record carries item 7's verdict rather than re-deciding it. The claim
    /// here is that the sentence is **checkable from one artefact** rather than believed across
    /// four — which is what the audit asked for, and all of it.
    #[test]
    fn a_durable_attributed_cross_domain_effect_is_reconstructable_from_one_record() {
        let env = ActionEnvelope::builder(
            "federation:depot.example/dispatcher",
            node(),
            "tools/call",
            "tool:reroute@n1",
        )
        .identities("op-77", "op-77/1")
        .scopes(["mcp:invoke"])
        .mapping(ActionMapping::mapped("cat", "1"))
        .validity(1_000, 61_000)
        .build();

        let decision = Decision::permit("within the granted remit", "rev-9");
        let record = AeEvidence::for_decision(&env, &decision, Execution::Completed, "depot-resource")
            .with_effect_durability(LocalDurability::OnDisk)
            .with_origin_domain("depot.example");

        // Durable — the rung the receipt named, not a re-derivation of it.
        assert_eq!(
            record.effect_durability,
            Some(LocalDurability::OnDisk),
            "the record must carry the receipt's rung",
        );
        // Attributed — item 7's verified principal.
        assert_eq!(record.subject, "federation:depot.example/dispatcher");
        // Cross-domain — a fact on the record, not an inference from the principal's spelling.
        assert_eq!(record.origin_domain.as_deref(), Some("depot.example"));
        // ... and an effect, which is the part that was never in doubt.
        assert_eq!(record.execution, Execution::Completed);
        assert_eq!(record.operation_id, "op-77");

        assert!(
            record.states_a_composed_effect(),
            "all four present must read as the composed claim",
        );
    }

    /// The gate must fail when **any** leg is missing, or it is not testing the composition.
    ///
    /// Both plants live here rather than in a comment: remove the durability rung, and remove the
    /// origin domain. A test that still passes with either absent would have been satisfied by the
    /// world before this change.
    #[test]
    fn the_composed_claim_needs_every_leg() {
        let env = ActionEnvelope::builder(
            "federation:depot.example/dispatcher",
            node(),
            "tools/call",
            "tool:reroute@n1",
        )
        .identities("op-77", "op-77/1")
        .mapping(ActionMapping::mapped("cat", "1"))
        .validity(1_000, 61_000)
        .build();
        let decision = Decision::permit("ok", "rev-9");
        let whole = AeEvidence::for_decision(&env, &decision, Execution::Completed, "depot-resource")
            .with_effect_durability(LocalDurability::OnDisk)
            .with_origin_domain("depot.example");
        assert!(whole.states_a_composed_effect());

        let no_rung = AeEvidence::for_decision(&env, &decision, Execution::Completed, "depot-resource")
            .with_origin_domain("depot.example");
        assert!(
            !no_rung.states_a_composed_effect(),
            "without a durability rung the effect is not known to be durable",
        );

        let no_domain =
            AeEvidence::for_decision(&env, &decision, Execution::Completed, "depot-resource")
                .with_effect_durability(LocalDurability::OnDisk);
        assert!(
            !no_domain.states_a_composed_effect(),
            "without an origin domain the effect is not known to be cross-domain",
        );
    }

    /// **The negative the finding is really about.** An effect whose durability was *not
    /// established* must not read as a durable one — and neither must one that was merely buffered.
    ///
    /// `Buffered` is the case worth having: the bytes survive a process crash and are lost to a
    /// power failure. It is a real rung and it is **not** `OnDisk`, so a composed claim resting on
    /// it would overstate exactly the thing item 1 added `Buffered` to stop overstating.
    #[test]
    fn a_buffered_or_failed_effect_does_not_state_a_durable_one() {
        let env = ActionEnvelope::builder(
            "federation:depot.example/dispatcher",
            node(),
            "tools/call",
            "tool:reroute@n1",
        )
        .identities("op-77", "op-77/1")
        .mapping(ActionMapping::mapped("cat", "1"))
        .validity(1_000, 61_000)
        .build();
        let decision = Decision::permit("ok", "rev-9");

        for rung in [
            LocalDurability::Buffered,
            LocalDurability::Failed("writer gone".into()),
            LocalDurability::NotConfigured,
        ] {
            let r = AeEvidence::for_decision(&env, &decision, Execution::Completed, "depot-resource")
                .with_effect_durability(rung.clone())
                .with_origin_domain("depot.example");
            assert!(
                !r.states_a_composed_effect(),
                "{rung:?} is not a durable effect, and the record must not say it is",
            );
        }
    }

    /// An effect that did not *complete* states nothing composed either, however durable the
    /// record claims to be — `Attempted` is item 1's honest answer and it is not completion.
    #[test]
    fn an_unfinished_effect_states_nothing_composed() {
        let env = ActionEnvelope::builder(
            "federation:depot.example/dispatcher",
            node(),
            "tools/call",
            "tool:reroute@n1",
        )
        .identities("op-77", "op-77/1")
        .mapping(ActionMapping::mapped("cat", "1"))
        .validity(1_000, 61_000)
        .build();
        let decision = Decision::permit("ok", "rev-9");

        for ex in [Execution::Attempted, Execution::Unknown, Execution::Failed, Execution::None] {
            let r = AeEvidence::for_decision(&env, &decision, ex, "depot-resource")
                .with_effect_durability(LocalDurability::OnDisk)
                .with_origin_domain("depot.example");
            assert!(
                !r.states_a_composed_effect(),
                "{ex:?} is not a completed effect",
            );
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


    /// X4: the envelope's causal fields are on the decision record and its execution record, absent from the
    /// serialised form when unset, and a record written before the fields existed still parses.
    #[test]
    fn the_causal_fields_reach_the_evidence_and_an_old_record_parses_without_them() {
        let hash = "cd".repeat(32);
        let env = ActionEnvelope::builder("oidc:idp/alice", node(), "tools/call", "tool:post@n1")
            .identities("op-1", "op-1/1")
            .mapping(ActionMapping::mapped("cat", "1"))
            .causal(crate::causal::CausalFields { correlation: Some("run-1".into()), parent: Some(hash.clone()) })
            .build();
        let d = Decision::permit("ok", "rev-1");
        let decided = AeEvidence::for_decision(&env, &d, Execution::Attempted, "gateway");
        assert_eq!((decided.correlation.as_deref(), decided.parent.as_deref()), (Some("run-1"), Some(hash.as_str())));
        let executed = decided.as_execution(Execution::Completed);
        assert_eq!((executed.correlation.as_deref(), executed.parent.as_deref()), (Some("run-1"), Some(hash.as_str())));
        let bare = AeEvidence::for_decision(&envelope("oidc:idp/alice", "tools/call", "tool:post@n1"), &d, Execution::None, "gateway");
        let text = serde_json::to_string(&bare).unwrap();
        assert!(!text.contains("correlation") && !text.contains("\"parent\""), "{text}");
        let old: AeEvidence = serde_json::from_str(&text).unwrap();
        assert!(old.correlation.is_none() && old.parent.is_none());
    }

    // ── Row K: policy over history ──────────────────────────────────────────────────────────────

    const TEN_MIN: u64 = 600_000;

    fn posts(limit: u32) -> ReferenceEvaluator {
        ReferenceEvaluator::new("rev-1").allow(
            Rule::new("oidc:idp/alice", "tools/call", "tool:post@n1").requiring_scopes(["mcp:invoke"]).at_most(limit, TEN_MIN),
        )
    }

    fn entry(at_ms: u64, attempt: &str, decision: DecisionKind) -> HistoryEntry {
        HistoryEntry {
            at_ms,
            attempt_id: attempt.into(),
            operation: "tools/call".into(),
            resource: "tool:post@n1".into(),
            decision,
            execution: Execution::Completed,
        }
    }

    fn history(as_of_ms: u64, entries: Vec<HistoryEntry>, complete: bool) -> History {
        History::View(HistoryView {
            scope: HistoryScope::EnforcementPoint,
            actor: "oidc:idp/alice".into(),
            point: "gateway".into(),
            as_of_ms,
            window_ms: TEN_MIN,
            entries,
            complete,
            omitted_through_ms: (!complete).then_some(as_of_ms),
            fleet: Vec::new(),
        })
    }

    #[test]
    fn a_limited_allowance_asks_for_the_history_it_counts() {
        assert_eq!(posts(3).history_request(), Some(HistoryRequest { window_ms: TEN_MIN, max_entries: usize::MAX }));
        assert_eq!(ReferenceEvaluator::new("rev-1").allow(Rule::new("*", "*", "*")).history_request(), None);
    }

    /// *At most three posts in ten minutes*: the fourth is denied, and the decision names whose history it counted.
    #[test]
    fn at_most_three_in_ten_minutes_denies_the_fourth_naming_the_scope() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let now = 2_000_000;
        let two = history(now, vec![entry(now - 1_000, "a", DecisionKind::Permit), entry(now - 2_000, "b", DecisionKind::Permit)], true);
        let d = posts(3).evaluate_with_history(&env, &two);
        assert_eq!(d.verdict, Verdict::Permit, "{d:?}");
        assert!(d.checked.iter().any(|c| c.contains("history scope=enforcement_point") && c.contains("counted=2")), "{d:?}");

        let three = history(now, vec![entry(now - 3_000, "a", DecisionKind::Permit), entry(now - 2_000, "b", DecisionKind::Permit),
                                      entry(now - 1_000, "c", DecisionKind::Permit)], true);
        let d = posts(3).evaluate_with_history(&env, &three);
        assert_eq!(d.verdict, Verdict::Deny, "{d:?}");
        assert!(d.reason.contains("enforcement_point") && d.reason.contains("at most 3"), "{d:?}");
    }

    #[test]
    fn only_permits_inside_the_window_and_matching_the_rule_count() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let now = 2_000_000;
        let mut other = entry(now - 500, "x", DecisionKind::Permit);
        other.resource = "tool:read@n1".into();
        let h = history(now, vec![
            entry(now - TEN_MIN, "old", DecisionKind::Permit), // exactly a window ago: outside it
            entry(now - 2_000, "d1", DecisionKind::Deny),
            entry(now - 1_500, "d2", DecisionKind::Indeterminate),
            other,
            entry(now - 1_000, "p1", DecisionKind::Permit),
            entry(now - 900, "p2", DecisionKind::Permit),
        ], true);
        assert_eq!(posts(3).evaluate_with_history(&env, &h).verdict, Verdict::Permit);
    }

    /// A history that cannot be read is not an empty one.
    #[test]
    fn an_unavailable_history_is_not_established_never_empty() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let d = posts(3).evaluate_with_history(&env, &History::Unavailable("the journal could not be read".into()));
        assert_eq!(d.verdict, Verdict::Indeterminate);
        assert!(d.errors.iter().any(|e| e.contains("history not established") && e.contains("could not be read")), "{d:?}");
        // Decided on the request alone — `evaluate`, the pre-row-K entry — a limited rule is not established either.
        assert_eq!(posts(3).evaluate(&env).verdict, Verdict::Indeterminate);
        // And through the seam with no history given.
        let ev: Arc<dyn ActionEvaluator> = Arc::new(posts(3));
        assert!(matches!(preflight(Some(&ev), &env, 2_000), Err(PreflightRefusal::NotEstablished(_))));
    }

    /// Below the limit on an incomplete view is not established; at the limit it is a denial whatever is missing.
    #[test]
    fn an_incomplete_history_establishes_a_denial_but_not_a_permit() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let now = 2_000_000;
        let one = history(now, vec![entry(now - 1_000, "a", DecisionKind::Permit)], false);
        let d = posts(3).evaluate_with_history(&env, &one);
        assert_eq!(d.verdict, Verdict::Indeterminate, "{d:?}");
        assert!(d.errors.iter().any(|e| e.contains("incomplete")), "{d:?}");
        let three = history(now, (0..3).map(|i| entry(now - 1_000 - i, &format!("p{i}"), DecisionKind::Permit)).collect(), false);
        assert_eq!(posts(3).evaluate_with_history(&env, &three).verdict, Verdict::Deny);
    }

    /// An evaluator that asks for no history is unchanged: the seam's history path calls its `evaluate`.
    #[test]
    fn an_evaluator_without_a_limit_ignores_history() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let plain = ReferenceEvaluator::new("rev-1").allow(Rule::new("oidc:idp/alice", "tools/call", "tool:post@n1").requiring_scopes(["mcp:invoke"]));
        let full = history(2_000_000, (0..10).map(|i| entry(1_999_000 + i, &format!("p{i}"), DecisionKind::Permit)).collect(), true);
        assert_eq!(plain.evaluate_with_history(&env, &full), plain.evaluate(&env));
        assert_eq!(plain.evaluate_with_history(&env, &History::Unavailable("x".into())).verdict, Verdict::Permit);
    }

    /// The evidence record carries what the decision read — absent when it read none, so older records are unchanged.
    #[test]
    fn the_history_a_decision_read_is_on_its_evidence_record() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let d = Decision::indeterminate("x", "rev-1");
        let plain = AeEvidence::for_decision(&env, &d, Execution::None, "gateway");
        assert!(!serde_json::to_string(&plain).unwrap().contains("history"), "a record without history serialises as before");
        let now = 2_000_000;
        let h = history(now, vec![entry(now - 1_000, "a", DecisionKind::Permit)], true);
        let with = plain.with_history(HistoryInput::of(&h));
        let back: AeEvidence = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        let input = back.history.expect("carried");
        assert_eq!(input.scope, Some(HistoryScope::EnforcementPoint));
        assert_eq!(input.attempts, vec!["a".to_string()]);
        assert!(input.complete && input.unavailable.is_none());
        let none = HistoryInput::of(&History::Unavailable("no journal".into()));
        assert_eq!(none.scope, None);
        assert_eq!(none.unavailable.as_deref(), Some("no journal"));
    }

    // ── Row K2: the fleet scopes ─────────────────────────────────────────────────────────────────

    fn fleet_posts(limit: u32, scope: HistoryScope) -> ReferenceEvaluator {
        ReferenceEvaluator::new("rev-1").allow(
            Rule::new("oidc:idp/alice", "tools/call", "tool:post@n1")
                .requiring_scopes(["mcp:invoke"])
                .at_most(limit, TEN_MIN)
                .scoped(scope, "c-posts"),
        )
    }

    /// The view an enforcement point gives: `local` permits at the point, and the fleet counter in `state`.
    fn fleet_view(ev: &ReferenceEvaluator, env: &ActionEnvelope, local: usize, state: Option<FleetState>) -> History {
        let now = 2_000_000;
        let mut h = history(now, (0..local).map(|i| entry(now - 1_000 - i as u64, &format!("p{i}"), DecisionKind::Permit)).collect(), true);
        if let (History::View(v), Some(state)) = (&mut h, state) {
            v.fleet = ev.fleet_counters(env).into_iter().map(|c| FleetCount::new(c, state.clone())).collect();
        }
        h
    }

    /// A rule names its scope with `scoped`, and asks for the fleet counter of the allowance that would decide —
    /// none for a per-point rule, none when a prohibition decides first.
    #[test]
    fn a_fleet_scoped_rule_names_its_counter() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let exact = fleet_posts(3, HistoryScope::FleetConsensus);
        let named = exact.fleet_counters(&env);
        assert_eq!(named.len(), 1);
        assert_eq!((named[0].scope, named[0].count, named[0].window_ms), (HistoryScope::FleetConsensus, 3, TEN_MIN));
        assert_eq!(named[0].key, "c-posts", "the declared counter id, never the rule's operation or resource");
        assert_eq!(fleet_posts(3, HistoryScope::FleetTally).fleet_counters(&env)[0].scope, HistoryScope::FleetTally);
        assert!(posts(3).fleet_counters(&env).is_empty(), "a per-point limit names no fleet counter");
        let forbidden = fleet_posts(3, HistoryScope::FleetConsensus).prohibit(Rule::new("*", "tools/call", "*"));
        assert!(forbidden.fleet_counters(&env).is_empty(), "a prohibition decides first and counts nothing");
        assert!(exact.fleet_counters(&envelope("oidc:idp/bob", "tools/call", "tool:post@n1")).is_empty());
        // The count survives a change of limit or window; what the rule matches is its identity.
        assert_eq!(Rule::new("a", "o", "r").at_most(9, 9).scoped(HistoryScope::FleetTally, "k").counter_key(), "k");
        assert!(!named[0].key.contains("tools/call") && !named[0].key.contains("tool:post"));
        // `scoped` without a limit changes nothing.
        let unlimited = Rule::new("a", "o", "r").scoped(HistoryScope::FleetTally, "k");
        assert_eq!((unlimited.limit, unlimited.counter), (None, None));
    }

    /// A count at another scope never satisfies a fleet rule: the point's own history below the limit, and no
    /// fleet count given, is not established — and a full point history does not deny it either.
    #[test]
    fn a_fleet_rule_is_not_satisfied_by_the_points_count() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        for scope in [HistoryScope::FleetConsensus, HistoryScope::FleetTally] {
            let ev = fleet_posts(3, scope);
            let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, None));
            assert_eq!(d.verdict, Verdict::Indeterminate, "{scope:?}: {d:?}");
            assert!(d.errors.iter().any(|e| e.contains("fleet history not established") && e.contains(scope.label())), "{d:?}");
            let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 5, Some(FleetState::Seats { held: 0 })));
            if scope == HistoryScope::FleetConsensus {
                assert_eq!(d.verdict, Verdict::Permit, "five permits at this point do not fill the fleet's seats: {d:?}");
            }
        }
    }

    /// The exact scope: seats held at the limit deny, naming the scope; below it, a permit that says a seat is
    /// claimed before admission; a counter not established is not established.
    #[test]
    fn the_consensus_scope_decides_on_the_seats_and_names_itself() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let ev = fleet_posts(3, HistoryScope::FleetConsensus);
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(FleetState::Seats { held: 3 })));
        assert_eq!(d.verdict, Verdict::Deny, "{d:?}");
        assert!(d.reason.contains("fleet_consensus") && d.checked.iter().any(|c| c.contains("seats_held=3")), "{d:?}");
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(FleetState::Seats { held: 2 })));
        assert_eq!(d.verdict, Verdict::Permit, "{d:?}");
        assert!(d.checked.iter().any(|c| c.contains("history scope=fleet_consensus") && c.contains("claims a seat")), "{d:?}");
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(FleetState::NotEstablished("no consensus".into()))));
        assert_eq!(d.verdict, Verdict::Indeterminate, "{d:?}");
        assert!(d.errors.iter().any(|e| e.contains("fleet_consensus") && e.contains("no consensus")), "{d:?}");
        let d = ev.evaluate_with_history(&env, &History::Unavailable("no journal".into()));
        assert_eq!(d.verdict, Verdict::Indeterminate, "an unreadable history is not an empty one at a fleet scope either");
    }

    /// An exact fleet limit names its actor: a wildcard would let every new principal add seats that are never
    /// collected (review finding 4), so the reference evaluator does not establish it. The tally, whose entries are
    /// collected once they age out, takes a wildcard.
    #[test]
    fn an_exact_fleet_limit_over_a_wildcard_actor_is_not_established() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        for (scope, expect) in [(HistoryScope::FleetConsensus, Verdict::Indeterminate), (HistoryScope::FleetTally, Verdict::Permit)] {
            let ev = ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "tools/call", "tool:post@n1").requiring_scopes(["mcp:invoke"]).at_most(3, TEN_MIN).scoped(scope, "c"));
            let state = match scope {
                HistoryScope::FleetConsensus => FleetState::Seats { held: 0 },
                _ => FleetState::Tally { counted: 0, publishers: vec!["n1".into()], stalest_ms: 0 },
            };
            let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(state)));
            assert_eq!(d.verdict, expect, "{scope:?}: {d:?}");
            if expect == Verdict::Indeterminate {
                assert!(d.reason.contains("must name its actor"), "{d:?}");
            }
        }
    }

    /// **A fleet limit is bounded** (the Cedar adapter's review, a denial of service by misconfiguration): the
    /// enforcement point reads every seat of a consensus counter on each in-scope request, so a limit of `u32::MAX`
    /// would pin a worker for minutes; and a window past the history horizon leases a seat, and keeps a tally entry,
    /// for longer than anything else here is kept. One seat past the cap, or one millisecond past the horizon, is not
    /// established at either fleet scope; the boundary values are a limit like any other.
    #[test]
    fn a_fleet_limit_past_the_seat_cap_or_the_horizon_is_not_established() {
        const CAP: u32 = 1_024;
        const HORIZON: u64 = 24 * 60 * 60 * 1_000;
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        for scope in [HistoryScope::FleetConsensus, HistoryScope::FleetTally] {
            let ev = |count: u32, window_ms: u64| {
                ReferenceEvaluator::new("rev-1").allow(
                    Rule::new("oidc:idp/alice", "tools/call", "tool:post@n1")
                        .requiring_scopes(["mcp:invoke"])
                        .at_most(count, window_ms)
                        .scoped(scope, "c-posts"),
                )
            };
            let room = match scope {
                HistoryScope::FleetConsensus => FleetState::Seats { held: 0 },
                _ => FleetState::Tally { counted: 0, publishers: vec!["n1".into()], stalest_ms: 0 },
            };
            for (count, window_ms, what) in [(CAP + 1, TEN_MIN, "seats"), (u32::MAX, TEN_MIN, "seats"), (3, HORIZON + 1, "horizon")] {
                let ev = ev(count, window_ms);
                // As an enforcement point would give it: the counter as named, with room to spare.
                let mut h = history(2_000_000, Vec::new(), true);
                if let History::View(v) = &mut h {
                    v.fleet = vec![FleetCount::new(FleetCounter::new(scope, "c-posts", count, window_ms), room.clone())];
                }
                let d = ev.evaluate_with_history(&env, &h);
                assert_eq!(d.verdict, Verdict::Indeterminate, "{scope:?} {count} in {window_ms} ms: {d:?}");
                let said = format!("{} {}", d.reason, d.errors.join(" "));
                assert!(said.contains(scope.label()) && said.contains(what), "{scope:?}: names the scope and the bound: {d:?}");
                // The seam refuses it too, whatever the evaluator said.
                let r = preflight_with_history(Some(&(Arc::new(ev) as Arc<dyn ActionEvaluator>)), &env, Some(&h), 30_000);
                assert!(matches!(r, Err(PreflightRefusal::NotEstablished(_))), "{r:?}");
            }
            // The boundary values: the cap and the horizon themselves are limits like any other.
            let ev = ev(CAP, HORIZON);
            let mut h = history(2_000_000, Vec::new(), true);
            if let History::View(v) = &mut h {
                v.fleet = vec![FleetCount::new(FleetCounter::new(scope, "c-posts", CAP, HORIZON), room.clone())];
            }
            let d = ev.evaluate_with_history(&env, &h);
            assert_eq!(d.verdict, Verdict::Permit, "{scope:?} at the cap and the horizon: {d:?}");
        }
    }

    /// **The seam bounds every evaluator's fleet counters**, not only the reference's: an evaluator that permits on a
    /// counter past the seat cap or the horizon is not established — the enforcement point read no such counter — and
    /// one at the bounds is admitted.
    #[test]
    fn the_seam_refuses_a_permit_on_an_unbounded_fleet_counter() {
        const CAP: u32 = 1_024;
        const HORIZON: u64 = 24 * 60 * 60 * 1_000;
        struct Permissive(FleetCounter);
        impl ActionEvaluator for Permissive {
            fn evaluate(&self, _env: &ActionEnvelope) -> Decision {
                Decision { verdict: Verdict::Permit, checked: vec!["anything".into()], reason: "permissive".into(), policy_revision: "rev-1".into(), errors: Vec::new() }
            }
            fn fleet_counters(&self, _env: &ActionEnvelope) -> Vec<FleetCounter> {
                vec![self.0.clone()]
            }
        }
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        for scope in [HistoryScope::FleetConsensus, HistoryScope::FleetTally] {
            for (count, window_ms, ok) in [(CAP + 1, TEN_MIN, false), (3, HORIZON + 1, false), (3, 0, false), (CAP, HORIZON, true), (0, 1, true)] {
                let counter = FleetCounter::new(scope, "k", count, window_ms);
                let mut h = history(2_000_000, Vec::new(), true);
                if let History::View(v) = &mut h {
                    v.fleet = vec![FleetCount::new(counter.clone(), FleetState::Seats { held: 0 })];
                }
                let ev: Arc<dyn ActionEvaluator> = Arc::new(Permissive(counter));
                let r = preflight_with_history(Some(&ev), &env, Some(&h), 30_000);
                if ok {
                    assert!(matches!(r, Ok(Some(_))), "{scope:?} {count} in {window_ms} ms is within the bounds: {r:?}");
                } else {
                    let Err(PreflightRefusal::NotEstablished(d)) = &r else { panic!("{scope:?} {count} in {window_ms} ms: {r:?}") };
                    assert!(d.reason.contains(scope.label()), "names the scope: {d:?}");
                }
            }
        }
    }

    /// The approximate scope: the sum at the limit denies; below it permits — and **every** tally decision says it is
    /// approximate, detection not prevention, and names its coverage. Unknown coverage is not established.
    #[test]
    fn the_tally_scope_states_it_is_approximate_on_every_decision() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let ev = fleet_posts(3, HistoryScope::FleetTally);
        let tally = |counted| FleetState::Tally { counted, publishers: vec!["n1".into(), "n2".into()], stalest_ms: 40 };
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(tally(3))));
        assert_eq!(d.verdict, Verdict::Deny, "{d:?}");
        assert!(d.reason.contains("fleet_tally") && d.reason.contains("approximate"), "{d:?}");
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(tally(2))));
        assert_eq!(d.verdict, Verdict::Permit, "{d:?}");
        let line = d.checked.iter().find(|c| c.contains("history scope=fleet_tally")).expect("names its scope");
        assert!(line.contains(FLEET_TALLY_CAVEAT) && line.contains("publishers=[\"n1\", \"n2\"]") && line.contains("stalest_ms=40"), "{line}");
        let d = ev.evaluate_with_history(&env, &fleet_view(&ev, &env, 0, Some(FleetState::NotEstablished("the tally of n2 is stale".into()))));
        assert_eq!(d.verdict, Verdict::Indeterminate, "unknown coverage is not established, never an empty tally: {d:?}");
        assert!(d.errors.iter().any(|e| e.contains("fleet_tally") && e.contains("stale")), "{d:?}");
        // A count of the other fleet scope does not stand in for this one.
        let d = ev.evaluate_with_history(&env, &fleet_view(&fleet_posts(3, HistoryScope::FleetConsensus), &env, 0, Some(FleetState::Seats { held: 0 })));
        assert_eq!(d.verdict, Verdict::Indeterminate, "{d:?}");
    }

    /// The fleet counters a decision read and the seats its permit claimed are on its evidence record; a record
    /// without them serialises as before.
    #[test]
    fn the_fleet_count_and_the_seat_are_on_the_evidence_record() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:post@n1");
        let ev = fleet_posts(3, HistoryScope::FleetConsensus);
        let h = fleet_view(&ev, &env, 0, Some(FleetState::Seats { held: 1 }));
        let seat = SeatClaim::new("k", "ae-seat/x/y/1", 1, 7, 2_600_000);
        let input = HistoryInput::of(&h).with_seats(vec![seat.clone()]);
        let back: HistoryInput = serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        assert_eq!(back.scope, Some(HistoryScope::FleetConsensus), "a fleet decision names its fleet scope (finding 7)");
        assert_eq!(back.fleet.len(), 1);
        assert_eq!(back.fleet[0].counter.scope, HistoryScope::FleetConsensus);
        assert_eq!(back.fleet[0].state, FleetState::Seats { held: 1 });
        assert_eq!(back.seats, vec![seat]);
        let plain = HistoryInput::of(&history(2_000_000, Vec::new(), true));
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("fleet") && !json.contains("seats"), "{json}");
        // The labels are the evidence's wire form.
        assert_eq!(serde_json::to_string(&HistoryScope::FleetConsensus).unwrap(), "\"fleet_consensus\"");
        assert_eq!(serde_json::to_string(&HistoryScope::FleetTally).unwrap(), "\"fleet_tally\"");
        assert_eq!(HistoryScope::FleetTally.label(), "fleet_tally");
    }

    // ── AE1: the authenticated action envelope binds a scoped mandate ───────────────────────────
    //
    // AE-T decided at the gateway with no mandate in the picture: a rule naming `mandate` was a
    // fact the evaluator *could not establish*, so every such clause answered `Indeterminate`.
    // AE1 puts the enforcement point's finding in the envelope so the clause can be decided, and
    // the interesting half is what must NOT become a permit.

    fn principal(name: &str) -> crate::mandate::PrincipalId {
        crate::mandate::PrincipalId::new(name).expect("a valid principal")
    }
    fn term(name: &str) -> crate::mandate::TermId {
        crate::mandate::TermId::new(name).expect("a valid term")
    }

    /// `depot` holds a live mandate over `depot-ops` enumerating this operation.
    fn with_mandate(state: MandateState, epoch: u64) -> ActionEnvelope {
        ActionEnvelope::builder("oidc:idp/dispatcher", node(), "tools/call", "tool:reroute@node-1")
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .arguments_digest(digest_of(b"{}"))
            .mapping(ActionMapping::mapped("cat-procurement", "7"))
            .validity(1_000, 61_000)
            .mandate(MandateBinding {
                holder: principal("depot-dispatcher"),
                term:   term("term-2026-q3"),
                scope:  "depot-ops".to_string(),
                epoch,
                state,
            })
            .build()
    }

    fn mandated_allowance() -> ReferenceEvaluator {
        ReferenceEvaluator::new("rev-1").allow(
            Rule::new("oidc:idp/dispatcher", "tools/call", "tool:reroute@node-1")
                .requiring_scopes(["mcp:invoke"])
                .requiring_mandate("depot-ops"),
        )
    }

    /// The decisive AE1 case: a **superseded** mandate cannot be laundered into a permit by a
    /// policy that would otherwise allow the action.
    ///
    /// The allowance below matches the actor, the operation, the resource and the scopes — it is
    /// exactly the rule that permits this call every other day. The mandate fence runs first, so
    /// the answer is `Deny` and it carries the refusal's own name. Item 5 refuses to let
    /// `Superseded` become a `Conflict` because a retry loop would launder a revocation; the same
    /// reasoning forbids a policy engine laundering it, which is why the fence is not a policy
    /// input.
    ///
    /// What this does NOT prove: nothing here shows the gateway *has* a fence to consult. It does
    /// not — it binds no mandate (see `http/authz.rs`), so this path is reached by a resource-side
    /// enforcement point, which is AE2.
    #[test]
    fn a_superseded_mandate_is_denied_even_where_policy_would_allow() {
        let refusal = MandateRefusal::Superseded { installed: 9, presented: 7 };
        let env = with_mandate(MandateState::Refused(refusal), 7);

        // Sanity: the very same action with a live mandate is permitted, so the deny below is
        // about the mandate and nothing else.
        let permitted = mandated_allowance().evaluate(&with_mandate(MandateState::Established, 9));
        assert_eq!(permitted.verdict, Verdict::Permit, "the allowance must permit a live mandate");
        assert!(
            permitted.checked.iter().any(|c| c.contains("term=term-2026-q3") && c.contains("epoch=9")),
            "evidence must name WHICH appointment was relied on, got {:?}",
            permitted.checked,
        );

        let d = mandated_allowance().evaluate(&env);
        assert_eq!(d.verdict, Verdict::Deny, "a superseded mandate must not be permitted");
        assert!(
            d.reason.contains("superseded") || d.reason.contains("Superseded"),
            "the refusal must travel by name, got {:?}",
            d.reason,
        );
        assert!(
            d.reason.contains('9') && d.reason.contains('7'),
            "and name both epochs, so the operator fixes the right thing: {:?}",
            d.reason,
        );
    }

    /// The other three fence refusals deny too, each by its own name.
    #[test]
    fn every_mandate_refusal_denies_by_its_own_name() {
        let cases = [
            (MandateRefusal::NotEnumerated { operation: "tools/call".into() }, "enumerate"),
            (MandateRefusal::OutOfWindow { now_ms: 5_000, valid_until_ms: 4_000 }, "window"),
            (MandateRefusal::WrongScope { resource: "depot-ops".into(), mandate: "kitchen-ops".into() }, "scope"),
        ];
        for (refusal, needle) in cases {
            let d = mandated_allowance().evaluate(&with_mandate(MandateState::Refused(refusal.clone()), 9));
            assert_eq!(d.verdict, Verdict::Deny, "{refusal:?} must deny");
            assert!(
                d.reason.to_lowercase().contains(needle),
                "{refusal:?} must be reported as itself; got {:?}",
                d.reason,
            );
        }
    }

    /// **Missing facts.** A mandate the enforcement point could not check is *authority not
    /// established* — `Indeterminate`, never `Deny`.
    ///
    /// This is AE0 §9's rule applied to item 5: an absent fact is not a finding of wrongdoing, and
    /// evidence that recorded it as a denial would read as drift that never happened.
    #[test]
    fn a_mandate_that_could_not_be_checked_is_indeterminate_not_denied() {
        let env = with_mandate(MandateState::Unknown("the fence was unreachable".into()), 9);
        let d = mandated_allowance().evaluate(&env);
        assert_eq!(
            d.verdict,
            Verdict::Indeterminate,
            "an uncheckable mandate is not established; it is not a denial",
        );
        assert!(
            d.errors.iter().any(|e| e.contains("mandate not established") && e.contains("unreachable")),
            "and the reason it could not be established must survive: {:?}",
            d.errors,
        );
        assert!(
            d.errors.iter().any(|e| e.contains("term-2026-q3")),
            "naming the appointment, so an operator knows which one to look at: {:?}",
            d.errors,
        );
    }

    /// An action claiming **no** mandate against a rule that needs one is likewise indeterminate —
    /// and is a different record from one whose mandate failed.
    #[test]
    fn claiming_no_mandate_is_not_the_same_record_as_a_failed_one() {
        let no_claim = ActionEnvelope::builder("oidc:idp/dispatcher", node(), "tools/call", "tool:reroute@node-1")
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .arguments_digest(digest_of(b"{}"))
            .mapping(ActionMapping::mapped("cat-procurement", "7"))
            .validity(1_000, 61_000)
            .build();
        let absent = mandated_allowance().evaluate(&no_claim);
        assert_eq!(absent.verdict, Verdict::Indeterminate);

        let failed = mandated_allowance()
            .evaluate(&with_mandate(MandateState::Refused(MandateRefusal::Superseded { installed: 9, presented: 7 }), 7));
        assert_eq!(failed.verdict, Verdict::Deny);

        assert_ne!(
            absent.verdict, failed.verdict,
            "claims none and claimed-one-that-failed must not collapse into one answer",
        );
    }

    /// **Impersonation.** A mandate binding is about its holder, but the *actor* is item 7's
    /// verified principal — a caller cannot reach another principal's allowance by presenting that
    /// principal's mandate, because the rule still matches on actor.
    #[test]
    fn a_mandate_does_not_carry_another_principals_authority() {
        let impostor = ActionEnvelope::builder("oidc:idp/intruder", node(), "tools/call", "tool:reroute@node-1")
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .arguments_digest(digest_of(b"{}"))
            .mapping(ActionMapping::mapped("cat-procurement", "7"))
            .validity(1_000, 61_000)
            .mandate(MandateBinding::established(
                principal("depot-dispatcher"),
                term("term-2026-q3"),
                "depot-ops",
                9,
            ))
            .build();
        let d = mandated_allowance().evaluate(&impostor);
        assert_eq!(
            d.verdict,
            Verdict::Indeterminate,
            "holding someone else's mandate must not match their allowance",
        );
    }

    /// A mandate for a **different scope** does not satisfy a rule scoped elsewhere: it is a
    /// different appointment, so the rule does not apply and authority is not established.
    #[test]
    fn a_mandate_for_another_scope_does_not_satisfy_the_rule() {
        let elsewhere = ActionEnvelope::builder("oidc:idp/dispatcher", node(), "tools/call", "tool:reroute@node-1")
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .arguments_digest(digest_of(b"{}"))
            .mapping(ActionMapping::mapped("cat-procurement", "7"))
            .validity(1_000, 61_000)
            .mandate(MandateBinding::established(
                principal("depot-dispatcher"),
                term("term-2026-q3"),
                "kitchen-ops",
                9,
            ))
            .build();
        let d = mandated_allowance().evaluate(&elsewhere);
        assert_eq!(d.verdict, Verdict::Indeterminate);
    }

    /// A **prohibition** is not softened by a live mandate: holding an appointment is not licence
    /// to do a thing the policy forbids.
    #[test]
    fn a_live_mandate_does_not_override_a_prohibition() {
        let e = ReferenceEvaluator::new("rev-1")
            .prohibit(Rule::new("*", "tools/call", "tool:reroute@node-1"))
            .allow(
                Rule::new("oidc:idp/dispatcher", "tools/call", "tool:reroute@node-1")
                    .requiring_mandate("depot-ops"),
            );
        let d = e.evaluate(&with_mandate(MandateState::Established, 9));
        assert_eq!(d.verdict, Verdict::Deny, "a prohibition still wins over a mandated allowance");
    }


    // ── Corrections (AE3) ───────────────────────────────────────────────────────────────

    /// **A correction supersedes; it never edits.**
    ///
    /// AE0 §5 requires every record to carry correction/supersession references, and the reason is
    /// the same one that makes evidence append-only in the first place: *a record that could be
    /// revised in place could be revised after someone read it*. So an observation that turns out
    /// wrong is corrected by a **new record citing the old one's content hash**, and the old record
    /// stays exactly as it was.
    ///
    /// The hash is the citation rather than a name, because a correction that pointed at "the last
    /// execution record for this attempt" would be ambiguous the moment there were two — which is
    /// precisely when a correction exists.
    #[test]
    fn a_correction_cites_the_record_it_supersedes_and_leaves_it_intact() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:pay@n1");
        let first = AeEvidence::for_decision(
            &env,
            &Decision::permit("ok", "rev-1"),
            Execution::Completed,
            "depot-resource",
        );
        assert_eq!(first.supersedes, None, "an original cites nothing");

        // The observer later establishes that it did not complete after all.
        let hash = "a1b2c3";
        let correction = AeEvidence::for_decision(
            &env,
            &Decision::permit("ok", "rev-1"),
            Execution::Failed,
            "depot-resource",
        )
        .correcting(hash);

        assert_eq!(correction.supersedes.as_deref(), Some(hash));
        assert!(correction.is_correction());
        assert!(!first.is_correction());

        // The original is untouched — it still says what it said.
        assert_eq!(first.execution, Execution::Completed);
        assert_eq!(first.supersedes, None);
    }

    /// A correction is **not** a licence to change the action it describes.
    ///
    /// Correcting an *observation* means the outcome was misread. If the subject, the operation or
    /// the identities move too, this is not a correction of that record — it is a different action
    /// wearing a citation, and a reader that accepted it would let one attempt's evidence be
    /// replaced by another's.
    #[test]
    fn a_correction_must_describe_the_same_action_it_corrects() {
        let env = envelope("oidc:idp/alice", "tools/call", "tool:pay@n1");
        let original = AeEvidence::for_decision(
            &env,
            &Decision::permit("ok", "rev-1"),
            Execution::Completed,
            "depot-resource",
        );

        let honest = AeEvidence::for_decision(
            &env,
            &Decision::permit("ok", "rev-1"),
            Execution::Failed,
            "depot-resource",
        )
        .correcting("a1b2c3");
        assert!(honest.corrects_the_same_action_as(&original));

        let other_env = envelope("oidc:idp/mallory", "tools/call", "tool:pay@n1");
        let impostor = AeEvidence::for_decision(
            &other_env,
            &Decision::permit("ok", "rev-1"),
            Execution::Failed,
            "depot-resource",
        )
        .correcting("a1b2c3");
        assert!(
            !impostor.corrects_the_same_action_as(&original),
            "a different subject is a different action, whatever it cites",
        );

        let other_op = envelope("oidc:idp/alice", "tools/call", "tool:refund@n1");
        let wrong_resource = AeEvidence::for_decision(
            &other_op,
            &Decision::permit("ok", "rev-1"),
            Execution::Failed,
            "depot-resource",
        )
        .correcting("a1b2c3");
        assert!(!wrong_resource.corrects_the_same_action_as(&original));
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

    /// **Both enforcement points describe a refusal the same way.**
    ///
    /// `/mcp` and `/a2a` are two doors into the same slice, and they had drifted: MCP sent
    /// `reason`, `policy_revision`, `checked` and `errors`; A2A sent a number and an English
    /// sentence. So the path a **partner domain** calls across the federation edge was the poorer
    /// one, and a consumer that wrote refusal handling against MCP broke on A2A.
    ///
    /// The fix is that both call `error_data`, and this pins what it must carry. `policy_revision`
    /// is the field that earns its place: without it a caller cannot tell a **stale-policy**
    /// refusal from a real denial, and those call for opposite responses — redeploy and retry,
    /// versus stop.
    #[test]
    fn a_refusal_carries_the_same_machine_readable_data_whichever_door_it_came_through() {
        let ev = evaluator(
            ReferenceEvaluator::new("rev-1")
                .allow(Rule::new("*", "*", "*"))
                .prohibit(Rule::new("oidc:idp/mallory", "tools/call", "*")),
        );
        let env = envelope("oidc:idp/mallory", "tools/call", "tool:square@n1");
        let refusal = preflight(Some(&ev), &env, 2_000).expect_err("denied");

        let data = refusal.error_data();
        assert_eq!(data["reason"], "action_denied", "machine-readable, not prose: {data}");
        assert_eq!(data["policy_revision"], "rev-1", "which artifact decided: {data}");
        assert!(data["checked"].is_array(), "what it checked: {data}");
        assert!(data["errors"].is_array(), "what it could not evaluate: {data}");

        // The three refusals stay distinguishable — this is the collapse the slice exists to
        // prevent, and it has to survive the trip to a caller, not only live in the Rust type.
        let unestablished = {
            let ev = evaluator(ReferenceEvaluator::new("rev-1").allow(Rule::new(
                "oidc:idp/alice",
                "tools/call",
                "tool:square@n1",
            )));
            let env = envelope("oidc:idp/alice", "tools/call", "tool:cube@n1");
            preflight(Some(&ev), &env, 2_000).expect_err("refused")
        };
        assert_eq!(unestablished.error_data()["reason"], "authority_not_established");
        assert_ne!(
            data["reason"], unestablished.error_data()["reason"],
            "a denial and an unestablished authority must not arrive as the same thing",
        );
    }

    /// **What an enforcement point loses by calling `evaluate` instead of `preflight`.**
    ///
    /// `preflight` is public so a resource can enforce at its own boundary. This is why it must be
    /// the thing it calls: for each case below the evaluator, asked directly, returns a **permit** —
    /// correctly, on its own terms — and the seam refuses. An adapter is not wrong to permit here;
    /// it simply is not being asked the question the seam asks.
    ///
    /// Written as a *difference* rather than as "preflight refuses", because the refusal alone
    /// would pass even if `evaluate` refused too, and then the test would prove nothing about which
    /// layer is carrying the guarantee.
    #[test]
    fn calling_the_evaluator_directly_skips_what_the_seam_enforces() {
        // 1. An expired envelope. The evaluator holds no clock and cannot know.
        {
            let ev = evaluator(ReferenceEvaluator::new("rev-1").allow(Rule::new("*", "*", "*")));
            let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
            assert_eq!(
                ev.evaluate(&env).verdict,
                Verdict::Permit,
                "the evaluator permits — it has no clock, and that is not its job",
            );
            let refused = preflight(Some(&ev), &env, 61_001).expect_err("the seam denies");
            assert!(matches!(refused, PreflightRefusal::Denied(_)));
        }

        // 2. A decision from a policy revision this enforcement point never deployed.
        {
            let ev = evaluator(ReferenceEvaluator::new("rev-2").allow(Rule::new("*", "*", "*")));
            let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1"); // expects rev-1
            assert_eq!(ev.evaluate(&env).verdict, Verdict::Permit);
            let refused = preflight(Some(&ev), &env, 2_000).expect_err("stale policy");
            assert_eq!(refused.decision().verdict, Verdict::Indeterminate, "never a denial");
        }

        // 3. An operation outside the reviewed catalogue.
        {
            let ev = evaluator(ReferenceEvaluator::new("rev-1").allow(Rule::new("*", "*", "*")));
            let mut env = envelope("oidc:idp/alice", "tools/call", "tool:execute_shell@n1");
            env.mapping = ActionMapping::unmapped("cat-procurement", "7");
            assert_eq!(ev.evaluate(&env).verdict, Verdict::Permit);
            assert!(preflight(Some(&ev), &env, 2_000).is_err(), "a permit cannot name it");
        }

        // 4. A permit carrying evaluation errors — a contradiction the seam resolves.
        {
            struct Sloppy;
            impl ActionEvaluator for Sloppy {
                fn evaluate(&self, _: &ActionEnvelope) -> Decision {
                    Decision::permit("allowed", "rev-1").with_errors(["clause 3 did not evaluate"])
                }
            }
            let ev: Arc<dyn ActionEvaluator> = Arc::new(Sloppy);
            let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
            assert_eq!(ev.evaluate(&env).verdict, Verdict::Permit);
            let refused = preflight(Some(&ev), &env, 2_000).expect_err("downgraded");
            assert_eq!(refused.decision().verdict, Verdict::Indeterminate);
        }

        // 5. No evaluator at all is inert — an enforcement point with none proceeds as before,
        //    rather than failing closed on a node that never opted in.
        let env = envelope("oidc:idp/alice", "tools/call", "tool:square@n1");
        assert_eq!(preflight(None, &env, 2_000), Ok(None));
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

#[cfg(test)]
mod a1_policy_tests {
    use super::{ReferenceEvaluator, Rule};

    /// An allowance permitting a protected operation without a mandate is reported — including one
    /// that reaches it through `*` — and one that requires a mandate is not.
    #[test]
    fn allowances_that_permit_protected_work_without_a_mandate_are_reported() {
        let e = ReferenceEvaluator::new("r1")
            .allow(Rule::new("*", "tools/call", "deploy").requiring_mandate("cap/fleet"))
            .allow(Rule::new("*", "*", "deploy"))
            .allow(Rule::new("*", "tools/call", "search"));
        let holes = e.allowances_without_mandate(&[("tools/call", "deploy")]);
        assert_eq!(holes.len(), 1, "only the wildcard allowance is a hole: {holes:?}");
        assert_eq!(holes[0].operation, "*");
        assert!(ReferenceEvaluator::new("r2")
            .allow(Rule::new("*", "tools/call", "deploy").requiring_mandate("cap/fleet"))
            .allowances_without_mandate(&[("tools/call", "deploy")])
            .is_empty());
    }
}


#[cfg(test)]
mod destination_refusal_tests {
    use super::*;

    /// A destination's refusal of a composed effect is a `Decided` record at enforcement point
    /// `destination`, denied, with nothing run — so a reader of the journal sees which point said
    /// no, after the gateway's own permit (`composed-effect.md` §9).
    #[test]
    fn a_destination_refusal_is_a_denied_decision_at_the_destination_with_nothing_run() {
        let via = NodeId::new("127.0.0.1", 7946).unwrap();
        let envelope = ActionEnvelope::builder("worker-a", via, "tools/call", "tool:ledger@depot")
            .identities("op-1", "attempt-1")
            .expected_policy_revision("rev-9")
            .validity(1_000, 2_000)
            .build();
        let e = AeEvidence::for_destination_refusal(&envelope, "authority", "mandate superseded: installed 4, presented 3");
        assert_eq!(e.kind, RecordKind::Decided);
        assert_eq!(e.enforcement_point, "destination");
        assert_eq!(e.decision, DecisionKind::Deny);
        assert_eq!(e.execution, Execution::None);
        assert_eq!(e.subject, "worker-a");
        assert_eq!(e.policy_revision, "rev-9");
        assert_eq!(e.at_ms, 1_000, "the envelope's own time, carried");
    }
}

/// Post-360 row R — inference routing as policy: the evaluator decides on the destination's locality
/// and the request's data class.
#[cfg(test)]
mod route_policy_tests {
    use super::*;
    use crate::agent::route_facts::{ClassSource, RefusedDestination};

    fn routed(class: Option<&str>, locality: Option<Locality>) -> ActionEnvelope {
        let via = NodeId::new("127.0.0.1", 9000).unwrap();
        let mut b = ActionEnvelope::builder("token:gw/alice", via, "llm.invoke", "prompt:llm/m@127.0.0.1:9001")
            .identities("op-r", "op-r/1")
            .scopes(["mesh:write"])
            .mapping(ActionMapping::mapped("cat", "1"))
            .expected_policy_revision("rev-r")
            .validity(1_000, 61_000);
        if let Some(c) = class {
            b = b.data_class(DataClass::new(c, ClassSource::Declared));
        }
        if let Some(l) = locality {
            b = b.destination(Destination::new("llm/m@127.0.0.1:9001", l));
        }
        b.build()
    }

    /// *Confidential may only go to in-domain models; public may go anywhere* — the prohibition form.
    fn confidential_stays_home() -> Arc<dyn ActionEvaluator> {
        Arc::new(
            ReferenceEvaluator::new("rev-r")
                .allow(Rule::new("*", "llm.invoke", "*"))
                .prohibit(Rule::new("*", "llm.invoke", "*").for_class("confidential").at_locality(Locality::External)),
        )
    }

    #[test]
    fn a_confidential_request_to_an_external_destination_is_denied_naming_class_and_locality() {
        let r = preflight(Some(&confidential_stays_home()), &routed(Some("confidential"), Some(Locality::External)), 2_000);
        let Err(PreflightRefusal::Denied(d)) = r else { panic!("an authority decided no: {r:?}") };
        let said = format!("{} {}", d.reason, d.checked.join(" "));
        assert!(said.contains("confidential") && said.contains("external"), "{said}");
    }

    #[test]
    fn a_confidential_request_to_an_in_domain_destination_is_permitted() {
        let r = preflight(Some(&confidential_stays_home()), &routed(Some("confidential"), Some(Locality::InDomain)), 2_000);
        assert!(matches!(r, Ok(Some(ref d)) if d.verdict == Verdict::Permit), "{r:?}");
    }

    #[test]
    fn a_public_request_may_go_to_an_external_destination() {
        let r = preflight(Some(&confidential_stays_home()), &routed(Some("public"), Some(Locality::External)), 2_000);
        assert!(matches!(r, Ok(Some(ref d)) if d.verdict == Verdict::Permit), "{r:?}");
    }

    /// A provider that declared no locality is not in-domain by default: a rule about locality cannot
    /// be decided on it, so the call is not established — refused, and the reason names locality.
    #[test]
    fn an_unknown_locality_is_not_established_never_in_domain() {
        for env in [routed(Some("confidential"), Some(Locality::Unknown)), routed(Some("confidential"), None)] {
            let r = preflight(Some(&confidential_stays_home()), &env, 2_000);
            let Err(PreflightRefusal::NotEstablished(d)) = r else { panic!("not established: {r:?}") };
            assert!(d.errors.join(" ").contains("locality not established"), "{d:?}");
        }
    }

    /// A door that does not classify — `/gateway/llm/call`, `/mcp` — cannot pass a class rule: the
    /// class is not established, so the rule decides nothing and the seam refuses. That is what keeps
    /// a policy written for the routing door from being walked around through another.
    #[test]
    fn a_request_with_no_class_cannot_pass_a_class_rule() {
        let r = preflight(Some(&confidential_stays_home()), &routed(None, Some(Locality::External)), 2_000);
        let Err(PreflightRefusal::NotEstablished(d)) = r else { panic!("not established: {r:?}") };
        assert!(d.errors.join(" ").contains("data class not established"), "{d:?}");
        // The allow-list form refuses an unclassified request too: no allowance matches.
        let allow_list: Arc<dyn ActionEvaluator> = Arc::new(
            ReferenceEvaluator::new("rev-r")
                .allow(Rule::new("*", "llm.invoke", "*").for_class("confidential").at_locality(Locality::InDomain))
                .allow(Rule::new("*", "llm.invoke", "*").for_class("public")),
        );
        let r = preflight(Some(&allow_list), &routed(None, Some(Locality::InDomain)), 2_000);
        assert!(matches!(r, Err(PreflightRefusal::NotEstablished(_))), "{r:?}");
        let r = preflight(Some(&allow_list), &routed(Some("confidential"), Some(Locality::External)), 2_000);
        assert!(matches!(r, Err(PreflightRefusal::NotEstablished(_))), "no allowance covers it: {r:?}");
        let r = preflight(Some(&allow_list), &routed(Some("confidential"), Some(Locality::InDomain)), 2_000);
        assert!(matches!(r, Ok(Some(_))), "{r:?}");
    }

    /// A policy with no class or locality clause decides exactly as before on a routed envelope.
    #[test]
    fn a_rule_without_class_or_locality_ignores_both() {
        let ev: Arc<dyn ActionEvaluator> =
            Arc::new(ReferenceEvaluator::new("rev-r").allow(Rule::new("*", "llm.invoke", "*")));
        for env in [routed(None, None), routed(Some("confidential"), Some(Locality::External))] {
            assert!(matches!(preflight(Some(&ev), &env, 2_000), Ok(Some(_))));
        }
    }

    /// The evidence names the class and who declared it, the destination decided on with its
    /// locality, and the destinations refused before it; a record without a route serialises as before.
    #[test]
    fn the_route_is_on_the_evidence_record() {
        let env = routed(Some("confidential"), Some(Locality::InDomain));
        let d = Decision::permit("ok", "rev-r");
        let plain = AeEvidence::for_decision(&env, &d, Execution::Attempted, "gateway:reason/route");
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("route").is_none(), "absent, not null: {wire}");

        let refused = RefusedDestination::new(
            Destination::new("llm/m@127.0.0.1:9002", Locality::External),
            "action_denied",
        );
        let route = RouteFacts::new(Destination::new("llm/m@127.0.0.1:9001", Locality::InDomain))
            .with_data_class(DataClass::new("confidential", ClassSource::Principal))
            .with_refused(vec![refused]);
        let rec = plain.with_route(route);
        assert_eq!(rec.destination.as_deref(), Some("llm/m@127.0.0.1:9001"));
        let wire = serde_json::to_value(&rec).unwrap();
        assert_eq!(wire["route"]["data_class"]["class"], "confidential");
        assert_eq!(wire["route"]["data_class"]["source"], "principal");
        assert_eq!(wire["route"]["destination"]["locality"], "in_domain");
        assert_eq!(wire["route"]["refused"][0]["destination"]["name"], "llm/m@127.0.0.1:9002");
        assert_eq!(wire["route"]["refused"][0]["destination"]["locality"], "external");
        assert_eq!(wire["route"]["refused"][0]["reason"], "action_denied");
        let back: AeEvidence = serde_json::from_value(wire).unwrap();
        assert_eq!(back, rec, "round-trips");
        // The execution record for the same attempt names where it went.
        assert_eq!(rec.as_execution(Execution::Completed).route, rec.route);
    }

    #[test]
    fn a_declared_locality_is_read_strictly() {
        assert_eq!(Locality::parse("in_domain"), Locality::InDomain);
        assert_eq!(Locality::parse("external"), Locality::External);
        for s in ["", "In_Domain", "internal", "local"] {
            assert_eq!(Locality::parse(s), Locality::Unknown, "{s:?}");
        }
        for l in [Locality::InDomain, Locality::External, Locality::Unknown] {
            assert_eq!(serde_json::to_value(l).unwrap(), l.label());
        }
    }
}
