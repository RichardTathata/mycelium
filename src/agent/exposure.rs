//! **The causal exposure policy** (adopter-surface Y5, D18): a `[[rule]]` whose input is an actor's **unresolved
//! closure** — the external effects attributed to it whose fate is not known — and which may hold only new dependent
//! work, never the work that resolves the closure.
//!
//! **The input.** An actor's unresolved closure is every effect an [`ExposureSource`] holds for it in a state whose
//! outcome is not known: prepared and not yet answered, answered with *delivery unknown*, or escalated *unresolved*
//! (`mycelium::effects::external`, X5 and X9). Two numbers are read from it: its **size** and the **age of its oldest
//! record**. They are separate thresholds because the second grows while nothing is done: an actor with one effect
//! unresolved for an hour is as exposed as one with fifty unresolved for a second, and only the age says so.
//!
//! **What it may hold — the stability record's decisive rule, applied unchanged** (`docs/design/adaptive-stability.md`
//! §2). It holds **new dependent work**: a call admitted at one of the gateway's doors (`/mcp`, `/a2a`, the LLM doors,
//! the federation route, the provider check), which would extend the closure. It **never** holds what resolves the
//! closure or protects the node, and this is structural rather than a rule an operator could misconfigure:
//!
//! - a **reconciliation probe** goes through no evaluator at all (`Reconciler::probe`);
//! - a **compensation** is decided through [`preflight`](super::action_evaluator::preflight) by the reconciler
//!   (`CompensationPermit::decide`), which never reaches this gate — the gate is in the gateway's decision
//!   (`http::ae_decide`), not in the shared seam;
//! - a **protective shed** and a **rescue from zero** are governor actions, never envelopes;
//! - and at a door, an operation in the reconciler's own family (`effect.*`) or a grant's (`grant.*`) is
//!   [`WorkClass::Exempt`] whatever a rule selects.
//!
//! **Counted as K2 counts history.** A rule names its scope: [`ExposureScope::EnforcementPoint`] — this node's
//! attached source — or [`ExposureScope::FleetTally`] — this node's source plus every peer's published summary
//! (`sys/ae-exposure/{node}`), approximate, under the same coverage rules as row K2's tally: this node's view of the
//! fleet under the governors' confidence bound, a peer with no summary, a stale one or one from the future is
//! **unknown coverage**, and unknown coverage **fails closed** (`Indeterminate`, never a zero). **Not built:**
//! [`ExposureScope::FleetConsensus`] — an exact bound would need a seat held until the effect *resolves*, which no
//! lease of a fixed window expresses; a rule naming it is refused at load, by name.
//!
//! **Shadow first, under the profile ladder** (`docs/operations/control-profiles.md`). Under `legacy` and `observe`
//! the policy is decided and counted — [`ExposureCounts::would_hold`] — and holds nothing, so attaching one starts in
//! shadow on a node that has not stepped the ladder. Under `enforce-local` and `enforce-allocated` it holds.
//! Unknown coverage is held only on an enforcing rung too: in shadow it is counted
//! ([`ExposureCounts::would_not_establish`]) and the call proceeds.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// The scope's name, as `ExposureScope::label` spells the same two.
    pub fn label(&self) -> &'static str {
        match self {
            ExposureScope::EnforcementPoint => "enforcement_point",
            ExposureScope::FleetTally => "fleet_tally",
        }
    }
}

/// How exposed one actor is, as one source holds it: how many of its effects have no known outcome, and when the
/// oldest of them was prepared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorExposure {
    /// Effects whose outcome is not known.
    pub unresolved: u32,
    /// When the oldest of them was prepared, epoch milliseconds; `None` when there is none.
    pub oldest_at_ms: Option<u64>,
}

impl ActorExposure {
    /// The two summed: sizes add, the oldest is the earliest.
    pub fn plus(self, other: ActorExposure) -> ActorExposure {
        ActorExposure {
            unresolved: self.unresolved.saturating_add(other.unresolved),
            oldest_at_ms: match (self.oldest_at_ms, other.oldest_at_ms) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }
}

/// What holds the effects an actor's exposure is read from — the external-effect reconciler
/// (`mycelium::effects::external::Reconciler`) implements it; attach one with
/// `GossipAgent::with_exposure_source`. **A source that cannot read its records says so** (`Err`), and a decision
/// on it is not established — never a zero.
pub trait ExposureSource: Send + Sync + 'static {
    /// The actor's unresolved closure as this source holds it.
    fn exposure_of(&self, actor: &str) -> Result<ActorExposure, String>;
    /// Every actor this source holds an unresolved effect for — what a node publishes for the fleet tally.
    fn exposures(&self) -> Result<Vec<(String, ActorExposure)>, String>;
}

/// The thresholds of one exposure rule. At least one is set; `max_unresolved` is at least 1 (zero would hold every
/// call of every actor, unresolved closure or none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExposureLimit {
    /// Hold new dependent work once the closure holds this many unresolved effects.
    pub max_unresolved: Option<u32>,
    /// Hold new dependent work once the oldest unresolved effect is older than this, in milliseconds.
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
    /// `max_unresolved = 0` would hold every call, whatever the closure holds.
    ZeroSize,
    /// `max_oldest_age_ms = 0` would hold every call of an actor with any effect in flight.
    ZeroAge,
    /// A scope this policy does not count at.
    Scope(String),
}

impl std::fmt::Display for ExposureRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoThreshold => f.write_str("exposure: set max_unresolved, max_oldest_age_ms or both — a rule with neither holds nothing"),
            Self::ZeroSize => f.write_str("exposure.max_unresolved = 0 would hold every call, whatever the closure holds; the least is 1"),
            Self::ZeroAge => f.write_str("exposure.max_oldest_age_ms = 0 would hold every call of an actor with any effect in flight"),
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

    /// The rules that select this request — empty when none does or the work is exempt.
    pub fn selecting(&self, actor: &str, operation: &str, resource: &str) -> Vec<&ExposureRule> {
        self.rules.iter().filter(|r| r.selects(actor, operation, resource)).collect()
    }
}

/// What a request is, as the exposure policy sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClass {
    /// New work that would extend the actor's closure: the policy may hold it.
    Dependent,
    /// Work the policy never holds, and why.
    Exempt(&'static str),
}

impl WorkClass {
    /// The class of `operation` at a door. The reconciler's own family (`effect.*` — a probe, a compensation, a leave)
    /// resolves the closure; a grant (`grant.*`) authorises and runs nothing. Everything else a door admits is new work.
    pub fn of(operation: &str) -> WorkClass {
        if operation.starts_with("effect.") {
            WorkClass::Exempt("resolves the closure: a probe, a compensation or a leave is never held by exposure")
        } else if operation.starts_with("grant.") {
            WorkClass::Exempt("an authorisation, not work: it extends no closure")
        } else {
            WorkClass::Dependent
        }
    }
}

/// The exposure a rule decides on: the closure read at the rule's scope, or why it could not be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// What was read, and from whom.
    View { exposure: ActorExposure, scope: ExposureScope, publishers: Vec<String> },
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
/// closure already holds `max_unresolved` (the call would extend it past); the age threshold when the oldest record
/// is **older** than `max_oldest_age_ms`. Each fires on its own — an aged record holds with the size unmet.
pub fn judge(rule: &ExposureRule, exposure: &Exposure, now_ms: u64) -> Judgement {
    let (e, scope) = match exposure {
        Exposure::Unavailable(why) => return Judgement::NotEstablished(format!("the actor's unresolved closure is not established: {why}")),
        Exposure::View { exposure, scope, .. } => (exposure, scope),
    };
    let at = scope.label();
    if let Some(max) = rule.limit.max_unresolved
        && e.unresolved >= max
    {
        return Judgement::Hold(format!(
            "exposure: the actor's unresolved closure holds {} effect(s) at {at}, the rule's size threshold is {max} — new dependent work is held until the closure resolves",
            e.unresolved
        ));
    }
    if let (Some(max_age), Some(oldest)) = (rule.limit.max_oldest_age_ms, e.oldest_at_ms) {
        let age = now_ms.saturating_sub(oldest);
        if age > max_age {
            return Judgement::Hold(format!(
                "exposure: the actor's oldest unresolved effect is {age} ms old at {at}, past the rule's age threshold of {max_age} ms ({} unresolved) — new dependent work is held until it resolves",
                e.unresolved
            ));
        }
    }
    Judgement::Clear
}

/// The policy's own counts, from attachment: what it judged, what it held, and what it **would have held** in shadow.
#[derive(Debug, Default)]
pub struct ExposureCounters {
    judged: AtomicU64,
    held: AtomicU64,
    would_hold: AtomicU64,
    not_established: AtomicU64,
    would_not_establish: AtomicU64,
    exempt: AtomicU64,
}

/// A snapshot of [`ExposureCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ExposureCounts {
    /// Dependent requests a rule selected and decided.
    pub judged: u64,
    /// Held on an enforcing rung.
    pub held: u64,
    /// In shadow, a threshold reached: an enforcing rung would have held it. Nothing was held.
    pub would_hold: u64,
    /// On an enforcing rung, the closure not established: refused as not established.
    pub not_established: u64,
    /// In shadow, the closure not established: an enforcing rung would have refused it. Nothing was held.
    pub would_not_establish: u64,
    /// Requests a rule selected whose work is exempt (a probe, a compensation, a grant): never held.
    pub exempt: u64,
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
        }
    }
}

/// What the gate came to for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// No rule selects it, or none is attached.
    Untouched,
    /// Selected but exempt: never held.
    Exempt(&'static str),
    /// Selected and clear — or in shadow, with what an enforcing rung would have done, for the evidence record.
    Proceed(Option<String>),
    /// Held: a threshold reached on an enforcing rung.
    Hold(String),
    /// Refused as not established on an enforcing rung.
    NotEstablished(String),
}

/// Decide the gate for a request against `policy` on the exposure `read` gives for each scope a selecting rule names,
/// under `enforcing` (the node's profile enforces), counting into `counters`. Pure but for the counters.
#[allow(clippy::too_many_arguments)]
pub fn gate(
    policy: &ExposurePolicy,
    actor: &str,
    operation: &str,
    resource: &str,
    enforcing: bool,
    now_ms: u64,
    counters: &ExposureCounters,
    mut read: impl FnMut(ExposureScope) -> Exposure,
) -> Gate {
    let rules = policy.selecting(actor, operation, resource);
    if rules.is_empty() {
        return Gate::Untouched;
    }
    if let WorkClass::Exempt(why) = WorkClass::of(operation) {
        counters.exempt.fetch_add(1, Ordering::Relaxed);
        return Gate::Exempt(why);
    }
    counters.judged.fetch_add(1, Ordering::Relaxed);
    // One read per scope a rule names, so two rules at one scope decide on one closure.
    let mut read_at: Vec<(ExposureScope, Exposure)> = Vec::new();
    let mut first: Option<Judgement> = None;
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
        (None, _) => Gate::Proceed(None),
        (Some(Judgement::Hold(why)), true) => {
            counters.held.fetch_add(1, Ordering::Relaxed);
            Gate::Hold(why)
        }
        (Some(Judgement::Hold(why)), false) => {
            counters.would_hold.fetch_add(1, Ordering::Relaxed);
            Gate::Proceed(Some(format!("shadow — would hold under an enforcing profile: {why}")))
        }
        (Some(Judgement::NotEstablished(why)), true) => {
            counters.not_established.fetch_add(1, Ordering::Relaxed);
            Gate::NotEstablished(why)
        }
        (Some(Judgement::NotEstablished(why)), false) => {
            counters.would_not_establish.fetch_add(1, Ordering::Relaxed);
            Gate::Proceed(Some(format!("shadow — would refuse as not established under an enforcing profile: {why}")))
        }
        (Some(Judgement::Clear), _) => Gate::Proceed(None),
    }
}

// ── The fleet tally ──────────────────────────────────────────────────────────

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// The summary namespace: `sys/ae-exposure/{node}` — one key per node, self-owned.
pub(crate) const SUMMARY_PREFIX: &str = "sys/ae-exposure/";

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// The most actors one summary lists. A node holding unresolved effects for more says `truncated`, and every reader
/// takes its coverage as unknown — never a list cut short read as complete.
pub(crate) const MAX_SUMMARY_ACTORS: usize = 4096;

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// One node's published exposure: when, whether it has a source attached, and, per actor (by digest), its size and
/// oldest record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) as_of: u64,
    #[serde(default)]
    pub(crate) source: bool,
    #[serde(default)]
    pub(crate) actors: std::collections::BTreeMap<String, ActorExposure>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) truncated: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) departed: bool,
}

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// The summary key of `node`.
pub(crate) fn summary_key(node: &str) -> String {
    format!("{SUMMARY_PREFIX}{node}")
}

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// The first 128 bits of a SHA-256 of `actor`, hex — the pseudonym an actor goes by in a fleet-visible key, as in
/// row K2's tally.
pub(crate) fn actor_digest(actor: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(actor.as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// One other node's summary, as this node holds it.
pub(crate) struct NodeSummary {
    pub(crate) node: String,
    /// In this node's view of the fleet now.
    pub(crate) peer: bool,
    /// `None`: no summary; `Some(Err)`: one this node cannot read.
    pub(crate) summary: Option<Result<Summary, ()>>,
}

#[cfg(any(test, all(feature = "gateway", feature = "tls")))]
/// The fleet's exposure of `actor`: this node's own, plus every other node's summary — or why it is not established.
///
/// - A **peer** with no summary, an unreadable one, a stale one (older than `staleness_ms`, unless departed), one
///   from the future (past `drift_ms`) or a truncated one is **unknown coverage**: not established.
/// - A node **out of view** whose last summary lists the actor is counted if it departed (its last word), and is
///   unknown coverage otherwise: its effects are still unresolved, and nothing here can say they resolved. One whose
///   last summary does not list the actor is not waited for — the work that would add to it is admitted by its own
///   enforcement point. A removed member's summary is not consulted (the caller leaves it out).
/// - A node with no source attached reports none: it holds no effects this policy can see.
pub(crate) fn sum_fleet(
    me: &str, mine: ActorExposure, actor: &str, others: &[NodeSummary], now_ms: u64, staleness_ms: u64, drift_ms: u64,
) -> Exposure {
    let digest = actor_digest(actor);
    let mut total = mine;
    let mut publishers = vec![me.to_string()];
    let mut unknown = Vec::new();
    for o in others {
        let s = match &o.summary {
            None if o.peer => {
                unknown.push(format!("{} (no summary)", o.node));
                continue;
            }
            None => continue,
            Some(Err(())) => {
                unknown.push(format!("{} (a summary this node cannot read)", o.node));
                continue;
            }
            Some(Ok(s)) => s,
        };
        if s.as_of > now_ms.saturating_add(drift_ms) {
            unknown.push(format!("{} (a summary {} ms in the future)", o.node, s.as_of - now_ms));
            continue;
        }
        let listed = s.actors.get(&digest).copied();
        if o.peer {
            let age = now_ms.saturating_sub(s.as_of);
            if !s.departed && age > staleness_ms {
                unknown.push(format!("{} (stale: {age} ms)", o.node));
                continue;
            }
            if s.truncated {
                unknown.push(format!("{} (its summary is truncated)", o.node));
                continue;
            }
        } else {
            match listed {
                None if !s.truncated => continue,
                _ if s.departed && !s.truncated => {}
                _ => {
                    unknown.push(format!("{} (out of view, and its last summary may hold this actor's unresolved effects)", o.node));
                    continue;
                }
            }
        }
        if let Some(e) = listed {
            total = total.plus(e);
        }
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

#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) use node::{exposure_gate, publish_summary, depart};

#[cfg(all(feature = "gateway", feature = "tls"))]
mod node {
    use super::*;
    use crate::agent::TaskCtx;
    use bytes::Bytes;
    use std::sync::Arc;

    /// The exposure the attached source gives for `actor` at this point, or why none could be read.
    fn local(ctx: &TaskCtx, actor: &str) -> Result<ActorExposure, String> {
        match ctx.exposure_source.get() {
            None => Err("no exposure source is attached to this node (see GossipAgent::with_exposure_source)".into()),
            Some(s) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.exposure_of(actor)))
                .unwrap_or_else(|_| Err("the exposure source panicked".into())),
        }
    }

    fn read(ctx: &Arc<TaskCtx>, actor: &str, scope: ExposureScope, now_ms: u64) -> Exposure {
        let mine = match local(ctx, actor) {
            Ok(e) => e,
            Err(why) => return Exposure::Unavailable(why),
        };
        match scope {
            ExposureScope::FleetTally => fleet(ctx, actor, mine, now_ms),
            ExposureScope::EnforcementPoint => Exposure::View { exposure: mine, scope: ExposureScope::EnforcementPoint, publishers: vec![ctx.node_id.to_string()] },
        }
    }

    fn fleet(ctx: &Arc<TaskCtx>, actor: &str, mine: ActorExposure, now_ms: u64) -> Exposure {
        let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
        let view = crate::agent::emergent::compute_view_confidence_at(ctx, mycelium_core::sim_seam::mono_instant());
        if let Err(why) = crate::control::assess(&view, &bound) {
            return Exposure::Unavailable(format!("this node's view of the fleet is not established ({why:?}), so the tally's coverage is unknown"));
        }
        let me = ctx.node_id.to_string();
        let peers: std::collections::BTreeSet<String> = ctx.peers.pin().keys().map(|p| p.to_string()).collect();
        let mut others: Vec<NodeSummary> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let parse = |b: Bytes| serde_json::from_slice::<Summary>(&b).map_err(|_| ());
        for (key, value) in mycelium_core::ops::kv_scan_prefix(ctx, SUMMARY_PREFIX) {
            let Some(node) = key.strip_prefix(SUMMARY_PREFIX) else { continue };
            if node == me || node.contains('/') {
                continue;
            }
            if let Ok(id) = node.parse::<mycelium_core::NodeId>()
                && ctx.removed.is_node_removed(&id)
            {
                continue;
            }
            seen.insert(node.to_string());
            others.push(NodeSummary { node: node.to_string(), peer: peers.contains(node), summary: Some(parse(value)) });
        }
        for peer in peers.iter().filter(|p| !seen.contains(*p)) {
            others.push(NodeSummary { node: peer.clone(), peer: true, summary: None });
        }
        sum_fleet(&me, mine, actor, &others, now_ms, bound.max_staleness_ms, ctx.config.max_clock_drift_ms)
    }

    /// The gateway's exposure gate for one request (`http::ae_decide`): untouched without a policy; the node's
    /// profile decides whether a hold holds.
    pub(crate) fn exposure_gate(ctx: &Arc<TaskCtx>, actor: &str, operation: &str, resource: &str, now_ms: u64) -> Gate {
        let Some(policy) = ctx.exposure_policy.get() else { return Gate::Untouched };
        let enforcing = crate::control::Profile::from_u8(ctx.control_profile.load(std::sync::atomic::Ordering::Acquire)).enforces();
        let g = gate(policy, actor, operation, resource, enforcing, now_ms, &ctx.exposure_counters, |scope| read(ctx, actor, scope, now_ms));
        #[cfg(feature = "metrics")]
        {
            let outcome = match &g {
                Gate::Untouched => None,
                Gate::Exempt(_) => Some("exempt"),
                Gate::Proceed(None) => Some("clear"),
                Gate::Proceed(Some(_)) => Some("shadow"),
                Gate::Hold(_) => Some("held"),
                Gate::NotEstablished(_) => Some("not_established"),
            };
            if let Some(o) = outcome {
                metrics::counter!("mycelium_ae_exposure_total", "outcome" => o).increment(1);
            }
        }
        g
    }

    /// This node's summary for the fleet tally, written on the tally heartbeat's tick (`fleet_history::start_heartbeat`)
    /// through the soft-state gate. A node with a source lists every actor it holds an unresolved effect for, up to
    /// [`MAX_SUMMARY_ACTORS`]; one without says it has none.
    pub(crate) fn publish_summary(ctx: &Arc<TaskCtx>, now_ms: u64) {
        let Some(bytes) = summary_bytes(ctx, now_ms, false) else { return };
        ctx.soft_state_gate.write(|| {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(summary_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
        });
    }

    /// This node's last summary at a graceful shutdown, marked departed: peers count what it lists and do not wait
    /// for it. Written past the soft-state gate by the shutdown sweep, as the tally heartbeat's last word is.
    pub(crate) fn depart(ctx: &TaskCtx, now_ms: u64) {
        let Some(bytes) = summary_bytes(ctx, now_ms, true) else { return };
        let _ = mycelium_core::ops::kv_set(ctx, Arc::from(summary_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
    }

    fn summary_bytes(ctx: &TaskCtx, now_ms: u64, departed: bool) -> Option<Vec<u8>> {
        let mut summary = Summary { as_of: now_ms, departed, ..Summary::default() };
        if let Some(source) = ctx.exposure_source.get() {
            summary.source = true;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.exposures())) {
                Ok(Ok(list)) => {
                    let mut list: Vec<_> = list.into_iter().filter(|(_, e)| e.unresolved > 0).collect();
                    list.sort_by(|a, b| a.0.cmp(&b.0));
                    if list.len() > MAX_SUMMARY_ACTORS {
                        summary.truncated = true;
                        list.truncate(MAX_SUMMARY_ACTORS);
                    }
                    for (actor, e) in list {
                        let d = actor_digest(&actor);
                        let prev = summary.actors.get(&d).copied().unwrap_or_default();
                        summary.actors.insert(d, prev.plus(e));
                    }
                }
                // A source that cannot list its records publishes a summary its readers take as unknown coverage.
                _ => summary.truncated = true,
            }
        }
        serde_json::to_vec(&summary).ok()
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
