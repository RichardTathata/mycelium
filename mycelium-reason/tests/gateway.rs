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
    let tr = TraceRecorder::new(Arc::clone(&agent), "gw-run");
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
    let mut started = None;
    for _ in 0..16 {
        let base = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let http_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = base;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = auth_token.map(str::to_owned);
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

    // Wait until A's router sees both providers (both cap ads gossiped + B live in SWIM).
    let filter = mycelium::CapFilter::new("llm", "fable-mini");
    for _ in 0..200 {
        if agent_a.capabilities().resolve(&filter).len() == 2 && agent_a.peers().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(agent_a.capabilities().resolve(&filter).len(), 2, "both providers visible");

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

