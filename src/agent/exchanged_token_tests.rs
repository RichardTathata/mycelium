//! Identity-and-lifecycle I5, PR 1: an exchanged OIDC token (`ot`) at the provider. One node is both the gateway that
//! frames the envelope (with its node key) and the provider that verifies it, so sender binding is tested against the
//! key that really signed the envelope; the IdP is the RSA test fixture, its JWKS served on loopback.

use super::gateway_caller::{frame_envelope_at, verify, verify_received, CallerError, ExchangedToken};
use super::exchanged_token::{ed25519_jwk_thumbprint, needs_exchanged_token, OtAdmit, OtReplayWindows};
use crate::{GossipAgent, GossipConfig, NodeId};
use bytes::{BufMut as _, Bytes, BytesMut};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::sync::Arc;
use std::time::Duration;

const ISSUER: &str = "https://idp.example";
const AUDIENCE: &str = "mycelium:fleet";
const CLIENT: &str = "mycelium-gateway";
const TEST_PRIV: &str = include_str!("../../tests/fixtures/oidc_test.key");
const JWKS: &str = include_str!("../../tests/fixtures/oidc_jwks.json");

fn now_s() -> u64 {
    mycelium_core::sim_seam::wall_now_ms() / 1_000
}

/// A started node with `[tls]` and an `[oidc]` table naming `exchange_audience`, its JWKS at `jwks_port`.
async fn provider(jwks_port: u16, tweak: impl FnOnce(&mut crate::OidcConfig)) -> Arc<GossipAgent> {
    let port = crate::test_util::alloc_port();
    let dir = std::env::temp_dir().join(format!("i5-ot-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir, ..Default::default() });
    let mut oidc = crate::OidcConfig {
        issuer: ISSUER.into(),
        audience: "mycelium-cluster".into(),
        group_claim: "groups".into(),
        group_scopes: Default::default(),
        jwks_uri: Some(format!("http://127.0.0.1:{jwks_port}/jwks")),
        exchange_audience: Some(AUDIENCE.into()),
        exchange_client_id: Some(CLIENT.into()),
        exchange_max_lifetime_secs: None,
    };
    tweak(&mut oidc);
    cfg.oidc = Some(oidc);
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await.expect("starts");
    a
}

/// Wait until the provider holds the fixture's key (fetched at `start()`).
async fn keys_held(a: &GossipAgent) {
    let v = a.task_ctx.exchanged_token_verifier.get().expect("a verifying provider has a verifier");
    for _ in 0..100 {
        if v.keys_now("test-kid").is_some_and(|k| k.iter().any(|(kid, _)| kid == "test-kid")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the JWKS was never fetched");
}

fn node_key(a: &GossipAgent) -> [u8; 32] {
    a.task_ctx.tls.get().unwrap().verifying_key_bytes()
}

/// The claims of an honest exchanged token for `alice`, bound to `key`.
fn claims(key: &[u8; 32]) -> serde_json::Value {
    let now = now_s();
    serde_json::json!({
        "iss": ISSUER, "aud": AUDIENCE, "azp": CLIENT, "sub": "alice",
        "iat": now, "exp": now + 300, "jti": format!("jti-{}", JTI.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
        "cnf": {"jkt": ed25519_jwk_thumbprint(key)},
    })
}

fn mint(claims: &serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".into());
    encode(&header, claims, &EncodingKey::from_rsa_pem(TEST_PRIV.as_bytes()).unwrap()).unwrap()
}

const ALICE: &str = "oidc:https://idp.example/alice";
static JTI: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A request from this node to itself, of `kind`, carrying `ot` for `principal`.
fn request(a: &GossipAgent, kind: &str, principal: &str, ot: Option<&str>) -> super::RpcRequest {
    let app = Bytes::from_static(b"{\"jsonrpc\":\"2.0\"}");
    let framed = frame_envelope_at(&a.task_ctx, a.node_id(), principal, &["mcp:invoke".into()], app, None, None, None, ot, a.task_ctx.hlc.decision_now_ms())
        .expect("fits");
    request_from(a.node_id(), kind, framed)
}

fn request_from(sender: &NodeId, kind: &str, framed: Bytes) -> super::RpcRequest {
    let mut buf = BytesMut::with_capacity(8 + framed.len());
    buf.put_u64_le(42);
    buf.put(framed);
    super::RpcRequest::from(crate::signal::Signal {
        kind: Arc::from(kind),
        scope: crate::signal::SignalScope::Individual(sender.clone()),
        payload: buf.freeze(),
        sender: sender.clone(),
        nonce: 7,
    })
}

fn code(r: Result<Option<super::gateway_caller::GatewayCaller>, CallerError>) -> Option<&'static str> {
    match r {
        Ok(_) => None,
        Err(e) => Some(e.code()),
    }
}

/// **An honest exchanged token is verified, and admitted once** (I5's exit: "an OIDC user's call verified at the
/// provider"; "the same exchanged token presented twice refused"). A token bound to the key that signed its envelope,
/// for the envelope's principal, by the gateway's client, is verified; presented again in a second, fresh envelope it
/// is refused `ot_replayed` at the receive boundary. Seen failing first: before the check, the second was admitted.
#[tokio::test]
async fn an_honest_exchanged_token_is_verified_and_admitted_once() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |_| {}).await;
    keys_held(&a).await;
    let ot = mint(&claims(&node_key(&a)));
    let first = verify_received(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some(&ot)));
    let again = verify_received(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some(&ot)));
    let counted = a.task_ctx.exchanged_token_replay.replayed.load(std::sync::atomic::Ordering::Relaxed);
    a.shutdown().await;
    assert_eq!(counted, 1, "the replay counted (GET /stats exchanged_tokens_refused.replayed)");
    let c = first.expect("verified").expect("a caller");
    assert_eq!(c.exchanged.as_ref().map(|t| t.iss.as_str()), Some(ISSUER), "VERIFIED");
    assert_eq!(code(again), Some("ot_replayed"), "REPLAY: the same token in a fresh envelope");
}

/// **Every check of the row's order refuses by name** (I5's exit, PR 1). One honest token, changed in one place for
/// each case; the honest one is verified beside them. Each seen failing first with its check removed, one build each.
#[tokio::test]
async fn each_check_refuses_an_exchanged_token_by_name() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |_| {}).await;
    keys_held(&a).await;
    let key = node_key(&a);
    let with = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut c = claims(&key);
        edit(&mut c);
        mint(&c)
    };
    let other_key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
    let tampered = {
        // A valid token's payload replaced under its signature: refused by the JWKS (a member's forgery).
        let honest = with(&|_| {});
        let parts: Vec<&str> = honest.split('.').collect();
        use base64::Engine as _;
        let mut c = claims(&key);
        c["sub"] = "mallory".into();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).unwrap());
        format!("{}.{}.{}", parts[0], payload, parts[2])
    };
    let cases: Vec<(&str, String, &str)> = vec![
        ("SIGNATURE", tampered, "ot_signature_invalid"),
        ("AUDIENCE: a gateway-audience token", with(&|c| c["aud"] = "mycelium-cluster".into()), "ot_wrong_audience"),
        ("CLIENT", with(&|c| c["azp"] = "another-client".into()), "ot_wrong_client"),
        ("UNBOUND", with(&|c| { c.as_object_mut().unwrap().remove("cnf"); }), "ot_unbound"),
        ("ANOTHER KEY", with(&|c| c["cnf"]["jkt"] = ed25519_jwk_thumbprint(&other_key).into()), "ot_bound_to_another_key"),
        ("SUBJECT", with(&|c| c["sub"] = "bob".into()), "ot_subject_mismatch"),
        ("LIFETIME", with(&|c| c["exp"] = (c["iat"].as_u64().unwrap() + 301).into()), "ot_lifetime_too_long"),
        ("EXPIRED", with(&|c| { c["iat"] = (now_s() - 1_000).into(); c["exp"] = (now_s() - 900).into(); }), "ot_expired"),
        ("NO JTI", with(&|c| { c.as_object_mut().unwrap().remove("jti"); }), "ot_without_jti"),
    ];
    let mut got = Vec::new();
    for (name, ot, _) in &cases {
        got.push((*name, code(verify(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some(ot))))));
    }
    let honest = code(verify(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some(&with(&|_| {})))));
    a.shutdown().await;
    assert_eq!(honest, None, "the honest token beside them is verified");
    for ((name, _, want), (_, got)) in cases.iter().zip(got) {
        assert_eq!(got, Some(*want), "{name}");
    }
}

/// **`so` binds the token to its envelope** (#757's confirm round, N3). An envelope whose `so` is stripped is refused
/// `ot_unsigned`; one whose `ot` is swapped for another under the same `so` fails the signature. Seen failing first
/// with `so` unchecked.
#[tokio::test]
async fn the_gateways_signature_over_the_token_is_required() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |_| {}).await;
    keys_held(&a).await;
    let key = node_key(&a);
    let req = request(&a, "mcp.invoke", ALICE, Some(&mint(&claims(&key))));
    let edited = |edit: &dyn Fn(&mut serde_json::Value)| {
        let super::gateway_caller::Frame::Framed { envelope, app } = req.frame() else { panic!() };
        let mut env: serde_json::Value = serde_json::from_slice(&envelope).unwrap();
        edit(&mut env);
        let env = serde_json::to_vec(&env).unwrap();
        let mut f = vec![0x00, b'G', b'W', b'C', 1];
        f.extend_from_slice(&(env.len() as u16).to_be_bytes());
        f.extend_from_slice(&env);
        f.extend_from_slice(&app);
        request_from(a.node_id(), "mcp.invoke", Bytes::from(f))
    };
    let stripped = code(verify(&a.task_ctx, &edited(&|e| { e.as_object_mut().unwrap().remove("so"); })));
    let swapped_to = mint(&claims(&key));
    let swapped = code(verify(&a.task_ctx, &edited(&|e| e["ot"] = swapped_to.clone().into())));
    a.shutdown().await;
    assert_eq!(stripped, Some("ot_unsigned"), "STRIPPED");
    assert_eq!(swapped, Some("bad_signature"), "SWAPPED");
}

/// **An `oidc:` principal's action needs an `ot`** (#757's confirm round, N4). At a verifying provider: `mcp.invoke`,
/// a companion's write kind and an operator-listed kind are refused `ot_required` without one; a companion's read kind
/// and an unprotected kind are not actions and pass; a `token:` principal is not asked. Seen failing first with the
/// requirement removed.
#[tokio::test]
async fn an_oidc_action_without_an_exchanged_token_is_refused() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |_| {}).await;
    let refused = |kind: &str, p: &str| code(verify(&a.task_ctx, &request(&a, kind, p, None)));
    let got = [
        refused("mcp.invoke", ALICE),
        refused("blackboard.ops.post", ALICE),
        refused("wiki.council.ingest", ALICE),
        refused("blackboard.ops.read", ALICE),
        refused("tuple.jobs.depth", ALICE),
        refused("k", ALICE),
        refused("mcp.invoke", "token:fleet/forecaster"),
    ];
    a.shutdown().await;
    assert_eq!(got, [Some("ot_required"), Some("ot_required"), Some("ot_required"), None, None, None, None], "REQUIRED");
}

/// **No key held, no admission** (the maintainer's decision of 2026-11-11). A provider whose JWKS cannot be fetched
/// holds no key, and refuses a token `ot_keys_unavailable` — retriable, never admitted. Seen failing first with the
/// refusal replaced by an admission.
#[tokio::test]
async fn a_token_arriving_before_any_key_is_held_is_refused_retriably() {
    let closed = crate::test_util::alloc_port();
    let a = provider(closed, |_| {}).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let got = code(verify(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some(&mint(&claims(&node_key(&a)))))));
    a.shutdown().await;
    assert_eq!(got, Some("ot_keys_unavailable"), "KEYS");
}

/// **A provider that does not verify behaves as before I5.** Without `exchange_audience` an envelope's `ot` is not
/// read, and an `oidc:` action without one is admitted as today.
#[tokio::test]
async fn a_provider_without_an_exchange_audience_reads_no_exchanged_token() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |o| {
        o.exchange_audience = None;
        o.exchange_client_id = None;
    })
    .await;
    let with_junk = code(verify(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, Some("not-a-token"))));
    let without = code(verify(&a.task_ctx, &request(&a, "mcp.invoke", ALICE, None)));
    let unset = a.task_ctx.exchanged_token_verifier.get().is_none();
    a.shutdown().await;
    assert!(unset);
    assert_eq!((with_junk, without), (None, None));
}

/// **The replay window's bounds** (#757's confirm round, N1). It refuses rather than evicts: past a principal's share
/// a token of that principal is refused `ot_principal_window_full` while another principal's is admitted; past the
/// gateway's, every token from it is refused `ot_window_full`; an expired entry frees its place. Seen failing first
/// with each bound removed.
#[test]
fn the_exchanged_token_window_is_bounded_per_principal_and_per_gateway() {
    let w = OtReplayWindows::with_bounds(3, 2);
    let gw = NodeId::new("127.0.0.1", 9001).unwrap();
    let t = |jti: &str, exp_ms: u64| ExchangedToken { iss: ISSUER.into(), jti: jti.into(), exp_ms };
    let now = 1_000_000;
    assert_eq!(w.admit(&gw, "alice", &t("a1", now + 10_000), now, 0), Ok(()));
    assert_eq!(w.admit(&gw, "alice", &t("a1", now + 10_000), now, 0), Err(OtAdmit::Replayed), "REPLAY");
    assert_eq!(w.admit(&gw, "alice", &t("a2", now + 10_000), now, 0), Ok(()));
    assert_eq!(w.admit(&gw, "alice", &t("a3", now + 10_000), now, 0), Err(OtAdmit::PrincipalFull), "PRINCIPAL");
    assert_eq!(w.admit(&gw, "bob", &t("b1", now + 10_000), now, 0), Ok(()), "another principal still admitted");
    assert_eq!(w.admit(&gw, "carol", &t("c1", now + 10_000), now, 0), Err(OtAdmit::GatewayFull), "GATEWAY");
    let other = NodeId::new("127.0.0.1", 9002).unwrap();
    assert_eq!(w.admit(&other, "carol", &t("c1", now + 10_000), now, 0), Ok(()), "another gateway's window is its own");
    // Past the entries' expiry plus the leeway, their places are free again.
    assert_eq!(w.admit(&gw, "carol", &t("c2", now + 50_000), now + 10_001, 0), Ok(()), "EXPIRY");
    assert_eq!(
        (w.replayed.load(std::sync::atomic::Ordering::Relaxed), w.principal_full.load(std::sync::atomic::Ordering::Relaxed), w.gateway_full.load(std::sync::atomic::Ordering::Relaxed)),
        (1, 1, 1),
        "each refusal counted"
    );
}

/// **The kinds that need an `ot`** (#757's confirm round, N4), read from the two existing predicates.
#[test]
fn the_kinds_an_oidc_action_needs_an_exchanged_token_for() {
    let mut cfg = GossipConfig::default();
    cfg.protected_rpc_kinds = vec!["deploy.run".into()];
    for kind in ["mcp.invoke", "skill.invoke", "llm.invoke", "deploy.run", "blackboard.ops.post", "tuple.jobs.take", "wiki.c.ingest", "wiki.c.unknownverb"] {
        assert!(needs_exchanged_token(&cfg, kind), "{kind} is an action");
    }
    for kind in ["blackboard.ops.read", "blackboard.ops.depth", "blackboard.ops.snapshot", "tuple.jobs.depth", "tuple.jobs.wal_replay", "wiki.c.access", "k", "explain.request"] {
        assert!(!needs_exchanged_token(&cfg, kind), "{kind} is not an action");
    }
}

/// **The settings refused at start** (I5): `exchange_audience` without `exchange_client_id`, an empty audience, and a
/// lifetime outside `1..=3600`. Seen failing first with each check removed.
#[test]
fn the_exchange_settings_are_refused_by_name_when_incomplete() {
    let base = || {
        let mut cfg = GossipConfig::default();
        cfg.oidc = Some(crate::OidcConfig {
            issuer: ISSUER.into(),
            audience: "mycelium-cluster".into(),
            group_claim: "groups".into(),
            group_scopes: Default::default(),
            jwks_uri: None,
            exchange_audience: Some(AUDIENCE.into()),
            exchange_client_id: Some(CLIENT.into()),
            exchange_max_lifetime_secs: None,
        });
        cfg
    };
    assert!(base().validate().is_ok());
    let refused = |edit: &dyn Fn(&mut crate::OidcConfig)| {
        let mut cfg = base();
        edit(cfg.oidc.as_mut().unwrap());
        cfg.validate().err().map(|e| e.to_string()).unwrap_or_default()
    };
    assert!(refused(&|o| o.exchange_client_id = None).contains("oidc_exchange_client_unset"), "CLIENT");
    assert!(refused(&|o| o.exchange_audience = Some(" ".into())).contains("exchange_audience"), "AUDIENCE");
    assert!(refused(&|o| o.exchange_max_lifetime_secs = Some(0)).contains("exchange_max_lifetime_secs"), "LIFETIME 0");
    assert!(refused(&|o| o.exchange_max_lifetime_secs = Some(3_601)).contains("exchange_max_lifetime_secs"), "LIFETIME CAP");
    let mut without = base();
    without.oidc.as_mut().unwrap().exchange_audience = None;
    without.oidc.as_mut().unwrap().exchange_client_id = None;
    assert!(without.validate().is_ok(), "an [oidc] table without exchange_audience is unchanged");
}

/// **The cap stays at 8 KiB** (I5, decided 2026-11-11): an envelope whose `ot` would take it past
/// `MAX_ENVELOPE_BYTES` is refused by the producer (`caller_context_too_large`), never truncated.
#[tokio::test]
async fn an_envelope_too_large_with_its_exchanged_token_is_refused_by_the_producer() {
    let (port, _) = crate::test_util::spawn_counting_listener(JWKS).await;
    let a = provider(port, |_| {}).await;
    let huge = "x".repeat(super::gateway_caller::MAX_ENVELOPE_BYTES);
    let framed = frame_envelope_at(&a.task_ctx, a.node_id(), ALICE, &[], Bytes::new(), None, None, None, Some(&huge), 0);
    let fits = frame_envelope_at(&a.task_ctx, a.node_id(), ALICE, &[], Bytes::new(), None, None, None, Some(&"y".repeat(6_000)), 0);
    a.shutdown().await;
    assert!(framed.is_none(), "TOO LARGE: refused, not truncated");
    assert!(fits.is_some(), "a 6 000 B token fits, as the spike measured");
}
