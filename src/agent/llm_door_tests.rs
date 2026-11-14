//! **The built-in `llm.invoke` provider runs the provider check** (#781's review, F1): `handle_llm_invoke` calls
//! `provider_enforcement::check` after it verifies the caller and before it parses the request, looks up a backend or
//! reads a template, and races the backend against the admission's lapse.
//!
//! One node is the gateway that frames each envelope, the provider that serves it through the real dispatch loop,
//! and the caller. The backends count what reaches them, so every "nothing ran" below is read from the backend, not
//! from the absence of an answer.
//!
//! **Why this is not a row of the C7 bypass matrix** (`test_c7_the_bypass_matrix_no_door_runs_revoked_work`): the
//! matrix is built under `compliance` + `a2a` and runs on CI's strict `compliance,a2a` lib line; a prompt skill needs
//! `llm`, which that line does not build. The matrix's question for this door — does revoked work run — is asked
//! here, by `a_revoked_mandate_runs_no_llm_backend`.
//!
//! **Two feature sets, and where CI runs each.** Everything at the top of this file needs `tls` + `llm` and runs on
//! CI's `tls,metrics,a2a,llm,test-frame-filter` lib line. The tests in [`with_member_keys`] also need `compliance`:
//! a `call:` mandate is issued by a gateway **node**, and a node's key is verified only through the member-key view
//! `compliance` builds (`gateway_member_keys`); so is a client named by a gateway token. No lib line builds
//! `compliance` with `llm`, so CI's `test` job runs them in a step of their own
//! (`--lib --features compliance,llm llm_door_tests`).

use super::action_evaluator::{AeEvidence, DecisionKind, Execution, RecordKind};
#[cfg(feature = "compliance")]
use super::gateway_authority::call_checkpoint_key;
use super::gateway_authority::{arguments_digest, call_mint, possession_request, ExecutionAuthority, PresentedMandate};
use super::gateway_caller::{frame_envelope_minting_at, node_principal};
use super::llm::{LlmBackend, LlmError, LlmResult};
use super::provider_enforcement::ENFORCEMENT_POINT_PROVIDER;
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::knowledge::IssuerId;
use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint};
use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use crate::{GossipAgent, GossipConfig, NodeId, PromptTemplate, ReferenceEvaluator, Rule};
use bytes::Bytes;
use ed25519_dalek::SigningKey;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A window no test here outlives.
const LONG_WINDOW_MS: u64 = 3_000_000;
const LLM: &str = "llm.invoke";
/// The gateway client that holds the `depot` mandate: a principal whose key the node is configured to trust, so
/// its possession proof verifies in a build without `compliance`.
const CALLER: &str = "token:gw/agent-1";

/// A backend that counts: how many calls began, and how many ran to their end. `hold` is how long one takes;
/// `fails` makes it answer an error, as an endpoint that is down does.
struct Probe {
    began: AtomicUsize,
    ended: AtomicUsize,
    hold: Duration,
    fails: bool,
    /// When set, a call waits for a permit here in place of `hold`: the test decides when it ends.
    gate: Option<tokio::sync::Semaphore>,
}

impl Probe {
    fn new(hold: Duration, fails: bool) -> Arc<Self> {
        Arc::new(Self { began: AtomicUsize::new(0), ended: AtomicUsize::new(0), hold, fails, gate: None })
    }
    fn gated() -> Arc<Self> {
        Arc::new(Self { began: AtomicUsize::new(0), ended: AtomicUsize::new(0), hold: Duration::ZERO, fails: false, gate: Some(tokio::sync::Semaphore::new(0)) })
    }
    fn counts(&self) -> (usize, usize) {
        (self.began.load(Ordering::SeqCst), self.ended.load(Ordering::SeqCst))
    }
}

struct ProbeBackend(Arc<Probe>);

#[async_trait::async_trait]
impl LlmBackend for ProbeBackend {
    async fn complete(&self, _system: &str, user: &str, _max_tokens: u32, _temperature: f32) -> Result<LlmResult, LlmError> {
        self.0.began.fetch_add(1, Ordering::SeqCst);
        match &self.0.gate {
            Some(gate) => gate.acquire().await.expect("open").forget(),
            None => tokio::time::sleep(self.0.hold).await,
        }
        self.0.ended.fetch_add(1, Ordering::SeqCst);
        if self.0.fails {
            return Err(LlmError::Api("503: the endpoint is down".into()));
        }
        Ok(LlmResult { output: format!("ran: {user}"), model_used: "probe".into(), tokens_used: 1 })
    }
}

/// How a test's node is set up.
#[derive(Clone, Copy)]
struct Opts {
    /// `with_provider_enforcement`.
    enforcing: bool,
    /// Attach the evaluator.
    evaluator: bool,
    /// The gate's freshness bound. 2 s makes the authority's sweep run every 100 ms (a cancel is seen at once);
    /// a test that holds a `depot` mandate takes a longer one, or the mandate goes stale under it.
    freshness_ms: u64,
    /// A gateway token for [`CALLER`] (`agent-1` / `s3cret`), so a request through the gateway is that client's.
    /// A token table needs `compliance`.
    token: bool,
    /// Attach a cohort budget in which every caller no cohort names shares **one** place.
    budget: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { enforcing: true, evaluator: true, freshness_ms: 2_000, token: false, budget: false }
    }
}

struct Door {
    a: Arc<GossipAgent>,
    authority: Arc<ExecutionAuthority>,
    dir: std::path::PathBuf,
    http_port: u16,
    /// The backend of `review/plain`, `review/denied`, `review/guarded` and `review/unruled`.
    fast: Arc<Probe>,
    /// The backend of `review/slow` and `review/slowguarded`: 30 s a call.
    slow: Arc<Probe>,
    /// The backend of `review/failing`.
    failing: Arc<Probe>,
    /// The backend of `review/held`: a call ends when the test adds a permit.
    held: Arc<Probe>,
    /// The operator whose `depot` mandates the node's authority honours.
    operator: SigningKey,
    /// [`CALLER`]'s key.
    holder: SigningKey,
}

impl Drop for Door {
    fn drop(&mut self) {
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.dir.starts_with(std::env::temp_dir()) && self.dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("llm-door-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// One node. Its policy on `llm.invoke`: `review/plain` and `review/failing` are allowed; `review/slow` is allowed
/// and **cancellable**; `review/guarded` and `review/slowguarded` need a `depot` mandate; `review/denied` is
/// prohibited; `review/unruled` has a backend and no rule.
async fn door(opts: Opts) -> Door {
    let (port, http_port) = (crate::test_util::alloc_port(), crate::test_util::alloc_port());
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let dir = std::env::temp_dir().join(format!("llm-door-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    if opts.token {
        cfg.gateway_identity_issuer = Some("gw".into());
        cfg.gateway_named_tokens = vec![crate::config::GatewayNamedToken { name: "agent-1".into(), token: "s3cret".into(), scopes: vec!["*".into()] }];
    }
    let a = Arc::new(GossipAgent::new(id.clone(), cfg));
    let at = |name: &str| format!("prompt:review/{name}@{id}");
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    if opts.evaluator {
        a.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-llm-door")
                .with_catalogue("cat-llm-door", "1")
                .map_action(LLM, "*")
                .allow(Rule::new("*", LLM, at("plain")))
                .allow(Rule::new("*", LLM, at("failing")))
                .allow(Rule::new("*", LLM, at("slow")).cancellable())
                .allow(Rule::new("*", LLM, at("guarded")).requiring_mandate("depot"))
                .allow(Rule::new("*", LLM, at("slowguarded")).requiring_mandate("depot"))
                .prohibit(Rule::new("*", LLM, at("denied"))),
        ));
    }
    let (operator, holder) = (SigningKey::from_bytes(&[60u8; 32]), SigningKey::from_bytes(&[62u8; 32]));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    entitlements.entitle_grants(PrincipalId::new(node_principal(&id)).unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), operator.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder.verifying_key().to_bytes()).unwrap();
    let f = opts.freshness_ms;
    let gate = ExecutionGate::strict(ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: f / 20 }, FreshnessPolicy { freshness_ms: f, interval_ms: f / 4, delivery_ms: f / 12 }).unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external));
    a.with_execution_authority(Arc::clone(&authority));
    if opts.enforcing {
        a.with_provider_enforcement();
    }
    if opts.budget {
        a.with_cohort_budget(crate::knowledge::cohort_budget::CohortBudget::new(3, 1), crate::knowledge::cohort::CohortView::trusting([]), TrustedExternalIssuers::new());
    }
    a.start().await.unwrap();
    let (fast, slow, failing, held) = (Probe::new(Duration::ZERO, false), Probe::new(Duration::from_secs(30), false), Probe::new(Duration::ZERO, true), Probe::gated());
    for (name, probe) in [("held", &held), ("plain", &fast), ("denied", &fast), ("guarded", &fast), ("unruled", &fast), ("slow", &slow), ("slowguarded", &slow), ("failing", &failing)] {
        let template = PromptTemplate { system: String::new(), user_template: "{{input}}".into(), max_tokens: 32, temperature: 0.0, metadata: Default::default() };
        std::mem::forget(a.llm().register_prompt_skill("review", name, template, Arc::new(ProbeBackend(Arc::clone(probe)))).await.unwrap());
    }
    assert!(until(|| std::net::TcpStream::connect(("127.0.0.1", http_port)).is_ok(), Duration::from_secs(20)).await, "the gateway is listening");
    Door { a, authority, dir, http_port, fast, slow, failing, held, operator, holder }
}

/// What goes in the envelope's second slot.
#[derive(Clone, Copy, PartialEq)]
enum Cm {
    /// What an honest gateway mints: term `c{n}` of this envelope. Verified only with the member-key view.
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))]
    Honest,
    /// Nothing: a gateway that does not mint, or a slot stripped on the way.
    None,
}

impl Door {
    fn me(&self) -> String {
        self.a.node_id().to_string()
    }
    fn gateway(&self) -> String {
        node_principal(self.a.node_id())
    }
    fn key(&self) -> Arc<SigningKey> {
        self.a.task_ctx.tls.get().map(|t| t.signing_key()).expect("the node has a TLS identity")
    }
    fn now(&self) -> u64 {
        self.a.task_ctx.hlc.decision_now_ms()
    }
    fn resource(&self, name: &str) -> String {
        format!("prompt:review/{name}@{}", self.me())
    }
    /// The application payload of a call to `review/{name}`, and the arguments a mandate binds — the gateway's.
    fn payload(name: &str) -> (Vec<u8>, serde_json::Value) {
        let (input, context) = ("the text", serde_json::json!({ "tone": "plain" }));
        (serde_json::json!({ "prompt": format!("review/{name}"), "input": input, "context": context }).to_string().into_bytes(), serde_json::json!({ "input": input, "context": context }))
    }
    /// Frame a call to `review/{name}` as this node's gateway does — for `principal`, carrying `mandate` in the first
    /// slot, the resource claim `claim` (the gateway's LLM doors send `prompt:{ns}/{name}@{target}`) and `cm` in
    /// the second slot. Returns the framed bytes and the envelope's id.
    fn frame_as(&self, principal: &str, name: &str, mandate: Option<&serde_json::Value>, claim: Option<&str>, cm: Cm) -> (Bytes, u64) {
        let (payload, arguments) = Self::payload(name);
        let id = std::cell::Cell::new(0u64);
        let mint = |n: u64| {
            id.set(n);
            let now = self.now();
            (cm == Cm::Honest).then(|| serde_json::to_value(call_mint::mint(&self.key(), &self.gateway(), &format!("c{n}"), LLM, &self.resource(name), &arguments, now, now + LONG_WINDOW_MS)).unwrap())
        };
        let framed = frame_envelope_minting_at(&self.a.task_ctx, self.a.node_id(), principal, &[], Bytes::from(payload), mandate, claim, None, None, self.now(), Some(&mint)).expect("fits");
        (framed, id.get())
    }
    /// [`frame_as`](Self::frame_as) for the node itself, with no mandate.
    fn frame(&self, name: &str, claim: Option<&str>, cm: Cm) -> (Bytes, u64) {
        self.frame_as(&self.gateway(), name, None, claim, cm)
    }
    /// [`CALLER`]'s call to `review/{name}` under `mandate`, sent straight to the provider.
    async fn direct(&self, name: &str, mandate: &serde_json::Value) -> serde_json::Value {
        let (framed, _) = self.frame_as(CALLER, name, Some(mandate), Some(&self.resource(name)), Cm::None);
        self.send(framed).await
    }
    /// Send a framed call to this node's own `llm.invoke` door and return its reply.
    async fn send(&self, framed: Bytes) -> serde_json::Value {
        let reply = super::rpc::rpc_call_framed(&self.a.task_ctx, self.a.node_id().clone(), Arc::from(LLM), framed, Duration::from_secs(60)).await.expect("a reply");
        serde_json::from_slice(&reply).unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(&reply) }))
    }
    /// A call to `review/{name}` as the gateway's LLM doors frame it.
    async fn call(&self, name: &str, cm: Cm) -> (serde_json::Value, u64) {
        let (framed, id) = self.frame(name, Some(&self.resource(name)), cm);
        (self.send(framed).await, id)
    }
    /// The gateway cancels the call of envelope `n`: the revoking checkpoint, written where a provider reads it.
    #[cfg(feature = "compliance")]
    fn cancel(&self, n: u64) {
        let term = format!("c{n}");
        let cp = call_mint::checkpoint(&self.key(), &self.gateway(), &format!("call:{term}"), 2, self.now(), &[term.as_str()]);
        super::state_machine::write_owned_kv(&self.a.task_ctx, &call_checkpoint_key(&self.gateway(), &term), serde_json::to_vec(&cp).unwrap());
    }
    /// The operator's checkpoint for `depot`, offered to the node.
    fn offer_depot_checkpoint(&self, seq: u64, revoked: &[&str]) {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: self.now(),
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        let signed = SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&self.operator, &c.canonical_bytes()).to_vec(), checkpoint: c };
        self.a.offer_revocation_checkpoint(&signed).unwrap();
    }
    /// A `depot` mandate held by [`CALLER`] for a call to `review/{name}`, presented with its possession proof over
    /// the operation, resource and arguments **the gateway's LLM doors decide on**.
    fn depot_mandate(&self, name: &str) -> serde_json::Value {
        let (_, arguments) = Self::payload(name);
        let m = Mandate {
            holder: PrincipalId::new(CALLER).unwrap(),
            established_by: PrincipalId::new("operator:acme").unwrap(),
            purpose: "depot".into(),
            scope: "depot".into(),
            operations: vec![format!("{LLM}:prompt:review/{name}")],
            epoch: 1,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: self.now() + LONG_WINDOW_MS,
        };
        let proof = mycelium_core::tls::sign_bytes(&self.holder, &possession_message(&m, &possession_request(LLM, &self.resource(name), &arguments_digest(&arguments))));
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&self.operator, &m.canonical_bytes()).to_vec(), mandate: m };
        serde_json::to_value(PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) }).unwrap()
    }
    /// The provider's records in the journal.
    fn provider_records(&self) -> Vec<AeEvidence> {
        crate::read_evidence_journal(&self.dir.join("evidence.log")).unwrap().iter().map(|b| serde_json::from_slice::<AeEvidence>(b).unwrap())
            .filter(|e| e.enforcement_point == ENFORCEMENT_POINT_PROVIDER)
            .collect()
    }
    /// The provider's execution records saying nothing ran.
    fn nothing_ran(&self) -> usize {
        self.provider_records().iter().filter(|e| e.kind == RecordKind::Execution && e.execution == Execution::None).count()
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.http_port)
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

/// The door's refusal: `error` the stable reason, `detail` the sentence, `data.reason` the same reason.
#[track_caller]
fn assert_refused(reply: &serde_json::Value, reason: &str) {
    assert_eq!((reply["error"].as_str(), reply["data"]["reason"].as_str()), (Some(reason), Some(reason)), "{reply}");
    assert!(reply["detail"].as_str().is_some_and(|d| !d.is_empty()) && reply["output"].is_null(), "{reply}");
}

/// The `call:` refusal, in this door's shape: `error` `authority_not_established`, `data.detail`
/// `call_mandate_not_established`, and the stable `data.cause`.
#[track_caller]
fn assert_not_established(reply: &serde_json::Value, cause: &str) {
    assert_refused(reply, "authority_not_established");
    assert_eq!((reply["code"].as_i64(), reply["data"]["detail"].as_str(), reply["data"]["cause"].as_str()), (Some(-32031), Some("call_mandate_not_established"), Some(cause)), "{reply}");
}

/// **A rule on `llm.invoke` is applied at the built-in provider.** A prohibited prompt skill is refused
/// `action_denied` and its backend is never called; one with no rule is refused too; a permitted one runs. The
/// provider decides on the prompt skill **the payload names**: an envelope that claims the permitted skill and
/// carries a payload naming the prohibited one is refused, and a member's own call — which carries no claim — is
/// decided like any other (`call_prompt_skill`: the permitted skill answers, the prohibited one is an error naming
/// the reason). A payload that names no prompt skill is `malformed_call`. Each decision is in the provider's
/// journal, on the resource `prompt:{ns}/{name}@{self}`. Seen failing first: with the check taken out of
/// `handle_llm_invoke`, the prohibited skill's backend answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rule_on_llm_invoke_is_applied_at_the_built_in_provider() {
    let d = door(Opts::default()).await;

    let (reply, _) = d.call("denied", Cm::None).await;
    assert_refused(&reply, "action_denied");
    let (reply, _) = d.call("unruled", Cm::None).await;
    assert!(reply["data"]["reason"].is_string() && reply["output"].is_null(), "no rule, no run: {reply}");
    assert_eq!(d.fast.counts(), (0, 0), "neither backend was called");

    let (reply, _) = d.call("plain", Cm::None).await;
    assert_eq!((reply["output"].as_str(), reply["error"].is_null()), (Some("ran: the text"), true), "{reply}");
    assert_eq!(d.fast.counts(), (1, 1), "the permitted skill ran");

    // The claim is not what is decided on: it names the permitted skill, the payload the prohibited one.
    let (framed, _) = d.frame("denied", Some(&d.resource("plain")), Cm::None);
    assert_refused(&d.send(framed).await, "action_denied");
    // No claim at all — a member's own call, or `mycelium-reason`'s router: decided on the payload's skill.
    let (framed, _) = d.frame("plain", None, Cm::None);
    assert_eq!(d.send(framed).await["output"].as_str(), Some("ran: the text"));
    assert_eq!(d.a.llm().call_prompt_skill("review", "plain", "hello", Default::default(), Duration::from_secs(10)).await.unwrap(), "ran: hello");
    let refused = d.a.llm().call_prompt_skill("review", "denied", "hello", Default::default(), Duration::from_secs(10)).await.unwrap_err().to_string();
    assert!(refused.contains("action_denied"), "a refusal is an error that names its reason, never an answer: {refused}");
    assert_eq!(d.fast.counts(), (3, 3), "three permitted calls ran; no prohibited one did");

    // A payload that names no prompt skill.
    for payload in [&b"not json"[..], br#"{"input":"x","context":{}}"#, br#"{"prompt":"review","input":"x","context":{}}"#, br#"{"prompt":"a/b/c","input":"x","context":{}}"#] {
        let framed = super::gateway_caller::frame_with_context(&d.a.task_ctx, d.a.node_id(), "oidc:idp/alice", &[], Bytes::copy_from_slice(payload)).unwrap();
        assert_refused(&d.send(framed).await, "malformed_call");
    }

    let decided: Vec<(String, DecisionKind)> = d.provider_records().into_iter().filter(|e| e.kind == RecordKind::Decided).map(|e| (e.resource, e.decision)).collect();
    assert!(decided.contains(&(d.resource("denied"), DecisionKind::Deny)), "{decided:?}");
    assert!(decided.contains(&(d.resource("plain"), DecisionKind::Permit)), "{decided:?}");
    assert!(decided.iter().all(|(r, _)| r.starts_with("prompt:review/")), "every decision names the payload's skill: {decided:?}");
    d.a.shutdown().await;
}

/// **A cancellable rule refuses `llm.invoke` that carries no `call:` mandate.** No gateway mints one yet (I4's
/// PR 2), so the call as the gateway's LLM doors frame it is refused `call_mandate_not_established`
/// (`no_call_mandate`): the backend is not called, nothing is registered, and the journal says nothing ran. (With
/// an honest `cm` the call runs: `with_member_keys`.) Seen failing first: with the check taken out, the backend
/// runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancellable_rule_refuses_llm_invoke_without_a_call_mandate() {
    let d = door(Opts::default()).await;
    let before = d.nothing_ran();
    let (reply, _) = d.call("slow", Cm::None).await;
    assert_not_established(&reply, "no_call_mandate");
    assert_eq!((d.slow.counts(), d.authority.running_count()), ((0, 0), 0), "the backend was not called and nothing is registered");
    assert_eq!(d.nothing_ran(), before + 1, "the journal says nothing ran");
    d.a.shutdown().await;
}

/// **The C7 question at this door: a revoked mandate runs no backend.** `review/guarded` needs a `depot` mandate.
///
/// **The plant:** [`CALLER`]'s call under a valid mandate reaches the backend — so the refusal below does not come
/// from a door that refuses everything, and the provider derives the operation, the resource and the arguments the
/// gateway's LLM doors do: the possession proof is over `llm.invoke`, `prompt:review/guarded@{self}` and
/// `{"input", "context"}`, and it is established. Without a mandate the call is refused.
///
/// Then the operator revokes the term and the call is sent again, in a new envelope: the backend's count does not
/// move. (Through the gateway as well: `with_member_keys`.) Seen failing first: with the check taken out, the
/// backend runs after the revocation; with the `llm.invoke` arm taken out of `derive`, the plant is refused — the
/// proof does not match `{"payload_b64"}`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_mandate_runs_no_llm_backend() {
    let d = door(Opts { freshness_ms: 120_000, ..Opts::default() }).await;
    d.offer_depot_checkpoint(1, &[]);
    let mandate = d.depot_mandate("guarded");

    let (framed, _) = d.frame_as(CALLER, "guarded", None, Some(&d.resource("guarded")), Cm::None);
    let reply = d.send(framed).await;
    assert!(reply["data"]["reason"].is_string() && reply["output"].is_null(), "no mandate, no run: {reply}");
    assert_eq!(d.fast.counts(), (0, 0));

    let reply = d.direct("guarded", &mandate).await;
    assert_eq!(reply["output"].as_str(), Some("ran: the text"), "plant: the call under a valid mandate reaches the backend: {reply}");
    assert_eq!(d.fast.counts(), (1, 1));

    d.offer_depot_checkpoint(2, &["t1"]);
    let reply = d.direct("guarded", &mandate).await;
    assert_refused(&reply, "authority_not_established");
    assert_eq!(d.fast.counts(), (1, 1), "the backend did not run after the revocation");
    d.a.shutdown().await;
}

/// **A mandate that lapses while the backend runs stops the call.** `review/slowguarded` runs under [`CALLER`]'s
/// `depot` mandate, its backend 30 s from answering, its work registered. The operator revokes the term: within
/// the sweep's bound the backend's future is dropped — it never reaches its end — the caller is answered
/// `cancelled`, the registration is gone, and the stop record names the mandate's term. **What this does not
/// show**, because no dropped future can do it: that an endpoint already generating stops, or that a task a
/// backend spawned for itself does. Seen failing first: with the race against `lapsed()` taken out, the call is
/// still running when the test's wait ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mandate_that_lapses_stops_a_running_llm_invoke() {
    let d = Arc::new(door(Opts { freshness_ms: 20_000, ..Opts::default() }).await);
    d.offer_depot_checkpoint(1, &[]);
    let mandate = d.depot_mandate("slowguarded");
    let d2 = Arc::clone(&d);
    let running = tokio::spawn(async move { d2.direct("slowguarded", &mandate).await });
    assert!(until(|| d.authority.running_count() == 1 && d.slow.counts() == (1, 0), Duration::from_secs(20)).await, "the backend is running under a registration: {:?}", d.slow.counts());
    let asked = std::time::Instant::now();
    d.offer_depot_checkpoint(2, &["t1"]);
    let reply = tokio::time::timeout(Duration::from_secs(15), running).await.expect("answered long before the backend's 30 s").unwrap();
    assert!(asked.elapsed() < Duration::from_secs(10), "stopped within the sweep's bound: {:?}", asked.elapsed());
    assert_refused(&reply, "cancelled");
    assert!(reply["detail"].as_str().unwrap().contains("the call's authority lapsed while it ran"), "{reply}");
    assert!(until(|| d.authority.running_count() == 0, Duration::from_secs(20)).await, "the registration is released");
    assert_eq!(d.slow.counts(), (1, 0), "the backend's future was dropped: it did not reach its end");
    assert_eq!(d.authority.named_stop_records().last().and_then(|s| s.term.clone()).as_deref(), Some("t1"), "the stop record names the mandate's term");
    d.a.shutdown().await;
}

/// **With provider enforcement off, the door is unchanged.** The same node, policy and authority, without
/// `with_provider_enforcement`: the prohibited skill and the cancellable one answer from their backends — the
/// evaluator is not consulted here, as before — in the same three fields; nothing is written to the journal as the
/// provider; and a payload that does not parse is still this door's `parse_error`, not the check's
/// `malformed_call`. Seen failing first: with the check's `enforcing` gate taken out of the way (enforcement forced
/// on), the prohibited skill is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_enforcement_off_llm_invoke_is_unchanged() {
    let d = door(Opts { enforcing: false, ..Opts::default() }).await;
    for name in ["denied", "plain", "unruled"] {
        let (reply, _) = d.call(name, Cm::None).await;
        let mut keys: Vec<&str> = reply.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!((reply["output"].as_str(), keys), (Some("ran: the text"), vec!["model_used", "output", "tokens_used"]), "{name}: {reply}");
    }
    assert_eq!(d.fast.counts(), (3, 3));
    let framed = super::gateway_caller::frame_with_context(&d.a.task_ctx, d.a.node_id(), "oidc:idp/alice", &[], Bytes::from_static(b"not json")).unwrap();
    assert_eq!(d.send(framed).await["error"], "parse_error");
    assert!(d.provider_records().is_empty(), "the provider decided nothing and recorded nothing");
    d.a.shutdown().await;
}

/// **A cohort budget counts `llm.invoke`, with enforcement off too, and the gateway answers a full pool `429`**
/// (#785's review, F4). The node has a cohort budget — one place for every caller no cohort names — and **no**
/// provider enforcement, so no policy is consulted: the prohibited skill would run. A call to `review/held` is in
/// flight, its backend waiting on the test. While it holds the place, a second call is refused `at_capacity` at
/// the door (`code` -32004) and its backend is not called; `/gateway/llm/call` answers that refusal **429** with the
/// provider's body, and `/gateway/llm/stream` as an error event. When the first call ends the place is free and
/// the next call runs. Before the door ran the check, `llm.invoke` was outside the budget. Seen failing first:
/// with the check taken out of `handle_llm_invoke`, the second call runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cohort_budget_counts_llm_invoke_with_enforcement_off_and_a_full_pool_is_429() {
    let d = Arc::new(door(Opts { enforcing: false, budget: true, ..Opts::default() }).await);
    let (framed, _) = d.frame("held", Some(&d.resource("held")), Cm::None);
    let d2 = Arc::clone(&d);
    let first = tokio::spawn(async move { d2.send(framed).await });
    assert!(until(|| d.held.counts() == (1, 0), Duration::from_secs(20)).await, "the first call's backend is running");

    let (reply, _) = d.call("denied", Cm::None).await;
    assert_refused(&reply, "at_capacity");
    assert_eq!(reply["code"].as_i64(), Some(-32004), "{reply}");
    assert_eq!(d.fast.counts(), (0, 0), "the refused call's backend was not called");

    let http = reqwest::Client::new();
    let body = serde_json::json!({ "ns": "review", "name": "plain", "input": "the text", "context": { "tone": "plain" } });
    let r = http.post(d.url("/gateway/llm/call")).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 429, "a full cohort is 429 at the gateway");
    assert_refused(&r.json::<serde_json::Value>().await.unwrap(), "at_capacity");
    let text = http.post(d.url("/gateway/llm/stream")).json(&body).send().await.unwrap().text().await.unwrap();
    let v: serde_json::Value = serde_json::from_str(text.lines().find_map(|l| l.strip_prefix("data:")).expect("one event").trim()).unwrap();
    assert_eq!((v["type"].as_str(), v["error"].as_str(), v["data"]["reason"].as_str()), (Some("error"), Some("at_capacity"), Some("at_capacity")), "{v}");
    assert_eq!(d.fast.counts(), (0, 0), "nothing ran while the pool was full");

    d.held.gate.as_ref().unwrap().add_permits(1);
    assert_eq!(first.await.unwrap()["output"].as_str(), Some("ran: the text"), "the first call ran to its end");
    // The place is released when the handler's task ends, just after its answer is sent.
    let d3 = Arc::clone(&d);
    let mut ran = false;
    for _ in 0..200 {
        if d3.call("denied", Cm::None).await.0["output"].is_string() {
            ran = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ran, "the released place admits the next call — and with enforcement off no policy refuses it");
    d.a.shutdown().await;
}

/// **With enforcement on and no evaluator, the door refuses** (`no_evaluator`), as `rpc_rx` does: nothing can
/// decide, so nothing runs. Seen failing first: with the check taken out, the backend answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enforcing_llm_provider_with_no_evaluator_refuses() {
    let d = door(Opts { evaluator: false, ..Opts::default() }).await;
    let (reply, _) = d.call("plain", Cm::None).await;
    assert_refused(&reply, "no_evaluator");
    assert_eq!(d.fast.counts(), (0, 0));
    d.a.shutdown().await;
}

/// **The gateway's LLM doors answer a provider's refusal as a refusal.** The gateway's own preflight permits the
/// cancellable skill; the provider refuses it (no gateway mints a `call:` mandate yet). `/gateway/llm/call` answers
/// **403** with the provider's reason, detail and data — not 502, which is what an upstream failure is, and which
/// the failing backend still gets — and `/gateway/llm/stream` sends an error event carrying the same, never a
/// `done` with an empty output. A backend failure on the stream is an error event too. Seen failing first: with
/// `provider_refusal_status` not consulted, `/call` answers 502; with the stream's error arm taken out, the stream
/// says `done`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateways_llm_doors_answer_a_providers_refusal_as_a_refusal() {
    let d = door(Opts::default()).await;
    let http = reqwest::Client::new();
    let body = |name: &str| serde_json::json!({ "ns": "review", "name": name, "input": "the text", "context": { "tone": "plain" } });

    let r = http.post(d.url("/gateway/llm/call")).json(&body("slow")).send().await.unwrap();
    assert_eq!(r.status(), 403, "a provider's refusal is a refusal, not an upstream failure");
    assert_not_established(&r.json::<serde_json::Value>().await.unwrap(), "no_call_mandate");

    let r = http.post(d.url("/gateway/llm/call")).json(&body("failing")).send().await.unwrap();
    assert_eq!(r.status(), 502, "a backend's failure is still an upstream failure");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "llm_error");

    let r = http.post(d.url("/gateway/llm/call")).json(&body("plain")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["output"], "ran: the text");

    let event = |text: String| serde_json::from_str::<serde_json::Value>(text.lines().find_map(|l| l.strip_prefix("data:")).expect("one event").trim()).unwrap();
    let v = event(http.post(d.url("/gateway/llm/stream")).json(&body("slow")).send().await.unwrap().text().await.unwrap());
    assert_eq!((v["type"].as_str(), v["error"].as_str(), v["data"]["detail"].as_str(), v["data"]["cause"].as_str()), (Some("error"), Some("authority_not_established"), Some("call_mandate_not_established"), Some("no_call_mandate")), "{v}");
    let v = event(http.post(d.url("/gateway/llm/stream")).json(&body("failing")).send().await.unwrap().text().await.unwrap());
    assert_eq!((v["type"].as_str(), v["error"].as_str()), (Some("error"), Some("llm_error")), "{v}");
    let v = event(http.post(d.url("/gateway/llm/stream")).json(&body("plain")).send().await.unwrap().text().await.unwrap());
    assert_eq!((v["type"].as_str(), v["output"].as_str()), (Some("done"), Some("ran: the text")), "{v}");

    assert_eq!((d.slow.counts(), d.failing.counts(), d.fast.counts()), ((0, 0), (2, 2), (2, 2)), "the refused skill's backend was never called");
    d.a.shutdown().await;
}

/// The tests that need the member-key view (`compliance`): a `call:` mandate, issued by the gateway's node, and a
/// client named by a gateway token.
#[cfg(feature = "compliance")]
mod with_member_keys {
    use super::*;

    /// **A cancellable `llm.invoke` runs registered under its gateway's `call:` mandate, and a cancel stops it.**
    /// The `cm` is minted over `llm.invoke`, `prompt:{ns}/{name}@{self}` and `{"input", "context"}` — what the
    /// gateway's LLM doors decide on, and what the mint of I4's PR 2 must bind. The call is admitted, its work
    /// registered, its backend 30 s from answering. The gateway cancels it: within the sweep's bound the backend's
    /// future is dropped, the caller is answered `cancelled`, the registration is gone, and the stop record names
    /// the call's term. Seen failing first: with the race against `lapsed()` taken out, the call is still running
    /// when the test's wait ends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancellable_llm_invoke_runs_under_its_call_mandate_and_a_cancel_stops_it() {
        let d = Arc::new(door(Opts::default()).await);
        let (framed, id) = d.frame("slow", Some(&d.resource("slow")), Cm::Honest);
        let d2 = Arc::clone(&d);
        let running = tokio::spawn(async move { d2.send(framed).await });
        assert!(until(|| d.authority.running_count() == 1 && d.slow.counts() == (1, 0), Duration::from_secs(20)).await, "admitted, registered, and its backend running: {:?}", d.slow.counts());
        let asked = std::time::Instant::now();
        d.cancel(id);
        let reply = tokio::time::timeout(Duration::from_secs(15), running).await.expect("answered long before the backend's 30 s").unwrap();
        assert!(asked.elapsed() < Duration::from_secs(10), "stopped within the sweep's bound: {:?}", asked.elapsed());
        assert_refused(&reply, "cancelled");
        assert!(until(|| d.authority.running_count() == 0, Duration::from_secs(20)).await, "the registration is released");
        assert_eq!(d.slow.counts(), (1, 0), "the backend's future was dropped: it did not reach its end");
        assert_eq!(d.authority.named_stop_records().last().and_then(|s| s.term.clone()), Some(format!("c{id}")), "the stop record names the call's term");
        d.a.shutdown().await;
    }

    /// **A revoked `call:` term refuses `llm.invoke` under any rule.** The gateway cancels the call between the
    /// provider's preflight and its registration (held at the `PAUSE` hook): under the cancellable rule and under
    /// the rule that is not, the call is refused `call_mandate_not_established` with cause `revoked`, no backend
    /// is called, and the journal says nothing ran. Seen failing first: with the check taken out, both backends
    /// are called.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_revoked_call_term_refuses_llm_invoke_under_any_rule() {
        use crate::agent::provider_enforcement::test_hooks::PAUSE;
        let d = Arc::new(door(Opts::default()).await);
        for name in ["slow", "plain"] {
            let (framed, id) = d.frame(name, Some(&d.resource(name)), Cm::Honest);
            let (reached, resume) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
            PAUSE.pin().insert(d.me(), (Arc::clone(&reached), Arc::clone(&resume)));
            let before = d.nothing_ran();
            let d2 = Arc::clone(&d);
            let sent = tokio::spawn(async move { d2.send(framed).await });
            tokio::time::timeout(Duration::from_secs(20), reached.notified()).await.expect("the call reached the window after its preflight");
            d.cancel(id);
            resume.notify_one();
            assert_not_established(&sent.await.unwrap(), "revoked");
            assert_eq!((d.slow.counts(), d.fast.counts(), d.authority.running_count()), ((0, 0), (0, 0), 0), "{name}: no backend was called, and nothing is left registered");
            assert_eq!(d.nothing_ran(), before + 1, "{name}: the journal says nothing ran");
        }
        d.a.shutdown().await;
    }

    /// **The C7 question through the gateway: one mandate, both doors.** [`CALLER`] is a gateway client
    /// (`agent-1`). **The plant:** its mandate — one possession proof — reaches the backend through
    /// `/gateway/llm/call`, where the gateway's preflight and then the provider's check each establish it, and
    /// through a call framed straight to the provider: the two doors derive one operation, resource and arguments.
    /// Without the mandate the gateway refuses. Then the operator revokes the term: `/gateway/llm/call` is
    /// refused at the gateway, the direct call at the provider, and the backend's count does not move. Seen
    /// failing first: with the `llm.invoke` arm taken out of `derive`, the gateway's permit is refused at the
    /// provider and the plant fails.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_revoked_mandate_runs_no_llm_backend_through_the_gateway_or_a_direct_call() {
        let d = door(Opts { freshness_ms: 120_000, token: true, ..Opts::default() }).await;
        d.offer_depot_checkpoint(1, &[]);
        let mandate = d.depot_mandate("guarded");
        let http = reqwest::Client::new();
        let through_the_gateway = |mandate: Option<&serde_json::Value>| {
            let (http, url) = (http.clone(), d.url("/gateway/llm/call"));
            let mut body = serde_json::json!({ "ns": "review", "name": "guarded", "input": "the text", "context": { "tone": "plain" } });
            if let Some(m) = mandate {
                body["_meta"] = serde_json::json!({ "mandate": m });
            }
            async move {
                let r = http.post(url).bearer_auth("s3cret").json(&body).send().await.unwrap();
                (r.status().as_u16(), r.json::<serde_json::Value>().await.unwrap())
            }
        };

        let (status, reply) = through_the_gateway(None).await;
        assert_eq!((status, reply["error"].as_str()), (403, Some("policy")), "{reply}");
        assert_eq!(d.fast.counts(), (0, 0));

        let (status, reply) = through_the_gateway(Some(&mandate)).await;
        assert_eq!((status, reply["output"].as_str()), (200, Some("ran: the text")), "plant: /gateway/llm/call reaches the backend: {reply}");
        let reply = d.direct("guarded", &mandate).await;
        assert_eq!(reply["output"].as_str(), Some("ran: the text"), "plant: a direct call reaches the backend: {reply}");
        assert_eq!(d.fast.counts(), (2, 2));

        d.offer_depot_checkpoint(2, &["t1"]);
        let (status, reply) = through_the_gateway(Some(&mandate)).await;
        assert_eq!((status, reply["error"].as_str()), (403, Some("policy")), "refused at the gateway: {reply}");
        assert_refused(&d.direct("guarded", &mandate).await, "authority_not_established");
        assert_eq!(d.fast.counts(), (2, 2), "no door ran the backend after the revocation");
        d.a.shutdown().await;
    }
}
