//! Identity-and-lifecycle I7, PR 3a: **`dev`'s side against a node** — a consent decision that is actually signed
//! outside a test fixture. `dev`'s own code (`crate::dev_consent`) creates the software key, enrols under the
//! certificate the CA issued over its public half, reads the request's record from the gateway under a low-scope
//! bearer, renders the prompt, and signs what the approver answers; the node verifies it at `grant_decision::verify`
//! and records `signed_with = tty`. A child of `consent_decision_tests`, for its fleet.
//!
//! **What stands in for the person:** [`Scripted`], an approver that answers from a list. No test here has a terminal
//! or a person; the terminal itself (`/dev/tty`, `TtyApprover`) is driven through a pseudo-terminal in
//! `tests/dev_consent_cli.rs`, against a stub gateway. `tty` is the grade of a key in a same-user file, and these
//! tests are the demonstration that a process with no person signs with it (I7's residual B).

use super::*;
use crate::dev_consent::{self as dc, Approval};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// The consent principal of these tests: a developer's, under this file's fleet.
const ALEX: &str = "consent:fleet/alex";

/// What a test does while a prompt is up.
type During = Box<dyn FnOnce(&dc::Prompt)>;

/// An approver answering from a list, keeping every prompt it was shown; `during` runs while the first prompt is up.
struct Scripted {
    answers: VecDeque<Approval>,
    prompts: Rc<RefCell<Vec<dc::Prompt>>>,
    during: Option<During>,
}

impl Scripted {
    fn answering(answers: impl IntoIterator<Item = Approval>) -> Self {
        Self { answers: answers.into_iter().collect(), prompts: Rc::default(), during: None }
    }
}

impl dc::Approver for Scripted {
    fn ask(&mut self, prompt: &dc::Prompt) -> Approval {
        self.prompts.borrow_mut().push(prompt.clone());
        if let Some(during) = self.during.take() {
            during(prompt);
        }
        self.answers.pop_front().unwrap_or(Approval::Dismissed)
    }
}

/// `dev`'s enrolment, as a developer makes it: the key created by `dev` under `dir/home` (outside any worktree), its
/// public half certified by the node's CA at `tty`, the certificate checked and kept by `dev`, and published.
fn enrolled(a: &GossipAgent, dir: &Path, name: &str) -> (dc::Store, dc::Enrolment) {
    let store = dc::Store::at(dir.join("home").join("consent"), ISSUER, name).expect("outside a worktree");
    let key = store.create_key(false).expect("dev creates the key");
    let cert = crate::issue_consent_cert(&dir.join("tls"), ISSUER, name, &key.spki_pem(), ConsentGrade::Tty, &dir.join("consent-out")).expect("the CA certifies the public half");
    let enrolment = store.enrol(&std::fs::read_to_string(&cert).unwrap()).expect("dev enrols under it");
    a.publish_client_cert(rustls::pki_types::CertificateDer::from_pem_file(&cert).unwrap().as_ref()).expect("published");
    (store, enrolment)
}

fn gateway(http_port: u16) -> dc::Gateway {
    // The relay's bearer: `grant:read` and `grant:relay`. It decides nothing and signs nothing.
    dc::Gateway::new(&format!("http://127.0.0.1:{http_port}"), Some("relay-secret".into())).expect("a client")
}

async fn pending_rows(http_port: u16) -> Vec<serde_json::Value> {
    let (status, body) = http(http_port, "relay-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert_eq!(status, 200, "{body}");
    body["pending"].as_array().cloned().unwrap_or_default()
}

/// Every 16-byte run of the private half of the PKCS#8 at `key_path`, as hex — and the whole as base64: what must
/// appear in nothing `dev` prints, records or sends.
fn private_runs(key_path: &Path) -> Vec<String> {
    use base64::Engine as _;
    let pkcs8 = std::fs::read(key_path).expect("the key file");
    let private = &pkcs8[..pkcs8.len() - 65];
    let mut runs: Vec<String> = private.windows(16).map(|w| w.iter().map(|b| format!("{b:02x}")).collect()).collect();
    runs.push(base64::engine::general_purpose::STANDARD.encode(&pkcs8));
    runs.push(base64::engine::general_purpose::STANDARD.encode(private));
    runs
}

/// **A decision answered at `dev`'s prompt is signed by its software key, verified by the node and recorded `tty`**
/// (I7, P7; exit 6 — the terminal itself is `tests/dev_consent_cli.rs`'s). `dev` creates the key, the CA certifies
/// its public half at `tty`, and three requests are put to `dev` by id:
/// - **approved**: the gateway answers `200`; the evidence record's subject is `consent:fleet/alex`, its
///   `signed_decision.signed_with` is `p256`, `tty`, `grade_source: operator`; the kept body names `tty` and
///   re-verifies from the record alone;
/// - **dismissed**: nothing is signed, nothing is sent, the request is still open and no record names it (exit 2's
///   "a dismissed prompt yields no signature and no decision", for this key);
/// - **refused**: a signed refusal, recorded with the same decider and grade.
///
/// Each prompt shows what the gateway's record says under the id — the holder, the tool, the resource, the digest.
/// And **the private key is in nothing `dev` showed, printed or sent, nor in the node's evidence journal**.
///
/// Seen failing first, one build per plant: `Enrolment::sign` naming no grade (`403 decision_grade_missing`);
/// `answer_one` signing a dismissed prompt as an issue (the dismissed request was answered `signed`).
#[tokio::test]
async fn a_decision_typed_at_devs_prompt_is_signed_by_its_software_key_and_recorded_tty() {
    let dir = scratch("dev-consent");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table(&["alex"])).await;
    let _ = forecaster_key(&a, &dir);
    let (store, enrolment) = enrolled(&a, &dir, "alex");
    let requests = ask(http_port, 3).await;
    let rows = pending_rows(http_port).await;
    let gw = gateway(http_port);
    let mut approver = Scripted::answering([Approval::Approve, Approval::Dismissed, Approval::Refuse]);
    let mut answers = Vec::new();
    for request in &requests {
        answers.push(tokio::time::timeout(Duration::from_secs(30), dc::answer_one(&gw, &enrolment, &mut approver, request, 600)).await.expect("dev answers within 30 s"));
    }
    let still_open: Vec<String> = pending_rows(http_port).await.iter().filter_map(|r| r["request"].as_str().map(str::to_string)).collect();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let journal_text = String::from_utf8_lossy(&std::fs::read(dir.join("evidence.log")).unwrap_or_default()).to_string();
    let private = private_runs(&store.key_path());
    let _ = std::fs::remove_dir_all(&dir);

    // Approved.
    let dc::Answer::Signed { decision, status, gateway: said } = &answers[0] else { panic!("APPROVED: {:?}", answers[0]) };
    assert_eq!((*decision, *status), ("issue", 200), "APPROVED: the node admitted dev's signed issue: {said}");
    let record = decided(&journal, "grant.issue", ALEX);
    assert_eq!(record.len(), 1, "JOURNAL: the consent principal is the actor of one issue");
    let kept = record[0].signed_decision.clone().unwrap_or_default();
    let w = &kept["signed_with"];
    assert_eq!((w["key"].as_str(), w["grade"].as_str(), w["grade_source"].as_str()), (Some("p256"), Some("tty"), Some("operator")), "SIGNED_WITH: {kept}");
    let body: crate::SignedDecision = serde_json::from_value(kept["signed"].clone()).expect("the signed body is kept");
    assert_eq!((body.decider.as_str(), body.grade.as_deref(), body.request.as_str()), (ALEX, Some("tty"), requests[0].as_str()));
    assert!(body.verify_signature("issue", Some(600), None), "RE-VERIFY: dev's signature, from the record alone: {kept}");
    // Dismissed.
    assert_eq!(answers[1], dc::Answer::Dismissed, "DISMISSED: nothing signed");
    assert_eq!(still_open, [requests[1].clone()], "DISMISSED: the request is still open, and the other two are answered");
    assert!(!journal_text.contains(&requests[1]), "DISMISSED: no record names the request");
    // Refused.
    let dc::Answer::Signed { decision, status, gateway: said } = &answers[2] else { panic!("REFUSED: {:?}", answers[2]) };
    assert_eq!((*decision, *status), ("refuse", 200), "REFUSED: {said}");
    let refusal = decided(&journal, "grant.refuse", ALEX);
    assert_eq!(refusal.len(), 1, "JOURNAL: one refusal by the consent principal");
    assert_eq!(refusal[0].signed_decision.clone().unwrap_or_default()["signed_with"]["grade"], "tty");
    // The prompts: from the record.
    let prompts = approver.prompts.borrow();
    assert_eq!(prompts.len(), 3, "one prompt per request");
    for (prompt, request) in prompts.iter().zip(&requests) {
        let row = rows.iter().find(|r| r["request"].as_str() == Some(request)).expect("the gateway's record");
        for field in ["request", "agent", "tool", "resource", "arguments_digest"] {
            let said = row[field].as_str().expect(field);
            assert!(prompt.text.contains(said), "PROMPT: names the record's {field} ({said}):\n{}", prompt.text);
        }
        assert!(prompt.text.contains(ALEX) && prompt.text.contains("grade tty") && !prompt.text.contains("os_presence"), "PROMPT: says who decides, and at what grade:\n{}", prompt.text);
    }
    // The private key: nowhere.
    let mut everything = format!("{enrolment:?} {store:?} {journal_text}");
    for (answer, request) in answers.iter().zip(&requests) {
        everything.push_str(&format!("{answer:?} {}", answer.line(request)));
    }
    for prompt in prompts.iter() {
        everything.push_str(&prompt.text);
    }
    for run in &private {
        assert!(!everything.contains(run), "PRIVATE KEY: a run of it appears in what dev showed, answered or the node recorded");
    }
}

/// **A request with no record prompts nothing, and the queue answers `busy` past its bound** (I7, exit 9), against a
/// node. A queue of two:
/// - an id no record holds is answered `unknown_request` and **never reaches the approver**;
/// - an id offered while two wait is answered `busy` at once, and is not prompted;
/// - offered again once there is room — here while the first prompt is up — it is prompted like any other;
/// - a fourth id offered behind a full queue is `busy`, never prompted, and still open at the gateway.
///
/// Seen failing first, one build per plant: `answer_one` falling back to the first open request for an unknown id
/// (the unknown id raised the first prompt, so the offers made during it found a request still waiting: `[Ok,
/// Busy, Busy]`); the queue made larger than its bound (the third offer waited instead of being answered `busy`).
#[tokio::test]
async fn a_request_with_no_record_prompts_nothing_and_the_queue_answers_busy_past_its_bound() {
    let dir = scratch("dev-consent-queue");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table(&["alex"])).await;
    let _ = forecaster_key(&a, &dir);
    let (store, _enrolment) = enrolled(&a, &dir, "alex");
    let requests = ask(http_port, 4).await;
    let gw = gateway(http_port);

    let (handle, queue) = dc::queue(2);
    assert_eq!(handle.offer("no-such-request"), Ok(()));
    assert_eq!(handle.offer(&requests[0]), Ok(()));
    assert_eq!(handle.offer(&requests[1]), Err(dc::Busy), "BUSY: two wait, the third is answered at once");
    let late: Rc<RefCell<Vec<Result<(), dc::Busy>>>> = Rc::default();
    let mut approver = Scripted::answering([Approval::Approve, Approval::Approve, Approval::Approve, Approval::Approve]);
    approver.during = Some(Box::new({
        let (late, ids) = (Rc::clone(&late), requests.clone());
        // While the first prompt is up the queue is empty: two more fit, the next does not. Then the caller goes away.
        move |_| late.borrow_mut().extend([handle.offer(&ids[1]), handle.offer(&ids[2]), handle.offer(&ids[3])])
    }));
    let mut said: Vec<(String, dc::Answer)> = Vec::new();
    let counts = tokio::time::timeout(Duration::from_secs(60), dc::serve(&gw, &store, &mut approver, queue, 600, &mut |r, answer| said.push((r.to_string(), answer.clone())))).await.expect("the queue drains within 60 s");
    let still_open: Vec<String> = pending_rows(http_port).await.iter().filter_map(|r| r["request"].as_str().map(str::to_string)).collect();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(*late.borrow(), [Ok(()), Ok(()), Err(dc::Busy)], "BUSY: offered during the prompt — two fit, the third is busy");
    let words: Vec<(&str, &str)> = said.iter().map(|(r, a)| (r.as_str(), a.word())).collect();
    assert_eq!(words, [("no-such-request", "unknown_request"), (requests[0].as_str(), "signed"), (requests[1].as_str(), "signed"), (requests[2].as_str(), "signed")], "ANSWERS: {said:?}");
    assert_eq!(counts, (3, 1), "three admitted, one not");
    let prompted: Vec<String> = approver.prompts.borrow().iter().map(|p| p.record.request.clone()).collect();
    assert_eq!(prompted, requests[..3], "PROMPTS: the unknown id and the busy id raised none");
    assert_eq!(still_open, [requests[3].clone()], "the busy request is untouched");
}

/// **`dev` signs nothing when the record changed under the prompt.** While the prompt for a weather forecast is up,
/// the request's record — unsigned in the KV, writable by any member — is rewritten to another call's arguments and
/// their digest. `dev` reads the record again after the answer: it no longer says what was shown, so `approve` signs
/// nothing and sends nothing (`record_changed`), and the request stays open. **What this does not close** is stated
/// in `crate::dev_consent` (#813's review, F1): the signature names neither tool nor digest, so a rewrite after
/// `dev`'s second read is applied by the node, and the same signed decision can be admitted again. It is closed by
/// the approver signing what it saw (plan row I7-S).
///
/// Seen failing first: the second read removed (the changed request was signed, `200`).
#[tokio::test]
async fn dev_signs_nothing_when_the_record_changed_under_the_prompt() {
    let dir = scratch("dev-consent-changed");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table(&["alex"])).await;
    let _ = forecaster_key(&a, &dir);
    let (_store, enrolment) = enrolled(&a, &dir, "alex");
    let requests = ask(http_port, 2).await;
    let rows = pending_rows(http_port).await;
    let row = rows.iter().find(|r| r["request"].as_str() == Some(&requests[0])).expect("the record").clone();
    let gw = gateway(http_port);
    let mut approver = Scripted::answering([Approval::Approve, Approval::Approve]);
    approver.during = Some(Box::new({
        let (a, mut row) = (Arc::clone(&a), row.clone());
        move |_| {
            let key = row["key"].as_str().unwrap().to_string();
            row.as_object_mut().unwrap().remove("key");
            // Another call, and a record that agrees with itself: its arguments hash to its digest.
            let other = serde_json::json!({"city": "Paris"});
            row["arguments_digest"] = crate::arguments_digest(&other).iter().map(|b| format!("{b:02x}")).collect::<String>().into();
            row["arguments"] = other;
            assert!(a.kv().set(key, bytes::Bytes::from(serde_json::to_vec(&row).unwrap())), "a member rewrites the request record");
        }
    }));
    let changed = tokio::time::timeout(Duration::from_secs(30), dc::answer_one(&gw, &enrolment, &mut approver, &requests[0], 600)).await.expect("bounded");
    let untouched = tokio::time::timeout(Duration::from_secs(30), dc::answer_one(&gw, &enrolment, &mut approver, &requests[1], 600)).await.expect("bounded");
    let still_open: Vec<String> = pending_rows(http_port).await.iter().filter_map(|r| r["request"].as_str().map(str::to_string)).collect();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(changed, dc::Answer::RecordChanged, "CHANGED: approve was typed for another record than the one now served");
    assert_eq!(still_open, [requests[0].clone()], "CHANGED: nothing was sent, so the request is still open");
    assert!(untouched.admitted(), "the control: an unchanged record is signed and admitted: {untouched:?}");
    assert_eq!(decided(&journal, "grant.issue", ALEX).len(), 1, "one issue — the unchanged request's");
}

/// **`dev` publishes its certificate through the client-certificate door, and `signed` is not `admitted`** (I7's
/// issue flow, step 3: "`dev` publishes it through I1's door"). `dev`'s key is certified and enrolled but not yet
/// published:
/// - a decision `dev` signs is **sent and refused by the node** — the answer is `signed`, with the gateway's refusal
///   quoted beside it, and it is not counted as admitted: `dev` says what it did, the node says what it admitted;
/// - the relay's bearer (`grant:read`, `grant:relay`) cannot publish: the door answers it `403`;
/// - under a bearer holding `identity:write` the door publishes it and answers the consent principal and the grade
///   with `grade_source: operator`;
/// - the next decision `dev` signs is admitted.
///
/// Seen failing first: `Gateway::publish` posting the certificate under another field name (the door answered `400`).
#[tokio::test]
async fn dev_publishes_its_certificate_through_the_door_and_signed_is_not_admitted() {
    let dir = scratch("dev-consent-publish");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| {
        cfg.gateway_consent_principals = consent_table(&["alex"]);
        cfg.gateway_named_tokens.push(crate::config::GatewayNamedToken { name: "operator".into(), token: "operator-secret".into(), scopes: vec!["identity:write".into()] });
    })
    .await;
    let _ = forecaster_key(&a, &dir);
    let store = dc::Store::at(dir.join("home").join("consent"), ISSUER, "alex").expect("outside a worktree");
    let key = store.create_key(false).expect("dev creates the key");
    let cert = crate::issue_consent_cert(&dir.join("tls"), ISSUER, "alex", &key.spki_pem(), ConsentGrade::Tty, &dir.join("consent-out")).expect("certified");
    let enrolment = store.enrol(&std::fs::read_to_string(&cert).unwrap()).expect("enrolled, not published");
    let requests = ask(http_port, 2).await;
    let gw = gateway(http_port);
    let mut approver = Scripted::answering([Approval::Approve, Approval::Approve]);
    let bounded = Duration::from_secs(30);

    let before = tokio::time::timeout(bounded, dc::answer_one(&gw, &enrolment, &mut approver, &requests[0], 600)).await.expect("bounded");
    let by_relay = tokio::time::timeout(bounded, gw.publish(enrolment.cert_pem())).await.expect("bounded").expect("the door answers");
    let operator = dc::Gateway::new(&format!("http://127.0.0.1:{http_port}"), Some("operator-secret".into())).unwrap();
    let published = tokio::time::timeout(bounded, operator.publish(enrolment.cert_pem())).await.expect("bounded").expect("the door answers");
    let after = tokio::time::timeout(bounded, dc::answer_one(&gw, &enrolment, &mut approver, &requests[1], 600)).await.expect("bounded");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    let dc::Answer::Signed { status, gateway: refusal, .. } = &before else { panic!("BEFORE: dev signed and sent: {before:?}") };
    assert!((400..500).contains(status) && refusal["ok"] == false, "BEFORE: the node refuses an unpublished key's decision: {status} {refusal}");
    assert!(!before.admitted() && before.word() == "signed", "BEFORE: signed, and not admitted");
    assert_eq!(by_relay.0, 403, "RELAY: a bearer without identity:write does not publish: {}", by_relay.1);
    assert_eq!(published.0, 200, "PUBLISHED: {}", published.1);
    assert_eq!((published.1["principal"].as_str(), published.1["grade"].as_str(), published.1["grade_source"].as_str()), (Some(ALEX), Some("tty"), Some("operator")), "PUBLISHED: {}", published.1);
    assert!(after.admitted(), "AFTER: the published key's decision is admitted: {after:?}");
}

/// **`dev` signs what its prompt showed, and the node refuses a record rewritten after `dev`'s second read**
/// (identity-and-lifecycle I7-S; #813's review, F1, which `dev_signs_nothing_when_the_record_changed_under_the_prompt`
/// left open). An approval at the prompt is signed naming the record's tool, resource, digest and expiry — exactly
/// the block of the record the prompt was rendered from. Then the steps of `answer_one` are taken by hand so that a
/// member can rewrite the record **after** `dev` read it the second time and signed: the decision is refused `409
/// decision_names_another_request` naming the digest, and nothing is issued. And a consent decision naming no block
/// is refused `decision_shown_missing` before any key is used. Seen failing first: `Enrolment::sign` naming no block
/// (the approval refused `decision_shown_missing`; before the node's comparison, the rewritten decision was issued).
#[tokio::test]
async fn dev_names_what_it_showed_and_a_rewrite_after_its_second_read_is_refused() {
    let dir = scratch("dev-consent-shown");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table(&["alex"])).await;
    let _ = forecaster_key(&a, &dir);
    let (_store, enrolment) = enrolled(&a, &dir, "alex");
    let requests = ask(http_port, 2).await;
    let gw = gateway(http_port);
    let mut approver = Scripted::answering([Approval::Approve]);
    let approved = tokio::time::timeout(Duration::from_secs(30), dc::answer_one(&gw, &enrolment, &mut approver, &requests[0], 600)).await.expect("bounded");
    // By hand: read, sign, then the rewrite, then send.
    let (pending, _) = gw.pending().await.expect("pending");
    let record = dc::PromptRecord::find(&pending, &requests[1]).expect("a record dev can show");
    let node = gw.node_id().await.expect("the node");
    let signed = enrolment.sign(&node, &record, "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms()).expect("signed");
    let mut row = pending.as_array().unwrap().iter().find(|r| r["request"].as_str() == Some(requests[1].as_str())).unwrap().clone();
    row.as_object_mut().unwrap().remove("key");
    let other = serde_json::json!({"city": "Paris"});
    row["arguments_digest"] = crate::agent::action_evaluator::hex32(&crate::arguments_digest(&other)).into();
    row["arguments"] = other;
    assert!(a.kv().set(record.key.clone(), bytes::Bytes::from(serde_json::to_vec(&row).unwrap())), "a member rewrites the record after dev's second read");
    let rewritten = gw.decide(&record, &serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed})).await.expect("sent");
    let unnamed = crate::SignedDecision { shown: None, ..signed.clone() };
    let missing = verified(&a, &unnamed, &requests[1]);
    let issued = mycelium_core::ops::kv_get(&a.task_ctx, &format!("{}/issued", record.key)).is_some();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(approved.admitted(), "APPROVED: {approved:?}");
    let shown_at_prompt = approver.prompts.borrow()[0].record.shown();
    let body: crate::SignedDecision = serde_json::from_value(decided(&journal, "grant.issue", ALEX)[0].signed_decision.clone().unwrap()["signed"].clone()).unwrap();
    assert_eq!(body.shown.as_ref(), Some(&shown_at_prompt), "NAMED: dev signs the block of the record its prompt showed");
    assert_eq!(signed.shown.as_ref(), Some(&record.shown()), "the block is the record's: its digest string, its expiry");
    assert_eq!((rewritten.0, rewritten.1["error"].as_str()), (409, Some("decision_names_another_request")), "REWRITE: {}", rewritten.1);
    assert!(rewritten.1["detail"].as_str().unwrap_or_default().starts_with("the decision names arguments_digest "), "{}", rewritten.1);
    assert!(!issued, "nothing issued for the rewritten record");
    assert_eq!(refusal(&missing), "decision_shown_missing", "{missing:?}");
}

/// **A request recorded under the pre-I3 digest form is approved by `dev` and admitted** (I7-S, the report's C2;
/// `deprecations.md` §93, for as long as the nodes honour that form). `dev` checks the record's digest against the
/// arguments it shows under either form, and signs the record's own digest string — so the node, comparing byte for
/// byte, admits it. Seen failing first with `dev` signing the RFC 8785 digest it recomputed (`409
/// decision_names_another_request`, naming the digest).
#[tokio::test]
async fn dev_approves_a_request_recorded_under_the_legacy_digest() {
    let dir = scratch("dev-consent-legacy");
    let http_port = alloc_port();
    let unit = unit_with(&decides(ALEX, ""));
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table(&["alex"])).await;
    let _ = forecaster_key(&a, &dir);
    let (_store, enrolment) = enrolled(&a, &dir, "alex");
    let args = serde_json::json!({"\u{ff01}": 1, "\u{1F600}": 2});
    let legacy = crate::agent::action_evaluator::hex32(&crate::legacy_arguments_digest(&args));
    assert_ne!(legacy, crate::agent::action_evaluator::hex32(&crate::arguments_digest(&args)), "the two forms differ here");
    let (_, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args}))).await;
    let key = asked["key"].as_str().unwrap().to_string();
    let mut record: serde_json::Value = serde_json::from_slice(&mycelium_core::ops::kv_get(&a.task_ctx, &key).unwrap()).unwrap();
    record["arguments_digest"] = legacy.clone().into();
    assert!(a.kv().set(key.clone(), bytes::Bytes::from(serde_json::to_vec(&record).unwrap())), "the record as a pre-I3 gateway wrote it");
    let gw = gateway(http_port);
    let mut approver = Scripted::answering([Approval::Approve]);
    let answer = tokio::time::timeout(Duration::from_secs(30), dc::answer_one(&gw, &enrolment, &mut approver, asked["request"].as_str().unwrap(), 600)).await.expect("bounded");
    let issued = mycelium_core::ops::kv_get(&a.task_ctx, &format!("{key}/issued")).is_some();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(answer.admitted(), "LEGACY: {answer:?}");
    assert!(issued, "issued");
    assert_eq!(approver.prompts.borrow()[0].record.shown().arguments_digest, legacy, "dev names the record's own digest string");
}
