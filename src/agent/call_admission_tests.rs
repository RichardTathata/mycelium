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
}

/// One node: gateway, provider and caller. Its policy: `slow` and the skill are **cancellable**; `plain` is not;
/// `both` is cancellable and needs a grant.
async fn node() -> Node {
    let (port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("i4-admit-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens = vec![crate::config::GatewayNamedToken { name: "alice".into(), token: "alice-secret".into(), scopes: vec!["mcp:invoke".into()] }];
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
            .allow(Rule::new("*", "skill.invoke", format!("skill:depot/long@{me}")).cancellable()),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle_grants(PrincipalId::new(node_principal(&id)).unwrap());
    // F = 2 s: the sweep runs every 100 ms.
    let gate = ExecutionGate::strict(ResourceAuthority::new("unused", 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, FreshnessPolicy { freshness_ms: 2_000, interval_ms: 500, delivery_ms: 160 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()));
    a.with_execution_authority(Arc::clone(&authority));
    a.with_provider_enforcement();
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
    let mut rx = a.service().rpc_rx(SKILL);
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
    tokio::time::sleep(Duration::from_millis(150)).await;
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
    fn key(&self) -> Arc<ed25519_dalek::SigningKey> {
        self.a.task_ctx.tls.get().unwrap().signing_key()
    }
    fn now(&self) -> u64 {
        self.a.task_ctx.hlc.decision_now_ms()
    }
    fn tool(&self, name: &str) -> Call {
        let arguments = serde_json::json!({ "aisle": 4 });
        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).to_string().into_bytes();
        Call { kind: MCP, payload, claim: None, operation: "tools/call", resource: format!("tool:{name}@{}", self.a.node_id()), arguments }
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

/// The refusal D-2 promises, at an MCP reply: -32031, `authority_not_established`, `call_mandate_not_established`.
#[track_caller]
fn assert_not_established(reply: &[u8], found: &str) {
    let v = json(reply);
    assert_eq!((v["error"]["code"].as_i64(), v["error"]["data"]["reason"].as_str(), v["error"]["data"]["detail"].as_str()), (Some(-32031), Some("authority_not_established"), Some("call_mandate_not_established")), "{v}");
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
    let running = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running(1).await;
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
    assert_not_established(&reply, "carries no call: mandate");
    let (reply, _) = n.call(&slow, None, Cm::NoCheckpoint).await;
    assert_not_established(&reply, "does not parse");
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
    assert_not_established(&reply, "minted for another call");
    assert_eq!((n.counts(), n.authority.running_count(), n.authority.view_sizes().2), ((0, 0), 0, 0), "nothing ran, nothing is registered, no window is held");

    // The non-canonical spelling, at the door.
    let (reply, _) = n.call(&slow, None, Cm::Alias).await;
    assert_not_established(&reply, "the gateway that sent it");
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
    for (tool, with_grant) in [("slow", false), ("both", true)] {
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
        assert_not_established(&sent.await.unwrap(), "revoked");
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
        let sent = tokio::spawn(async move { n2.send(&call, framed).await });
        n.running(2).await;
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
    let sent = tokio::spawn(async move { n2.send(&call, framed).await });
    n.running(1).await;
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
/// No gateway mints yet (the row's next PR), so each of these doors carries none:
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
    assert_not_established(&reply, "carries no call: mandate");
    // An unframed call: the bare payload, no envelope.
    let reply = super::rpc::rpc_call_framed(&n.a.task_ctx, n.a.node_id().clone(), Arc::from(MCP), Bytes::from(slow.payload.clone()), Duration::from_secs(20)).await.expect("a reply");
    let v = json(&reply);
    assert!(v["error"].is_object(), "an unframed call is refused: {v}");
    assert_eq!((n.counts(), n.authority.running_count()), ((0, 0), 0), "none of them ran");
    n.a.shutdown().await;
}
