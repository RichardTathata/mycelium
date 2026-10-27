//! Adopter-surface Y4's last promise — *a holder asks the node for the record by hash, under the audit scope, and
//! verifies it there*: `GET /gateway/records/decision/{sha256}` and `GET /gateway/records/evidence/{sha256}`, each
//! asked for by the hash a span carries ([`RecordSpan`]), each answering what *not found* means.

use crate::agent::action_evaluator::{ActionEnvelope, ActionMapping, AeEvidence, Decision, Execution};
use crate::agent::evidence_journal::{EvidenceJournal, EvidenceProfile};
use crate::config::TlsConfig;
use crate::decision::{DecisionRecord, DecisionSink, OutcomeKind, SinkConfig};
use crate::span_export::RecordSpan;
use crate::test_util::alloc_port;
use crate::{GatewayToken, GossipAgent, GossipConfig, NodeId};
use axum::http::header::AUTHORIZATION;
use base64::Engine as _;
use std::sync::Arc;

struct Node {
    agent: Arc<GossipAgent>,
    base: String,
    dir: std::path::PathBuf,
}

impl Node {
    async fn get(&self, path: &str, token: &str) -> (u16, serde_json::Value) {
        let r = reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(serde_json::Value::Null))
    }

    async fn stop(self) {
        self.agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A node with an auditor token and a token without the scope; `prepare` attaches what the case needs before start.
async fn node(tag: &str, prepare: impl FnOnce(&GossipAgent, &std::path::Path)) -> Node {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let dir = std::env::temp_dir().join(format!("y4-records-{tag}-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("certs"), ..TlsConfig::default() });
    cfg.gateway_scoped_tokens = vec![
        GatewayToken { token: "auditor".into(), scopes: vec!["audit:read".into()] },
        GatewayToken { token: "reader".into(), scopes: vec!["kv:read".into(), "fleet:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id, cfg));
    prepare(&agent, &dir);
    agent.start().await.unwrap();
    Node { agent, base: format!("http://127.0.0.1:{http_port}"), dir }
}

fn decision(n: u32) -> DecisionRecord {
    DecisionRecord::new("membership.governed", 2, format!("round {n}"), OutcomeKind::Deferral, "declined")
        .node("n1", 1)
        .at(1_000 + u64::from(n))
        .correlation("run-1")
        .input(crate::decision::InputSnapshot::new("peers", format!("secret-input-{n}"), crate::decision::Provenance::Local))
}

/// SHA-256 over the bytes the answer carries — what a holder recomputes, never what the node says.
fn hash_of_answer(body: &serde_json::Value) -> String {
    let bytes = base64::engine::general_purpose::STANDARD.decode(body["record_b64"].as_str().unwrap()).unwrap();
    crate::causal::content_hash(&bytes)
}

/// A decision record the sink holds is served by the hash its span carries, only under `audit:read`, with the exact
/// bytes that hash is over, so the holder verifies it by recomputing; it is said to be not chained.
#[tokio::test]
async fn a_decision_record_is_served_by_the_hash_its_span_carries_under_the_audit_scope() {
    let sink = Arc::new(DecisionSink::default());
    let s2 = Arc::clone(&sink);
    let n = node("dec", move |a, _| a.with_decision_trace(s2)).await;
    for i in 0..3 {
        sink.record(decision(i)).unwrap();
    }
    let held = sink.snapshot();
    let span = RecordSpan::from_decision(&held[1]);
    let path = format!("/gateway/records/decision/{}", span.record_sha256);

    let (status, _) = n.get(&path, "reader").await;
    assert_eq!(status, 403, "a token without audit:read is refused");

    let (status, body) = n.get(&path, "auditor").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["schema"], "mycelium.record/1");
    assert_eq!(body["found"], true);
    assert_eq!(body["source"], "sink");
    assert_eq!(hash_of_answer(&body), span.record_sha256, "the bytes served are the bytes the span's hash is over");
    assert_eq!(body["verification"]["hash"], "matches");
    assert_eq!(body["verification"]["chain"]["state"], "not_chained");
    assert_eq!(body["record"]["trigger"], "round 1", "the record itself, not a neighbour");
    // The record carries what the span never does: its inputs are on the node, read here under the audit scope.
    assert_eq!(body["record"]["inputs"][0]["value"], "secret-input-1");
    n.stop().await;
}

/// A record drained out of the sink is served from the trace the embedder declared it writes; without the declaration
/// the same read cannot rule out that it was evicted, and says so rather than *never existed*.
#[tokio::test]
async fn a_drained_record_is_served_from_the_declared_trace_and_otherwise_is_said_possibly_evicted() {
    let sink = Arc::new(DecisionSink::default());
    let (s2, s3) = (Arc::clone(&sink), Arc::clone(&sink));
    let declared = node("trace", move |a, dir| {
        a.with_decision_trace(s2);
        a.with_decision_trace_file(dir.join("decisions.jsonl"));
    })
    .await;
    for i in 0..3 {
        sink.record(decision(i)).unwrap();
    }
    let jsonl = sink.to_jsonl();
    let span = RecordSpan::from_decision(&sink.drain()[0]);
    std::fs::write(declared.dir.join("decisions.jsonl"), jsonl).unwrap();
    let path = format!("/gateway/records/decision/{}", span.record_sha256);
    let (status, body) = declared.get(&path, "auditor").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source"], "trace");
    assert_eq!(hash_of_answer(&body), span.record_sha256);
    declared.stop().await;

    // The same drain on a node that declares no trace.
    let undeclared = node("evicted", move |a, _| a.with_decision_trace(s3)).await;
    let (status, body) = undeclared.get(&path, "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["found"], false);
    assert_eq!(body["not_found"], "undetermined");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["evicted", "never_existed"]), "{body}");
    assert_eq!(body["searched"]["sink"]["not_searched"], 3);
    undeclared.stop().await;
}

/// *Not found* says the one meaning it can, or every meaning it cannot rule out: `never_existed` alone from a sink that
/// holds all it kept and dropped nothing; `not_written` among the possibilities once the sink has dropped a record;
/// `not_written` alone where the node keeps no trace at all.
#[tokio::test]
async fn not_found_names_never_existed_not_written_or_what_it_cannot_rule_out() {
    let absent = "ab".repeat(32);
    let path = format!("/gateway/records/decision/{absent}");

    let sink = Arc::new(DecisionSink::new(SinkConfig { max_records: 2, ..SinkConfig::default() }));
    let s2 = Arc::clone(&sink);
    let n = node("nf", move |a, _| a.with_decision_trace(s2)).await;
    sink.record(decision(0)).unwrap();
    let (status, body) = n.get(&path, "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["not_found"], "never_existed", "{body}");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["never_existed"]));

    sink.record(decision(1)).unwrap();
    assert!(sink.record(decision(2)).is_err(), "the third record is dropped: the sink holds two");
    let (_, body) = n.get(&path, "auditor").await;
    assert_eq!(body["not_found"], "undetermined", "{body}");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "never_existed"]), "{body}");
    assert_eq!(body["searched"]["sink"]["dropped"]["full"], 1);
    n.stop().await;

    let bare = node("bare", |_, _| {}).await;
    let (status, body) = bare.get(&path, "auditor").await;
    assert_eq!(status, 404);
    assert_eq!(body["not_found"], "not_written", "{body}");
    let (status, body) = bare.get(&format!("/gateway/records/evidence/{absent}"), "auditor").await;
    assert_eq!(status, 404);
    assert_eq!(body["not_found"], "not_written", "no journal: this node writes no evidence — {body}");
    bare.stop().await;
}

/// Only a full lowercase hash is a record's name: a prefix, uppercase hex or a non-hex string is refused 400 at both
/// routes, before any source is read.
#[tokio::test]
async fn a_record_is_asked_for_by_its_full_lowercase_hash_only() {
    let n = node("bad", |_, _| {}).await;
    for bad in ["abcd1234", &"AB".repeat(32), &"zz".repeat(32), &"ab".repeat(33)] {
        for kind in ["decision", "evidence"] {
            let (status, body) = n.get(&format!("/gateway/records/{kind}/{bad}"), "auditor").await;
            assert_eq!(status, 400, "{kind}/{bad}: {body}");
            assert_eq!(body["error"], "bad_hash");
        }
    }
    n.stop().await;
}

fn evidence(op: &str) -> AeEvidence {
    let via = NodeId::new("127.0.0.1", 7101).unwrap();
    let env = ActionEnvelope::builder("oidc:idp/alice", via, "tools/call", "tool:post@n1")
        .identities(op, format!("{op}/1"))
        .mapping(ActionMapping::mapped("cat", "1"))
        .build();
    AeEvidence::for_decision(&env, &Decision::permit("ok", "rev-1"), Execution::Attempted, "gateway:mcp/tools/call")
}

/// An evidence record is served from the journal by the hash its span carries, and verified here two ways: the bytes
/// hash to it, and this node's signed, hash-linked audit stream holds the reference record that cites it, in its
/// verified segment.
#[tokio::test]
async fn an_evidence_record_is_served_from_the_journal_and_verified_against_the_chain() {
    let n = node("ev", |a, dir| {
        a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    })
    .await;
    let tc = Arc::clone(&n.agent.task_ctx);
    for op in ["op-1", "op-2"] {
        super::authz::ae_record(&tc, &evidence(op)).await.unwrap();
    }
    let frames = crate::read_evidence_journal(&n.dir.join("evidence.log")).unwrap();
    let entry = crate::agent::journal::JournalEntry {
        seq: 1,
        content_hash: {
            let h = crate::causal::content_hash(&frames[1]);
            let mut out = [0u8; 32];
            for (i, b) in out.iter_mut().enumerate() {
                *b = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap();
            }
            out
        },
        bytes: frames[1].clone(),
    };
    let span = RecordSpan::from_evidence(&entry).unwrap();
    let (status, _) = n.get(&format!("/gateway/records/evidence/{}", span.record_sha256), "reader").await;
    assert_eq!(status, 403, "a token without audit:read is refused");
    let (status, body) = n.get(&format!("/gateway/records/evidence/{}", span.record_sha256), "auditor").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source"], "journal");
    assert_eq!(hash_of_answer(&body), span.record_sha256);
    assert_eq!(body["record"]["operation_id"], "op-2", "the record itself, not its neighbour");
    assert_eq!(body["verification"]["hash"], "matches");
    let chain = &body["verification"]["chain"];
    assert_eq!(chain["state"], "referenced", "{body}");
    assert_eq!(chain["linked"], true, "{body}");
    // The reference the answer names is in the stream `/gateway/audit` serves, under the same content hash.
    let (_, audit) = n.get("/gateway/audit", "auditor").await;
    let cited = chain["reference_content_hash"].as_str().unwrap();
    let streams = audit["streams"].as_array().unwrap();
    assert!(
        streams.iter().flat_map(|s| s["records"].as_array().unwrap()).any(|r| r["content_hash"] == cited),
        "{audit}"
    );

    let (status, body) = n.get(&format!("/gateway/records/evidence/{}", "cd".repeat(32)), "auditor").await;
    assert_eq!(status, 404);
    assert_eq!(body["not_found"], "never_existed", "the journal loses nothing it acknowledged — {body}");
    assert_eq!(body["searched"]["journal"]["records"], 2);
    n.stop().await;
}

/// A journal that truncated records when it was opened cannot say a missing record never existed: it may have been one
/// of those cut away.
#[tokio::test]
async fn a_journal_repaired_at_open_cannot_rule_out_an_evicted_record() {
    let gossip_dir_tag = "torn";
    let n = node(gossip_dir_tag, |a, dir| {
        let path = dir.join("evidence.log");
        // One whole frame, then a torn one: a length prefix promising more bytes than follow.
        let body = serde_json::to_vec(&evidence("op-1")).unwrap();
        let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&body);
        bytes.extend_from_slice(&64u32.to_le_bytes());
        bytes.extend_from_slice(b"{\"torn\":");
        std::fs::write(&path, bytes).unwrap();
        a.with_evidence_journal(EvidenceJournal::open(&path, EvidenceProfile::Strict).unwrap());
    })
    .await;
    let (status, body) = n.get(&format!("/gateway/records/evidence/{}", "ef".repeat(32)), "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["searched"]["journal"]["truncated_at_open"], true, "{body}");
    assert_eq!(body["not_found"], "undetermined");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["evicted", "never_existed"]));
    n.stop().await;
}
