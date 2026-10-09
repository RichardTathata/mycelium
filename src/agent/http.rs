//! Embedded HTTP server — Layer 3 + Layer 4 gateway.
//!
//! ## Library-level endpoints
//! Public (the M16 edge criterion — what a probe, balancer or scraper needs without a credential):
//! - `GET  /health`                — liveness probe
//! - `GET  /ready`                 — readiness probe (startup complete → serving; caps gossip independently)
//! - `GET  /stats`                 — KV store metrics (node_id, store_entries, dropped_frames, task_count)
//! - `GET  /metrics`               — Prometheus exposition
//! - `GET  /bulk/{corr_id}`        — staged bulk payload; the per-call random nonce *is* the credential
//!
//! Behind the gateway bearer (and scope) boundary whenever a token model is configured — open only
//! when none is, exactly like `/gateway/*` (since 2026-09-05; before, these answered with no token):
//! - `GET  /consensus/{*slot}`      — inspect committed value + ballot for a consensus slot (`consensus:read`)
//! - `GET  /signals/{kind}`        — SSE stream of admitted signals (`mesh:read`)
//! - `POST /mcp`                   — JSON-RPC 2.0 MCP protocol bridge (`mcp:invoke`)
//!
//! ## Language-bridge gateway endpoints (`/gateway/*`)
//! These endpoints let Python/TypeScript agents participate in the mesh
//! without a Rust dependency. The gateway is the HTTP sidecar described in
//! the Layer 4 architecture.
//!
//! - `POST   /gateway/capability/advertise`    — advertise a capability; returns handle_id
//! - `POST   /gateway/capability/{handle_id}/heartbeat` — renew a leased advertisement
//! - `DELETE /gateway/capability/{handle_id}`  — retract (tombstone) a capability
//! - `GET    /gateway/capability/resolve`      — filter-match with optional caller_id scoping
//! - `POST   /gateway/signal/emit`             — fire a signal into the mesh
//! - `GET    /gateway/signal/sse/{kind}`       — SSE stream for a signal kind
//! - `GET    /gateway/demand`                  — demand pressure for a capability filter
//! - `POST   /gateway/rpc/call`               — blocking RPC call to a named node
//! - `GET    /gateway/rpc/serve/{kind}`        — SSE stream of incoming RPC requests
//! - `POST   /gateway/rpc/respond`             — send reply to an in-flight RPC request
//! - `POST   /gateway/scatter`                 — scatter-gather RPC to multiple targets
//! - `GET    /gateway/kv?key=K`                — read a KV key
//! - `POST   /gateway/kv`                      — write a KV key
//! - `DELETE /gateway/kv?key=K`                — delete (tombstone) a KV key
//! - `GET    /gateway/kv/keys?prefix=P`        — list live keys (optionally filtered)
//! - `POST   /gateway/kv/quorum`               — write, then ask peers who holds it (item 1 PR 4b)
//! - `GET    /gateway/mailbox/{kind}`          — SSE stream of mailbox events for this node
//! - `POST   /gateway/mailbox/deliver`         — deliver an event to a target's mailbox
//! - `GET    /gateway/shard/{ns}/{name}?key=K` — deterministic shard owner for a key
//! - `POST   /gateway/shard/emit`              — emit signal to consistent-hash owner
//! - `POST   /gateway/consensus/cross_group_propose` — multi-group independent-quorum proposal
//!
//! Started when `GossipConfig::http_port` is `Some(port)`. Shuts down cleanly
//! when the agent's broadcast shutdown signal fires.

use axum::{
    Router,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response, Sse},
    response::sse::{Event, KeepAlive},
    routing::{delete, get, post},
};
#[cfg(feature = "compliance")]
use axum::extract::MatchedPath;
use bytes::{BufMut, Bytes, BytesMut};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{oneshot, watch, Notify};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt as _;
use tracing::{info, warn};

use crate::LogEntry;
#[cfg(feature = "consensus")]
#[cfg(feature = "consensus")]
use super::overlay_consistent::LockGuard;

use super::TaskCtx;
use super::gateway_caller::{self, GatewayDispatchError, ResolvedPrincipal};
use axum::Extension;

/// Shared state passed to every HTTP handler.
struct HttpCtx {
    agent_ctx:       Arc<TaskCtx>,
    /// Capability handle table for the language gateway.
    /// Key: opaque handle_id string returned to the caller.
    /// Value: dropping it (map removal) tombstones the capability.
    gateway_caps:    Arc<Mutex<HashMap<String, GatewayCapHandle>>>,
    /// Distributed lock guards held on behalf of HTTP clients.
    /// Key: opaque guard_id returned to the caller.
    /// Drop-on-remove tombstones `lock/{name}` in the gossip KV.
    #[cfg(feature = "consensus")]
    lock_guards:     Arc<Mutex<HashMap<String, LockGuard>>>,
    /// Shutdown receiver used when spawning gateway advertisement tasks.
    shutdown_rx:     watch::Receiver<bool>,
    /// Prometheus scrape handle (only present when the `metrics` feature is enabled).
    #[cfg(feature = "metrics")]
    prometheus:      metrics_exporter_prometheus::PrometheusHandle,
    /// OIDC verifier (WS4) — present when `GossipConfig::oidc` is set. Validates
    /// JWT bearers against the IdP JWKS and maps groups to gateway scopes.
    #[cfg(feature = "compliance")]
    oidc:            Option<Arc<super::oidc::OidcVerifier>>,
    /// Requests streamed to SDK agents and not yet answered: `(sender, nonce, principal)` → the
    /// HLC decision time (ms) it was streamed at. `gw_rpc_serve` records one when it streams a
    /// request to the principal that opened the stream; `gw_rpc_respond` answers only a nonce
    /// recorded for the responding principal and removes it. Bounded: entries older than
    /// `SERVED_RPC_TTL_MS` (the gateway's own RPC ceiling) are evicted on insert, and past
    /// `SERVED_RPC_CAP` the oldest go. Lock-order row 55 — leaf, µs.
    served_rpcs:     Mutex<HashMap<(crate::node_id::NodeId, u64, String), u64>>,
}

/// Longest a streamed request stays answerable: the gateway's own RPC ceiling (`rpc/call` clamps
/// `timeout_secs` to 300), after which the caller has timed out and the reply has no one to reach.
const SERVED_RPC_TTL_MS: u64 = 300_000;
/// Hard cap on unanswered streamed requests remembered at once; past it the oldest are evicted.
const SERVED_RPC_CAP: usize = 65_536;

/// `gw_rpc_serve`'s record: this `principal` was handed the request `(sender, nonce)` now.
fn record_served_rpc(ctx: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64, principal: &str) {
    let now = ctx.agent_ctx.hlc.decision_now_ms();
    let mut served = ctx.served_rpcs.lock().unwrap_or_else(|e| e.into_inner());
    served.retain(|_, at| now.saturating_sub(*at) <= SERVED_RPC_TTL_MS);
    if served.len() >= SERVED_RPC_CAP
        && let Some(oldest) = served.iter().min_by_key(|(_, at)| **at).map(|(k, _)| k.clone())
    {
        served.remove(&oldest);
    }
    served.insert((sender.clone(), nonce, principal.to_string()), now);
}

/// `gw_rpc_respond`'s check: was `(sender, nonce)` streamed to `principal` and not yet answered?
/// Consumes the record — a request is answered once.
fn take_served_rpc(ctx: &HttpCtx, sender: &crate::node_id::NodeId, nonce: u64, principal: &str) -> bool {
    let now = ctx.agent_ctx.hlc.decision_now_ms();
    let mut served = ctx.served_rpcs.lock().unwrap_or_else(|e| e.into_inner());
    match served.remove(&(sender.clone(), nonce, principal.to_string())) {
        Some(at) => now.saturating_sub(at) <= SERVED_RPC_TTL_MS,
        None => false,
    }
}

/// One gateway-advertised capability. In-process advertisers get liveness
/// coupling for free (their refresh loop dies with them); a *bridged*
/// advertiser's refresh loop runs in this node, so its advert would outlive
/// the client process — the lease closes that gap.
struct GatewayCapHandle {
    /// Dropped on map removal → the persist task exits and tombstones the
    /// KV entry. Never read; its `Drop` is the retraction mechanism.
    _cancel:   oneshot::Sender<()>,
    /// `Some` when the advertiser passed `lease_secs`: each notification
    /// restarts the lease watchdog's window; a full window with no beat
    /// retracts the advert exactly as `DELETE /gateway/capability/{id}` would.
    heartbeat: Option<Arc<Notify>>,
    /// Whatever else this handle keeps alive (`POST /gateway/units/declare`: the unit's capability
    /// registrations, requirement and group handles). Dropped with the entry, which retracts them.
    _held:     Option<Box<dyn std::any::Any + Send>>,
}

/// Returns the process-wide Prometheus scrape handle, installing the recorder
/// the first time it is called. Safe to call from multiple agents in the same
/// process (e.g. in tests) — subsequent calls return a clone of the same handle.
#[cfg(feature = "metrics")]
fn prometheus_handle(cluster_name: Option<&str>) -> metrics_exporter_prometheus::PrometheusHandle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    // The recorder is process-wide (installed once). When a `cluster_name` is configured it becomes
    // a `cluster` *global label* on every series, so one Prometheus can disambiguate environments
    // (WS-ops cluster-name). One-agent-per-process is the production case; if several agents share a
    // process (tests) the first one's label wins — harmless.
    HANDLE.get_or_init(|| {
        let mut builder = metrics_exporter_prometheus::PrometheusBuilder::new();
        if let Some(name) = cluster_name.filter(|n| !n.is_empty()) {
            builder = builder.add_global_label("cluster", name);
        }
        builder.install_recorder().expect("Prometheus recorder install failed")
    }).clone()
}

/// Starts the axum HTTP server on `addr`. Returns when the agent shuts down
/// (shutdown_rx fires) or if the listener fails to bind.
///
/// `extra_routes` is an optional `Router<()>` (state already attached by the
/// caller) that is merged into the library router so application-level
/// handlers share the same port without a second TCP listener.
/// The gateway's listener and, when configured, its TLS server config — everything that can fail
/// before a request is served, prepared **inside `start()`** so the failure is `start()`'s
/// (`gw.tls_runtime`, plan §8). Before this, the bind and the TLS setup ran in the spawned task: a
/// busy port or an unreadable certificate was logged as "HTTP server exited" while the node started
/// and reported ready without a gateway.
pub(super) struct PreparedGateway {
    listener: tokio::net::TcpListener,
    #[cfg(feature = "tls")]
    server_config: Option<std::sync::Arc<rustls::ServerConfig>>,
}

/// Bind the gateway and resolve its TLS material; an error here fails `start()`.
pub(super) fn prepare_gateway(addr: SocketAddr, ctx: &Arc<TaskCtx>) -> Result<PreparedGateway, std::io::Error> {
    // SO_REUSEADDR, like the gossip listener (`tasks::new_listener`): without it, a fast
    // process restart on a fixed port can hit AddrInUse from lingering TIME_WAIT tuples
    // (server-side-closed HTTP connections linger ~60 s) and panic the whole node — seen as
    // scenario 03's restart killing node-a on a CPU-starved hosted runner (#156 gate, PR
    // #159 run). Plain `TcpListener::bind` sets no socket options; do it explicitly.
    let sock = if addr.is_ipv6() {
        tokio::net::TcpSocket::new_v6()
    } else {
        tokio::net::TcpSocket::new_v4()
    }?;
    sock.set_reuseaddr(true)?;
    sock.bind(addr).map_err(|e| std::io::Error::new(e.kind(), format!("bind {addr}: {e}")))?;
    let listener = sock.listen(1024)?;
    #[cfg(feature = "tls")]
    let server_config = match ctx.config.gateway_tls.clone() {
        Some(gw_tls) => Some(build_gateway_server_config(ctx, &gw_tls)?),
        None => None,
    };
    #[cfg(not(feature = "tls"))]
    let _ = ctx;
    Ok(PreparedGateway {
        listener,
        #[cfg(feature = "tls")]
        server_config,
    })
}

pub(super) async fn run_http_server(
    prepared:     PreparedGateway,
    ctx:          Arc<TaskCtx>,
    shutdown_rx:  watch::Receiver<bool>,
    extra_routes: Option<axum::Router>,
) -> Result<(), std::io::Error> {
    #[cfg(feature = "metrics")]
    let prometheus = prometheus_handle(ctx.config.cluster_name.as_deref());

    #[cfg(feature = "compliance")]
    let oidc = ctx
        .config
        .oidc
        .clone()
        .map(|c| Arc::new(super::oidc::OidcVerifier::new(c, ctx.config.egress.clone())));

    let state = Arc::new(HttpCtx {
        agent_ctx:    ctx,
        gateway_caps: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "consensus")]
        lock_guards:  Arc::new(Mutex::new(HashMap::new())),
        shutdown_rx:  shutdown_rx.clone(),
        #[cfg(feature = "metrics")]
        prometheus,
        #[cfg(feature = "compliance")]
        oidc,
        served_rpcs:  Mutex::new(HashMap::new()),
    });

    // ── Language-bridge gateway routes (optionally auth-protected) ────────────
    // Nested under /gateway so the auth middleware applies to all of them; the node-level
    // `/mcp`, `/signals/{kind}`, `/consensus/{*slot}` get the same layer below, leaving only
    // /health, /ready, /stats, /metrics and the nonce-capability /bulk/{id} public.
    // route_layer is applied once at the end so all routes (including
    // cfg-gated llm routes) are covered by a single middleware instance.
    let gateway = Router::new()
        .route("/capability/advertise",   post(gw_cap_advertise))
        .route("/capability/{handle_id}", delete(gw_cap_drop))
        .route("/capability/{handle_id}/heartbeat", post(gw_cap_heartbeat))
        .route("/capability/resolve",     get(gw_cap_resolve))
        .route("/units/declare",          post(gw_units_declare))
        .route("/signal/emit",            post(gw_signal_emit))
        .route("/signal/sse/{kind}",      get(gw_signal_sse))
        .route("/demand",                 get(gw_demand))
        .route("/rpc/call",               post(gw_rpc_call))
        .route("/rpc/serve/{kind}",       get(gw_rpc_serve))
        .route("/rpc/respond",            post(gw_rpc_respond))
        .route("/scatter",                post(gw_scatter))
        .route("/kv",                     get(gw_kv_get).post(gw_kv_set).delete(gw_kv_delete))
        .route("/kv/keys",                get(gw_kv_keys))
        .route("/kv/quorum",              post(gw_kv_quorum))
        .route("/mailbox/deliver",        post(gw_mailbox_deliver))
        .route("/mailbox/{kind}",         get(gw_mailbox_subscribe))
        // ── Overlay: ordered log ──────────────────────────────────────────
        .route("/overlay/log/append",             post(gw_overlay_log_append))
        .route("/overlay/log/scan",               get(gw_overlay_log_scan))
        .route("/overlay/log/compact",            post(gw_overlay_log_compact))
        .route("/overlay/log/subscribe",          get(gw_overlay_log_subscribe))
        // ── Overlay: reliable delivery ────────────────────────────────────
        .route("/overlay/emit_reliable",          post(gw_overlay_emit_reliable))
        // ── Cluster sharding ──────────────────────────────────────────────
        .route("/shard/{ns}/{name}",               get(gw_shard_owner))
        .route("/shard/emit",                     post(gw_shard_emit))
        // ── WS-C governance: management = intent + local reconcile ─────────
        .route("/govern",                         get(gw_govern_snapshot))
        .route("/govern/tuning",                  post(gw_govern_tuning))
        .route("/govern/timing",                  post(gw_govern_timing))
        .route("/govern/membership",              post(gw_govern_membership))
        .route("/govern/topology-override",       post(gw_govern_topology_override))
        .route("/mesh/group",                     get(gw_group_members)
                                                  .post(gw_group_join)
                                                  .delete(gw_group_leave))
        .route("/govern/profile",                 post(gw_govern_profile))
        .route("/govern/group",                   post(gw_govern_group_join).delete(gw_govern_group_leave))
        // ── Legible Emergence Phase 2: the relational fleet snapshot (localize) ─
        .route("/fleet",                          get(gw_fleet_snapshot))
        // ── Legible Emergence Phase 3: the causal event ring (explain) ─────────
        .route("/explain",                        get(gw_explain))
        // ── Legible Emergence Phase 4: the fleet narrative (why / diagnose) ────
        .route("/diagnose",                       get(gw_diagnose))
        .route("/guarantees",                     get(gw_guarantees));

    // ── Consensus + the consistency/lock/election overlays built on it ────────
    // (v2 M2 feature gate). The ordered-log and reliable-delivery overlays above
    // are KV/anti-entropy based, not consensus, so they stay unconditional.
    #[cfg(feature = "consensus")]
    let gateway = gateway
        .route("/overlay/consistent/set",         post(gw_overlay_consistent_set))
        .route("/overlay/consistent/get",         get(gw_overlay_consistent_get))
        .route("/overlay/lock/acquire",           post(gw_overlay_lock_acquire))
        .route("/overlay/lock/{guard_id}",         delete(gw_overlay_lock_release))
        .route("/overlay/elect",                  post(gw_overlay_elect))
        // log/group/subscribe uses the distributed-lock claim (consensus overlay)
        .route("/overlay/log/group/subscribe",    get(gw_overlay_log_group_subscribe))
        .route("/consensus/cross_group_propose",  post(gw_cross_group_propose));

    #[cfg(feature = "llm")]
    let gateway = gateway
        .route("/prompts",             get(gw_prompts_list))
        .route("/prompts/{ns}/{name}", get(gw_prompt_get).put(gw_prompt_put).delete(gw_prompt_delete))
        .route("/llm/call",            post(gw_llm_call))
        .route("/llm/stream",          post(gw_llm_stream));

    // ── Federation, the consumer side (item 2 row 11) ─────────────────────────
    // The provider side is merged at the root (`/federation/catalog`, credential-authenticated);
    // these are the local client's verbs, and they sit inside `/gateway` precisely so they get the
    // bearer-then-scope layer every other local verb gets. `federation:read` / `federation:invoke`.
    #[cfg(feature = "tls")]
    let gateway = gateway
        .route("/federation/domain",           get(gw_federation_domain))
        .route("/federation/partners",         get(gw_federation_partners))
        .route("/federation/catalog/{domain}", get(gw_federation_catalog))
        .route("/federation/connect",          post(gw_federation_connect))
        .route("/federation/call",             post(gw_federation_call));

    // WS2 audit trail query + verification (compliance feature).
    #[cfg(feature = "compliance")]
    let gateway = gateway.route("/audit", get(gw_audit));

    // WS-D / D2: revocation transparency — Merkle heads + client-checkable inclusion proofs.
    #[cfg(feature = "compliance")]
    let gateway = gateway.route("/transparency", get(gw_transparency));

    // SOC 2 WS-B: operator-facing key revocation (compromise remediation). The crypto +
    // cluster-wide exclusion already exist; this is the missing trigger surface.
    #[cfg(feature = "compliance")]
    let gateway = gateway.route("/identity/revoke", post(gw_identity_revoke));

    // Apply auth middleware to all gateway routes in one shot.
    let gateway = gateway
        .route_layer(middleware::from_fn_with_state(Arc::clone(&state), gateway_auth));

    // ── Main router ───────────────────────────────────────────────────────────
    let app = Router::new()
        // Library endpoints — public by the M16 edge criterion: what a probe, load balancer or
        // scraper needs with no credential. This list and /bulk are the WHOLE public surface
        // (docs/operations/rbac.md) — anything else goes behind `gateway_auth`.
        .route("/health",               get(health_handler))
        .route("/ready",                get(ready_handler))
        .route("/stats",                get(stats_handler))
        .route("/metrics",              get(metrics_handler))
        // Bulk staging is a capability URL: the 64-bit random per-call nonce (`bulk.rs`, dropped
        // when the call completes) is the credential. The serving PEER fetches it node-to-node
        // with no shared bearer, so it cannot sit behind the token layer.
        .route("/bulk/{corr_id}",       get(bulk_staging_handler))
        // Gateway — auth-protected when gateway_auth_token is set
        .nest("/gateway", gateway);

    // Node-level routes behind the SAME bearer-then-scope boundary as `/gateway/*` (open when no
    // token model is configured, like the gateway). Found by external review 2026-09-05: with a
    // token set, `POST /mcp` `tools/call` still invoked any tool in the cluster **with this node's
    // identity** (provider-side `authorized_callers` sees the node, not the HTTP caller — a
    // confused deputy), `/signals/{kind}` streamed live mesh traffic and `/consensus/{*slot}`
    // disclosed committed values (lock holders) — while rbac.md and the wiki listed only
    // /health|/ready|/stats|/metrics as public. Scope entries live in `required_scope`
    // (`mcp:invoke`, `mesh:read`, `consensus:read`); the same layer instance the gateway uses.
    let gated = Router::new()
        .route("/signals/{kind}",       get(signal_sse_handler))
        .route("/mcp",                  post(mcp_handler));
    #[cfg(feature = "consensus")]
    let gated = gated.route("/consensus/{*slot}", get(consensus_slot_handler));
    let gated = gated
        .route_layer(middleware::from_fn_with_state(Arc::clone(&state), gateway_auth));
    let app = app.merge(gated);

    let app = app.with_state(Arc::clone(&state));

    // Application routes merged in via `with_http_routes` (companion crates, A2A). Their
    // `/gateway/…` paths must sit behind the SAME auth boundary as the library's own — a
    // merged router is not inside the `.nest("/gateway", …)` above, so without this layer a
    // companion's `/gateway/reason/route` or `/gateway/wiki/ingest` answered without a bearer
    // while `/gateway/kv` demanded one (found 2026-09-04 while adding the OpenAI façade; the
    // companion docs had claimed coverage all along). Prefix-guarded: an application's
    // deliberately public path (`/.well-known/agent.json`, `/a2a`) stays open.
    let app = if let Some(extra) = extra_routes {
        let extra = extra.route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            gateway_auth_if_gateway_path,
        ));
        app.merge(extra)
    } else {
        app
    };

    let listener = prepared.listener;

    // Native gateway TLS (SOC 2 WS-A): when GossipConfig::gateway_tls is set, serve HTTPS
    // over a hand-rolled tokio-rustls accept loop; otherwise the plain axum::serve path. The
    // material was resolved in `prepare_gateway`, inside `start()`.
    #[cfg(feature = "tls")]
    if let Some(server_config) = prepared.server_config {
        info!(addr = %listener.local_addr().unwrap(), "HTTPS gateway listening (native TLS)");
        return serve_https(listener, app, server_config, shutdown_rx).await;
    }

    info!(addr = %listener.local_addr().unwrap(), "HTTP server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Resolve the gateway's rustls `ServerConfig` from `GatewayTlsConfig`: an operator-supplied
/// cert/key PEM pair, or (both `None`) the node identity cert built with no client-cert demand.
#[cfg(feature = "tls")]
fn build_gateway_server_config(
    ctx:    &Arc<TaskCtx>,
    gw_tls: &crate::config::GatewayTlsConfig,
) -> Result<std::sync::Arc<rustls::ServerConfig>, std::io::Error> {
    match (&gw_tls.cert_pem_path, &gw_tls.key_pem_path) {
        (Some(cert_path), Some(key_path)) => {
            let cert_pem = std::fs::read_to_string(cert_path)?;
            let key_pem  = std::fs::read_to_string(key_path)?;
            mycelium_core::tls::gateway_server_config_from_pem(&cert_pem, &key_pem)
                .map(std::sync::Arc::new)
                .map_err(|e| std::io::Error::other(e.to_string()))
        }
        (None, None) => {
            // Reuse the node identity cert (requires GossipConfig::tls).
            let tls = ctx.tls.get().ok_or_else(|| std::io::Error::other(
                "gateway_tls reuses the node identity cert but GossipConfig::tls is not set; \
                 either set tls, or provide cert_pem_path + key_pem_path"))?;
            Ok(tls.gateway_server_config())
        }
        _ => Err(std::io::Error::other(
            "gateway_tls: cert_pem_path and key_pem_path must both be set or both unset")),
    }
}

/// Hand-rolled TLS accept loop: terminate rustls per connection, then serve the axum app over
/// the TLS stream via hyper-util (both already in-tree via axum — no new compiled crate).
/// Stops accepting on shutdown; in-flight connections drain on their own tasks.
#[cfg(feature = "tls")]
async fn serve_https(
    listener:      tokio::net::TcpListener,
    app:           axum::Router,
    server_config: std::sync::Arc<rustls::ServerConfig>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), std::io::Error> {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;
    use hyper_util::service::TowerToHyperService;

    let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
    loop {
        tokio::select! {
            _ = shutdown_rx.wait_for(|v| *v) => break,
            accepted = listener.accept() => {
                let (tcp, _peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e)   => { warn!("gateway TLS accept error: {e}"); continue; }
                };
                let acceptor = acceptor.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp).await {
                        Ok(s)  => s,
                        Err(_) => return, // handshake failure (no client cert needed) — drop quietly
                    };
                    let io  = TokioIo::new(tls_stream);
                    let svc = TowerToHyperService::new(app);
                    let _ = ConnBuilder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(io, svc)
                        .await;
                });
            }
        }
    }
    Ok(())
}

async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

/// Axum middleware applied to every `/gateway/**` route.
///
/// Two layers, the second feature-gated:
///
/// 1. **Authentication** (always): when `gateway_auth_token` is set, or
///    (compliance) any `gateway_scoped_tokens` are configured, every gateway
///    request must carry a valid `Authorization: Bearer <token>`. With neither
///    set the gateway is open (loopback-only deployments). `/health`, `/ready`,
///    `/stats`, `/metrics`, `/bulk/{id}` and the descriptor path stay public
///    regardless; the node-level `/mcp`, `/signals/{kind}` and `/consensus/{*slot}`
///    carry this same layer (2026-09-05).
///
/// 2. **OAuth2 scope authorization** (`compliance` feature): the presented
///    token resolves to a scope grant — `gateway_auth_token` ⇒ the `"*"`
///    wildcard (full access, unchanged behaviour), or a `gateway_scoped_tokens`
///    entry ⇒ its scopes. The matched route requires a `resource:verb` scope
///    ([`required_scope`]); the request is admitted only if the grant holds it
///    or `"*"`. Deny-by-default: an unmapped gateway route requires `admin`.
async fn gateway_auth(
    State(ctx): State<Arc<HttpCtx>>,
    request: Request,
    next: Next,
) -> Response {
    let cfg = &ctx.agent_ctx.config;
    let legacy = cfg.gateway_auth_token.as_deref();

    // **Every** token table counts, not only the positional one. `gateway_named_tokens` was added
    // in 2.10.0 and `resolve_token` has honoured it since, but this predicate did not — so a
    // deployment whose *only* credential model was named tokens (the model the config docs tell an
    // operator to prefer) fell into the open-gateway branch below: no bearer required, and
    // `open_gateway_scopes` granting each route exactly the scope it asks for. The tokens worked,
    // which is what hid it: presenting one was admitted, and so was presenting nothing.
    #[cfg(feature = "compliance")]
    let have_scoped = !cfg.gateway_scoped_tokens.is_empty() || !cfg.gateway_named_tokens.is_empty();
    #[cfg(not(feature = "compliance"))]
    let have_scoped = false;

    #[cfg(feature = "compliance")]
    let have_oidc = ctx.oidc.is_some();
    #[cfg(not(feature = "compliance"))]
    let have_oidc = false;

    // The scope this route requires (compliance) — also the authority a caller context is
    // granted for the request (item 7: never more than the credential holds, never more than
    // the route needs).
    #[cfg(feature = "compliance")]
    let required_scope_for_route: &'static str = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| required_scope(request.method(), m.as_str()))
        .unwrap_or("admin");
    #[cfg(feature = "compliance")]
    let required: Option<&'static str> = Some(required_scope_for_route);
    #[cfg(not(feature = "compliance"))]
    let required: Option<&'static str> = None;

    // Open gateway: no token model and no OIDC configured. The caller context still exists —
    // principal `anonymous`, granted what the route needs (compliance) or everything (no scope
    // model exists to intersect with) — so a provider can tell a gateway client from the node.
    if legacy.is_none() && !have_scoped && !have_oidc {
        let mut request = request;
        request.extensions_mut().insert(ResolvedPrincipal::anonymous(open_gateway_scopes(required)));
        return next.run(request).await;
    }

    let presented = request.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(presented) = presented else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "authentication required"})),
        ).into_response();
    };

    let Some((principal, scopes)) = resolve_bearer(&ctx, presented).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "authentication required"})),
        ).into_response();
    };

    #[cfg(feature = "compliance")]
    if !scope_admits(&scopes, required_scope_for_route) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "insufficient scope", "required_scope": required_scope_for_route})),
        ).into_response();
    }

    // Item 7: the resolved principal and the authority granted for this request travel to the
    // dispatch site as a request extension. Constructed here and nowhere else.
    let granted = match required {
        Some(_) => gateway_caller::granted_scopes(&scopes, required),
        // Without compliance the token is authenticated but not scope-gated: what it holds.
        None => scopes,
    };
    let mut request = request;
    request.extensions_mut().insert(ResolvedPrincipal { principal, scopes: granted });

    next.run(request).await
}

/// The authority an open gateway grants a request: the route's scope under `compliance`
/// (there is a scope model, and this is what the request needed); everything otherwise (no
/// scope model exists — the deployment has decided everyone on the port is trusted).
fn open_gateway_scopes(required: Option<&'static str>) -> Vec<String> {
    match required {
        Some(r) => vec![r.to_string()],
        None => vec!["*".to_string()],
    }
}

/// Resolve a presented bearer to `(principal, scopes)`: OIDC first (a JWT from the IdP →
/// `oidc:{subject}` + groups→scopes), then the static token table. A JWT that fails OIDC
/// validation won't match a static token either, so it correctly ends in `None` (→ 401).
async fn resolve_bearer(ctx: &HttpCtx, presented: &str) -> Option<(String, Vec<String>)> {
    #[cfg(feature = "compliance")]
    if let Some(verifier) = &ctx.oidc
        && let Some(principal) = verifier.verify(presented).await
    {
        return Some((gateway_caller::oidc_principal(verifier.issuer(), &principal.subject), principal.scopes));
    }
    resolve_token(&ctx.agent_ctx.config, &gateway_identity_issuer(&ctx.agent_ctx), presented)
}

/// The issuer that qualifies this gateway's local principals: `gateway_identity_issuer`, or this
/// node's id (item 7, review finding 2 — a token's list position is a gateway-local name, never a
/// domain-wide identity).
fn gateway_identity_issuer(ctx: &TaskCtx) -> String {
    ctx.config.gateway_identity_issuer.clone().unwrap_or_else(|| ctx.node_id.to_string())
}

/// Optional authentication for `POST /a2a` (item 7). The route is public by design (an A2A
/// peer needs no Mycelium credential), but the caller context must still be *constructed by
/// this layer*: a valid bearer resolves to its principal; no bearer is `anonymous`; a presented
/// but unrecognised bearer is refused (never silently downgraded to anonymous). No scope is
/// required, so none is granted.
async fn a2a_optional_auth(ctx: Arc<HttpCtx>, mut request: Request, next: Next) -> Response {
    // Item 2 PR 8: a federation credential, if presented, is the caller's identity — authenticated
    // here, authorised for its export in the handler. Present-and-refused is a refusal, never
    // anonymous (see `federation_http`).
    #[cfg(feature = "tls")]
    match super::federation_http::authenticate_presented(&ctx.agent_ctx, request.headers()) {
        Ok(None) => {}
        Ok(Some(identity)) => {
            super::federation_http::insert_identity(&mut request, identity);
            return next.run(request).await;
        }
        Err(response) => return *response,
    }
    let presented = request.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let resolved = match presented {
        None => ResolvedPrincipal::anonymous(Vec::new()),
        Some(token) => match resolve_bearer(&ctx, &token).await {
            Some((principal, _held)) => ResolvedPrincipal { principal, scopes: Vec::new() },
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "authentication required"})),
                ).into_response();
            }
        },
    };
    request.extensions_mut().insert(resolved);
    next.run(request).await
}

/// [`gateway_auth`] for routes merged via `with_http_routes`: applied only when the request
/// path is under `/gateway/`, so a companion's gateway routes get the library's auth
/// boundary (bearer, then scope — an unmapped `/gateway/` route is deny-by-default `admin`)
/// while its intentionally public paths pass straight through.
async fn gateway_auth_if_gateway_path(
    State(ctx): State<Arc<HttpCtx>>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/gateway/") {
        gateway_auth(State(ctx), request, next).await
    } else if request.uri().path() == "/a2a" {
        a2a_optional_auth(ctx, request, next).await
    } else if request.uri().path().starts_with("/federation/") {
        federation_path_auth(ctx, request, next).await
    } else {
        next.run(request).await
    }
}

/// `/federation/*` (item 2 PR 8): a credential is required. Without `tls` no federation route is
/// ever mounted, so the branch falls through to the router's 404.
#[cfg(feature = "tls")]
async fn federation_path_auth(ctx: Arc<HttpCtx>, request: Request, next: Next) -> Response {
    super::federation_http::federation_auth(Arc::clone(&ctx.agent_ctx), request, next).await
}

#[cfg(not(feature = "tls"))]
async fn federation_path_auth(_ctx: Arc<HttpCtx>, request: Request, next: Next) -> Response {
    next.run(request).await
}

/// Map a presented bearer token to `(principal, scopes)`, or `None` if unrecognised.
///
/// Every gateway-local principal is qualified by `issuer` (this gateway's identity issuer): the
/// legacy `gateway_auth_token` is `token:{issuer}/legacy` with scopes `["*"]` (deployments that
/// only set it behave exactly as before); a named token (`gateway_named_tokens`, `compliance`) is
/// `token:{issuer}/{name}`; a positional token (`gateway_scoped_tokens`) is `token:{issuer}/#{i}` —
/// the list position, which reordering moves, so prefer named tokens. Never the secret.
fn resolve_token(cfg: &crate::config::GossipConfig, issuer: &str, presented: &str) -> Option<(String, Vec<String>)> {
    if let Some(legacy) = cfg.gateway_auth_token.as_deref()
        && presented == legacy
    {
        return Some((gateway_caller::legacy_token_principal(issuer), vec!["*".to_string()]));
    }
    #[cfg(feature = "compliance")]
    {
        for t in &cfg.gateway_named_tokens {
            if t.token == presented {
                return Some((gateway_caller::named_token_principal(issuer, &t.name), t.scopes.clone()));
            }
        }
        for (i, t) in cfg.gateway_scoped_tokens.iter().enumerate() {
            if t.token == presented {
                return Some((gateway_caller::positional_token_principal(issuer, i), t.scopes.clone()));
            }
        }
    }
    #[cfg(not(feature = "compliance"))]
    let _ = issuer;
    None
}

/// True if `scopes` grants `required` (exact match or the `"*"` wildcard).
#[cfg(feature = "compliance")]
fn scope_admits(scopes: &[String], required: &str) -> bool {
    scopes.iter().any(|s| s == "*" || s == required)
}

/// The OAuth2 scope a gateway route requires, keyed on its matched-path pattern
/// and method. This is the gateway ACL policy table; deny-by-default — any route
/// not listed requires `admin`. Scopes are coarse `resource:verb` families so the
/// vocabulary stays small (`kv`, `cap`, `mesh`, `consensus`, `llm` × `read`/`write`,
/// plus `llm:invoke`).
#[cfg(feature = "compliance")]
fn required_scope(method: &axum::http::Method, matched_path: &str) -> &'static str {
    use axum::http::Method;
    let read = method == Method::GET;
    match matched_path {
        // KV
        "/gateway/kv"          => if read { "kv:read" } else { "kv:write" },
        "/gateway/kv/keys"     => "kv:read",
        "/gateway/kv/quorum"   => "kv:write",
        // Capabilities
        "/gateway/capability/advertise"   => "cap:write",
        "/gateway/capability/{handle_id}" => "cap:write",
        "/gateway/capability/{handle_id}/heartbeat" => "cap:write",
        "/gateway/capability/resolve"     => "cap:read",
        // Q2: an SDK agent's unit file — capabilities, requirements, groups — under one handle.
        "/gateway/units/declare"          => "cap:write",
        "/gateway/shard/{ns}/{name}"      => "cap:read",
        // Layer II mesh messaging
        "/gateway/signal/emit"     => "mesh:write",
        "/gateway/signal/sse/{kind}" => "mesh:read",
        "/gateway/demand"          => "mesh:read",
        "/gateway/rpc/call"        => "mesh:write",
        // Closure plan C1: serving is its own scope, so an agent that serves skills needs no power
        // to *call* (`mesh:write` also opens `rpc/call`). The 2.15.0 window that admitted `mesh:read`/
        // `mesh:write` here is closed (2.18.2).
        "/gateway/rpc/serve/{kind}" => "mesh:serve",
        "/gateway/rpc/respond"     => "mesh:serve",
        "/gateway/scatter"         => "mesh:write",
        "/gateway/mailbox/deliver" => "mesh:write",
        "/gateway/mailbox/{kind}"  => "mesh:read",
        "/gateway/shard/emit"      => "mesh:write",
        // Node-level routes behind the gate since 2026-09-05 (MatchedPath is the bare pattern —
        // these are merged, not nested under /gateway).
        "/mcp"              => "mcp:invoke",
        "/signals/{kind}"   => "mesh:read",
        "/consensus/{*slot}" => "consensus:read",
        // Layer III consensus / consistency overlay
        "/gateway/overlay/consistent/set"      => "consensus:write",
        "/gateway/overlay/consistent/get"      => "consensus:read",
        "/gateway/overlay/lock/acquire"        => "consensus:write",
        "/gateway/overlay/lock/{guard_id}"     => "consensus:write",
        "/gateway/overlay/elect"               => "consensus:write",
        "/gateway/overlay/log/append"          => "consensus:write",
        "/gateway/overlay/log/scan"            => "consensus:read",
        "/gateway/overlay/log/compact"         => "consensus:write",
        "/gateway/overlay/log/subscribe"       => "consensus:read",
        "/gateway/overlay/log/group/subscribe" => "consensus:read",
        "/gateway/overlay/emit_reliable"       => "consensus:write",
        "/gateway/consensus/cross_group_propose" => "consensus:write",
        // LLM / prompt skills
        "/gateway/prompts"             => "llm:read",
        "/gateway/prompts/{ns}/{name}" => if read { "llm:read" } else { "llm:write" },
        "/gateway/llm/call"            => "llm:invoke",
        "/gateway/llm/stream"          => "llm:invoke",
        // Audit trail (WS2)
        "/gateway/audit"               => "audit:read",
        "/gateway/transparency"        => "transparency:read",
        "/gateway/identity/revoke"     => "identity:write",
        // WS-C governance (intent publish + effective-state snapshot)
        "/gateway/govern"              => "govern:read",
        "/gateway/govern/tuning"       => "govern:write",
        "/gateway/govern/timing"       => "govern:write",
        "/gateway/govern/membership"   => "govern:write",
        "/gateway/govern/topology-override" => "govern:write",
        // Membership is a node speaking for itself: read the roster with `mesh:read`, join or
        // leave **this** node with `mesh:write`. There is deliberately no verb for enrolling
        // another node — see `gw_group_join`.
        "/gateway/mesh/group"          => if read { "mesh:read" } else { "mesh:write" },
        "/gateway/govern/group"        => "govern:write",
        "/gateway/govern/profile"      => "govern:write",
        // Legible Emergence Phase 2/3: the relational fleet snapshot + causal explain.
        "/gateway/fleet"               => "fleet:read",
        "/gateway/explain"             => "fleet:read",
        "/gateway/diagnose"            => "fleet:read",
        "/gateway/guarantees"          => "fleet:read",
        // Companion-crate gateway surfaces (merged via `with_http_routes`; behind this
        // layer since 2026-09-04). Exact paths only — a companion route not listed here
        // stays deny-by-default `admin`, like any other unmapped route.
        //   mycelium-reason → the `llm` family (routed inference is an LLM call).
        "/gateway/reason/route"               => "llm:invoke",
        "/gateway/reason/v1/chat/completions" => "llm:invoke",
        "/gateway/reason/v1/models"           => "llm:read",
        "/gateway/reason/trace/{run_id}"      => "llm:read",
        "/gateway/reason/blob/{id}"           => "llm:read",
        "/gateway/reason/blob"                => "llm:write",
        //   mycelium-wiki → `wiki:*`.
        "/gateway/wiki/read"    => "wiki:read",
        "/gateway/wiki/query"   => "wiki:read",
        "/gateway/wiki/propose" => "wiki:write",
        "/gateway/wiki/ingest"  => "wiki:write",
        //   mycelium-blackboard → `board:*`.
        "/gateway/bb/read"    => "board:read",
        "/gateway/bb/depth"   => "board:read",
        "/gateway/bb/post"    => "board:write",
        "/gateway/bb/claim"   => "board:write",
        "/gateway/bb/ack"     => "board:write",
        "/gateway/bb/release" => "board:write",
        //   mycelium-wasm-host → `artifact:*` (A3): a signed catalogue line into `installable/`.
        //   Its own family, not `kv:write`, because it is a narrower power with a check the raw
        //   KV route does not make (provenance against the node's trusted publishers).
        "/gateway/artifacts/publish" => "artifact:publish",
        //   mycelium-tuple-space → `tuple:*`.
        "/gateway/tuple/depth"       => "tuple:read",
        "/gateway/tuple/put"         => "tuple:write",
        "/gateway/tuple/take"        => "tuple:write",
        "/gateway/tuple/take_by_key" => "tuple:write",
        "/gateway/tuple/complete"    => "tuple:write",
        "/gateway/tuple/ack"         => "tuple:write",
        // Federation's consumer side (item 2 row 11). `federation:invoke` is separate from
        // `federation:read` because the two are different powers: reading which partners exist and
        // what they last exported is operator information, while `connect` and `call` spend this
        // domain's credentials on a partner's gateway under the caller's own name.
        "/gateway/federation/domain"           => "federation:read",
        "/gateway/federation/partners"         => "federation:read",
        "/gateway/federation/catalog/{domain}" => "federation:read",
        "/gateway/federation/connect"          => "federation:invoke",
        "/gateway/federation/call"             => "federation:invoke",
        // Deny-by-default.
        _ => "admin",
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /metrics` — Prometheus text-format scrape endpoint.
///
/// Available when the `metrics` cargo feature is enabled. Returns
/// `text/plain; version=0.0.4` as expected by Prometheus scrapers.
/// When the feature is disabled, returns 404.
async fn metrics_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    #[cfg(feature = "metrics")]
    {
        let body = ctx.prometheus.render();
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        ).into_response()
    }
    #[cfg(not(feature = "metrics"))]
    {
        let _ = ctx;
        (StatusCode::NOT_FOUND, "metrics feature not enabled").into_response()
    }
}

async fn health_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    // The WAL writer's state, so a writer refusing appends after a failed write is visible here and
    // not only in the log (the adversarial review of #584). Liveness is unchanged: the process is
    // alive, and the body says what its disk is doing.
    let persistence = match ctx.agent_ctx.wal.get() {
        None => json!({ "configured": false }),
        Some(wal) => {
            let reason = wal.refusing_appends();
            json!({
                "configured": true,
                "wal_refusing_appends": reason.is_some(),
                "reason": reason,
                "dropped_appends": wal.dropped_appends(),
            })
        }
    };
    Json(json!({
        "status":  "ok",
        "node_id": ctx.agent_ctx.node_id.to_string(),
        "persistence": persistence,
    }))
}

/// Readiness probe: returns 200 once soft-state keys (capabilities, locality)
/// have been written to the local store after startup or restart.
/// Returns 503 while WAL replay is still pending or the first advertisement
/// tick has not yet fired.
///
/// Use `/health` for liveness; use `/ready` before routing traffic. Ready = the node has completed
/// startup and serves KV/signals/membership; capability discovery gossips independently and does not
/// gate readiness (so a node advertising no soft state is still ready — audit 2026-07-15 pass 4).
async fn ready_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    if ctx.agent_ctx.soft_state_advertised.load(std::sync::atomic::Ordering::Acquire) {
        (StatusCode::OK, Json(json!({ "status": "ready", "node_id": ctx.agent_ctx.node_id.to_string() }))).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "starting", "node_id": ctx.agent_ctx.node_id.to_string() }))).into_response()
    }
}

/// `GET /bulk/{corr_id}`
///
/// Serves a staged bulk-call payload by nonce (hex-encoded 16-char string).
/// Used by the `bulk_serve` target to fetch the caller's staged data over HTTP.
/// Returns 200 + raw bytes on hit, 404 when the nonce is not found.
async fn bulk_staging_handler(
    Path(corr_id): Path<String>,
    State(ctx):    State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let nonce = match u64::from_str_radix(corr_id.trim_start_matches("0x"), 16) {
        Ok(n)  => n,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    match ctx.agent_ctx.bulk_transport.get(nonce) {
        Some(bytes) => (StatusCode::OK, bytes.to_vec()).into_response(),
        None        => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /consensus/{*slot}` — inspect the committed value and current ballot for a slot.
///
/// Returns `{"slot": "…", "committed": "<base64>" | null, "ballot": <u64>,
/// "lease_ms": <u64> | null, "lease_expired": <bool>}`.
/// `committed` is the **live** value: `null` when nothing has been committed
/// yet *or* when an epoch lease has expired (the slot has reopened —
/// `lease_expired: true` distinguishes the two). `ballot` reflects the highest
/// ballot number seen for that slot (0 = never proposed).
///
/// This endpoint is public (no auth) and is intended for operational debugging.
#[cfg(feature = "consensus")]
async fn consensus_slot_handler(
    Path(slot):   Path<String>,
    State(ctx):   State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let committed_key = format!("{}{}", crate::consensus::consensus_ns::COMMITTED, slot);
    let lease_key     = format!("{}{}", crate::consensus::consensus_ns::LEASE,     slot);
    let ballot_key    = format!("{}{}", crate::consensus::consensus_ns::BALLOT,    slot);
    let live = crate::consensus::live_committed_value(
        &ctx.agent_ctx.kv_state, &slot, crate::consensus::causal_now_ms(&ctx.agent_ctx.hlc),
    );
    let store = ctx.agent_ctx.kv_state.store.pin();
    let raw_present = store.get(committed_key.as_str())
        .and_then(|e| e.data.as_ref())
        .is_some();
    let committed_b64 = live
        .map(|b| base64::engine::general_purpose::STANDARD.encode(&b));
    let lease_expired = raw_present && committed_b64.is_none();
    let lease_ms = store.get(lease_key.as_str())
        .and_then(|e| e.data.clone())
        .and_then(|b| crate::consensus::decode_lease_ms(&b));
    let ballot: u64 = store.get(ballot_key.as_str())
        .and_then(|e| e.data.clone())
        .map(|b| crate::consensus::decode_ballot(&b))
        .unwrap_or(0);
    Json(json!({
        "slot":          slot,
        "committed":     committed_b64,
        "ballot":        ballot,
        "lease_ms":      lease_ms,
        "lease_expired": lease_expired,
    }))
}

async fn stats_handler(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    let kv = &ctx.agent_ctx.kv_state;
    let task_count = ctx.agent_ctx.task_handles
        .lock().unwrap_or_else(|e| e.into_inner())
        .len();
    let mut body = json!({
        "node_id":       ctx.agent_ctx.node_id.to_string(),
        "cluster_name":  ctx.agent_ctx.config.cluster_name,
        "store_entries": kv.store.pin().len(),
        // SWIM membership-view size — the map the de-pin reads; the direct diagnostic
        // for connection-fan-out / scale behaviour (small here ⇒ de-pin precondition unmet).
        "peers":         ctx.agent_ctx.peers.pin().len(),
        "dropped_frames": kv.dropped_frames.load(std::sync::atomic::Ordering::Relaxed),
        "individual_flood_fallbacks": kv.individual_flood_fallbacks.load(std::sync::atomic::Ordering::Relaxed),
        "task_count":    task_count,
        "commit_conflicts": ctx.agent_ctx.commit_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "sys_namespace_violations": ctx.agent_ctx.sys_namespace_violations
            .load(std::sync::atomic::Ordering::Relaxed),
        "identity_anchor_conflicts": ctx.agent_ctx.identity_anchor_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "cap_authz_violations": ctx.agent_ctx.cap_authz_violations
            .load(std::sync::atomic::Ordering::Relaxed),
        "schema_mismatch": ctx.agent_ctx.schema_mismatch
            .load(std::sync::atomic::Ordering::Relaxed),
        "governance_changes": ctx.agent_ctx.governance_changes
            .load(std::sync::atomic::Ordering::Relaxed),
        "governance_unaudited": ctx.agent_ctx.governance_unaudited
            .load(std::sync::atomic::Ordering::Relaxed),
        "rate_limited_senders": mycelium_core::rate::throttled_sender_count(&ctx.agent_ctx.core),
        "rpc_reply_sender_mismatches": ctx.agent_ctx.rpc_reply_sender_mismatches
            .load(std::sync::atomic::Ordering::Relaxed),
        // Legible-Emergence Phase 1 (emergent detectors). The conflict gauge is always present
        // (0 unless the detector loop is running); `view_confidence` — the RT1/RT2 "this is a
        // per-node estimate, not fleet truth" header — is attached only when detectors are enabled.
        "governed_group_conflicts": ctx.agent_ctx.governed_group_conflicts
            .load(std::sync::atomic::Ordering::Relaxed),
        "capability_coverage_gaps": ctx.agent_ctx.capability_coverage_gaps
            .load(std::sync::atomic::Ordering::Relaxed),
        "membership_flaps": ctx.agent_ctx.membership_flaps
            .load(std::sync::atomic::Ordering::Relaxed),
        // P10 — the largest share of the fleet's single-writer roles on one node. A **percentage,
        // not a count**: the reading is present whether or not it trips, so an operator can watch
        // it climb rather than learn about it at the threshold.
        "role_concentration_pct": ctx.agent_ctx.role_concentration_pct
            .load(std::sync::atomic::Ordering::Relaxed),
        "opacity_oscillations": ctx.agent_ctx.opacity_oscillations
            .load(std::sync::atomic::Ordering::Relaxed),
        "opaque_node_pct": ctx.agent_ctx.config.emergent_detectors_enabled
            .then(|| super::emergent::compute_opaque_node_pct(&ctx.agent_ctx)),
        "view_confidence": ctx.agent_ctx.config.emergent_detectors_enabled
            .then(|| super::emergent::compute_view_confidence(&ctx.agent_ctx)),
    });
    for (name, value) in feature_gated_counters() {
        body[name] = json!(value);
    }
    Json(body)
}

/// The `/stats` counters that exist only in some builds — each present exactly when the code that
/// increments it is compiled, so a reader never sees a `0` for a counter this build cannot raise.
/// Returned as a list (possibly empty) so the caller's loop is the same in every build.
fn feature_gated_counters() -> Vec<(&'static str, u64)> {
    // The 2.32.0 mixed-fleet allowance: signature validations accepted in the bare (pre-2.32.0)
    // form. Still rising means a peer not yet upgraded; the allowance closes in the next MINOR
    // (`deprecations.md` §23).
    let identity_untagged: Option<(&'static str, u64)> = {
        #[cfg(feature = "tls")]
        { Some(("identity_untagged_proofs", crate::agent::helpers::untagged_identity_proofs_accepted())) }
        #[cfg(not(feature = "tls"))]
        { None }
    };
    let consensus_untagged: Option<(&'static str, u64)> = {
        #[cfg(all(feature = "tls", feature = "consensus"))]
        { Some(("consensus_untagged_signatures", crate::consensus::untagged_consensus_signatures_accepted())) }
        #[cfg(not(all(feature = "tls", feature = "consensus")))]
        { None }
    };
    // Answers this node's acceptor withheld because its promise or acceptance did not reach the WAL.
    let acceptor_unrecorded: Option<(&'static str, u64)> = {
        #[cfg(feature = "consensus")]
        { Some(("consensus_acceptor_unrecorded", crate::consensus::acceptor_answers_unrecorded())) }
        #[cfg(not(feature = "consensus"))]
        { None }
    };
    [identity_untagged, consensus_untagged, acceptor_unrecorded].into_iter().flatten().collect()
}

/// `GET /gateway/audit` — query the tamper-evident audit trail (compliance, scope
/// `audit:read`). Optional `?node=` selects one stream (default: all known
/// streams); optional `?limit=` caps the records returned per stream (most
/// recent), while verification always runs over the full stream. Each stream
/// reports `verified` + any `verify_error`, the chain-tip `head_hash`, and each
/// record's stable `content_hash` (the M16-citable identifier).
#[cfg(feature = "compliance")]
#[derive(Deserialize)]
struct AuditQuery {
    node:  Option<String>,
    limit: Option<usize>,
}

#[cfg(feature = "compliance")]
fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(feature = "compliance")]
async fn gw_audit(
    State(ctx): State<Arc<HttpCtx>>,
    Query(q): Query<AuditQuery>,
) -> Response {
    let tc = &ctx.agent_ctx;
    let nodes: Vec<crate::node_id::NodeId> = match q.node.as_deref() {
        Some(s) => match s.parse() {
            Ok(n) => vec![n],
            Err(_) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid node id"})))
                    .into_response();
            }
        },
        None => super::audit::stream_nodes(tc),
    };
    let limit = q.limit.unwrap_or(usize::MAX);

    let mut streams = Vec::with_capacity(nodes.len());
    for node in nodes {
        let records = super::audit::read_stream(tc, &node);
        let verify  = super::audit::verify_stream(tc, &node);
        let head_hash = records.last().map(|sr| hex32(&sr.record.content_hash()));
        let shown: Vec<_> = records
            .iter()
            .rev()
            .take(limit)
            .rev()
            .map(|sr| {
                let r = &sr.record;
                json!({
                    "seq":          r.seq,
                    "hlc":          r.hlc,
                    "principal":    r.principal,
                    "action":       format!("{:?}", r.action),
                    "target":       r.target,
                    "outcome":      format!("{:?}", r.outcome),
                    "detail":       r.detail,
                    "content_hash": hex32(&r.content_hash()),
                })
            })
            .collect();
        streams.push(json!({
            "node":         node.to_string(),
            "count":        records.len(),
            "verified":     verify.is_ok(),
            "verify_error": verify.err().map(|e| format!("{e:?}")),
            "head_hash":    head_hash,
            "records":      shown,
        }));
    }
    Json(json!({ "streams": streams })).into_response()
}

/// `GET /gateway/transparency` (scope `transparency:read`) — the revocation transparency log
/// (WS-D / D2). With no query: each node's Merkle `root` + `count` (the head). With `?node=&key=`
/// (key = 64-hex of a revoked verifying key): a **client-checkable inclusion proof** that the
/// revocation is in that node's log — `leaf`, `index`, the Merkle audit `proof`, and the `root` to
/// verify against (run `transparency::verify_inclusion` locally; no trust in this server needed).
#[cfg(feature = "compliance")]
#[derive(Deserialize)]
struct TransparencyQuery {
    node: Option<String>,
    key:  Option<String>,
}

#[cfg(feature = "compliance")]
fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    // `s.len()` is a BYTE length; the loop below byte-slices `&s[i*2..i*2+2]` assuming 1 byte/char.
    // Without the ASCII guard a 64-byte string containing a multibyte UTF-8 char panics on a
    // non-char-boundary slice — and with `panic = "abort"` (release) that aborts the node. Hex is
    // ASCII, so reject non-ASCII up front (audit 2026-07-15 pass 2).
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// `POST /gateway/identity/revoke` (scope `identity:write`, `compliance`) — SOC 2 WS-B.
///
/// Operator-facing key revocation, the compromise-remediation trigger. Body:
/// `{ "revoked_key": "<64 hex>", "reason": "..."? }`. Writes a signed revocation (signed by this
/// node's **current** key) which all verify paths — roles, audit, **and consensus** — then exclude
/// cluster-wide. Only this node's own historical keys can be revoked (the signer must hold the
/// current key), so this remediates *this* node's compromised key: rotate to a fresh key first,
/// then revoke the old one (or use `rotate_identity_on_compromise`, which does both).
#[cfg(feature = "compliance")]
async fn gw_identity_revoke(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> Response {
    let key_s = match body["revoked_key"].as_str() {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing revoked_key (64 hex chars)"}))).into_response(),
    };
    let Some(revoked_key) = parse_hex32(key_s) else {
        return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "revoked_key must be 64 hex chars"}))).into_response();
    };
    let reason = body["reason"].as_str().map(|s| s.to_string());
    match super::revocation::revoke_key(&ctx.agent_ctx, revoked_key, reason) {
        Ok(())  => {
            audit_govern(&ctx.agent_ctx, "identity/revoke", body.to_string());
            Json(json!({"ok": true, "revoked_key": key_s})).into_response()
        }
        Err(e)  => (StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[cfg(feature = "compliance")]
async fn gw_transparency(
    State(ctx): State<Arc<HttpCtx>>,
    Query(q): Query<TransparencyQuery>,
) -> Response {
    let tc = &ctx.agent_ctx;

    // Inclusion-proof mode: ?node=&key=.
    if let (Some(node_s), Some(key_s)) = (q.node.as_deref(), q.key.as_deref()) {
        let Ok(node) = node_s.parse::<crate::node_id::NodeId>() else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid node id"}))).into_response();
        };
        let Some(revoked_key) = parse_hex32(key_s) else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "key must be 64 hex chars"}))).into_response();
        };
        return match super::transparency::inclusion_proof(tc, &node, &revoked_key) {
            Some((leaf, index, proof, root)) => Json(json!({
                "node":        node.to_string(),
                "revoked_key": key_s,
                "included":    true,
                "root":        hex32(&root),
                "leaf":        hex32(&leaf),
                "index":       index,
                "proof":       proof.iter().map(|s| json!({
                    "sibling":  hex32(&s.sibling),
                    "on_right": s.on_right,
                })).collect::<Vec<_>>(),
            })).into_response(),
            None => Json(json!({
                "node": node.to_string(), "revoked_key": key_s, "included": false,
            })).into_response(),
        };
    }

    // Head mode: every node's revocation-log root + count.
    let nodes = super::revocation::revocation_nodes(tc);
    let heads: Vec<_> = nodes.iter().map(|node| {
        let (root, count) = super::transparency::revocation_head(tc, node);
        json!({ "node": node.to_string(), "root": hex32(&root), "count": count })
    }).collect();
    Json(json!({ "nodes": heads })).into_response()
}

// ── WS-C governance: management = intent + local reconcile ───────────────────
//
// A HITL operator (or an agent with a concern) publishes an evaporating fleet
// *intent* over the gossip KV; every node reconciles it locally, local pins win,
// and the intent self-heals away if the publisher vanishes. These routes are the
// publish surface (POST) plus an effective-state snapshot (GET). They never command
// a node — they only seed soft-state that nodes choose to honour (Principles 1 & 5).

/// Record a governance change in the tamper-evident audit trail. It needs `compliance` and a `[tls]` identity
/// to seal; a change that leaves no record — either missing, or a seal that failed — is counted
/// (`governance_unaudited` on `/stats`) and, when the seal failed, warned about, rather than vanishing.
fn audit_govern(ctx: &Arc<TaskCtx>, target: &str, detail: String) {
    use std::sync::atomic::Ordering::Relaxed;
    ctx.governance_changes.fetch_add(1, Relaxed);
    #[cfg(feature = "compliance")]
    let recorded = match super::audit::seal_and_write(
        ctx,
        super::audit::AuditAction::Admin,
        "gateway/govern",
        target,
        super::audit::AuditOutcome::Success,
        Some(detail),
    ) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(target, "governance change applied but not audited: {e}");
            false
        }
    };
    #[cfg(not(feature = "compliance"))]
    let recorded = { let _ = (target, detail); false };
    if !recorded {
        ctx.governance_unaudited.fetch_add(1, Relaxed);
    }
}

/// Parse an optional `"target"` field into a node id. `Err` carries a message the
/// caller turns into a 400 (a small error type keeps the `Result` cheap).
///
/// A `target` that is present but not a node-id string is refused: read as *no* target it governed the
/// whole fleet (the reviews of #544).
fn parse_optional_target(body: &serde_json::Value) -> Result<Option<crate::node_id::NodeId>, &'static str> {
    match body.get("target") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some(s) => s.parse().map(Some).map_err(|_| "invalid target node id"),
            None => Err("`target` must be a node id string (host:port)"),
        },
    }
}

/// A governance body must be an object naming only the fields its route reads: a misspelled field was
/// ignored and the intent published without it.
fn refuse_unknown_fields(body: &serde_json::Value, known: &[&str]) -> Option<axum::response::Response> {
    let Some(obj) = body.as_object() else {
        return Some((StatusCode::BAD_REQUEST, Json(json!({"error": "the body must be a JSON object"}))).into_response());
    };
    obj.keys().find(|k| !known.contains(&k.as_str())).map(|k| {
        (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown field `{k}`")}))).into_response()
    })
}

/// An optional non-negative integer field: absent or `null` is `None`; anything else that is not a `u64`
/// is refused rather than read as absent.
fn optional_u64(v: Option<&serde_json::Value>, name: &str) -> Result<Option<u64>, String> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("{name} must be a non-negative integer")),
    }
}

fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

/// `GET /gateway/govern` — this node's **effective** tuning-governor state (the
/// reconciled result of local pins + the current fleet intent). Per-node by design;
/// scrape every node for the fleet picture (there is no central view — Principle 1).
async fn gw_govern_snapshot(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    let snap = ctx.agent_ctx.tuning_governor.snapshot();
    let params: Vec<_> = snap
        .params
        .iter()
        .map(|p| {
            json!({
                "param":          p.param.key(),
                "floor":          p.floor,
                "ceiling":        p.ceiling,
                "ratchet":        format!("{:?}", p.ratchet).to_lowercase(),
                "locally_pinned": p.locally_pinned,
                "pending":        p.pending,
            })
        })
        .collect();
    // Item 4 §7: the node's control profile and the tripwires an operator watches before stepping
    // it (`docs/operations/control-profiles.md`). The counters keep their names across the ladder;
    // `profile` says which reading they are.
    let relaxed = std::sync::atomic::Ordering::Relaxed;
    let control = json!({
        "profile":                 crate::control::Profile::from_u8(ctx.agent_ctx.control_profile.load(relaxed)).name(),
        "would_hold":              ctx.agent_ctx.control_would_hold.load(relaxed),
        "opacity_releases_spaced": ctx.agent_ctx.opacity_releases_spaced.load(relaxed),
        "tuning": {
            "profile":          snap.profile.name(),
            "held_by_spacing":  snap.held_by_spacing,
            "held_by_settling": snap.held_by_settling,
            "settled_unknown":  snap.settled_unknown,
        },
    });
    Json(json!({
        "node_id":      ctx.agent_ctx.node_id.to_string(),
        "auto_enabled": snap.auto_enabled,
        "params":       params,
        "control":      control,
    }))
    .into_response()
}

/// `POST /gateway/govern/profile` — step **this node's** control profile along the ADR's ladder
/// (§7: `legacy` · `observe` · `enforce-local` · `enforce-allocated`; the rollout is
/// `docs/operations/control-profiles.md`). Body `{"profile": "observe"}`. Per node, like every
/// govern route, and it takes effect on each governor's next pass. An unknown name is `400` and
/// changes nothing — never read as `legacy`. Scope `govern:write`.
async fn gw_govern_profile(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::control::Profile;
    if let Some(refused) = refuse_unknown_fields(&body, &["profile"]) {
        return refused;
    }
    let Some(name) = body.get("profile").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing 'profile'"}))).into_response();
    };
    let Some(profile) = Profile::parse(name) else {
        let known: Vec<&str> = Profile::ALL.iter().map(|p| p.name()).collect();
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown profile '{name}'"), "known": known})))
            .into_response();
    };
    let was = Profile::from_u8(ctx.agent_ctx.control_profile.load(std::sync::atomic::Ordering::Relaxed));
    ctx.agent_ctx.set_control_profile(profile);
    audit_govern(&ctx.agent_ctx, "govern/profile", body.to_string());
    Json(json!({ "ok": true, "profile": profile.name(), "was": was.name() })).into_response()
}

/// `GET /gateway/fleet` — the Legible-Emergence Phase-2 **relational fleet snapshot**: the
/// operator's "localize" view, computed **locally** from the gossiped KV this node already holds
/// (no collector — any node answers it, and it survives killing any node; Principle 1). Governed-
/// group status (intent vs observed), capability-coverage gaps, fleet opacity, and the flap/
/// oscillation counters — each paired with the RT1/RT2 `view_confidence` header (a per-node
/// *estimate*, not fleet ground truth; at convergence the *diagnosis* agrees across nodes while
/// `view_confidence` stays each observer's own). Scope `fleet:read`.
async fn gw_fleet_snapshot(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::emergent::compute_fleet_snapshot(&ctx.agent_ctx)).into_response()
}

/// `GET /gateway/diagnose` — the Legible-Emergence Phase-4 **fleet narrative**: the "why is the
/// fleet in this state" diagnosis. A templated rule engine over the Phase-2 snapshot (one rule per
/// Phase-0 pathology) that names each cause in code-free, actionable terms — the artifact an on-call
/// engineer who did not build the system can act on. Every diagnosis is qualified by the observer's
/// own RT1/RT2 view health (`caveat`), so a clean read from a blind node is not mistaken for a
/// healthy fleet. Scope `fleet:read`.
/// `GET /gateway/guarantees` — the node's guarantee report (plan I2), recomputed live: every
/// registered guarantee resolved to `enforced` / `not_configured` / `not_in_build` /
/// `not_applicable` (with the role fact) / `not_verifiable_here`, with the configuration digest it was
/// computed over. Read-only; scope `fleet:read`.
async fn gw_guarantees(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::guarantee::report(&ctx.agent_ctx)).into_response()
}

async fn gw_diagnose(State(ctx): State<Arc<HttpCtx>>) -> impl IntoResponse {
    Json(super::emergent::compute_fleet_diagnosis(&ctx.agent_ctx)).into_response()
}

/// `GET /gateway/explain?since=<hlc>` — the Legible-Emergence Phase-3 causal **explain**: the
/// HLC-ordered narrative of significant fleet events (`?since` filters to `hlc >= since`; default
/// all). Fans a best-effort `sys.explain` RPC out to a **capped** subset of known peers
/// (`EXPLAIN_MAX_FANOUT`, so the query never becomes an O(N) RPC storm), merges each node's ring into
/// one causal stream, and — RT3 — names both the peers that did not answer (`non_responders`) and the
/// count skipped by the cap (`not_queried`) rather than silently dropping either. Scope `fleet:read`.
async fn gw_explain(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let since = q.get("since").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Json(super::emergent::assemble_explain(&ctx.agent_ctx, since).await).into_response()
}

/// `POST /gateway/govern/tuning` — publish a cluster-wide (or `target`-ed) tuning
/// intent. Body:
/// ```json
/// {"enabled": true,
///  "params": [{"param": "writer_depth", "floor": 1024, "ceiling": 8192, "ratchet": "up"}],
///  "target": "10.0.0.5:9000"}
/// ```
/// All fields optional except that at least one of `enabled` / `params` must be present.
async fn gw_govern_tuning(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use super::tuning_governor::{GovernIntent, HotParam, ParamDirective, Ratchet};

    if let Some(refused) = refuse_unknown_fields(&body, &["enabled", "params", "target"]) {
        return refused;
    }
    let enabled = match body.get("enabled") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_bool() {
            Some(b) => Some(b),
            None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "`enabled` must be a boolean"}))).into_response(),
        },
    };

    let mut params = Vec::new();
    let params_in = match body.get("params") {
        None | Some(serde_json::Value::Null) => &[][..],
        Some(serde_json::Value::Array(a)) => a.as_slice(),
        Some(_) => return bad_request("`params` must be an array of directives".into()),
    };
    {
        for d in params_in {
            if let Some(refused) = refuse_unknown_fields(d, &["param", "floor", "ceiling", "ratchet"]) {
                return refused;
            }
            let Some(pkey) = d.get("param").and_then(|v| v.as_str()) else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "param directive missing 'param'"}))).into_response();
            };
            if HotParam::from_key(pkey).is_none() {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown param '{pkey}'")}))).into_response();
            }
            let ratchet = match d.get("ratchet") {
                None | Some(serde_json::Value::Null) => Ratchet::Off,
                Some(v) => match v.as_str() {
                    Some("up")   => Ratchet::Up,
                    Some("down") => Ratchet::Down,
                    Some("off")  => Ratchet::Off,
                    Some(other)  => return bad_request(format!("unknown ratchet '{other}'")),
                    None         => return bad_request("`ratchet` must be \"up\", \"down\" or \"off\"".into()),
                },
            };
            let floor = match optional_u64(d.get("floor"), "floor") { Ok(v) => v, Err(e) => return bad_request(e) };
            let ceiling = match optional_u64(d.get("ceiling"), "ceiling") { Ok(v) => v, Err(e) => return bad_request(e) };
            params.push(ParamDirective { param: pkey.to_string(), floor, ceiling, ratchet });
        }
    }

    if enabled.is_none() && params.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "intent must set 'enabled' or 'params'"}))).into_response();
    }

    let target = match parse_optional_target(&body) {
        Ok(t)  => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };

    let intent = GovernIntent { enabled, params, written_at_ms: 0, target };
    let kv = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&ctx.agent_ctx.core));
    let ok = super::intent::publish_intent(&kv, super::tuning_governor::GOVERN_FLEET_KEY, intent);
    audit_govern(&ctx.agent_ctx, super::tuning_governor::GOVERN_FLEET_KEY, body.to_string());
    Json(json!({"ok": ok, "key": super::tuning_governor::GOVERN_FLEET_KEY})).into_response()
}

/// `POST /gateway/govern/timing` — publish a cluster-wide (or `target`-ed) **timing** intent
/// (WS-C / M10.2). Body:
/// ```json
/// {"health_check_interval_secs": 2, "reconnect_backoff_secs": 3, "target": null}
/// ```
/// `0`/absent for a field leaves it as it is; `target` `null` = whole fleet. Newest-wins,
/// local-wins (a node that called a `set_*` setter ignores it), evaporating. No consensus fence.
async fn gw_govern_timing(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // A present field must be a non-negative integer: `as_u64().unwrap_or(0)` read `"30"` or `-5` as
    // `0`, "ungoverned", and answered success for an intent that governs nothing. The same for the shape
    // around them: a non-object, a misspelled field, or a `target` that is not a node id (read as *no*
    // target — the whole fleet) each published something other than what was asked.
    if let Some(refused) = refuse_unknown_fields(&body, &["health_check_interval_secs", "reconnect_backoff_secs", "target"]) {
        return refused;
    }
    let mut fields = [0u64; 2];
    for (slot, name) in fields.iter_mut().zip(["health_check_interval_secs", "reconnect_backoff_secs"]) {
        match body.get(name) {
            None | Some(serde_json::Value::Null) => {}
            Some(v) => match v.as_u64() {
                Some(n) => *slot = n,
                None => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("{name} must be a non-negative integer")}))).into_response(),
            },
        }
    }
    let [health, reconnect] = fields;
    let target = match parse_optional_target(&body) {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let intent = super::timing_governor::TimingIntent {
        health_check_interval_secs: health,
        reconnect_backoff_secs: reconnect,
        target,
        written_at_ms: 0,
    };
    match super::timing_governor::publish_timing_intent(&ctx.agent_ctx, intent) {
        Ok(ok) => {
            // Audited as its siblings are; it never was (the third review of #544).
            audit_govern(&ctx.agent_ctx, super::timing_governor::TIMING_INTENT_KEY, body.to_string());
            Json(json!({ "published": ok, "key": super::timing_governor::TIMING_INTENT_KEY })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// `POST /gateway/govern/topology-override` — set or release a group's Hard-topology escape hatch.
/// Body: `{"group": "G", "override": true|false}`. `true` writes `sys/topology-override/{group}` = `b"true"`
/// (the only value the consensus gate reads as active); `false` tombstones it. Scope `govern:write`, audited:
/// relaxing the failure-domain gate is a governance act, and until 2026-10-07 it was an unaudited `kv:write`.
async fn gw_govern_topology_override(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_unknown_fields(&body, &["group", "override"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|v| v.as_str()) else {
        return bad_request("missing 'group'".into());
    };
    if group.is_empty() || group.contains('/') {
        return bad_request("`group` must be a non-empty group name without '/'".into());
    }
    let Some(engage) = body.get("override").and_then(|v| v.as_bool()) else {
        return bad_request("`override` must be a boolean".into());
    };
    let key = format!("sys/topology-override/{group}");
    if engage {
        kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::from_static(b"true"), false);
    } else {
        kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::new(), true);
    }
    audit_govern(&ctx.agent_ctx, &key, body.to_string());
    Json(json!({ "ok": true, "key": key, "override": engage })).into_response()
}

/// `POST /gateway/govern/membership` — publish an elastic-sizing intent for a group.
/// Body:
/// ```json
/// {"group": "workers", "min": 3, "max": 10, "drain": ["10.0.0.5:9000"], "target": null}
/// ```
/// `group` + `min` required; `max` `null`/absent = unbounded; `drain` cooperative self-removal.
#[derive(Deserialize)]
struct GroupBody { group: String }

/// `POST /gateway/mesh/group` — **this node** joins signal-boundary group `{group}`.
///
/// Body: `{"group": "G"}`. Returns `{"ok": true, "group": "G", "members": [...]}`.
///
/// **A node joins itself, and there is no verb for enrolling another node.** That is the shape the
/// substrate already commits to everywhere else — an agent promises only its own behaviour — and it
/// is why this route takes no node id. Membership is published at `grp/{group}/{self}` and gossips
/// like any other soft state.
///
/// **Why this route exists (2026-09-24).** It did not, and its absence was a finding: the gateway
/// offered `POST /gateway/overlay/elect` over a group while providing no supported way for an HTTP
/// caller to *populate* one. The only roster an HTTP client could reach was the empty one — which
/// was precisely the state that used to confer solo authority
/// (`ConsensusResult::ElectorateUnavailable`). An election surface without a membership surface is
/// a surface that can only be used wrongly.
///
/// **Who may change an electorate (decided 2026-10-08).** A group under a live membership intent is *governed*: its
/// members are the roster and quorum its elections count, so moving this node in or out of one is a governance act —
/// this route refuses it **403** `governed_group`, and `POST`/`DELETE /gateway/govern/group` (`govern:write`) does it.
/// A plain group stays a `mesh:write` join. Every membership change through either route is audited (or counted on
/// `/stats` when it cannot be). *Which* membership version an election is decided against stays open
/// (`docs/wiki/dev/.log/2026-09-24-consensus-vote-binding.md`). An embedded caller holding the handle is not a
/// gateway caller and is not covered here.
#[cfg(feature = "gateway")]
async fn gw_group_join(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<GroupBody>,
) -> impl IntoResponse {
    if body.group.is_empty() || body.group.contains('/') {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"error": "group must be non-empty and contain no '/'"}))).into_response();
    }
    if let Some(refused) = refuse_governed_group(&ctx.agent_ctx, &body.group) {
        return refused;
    }
    if !is_member(&ctx.agent_ctx, &body.group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core))
            .join_group(body.group.as_str());
        audit_membership(&ctx.agent_ctx, &format!("grp/{}/join", body.group));
    }
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, &body.group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({ "ok": true, "group": body.group, "members": members })).into_response()
}

/// `DELETE /gateway/mesh/group?group=G` — **this node** leaves `G`, tombstoning `grp/G/{self}`.
#[cfg(feature = "gateway")]
async fn gw_group_leave(
    Query(q):   Query<GroupQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_governed_group(&ctx.agent_ctx, &q.group) {
        return refused;
    }
    if is_member(&ctx.agent_ctx, &q.group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core))
            .leave_group(q.group.as_str());
        audit_membership(&ctx.agent_ctx, &format!("grp/{}/leave", q.group));
    }
    Json(json!({ "ok": true, "group": q.group })).into_response()
}

/// Whether `group` is under a live membership intent (`sys/govern/membership/{group}`, fresh within
/// `MEMBERSHIP_INTENT_TTL_MS` — the governor's own reading, `emergent::detect_governed_group_conflicts`). Such a
/// group's population is governed: who belongs to it decides an election's roster and quorum, so changing it is a
/// governance act, not a data-plane write. Fleet-wide: an intent `target`ed at one node still governs the group here —
/// the electorate floor (`declared_electorate_min`) and the conflict detector read it the same way. "Live" lapses: an
/// intent not re-published within the TTL leaves the group ungoverned (and a future-dated `written_at_ms` reads as
/// fresh, as it does for the governor).
#[cfg(feature = "gateway")]
fn is_governed_group(ctx: &TaskCtx, group: &str) -> bool {
    use super::membership_governor::{MembershipIntent, MEMBERSHIP_INTENT_TTL_MS, MEMBERSHIP_PREFIX};
    let key = format!("{MEMBERSHIP_PREFIX}{group}");
    let Some(bytes) = ctx.kv_state.store.pin().get(key.as_str()).and_then(|e| e.data.clone()) else { return false };
    let Ok(intent) = mycelium_core::serde_fixint::from_slice::<MembershipIntent>(&bytes) else { return false };
    mycelium_core::sim_seam::wall_now_ms().saturating_sub(intent.written_at_ms) <= MEMBERSHIP_INTENT_TTL_MS
}

/// `/gateway/mesh/group`'s refusal for a governed group: **403** `governed_group`, naming the route that moves a node
/// in or out of one under `govern:write` (where the accepted change is audited; the refusal itself is not).
#[cfg(feature = "gateway")]
fn refuse_governed_group(ctx: &TaskCtx, group: &str) -> Option<axum::response::Response> {
    is_governed_group(ctx, group).then(|| (
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "governed_group",
            "message": format!("group {group} is under a membership intent, so its members decide its elections: \
                                this node joins or leaves it through POST/DELETE /gateway/govern/group (govern:write)") })),
    ).into_response())
}

/// `POST /gateway/govern/group` (`govern:write`) — **this node** joins `{group}`, governed or not; audited. The
/// governed path for what `POST /gateway/mesh/group` refuses for a group under a membership intent.
#[cfg(feature = "gateway")]
async fn gw_govern_group_join(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_unknown_fields(&body, &["group"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|g| g.as_str()).filter(|g| !g.is_empty() && !g.contains('/')) else {
        return bad_request("`group` must be a non-empty string without '/'".into());
    };
    if !is_member(&ctx.agent_ctx, group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core)).join_group(group);
        audit_govern(&ctx.agent_ctx, &format!("grp/{group}/join"), body.to_string());
    }
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({ "ok": true, "group": group, "governed": is_governed_group(&ctx.agent_ctx, group), "members": members }))
        .into_response()
}

/// `DELETE /gateway/govern/group?group=G` (`govern:write`) — **this node** leaves `G`, governed or not; audited.
#[cfg(feature = "gateway")]
async fn gw_govern_group_leave(
    Query(q):   Query<std::collections::HashMap<String, String>>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    // As strict as the POST: a `target` (or any other parameter) is refused, not dropped and applied here.
    if let Some(k) = q.keys().find(|k| k.as_str() != "group") {
        return bad_request(format!("unknown parameter `{k}`"));
    }
    let Some(group) = q.get("group").filter(|g| !g.is_empty() && !g.contains('/')) else {
        return bad_request("`group` must be a non-empty name without '/'".into());
    };
    if is_member(&ctx.agent_ctx, group) {
        mycelium_core::mesh_handle::MeshHandle::from_core(Arc::clone(&ctx.agent_ctx.core)).leave_group(group.as_str());
        audit_govern(&ctx.agent_ctx, &format!("grp/{group}/leave"), json!({"route": "govern/group"}).to_string());
    }
    Json(json!({ "ok": true, "group": group })).into_response()
}

/// Whether this node is a live member of `group` — so a join that changes nothing, or a leave of a group it is not
/// in, is not recorded as a change.
#[cfg(feature = "gateway")]
fn is_member(ctx: &TaskCtx, group: &str) -> bool {
    crate::agent::helpers::group_members_ctx(ctx, group).contains(&ctx.node_id)
}

/// Record a plain group's membership change in the audit trail. Not a governance change — a plain group's membership is
/// `mesh:write` data plane — so the governance counters are not touched; a failed seal is a warning (#572's review).
#[cfg(feature = "gateway")]
fn audit_membership(ctx: &Arc<TaskCtx>, target: &str) {
    #[cfg(feature = "compliance")]
    if let Err(e) = super::audit::seal_and_write(ctx, super::audit::AuditAction::Admin, "gateway/mesh", target,
                                                 super::audit::AuditOutcome::Success, None) {
        tracing::debug!(target, "group membership change not audited: {e}");
    }
    #[cfg(not(feature = "compliance"))]
    let _ = (ctx, target);
}

/// `GET /gateway/mesh/group?group=G` — the roster this node can see for `G`.
///
/// This is the read an operator needs *before* an election: it answers "does this node have an
/// electorate, and is its view complete?", which is the question a refusal will otherwise answer
/// for them.
#[cfg(feature = "gateway")]
async fn gw_group_members(
    Query(q):   Query<GroupQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let members: Vec<String> = crate::agent::helpers::group_members_ctx(&ctx.agent_ctx, &q.group)
        .iter().map(|n| n.to_string()).collect();
    Json(json!({
        "group":        q.group,
        "members":      members,
        "declared_min": crate::agent::helpers::declared_electorate_min(&ctx.agent_ctx, &q.group),
    })).into_response()
}

#[derive(Deserialize)]
struct GroupQuery { group: String }

async fn gw_govern_membership(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use super::membership_governor::{MembershipIntent, MEMBERSHIP_PREFIX};

    if let Some(refused) = refuse_unknown_fields(&body, &["group", "min", "max", "drain", "target"]) {
        return refused;
    }
    let Some(group) = body.get("group").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing 'group'"}))).into_response();
    };
    // `min` is required (the route's contract); a `"3"` or `-1` read as 0 and published a floor of nothing.
    let min = match optional_u64(body.get("min"), "min") {
        Ok(Some(m)) => m as usize,
        Ok(None) => return bad_request("missing 'min'".into()),
        Err(e) => return bad_request(e),
    };
    let max = match optional_u64(body.get("max"), "max") { Ok(v) => v.map(|m| m as usize), Err(e) => return bad_request(e) };

    let mut drain: Vec<crate::node_id::NodeId> = Vec::new();
    let drain_in = match body.get("drain") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(a)) => Some(a),
        Some(_) => return bad_request("`drain` must be an array of node ids".into()),
    };
    if let Some(arr) = drain_in {
        for v in arr {
            let Some(s) = v.as_str() else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "drain entries must be node-id strings"}))).into_response();
            };
            match s.parse() {
                Ok(n)  => drain.push(n),
                Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid drain node id '{s}'")}))).into_response(),
            }
        }
    }

    let target = match parse_optional_target(&body) {
        Ok(t)  => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };

    let mut intent = MembershipIntent::new(group, min, max).with_drain(drain);
    if let Some(t) = target {
        intent = intent.for_node(t);
    }
    let key = format!("{MEMBERSHIP_PREFIX}{group}");
    let kv = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&ctx.agent_ctx.core));
    let ok = super::intent::publish_intent(&kv, &key, intent);
    audit_govern(&ctx.agent_ctx, &key, body.to_string());
    Json(json!({"ok": ok, "key": key})).into_response()
}

/// SSE endpoint — streams admitted signals of the requested `kind`.
///
/// Each event carries:
/// - `event` field: the signal kind
/// - `data` field: JSON `{"kind","sender","payload_b64","nonce","payload"}` — the gateway stream's shape
///   (`kind` since 2.24.0; `payload_b64` and `nonce` since 2.26.0), plus `payload`, the same base64 under
///   the name this route used first, kept for existing readers
///
/// The subscription is torn down automatically when the client disconnects.
async fn signal_sse_handler(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Response {
    // Observing protected work is refused like sending it (closure plan C1, the SSE half): this
    // stream registers on the same table `rpc/serve` and the native MCP tools register on, and a
    // signal fans to every receiver — so a `mesh:read` holder would read each protected request's
    // whole frame (caller envelope, carried mandate and possession proof, correlation nonce).
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let rx = ctx.agent_ctx.signal_handlers.register_with_capacity(
        std::sync::Arc::from(kind.as_str()),
        256,
    );

    let stream = ReceiverStream::new(rx).map(|sig: crate::signal::Signal| {
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&sig.payload);
        // The gateway stream's shape, so one parser reads both routes (doc-coverage run 20): `payload_b64`
        // and the `nonce` added; `payload` kept, the same bytes, for readers of the original shape.
        let data = json!({
            "kind":        sig.kind.as_ref(),
            "sender":      sig.sender.to_string(),
            "payload_b64": payload_b64.clone(),
            "nonce":       sig.nonce,
            "payload":     payload_b64,
        });
        Ok::<_, Infallible>(Event::default()
            .event(sig.kind.as_ref())
            .data(data.to_string()))
    });

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// JSON-RPC 2.0 handler for the MCP protocol (`POST /mcp`).
///
/// Dispatches on `method`:
/// - `initialize`   — returns server capabilities.
/// - `tools/list`   — scans `tools/` prefix and returns registered tools.
/// - `tools/call`   — locates a provider and proxies the call via `rpc_call_ctx`.
async fn mcp_handler(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let caller = caller.map(|Extension(c)| c);
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v)  => v,
        Err(_) => {
            return Json(json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": "parse error"},
            })).into_response();
        }
    };

    let id     = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = req["method"].as_str().unwrap_or("");

    match method {
        "initialize" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "mycelium",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        })).into_response(),

        "tools/list" => {
            let mut tool_map: std::collections::HashMap<String, serde_json::Value>
                = Default::default();
            for (key, bytes) in
                crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, "tools/")
            {
                let rest = key.strip_prefix("tools/").unwrap_or_default();
                let Some((name, _node_id)) = rest.split_once('/') else { continue };
                if tool_map.contains_key(name) { continue; }
                let schema: serde_json::Value =
                    serde_json::from_slice(&bytes).unwrap_or(json!({}));
                tool_map.insert(name.to_string(), schema);
            }
            let tools: Vec<serde_json::Value> = tool_map.into_iter().map(|(name, schema)| {
                let description = schema.get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let mut entry = json!({"name": name, "inputSchema": schema});
                if let Some(desc) = description {
                    entry["description"] = json!(desc);
                }
                entry
            }).collect();
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tools},
            })).into_response()
        }

        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("").to_string();
            let arguments = req["params"]["arguments"].clone();

            if name.is_empty() {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32602, "message": "invalid params: missing tool name"},
                })).into_response();
            }

            let prefix = format!("tools/{name}/");
            let provider = crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, &prefix)
                .into_iter()
                .find_map(|(key, _)| {
                    let rest = key.strip_prefix(&prefix)?;
                    rest.parse::<crate::node_id::NodeId>().ok()
                });

            let Some(provider_node_id) = provider else {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("tool not found: {name}")},
                })).into_response();
            };

            let tool_req = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            });

            // AE slice: the evaluator preflight, between the auth layer and the dispatch. Inert
            // unless an evaluator is attached (`with_action_evaluator`); with one, a call whose
            // authority the policy does not establish never reaches the provider *through this
            // gateway*. A route-level preflight, not enforcement at the effect
            // (`docs/design/action-envelope-ae0.md` §7).
            #[cfg(all(feature = "gateway", feature = "tls"))]
            let preflight = ae_preflight(
                &ctx.agent_ctx,
                caller.as_ref(),
                "tools/call",
                &format!("tool:{name}@{provider_node_id}"),
                &arguments,
                &req["params"],
                ENFORCEMENT_POINT_MCP,
            )
            .await;
            #[cfg(all(feature = "gateway", feature = "tls"))]
            if let Preflight::Refuse(refusal) = &preflight {
                return Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": refusal.json_rpc_code(), "message": refusal.to_string(),
                              "data": refusal.error_data()},
                })).into_response();
            }

            // Item 7: the call carries the auth layer's caller context (never anything the
            // client put in `params`), or is refused — it is never dispatched as the node.
            // Closure plan C2/C3: the presented mandate, and the resource it is presented for, travel
            // to the provider, which verifies them itself.
            let resource_claim = format!("tool:{name}@{provider_node_id}");
            let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
                &ctx.agent_ctx,
                caller.as_ref(),
                provider_node_id,
                std::sync::Arc::from(crate::signal::signal_kind::MCP_INVOKE),
                Bytes::from(tool_req.to_string().into_bytes()),
                Duration::from_secs(30),
                gateway_caller::presented_mandate(&req["params"]),
                Some(&resource_claim),
            ).await;

            // What this gateway actually observed. A timeout is **unknown**, never a negative: the
            // call may well have run (item 1's rule, and the hot invariant). A dispatch the gateway
            // itself refused before sending is the one case where nothing ran and it can say so.
            #[cfg(all(feature = "gateway", feature = "tls"))]
            {
                use super::action_evaluator::Execution;
                let mut observed = observed_execution(&dispatched);
                // A reply that arrived is a completed RPC; whether the tool inside it succeeded is
                // a separate question the consumer reads as `effect: failed`.
                if observed == Execution::Completed
                    && dispatched.as_ref().is_ok_and(|b| reply_reports_failure(b))
                {
                    observed = Execution::Failed;
                }
                ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
            }

            match dispatched {
                Ok(reply_bytes) => {
                    let resp: serde_json::Value = serde_json::from_slice(&reply_bytes)
                        .unwrap_or_else(|_| json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32603, "message": "tool returned invalid JSON"},
                        }));
                    Json(resp).into_response()
                }
                Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32000, "message": "tool invocation timed out"},
                })).into_response(),
                Err(e) => Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": e.json_rpc_code(), "message": e.to_string(),
                              "data": {"reason": e.reason()}},
                })).into_response(),
            }
        }

        _ => Json(json!({
            "jsonrpc": "2.0", "id": id,
            "error": {"code": -32601, "message": format!("method not found: {method}")},
        })).into_response(),
    }
}

// ── Language-bridge gateway handlers ─────────────────────────────────────────
//
// These seven endpoints form the HTTP sidecar API for Python/TypeScript agents.
// All inputs and outputs use JSON. Binary payloads are base64-encoded.

/// `POST /gateway/capability/advertise`
///
/// Advertises a capability on behalf of a language-bridge agent. The
/// returned `handle_id` must be supplied to `DELETE /gateway/capability/{id}`
/// to retract the advertisement (tombstone the KV entry).
///
/// Request body:
/// ```json
/// { "ns": "compute", "name": "gpu",
///   "interval_secs": 30,
///   "lease_secs": 90,
///   "attributes": { "model": "A100" },
///   "authorized_callers": ["orchestrator"] }
/// ```
/// Response: `{ "handle_id": "<uuid>" }`
///
/// `lease_secs` (optional) binds the advertisement to the *client's* liveness,
/// not this node's: the caller must `POST /gateway/capability/{handle_id}/heartbeat`
/// within every `lease_secs` window or the advert is retracted (tombstoned) as if
/// DELETEd. Beat at `lease_secs / 3` for margin — mirroring the mesh's own 3×
/// evaporation convention. Without it, the refresh task keeps the advert fresh
/// until DELETE or node shutdown — which outlives a crashed bridge client.
async fn gw_cap_advertise(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::capability::{Capability, CapValue};

    let ns   = match body["ns"].as_str()   { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing ns"}))).into_response() };
    let name = match body["name"].as_str() { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing name"}))).into_response() };
    let interval_secs = body["interval_secs"].as_u64().unwrap_or(30);

    let mut cap = Capability::new(ns.as_str(), name.as_str());

    if let Some(attrs) = body["attributes"].as_object() {
        for (k, v) in attrs {
            let cv = match v {
                serde_json::Value::String(s) => CapValue::Text(Arc::from(s.as_str())),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() { CapValue::Integer(i) }
                    else if let Some(f) = n.as_f64() { CapValue::Float(f) }
                    else { continue }
                }
                serde_json::Value::Bool(b) => CapValue::Bool(*b),
                _ => continue,
            };
            cap = cap.with(k.as_str(), cv);
        }
    }

    if let Some(callers) = body["authorized_callers"].as_array() {
        let list: Vec<Arc<str>> = callers.iter()
            .filter_map(|v| v.as_str())
            .map(Arc::from)
            .collect();
        cap = cap.with_authorized_callers(list);
    }

    let interval = Duration::from_secs(interval_secs.max(1));
    let kv_key: Arc<str> = Arc::from(
        format!("cap/{}/{}/{}", ctx.agent_ctx.node_id, cap.namespace, cap.name).as_str()
    );
    let cap_arc = Arc::new(cap);
    let payload_fn: mycelium_core::kv_persist::PersistPayloadFn = {
        let cap = Arc::clone(&cap_arc);
        Arc::new(move || cap.encode())
    };

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let shutdown_rx = ctx.shutdown_rx.clone();
    tokio::spawn(mycelium_core::kv_persist::run_kv_persist_task(
        Arc::clone(&ctx.agent_ctx.core), cancel_rx, shutdown_rx, kv_key, interval, payload_fn, None,
    ));

    let handle_id = new_handle_id();

    // Lease mode: the watchdog retracts through the same path as DELETE (map
    // removal drops the cancel sender). `remove` returning `None` means the
    // caller already retracted — exit without noise.
    let heartbeat = body["lease_secs"].as_u64().map(|secs| lease_watchdog(&ctx, &handle_id, secs));

    ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner())
        .insert(handle_id.clone(), GatewayCapHandle { _cancel: cancel_tx, heartbeat, _held: None });

    Json(json!({ "handle_id": handle_id })).into_response()
}

/// An opaque gateway handle id. One mint for every handle kind, so the replay inventory counts
/// one nondeterministic site, not one per route.
fn new_handle_id() -> String {
    format!("{:x}", fastrand::u128(..))
}

/// The lease watchdog every gateway handle with `lease_secs` shares: a full window without a
/// heartbeat removes the handle from `gateway_caps`, which retracts whatever it holds — exactly as
/// `DELETE` would.
fn lease_watchdog(ctx: &HttpCtx, handle_id: &str, secs: u64) -> Arc<Notify> {
    let lease = Duration::from_secs(secs.max(1));
    let hb = Arc::new(Notify::new());
    let watchdog_hb = Arc::clone(&hb);
    let caps = Arc::clone(&ctx.gateway_caps);
    let hid = handle_id.to_string();
    let mut wshutdown = ctx.shutdown_rx.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = wshutdown.wait_for(|v| *v) => return,
                beat = tokio::time::timeout(lease, watchdog_hb.notified()) => {
                    if beat.is_ok() { continue; }
                    if caps.lock().unwrap_or_else(|e| e.into_inner()).remove(&hid).is_some() {
                        warn!(handle = %hid, "gateway capability lease expired without heartbeat — retracting");
                    }
                    return;
                }
            }
        }
    });
    hb
}

/// `POST /gateway/units/declare` — an SDK agent's unit file (design-time-tooling.md Q2).
///
/// Body: `{"toml": "<the unit file>", "interval_secs"?: n, "lease_secs"?: n}`. The node parses and
/// validates the file with its own loader — the SDKs carry text, never a second parser — then
/// declares its `[[capability]]`, `[[requirement]]` and `[[group]]` sections under **one handle**
/// that `DELETE /gateway/capability/{handle_id}` retracts and the heartbeat route renews (with
/// `lease_secs`, a missed window retracts it). Refused by name: an unparsable or invalid file
/// (400), and the hosting sections — `[hosts]`, `[[presence]]`, `[[activation]]`, `[[serve]]` (422) — which are
/// a stem's: an SDK agent hosts nothing. `[[lane]]`, `[[mandate]]` and `[[rule]]` are accepted as
/// declarations and reported in `not_enforced`, as the stem reports them.
async fn gw_units_declare(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(text) = body["toml"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing toml"}))).into_response();
    };
    let units = match crate::NodeCapabilityConfig::from_toml_str(text) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e.to_string()}))).into_response(),
    };
    let mut hosting = Vec::new();
    if units.hosts.is_some() { hosting.push("[hosts]"); }
    if !units.presence.is_empty() { hosting.push("[[presence]]"); }
    if !units.activations.is_empty() { hosting.push("[[activation]]"); }
    if !units.serves.is_empty() { hosting.push("[[serve]]"); }
    if !hosting.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
            "error": "hosting sections",
            "detail": format!("{} belong to a stem (mycelium-stem): an SDK agent hosts nothing", hosting.join(", ")),
        }))).into_response();
    }
    // Convert everything before declaring anything: a refusal leaves nothing half-declared.
    let filters = match units.requirements.iter().map(|r| r.to_filter()).collect::<Result<Vec<_>, _>>() {
        Ok(f) => f,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e}))).into_response(),
    };
    // A group under a membership intent is governed: its definition (its eligibility filter) decides which nodes
    // the governor can elect into it, so `cap:write` does not redefine it (#572's review).
    let governed: Vec<&str> = units.groups.iter().map(|g| g.name.as_str()).filter(|n| is_governed_group(&ctx.agent_ctx, n)).collect();
    if !governed.is_empty() {
        return (StatusCode::FORBIDDEN, Json(json!({
            "ok": false, "error": "governed_group",
            "message": format!("{} under a membership intent: a governed group's definition is not redefined through units/declare",
                               governed.join(", ")),
        }))).into_response();
    }
    let defs = match units.groups.iter().map(|g| g.to_def().map(|d| (g.name.clone(), d))).collect::<Result<Vec<_>, _>>() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid unit file", "detail": e}))).into_response(),
    };
    let interval = Duration::from_secs(body["interval_secs"].as_u64().unwrap_or(30).max(1));
    let caps = crate::agent::CapabilitiesHandle { ctx: Arc::clone(&ctx.agent_ctx) };
    let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
    for c in &units.capabilities {
        held.push(Box::new(caps.advertise_capability(c.build_capability_public(), Duration::from_secs(c.ttl_secs.max(1)))));
    }
    for f in filters {
        held.push(Box::new(caps.declare_requirement(f, interval)));
    }
    for (name, def) in defs {
        held.push(Box::new(caps.define_capability_group(name.as_str(), def, interval)));
    }
    let mut not_enforced = Vec::new();
    if !units.lanes.is_empty() { not_enforced.push("[[lane]]"); }
    if !units.mandates.is_empty() { not_enforced.push("[[mandate]]"); }
    if !units.rules.is_empty() { not_enforced.push("[[rule]]"); }

    let handle_id = new_handle_id();
    let heartbeat = body["lease_secs"].as_u64().map(|secs| lease_watchdog(&ctx, &handle_id, secs));
    let (cancel_tx, _cancel_rx) = oneshot::channel::<()>();
    ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner())
        .insert(handle_id.clone(), GatewayCapHandle { _cancel: cancel_tx, heartbeat, _held: Some(Box::new(held)) });
    Json(json!({
        "handle_id": handle_id,
        "principal": units.principal,
        "declared": {
            "capabilities": units.capabilities.len(),
            "requirements": units.requirements.len(),
            "groups": units.groups.len(),
        },
        "not_enforced": not_enforced,
    })).into_response()
}

/// `POST /gateway/capability/{handle_id}/heartbeat`
///
/// Renews the lease on a capability advertised with `lease_secs`. `404` for an
/// unknown or already-retracted handle (a client seeing this should re-advertise);
/// `409` when the handle was advertised without `lease_secs` and has no lease.
async fn gw_cap_heartbeat(
    Path(handle_id): Path<String>,
    State(ctx):      State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let guard = ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(&handle_id) {
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "handle not found" }))).into_response(),
        Some(GatewayCapHandle { heartbeat: None, .. }) =>
            (StatusCode::CONFLICT, Json(json!({ "error": "handle has no lease (advertised without lease_secs)" }))).into_response(),
        Some(GatewayCapHandle { heartbeat: Some(hb), .. }) => {
            hb.notify_one();
            Json(json!({ "ok": true })).into_response()
        }
    }
}

/// `DELETE /gateway/capability/{handle_id}`
///
/// Retracts a previously-advertised capability. Drops the cancel sender,
/// which causes the persist task to tombstone the KV entry.
async fn gw_cap_drop(
    Path(handle_id): Path<String>,
    State(ctx):      State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let removed = ctx.gateway_caps.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle_id).is_some();
    if removed {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "error": "handle not found" }))).into_response()
    }
}

/// `GET /gateway/capability/resolve?ns=X&name=Y[&caller_id=Z]`
///
/// Snapshot filter-match over the local `cap/` KV view. If `caller_id` is
/// supplied, capabilities with non-empty `authorized_callers` are filtered
/// to only those that list the caller's identity.
#[derive(Deserialize)]
struct ResolveQuery {
    ns:        String,
    name:      String,
    caller_id: Option<String>,
}

async fn gw_cap_resolve(
    Query(q):   Query<ResolveQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::{CallerContext, CapFilter, Capability};

    let filter     = CapFilter::new(q.ns.as_str(), q.name.as_str());
    let caller_ctx = match q.caller_id {
        Some(id) => CallerContext::for_caller(id.as_str()),
        None     => CallerContext::unrestricted(),
    };

    let mut results = Vec::new();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, "cap/") {
        if super::capability_ops::is_cap_locality_key(&key) { continue; }
        let Some((node_id, _ns, _name)) =
            super::capability_ops::parse_cap_key_or_warn("cap/", &key)
            else { continue };
        let Some(cap) = Capability::decode(&bytes) else { continue };
        if filter.matches(&cap) && caller_ctx.can_see(&cap) {
            let attrs: serde_json::Map<String, serde_json::Value> = cap.attributes.iter()
                .map(|(k, v)| (k.as_ref().to_string(), capvalue_to_json(v)))
                .collect();
            results.push(json!({
                "node_id":    node_id.to_string(),
                "ns":         cap.namespace.as_ref(),
                "name":       cap.name.as_ref(),
                "attributes": attrs,
            }));
        }
    }

    Json(json!({ "providers": results })).into_response()
}

fn capvalue_to_json(v: &crate::capability::CapValue) -> serde_json::Value {
    use crate::capability::CapValue;
    match v {
        CapValue::Text(s)    => serde_json::Value::String(s.as_ref().to_string()),
        CapValue::Integer(n) => json!(n),
        CapValue::Float(f)   => json!(f),
        CapValue::Bool(b)    => json!(b),
        CapValue::Version(v) => serde_json::Value::String(format!("{}.{}.{}", v[0], v[1], v[2])),
    }
}

/// `POST /gateway/signal/emit`
///
/// Fires a signal into the mesh. `scope` is `"cluster"` (every node; default), `"group:NAME"`,
/// or `"node:IP:PORT"`. `"system"` is still accepted as a deprecated alias for `"cluster"`.
/// `payload_b64` is the base64-encoded signal payload.
async fn gw_signal_emit(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    use crate::signal::SignalScope;

    let kind = match body["kind"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing kind"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }

    let scope_str = body["scope"].as_str().unwrap_or("cluster");
    // "cluster" is the name; "system" stays accepted as a deprecated alias (2026-07-10 rename).
    let scope = if scope_str == "cluster" || scope_str == "system" {
        SignalScope::Cluster
    } else if let Some(rest) = scope_str.strip_prefix("group:") {
        SignalScope::Group(Arc::from(rest))
    } else if let Some(rest) = scope_str.strip_prefix("node:") {
        match rest.parse::<crate::node_id::NodeId>() {
            Ok(nid) => SignalScope::Individual(nid),
            Err(_)  => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid node id"}))).into_response(),
        }
    } else {
        // Reject an unrecognized scope instead of silently widening it to a cluster-wide broadcast:
        // a typo'd prefix (`grp:`, `individual:`) would otherwise emit to the WHOLE cluster rather
        // than the intended narrow scope (audit 2026-07-15 pass 2).
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "unknown scope; expected \"cluster\" | \"group:<name>\" | \"node:<id>\""
        }))).into_response();
    };

    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // A raw emission carries the client's bytes verbatim with *this node* as the sender, so a
    // client-supplied caller-context frame would verify as this gateway's own envelope. The frame is
    // constructed by the auth layer and never by a request body: refuse rather than emit.
    // Found by the Phase-C adversarial audit (items 1+2+7).
    if super::gateway_caller::carries_caller_frame(&payload) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "payload carries a caller-context frame; that context is constructed by the gateway, not supplied"
        }))).into_response();
    }

    // Same code path as GossipAgent::emit — local delivery + gossip fan-out
    let ok = super::helpers::emit_signal(&ctx.agent_ctx, kind, scope, payload);
    Json(json!({ "ok": ok })).into_response()
}

/// `GET /gateway/signal/sse/{kind}` — SSE stream of admitted signals for a kind.
///
/// Each event has `event: <kind>` and `data: {"kind":"…","sender":"…","payload_b64":"…","nonce":…}`;
/// `kind` repeats the event name (since 2.24.0) so a client reading the body alone is not wrong.
async fn gw_signal_sse(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Response {
    // Closure plan C1, the SSE half: a protected kind can be observed no more than it can be sent
    // on a raw route (see `signal_sse_handler`).
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let rx = ctx.agent_ctx.signal_handlers.register_with_capacity(
        Arc::from(kind.as_str()),
        256,
    );

    let stream = ReceiverStream::new(rx).map(|sig: crate::signal::Signal| {
        use base64::Engine as _;
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&sig.payload);
        let data = json!({
            "kind":        sig.kind.as_ref(),
            "sender":      sig.sender.to_string(),
            "payload_b64": payload_b64,
            "nonce":       sig.nonce,
        });
        Ok::<_, Infallible>(Event::default()
            .event(sig.kind.as_ref())
            .data(data.to_string()))
    });

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// `GET /gateway/demand?ns=X&name=Y`
///
/// Returns the demand-pressure snapshot for a capability filter.
#[derive(Deserialize)]
struct DemandQuery { ns: String, name: String }

async fn gw_demand(
    Query(q):   Query<DemandQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;

    let filter   = CapFilter::new(q.ns.as_str(), q.name.as_str());
    let kv       = &ctx.agent_ctx.kv_state;

    let providers = crate::store::scan_kv_prefix(kv, "cap/")
        .into_iter()
        .filter(|(k, v)| {
            if super::capability_ops::is_cap_locality_key(k) { return false; }
            crate::capability::Capability::decode(v)
                .map(|c| filter.matches(&c))
                .unwrap_or(false)
        })
        .count();

    let requirers = crate::store::scan_kv_prefix(kv, "req/")
        .into_iter()
        .filter(|(_, v)| {
            crate::capability::CapFilter::decode(v)
                .map(|f| f.namespace == filter.namespace && f.name == filter.name)
                .unwrap_or(false)
        })
        .count();

    let pressure = (requirers as f64) / (providers.max(1) as f64);

    Json(json!({
        "ns":              q.ns,
        "name":            q.name,
        "providers":       providers,
        "requirers":       requirers,
        "demand_pressure": pressure,
    })).into_response()
}

// The protected-kind list (`BUILTIN_PROTECTED_RPC_KINDS`) and its predicate live ungated in
// `agent/mod.rs` since the wasm host began refusing them at a component's `mesh.emit` (a build
// without the gateway has that door too).
pub(crate) use super::is_protected_kind;

/// **Closure plan C1.** The gateway's raw routes (`rpc/call`, `scatter`, `signal/emit`,
/// `mailbox/deliver`, `shard/emit`, `overlay/emit_reliable`) take the RPC kind from the request
/// body. Before this, a client with `mesh:write` could send `mcp.invoke` or `skill.invoke` straight
/// to a provider through them, framed with its own principal, and the AE preflight that `/mcp` and
/// `/a2a` run, mandates included, never ran. `llm.invoke` the same, around `llm:invoke`.
///
/// A protected kind is refused `403` with the door to use instead. It is refused whatever the
/// token's scopes (the legacy token holds `*`), and whether or not `compliance` is built in.
///
/// The two signal SSE doors (`/signals/{kind}`, `/gateway/signal/sse/{kind}`) refuse it too: they
/// register on the same handler table the serve routes and the native MCP tools use, and a signal
/// fans to every receiver, so observing a protected kind is reading every protected request's frame.
fn refuse_protected_kind(cfg: &crate::config::GossipConfig, kind: &str) -> Option<axum::response::Response> {
    if !is_protected_kind(cfg, kind) {
        return None;
    }
    let door = match kind {
        k if k == crate::signal::signal_kind::MCP_INVOKE => "/mcp (tools/call)",
        "skill.invoke" => "/a2a",
        k if k == crate::signal::signal_kind::LLM_INVOKE => "/gateway/llm/call",
        _ => "the route your operator publishes for it",
    };
    warn!(kind, "gateway: protected RPC kind refused on a raw route");
    Some((
        StatusCode::FORBIDDEN,
        Json(json!({
            "ok": false,
            "error": "protected_kind",
            "kind": kind,
            "message": format!("`{kind}` is protected work and is not accepted on raw mesh routes; use {door}"),
        })),
    ).into_response())
}

/// `POST /gateway/rpc/call`
///
/// Sends a blocking RPC call to a named node. `payload_b64` is base64.
/// Returns `{ "ok": true, "result_b64": "…" }` or `{ "ok": false, "error": "timeout" }`.
async fn gw_rpc_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);

    let target_str = match body["target"].as_str() {
        Some(s) => s.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing target"}))).into_response(),
    };
    let target: crate::node_id::NodeId = match target_str.parse() {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid target node id"}))).into_response(),
    };

    let method = match body["method"].as_str() {
        Some(m) => Arc::from(m),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing method"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &method) {
        return refused;
    }

    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let timeout_secs = body["timeout_secs"].as_u64().unwrap_or(30);
    let timeout      = Duration::from_secs(timeout_secs.clamp(1, 300));

    match gateway_caller::gateway_rpc_call(&ctx.agent_ctx, caller.as_ref(), target, method, payload, timeout).await {
        Ok(result) => {
            let result_b64 = base64::engine::general_purpose::STANDARD.encode(&result);
            Json(json!({ "ok": true, "result_b64": result_b64 })).into_response()
        }
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => {
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": "timeout" }))).into_response()
        }
        Err(e) => dispatch_refused(e),
    }
}

/// What the preflight decided, and what the caller owes afterwards.
///
/// Three-valued rather than `Option<Refusal>` because a permitted dispatch leaves an obligation: the
/// decision said what was *allowed*, and only the caller can say what then *happened*. Carrying the
/// decided record out is what lets the execution record reuse its identities instead of re-deriving
/// them — a second derivation is a second chance to disagree.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) enum Preflight {
    /// No evaluator is attached. The seam is inert and the gateway behaves as it always did.
    Inert,
    /// Permitted. The caller dispatches, then reports the outcome with
    /// [`ae_record_execution`].
    Proceed(Box<super::action_evaluator::AeEvidence>),
    /// Refused. Nothing is dispatched.
    Refuse(super::action_evaluator::PreflightRefusal),
}

/// The MCP tool-call route, as it is named in every piece of evidence it produces.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) const ENFORCEMENT_POINT_MCP: &str = "gateway:mcp/tools/call";

/// The A2A route. Gated on `a2a` as well: it is referenced only from that module, and an item
/// alive in a build that never uses it is the feature-gated dead-code trap CI checks for.
#[cfg(all(feature = "gateway", feature = "tls", feature = "a2a"))]
pub(crate) const ENFORCEMENT_POINT_A2A: &str = "gateway:a2a";

/// The outbound federated-call route (item 2 row 11). Named separately from `gateway:a2a` because
/// it *is* a different point: `/a2a` is where a partner's call arrives, this is where ours leaves.
/// An operator reading evidence has to be able to tell the two directions apart.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) const ENFORCEMENT_POINT_FEDERATION: &str = "gateway:federation/call";

/// Assemble an [`ActionEnvelope`](super::action_evaluator::ActionEnvelope) from facts this
/// gateway verified and run the evaluator over it. `Some(refusal)` means do not dispatch.
///
/// Identity note: `operation_id` is a **correlation** identity, not authority, so a client may
/// supply it (`params._meta.operation_id`) to make its own retries idempotent downstream — item 1's
/// rule. Everything that *is* authority — the actor, the granted scopes — comes from the auth layer
/// and never from the request body (item 7). The `attempt_id` is minted per dispatch here.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) async fn ae_preflight(
    ctx: &Arc<TaskCtx>,
    caller: Option<&ResolvedPrincipal>,
    operation: &str,
    resource: &str,
    arguments: &serde_json::Value,
    params: &serde_json::Value,
    enforcement_point: &str,
) -> Preflight {
    use super::action_evaluator as ae;
    let Some(evaluator) = ctx.action_evaluator.get() else { return Preflight::Inert };

    // Both evaluator questions asked behind the unwind boundary (AE0 §3): a panicking adapter
    // cannot be used to build the envelope either.
    let Some((wanted, mapping)) = ae::evaluator_facts(evaluator, operation, resource) else {
        warn!(%operation, %resource, "AE preflight: evaluator panicked while reporting its facts");
        return Preflight::Refuse(ae::PreflightRefusal::NotEstablished(
            ae::Decision::indeterminate("evaluator panicked while reporting its facts", ""),
        ));
    };

    // Only the argument names the policy declared cross into the envelope — and thence into the
    // evidence record. Everything else stays in the request (AE0 §5).
    let mut selected = serde_json::Map::new();
    if let Some(obj) = arguments.as_object() {
        for name in &wanted {
            if let Some(v) = obj.get(name) {
                selected.insert(name.clone(), v.clone());
            }
        }
    }

    let operation_id = params["_meta"]["operation_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("gw:{}/{:016x}", ctx.node_id, fastrand::u64(..)));
    let attempt_id = format!("{operation_id}/{:08x}", fastrand::u32(..));
    let (actor, scopes) = match caller {
        Some(c) => (c.principal.clone(), c.scopes.clone()),
        None => (gateway_caller::PRINCIPAL_ANONYMOUS.to_string(), Vec::new()),
    };
    // Canonical arguments: serde_json's object serialization is key-ordered for `Map` in its
    // default (BTreeMap) configuration, so the digest is stable for equal arguments.
    let canonical = serde_json::to_vec(arguments).unwrap_or_default();
    // `decision_now_ms`, not `current()`: this value decides whether a mandate has expired and
    // whether a checkpoint is stale. `current()` never reads the wall clock, so on a node whose
    // gossip has gone quiet — the partition case — it freezes and both checks fail OPEN.
    let now_ms = ctx.hlc.decision_now_ms();
    let arguments_digest = ae::arguments_digest(&canonical);

    // Boundary H A1: with an execution authority attached, a presented grant
    // (`params._meta.mandate`) becomes a real mandate finding for this call — checked against the
    // **authenticated** caller, P2's grant checks and A1's execution gate — instead of `None`.
    let assessment = match ctx.execution_authority.get() {
        Some(authority) => {
            let presented: Option<super::gateway_authority::PresentedMandate> =
                serde_json::from_value(params["_meta"]["mandate"].clone()).ok();
            authority.assess(
                &actor,
                operation,
                resource,
                &arguments_digest,
                presented.as_ref(),
                now_ms,
                &super::gateway_member_keys(ctx),
            )
        }
        None => super::gateway_authority::Assessment { binding: None, valid_until_ms: None },
    };
    // A1 rule 3: the envelope can never outlive the mandate it acts under.
    let not_after_ms = match assessment.valid_until_ms {
        Some(until) => now_ms.saturating_add(60_000).min(until),
        None => now_ms.saturating_add(60_000),
    };

    let envelope = ae::ActionEnvelope {
        operation_id,
        attempt_id,
        actor,
        via: ctx.node_id.clone(),
        scopes,
        operation: operation.to_string(),
        resource: resource.to_string(),
        arguments_digest,
        selected_arguments: selected,
        mapping,
        // The expected policy revision comes from the deployment report an operator filed, via
        // `GossipAgent::set_deployed_policy_revision` (AE0 §6). Unset, the seam has no second
        // opinion and the stale-policy check does not fire — which is why leaving it unset means a
        // gateway running a superseded policy cannot be detected.
        expected_policy_revision: ctx.deployed_policy_revision.load_full().map(|r| (*r).clone()),
        issued_at_ms: now_ms,
        not_after_ms,
        // AE1 + Boundary H A1: `None` when no execution authority is attached or no grant was
        // presented — *claims none*, which a rule requiring a mandate reads as **authority not
        // established** (`Indeterminate`), never a denial. With an authority, the finding above.
        mandate: assessment.binding,
    };

    // Decide, then record, *then* dispatch. Both outcomes are sealed: a permit with no record of
    // why is precisely the gap this slice exists to close, so recording only refusals would leave
    // the interesting half invisible — and an evidence stream that omits its permits cannot support
    // any statement about what an agent was allowed to do.
    let outcome = ae::preflight(Some(evaluator), &envelope, now_ms);
    let (decision, execution) = match &outcome {
        Ok(Some(d)) => (d.clone(), ae::Execution::Attempted),
        // No evaluator — unreachable here, one was fetched above. Nothing decided, nothing to
        // record, nothing to refuse.
        Ok(None) => return Preflight::Inert,
        Err(refusal) => (refusal.decision().clone(), ae::Execution::None),
    };

    // Every decision, permits included — the refusal counter below has no denominator on its own,
    // so a denial *rate* is not computable from it and an alert on denial volume fires on traffic
    // growth. Labels are the evidence document's own vocabulary (`DecisionKind`/`MappingKind`), so
    // a reader never has to translate between a dashboard and a record. Both label sets are small
    // closed sets: nothing here is labelled by principal, operation or resource, whose value spaces
    // grow with traffic.
    #[cfg(feature = "metrics")]
    {
        let verdict = ae::DecisionKind::from(decision.verdict).label();
        let mapping = ae::MappingKind::from(envelope.mapping.status).label();
        metrics::counter!("mycelium_ae_decisions_total", "verdict" => verdict, "mapping" => mapping)
            .increment(1);
    }

    let evidence = ae::AeEvidence::for_decision(&envelope, &decision, execution, enforcement_point);
    if let Err(e) = ae_record(ctx, &evidence).await {
        warn!(actor = %envelope.actor, %operation, %resource,
              "AE preflight: the decision could not be recorded, so the action is refused: {e}");
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_ae_preflight_refusals_total", "reason" => "evidence_not_recorded")
            .increment(1);
        return Preflight::Refuse(ae::PreflightRefusal::NotRecorded(decision));
    }

    match outcome {
        Ok(_) => Preflight::Proceed(Box::new(evidence)),
        Err(refusal) => {
            warn!(actor = %envelope.actor, %operation, %resource,
                  "AE preflight refused: {refusal}");
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_ae_preflight_refusals_total", "reason" => refusal.reason()).increment(1);
            Preflight::Refuse(refusal)
        }
    }
}

/// What this gateway **observed** of a dispatch, in §5's vocabulary.
///
/// The rule that matters is the last arm. A timeout — or any transport error — leaves the call's
/// fate genuinely open: the provider may have run it and failed to answer. Reporting `Failed` there
/// would claim knowledge nobody has, and a consumer reading `failed` will act on it. So the honest
/// answer is `Unknown`, which item 1 names `DeliveryUnknown` and which this project has as a hot
/// invariant: *a timeout is never a negative*.
///
/// The one case where "nothing ran" can be stated is a dispatch this gateway refused before sending.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn observed_execution(
    dispatched: &Result<Bytes, GatewayDispatchError>,
) -> super::action_evaluator::Execution {
    use super::action_evaluator::Execution;
    match dispatched {
        Ok(_) => Execution::Completed,
        Err(GatewayDispatchError::ProviderWithoutContext(_))
        | Err(GatewayDispatchError::ContextTooLarge) => Execution::None,
        Err(_) => Execution::Unknown,
    }
}

/// Did the provider's JSON-RPC reply carry an error?
///
/// A reply that arrived is a *completed* RPC; whether the tool inside it succeeded is a different
/// question, and the consumer's `effect` reads `failed` from this. Unparseable bytes count as failed:
/// something answered, and it was not an answer.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn reply_reports_failure(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .map(|v| v.get("error").is_some())
        .unwrap_or(true)
}

/// Record what became of a dispatch this gateway permitted (AE0 §5's execution record).
///
/// **Why this is not optional.** Without it every permitted call exports as `effect: unknown`, even
/// when this gateway watched the provider answer — so the evidence could say what an agent was
/// *allowed* to do and never what it *did*, which is most of what anyone wants to know.
///
/// The record is appended beside the decision, never over it: `operation_id`, `attempt_id` and the
/// principal are carried through unchanged so a consumer can correlate them, and the decision stands
/// exactly as written.
///
/// A failure to record here does **not** retract the dispatch — it already happened, and pretending
/// otherwise would be the one lie worse than silence. It is logged, and the reference record carries
/// the journal's own state.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) async fn ae_record_execution(
    ctx: &Arc<TaskCtx>,
    preflight: &Preflight,
    execution: super::action_evaluator::Execution,
) {
    let Preflight::Proceed(decided) = preflight else { return };
    if let Err(e) = ae_record(ctx, &decided.as_execution(execution)).await {
        // The effect has happened. The evidence for it has not.
        warn!(
            operation_id = %decided.operation_id,
            attempt_id = %decided.attempt_id,
            "AE: the execution record could not be established: {e}"
        );
    }
}

/// Record one AE decision: **journal first, then a reference into the chain** (AE0 §5).
///
/// The decision document goes to the node-local [journal](super::evidence_journal) — durable,
/// append-only, never gossiped. What is sealed into the tamper-evident chain is an
/// [`AeReference`](super::action_evaluator::AeReference): the verdict, the identities, the policy
/// revision, the catalogue id, and the journal record's **content hash**. Nothing else, because the
/// chain reaches every node and the evidence is not for every node.
///
/// `Err` means *refuse the dispatch*. Which failures refuse is the operator's
/// [`EvidenceProfile`](super::evidence_journal::EvidenceProfile): the strict profile gates the
/// effect on durable evidence, the lenient one proceeds and the reference record says it did.
#[cfg(all(feature = "gateway", feature = "tls"))]
async fn ae_record(
    ctx: &Arc<TaskCtx>,
    evidence: &super::action_evaluator::AeEvidence,
) -> Result<(), String> {
    use super::action_evaluator::{AeReference, EvidenceState};
    use super::evidence_journal::{EvidenceProfile, JournalError};

    let bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;

    let (journal_sha256, state, refuse) = match ctx.evidence_journal.get() {
        // No journal: nothing is recorded, and the reference says exactly that rather than being
        // absent. `with_action_evaluator` warns at attach time so this is never a surprise.
        None => (None, EvidenceState::NotConfigured, None),
        Some(journal) => match journal.append(bytes).await {
            Ok(appended) => (
                Some(super::action_evaluator::hex32(&appended.content_hash)),
                EvidenceState::OnDisk,
                None,
            ),
            Err(e) => {
                // A lost acknowledgement is *unknown*, not failed: the record may be on disk, and
                // claiming failure would assert knowledge nobody has.
                let state = match e {
                    JournalError::DeliveryUnknown => EvidenceState::Unknown,
                    _ => EvidenceState::NotEstablished,
                };
                let refuse =
                    matches!(journal.profile(), EvidenceProfile::Strict).then(|| e.to_string());
                (None, state, refuse)
            }
        },
    };

    ae_seal_reference(ctx, &AeReference::for_evidence(evidence, journal_sha256, state));
    match refuse {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Seal the safe reference record into the tamper-evident chain.
///
/// Best-effort by design: the journal is what gates the effect (AE0 §5 names it, not the chain and
/// not the sink, as the ack-capable contract), and the chain adds ordering and hash-linking on top.
/// A node without a `tls` identity gets a warning, not a refused dispatch — its evidence is still
/// durable, just not chain-linked.
///
/// The audit record's `target` is the **catalogue id**, never the resource: `ae:{catalogue}` is the
/// most specific thing §5 permits to gossip.
#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
fn ae_seal_reference(ctx: &Arc<TaskCtx>, reference: &super::action_evaluator::AeReference) {
    use super::action_evaluator::DecisionKind;
    let detail = match serde_json::to_string(reference) {
        Ok(d) => d,
        Err(e) => {
            warn!("AE: the reference record did not serialise: {e}");
            return;
        }
    };
    let outcome = match reference.decision {
        DecisionKind::Permit => super::audit::AuditOutcome::Success,
        DecisionKind::Deny => super::audit::AuditOutcome::Denied,
        DecisionKind::Indeterminate => super::audit::AuditOutcome::Error,
    };
    if let Err(e) = super::audit::seal_and_write(
        ctx,
        super::audit::AuditAction::Invoke,
        reference.principal.clone(),
        format!("ae:{}", reference.catalogue),
        outcome,
        Some(detail),
    ) {
        warn!("AE: the reference record could not be chained: {e}");
    }
}

/// Without `compliance` there is no audit chain to link into. The journal still holds the evidence.
#[cfg(all(feature = "gateway", feature = "tls", not(feature = "compliance")))]
fn ae_seal_reference(_ctx: &Arc<TaskCtx>, _reference: &super::action_evaluator::AeReference) {}

/// HTTP shape of a refused gateway dispatch (item 7): `412 Precondition Failed` — the
/// precondition being a provider that enforces the caller context, or a context to send.
fn dispatch_refused(e: GatewayDispatchError) -> Response {
    let provider = match &e {
        GatewayDispatchError::ProviderWithoutContext(n) => Some(n.to_string()),
        _ => None,
    };
    let status = match e {
        GatewayDispatchError::ContextTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::PRECONDITION_FAILED,
    };
    (status, Json(json!({ "ok": false, "error": e.reason(), "detail": e.to_string(), "provider": provider })))
        .into_response()
}

// ── KV gateway handlers ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct KvKeyQuery { key: String }

/// `GET /gateway/kv?key=K` — read a single KV entry.
///
/// Returns `{"found": true, "value_b64": "…"}` or `{"found": false}`.
async fn gw_kv_get(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    match ctx.agent_ctx.kv_state.store.pin().get(q.key.as_str()).and_then(|e| e.data.clone()) {
        Some(bytes) => {
            use base64::Engine as _;
            let v = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Json(json!({ "found": true, "value_b64": v })).into_response()
        }
        None => Json(json!({ "found": false })).into_response(),
    }
}

/// `POST /gateway/kv` — write a KV entry.
///
/// Body: `{"key": "…", "value_b64": "…"}`. Returns `{"ok": true, "operation_id", "local_durability",
/// "local_durability_error"?}` — the write's **receipt** (rung 1 always; rung 2 as
/// `local_durability`, the same vocabulary the consensus commits and the SDKs carry). Until
/// 2026-09-26 the route discarded the receipt and answered a bare `{"ok": true}`, so an HTTP or SDK
/// client could not learn rung 2 for an ordinary set (doc-coverage run 17). Additive: `ok` is
/// unchanged.
async fn gw_kv_set(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;

    let key = match body["key"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing key"}))).into_response(),
    };
    if let Some(refused) = refuse_protected_key(&key) {
        return refused;
    }
    // A write must say what it writes. Until 2026-09-24 a missing `value_b64` silently wrote an
    // EMPTY value and answered `{"ok": true}` — so a misspelled field name erased a key and
    // reported success. The overlay test helper had been sending `value` since it was written, and
    // every sentinel it ever wrote was empty; nothing noticed, because the check that consumed them
    // only counted keys. An explicitly empty value is legitimate (`"value_b64": ""`); an *omitted*
    // one must not become one. Tombstoning has its own verb (`DELETE`), so silent-empty had no
    // legitimate caller.
    let value = match body.get("value_b64") {
        Some(serde_json::Value::String(b64)) => {
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(v)  => Bytes::from(v),
                Err(_) => return (StatusCode::BAD_REQUEST,
                                  Json(json!({"error":"invalid base64 in 'value_b64'"}))).into_response(),
            }
        }
        Some(_) => return (StatusCode::BAD_REQUEST,
                           Json(json!({"error":"'value_b64' must be a base64 string"}))).into_response(),
        None    => return (StatusCode::BAD_REQUEST,
                           Json(json!({"error":"missing 'value_b64' (use \"\" for an empty value, or DELETE to tombstone)"}))).into_response(),
    };

    let tc = Arc::clone(&ctx.agent_ctx);
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    match mycelium_core::ops::kv_set_with_receipt(&tc, op, attempt, key, value, None).await {
        Ok(r) => {
            let mut body = json!({
                "ok": true,
                "operation_id": r.operation_id.to_string(),
                "local_durability": r.local_durability.tag(),
            });
            if let Some(reason) = r.local_durability.failure_reason() {
                body["local_durability_error"] = json!(reason);
            }
            Json(body).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                   Json(json!({ "ok": false, "error": e.to_string() }))).into_response(),
    }
}

/// Top-level KV namespaces the substrate or a companion owns (`src/lib.rs` § KV namespace ownership). The
/// raw KV routes refuse them: each has its own door, guarded by its own scope, and a write through
/// `kv:write` would bypass it — a governance intent without `govern:write`, a prompt template without
/// `llm:write`, another node enrolled in a group (a node joins itself), a forged capability or requirement,
/// a forged mailbox sender, a planted `sys/caller-context/` marker, a deleted acceptor ballot (the reviews
/// of #544; the kv:write decision of 2026-10-07). `every_namespace_in_the_table_is_classified_for_the_raw_kv_routes`
/// keeps this in step with the table.
pub(crate) const OWNED_KV_PREFIXES: &[&str] = &[
    "sys/", "consensus/", "grp/", "audit/", "cap/", "req/", "cap-group/", "gcap/", "mailbox/", "tools/",
    "agent/", "svc/", "log/", "clog/", "lock/", "prompts/", "skills/", "installable/", "comp/", "wiki/", "tuple/",
    "facts/", "mandate/", "knowledge/", "rights/", "cn/",
];

/// Namespaces the table assigns to an application writing through the gateway's KV routes — the LangGraph
/// checkpointer's index rows, the mesh manifest (application-owned, `mesh_manifest.rs`) and the schema
/// registry (`publish_schema` from Rust; this is a non-Rust application's door). (`agent/{node}/provision/{item}/error`,
/// an application's provisioning report, is the one owned-namespace subtree the raw routes also accept.)
pub(crate) const APPLICATION_KV_PREFIXES: &[&str] = &["ckpt/", "ckptw/", "manifest/", "schemas/"];

/// Streams under `log/` that a substrate component or companion owns: the commitment net's records, a wiki's
/// durable proposals, `mycelium-reason`'s traces. The log routes take a stream name, so without this they
/// reach those as well (the review of #549).
pub(crate) const OWNED_LOG_STREAMS: &[&str] = &["cn/", "wiki/", "reason/"];

fn refuse_owned_stream(stream: &str) -> Option<axum::response::Response> {
    let owned = OWNED_LOG_STREAMS.iter().any(|p| stream.starts_with(p) || stream == p.trim_end_matches('/'));
    owned.then(|| (
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "protected_stream",
            "message": "this log stream belongs to a component (commitment, wiki, reason) and is written through it" })),
    ).into_response())
}

/// Whether the raw KV routes write `key`: an application key, not one the substrate or a companion owns.
pub(crate) fn raw_kv_writable(key: &str) -> bool {
    if APPLICATION_KV_PREFIXES.iter().any(|p| key.starts_with(p)) {
        return true;
    }
    if let Some(rest) = key.strip_prefix("agent/")
        && let Some((_node, sub)) = rest.split_once('/')
        && sub.starts_with("provision/")
    {
        return true;
    }
    !OWNED_KV_PREFIXES.iter().any(|p| key.starts_with(p) || key == p.trim_end_matches('/'))
}

/// The doors that take a KV key from the request — `POST`/`DELETE /gateway/kv`, `POST /gateway/kv/quorum`, and
/// `POST /gateway/overlay/consistent/set`, which writes the raw key once consensus commits — write application
/// keys only ([`raw_kv_writable`]). An owned key is refused `403` whatever the token's scopes, as
/// [`refuse_protected_kind`] does for RPC kinds, naming the door to use. Layer I still accepts these keys from
/// a peer — detection, not prevention; this is the gateway's door.
fn refuse_protected_key(key: &str) -> Option<axum::response::Response> {
    if raw_kv_writable(key) {
        return None;
    }
    warn!(key, "gateway: an owned KV key refused on a raw KV route");
    let door = if key.starts_with("sys/govern/") {
        "governance intents are published through /gateway/govern/{tuning,timing,membership} (govern:write)"
    } else if key.starts_with("sys/topology-override/") {
        "the topology override is set through POST /gateway/govern/topology-override (govern:write)"
    } else if key.starts_with("grp/") {
        "a node joins a group itself: POST /gateway/mesh/group on that node (mesh:write), or POST /gateway/govern/group (govern:write) for a group under a membership intent"
    } else if key.starts_with("prompts/") {
        "prompt templates are written through /gateway/prompts/{ns}/{name} (llm:write)"
    } else if key.starts_with("log/") || key.starts_with("clog/") {
        "logs are appended through /gateway/overlay/log/append (consensus:write)"
    } else if key.starts_with("cap/") {
        "capabilities are advertised through /gateway/capability/advertise (cap:write)"
    } else if key.starts_with("req/") {
        "requirements are declared through POST /gateway/units/declare (cap:write)"
    } else if key.starts_with("mailbox/") {
        "mailbox entries are delivered through POST /gateway/mailbox/deliver (mesh:write), which records the sender"
    } else if key.starts_with("installable/") {
        "catalogue lines are published through POST /gateway/artifacts/publish (artifact:publish, signature-checked) where mycelium-wasm-host's routes are merged, or by a librarian"
    } else if key.starts_with("lock/") {
        "locks are taken through /gateway/overlay/lock/acquire (consensus:write)"
    } else {
        "this namespace is written by the substrate or the companion that owns it, not through the KV routes \
         (src/lib.rs § KV namespace ownership); the KV routes write application keys"
    };
    Some((
        StatusCode::FORBIDDEN,
        Json(json!({ "ok": false, "error": "protected_key", "message": door })),
    ).into_response())
}

/// `DELETE /gateway/kv?key=K` — tombstone a KV entry.
///
/// Returns `{"ok": true}`.
async fn gw_kv_delete(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_protected_key(&q.key) {
        return refused;
    }
    kv_write(&ctx.agent_ctx, Arc::from(q.key.as_str()), Bytes::new(), true);
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct KvKeysQuery { prefix: Option<String> }

/// `GET /gateway/kv/keys?prefix=P` — list live KV keys, optionally filtered by prefix.
///
/// Returns `{"keys": ["key1", "key2", …]}`.
async fn gw_kv_keys(
    Query(q):   Query<KvKeysQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let keys: Vec<String> = if let Some(ref pfx) = q.prefix {
        crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, pfx.as_str())
            .into_iter()
            .map(|(k, _)| k.as_ref().to_string())
            .collect()
    } else {
        ctx.agent_ctx.kv_state.store.pin()
            .iter()
            .filter(|(_, v)| v.data.is_some())
            .map(|(k, _)| k.as_ref().to_string())
            .collect()
    };
    Json(json!({ "keys": keys })).into_response()
}

/// `POST /gateway/kv/quorum` — write + wait for peer acknowledgements.
///
/// Since item 1 PR 4b this route **asks** each peer whether it holds the operation, rather than
/// watching the gossip stream for evidence the substrate cannot carry (`contracts-receipts.md`
/// §1a — before 4b it timed out however widely the write spread). `acks_received` counts peers that
/// answered *persisted*: their store holds this exact stamp and content and their WAL `fdatasync`
/// returned `Ok`, so they hold it across their own restart.
///
/// On timeout, `unknown_peers` counts those that did not answer *persisted* — **unknown, never
/// "did not persist"**. A peer may be unreachable, mid-restart, on a build without the handler, or
/// already holding a newer value for the key. The write itself is applied and gossiped either way,
/// so a timeout here never means the value was not written.
///
/// Request body:
/// ```json
/// { "key": "...", "value_b64": "<base64>", "min_acks": 2, "timeout_secs": 5.0 }
/// ```
/// Success: `{ "ok": true, "acks_received": 2 }`
/// Timeout: `{ "ok": false, "error": "timeout", "acks_received": 0 }`
#[derive(Deserialize)]
struct KvQuorumBody {
    key:         String,
    #[serde(default)]
    value_b64:   String,
    min_acks:    usize,
    #[serde(default = "default_quorum_timeout")]
    timeout_secs: f64,
}

fn default_quorum_timeout() -> f64 { 5.0 }

async fn gw_kv_quorum(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<KvQuorumBody>,
) -> impl IntoResponse {
    use base64::Engine as _;

    if let Some(refused) = refuse_protected_key(&body.key) {
        return refused;
    }
    let value = match base64::engine::general_purpose::STANDARD.decode(&body.value_b64) {
        Ok(v)  => Bytes::from(v),
        Err(_) => return (StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid base64" }))).into_response(),
    };

    let key: Arc<str> = Arc::from(body.key.as_str());
    // `try_from_secs_f64`, NOT `from_secs_f64`: the latter PANICS on a negative, non-finite, or
    // over-large value. `timeout_secs` is an untrusted client `f64`, and with `panic = "abort"` in
    // the release profile a panic aborts the whole node — so `{"timeout_secs":-1}` on the (default
    // loopback-open) gateway was an unauthenticated single-request node kill (audit 2026-07-15 pass 2).
    let timeout = match Duration::try_from_secs_f64(body.timeout_secs) {
        Ok(d)  => d,
        Err(_) => return (StatusCode::BAD_REQUEST,
            Json(json!({ "error": "timeout_secs must be a finite, non-negative number" }))).into_response(),
    };
    let tc             = Arc::clone(&ctx.agent_ctx);

    if body.min_acks == 0 {
        kv_write(&tc, key, value, false);
        return Json(json!({ "ok": true, "acks_received": 0 })).into_response();
    }

    // Item 1 PR 4b: write, then **ask** the peers. The previous implementation installed a
    // tracker and watched the gossip stream for evidence of its own write, which this substrate
    // cannot carry (contracts-receipts.md §1a), so it timed out however widely the write spread.
    let op = mycelium_core::receipt::OperationId::generate(&tc.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    let receipt = match mycelium_core::ops::kv_set_with_receipt(
        &tc, op, attempt, Arc::clone(&key), value, None,
    ).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": e.to_string() }))).into_response(),
    };
    let replica = super::replica_sync::collect(
        &tc,
        super::replica_sync::Query {
            stamp: receipt.stamp,
            content_hash: receipt.content_hash,
            key: Arc::clone(&key),
        },
        timeout,
    )
    .await;
    let acks = replica.persisted_by.len();
    let result: Result<usize, ()> = if acks >= body.min_acks { Ok(acks) } else { Err(()) };

    match result {
        Ok(n)  => Json(json!({ "ok": true, "acks_received": n })).into_response(),
        Err(_) => Json(json!({
            "ok": false,
            "error": "timeout",
            "acks_received": acks,
            // Peers that did not answer "persisted" — unknown, never "did not persist".
            "unknown_peers": replica.missing.len(),
        })).into_response(),
    }
}

/// Applies a KV write (set or delete) and fans out to gossip peers.
fn kv_write(ctx: &Arc<TaskCtx>, key: Arc<str>, value: Bytes, tombstone: bool) -> bool {
    use crate::framing::{dispatch_gossip_try_send, make_gossip_update, ForwardHint, WireMessage};
    use crate::store::apply_and_notify;
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, key, value, tombstone, &ctx.hlc);
    apply_and_notify(&ctx.kv_state, &update); // apply, then persist (persistence.rs invariant 1)
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(crate::framing::sync_entry_from(&update));
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        WireMessage::Data(update),
        ctx.node_id.id_hash(),
        ForwardHint::All,
        &ctx.kv_state.dropped_frames,
    )
}

// ── RPC serve / respond gateway handlers ─────────────────────────────────────

/// `GET /gateway/rpc/serve/{kind}` — SSE stream of incoming RPC requests.
///
/// Streams requests as `{"nonce_hex": "…", "sender": "IP:PORT", "payload_b64": "…"}`.
/// The receiver must call `POST /gateway/rpc/respond` with the same `nonce_hex` and
/// `sender` to complete the round-trip — **the same principal** that opened this stream: each
/// streamed request is recorded against it (`record_served_rpc`), and `rpc/respond` answers
/// nothing else. `mesh:serve` alone bound neither a kind nor a request, so any holder could
/// answer any in-flight call it learned a nonce for.
async fn gw_rpc_serve(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = ctx.agent_ctx.signal_handlers.register_with_capacity(
        Arc::from(kind.as_str()),
        256,
    );
    let principal: Arc<str> = Arc::from(
        caller.map(|Extension(c)| c.principal).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string()).as_str(),
    );

    let http_ctx = Arc::clone(&ctx);
    let agent_ctx = Arc::clone(&ctx.agent_ctx);
    // Async, because closure plan C3's provider check runs the action preflight (and its journal
    // write) before a request is streamed to the SDK agent that serves it.
    let stream = futures_util::StreamExt::filter_map(ReceiverStream::new(rx), move |sig: crate::signal::Signal| {
      let agent_ctx = Arc::clone(&agent_ctx);
      let http_ctx = Arc::clone(&http_ctx);
      let principal = Arc::clone(&principal);
      async move {
        use base64::Engine as _;
        if sig.payload.len() < 8 { return None; }
        let req = super::rpc::RpcRequest::from(sig);
        // Item 7: a gateway-dispatched request carries a caller context. Verified, it is
        // handed to the served handler as `caller`; refused, the request is dropped here (the
        // served handler never sees a forged principal, and the caller times out).
        let caller = match gateway_caller::verify(&agent_ctx, &req) {
            Ok(Some(c)) => Some(json!({
                "principal": c.principal,
                "via":       c.via.to_string(),
                "scopes":    c.scopes,
                "attested":  matches!(c.attestation, super::gateway_caller::CallerAttestation::Signed { .. }),
            })),
            Ok(None) => None,
            Err(e) => {
                warn!(kind = %req.kind(), sender = %req.sender(), "rpc/serve: caller context refused: {e}");
                return None;
            }
        };
        // Closure plan C3: a refused protected call is answered here and never streamed.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        match super::provider_enforcement::check(&agent_ctx, &req).await {
            // C4: the admission is parked until the SDK agent replies through `/rpc/respond`.
            Ok(admission) => super::provider_enforcement::park(&agent_ctx, req.sender(), req.nonce(), admission),
            Err(refusal) => {
                warn!(kind = %req.kind(), sender = %req.sender(), reason = %refusal.reason,
                      "rpc/serve: refused by provider enforcement");
                super::rpc::rpc_respond_ctx(&agent_ctx, &req, Bytes::from(refusal.rpc_body()));
                return None;
            }
        }
        // Handed to this principal: the only one `rpc/respond` will take an answer from.
        record_served_rpc(&http_ctx, req.sender(), req.nonce(), &principal);
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(req.payload());
        let mut data = json!({
            "nonce_hex":   format!("{:016x}", req.nonce()),
            "sender":      req.sender().to_string(),
            "payload_b64": payload_b64,
        });
        if let Some(c) = caller {
            data["caller"] = c;
        }
        Some(Ok(Event::default()
            .event(req.kind().as_ref())
            .data(data.to_string())))
      }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `POST /gateway/rpc/respond` — send a reply to an in-flight RPC request.
///
/// Body: `{"nonce_hex": "…", "sender": "IP:PORT", "result_b64": "…"}`.
/// Returns `{"ok": true}`. Only a request that `rpc/serve` streamed to **this principal** and
/// that is not yet answered is accepted; anything else — another principal's request, a nonce never
/// streamed, a second answer, one past the gateway's RPC ceiling — is `403 unserved_request` and
/// nothing is emitted or released.
async fn gw_rpc_respond(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    use crate::signal::SignalScope;

    let nonce_hex = match body["nonce_hex"].as_str() {
        Some(s) => s,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing nonce_hex"}))).into_response(),
    };
    let nonce = match u64::from_str_radix(nonce_hex, 16) {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid nonce_hex"}))).into_response(),
    };
    let sender: crate::node_id::NodeId = match body["sender"].as_str().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing or invalid sender"}))).into_response(),
    };
    let result = if let Some(b64) = body["result_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 result"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // The request must have been handed to this principal on its own serve stream. Before this
    // check, `mesh:serve` bound to neither a kind nor a request: any holder could answer — and
    // pre-empt — any in-flight call whose nonce it learned, and release its parked admission.
    let principal = caller.map(|Extension(c)| c.principal)
        .unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    if !take_served_rpc(&ctx, &sender, nonce, &principal) {
        warn!(nonce = %nonce_hex, %sender, %principal, "rpc/respond: refused — not a request this principal was handed");
        return (StatusCode::FORBIDDEN, Json(json!({
            "ok": false,
            "error": "unserved_request",
            "message": "rpc/respond answers only a request your own rpc/serve stream delivered and that is not yet answered",
        }))).into_response();
    }

    // Closure plan C4: the SDK agent has replied, so the call is no longer in flight.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    super::provider_enforcement::release_parked(&ctx.agent_ctx, &sender, nonce);

    let mut buf = BytesMut::with_capacity(8 + result.len());
    buf.put_u64_le(nonce);
    buf.put(result);
    super::helpers::emit_signal(
        &ctx.agent_ctx,
        Arc::from(crate::signal::signal_kind::RPC_RESULT),
        SignalScope::Individual(sender),
        buf.freeze(),
    );

    Json(json!({ "ok": true })).into_response()
}

// ── Scatter-gather gateway handler ────────────────────────────────────────────

/// `POST /gateway/scatter` — fan-out RPC to multiple targets, collect replies.
///
/// Body:
/// ```json
/// {
///   "targets":       ["IP:PORT", …],
///   "method":        "signal-kind",
///   "payload_b64":   "…",
///   "timeout_secs":  10,
///   "min_ok":        1
/// }
/// ```
/// Returns `{"ok": true, "replies": [{"sender": "…", "result_b64": "…"}, …]}` once
/// `min_ok` replies arrive, or `{"ok": false, "error": "…", "replies": […]}` on timeout.
async fn gw_scatter(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);

    let targets: Vec<crate::node_id::NodeId> = match body["targets"].as_array() {
        Some(arr) => arr.iter()
            .filter_map(|v| v.as_str()?.parse().ok())
            .collect(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing targets"}))).into_response(),
    };
    let method: Arc<str> = match body["method"].as_str() {
        Some(m) => Arc::from(m),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing method"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &method) {
        return refused;
    }
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    let timeout_secs = body["timeout_secs"].as_u64().unwrap_or(10).clamp(1, 300);
    let timeout      = Duration::from_secs(timeout_secs);
    let min_ok       = body["min_ok"].as_u64().unwrap_or(1) as usize;

    let mut js: tokio::task::JoinSet<(crate::node_id::NodeId, Result<Bytes, GatewayDispatchError>)>
        = tokio::task::JoinSet::new();
    for target in targets {
        let c = Arc::clone(&ctx.agent_ctx);
        let k = Arc::clone(&method);
        let p = payload.clone();
        let t = target.clone();
        let who = caller.clone();
        js.spawn(async move {
            let res = gateway_caller::gateway_rpc_call(&c, who.as_ref(), t.clone(), k, p, timeout).await;
            (t, res)
        });
    }

    let mut replies: Vec<serde_json::Value> = Vec::new();
    // Item 7: a target refused before dispatch (no caller-context marker) is reported per
    // target, not folded into "insufficient replies".
    let mut refused: Vec<serde_json::Value> = Vec::new();
    while let Some(res) = js.join_next().await {
        match res {
            Ok((nid, Ok(bytes))) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                replies.push(json!({ "sender": nid.to_string(), "result_b64": b64 }));
                if replies.len() >= min_ok {
                    js.abort_all();
                    break;
                }
            }
            Ok((nid, Err(e))) if !matches!(e, GatewayDispatchError::Rpc(_)) => {
                refused.push(json!({ "target": nid.to_string(), "error": e.reason(), "detail": e.to_string() }));
            }
            _ => {}
        }
    }

    if replies.len() >= min_ok {
        Json(json!({ "ok": true, "replies": replies, "refused": refused })).into_response()
    } else {
        (StatusCode::GATEWAY_TIMEOUT,
         Json(json!({ "ok": false, "error": "insufficient replies", "replies": replies, "refused": refused })))
            .into_response()
    }
}

// ── Mailbox gateway handlers ──────────────────────────────────────────────────

/// `GET /gateway/mailbox/{kind}` — SSE stream of mailbox events for this node.
///
/// Streams events as `{"sender": "IP:PORT", "kind": "…", "payload_b64": "…"}`.
/// The subscription is torn down when the client disconnects.
async fn gw_mailbox_subscribe(
    Path(kind):  Path<String>,
    State(ctx):  State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let kind_arc: Arc<str> = Arc::from(kind.as_str());
    let (handle, rx) = super::mailbox::open_mailbox_ctx(
        Arc::clone(&ctx.agent_ctx),
        &ctx.agent_ctx.node_id,
        Arc::clone(&kind_arc),
        256,
        ctx.shutdown_rx.clone(),
    );

    let stream = ReceiverStream::new(rx).map(move |event: super::mailbox::MeshEvent| {
        use base64::Engine as _;
        let _ = &handle; // keep the MailboxHandle alive for the duration of the stream
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&event.payload);
        let data = json!({
            "sender":      event.sender.to_string(),
            "kind":        event.kind.as_ref(),
            "payload_b64": payload_b64,
        });
        Ok(Event::default()
            .event(event.kind.as_ref())
            .data(data.to_string()))
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `POST /gateway/mailbox/deliver` — deliver an event to a target node's mailbox.
///
/// Body: `{"target": "IP:PORT", "kind": "…", "payload_b64": "…"}`.
/// Returns `{"ok": true}`.
async fn gw_mailbox_deliver(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;

    let target: crate::node_id::NodeId = match body["target"].as_str().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing or invalid target"}))).into_response(),
    };
    let kind: Arc<str> = match body["kind"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing kind"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    super::mailbox::deliver_event_ctx(
        &ctx.agent_ctx,
        &ctx.agent_ctx.node_id,
        &target,
        kind,
        payload,
    );

    Json(json!({ "ok": true })).into_response()
}

// ── Overlay gateway helpers ───────────────────────────────────────────────────

/// Build a `ConsensusEngine` from `TaskCtx`, skipping the opacity/load-balance
/// heuristics used by `ConsensusHandle::cluster_propose` — those are performance
/// hints, not correctness requirements, and are not available from `TaskCtx`.
#[cfg(feature = "consensus")]
fn overlay_make_engine(ctx: &Arc<TaskCtx>) -> crate::consensus::ConsensusEngine {
    crate::consensus::ConsensusEngine {
        task_ctx:            Arc::clone(ctx),
        abstain_when_opaque: false,
        use_trust_slices:    false,
        max_abstain_ballots: 3,
        self_locality:       None,
        topology_policy:     None,
    }
}

/// Thin system-wide propose from `TaskCtx` (quorum = floor(N/2)+1 over live peers).
#[cfg(feature = "consensus")]
async fn overlay_cluster_propose(
    ctx:    &Arc<TaskCtx>,
    slot:   &str,
    value:  Bytes,
    config: crate::consensus::ConsensusConfig,
) -> crate::consensus::ConsensusResult {
    let n_nodes = (ctx.peers.len() + 1).max(1);
    let quorum  = super::helpers::compute_quorum_size(config.quorum_size, n_nodes);
    overlay_make_engine(ctx)
        .propose(
            crate::signal::SignalScope::Cluster,
            Arc::from(slot),
            value,
            quorum,
            config,
            None,
        )
        .await
}

/// Thin group propose from `TaskCtx`.
#[cfg(feature = "consensus")]
async fn overlay_group_propose(
    ctx:    &Arc<TaskCtx>,
    group:  &str,
    slot:   &str,
    value:  Bytes,
    config: crate::consensus::ConsensusConfig,
) -> crate::consensus::ConsensusResult {
    let prefix  = crate::signal::grp_prefix(group);
    let members = crate::store::scan_kv_prefix(ctx.kv_state.as_ref(), &prefix);
    // The electorate must be established, not inferred from absence — the HTTP surface enforces
    // the same contract as the library path (`ConsensusHandle::group_propose`), because a caller
    // must not get a different answer for reaching the same election through a socket.
    let declared_min = super::helpers::declared_electorate_min(ctx, group);
    match super::helpers::resolve_electorate(members.len(), declared_min) {
        super::helpers::Electorate::Established(_) => {}
        super::helpers::Electorate::Unavailable { observed, declared_min } =>
            return crate::consensus::ConsensusResult::ElectorateUnavailable {
                slot:  Arc::from(slot),
                group: Arc::from(group),
                observed_members: observed,
                declared_min,
            },
    }
    // NO `+ 1`: the `grp/{group}/` roster already includes self (a node joins by writing its own
    // member key), so `+ 1` double-counts self and over-sizes the quorum — a solo group `{self}`
    // then needs 2 votes and can only ever cast 1 → spurious Timeout where the library path
    // (`member_ids.len().max(1)`, consensus_handle.rs) commits at quorum 1 (audit 2026-07-15 pass 2).
    let n       = members.len().max(1);
    let quorum  = super::helpers::compute_quorum_size(config.quorum_size, n);
    overlay_make_engine(ctx)
        .propose(
            crate::signal::SignalScope::Group(Arc::from(group)),
            Arc::from(slot),
            value,
            quorum,
            config,
            None,
        )
        .await
}

// ── Cross-group consensus ─────────────────────────────────────────────────────

#[cfg(feature = "consensus")]
#[derive(serde::Deserialize)]
struct CrossGroupProposeBody {
    slot:      String,
    value_b64: Option<String>,
    groups:    Vec<crate::consensus::GroupQuorum>,
}

/// `POST /gateway/consensus/cross_group_propose` — multi-group proposal.
///
/// Body: `{"slot": "S", "value_b64": "...", "groups": [{"group":"G","quorum":0.5,"veto":false}]}`
/// Returns `{"ok":true}` on commit, or an error status.
#[cfg(feature = "consensus")]
async fn gw_cross_group_propose(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<CrossGroupProposeBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let value = if let Some(b64) = body.value_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    if body.groups.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"groups must not be empty"}))).into_response();
    }

    let engine = overlay_make_engine(&ctx.agent_ctx);
    let result = engine.cross_propose(
        Arc::from(body.slot.as_str()),
        value,
        &body.groups,
        crate::consensus::ConsensusConfig::default(),
    ).await;

    let persisted = committed_bool(&result);
    match super::consensus_handle::receipt_from(result, &ctx.agent_ctx) {
        Ok(receipt) => Json(commit_json(persisted, &receipt.local_durability)).into_response(),
        Err(err) => commit_error_response(err),
    }
}

/// The v2.4.2 `"persisted"` bool, read off the result **before** it becomes a receipt so the
/// old field is exactly what it always was — the receipt is added beside it, never derived back
/// into it (the regression floor pins `persisted`; PR 7 must not move that pin).
#[cfg(feature = "consensus")]
fn committed_bool(result: &crate::consensus::ConsensusResult) -> bool {
    matches!(result, crate::consensus::ConsensusResult::Committed { persisted: true, .. })
}

/// The commit's JSON — HTTP parity with [`CommitReceipt`](crate::CommitReceipt) (item 1 PR 7,
/// `docs/design/contracts-receipts.md` §5). `"persisted"` is unchanged; `"local_durability"` is
/// [`LocalDurability::tag`](crate::LocalDurability::tag) beside it — the same four names the
/// SDKs read — and `"local_durability_error"` is present **only** when the state is `failed`.
/// JSON is additive-safe: a pre-2.8 client reads `persisted` and ignores the rest.
#[cfg(feature = "consensus")]
fn commit_json(persisted: bool, durability: &crate::LocalDurability) -> serde_json::Value {
    let mut body = json!({ "ok": true, "persisted": persisted, "local_durability": durability.tag() });
    if let Some(reason) = durability.failure_reason() {
        body["local_durability_error"] = json!(reason);
    }
    body
}

/// A refused commit, in the statuses the two overlay verbs have always used: delivery unknown is
/// `504` (the value *may* have committed elsewhere — the body says so), a lost or untried
/// proposal is `409`.
#[cfg(feature = "consensus")]
fn commit_error_response(err: crate::CommitError) -> axum::response::Response {
    use crate::CommitError;
    match err {
        CommitError::DeliveryUnknown { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("consensus timed out after {ballots_tried} ballot(s)") }))).into_response(),
        CommitError::Superseded { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response(),
        CommitError::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        CommitError::NotAMember { group, .. } => not_a_member_response(&group),
        // `CommitError` is `#[non_exhaustive]` in `mycelium-core`: a refusal this build does not
        // name is still a refusal, and its `Display` says what it is.
        other => (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": other.to_string() }))).into_response(),
    }
}

// ── Overlay: consistent KV ────────────────────────────────────────────────────

/// `POST /gateway/overlay/consistent/set` — consensus-durable KV write (ballot-serialized).
///
/// Body: `{"key": "K", "value_b64": "V"}`.
#[cfg(feature = "consensus")]
async fn gw_overlay_consistent_set(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let key = match body["key"].as_str() {
        Some(k) => k.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing key"}))).into_response(),
    };
    if let Some(refused) = refuse_protected_key(&key) {
        return refused;
    }
    let value = if let Some(b64) = body["value_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let slot = format!("consistent/{key}");
    let result = overlay_cluster_propose(
        &ctx.agent_ctx, &slot, value.clone(),
        crate::consensus::ConsensusConfig::default(),
    ).await;

    let persisted = committed_bool(&result);
    match super::consensus_handle::receipt_from(result, &ctx.agent_ctx) {
        Ok(receipt) => {
            let key_arc: Arc<str> = Arc::from(key.as_str());
            let update = crate::framing::make_gossip_update(
                &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
                key_arc, value, false, &ctx.agent_ctx.hlc,
            );
            crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
            crate::framing::dispatch_gossip_try_send(
                &ctx.agent_ctx.gossip_txs,
                crate::framing::WireMessage::Data(update),
                ctx.agent_ctx.node_id.id_hash(),
                crate::framing::ForwardHint::All,
                &ctx.agent_ctx.kv_state.dropped_frames,
            );
            Json(commit_json(persisted, &receipt.local_durability)).into_response()
        }
        Err(err) => commit_error_response(err),
    }
}

/// `GET /gateway/overlay/consistent/get?key=K` — read latest ballot-committed value (local, eventually consistent).
#[cfg(feature = "consensus")]
async fn gw_overlay_consistent_get(
    Query(q):   Query<KvKeyQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let committed_key = format!("consensus/committed/consistent/{}", q.key);
    let value = ctx.agent_ctx.kv_state.store.pin()
        .get(committed_key.as_str())
        .and_then(|e| e.data.clone())
        .or_else(|| {
            ctx.agent_ctx.kv_state.store.pin()
                .get(q.key.as_str())
                .and_then(|e| e.data.clone())
        });
    match value {
        Some(v) => Json(json!({ "found": true, "value_b64": base64::engine::general_purpose::STANDARD.encode(&v) })).into_response(),
        None    => Json(json!({ "found": false })).into_response(),
    }
}

// ── Overlay: distributed lock ─────────────────────────────────────────────────

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
struct LockAcquireBody { name: String, ttl_secs: Option<u64> }

/// `POST /gateway/overlay/lock/acquire` — acquire a named distributed lock.
///
/// Body: `{"name": "N", "ttl_secs": 30}`.
/// Returns `{"guard_id": "…", "token": "N"}` — the token is a **decimal string** (the
/// fencing HLC exceeds JS safe-integer range; compare it as a big integer).
#[cfg(feature = "consensus")]
async fn gw_overlay_lock_acquire(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LockAcquireBody>,
) -> impl IntoResponse {
    let ttl_secs = body.ttl_secs.unwrap_or(30).clamp(1, 3600);
    let slot  = format!("lock/{}", body.name);
    // #164: `{holder}:{nonce}` value + a real consensus lease from ttl + converged-holder
    // confirmation — the same fix as the Rust `distributed_lock`. The pre-fix gateway lock had
    // all three bugs (no mutual exclusion, decorative ttl, unreleasable slot).
    let value = Bytes::from(
        format!("{}:{:016x}", ctx.agent_ctx.node_id, fastrand::u64(..)).into_bytes(),
    );
    let cfg = crate::consensus::ConsensusConfig {
        committed_lease_secs: Some(ttl_secs),
        ..crate::consensus::ConsensusConfig::default()
    };

    let result = overlay_cluster_propose(&ctx.agent_ctx, &slot, value.clone(), cfg).await;

    match result {
        crate::consensus::ConsensusResult::Committed { .. } => {
            // Confirm the converged holder before handing out a guard (bug A); the token is the
            // commit's HLC (a monotonic fencing token — the ballot is not, #164).
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            let confirmed = crate::consensus::live_committed_with_hlc(
                    &ctx.agent_ctx.kv_state, &slot, crate::consensus::causal_now_ms(&ctx.agent_ctx.hlc))
                .filter(|(v, _)| v.as_ref() == value.as_ref());
            let Some((_, token)) = confirmed else {
                return (StatusCode::CONFLICT,
                    Json(json!({ "ok": false, "error": "superseded" }))).into_response();
            };
            let guard = LockGuard {
                ctx:      Arc::clone(&ctx.agent_ctx),
                name:     Arc::from(body.name.as_str()),
                value,
                token,
                released: false,
            };
            let guard_id = format!("{:016x}", fastrand::u64(..));
            ctx.lock_guards.lock().unwrap_or_else(|e| e.into_inner()).insert(guard_id.clone(), guard);
            Json(json!({ "ok": true, "guard_id": guard_id, "token": token.to_string() })).into_response()
        }
        crate::consensus::ConsensusResult::Timeout { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("timeout after {ballots_tried} ballot(s)") }))).into_response(),
        crate::consensus::ConsensusResult::Superseded { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response(),
        crate::consensus::ConsensusResult::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        crate::consensus::ConsensusResult::ElectorateUnavailable {
            observed_members, declared_min, ..
        } => (StatusCode::CONFLICT, Json(json!({
            "ok": false,
            "error": "electorate_unavailable",
            "observed_members": observed_members,
            "declared_min": declared_min,
        }))).into_response(),
        crate::consensus::ConsensusResult::NotAMember { group, .. } => not_a_member_response(&group),
    }
}

/// `DELETE /gateway/overlay/lock/:guard_id` — release a held lock.
#[cfg(feature = "consensus")]
async fn gw_overlay_lock_release(
    Path(guard_id): Path<String>,
    State(ctx):     State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    let removed = ctx.lock_guards.lock().unwrap_or_else(|e| e.into_inner()).remove(&guard_id);
    if removed.is_some() {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "guard_not_found" }))).into_response()
    }
}

// ── Overlay: leader election ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
struct ElectBody { group: String }

/// `POST /gateway/overlay/elect` — elect a leader for `group`.
///
/// Body: `{"group": "G"}`.
/// Returns `{"leader": "IP:PORT"}` on success.
#[cfg(feature = "consensus")]
async fn gw_overlay_elect(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<ElectBody>,
) -> impl IntoResponse {
    let slot  = format!("leader/{}", body.group);
    let value = Bytes::from(ctx.agent_ctx.node_id.to_string().into_bytes());

    let result = overlay_group_propose(
        &ctx.agent_ctx, &body.group, &slot, value,
        crate::consensus::ConsensusConfig::default(),
    ).await;

    // #164 class: an optimistic `Committed` is NOT mutually exclusive — never return `self`.
    // Let the winning commit converge, then read the AUTHORITATIVE leader from the committed slot
    // (mirrors `elect_leader` + `distributed_lock`; returning self split-brained — audit 2026-07-15).
    let converge = matches!(result, crate::consensus::ConsensusResult::Committed { .. });
    match result {
        crate::consensus::ConsensusResult::Committed { .. }
        | crate::consensus::ConsensusResult::Superseded { .. } => {
            if converge {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            let committed_key = format!("consensus/committed/{slot}");
            if let Some(raw) = ctx.agent_ctx.kv_state.store.pin().get(committed_key.as_str()).and_then(|e| e.data.clone())
                && let Ok(s) = std::str::from_utf8(&raw) {
                    return Json(json!({ "ok": true, "leader": s.to_string() })).into_response();
                }
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "superseded" }))).into_response()
        }
        crate::consensus::ConsensusResult::Timeout { ballots_tried, .. } =>
            (StatusCode::GATEWAY_TIMEOUT, Json(json!({ "ok": false, "error": format!("timeout after {ballots_tried} ballot(s)") }))).into_response(),
        crate::consensus::ConsensusResult::TopologyUnsatisfied { .. } =>
            (StatusCode::CONFLICT, Json(json!({ "ok": false, "error": "topology_unsatisfied" }))).into_response(),
        // Nothing was proposed, so there is no leader. Answering from the committed slot here
        // would hand back a *stale* winner from an earlier, differently-constituted election —
        // which is the failure this refusal exists to prevent, one level down.
        crate::consensus::ConsensusResult::ElectorateUnavailable {
            observed_members, declared_min, ..
        } => (StatusCode::CONFLICT, Json(json!({
            "ok": false,
            "error": "electorate_unavailable",
            "observed_members": observed_members,
            "declared_min": declared_min,
            "detail": if observed_members == 0 {
                "the group roster is empty (unknown or unjoined group) — an election needs \
                 members, and absence is not authority"
            } else {
                "this node sees fewer members than the group declares; its view is partial"
            },
        }))).into_response(),
        // This node is not in the group: it may not elect a leader for it, least of all itself.
        crate::consensus::ConsensusResult::NotAMember { group, .. } => not_a_member_response(&group),
    }
}

/// **403 `not_a_member`**: this node is not in the named group's roster, so it did not propose.
/// A proposer counts its own vote toward a quorum drawn from the roster; from outside it that vote
/// is one the electorate does not contain (`ConsensusResult::NotAMember`). An authority refusal,
/// so 403 like `governed_group`, not the 409 of a roster that could not be established.
#[cfg(feature = "consensus")]
fn not_a_member_response(group: &str) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(json!({
        "ok": false,
        "error": "not_a_member",
        "group": group,
        "detail": "this node is not in the group's roster, so it may not propose to it — join the \
                   group first (POST /gateway/mesh/group, or /gateway/govern/group for a governed one)",
    }))).into_response()
}

// ── Overlay: ordered log ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LogAppendBody { stream: String, value_b64: Option<String> }

/// `POST /gateway/overlay/log/append` — append to `stream`.
///
/// Body: `{"stream": "S", "value_b64": "V"}`.
/// Returns `{"hlc": N}`.
async fn gw_overlay_log_append(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LogAppendBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    if let Some(refused) = refuse_owned_stream(&body.stream) {
        return refused;
    }
    let value = if let Some(b64) = body.value_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 value"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    let hlc = ctx.agent_ctx.hlc.tick();
    // Salt with the node id so two nodes appending in the same wall-ms don't collide on one key
    // and silently drop an entry via LWW (audit 2026-07-15); HLC stays the first key segment.
    let node = &ctx.agent_ctx.node_id;
    let key: Arc<str> = Arc::from(format!("log/{}/{hlc:016x}/{node}", body.stream).as_str());
    let update = crate::framing::make_gossip_update(
        &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
        key, value, false, &ctx.agent_ctx.hlc,
    );
    crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
    crate::framing::dispatch_gossip_try_send(
        &ctx.agent_ctx.gossip_txs,
        crate::framing::WireMessage::Data(update),
        ctx.agent_ctx.node_id.id_hash(),
        crate::framing::ForwardHint::All,
        &ctx.agent_ctx.kv_state.dropped_frames,
    );
    Json(json!({ "hlc": hlc })).into_response()
}

#[derive(Deserialize)]
struct LogScanQuery { stream: String, from: Option<u64>, to: Option<u64> }

/// `GET /gateway/overlay/log/scan?stream=S&from=0&to=MAX` — range scan.
///
/// Returns `[{"hlc": N, "value_b64": "…"}]`.
async fn gw_overlay_log_scan(
    Query(q):   Query<LogScanQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let from = q.from.unwrap_or(0);
    let to   = q.to.unwrap_or(u64::MAX);
    let prefix = format!("log/{}/", q.stream);
    let mut entries: Vec<LogEntry> = crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, &prefix)
        .into_iter()
        .filter_map(|(k, v)| {
            let suffix = k.strip_prefix(&prefix)?;
            let hlc    = u64::from_str_radix(suffix.split('/').next()?, 16).ok()?;
            if hlc >= from && hlc < to { Some(LogEntry { hlc, value: v }) } else { None }
        })
        .collect();
    entries.sort_by_key(|e| e.hlc);
    let arr: Vec<serde_json::Value> = entries.iter().map(|e| json!({
        "hlc":       e.hlc,
        "value_b64": base64::engine::general_purpose::STANDARD.encode(&e.value),
    })).collect();
    Json(arr).into_response()
}

#[derive(Deserialize)]
struct LogCompactBody { stream: String, before_hlc: u64 }

/// `POST /gateway/overlay/log/compact` — tombstone entries with HLC < `before_hlc`.
async fn gw_overlay_log_compact(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<LogCompactBody>,
) -> impl IntoResponse {
    if let Some(refused) = refuse_owned_stream(&body.stream) {
        return refused;
    }
    let prefix = format!("log/{}/", body.stream);
    for (k, _) in crate::store::scan_kv_prefix(&ctx.agent_ctx.kv_state, &prefix) {
        let suffix = k.strip_prefix(&prefix).unwrap_or("");
        if let Some(hlc) = suffix.split('/').next().and_then(|s| u64::from_str_radix(s, 16).ok())
            && hlc < body.before_hlc {
                let update = crate::framing::make_gossip_update(
                    &ctx.agent_ctx.node_id, ctx.agent_ctx.default_ttl,
                    k, Bytes::new(), true, &ctx.agent_ctx.hlc,
                );
                crate::store::apply_and_notify(&ctx.agent_ctx.kv_state, &update);
                crate::framing::dispatch_gossip_try_send(
                    &ctx.agent_ctx.gossip_txs,
                    crate::framing::WireMessage::Data(update),
                    ctx.agent_ctx.node_id.id_hash(),
                    crate::framing::ForwardHint::All,
                    &ctx.agent_ctx.kv_state.dropped_frames,
                );
            }
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct LogSubscribeQuery { stream: String, since: Option<u64> }

/// `GET /gateway/overlay/log/subscribe?stream=S&since=0` — SSE stream of log entries.
async fn gw_overlay_log_subscribe(
    Query(q):   Query<LogSubscribeQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let prefix      = format!("log/{}/", q.stream);
    let prefix_arc: Arc<str> = Arc::from(prefix.as_str());
    let mut watcher  = super::capability_ops::subscribe_prefix_on_kv(&ctx.agent_ctx.kv_state, Arc::clone(&prefix_arc));
    let stream_name  = q.stream.clone();
    let kv_state     = Arc::clone(&ctx.agent_ctx.kv_state);
    let mut last_seen = q.since.unwrap_or(0);

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    tokio::spawn(async move {
        loop {
            let entries = {
                let mut es: Vec<LogEntry> = crate::store::scan_kv_prefix(&kv_state, &prefix)
                    .into_iter()
                    .filter_map(|(k, v)| {
                        let suffix = k.strip_prefix(&prefix)?;
                        let hlc    = u64::from_str_radix(suffix.split('/').next()?, 16).ok()?;
                        if hlc >= last_seen { Some(LogEntry { hlc, value: v }) } else { None }
                    })
                    .collect();
                es.sort_by_key(|e| e.hlc);
                es
            };
            for entry in entries {
                use base64::Engine as _;
                // `saturating_add`: the HLC is parsed from the (any-node-writable) log key, so a
                // crafted `log/{stream}/ffffffffffffffff/...` entry gives `hlc == u64::MAX`; `+ 1`
                // panicked (overflow-checks → node-abort) or wrapped to 0 (release → the cursor resets
                // and the subscriber re-floods the whole stream on every change) (audit 2026-07-15 pass 4).
                last_seen = entry.hlc.saturating_add(1);
                let data  = json!({
                    "stream":    stream_name,
                    "hlc":       entry.hlc,
                    "value_b64": base64::engine::general_purpose::STANDARD.encode(&entry.value),
                });
                if tx.send(Event::default().data(data.to_string())).await.is_err() { return; }
            }
            if watcher.changed().await.is_err() { return; }
        }
    });

    Sse::new(ReceiverStream::new(rx).map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
#[cfg(feature = "consensus")]
struct LogGroupSubscribeQuery { stream: String, group: String }

/// `GET /gateway/overlay/log/group/subscribe?stream=S&group=G` — **single-active, exact-once**
/// ordered log consumption over SSE.
///
/// **Contract:** at most one consumer of `(stream, group)` is active at a time; it receives every
/// entry in HLC order, exactly once, and another subscriber takes over if it dies (failover). This
/// is *not* a load-balanced work queue — the group does not share entries across active consumers.
/// (For competitive, load-balanced exactly-once *work distribution*, use the `mycelium-tuple-space`
/// companion, which claims each item atomically. A single advancing offset cannot do per-item
/// competitive consumption — that is a different pattern; see the `log-group vs work-queue` note.)
///
/// **How it's achieved (#149):** the claim `clog/{stream}/{group}/claim` is a **leased consensus
/// commitment** (`committed_lease_secs`). Because two near-simultaneous proposers can both
/// *optimistically* commit — each checks only its *local* committed view at commit time (the
/// ~40% double-acquire race) — the propose return is not trusted for exclusivity. Instead, after a
/// commit we read the **converged committed holder** (`live_committed_value`): commit-keys are
/// LWW-resolved by HLC, so exactly one holder converges, and only that node proceeds; losers stand
/// by *without releasing* (their losing commit is LWW-overwritten by the winner's — a tombstone
/// would clear the winner's claim). The winner drains with a **private local offset** (exact-once
/// by construction — no cross-consumer offset hand-off) and **renews the lease** while active; on
/// its death the lease lapses and a standby takes over (failover). The earlier code used a
/// bare-LWW "lock" (no mutual exclusion → every consumer drained the whole stream).
#[cfg(feature = "consensus")]
async fn gw_overlay_log_group_subscribe(
    Query(q):   Query<LogGroupSubscribeQuery>,
    State(ctx): State<Arc<HttpCtx>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream_name = q.stream.clone();
    let group_name  = q.group.clone();
    let kv_state    = Arc::clone(&ctx.agent_ctx.kv_state);
    let task_ctx    = Arc::clone(&ctx.agent_ctx);

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        let claim_slot = format!("clog/{stream_name}/{group_name}/claim");
        let offset_key = format!("clog/{stream_name}/{group_name}/offset");
        let prefix     = format!("log/{stream_name}/");
        // Stable claim value = holder id only (no expiry inside the value), so a re-propose is the
        // "same value" the lease path re-endorses; the lease governs expiry + failover.
        let holder: Bytes = Bytes::from(task_ctx.node_id.to_string().into_bytes());
        let lease_secs: u64 = 30;
        let mk_cfg = || crate::consensus::ConsensusConfig {
            committed_lease_secs: Some(lease_secs),
            ..crate::consensus::ConsensusConfig::default()
        };

        // Exact-once across a consumer group = a SINGLE active consumer (issue #149). The claim is a
        // *leased* consensus commitment. Two near-simultaneous proposers can both *optimistically*
        // commit (each checks only its local committed view at commit time), so the propose return
        // cannot be trusted for exclusivity — that is the ~40% double-acquire race. But the
        // commit-keys are LWW-resolved by HLC, so the CONVERGED holder is deterministic: propose,
        // let it converge, then read the authoritative committed holder. Exactly one wins; losers
        // stand by WITHOUT releasing (their losing commit is LWW-overwritten by the winner's —
        // tombstoning would clear the winner's claim). The lease gives failover if the holder dies.
        'acquire: loop {
            if tx.is_closed() { return; }
            let won = matches!(
                overlay_cluster_propose(&task_ctx, &claim_slot, holder.clone(), mk_cfg()).await,
                crate::consensus::ConsensusResult::Committed { .. },
            );
            if won {
                tokio::time::sleep(Duration::from_millis(1000)).await; // let the winning commit converge
                let is_me = crate::consensus::live_committed_value(
                        &kv_state, &claim_slot, crate::consensus::causal_now_ms(&task_ctx.hlc))
                    .as_deref() == Some(holder.as_ref());
                if is_me { break 'acquire; }
            }
            // Lost the race, or a live lease is held elsewhere — stand by; retry after it may lapse.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Resume from the persisted offset (LWW read; the sole holder is the only writer).
        let mut offset: u64 = kv_state.store.pin().get(offset_key.as_str())
            .and_then(|e| e.data.clone())
            .and_then(|b| std::str::from_utf8(&b).ok().and_then(|s| u64::from_str_radix(s, 16).ok()))
            .unwrap_or(0);
        let renew_every = Duration::from_secs((lease_secs / 3).max(1));
        let mut last_renew = std::time::Instant::now();

        loop {
            // Renew the lease while active (re-propose the SAME holder value → re-endorsed while
            // live; a different proposer is superseded). If we somehow lost the claim (a partition
            // let another win), stop — a single active consumer is the invariant.
            if last_renew.elapsed() >= renew_every {
                let _ = overlay_cluster_propose(&task_ctx, &claim_slot, holder.clone(), mk_cfg()).await;
                last_renew = std::time::Instant::now();
                let still_me = crate::consensus::live_committed_value(
                        &kv_state, &claim_slot, crate::consensus::causal_now_ms(&task_ctx.hlc))
                    .as_deref() == Some(holder.as_ref());
                if !still_me { return; }
            }

            let mut entries: Vec<LogEntry> = crate::store::scan_kv_prefix(&kv_state, &prefix)
                .into_iter()
                .filter_map(|(k, v)| {
                    let suffix = k.strip_prefix(&prefix)?;
                    let hlc    = u64::from_str_radix(suffix.split('/').next()?, 16).ok()?;
                    if hlc > offset { Some(LogEntry { hlc, value: v }) } else { None }
                })
                .collect();
            entries.sort_by_key(|e| e.hlc);

            if let Some(entry) = entries.into_iter().next() {
                offset = entry.hlc;
                // Persist the offset (LWW; sole writer) so a replacement consumer resumes on failover.
                let offset_key_arc: Arc<str> = Arc::from(offset_key.as_str());
                let update = crate::framing::make_gossip_update(
                    &task_ctx.node_id, task_ctx.default_ttl,
                    offset_key_arc, Bytes::from(format!("{:016x}", entry.hlc).into_bytes()),
                    false, &task_ctx.hlc,
                );
                crate::store::apply_and_notify(&task_ctx.kv_state, &update);
                crate::framing::dispatch_gossip_try_send(
                    &task_ctx.gossip_txs,
                    crate::framing::WireMessage::Data(update),
                    task_ctx.node_id.id_hash(),
                    crate::framing::ForwardHint::All,
                    &task_ctx.kv_state.dropped_frames,
                );
                use base64::Engine as _;
                let data = json!({
                    "stream":    stream_name,
                    "hlc":       entry.hlc,
                    "value_b64": base64::engine::general_purpose::STANDARD.encode(&entry.value),
                });
                if tx.send(Event::default().data(data.to_string())).await.is_err() { return; }
            } else {
                // Idle: the drain loop otherwise only notices a disconnected client via a failed
                // `tx.send`, which never fires while the stream is idle — so the task looped forever
                // RENEWING the exclusive `clog/{stream}/{group}/claim` lease, permanently blocking
                // failover to a standby consumer (audit 2026-07-15 pass 4). Check the channel here so a
                // client that disconnects during an idle stream releases the claim (task returns → lease
                // stops renewing → it expires and a standby can acquire).
                if tx.is_closed() { return; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx).map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default())
}

// ── Overlay: reliable delivery ────────────────────────────────────────────────

#[derive(Deserialize)]
struct EmitReliableBody {
    target:       String,
    kind:         String,
    payload_b64:  Option<String>,
    timeout_secs: Option<u64>,
}

/// `POST /gateway/overlay/emit_reliable` — send with explicit ACK.
///
/// Body: `{"target": "IP:PORT", "kind": "K", "payload_b64": "V", "timeout_secs": 5}`.
/// Returns `{"ack": "acknowledged" | "timeout"}`.
async fn gw_overlay_emit_reliable(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body):  Json<EmitReliableBody>,
) -> impl IntoResponse {
    use base64::Engine as _;
    let caller = caller.map(|Extension(c)| c);
    let target: crate::node_id::NodeId = match body.target.parse() {
        Ok(n)  => n,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid target node id"}))).into_response(),
    };
    let payload = if let Some(b64) = body.payload_b64.as_deref() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(v)  => Bytes::from(v),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64 payload"}))).into_response(),
        }
    } else {
        Bytes::new()
    };
    let timeout = Duration::from_secs(body.timeout_secs.unwrap_or(5).clamp(1, 300));
    let kind: Arc<str> = Arc::from(body.kind.as_str());
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }

    match gateway_caller::gateway_rpc_call(&ctx.agent_ctx, caller.as_ref(), target, kind, payload, timeout).await {
        Ok(_)                                                    => Json(json!({ "ack": "acknowledged" })).into_response(),
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) => Json(json!({ "ack": "timeout" })).into_response(),
        Err(e)                                                   => dispatch_refused(e),
    }
}

// ── Cluster sharding ──────────────────────────────────────────────────────────

/// `GET /gateway/shard/{ns}/{name}?key=<shard_key>`
///
/// Returns the consistent-hash owner NodeId for `shard_key` among providers of
/// capability `ns/name`. The result is deterministic: every node with the same
/// provider view returns the same owner for the same key.
///
/// 200 `{"owner":"ip:port"}` — owner found.
/// 404 `{"error":"no providers"}` — no live providers match the filter.
#[derive(Deserialize)]
struct ShardOwnerQuery { key: String }

async fn gw_shard_owner(
    Path((ns, name)): Path<(String, String)>,
    Query(q):         Query<ShardOwnerQuery>,
    State(ctx):       State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;
    use super::sharding::shard_owner;

    let filter = CapFilter::new(ns.as_str(), name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    match shard_owner(&q.key, &providers) {
        Some(owner) => Json(json!({ "owner": owner.to_string() })).into_response(),
        None        => (StatusCode::NOT_FOUND, Json(json!({ "error": "no providers" }))).into_response(),
    }
}

/// `POST /gateway/shard/emit`
///
/// Emits `kind` signal to the consistent-hash owner for `shard_key` among
/// providers of `ns/name`. Equivalent to calling `emit_sharded` from Rust.
///
/// Request body:
/// ```json
/// { "kind": "actor.msg", "ns": "actor", "name": "user",
///   "shard_key": "user-12345", "payload_b64": "<base64>" }
/// ```
/// Response 200: `{"ok":true,"owner":"ip:port"}`
/// Response 404: `{"ok":false,"error":"no providers"}`
async fn gw_shard_emit(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body):  Json<serde_json::Value>,
) -> impl IntoResponse {
    use base64::Engine as _;
    use crate::capability::CapFilter;
    use super::sharding::shard_owner;
    use crate::signal::SignalScope;

    let kind = match body["kind"].as_str() {
        Some(k) => Arc::from(k),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing kind"}))).into_response(),
    };
    // Closure plan C1: a raw route never carries protected work around the door that checks it.
    if let Some(refused) = refuse_protected_kind(&ctx.agent_ctx.config, &kind) {
        return refused;
    }
    let ns   = match body["ns"].as_str()   { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing ns"}))).into_response() };
    let name = match body["name"].as_str() { Some(s) => s.to_string(), None => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing name"}))).into_response() };
    let shard_key = match body["shard_key"].as_str() {
        Some(s) => s.to_string(),
        None    => return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing shard_key"}))).into_response(),
    };
    let payload = if let Some(b64) = body["payload_b64"].as_str() {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b)  => Bytes::from(b),
            Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid base64"}))).into_response(),
        }
    } else {
        Bytes::new()
    };

    // As for `/gateway/signal/emit`: a client-supplied caller-context frame would verify as this
    // gateway's own envelope, because the raw emission carries the bytes verbatim with this node as
    // the sender. Found by the Phase-C adversarial audit (items 1+2+7).
    if super::gateway_caller::carries_caller_frame(&payload) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "payload carries a caller-context frame; that context is constructed by the gateway, not supplied"
        }))).into_response();
    }

    let filter    = CapFilter::new(ns.as_str(), name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    match shard_owner(&shard_key, &providers) {
        Some(owner) => {
            super::helpers::emit_signal_async(
                &ctx.agent_ctx, kind, SignalScope::Individual(owner.clone()), payload,
            ).await;
            Json(json!({ "ok": true, "owner": owner.to_string() })).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "ok": false, "error": "no providers" }))).into_response(),
    }
}

/// Shared helper: scan `cap/` KV and return providers matching `filter`.
/// Mirrors the scan in `gw_cap_resolve` (no freshness check — same as the HTTP resolve endpoint).
fn resolve_cap_providers(
    kv_state: &crate::store::KvState,
    filter:   &crate::capability::CapFilter,
) -> Vec<(crate::node_id::NodeId, crate::capability::Capability)> {
    use crate::capability::Capability;
    use crate::store::scan_kv_prefix;
    use super::capability_ops::{is_cap_locality_key, parse_cap_key_or_warn};

    let mut out = Vec::new();
    for (key, bytes) in scan_kv_prefix(kv_state, "cap/") {
        if is_cap_locality_key(&key) { continue; }
        let Some((node_id, _ns, _name)) = parse_cap_key_or_warn("cap/", &key) else { continue };
        let Some(cap) = Capability::decode(&bytes) else { continue };
        if filter.matches(&cap) {
            out.push((node_id, cap));
        }
    }
    out
}

// ── LLM / Prompt Skills gateway handlers ─────────────────────────────────────

#[cfg(feature = "llm")]
fn llm_get_prompt_from_kv(
    kv_state: &crate::store::KvState,
    ns: &str,
    name: &str,
) -> Option<crate::agent::prompt::PromptTemplate> {
    use crate::signal::kv_ns;
    let key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    let bytes = kv_state.store.pin().get(key.as_str())
        .and_then(|e| e.data.clone())?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(feature = "llm")]
async fn gw_prompts_list(
    State(ctx): State<Arc<HttpCtx>>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let entries: Vec<serde_json::Value> = crate::store::scan_kv_prefix(
        &ctx.agent_ctx.kv_state, kv_ns::PROMPTS,
    )
    .into_iter()
    .filter_map(|(k, _v)| {
        let rest = k.strip_prefix(kv_ns::PROMPTS)?;
        let mut parts = rest.splitn(2, '/');
        let ns   = parts.next()?.to_owned();
        let name = parts.next()?.to_owned();
        if name.is_empty() { return None; }
        llm_get_prompt_from_kv(&ctx.agent_ctx.kv_state, &ns, &name).map(|t| {
            serde_json::json!({
                "ns":          ns,
                "name":        name,
                "max_tokens":  t.max_tokens,
                "temperature": t.temperature,
                "metadata":    t.metadata,
            })
        })
    })
    .collect();
    axum::Json(entries)
}

#[cfg(feature = "llm")]
async fn gw_prompt_get(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match llm_get_prompt_from_kv(&ctx.agent_ctx.kv_state, &ns, &name) {
        Some(t) => axum::Json(serde_json::to_value(t).unwrap_or_default())
                       .into_response(),
        None    => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(feature = "llm")]
async fn gw_prompt_put(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
    axum::Json(body): axum::Json<crate::agent::prompt::PromptTemplate>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let kv_key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    match serde_json::to_vec(&body) {
        Ok(bytes) => {
            kv_write(&ctx.agent_ctx, Arc::from(kv_key.as_str()), Bytes::from(bytes), false);
            axum::Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(feature = "llm")]
async fn gw_prompt_delete(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path((ns, name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    use crate::signal::kv_ns;
    let key = format!("{}{}/{}", kv_ns::PROMPTS, ns, name);
    kv_write(&ctx.agent_ctx, Arc::from(key.as_str()), Bytes::new(), true);
    axum::Json(serde_json::json!({"ok": true}))
}

#[cfg(feature = "llm")]
#[derive(serde::Deserialize)]
struct LlmCallBody {
    ns:         String,
    name:       String,
    input:      String,
    #[serde(default)]
    context:    std::collections::HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[cfg(feature = "llm")]
fn default_timeout_ms() -> u64 { 30_000 }

#[cfg(feature = "llm")]
async fn gw_llm_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    axum::Json(body): axum::Json<LlmCallBody>,
) -> impl IntoResponse {
    use crate::capability::CapFilter;
    let caller = caller.map(|Extension(c)| c);
    use crate::signal::signal_kind;

    let timeout = std::time::Duration::from_millis(body.timeout_ms);
    let filter  = CapFilter::new(body.ns.as_str(), body.name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    let provider_str = providers.first()
        .map(|(id, _)| id.to_string())
        .unwrap_or_default();

    let (target, _) = match providers.into_iter().next() {
        Some(p) => p,
        None => {
            return (StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error":"no_provider","detail":""})))
                .into_response();
        }
    };

    let req = serde_json::json!({
        "prompt":  format!("{}/{}", body.ns, body.name),
        "input":   body.input,
        "context": body.context,
    });
    let payload = Bytes::from(req.to_string().into_bytes());

    match gateway_caller::gateway_rpc_call(
        &ctx.agent_ctx,
        caller.as_ref(),
        target,
        Arc::from(signal_kind::LLM_INVOKE),
        payload,
        timeout,
    ).await {
        Ok(reply) => {
            let v: serde_json::Value = serde_json::from_slice(&reply)
                .unwrap_or_else(|_| serde_json::json!({"error":"parse_error","detail":""}));
            if v.get("error").is_some() {
                // Provider-side failure forwarded to the caller: upstream error.
                return (StatusCode::BAD_GATEWAY, axum::Json(v)).into_response();
            }
            axum::Json(serde_json::json!({
                "output":   v["output"],
                "provider": provider_str,
            })).into_response()
        }
        Err(GatewayDispatchError::Rpc(super::rpc::RpcError::Timeout)) =>
            (StatusCode::GATEWAY_TIMEOUT,
                axum::Json(serde_json::json!({"error":"timeout","detail":""})))
                .into_response(),
        Err(e) => dispatch_refused(e),
    }
}

#[cfg(feature = "llm")]
#[derive(serde::Deserialize)]
struct LlmStreamBody {
    ns:      String,
    name:    String,
    input:   String,
    #[serde(default)]
    context: std::collections::HashMap<String, String>,
}

#[cfg(feature = "llm")]
async fn gw_llm_stream(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    axum::Json(body): axum::Json<LlmStreamBody>,
) -> impl IntoResponse {
    use axum::response::sse::Event;
    let caller = caller.map(|Extension(c)| c);
    use crate::capability::CapFilter;
    use crate::signal::signal_kind;
    use futures_util::stream;

    // v1: buffer full response via RPC, emit as single "done" event.
    // Errors are reported as in-stream `{"type":"error",...}` events, not HTTP
    // status codes: SSE commits the status line before the body, so this is
    // the only legible channel once streaming starts (deliberate asymmetry
    // with gw_llm_call, which uses 404/502/504).
    let timeout = std::time::Duration::from_secs(30);
    let filter  = CapFilter::new(body.ns.as_str(), body.name.as_str());
    let providers = resolve_cap_providers(&ctx.agent_ctx.kv_state, &filter);

    let event = match providers.into_iter().next() {
        None => {
            let data = serde_json::json!({"type":"error","error":"no_provider"}).to_string();
            Event::default().data(data)
        }
        Some((target, _)) => {
            let req = serde_json::json!({
                "prompt":  format!("{}/{}", body.ns, body.name),
                "input":   body.input,
                "context": body.context,
            });
            let payload = Bytes::from(req.to_string().into_bytes());
            match gateway_caller::gateway_rpc_call(
                &ctx.agent_ctx,
                caller.as_ref(),
                target,
                Arc::from(signal_kind::LLM_INVOKE),
                payload,
                timeout,
            ).await {
                Ok(reply) => {
                    let v: serde_json::Value = serde_json::from_slice(&reply)
                        .unwrap_or_else(|_| serde_json::json!({"error":"parse_error"}));
                    let output = v["output"].as_str().unwrap_or("").to_owned();
                    let data = serde_json::json!({"type":"done","output":output}).to_string();
                    Event::default().data(data)
                }
                Err(GatewayDispatchError::Rpc(_)) => {
                    let data = serde_json::json!({"type":"error","error":"timeout"}).to_string();
                    Event::default().data(data)
                }
                Err(e) => {
                    let data = serde_json::json!({"type":"error","error":e.reason(),"detail":e.to_string()}).to_string();
                    Event::default().data(data)
                }
            }
        }
    };

    Sse::new(stream::once(async move { Ok::<_, std::convert::Infallible>(event) }))
}

// ── Federation: the consumer side (item 2 row 11) ────────────────────────────
//
// The verbs a *local* client — Rust, Python or TypeScript — uses to reach a partner domain. The
// decisions are `crate::federation::client`'s and the refusal vocabulary is
// `federation_http`'s; what lives here is the part that is this file's business, and it is one
// sentence long: **the principal on the credential is the authenticated caller's, and is never
// read from the request body.**
//
// That is item 7's rule at one more boundary. A gateway holds the domain's signing key and the
// partner's trust; the client calling it has neither. If the body could name the principal, any
// local caller with `federation:invoke` could have this domain vouch for a principal it made up,
// and the partner's evidence would record it — authority by assertion, which is exactly the
// confused deputy the axis has now removed twice.

/// `GET /gateway/federation/domain`
#[cfg(feature = "tls")]
async fn gw_federation_domain(State(ctx): State<Arc<HttpCtx>>) -> Response {
    super::federation_http::domain_json(&ctx.agent_ctx)
}

/// `GET /gateway/federation/partners`
#[cfg(feature = "tls")]
async fn gw_federation_partners(State(ctx): State<Arc<HttpCtx>>) -> Response {
    super::federation_http::partners_json(&ctx.agent_ctx)
}

/// `GET /gateway/federation/catalog/{domain}` — the **last observed** catalogue for one partner.
///
/// Deliberately reads only what this node already holds: no network, so an operator can look at a
/// partner during an outage without the act of looking changing the link's state. `POST
/// /gateway/federation/connect` is the one that goes and asks.
#[cfg(feature = "tls")]
async fn gw_federation_catalog(
    State(ctx): State<Arc<HttpCtx>>,
    axum::extract::Path(domain): axum::extract::Path<String>,
) -> Response {
    let client = match super::federation_http::client_for(&ctx.agent_ctx, &domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let exports = client.last_catalogue();
    Json(json!({
        "domain": domain,
        "link": super::federation_http::link_word(client.link_state()),
        "observed": exports.is_some(),
        "exports": exports,
    }))
    .into_response()
}

/// `POST /gateway/federation/connect` — fetch a partner's catalogue and bring the link up.
#[cfg(feature = "tls")]
async fn gw_federation_connect(
    State(ctx): State<Arc<HttpCtx>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(domain) = body["domain"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing domain"}))).into_response();
    };
    let client = match super::federation_http::client_for(&ctx.agent_ctx, domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    match client.connect().await {
        Ok(exports) => Json(json!({
            "domain": domain,
            "link": super::federation_http::link_word(client.link_state()),
            "exports": exports,
        }))
        .into_response(),
        Err(e) => super::federation_http::call_refusal(&e),
    }
}

/// `POST /gateway/federation/call` — invoke one of a partner's exports.
///
/// `repeatable` is the caller's statement about **their own** effect, not ours, and it is the field
/// that decides whether a silent gateway may be retried elsewhere: a repeatable call can be, an
/// at-most-once call comes back `delivery: unknown` instead. It defaults to `false`, because the
/// safe default for an unstated effect is the one that never runs it twice.
#[cfg(feature = "tls")]
async fn gw_federation_call(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let caller = caller.map(|Extension(c)| c);
    let (Some(domain), Some(export)) = (body["domain"].as_str(), body["export"].as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"missing domain or export"}))).into_response();
    };
    let text = body["text"].as_str().unwrap_or("");
    let repeatability = if body["repeatable"].as_bool().unwrap_or(false) {
        crate::federation::gateway::Repeatability::Repeatable
    } else {
        crate::federation::gateway::Repeatability::AtMostOnce
    };
    let client = match super::federation_http::client_for(&ctx.agent_ctx, domain) {
        Ok(c) => c,
        Err(r) => return *r,
    };

    // AE slice: the same evaluator preflight `/mcp` and `/a2a` run. A federated call is a third
    // door onto an effect, and the argument a2a.rs makes for its own preflight is the argument
    // here verbatim — an enforcement point that can be walked around by choosing a different door
    // is not one. Inert unless an evaluator is attached.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let preflight = ae_preflight(
        &ctx.agent_ctx,
        caller.as_ref(),
        "federation.call",
        &format!("export:{export}@{domain}"),
        &json!({ "domain": domain, "export": export, "text": text }),
        &body,
        ENFORCEMENT_POINT_FEDERATION,
    )
    .await;
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if let Preflight::Refuse(refusal) = &preflight {
        // `sent: false` is a fact about this refusal, not a courtesy: the preflight runs before
        // the client is touched, so no credential was minted and no byte left the process.
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "policy", "detail": refusal.to_string(), "data": refusal.error_data(),
                "sent": false, "delivery": "none",
            })),
        )
            .into_response();
    }

    // Item 7 across the boundary: the principal the partner is told is the one the auth layer
    // resolved. On an open gateway that is `anonymous`, which is honest — this domain is vouching
    // that the caller was anonymous *here* — and is why the federation runbook tells an operator
    // who needs attribution to put a token model in front of these routes.
    let principal = caller
        .as_ref()
        .map(|c| c.principal.clone())
        .unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());

    let outcome = client.call_as(&principal, export, text, repeatability).await;

    // What this gateway observed, in the evidence vocabulary. The mapping is the refusal
    // vocabulary's `delivery` field one more time, and it is the same rule: a refusal this side
    // made is `None`, an answer is `Completed`, and everything else is `Unknown` — never a
    // negative we did not establish.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        use super::action_evaluator::Execution;
        use crate::federation::client::ClientError;
        let observed = match &outcome {
            Ok(_) => Execution::Completed,
            Err(ClientError::Link(_)) | Err(ClientError::Resolve(_)) | Err(ClientError::Principal(_))
            | Err(ClientError::Tls(_)) | Err(ClientError::Egress { .. }) => Execution::None,
            Err(ClientError::Outcome(crate::federation::gateway::CallOutcome::NoCapacity { .. })) => Execution::None,
            Err(ClientError::Refused { .. }) => Execution::Failed,
            Err(_) => Execution::Unknown,
        };
        ae_record_execution(&ctx.agent_ctx, &preflight, observed).await;
    }

    match outcome {
        Ok(reply) => Json(json!({ "reply": reply, "sent": true, "delivery": "completed" })).into_response(),
        Err(e) => super::federation_http::call_refusal(&e),
    }
}

#[cfg(test)]
mod tests {
    use crate::{GossipAgent, GossipConfig, NodeId};
    use std::{sync::Arc, time::Duration};

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    /// Item 1 PR 7: the commit JSON carries the receipt's tag beside the old bool, and the error
    /// field **only** for `failed` — the shape the live gateway test cannot induce.
    #[cfg(feature = "consensus")]
    #[test]
    fn commit_json_renders_the_receipt_beside_persisted() {
        use crate::LocalDurability;
        let on_disk = super::commit_json(true, &LocalDurability::OnDisk);
        assert_eq!(on_disk["persisted"], true);
        assert_eq!(on_disk["local_durability"], "on_disk");
        assert!(on_disk.get("local_durability_error").is_none());

        // The collapse the field undoes: `persisted: true` with nothing promised.
        let none = super::commit_json(true, &LocalDurability::NotConfigured);
        assert_eq!(none["persisted"], true);
        assert_eq!(none["local_durability"], "not_configured");

        let failed = super::commit_json(false, &LocalDurability::Failed("no ack".into()));
        assert_eq!(failed["persisted"], false);
        assert_eq!(failed["local_durability"], "failed");
        assert_eq!(failed["local_durability_error"], "no ack");
        assert_eq!(failed["ok"], true, "a failed *durability* is still a committed value");
    }

    /// Realignment repairs R9's third door (verification policy, 2026-10-06): `POST /gateway/govern/timing`
    /// published any value — every node's reconciler then ignored one outside `validate()`'s bounds, so
    /// the route answered `published: true` for an intent that governs nothing. It refuses it now, and a
    /// field that is present but not an integer is refused rather than read as `0` ("ungoverned").
    #[tokio::test]
    async fn govern_timing_refuses_what_no_node_would_apply() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        let url = format!("http://127.0.0.1:{http_port}/gateway/govern/timing");
        let client = reqwest::Client::new();
        let post = |body: serde_json::Value| { let (c, u) = (client.clone(), url.clone()); async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                match c.post(&u).json(&body).send().await {
                    Ok(r) => break r.status().as_u16(),
                    Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
                }
            }
        }};
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 99999})).await, 400, "above 3600");
        assert_eq!(post(serde_json::json!({"reconnect_backoff_secs": 301})).await, 400, "above 300");
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": "30"})).await, 400, "not an integer");
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "reconnect_backoff_secs": 5})).await, 200, "in range");
        // The adversarial review of #544: the route still published intents that govern nothing, or the
        // wrong thing — each was 200 and `published: true`.
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 0})).await, 400, "governs nothing");
        assert_eq!(post(serde_json::json!({})).await, 400, "an empty intent governs nothing");
        assert_eq!(post(serde_json::json!([30])).await, 400, "not an object");
        assert_eq!(post(serde_json::json!({"health_check_interval": 30})).await, 400, "a misspelled field");
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": "node-b"})).await, 400,
            "a target that is not a node id would have governed the whole fleet");
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": 5})).await, 400, "a target that is not a string");
        assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": format!("127.0.0.1:{gossip_port}")})).await, 200, "a node id");
        agent.shutdown().await;
    }

    /// The raw KV routes' classification is the namespace table's: every top-level prefix `src/lib.rs`
    /// documents is either owned (refused at the raw doors) or a named application namespace. A table row
    /// added without a decision here fails this test, so the doors cannot fall behind the table.
    #[test]
    fn every_namespace_in_the_table_is_classified_for_the_raw_kv_routes() {
        let table = include_str!("../lib.rs");
        let mut seen = 0;
        for line in table.lines().filter(|l| l.starts_with("//! | `")) {
            let cell = line.trim_start_matches("//! | `");
            let first = cell.split('`').next().unwrap_or("");
            let Some((top, _)) = first.split_once('/') else { continue };
            if top.is_empty() || top.chars().any(|c| c.is_ascii_uppercase()) || top.contains(' ') {
                continue;
            }
            let prefix = format!("{top}/");
            seen += 1;
            assert!(super::OWNED_KV_PREFIXES.contains(&prefix.as_str()) || super::APPLICATION_KV_PREFIXES.contains(&prefix.as_str()),
                "namespace {prefix} in src/lib.rs is neither owned nor an application namespace for the raw KV routes");
        }
        assert!(seen > 20, "the table was read ({seen} rows)");
    }

    /// The third review of #544: the timing route's strictness, at its two siblings. A `target` that was not
    /// a string, a `min` that was not an integer, a `floor` that was a string — each read as absent.
    #[tokio::test]
    async fn the_governance_routes_parse_strictly() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        let client = reqwest::Client::new();
        let post = |route: &'static str, body: serde_json::Value| { let c = client.clone(); async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                match c.post(format!("http://127.0.0.1:{http_port}/gateway/govern/{route}")).json(&body).send().await {
                    Ok(r) => break r.status().as_u16(),
                    Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
                }
            }
        }};
        assert_eq!(post("tuning", serde_json::json!({"enabled": true, "target": 5})).await, 400, "a target that is not a string");
        assert_eq!(post("tuning", serde_json::json!({"enabled": true, "enabeld": false})).await, 400, "an unknown field");
        assert_eq!(post("tuning", serde_json::json!({"params": [{"param": "inbound_fps", "floor": "5"}]})).await, 400, "a floor that is not an integer");
        assert_eq!(post("tuning", serde_json::json!({"enabled": true, "params": {"param": "writer_depth"}})).await, 400, "params that is not an array");
        assert_eq!(post("tuning", serde_json::json!({"params": [{"param": "inbound_fps", "ratchet": 1}]})).await, 400, "a ratchet that is not a string");
        assert_eq!(post("tuning", serde_json::json!({"enabled": true})).await, 200);
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": "3"})).await, 400, "a min that is not an integer");
        assert_eq!(post("membership", serde_json::json!({"group": "w"})).await, 400, "min is required");
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "target": ["x"]})).await, 400, "a target that is not a string");
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "max": "10"})).await, 400, "a max that is not an integer");
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "mn": 2})).await, 400, "an unknown field");
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "drain": "10.0.0.5:9000"})).await, 400, "a drain that is not an array");
        assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "max": null})).await, 200);
        agent.shutdown().await;
    }

    /// The reviews of #544 (enumerating every door to the timing intent): the doors that take a KV key
    /// wrote `sys/govern/…` under `kv:write`, publishing a governance intent without `govern:write`, and
    /// reached every other key the substrate owns. They refuse `sys/` and `consensus/` now, naming the route.
    #[tokio::test]
    async fn the_raw_kv_routes_refuse_governance_intents() {
        use base64::Engine as _;
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        // No token table: the gateway is open, so the caller holds every scope — the refusal must not
        // depend on scopes (as for protected kinds), so a `kv:write` token is refused a fortiori.
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        let base = format!("http://127.0.0.1:{http_port}/gateway/kv");
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let intent = base64::engine::general_purpose::STANDARD.encode(br#"{"health_check_interval_secs":5}"#);
        let r = loop {
            match client.post(&base)
                .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent})).send().await {
                Ok(r) => break r,
                Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
            }
        };
        assert_eq!(r.status(), 403, "a governance intent through the raw KV route");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["error"], "protected_key");
        assert!(agent.kv().get("sys/govern/timing").is_none(), "nothing was written");
        for key in ["sys/govern/fleet", "sys/govern/membership/workers"] {
            let r = client.delete(format!("{base}?key={key}")).send().await.unwrap();
            assert_eq!(r.status(), 403, "DELETE {key}");
        }
        let r = client.post(format!("{base}/quorum"))
            .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent, "min_acks": 0, "timeout_secs": 1})).send().await.unwrap();
        assert_eq!(r.status(), 403, "the quorum door");
        assert!(agent.kv().get("sys/govern/timing").is_none(), "nothing was written");
        // The third review of #544: the consensus door wrote the raw key too, and the doors reached every
        // other key the substrate owns — a planted caller-context marker fails a secure gateway's check
        // open; a forged commit or a deleted acceptor record undoes consensus.
        #[cfg(feature = "consensus")]
        {
            let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/overlay/consistent/set"))
                .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent})).send().await.unwrap();
            assert_eq!(r.status(), 403, "the consensus door");
        }
        for key in ["sys/caller-context/10.0.0.9:9000", "consensus/committed/slot-x", "sys/consensus-accepted/n/s", "sys/config/x", "sys/capauthz/ns/name"] {
            let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": ""})).send().await.unwrap();
            assert_eq!(r.status(), 403, "POST {key}");
        }
        // The kv:write decision (2026-10-07): the raw KV routes write application keys only. Every prefix
        // the namespace table assigns to the substrate or a companion is refused, naming its door — the
        // topology escape hatch included, which now has an audited governance route.
        for key in ["sys/topology-override/workers", "grp/workers/10.0.0.5:9000", "prompts/ns/name", "log/s/0001",
                    "cap/10.0.0.5:9000/demo/echo", "req/n/demo/echo", "mailbox/n/kind/01", "installable/ns/n/ab",
                    "tools/t/n", "skills/ns/n/node/input", "agent/n/task/1/turn", "lock/l", "svc/k/n",
                    "wiki/g/proposal/1", "tuple/inflight/ns/1", "facts/n/f", "cn/r", "mandate/m",
                    "knowledge/head/i/s", "rights/head/h", "clog/x", "comp/n/ns/k", "gcap/g/ns/n/c", "cap-group/g", "audit/1/n"] {
            let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": base64::engine::general_purpose::STANDARD.encode(b"true")})).send().await.unwrap();
            assert_eq!(r.status(), 403, "POST {key}");
            let body: serde_json::Value = r.json().await.unwrap();
            assert_eq!(body["error"], "protected_key", "{key}");
            assert!(agent.kv().get(key).is_none(), "{key} was not written");
        }
        // The application namespaces the table names stay writable: the checkpointer's rows, and an
        // application's provisioning error report.
        // `manifest/` and `schemas/` are application-written by design (src/lib.rs; mesh_manifest.rs) and have
        // no gateway route of their own — the review of #549.
        for key in ["ckpt/t/ns/1", "ckptw/t/ns/1/task/0", "agent/n/provision/item/error", "orders/42",
                    "manifest/current", "manifest/control/system", "schemas/s"] {
            let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": ""})).send().await.unwrap();
            assert_eq!(r.status(), 200, "POST {key}");
        }
        // The log door names the stream; it must not reach a stream another owner keeps under `log/` — a
        // commitment's offers, a wiki's durable proposals, a reason trace (the review of #549).
        #[cfg(feature = "consensus")]
        {
            let ov = format!("http://127.0.0.1:{http_port}/gateway/overlay/log");
            for stream in ["cn/req-1/offers", "wiki/g/proposals", "reason/run-1/n"] {
                let r = client.post(format!("{ov}/append")).json(&serde_json::json!({"stream": stream, "value_b64": ""})).send().await.unwrap();
                assert_eq!(r.status(), 403, "append to {stream}");
                let r = client.post(format!("{ov}/compact")).json(&serde_json::json!({"stream": stream, "before_hlc": u64::MAX})).send().await.unwrap();
                assert_eq!(r.status(), 403, "compact {stream}");
            }
            let r = client.post(format!("{ov}/append")).json(&serde_json::json!({"stream": "events", "value_b64": ""})).send().await.unwrap();
            assert_eq!(r.status(), 200, "an application stream");
        }
        // The override's own door: govern:write, audited; `true` engages it, `false` tombstones it.
        let over = format!("http://127.0.0.1:{http_port}/gateway/govern/topology-override");
        let r = client.post(&over).json(&serde_json::json!({"group": "workers", "override": true})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(agent.kv().get("sys/topology-override/workers").as_deref(), Some(&b"true"[..]));
        let r = client.post(&over).json(&serde_json::json!({"group": "workers", "override": false})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert!(agent.kv().get("sys/topology-override/workers").is_none(), "released");
        for bad in [serde_json::json!({"group": "workers"}), serde_json::json!({"group": "workers", "override": "true"}),
                    serde_json::json!({"override": true}), serde_json::json!({"group": "a/b", "override": true}),
                    serde_json::json!({"group": "workers", "override": true, "x": 1})] {
            let r = client.post(&over).json(&bad).send().await.unwrap();
            assert_eq!(r.status(), 400, "{bad}");
        }
        // An ordinary key is unaffected.
        let r = client.post(&base)
            .json(&serde_json::json!({"key": "app/x", "value_b64": ""})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        agent.shutdown().await;
    }

    /// Both signal SSE routes carry the kind in the data object as well as the SSE event name, so a
    /// client that reads the body alone is not wrong (realignment repairs §3.2: `mycelium-ts`'s old
    /// `raw.kind` was always undefined). Additive: the event name is unchanged.
    #[tokio::test]
    async fn signal_sse_data_carries_the_kind() {
        use futures_util::StreamExt as _;
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        for path in ["/gateway/signal/sse/repair.request", "/signals/repair.request"] {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let resp = loop {
                match reqwest::get(format!("http://127.0.0.1:{http_port}{path}")).await {
                    Ok(r) => break r,
                    Err(e) => {
                        assert!(tokio::time::Instant::now() < deadline, "{path} never opened: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            };
            assert_eq!(resp.status(), 200, "{path}");
            let mut body = resp.bytes_stream();
            let signal = crate::signal::Signal {
                kind:    Arc::from("repair.request"),
                scope:   crate::signal::SignalScope::Cluster,
                payload: bytes::Bytes::from_static(b"kettle"),
                sender:  id.clone(),
                nonce:   7,
            };
            // The subscription registers when the stream is polled; deliver until an event arrives.
            let mut seen = String::new();
            while !seen.contains("data:") {
                assert!(tokio::time::Instant::now() < deadline, "{path} delivered nothing");
                agent.task_ctx.signal_handlers.deliver(&signal);
                if let Ok(Some(Ok(chunk))) = tokio::time::timeout(Duration::from_millis(100), body.next()).await {
                    seen.push_str(&String::from_utf8_lossy(&chunk));
                }
            }
            assert!(seen.contains("event: repair.request"), "{path}: the event name is unchanged: {seen}");
            let data = seen.lines().find_map(|l| l.strip_prefix("data:")).expect("a data line").trim();
            let v: serde_json::Value = serde_json::from_str(data).unwrap();
            assert_eq!(v["kind"], "repair.request", "{path}: the data object names its kind: {data}");
            // One shape on both routes (doc-coverage run 20): `payload_b64` and the `nonce` on the
            // node-level route too; its `payload` stays for existing readers.
            assert_eq!(v["payload_b64"], "a2V0dGxl", "{path}: payload_b64: {data}");
            assert_eq!(v["nonce"], 7, "{path}: nonce: {data}");
        }
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_http_health_responds() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();

        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port  = gossip_port;
        cfg.http_port  = Some(http_port);
        cfg.http_addr  = "127.0.0.1".to_string();

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        // Brief pause for the HTTP server to bind and accept.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let url = format!("http://127.0.0.1:{http_port}/health");
        let resp = reqwest::get(&url).await.expect("HTTP request failed");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert!(body["node_id"].as_str().unwrap().contains("127.0.0.1"));

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_http_stats_responds() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();

        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let url = format!("http://127.0.0.1:{http_port}/stats");
        let resp = reqwest::get(&url).await.expect("stats request failed");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["store_entries"].is_number());
        assert!(body["dropped_frames"].is_number());

        agent.shutdown().await;
    }

    /// Providers currently resolvable for `(scrape, worker)` on this gateway.
    async fn provider_count(client: &reqwest::Client, base: &str) -> usize {
        let v: serde_json::Value = client
            .get(format!("{base}/gateway/capability/resolve?ns=scrape&name=worker"))
            .send().await.unwrap().json().await.unwrap();
        v["providers"].as_array().map(|a| a.len()).unwrap_or(0)
    }

    async fn start_test_agent() -> (Arc<GossipAgent>, String, reqwest::Client) {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        (agent, format!("http://127.0.0.1:{http_port}"), reqwest::Client::new())
    }

    /// Lease mode (2026-07-20): a bridged advertiser's refresh loop runs in
    /// THIS node's process, so without a lease the advert outlives a crashed
    /// client — provider liveness decoupled from refresher liveness (the w15
    /// stale-advert fingerprint). With `lease_secs`, a full window without a
    /// heartbeat must retract the advert exactly as DELETE would.
    #[tokio::test]
    async fn test_gateway_cap_lease_expires_without_heartbeat() {
        let (agent, base, client) = start_test_agent().await;

        let resp = client.post(format!("{base}/gateway/capability/advertise"))
            .json(&serde_json::json!({ "ns": "scrape", "name": "worker",
                                       "interval_secs": 1, "lease_secs": 1 }))
            .send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let handle_id = resp.json::<serde_json::Value>().await.unwrap()
            ["handle_id"].as_str().unwrap().to_string();

        // Advert appears (first persist tick is immediate).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while provider_count(&client, &base).await == 0 {
            assert!(std::time::Instant::now() < deadline, "advert never appeared");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // No heartbeats → the watchdog tombstones the advert.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while provider_count(&client, &base).await != 0 {
            assert!(std::time::Instant::now() < deadline, "leased advert was never retracted");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // The handle died with the lease: DELETE now 404s.
        let del = client.delete(format!("{base}/gateway/capability/{handle_id}"))
            .send().await.unwrap();
        assert_eq!(del.status(), 404);

        agent.shutdown().await;
    }

    /// Q2: an SDK agent's unit file declared through the gateway — the node's own loader parses
    /// it, capabilities, requirements and groups land under one handle, `DELETE` retracts them all,
    /// and the hosting sections and a broken file are refused by name. Seen failing first: before
    /// the route existed the POST answered 404.
    #[tokio::test]
    async fn test_gateway_units_declare_one_handle_for_a_whole_unit() {
        let (agent, base, client) = start_test_agent().await;
        let node = agent.node_id().to_string();
        let unit = "principal = \"sdk-planner\"\n\
            [[capability]]\nns = \"plan\"\nname = \"route\"\nttl_secs = 30\n  [capability.attrs]\n  region = \"north\"\n\
            [[requirement]]\nns = \"data\"\nname = \"realtime\"\n  [requirement.attrs]\n  hz = { gte = 10 }\n\
            [[group]]\nname = \"routers\"\n  [group.filter]\n  ns = \"plan\"\n  name = \"route\"\n\
            [[lane]]\nname = \"plans\"\nrole = \"produces\"\n";
        let resp = client.post(format!("{base}/gateway/units/declare"))
            .json(&serde_json::json!({ "toml": unit, "interval_secs": 1 }))
            .send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["principal"], "sdk-planner");
        assert_eq!(body["declared"], serde_json::json!({"capabilities": 1, "requirements": 1, "groups": 1}));
        assert_eq!(body["not_enforced"], serde_json::json!(["[[lane]]"]));
        let handle_id = body["handle_id"].as_str().unwrap().to_string();

        let cap_key = format!("cap/{node}/plan/route");
        let req_key = format!("req/{node}/data/realtime");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while agent.kv().get(&cap_key).is_none() || agent.kv().get(&req_key).is_none() || agent.kv().get("cap-group/routers").is_none() {
            assert!(std::time::Instant::now() < deadline, "the capability, the requirement and the group were declared");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // One handle retracts the whole unit.
        let del = client.delete(format!("{base}/gateway/capability/{handle_id}")).send().await.unwrap();
        assert_eq!(del.status(), 200);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while agent.kv().get(&cap_key).is_some() || agent.kv().get(&req_key).is_some() {
            assert!(std::time::Instant::now() < deadline, "DELETE retracted the capability and the requirement");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Refusals, by name.
        let r = client.post(format!("{base}/gateway/units/declare"))
            .json(&serde_json::json!({ "toml": "[hosts]\nkinds = [\"blob\"]\naccept_unsigned = true\n" })).send().await.unwrap();
        assert_eq!(r.status(), 422);
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "hosting sections");
        let r = client.post(format!("{base}/gateway/units/declare"))
            .json(&serde_json::json!({ "toml": "[[presence]]\nns = \"a\"\nname = \"b\"\nmin_providers = 0\n" })).send().await.unwrap();
        assert_eq!(r.status(), 400, "validate() runs: a floor of zero is refused before the hosting check");
        let r = client.post(format!("{base}/gateway/units/declare"))
            .json(&serde_json::json!({ "toml": "not = [toml" })).send().await.unwrap();
        assert_eq!(r.status(), 400);
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "invalid unit file");

        agent.shutdown().await;
    }

    /// The complement: heartbeats within the window keep a leased advert alive
    /// past many lease periods, and a lease-less handle rejects heartbeats (409).
    #[tokio::test]
    async fn test_gateway_cap_lease_heartbeat_keeps_alive() {
        let (agent, base, client) = start_test_agent().await;

        let resp = client.post(format!("{base}/gateway/capability/advertise"))
            .json(&serde_json::json!({ "ns": "scrape", "name": "worker",
                                       "interval_secs": 1, "lease_secs": 1 }))
            .send().await.unwrap();
        let handle_id = resp.json::<serde_json::Value>().await.unwrap()
            ["handle_id"].as_str().unwrap().to_string();

        // Beat every 300 ms for 2.4 s — several full lease windows.
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let hb = client.post(format!("{base}/gateway/capability/{handle_id}/heartbeat"))
                .send().await.unwrap();
            assert_eq!(hb.status(), 200);
        }
        assert_eq!(provider_count(&client, &base).await, 1,
                   "heartbeated advert must stay live across lease windows");

        // Heartbeats stop → retraction.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while provider_count(&client, &base).await != 0 {
            assert!(std::time::Instant::now() < deadline, "advert not retracted after heartbeats stopped");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // A handle advertised WITHOUT lease_secs has no lease to renew: 409.
        let resp = client.post(format!("{base}/gateway/capability/advertise"))
            .json(&serde_json::json!({ "ns": "scrape", "name": "worker", "interval_secs": 1 }))
            .send().await.unwrap();
        let durable_id = resp.json::<serde_json::Value>().await.unwrap()
            ["handle_id"].as_str().unwrap().to_string();
        let hb = client.post(format!("{base}/gateway/capability/{durable_id}/heartbeat"))
            .send().await.unwrap();
        assert_eq!(hb.status(), 409);

        agent.shutdown().await;
    }

    /// Native gateway TLS (SOC 2 WS-A, 2026-07-22): with `gateway_tls` set (node-cert
    /// reuse), the gateway serves HTTPS — bearer tokens/JWTs never traverse cleartext.
    /// A faithful client (rustls, trusting the generated cluster CA, IP-SAN match) must
    /// complete a real handshake and get `/health` 200; a plaintext client must fail.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn test_gateway_serves_native_tls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("gw-tls-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.http_addr = "127.0.0.1".to_string();
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        cfg.gateway_tls = Some(crate::GatewayTlsConfig::default()); // reuse node cert

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Build a rustls client that trusts the cluster CA the node just generated.
        let ca_pem = std::fs::read(cert_dir.join("ca-cert.pem")).expect("ca-cert.pem written");
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_pem)) {
            roots.add(c.unwrap()).unwrap();
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
        let server_name = rustls::pki_types::ServerName::IpAddress(
            std::net::Ipv4Addr::new(127, 0, 0, 1).into());
        let mut tls = connector.connect(server_name, tcp).await
            .expect("TLS handshake against the gateway (server must speak TLS)");
        tls.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await.unwrap();
        let mut buf = Vec::new();
        tls.read_to_end(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.contains("200 OK"), "expected 200 over HTTPS, got: {resp}");
        assert!(resp.contains("\"status\":\"ok\""), "health body over HTTPS: {resp}");

        // Negative: a plaintext HTTP client must NOT get a valid HTTP response — the
        // server now expects a TLS ClientHello, so a raw GET yields no "200 OK".
        let mut plain = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
        plain.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await.unwrap();
        let mut pbuf = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(2), plain.read_to_end(&mut pbuf)).await;
        assert!(!String::from_utf8_lossy(&pbuf).contains("200 OK"),
            "plaintext HTTP must fail against a TLS gateway");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// Identity-auth Phase 2 (SOC 2 WS-E): signed-proof **prevention**. Poisoning — a
    /// `sys/identity/{V}` overwrite whose proof is signed by an untrusted key — must be
    /// **rejected** (the foreign key never enters `peer_keys[V]`), while a legitimate self-signed
    /// entry is accepted. Single-node: drive the merge helper directly against a controlled store.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn test_identity_proof_rejects_poisoning_accepts_signed() {
        use ed25519_dalek::{Signer, SigningKey};
        let anchor_keys: papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> =
            papaya::HashMap::new();
        let peer_keys: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
        let conflicts = std::sync::atomic::AtomicU64::new(0);
        let victim = NodeId::new("127.0.0.1", 7001).unwrap();

        // V's real key. Anchor it (as Phase 1b would on a live connection).
        let v_sk = SigningKey::from_bytes(&[7u8; 32]);
        let v_key = v_sk.verifying_key().to_bytes();
        anchor_keys.pin().insert(victim.clone(), std::collections::HashSet::from([v_key]));

        // ── Poisoning: attacker M writes sys/identity/{V} = v_key‖m_key + a proof signed by M's
        //    own key (NOT trusted for V). Must be rejected — m_key must not enter peer_keys[V].
        let m_sk = SigningKey::from_bytes(&[66u8; 32]);
        let m_key = m_sk.verifying_key().to_bytes();
        let mut history = v_key.to_vec();
        history.extend_from_slice(&m_key);
        let m_sig = m_sk.sign(&crate::agent::helpers::identity_proof_message(&history)).to_bytes();
        let bad_proof = crate::agent::helpers::encode_identity_proof(&m_key, &m_sig);
        let kv_keys = [v_key, m_key];
        crate::agent::helpers::validate_and_merge_identity(
            &peer_keys, &anchor_keys, &conflicts, &victim, &history, &kv_keys, Some(&bad_proof), false);
        assert!(!peer_keys.pin().get(&victim).map(|v| v.contains(&m_key)).unwrap_or(false),
                "poisoning rejected: M's key must NOT be trusted for V");
        assert_eq!(conflicts.load(std::sync::atomic::Ordering::Relaxed), 1, "conflict counted");

        // ── Legitimate: V rotates, history = v2‖v_key, proof signed by the PRIOR (trusted) key.
        //    Must be accepted — v2 enters peer_keys[V].
        let v2_sk = SigningKey::from_bytes(&[8u8; 32]);
        let v2_key = v2_sk.verifying_key().to_bytes();
        let mut hist2 = v2_key.to_vec();
        hist2.extend_from_slice(&v_key);
        let good_sig = v_sk.sign(&crate::agent::helpers::identity_proof_message(&hist2)).to_bytes(); // signed by the prior key
        let good_proof = crate::agent::helpers::encode_identity_proof(&v_key, &good_sig);
        crate::agent::helpers::validate_and_merge_identity(
            &peer_keys, &anchor_keys, &conflicts, &victim, &hist2, &[v2_key, v_key], Some(&good_proof), false);
        assert!(peer_keys.pin().get(&victim).unwrap().contains(&v2_key),
                "legitimate rotation chained by the prior key is accepted");
    }

    /// Identity-auth Phase 3 (SOC 2 WS-E): with `require_identity_proofs`, an **unsigned**
    /// `sys/identity` entry (mimicking a pre-Phase-2 node) is **rejected** — the last poisoning
    /// residual — whereas the same entry is tolerated when the flag is off (rollout).
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn test_require_identity_proofs_rejects_unsigned() {
        use ed25519_dalek::SigningKey;
        let anchor_keys: papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> =
            papaya::HashMap::new();
        let node = NodeId::new("127.0.0.1", 7002).unwrap();
        let k = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
        let history = k.to_vec();

        // Flag OFF (**the default** — the 2026-09-23 flip to on was reverted the next day; see
        // `config::tests::the_default_requires_identity_proofs`): unsigned accepted.
        let pk_off: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
        let c_off = std::sync::atomic::AtomicU64::new(0);
        crate::agent::helpers::validate_and_merge_identity(
            &pk_off, &anchor_keys, &c_off, &node, &history, &[k], None, false);
        assert!(pk_off.pin().get(&node).is_some_and(|v| v.contains(&k)),
                "flag off: unsigned entry accepted (rollout tolerance)");

        // Flag ON (Phase 3, the operator opt-in): the same unsigned entry rejected + counted.
        let pk_on: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
        let c_on = std::sync::atomic::AtomicU64::new(0);
        crate::agent::helpers::validate_and_merge_identity(
            &pk_on, &anchor_keys, &c_on, &node, &history, &[k], None, true);
        assert!(pk_on.pin().get(&node).is_none(), "flag on: unsigned entry rejected");
        assert_eq!(c_on.load(std::sync::atomic::Ordering::Relaxed), 1, "rejection counted");
    }

    /// Identity-auth Phase 1b (SOC 2 WS-E): a directly-connected TLS peer's CA-validated
    /// key is harvested from its cert into an authenticated anchor; a `sys/identity` KV entry
    /// introducing a *different* key then trips the conflict counter (the poisoning signal).
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn test_identity_anchor_recorded_and_conflict_flagged() {
        let cert_dir = std::env::temp_dir().join(format!("wse-anchor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let pa = alloc_port();
        let pb = alloc_port();
        let mk = |port: u16, boot: Vec<NodeId>| {
            let mut cfg = GossipConfig::default();
            cfg.bind_port = port;
            cfg.bootstrap_peers = boot;
            cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
            Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
        };
        let a = mk(pa, vec![]);
        let b = mk(pb, vec![NodeId::new("127.0.0.1", pa).unwrap()]);
        a.start().await.unwrap();
        b.start().await.unwrap();

        // Wait until B has anchored A (B dialed A, so B's outbound writer harvested A's cert key).
        let ida = NodeId::new("127.0.0.1", pa).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let anchored = loop {
            if b.task_ctx.peer_anchor_keys.pin().get(&ida).is_some_and(|s| !s.is_empty()) {
                break true;
            }
            if std::time::Instant::now() > deadline { break false; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(anchored, "B must anchor A's CA-validated key after dialing it");

        // The anchored key equals A's real identity key.
        let a_real = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
        assert!(b.task_ctx.peer_anchor_keys.pin().get(&ida).unwrap().contains(&a_real),
                "the anchor is A's actual identity key");

        // Since identity and its proof travel as one sealed record (v2.14.0), the watcher reads
        // `sys/identity-signed/{A}` first and the legacy pair only when no sealed record is held
        // (`helpers::resolve_identity_record`). So a poisoned legacy `sys/identity/{A}` is
        // **masked** while B holds A's sealed record — the sealed record is authoritative — and
        // whether the tripwire fired used to depend on whether that record had arrived yet: a
        // race this test lost once in nine runs of main (the v2.17.0 release commit, 2026-09-30).
        // Now the test waits for the sealed record, shows the masking, and then poisons the
        // record the watcher actually reads: the sealed record is tombstoned (any node can gossip
        // a newer tombstone) and the legacy pair carries the foreign key.
        let sealed_key = format!("sys/identity-signed/{ida}");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while b.kv().get(&sealed_key).is_none() {
            assert!(std::time::Instant::now() < deadline, "B must hold A's sealed identity record");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let before = b.system_stats().identity_anchor_conflicts;
        let foreign = [0x42u8; 32];
        let mut poisoned = a_real.to_vec();
        poisoned.extend_from_slice(&foreign);
        // The masked case: with the sealed record present, the legacy poison changes nothing.
        let _ = b.kv().set(format!("sys/identity/{ida}"), bytes::Bytes::from(poisoned.clone()));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(b.system_stats().identity_anchor_conflicts, before,
            "a legacy poison is masked while the sealed record is held: the sealed record is authoritative");
        // The real poisoning: the sealed record gone, the legacy pair introduces a foreign key —
        // the watcher re-scans (both keys sit under the `sys/identity` prefix) and reads the pair.
        let _ = b.kv().delete(sealed_key);
        let _ = b.kv().set(format!("sys/identity/{ida}"), bytes::Bytes::from(poisoned));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if b.system_stats().identity_anchor_conflicts > before { break; }
            assert!(std::time::Instant::now() < deadline, "conflict tripwire never fired");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        a.shutdown().await;
        b.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// SOC 2 WS-D: after a checkpoint, pruned records still verify from the signed
    /// checkpoint boundary (not genesis) — the mechanism that makes retention safe.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_audit_checkpoint_prune_and_verify() {
        let gossip_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("wsd-ckpt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        for i in 0..6u32 {
            agent.audit(crate::AuditAction::Write, "op", format!("t-{i}"),
                        crate::AuditOutcome::Success, None).unwrap();
        }
        assert_eq!(agent.audit_verify(&id), Ok(()), "full chain verifies before checkpoint");

        // Checkpoint at the current boundary (6), then prune the exported prefix.
        let (cp_seq, _) = agent.audit_checkpoint().unwrap();
        assert_eq!(cp_seq, 6);
        // Seal 2 more, then prune everything below the checkpoint (records 0..6).
        for i in 6..8u32 {
            agent.audit(crate::AuditAction::Write, "op", format!("t-{i}"),
                        crate::AuditOutcome::Success, None).unwrap();
        }
        let pruned = agent.audit_prune_to_checkpoint();
        assert_eq!(pruned, 6, "records 0..6 pruned");
        assert_eq!(agent.audit_stream(&id).len(), 2, "records 6,7 remain");

        // The pruned stream still verifies — from the checkpoint boundary, not genesis.
        assert_eq!(agent.audit_verify(&id), Ok(()),
                   "pruned stream verifies from the signed checkpoint");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// SOC 2 WS-C: an attached AuditSink receives every sealed record, off the write
    /// path. Seal a few audit events and assert the sink captured them in order, while
    /// the authoritative chain still verifies.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_audit_sink_mirrors_sealed_records() {
        use std::sync::Mutex as StdMutex;

        struct CapturingSink(Arc<StdMutex<Vec<u64>>>);
        impl crate::AuditSink for CapturingSink {
            fn export(&self, record: &crate::SignedAuditRecord) {
                self.0.lock().unwrap().push(record.record.seq);
            }
        }

        let gossip_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("wsc-sink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(id, cfg));

        let captured = Arc::new(StdMutex::new(Vec::new()));
        agent.with_audit_sink(Arc::new(CapturingSink(Arc::clone(&captured))));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        for i in 0..5u32 {
            agent.audit(crate::AuditAction::Write, "op", format!("target-{i}"),
                        crate::AuditOutcome::Success, None).unwrap();
        }

        // Drain task runs off-path — poll until the sink has all 5, in seal order.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if captured.lock().unwrap().len() >= 5 { break; }
            assert!(std::time::Instant::now() < deadline, "sink never received all records");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(*captured.lock().unwrap(), vec![0, 1, 2, 3, 4], "records mirrored in seal order");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// SOC 2 WS-B: rotation alone is hygiene (the old key stays accepted); the
    /// compromise flow rotates AND revokes the old key, so it stops verifying
    /// cluster-wide. A revocation must be recorded after `rotate_identity_on_compromise`
    /// and none before.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_rotate_on_compromise_revokes_old_key() {
        let gossip_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("wsb-compromise-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(agent.revocation_head().is_none(), "no revocations before the compromise rotation");

        agent.rotate_identity_on_compromise(Duration::from_millis(50)).await.unwrap();

        let (_root, count) = agent.revocation_head()
            .expect("the outgoing key must be revoked by the compromise flow");
        assert_eq!(count, 1, "exactly the old key revoked");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// The adversarial review of #584 (finding 5): a WAL writer refusing appends after a failed
    /// write was visible only in the log. `/health` now carries a `persistence` block —
    /// `wal_refusing_appends` with the reason — that flips on and, once a snapshot truncates the
    /// torn tail, off. Seen failing first: the block was absent (`null`).
    #[cfg(unix)]
    #[tokio::test]
    async fn health_says_when_the_wal_writer_refuses_appends() {
        use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let base = std::env::temp_dir().join(format!("myc-health-wal-{gossip_port}"));
        let _ = std::fs::remove_dir_all(&base);
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.persistence = Some(PersistenceConfig { base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        let url = format!("http://127.0.0.1:{http_port}/health");
        let client = reqwest::Client::builder().timeout(Duration::from_millis(500)).build().unwrap();
        let mut body = None;
        for _ in 0..100 {
            if let Ok(resp) = client.get(&url).send().await && resp.status() == 200 {
                body = Some(resp.json::<serde_json::Value>().await.unwrap());
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let body = body.expect("gateway /health never returned 200");
        assert_eq!(body["persistence"]["configured"], serde_json::json!(true), "{body}");
        assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(false), "a healthy writer: {body}");

        let wal = Arc::clone(agent.task_ctx.wal.get().expect("persistence configured"));
        wal.poison_for_test("injected write failure").await.unwrap();
        let body: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(true), "{body}");
        assert_eq!(body["persistence"]["reason"], serde_json::json!("injected write failure"), "{body}");
        assert!(agent.kv().set_requiring_sync(&mycelium_core::receipt::OperationId::new("h-refused"), "h/refused", b"v".to_vec()).await.is_err(),
            "an append is refused while the writer is poisoned");

        wal.trigger_snapshot().await.expect("the repairing snapshot");
        let body: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(false), "recovered: {body}");
        assert_eq!(body["persistence"]["reason"], serde_json::Value::Null);
        assert!(body["persistence"]["dropped_appends"].as_u64().unwrap() >= 1, "the refusal was counted: {body}");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Operational-readiness invariant: shutdown must actually close the
    /// gateway port. A load balancer drains a node by observing connection
    /// refusal; a zombie listener that keeps accepting after shutdown() would
    /// answer health checks from a dead agent (M2 Run-22 probe).
    #[tokio::test]
    async fn test_gateway_port_closes_on_shutdown() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();

        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();

        let url = format!("http://127.0.0.1:{http_port}/health");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(250))
            .build()
            .unwrap();
        // Poll until the HTTP server has bound — a fixed sleep races server startup under load.
        let mut up = false;
        for _ in 0..100 {
            if let Ok(resp) = client.get(&url).send().await
                && resp.status() == 200
            {
                up = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(up, "gateway /health never returned 200");

        agent.shutdown().await;

        // Poll briefly: the server task abort is asynchronous, but the port
        // must stop accepting within the shutdown grace window.
        let mut closed = false;
        for _ in 0..40 {
            if reqwest::Client::builder()
                .timeout(Duration::from_millis(250))
                .build()
                .unwrap()
                .get(&url)
                .send()
                .await
                .is_err()
            {
                closed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(closed, "gateway port still accepting after shutdown");
    }

    /// gw_llm_call reports failures via HTTP status codes (the gateway-wide
    /// convention), not a 200 + error-JSON envelope: a no-provider miss is a
    /// 404 so plain `curl -f` / `raise_for_status()` callers see the failure.
    #[cfg(feature = "llm")]
    #[tokio::test]
    async fn test_llm_call_no_provider_returns_404() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();

        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let url = format!("http://127.0.0.1:{http_port}/gateway/llm/call");
        let resp = reqwest::Client::new()
            .post(&url)
            .json(&serde_json::json!({"ns":"nobody","name":"provides-this","input":"x"}))
            .send()
            .await
            .expect("llm/call request failed");
        assert_eq!(resp.status(), 404, "no provider must surface as HTTP 404");

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "no_provider", "error JSON body is kept alongside the status");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_sse_delivers_signals() {
        use crate::signal::SignalScope;
        use bytes::Bytes;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();

        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        // Must be in the "test-sse" group to admit the signal.
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.mesh().join_group("test-sse");
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect SSE client.
        let url = format!("http://127.0.0.1:{http_port}/signals/sse-probe");
        let mut resp = reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .expect("SSE connect failed");
        assert_eq!(resp.status(), 200);

        // Emit a signal to self.
        let _ = agent.mesh().emit("sse-probe", SignalScope::Cluster, Bytes::from_static(b"payload"));

        // Read SSE chunks until we see the expected event or timeout.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut found = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), resp.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    if text.contains("sse-probe") {
                        found = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(found, "SSE event for 'sse-probe' was not received within timeout");

        agent.shutdown().await;
    }

    // ── MCP endpoint tests ────────────────────────────────────────────────────

    fn mcp_agent(http_port: u16) -> Arc<GossipAgent> {
        let gossip_port = alloc_port();
        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        Arc::new(GossipAgent::new(id, cfg))
    }

    #[tokio::test]
    async fn test_mcp_initialize() {
        let http_port = alloc_port();
        let agent = mcp_agent(http_port);
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1.0"},
                },
            }))
            .send()
            .await
            .expect("initialize request failed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["protocolVersion"], "2024-11-05");
        assert!(body["result"]["serverInfo"]["name"].as_str().is_some());

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let http_port = alloc_port();
        let agent = mcp_agent(http_port);
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let _handle = agent.mcp().register_mcp_tool(
            "greet",
            serde_json::json!({
                "type": "object",
                "description": "Greets a person",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
            }),
            |args| async move {
                Ok(serde_json::json!(format!("hello, {}", args["name"].as_str().unwrap_or("?"))))
            },
        );

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {},
            }))
            .send()
            .await
            .expect("tools/list request failed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        let tools = body["result"]["tools"].as_array().unwrap();
        assert!(
            tools.iter().any(|t| t["name"] == "greet"),
            "tool 'greet' not in list: {body}"
        );

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_tools_call_round_trip() {
        let http_port = alloc_port();
        let agent = mcp_agent(http_port);
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let _handle = agent.mcp().register_mcp_tool(
            "square",
            serde_json::json!({
                "type": "object",
                "properties": {"n": {"type": "number"}},
                "required": ["n"],
            }),
            |args| async move {
                let n = args["n"].as_f64().unwrap_or(0.0);
                Ok(serde_json::json!(n * n))
            },
        );

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "square", "arguments": {"n": 5.0}},
            }))
            .send()
            .await
            .expect("tools/call request failed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("25"), "expected 25, got '{text}'");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_tools_call_not_found() {
        let http_port = alloc_port();
        let agent = mcp_agent(http_port);
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "no-such-tool", "arguments": {}},
            }))
            .send()
            .await
            .expect("tools/call request failed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32601);
        assert!(
            body["error"]["message"].as_str().unwrap().contains("no-such-tool"),
            "unexpected error message: {body}"
        );

        agent.shutdown().await;
    }

    // ── WS1 gateway OAuth2 scope ACLs (compliance feature) ────────────────

    #[cfg(feature = "compliance")]
    #[test]
    fn required_scope_table_maps_families_and_denies_by_default() {
        use axum::http::Method;
        use super::required_scope;
        // read/write split on the same path keys off the method.
        assert_eq!(required_scope(&Method::GET,    "/gateway/kv"), "kv:read");
        assert_eq!(required_scope(&Method::POST,   "/gateway/kv"), "kv:write");
        assert_eq!(required_scope(&Method::DELETE, "/gateway/kv"), "kv:write");
        // resource families.
        assert_eq!(required_scope(&Method::GET,  "/gateway/capability/resolve"), "cap:read");
        assert_eq!(required_scope(&Method::POST, "/gateway/signal/emit"), "mesh:write");
        assert_eq!(required_scope(&Method::GET,  "/gateway/rpc/serve/{kind}"), "mesh:serve");
        assert_eq!(required_scope(&Method::POST, "/gateway/rpc/respond"), "mesh:serve");
        assert_eq!(required_scope(&Method::POST, "/gateway/rpc/call"), "mesh:write");
        assert_eq!(required_scope(&Method::POST, "/gateway/overlay/consistent/set"), "consensus:write");
        assert_eq!(required_scope(&Method::GET,  "/gateway/overlay/consistent/get"), "consensus:read");
        assert_eq!(required_scope(&Method::POST, "/gateway/llm/call"), "llm:invoke");
        assert_eq!(required_scope(&Method::POST, "/gateway/govern/group"), "govern:write");
        assert_eq!(required_scope(&Method::DELETE, "/gateway/govern/group"), "govern:write");
        assert_eq!(required_scope(&Method::GET,  "/gateway/fleet"), "fleet:read");
        assert_eq!(required_scope(&Method::GET,  "/gateway/explain"), "fleet:read");
        assert_eq!(required_scope(&Method::GET,  "/gateway/diagnose"), "fleet:read");
        // companion families (merged routes, 2026-09-04).
        assert_eq!(required_scope(&Method::POST, "/gateway/reason/route"), "llm:invoke");
        // Node-level routes gated since 2026-09-05.
        assert_eq!(required_scope(&Method::POST, "/mcp"),              "mcp:invoke");
        assert_eq!(required_scope(&Method::GET,  "/signals/{kind}"),   "mesh:read");
        assert_eq!(required_scope(&Method::GET,  "/consensus/{*slot}"), "consensus:read");
        assert_eq!(required_scope(&Method::POST, "/gateway/reason/v1/chat/completions"), "llm:invoke");
        assert_eq!(required_scope(&Method::GET,  "/gateway/reason/trace/{run_id}"), "llm:read");
        assert_eq!(required_scope(&Method::PUT,  "/gateway/reason/blob"), "llm:write");
        assert_eq!(required_scope(&Method::POST, "/gateway/wiki/query"), "wiki:read");
        // Federation's consumer side (item 2 row 11): reading about partners and spending a
        // credential on one are different powers, and the table is where that is decided.
        assert_eq!(required_scope(&Method::GET,  "/gateway/federation/domain"), "federation:read");
        assert_eq!(required_scope(&Method::GET,  "/gateway/federation/partners"), "federation:read");
        assert_eq!(required_scope(&Method::GET,  "/gateway/federation/catalog/{domain}"), "federation:read");
        assert_eq!(required_scope(&Method::POST, "/gateway/federation/connect"), "federation:invoke");
        assert_eq!(required_scope(&Method::POST, "/gateway/federation/call"), "federation:invoke");
        assert_eq!(required_scope(&Method::POST, "/gateway/wiki/ingest"), "wiki:write");
        assert_eq!(required_scope(&Method::GET,  "/gateway/bb/depth"), "board:read");
        assert_eq!(required_scope(&Method::POST, "/gateway/tuple/take"), "tuple:write");
        assert_eq!(required_scope(&Method::POST, "/gateway/artifacts/publish"), "artifact:publish");
        assert_eq!(required_scope(&Method::POST, "/gateway/units/declare"), "cap:write");
        // deny-by-default: anything unmapped requires admin — including an unlisted companion path.
        assert_eq!(required_scope(&Method::POST, "/gateway/some/future/route"), "admin");
        assert_eq!(required_scope(&Method::POST, "/gateway/wiki/some/future/verb"), "admin");
    }

    #[cfg(feature = "compliance")]
    #[test]
    fn scope_admits_exact_and_wildcard_only() {
        use super::scope_admits;
        let ro = vec!["kv:read".to_string()];
        assert!(scope_admits(&ro, "kv:read"));
        assert!(!scope_admits(&ro, "kv:write"));
        let star = vec!["*".to_string()];
        assert!(scope_admits(&star, "kv:write"));
        assert!(scope_admits(&star, "admin"));
        // Empty grant admits nothing.
        assert!(!scope_admits(&[], "kv:read"));
    }

    #[cfg(feature = "compliance")]
    #[test]
    fn resolve_token_scopes_legacy_is_wildcard() {
        use super::resolve_token;
        let mut cfg = GossipConfig::default();
        cfg.gateway_auth_token = Some("legacy-tok".to_string());
        cfg.gateway_scoped_tokens = vec![crate::GatewayToken {
            token:  "ro-tok".to_string(),
            scopes: vec!["kv:read".to_string()],
        }];
        cfg.gateway_named_tokens = vec![crate::GatewayNamedToken {
            name: "ci-bot".to_string(), token: "named-tok".to_string(), scopes: vec!["kv:write".to_string()],
        }];
        // Legacy token → superuser wildcard (unchanged upgrade path), principal qualified by the issuer.
        assert_eq!(resolve_token(&cfg, "gw-1", "legacy-tok"),
                   Some(("token:gw-1/legacy".to_string(), vec!["*".to_string()])));
        // Named token → its grant, principal by name (never the secret, stable under reordering).
        assert_eq!(resolve_token(&cfg, "gw-1", "named-tok"), Some(("token:gw-1/ci-bot".to_string(), vec!["kv:write".to_string()])));
        // Positional token → its grant, principal by position.
        assert_eq!(resolve_token(&cfg, "gw-1", "ro-tok"), Some(("token:gw-1/#0".to_string(), vec!["kv:read".to_string()])));
        // Unknown token → None (unauthenticated).
        assert_eq!(resolve_token(&cfg, "gw-1", "nope"), None);
    }

    #[cfg(feature = "compliance")]
    #[test]
    fn regression_parse_hex32_rejects_non_ascii_without_panic() {
        use super::parse_hex32;
        // 64 BYTES but not 64 chars: one 3-byte '€' + 61 ASCII. The old code byte-sliced after a
        // BYTE-length check and panicked on the non-char-boundary (node-abort under panic=abort).
        // Must return None, never panic (audit 2026-07-15 pass 2).
        let s = format!("€{}", "a".repeat(61));
        assert_eq!(s.len(), 64, "precondition: 64 bytes, <64 chars");
        assert_eq!(parse_hex32(&s), None, "non-ASCII 64-byte input must be rejected, not panic");
        // Valid 64-hex still parses; wrong lengths rejected.
        assert!(parse_hex32(&"ab".repeat(32)).is_some());
        assert_eq!(parse_hex32("abcd"), None);
    }

    /// Review 2026-09-05 finding 4: `/mcp`, `/signals/{kind}` and `/consensus/{*slot}` answered
    /// without a bearer when `gateway_auth_token` was set — `tools/call` invoked any cluster tool
    /// with this node's identity. They now sit behind the gateway's bearer boundary. The M16
    /// public set (`/health`, `/ready`, `/stats`, `/metrics`) and the nonce-capability
    /// `/bulk/{id}` stay open — never 401.
    #[tokio::test]
    /// Identifiers in paths (contracts-axis plan §9, 2026-09-06): every slot the substrate mints
    /// is hierarchical (`lock/{name}`, `consistent/{key}`, `leader/{group}`), so the read route
    /// captures the **path tail** (`/consensus/{*slot}`) and an operator can type the slot as a
    /// receipt shows it. Before this the pattern was one-segment and the runbooks' literal URL
    /// 404ed for two months (wiki-lint ledger 2026-09-06). Both the literal and the
    /// percent-encoded form must reach the handler and name the same slot. `consensus`-gated
    /// because the route is.
    #[cfg(feature = "consensus")]
    async fn regression_consensus_slot_route_accepts_hierarchical_slots() {
        use axum::http::header::AUTHORIZATION;
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let literal = client.get(format!("{base}/consensus/lock/x"))
            .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        let encoded = client.get(format!("{base}/consensus/lock%2Fx"))
            .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        let (ls, es) = (literal.status(), encoded.status());
        let lb = literal.text().await.unwrap_or_default();
        let eb = encoded.text().await.unwrap_or_default();
        agent.shutdown().await;
        assert_eq!(ls, 200, "literal hierarchical slot reaches the handler (body: {lb})");
        assert_eq!(es, 200, "percent-encoded slot still reaches the handler (body: {eb})");
        let lv: serde_json::Value = serde_json::from_str(&lb).unwrap();
        let ev: serde_json::Value = serde_json::from_str(&eb).unwrap();
        assert_eq!(lv["slot"], "lock/x");
        assert_eq!(ev["slot"], "lock/x", "the extractor decodes the tail");
    }

    #[tokio::test]
    async fn regression_node_level_routes_require_bearer_when_token_set() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});

        // Gated: 401 bare, 200 with the configured token.
        let r = client.post(format!("{base}/mcp")).json(&init).send().await.unwrap();
        assert_eq!(r.status(), 401, "/mcp must demand the bearer when a token is set");
        let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer secret")
            .json(&init).send().await.unwrap();
        assert_eq!(r.status(), 200, "the configured token admits /mcp");

        let r = client.get(format!("{base}/signals/probe-kind")).send().await.unwrap();
        assert_eq!(r.status(), 401, "/signals/{{kind}} must demand the bearer");
        let r = client.get(format!("{base}/signals/probe-kind"))
            .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        assert_eq!(r.status(), 200, "the configured token admits the SSE stream");
        drop(r);

        #[cfg(feature = "consensus")]
        {
            let r = client.get(format!("{base}/consensus/some-slot")).send().await.unwrap();
            assert_eq!(r.status(), 401, "/consensus/{{slot}} must demand the bearer");
            let r = client.get(format!("{base}/consensus/some-slot"))
                .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
            assert_eq!(r.status(), 200, "the configured token admits slot inspection");
        }

        // Public by design — never 401. (`/ready` may be 503 this early; `/metrics` is 404
        // without the `metrics` feature — the property under test is "no bearer demanded".)
        for path in ["/health", "/ready", "/stats", "/metrics"] {
            let r = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_ne!(r.status(), 401, "{path} is public (M16 edge criterion)");
        }
        let r = client.get(format!("{base}/bulk/0000000000000001")).send().await.unwrap();
        assert_eq!(r.status(), 404, "/bulk/{{id}} is a nonce-capability URL: public, unknown nonce → 404");

        agent.shutdown().await;
    }

    /// Run-61 falsification probe (Security): bearer header *shapes* that a lax parser might
    /// admit — lowercase scheme, double space, trailing space, scheme only, token in a query
    /// string, Basic scheme, and the token as a cookie. None may reach a gated handler: every
    /// one is 401; only the exact `Bearer <token>` is 200.
    #[tokio::test]
    async fn probe_bearer_header_shapes_are_not_accepted() {
        use axum::http::header::{AUTHORIZATION, COOKIE};
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        // Not in the list: a *trailing* space (`"Bearer secret "`) — HTTP parsers strip trailing
        // OWS from field values (RFC 9110 §5.5), so the server sees the exact token; the probe's
        // first run asserted it must be refused and learned that it is admitted *by the spec*, not
        // by a lax comparison. Internal double space is NOT stripped → refused.
        for bad in ["bearer secret", "Bearer  secret", "Bearer", "BEARER secret",
                    "Basic c2VjcmV0", "Token secret", "Bearer secre", "Bearer secrett"] {
            let r = client.get(format!("{base}/gateway/kv/keys")).header(AUTHORIZATION, bad).send().await.unwrap();
            assert_eq!(r.status(), 401, "header {bad:?} must be refused");
        }
        let r = client.get(format!("{base}/gateway/kv/keys?token=secret")).send().await.unwrap();
        assert_eq!(r.status(), 401, "a query-string token must be refused");
        let r = client.get(format!("{base}/gateway/kv/keys")).header(COOKIE, "token=secret").send().await.unwrap();
        assert_eq!(r.status(), 401, "a cookie token must be refused");
        let r = client.get(format!("{base}/gateway/kv/keys")).header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        assert_eq!(r.status(), 200, "the exact form is admitted");
        agent.shutdown().await;
    }

    /// Scoped tokens on the node-level routes (compliance): `/mcp` needs `mcp:invoke`,
    /// `/signals/{kind}` needs `mesh:read`; a token outside the family is refused 403 naming
    /// the required scope; the wildcard admits everything.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn node_level_routes_honour_scoped_tokens() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "mcp-tok".into(),  scopes: vec!["mcp:invoke".into()] },
            crate::GatewayToken { token: "mesh-tok".into(), scopes: vec!["mesh:read".into()] },
            crate::GatewayToken { token: "kv-tok".into(),   scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});

        let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer mcp-tok")
            .json(&init).send().await.unwrap();
        assert_eq!(r.status(), 200, "mcp:invoke admits /mcp");
        let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer kv-tok")
            .json(&init).send().await.unwrap();
        assert_eq!(r.status(), 403, "kv:read must not reach /mcp");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["required_scope"], "mcp:invoke");

        let r = client.get(format!("{base}/signals/k")).header(AUTHORIZATION, "Bearer mesh-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 200, "mesh:read admits the SSE stream");
        drop(r);
        let r = client.get(format!("{base}/signals/k")).header(AUTHORIZATION, "Bearer mcp-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "mcp:invoke must not reach /signals");

        agent.shutdown().await;
    }

    /// **Closure plan C1.** Every raw route that takes an RPC kind from the body refuses protected
    /// work, whatever the token holds (the legacy token holds `*`). A counting MCP tool on the target
    /// proves the refusal happens before dispatch: nothing reaches the handler through any of the six.
    /// The plant is a non-protected kind on `rpc/call`, which is not refused.
    #[tokio::test]
    async fn raw_routes_refuse_protected_kinds_and_the_handler_is_never_reached() {
        use axum::http::header::AUTHORIZATION;
        use base64::Engine as _;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());
        cfg.protected_rpc_kinds = vec!["depot.dispatch".into()];
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();

        let reached = Arc::new(AtomicUsize::new(0));
        let r2 = Arc::clone(&reached);
        let _tool = agent.mcp().register_mcp_tool("count", serde_json::json!({}), move |_args| {
            let r = Arc::clone(&r2);
            async move { r.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("ran")) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                                      "params":{"name":"count","arguments":{}}});
        let b64 = base64::engine::general_purpose::STANDARD.encode(call.to_string());
        let target = id.to_string();

        for kind in ["mcp.invoke", "skill.invoke", "llm.invoke", "depot.dispatch"] {
            let routes: Vec<(&str, serde_json::Value)> = vec![
                ("rpc/call", serde_json::json!({"target": target, "method": kind, "payload_b64": b64, "timeout_secs": 1})),
                ("scatter", serde_json::json!({"targets": [target], "method": kind, "payload_b64": b64, "timeout_secs": 1})),
                ("signal/emit", serde_json::json!({"kind": kind, "scope": format!("node:{target}"), "payload_b64": b64})),
                ("mailbox/deliver", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64})),
                ("shard/emit", serde_json::json!({"kind": kind, "ns": "x", "name": "y", "shard_key": "k", "payload_b64": b64})),
                ("overlay/emit_reliable", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64, "timeout_secs": 1})),
            ];
            for (route, body) in routes {
                let r = client.post(format!("{base}/gateway/{route}")).header(AUTHORIZATION, "Bearer secret")
                    .json(&body).send().await.unwrap();
                assert_eq!(r.status(), 403, "{route} must refuse `{kind}`");
                let v: serde_json::Value = r.json().await.unwrap();
                assert_eq!(v["error"], "protected_kind", "{route} names the refusal for `{kind}`: {v}");
            }
        }

        // The plant: an ordinary kind is not refused (nothing serves it, so it times out).
        let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer secret")
            .json(&serde_json::json!({"target": target, "method": "echo", "payload_b64": b64, "timeout_secs": 1}))
            .send().await.unwrap();
        assert_ne!(r.status(), 403, "a non-protected kind still passes the raw route");

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(reached.load(Ordering::SeqCst), 0, "no raw route reached the tool");
        agent.shutdown().await;
    }

    /// **The SSE doors streamed every protected RPC request to a `mesh:read` holder.** Both signal
    /// streams registered a receiver for whatever kind the path named — the same table `rpc/serve`
    /// and the native MCP tools register on, fanned to every receiver — so a `mesh:read` token could
    /// open `/signals/mcp.invoke` and read each tool call's whole frame (`payload_b64`: the caller
    /// envelope, the carried mandate and possession proof, the correlation nonce) while the raw
    /// routes refused to *send* that kind. Observing protected work is refused with the same body
    /// the raw routes use; an ordinary kind still streams. Seen failing first: both doors answered
    /// 200 for `mcp.invoke`.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn sse_doors_refuse_protected_kinds() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.protected_rpc_kinds = vec!["depot.dispatch".into()];
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "mesh-tok".into(), scopes: vec!["mesh:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        for kind in ["mcp.invoke", "skill.invoke", "llm.invoke", "depot.dispatch"] {
            for path in [format!("/signals/{kind}"), format!("/gateway/signal/sse/{kind}")] {
                let r = client.get(format!("{base}{path}")).header(AUTHORIZATION, "Bearer mesh-tok")
                    .send().await.unwrap();
                assert_eq!(r.status(), 403, "{path}: a mesh:read holder cannot observe protected work");
                let v: serde_json::Value = r.json().await.unwrap();
                assert_eq!(v["error"], "protected_kind", "{path} names the refusal: {v}");
                assert_eq!(v["kind"], kind, "{path}: {v}");
            }
        }
        // The plant: an ordinary kind still streams on both doors.
        for path in ["/signals/repair.request", "/gateway/signal/sse/repair.request"] {
            let r = client.get(format!("{base}{path}")).header(AUTHORIZATION, "Bearer mesh-tok")
                .send().await.unwrap();
            assert_eq!(r.status(), 200, "{path}: an ordinary kind streams");
            drop(r);
        }
        agent.shutdown().await;
    }

    /// **Closure plan C1, the scope split.** `mesh:serve` serves and responds, and cannot call; a
    /// `mesh:write` or `mesh:read` token is refused on the serve routes — the one-release
    /// compatibility window from 2.15.0 is closed. A token with neither is refused too.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn mesh_serve_serves_and_responds_but_cannot_call() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "serve-tok".into(),  scopes: vec!["mesh:serve".into()] },
            crate::GatewayToken { token: "legacy-tok".into(), scopes: vec!["mesh:read".into()] },
            crate::GatewayToken { token: "kv-tok".into(),     scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let respond = serde_json::json!({"nonce_hex": "0000000000000001", "sender": id.to_string(), "result_b64": ""});

        let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer serve-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 200, "mesh:serve opens the serve stream");
        drop(r);
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-tok")
            .json(&respond).send().await.unwrap();
        // The scope admits the route; the handler then refuses a nonce this principal was never
        // handed (`rpc_respond_answers_only_a_request_this_principal_was_handed`) — 403 from the
        // handler, not the 403 `required_scope` the scope layer sends.
        assert_eq!(r.status(), 403, "mesh:serve reaches the handler, which refuses an unserved nonce");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "unserved_request", "{v}");
        let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer serve-tok")
            .json(&serde_json::json!({"target": id.to_string(), "method": "echo", "timeout_secs": 1}))
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "mesh:serve cannot call");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["required_scope"], "mesh:write");

        // The compatibility window (one release from 2.15.0) is closed: a token without `mesh:serve`
        // is refused on the serve routes, naming the scope (360 review, 2026-10-02 — the window
        // had outlived its promise by three releases).
        let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer legacy-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "a mesh:read token no longer serves");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["required_scope"], "mesh:serve");
        let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer kv-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "a token with no mesh scope is refused");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["required_scope"], "mesh:serve");
        agent.shutdown().await;
    }

    /// **`mesh:serve` bound to neither a kind nor a request: `rpc/respond` could answer any in-flight
    /// RPC by nonce.** The route took `nonce_hex` and `sender` from the body with nothing binding them
    /// to a request this principal's serve stream was handed, so any `mesh:serve` holder could pre-empt
    /// any call it learned a nonce for (and release its parked admission). Now a request streamed on
    /// `rpc/serve` is recorded against the principal that opened the stream, and `rpc/respond` answers
    /// only a nonce that principal was handed — another principal, or a nonce never streamed, is
    /// refused `403 unserved_request` and nothing is emitted. Seen failing first: the other principal's
    /// reply answered 200 and the caller received it.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn rpc_respond_answers_only_a_request_this_principal_was_handed() {
        use axum::http::header::AUTHORIZATION;
        use futures_util::StreamExt as _;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "serve-1".into(), scopes: vec!["mesh:serve".into()] },
            crate::GatewayToken { token: "serve-2".into(), scopes: vec!["mesh:serve".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // Principal 1 serves `work`.
        let resp = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer serve-1")
            .send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
        tokio::spawn(async move {
            let mut body = resp.bytes_stream();
            let mut buf = String::new();
            let mut seen_tx = Some(seen_tx);
            while let Some(Ok(chunk)) = body.next().await {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                if let Some(line) = buf.lines().find_map(|l| l.strip_prefix("data:"))
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                    && let Some(tx) = seen_tx.take()
                {
                    let _ = tx.send(v);
                }
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        // A member calls `work`; the request is streamed to principal 1.
        let caller = {
            let agent = Arc::clone(&agent);
            let id = id.clone();
            tokio::spawn(async move {
                agent.service().rpc_call(id, "work", b"go".to_vec(), Duration::from_secs(8)).await
            })
        };
        let streamed = tokio::time::timeout(Duration::from_secs(5), seen_rx).await
            .expect("the request was streamed").expect("stream open");
        let nonce_hex = streamed["nonce_hex"].as_str().unwrap().to_string();
        let sender = streamed["sender"].as_str().unwrap().to_string();

        // Principal 2 holds `mesh:serve` too, but was never handed this request.
        let forged = serde_json::json!({"nonce_hex": nonce_hex, "sender": sender, "result_b64": "Zm9yZ2Vk"});
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-2")
            .json(&forged).send().await.unwrap();
        assert_eq!(r.status(), 403, "another principal cannot answer a request it was not handed");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "unserved_request", "{v}");

        // A nonce nobody was handed, from the serving principal itself.
        let unknown = serde_json::json!({"nonce_hex": "00000000000000ff", "sender": sender, "result_b64": ""});
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
            .json(&unknown).send().await.unwrap();
        assert_eq!(r.status(), 403, "a nonce never streamed is refused");

        // The principal that was handed the request answers it, and the caller gets that reply.
        let real = serde_json::json!({"nonce_hex": nonce_hex, "sender": sender, "result_b64": "ZG9uZQ=="});
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
            .json(&real).send().await.unwrap();
        assert_eq!(r.status(), 200, "the serving principal answers");
        let reply = caller.await.unwrap();
        assert_eq!(reply.as_deref().ok(), Some(&b"done"[..]), "the caller received the served reply: {reply:?}");

        // Answered once: the same nonce is not answerable again.
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
            .json(&real).send().await.unwrap();
        assert_eq!(r.status(), 403, "a request is answered once");
        agent.shutdown().await;
    }

    /// Routes merged via `with_http_routes` sit behind the same auth boundary as the
    /// library's own gateway routes: a merged `/gateway/…` path demands the bearer, a merged
    /// path outside `/gateway/` stays public. Pre-fix (2026-09-04) the merged `/gateway/`
    /// route answered 200 with no token — every companion's gateway surface was open.
    #[tokio::test]
    async fn test_merged_app_routes_under_gateway_prefix_require_auth() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());

        let agent = Arc::new(GossipAgent::new(id, cfg));
        async fn ok() -> &'static str { "ok" }
        agent.with_http_routes(
            axum::Router::new()
                .route("/gateway/app/protected", axum::routing::get(ok))
                .route("/app/public", axum::routing::get(ok)),
        );
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        let r = client.get(format!("{base}/gateway/app/protected")).send().await.unwrap();
        assert_eq!(r.status(), 401, "a merged /gateway/ route must demand the bearer");
        let r = client.get(format!("{base}/gateway/app/protected"))
            .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        assert_eq!(r.status(), 200, "the configured token admits the merged /gateway/ route");
        let r = client.get(format!("{base}/app/public")).send().await.unwrap();
        assert_eq!(r.status(), 200, "a merged route outside /gateway/ stays public");

        agent.shutdown().await;
    }

    /// Falsification probe (Run 60): path shapes that a naive prefix guard might let past
    /// the bearer on a merged route — percent-encoded prefix, double slash, trailing slash,
    /// upper-case, dot-segments. None may reach the handler without a bearer: either the
    /// guard fires (401) or the router does not match the shape (404). Never 200.
    #[tokio::test]
    async fn probe_merged_route_guard_survives_path_shapes() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("secret".into());
        let agent = Arc::new(GossipAgent::new(id, cfg));
        async fn ok() -> &'static str { "ok" }
        agent.with_http_routes(
            axum::Router::new()
                .route("/gateway/app/protected", axum::routing::get(ok))
                .route("/gateway/app/{*rest}", axum::routing::get(ok)),
        );
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        for shape in [
            "/gateway/app/protected",
            "/%67ateway/app/protected",
            "/gateway%2Fapp/protected",
            "/gateway//app/protected",
            "/gateway/app/protected/",
            "/GATEWAY/app/protected",
            "/gateway/app/../app/protected",
            "/gateway/app/deep/wild/card",
            "/gateway/app/%2e%2e/protected",
        ] {
            let r = client.get(format!("{base}{shape}")).send().await.unwrap();
            assert_ne!(r.status(), 200, "shape {shape} reached a merged /gateway/ handler without a bearer");
        }
        agent.shutdown().await;
    }

    /// Scoped tokens on merged companion routes (compliance): a route in the companion
    /// scope table is admitted by its family and refused (403) outside it; an unlisted
    /// merged `/gateway/` path is deny-by-default `admin`, which only the wildcard reaches.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_scoped_tokens_on_merged_companion_routes() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "llmro".into(), scopes: vec!["llm:read".into()] },
            crate::GatewayToken { token: "wikiw".into(), scopes: vec!["wiki:write".into()] },
            crate::GatewayToken { token: "artpub".into(), scopes: vec!["artifact:publish".into()] },
            crate::GatewayToken { token: "kvw".into(), scopes: vec!["kv:write".into()] },
            crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(id, cfg));
        async fn ok() -> &'static str { "ok" }
        agent.with_http_routes(
            axum::Router::new()
                .route("/gateway/reason/trace/{run_id}", axum::routing::get(ok))
                .route("/gateway/wiki/ingest", axum::routing::post(ok))
                .route("/gateway/artifacts/publish", axum::routing::post(ok))
                .route("/gateway/wiki/some/future/verb", axum::routing::post(ok)),
        );
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let get = |path: &str, tok: &str| client.get(format!("{base}{path}")).header(AUTHORIZATION, format!("Bearer {tok}")).send();
        let post = |path: &str, tok: &str| client.post(format!("{base}{path}")).header(AUTHORIZATION, format!("Bearer {tok}")).send();

        assert_eq!(get("/gateway/reason/trace/r1", "llmro").await.unwrap().status(), 200, "llm:read reaches a reason read route");
        assert_eq!(post("/gateway/wiki/ingest", "llmro").await.unwrap().status(), 403, "llm:read is refused on wiki:write");
        assert_eq!(post("/gateway/wiki/ingest", "wikiw").await.unwrap().status(), 200, "wiki:write reaches wiki/ingest");
        let r = post("/gateway/wiki/some/future/verb", "wikiw").await.unwrap();
        assert_eq!(r.status(), 403, "an unlisted companion path is deny-by-default");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["required_scope"], "admin");
        assert_eq!(post("/gateway/wiki/some/future/verb", "super").await.unwrap().status(), 200, "wildcard reaches it");
        // A3: the artifact publish door has its own family; kv:write does not open it.
        assert_eq!(post("/gateway/artifacts/publish", "artpub").await.unwrap().status(), 200, "artifact:publish reaches artifacts/publish");
        assert_eq!(post("/gateway/artifacts/publish", "kvw").await.unwrap().status(), 403, "kv:write is not artifact:publish");
        assert_eq!(post("/gateway/artifacts/publish", "wikiw").await.unwrap().status(), 403, "wiki:write is not artifact:publish");

        agent.shutdown().await;
    }

    /// End-to-end: a scoped token is admitted on routes within its grant,
    /// denied (403) on routes outside it, the wildcard token passes scope
    /// gating everywhere, an unknown token is 401, and public routes stay open
    /// with no credentials at all.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_scoped_token_acl_end_to_end() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "ro".into(),    scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
        ];

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // Public route: open, no credentials.
        let r = client.get(format!("{base}/health")).send().await.unwrap();
        assert_eq!(r.status(), 200, "public /health must stay open");

        // No token on a protected route → 401.
        let r = client.get(format!("{base}/gateway/kv/keys")).send().await.unwrap();
        assert_eq!(r.status(), 401, "missing token must be unauthorized");

        // Unknown token → 401.
        let r = client.get(format!("{base}/gateway/kv/keys"))
            .header(AUTHORIZATION, "Bearer bogus").send().await.unwrap();
        assert_eq!(r.status(), 401, "unknown token must be unauthorized");

        // ro token on a kv:read route → admitted (not 401/403).
        let r = client.get(format!("{base}/gateway/kv/keys"))
            .header(AUTHORIZATION, "Bearer ro").send().await.unwrap();
        assert_eq!(r.status(), 200, "kv:read token must reach kv/keys");

        // ro token on a kv:write route → 403 (authenticated, insufficient scope).
        let r = client.post(format!("{base}/gateway/kv"))
            .header(AUTHORIZATION, "Bearer ro")
            .json(&serde_json::json!({"key": "k", "value": "v"}))
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "kv:read token must be forbidden on kv:write");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["required_scope"], "kv:write");

        // super (wildcard) token on the same write route → passes scope gating.
        let r = client.post(format!("{base}/gateway/kv"))
            .header(AUTHORIZATION, "Bearer super")
            .json(&serde_json::json!({"key": "k", "value": "v"}))
            .send().await.unwrap();
        assert_ne!(r.status(), 401, "wildcard token must authenticate");
        assert_ne!(r.status(), 403, "wildcard token must pass scope gating");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn regression_gateway_rejects_hostile_inputs_without_crashing() {
        // Two untrusted-input fixes on the (default loopback-open) gateway (audit 2026-07-15 pass 2):
        //   1. gw_kv_quorum: an out-of-range `timeout_secs` f64 fed `Duration::from_secs_f64`, which
        //      PANICS → node-abort under the release profile's panic="abort". Must be a clean 400.
        //   2. gw_signal_emit: an unknown `scope` string silently widened to a cluster-wide broadcast.
        //      Must be a 400, not a silent whole-cluster emit.
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // 1. Hostile timeout_secs (finite, JSON-serialisable — NaN/Inf aren't valid JSON so can't
        //    reach the handler) → clean 400, and the node stays up to serve the next request.
        for bad in [-1.0f64, -0.5, 1e300] {
            let r = client.post(format!("{base}/gateway/kv/quorum"))
                .json(&serde_json::json!({"key":"k","min_acks":1,"timeout_secs":bad}))
                .send().await.unwrap();
            assert_eq!(r.status(), 400, "hostile timeout_secs={bad} must be a clean 400, not a crash");
        }
        // A valid timeout still reaches the handler (min_acks=0 fast path → 200), proving the node
        // survived the hostile requests above.
        let r = client.post(format!("{base}/gateway/kv/quorum"))
            .json(&serde_json::json!({"key":"k","min_acks":0,"timeout_secs":1.0}))
            .send().await.unwrap();
        assert_eq!(r.status(), 200, "node must survive and still serve a valid quorum request");

        // 2. Unknown scope → 400; a valid scope is accepted (not 400).
        let r = client.post(format!("{base}/gateway/signal/emit"))
            .json(&serde_json::json!({"kind":"k","scope":"grp:typo","payload_b64":""}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400, "unknown scope must be rejected, not widened to cluster-wide");
        let r = client.post(format!("{base}/gateway/signal/emit"))
            .json(&serde_json::json!({"kind":"k","scope":"cluster","payload_b64":""}))
            .send().await.unwrap();
        assert_ne!(r.status(), 400, "a valid 'cluster' scope must be accepted");

        agent.shutdown().await;
    }

    /// WS-C governance surface (Track 3): an operator publishes tuning + membership
    /// intents over HTTP; the writes land in the gossip KV as evaporating soft-state,
    /// malformed bodies are rejected, and the effective-state snapshot is served.
    /// Open gateway here — scope gating is covered by the next test.
    #[tokio::test]
    async fn test_gateway_govern_publish_and_snapshot() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // Publish a tuning intent → 200, lands at sys/govern/fleet.
        let r = client.post(format!("{base}/gateway/govern/tuning"))
            .json(&serde_json::json!({
                "enabled": true,
                "params": [{"param": "writer_depth", "floor": 1024, "ceiling": 8192, "ratchet": "up"}]
            }))
            .send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(body["key"], "sys/govern/fleet");

        // Unknown param → 400.
        let r = client.post(format!("{base}/gateway/govern/tuning"))
            .json(&serde_json::json!({"params": [{"param": "nope"}]}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400, "unknown param must be rejected");

        // Empty intent (neither enabled nor params) → 400.
        let r = client.post(format!("{base}/gateway/govern/tuning"))
            .json(&serde_json::json!({}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400, "empty intent must be rejected");

        // Publish a membership intent → 200, lands at sys/govern/membership/workers.
        let r = client.post(format!("{base}/gateway/govern/membership"))
            .json(&serde_json::json!({"group": "workers", "min": 3, "max": 10}))
            .send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(body["key"], "sys/govern/membership/workers");

        // Missing group → 400.
        let r = client.post(format!("{base}/gateway/govern/membership"))
            .json(&serde_json::json!({"min": 1}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400, "membership intent without group must be rejected");

        // Both intents are now in the gossip KV (evaporating soft-state).
        assert!(agent.kv().get("sys/govern/fleet").is_some(), "tuning intent must be in KV");
        assert!(agent.kv().get("sys/govern/membership/workers").is_some(), "membership intent must be in KV");

        // Effective-state snapshot is served and well-formed.
        let r = client.get(format!("{base}/gateway/govern")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let snap: serde_json::Value = r.json().await.unwrap();
        assert!(snap["auto_enabled"].is_boolean());
        assert_eq!(snap["params"].as_array().unwrap().len(), 3);

        agent.shutdown().await;
    }

    /// Item 4 §7 over the gateway: `GET /gateway/govern` shows the node's profile and its
    /// tripwires; `POST /gateway/govern/profile` steps the ladder, and the step reaches the tuning
    /// governor's own copy (the fan-out) — read back through the same GET; an unknown name is
    /// `400` and changes nothing.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn test_gateway_control_profile_round_trip() {
        use axum::http::header::AUTHORIZATION;
        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("t".into());
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        let snap: serde_json::Value = client.get(format!("{base}/gateway/govern"))
            .header(AUTHORIZATION, "Bearer t").send().await.unwrap().json().await.unwrap();
        assert_eq!(snap["control"]["profile"], "legacy", "the default: {snap}");
        assert_eq!(snap["control"]["tuning"]["profile"], "legacy");
        assert_eq!(snap["control"]["would_hold"], 0);

        let r = client.post(format!("{base}/gateway/govern/profile"))
            .header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"profile": "observe"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!((body["profile"].as_str(), body["was"].as_str()), (Some("observe"), Some("legacy")));
        assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "the agent's reading");
        assert_eq!(agent.tuning_governor().profile, crate::control::Profile::Observe, "and the fan-out");
        let snap: serde_json::Value = client.get(format!("{base}/gateway/govern"))
            .header(AUTHORIZATION, "Bearer t").send().await.unwrap().json().await.unwrap();
        assert_eq!(snap["control"]["profile"], "observe");
        assert_eq!(snap["control"]["tuning"]["profile"], "observe");

        let r = client.post(format!("{base}/gateway/govern/profile"))
            .header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"profile": "enforce_local"})).send().await.unwrap();
        assert_eq!(r.status(), 400, "a misspelt name is refused");
        assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "and changes nothing");
        // Like every governance route: a non-object body and an unknown field are refused, not ignored — a
        // `target` here used to be dropped and the step applied to this node.
        for bad in [serde_json::json!(["observe"]), serde_json::json!({"profile": "legacy", "target": "127.0.0.1:1"})] {
            let r = client.post(format!("{base}/gateway/govern/profile"))
                .header(AUTHORIZATION, "Bearer t").json(&bad).send().await.unwrap();
            assert_eq!(r.status(), 400, "{bad}");
        }
        assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "and changes nothing");

        agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    }

    /// Every governance write route — enumerated from this file's router, so a new one is covered the day it
    /// lands — records one audit attempt per accepted change (#549's review: `govern/profile` recorded none).
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn every_governance_write_route_audits_its_change() {
        use axum::http::header::AUTHORIZATION;
        // Every `"/govern/…"` route literal in the production half of this file, whatever its layout (a chained
        // `.route(`, a feature-gated `let gateway = gateway.route(`, a call split across lines); `/govern` alone is
        // the read route. Scope-map and doc lines carry `/gateway/govern/`, not a bare `"/govern/`.
        let src = include_str!("http.rs");
        let production = &src[..src.find("#[cfg(test)]\nmod tests").expect("the test module")];
        let routed: std::collections::BTreeSet<String> = production.split("\"/govern/").skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(|r| format!("/gateway/govern/{r}"))
            .collect();
        let bodies: Vec<(&str, serde_json::Value)> = vec![
            ("/gateway/govern/tuning", serde_json::json!({"enabled": true})),
            ("/gateway/govern/timing", serde_json::json!({"health_check_interval_secs": 5})),
            ("/gateway/govern/membership", serde_json::json!({"group": "workers", "min": 1})),
            ("/gateway/govern/topology-override", serde_json::json!({"group": "workers", "override": true})),
            ("/gateway/govern/profile", serde_json::json!({"profile": "observe"})),
            ("/gateway/govern/group", serde_json::json!({"group": "workers"})),
        ];
        let listed: std::collections::BTreeSet<String> = bodies.iter().map(|(r, _)| r.to_string()).collect();
        assert_eq!(routed, listed, "a governance write route without a case here");

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("t".into());
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);
        let unaudited = || agent.task_ctx.governance_unaudited.load(std::sync::atomic::Ordering::Relaxed);
        for (route, body) in bodies {
            let url = format!("http://127.0.0.1:{http_port}{route}");
            // A refused body changes nothing and is not counted — validation comes before the audit.
            let before = changes();
            let r = client.post(&url).header(AUTHORIZATION, "Bearer t")
                .json(&serde_json::json!({"no_such_field": 1})).send().await.unwrap();
            assert_eq!(r.status(), 400, "{route}");
            assert_eq!(changes(), before, "{route} counted a refused body");
            let r = client.post(&url).header(AUTHORIZATION, "Bearer t").json(&body).send().await.unwrap();
            assert_eq!(r.status(), 200, "{route}: {}", r.text().await.unwrap_or_default());
            assert_eq!(changes(), before + 1, "{route} recorded no audit attempt");
        }
        // No `[tls]` identity here, so nothing could be sealed (and without `compliance` nothing is tried):
        // every accepted change is counted as unaudited.
        assert_eq!(unaudited(), changes());
        agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    }

    /// A governed group's members decide its elections, so changing them is governance: `POST`/`DELETE
    /// /gateway/mesh/group` (`mesh:write`) refuses a group under a membership intent **403** `governed_group`, naming
    /// `/gateway/govern/group` (`govern:write`), which moves the node in or out; every membership change through either
    /// route is audited or counted. A plain group stays a `mesh:write` join.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_governed_groups_membership_moves_only_through_a_governance_route() {
        use axum::http::header::AUTHORIZATION;
        let (gossip_port, http_port) = (alloc_port(), alloc_port());
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("t".into());
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}/gateway");
        let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);

        let before = changes();
        let r = client.post(format!("{base}/mesh/group")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"group": "plain"})).send().await.unwrap();
        assert_eq!(r.status(), 200, "a plain group is a mesh:write join");
        assert_eq!(changes(), before, "a plain group's membership is data plane, not a governance change");

        let r = client.post(format!("{base}/govern/membership")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"group": "ruled", "min": 1})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let r = client.post(format!("{base}/mesh/group")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
        assert_eq!(r.status(), 403, "a governed group refuses the data-plane join");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["error"], "governed_group");
        assert!(body["message"].as_str().unwrap().contains("/gateway/govern/group"), "{body}");
        let r = client.delete(format!("{base}/mesh/group?group=ruled")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
        assert_eq!(r.status(), 403, "and the data-plane leave");

        let before = changes();
        let r = client.post(format!("{base}/govern/group")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["governed"], true, "{body}");
        assert!(body["members"].as_array().unwrap().iter().any(|m| m == &serde_json::json!(agent.node_id().to_string())), "{body}");
        assert_eq!(changes(), before + 1);
        // Joining again changes nothing, so nothing is recorded.
        let r = client.post(format!("{base}/govern/group")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(changes(), before + 1, "a no-op join is not a change");

        // The governed leave: as strict as the join — a `target` is refused, not dropped and applied here.
        for bad in ["group=ruled&target=10.0.0.5:9000", "group=", "group=a/b", ""] {
            let r = client.delete(format!("{base}/govern/group?{bad}")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
            assert_eq!(r.status(), 400, "{bad}");
        }
        let r = client.delete(format!("{base}/govern/group?group=ruled")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(changes(), before + 2, "the leave is a governance change");

        // A governed group is not redefined through units/declare (`cap:write`): its filter decides who the governor
        // can elect into it (#572's review).
        let unit = "[[group]]\nname = \"ruled\"\n  [group.filter]\n  ns   = \"depot\"\n  name = \"intake\"\n";
        let r = client.post(format!("{base}/units/declare")).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({ "toml": unit })).send().await.unwrap();
        assert_eq!(r.status(), 403);
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "governed_group");
        agent.shutdown_with_timeout(Duration::from_secs(5)).await;
    }

    /// With `compliance` and a `[tls]` identity every accepted governance change is sealed into this node's audit
    /// stream — the profile step and an identity revocation included — and none is counted unaudited.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn governance_changes_are_sealed_with_an_identity() {
        use crate::config::TlsConfig;
        use axum::http::header::AUTHORIZATION;
        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let cert_dir = std::env::temp_dir().join(format!("myc-govern-audit-{gossip_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        cfg.gateway_scoped_tokens = vec![crate::GatewayToken {
            token: "gov".into(), scopes: vec!["govern:write".into(), "identity:write".into()],
        }];
        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let old_key = agent.identity_public_key().expect("tls identity");
        agent.rotate_identity(Duration::from_millis(0)).await.expect("rotate");
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}/gateway");
        for (route, body) in [
            ("govern/profile", serde_json::json!({"profile": "observe"})),
            ("govern/topology-override", serde_json::json!({"group": "workers", "override": true})),
            ("identity/revoke", serde_json::json!({"revoked_key": super::hex32(&old_key)})),
        ] {
            let r = client.post(format!("{base}/{route}")).header(AUTHORIZATION, "Bearer gov").json(&body).send().await.unwrap();
            assert_eq!(r.status(), 200, "{route}: {}", r.text().await.unwrap_or_default());
        }
        let sealed: Vec<String> = agent.audit_stream(&id).into_iter()
            .filter(|r| r.record.principal == "gateway/govern").map(|r| r.record.target).collect();
        for target in ["govern/profile", "sys/topology-override/workers", "identity/revoke"] {
            assert!(sealed.iter().any(|t| t == target), "{target} not sealed: {sealed:?}");
        }
        assert_eq!(agent.task_ctx.governance_unaudited.load(std::sync::atomic::Ordering::Relaxed), 0);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// WS-C governance scope gating (Track 3, compliance): `govern:read` reaches the
    /// snapshot but is forbidden on a publish route; `govern:write` reaches publish.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_govern_scope_gating() {
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "gov-ro".into(), scopes: vec!["govern:read".into()] },
            crate::GatewayToken { token: "gov-rw".into(), scopes: vec!["govern:write".into()] },
        ];

        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // govern:read reaches the snapshot.
        let r = client.get(format!("{base}/gateway/govern"))
            .header(AUTHORIZATION, "Bearer gov-ro").send().await.unwrap();
        assert_eq!(r.status(), 200, "govern:read must reach the snapshot");

        // govern:read is forbidden on a publish route.
        let r = client.post(format!("{base}/gateway/govern/tuning"))
            .header(AUTHORIZATION, "Bearer gov-ro")
            .json(&serde_json::json!({"enabled": false}))
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "govern:read must be forbidden on publish");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["required_scope"], "govern:write");

        // govern:write reaches the publish route.
        let r = client.post(format!("{base}/gateway/govern/tuning"))
            .header(AUTHORIZATION, "Bearer gov-rw")
            .json(&serde_json::json!({"enabled": false}))
            .send().await.unwrap();
        assert_eq!(r.status(), 200, "govern:write must reach publish");

        // No token → 401.
        let r = client.get(format!("{base}/gateway/govern")).send().await.unwrap();
        assert_eq!(r.status(), 401, "missing token must be unauthorized");

        agent.shutdown().await;
    }

    /// Legible Emergence Phase 2: the `/gateway/fleet` snapshot endpoint is live and
    /// scope-gated (`fleet:read`, deny-by-default) — a `fleet:read` token reads it and
    /// gets the relational snapshot shape; a wrong-scope token is 403; no token is 401.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_fleet_snapshot_endpoint_scope_gated() {
        use axum::http::header::AUTHORIZATION;
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.emergent_detectors_enabled = true;
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "fleet-ro".into(), scopes: vec!["fleet:read".into()] },
            crate::GatewayToken { token: "kv-ro".into(),    scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        agent.mesh().join_group("workers"); // ungoverned: no membership intent anywhere
        tokio::time::sleep(Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // No token → 401.
        let r = client.get(format!("{base}/gateway/fleet")).send().await.unwrap();
        assert_eq!(r.status(), 401, "missing token unauthorized");
        // Wrong scope → 403 naming fleet:read.
        let r = client.get(format!("{base}/gateway/fleet"))
            .header(AUTHORIZATION, "Bearer kv-ro").send().await.unwrap();
        assert_eq!(r.status(), 403, "kv:read token forbidden on fleet:read");
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["required_scope"], "fleet:read");
        // fleet:read token → 200 with the relational snapshot shape.
        let r = client.get(format!("{base}/gateway/fleet"))
            .header(AUTHORIZATION, "Bearer fleet-ro").send().await.unwrap();
        assert_eq!(r.status(), 200, "fleet:read token admitted");
        let body: serde_json::Value = r.json().await.unwrap();
        assert!(body["view_confidence"]["observer"].is_string(), "snapshot carries the RT1 view_confidence header");
        assert!(body["governed_groups"].is_array());
        // #168: every group's observed size, governed or not — an ungoverned group was invisible here.
        assert_eq!(body["group_sizes"], serde_json::json!([{"group": "workers", "observed": 1}]), "{body}");
        assert!(body["throttle_graph"].is_array());
        assert!(body["store_hash"].is_number());
        agent.shutdown().await;
    }

    /// WS4: an OIDC JWT from a (mock) IdP is validated at the gateway and its
    /// groups are mapped to scopes — a `readers`-group token reaches a `kv:read`
    /// route but is forbidden on a `kv:write` route; no token is 401.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_oidc_jwt_maps_groups_to_scopes() {
        use axum::{routing::get, Router};
        use axum::http::header::AUTHORIZATION;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use std::time::{SystemTime, UNIX_EPOCH};

        const TEST_PRIV: &str = include_str!("../../tests/fixtures/oidc_test.key");
        let jwks_body = include_str!("../../tests/fixtures/oidc_jwks.json");

        // ── Mock IdP: discovery + JWKS ───────────────────────────────────────
        let idp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let idp_port = idp_listener.local_addr().unwrap().port();
        let issuer = format!("http://127.0.0.1:{idp_port}");
        let disco = serde_json::json!({
            "issuer": issuer,
            "jwks_uri": format!("{issuer}/jwks"),
        }).to_string();
        let idp = Router::new()
            .route("/.well-known/openid-configuration", get(move || {
                let disco = disco.clone();
                async move { ([("content-type", "application/json")], disco) }
            }))
            .route("/jwks", get(move || {
                async move { ([("content-type", "application/json")], jwks_body) }
            }));
        let _idp = tokio::spawn(async move { axum::serve(idp_listener, idp).await.unwrap(); });

        // ── Mycelium node with OIDC configured ───────────────────────────────
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let mut group_scopes = std::collections::HashMap::new();
        group_scopes.insert("readers".to_string(), vec!["kv:read".to_string()]);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.oidc = Some(crate::OidcConfig {
            issuer: issuer.clone(),
            audience: "mycelium-cluster".into(),
            group_claim: "groups".into(),
            group_scopes,
            jwks_uri: None, // exercise discovery
        });
        let agent = Arc::new(GossipAgent::new(id, cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // ── Mint a JWT for a "readers" user ──────────────────────────────────
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = serde_json::json!({
            "sub": "alice", "iss": issuer, "aud": "mycelium-cluster",
            "exp": now + 3600, "groups": ["readers"],
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        let jwt = encode(&header, &claims, &EncodingKey::from_rsa_pem(TEST_PRIV.as_bytes()).unwrap()).unwrap();

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");

        // No token → 401.
        let r = client.get(format!("{base}/gateway/kv/keys")).send().await.unwrap();
        assert_eq!(r.status(), 401, "no token must be unauthorized");

        // OIDC JWT with kv:read → admitted on the kv:read route.
        let r = client.get(format!("{base}/gateway/kv/keys"))
            .header(AUTHORIZATION, format!("Bearer {jwt}")).send().await.unwrap();
        assert_eq!(r.status(), 200, "readers JWT must reach kv/keys (kv:read)");

        // Same JWT on a kv:write route → 403 (group grants only kv:read).
        let r = client.post(format!("{base}/gateway/kv"))
            .header(AUTHORIZATION, format!("Bearer {jwt}"))
            .json(&serde_json::json!({"key":"k","value":"v"}))
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "readers JWT must be forbidden on kv:write");

        agent.shutdown().await;
    }

    /// WS2: the `/gateway/audit` endpoint returns the node's verified audit
    /// stream to a token holding `audit:read`, and 403s a token without it.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_audit_endpoint_verifies_and_scope_gates() {
        use crate::config::TlsConfig;
        use crate::{AuditAction, AuditOutcome, GatewayToken};
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let cert_dir = std::env::temp_dir().join(format!("myc-audit-ep-{gossip_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        cfg.gateway_scoped_tokens = vec![
            GatewayToken { token: "auditor".into(), scopes: vec!["audit:read".into()] },
            GatewayToken { token: "noaudit".into(), scopes: vec!["kv:read".into()] },
        ];

        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Seal two events into this node's stream.
        agent.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
        agent.audit(AuditAction::Read, "10.0.0.2:9000", "kv/secret", AuditOutcome::Denied, None).unwrap();

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{http_port}/gateway/audit");

        // Wrong scope → 403.
        let r = client.get(&url).header(AUTHORIZATION, "Bearer noaudit").send().await.unwrap();
        assert_eq!(r.status(), 403, "kv:read token must not reach the audit trail");

        // Correct scope → 200, verified stream with both records.
        let r = client.get(&url).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
        assert_eq!(r.status(), 200, "audit:read token must reach the audit trail");
        let body: serde_json::Value = r.json().await.unwrap();
        let streams = body["streams"].as_array().expect("streams array");
        let mine = streams.iter()
            .find(|s| s["node"] == id.to_string())
            .expect("this node's stream present");
        assert_eq!(mine["verified"], true, "honest stream must verify");
        assert!(mine["count"].as_u64().unwrap() >= 2, "both sealed records counted");
        assert!(mine["head_hash"].is_string(), "chain tip hash present");
        let recs = mine["records"].as_array().unwrap();
        assert!(recs.iter().all(|r| r["content_hash"].is_string()),
            "every record carries a citable content_hash");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// WS-D / D2 gate (G-D2): the `/gateway/transparency` endpoint serves a Merkle inclusion proof
    /// that a fetcher verifies *locally* with the public [`verify_inclusion`], and the endpoint is
    /// scope-gated.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn test_gateway_transparency_inclusion_proof_verifies_and_scope_gates() {
        use crate::config::TlsConfig;
        use crate::{verify_inclusion, GatewayToken, ProofStep};
        use axum::http::header::AUTHORIZATION;

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
        let cert_dir = std::env::temp_dir().join(format!("myc-transp-ep-{gossip_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);

        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        cfg.gateway_scoped_tokens = vec![
            GatewayToken { token: "auditor".into(), scopes: vec!["transparency:read".into()] },
            GatewayToken { token: "noaudit".into(), scopes: vec!["kv:read".into()] },
        ];

        let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Rotate so there is an old key to revoke, then revoke it (two revocations would build a
        // taller tree; one is enough to prove the inclusion path round-trips).
        let old_key = agent.identity_public_key().unwrap();
        agent.rotate_identity(Duration::from_millis(200)).await.unwrap();
        agent.revoke_identity_key(old_key).unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}/gateway/transparency");
        let key_hex: String = old_key.iter().map(|b| format!("{b:02x}")).collect();

        // Wrong scope → 403.
        let r = client.get(&base).header(AUTHORIZATION, "Bearer noaudit").send().await.unwrap();
        assert_eq!(r.status(), 403, "kv:read token must not reach the transparency log");

        // Head: this node has a non-empty revocation root.
        let r = client.get(&base).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
        assert_eq!(r.status(), 200);
        let head: serde_json::Value = r.json().await.unwrap();
        let mine = head["nodes"].as_array().unwrap().iter()
            .find(|n| n["node"] == id.to_string()).expect("this node's head");
        assert!(mine["count"].as_u64().unwrap() >= 1, "the revocation is in the log");

        // Inclusion proof for the revoked key — verify it locally against the root.
        let url = format!("{base}?node={id}&key={key_hex}");
        let r = client.get(&url).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
        let proof_doc: serde_json::Value = r.json().await.unwrap();
        assert_eq!(proof_doc["included"], true, "the revoked key is included");
        let hex32 = |s: &str| { let mut o = [0u8; 32];
            for i in 0..32 { o[i] = u8::from_str_radix(&s[i*2..i*2+2], 16).unwrap(); } o };
        let leaf = hex32(proof_doc["leaf"].as_str().unwrap());
        let root = hex32(proof_doc["root"].as_str().unwrap());
        let proof: Vec<ProofStep> = proof_doc["proof"].as_array().unwrap().iter().map(|s| ProofStep {
            sibling:  hex32(s["sibling"].as_str().unwrap()),
            on_right: s["on_right"].as_bool().unwrap(),
        }).collect();
        assert!(verify_inclusion(&leaf, &proof, &root), "the served proof verifies locally");

        // A tampered root must NOT verify.
        let mut bad_root = root;
        bad_root[0] ^= 0xff;
        assert!(!verify_inclusion(&leaf, &proof, &bad_root), "a tampered root is rejected");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }
}

/// Item 7 gates (`docs/plans/v3-contracts-axis.md` §6.4, D27): the four negative cases the
/// secure profile must refuse, the `authorized_callers` gate, and the `/a2a` principal.
#[cfg(test)]
mod gateway_caller_tests {
    use crate::{GossipAgent, GossipConfig, NodeId, RequestPrincipal};
    use std::{sync::{Arc, Mutex}, time::Duration};

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    async fn poll_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if cond() { return true; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cond()
    }

    fn node(http_port: Option<u16>, boot: Vec<NodeId>, tweak: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
        let gossip_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = http_port;
        cfg.bootstrap_peers = boot;
        cfg.reconnect_backoff_secs = 1;
        tweak(&mut cfg);
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg))
    }

    /// A tool that records who called it, registered through the principal-aware API.
    fn observing_tool(agent: &GossipAgent) -> (crate::McpToolHandle, Arc<Mutex<Option<RequestPrincipal>>>) {
        let seen: Arc<Mutex<Option<RequestPrincipal>>> = Arc::new(Mutex::new(None));
        let seen2 = Arc::clone(&seen);
        let handle = agent.mcp().register_mcp_tool_with_principal(
            "whoami",
            serde_json::json!({"type": "object", "properties": {}}),
            move |who, _args| {
                let seen = Arc::clone(&seen2);
                async move {
                    let name = who.name();
                    *seen.lock().unwrap() = Some(who);
                    Ok(serde_json::json!(name))
                }
            },
        );
        (handle, seen)
    }

    async fn tools_call(http_port: u16, bearer: Option<&str>, params: serde_json::Value) -> serde_json::Value {
        let mut req = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}));
        if let Some(b) = bearer {
            req = req.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}"));
        }
        let resp = req.send().await.expect("tools/call request");
        assert_eq!(resp.status(), 200, "tools/call is answered as JSON-RPC");
        resp.json().await.unwrap()
    }

    /// Negative case 1 — a **client-supplied (forged) caller context is not evidence**: whatever
    /// the client puts in `params` (`_meta`, a `caller` block), the provider sees the principal
    /// the auth layer resolved. On an open gateway that is `anonymous` — and it is a *client*
    /// principal, never the node (negative case 2: no fallback to the node's identity).
    #[tokio::test]
    async fn forged_client_context_is_ignored_and_the_node_is_never_the_principal() {
        let http_port = alloc_port();
        let agent = node(Some(http_port), vec![], |_| {});
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_tool, seen) = observing_tool(&agent);

        let body = tools_call(http_port, None, serde_json::json!({
            "name": "whoami",
            "arguments": {},
            "_meta": {"mycelium_caller": {"principal": "oidc:admin", "scopes": ["*"]}},
            "caller": {"principal": "oidc:admin"},
        })).await;
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(crate::PRINCIPAL_ANONYMOUS), "the resolved principal, not the forged one: {text}");

        let who = seen.lock().unwrap().clone().expect("the tool ran");
        match who {
            RequestPrincipal::Client(c) => {
                assert_eq!(c.principal, crate::PRINCIPAL_ANONYMOUS);
                assert_eq!(&c.via, agent.node_id(), "the gateway node is bound in as `via`");
                #[cfg(feature = "compliance")]
                assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "open gateway grants what the route needs");
                #[cfg(not(feature = "compliance"))]
                assert_eq!(c.scopes, vec!["*".to_string()], "no scope model to intersect with");
            }
            RequestPrincipal::Node(n) => panic!("a gateway call must never be attributed to the node ({n})"),
        }

        // The node's own action (a direct in-mesh rpc_call) is still the node.
        let reply = agent.service().rpc_call(
            agent.node_id().clone(), crate::signal::signal_kind::MCP_INVOKE,
            serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"whoami","arguments":{}}})
                .to_string().into_bytes(),
            Duration::from_secs(5),
        ).await.expect("direct call");
        let v: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains(&agent.node_id().to_string()));
        assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Node(_))));

        agent.shutdown().await;
    }

    /// The legacy bearer resolves to `token:legacy`; the principal is never the credential.
    #[tokio::test]
    async fn bearer_resolves_to_a_principal_never_the_credential() {
        let http_port = alloc_port();
        let agent = node(Some(http_port), vec![], |c| c.gateway_auth_token = Some("s3cret".into()));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_tool, seen) = observing_tool(&agent);

        let body = tools_call(http_port, Some("s3cret"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let who = seen.lock().unwrap().clone().expect("the tool ran");
        let RequestPrincipal::Client(c) = who else { panic!("client principal expected") };
        assert_eq!(c.principal, crate::legacy_token_principal(&agent.node_id().to_string()),
            "qualified by the gateway's identity issuer (its node id by default)");
        assert!(!c.principal.contains("s3cret"));
        agent.shutdown().await;
    }

    /// Negative case 3 — **the gateway never asserts more scope than the credential holds**: a
    /// scoped token holding `mcp:invoke` + `kv:read` yields exactly `["mcp:invoke"]` on
    /// `tools/call` (the intersection with the route), and a `*` token yields `["mcp:invoke"]`,
    /// never `*`.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn granted_scopes_are_the_intersection_never_the_wildcard() {
        let http_port = alloc_port();
        let agent = node(Some(http_port), vec![], |c| {
            c.gateway_scoped_tokens = vec![
                crate::GatewayToken { token: "narrow".into(), scopes: vec!["mcp:invoke".into(), "kv:read".into()] },
                crate::GatewayToken { token: "root".into(),   scopes: vec!["*".into()] },
                crate::GatewayToken { token: "kvonly".into(), scopes: vec!["kv:read".into()] },
            ];
        });
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_tool, seen) = observing_tool(&agent);

        let body = tools_call(http_port, Some("narrow"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
        assert_eq!(c.principal, format!("token:{}/#0", agent.node_id()));
        assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "kv:read is held but not granted here");

        let body = tools_call(http_port, Some("root"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
        assert_eq!(c.principal, format!("token:{}/#1", agent.node_id()));
        assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "`*` is never carried as an authority");

        // A token without the route's scope is refused at the gate (403) — no dispatch at all.
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .header(axum::http::header::AUTHORIZATION, "Bearer kvonly")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"whoami","arguments":{}}}))
            .send().await.unwrap();
        assert_eq!(resp.status(), 403);
        agent.shutdown().await;
    }

    /// Negative case 4 — **an older provider that cannot enforce the context is refused, not
    /// silently run as the node**: with the provider's `sys/caller-context/` marker absent, the
    /// secure gateway answers `-32021`; the explicit `legacy` profile dispatches as before.
    #[tokio::test]
    async fn secure_profile_refuses_a_provider_without_the_marker_and_legacy_does_not() {
        let provider = node(None, vec![], |_| {});
        provider.start().await.unwrap();
        let boot = vec![provider.node_id().clone()];
        let secure_port = alloc_port();
        let legacy_port = alloc_port();
        let secure = node(Some(secure_port), boot.clone(), |_| {});
        let legacy = node(Some(legacy_port), boot, |c| c.gateway_caller_profile = crate::GatewayCallerProfile::Legacy);
        secure.start().await.unwrap();
        legacy.start().await.unwrap();
        let (_tool, seen) = observing_tool(&provider);

        // Structural readiness: both gateways see the provider's tool and its marker.
        let marker = format!("sys/caller-context/{}", provider.node_id());
        let tool_key = format!("tools/whoami/{}", provider.node_id());
        assert!(poll_until(|| [&secure, &legacy].iter().all(|g|
            g.kv().get(&tool_key).is_some() && g.kv().get(&marker).is_some()
        ), Duration::from_secs(15)).await, "gateways learn the provider's tool and marker");

        // Both profiles reach the provider while the marker is present.
        let body = tools_call(secure_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "secure dispatch with marker: {body}");
        assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Client(_))));

        // Simulate a pre-item-7 provider: tombstone its marker (LWW, later HLC wins everywhere).
        assert!(secure.kv().delete(marker.clone()));
        assert!(poll_until(|| secure.kv().get(&marker).is_none() && legacy.kv().get(&marker).is_none(),
                           Duration::from_secs(10)).await, "the tombstone reaches both gateways");

        let body = tools_call(secure_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert_eq!(body["error"]["code"], -32021, "secure profile refuses: {body}");
        assert_eq!(body["error"]["data"]["reason"], "provider_without_caller_context");
        assert!(body["error"]["message"].as_str().unwrap().contains(&provider.node_id().to_string()),
                "the refusal names the provider");

        // The explicit legacy profile still dispatches — as the node, which is what the
        // provider (here: one that would strip nothing) then sees.
        *seen.lock().unwrap() = None;
        let body = tools_call(legacy_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "legacy dispatch: {body}");
        assert_eq!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Node(legacy.node_id().clone())),
                   "legacy = node-as-caller, exactly the pre-item-7 behaviour");

        secure.shutdown().await;
        legacy.shutdown().await;
        provider.shutdown().await;
    }

    /// The `authorized_callers` gate (§6.4): a provider restricting the allowlist to the gateway
    /// **node** rejects a gateway **client**, although the node is listed — and admits the node's
    /// own direct call. Listing the client's principal admits it. A forged envelope from another
    /// node is refused as a caller-context error, never as the node. Under `tls` + `compliance`
    /// the context is Ed25519-attested by the gateway and verified against its identity key.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn authorized_callers_judges_the_client_not_the_gateway_node() {
        use crate::signal::SignalScope;
        use bytes::{BufMut, BytesMut};

        let cert_dir = std::env::temp_dir().join(format!("gwcaller-gate-{}", alloc_port()));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let tls = |c: &mut GossipConfig| {
            c.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        };
        let http_port = alloc_port();
        let gateway = node(Some(http_port), vec![], |c| { tls(c); c.gateway_auth_token = Some("tok".into()); });
        gateway.start().await.unwrap();
        let boot = vec![gateway.node_id().clone()];
        let provider = node(None, boot.clone(), tls);
        let rogue = node(None, boot, tls);
        provider.start().await.unwrap();
        rogue.start().await.unwrap();

        // Structural readiness: the mesh forms and the provider knows the gateway's identity key
        // (harvested from its cert on connect — what the attestation verifies against).
        let gid = gateway.node_id().clone();
        assert!(poll_until(|| provider.peers().len() >= 2 && rogue.peers().len() >= 2
                    && provider.task_ctx.peer_keys.pin().get(&gid).is_some_and(|k| !k.is_empty()),
                Duration::from_secs(30)).await, "mesh forms and the provider anchors the gateway's key");

        // The provider serves `guarded.echo` with a switchable allowlist, judged by
        // `request_authorized` (the caller-context-aware gate). The reply names the outcome.
        let allow: Arc<Mutex<Vec<Arc<str>>>> = Arc::new(Mutex::new(vec![Arc::from(gid.to_string().as_str())]));
        let attested: Arc<Mutex<Option<crate::CallerAttestation>>> = Arc::new(Mutex::new(None));
        {
            let p = Arc::clone(&provider);
            let allow = Arc::clone(&allow);
            let attested = Arc::clone(&attested);
            let mut rx = provider.service().rpc_rx("guarded.echo");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    let list = allow.lock().unwrap().clone();
                    let reply = match p.request_authorized(&req, &list) {
                        Ok(true) => {
                            if let Ok(RequestPrincipal::Client(c)) = p.request_principal(&req) {
                                *attested.lock().unwrap() = Some(c.attestation);
                            }
                            format!("ok:{}", p.request_principal(&req).unwrap().name())
                        }
                        Ok(false) => "denied:authorized_callers".to_string(),
                        Err(e) => format!("denied:caller_context:{e}"),
                    };
                    p.service().rpc_respond(&req, reply.into_bytes());
                }
            });
        }

        let client = reqwest::Client::new();
        let call = |payload: &str| {
            let client = client.clone();
            let body = serde_json::json!({"target": provider.node_id().to_string(), "method": "guarded.echo",
                                          "payload_b64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, payload),
                                          "timeout_secs": 10});
            async move {
                let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call"))
                    .header(axum::http::header::AUTHORIZATION, "Bearer tok")
                    .json(&body).send().await.unwrap();
                let status = r.status();
                let v: serde_json::Value = r.json().await.unwrap();
                let text = v["result_b64"].as_str().map(|b| {
                    String::from_utf8(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).unwrap()).unwrap()
                });
                (status, text, v)
            }
        };

        // 1. Node listed, client not: the gateway client is rejected …
        let (status, text, v) = call("hi").await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(text.as_deref(), Some("denied:authorized_callers"), "the node being listed admits no client");
        // … while the node's own direct call is admitted.
        let reply = gateway.service().rpc_call(provider.node_id().clone(), "guarded.echo", b"hi".to_vec(), Duration::from_secs(10))
            .await.expect("direct reply");
        assert_eq!(String::from_utf8(reply.to_vec()).unwrap(), format!("ok:{gid}"));

        // 2. The client's principal listed: admitted, and the context was signature-attested.
        let legacy = crate::legacy_token_principal(&gid.to_string());
        *allow.lock().unwrap() = vec![Arc::from(legacy.as_str())];
        let (_, text, v) = call("hi").await;
        assert_eq!(text.as_deref(), Some(&*format!("ok:{legacy}")), "{v}");
        assert!(matches!(*attested.lock().unwrap(), Some(crate::CallerAttestation::Signed { .. })),
                "under tls the provider verified the gateway's signature");

        // 3. A forged envelope from another node (unsigned, claiming the token principal):
        // refused as a caller-context error — never judged as the sending node.
        let forged_env = serde_json::json!({"v": 1, "p": legacy, "via": rogue.node_id().to_string(), "s": [], "t": 0}).to_string();
        let mut framed = BytesMut::new();
        framed.put_slice(&[0x00, b'G', b'W', b'C', crate::CALLER_CONTEXT_VERSION]);
        framed.put_u16(forged_env.len() as u16);
        framed.put_slice(forged_env.as_bytes());
        framed.put_slice(b"hi");
        let reply = rogue.service().rpc_call(provider.node_id().clone(), "guarded.echo", framed.freeze(), Duration::from_secs(10))
            .await.expect("forged call still gets a reply");
        let reply = String::from_utf8(reply.to_vec()).unwrap();
        // Refused at the `rpc_rx` boundary itself (review finding 3): the provider loop never
        // sees the request; the boundary answers with the refusal.
        assert!(reply.contains("caller context refused"), "forgery is refused as such: {reply}");
        // Even with an open allowlist a forged context is refused.
        *allow.lock().unwrap() = vec![];
        let reply = rogue.service().rpc_call(provider.node_id().clone(), "guarded.echo",
            { let mut b = BytesMut::new(); b.put_slice(&[0x00, b'G', b'W', b'C', crate::CALLER_CONTEXT_VERSION]);
              b.put_u16(forged_env.len() as u16); b.put_slice(forged_env.as_bytes()); b.put_slice(b"hi"); b.freeze() },
            Duration::from_secs(10)).await.expect("reply");
        assert!(String::from_utf8(reply.to_vec()).unwrap().contains("caller context refused"));
        let _ = SignalScope::Cluster; // keep the import honest under cfg combinations

        rogue.shutdown().await;
        provider.shutdown().await;
        gateway.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// `/a2a` (public by design) still resolves a principal: no bearer ⇒ `anonymous`; a valid
    /// bearer ⇒ its principal; an unrecognised bearer ⇒ 401 (never downgraded to anonymous).
    #[cfg(feature = "a2a")]
    #[tokio::test]
    async fn a2a_resolves_the_caller_principal() {
        let http_port = alloc_port();
        let agent = {
            let gossip_port = alloc_port();
            let mut cfg = GossipConfig::default();
            cfg.bind_port = gossip_port;
            cfg.http_port = Some(http_port);
            cfg.gateway_auth_token = Some("tok".into());
            let a = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a();
            Arc::new(a)
        };
        agent.start().await.unwrap();
        let _reg = agent.capabilities().advertise_capability(
            crate::capability::Capability::new("demo", "whoami"), Duration::from_secs(5));
        {
            let a = Arc::clone(&agent);
            let mut rx = agent.service().rpc_rx("skill.invoke");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    let reply = match a.request_principal(&req) {
                        Ok(p) => p.name(),
                        Err(e) => format!("refused:{e}"),
                    };
                    a.service().rpc_respond(&req, reply.into_bytes());
                }
            });
        }
        let cap_key = format!("cap/{}/demo/whoami", agent.node_id());
        assert!(poll_until(|| agent.kv().get(&cap_key).is_some(), Duration::from_secs(5)).await);

        let client = reqwest::Client::new();
        let send = |bearer: Option<&str>| {
            let client = client.clone();
            let bearer = bearer.map(str::to_owned);
            async move {
                let mut req = client.post(format!("http://127.0.0.1:{http_port}/a2a")).json(&serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                    "params": {"skillId": "demo/whoami", "message": {"role": "user", "parts": [{"type": "text", "text": "?"}]}},
                }));
                if let Some(b) = bearer { req = req.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}")); }
                let r = req.send().await.unwrap();
                let status = r.status();
                let v: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
                (status, v)
            }
        };
        let (status, v) = send(None).await;
        assert_eq!(status, 200);
        assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], crate::PRINCIPAL_ANONYMOUS, "{v}");
        let (status, v) = send(Some("tok")).await;
        assert_eq!(status, 200);
        assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], crate::legacy_token_principal(&agent.node_id().to_string()), "{v}");
        let (status, _) = send(Some("wrong")).await;
        assert_eq!(status, 401, "a presented but unrecognised bearer is refused, not anonymised");

        agent.shutdown().await;
    }

    /// Review finding 1, closed: a raw `/gateway/signal/emit` of RPC-shaped bytes reaches an
    /// `rpc_rx` provider **refused** — the gateway node promises an envelope on every RPC it
    /// originates, so the bare frame is `Missing`, never the node's own action; the gateway's
    /// genuine dispatch on the same route is judged by the client's principal.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn raw_gateway_signal_cannot_pass_as_the_node() {
        use base64::Engine as _;
        let http_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("gwraw-{http_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let a = node(Some(http_port), vec![], |c| {
            c.gateway_auth_token = Some("review-token".into());
            c.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        });
        a.start().await.unwrap();
        // Observe the raw signal below the verifying `rpc_rx` boundary too.
        let mut raw_rx = a.task_ctx.signal_handlers.register(Arc::from("review.guarded"));
        let mut rx = a.service().rpc_rx("review.guarded");
        let target = a.node_id().clone();
        let node_allow: Vec<Arc<str>> = vec![Arc::from(target.to_string().as_str())];
        let client = reqwest::Client::new();

        // The genuine gateway dispatch: the client is not the node.
        let safe = client.post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call"))
            .bearer_auth("review-token")
            .json(&serde_json::json!({"target": target.to_string(), "method": "review.guarded", "payload_b64": "eA==", "timeout_secs": 2}));
        let call = tokio::spawn(async move { safe.send().await });
        let req = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(a.request_principal(&req).unwrap(), RequestPrincipal::Client(_)));
        assert!(!a.request_authorized(&req, &node_allow).unwrap(), "listing the node admits no client");
        a.service().rpc_respond(&req, bytes::Bytes::new());
        let _ = call.await;
        let _ = raw_rx.recv().await; // drain the genuine one

        // The bypass: RPC-shaped bytes through the raw signal route.
        let mut raw = 1234u64.to_le_bytes().to_vec(); raw.extend_from_slice(b"x");
        let resp = client.post(format!("http://127.0.0.1:{http_port}/gateway/signal/emit"))
            .bearer_auth("review-token")
            .json(&serde_json::json!({"kind": "review.guarded", "scope": format!("node:{target}"),
                                      "payload_b64": base64::engine::general_purpose::STANDARD.encode(raw)}))
            .send().await.unwrap();
        assert_eq!(resp.status(), 200, "emitting a plain signal is still allowed");
        // Below the boundary the frame arrives — and is judged Missing, never Node.
        let sig = tokio::time::timeout(Duration::from_secs(3), raw_rx.recv()).await.unwrap().unwrap();
        let req = crate::RpcRequest::from(sig);
        assert_eq!(a.gateway_caller(&req), Err(crate::CallerError::Missing));
        assert!(a.request_authorized(&req, &node_allow).is_err(), "never admitted as the node");
        // The verifying `rpc_rx` never yields it.
        assert!(tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.is_err(),
            "rpc_rx refuses the raw frame instead of yielding it");
        // And the node's own direct call still works, as its own action.
        let caller = Arc::clone(&a);
        let t2 = target.clone();
        let own = tokio::spawn(async move {
            caller.service().rpc_call(t2, "review.guarded", b"x".to_vec(), Duration::from_secs(2)).await
        });
        let req = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert_eq!(a.request_principal(&req).unwrap(), RequestPrincipal::Node(target.clone()));
        assert!(a.request_authorized(&req, &node_allow).unwrap(), "the node's own call is admitted by its id");
        a.service().rpc_respond(&req, bytes::Bytes::new());
        let _ = own.await;
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// Review finding 2, closed at the HTTP level: the same bearer position on two gateways
    /// resolves to two principals; a named token under an explicit shared issuer resolves to one.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn token_identities_are_qualified_by_the_issuing_gateway() {
        let provider = node(None, vec![], |_| {});
        provider.start().await.unwrap();
        let boot = vec![provider.node_id().clone()];
        let p1 = alloc_port(); let p2 = alloc_port();
        let tokens = |c: &mut GossipConfig| {
            c.gateway_scoped_tokens = vec![crate::GatewayToken { token: "pos".into(), scopes: vec!["*".into()] }];
            c.gateway_named_tokens = vec![crate::GatewayNamedToken { name: "ci-bot".into(), token: "named".into(), scopes: vec!["*".into()] }];
        };
        let g1 = node(Some(p1), boot.clone(), |c| { tokens(c); });
        let g2 = node(Some(p2), boot, |c| { tokens(c); c.gateway_identity_issuer = Some("fleet-gw".into()); });
        g1.start().await.unwrap(); g2.start().await.unwrap();
        let (_tool, seen) = observing_tool(&provider);
        let tool_key = format!("tools/whoami/{}", provider.node_id());
        let marker = format!("sys/caller-context/{}", provider.node_id());
        assert!(poll_until(|| [&g1, &g2].iter().all(|g| g.kv().get(&tool_key).is_some() && g.kv().get(&marker).is_some()), Duration::from_secs(15)).await);
        let who = |port: u16, bearer: &'static str, seen: Arc<Mutex<Option<RequestPrincipal>>>| async move {
            let body = tools_call(port, Some(bearer), serde_json::json!({"name": "whoami", "arguments": {}})).await;
            assert!(body.get("error").is_none(), "{body}");
            let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
            c.principal
        };
        let a = who(p1, "pos", Arc::clone(&seen)).await;
        let b = who(p2, "pos", Arc::clone(&seen)).await;
        assert_eq!(a, format!("token:{}/#0", g1.node_id()));
        assert_eq!(b, "token:fleet-gw/#0");
        assert_ne!(a, b, "the same list position on two gateways is two identities");
        let n1 = who(p1, "named", Arc::clone(&seen)).await;
        assert_eq!(n1, format!("token:{}/ci-bot", g1.node_id()));
        g1.shutdown().await; g2.shutdown().await; provider.shutdown().await;
    }

    /// **A named token is a token model.** Found 2026-09-23 while adding the federation verbs'
    /// scope test, which failed with a 504 where a 403 was expected — the scope layer had not run
    /// at all.
    ///
    /// `gateway_auth`'s open-gateway predicate counted `gateway_auth_token` and
    /// `gateway_scoped_tokens` and **not** `gateway_named_tokens`, which 2.10.0 added and
    /// `resolve_token` has honoured since. So a deployment whose only credential model was named
    /// tokens — the model `GossipConfig`'s own documentation says to *prefer* — ran an open
    /// gateway: no bearer required, and `open_gateway_scopes` handing each route exactly the scope
    /// it asks for, which is deny-by-default inverted into admit-by-default.
    ///
    /// It hid because the tokens kept working: presenting one was admitted (`resolve_token` knows
    /// them), and the only way to see the hole was to present **nothing**. The existing identity
    /// test above configures both tables, so it never could.
    ///
    /// Both halves are asserted, because the fix must not simply close the port: no bearer is 401,
    /// and a named token still resolves to its own principal.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn named_tokens_alone_still_close_the_gateway() {
        let g = node(Some(alloc_port()), vec![], |c| {
            c.gateway_named_tokens = vec![crate::GatewayNamedToken {
                name: "ci-bot".into(), token: "named".into(), scopes: vec!["kv:read".into()],
            }];
        });
        g.start().await.unwrap();
        let port = g.config().http_port.unwrap();
        let url = format!("http://127.0.0.1:{port}/gateway/kv/keys");
        let http = reqwest::Client::new();
        for _ in 0..100 {
            if http.get(format!("http://127.0.0.1:{port}/health")).send().await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let anonymous = http.get(&url).send().await.expect("gateway answered");
        assert_eq!(anonymous.status(), 401, "a named-token deployment is not an open gateway");

        let wrong = http.get(&url).header(axum::http::header::AUTHORIZATION, "Bearer nope").send().await.unwrap();
        assert_eq!(wrong.status(), 401, "an unrecognised bearer is refused");

        let named = http.get(&url).header(axum::http::header::AUTHORIZATION, "Bearer named").send().await.unwrap();
        assert_eq!(named.status(), 200, "the named token still admits what its scopes allow");

        // ...and its scopes still bound it: `kv:read` is not `kv:write`.
        let write = http.post(format!("http://127.0.0.1:{port}/gateway/kv"))
            .header(axum::http::header::AUTHORIZATION, "Bearer named")
            .json(&serde_json::json!({"key": "probe/x", "value_b64": ""}))
            .send().await.unwrap();
        assert_eq!(write.status(), 403, "a read token does not write");
        g.shutdown().await;

        // The write's receipt (doc-coverage run 17, code gap 2): a writer sees rung 2, not a bare ok.
        let g = node(Some(alloc_port()), vec![], |c| {
            c.gateway_named_tokens = vec![crate::GatewayNamedToken {
                name: "writer".into(), token: "w".into(), scopes: vec!["kv:write".into()],
            }];
        });
        g.start().await.unwrap();
        let port = g.config().http_port.unwrap();
        for _ in 0..100 {
            if http.get(format!("http://127.0.0.1:{port}/health")).send().await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let write = http.post(format!("http://127.0.0.1:{port}/gateway/kv"))
            .header(axum::http::header::AUTHORIZATION, "Bearer w")
            .json(&serde_json::json!({"key": "probe/y", "value_b64": "aGk="}))
            .send().await.unwrap();
        assert_eq!(write.status(), 200);
        let body: serde_json::Value = write.json().await.unwrap();
        assert_eq!(body["ok"], true);
        assert!(body["operation_id"].as_str().is_some_and(|s| !s.is_empty()), "{body}");
        assert_eq!(body["local_durability"], "not_configured", "no persistence on this node: rung 2 says so — {body}");

        // Run 62: a bearer is matched exactly or not at all. Interior whitespace, case, or a scheme
        // the parser does not strip must never admit — and on a closed gateway "not admitted" is
        // 401, never a downgrade to anonymous 200. (Trailing whitespace is deliberately not a case:
        // HTTP strips trailing OWS from a field value before the server sees it — RFC 9110 §5.5,
        // established by Run 61's probe — so `"Bearer w "` reaches the token table as `"w"`.)
        for (hdr, why) in [
            ("Bearer  w", "double space"),
            ("bearer w", "lowercase scheme"),
            ("Bearer W", "wrong case in the token"),
            ("Bearerw", "no separator"),
        ] {
            let r = http.get(format!("http://127.0.0.1:{port}/gateway/kv/keys"))
                .header(axum::http::header::AUTHORIZATION, hdr)
                .send().await.unwrap();
            assert_eq!(r.status(), 401, "{why}: {hdr:?} must not be admitted");
        }

        g.shutdown().await;
    }
}

/// The AE evaluator seam at the live gateway (`docs/design/action-envelope-ae0.md`): the unit
/// fixtures prove the decision rules; these prove the **wiring** — that a refusal stops the
/// dispatch and a permit does not.
#[cfg(all(test, feature = "tls", feature = "compliance"))]
mod ae_seam_tests {
    use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
    use std::time::Duration;

    fn alloc_port() -> u16 { crate::test_util::alloc_port() }

    async fn tools_call(http_port: u16, name: &str, args: serde_json::Value) -> serde_json::Value {
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/mcp"))
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                      "params": {"name": name, "arguments": args}}))
            .send().await.expect("tools/call");
        assert_eq!(resp.status(), 200, "answered as JSON-RPC");
        resp.json().await.unwrap()
    }

    /// **The other door describes a refusal the same way — over the wire, not in the type.**
    ///
    /// The `/mcp` refusal body has been asserted at this surface since AE-T. `/a2a` never was, and
    /// that is exactly where the two drifted: MCP sent `data.reason` and `data.policy_revision`,
    /// A2A sent a number and a sentence. A unit test on `PreflightRefusal::error_data` pins what
    /// the *type* produces; only this pins what a **client actually receives**, which is where the
    /// gap was.
    ///
    /// `/a2a` is the door a foreign agent and a partner domain come through, so it is the one that
    /// mattered most and was checked least.
    ///
    /// Gated on `a2a` as well as the module's own features: `with_a2a` only exists when the
    /// route does, and a test that cannot mount the door cannot check what comes through it.
    #[cfg(feature = "a2a")]
    #[tokio::test]
    async fn the_a2a_door_sends_the_same_refusal_body_as_mcp() {
        use crate::capability::Capability;

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("ae-a2a-surface-{http_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        // `/a2a` is mounted by `with_a2a`; without it the route 404s and the preflight this test
        // is about never runs.
        let agent = Arc::new(
            GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a(),
        );
        let me = agent.node_id().clone();

        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-a2a-surface")
                .with_catalogue("cat-test", "1")
                .map_action("skill.invoke", format!("skill:depot/dispatch@{me}"))
                .prohibit(Rule::new("*", "skill.invoke", format!("skill:depot/dispatch@{me}"))),
        ));
        agent.start().await.unwrap();

        // The skill must resolve: `/a2a` looks the provider up before the preflight runs, so an
        // unadvertised skill is refused as "not found" and never reaches the evaluator at all.
        let _reg = agent
            .capabilities()
            .advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/a2a"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                "params": {
                    "id": "task-1",
                    "skillId": "depot/dispatch",
                    "message": {"role": "user", "parts": [{"type": "text", "text": "dispatch"}]},
                },
            }))
            .send()
            .await
            .expect("tasks/send");
        assert_eq!(resp.status(), 200, "answered as JSON-RPC");
        let body: serde_json::Value = resp.json().await.unwrap();

        assert_eq!(body["error"]["code"], -32030, "an explicit prohibition denies: {body}");
        assert_eq!(
            body["error"]["data"]["reason"], "action_denied",
            "machine-readable, not prose — the field A2A was missing: {body}",
        );
        assert_eq!(
            body["error"]["data"]["policy_revision"], "rev-a2a-surface",
            "which artifact decided — how a caller tells a stale policy from a denial: {body}",
        );
        assert!(body["error"]["data"]["checked"].is_array(), "{body}");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// **A terminal task state is the last thing a subscriber hears.**
    ///
    /// `tasks/sendSubscribe` spawns a detached task that sleeps 100 ms and then emits `working`,
    /// to show progress while an RPC runs. It was spawned *before* the outcome was known and
    /// nothing cancelled it, so an outcome reached inside 100 ms was overtaken by its own progress
    /// report: an AE refusal resolves in about a millisecond, and the stream read
    /// `submitted → failed → working`.
    ///
    /// That is not cosmetic. A client tracking task state sees a task recover from failing, and a
    /// terminal state it can act on is the whole reason the streaming edge emits one — the same
    /// argument that put the refusal on this path at all rather than dropping the subscription.
    /// The success path had it too (`completed → working` for any dispatch under 100 ms), so it
    /// predates the AE seam and is not about refusals.
    ///
    /// Asserted on the **order of states as received**, not on their presence: `working` before a
    /// terminal state is correct and expected on a slow skill, and a test that merely forbade the
    /// event would forbid the feature.
    ///
    /// Found by `examples/a2a_skill_authority`'s first CI run (2026-09-25), which is what that
    /// example exists for.
    #[cfg(feature = "a2a")]
    #[tokio::test]
    async fn no_event_follows_a_terminal_task_state_on_the_stream() {
        use crate::capability::Capability;

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("a2a-terminal-{http_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(
            GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a(),
        );
        let me = agent.node_id().clone();

        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-a2a-terminal")
                .with_catalogue("cat-test", "1")
                .map_action("skill.invoke", format!("skill:depot/dispatch@{me}"))
                .prohibit(Rule::new("*", "skill.invoke", format!("skill:depot/dispatch@{me}"))),
        ));
        agent.start().await.unwrap();

        // The skill must resolve, or the stream takes the `skill not found` path — which returns
        // *before* the progress task is spawned and so could never have shown this.
        let _reg = agent
            .capabilities()
            .advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/a2a"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/sendSubscribe",
                "params": {
                    "id": "task-terminal",
                    "skillId": "depot/dispatch",
                    "message": {"role": "user", "parts": [{"type": "text", "text": "dispatch"}]},
                },
            }))
            .send()
            .await
            .expect("sendSubscribe");

        // The stream must END once the outcome is known; a hang here is its own failure, and the
        // bound is generous enough that it can only mean the sender was never dropped.
        let body = tokio::time::timeout(Duration::from_secs(10), resp.text())
            .await
            .expect("the stream must end after a terminal state")
            .expect("stream body");

        let states: Vec<String> = body
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<serde_json::Value>(d.trim()).ok())
            .filter_map(|v| v["status"]["state"].as_str().map(str::to_owned))
            .collect();

        assert!(
            states.contains(&"failed".to_string()),
            "a prohibited skill must fail the streamed task: {states:?}\n{body}",
        );
        let terminal = |s: &str| matches!(s, "failed" | "completed" | "canceled");
        let first_terminal = states
            .iter()
            .position(|s| terminal(s))
            .expect("a terminal state");
        assert_eq!(
            first_terminal,
            states.len() - 1,
            "a terminal state must be the LAST state a subscriber receives, and {:?} follows it: \
             a client tracking this task sees it recover from failing.\n{states:?}",
            &states[first_terminal + 1..],
        );

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// An attached evaluator refuses at the gateway: the provider's tool never runs, the client
    /// gets the decision, and a permitted call on the same node still works.
    #[tokio::test]
    async fn the_gateway_preflight_refuses_before_the_tool_runs() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("ae-seam-{http_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));

        // `anonymous` may call `allowed`; `forbidden` is prohibited outright; `uncovered` is in
        // the catalogue but named by no allowance.
        let me = agent.node_id().clone();
        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-1")
                .with_catalogue("cat-test", "1")
                .map_action("tools/call", format!("tool:allowed@{me}"))
                .map_action("tools/call", format!("tool:forbidden@{me}"))
                .map_action("tools/call", format!("tool:uncovered@{me}"))
                .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", format!("tool:allowed@{me}")))
                .prohibit(Rule::new("*", "tools/call", format!("tool:forbidden@{me}"))),
        ));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let ran = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for tool in ["allowed", "forbidden", "uncovered"] {
            let ran = Arc::clone(&ran);
            handles.push(agent.mcp().register_mcp_tool(
                tool,
                serde_json::json!({"type": "object", "properties": {}}),
                move |_args| {
                    let ran = Arc::clone(&ran);
                    async move { ran.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("ran")) }
                },
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Permitted: dispatched, the tool runs.
        let body = tools_call(http_port, "allowed", serde_json::json!({})).await;
        assert!(body.get("error").is_none(), "permitted call: {body}");
        assert_eq!(ran.load(Ordering::SeqCst), 1);

        // Prohibited: refused at the gateway with the decision; the tool does not run.
        let body = tools_call(http_port, "forbidden", serde_json::json!({})).await;
        assert_eq!(body["error"]["code"], -32030, "explicit prohibition denies: {body}");
        assert_eq!(body["error"]["data"]["reason"], "action_denied");
        assert_eq!(body["error"]["data"]["policy_revision"], "rev-1");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the prohibited tool must not run");

        // Uncovered by any allowance: *authority not established*, never reported as a denial.
        let body = tools_call(http_port, "uncovered", serde_json::json!({})).await;
        assert_eq!(body["error"]["code"], -32031, "incomplete allow-list: {body}");
        assert_eq!(body["error"]["data"]["reason"], "authority_not_established");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the uncovered tool must not run");

        drop(handles);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// **Evidence is recorded, and it does not gossip.** The first version of this sealed the whole
    /// decision document into the tamper-evident chain — which is an ordinary signed KV entry, so
    /// every node received the exact resource a call targeted, the policy's reason and the
    /// constraints it checked. AE0 §5 corrected against that shape after reviewing its own first
    /// draft; this test is the pin that keeps the correction.
    ///
    /// The evidence lives in the node-local journal. The chain carries a reference and a hash.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn evidence_goes_to_the_journal_and_only_a_reference_gossips() {
        use crate::{AeEvidence, AeReference, DecisionKind, EvidenceState, Execution, RecordKind};
        use super::super::evidence_journal::{EvidenceJournal, EvidenceProfile};

        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("ae-evidence-{http_port}"));
        let journal_path = std::env::temp_dir()
            .join(format!("ae-journal-{http_port}"))
            .join("evidence.log");
        let _ = std::fs::remove_dir_all(&cert_dir);
        let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));

        let me = agent.node_id().clone();
        agent.with_evidence_journal(
            EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).unwrap(),
        );
        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-7")
                .with_catalogue("cat-test", "2")
                .map_action("tools/call", format!("tool:allowed@{me}"))
                .map_action("tools/call", format!("tool:forbidden@{me}"))
                .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", format!("tool:allowed@{me}")))
                .prohibit(Rule::new("*", "tools/call", format!("tool:forbidden@{me}"))),
        ));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut handles = Vec::new();
        for tool in ["allowed", "forbidden"] {
            handles.push(agent.mcp().register_mcp_tool(
                tool,
                serde_json::json!({"type": "object", "properties": {}}),
                move |_args| async move { Ok(serde_json::json!("ran")) },
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let permitted = tools_call(http_port, "allowed", serde_json::json!({})).await;
        assert!(permitted.get("error").is_none(), "permitted call: {permitted}");
        let refused = tools_call(http_port, "forbidden", serde_json::json!({})).await;
        assert_eq!(refused["error"]["code"], -32030);

        // ── The journal holds the evidence, in full ──────────────────────────────────────────
        let journalled: Vec<AeEvidence> = super::super::evidence_journal::read_evidence_journal(&journal_path)
            .unwrap()
            .iter()
            .map(|b| serde_json::from_slice(b).expect("an AE evidence record"))
            .collect();
        // Two decisions, and one execution record for the call that was actually dispatched.
        let decided: Vec<&AeEvidence> =
            journalled.iter().filter(|e| e.kind == RecordKind::Decided).collect();
        let executed: Vec<&AeEvidence> =
            journalled.iter().filter(|e| e.kind == RecordKind::Execution).collect();
        assert_eq!(decided.len(), 2, "both decisions are journalled");
        assert_eq!(executed.len(), 1, "only the permitted call was dispatched");

        let deny = decided.iter().find(|e| e.decision == DecisionKind::Deny).expect("the denial");
        assert!(deny.resource.starts_with("tool:forbidden@"), "the journal keeps the detail");
        assert_eq!(deny.execution, Execution::None);
        let permit =
            decided.iter().find(|e| e.decision == DecisionKind::Permit).expect("the permit");
        // Event time is the decision's own, carried from the envelope — an exporter needs it stable,
        // so it may not be invented downstream.
        assert!(permit.at_ms > 0, "the decision carries its own event time");
        // The *decision* still says only what was decided: it establishes nothing about execution.
        assert_eq!(permit.execution, Execution::Attempted);

        // The execution record is what turns `effect: unknown` into something a page can use. It is
        // appended beside the decision, carrying the same identities, never over it.
        let ran = executed[0];
        assert_eq!(ran.execution, Execution::Completed);
        assert_eq!(ran.operation_id, permit.operation_id);
        assert_eq!(ran.attempt_id, permit.attempt_id);
        assert_eq!(ran.decision, DecisionKind::Permit, "the decision is carried, not re-decided");
        assert_eq!(ran.at_ms, permit.at_ms, "one attempt, one event time");

        // ── The chain holds a reference, and nothing that identifies the action ──────────────
        let chain = agent.audit_stream(agent.node_id());
        let refs: Vec<AeReference> = chain
            .iter()
            .filter_map(|r| AeReference::from_detail(r.record.detail.as_deref()))
            .collect();
        assert_eq!(refs.len(), 3, "each journal record leaves one reference in the chain");
        assert_eq!(refs.iter().filter(|r| r.kind == "execution").count(), 1);
        for r in &refs {
            assert_eq!(r.evidence, EvidenceState::OnDisk);
            assert!(r.journal_sha256.is_some(), "a reference must cite the record it refers to");
            assert_eq!(r.catalogue, "cat-test");
        }

        // The pin. Everything that gossips, as raw text — the record's own `target` included.
        let gossiped: String = chain
            .iter()
            .map(|r| format!("{}|{}", r.record.target, r.record.detail.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "tool:forbidden",          // the exact resource — "resource details beyond the catalogue id"
            "tool:allowed",
            "prohibits this action",   // the policy's reason
            "allows this action",
        ] {
            assert!(
                !gossiped.contains(forbidden),
                "{forbidden:?} reached the gossiped audit chain; §5 forbids it.\n{gossiped}"
            );
        }
        // And the journal's hash is what ties the two together, so tampering stays detectable.
        let cited = refs[0].journal_sha256.clone().unwrap();
        let on_disk = super::super::evidence_journal::read_evidence_journal(&journal_path).unwrap();
        let hashes: Vec<String> = on_disk
            .iter()
            .map(|b| {
                use sha2::{Digest, Sha256};
                super::super::action_evaluator::hex32(&<[u8; 32]>::from(Sha256::digest(b)))
            })
            .collect();
        assert!(hashes.contains(&cited), "the reference cites a record that is actually on disk");

        agent.audit_verify(agent.node_id()).expect("the chain verifies");

        drop(handles);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
        let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());
    }

    /// **The profile decides what a failed journal means.** Strict gates the effect on durable
    /// evidence; lenient proceeds, and the chain record says the evidence was never established —
    /// so the weaker profile is legible as weaker rather than looking identical to the other kind.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn a_failing_journal_refuses_under_strict_and_is_declared_under_lenient() {
        use crate::{AeReference, EvidenceState};
        use super::super::evidence_journal::{EvidenceJournal, EvidenceProfile};

        for (profile, expect_refusal) in
            [(EvidenceProfile::Strict, true), (EvidenceProfile::Lenient, false)]
        {
            let gossip_port = alloc_port();
            let http_port   = alloc_port();
            let cert_dir = std::env::temp_dir().join(format!("ae-profile-{http_port}"));
            let _ = std::fs::remove_dir_all(&cert_dir);
            let mut cfg = GossipConfig::default();
            cfg.bind_port = gossip_port;
            cfg.http_port = Some(http_port);
            cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
            let agent =
                Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));

            let me = agent.node_id().clone();
            agent.with_evidence_journal(EvidenceJournal::stalled(profile));
            agent.with_action_evaluator(Arc::new(
                ReferenceEvaluator::new("rev-p")
                    .with_catalogue("cat-test", "1")
                    .map_action("tools/call", format!("tool:allowed@{me}"))
                    .allow(Rule::new(
                        crate::PRINCIPAL_ANONYMOUS,
                        "tools/call",
                        format!("tool:allowed@{me}"),
                    )),
            ));
            agent.start().await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _h = agent.mcp().register_mcp_tool(
                "allowed",
                serde_json::json!({"type": "object", "properties": {}}),
                |_args| async move { Ok(serde_json::json!("ran")) },
            );
            tokio::time::sleep(Duration::from_millis(50)).await;

            let body = tools_call(http_port, "allowed", serde_json::json!({})).await;
            if expect_refusal {
                assert_eq!(
                    body["error"]["code"], -32032,
                    "strict must not dispatch what it cannot evidence: {body}"
                );
                assert_eq!(body["error"]["data"]["reason"], "evidence_not_recorded");
            } else {
                assert!(body.get("error").is_none(), "lenient proceeds: {body}");
            }

            // Either way the chain says the evidence was not established — a lost acknowledgement
            // is *unknown*, never "failed", because the record may well be on disk.
            let refs: Vec<AeReference> = agent
                .audit_stream(agent.node_id())
                .iter()
                .filter_map(|r| AeReference::from_detail(r.record.detail.as_deref()))
                .collect();
            // Strict refused before dispatch, so there is only the decision. Lenient dispatched,
            // so there is a decision and an execution record — both undurable, and both saying so.
            assert_eq!(refs.len(), if expect_refusal { 1 } else { 2 });
            for r in &refs {
                assert_eq!(r.evidence, EvidenceState::Unknown);
                assert!(r.journal_sha256.is_none(), "nothing to cite, so nothing is cited");
            }

            agent.shutdown().await;
            let _ = std::fs::remove_dir_all(&cert_dir);
        }
    }

    /// **The stale-policy check could never fire.** `expected_policy_revision` was hardcoded `None`,
    /// so the seam had no second opinion and a gateway running a superseded policy was undetectable —
    /// the exact condition the check exists for. An operator now reports the deployed revision, and
    /// a decision from any other one is *authority not established*, never a permit.
    #[tokio::test]
    async fn a_reported_deployment_makes_a_stale_policy_detectable() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("ae-stale-{http_port}"));
        let _ = std::fs::remove_dir_all(&cert_dir);
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));

        let me = agent.node_id().clone();
        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-1")
                .with_catalogue("cat-test", "1")
                .map_action("tools/call", format!("tool:allowed@{me}"))
                .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", format!("tool:allowed@{me}"))),
        ));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _h = agent.mcp().register_mcp_tool(
            "allowed",
            serde_json::json!({"type": "object", "properties": {}}),
            |_args| async move { Ok(serde_json::json!("ran")) },
        );
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Control: with nothing reported there is nothing to compare against, and the call goes.
        assert!(agent.deployed_policy_revision().is_none());
        let body = tools_call(http_port, "allowed", serde_json::json!({})).await;
        assert!(body.get("error").is_none(), "no reported deployment, so no staleness: {body}");

        // The operator reports a *different* revision as deployed. The evaluator is now provably
        // not the policy anyone signed off, and the same call stops.
        agent.set_deployed_policy_revision("rev-2");
        assert_eq!(agent.deployed_policy_revision().as_deref(), Some("rev-2"));
        let body = tools_call(http_port, "allowed", serde_json::json!({})).await;
        assert_eq!(body["error"]["code"], -32031, "a stale policy establishes nothing: {body}");
        assert_eq!(body["error"]["data"]["reason"], "authority_not_established");

        // Reporting the revision actually loaded lets it through again — the check is about
        // agreement, not about being switched on.
        agent.set_deployed_policy_revision("rev-1");
        let body = tools_call(http_port, "allowed", serde_json::json!({})).await;
        assert!(body.get("error").is_none(), "agreeing revisions permit: {body}");

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
    }

    /// **A timeout is never a negative.** The call may have run; the provider may simply not have
    /// answered. `Failed` would be a claim about the world, and a consumer would act on it.
    #[test]
    fn a_timeout_or_transport_error_is_unknown_never_failed() {
        use super::super::action_evaluator::Execution;
        use super::super::gateway_caller::GatewayDispatchError;
        use super::super::rpc::RpcError;
        use super::observed_execution;
        use bytes::Bytes;

        assert_eq!(
            observed_execution(&Err(GatewayDispatchError::Rpc(RpcError::Timeout))),
            Execution::Unknown
        );
        // Refused before it left this node: nothing ran, and that *can* be stated.
        assert_eq!(
            observed_execution(&Err(GatewayDispatchError::ContextTooLarge)),
            Execution::None
        );
        assert_eq!(
            observed_execution(&Err(GatewayDispatchError::ProviderWithoutContext(
                NodeId::new("127.0.0.1", 1).unwrap(),
            ))),
            Execution::None
        );
        assert_eq!(observed_execution(&Ok(Bytes::from_static(b"{}"))), Execution::Completed);
    }

    /// A reply that arrived is a completed RPC. Whether the tool inside it succeeded is the separate
    /// question the consumer reads as `effect: failed` — and bytes that parse as nothing count as a
    /// failure, because something answered and it was not an answer.
    #[test]
    fn a_reply_carrying_an_error_is_a_failed_execution() {
        use super::reply_reports_failure;
        assert!(reply_reports_failure(br#"{"jsonrpc":"2.0","error":{"code":-32000}}"#));
        assert!(reply_reports_failure(b"not json at all"));
        assert!(!reply_reports_failure(br#"{"jsonrpc":"2.0","result":{"ok":true}}"#));
    }

    /// With no evaluator attached the gateway behaves exactly as before — the seam is additive.
    #[tokio::test]
    async fn without_an_evaluator_the_gateway_is_unchanged() {
        let gossip_port = alloc_port();
        let http_port   = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        agent.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _h = agent.mcp().register_mcp_tool(
            "square",
            serde_json::json!({"type": "object", "properties": {"n": {"type": "number"}}}),
            |args| async move { Ok(serde_json::json!(args["n"].as_f64().unwrap_or(0.0) * 2.0)) },
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        let body = tools_call(http_port, "square", serde_json::json!({"n": 21.0})).await;
        assert!(body.get("error").is_none(), "inert seam must not refuse: {body}");
        assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("42"));
        agent.shutdown().await;
    }
}
