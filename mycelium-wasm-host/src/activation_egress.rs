//! **An egress proxy for a confined activation** (post-360 rows L and M): the one network door an
//! `[[activation]]` with an `[activation.confine.egress]` table has.
//!
//! The stem starts one proxy **per such activation**, on loopback, when it starts, and keeps it for its
//! life. The activation's confinement profile lets its command and probe open a TCP connection to that one
//! port (`localhost:<port>` under Seatbelt; the port, on any address, under Landlock — stated below), and
//! the command is started with a **cleared environment** that names the proxy
//! ([`EgressProxy::command_env`]). The proxy then:
//!
//! - **limits by hostname** — a request reaches only an upstream in the activation's `hosts` list, matched
//!   by the name the command asked for (never by suffix, never by the address it resolves to), and the
//!   node's own egress allow-list (`EgressPolicy`) is checked too;
//! - **changes live** — the list is held behind one lock ([`EgressProxy::set_hosts`]); a change applies to
//!   the next request, with the command, its profile and the proxy's port unchanged;
//! - **attaches credentials** — a request header carrying a placeholder (`mycelium-credential:<name>`) is
//!   sent through the node's egress gate (`mycelium::EgressCredentials::send_once`), which attaches the
//!   binding's value only where the binding authorises (its host, port, path prefix, https or loopback) and
//!   refuses the request before it is sent anywhere else. A placeholder naming a binding the activation's
//!   `credentials` does not list is refused here first. The value is never in the command's environment,
//!   arguments or files.
//!
//! **Two ways through, and why both** (the HTTPS question). A proxy cannot change a header inside TLS it does
//! not terminate, and this one does not terminate TLS — minting a CA the command must trust would put a key
//! able to impersonate every host in the stem's hands, for a capability (rewriting headers inside a tunnel)
//! a plain-HTTP hop gives without it. So:
//!
//! - **(a) absolute-form plain HTTP** — what `HTTP_PROXY` makes a client send (`GET http://host/path`): the
//!   proxy matches `host` (and the port, when written) to an entry and **originates the upstream request on
//!   the entry's scheme** — TLS for an `https://` entry — with placeholders resolved. This is the only way a
//!   credential is attached: a command that needs one writes its URL as `http://api.example.com/…` (the hop
//!   to the proxy is loopback; the hop that leaves the host is the entry's https).
//! - **(b) `CONNECT host:port`** — what `HTTPS_PROXY` makes a client send for an `https://` URL: a byte
//!   tunnel to an entry's host and port, allow-list only; the proxy cannot see or change what is inside, so
//!   no credential is attached (a placeholder inside reaches the upstream as the opaque name it is).
//!
//! Redirects are never followed here: a 3xx goes back to the command, whose client follows it through the
//! proxy again, so each hop meets the allow-list and the gate afresh.
//!
//! **Who may use it.** The proxy listens on `127.0.0.1` only and requires a per-proxy token
//! (`Proxy-Authorization: Basic mycelium:<token>`, which a client sends from the userinfo of the proxy URL
//! the command's environment carries) — without it any local process could have credentials attached. The
//! token is drawn from the operating system's random source, not the replay seam: a secret must not be
//! reproducible (activation, like any spawned process, is outside the replay seams already).
//!
//! **No silent fallback.** A proxy that cannot bind refuses its activation by name
//! (`egress_proxy_unavailable`), as row M's confinement refusals do; nothing runs without it.
//!
//! **What this is not**, stated: on Linux, Landlock limits a TCP port, not an address, so a confined command
//! can open a TCP connection to the proxy's port **on any address** — the hostname limit holds for all the
//! proxy carries, and a host listening on that port elsewhere is reachable directly (`stem.activation_egress`
//! reads `not_configured` there). Host names are compared, not the addresses they resolve to (the node
//! allow-list's own limit). A request body is buffered, up to [`MAX_REQUEST_BODY_BYTES`]; a placeholder in
//! a URL or body is not resolved. A path is not limited (a credential binding's `path_prefix` is). The
//! credentials list and the passed-through environment are fixed for the stem's life; only `hosts` changes
//! live. HTTP/1.1 only, between the command and the proxy.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt as _, Full};
use hyper::body::{Body, Frame, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use mycelium::{parse_activation_egress_host, EgressDecl, EgressHost, EgressPolicy};

/// The largest request body the proxy buffers before sending it on.
pub const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

/// How long a tunnel whose command side has closed may stay silent before it is closed (#644's round-2
/// review, N3).
const TUNNEL_HALF_CLOSED_MS: u64 = 2_000;

/// How long the proxy waits to open a connection upstream.
const CONNECT_TIMEOUT_MS: u64 = 10_000;

/// The user name in the proxy URL; the password is the proxy's token.
const PROXY_USER: &str = "mycelium";

type ProxyBody = BoxBody<Bytes, std::io::Error>;

/// What one proxy has done, from [`EgressProxy::counts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EgressCounts {
    /// Plain-HTTP requests sent on upstream (answered by the upstream, whatever its status).
    pub forwarded: u64,
    /// `CONNECT` tunnels opened.
    pub tunnelled: u64,
    /// Requests refused here (host not listed, credential not granted or not authorised, no token, …).
    pub refused: u64,
    /// Of those, connections another user's process opened, closed at accept (`peer_uid_other`).
    pub refused_other_user: u64,
    /// Reads of the kernel's socket table made to name connections' owners — one per batch, not per connection.
    pub peer_lookups: u64,
    /// The most connections ever waiting for a lookup at once (never above `ProxyLimits::max_unchecked`).
    pub waiting_max: u64,
}

struct Inner {
    /// `ns/name` of the activation, for logs, refusals and the metric label.
    label: String,
    /// The allow-list. Lock-order table row 64: a leaf, read once per request and replaced by
    /// [`EgressProxy::set_hosts`]; never held across an `await` or with another lock.
    hosts: RwLock<Arc<Vec<EgressHost>>>,
    /// The bindings a placeholder may name.
    credentials: Vec<String>,
    node_egress: EgressPolicy,
    /// `Basic base64(mycelium:<token>)`, the `Proxy-Authorization` value a client must send.
    auth: String,
    /// The proxy URL with its token, for the command's environment.
    url: String,
    #[cfg(any(feature = "gateway", feature = "llm"))]
    gate: Option<Arc<mycelium::EgressCredentials>>,
    /// Masks the granted bindings' values in every relayed response (#644's review, finding 3): an upstream
    /// that echoes what it was sent cannot hand the value to the command.
    #[cfg(any(feature = "gateway", feature = "llm"))]
    redactor: Option<mycelium::Redactor>,
    limits: ProxyLimits,
    /// Connections that presented the token and are still open (a tunnel's included), and those that have not
    /// yet (N2, N3) — each kept by its [`Conn`]'s life.
    authed_live: AtomicUsize,
    preauth_live: AtomicUsize,
    /// Sockets held ([`OpenSlot`]), against `limits.max_open`.
    open: AtomicUsize,
    refused_other_user: AtomicU64,
    peer_gate: Arc<PeerGate>,
    /// The connections that have not presented the token, oldest first, to close past
    /// `limits.max_unauthenticated`. Lock-order table row 65: a leaf, held for a push or a scan of weak
    /// references, never across an `await`.
    preauth_queue: std::sync::Mutex<std::collections::VecDeque<std::sync::Weak<Conn>>>,
    /// Request bodies being buffered (`limits.max_buffered_bodies` permits).
    bodies: Arc<tokio::sync::Semaphore>,
    /// For requests with no placeholder in a build without the gate: follows no redirect.
    #[cfg_attr(any(feature = "gateway", feature = "llm"), allow(dead_code))]
    client: reqwest::Client,
    forwarded: AtomicU64,
    tunnelled: AtomicU64,
    refused: AtomicU64,
}

/// One activation's egress proxy. Dropping it stops the listener (open tunnels end with their peers).
pub struct EgressProxy {
    inner: Arc<Inner>,
    port:  u16,
    task:  tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for EgressProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressProxy")
            .field("activation", &self.inner.label)
            .field("port", &self.port)
            .field("hosts", &self.hosts())
            .field("credentials", &self.inner.credentials)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Parse a `hosts` list — every entry, or the first refusal naming it — and check each against the node's
/// allow-list: an entry the node itself would refuse is refused here, by name, rather than accepted and
/// then refused at every request.
pub fn parse_hosts(hosts: &[String], node_egress: &EgressPolicy) -> Result<Vec<EgressHost>, String> {
    let mut out: Vec<EgressHost> = Vec::with_capacity(hosts.len());
    for h in hosts {
        let e = parse_activation_egress_host(h)?;
        if !node_egress.permits_host(&e.host) {
            return Err(format!("egress host {h:?}: the node's egress allow-list does not permit {}", e.host));
        }
        if out.iter().any(|o| o.host == e.host && o.port == e.port) {
            return Err(format!("egress host {h:?}: {}:{} is listed twice", e.host, e.port));
        }
        out.push(e);
    }
    Ok(out)
}

/// How much one proxy holds at once (#644's review, finding 5): any local process can connect to it, so a
/// slow or idle client must not hold it open without end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyLimits {
    /// **Authenticated** connections at once — one that has presented the token, counted until it closes, a
    /// `CONNECT` tunnel's for the tunnel's life (#644's round-2 review, N2, N3). An authenticated request past
    /// it is refused 503 and its connection closed, counted `connection_cap`.
    pub max_connections: usize,
    /// Connections that have not presented the token yet. Past it the **oldest** is closed to admit the new
    /// one, so idle token-less connections cannot keep the command from connecting (N2).
    pub max_unauthenticated: usize,
    /// How long a connection may stay without presenting the token before it is closed (N2).
    pub preauth_timeout_ms: u64,
    /// Connections waiting for their owner to be named; past it the oldest is closed (#644's round-4 review).
    pub max_unchecked: usize,
    /// Sockets the proxy holds at once, whatever their state; one accepted past it is closed at once, so one
    /// activation's proxy cannot take the stem's descriptor table (#644's round-4 review).
    pub max_open: usize,
    /// How long a connection may take to send a request's headers (hyper's header-read timeout).
    pub header_read_timeout_ms: u64,
    /// How long a `CONNECT` tunnel may carry nothing either way before it is closed.
    pub tunnel_idle_ms: u64,
    /// Request bodies buffered at once (each up to [`MAX_REQUEST_BODY_BYTES`]); one past it is refused 503.
    pub max_buffered_bodies: usize,
}

impl Default for ProxyLimits {
    fn default() -> Self {
        ProxyLimits { max_connections: 64, max_unauthenticated: 16, preauth_timeout_ms: 2_000, max_unchecked: 64, max_open: 256, header_read_timeout_ms: 10_000, tunnel_idle_ms: 300_000, max_buffered_bodies: 4 }
    }
}

/// The options a proxy starts with ([`EgressProxy::start`]).
pub struct ProxyOptions<'a> {
    /// `ns/name` of the activation.
    pub label: String,
    pub decl: &'a EgressDecl,
    pub node_egress: &'a EgressPolicy,
    /// Where it listens. `127.0.0.1:0` (a free port) unless a test needs otherwise; a non-loopback address
    /// is refused.
    pub bind: SocketAddr,
    /// The node's egress gate, which attaches credentials. `None`: a placeholder is refused.
    #[cfg(any(feature = "gateway", feature = "llm"))]
    pub gate: Option<Arc<mycelium::EgressCredentials>>,
    /// What it holds at once ([`ProxyLimits::default`] for a stem's).
    pub limits: ProxyLimits,
    /// Who owns connecting sockets — `None` for the kernel's answer ([`crate::peer_uid::loopback_peer_uids`]);
    /// a test injects its own. A connection whose owner is not this process's user, or cannot be read, is
    /// closed at accept (#644's round-3 review).
    pub peer_uid: Option<PeerUidLookup>,
}

/// Every `(our address, the peer's address)` pair's owner from one read of the kernel's table, in order — or why
/// a pair's cannot be told ([`crate::peer_uid::loopback_peer_uids`]).
pub type PeerUidLookup = Arc<dyn Fn(&[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> + Send + Sync>;

/// A connection waiting to have its owner named (#644's round-4 review).
struct Waiter {
    local: SocketAddr,
    peer: SocketAddr,
    reply: tokio::sync::oneshot::Sender<Result<u32, String>>,
}

/// The connections waiting for their owners to be named, and the one task that names them: every connection
/// accepted before a read of the kernel's table begins is answered by that read, so the work done grows with
/// reads, not connections, and a connection waits about one read whatever the arrival rate (#644's round-4
/// review, finding 1). At most `ProxyLimits::max_unchecked` wait; past it the **oldest** is closed.
struct PeerGate {
    /// Lock-order table row 66: a leaf, held for a push, an eviction or a drain — never across an `await`, a
    /// lookup or another lock.
    queue: std::sync::Mutex<std::collections::VecDeque<Waiter>>,
    wake: tokio::sync::Notify,
    lookups: AtomicU64,
    waiting_max: AtomicUsize,
}

impl PeerGate {
    /// Queue a connection; past `cap`, the oldest waiters are dropped (their connections close, counted).
    fn enqueue(&self, w: Waiter, cap: usize, label: &str) {
        let evicted = {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(w);
            let mut out = Vec::new();
            while q.len() > cap.max(1) {
                if let Some(old) = q.pop_front() {
                    out.push(old);
                }
            }
            self.waiting_max.fetch_max(q.len(), Ordering::Relaxed);
            out
        };
        for _ in &evicted {
            count(label, "peer_check_evicted");
        }
        drop(evicted);
        self.wake.notify_one();
    }

    /// The naming task: drain everything queued, answer it with one read, repeat until nothing waits.
    async fn run(self: Arc<Self>, lookup: PeerUidLookup) {
        loop {
            self.wake.notified().await;
            loop {
                let batch: Vec<Waiter> = self.queue.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                if batch.is_empty() {
                    break;
                }
                self.lookups.fetch_add(1, Ordering::Relaxed);
                let pairs: Vec<(SocketAddr, SocketAddr)> = batch.iter().map(|w| (w.local, w.peer)).collect();
                let lookup = Arc::clone(&lookup);
                let answers = tokio::task::spawn_blocking(move || lookup(&pairs)).await.unwrap_or_default();
                let mut answers = answers.into_iter();
                for w in batch {
                    let _ = w.reply.send(answers.next().unwrap_or_else(|| Err("the lookup gave no answer".into())));
                }
            }
        }
    }
}

/// One accepted socket held by the proxy, counted against `ProxyLimits::max_open` until dropped — by the
/// connection's task, or, inside its [`Conn`], by a `CONNECT` tunnel's.
struct OpenSlot(Arc<Inner>);

impl Drop for OpenSlot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

impl EgressProxy {
    /// The address a stem's proxy listens on: loopback, a free port.
    pub const DEFAULT_BIND: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

    /// Start a proxy: bind, then serve on the current tokio runtime. `Err` names why it could not start —
    /// its activation is then refused, never run without it.
    pub fn start(opts: ProxyOptions<'_>) -> Result<Self, String> {
        let label = opts.label;
        let fail = |why: String| format!("activation {label}: its egress proxy could not start — {why}");
        if !opts.bind.ip().is_loopback() {
            return Err(fail(format!("{} is not a loopback address", opts.bind)));
        }
        let hosts = parse_hosts(&opts.decl.hosts, opts.node_egress).map_err(&fail)?;
        let rt = tokio::runtime::Handle::try_current().map_err(|_| fail("no tokio runtime to serve on".into()))?;
        let listener = std::net::TcpListener::bind(opts.bind).map_err(|e| fail(format!("bind {}: {e}", opts.bind)))?;
        listener.set_nonblocking(true).map_err(|e| fail(format!("{e}")))?;
        let port = listener.local_addr().map_err(|e| fail(format!("{e}")))?.port();
        let token = token().map_err(&fail)?;
        let auth = {
            use base64_lite::encode;
            format!("Basic {}", encode(format!("{PROXY_USER}:{token}").as_bytes()))
        };
        let url = format!("http://{PROXY_USER}:{token}@127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_millis(CONNECT_TIMEOUT_MS))
            .build()
            .map_err(|e| fail(format!("http client: {e}")))?;
        let inner = Arc::new(Inner {
            label: label.clone(),
            hosts: RwLock::new(Arc::new(hosts)),
            credentials: opts.decl.credentials.clone(),
            node_egress: opts.node_egress.clone(),
            auth,
            url,
            #[cfg(any(feature = "gateway", feature = "llm"))]
            redactor: opts.gate.as_ref().map(|g| g.redactor(&opts.decl.credentials)).filter(|r| !r.is_empty()),
            #[cfg(any(feature = "gateway", feature = "llm"))]
            gate: opts.gate,
            limits: opts.limits,
            authed_live: AtomicUsize::new(0),
            preauth_live: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            refused_other_user: AtomicU64::new(0),
            peer_gate: Arc::new(PeerGate {
                queue: std::sync::Mutex::new(std::collections::VecDeque::new()),
                wake: tokio::sync::Notify::new(),
                lookups: AtomicU64::new(0),
                waiting_max: AtomicUsize::new(0),
            }),
            preauth_queue: std::sync::Mutex::new(std::collections::VecDeque::new()),
            bodies: Arc::new(tokio::sync::Semaphore::new(opts.limits.max_buffered_bodies.max(1))),
            client,
            forwarded: AtomicU64::new(0),
            tunnelled: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        });
        let listener = {
            let _guard = rt.enter();
            tokio::net::TcpListener::from_std(listener).map_err(|e| fail(format!("{e}")))?
        };
        let served = Arc::clone(&inner);
        let peer_uid: PeerUidLookup = opts.peer_uid.unwrap_or_else(|| Arc::new(crate::peer_uid::loopback_peer_uids));
        let own_uid = crate::peer_uid::own_uid();
        let namer = rt.spawn(Arc::clone(&inner.peer_gate).run(peer_uid));
        let task = rt.spawn(async move {
            let _namer = AbortOnDrop(namer);
            loop {
                let (sock, peer) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(activation = %served.label, error = %e, "egress proxy: accept failed");
                        mycelium::sim_seam::sleep_ms("activation_egress.accept", 50).await;
                        continue;
                    }
                };
                // #644's round-4 review: a hard ceiling on the sockets this proxy holds.
                if served.open.fetch_add(1, Ordering::AcqRel) >= served.limits.max_open.max(1) {
                    served.open.fetch_sub(1, Ordering::AcqRel);
                    served.refused.fetch_add(1, Ordering::Relaxed);
                    count(&served.label, "open_cap");
                    drop(sock);
                    continue;
                }
                let slot = OpenSlot(Arc::clone(&served));
                let inner = Arc::clone(&served);
                tokio::spawn(async move {
                    // #644's round-3 review: only this user's processes are served. Another user's connection —
                    // or one whose owner cannot be read (fail closed) — is closed before it holds any place.
                    if let Err(event) = admit_peer(&inner, &sock, peer, own_uid).await {
                        inner.refused.fetch_add(1, Ordering::Relaxed);
                        if event == "peer_uid_other" {
                            inner.refused_other_user.fetch_add(1, Ordering::Relaxed);
                        }
                        count(&inner.label, event);
                        return;
                    }
                    let conn = Conn::admit(&inner, slot);
                    let header_timeout = std::time::Duration::from_millis(inner.limits.header_read_timeout_ms.max(1));
                    let preauth_ms = inner.limits.preauth_timeout_ms.max(1);
                    let svc = {
                        let (inner, conn) = (Arc::clone(&inner), Arc::clone(&conn));
                        hyper::service::service_fn(move |req| {
                            let (inner, conn) = (Arc::clone(&inner), Arc::clone(&conn));
                            async move { Ok::<_, Infallible>(inner.handle(req, conn).await) }
                        })
                    };
                    let served = hyper::server::conn::http1::Builder::new()
                        .timer(hyper_util::rt::TokioTimer::new())
                        .header_read_timeout(header_timeout)
                        .serve_connection(hyper_util::rt::TokioIo::new(sock), svc)
                        .with_upgrades();
                    tokio::pin!(served);
                    let deadline = mycelium::sim_seam::sleep_ms("activation_egress.preauth", preauth_ms);
                    tokio::pin!(deadline);
                    let mut deadline_passed = false;
                    // N2: a connection that has not presented the token is closed at the pre-auth deadline, or
                    // when a newer one needs its place; dropping `served` closes it.
                    loop {
                        tokio::select! {
                            _ = &mut served => break,
                            _ = conn.evict.notified(), if !conn.authenticated() => break,
                            _ = &mut deadline, if !deadline_passed => {
                                if !conn.authenticated() {
                                    break;
                                }
                                deadline_passed = true;
                            }
                        }
                    }
                });
            }
        });
        tracing::info!(activation = %label, port, "activation egress proxy listening on loopback");
        Ok(EgressProxy { inner, port, task })
    }

    /// The loopback port it listens on — the one port the activation's profile lets it connect to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The allow-list now, as `scheme://host:port`.
    pub fn hosts(&self) -> Vec<String> {
        self.inner.hosts_now().iter().map(EgressHost::url_base).collect()
    }

    /// Replace the allow-list. Every entry is checked (form, duplicates, the node's allow-list) before
    /// anything changes; on `Err` the old list stands. Applies to the next request — the command, its
    /// profile and this port are unchanged. An open `CONNECT` tunnel is not closed.
    pub fn set_hosts(&self, hosts: &[String]) -> Result<(), String> {
        let parsed = parse_hosts(hosts, &self.inner.node_egress)?;
        *self.inner.hosts.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(parsed);
        tracing::info!(activation = %self.inner.label, hosts = ?hosts, "activation egress allow-list replaced");
        Ok(())
    }

    /// What it has done.
    pub fn counts(&self) -> EgressCounts {
        EgressCounts {
            forwarded: self.inner.forwarded.load(Ordering::Relaxed),
            tunnelled: self.inner.tunnelled.load(Ordering::Relaxed),
            refused: self.inner.refused.load(Ordering::Relaxed),
            refused_other_user: self.inner.refused_other_user.load(Ordering::Relaxed),
            peer_lookups: self.inner.peer_gate.lookups.load(Ordering::Relaxed),
            waiting_max: self.inner.peer_gate.waiting_max.load(Ordering::Relaxed) as u64,
        }
    }

    /// The environment an egress-proxied command and probe start with: [`confined_env`] with this proxy's
    /// URL in every proxy variable ([`mycelium::ACTIVATION_EGRESS_PROXY_ENV`]).
    pub fn command_env(&self, passthrough: &[String]) -> Vec<(String, String)> {
        confined_env(passthrough, Some(&self.inner.url))
    }
}

/// The environment **every confined** command and probe start with (#644's review, finding 1) — cleared of
/// everything else: the base set ([`mycelium::ACTIVATION_EGRESS_BASE_ENV`]) and the `[activation.confine] env`
/// names, each as the stem has it (absent ones skipped, each once), then, with an egress proxy, its URL in
/// every proxy variable. Which of these may hold a credential is checked when the stem starts
/// ([`crate::stem::start_egress_proxies`]).
pub fn confined_env(passthrough: &[String], proxy_url: Option<&str>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for k in mycelium::ACTIVATION_EGRESS_BASE_ENV.iter().map(|s| s.to_string()).chain(passthrough.iter().cloned()) {
        if env.iter().any(|(have, _)| *have == k) {
            continue;
        }
        if let Ok(v) = std::env::var(&k) {
            env.push((k, v));
        }
    }
    if let Some(url) = proxy_url {
        for k in mycelium::ACTIVATION_EGRESS_PROXY_ENV {
            env.push((k.to_string(), url.to_string()));
        }
    }
    env
}

/// 16 bytes from the operating system's random source, hex.
fn token() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| format!("the OS random source: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Constant-time byte comparison (the token is compared on every request).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn full(status: StatusCode, text: String) -> Response<ProxyBody> {
    let mut r = Response::new(Full::new(Bytes::from(text)).map_err(|never: Infallible| match never {}).boxed());
    *r.status_mut() = status;
    r
}

/// Headers that belong to one hop, never forwarded either way.
const HOP_BY_HOP: &[&str] = &[
    "connection", "proxy-connection", "keep-alive", "proxy-authorization", "proxy-authenticate", "te", "trailer",
    "transfer-encoding", "upgrade", "host",
];

/// `host` as the allow-list holds it: lower-case, an IPv6 address without brackets.
fn norm_host(h: &str) -> String {
    h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase()
}

impl Inner {
    fn hosts_now(&self) -> Arc<Vec<EgressHost>> {
        Arc::clone(&self.hosts.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Refuse with `status`, counted and logged under `reason` (one of `prov.activation_egress`'s refusal
    /// reasons); `why` is the body's prose — never a credential's value.
    fn refuse(&self, status: StatusCode, host: &str, reason: &'static str, why: String) -> Response<ProxyBody> {
        self.refused.fetch_add(1, Ordering::Relaxed);
        count(&self.label, reason);
        tracing::info!(activation = %self.label, host, reason, detail = %why, "activation egress refused");
        let mut r = full(status, format!("mycelium egress proxy ({}): refused: {why}\n", self.label));
        r.headers_mut().insert("x-mycelium-egress", hyper::header::HeaderValue::from_static("refused"));
        r
    }

    async fn handle(self: Arc<Self>, req: Request<Incoming>, conn: Arc<Conn>) -> Response<ProxyBody> {
        let authorised = req
            .headers()
            .get(hyper::header::PROXY_AUTHORIZATION)
            .is_some_and(|v| same(v.as_bytes(), self.auth.as_bytes()));
        if !authorised {
            let mut r = self.refuse(StatusCode::PROXY_AUTHENTICATION_REQUIRED, "-", "no_proxy_token", "no proxy token (use the proxy URL from the environment, userinfo included)".into());
            r.headers_mut().insert(hyper::header::PROXY_AUTHENTICATE, hyper::header::HeaderValue::from_static("Basic realm=\"mycelium-egress\""));
            // N2: a connection that cannot authenticate is not kept open for another try.
            r.headers_mut().insert(hyper::header::CONNECTION, hyper::header::HeaderValue::from_static("close"));
            return r;
        }
        if !conn.authenticate(&self) {
            let mut r = self.refuse(StatusCode::SERVICE_UNAVAILABLE, "-", "connection_cap", format!("{} authenticated connections are already open", self.limits.max_connections));
            r.headers_mut().insert(hyper::header::CONNECTION, hyper::header::HeaderValue::from_static("close"));
            return r;
        }
        if req.method() == Method::CONNECT {
            self.connect(req, conn).await
        } else {
            self.forward(req).await
        }
    }

    /// (b) A `CONNECT host:port` tunnel to a listed entry: allow-list only, nothing inside looked at.
    async fn connect(self: Arc<Self>, mut req: Request<Incoming>, conn: Arc<Conn>) -> Response<ProxyBody> {
        let Some(auth) = req.uri().authority().cloned() else {
            return self.refuse(StatusCode::BAD_REQUEST, "-", "malformed_request", "CONNECT names no host:port".into());
        };
        let host = norm_host(auth.host());
        let Some(port) = auth.port_u16() else {
            return self.refuse(StatusCode::BAD_REQUEST, &host, "malformed_request", "CONNECT names no port".into());
        };
        if !self.hosts_now().iter().any(|e| e.host == host && e.port == port) {
            return self.refuse(StatusCode::FORBIDDEN, &host, "host_not_listed", format!("{host}:{port} is not in this activation's egress hosts"));
        }
        if !self.node_egress.permits_host(&host) {
            return self.refuse(StatusCode::FORBIDDEN, &host, "node_egress_denied", format!("the node's egress allow-list does not permit {host}"));
        }
        // #644's review, finding 3: a host a granted binding is bound to is reached over plain HTTP to the
        // proxy only, where its responses are checked for the value — never through a tunnel the proxy
        // cannot see into, where the host could hand back what it was sent.
        #[cfg(any(feature = "gateway", feature = "llm"))]
        if let Some(gate) = &self.gate
            && let Some(c) = self.credentials.iter().find(|c| gate.binds_host(c, &host))
        {
            return self.refuse(StatusCode::FORBIDDEN, &host, "credential_host_tunnel", format!(
                "{host} is credential `{c}`'s host — reach it as http://{host}/… through the proxy, which originates TLS, not by CONNECT"
            ));
        }
        let dial = tokio::net::TcpStream::connect((host.as_str(), port));
        let upstream = tokio::select! {
            r = dial => r,
            _ = mycelium::sim_seam::sleep_ms("activation_egress.connect", CONNECT_TIMEOUT_MS) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out")),
        };
        let upstream = match upstream {
            Ok(s) => s,
            Err(e) => {
                tracing::info!(activation = %self.label, host = %host, error = %e, "activation egress: CONNECT upstream failed");
                return full(StatusCode::BAD_GATEWAY, format!("mycelium egress proxy ({}): {host}:{port}: {e}\n", self.label));
            }
        };
        self.tunnelled.fetch_add(1, Ordering::Relaxed);
        count(&self.label, "tunnelled");
        let on_upgrade = hyper::upgrade::on(&mut req);
        let idle_ms = self.limits.tunnel_idle_ms.max(1);
        let label = self.label.clone();
        tokio::spawn(async move {
            // N3: the tunnel holds its connection's place against the cap until it closes.
            let _conn = conn;
            let Ok(up) = on_upgrade.await else { return };
            // #644's review, finding 5: a tunnel that carries nothing either way for `tunnel_idle_ms` is closed.
            let last = Arc::new(AtomicU64::new(mycelium::sim_seam::mono_now_ns()));
            let io = Idle { io: hyper_util::rt::TokioIo::new(up), last: Arc::clone(&last) };
            let (mut client_r, mut client_w) = tokio::io::split(io);
            let (mut up_r, mut up_w) = upstream.into_split();
            // N3: once the command has closed its side, a tunnel the upstream keeps silent is closed after a short
            // grace (`TUNNEL_HALF_CLOSED_MS`) rather than holding its place until the idle bound.
            let client_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let done = Arc::clone(&client_done);
            let mut to_up = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut client_r, &mut up_w).await;
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut up_w).await;
                done.store(true, Ordering::Release);
            });
            let mut to_client = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut up_r, &mut client_w).await;
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut client_w).await;
            });
            let check = (idle_ms / 4).clamp(1, 500);
            loop {
                tokio::select! {
                    _ = &mut to_client => break,
                    _ = mycelium::sim_seam::sleep_ms("activation_egress.tunnel_idle", check) => {
                        let bound = if client_done.load(Ordering::Acquire) { idle_ms.min(TUNNEL_HALF_CLOSED_MS) } else { idle_ms };
                        let idle = mycelium::sim_seam::mono_since(last.load(Ordering::Relaxed));
                        if idle.as_millis() as u64 >= bound {
                            count(&label, "tunnel_idle");
                            break;
                        }
                    }
                }
            }
            to_up.abort();
            to_client.abort();
            let _ = (&mut to_up).await;
        });
        full(StatusCode::OK, String::new())
    }

    /// (a) An absolute-form plain-HTTP request: matched to an entry by host (and port, when written), sent
    /// on over the entry's scheme with its placeholders resolved at the node's gate.
    async fn forward(self: Arc<Self>, req: Request<Incoming>) -> Response<ProxyBody> {
        let uri = req.uri().clone();
        match uri.scheme_str() {
            Some("http") => {}
            Some(other) => {
                return self.refuse(StatusCode::BAD_REQUEST, "-", "malformed_request", format!("an absolute {other}:// URL — use CONNECT for a tunnel, or write http:// and the proxy originates the entry's https"));
            }
            None => {
                return self.refuse(StatusCode::BAD_REQUEST, "-", "malformed_request", "not an absolute-form request — send it through the proxy (HTTP_PROXY), `GET http://host/path`".into());
            }
        }
        let Some(host) = uri.host().map(norm_host) else {
            return self.refuse(StatusCode::BAD_REQUEST, "-", "malformed_request", "the URL names no host".into());
        };
        let hosts = self.hosts_now();
        let candidates: Vec<&EgressHost> = hosts.iter().filter(|e| e.host == host && uri.port_u16().is_none_or(|p| p == e.port)).collect();
        let entry = match candidates.as_slice() {
            [one] => (*one).clone(),
            [] => return self.refuse(StatusCode::FORBIDDEN, &host, "host_not_listed", format!("{host}{} is not in this activation's egress hosts", uri.port_u16().map(|p| format!(":{p}")).unwrap_or_default())),
            _ => return self.refuse(StatusCode::BAD_REQUEST, &host, "malformed_request", format!("{host} is listed on more than one port — write the port in the URL")),
        };
        // Every placeholder a header carries must name a binding this activation was given.
        let mut named: Vec<String> = Vec::new();
        for v in req.headers().values() {
            for n in mycelium::credential_placeholders_in(&String::from_utf8_lossy(v.as_bytes())) {
                if !named.iter().any(|x| x == n) {
                    named.push(n.to_string());
                }
            }
        }
        if let Some(n) = named.iter().find(|n| !self.credentials.iter().any(|c| c == *n)) {
            return self.refuse(StatusCode::FORBIDDEN, &host, "credential_not_granted", format!("credential `{n}` is not one this activation's egress may name"));
        }
        let upstream = format!("{}{}", entry.url_base(), uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"));
        let method = req.method().clone();
        let head = method == Method::HEAD;
        let mut headers = req.headers().clone();
        for h in HOP_BY_HOP {
            headers.remove(*h);
        }
        // N1: a permit only while a body is being buffered — a bodiless request takes none, and the permit is
        // dropped once the body is in hand, before the upstream is asked.
        let incoming = req.into_body();
        let body = if incoming.is_end_stream() {
            Bytes::new()
        } else {
            let Ok(buffering) = Arc::clone(&self.bodies).try_acquire_owned() else {
                return self.refuse(StatusCode::SERVICE_UNAVAILABLE, &host, "body_cap", format!("{} request bodies are already being buffered", self.limits.max_buffered_bodies));
            };
            let collected = http_body_util::Limited::new(incoming, MAX_REQUEST_BODY_BYTES).collect().await;
            drop(buffering);
            match collected {
                Ok(b) => b.to_bytes(),
                Err(_) => return self.refuse(StatusCode::PAYLOAD_TOO_LARGE, &host, "body_too_large", format!("a request body over {MAX_REQUEST_BODY_BYTES} bytes, or one that failed to arrive")),
            }
        };
        let mut out = reqwest::Request::new(method, match reqwest::Url::parse(&upstream) {
            Ok(u) => u,
            Err(_) => return self.refuse(StatusCode::BAD_REQUEST, &host, "malformed_request", "the upstream URL does not parse".into()),
        });
        *out.headers_mut() = headers;
        // With a value to mask, ask for the body as it is: an encoded body cannot be checked.
        #[cfg(any(feature = "gateway", feature = "llm"))]
        if self.redactor.is_some() {
            out.headers_mut().insert(reqwest::header::ACCEPT_ENCODING, reqwest::header::HeaderValue::from_static("identity"));
        }
        if !body.is_empty() {
            *out.body_mut() = Some(body.into());
        }
        let sent = self.send(out, !named.is_empty()).await;
        let resp = match sent {
            Ok(r) => r,
            Err((status, reason, why)) => {
                if status == StatusCode::BAD_GATEWAY {
                    tracing::info!(activation = %self.label, host = %host, reason = %why, "activation egress: upstream failed");
                    return full(status, format!("mycelium egress proxy ({}): {why}\n", self.label));
                }
                return self.refuse(status, &host, reason, why);
            }
        };
        #[cfg(any(feature = "gateway", feature = "llm"))]
        let redactor = self.redactor.as_ref();
        #[cfg(not(any(feature = "gateway", feature = "llm")))]
        let redactor: Option<&NoRedactor> = None;
        // Round 3: an answer that carries no body (to a `HEAD`, or a 1xx, 204 or 304) has nothing to mask,
        // whatever its `Content-Encoding` says.
        let bodiless = head || resp.status().is_informational() || matches!(resp.status().as_u16(), 204 | 304);
        if redactor.is_some() && !bodiless && encoded(resp.headers()) {
            return self.refuse(StatusCode::BAD_GATEWAY, &host, "encoded_response", "the upstream answered with an encoded body, which cannot be checked for a credential's value".into());
        }
        self.forwarded.fetch_add(1, Ordering::Relaxed);
        count(&self.label, "forwarded");
        let mut back = Response::builder().status(resp.status().as_u16());
        if let Some(h) = back.headers_mut() {
            for (k, v) in resp.headers() {
                if !HOP_BY_HOP.contains(&k.as_str()) {
                    // Masked to the same length, so `Content-Length` stays true.
                    match redactor.and_then(|r| r.header(v)) {
                        Some(masked) => h.append(k.clone(), masked),
                        None => h.append(k.clone(), v.clone()),
                    };
                }
            }
        }
        let mut masker = redactor.map(|r| r.stream());
        // The body streams back as it arrives (a model download is not held in memory).
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
        tokio::spawn(async move {
            let mut resp = resp;
            loop {
                match resp.chunk().await {
                    Ok(Some(c)) => {
                        // A value split across chunks is masked too: the masker withholds a value's length
                        // less one until the next chunk (`RedactStream`).
                        let c = match masker.as_mut() {
                            Some(m) => Bytes::from(m.feed(&c)),
                            None => c,
                        };
                        if !c.is_empty() && tx.send(Ok(c)).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        if let Some(m) = masker.take() {
                            let rest = m.finish();
                            if !rest.is_empty() {
                                let _ = tx.send(Ok(Bytes::from(rest))).await;
                            }
                        }
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(std::io::Error::other(e))).await;
                        return;
                    }
                }
            }
        });
        back.body(ChannelBody { rx }.boxed()).unwrap_or_else(|_| full(StatusCode::BAD_GATEWAY, "an upstream response the proxy cannot relay\n".into()))
    }

    /// Send through the node's gate (a placeholder present), or on the proxy's own client when the build has
    /// no gate. `Err` is the status to answer and why — never a credential's value.
    #[cfg(any(feature = "gateway", feature = "llm"))]
    async fn send(&self, req: reqwest::Request, placeholders: bool) -> Result<reqwest::Response, (StatusCode, &'static str, String)> {
        let Some(gate) = &self.gate else {
            if placeholders {
                return Err((StatusCode::FORBIDDEN, "credential_not_authorised", "a credential placeholder, and this node has no egress credential binding".into()));
            }
            return self.send_plain(req).await;
        };
        gate.send_once(req).await.map_err(|e| match e {
            mycelium::CredentialError::Http(e) => (StatusCode::BAD_GATEWAY, "upstream_failed", format!("upstream: {}", chain(&e))),
            e @ mycelium::CredentialError::EgressDenied { .. } => (StatusCode::FORBIDDEN, "node_egress_denied", e.to_string()),
            other => (StatusCode::FORBIDDEN, "credential_not_authorised", other.to_string()),
        })
    }

    #[cfg(not(any(feature = "gateway", feature = "llm")))]
    async fn send(&self, req: reqwest::Request, placeholders: bool) -> Result<reqwest::Response, (StatusCode, &'static str, String)> {
        if placeholders {
            return Err((StatusCode::FORBIDDEN, "credential_not_authorised", "a credential placeholder, and this build attaches none (feature gateway or llm)".into()));
        }
        self.send_plain(req).await
    }

    async fn send_plain(&self, req: reqwest::Request) -> Result<reqwest::Response, (StatusCode, &'static str, String)> {
        if !self.node_egress.permits_url(req.url().as_str()) {
            return Err((StatusCode::FORBIDDEN, "node_egress_denied", format!("the node's egress allow-list does not permit {}", req.url().host_str().unwrap_or("-"))));
        }
        self.client.execute(req).await.map_err(|e| (StatusCode::BAD_GATEWAY, "upstream_failed", format!("upstream: {}", chain(&e))))
    }
}

/// Is the connection from `peer` this user's? Queued for the next batched read of the kernel's table
/// ([`PeerGate`]). `Err` names the refusal counted: `peer_uid_other` (another user), `peer_uid_unknown` (no answer
/// — refused, never assumed), `peer_check_evicted` (closed to make room, past `max_unchecked`), `peer_check_busy`
/// (no answer within the pre-auth timeout).
async fn admit_peer(inner: &Inner, sock: &tokio::net::TcpStream, peer: SocketAddr, own_uid: u32) -> Result<(), &'static str> {
    let local = sock.local_addr().map_err(|_| "peer_uid_unknown")?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    inner.peer_gate.enqueue(Waiter { local, peer, reply: tx }, inner.limits.max_unchecked, &inner.label);
    let answer = tokio::select! {
        r = rx => r.map_err(|_| "peer_check_evicted")?,
        _ = mycelium::sim_seam::sleep_ms("activation_egress.peer_check", inner.limits.preauth_timeout_ms.max(1)) => return Err("peer_check_busy"),
    };
    match answer {
        Ok(uid) if uid == own_uid => Ok(()),
        Ok(uid) => {
            tracing::info!(activation = %inner.label, %peer, uid, "activation egress: a connection from another user, closed at accept");
            Err("peer_uid_other")
        }
        Err(why) => {
            tracing::info!(activation = %inner.label, %peer, reason = %why, "activation egress: a connection whose owner cannot be read, closed at accept");
            Err("peer_uid_unknown")
        }
    }
}

/// Aborts a task when dropped (the proxy's naming task ends with its listener).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Could a value hide, encoded, in this answer's body (#644's round-2 review, N5)? Any coding but `identity`
/// in any `Content-Encoding` header, or a `Transfer-Encoding` naming anything but `chunked` — every header of
/// each name read, every comma-separated coding in each (repeated `identity` headers are no encoding; round 3).
fn encoded(h: &reqwest::header::HeaderMap) -> bool {
    let codings_other_than = |name: reqwest::header::HeaderName, allowed: &str| {
        h.get_all(name).iter().any(|v| match v.to_str() {
            Ok(text) => text.split(',').map(str::trim).any(|c| !c.is_empty() && !c.eq_ignore_ascii_case(allowed)),
            Err(_) => true,
        })
    };
    codings_other_than(reqwest::header::CONTENT_ENCODING, "identity")
        || codings_other_than(reqwest::header::TRANSFER_ENCODING, "chunked")
}

/// One accepted connection's place in the proxy's accounting (#644's round-2 review, N2, N3): it counts as
/// unauthenticated until a request presents the token, then as authenticated — against `max_connections` —
/// until the last holder drops it: the connection's task, or a `CONNECT` tunnel's.
struct Conn {
    /// 0 unauthenticated, 1 authenticated.
    phase: AtomicU8,
    inner: Arc<Inner>,
    /// Its place against `max_open`, released with the last holder.
    _open: OpenSlot,
    /// Closes an unauthenticated connection to make room for a newer one.
    evict: tokio::sync::Notify,
}

impl Conn {
    /// Count a new connection as unauthenticated; past `max_unauthenticated`, close the oldest ones.
    fn admit(inner: &Arc<Inner>, open: OpenSlot) -> Arc<Conn> {
        let conn = Arc::new(Conn { phase: AtomicU8::new(0), inner: Arc::clone(inner), _open: open, evict: tokio::sync::Notify::new() });
        let live = inner.preauth_live.fetch_add(1, Ordering::AcqRel) + 1;
        let mut excess = live.saturating_sub(inner.limits.max_unauthenticated.max(1));
        let mut q = inner.preauth_queue.lock().unwrap_or_else(|e| e.into_inner());
        while excess > 0 {
            let Some(oldest) = q.pop_front() else { break };
            if let Some(c) = oldest.upgrade()
                && !c.authenticated()
            {
                c.evict.notify_one();
                count(&inner.label, "preauth_evicted");
                excess -= 1;
            }
        }
        q.retain(|w| w.upgrade().is_some_and(|c| !c.authenticated()));
        q.push_back(Arc::downgrade(&conn));
        conn
    }

    fn authenticated(&self) -> bool {
        self.phase.load(Ordering::Acquire) == 1
    }

    /// Move to authenticated, if `max_connections` allows; false when it does not.
    fn authenticate(&self, inner: &Inner) -> bool {
        if self.authenticated() {
            return true;
        }
        let cap = inner.limits.max_connections.max(1);
        if inner.authed_live.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < cap).then_some(n + 1)).is_err() {
            return false;
        }
        if self.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            inner.preauth_live.fetch_sub(1, Ordering::AcqRel);
        } else {
            inner.authed_live.fetch_sub(1, Ordering::AcqRel);
        }
        true
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        match self.phase.load(Ordering::Acquire) {
            1 => self.inner.authed_live.fetch_sub(1, Ordering::AcqRel),
            _ => self.inner.preauth_live.fetch_sub(1, Ordering::AcqRel),
        };
    }
}

/// Stands in for `mycelium::Redactor` in a build without the gate (there is no value to mask).
#[cfg(not(any(feature = "gateway", feature = "llm")))]
struct NoRedactor;

#[cfg(not(any(feature = "gateway", feature = "llm")))]
impl NoRedactor {
    fn header(&self, _: &hyper::header::HeaderValue) -> Option<hyper::header::HeaderValue> {
        None
    }
    fn stream(&self) -> NoRedactor {
        NoRedactor
    }
    fn feed(&mut self, c: &[u8]) -> Vec<u8> {
        c.to_vec()
    }
    fn finish(self) -> Vec<u8> {
        Vec::new()
    }
}

/// A tunnel end that records when it last carried a byte, for the idle bound.
struct Idle<T> {
    io: T,
    last: Arc<AtomicU64>,
}

impl<T: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Idle<T> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = std::pin::Pin::new(&mut self.io).poll_read(cx, buf);
        if matches!(r, std::task::Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.last.store(mycelium::sim_seam::mono_now_ns(), Ordering::Relaxed);
        }
        r
    }
}

impl<T: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Idle<T> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, b: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        let r = std::pin::Pin::new(&mut self.io).poll_write(cx, b);
        if matches!(r, std::task::Poll::Ready(Ok(n)) if n > 0) {
            self.last.store(mycelium::sim_seam::mono_now_ns(), Ordering::Relaxed);
        }
        r
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// A response body fed by the task that reads the upstream.
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
}

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        self.rx.poll_recv(cx).map(|o| o.map(|r| r.map(Frame::data)))
    }
}

/// An error and its causes, `: `-joined (reqwest's own message stops at "error sending request").
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cur = c.source();
    }
    out
}

fn count(activation: &str, event: &'static str) {
    metrics::counter!("mycelium_activation_egress_total", "activation" => activation.to_string(), "event" => event).increment(1);
}

/// Standard base64 for the token's `Basic` value (the crate has no base64 dependency; this is the only use).
mod base64_lite {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(b: &[u8]) -> String {
        let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[cfg(test)]
    #[test]
    fn encodes_like_the_standard() {
        assert_eq!(encode(b"mycelium:ab"), "bXljZWxpdW06YWI=");
        assert_eq!(encode(b"ab"), "YWI=");
        assert_eq!(encode(b"abc"), "YWJj");
        assert_eq!(encode(b""), "");
    }
}

/// The guarantee a stem's activations carry about the network, for `GossipAgent::register_guarantee`
/// **before** `start()` (`mycelium-stem` registers it beside `stem.activation_confined`):
/// `stem.activation_egress` — *an activation reaches another host only through its stem's egress proxy, to
/// the hosts its allow-list names, by name*. Resolved from `units` and this platform: **enforced** when every
/// `[[activation]]` is confined by a backend that limits a connection by address (Seatbelt) and declares no
/// `connect` entry — so its only ways off the host are its egress proxy or none; **not configured**
/// otherwise, naming which (an unconfined or refused activation; a `*:<port>` entry, which reaches any
/// address on that port; a `localhost:<port>` entry, a local service that may forward anywhere — #644's
/// review, finding 4; or Landlock, which limits the proxy's port, not its address); **not applicable**
/// with no `[[activation]]`. A proxy that could not start refuses its activation (it never runs without
/// it); that is on `Stem::activation_confinement`, not here.
pub fn egress_guarantee(units: &mycelium::NodeCapabilityConfig) -> mycelium::GuaranteeDescriptor {
    use crate::activation_sandbox::{assess, Confinement};
    use mycelium::{ConfineBackend, GuaranteeDescriptor, GuaranteeKind, GuaranteeResolution as Resolution};
    let none = units.activations.is_empty();
    let unconfined = units.activations.iter().any(|a| !matches!(assess(a.confine.as_ref()), Confinement::Confined { .. }));
    let any_port = units.activations.iter().any(|a| a.confine.as_ref().is_some_and(|c| c.connect.iter().any(|e| e.starts_with("*:"))));
    // #644's review, finding 4: a local service a command may reach directly can forward it anywhere (another
    // proxy, a gateway), so a declared `localhost:<port>` is not "only through its egress proxy" either.
    let local_port = units.activations.iter().any(|a| a.confine.as_ref().is_some_and(|c| c.connect.iter().any(|e| e.starts_with("localhost:"))));
    let by_port_only = units.activations.iter().any(|a| {
        a.confine.as_ref().is_some_and(|c| c.egress.is_some())
            && matches!(assess(a.confine.as_ref()), Confinement::Confined { backend: ConfineBackend::Landlock })
    });
    GuaranteeDescriptor::new(
        "stem.activation_egress",
        1,
        "stem",
        GuaranteeKind::NodeEnforced,
        "every `[[activation]]` command and probe reaches another host only through its stem's egress proxy on loopback — to the `https://` or `http://` hosts its `[activation.confine.egress]` allow-list names, by name, under the node's egress allow-list, the list replaceable while the stem runs — or not at all; a credential is attached by the node's egress gate at the proxy, only where its binding authorises and only for a binding the activation names, its value is never in the command's environment (cleared but for PATH, HOME, USER, LOGNAME, LANG, LC_ALL, LC_CTYPE, TZ, TMPDIR, the declared `env` and the proxy's address) nor — as checked when the stem starts — in a file its profile lets it read (a file moved or linked there afterwards is not seen), and an echo of its bytes is masked in every answer relayed back",
        "`[activation.confine]` on every `[[activation]]` with no `connect` entry (its network through `[activation.confine.egress]`, or none); Seatbelt (`sandbox-exec`, macOS) — Landlock limits a TCP port, not an address",
        &["activation_egress::EgressProxy::start", "activation_sandbox::prepare_with", "activation::hook_with_egress", "EgressCredentials::send_once"],
        "docs/reference/unit-file.md",
        move |_| none.then_some("this unit declares no [[activation]]"),
        move |_| {
            if unconfined {
                Resolution::NotConfigured { missing: "[activation.confine] on every [[activation]] that this node can apply — an unconfined activation reaches any host with the stem's privileges" }
            } else if local_port {
                Resolution::NotConfigured { missing: "no `localhost:<port>` in any [activation.confine] connect — a local service the command reaches directly may forward it to any host; reach it through [activation.confine.egress] as http://localhost:<port>" }
            } else if any_port {
                Resolution::NotConfigured { missing: "no `*:<port>` in any [activation.confine] connect — such an entry reaches any address on that port, not a named host; name the host in [activation.confine.egress]" }
            } else if by_port_only {
                Resolution::NotConfigured { missing: "a confinement backend that limits a connection by address (Seatbelt) — under Landlock the command can reach the egress proxy's port on any address, so the hostname limit holds only for what the proxy carries" }
            } else {
                Resolution::Enforced
            }
        },
    )
}

#[cfg(test)]
#[path = "activation_egress_tests.rs"]
mod tests;
