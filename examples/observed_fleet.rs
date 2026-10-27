//! **A three-node fleet to point the Ops Console at** (adopter-surface Y3, D28) — and the fleet CI
//! renders the console against.
//!
//! Three nodes in one process, each with a gateway on loopback (`:9061`, `:9062`, `:9063` by default),
//! the tuple-space companion mounted on each (so `/gateway/tuple/overview` answers) and a little work in
//! it, and **`gateway_cors_origins`** set to the console's origin, so a browser page served from that
//! origin may read each node's read-only routes directly. Nothing here aggregates: the console fans
//! out to the three gateways from the browser and merges their answers itself.
//!
//! ```bash
//! cargo run --example observed_fleet --features metrics        # origin http://127.0.0.1:8099
//! python3 -m http.server 8099 --bind 127.0.0.1 -d examples/ops_console
//! # open http://127.0.0.1:8099/?nodes=127.0.0.1:9061,127.0.0.1:9062,127.0.0.1:9063
//! ```
//!
//! Environment: `OBSERVED_FLEET_CORS` (comma-separated origins; default `http://127.0.0.1:8099`),
//! `OBSERVED_FLEET_TOKEN` (a gateway bearer every node requires; default none — loopback only),
//! `OBSERVED_FLEET_PORT` (the first gateway port; default `9061`). It runs until Ctrl-C or SIGTERM.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};

const NODES: u16 = 3;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let origins: Vec<String> = std::env::var("OBSERVED_FLEET_CORS")
        .unwrap_or_else(|_| "http://127.0.0.1:8099".into())
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(String::from)
        .collect();
    let token = std::env::var("OBSERVED_FLEET_TOKEN").ok().filter(|t| !t.is_empty());
    let first_http: u16 = std::env::var("OBSERVED_FLEET_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9061);
    let first_gossip = first_http + 100;

    let shutdown = mycelium::shutdown::ShutdownSignal::install()?;
    let mut agents = Vec::new();
    let mut spaces = Vec::new();
    for i in 0..NODES {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = first_gossip + i;
        cfg.http_port = Some(first_http + i);
        cfg.cluster_name = Some("observed".into());
        cfg.gateway_auth_token = token.clone();
        cfg.gateway_cors_origins = origins.clone();
        cfg.bootstrap_peers = (0..NODES)
            .filter(|j| *j != i)
            .map(|j| NodeId::new("127.0.0.1", first_gossip + j))
            .collect::<Result<_, _>>()?;
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", first_gossip + i)?, cfg));
        let ts = TupleSpace::new(
            Arc::clone(&agent),
            TupleConfig {
                namespace: Arc::from("observed"),
                role: if i == 0 { TupleRole::Primary } else { TupleRole::Client },
                cap_refresh: Duration::from_secs(2),
                heartbeat_interval: Duration::from_secs(1),
                ..Default::default()
            },
        )
        .await?;
        agent.with_http_routes(Arc::clone(&ts).http_router());
        agent.start().await?;
        agents.push(agent);
        spaces.push(ts);
    }

    // A little work, so the tuple overview and the KV panel have something to show.
    for (i, agent) in agents.iter().enumerate() {
        let _ = agent.kv().set(format!("app/observed/node-{i}"), format!("hello from node {i}"));
    }
    for n in 0..3u8 {
        if let Err(e) = spaces[0].put("intake", Bytes::from(vec![n])).await {
            eprintln!("observed-fleet: tuple put: {e}");
        }
    }

    let urls: Vec<String> = (0..NODES).map(|i| format!("127.0.0.1:{}", first_http + i)).collect();
    println!("observed-fleet: ready — gateways {}", urls.join(" "));
    println!("  cross-origin reads answered for {origins:?} only");
    println!("  console: http://127.0.0.1:8099/?nodes={}", urls.join(","));

    shutdown.wait().await?;
    for agent in &agents {
        agent.shutdown().await;
    }
    Ok(())
}
