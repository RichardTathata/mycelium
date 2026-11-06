//! **A bulk fetch ignores the environment's proxy** (external review R7, its PR's review round 2,
//! finding 10). Both bulk fetch clients are built with `.no_proxy()`: a fetch goes to the address the
//! caller's mesh frames came from, and an `HTTP(S)_PROXY` would put a third party between two
//! members. reqwest reads the proxy variables when a client is built, so they are set before either
//! agent exists, to an address that refuses every connection (`127.0.0.1:9`); a client that honoured
//! them would fail every fetch and the call would time out.
//!
//! Its own test binary, with one test, because it sets process-wide environment variables that any
//! other HTTP client built in the same process would read.
#![cfg(all(feature = "gateway", feature = "tls"))]

use bytes::Bytes;
use mycelium::{GatewayTlsConfig, GossipAgent, GossipConfig, NodeId, TlsConfig};
use std::{sync::Arc, time::Duration};

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn pair(tls_dir: Option<&std::path::Path>) -> (Arc<GossipAgent>, Arc<GossipAgent>, NodeId) {
    let mk = |boot: Vec<NodeId>| {
        let p = port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = p;
        cfg.http_port = Some(port());
        cfg.http_addr = "127.0.0.1".to_string();
        cfg.bootstrap_peers = boot;
        cfg.health_check_max_jitter_ms = 1;
        if let Some(dir) = tls_dir {
            cfg.tls = Some(TlsConfig { auto_cert_dir: dir.to_path_buf(), ..Default::default() });
            cfg.gateway_tls = Some(GatewayTlsConfig::default());
        }
        (NodeId::new("127.0.0.1", p).unwrap(), cfg)
    };
    let (ida, cfga) = mk(vec![]);
    let (idb, cfgb) = mk(vec![ida.clone()]);
    let a = Arc::new(GossipAgent::new(ida, cfga));
    a.start().await.unwrap();
    let b = Arc::new(GossipAgent::new(idb.clone(), cfgb));
    b.start().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while a.peers().is_empty() || b.peers().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the pair never connected");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (a, b, idb)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bulk_fetch_ignores_the_environment_proxy() {
    for var in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
        // SAFETY: this binary holds one test, so no other thread reads the environment while it is set.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var(var, "http://127.0.0.1:9") };
    }
    #[allow(unsafe_code)]
    unsafe { std::env::remove_var("NO_PROXY"); std::env::remove_var("no_proxy") };

    let dir = std::env::temp_dir().join(format!("bulk-noproxy-{}-{}", std::process::id(), port()));
    let _ = std::fs::remove_dir_all(&dir);
    for (what, tls) in [("plaintext", None), ("https", Some(dir.as_path()))] {
        let (a, b, idb) = pair(tls).await;
        let _serve = b.service().bulk_serve("echo-bulk", |_s, p| async move { p });
        let payload = Bytes::from(vec![6u8; 20_000]);
        let got = a.service().bulk_call(idb, "echo-bulk", payload.clone(), Duration::from_secs(5)).await;
        a.shutdown().await;
        b.shutdown().await;
        assert_eq!(got, Ok(payload), "{what}: the fetch went direct, past the environment's proxy");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
