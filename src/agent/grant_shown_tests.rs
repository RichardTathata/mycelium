//! Identity-and-lifecycle I7-S, PR 1: **a grant decision names what its decider saw** — the request's tool, resource,
//! arguments digest and expiry, signed after the grade (or, for a bearer's unsigned decision, sent beside it) — and the
//! node it is addressed to compares them with its own copy of the request record. A child of `client_grant_tests`, for
//! its fleet and decision helpers.
//!
//! The record is unsigned in the KV (`owner-signed-records.md`, row `grant/{agent}/{request}`): any admitted member can
//! rewrite it. The forged-record tests here have **a second member** rewrite it over gossip after the decider read it.

use super::*;
use crate::agent::action_evaluator::RecordKind;

/// A change to a request record, as a test's member makes it.
type Rewrite = Box<dyn Fn(&mut serde_json::Value)>;

/// The forecaster's request record `request`, as `a` holds it.
fn record_at(a: &GossipAgent, request: &str) -> serde_json::Value {
    let key = format!("grant/{}/{request}", crate::agent::gateway_authority::grant_holder_segment(FORECASTER));
    mycelium_core::ops::kv_get(&a.task_ctx, &key).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn record_key(request: &str) -> String {
    format!("grant/{}/{request}", crate::agent::gateway_authority::grant_holder_segment(FORECASTER))
}

/// **A record rewritten by another member after the decider read it is not the one decided** (I7-S, D1; #813's review,
/// F1, reproduced). Two members: A, the gateway the decisions are addressed to, and B. The forecaster asks A for five
/// grants; the treasurer signs an issue of each naming the record as A served it, and — for a sixth — a bearer issues
/// naming it beside an unsigned body (D4). Then **B** rewrites each record over gossip, one field each, keeping the
/// record self-consistent:
/// - F1's rewrite: a weather forecast becomes a payment, its tool, resource and digest together (`tool` named first);
/// - the resource alone: the provider moved to B (`node` and `resource` together);
/// - the digest alone: another call's arguments;
/// - the expiry alone: the request kept open longer;
/// - the bearer's request: the digest.
///
/// Each decision is refused `409 decision_names_another_request` naming the field; nothing is issued; the evaluator is
/// never asked (no `grant.issue` record names the treasurer or the bearer); `/stats` counts five. The untouched
/// request's decision is admitted. Seen failing first: before the change every rewritten decision was answered `200`
/// and a grant was issued for the rewritten record (the plant: the comparison in `decide_grant` removed).
#[tokio::test]
async fn a_record_another_member_rewrote_after_the_decider_read_it_is_refused_naming_the_field() {
    let dir_a = scratch("shown-a");
    let dir_b = scratch("shown-b");
    let certs = dir_a.join("tls");
    let http_a = alloc_port();
    let (a, _) = started_as(&dir_a, http_a, &Node { certs: certs.clone(), bootstrap: vec![], authority: true, port: None }, true).await;
    let (b, _) = started_as(&dir_b, alloc_port(), &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: false, port: None }, false).await;
    let _ = forecaster_key_in(&a, &certs, &dir_a.join("client"));
    let treasurer = client_key_in(&a, &certs, &dir_a.join("client-treasurer"), "treasurer");
    let mut requests = Vec::new();
    for i in 0..6 {
        let (status, asked) = http(http_a, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {"city": "Lyon", "n": i}}))).await;
        assert_eq!(status, 200, "{asked}");
        requests.push(asked["request"].as_str().unwrap().to_string());
    }
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    // Signed (and, for the last, named beside a bearer's body) from the records as A serves them, before any rewrite.
    let pending: Vec<serde_json::Value> = http(http_a, "relay-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await.1["pending"].as_array().cloned().unwrap_or_default();
    let served = |request: &str| crate::Shown::from_record(pending.iter().find(|r| r["request"].as_str() == Some(request)).expect("served")).expect("a block");
    let bodies: Vec<serde_json::Value> = requests[..5]
        .iter()
        .map(|r| serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, r, "issue", Some(600), None, now, &served(r))}))
        .collect();
    let bearer_body = serde_json::json!({"decision": "issue", "ttl_secs": 60, "shown": served(&requests[5])});

    // B rewrites four records and the bearer's, over gossip.
    let other_digest = crate::agent::action_evaluator::hex32(&crate::arguments_digest(&serde_json::json!({"to": "an account of the member's", "amount": 10_000})));
    let b_node = b.node_id().to_string();
    let rewrites: Vec<(usize, &str, Rewrite)> = vec![
        (0, "tool", Box::new({
            let d = other_digest.clone();
            let n = node.clone();
            move |r: &mut serde_json::Value| {
                r["tool"] = "payments.transfer".into();
                r["resource"] = format!("tool:payments.transfer@{n}").into();
                r["arguments_digest"] = d.clone().into();
            }
        })),
        (1, "resource", Box::new(move |r: &mut serde_json::Value| {
            r["node"] = b_node.clone().into();
            r["resource"] = format!("tool:{TOOL}@{b_node}").into();
        })),
        (2, "arguments_digest", Box::new({
            let d = other_digest.clone();
            move |r: &mut serde_json::Value| r["arguments_digest"] = d.clone().into()
        })),
        (3, "request_expires_at_ms", Box::new(|r: &mut serde_json::Value| r["expires_at_ms"] = (r["expires_at_ms"].as_u64().unwrap() + 3_600_000).into())),
        (5, "arguments_digest", Box::new({
            let d = other_digest.clone();
            move |r: &mut serde_json::Value| r["arguments_digest"] = d.clone().into()
        })),
    ];
    let mut rewritten = Vec::new();
    for (i, _, rewrite) in &rewrites {
        let mut rec = record_at(&a, &requests[*i]);
        rewrite(&mut rec);
        assert!(b.kv().set(record_key(&requests[*i]), bytes::Bytes::from(serde_json::to_vec(&rec).unwrap())), "B writes the record");
        rewritten.push((requests[*i].clone(), rec));
    }
    let arrived = until(|| rewritten.iter().all(|(r, rec)| &record_at(&a, r) == rec), Duration::from_secs(15)).await;

    let mut answers = Vec::new();
    for (i, field, _) in &rewrites {
        let answer = if *i == 5 { http_raw(http_a, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(&requests[5])), Some(bearer_body.clone())).await } else { post_decision(http_a, "relay-secret", &requests[*i], bodies[*i].clone()).await };
        answers.push((*field, answer));
    }
    let untouched = post_decision(http_a, "relay-secret", &requests[4], bodies[4].clone()).await;
    let issued: Vec<bool> = requests.iter().map(|r| mycelium_core::ops::kv_get(&a.task_ctx, &format!("{}/issued", record_key(r))).is_some()).collect();
    let (_, stats) = http(http_a, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    b.shutdown().await;
    a.shutdown().await;
    let journal = journal_of(&dir_a);
    for d in [&dir_a, &dir_b] {
        let _ = std::fs::remove_dir_all(d);
    }

    assert!(arrived, "B's rewrites reached A over gossip");
    for (field, answer) in &answers {
        assert_eq!(code(answer), (409, Some("decision_names_another_request".into())), "MOVED: a decision applied to a rewritten {field}: {}", answer.1);
        assert!(answer.1["detail"].as_str().unwrap_or_default().starts_with(&format!("the decision names {field} ")), "FIELD: {field}: {}", answer.1);
    }
    assert_eq!(untouched.0, 200, "the control: {}", untouched.1);
    assert_eq!(issued, [false, false, false, false, true, false], "ISSUED: only the untouched request");
    assert_eq!(stats["grant_decisions_naming_another"].as_u64(), Some(5), "COUNTED: {stats}");
    let asked = journal.iter().filter(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue").count();
    assert_eq!(asked, 1, "EVALUATOR: asked only about the untouched request");
}

/// **The record is checked against itself, for every decision** (I7-S; the owner-signed rule's body-against-path
/// check). Four records written under the forecaster's request keys, each disagreeing with itself in one way — its
/// `request` not the path's, its `agent` not the path's holder, its `resource` not `tool:{tool}@{node}`, its `node`
/// absent — are refused `409 unreadable_request` naming the field, **signed or not**: a treasurer-signed issue and a
/// bearer's issue, each naming exactly the record it was sent for, so nothing but the self-check refuses them. The
/// evaluator is never asked about a resource the grant would not bind. Seen failing first: before the change each was
/// issued (the plant: the self-check in `decide_grant` removed).
#[tokio::test]
async fn a_record_that_disagrees_with_itself_is_refused_signed_or_not() {
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("self-check", 4).await;
    let node = a.node_id().to_string();
    let base = record_at(&a, &requests[0]);
    let flaws: [(&str, Rewrite); 4] = [
        ("request", Box::new(|r: &mut serde_json::Value| r["request"] = "another-request".into())),
        ("agent", Box::new(|r: &mut serde_json::Value| r["agent"] = "token:fleet/nobody".into())),
        ("resource", Box::new(|r: &mut serde_json::Value| r["resource"] = format!("tool:{TOOL}@127.0.0.1:1").into())),
        ("node", Box::new(|r: &mut serde_json::Value| {
            r.as_object_mut().unwrap().remove("node");
        })),
    ];
    let now = mycelium_core::sim_seam::wall_now_ms();
    let mut answers = Vec::new();
    for ((field, flaw), request) in flaws.iter().zip(&requests) {
        let mut rec = base.clone();
        rec["request"] = request.as_str().into();
        flaw(&mut rec);
        assert!(a.kv().set(record_key(request), bytes::Bytes::from(serde_json::to_vec(&rec).unwrap())));
        let shown = crate::Shown::from_record(&rec).expect("a block");
        let signed = serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, request, "issue", Some(600), None, now, &shown)});
        let by_signature = post_decision(http_port, "relay-secret", request, signed).await;
        let by_bearer = http_raw(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(request)), Some(serde_json::json!({"decision": "issue", "shown": shown}))).await;
        answers.push((*field, by_signature, by_bearer));
    }
    let issued = requests.iter().filter(|r| mycelium_core::ops::kv_get(&a.task_ctx, &format!("{}/issued", record_key(r))).is_some()).count();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    for (field, by_signature, by_bearer) in &answers {
        for (who, answer) in [("SIGNED", by_signature), ("BEARER", by_bearer)] {
            assert_eq!(code(answer), (409, Some("unreadable_request".into())), "{who}: a record whose {field} disagrees: {}", answer.1);
            assert!(answer.1["detail"].as_str().unwrap_or_default().contains(&format!("the record's {field} ")), "{who}: FIELD {field}: {}", answer.1);
        }
    }
    assert_eq!(issued, 0, "nothing issued");
    assert!(!journal.iter().any(|r| r.operation == "grant.issue"), "EVALUATOR: never asked about a record that disagrees with itself");
}

/// **A decision naming nothing of what its decider saw is refused by default; the one-release opt-out admits it,
/// counted** (I7-S, D2 and D4). At a gateway as configured by default:
/// - a service's signed issue with no block, and a bearer's unsigned issue with no `shown`, are refused `403
///   decision_names_nothing` — before any key is used, so nothing is recorded;
/// - a body naming a block without a grade is refused `400 decision_shown_ungraded`; a signed body with `shown` beside
///   `signed` `400 bad_request`; a revocation naming a block `400 decision_shown_on_revoke`;
/// - a relay that rewrites the signed block is refused `not_the_decider`: the block is inside the signature;
/// - the honest decision, naming its block, is admitted and its evidence record keeps the block, which re-verifies.
///
/// At a gateway setting `gateway_accept_unnamed_decisions`, the same two unnamed decisions are admitted and counted
/// (`grant_decisions_unnamed` = 2), and a named one is not counted. Seen failing first: before the change both unnamed
/// decisions were admitted at the default gateway (the plant: `shown_refusal`'s default arm returning `None`).
#[tokio::test]
async fn a_decision_naming_nothing_is_refused_by_default_and_admitted_counted_under_the_opt_out() {
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("unnamed", 5).await;
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let shown = |r: &str| asked_block(r).expect("asked here");
    let unnamed_signed = post_decision(http_port, "relay-secret", &requests[0], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision(&treasurer, DECIDER, &node, FORECASTER, &requests[0], "issue", Some(600), None, now)})).await;
    let unnamed_bearer = http_raw(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(&requests[0])), Some(serde_json::json!({"decision": "issue"}))).await;
    let mut ungraded = crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, &requests[0], "issue", Some(600), None, now, &shown(&requests[0]));
    ungraded.grade = None;
    let ungraded = post_decision(http_port, "relay-secret", &requests[0], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": ungraded})).await;
    let honest = crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, &requests[1], "issue", Some(600), None, now, &shown(&requests[1]));
    let beside = post_decision(http_port, "relay-secret", &requests[1], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": honest, "shown": shown(&requests[1])})).await;
    let mut relayed = honest.clone();
    relayed.shown.as_mut().unwrap().arguments_digest = "00".repeat(32);
    let altered = post_decision(http_port, "relay-secret", &requests[1], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": relayed})).await;
    let admitted = post_decision(http_port, "relay-secret", &requests[1], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": honest})).await;
    let on_revoke = crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, &requests[1], "revoke", None, None, now, &shown(&requests[1]));
    let on_revoke = post_revoke(http_port, "relay-secret", &requests[1], serde_json::json!({"signed": on_revoke})).await;
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(code(&unnamed_signed), (403, Some("decision_names_nothing".into())), "SERVICE: {}", unnamed_signed.1);
    assert_eq!(code(&unnamed_bearer), (403, Some("decision_names_nothing".into())), "BEARER: {}", unnamed_bearer.1);
    assert_eq!(code(&ungraded), (400, Some("decision_shown_ungraded".into())), "UNGRADED: {}", ungraded.1);
    assert_eq!(code(&beside), (400, Some("bad_request".into())), "BESIDE: {}", beside.1);
    assert_eq!(code(&altered), (403, Some("not_the_decider".into())), "RELAY: a rewritten block still verified: {}", altered.1);
    assert_eq!(admitted.0, 200, "HONEST: {}", admitted.1);
    assert_eq!(code(&on_revoke), (400, Some("decision_shown_on_revoke".into())), "REVOKE: {}", on_revoke.1);
    assert_eq!(stats["grant_decisions_unnamed"].as_u64(), Some(0), "nothing admitted unnamed here: {stats}");
    let kept = journal.iter().find(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue").and_then(|r| r.signed_decision.clone()).expect("the honest issue's evidence");
    let body: crate::SignedDecision = serde_json::from_value(kept["signed"].clone()).unwrap();
    assert_eq!(body.shown.as_ref(), Some(&shown(&requests[1])), "EVIDENCE: the block is kept: {kept}");
    assert!(body.verify_signature("issue", Some(600), None), "RE-VERIFY: with the block, from the record alone");
    assert_eq!(journal.iter().filter(|r| r.kind == RecordKind::Decided).count(), 1, "the refused bodies were decided by no one");

    // The opt-out, for one release: the same two unnamed decisions admitted, each counted; a named one not counted.
    let dir = scratch("unnamed-opt-out");
    let http_port = alloc_port();
    let (id, mut cfg) = config(&dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: None });
    cfg.gateway_accept_unnamed_decisions = true;
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("starts");
    std::mem::forget(a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| async move { Ok(serde_json::json!("sunny")) }));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = forecaster_key(&a, &dir);
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let mut requests = Vec::new();
    for i in 0..3 {
        let (_, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {"city": "Metz", "n": i}}))).await;
        requests.push(asked["request"].as_str().unwrap().to_string());
    }
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let signed = post_decision(http_port, "relay-secret", &requests[0], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision(&treasurer, DECIDER, &node, FORECASTER, &requests[0], "issue", Some(600), None, now)})).await;
    let bearer = http_raw(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(&requests[1])), Some(serde_json::json!({"decision": "refuse", "reason": "no"}))).await;
    let named = post_decision(http_port, "relay-secret", &requests[2], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, &requests[2], "issue", Some(600), None, now, &shown(&requests[2]))})).await;
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((signed.0, bearer.0, named.0), (200, 200, 200), "OPT-OUT: {} / {} / {}", signed.1, bearer.1, named.1);
    assert_eq!(stats["grant_decisions_unnamed"].as_u64(), Some(2), "COUNTED: each unnamed decision answered: {stats}");
}

/// **An honest re-send of an answered decision is told it was answered, after the request's window too** (#820's
/// review, M1). A decider whose answer timed out re-sends the same signed body. The request's window is 2 s; the
/// signature stays fresh far longer. Issued `200`, re-sent at once `409 already_answered`, and re-sent once the window
/// has passed: still `409 already_answered` — never `410 expired … the agent has refused the call` while the grant it
/// issued stands. Seen failing first with the block's expiry checked before `already_answered` (the third answer was
/// `410 expired`, the grant issued).
#[tokio::test]
async fn an_answered_decision_re_sent_after_the_window_is_told_it_was_answered() {
    let dir = scratch("shown-resend");
    let http_port = alloc_port();
    let (a, _) = started(&dir, http_port).await;
    let _ = forecaster_key(&a, &dir);
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let (status, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {"city": "Lyon"}, "window_secs": 2}))).await;
    assert_eq!(status, 200, "{asked}");
    let request = asked["request"].as_str().unwrap().to_string();
    let shown = asked_block(&request).expect("asked here");
    let node = a.node_id().to_string();
    let body = serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, &request, "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms(), &shown)});
    let first = post_decision(http_port, "relay-secret", &request, body.clone()).await;
    let again = post_decision(http_port, "relay-secret", &request, body.clone()).await;
    let lapsed = until(|| a.task_ctx.hlc.decision_now_ms() > shown.request_expires_at_ms + 50, Duration::from_secs(10)).await;
    let after = post_decision(http_port, "relay-secret", &request, body).await;
    let issued = mycelium_core::ops::kv_get(&a.task_ctx, &format!("{}/issued", record_key(&request))).is_some();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(first.0, 200, "{}", first.1);
    assert_eq!(code(&again), (409, Some("already_answered".into())), "{}", again.1);
    assert!(lapsed, "the request's window passed");
    assert_eq!(code(&after), (409, Some("already_answered".into())), "RE-SENT AFTER THE WINDOW: {}", after.1);
    assert!(issued, "the grant stands");
}
