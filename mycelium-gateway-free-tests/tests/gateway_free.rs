//! A node built without the `gateway` feature (realignment repairs R8).
//!
//! Such a build runs no HTTP server. Before R8 it accepted an `http_port` and a `[gateway_tls]` table
//! and ignored them — and still advertised `http_port` as its bulk-transfer port, which nothing served
//! — while the guarantee report resolved the gateway guarantees from the configuration, so
//! `gw.not_open` read `enforced` for a gateway that did not exist.

use mycelium::{GossipAgent, GossipConfig, GossipError, NodeId};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn node(cfg: GossipConfig) -> GossipAgent {
    let port = free_port();
    let mut cfg = cfg;
    cfg.bind_port = port;
    GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
}

/// Every other test here means something only if this holds.
#[test]
fn this_build_has_no_gateway() {
    let r = node(GossipConfig::default()).guarantee_report();
    assert!(
        !r.features.contains(&"gateway"),
        "`mycelium` was built with `gateway` ({:?}): run `cargo test -p mycelium-gateway-free-tests` on its own, \
         not in one invocation with a crate that enables the feature",
        r.features
    );
}

#[tokio::test]
async fn an_http_port_refuses_the_start() {
    let mut cfg = GossipConfig::default();
    cfg.http_port = Some(free_port());
    match node(cfg).start().await {
        Err(GossipError::InvalidField { field, .. }) => assert_eq!(field, "http_port"),
        other => panic!("a build without the gateway must not start with an http_port: {other:?}"),
    }
}

#[tokio::test]
async fn a_node_without_gateway_settings_starts() {
    let agent = node(GossipConfig::default());
    agent.start().await.expect("a gateway-free node with no gateway settings starts");
    agent.shutdown().await;
}

#[test]
fn the_gateway_guarantees_read_not_in_build() {
    let mut cfg = GossipConfig::default();
    cfg.http_port = Some(free_port());
    cfg.gateway_auth_token = Some("s3cret".into());
    let r = node(cfg).guarantee_report();
    for id in ["gw.not_open", "gw.token_tables", "gw.oidc", "gw.tls", "gw.caller_profile"] {
        let e = r.entries.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} is registered"));
        assert_eq!(e.resolution.state(), "not_in_build", "{id}");
    }
}
