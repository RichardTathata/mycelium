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
use crate::knowledge::issuer::{MemberKeySource, TrustedExternalIssuers};
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
}

impl Admission {
    /// The running-work guard, if this call acts under an established mandate.
    pub(crate) fn work(&self) -> Option<&super::gateway_authority::WorkGuard> {
        self.work.as_ref()
    }
}

/// **Run a handler under its admission** (closure plan C10): if the call acts under a mandate and
/// the authority cancels it while it runs, the handler's future is dropped (a confirmed stop) and
/// the call answers that its authority lapsed.
pub(crate) async fn run<F>(admission: &Admission, handler: F) -> Result<Value, String>
where
    F: std::future::Future<Output = Result<Value, String>>,
{
    match admission.work() {
        Some(work) => tokio::select! {
            result = handler => result,
            _ = work.cancelled() => Err("cancelled: the call's authority lapsed while it ran".to_string()),
        },
        None => handler.await,
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
    external: TrustedExternalIssuers,
    refusals: AtomicU64,
    /// Lock-order row 46: admissions for requests streamed to an SDK agent, released by
    /// `/gateway/rpc/respond` or after `PARKED_TTL_MS`. Leaf.
    parked: Mutex<HashMap<(NodeId, u64), (u64, Admission)>>,
}

impl ProviderBudget {
    pub(crate) fn new(budget: Arc<CohortBudget>, view: CohortView, external: TrustedExternalIssuers) -> Self {
        Self {
            budget,
            view: RwLock::new(view),
            external,
            refusals: AtomicU64::new(0),
            parked: Mutex::new(HashMap::new()),
        }
    }

    /// Offer a signed cohort declaration to this provider's view.
    pub(crate) fn offer(&self, signed: &SignedCohortDeclaration, members: &impl MemberKeySource) -> CohortOffer {
        self.view.write().unwrap_or_else(|e| e.into_inner()).offer(signed, members, &self.external)
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
pub(crate) async fn check(ctx: &Arc<TaskCtx>, req: &RpcRequest) -> Checked {
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
    let (principal, scopes, mandate, claim) = match caller {
        Some(c) => (c.principal, c.scopes, c.mandate, c.resource),
        None => (gateway_caller::node_principal(req.sender()), Vec::new(), None, None),
    };
    if !enforcing {
        return Checked::Done(admit_after(ctx, &principal, None));
    }
    let me = ctx.node_id.to_string();
    let payload = req.payload();
    let (operation, resource, arguments) = match derive(ctx, kind, &payload, claim.as_deref(), &me) {
        Ok(d) => d,
        Err(refusal) => return Checked::Done(Err(refusal)),
    };
    let resolved = ResolvedPrincipal { principal: principal.clone(), scopes };
    let params = match &mandate {
        Some(m) => json!({ "_meta": { "mandate": m } }),
        None => Value::Null,
    };
    let decided =
        super::http::ae_decide(ctx, Some(&resolved), &operation, &resource, &arguments, &params, ENFORCEMENT_POINT_PROVIDER, None).await;
    let call = AdmittedCall { principal, mandate, operation, resource, arguments };
    match decided {
        super::http::Decided::Done(p) => Checked::Done(refusal_of(p).and_then(|()| admit_after(ctx, &call.principal, Some(&call)))),
        super::http::Decided::Settle(settling) if settling.will_claim() => {
            let ctx = Arc::clone(ctx);
            Checked::Waiting(Box::pin(async move {
                refusal_of(settling.finish().await)?;
                admit_after(&ctx, &call.principal, Some(&call))
            }))
        }
        super::http::Decided::Settle(settling) => {
            Checked::Done(refusal_of(settling.finish().await).and_then(|()| admit_after(ctx, &call.principal, Some(&call))))
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
}

/// A preflight's refusal as the provider's.
fn refusal_of(p: super::http::Preflight) -> Result<(), ProviderRefusal> {
    match p {
        super::http::Preflight::Refuse(refusal) => Err(ProviderRefusal {
            reason: refusal.reason().to_string(),
            message: refusal.to_string(),
            code: refusal.json_rpc_code(),
            data: refusal.error_data(),
        }),
        // `Inert` cannot happen here (an evaluator is attached), and `Proceed` is admission.
        _ => Ok(()),
    }
}

/// After the policy admitted the call (or with enforcement off): the running-work registration and the cohort slot.
fn admit_after(ctx: &Arc<TaskCtx>, principal: &str, call: Option<&AdmittedCall>) -> Result<Admission, ProviderRefusal> {
    let mut work = None;
    // C10: work acting under an **established** mandate is registered as running, so the
    // authority's sweep can stop it if the mandate lapses while it runs. Asking the authority
    // again duplicates one assessment the preflight made; the evidence record does not carry
    // the binding, and registering work under a mandate that is not established would cancel
    // work the policy admitted without one.
    if let Some(call) = call
        && let (Some(m), Some(authority)) = (&call.mandate, ctx.execution_authority.get())
        && let Ok(presented) = serde_json::from_value::<super::gateway_authority::PresentedMandate>(m.clone())
    {
        let digest = super::action_evaluator::arguments_digest(&serde_json::to_vec(&call.arguments).unwrap_or_default());
        let now = ctx.hlc.decision_now_ms();
        let assessed = authority.assess(principal, &call.operation, &call.resource, &digest, Some(&presented), now, &super::gateway_member_keys(ctx));
        if assessed.binding.is_some_and(|b| b.state == super::action_evaluator::MandateState::Established) {
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

    // C4: a place in every cohort the verified principal belongs to, or a refusal. Taken only after
    // the policy admitted the call, so a refused call never holds budget.
    let slot = match ctx.cohort_budget.get() {
        Some(b) => Some(b.admit(principal, ctx.hlc.decision_now_ms())?),
        None => None,
    };
    Ok(Admission { _slot: slot, work })
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

/// Does this node advertise capability `ns/name`?
fn advertises(ctx: &TaskCtx, ns: &str, name: &str) -> bool {
    crate::store::scan_kv_prefix(&ctx.kv_state, "cap/").into_iter().any(|(key, _)| {
        !super::capability_ops::is_cap_locality_key(&key)
            && super::capability_ops::parse_cap_key_or_warn("cap/", &key).is_some_and(|(node, n, m)| {
                node == ctx.node_id && n.as_ref() == ns && m.as_ref() == name
            })
    })
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

    /// **Closure plan C4: a declared cohort is capped together, through a real serve loop.**
    ///
    /// Five members of cohort `fleet`, a cap of three, and a serve loop that holds every request it
    /// receives (so each admitted call stays in flight):
    /// - exactly three are held, and two are refused `at_capacity` without reaching the loop;
    /// - an undeclared caller is unaffected by the full cohort, and shares the undeclared pool;
    /// - a principal *named* like the cohort gets nothing from it: membership comes only from the view;
    /// - dropping held requests releases their places, and a member is admitted again.
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
