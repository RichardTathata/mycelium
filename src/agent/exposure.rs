//! **The causal exposure policy** (adopter-surface Y5, D18): a `[[rule]]` whose input is an **unresolved closure** — the
//! external effects whose outcome is not known, and the work that depends on them — and which may hold only the new
//! work that would **extend** such a closure, never the work that resolves it and never work independent of it.
//!
//! **The input is the request's causal cone** (X4, X9). A request names what it depends on with its causal fields:
//! `parent`, the hash of the record it acts on, and `correlation`, its run. It **extends** an unresolved closure when
//! its `parent` is an unresolved effect's record — its prepared record's hash or any execution evidence hash it has had
//! (a probe writes a new one; the earlier ones still name it) — or its `correlation` is that effect's run. A request
//! that names neither, or names records that have resolved, is **independent** and passes, whoever sends it.
//!
//! **The omission limit.** A request carrying neither `_meta.parent` nor `_meta.correlation` is a root and is never
//! held: the policy bounds causal extension *as each envelope declares it*, so it binds only callers that propagate
//! their lineage. A cap on what an actor does whatever it declares is row K2's history rules.
//!
//! **Where the causal fields reach.** At the gateway's doors the request's own `_meta` is read. At the provider check
//! (`provider_enforcement::check`) they are read where the kind's payload carries them: `mcp.invoke`, a JSON-RPC
//! `tools/call` whose `params._meta` holds them. A `skill.invoke` payload is the skill's text, and an operator's
//! protected kind's is opaque, so neither carries any to the provider — which reads such a call as a root and never
//! holds it on exposure; the door that dispatched it (`/a2a` for a skill) decided on the request's own `_meta`. The
//! caller envelope carries no causal fields by design (X4). The cone is followed as far as those two fields name it: a request
//! whose parent is a *descendant* of an unresolved effect, in another run and naming no unresolved record itself, is
//! not followed through the evidence journal — stated, not built. Two numbers are read from the closure a request
//! extends: its **size** (the unresolved effects it names) and the **age of its oldest record**. They are separate
//! thresholds because the second grows while nothing is done.
//!
//! **What it may hold — the stability record's decisive rule, applied unchanged** (`docs/design/adaptive-stability.md`
//! §2). It holds **new dependent work**: a call admitted at one of the gateway's doors (`/mcp`, `/a2a`, the LLM doors,
//! the federation route, the provider check) that would extend an unresolved closure. It **never** holds what resolves
//! the closure or protects the node, and this is structural rather than a rule an operator could misconfigure:
//!
//! - a **reconciliation probe** goes through no evaluator at all (`Reconciler::probe`);
//! - a **compensation** is decided through [`preflight`](super::action_evaluator::preflight) by the reconciler
//!   (`CompensationPermit::decide`), which never reaches this gate — the gate is in the gateway's decision
//!   (`http::ae_decide`), not in the shared seam;
//! - a **protective shed** and a **rescue from zero** are governor actions, never envelopes;
//! - and at a door, an operation in [`EXEMPT_OPERATIONS`] — named exactly, not by prefix — is
//!   [`WorkClass::Exempt`] whatever a rule selects. `effect.apply` and `effect.retry` are not in it: they extend a
//!   closure.
//!
//! **How a closure ends.** A probe that finds an *unknown* effect's fate resolves it. An effect escalated *unresolved*
//! (its probe budget spent, or left as irreversible and unobservable) has no exit: it is terminal, a compensation
//! takes only a committed effect, and no operator route resolves one (*not built*). Such an effect holds only the
//! work that names it — its own cone — for good; work outside the cone is never held.
//!
//! **Counted as K2 counts history.** A rule names its scope: [`ExposureScope::EnforcementPoint`] — this node's attached
//! source, which a rule at this scope requires (no source is *not established*, never clear) — or
//! [`ExposureScope::FleetTally`] — this node's source, if it has one, plus every node's published summary
//! (`sys/ae-exposure/{node}`), approximate, under row K2's coverage rules (summaries are unsigned, as K2's tally
//! entries are: a member can write another's — counted by the `sys/` tripwire at the owner — and a forged summary
//! for an owner **out of view** that marks it `departed` holds the digests it lists fleet-wide until the history
//! horizon collects it): this node's view of the fleet under the
//! governors' confidence bound; a peer with no summary, an unreadable, stale, future-dated or truncated one is
//! **unknown coverage**, and unknown coverage **fails closed** (`Indeterminate`, never a zero). The size bound is
//! **approximate under concurrency** at both scopes: two requests read the closure before either's own effect is
//! prepared, so both pass a size threshold one of them would have reached. **Not built:** `HistoryScope::FleetConsensus`
//! — an exact bound would need a seat held until the effect *resolves*, which no lease of a fixed window expresses; a
//! rule naming it is refused at load, by name.
//!
//! **Shadow first, under the profile ladder** (`docs/operations/control-profiles.md`). Under `legacy` the policy is
//! neither read nor counted — the rung that touches nothing. Under `observe` it is decided and counted —
//! [`ExposureCounts::would_hold`] — and holds nothing. Under `enforce-local` and `enforce-allocated` it holds. A request
//! is counted only when the evaluator **permitted** it: a call policy refused is not one exposure held.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// The operations an exposure rule never holds at a door, whatever it selects: the reconciler's moves that resolve a
/// closure (a probe, a compensation, a leave) and a grant's issue and revocation, which authorise and run nothing. Named
/// exactly: `effect.apply` and `effect.retry` extend a closure and are dependent work. A `protected_rpc_kinds` entry
/// named here is refused at start, since a provider checks a protected kind under its own name.
pub const EXEMPT_OPERATIONS: &[&str] = &["effect.probe", "effect.compensate", "effect.leave", "grant.issue", "grant.revoke"];

/// Where an exposure rule reads the closure — the two of row K2's history scopes this policy counts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureScope {
    /// This node's attached source.
    EnforcementPoint,
    /// This node's source plus every node's published summary — approximate, unknown coverage failing closed.
    FleetTally,
}

impl ExposureScope {
    /// The scope's name, as `HistoryScope::label` spells the same two.
    pub fn label(&self) -> &'static str {
        match self {
            ExposureScope::EnforcementPoint => "enforcement_point",
            ExposureScope::FleetTally => "fleet_tally",
        }
    }
}

/// What a request names of what it depends on — its causal fields (X4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClosureKey<'a> {
    /// The hash of the record it acts on.
    pub parent: Option<&'a str>,
    /// Its run.
    pub correlation: Option<&'a str>,
}

impl ClosureKey<'_> {
    /// Names nothing it depends on: a root, independent of every closure.
    pub fn is_root(&self) -> bool {
        self.parent.is_none() && self.correlation.is_none()
    }
}

/// One effect whose outcome is not known, by every name a request's causal fields can give it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedRecord {
    /// The prepared record's hash.
    pub record_hash: String,
    /// Every execution evidence hash it has had, the latest last, when an evidence journal is attached — a probe or a
    /// retry writes a new one, and a request naming an earlier one still names this record.
    pub evidence_hashes: Vec<String>,
    /// The run it belongs to.
    pub correlation: Option<String>,
    /// When it was prepared, epoch milliseconds.
    pub prepared_at_ms: u64,
}

impl UnresolvedRecord {
    /// Does a request naming `key` extend this effect's closure?
    pub fn extended_by(&self, key: &ClosureKey<'_>) -> bool {
        key.parent.is_some_and(|p| p == self.record_hash || self.evidence_hashes.iter().any(|e| e == p))
            || key.correlation.is_some_and(|c| Some(c) == self.correlation.as_deref())
    }
}

/// How large and how old the closure a request extends is: the unresolved effects it names, and when the oldest was
/// prepared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureExposure {
    /// Effects whose outcome is not known.
    pub unresolved: u32,
    /// When the oldest of them was prepared, epoch milliseconds; `None` when there is none.
    pub oldest_at_ms: Option<u64>,
}

impl ClosureExposure {
    /// The two summed: sizes add, the oldest is the earliest.
    pub fn plus(self, other: ClosureExposure) -> ClosureExposure {
        ClosureExposure {
            unresolved: self.unresolved.saturating_add(other.unresolved),
            oldest_at_ms: match (self.oldest_at_ms, other.oldest_at_ms) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }

    fn one(prepared_at_ms: u64) -> ClosureExposure {
        ClosureExposure { unresolved: 1, oldest_at_ms: Some(prepared_at_ms) }
    }
}

/// What holds the effects a closure is read from — the external-effect reconciler
/// (`mycelium::effects::external::Reconciler`) implements it; attach one with `GossipAgent::with_exposure_source`. **A
/// source that cannot read its records says so** (`Err`), and a decision on it is not established — never a zero.
pub trait ExposureSource: Send + Sync + 'static {
    /// Every effect this source holds whose outcome is not known.
    fn unresolved(&self) -> Result<Vec<UnresolvedRecord>, String>;

    /// The closure a request naming `key` extends, as this source holds it.
    fn exposure_of(&self, key: &ClosureKey<'_>) -> Result<ClosureExposure, String> {
        Ok(self
            .unresolved()?
            .iter()
            .filter(|r| r.extended_by(key))
            .fold(ClosureExposure::default(), |e, r| e.plus(ClosureExposure::one(r.prepared_at_ms))))
    }
}

/// The thresholds of one exposure rule. At least one is set; `max_unresolved` is at least 1 (zero would hold every
/// dependent call, whatever its closure holds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExposureLimit {
    /// Hold new dependent work once the closure it extends holds this many unresolved effects.
    pub max_unresolved: Option<u32>,
    /// Hold it once that closure's oldest unresolved effect is older than this, in milliseconds.
    pub max_oldest_age_ms: Option<u64>,
    /// Where the closure is read: this point's source, or the fleet's tally.
    pub scope: ExposureScope,
}

/// One exposure rule: whom and what it selects (`*` is any, as the reference evaluator's rules), and its limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposureRule {
    pub actor: String,
    pub operation: String,
    pub resource: String,
    pub limit: ExposureLimit,
}

impl ExposureRule {
    fn selects(&self, actor: &str, operation: &str, resource: &str) -> bool {
        let any = |pat: &str, v: &str| pat == "*" || pat == v;
        any(&self.actor, actor) && any(&self.operation, operation) && any(&self.resource, resource)
    }

    /// A short name for the rule, as decisions and counters cite it.
    pub fn label(&self) -> String {
        format!("{}:{}:{}", self.actor, self.operation, self.resource)
    }
}

/// Why an exposure rule could not be built from its declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExposureRuleError {
    /// Neither threshold is set: the rule would hold nothing and look as though it guarded something.
    NoThreshold,
    /// `max_unresolved = 0` would hold every dependent call, whatever its closure holds.
    ZeroSize,
    /// `max_oldest_age_ms = 0` would hold every call extending a closure with an effect in flight.
    ZeroAge,
    /// A scope this policy does not count at.
    Scope(String),
}

impl std::fmt::Display for ExposureRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoThreshold => f.write_str("exposure: set max_unresolved, max_oldest_age_ms or both — a rule with neither holds nothing"),
            Self::ZeroSize => f.write_str("exposure.max_unresolved = 0 would hold every dependent call, whatever its closure holds; the least is 1"),
            Self::ZeroAge => f.write_str("exposure.max_oldest_age_ms = 0 would hold every call extending a closure with an effect in flight"),
            Self::Scope(s) => write!(
                f,
                "exposure.scope = {s:?}: the exposure policy counts at \"enforcement_point\" or \"fleet_tally\"; \"fleet_consensus\" is not built — an exact bound would need a seat held until the effect resolves, which no fixed lease expresses"
            ),
        }
    }
}

impl std::error::Error for ExposureRuleError {}

impl ExposureLimit {
    /// The limit, checked: at least one threshold, neither zero, a scope the policy counts at.
    pub fn new(max_unresolved: Option<u32>, max_oldest_age_ms: Option<u64>, scope: &str) -> Result<Self, ExposureRuleError> {
        if max_unresolved.is_none() && max_oldest_age_ms.is_none() {
            return Err(ExposureRuleError::NoThreshold);
        }
        if max_unresolved == Some(0) {
            return Err(ExposureRuleError::ZeroSize);
        }
        if max_oldest_age_ms == Some(0) {
            return Err(ExposureRuleError::ZeroAge);
        }
        let scope = match scope {
            "enforcement_point" => ExposureScope::EnforcementPoint,
            "fleet_tally" => ExposureScope::FleetTally,
            other => return Err(ExposureRuleError::Scope(other.to_string())),
        };
        Ok(Self { max_unresolved, max_oldest_age_ms, scope })
    }
}

/// The exposure rules a node enforces — built from the `[[rule]]`s that carry an `exposure` table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExposurePolicy {
    rules: Vec<ExposureRule>,
}

impl ExposurePolicy {
    /// A policy of `rules`.
    pub fn new(rules: Vec<ExposureRule>) -> Self {
        Self { rules }
    }

    /// The rules, in order.
    pub fn rules(&self) -> &[ExposureRule] {
        &self.rules
    }

    /// The rules that select this request — empty when none does.
    pub fn selecting(&self, actor: &str, operation: &str, resource: &str) -> Vec<&ExposureRule> {
        self.rules.iter().filter(|r| r.selects(actor, operation, resource)).collect()
    }
}

/// What a request's operation is, as the exposure policy sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClass {
    /// Work that may extend a closure: the policy may hold it, if its causal fields name an unresolved one.
    Dependent,
    /// Work the policy never holds, and why.
    Exempt(&'static str),
}

impl WorkClass {
    /// The class of `operation` at a door: exempt only when it is in [`EXEMPT_OPERATIONS`], by its exact name.
    pub fn of(operation: &str) -> WorkClass {
        match operation {
            "effect.probe" | "effect.compensate" | "effect.leave" => {
                WorkClass::Exempt("resolves a closure: a probe, a compensation or a leave is never held by exposure")
            }
            "grant.issue" | "grant.revoke" => WorkClass::Exempt("an authorisation, not work: it extends no closure"),
            _ => WorkClass::Dependent,
        }
    }
}

/// The exposure a rule decides on: the closure a request extends, read at the rule's scope, or why it could not be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// What was read, and from whom.
    View { exposure: ClosureExposure, scope: ExposureScope, publishers: Vec<String> },
    /// Not established, and why — unknown coverage, an unreadable source, no source attached.
    Unavailable(String),
}

/// What one rule decided on one exposure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Judgement {
    /// Inside both thresholds.
    Clear,
    /// A threshold is reached: new dependent work is held.
    Hold(String),
    /// The exposure could not be read: not established.
    NotEstablished(String),
}

/// Decide one rule on one exposure at `now_ms`. Pure: no clock, no draw. The size threshold is reached when the
/// closure already holds `max_unresolved` (the call would extend it past); the age threshold when its oldest record is
/// **older** than `max_oldest_age_ms`. Each fires on its own — an aged record holds with the size unmet.
pub fn judge(rule: &ExposureRule, exposure: &Exposure, now_ms: u64) -> Judgement {
    let (e, scope) = match exposure {
        Exposure::Unavailable(why) => return Judgement::NotEstablished(format!("the unresolved closure this request extends is not established: {why}")),
        Exposure::View { exposure, scope, .. } => (exposure, scope),
    };
    let at = scope.label();
    if let Some(max) = rule.limit.max_unresolved
        && e.unresolved >= max
    {
        return Judgement::Hold(format!(
            "exposure: the unresolved closure this request extends holds {} effect(s) at {at}, the rule's size threshold is {max} — new dependent work is held until the closure resolves",
            e.unresolved
        ));
    }
    if let (Some(max_age), Some(oldest)) = (rule.limit.max_oldest_age_ms, e.oldest_at_ms) {
        let age = now_ms.saturating_sub(oldest);
        if age > max_age {
            return Judgement::Hold(format!(
                "exposure: the oldest unresolved effect in the closure this request extends is {age} ms old at {at}, past the rule's age threshold of {max_age} ms ({} unresolved) — new dependent work is held until it resolves",
                e.unresolved
            ));
        }
    }
    Judgement::Clear
}

/// What the gate came to for one request. [`label`](Self::label) is the one set of names the rule catalogue's
/// `ae.exposure` reasons and the metric `mycelium_ae_exposure_total{outcome}` both use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// No rule selects it, none is attached, or the profile is `legacy`.
    Untouched,
    /// Selected but exempt: never held.
    Exempt(&'static str),
    /// Selected, and it extends no unresolved closure: independent work, never held.
    Independent,
    /// Selected, extends a closure, inside every threshold.
    Clear,
    /// In shadow, a threshold reached: an enforcing rung would have held it. Nothing is held.
    ShadowWouldHold(String),
    /// In shadow, the closure not established: an enforcing rung would have refused it. Nothing is held.
    ShadowWouldNotEstablish(String),
    /// Held: a threshold reached on an enforcing rung.
    Hold(String),
    /// Refused as not established on an enforcing rung.
    NotEstablished(String),
}

/// Every label [`Gate::label`] gives, in catalogue order: what the rule catalogue lists as `ae.exposure`'s reasons.
pub const GATE_LABELS: &[&str] = &["clear", "independent", "shadow_would_hold", "shadow_would_not_establish", "held", "not_established", "exempt"];

impl Gate {
    /// The outcome's name, or `None` for a request the policy did not touch.
    pub fn label(&self) -> Option<&'static str> {
        Some(match self {
            Gate::Untouched => return None,
            Gate::Exempt(_) => "exempt",
            Gate::Independent => "independent",
            Gate::Clear => "clear",
            Gate::ShadowWouldHold(_) => "shadow_would_hold",
            Gate::ShadowWouldNotEstablish(_) => "shadow_would_not_establish",
            Gate::Hold(_) => "held",
            Gate::NotEstablished(_) => "not_established",
        })
    }
}

/// The policy's own counts, from attachment, of the requests the evaluator **permitted**: what it judged, held and
/// would have held in shadow.
#[derive(Debug, Default)]
pub struct ExposureCounters {
    judged: AtomicU64,
    held: AtomicU64,
    would_hold: AtomicU64,
    not_established: AtomicU64,
    would_not_establish: AtomicU64,
    exempt: AtomicU64,
    independent: AtomicU64,
    collected: AtomicU64,
}

/// A snapshot of [`ExposureCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ExposureCounts {
    /// Permitted requests a rule selected that extend an unresolved closure, and were decided on it.
    pub judged: u64,
    /// Held on an enforcing rung.
    pub held: u64,
    /// In shadow, a threshold reached: an enforcing rung would have held it. Nothing was held.
    pub would_hold: u64,
    /// On an enforcing rung, the closure not established: refused as not established.
    pub not_established: u64,
    /// In shadow, the closure not established: an enforcing rung would have refused it. Nothing was held.
    pub would_not_establish: u64,
    /// Permitted requests a rule selected whose work is exempt (a probe, a compensation, a grant): never held.
    pub exempt: u64,
    /// Permitted requests a rule selected that extend no unresolved closure: never held.
    pub independent: u64,
    /// Other nodes' summaries this node collected (unreadable, forged, removed or past the horizon).
    pub collected: u64,
}

impl ExposureCounters {
    /// The counts now.
    pub fn snapshot(&self) -> ExposureCounts {
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        ExposureCounts {
            judged: l(&self.judged),
            held: l(&self.held),
            would_hold: l(&self.would_hold),
            not_established: l(&self.not_established),
            would_not_establish: l(&self.would_not_establish),
            exempt: l(&self.exempt),
            independent: l(&self.independent),
            collected: l(&self.collected),
        }
    }

    /// Count one gate the evaluator's permit met — the only gates counted, so a policy refusal counts nothing.
    pub fn record(&self, gate: &Gate) {
        let bump = |a: &AtomicU64| {
            a.fetch_add(1, Ordering::Relaxed);
        };
        match gate {
            Gate::Untouched => (),
            Gate::Exempt(_) => bump(&self.exempt),
            Gate::Independent => bump(&self.independent),
            Gate::Clear => bump(&self.judged),
            Gate::ShadowWouldHold(_) => {
                bump(&self.judged);
                bump(&self.would_hold);
            }
            Gate::ShadowWouldNotEstablish(_) => {
                bump(&self.judged);
                bump(&self.would_not_establish);
            }
            Gate::Hold(_) => {
                bump(&self.judged);
                bump(&self.held);
            }
            Gate::NotEstablished(_) => {
                bump(&self.judged);
                bump(&self.not_established);
            }
        }
        #[cfg(feature = "metrics")]
        if let Some(outcome) = gate.label() {
            metrics::counter!("mycelium_ae_exposure_total", "outcome" => outcome).increment(1);
        }
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn collected(&self, n: usize) {
        self.collected.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Decide the gate for a request naming `key` against `policy` on the exposure `read` gives for each scope a selecting
/// rule names, under `enforcing` (the node's profile enforces). Pure, and counts nothing: the caller counts a gate
/// only once the evaluator has permitted ([`ExposureCounters::record`]).
#[allow(clippy::too_many_arguments)]
pub fn gate(
    policy: &ExposurePolicy,
    actor: &str,
    operation: &str,
    resource: &str,
    key: &ClosureKey<'_>,
    enforcing: bool,
    now_ms: u64,
    mut read: impl FnMut(ExposureScope) -> Exposure,
) -> Gate {
    let rules = policy.selecting(actor, operation, resource);
    if rules.is_empty() {
        return Gate::Untouched;
    }
    if let WorkClass::Exempt(why) = WorkClass::of(operation) {
        return Gate::Exempt(why);
    }
    if key.is_root() {
        return Gate::Independent;
    }
    // One read per scope a rule names, so two rules at one scope decide on one closure.
    let mut read_at: Vec<(ExposureScope, Exposure)> = Vec::new();
    let mut first: Option<Judgement> = None;
    let mut extends = false;
    for rule in rules {
        let scope = rule.limit.scope;
        let exposure = match read_at.iter().find(|(s, _)| *s == scope) {
            Some((_, e)) => e.clone(),
            None => {
                let e = read(scope);
                read_at.push((scope, e.clone()));
                e
            }
        };
        extends |= !matches!(&exposure, Exposure::View { exposure, .. } if exposure.unresolved == 0);
        match judge(rule, &exposure, now_ms) {
            Judgement::Clear => {}
            j => {
                // A hold outranks not-established: a threshold read is a fact, unknown coverage elsewhere is not.
                let replace = matches!((&first, &j), (None, _) | (Some(Judgement::NotEstablished(_)), Judgement::Hold(_)));
                if replace {
                    first = Some(j);
                }
            }
        }
    }
    match (first, enforcing) {
        (None, _) if !extends => Gate::Independent,
        (None, _) | (Some(Judgement::Clear), _) => Gate::Clear,
        (Some(Judgement::Hold(why)), true) => Gate::Hold(why),
        (Some(Judgement::Hold(why)), false) => Gate::ShadowWouldHold(format!("shadow — would hold under an enforcing profile: {why}")),
        (Some(Judgement::NotEstablished(why)), true) => Gate::NotEstablished(why),
        (Some(Judgement::NotEstablished(why)), false) => {
            Gate::ShadowWouldNotEstablish(format!("shadow — would refuse as not established under an enforcing profile: {why}"))
        }
    }
}

// ── The fleet tally ──────────────────────────────────────────────────────────

/// The summary namespace: `sys/ae-exposure/{node}` — one key per node, self-owned.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) const SUMMARY_PREFIX: &str = "sys/ae-exposure/";

/// The most unresolved records one summary lists. A node holding more says `truncated`, and every reader takes its
/// coverage as unknown — never a list cut short read as complete. Each record is three 128-bit digests and a time, so a
/// full summary stays far below the KV write cap.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) const MAX_SUMMARY_RECORDS: usize = 2048;

/// How long a node out of view keeps its summary counted: the history horizon plus the staleness and drift bounds,
/// as row K2 keeps a departed node's tally. Past it, any node collects the summary and its unresolved effects count
/// toward nothing — the one place this policy forgets an unresolved effect, stated.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) fn summary_horizon_ms(staleness_ms: u64, drift_ms: u64) -> u64 {
    HISTORY_HORIZON_MS.saturating_add(staleness_ms).saturating_add(drift_ms)
}

/// The history horizon, 24 hours — `action_evaluator::MAX_FLEET_WINDOW_MS`, which a build without `tls` does not have;
/// pinned equal by a test.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) const HISTORY_HORIZON_MS: u64 = 24 * 60 * 60 * 1000;

/// One unresolved record as a summary lists it: digests of its names, and when it was prepared.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SummaryRecord {
    /// The prepared record's hash, digested.
    pub(crate) r: String,
    /// Every execution evidence hash it has had, digested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) e: Vec<String>,
    /// The run's correlation, digested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) c: Option<String>,
    /// When it was prepared.
    pub(crate) t: u64,
}

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
impl SummaryRecord {
    pub(crate) fn of(r: &UnresolvedRecord) -> Self {
        SummaryRecord { r: digest(&r.record_hash), e: r.evidence_hashes.iter().map(|h| digest(h)).collect(), c: r.correlation.as_deref().map(digest), t: r.prepared_at_ms }
    }

    fn extended_by(&self, parent: Option<&str>, correlation: Option<&str>) -> bool {
        parent.is_some_and(|p| p == self.r || self.e.iter().any(|e| e == p)) || correlation.is_some_and(|c| Some(c) == self.c.as_deref())
    }
}

/// One node's published exposure: when, whether it has a source attached, and its unresolved records.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) as_of: u64,
    #[serde(default)]
    pub(crate) source: bool,
    #[serde(default)]
    pub(crate) records: Vec<SummaryRecord>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) truncated: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) departed: bool,
}

/// The summary key of `node`.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) fn summary_key(node: &str) -> String {
    format!("{SUMMARY_PREFIX}{node}")
}

/// The first 128 bits of a SHA-256, hex — what a fleet-visible key names a record or a run by, as row K2's tally does.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) fn digest(s: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(s.as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// One other node's summary, as this node holds it.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) struct NodeSummary {
    pub(crate) node: String,
    /// In this node's view of the fleet now.
    pub(crate) peer: bool,
    /// A member this node can verify — or this node cannot verify any (no `[tls]`), and so cannot rule it out.
    pub(crate) member: bool,
    /// `None`: no summary; `Some(Err)`: one this node cannot read.
    pub(crate) summary: Option<Result<Summary, ()>>,
}

/// The fleet's exposure of the closure a request naming `key` extends: this node's own, plus every other node's
/// summary — or why it is not established.
///
/// - A **peer** with no summary, an unreadable, stale (older than `staleness_ms`), future-dated (past `drift_ms`) or
///   truncated one is **unknown coverage**: not established. A peer's `departed` mark is not read — a node in view has
///   not left, and the mark would excuse it from the staleness bound.
/// - A node **out of view** counts only as a member: an id no member holds is not read. One whose summary cannot be
///   read, or is from the future, is not read either — the collector drops it. One whose last summary lists a record
///   the request extends is counted if it departed (its last word), and is unknown coverage otherwise: its effects are
///   still unresolved, and nothing here can say they resolved — until the collector forgets it past the horizon. One
///   whose last summary lists none of them is not waited for.
/// - A node with no source attached reports none: it holds no effects this policy can see.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) fn sum_fleet(
    me: &str, mine: ClosureExposure, key: &ClosureKey<'_>, others: &[NodeSummary], now_ms: u64, staleness_ms: u64, drift_ms: u64,
) -> Exposure {
    let (parent, correlation) = (key.parent.map(digest), key.correlation.map(digest));
    let mut total = mine;
    let mut publishers = vec![me.to_string()];
    let mut unknown = Vec::new();
    for o in others {
        let readable = match &o.summary {
            Some(Ok(s)) if s.as_of <= now_ms.saturating_add(drift_ms) => Some(s),
            _ => None,
        };
        if o.peer {
            let Some(s) = readable else {
                let why = match &o.summary {
                    None => "no summary".to_string(),
                    Some(Err(())) => "a summary this node cannot read".into(),
                    Some(Ok(s)) => format!("a summary {} ms in the future", s.as_of.saturating_sub(now_ms)),
                };
                unknown.push(format!("{} ({why})", o.node));
                continue;
            };
            let age = now_ms.saturating_sub(s.as_of);
            if age > staleness_ms {
                unknown.push(format!("{} (stale: {age} ms)", o.node));
                continue;
            }
            if s.truncated {
                unknown.push(format!("{} (its summary is truncated)", o.node));
                continue;
            }
        }
        let Some(s) = readable else { continue };
        let matched = s
            .records
            .iter()
            .filter(|r| r.extended_by(parent.as_deref(), correlation.as_deref()))
            .fold(ClosureExposure::default(), |e, r| e.plus(ClosureExposure::one(r.t)));
        if !o.peer {
            if !o.member {
                continue;
            }
            match (matched.unresolved > 0 || s.truncated, s.departed && !s.truncated) {
                (false, _) => continue,
                (true, true) => {}
                (true, false) => {
                    unknown.push(format!("{} (out of view, and its last summary may hold effects this request extends)", o.node));
                    continue;
                }
            }
        }
        total = total.plus(matched);
        publishers.push(o.node.clone());
    }
    if !unknown.is_empty() {
        return Exposure::Unavailable(format!(
            "the fleet tally of {} is unknown (fresh within {staleness_ms} ms is required) — a closure that cannot be read is not empty",
            unknown.join(", ")
        ));
    }
    Exposure::View { exposure: total, scope: ExposureScope::FleetTally, publishers }
}

/// Whether the collector removes another node's summary: an out-of-view node's that cannot be read, is dated in the
/// future, names a node no member holds (when this node can verify), or is past the horizon. A peer's is never removed.
#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
pub(crate) fn collectable(o: &NodeSummary, can_verify: bool, now_ms: u64, staleness_ms: u64, drift_ms: u64) -> bool {
    if o.peer {
        return false;
    }
    match &o.summary {
        None => false,
        Some(Err(())) => true,
        Some(Ok(s)) => {
            s.as_of > now_ms.saturating_add(drift_ms)
                || (can_verify && !o.member)
                || now_ms.saturating_sub(s.as_of) > summary_horizon_ms(staleness_ms, drift_ms)
        }
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) use node::{collect_summaries, depart, exposure_gate, publish_summary, reassert_own};

#[cfg(all(feature = "gateway", feature = "tls"))]
mod node {
    use super::*;
    use crate::agent::TaskCtx;
    use bytes::Bytes;
    use std::sync::Arc;

    /// The closure the attached source gives for `key`: `Ok(None)` when no source is attached.
    fn local(ctx: &TaskCtx, key: &ClosureKey<'_>) -> Result<Option<ClosureExposure>, String> {
        match ctx.exposure_source.get() {
            None => Ok(None),
            Some(s) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.exposure_of(key)))
                .unwrap_or_else(|_| Err("the exposure source panicked".into()))
                .map(Some),
        }
    }

    /// The topology rules: `enforcement_point` needs this node's source — none is *not established*, never clear;
    /// `fleet_tally` reads this node's source if it has one (none adds nothing) and every node's summary.
    fn read(ctx: &Arc<TaskCtx>, key: &ClosureKey<'_>, scope: ExposureScope, now_ms: u64) -> Exposure {
        let mine = match local(ctx, key) {
            Ok(m) => m,
            Err(why) => return Exposure::Unavailable(why),
        };
        match scope {
            ExposureScope::EnforcementPoint => match mine {
                Some(exposure) => Exposure::View { exposure, scope, publishers: vec![ctx.node_id.to_string()] },
                None => Exposure::Unavailable("no exposure source is attached to this node, and an enforcement_point rule reads this node's (see GossipAgent::with_exposure_source)".into()),
            },
            ExposureScope::FleetTally => fleet(ctx, key, mine.unwrap_or_default(), now_ms),
        }
    }

    /// Every other node's summary this node holds, and every peer it holds none for.
    pub(crate) fn others(ctx: &Arc<TaskCtx>) -> (Vec<(NodeSummary, Option<u64>)>, bool) {
        let me = ctx.node_id.to_string();
        let can_verify = ctx.tls.get().is_some();
        let peers: std::collections::BTreeSet<String> = ctx.peers.pin().keys().map(|p| p.to_string()).collect();
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in mycelium_core::ops::kv_scan_prefix(ctx, SUMMARY_PREFIX) {
            let Some(node) = key.strip_prefix(SUMMARY_PREFIX) else { continue };
            // This node's own key is never read: its own exposure comes from its source, live.
            if node == me || node.contains('/') {
                continue;
            }
            // A removed member's summary is not consulted.
            if let Ok(id) = node.parse::<mycelium_core::NodeId>()
                && ctx.removed.is_node_removed(&id)
            {
                continue;
            }
            seen.insert(node.to_string());
            let stamp = ctx.kv_state.store.pin().get(key.as_ref()).map(|e| e.timestamp);
            let member = !can_verify || super::super::fleet_history::verified_member(ctx, node);
            let summary = Some(serde_json::from_slice::<Summary>(&value).map_err(|_| ()));
            out.push((NodeSummary { node: node.to_string(), peer: peers.contains(node), member, summary }, stamp));
        }
        for peer in peers.iter().filter(|p| !seen.contains(*p)) {
            out.push((NodeSummary { node: peer.clone(), peer: true, member: true, summary: None }, None));
        }
        (out, can_verify)
    }

    fn fleet(ctx: &Arc<TaskCtx>, key: &ClosureKey<'_>, mine: ClosureExposure, now_ms: u64) -> Exposure {
        let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
        let view = crate::agent::emergent::compute_view_confidence_at(ctx, mycelium_core::sim_seam::mono_instant());
        if let Err(why) = crate::control::assess(&view, &bound) {
            return Exposure::Unavailable(format!("this node's view of the fleet is not established ({why:?}), so the tally's coverage is unknown"));
        }
        let others: Vec<NodeSummary> = others(ctx).0.into_iter().map(|(o, _)| o).collect();
        sum_fleet(&ctx.node_id.to_string(), mine, key, &others, now_ms, bound.max_staleness_ms, ctx.config.max_clock_drift_ms)
    }

    /// The gateway's exposure gate for one request (`http::ae_decide`): untouched without a policy or under `legacy`
    /// (neither read nor counted); the node's profile decides whether a hold holds. Counts nothing — the caller counts
    /// once the evaluator has permitted.
    pub(crate) fn exposure_gate(
        ctx: &Arc<TaskCtx>, actor: &str, operation: &str, resource: &str, parent: Option<&str>, correlation: Option<&str>, now_ms: u64,
    ) -> Gate {
        let Some(policy) = ctx.exposure_policy.get() else { return Gate::Untouched };
        let profile = crate::control::Profile::from_u8(ctx.control_profile.load(std::sync::atomic::Ordering::Acquire));
        if profile == crate::control::Profile::Legacy {
            return Gate::Untouched;
        }
        let key = ClosureKey { parent, correlation };
        gate(policy, actor, operation, resource, &key, profile.enforces(), now_ms, |scope| read(ctx, &key, scope, now_ms))
    }

    /// This node's summary for the fleet tally, written on the tally heartbeat's tick (`fleet_history::start_heartbeat`)
    /// through the soft-state gate. A node with a source lists its unresolved records, up to [`MAX_SUMMARY_RECORDS`]; one
    /// without says it has none; one whose source cannot list them says `truncated`, which every reader takes as unknown.
    pub(crate) fn publish_summary(ctx: &Arc<TaskCtx>, now_ms: u64) {
        let Some(bytes) = summary_bytes(ctx, now_ms, false) else { return };
        ctx.soft_state_gate.write(|| {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(summary_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
        });
    }

    /// This node's last summary at a graceful shutdown, marked departed: a node out of view counts what it lists and
    /// does not wait for it. Written past the soft-state gate by the shutdown sweep, as the tally heartbeat's last word is.
    pub(crate) fn depart(ctx: &TaskCtx, now_ms: u64) {
        let Some(bytes) = summary_bytes(ctx, now_ms, true) else { return };
        let _ = mycelium_core::ops::kv_set(ctx, Arc::from(summary_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
    }

    /// **Before this node's connections come up**: its summary re-asserted on this node only, so a peer's collection of
    /// its old one while it was away is older than what it holds — as row K2's `reassert_own` does for the tally.
    pub(crate) fn reassert_own(ctx: &TaskCtx) {
        let now = ctx.hlc.decision_now_ms();
        if let Some(bytes) = summary_bytes(ctx, now, false) {
            mycelium_core::ops::kv_write_local(ctx, Arc::from(summary_key(&ctx.node_id.to_string()).as_str()), Some(Bytes::from(bytes)));
        }
    }

    /// **Collect other nodes' summaries** on the tally heartbeat's tick, as row K2's `collect_residue` collects a
    /// non-peer's tally: an out-of-view node's summary that cannot be read, is dated in the future, names a node no
    /// member holds (with `[tls]`, which verifies) or is past the horizon is tombstoned at the stamp of the record this
    /// node holds (`kv_delete_held`), so it loses to anything its owner wrote later. A peer's is never collected. Counted
    /// (`ExposureCounts::collected`).
    pub(crate) fn collect_summaries(ctx: &Arc<TaskCtx>, now_ms: u64) -> usize {
        let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
        let (others, can_verify) = others(ctx);
        let mut n = 0;
        for (o, stamp) in others {
            if let (true, Some(ts)) = (collectable(&o, can_verify, now_ms, bound.max_staleness_ms, ctx.config.max_clock_drift_ms), stamp)
                && mycelium_core::ops::kv_delete_held(ctx, Arc::from(summary_key(&o.node).as_str()), ts)
            {
                n += 1;
            }
        }
        ctx.exposure_counters.collected(n);
        n
    }

    pub(crate) fn summary_bytes(ctx: &TaskCtx, now_ms: u64, departed: bool) -> Option<Vec<u8>> {
        let mut summary = Summary { as_of: now_ms, departed, ..Summary::default() };
        if let Some(source) = ctx.exposure_source.get() {
            summary.source = true;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.unresolved())) {
                Ok(Ok(list)) => {
                    let mut records: Vec<SummaryRecord> = list.iter().map(SummaryRecord::of).collect();
                    records.sort_by(|a, b| a.r.cmp(&b.r));
                    if records.len() > MAX_SUMMARY_RECORDS {
                        summary.truncated = true;
                        records.truncate(MAX_SUMMARY_RECORDS);
                    }
                    summary.records = records;
                }
                // A source that cannot list its records publishes a summary its readers take as unknown coverage.
                _ => summary.truncated = true,
            }
        }
        serde_json::to_vec(&summary).ok()
    }
}

#[cfg(all(test, feature = "gateway", feature = "tls"))]
pub(crate) use node::{others as node_others, summary_bytes};

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
