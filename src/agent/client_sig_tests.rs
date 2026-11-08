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
    let (op, res, d) = door_binding_mcp(TOOL, arguments);
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
    let (op, res, d) = door_binding_mcp("weather.forecast", &v["arguments"]);
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
    assert_eq!(foreign.err(), Some(CallerError::ClientSignature("client_key_not_current")), "FORGED: a key with no certificate on record");
    assert_eq!(tampered.err(), Some(CallerError::ClientSignature("client_signature_invalid")), "TAMPERED: a signature over other arguments");
    assert_eq!(unbound.err(), Some(CallerError::ClientSignature("client_signature_unbound")), "UNBOUND: a kind the provider cannot read");
    assert_eq!(superseded.err(), Some(CallerError::ClientSignature("client_key_not_current")), "SUPERSEDED: an older key after a newer certificate");
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
    let (op, res, d) = door_binding_mcp(TOOL, &args);
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
    assert_eq!(refused["error"]["data"]["reason"], "gateway_identity_issuer_unset", "DISPATCHED: a signed call at an issuer-less gateway: {refused}");
    assert_eq!(ran_b.load(Ordering::SeqCst), 0, "nothing ran at the issuer-less gateway");
    assert_eq!(ran.load(Ordering::SeqCst), 2);
}
