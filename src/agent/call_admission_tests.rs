//! Identity-and-lifecycle I4, PR 1b: the provider's admission of a call under its gateway's `call:` mandate — the
//! envelope's second slot (`cm`), `Decision.cancellable`, and `admit_after` registering the work under both guards.
//! One node is the gateway that frames each envelope (with its node key, minting through the test-only `call_mint`:
//! the gateway's own mint is the row's next PR) and the provider that serves it through its real serve loops.

use super::action_evaluator::{AeEvidence, Execution, RecordKind};
use super::gateway_authority::{arguments_digest, call_checkpoint_key, call_mint, mandate_operation, possession_request, ExecutionAuthority, PresentedMandate};
use super::gateway_caller::{frame_envelope_minting_at, node_principal};
use super::provider_enforcement::ENFORCEMENT_POINT_PROVIDER;
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A window no test here outlives: the node's real-clock sweep must not end a call under it on a slow host.
const LONG_WINDOW_MS: u64 = 3_000_000;
const MCP: &str = "mcp.invoke";
const SKILL: &str = "skill.invoke";
const CUSTOM: &str = "depot.custom";
const HOSTED: &str = "cap.invoke/depot/thing";
/// An operator's protected kind under a rule that is not cancellable.
const PLAIN_RPC: &str = "depot.plain";

struct Node {
    a: Arc<GossipAgent>,
    authority: Arc<ExecutionAuthority>,
    dir: std::path::PathBuf,
    http_port: u16,
    /// How many times a tool or the skill started, and how many ran to their end.
    started: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

impl Drop for Node {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("i4-admit-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// What goes in the envelope's second slot.
enum Cm {
    /// What an honest gateway mints: term `c{n}` of this envelope.
    Honest,
    /// Nothing: the slot stripped, or a gateway that does not mint.
    Stripped,
    /// A `cm` taken from another envelope.
    Lifted(serde_json::Value),
    /// Honest, with its checkpoint removed.
    NoCheckpoint,
    /// Honest, but issued and held under another spelling of this gateway's address.
    Alias,
    /// Honest in every field, with a window that ended before the call.
    Expired,
}

/// How a test's node differs from the usual one.
#[derive(Clone, Copy)]
struct Opts {
    /// A TLS identity. Without one the node signs no envelope and holds no key.
    tls: bool,
    /// `gateway_caller_profile = legacy`: the node does not promise an envelope on every RPC it originates, so a
    /// bare frame from it is its own call.
    legacy: bool,
    /// Attach the execution authority.
    authority: bool,
    /// Attach a cohort budget (exit 17: each serve path runs with and without one).
    budget: bool,
    /// Turn provider enforcement on.
    enforce: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { tls: true, legacy: false, authority: true, budget: false, enforce: true }
    }
}

async fn node() -> Node {
    node_with(Opts::default()).await
}

/// One node: gateway, provider and caller. Its policy: `slow` and the skill are **cancellable**; `plain` is not;
/// `both` is cancellable and needs a grant.
async fn node_with(opts: Opts) -> Node {
    let (port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("i4-admit-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    if opts.tls {
        cfg.tls = Some({
            let mut tls_cfg = crate::TlsConfig::default();
            tls_cfg.auto_cert_dir = dir.join("tls");
            tls_cfg
        });
    }
    if opts.legacy {
        cfg.gateway_caller_profile = crate::config::GatewayCallerProfile::Legacy;
    }
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![
        crate::config::GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] },
        crate::config::GatewayNamedToken { name: "sdk".into(), token: "sdk-secret".into(), scopes: vec!["mesh:serve".into()] },
        // A second serving principal (#806's review, M5: another principal's `stopped`).
        crate::config::GatewayNamedToken { name: "sdk2".into(), token: "sdk2-secret".into(), scopes: vec!["mesh:serve".into()] },
    ];
    // An operator's protected kind, served to an SDK agent over `rpc/serve` (the doors that cannot stop a call).
    cfg.protected_rpc_kinds = vec![CUSTOM.into(), PLAIN_RPC.into()];
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let me = id.to_string();
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-call")
            .with_catalogue("cat-call", "1")
            .map_action("tools/call", "*")
            .map_action("skill.invoke", "*")
            .allow(Rule::new("*", "tools/call", format!("tool:slow@{me}")).cancellable())
            .allow(Rule::new("*", "tools/call", format!("tool:plain@{me}")))
            .allow(Rule::new("*", "tools/call", format!("tool:both@{me}")).requiring_grant().cancellable())
            .allow(Rule::new("*", "skill.invoke", format!("skill:depot/long@{me}")).cancellable())
            // The doors that cannot stop a running call (the maintainer's decision of 2026-11-15): a bridged MCP
            // server's tool, a tool registered as unstoppable, a hosted capability's kind, an SDK-served kind — each
            // cancellable, and the unstoppable tool's twin not.
            .allow(Rule::new("*", "tools/call", format!("tool:remote@{me}")).cancellable())
            .allow(Rule::new("*", "tools/call", format!("tool:remote_plain@{me}")))
            .allow(Rule::new("*", "tools/call", format!("tool:hosted@{me}")).cancellable())
            .allow(Rule::new("*", "tools/call", format!("tool:hosted_plain@{me}")))
            .map_action("cap.invoke", "*")
            .allow(Rule::new("*", "cap.invoke", format!("cap:depot/thing@{me}")).cancellable())
            .map_action(CUSTOM, "*")
            .allow(Rule::new("*", CUSTOM, format!("res:x@{me}")).cancellable())
            .map_action(PLAIN_RPC, "*")
            .allow(Rule::new("*", PLAIN_RPC, format!("res:y@{me}"))),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(node_principal(&id)).unwrap());
    // F = 2 s: the sweep runs every 100 ms.
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, FreshnessPolicy { freshness_ms: 2_000, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()));
    if opts.authority {
        a.with_execution_authority(Arc::clone(&authority));
    }
    if opts.enforce {
        a.with_provider_enforcement();
    }
    if opts.budget {
        a.with_cohort_budget(crate::knowledge::cohort_budget::CohortBudget::new(8, 4), crate::knowledge::cohort::CohortView::trusting([]), TrustedExternalIssuers::new());
    }
    a.start().await.unwrap();
    let (started, finished) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    for (tool, secs) in [("slow", 30u64), ("both", 30), ("plain", 0)] {
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
    // A cooperative `rpc_rx` skill: its work ends when it finishes or when its authority lapses.
    std::mem::forget(a.capabilities().advertise_capability(crate::capability::Capability::new("depot", "long"), Duration::from_secs(600)));
    let (agent, s, f) = (Arc::clone(&a), Arc::clone(&started), Arc::clone(&finished));
    let mut rx = a.service().rpc_rx(SKILL).stops_when_authority_lapses();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let (agent, s, f) = (Arc::clone(&agent), Arc::clone(&s), Arc::clone(&f));
            tokio::spawn(async move {
                s.fetch_add(1, Ordering::SeqCst);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(30)) => { f.fetch_add(1, Ordering::SeqCst); agent.service().rpc_respond(&req, b"done".to_vec()); }
                    _ = req.authority_lapsed() => agent.service().rpc_respond(&req, b"stopped: authority lapsed".to_vec()),
                }
            });
        }
    });
    // Ready when the gateway's listener accepts and the skill is advertised. The tools (their KV entries are written
    // in `register_mcp_tool`) and the skill's receiver are registered synchronously above; the capability is not —
    // `advertise_capability` hands its first write to a spawned task, and until it lands the provider answers the
    // skill `resource_not_served`.
    assert!(until(|| std::net::TcpStream::connect(("127.0.0.1", http_port)).is_ok(), Duration::from_secs(20)).await, "the gateway is listening");
    super::capability_ops::until_advertised(&a.task_ctx, "depot", "long").await;
    Node { a, authority, dir, http_port, started, finished }
}

/// One call as the provider derives it: its kind, application payload, resource claim, and the operation, resource
/// and arguments a mandate binds.
struct Call {
    kind: &'static str,
    payload: Vec<u8>,
    claim: Option<String>,
    operation: &'static str,
    resource: String,
    arguments: serde_json::Value,
}

impl Node {
    fn gateway(&self) -> String {
        node_principal(self.a.node_id())
    }
    /// The node's key; on a node with no TLS identity, a key of the test's own (nothing there holds its public half).
    fn key(&self) -> Arc<ed25519_dalek::SigningKey> {
        self.a.task_ctx.tls.get().map(|t| t.signing_key()).unwrap_or_else(|| Arc::new(ed25519_dalek::SigningKey::from_bytes(&[61u8; 32])))
    }
    fn now(&self) -> u64 {
        self.a.task_ctx.hlc.decision_now_ms()
    }
    fn tool(&self, name: &str) -> Call {
        let arguments = serde_json::json!({ "aisle": 4 });
        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).to_string().into_bytes();
        Call { kind: MCP, payload, claim: None, operation: "tools/call", resource: format!("tool:{name}@{}", self.a.node_id()), arguments }
    }
    /// A hosted capability's kind, as the wasm host serves it (`rpc_rx`), decided as `cap.invoke` on its `cap:` resource.
    fn hosted(&self) -> Call {
        let arguments = serde_json::json!({ "n": 1 });
        Call { kind: HOSTED, payload: arguments.to_string().into_bytes(), claim: None, operation: "cap.invoke", resource: format!("cap:depot/thing@{}", self.a.node_id()), arguments }
    }
    /// The operator's protected kind under a plain rule.
    fn plain_kind(&self) -> Call {
        let resource = format!("res:y@{}", self.a.node_id());
        Call { kind: PLAIN_RPC, payload: b"work".to_vec(), claim: Some(resource.clone()), operation: PLAIN_RPC, resource, arguments: serde_json::json!({ "payload_b64": "d29yaw==" }) }
    }
    /// The operator's protected kind, as an SDK agent serves it.
    fn custom(&self) -> Call {
        let resource = format!("res:x@{}", self.a.node_id());
        Call { kind: CUSTOM, payload: b"work".to_vec(), claim: Some(resource.clone()), operation: CUSTOM, resource, arguments: serde_json::json!({ "payload_b64": "d29yaw==" }) }
    }
    fn skill(&self) -> Call {
        let resource = format!("skill:depot/long@{}", self.a.node_id());
        Call { kind: SKILL, payload: b"restock".to_vec(), claim: Some(resource.clone()), operation: "skill.invoke", resource, arguments: serde_json::json!({ "text": "restock" }) }
    }
    /// Frame `call` as this node's gateway does, with `cm` in the second slot and `grant` in the first. Returns the
    /// framed bytes, the envelope's id, and the `cm` it carries.
    fn frame(&self, call: &Call, grant: Option<&serde_json::Value>, cm: Cm) -> (Bytes, u64, Option<serde_json::Value>) {
        let (id, minted) = (std::cell::Cell::new(0u64), std::cell::RefCell::new(None));
        let honest = |gateway: &str, n: u64| {
            let now = self.now();
            serde_json::to_value(call_mint::mint(&self.key(), gateway, &format!("c{n}"), call.operation, &call.resource, &call.arguments, now, now + LONG_WINDOW_MS)).unwrap()
        };
        let mint = |n: u64| {
            id.set(n);
            let cm = match &cm {
                Cm::Honest => Some(honest(&self.gateway(), n)),
                Cm::Stripped => None,
                Cm::Expired => {
                    let now = self.now();
                    Some(serde_json::to_value(call_mint::mint(&self.key(), &self.gateway(), &format!("c{n}"), call.operation, &call.resource, &call.arguments, now - 2_000, now - 1_000)).unwrap())
                }
                Cm::Lifted(other) => Some(other.clone()),
                Cm::NoCheckpoint => {
                    let mut v = honest(&self.gateway(), n);
                    v.as_object_mut().unwrap().remove("checkpoint");
                    Some(v)
                }
                // The same address with a leading zero on the port: it parses to this node.
                Cm::Alias => Some(honest(&format!("node:127.0.0.1:0{}", self.a.node_id().to_string().rsplit(':').next().unwrap()), n)),
            };
            *minted.borrow_mut() = cm.clone();
            cm
        };
        let framed = frame_envelope_minting_at(&self.a.task_ctx, self.a.node_id(), &self.gateway(), &[], Bytes::from(call.payload.clone()), grant, call.claim.as_deref(), None, None, self.now(), Some(&mint)).expect("fits");
        (framed, id.get(), minted.into_inner())
    }
    /// Send a framed call to this node's own serve loop and return its reply.
    async fn send(&self, call: &Call, framed: Bytes) -> Vec<u8> {
        super::rpc::rpc_call_framed(&self.a.task_ctx, self.a.node_id().clone(), Arc::from(call.kind), framed, Duration::from_secs(60)).await.expect("a reply").to_vec()
    }
    async fn call(&self, call: &Call, grant: Option<&serde_json::Value>, cm: Cm) -> (Vec<u8>, u64) {
        let (framed, id, _) = self.frame(call, grant, cm);
        (self.send(call, framed).await, id)
    }
    /// The gateway cancels the call of envelope `n`: the revoking checkpoint, written where a provider reads it.
    fn cancel(&self, n: u64) {
        let term = format!("c{n}");
        let cp = call_mint::checkpoint(&self.key(), &self.gateway(), &format!("call:{term}"), 2, self.now(), &[term.as_str()]);
        super::state_machine::write_owned_kv(&self.a.task_ctx, &call_checkpoint_key(&self.gateway(), &term), serde_json::to_vec(&cp).unwrap());
    }
    /// A grant for `call` held by this node, its issuer's first checkpoint already at the authority: slot 1.
    fn grant(&self, call: &Call, request: &str) -> serde_json::Value {
        let (gw, digest) = (PrincipalId::new(self.gateway()).unwrap(), arguments_digest(&call.arguments));
        let m = Mandate {
            holder: gw.clone(),
            established_by: gw,
            purpose: format!("grant {request}"),
            scope: format!("grant:{request}"),
            operations: Mandate::grant_operations(mandate_operation(call.operation, &call.resource), &super::action_evaluator::hex32(&digest), None),
            epoch: 1,
            term: TermId::new(request).unwrap(),
            valid_from_ms: self.now(),
            valid_until_ms: self.now() + LONG_WINDOW_MS,
        };
        let proof = mycelium_core::tls::sign_bytes(&self.key(), &possession_message(&m, &possession_request(call.operation, &call.resource, &digest)));
        self.offer_grant_checkpoint(request, 1, &[]);
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&self.key(), &m.canonical_bytes()).to_vec(), mandate: m };
        serde_json::to_value(PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) }).unwrap()
    }
    fn offer_grant_checkpoint(&self, request: &str, seq: u64, revoked: &[&str]) {
        let cp = call_mint::checkpoint(&self.key(), &self.gateway(), &format!("grant:{request}"), seq, self.now(), revoked);
        let _ = self.authority.offer_checkpoint(&cp, self.now(), &super::gateway_member_keys(&self.a.task_ctx));
    }
    fn counts(&self) -> (usize, usize) {
        (self.started.load(Ordering::SeqCst), self.finished.load(Ordering::SeqCst))
    }
    /// The provider's execution records saying nothing ran.
    fn nothing_ran(&self) -> usize {
        crate::read_evidence_journal(&self.dir.join("evidence.log")).unwrap().iter().map(|b| serde_json::from_slice::<AeEvidence>(b).unwrap())
            .filter(|e| e.enforcement_point == ENFORCEMENT_POINT_PROVIDER && e.kind == RecordKind::Execution && e.execution == Execution::None)
            .count()
    }
    /// Wait until `k` handlers have started. Registration comes first and the handler is polled after it, so a test
    /// that cancels as soon as the work is registered can cancel a handler that has not begun (a flake under load).
    async fn began(&self, k: usize) {
        assert!(until(|| self.started.load(Ordering::SeqCst) == k, Duration::from_secs(20)).await, "started {} (wanted {k})", self.started.load(Ordering::SeqCst));
    }
    async fn running(&self, n: usize) {
        assert!(until(|| self.authority.running_count() == n, Duration::from_secs(20)).await, "running_count {} (wanted {n})", self.authority.running_count());
    }
    /// Wait until `n` calls are registered here while the call `answered` is still out — and if it is answered first,
    /// fail with its answer (#794's review, F1): a refusal names its code and reason, not only "running_count 0".
    async fn running_or_answered(&self, n: usize, answered: &mut tokio::task::JoinHandle<Vec<u8>>) {
        let done = until(|| self.authority.running_count() == n || answered.is_finished(), Duration::from_secs(20)).await;
        if done && self.authority.running_count() == n {
            return;
        }
        let reply = if answered.is_finished() { (&mut *answered).await.ok().map(|r| String::from_utf8_lossy(&r).into_owned()) } else { None };
        panic!("running_count {} (wanted {n}); the call's answer: {reply:?}", self.authority.running_count());
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

/// A reply as a JSON-RPC answer (`mcp.invoke`) or the plain receiver's refusal body (`{error, reason, data}`).
fn json(reply: &[u8]) -> serde_json::Value {
    serde_json::from_slice(reply).unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(reply) }))
}

/// The refusal D-2 promises, at an MCP reply: -32031, `authority_not_established`, `call_mandate_not_established`,
/// with the stable `data.cause` beside the sentence in `data.found`.
#[track_caller]
fn assert_not_established(reply: &[u8], cause: &str, found: &str) {
    let v = json(reply);
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["reason"].as_str(), v["error"]["data"]["detail"].as_str()), (Some(-32031), Some("authority_not_established"), Some("call_mandate_not_established")), "{v}");
    assert_eq!(v["error"]["data"]["cause"].as_str(), Some(cause), "{v}");
    assert!(v["error"]["data"]["found"].as_str().is_some_and(|f| f.contains(found)), "found {found:?} in {v}");
}

/// **Exits 1 and 4, D-2, at a live call through the native MCP serve loop.** A cancellable tool called with its
/// gateway's `call:` mandate runs, registered (`running_count` 1); the gateway's cancel stops it within the sweep's
/// bound, the caller is answered `cancelled`, and the stop record names the call's term. The same call with the
/// slot stripped, or with the checkpoint stripped from it, is refused -32031 `call_mandate_not_established`: the
/// tool does not run and the provider's journal says nothing ran. Under a rule that is **not** cancellable the
/// stripped call runs, unregistered, and the honest one runs registered. Seen failing first: with the D-2 refusal
/// taken out of `admit_after`, the stripped cancellable call runs — uncancellable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancellable_call_runs_only_registered_under_its_call_mandate_and_a_cancel_stops_it() {
    let n = Arc::new(node().await);
    let slow = n.tool("slow");
    // Honest: registered, then cancelled.
    let (framed, id, _) = n.frame(&slow, None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.tool("slow"));
    let mut running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut running).await;
    n.began(1).await;
    assert_eq!(n.counts(), (1, 0), "the tool is running");
    let asked = std::time::Instant::now();
    n.cancel(id);
    let reply = json(&running.await.unwrap());
    assert!(asked.elapsed() < Duration::from_secs(5), "stopped within the sweep's bound, not at the tool's 30 s: {:?}", asked.elapsed());
    assert!(reply.to_string().contains("cancelled: the call's authority lapsed while it ran"), "{reply}");
    n.running(0).await;
    assert_eq!(n.counts(), (1, 0), "the handler's future was dropped: it did not finish");
    assert_eq!(n.authority.named_stop_records().last().and_then(|s| s.term.clone()), Some(format!("c{id}")), "the stop record names the call's term");

    // Stripped, and with the checkpoint stripped: refused, nothing runs, and the record says so.
    let before = n.nothing_ran();
    let (reply, _) = n.call(&slow, None, Cm::Stripped).await;
    assert_not_established(&reply, "no_call_mandate", "carries no call: mandate");
    let (reply, _) = n.call(&slow, None, Cm::NoCheckpoint).await;
    assert_not_established(&reply, "malformed", "does not parse");
    assert_eq!((n.counts(), n.authority.running_count()), ((1, 0), 0), "neither ran");
    assert_eq!(n.nothing_ran(), before + 2, "an execution record for each says nothing ran");

    // A rule that is not cancellable: the stripped call runs, unregistered; the honest one is registered while it runs
    // (the tool returns at once, so the registration is seen by its having been released without a stop record).
    let plain = n.tool("plain");
    let (reply, _) = n.call(&plain, None, Cm::Stripped).await;
    assert_eq!(json(&reply)["error"], serde_json::Value::Null, "{}", json(&reply));
    let (reply, _) = n.call(&plain, None, Cm::Honest).await;
    assert_eq!(json(&reply)["error"], serde_json::Value::Null, "{}", json(&reply));
    assert_eq!(n.counts(), (3, 2), "both ran to their end");
    assert_eq!(n.authority.view_sizes().2, 2, "and a window is held for each honest call: the cancelled one's and this one's");
    n.a.shutdown().await;
}

/// **Q9, answered: a `call:` mandate fits only the envelope it was minted for.** `cm` is outside the gateway's
/// envelope signature, and its possession proof binds the operation, the resource and the arguments — not the call.
/// The gateway frames two identical calls; a relay puts the first envelope's `cm` — valid, the same gateway's, for
/// the same operation, resource and arguments — into the second. Were it accepted, the second call's work would be
/// registered under the first call's term and the gateway's cancel of the second would miss it. It is refused: the
/// term must be `c{n}` of the envelope that carries it, and `n` is inside the envelope's signature. Also at the
/// door: a mandate issued under **another spelling of the gateway's address** is not established — the provider
/// passes the envelope's verified `via` in its node's canonical spelling. Seen failing first: with the term check
/// taken out of `assess_call_slot`, the lifted `cm` is established, the call runs, and cancelling its own term
/// stops nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_mandate_lifted_from_another_envelope_of_the_same_gateway_is_not_established() {
    let n = Arc::new(node().await);
    let slow = n.tool("slow");
    let (_first, first_id, first_cm) = n.frame(&slow, None, Cm::Honest);
    // The relay's envelope: the gateway's second, honest in every signed field, carrying the first one's `cm`.
    let (framed, id, _) = n.frame(&slow, None, Cm::Lifted(first_cm.unwrap()));
    assert_ne!(id, first_id);
    let (n2, call) = (Arc::clone(&n), n.tool("slow"));
    let sent = tokio::spawn(async move { n2.send(&call, framed).await });
    // Had the swap worked, the tool would now be running under the first call's term: cancel this call's own term and
    // give the sweep time. The reply must be the refusal, not a cancellation and not a hang to the tool's 30 s.
    n.cancel(id);
    let reply = tokio::time::timeout(Duration::from_secs(10), sent).await.expect("answered at once: the call was refused, not run").unwrap();
    assert_not_established(&reply, "other_envelope", "minted for another call");
    assert_eq!((n.counts(), n.authority.running_count(), n.authority.view_sizes().2), ((0, 0), 0, 0), "nothing ran, nothing is registered, no window is held");

    // The non-canonical spelling, at the door.
    let (reply, _) = n.call(&slow, None, Cm::Alias).await;
    assert_not_established(&reply, "not_verified", "the gateway that sent it");
    assert_eq!(n.counts(), (0, 0));
    n.a.shutdown().await;
}

/// **N1 and exit 6, for a `call:` call (D-3): a cancel between the provider's preflight and its registration
/// refuses the call.** The call is framed, held at `before_admit_after` after its permit, the revoking checkpoint is
/// written, and it is resumed: the one assessment of slot 2 reads the revocation in this node's KV, the call is
/// refused -32031 `call_mandate_not_established`, the tool does not run and the journal says nothing ran. The same
/// with both slots present, under a rule that requires the grant. Seen failing first: with the refusal taken out,
/// the call runs unregistered — the spike's shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_cancelled_between_the_preflight_and_the_registration_is_refused() {
    use super::provider_enforcement::test_hooks::PAUSE;
    let n = Arc::new(node().await);
    // `plain` is the rule that is **not** cancellable: a revoked term refuses the call under any rule (the
    // maintainer, 2026-11-13). Seen failing first: with the refusal kept for cancellable rules only, `plain` runs.
    for (tool, with_grant) in [("slow", false), ("both", true), ("plain", false)] {
        let call = n.tool(tool);
        let grant = with_grant.then(|| n.grant(&call, &format!("r-{tool}")));
        let (framed, id, _) = n.frame(&call, grant.as_ref(), Cm::Honest);
        let (reached, resume) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        PAUSE.pin().insert(n.a.node_id().to_string(), (Arc::clone(&reached), Arc::clone(&resume)));
        let before = n.nothing_ran();
        let (n2, call2) = (Arc::clone(&n), n.tool(tool));
        let sent = tokio::spawn(async move { n2.send(&call2, framed).await });
        tokio::time::timeout(Duration::from_secs(20), reached.notified()).await.expect("the call reached the window after its preflight");
        n.cancel(id);
        resume.notify_one();
        assert_not_established(&sent.await.unwrap(), "revoked", "revoked");
        assert_eq!((n.counts(), n.authority.running_count()), ((0, 0), 0), "{tool}: the tool did not run, and nothing is left registered");
        assert_eq!(n.nothing_ran(), before + 1, "{tool}: the journal says nothing ran");
    }
    n.a.shutdown().await;
}

/// **N4 and exit 2, at a live call: both slots are assessed and both are registered.** A call under a client's
/// grant (slot 1, assessed against the caller) and the gateway's `call:` mandate (slot 2, against `node:{via}`) is
/// registered twice — `running_count` is 2 for the one call. Revoking the grant stops it, and the stop record names
/// the grant; a second such call, cancelled, stops with a record naming the `call:` term. The `call:` mandate alone
/// does not meet the rule's `requires_mandate`: without the grant the call is refused before it reaches the
/// registration. Seen failing first: with one registration (the `call:` one not made), a cancel stops nothing and
/// `running_count` is 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_under_a_grant_and_a_call_mandate_is_registered_under_both_and_either_stops_it() {
    let n = Arc::new(node().await);
    let both = n.tool("both");
    // The `call:` mandate is no one's authority: with it alone, the grant-requiring rule is not met.
    let (reply, _) = n.call(&both, None, Cm::Honest).await;
    assert_eq!(json(&reply)["error"]["data"]["reason"], "authority_not_established", "{}", json(&reply));
    assert_ne!(json(&reply)["error"]["data"]["detail"], "call_mandate_not_established", "refused by the policy, not by D-2");
    assert_eq!(n.counts(), (0, 0));

    for (round, by_grant) in [(1, true), (2, false)] {
        let request = format!("r{round}");
        let grant = n.grant(&both, &request);
        let (framed, id, _) = n.frame(&both, Some(&grant), Cm::Honest);
        let (n2, call) = (Arc::clone(&n), n.tool("both"));
        let mut sent = tokio::spawn(async move { n2.send(&call, framed).await });
        n.running_or_answered(2, &mut sent).await;
        n.began(round).await;
        if by_grant {
            n.offer_grant_checkpoint(&request, 2, &[request.as_str()]);
        } else {
            n.cancel(id);
        }
        let reply = json(&sent.await.unwrap());
        assert!(reply.to_string().contains("cancelled: the call's authority lapsed while it ran"), "{reply}");
        n.running(0).await;
        let stop = n.authority.named_stop_records().last().cloned().expect("a stop record");
        let named = if by_grant { (Some(format!("grant:{request}")), Some(request.clone())) } else { (Some(format!("call:c{id}")), Some(format!("c{id}"))) };
        assert_eq!((stop.scope, stop.term), named, "round {round}: the record names the mandate whose lapse stopped the work");
        assert_eq!(n.authority.named_stop_records().len(), round, "one record a call: only the cancelled guard writes one");
    }
    assert_eq!(n.counts(), (2, 0));
    n.a.shutdown().await;
}

/// **Exit 17, the in-process half: a cancel reaches an `rpc_rx` serve loop.** A cooperative skill races
/// `RpcRequest::authority_lapsed()`. Its call carries only the gateway's `call:` mandate — no caller mandate — and
/// a cancel ends it: the skill answers `stopped: authority lapsed`. And the D-2 refusal at this door: the same
/// skill call with the slot stripped is refused `call_mandate_not_established` and never reaches the skill. Seen
/// failing first: with `authority_lapsed()` awaiting the first registration only, the skill runs to its own end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_reaches_an_in_process_serve_loop_through_authority_lapsed() {
    let n = Arc::new(node().await);
    let skill = n.skill();
    let (framed, id, _) = n.frame(&skill, None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.skill());
    let mut sent = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut sent).await;
    n.began(1).await;
    n.cancel(id);
    let reply = tokio::time::timeout(Duration::from_secs(10), sent).await.expect("the skill was told its authority lapsed").unwrap();
    assert_eq!(reply, b"stopped: authority lapsed");
    assert_eq!(n.counts(), (1, 0));

    let (reply, _) = n.call(&skill, None, Cm::Stripped).await;
    let v = json(&reply);
    assert_eq!((v["reason"].as_str(), v["data"]["detail"].as_str()), (Some("authority_not_established"), Some("call_mandate_not_established")), "{v}");
    assert_eq!(n.counts(), (1, 0), "the skill never saw it");
    n.a.shutdown().await;
}

/// **Every other way a call reaches a cancellable rule without a `call:` mandate is refused, by the same name.**
/// A gateway that does not mint (`gateway_mint_call_mandates` off, the default), so each of these doors carries none:
///
/// - **the gateway's `/mcp`**: its preflight permits and carries the rule's `cancellable`; the provider refuses;
/// - **a member's own call** (`ServiceHandle::rpc_call`, a self envelope with no second slot);
/// - **an unframed call** — no envelope at all, so nothing could carry one (a member's own unframed call is not
///   cancellable under I4: not built, by the maintainer's decision);
/// - **a provider with no execution authority** is covered by `assess_call_slot`'s own answer, asserted by reading.
///
/// In each the tool does not run. Seen failing first: with the D-2 refusal taken out, each runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancellable_rule_refuses_a_call_from_every_door_that_carries_no_call_mandate() {
    let n = node().await;
    let slow = n.tool("slow");
    // The gateway's /mcp door.
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "slow", "arguments": slow.arguments}});
    let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/mcp", n.http_port)).bearer_auth("alice-secret").json(&body).send().await.expect("gateway");
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["detail"].as_str()), (Some(-32031), Some("call_mandate_not_established")), "/mcp: {v}");
    // A member's own call: a self envelope, no second slot.
    let reply = n.a.service().rpc_call(n.a.node_id().clone(), MCP, slow.payload.clone(), Duration::from_secs(20)).await.expect("a reply");
    assert_not_established(&reply, "no_call_mandate", "carries no call: mandate");
    // An unframed call: the bare payload, no envelope.
    let reply = super::rpc::rpc_call_framed(&n.a.task_ctx, n.a.node_id().clone(), Arc::from(MCP), Bytes::from(slow.payload.clone()), Duration::from_secs(20)).await.expect("a reply");
    // This node promises an envelope on every RPC it originates, so its bare frame is refused at the caller context,
    // before any rule is read. (The unframed call that reaches the rule is the legacy sender's, in the next test.)
    let v = json(&reply);
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["reason"].as_str()), (Some(-32022), Some("missing")), "{v}");
    assert_eq!((n.counts(), n.authority.running_count()), ((0, 0), 0), "none of them ran");
    n.a.shutdown().await;
}

/// One cancellable call at a node built with `opts`, refused by `cause`, and the tool not run. `bare`: sent with no
/// envelope at all.
async fn refused_by_cause(opts: Opts, bare: bool, cm: Cm, cause: &str, found: &str) {
    let n = node_with(opts).await;
    let slow = n.tool("slow");
    let reply = if bare {
        super::rpc::rpc_call_framed(&n.a.task_ctx, n.a.node_id().clone(), Arc::from(MCP), Bytes::from(slow.payload.clone()), Duration::from_secs(20)).await.expect("a reply").to_vec()
    } else {
        n.call(&slow, None, cm).await.0.to_vec()
    };
    assert_not_established(&reply, cause, found);
    assert_eq!((n.counts(), n.authority.running_count()), ((0, 0), 0));
    n.a.shutdown().await;
}

/// **A member's own unframed call under a cancellable rule is refused, `no_envelope`** (#781's review, F3). The
/// sender does not promise envelopes (`gateway_caller_profile = legacy`), so its bare frame is read as the node's
/// own call, the rule permits it, and there is no envelope to carry a `call:` mandate: the unframed call that is not
/// cancellable under I4. Seen failing first: with the D-2 refusal out, it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unframed_call_from_a_sender_that_promises_no_envelope_is_refused_under_a_cancellable_rule() {
    refused_by_cause(Opts { legacy: true, ..Opts::default() }, true, Cm::Stripped, "no_envelope", "no caller envelope").await;
}

/// **A provider with enforcement on and no execution authority refuses a cancellable call, `no_authority`**, though
/// the envelope's `cm` is honest (F3). Seen failing first: with the D-2 refusal out, it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_with_no_execution_authority_refuses_a_cancellable_call() {
    refused_by_cause(Opts { authority: false, ..Opts::default() }, false, Cm::Honest, "no_authority", "no execution authority").await;
}

/// **A `cm` that is JSON and not a mandate is `malformed`** (F3). Seen failing first: with the D-2 refusal out, the
/// call runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cm_that_is_json_and_not_a_mandate_is_not_established() {
    refused_by_cause(Opts::default(), false, Cm::Lifted(serde_json::json!("not an object")), "malformed", "does not parse").await;
}

/// **The slot is assessed only for an envelope its gateway signed** (#781's review, F7). A node with no TLS identity
/// carries its envelope unsigned, so the envelope's id — what binds a `cm` to its call — is not authenticated, and a
/// relay could set it to fit a lifted `cm`. Seen failing first: with the check out, the cause is `not_verified` —
/// refused only because nothing there happens to hold a key for the gateway.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_mandate_in_an_unsigned_envelope_is_not_established() {
    refused_by_cause(Opts { tls: false, ..Opts::default() }, false, Cm::Honest, "unsigned_envelope", "not signed by its gateway").await;
}

/// **A `call_mandate_not_established` refusal does not spend the caller's grant** (#781's review, F4). Under a rule
/// that needs a grant and is cancellable, a call with a valid grant and its `cm` stripped is refused — and the same
/// grant, presented again with an honest `cm`, is accepted and runs: the provider gave the use back, because it
/// refused before anything ran. Otherwise a relay that strips `cm` would turn an approved grant into a spent one.
/// Seen failing first: with the release taken out, the second
/// presentation is refused -32030, the grant already used at the provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_refused_for_its_call_mandate_gives_the_grant_back() {
    let n = Arc::new(node().await);
    let both = n.tool("both");
    let grant = n.grant(&both, "r-kept");
    let (reply, _) = n.call(&both, Some(&grant), Cm::Stripped).await;
    assert_not_established(&reply, "no_call_mandate", "carries no call: mandate");
    // The same grant, an honest envelope: it runs.
    let (framed, id, _) = n.frame(&both, Some(&grant), Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.tool("both"));
    let mut sent = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(2, &mut sent).await;
    n.began(1).await;
    n.cancel(id);
    assert!(json(&sent.await.unwrap()).to_string().contains("cancelled: the call's authority lapsed while it ran"));
    // And it is one use: a third presentation is the grant's second, refused as used.
    let (reply, _) = n.call(&both, Some(&grant), Cm::Honest).await;
    assert!(json(&reply)["error"]["message"].as_str().is_some_and(|m| m.contains("already used")), "{}", json(&reply));

    n.a.shutdown().await;
}

fn answer(detail: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32031, "message": "x", "data": {"reason": "authority_not_established", "detail": detail}}})
}

/// **The gateway's door does NOT give its claim back on this answer** (#781's confirm round, N1), nor on any other
/// -32031. A provider at 2.40 or before that bridges an MCP server forwards the server's reply in this shape
/// unrewritten, so a gateway that honoured it would let a server that ran its effect have the grant run again. The
/// provider gives its own claim back, which trusts no reply; the gateway's stays spent. Seen failing first: against
/// the gateway half as first built, the answer gave the grant back.
#[test]
fn the_gateway_gives_no_grant_back_on_call_mandate_not_established_or_any_authority_refusal() {
    assert_eq!(super::http::pre_execution_refusal(answer("call_mandate_not_established").to_string().as_bytes()), None);
    assert_eq!(super::http::pre_execution_refusal(answer("mandate_lapsed_before_run").to_string().as_bytes()), None);
}

/// **A `call:` mandate whose window has ended is `refused`** (the cause for a mandate that verifies and is refused
/// for another reason than revocation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_mandate_out_of_its_window_is_refused_by_its_cause() {
    refused_by_cause(Opts::default(), false, Cm::Expired, "refused", "refused").await;
}

/// **The set of causes is closed, and so is the counter's label set** (#781's confirm round): `data.cause` and the
/// `cause` label of `mycelium_provider_call_mandate_refusals_total` are one function, and these ten names are what
/// §104 documents and what a caller matches. Renaming one, or adding one without listing it here and there, fails.
/// Seen failing first with `no_call_mandate` renamed.
#[test]
fn the_causes_of_a_call_mandate_refusal_are_a_closed_list() {
    use super::provider_enforcement::CallCause;
    // Exhaustive, with no wildcard: a new variant stops this compiling until it is listed here. `ALL` is not forced the same way: a variant left out of it compiles and is unpinned, so add it there too.
    for c in CallCause::ALL {
        match c {
            CallCause::Revoked | CallCause::NoEnvelope | CallCause::NoCallMandate | CallCause::UnsignedEnvelope | CallCause::NoAuthority => {}
            CallCause::Malformed | CallCause::NoEnvelopeId | CallCause::OtherEnvelope | CallCause::Refused | CallCause::NotVerified => {}
        }
    }
    let labels: Vec<&str> = CallCause::ALL.iter().map(|c| c.label()).collect();
    assert_eq!(labels, ["revoked", "no_envelope", "no_call_mandate", "unsigned_envelope", "no_authority", "malformed", "no_envelope_id", "other_envelope", "refused", "not_verified"]);
}

/// **Only the node authors that answer** (F4): a bridged server's reply in its shape becomes an ordinary tool error,
/// which gives nothing back — or a server that ran its effect could have it run twice. Seen failing first: with the
/// clause out of `as_tool_answer`, the server's reply passes through with its `data`. No gateway honours the shape
/// today (N1); the rewrite is what lets a later release do so.
#[test]
fn a_bridged_servers_reply_shaped_like_call_mandate_not_established_gives_nothing_back() {
    let rewritten = super::mcp::as_tool_answer(answer("call_mandate_not_established"), &serde_json::json!(1));
    assert_eq!((rewritten["error"]["code"].as_i64(), rewritten["error"]["data"].is_null()), (Some(-32000), true), "{rewritten}");
    assert_eq!(super::http::pre_execution_refusal(rewritten.to_string().as_bytes()), None);
}

/// The refusal of a door that cannot stop a running call: -32031, `authority_not_established`, `door_cannot_stop`,
/// with the door's stable name in `data.door`. `v` is a JSON-RPC answer or a plain receiver's refusal body.
#[track_caller]
fn assert_door_cannot_stop(v: &serde_json::Value, door: &str) {
    let data = if v["error"].is_object() { &v["error"]["data"] } else { &v["data"] };
    assert_eq!((data["reason"].as_str(), data["detail"].as_str(), data["door"].as_str()), (Some("authority_not_established"), Some("door_cannot_stop"), Some(door)), "{v}");
}

/// **The doors that cannot stop a running call refuse a call under a cancellable rule, by name** (the maintainer's
/// decision of 2026-11-15, until I4's PR 3 builds their stop): the **bridged MCP loop** (an external MCP server's
/// tool), a tool registered with **`register_mcp_tool_unstoppable`** (the wasm host's bridge: a begun guest call is
/// never interrupted), a **hosted capability's kind** through `rpc_rx`, and the **SDK serve stream**. Each call
/// carries an honest `cm`, so without this refusal it would be admitted and registered where a cancel claims a stop it
/// cannot make. In each case nothing is forwarded, streamed or run, and the provider's journal says nothing ran; the
/// unstoppable tool's twin under a rule that is **not** cancellable runs. Seen failing first: with the refusal taken
/// out of `admit_after`, the bridged call was forwarded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_doors_that_cannot_stop_a_call_refuse_one_under_a_cancellable_rule_by_name() {
    let n = node().await;
    let before = n.nothing_ran();
    // The bridged MCP loop.
    let (mock_port, _mock) = super::mcp::tests::spawn_mock_mcp_server(vec![serde_json::json!({"name": "remote", "inputSchema": {"type": "object"}})]).await;
    let _bridge = n.a.mcp().connect_mcp_server(format!("http://127.0.0.1:{mock_port}/")).await.expect("bridged");
    let (reply, _) = n.call(&n.tool("remote"), None, Cm::Honest).await;
    assert_door_cannot_stop(&json(&reply), "bridged_mcp");
    assert!(!String::from_utf8_lossy(&reply).contains("echo:remote"), "nothing was forwarded");
    // A tool whose handler cannot stop what it started.
    let ran = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&ran);
    let _hosted = n.a.mcp().register_mcp_tool_unstoppable("hosted", serde_json::json!({"type": "object"}), move |_| {
        r.fetch_add(1, Ordering::SeqCst);
        async { Ok(serde_json::json!("ran")) }
    });
    let r = Arc::clone(&ran);
    let _plain = n.a.mcp().register_mcp_tool_unstoppable("hosted_plain", serde_json::json!({"type": "object"}), move |_| {
        r.fetch_add(1, Ordering::SeqCst);
        async { Ok(serde_json::json!("ran")) }
    });
    let (reply, _) = n.call(&n.tool("hosted"), None, Cm::Honest).await;
    assert_door_cannot_stop(&json(&reply), "unstoppable_mcp_tool");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the unstoppable tool did not run under the cancellable rule");
    let (reply, _) = n.call(&n.tool("hosted_plain"), None, Cm::Honest).await;
    assert_eq!(json(&reply)["error"], serde_json::Value::Null, "under a plain rule it runs: {}", json(&reply));
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // A hosted capability's kind, through `rpc_rx` (the check runs inside `recv`): the receiver is handed nothing.
    let mut rx = n.a.service().rpc_rx(HOSTED);
    let handed = Arc::new(AtomicUsize::new(0));
    let h = Arc::clone(&handed);
    let receiver = tokio::spawn(async move {
        while rx.recv().await.is_some() {
            h.fetch_add(1, Ordering::SeqCst);
        }
    });
    let hosted = n.hosted();
    let (reply, _) = n.call(&hosted, None, Cm::Honest).await;
    assert_door_cannot_stop(&json(&reply), "hosted_capability");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(handed.load(Ordering::SeqCst), 0, "the hosted kind's receiver was handed nothing");
    receiver.abort();
    // The SDK serve stream, read as an SDK agent reads it: the request is answered, never streamed.
    let mut stream = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/gateway/rpc/serve/{CUSTOM}", n.http_port))
        .bearer_auth("sdk-secret")
        .send()
        .await
        .expect("the serve stream opens");
    assert_eq!(stream.status(), 200);
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    let into = Arc::clone(&seen);
    let reader = tokio::spawn(async move {
        while let Ok(Some(c)) = stream.chunk().await {
            into.lock().unwrap().push_str(&String::from_utf8_lossy(&c));
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (reply, _) = n.call(&n.custom(), None, Cm::Honest).await;
    assert_door_cannot_stop(&json(&reply), "sdk_serve");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let streamed = seen.lock().unwrap().clone();
    assert!(!streamed.contains("d29yaw") && !streamed.contains("nonce"), "nothing was streamed: {streamed}");
    reader.abort();
    assert_eq!(n.authority.running_count(), 0, "nothing was registered");
    assert_eq!(n.nothing_ran(), before + 4, "an execution record for each refusal says nothing ran");
    n.a.shutdown().await;
}

/// An `rpc_rx` serve loop on `kind` that answers `"done"` once `work` has passed — or, if `races`, `"stopped"` 300 ms
/// after the call's authority lapses (a receiver that stops and tidies up first). Returns how many requests it was
/// handed.
fn serve_rpc(n: &Node, kind: &'static str, declared: bool, races: bool, work: Duration) -> Arc<AtomicUsize> {
    let rx = n.a.service().rpc_rx(kind);
    let mut rx = if declared { rx.stops_when_authority_lapses() } else { rx };
    let handed = Arc::new(AtomicUsize::new(0));
    let (h, agent) = (Arc::clone(&handed), Arc::clone(&n.a));
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            h.fetch_add(1, Ordering::SeqCst);
            let agent = Arc::clone(&agent);
            tokio::spawn(async move {
                if races {
                    tokio::select! {
                        _ = tokio::time::sleep(work) => agent.service().rpc_respond(&req, b"done".to_vec()),
                        _ = req.authority_lapsed() => {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            agent.service().rpc_respond(&req, b"stopped".to_vec());
                        }
                    }
                } else {
                    tokio::time::sleep(work).await;
                    agent.service().rpc_respond(&req, b"done".to_vec());
                }
            });
        }
    });
    handed
}

/// **An `rpc_rx` receiver that has not declared it stops is refused a call under a cancellable rule, by name, and
/// served one under any other rule** (the maintainer's decision of 2026-11-15: a receiver promises only its own
/// behaviour, and the node serves a cancellable call only where that promise was made). The undeclared receiver of
/// the cancellable kind is handed nothing and the caller is answered -32031 `door_cannot_stop`, `data.door`
/// `rpc_rx`; the undeclared receiver of a kind under a plain rule is handed the call, which runs. Seen failing first:
/// with `Door::RpcRx` answering as a door that stops whatever the receiver declared, the cancellable call was handed
/// to the undeclared receiver.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_rpc_rx_receiver_is_refused_a_cancellable_call_and_served_a_plain_one() {
    let n = node().await;
    let custom = serve_rpc(&n, CUSTOM, false, false, Duration::ZERO);
    let plain = serve_rpc(&n, PLAIN_RPC, false, false, Duration::ZERO);
    let before = n.nothing_ran();
    let (reply, _) = n.call(&n.custom(), None, Cm::Honest).await;
    assert_door_cannot_stop(&json(&reply), "rpc_rx");
    let (reply, _) = n.call(&n.plain_kind(), None, Cm::Honest).await;
    assert_eq!(reply, b"done".to_vec(), "{}", String::from_utf8_lossy(&reply));
    assert_eq!((custom.load(Ordering::SeqCst), plain.load(Ordering::SeqCst)), (0, 1));
    assert_eq!(n.nothing_ran(), before + 1, "the refusal says nothing ran");
    n.a.shutdown().await;
}

/// **A receiver that declared it stops is served a cancellable call and stops it on the gateway's revoke, uncounted;
/// one that declared and ignores the lapse is counted** (the assessment, not enforcement: nothing is refused). The
/// declared, racing receiver stops and answers `stopped` 300 ms after the lapse — inside `LAPSE_ANSWER_GRACE_MS`, so
/// `rpc_rx_answered_after_lapse` stays at zero. A second declared receiver keeps working past the lapse and answers
/// `done` 2.5 s after the revoke: counted once under its kind. Seen failing first: with `assess_answer` returning at
/// once the late answer was not counted, and with the grace at zero the racing receiver's answer was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_rpc_rx_receiver_is_served_and_stopped_and_one_that_ignores_the_lapse_is_counted() {
    let n = Arc::new(node().await);
    let late = |n: &Node| n.a.task_ctx.rpc_rx_late_answers.pin().get(CUSTOM).copied().unwrap_or(0);
    // Declared and racing: served, registered, stopped by the revoke within the grace.
    let handed = serve_rpc(&n, CUSTOM, true, true, Duration::from_secs(30));
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut running).await;
    let asked = std::time::Instant::now();
    n.cancel(id);
    let reply = running.await.unwrap();
    assert_eq!(reply, b"stopped".to_vec(), "{}", String::from_utf8_lossy(&reply));
    assert!(asked.elapsed() < Duration::from_secs(5), "{:?}", asked.elapsed());
    assert_eq!((handed.load(Ordering::SeqCst), late(&n)), (1, 0), "an answer racing the cancel is not a broken promise");
    n.running(0).await;
    n.a.shutdown().await;

    // Declared and ignoring the lapse: served too (the node does not check the promise), and its late answer counted.
    let n = Arc::new(node().await);
    let _handed = serve_rpc(&n, CUSTOM, true, false, Duration::from_secs(3));
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut running).await;
    n.cancel(id);
    let reply = running.await.unwrap();
    assert_eq!(reply, b"done".to_vec(), "the receiver ignored the lapse and answered when its work ended");
    assert_eq!(late(&n), 1, "counted under its kind");
    n.a.shutdown().await;
}

/// **A stop is recorded as confirmed only where the agent doing the work promised to stop** (#794's review, the
/// promise-theory finding). An undeclared `rpc_rx` receiver of a kind under a plain rule is served a call with an
/// honest `call:` mandate; the gateway cancels it; the receiver ignores the lapse and answers when its work ends. Its
/// stop record says the stop was asked (`requested_ms`) and nothing more — no acknowledgement, no confirmation:
/// the library saw the request released, not the work stopped. A declared receiver's stop is confirmed. Seen
/// failing first: with every dropped, cancelled guard recording `confirmed_ms = now`, the undeclared receiver's work,
/// which ran to its end, was recorded as a confirmed stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_receivers_stop_is_a_release_and_a_declared_ones_is_confirmed() {
    let n = Arc::new(node().await);
    let _plain = serve_rpc(&n, PLAIN_RPC, false, false, Duration::from_millis(1_500));
    let (framed, id, _) = n.frame(&n.plain_kind(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.plain_kind());
    let mut running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut running).await;
    n.cancel(id);
    assert_eq!(running.await.unwrap(), b"done".to_vec(), "the undeclared receiver ran to its end");
    n.running(0).await;
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert!(stop.record.requested_ms.is_some(), "{stop:?}");
    assert_eq!((stop.record.acknowledged_ms, stop.record.confirmed_ms), (None, None), "a release, not a confirmed stop: {stop:?}");
    // The declared receiver's stop is its own promise kept: confirmed.
    let _custom = serve_rpc(&n, CUSTOM, true, true, Duration::from_secs(30));
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut running).await;
    n.cancel(id);
    assert_eq!(running.await.unwrap(), b"stopped".to_vec());
    n.running(0).await;
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert!(stop.record.acknowledged_ms.is_some() && stop.record.confirmed_ms.is_some(), "{stop:?}");
    n.a.shutdown().await;
}

// ── I4, PR 3b: the node's own stop at the bridged MCP loop and the SDK serve stream ───────────────────────────────

/// The bound on every HTTP exchange these tests make (#806's third round: a failure is a named failure, never a
/// hang — a `read_ahead` plant once opened a stream whose body a test then read without one, for 16 hours).
const HTTP_BOUND: Duration = Duration::from_secs(30);

/// A client whose every request ends within [`HTTP_BOUND`]. Not for a serve stream, which is meant to stay open.
fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(HTTP_BOUND).build().expect("a client")
}

/// The provider's receipt for a cancel of `c{n}`: the gateway's revoking checkpoint sent to this node as the
/// cancel-forward (`sys.call-cancel.v1`) — one node is the gateway and the provider here — and the class answered.
async fn forward_cancel(n: &Node, id: u64) -> String {
    let term = format!("c{id}");
    let cp = call_mint::checkpoint(&n.key(), &n.gateway(), &format!("call:{term}"), 2, n.now(), &[term.as_str()]);
    let payload = serde_json::to_vec(&serde_json::json!({ "v": 1, "checkpoint": cp })).unwrap();
    let reply = super::rpc::rpc_call_ctx(&n.a.task_ctx, n.a.node_id().clone(), Arc::from(super::call_cancel::CANCEL_RPC_KIND), Bytes::from(payload), Duration::from_secs(10)).await.expect("the provider answers");
    let v: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    v["class"].as_str().unwrap_or_default().to_string()
}

/// A mock MCP server whose one tool, `remote_plain`, answers its **first** call only after `hold` (or never, within a
/// test) and any later one at once: it records the id of each `tools/call` it was sent and each
/// `notifications/cancelled`.
struct SlowServer {
    port: u16,
    calls: Arc<AtomicUsize>,
    ids: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    cancelled: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

async fn slow_mcp_server(hold: Duration) -> SlowServer {
    use axum::{extract::Json as AJson, routing::post, Router};
    let (calls, cancelled) = (Arc::new(AtomicUsize::new(0)), Arc::new(std::sync::Mutex::new(Vec::new())));
    let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (c, x, i) = (Arc::clone(&calls), Arc::clone(&cancelled), Arc::clone(&ids));
    let app = Router::new().route("/", post(move |AJson(req): AJson<serde_json::Value>| {
        let (c, x, i) = (Arc::clone(&c), Arc::clone(&x), Arc::clone(&i));
        async move {
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => AJson(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "slow", "version": "0"}}})),
                "tools/list" => AJson(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "remote_plain", "inputSchema": {"type": "object"}}]}})),
                "tools/call" => {
                    i.lock().unwrap().push(id.clone());
                    if c.fetch_add(1, Ordering::SeqCst) == 0 {
                        tokio::time::sleep(hold).await;
                    }
                    AJson(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "late:remote_plain"}]}}))
                }
                "notifications/cancelled" => {
                    x.lock().unwrap().push(req["params"].clone());
                    AJson(serde_json::json!({}))
                }
                _ => AJson(serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})),
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    SlowServer { port, calls, ids, cancelled }
}

fn door_stops(n: &Node, door: &'static str) -> u64 {
    n.a.task_ctx.door_stops.pin().get(door).copied().unwrap_or(0)
}

/// **The bridged MCP loop's own stop** (I4, PR 3b; the maintainer's decision (C) of 2026-11-16). A call under a rule
/// that is **not** cancellable is forwarded to a bridged server whose tool answers only after 20 s. The gateway
/// cancels the call (the cancel-forward): the node stops waiting at once — the requester is answered `cancelled`,
/// `data.door` `bridged_mcp`, well before the server would answer, and never with the server's late reply — and
/// sends the server `notifications/cancelled` for the request's id. It promises nothing about the work in the server:
/// the receipt is `Unbounded` and the stop record has no confirmation. Seen failing first with `forward_bridged`'s
/// `stop` arm taken out: the requester waited on the server's reply and the cancel stopped nothing the client saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bridged_loop_stops_waiting_tells_the_server_and_promises_nothing_about_its_work() {
    let n = Arc::new(node().await);
    let server = slow_mcp_server(Duration::from_secs(20)).await;
    let _bridge = n.a.mcp().connect_mcp_server(format!("http://127.0.0.1:{}/", server.port)).await.expect("bridged");
    let (framed, id, _) = n.frame(&n.tool("remote_plain"), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.tool("remote_plain"));
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    assert!(until(|| server.calls.load(Ordering::SeqCst) == 1, Duration::from_secs(10)).await, "the call was forwarded");
    let asked = std::time::Instant::now();
    assert_eq!(forward_cancel(&n, id).await, "unbounded", "the bridged server made no promise the node can confirm");
    let reply = tokio::time::timeout(Duration::from_secs(5), answered).await.expect("answered before the server would be").unwrap();
    let v = json(&reply);
    assert_eq!((v["error"]["data"]["reason"].as_str(), v["error"]["data"]["door"].as_str()), (Some("cancelled"), Some("bridged_mcp")), "{v}");
    assert!(!String::from_utf8_lossy(&reply).contains("late:"), "the server's reply is not the answer");
    assert!(asked.elapsed() < Duration::from_secs(5), "{:?}", asked.elapsed());
    assert!(until(|| server.cancelled.lock().unwrap().len() == 1, Duration::from_secs(5)).await, "the server was told");
    assert_eq!(door_stops(&n, "bridged_mcp"), 1);
    n.running(0).await;
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert_eq!((stop.record.confirmed_ms, stop.record.acknowledged_ms), (None, None), "a release, neither confirmed nor acknowledged: {stop:?}");
    // A second requester also numbering from 1 (#806's review, M1), while the server still holds the first call: the
    // bridge forwards each under an id of its own, the notice names only the first's, and the second requester is
    // answered under its own id 1.
    let reply = n.send(&n.tool("remote_plain"), n.frame(&n.tool("remote_plain"), None, Cm::Honest).0).await;
    let v = json(&reply);
    assert_eq!((v["id"].clone(), v["result"]["content"][0]["text"].as_str()), (serde_json::json!(1), Some("late:remote_plain")), "{v}");
    let ids = server.ids.lock().unwrap().clone();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1], "two forwards, two ids: {ids:?}");
    // …and this bridge task's alone (#806's confirm round, C2): the node's id and a token drawn for the task, so
    // another node bridging the same server, or this bridge restarted, forwards under other ids.
    let prefix = format!("mycelium-bridge-{}-", n.a.node_id());
    assert!(ids.iter().all(|i| i.as_str().is_some_and(|i| i.starts_with(&prefix))), "{ids:?}");
    let (mut a, mut b) = (0u64, 0u64);
    let draw = || mycelium_core::sim_seam::rng_u64_from("nonce", 0);
    assert_ne!(
        super::mcp::mint_forward_id(&super::mcp::bridge_token("127.0.0.1:1", draw()), &mut a),
        super::mcp::mint_forward_id(&super::mcp::bridge_token("127.0.0.1:1", draw()), &mut b),
        "two bridge tasks of one node, each at its first forward"
    );
    assert_eq!(server.cancelled.lock().unwrap()[0]["requestId"], ids[0], "the notice names the cancelled forward's id, and no other call's");
    n.a.shutdown().await;
}

/// One SDK subscriber's `rpc/serve` stream, read as an SDK reads it: each event's name and data, in order.
struct Subscriber {
    events: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
    reader: tokio::task::JoinHandle<()>,
    http_port: u16,
    /// The stream's name, from the `Mycelium-Serve-Stream` header.
    stream: String,
}

impl Subscriber {
    async fn open(n: &Node, kind: &str, query: &str) -> Self {
        // No total timeout on the stream itself; its opening is bounded.
        let opening = reqwest::Client::new().get(format!("http://127.0.0.1:{}/gateway/rpc/serve/{kind}{query}", n.http_port)).bearer_auth("sdk-secret").send();
        let mut stream = tokio::time::timeout(HTTP_BOUND, opening).await.expect("the serve stream opens within its bound").expect("the serve stream opens");
        assert_eq!(stream.status(), 200);
        let name = stream.headers().get("mycelium-serve-stream").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = Arc::clone(&events);
        let reader = tokio::spawn(async move {
            let mut buf = String::new();
            while let Ok(Some(c)) = stream.chunk().await {
                buf.push_str(&String::from_utf8_lossy(&c));
                while let Some(at) = buf.find("\n\n") {
                    let block: String = buf.drain(..at + 2).collect();
                    let (mut name, mut data) = (String::new(), String::new());
                    for line in block.lines() {
                        if let Some(v) = line.strip_prefix("event:") {
                            name = v.trim().to_string();
                        } else if let Some(v) = line.strip_prefix("data:") {
                            data.push_str(v.trim());
                        }
                    }
                    if !data.is_empty() {
                        into.lock().unwrap().push((name, serde_json::from_str(&data).unwrap_or_default()));
                    }
                }
            }
        });
        // The stream's serve loop is registered when the response starts; give its first ask a moment.
        tokio::time::sleep(Duration::from_millis(200)).await;
        Self { events, reader, http_port: n.http_port, stream: name }
    }
    /// Wait for the `i`-th event named `name`.
    async fn next(&self, name: &str) -> serde_json::Value {
        let found = || self.events.lock().unwrap().iter().find(|(e, _)| e == name).map(|(_, d)| d.clone());
        assert!(until(|| found().is_some(), Duration::from_secs(10)).await, "no `{name}` event: {:?}", self.events.lock().unwrap());
        found().unwrap()
    }
    fn count(&self, name: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|(e, _)| e == name).count()
    }
    fn saw(&self, name: &str) -> bool {
        self.events.lock().unwrap().iter().any(|(e, _)| e == name)
    }
    /// `POST /gateway/rpc/respond` for the request `req` (its event's data) with `body`'s extra fields.
    async fn respond(&self, req: &serde_json::Value, extra: serde_json::Value) -> (u16, serde_json::Value) {
        // The stream the request went out on, as an SDK names it; `extra` may override it (`null`: none).
        let mut body = serde_json::json!({ "nonce_hex": req["nonce_hex"], "sender": req["sender"], "stream": req["stream"] });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let r = http().post(format!("http://127.0.0.1:{}/gateway/rpc/respond", self.http_port)).bearer_auth("sdk-secret").json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or_default())
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn late_serve_answers(n: &Node) -> u64 {
    n.a.task_ctx.rpc_serve_late_answers.pin().get(CUSTOM).copied().unwrap_or(0)
}

/// **A subscriber that declares it stops on a `cancel` event is served a cancellable call, and only its own word
/// records a confirmed stop** (I4, PR 3b; exit 17's SDK half), with and without a cohort budget. The stream is opened
/// with `stops_on_cancel=true`; the cancellable kind is streamed to it (an undeclared subscriber is refused it:
/// `the_doors_that_cannot_stop_a_call_refuse_one_under_a_cancellable_rule_by_name`). The gateway cancels: the
/// requester is answered `cancelled` (`data.door` `sdk_serve`) while the subscriber still holds the call, the stream
/// carries the `cancel` event naming the request, and the subscriber's `"stopped": true` is accepted — the receipt is
/// `Stopped`, the stop record confirmed. Its confirmation came inside the grace, so nothing is counted. Seen failing
/// first with `Door::SdkServe { stops: true }` answering as an undeclared stream: the call was refused
/// `door_cannot_stop` and never streamed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_subscriber_is_served_a_cancellable_call_and_its_confirmation_is_the_stop() {
    for budget in [false, true] {
        let n = Arc::new(node_with(Opts { budget, ..Opts::default() }).await);
        let sub = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true").await;
        let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
        let (n2, call) = (Arc::clone(&n), n.custom());
        let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
        n.running_or_answered(1, &mut answered).await;
        let req = sub.next(CUSTOM).await;
        let n3 = Arc::clone(&n);
        let receipt = tokio::spawn(async move { forward_cancel(&n3, id).await });
        let cancel = sub.next("cancel").await;
        assert_eq!((cancel["cancel"].as_bool(), &cancel["nonce_hex"], &cancel["sender"]), (Some(true), &req["nonce_hex"], &req["sender"]), "the event names the request: {cancel}");
        let reply = tokio::time::timeout(Duration::from_secs(5), &mut answered).await.expect("the requester is answered").unwrap();
        let v = json(&reply);
        assert_eq!((v["reason"].as_str(), v["data"]["door"].as_str()), (Some("cancelled"), Some("sdk_serve")), "budget {budget}: {v}");
        assert_eq!(n.authority.running_count(), 1, "the node's stop is not the subscriber's: its registration stands until it says it stopped");
        assert_eq!(sub.respond(&req, serde_json::json!({ "stopped": true })).await, (200, serde_json::json!({ "ok": true, "stopped": true })));
        assert_eq!(receipt.await.unwrap(), "stopped", "budget {budget}: the subscriber's own confirmation");
        n.running(0).await;
        let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
        assert!(stop.record.confirmed_ms.is_some(), "{stop:?}");
        assert_eq!((late_serve_answers(&n), door_stops(&n, "sdk_serve")), (0, 1));
        n.a.shutdown().await;
    }
}

/// **A declared subscriber that answers with a result instead of confirming is not a confirmed stop**, and its answer
/// is discarded (I4, PR 3b). The cancel's receipt waits for the work to end; the subscriber answers `done` inside the
/// wait, so the work has ended — but the subscriber did not say it stopped (it may have finished), so the receipt is
/// `Unbounded`, not `Stopped`, and the record has no confirmation. `rpc/respond` answers `{"ok": true, "cancelled":
/// true}` and the requester keeps the `cancelled` answer. Inside the grace, so not counted. Seen failing first with
/// the receipt's wait reading the count alone (PR 3a's loop): `stopped`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_subscribers_result_after_the_cancel_is_discarded_and_not_a_confirmed_stop() {
    let n = Arc::new(node().await);
    let sub = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true").await;
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    let req = sub.next(CUSTOM).await;
    let n3 = Arc::clone(&n);
    let receipt = tokio::spawn(async move { forward_cancel(&n3, id).await });
    sub.next("cancel").await;
    let reply = tokio::time::timeout(Duration::from_secs(5), &mut answered).await.expect("answered").unwrap();
    assert_eq!(json(&reply)["reason"], "cancelled");
    let late = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
    assert_eq!(sub.respond(&req, serde_json::json!({ "result_b64": late })).await, (200, serde_json::json!({ "ok": true, "cancelled": true })), "discarded");
    assert_eq!(receipt.await.unwrap(), "unbounded");
    n.running(0).await;
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert_eq!(stop.record.confirmed_ms, None, "{stop:?}");
    assert_eq!(late_serve_answers(&n), 0, "inside the grace");
    n.a.shutdown().await;
}

/// **A declared subscriber's answer past the grace is counted, not refused** (I4, PR 3b: assessment, not
/// enforcement). The call is cancelled through the KV; the subscriber answers 1.6 s after the `cancel` event, past
/// `LAPSE_ANSWER_GRACE_MS`: the answer is discarded as any late answer is (`cancelled: true`, 200, nothing refused),
/// and `rpc_serve_answered_after_lapse` counts it once under its kind, in `GET /stats` too. Seen failing first with
/// `ServedCall::assess` returning at once: nothing was counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_subscribers_answer_past_the_grace_is_counted_and_nothing_is_refused() {
    let n = Arc::new(node().await);
    let sub = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true").await;
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    let req = sub.next(CUSTOM).await;
    n.cancel(id);
    sub.next("cancel").await;
    let reply = tokio::time::timeout(Duration::from_secs(10), &mut answered).await.expect("answered").unwrap();
    assert_eq!(json(&reply)["reason"], "cancelled");
    tokio::time::sleep(Duration::from_millis(super::provider_enforcement::LAPSE_ANSWER_GRACE_MS + 600)).await;
    let late = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
    assert_eq!(sub.respond(&req, serde_json::json!({ "result_b64": late })).await, (200, serde_json::json!({ "ok": true, "cancelled": true })));
    assert_eq!(late_serve_answers(&n), 1);
    let stats: serde_json::Value = http().get(format!("http://127.0.0.1:{}/stats", n.http_port)).send().await.unwrap().json().await.unwrap();
    assert_eq!(stats["rpc_serve_answered_after_lapse"][CUSTOM].as_u64(), Some(1), "{}", stats["rpc_serve_answered_after_lapse"]);
    assert_eq!(stats["door_stops"]["sdk_serve"].as_u64(), Some(1), "{}", stats["door_stops"]);
    n.running(0).await;
    n.a.shutdown().await;
}

/// **An undeclared subscriber's plain-rule call: the node's own stop, recorded unconfirmed** (I4, PR 3b), with and
/// without `cancel_events`. Under a rule that is not cancellable the call is streamed to a subscriber that made no
/// promise. The gateway cancels: the requester is answered `cancelled`, the receipt is `Unbounded`, and a later answer
/// is discarded and not counted (no promise to assess). A stream opened with no parameter — an older SDK, which would
/// read any event as a request — is sent no `cancel` event; one opened with `cancel_events=true` is. A subscriber's
/// `"stopped": true` for a call the node did not cancel is refused `409 not_cancelled`, and its result is still taken.
/// Seen failing first with `served_call::run`'s stop arm taken out (the requester waited on the subscriber), and with
/// `hears_cancel` set for every stream (an older client was sent a `cancel` event).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_subscribers_plain_call_is_stopped_by_the_node_and_recorded_unconfirmed() {
    for (query, hears) in [("", false), ("?cancel_events=true", true)] {
        let n = Arc::new(node().await);
        let sub = Subscriber::open(&n, PLAIN_RPC, query).await;
        // Not cancelled: a `stopped` is refused and nothing moves; the result is then delivered.
        let (framed, _, _) = n.frame(&n.plain_kind(), None, Cm::Honest);
        let (n2, call) = (Arc::clone(&n), n.plain_kind());
        let first = tokio::spawn(async move { n2.send(&call, framed).await });
        let req = sub.next(PLAIN_RPC).await;
        let (status, body) = sub.respond(&req, serde_json::json!({ "stopped": true })).await;
        assert_eq!((status, body["error"].as_str()), (409, Some("not_cancelled")), "{body}");
        let done = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
        assert_eq!(sub.respond(&req, serde_json::json!({ "result_b64": done })).await.0, 200);
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), first).await.expect("answered").unwrap(), b"done".to_vec());
        // Cancelled while the subscriber holds it.
        sub.events.lock().unwrap().clear();
        let (framed, id, _) = n.frame(&n.plain_kind(), None, Cm::Honest);
        let (n2, call) = (Arc::clone(&n), n.plain_kind());
        let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
        n.running_or_answered(1, &mut answered).await;
        let req = sub.next(PLAIN_RPC).await;
        assert_eq!(forward_cancel(&n, id).await, "unbounded", "{query}: the subscriber made no promise");
        let reply = tokio::time::timeout(Duration::from_secs(5), &mut answered).await.expect("answered").unwrap();
        assert_eq!(json(&reply)["reason"], "cancelled", "{query}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sub.saw("cancel"), hears, "{query}: {:?}", sub.events.lock().unwrap());
        // An undeclared stream made no promise to stop: its `stopped` is refused by name (#806's review, L2).
        let (status, body) = sub.respond(&req, serde_json::json!({ "stopped": true })).await;
        assert_eq!((status, body["error"].as_str()), (409, Some("not_declared")), "{query}: {body}");
        tokio::time::sleep(Duration::from_millis(super::provider_enforcement::LAPSE_ANSWER_GRACE_MS + 200)).await;
        assert_eq!(sub.respond(&req, serde_json::json!({ "result_b64": done })).await, (200, serde_json::json!({ "ok": true, "cancelled": true })), "{query}");
        n.running(0).await;
        assert_eq!(n.a.task_ctx.rpc_serve_late_answers.pin().get(PLAIN_RPC).copied(), None, "{query}: no promise, nothing assessed");
        n.a.shutdown().await;
    }
}

/// **An enveloped call reaches one serve stream at a node, and only that stream's own word confirms its stop** (#806's
/// review, M3 and M5; the maintainer's decision of 2026-11-17). Two declaring streams of one principal are open on the
/// kind: the envelope replay window hands the call to exactly one of them — the reliance the per-stream keying no
/// longer depends on, pinned here — and every `cancel` names that stream. A `stopped` naming no stream is refused
/// `stream_required`, one naming the other stream `not_your_stream`, and **another principal's** `not_your_stream`
/// (it was never streamed the call); the registration stands through all three. The stream's own confirmation is
/// `Stopped`. Seen failing first with both principal checks taken out — `rpc/respond`'s and the watcher's; either alone
/// refuses it (the other principal's `stopped` was taken).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_enveloped_call_reaches_one_stream_and_only_its_word_confirms() {
    let n = Arc::new(node().await);
    let one = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true").await;
    let two = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true").await;
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (handed, other) = match (one.saw(CUSTOM), two.saw(CUSTOM)) {
        (true, false) => (&one, &two),
        (false, true) => (&two, &one),
        seen => panic!("exactly one stream is handed an enveloped call: {seen:?}"),
    };
    assert_eq!(n.authority.running_count(), 1);
    let req = handed.next(CUSTOM).await;
    let n3 = Arc::clone(&n);
    let receipt = tokio::spawn(async move { forward_cancel(&n3, id).await });
    let cancel = handed.next("cancel").await;
    assert_eq!(cancel["stream"], req["stream"]);
    assert!(!other.saw("cancel"), "a stream that was not handed the call is not told of it");
    let reply = tokio::time::timeout(Duration::from_secs(5), &mut answered).await.expect("answered").unwrap();
    assert_eq!(json(&reply)["reason"], "cancelled");
    let (status, body) = handed.respond(&req, serde_json::json!({ "stopped": true, "stream": null })).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("stream_required")), "{body}");
    // Stream names are 128 random bits (#806's confirm round, C3): the header's, the events', never a counter's.
    assert_eq!(req["stream"].as_str(), Some(handed.stream.as_str()));
    let id = |s: &Subscriber| u128::from_str_radix(&s.stream, 16).expect("32 hex digits");
    assert!(one.stream.len() == 32 && two.stream.len() == 32 && id(&one).abs_diff(id(&two)) > 1_000_000, "{} {}", one.stream, two.stream);
    // The sibling's real stream, named by a process that somehow learned it: it was not handed the call.
    let (status, body) = handed.respond(&req, serde_json::json!({ "stopped": true, "stream": other.stream })).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_your_stream")), "{body}");
    let r = http().post(format!("http://127.0.0.1:{}/gateway/rpc/respond", n.http_port)).bearer_auth("sdk2-secret")
        .json(&serde_json::json!({ "nonce_hex": req["nonce_hex"], "sender": req["sender"], "stream": req["stream"], "stopped": true })).send().await.unwrap();
    let (status, body): (u16, serde_json::Value) = (r.status().as_u16(), r.json().await.unwrap_or_default());
    assert_eq!((status, body["error"].as_str()), (409, Some("not_your_stream")), "another principal's stopped: {body}");
    assert_eq!(n.authority.running_count(), 1, "nothing moved");
    assert_eq!(handed.respond(&req, serde_json::json!({ "stopped": true })).await.0, 200);
    assert_eq!(receipt.await.unwrap(), "stopped");
    n.running(0).await;
    n.a.shutdown().await;
}

/// **A call cancelled while it waits in the stream's queue is a stop the node saw complete** (#806's review, L7): it
/// was never handed out, so its record is confirmed whether or not the stream declared. With the stream's loop holding
/// what it has checked (a test hook), a plain-rule call for an undeclared stream is admitted and registered, then
/// cancelled through the KV; released, the loop answers it `cancelled`, streams nothing, and the stop record is
/// confirmed. Seen failing first with the queue's lapse dropping the admission as it was (the undeclared guard's
/// release: no confirmation).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_cancelled_in_the_streams_queue_is_the_nodes_confirmed_stop() {
    let n = Arc::new(node().await);
    let sub = Subscriber::open(&n, PLAIN_RPC, "?cancel_events=true").await;
    let me = n.a.node_id().to_string();
    super::http::mesh_test_hooks::HOLD_STREAMING.pin().insert(me.clone());
    let (framed, id, _) = n.frame(&n.plain_kind(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.plain_kind());
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    n.cancel(id);
    // The sweep asks the queued call's work to stop while the loop still holds it.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(!answered.is_finished(), "still held in the queue");
    super::http::mesh_test_hooks::HOLD_STREAMING.pin().remove(&me);
    let reply = tokio::time::timeout(Duration::from_secs(5), &mut answered).await.expect("answered").unwrap();
    assert_eq!(json(&reply)["reason"], "cancelled");
    n.running(0).await;
    assert!(!sub.saw(PLAIN_RPC), "never streamed: {:?}", sub.events.lock().unwrap());
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert!(stop.record.confirmed_ms.is_some(), "the node's own stop, confirmed: {stop:?}");
    n.a.shutdown().await;
}

/// **A declaring stream is handed no more than its window, and a `cancel` is never held behind requests** (#806's
/// confirm round, C1). A declaring SDK reads its stream without pause, so the bound on what it holds is the node's
/// own: with `read_ahead=1` the stream is handed the first call and not the second, which waits at the node — where
/// admission sees it — until the stream ends the first. Cancelling the first reaches the stream at once, though a
/// request is waiting; its `stopped` (naming the stream) makes room, and the second call is then handed out. Seen
/// failing first with the window never full: both calls were handed out at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declaring_stream_holds_no_more_than_its_window_and_a_cancel_is_not_held_back() {
    let n = Arc::new(node().await);
    let sub = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true&read_ahead=1").await;
    let (framed, id, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let mut first = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut first).await;
    let req = sub.next(CUSTOM).await;
    let (framed, _, _) = n.frame(&n.custom(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.custom());
    let second = tokio::spawn(async move { n2.send(&call, framed).await });
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!((sub.count(CUSTOM), n.authority.running_count()), (1, 1), "the second call waits at the node");
    n.cancel(id);
    sub.next("cancel").await;
    assert_eq!(sub.count(CUSTOM), 1, "the cancel came while the second call still waited");
    let reply = tokio::time::timeout(Duration::from_secs(5), &mut first).await.expect("answered").unwrap();
    assert_eq!(json(&reply)["reason"], "cancelled");
    assert_eq!(sub.respond(&req, serde_json::json!({ "stopped": true })).await.0, 200);
    assert!(until(|| sub.count(CUSTOM) == 2, Duration::from_secs(5)).await, "room in the window: the second call is handed out");
    let req2 = sub.events.lock().unwrap().iter().filter(|(e, _)| e == CUSTOM).nth(1).map(|(_, d)| d.clone()).unwrap();
    let done = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
    assert_eq!(sub.respond(&req2, serde_json::json!({ "result_b64": done })).await.0, 200);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), second).await.expect("answered").unwrap(), b"done".to_vec());
    n.a.shutdown().await;
}

/// **A call cancelled in the queue of a stream that then closes is still the node's confirmed stop** (#806's confirm
/// round, on L7): recorded by what happened, as when the queue is drained while the stream lives. The loop holds the
/// checked call, it is cancelled, and the subscriber goes away: the requester is answered `cancelled` — not
/// `withdrawn` — and the stop record is confirmed. Seen failing first with the closing drain answering every queued
/// call `withdrawn` and dropping its admission as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_cancelled_in_the_queue_of_a_stream_that_closes_is_the_nodes_confirmed_stop() {
    let n = Arc::new(node().await);
    let sub = Subscriber::open(&n, PLAIN_RPC, "?cancel_events=true").await;
    let me = n.a.node_id().to_string();
    super::http::mesh_test_hooks::HOLD_STREAMING.pin().insert(me.clone());
    let (framed, id, _) = n.frame(&n.plain_kind(), None, Cm::Honest);
    let (n2, call) = (Arc::clone(&n), n.plain_kind());
    let mut answered = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running_or_answered(1, &mut answered).await;
    n.cancel(id);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(!answered.is_finished(), "still held in the queue");
    drop(sub);
    let reply = tokio::time::timeout(Duration::from_secs(10), &mut answered).await.expect("answered when the stream closes").unwrap();
    super::http::mesh_test_hooks::HOLD_STREAMING.pin().remove(&me);
    assert_eq!(json(&reply)["reason"], "cancelled", "{}", String::from_utf8_lossy(&reply));
    n.running(0).await;
    let stop = n.authority.named_stop_records().into_iter().rev().find(|s| s.term.as_deref() == Some(&format!("c{id}"))).expect("a stop record");
    assert!(stop.record.confirmed_ms.is_some(), "{stop:?}");
    n.a.shutdown().await;
}

/// **A call holds one cohort place, however many serve streams it reaches** (the maintainer's decision of 2026-11-17,
/// M4: a caller's budget is the caller's, and how many streams a server opens must not multiply what a caller is
/// charged). A member's call without an envelope reaches both streams a server opened on the kind; the caller's pool
/// holds **one** place while both hold the call, and none once the call is answered and both registrations have
/// ended. Seen failing first with each stream's watcher keeping its own admission's place: two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_holds_one_cohort_place_however_many_streams_it_reaches() {
    let n = Arc::new(node_with(Opts { budget: true, legacy: true, enforce: false, ..Opts::default() }).await);
    let held = |n: &Node| super::provider_enforcement::test_undeclared_in_flight(&n.a.task_ctx).expect("a budget");
    let (one, two) = (Subscriber::open(&n, PLAIN_RPC, "").await, Subscriber::open(&n, PLAIN_RPC, "").await);
    let n2 = Arc::clone(&n);
    let answered = tokio::spawn(async move {
        super::rpc::rpc_call_framed(&n2.a.task_ctx, n2.a.node_id().clone(), Arc::from(PLAIN_RPC), Bytes::from_static(b"work"), Duration::from_secs(20)).await.expect("a reply").to_vec()
    });
    let (r1, _r2) = (one.next(PLAIN_RPC).await, two.next(PLAIN_RPC).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(held(&n), 1, "one call, two streams: one place");
    let done = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
    assert_eq!(one.respond(&r1, serde_json::json!({ "result_b64": done })).await.0, 200);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), answered).await.expect("answered").unwrap(), b"done".to_vec());
    assert!(until(|| held(&n) == 0, Duration::from_secs(5)).await, "released once both streams' registrations ended: {}", held(&n));
    n.a.shutdown().await;
}

/// **A refused `rpc/respond` does not open the window, and a bad `read_ahead` is refused by name** (#806's second
/// confirm round, D1 and D2). At `read_ahead=1` the stream holds the first call and the second waits at the node. A
/// `"stopped": true` for the call in hand — which the node has not cancelled — is refused `409 not_cancelled` and
/// ends nothing: the second call still waits. The first call's answer, which is taken, makes room. And a declaring
/// stream's `read_ahead` of 0, 257 or `abc` is refused `400 invalid_read_ahead` in the route's JSON shape, while an
/// undeclared stream, which has no window, is opened whatever it says. Seen failing first with the window released
/// before the respond is judged (the second call was handed out on the refusal), and with the value clamped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_respond_does_not_open_the_window_and_a_bad_read_ahead_is_refused() {
    let n = Arc::new(node().await);
    let open = |query: &'static str| {
        let port = n.http_port;
        // A stream that opens never ends, so only its head is awaited here, and that within a bound.
        async move {
            let opening = reqwest::Client::new().get(format!("http://127.0.0.1:{port}/gateway/rpc/serve/{PLAIN_RPC}{query}")).bearer_auth("sdk-secret").send();
            tokio::time::timeout(HTTP_BOUND, opening).await.expect("answered within its bound").unwrap()
        }
    };
    for bad in ["?stops_on_cancel=true&read_ahead=0", "?stops_on_cancel=true&read_ahead=257", "?stops_on_cancel=true&read_ahead=abc"] {
        let r = open(bad).await;
        // The status first: a stream that was opened (200) has no body to wait for — a named failure, not a hang.
        assert_eq!(r.status().as_u16(), 400, "{bad}: refused, never opened");
        let body: serde_json::Value = tokio::time::timeout(HTTP_BOUND, r.json()).await.expect("the refusal's body within its bound").expect("a JSON refusal");
        assert_eq!(body["error"].as_str(), Some("invalid_read_ahead"), "{bad}: {body}");
    }
    assert_eq!(open("?read_ahead=abc").await.status(), 200, "an undeclared stream has no window: the parameter is ignored");

    let sub = Subscriber::open(&n, CUSTOM, "?stops_on_cancel=true&read_ahead=1").await;
    let send = |n: &Arc<Node>| {
        let (framed, _, _) = n.frame(&n.custom(), None, Cm::Honest);
        let (n2, call) = (Arc::clone(n), n.custom());
        tokio::spawn(async move { n2.send(&call, framed).await })
    };
    let mut first = send(&n);
    n.running_or_answered(1, &mut first).await;
    let req = sub.next(CUSTOM).await;
    let second = send(&n);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(sub.count(CUSTOM), 1);
    let (status, body) = sub.respond(&req, serde_json::json!({ "stopped": true })).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_cancelled")), "{body}");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(sub.count(CUSTOM), 1, "a refused respond ends nothing: the second call still waits");
    let done = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"done");
    assert_eq!(sub.respond(&req, serde_json::json!({ "result_b64": done })).await.0, 200);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), first).await.expect("answered").unwrap(), b"done".to_vec());
    assert!(until(|| sub.count(CUSTOM) == 2, Duration::from_secs(5)).await, "the taken answer made room");
    let req2 = sub.events.lock().unwrap().iter().filter(|(e, _)| e == CUSTOM).nth(1).map(|(_, d)| d.clone()).unwrap();
    assert_eq!(sub.respond(&req2, serde_json::json!({ "result_b64": done })).await.0, 200);
    let _ = tokio::time::timeout(Duration::from_secs(5), second).await.expect("answered");
    n.a.shutdown().await;
}
