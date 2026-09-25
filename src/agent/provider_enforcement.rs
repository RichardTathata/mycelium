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

/// **May this node run `req` now?** An empty admission for anything that is not protected work, or
/// when neither provider enforcement nor a cohort budget is on. Call it after the caller context has
/// been verified and before any handler sees the request, and keep the [`Admission`] until the call
/// is done.
pub(crate) async fn check(ctx: &Arc<TaskCtx>, req: &RpcRequest) -> Result<Admission, ProviderRefusal> {
    let enforcing = ctx.provider_enforcement.load(Ordering::Acquire);
    let budget = ctx.cohort_budget.get();
    if !enforcing && budget.is_none() {
        return Ok(Admission::default());
    }
    let kind: &str = req.kind();
    if !super::http::is_protected_kind(&ctx.config, kind) {
        return Ok(Admission::default());
    }
    if enforcing && ctx.action_evaluator.get().is_none() {
        // Fail closed: enforcement was asked for and nothing can decide.
        return Err(ProviderRefusal::local(
            "no_evaluator",
            "provider enforcement is on and no action evaluator is attached; protected work is refused".into(),
        ));
    }

    let caller = gateway_caller::verify(ctx, req).map_err(|e| {
        ProviderRefusal::local("caller_context_refused", format!("caller context refused: {e}"))
    })?;
    let (principal, scopes, mandate, claim) = match caller {
        Some(c) => (c.principal, c.scopes, c.mandate, c.resource),
        None => (gateway_caller::node_principal(req.sender()), Vec::new(), None, None),
    };

    if enforcing {
        let me = ctx.node_id.to_string();
        let payload = req.payload();
        let (operation, resource, arguments) = derive(ctx, kind, &payload, claim.as_deref(), &me)?;
        let resolved = ResolvedPrincipal { principal: principal.clone(), scopes };
        let params = match &mandate {
            Some(m) => json!({ "_meta": { "mandate": m } }),
            None => Value::Null,
        };
        if let super::http::Preflight::Refuse(refusal) = super::http::ae_preflight(
            ctx,
            Some(&resolved),
            &operation,
            &resource,
            &arguments,
            &params,
            ENFORCEMENT_POINT_PROVIDER,
        )
        .await
        {
            return Err(ProviderRefusal {
                reason: refusal.reason().to_string(),
                message: refusal.to_string(),
                code: refusal.json_rpc_code(),
                data: refusal.error_data(),
            });
        }
        // `Inert` cannot happen here (an evaluator is attached), and `Proceed` is admission.
    }

    // C4: a place in every cohort the verified principal belongs to, or a refusal. Taken only after
    // the policy admitted the call, so a refused call never holds budget.
    let slot = match budget {
        Some(b) => Some(b.admit(&principal, ctx.hlc.decision_now_ms())?),
        None => None,
    };
    Ok(Admission { _slot: slot })
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
                let framed = gateway_caller::frame_with_context(&ctx, &principal, &[], Bytes::from_static(b"go")).unwrap();
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
