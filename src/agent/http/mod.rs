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
//!
//! ## Layout (plan row P5, 2026-10-12)
//!
//! One file per route family, and one shared layer every family goes through — split from a single
//! 9,889-line `http.rs` with no route, scope or handler changed:
//!
//! - `mod.rs` — this file: the handler state (`HttpCtx`), the listener and TLS, the router, the public surface
//! - `authz.rs` — **the shared authorisation-and-evidence layer**: `gateway_auth` (bearer, then scope),
//!   `required_scope`, the AE preflight and its evidence records, the protected-kind / protected-key /
//!   owned-stream refusals every door calls
//! - `node.rs` (probes, `/consensus/{*slot}`) · `mcp_bridge.rs` (`/mcp`) · `signals.rs` · `caps.rs` ·
//!   `mesh.rs` (rpc, scatter, mailbox, shard) · `kv.rs` · `overlay.rs` (consensus, lock, election, log,
//!   reliable delivery) · `govern.rs` (governance + the fleet reads) · `compliance.rs` (audit, transparency,
//!   revoke) · `llm_bridge.rs` · `federation_routes.rs`
//! - `http_tests.rs` (module `tests`), `gateway_caller_tests.rs`, `ae_seam_tests.rs` — the test modules, as
//!   they were; `remainder_tests.rs` — post-360 row E's
//!
//! Every family file opens with `use super::*;`, and this file glob-imports every family back, so the
//! families, the imports below and the sibling `agent` modules named below are one namespace — a handler
//! reaches `super::helpers` or `dispatch_refused` exactly as it did in the single file.

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

// The sibling `agent` modules the handlers reach as `super::x` — named here so that path still resolves
// from a family file one level down. Gated as the modules themselves are.
use super::{
    capability_ops, electorate, emergent, guarantee, helpers, intent, mailbox, membership_governor, replica_sync,
    rpc, sharding, timing_governor, tuning_governor,
};
#[cfg(feature = "consensus")]
use super::{consensus_handle, migration};
#[cfg(feature = "compliance")]
use super::{audit, revocation, transparency};
#[cfg(all(feature = "gateway", feature = "tls"))]
use super::{action_evaluator, evidence_journal, federation_http, gateway_authority, gateway_member_keys, provider_enforcement};

mod authz;
mod caps;
#[cfg(feature = "compliance")]
mod compliance;
#[cfg(feature = "tls")]
mod federation_routes;
mod govern;
mod kv;
#[cfg(feature = "llm")]
mod llm_bridge;
mod mcp_bridge;
mod mesh;
mod node;
mod overlay;
mod signals;

use authz::*;
use caps::*;
#[cfg(feature = "compliance")]
use compliance::*;
#[cfg(feature = "tls")]
use federation_routes::*;
use govern::*;
use kv::*;
#[cfg(feature = "llm")]
use llm_bridge::*;
use mcp_bridge::*;
use mesh::*;
use node::*;
use overlay::*;
use signals::*;

// The public surface of this module, unchanged: what `agent/mod.rs`, `a2a.rs` and `provider_enforcement.rs` name.
pub use authz::{GatewayPreflight, GatewayRefusal};
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) use authz::{ae_preflight, ae_record_execution, observed_execution, Preflight};
#[cfg(all(feature = "gateway", feature = "tls", feature = "a2a"))]
pub(crate) use authz::ENFORCEMENT_POINT_A2A;

// The protected-kind list (`BUILTIN_PROTECTED_RPC_KINDS`) and its predicate live ungated in
// `agent/mod.rs` since the wasm host began refusing them at a component's `mesh.emit` (a build
// without the gateway has that door too).
pub(crate) use super::is_protected_kind;

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
    /// Requests streamed to SDK agents: `(sender, nonce, principal)` → when it was streamed and
    /// whether it has been answered. `gw_rpc_serve` records one when it streams a request to the
    /// principal that opened the stream (first record wins — a second stream of the same principal
    /// on the same kind does not reset it); `gw_rpc_respond` answers only a request recorded for the
    /// responding principal, once, and accepts a repeat from that principal idempotently (#587 F1:
    /// replicated serve loops). Bounded: entries older than `SERVED_RPC_TTL_MS` are evicted every
    /// `SERVED_RPC_SWEEP_EVERY` inserts or when the map reaches `SERVED_RPC_CAP`, and past the cap
    /// the oldest goes. Lock-order row 55 — leaf, µs (amortised).
    served_rpcs:     Mutex<ServedRpcs>,
    /// Repeat answers to an already-answered request, accepted and dropped (`/stats`
    /// `rpc_respond_duplicates`): a replicated serve loop's losing replica.
    rpc_respond_duplicates: std::sync::atomic::AtomicU64,
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
        served_rpcs:  Mutex::new(ServedRpcs::default()),
        rpc_respond_duplicates: std::sync::atomic::AtomicU64::new(0),
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
        .route("/govern/electorate",              post(gw_govern_electorate_declare))
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
        .route("/overlay/elect/{group}",          delete(gw_overlay_elect_release))
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
        // (docs/operations/rbac.md, `PUBLIC_PATHS`) — anything else goes behind `gateway_auth`.
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
    // deliberately public path (the agent card, `/a2a`) stays open.
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
    // The one place the public surface is stated in code; the test
    // `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not` holds the router to it.
    info!(public_paths = ?PUBLIC_PATHS, companion_public_paths = ?COMPANION_PUBLIC_PATHS,
          "gateway: the paths answered without a credential (the companions' only when mounted)");

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

/// **The whole public surface** — every path a gateway answers with no credential, as
/// `docs/operations/rbac.md` lists it (M16 edge criterion: what a probe, load balancer or scraper
/// needs; the A2A descriptor and `/a2a`, which an A2A peer reaches without a Mycelium credential;
/// `/bulk/{id}`, a capability URL). The router above mounts exactly these outside `gateway_auth`;
/// `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not` asserts it against a running gateway. A
/// companion route mounted outside `/gateway/`, `/a2a`, `/federation/` is public by construction
/// and belongs here or under the prefix — never silently elsewhere.
pub(crate) const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/ready",
    "/stats",
    "/metrics",
    "/bulk/{corr_id}",
    "/.well-known/agent-card.json",
    "/.well-known/agent.json",
    "/a2a",
];

/// Public paths **companions** mount through `with_http_routes`, outside the gated prefixes — public
/// by construction, listed here and in rbac.md so the public surface is written down in one place.
/// The library does not mount them; each companion's own tests serve them. `mycelium-agentfacts`'
/// signed agent-facts document and its domain record (read by peers with no Mycelium credential).
pub(crate) const COMPANION_PUBLIC_PATHS: &[&str] = &[
    "/.well-known/agent-facts.json",
    "/.well-known/agent-facts/domain.json",
];

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

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;

/// Post-360 hardening row E: the gateway remainder, one test per promise.
#[cfg(test)]
mod remainder_tests;

/// Item 7 gates (`docs/plans/v3-contracts-axis.md` §6.4, D27): the four negative cases the
/// secure profile must refuse, the `authorized_callers` gate, and the `/a2a` principal.
#[cfg(test)]
mod gateway_caller_tests;

/// The AE evaluator seam at the live gateway (`docs/design/action-envelope-ae0.md`): the unit
/// fixtures prove the decision rules; these prove the **wiring** — that a refusal stops the
/// dispatch and a permit does not.
#[cfg(all(test, feature = "tls", feature = "compliance"))]
mod ae_seam_tests;
