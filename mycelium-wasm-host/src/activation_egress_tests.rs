//! The egress proxy on its own (post-360 rows L and M), driven by an HTTP client rather than a confined
//! command — so these run on every platform, with or without a confinement backend. The confined, end-to-end
//! cases are in `activation.rs`'s tests (live under `MYCELIUM_CONFINE_LIVE_REQUIRED`).

use super::*;
use std::sync::Mutex;

/// A loopback upstream that records every request head and answers `200 ok`.
async fn upstream() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply: &[u8] = if head.starts_with("GET /redirect") {
                    b"HTTP/1.1 302 Found\r\nLocation: http://elsewhere.example/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                log.lock().unwrap().push(head);
                let _ = sock.write_all(reply).await;
            });
        }
    });
    (port, seen)
}

fn start(hosts: Vec<String>, credentials: Vec<String>, node: &EgressPolicy) -> Result<EgressProxy, String> {
    let decl = EgressDecl { hosts, credentials };
    EgressProxy::start(ProxyOptions {
        label: "data/pack".into(),
        decl: &decl,
        node_egress: node,
        bind: EgressProxy::DEFAULT_BIND,
        limits: Default::default(),
        peer_uid: None,
        fd_budget: None,
        #[cfg(any(feature = "gateway", feature = "llm"))]
        gate: None,
    })
}

/// The proxy URL a command's environment carries (token included).
fn proxy_url(p: &EgressProxy) -> String {
    p.command_env(&[]).into_iter().find(|(k, _)| k == "HTTP_PROXY").expect("HTTP_PROXY is set").1
}

fn client_via(url: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(url).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// What any client of the proxy meets, whatever its platform: the token is required (a local process
/// without it gets 407, never a forwarded request); a listed host is reached; an unlisted one is refused
/// with the proxy's own marker and the upstream sees nothing; a request that is not absolute-form is
/// refused rather than guessed at; a redirect comes back to the client, not followed. Seen failing first
/// with the token check removed: the token-less request was forwarded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_proxy_requires_its_token_and_limits_by_host() {
    let ((a, at_a), (b, at_b)) = (upstream().await, upstream().await);
    let p = start(vec![format!("http://127.0.0.1:{a}")], Vec::new(), &EgressPolicy::default()).expect("starts");
    let url = proxy_url(&p);
    assert!(url.starts_with("http://mycelium:") && url.ends_with(&format!("@127.0.0.1:{}", p.port())), "{url}");

    // No token: 407, nothing forwarded.
    let bare = client_via(&format!("http://127.0.0.1:{}", p.port()));
    let r = bare.get(format!("http://127.0.0.1:{a}/no-token")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 407);
    assert!(at_a.lock().unwrap().is_empty(), "a request without the token is not forwarded");

    let c = client_via(&url);
    let r = c.get(format!("http://127.0.0.1:{a}/yes")).send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.text().await.unwrap().as_str()), (200, "ok"));
    let head = at_a.lock().unwrap()[0].to_ascii_lowercase();
    assert!(head.starts_with("get /yes") && !head.contains("proxy-authorization"), "the token stays at the proxy: {head}");

    let r = c.get(format!("http://127.0.0.1:{b}/no")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    assert_eq!(r.headers().get("x-mycelium-egress").map(|v| v.to_str().unwrap()), Some("refused"));
    assert!(r.text().await.unwrap().contains("not in this activation's egress hosts"));
    assert!(at_b.lock().unwrap().is_empty());

    // A redirect is the client's to follow — through the proxy, where it meets the list again.
    let r = c.get(format!("http://127.0.0.1:{a}/redirect")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 302);

    // Origin-form (a client given the proxy as a base URL): refused, not guessed at.
    let token_header = {
        let u = reqwest::Url::parse(&url).unwrap();
        format!("Basic {}", base64_lite::encode(format!("{}:{}", u.username(), u.password().unwrap()).as_bytes()))
    };
    let r = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/v1/chat", p.port()))
        .header("proxy-authorization", token_header)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    assert!(r.text().await.unwrap().contains("absolute-form"));
    let c = p.counts();
    assert_eq!((c.forwarded, c.tunnelled), (2, 0), "{c:?}");
    assert!(c.refused >= 3, "{c:?}");
}

/// A placeholder naming a binding this activation was not given is refused at the proxy, before the gate
/// — and with no gate at all (no binding on the node, or a build without one) a placeholder is refused, never
/// sent as written. The upstream sees nothing either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_placeholder_the_activation_may_not_name_is_refused_and_never_sent() {
    let (a, at_a) = upstream().await;
    let p = start(vec![format!("http://127.0.0.1:{a}")], vec!["model".into()], &EgressPolicy::default()).expect("starts");
    let c = client_via(&proxy_url(&p));
    for name in ["other", "model"] {
        let r = c
            .get(format!("http://127.0.0.1:{a}/v1"))
            .header("authorization", format!("Bearer {}", mycelium::credential_placeholder(name)))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 403, "{name}");
        let why = r.text().await.unwrap();
        if name == "other" {
            assert!(why.contains("credential `other` is not one this activation's egress may name"), "{why}");
        } else {
            assert!(why.contains("no egress credential binding") || why.contains("attaches none"), "no gate here: {why}");
        }
    }
    assert!(at_a.lock().unwrap().is_empty(), "nothing was sent: {:?}", at_a.lock().unwrap());
}

/// The list is checked before the proxy starts and before a live change: an entry the node's own egress
/// allow-list denies is refused by name (the old list standing), and the proxy listens on loopback only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_the_node_denies_is_refused_and_the_proxy_binds_loopback_only() {
    let node = EgressPolicy { allow_hosts: vec!["api.example.com".into()] };
    let e = start(vec!["https://other.example".into()], Vec::new(), &node).expect_err("denied by the node");
    assert!(e.contains("data/pack") && e.contains("allow-list does not permit other.example"), "{e}");
    let p = start(vec!["https://api.example.com".into()], Vec::new(), &node).expect("permitted");
    let e = p.set_hosts(&["https://other.example".into()]).expect_err("denied live too");
    assert!(e.contains("other.example"), "{e}");
    assert_eq!(p.hosts(), ["https://api.example.com:443"]);

    let decl = EgressDecl::default();
    let e = EgressProxy::start(ProxyOptions {
        label: "data/pack".into(),
        decl: &decl,
        node_egress: &EgressPolicy::default(),
        bind: "0.0.0.0:0".parse().unwrap(),
        limits: Default::default(),
        peer_uid: None,
        fd_budget: None,
        #[cfg(any(feature = "gateway", feature = "llm"))]
        gate: None,
    })
    .expect_err("a non-loopback bind is refused");
    assert!(e.contains("not a loopback address"), "{e}");
}

/// The command's environment: cleared but for the base set and the declared names, the proxy URL in every
/// proxy variable, and no `NO_PROXY` to route around it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_environment_is_cleared_and_names_the_proxy() {
    let p = start(Vec::new(), Vec::new(), &EgressPolicy::default()).expect("starts");
    let env = p.command_env(&["PATH".into(), "MYCELIUM_TEST_SURELY_UNSET_7F".into()]);
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    for k in mycelium::ACTIVATION_EGRESS_PROXY_ENV {
        assert!(keys.contains(k), "{k} is set: {keys:?}");
    }
    assert!(keys.iter().all(|k| mycelium::ACTIVATION_EGRESS_BASE_ENV.contains(k) || mycelium::ACTIVATION_EGRESS_PROXY_ENV.contains(k)), "{keys:?}");
    assert_eq!(keys.iter().filter(|k| **k == "PATH").count(), 1, "PATH once: {keys:?}");
    assert!(!keys.iter().any(|k| k.eq_ignore_ascii_case("NO_PROXY")));
    assert!(!format!("{p:?}").contains(&proxy_url(&p)[15..]), "the token is not in the proxy's Debug");
}

/// The guarantee says which applies (rows L and M): not applicable with no activation; not configured for
/// an unconfined one or a `*:<port>` entry; on this platform's own backend, enforced (Seatbelt) or not
/// configured, naming Landlock's port-only limit. Planted: with the check answering `enforced`, the first
/// `not_configured` assertion fails.
#[test]
fn the_egress_guarantee_says_which_applies() {
    let state = |activations: &str| -> (String, String) {
        let units = mycelium::NodeCapabilityConfig::from_toml_str(&format!("[hosts]\nkinds = [\"blob\"]\naccept_unsigned = true\n{activations}")).unwrap();
        let id = mycelium::NodeId::new("127.0.0.1", mycelium::test_util::alloc_port()).unwrap();
        let agent = mycelium::GossipAgent::new(id, mycelium::GossipConfig::default());
        agent.register_guarantee(egress_guarantee(&units)).expect("registered before start");
        let r = agent.guarantee_report();
        let e = r.entry("stem.activation_egress").expect("the report carries it");
        (e.resolution.state().to_string(), format!("{:?}", e.resolution))
    };
    let act = |confine: &str| format!("[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = [\"true\"]\n{confine}");
    assert_eq!(state("").0, "not_applicable");
    let (st, why) = state(&act(""));
    assert_eq!(st, "not_configured");
    assert!(why.contains("unconfined"), "{why}");
    if crate::activation_sandbox::native_backend_or_skip("the_egress_guarantee_says_which_applies") {
        let (st, why) = state(&act("[activation.confine]\nconnect = [\"*:443\"]\n"));
        assert_eq!(st, "not_configured");
        assert!(why.contains("*:<port>"), "{why}");
        let (st, why) = state(&act("[activation.confine]\n[activation.confine.egress]\nhosts = [\"https://api.example.com\"]\n"));
        if cfg!(target_os = "macos") {
            assert_eq!(st, "enforced", "{why}");
        } else {
            assert_eq!(st, "not_configured");
            assert!(why.contains("Landlock"), "{why}");
        }
        // #644's review, finding 1: this holds because every confined command now starts with a cleared
        // environment (`confined_env`), so no binding's `value_env` is in it — shown live by
        // `a_confined_activation_starts_with_a_cleared_environment`.
        assert_eq!(state(&act("[activation.confine]\n")).0, "enforced", "no network at all holds on either backend");
        // #644's review, finding 4: a local service the command reaches directly may forward it anywhere.
        // Seen failing first: `enforced` on macOS.
        let (st, why) = state(&act("[activation.confine]\nconnect = [\"localhost:11434\"]\n[activation.confine.egress]\nhosts = [\"https://api.example.com\"]\n"));
        assert_eq!(st, "not_configured", "{why}");
        if cfg!(target_os = "macos") {
            assert!(why.contains("localhost:<port>"), "{why}");
        }
    }
}

/// Raw HTTP to the proxy: `head` sent as is (the token header added), then the whole answer read.
async fn raw(port: u16, token: &str, head: &str) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let close = if head.to_ascii_lowercase().contains("connection: close") { "" } else { "Connection: close\r\n" };
    let head = head.replacen("\r\n", &format!("\r\nProxy-Authorization: {token}\r\n{close}"), 1);
    s.write_all(head.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).to_string()
}

fn token_header(p: &EgressProxy) -> String {
    let u = reqwest::Url::parse(&proxy_url(p)).unwrap();
    format!("Basic {}", base64_lite::encode(format!("{}:{}", u.username(), u.password().unwrap()).as_bytes()))
}

fn start_limited(hosts: Vec<String>, limits: ProxyLimits) -> EgressProxy {
    start_with_peer_uid(hosts, limits, None)
}

fn start_with_peer_uid(hosts: Vec<String>, limits: ProxyLimits, peer_uid: Option<PeerUidLookup>) -> EgressProxy {
    start_with_budget(hosts, limits, peer_uid, FdBudget::new(10_000))
}

fn start_with_budget(hosts: Vec<String>, limits: ProxyLimits, peer_uid: Option<PeerUidLookup>, budget: Arc<FdBudget>) -> EgressProxy {
    let decl = EgressDecl { hosts, ..Default::default() };
    EgressProxy::start(ProxyOptions {
        label: "data/pack".into(),
        decl: &decl,
        node_egress: &EgressPolicy::default(),
        bind: EgressProxy::DEFAULT_BIND,
        #[cfg(any(feature = "gateway", feature = "llm"))]
        gate: None,
        limits,
        peer_uid,
        // A budget of its own, so a test's floods are not judged against this process's descriptor limit.
        fd_budget: Some(budget),
    })
    .unwrap()
}

/// #644's round-3 review: a connection whose socket another user owns is closed at accept, before it holds any
/// place — and so is one whose owner cannot be read (fail closed). Through the injectable lookup, so it runs on
/// every platform and needs no second user; `another_users_connection_is_closed_at_accept` (live, under
/// `MYCELIUM_EGRESS_OTHER_UID_LIVE_REQUIRED`) shows the kernel's lookup doing it for a real other user. Seen
/// failing first: before peer-uid admission both were served (the token check answered 200).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_another_user_owns_or_no_one_can_name_is_closed_at_accept() {
    let (u, seen) = upstream().await;
    let me = crate::peer_uid::own_uid();
    for (label, lookup) in [
        ("another user", Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| pairs.iter().map(|_| Ok(me.wrapping_add(1))).collect()) as PeerUidLookup),
        ("unknown", Arc::new(|pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| pairs.iter().map(|_| Err("no answer".to_string())).collect()) as PeerUidLookup),
    ] {
        let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], ProxyLimits::default(), Some(lookup));
        let answer = raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await;
        assert!(answer.is_empty(), "{label}: closed with no answer: {answer}");
        assert!(p.counts().refused >= 1, "{label}: counted");
    }
    assert!(seen.lock().unwrap().is_empty(), "nothing was forwarded");
    // The plant: the same request from this user, through the kernel's own lookup, is served.
    let p = start_limited(vec![format!("http://127.0.0.1:{u}")], ProxyLimits::default());
    let answer = raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

/// #644's round-4 review, finding 1: a flood of another user's connections neither queues without bound nor
/// multiplies the kernel reads, and the command still gets in. 100 connections arrive while each read takes
/// 150 ms (a stub that names every flood connection another user's): at most `max_unchecked` (64) ever wait —
/// the rest in the kernel's backlog since round 5 — reads grow per batch, not per connection, and the command's
/// own connection, made after the flood, is served within a bound. Seen failing first on round 3's
/// per-connection checks: every connection waited and paid its own read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_of_other_users_connections_is_bounded_and_batched() {
    let (u, _) = upstream().await;
    let me = crate::peer_uid::own_uid();
    let mine: Arc<Mutex<std::collections::HashSet<u16>>> = Arc::default();
    let mine_seen = Arc::clone(&mine);
    let lookup: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let mine = mine_seen.lock().unwrap();
        pairs.iter().map(|(_, peer)| Ok(if mine.contains(&peer.port()) { me } else { me.wrapping_add(1) })).collect()
    });
    let limits = ProxyLimits { max_unchecked: 64, preauth_timeout_ms: 5_000, ..ProxyLimits::default() };
    let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], limits, Some(lookup));
    let mut flood = Vec::new();
    for _ in 0..100 {
        flood.push(tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap());
    }
    // The command's connection, after the flood: registered as this user's, then a request on it.
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut cmd = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    mine.lock().unwrap().insert(cmd.local_addr().unwrap().port());
    let started = std::time::Instant::now();
    cmd.write_all(format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n", token_header(&p)).as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.read_to_end(&mut answer)).await;
    let answer = String::from_utf8_lossy(&answer).to_string();
    assert!(answer.starts_with("HTTP/1.1 200"), "the command is served: {answer:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "within a few reads: {:?}", started.elapsed());
    let c = p.counts();
    assert!(c.waiting_max <= 64, "at most max_unchecked wait: {c:?}");
    assert!(c.peer_lookups < 20, "reads per batch, not per connection (101 connections): {c:?}");
    drop(flood);
}

/// #644's round-5 review: the command's connection is never closed to make room — even when it is the **oldest**
/// waiter and more than `max_unchecked` other-user connections arrive within one read. The proxy stops accepting
/// while `max_unchecked` wait, so the rest stay in the kernel's first-in, first-out listen backlog; each read
/// classifies every waiter. Seen failing first on round 4's oldest-first eviction: the command was closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_oldest_waiter_is_never_closed_to_make_room() {
    let (u, _) = upstream().await;
    let me = crate::peer_uid::own_uid();
    let mine: Arc<Mutex<std::collections::HashSet<u16>>> = Arc::default();
    let mine_seen = Arc::clone(&mine);
    let lookup: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mine = mine_seen.lock().unwrap();
        pairs.iter().map(|(_, peer)| Ok(if mine.contains(&peer.port()) { me } else { me.wrapping_add(1) })).collect()
    });
    let limits = ProxyLimits { max_unchecked: 16, preauth_timeout_ms: 10_000, ..ProxyLimits::default() };
    let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], limits, Some(lookup));
    // An idle connection first, so the next read is in flight when the command and the flood arrive.
    let _first = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut cmd = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    mine.lock().unwrap().insert(cmd.local_addr().unwrap().port());
    let mut flood = Vec::new();
    for _ in 0..80 {
        flood.push(tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap());
    }
    cmd.write_all(format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n", token_header(&p)).as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(8), cmd.read_to_end(&mut answer)).await;
    let answer = String::from_utf8_lossy(&answer).to_string();
    assert!(answer.starts_with("HTTP/1.1 200"), "the oldest waiter, the command, is served: {answer:?} {:?}", p.counts());
    assert!(p.counts().waiting_max <= 16, "{:?}", p.counts());
    drop(flood);
}

/// #644's round-5 review, finding 3: a read of the kernel's table that hangs is bounded — that batch is refused
/// (`peer_uid_unknown`) and, once the read returns, the next batch is served (round 6: while it has not, later
/// batches are refused rather than given a read each). Seen failing first: the hung read stalled every later
/// connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_table_read_is_bounded_and_the_next_batch_is_served() {
    let (u, _) = upstream().await;
    let me = crate::peer_uid::own_uid();
    let calls = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&calls);
    let lookup: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1_500));
        }
        pairs.iter().map(|_| Ok(me)).collect()
    });
    let limits = ProxyLimits { peer_read_timeout_ms: 500, ..ProxyLimits::default() };
    let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], limits, Some(lookup));
    let (token, get) = (token_header(&p), format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n"));
    let first = tokio::time::timeout(std::time::Duration::from_secs(8), raw(p.port(), &token, &get)).await.expect("the hung read is bounded");
    assert!(first.is_empty(), "the hung batch is refused: {first}");
    let mut served = String::new();
    for _ in 0..20 {
        served = tokio::time::timeout(std::time::Duration::from_secs(8), raw(p.port(), &token, &get)).await.expect("a later batch is answered");
        if served.starts_with("HTTP/1.1 200") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(served.starts_with("HTTP/1.1 200"), "once the read returns, the next batch is served: {served}");
}

/// #644's round-7 review: a read abandoned past its bound that then **panics** still clears the one-read flag, so a
/// later batch is served. Seen failing first with the flag cleared only on a normal return: every later batch was
/// refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_that_panics_after_its_bound_does_not_stop_later_reads() {
    let (u, _) = upstream().await;
    let me = crate::peer_uid::own_uid();
    let calls = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&calls);
    let lookup: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(800));
            panic!("a read that outlived its bound, then failed");
        }
        pairs.iter().map(|_| Ok(me)).collect()
    });
    let limits = ProxyLimits { peer_read_timeout_ms: 300, ..ProxyLimits::default() };
    let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], limits, Some(lookup));
    let (token, get) = (token_header(&p), format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n"));
    let first = tokio::time::timeout(std::time::Duration::from_secs(8), raw(p.port(), &token, &get)).await.expect("bounded");
    assert!(first.is_empty(), "the abandoned batch is refused: {first}");
    // Once the abandoned read has panicked, a later batch is served (polled: while it is still out, refused).
    let mut later = String::new();
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        later = tokio::time::timeout(std::time::Duration::from_secs(8), raw(p.port(), &token, &get)).await.expect("answered");
        if later.starts_with("HTTP/1.1 200") {
            break;
        }
    }
    assert!(later.starts_with("HTTP/1.1 200"), "a later batch is served: {later:?} after {} reads", calls.load(Ordering::SeqCst));
}

/// #644's round-7 review: the stem's budget is a quarter of the soft open-file limit, at least 32 and at most
/// 16,384 (`None`, unlimited, read as 1,024).
#[test]
fn the_stem_budget_is_a_quarter_of_the_soft_limit_clamped() {
    assert_eq!(stem_budget(Some(256)), 64, "launchd's default: a quarter");
    assert_eq!(stem_budget(Some(1024)), 256);
    assert_eq!(stem_budget(Some(64)), 32, "clamped up to 32");
    assert_eq!(stem_budget(Some(0)), 32);
    assert_eq!(stem_budget(Some(1 << 20)), 16_384, "clamped down to 16,384");
    assert_eq!(stem_budget(Some(u64::MAX)), 16_384);
    assert_eq!(stem_budget(None), 256);
}

/// A keep-alive upstream that answers every request on a connection and counts the connections still open.
async fn keepalive_upstream() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let live = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&live);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let live = Arc::clone(&count);
            live.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    while let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        buf.drain(..end + 4);
                        if sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await.is_err() {
                            break;
                        }
                    }
                }
                live.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    (port, live)
}

/// #644's round-6 review, finding 1: another user's flood on one proxy cannot spend the stem's descriptor budget.
/// Each proxy draws on its share of the budget; a proxy at its share **stops accepting** (the flood waits in the
/// kernel's backlog, holding no descriptor of the stem's), and part of each share is reserved for upstream sockets.
/// So proxy B's command is still accepted and forwarded, and proxy A's own command — admitted before the flood —
/// still reaches its upstream. Seen failing first on round 5's single shared pool: A's flood took all 16
/// descriptors and B's command's connection was accepted and closed (`fd_budget`); and with the share or the
/// reserve planted out.
///
/// What it measures, and when (the 2026-10-20 flake, CI run 39392016584: `used` read 8): A's own count is sampled
/// for the whole test and never exceeds its share; the whole budget is held to 7 only at a moment with nothing in
/// flight (the flood settled, before either command). Read straight after the commands, `used` may still count a
/// socket the proxy has answered but not yet closed — B's connection, or an upstream socket whose relay task has
/// not ended — which is a descriptor really held, so it is waited out rather than asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_on_one_proxy_leaves_every_other_proxy_its_share() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (u, _) = upstream().await;
    let me = crate::peer_uid::own_uid();
    // A's first read (its command) answers at once; every later read (the flood) takes 12 s, so the flood holds
    // what it was given for the whole test.
    let reads = Arc::new(AtomicU64::new(0));
    let slow: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
            return pairs.iter().map(|_| Ok(me)).collect();
        }
        std::thread::sleep(std::time::Duration::from_secs(12));
        pairs.iter().map(|_| Ok(me.wrapping_add(1))).collect()
    });
    let fast: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| pairs.iter().map(|_| Ok(me)).collect());
    let budget = FdBudget::new(16);
    let limits = ProxyLimits { max_unchecked: 100, peer_read_timeout_ms: 60_000, preauth_timeout_ms: 60_000, header_read_timeout_ms: 60_000, ..ProxyLimits::default() };
    let hosts = vec![format!("http://127.0.0.1:{u}")];
    let a = start_with_budget(hosts.clone(), limits, Some(slow), Arc::clone(&budget));
    let b = start_with_budget(hosts, limits, Some(fast), Arc::clone(&budget));
    // A's share is 8 (16 over two proxies); its accepted sockets may use 6 of it, its upstream reserve the other 2.
    let (a_share, a_accept) = (budget.share(), budget.share() - (budget.share() / 4).max(1));
    assert_eq!((a_share, a_accept), (8, 6));
    // A's count, sampled for the whole test: the most it ever held.
    let a_max = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sampler = {
        let (share, a_max) = (Arc::clone(&a.inner.share), Arc::clone(&a_max));
        tokio::spawn(async move {
            loop {
                a_max.fetch_max(share.held.load(Ordering::Acquire), Ordering::AcqRel);
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
    };
    // A's command connects and is admitted before the flood.
    let mut cmd_a = tokio::net::TcpStream::connect(("127.0.0.1", a.port())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut flood = Vec::new();
    for _ in 0..40 {
        flood.push(tokio::net::TcpStream::connect(("127.0.0.1", a.port())).await.unwrap());
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(a.counts().refused, 0, "nothing was accepted only to be closed: {:?}", a.counts());
    // Nothing in flight: A holds its command and the flood up to its accept limit; B's accept loop one descriptor
    // for its next connection.
    let a_held = a.inner.share.held.load(Ordering::Acquire);
    assert!(a_held <= a_accept, "A's accepted sockets stay within its share less the reserve: {a_held}");
    assert!(budget.used() <= a_accept + 1, "the flood holds no more than A's accept limit: {} (A {a_held})", budget.used());
    // B's command: accepted, admitted, forwarded.
    let get = format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n");
    let via_b = raw(b.port(), &token_header(&b), &get).await;
    assert!(via_b.starts_with("HTTP/1.1 200"), "B's command is served while A is flooded: {via_b:?} {:?} used {}", b.counts(), budget.used());
    // A's own admitted command still reaches its upstream (the reserve).
    cmd_a.write_all(format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n", token_header(&a)).as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(8), cmd_a.read_to_end(&mut answer)).await;
    let answer = String::from_utf8_lossy(&answer).to_string();
    assert!(answer.starts_with("HTTP/1.1 200"), "A's admitted command reaches its upstream: {answer:?} {:?}", a.counts());
    sampler.abort();
    let a_most = a_max.load(Ordering::Acquire).max(a.inner.share.held.load(Ordering::Acquire));
    assert!(a_most <= a_share, "A never holds more than its share: {a_most}");
    // Once both commands' sockets are closed, the flood again holds no more than A's accept limit, and B one.
    let mut used = budget.used();
    for _ in 0..100 {
        if used <= a_accept + 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        used = budget.used();
    }
    assert!(used <= a_accept + 1, "A holds no more than its share once the commands are done: {used}");
    drop(flood);
}

/// #644's round-6 review, finding 2: the proxy keeps no idle upstream socket — each closes when its answer has been
/// relayed, so the descriptor budget counts every upstream socket the proxy holds. Seen failing first with
/// reqwest's default pool: eight idle upstream connections stayed open after a burst of eight forwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_idle_upstream_socket_outlives_its_answer() {
    use std::sync::atomic::Ordering;
    let (u, live) = keepalive_upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{u}")], ProxyLimits::default());
    let token = token_header(&p);
    let get = format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut burst = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (port, token, get) = (p.port(), token.clone(), get.clone());
        burst.spawn(async move { raw(port, &token, &get).await });
    }
    while let Some(answer) = burst.join_next().await {
        let answer = answer.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }
    let mut open = live.load(Ordering::SeqCst);
    for _ in 0..20 {
        if open == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        open = live.load(Ordering::SeqCst);
    }
    assert_eq!(open, 0, "no upstream socket is kept idle");
}

/// #644's round-6 review, finding 3: a read of the kernel's table that never returns leaves **one** read
/// outstanding — later batches are refused while it is in flight, not given a thread each. Seen failing first:
/// every batch past the bound started a new read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_that_never_returns_leaves_one_read_outstanding() {
    let (u, _) = upstream().await;
    let calls = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&calls);
    let lookup: PeerUidLookup = Arc::new(move |_pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        seen.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_secs(30));
        Vec::new()
    });
    let limits = ProxyLimits { peer_read_timeout_ms: 200, ..ProxyLimits::default() };
    let p = start_with_peer_uid(vec![format!("http://127.0.0.1:{u}")], limits, Some(lookup));
    let (token, get) = (token_header(&p), format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n"));
    for _ in 0..4 {
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), raw(p.port(), &token, &get)).await.expect("refused within a bound");
        assert!(answer.is_empty(), "refused: {answer}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one read outstanding, not one per batch");
    assert!(p.counts().refused >= 4);
}

/// #644's round-4 review: one proxy holds at most `max_open` sockets, so an activation's proxy cannot take the
/// stem's descriptor table. Seen failing first: the extra connection was held. Since round 6 the fifth is not
/// accepted only to be closed: it waits in the kernel's backlog and is served once one of the four closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_holds_at_most_max_open_sockets() {
    let me = crate::peer_uid::own_uid();
    // Owners are named only after 3 s, so every connection is still held while the fifth arrives.
    let lookup: PeerUidLookup = Arc::new(move |pairs: &[(std::net::SocketAddr, std::net::SocketAddr)]| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        pairs.iter().map(|_| Ok(me)).collect()
    });
    let limits = ProxyLimits { max_open: 4, max_unchecked: 100, peer_read_timeout_ms: 10_000, preauth_timeout_ms: 60_000, ..ProxyLimits::default() };
    let p = start_with_peer_uid(Vec::new(), limits, Some(lookup));
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap());
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut fifth = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    assert!(!closed_within(&mut fifth, 1).await, "the fifth waits, not closed");
    assert_eq!(p.counts().waiting_max, 4, "only four were accepted: {:?}", p.counts());
    assert_eq!(p.counts().refused, 0);
    drop(held.pop());
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    fifth.write_all(b"GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), fifth.read_to_end(&mut answer)).await;
    assert!(String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 407"), "the fifth is accepted once one closes: {:?}", String::from_utf8_lossy(&answer));
    for h in &mut held {
        assert!(!closed_within(h, 1).await, "the others stay open");
    }
}

/// The real thing: a process of **another user** connects to the proxy and is closed at accept, with no answer
/// — by the kernel's lookup, not an injected one. Needs a second user and passwordless `sudo`; runs where
/// `MYCELIUM_EGRESS_OTHER_UID_LIVE_REQUIRED` is set (CI's Linux `wasm-host` job), skips elsewhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_users_connection_is_closed_at_accept() {
    if std::env::var_os("MYCELIUM_EGRESS_OTHER_UID_LIVE_REQUIRED").is_none() {
        eprintln!("another_users_connection_is_closed_at_accept: SKIPPED — set MYCELIUM_EGRESS_OTHER_UID_LIVE_REQUIRED where passwordless sudo and the user `nobody` exist");
        return;
    }
    let (u, seen) = upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{u}")], ProxyLimits::default());
    let script = format!(
        "echo client-ran; exec 3<>/dev/tcp/127.0.0.1/{port} || exit 9; printf 'GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {auth}\r\nConnection: close\r\n\r\n' >&3 2>/dev/null; cat <&3 2>/dev/null; true",
        port = p.port(),
        auth = token_header(&p)
    );
    let out = tokio::task::spawn_blocking(move || std::process::Command::new("sudo").current_dir("/").args(["-n", "-u", "nobody", "bash", "-c", &script]).output())
        .await
        .unwrap()
        .expect("sudo runs");
    let answer = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // sudo ran the client (`client-ran`) and its connection was made (exit 0, not 9) — then closed, which `cat`
    // sees as EOF (Linux) or a reset (macOS), both ignored. A sudo that could not run prints nothing and exits 1:
    // refused here rather than read as a closed connection.
    assert!(answer.starts_with("client-ran") && out.status.success(), "sudo -u nobody did not run and connect the client: {} {answer} {stderr}", out.status);
    assert!(!answer.contains("HTTP/1.1"), "another user's connection got an answer: {answer} ({stderr})");
    assert!(seen.lock().unwrap().is_empty(), "nothing was forwarded");
    let c = p.counts();
    assert_eq!(c.refused_other_user, 1, "refused as another user's, by the kernel's lookup: {c:?}");
    // The plant: this user, the same request, is served.
    let mine = raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await;
    assert!(mine.starts_with("HTTP/1.1 200"), "{mine}");
}

/// Has the server closed `s` (EOF or reset) within `secs`?
async fn closed_within(s: &mut tokio::net::TcpStream, secs: u64) -> bool {
    use tokio::io::AsyncReadExt as _;
    let mut buf = [0u8; 512];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), s.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return true,
            Ok(Ok(_)) => continue,
            Err(_) => continue,
        }
    }
    false
}

/// #644's review, finding 5 — any local process can connect, so the proxy bounds what one may hold: a
/// connection past the cap is closed at once; a request whose headers do not arrive in time is closed; a
/// tunnel that carries nothing is closed; a body past the buffering cap is refused 503. Seen failing first
/// with every limit set to its widest: each connection stayed open and the second body was buffered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_proxy_bounds_connections_header_reads_tunnels_and_bodies() {
    use tokio::io::AsyncWriteExt as _;
    let wide = ProxyLimits { max_connections: 1000, max_unauthenticated: 1000, preauth_timeout_ms: 600_000, max_unchecked: 1000, peer_read_timeout_ms: 600_000, max_open: 10_000, header_read_timeout_ms: 600_000, tunnel_idle_ms: 600_000, max_buffered_bodies: 1000 };
    // Connections: two authenticated and held open, a third authenticated one refused 503 (#644's round-2
    // review, N2: only a connection that presented the token counts against the cap).
    let (cu, _) = upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{cu}")], ProxyLimits { max_connections: 2, ..wide });
    let mut held = Vec::new();
    for _ in 0..2 {
        held.push(authenticated_keepalive(&p, cu).await);
    }
    let third = raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{cu}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await;
    assert!(third.starts_with("HTTP/1.1 503"), "an authenticated connection past the cap is refused: {third}");
    drop(held);

    // Header read: a request whose headers never finish is closed.
    let p = start_limited(Vec::new(), ProxyLimits { header_read_timeout_ms: 300, ..wide });
    let mut slow = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    slow.write_all(b"GET http://127.0.0.1/ HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    assert!(closed_within(&mut slow, 5).await, "a slow header is closed");

    // Tunnel idle: a silent upstream's tunnel is closed.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sp = silent.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut keep = Vec::new();
        while let Ok((c, _)) = silent.accept().await {
            keep.push(c);
        }
    });
    let p = start_limited(vec![format!("http://127.0.0.1:{sp}")], ProxyLimits { tunnel_idle_ms: 300, ..wide });
    let mut t = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    t.write_all(format!("CONNECT 127.0.0.1:{sp} HTTP/1.1\r\nHost: 127.0.0.1:{sp}\r\nProxy-Authorization: {}\r\n\r\n", token_header(&p)).as_bytes()).await.unwrap();
    let mut head = [0u8; 64];
    let n = tokio::io::AsyncReadExt::read(&mut t, &mut head).await.unwrap();
    assert!(head[..n].starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head[..n]));
    assert!(closed_within(&mut t, 5).await, "an idle tunnel is closed");

    // Bodies: one being buffered, the second refused.
    let (u, _) = upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{u}")], ProxyLimits { max_buffered_bodies: 1, ..wide });
    let mut first = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    first.write_all(format!("POST http://127.0.0.1:{u}/a HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\nContent-Length: 100\r\n\r\npartial", token_header(&p)).as_bytes()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let second = raw(p.port(), &token_header(&p), &format!("POST http://127.0.0.1:{u}/b HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi")).await;
    assert!(second.starts_with("HTTP/1.1 503"), "{second}");
    drop(first);
}

/// An authenticated keep-alive connection: one request answered, the connection left open.
async fn authenticated_keepalive(p: &EgressProxy, upstream: u16) -> tokio::net::TcpStream {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{upstream}/ HTTP/1.1\r\nHost: x\r\nProxy-Authorization: {}\r\n\r\n", token_header(p)).as_bytes()).await.unwrap();
    let mut buf = [0u8; 512];
    let n = s.read(&mut buf).await.unwrap();
    assert!(buf[..n].starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&buf[..n]));
    s
}

/// #644's round-2 review, N1: the body cap counts bodies being buffered, not requests in flight — five
/// concurrent bodiless requests to a slow upstream, past the four-body cap, all succeed. Seen failing first:
/// the fifth was refused 503.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bodiless_requests_in_flight_are_not_counted_as_buffered_bodies() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
            });
        }
    });
    let p = start_limited(vec![format!("http://127.0.0.1:{port}")], ProxyLimits::default());
    let c = client_via(&proxy_url(&p));
    let calls: Vec<_> = (0..5).map(|i| c.get(format!("http://127.0.0.1:{port}/{i}")).send()).collect();
    let codes: Vec<u16> = futures_join(calls).await.into_iter().map(|r| r.unwrap().status().as_u16()).collect();
    assert_eq!(codes, [200; 5], "{codes:?}");
}

async fn futures_join<F: std::future::Future + Send + 'static>(fs: Vec<F>) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let hs: Vec<_> = fs.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::new();
    for h in hs {
        out.push(h.await.unwrap());
    }
    out
}

/// #644's round-2 review, N2: token-less local connections cannot lock the command out of its own proxy — 70
/// idle ones, more than the authenticated cap, and the command's request still gets through; an unauthenticated
/// connection is closed after a short pre-auth timeout, and on its 407. Seen failing first: the 65th connection
/// was closed at accept and the command's request with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_less_connections_cannot_lock_the_command_out() {
    let (u, _) = upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{u}")], ProxyLimits::default());
    let mut idle = Vec::new();
    for _ in 0..70 {
        idle.push(tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap());
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), client_via(&proxy_url(&p)).get(format!("http://127.0.0.1:{u}/through")).send())
        .await
        .expect("not stuck behind the idle connections")
        .expect("answered");
    assert_eq!(r.status().as_u16(), 200);
    // An idle token-less connection is closed within the pre-auth timeout.
    let mut late = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    assert!(closed_within(&mut late, 5).await, "an unauthenticated idle connection is closed");
    // A 407 closes its connection.
    use tokio::io::AsyncWriteExt as _;
    let mut bare = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    bare.write_all(format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
    assert!(closed_within(&mut bare, 2).await, "a 407 closes the connection");
    drop(idle);
}

/// #644's round-2 review, N3: a `CONNECT` tunnel counts against the connection cap until it closes. Seen
/// failing first: with one tunnel open under a cap of one, another authenticated request was served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_counts_against_the_connection_cap_until_it_closes() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sp = silent.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut keep = Vec::new();
        while let Ok((c, _)) = silent.accept().await {
            keep.push(c);
        }
    });
    let (u, _) = upstream().await;
    let p = start_limited(vec![format!("http://127.0.0.1:{sp}"), format!("http://127.0.0.1:{u}")], ProxyLimits { max_connections: 1, ..ProxyLimits::default() });
    let mut t = tokio::net::TcpStream::connect(("127.0.0.1", p.port())).await.unwrap();
    t.write_all(format!("CONNECT 127.0.0.1:{sp} HTTP/1.1\r\nHost: 127.0.0.1:{sp}\r\nProxy-Authorization: {}\r\n\r\n", token_header(&p)).as_bytes()).await.unwrap();
    let mut head = [0u8; 64];
    let n = t.read(&mut head).await.unwrap();
    assert!(head[..n].starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head[..n]));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let other = raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await;
    assert!(other.starts_with("HTTP/1.1 503"), "the open tunnel holds the only slot: {other}");
    drop(t);
    let mut freed = false;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if raw(p.port(), &token_header(&p), &format!("GET http://127.0.0.1:{u}/ HTTP/1.1\r\nHost: x\r\n\r\n")).await.starts_with("HTTP/1.1 200") {
            freed = true;
            break;
        }
    }
    assert!(freed, "the slot is freed when the tunnel closes");
}

/// An upstream that answers with `extra` headers (and a two-byte body, chunked when `chunked`).
#[cfg(any(feature = "gateway", feature = "llm"))]
async fn answering_with(extra: &'static str, chunked: bool) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let body = if chunked { "2\r\nok\r\n0\r\n\r\n" } else { "ok" };
            let len = if chunked { String::new() } else { "Content-Length: 2\r\n".into() };
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\n{extra}{len}Connection: close\r\n\r\n{body}").as_bytes()).await;
        }
    });
    port
}

/// #644's round-2 review, N5: every `Content-Encoding` header is read (two of them, or any non-identity coding
/// in a list, is refused) and a `Transfer-Encoding` carrying anything but `chunked` is refused too. Seen failing
/// first: both were relayed.
#[cfg(any(feature = "gateway", feature = "llm"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_encoding_header_is_read_before_a_body_is_relayed() {
    for (extra, chunked) in [
        ("Content-Encoding: identity\r\nContent-Encoding: gzip\r\n", false),
        ("Content-Encoding: identity, br\r\n", false),
        ("Transfer-Encoding: gzip, chunked\r\n", true),
    ] {
        let port = answering_with(extra, chunked).await;
        let file = std::env::temp_dir().join(format!("act-egress-n5-{}-{port}", std::process::id()));
        std::fs::write(&file, "sk-n5-7f").unwrap();
        let binding = mycelium::CredentialBinding {
            name: "model".into(), host: "127.0.0.1".into(), port: Some(port), path_prefix: None,
            form: mycelium::CredentialForm::Bearer, header: None, username: None,
            value_env: None, value_file: Some(file.clone()), allow_plaintext: false,
        };
        let gate = Arc::new(mycelium::EgressCredentials::load(&[binding], &EgressPolicy::default()).unwrap());
        let decl = EgressDecl { hosts: vec![format!("http://127.0.0.1:{port}")], credentials: vec!["model".into()] };
        let p = EgressProxy::start(ProxyOptions {
            label: "data/pack".into(), decl: &decl, node_egress: &EgressPolicy::default(), bind: EgressProxy::DEFAULT_BIND,
            gate: Some(gate), limits: ProxyLimits::default(), peer_uid: None, fd_budget: None,
        })
        .unwrap();
        let r = client_via(&proxy_url(&p)).get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 502, "{extra:?} must be refused");
        let _ = std::fs::remove_file(&file);
    }
}

/// An upstream answering every request with `status` and `extra` headers and, unless `HEAD` or bodiless, a
/// two-byte body.
#[cfg(any(feature = "gateway", feature = "llm"))]
async fn answering_status(status: &'static str, extra: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = buf[..n].starts_with(b"HEAD ");
            let body = if head || status.starts_with("304") { "" } else { "ok" };
            let len = if status.starts_with("304") { String::new() } else { "Content-Length: 2\r\n".into() };
            let _ = sock.write_all(format!("HTTP/1.1 {status}\r\n{extra}{len}Connection: close\r\n\r\n{body}").as_bytes()).await;
        }
    });
    port
}

/// #644's round-3 review, finding 3: what carries no body has nothing to mask — a `HEAD`'s answer and a 304
/// are relayed whatever their `Content-Encoding` says — and repeated `identity` headers are no encoding. Seen
/// failing first: all three answered 502.
#[cfg(any(feature = "gateway", feature = "llm"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bodiless_answer_or_repeated_identity_is_relayed() {
    for (status, extra, method, want) in [
        ("200 OK", "Content-Encoding: gzip\r\n", reqwest::Method::HEAD, 200u16),
        ("304 Not Modified", "Content-Encoding: gzip\r\n", reqwest::Method::GET, 304),
        ("200 OK", "Content-Encoding: identity\r\nContent-Encoding: identity\r\n", reqwest::Method::GET, 200),
    ] {
        let port = answering_status(status, extra).await;
        let file = std::env::temp_dir().join(format!("act-egress-r3-{}-{port}", std::process::id()));
        std::fs::write(&file, "sk-r3-7f").unwrap();
        let binding = mycelium::CredentialBinding {
            name: "model".into(), host: "127.0.0.1".into(), port: Some(port), path_prefix: None,
            form: mycelium::CredentialForm::Bearer, header: None, username: None,
            value_env: None, value_file: Some(file.clone()), allow_plaintext: false,
        };
        let gate = Arc::new(mycelium::EgressCredentials::load(&[binding], &EgressPolicy::default()).unwrap());
        let decl = EgressDecl { hosts: vec![format!("http://127.0.0.1:{port}")], credentials: vec!["model".into()] };
        let p = EgressProxy::start(ProxyOptions {
            label: "data/pack".into(), decl: &decl, node_egress: &EgressPolicy::default(), bind: EgressProxy::DEFAULT_BIND,
            gate: Some(gate), limits: ProxyLimits::default(), peer_uid: None, fd_budget: None,
        })
        .unwrap();
        let r = client_via(&proxy_url(&p)).request(method.clone(), format!("http://127.0.0.1:{port}/")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), want, "{status} {extra:?} {method}");
        let _ = std::fs::remove_file(&file);
    }
}

/// A TLS upstream on loopback for `localhost`, signed by a test CA. Answers every request with the
/// `Authorization` it received echoed in a header and, split across two chunks, in the body.
#[cfg(any(feature = "gateway", feature = "llm"))]
async fn tls_echo() -> (u16, Vec<u8>, Arc<Mutex<Vec<String>>>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let now = time::OffsetDateTime::now_utc();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    // Distinct names: with rcgen's default for both, OpenSSL reads the leaf as self-signed (seen in CI).
    ca_params.distinguished_name.push(rcgen::DnType::CommonName, "mycelium egress test CA");
    ca_params.not_before = now - time::Duration::days(1);
    ca_params.not_after = now + time::Duration::days(30);
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf_params.not_before = now - time::Duration::days(1);
    leaf_params.not_after = now + time::Duration::days(30);
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    leaf_params.distinguished_name.push(rcgen::DnType::CommonName, "localhost");
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()))
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let (acceptor, log) = (acceptor.clone(), Arc::clone(&log));
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let Ok(mut tls) = acceptor.accept(sock).await else { return };
                let mut buf = vec![0u8; 16384];
                let n = tls.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                log.lock().unwrap().push(head.clone());
                let auth = head.lines().find_map(|l| l.to_ascii_lowercase().starts_with("authorization:").then(|| l[14..].trim().to_string())).unwrap_or_default();
                let (a, b) = auth.split_at(auth.len() / 2);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nx-echo: {auth}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                    a.len(), b.len()
                );
                let _ = tls.write_all(reply.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, ca.pem().into_bytes(), seen)
}

/// #644's review, findings 6 and 3: the credential's real route — `http://` to the proxy, TLS from the proxy
/// to an `https://` entry — carries the value to the bound host over TLS (the upstream, trusting only the test
/// CA, saw it), and an upstream that echoes it back has it masked in the header and in a body split across
/// chunks before the command sees it; a `CONNECT` to the bound host, where the proxy could not look, is
/// refused. Seen failing first with the redactor off: the echo carried the value to the client.
#[cfg(any(feature = "gateway", feature = "llm"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_https_entry_gets_the_credential_over_tls_and_an_echo_of_it_is_masked() {
    const SECRET: &str = "sk-act-egress-tls-7f3a-never-relayed";
    let (port, ca_pem, seen) = tls_echo().await;
    let file = std::env::temp_dir().join(format!("act-egress-tls-{}", std::process::id()));
    std::fs::write(&file, SECRET).unwrap();
    let binding = mycelium::CredentialBinding {
        name: "model".into(), host: "localhost".into(), port: Some(port), path_prefix: None,
        form: mycelium::CredentialForm::Bearer, header: None, username: None,
        value_env: None, value_file: Some(file.clone()), allow_plaintext: false,
    };
    let gate = Arc::new(mycelium::EgressCredentials::load_trusting_root(&[binding], &EgressPolicy::default(), &ca_pem).unwrap());
    let decl = EgressDecl { hosts: vec![format!("https://localhost:{port}")], credentials: vec!["model".into()] };
    let p = EgressProxy::start(ProxyOptions {
        label: "data/pack".into(), decl: &decl, node_egress: &EgressPolicy::default(), bind: EgressProxy::DEFAULT_BIND,
        gate: Some(Arc::clone(&gate)), limits: ProxyLimits::default(), peer_uid: None, fd_budget: None,
    })
    .unwrap();
    let r = client_via(&proxy_url(&p))
        .get(format!("http://localhost:{port}/v1/echo"))
        .header("authorization", format!("Bearer {}", mycelium::credential_placeholder("model")))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    if status != 200 {
        panic!("{status}: {}", r.text().await.unwrap_or_default());
    }
    let echo = r.headers().get("x-echo").unwrap().to_str().unwrap().to_string();
    let body = r.text().await.unwrap();
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one request, over TLS");
    assert!(got[0].to_ascii_lowercase().contains(&format!("authorization: bearer {SECRET}")), "attached over TLS: {}", got[0]);
    assert!(!echo.contains(SECRET) && echo.contains("*****"), "the echoed header is masked: {echo}");
    assert!(!body.contains(SECRET) && body.len() == format!("Bearer {SECRET}").len(), "the echoed body is masked, its length kept: {body}");
    assert_eq!(gate.uses()[0].attached, 1);
    // A tunnel to the bound host is refused: the proxy could not see what comes back through it.
    let t = raw(p.port(), &token_header(&p), &format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")).await;
    assert!(t.starts_with("HTTP/1.1 403") && t.contains("credential `model`"), "{t}");
    assert_eq!(seen.lock().unwrap().len(), 1);
    let _ = std::fs::remove_file(&file);
}

/// #644's review, finding 3: with a value to mask, the proxy asks for an unencoded body and refuses an encoded
/// one — a value inside a gzip body could not be found. Seen failing first: the encoded body was relayed.
#[cfg(any(feature = "gateway", feature = "llm"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encoded_response_is_refused_where_a_value_could_hide_in_it() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&asked);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\x1f\x8b\x08\x00").await;
        }
    });
    let file = std::env::temp_dir().join(format!("act-egress-gz-{}", std::process::id()));
    std::fs::write(&file, "sk-gz-7f").unwrap();
    let binding = mycelium::CredentialBinding {
        name: "model".into(), host: "127.0.0.1".into(), port: Some(port), path_prefix: None,
        form: mycelium::CredentialForm::Bearer, header: None, username: None,
        value_env: None, value_file: Some(file.clone()), allow_plaintext: false,
    };
    let gate = Arc::new(mycelium::EgressCredentials::load(&[binding], &EgressPolicy::default()).unwrap());
    let decl = EgressDecl { hosts: vec![format!("http://127.0.0.1:{port}")], credentials: vec!["model".into()] };
    let p = EgressProxy::start(ProxyOptions {
        label: "data/pack".into(), decl: &decl, node_egress: &EgressPolicy::default(), bind: EgressProxy::DEFAULT_BIND,
        gate: Some(gate), limits: ProxyLimits::default(), peer_uid: None, fd_budget: None,
    })
    .unwrap();
    let r = client_via(&proxy_url(&p)).get(format!("http://127.0.0.1:{port}/")).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 502);
    assert!(r.text().await.unwrap().contains("encoded body"));
    assert!(asked.lock().unwrap()[0].contains("accept-encoding: identity"), "{:?}", asked.lock().unwrap());
    let _ = std::fs::remove_file(&file);
}
