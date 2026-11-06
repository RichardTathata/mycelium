//! External review 2026-10-31, F1 (plan row R1, decision D1): the board's write kind through the gateway's raw
//! routes needs `board:write`, as `/gateway/bb/post` does. Before this, a `mesh:write`-only token was refused
//! on `bb/post` and admitted on `rpc/call` with `method: blackboard.<ns>.post` — and the fact landed on the
//! board. Driven end to end here: the companion's own route, the raw route, and the board's contents after each.
#![cfg(feature = "gateway")]

use std::sync::Arc;
use std::time::Duration;

use mycelium::{GatewayNamedToken, GossipAgent, GossipConfig, NodeId};
use mycelium_blackboard::{Blackboard, BoardConfig, BoardRole, Predicate};

fn b64(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// The wire form of a post request, as `rpc::enc_post_req` writes it (`pub(crate)`, so spelled out here).
fn post_req(attrs: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
    for (k, v) in attrs {
        buf.extend_from_slice(&(k.len() as u16).to_le_bytes());
        buf.extend_from_slice(k.as_bytes());
        buf.extend_from_slice(&(v.len() as u16).to_le_bytes());
        buf.extend_from_slice(v.as_bytes());
    }
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

fn token(name: &str, token: &str, scopes: &[&str]) -> GatewayNamedToken {
    GatewayNamedToken { name: name.into(), token: token.into(), scopes: scopes.iter().map(|s| s.to_string()).collect() }
}

/// Seen failing first with the gate's table returning `None` for every kind: the `mesh:write`-only raw call answered
/// 200 and the board held its fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_write_through_a_raw_route_needs_board_write_as_the_boards_own_route_does() {
    let gossip = mycelium::test_util::alloc_port();
    let http = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig { bind_port: gossip, http_port: Some(http), health_check_max_jitter_ms: 50, ..Default::default() };
    cfg.gateway_named_tokens = vec![
        token("emitter", "t-mesh", &["mesh:write"]),
        token("boarder", "t-board", &["board:write"]),
        token("both", "t-both", &["mesh:write", "board:write"]),
        token("reader", "t-read", &["mesh:write", "board:read"]),
    ];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip).unwrap(), cfg));
    let bb = Blackboard::new(
        Arc::clone(&agent),
        BoardConfig { namespace: Arc::from("board"), role: BoardRole::Primary, persist: false, ..Default::default() },
    ).await.unwrap();
    agent.with_http_routes(Arc::clone(&bb).http_router());
    agent.start().await.unwrap();
    let base = format!("http://127.0.0.1:{http}");
    let c = reqwest::Client::new();
    for _ in 0..100 {
        if c.get(format!("{base}/health")).send().await.is_ok() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let me = agent.node_id().to_string();
    let raw_post = |tok: &'static str, op: &'static str| {
        c.post(format!("{base}/gateway/rpc/call")).bearer_auth(tok).json(&serde_json::json!({
            "target": me, "method": format!("blackboard.board.{op}"),
            "payload_b64": b64(&post_req(&[("kind", "surplus")], b"via-rpc")), "timeout_secs": 5,
        })).send()
    };
    let facts = || async { bb.read(&Predicate::new().eq("kind", "surplus")).await.unwrap().len() };

    // The board's own route: refused without `board:write`, admitted with it.
    let r = c.post(format!("{base}/gateway/bb/post")).bearer_auth("t-mesh")
        .json(&serde_json::json!({ "ns": "board", "attributes": { "kind": "surplus" }, "payload_b64": b64(b"direct") })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "bb/post admitted mesh:write");
    let r = c.post(format!("{base}/gateway/bb/post")).bearer_auth("t-board")
        .json(&serde_json::json!({ "ns": "board", "attributes": { "kind": "surplus" }, "payload_b64": b64(b"direct") })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "bb/post refused board:write");
    assert_eq!(facts().await, 1);

    // The raw route: `mesh:write` alone is refused naming `board:write`, and nothing lands.
    let r = raw_post("t-mesh", "post").await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "F1: the raw route admitted a board post under mesh:write");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "insufficient scope", "{v}");
    assert_eq!(v["required_scope"], "board:write", "{v}");
    assert_eq!(facts().await, 1, "the refused post reached the board");
    // `board:write` alone is refused the raw route too — the route's own scope is still required.
    let r = raw_post("t-board", "post").await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:write", "{v}");
    // The read scope does not carry the write verb.
    let r = raw_post("t-read", "post").await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    assert_eq!(facts().await, 1);
    // Both scopes: the raw call is the board's own operation, admitted, and the fact lands.
    let r = raw_post("t-both", "post").await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "mesh:write + board:write was refused the board's kind");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["ok"], serde_json::json!(true), "{v}");
    assert_eq!(facts().await, 2);
    // A read verb under the write-only token is refused naming `board:read`.
    let r = raw_post("t-both", "depth").await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "board:read", "{v}");
    agent.shutdown().await;
}
