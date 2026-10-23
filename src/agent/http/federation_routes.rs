//! Federation, the consumer side: `/gateway/federation/*`.

use super::*;

// ── Federation: the consumer side (item 2 row 11) ────────────────────────────
//
// The verbs a *local* client — Rust, Python or TypeScript — uses to reach a partner domain. The
// decisions are `crate::federation::client`'s and the refusal vocabulary is
// `federation_http`'s; what lives here is the part that is this file's business, and it is one
// sentence long: **the principal on the credential is the authenticated caller's, and is never
// read from the request body.**
//
// That is item 7's rule at one more boundary. A gateway holds the domain's signing key and the
// partner's trust; the client calling it has neither. If the body could name the principal, any
// local caller with `federation:invoke` could have this domain vouch for a principal it made up,
// and the partner's evidence would record it — authority by assertion, which is exactly the
// confused deputy the axis has now removed twice.

/// `GET /gateway/federation/domain`
#[cfg(feature = "tls")]
pub(super) async fn gw_federation_domain(State(ctx): State<Arc<HttpCtx>>) -> Response {
    super::federation_http::domain_json(&ctx.agent_ctx)
}

/// `GET /gateway/federation/partners`
#[cfg(feature = "tls")]
pub(super) async fn gw_federation_partners(State(ctx): State<Arc<HttpCtx>>) -> Response {
    super::federation_http::partners_json(&ctx.agent_ctx)
}

/// `GET /gateway/federation/catalog/{domain}` — the **last observed** catalogue for one partner.
///
/// Deliberately reads only what this node already holds: no network, so an operator can look at a
/// partner during an outage without the act of looking changing the link's state. `POST
/// /gateway/federation/connect` is the one that goes and asks.
#[cfg(feature = "tls")]
pub(super) async fn gw_federation_catalog(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path(domain): axum::extract::Path<String>,
) -> Response {
    let client = match super::federation_http::client_for(&ctx.agent_ctx, &domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let exports = client.last_catalogue();
    Json(json!({
        "domain": domain,
        "link": super::federation_http::link_word(client.link_state()),
        "observed": exports.is_some(),
        "exports": exports,
    }))
    .into_response()
}

/// `POST /gateway/federation/connect` — fetch a partner's catalogue and bring the link up.
#[cfg(feature = "tls")]
pub(super) async fn gw_federation_connect(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(domain) = body["domain"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing domain"}))).into_response();
    };
    let client = match super::federation_http::client_for(&ctx.agent_ctx, domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match client.connect().await {
        Ok(exports) => Json(json!({
            "domain": domain,
            "link": super::federation_http::link_word(client.link_state()),
            "exports": exports,
        }))
        .into_response(),
        Err(e) => super::federation_http::call_refusal(&e),
    }
}

/// `POST /gateway/federation/call` — invoke one of a partner's exports.
///
/// `repeatable` is the caller's statement about **their own** effect, not ours, and it is the field
/// that decides whether a silent gateway may be retried elsewhere: a repeatable call can be, an
/// at-most-once call comes back `delivery: unknown` instead. It defaults to `false`, because the
/// safe default for an unstated effect is the one that never runs it twice.
#[cfg(feature = "tls")]
pub(super) async fn gw_federation_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let caller = caller.map(|Extension(c)| c);
    super::mcp_bridge::note_misplaced_causal_headers(&ctx.agent_ctx, &headers, "federation");
    let (Some(domain), Some(export)) = (body["domain"].as_str(), body["export"].as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing domain or export"}))).into_response();
    };
    let text = body["text"].as_str().unwrap_or("");
    let repeatability = if body["repeatable"].as_bool().unwrap_or(false) {
        crate::federation::gateway::Repeatability::Repeatable
    } else {
        crate::federation::gateway::Repeatability::AtMostOnce
    };
    let client = match super::federation_http::client_for(&ctx.agent_ctx, domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };

    // AE slice: the same evaluator preflight `/mcp` and `/a2a` run. A federated call is a third
    // door onto an effect, and the argument a2a.rs makes for its own preflight is the argument
    // here verbatim — an enforcement point that can be walked around by choosing a different door
    // is not one. Inert unless an evaluator is attached.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let preflight = ae_preflight(
        &ctx.agent_ctx,
        caller.as_ref(),
        "federation.call",
        &format!("export:{export}@{domain}"),
        &json!({ "domain": domain, "export": export, "text": text }),
        &body,
        ENFORCEMENT_POINT_FEDERATION,
    )
    .await;
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if let Preflight::Refuse(refusal) = &preflight {
        // `sent: false` is a fact about this refusal, not a courtesy: the preflight runs before
        // the client is touched, so no credential was minted and no byte left the process.
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "policy", "detail": refusal.to_string(), "data": refusal.error_data(),
                "sent": false, "delivery": "none",
            })),
        )
            .into_response();
    }

    // Item 7 across the boundary: the principal the partner is told is the one the auth layer
    // resolved. On an open gateway that is `anonymous`, which is honest — this domain is vouching
    // that the caller was anonymous *here* — and is why the federation runbook tells an operator
    // who needs attribution to put a token model in front of these routes.
    let principal = caller
        .as_ref()
        .map(|c| c.principal.clone())
        .unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());

    let outcome = client.call_as(&principal, export, text, repeatability).await;

    // What this gateway observed, in the evidence vocabulary. The mapping is the refusal
    // vocabulary's `delivery` field one more time, and it is the same rule: a refusal this side
    // made is `None`, an answer is `Completed`, and everything else is `Unknown` — never a
    // negative we did not establish.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        use super::action_evaluator::Execution;
        use crate::federation::client::ClientError;
        let observed = match &outcome {
            Ok(_) => Execution::Completed,
            Err(ClientError::Link(_)) | Err(ClientError::Resolve(_)) | Err(ClientError::Principal(_))
            | Err(ClientError::Tls(_)) | Err(ClientError::Egress { .. }) => Execution::None,
            Err(ClientError::Outcome(crate::federation::gateway::CallOutcome::NoCapacity { .. })) => Execution::None,
            Err(ClientError::Refused { .. }) => Execution::Failed,
            Err(_) => Execution::Unknown,
        };
        ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
    }

    match outcome {
        Ok(reply) => Json(json!({ "reply": reply, "sent": true, "delivery": "completed" })).into_response(),
        Err(e) => super::federation_http::call_refusal(&e),
    }
}
