//! External review 2026-10-31, F1 (plan row R1, decision D1): a tuple space's write kinds through the gateway's
//! raw routes need `tuple:write`, as `/gateway/tuple/put` does. Before this, a `mesh:write`-only token was refused
//! on `tuple/put` and admitted on `rpc/call` with `method: tuple.<ns>.put` — and the tuple was there to take.
#![cfg(feature = "gateway")]

use std::sync::Arc;
use std::time::Duration;

use mycelium::{GatewayNamedToken, GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};

fn b64(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// The wire form of a put request, as `rpc::enc_put_req` writes it (`pub(crate)`, so spelled out here).
fn put_req(stage: &str, payload: &[u8]) -> Vec<u8> {
    let mut buf = (stage.len() as u16).to_le_bytes().to_vec();
    buf.extend_from_slice(stage.as_bytes());
    buf.extend_from_slice(payload);
    buf
}

fn token(name: &str, token: &str, scopes: &[&str]) -> GatewayNamedToken {
    GatewayNamedToken { name: name.into(), token: token.into(), scopes: scopes.iter().map(|s| s.to_string()).collect() }
}

/// Seen failing first with the gate's table returning `None` for every kind: the `mesh:write`-only raw call answered
/// 200 and the tuple was taken afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tuple_put_through_a_raw_route_needs_tuple_write_as_the_spaces_own_route_does() {
    let gossip = mycelium::test_util::alloc_port();
    let http = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig { bind_port: gossip, http_port: Some(http), health_check_max_jitter_ms: 50, ..Default::default() };
    cfg.gateway_named_tokens = vec![
        token("emitter", "t-mesh", &["mesh:write"]),
        token("both", "t-both", &["mesh:write", "tuple:write"]),
        token("other", "t-board", &["mesh:write", "board:write"]),
    ];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip).unwrap(), cfg));
    let ts = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig { namespace: Arc::from("work"), role: TupleRole::Primary, persist: false, ..Default::default() },
    ).await.unwrap();
    agent.with_http_routes(Arc::clone(&ts).http_router());
    agent.start().await.unwrap();
    let base = format!("http://127.0.0.1:{http}");
    let c = reqwest::Client::new();
    for _ in 0..100 {
        if c.get(format!("{base}/health")).send().await.is_ok() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let me = agent.node_id().to_string();
    let raw_put = |tok: &'static str| {
        c.post(format!("{base}/gateway/rpc/call")).bearer_auth(tok).json(&serde_json::json!({
            "target": me, "method": "tuple.work.put", "payload_b64": b64(&put_req("in", b"via-rpc")), "timeout_secs": 5,
        })).send()
    };

    let r = c.post(format!("{base}/gateway/tuple/put")).bearer_auth("t-mesh")
        .json(&serde_json::json!({ "ns": "work", "stage": "in", "payload_b64": b64(b"direct") })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "tuple/put admitted mesh:write");

    let r = raw_put("t-mesh").await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "F1: the raw route admitted a tuple put under mesh:write");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "tuple:write", "{v}");
    let r = raw_put("t-board").await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "another family's write scope carried a tuple put");
    assert!(ts.take("in", Duration::from_millis(300)).await.is_err(), "a refused put reached the space");

    let r = raw_put("t-both").await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "mesh:write + tuple:write was refused the space's kind");
    let taken = ts.take("in", Duration::from_secs(2)).await.unwrap();
    assert_eq!(&taken.1[..], b"via-rpc");
    agent.shutdown().await;
}
