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
    if request.contains('/') || agent.is_empty() || request.is_empty() {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", "the agent is named and the request is one key segment"));
    }
    // A grant is issued for an agent on this node, or on a fleet member this node knows the identity key of (adopter-
    // surface X3, second PR): the grant and its checkpoint travel to the agent's node in the KV, where its authority is
    // fed them, and that node's entitlement table decides whether this node may issue (`entitle_grants_from_fleet`).
    // A principal that is no member's is refused by name.
    // Identity-and-lifecycle I1: the path names the holder — a node principal as written, a client principal as itself
    // (the record's `agent`, as the SDKs send it) or as its hex key segment.
    let Some((segment, holder_principal)) = super::gateway_authority::grant_holder_from_path(agent) else {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", format!("{agent} is neither a node principal, a client principal nor a client principal's hex segment")));
    };
    let is_client = !holder_principal.starts_with("node:");
    if is_client {
        // A client holder must be able to prove possession: its certificate is on record here.
        if ctx.client_certs.mark(&holder_principal).is_none() {
            return Err(GrantRefusal::door(StatusCode::CONFLICT, "not_a_member", format!("{holder_principal} has no client certificate on record here; a grant held by a client is issued only to a principal whose current key this node knows")));
        }
    } else if holder_principal != gateway_caller::node_principal(&ctx.node_id) && !is_member_agent(ctx, &holder_principal) {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "not_a_member", format!("{agent} is neither this node's agent nor a fleet member whose identity key this node holds; a grant is issued for an agent on a member node")));
    }
    let key = format!("grant/{segment}/{request}");
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
    // The evaluator sees the holder's principal, a client's as a node's (#743's review, item 8), never its key segment.
    let arguments = json!({ "tool": tool, "agent": holder_principal, "arguments_digest": digest, "request": request });
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
            // On disk or `503 not_durable` (#713's review, round 2): a `200` carrying `local_durability: failed` is the
            // shape the revoke lost; an answer that is not on disk is not an answer the agent can be held to.
            let written = write_record_durable(ctx, format!("{key}/refused"), record.to_string().into_bytes()).await;
            ae_record_execution(ctx, &preflight, if written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
            // Counted only once it stands (#713's review, round 4): a refusal the disk did not take is no refusal, and
            // the pending set it would have left is unchanged — the revoke arm's shape.
            match written {
                Ok(receipt) => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!("mycelium_grants_refused_total").increment(1);
                    set_pending_gauge(ctx, now_ms);
                    Ok(GrantOutcome { receipt })
                }
                Err(e) => Err(GrantRefusal::door(StatusCode::SERVICE_UNAVAILABLE, "not_durable", format!("the refusal was not recorded durably, so no answer stands — the request is still pending; retry once the WAL writer is repaired: {e}"))),
            }
        }
        GrantDecision::Issue { ttl_secs } => {
            let Some(tls) = ctx.tls.get() else {
                return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_identity", "issuing a grant needs this node's identity ([tls]) to sign it"));
            };
            let ttl_ms = ttl_secs.unwrap_or(300).min(3_600) * 1_000;
            let node_principal = gateway_caller::node_principal(&ctx.node_id);
            let (Some(holder), Some(established_by), Some(term)) = (PrincipalId::new(&holder_principal), PrincipalId::new(&node_principal), TermId::new(request)) else {
                return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_request", "the agent principal or request id is not a valid identifier"));
            };
            let mandate = Mandate {
                holder,
                established_by,
                purpose: format!("grant issued by {issuer} for request {request}"),
                scope: format!("{GRANT_SCOPE_PREFIX}{request}"),
                // The resource this decision evaluated is signed beside the binding: the revocation evaluates the same
                // one, with no request record or live provider to read (#743's review, item 1).
                operations: Mandate::grant_operations_on(format!("tools/call:tool:{tool}"), digest, policy_revision.as_deref(), resource),
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
            // The checkpoint is on disk before the grant is: a grant whose first checkpoint a restart cannot find is
            // not established at a verifier (external review 2026-10-31, R3 — the sibling of the use marker).
            match serde_json::to_vec(&signed_checkpoint) {
                Ok(cp) => {
                    if let Err(e) = write_record_durable(ctx, format!("{key}/checkpoint"), cp).await {
                        ae_record_execution(ctx, &preflight, super::action_evaluator::Execution::Failed).await;
                        return Err(GrantRefusal::door(StatusCode::SERVICE_UNAVAILABLE, "not_durable", format!("the grant's checkpoint was not recorded durably, so no grant is issued: {e}")));
                    }
                }
                Err(e) => return Err(GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "serialise", e.to_string())),
            }
            let bytes = match serde_json::to_vec(&grant) {
                Ok(b) => b,
                Err(e) => return Err(GrantRefusal::door(StatusCode::INTERNAL_SERVER_ERROR, "serialise", e.to_string())),
            };
            // On disk or `503 not_durable` (#713's review, round 2), by the path the checkpoint above took — the
            // checkpoint guards this write: a writer that refuses appends refuses it first, and no grant is issued.
            let written = write_record_durable(ctx, format!("{key}/issued"), bytes).await;
            // The execution record says what the write did (#673's review, finding 8).
            ae_record_execution(ctx, &preflight, if written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
            // Counted only once it stands (#713's review, round 4): no grant stands after a refused write, so none is
            // counted issued and the pending set is unchanged — the revoke arm's shape.
            match written {
                Ok(receipt) => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!("mycelium_grants_issued_total").increment(1);
                    set_pending_gauge(ctx, now_ms);
                    Ok(GrantOutcome { receipt })
                }
                Err(e) => Err(GrantRefusal::door(StatusCode::SERVICE_UNAVAILABLE, "not_durable", format!("the grant was not recorded durably, so no grant stands — the request is still pending; retry once the WAL writer is repaired: {e}"))),
            }
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

/// The node providing `tool`, as `/mcp` resolves it (the first `tools/{tool}/{node}` record).
fn tool_provider(ctx: &TaskCtx, tool: &str) -> Option<crate::node_id::NodeId> {
    let prefix = format!("tools/{tool}/");
    crate::store::scan_kv_prefix(&ctx.kv_state, &prefix).into_iter().find_map(|(key, _)| key.strip_prefix(&prefix)?.parse().ok())
}

/// The most requests one client principal may have standing unanswered at once: past it the door refuses by name
/// (`429 too_many_pending`) rather than write another fsynced record and wake every supervisor's stream (#743's
/// review, item 5). A request for the same tool and arguments is the standing one, answered again, not counted twice.
/// **A soft bound:** the scan and the write are not one step, so requests racing at one gateway — or arriving at two,
/// whose records meet only by gossip — can each pass it, and a duplicate can be written beside the standing one. It
/// bounds a client that waits for its answers, not one that sends in parallel; the gateway's rate limits are the
/// hard bound.
pub(crate) const MAX_PENDING_GRANT_REQUESTS_PER_CLIENT: usize = 16;

#[derive(serde::Deserialize)]
pub(super) struct GrantRequestBody {
    /// The tool the grant is for.
    tool: String,
    /// The call's arguments; the grant binds their digest.
    #[serde(default)]
    arguments: serde_json::Value,
    /// The arguments' data class, in the operator's vocabulary. A classified argument set is **withheld** from the
    /// gossiped record (a client has no agent policy listing renderable classes); the grant binds the digest either way.
    #[serde(default)]
    arguments_class: Option<String>,
    /// How long the request stands unanswered; default 60 s, at most 600 s.
    #[serde(default)]
    window_secs: Option<u64>,
}

/// `POST /gateway/grants/request` (`mcp:invoke`) — a **client principal asks for a grant** (identity-and-lifecycle
/// I1). The caller — a principal with a client certificate on record here, whose key can prove possession — names a
/// tool and the call's arguments; the gateway resolves the tool's provider and writes the request under
/// `grant/{hex(principal)}/{request}`, in the record shape a node agent's state machine writes
/// ([`grant_request_record`](super::super::state_machine::grant_request_record)), where a supervisor's
/// `GET /gateway/grants/pending` and `POST /gateway/grants/{agent}/{request}` find it. A request standing for the
/// same tool and arguments is answered again (`"reused": true`), as the agent's door waits on its own; past
/// [`MAX_PENDING_GRANT_REQUESTS_PER_CLIENT`] standing requests the door refuses. The request is sealed into the audit
/// chain where the build keeps one (`compliance`). Answers the request id and key; `GET
/// /gateway/grants/request/{request}` reads the answer. The request id names this node, so two gateways serving one
/// client never write one key.
pub(super) async fn gw_grant_request(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body): Json<GrantRequestBody>,
) -> axum::response::Response {
    let tc = &ctx.agent_ctx;
    let Some(principal) = caller.map(|Extension(c)| c.principal().to_string()) else {
        return refused(StatusCode::UNAUTHORIZED, "no_principal", "a grant is requested by an authenticated principal");
    };
    if principal.starts_with("node:") || tc.client_certs.mark(&principal).is_none() {
        return refused(StatusCode::CONFLICT, "not_a_client_holder", format!("{principal} has no client certificate on record here: a grant held by a client needs a key that proves possession (mycelium tls issue --client, then POST /gateway/identity/client-cert)"));
    }
    if body.tool.is_empty() || body.tool.contains('/') {
        return refused(StatusCode::BAD_REQUEST, "bad_request", "tool is one key segment");
    }
    // Identity-and-lifecycle I3 (§9 O1): the digest a grant binds is RFC 8785, which cannot bind an integer past
    // ±(2^53 − 1) exactly; such arguments are refused here, before a grant is asked for.
    if crate::agent::client_sig::holds_unsafe_integer(&body.arguments) {
        return refused(StatusCode::BAD_REQUEST, "unsafe_integer", "the arguments hold an integer outside +-(2^53 - 1), which a grant cannot bind exactly: send it as a string");
    }
    let now_ms = tc.hlc.decision_now_ms();
    let segment = super::gateway_authority::grant_holder_segment(&principal);
    let prefix = format!("grant/{segment}/");
    let digest = super::action_evaluator::hex32(&super::gateway_authority::arguments_digest(&body.arguments));
    // The caller's standing requests: unanswered and unexpired. One for the same tool and digest is the answer.
    let entries = crate::store::scan_kv_prefix(&tc.kv_state, &prefix);
    let answered: std::collections::HashSet<&str> = entries.iter().filter_map(|(k, _)| k.strip_suffix("/issued").or_else(|| k.strip_suffix("/refused"))).collect();
    let standing: Vec<serde_json::Value> = entries
        .iter()
        .filter(|(k, _)| k.strip_prefix(prefix.as_str()).is_some_and(|rest| !rest.contains('/')) && !answered.contains(k.as_ref()))
        .filter_map(|(k, v)| serde_json::from_slice::<serde_json::Value>(v).ok().map(|mut r| { r["key"] = json!(k.as_ref()); r }))
        .filter(|r| r["expires_at_ms"].as_u64().is_some_and(|e| e > now_ms))
        .collect();
    // A request asked before identity-and-lifecycle I3 recorded the legacy digest form: for the window it is the same
    // request (`deprecations.md` §93), and its answer names the digest its grant will bind.
    if let Some(r) = standing.iter().find(|r| r["tool"] == body.tool.as_str() && super::gateway_authority::recorded_digest_matches(&r["arguments_digest"], &body.arguments)) {
        return Json(json!({ "ok": true, "reused": true, "request": r["request"], "key": r["key"], "resource": r["resource"], "arguments_digest": r["arguments_digest"], "expires_at_ms": r["expires_at_ms"] })).into_response();
    }
    if standing.len() >= MAX_PENDING_GRANT_REQUESTS_PER_CLIENT {
        return refused(StatusCode::TOO_MANY_REQUESTS, "too_many_pending", format!("{principal} has {} grant requests standing unanswered, the most one client may (MAX_PENDING_GRANT_REQUESTS_PER_CLIENT); wait for an answer or an expiry", standing.len()));
    }
    let Some(provider) = tool_provider(tc, &body.tool) else {
        return refused(StatusCode::NOT_FOUND, "no_provider", format!("no node provides {}", body.tool));
    };
    let window_ms = body.window_secs.unwrap_or(60).clamp(1, 600) * 1_000;
    // The id names the writer: two gateways serving one client tick their own clocks, and the key does not name them.
    let request = format!("{:016x}-{}", tc.hlc.tick(), tc.node_id);
    let key = format!("{prefix}{request}");
    let record = super::super::state_machine::grant_request_record(tc, &super::super::state_machine::GrantRequestFacts {
        request: &request,
        agent: &principal,
        node: &provider,
        tool: &body.tool,
        arguments: &body.arguments,
        digest_hex: &digest,
        class: body.arguments_class.as_deref(),
        renderable: &[],
        now_ms,
        expires_at_ms: now_ms + window_ms,
    });
    match write_record_durable(tc, key.clone(), record.to_string().into_bytes()).await {
        Ok(_) => {
            super::super::state_machine::count_grant_requested(tc, now_ms);
            audit_request(tc, &principal, &key, &record);
            Json(json!({ "ok": true, "reused": false, "request": request, "key": key, "resource": record["resource"], "arguments_digest": digest, "expires_at_ms": now_ms + window_ms })).into_response()
        }
        Err(e) => refused(StatusCode::SERVICE_UNAVAILABLE, "not_durable", format!("the request was not recorded durably: {e}")),
    }
}

/// A client's grant request, sealed into the audit chain (`Write` on its key, the caller as the principal, the tool,
/// digest and resource as the detail). A seal that fails is warned about; a build without `compliance` keeps no chain.
fn audit_request(ctx: &Arc<TaskCtx>, principal: &str, key: &str, record: &serde_json::Value) {
    #[cfg(feature = "compliance")]
    {
        let detail = json!({ "tool": record["tool"], "resource": record["resource"], "arguments_digest": record["arguments_digest"], "expires_at_ms": record["expires_at_ms"] }).to_string();
        if let Err(e) = super::audit::seal_and_write(ctx, super::audit::AuditAction::Write, principal, key, super::audit::AuditOutcome::Success, Some(detail)) {
            warn!(key, "a client's grant request was recorded but not audited: {e}");
        }
    }
    #[cfg(not(feature = "compliance"))]
    let _ = (ctx, principal, key, record);
}

/// `GET /gateway/grants/request/{request}` (`mcp:invoke`) — the caller's own request and its answer: `pending`,
/// `issued` (with the signed grant, which the caller presents as `_meta.mandate` with its possession proof),
/// `refused` (with the reason), `revoked`, or `expired`. Only the caller's own requests are read.
pub(super) async fn gw_grant_request_status(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Path(request): Path<String>,
) -> axum::response::Response {
    let tc = &ctx.agent_ctx;
    let Some(principal) = caller.map(|Extension(c)| c.principal().to_string()) else {
        return refused(StatusCode::UNAUTHORIZED, "no_principal", "a grant is read by an authenticated principal");
    };
    if request.is_empty() || request.contains('/') {
        return refused(StatusCode::BAD_REQUEST, "bad_request", "request is one key segment");
    }
    let key = format!("grant/{}/{request}", super::gateway_authority::grant_holder_segment(&principal));
    let read = |suffix: &str| mycelium_core::ops::kv_get(tc, &format!("{key}{suffix}")).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let Some(rec) = read("") else {
        return refused(StatusCode::NOT_FOUND, "unknown_request", format!("no grant request of yours at {request}"));
    };
    let now_ms = tc.hlc.decision_now_ms();
    let body = if let Some(r) = read("/revoked") {
        json!({ "ok": true, "state": "revoked", "revocation": r })
    } else if let Some(g) = read("/issued") {
        json!({ "ok": true, "state": "issued", "grant": g })
    } else if let Some(r) = read("/refused") {
        json!({ "ok": true, "state": "refused", "refusal": r })
    } else if rec["expires_at_ms"].as_u64().is_none_or(|e| e <= now_ms) {
        json!({ "ok": true, "state": "expired", "request": rec })
    } else {
        json!({ "ok": true, "state": "pending", "request": rec })
    };
    Json(body).into_response()
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
    if request.contains('/') || agent.is_empty() || request.is_empty() {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", "the agent is named and the request is one key segment"));
    }
    let Some((segment, holder_principal)) = super::gateway_authority::grant_holder_from_path(agent) else {
        return Err(GrantRefusal::door(StatusCode::BAD_REQUEST, "bad_request", format!("{agent} is neither a node principal, a client principal nor a client principal's hex segment")));
    };
    let key = format!("grant/{segment}/{request}");
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
    // The record is this node's own signature, or it is not this node's grant: a record any member overwrote under the
    // key is not one this node revokes or reads its resource from (#743's review, item 1).
    let Some(tls) = ctx.tls.get() else {
        return Err(GrantRefusal::door(StatusCode::FORBIDDEN, "no_identity", "revoking a grant needs this node's identity ([tls]) to sign the checkpoint"));
    };
    // Any key this node has held counts, not only the current one: a grant issued before a `rotate_identity` is still
    // this node's to revoke (#743's review, round 2). Bytes no key of this node signed are refused.
    let canonical = grant.mandate.canonical_bytes();
    if !own_keys(ctx, tls).iter().any(|k| mycelium_core::tls::verify_bytes(k, &canonical, &grant.signature)) {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", format!("{key}/issued is not signed by any key of this node's identity")));
    }
    if grant.mandate.holder.as_str() != holder_principal {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", format!("{key}/issued is held by {}, not {holder_principal}", grant.mandate.holder.as_str())));
    }
    if mycelium_core::ops::kv_get(ctx, &format!("{key}/revoked")).is_some() {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "already_revoked", format!("{key} was already revoked")));
    }
    // The tool and the resource come from the **signed** grant — its one enumerated operation and its holder — not
    // from the request record, which any member can overwrite and which may be absent here (#678's review, finding 6).
    let Some(tool) = grant.mandate.operations.iter().find_map(|op| op.strip_prefix("tools/call:tool:").map(str::to_string)) else {
        return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", "the grant enumerates no tool call"));
    };
    // The resource the decision evaluates against is the one the issue evaluated, signed into the grant
    // (`bound:resource:`) — never a live re-scan, so a revocation needs no provider (#743's review, item 1). A grant
    // issued before that binding names a node holder, whose own node the agent ran the tool on, as before.
    let resource = match (grant.mandate.grant_resource(), grant.mandate.holder.as_str().strip_prefix("node:")) {
        (Some(r), _) => r.to_string(),
        (None, Some(agent_node)) => format!("tool:{tool}@{agent_node}"),
        (None, None) => return Err(GrantRefusal::door(StatusCode::CONFLICT, "unreadable_grant", "the grant names no resource and its holder is not a node principal")),
    };
    let now_ms = ctx.hlc.decision_now_ms();
    let arguments = json!({ "tool": tool, "agent": holder_principal, "request": request });
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
    // Both records on disk, or the route refuses by name (external review 2026-10-31, R3 — the sibling of the use
    // marker: a `200` with `local_durability: failed` was answered before, and a restart that kept only the WAL
    // re-armed the grant for the rest of its window). The in-memory revocation above already stands for this
    // process — work under the grant is refused here until a restart. Repairing the writer records nothing by
    // itself: **any** restart before the revoke is retried re-arms the grant, so the supervisor owes a retry, once
    // `/health` no longer reports `wal_refusing_appends`. `503` because the refusal is the node's state, not the
    // request's, and a retry is the answer.
    let record = json!({ "schema": "mycelium.grant-revocation/1", "request": request, "issuer": issuer, "reason": reason.unwrap_or_default(), "at_ms": now_ms, "checkpoint_seq": 2 });
    let written = match write_record_durable(ctx, format!("{key}/checkpoint"), cp).await {
        Ok(_) => write_record_durable(ctx, format!("{key}/revoked"), record.to_string().into_bytes()).await,
        Err(e) => Err(e),
    };
    ae_record_execution(ctx, &preflight, if written.is_ok() { super::action_evaluator::Execution::Completed } else { super::action_evaluator::Execution::Failed }).await;
    match written {
        Ok(receipt) => {
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_grants_revoked_total").increment(1);
            Ok(GrantOutcome { receipt })
        }
        Err(e) => Err(GrantRefusal::door(StatusCode::SERVICE_UNAVAILABLE, "not_durable", format!("revocation not recorded durably — it stands in this process only, and any restart before this revoke is retried re-arms the grant; retry once the WAL writer is repaired: {e}"))),
    }
}

/// Every key this node's identity has held — the current one, those its identity record carries (`peer_keys` for
/// itself, the rotation history) and its certificate-anchored ones: the set `gateway_member_keys` holds for this node,
/// read without the `compliance` feature that builds the rest of that map.
fn own_keys(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls) -> Vec<[u8; 32]> {
    let mut keys = vec![tls.verifying_key_bytes()];
    let mut add = |k: &[u8; 32]| {
        if !keys.contains(k) {
            keys.push(*k);
        }
    };
    if let Some(held) = ctx.peer_keys.pin().get(&ctx.node_id) {
        held.iter().for_each(&mut add);
    }
    if let Some(standing) = ctx.key_standing.get(&ctx.node_id) {
        standing.anchored.iter().flatten().for_each(&mut add);
    }
    keys
}

/// [`write_record`]'s durable form: the receipt path's required-sync write — on the WAL and fsynced before it is
/// applied or gossiped — or `Err` naming why the record is not on disk. A node with no `[persistence]` writes in
/// memory and answers `Ok` with `local_durability: not_configured` — the rung `kv_set_with_receipt` reports with no
/// WAL — as [`write_record`] does; that node has never claimed a disk.
async fn write_record_durable(tc: &Arc<TaskCtx>, key: String, value: Vec<u8>) -> Result<serde_json::Value, String> {
    if tc.wal.get().is_none() {
        return write_record(tc, key, value).await;
    }
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    match mycelium_core::ops::kv_set_requiring_sync(tc, op, attempt, Arc::from(key.as_str()), Bytes::from(value), None).await {
        Ok(r) => Ok(json!({ "ok": true, "key": key, "operation_id": r.operation_id.to_string(), "local_durability": r.local_durability.tag() })),
        Err(e) => Err(e.to_string()),
    }
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
