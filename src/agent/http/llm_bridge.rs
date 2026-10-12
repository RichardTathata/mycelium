//! Prompt templates and the two LLM doors (`/gateway/llm/call`, `/gateway/llm/stream`).

use super::*;

// ── LLM / Prompt Skills gateway handlers ─────────────────────────────────────

#[cfg(feature = "llm")]
pub(super) fn llm_get_prompt_from_kv(
    kv_state: &crate::store::KvState,
    ns: &str,
    name: &str,
) -> Option<crate::agent::prompt::PromptTemplate> {
    use crate::signal::kv_ns;
    let key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    let bytes = kv_state.store.pin().get(key.as_str())
        .and_then(|e| e.data.clone())?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(feature = "llm")]
pub(super) async fn gw_prompts_list(
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let entries: Vec<serde_json::Value> = crate::store::scan_kv_prefix(
        &ctx.agent_ctx.kv_state, kv_ns::PROMPTS,
    )
    .into_iter()
    .filter_map(|(k, _v)| {
        let rest = k.strip_prefix(kv_ns::PROMPTS)?;
        let mut parts = rest.splitn(2, '/');
        let ns   = parts.next()?.to_owned();
        let name = parts.next()?.to_owned();
        if name.is_empty() { return None; }
        llm_get_prompt_from_kv(&ctx.agent_ctx.kv_state, &ns, &name).map(|t| {
            serde_json::json!({
                "ns":          ns,
                "name":        name,
                "max_tokens":  t.max_tokens,
                "temperature": t.temperature,
                "metadata":    t.metadata,
            })
        })
    })
    .collect();
    axum::Json(entries)
}

#[cfg(feature = "llm")]
pub(super) async fn gw_prompt_get(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match llm_get_prompt_from_kv(&ctx.agent_ctx.kv_state, &ns, &name) {
        Some(t) => axum::Json(serde_json::to_value(t).unwrap_or_default())
                       .into_response(),
        None    => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(feature = "llm")]
pub(super) async fn gw_prompt_put(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
    axum::Json(body): axum::Json<crate::agent::prompt::PromptTemplate>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let kv_key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    match serde_json::to_vec(&body) {
        Ok(bytes) => {
            kv_write(&ctx.agent_ctx, Arc::from(kv_key.as_str()), Bytes::from(bytes), false);
            axum::Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(feature = "llm")]
pub(super) async fn gw_prompt_delete(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::new(), true);
    axum::Json(serde_json::json!({"ok": true}))
}

#[cfg(feature = "llm")]
#[derive(serde::Deserialize)]
pub(super) struct LlmCallBody {
    ns:         String,
    name:       String,
    input:      String,
    #[serde(default)]
    context:    std::collections::HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    /// `_meta.mandate` / `_meta.operation_id`, as on `/mcp`: a presented mandate is assessed by the
    /// preflight and carried to the provider.
    #[serde(default, rename = "_meta")]
    meta: serde_json::Value,
}

#[cfg(feature = "llm")]
pub(super) fn default_timeout_ms() -> u64 { 30_000 }

#[cfg(feature = "llm")]
pub(super) async fn gw_llm_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    axum::Json(body): axum::Json<LlmCallBody>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;
    let caller = caller.map(|Extension(c)| c);
    use crate::signal::signal_kind;

    let timeout = gateway_rpc_timeout_ms(body.timeout_ms);
    let filter  = CapFilter::new(body.ns.as_str(), body.name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    let provider_str = providers.first()
        .map(|(id, _)| id.to_string())
        .unwrap_or_default();

    let (target, _) = match providers.into_iter().next() {
        Some(p) => p,
        None => {
            return (StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error":"no_provider","detail":""})))
                .into_response();
        }
    };

    let req = serde_json::json!({
        "prompt":  format!("{}/{}", body.ns, body.name),
        "input":   body.input,
        "context": body.context,
    });
    let payload = Bytes::from(req.to_string().into_bytes());
    let params = serde_json::json!({ "_meta": body.meta });
    let resource = format!("prompt:{}/{}@{target}", body.ns, body.name);

    // AE slice: the same evaluator preflight `/mcp` and `/a2a` run. This is the door the raw routes
    // send `llm.invoke` to, so a policy denying it was walked around by choosing this door.
    #[cfg(feature = "tls")]
    let preflight = ae_preflight(
        &ctx.agent_ctx,
        caller.as_ref(),
        signal_kind::LLM_INVOKE,
        &resource,
        &serde_json::json!({ "input": body.input, "context": body.context }),
        &params,
        ENFORCEMENT_POINT_LLM_CALL,
    )
    .await;
    #[cfg(feature = "tls")]
    if let Preflight::Refuse(refusal) = &preflight {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "policy", "detail": refusal.to_string(), "data": refusal.error_data(),
            })),
        )
            .into_response();
    }

    let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
        &ctx.agent_ctx,
        caller.as_ref(),
        target,
        Arc::from(signal_kind::LLM_INVOKE),
        payload,
        timeout,
        gateway_caller::presented_mandate(&params),
        Some(&resource),
    ).await;
    #[cfg(feature = "tls")]
    {
        use super::action_evaluator::Execution;
        let mut observed = observed_execution(&dispatched);
        if observed == Execution::Completed && dispatched.as_ref().is_ok_and(|b| reply_reports_failure(b)) {
            observed = Execution::Failed;
        }
        ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
    }
    match dispatched {
        Ok(reply) => {
            let v: serde_json::Value = serde_json::from_slice(&reply)
                .unwrap_or_else(|_| serde_json::json!({"error":"parse_error","detail":""}));
            if v.get("error").is_some() {
                // Provider-side failure forwarded to the caller: upstream error.
                return (StatusCode::BAD_GATEWAY, axum::Json(v)).into_response();
            }
            axum::Json(serde_json::json!({
                "output":   v["output"],
                "provider": provider_str,
            })).into_response()
        }
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) =>
            (StatusCode::GATEWAY_TIMEOUT,
                axum::Json(serde_json::json!({"error":"timeout","detail":""})))
                .into_response(),
        Err(e) => dispatch_refused(e),
    }
}

#[cfg(feature = "llm")]
#[derive(serde::Deserialize)]
pub(super) struct LlmStreamBody {
    ns:      String,
    name:    String,
    input:   String,
    #[serde(default)]
    context: std::collections::HashMap<String, String>,
    /// As on `/gateway/llm/call`.
    #[serde(default, rename = "_meta")]
    meta: serde_json::Value,
}

#[cfg(feature = "llm")]
pub(super) async fn gw_llm_stream(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    axum::Json(body): axum::Json<LlmStreamBody>,
) -> impl IntoResponse {
    use axum::response::sse::Event;
    let caller = caller.map(|Extension(c)| c);
    use crate::capability::CapFilter;
    use futures_util::stream;

    // v1: buffer full response via RPC, emit as single "done" event.
    // Errors are reported as in-stream `{"type":"error",...}` events, not HTTP
    // status codes: SSE commits the status line before the body, so this is
    // the only legible channel once streaming starts (deliberate asymmetry
    // with gw_llm_call, which uses 404/502/504).
    let timeout = std::time::Duration::from_secs(30);
    let filter  = CapFilter::new(body.ns.as_str(), body.name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    let event = match providers.into_iter().next() {
        None => {
            let data = serde_json::json!({"type":"error","error":"no_provider"}).to_string();
            Event::default().data(data)
        }
        Some((target, _)) => llm_stream_event(&ctx, caller.as_ref(), body, target, timeout).await,
    };

    Sse::new(stream::once(async move { Ok::<_, std::convert::Infallible>(event) }))
}

/// `/gateway/llm/stream`'s one event for a resolved provider: the preflight, the dispatch, the record.
/// A refusal or failure is an in-stream `{"type":"error",…}` event (the status line is already sent).
#[cfg(feature = "llm")]
pub(super) async fn llm_stream_event(
    ctx: &Arc<HttpCtx>,
    caller: Option<&ResolvedPrincipal>,
    body: LlmStreamBody,
    target: crate::node_id::NodeId,
    timeout: std::time::Duration,
) -> axum::response::sse::Event {
    use axum::response::sse::Event;
    use crate::signal::signal_kind;
    let req = serde_json::json!({
        "prompt":  format!("{}/{}", body.ns, body.name),
        "input":   body.input,
        "context": body.context,
    });
    let payload = Bytes::from(req.to_string().into_bytes());
    let params = serde_json::json!({ "_meta": body.meta });
    let resource = format!("prompt:{}/{}@{target}", body.ns, body.name);
    // The same preflight as `/gateway/llm/call`; a refusal is the stream's one event.
    #[cfg(feature = "tls")]
    let preflight = ae_preflight(
        &ctx.agent_ctx,
        caller,
        signal_kind::LLM_INVOKE,
        &resource,
        &serde_json::json!({ "input": body.input, "context": body.context }),
        &params,
        ENFORCEMENT_POINT_LLM_STREAM,
    )
    .await;
    #[cfg(feature = "tls")]
    if let Preflight::Refuse(refusal) = &preflight {
        let data = serde_json::json!({
            "type": "error", "error": "policy", "detail": refusal.to_string(), "data": refusal.error_data(),
        }).to_string();
        return Event::default().data(data);
    }
    let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
        &ctx.agent_ctx,
        caller,
        target,
        Arc::from(signal_kind::LLM_INVOKE),
        payload,
        timeout,
        gateway_caller::presented_mandate(&params),
        Some(&resource),
    ).await;
    #[cfg(feature = "tls")]
    {
        use super::action_evaluator::Execution;
        let mut observed = observed_execution(&dispatched);
        if observed == Execution::Completed && dispatched.as_ref().is_ok_and(|b| reply_reports_failure(b)) {
            observed = Execution::Failed;
        }
        ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
    }
    match dispatched {
        Ok(reply) => {
            let v: serde_json::Value = serde_json::from_slice(&reply)
                .unwrap_or_else(|_| serde_json::json!({"error":"parse_error"}));
            let output = v["output"].as_str().unwrap_or("").to_owned();
            let data = serde_json::json!({"type":"done","output":output}).to_string();
            Event::default().data(data)
        }
        Err(GatewayDispatchError::Rpc(_)) => {
            let data = serde_json::json!({"type":"error","error":"timeout"}).to_string();
            Event::default().data(data)
        }
        Err(e) => {
            let data = serde_json::json!({"type":"error","error":e.reason(),"detail":e.to_string()}).to_string();
            Event::default().data(data)
        }
    }
}
