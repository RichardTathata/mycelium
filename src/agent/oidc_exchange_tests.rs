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
const MODE_REDIRECT: usize = 1;
const MODE_HUGE: usize = 2;
const MODE_NONCE: usize = 3;
const NONCE: &str = "server-nonce-1";

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
    /// How `/token` answers otherwise: `MODE_*`.
    mode: AtomicUsize,
    /// Calls that reached `/token2`, the redirect's target.
    redirect_followed: AtomicUsize,
    /// Whether a nonce challenge was answered already (the first call in `MODE_NONCE` is challenged).
    challenged: std::sync::atomic::AtomicBool,
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
                async move { token_endpoint(&s, &issuer, &headers, &form) }
            }),
        )
        .route(
            "/token2",
            post({
                let s = Arc::clone(&state);
                move || {
                    let s = Arc::clone(&s);
                    async move {
                        s.redirect_followed.fetch_add(1, Ordering::SeqCst);
                        (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"access_token": "followed"})))
                    }
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, state)
}

/// The mock IdP's token endpoint: the client credential read per RFC 6749 §2.3.1 (each half form-urldecoded), the DPoP
/// proof's key bound into the token it issues, and the `MODE_*` it is set to.
fn token_endpoint(s: &Idp, issuer: &str, headers: &axum::http::HeaderMap, form: &std::collections::HashMap<String, String>) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    use base64::Engine as _;
    s.exchanges.fetch_add(1, Ordering::SeqCst);
    let dpop = headers.get("dpop").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    *s.last.lock() = Some((form.clone(), dpop.clone()));
    let credential = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|raw| {
            let (id, secret) = raw.split_once(':')?;
            let dec = |v: &str| url::form_urlencoded::parse(format!("v={v}").as_bytes()).next().map(|(_, v)| v.into_owned());
            Some((dec(id)?, dec(secret)?))
        });
    let bad = || (axum::http::StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error": "invalid_request"}))).into_response();
    if s.refuse.load(Ordering::SeqCst) || credential != Some((CLIENT.to_string(), secret())) || dpop.is_empty() {
        return bad();
    }
    if s.malformed.load(Ordering::SeqCst) {
        return (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"token_type": "DPoP"}))).into_response();
    }
    let mode = s.mode.load(Ordering::SeqCst);
    if mode == MODE_REDIRECT {
        return (axum::http::StatusCode::TEMPORARY_REDIRECT, [("location", format!("{issuer}/token2"))]).into_response();
    }
    let claims = b64u_json(dpop.split('.').nth(1).unwrap_or_default());
    if mode == MODE_NONCE && claims.get("nonce").and_then(|n| n.as_str()) != Some(NONCE) {
        if s.challenged.swap(true, Ordering::SeqCst) {
            return bad();
        }
        return (axum::http::StatusCode::BAD_REQUEST, [("dpop-nonce", NONCE)], axum::Json(serde_json::json!({"error": "use_dpop_nonce"}))).into_response();
    }
    let jwk = &b64u_json(dpop.split('.').next().unwrap())["jwk"];
    let key: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap().try_into().unwrap();
    let sub = b64u_json(form["subject_token"].split('.').nth(1).unwrap())["sub"].clone();
    let now = now_s();
    let token = mint(&serde_json::json!({
        "iss": issuer, "aud": AUDIENCE, "azp": CLIENT, "sub": sub, "iat": now, "exp": now + 300,
        "jti": format!("x-{}", s.exchanges.load(Ordering::SeqCst)), "cnf": {"jkt": ed25519_jwk_thumbprint(&key)},
    }));
    let mut body = serde_json::json!({"access_token": token, "token_type": "DPoP"});
    if mode == MODE_HUGE {
        body["padding"] = "x".repeat(100 * 1024).into();
    }
    (axum::http::StatusCode::OK, axum::Json(body)).into_response()
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

/// **The exchange's settings are refused by name at start** (I5's exit; the maintainer's decision of 2026-11-11, #760's
/// review M1: a node **exchanges iff `exchange_token_endpoint` is set**, and verifies iff `exchange_audience` is). With
/// the endpoint set: the audience unset (`oidc_exchange_audience_unset`), the client unset (`oidc_exchange_client_unset`),
/// an endpoint neither https nor loopback (`oidc_exchange_endpoint_insecure`, L2), the secret's variable unset or empty
/// (`oidc_exchange_secret_unset`), an endpoint the egress allow-list does not permit. Without it, nothing is asked: a
/// verifying node with an HTTP port starts with no secret, and so does an unchanged `[oidc]` gateway; under
/// `gateway_caller_profile = legacy` the settings are inert and not demanded (L6). Seen failing first with each check
/// removed, and (M1) with `http_port` deciding.
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
    let unset_secret = |c: &mut GossipConfig| c.oidc.as_mut().unwrap().exchange_client_secret_env = Some("I5_SURELY_UNSET_VARIABLE_9C2E".into());
    let secret_unset = started(node(&issuer, crate::test_util::alloc_port(), unset_secret).await);
    let audience = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        let o = c.oidc.as_mut().unwrap();
        o.exchange_audience = None;
    }).await);
    let client = started(node(&issuer, crate::test_util::alloc_port(), |c| c.oidc.as_mut().unwrap().exchange_client_id = None).await);
    let insecure = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        c.oidc.as_mut().unwrap().exchange_token_endpoint = Some("http://idp.example/token".into());
    }).await);
    // The IdP's keys on loopback, which the list permits; its token endpoint on a host it does not.
    let egress = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        c.egress.allow_hosts = vec!["127.0.0.1".into()];
        c.oidc.as_mut().unwrap().exchange_token_endpoint = Some("https://idp-elsewhere.example/token".into());
    }).await);
    // M1: no endpoint, no exchange — whatever the HTTP port says.
    let verifies_only = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        unset_secret(c);
        c.oidc.as_mut().unwrap().exchange_token_endpoint = None;
    }).await);
    let unchanged = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        let o = c.oidc.as_mut().unwrap();
        o.exchange_audience = None;
        o.exchange_client_id = None;
        o.exchange_token_endpoint = None;
    }).await);
    let legacy = started(node(&issuer, crate::test_util::alloc_port(), |c| {
        unset_secret(c);
        c.gateway_caller_profile = crate::config::GatewayCallerProfile::Legacy;
    }).await);
    // An empty variable, through the loader's seam: no test sets one (`deny(unsafe_code)`).
    let mut cfg = GossipConfig::default();
    cfg.oidc = Some(crate::OidcConfig {
        issuer: issuer.clone(),
        audience: "a".into(),
        exchange_audience: Some(AUDIENCE.into()),
        exchange_client_id: Some(CLIENT.into()),
        exchange_token_endpoint: Some(format!("{issuer}/token")),
        ..Default::default()
    });
    let secret_empty = super::oidc_exchange::OidcExchanger::load_with(&cfg, |_| Some(String::new())).err().map(|e| e.to_string()).unwrap_or_default();
    // M1: the endpoint decides, not the HTTP port — `cfg` has none, and still exchanges.
    let no_http_port_exchanges = super::oidc_exchange::OidcExchanger::load_with(&cfg, |_| Some("s".into())).ok().flatten().is_some();
    assert!(secret_unset.contains("oidc_exchange_secret_unset"), "SECRET UNSET: {secret_unset}");
    assert!(secret_empty.contains("oidc_exchange_secret_unset"), "SECRET EMPTY: {secret_empty}");
    assert!(audience.contains("oidc_exchange_audience_unset"), "AUDIENCE: {audience}");
    assert!(client.contains("oidc_exchange_client_unset"), "CLIENT: {client}");
    assert!(insecure.contains("oidc_exchange_endpoint_insecure"), "INSECURE: {insecure}");
    assert!(egress.contains("does not permit the token endpoint"), "EGRESS: {egress}");
    assert!(no_http_port_exchanges, "M1: a node without an HTTP port exchanges when its endpoint is set");
    assert_eq!(
        (verifies_only.as_str(), unchanged.as_str(), legacy.as_str()),
        ("", "", ""),
        "a verifying node without an endpoint (an HTTP port, no secret), an [oidc] gateway without exchange_audience, and a legacy gateway, start"
    );
}

/// **A node may verify without exchanging** (the maintainer's decision of 2026-11-11, #760's review M1). With
/// `exchange_audience` and no `exchange_token_endpoint`, the node verifies and exchanges nothing — so an OIDC user's
/// action through its own gateway, toward a verifying provider (here itself), is refused `ot_required` — and its
/// guarantees say which half it runs: `id.oidc_exchanged_token` enforced, `id.oidc_exchange` not configured. Seen
/// failing first with `http_port` deciding (the start refused `oidc_exchange_secret_unset`... then with the guarantee
/// one entry).
#[tokio::test]
async fn a_node_verifies_without_exchanging_when_no_endpoint_is_set() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, ran) = node(&issuer, http_port, |c| {
        let o = c.oidc.as_mut().unwrap();
        o.exchange_token_endpoint = None;
        o.exchange_client_secret_env = Some("I5_SURELY_UNSET_VARIABLE_9C2E".into());
    })
    .await
    .expect("a verifying node without an endpoint starts, with no secret");
    let mut answer = serde_json::Value::Null;
    for _ in 0..20 {
        answer = mcp_call(http_port, &user_token(&issuer)).await;
        if !answer.to_string().contains("ot_keys_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let report = a.guarantee_report();
    let state_of = |id: &str| report.entries.iter().find(|e| e.id == id).map(|e| e.resolution.state()).unwrap_or("absent");
    let (exchange, verify) = (state_of("id.oidc_exchange"), state_of("id.oidc_exchanged_token"));
    let runs = ran.load(Ordering::SeqCst);
    a.shutdown().await;
    assert!(answer.to_string().contains("ot_required"), "VERIFIES, DOES NOT EXCHANGE: {answer}");
    assert_eq!((state.exchanges.load(Ordering::SeqCst), runs), (0, 0));
    assert_eq!((exchange, verify), ("not_configured", "enforced"), "the two halves reported apart");
}

/// **The exchange's transport** (#760's review, M3, L1, L3). A token endpoint answering 307 to another that would issue
/// a token is not followed (`oidc_exchange_unreachable`); an answer past 64 KiB is `oidc_exchange_too_large`; one
/// `use_dpop_nonce` challenge is answered with its nonce, and the call is verified. Seen failing first with the client
/// following redirects, with the body unbounded, and with the challenge refused.
#[tokio::test]
async fn the_exchange_follows_no_redirect_bounds_its_answer_and_answers_one_nonce_challenge() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, ran) = node(&issuer, http_port, |_| {}).await.expect("starts");
    let bearer = user_token(&issuer);
    let call = || mcp_call(http_port, &bearer);
    let mut warm = serde_json::Value::Null;
    for _ in 0..20 {
        warm = call().await;
        if !warm.to_string().contains("ot_keys_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    state.mode.store(MODE_REDIRECT, Ordering::SeqCst);
    let redirected = call().await;
    state.mode.store(MODE_HUGE, Ordering::SeqCst);
    let huge = call().await;
    state.mode.store(MODE_NONCE, Ordering::SeqCst);
    let nonce = call().await;
    let nonce_proof = state.last.lock().clone().map(|(_, dpop)| b64u_json(dpop.split('.').nth(1).unwrap())["nonce"].clone());
    let runs = ran.load(Ordering::SeqCst);
    a.shutdown().await;
    assert!(warm.get("result").is_some(), "the honest call first: {warm}");
    assert!(redirected.to_string().contains("oidc_exchange_unreachable"), "REDIRECT NOT FOLLOWED: {redirected}");
    assert_eq!(state.redirect_followed.load(Ordering::SeqCst), 0, "the redirect's target saw nothing");
    assert!(huge.to_string().contains("oidc_exchange_too_large"), "BOUNDED: {huge}");
    assert!(nonce.get("result").is_some(), "NONCE: the challenge answered once: {nonce}");
    assert_eq!(nonce_proof, Some(serde_json::json!(NONCE)), "the retried proof carries the server's nonce");
    assert_eq!(runs, 2, "the honest call and the nonce-answered call ran; the others did not");
}

/// **The DPoP `htu` and the client credential** (RFC 9449 §4.2, RFC 6749 §2.3.1; #760's review, L3). `htu` drops the
/// endpoint's query and fragment; the client id and secret are form-urlencoded before the Basic encoding, so a secret
/// holding `%`, `+`, `:` or a space reaches the IdP as it is. Seen failing first: `htu` kept the query, and the secret
/// was encoded raw.
#[test]
fn the_dpop_htu_and_the_client_credential_are_encoded_as_the_rfcs_say() {
    use super::oidc_exchange::{client_basic, dpop_htu};
    use base64::Engine as _;
    assert_eq!(dpop_htu("https://idp.example/realms/r/token?x=1#frag"), "https://idp.example/realms/r/token");
    let basic = client_basic("my gateway", "p%+: s");
    let raw = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(basic.strip_prefix("Basic ").unwrap()).unwrap()).unwrap();
    assert_eq!(raw, "my+gateway:p%25%2B%3A+s");
    // The IdP's reading (decode each half): what was configured.
    let (id, secret) = raw.split_once(':').unwrap();
    let decode = |v: &str| url::form_urlencoded::parse(format!("v={v}").as_bytes()).next().unwrap().1.into_owned();
    assert_eq!((decode(id), decode(secret)), ("my gateway".to_string(), "p%+: s".to_string()));
}

/// **The DPoP proof** (RFC 9449, I5): an EdDSA JWS whose JWK is the node key, over `htm`, `htu`, `iat` and `jti`,
/// verifying under that key; its RFC 7638 thumbprint is what the provider compares `cnf.jkt` with.
#[test]
fn the_dpop_proof_is_an_eddsa_jws_under_the_node_key() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let proof = dpop_proof(&key, "https://idp.example/token", 1_794_000_000, 42, None);
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

/// **`/a2a` exchanges as `/mcp` does** (#760's review, H1). An OIDC user's bearer at `/a2a` — `tasks/send` and
/// `tasks/sendSubscribe` — is a skill call (`skill.invoke`), an action: the gateway exchanges the user's token and the
/// provider verifies it, and the skill runs once each. Seen failing first: the A2A door resolved the user without their
/// token, so both were refused at the gateway `oidc_exchange_no_subject_token`.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn an_oidc_users_a2a_call_is_exchanged_on_send_and_stream() {
    let (issuer, state) = idp().await;
    let http_port = crate::test_util::alloc_port();
    let (a, _ran) = node(&issuer, http_port, |c| c.gateway_a2a = true).await.expect("starts");
    let _reg = a.capabilities().advertise_capability(crate::capability::Capability::new("weather", "forecast"), Duration::from_secs(30));
    let skill_ran = Arc::new(AtomicUsize::new(0));
    {
        let (agent, ran) = (Arc::clone(&a), Arc::clone(&skill_ran));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                ran.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"sunny".to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let bearer = user_token(&issuer);
    let a2a = |method: &'static str, id: String| {
        let bearer = bearer.clone();
        async move {
            reqwest::Client::new()
                .post(format!("http://127.0.0.1:{http_port}/a2a"))
                .bearer_auth(&bearer)
                .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": {
                    "id": id, "skillId": "weather/forecast",
                    "message": {"role": "user", "parts": [{"type": "text", "text": "Lyon"}]}}}))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }
    };
    let mut sent = String::new();
    for i in 0..20 {
        sent = a2a("tasks/send", format!("t-send-{i}")).await;
        if !sent.contains("ot_keys_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let streamed = a2a("tasks/sendSubscribe", "t-stream".to_string()).await;
    let runs = skill_ran.load(Ordering::SeqCst);
    let exchanges = state.exchanges.load(Ordering::SeqCst);
    a.shutdown().await;
    assert!(sent.contains("\"completed\""), "SEND: {sent}");
    assert!(streamed.contains("\"completed\""), "STREAM: {streamed}");
    assert!(!sent.contains("oidc_exchange") && !streamed.contains("oidc_exchange"), "no exchange refusal: {sent} / {streamed}");
    assert_eq!(runs, 2, "the skill ran once per call, verified at the provider");
    assert!(exchanges >= 2, "each call exchanged: {exchanges}");
}
