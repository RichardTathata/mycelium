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

const MCP: &str = "mcp.invoke";
/// The gate's freshness window *F*: the sweep runs every *F* / 20 = 100 ms.
const F_MS: u64 = 2_000;
/// The skew of the gate's clock model: D-5's floor counts it twice.
const SKEW_MS: u64 = 100;

#[derive(Clone, Default)]
struct Opts {
    /// `gateway_mint_call_mandates`.
    mint: bool,
    /// `call_mandate_ttl_secs`, when not the default.
    ttl_secs: Option<u64>,
    /// A certificate directory shared with the other node, and the peers to bootstrap to.
    cert_dir: Option<std::path::PathBuf>,
    bootstrap: Vec<NodeId>,
    /// Fixed ports and directory: a restart over the same identity.
    gossip_port: Option<u16>,
    http_port: Option<u16>,
    /// `[persistence]` under the node's directory.
    persist: bool,
    /// This node serves the tools (`slow`, 30 s; `quick`, at once).
    tools: bool,
}

struct N {
    a: Arc<GossipAgent>,
    authority: Arc<ExecutionAuthority>,
    dir: std::path::PathBuf,
    http_port: u16,
    started: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

/// One node: a gateway (named token `alice`, `mcp:invoke`), an enforcing provider, an execution authority with
/// *F* = 2 s, and a policy under which **every tool call is cancellable**.
async fn node(opts: Opts) -> N {
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
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: opts.cert_dir.clone().unwrap_or_else(|| dir.join("tls")), ..Default::default() });
    if opts.persist {
        use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
        cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    }
    cfg.gateway_mint_call_mandates = opts.mint;
    if let Some(t) = opts.ttl_secs {
        cfg.call_mandate_ttl_secs = t;
    }
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![crate::config::GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] }];
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(ReferenceEvaluator::new("rev-mint").with_catalogue("cat-mint", "1").map_action("tools/call", "*").allow(Rule::new("*", "tools/call", "*").cancellable())));
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, FreshnessPolicy { freshness_ms: F_MS, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(EntitlementTable::new()), TrustedExternalIssuers::new()));
    a.with_execution_authority(Arc::clone(&authority));
    a.with_provider_enforcement();
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
    assert!(until(|| std::net::TcpStream::connect(("127.0.0.1", http_port)).is_ok(), Duration::from_secs(20)).await, "the gateway is listening");
    N { a, authority, dir, http_port, started, finished }
}

impl N {
    fn ctx(&self) -> &Arc<super::TaskCtx> {
        &self.a.task_ctx
    }
    fn gateway(&self) -> String {
        node_principal(self.a.node_id())
    }
    fn now(&self) -> u64 {
        self.ctx().hlc.decision_now_ms()
    }
    fn kv(&self, key: &str) -> Option<Bytes> {
        mycelium_core::ops::kv_get(self.ctx(), key)
    }
    fn call_keys(&self) -> Vec<String> {
        crate::store::scan_kv_prefix(&self.ctx().kv_state, "call/").into_iter().map(|(k, _)| k.to_string()).collect()
    }
    async fn running(&self, n: usize) {
        assert!(until(|| self.authority.running_count() == n, Duration::from_secs(20)).await, "running_count {} (wanted {n})", self.authority.running_count());
    }
    async fn began(&self, k: usize) {
        assert!(until(|| self.started.load(Ordering::SeqCst) == k, Duration::from_secs(20)).await, "started {}", self.started.load(Ordering::SeqCst));
    }
    /// A client's `tools/call` through this node's `/mcp`.
    async fn client_call(&self, tool: &str) -> serde_json::Value {
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {"aisle": 4}}});
        let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/mcp", self.http_port)).bearer_auth("alice-secret").json(&body).send().await.expect("the gateway answers");
        let status = resp.status().as_u16();
        let mut v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        v["http_status"] = serde_json::json!(status);
        v
    }
    /// Frame a member's own `tool` call to `target` as the gateway's dispatch frames it — the real mint, its records
    /// written — and return the frame and its term.
    async fn framed(&self, target: &NodeId, tool: &str) -> (Bytes, String) {
        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {"aisle": 4}}}).to_string();
        let (framed, minted) = frame_minting(self.ctx(), target, &self.gateway(), &[], MCP, Bytes::from(payload), None, None, None, None, self.now());
        let minted = minted.expect("minted");
        let term = minted.cm["grant"]["mandate"]["term"].as_str().unwrap().to_string();
        call_issuer::record(self.ctx(), minted).await.expect("the records are written");
        (framed.expect("fits"), term)
    }
    async fn send(&self, target: &NodeId, framed: Bytes) -> serde_json::Value {
        let reply = super::rpc::rpc_call_framed(self.ctx(), target.clone(), Arc::from(MCP), framed, Duration::from_secs(60)).await.expect("a reply");
        serde_json::from_slice(&reply).unwrap()
    }
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
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
/// other's key and `g` sees `p`'s tool.
async fn pair(gopts: Opts) -> (N, N, std::path::PathBuf) {
    let cert_dir = std::env::temp_dir().join(format!("i4-mint-ca-{}", crate::test_util::alloc_port()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let g = node(Opts { cert_dir: Some(cert_dir.clone()), ..gopts }).await;
    let p = node(Opts { cert_dir: Some(cert_dir.clone()), bootstrap: vec![g.a.node_id().clone()], tools: true, ..Opts::default() }).await;
    let knows = |x: &GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    assert!(until(|| knows(&g.a, p.a.node_id()) && knows(&p.a, g.a.node_id()), Duration::from_secs(20)).await, "the nodes learned each other's keys");
    let tool = format!("tools/slow/{}", p.a.node_id());
    assert!(until(|| g.kv(&tool).is_some(), Duration::from_secs(20)).await, "the gateway sees the provider's tool");
    (g, p, cert_dir)
}

/// **Exits 3 (its positive half) and 4, end to end over two nodes.** A client's call through `g`'s `/mcp` to `p`'s
/// 30 s tool, under a cancellable rule: `g` mints the call's `call:` mandate (its term `c{n}` of the envelope), writes
/// its checkpoint and window record (which reach `p`), and `p` admits the call and registers its work under it. The
/// gateway's cancel (`revoke_call`) reaches `p` by gossip, `p`'s feeder reads it for the window it holds, and its sweep
/// stops the call **within gossip + *F* / 20 + the drain bound, asserted against the configured *F* (2 s)**; the
/// client is answered `cancelled`, the tool did not finish, and `p`'s stop record names the term. Seen failing first:
/// with `mints_for` answering `false`, the call was never registered at the provider (refused `no_call_mandate`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clients_call_through_the_gateway_is_minted_admitted_and_stopped_by_a_cancel() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
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
    // The term is known again from the gateway's own verified records, so a cancel after the restart answers it.
    assert_eq!(call_issuer::revoke_call(n.ctx(), &term).await, Ok(false), "already revoked, read back from the KV");
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
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
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
    assert_eq!(n.ctx().call_issuer.reasserted.load(Ordering::Relaxed), 2);

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
    assert_eq!(call_issuer::collect(n.ctx(), fallback + 1).call_records, 2, "and collected past it");
    assert!(n.kv(&cp).is_none() && n.kv(&win).is_none());

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

/// **Exit 12, the `call/` family.** A minted call's two records are kept up to D-5's floor — the window's end +
/// 2·skew + `max_drain_secs` — and collected one millisecond past it, by the issuer with `kv_delete`; what the gateway
/// held as issued goes with them. **With the issuer stopped**, the provider that holds the records (by gossip)
/// collects them itself with `kv_delete_held`, at the same floor and not before. Seen failing first: with the floor
/// read without `max_drain_secs`, the records went at the window's end + 2·skew.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_calls_records_are_kept_to_their_floor_and_collected_past_it_by_the_issuer_or_any_other_node() {
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
    assert_eq!(call_issuer::collect(p.ctx(), floor).call_records, 0, "kept at the floor with the issuer gone");
    assert_eq!(call_issuer::collect(p.ctx(), floor + 1).call_records, 2, "collected past it by a node that is not the issuer");
    assert!(p.kv(&cp).is_none() && p.kv(&win).is_none());
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 12, the `grant/` family, with critique plan CR2's markers and releases.** A grant whose `issued` record
/// verifies is collected — the request, the grant, its checkpoint, `consumed`, the use marker and a release under
/// `used/` — past the later of its window's end + 2·skew and its issue + `grant_retention_secs`, and kept before.
/// A grant whose `issued` record is signed by another key is never collected; nor is a request with no `issued`
/// record (pending or refused: not built). Seen failing first: with the floor at the window's end + 2·skew only, the
/// grant's records went six days early.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grants_records_are_kept_for_their_retention_and_collected_past_it() {
    let n = node(Opts::default()).await;
    let key = n.ctx().tls.get().unwrap().signing_key();
    let other = ed25519_dalek::SigningKey::from_bytes(&[78u8; 32]);
    let holder = "token:gw/alice";
    let now = n.now();
    let write_grant = |request: &str, signer: &ed25519_dalek::SigningKey| {
        let m = Mandate {
            holder: PrincipalId::new(holder).unwrap(),
            established_by: PrincipalId::new(n.gateway()).unwrap(),
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
        for (k, v) in [
            (base.clone(), b"{\"request\":1}".to_vec()),
            (format!("{base}/issued"), serde_json::to_vec(&g).unwrap()),
            (format!("{base}/checkpoint"), b"{}".to_vec()),
            (format!("{base}/consumed"), b"{}".to_vec()),
            (format!("{base}/used/gateway"), b"{\"kind\":\"use\"}".to_vec()),
            (format!("{base}/used/provider"), b"{\"kind\":\"release\"}".to_vec()),
        ] {
            super::state_machine::write_owned_kv(n.ctx(), &k, v);
        }
        base
    };
    let genuine = write_grant("r1", &key);
    let forged = write_grant("r2", &other);
    let pending = format!("grant/{}/r3", grant_holder_segment(holder));
    super::state_machine::write_owned_kv(n.ctx(), &pending, b"{\"request\":3}".to_vec());
    // A request id that is a prefix of another's is its own grant: `r1`'s collection leaves `r10` alone.
    let neighbour = format!("grant/{}/r10", grant_holder_segment(holder));
    super::state_machine::write_owned_kv(n.ctx(), &neighbour, b"{\"request\":10}".to_vec());
    let held = |base: &str| crate::store::scan_kv_prefix(&n.ctx().kv_state, base).into_iter().filter(|(k, _)| k.as_ref() == base || k.starts_with(&format!("{base}/"))).count();
    assert_eq!((held(&genuine), held(&forged)), (6, 6));
    let retention = n.ctx().config.grant_retention_secs * 1_000;
    let floor = now + retention;
    assert!(floor > now + 300_000 + 2 * SKEW_MS, "the retention, not the window, is the later bound at the default");
    assert_eq!(call_issuer::collect(n.ctx(), floor).grant_records, 0, "kept for its retention");
    assert_eq!(call_issuer::collect(n.ctx(), floor + 1).grant_records, 6, "collected past it, markers and release with it");
    assert_eq!((held(&genuine), held(&forged), held(&pending), held(&neighbour)), (0, 6, 1, 1), "the forged grant, the pending request and the neighbour stay");
    assert_eq!(call_issuer::collect(n.ctx(), u64::MAX / 2).grant_records, 0, "and are never collected here");
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
    let _answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
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
    let answered = tokio::spawn(async move { n2.send(&me2, framed).await });
    n.running(1).await;
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
