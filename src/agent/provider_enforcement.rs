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

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use super::gateway_caller::{self, ResolvedPrincipal};
use super::rpc::RpcRequest;
use super::TaskCtx;

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

/// **May this node run `req` now?** `Ok(())` for anything that is not protected work, or when
/// provider enforcement is off. Call it after the caller context has been verified and before any
/// handler sees the request.
pub(crate) async fn check(ctx: &Arc<TaskCtx>, req: &RpcRequest) -> Result<(), ProviderRefusal> {
    if !ctx.provider_enforcement.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let kind: &str = req.kind();
    if !super::http::is_protected_kind(&ctx.config, kind) {
        return Ok(());
    }
    if ctx.action_evaluator.get().is_none() {
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

    let me = ctx.node_id.to_string();
    let payload = req.payload();
    let (operation, resource, arguments) = derive(ctx, kind, &payload, claim.as_deref(), &me)?;

    let resolved = ResolvedPrincipal { principal, scopes };
    let params = match &mandate {
        Some(m) => json!({ "_meta": { "mandate": m } }),
        None => Value::Null,
    };
    match super::http::ae_preflight(
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
