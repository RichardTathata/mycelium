//! Adopter-surface Y3 (D28): a browser on another origin reads a gateway only where the operator named
//! its origin, only on the read-only routes, and only as far as the bearer admits.

use super::CORS_READ_PATHS;
use crate::test_util::alloc_port;
use crate::{GossipAgent, GossipConfig, NodeId};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, ORIGIN, VARY,
};
use std::sync::Arc;
use std::time::Duration;

const CONSOLE: &str = "http://127.0.0.1:8099";
const OTHER: &str = "http://evil.example";
const TOKEN: &str = "secret";

/// The read paths this test node mounts: `/gateway/audit` needs `compliance` and `/metrics` answers
/// only in a `metrics` build, `/gateway/tuple/overview` is the tuple-space companion's, so a node of
/// this build answers each of the rest.
fn mounted_read_paths() -> impl Iterator<Item = &'static str> {
    CORS_READ_PATHS.iter().copied().filter(|p| {
        !(*p == "/gateway/tuple/overview"
            || (*p == "/gateway/audit" && !cfg!(feature = "compliance"))
            || (*p == "/metrics" && !cfg!(feature = "metrics")))
    })
}

async fn node(origins: &[&str]) -> (Arc<GossipAgent>, String) {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some(TOKEN.into());
    cfg.gateway_cors_origins = origins.iter().map(|o| o.to_string()).collect();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    assert!(agent.kv().set("ui/probe", "x".to_string()), "seed the key `/gateway/kv` reads");
    assert!(agent.kv().set("app/secret", "x".to_string()), "and one no page may read");
    tokio::time::sleep(Duration::from_millis(50)).await;
    (agent, format!("http://127.0.0.1:{http_port}"))
}

fn read_url(base: &str, path: &str) -> String {
    // `/gateway/kv` reads one key; the others take no parameters.
    if path == "/gateway/kv" { format!("{base}{path}?key=ui/probe") } else { format!("{base}{path}") }
}

async fn get(c: &reqwest::Client, url: &str, origin: Option<&str>, bearer: bool) -> reqwest::Response {
    let mut r = c.get(url);
    if let Some(o) = origin {
        r = r.header(ORIGIN, o);
    }
    if bearer {
        r = r.header(AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    r.send().await.unwrap()
}

async fn preflight(c: &reqwest::Client, url: &str, origin: &str, method: &str) -> reqwest::Response {
    c.request(reqwest::Method::OPTIONS, url)
        .header(ORIGIN, origin)
        .header(ACCESS_CONTROL_REQUEST_METHOD, method)
        .header(ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
        .send()
        .await
        .unwrap()
}

fn acao(r: &reqwest::Response) -> Option<String> {
    r.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).map(|v| v.to_str().unwrap().to_string())
}

/// Unset — the default — no answer on any route carries `Access-Control-Allow-Origin`, and no preflight
/// is answered, whatever the origin: a page on another origin reads nothing.
#[tokio::test]
async fn with_cors_unset_no_answer_carries_an_allow_origin() {
    let (agent, base) = node(&[]).await;
    let c = reqwest::Client::new();
    for path in mounted_read_paths() {
        let url = read_url(&base, path);
        let r = get(&c, &url, Some(CONSOLE), true).await;
        assert!(r.status().is_success(), "{path} answers the bearer: {}", r.status());
        assert_eq!(acao(&r), None, "{path}: no cross-origin read without the setting");
        let p = preflight(&c, &url, CONSOLE, "GET").await;
        assert_ne!(p.status(), 204, "{path}: no preflight answered");
        assert_eq!(acao(&p), None, "{path}: preflight");
    }
    agent.shutdown().await;
}

/// Set — a listed origin's `GET` on a read path carries its own origin back (never `*`) and
/// `Vary: Origin`; its preflight for `GET` is answered with no credential and admits `Authorization`.
/// Nothing else is answered: an unlisted origin, a lookalike origin, a preflight for another method, a
/// write route, a read route not on the list, or a write on a listed path.
#[tokio::test]
async fn with_cors_set_only_listed_origins_read_only_the_read_paths() {
    let (agent, base) = node(&[CONSOLE, "https://ops.example"]).await;
    let c = reqwest::Client::new();
    for path in mounted_read_paths() {
        let url = read_url(&base, path);
        let r = get(&c, &url, Some(CONSOLE), true).await;
        assert!(r.status().is_success(), "{path}: {}", r.status());
        assert_eq!(acao(&r).as_deref(), Some(CONSOLE), "{path}: the listed origin, echoed");
        let h = c.head(&url).header(ORIGIN, CONSOLE).header(AUTHORIZATION, format!("Bearer {TOKEN}")).send().await.unwrap();
        assert_eq!(acao(&h).as_deref(), Some(CONSOLE), "{path}: HEAD is a read too");
        assert!(r.headers().get_all(VARY).iter().any(|v| v.to_str().unwrap().contains("Origin")), "{path}: Vary");
        let p = preflight(&c, &url, CONSOLE, "GET").await;
        assert_eq!(p.status(), 204, "{path}: preflight answered with no credential");
        assert_eq!(acao(&p).as_deref(), Some(CONSOLE), "{path}: preflight origin");
        assert_eq!(p.headers()[ACCESS_CONTROL_ALLOW_METHODS], "GET, HEAD", "{path}");
        assert_eq!(p.headers()[ACCESS_CONTROL_ALLOW_HEADERS], "authorization", "{path}");
        for origin in [OTHER, "http://127.0.0.1:8099/", "http://127.0.0.1:80999", "null", "http://127.0.0.1"] {
            assert_eq!(acao(&get(&c, &url, Some(origin), true).await), None, "{path} from {origin}");
            let p = preflight(&c, &url, origin, "GET").await;
            assert_ne!(p.status(), 204, "{path}: {origin}'s preflight");
            assert_eq!(acao(&p), None, "{path}: {origin}'s preflight");
        }
        for method in ["POST", "DELETE", "PUT", "PATCH"] {
            let p = preflight(&c, &url, CONSOLE, method).await;
            assert_ne!(p.status(), 204, "{path}: a {method} preflight");
            assert_eq!(acao(&p), None, "{path}: a {method} preflight");
        }
    }
    // The bearer still decides: a listed origin with no credential reads a 401 (and may see that it did).
    let r = get(&c, &format!("{base}/gateway/fleet"), Some(CONSOLE), false).await;
    assert_eq!(r.status(), 401, "the setting never stands in for the bearer");
    // A write on a listed path, from a listed origin with the bearer: it is the gateway's to decide, and
    // its answer carries no CORS header, so a page cannot read what a write returned.
    let w = c
        .post(format!("{base}/gateway/kv"))
        .header(ORIGIN, CONSOLE)
        .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
        .json(&serde_json::json!({"key": "app/probe", "value_b64": "eA=="}))
        .send()
        .await
        .unwrap();
    assert_eq!(acao(&w), None, "a write's answer is never readable cross-origin ({})", w.status());
    // Routes not on the list: writes, calls, streams' neighbours and governance — read or not.
    for (method, path) in [
        ("POST", "/gateway/kv/quorum"),
        ("POST", "/gateway/rpc/call"),
        ("POST", "/gateway/govern/profile"),
        ("POST", "/mcp"),
        ("GET", "/gateway/demand"),
        ("GET", "/gateway/capability/resolve"),
        ("GET", "/gateway/explain"),
        ("GET", "/gateway/fleet/"),
        ("GET", "/gateway/fleetx"),
    ] {
        let url = format!("{base}{path}");
        let p = preflight(&c, &url, CONSOLE, method).await;
        assert_ne!(p.status(), 204, "{method} {path}: preflight");
        assert_eq!(acao(&p), None, "{method} {path}: preflight");
        if method == "GET" {
            assert_eq!(acao(&get(&c, &url, Some(CONSOLE), true).await), None, "GET {path}");
        }
    }
    agent.shutdown().await;
}

const CONSOLE_PAGE: &str = include_str!("../../../examples/ops_console/index.html");
const CONSOLE_README: &str = include_str!("../../../examples/ops_console/README.md");

/// The page's `PANELS` list: each tab and its routes, as the page declares them.
/// Parsed by hand for the one shape the page writes — `{tab:"X", routes:["/a", "/b"]},` — so the test needs
/// no regex dependency; a line that starts `{tab:` and does not parse fails the test.
fn page_panels() -> Vec<(String, Vec<String>)> {
    let start = CONSOLE_PAGE.find("const PANELS = [").expect("the page declares PANELS");
    let end = start + CONSOLE_PAGE[start..].find("];").expect("PANELS ends");
    CONSOLE_PAGE[start..end]
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("{tab:"))
        .map(|l| {
            let tab = l.split('"').nth(1).expect("a tab name").to_string();
            let routes = l[l.find("routes:[").expect("routes") + 8..l.rfind(']').expect("]")]
                .split(',')
                .map(|r| r.trim().trim_matches('"').to_string())
                .collect();
            (tab, routes)
        })
        .collect()
}

/// The README's `| Tab | Route(s) | Shows |` rows.
fn readme_panels() -> Vec<(String, Vec<String>)> {
    CONSOLE_README
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Tab ") && l.matches('|').count() >= 4)
        .map(|l| {
            let cols: Vec<&str> = l.split('|').map(str::trim).collect();
            let routes = cols[2].split('`').skip(1).step_by(2).map(String::from).collect();
            (cols[1].to_string(), routes)
        })
        .collect()
}

/// Y3's exit: each panel renders from a route its README names, and every route the page reads is on the
/// gateway's cross-origin read list — so the page can read nothing a gateway does not answer across
/// origins, and the README cannot drift from what the page does.
#[test]
fn the_console_reads_only_the_routes_its_readme_names_and_each_is_a_cross_origin_read() {
    let page = page_panels();
    assert!(page.len() >= 10, "the page's tabs: {page:?}");
    assert_eq!(page, readme_panels(), "README.md's table is the page's PANELS list");
    let link_routes = ["/gateway/kv?key=ui%2Fviz", "/gateway/kv?key=ui%2Flabel"];
    assert!(CONSOLE_PAGE.contains(r#"const LINK_ROUTES = ["/gateway/kv?key=ui%2Fviz", "/gateway/kv?key=ui%2Flabel"];"#));
    for route in page.iter().flat_map(|(_, r)| r.iter().map(String::as_str)).chain(link_routes) {
        let path = route.split('?').next().unwrap();
        assert!(CORS_READ_PATHS.contains(&path), "{route}: the page reads a path no gateway answers cross-origin");
    }
}

/// Y3's exit: the console opens no listener of its own. It is a static page with no binary — no Rust
/// source beside it, no `[[example]]` named for it — and the page makes one kind of request, a `GET` to a
/// node's own route built from the node the operator listed: no relative fetch back to wherever it is
/// served from (the old `/api?host=…&path=…` proxy), no socket, stream or beacon.
#[test]
fn the_console_opens_no_listener_of_its_own() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ops_console");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["README.md", "index.html"], "a static page and its README, nothing that runs");
    let manifest = include_str!("../../../Cargo.toml");
    assert!(
        !manifest.lines().any(|l| l.trim_start().starts_with("path") && l.contains("examples/ops_console/")),
        "no binary is built from the console's directory"
    );
    assert!(!manifest.contains("name              = \"ops_console\""), "no example is named for the console");
    assert_eq!(CONSOLE_PAGE.matches("fetch(").count(), 1, "one network call");
    assert!(CONSOLE_PAGE.contains("const base = nodeBase(node);") && CONSOLE_PAGE.contains("r = await fetch(base+route, {headers});"),
        "to a listed node, by its base");
    assert!(CONSOLE_PAGE.contains("u.pathname!==\"/\" || u.search || u.hash) return null;") && CONSOLE_PAGE.contains("return u.origin;"),
        "a node's base is the origin of an entry with no path, query or fragment: absolute, never relative to the page");
    for absent in ["/api?", "XMLHttpRequest", "WebSocket", "EventSource", "sendBeacon", "method:", "__DEFAULT_TARGET__"] {
        assert!(!CONSOLE_PAGE.contains(absent), "the page uses {absent}");
    }
}

/// The KV value route is a cross-origin read for one key under `ui/` only — the `ui/viz` link — never another
/// key's value, whatever the encoding, and never when a second `key` could be the one the handler reads.
#[tokio::test]
async fn a_listed_origin_reads_only_ui_keys_through_the_kv_value_route() {
    let (agent, base) = node(&[CONSOLE]).await;
    let c = reqwest::Client::new();
    for q in ["key=ui/probe", "key=ui%2Fprobe", "key=ui/viz"] {
        let url = format!("{base}/gateway/kv?{q}");
        assert_eq!(acao(&get(&c, &url, Some(CONSOLE), true).await).as_deref(), Some(CONSOLE), "{q}");
        assert_eq!(preflight(&c, &url, CONSOLE, "GET").await.status(), 204, "{q}: preflight");
    }
    for q in [
        "key=app/secret", "key=sys/identity/x", "key=grp/workers/x", "key=", "", "key=ui", "key=%55i/probe", "key=+ui/probe",
        "key=ui/probe&key=app/secret", "key=app/secret&key=ui/probe", "k=ui/probe",
    ] {
        let url = format!("{base}/gateway/kv?{q}");
        let r = get(&c, &url, Some(CONSOLE), true).await;
        assert_eq!(acao(&r), None, "{q:?}: no cross-origin read of a value outside ui/");
        let p = preflight(&c, &url, CONSOLE, "GET").await;
        assert_ne!(p.status(), 204, "{q:?}: preflight");
        assert_eq!(acao(&p), None, "{q:?}: preflight");
    }
    agent.shutdown().await;
}

/// A companion's router merged with `with_http_routes` sits inside the layer: its read route on the list is
/// answered, a route of its own off the list is not.
#[tokio::test]
async fn a_merged_companion_route_is_answered_only_where_listed() {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some(TOKEN.into());
    cfg.gateway_cors_origins = vec![CONSOLE.into()];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    async fn ok() -> &'static str { "{}" }
    agent.with_http_routes(
        axum::Router::new()
            .route("/gateway/tuple/overview", axum::routing::get(ok))
            .route("/gateway/tuple/depth", axum::routing::get(ok)),
    );
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://127.0.0.1:{http_port}");
    let c = reqwest::Client::new();
    let r = get(&c, &format!("{base}/gateway/tuple/overview"), Some(CONSOLE), true).await;
    assert_eq!(r.status(), 200);
    assert_eq!(acao(&r).as_deref(), Some(CONSOLE), "the companion's listed read");
    let r = get(&c, &format!("{base}/gateway/tuple/depth"), Some(CONSOLE), true).await;
    assert_eq!(r.status(), 200);
    assert_eq!(acao(&r), None, "a companion route off the list");
    let r = get(&c, &format!("{base}/gateway/tuple/overview"), Some(CONSOLE), false).await;
    assert_eq!(r.status(), 401, "and the bearer still applies to a merged route");
    agent.shutdown().await;
}

/// A gateway with no credential model lists only pages on this host: a script on any other origin, run by an
/// operator's browser, would read whatever the gateway answers anyone. Refused at `start()` by name; a
/// credential, or a loopback origin, starts.
#[tokio::test]
async fn an_open_gateway_refuses_an_origin_that_is_not_on_this_host() {
    async fn start(origins: Vec<&str>, token: Option<&str>) -> Result<(), crate::GossipError> {
        let gossip_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(alloc_port());
        cfg.gateway_auth_token = token.map(String::from);
        cfg.gateway_cors_origins = origins.into_iter().map(String::from).collect();
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        let r = agent.start().await;
        if r.is_ok() {
            agent.shutdown().await;
        }
        r
    }
    for o in ["https://ops.example", "http://10.0.0.5:8099", "http://localhost.example"] {
        let err = start(vec![CONSOLE, o], None).await.expect_err("an open gateway and a remote page");
        assert!(matches!(&err, crate::GossipError::InvalidField { field: "gateway_cors_origins", reason } if reason.contains(o)), "{err}");
        start(vec![CONSOLE, o], Some(TOKEN)).await.expect("with a credential it starts");
    }
    start(vec![CONSOLE, "http://localhost:8099", "http://[::1]:8099"], None).await.expect("pages on this host");
}

/// The HTTPS listener (`gateway_tls`) serves the same router, so the layer answers there too.
#[cfg(feature = "tls")]
#[tokio::test]
async fn the_https_gateway_answers_a_listed_origin_the_same_way() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("gw-cors-tls-{}-{http_port}", std::process::id()));
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.gateway_tls = Some(crate::GatewayTlsConfig::default());
    cfg.gateway_cors_origins = vec![CONSOLE.into()];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let ca_pem = std::fs::read(cert_dir.join("ca-cert.pem")).expect("ca-cert.pem written");
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_pem)) {
        roots.add(cert.unwrap()).unwrap();
    }
    let connector = tokio_rustls::TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth(),
    ));
    let ask = |path: &'static str, origin: &'static str| {
        let connector = connector.clone();
        async move {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
            let name = rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::new(127, 0, 0, 1).into());
            let mut tls = connector.connect(name, tcp).await.unwrap();
            tls.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: {origin}\r\nConnection: close\r\n\r\n").as_bytes())
                .await.unwrap();
            let mut buf = Vec::new();
            tls.read_to_end(&mut buf).await.unwrap();
            String::from_utf8_lossy(&buf).to_ascii_lowercase()
        }
    };
    let listed = ask("/stats", CONSOLE).await;
    assert!(listed.contains("200 ok") && listed.contains(&format!("access-control-allow-origin: {CONSOLE}")), "{listed}");
    let other = ask("/stats", OTHER).await;
    assert!(other.contains("200 ok") && !other.contains("access-control-allow-origin"), "{other}");
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
