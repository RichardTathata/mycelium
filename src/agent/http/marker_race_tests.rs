//! #802: **the gateway judges only what it holds.** A provider promises what it enforces by publishing its
//! `sys/caller-context/{node}` marker; a gateway sends a client's call only to a provider whose promise it has seen.
//! The marker and a provider's `tools/` or `cap/` entry are separate keys, and gossip does not deliver separate keys in
//! the order they were written, so a gateway can hold the advertisement first. Until the marker arrives that provider
//! is not yet one the gateway can send to: every door that chooses a provider passes it over, and with none left the
//! client is answered the door's own "no provider" — the answer it already retries — never -32021.

use crate::{GossipAgent, GossipConfig, NodeId, RequestPrincipal};
use std::{sync::{Arc, Mutex}, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

async fn poll_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if cond() { return true; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

fn node_at(gossip_port: u16, http_port: Option<u16>, boot: Vec<NodeId>) -> Arc<GossipAgent> {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = http_port;
    cfg.bootstrap_peers = boot;
    cfg.reconnect_backoff_secs = 1;
    Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg))
}

/// Plant at `at` an advertisement `key` as `origin` wrote it, without its marker: what a gateway holds when gossip
/// delivered the provider's advertisement before its caller-context marker.
fn plant(at: &GossipAgent, origin: &NodeId, key: &str, value: bytes::Bytes) {
    let upd = crate::framing::make_gossip_update(origin, 5, Arc::from(key), value, false, &at.task_ctx.hlc);
    crate::store::apply_and_notify(&at.task_ctx.kv_state, &upd);
}

async fn tools_listed(http_port: u16) -> Vec<String> {
    let v: serde_json::Value = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send().await.expect("tools/list").json().await.unwrap();
    v["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str().map(String::from)).collect()
}

async fn tools_call(http_port: u16, tool: &str) -> serde_json::Value {
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {}}}))
        .send().await.expect("tools/call request");
    assert_eq!(resp.status(), 200, "tools/call is answered as JSON-RPC");
    resp.json().await.unwrap()
}

fn observing_tool(agent: &GossipAgent) -> (crate::McpToolHandle, Arc<Mutex<Option<RequestPrincipal>>>) {
    let seen: Arc<Mutex<Option<RequestPrincipal>>> = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let handle = agent.mcp().register_mcp_tool_with_principal("whoami", serde_json::json!({"type": "object"}), move |who, _args| {
        let seen = Arc::clone(&seen2);
        async move {
            let name = who.name();
            *seen.lock().unwrap() = Some(who);
            Ok(serde_json::json!(name))
        }
    });
    (handle, seen)
}

/// **A tool whose provider's marker the gateway does not yet hold is not yet a tool it can send to** — not listed, not
/// sent — and it is listed and served once the marker arrives. The provider's tool entry reaches the gateway first (planted, as gossip may deliver
/// it): the client's `tools/call` is answered `-32601 tool not found`, the answer a client gets for a tool not yet
/// advertised, which it retries — not `-32021 provider_without_caller_context`. Then the provider starts, its marker
/// reaches the gateway, and the same call is served with the client as the caller. Seen failing first on c50f7bde:
/// the first call was answered -32021 naming the provider.
#[tokio::test]
async fn a_tool_seen_before_its_providers_marker_is_answered_not_found_and_served_once_the_marker_arrives() {
    let gw_http = alloc_port();
    let gateway = node_at(alloc_port(), Some(gw_http), vec![]);
    gateway.start().await.unwrap();
    let provider_port = alloc_port();
    let provider_id = NodeId::new("127.0.0.1", provider_port).unwrap();
    let tool_key = format!("tools/whoami/{provider_id}");
    plant(&gateway, &provider_id, &tool_key, bytes::Bytes::from_static(br#"{"type":"object"}"#));
    assert!(gateway.kv().get(&super::super::gateway_caller::marker_key(&provider_id)).is_none(), "the marker has not arrived");

    assert!(!tools_listed(gw_http).await.contains(&"whoami".to_string()), "NOT LISTED: tools/list agrees with tools/call");
    let early = tools_call(gw_http, "whoami").await;
    assert_eq!(early["error"]["code"], -32601, "NOT YET A PROVIDER: the tool's only provider has not been seen to promise: {early}");
    assert_ne!(early["error"]["data"]["reason"], "provider_without_caller_context", "{early}");

    let provider = node_at(provider_port, None, vec![gateway.node_id().clone()]);
    provider.start().await.unwrap();
    let (_tool, seen) = observing_tool(&provider);
    let marker = super::super::gateway_caller::marker_key(&provider_id);
    assert!(poll_until(|| gateway.kv().get(&marker).is_some(), Duration::from_secs(15)).await, "the provider's marker reaches the gateway");
    assert!(tools_listed(gw_http).await.contains(&"whoami".to_string()), "listed once the marker arrived");
    let served = tools_call(gw_http, "whoami").await;
    assert!(served.get("error").is_none(), "SERVED once the marker arrived: {served}");
    assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Client(_))), "the client is the caller");
    provider.shutdown().await;
    gateway.shutdown().await;
}

/// **With two providers, the one whose promise the gateway holds is chosen**, though the other sorts first. Seen failing
/// first on c50f7bde: refused -32021 naming the unpromised provider, the first `tools/{name}/` record.
#[tokio::test]
async fn a_call_is_routed_past_a_provider_whose_marker_the_gateway_does_not_hold() {
    let gw_http = alloc_port();
    let gateway = node_at(alloc_port(), Some(gw_http), vec![]);
    gateway.start().await.unwrap();
    let provider = node_at(alloc_port(), None, vec![gateway.node_id().clone()]);
    provider.start().await.unwrap();
    let (_tool, seen) = observing_tool(&provider);
    let marker = super::super::gateway_caller::marker_key(provider.node_id());
    let tool_key = format!("tools/whoami/{}", provider.node_id());
    assert!(poll_until(|| gateway.kv().get(&marker).is_some() && gateway.kv().get(&tool_key).is_some(), Duration::from_secs(15)).await,
        "the gateway holds the provider's tool and marker");
    // A second provider whose tool entry arrived first, its marker not yet: it sorts before the real one.
    let unseen = NodeId::new("127.0.0.1", 1).unwrap();
    plant(&gateway, &unseen, &format!("tools/whoami/{unseen}"), bytes::Bytes::from_static(br#"{"type":"object"}"#));
    let first = crate::store::scan_kv_prefix(&gateway.task_ctx.kv_state, "tools/whoami/").into_iter().next().map(|(k, _)| k.to_string());
    assert_eq!(first.as_deref(), Some(format!("tools/whoami/{unseen}").as_str()), "the unpromised provider is the first record");
    let body = tools_call(gw_http, "whoami").await;
    assert!(body.get("error").is_none(), "ROUTED to the promised provider: {body}");
    assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Client(_))));
    provider.shutdown().await;
    gateway.shutdown().await;
}

/// **The LLM doors pass over a provider whose promise the gateway does not hold** and answer `404 no_provider` when
/// none is left — `/gateway/llm/call` by status, `/gateway/llm/stream` by its one in-stream event. Seen failing first
/// on c50f7bde: `/gateway/llm/call` answered `412 provider_without_caller_context`.
#[cfg(feature = "llm")]
#[tokio::test]
async fn the_llm_doors_answer_no_provider_for_a_provider_whose_marker_is_not_held() {
    let gw_http = alloc_port();
    let gateway = node_at(alloc_port(), Some(gw_http), vec![]);
    gateway.start().await.unwrap();
    let unseen = NodeId::new("127.0.0.1", 1).unwrap();
    plant(&gateway, &unseen, &format!("cap/{unseen}/demo/greet"), crate::capability::Capability::new("demo", "greet").encode());
    let client = reqwest::Client::new();
    let body = serde_json::json!({"ns": "demo", "name": "greet", "input": "hi"});
    let call = client.post(format!("http://127.0.0.1:{gw_http}/gateway/llm/call")).json(&body).send().await.unwrap();
    let status = call.status().as_u16();
    let v: serde_json::Value = call.json().await.unwrap();
    assert_eq!((status, v["error"].as_str()), (404, Some("no_provider")), "NOT YET A PROVIDER at /gateway/llm/call: {v}");
    let stream = client.post(format!("http://127.0.0.1:{gw_http}/gateway/llm/stream")).json(&body).send().await.unwrap().text().await.unwrap();
    assert!(stream.contains("\"no_provider\""), "NOT YET A PROVIDER at /gateway/llm/stream: {stream}");
    gateway.shutdown().await;
}

/// **`/a2a` passes over a skill's provider whose promise the gateway does not hold**, and with none left answers the
/// unresolved skill it answers for a skill nobody advertises. Seen failing first on c50f7bde: refused naming
/// `does not enforce the gateway caller context`.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn a2a_answers_an_unresolved_skill_for_a_provider_whose_marker_is_not_held() {
    let gw_http = alloc_port();
    let gw_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gw_port;
    cfg.http_port = Some(gw_http);
    let gateway = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gw_port).unwrap(), cfg).with_a2a());
    gateway.start().await.unwrap();
    let unseen = NodeId::new("127.0.0.1", 1).unwrap();
    plant(&gateway, &unseen, &format!("cap/{unseen}/depot/dispatch"), crate::capability::Capability::new("depot", "dispatch").encode());
    let params = serde_json::json!({"id": "t-802", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
    let answer = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{gw_http}/a2a"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tasks/send", "params": params}))
        .send().await.unwrap().text().await.unwrap();
    assert!(answer.contains("skill not found"), "NOT YET A PROVIDER at /a2a: {answer}");
    assert!(!answer.contains("caller context"), "{answer}");
    // The card agrees with `tasks/send`: a skill whose providers have not been seen to promise is not offered.
    let card: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{gw_http}/.well-known/agent-card.json")).await.unwrap().json().await.unwrap();
    assert!(!card.to_string().contains("depot/dispatch"), "NOT ON THE CARD: {card}");
    gateway.shutdown().await;
}
