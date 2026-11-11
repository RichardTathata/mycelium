//! Identity-and-lifecycle I5, PR 2: the gateway's token exchange, in process. One node is the gateway and the provider;
//! the IdP is a mock — its JWKS the RSA test fixture, its token endpoint binding what it issues to the key the DPoP
//! proof names, as Keycloak 26.8.2 does (spike S-exchange). The exit's run against a real Keycloak is
//! `oidc_exchange_live_tests`.

use super::exchanged_token::ed25519_jwk_thumbprint;
use super::oidc_exchange::dpop_proof;
use crate::{GossipAgent, GossipConfig, NodeId};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TEST_PRIV: &str = include_str!("../../tests/fixtures/oidc_test.key");
const JWKS: &str = include_str!("../../tests/fixtures/oidc_jwks.json");
const AUDIENCE: &str = "mycelium:fleet";
const CLIENT: &str = "mycelium-gateway";
const TOOL: &str = "weather.forecast";

fn now_s() -> u64 {
    mycelium_core::sim_seam::wall_now_ms() / 1_000
}

fn mint(claims: &serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".into());
    encode(&header, claims, &EncodingKey::from_rsa_pem(TEST_PRIV.as_bytes()).unwrap()).unwrap()
}

fn b64u_json(part: &str) -> serde_json::Value {
    use base64::Engine as _;
    serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

/// What the mock IdP saw, and how it answers.
#[derive(Default)]
struct Idp {
    exchanges: AtomicUsize,
    refuse: std::sync::atomic::AtomicBool,
    /// Answer 200 with a body that holds no token.
    malformed: std::sync::atomic::AtomicBool,
    last: parking_lot::Mutex<Option<(std::collections::HashMap<String, String>, String)>>,
}

/// A mock IdP on loopback: `/jwks` the fixture; `/token` an RFC 8693 exchange that checks the client credential and
/// binds the token it issues to the DPoP proof's key, for the subject token's `sub`.
async fn idp() -> (String, Arc<Idp>) {
    use axum::routing::{get, post};
    let state = Arc::new(Idp::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let issuer = base.clone();
    let s = Arc::clone(&state);
    let router = axum::Router::new()
        .route("/jwks", get(|| async { ([("content-type", "application/json")], JWKS) }))
        .route(
            "/token",
            post(move |headers: axum::http::HeaderMap, axum::Form(form): axum::Form<std::collections::HashMap<String, String>>| {
                let s = Arc::clone(&s);
                let issuer = issuer.clone();
                async move {
                    s.exchanges.fetch_add(1, Ordering::SeqCst);
                    let dpop = headers.get("dpop").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                    *s.last.lock() = Some((form.clone(), dpop.clone()));
                    let basic = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
                    use base64::Engine as _;
                    let want = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{CLIENT}:{}", secret())));
                    if s.refuse.load(Ordering::SeqCst) || basic != want || dpop.is_empty() {
                        return (axum::http::StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error": "invalid_request"})));
                    }
                    if s.malformed.load(Ordering::SeqCst) {
                        return (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"token_type": "DPoP"})));
                    }
                    let jwk = &b64u_json(dpop.split('.').next().unwrap())["jwk"];
                    let key: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap().try_into().unwrap();
                    let sub = b64u_json(form["subject_token"].split('.').nth(1).unwrap())["sub"].clone();
                    let now = now_s();
                    let token = mint(&serde_json::json!({
                        "iss": issuer, "aud": AUDIENCE, "azp": CLIENT, "sub": sub, "iat": now, "exp": now + 300,
                        "jti": format!("x-{}", s.exchanges.load(Ordering::SeqCst)), "cnf": {"jkt": ed25519_jwk_thumbprint(&key)},
                    }));
                    (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"access_token": token, "token_type": "DPoP"})))
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, state)
}

/// A node that is both the gateway (exchanging) and the provider (verifying), serving `TOOL` over MCP.
async fn node(idp: &str, http_port: u16, tweak: impl FnOnce(&mut GossipConfig)) -> Result<(Arc<GossipAgent>, Arc<AtomicUsize>), crate::GossipError> {
    let port = crate::test_util::alloc_port();
    let dir = std::env::temp_dir().join(format!("i5-x-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir, ..Default::default() });
    let mut group_scopes = std::collections::HashMap::new();
    group_scopes.insert("users".to_string(), vec!["mcp:invoke".to_string()]);
    cfg.oidc = Some(crate::OidcConfig {
        issuer: idp.to_string(),
        audience: "mycelium-cluster".into(),
        group_claim: "groups".into(),
        group_scopes,
        jwks_uri: Some(format!("{idp}/jwks")),
        exchange_audience: Some(AUDIENCE.into()),
        exchange_client_id: Some(CLIENT.into()),
        exchange_max_lifetime_secs: None,
        exchange_token_endpoint: Some(format!("{idp}/token")),
        exchange_client_secret_env: Some(SECRET_ENV.into()),
    });
    tweak(&mut cfg);
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await?;
    let ran = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&ran);
    let handle = a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| {
        let r = Arc::clone(&r);
        async move {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("sunny"))
        }
    });
    std::mem::forget(handle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok((a, ran))
}

/// The user's own bearer: a gateway-audience token for `alice`, in the `users` group.
fn user_token(idp: &str) -> String {
    let now = now_s();
    mint(&serde_json::json!({"iss": idp, "aud": "mycelium-cluster", "sub": "alice", "groups": ["users"], "iat": now, "exp": now + 300}))
}

async fn mcp_call(http_port: u16, bearer: &str) -> serde_json::Value {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": TOOL, "arguments": {"city": "Lyon"}}});
    let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{http_port}/mcp")).bearer_auth(bearer).json(&body).send().await.unwrap();
    resp.json().await.unwrap_or_default()
}

/// The client secret's variable in these tests: one the test process always has, so no test sets an environment
/// variable (`deny(unsafe_code)`); the mock IdP expects its value.
const SECRET_ENV: &str = "PATH";

fn secret() -> String {
    std::env::var(SECRET_ENV).unwrap_or_default()
}

/// **An OIDC user's call is exchanged at the gateway and verified at the provider** (I5's exit, in process). The user's
/// bearer reaches `/mcp`; the gateway exchanges it once, with a DPoP proof under its node key and its client
/// credential; the provider verifies the token bound to that key and runs the tool. A refused exchange answers
/// `oidc_exchange_refused` and runs nothing. Seen failing first with the exchange removed from the dispatch (the call
/// refused `ot_required` at the provider).
#[tokio::test]
async fn an_oidc_users_call_is_exchanged_at_the_gateway_and_verified_at_the_provider() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, ran) = node(&issuer, http_port, |_| {}).await.expect("starts");
    let bearer = user_token(&issuer);
    let mut answer = serde_json::Value::Null;
    for _ in 0..20 {
        answer = mcp_call(http_port, &bearer).await;
        if answer.get("result").is_some() || !answer.to_string().contains("ot_keys_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let exchanges = state.exchanges.load(Ordering::SeqCst);
    let (form, dpop) = state.last.lock().clone().expect("an exchange was made");
    state.refuse.store(true, Ordering::SeqCst);
    let refused = mcp_call(http_port, &bearer).await;
    let ran_after = ran.load(Ordering::SeqCst);
    let key = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
    a.shutdown().await;
    assert!(answer.get("result").is_some(), "VERIFIED: {answer}");
    assert_eq!(ran_after, 1, "the tool ran once");
    assert!(exchanges >= 1);
    assert_eq!(form.get("grant_type").map(String::as_str), Some("urn:ietf:params:oauth:grant-type:token-exchange"));
    assert_eq!(form.get("audience").map(String::as_str), Some(AUDIENCE));
    assert_eq!(form.get("subject_token").map(String::as_str), Some(bearer.as_str()), "the user's own token is what is exchanged");
    assert_eq!(ed25519_jwk_thumbprint(&key), {
        use base64::Engine as _;
        let jwk = &b64u_json(dpop.split('.').next().unwrap())["jwk"];
        let x: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap().try_into().unwrap();
        ed25519_jwk_thumbprint(&x)
    }, "the DPoP proof names the node key");
    assert!(refused.to_string().contains("oidc_exchange_refused"), "REFUSED: {refused}");
}

/// **Only an action is exchanged for** (the maintainer's decision, 2026-11-11). A dispatch of a kind that is not an
/// action carries no token and makes no exchange; a non-OIDC principal's action makes none either. Seen failing first
/// with the kind check removed (an exchange made for every call), and with the principal check removed (the service's
/// call refused for want of a user's token).
#[tokio::test]
async fn only_an_oidc_users_action_is_exchanged_for() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, _ran) = node(&issuer, http_port, |_| {}).await.expect("starts");
    let alice = super::gateway_caller::ResolvedPrincipal {
        principal: format!("oidc:{issuer}/alice"),
        scopes: vec!["mesh:write".into()],
        subject_token: Some(super::gateway_caller::SubjectToken(Arc::from(user_token(&issuer)))),
    };
    let token_holder = super::gateway_caller::ResolvedPrincipal { principal: "token:fleet/ops".into(), scopes: vec![], subject_token: None };
    let call = |caller: super::gateway_caller::ResolvedPrincipal, kind: &'static str| {
        let ctx = Arc::clone(&a.task_ctx);
        let me = a.node_id().clone();
        async move {
            super::gateway_caller::gateway_rpc_call(&ctx, Some(&caller), me, Arc::from(kind), bytes::Bytes::from_static(b"{}"), Duration::from_millis(300))
                .await
                .err()
                .and_then(|e| matches!(e, super::gateway_caller::GatewayDispatchError::Exchange(_)).then(|| e.to_string()))
        }
    };
    let refused_by_exchange = [
        call(alice.clone(), "not.an.action").await,
        call(alice.clone(), "blackboard.ops.read").await,
        call(token_holder, "mcp.invoke").await,
    ];
    let none_yet = state.exchanges.load(Ordering::SeqCst);
    call(alice, "mcp.invoke").await;
    let after_action = state.exchanges.load(Ordering::SeqCst);
    a.shutdown().await;
    assert_eq!((none_yet, after_action), (0, 1), "ONLY ACTIONS: one exchange, for the oidc: action");
    assert_eq!(refused_by_exchange, [None, None, None], "no other call is refused for want of an exchange");
}

/// **The exchange's settings are refused by name at start** (I5's exit): on a gateway with `exchange_audience`, the
/// token endpoint unset (`oidc_exchange_endpoint_unset`), the client unset (`oidc_exchange_client_unset`), the
/// secret's variable unset or empty (`oidc_exchange_secret_unset`), a token endpoint the egress allow-list does not
/// permit. A provider-only node (no gateway port) needs none
/// of them, and an `[oidc]` gateway without `exchange_audience` is unchanged. Seen failing first with each check
/// removed.
#[tokio::test]
async fn the_exchange_settings_are_refused_by_name_at_start() {
    let (issuer, _state) = idp().await;
    let started = |r: Result<(Arc<GossipAgent>, Arc<AtomicUsize>), crate::GossipError>| match r {
        Ok((a, _)) => {
            tokio::spawn(async move { a.shutdown().await });
            String::new()
        }
        Err(e) => e.to_string(),
    };
    let secret_unset = started(node(&issuer, crate::test_util::alloc_port(), |c| c.oidc.as_mut().unwrap().exchange_client_secret_env = Some("I5_SURELY_UNSET_VARIABLE_9C2E".into())).await);
    let endpoint = started(node(&issuer, crate::test_util::alloc_port(), |c| c.oidc.as_mut().unwrap().exchange_token_endpoint = None).await);
    let client = started(node(&issuer, crate::test_util::alloc_port(), |c| c.oidc.as_mut().unwrap().exchange_client_id = None).await);
    // The IdP's keys on loopback, which the list permits; its token endpoint on a host it does not.
    let egress = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        c.egress.allow_hosts = vec!["127.0.0.1".into()];
        c.oidc.as_mut().unwrap().exchange_token_endpoint = Some("http://idp-elsewhere.example/token".into());
    }).await);
    let provider_only = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        c.http_port = None;
        let o = c.oidc.as_mut().unwrap();
        o.exchange_token_endpoint = None;
        o.exchange_client_secret_env = Some("I5_SURELY_UNSET_VARIABLE_9C2E".into());
    }).await);
    let unchanged = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        let o = c.oidc.as_mut().unwrap();
        o.exchange_audience = None;
        o.exchange_client_id = None;
        o.exchange_token_endpoint = None;
    }).await);
    // An empty variable, through the loader's seam: no test sets one (`deny(unsafe_code)`).
    let mut cfg = GossipConfig::default();
    cfg.http_port = Some(1);
    cfg.oidc = Some(crate::OidcConfig {
        issuer: issuer.clone(),
        audience: "a".into(),
        exchange_audience: Some(AUDIENCE.into()),
        exchange_client_id: Some(CLIENT.into()),
        exchange_token_endpoint: Some(format!("{issuer}/token")),
        ..Default::default()
    });
    let secret_empty = super::oidc_exchange::OidcExchanger::load_with(&cfg, |_| Some(String::new())).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(secret_unset.contains("oidc_exchange_secret_unset"), "SECRET UNSET: {secret_unset}");
    assert!(secret_empty.contains("oidc_exchange_secret_unset"), "SECRET EMPTY: {secret_empty}");
    assert!(endpoint.contains("oidc_exchange_endpoint_unset"), "ENDPOINT: {endpoint}");
    assert!(client.contains("oidc_exchange_client_unset"), "CLIENT: {client}");
    assert!(egress.contains("does not permit the token endpoint"), "EGRESS: {egress}");
    assert_eq!((provider_only.as_str(), unchanged.as_str()), ("", ""), "a provider-only node, and an [oidc] gateway without exchange_audience, start");
}

/// **The DPoP proof** (RFC 9449, I5): an EdDSA JWS whose JWK is the node key, over `htm`, `htu`, `iat` and `jti`,
/// verifying under that key; its RFC 7638 thumbprint is what the provider compares `cnf.jkt` with.
#[test]
fn the_dpop_proof_is_an_eddsa_jws_under_the_node_key() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let proof = dpop_proof(&key, "https://idp.example/token", 1_794_000_000, 42);
    let parts: Vec<&str> = proof.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert!(proof.starts_with("eyJ"), "a JWS signing input, none of a Mycelium message's three shapes");
    let header = b64u_json(parts[0]);
    let payload = b64u_json(parts[1]);
    assert_eq!((header["typ"].as_str(), header["alg"].as_str(), header["jwk"]["crv"].as_str()), (Some("dpop+jwt"), Some("EdDSA"), Some("Ed25519")));
    assert_eq!((payload["htm"].as_str(), payload["htu"].as_str(), payload["iat"].as_u64()), (Some("POST"), Some("https://idp.example/token"), Some(1_794_000_000)));
    use base64::Engine as _;
    let sig: [u8; 64] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[2]).unwrap().try_into().unwrap();
    assert!(crate::tls::verify_bytes(&key.verifying_key().to_bytes(), format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig));
}

/// **An exchange that does not yield a token is refused by name, and nothing runs** (I5, PR 2). An IdP that answers 200
/// without an `access_token` is `oidc_exchange_malformed`; a token endpoint nothing listens on is
/// `oidc_exchange_unreachable`. Seen failing first: with each failure mapped to a forwarded call without an `ot`, the
/// provider answered `ot_required` instead.
#[tokio::test]
async fn an_exchange_that_yields_no_token_is_refused_by_name() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, ran) = node(&issuer, http_port, |_| {}).await.expect("starts");
    state.malformed.store(true, Ordering::SeqCst);
    let malformed = mcp_call(http_port, &user_token(&issuer)).await;
    let ran_malformed = ran.load(Ordering::SeqCst);
    a.shutdown().await;
    let closed = crate::test_util::alloc_port();
    let http_port = crate::test_util::alloc_port();
    let (b, ran_b) = node(&issuer, http_port, |c| {
        c.oidc.as_mut().unwrap().exchange_token_endpoint = Some(format!("http://127.0.0.1:{closed}/token"));
    })
    .await
    .expect("starts");
    let unreachable = mcp_call(http_port, &user_token(&issuer)).await;
    let ran_unreachable = ran_b.load(Ordering::SeqCst);
    b.shutdown().await;
    assert!(malformed.to_string().contains("oidc_exchange_malformed"), "MALFORMED: {malformed}");
    assert!(unreachable.to_string().contains("oidc_exchange_unreachable"), "UNREACHABLE: {unreachable}");
    assert_eq!((ran_malformed, ran_unreachable), (0, 0), "nothing ran");
}
