//! **Authority where the work happens** (Boundary H closure plan C3).
//!
//! # The gap this closes
//!
//! A provider checked **who** was calling and then ran the call. Whether the caller **may** was
//! decided only at the gateway, in `ae_preflight`, so any path that reached a provider without
//! passing through a gateway skipped the decision: a member calling `rpc_call` directly, or a door
//! nobody had found yet. Revoking an agent's mandate stopped it at the gateway and nowhere else.
//!
//! # What this does
//!
//! With [`GossipAgent::with_provider_enforcement`](crate::GossipAgent::with_provider_enforcement)
//! on, every **protected** RPC this node receives runs the gateway's own preflight, as the
//! enforcement point [`ENFORCEMENT_POINT_PROVIDER`], before any handler sees it. The node's action
//! evaluator decides; a presented mandate is verified by the node's own `ExecutionAuthority`, never
//! taken from a gateway (Boundary H's adversary is a colluding population of members, and a
//! gateway is a member); the decision goes to the node's evidence journal.
//!
//! # Deriving what the call is
//!
//! The preflight needs the operation, the resource and the arguments, derived as the gateway
//! derives them, so the policy and the possession proof mean the same thing at both doors:
//!
//! | Kind | Operation | Resource | Arguments |
//! |---|---|---|---|
//! | `mcp.invoke` | `tools/call` | `tool:{name}@{self}`, from the payload | the call's `arguments` |
//! | `skill.invoke` | `skill.invoke` | the envelope's claim, confirmed as `skill:{ns}/{name}@{self}` for a capability this node advertises | `{"text": payload}` |
//! | any other protected kind | the kind | the envelope's claim, confirmed as `…@{self}` | `{"payload_b64": base64(payload)}` |
//!
//! A claim that names another node, or a skill this node does not serve, is refused: a mandate for
//! one resource does not admit a call routed to another.
//!
//! # Cohort budgets (closure plan C4)
//!
//! With [`GossipAgent::with_cohort_budget`](crate::GossipAgent::with_cohort_budget), a protected call
//! the node admits also takes a place in H6's [`CohortBudget`](crate::knowledge::cohort_budget::CohortBudget)
//! for every cohort its **verified** principal belongs to, in this node's own
//! [`CohortView`](crate::knowledge::cohort::CohortView). The place is held by the [`Admission`] for as
//! long as the call is in flight: across the handler in the MCP loops, for the request's lifetime
//! through `rpc_rx`, and until `/gateway/rpc/respond` for the SDK serve stream. A full pool refuses
//! the call `AtCapacity` (JSON-RPC `-32004`, as the federation edge does), and nothing runs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use base64::Engine as _;
use serde_json::{json, Value};

use super::gateway_caller::{self, ResolvedPrincipal};
use super::rpc::RpcRequest;
use super::TaskCtx;
use crate::knowledge::cohort::{CohortOffer, CohortView, SignedCohortDeclaration};
use crate::knowledge::cohort_budget::{CohortBudget, CohortSlot};
use crate::knowledge::issuer::{MemberKeySource, SharedExternalIssuers};
#[cfg(test)]
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::knowledge::IssuerId;
use crate::node_id::NodeId;

/// What an admitted protected call holds while it is in flight: its cohort-budget places, if a
/// budget is attached. Dropping it releases them.
#[derive(Default)]
pub(crate) struct Admission {
    _slot: Option<CohortSlot>,
    /// Closure plan C10: the running work, when the call acts under an established mandate. The
    /// authority's sweep cancels it when the mandate lapses; dropping it ends the work.
    work: Option<super::gateway_authority::WorkGuard>,
    /// Identity-and-lifecycle I4 (N4): the same call's second registration, under its gateway's `call:` mandate —
    /// what a cancel stops. A call under a client's grant and a `call:` mandate holds both; when the handler's
    /// future drops, both drop, and only a cancelled one writes a stop record.
    call_work: Option<super::gateway_authority::WorkGuard>,
}

impl Admission {
    /// **Resolves when the authority cancels this call's work** — under either registration: the caller's mandate
    /// lapsed, or the call was cancelled (its `call:` term revoked). Never resolves for a call registered under
    /// neither. Awaiting it records that the work acknowledged the cancellation.
    pub(crate) async fn lapsed(&self) {
        match (&self.work, &self.call_work) {
            (Some(a), Some(b)) => tokio::select! { _ = a.cancelled() => {}, _ = b.cancelled() => {} },
            (Some(g), None) | (None, Some(g)) => g.cancelled().await,
            (None, None) => std::future::pending().await,
        }
    }
}

/// **Run a handler under its admission** (closure plan C10): if the call acts under a mandate and
/// the authority cancels it while it runs, the handler's future is dropped (a confirmed stop) and
/// the call answers that its authority lapsed.
pub(crate) async fn run<F>(admission: &Admission, handler: F) -> Result<Value, String>
where
    F: std::future::Future<Output = Result<Value, String>>,
{
    tokio::select! {
        result = handler => result,
        _ = admission.lapsed() => Err("cancelled: the call's authority lapsed while it ran".to_string()),
    }
}

/// Longest an SDK-served call may hold its places without a reply: the gateway's own RPC ceiling.
const PARKED_TTL_MS: u64 = 300_000;

/// H6's cohort budget at this provider (closure plan C4), attached with `with_cohort_budget`.
pub(crate) struct ProviderBudget {
    budget: Arc<CohortBudget>,
    /// Lock-order row 45: read to place a caller (µs), written to offer a declaration. The budget's
    /// own `in_flight` lock (row 42) is taken **under** a read of this one, never the reverse.
    view: RwLock<CohortView>,
    /// Shared (identity-and-lifecycle I1). Not fed client keys: a cohort's authorities are the operator's.
    external: SharedExternalIssuers,
    refusals: AtomicU64,
    /// Lock-order row 46: admissions for requests streamed to an SDK agent, released by
    /// `/gateway/rpc/respond` or after `PARKED_TTL_MS`. Leaf.
    parked: Mutex<HashMap<(NodeId, u64), (u64, Admission)>>,
}

impl ProviderBudget {
    pub(crate) fn new(budget: Arc<CohortBudget>, view: CohortView, external: impl Into<SharedExternalIssuers>) -> Self {
        Self {
            budget,
            view: RwLock::new(view),
            external: external.into(),
            refusals: AtomicU64::new(0),
            parked: Mutex::new(HashMap::new()),
        }
    }

    /// Offer a signed cohort declaration to this provider's view.
    pub(crate) fn offer(&self, signed: &SignedCohortDeclaration, members: &impl MemberKeySource) -> CohortOffer {
        // A snapshot before the view's lock: the shared set's lock is never held with another (lock-order row 77).
        let external = self.external.read().clone();
        self.view.write().unwrap_or_else(|e| e.into_inner()).offer(signed, members, &external)
    }

    /// Calls refused for want of room since the budget was attached.
    pub(crate) fn refusals(&self) -> u64 {
        self.refusals.load(Ordering::Relaxed)
    }

    fn admit(&self, principal: &str, now_ms: u64) -> Result<CohortSlot, ProviderRefusal> {
        // A principal that is not a valid issuer id can belong to no declared cohort: it shares the
        // undeclared pool, like any unplaced caller.
        let issuer = IssuerId::new(principal).unwrap_or_else(|| IssuerId::new("undeclared:invalid-principal").expect("valid"));
        let view = self.view.read().unwrap_or_else(|e| e.into_inner());
        self.budget.admit(&issuer, &view, now_ms).map_err(|refusal| {
            self.refusals.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_provider_cohort_refusals_total").increment(1);
            ProviderRefusal {
                reason: "at_capacity".into(),
                message: format!("{refusal}"),
                code: -32004,
                data: json!({ "reason": "at_capacity", "detail": refusal.to_string() }),
            }
        })
    }
}

/// Hold `admission` for a request streamed to an SDK agent until it replies (or `PARKED_TTL_MS`).
pub(crate) fn park(ctx: &TaskCtx, sender: &NodeId, nonce: u64, admission: Admission) {
    let Some(b) = ctx.cohort_budget.get() else { return };
    let now = ctx.hlc.decision_now_ms();
    let mut parked = b.parked.lock().unwrap_or_else(|e| e.into_inner());
    parked.retain(|_, (at, _)| now.saturating_sub(*at) <= PARKED_TTL_MS);
    parked.insert((sender.clone(), nonce), (now, admission));
}

/// Release what [`park`] held for this request: the SDK agent has replied.
pub(crate) fn release_parked(ctx: &TaskCtx, sender: &NodeId, nonce: u64) {
    if let Some(b) = ctx.cohort_budget.get() {
        b.parked.lock().unwrap_or_else(|e| e.into_inner()).remove(&(sender.clone(), nonce));
    }
}

/// The enforcement point a provider's decisions are recorded under.
pub(crate) const ENFORCEMENT_POINT_PROVIDER: &str = "provider";

/// Why a provider refused a protected call. Every field is safe to return to the caller.
#[derive(Clone, Debug)]
pub(crate) struct ProviderRefusal {
    /// A stable machine-readable reason.
    pub(crate) reason: String,
    /// A human-readable sentence.
    pub(crate) message: String,
    /// The JSON-RPC error code, for MCP replies.
    pub(crate) code: i32,
    /// The structured error data (the preflight's, when it decided).
    pub(crate) data: Value,
}

impl ProviderRefusal {
    fn local(reason: &str, message: String) -> Self {
        Self { reason: reason.to_string(), message, code: -32010, data: json!({ "reason": reason }) }
    }

    /// The reply body for a plain RPC receiver: `{"error", "reason", "data"}`.
    pub(crate) fn rpc_body(&self) -> Vec<u8> {
        json!({ "error": self.message, "reason": self.reason, "data": self.data }).to_string().into_bytes()
    }

    /// The reply body for an MCP tool call: a JSON-RPC error.
    pub(crate) fn jsonrpc_body(&self, id: &Value) -> Vec<u8> {
        json!({ "jsonrpc": "2.0", "id": id,
                "error": { "code": self.code, "message": self.message, "data": self.data } })
            .to_string()
            .into_bytes()
    }
}

/// A provider check still deciding: a seat claim in a fleet-consensus counter (post-360 row K2), which may wait on a
/// consensus round.
pub(crate) type WaitingCheck =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Admission, ProviderRefusal>> + Send + 'static>>;

/// What [`check`] came to: decided now, or a check that must still settle a seat claim.
pub(crate) enum Checked {
    /// Decided.
    Done(Result<Admission, ProviderRefusal>),
    /// The policy permitted the call on a fleet-consensus counter, and its seat is still to be claimed: settle it
    /// beside the serve loop ([`DeferredChecks`]), so one claim does not stall the requests behind it.
    Waiting(WaitingCheck),
}

/// **May this node run `req` now?** An empty admission for anything that is not protected work, or
/// when neither provider enforcement nor a cohort budget is on. Call it after the caller context has
/// been verified and before any handler sees the request, and keep the [`Admission`] until the call
/// is done.
///
/// It decides **from the envelope the preflight itself builds** — the verified caller's scopes, the derived
/// arguments, the presented mandate — whether it must wait on a seat claim (post-360 row K2; #642's review, N1): if
/// it must, the decision is made, the history gate released, and the claim and everything after it returned as a
/// [`Checked::Waiting`] for the serve loop to settle beside itself ([`DeferredChecks`]); otherwise the whole check
/// runs here. Nothing is decided twice and no second envelope is built. Every serve loop on this node — `rpc_rx`,
/// the MCP tool loop, the bridged MCP loop and the `rpc/serve` stream — settles a `Waiting` check that way.
/// `already_verified`: the receive boundary ran `gateway_caller::verify_received` on this request first, so its
/// counting is done; `false` where this check is the request's first verification (the native MCP loop).
pub(crate) async fn check(ctx: &Arc<TaskCtx>, req: &RpcRequest, already_verified: bool) -> Checked {
    let enforcing = ctx.provider_enforcement.load(Ordering::Acquire);
    let budget = ctx.cohort_budget.get();
    if !enforcing && budget.is_none() {
        return Checked::Done(Ok(Admission::default()));
    }
    let kind: &str = req.kind();
    if !super::is_protected_kind(&ctx.config, kind) {
        return Checked::Done(Ok(Admission::default()));
    }
    if enforcing && ctx.action_evaluator.get().is_none() {
        // Fail closed: enforcement was asked for and nothing can decide.
        return Checked::Done(Err(ProviderRefusal::local(
            "no_evaluator",
            "provider enforcement is on and no action evaluator is attached; protected work is refused".into(),
        )));
    }

    let caller = match gateway_caller::verify(ctx, req) {
        Ok(c) => c,
        Err(e) => {
            return Checked::Done(Err(ProviderRefusal::local("caller_context_refused", format!("caller context refused: {e}"))))
        }
    };
    if !already_verified && caller.as_ref().is_some_and(|c| c.misplaced_causal) {
        ctx.note_misplaced_causal("caller_envelope");
    }
    // Identity-and-lifecycle I4: the envelope's second slot, with what binds it — the gateway that sent the envelope
    // (`via`, checked equal to the frame's verified sender, in its node's canonical spelling) and the envelope's id.
    // An unframed call has no envelope and so carries none.
    let slot2 = caller.as_ref().map(|c| CallSlot {
        presented: c.call_mandate.as_deref().cloned(),
        gateway: gateway_caller::node_principal(&c.via),
        envelope_id: c.envelope_id,
    });
    let (principal, scopes, mandate, claim) = match caller {
        Some(c) => (c.principal, c.scopes, c.mandate, c.resource),
        None => (gateway_caller::node_principal(req.sender()), Vec::new(), None, None),
    };
    if !enforcing {
        return Checked::Done(admit_after(ctx, &principal, None).await);
    }
    let me = ctx.node_id.to_string();
    let payload = req.payload();
    let (operation, resource, arguments) = match derive(ctx, kind, &payload, claim.as_deref(), &me) {
        Ok(d) => d,
        Err(refusal) => return Checked::Done(Err(refusal)),
    };
    let resolved = ResolvedPrincipal { principal: principal.clone(), scopes, subject_token: None };
    // The preflight's `_meta`: the mandate from the envelope (C2), and — X4 — the causal fields from the application
    // payload where its kind carries them (`causal_meta`). The exposure policy (Y5) decides on these.
    let mut meta = causal_meta(kind, &payload);
    if let Some(m) = &mandate {
        meta.insert("mandate".into(), m.clone());
    }
    let params = if meta.is_empty() { Value::Null } else { json!({ "_meta": meta }) };
    let decided =
        super::http::ae_decide(ctx, Some(&resolved), &operation, &resource, &arguments, &params, ENFORCEMENT_POINT_PROVIDER, None).await;
    let call = AdmittedCall { principal, mandate, operation, resource, arguments, established_at_preflight: false, permit: None, slot2, cancellable: false };
    match decided {
        super::http::Decided::Done(p) => Checked::Done(after_preflight(ctx, call, p).await),
        super::http::Decided::Settle(settling) if settling.will_claim() => {
            let ctx = Arc::clone(ctx);
            Checked::Waiting(Box::pin(async move { after_preflight(&ctx, call, settling.finish().await).await }))
        }
        super::http::Decided::Settle(settling) => Checked::Done(after_preflight(ctx, call, settling.finish().await).await),
    }
}

/// The preflight has decided: a refusal is the provider's; a permit goes on to [`admit_after`], carrying whether it was
/// decided on an established mandate and the permit itself (its record is the one an execution record cites).
async fn after_preflight(ctx: &Arc<TaskCtx>, mut call: AdmittedCall, p: super::http::Preflight) -> Result<Admission, ProviderRefusal> {
    call.established_at_preflight = refusal_of(&p)?;
    call.cancellable = matches!(&p, super::http::Preflight::Proceed { cancellable: true, .. });
    call.permit = Some(p);
    #[cfg(test)]
    test_hooks::before_admit_after(ctx).await;
    admit_after(ctx, &call.principal, Some(&call)).await
}

/// Test hook: hold a call between its preflight and [`admit_after`], so a test can land a revocation in the window.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::sync::Arc;

    /// `(reached, resume)`: `reached` is notified when a call arrives at the window, which then waits for `resume`.
    pub(crate) type Pause = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

    /// Per node, one call.
    pub(crate) static PAUSE: std::sync::LazyLock<papaya::HashMap<String, Pause>> =
        std::sync::LazyLock::new(papaya::HashMap::new);

    pub(super) async fn before_admit_after(ctx: &super::TaskCtx) {
        let taken = PAUSE.pin().remove(&ctx.node_id.to_string()).cloned();
        if let Some((reached, resume)) = taken {
            reached.notify_one();
            resume.notified().await;
        }
    }
}

/// What an enforced call was decided on, for what follows the decision.
struct AdmittedCall {
    principal: String,
    mandate: Option<Value>,
    operation: String,
    resource: String,
    arguments: Value,
    /// The preflight's permit was decided on this call's mandate found **established** (`Preflight::Proceed`'s
    /// `mandate_established`), so a re-assessment that no longer finds it established refuses the call.
    established_at_preflight: bool,
    /// The preflight's permit, once decided: what an execution record for this call is derived from.
    permit: Option<super::http::Preflight>,
    /// The envelope's second mandate slot (identity-and-lifecycle I4). `None`: the call came in no envelope.
    slot2: Option<CallSlot>,
    /// The permit was decided by a cancellable rule (`Preflight::Proceed`'s `cancellable`, D-2): without an
    /// established `call:` mandate the call is refused.
    cancellable: bool,
}

/// What an envelope carried in its second mandate slot, and what it is bound to.
struct CallSlot {
    /// `cm`, as carried: a `PresentedCallMandate` as JSON, if the gateway minted one.
    presented: Option<Value>,
    /// The gateway that sent the envelope, `node:{via}`.
    gateway: String,
    /// The envelope's id, which the mandate's term must name (`c{n}`).
    envelope_id: Option<u64>,
}

/// A preflight's refusal as the provider's; on admission, whether the permit was decided on an established mandate.
fn refusal_of(p: &super::http::Preflight) -> Result<bool, ProviderRefusal> {
    match p {
        super::http::Preflight::Refuse(refusal) => Err(ProviderRefusal {
            reason: refusal.reason().to_string(),
            message: refusal.to_string(),
            code: refusal.json_rpc_code(),
            data: refusal.error_data(),
        }),
        super::http::Preflight::Proceed { mandate_established, .. } => Ok(*mandate_established),
        // `Inert` cannot happen here (an evaluator is attached).
        super::http::Preflight::Inert => Ok(false),
    }
}

/// After the policy admitted the call (or with enforcement off): the running-work registration and the cohort slot.
async fn admit_after(ctx: &Arc<TaskCtx>, principal: &str, call: Option<&AdmittedCall>) -> Result<Admission, ProviderRefusal> {
    let mut work = None;
    // C10: work acting under an **established** mandate is registered as running, so the
    // authority's sweep can stop it if the mandate lapses while it runs. Asking the authority
    // again duplicates one assessment the preflight made; the evidence record does not carry
    // the binding, and registering work under a mandate that is not established would cancel
    // work the policy admitted without one.
    //
    // But a permit decided **on** an established mandate (`established_at_preflight`) is not one the policy
    // admitted without it: if the mandate is no longer established here — a revocation that landed between the
    // preflight and this re-assessment — the call is refused, not run unregistered where no sweep could stop it.
    let mut lapsed: Option<String> = call.filter(|c| c.established_at_preflight).map(|_| "the mandate could not be re-assessed".to_string());
    if let Some(call) = call
        && let (Some(m), Some(authority)) = (&call.mandate, ctx.execution_authority.get())
        && let Ok(presented) = serde_json::from_value::<super::gateway_authority::PresentedMandate>(m.clone())
    {
        let now = ctx.hlc.decision_now_ms();
        super::gateway_authority::feed_grant_checkpoint(ctx, authority, &presented.grant.mandate, now);
        // Both digest forms, as the preflight assessed them (identity-and-lifecycle I3, §9 O1).
        let assessed = authority.reassess_arguments(principal, &call.operation, &call.resource, &call.arguments, Some(&presented), now, &super::gateway_member_keys(ctx));
        let state = assessed.binding.map(|b| b.state);
        if let Some(why) = lapsed.as_mut() {
            *why = match &state {
                Some(super::action_evaluator::MandateState::Established) => String::new(),
                Some(super::action_evaluator::MandateState::Refused(r)) => format!("refused: {r}"),
                Some(super::action_evaluator::MandateState::Unknown(w)) => format!("not established: {w}"),
                None => "no longer found".to_string(),
            };
        }
        if state == Some(super::action_evaluator::MandateState::Established) {
            let g = &presented.grant.mandate;
            let clock_ctx = Arc::clone(ctx);
            work = Some(authority.begin(
                crate::mandate::authority::AuthorizedWork::new(
                    super::gateway_authority::mandate_operation(&call.operation, &call.resource),
                    Some(g.clone()),
                    g.valid_until_ms,
                ),
                Arc::new(move || clock_ctx.hlc.decision_now_ms()),
            ));
        }
    }
    if let Some(why) = lapsed.filter(|w| !w.is_empty()) {
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_provider_mandate_lapsed_total").increment(1);
        // The permit's record says `Attempted`; this one, citing the same attempt, says nothing ran.
        if let Some(permit) = call.and_then(|c| c.permit.as_ref()) {
            super::http::ae_record_execution(ctx, permit, super::action_evaluator::Execution::None).await;
        }
        // Classified as *authority not established* — the family a revoked mandate refused at the preflight is in, so a
        // client reads the same revocation the same way whichever side of the preflight it landed on; `detail` names it.
        return Err(ProviderRefusal {
            reason: "authority_not_established".into(),
            message: format!("authority not established: the call's mandate was established at the provider's preflight and is no longer ({why}); nothing ran (mandate_lapsed_before_run)"),
            code: -32031,
            data: json!({ "reason": "authority_not_established", "detail": "mandate_lapsed_before_run", "found": why }),
        });
    }

    // Identity-and-lifecycle I4 (D-3): the call's `call:` mandate is assessed **here, once**, immediately before its
    // work is registered — there is no earlier assessment for it to disagree with. Established: the work is
    // registered a second time, under the `call:` mandate, so a cancel stops it. Revoked: the gateway cancelled the
    // call before it began, and it is refused whatever the rule. Anything else: under a cancellable rule (D-2) the
    // call is refused, so stripping or spoiling the slot cannot make it run uncancellable; under any other rule it
    // runs unregistered, as it did before I4.
    let mut call_work = None;
    if let Some(call) = call {
        let (established, found) = assess_call_slot(ctx, call);
        match established {
            Ok(guard) => call_work = guard,
            Err(revoked) if revoked || call.cancellable => {
                #[cfg(feature = "metrics")]
                metrics::counter!("mycelium_provider_call_mandate_refusals_total").increment(1);
                // The permit's record says `Attempted`; this one, citing the same attempt, says nothing ran.
                if let Some(permit) = call.permit.as_ref() {
                    super::http::ae_record_execution(ctx, permit, super::action_evaluator::Execution::None).await;
                }
                let message = if revoked {
                    format!("authority not established: the call was cancelled before it ran ({found}); nothing ran (call_mandate_not_established)")
                } else {
                    format!("authority not established: the rule that permits this call is cancellable, and the call's call: mandate is not established at this provider ({found}); nothing ran (call_mandate_not_established)")
                };
                return Err(ProviderRefusal {
                    reason: "authority_not_established".into(),
                    message,
                    code: -32031,
                    data: json!({ "reason": "authority_not_established", "detail": "call_mandate_not_established", "found": found }),
                });
            }
            Err(_) => {}
        }
    }

    // C4: a place in every cohort the verified principal belongs to, or a refusal. Taken only after
    // the policy admitted the call, so a refused call never holds budget.
    let slot = match ctx.cohort_budget.get() {
        Some(b) => match b.admit(principal, ctx.hlc.decision_now_ms()) {
            Ok(slot) => Some(slot),
            Err(refusal) => {
                // After a permit, the permit's record says `Attempted`; this one says nothing ran.
                if let Some(permit) = call.and_then(|c| c.permit.as_ref()) {
                    super::http::ae_record_execution(ctx, permit, super::action_evaluator::Execution::None).await;
                }
                return Err(refusal);
            }
        },
        None => None,
    };
    Ok(Admission { _slot: slot, work, call_work })
}

/// **Assess a call's second mandate slot and register its work** (identity-and-lifecycle I4). `Ok(Some(guard))`:
/// the `call:` mandate is established and the work registered under it, until the mandate's own window.
/// `Ok(None)` never happens with a slot to assess; `Err(true)`: the term is revoked; `Err(false)`: not established,
/// for the reason given — no envelope, no `cm`, no authority attached, a `cm` that does not parse, a term that is not
/// this envelope's, or a mandate that does not verify.
///
/// **The term must be `c{n}` of the envelope that carried it** (Q9, answered in this PR). `cm` is outside the
/// gateway's envelope signature, and its possession proof binds the operation, the resource and the arguments —
/// not the call. So a relaying member could put another of the same gateway's `call:` mandates, minted for an
/// identical call, into this envelope: the work would be registered under the other call's term and the gateway's
/// cancel of this one would miss it. The envelope's id is inside its signature, so requiring the term to name it
/// closes that with no change to the signed bytes.
fn assess_call_slot(ctx: &Arc<TaskCtx>, call: &AdmittedCall) -> (Result<Option<super::gateway_authority::WorkGuard>, bool>, String) {
    use super::action_evaluator::MandateState;
    let not = |why: &str| (Err(false), why.to_string());
    let Some(slot) = &call.slot2 else { return not("the call came in no caller envelope, which is what carries a call: mandate") };
    let Some(cm) = &slot.presented else { return not("the envelope carries no call: mandate") };
    let Some(authority) = ctx.execution_authority.get() else { return not("this provider has no execution authority to assess a call: mandate") };
    let Ok(presented) = serde_json::from_value::<super::gateway_authority::PresentedCallMandate>(cm.clone()) else {
        return not("the envelope's call: mandate does not parse");
    };
    let m = &presented.grant.mandate;
    match slot.envelope_id {
        Some(n) if m.term.as_str() == format!("c{n}") => {}
        Some(n) => return (Err(false), format!("the call: mandate's term {} is not this envelope's (c{n}): it was minted for another call", m.term.as_str())),
        None => return not("the envelope has no id for a call: mandate's term to name"),
    }
    let now = ctx.hlc.decision_now_ms();
    let assessed = authority.assess_call(&slot.gateway, &call.operation, &call.resource, &call.arguments, &presented, now, &super::gateway_member_keys(ctx), |key| {
        mycelium_core::ops::kv_get(ctx, key)
    });
    match assessed.binding.map(|b| b.state) {
        Some(MandateState::Established) => {
            let clock_ctx = Arc::clone(ctx);
            // `AuthorizedWork::new` clamps the registration to the mandate's own window.
            let guard = authority.begin(
                crate::mandate::authority::AuthorizedWork::new(
                    super::gateway_authority::mandate_operation(&call.operation, &call.resource),
                    Some(m.clone()),
                    m.valid_until_ms,
                ),
                Arc::new(move || clock_ctx.hlc.decision_now_ms()),
            );
            (Ok(Some(guard)), String::new())
        }
        Some(MandateState::Refused(crate::mandate::MandateRefusal::Revoked { term })) => (Err(true), format!("term {term} is revoked")),
        Some(MandateState::Refused(r)) => (Err(false), format!("refused: {r}")),
        Some(MandateState::Unknown(why)) => (Err(false), why),
        None => not("the call: mandate was not assessed"),
    }
}

/// Test hook: take one place in the cohort budget for `principal`, as an in-flight call would (`grant_tests`).
#[cfg(all(test, feature = "compliance"))]
pub(crate) fn test_hold_place(ctx: &TaskCtx, principal: &str) -> Option<CohortSlot> {
    ctx.cohort_budget.get()?.admit(principal, ctx.hlc.decision_now_ms()).ok()
}

/// How many deferred checks a serve loop holds at once (#642's review, N2). Full, it stops draining its channel, so
/// the channel's own bound pushes back on senders again.
pub(crate) const MAX_DEFERRED_CHECKS: usize = 64;

/// Test hook: a per-node bound in place of [`MAX_DEFERRED_CHECKS`].
#[cfg(test)]
pub(crate) static TEST_DEFERRED_BOUND: std::sync::LazyLock<papaya::HashMap<String, usize>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// The deferred checks a serve loop has in flight, each with what it needs to answer its caller (`C`): bounded, and
/// never losing an answer — a check whose task panics answers `authority_not_established`, and one still in flight
/// when the loop ends is handed back to be answered ([`abandon`](Self::abandon)).
pub(crate) struct DeferredChecks<C> {
    set: tokio::task::JoinSet<(u64, Result<Admission, ProviderRefusal>)>,
    carried: HashMap<u64, C>,
    /// Each check's own task, so abandoning the set — or dropping it — stops the check too (#642's review, round 3,
    /// finding 4): a caller answered `withdrawn` has no claim still running behind its answer.
    checks: HashMap<u64, tokio::task::AbortHandle>,
    next_id: u64,
    bound: usize,
}

impl<C> DeferredChecks<C> {
    /// An empty set for `ctx`'s serve loop.
    pub(crate) fn new(ctx: &TaskCtx) -> Self {
        let _ = ctx;
        #[cfg(test)]
        let bound = TEST_DEFERRED_BOUND.pin().get(&ctx.node_id.to_string()).copied().unwrap_or(MAX_DEFERRED_CHECKS);
        #[cfg(not(test))]
        let bound = MAX_DEFERRED_CHECKS;
        Self { set: tokio::task::JoinSet::new(), carried: HashMap::new(), checks: HashMap::new(), next_id: 0, bound }
    }

    /// No check in flight.
    pub(crate) fn is_empty(&self) -> bool {
        self.carried.is_empty()
    }

    /// At the bound: the loop stops taking requests until one completes.
    pub(crate) fn is_full(&self) -> bool {
        self.carried.len() >= self.bound
    }

    /// Settle `waiting` beside the loop, answering with `carry` when it completes. The check runs on its own task,
    /// awaited by one that cannot panic, so a panic in the check comes back as a refusal rather than a lost answer.
    pub(crate) fn spawn(&mut self, waiting: WaitingCheck, carry: C) {
        let id = self.next_id;
        self.next_id += 1;
        self.carried.insert(id, carry);
        let check = tokio::spawn(waiting);
        self.checks.insert(id, check.abort_handle());
        self.set.spawn(async move {
            let result = match check.await {
                Ok(r) => r,
                Err(_) => Err(ProviderRefusal::local(
                    "authority_not_established",
                    "the provider check failed while it was being decided".into(),
                )),
            };
            (id, result)
        });
    }

    /// The next check to complete, with its carry. `None` only when none is in flight.
    pub(crate) async fn next(&mut self) -> Option<(C, Result<Admission, ProviderRefusal>)> {
        loop {
            match self.set.join_next().await? {
                Ok((id, result)) => {
                    self.checks.remove(&id);
                    if let Some(carry) = self.carried.remove(&id) {
                        return Some((carry, result));
                    }
                }
                // Only an abort ends the awaiting task without a result; `abandon` answers those.
                Err(_) => continue,
            }
        }
    }

    /// The loop is ending: stop every check — its claim and its record go no further — and hand back each one still
    /// in flight so its caller is answered. A seat whose proposal had already left may still be decided, holding a
    /// seat no call used; the permit counted provisionally at this point is taken back.
    pub(crate) fn abandon(&mut self) -> Vec<C> {
        for (_, check) in self.checks.drain() {
            check.abort();
        }
        self.set.abort_all();
        self.carried.drain().map(|(_, c)| c).collect()
    }
}

impl<C> Drop for DeferredChecks<C> {
    fn drop(&mut self) {
        for (_, check) in self.checks.drain() {
            check.abort();
        }
    }
}

/// The refusal a deferred check still in flight is answered with when its serve loop ends.
pub(crate) fn withdrawn() -> ProviderRefusal {
    ProviderRefusal::local(
        "authority_not_established",
        "the serve loop ended while this call's authority was being decided".into(),
    )
}

/// The causal fields (X4) a protected call's payload carries to the provider, by kind. `mcp.invoke` is a JSON-RPC
/// `tools/call` and carries them under `params._meta`; a `skill.invoke` payload is the skill's text and an operator's
/// protected kind's is opaque, so **neither carries any** — the provider reads such a call as a root, and the exposure
/// policy never holds it here; the door that dispatched it decided on the request's own `_meta` (`/a2a` for a skill).
/// The caller envelope carries no causal fields by design (X4).
pub(crate) fn causal_meta(kind: &str, payload: &[u8]) -> serde_json::Map<String, Value> {
    let mut meta = serde_json::Map::new();
    if kind == crate::signal::signal_kind::MCP_INVOKE && let Ok(call) = serde_json::from_slice::<Value>(payload) {
        for key in ["correlation", "parent"] {
            if let Some(v) = call["params"]["_meta"].get(key) {
                meta.insert(key.into(), v.clone());
            }
        }
    }
    meta
}

/// The operation, resource and arguments of a protected call, derived as the gateway derives them.
fn derive(
    ctx: &TaskCtx,
    kind: &str,
    payload: &[u8],
    claim: Option<&str>,
    me: &str,
) -> Result<(String, String, Value), ProviderRefusal> {
    if kind == crate::signal::signal_kind::MCP_INVOKE {
        let call: Value = serde_json::from_slice(payload)
            .map_err(|_| ProviderRefusal::local("malformed_call", "the tool call is not JSON".into()))?;
        let name = call["params"]["name"]
            .as_str()
            .ok_or_else(|| ProviderRefusal::local("malformed_call", "the tool call names no tool".into()))?;
        let arguments = call["params"].get("arguments").cloned().unwrap_or_else(|| json!({}));
        return Ok(("tools/call".into(), format!("tool:{name}@{me}"), arguments));
    }

    let Some(resource) = claim else {
        return Err(ProviderRefusal::local(
            "resource_unnamed",
            format!("a protected `{kind}` call must name the resource it acts on"),
        ));
    };
    let Some(key) = resource.strip_suffix(&format!("@{me}")) else {
        return Err(ProviderRefusal::local(
            "resource_not_here",
            format!("the call names {resource}, which is not this node's"),
        ));
    };

    if kind == "skill.invoke" {
        let served = key
            .strip_prefix("skill:")
            .and_then(|id| id.split_once('/'))
            .is_some_and(|(ns, name)| advertises(ctx, ns, name));
        if !served {
            return Err(ProviderRefusal::local(
                "resource_not_served",
                format!("this node does not serve {key}"),
            ));
        }
        return Ok(("skill.invoke".into(), resource.to_string(), json!({ "text": String::from_utf8_lossy(payload) })));
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
    Ok((kind.to_string(), resource.to_string(), json!({ "payload_b64": b64 })))
}

/// Does this node advertise capability `ns/name`, not aged out? An own entry it stopped refreshing — read back
/// from persisted state after a restart, say — has (rule `kv.expiry`; the review of #730); a stamp its clock reads
/// as ahead has not (#734's review). One key read (`capability_ops::own_cap_live`).
fn advertises(ctx: &TaskCtx, ns: &str, name: &str) -> bool {
    super::capability_ops::own_cap_live(&ctx.kv_state, &ctx.node_id, ns, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TlsConfig;
    use crate::knowledge::cohort::CohortDeclaration;
    use crate::{GossipAgent, GossipConfig};
    use bytes::Bytes;
    use ed25519_dalek::SigningKey;
    use std::time::Duration;

    /// **This node's own advertisement, gone stale, is not served.** A node restarted over persisted
    /// `cap/` entries it no longer refreshes reads its old skill back; the advertisement has evaporated
    /// (`CapEntry::is_fresh`, rule `kv.expiry`), so the skill is `resource_not_served` — the same answer
    /// every other reader of `cap/` gives (the review of #730). Seen failing first: `advertises` checked
    /// only that the key existed.
    #[test]
    fn a_stale_own_advertisement_is_not_served() {
        use crate::capability::{CapEntry, Capability};
        use crate::framing::make_gossip_update_stamped;
        use crate::store::apply_and_notify;
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", 9001).unwrap(), GossipConfig::default());
        let ctx = &agent.task_ctx;
        let now = mycelium_core::sim_seam::wall_now_ms();
        for (name, written_ms) in [("live", now), ("gone", now - 60_000)] {
            let entry = CapEntry { capability: Capability::new("demo", name), refresh_interval_ms: 1_000 };
            let key = format!("cap/{}/demo/{name}", ctx.node_id);
            apply_and_notify(&ctx.kv_state, &make_gossip_update_stamped(&ctx.node_id, 5, Arc::from(key.as_str()),
                entry.encode(), false, crate::hlc::pack(written_ms, 0)));
        }
        assert!(advertises(ctx, "demo", "live"));
        assert!(!advertises(ctx, "demo", "gone"), "a stale own advertisement read as served");
    }

    /// **A node whose HLC runs ahead still serves its own skill** (#734's review, finding 5). Its own stamp can
    /// sit more than three refresh intervals ahead of its wall clock — a peer ahead within the drift bound, or
    /// the wall clock stepped back — and the symmetric `is_fresh` would read its own advertisement as
    /// quarantined and refuse the skill `resource_not_served`. The own-key check judges staleness only.
    #[test]
    fn an_own_advertisement_stamped_ahead_is_still_served() {
        use crate::capability::{CapEntry, Capability};
        use crate::framing::make_gossip_update_stamped;
        use crate::store::apply_and_notify;
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", 9002).unwrap(), GossipConfig::default());
        let ctx = &agent.task_ctx;
        let ahead = mycelium_core::sim_seam::wall_now_ms() + 60_000;
        let entry = CapEntry { capability: Capability::new("demo", "ahead"), refresh_interval_ms: 1_000 };
        let key = format!("cap/{}/demo/ahead", ctx.node_id);
        apply_and_notify(&ctx.kv_state, &make_gossip_update_stamped(&ctx.node_id, 5, Arc::from(key.as_str()),
            entry.encode(), false, crate::hlc::pack(ahead, 0)));
        assert!(advertises(ctx, "demo", "ahead"), "an own advertisement stamped ahead was refused");
    }

    /// **Closure plan C4: a declared cohort is capped together, through a real serve loop.**
    ///
    /// Five members of cohort `fleet`, a cap of three, and a serve loop that holds every request it
    /// receives (so each admitted call stays in flight):
    /// - exactly three are held, and two are refused `at_capacity` without reaching the loop;
    /// - an undeclared caller is unaffected by the full cohort, and shares the undeclared pool;
    /// - a principal *named* like the cohort gets nothing from it: membership comes only from the view;
    /// - dropping held requests releases their places, and a member is admitted again.
    /// Y7 review finding 7: a cohort budget attached after `start()` is counted as late (G13), under
    /// its own name, like every other attachment. Seen failing first: `with_cohort_budget` noted nothing.
    #[tokio::test]
    async fn a_cohort_budget_attached_after_start_is_counted_as_late() {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        agent.with_cohort_budget(CohortBudget::new(3, 1), CohortView::trusting([]), TrustedExternalIssuers::new());
        assert_eq!(agent.late_attachments(), 0, "before the boundary nothing is late");
        agent.start().await.unwrap();
        agent.with_cohort_budget(CohortBudget::new(3, 1), CohortView::trusting([]), TrustedExternalIssuers::new());
        assert_eq!(agent.late_attachments(), 1, "after it, counted — even though the second is ignored as a duplicate");
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn a_declared_cohort_is_capped_together_and_nothing_else_is_affected() {
        let operator = SigningKey::from_bytes(&[61u8; 32]);
        let port = crate::test_util::alloc_port();
        let dir = std::env::temp_dir().join(format!("c4-budget-{port}"));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.tls = Some(TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
        cfg.protected_rpc_kinds = vec!["depot.custom".into()];
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.unwrap();

        let acme = IssuerId::new("operator:acme").unwrap();
        let mut external = TrustedExternalIssuers::new();
        external.trust(acme.clone(), operator.verifying_key().to_bytes()).unwrap();
        agent.with_cohort_budget(CohortBudget::new(3, 1), CohortView::trusting([acme.clone()]), external);
        let wall = agent.task_ctx.hlc.decision_now_ms();
        let declaration = CohortDeclaration {
            operator: acme,
            cohort: "fleet".into(),
            seq: 1,
            members: (1..=5).map(|i| IssuerId::new(format!("token:gw/m{i}")).unwrap()).collect(),
            valid_from_ms: 0,
            valid_until_ms: wall + 3_600_000,
        };
        let signed = SignedCohortDeclaration {
            signature: crate::tls::sign_bytes(&operator, &declaration.canonical_bytes()).to_vec(),
            declaration,
        };
        assert_eq!(agent.offer_cohort_declaration(&signed), Some(CohortOffer::Accepted));

        let held: Arc<Mutex<Vec<RpcRequest>>> = Arc::default();
        {
            let (agent, held) = (Arc::clone(&agent), Arc::clone(&held));
            let mut rx = agent.service().rpc_rx("depot.custom");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    held.lock().unwrap().push(req); // never answered: the call stays in flight
                }
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let ctx = Arc::clone(&agent.task_ctx);
        let me = agent.node_id().clone();
        let call = |principal: String| {
            let (ctx, me) = (Arc::clone(&ctx), me.clone());
            async move {
                let framed = gateway_caller::frame_with_context(&ctx, &me, &principal, &[], Bytes::from_static(b"go")).unwrap();
                match super::super::rpc::rpc_call_framed(&ctx, me, Arc::from("depot.custom"), framed, Duration::from_millis(800)).await {
                    Ok(b) => serde_json::from_slice::<Value>(&b).map(|v| v["reason"].as_str().unwrap_or("").to_string()).unwrap_or_default(),
                    Err(_) => "held".to_string(), // admitted, and never answered
                }
            }
        };

        let outcomes = futures_util::future::join_all((1..=5).map(|i| call(format!("token:gw/m{i}")))).await;
        assert_eq!(outcomes.iter().filter(|o| *o == "held").count(), 3, "{outcomes:?}");
        assert_eq!(outcomes.iter().filter(|o| *o == "at_capacity").count(), 2, "{outcomes:?}");
        assert_eq!(held.lock().unwrap().len(), 3, "refused calls never reached the serve loop");
        assert_eq!(agent.cohort_budget_refusals(), 2);

        // The full cohort does not touch the undeclared pool (cap 1): one stranger is admitted, the
        // next is refused by *its* pool, and one named like the cohort is only a stranger.
        assert_eq!(call("token:gw/stranger".into()).await, "held");
        assert_eq!(call("fleet".into()).await, "at_capacity", "a name is not membership");
        assert_eq!(held.lock().unwrap().len(), 4);

        // Dropping held requests releases their places.
        held.lock().unwrap().clear();
        assert_eq!(call("token:gw/m1".into()).await, "held", "a released place admits a member again");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
