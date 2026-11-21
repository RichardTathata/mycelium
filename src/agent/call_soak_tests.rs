//! Identity-and-lifecycle I4, PR 4: **the soak** (exit 14) and **the measurements** (exit 18, and the memory per held
//! window PR 1a left to this PR).
//!
//! **The soak.** Two nodes under one CA — a gateway that mints (`gateway_mint_call_mandates`) and an enforcing
//! provider — and `n` cancellable calls in batches, **half of them cancelled while running**. Three lanes, by the
//! call's index modulo 10, the odd ones cancelled:
//!
//! - **the A2A door** (0–5): `tasks/send` to a skill served by an `rpc_rx` receiver that declared it stops, cancelled
//!   by the caller's `tasks/cancel`;
//! - **the `/mcp` door** (6, 7): a client's `tools/call` to a registered tool, cancelled by the client's
//!   `notifications/cancelled` (the door's id → term table). A registered tool runs one call at a time, so each call
//!   in flight has a tool of its own;
//! - **both slots** (8, 9): the gateway's own dispatch (`gateway_rpc_call_with_mandate`, the one dispatch path of
//!   every gateway door) carrying a grant in slot 1 beside the minted `call:` mandate in slot 2, cancelled through
//!   `GossipAgent::cancel_call`. Each such call is **two registrations**.
//!
//! Every cancel revokes the term on disk, forwards the revocation to the provider (the cancel-forward) and leaves it
//! in the KV. In each batch every call is first seen running (its handler started), and only then is `running_count`
//! read: it is asserted as **registrations**, one per call and a second for each both-slots call. Then the odd calls
//! are cancelled and the even ones released. What the soak asserts is in [`soak`]'s doc comment.
//!
//! **The gateway's `[persistence]`.** The soak passes over a gateway without it and **fails** over one with it under
//! `SyncMode::Flush` (what `Soak::persist` sets), at the collector's minute pass: a cancel waits its turn at the WAL,
//! one sync per synced append. The finding, pinned at the end of this file by a test that fails today
//! (`a_cancel_is_on_disk_inside_bound_1_while_the_collector_deletes_a_backlog`). The CI run and the dated run are
//! therefore of a gateway with no `[persistence]`, and say so.
//!
//! **What it does not drive** (each has its own tests, none at this count): the SDK serve stream (`rpc/serve`), the
//! bridged MCP loop, the built-in `llm.invoke` door, a hosted capability, a member's own unframed call (not
//! cancellable, by decision), a cancel that travels only by the KV (the forward is not suppressed), a cancel that
//! arrives before the mint, a cancel by another principal, an OIDC caller, a restart of either node, and a real
//! network: both nodes are in one process on loopback.

use super::call_cancel::{self, CallSink, CALL_SINK};
use super::call_issuer::{self, call_window_key, CallWindow, SignedCallWindow};
use super::call_mint_tests::until;
use super::gateway_authority::{arguments_digest, call_checkpoint_key, call_mint, mandate_operation, possession_request, ExecutionAuthority, PresentedMandate};
use super::gateway_caller::{frame_minting, gateway_rpc_call_with_mandate, node_principal, ResolvedPrincipal, MAX_ENVELOPE_BYTES};
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant, CALL_TERM_MAX_BYTES};
use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use crate::{CancelClass, GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use bytes::Bytes;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const MCP: &str = "mcp.invoke";
const SKILL: &str = "skill.invoke";
/// The gate's freshness window *F*: the sweep runs every *F* / 20 = 100 ms. Bound 1 is asserted against it.
const F_MS: u64 = 2_000;
const SKEW_MS: u64 = 100;
const ALICE: &str = "token:gw/alice";

/// One soak's size and settings.
#[derive(Clone, Copy)]
struct Soak {
    /// How many calls, in all. Half are cancelled.
    n: usize,
    /// How many calls are in flight together. A multiple of 10, so each batch holds the lanes in their proportion.
    batch: usize,
    /// `call_mandate_ttl_secs`: every window ends this long after its mint, so it bounds what both nodes hold.
    ttl_secs: u64,
    /// The gateway's `[persistence]` and its `sync_mode`: with it, each mint's two records are fsynced before the call
    /// is sent, and each cancel's revocation before the cancel is acknowledged.
    persist: Option<crate::config::SyncMode>,
    /// The whole soak fails past this.
    within: Duration,
}

/// Is call `i` cancelled while it runs? Every odd one: half of any even count.
fn cancelled(i: usize) -> bool {
    i % 2 == 1
}

/// The door call `i` comes through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    /// `/a2a` `tasks/send`, cancelled by `tasks/cancel`.
    A2a,
    /// `/mcp` `tools/call`, cancelled by `notifications/cancelled`.
    McpDoor,
    /// The gateway's dispatch with a grant in slot 1 (two registrations), cancelled by `cancel_call`.
    Both,
}

fn lane(i: usize) -> Lane {
    match i % 10 {
        0..=5 => Lane::A2a,
        6 | 7 => Lane::McpDoor,
        _ => Lane::Both,
    }
}

/// The tool an `/mcp`-lane call runs on: a registered tool runs one call at a time, so each of a batch's has its own.
fn tool_of(i: usize, batch: usize) -> String {
    format!("soak-{}", (i % batch) / 10 * 2 + (i % 10 - 6))
}

/// One cancel, sent: when (the test's clock and the gateway's), `cancel_call`'s receipt if that is the door, and how
/// long the door took to answer.
type CancelSent = (Instant, u64, Option<CancelClass>, Duration);

/// What the provider's handlers record, by call index, and how the soak lets a call finish.
struct Tools {
    started: Vec<AtomicBool>,
    finished: Vec<AtomicBool>,
    release: Vec<tokio::sync::Notify>,
    /// The call's term as the **provider** read it from the envelope it verified (the skill lanes).
    term: Vec<OnceLock<String>>,
    /// A handler that was neither released nor stopped inside its own bound (90 s): the soak fails on any.
    overran: AtomicUsize,
}

impl Tools {
    fn new(n: usize) -> Arc<Self> {
        Arc::new(Self {
            started: (0..n).map(|_| AtomicBool::new(false)).collect(),
            finished: (0..n).map(|_| AtomicBool::new(false)).collect(),
            release: (0..n).map(|_| tokio::sync::Notify::new()).collect(),
            term: (0..n).map(|_| OnceLock::new()).collect(),
            overran: AtomicUsize::new(0),
        })
    }
    fn count(v: &[AtomicBool]) -> usize {
        v.iter().filter(|b| b.load(Ordering::SeqCst)).count()
    }
}

struct Node {
    a: Arc<GossipAgent>,
    authority: Arc<ExecutionAuthority>,
    dir: std::path::PathBuf,
    http_port: u16,
}

impl Drop for Node {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("i4-soak-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Node {
    fn ctx(&self) -> &Arc<super::TaskCtx> {
        &self.a.task_ctx
    }
    fn principal(&self) -> String {
        node_principal(self.a.node_id())
    }
    fn now(&self) -> u64 {
        self.ctx().hlc.decision_now_ms()
    }
    fn key(&self) -> Arc<ed25519_dalek::SigningKey> {
        self.ctx().tls.get().expect("a TLS identity").signing_key()
    }
    /// The live `call/` records in this node's KV.
    fn call_keys(&self) -> usize {
        crate::store::scan_kv_prefix(&self.ctx().kv_state, "call/").len()
    }
}

/// What differs between the nodes of a soak.
struct NodeOpts<'a> {
    cert_dir: &'a std::path::Path,
    bootstrap: Vec<NodeId>,
    /// The provider's id: both nodes' policies name its tools and skills.
    provider: &'a NodeId,
    /// The gateway's id: the provider entitles its grants (the both-slots lane's slot 1).
    gateway: &'a NodeId,
    mint: bool,
    ttl_secs: u64,
    persist: Option<crate::config::SyncMode>,
    /// How many `/mcp`-lane tools the policy names (and, with `tools`, the provider registers).
    mcp_tools: usize,
    tools: Option<Arc<Tools>>,
}

/// One node: a gateway (`/mcp` and `/a2a`; named token `alice`), an enforcing provider and an execution authority
/// with *F* = 2 s. The policy: the skill `depot/work` and every `soak-{k}` tool are **cancellable**; the skill
/// `depot/both` is cancellable **and needs a grant**.
async fn soak_node(port: u16, http_port: u16, o: NodeOpts<'_>) -> Node {
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("i4-soak-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.bootstrap_peers = o.bootstrap.clone();
    cfg.reconnect_backoff_secs = 1;
    cfg.tls = Some({
        let mut tls_cfg = crate::TlsConfig::default();
        tls_cfg.auto_cert_dir = o.cert_dir.to_path_buf();
        tls_cfg
    });
    if let Some(sync_mode) = o.persist {
        use crate::config::{OnUnreadable, PersistenceConfig};
        cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode, snapshot_wal_threshold: 1_000_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    }
    cfg.gateway_mint_call_mandates = o.mint;
    cfg.call_mandate_ttl_secs = o.ttl_secs;
    // The gateway's own records are collected one second past their window + 2·skew (D-5's floor).
    cfg.max_drain_secs = 1;
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![crate::config::GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] }];
    let a = Arc::new(GossipAgent::new(id.clone(), cfg).with_a2a());
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    let mut policy = ReferenceEvaluator::new("rev-soak")
        .with_catalogue("cat-soak", "1")
        .map_action("tools/call", "*")
        .map_action(SKILL, "*")
        .allow(Rule::new("*", SKILL, format!("skill:depot/work@{}", o.provider)).cancellable())
        .allow(Rule::new("*", SKILL, format!("skill:depot/both@{}", o.provider)).requiring_grant().cancellable());
    for k in 0..o.mcp_tools {
        policy = policy.allow(Rule::new("*", "tools/call", format!("tool:soak-{k}@{}", o.provider)).cancellable());
    }
    a.with_action_evaluator(Arc::new(policy));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(node_principal(o.gateway)).unwrap());
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, FreshnessPolicy { freshness_ms: F_MS, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()));
    a.with_execution_authority(Arc::clone(&authority));
    a.with_provider_enforcement();
    a.start().await.unwrap();
    if let Some(tools) = o.tools {
        // The `/mcp` lane's tools: each runs until it is released, and the node drops its future on a cancel.
        for k in 0..o.mcp_tools {
            let tools = Arc::clone(&tools);
            std::mem::forget(a.mcp().register_mcp_tool(format!("soak-{k}"), serde_json::json!({"type": "object"}), move |args: serde_json::Value| {
                let tools = Arc::clone(&tools);
                async move {
                    let i = args["i"].as_u64().ok_or("no index")? as usize;
                    tools.started[i].store(true, Ordering::SeqCst);
                    tokio::select! {
                        _ = tools.release[i].notified() => {}
                        _ = tokio::time::sleep(Duration::from_secs(90)) => { tools.overran.fetch_add(1, Ordering::SeqCst); }
                    }
                    tools.finished[i].store(true, Ordering::SeqCst);
                    Ok(serde_json::json!(format!("done:{i}")))
                }
            }));
        }
        // The skill lanes: one receiver that declared it stops, a task per call racing `authority_lapsed()`.
        for skill in ["work", "both"] {
            std::mem::forget(a.capabilities().advertise_capability(crate::capability::Capability::new("depot", skill), Duration::from_secs(7_200)));
        }
        let mut rx = a.service().rpc_rx(SKILL).stops_when_authority_lapses();
        let a2 = Arc::clone(&a);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let (a3, tools) = (Arc::clone(&a2), Arc::clone(&tools));
                tokio::spawn(async move {
                    let Ok(i) = String::from_utf8_lossy(&req.payload()).trim().parse::<usize>() else {
                        a3.service().rpc_respond(&req, b"no index".to_vec());
                        return;
                    };
                    // The term, as this provider reads it from the envelope it verified at its receive boundary.
                    if let Ok(Some(caller)) = super::gateway_caller::verify(&a3.task_ctx, &req)
                        && let Some(term) = caller.call_mandate.as_ref().and_then(|cm| cm["grant"]["mandate"]["term"].as_str())
                    {
                        let _ = tools.term[i].set(term.to_string());
                    }
                    tools.started[i].store(true, Ordering::SeqCst);
                    tokio::select! {
                        _ = tools.release[i].notified() => {
                            tools.finished[i].store(true, Ordering::SeqCst);
                            a3.service().rpc_respond(&req, format!("done:{i}").into_bytes());
                        }
                        _ = req.authority_lapsed() => a3.service().rpc_respond(&req, format!("stopped:{i}").into_bytes()),
                        _ = tokio::time::sleep(Duration::from_secs(90)) => {
                            tools.overran.fetch_add(1, Ordering::SeqCst);
                            a3.service().rpc_respond(&req, format!("overran:{i}").into_bytes());
                        }
                    }
                });
            }
        });
    }
    assert!(until(|| std::net::TcpStream::connect(("127.0.0.1", http_port)).is_ok(), Duration::from_secs(20)).await, "the gateway is listening");
    Node { a, authority, dir, http_port }
}

/// The soak's two nodes, ready: each holds the other's key, and the gateway sees every tool, both skills and the
/// provider's caller-context marker (#802).
/// A fleet's certificate directory, removed when the test lets go of it — on a failure too (#816's review: a failing
/// soak left its CA directory behind).
struct CertDir(std::path::PathBuf);

impl Drop for CertDir {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.0.starts_with(std::env::temp_dir()) && self.0.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("i4-soak-ca-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn fleet(s: &Soak, tools: Arc<Tools>) -> (Arc<Node>, Arc<Node>, CertDir) {
    let port = crate::test_util::alloc_port;
    let (gport, pport) = (port(), port());
    let (gid, pid) = (NodeId::new("127.0.0.1", gport).unwrap(), NodeId::new("127.0.0.1", pport).unwrap());
    let cert_dir = std::env::temp_dir().join(format!("i4-soak-ca-{gport}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let cert_dir = CertDir(cert_dir);
    let mcp_tools = s.batch / 5;
    let g = soak_node(gport, port(), NodeOpts { cert_dir: &cert_dir.0, bootstrap: vec![], provider: &pid, gateway: &gid, mint: true, ttl_secs: s.ttl_secs, persist: s.persist, mcp_tools, tools: None }).await;
    let p = soak_node(pport, port(), NodeOpts { cert_dir: &cert_dir.0, bootstrap: vec![gid.clone()], provider: &pid, gateway: &gid, mint: false, ttl_secs: s.ttl_secs, persist: None, mcp_tools, tools: Some(tools) }).await;
    let knows = |x: &GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    assert!(until(|| knows(&g.a, &pid) && knows(&p.a, &gid), Duration::from_secs(20)).await, "the nodes learned each other's keys");
    for k in 0..mcp_tools {
        let key = format!("tools/soak-{k}/{pid}");
        assert!(until(|| mycelium_core::ops::kv_get(g.ctx(), &key).is_some(), Duration::from_secs(20)).await, "the gateway sees the provider's tool soak-{k}");
    }
    for skill in ["work", "both"] {
        let seen = || crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot") && k.contains(skill));
        assert!(until(seen, Duration::from_secs(20)).await, "the gateway sees the skill depot/{skill}");
    }
    assert!(until(|| super::gateway_caller::gateway_sends_to(g.ctx(), &pid), Duration::from_secs(20)).await, "the gateway holds the provider's marker");
    (Arc::new(g), Arc::new(p), cert_dir)
}

/// A grant for one `depot/both` call, issued and held by the gateway node, its first checkpoint offered to the
/// provider's authority directly (in a fleet it reaches it through the `grant/` feeder, which this soak does not drive).
fn grant_for(g: &Node, p: &Node, resource: &str, arguments: &serde_json::Value, request: &str, until_ms: u64) -> serde_json::Value {
    let (gw, digest, key) = (PrincipalId::new(g.principal()).unwrap(), arguments_digest(arguments), g.key());
    let m = Mandate {
        holder: gw.clone(),
        established_by: gw,
        purpose: format!("grant {request}"),
        scope: format!("grant:{request}"),
        operations: Mandate::grant_operations(mandate_operation(SKILL, resource), &super::action_evaluator::hex32(&digest), None),
        epoch: 1,
        term: TermId::new(request).unwrap(),
        valid_from_ms: g.now(),
        valid_until_ms: until_ms,
    };
    let proof = mycelium_core::tls::sign_bytes(&key, &possession_message(&m, &possession_request(SKILL, resource, &digest)));
    let first = call_mint::checkpoint(&key, &g.principal(), &m.scope, 1, g.now(), &[]);
    let _ = p.authority.offer_checkpoint(&first, p.now(), &super::gateway_member_keys(p.ctx()));
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&key, &m.canonical_bytes()).to_vec(), mandate: m };
    serde_json::to_value(PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) }).unwrap()
}

/// The process's resident set, in KiB, as `ps` reports it. Both nodes are in this process.
fn rss_kib() -> Option<u64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn pct(sorted: &[Duration], p: usize) -> Duration {
    sorted[((sorted.len() * p) / 100).min(sorted.len() - 1)]
}

/// One request to the gateway's `path` as alice; the status and the body, or the transport's error.
async fn post(http: &reqwest::Client, url: &str, body: serde_json::Value) -> String {
    match http.post(url).bearer_auth("alice-secret").json(&body).send().await {
        Ok(r) => format!("{} {}", r.status().as_u16(), r.text().await.unwrap_or_default()),
        Err(e) => format!("transport: {e}"),
    }
}

/// The gateway's count of cancel-forward receipts by class (`GET /stats` `call_cancel_receipts`): one for each
/// cancel, whichever door sent it.
fn door_receipts(g: &Node) -> BTreeMap<&'static str, u64> {
    g.ctx().call_issuer.cancel_receipts.pin().iter().map(|(k, v)| (*k, *v)).collect()
}

/// What each node's transport dropped: frames to a peer in reconnect backoff (or oversized), and signals a handler's
/// full queue could not take — `(gateway frames, gateway signals, provider frames, provider signals)`. An RPC whose
/// request or reply is among them is a timeout at its caller, not an answer.
fn transport_drops(g: &Node, p: &Node) -> (u64, u64, u64, u64) {
    let of = |n: &Node| (n.a.system_stats().dropped_frames, n.ctx().signal_handlers.handler_drops());
    let ((gf, gs), (pf, ps)) = (of(g), of(p));
    (gf, gs, pf, ps)
}

/// For a failed batch: each node's runtime state as it reports it, and whether the gateway's HTTP door still answers.
async fn diagnose(g: &Node, p: &Node, http: &reqwest::Client) -> String {
    let of = |n: &Node| {
        let s = n.a.system_stats();
        format!("handler drops {}, {s:?}", n.ctx().signal_handlers.handler_drops())
    };
    let stats = tokio::time::timeout(Duration::from_secs(5), http.get(format!("http://127.0.0.1:{}/stats", g.http_port)).send()).await;
    let door = match stats {
        Ok(Ok(r)) => format!("answers {}", r.status().as_u16()),
        Ok(Err(e)) => format!("fails: {e}"),
        Err(_) => "does not answer in 5 s".to_string(),
    };
    format!("gateway: {}; provider: {}; the gateway's /stats {door}; receipts {:?}", of(g), of(p), door_receipts(g))
}

/// What a soak measured; every count in it was asserted before it is returned.
struct Report {
    /// Cancel sent → caller answered, by lane, sorted.
    cancel_to_answer: BTreeMap<&'static str, Vec<Duration>>,
    /// The cancelled calls' timelines in milliseconds of the nodes' decision clocks, by phase, sorted.
    phases: BTreeMap<&'static str, Vec<u64>>,
    /// The cancelled call the caller waited longest for, its timeline.
    worst: String,
    /// Each `cancel_call` whose forward the provider did not answer in time (`DeliveryUnknown`), with its timeline.
    unanswered: Vec<String>,
    /// Every cancel-forward's receipt as the gateway counted it, by class — the doors' forwards and `cancel_call`'s.
    forwards: BTreeMap<&'static str, u64>,
    /// The slowest batch in each phase, and the batch's first call.
    slowest: BTreeMap<&'static str, (Duration, usize)>,
    /// [`transport_drops`] at the end.
    drops: (u64, u64, u64, u64),
    took: Duration,
    receipts: BTreeMap<&'static str, usize>,
    peak_windows: usize,
    peak_issued: usize,
    rss: (Option<u64>, Option<u64>, Option<u64>),
    grant_scopes_left: usize,
    records_collected: usize,
    /// Each mint's two durable writes at the gateway, under the soak's load, sorted.
    mint_writes: Vec<Duration>,
}

/// **Exit 14: the soak.** `s.n` cancellable calls through a minting gateway to an enforcing provider, half of them
/// cancelled while running. Asserted, for every batch and at the end:
///
/// - **every call is registered before anything ends it**, and `running_count` is the **registrations**: one per
///   call, and a second for each both-slots call (the provider's); the gateway's own authority runs nothing;
/// - **every cancelled call ends inside bound 1**: from the gateway's revocation (the revoking checkpoint's
///   `issued_at_ms`) to the provider's **confirmed** stop (its stop record), asserted against the configured *F*
///   (2 s) — and its handler is stopped, never finishing. What the caller waits, from the cancel it sent to its
///   answer, is measured beside it and not asserted: it adds the door's own work, the gateway's fsync of the
///   revocation among it, which bound 1 does not count;
/// - **every other call ends by its own completion**: answered `done`, its handler finished, not stopped by a
///   neighbour's cancel;
/// - **no call ends without a stop record or its own completion**: the provider's stop records name exactly the
///   cancelled calls' terms (scope `call:{term}`, issued by the gateway) — one each, none for a completed call — and
///   `finished` counts exactly the others;
/// - **the counters reconcile**: one mint per call and none refused; no window, checkpoint or hold refused at the
///   provider; no cancel unmatched, no cancel-forward refused, the `/mcp` table neither full nor colliding, and empty
///   at the end; every cancel-forward's receipt, on every lane, a stop (`stopped`, `stop_requested`) or `delivery_unknown` — no answer
///   to the forward in its 5 s, which is not a negative: it is counted and printed with the call's timeline, and the
///   call must still have stopped inside bound 1;
/// - **both nodes' views return to zero after the last window**: the gateway's `(0, 0, 0)`; the provider's `call:`
///   windows 0 and its checkpoints and revoked sets down to the **`grant:` scopes of the both-slots lane, one per
///   such call** — an authority never forgets a `grant:` scope (it forgets only `call:` scopes, with their windows),
///   which this soak pins rather than hides; the gateway holds nothing as issued; and once the gateway's collector
///   has run past D-5's floor, neither node's KV holds a `call/` record.
async fn soak(s: Soak) -> Report {
    assert!(s.n.is_multiple_of(s.batch) && s.batch.is_multiple_of(10), "whole batches, each with the lanes in proportion");
    // For a failed soak: `I4_SOAK_TRACE=mycelium=warn,mycelium_core=warn` prints the nodes' own log beside the run.
    #[cfg(feature = "cli")]
    if let Ok(filter) = std::env::var("I4_SOAK_TRACE") {
        let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    }
    let began = Instant::now();
    let tools = Tools::new(s.n);
    let (g, p, _cert_dir) = fleet(&s, Arc::clone(&tools)).await;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(120)).build().unwrap();
    let (mcp_url, a2a_url) = (format!("http://127.0.0.1:{}/mcp", g.http_port), format!("http://127.0.0.1:{}/a2a", g.http_port));
    let rss_start = rss_kib();
    let (mut rss_peak, mut peak_windows, mut peak_issued) = (rss_start, 0usize, 0usize);
    let mut cancel_to_answer: BTreeMap<&'static str, Vec<Duration>> = BTreeMap::new();
    let mut receipts: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Every stop record the provider wrote, as `(scope, term)`: read after each batch (it keeps the newest 1 024).
    let mut stops: std::collections::HashMap<String, (String, crate::mandate::authority::StopRecord)> = Default::default();
    let mut phases: BTreeMap<&'static str, Vec<u64>> = BTreeMap::new();
    let mut worst: Option<(u64, String)> = None;
    // `cancel_call`s whose forward was not answered: the call, and how long the cancel took to say so.
    let mut unanswered: Vec<(usize, Duration)> = Vec::new();
    let mut unanswered_lines: Vec<String> = Vec::new();
    // The slowest batch in each phase, and which batch it was.
    let mut slowest: BTreeMap<&'static str, (Duration, usize)> = BTreeMap::new();
    let mut cancelled_terms: HashSet<String> = HashSet::new();
    let mut all_terms: HashSet<String> = HashSet::new();
    let mut last_until_ms = 0u64;

    for lo in (0..s.n).step_by(s.batch) {
        assert!(began.elapsed() < s.within, "the soak's own bound: {} calls in {:?}", lo, began.elapsed());
        let hi = lo + s.batch;
        let until_ms = g.now() + s.ttl_secs * 1_000;
        last_until_ms = until_ms + 1_000; // each mint draws its own `now`: a second covers the batch's launch
        let launched = Instant::now();
        let sinks: Vec<Arc<CallSink>> = (lo..hi).map(|_| Arc::new(CallSink::default())).collect();
        let mut calls: Vec<tokio::task::JoinHandle<(String, Instant, u64)>> = Vec::with_capacity(s.batch);
        for i in lo..hi {
            calls.push(match lane(i) {
                Lane::A2a => {
                    let (http, url, g) = (http.clone(), a2a_url.clone(), Arc::clone(&g));
                    tokio::spawn(async move {
                        let send = serde_json::json!({"id": format!("t-{i}"), "skillId": "depot/work", "message": {"role": "user", "parts": [{"type": "text", "text": i.to_string()}]}});
                        (post(&http, &url, serde_json::json!({"jsonrpc": "2.0", "id": i, "method": "tasks/send", "params": send})).await, Instant::now(), g.now())
                    })
                }
                Lane::McpDoor => {
                    let (http, url, tool, g) = (http.clone(), mcp_url.clone(), tool_of(i, s.batch), Arc::clone(&g));
                    tokio::spawn(async move {
                        let body = serde_json::json!({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {"name": tool, "arguments": {"i": i}}});
                        (post(&http, &url, body).await, Instant::now(), g.now())
                    })
                }
                Lane::Both => {
                    let (g, p, sink) = (Arc::clone(&g), Arc::clone(&p), Arc::clone(&sinks[i - lo]));
                    tokio::spawn(async move {
                        let resource = format!("skill:depot/both@{}", p.a.node_id());
                        let grant = grant_for(&g, &p, &resource, &serde_json::json!({ "text": i.to_string() }), &format!("s{i}"), until_ms);
                        let caller = ResolvedPrincipal { principal: g.principal(), scopes: vec![], subject_token: None };
                        let dispatch = gateway_rpc_call_with_mandate(g.ctx(), Some(&caller), p.a.node_id().clone(), Arc::from(SKILL), Bytes::from(i.to_string()), Duration::from_secs(120), Some(&grant), Some(&resource), None);
                        let out = CALL_SINK.scope(sink, dispatch).await;
                        (out.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|e| format!("dispatch refused: {e:?}")), Instant::now(), g.now())
                    })
                }
            });
        }

        // Every call of the batch is running: its handler started. A call answered before that is a failure, named.
        let all_started = || (lo..hi).all(|i| tools.started[i].load(Ordering::SeqCst));
        until(|| all_started() || calls.iter().any(|c| c.is_finished()), Duration::from_secs(60)).await;
        if !all_started() {
            let mut early = Vec::new();
            for (k, c) in calls.iter_mut().enumerate() {
                if c.is_finished() && early.len() < 3 {
                    early.push((lo + k, lane(lo + k), c.await.map(|a| a.0).unwrap_or_default()));
                }
            }
            let c = &g.ctx().call_issuer;
            let waiting: Vec<(usize, Lane)> = (lo..hi).filter(|&i| !tools.started[i].load(Ordering::SeqCst)).map(|i| (i, lane(i))).take(12).collect();
            panic!(
                "batch {lo}..{hi}: {} of {} handlers started after {:?}; not started (the first 12): {waiting:?}; answered early: {early:?}; mints {} refused {}; provider running {}, views {:?}, windows refused {}, checkpoints refused {}; {}",
                (lo..hi).filter(|&i| tools.started[i].load(Ordering::SeqCst)).count(), s.batch, launched.elapsed(), c.mints.load(Ordering::Relaxed), c.mints_refused.load(Ordering::Relaxed),
                p.authority.running_count(), p.authority.view_sizes(), p.authority.call_windows_refused(), p.authority.call_checkpoints_refused(), diagnose(&g, &p, &http).await,
            );
        }
        let t_running = launched.elapsed();
        // `running_count` counts registrations: one per call under its `call:` mandate, a second under its grant.
        let registrations = s.batch + (lo..hi).filter(|&i| lane(i) == Lane::Both).count();
        assert_eq!(p.authority.running_count(), registrations, "batch {lo}: registrations at the provider, two per call under both slots");
        assert_eq!(g.authority.running_count(), 0, "the gateway's own authority runs nothing");

        // Each call's term: as the provider read it from the envelope (the skill lanes) — the same the gateway's
        // dispatch handed its door, where the test holds that handle — or from the `/mcp` door's table.
        let mut terms: Vec<String> = Vec::with_capacity(s.batch);
        for i in lo..hi {
            let term = match lane(i) {
                Lane::McpDoor => call_cancel::test_hooks::mcp_sink(g.ctx(), &(ALICE.to_string(), String::new(), i.to_string())).as_deref().and_then(CallSink::term).map(str::to_string),
                Lane::A2a => tools.term[i].get().cloned(),
                Lane::Both => {
                    assert_eq!(sinks[i - lo].term(), tools.term[i].get().map(String::as_str), "call {i}: the term the dispatch minted is the term the provider verified");
                    tools.term[i].get().cloned()
                }
            }
            .unwrap_or_else(|| panic!("call {i} ({:?}) is running and no term is known for it", lane(i)));
            assert!(all_terms.insert(term.clone()), "call {i}: the term {term} was minted twice");
            terms.push(term);
        }
        peak_windows = peak_windows.max(p.authority.view_sizes().2);
        peak_issued = peak_issued.max(g.ctx().call_issuer.issued_terms().len());

        // Cancel the odd calls, each through its own door, all at once; the even ones are still running.
        let mut cancels: Vec<(usize, tokio::task::JoinHandle<CancelSent>)> = Vec::new();
        for i in (lo..hi).filter(|&i| cancelled(i)) {
            let term = terms[i - lo].clone();
            cancelled_terms.insert(term.clone());
            cancels.push((i, match lane(i) {
                Lane::A2a => {
                    let (http, url, g) = (http.clone(), a2a_url.clone(), Arc::clone(&g));
                    tokio::spawn(async move {
                        let (sent, sent_ms) = (Instant::now(), g.now());
                        let answer = post(&http, &url, serde_json::json!({"jsonrpc": "2.0", "id": i, "method": "tasks/cancel", "params": {"id": format!("t-{i}")}})).await;
                        assert!(answer.starts_with("200 ") && answer.contains("canceled"), "call {i}: tasks/cancel answered {answer}");
                        (sent, sent_ms, None, sent.elapsed())
                    })
                }
                Lane::McpDoor => {
                    let (http, url, g) = (http.clone(), mcp_url.clone(), Arc::clone(&g));
                    tokio::spawn(async move {
                        let (sent, sent_ms) = (Instant::now(), g.now());
                        let answer = post(&http, &url, serde_json::json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": i, "reason": "soak"}})).await;
                        assert_eq!(answer, "202 ", "call {i}: the cancel notification is accepted");
                        (sent, sent_ms, None, sent.elapsed())
                    })
                }
                Lane::Both => {
                    let g = Arc::clone(&g);
                    tokio::spawn(async move {
                        let (sent, sent_ms) = (Instant::now(), g.now());
                        let receipt = g.a.cancel_call(&term).await.unwrap_or_else(|e| panic!("call {i}: cancel_call refused: {e:?}"));
                        (sent, sent_ms, Some(receipt.class), sent.elapsed())
                    })
                }
            }));
        }
        let mut sent_at: BTreeMap<usize, (Instant, u64)> = BTreeMap::new();
        for (i, c) in cancels {
            let (sent, sent_ms, class, cancel_took) = c.await.expect("the cancel ran");
            sent_at.insert(i, (sent, sent_ms));
            if let Some(class) = class {
                // A receipt names what the provider answered. `DeliveryUnknown` — no answer inside the forward's 5 s —
                // is not a negative and is counted, not failed on: the call must still stop inside bound 1, below.
                assert!(matches!(class, CancelClass::Stopped | CancelClass::StopRequested | CancelClass::DeliveryUnknown), "call {i}: the provider answered the cancel-forward {class:?}");
                if class == CancelClass::DeliveryUnknown {
                    unanswered.push((i, cancel_took));
                }
                *receipts.entry(class.label()).or_default() += 1;
            }
        }
        let t_cancels = launched.elapsed() - t_running;
        // The cancelled calls are answered while the others are still running.
        let mut answers: BTreeMap<usize, String> = BTreeMap::new();
        let mut answered_at: BTreeMap<usize, (Duration, u64)> = BTreeMap::new();
        for (k, call) in calls.iter_mut().enumerate().filter(|(k, _)| cancelled(lo + k)) {
            let i = lo + k;
            let (answer, done, done_ms) = tokio::time::timeout(Duration::from_secs(30), call).await.unwrap_or_else(|_| panic!("call {i} was cancelled and never answered")).expect("the call ran");
            answered_at.insert(i, (done.saturating_duration_since(sent_at[&i].0), done_ms));
            answers.insert(i, answer);
        }
        let t_answered = launched.elapsed() - t_running - t_cancels;
        for (name, t) in [("launch -> every call running", t_running), ("the cancels", t_cancels), ("the cancelled calls answered", t_answered)] {
            let slot = slowest.entry(name).or_insert((Duration::ZERO, 0));
            if t > slot.0 {
                *slot = (t, lo);
            }
        }
        // One stop record for each cancelled call of the batch, naming its term and its gateway.
        let batch_cancelled: Vec<&String> = (lo..hi).filter(|&i| cancelled(i)).map(|i| &terms[i - lo]).collect();
        let named = |stops: &mut std::collections::HashMap<String, (String, crate::mandate::authority::StopRecord)>| {
            for r in p.authority.named_stop_records() {
                if let (Some(scope), Some(term)) = (r.scope, r.term) {
                    assert_eq!(r.established_by.as_deref(), Some(g.principal().as_str()), "the record names the issuing gateway");
                    stops.insert(term, (scope, r.record));
                }
            }
            batch_cancelled.iter().all(|t| stops.get(*t).is_some_and(|(scope, _)| scope == &format!("call:{t}")))
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !named(&mut stops) {
            assert!(Instant::now() < deadline, "batch {lo}: a cancelled call ended with no stop record naming its term");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // **Bound 1**, for each cancelled call: from the gateway's revocation (its revoking checkpoint's `issued_at_ms`,
        // sequence 2, read from the gateway's own record) to the provider's confirmed stop (the stop record), against
        // *F*. On either side of it, reported and not part of the bound: the door (the cancel sent → the revocation
        // signed) and the answer (the stop confirmed → the caller answered).
        for i in (lo..hi).filter(|&i| cancelled(i)) {
            let term = &terms[i - lo];
            let revoking: crate::mandate::authority::SignedRevocationCheckpoint =
                serde_json::from_slice(&mycelium_core::ops::kv_get(g.ctx(), &call_checkpoint_key(&g.principal(), term)).expect("the gateway's record")).expect("a checkpoint");
            assert!(
                revoking.checkpoint.seq == 2 && revoking.checkpoint.revoked.iter().any(|t| t.as_str() == term),
                "call {i} ({:?}): its door accepted the cancel and the gateway's record is not the revoking checkpoint: {:?}; answered {:?}",
                lane(i), revoking.checkpoint, answers.get(&i)
            );
            let record = stops[term].1;
            let (revoked_ms, sent_ms, (took, answered_ms)) = (revoking.checkpoint.issued_at_ms, sent_at[&i].1, answered_at[&i]);
            let (Some(asked_ms), Some(confirmed_ms)) = (record.requested_ms, record.confirmed_ms) else { panic!("call {i} ({:?}): its stop record is not a confirmed stop: {record:?}", lane(i)) };
            let bound_1 = confirmed_ms.saturating_sub(revoked_ms);
            let line = format!(
                "call {i} ({:?}): cancel sent at {sent_ms}, revocation signed +{} ms, stop asked at the provider +{} ms, stop confirmed +{} ms, caller answered +{} ms (cancel sent -> answered {took:?})",
                lane(i), revoked_ms.saturating_sub(sent_ms), asked_ms.saturating_sub(sent_ms), confirmed_ms.saturating_sub(sent_ms), answered_ms.saturating_sub(sent_ms)
            );
            assert!(
                bound_1 < F_MS,
                "outside bound 1 ({bound_1} ms from the revocation to the confirmed stop, against F = {F_MS} ms): {line}; answered {:?}; batch {lo}: launch -> all running {:?}, cancels {:?}, cancelled calls answered {:?}; the gateway's receipts {:?}; transport drops {:?}; provider: checkpoints refused {} stale {} missing {}, holds refused {}",
                answers.get(&i), t_running, t_cancels, t_answered, door_receipts(&g), transport_drops(&g, &p),
                p.authority.call_checkpoints_refused(), p.authority.call_checkpoints_stale(), p.authority.call_checkpoints_missing(), p.authority.call_holds_refused(),
            );
            phases.entry("door: cancel sent -> revocation signed").or_default().push(revoked_ms.saturating_sub(sent_ms));
            phases.entry("bound 1: revocation signed -> stop confirmed at the provider").or_default().push(bound_1);
            phases.entry("answer: stop confirmed -> caller answered").or_default().push(answered_ms.saturating_sub(confirmed_ms));
            if let Some((_, cancel_took)) = unanswered.iter().find(|(u, _)| *u == i) {
                unanswered_lines.push(format!("{line}; its cancel_call returned DeliveryUnknown after {cancel_took:?}"));
            }
            if worst.as_ref().is_none_or(|(w, _)| took.as_millis() as u64 > *w) {
                worst = Some((took.as_millis() as u64, line));
            }
            let name = match lane(i) { Lane::A2a => "a2a", Lane::McpDoor => "mcp", Lane::Both => "cancel_call" };
            cancel_to_answer.entry(name).or_default().push(took);
        }
        let still = (lo..hi).filter(|&i| !cancelled(i) && !tools.finished[i].load(Ordering::SeqCst) && !calls[i - lo].is_finished()).count();
        if launched.elapsed() >= Duration::from_secs(s.ttl_secs) {
            panic!(
                "batch {lo} outlived its calls' windows ({} s): launch -> all running {t_running:?}, cancels {t_cancels:?}, cancelled calls answered {t_answered:?}, in all {:?}; {} — the calls that were not cancelled end at their windows, so nothing after this would be the soak's to assert",
                s.ttl_secs, launched.elapsed(), diagnose(&g, &p, &http).await
            );
        }
        assert_eq!(still, s.batch / 2, "batch {lo}: a neighbour's cancel ended none of the calls that were not cancelled");
        for i in (lo..hi).filter(|&i| !cancelled(i)) {
            tools.release[i].notify_one();
        }
        for (k, call) in calls.iter_mut().enumerate().filter(|(k, _)| !cancelled(lo + k)) {
            let (answer, _, _) = tokio::time::timeout(Duration::from_secs(60), call).await.unwrap_or_else(|_| panic!("call {} was released and never answered", lo + k)).expect("the call ran");
            answers.insert(lo + k, answer);
        }
        for (i, answer) in &answers {
            let finished = tools.finished[*i].load(Ordering::SeqCst);
            if cancelled(*i) {
                // A tool's future is dropped and the node answers `cancelled`; a skill is told its authority lapsed and
                // answers for itself.
                let stopped = if lane(*i) == Lane::McpDoor { answer.contains("cancelled") } else { answer.contains(&format!("stopped:{i}")) };
                assert!(stopped && !finished, "call {i} ({:?}) was cancelled: answered {answer}; its handler finished: {finished}", lane(*i));
            } else {
                assert!(answer.contains(&format!("done:{i}")) && finished, "call {i} ({:?}) completed on its own: answered {answer}; its handler finished: {finished}", lane(*i));
            }
        }
        assert!(until(|| p.authority.running_count() == 0, Duration::from_secs(20)).await, "batch {lo}: running_count {} after every call ended", p.authority.running_count());
        if (hi / s.batch).is_multiple_of(20) || hi == s.n {
            let rss = rss_kib();
            rss_peak = rss_peak.max(rss);
            eprintln!(
                "i4-pr4 soak: {hi}/{} calls in {:?}; provider views {:?}, gateway issued {}, call/ records {} (gateway) {} (provider), rss {} KiB; the gateway's receipts {:?}; transport drops (gateway frames, signals; provider frames, signals) {:?}; slowest batch by phase {slowest:?}",
                s.n, began.elapsed(), p.authority.view_sizes(), g.ctx().call_issuer.issued_terms().len(), g.call_keys(), p.call_keys(), rss.unwrap_or(0), door_receipts(&g), transport_drops(&g, &p)
            );
        }
    }

    // ── the whole run, reconciled ───────────────────────────────────────────────────────────────────────────────
    let in_lane = |l: Lane| (0..s.n).filter(|&i| lane(i) == l).count();
    let both_n = in_lane(Lane::Both);
    assert_eq!(Tools::count(&tools.started), s.n, "every call's handler started");
    assert_eq!(Tools::count(&tools.finished), s.n / 2, "exactly the calls that were not cancelled finished");
    assert_eq!(tools.overran.load(Ordering::SeqCst), 0, "no handler ran past its own bound");
    assert_eq!(cancelled_terms.len(), s.n / 2);
    let stopped: HashSet<String> = stops.iter().map(|(term, (scope, _))| {
        assert_eq!(scope, &format!("call:{term}"), "every stop is under a call: mandate — the grants were not revoked");
        term.clone()
    }).collect();
    assert_eq!(stopped, cancelled_terms, "the stop records name exactly the cancelled calls: no call ended without a stop record or its own completion");
    let c = &g.ctx().call_issuer;
    assert_eq!((c.mints.load(Ordering::Relaxed), c.mints_refused.load(Ordering::Relaxed)), (s.n as u64, 0), "one mint a call");
    let t = &c.mcp_cancels;
    assert_eq!(
        (t.cancelled.load(Ordering::Relaxed), t.unmatched.load(Ordering::Relaxed), t.full.load(Ordering::Relaxed), t.collisions.load(Ordering::Relaxed)),
        ((in_lane(Lane::McpDoor) / 2) as u64, 0, 0, 0),
        "the /mcp door's cancels"
    );
    assert_eq!(call_cancel::test_hooks::mcp_calls_held(g.ctx()), 0, "the /mcp table holds no finished call");
    assert_eq!(receipts.values().sum::<usize>(), both_n / 2, "one receipt for each cancel_call");
    // Every lane's receipt (#816's review): a door's forward is sent on a task of its own and counted when the provider
    // answers, so the count is waited for; then each of the n/2 is a stop, or no answer in time — never `not_running`
    // for a call this soak saw stopped, `not_registered`, `not_held`, `unbounded` or a refusal.
    assert!(until(|| door_receipts(&g).values().sum::<u64>() >= (s.n / 2) as u64, Duration::from_secs(30)).await, "a receipt for every cancel: {:?}", door_receipts(&g));
    let by_class = door_receipts(&g);
    assert_eq!(by_class.values().sum::<u64>(), (s.n / 2) as u64, "one receipt a cancel, whichever door sent it: {by_class:?}");
    assert!(by_class.keys().all(|class| ["stopped", "stop_requested", "delivery_unknown"].contains(class)), "every cancel-forward's receipt is a stop, or no answer in time: {by_class:?}");
    for n in [&g, &p] {
        assert!(n.ctx().call_issuer.cancel_forwards_refused.pin().is_empty(), "no cancel-forward was refused");
        assert_eq!((n.authority.call_windows_refused(), n.authority.call_checkpoints_refused(), n.authority.call_holds_refused()), (0, 0, 0), "nothing refused at an authority");
    }

    // ── both nodes' views after the last window ─────────────────────────────────────────────────────────────────
    let slack = Duration::from_secs(s.ttl_secs + 20);
    assert!(until(|| p.authority.view_sizes().2 == 0, slack).await, "the provider's call: windows after the last window: {:?}", p.authority.view_sizes());
    assert_eq!(p.authority.view_sizes(), (both_n, both_n, 0), "the provider's views: no call: scope; one grant: scope per both-slots call, which an authority never forgets");
    assert_eq!(g.authority.view_sizes(), (0, 0, 0), "the gateway's views");
    assert_eq!((p.authority.running_count(), g.authority.running_count()), (0, 0));
    // The gateway's collector (D-5): past the last window + 2·skew + `max_drain_secs` it holds nothing as issued and
    // deletes its own records; the tombstones reach the provider.
    let floor = last_until_ms + 2 * SKEW_MS + 1_000;
    assert!(until(|| g.now() > floor, Duration::from_secs(30)).await, "the collector's floor passes");
    let held_before = g.call_keys();
    let _ = call_issuer::collect(g.ctx(), g.now());
    assert!(g.ctx().call_issuer.issued_terms().is_empty(), "the gateway holds no call as issued");
    assert_eq!(g.call_keys(), 0, "the gateway's own call/ records are collected");
    let records_collected = c.call_records_collected.load(Ordering::Relaxed) as usize;
    // Not an equality (#816's review): the node's own collector runs the same pass each minute on another task, and
    // two passes that overlap both count a key. What is asserted is the KV, above; the counter is a floor.
    assert!(records_collected >= 2 * s.n, "two records a call were collected: {records_collected} (the gateway held {held_before} at the last collection)");
    assert!(until(|| p.call_keys() == 0, Duration::from_secs(120)).await, "the provider's copies go with the gateway's tombstones: {} left", p.call_keys());

    let rss_end = rss_kib();
    let took = began.elapsed();
    let mut mint_writes = call_issuer::test_hooks::record_latencies(g.a.node_id());
    mint_writes.sort();
    assert_eq!(mint_writes.len(), s.n, "one pair of durable writes a mint");
    let (forwards, drops) = (door_receipts(&g), transport_drops(&g, &p));
    g.a.shutdown().await;
    p.a.shutdown().await;
    cancel_to_answer.values_mut().for_each(|v| v.sort());
    phases.values_mut().for_each(|v| v.sort());
    Report { drops, forwards, slowest, phases, worst: worst.map(|w| w.1).unwrap_or_default(), unanswered: unanswered_lines, cancel_to_answer, took, receipts, peak_windows, peak_issued, rss: (rss_start, rss_peak.max(rss_end), rss_end), grant_scopes_left: both_n, records_collected, mint_writes }
}

fn print(name: &str, s: &Soak, r: &Report) {
    let mut all: Vec<Duration> = r.cancel_to_answer.values().flatten().copied().collect();
    all.sort();
    let dist = |l: &[Duration]| format!("n = {}: p50 {:?}, p95 {:?}, p99 {:?}, max {:?}", l.len(), pct(l, 50), pct(l, 95), pct(l, 99), l[l.len() - 1]);
    let ms = |l: &[u64]| format!("n = {}: p50 {}, p95 {}, p99 {}, max {}", l.len(), l[l.len() / 2], l[l.len() * 95 / 100], l[l.len() * 99 / 100], l[l.len() - 1]);
    let phases: Vec<String> = r.phases.iter().map(|(phase, l)| format!("{phase} {}", ms(l))).collect();
    let by_lane: Vec<String> = r.cancel_to_answer.iter().map(|(lane, l)| format!("{lane} {}", dist(l))).collect();
    eprintln!(
        "i4-pr4 {name}: {} calls ({} cancelled while running; {} under both slots), batches of {}, window {} s, gateway persistence {:?}, F = {F_MS} ms, in {:?}. \
         Bound 1 and its neighbours, ms, {}. The caller's wait, cancel sent -> caller answered, {}; by door: {}; the longest: {}. cancel_call receipts {:?}; forwards not answered: {:?}. Every cancel-forward's receipt, as the gateway counted it: {:?}. The slowest batch by phase (and its first call): {:?}. Transport drops (gateway frames, signals; provider frames, signals): {:?}. \
         Peak held: {} call: windows (provider), {} issued (gateway). Records collected {}. grant: scopes left at the provider {}. \
         The mint's two durable writes under this load, {}. \
         Process rss (both nodes) KiB: start {:?}, peak sampled {:?}, end {:?}.",
        s.n, s.n / 2, r.grant_scopes_left, s.batch, s.ttl_secs, s.persist, r.took, phases.join("; "), dist(&all), by_lane.join("; "), r.worst, r.receipts, r.unanswered, r.forwards, r.slowest, r.drops,
        r.peak_windows, r.peak_issued, r.records_collected, r.grant_scopes_left, dist(&r.mint_writes), r.rss.0, r.rss.1, r.rss.2,
    );
}
/// **Exit 14 in CI: 10³ cancellable calls over two nodes, half of them cancelled while running** — what [`soak`]
/// asserts, in batches of 20 (ten cancels at once). **The gateway has no `[persistence]` here**, so the mint's and the
/// revoke's records are written without an fsync: with it, a run that is still minting when the gateway's collector
/// makes its minute's pass stalls behind that pass's deletes (the soak's finding,
/// [`a_cancel_is_on_disk_inside_bound_1_while_the_collector_deletes_a_backlog`]), and a soak that passes or fails by
/// how fast its runner is would be a flake. The durable writes have their own tests (`call_mint_tests`).
///
/// Seen failing first, with a plant: one stop record in five not kept (`stops.push_back` skipped when the stop's
/// millisecond is a multiple of five, in the work guard's drop, `gateway_authority.rs`) — *"batch 100: a cancelled
/// call ended with no stop record naming its term"*. And, unplanted, against what the soak met while it was written:
/// a registered tool serving one call at a time (2 of 50 handlers started), bound 1 timed from the client's cancel
/// (2.03 s against *F*), and the gateway's collector (the finding).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thousand_cancellable_calls_half_cancelled_while_running_all_reconcile() {
    let s = Soak { n: 1_000, batch: 20, ttl_secs: 30, persist: None, within: Duration::from_secs(420) };
    let r = soak(s).await;
    print("soak (CI, 10^3)", &s, &r);
}

/// **Exit 14, the soak itself: 10⁵ cancellable calls over two nodes, half of them cancelled while running.** Run
/// once, in release, and dated on `docs/operations/what-is-proven.md`; not run in CI. Bounded: the soak fails past
/// two hours, and each batch has its own deadlines.
///
/// **The dated run is of a gateway with no `[persistence]`.** With it under `SyncMode::Flush` (`I4_SOAK_PERSIST=1`) the soak **fails** at
/// one of the gateway's collection passes with a backlog — it first stalled 60 to 130 s in, between calls 1 100 and
/// 2 600, in every run made: the finding pinned by [`a_cancel_is_on_disk_inside_bound_1_while_the_collector_deletes_a_backlog`]. Exit 14 as the
/// plan states it — over a fleet whose gateway keeps the promise of exit 11 — is therefore **not met**.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "the 10^5 soak, run once and dated: cargo test --release --lib --features compliance,a2a a_hundred_thousand_cancellable_calls -- --ignored --nocapture"]
async fn a_hundred_thousand_cancellable_calls_half_cancelled_while_running_all_reconcile() {
    // The dated run uses these. `I4_SOAK_N`, `I4_SOAK_BATCH`, `I4_SOAK_TTL` and `I4_SOAK_PERSIST` (`flush` or `1`,
    // `async` — the default `sync_mode`) shape another.
    let env = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let s = Soak { n: env("I4_SOAK_N", 100_000) as usize, batch: env("I4_SOAK_BATCH", 100) as usize, ttl_secs: env("I4_SOAK_TTL", 30), persist: match std::env::var("I4_SOAK_PERSIST").as_deref() {
            Ok("1" | "flush") => Some(crate::config::SyncMode::Flush),
            Ok("async") => Some(crate::config::SyncMode::Async),
            _ => None,
        }, within: Duration::from_secs(7_200) };
    let r = soak(s).await;
    print("soak (10^5)", &s, &r);
}

// ── the measurements (exit 18; the memory per held window) ──────────────────────────────────────────────────────

/// A lone node that mints, as the measurements' gateway: its own id stands for the provider's in its policy.
async fn lone(persist: bool) -> Node {
    lone_with(persist, 300).await
}

/// [`lone`], with `call_mandate_ttl_secs`.
async fn lone_with(persist: bool, ttl_secs: u64) -> Node {
    let (port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("i4-soak-{port}"));
    soak_node(port, http_port, NodeOpts { cert_dir: &dir.join("tls"), bootstrap: vec![], provider: &id, gateway: &id, mint: true, ttl_secs, persist: persist.then_some(crate::config::SyncMode::Flush), mcp_tools: 0, tools: None }).await
}

/// An execution authority as `soak_node` attaches one, standing alone, and the member table that names `gateway`'s key.
fn lone_authority(gateway: &NodeId, key: &ed25519_dalek::SigningKey) -> (ExecutionAuthority, std::collections::HashMap<NodeId, MemberKeys>) {
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(node_principal(gateway)).unwrap());
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, FreshnessPolicy { freshness_ms: F_MS, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let members = std::collections::HashMap::from([(gateway.clone(), MemberKeys { retained: vec![key.verifying_key().to_bytes()], ..Default::default() })]);
    (ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()), members)
}

fn established(a: &super::gateway_authority::Assessment) -> bool {
    a.binding.as_ref().is_some_and(|b| b.state == super::action_evaluator::MandateState::Established)
}

/// A window record for `term`, signed by `key`, as the mint writes it.
fn window_record(key: &ed25519_dalek::SigningKey, term: &str, until_ms: u64) -> Vec<u8> {
    let window = CallWindow { v: 1, scope: format!("call:{term}"), term: term.into(), valid_until_ms: until_ms };
    let signature = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, mycelium_core::tls::sign_bytes(key, &call_issuer::window_message(&window)));
    serde_json::to_vec(&SignedCallWindow { window, signature }).unwrap()
}

/// **Exit 18's bytes: the worst-case envelope against the 8 KiB cap.** One envelope carrying everything a call can:
/// an OIDC user's exchanged token (`ot`, RS256, DPoP-bound by `cnf.jkt`) under the gateway's added signature (`so`);
/// a client's grant in slot 1, bound to its resource and policy revision, with the client's own `cs`/`cn`; the
/// resource claim; and the minted `call:` mandate with its base64 checkpoint in slot 2. Its size is **computed at its
/// most** — two of its signatures are JSON arrays of decimals, whose width depends on the signature's bytes — and that
/// fits the cap; its bytes are printed by field. The room left for the token is computed the same way: a token of
/// that length is framed every time, held to a floor of 4 KiB, and one too long for even the shortest signatures is
/// framed by none (the dispatch's refusal by name, `caller_context_too_large`, is
/// `an_envelope_past_the_cap_with_its_call_mandate_is_refused_by_name_and_writes_nothing`). A DPoP **proof** is not
/// in the envelope: it goes to the IdP with the exchange. Seen failing first as it was written before #816's review
/// (a binary search over frames whose size is not repeatable): 17 of the reviewer's 40 runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_worst_case_envelope_fits_the_cap_and_its_bytes_are_stated() {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let n = lone(false).await;
    let target = NodeId::new("127.0.0.1", 65_000).unwrap();
    let principal = "oidc:https://idp.example/alice";
    let resource = format!("tool:work@{target}");
    let arguments = serde_json::json!({ "aisle": 4 });
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "work", "arguments": arguments}}).to_string();
    let now = n.now();
    // The exchanged token: the claims the provider verifies, signed RS256 by the test IdP's 2048-bit key.
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".into());
    let claims = serde_json::json!({
        "iss": "https://idp.example", "aud": "mycelium:fleet", "azp": "mycelium-gateway", "sub": "alice", "iat": now / 1_000, "exp": now / 1_000 + 300,
        "jti": "0b7c1d0e-6f1a-4f5e-9d3b-2a4c6e8f0a12", "cnf": {"jkt": super::exchanged_token::ed25519_jwk_thumbprint(&n.key().verifying_key().to_bytes())},
    });
    let ot = encode(&header, &claims, &EncodingKey::from_rsa_pem(include_str!("../../tests/fixtures/oidc_test.key").as_bytes()).unwrap()).unwrap();
    // The client's grant: issued by the gateway to the user, bound to the arguments, the policy revision and the resource.
    let digest = arguments_digest(&arguments);
    let request = "01JCZ3V8Q4M7T2N5R9X6B0K1HD";
    let m = Mandate {
        holder: PrincipalId::new(principal).unwrap(),
        established_by: PrincipalId::new(n.principal()).unwrap(),
        purpose: format!("grant {request}"),
        scope: format!("grant:{request}"),
        operations: Mandate::grant_operations_on(mandate_operation("tools/call", &resource), &super::action_evaluator::hex32(&digest), Some("rev-soak"), &resource),
        epoch: 1,
        term: TermId::new(request).unwrap(),
        valid_from_ms: now,
        valid_until_ms: now + 300_000,
    };
    let client_key = ed25519_dalek::SigningKey::from_bytes(&[73u8; 32]);
    let proof = mycelium_core::tls::sign_bytes(&client_key, &possession_message(&m, &possession_request("tools/call", &resource, &digest)));
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&n.key(), &m.canonical_bytes()).to_vec(), mandate: m };
    let b64 = |b: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b);
    let slot1 = serde_json::to_value(PresentedMandate { grant, possession: b64(&proof) }).unwrap();
    let client = (
        super::client_sig::ClientSignature { k: b64(&client_key.verifying_key().to_bytes()), sig: b64(&[0u8; 64]) },
        super::client_sig::ClientNonce { t: now, i: "0123456789abcdef".into(), c: u64::from(u32::MAX) },
    );
    let scopes = vec!["mcp:invoke".to_string()];
    let frame = |ot: &str| frame_minting(n.ctx(), &target, principal, &scopes, MCP, Bytes::from(payload.clone()), Some(&slot1), Some(&resource), Some(&client), Some(ot), now).0;

    let framed = frame(&ot).expect("the worst-case envelope fits the cap");
    let len = u16::from_be_bytes([framed[5], framed[6]]) as usize;
    let env: serde_json::Value = serde_json::from_slice(&framed[7..7 + len]).unwrap();
    let field = |k: &str| serde_json::to_vec(&env[k]).unwrap().len();
    assert!(["m", "cm", "ot", "so", "cs", "cn", "r"].iter().all(|k| !env[*k].is_null()), "every slot is carried: {env}");
    // **One sample is not the worst case** (#816's review): a `SignedMandateGrant`'s signature is a `Vec<u8>`, which
    // JSON writes as an array of decimals — 129 to 257 bytes for 64 signature bytes — and each frame mints a fresh
    // `cm` with fresh signatures, so the envelope's size moves by tens of bytes from one frame to the next. The
    // bounds are therefore computed: every byte of every array-encoded signature at three digits (the most), and at
    // one (the least). Everything else in the envelope has a fixed width.
    let (mut arrays, mut to_most, mut to_least) = (0usize, 0usize, 0usize);
    let mut stack = vec![&env];
    while let Some(v) = stack.pop() {
        match v {
            serde_json::Value::Array(a) if a.len() == 64 && a.iter().all(|b| b.as_u64().is_some_and(|b| b < 256)) => {
                arrays += 1;
                for digits in a.iter().map(|b| b.to_string().len()) {
                    to_most += 3 - digits;
                    to_least += digits - 1;
                }
            }
            serde_json::Value::Array(a) => stack.extend(a),
            serde_json::Value::Object(o) => stack.extend(o.values()),
            _ => {}
        }
    }
    assert_eq!(arrays, 2, "the two array-encoded signatures: the client's grant (slot 1) and the call: mandate (slot 2)");
    let (most, least) = (len + to_most, len - to_least);
    assert!(most <= MAX_ENVELOPE_BYTES, "the envelope fits the cap whatever its signatures' bytes: at most {most} B");
    // The room an exchanged token has beside everything else, whatever the signatures: the cap less the rest at its
    // most. A token of that length always fits; one longer than the rest at its least allows never does.
    let always_fits = MAX_ENVELOPE_BYTES - (most - ot.len());
    let never_fits = MAX_ENVELOPE_BYTES - (least - ot.len()) + 1;
    eprintln!(
        "i4-pr4 exit 18, the worst-case envelope: at most {most} B of the {MAX_ENVELOPE_BYTES} B cap (this frame {len} B, at least {least} B: two signatures are JSON arrays of decimals) — ot {} B (so {} B), slot 1 (m, a client's grant) {} B, cs {} B, cn {} B, r {} B, slot 2 (cm) {} B of which its checkpoint {} B; the rest (p, via, s, t, k, sig, n, o, sn) {} B. An ot of up to {always_fits} B fits whatever the signatures; one of {never_fits} B never does.",
        field("ot"), field("so"), field("m"), field("cs"), field("cn"), field("r"), field("cm"), serde_json::to_vec(&env["cm"]["checkpoint"]).unwrap().len(),
        len - ["ot", "so", "m", "cs", "cn", "r", "cm"].iter().map(|k| field(k)).sum::<usize>(),
    );
    // A floor under the room an IdP's token has, so a field added to the envelope shows here before it shows in a fleet.
    assert!(always_fits >= 4_096, "with every other slot carried, an exchanged token of 4 KiB fits whatever the signatures: {always_fits}");
    for _ in 0..20 {
        assert!(frame(&"x".repeat(always_fits)).is_some(), "a token of {always_fits} B fits on every frame");
        assert!(frame(&"x".repeat(never_fits)).is_none(), "a token of {never_fits} B is framed by none");
    }
    assert!(n.ctx().call_issuer.issued_terms().is_empty() && n.call_keys() == 0, "a framed envelope that was never recorded holds and writes nothing");
    n.a.shutdown().await;
}

/// **Exit 18's re-run of S-cancel's figures on the shipped code** (release; printed, and dated on
/// `docs/operations/what-is-proven.md`):
/// - **the provider's `assess`**: `assess_call` of a `call:` mandate with its carried first checkpoint, the KV copy
///   absent, interleaved with the grant path's `assess` of a `grant:` mandate whose checkpoint the view already
///   holds — 20 000 of each, three repetitions;
/// - **the gateway's mint**: framing with the mint (the mandate, its possession proof and its first checkpoint signed,
///   and the window record) and, apart, its two records' durable writes (`SyncMode::Flush`, one call at a time).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement: cargo test --release --lib --features compliance,a2a the_call_mandates_costs_are_measured -- --ignored --nocapture"]
async fn the_call_mandates_costs_are_measured_on_the_shipped_code() {
    let n = lone(true).await;
    let (key, gateway, id) = (n.key(), n.principal(), n.a.node_id().clone());
    let (authority, members) = lone_authority(&id, &key);
    let resource = format!("tool:work@{id}");
    let args = serde_json::json!({ "aisle": 4 });
    let digest = arguments_digest(&args);
    const N: usize = 20_000;
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let stats = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (v.iter().sum::<f64>() / v.len() as f64, v[v.len() / 2], v[v.len() * 99 / 100])
    };
    for rep in 0..3 {
        let now = 1_000_000 + rep as u64 * 10_000;
        let (mut call_us, mut grant_us) = (Vec::with_capacity(N), Vec::with_capacity(N));
        for i in 0..N {
            let cm = call_mint::mint(&key, &gateway, &format!("c{rep}x{i}"), "tools/call", &resource, &args, now, now + 5_000);
            let request = format!("r{rep}x{i}");
            let m = Mandate {
                holder: PrincipalId::new(gateway.as_str()).unwrap(),
                established_by: PrincipalId::new(gateway.as_str()).unwrap(),
                purpose: format!("grant {request}"),
                scope: format!("grant:{request}"),
                operations: Mandate::grant_operations(mandate_operation("tools/call", &resource), &super::action_evaluator::hex32(&digest), None),
                epoch: 1,
                term: TermId::new(request.as_str()).unwrap(),
                valid_from_ms: now,
                valid_until_ms: now + 5_000,
            };
            let proof = mycelium_core::tls::sign_bytes(&key, &possession_message(&m, &possession_request("tools/call", &resource, &digest)));
            let _ = authority.offer_checkpoint(&call_mint::checkpoint(&key, &gateway, &m.scope, 1, now, &[]), now, &members);
            let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&key, &m.canonical_bytes()).to_vec(), mandate: m };
            let presented = PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) };
            let t = Instant::now();
            let a = authority.assess_call(&gateway, "tools/call", &resource, &args, &cm, now + 1, &members, |_| None);
            call_us.push(us(t.elapsed()));
            assert!(established(&a), "{a:?}");
            let t = Instant::now();
            let a = authority.assess(&gateway, "tools/call", &resource, &digest, Some(&presented), now + 1, &members);
            grant_us.push(us(t.elapsed()));
            assert!(established(&a), "{a:?}");
        }
        // Past the windows, so the next repetition starts from an empty table of call: windows.
        while authority.view_sizes().2 > 0 {
            authority.sweep(now + 5_001);
        }
        let (c, g) = (stats(&mut call_us), stats(&mut grant_us));
        eprintln!("i4-pr4 exit 18, the provider's assess, repetition {rep}, n = {N} each, interleaved: call: mandate with its carried checkpoint mean {:.1} / p50 {:.1} / p99 {:.1} µs; grant path mean {:.1} / p50 {:.1} / p99 {:.1} µs", c.0, c.1, c.2, g.0, g.1, g.2);
    }

    // The gateway's mint: framing (the envelope's own signature and the mint's three), then the durable writes.
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "work", "arguments": args}}).to_string();
    let target = NodeId::new("127.0.0.1", 65_000).unwrap();
    let (mut mint_us, mut plain_us, mut write_ms) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..2_000 {
        let t = Instant::now();
        let (framed, minted) = frame_minting(n.ctx(), &target, ALICE, &[], MCP, Bytes::from(payload.clone()), None, None, None, None, n.now());
        mint_us.push(us(t.elapsed()));
        let minted = minted.expect("minted");
        assert!(framed.is_some());
        let t = Instant::now();
        let plain = super::gateway_caller::frame_envelope_at(n.ctx(), &target, ALICE, &[], Bytes::from(payload.clone()), None, None, None, None, n.now());
        plain_us.push(us(t.elapsed()));
        assert!(plain.is_some());
        if i < 500 {
            let t = Instant::now();
            call_issuer::record(n.ctx(), minted).await.expect("on disk");
            write_ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let (m, p, w) = (stats(&mut mint_us), stats(&mut plain_us), stats(&mut write_ms));
    eprintln!(
        "i4-pr4 exit 18, the gateway's mint: framing with the mint mean {:.1} / p50 {:.1} / p99 {:.1} µs against framing without it {:.1} / {:.1} / {:.1} µs (n = 2 000 each, interleaved); its two durable writes, SyncMode::Flush, one call at a time, n = 500: mean {:.2} / p50 {:.2} / p99 {:.2} ms, max {:.2} ms",
        m.0, m.1, m.2, p.0, p.1, p.2, w.0, w.1, w.2, write_ms[write_ms.len() - 1],
    );
    n.a.shutdown().await;
}

/// **The memory a held `call:` window takes, on each node** — what PR 1a left to this PR: *"resident memory per held
/// window on both nodes, at the 64-byte term cap"* (S5 counted about 675 B per call, one configuration, by structure).
/// 60 000 calls with 64-byte terms (one gateway's cap at a provider is 65 536), held in three steps, the process's
/// resident set read after each (`ps -o rss=`), so each step's growth belongs to one structure:
///
/// 1. **the call's two records in a node's KV** — the signed checkpoint and the signed window record, as the mint
///    writes them: held by the gateway and, once gossiped, by the provider;
/// 2. **what the gateway holds as issued** for the call;
/// 3. **what a provider's authority holds for the window** it admitted: the checkpoint, the revoked set and the window.
///
/// A gateway holds 1 + 2 per call until its collector's floor; a provider holds 1 + 3 per call until the window
/// ends (3) and the gateway's tombstone arrives (1). Not the soak's live traffic: nothing runs here, so no
/// registration, task or connection is counted. Run in release, `--ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement: cargo test --release --lib --features compliance,a2a the_memory_a_held_call_window_takes -- --ignored --nocapture"]
async fn the_memory_a_held_call_window_takes_is_measured_on_each_node() {
    const H: usize = 60_000;
    let n = lone(false).await;
    let (key, gateway, id) = (n.key(), n.principal(), n.a.node_id().clone());
    let pad = "t".repeat(CALL_TERM_MAX_BYTES - 8);
    let term = |i: usize| format!("{pad}{i:08}");
    assert_eq!(term(0).len(), CALL_TERM_MAX_BYTES);
    let now = n.now();
    let until = now + 3_000_000;
    let settle = || async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        rss_kib().expect("ps reports the resident set") as f64 * 1024.0
    };
    let rss0 = settle().await;
    let mut record_bytes = 0usize;
    for i in 0..H {
        let t = term(i);
        let cp = serde_json::to_vec(&call_issuer::test_sign_checkpoint(n.ctx(), &t, 1, now, false)).unwrap();
        let win = window_record(&key, &t, until);
        let (ck, wk) = (call_checkpoint_key(&gateway, &t), call_window_key(&gateway, &t));
        record_bytes += ck.len() + cp.len() + wk.len() + win.len();
        super::state_machine::write_owned_kv(n.ctx(), &ck, cp);
        super::state_machine::write_owned_kv(n.ctx(), &wk, win);
    }
    assert_eq!(n.call_keys(), 2 * H);
    let rss1 = settle().await;
    for i in 0..H {
        call_issuer::test_hold_issued(n.ctx(), &term(i), until);
    }
    assert_eq!(n.ctx().call_issuer.issued_terms().len(), H);
    let rss2 = settle().await;
    let (authority, members) = lone_authority(&id, &key);
    let resource = format!("tool:work@{id}");
    let args = serde_json::json!({ "aisle": 4 });
    for i in 0..H {
        let cm = call_mint::mint(&key, &gateway, &term(i), "tools/call", &resource, &args, now, until);
        let a = authority.assess_call(&gateway, "tools/call", &resource, &args, &cm, now + 1, &members, |_| None);
        assert!(established(&a), "{a:?}");
    }
    assert_eq!(authority.view_sizes(), (H, H, H), "one checkpoint, one revoked set and one window per admitted call");
    let rss3 = settle().await;
    let per = |a: f64, b: f64| (b - a) / H as f64;
    let (kv, issued, view) = (per(rss0, rss1), per(rss1, rss2), per(rss2, rss3));
    eprintln!(
        "i4-pr4 memory per held call: window, {H} calls, 64-byte terms, resident set by step: the call's two KV records {kv:.0} B (their keys and values are {} B); the gateway's issued entry {issued:.0} B; a provider's authority view {view:.0} B. A gateway therefore holds about {:.0} B per call it minted, a provider about {:.0} B per window it admitted. Resident set: {:.1} MiB at the start, {:.1} MiB at the end.",
        record_bytes / H, kv + issued, kv + view, rss0 / 1_048_576.0, rss3 / 1_048_576.0,
    );
    while authority.view_sizes().2 > 0 {
        authority.sweep(until + 1);
    }
    assert_eq!(authority.view_sizes(), (0, 0, 0), "past the window the authority holds nothing");
    n.a.shutdown().await;
}

/// **The soak's finding (2026-11-20): a cancel waits its turn at the gateway's WAL** — here, behind the collector's
/// deletes. A gateway with `[persistence]` under `SyncMode::Flush` mints 500 calls; once their windows and D-5's floor
/// have passed, its collector's pass (`call_issuer::collect`, what `start_collector` runs each minute) deletes their
/// 1 000 records in one go — each a WAL append, which under `Flush` the WAL's writer syncs one at a time, in arrival
/// order. A cancel asked for at that moment is a required-sync write (`revoke` → `kv_set_requiring_sync`), which is
/// **durable first: nothing is applied, gossiped or forwarded until its own sync returns** — so it waits for every
/// delete queued ahead of it, and the provider is not told meanwhile.
///
/// **The collector is one trigger, not the defect** (#816's review): the defect is one sync per synced append, served
/// in arrival order. Under `Flush` any burst of appends does the same (the reviewer: 5.9 s behind 1 000 plain owned
/// writes); under any mode required-sync writes queue behind each other (1.7 s behind 300 concurrent mints under
/// `Flush`, 1.25 s under the default `Async`, where the collector's deletes hold nothing: 15 ms). This test pins the
/// one trigger the soak met.
///
/// Asserted — **structure, not a wall clock**: the cancel is on disk within ten times what one asked just before the
/// pass took (and 25 ms), so a disk that syncs in a millisecond does not pass it unfixed. **It is not, today** —
/// 4.3 s on one run and 13.7 s on another against 5 and 8 ms quiet, on the machine of the dated run — during which
/// the call runs on, to its window if that comes first. That is why the 10⁵ soak fails over a `Flush` gateway.
/// **It is the test of the WAL's group commit (the plan's §9 O11), not of O10's fix** (#816's confirm round, R2): the
/// fix the maintainer chose for O10 forwards the cancel at once and holds only the caller's acknowledgement for the
/// disk, which changes when the provider is told and not when the revocation reaches disk — what this measures. O10's
/// own PR adds its own test: the provider's stop inside bound 1 during such a burst while the acknowledgement waits.
/// Ignored until O11 (`docs/operations/what-is-proven.md#call-cancel-behind-collector`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "fails today — the soak's finding: a revocation waits its turn at the WAL behind the collector's deletes; the test of WAL group commit (plan §9 O11); run it with -- --ignored --nocapture; un-ignore with O11"]
async fn a_cancel_is_on_disk_inside_bound_1_while_the_collector_deletes_a_backlog() {
    const BACKLOG: usize = 500;
    let n = lone_with(true, 1).await;
    let target = NodeId::new("127.0.0.1", 65_000).unwrap();
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "work", "arguments": {"aisle": 4}}}).to_string();
    let mint = || async {
        let (framed, minted) = frame_minting(n.ctx(), &target, ALICE, &[], MCP, Bytes::from(payload.clone()), None, None, None, None, n.now());
        let minted = minted.expect("minted");
        assert!(framed.is_some());
        let term = minted.term.clone();
        call_issuer::record(n.ctx(), minted).await.expect("on disk");
        term
    };
    for _ in 0..BACKLOG {
        mint().await;
    }
    assert_eq!(n.call_keys(), 2 * BACKLOG);
    // Past every window + 2·skew + `max_drain_secs` (1 s): the whole backlog is the collector's to delete.
    let floor = n.now() + 1_000 + 2 * SKEW_MS + 1_000;
    assert!(until(|| n.now() > floor, Duration::from_secs(10)).await);
    let (before, during) = (mint().await, mint().await);
    let asked = Instant::now();
    assert_eq!(call_issuer::revoke_call(n.ctx(), &before).await, Ok(true));
    let quiet = asked.elapsed();
    // The collector's pass: every expired pair deleted, the two fresh calls kept.
    let collected = call_issuer::collect(n.ctx(), n.now());
    assert_eq!(collected.call_records, 2 * BACKLOG, "the backlog is collected in one pass");
    let asked = Instant::now();
    assert_eq!(call_issuer::revoke_call(n.ctx(), &during).await, Ok(true), "the cancel is recorded");
    let behind = asked.elapsed();
    eprintln!("i4-pr4 finding: a cancel recorded with the WAL quiet took {quiet:?}; asked as the collector deleted {} records, {behind:?}", 2 * BACKLOG);
    n.a.shutdown().await;
    // Structure, not a wall clock (#816's review): against the quiet write, so a disk that syncs in a millisecond does
    // not pass it unfixed. A cancel that waits for its own sync, or for one more that was under way, costs a few
    // quiet writes; one that waits for a thousand costs a thousand.
    let allowed = quiet * 10 + Duration::from_millis(25);
    assert!(behind < allowed, "a cancel asked while the collector deleted {} records was on disk only after {behind:?} — {:.0} times the {quiet:?} it takes with the WAL quiet (allowed: ten times, and 25 ms)", 2 * BACKLOG, behind.as_secs_f64() / quiet.as_secs_f64());
}
