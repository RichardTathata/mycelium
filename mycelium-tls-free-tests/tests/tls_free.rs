//! A node built without the `tls` feature (I1 audit findings `mesh.tls`, `gw.tls`, 2026-10-02).
//!
//! The TLS init and the gateway's HTTPS branch are compiled out of such a build, so a `[tls]` or `[gateway_tls]`
//! table used to be accepted and ignored: gossip ran unauthenticated and the gateway served bearer tokens in
//! cleartext, with no warning. `start()` now refuses each by name. This test lived in `src/lib_tests.rs` under
//! `#[cfg(not(feature = "tls"))]`, where it never ran: the root crate's dev-dependencies turn `tls` on.

use std::time::Duration;

use mycelium::{GatewayTlsConfig, GossipAgent, GossipConfig, GossipError, NodeId, TlsConfig};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Every other test here means something only if this holds.
#[test]
fn this_build_has_the_gateway_and_no_tls() {
    let port = free_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let r = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg).guarantee_report();
    assert!(
        !r.features.contains(&"tls") && r.features.contains(&"gateway"),
        "`mycelium` was built with {:?}: run `cargo test -p mycelium-tls-free-tests` on its own, \
         not in one invocation with a crate that enables `tls`",
        r.features
    );
}

#[tokio::test]
async fn a_tls_table_this_build_cannot_enforce_refuses_to_start() {
    for field in ["tls", "gateway_tls"] {
        let port = free_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        match field {
            "tls" => cfg.tls = Some(TlsConfig { auto_cert_dir: std::env::temp_dir().join(format!("tls-{port}")), ..Default::default() }),
            _ => {
                cfg.http_port = Some(free_port());
                cfg.gateway_tls = Some(GatewayTlsConfig { cert_pem_path: Some("c.pem".into()), key_pem_path: Some("k.pem".into()) });
            }
        }
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        match agent.start().await {
            Err(GossipError::InvalidField { field: f, reason }) => {
                assert_eq!(f, field);
                assert!(reason.contains("`tls`"), "the refusal names the missing feature: {reason}");
            }
            other => {
                let _ = agent.shutdown_with_timeout(Duration::from_secs(5)).await;
                panic!("[{field}] without the `tls` feature must refuse to start, got {other:?}");
            }
        }
    }
}
