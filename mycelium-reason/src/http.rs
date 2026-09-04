//! HTTP gateway endpoints (feature `gateway`) — `/gateway/reason/*`.
//!
//! Registered onto the Mycelium embedded gateway via
//! [`GossipAgent::with_http_routes`](mycelium::GossipAgent::with_http_routes) (routers
//! merge; every merged `/gateway/…` path sits behind the gateway auth boundary — bearer,
//! then scope — since 2026-09-04; before that the claim was made but the layer was not
//! applied to merged routers). This is the boundary the Python LangGraph checkpointer
//! speaks: blob PUT/GET carry **raw bytes** (checkpoint payloads — no base64 inflation),
//! the trace endpoint returns JSON events + narrative.
//!
//! ## The OpenAI-compatible façade — `/gateway/reason/v1/*`
//!
//! `POST /gateway/reason/v1/chat/completions` and `GET /gateway/reason/v1/models` speak
//! the OpenAI chat wire shape, so **any OpenAI-compatible client becomes a mesh client by
//! changing its base URL** to `http://node:port/gateway/reason/v1` (and, when the gateway
//! is token-protected, using the gateway bearer as its API key). The `model` field is the
//! `llm/{model}` capability; the call is routed exactly as `/gateway/reason/route` routes
//! it — same [`InferenceRouter`], same reservations, same failover. This is the drop-in
//! adoption path NVIDIA's PAIR proxy demonstrates for one machine's engines, applied to a
//! fleet: nothing central answers, the node you point at routes.
//!
//! Mapping, stated honestly (the served side is a *prompt skill*, not a raw chat model):
//! the last `user` message is the skill's `input`; `system` messages are joined into
//! context `system`; earlier non-system turns are rendered `role: content` per line into
//! context `history` — a template that wants them says `{{system}}` / `{{history}}`,
//! the default `{{input}}` template ignores them. `max_tokens` / `temperature` are
//! template-bound on the serving node and the request's values are not applied.
//! `stream: true` is honoured as a one-chunk SSE stream (the mesh RPC is not streamed),
//! so streaming clients work unchanged. `usage.total_tokens` is what the backend
//! reported; the prompt/completion split is not known and is reported as `0`. An
//! optional top-level `run_id` records the route to that run's trace, as on `/route`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::Deserialize;
use serde_json::json;

use mycelium::{CapFilter, GossipAgent};

use crate::blob::{BlobId, FsBlobStore, MAX_BLOB_BYTES, MeshBlobStore};
use crate::route::{InferenceRouter, ModelQuery, RouteError, Routed, RouterConfig};
use crate::trace::{TraceRecorder, narrate, replay};

/// Shared route state: the agent (trace replay), the local-first/mesh-fallback store, and
/// **one** router — shared so its in-flight reservations span every concurrent gateway
/// request (a router per request would see an empty reservation map every time).
#[derive(Clone)]
struct ReasonState {
    agent: Arc<GossipAgent>,
    blobs: MeshBlobStore,
    router: Arc<InferenceRouter>,
}

/// An axum `Router` with the reason gateway endpoints, ready for
/// `GossipAgent::with_http_routes`. Mesh blob fetches (a GET whose id is not local)
/// use a 10 s per-provider timeout; routing uses [`RouterConfig::default`] — see
/// [`reason_router_with`] to tune it.
pub fn reason_router(agent: Arc<GossipAgent>, store: Arc<FsBlobStore>) -> axum::Router {
    reason_router_with(agent, store, RouterConfig::default())
}

/// [`reason_router`] with an explicit routing policy for `/route` and the `/v1` façade.
pub fn reason_router_with(
    agent: Arc<GossipAgent>,
    store: Arc<FsBlobStore>,
    router_cfg: RouterConfig,
) -> axum::Router {
    let state = ReasonState {
        blobs: MeshBlobStore::new(Arc::clone(&agent), store, Duration::from_secs(10)),
        router: Arc::new(InferenceRouter::new(Arc::clone(&agent), router_cfg)),
        agent,
    };
    axum::Router::new()
        .route(
            "/gateway/reason/blob",
            // Axum's default body cap (2 MiB) is under the blob ceiling; lift it to the
            // ceiling + 1 KiB slack so *our* 413 fires with the JSON error body.
            put(gw_blob_put).layer(DefaultBodyLimit::max(MAX_BLOB_BYTES + 1024)),
        )
        .route("/gateway/reason/blob/{id}", get(gw_blob_get))
        .route("/gateway/reason/trace/{run_id}", get(gw_trace_get))
        .route("/gateway/reason/route", post(gw_route))
        .route("/gateway/reason/v1/chat/completions", post(gw_openai_chat))
        .route("/gateway/reason/v1/models", get(gw_openai_models))
        .with_state(state)
}

/// Body of `POST /gateway/reason/route`. Gateway v1 is intentionally
/// constraint-free — `ModelQuery::constraints` (typed metadata filtering over the
/// `llm-meta/{model}` ad) is a Rust-API-only feature; the JSON boundary carries just
/// model + input + context.
#[derive(Deserialize)]
struct RouteBody {
    model: String,
    input: String,
    #[serde(default)]
    context: HashMap<String, String>,
    /// When set, the route decision + each `llm_call` attempt are recorded to the run's
    /// trace (log stream `reason/{run_id}/{node}`), fetchable via `GET
    /// /gateway/reason/trace/{run_id}` — so a Python driver can produce a replayable,
    /// causal trace of routed inference (rung 5). Omitted → no trace (back-compat).
    #[serde(default)]
    run_id: Option<String>,
}

fn error_json(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

/// `PUT /gateway/reason/blob` — raw body in, `{"id":"<hex>"}` out. 413 over the ceiling.
async fn gw_blob_put(State(s): State<ReasonState>, body: bytes::Bytes) -> Response {
    if body.len() > MAX_BLOB_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "too_large");
    }
    match s.blobs.put(&body) {
        Ok(id) => Json(json!({ "id": id.to_hex() })).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "gateway blob put failed");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    }
}

/// `GET /gateway/reason/blob/{id}` — local-then-mesh; body = verified blob bytes.
async fn gw_blob_get(State(s): State<ReasonState>, Path(id): Path<String>) -> Response {
    let Some(id) = BlobId::from_hex(&id) else {
        return error_json(StatusCode::BAD_REQUEST, "bad_id");
    };
    match s.blobs.get(&id).await {
        Some(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        None => error_json(StatusCode::NOT_FOUND, "not_found"),
    }
}

/// `GET /gateway/reason/trace/{run_id}` — the replayed run + its narrative, from this
/// node's KV view (gossip-replicated: any node can serve any run's trace).
async fn gw_trace_get(State(s): State<ReasonState>, Path(run_id): Path<String>) -> Response {
    let events = replay(&s.agent, &run_id);
    let narrative = narrate(&events);
    let events_json: Vec<_> = events
        .iter()
        .map(|e| json!({ "hlc": e.hlc, "node": e.node, "kind": e.kind, "detail": e.detail }))
        .collect();
    Json(json!({ "run_id": run_id, "events": events_json, "narrative": narrative })).into_response()
}

/// `POST /gateway/reason/route` — load-aware, failover-capable inference routing over
/// `llm/{model}` providers (wedge ①), the mesh-native counterpart to the single-shot
/// `/gateway/llm/call`. The [`InferenceRouter`] call side is core-only, so this route
/// compiles under `gateway` alone (no `llm`): a gateway node can route inference to
/// models served elsewhere without serving any itself.
///
/// Success → `200 {"output","model_used","tokens_used","provider":"<node>","attempt"}`.
/// No live provider → `404 {"error":"no_provider"}`; every candidate failed →
/// `502 {"error":"exhausted","detail":"<per-node failures>"}`.
async fn gw_route(State(s): State<ReasonState>, Json(body): Json<RouteBody>) -> Response {
    let query = ModelQuery::new(body.model);
    // Record a trace only when the caller supplied a run_id (rung 5); otherwise the
    // route is untraced, exactly as before.
    let recorder = body.run_id.map(|id| TraceRecorder::new(Arc::clone(&s.agent), id));
    match s.router.call(&query, &body.input, &body.context, recorder.as_ref()).await {
        Ok(routed) => Json(json!({
            "output": routed.output,
            "model_used": routed.model_used,
            "tokens_used": routed.tokens_used,
            "provider": routed.provider.to_string(),
            "attempt": routed.attempt,
        }))
        .into_response(),
        Err(RouteError::NoProvider) => error_json(StatusCode::NOT_FOUND, "no_provider"),
        Err(e @ RouteError::Exhausted(_)) => {
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": "exhausted", "detail": e.to_string() })))
                .into_response()
        }
    }
}

// ── The OpenAI-compatible façade ──────────────────────────────────────────────

/// One message of an OpenAI chat request. `content` is a string or the parts array
/// (`[{"type":"text","text":…}, …]`); only text parts are read.
#[derive(Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
}

impl ChatMessage {
    fn text(&self) -> String {
        match &self.content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

/// `POST /gateway/reason/v1/chat/completions` body — the OpenAI chat shape. Unknown
/// fields (`temperature`, `max_tokens`, `tools`, …) are accepted and ignored: the served
/// prompt skill binds them (module doc).
#[derive(Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
    /// Mycelium extension: record the route to this run's trace.
    #[serde(default)]
    run_id: Option<String>,
}

/// OpenAI's error envelope: `{"error":{"message","type","code","param"}}`.
fn openai_error(status: StatusCode, message: String, kind: &str, code: &str, param: Option<&str>) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "type": kind, "code": code, "param": param } })),
    )
        .into_response()
}

static COMPLETION_SEQ: AtomicU64 = AtomicU64::new(0);

fn completion_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("chatcmpl-{now:x}{:04x}", COMPLETION_SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Split OpenAI messages into the prompt skill's `(input, context)`: last `user` message
/// → `input`; `system` messages → context `system`; earlier non-system turns → context
/// `history` (`role: content` lines). `None` when there is no user message to route.
fn messages_to_skill_call(messages: &[ChatMessage]) -> Option<(String, HashMap<String, String>)> {
    let last_user = messages.iter().rposition(|m| m.role == "user")?;
    let input = messages[last_user].text();
    let system = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(ChatMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    let history = messages[..last_user]
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| format!("{}: {}", m.role, m.text()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut context = HashMap::new();
    context.insert("system".to_owned(), system);
    context.insert("history".to_owned(), history);
    Some((input, context))
}

fn chat_completion_json(id: &str, created: u64, routed: &Routed) -> serde_json::Value {
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": routed.model_used,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": routed.output },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": routed.tokens_used,
        },
        "mycelium": { "provider": routed.provider.to_string(), "attempt": routed.attempt },
    })
}

/// The one-chunk SSE rendering of the same completion: a content delta, a stop chunk
/// carrying `usage`, then `[DONE]` — the sequence streaming clients expect.
fn chat_completion_sse(id: &str, created: u64, routed: &Routed) -> String {
    let head = json!({
        "id": id, "object": "chat.completion.chunk", "created": created, "model": routed.model_used,
        "choices": [{ "index": 0, "delta": { "role": "assistant", "content": routed.output }, "finish_reason": null }],
    });
    let tail = json!({
        "id": id, "object": "chat.completion.chunk", "created": created, "model": routed.model_used,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": routed.tokens_used },
        "mycelium": { "provider": routed.provider.to_string(), "attempt": routed.attempt },
    });
    format!("data: {head}\n\ndata: {tail}\n\ndata: [DONE]\n\n")
}

/// `POST /gateway/reason/v1/chat/completions` — the OpenAI chat shape over the
/// [`InferenceRouter`] (module doc for the mapping). Errors use OpenAI's envelope with the
/// statuses its clients map: `400 invalid_request_error` (no user message), `404
/// model_not_found` (no live provider), `502 server_error/exhausted` (every candidate
/// failed).
async fn gw_openai_chat(State(s): State<ReasonState>, Json(body): Json<ChatRequest>) -> Response {
    let Some((input, context)) = messages_to_skill_call(&body.messages) else {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "messages must contain at least one user message".into(),
            "invalid_request_error",
            "missing_user_message",
            Some("messages"),
        );
    };
    let query = ModelQuery::new(body.model.clone());
    let recorder = body.run_id.map(|id| TraceRecorder::new(Arc::clone(&s.agent), id));
    match s.router.call(&query, &input, &context, recorder.as_ref()).await {
        Ok(routed) => {
            let id = completion_id();
            let created = unix_now();
            if body.stream {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/event-stream"), (header::CACHE_CONTROL, "no-cache")],
                    chat_completion_sse(&id, created, &routed),
                )
                    .into_response()
            } else {
                Json(chat_completion_json(&id, created, &routed)).into_response()
            }
        }
        Err(RouteError::NoProvider) => openai_error(
            StatusCode::NOT_FOUND,
            format!("no live provider serves model '{}'", body.model),
            "invalid_request_error",
            "model_not_found",
            Some("model"),
        ),
        Err(e @ RouteError::Exhausted(_)) => {
            openai_error(StatusCode::BAD_GATEWAY, e.to_string(), "server_error", "exhausted", None)
        }
    }
}

/// `GET /gateway/reason/v1/models` — every model with at least one *fresh* `llm/{model}`
/// ad in this node's KV view, in OpenAI's list shape, with the live provider count as a
/// Mycelium extension. This node's view: eventually consistent by construction.
async fn gw_openai_models(State(s): State<ReasonState>) -> Response {
    let caps = s.agent.capabilities();
    // Cap keys are `cap/{node}/{ns}/{name}` (the name may itself contain `/`).
    let mut names: Vec<String> = s
        .agent
        .kv()
        .scan_prefix("cap/")
        .into_iter()
        .filter_map(|(key, _)| {
            let rest = key.strip_prefix("cap/")?;
            let (_node, rest) = rest.split_once('/')?;
            let (ns, name) = rest.split_once('/')?;
            (ns == "llm").then(|| name.to_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    let data: Vec<serde_json::Value> = names
        .into_iter()
        .filter_map(|name| {
            let providers = caps.resolve(&CapFilter::new("llm", name.as_str())).len();
            (providers > 0).then(|| {
                json!({
                    "id": name,
                    "object": "model",
                    "created": 0,
                    "owned_by": "mycelium",
                    "mycelium": { "providers": providers },
                })
            })
        })
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: serde_json::Value) -> ChatMessage {
        ChatMessage { role: role.into(), content }
    }

    #[test]
    fn messages_map_to_input_system_and_history() {
        let msgs = vec![
            msg("system", json!("be brief")),
            msg("user", json!("first")),
            msg("assistant", json!("ok")),
            msg("user", json!([{ "type": "text", "text": "sec" }, { "type": "text", "text": "ond" }])),
        ];
        let (input, ctx) = messages_to_skill_call(&msgs).unwrap();
        assert_eq!(input, "second");
        assert_eq!(ctx["system"], "be brief");
        assert_eq!(ctx["history"], "user: first\nassistant: ok");
        assert!(messages_to_skill_call(&[msg("system", json!("only"))]).is_none());
    }

    #[test]
    fn sse_rendering_ends_with_done() {
        let routed = Routed {
            output: "hi".into(),
            model_used: "echo".into(),
            tokens_used: 3,
            provider: mycelium::NodeId::new("127.0.0.1", 1).unwrap(),
            attempt: 1,
        };
        let sse = chat_completion_sse("chatcmpl-x", 1, &routed);
        assert!(sse.starts_with("data: {"));
        assert!(sse.contains("\"content\":\"hi\""));
        assert!(sse.contains("\"finish_reason\":\"stop\""));
        assert!(sse.ends_with("data: [DONE]\n\n"));
    }
}
