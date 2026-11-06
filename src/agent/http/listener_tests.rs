//! The gateway's accept bounds (external review 2026-10-31, F5; `docs/plans/external-review-2026-10-31.md`
//! row R5). One test per exit promise, on each listener it names: a socket that connects and sends nothing
//! is closed within `handshake_timeout_ms`; the gateway serves at most `gateway_max_connections` at once;
//! N+1 silent sockets do not block a real request; shutdown closes a connection still in its handshake.
//!
//! "Closed" is observed from the client: a read that returns EOF or an error. A read that times out means
//! the server still holds the socket.

use crate::{GossipAgent, GossipConfig, NodeId};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// A plaintext gateway with the given bounds.
async fn plain_gateway(handshake_ms: u64, max_conns: usize) -> (Arc<GossipAgent>, u16) {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.handshake_timeout_ms   = handshake_ms;
    cfg.gateway_max_connections = max_conns;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    wait_listening(http_port).await;
    (agent, http_port)
}

/// Poll until the port accepts (the gateway task binds in `start()`; the accept loop may lag it).
async fn wait_listening(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("gateway on {port} never accepted");
}

/// Whether the server closes `s` within `within`, reading whatever it sends (a TLS alert, nothing).
async fn closed_within<S: tokio::io::AsyncRead + Unpin>(s: &mut S, within: Duration) -> bool {
    let mut buf = [0u8; 512];
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, s.read(&mut buf)).await {
            Err(_)       => return false,       // still open at the deadline
            Ok(Ok(0))    => return true,        // EOF
            Ok(Err(_))   => return true,        // reset
            Ok(Ok(_))    => continue,           // bytes (an alert, a 408) — keep reading to the close
        }
    }
}

/// `GET /health` on a fresh plaintext socket; `Some(status line)` if answered within `within`.
async fn health_within(port: u16, within: Duration) -> Option<String> {
    let fut = async {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.ok()?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.ok()?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        text.lines().next().map(str::to_string)
    };
    tokio::time::timeout(within, fut).await.ok().flatten()
}

#[tokio::test]
async fn a_silent_socket_is_closed_within_the_handshake_timeout_on_the_plaintext_gateway() {
    let (agent, port) = plain_gateway(300, 1024).await;
    let mut silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert!(closed_within(&mut silent, Duration::from_secs(3)).await,
        "a socket that connects and sends nothing must be closed within handshake_timeout_ms (300 ms)");
    assert!(agent.system_stats().gateway_connections_timed_out >= 1,
        "the close is counted (#719's review, finding 4)");
    agent.shutdown().await;
}

/// hyper-util decides HTTP/1 or HTTP/2 by reading up to the 24-byte HTTP/2 preface before hyper's own
/// header-read timer starts; a client that stops inside the preface must not escape the bound.
#[tokio::test]
async fn a_socket_that_stops_inside_the_http2_preface_is_closed() {
    let (agent, port) = plain_gateway(300, 1024).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"PRI * HTTP/2").await.unwrap();
    assert!(closed_within(&mut s, Duration::from_secs(3)).await,
        "a partial HTTP/2 preface must not hold the connection past handshake_timeout_ms");
    agent.shutdown().await;
}

/// The bound is on the preface, not on the request: a client whose request is under way is served.
#[tokio::test]
async fn a_request_sent_in_time_is_served_after_the_timeout_would_have_passed() {
    let (agent, port) = plain_gateway(300, 1024).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n").await.unwrap();
    // Past the preface bound, inside hyper's header-read bound (30 s): the head completes now.
    tokio::time::sleep(Duration::from_millis(600)).await;
    s.write_all(b"Connection: close\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut buf)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 200"),
        "a request whose preface arrived in time is served: {}", String::from_utf8_lossy(&buf));
    agent.shutdown().await;
}

/// The HTTP/2 preface, then nothing (#719's review, finding 1): the deadline must not disarm at the preface's
/// last byte, or one such socket holds a permit forever — with `gateway_max_connections = 1`, the gateway.
#[tokio::test]
async fn a_bare_http2_preface_then_silence_does_not_hold_the_only_permit() {
    let (agent, port) = plain_gateway(300, 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
    let answered = health_within(port, Duration::from_secs(5)).await;
    assert!(answered.as_deref().is_some_and(|l| l.starts_with("HTTP/1.1 200")),
        "a preface with no frame after it must not hold the only permit: {answered:?}");
    assert!(closed_within(&mut s, Duration::from_secs(1)).await, "and the preface-only socket is closed");
    agent.shutdown().await;
}

/// An HTTP/2 client that sends its preface and SETTINGS, then never answers (#719's review, finding 1): the
/// server's keep-alive PING, derived from `handshake_timeout_ms`, finds it gone and closes the connection.
#[tokio::test]
async fn an_http2_client_silent_after_its_settings_is_closed_by_the_keep_alive() {
    let (agent, port) = plain_gateway(300, 1024).await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
    // An empty SETTINGS frame: length 0, type 0x4, flags 0, stream 0.
    s.write_all(&[0, 0, 0, 0x4, 0, 0, 0, 0, 0]).await.unwrap();
    assert!(closed_within(&mut s, Duration::from_secs(5)).await,
        "an HTTP/2 connection that never answers a PING is closed within the keep-alive bound");
    agent.shutdown().await;
}

#[tokio::test]
async fn the_gateway_serves_no_more_than_gateway_max_connections_at_once() {
    // A long preface bound, so the two silent sockets hold their permits for the whole test.
    let (agent, port) = plain_gateway(30_000, 2).await;
    // `wait_listening`'s probe socket closed at once; give the server the moment to see its EOF.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let silent_a = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _silent_b = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(agent.system_stats().gateway_connections_open, 2, "both silent sockets hold a permit");
    assert_eq!(health_within(port, Duration::from_millis(800)).await, None,
        "with gateway_max_connections = 2 held, a third connection is not served");
    assert!(agent.system_stats().gateway_connection_cap_waits >= 1,
        "the accept loop's wait at the cap is counted (#719's review, finding 4)");
    drop(silent_a);
    let answered = health_within(port, Duration::from_secs(3)).await;
    assert!(answered.as_deref().is_some_and(|l| l.starts_with("HTTP/1.1 200")),
        "a returned permit admits the waiting connection: {answered:?}");
    agent.shutdown().await;
}

#[tokio::test]
async fn n_plus_one_silent_sockets_do_not_block_a_real_request_on_the_plaintext_gateway() {
    let (agent, port) = plain_gateway(300, 2).await;
    let mut _silent = Vec::new();
    for _ in 0..3 { _silent.push(TcpStream::connect(("127.0.0.1", port)).await.unwrap()); }
    let answered = health_within(port, Duration::from_secs(5)).await;
    assert!(answered.as_deref().is_some_and(|l| l.starts_with("HTTP/1.1 200")),
        "three silent sockets against a cap of two expire and admit a real request: {answered:?}");
    agent.shutdown().await;
}

#[tokio::test]
async fn shutdown_closes_a_connection_still_in_its_preface_on_the_plaintext_gateway() {
    let (agent, port) = plain_gateway(30_000, 1024).await;
    let mut silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let a = Arc::clone(&agent);
    let stop = tokio::spawn(async move { a.shutdown().await });
    assert!(closed_within(&mut silent, Duration::from_secs(3)).await,
        "shutdown releases a connection waiting on its preface");
    stop.await.unwrap();
}

#[cfg(feature = "tls")]
mod tls {
    use super::*;

    struct TlsGateway {
        agent: Arc<GossipAgent>,
        port: u16,
        connector: tokio_rustls::TlsConnector,
        cert_dir: std::path::PathBuf,
    }

    impl TlsGateway {
        async fn start(handshake_ms: u64, max_conns: usize) -> Self {
            let gossip_port = alloc_port();
            let port        = alloc_port();
            let cert_dir = std::env::temp_dir()
                .join(format!("gw-bounds-{}-{gossip_port}", std::process::id()));
            let _ = std::fs::remove_dir_all(&cert_dir);
            let mut cfg = GossipConfig::default();
            cfg.bind_port = gossip_port;
            cfg.http_port = Some(port);
            cfg.http_addr = "127.0.0.1".to_string();
            cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
            cfg.gateway_tls = Some(crate::GatewayTlsConfig::default());
            cfg.handshake_timeout_ms    = handshake_ms;
            cfg.gateway_max_connections = max_conns;
            let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
            agent.start().await.unwrap();
            wait_listening(port).await;
            let ca_pem = std::fs::read(cert_dir.join("ca-cert.pem")).expect("ca-cert.pem written");
            let mut roots = rustls::RootCertStore::empty();
            for c in rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_pem)) {
                roots.add(c.unwrap()).unwrap();
            }
            let client = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
            Self { agent, port, connector: tokio_rustls::TlsConnector::from(Arc::new(client)), cert_dir }
        }

        async fn handshake(&self) -> tokio_rustls::client::TlsStream<TcpStream> {
            let tcp = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
            let name = rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into());
            self.connector.connect(name, tcp).await.expect("TLS handshake against the gateway")
        }

        async fn health_within(&self, within: Duration) -> Option<String> {
            let fut = async {
                let tcp = TcpStream::connect(("127.0.0.1", self.port)).await.ok()?;
                let name = rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into());
                let mut s = self.connector.connect(name, tcp).await.ok()?;
                s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.ok()?;
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await; // a close_notify-less close is an Err here
                String::from_utf8_lossy(&buf).lines().next().map(str::to_string)
            };
            tokio::time::timeout(within, fut).await.ok().flatten()
        }

        async fn stop(self) {
            self.agent.shutdown().await;
            let _ = std::fs::remove_dir_all(&self.cert_dir);
        }
    }

    #[tokio::test]
    async fn a_silent_socket_is_closed_within_the_handshake_timeout_on_the_tls_gateway() {
        let gw = TlsGateway::start(300, 1024).await;
        let mut silent = TcpStream::connect(("127.0.0.1", gw.port)).await.unwrap();
        assert!(closed_within(&mut silent, Duration::from_secs(3)).await,
            "a socket that never sends a ClientHello must be closed within handshake_timeout_ms (300 ms)");
        assert!(gw.agent.system_stats().gateway_connections_timed_out >= 1, "the handshake timeout is counted");
        gw.stop().await;
    }

    #[tokio::test]
    async fn a_socket_silent_after_the_tls_handshake_is_closed() {
        let gw = TlsGateway::start(300, 1024).await;
        let mut s = gw.handshake().await;
        assert!(closed_within(&mut s, Duration::from_secs(3)).await,
            "a completed handshake followed by no request must not hold the connection");
        gw.stop().await;
    }

    #[tokio::test]
    async fn n_plus_one_silent_sockets_do_not_block_a_real_request_on_the_tls_gateway() {
        let gw = TlsGateway::start(300, 2).await;
        let mut _silent = Vec::new();
        for _ in 0..3 { _silent.push(TcpStream::connect(("127.0.0.1", gw.port)).await.unwrap()); }
        let answered = gw.health_within(Duration::from_secs(5)).await;
        assert!(answered.as_deref().is_some_and(|l| l.starts_with("HTTP/1.1 200")),
            "three silent sockets against a cap of two expire and admit a real request: {answered:?}");
        gw.stop().await;
    }

    #[tokio::test]
    async fn the_tls_gateway_serves_no_more_than_gateway_max_connections_at_once() {
        let gw = TlsGateway::start(30_000, 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let silent_a = TcpStream::connect(("127.0.0.1", gw.port)).await.unwrap();
        let _silent_b = TcpStream::connect(("127.0.0.1", gw.port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(gw.health_within(Duration::from_millis(800)).await, None,
            "with gateway_max_connections = 2 held in their handshakes, a third connection is not served");
        drop(silent_a);
        let answered = gw.health_within(Duration::from_secs(3)).await;
        assert!(answered.as_deref().is_some_and(|l| l.starts_with("HTTP/1.1 200")),
            "a returned permit admits the waiting connection: {answered:?}");
        gw.stop().await;
    }

    #[tokio::test]
    async fn shutdown_closes_a_connection_still_in_its_tls_handshake() {
        let gw = TlsGateway::start(30_000, 1024).await;
        let mut silent = TcpStream::connect(("127.0.0.1", gw.port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let a = Arc::clone(&gw.agent);
        let stop = tokio::spawn(async move { a.shutdown().await });
        assert!(closed_within(&mut silent, Duration::from_secs(3)).await,
            "shutdown releases a connection waiting in its TLS handshake");
        stop.await.unwrap();
        let _ = std::fs::remove_dir_all(&gw.cert_dir);
    }
}
