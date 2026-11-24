//! Identity-and-lifecycle I4, PR 2: the gateway's half — the mint under `gateway_mint_call_mandates`, its two records
//! written durably before the envelope is sent, the revoke, the re-assertion of a checkpoint another member wrote
//! over, the collector (D-5), the issuer down, and the envelope cap. End to end: a client's call through one node's
//! gateway to another node's tool, minted, admitted, registered and stopped by the gateway's cancel.

use super::call_issuer::{self, call_window_key, CallWindow, SignedCallWindow};
use super::gateway_authority::{call_checkpoint_key, grant_holder_segment, ExecutionAuthority};
use super::gateway_caller::{frame_minting, node_principal};
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use crate::mandate::grant::{EntitlementTable, GrantVerifier, SignedMandateGrant, GRANT_TTL_CAP_SECS};
use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use base64::Engine as _;
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(super) const MCP: &str = "mcp.invoke";
/// The gate's freshness window *F*: the sweep runs every *F* / 20 = 100 ms.
pub(super) const F_MS: u64 = 2_000;
/// The skew of the gate's clock model: D-5's floor counts it twice.
const SKEW_MS: u64 = 100;

#[derive(Clone, Default)]
pub(super) struct Opts {
    /// `gateway_mint_call_mandates`.
    pub(super) mint: bool,
    /// `call_mandate_ttl_secs`, when not the default.
    pub(super) ttl_secs: Option<u64>,
    /// A certificate directory shared with the other node, and the peers to bootstrap to.
    pub(super) cert_dir: Option<std::path::PathBuf>,
    pub(super) bootstrap: Vec<NodeId>,
    /// Fixed ports and directory: a restart over the same identity.
    pub(super) gossip_port: Option<u16>,
    pub(super) http_port: Option<u16>,
    /// `[persistence]` under the node's directory.
    pub(super) persist: bool,
    /// With `persist`: its `sync_mode` and `snapshot_wal_threshold`, when not `Flush` and 1 000 (§9 O10's tests: a
    /// burst of appends that no inline snapshot interrupts and the WAL's queue, four times the threshold, holds).
    pub(super) wal: Option<(crate::config::SyncMode, usize)>,
    /// This node serves the tools (`slow`, 30 s; `quick`, at once).
    pub(super) tools: bool,
    /// `collect_expired_records`.
    pub(super) collect: bool,
    /// `grant_retention_secs`, when not the default.
    pub(super) retention_secs: Option<u64>,
    /// I4, PR 3: the policy's rules are **not** cancellable (a plain allowance), so a call at a door that cannot stop
    /// it is admitted and registered unconfirmed.
    pub(super) plain: bool,
    /// I4, PR 3: this node also serves `unstoppable` (30 s) with `register_mcp_tool_unstoppable`, and the A2A skill
    /// `depot/dispatch` through an `rpc_rx` receiver that declared it stops; the gateway mounts `/a2a`.
    pub(super) more_doors: bool,
    /// I4, PR 3a (#799's review, M1): the provider in `pair` has an execution authority and **no** provider
    /// enforcement.
    pub(super) no_enforce: bool,
    /// B1: `control_lane = false` on this node.
    pub(super) no_lane: bool,
    /// B1, in `pair`: the provider runs with `[persistence]` (`Flush`) and with `control_lane = false`.
    pub(super) provider_persist: bool,
    pub(super) provider_no_lane: bool,
}

pub(super) struct N {
    pub(super) a: Arc<GossipAgent>,
    pub(super) authority: Arc<ExecutionAuthority>,
    dir: std::path::PathBuf,
    pub(super) http_port: u16,
    pub(super) started: Arc<AtomicUsize>,
    pub(super) finished: Arc<AtomicUsize>,
}

/// One node: a gateway (named token `alice`, `mcp:invoke`), an enforcing provider, an execution authority with
/// *F* = 2 s, and a policy under which **every tool call is cancellable**.
pub(super) async fn node(opts: Opts) -> N {
    let port = opts.gossip_port.unwrap_or_else(crate::test_util::alloc_port);
    let http_port = opts.http_port.unwrap_or_else(crate::test_util::alloc_port);
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("i4-mint-{port}"));
    if opts.gossip_port.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.bootstrap_peers = opts.bootstrap.clone();
    cfg.reconnect_backoff_secs = 1;
    cfg.tls = Some({
        let mut tls_cfg = crate::TlsConfig::default();
        tls_cfg.auto_cert_dir = opts.cert_dir.clone().unwrap_or_else(|| dir.join("tls"));
        tls_cfg
    });
    if opts.persist {
        use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
        let (sync_mode, snapshot_wal_threshold) = opts.wal.unwrap_or((SyncMode::Flush, 1_000));
        cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode, snapshot_wal_threshold, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    }
    cfg.gateway_mint_call_mandates = opts.mint;
    cfg.control_lane = !opts.no_lane;
    cfg.collect_expired_records = opts.collect;
    if let Some(r) = opts.retention_secs {
        cfg.grant_retention_secs = r;
    }
    if let Some(t) = opts.ttl_secs {
        cfg.call_mandate_ttl_secs = t;
    }
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![
        crate::config::GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] },
        crate::config::GatewayNamedToken { name: "bob".into(), token: "bob-secret".into(), scopes: vec!["mcp:invoke".into()] },
        // B1's T4b: a client that may emit signals.
        crate::config::GatewayNamedToken { name: "mesher".into(), token: "mesher-secret".into(), scopes: vec!["mesh:write".into()] },
    ];
    let a = GossipAgent::new(id.clone(), cfg);
    #[cfg(feature = "a2a")]
    let a = if opts.more_doors { a.with_a2a() } else { a };
    let a = Arc::new(a);
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    let rule = |action: &str| if opts.plain { Rule::new("*", action, "*") } else { Rule::new("*", action, "*").cancellable() };
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-mint")
            .with_catalogue("cat-mint", "1")
            .map_action("tools/call", "*")
            .map_action("skill.invoke", "*")
            .allow(rule("tools/call"))
            .allow(rule("skill.invoke")),
    ));
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, FreshnessPolicy { freshness_ms: F_MS, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(EntitlementTable::new()), TrustedExternalIssuers::new()));
    a.with_execution_authority(Arc::clone(&authority));
    if !opts.no_enforce {
        a.with_provider_enforcement();
    }
    a.start().await.unwrap();
    let (started, finished) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    if opts.tools {
        for (tool, secs) in [("slow", 30u64), ("quick", 0)] {
            let (s, f) = (Arc::clone(&started), Arc::clone(&finished));
            std::mem::forget(a.mcp().register_mcp_tool(tool, serde_json::json!({"type": "object"}), move |_| {
                let (s, f) = (Arc::clone(&s), Arc::clone(&f));
                async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    f.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!("done"))
                }
            }));
        }
    }
    if opts.tools && opts.more_doors {
        let (s, f) = (Arc::clone(&started), Arc::clone(&finished));
        std::mem::forget(a.mcp().register_mcp_tool_unstoppable("unstoppable", serde_json::json!({"type": "object"}), move |_| {
            let (s, f) = (Arc::clone(&s), Arc::clone(&f));
            async move {
                s.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(30)).await;
                f.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!("done"))
            }
        }));
        // The A2A skill, served by a receiver that declared it stops: it races `authority_lapsed()`.
        std::mem::forget(a.capabilities().advertise_capability(crate::capability::Capability::new("depot", "dispatch"), Duration::from_secs(60)));
        let mut rx = a.service().rpc_rx("skill.invoke").stops_when_authority_lapses();
        let (a2, s, f) = (Arc::clone(&a), Arc::clone(&started), Arc::clone(&finished));
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let (a3, s, f) = (Arc::clone(&a2), Arc::clone(&s), Arc::clone(&f));
                tokio::spawn(async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    // `soon` completes on its own in 2 s (#821's review, F1: a completion racing its cancel); else 30 s.
                    let secs = if req.payload().as_ref() == b"soon" { 2 } else { 30 };
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                            f.fetch_add(1, Ordering::SeqCst);
                            a3.service().rpc_respond(&req, b"dispatched".to_vec());
                        }
                        _ = req.authority_lapsed() => {
                            // `linger` takes 800 ms to stop once asked (#826's review, F1: a provider still waiting for
                            // the work to end when a re-sent forward arrives).
                            if req.payload().as_ref() == b"linger" {
                                tokio::time::sleep(Duration::from_millis(800)).await;
                            }
                            a3.service().rpc_respond(&req, b"stopped: authority lapsed".to_vec())
                        }
                    }
                });
            }
        });
    }
    assert!(until(|| std::net::TcpStream::connect(("127.0.0.1", http_port)).is_ok(), Duration::from_secs(20)).await, "the gateway is listening");
    N { a, authority, dir, http_port, started, finished }
}

impl N {
    pub(super) fn ctx(&self) -> &Arc<super::TaskCtx> {
        &self.a.task_ctx
    }
    pub(super) fn gateway(&self) -> String {
        node_principal(self.a.node_id())
    }
    pub(super) fn now(&self) -> u64 {
        self.ctx().hlc.decision_now_ms()
    }
    pub(super) fn kv(&self, key: &str) -> Option<Bytes> {
        mycelium_core::ops::kv_get(self.ctx(), key)
    }
    fn call_keys(&self) -> Vec<String> {
        crate::store::scan_kv_prefix(&self.ctx().kv_state, "call/").into_iter().map(|(k, _)| k.to_string()).collect()
    }
    pub(super) async fn running(&self, n: usize) {
        assert!(until(|| self.authority.running_count() == n, Duration::from_secs(20)).await, "running_count {} (wanted {n})", self.authority.running_count());
    }
    pub(super) async fn began(&self, k: usize) {
        assert!(until(|| self.started.load(Ordering::SeqCst) == k, Duration::from_secs(20)).await, "started {}", self.started.load(Ordering::SeqCst));
    }
    /// Wait until `n` calls are registered here while the call `answered` is still out — and if it is answered first,
    /// fail with its answer (#794's review, F1): a refusal names its code and reason, not only "running_count 0".
    async fn running_or_answered(&self, n: usize, answered: &mut tokio::task::JoinHandle<serde_json::Value>) {
        let done = until(|| self.authority.running_count() == n || answered.is_finished(), Duration::from_secs(20)).await;
        if done && self.authority.running_count() == n {
            return;
        }
        let reply = if answered.is_finished() { Some((&mut *answered).await) } else { None };
        panic!("running_count {} (wanted {n}); the call's answer: {reply:?}", self.authority.running_count());
    }
    /// A client's `tools/call` through this node's `/mcp`.
    pub(super) async fn client_call(&self, tool: &str) -> serde_json::Value {
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {"aisle": 4}}});
        let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/mcp", self.http_port)).bearer_auth("alice-secret").json(&body).send().await.expect("the gateway answers");
        let status = resp.status().as_u16();
        let mut v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        v["http_status"] = serde_json::json!(status);
        v
    }
    /// Frame a member's own `tool` call to `target` as the gateway's dispatch frames it — the real mint, its records
    /// written — and return the frame and its term.
    pub(super) async fn framed(&self, target: &NodeId, tool: &str) -> (Bytes, String) {
        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {"aisle": 4}}}).to_string();
        let (framed, minted) = frame_minting(self.ctx(), target, &self.gateway(), &[], MCP, Bytes::from(payload), None, None, None, None, self.now());
        let minted = minted.expect("minted");
        let term = minted.cm["grant"]["mandate"]["term"].as_str().unwrap().to_string();
        call_issuer::record(self.ctx(), minted).await.expect("the records are written");
        (framed.expect("fits"), term)
    }
    pub(super) async fn send(&self, target: &NodeId, framed: Bytes) -> serde_json::Value {
        let reply = super::rpc::rpc_call_framed(self.ctx(), target.clone(), Arc::from(MCP), framed, Duration::from_secs(60)).await.expect("a reply");
        serde_json::from_slice(&reply).unwrap()
    }
}

pub(super) async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

/// Two nodes under one CA: `g` the gateway (`gopts`), `p` the provider serving the tools; ready when each holds the
/// other's key and `g` sees both of `p`'s tools.
pub(super) async fn pair(gopts: Opts) -> (N, N, std::path::PathBuf) {
    let (plain, more_doors, no_enforce) = (gopts.plain, gopts.more_doors, gopts.no_enforce);
    let (persist, no_lane) = (gopts.provider_persist, gopts.provider_no_lane);
    let cert_dir = std::env::temp_dir().join(format!("i4-mint-ca-{}", crate::test_util::alloc_port()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let g = node(Opts { cert_dir: Some(cert_dir.clone()), ..gopts }).await;
    let p = node(Opts { cert_dir: Some(cert_dir.clone()), bootstrap: vec![g.a.node_id().clone()], tools: true, plain, more_doors, no_enforce, persist, no_lane, ..Opts::default() }).await;
    let knows = |x: &GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    assert!(until(|| knows(&g.a, p.a.node_id()) && knows(&p.a, g.a.node_id()), Duration::from_secs(20)).await, "the nodes learned each other's keys");
    // Both tools: a test that calls `quick` straight after this was answered `tool not found: quick` under load when
    // only `slow` was waited for (#794's review, F1 — `only_a_gateway_with_minting_on_mints_a_call_mandate`).
    for tool in ["slow", "quick"] {
        let key = format!("tools/{tool}/{}", p.a.node_id());
        assert!(until(|| g.kv(&key).is_some(), Duration::from_secs(20)).await, "the gateway sees the provider's tool {tool}");
    }
    // #802: and holds the provider's caller-context marker — until then `/mcp` passes the provider over (`tool not
    // found`, which this test's client does not retry). The marker and the tool are separate keys gossip may deliver in
    // either order: this wait, missing, is what the -32021 sightings of this file were (before #802 the gateway sent
    // and was refused `provider_without_caller_context`).
    assert!(until(|| super::gateway_caller::gateway_sends_to(g.ctx(), p.a.node_id()), Duration::from_secs(20)).await, "the gateway holds the provider's marker");
    (g, p, cert_dir)
}

/// **Exits 3 (its positive half) and 4, end to end over two nodes.** A client's call through `g`'s `/mcp` to `p`'s
/// 30 s tool, under a cancellable rule: `g` mints the call's `call:` mandate (its term `c{n}` of the envelope), writes
/// its checkpoint and window record (which reach `p`), and `p` admits the call and registers its work under it. The
/// gateway's cancel (`revoke_call`) reaches `p` by gossip, `p`'s feeder reads it for the window it holds, and its sweep
/// stops the call **within gossip + *F* / 20 + the drain bound, asserted against the configured *F* (2 s)**; the
/// client is answered `cancelled`, the tool did not finish, and `p`'s stop record names the term. Seen failing first:
/// with `mints_for` answering `false`, the call was never registered at the provider (refused `no_call_mandate`).
/// #794's review, K2: this test timed out once in CI with nothing registered — the gateway saw the provider's tool
/// before its caller-context marker and refused the call `provider_without_caller_context`. The provider publishes the
/// marker before it advertises the tool (`a_provider_publishes_its_caller_context_marker_before_it_advertises_work`),
/// but gossip delivers the two keys in either order, so it recurred under load on main 542df4ae (#802). Fixed at the
/// gateway: it passes over a provider whose marker it does not hold (`gateway_sends_to`), and `pair()` waits for it;
/// a miss prints the client's answer and the counters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clients_call_through_the_gateway_is_minted_admitted_and_stopped_by_a_cancel() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    // On a miss, say why (#794's review, K2): the client's answer, the gateway's mints and the provider's refusals.
    if !until(|| p.authority.running_count() == 1 || answered.is_finished(), Duration::from_secs(20)).await || p.authority.running_count() != 1 {
        let reply = if answered.is_finished() { answered.await.ok() } else { None };
        let c = &g.ctx().call_issuer;
        panic!(
            "the call was not registered at the provider: client answer {reply:?}; gateway mints {} refused {}; provider windows {:?}, call_windows_refused {}, checkpoints refused {}",
            c.mints.load(Ordering::Relaxed), c.mints_refused.load(Ordering::Relaxed), p.authority.view_sizes(),
            p.authority.call_windows_refused(), p.authority.call_checkpoints_refused(),
        );
    }
    p.began(1).await;
    let terms = g.ctx().call_issuer.issued_terms();
    assert_eq!(terms.len(), 1, "one call, one mint: {terms:?}");
    let term = terms[0].clone();
    assert!(term.starts_with('c') && term[1..].parse::<u64>().is_ok(), "the term is c{{n}}: {term}");
    let (cp, win) = (call_checkpoint_key(&g.gateway(), &term), call_window_key(&g.gateway(), &term));
    assert!(g.kv(&cp).is_some() && g.kv(&win).is_some(), "the gateway wrote both records");
    assert!(until(|| p.kv(&cp).is_some() && p.kv(&win).is_some(), Duration::from_secs(10)).await, "both reached the provider");
    let members = call_issuer::record_keys(p.ctx());
    assert!(call_issuer::verified_window(&win, &p.kv(&win).unwrap(), &members).is_some(), "the provider verifies the window record under the gateway's key");
    assert_eq!(p.authority.view_sizes().2, 1, "the provider holds the call's window");

    let asked = std::time::Instant::now();
    assert_eq!(call_issuer::revoke_call(g.ctx(), &term).await, Ok(true), "the cancel is written");
    let reply = answered.await.unwrap();
    let took = asked.elapsed();
    assert!(took < Duration::from_millis(F_MS), "stopped within gossip + F/20 + the drain bound, against F = {F_MS} ms: {took:?}");
    assert!(reply.to_string().contains("cancelled"), "the client is answered cancelled: {reply}");
    p.running(0).await;
    assert_eq!((p.started.load(Ordering::SeqCst), p.finished.load(Ordering::SeqCst)), (1, 0), "the tool's future was dropped: it never finished");
    assert_eq!(p.authority.named_stop_records().last().and_then(|s| s.term.clone()), Some(term.clone()), "the stop record names the term");
    assert_eq!(call_issuer::revoke_call(g.ctx(), &term).await, Ok(false), "a second cancel is idempotent");
    eprintln!("i4-pr2 bound 1, loopback, two nodes, F = {F_MS} ms: revoke written -> client answered {took:?}");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 3: only a gateway with minting turned on mints one.** The same client call through a gateway with
/// `gateway_mint_call_mandates` off is refused at the provider `no_call_mandate` (the tool does not start) and no
/// `call/` record is written anywhere; through one with it on, the quick tool answers and the gateway counts one mint.
/// Seen failing first: with `mints_for` ignoring the setting, the mint-off gateway minted and the call ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_gateway_with_minting_on_mints_a_call_mandate() {
    let (g, p, cert_dir) = pair(Opts::default()).await;
    let v = g.client_call("quick").await;
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["cause"].as_str()), (Some(-32031), Some("no_call_mandate")), "{v}");
    assert_eq!(p.started.load(Ordering::SeqCst), 0, "the tool did not start");
    assert!(g.call_keys().is_empty() && p.call_keys().is_empty(), "nothing under call/");
    assert_eq!(g.ctx().call_issuer.mints.load(Ordering::Relaxed), 0);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);

    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let v = g.client_call("quick").await;
    assert!(v["error"].is_null() && v.to_string().contains("done"), "{v}");
    assert_eq!((g.ctx().call_issuer.mints.load(Ordering::Relaxed), p.finished.load(Ordering::SeqCst)), (1, 1));
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 11, the durable writes.** At a gateway with `[persistence]`, each mint's checkpoint and window record are
/// on the WAL — fsynced — before the envelope is sent: the added admission latency is measured here (both writes, n =
/// 20, printed). With the WAL writer poisoned, the client's call is refused **`call_mandate_not_durable`** (-32025): no
/// envelope is sent (the tool never starts), no record is written, nothing is held as issued; and a revoke is
/// answered `NotDurable`, never acknowledged, while the disk is gone (the same write). Seen failing first: with the
/// writes on `kv_set_with_receipt` (no sync), the poisoned call was sent and ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mints_records_are_on_disk_before_the_call_is_sent_or_nothing_is_sent() {
    let n = node(Opts { mint: true, persist: true, tools: true, ..Opts::default() }).await;
    for _ in 0..20 {
        let v = n.client_call("quick").await;
        assert!(v["error"].is_null(), "{v}");
    }
    let mut took = call_issuer::test_hooks::record_latencies(n.a.node_id());
    took.sort();
    assert_eq!(took.len(), 20);
    eprintln!("i4-pr2 exit 11: the mint's two durable writes, SyncMode::Flush, n = 20: p50 {:?}, p95 {:?}, max {:?}", took[10], took[18], took[19]);
    let (keys, issued) = (n.call_keys().len(), n.ctx().call_issuer.issued_terms().len());
    assert_eq!((keys, issued), (40, 20), "two records per call, each held as issued");
    n.ctx().wal.get().expect("persists").poison_for_test("the disk is gone").await.unwrap();
    let v = n.client_call("quick").await;
    // `/mcp` answers a dispatch the gateway refused as a JSON-RPC error: -32025, its reason by name.
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["reason"].as_str()), (Some(-32025), Some("call_mandate_not_durable")), "{v}");
    assert_eq!(n.finished.load(Ordering::SeqCst), 20, "the refused call was never sent");
    assert_eq!((n.call_keys().len(), n.ctx().call_issuer.issued_terms().len()), (keys, issued), "nothing written, nothing held");
    // The cancel is acknowledged only once its checkpoint is on disk: refused by name while the disk is gone.
    let term = n.ctx().call_issuer.issued_terms().pop().unwrap();
    assert!(matches!(call_issuer::revoke_call(n.ctx(), &term).await, Err(call_issuer::RevokeRefusal::NotDurable(_))), "a revoke the disk will not take is not acknowledged");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// **Exit 11, the WAL-only restart.** A gateway that is also the provider mints two calls (their records on its WAL)
/// and cancels the first; the node is restarted with only the WAL carrying its records (the exit snapshot sabotaged).
/// The cancelled call's envelope, presented again inside its window, is refused **`revoked`**, read from the KV record
/// the WAL brought back — the restart cleared the replay window, which is what makes the re-presentation reach the
/// rule — and the other call's envelope, whose checkpoint was issued before the restart, is established and runs
/// (`call:` is exempt from the start rule). No window table is persisted: after the restart the provider holds only
/// the window of the call it admitted since. Seen failing first: with the revoke's write taken out (the revocation
/// in memory only), the restarted node ran the cancelled call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_call_presented_again_after_a_wal_only_restart_is_refused_and_another_runs() {
    use crate::test_util::{kv_dir_under, restore_start_snapshot, sabotage_exit_snapshot};
    let (gossip_port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let opts = Opts { mint: true, persist: true, tools: true, gossip_port: Some(gossip_port), http_port: Some(http_port), ..Opts::default() };
    let dir = std::env::temp_dir().join(format!("i4-mint-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let n = node(opts.clone()).await;
    let me = n.a.node_id().clone();
    let (cancelled, term) = n.framed(&me, "quick").await;
    let (kept, _) = n.framed(&me, "quick").await;
    assert_eq!(call_issuer::revoke_call(n.ctx(), &term).await, Ok(true));
    let kv_dir = kv_dir_under(&dir.join("persist"));
    sabotage_exit_snapshot(&kv_dir);
    n.a.shutdown().await;
    drop(n);
    tokio::time::sleep(Duration::from_millis(300)).await;
    restore_start_snapshot(&kv_dir);
    let n = node(opts).await;
    assert!(n.ctx().call_issuer.issued_terms().is_empty(), "nothing issued is kept in memory across a restart (D-4)");
    assert_eq!(n.authority.view_sizes().2, 0, "no window table is persisted");
    let v = n.send(&me, cancelled).await;
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["cause"].as_str()), (Some(-32031), Some("revoked")), "{v}");
    let v = n.send(&me, kept).await;
    assert!(v["error"].is_null(), "the call minted before the restart is established after it: {v}");
    assert_eq!(n.finished.load(Ordering::SeqCst), 1, "only the kept call ran");
    // The term is known again from the gateway's own verified records, so a cancel after the restart answers it — by
    // writing the revoking checkpoint durably again: this process cannot know the KV's copy is on its disk (K1).
    assert_eq!(call_issuer::revoke_call(n.ctx(), &term).await, Ok(true), "written again after the restart");
    assert_eq!(call_issuer::revoke_call(n.ctx(), &term).await, Ok(false), "and then on record");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **H1 (#775's review), end to end: the gateway re-asserts its checkpoint, so a member that writes the call's first
/// checkpoint back over the cancel does not keep the call running.** The call runs at `p`. A member writes `g`'s own
/// first checkpoint — genuine bytes, already verified at `p` — back at the key with a later stamp than the cancel will
/// have, and it reaches both nodes before the cancel: at `p` the cancel then loses last-writer-wins and `p` sees only
/// bytes it has verified (the case the detectors cannot see). `g`, seeing at its own key a checkpoint older than the
/// one it holds, writes its revoking checkpoint again past that stamp; it wins at `p`, and `p` stops the call. Seen
/// failing first: with `reassert` returning before it writes, the call had not stopped at `p` 10 s after the cancel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_checkpoint_written_back_over_the_cancel_is_re_asserted_and_the_call_still_stops() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let mut answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running_or_answered(1, &mut answered).await;
    let term = g.ctx().call_issuer.issued_terms().pop().unwrap();
    let key = call_checkpoint_key(&g.gateway(), &term);
    assert!(until(|| p.kv(&key).is_some(), Duration::from_secs(10)).await);
    let first = p.kv(&key).unwrap();
    // The member's write: the first checkpoint, at a stamp 2 s ahead of the cancel. It reaches the provider before
    // the cancel does — so there the cancel loses last-writer-wins — and the gateway after it, as gossip would.
    let ahead = crate::hlc::pack(g.now() + 2_000, 0);
    let member = NodeId::new("127.0.0.1", 1).unwrap();
    let replay = || crate::framing::make_gossip_update_stamped(&member, 5, Arc::from(key.as_str()), first.clone(), false, ahead);
    crate::store::apply_and_notify(&p.ctx().kv_state, &replay());
    let asked = std::time::Instant::now();
    assert_eq!(call_issuer::revoke_call(g.ctx(), &term).await, Ok(true));
    crate::store::apply_and_notify(&g.ctx().kv_state, &replay());
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the call stopped").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert!(g.ctx().call_issuer.reasserted.load(Ordering::Relaxed) >= 1, "the gateway re-asserted its checkpoint");
    eprintln!("i4-pr2 H1: cancel under a replayed first checkpoint, re-asserted: revoke -> answered {:?}", asked.elapsed());
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// A checkpoint for `term`'s scope under `gateway`'s key segment, signed by `key` — what another member could write.
fn forged_checkpoint(key: &ed25519_dalek::SigningKey, gateway: &str, term: &str, seq: u64, at_ms: u64) -> Vec<u8> {
    serde_json::to_vec(&super::gateway_authority::call_mint::checkpoint(key, gateway, &format!("call:{term}"), seq, at_ms, &[])).unwrap()
}

/// A window record for `term`, signed by `key`.
fn forged_window(key: &ed25519_dalek::SigningKey, term: &str, until_ms: u64) -> Vec<u8> {
    let window = CallWindow { v: 1, scope: format!("call:{term}"), term: term.into(), valid_until_ms: until_ms };
    let signature = base64::engine::general_purpose::STANDARD.encode(mycelium_core::tls::sign_bytes(key, &call_issuer::window_message(&window)));
    serde_json::to_vec(&SignedCallWindow { window, signature }).unwrap()
}

/// **The owner-signed rule over the two records PR 2 writes** (hot invariant; the plan's H1). Each is written as
/// another member would and the gateway is shown unmoved:
/// - at the **checkpoint** key, a checkpoint signed by another key, then a tombstone: each is re-asserted — the
///   gateway's own value is back, counted;
/// - a **window** record signed by another key, or naming another term, or missing, reads as absent: the collector
///   then keeps the pair until the checkpoint's `issued_at_ms` + `GRANT_TTL_CAP_SECS` + 2·skew + `max_drain_secs`,
///   never earlier (a forged record can only delay collection) — and a genuine one lets it go at its window's floor;
/// - a pair under this gateway's segment **neither of whose records verifies** is never collected, and counted.
///
/// Seen failing first: with `verified_window` not checking the signature, the forged window (ending at once) let the
/// collector take the pair at its forged floor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forged_or_missing_record_at_a_calls_keys_moves_neither_the_gateway_nor_its_collector() {
    let n = node(Opts { mint: true, tools: true, ..Opts::default() }).await;
    let me = n.a.node_id().clone();
    let other = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
    let (_framed, term) = n.framed(&me, "quick").await;
    let (cp, win) = (call_checkpoint_key(&n.gateway(), &term), call_window_key(&n.gateway(), &term));
    let ours = n.kv(&cp).unwrap();
    // The checkpoint key: forged, then tombstoned — each re-asserted.
    super::state_machine::write_owned_kv(n.ctx(), &cp, forged_checkpoint(&other, &n.gateway(), &term, 9, n.now()));
    assert!(until(|| n.kv(&cp).as_ref() == Some(&ours), Duration::from_secs(5)).await, "the forged checkpoint was written over");
    mycelium_core::ops::kv_delete(n.ctx(), Arc::from(cp.as_str()));
    assert!(until(|| n.kv(&cp).as_ref() == Some(&ours), Duration::from_secs(5)).await, "the tombstone was written over");
    // The count follows the write (`reassert` writes, then counts): wait on it, not on the value.
    assert!(until(|| n.ctx().call_issuer.reasserted.load(Ordering::Relaxed) == 2, Duration::from_secs(5)).await, "each re-assertion counted");

    // The window: genuine first, to read its end.
    let members = call_issuer::record_keys(n.ctx());
    let genuine = call_issuer::verified_window(&win, &n.kv(&win).unwrap(), &members).expect("the gateway's own window verifies");
    let checkpoint = call_issuer::verified_checkpoint(&cp, &ours, &members).unwrap();
    let drain = n.ctx().config.max_drain_secs * 1_000;
    let floor = genuine.valid_until_ms + 2 * SKEW_MS + drain;
    let fallback = checkpoint.issued_at_ms + GRANT_TTL_CAP_SECS * 1_000 + 2 * SKEW_MS + drain;
    assert!(fallback > floor);
    // Forged, ending now: read as absent — kept to the fallback floor, not the forged one.
    super::state_machine::write_owned_kv(n.ctx(), &win, forged_window(&other, &term, n.now()));
    assert!(call_issuer::verified_window(&win, &n.kv(&win).unwrap(), &members).is_none());
    assert_eq!(call_issuer::collect(n.ctx(), floor + 1).call_records, 0, "a forged window does not hasten collection");
    // Naming another term, signed by the gateway: absent too.
    let tls = n.ctx().tls.get().unwrap().signing_key();
    super::state_machine::write_owned_kv(n.ctx(), &win, forged_window(&tls, "c1", n.now()));
    assert!(call_issuer::verified_window(&win, &n.kv(&win).unwrap(), &members).is_none(), "a window for another term at this key");
    assert_eq!(call_issuer::collect(n.ctx(), floor + 1).call_records, 0);
    assert_eq!(call_issuer::collect(n.ctx(), fallback).call_records, 0, "kept up to the fallback floor");
    let before = n.ctx().call_issuer.call_records_unverifiable.load(Ordering::Relaxed);
    assert_eq!(call_issuer::collect(n.ctx(), fallback + 1).call_records, 1, "past it, the gateway's own checkpoint goes");
    assert!(n.kv(&cp).is_none(), "its own record collected");
    assert!(n.kv(&win).is_some(), "the value it did not sign at its window key is neither trusted nor deleted");
    assert!(n.ctx().call_issuer.call_records_unverifiable.load(Ordering::Relaxed) > before, "and counted");

    // A pair neither of whose records verifies: never collected, counted.
    let junk = "c4242";
    super::state_machine::write_owned_kv(n.ctx(), &call_checkpoint_key(&n.gateway(), junk), forged_checkpoint(&other, &n.gateway(), junk, 1, 0));
    super::state_machine::write_owned_kv(n.ctx(), &call_window_key(&n.gateway(), junk), forged_window(&other, junk, 0));
    let before = n.ctx().call_issuer.call_records_unverifiable.load(Ordering::Relaxed);
    assert_eq!(call_issuer::collect(n.ctx(), u64::MAX / 2).call_records, 0);
    assert!(n.ctx().call_issuer.call_records_unverifiable.load(Ordering::Relaxed) > before, "counted");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// **Exit 12, the `call/` family, owner-only** (the maintainer's decision of 2026-11-15). A minted call's two records
/// are kept up to D-5's floor — the window's end + 2·skew + `max_drain_secs` — and collected one millisecond past it,
/// by their issuer; what the gateway held as issued goes with them. **The provider that holds them by gossip never
/// deletes them**, even with the issuer stopped and far past the floor: they are another agent's records. Seen
/// failing first: with the floor read without `max_drain_secs`, the records went at the window's end + 2·skew; and
/// with the collector deleting every `call/` pair it holds (the PR's first head), the provider took the gateway's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_calls_records_are_collected_past_their_floor_by_their_issuer_and_by_no_other_node() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let target = p.a.node_id().clone();
    let (_a, issuer_term) = g.framed(&target, "quick").await;
    let (_b, orphan_term) = g.framed(&target, "quick").await;
    let members = call_issuer::record_keys(g.ctx());
    let win = call_window_key(&g.gateway(), &issuer_term);
    let until_ms = call_issuer::verified_window(&win, &g.kv(&win).unwrap(), &members).unwrap().valid_until_ms;
    let floor = until_ms + 2 * SKEW_MS + g.ctx().config.max_drain_secs * 1_000;
    assert_eq!(call_issuer::collect(g.ctx(), floor).call_records, 0, "kept at the floor");
    assert_eq!(g.call_keys().len(), 4);
    // Past it, both calls' records go at the issuer (the second's floor is within a millisecond or two of the first's).
    let past = floor + 1_000;
    assert_eq!(call_issuer::collect(g.ctx(), past).call_records, 4, "collected past the floor");
    assert!(g.call_keys().is_empty() && g.ctx().call_issuer.issued_terms().is_empty());
    let _ = orphan_term;

    // The issuer stopped: a third call's records, held at the provider, are collected there.
    let (_c, term) = g.framed(&target, "quick").await;
    let (cp, win) = (call_checkpoint_key(&g.gateway(), &term), call_window_key(&g.gateway(), &term));
    assert!(until(|| p.kv(&cp).is_some() && p.kv(&win).is_some(), Duration::from_secs(10)).await, "the provider holds the records");
    let until_ms = call_issuer::verified_window(&win, &p.kv(&win).unwrap(), &call_issuer::record_keys(p.ctx())).unwrap().valid_until_ms;
    let floor = until_ms + 2 * SKEW_MS + p.ctx().config.max_drain_secs * 1_000;
    g.a.shutdown().await;
    assert_eq!(call_issuer::collect(p.ctx(), floor + 1).call_records, 0, "not the provider's to delete");
    assert_eq!(call_issuer::collect(p.ctx(), u64::MAX / 2).call_records, 0, "not even far past the floor");
    assert!(p.kv(&cp).is_some() && p.kv(&win).is_some(), "the gateway's records stand at the provider");
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 12, the `grant/` family, owner-only, with critique plan CR2's markers and releases** (the maintainer's
/// decision of 2026-11-15: each agent's retention is a promise about its own records). This node is the grant's
/// issuer and one of its doors:
/// - as the issuer it deletes its own `issued` and `checkpoint` — each signed under its key — past the later of the
///   window + 2·skew and the issue + `grant_retention_secs`, and keeps them before;
/// - as a door it deletes the use marker it signed past its `at_ms` + the retention + 2·skew;
/// - it deletes nothing else: another door's marker (signed by another node), a forged marker naming this node, the
///   request, `consumed`, `revoked`, and every record of a grant whose `issued` another key signed — even far past;
/// - and the holder's status route then answers the request **`expired`**, from the request record it kept, not
///   `unknown_request`.
///
/// Seen failing first: with the collector deleting every record under a verified grant (the PR's first head), the
/// other door's marker and the request went, and the status route answered `404 unknown_request`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_owner_collects_only_its_own_grant_records_and_the_holder_reads_expired() {
    let n = node(Opts { collect: true, ..Opts::default() }).await;
    let ctx = n.ctx();
    let key = ctx.tls.get().unwrap().signing_key();
    let other = ed25519_dalek::SigningKey::from_bytes(&[78u8; 32]);
    let holder = "token:gw/alice";
    let now = n.now();
    let me = n.gateway();
    let sign_checkpoint = |sk: &ed25519_dalek::SigningKey, request: &str| {
        serde_json::to_vec(&super::gateway_authority::call_mint::checkpoint(sk, &me, &format!("grant:{request}"), 1, now, &[])).unwrap()
    };
    let write_grant = |request: &str, signer: &ed25519_dalek::SigningKey| {
        let m = Mandate {
            holder: PrincipalId::new(holder).unwrap(),
            established_by: PrincipalId::new(&me).unwrap(),
            purpose: "grant".into(),
            scope: format!("grant:{request}"),
            operations: vec!["tools/call:tool:deploy@x".into()],
            epoch: 1,
            term: TermId::new(request).unwrap(),
            valid_from_ms: now,
            valid_until_ms: now + 300_000,
        };
        let g = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(signer, &m.canonical_bytes()).to_vec(), mandate: m };
        let base = format!("grant/{}/{request}", grant_holder_segment(holder));
        let request_record = serde_json::json!({"schema": "mycelium.grant-request/1", "request": request, "agent": holder, "tool": "deploy", "requested_at_ms": now, "expires_at_ms": now.saturating_sub(1)});
        for (k, v) in [
            (base.clone(), request_record.to_string().into_bytes()),
            (format!("{base}/issued"), serde_json::to_vec(&g).unwrap()),
            (format!("{base}/checkpoint"), sign_checkpoint(signer, request)),
            (format!("{base}/consumed"), b"{}".to_vec()),
            (format!("{base}/revoked"), b"{}".to_vec()),
        ] {
            super::state_machine::write_owned_kv(ctx, &k, v);
        }
        base
    };
    let genuine = write_grant("r1", &key);
    let forged = write_grant("r2", &other);
    // The doors: this node's own marker; another door's, signed by its node; a forged one that names this node.
    let marker = |base: &str, point: &str| format!("{base}/used/{point}");
    let own = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker(&genuine, "provider"), "u1", now + 300_000).unwrap();
    ctx.grant_uses.record_use(&marker(&genuine, "provider"), "u1", own.at_ms, now + 300_000).await.unwrap();
    super::state_machine::write_owned_kv(ctx, &marker(&genuine, "provider"), own.to_bytes());
    let mut theirs = own.clone();
    theirs.node = "127.0.0.1:1".into();
    theirs.door = "gateway".into();
    theirs.sign_with(&other);
    super::state_machine::write_owned_kv(ctx, &marker(&genuine, "gateway"), theirs.to_bytes());
    let mut fake = own.clone();
    fake.door = "relay".into();
    fake.sign_with(&other);
    super::state_machine::write_owned_kv(ctx, &marker(&genuine, "relay"), fake.to_bytes());
    let held = |k: &str| n.kv(k).is_some();
    let retention = ctx.config.grant_retention_secs * 1_000;
    let floor = now + retention;
    assert!(floor > now + 300_000 + 2 * SKEW_MS, "the retention, not the window, is the later bound at the default");
    assert_eq!(call_issuer::collect(ctx, floor).grant_records, 0, "kept for its retention");
    let marker_floor = own.at_ms + retention + 2 * SKEW_MS;
    let past = floor.max(marker_floor) + 1;
    assert_eq!(call_issuer::collect(ctx, past).grant_records, 3, "its issued, its checkpoint and its own marker");
    assert!(!held(&format!("{genuine}/issued")) && !held(&format!("{genuine}/checkpoint")) && !held(&marker(&genuine, "provider")));
    for k in [genuine.clone(), format!("{genuine}/consumed"), format!("{genuine}/revoked"), marker(&genuine, "gateway"), marker(&genuine, "relay")] {
        assert!(held(&k), "{k}: another agent's, unsigned, or forged — left alone");
    }
    for k in [forged.clone(), format!("{forged}/issued"), format!("{forged}/checkpoint")] {
        assert!(held(&k), "{k}: a grant this node did not issue");
    }
    assert_eq!(call_issuer::collect(ctx, u64::MAX / 2).grant_records, 0, "and never collected here");
    // The holder's status route answers the request it kept: expired, not unknown.
    let resp = reqwest::Client::new().get(format!("http://127.0.0.1:{}/gateway/grants/request/r1", n.http_port)).bearer_auth("alice-secret").send().await.unwrap();
    let status = resp.status().as_u16();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!((status, v["state"].as_str()), (200, Some("revoked")), "the unsigned revoked record still answers: {v}");
    mycelium_core::ops::kv_delete(ctx, Arc::from(format!("{genuine}/revoked").as_str()));
    let resp = reqwest::Client::new().get(format!("http://127.0.0.1:{}/gateway/grants/request/r1", n.http_port)).bearer_auth("alice-secret").send().await.unwrap();
    let status = resp.status().as_u16();
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!((status, v["state"].as_str()), (200, Some("expired")), "{v}");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// **Exit 13, the issuer down.** With `call_mandate_ttl_secs = 2`, a client's call to `p`'s 30 s tool is admitted and
/// running when the gateway is stopped: no cancel can come, and `p`'s sweep ends the work at the mandate's window — 2
/// s after the mint — not at the tool's 30 s; the stop record names the term. Seen failing first: with the mint
/// ignoring `call_mandate_ttl_secs` (a 300 s window), the tool ran on past 20 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_issuer_down_a_running_call_ends_at_its_mandates_window() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ttl_secs: Some(2), ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let minted_at = std::time::Instant::now();
    let mut answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running_or_answered(1, &mut answered).await;
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().unwrap();
    g.a.shutdown().await;
    p.running(0).await;
    let ended = minted_at.elapsed();
    assert!(ended < Duration::from_secs(6) && ended >= Duration::from_millis(1_500), "ended at the 2 s window, not the tool's 30 s: {ended:?}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the tool never finished");
    assert_eq!(p.authority.named_stop_records().last().and_then(|s| s.term.clone()), Some(term));
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 13 at the default window** (300 s), under a clock the test moves: the minted mandate's window is
/// `call_mandate_ttl_secs` = 300 s after the mint, and a sweep at one millisecond past it stops the running call — the
/// sweep's own clock argument stands in for the sim clock, so the test does not wait five minutes. Seen failing first:
/// with the default at 60 s, the window was 60 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_the_default_window_a_running_call_ends_300_s_after_its_mint() {
    let n = Arc::new(node(Opts { mint: true, tools: true, ..Opts::default() }).await);
    let me = n.a.node_id().clone();
    let (framed, term) = n.framed(&me, "slow").await;
    let win = call_window_key(&n.gateway(), &term);
    let w = call_issuer::verified_window(&win, &n.kv(&win).unwrap(), &call_issuer::record_keys(n.ctx())).unwrap();
    let cp = call_issuer::verified_checkpoint(&call_checkpoint_key(&n.gateway(), &term), &n.kv(&call_checkpoint_key(&n.gateway(), &term)).unwrap(), &call_issuer::record_keys(n.ctx())).unwrap();
    assert_eq!(w.valid_until_ms - cp.issued_at_ms, 300_000, "the default window is 300 s from the mint");
    let (n2, me2) = (Arc::clone(&n), me.clone());
    let mut answered = tokio::spawn(async move { n2.send(&me2, framed).await });
    n.running_or_answered(1, &mut answered).await;
    n.authority.sweep(w.valid_until_ms - 1);
    assert_eq!(n.authority.running_count(), 1, "inside the window it runs");
    n.authority.sweep(w.valid_until_ms + 1);
    let reply = answered.await.unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// **Exit 18's cap refusal.** A call whose envelope fits the 8 KiB cap without its `call:` mandate and not with it is
/// refused by the producer **by name** (`caller_context_too_large`) — never truncated — and nothing the mint made is
/// written: no `call/` record, nothing held as issued, no mint counted. Seen failing first: with `record` called
/// before the frame's size was checked, the refused call left its two records behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_envelope_past_the_cap_with_its_call_mandate_is_refused_by_name_and_writes_nothing() {
    let n = node(Opts { mint: true, tools: true, ..Opts::default() }).await;
    let me = n.a.node_id().clone();
    let payload = Bytes::from(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "quick", "arguments": {}}}).to_string());
    // A presented mandate padded so that the envelope without `cm` is just under the cap.
    let size = |pad: usize, mint: bool| {
        let m = serde_json::json!({ "pad": "x".repeat(pad) });
        let framed = if mint {
            frame_minting(n.ctx(), &me, &n.gateway(), &[], MCP, payload.clone(), Some(&m), None, None, None, n.now()).0
        } else {
            super::gateway_caller::frame_envelope_at(n.ctx(), &me, &n.gateway(), &[], payload.clone(), Some(&m), None, None, None, n.now())
        };
        framed.map(|f| f.len() - payload.len())
    };
    let base = size(0, false).unwrap();
    let pad = super::gateway_caller::MAX_ENVELOPE_BYTES - base - 200;
    assert!(size(pad, false).is_some(), "fits without the call: mandate");
    assert!(size(pad, true).is_none(), "and not with it");
    let caller = super::gateway_caller::ResolvedPrincipal { principal: "token:gw/alice".into(), scopes: vec!["mcp:invoke".into()], subject_token: None };
    let m = serde_json::json!({ "pad": "x".repeat(pad) });
    let refused = super::gateway_caller::gateway_rpc_call_with_mandate(n.ctx(), Some(&caller), me, Arc::from(MCP), payload, Duration::from_secs(5), Some(&m), None, None).await;
    assert_eq!(refused.as_ref().map_err(|e| e.reason()).err(), Some("caller_context_too_large"), "{refused:?}");
    assert!(n.call_keys().is_empty(), "no record written");
    assert!(n.ctx().call_issuer.issued_terms().is_empty());
    assert_eq!(n.ctx().call_issuer.mints.load(Ordering::Relaxed), 0);
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}


/// rev794 K1 — **a revoke the disk refused, then a member's write at the call's key: the re-assertion puts the revoking
/// checkpoint in the KV without an fsync, and the next revoke answers `Ok` — acknowledged — while the disk is still
/// gone.** `revoke_call`'s "already on record" shortcut reads the KV, and `reassert` writes through `write_owned_kv`
/// (`append_try`, no sync). The reviewer's test, seen failing on 8e56186a (`Ok(false)`); fixed by `Issued::durable_seq`:
/// only this process's own durable write lets a revoke answer that the checkpoint is on record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rev794_a_revoke_is_not_acknowledged_while_the_disk_is_gone_whatever_the_re_assertion_wrote() {
    let n = node(Opts { mint: true, persist: true, tools: true, ..Opts::default() }).await;
    let me = n.a.node_id().clone();
    let (_framed, term) = n.framed(&me, "quick").await;
    let cp = call_checkpoint_key(&n.gateway(), &term);
    n.ctx().wal.get().expect("persists").poison_for_test("the disk is gone").await.unwrap();
    assert!(matches!(call_issuer::revoke_call(n.ctx(), &term).await, Err(call_issuer::RevokeRefusal::NotDurable(_))), "the first revoke is refused");
    // A member writes at the key (anything): the gateway's watcher re-asserts what it holds — the revoking checkpoint.
    let before = n.ctx().call_issuer.reasserted.load(Ordering::Relaxed);
    let member = NodeId::new("127.0.0.1", 1).unwrap();
    let junk = crate::framing::make_gossip_update_stamped(&member, 5, Arc::from(cp.as_str()), Bytes::from_static(b"junk"), false, crate::hlc::pack(n.now() + 10, 0));
    crate::store::apply_and_notify(&n.ctx().kv_state, &junk);
    assert!(until(|| n.ctx().call_issuer.reasserted.load(Ordering::Relaxed) > before, Duration::from_secs(5)).await, "re-asserted");
    let again = call_issuer::revoke_call(n.ctx(), &term).await;
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
    assert!(matches!(again, Err(call_issuer::RevokeRefusal::NotDurable(_))), "the disk is still gone, so the revoke is not acknowledged: {again:?}");
}

/// **The collector runs only where it is turned on, and the mint implies its own `call/` half** (#794's review, K3;
/// round 2, C3). A node with neither switch starts none. A minting gateway with `collect_expired_records` off starts
/// one and collects its own `call/` records — its promise about its own records — and nothing of `grant/`, not even
/// a grant it issued. With the switch on, a node starts one, once. Seen failing first: with `start_collector`
/// ignoring the settings the default node started one; with the mint not implying the collector the minting
/// gateway's records stayed for ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_collector_runs_only_where_it_is_turned_on() {
    let off = node(Opts::default()).await;
    assert!(!call_issuer::start_collector(off.ctx()), "neither switch: not started");
    assert!(!off.ctx().call_issuer.collecting(), "no collector runs at a default node");
    off.a.shutdown().await;
    // The mint alone: its own `call/` records, and no `grant/` record.
    let mint = node(Opts { mint: true, tools: true, ..Opts::default() }).await;
    assert!(mint.ctx().call_issuer.collecting(), "a minting gateway collects its own call/ records");
    let me = mint.a.node_id().clone();
    let (_framed, term) = mint.framed(&me, "quick").await;
    let issued = format!("grant/{}/r9/issued", grant_holder_segment("token:gw/alice"));
    let m = Mandate {
        holder: PrincipalId::new("token:gw/alice").unwrap(),
        established_by: PrincipalId::new(mint.gateway()).unwrap(),
        purpose: "grant".into(),
        scope: "grant:r9".into(),
        operations: vec!["tools/call:tool:deploy@x".into()],
        epoch: 1,
        term: TermId::new("r9").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: 1,
    };
    let key = mint.ctx().tls.get().unwrap().signing_key();
    let g = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&key, &m.canonical_bytes()).to_vec(), mandate: m };
    super::state_machine::write_owned_kv(mint.ctx(), &issued, serde_json::to_vec(&g).unwrap());
    let out = call_issuer::collect(mint.ctx(), u64::MAX / 2);
    assert_eq!((out.call_records, out.grant_records), (2, 0), "its own call/ pair, and no grant/ record");
    assert!(mint.kv(&call_checkpoint_key(&mint.gateway(), &term)).is_none() && mint.kv(&issued).is_some());
    mint.a.shutdown().await;
    let on = node(Opts { collect: true, ..Opts::default() }).await;
    assert!(on.ctx().call_issuer.collecting(), "on: started at the node's start");
    assert!(!call_issuer::start_collector(on.ctx()), "and only once");
    on.a.shutdown().await;
}

/// **A door keeps its marker until the grant's own window ends, whatever the retention** (#794's round-2 review, C2).
/// `GrantVerifier` caps no `grant:` window, so an entitled issuer can issue a two-day grant; with
/// `grant_retention_secs = 3600` the door's marker was collected after an hour — the journal had forgotten the use,
/// the tombstone would be collected, and the grant could be used again inside its window. Now the door's own record
/// of the use carries the grant's window, and the marker is kept to its end + 2·skew. Seen failing first: with the
/// floor at the use + the retention + 2·skew alone, the marker went an hour after the use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_doors_marker_is_kept_to_its_grants_window_whatever_the_retention() {
    let n = node(Opts { collect: true, retention_secs: Some(3_600), ..Opts::default() }).await;
    let ctx = n.ctx();
    let marker = format!("grant/{}/r2d/used/provider", grant_holder_segment("token:gw/alice"));
    let until = n.now() + 2 * 24 * 3_600_000;
    let rec = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "u2", until).unwrap();
    ctx.grant_uses.record_use(&marker, "u2", rec.at_ms, until).await.unwrap();
    super::state_machine::write_owned_kv(ctx, &marker, rec.to_bytes());
    let after_retention = rec.at_ms + 3_600_000 + 2 * SKEW_MS + 1;
    assert_eq!(call_issuer::collect(ctx, after_retention).grant_records, 0, "kept past the retention, inside the window");
    assert_eq!(call_issuer::collect(ctx, until + 2 * SKEW_MS).grant_records, 0, "kept to the window's end + 2·skew");
    assert_eq!(call_issuer::collect(ctx, until + 2 * SKEW_MS + 1).grant_records, 1, "and collected past it");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// **A genuine grant another member issued is not this node's to delete** (#794's round-2 review, C4). `g` holds, in
/// its KV, a grant validly signed by `p` and issued by `p`; with `g`'s collector on and far past every floor, `g`
/// deletes none of it. Seen failing first: with the `issuer == me` check removed, `g` deleted `p`'s grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_genuine_grant_another_member_issued_is_not_collected_here() {
    let (g, p, cert_dir) = pair(Opts { collect: true, ..Opts::default() }).await;
    let theirs = p.ctx().tls.get().unwrap().signing_key();
    let m = Mandate {
        holder: PrincipalId::new("token:gw/alice").unwrap(),
        established_by: PrincipalId::new(p.gateway()).unwrap(),
        purpose: "grant".into(),
        scope: "grant:rp".into(),
        operations: vec!["tools/call:tool:deploy@x".into()],
        epoch: 1,
        term: TermId::new("rp").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: 1,
    };
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&theirs, &m.canonical_bytes()).to_vec(), mandate: m };
    let base = format!("grant/{}/rp", grant_holder_segment("token:gw/alice"));
    let cp = serde_json::to_vec(&super::gateway_authority::call_mint::checkpoint(&theirs, &p.gateway(), "grant:rp", 1, 0, &[])).unwrap();
    super::state_machine::write_owned_kv(g.ctx(), &format!("{base}/issued"), serde_json::to_vec(&grant).unwrap());
    super::state_machine::write_owned_kv(g.ctx(), &format!("{base}/checkpoint"), cp);
    let members = call_issuer::record_keys(g.ctx());
    assert!(members.get(p.a.node_id()).is_some_and(|k| k.retained.contains(&theirs.verifying_key().to_bytes())), "g verifies p's signature");
    assert_eq!(call_issuer::collect(g.ctx(), u64::MAX / 2).grant_records, 0, "p's grant is p's to collect");
    assert!(g.kv(&format!("{base}/issued")).is_some() && g.kv(&format!("{base}/checkpoint")).is_some());
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A provider never advertises work before it has published that it enforces the caller context** (#794's review,
/// K2; the Eve fleet job on #796, -32021 `provider_without_caller_context`). The marker came only from a task in
/// `start()` that waits for a peer (up to 5 s), while a tool's entry and a capability's were written at once, so a
/// gateway could see the tool first and refuse a client's call. Now every advertisement publishes the marker first:
/// on a node with no peer yet, the marker is in its store the moment a tool is registered or a capability
/// advertised, and stamped no later than the tool's entry. Seen failing first: with the publish taken out of the tool
/// registration, the tool's entry was there and the marker was not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_publishes_its_caller_context_marker_before_it_advertises_work() {
    let n = node(Opts::default()).await;
    let ctx = n.ctx();
    let marker = super::gateway_caller::marker_key(n.a.node_id());
    assert!(n.kv(&marker).is_none(), "no peer yet: the lazy marker task is still waiting");
    let _tool = n.a.mcp().register_mcp_tool("fresh", serde_json::json!({"type": "object"}), |_| async { Ok(serde_json::json!("ok")) });
    let tool_key = format!("tools/fresh/{}", n.a.node_id());
    let stamp = |k: &str| ctx.kv_state.store.pin().get(k).map(|e| e.timestamp);
    assert!(n.kv(&tool_key).is_some(), "the tool is advertised");
    assert!(n.kv(&marker).is_some(), "and the marker is already published");
    assert!(stamp(&marker) <= stamp(&tool_key), "written before the tool's entry");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
    let n = node(Opts::default()).await;
    let _cap = n.a.capabilities().advertise_capability(crate::capability::Capability::new("depot", "fresh"), Duration::from_secs(60));
    assert!(n.kv(&super::gateway_caller::marker_key(n.a.node_id())).is_some(), "a capability's advertisement publishes it too");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// rev794 round 3, N1 — **at a door without a journal, a marker whose use a later use pruned is never collected.**
/// No `[persistence]` (the test node's default) and the default 7-day retention. The door uses grant `rA` and writes
/// its marker; 1 025 uses of other grants, past the first use's in-memory hold, prune `rA`'s use from the ledger
/// (`GrantUseLedger::record_use`'s `retain`). Far past the retention and `rA`'s window, the collector found no use held
/// and no journal and kept the marker — at every later collection too. The reviewer's test, seen failing on
/// `62467e7c` (`(0, true)`); fixed by the grant's window in the door's own signed marker body (`UseRecord::until_ms`),
/// which the collector reads without the ledger. A marker without it (a pre-release build's) is kept and counted:
/// `a_marker_without_its_grants_window_is_kept_and_counted`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rev794c_a_journal_less_doors_marker_is_collected_after_a_later_use_prunes_its_use() {
    let n = node(Opts { collect: true, ..Opts::default() }).await;
    let ctx = n.ctx();
    let seg = grant_holder_segment("token:gw/alice");
    let marker = format!("grant/{seg}/rA/used/provider");
    let rec = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "uA", n.now() + 300_000).unwrap();
    ctx.grant_uses.record_use(&marker, "uA", rec.at_ms, rec.until_ms).await.unwrap();
    super::state_machine::write_owned_kv(ctx, &marker, rec.to_bytes());
    // The ledger prunes when it holds its threshold (1 024 uses): a busy door's later uses.
    let then = rec.at_ms + 3_721_000;
    for i in 0..1_025 {
        let later = format!("grant/{seg}/rB{i}/used/provider");
        ctx.grant_uses.record_use(&later, &format!("uB{i}"), then, then + 300_000).await.unwrap();
    }
    assert!(ctx.grant_uses.own(&marker).is_none(), "the later use pruned the first from memory");
    let past = rec.at_ms + ctx.config.grant_retention_secs * 1_000 + 2 * SKEW_MS + 1;
    let collected = call_issuer::collect(ctx, past).grant_records;
    let held = n.kv(&marker).is_some();
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
    assert_eq!((collected, held), (1, false), "this door's own marker, past its retention and its grant's window, is collected");
}

/// **A door's own marker with no grant window in its signed body is kept and counted** (#794's round-3 review, D3): such
/// a body comes only from a build before `UseRecord::until_ms`, never released, and the door cannot tell how long its
/// grant lasts, so it keeps the marker (`grant_markers_without_window` on `/stats`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_without_its_grants_window_is_kept_and_counted() {
    let n = node(Opts { collect: true, ..Opts::default() }).await;
    let ctx = n.ctx();
    let marker = format!("grant/{}/rC/used/provider", grant_holder_segment("token:gw/alice"));
    let rec = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "uC", 0).unwrap();
    super::state_machine::write_owned_kv(ctx, &marker, rec.to_bytes());
    assert_eq!(call_issuer::collect(ctx, u64::MAX / 2).grant_records, 0, "kept");
    assert!(n.kv(&marker).is_some());
    assert_eq!(ctx.call_issuer.markers_without_window.load(Ordering::Relaxed), 1, "counted");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}

/// rev794 round 4, T1 — **a door's marker cannot lose, gain or change its grant window and still read as its own.**
/// A windowed marker with `until_ms` stripped (re-encoded without the field), one with the window moved, and a
/// window-less marker given a window all fail `own_record_at`: the window's presence and value are signed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rev794d_a_markers_window_is_signed_and_cannot_be_stripped_or_moved() {
    let n = node(Opts::default()).await;
    let ctx = n.ctx();
    let marker = format!("grant/{}/rW/used/provider", grant_holder_segment("token:gw/alice"));
    let until = n.now() + 2 * 24 * 3_600_000;
    let rec = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "uW", until).unwrap();
    assert_eq!(super::grant_use::own_record_at(ctx, &marker, &rec.to_bytes()), Some((rec.at_ms, until)), "the genuine marker is this door's");
    let mut stripped = rec.clone();
    stripped.until_ms = 0;
    let mut moved = rec.clone();
    moved.until_ms = until - 1;
    let plain = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "uP", 0).unwrap();
    let mut given = plain.clone();
    given.until_ms = until;
    let verdicts = [stripped, moved, given].map(|r| super::grant_use::own_record_at(ctx, &marker, &r.to_bytes()));
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
    assert_eq!(verdicts, [None, None, None], "stripped, moved and added windows are not this door's signed body");
}

/// **A release carries its use's window by construction** (#794's round-4 review, E4): `release_use` takes the window
/// from the held marker's own signed body, not from the ledger. Here the ledger holds no record of the use (a door
/// without a journal whose busy ledger pruned it, say), and the release still names the marker's window, so the
/// door's collector keeps the release as long as the use. Seen failing first: with the window taken from the ledger,
/// the release carried none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_carries_its_uses_window_from_the_held_marker() {
    let n = node(Opts::default()).await;
    let ctx = n.ctx();
    let marker = format!("grant/{}/rR/used/provider", grant_holder_segment("token:gw/alice"));
    let until = n.now() + 2 * 24 * 3_600_000;
    let rec = super::grant_use::UseRecord::signed(ctx, super::grant_use::UseKind::Use, &marker, "uR", until).unwrap();
    super::state_machine::write_owned_kv(ctx, &marker, rec.to_bytes());
    assert!(ctx.grant_uses.own(&marker).is_none(), "the ledger holds no record of the use");
    assert!(super::grant_use::release_use(ctx, &marker, "uR").await, "released");
    let held: super::grant_use::UseRecord = serde_json::from_slice(&n.kv(&marker).unwrap()).unwrap();
    assert_eq!((held.kind, held.until_ms), (super::grant_use::UseKind::Release, until), "the release names the use's window");
    assert_eq!(super::grant_use::own_record_at(ctx, &marker, &n.kv(&marker).unwrap()).map(|r| r.1), Some(until), "and is this door's own, signed");
    n.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&n.dir);
}
