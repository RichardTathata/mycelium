//! Identity-and-lifecycle I1 (D1′): a client's own signature on a gateway call — the vector, the provider's checks
//! at its receive boundary, the replay window, and the `/mcp` door.

use super::client_sig::{client_signature_message, door_binding_mcp, ClientNonce, ClientSignature, ClientSigner};
use super::gateway_caller::{self, CallerError};
use super::GossipAgent;
#[cfg(feature = "compliance")]
use crate::config::GatewayNamedToken;
use crate::config::{GossipConfig, TlsConfig};
use crate::node_id::NodeId;
use crate::test_util::alloc_port;
use bytes::{BufMut, Bytes, BytesMut};
use rustls::pki_types::pem::PemObject;
use std::path::{Path, PathBuf};
#[cfg(feature = "compliance")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PRINCIPAL: &str = "token:fleet/forecaster";
const TOOL: &str = "weather.forecast";

fn fresh(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("myc-client-sig-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(dir: &Path, port: u16, issuer: Option<&str>) -> GossipConfig {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("certs"), ..TlsConfig::default() });
    cfg.gateway_identity_issuer = issuer.map(str::to_string);
    cfg
}

async fn started(cfg: GossipConfig) -> Arc<GossipAgent> {
    let id = NodeId::new("127.0.0.1", cfg.bind_port).unwrap();
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.unwrap();
    a
}

/// Issue `name`'s client certificate from the node's CA, publish it, and return its signing key.
fn issue_and_publish(a: &GossipAgent, dir: &Path, name: &str, out: &str) -> ed25519_dalek::SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let (cert, key) = crate::issue_client_cert(&dir.join("certs"), "fleet", name, &dir.join(out)).unwrap();
    let der = rustls::pki_types::CertificateDer::from_pem_file(&cert).unwrap();
    a.publish_client_cert(der.as_ref()).expect("published");
    let der = rustls::pki_types::PrivatePkcs8KeyDer::from_pem_file(&key).unwrap();
    ed25519_dalek::SigningKey::from_pkcs8_der(der.secret_pkcs8_der()).unwrap()
}

fn request_from(sender: &NodeId, kind: &str, framed: Bytes) -> super::rpc::RpcRequest {
    let mut buf = BytesMut::with_capacity(8 + framed.len());
    buf.put_u64_le(42);
    buf.put(framed);
    super::rpc::RpcRequest::from(crate::signal::Signal {
        kind: Arc::from(kind),
        scope: crate::signal::SignalScope::Individual(sender.clone()),
        payload: buf.freeze(),
        sender: sender.clone(),
        nonce: 7,
    })
}

fn tool_payload(arguments: &serde_json::Value) -> Bytes {
    Bytes::from(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": TOOL, "arguments": arguments}}).to_string())
}

/// A tools/call request to `a` itself, framed as its gateway would, carrying `signed`.
fn signed_request(a: &GossipAgent, kind: &str, arguments: &serde_json::Value, signed: &(ClientSignature, ClientNonce)) -> super::rpc::RpcRequest {
    let resource = format!("tool:{TOOL}@{}", a.node_id());
    let framed = gateway_caller::frame_with_client_signature_at(
        &a.task_ctx, a.node_id(), PRINCIPAL, &[], tool_payload(arguments), None, Some(&resource), Some(signed),
        a.task_ctx.hlc.decision_now_ms(),
    )
    .unwrap();
    request_from(a.node_id(), kind, framed)
}

fn sign(signer: &ClientSigner, arguments: &serde_json::Value) -> (ClientSignature, ClientNonce) {
    let (op, res, d) = door_binding_mcp(TOOL, arguments).expect("safe arguments");
    signer.sign(&op, &res, &d)
}

/// **The cross-language vector** (`tests/fixtures/cs/vector.json`): the message rebuilt from its inputs, the
/// arguments' canonical digest, and the deterministic Ed25519 signature under the seed's key. jest and pytest check
/// the same file.
#[test]
fn the_client_signature_vector_is_the_same_in_every_language() {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/cs/vector.json")).unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let seed: [u8; 32] = (0..32).map(|i| u8::from_str_radix(&v["seed_hex"].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    assert_eq!(serde_json::to_string(&v["arguments"]).unwrap(), v["arguments_canonical"].as_str().unwrap());
    let (op, res, d) = door_binding_mcp("weather.forecast", &v["arguments"]).unwrap();
    assert_eq!((op.as_str(), hex(&d)), (v["operation"].as_str().unwrap(), v["arguments_digest_hex"].as_str().unwrap().to_string()));
    assert_eq!(res, "tool:weather.forecast");
    let instance = u64::from_str_radix(v["instance_hex"].as_str().unwrap(), 16).unwrap();
    let msg = client_signature_message(
        v["principal"].as_str().unwrap(), &op, v["resource"].as_str().unwrap(), &d,
        v["issued_at_ms"].as_u64().unwrap(), instance, v["counter"].as_u64().unwrap(),
    );
    assert_eq!(hex(&msg), v["message_hex"].as_str().unwrap());
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(b64.encode(key.verifying_key().to_bytes()), v["verifying_key_b64"].as_str().unwrap());
    assert_eq!(b64.encode(key.sign(&msg).to_bytes()), v["signature_b64"].as_str().unwrap());
    let meta: (ClientSignature, ClientNonce) = (serde_json::from_value(v["meta"]["cs"].clone()).unwrap(), serde_json::from_value(v["meta"]["cn"].clone()).unwrap());
    assert_eq!(meta.1.instance(), Some(instance));
}

/// **The arguments' canonical form is the same in every language** (#742's review, finding 1): RFC 8785 — exponent-form
/// floats (`0.00001`, `1e16`, `1e21`, `1e-7`), the largest double, non-ASCII text and control characters, and keys
/// sorted by UTF-16 code units at every depth. jest and pytest check the same cases. Seen failing first with the digest
/// over `serde_json`'s own encoding: `{"huge":1e21}` digested `{"huge":1e21}`, not `{"huge":1e+21}`.
#[test]
fn the_canonical_arguments_are_the_same_in_every_language() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/cs/vector.json")).unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let cases = v["canonical_cases"].as_array().unwrap();
    assert!(cases.len() >= 7);
    for c in cases {
        let args = &c["arguments"];
        assert_eq!(super::client_sig::canonical_json(args), c["canonical"].as_str().unwrap(), "canonical form of {args}");
        assert_eq!(hex(&super::client_sig::client_arguments_digest(args)), c["digest_hex"].as_str().unwrap(), "DIVERGED: the digest of {args}");
    }
}

/// **A client-signed call verifies at the provider and names its key**; a call signed by a key with no certificate
/// on record, by a superseded key, over other arguments, or for a kind whose call the provider cannot read, is refused
/// by name. Seen failing first, each with its check planted out in `client_sig::check` (the standing, the signature,
/// the binding): the forged, superseded and tampered calls verified.
#[tokio::test]
async fn the_provider_checks_the_client_signature_binding_signature_and_key_standing() {
    let dir = fresh("checks");
    let a = started(config(&dir, alloc_port(), Some("fleet"))).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let args = serde_json::json!({"city": "Lyon", "days": 3});
    let kind = crate::signal::signal_kind::MCP_INVOKE;

    let signer = ClientSigner::new(k1.clone(), PRINCIPAL);
    let ok = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, kind, &args, &sign(&signer, &args)));
    let ok_key = ok.as_ref().ok().and_then(|c| c.as_ref()).and_then(|c| c.client_signed.as_ref()).map(|s| s.key);

    let rogue = ClientSigner::new(ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]), PRINCIPAL);
    let foreign = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, kind, &args, &sign(&rogue, &args)));

    let other = serde_json::json!({"city": "Brest", "days": 3});
    let tampered = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, kind, &other, &sign(&signer, &args)));

    let unbound = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, "some.other.kind", &args, &sign(&signer, &args)));

    tokio::time::sleep(Duration::from_millis(5)).await;
    let _k2 = issue_and_publish(&a, &dir, "forecaster", "k2");
    let superseded = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, kind, &args, &sign(&signer, &args)));
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(ok_key, Some(k1.verifying_key().to_bytes()), "a signed call verified, naming its key: {ok:?}");
    assert_eq!(foreign.err(), Some(CallerError::ClientSignature("client_key_unknown")), "FORGED: a key with no certificate on record");
    assert_eq!(tampered.err(), Some(CallerError::ClientSignature("client_signature_invalid")), "TAMPERED: a signature over other arguments");
    assert_eq!(unbound.err(), Some(CallerError::ClientSignature("client_signature_unbound")), "UNBOUND: a kind the provider cannot read");
    assert_eq!(superseded.err(), Some(CallerError::ClientSignature("client_key_superseded")), "SUPERSEDED: an older key after a newer certificate");
}

/// **A replayed `cn` is dropped, a reordered honest pair is admitted, and two processes under one certificate do not
/// collide.** Counter 1 then counter 0 from one process both verify (a gateway may deliver them out of order); counter
/// 1 again is a replay, dropped without an answer. A second process — another instance id — sends the same issue time
/// and counter and is admitted. Seen failing first: with the window planted to key on `(t, c)` across instances, the
/// second process's call was refused as a replay.
#[tokio::test]
async fn a_replayed_cn_is_dropped_and_reordered_and_concurrent_honest_calls_are_admitted() {
    let dir = fresh("replay");
    let a = started(config(&dir, alloc_port(), Some("fleet"))).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let args = serde_json::json!({"city": "Lyon", "days": 3});
    let kind = crate::signal::signal_kind::MCP_INVOKE;
    let (op, res, d) = door_binding_mcp(TOOL, &args).unwrap();
    let t = mycelium_core::sim_seam::wall_now_ms();
    let p1 = ClientSigner::with_instance(k1.clone(), PRINCIPAL, 0x1111);
    let first = p1.sign_at(&op, &res, &d, t);
    let second = p1.sign_at(&op, &res, &d, t);
    let verify = |s: &(ClientSignature, ClientNonce)| gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, kind, &args, s));
    let later_first = verify(&second);
    let earlier_second = verify(&first);
    let again = verify(&second);
    let p2 = ClientSigner::with_instance(k1, PRINCIPAL, 0x2222);
    let other_process = verify(&p2.sign_at(&op, &res, &d, t));
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(later_first.is_ok() && earlier_second.is_ok(), "a reordered honest pair is admitted: {later_first:?} {earlier_second:?}");
    assert_eq!(again.as_ref().err(), Some(&CallerError::ClientReplayed), "REPLAYED: the same cn was admitted twice");
    assert!(again.unwrap_err().unanswered(), "a replay is dropped without an answer");
    assert!(other_process.is_ok(), "COLLIDED: a second process under one certificate was refused: {other_process:?}");
}

/// **The `/mcp` door:** a client-signed `tools/call` runs, and the tool sees the verified signature; an unsigned call
/// from the same principal, whose certificate is on record, runs and is counted (`gateway_actions_unsigned`); and a
/// signed call at a gateway with no `gateway_identity_issuer` is refused by name before dispatch. Seen failing first:
/// with the door's issuer check planted out, the signed call at the issuer-less gateway was dispatched; with the count
/// planted out, the unsigned call was not counted. Needs `compliance`: a named token table refuses the start without it.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn the_mcp_door_carries_a_client_signature_counts_an_unsigned_one_and_needs_an_issuer() {
    use axum::http::header::AUTHORIZATION;
    let dir = fresh("door");
    let (port, http_port) = (alloc_port(), alloc_port());
    let mut cfg = config(&dir, port, Some("fleet"));
    cfg.http_port = Some(http_port);
    cfg.gateway_named_tokens = vec![GatewayNamedToken { name: "forecaster".into(), token: "fc-secret".into(), scopes: vec!["mcp:invoke".into()] }];
    let a = started(cfg).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let ran = Arc::new(AtomicUsize::new(0));
    let saw_signed = Arc::new(parking_lot::Mutex::new(Vec::<bool>::new()));
    let (r, s) = (Arc::clone(&ran), Arc::clone(&saw_signed));
    let _tool = a.mcp().register_mcp_tool_with_call(TOOL, serde_json::json!({"type": "object"}), move |call, _args| {
        let (r, s) = (Arc::clone(&r), Arc::clone(&s));
        async move {
            r.fetch_add(1, Ordering::SeqCst);
            let signed = matches!(&call.principal, crate::RequestPrincipal::Client(c) if c.client_signed.is_some());
            s.lock().push(signed);
            Ok(serde_json::json!("sunny"))
        }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let args = serde_json::json!({"city": "Lyon", "days": 3});
    let signer = ClientSigner::new(k1, PRINCIPAL);
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_port}/mcp");
    let call = |meta: Option<serde_json::Value>| {
        let mut params = serde_json::json!({"name": TOOL, "arguments": args});
        if let Some(m) = meta {
            params["_meta"] = m;
        }
        client.post(&url).header(AUTHORIZATION, "Bearer fc-secret")
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params})).send()
    };
    let signed: serde_json::Value = call(Some(ClientSigner::meta(&sign(&signer, &args)))).await.unwrap().json().await.unwrap();
    let unsigned: serde_json::Value = call(None).await.unwrap().json().await.unwrap();
    let counted = a.task_ctx.unsigned_actions.tools_call.load(Ordering::Relaxed);
    let stats: serde_json::Value = client.get(format!("http://127.0.0.1:{http_port}/stats")).header(AUTHORIZATION, "Bearer fc-secret")
        .send().await.unwrap().json().await.unwrap_or_default();
    a.shutdown().await;

    // A second gateway, the same tool and principal, no issuer.
    let dir2 = fresh("door-no-issuer");
    let (port2, http2) = (alloc_port(), alloc_port());
    let mut cfg2 = config(&dir2, port2, None);
    cfg2.http_port = Some(http2);
    cfg2.gateway_named_tokens = vec![GatewayNamedToken { name: "forecaster".into(), token: "fc-secret".into(), scopes: vec!["mcp:invoke".into()] }];
    let b = started(cfg2).await;
    let ran_b = Arc::new(AtomicUsize::new(0));
    let rb = Arc::clone(&ran_b);
    let _tool_b = b.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| {
        let rb = Arc::clone(&rb);
        async move {
            rb.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("sunny"))
        }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let refused: serde_json::Value = client.post(format!("http://127.0.0.1:{http2}/mcp")).header(AUTHORIZATION, "Bearer fc-secret")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": TOOL, "arguments": args, "_meta": ClientSigner::meta(&sign(&signer, &args))}}))
        .send().await.unwrap().json().await.unwrap();
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);

    assert!(signed.get("result").is_some(), "the signed call ran: {signed}");
    assert!(unsigned.get("result").is_some(), "the unsigned call ran (counted, not yet refused): {unsigned}");
    assert_eq!(*saw_signed.lock(), vec![true, false], "the tool saw the signed call's verified signature, and none on the other");
    assert_eq!(counted, 1, "UNCOUNTED: the unsigned call from a certificate holder was not counted");
    assert_eq!(stats["gateway_actions_unsigned"]["tools/call"], 1, "the count is on /stats: {stats}");
    assert_eq!(stats["provider_actions_unsigned"], 1, "and counted where it ran: {stats}");
    assert_eq!(refused["error"]["data"]["reason"], "gateway_identity_issuer_unset", "DISPATCHED: a signed call at an issuer-less gateway: {refused}");
    assert_eq!(ran_b.load(Ordering::SeqCst), 0, "nothing ran at the issuer-less gateway");
    assert_eq!(ran.load(Ordering::SeqCst), 2);
}

/// **The 1,025th instance of a key is refused with an answer, not dropped as a replay, and an instance's floor refuses
/// what lies at or below it** (#742's review, finding 2). A per-invocation signer (a CLI, a cron job) draws an instance
/// per call; past 1 024 inside the window the provider answers `client_instances_exhausted` — not unanswered, so the
/// caller is told — and counts it. Past 65 536 entries one instance's oldest is evicted and becomes its floor. Seen
/// failing first with the cap mapped to `ClientReplayed`: the 1,025th was dropped without an answer.
#[tokio::test]
async fn the_instance_cap_is_answered_and_the_eviction_floor_refuses_at_or_below_it() {
    use super::client_sig::{ClientReplay, ClientReplayWindows, ClientSigned, CLIENT_INSTANCES_PER_KEY, CLIENT_WINDOW_PER_INSTANCE};
    let w = ClientReplayWindows::default();
    let key = [9u8; 32];
    let s = |instance: u64, t: u64, counter: u64| ClientSigned { key, issued_at_ms: t, instance, counter };
    for c in 0..=CLIENT_WINDOW_PER_INSTANCE as u64 {
        assert_eq!(w.admit("p", &s(1, 1_000, c), Some(key), 0), Ok(()));
    }
    assert_eq!(w.admit("p", &s(1, 1_000, 0), Some(key), 0), Err(ClientReplay::Replayed), "the evicted oldest is the floor");
    assert_eq!(w.admit("p", &s(1, 999, 70_000), Some(key), 0), Err(ClientReplay::Replayed), "below the floor");
    assert_eq!(w.admit("p", &s(1, 1_001, 0), Some(key), 0), Ok(()), "above the floor");

    let dir = fresh("cap");
    let a = started(config(&dir, alloc_port(), Some("fleet"))).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let kb = k1.verifying_key().to_bytes();
    let now = a.task_ctx.hlc.decision_now_ms();
    for i in 0..CLIENT_INSTANCES_PER_KEY as u64 {
        a.task_ctx.client_replay.admit(PRINCIPAL, &ClientSigned { key: kb, issued_at_ms: now, instance: 10_000 + i, counter: 0 }, Some(kb), 0).unwrap();
    }
    let args = serde_json::json!({"city": "Lyon"});
    let one_more = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, crate::signal::signal_kind::MCP_INVOKE, &args, &sign(&ClientSigner::with_instance(k1, PRINCIPAL, 1), &args)));
    let refused = a.task_ctx.client_replay.instances_refused.load(std::sync::atomic::Ordering::Relaxed);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let err = one_more.unwrap_err();
    assert_eq!(err, CallerError::ClientSignature("client_instances_exhausted"), "DROPPED: the 1,025th instance was answered as a replay");
    assert!(!err.unanswered(), "the cap is answered");
    assert_eq!(refused, 1);
}

/// **A superseded key's windows go when its successor signs** (#742's review, item 7): a principal holds only its
/// current key's windows (and a racing predecessor's), so `by_principal` is bounded by the principals, not by every key
/// they ever held. Seen failing first: both keys' windows stayed.
#[test]
fn a_principals_superseded_key_windows_are_dropped_when_its_successor_signs() {
    use super::client_sig::{ClientReplayWindows, ClientSigned};
    let w = ClientReplayWindows::default();
    let (k1, k2) = ([1u8; 32], [2u8; 32]);
    w.admit("p", &ClientSigned { key: k1, issued_at_ms: 10, instance: 1, counter: 0 }, Some(k1), 0).unwrap();
    // K2 is current; a call K1 signed earlier still lands — both kept while they race.
    w.admit("p", &ClientSigned { key: k1, issued_at_ms: 11, instance: 1, counter: 1 }, Some(k2), 0).unwrap();
    w.admit("p", &ClientSigned { key: k2, issued_at_ms: 12, instance: 2, counter: 0 }, Some(k2), 0).unwrap();
    assert_eq!(w.keys_held("p"), 1, "GROWN: the superseded key's windows were kept");
}

/// **An unsigned call for a certificate holder is counted where it runs** (#742's review, item 4): a gateway that strips
/// `cs` leaves a trace at the provider. Seen failing first: the provider counted nothing.
#[tokio::test]
async fn an_unsigned_call_for_a_certificate_holder_is_counted_at_the_provider() {
    let dir = fresh("unsigned-provider");
    let a = started(config(&dir, alloc_port(), Some("fleet"))).await;
    let _k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let args = serde_json::json!({"city": "Lyon"});
    let framed = gateway_caller::frame_with_context_at(&a.task_ctx, a.node_id(), PRINCIPAL, &[], tool_payload(&args), None, None, a.task_ctx.hlc.decision_now_ms()).unwrap();
    let ok = gateway_caller::verify_received(&a.task_ctx, &request_from(a.node_id(), crate::signal::signal_kind::MCP_INVOKE, framed));
    let counted = a.task_ctx.unsigned_actions.at_provider.load(std::sync::atomic::Ordering::Relaxed);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok.is_ok(), "served: counted, not refused: {ok:?}");
    assert_eq!(counted, 1, "UNCOUNTED at the provider");
}

/// **The LLM doors refuse a client signature rather than drop it** (#742's review, item 4): `/gateway/llm/call` answers
/// `400 client_signature_unbound`, `/gateway/llm/stream` its error event. Seen failing first: the call went on to
/// resolve a provider (`404 no_provider`) with the signature dropped.
#[cfg(feature = "llm")]
#[tokio::test]
async fn the_llm_doors_refuse_a_client_signature_rather_than_drop_it() {
    let dir = fresh("llm");
    let (port, http_port) = (alloc_port(), alloc_port());
    let mut cfg = config(&dir, port, Some("fleet"));
    cfg.http_port = Some(http_port);
    let a = started(cfg).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let signed = ClientSigner::meta(&ClientSigner::new(ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]), PRINCIPAL).sign("x", "y", &[0u8; 32]));
    let body = serde_json::json!({"ns": "demo", "name": "hello", "input": "hi", "_meta": signed});
    let client = reqwest::Client::new();
    let call = client.post(format!("http://127.0.0.1:{http_port}/gateway/llm/call")).json(&body).send().await.unwrap();
    let call_status = call.status().as_u16();
    let call_body: serde_json::Value = call.json().await.unwrap_or_default();
    let stream = client.post(format!("http://127.0.0.1:{http_port}/gateway/llm/stream")).json(&body).send().await.unwrap().text().await.unwrap();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((call_status, call_body["reason"].as_str()), (400, Some("client_signature_unbound")), "DROPPED at /llm/call: {call_body}");
    assert!(stream.contains("client_signature_unbound"), "DROPPED at /llm/stream: {stream}");
}

/// **Both `/a2a` doors carry a client signature, and the skill sees it verified** (#742's review, finding 5):
/// `tasks/send` and `tasks/sendSubscribe` signed over `("skill.invoke", "skill:depot/dispatch", {"text": …})` reach the
/// skill with the signature verified at its receive boundary (the `skill.invoke` binding); at a gateway without an
/// issuer the stream answers a failed task naming it. Seen failing first with `None` planted at each door's dispatch: the
/// skill saw no signature on either path.
#[cfg(all(feature = "compliance", feature = "a2a"))]
#[tokio::test]
async fn both_a2a_doors_carry_a_client_signature_and_the_skill_sees_it() {
    use crate::capability::Capability;
    let a2a_node = |dir: &Path, issuer: Option<&str>| {
        let (port, http_port) = (alloc_port(), alloc_port());
        let mut cfg = config(dir, port, issuer);
        cfg.http_port = Some(http_port);
        cfg.gateway_named_tokens = vec![GatewayNamedToken { name: "forecaster".into(), token: "fc-secret".into(), scopes: vec!["kv:read".into()] }];
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg).with_a2a());
        (a, http_port)
    };
    let serve = |a: &Arc<GossipAgent>, seen: Arc<parking_lot::Mutex<Vec<bool>>>| {
        let (a, seen) = (Arc::clone(a), seen);
        let mut rx = a.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let signed = a.gateway_caller(&req).ok().flatten().is_some_and(|c| c.client_signed.is_some());
                seen.lock().push(signed);
                a.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    };
    let until_advertised = |a: &Arc<GossipAgent>| {
        let key = format!("cap/{}/depot/dispatch", a.node_id());
        let a = Arc::clone(a);
        async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while a.kv().get(&key).is_none() {
                assert!(std::time::Instant::now() < deadline, "{key} never written");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    let client = reqwest::Client::new();
    let post = |http_port: u16, method: &'static str, params: serde_json::Value| {
        client.post(format!("http://127.0.0.1:{http_port}/a2a")).bearer_auth("fc-secret")
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).send()
    };
    let params = |id: &str, meta: serde_json::Value| serde_json::json!({
        "id": id, "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}, "_meta": meta,
    });

    let dir = fresh("a2a");
    let (a, http_port) = a2a_node(&dir, Some("fleet"));
    a.start().await.unwrap();
    let _cap = a.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    serve(&a, Arc::clone(&seen));
    until_advertised(&a).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    let signer = ClientSigner::new(k1, PRINCIPAL);
    let meta = || {
        let (op, res, d) = super::client_sig::door_binding_a2a("depot/dispatch", "go");
        ClientSigner::meta(&signer.sign(&op, &res, &d))
    };
    let sent: serde_json::Value = post(http_port, "tasks/send", params("t-send", meta())).await.unwrap().json().await.unwrap();
    let streamed = post(http_port, "tasks/sendSubscribe", params("t-stream", meta())).await.unwrap().text().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.shutdown().await;

    let dir2 = fresh("a2a-no-issuer");
    let (b, http2) = a2a_node(&dir2, None);
    b.start().await.unwrap();
    let _cap_b = b.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
    let seen_b = Arc::new(parking_lot::Mutex::new(Vec::new()));
    serve(&b, Arc::clone(&seen_b));
    until_advertised(&b).await;
    let refused = post(http2, "tasks/sendSubscribe", params("t-refused", meta())).await.unwrap().text().await.unwrap();
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);

    assert_eq!(sent["result"]["status"]["state"], "completed", "{sent}");
    assert!(streamed.contains("completed"), "{streamed}");
    assert_eq!(*seen.lock(), vec![true, true], "UNSIGNED: the skill did not see the signature on both doors");
    assert!(refused.contains("failed") && refused.contains("gateway_identity_issuer"), "the stream's refusal: {refused}");
    assert!(seen_b.lock().is_empty(), "nothing ran at the issuer-less gateway");
}

/// **An integer outside ±(2^53 − 1) is refused at the signer and at the provider** (#742's confirm round, finding 1).
/// RFC 8785 writes `9007199254740993` as `9007199254740992`, so the digest cannot tell them apart while the tool
/// receives the exact integer. Every signer refuses such arguments, and the provider refuses a call carrying them by
/// name, `client_signature_unsafe_integer` — the vector's integer cases, as jest and pytest check them. Seen failing
/// first with the check planted out: the provider verified a signature over `{"id":9007199254740992}` for a call whose
/// tool received `9007199254740993`.
#[tokio::test]
async fn an_integer_beyond_2_53_is_refused_at_the_signer_and_at_the_provider() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/cs/vector.json")).unwrap();
    for text in v["unsafe_integer_cases"].as_array().unwrap() {
        let args: serde_json::Value = serde_json::from_str(text.as_str().unwrap()).unwrap();
        assert_eq!(door_binding_mcp(TOOL, &args).err(), Some(super::client_sig::UnsafeInteger), "SIGNED: {text}");
    }
    for text in v["safe_integer_cases"].as_array().unwrap() {
        let args: serde_json::Value = serde_json::from_str(text.as_str().unwrap()).unwrap();
        assert!(door_binding_mcp(TOOL, &args).is_ok(), "refused a safe case: {text}");
    }

    let dir = fresh("unsafe-int");
    let a = started(config(&dir, alloc_port(), Some("fleet"))).await;
    let k1 = issue_and_publish(&a, &dir, "forecaster", "k1");
    // A relay's swap: signed over the neighbour (they digest alike), sent with the exact large id.
    let sent: serde_json::Value = serde_json::from_str(r#"{"id":9007199254740993}"#).unwrap();
    let neighbour: serde_json::Value = serde_json::from_str(r#"{"id":9007199254740992}"#).unwrap();
    assert_eq!(super::client_sig::client_arguments_digest(&sent), super::client_sig::client_arguments_digest(&neighbour), "the two digest alike");
    let signer = ClientSigner::new(k1, PRINCIPAL);
    let signed = signer.sign("tools/call", &format!("tool:{TOOL}"), &super::client_sig::client_arguments_digest(&neighbour));
    let verified = gateway_caller::verify_received(&a.task_ctx, &signed_request(&a, crate::signal::signal_kind::MCP_INVOKE, &sent, &signed));
    let at_signer = signer.sign_tools_call(TOOL, &sent);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(verified.err(), Some(CallerError::ClientSignature("client_signature_unsafe_integer")), "SWAPPED: a large id bound inexactly verified");
    assert!(at_signer.is_err(), "the signer refuses it");
}
