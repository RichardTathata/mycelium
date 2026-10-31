//! **Exposure at the take and emit doors** (adopter-surface Y11, from Y5): a tuple **take**, a blackboard **claim**
//! and a signal **emit** extend a closure as surely as a call does, so each runs the gateway's decision — the action
//! evaluator, then Y5's exposure gate — **when an exposure rule selects its operation**, and is untouched otherwise.
//!
//! **Opt-in, by the rule.** A door asks [`selected`] first: no exposure policy, the `legacy` profile, or no rule whose
//! actor, operation and resource select this request, and the door does what it did before — no decision, no record,
//! no clock read and no draw, so every recording made before this replays unchanged
//! (`the_checked_in_schedule_run_replays_decision_for_decision`). Selected, it runs `ae_decide` exactly as `/mcp`
//! does: inert without an evaluator (Y5's rule — the exposure policy is consulted only on a permit), a permit counted
//! by the gate, a hold or unknown coverage refused on an enforcing rung, a would-hold counted in shadow.
//!
//! **Door-only lineage.** The two causal fields (X4) are read where the request carries them — a gateway body's
//! `_meta`, a Rust verb's [`CausalFields`] — and go on **this door's own decision record**, and nowhere further: a
//! signal frame and a tuple's WAL record carry no field for them, and adding one would change the wire or the WAL.
//! *Not built, stated:* carrying lineage onward; and enforcement at these doors whenever an evaluator is attached
//! without a rule selecting them.
//!
//! **Never decided, stated** (#708's review, finding 2, round 2 C): the plain verbs — `MeshHandle::{emit,
//! emit_ordered, emit_async, request, advertise, advertise_persistent}`, `mycelium_core::ops::{emit_signal,
//! emit_signal_ordered, emit_signal_async}`, `ServiceHandle::{deliver_event, emit_reliable, emit_sharded, rpc_call,
//! rpc_call_with_mandate, rpc_call_as, scatter_gather, bulk_call}`, `TupleSpace::{take, take_by_key}`,
//! `Blackboard::claim` — and a hosted WASM component's `mesh.emit` (`mycelium-wasm-host`), and a remote worker's take
//! or claim where the primary serves it: each door is decided at the node the work starts on, by its `*_in` verb or
//! its gateway route. A raw gateway call of a companion's registered door kind (`rpc/call`, `scatter`,
//! `overlay/emit_reliable`) runs that door's decision ([`decide_raw`]) — **only at a gateway node that hosts the
//! companion**, since only the companion registers its kinds: through a gateway without it, the call reaches the
//! provider undecided (`a_raw_take_through_a_gateway_without_the_companion_is_not_decided`; the decision at the
//! provider is not built). A parked take is decided before it waits, so its check-then-act window is the park time.
//!
//! **The doors' operations** are [`DOOR_OPERATIONS`]. None may be an [`EXEMPT_OPERATIONS`] name, and a door asking
//! under an exempt name is refused by name ([`DoorRefusal::EXEMPT_NAME`]) — the companions' side of Y5's collision
//! refusal (`protected_rpc_kinds` at start): a door named as a reconciler's move would pass every rule unheld.

use super::causal::CausalFields;
use super::exposure::EXEMPT_OPERATIONS;
use std::sync::Arc;

/// The operation each door is decided under, as an exposure `[[rule]]` names it.
pub const OP_TUPLE_TAKE: &str = "tuple.take";
/// The keyed take.
pub const OP_TUPLE_TAKE_BY_KEY: &str = "tuple.take_by_key";
/// A blackboard claim.
pub const OP_BB_CLAIM: &str = "bb.claim";
/// A signal emit, at `POST /gateway/signal/emit` and [`GossipAgent::emit_in`](crate::GossipAgent::emit_in).
pub const OP_SIGNAL_EMIT: &str = "signal.emit";
/// A mailbox delivery.
pub const OP_MAILBOX_DELIVER: &str = "mailbox.deliver";
/// A reliable overlay emit.
pub const OP_OVERLAY_EMIT_RELIABLE: &str = "overlay.emit_reliable";
/// A sharded emit.
pub const OP_SHARD_EMIT: &str = "shard.emit";

/// Every operation a take, claim or emit door is decided under.
pub const DOOR_OPERATIONS: &[&str] =
    &[OP_TUPLE_TAKE, OP_TUPLE_TAKE_BY_KEY, OP_BB_CLAIM, OP_SIGNAL_EMIT, OP_MAILBOX_DELIVER, OP_OVERLAY_EMIT_RELIABLE, OP_SHARD_EMIT];

/// The resource a take door names: `tuple:{ns}/{stage}` (`space_resource("tuple", ns, stage)`). A claim door names
/// its board alone, `bb:{ns}`: a predicate names no single resource.
pub fn space_resource(space: &str, ns: &str, name: &str) -> String {
    format!("{space}:{ns}/{name}")
}

/// The resource an emit door names: `signal:{kind}`.
pub fn signal_resource(kind: &str) -> String {
    format!("signal:{kind}")
}

/// How a companion names the resource of a raw call of its door's kind, from the call's payload (`tuple:{ns}/{stage}`
/// from a take request) — [`GossipAgent::register_door_kind`](crate::GossipAgent::register_door_kind).
pub type DoorKindResource = Arc<dyn Fn(&[u8]) -> String + Send + Sync>;

/// Why a door refused: the gateway's machine-readable reason (`action_denied`, `not_established`,
/// `request_malformed`, …, or [`EXEMPT_NAME`](Self::EXEMPT_NAME)) and the sentence. Nothing was taken or emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorRefusal {
    pub reason: String,
    pub message: String,
}

impl DoorRefusal {
    /// A door asked under an operation in [`EXEMPT_OPERATIONS`].
    pub const EXEMPT_NAME: &'static str = "exempt_operation_name";

    /// Whether the refusal is malformed input — a gateway answers it 400 rather than 403.
    pub fn is_malformed(&self) -> bool {
        self.reason == "request_malformed"
    }
}

impl std::fmt::Display for DoorRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused at the door ({}): {}", self.reason, self.message)
    }
}

impl std::error::Error for DoorRefusal {}

/// What became of the act a door permitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorOutcome {
    /// Taken, claimed, emitted or acknowledged.
    Completed,
    /// Nothing happened: a take that timed out, a claim that found nothing, an emit refused before it was sent.
    Nothing,
    /// It may have happened: a transport error, an unacknowledged reliable emit.
    Unknown,
}

/// A door's permission to act, owed its outcome ([`finish`](Self::finish)) once it has. An untouched door — no rule
/// selected it — records nothing.
#[must_use = "a permitted door reports what it did with `finish`"]
pub struct DoorPass {
    #[cfg(all(feature = "gateway", feature = "tls"))]
    decided: Option<(Arc<super::TaskCtx>, super::http::Preflight)>,
}

impl DoorPass {
    fn untouched() -> Self {
        DoorPass {
            #[cfg(all(feature = "gateway", feature = "tls"))]
            decided: None,
        }
    }

    /// Whether this door was decided — a rule selected it and an evaluator permitted it.
    pub fn decided(&self) -> bool {
        #[cfg(all(feature = "gateway", feature = "tls"))]
        return self.decided.is_some();
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        false
    }

    /// Record what the door did, on the decision record's execution (nothing for an untouched door).
    pub async fn finish(self, outcome: DoorOutcome) {
        #[cfg(all(feature = "gateway", feature = "tls"))]
        if let Some((ctx, preflight)) = self.decided {
            use super::action_evaluator::Execution;
            let execution = match outcome {
                DoorOutcome::Completed => Execution::Completed,
                DoorOutcome::Nothing => Execution::None,
                DoorOutcome::Unknown => Execution::Unknown,
            };
            super::http::ae_record_execution(&ctx, &preflight, execution).await;
        }
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let _ = outcome;
    }
}

/// Who acts at a door: an authenticated gateway caller (`None` without a credential: anonymous), or this node itself —
/// a Rust verb, whose actor is `node:{id}`.
#[derive(Clone, Copy)]
pub(crate) enum DoorActor<'a> {
    #[cfg(feature = "gateway")]
    #[cfg_attr(not(feature = "tls"), allow(dead_code))]
    Caller(Option<&'a super::ResolvedPrincipal>),
    Node,
    #[cfg(not(feature = "gateway"))]
    #[allow(dead_code)]
    Never(std::marker::PhantomData<&'a ()>),
}

/// Whether an exposure policy could select anything on this node: one is attached and the profile is not `legacy`.
/// No clock, no draw, no allocation — what an unselected door pays. `Some(shadow)`: the rung does not enforce.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn live(ctx: &super::TaskCtx) -> Option<(&super::exposure::ExposurePolicy, bool)> {
    let policy = ctx.exposure_policy.get()?;
    let profile = crate::control::Profile::from_u8(ctx.control_profile.load(std::sync::atomic::Ordering::Acquire));
    (profile != crate::control::Profile::Legacy).then(|| (policy.as_ref(), !profile.enforces()))
}

/// Decide one door. `params` is the request's payload as the gateway reads it (`_meta` under it).
pub(crate) async fn decide(
    ctx: &Arc<super::TaskCtx>,
    actor: DoorActor<'_>,
    operation: &str,
    resource: &str,
    params: &serde_json::Value,
    enforcement_point: &str,
) -> Result<DoorPass, DoorRefusal> {
    if EXEMPT_OPERATIONS.contains(&operation) {
        return Err(DoorRefusal {
            reason: DoorRefusal::EXEMPT_NAME.into(),
            message: format!(
                "{operation:?} is an exempt operation (a reconciler's move or a grant's issue or revocation), never held by exposure; a take, claim or emit door cannot be decided under it"
            ),
        });
    }
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        use super::http::{ae_preflight_door, Preflight};
        // The opt-in, before anything is built: no policy, or `legacy`, and the door is untouched.
        let Some((policy, shadow)) = live(ctx) else { return Ok(DoorPass::untouched()) };
        let node;
        let caller = match actor {
            DoorActor::Caller(c) => c,
            DoorActor::Node => {
                node = super::ResolvedPrincipal::node(&ctx.node_id);
                Some(&node)
            }
        };
        let principal = caller.map_or(super::gateway_caller::PRINCIPAL_ANONYMOUS, |c| c.principal());
        if policy.selecting(principal, operation, resource).is_empty() {
            return Ok(DoorPass::untouched());
        }
        // Selected: the evaluator and the exposure gate decide. On the shadow rung the door admits what the evaluator
        // refuses, the refusal recorded (#708's review, finding 3) — `observe` holds nothing.
        match ae_preflight_door(ctx, caller, operation, resource, params, enforcement_point, shadow).await {
            Preflight::Inert => Ok(DoorPass::untouched()),
            p @ Preflight::Proceed { .. } => Ok(DoorPass { decided: Some((Arc::clone(ctx), p)) }),
            Preflight::Refuse(r) => Err(DoorRefusal { reason: r.reason().to_string(), message: r.to_string() }),
        }
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    {
        let _ = (ctx, actor, resource, params, enforcement_point);
        Ok(DoorPass::untouched())
    }
}

/// The payload a Rust verb's causal fields stand for: `{"_meta": {…}}`, as a gateway body carries them.
pub(crate) fn params_of(causal: &CausalFields) -> serde_json::Value {
    match causal.meta() {
        Some(m) => serde_json::json!({ "_meta": serde_json::Value::Object(m) }),
        None => serde_json::json!({}),
    }
}

/// The enforcement point a door's decision is recorded under: `gateway:{door}` or `node:{door}`.
pub(crate) fn point(gateway: bool, door: &str) -> String {
    if gateway { format!("gateway:{door}") } else { format!("node:{door}") }
}

/// The door a raw call of `kind` reaches (#708's review, finding 1): its operation and resource, when a companion
/// registered the kind. `None` for every other kind, and in a build without `gateway` and `tls`.
#[cfg(feature = "gateway")]
pub(crate) fn door_kind(ctx: &super::TaskCtx, kind: &str, payload: &[u8]) -> Option<(&'static str, String)> {
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        // The opt-in first (#708's round 2, F): no policy, or `legacy`, and the companion's resource closure — which may
        // copy the payload — is not run.
        live(ctx)?;
        let kinds = ctx.door_kinds.pin();
        let (operation, resource_of) = kinds.get(kind)?;
        Some((*operation, resource_of(payload)))
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    {
        let _ = (ctx, kind, payload);
        None
    }
}

/// A raw door — `POST /gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable` — of a registered door
/// kind runs that door's decision, selected or untouched exactly as the door is (#708's review, finding 1). `Ok(None)`:
/// not a door's kind.
#[cfg(feature = "gateway")]
pub(crate) async fn decide_raw(
    ctx: &Arc<super::TaskCtx>, caller: Option<&super::ResolvedPrincipal>, kind: &str, payload: &[u8], params: &serde_json::Value,
    enforcement_point: &str,
) -> Result<Option<DoorPass>, DoorRefusal> {
    let Some((operation, resource)) = door_kind(ctx, kind, payload) else { return Ok(None) };
    decide(ctx, DoorActor::Caller(caller), operation, &resource, params, enforcement_point).await.map(Some)
}

impl crate::GossipAgent {
    /// **A companion's door kind** (adopter-surface Y11): the RPC kind its take or claim door serves, decided under
    /// `operation` on the resource `resource_of` reads from a call's payload. A raw call of the kind — `POST
    /// /gateway/rpc/call`, `/gateway/scatter`, `/gateway/overlay/emit_reliable` — then runs that door's decision,
    /// selected or untouched exactly as the door is. `TupleSpace` and `Blackboard` register theirs; a member's own
    /// `ServiceHandle::rpc_call` of the kind is not decided. A no-op without `gateway` and `tls`.
    pub fn register_door_kind(&self, kind: impl Into<Arc<str>>, operation: &'static str, resource_of: DoorKindResource) {
        #[cfg(all(feature = "gateway", feature = "tls"))]
        self.task_ctx.door_kinds.pin().insert(kind.into(), (operation, resource_of));
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let _ = (kind.into(), operation, resource_of);
    }

    /// **A take, claim or emit door, decided** (adopter-surface Y11) — for a door this node's own code opens, its
    /// actor this node (`node:{id}`): untouched (an empty [`DoorPass`]) unless an exposure rule selects `operation` on
    /// `resource`; selected, the action evaluator and the exposure gate decide on `causal`, recorded under
    /// `node:{door}`. A companion's Rust verb (`TupleSpace::take_in`, `Blackboard::claim_in`) calls this; report the
    /// outcome with [`DoorPass::finish`]. Without `gateway` and `tls` no policy can be attached and every door is
    /// untouched.
    pub async fn exposure_door_as_node(&self, operation: &str, resource: &str, causal: &CausalFields, door: &str) -> Result<DoorPass, DoorRefusal> {
        decide(&self.task_ctx, DoorActor::Node, operation, resource, &params_of(causal), &point(false, door)).await
    }

    /// [`exposure_door_as_node`](Self::exposure_door_as_node) for a door a **gateway route** opens, its actor the
    /// authenticated caller (`caller`, axum's `Extension<ResolvedPrincipal>`; `None` is anonymous) and its causal
    /// fields the body's `_meta` (`params`), recorded under `gateway:{door}`. A malformed `_meta` on a selected door is
    /// refused ([`DoorRefusal::is_malformed`]); on an untouched one nothing reads it.
    #[cfg(feature = "gateway")]
    pub async fn exposure_door(
        &self, caller: Option<&super::ResolvedPrincipal>, operation: &str, resource: &str, params: &serde_json::Value, door: &str,
    ) -> Result<DoorPass, DoorRefusal> {
        decide(&self.task_ctx, DoorActor::Caller(caller), operation, resource, params, &point(true, door)).await
    }

    /// [`MeshHandle::emit`](crate::MeshHandle::emit) through the emit door (Y11): decided under `signal.emit` on
    /// `signal:{kind}` when an exposure rule selects it, with `causal` on the door's record; untouched otherwise.
    /// `Ok(false)` is the emit's own `false` — the gossip queue was full, local delivery done — recorded as an unknown
    /// execution; `Err` is a refusal, and nothing was emitted.
    pub async fn emit_in(
        &self, kind: impl Into<Arc<str>>, scope: crate::SignalScope, payload: impl Into<bytes::Bytes>, causal: &CausalFields,
    ) -> Result<bool, DoorRefusal> {
        let kind: Arc<str> = kind.into();
        let pass = self.exposure_door_as_node(OP_SIGNAL_EMIT, &signal_resource(&kind), causal, "signal/emit").await?;
        let ok = self.mesh().emit(kind, scope, payload.into());
        pass.finish(if ok { DoorOutcome::Completed } else { DoorOutcome::Unknown }).await;
        Ok(ok)
    }

    /// [`MeshHandle::emit_ordered`](crate::MeshHandle::emit_ordered) through the emit door, as [`emit_in`](Self::emit_in).
    pub async fn emit_ordered_in(
        &self, kind: impl Into<Arc<str>>, scope: crate::SignalScope, payload: impl Into<bytes::Bytes>, causal: &CausalFields,
    ) -> Result<bool, DoorRefusal> {
        let kind: Arc<str> = kind.into();
        let pass = self.exposure_door_as_node(OP_SIGNAL_EMIT, &signal_resource(&kind), causal, "signal/emit").await?;
        let ok = self.mesh().emit_ordered(kind, scope, payload.into());
        pass.finish(if ok { DoorOutcome::Completed } else { DoorOutcome::Unknown }).await;
        Ok(ok)
    }

    /// [`MeshHandle::emit_async`](crate::MeshHandle::emit_async) through the emit door, as [`emit_in`](Self::emit_in).
    pub async fn emit_async_in(
        &self, kind: impl Into<Arc<str>>, scope: crate::SignalScope, payload: impl Into<bytes::Bytes>, causal: &CausalFields,
    ) -> Result<bool, DoorRefusal> {
        let kind: Arc<str> = kind.into();
        let pass = self.exposure_door_as_node(OP_SIGNAL_EMIT, &signal_resource(&kind), causal, "signal/emit").await?;
        let ok = self.mesh().emit_async(kind, scope, payload.into()).await;
        pass.finish(if ok { DoorOutcome::Completed } else { DoorOutcome::Unknown }).await;
        Ok(ok)
    }
}

impl super::ServiceHandle {
    /// [`deliver_event`](Self::deliver_event) through the mailbox door (Y11): decided under `mailbox.deliver` on
    /// `signal:{kind}` when an exposure rule selects it, with `causal` on the door's record. `Err` is a refusal, and
    /// nothing was delivered.
    pub async fn deliver_event_in(
        &self, target: &crate::NodeId, kind: impl Into<Arc<str>>, payload: impl Into<bytes::Bytes>, causal: &CausalFields,
    ) -> Result<bool, DoorRefusal> {
        let kind: Arc<str> = kind.into();
        let pass = decide(&self.ctx, DoorActor::Node, OP_MAILBOX_DELIVER, &signal_resource(&kind), &params_of(causal), &point(false, "mailbox/deliver")).await?;
        let ok = self.deliver_event(target, kind, payload);
        // Written locally either way; `false` is a full gossip queue, so whether it reached the target is unknown.
        pass.finish(if ok { DoorOutcome::Completed } else { DoorOutcome::Unknown }).await;
        Ok(ok)
    }

    /// [`emit_reliable`](Self::emit_reliable) through the reliable-emit door (Y11): decided under
    /// `overlay.emit_reliable` on `signal:{kind}` when an exposure rule selects it. A timeout is recorded as an unknown
    /// execution — the receiver may have acted — never as nothing.
    pub async fn emit_reliable_in(
        &self, target: crate::NodeId, kind: impl Into<Arc<str>>, payload: impl Into<bytes::Bytes>, timeout: std::time::Duration,
        causal: &CausalFields,
    ) -> Result<super::overlay_reliable::AckResult, DoorRefusal> {
        use super::overlay_reliable::AckResult;
        let kind: Arc<str> = kind.into();
        let pass =
            decide(&self.ctx, DoorActor::Node, OP_OVERLAY_EMIT_RELIABLE, &signal_resource(&kind), &params_of(causal), &point(false, "overlay/emit_reliable"))
                .await?;
        let ack = self.emit_reliable(target, kind, payload, timeout).await;
        pass.finish(match ack {
            AckResult::Acknowledged => DoorOutcome::Completed,
            _ => DoorOutcome::Unknown,
        })
        .await;
        Ok(ack)
    }

    /// [`emit_sharded`](Self::emit_sharded) through the sharded-emit door (Y11): decided under `shard.emit` on
    /// `signal:{kind}` when an exposure rule selects it. A refusal is [`ShardError::Refused`](super::ShardError::Refused).
    pub async fn emit_sharded_in(
        &self, kind: impl Into<Arc<str>>, shard_key: &str, filter: &crate::CapFilter, payload: impl Into<bytes::Bytes>, causal: &CausalFields,
    ) -> Result<crate::NodeId, super::ShardError> {
        let kind: Arc<str> = kind.into();
        let pass = decide(&self.ctx, DoorActor::Node, OP_SHARD_EMIT, &signal_resource(&kind), &params_of(causal), &point(false, "shard/emit"))
            .await
            .map_err(super::ShardError::Refused)?;
        let owner = self.emit_sharded(kind, shard_key, filter, payload).await;
        pass.finish(if owner.is_ok() { DoorOutcome::Completed } else { DoorOutcome::Nothing }).await;
        owner
    }
}

/// A door's refusal as a gateway route answers it: 400 for a malformed `_meta`, 403 otherwise, `{"error", "reason"}`.
#[cfg(feature = "gateway")]
pub(crate) fn refusal_response(r: &DoorRefusal) -> axum::response::Response {
    use axum::response::IntoResponse;
    let status = if r.is_malformed() { axum::http::StatusCode::BAD_REQUEST } else { axum::http::StatusCode::FORBIDDEN };
    (status, axum::Json(serde_json::json!({ "error": r.message, "reason": r.reason }))).into_response()
}

#[cfg(test)]
#[path = "door_tests.rs"]
mod tests;
