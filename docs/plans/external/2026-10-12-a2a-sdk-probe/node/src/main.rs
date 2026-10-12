//! A loopback Mycelium node with `/a2a` mounted and one skill (`depot/dispatch`) served, for an A2A client probe.
use std::{sync::Arc, time::Duration};
use mycelium::{GossipAgent, GossipConfig, NodeId};

#[tokio::main]
async fn main() {
    let gossip: u16 = std::env::var("PORT").unwrap().parse().unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(gossip + 1);
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip).unwrap(), cfg).with_a2a());
    agent.start().await.expect("start");
    let mut rx = agent.service().rpc_rx("skill.invoke");
    let a = Arc::clone(&agent);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await { a.service().rpc_respond(&req, b"dispatched".to_vec()); }
    });
    let _cap = agent.capabilities().advertise_capability(
        mycelium::capability::Capability::new("depot", "dispatch"), Duration::from_secs(600));
    println!("ready http={}", gossip + 1);
    tokio::time::sleep(Duration::from_secs(900)).await;
}
