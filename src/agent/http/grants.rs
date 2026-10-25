//! The supervisor's door for **grants** (adopter-surface X3): `GET /gateway/grants/pending` (`grant:read`) lists the
//! requests agents have written under `grant/{agent}/{request}` and not yet answered; `POST /gateway/grants/{agent}/{request}`
//! (`grant:issue`) issues or refuses one. Issuing is an action the evaluator sees — *may this principal grant this
//! tool for this agent* — decided by the same preflight every door runs, with the issuer as the actor
//! (`grant.issue`, `tool:{tool}@{agent node}`), so a client holding `grant:issue` is not thereby an approver of
//! everything; a grant issued without that permit is refused and recorded. An issued grant is a scoped mandate
//! (`crate::mandate::grant::GRANT_SCOPE_PREFIX`) signed by **this node's** identity key, with the issuer named in its
//! `purpose` and on the evidence record: the supervisor is the actor of the decision, the node is the signing
//! authority an agent's verifier is entitled to trust (`EntitlementTable::entitle_grants`). The write of `…/issued`
//! answers with the receipt's rung, so a supervisor knows whether the grant has reached their node's disk or only its
//! memory; reaching the agent's node is gossip's, and the agent's wait observes it.

use super::*;
use crate::mandate::grant::{SignedMandateGrant, GRANT_SCOPE_PREFIX};
use crate::mandate::{Mandate, PrincipalId, TermId};

/// The route's own vocabulary on a refusal.
fn refused(status: StatusCode, error: &str, detail: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({ "ok": false, "error": error, "detail": detail.into() }))).into_response()
}

/// Every request record under `grant/` whose `…/issued` and `…/refused` are both absent and whose expiry is ahead.
fn pending(ctx: &TaskCtx, now_ms: u64) -> Vec<serde_json::Value> {
    let entries = crate::store::scan_kv_prefix(&ctx.kv_state, "grant/");
    let answered: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|(k, _)| k.strip_suffix("/issued").or_else(|| k.strip_suffix("/refused")).map(str::to_string))
        .collect();
    let mut out: Vec<serde_json::Value> = entries
        .iter()
        .filter(|(k, _)| k.matches('/').count() == 2 && !answered.contains(k.as_ref()))
        .filter_map(|(k, v)| {
            let mut rec: serde_json::Value = serde_json::from_slice(v).ok()?;
            (rec["expires_at_ms"].as_u64()? > now_ms).then(|| {
                rec["key"] = json!(k.as_ref());
                rec
            })
        })
        .collect();
    out.sort_by_key(|r| r["requested_at_ms"].as_u64().unwrap_or(0));
    out
}

/// `GET /gateway/grants/pending` — the open requests, oldest first.
pub(super) async fn gw_grants_pending(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    let now_ms = ctx.agent_ctx.hlc.decision_now_ms();
    let rows = pending(&ctx.agent_ctx, now_ms);
    set_pending_gauge(&ctx.agent_ctx, now_ms);
    Json(json!({ "ok": true, "pending": rows, "now_ms": now_ms }))
}

/// The gauge moves when a request is written (the state machine) and when one is answered (here), not only when
/// someone polls (#673's review, finding 9).
fn set_pending_gauge(ctx: &TaskCtx, now_ms: u64) {
    #[cfg(feature = "metrics")]
    metrics::gauge!("mycelium_grants_pending").set(super::super::state_machine::pending_grants(ctx, now_ms) as f64);
    #[cfg(not(feature = "metrics"))]
    let _ = (ctx, now_ms);
}

#[derive(serde::Deserialize)]
pub(super) struct GrantDecisionBody {
    /// `"issue"` or `"refuse"`.
    decision: String,
    /// How long the grant stands, from now; default 300 s, at most 3 600 s.
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// A refusal's reason, shown to the agent.
    #[serde(default)]
    reason: Option<String>,
}

/// What a supervisor answers a request with.
pub(crate) enum GrantDecision {
    /// Issue the grant, standing `ttl_secs` from now (default 300 s, at most 3 600 s).
    Issue { ttl_secs: Option<u64> },
    /// Refuse it, with a reason shown to the agent.
    Refuse { reason: Option<String> },
}

/// Why a grant was not issued, refused or revoked — the door's own refusals, and the evaluator's.
#[derive(Debug)]
pub(crate) enum GrantRefusal {
    /// The door's: an unknown or expired request, an answer already written, no identity, a write that failed.
    Door { status: StatusCode, code: &'static str, detail: String },
    /// The evaluator's: the issuer is not admitted to this answer on this resource.
    Policy { reason: &'static str, detail: String, data: serde_json::Value },
}

impl GrantRefusal {
    fn door(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        GrantRefusal::Door { status, code, detail: detail.into() }
    }

    /// The door's answer.
    fn into_response(self) -> axum::response::Response {
        match self {
            GrantRefusal::Door { status, code, detail } => refused(status, code, detail),
            GrantRefusal::Policy { reason, detail, data } => (StatusCode::FORBIDDEN, Json(json!({ "ok": false, "error": reason, "detail": detail, "data": data }))).into_response(),
        }
    }
}

impl std::fmt::Display for GrantRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantRefusal::Door { code, detail, .. } => write!(f, "{code}: {detail}"),
            GrantRefusal::Policy { reason, detail, .. } => write!(f, "{reason}: {detail}"),
        }
    }
}

/// An answer written: the receipt the write returned (`POST /gateway/kv`'s shape, its `key` the record's).
pub(crate) struct GrantOutcome {
    pub receipt: serde_json::Value,
}

/// `POST /gateway/grants/{agent}/{request}` — issue or refuse a pending request.
pub(super) async fn gw_grant_decide(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Path((agent, request)): Path<(String, String)>,
    Json(body): Json<GrantDecisionBody>,
) -> axum::response::Response {
    let caller = caller.map(|Extension(c)| c);
    let decision = match body.decision.as_str() {
        "issue" => GrantDecision::Issue { ttl_secs: body.ttl_secs },
        "refuse" => GrantDecision::Refuse { reason: body.reason },
        other => return refused(StatusCode::BAD_REQUEST, "bad_request", format!("decision must be \"issue\" or \"refuse\", not {other:?}")),
    };
    match decide_grant(&ctx.agent_ctx, caller.as_ref(), &agent, &request, decision).await {
        Ok(outcome) => Json(outcome.receipt).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// Issue or refuse the pending request `grant/{agent}/{request}` as `caller` — the route's whole decision, callable
/// without the route (adopter-surface X3, fourth PR: the `grant` fault class plays the supervisor on a fleet with no
/// gateway port). Issuing and refusing are actions the evaluator decides with the caller as the actor; the grant is
/// signed by this node's identity; the answer and the grant's first checkpoint are written under the request.
pub(crate) async fn decide_grant(ctx: &Arc<TaskCtx>, caller: Option<&ResolvedPrincipal>, agent: &str, request: &str, decision: GrantDecision) -> Result<GrantOutcome, GrantRefusal> {
    let issuer = caller.map(|c| c.principal.clone()).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    if agent.contains('/') || request.contains('/') || agent.is_empty() || request.is_empty() {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", "agent and request are each one key segment"));
    }
    // A grant is issued for an agent on this node, or on a fleet member this node knows the identity key of (adopter-
    // surface X3, second PR): the grant and its checkpoint travel to the agent's node in the KV, where its authority is
    // fed them, and that node's entitlement table decides whether this node may issue (`entitle_grants_from_fleet`).
    // A principal that is no member's is refused by name.
    if agent != gateway_caller::node_principal(&ctx.node_id) && !is_member_agent(ctx, agent) {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "not_a_member", format!("{agent} is neither this node's agent nor a fleet member whose identity key this node holds; a grant is issued for an agent on a member node")));
    }
    let key = format!("grant/{agent}/{request}");
    let Some(bytes) = mycelium_core::ops::kv_get(ctx, &key) else {
        return Err(GrantRefusal::door(StatusCode::NOT_FOUND, "unknown_request", format!("no grant request at {key}")));
    };
    let rec: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_request", format!("{key}: {e}"))),
    };
    let now_ms = ctx.hlc.decision_now_ms();
    if mycelium_core::ops::kv_get(ctx, &format!("{key}/issued")).is_some() || mycelium_core::ops::kv_get(ctx, &format!("{key}/refused")).is_some() {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "already_answered", format!("{key} was already issued or refused")));
    }
    if rec["expires_at_ms"].as_u64().is_none_or(|e| e <= now_ms) {
        return Err(GrantRefusal::door(StatusCode::GONE, "expired", format!("{key} expired; the agent has refused the call")));
    }
    let (Some(tool), Some(digest), Some(resource)) = (rec["tool"].as_str(), rec["arguments_digest"].as_str(), rec["resource"].as_str()) else {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_request", format!("{key} lacks tool, arguments_digest or resource")));
    };

    // Issuing — and refusing, which stops an agent's work just as surely — is an action: the evaluator decides with
    // the issuer as the actor (`grant.issue` / `grant.refuse` on the request's resource). The arguments the policy
    // may need are the request's own facts.
    let operation = match &decision {
        GrantDecision::Issue { .. } => "grant.issue",
        GrantDecision::Refuse { .. } => "grant.refuse",
    };
    let arguments = json!({ "tool": tool, "agent": agent, "arguments_digest": digest, "request": request });
    let preflight = ae_preflight(ctx, caller, operation, resource, &arguments, &json!({}), ENFORCEMENT_POINT_GRANT_ISSUE).await;
    let policy_revision: Option<String> = match &preflight {
        Preflight::Refuse(r) => {
            return Err(GrantRefusal::Policy { reason: r.reason(), detail: r.to_string(), data: r.error_data() });
        }
        Preflight::Proceed { evidence, .. } => Some(evidence.policy_revision.clone()),
        // No evaluator: nothing decides who may grant or refuse, so no one may — a grant is authority, and authority
        // that no policy established is not granted by holding a scope.
        Preflight::Inert => {
            return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_evaluator", "no action evaluator is attached: nothing can decide who may grant, so no answer is written"));
        }
    };
    match decision {
        GrantDecision::Refuse { reason } => {
            let record = json!({ "schema": "mycelium.grant-refusal/1", "request": request, "issuer": issuer, "reason": reason.unwrap_or_default(), "at_ms": now_ms });
            let written = write_record(ctx, format!("{key}/refused"), record.to_string().into_bytes()).await;
            ae_record_execution(ctx, &preflight, if written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_grants_refused_total").increment(1);
            set_pending_gauge(ctx, now_ms);
            written.map(|receipt| GrantOutcome { receipt }).map_err(|e| GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e))
        }
        GrantDecision::Issue { ttl_secs } => {
            let Some(tls) = ctx.tls.get() else {
                return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_identity", "issuing a grant needs this node's identity ([tls]) to sign it"));
            };
            let ttl_ms = ttl_secs.unwrap_or(300).min(3_600) * 1_000;
            let node_principal = gateway_caller::node_principal(&ctx.node_id);
            let (Some(holder), Some(established_by), Some(term)) = (PrincipalId::new(agent), PrincipalId::new(&node_principal), TermId::new(request)) else {
                return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_request", "the agent principal or request id is not a valid identifier"));
            };
            let mandate = Mandate {
                holder,
                established_by,
                purpose: format!("grant issued by {issuer} for request {request}"),
                scope: format!("{GRANT_SCOPE_PREFIX}{request}"),
                operations: Mandate::grant_operations(format!("tools/call:tool:{tool}"), digest, policy_revision.as_deref()),
                epoch: 1,
                term,
                valid_from_ms: now_ms,
                valid_until_ms: now_ms + ttl_ms,
            };
            let signature = mycelium_core::tls::sign_bytes(&tls.signing_key(), &mandate.canonical_bytes()).to_vec();
            let grant = SignedMandateGrant { mandate, signature };
            // A grant's revocation standing must be *known* for it to be established: the issuing node is the grant's
            // authority, so it publishes the first checkpoint for the grant's scope (nothing revoked) — offered to its
            // own execution authority here, and written beside the grant for another node's authority to feed from
            // (carrying it there is the second PR's). A revocation is the next checkpoint naming the term.
            let checkpoint = crate::mandate::authority::RevocationCheckpoint {
                authority: grant.mandate.established_by.clone(),
                scope: grant.mandate.scope.clone(),
                seq: 1,
                issued_at_ms: now_ms,
                revoked: std::collections::BTreeSet::new(),
            };
            let signed_checkpoint = crate::mandate::authority::SignedRevocationCheckpoint {
                signature: mycelium_core::tls::sign_bytes(&tls.signing_key(), &checkpoint.canonical_bytes()).to_vec(),
                checkpoint,
            };
            if let Some(authority) = ctx.execution_authority.get() {
                let _ = authority.offer_checkpoint(&signed_checkpoint, now_ms, &gateway_member_keys(ctx));
            }
            if let Ok(cp) = serde_json::to_vec(&signed_checkpoint) {
                let _ = write_record(ctx, format!("{key}/checkpoint"), cp).await;
            }
            let bytes = match serde_json::to_vec(&grant) {
                Ok(b) => b,
                Err(e) => return Err(GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "serialise", e.to_string())),
            };
            let written = write_record(ctx, format!("{key}/issued"), bytes).await;
            // The execution record says what the write did (#673's review, finding 8).
            ae_record_execution(ctx, &preflight, if written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_grants_issued_total").increment(1);
            set_pending_gauge(ctx, now_ms);
            written.map(|receipt| GrantOutcome { receipt }).map_err(|e| GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e))
        }
    }
}

/// `GET /gateway/grants/stream` (`grant:read`) — the open requests as they come (adopter-surface X3, third PR). On
/// connect, every request standing now, one `grant_request` event each (a supervisor connecting late sees the
/// backlog); then each new request as its record lands; a `grant_answered` event for each answer to a request this
/// stream reported — issued, refused, revoked, by anyone at any node, since the records gossip; an issue followed by
/// a revocation is two — and a `grant_expired` event once for one that lapses unanswered. Woken by writes under
/// `grant/` and by the soonest reported expiry; a request already reported is reported once. The stream's task ends
/// when the client goes.
pub(super) async fn gw_grants_stream(State(ctx): State<Arc<HttpCtx>>) -> axum::response::Response {
    use std::collections::HashSet;
    let tc = Arc::clone(&ctx.agent_ctx);
    let mut rx = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&tc.core)).subscribe_prefix("grant/");
    let (tx, out) = tokio::sync::mpsc::channel::<Event>(256);
    tokio::spawn(async move {
        // The requests this stream reported and not yet seen answered or expired, by key, with their expiry; the
        // answers already sent.
        let (mut reported, mut answered): (std::collections::HashMap<String, u64>, HashSet<String>) = (Default::default(), HashSet::new());
        loop {
            let now_ms = tc.hlc.decision_now_ms();
            for rec in pending(&tc, now_ms) {
                let key = rec["key"].as_str().unwrap_or_default().to_string();
                let expires = rec["expires_at_ms"].as_u64().unwrap_or(u64::MAX);
                if !reported.contains_key(&key) && !answered.iter().any(|a| a.starts_with(&format!("{key}/"))) {
                    reported.insert(key, expires);
                    if tx.send(Event::default().event("grant_request").data(rec.to_string())).await.is_err() {
                        return;
                    }
                }
            }
            // A request this stream reported that expired unanswered: said once (#679's review, finding 6).
            let expired: Vec<String> = reported.iter().filter(|(_, e)| **e <= now_ms).map(|(k, _)| k.clone()).collect();
            for key in expired {
                let answered_already = answered.iter().any(|a| a.starts_with(&format!("{key}/")));
                reported.remove(&key);
                if !answered_already {
                    let data = json!({ "key": key, "request": key.rsplit('/').next().unwrap_or_default(), "outcome": "expired", "at_ms": now_ms });
                    if tx.send(Event::default().event("grant_expired").data(data.to_string())).await.is_err() {
                        return;
                    }
                }
            }
            // An answer to a request this stream reported: the record names the outcome and who answered.
            for (k, v) in crate::store::scan_kv_prefix(&tc.kv_state, "grant/") {
                let Some((base, outcome)) = k.rsplit_once('/').filter(|(_, o)| matches!(*o, "issued" | "refused" | "revoked")) else { continue };
                // An answer of a request this stream reported — or one it reported and has since seen answered (an
                // issue followed by a revocation is two answers, each once).
                let known = reported.contains_key(base) || answered.iter().any(|a| a.starts_with(&format!("{base}/")));
                if !known || !answered.insert(k.to_string()) {
                    continue;
                }
                if outcome != "revoked" {
                    reported.remove(base);
                }
                let detail: serde_json::Value = serde_json::from_slice(&v).unwrap_or_default();
                // An issued record is the signed grant, whose `purpose` names the issuer; a refusal or revocation names
                // the issuer itself.
                let issuer = detail["issuer"].as_str().map(str::to_string).or_else(|| {
                    detail["mandate"]["purpose"].as_str().and_then(|p| p.strip_prefix("grant issued by ")).and_then(|p| p.split(" for request ").next()).map(str::to_string)
                });
                let data = json!({ "key": base, "request": base.rsplit('/').next().unwrap_or_default(), "outcome": outcome, "issuer": issuer, "reason": detail["reason"], "at_ms": detail["at_ms"].as_u64().or_else(|| detail["mandate"]["valid_from_ms"].as_u64()) });
                if tx.send(Event::default().event("grant_answered").data(data.to_string())).await.is_err() {
                    return;
                }
            }
            // A write under `grant/` wakes the scan; the client going ends the task at once, not at the next write
            // (#679's review, finding 7); the expiry of a reported request is checked on a timer, since nothing writes
            // when a request merely lapses.
            let soonest = reported.values().copied().min().map(|e| e.saturating_sub(now_ms).max(50)).unwrap_or(60_000);
            tokio::select! {
                changed = rx.changed() => { if changed.is_err() { return; } }
                _ = tx.closed() => return,
                _ = mycelium_core::sim_seam::sleep_ms("grants/stream-expiry", soonest) => {}
            }
        }
    });
    Sse::new(tokio_stream::StreamExt::map(tokio_stream::wrappers::ReceiverStream::new(out), Ok::<_, std::convert::Infallible>)).keep_alive(KeepAlive::default()).into_response()
}

/// Is `agent` the principal (`node:{ip}:{port}`) of a fleet member whose identity key this node holds?
fn is_member_agent(ctx: &TaskCtx, agent: &str) -> bool {
    agent
        .strip_prefix("node:")
        .and_then(|s| s.parse::<crate::node_id::NodeId>().ok())
        .is_some_and(|n| gateway_member_keys(ctx).contains_key(&n))
}

#[derive(serde::Deserialize, Default)]
pub(super) struct GrantRevokeBody {
    /// Why, shown on the record.
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /gateway/grants/{agent}/{request}/revoke` (`grant:issue`) — revoke an issued grant (adopter-surface X3,
/// second PR). Revoking is an action the evaluator decides (`grant.revoke` on the request's resource, the caller as
/// the actor), as issuing is. The grant's authority is the node that signed it, so a grant is revoked **at the node
/// that issued it** (`409 not_the_issuer` elsewhere): that node publishes the next checkpoint for the grant's scope
/// naming the term, offers it to its own authority — whose sweep then stops work running under the grant — and
/// writes it beside the grant (`…/checkpoint`), where every other node's authority is fed it; `…/revoked` names who
/// and why. A revoked grant is refused at every door as `revoked`; revocation is monotonic.
pub(super) async fn gw_grant_revoke(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Path((agent, request)): Path<(String, String)>,
    body: Option<Json<GrantRevokeBody>>,
) -> axum::response::Response {
    let caller = caller.map(|Extension(c)| c);
    let body = body.map(|Json(b)| b).unwrap_or_default();
    match revoke_grant(&ctx.agent_ctx, caller.as_ref(), &agent, &request, body.reason).await {
        Ok(outcome) => Json(outcome.receipt).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// Revoke the issued grant `grant/{agent}/{request}` as `caller` — the route's whole decision, callable without the
/// route (the `grant` fault class). The next checkpoint for the grant's scope names the term, is offered to this
/// node's authority (whose sweep stops work running under the grant) and written beside the grant; `…/revoked`
/// names who and why.
pub(crate) async fn revoke_grant(ctx: &Arc<TaskCtx>, caller: Option<&ResolvedPrincipal>, agent: &str, request: &str, reason: Option<String>) -> Result<GrantOutcome, GrantRefusal> {
    let issuer = caller.map(|c| c.principal.clone()).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    if agent.contains('/') || request.contains('/') || agent.is_empty() || request.is_empty() {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", "agent and request are each one key segment"));
    }
    let key = format!("grant/{agent}/{request}");
    let Some(bytes) = mycelium_core::ops::kv_get(ctx, &format!("{key}/issued")) else {
        return Err(GrantRefusal::door(StatusCode::NOT_FOUND, "not_issued", format!("no grant was issued at {key}")));
    };
    let grant: SignedMandateGrant = match serde_json::from_slice(&bytes) {
        Ok(g) => g,
        Err(e) => return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", format!("{key}/issued: {e}"))),
    };
    let this_node = gateway_caller::node_principal(&ctx.node_id);
    if grant.mandate.established_by.as_str() != this_node {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "not_the_issuer", format!("the grant was issued by {}; revoke it at that node — only the issuer's key signs the checkpoint that revokes it", grant.mandate.established_by.as_str())));
    }
    if mycelium_core::ops::kv_get(ctx, &format!("{key}/revoked")).is_some() {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "already_revoked", format!("{key} was already revoked")));
    }
    // The tool and the resource come from the **signed** grant — its one enumerated operation and its holder — not
    // from the request record, which any member can overwrite and which may be absent here (#678's review, finding 6).
    let Some(tool) = grant.mandate.operations.iter().find_map(|op| op.strip_prefix("tools/call:tool:").map(str::to_string)) else {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", "the grant enumerates no tool call"));
    };
    let Some(agent_node) = grant.mandate.holder.as_str().strip_prefix("node:") else {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", "the grant's holder is not a node principal"));
    };
    let resource = format!("tool:{tool}@{agent_node}");
    let now_ms = ctx.hlc.decision_now_ms();
    let arguments = json!({ "tool": tool, "agent": agent, "request": request });
    let preflight = ae_preflight(ctx, caller, "grant.revoke", &resource, &arguments, &json!({}), ENFORCEMENT_POINT_GRANT_ISSUE).await;
    match &preflight {
        Preflight::Refuse(r) => {
            return Err(GrantRefusal::Policy { reason: r.reason(), detail: r.to_string(), data: r.error_data() });
        }
        Preflight::Proceed { .. } => {}
        Preflight::Inert => {
            return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_evaluator", "no action evaluator is attached: nothing can decide who may revoke, so no answer is written"));
        }
    }
    let Some(tls) = ctx.tls.get() else {
        return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_identity", "revoking a grant needs this node's identity ([tls]) to sign the checkpoint"));
    };
    // The next checkpoint for the grant's scope: the issue wrote sequence 1 with nothing revoked; this is 2, naming the
    // term. A later revocation of the same grant is refused above, so the sequence is these two.
    let checkpoint = crate::mandate::authority::RevocationCheckpoint {
        authority: grant.mandate.established_by.clone(),
        scope: grant.mandate.scope.clone(),
        seq: 2,
        issued_at_ms: now_ms,
        revoked: std::iter::once(grant.mandate.term.clone()).collect(),
    };
    let signed_checkpoint = crate::mandate::authority::SignedRevocationCheckpoint {
        signature: mycelium_core::tls::sign_bytes(&tls.signing_key(), &checkpoint.canonical_bytes()).to_vec(),
        checkpoint,
    };
    if let Some(authority) = ctx.execution_authority.get() {
        let _ = authority.offer_checkpoint(&signed_checkpoint, now_ms, &gateway_member_keys(ctx));
        // Work admitted under the grant and still running is stopped now, not at the next sweep.
        authority.sweep(now_ms);
    }
    let cp = match serde_json::to_vec(&signed_checkpoint) {
        Ok(b) => b,
        Err(e) => return Err(GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "serialise", e.to_string())),
    };
    let cp_written = write_record(ctx, format!("{key}/checkpoint"), cp).await;
    let record = json!({ "schema": "mycelium.grant-revocation/1", "request": request, "issuer": issuer, "reason": reason.unwrap_or_default(), "at_ms": now_ms, "checkpoint_seq": 2 });
    let written = write_record(ctx, format!("{key}/revoked"), record.to_string().into_bytes()).await;
    ae_record_execution(ctx, &preflight, if written.is_ok() && cp_written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_grants_revoked_total").increment(1);
    written.map(|receipt| GrantOutcome { receipt }).map_err(|e| GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e))
}

/// One owned-namespace write with its receipt — the shape `POST /gateway/kv` answers with — or why it was not made.
async fn write_record(tc: &Arc<TaskCtx>, key: String, value: Vec<u8>) -> Result<serde_json::Value, String> {
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    match mycelium_core::ops::kv_set_with_receipt(tc, op, attempt, Arc::from(key.as_str()), Bytes::from(value), None).await {
        Ok(r) => {
            let mut body = json!({ "ok": true, "key": key, "operation_id": r.operation_id.to_string(), "local_durability": r.local_durability.tag() });
            if let Some(reason) = r.local_durability.failure_reason() {
                body["local_durability_error"] = json!(reason);
            }
            Ok(body)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The route, as it is named in every piece of evidence it produces.
pub(crate) const ENFORCEMENT_POINT_GRANT_ISSUE: &str = "gateway:grants/issue";
