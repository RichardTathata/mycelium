//! `POST /mcp` — the JSON-RPC 2.0 MCP bridge.

use super::*;

/// JSON-RPC 2.0 handler for the MCP protocol (`POST /mcp`).
///
/// Dispatches on `method`:
/// - `initialize`   — returns server capabilities.
/// - `tools/list`   — scans `tools/` prefix and returns registered tools.
/// - `tools/call`   — locates a provider and proxies the call via `rpc_call_ctx`.
pub(super) async fn mcp_handler(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let caller = caller.map(|Extension(c)| c);
    // X4: a causal field carried as a header is never read — counted, and the record's field stays empty.
    note_misplaced_causal_headers(&ctx.agent_ctx, &headers, "mcp");
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v)  => v,
        Err(_) => {
            return Json(json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": "parse error"},
            })).into_response();
        }
    };

    let id     = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = req["method"].as_str().unwrap_or("");

    match method {
        "initialize" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "mycelium",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        })).into_response(),

        "tools/list" => {
            let mut tool_map: std::collections::HashMap<String, serde_json::Value>
                = Default::default();
            for (key, bytes) in
                crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, "tools/")
            {
                let rest = key.strip_prefix("tools/").unwrap_or_default();
                let Some((name, _node_id)) = rest.split_once('/') else { continue };
                if tool_map.contains_key(name) { continue; }
                let schema: serde_json::Value =
                    serde_json::from_slice(&bytes).unwrap_or(json!({}));
                tool_map.insert(name.to_string(), schema);
            }
            let tools: Vec<serde_json::Value> = tool_map.into_iter().map(|(name, schema)| {
                let description = schema.get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let mut entry = json!({"name": name, "inputSchema": schema});
                if let Some(desc) = description {
                    entry["description"] = json!(desc);
                }
                entry
            }).collect();
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tools},
            })).into_response()
        }

        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("").to_string();
            let arguments = req["params"]["arguments"].clone();

            if name.is_empty() {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32602, "message": "invalid params: missing tool name"},
                })).into_response();
            }

            let prefix = format!("tools/{name}/");
            let provider = crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, &prefix)
                .into_iter()
                .find_map(|(key, _)| {
                    let rest = key.strip_prefix(&prefix)?;
                    rest.parse::<crate::node_id::NodeId>().ok()
                });

            let Some(provider_node_id) = provider else {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("tool not found: {name}")},
                })).into_response();
            };

            // Where a call's time went, kept for the one answer that cannot say why: a dispatch timeout (it named
            // the #642 door-test flake's cause — a reply lost to registration after the emit, `rpc::PendingReply`).
            let started = mycelium_core::sim_seam::mono_instant();
            let mut tool_req = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            });
            // X4: the two causal fields travel to the provider inside the application payload — the part the caller
            // envelope's signature digests — and nothing else of `_meta` does (the mandate rides in the envelope, C2).
            // A malformed field is refused by the preflight below before this payload is sent. The `parent` the
            // provider receives is **this gateway's decision record** (its content hash, below), never the caller's:
            // the provider's work depends on this decision, and this decision's record carries the caller's parent.
            let caller_causal = match crate::agent::causal::causal_fields(&req["params"]) {
                Ok(c) => c,
                // Refused here in every build (the preflight, where compiled in, would refuse the same way): never
                // corrected, never dropped (#672's review, finding 8).
                Err(e) => {
                    return Json(json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32602, "message": format!("request malformed: {e}"), "data": {"reason": "request_malformed"}},
                    })).into_response();
                }
            };

            // AE slice: the evaluator preflight, between the auth layer and the dispatch. Inert
            // unless an evaluator is attached (`with_action_evaluator`); with one, a call whose
            // authority the policy does not establish never reaches the provider *through this
            // gateway*. A route-level preflight, not enforcement at the effect
            // (`docs/design/action-envelope-ae0.md` §7).
            #[cfg(all(feature = "gateway", feature = "tls"))]
            let preflight = ae_preflight(
                &ctx.agent_ctx,
                caller.as_ref(),
                "tools/call",
                &format!("tool:{name}@{provider_node_id}"),
                &arguments,
                &req["params"],
                ENFORCEMENT_POINT_MCP,
            )
            .await;
            #[cfg(all(feature = "gateway", feature = "tls"))]
            if let Preflight::Refuse(refusal) = &preflight {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": refusal.json_rpc_code(), "message": refusal.to_string(),
                              "data": refusal.error_data()},
                })).into_response();
            }
            // X4: what travels — the caller's correlation, and as parent this decision's hash where the journal
            // established one (an inert gateway, or a lenient one that lost the record, forwards the caller's parent).
            {
                #[cfg_attr(not(all(feature = "gateway", feature = "tls")), allow(unused_mut))]
                let mut forwarded = caller_causal.clone();
                #[cfg(all(feature = "gateway", feature = "tls"))]
                if let Preflight::Proceed { journal_sha256: Some(h), .. } = &preflight {
                    forwarded.parent = Some(h.clone());
                }
                if let Some(meta) = forwarded.meta() {
                    tool_req["params"]["_meta"] = serde_json::Value::Object(meta);
                }
            }

            // Item 7: the call carries the auth layer's caller context (never anything the
            // client put in `params`), or is refused — it is never dispatched as the node.
            // Closure plan C2/C3: the presented mandate, and the resource it is presented for, travel
            // to the provider, which verifies them itself.
            let decided = mycelium_core::sim_seam::mono_instant();
            let resource_claim = format!("tool:{name}@{provider_node_id}");
            let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
                &ctx.agent_ctx,
                caller.as_ref(),
                provider_node_id.clone(),
                std::sync::Arc::from(crate::signal::signal_kind::MCP_INVOKE),
                Bytes::from(tool_req.to_string().into_bytes()),
                mcp_dispatch_timeout(&ctx.agent_ctx.node_id),
                gateway_caller::presented_mandate(&req["params"]),
                Some(&resource_claim),
            ).await;

            // What this gateway actually observed. A timeout is **unknown**, never a negative: the
            // call may well have run (item 1's rule, and the hot invariant). A dispatch the gateway
            // itself refused before sending is the one case where nothing ran and it can say so.
            #[cfg(all(feature = "gateway", feature = "tls"))]
            {
                use super::action_evaluator::Execution;
                let mut observed = observed_execution(&dispatched);
                // A reply that arrived is a completed RPC; whether the tool inside it succeeded is
                // a separate question the consumer reads as `effect: failed`.
                if observed == Execution::Completed
                    && dispatched.as_ref().is_ok_and(|b| reply_reports_failure(b))
                {
                    observed = Execution::Failed;
                }
                ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
            }

            match dispatched {
                Ok(reply_bytes) => {
                    let resp: serde_json::Value = serde_json::from_slice(&reply_bytes)
                        .unwrap_or_else(|_| json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32603, "message": "tool returned invalid JSON"},
                        }));
                    Json(resp).into_response()
                }
                Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => {
                    let now = mycelium_core::sim_seam::mono_instant();
                    let preflight_ms = mycelium_core::sim_seam::mono_span(&started, &decided).as_millis() as u64;
                    let dispatch_ms = mycelium_core::sim_seam::mono_span(&decided, &now).as_millis() as u64;
                    let to_self = provider_node_id == ctx.agent_ctx.node_id;
                    let peers = ctx.agent_ctx.peers.pin().len();
                    let dropped = ctx.agent_ctx.kv_state.dropped_frames.load(std::sync::atomic::Ordering::Relaxed);
                    // Operator diagnostics only: the provider, the mesh's shape and the timings are not the
                    // caller's to see (#642's review round 5, finding 2).
                    warn!(tool = %name, provider = %provider_node_id, preflight_ms, dispatch_ms, to_self, peers, dropped,
                          "mcp tools/call: the dispatch timed out");
                    #[cfg(test)]
                    TEST_LAST_TIMEOUT.pin().insert(
                        ctx.agent_ctx.node_id.to_string(),
                        format!("provider={provider_node_id} to_self={to_self} preflight_ms={preflight_ms} \
                                 dispatch_ms={dispatch_ms} peers={peers} dropped_frames={dropped}"),
                    );
                    Json(json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": "tool invocation timed out"},
                    })).into_response()
                }
                Err(e) => Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": e.json_rpc_code(), "message": e.to_string(),
                              "data": {"reason": e.reason()}},
                })).into_response(),
            }
        }

        _ => Json(json!({
            "jsonrpc": "2.0", "id": id,
            "error": {"code": -32601, "message": format!("method not found: {method}")},
        })).into_response(),
    }
}

/// X4: a `correlation` or `parent` carried as a header (`x-mycelium-correlation`, `x-mycelium-parent`) is outside the
/// signed payload and is never read; it is counted against the door so the misplacement is visible.
pub(crate) fn note_misplaced_causal_headers(ctx: &crate::agent::TaskCtx, headers: &axum::http::HeaderMap, door: &'static str) {
    if crate::agent::causal::MISPLACED_HEADERS.iter().any(|h| headers.contains_key(*h)) {
        ctx.note_misplaced_causal(door);
    }
}

/// How long `/mcp` waits for a tool's answer: 30 s, or a test's override for this gateway node.
fn mcp_dispatch_timeout(_node: &crate::node_id::NodeId) -> Duration {
    #[cfg(test)]
    if let Some(ms) = TEST_DISPATCH_TIMEOUT_MS.pin().get(&_node.to_string()).copied() {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(30)
}

/// Test hook: `/mcp`'s dispatch timeout in milliseconds, per gateway node.
#[cfg(test)]
pub(crate) static TEST_DISPATCH_TIMEOUT_MS: std::sync::LazyLock<papaya::HashMap<String, u64>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// Test hook: the last dispatch timeout's diagnostics, per gateway node — what the `warn!` logs.
#[cfg(test)]
pub(crate) static TEST_LAST_TIMEOUT: std::sync::LazyLock<papaya::HashMap<String, String>> =
    std::sync::LazyLock::new(papaya::HashMap::new);
