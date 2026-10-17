//! A loopback node an unmodified A2A client can use — the node half of the `a2a-sdk` CI test
//! (post-360 plan row N; the test is `tests/a2a-sdk/`, run by the `a2a-sdk` CI job).
//!
//! `/a2a` is mounted (`with_a2a`), one skill is served (`depot/dispatch`, answering
//! `dispatched: <text>`) and declared the default (`with_a2a_default_skill`), because a standard
//! client — `a2a-sdk`, Strands Agents — sends a message and never a skill. The gossip port is `PORT`
//! (default 7311) and the gateway `PORT + 1`. It prints `ready http=<port>` once a send can be
//! dispatched — the skill advertised and the node's caller-context marker written, without which the
//! gateway refuses to route to it — and runs until it is killed.
//!
//! One bearer token, `A2A_TOKEN` (default `a2a-sdk-test`), names a principal: `/a2a` admits a caller
//! with none as anonymous, but a task is read back (`GetTask`) only by the identity that created it,
//! and an anonymous caller has none.
//!
//! ```bash
//! cargo run --example a2a_sdk_node --features a2a,compliance &
//! MYCELIUM_A2A_URL=http://127.0.0.1:7312 MYCELIUM_A2A_TOKEN=a2a-sdk-test MYCELIUM_A2A_LIVE_REQUIRED=1 \
//!   pytest tests/a2a-sdk -v
//! ```
use std::{sync::Arc, time::Duration};

use mycelium::{capability::Capability, GossipAgent, GossipConfig, NodeId};

#[tokio::main]
async fn main() {
    let gossip: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7311);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(gossip + 1);
    let token = std::env::var("A2A_TOKEN").unwrap_or_else(|_| "a2a-sdk-test".into());
    cfg.gateway_scoped_tokens = vec![mycelium::GatewayToken { token, scopes: vec!["kv:read".into()] }];
    let agent = Arc::new(
        GossipAgent::new(NodeId::new("127.0.0.1", gossip).expect("node id"), cfg)
            .with_a2a()
            .with_a2a_default_skill("depot", "dispatch"),
    );
    agent.start().await.expect("start");

    let mut rx = agent.service().rpc_rx("skill.invoke");
    let provider = Arc::clone(&agent);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let reply = format!("dispatched: {}", String::from_utf8_lossy(&req.payload()));
            provider.service().rpc_respond(&req, reply.into_bytes());
        }
    });
    let _cap = agent.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));

    // Ready means a send dispatches: the skill is in the store, and so is this node's caller-context
    // marker (published lazily, after a grace period on a node with no peers).
    let me = agent.node_id().to_string();
    for key in [format!("cap/{me}/depot/dispatch"), format!("sys/caller-context/{me}")] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while agent.kv().get(&key).is_none() {
            assert!(tokio::time::Instant::now() < deadline, "{key} was never written");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    println!("ready http={}", gossip + 1);

    let stop = mycelium::shutdown::ShutdownSignal::install().expect("signal handlers");
    stop.wait().await.expect("wait for a stop signal");
    agent.shutdown().await;
}
