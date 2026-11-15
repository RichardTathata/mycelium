//! **A hosted capability's kind is protected work at an enforcing provider** (the maintainer's decision of
//! 2026-11-14, on #785's review): `cap.invoke/{ns}/{name}` reaches `rpc_rx`, where `provider_enforcement::check` now
//! decides it — on the capability **the kind names**, before the receiver yields the request.
//!
//! One node is the provider and the caller. The receivers below stand where a hosted component's serve loop stands
//! (`mycelium-wasm-host`'s `serve_loop` reads the same `rpc_rx`), and each counts what reaches it, so every
//! "nothing ran" is read from the receiver, not from the absence of an answer. The wasm host's own tests ask the same
//! questions of a real component and of its MCP bridge
//! (`mycelium-wasm-host/tests/hosted_capability_enforcement.rs`).
//!
//! Needs `gateway` + `tls` (the provider check's own gate): it runs on both of CI's lib lines.

use super::action_evaluator::{AeEvidence, DecisionKind, RecordKind};
use super::gateway_authority::{arguments_digest, possession_request, ExecutionAuthority, PresentedMandate};
use super::gateway_caller::{frame_with_context_and_mandate, node_principal};
use crate::knowledge::IssuerId;
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint};
use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use ed25519_dalek::SigningKey;
use super::provider_enforcement::{derive, protected_here, ENFORCEMENT_POINT_PROVIDER};
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::signal::{Signal, SignalScope};
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use bytes::{BufMut, Bytes, BytesMut};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A gateway client: the principal a rule admits where the node itself is not admitted.
const CLIENT: &str = "token:gw/agent-1";

/// The capabilities the node "hosts", as `(namespace, name)`.
const HOSTED: &[(&str, &str)] = &[("tool", "echo"), ("tool", "secret"), ("tool", "clientonly"), ("tool", "guarded"), ("route", "optimize"), ("route", "secret"), ("route", "slow")];

fn kind(ns: &str, name: &str) -> String {
    format!("{}{ns}/{name}", crate::HOSTED_CAPABILITY_KIND_PREFIX)
}

#[derive(Clone, Copy)]
struct Opts {
    enforcing: bool,
    /// A cohort budget in which every caller no cohort names shares **one** place.
    budget: bool,
    /// The receivers keep each request and never answer it, so an admitted call stays in flight.
    hold: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { enforcing: true, budget: false, hold: false }
    }
}

struct Door {
    a: Arc<GossipAgent>,
    dir: std::path::PathBuf,
    /// How many requests each kind's receiver was handed.
    ran: Arc<Mutex<HashMap<String, usize>>>,
    /// The MCP tool `echo`'s runs.
    tool_ran: Arc<AtomicUsize>,
    /// The MCP tool `guarded`'s runs.
    guarded_ran: Arc<AtomicUsize>,
    /// The operator whose `depot` mandates the node's authority honours, and the client's own key.
    operator: SigningKey,
    holder: SigningKey,
    /// Requests kept in flight (`Opts::hold`).
    held: Arc<Mutex<Vec<super::rpc::RpcRequest>>>,
}

impl Drop for Door {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("hosted-door-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// One node. Its policy: the node itself may call the tool `echo` and the capability `route/optimize`; the client
/// may call the tool `clientonly`, and the tool `guarded` under a `depot` mandate; `route/slow` is allowed to anyone
/// and **cancellable**; `tool/secret` and `route/secret` have a receiver and no rule.
async fn door(opts: Opts) -> Door {
    let port = crate::test_util::alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("hosted-door-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some({
        let mut tls_cfg = crate::TlsConfig::default();
        tls_cfg.auto_cert_dir = dir.join("tls");
        tls_cfg
    });
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let me = node_principal(&id);
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-hosted-door")
            .with_catalogue("cat-hosted-door", "1")
            .map_action("tools/call", "*")
            .map_action(crate::HOSTED_CAPABILITY_OPERATION, "*")
            .allow(Rule::new(me.clone(), "tools/call", format!("tool:echo@{id}")))
            .allow(Rule::new(CLIENT, "tools/call", format!("tool:clientonly@{id}")))
            .allow(Rule::new(CLIENT, "tools/call", format!("tool:guarded@{id}")).requiring_mandate("depot"))
            .allow(Rule::new(me, crate::HOSTED_CAPABILITY_OPERATION, format!("cap:route/optimize@{id}")))
            .allow(Rule::new("*", crate::HOSTED_CAPABILITY_OPERATION, format!("cap:route/slow@{id}")).cancellable()),
    ));
    let (operator, holder) = (SigningKey::from_bytes(&[60u8; 32]), SigningKey::from_bytes(&[62u8; 32]));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), operator.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CLIENT).unwrap(), holder.verifying_key().to_bytes()).unwrap();
    let f = 120_000;
    let gate = ExecutionGate::strict(ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: f / 20 }, FreshnessPolicy { freshness_ms: f, interval_ms: f / 4, delivery_ms: f / 12 }).unwrap();
    a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    if opts.enforcing {
        a.with_provider_enforcement();
    }
    if opts.budget {
        a.with_cohort_budget(crate::knowledge::cohort_budget::CohortBudget::new(3, 1), crate::knowledge::cohort::CohortView::trusting([]), TrustedExternalIssuers::new());
    }
    a.start().await.unwrap();

    let ran: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let held: Arc<Mutex<Vec<super::rpc::RpcRequest>>> = Arc::default();
    let kinds = HOSTED.iter().map(|(ns, name)| kind(ns, name)).chain(["cap.invoke/lonely".to_string()]);
    for k in kinds {
        let (a, ran, held) = (Arc::clone(&a), Arc::clone(&ran), Arc::clone(&held));
        let mut rx = a.service().rpc_rx(k.clone());
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                *ran.lock().unwrap().entry(k.clone()).or_default() += 1;
                if opts.hold {
                    held.lock().unwrap().push(req);
                } else {
                    a.service().rpc_respond(&req, json!({ "ran": k, "payload": String::from_utf8_lossy(&req.payload()) }).to_string().into_bytes());
                }
            }
        });
    }
    // The MCP tools `echo` and `guarded`: what the wasm host's bridge registers for a `tool/{name}` component. Each
    // answers the arguments it was handed.
    let (tool_ran, guarded_ran) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    for (name, counter) in [("echo", &tool_ran), ("guarded", &guarded_ran)] {
        let counter = Arc::clone(counter);
        std::mem::forget(a.mcp().register_mcp_tool(name, json!({"type": "object"}), move |args| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(args)
            }
        }));
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    Door { a, dir, ran, tool_ran, guarded_ran, held, operator, holder }
}

impl Door {
    fn me(&self) -> String {
        self.a.node_id().to_string()
    }
    fn ran(&self, ns: &str, name: &str) -> usize {
        self.ran.lock().unwrap().get(&kind(ns, name)).copied().unwrap_or(0)
    }
    /// The node's own direct call of `kind`: a member's `rpc_call`, unframed.
    async fn call(&self, kind: &str, payload: &[u8]) -> Value {
        self.call_within(kind, payload, Duration::from_secs(20)).await.expect("a reply")
    }
    async fn call_within(&self, kind: &str, payload: &[u8], within: Duration) -> Option<Value> {
        let reply = self.a.service().rpc_call(self.a.node_id().clone(), kind.to_string(), payload.to_vec(), within).await.ok()?;
        Some(serde_json::from_slice(&reply).unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&reply) })))
    }
    /// A call of `kind` framed for `principal`, its envelope claiming `claim`.
    async fn call_framed(&self, principal: &str, kind: &str, payload: &[u8], claim: Option<&str>) -> Value {
        let framed = frame_with_context_and_mandate(&self.a.task_ctx, self.a.node_id(), principal, &[], Bytes::copy_from_slice(payload), None, claim).expect("fits");
        let reply = super::rpc::rpc_call_framed(&self.a.task_ctx, self.a.node_id().clone(), Arc::from(kind), framed, Duration::from_secs(20)).await.expect("a reply");
        serde_json::from_slice(&reply).unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&reply) }))
    }
    /// A call of `kind` framed for `principal`, presenting `mandate`.
    async fn call_under(&self, principal: &str, kind: &str, payload: &[u8], mandate: &Value) -> Value {
        let framed = frame_with_context_and_mandate(&self.a.task_ctx, self.a.node_id(), principal, &[], Bytes::copy_from_slice(payload), Some(mandate), None).expect("fits");
        let reply = super::rpc::rpc_call_framed(&self.a.task_ctx, self.a.node_id().clone(), Arc::from(kind), framed, Duration::from_secs(20)).await.expect("a reply");
        serde_json::from_slice(&reply).unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&reply) }))
    }
    /// The operator's first checkpoint for `depot`, revoking nothing: the authority has heard from it.
    fn offer_depot_checkpoint(&self) {
        let c = RevocationCheckpoint { authority: PrincipalId::new("operator:acme").unwrap(), scope: "depot".into(), seq: 1, issued_at_ms: self.a.task_ctx.hlc.decision_now_ms(), revoked: Default::default() };
        let signed = SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&self.operator, &c.canonical_bytes()).to_vec(), checkpoint: c };
        self.a.offer_revocation_checkpoint(&signed).unwrap();
    }
    /// A `depot` mandate held by [`CLIENT`] for the tool `guarded`, its possession proof over **the MCP door's**
    /// operation, resource and arguments digest: `tools/call`, `tool:guarded@{self}`, the digest of `arguments`.
    fn tool_mandate(&self, arguments: &Value) -> Value {
        let m = Mandate {
            holder: PrincipalId::new(CLIENT).unwrap(),
            established_by: PrincipalId::new("operator:acme").unwrap(),
            purpose: "depot".into(),
            scope: "depot".into(),
            operations: vec!["tools/call:tool:guarded".to_string()],
            epoch: 1,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: self.a.task_ctx.hlc.decision_now_ms() + 3_000_000,
        };
        let proof = mycelium_core::tls::sign_bytes(&self.holder, &possession_message(&m, &possession_request("tools/call", &format!("tool:guarded@{}", self.me()), &arguments_digest(arguments))));
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&self.operator, &m.canonical_bytes()).to_vec(), mandate: m };
        serde_json::to_value(PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) }).unwrap()
    }
    /// The provider's decision records, as `(subject, operation, resource, decision)`.
    fn decisions(&self) -> Vec<(String, String, String, DecisionKind)> {
        crate::read_evidence_journal(&self.dir.join("evidence.log")).unwrap().iter().map(|b| serde_json::from_slice::<AeEvidence>(b).unwrap())
            .filter(|e| e.enforcement_point == ENFORCEMENT_POINT_PROVIDER && e.kind == RecordKind::Decided)
            .map(|e| (e.subject, e.operation, e.resource, e.decision))
            .collect()
    }
}

#[track_caller]
fn assert_ran(reply: &Value, kind: &str) {
    assert_eq!(reply["ran"], kind, "RAN NOT: {reply}");
}

/// The refusal a plain RPC receiver answers: `{"error", "reason", "data"}`.
#[track_caller]
fn assert_refused(reply: &Value, reason: &str) {
    assert_eq!((reply["reason"].as_str(), reply["ran"].is_null()), (Some(reason), true), "NOT REFUSED: {reply}");
    assert!(reply["error"].as_str().is_some_and(|e| !e.is_empty()), "{reply}");
}

/// **A member's direct call of a hosted capability is decided at an enforcing provider**, in the `tool` namespace
/// and outside it. With a rule for the caller the receiver is handed the request; with none it is refused and the
/// receiver never sees it. A `tool/{name}` capability is decided as the tool it is — operation `tools/call`,
/// resource `tool:{name}@{self}` — so **the rule that admits the node at the MCP tool loop admits it here**: the
/// same node's `mcp.invoke` of the tool `echo` and its `cap.invoke/tool/echo` leave two provider decisions on one
/// (subject, operation, resource). Any other namespace is decided under `cap.invoke` on `cap:{ns}/{name}@{self}`.
/// Seen failing first: with the family outside the provider's protected set (`protected_here` answering for
/// `is_protected_kind` alone — the code as it was), `tool/secret` and `route/secret` ran and no decision was
/// recorded.
#[tokio::test]
async fn a_members_direct_call_of_a_hosted_capability_is_decided_at_an_enforcing_provider() {
    let d = door(Opts::default()).await;
    let me = d.me();
    let node = node_principal(d.a.node_id());

    assert_ran(&d.call(&kind("tool", "echo"), br#"{"kg": 5}"#).await, &kind("tool", "echo"));
    assert_ran(&d.call(&kind("route", "optimize"), br#"{"stops": 3}"#).await, &kind("route", "optimize"));
    // No allowance names the caller: the reference evaluator answers *indeterminate*, which is never a permit.
    let refused = d.call(&kind("tool", "secret"), br#"{"kg": 5}"#).await;
    assert!(refused["ran"].is_null() && refused["reason"].as_str().is_some_and(|r| !r.is_empty()), "NOT REFUSED: {refused}");
    let refused = d.call(&kind("route", "secret"), br#"{"stops": 3}"#).await;
    assert!(refused["ran"].is_null() && refused["reason"].as_str().is_some_and(|r| !r.is_empty()), "NOT REFUSED: {refused}");
    // The client's rule is not the node's: the tool namespace is decided on who calls, like any tool.
    let refused = d.call(&kind("tool", "clientonly"), b"{}").await;
    assert!(refused["ran"].is_null() && refused["reason"].as_str().is_some_and(|r| !r.is_empty()), "NOT REFUSED: {refused}");
    assert_eq!(
        (d.ran("tool", "echo"), d.ran("route", "optimize"), d.ran("tool", "secret"), d.ran("route", "secret"), d.ran("tool", "clientonly")),
        (1, 1, 0, 0, 0),
        "RAN UNDECIDED: a refused call reached its receiver"
    );

    // The MCP tool loop, same node, same tool, same rule.
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": {"kg": 5}}});
    let reply = d.call(crate::signal::signal_kind::MCP_INVOKE, call.to_string().as_bytes()).await;
    assert!(reply["error"].is_null(), "the tool loop admits the node under the same rule: {reply}");
    assert_eq!(d.tool_ran.load(Ordering::SeqCst), 1);

    let decisions = d.decisions();
    let on = |op: &str, resource: String| decisions.iter().filter(|(s, o, r, _)| *s == node && o == op && *r == resource).map(|(.., v)| *v).collect::<Vec<_>>();
    assert_eq!(on("tools/call", format!("tool:echo@{me}")), vec![DecisionKind::Permit, DecisionKind::Permit], "one rule, two doors: {decisions:?}");
    assert_eq!(on(crate::HOSTED_CAPABILITY_OPERATION, format!("cap:route/optimize@{me}")), vec![DecisionKind::Permit], "{decisions:?}");
    for (op, resource) in [("tools/call", format!("tool:secret@{me}")), ("tools/call", format!("tool:clientonly@{me}")), (crate::HOSTED_CAPABILITY_OPERATION, format!("cap:route/secret@{me}"))] {
        let found = on(op, resource.clone());
        assert!(found.len() == 1 && found[0] != DecisionKind::Permit, "the refusal of {resource} is on the record: {decisions:?}");
    }
    d.a.shutdown().await;
}

/// **The envelope's claim cannot redirect the decision.** The kind names what runs, so an envelope that claims a
/// capability its principal is permitted while the kind names another is decided on the kind's: refused, the
/// receiver untouched, the record on the capability the kind names. In both namespaces. Seen failing first: with
/// the family's arm in `derive` planted to read the resource from the claim where one is carried, the first call
/// ran `tool/secret` on `tool:echo`'s rule (and with the family outside the protected set, as it was, it ran
/// undecided).
#[tokio::test]
async fn the_envelopes_claim_does_not_redirect_a_hosted_capability_call() {
    let d = door(Opts::default()).await;
    let me = d.me();
    let node = node_principal(d.a.node_id());

    let refused = d.call_framed(&node, &kind("tool", "secret"), b"{}", Some(&format!("tool:echo@{me}"))).await;
    assert!(refused["ran"].is_null() && refused["reason"].as_str().is_some_and(|r| !r.is_empty()), "REDIRECTED: {refused}");
    let refused = d.call_framed(&node, &kind("route", "secret"), b"{}", Some(&format!("cap:route/optimize@{me}"))).await;
    assert!(refused["ran"].is_null() && refused["reason"].as_str().is_some_and(|r| !r.is_empty()), "REDIRECTED: {refused}");
    assert_eq!((d.ran("tool", "secret"), d.ran("route", "secret")), (0, 0), "REDIRECTED: the claim admitted another capability");

    // And a claim is not needed: the same envelope with none, for a permitted capability, runs.
    assert_ran(&d.call_framed(&node, &kind("route", "optimize"), b"{}", None).await, &kind("route", "optimize"));
    // Nor does a claim that names another node stop a permitted call: it is not consulted.
    assert_ran(&d.call_framed(&node, &kind("tool", "echo"), b"{}", Some("tool:echo@127.0.0.1:1")).await, &kind("tool", "echo"));

    let resources: Vec<String> = d.decisions().into_iter().map(|(_, _, r, _)| r).collect();
    assert_eq!(resources, vec![format!("tool:secret@{me}"), format!("cap:route/secret@{me}"), format!("cap:route/optimize@{me}"), format!("tool:echo@{me}")], "each decided on the kind's capability");
    d.a.shutdown().await;
}

/// **A frame that names this node as its sender is a member's frame, decided like any other.** The wasm host's MCP
/// bridge used to reach its component by an RPC to this node; what would have let that inner leg through once the
/// family is decided is an exemption for "a call from this node" — and a signal's `sender` is whatever its frame
/// says (`mycelium-core/src/connection.rs` delivers it as written: only the envelope inside is signed). There is
/// no such exemption: the bridge hands its call over in process (`mycelium-wasm-host`'s `serve_loop`), so this
/// frame — delivered here exactly as the connection delivers one off the wire, its sender this node — is decided
/// as `node:{self}`. The tool `clientonly` is the client's alone: the frame is refused, the receiver untouched,
/// and the refusal on the record under the node's own principal; the client's own envelope for the same tool is
/// admitted by the same rule. Seen failing first: with the family outside the provider's protected set, the forged
/// frame ran the capability.
#[tokio::test]
async fn a_frame_naming_this_node_as_its_sender_is_decided_as_the_node() {
    let d = door(Opts::default()).await;
    let me = d.me();
    let k = kind("tool", "clientonly");

    let mut wire = BytesMut::new();
    wire.put_u64_le(0x5eed_f00d);
    wire.put_slice(br#"{"kg": 5}"#);
    d.a.task_ctx.signal_handlers.deliver(&Signal {
        kind: Arc::from(k.as_str()),
        scope: SignalScope::Individual(d.a.node_id().clone()),
        payload: wire.freeze(),
        sender: d.a.node_id().clone(),
        nonce: 0x0bad_cafe,
    });
    let node = node_principal(d.a.node_id());
    let resource = format!("tool:clientonly@{me}");
    let decided = |d: &Door| d.decisions().into_iter().filter(|(_, _, r, _)| *r == resource).collect::<Vec<_>>();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while decided(&d).is_empty() && d.ran("tool", "clientonly") == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(d.ran("tool", "clientonly"), 0, "FORGED: a frame naming this node as its sender ran the capability undecided");
    let found = decided(&d);
    assert!(found.len() == 1 && found[0].0 == node && found[0].3 != DecisionKind::Permit, "decided as the node, and refused: {found:?}");

    // The client's own envelope, same tool, same rule.
    assert_ran(&d.call_framed(CLIENT, &k, br#"{"kg": 5}"#, None).await, &k);
    assert_eq!(d.ran("tool", "clientonly"), 1);
    d.a.shutdown().await;
}

/// **A mandate bound at the MCP door's digest verifies on a direct call of the same tool and arguments, and not for
/// other arguments** (#788's review, F4). The client holds a `depot` mandate for the tool `guarded`, its possession
/// proof signed over `tools/call`, `tool:guarded@{self}` and the digest of the tool call's `arguments` — what the
/// MCP door decides on. Presented at the MCP door the tool runs; presented on a direct `cap.invoke/tool/guarded`
/// whose payload is those arguments, the capability runs under the same mandate; with the same mandate and other
/// arguments the direct call is refused and nothing runs; with no mandate it is refused. So "one mandate covers both
/// doors" is shown end to end, for a call that carries arguments. Seen to bite: with the direct arm's arguments
/// planted to `{"payload_b64"}` (the generic arm's form) the direct call under the mandate was refused.
#[tokio::test]
async fn a_mandate_bound_at_the_mcp_door_verifies_on_a_direct_call_of_the_same_tool_and_arguments() {
    let d = door(Opts::default()).await;
    d.offer_depot_checkpoint();
    let args = json!({"order": 7, "to": ["a", "b"]});
    let mandate = d.tool_mandate(&args);
    let k = kind("tool", "guarded");

    let tool_call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "guarded", "arguments": args}});
    let at_mcp = d.call_under(CLIENT, crate::signal::signal_kind::MCP_INVOKE, tool_call.to_string().as_bytes(), &mandate).await;
    assert!(at_mcp["error"].is_null(), "plant: the mandate is established at the MCP door: {at_mcp}");
    assert_eq!(d.guarded_ran.load(Ordering::SeqCst), 1);

    assert_ran(&d.call_under(CLIENT, &k, args.to_string().as_bytes(), &mandate).await, &k);
    assert_eq!(d.ran("tool", "guarded"), 1, "the same mandate admits the direct call of the same tool and arguments");

    let other = d.call_under(CLIENT, &k, json!({"order": 8, "to": ["a", "b"]}).to_string().as_bytes(), &mandate).await;
    assert_refused(&other, "authority_not_established");
    let none = d.call_framed(CLIENT, &k, args.to_string().as_bytes(), None).await;
    assert_refused(&none, "authority_not_established");
    assert_eq!(d.ran("tool", "guarded"), 1, "other arguments, or no mandate: nothing ran");
    d.a.shutdown().await;
}

/// **What is decided is what is handed over, at both doors** (#788's review, F2 and F3).
///
/// - **A tool call with no `arguments`** is decided on `{}`, and the tool's handler is handed `{}` — it was handed
///   `null`, a value the provider had not decided on, and one whose digest differs. Seen failing first: with
///   `serve_mcp_call` handing `params.arguments` as it stood, the tool answered `null`.
/// - **A repeated object key at the MCP door** is read once, by the node: the provider decides on the parsed value
///   (the last of the repeated key) and the handler is handed that same value.
/// - **A repeated object key at the direct door** is refused `malformed_call`, at any depth: the component is handed
///   the payload's bytes, so a guest whose parser keeps the first of a repeated key would act on a value no rule and
///   no digest saw. Seen failing first: with the refusal out of `derive`, both payloads reached the receiver.
/// - Bytes that are not JSON are not that case: they are carried as `{"payload_b64"}` and run under the rule.
#[tokio::test]
async fn what_is_decided_is_what_is_handed_over_at_both_doors() {
    let d = door(Opts::default()).await;
    let text = |reply: &Value| reply["result"]["content"][0]["text"].as_str().map(str::to_string);

    let bare = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo"}});
    let reply = d.call(crate::signal::signal_kind::MCP_INVOKE, bare.to_string().as_bytes()).await;
    assert_eq!(text(&reply).as_deref(), Some("{}"), "HANDED UNDECIDED: decided on {{}}, handed something else: {reply}");

    let repeated = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"kg":1000000,"kg":1}}}"#;
    let reply = d.call(crate::signal::signal_kind::MCP_INVOKE, repeated).await;
    assert_eq!(text(&reply).as_deref(), Some(r#"{"kg":1}"#), "the MCP door hands the handler the value it decided on: {reply}");
    assert_eq!(d.tool_ran.load(Ordering::SeqCst), 2);

    let k = kind("tool", "echo");
    for payload in [&br#"{"kg":1000000,"kg":1}"#[..], br#"{"a":[{"b":1,"c":{"d":1,"d":2}}]}"#] {
        let reply = d.call(&k, payload).await;
        assert_refused(&reply, "malformed_call");
        assert!(reply["error"].as_str().unwrap().contains("repeats an object key"), "{reply}");
    }
    assert_eq!(d.ran("tool", "echo"), 0, "READ TWO WAYS: a payload with a repeated key reached the receiver");
    // The same key in two different objects is not a repeat; nor are bytes that are not JSON.
    assert_ran(&d.call(&k, br#"{"a":{"k":1},"b":{"k":2},"c":[{"k":3},{"k":4}]}"#).await, &k);
    assert_ran(&d.call(&k, br#"{"kg":1,"kg":2"#).await, &k);
    assert_eq!(d.ran("tool", "echo"), 2);
    d.a.shutdown().await;
}

/// **Enforcement off, and no cohort budget: nothing changes.** A capability no rule names runs, and the provider
/// records nothing — and a kind that names no capability, which an enforcing provider refuses, runs too. A pin,
/// not a regression test: it passes on the code as it was. Seen to bite with the family's `malformed_call` refusal
/// planted at the top of the check, ahead of its enforcement test: `cap.invoke/lonely` was refused with
/// enforcement off.
#[tokio::test]
async fn with_enforcement_off_a_hosted_capability_call_is_as_it_was() {
    let d = door(Opts { enforcing: false, ..Opts::default() }).await;
    assert_ran(&d.call(&kind("tool", "secret"), b"not json").await, &kind("tool", "secret"));
    assert_ran(&d.call(&kind("route", "secret"), b"{}").await, &kind("route", "secret"));
    assert_ran(&d.call("cap.invoke/lonely", b"{}").await, "cap.invoke/lonely");
    // A payload an enforcing provider refuses for a repeated key runs too: nothing is decided on it here.
    assert_ran(&d.call(&kind("tool", "secret"), br#"{"kg":1000000,"kg":1}"#).await, &kind("tool", "secret"));
    assert!(d.decisions().is_empty(), "no authority is checked with enforcement off");
    d.a.shutdown().await;
}

/// **A cohort budget counts a hosted capability's call as it counts any protected kind's** — with enforcement off
/// too. Every caller no cohort names shares one place: the first call is held by its receiver, the second is
/// refused `at_capacity` and never reaches it, and dropping the held request gives the place back. Seen failing
/// first: with the family outside the provider's protected set both calls were held — the budget did not see them.
#[tokio::test]
async fn a_cohort_budget_counts_a_hosted_capability_call() {
    let d = door(Opts { enforcing: false, budget: true, hold: true }).await;
    let k = kind("route", "secret");
    assert_eq!(d.call_within(&k, b"{}", Duration::from_millis(800)).await, None, "admitted, held and never answered");
    assert_eq!(d.ran("route", "secret"), 1);
    let second = d.call_within(&k, b"{}", Duration::from_millis(800)).await.expect("UNCOUNTED: the second call was held too — the budget did not count the first");
    assert_refused(&second, "at_capacity");
    assert_eq!(d.ran("route", "secret"), 1, "a refused call never reached the receiver");
    assert_eq!(d.a.cohort_budget_refusals(), 1);

    d.held.lock().unwrap().clear();
    assert_eq!(d.call_within(&k, b"{}", Duration::from_millis(800)).await, None, "a released place admits again");
    assert_eq!(d.ran("route", "secret"), 2);
    d.a.shutdown().await;
}

/// **A cancellable rule refuses a member's direct call of a hosted capability** (identity-and-lifecycle I4, §104,
/// §110): the rule permits the call only where a cancel can stop it, and a begun guest call is never interrupted —
/// so since I4's PR 2 the call is refused at its door before its second slot is read (`door_cannot_stop`, door
/// `hosted_capability`), unframed or framed, with or without a `call:` mandate. Refused *authority not established*,
/// nothing run. Seen failing first: with the family outside the provider's protected set both ran.
#[tokio::test]
async fn a_cancellable_rule_refuses_a_members_direct_call_of_a_hosted_capability() {
    let d = door(Opts::default()).await;
    let k = kind("route", "slow");
    for reply in [d.call(&k, b"{}").await, d.call_framed(&node_principal(d.a.node_id()), &k, b"{}", None).await] {
        assert_refused(&reply, "authority_not_established");
        assert_eq!((reply["data"]["detail"].as_str(), reply["data"]["door"].as_str()), (Some("door_cannot_stop"), Some("hosted_capability")), "{reply}");
    }
    assert_eq!(d.ran("route", "slow"), 0, "RAN UNCANCELLABLE");
    d.a.shutdown().await;
}

/// **A kind under the family that names no capability is refused**, not run undecided: the provider's protected
/// set is the whole `cap.invoke/` prefix. Seen failing first: with the family outside the set, it ran.
#[tokio::test]
async fn a_kind_under_the_family_that_names_no_capability_is_refused() {
    let d = door(Opts::default()).await;
    assert_refused(&d.call("cap.invoke/lonely", b"{}").await, "malformed_call");
    assert_eq!(d.ran.lock().unwrap().get("cap.invoke/lonely").copied().unwrap_or(0), 0);
    d.a.shutdown().await;
}

/// **The SDK serve stream runs the same check.** A client may serve a kind of the family this node serves no
/// component for (`GET /gateway/rpc/serve/{kind}`); a member's call of it is decided before it is streamed: with
/// no rule it is answered with the refusal and the stream is handed nothing, with a rule the stream is handed the
/// request. Seen failing first: with the family outside the provider's protected set the unruled call was
/// streamed.
#[tokio::test]
async fn the_serve_stream_decides_a_hosted_capability_kind_before_streaming_it() {
    let (port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("hosted-door-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some({
        let mut tls_cfg = crate::TlsConfig::default();
        tls_cfg.auto_cert_dir = dir.join("tls");
        tls_cfg
    });
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-hosted-serve")
            .with_catalogue("cat-hosted-serve", "1")
            .map_action(crate::HOSTED_CAPABILITY_OPERATION, "*")
            .allow(Rule::new(node_principal(&id), crate::HOSTED_CAPABILITY_OPERATION, format!("cap:sdk/open@{id}"))),
    ));
    a.with_provider_enforcement();
    a.start().await.unwrap();

    let mut saw = HashMap::new();
    for name in ["open", "shut"] {
        let events = Arc::new(AtomicUsize::new(0));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let resp = loop {
            match reqwest::Client::new().get(format!("http://127.0.0.1:{http_port}/gateway/rpc/serve/cap.invoke%2Fsdk%2F{name}")).send().await {
                Ok(r) => break r,
                Err(e) => {
                    assert!(tokio::time::Instant::now() < deadline, "the serve stream never opened: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        assert_eq!(resp.status(), 200, "the serve stream for cap.invoke/sdk/{name} is open");
        let counted = Arc::clone(&events);
        tokio::spawn(async move {
            use futures_util::StreamExt as _;
            let mut body = resp.bytes_stream();
            while let Some(Ok(chunk)) = body.next().await {
                counted.fetch_add(String::from_utf8_lossy(&chunk).matches("event: cap.invoke/sdk/").count(), Ordering::SeqCst);
            }
        });
        saw.insert(name, events);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let call = |name: &'static str| {
        let a = Arc::clone(&a);
        async move { a.service().rpc_call(a.node_id().clone(), kind("sdk", name), b"{}".to_vec(), Duration::from_millis(1500)).await }
    };
    let refused = call("shut").await.expect("STREAMED UNDECIDED: the unruled call was not answered with a refusal");
    let refused: Value = serde_json::from_slice(&refused).unwrap();
    assert_refused(&refused, "authority_not_established");
    assert_eq!(saw["shut"].load(Ordering::SeqCst), 0, "STREAMED UNDECIDED: the refused call reached the SDK agent");
    // The permitted call is streamed; nobody answers it here, so the caller times out.
    assert!(call("open").await.is_err(), "streamed to the SDK agent, which does not answer in this test");
    assert_eq!(saw["open"].load(Ordering::SeqCst), 1, "the permitted call was streamed");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Decided, signed and handed agree for a tool call with no `arguments`** (#788's confirm round, N1). The provider
/// decides such an `mcp.invoke` on `{}` and hands the tool `{}`; the client signature it checks
/// (`client_sig::binding_of`) is over the same value, and a signature over `null` no longer binds it. An explicit
/// `"arguments": null` — what the gateway's `/mcp` forwards for a client's call with none — is `null` all three
/// ways. Seen failing first: with `binding_of` reading `params.arguments` as it stood, the absent case bound
/// `digest(null)`.
#[test]
fn decided_signed_and_handed_agree_for_a_tool_call_with_no_arguments() {
    use super::client_sig::{binding_of, door_binding_mcp};
    let a = GossipAgent::new(NodeId::new("127.0.0.1", 9004).unwrap(), GossipConfig::default());
    let me = "127.0.0.1:9004";
    for (call, want) in [
        (json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo"}}), json!({})),
        (json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": null}}), Value::Null),
        (json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": {"kg": 5}}}), json!({"kg": 5})),
    ] {
        let payload = call.to_string().into_bytes();
        let decided = derive(&a.task_ctx, crate::signal::signal_kind::MCP_INVOKE, &payload, None, me).unwrap().2;
        let handed = super::mcp::decided_arguments(&call);
        let signed = binding_of(crate::signal::signal_kind::MCP_INVOKE, &payload, None).unwrap();
        assert_eq!((&decided, &handed), (&want, &want), "{call}");
        assert_eq!(signed, door_binding_mcp("echo", &want).unwrap(), "SIGNED OTHER: the signature binds another value than was decided: {call}");
    }
}

/// **What the family is decided on**, read from `derive` itself:
/// - the `tool` namespace derives **exactly** what the MCP tool loop derives for the same tool and arguments —
///   operation, resource and arguments — so a rule's value conditions and a mandate's arguments digest mean one
///   thing at both doors;
/// - any other namespace derives `cap.invoke` on `cap:{ns}/{name}@{self}`, the payload as JSON;
/// - the claim is not consulted, in either;
/// - a payload that is not JSON is carried as `{"payload_b64"}`, as the generic arm carries one;
/// - a kind that names no capability, or carries `@`, is `malformed_call`;
/// - `protected_here` is true for the whole prefix and `is_protected_kind` — which the gateway's routes and a
///   component's `mesh.emit` ask — is unchanged.
///
/// Seen failing first: with the family's arm out of `derive`, the tool namespace derived `resource_unnamed`.
#[test]
fn a_hosted_capability_is_derived_from_its_kind() {
    let a = GossipAgent::new(NodeId::new("127.0.0.1", 9003).unwrap(), GossipConfig::default());
    let ctx = &a.task_ctx;
    let me = "127.0.0.1:9003";
    let digest_of = |p: &[u8]| arguments_digest(&derive(ctx, "cap.invoke/route/optimize", p, None, me).unwrap().2);
    let args = json!({"kg": 5000.0, "to": ["a", "b"], "nested": {"z": 1, "a": null}});

    let tool_call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "echo", "arguments": args}});
    let at_mcp = derive(ctx, crate::signal::signal_kind::MCP_INVOKE, tool_call.to_string().as_bytes(), None, me).expect("derived");
    // The bridge's payload: the tool call's `arguments`, serialised.
    let direct = derive(ctx, "cap.invoke/tool/echo", args.to_string().as_bytes(), None, me).expect("derived");
    assert_eq!(direct, at_mcp, "the tool namespace is decided as the MCP tool loop decides the tool");
    assert_eq!(direct, ("tools/call".to_string(), format!("tool:echo@{me}"), args.clone()));
    assert_eq!(
        arguments_digest(&direct.2),
        arguments_digest(&at_mcp.2),
        "one arguments digest at both doors"
    );

    let other = derive(ctx, "cap.invoke/route/optimize", args.to_string().as_bytes(), Some("cap:route/other@127.0.0.1:9003"), me).expect("derived");
    assert_eq!(other, (crate::HOSTED_CAPABILITY_OPERATION.to_string(), format!("cap:route/optimize@{me}"), args.clone()), "the claim is not consulted");
    let claimed = derive(ctx, "cap.invoke/tool/echo", args.to_string().as_bytes(), Some("tool:other@127.0.0.1:1"), me).expect("derived");
    assert_eq!(claimed, direct, "nor in the tool namespace");
    // A name may hold `/` and `.`: everything after the namespace is the name.
    assert_eq!(derive(ctx, "cap.invoke/route/eu/optimize.shadow", b"{}", None, me).unwrap().1, format!("cap:route/eu/optimize.shadow@{me}"));

    // No `arguments` at the MCP door is `{}`; an explicit `null` is `null`, at both doors.
    let bare = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "echo"}});
    assert_eq!(derive(ctx, crate::signal::signal_kind::MCP_INVOKE, bare.to_string().as_bytes(), None, me).unwrap().2, json!({}));
    assert_eq!(super::mcp::decided_arguments(&bare), json!({}), "and the handler is handed the same");
    let null = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "echo", "arguments": null}});
    assert_eq!(derive(ctx, crate::signal::signal_kind::MCP_INVOKE, null.to_string().as_bytes(), None, me).unwrap().2, Value::Null);
    assert_eq!(derive(ctx, "cap.invoke/tool/echo", b"null", None, me).unwrap().2, Value::Null);

    // A repeated object key, at any depth, is refused at the direct door — only in a payload that is JSON as a whole;
    // numbers RFC 8785 writes alike share a digest.
    for whole_not_json in [&br#"{"a":1,"a":2,"b":1e400}"#[..], br#"{"a":1,"a":2,"b":"\ud800"}"#, br#"{"a":1,"a":2"#] {
        let (_, _, args) = derive(ctx, "cap.invoke/route/optimize", whole_not_json, None, me).expect("carried, not refused");
        assert!(args.get("payload_b64").is_some(), "{}: not JSON as a whole, so carried as bytes", String::from_utf8_lossy(whole_not_json));
    }
    assert_eq!(digest_of(br#"{"n":0.1}"#), digest_of(br#"{"n":0.1000000000000000055}"#), "two decimals that round to one double");
    for (payload, repeats) in [(&br#"{"a":1,"a":2}"#[..], true), (br#"[{"x":{"y":1,"y":1}}]"#, true), (br#"{"a":{"k":1},"b":{"k":1}}"#, false), (b"plain", false), (b"", false), (br#"{"a":1} trailing"#, false)] {
        assert_eq!(super::provider_enforcement::repeats_a_key(payload), repeats, "{}", String::from_utf8_lossy(payload));
    }
    assert_eq!(derive(ctx, "cap.invoke/route/optimize", br#"{"a":1,"a":2}"#, None, me).unwrap_err().reason, "malformed_call");
    let digest = digest_of;
    assert_eq!(digest(br#"{"n":1}"#), digest(br#"{"n":1.0}"#), "equal numbers, one digest: RFC 8785's canonical form, by design");
    assert_ne!(digest(br#"{"n":1}"#), digest(br#"{"n":2}"#));

    let raw = derive(ctx, "cap.invoke/text/echo", b"plain bytes", None, me).expect("derived");
    assert_eq!(raw.2, json!({"payload_b64": "cGxhaW4gYnl0ZXM="}), "a payload that is not JSON");
    assert_eq!(derive(ctx, "cap.invoke/text/echo", b"", None, me).unwrap().2, json!({"payload_b64": ""}), "an empty payload is not JSON");

    for bad in ["cap.invoke/", "cap.invoke/lonely", "cap.invoke//name", "cap.invoke/ns/", "cap.invoke/tool/x@10.0.0.9:7000", "cap.invoke/n@s/x"] {
        assert_eq!(derive(ctx, bad, b"{}", Some(&format!("tool:echo@{me}")), me).unwrap_err().reason, "malformed_call", "{bad}");
        assert!(protected_here(&ctx.config, bad), "{bad}: the whole prefix is protected at the provider");
    }
    assert!(protected_here(&ctx.config, "cap.invoke/tool/echo") && protected_here(&ctx.config, "mcp.invoke") && !protected_here(&ctx.config, "cap.invoke") && !protected_here(&ctx.config, "comp/x/y"));
    assert!(!crate::is_protected_kind(&ctx.config, "cap.invoke/tool/echo"), "the public predicate is the exact-kind list, unchanged");
    assert_eq!(crate::hosted_capability_of("cap.invoke/tool/echo"), Some(("tool", "echo")));
    assert_eq!(crate::hosted_capability_of("mcp.invoke"), None);
}
