//! **I5's exit against a real IdP**: an OIDC user's call through `/mcp`, exchanged by the gateway at Keycloak
//! (RFC 8693, DPoP-bound to the node key) and verified at the provider. A live suite: it runs when
//! `MYCELIUM_KEYCLOAK_URL` names a Keycloak (CI's `oidc-exchange-live` job starts one pinned by digest, the image the
//! spike ran, `quay.io/keycloak/keycloak@sha256:723b1d6e…`), skips itself otherwise, and fails instead of skipping when
//! `MYCELIUM_OIDC_EXCHANGE_LIVE_REQUIRED` is set. The gateway's client secret is `MYCELIUM_OIDC_EXCHANGE_SECRET`, which
//! the job sets and the test gives the realm's client: no test writes an environment variable.

use crate::{GossipAgent, GossipConfig, NodeId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TOOL: &str = "weather.forecast";

fn live() -> Option<(String, String)> {
    let url = std::env::var("MYCELIUM_KEYCLOAK_URL").ok().filter(|u| !u.is_empty());
    let secret = std::env::var(crate::config::DEFAULT_EXCHANGE_SECRET_ENV).ok().filter(|s| !s.is_empty());
    match (url, secret) {
        (Some(u), Some(s)) => Some((u.trim_end_matches('/').to_string(), s)),
        _ => {
            assert!(
                std::env::var("MYCELIUM_OIDC_EXCHANGE_LIVE_REQUIRED").is_err(),
                "MYCELIUM_OIDC_EXCHANGE_LIVE_REQUIRED is set, but MYCELIUM_KEYCLOAK_URL or MYCELIUM_OIDC_EXCHANGE_SECRET is not"
            );
            None
        }
    }
}

async fn call(http: &reqwest::Client, method: reqwest::Method, url: String, token: Option<&str>, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let mut req = http.request(method, url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.expect("keycloak reachable");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or_default())
}

/// A fresh realm: `user-app` (public; its tokens carry `mycelium-gateway` in `aud` and `groups: ["users"]`),
/// `mycelium-gateway` (confidential, `secret`, standard token exchange on; its tokens carry `mycelium:fleet`), the
/// target `mycelium:fleet`, and `alice`. Returns the realm's issuer.
async fn realm(http: &reqwest::Client, base: &str, secret: &str) -> String {
    let form = [("grant_type", "password"), ("client_id", "admin-cli"), ("username", "admin"), ("password", "admin")];
    let admin: serde_json::Value = http.post(format!("{base}/realms/master/protocol/openid-connect/token")).form(&form).send().await.unwrap().json().await.unwrap();
    let admin = admin["access_token"].as_str().expect("an admin token").to_string();
    let name = format!("i5-{}", mycelium_core::sim_seam::wall_now_ms());
    let a = format!("{base}/admin/realms/{name}");
    let post = |url: String, body: serde_json::Value| call(http, reqwest::Method::POST, url, Some(&admin), Some(body));
    assert_eq!(post(format!("{base}/admin/realms"), serde_json::json!({"realm": name, "enabled": true})).await.0, 201);
    let aud = |n: &str, a: &str| serde_json::json!({"name": n, "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper", "config": {"included.client.audience": a, "access.token.claim": "true"}});
    let groups = serde_json::json!({"name": "groups", "protocol": "openid-connect", "protocolMapper": "oidc-hardcoded-claim-mapper",
        "config": {"claim.name": "groups", "claim.value": "[\"users\"]", "jsonType.label": "JSON", "access.token.claim": "true"}});
    for body in [
        serde_json::json!({"clientId": "mycelium:fleet", "publicClient": false, "secret": "unused", "standardFlowEnabled": false}),
        serde_json::json!({"clientId": "user-app", "publicClient": true, "directAccessGrantsEnabled": true, "standardFlowEnabled": false, "protocolMappers": [aud("aud-gateway", "mycelium-gateway"), groups]}),
        serde_json::json!({"clientId": "mycelium-gateway", "publicClient": false, "secret": secret, "serviceAccountsEnabled": true, "standardFlowEnabled": false,
            "attributes": {"standard.token.exchange.enabled": "true"}, "protocolMappers": [aud("aud-fleet", "mycelium:fleet")]}),
    ] {
        assert_eq!(post(format!("{a}/clients"), body).await.0, 201);
    }
    let alice = serde_json::json!({"username": "alice", "enabled": true, "email": "alice@example.org", "emailVerified": true, "firstName": "Alice", "lastName": "Example",
        "credentials": [{"type": "password", "value": "alice", "temporary": false}]});
    assert_eq!(post(format!("{a}/users"), alice).await.0, 201);
    format!("{base}/realms/{name}")
}

/// **An OIDC user's call verified at the provider, against Keycloak** (I5's exit). Alice's token from Keycloak reaches
/// `/mcp`; the gateway exchanges it at Keycloak's token endpoint under its client credential with a DPoP proof (EdDSA,
/// its node key); the provider verifies the DPoP-bound token against Keycloak's JWKS and the key that signed the
/// envelope, and the tool runs. Twice, each call its own exchange.
#[tokio::test]
async fn oidc_exchange_live_an_oidc_users_call_is_verified_at_the_provider() {
    let Some((base, secret)) = live() else { return };
    let http = reqwest::Client::new();
    let issuer = realm(&http, &base, &secret).await;
    let port = crate::test_util::alloc_port();
    let http_port = crate::test_util::alloc_port();
    let dir = std::env::temp_dir().join(format!("i5-live-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir, ..Default::default() });
    let mut group_scopes = std::collections::HashMap::new();
    group_scopes.insert("users".to_string(), vec!["mcp:invoke".to_string()]);
    cfg.oidc = Some(crate::OidcConfig {
        issuer: issuer.clone(),
        audience: "mycelium-gateway".into(),
        group_claim: "groups".into(),
        group_scopes,
        jwks_uri: None,
        exchange_audience: Some("mycelium:fleet".into()),
        exchange_client_id: Some("mycelium-gateway".into()),
        exchange_max_lifetime_secs: None,
        exchange_token_endpoint: Some(format!("{issuer}/protocol/openid-connect/token")),
        exchange_client_secret_env: None,
    });
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await.expect("starts");
    let ran = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&ran);
    std::mem::forget(a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| {
        let r = Arc::clone(&r);
        async move {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("sunny"))
        }
    }));
    let form = [("grant_type", "password"), ("client_id", "user-app"), ("username", "alice"), ("password", "alice")];
    let user: serde_json::Value = http.post(format!("{issuer}/protocol/openid-connect/token")).form(&form).send().await.unwrap().json().await.unwrap();
    let bearer = user["access_token"].as_str().expect("alice's token").to_string();
    let mcp = |id: u64| {
        let (http, bearer) = (http.clone(), bearer.clone());
        async move {
            let body = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": TOOL, "arguments": {"city": "Lyon"}}});
            call(&http, reqwest::Method::POST, format!("http://127.0.0.1:{http_port}/mcp"), Some(&bearer), Some(body)).await.1
        }
    };
    // The provider fetched Keycloak's keys at start; a call that arrives first is refused retriably.
    let mut first = serde_json::Value::Null;
    for i in 0..50 {
        first = mcp(i).await;
        if first.get("result").is_some() || !first.to_string().contains("ot_keys_unavailable") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let second = mcp(100).await;
    let runs = ran.load(Ordering::SeqCst);
    // Each call's exchanged token admitted once at the provider's replay window: verified there, not only at the door.
    let (admitted, _) = a.task_ctx.exchanged_token_replay.held();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("i5-live-{port}")));
    assert!(first.get("result").is_some(), "VERIFIED AT THE PROVIDER: {first}");
    assert!(second.get("result").is_some(), "a second call, its own exchange: {second}");
    assert_eq!(runs, 2);
    assert_eq!(admitted, 2, "two exchanged tokens, each admitted once at the provider");
}
