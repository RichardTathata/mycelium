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

/// A decision record the agent's sink kept is served by the hash its span carries, only under `audit:read`, with the
/// exact bytes that hash is over, so the holder verifies it by recomputing; it is said to be not chained.
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
    assert_eq!(body["source"], serde_json::json!({ "from": "sink", "label": "agent" }));
    assert_eq!(hash_of_answer(&body), span.record_sha256, "the bytes served are the bytes the span's hash is over");
    assert_eq!(body["verification"]["hash"], "matches");
    assert_eq!(body["verification"]["chain"]["state"], "not_chained");
    assert_eq!(body["record"]["trigger"], "round 1", "the record itself, not a neighbour");
    // The record carries what the span never does: its inputs are on the node, read here under the audit scope.
    assert_eq!(body["record"]["inputs"][0]["value"], "secret-input-1");
    // A drain (an embedder writing its trace) does not take a record out of the route's index.
    sink.drain();
    let (status, _) = n.get(&path, "auditor").await;
    assert_eq!(status, 200, "a drained record is still indexed");
    n.stop().await;
}

/// **The route never takes a sink's ring lock** (#692's review, finding 2): by-hash reads issued while a decision point
/// records 20,000 records make it drop none `Contended` — the counterpart of the collector's
/// `a_collector_never_makes_a_decision_point_drop_a_record`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn by_hash_reads_under_load_never_make_a_decision_point_drop_a_record() {
    let sink = Arc::new(DecisionSink::new(SinkConfig { max_records: 50_000, max_bytes: 256 << 20, ..SinkConfig::default() }));
    let s2 = Arc::clone(&sink);
    let n = Arc::new(node("load", move |a, _| a.with_decision_trace(s2)).await);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (n, stop) = (Arc::clone(&n), Arc::clone(&stop));
        tokio::spawn(async move {
            let mut reads = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = n.get(&format!("/gateway/records/decision/{}", "ab".repeat(32)), "auditor").await;
                reads += 1;
            }
            reads
        })
    };
    let writer = {
        let sink = Arc::clone(&sink);
        tokio::task::spawn_blocking(move || {
            for i in 0..20_000u32 {
                let _ = sink.record(decision(i));
                if i % 64 == 0 {
                    std::thread::yield_now();
                }
            }
        })
    };
    writer.await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let reads = reader.await.unwrap();
    assert!(reads > 0, "the route was read while the decisions were made");
    assert_eq!(sink.drop_counts().contended, 0, "{reads} reads made a decision point drop its record");
    assert_eq!(sink.last_seq(), 20_000);
    Arc::try_unwrap(n).ok().expect("the reader is done").stop().await;
}

/// The index keeps the **newest** records (#692's confirmation round, finding A): 5000 decisions with no read between
/// them, then the 5000th — the span a collector just exported — is served; the oldest are the ones let go, and counted.
#[tokio::test]
async fn the_index_keeps_the_newest_records_when_no_one_reads_between_them() {
    let sink = Arc::new(DecisionSink::new(SinkConfig { max_records: 10_000, max_bytes: 64 << 20, ..SinkConfig::default() }));
    let s2 = Arc::clone(&sink);
    let n = node("newest", move |a, _| a.with_decision_trace(s2)).await;
    for i in 0..5000 {
        sink.record(decision(i)).unwrap();
    }
    let newest = RecordSpan::from_decision(sink.snapshot().last().unwrap());
    let (status, body) = n.get(&format!("/gateway/records/decision/{}", newest.record_sha256), "auditor").await;
    assert_eq!(status, 200, "the newest record is the one kept — {body}");
    let oldest = RecordSpan::from_decision(&sink.snapshot()[0]);
    let (status, body) = n.get(&format!("/gateway/records/decision/{}", oldest.record_sha256), "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert!(body["cannot_rule_out"].as_array().unwrap().iter().any(|m| m == "evicted"), "{body}");
    n.stop().await;
}

/// A node whose embedder declares only a trace: a record drained to it is served from it; a hash it does not hold is
/// `undetermined` with `not_written` and `evicted` possible, since the sink that wrote the trace is not one the route
/// reads; and a declared file not written yet is not an empty trace (#692's review, finding 1).
#[tokio::test]
async fn a_trace_only_node_serves_from_the_trace_and_never_says_never_existed() {
    let sink = DecisionSink::default();
    let declared = node("trace", |a, dir| a.with_decision_trace_file(dir.join("decisions.jsonl"))).await;
    let absent = format!("/gateway/records/decision/{}", "ab".repeat(32));

    // Not written yet: the file is missing.
    let (status, body) = declared.get(&absent, "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["not_found"], "undetermined");
    assert_eq!(body["searched"]["trace"]["exists"], false, "{body}");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "evicted", "not_searched", "never_existed"]));
    assert!(body["unsearched"].as_str().unwrap().contains("with_decision_record_source"), "{body}");

    for i in 0..3 {
        sink.record(decision(i)).unwrap();
    }
    let jsonl = format!("not a record\n{}", sink.to_jsonl());
    let span = RecordSpan::from_decision(&sink.drain()[0]);
    std::fs::write(declared.dir.join("decisions.jsonl"), jsonl).unwrap();
    let (status, body) = declared.get(&format!("/gateway/records/decision/{}", span.record_sha256), "auditor").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source"]["from"], "trace");
    assert_eq!(hash_of_answer(&body), span.record_sha256);

    let (_, body) = declared.get(&absent, "auditor").await;
    assert_eq!(body["searched"]["trace"]["exists"], true);
    assert_eq!(body["searched"]["trace"]["records"], 3, "a line that is not a record is not counted as one — {body}");
    assert_eq!(body["searched"]["trace"]["unparsed"], 1);
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "evicted", "not_searched", "never_existed"]));
    declared.stop().await;
}

/// A second sink — a stem provisioner's, an activation hook's — is not read until it is registered: its record is not
/// found, and the answer names it as possibly in a sink not searched. Registered, its later records are served, and the
/// ones it kept before are counted `missed`, so `evicted` is possible.
#[tokio::test]
async fn a_second_sink_is_searched_only_once_registered_and_what_it_kept_before_is_missed() {
    let agent_sink = Arc::new(DecisionSink::default());
    let other = Arc::new(DecisionSink::default());
    let s2 = Arc::clone(&agent_sink);
    let n = node("second", move |a, _| a.with_decision_trace(s2)).await;
    other.record(decision(0)).unwrap();
    let before = RecordSpan::from_decision(&other.snapshot()[0]);
    let path = format!("/gateway/records/decision/{}", before.record_sha256);
    let (status, body) = n.get(&path, "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_ne!(body["not_found"], "never_existed", "{body}");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_searched", "never_existed"]), "{body}");

    n.agent.with_decision_record_source("provisioner", Arc::clone(&other));
    other.record(decision(1)).unwrap();
    let after = RecordSpan::from_decision(&other.snapshot()[1]);
    let (status, body) = n.get(&format!("/gateway/records/decision/{}", after.record_sha256), "auditor").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source"], serde_json::json!({ "from": "sink", "label": "provisioner" }));
    let (_, body) = n.get(&path, "auditor").await;
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["evicted", "not_searched", "never_existed"]), "{body}");
    let provisioner = body["searched"]["index"]["sources"].as_array().unwrap().iter().find(|s| s["label"] == "provisioner").cloned().unwrap();
    assert_eq!(provisioner["missed"], 1, "{body}");
    // Labels are unique (#692's confirmation round, D): a second source under one is refused, and not read.
    let third = Arc::new(DecisionSink::default());
    n.agent.with_decision_record_source("provisioner", Arc::clone(&third));
    third.record(decision(9)).unwrap();
    let hidden = RecordSpan::from_decision(&third.snapshot()[0]);
    let (status, body) = n.get(&format!("/gateway/records/decision/{}", hidden.record_sha256), "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["searched"]["index"]["sources"].as_array().unwrap().len(), 2, "{body}");
    assert!(third.tap_ring(1, 1).is_ok(), "the refused registration left the sink's slot free");
    n.stop().await;
}

/// *Not found* for a decision is always `undetermined` and names what the counters cannot rule out: from a clean
/// registered sink only `not_searched` and `never_existed`; `not_written` once it has dropped a record; and everything
/// from a node with no decision source at all.
#[tokio::test]
async fn not_found_names_what_the_counters_cannot_rule_out() {
    let absent = "ab".repeat(32);
    let path = format!("/gateway/records/decision/{absent}");

    let sink = Arc::new(DecisionSink::new(SinkConfig { max_records: 2, ..SinkConfig::default() }));
    let s2 = Arc::clone(&sink);
    let n = node("nf", move |a, _| a.with_decision_trace(s2)).await;
    sink.record(decision(0)).unwrap();
    let (status, body) = n.get(&path, "auditor").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["not_found"], "undetermined", "{body}");
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_searched", "never_existed"]), "{body}");

    sink.record(decision(1)).unwrap();
    assert!(sink.record(decision(2)).is_err(), "the third record is dropped: the sink holds two");
    let (_, body) = n.get(&path, "auditor").await;
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "not_searched", "never_existed"]), "{body}");
    assert_eq!(body["searched"]["index"]["sources"][0]["dropped"]["full"], 1);
    n.stop().await;

    // A declared trace file that does not exist is not an empty trace: it may not have been written yet.
    let sink = Arc::new(DecisionSink::default());
    let s3 = Arc::clone(&sink);
    let unwritten = node("unwritten", move |a, dir| {
        a.with_decision_trace(s3);
        a.with_decision_trace_file(dir.join("decisions.jsonl"));
    })
    .await;
    let (_, body) = unwritten.get(&path, "auditor").await;
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "not_searched", "never_existed"]), "{body}");
    unwritten.stop().await;

    let bare = node("bare", |_, _| {}).await;
    let (status, body) = bare.get(&path, "auditor").await;
    assert_eq!(status, 404);
    assert_eq!(body["cannot_rule_out"], serde_json::json!(["not_written", "evicted", "not_searched", "never_existed"]), "{body}");
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
/// verified segment — preferred over a citing record that does not verify, sealed before it (#692's review, finding 3).
#[tokio::test]
async fn an_evidence_record_is_served_from_the_journal_and_verified_against_the_chain() {
    let n = node("ev", |a, dir| {
        a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    })
    .await;
    let tc = Arc::clone(&n.agent.task_ctx);
    super::authz::ae_record(&tc, &evidence("op-1")).await.unwrap();
    // A record citing op-2's hash, planted at the head of this node's stream and signed by a key that is not the
    // node's: it does not verify, and the genuine reference is sealed past it, at a higher seq.
    {
        use crate::{audit_key, AuditAction, AuditOutcome, AuditRecord, SignedAuditRecord};
        let h = crate::causal::content_hash(&serde_json::to_vec(&evidence("op-2")).unwrap());
        let detail = serde_json::json!({ "schema": super::action_evaluator::AE_REFERENCE_SCHEMA, "journal_sha256": h });
        let seq = tc.audit_chain.lock().unwrap().next_seq;
        let forged = AuditRecord {
            node_id: tc.node_id.clone(), seq, hlc: 1, principal: "attacker".into(), action: AuditAction::Invoke,
            target: "ae:cat".into(), outcome: AuditOutcome::Success, detail: Some(detail.to_string()), prev_hash: [9u8; 32],
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        assert!(n.agent.kv().set(audit_key(&tc.node_id, seq), SignedAuditRecord::sign(forged, &key).encode()));
    }
    super::authz::ae_record(&tc, &evidence("op-2")).await.unwrap();
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
    assert_eq!(body["source"]["from"], "journal");
    assert_eq!(hash_of_answer(&body), span.record_sha256);
    assert_eq!(body["record"]["operation_id"], "op-2", "the record itself, not its neighbour");
    assert_eq!(body["verification"]["hash"], "matches");
    let chain = &body["verification"]["chain"];
    assert_eq!(chain["state"], "referenced", "{body}");
    assert_eq!(chain["linked"], true, "the verifying reference, not the planted one sealed before it — {body}");
    assert_eq!(chain["citing_records"], 2, "{body}");
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
    assert!(body["unsearched"].as_str().unwrap().contains("earlier open"), "the answer is scoped to this open — {body}");
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
