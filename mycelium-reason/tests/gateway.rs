//! The HTTP gateway edge: blob PUT → GET byte-identical, and the trace endpoint
//! serving a recorded run — the paths the Python LangGraph checkpointer uses over
//! the wire. Transport mirrors the wiki's gateway test (real agent + `http_port`,
//! routes mounted via `with_http_routes` before start).
#![cfg(all(feature = "gateway", feature = "llm"))]
#![allow(clippy::field_reassign_with_default)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::SlowEcho;
use mycelium::{EchoBackend, GossipAgent, GossipConfig, NodeId, PromptTemplate};
use mycelium_reason::{FsBlobStore, ModelProfile, TraceRecorder, reason_router, serve_model};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_blob_roundtrip_and_trace_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());

    // Retry fresh ports when a bind loses the bind-:0-then-drop TOCTOU race against
    // parallel test binaries (the AddrInUse CI flake class, 2026-07-07).
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
        agent.with_http_routes(reason_router(Arc::clone(&agent), Arc::clone(&store)));
        if agent.start().await.is_ok() {
            started = Some((agent, http_port));
            break;
        }
    }
    let (agent, http_port) = started.expect("could not bind agent + gateway after 16 attempts");

    let url = format!("http://127.0.0.1:{http_port}");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("{url}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // PUT a blob → its content address comes back.
    let payload: Vec<u8> = (0..100_000u32).flat_map(|i| i.to_le_bytes()).collect();
    let resp: serde_json::Value = http
        .put(format!("{url}/gateway/reason/blob"))
        .body(payload.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = resp["id"].as_str().expect("an id came back").to_string();

    // GET it back byte-identical, as an octet stream.
    let got = http.get(format!("{url}/gateway/reason/blob/{id}")).send().await.unwrap();
    assert_eq!(got.status().as_u16(), 200);
    assert_eq!(
        got.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/octet-stream")
    );
    assert_eq!(got.bytes().await.unwrap().as_ref(), payload.as_slice());

    // Bad hex → 400; unknown id → 404.
    let bad = http.get(format!("{url}/gateway/reason/blob/nothex")).send().await.unwrap();
    assert_eq!(bad.status().as_u16(), 400);
    let missing = http
        .get(format!("{url}/gateway/reason/blob/{}", "0".repeat(64)))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);

    // Record a trace → the endpoint serves events + narrative. record() writes to the
    // local KV synchronously, but poll structurally anyway (parity with real replicas).
    let tr = TraceRecorder::new(Arc::clone(&agent), "gw-run").unwrap();
    tr.tool_call("checkpoint-write", true);
    tr.resume("fable-mini", 250, &[agent.node_id().clone()]);

    let mut trace: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let t: serde_json::Value = http
            .get(format!("{url}/gateway/reason/trace/gw-run"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if t["events"].as_array().map(Vec::len) == Some(2) {
            trace = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let trace = trace.expect("both trace events served");
    assert_eq!(trace["run_id"].as_str(), Some("gw-run"));
    assert_eq!(trace["events"][0]["kind"].as_str(), Some("tool_call"));
    assert_eq!(trace["events"][1]["kind"].as_str(), Some("resume"));
    let narrative: Vec<String> = trace["narrative"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    assert_eq!(narrative.len(), 2);
    assert!(narrative[0].contains("tool checkpoint-write (ok)"));
    assert!(narrative[1].contains("resumed with fable-mini"));

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_route_endpoint_routes_and_reports_no_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());

    // Same bind-race hardening as the blob test.
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
        agent.with_http_routes(reason_router(Arc::clone(&agent), Arc::clone(&store)));
        if agent.start().await.is_ok() {
            started = Some((agent, base, http_port));
            break;
        }
    }
    let (agent, base, http_port) = started.expect("could not bind agent + gateway after 16 attempts");

    let url = format!("http://127.0.0.1:{http_port}");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("{url}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Serve `fable-mini` on this node with an EchoBackend + `{{input}}` template — so a
    // routed call echoes the input back (output is `echo: {input}`).
    let template = PromptTemplate {
        system: "deterministic echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 512,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let profile =
        ModelProfile { model: "fable-mini".into(), ctx_window: Some(8192), family: Some("echo".into()), extra: Vec::new() };
    let _model = serve_model(&agent, profile, template, Arc::new(EchoBackend)).await.unwrap();

    // The skill's `llm/fable-mini` capability must resolve locally before a route can land;
    // poll structurally rather than sleep a fixed interval.
    let mut routed: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let resp = http
            .post(format!("{url}/gateway/reason/route"))
            .json(&serde_json::json!({ "model": "fable-mini", "input": "hello-mesh" }))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 200 {
            routed = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let routed = routed.expect("a provider answered the route within the poll window");
    assert!(routed["output"].as_str().unwrap().contains("hello-mesh"), "echo output carries the input");
    assert!(routed["model_used"].as_str().is_some(), "model_used reported");
    assert_eq!(routed["provider"].as_str(), Some(format!("127.0.0.1:{base}").as_str()));
    assert_eq!(routed["attempt"].as_u64(), Some(1));

    // A model nobody serves → 404 no_provider.
    let missing = http
        .post(format!("{url}/gateway/reason/route"))
        .json(&serde_json::json!({ "model": "no-such-model", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
    let body: serde_json::Value = missing.json().await.unwrap();
    assert_eq!(body["error"].as_str(), Some("no_provider"));

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Start one gateway-carrying agent with the reason routes mounted (the bind-race
/// hardening the tests above use), optionally token-protected. Returns the agent, its
/// gossip port, and the HTTP port once `/health` answers.
async fn start_gateway_node(store: Arc<FsBlobStore>, auth_token: Option<&str>) -> (Arc<GossipAgent>, u16, u16) {
    start_gateway_node_with(store, auth_token, |_| {}).await
}

/// [`start_gateway_node`] with a hook over the config before the agent is built.
async fn start_gateway_node_with(
    store: Arc<FsBlobStore>,
    auth_token: Option<&str>,
    tune: impl Fn(&mut GossipConfig),
) -> (Arc<GossipAgent>, u16, u16) {
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = auth_token.map(str::to_owned);
        tune(&mut cfg);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
        agent.with_http_routes(reason_router(Arc::clone(&agent), Arc::clone(&store)));
        if agent.start().await.is_ok() {
            started = Some((agent, base, http_port));
            break;
        }
    }
    let (agent, base, http_port) = started.expect("could not bind agent + gateway after 16 attempts");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{http_port}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (agent, base, http_port)
}

/// **The façade dispatched protected `llm.invoke` as the node.** `/gateway/reason/route` and
/// `/gateway/reason/v1/chat/completions` never read the principal the gateway's auth layer resolved
/// and called `rpc_call` — the node's own action — so under `gateway_caller_profile = secure` the
/// provider saw `node:{gateway}` as the caller, not the HTTP client, and `gw.caller_profile`'s promise
/// (the provider sees the client as the caller) did not hold on these two routes while it held on
/// `/gateway/llm/call`. Now both route the resolved principal into the router's dispatch
/// (`InferenceRouter::call_as` → `ServiceHandle::rpc_call_as`), so the provider's `request_principal`
/// is `Client(<the bearer's principal>)`. Seen failing first: the provider saw `node:127.0.0.1:…`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_facade_dispatches_as_the_http_client_not_the_node() {
    use mycelium::RequestPrincipal;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node_with(store, Some("mesh-key"), |cfg| {
        cfg.gateway_caller_profile = mycelium::GatewayCallerProfile::Secure;
    })
    .await;
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason");
    let http = reqwest::Client::new();

    let template = PromptTemplate {
        system: "deterministic echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 512,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let profile =
        ModelProfile { model: "fable-mini".into(), ctx_window: Some(8192), family: Some("echo".into()), extra: Vec::new() };
    let _model = serve_model(&agent, profile, template, Arc::new(EchoBackend)).await.unwrap();

    // A second receiver on the provider's kind: it records who each request is from and answers
    // nothing (the served skill answers). A signal fans to every receiver of its kind.
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::channel::<String>(8);
    {
        let agent = Arc::clone(&agent);
        let mut rx = agent.service().rpc_rx(mycelium::signal::signal_kind::LLM_INVOKE);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let who = match agent.request_principal(&req) {
                    Ok(RequestPrincipal::Client(c)) => format!("client:{}", c.principal),
                    Ok(RequestPrincipal::Node(n)) => format!("node:{n}"),
                    Err(e) => format!("refused:{e}"),
                };
                let _ = seen_tx.send(who).await;
            }
        });
    }

    let expected = format!("client:{}", mycelium::legacy_token_principal(&agent.node_id().to_string()));
    // The skill's capability must resolve before a route lands; poll structurally.
    let mut routed = None;
    for _ in 0..100 {
        let resp = http
            .post(format!("{url}/route"))
            .bearer_auth("mesh-key")
            .json(&serde_json::json!({ "model": "fable-mini", "input": "who-am-i" }))
            .send()
            .await
            .unwrap();
        if resp.status().as_u16() == 200 {
            routed = Some(resp.json::<serde_json::Value>().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let routed = routed.expect("the route landed");
    assert!(routed["output"].as_str().unwrap().contains("who-am-i"));
    let seen = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv()).await.unwrap().unwrap();
    assert_eq!(seen, expected, "/route: the provider sees the HTTP client as the caller");

    let chat = http
        .post(format!("{url}/v1/chat/completions"))
        .bearer_auth("mesh-key")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "again" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(chat.status().as_u16(), 200);
    let seen = tokio::time::timeout(Duration::from_secs(5), seen_rx.recv()).await.unwrap().unwrap();
    assert_eq!(seen, expected, "/v1/chat/completions: the provider sees the HTTP client as the caller");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **The façade ran no action preflight.** An evaluator that denies `llm.invoke` was never consulted
/// on `/gateway/reason/route` or `/v1/chat/completions` — both dispatched straight to the provider —
/// while `/mcp` and `/a2a` refused the same call. Now both run the gateway's preflight per attempt
/// (`GossipAgent::gateway_preflight`, enforcement points `gateway:reason/route` and
/// `gateway:reason/v1/chat/completions`): a denial is `403` with the gateway's `reason` and `data`,
/// and the provider is never reached. Gated on `knowledge`, the feature that brings `mycelium/tls`
/// in and with it the evaluator (CI runs this crate's tests with it). Seen failing first: both
/// doors answered 200 and the provider ran.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_facade_runs_the_action_preflight() {
    use mycelium::{ReferenceEvaluator, Rule};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let cert_dir = dir.path().join("certs");
    let (agent, _base, http_port) = start_gateway_node_with(store, Some("mesh-key"), |cfg| {
        cfg.tls = Some(mycelium::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    })
    .await;
    let me = agent.node_id().clone();
    let resource = format!("prompt:llm/fable-mini@{me}");
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-facade")
            .with_catalogue("cat-facade", "1")
            .map_action("llm.invoke", resource.clone())
            .prohibit(Rule::new("*", "llm.invoke", resource)),
    ));
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason");
    let http = reqwest::Client::new();

    let template = PromptTemplate {
        system: "deterministic echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 512,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let _model = serve_model(&agent, ModelProfile::new("fable-mini"), template, Arc::new(EchoBackend)).await.unwrap();
    let reached = Arc::new(AtomicUsize::new(0));
    {
        let (agent, reached) = (Arc::clone(&agent), Arc::clone(&reached));
        let mut rx = agent.service().rpc_rx(mycelium::signal::signal_kind::LLM_INVOKE);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                reached.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    let filter = mycelium::CapFilter::new("llm", "fable-mini");
    for _ in 0..100 {
        if !agent.capabilities().resolve(&filter).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let r = http
        .post(format!("{url}/route"))
        .bearer_auth("mesh-key")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "/route: the evaluator's denial is answered");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "policy", "{v}");
    assert_eq!(v["data"]["reason"], "action_denied", "{v}");
    assert_eq!(v["data"]["policy_revision"], "rev-facade", "{v}");

    let r = http
        .post(format!("{url}/v1/chat/completions"))
        .bearer_auth("mesh-key")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "/v1/chat/completions: the evaluator's denial is answered");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"]["type"], "permission_error", "{v}");
    assert_eq!(v["error"]["code"], "policy", "{v}");
    assert_eq!(v["mycelium"]["data"]["reason"], "action_denied", "{v}");

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(reached.load(Ordering::SeqCst), 0, "the provider was never reached");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The OpenAI-compatible façade: an OpenAI chat request against `/gateway/reason/v1`
/// routes to the served `llm/{model}` and comes back in OpenAI's shape (JSON, and as a
/// one-chunk SSE stream when `stream: true`); errors use OpenAI's envelope with the
/// statuses clients map (`404 model_not_found`, `400` without a user message); `/models`
/// lists what the mesh serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn openai_facade_speaks_the_chat_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, base, http_port) = start_gateway_node(store, None).await;
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason/v1");
    let http = reqwest::Client::new();

    let template = PromptTemplate {
        system: "deterministic echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 512,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let _model = serve_model(&agent, ModelProfile::new("fable-mini"), template, Arc::new(EchoBackend)).await.unwrap();

    let chat = serde_json::json!({
        "model": "fable-mini",
        "messages": [
            { "role": "system", "content": "be brief" },
            { "role": "user", "content": "hello-facade" },
        ],
    });
    // Poll structurally until the served cap resolves and the route lands.
    let mut got: Option<serde_json::Value> = None;
    for _ in 0..100 {
        let resp = http.post(format!("{url}/chat/completions")).json(&chat).send().await.unwrap();
        if resp.status().as_u16() == 200 {
            got = Some(resp.json().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let got = got.expect("a provider answered the façade within the poll window");
    assert_eq!(got["object"], "chat.completion");
    assert!(got["id"].as_str().unwrap().starts_with("chatcmpl-"));
    assert_eq!(got["choices"][0]["message"]["role"], "assistant");
    assert!(got["choices"][0]["message"]["content"].as_str().unwrap().contains("hello-facade"));
    assert_eq!(got["choices"][0]["finish_reason"], "stop");
    assert!(got["usage"]["total_tokens"].is_number());
    assert_eq!(got["mycelium"]["provider"], format!("127.0.0.1:{base}"));
    assert_eq!(got["mycelium"]["attempt"], 1);

    // stream: true → one-chunk SSE, terminated by [DONE].
    let mut streamed = chat.clone();
    streamed["stream"] = serde_json::json!(true);
    let resp = http.post(format!("{url}/chat/completions")).json(&streamed).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert!(resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap().starts_with("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert!(body.starts_with("data: {"), "SSE data frames: {body}");
    assert!(body.contains("chat.completion.chunk"));
    assert!(body.contains("hello-facade"));
    assert!(body.trim_end().ends_with("data: [DONE]"), "terminated by [DONE]: {body}");

    // Unknown model → 404 in OpenAI's envelope.
    let resp = http
        .post(format!("{url}/chat/completions"))
        .json(&serde_json::json!({ "model": "no-such-model", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let err: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(err["error"]["code"], "model_not_found");
    assert_eq!(err["error"]["param"], "model");

    // No user message → 400.
    let resp = http
        .post(format!("{url}/chat/completions"))
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "system", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    // /models lists the served model with its provider count.
    let models: serde_json::Value = http.get(format!("{url}/models")).send().await.unwrap().json().await.unwrap();
    assert_eq!(models["object"], "list");
    let ids: Vec<&str> = models["data"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(ids, vec!["fable-mini"]);
    assert_eq!(models["data"][0]["mycelium"]["providers"], 1);

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The reason routes — merged via `with_http_routes` — sit behind the gateway's auth
/// boundary: with `gateway_auth_token` set, `/gateway/reason/*` demands the bearer and the
/// façade accepts it as the OpenAI API key. Fails on the pre-2026-09-04 core, where merged
/// routers never saw the auth layer (every companion's gateway surface was open).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reason_routes_require_the_gateway_bearer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node(store, Some("mesh-key")).await;
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason");
    let http = reqwest::Client::new();

    let route_body = serde_json::json!({ "model": "any", "input": "x" });
    let r = http.post(format!("{url}/route")).json(&route_body).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401, "no bearer → 401 on /route");
    let r = http.get(format!("{url}/v1/models")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401, "no bearer → 401 on the façade");
    let r = http.get(format!("{url}/v1/models")).bearer_auth("wrong").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401, "wrong bearer → 401");
    let r = http.get(format!("{url}/v1/models")).bearer_auth("mesh-key").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "the gateway token is the façade's API key");
    let r = http.post(format!("{url}/route")).bearer_auth("mesh-key").json(&route_body).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404, "authenticated: the usual no_provider answer");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The shared router behind the façade: four concurrent HTTP requests to one gateway node
/// land on **both** providers (the gateway node and a peer), which only holds if the
/// reservations live in one `InferenceRouter` shared across requests — a router built per
/// request would see an empty map every time and herd onto the lower id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn facade_requests_share_one_router_and_spread_across_providers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent_a, base_a, http_port) = start_gateway_node(store, None).await;

    // Peer B joins A's mesh (no gateway needed on B).
    let mut peer = None;
    for _ in 0..16 {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", base_a).unwrap()];
        let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if b.start().await.is_ok() {
            peer = Some(b);
            break;
        }
    }
    let agent_b = peer.expect("could not start peer B");

    let template = || PromptTemplate {
        system: "slow echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 64,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let slow = || Arc::new(SlowEcho(Duration::from_millis(400)));
    let _ra = serve_model(&agent_a, ModelProfile::new("fable-mini"), template(), slow()).await.unwrap();
    let _rb = serve_model(&agent_b, ModelProfile::new("fable-mini"), template(), slow()).await.unwrap();

    // Wait until A's router sees both providers (both cap ads gossiped + B live in SWIM) — and
    // B's caller-context marker: the façade dispatches for the HTTP client (0.8.0), and under the
    // secure profile a provider whose marker has not arrived is refused rather than called as the
    // node, which would fail every attempt over to A.
    let filter = mycelium::CapFilter::new("llm", "fable-mini");
    let marker_b = format!("{}{}", mycelium::signal::kv_ns::CALLER_CONTEXT, agent_b.node_id());
    for _ in 0..200 {
        if agent_a.capabilities().resolve(&filter).len() == 2
            && agent_a.peers().len() == 1
            && agent_a.kv().get(&marker_b).is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(agent_a.capabilities().resolve(&filter).len(), 2, "both providers visible");
    assert!(agent_a.kv().get(&marker_b).is_some(), "B's caller-context marker reached A");

    let url = format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions");
    let http = reqwest::Client::new();
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..4 {
        let http = http.clone();
        let url = url.clone();
        tasks.spawn(async move {
            let body: serde_json::Value = http
                .post(&url)
                .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": format!("c{i}") }] }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            body["mycelium"]["provider"].as_str().unwrap().to_owned()
        });
    }
    let mut providers = std::collections::HashSet::new();
    while let Some(r) = tasks.join_next().await {
        providers.insert(r.unwrap());
    }
    assert_eq!(providers.len(), 2, "façade requests spread across both providers: {providers:?}");

    agent_a.shutdown_with_timeout(Duration::from_secs(5)).await;
    agent_b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Falsification probe (Run 60): hostile inputs to the façade — wrong types, empty
/// messages, non-JSON, an oversized body, absurd model ids — must answer a 4xx and leave
/// the node serviceable. A 5xx or a dead `/health` would be a finding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_facade_hostile_inputs_never_5xx_and_node_stays_serviceable() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node(store, None).await;
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions");
    let http = reqwest::Client::new();

    let bodies: Vec<(&str, serde_json::Value)> = vec![
        ("content is a number", serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": 7 }] })),
        ("content is an object", serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": { "a": 1 } }] })),
        ("parts with no text", serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": [{ "type": "image_url", "image_url": "x" }] }] })),
        ("empty messages", serde_json::json!({ "model": "m", "messages": [] })),
        ("messages not an array", serde_json::json!({ "model": "m", "messages": "hi" })),
        ("stream is a string", serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": "x" }], "stream": "yes" })),
        ("model is a number", serde_json::json!({ "model": 3, "messages": [{ "role": "user", "content": "x" }] })),
        ("absurd model id", serde_json::json!({ "model": "../../etc/passwd\u{0}", "messages": [{ "role": "user", "content": "x" }] })),
        ("no role", serde_json::json!({ "model": "m", "messages": [{ "content": "x" }] })),
    ];
    for (what, body) in bodies {
        let r = http.post(&url).json(&body).send().await.unwrap();
        let st = r.status().as_u16();
        assert!((400..500).contains(&st), "{what}: expected 4xx, got {st}: {}", r.text().await.unwrap_or_default());
    }
    // Non-JSON, wrong content type, and an oversized body.
    let r = http.post(&url).header("content-type", "application/json").body("{not json").send().await.unwrap();
    assert!((400..500).contains(&r.status().as_u16()), "non-JSON → 4xx");
    let r = http.post(&url).body("model=m").send().await.unwrap();
    assert!((400..500).contains(&r.status().as_u16()), "wrong content type → 4xx");
    let huge = serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": "x".repeat(3 * 1024 * 1024) }] });
    let r = http.post(&url).json(&huge).send().await.unwrap();
    assert!((400..500).contains(&r.status().as_u16()), "3 MiB body → 4xx (413), got {}", r.status());

    // Still serviceable.
    let h = http.get(format!("http://127.0.0.1:{http_port}/health")).send().await.unwrap();
    assert_eq!(h.status().as_u16(), 200);
    let models = http.get(format!("http://127.0.0.1:{http_port}/gateway/reason/v1/models")).send().await.unwrap();
    assert_eq!(models.status().as_u16(), 200);

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// S5's unbuilt half (`docs/plans/realignment-repairs.md`: "absence, temporary unavailability,
/// authorization refusal and corrupt content stay distinguishable"): a provider that serves bytes
/// failing content verification is **corrupt**, not absent. The blob tier folded every miss into
/// one 404, so the checkpointer read a forged or damaged blob as "not arrived yet" and retried
/// forever. Node B advertises the cache and answers every fetch with the wrong bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_only_corrupt_providers_hold_is_reported_corrupt_not_missing() {
    let dir_a = tempfile::tempdir().unwrap();
    let store_a = Arc::new(FsBlobStore::open(dir_a.path()).unwrap());
    let (a, a_port, http_port) = start_gateway_node(store_a, None).await;

    let b_port = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = b_port;
    cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", a_port).unwrap()];
    let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", b_port).unwrap(), cfg));
    b.start().await.unwrap();
    let _cap = b.capabilities().advertise_capability(
        mycelium::Capability::new("reason", "blob-cache"),
        Duration::from_secs(30),
    );
    let mut rx = b.service().rpc_rx(mycelium_reason::BLOB_FETCH_KIND);
    let server = Arc::clone(&b);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            server.service().rpc_respond(&req, b"these are not the bytes you asked for".to_vec());
        }
    });
    // Structural wait: A sees B as a blob-cache provider.
    let filter = mycelium::CapFilter::new("reason", "blob-cache");
    for _ in 0..200 {
        if a.capabilities().resolve(&filter).iter().any(|(n, _)| n == b.node_id()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(a.capabilities().resolve(&filter).iter().any(|(n, _)| n == b.node_id()), "B must be visible to A");

    let id = mycelium_reason::BlobId::of(b"the real payload");
    let got = reqwest::get(format!("http://127.0.0.1:{http_port}/gateway/reason/blob/{id}")).await.unwrap();
    let status = got.status().as_u16();
    let body: serde_json::Value = got.json().await.unwrap_or_default();
    assert_eq!(status, 502, "corrupt content is not absence: {body}");
    assert_eq!(body["error"], "corrupt");

    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}


/// The review of #542, finding 1: one provider serving the wrong bytes must not make a blob that an
/// honest holder may still receive read as corrupt — a non-retriable answer would stop every reader for a
/// blob that was merely still spreading. B serves garbage; C runs the stock blob server and simply does not
/// hold the blob yet. The answer is `not_found`, retriable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_bad_provider_beside_an_honest_one_that_lacks_the_blob_is_not_found() {
    let dir_a = tempfile::tempdir().unwrap();
    let (a, a_port, http_port) = start_gateway_node(Arc::new(FsBlobStore::open(dir_a.path()).unwrap()), None).await;
    let peer = |bootstrap: u16| {
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", bootstrap).unwrap()];
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    };
    let b = peer(a_port);
    b.start().await.unwrap();
    let _b_cap = b.capabilities().advertise_capability(mycelium::Capability::new("reason", "blob-cache"), Duration::from_secs(30));
    let mut rx = b.service().rpc_rx(mycelium_reason::BLOB_FETCH_KIND);
    let server = Arc::clone(&b);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            server.service().rpc_respond(&req, b"garbage".to_vec());
        }
    });
    let c = peer(a_port);
    c.start().await.unwrap();
    let dir_c = tempfile::tempdir().unwrap();
    let _c_server = mycelium_reason::spawn_blob_server(&c, Arc::new(FsBlobStore::open(dir_c.path()).unwrap()));
    let filter = mycelium::CapFilter::new("reason", "blob-cache");
    for _ in 0..200 {
        if a.capabilities().resolve(&filter).len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Without both providers the case is not exercised: no provider at all is also `not_found`.
    assert!(a.capabilities().resolve(&filter).len() >= 2, "both providers must be visible to A");
    let id = mycelium_reason::BlobId::of(b"still spreading");
    let got = reqwest::get(format!("http://127.0.0.1:{http_port}/gateway/reason/blob/{id}")).await.unwrap();
    let status = got.status().as_u16();
    let body: serde_json::Value = got.json().await.unwrap_or_default();
    assert_eq!((status, body["error"].as_str()), (404, Some("not_found")), "{body}");
    c.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The review of #542, finding 2: damage at rest is corruption, not absence. The only copy of a blob, on
/// this node's disk, no longer matches its address — the stock tier read that as a miss, which is retriable
/// forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blob_damaged_on_the_only_holders_disk_is_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let id = store.put(b"the original bytes").unwrap();
    std::fs::write(dir.path().join(id.to_hex()), b"bit rot").unwrap();
    let (a, _port, http_port) = start_gateway_node(store, None).await;
    let got = reqwest::get(format!("http://127.0.0.1:{http_port}/gateway/reason/blob/{id}")).await.unwrap();
    let status = got.status().as_u16();
    let body: serde_json::Value = got.json().await.unwrap_or_default();
    assert_eq!((status, body["error"].as_str()), (502, Some("corrupt")), "{body}");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}


/// The second review of #542, finding 1: a copy damaged on a *remote* holder's disk — here truncated to
/// nothing, a realistic crash outcome — is corruption, through the **stock** blob server. It served the
/// damaged bytes, and empty bytes are the protocol's "miss", so the requester read it as not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_copy_truncated_on_the_only_remote_holder_is_corrupt_through_the_stock_server() {
    let dir_a = tempfile::tempdir().unwrap();
    let (a, a_port, http_port) = start_gateway_node(Arc::new(FsBlobStore::open(dir_a.path()).unwrap()), None).await;
    let c_port = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = c_port;
    cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", a_port).unwrap()];
    let c = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", c_port).unwrap(), cfg));
    c.start().await.unwrap();
    let dir_c = tempfile::tempdir().unwrap();
    let store_c = Arc::new(FsBlobStore::open(dir_c.path()).unwrap());
    let id = store_c.put(b"the only good copy, about to be truncated").unwrap();
    std::fs::write(dir_c.path().join(id.to_hex()), b"").unwrap();
    let _server = mycelium_reason::spawn_blob_server(&c, store_c);
    let filter = mycelium::CapFilter::new("reason", "blob-cache");
    for _ in 0..200 {
        if a.capabilities().resolve(&filter).iter().any(|(n, _)| n == c.node_id()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(a.capabilities().resolve(&filter).iter().any(|(n, _)| n == c.node_id()), "C must be visible to A");
    let got = reqwest::get(format!("http://127.0.0.1:{http_port}/gateway/reason/blob/{id}")).await.unwrap();
    let status = got.status().as_u16();
    let body: serde_json::Value = got.json().await.unwrap_or_default();
    assert_eq!((status, body["error"].as_str()), (502, Some("corrupt")), "{body}");
    c.shutdown_with_timeout(Duration::from_secs(5)).await;
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The third review of #542: a copy damaged on this node's disk is repaired by the next successful mesh
/// fetch — the read answers whole bytes from an honest provider, and the local copy is valid afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_local_copy_is_repaired_by_the_next_mesh_fetch() {
    let dir_a = tempfile::tempdir().unwrap();
    let store_a = Arc::new(FsBlobStore::open(dir_a.path()).unwrap());
    let (a, a_port, http_port) = start_gateway_node(Arc::clone(&store_a), None).await;
    let b_port = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = b_port;
    cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", a_port).unwrap()];
    let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", b_port).unwrap(), cfg));
    b.start().await.unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let store_b = Arc::new(FsBlobStore::open(dir_b.path()).unwrap());
    let bytes: &[u8] = b"the honest copy B still holds";
    let id = store_b.put(bytes).unwrap();
    let _server = mycelium_reason::spawn_blob_server(&b, store_b);
    store_a.put(bytes).unwrap();
    std::fs::write(dir_a.path().join(id.to_hex()), b"rot").unwrap();
    assert!(matches!(store_a.read(&id), mycelium_reason::LocalRead::Damaged(_)));
    let filter = mycelium::CapFilter::new("reason", "blob-cache");
    for _ in 0..200 {
        if a.capabilities().resolve(&filter).iter().any(|(n, _)| n == b.node_id()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(a.capabilities().resolve(&filter).iter().any(|(n, _)| n == b.node_id()), "B must be visible to A");
    let got = reqwest::get(format!("http://127.0.0.1:{http_port}/gateway/reason/blob/{id}")).await.unwrap();
    assert_eq!(got.status().as_u16(), 200);
    assert_eq!(got.bytes().await.unwrap().as_ref(), bytes);
    assert!(matches!(store_a.read(&id), mycelium_reason::LocalRead::Valid(_)), "A's copy was repaired");
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
}


/// Post-360 row H: every door that takes a caller-chosen `run_id` validates it as one bounded segment
/// before it is used as a log-stream segment (`reason/{run_id}/{node}`): the trace GET, `/route`'s body
/// and the façade's body. A `/` reached another run's prefix; an empty or 10 KiB id was written as given.
/// Seen failing first: `GET /gateway/reason/trace/victim%2Fx` answered 200 with the victim's events.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_run_id_door_refuses_an_id_that_is_not_one_bounded_segment() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, _, http_port) = start_gateway_node(Arc::new(FsBlobStore::open(dir.path()).unwrap()), None).await;
    let url = format!("http://127.0.0.1:{http_port}");
    let http = reqwest::Client::new();
    let bad_ids: Vec<String> = vec!["victim/x".into(), "..".into(), "".into(), "x".repeat(10 * 1024), "a b".into()];

    for id in &bad_ids {
        let encoded: String = id.bytes().map(|b| format!("%{b:02X}")).collect();
        let got = http.get(format!("{url}/gateway/reason/trace/{encoded}")).send().await.unwrap();
        let status = got.status().as_u16();
        let body: serde_json::Value = got.json().await.unwrap_or_default();
        // An empty segment is axum's 404 (no such route), and a conforming client resolves `%2E%2E` as a
        // double-dot segment before sending (so the router sees `/gateway/reason/`, 404 — the raw request
        // below is what reaches the door); every other bad id is the route's own 400.
        if id.is_empty() || id == ".." {
            assert_eq!(status, 404, "{id:?} never reaches the door from a conforming client");
        } else {
            assert_eq!(status, 400, "trace GET for {id:?} must be refused: {body}");
            assert_eq!(body["error"], "bad_run_id", "named refusal for {id:?}");
        }

        let got = http
            .post(format!("{url}/gateway/reason/route"))
            .json(&serde_json::json!({ "model": "m", "input": "hi", "run_id": id }))
            .send()
            .await
            .unwrap();
        let status = got.status().as_u16();
        let body: serde_json::Value = got.json().await.unwrap_or_default();
        assert_eq!(status, 400, "/route with run_id {id:?} must be refused before routing: {body}");
        assert_eq!(body["error"], "bad_run_id");

        let got = http
            .post(format!("{url}/gateway/reason/v1/chat/completions"))
            .json(&serde_json::json!({ "model": "m", "messages": [{ "role": "user", "content": "hi" }], "run_id": id }))
            .send()
            .await
            .unwrap();
        let status = got.status().as_u16();
        let body: serde_json::Value = got.json().await.unwrap_or_default();
        assert_eq!(status, 400, "the façade with run_id {id:?} must be refused before routing: {body}");
        assert_eq!(body["error"]["code"], "bad_run_id");
        assert_eq!(body["error"]["param"], "run_id");
    }

    // `..` as the server sees it — a raw request no URL parser has normalised — is refused at the door.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut raw = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
        raw.write_all(b"GET /gateway/reason/trace/.. HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").await.unwrap();
        // Read until the status line and the body's refusal are in (the server need not close first).
        let mut reply = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let n = tokio::time::timeout_at(deadline, raw.read(&mut buf)).await.expect("a reply within 10 s").unwrap();
            reply.extend_from_slice(&buf[..n]);
            let so_far = String::from_utf8_lossy(&reply);
            if n == 0 || so_far.contains("bad_run_id") || (so_far.contains("\r\n\r\n") && !so_far.starts_with("HTTP/1.1 400")) {
                break;
            }
        }
        let reply = String::from_utf8_lossy(&reply);
        assert!(reply.starts_with("HTTP/1.1 400"), "a raw `..` segment is refused at the door: {reply}");
        assert!(reply.contains("bad_run_id"), "{reply}");
    }

    // A plain id still routes (404 no_provider: nothing serves `m`) and still reads (an empty trace).
    let got = http
        .post(format!("{url}/gateway/reason/route"))
        .json(&serde_json::json!({ "model": "m", "input": "hi", "run_id": "plain-run_1:ok" }))
        .send()
        .await
        .unwrap();
    assert_eq!(got.status().as_u16(), 404, "a valid id reaches the router");
    let got = http.get(format!("{url}/gateway/reason/trace/plain-run_1:ok")).send().await.unwrap();
    assert_eq!(got.status().as_u16(), 200);

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Post-360 row H: the blob door is bounded by the store's quota. A put that would pass it answers
/// **507 `quota_exceeded`** and writes nothing; a blob already held still answers 200 (it costs
/// nothing); the per-blob ceiling's 413 is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_blob_door_answers_507_when_the_store_is_at_its_quota() {
    let dir = tempfile::tempdir().unwrap();
    // Sizes above one block cost their length; a smaller blob costs one block (4096).
    let store = Arc::new(FsBlobStore::open_with_quota(dir.path(), 10_096).unwrap());
    let (agent, _, http_port) = start_gateway_node(Arc::clone(&store), None).await;
    let url = format!("http://127.0.0.1:{http_port}/gateway/reason/blob");
    let http = reqwest::Client::new();

    let first = vec![1u8; 6000];
    let put = http.put(&url).body(first.clone()).send().await.unwrap();
    assert_eq!(put.status().as_u16(), 200);
    assert_eq!(store.used_bytes(), 6000);

    let put = http.put(&url).body(vec![2u8; 6000]).send().await.unwrap();
    let status = put.status().as_u16();
    let body: serde_json::Value = put.json().await.unwrap_or_default();
    assert_eq!(status, 507, "past the quota: {body}");
    assert_eq!(body["error"], "quota_exceeded");
    assert_eq!(store.used_bytes(), 6000, "nothing was written");

    let put = http.put(&url).body(first).send().await.unwrap();
    assert_eq!(put.status().as_u16(), 200, "a blob already held costs nothing");
    let put = http.put(&url).body(vec![3u8; 100]).send().await.unwrap();
    assert_eq!(put.status().as_u16(), 200, "a small blob costs one block — exactly the quota");
    assert_eq!(store.used_bytes(), 10_096);

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── Post-360 row R: inference routing as policy ────────────────────────────────────────────────

/// A gateway node mounting the **classifying** reason routes (`reason_router_classified`) with an
/// evaluator and an evidence journal attached, plus a meshed peer. Returns `(gateway, peer, http port)`.
#[cfg(feature = "knowledge")]
async fn start_classified_pair(
    dir: &std::path::Path,
    evaluator: mycelium::ReferenceEvaluator,
    classes: mycelium_reason::DataClassPolicy,
) -> (Arc<GossipAgent>, Arc<GossipAgent>, u16) {
    start_classified_pair_with(dir, evaluator, classes, |_| {}).await
}

/// [`start_classified_pair`] with a hook over the gateway node's config. The journal and the evaluator are
/// attached **before** `start()` (round-2 review A: after it, the gateway answers in between).
#[cfg(feature = "knowledge")]
async fn start_classified_pair_with(
    dir: &std::path::Path,
    evaluator: mycelium::ReferenceEvaluator,
    classes: mycelium_reason::DataClassPolicy,
    tune: impl Fn(&mut GossipConfig),
) -> (Arc<GossipAgent>, Arc<GossipAgent>, u16) {
    let store = Arc::new(FsBlobStore::open(dir.join("blobs")).unwrap());
    let journal =
        mycelium::EvidenceJournal::open(dir.join("evidence.journal"), mycelium::EvidenceProfile::Strict).unwrap();
    let evaluator: Arc<dyn mycelium::ActionEvaluator> = Arc::new(evaluator);
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        tune(&mut cfg);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
        agent.with_http_routes(mycelium_reason::reason_router_classified(
            Arc::clone(&agent),
            Arc::clone(&store),
            mycelium_reason::RouterConfig::default(),
            classes.clone(),
        ));
        agent.with_evidence_journal(Arc::clone(&journal));
        agent.with_action_evaluator(Arc::clone(&evaluator));
        if agent.start().await.is_ok() {
            started = Some((agent, base, http_port));
            break;
        }
    }
    let (agent, base, http_port) = started.expect("could not bind agent + gateway");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{http_port}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut peer = None;
    for _ in 0..16 {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", base).unwrap()];
        let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if b.start().await.is_ok() {
            peer = Some(b);
            break;
        }
    }
    (agent, peer.expect("could not start the peer"), http_port)
}

/// *Confidential may only go to in-domain models; anything else may go anywhere.*
#[cfg(feature = "knowledge")]
fn confidential_stays_in_domain() -> mycelium::ReferenceEvaluator {
    use mycelium::{Locality, ReferenceEvaluator, Rule};
    ReferenceEvaluator::new("rev-route")
        .with_catalogue("cat-route", "1")
        .map_action("llm.invoke", "*")
        .allow(Rule::new("*", "llm.invoke", "*"))
        .prohibit(Rule::new("*", "llm.invoke", "*").for_class("confidential").at_locality(Locality::External))
}

#[cfg(feature = "knowledge")]
fn echo_template() -> PromptTemplate {
    PromptTemplate {
        system: "deterministic echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 64,
        temperature: 0.0,
        metadata: HashMap::new(),
    }
}

/// Wait until `agent` sees `n` providers of `model`, each with its `llm-meta` ad, and every peer's
/// caller-context marker (the façade dispatches for the HTTP client).
#[cfg(feature = "knowledge")]
async fn await_providers(agent: &Arc<GossipAgent>, model: &str, n: usize, peers: &[&Arc<GossipAgent>]) {
    let filter = mycelium::CapFilter::new("llm", model);
    let meta = mycelium::CapFilter::new("llm-meta", model);
    for _ in 0..200 {
        let markers = peers.iter().all(|p| {
            agent.kv().get(&format!("{}{}", mycelium::signal::kv_ns::CALLER_CONTEXT, p.node_id())).is_some()
        });
        if agent.capabilities().resolve(&filter).len() == n
            && agent.capabilities().resolve(&meta).len() == n
            && agent.peers().len() == peers.len()
            && markers
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{model}: {n} provider(s) never became visible");
}

/// **Row R, promises 1–4 together.** Two providers of one model: the one the router ranks first
/// declares its inference **external**, the other **in-domain**. Under *confidential stays in the
/// domain*:
/// - a request declared `confidential` is **routed to the in-domain model**, not refused — the
///   external one is passed over, and the response names it as refused;
/// - a `public` request goes to the first-ranked (external) model as before;
/// - an **unclassified** request is the most restrictive class, so it too stays in the domain;
/// - a class the policy does not know is `400`, and nothing is routed;
/// - the evidence journal holds a **denied** decision for the external destination naming the class
///   and its locality, and the permitted decision names the class, the destination it went to and
///   the destination refused before it.
///
/// Seen failing first, twice: with the router telling the preflight a destination and class other
/// than the ones it established (the evaluator deciding blind, as before row R), the confidential
/// request answered `200` from the **external** provider; and with the router stopping at the first
/// refused candidate (the pre-row-R loop), it answered `403` although an in-domain model existed.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_confidential_request_is_routed_to_an_in_domain_model_and_the_external_one_is_passed_over() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "internal", "confidential"]).unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;

    // The router ranks equal fills by node id: put the external model where it is tried first.
    let (first, second) = if a.node_id().to_string() < b.node_id().to_string() { (&a, &b) } else { (&b, &a) };
    let _ext = serve_model(first, ModelProfile::new("fable-mini").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    let _dom = serve_model(second, ModelProfile::new("fable-mini").in_domain(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 2, &[&b]).await;
    let (external, in_domain) = (first.node_id().to_string(), second.node_id().to_string());

    let http = reqwest::Client::new();
    let route = |class: Option<&'static str>| {
        let mut r = http
            .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
            .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }));
        if let Some(c) = class {
            r = r.header(mycelium_reason::DATA_CLASS_HEADER, c);
        }
        r.send()
    };

    let r = route(Some("confidential")).await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "a permitted destination exists, so it is not refused");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["provider"], in_domain.as_str(), "routed in-domain: {v}");
    assert_eq!(v["data_class"], "confidential", "{v}");
    assert_eq!(v["attempt"], 1, "a refused destination is not an attempt: {v}");
    assert_eq!(v["refused"][0]["provider"], external.as_str(), "{v}");
    assert_eq!(v["refused"][0]["reason"], "action_denied", "{v}");

    // No floor is bound for this (anonymous) caller, so the default is its floor: declaring `public`
    // does not lower it (the round-1 review's finding 3) — it stays in the domain.
    let r = route(Some("public")).await.unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data_class"], "confidential", "the default is a floor: {v}");
    assert_eq!(v["provider"], in_domain.as_str(), "{v}");

    let r = route(None).await.unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data_class"], "confidential", "unclassified is the most restrictive class: {v}");
    assert_eq!(v["provider"], in_domain.as_str(), "{v}");

    // The OpenAI façade, the same.
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["mycelium"]["provider"], in_domain.as_str(), "{v}");
    assert_eq!(v["mycelium"]["data_class"], "confidential", "{v}");

    let r = route(Some("secret")).await.unwrap();
    assert_eq!(r.status().as_u16(), 400, "an unknown class is refused, never guessed");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "unclassifiable", "{v}");

    // The evidence: the refusal and the permit, each naming what it decided on.
    let records: Vec<mycelium::AeEvidence> = mycelium::read_evidence_journal(&dir.path().join("evidence.journal"))
        .unwrap()
        .iter()
        .filter_map(|b| serde_json::from_slice(b).ok())
        .collect();
    let decided: Vec<&mycelium::AeEvidence> =
        records.iter().filter(|r| r.kind == mycelium::RecordKind::Decided && r.route.is_some()).collect();
    let denial = decided
        .iter()
        .find(|r| r.decision == mycelium::DecisionKind::Deny)
        .expect("the external destination's denial is recorded");
    let route = denial.route.as_ref().unwrap();
    assert_eq!(route.data_class.as_ref().unwrap().class, "confidential");
    assert_eq!(route.destination.locality, mycelium::Locality::External);
    assert_eq!(denial.destination.as_deref(), Some(format!("llm/fable-mini@{external}").as_str()));
    assert_eq!(denial.execution, mycelium::Execution::None, "nothing ran there");
    let permit = decided
        .iter()
        .find(|r| {
            r.decision == mycelium::DecisionKind::Permit
                && r.route.as_ref().is_some_and(|rt| !rt.refused.is_empty())
        })
        .expect("the in-domain permit names the destination refused before it");
    let route = permit.route.as_ref().unwrap();
    assert_eq!(route.destination.locality, mycelium::Locality::InDomain);
    assert_eq!(route.data_class.as_ref().unwrap().source, mycelium::ClassSource::Declared);
    assert_eq!(route.refused[0].destination.name, format!("llm/fable-mini@{external}"));
    assert_eq!(route.refused[0].destination.locality, mycelium::Locality::External);
    assert_eq!(route.refused[0].reason, "action_denied");
    assert_eq!(permit.operation_id, denial.operation_id, "one call, one operation id across its candidates");
    assert!(permit.operation_id.starts_with("reason:"), "{}", permit.operation_id);

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Row R: refused, naming why, when no permitted destination exists.** Only an external model,
/// and one that declares no locality at all: a confidential request is refused `403` with every
/// candidate named — the external one *denied*, the undeclared one *not established* (an undeclared
/// locality is never read as in-domain) — and the class it was refused for.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_permitted_destination_a_classified_request_is_refused_naming_every_candidate() {
    let dir = tempfile::tempdir().unwrap();
    // Unclassified is `public` here, so a request may go anywhere unless it declares itself confidential.
    let classes =
        mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap().unclassified_as("public").unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _ext = serve_model(&a, ModelProfile::new("fable-mini").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    let _undeclared = serve_model(&b, ModelProfile::new("fable-mini"), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 2, &[&b]).await;

    let http = reqwest::Client::new();
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "policy", "{v}");
    let data = &v["mycelium"]["data"];
    assert_eq!(data["data_class"]["class"], "confidential", "{v}");
    let listed = data["refused_destinations"].as_array().expect("every candidate named");
    assert_eq!(listed.len(), 2, "{v}");
    let of = |node: &Arc<GossipAgent>| {
        listed
            .iter()
            .find(|d| d["destination"] == format!("llm/fable-mini@{}", node.node_id()))
            .unwrap_or_else(|| panic!("{} named: {v}", node.node_id()))
            .clone()
    };
    assert_eq!(of(&a)["locality"], "external");
    assert_eq!(of(&a)["reason"], "action_denied");
    assert_eq!(of(&b)["locality"], "unknown");
    assert_eq!(of(&b)["reason"], "authority_not_established");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("confidential"), "{message}");

    // Public (here the default) goes anywhere — including the undeclared one: the prohibition is about one class.
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data_class"], "public", "{v}");

    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Row R, the other doors.** A policy about class cannot be walked around through a door that
/// does not classify: `/gateway/llm/call` (the substrate's own `llm.invoke` door) establishes no
/// class and no destination, so the class rule decides *not established* and the call is refused —
/// even for data that would have been public.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_door_that_does_not_classify_cannot_pass_a_class_rule() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _dom = serve_model(&a, ModelProfile::new("fable-mini").in_domain(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/gateway/llm/call"))
        .json(&serde_json::json!({ "ns": "llm", "name": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "refused, not routed around the policy");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data"]["reason"], "authority_not_established", "{v}");
    assert!(v["data"]["errors"].to_string().contains("data class not established"), "{v}");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Row R: a declaration this node cannot honour is not dropped.** The plain `reason_router` classifies
/// nothing; a request that declares a class is `400` on both doors and nothing is routed, rather than
/// routed as if it had declared nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_class_on_a_node_that_classifies_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node(store, None).await;
    let _m = serve_model(&agent, ModelProfile::new("fable-mini"), PromptTemplate {
        system: "echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 64,
        temperature: 0.0,
        metadata: HashMap::new(),
    }, Arc::new(EchoBackend))
    .await
    .unwrap();
    let http = reqwest::Client::new();
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "unclassifiable", "{v}");
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "data_class", "{v}");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Row R: a client cannot write a served model's `llm-meta` ad.** `locality` on that ad is what a routing policy
/// decides on, and a gateway client writes ads under the gateway node's own key — so `capability/advertise` and
/// `units/declare` refuse the namespace `403 reserved_namespace`, with or without a `locality` attribute, and
/// nothing is advertised. Seen failing first twice: with the attribute refusal disabled the forged ad answered `200`
/// (the internal review); and with only the attribute refused, an ad *without* `locality` answered `200` and replaced
/// the node's own, its locality read `unknown` (round-1 review, finding 2 — `the_node_keeps_its_own_model_ad`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gateway_client_cannot_declare_a_models_locality() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node(store, None).await;
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}/gateway");
    for attributes in [serde_json::json!({ "locality": "in_domain" }), serde_json::json!({ "ctx_window": 8192 })] {
        let r = http
            .post(format!("{base}/capability/advertise"))
            .json(&serde_json::json!({ "ns": "llm-meta", "name": "m", "attributes": attributes }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 403, "{attributes}");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "reserved_namespace", "{v}");
    }
    for attrs in ["{ locality = \"in_domain\" }", "{ ctx_window = 8192 }"] {
        let r = http
            .post(format!("{base}/units/declare"))
            .json(&serde_json::json!({ "toml": format!("[[capability]]\nns = \"llm-meta\"\nname = \"m\"\nattrs = {attrs}\n") }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 403, "{attrs}");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "reserved_namespace", "{v}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let router = mycelium_reason::InferenceRouter::new(Arc::clone(&agent), mycelium_reason::RouterConfig::default());
    assert!(router.localities("m").is_empty(), "nothing was advertised");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── Row R, round-1 review: the reviewer's probes, as regression tests ───────────────────────────────

/// Finding 2: a `cap:write` client's `llm-meta` ad without `locality` replaced the node's own and dropped its
/// locality (`{gw: InDomain}` → `{gw: Unknown}`, a confidential route then 403). Seen failing first: as stated.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_keeps_its_own_model_ad() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _dom = serve_model(&a, ModelProfile::new("fable-mini").in_domain(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let router = mycelium_reason::InferenceRouter::new(Arc::clone(&a), mycelium_reason::RouterConfig::default());
    assert_eq!(router.localities("fable-mini").get(a.node_id()), Some(&mycelium::Locality::InDomain));
    let http = reqwest::Client::new();
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/capability/advertise"))
        .json(&serde_json::json!({ "ns": "llm-meta", "name": "fable-mini", "attributes": { "ctx_window": 8192 } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(router.localities("fable-mini").get(a.node_id()), Some(&mycelium::Locality::InDomain), "unchanged");
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding 1: `GET /gateway/rpc/serve/llm.invoke` registered a receiver beside the node's prompt skill, and
/// `llm.invoke` fans to every receiver — so a `mesh:serve` client received the confidential prompt routed
/// in-domain, and could answer it first. The stream is now refused `403 protected_kind` and receives nothing.
/// Seen failing first: the stream opened `200` and carried `TOP-SECRET-PAYLOAD`.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_rpc_serve_stream_cannot_receive_a_routed_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _dom = serve_model(&a, ModelProfile::new("fable-mini").in_domain(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let http = reqwest::Client::new();
    for kind in ["llm.invoke", "mcp.invoke", "skill.invoke", "rpc.result"] {
        let r = http.get(format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/{kind}")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 403, "{kind}");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "protected_kind", "{v}");
    }
    let mut sse = http.get(format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/llm.invoke")).send().await.unwrap();
    let seen = tokio::spawn(async move {
        let mut buf = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(1), sse.chunk()).await {
                Ok(Ok(Some(c))) => buf.push_str(&String::from_utf8_lossy(&c)),
                Ok(_) => break,
                Err(_) => {}
            }
        }
        buf
    });
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "TOP-SECRET-PAYLOAD" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "served by the node's own prompt skill");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["output"], "echo: TOP-SECRET-PAYLOAD", "{v}");
    let body = seen.await.unwrap();
    assert!(!body.contains("payload_b64"), "nothing streamed to the client: {body}");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding 4: a repeated `x-mycelium-data-class` took the first value (`public` then `confidential` routed
/// external). Now `400`, and nothing is routed. Seen failing first: `200` from the external provider.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_class_header_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let classes =
        mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap().unclassified_as("public").unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _ext = serve_model(&a, ModelProfile::new("fable-mini").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "public")
        .header(mycelium_reason::DATA_CLASS_HEADER, "confidential")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "unclassifiable", "{v}");
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/v1/chat/completions"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "public")
        .header(mycelium_reason::DATA_CLASS_HEADER, "public")
        .json(&serde_json::json!({ "model": "fable-mini", "messages": [{ "role": "user", "content": "x" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400, "twice, even the same value");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding 3: with no floor bound, an anonymous caller declared ` public ` on confidential text and it went to
/// the external model. The default is now the floor: refused `403`, nothing dispatched. Seen failing first: `200`.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_class_is_a_floor_a_client_cannot_declare_below() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "confidential"]).unwrap();
    let (a, b, http_port) = start_classified_pair(dir.path(), confidential_stays_in_domain(), classes).await;
    let _ext = serve_model(&b, ModelProfile::new("fable-mini").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, " public ")
        .json(&serde_json::json!({ "model": "fable-mini", "input": "confidential text" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "held at the default class, which may not go external");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data"]["data_class"]["class"], "confidential", "{v}");
    assert_eq!(v["data"]["data_class"]["source"], "default", "{v}");
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── Guide 20's snippet, compiled and run (round-1 review, finding 7) ─────────────────────────────────

/// Guide 20 § *Deciding where data may go* — the snippet's body verbatim (its imports are here as `use`s). Keep the two in step.
#[cfg(feature = "knowledge")]
async fn routing_policy(
    agent: Arc<GossipAgent>,
    dir: &std::path::Path,
) -> Result<mycelium_reason::ModelReg, Box<dyn std::error::Error>> {
    use mycelium::{EvidenceJournal, EvidenceProfile, Locality, ReferenceEvaluator, Rule};
    use mycelium_reason::{DataClassPolicy, RouterConfig, reason_router_classified};
    // Gateway door, mounted before start: classes least to most restrictive. An unclassified request — and any
    // caller with no floor bound — is the most restrictive class; `web-agent`'s credential may send `public`.
    let classes = DataClassPolicy::new(["public", "internal", "confidential"])?
        .floor_for(format!("token:{}/web-agent", agent.node_id()), "public")?;
    let store = Arc::new(FsBlobStore::open(dir.join("blobs"))?);
    agent.with_http_routes(reason_router_classified(Arc::clone(&agent), store, RouterConfig::default(), classes));

    // Evidence and policy, attached before start: from the first request on, every route is decided and
    // recorded. (A classifying router refuses to route while no evaluator is attached.)
    agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.journal"), EvidenceProfile::Strict)?);
    // Policy, as an allow-list: public and internal data may go anywhere; confidential only in-domain.
    // A class not listed here (a typo, a class added later, `restricted`) matches no allowance: refused.
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-route")
            .with_catalogue("cat-route", "1")
            .map_action("llm.invoke", "*")
            .allow(Rule::new("*", "llm.invoke", "*").for_class("public"))
            .allow(Rule::new("*", "llm.invoke", "*").for_class("internal"))
            .allow(Rule::new("*", "llm.invoke", "*").for_class("confidential").at_locality(Locality::InDomain)),
    ));
    agent.start().await?;

    // Serve side: each provider declares where its inference runs (the `locality` attribute of its llm-meta ad).
    let template = PromptTemplate {
        system: "You answer briefly.".into(),
        user_template: "{{input}}".into(),
        max_tokens: 256,
        temperature: 0.0,
        metadata: Default::default(),
    };
    Ok(serve_model(&agent, ModelProfile::new("qwen3").in_domain(), template, Arc::new(EchoBackend)).await?)
}

/// The guide's snippet runs: an unclassified request is confidential and goes to the in-domain model, and the
/// decision is in the evidence journal the snippet attached.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guide_20_routing_policy() {
    let dir = tempfile::tempdir().unwrap();
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
        if let Ok(reg) = routing_policy(Arc::clone(&agent), dir.path()).await {
            started = Some((agent, reg, http_port));
            break;
        }
    }
    let (agent, _reg, http_port) = started.expect("the snippet starts a node");
    await_providers(&agent, "qwen3", 1, &[]).await;
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{http_port}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .json(&serde_json::json!({ "model": "qwen3", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data_class"], "confidential", "{v}");
    let records = mycelium::read_evidence_journal(&dir.path().join("evidence.journal")).unwrap();
    assert!(!records.is_empty(), "the decision was recorded");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── Row R, round-2 review ───────────────────────────────────────────────────────────────────────────

/// Finding A: a classifying router routed anywhere while no evaluator was attached — the reviewer's probe
/// (`review2_window_between_start_and_evaluator`) got `200` from the external provider for a `confidential`
/// request between `start()` and `with_action_evaluator`. Now it refuses `403 authority_not_established` and
/// nothing is dispatched; once an evaluator is attached, it routes. Seen failing first: `200`, external.
#[cfg(feature = "knowledge")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_classifying_router_routes_nothing_without_an_evaluator() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path().join("blobs")).unwrap());
    let (agent, http_port) = {
        let mut started = None;
        for _ in 0..16 {
            let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = base;
            cfg.http_port = Some(http_port);
            let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", base).unwrap(), cfg));
            // Unclassified is public here, so a request may go anywhere once something decides it may.
            let classes = mycelium_reason::DataClassPolicy::new(["public", "internal", "confidential"])
                .unwrap()
                .unclassified_as("public")
                .unwrap();
            agent.with_http_routes(mycelium_reason::reason_router_classified(
                Arc::clone(&agent),
                Arc::clone(&store),
                mycelium_reason::RouterConfig::default(),
                classes,
            ));
            if agent.start().await.is_ok() {
                started = Some((agent, http_port));
                break;
            }
        }
        started.expect("could not start")
    };
    let _ext = serve_model(&agent, ModelProfile::new("m").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&agent, "m", 1, &[]).await;
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{http_port}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for class in ["confidential", "public"] {
        let r = http
            .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
            .header(mycelium_reason::DATA_CLASS_HEADER, class)
            .json(&serde_json::json!({ "model": "m", "input": "SECRET" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 403, "{class}: nothing decides, so nothing is routed");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["reason"], "authority_not_established", "{v}");
    }
    // Attached (late — counted and warned), it routes: public may go to the external model.
    agent.with_evidence_journal(
        mycelium::EvidenceJournal::open(dir.path().join("evidence.journal"), mycelium::EvidenceProfile::Strict).unwrap(),
    );
    agent.with_action_evaluator(Arc::new(confidential_stays_in_domain()));
    let r = http
        .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
        .header(mycelium_reason::DATA_CLASS_HEADER, "public")
        .json(&serde_json::json!({ "model": "m", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "decided now");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding D: a floor bound **below** the default, through a real named token: `web-agent` (bound to `public`)
/// may send `public` to an external model, and a caller without the credential is refused at the gateway. The
/// token table needs `compliance` (a build without it refuses the table at start), so CI's reason job runs its
/// gateway suite with `compliance`.
#[cfg(all(feature = "knowledge", feature = "compliance"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_floor_bound_below_the_default_lets_that_credential_send_lower() {
    let dir = tempfile::tempdir().unwrap();
    let classes = mycelium_reason::DataClassPolicy::new(["public", "confidential"])
        .unwrap()
        .floor_for("token:gw/web-agent", "public")
        .unwrap();
    let (a, b, http_port) = start_classified_pair_with(dir.path(), confidential_stays_in_domain(), classes, |cfg| {
        cfg.gateway_identity_issuer = Some("gw".into());
        cfg.gateway_named_tokens = vec![mycelium::GatewayNamedToken {
            name: "web-agent".into(),
            token: "web-secret".into(),
            scopes: vec!["*".into()],
        }];
    })
    .await;
    let _ext = serve_model(&b, ModelProfile::new("fable-mini").external(), echo_template(), Arc::new(EchoBackend))
        .await
        .unwrap();
    await_providers(&a, "fable-mini", 1, &[&b]).await;
    let http = reqwest::Client::new();
    let route = |bearer: Option<&'static str>| {
        let mut r = http
            .post(format!("http://127.0.0.1:{http_port}/gateway/reason/route"))
            .header(mycelium_reason::DATA_CLASS_HEADER, "public")
            .json(&serde_json::json!({ "model": "fable-mini", "input": "x" }));
        if let Some(t) = bearer {
            r = r.bearer_auth(t);
        }
        r.send()
    };
    let r = route(Some("web-secret")).await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "the bound floor is public, so the credential may send public");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["data_class"], "public", "{v}");
    assert_eq!(v["provider"], b.node_id().to_string(), "{v}");
    // Without the credential: refused at the gateway (named tokens close it), never routed as public.
    let r = route(None).await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    a.shutdown_with_timeout(Duration::from_secs(5)).await;
    b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Finding B, through the gateway: a client serve stream is refused on a kind this node serves in process
/// (`403 served_in_process`, naming it), and a stream already holding a kind ends when an in-process receiver
/// registers for it. Seen failing first: the stream opened `200` beside the receiver, and stayed open after one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_rpc_serve_stream_never_shares_a_kind_with_the_nodes_own_receiver() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsBlobStore::open(dir.path()).unwrap());
    let (agent, _base, http_port) = start_gateway_node(store, None).await;
    let http = reqwest::Client::new();
    let url = |kind: &str| format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/{kind}");

    let _own = agent.service().rpc_rx("cap.invoke.demo.echo");
    let r = http.get(url("cap.invoke.demo.echo")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "served_in_process", "{v}");
    assert_eq!(v["kind"], "cap.invoke.demo.echo", "{v}");

    // The reverse order: a stream holding a kind ends when the node registers its own receiver for it.
    let mut stream = http.get(url("depot.work")).send().await.unwrap();
    assert_eq!(stream.status().as_u16(), 200);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _later = agent.service().rpc_rx("depot.work");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        // Keep-alive comments until the stream ends.
        while let Ok(Some(_)) = stream.chunk().await {}
    })
    .await;
    assert!(ended.is_ok(), "the evicted stream ends");
    let r = http.get(url("depot.work")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "and the kind is the node's now");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
