//! Identity-and-lifecycle I7, PR 2: **the grade in signed decisions** — one P-256 verifier at
//! `grant_decision::verify`, the grade inside the signed message, consent's own authority
//! (`gateway_consent_principals`), and `signed_with` on the record and the envelope. A child of
//! `client_grant_tests`, for its fleet and its decision helpers; built only where a consent certificate is read
//! (`provisional-consent-grade`).
//!
//! What a grade is, throughout: for a consent key, **what the operator who issued the certificate was told** — no
//! test here observes a presence, and none could.

use super::*;
use crate::agent::action_evaluator::{AeEvidence, DecisionKind, RecordKind};
use crate::agent::client_cert::ClientKeyStanding;
use crate::agent::grant_decision::{self as gd, SignedWith};
use crate::ConsentGrade;

const RICHARD: &str = "consent:fleet/richard";
const RICHARD_SERVICE: &str = "token:fleet/richard";
const OS: &str = "os_presence(macos, unbound)";

/// The unit of these tests: the base unit, and Richard permitted to decide as his consent principal **and**, by a
/// separate rule, as his service principal — so the door's scope check, not the policy, tells the two apart.
fn unit_with(rules: &str) -> String {
    format!("{UNIT}\n{rules}")
}

fn decides(actor: &str, extra: &str) -> String {
    ["grant.issue", "grant.revoke", "grant.refuse"]
        .iter()
        .map(|op| format!("[[rule]]\nactor = \"{actor}\"\noperation = \"{op}\"\nresource = \"tool:weather.forecast\"\n{extra}\n"))
        .collect()
}

/// A node from `dir`, its unit replaced by `unit` and its configuration changed by `mutate`.
async fn started_with(dir: &Path, http_port: u16, port: Option<u16>, unit: &str, mutate: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
    std::fs::write(dir.join("units").join("weather.toml"), unit).unwrap();
    let (id, mut cfg) = config(dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port });
    mutate(&mut cfg);
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
    let handle = a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| async move { Ok(serde_json::json!("sunny")) });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(150)).await;
    a
}

/// A consent table: each name holding `grant:issue`, its decisions accepted at `tty` — the grade most tests certify.
fn consent_table(names: &[&str]) -> Vec<crate::config::GatewayConsentPrincipal> {
    consent_table_at(names, "tty")
}

fn consent_table_at(names: &[&str], grade: &str) -> Vec<crate::config::GatewayConsentPrincipal> {
    names.iter().map(|n| crate::config::GatewayConsentPrincipal::new(*n, grade)).collect()
}

/// A P-256 consent key as a developer's device would hold it: the private half stays here, the issuer sees the point.
struct ConsentKey {
    pkcs8: Vec<u8>,
    point: [u8; 65],
}

impl ConsentKey {
    fn new() -> Self {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        Self { pkcs8: kp.serialize_der(), point: kp.public_key_raw().try_into().unwrap() }
    }

    /// A consent certificate for this key under `name`, issued over its public key alone with `grade`.
    fn certificate(&self, dir: &Path, name: &str, out: &str, grade: ConsentGrade) -> Vec<u8> {
        let cert = crate::issue_consent_cert(&dir.join("tls"), ISSUER, name, &crate::p256_spki_pem(&self.point), grade, &dir.join(out)).expect("issued over the public key alone");
        rustls::pki_types::CertificateDer::from_pem_file(&cert).unwrap().as_ref().to_vec()
    }

    /// Issue and publish through the door's function.
    fn enrol(&self, a: &GossipAgent, dir: &Path, name: &str, out: &str, grade: ConsentGrade) -> Vec<u8> {
        let der = self.certificate(dir, name, out, grade);
        a.publish_client_cert(&der).expect("the door publishes a consent certificate");
        der
    }

    /// A decision by `decider` on the forecaster's `request`, naming `grade`, signed by this key. A graded issue or
    /// refusal names what the request door answered for `request` (identity-and-lifecycle I7-S, [`asked_block`]), or —
    /// for the tests that verify a body with no request behind it — a block that is no record's.
    #[allow(clippy::too_many_arguments)]
    fn sign(&self, decider: &str, node: &str, request: &str, decision: &str, ttl: Option<u64>, reason: Option<&str>, grade: Option<&str>) -> crate::SignedDecision {
        let shown = (grade.is_some() && decision != "revoke").then(|| asked_block(request).unwrap_or_else(|| crate::Shown::new(TOOL, format!("tool:{TOOL}@{node}"), "00", u64::MAX)));
        self.sign_naming(decider, node, request, decision, ttl, reason, grade, shown)
    }

    #[allow(clippy::too_many_arguments)]
    fn sign_naming(&self, decider: &str, node: &str, request: &str, decision: &str, ttl: Option<u64>, reason: Option<&str>, grade: Option<&str>, shown: Option<crate::Shown>) -> crate::SignedDecision {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let t = mycelium_core::sim_seam::wall_now_ms();
        let msg = crate::grant_decision_message_shown(decider, node, FORECASTER, request, decision, ttl, reason, t, grade, shown.as_ref());
        let sig = mycelium_core::tls::sign_consent_p256_in_test_build(&self.pkcs8, &msg).expect("signed");
        let signed = crate::SignedDecision::from_parts(decider, node, FORECASTER, request, t, b64.encode(self.point), b64.encode(sig), grade);
        match shown {
            Some(s) => signed.naming(s),
            None => signed,
        }
    }
}

/// `grant_decision::verify` on the node itself, for an issue of `ttl` 600 on `request`: the refusal's code, or what
/// the decision is recorded as signed with — the key's kind, the grade and its source; which certificate the grade
/// was read from is [`verified_whole`]'s.
fn verified(a: &GossipAgent, signed: &crate::SignedDecision, request: &str) -> Result<SignedWith, String> {
    verified_whole(a, signed, request).map(|s| s.from_certificate(None, None))
}

fn verified_whole(a: &GossipAgent, signed: &crate::SignedDecision, request: &str) -> Result<SignedWith, String> {
    gd::verify(&a.task_ctx, signed, FORECASTER, request, "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms()).map_err(|e| format!("{}: {}", e.code(), e.detail()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    <sha2::Sha256 as sha2::Digest>::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn refusal(r: &Result<SignedWith, String>) -> &str {
    r.as_ref().err().map(|e| e.split(':').next().unwrap_or_default()).unwrap_or("admitted")
}

async fn ask(http_port: u16, n: usize) -> Vec<String> {
    let mut requests = Vec::new();
    for i in 0..n {
        let (status, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {"city": "Lyon", "n": i}}))).await;
        assert_eq!(status, 200, "{asked}");
        requests.push(asked["request"].as_str().unwrap().to_string());
    }
    requests
}

fn issue_body(signed: crate::SignedDecision) -> serde_json::Value {
    serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed})
}

fn decided<'a>(journal: &'a [AeEvidence], op: &str, subject: &str) -> Vec<&'a AeEvidence> {
    journal.iter().filter(|r| r.kind == RecordKind::Decided && r.operation == op && r.subject == subject).collect()
}

/// **A consent decision is verified by the one P-256 verifier and recorded with the grade its certificate carries**
/// (I7: P2, P4/H2; exits 5 and 6's verifier half). Richard's consent key is certified `tty` and
/// `consent:fleet/richard` holds `grant:issue` in the consent table. His signed issue, carried by a relay, is
/// answered: the evidence record's subject is the consent principal and its `signed_decision.signed_with` says
/// `p256`, `tty`, and that the grade is the **operator's** statement; the body re-verifies from the record alone. His
/// signed revocation's record carries the same `signed_with`. A service decider's records say `ed25519`, `service`,
/// `certificate_scheme` — naming no grade (a pre-I7 body) and naming `service`.
///
/// Each `signed_with` names **which certificate the grade was read from** — its issuance, and for the consent key its
/// digest — so an auditor can tell, after a re-issue over the same key, which certificate the node's statement was
/// about (#810's review, finding 2).
///
/// Seen failing first, one build per plant: `verify` answering every decision `SignedWith::service()` (the consent
/// record said `service`); `Relayed::mark` not writing `signed_with` (the revocation record carried none); `verify`
/// not naming the certificate (the record carried no issuance or digest).
#[tokio::test]
async fn a_consent_decision_is_verified_and_recorded_with_the_grade_its_certificate_carries() {
    let dir = scratch("consent-decide");
    let http_port = alloc_port();
    let unit = unit_with(&decides(RICHARD, ""));
    // The service's two bodies below name nothing of what was seen (a pre-I7 body, and one naming only `service`):
    // admitted here under the one-release opt-out (I7-S, D2), so the grade they are recorded with is what is tested.
    let a = started_with(&dir, http_port, None, &unit, |cfg| {
        cfg.gateway_consent_principals = consent_table(&["richard"]);
        cfg.gateway_accept_unnamed_decisions = true;
    })
    .await;
    let _ = forecaster_key(&a, &dir);
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let key = ConsentKey::new();
    let certificate = key.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
    let certificate_issued = mycelium_core::tls::consent_identity(&certificate).and_then(|id| id.issued_ms).expect("the certificate's issuance");
    let node = a.node_id().to_string();
    let requests = ask(http_port, 3).await;

    let issued = post_decision(http_port, "relay-secret", &requests[0], issue_body(key.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
    let revoked = post_revoke(http_port, "relay-secret", &requests[0], serde_json::json!({"reason": "season over", "signed": key.sign(RICHARD, &node, &requests[0], "revoke", None, Some("season over"), Some("tty"))})).await;
    let revocation: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{}/revoked", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), requests[0]))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let plain = post_decision(http_port, "relay-secret", &requests[1], issue_body(crate::sign_grant_decision(&treasurer, DECIDER, &node, FORECASTER, &requests[1], "issue", Some(600), None, now))).await;
    let named = crate::sign_grant_decision_graded(&treasurer, DECIDER, &node, FORECASTER, &requests[2], "issue", Some(600), None, now, Some("service"));
    let named = post_decision(http_port, "relay-secret", &requests[2], issue_body(named)).await;
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(issued.0, 200, "CONSENT: the relayed consent-signed issue: {}", issued.1);
    assert_eq!(revoked.0, 200, "CONSENT REVOKE: {}", revoked.1);
    // Which certificate the grade was read from travels with it (#810's review, finding 2): its issuance and digest.
    let operator_tty = serde_json::json!({"key": "p256", "grade": "tty", "grade_source": "operator", "certificate_issued_ms": certificate_issued, "certificate_sha256": sha256_hex(&certificate)});
    let record = decided(&journal, "grant.issue", RICHARD);
    assert_eq!(record.len(), 1, "JOURNAL: the consent principal is the actor: {:?}", journal.iter().filter(|r| r.operation == "grant.issue").map(|r| &r.subject).collect::<Vec<_>>());
    let kept = record[0].signed_decision.clone().unwrap_or_default();
    assert_eq!(kept["signed_with"], operator_tty, "SIGNED_WITH: the evidence record: {kept}");
    let body: crate::SignedDecision = serde_json::from_value(kept["signed"].clone()).expect("the signed body is kept");
    assert_eq!(body.grade.as_deref(), Some("tty"));
    assert!(body.verify_signature("issue", kept["ttl_secs"].as_u64(), kept["reason"].as_str()), "RE-VERIFY: the consent signature from the record alone: {kept}");
    assert!(!body.verify_signature("issue", Some(601), None), "another term re-verified");
    assert_eq!((revocation["issuer"].as_str(), &revocation["signed_with"]), (Some(RICHARD), &operator_tty), "REVOCATION RECORD: {revocation}");
    assert_eq!((plain.0, named.0), (200, 200), "{} / {}", plain.1, named.1);
    let by_service: Vec<_> = decided(&journal, "grant.issue", DECIDER).iter().map(|r| r.signed_decision.clone().unwrap_or_default()).collect();
    assert_eq!(by_service.len(), 2);
    for kept in &by_service {
        let w = &kept["signed_with"];
        assert_eq!((w["key"].as_str(), w["grade"].as_str(), w["grade_source"].as_str()), (Some("ed25519"), Some("service"), Some("certificate_scheme")), "SERVICE: {kept}");
        assert!(w["certificate_issued_ms"].as_u64().is_some_and(|t| t > 0) && w.get("certificate_sha256").is_none(), "SERVICE CERTIFICATE: its issuance, and no digest (the certificate is not read): {kept}");
    }
    assert_eq!(by_service.iter().filter(|k| k["signed"]["grade"] == "service").count(), 1, "one body named `service`, one named no grade");
    assert_eq!(by_service.iter().filter(|k| k["signed"].get("grade").is_none()).count(), 1, "a body naming no grade is kept as it was sent");
}

/// **The grade is the certificate's, inside the signature** (I7 exit 5), on the verifier itself. Three consent
/// principals, each certified one grade:
/// - a body naming **any other** grade — each grade against each other grade, and `service` — is refused
///   `decision_grade_mismatch`; the certificate's own grade is admitted, recorded with it and `grade_source:
///   operator` (`recording`: in a `sim` build only; elsewhere `recording_grade_outside_sim`);
/// - a body naming no grade is refused `decision_grade_missing`, and an empty one `decision_grade_empty`;
/// - **a relay altering the grade breaks the signature**: signed naming another grade and rewritten to the
///   certificate's, or signed with the right grade and the field stripped — `not_the_decider` /
///   `decision_grade_missing`, never admitted;
/// - a **service** decider naming `service` or nothing is recorded `service`; naming any consent grade it is refused
///   `decision_grade_mismatch`; its `service`-signed body with the field stripped no longer verifies;
/// - **an Ed25519 service key presented as a consent key is refused** — under the consent principal (signed by the
///   service's own current key), by name — and a P-256 key under a service principal likewise.
///
/// Seen failing first, one build per plant: the certificate's grade not compared (every mismatch admitted); the
/// message written without the grade (the relay's rewrite admitted); the key-kind check removed (the Ed25519 key
/// answered by the standing reader's `Unknown`, not by name — PR 1's guard still refused it); the `recording` rule
/// returning `true` (a non-`sim` build admitted `recording`).
#[tokio::test]
async fn a_decision_names_the_grade_its_certificate_carries_and_a_relay_cannot_alter_it() {
    let dir = scratch("consent-grade");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, UNIT, |_| {}).await;
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let node = a.node_id().to_string();
    let grades = ConsentGrade::ALL;
    let keys: Vec<(String, ConsentKey, ConsentGrade)> = grades
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let key = ConsentKey::new();
            key.enrol(&a, &dir, &format!("g{i}"), &format!("consent-g{i}"), *g);
            (format!("consent:fleet/g{i}"), key, *g)
        })
        .collect();
    let mut wrong = Vec::new();
    for (principal, key, certified) in &keys {
        for named in grades.iter().map(|g| g.as_str()).chain(["service"]) {
            let got = verified(&a, &key.sign(principal, &node, "r1", "issue", Some(600), None, Some(named)), "r1");
            let want = if named != certified.as_str() {
                "decision_grade_mismatch"
            } else if *certified == ConsentGrade::Recording && !cfg!(feature = "sim") {
                "recording_grade_outside_sim"
            } else {
                "admitted"
            };
            if refusal(&got) != want {
                wrong.push(format!("certified {certified}, named {named}: {got:?}, want {want}"));
            }
            if want == "admitted" && got != Ok(SignedWith::consent(certified.as_str())) {
                wrong.push(format!("certified {certified}: recorded {got:?}"));
            }
        }
        let missing = verified(&a, &key.sign(principal, &node, "r1", "issue", Some(600), None, None), "r1");
        let empty = verified(&a, &key.sign(principal, &node, "r1", "issue", Some(600), None, Some("")), "r1");
        if (refusal(&missing), refusal(&empty)) != ("decision_grade_missing", "decision_grade_empty") {
            wrong.push(format!("certified {certified}: no grade {missing:?}, empty {empty:?}"));
        }
    }
    if crate::recording_grade_accepted_when(false) || !crate::recording_grade_accepted_when(true) {
        wrong.push("RECORDING: the rule accepts `recording` outside a sim build, or refuses it in one".into());
    }

    // A relay altering the grade: the `tty` principal's key signs naming `os_presence`, and the relay rewrites the
    // field to the certificate's `tty`; then the reverse, the right grade signed and the field stripped.
    let (tty_principal, tty_key, _) = keys.iter().find(|(_, _, g)| *g == ConsentGrade::Tty).unwrap();
    let mut rewritten = tty_key.sign(tty_principal, &node, "r1", "issue", Some(600), None, Some(OS));
    rewritten.grade = Some("tty".into());
    let rewritten = verified(&a, &rewritten, "r1");
    if refusal(&rewritten) != "not_the_decider" {
        wrong.push(format!("ALTERED: a relay rewrote the grade to the certificate's: {rewritten:?}"));
    }
    let mut stripped = tty_key.sign(tty_principal, &node, "r1", "issue", Some(600), None, Some("tty"));
    stripped.grade = None;
    if refusal(&verified(&a, &stripped, "r1")) != "decision_grade_missing" {
        wrong.push("STRIPPED: a consent body with its grade removed".into());
    }

    // A service decider: `service` or nothing; never a consent grade.
    let now = mycelium_core::sim_seam::wall_now_ms();
    let by_service = |grade: Option<&str>| crate::sign_grant_decision_graded(&treasurer, DECIDER, &node, FORECASTER, "r1", "issue", Some(600), None, now, grade);
    // Since I7-S a service's issue names what it saw: a body naming nothing — pre-I7, or naming only `service` — is
    // refused by default (D2), and one naming the block under `service` is recorded `service`.
    for (what, grade) in [("a pre-I7 body", None), ("naming service, and nothing seen", Some("service"))] {
        let got = verified(&a, &by_service(grade), "r1");
        if refusal(&got) != "decision_names_nothing" {
            wrong.push(format!("SERVICE: {what} is recorded {got:?}"));
        }
    }
    let shown = crate::Shown::new(TOOL, format!("tool:{TOOL}@{node}"), "00", u64::MAX);
    let named = crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, FORECASTER, "r1", "issue", Some(600), None, now, &shown);
    if verified(&a, &named, "r1") != Ok(SignedWith::service()) {
        wrong.push(format!("SERVICE: naming the block is recorded {:?}", verified(&a, &named, "r1")));
    }
    for g in grades {
        let got = verified(&a, &by_service(Some(g.as_str())), "r1");
        if refusal(&got) != "decision_grade_mismatch" {
            wrong.push(format!("SERVICE CLAIMS {g}: {got:?}"));
        }
    }
    // The grade stripped after signing: the signature no longer verifies, and the door refuses the block left
    // without a grade before it reads the key.
    let mut stripped = named.clone();
    stripped.grade = None;
    if stripped.verify_signature("issue", Some(600), None) || refusal(&verified(&a, &stripped, "r1")) != "decision_shown_ungraded" {
        wrong.push(format!("STRIPPED: a service's `service` grade removed after signing: {:?}", verified(&a, &stripped, "r1")));
    }

    // The service's own, current Ed25519 key under the consent principal of the same name.
    wrong.extend(key_of_a_service_as_consent(&a, &dir, &node));
    // A P-256 consent key under a service principal.
    let as_service = verified(&a, &tty_key.sign(DECIDER, &node, "r1", "issue", Some(600), None, Some("service")), "r1");
    if !as_service.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("is not a consent principal")) {
        wrong.push(format!("P-256 AS A SERVICE: {as_service:?}"));
    }
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wrong.is_empty(), "GRADE:\n{}", wrong.join("\n"));
}

/// Exit 5's last item: one person, `token:fleet/dana` with a current Ed25519 client certificate and
/// `consent:fleet/dana` with a current consent certificate. The Ed25519 key signs as the consent principal, naming
/// the consent certificate's grade: refused by name.
fn key_of_a_service_as_consent(a: &GossipAgent, dir: &Path, node: &str) -> Option<String> {
    let service = client_key_in(a, &dir.join("tls"), &dir.join("client-dana"), "dana");
    ConsentKey::new().enrol(a, dir, "dana", "consent-dana", ConsentGrade::Tty);
    // Naming a block (I7-S), so the key's kind is what refuses it.
    let shown = crate::Shown::new(TOOL, format!("tool:{TOOL}@{node}"), "00", u64::MAX);
    let msg = crate::grant_decision_message_shown("consent:fleet/dana", node, FORECASTER, "r1", "issue", Some(600), None, 1, Some("tty"), Some(&shown));
    let b64 = |b: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b);
    let forged = crate::SignedDecision::from_parts("consent:fleet/dana", node, FORECASTER, "r1", 1, b64(&service.verifying_key().to_bytes()), b64(&ed25519_dalek::Signer::sign(&service, &msg).to_bytes()), Some("tty")).naming(shown);
    let got = verified(a, &forged, "r1");
    (!got.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("never a consent key"))).then(|| format!("SERVICE KEY AS CONSENT: {got:?}"))
}

/// **Consent has its own authority** (I7, the maintainer's decision A; exit 14). One person, Richard: a service
/// principal `token:fleet/richard` (a named token and a client certificate) and a consent principal
/// `consent:fleet/richard` (a consent certificate). The unit permits each to issue, so the **door** decides. Over
/// the four configurations — `grant:issue` in the consent table or not, on the named token or not — a principal's
/// signed issue is admitted exactly when **its own** table grants it: the consent decider with the table and a
/// scopeless token, refused `decider_lacks_scope` without the table whatever the token holds; the service decider
/// the reverse. Removing either entry revokes that principal alone. A consent entry with empty scopes grants
/// nothing.
///
/// Seen failing first: the consent decider matched against the named-token table (it decided with the token's scope
/// and without its own entry).
#[tokio::test]
async fn a_consent_principal_decides_by_its_own_table_and_never_by_a_named_tokens() {
    let unit = unit_with(&format!("{}{}", decides(RICHARD, ""), decides(RICHARD_SERVICE, "")));
    let mut wrong = Vec::new();
    for (in_consent_table, on_token) in [(true, true), (true, false), (false, true), (false, false)] {
        let dir = scratch(&format!("consent-table-{in_consent_table}-{on_token}"));
        let http_port = alloc_port();
        let a = started_with(&dir, http_port, None, &unit, |cfg| {
            let scopes = if on_token { vec!["grant:read".to_string(), "grant:issue".to_string()] } else { vec!["grant:read".to_string()] };
            cfg.gateway_named_tokens.push(GatewayNamedToken { name: "richard".into(), token: "richard-secret".into(), scopes });
            // An entry with no scope stands for "listed, granting nothing"; no entry at all is the removal.
            cfg.gateway_consent_principals = if in_consent_table { consent_table(&["richard"]) } else if on_token { vec![] } else { {
                let mut listed = crate::config::GatewayConsentPrincipal::new("richard", "tty");
                listed.scopes = vec![];
                vec![listed]
            } };
        })
        .await;
        let _ = forecaster_key(&a, &dir);
        let service = client_key_in(&a, &dir.join("tls"), &dir.join("client-richard"), "richard");
        let consent = ConsentKey::new();
        consent.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
        let node = a.node_id().to_string();
        let requests = ask(http_port, 2).await;
        let by_consent = post_decision(http_port, "relay-secret", &requests[0], issue_body(consent.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
        let by_service = post_decision(http_port, "relay-secret", &requests[1],
            issue_body(signed_by(&service, RICHARD_SERVICE, &node, &requests[1], "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms()))).await;
        a.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
        let want = |granted: bool| if granted { (200, None) } else { (403, Some("decider_lacks_scope".to_string())) };
        if code(&by_consent) != want(in_consent_table) {
            wrong.push(format!("consent table {in_consent_table}, token {on_token}: the CONSENT decider answered {} {}", by_consent.0, by_consent.1));
        }
        if code(&by_service) != want(on_token) {
            wrong.push(format!("consent table {in_consent_table}, token {on_token}: the SERVICE decider answered {} {}", by_service.0, by_service.1));
        }
    }
    assert!(wrong.is_empty(), "AUTHORITY:\n{}", wrong.join("\n"));
}

/// **A policy keyed on `token:{issuer}/{name}` does not admit the `consent:` decider** (I7 exit 14, the upgrade
/// note, shown). The unit permits `token:fleet/richard` to issue and names no `consent:` principal;
/// `consent:fleet/richard` holds `grant:issue` at the door. Its signed issue passes the door and is refused by the
/// evaluator — authority not established — with the **consent principal** as the record's subject and the grade it
/// was evaluated with on the record; no grant is written. The same unit with a rule for the consent principal admits
/// it.
///
/// Seen failing first with the envelope's actor rewritten from `consent:` to `token:` (the service's rule admitted
/// the consent decider).
#[tokio::test]
async fn a_rule_for_the_service_principal_does_not_admit_the_consent_principal() {
    let mut answers = Vec::new();
    for with_consent_rule in [false, true] {
        let dir = scratch(&format!("consent-upgrade-{with_consent_rule}"));
        let http_port = alloc_port();
        let rules = if with_consent_rule { format!("{}{}", decides(RICHARD_SERVICE, ""), decides(RICHARD, "")) } else { decides(RICHARD_SERVICE, "") };
        let a = started_with(&dir, http_port, None, &unit_with(&rules), |cfg| cfg.gateway_consent_principals = consent_table(&["richard"])).await;
        let _ = forecaster_key(&a, &dir);
        let consent = ConsentKey::new();
        consent.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
        let node = a.node_id().to_string();
        let requests = ask(http_port, 1).await;
        let answer = post_decision(http_port, "relay-secret", &requests[0], issue_body(consent.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
        let issued = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{}/issued", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), requests[0])).is_some();
        a.shutdown().await;
        answers.push((answer, issued, journal_of(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }
    let (answer, issued, journal) = &answers[0];
    let door_codes = ["not_the_decider", "decider_lacks_scope", "decision_grade_mismatch", "decision_grade_missing", "consent_grade_oid_provisional"];
    assert_eq!(answer.0, 403, "UPGRADE NOTE: a token: rule admitted the consent decider: {}", answer.1);
    assert!(!door_codes.contains(&answer.1["error"].as_str().unwrap_or_default()), "POLICY: a door refusal, not the evaluator's: {}", answer.1);
    assert!(!issued, "no grant was written");
    let refused = decided(journal, "grant.issue", RICHARD);
    assert_eq!(refused.len(), 1, "SUBJECT: the consent principal is the actor refused: {:?}", journal.iter().map(|r| (&r.operation, &r.subject)).collect::<Vec<_>>());
    assert_eq!(refused[0].decision, DecisionKind::Indeterminate);
    assert_eq!(refused[0].signed_decision.clone().unwrap_or_default()["signed_with"]["grade"], "tty", "the refusal's record names the grade it was evaluated with");
    assert!(decided(journal, "grant.issue", RICHARD_SERVICE).is_empty(), "the service principal decided nothing");
    assert_eq!((answers[1].0 .0, answers[1].1), (200, true), "with its own rule the consent principal decides: {}", answers[1].0 .1);
}

/// **The grade at the evaluator** (I7 exit 15). The unit permits an issue only when it is signed with
/// `os_presence(macos, unbound)` — a **set** of one accepted grade — on each decider's rule. The same issue is
/// refused signed at `tty` (Dana's consent key, certified `tty` and accepted at `tty` by the gateway's table) and at
/// `service` (the treasurer's Ed25519 key), and admitted at `os_presence(macos, unbound)` (Richard's consent key,
/// certified and accepted so). All three pass the door: the evaluator is what refuses. Each evidence record names the
/// grade it decided with, and the permit's `checked` names the clause: the grade and that it is the operator's
/// statement.
///
/// Seen failing first with the rule's grade clause not read in `matches` (the `tty` and `service` decisions were
/// admitted).
#[tokio::test]
async fn a_policy_accepting_one_grade_refuses_the_same_decision_at_any_other() {
    const DANA: &str = "consent:fleet/dana";
    let dir = scratch("consent-policy");
    let http_port = alloc_port();
    let accept = format!("signed_with = [\"{OS}\"]");
    // The base unit's unconditional treasurer rules are replaced: the treasurer too issues only at the accepted grade.
    let base = UNIT.split("[[rule]]").take(2).collect::<Vec<_>>().join("[[rule]]");
    let unit = format!("{base}\n{}{}{}", decides(RICHARD, &accept), decides(DANA, &accept), decides(DECIDER, &accept));
    let a = started_with(&dir, http_port, None, &unit, |cfg| {
        cfg.gateway_consent_principals = vec![crate::config::GatewayConsentPrincipal::new("richard", OS), crate::config::GatewayConsentPrincipal::new("dana", "tty")];
    })
    .await;
    let _ = forecaster_key(&a, &dir);
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let node = a.node_id().to_string();
    let requests = ask(http_port, 3).await;
    let tty = ConsentKey::new();
    tty.enrol(&a, &dir, "dana", "consent-tty", ConsentGrade::Tty);
    let at_tty = post_decision(http_port, "relay-secret", &requests[0], issue_body(tty.sign(DANA, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
    let at_service = post_decision(http_port, "relay-secret", &requests[1],
        issue_body(signed_at(&treasurer, &node, &requests[1], "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms()))).await;
    let os = ConsentKey::new();
    os.enrol(&a, &dir, "richard", "consent-os", ConsentGrade::OsPresenceMacosUnbound);
    let at_os = post_decision(http_port, "relay-secret", &requests[2], issue_body(os.sign(RICHARD, &node, &requests[2], "issue", Some(600), None, Some(OS)))).await;
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(at_tty.0, 403, "TTY: admitted by a rule accepting only {OS}: {}", at_tty.1);
    assert_eq!(at_service.0, 403, "SERVICE: admitted by a rule accepting only {OS}: {}", at_service.1);
    assert_eq!(at_os.0, 200, "OS_PRESENCE: {}", at_os.1);
    let grade_of = |r: &AeEvidence| r.signed_decision.clone().unwrap_or_default()["signed_with"]["grade"].as_str().unwrap_or_default().to_string();
    let by = |who: &str| -> Vec<(DecisionKind, String)> { decided(&journal, "grant.issue", who).iter().map(|r| (r.decision, grade_of(r))).collect() };
    assert_eq!(by(DANA), vec![(DecisionKind::Indeterminate, "tty".to_string())], "EVIDENCE: the evaluator, not the door, refused the tty decision, and its record names the grade");
    assert_eq!(by(RICHARD), vec![(DecisionKind::Permit, OS.to_string())]);
    assert_eq!(by(DECIDER), vec![(DecisionKind::Indeterminate, "service".to_string())]);
    let permit = journal.iter().find(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue" && r.decision == DecisionKind::Permit).expect("the permit");
    let checked = serde_json::to_string(permit).unwrap();
    assert!(checked.contains(&format!("signed_with grade={OS} key=p256 source=operator")), "CHECKED: the permit names the grade clause: {checked}");
}

/// **Re-enrolment at the decision** (I7 exit 11, the decision half; P6). Richard's consent key K1, then a newer
/// consent certificate over K2: a decision signed by K1 is refused `not_the_decider` (a key held before a newer
/// certificate) and K2's is admitted. After a restart under `[persistence]` — K2's record tombstoned first, so only
/// the journal remembers it — K1 is still refused as superseded, and K2, whose certificate is no longer on record,
/// is refused too: its grade cannot be read, and absence fails closed — counted, and readable on `GET /stats`
/// (`consent_grades_unread`). Republished, K2 decides again.
///
/// Seen failing first with a consent key's standing read from its own record alone and the mark's certificate not
/// compared (K1 was admitted); and with the counters left off `/stats` (`STATS: … null`).
#[tokio::test]
async fn a_superseded_consent_key_decides_nothing_before_or_after_a_restart() {
    let dir = scratch("consent-reenrol");
    let (http_port, port) = (alloc_port(), alloc_port());
    let unit = unit_with(&decides(RICHARD, ""));
    let table = |cfg: &mut GossipConfig| cfg.gateway_consent_principals = consent_table(&["richard"]);
    let (k1, k2) = (ConsentKey::new(), ConsentKey::new());
    let node = NodeId::new("127.0.0.1", port).unwrap().to_string();
    let by = |key: &ConsentKey| key.sign(RICHARD, &node, "r1", "issue", Some(600), None, Some("tty"));
    let k2_der = {
        let a = started_with(&dir, http_port, Some(port), &unit, table).await;
        k1.enrol(&a, &dir, "richard", "consent-k1", ConsentGrade::Tty);
        assert_eq!(verified(&a, &by(&k1), "r1"), Ok(SignedWith::consent("tty")), "K1 decides while it is current");
        tokio::time::sleep(Duration::from_millis(20)).await;
        // K2's certificate arrives as any member's write, so this gateway holds no copy to republish.
        let k2_der = k2.certificate(&dir, "richard", "consent-k2", ConsentGrade::Tty);
        let k2_path: Arc<str> = Arc::from(crate::consent_cert_key(RICHARD, &k2.point).as_str());
        let _ = crate::agent::helpers::kv_set(&a.task_ctx, Arc::clone(&k2_path), bytes::Bytes::from(k2_der.clone()));
        let older = verified(&a, &by(&k1), "r1");
        assert!(older.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("newer certificate")), "SUPERSEDED: K1 after K2's certificate: {older:?}");
        assert_eq!(verified(&a, &by(&k2), "r1"), Ok(SignedWith::consent("tty")), "K2 decides");
        let _ = crate::agent::helpers::kv_delete(&a.task_ctx, k2_path);
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.shutdown().await;
        k2_der
    };
    let b = started_with(&dir, http_port, Some(port), &unit, table).await;
    let older = verified(&b, &by(&k1), "r1");
    let newer_absent = verified(&b, &by(&k2), "r1");
    let unread = b.task_ctx.client_certs.grade_unread.load(Ordering::Relaxed);
    // The same count where an operator can read it (#810's review, finding 4).
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    let _ = crate::agent::helpers::kv_set(&b.task_ctx, Arc::from(crate::consent_cert_key(RICHARD, &k2.point).as_str()), bytes::Bytes::from(k2_der));
    let newer_back = verified(&b, &by(&k2), "r1");
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(older.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("newer certificate")), "RESTORED: K1 after a restart: {older:?}");
    assert_eq!(b.consent_key_standing(RICHARD, &k2.point), ClientKeyStanding::Current);
    assert!(newer_absent.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("grade cannot be read")), "ABSENT: K2 with its certificate gone: {newer_absent:?}");
    assert_eq!(unread, 1, "COUNTED: the unread grade");
    assert_eq!((stats["consent_grades_unread"].as_u64(), stats["consent_certificates_provisional"].as_u64()), (Some(1), Some(0)), "STATS: the unread grade on GET /stats: {stats}");
    assert_eq!(newer_back, Ok(SignedWith::consent("tty")), "K2 republished decides again");
}

/// **A member cannot change a consent key's grade by writing an older certificate back** (the owner-signed rule,
/// for `sys/client-cert/`'s grade; I7, H2). One key, two certificates from the fleet CA: first certified
/// `os_presence(macos, unbound)`, then re-certified `tty` — the operator's newer statement. Both sit at the **same**
/// path (the path names the principal and the key), so the newer replaces the older there, and any admitted member
/// can write the older one back: it is a genuine certificate for that key. With the older one back at the path, the
/// key is still current (the mark holds the newer issuance) and its grade is the newer certificate's, which is no
/// longer on record: a decision naming `os_presence(macos, unbound)` — the grade the replayed certificate would give
/// — and one naming `tty` are both refused, and the unread grade is counted. With the newer certificate written
/// again, `tty` decides and the old grade is a mismatch.
///
/// Seen failing first with the grade read from whatever verified certificate sits at the key's path (the decision
/// naming `os_presence(macos, unbound)` was admitted and recorded with it).
#[tokio::test]
async fn an_older_certificate_written_back_does_not_change_a_consent_keys_grade() {
    let dir = scratch("consent-forged-grade");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, UNIT, |_| {}).await;
    let node = a.node_id().to_string();
    let key = ConsentKey::new();
    let path: Arc<str> = Arc::from(crate::consent_cert_key(RICHARD, &key.point).as_str());
    let write = |der: &[u8]| {
        let _ = crate::agent::helpers::kv_set(&a.task_ctx, Arc::clone(&path), bytes::Bytes::copy_from_slice(der));
    };
    let by = |grade: &str| key.sign(RICHARD, &node, "r1", "issue", Some(600), None, Some(grade));
    let older = key.certificate(&dir, "richard", "consent-older", ConsentGrade::OsPresenceMacosUnbound);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let newer = key.certificate(&dir, "richard", "consent-newer", ConsentGrade::Tty);
    write(&older);
    assert_eq!(verified(&a, &by(OS), "r1"), Ok(SignedWith::consent(OS)), "the first certificate's grade, while it is the newest");
    write(&newer);
    assert_eq!(verified(&a, &by("tty"), "r1"), Ok(SignedWith::consent("tty")), "the re-certified grade");
    assert_eq!(refusal(&verified(&a, &by(OS), "r1")), "decision_grade_mismatch");

    write(&older);
    let unread_before = a.task_ctx.client_certs.grade_unread.load(Ordering::Relaxed);
    let replayed_grade = verified(&a, &by(OS), "r1");
    let current_grade = verified(&a, &by("tty"), "r1");
    let unread = a.task_ctx.client_certs.grade_unread.load(Ordering::Relaxed) - unread_before;
    assert_eq!(a.consent_key_standing(RICHARD, &key.point), ClientKeyStanding::Current, "the key itself is still the principal's");
    assert!(replayed_grade.as_ref().is_err_and(|e| e.starts_with("not_the_decider") && e.contains("grade cannot be read")), "REPLAYED: an older certificate gave the key its old grade: {replayed_grade:?}");
    assert!(current_grade.as_ref().is_err_and(|e| e.contains("grade cannot be read")), "ABSENT: the current grade with its certificate off the record: {current_grade:?}");
    assert_eq!(unread, 2, "COUNTED: each unread grade");

    write(&newer);
    assert_eq!(verified(&a, &by("tty"), "r1"), Ok(SignedWith::consent("tty")), "the newer certificate back at its path");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A shipped build judges no consent decision** (I7; #800's review, finding 1, at the decision). With the
/// provisional gate closed — as in a build without `cfg(test)` or `provisional-consent-grade` — a consent decider's
/// valid decision is refused `consent_grade_oid_provisional` before its key is read, a P-256 key under a service
/// principal likewise, the signature does not re-verify (`SignedDecision::verify_signature`), and `start()`'s
/// preflight refuses a `gateway_consent_principals` table by name. The same decision with the gate open is admitted
/// (the plant).
///
/// Seen failing first, one build per plant: `verify`'s two gate checks removed (the closed gate answered
/// `not_the_decider`, by the fold reading no certificate, not by name); `verify_consent_p256` ungated (the signature
/// re-verified with the gate closed); the preflight's refusal removed (the table was accepted).
#[tokio::test]
async fn with_the_provisional_gate_closed_a_consent_decision_is_refused_by_name_and_nothing_verifies() {
    let dir = scratch("consent-closed");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, UNIT, |cfg| cfg.gateway_consent_principals = consent_table(&["richard"])).await;
    let node = a.node_id().to_string();
    let key = ConsentKey::new();
    key.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
    let signed = key.sign(RICHARD, &node, "r1", "issue", Some(600), None, Some("tty"));
    let as_service = key.sign(DECIDER, &node, "r1", "issue", Some(600), None, Some("service"));
    assert_eq!(verified(&a, &signed, "r1"), Ok(SignedWith::consent("tty")), "plant: admitted with the gate open");
    assert!(signed.verify_signature("issue", Some(600), None), "plant: re-verifies with the gate open");
    let (closed, closed_as_service, reverified, preflight) = mycelium_core::tls::with_provisional_consent_grade_closed(|| {
        (verified(&a, &signed, "r1"), verified(&a, &as_service, "r1"), signed.verify_signature("issue", Some(600), None), a.preflight())
    });
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let mut wrong = Vec::new();
    if refusal(&closed) != "consent_grade_oid_provisional" {
        wrong.push(format!("JUDGED: a consent decision with the gate closed, not refused by name: {closed:?}"));
    }
    if refusal(&closed_as_service) != "consent_grade_oid_provisional" {
        wrong.push(format!("JUDGED: a P-256 key under a service principal with the gate closed: {closed_as_service:?}"));
    }
    if reverified {
        wrong.push("RE-VERIFIED: a consent signature with the gate closed".into());
    }
    if !matches!(&preflight, Err(crate::GossipError::InvalidField { field: "gateway_consent_principals", .. })) {
        wrong.push(format!("ACCEPTED: the consent table with the gate closed: {preflight:?}"));
    }
    assert!(wrong.is_empty(), "CLOSED GATE:\n{}", wrong.join("\n"));
}

/// **A consent key is no one's Ed25519 key** (I7 exit 8, the rest: a `cs` signer, a possession proof, a provider's
/// caller, a configured issuer, a removal's authority, `ProviderBudget`). With a consent certificate published on a
/// node whose execution authority's key set is fed, and a signature the consent key really made:
/// - **`cs`** (the door's and the provider's caller check, `client_sig::check`): a `cs` carrying the point is
///   malformed; one carrying the 32 bytes the fold knows the key by (the SHA-256 of the point) is refused;
/// - **possession** (`verify_client`, the reader `GrantVerifier::check`, the preflight and `reassess_arguments`
///   share): the consent principal holds no key in the fed set, and the consent signature proves nothing;
/// - **a configured issuer** (`verify_signed_by`): the consent principal is not one, and its signature is not
///   authentic — also when the set was (wrongly) fed its 32-byte id;
/// - **a removal's authority** (`MembershipAuthorities::verify`): a removal the consent key signed, for a consent
///   principal listed as an authority, does not verify;
/// - **`ProviderBudget`**: a cohort declaration the consent key signed, from a consent principal the budget is told
///   to trust, is unverifiable, and the set it reads holds no consent key.
///
/// No type here is widened (each takes an Ed25519 key of 32 bytes and a signature of 64), so these hold by
/// construction as well as by test. Seen failing first, the one plant that reaches them: `feed_client_keys` feeding
/// every principal (the consent principal then had a key in the fed set — possession's and the budget's assertions).
#[tokio::test]
async fn a_consent_key_is_refused_or_ignored_wherever_an_ed25519_key_is_read() {
    use crate::knowledge::issuer::{verify_signed_by, Authenticity, SharedExternalIssuers, TrustedExternalIssuers};
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let dir = scratch("consent-not-reached");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, UNIT, |_| {}).await;
    let fed = SharedExternalIssuers::from(TrustedExternalIssuers::new());
    a.task_ctx.client_key_sinks.pin().insert(fed.id(), fed.clone());
    // The budget shares the fed set and takes cohort declarations from the consent principal by name.
    let budget = crate::agent::provider_enforcement::ProviderBudget::new(
        crate::knowledge::cohort_budget::CohortBudget::new(3, 1),
        crate::knowledge::cohort::CohortView::trusting([crate::knowledge::IssuerId::new(RICHARD).unwrap()]),
        fed.clone(),
    );
    let key = ConsentKey::new();
    key.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
    let consent = crate::knowledge::IssuerId::new(RICHARD).unwrap();
    let id: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(key.point).into();
    assert_eq!(a.consent_key_standing(RICHARD, &key.point), ClientKeyStanding::Current);

    // `cs`: the point, and the fold's 32-byte id, as a client signature's key.
    let cn = crate::ClientNonce { t: mycelium_core::sim_seam::wall_now_ms(), i: "00000000000000aa".into(), c: 1 };
    let payload = serde_json::to_vec(&serde_json::json!({"name": TOOL, "arguments": {}})).unwrap();
    let sig = mycelium_core::tls::sign_consent_p256_in_test_build(&key.pkcs8, b"anything").unwrap();
    let cs_point = crate::ClientSignature { k: b64.encode(key.point), sig: b64.encode(&sig) };
    let cs_id = crate::ClientSignature { k: b64.encode(id), sig: b64.encode([0u8; 64]) };
    for (what, cs) in [("the point", &cs_point), ("the fold's id", &cs_id)] {
        let got = crate::agent::client_sig::check(&a.task_ctx, RICHARD, "tools/call", &payload, None, cs, &cn);
        assert!(got.is_err(), "CS: a consent key signed a call ({what}): {got:?}");
    }
    // Possession: no key fed, and nothing verifies.
    assert_eq!(fed.read().client_key(&consent), None, "FED: a consent principal's key reached the possession set");
    assert!(!fed.read().verify_client(&consent, b"anything", &sig), "POSSESSION: a consent signature proved possession");
    // A configured issuer: not one, with the set as fed and with the id fed by hand.
    let members: std::collections::HashMap<NodeId, crate::knowledge::issuer::MemberKeys> = std::collections::HashMap::new();
    assert!(!matches!(verify_signed_by(&consent, b"anything", &sig, &members, &fed.read().clone()), Authenticity::Current { .. }), "ISSUER: a consent key as a configured issuer");
    let by_hand = SharedExternalIssuers::from(TrustedExternalIssuers::new());
    by_hand.set_client_key(consent.clone(), 1, id).unwrap();
    assert!(!matches!(verify_signed_by(&consent, b"anything", &sig, &members, &by_hand.read().clone()), Authenticity::Current { .. }), "ISSUER: the fold's id read as an issuer's key");
    // A removal's authority.
    let removal = crate::membership::MemberRemoval { authority: consent.clone(), node: a.node_id().clone(), keys: vec![[7u8; 32]], seq: 1, issued_at_ms: 1_000, reason: "x".into() };
    let signed = crate::membership::SignedMemberRemoval { signature: mycelium_core::tls::sign_consent_p256_in_test_build(&key.pkcs8, &removal.canonical_bytes()).unwrap(), removal };
    let authorities = crate::membership::MembershipAuthorities::new([consent.clone()], fed.clone());
    assert!(authorities.verify(&signed, &members).is_err(), "REMOVAL: a consent key entitled a removal");
    // ProviderBudget: a cohort declaration the consent key signed, from the consent principal it is told to trust.
    crate::agent::client_cert::refresh_through_for_test(&a.task_ctx);
    let declaration = crate::knowledge::cohort::CohortDeclaration { operator: consent.clone(), cohort: "night".into(), seq: 1, members: Default::default(), valid_from_ms: 0, valid_until_ms: u64::MAX };
    let signed = crate::knowledge::cohort::SignedCohortDeclaration { signature: mycelium_core::tls::sign_consent_p256_in_test_build(&key.pkcs8, &declaration.canonical_bytes()).unwrap(), declaration };
    let offer = budget.offer(&signed, &members);
    assert!(matches!(offer, crate::knowledge::cohort::CohortOffer::Unverifiable(_)), "BUDGET: a consent key's cohort declaration: {offer:?}");
    assert_eq!(fed.read().client_key(&consent), None, "BUDGET: a consent principal's key in the set ProviderBudget reads");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Only the grant decision routes can put a grade on an envelope** (#810's review, finding 5). A companion door
/// calls the public `gateway_preflight`, names its enforcement point `gateway:grants/issue` — the grant routes' own —
/// and passes its client's `params`, which claim a verified decision signed at `os_presence(macos, unbound)`. The
/// unit permits the treasurer to issue only at that grade. The preflight refuses: the envelope carries no
/// `signed_with`, and the evidence record no `signed_decision`, because both travel from `http/grants.rs` in a typed
/// argument and are never read from `params`.
///
/// Seen failing first with the envelope's `signed_with` read from `params` at that enforcement point's name, as it
/// was (the call proceeded: *an allowance matches this action*).
#[tokio::test]
async fn a_door_naming_the_grant_point_cannot_put_a_grade_on_the_envelope() {
    let dir = scratch("consent-params");
    let http_port = alloc_port();
    let base = UNIT.split("[[rule]]").take(2).collect::<Vec<_>>().join("[[rule]]");
    let unit = format!("{base}\n{}", decides(DECIDER, &format!("signed_with = [\"{OS}\"]")));
    let a = started_with(&dir, http_port, None, &unit, |_| {}).await;
    let node = a.node_id().to_string();
    let caller = crate::agent::gateway_caller::ResolvedPrincipal { principal: DECIDER.into(), scopes: vec!["grant:issue".into()], subject_token: None };
    let claimed = serde_json::json!({"key": "p256", "grade": OS, "grade_source": "operator"});
    let params = serde_json::json!({"_meta": {"signed_decision": {"signed": {"decider": DECIDER}, "signed_with": claimed, "decision": "issue", "relayed_by": "token:fleet/relay"}}});
    let mut answers = Vec::new();
    for resource in ["tool:weather.forecast".to_string(), format!("tool:weather.forecast@{node}")] {
        let p = a.gateway_preflight(Some(&caller), "grant.issue", &resource, &serde_json::json!({}), &params, crate::agent::http::grants::ENFORCEMENT_POINT_GRANT_ISSUE).await;
        answers.push(if p.refusal().is_some() { "refuse" } else { "proceed" }.to_string());
    }
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(answers.iter().all(|a| a.starts_with("refuse")), "ADMITTED: a client's params put a grade on the envelope: {answers:?}");
    let records = decided(&journal, "grant.issue", DECIDER);
    assert_eq!(records.len(), 2, "{:?}", journal.iter().map(|r| (&r.operation, &r.subject)).collect::<Vec<_>>());
    assert!(records.iter().all(|r| r.signed_decision.is_none()), "RECORDED: a client's `_meta.signed_decision` on the evidence record: {:?}", records.iter().map(|r| &r.signed_decision).collect::<Vec<_>>());
}

/// **A consent principal never holds a grant** (I7, *not built*: a consent key as a grant holder; #810's review). A
/// consent key proves no possession, so a grant issued to `consent:{issuer}/{name}` could never be presented. With
/// the consent principal's certificate on record — so the ledger holds a mark for it, which is what the door asks of
/// a client holder — an issue, a refusal and a signed issue for a request under that holder are each answered `409
/// consent_holder`, by name, and nothing is written.
///
/// Seen failing first with the refusal removed (the door went on to look for the request: `404 unknown_request`).
#[tokio::test]
async fn a_grant_is_never_issued_to_a_consent_principal() {
    let dir = scratch("consent-holder");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, UNIT, |_| {}).await;
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    ConsentKey::new().enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
    assert!(a.task_ctx.client_certs.mark(RICHARD).is_some(), "the ledger holds the consent principal's mark");
    let node = a.node_id().to_string();
    let path = format!("/gateway/grants/{}/r1", segment(RICHARD));
    // Each names a block (I7-S), so nothing but the holder refuses it.
    let shown = serde_json::to_value(crate::Shown::new(TOOL, format!("tool:{TOOL}@{node}"), "00", u64::MAX)).unwrap();
    let issue = http(http_port, "treasurer-secret", reqwest::Method::POST, &path, Some(serde_json::json!({"decision": "issue", "ttl_secs": 60, "shown": shown}))).await;
    let refuse = http(http_port, "treasurer-secret", reqwest::Method::POST, &path, Some(serde_json::json!({"decision": "refuse", "reason": "no", "shown": shown}))).await;
    let signed = crate::sign_grant_decision_naming(&treasurer, DECIDER, &node, RICHARD, "r1", "issue", Some(60), None, mycelium_core::sim_seam::wall_now_ms(), &serde_json::from_value(shown.clone()).unwrap());
    let signed = http(http_port, "relay-secret", reqwest::Method::POST, &path, Some(serde_json::json!({"decision": "issue", "ttl_secs": 60, "signed": signed}))).await;
    let written = mycelium_core::store::scan_kv_prefix(&a.task_ctx.kv_state, &format!("grant/{}/", crate::agent::gateway_authority::grant_holder_segment(RICHARD))).len();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    for (what, answer) in [("issue", &issue), ("refuse", &refuse), ("signed issue", &signed)] {
        assert_eq!(code(answer), (409, Some("consent_holder".into())), "CONSENT HOLDER ({what}): {}", answer.1);
    }
    assert_eq!(written, 0, "nothing was written under the consent holder");
}

/// **The grade is bound outside the KV: a node never relies on which certificate reached it first** (the maintainer's
/// decision of 2026-11-17; #810's review, finding 1). A decision is admitted only when three statements agree — the
/// grade the signed body names, the grade of the certificate this node read, and the grade the gateway's own
/// `gateway_consent_principals` entry accepts.
///
/// The reviewer's scenario. A key is certified `os_presence(macos, unbound)`; the operator then re-certifies it `tty`
/// and writes `signed_with = "tty"` in the gateway's table. A **freshly joined node, its persistence directory
/// empty**, holds no mark for the principal, and the only certificate at the key's path is the older, genuine one —
/// a member wrote it back, and the newer one never arrived. To this node the older certificate is the newest it has
/// read, so the certificate alone would give the key its old grade.
/// - A decision naming `os_presence(macos, unbound)` — the grade the replayed certificate carries — is refused `403
///   decision_grade_not_accepted`: the body and the certificate agree, the operator's entry does not. No grant is
///   written, it is counted, and `GET /stats` shows it (`consent_grades_not_accepted`).
/// - A decision naming `tty` is refused `decision_grade_mismatch`: the certificate on record carries another grade.
///   Neither grade decides until the newer certificate arrives.
/// - Once the newer certificate is on record, `tty` — body, certificate and entry agreeing — is admitted, and the old
///   grade is refused.
///
/// The other configuration error it catches: an entry not updated after a re-certification (the table still says
/// `os_presence(macos, unbound)`, the newest certificate `tty`) refuses the `tty` decision by the same name.
///
/// Seen failing first with the door's comparison against the entry removed — this PR's code before the decision: the
/// decision naming the replayed grade was admitted (`200`, a grant written, recorded `{p256, os_presence(macos,
/// unbound), operator}`).
#[tokio::test]
async fn a_node_that_never_read_the_newer_certificate_does_not_take_the_older_ones_grade() {
    let dir = scratch("consent-first-seen");
    let http_port = alloc_port();
    let unit = unit_with(&decides(RICHARD, ""));
    // The operator's table at this gateway: the grade the key was re-certified at.
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table_at(&["richard"], "tty")).await;
    let _ = forecaster_key(&a, &dir);
    let node = a.node_id().to_string();
    let key = ConsentKey::new();
    let older = key.certificate(&dir, "richard", "consent-older", ConsentGrade::OsPresenceMacosUnbound);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let newer = key.certificate(&dir, "richard", "consent-newer", ConsentGrade::Tty);
    let path: Arc<str> = Arc::from(crate::consent_cert_key(RICHARD, &key.point).as_str());
    let write = |der: &[u8]| {
        let _ = crate::agent::helpers::kv_set(&a.task_ctx, Arc::clone(&path), bytes::Bytes::copy_from_slice(der));
    };
    let requests = ask(http_port, 3).await;
    let issued = |request: &str| mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{request}/issued", crate::agent::gateway_authority::grant_holder_segment(FORECASTER))).is_some();

    // This node has never read the newer certificate: no mark, and the older one at the path.
    assert!(a.task_ctx.client_certs.mark(RICHARD).is_none(), "a fresh node holds no mark for the principal");
    write(&older);
    let replayed = post_decision(http_port, "relay-secret", &requests[0], issue_body(key.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some(OS)))).await;
    let replayed_issued = issued(&requests[0]);
    let current = post_decision(http_port, "relay-secret", &requests[0], issue_body(key.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;

    // The newer certificate arrives: the three statements agree on `tty`.
    write(&newer);
    let old_grade = post_decision(http_port, "relay-secret", &requests[1], issue_body(key.sign(RICHARD, &node, &requests[1], "issue", Some(600), None, Some(OS)))).await;
    let agreed = post_decision(http_port, "relay-secret", &requests[1], issue_body(key.sign(RICHARD, &node, &requests[1], "issue", Some(600), None, Some("tty")))).await;
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(code(&replayed), (403, Some("decision_grade_not_accepted".into())), "FIRST SEEN: an older certificate gave the key its old grade at a node that never read the newer one: {}", replayed.1);
    assert!(!replayed_issued, "FIRST SEEN: a grant was written under the replayed grade");
    assert_eq!(code(&current), (403, Some("decision_grade_mismatch".into())), "the re-certified grade, with only the older certificate on record: {}", current.1);
    assert_eq!(stats["consent_grades_not_accepted"].as_u64(), Some(1), "COUNTED: on GET /stats: {stats}");
    assert_eq!(code(&old_grade), (403, Some("decision_grade_mismatch".into())), "{}", old_grade.1);
    assert_eq!(agreed.0, 200, "the body, the certificate and the entry agree: {}", agreed.1);
    let recorded: Vec<String> = decided(&journal, "grant.issue", RICHARD).iter().map(|r| r.signed_decision.clone().unwrap_or_default()["signed_with"]["grade"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(recorded, vec!["tty".to_string()], "RECORDED: only the agreed decision reached the evaluator");

    // An entry not updated after a re-certification: the table still accepts the old grade, the certificate is `tty`.
    let dir = scratch("consent-entry-stale");
    let http_port = alloc_port();
    let a = started_with(&dir, http_port, None, &unit, |cfg| cfg.gateway_consent_principals = consent_table_at(&["richard"], OS)).await;
    let _ = forecaster_key(&a, &dir);
    let node = a.node_id().to_string();
    let key = ConsentKey::new();
    key.enrol(&a, &dir, "richard", "consent-richard", ConsentGrade::Tty);
    let requests = ask(http_port, 1).await;
    let stale_entry = post_decision(http_port, "relay-secret", &requests[0], issue_body(key.sign(RICHARD, &node, &requests[0], "issue", Some(600), None, Some("tty")))).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code(&stale_entry), (403, Some("decision_grade_not_accepted".into())), "STALE ENTRY: {}", stale_entry.1);
    assert!(stale_entry.1["detail"].as_str().unwrap_or_default().contains("signed_with was not updated"), "{}", stale_entry.1);
}

/// Identity-and-lifecycle I7, PR 3a: `dev`'s side — its software key, its prompt from the record and its signer —
/// against this file's fleet.
#[path = "dev_consent_tests.rs"]
mod dev_consent;
