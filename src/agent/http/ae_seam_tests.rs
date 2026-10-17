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

/// **Policy over history, end to end** (post-360 row K): *at most three posts in ten minutes*, counted in the
/// actor's history at this enforcement point.
///
/// The fourth post is refused, and its evidence record carries the history it was decided on — the three
/// attempts, the scope, the window. Ten concurrent calls under the same limit admit exactly three: the check and
/// the count are one step, so concurrent calls cannot all slip under it. And the history is the journal's, so a
/// restarted node still refuses.
#[tokio::test]
async fn at_most_three_posts_in_ten_minutes_counted_at_this_gateway() {
    use super::super::evidence_journal::{EvidenceJournal, EvidenceProfile};
    use crate::{AeEvidence, DecisionKind, HistoryScope, RecordKind};

    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("ae-history-{http_port}"));
    let journal_path = std::env::temp_dir().join(format!("ae-history-journal-{http_port}")).join("evidence.log");
    let _ = std::fs::remove_dir_all(&cert_dir);
    let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());

    let start = || async {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
        let me = agent.node_id().clone();
        agent.with_evidence_journal(EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).unwrap());
        let mut policy = ReferenceEvaluator::new("rev-k").with_catalogue("cat-k", "1");
        for tool in ["post", "burst"] {
            policy = policy
                .map_action("tools/call", format!("tool:{tool}@{me}"))
                .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", format!("tool:{tool}@{me}")).at_most(3, 600_000));
        }
        agent.with_action_evaluator(Arc::new(policy));
        agent.start().await.unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for tool in ["post", "burst"] {
            let runs = Arc::clone(&runs);
            handles.push(agent.mcp().register_mcp_tool(tool, serde_json::json!({"type": "object", "properties": {}}), move |_args| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!("ran"))
                }
            }));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        (agent, runs, handles)
    };

    let (agent, runs, handles) = start().await;
    for i in 0..3 {
        let r = tools_call(http_port, "post", serde_json::json!({})).await;
        assert!(r.get("error").is_none(), "post {i} is within the limit: {r}");
    }
    let fourth = tools_call(http_port, "post", serde_json::json!({})).await;
    assert_eq!(fourth["error"]["code"], -32030, "the fourth post in ten minutes is denied: {fourth}");
    assert_eq!(runs.load(Ordering::SeqCst), 3, "the denied post never ran");

    let journalled: Vec<AeEvidence> = super::super::evidence_journal::read_evidence_journal(&journal_path)
        .unwrap()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    let decided: Vec<&AeEvidence> = journalled.iter().filter(|e| e.kind == RecordKind::Decided).collect();
    assert_eq!(decided.len(), 4);
    let refused = decided[3];
    assert_eq!(refused.decision, DecisionKind::Deny);
    let read = refused.history.as_ref().expect("the refusal carries the history it was decided on");
    assert_eq!(read.scope, Some(HistoryScope::EnforcementPoint));
    assert_eq!(read.window_ms, 600_000);
    assert_eq!(read.attempts, decided[..3].iter().map(|e| e.attempt_id.clone()).collect::<Vec<_>>(),
               "the three earlier attempts, by the ids a replay finds them under");
    assert!(read.complete);
    assert!(refused.checked.iter().any(|c| c.contains("history scope=enforcement_point") && c.contains("counted=3")));
    assert!(journalled.iter().filter(|e| e.kind == RecordKind::Execution).all(|e| e.history.is_none()));

    // Concurrent: ten calls under a limit of three admit three.
    let before = runs.load(Ordering::SeqCst);
    let burst: Vec<_> = (0..10).map(|_| tokio::spawn(tools_call(http_port, "burst", serde_json::json!({})))).collect();
    let mut admitted = 0;
    for call in burst {
        let r = call.await.unwrap();
        if r.get("error").is_none() {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 3, "the check and the count are one step");
    assert_eq!(runs.load(Ordering::SeqCst) - before, 3);

    drop(handles);
    agent.shutdown().await;
    drop(agent);

    // Restarted on the same journal: the history is still there.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (agent, runs, handles) = start().await;
    let after_restart = tools_call(http_port, "post", serde_json::json!({})).await;
    assert_eq!(after_restart["error"]["code"], -32030, "the journal's history survives a restart: {after_restart}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    drop(handles);
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());
}
