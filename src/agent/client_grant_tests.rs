//! Identity-and-lifecycle I1 (second half, PR 2): a grant **held by a client principal**, end to end, on a node whose
//! authority comes from its declaration — the client asks at `POST /gateway/grants/request`, the supervisor issues at
//! the existing door, the client proves possession with its client-certificate key at `/mcp`, and the supervisor
//! revokes it against the tool's provider.

use crate::config::{AuthorityConfig, AuthorityTier, GatewayNamedToken, GrantEntitlement, GossipConfig, TlsConfig};
use crate::test_util::alloc_port;
use crate::{GossipAgent, NodeId};
use rustls::pki_types::pem::PemObject;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const ISSUER: &str = "fleet";
const FORECASTER: &str = "token:fleet/forecaster";
const TOOL: &str = "weather.forecast";

/// This node's unit: the forecaster may call the tool only under a grant; the treasurer may issue and revoke one.
const UNIT: &str = r#"principal = "weather"

[[rule]]
actor          = "token:fleet/forecaster"
operation      = "tools/call"
resource       = "tool:weather.forecast"
requires_grant = true

[[rule]]
actor     = "token:fleet/treasurer"
operation = "grant.issue"
resource  = "tool:weather.forecast"

[[rule]]
actor     = "token:fleet/treasurer"
operation = "grant.revoke"
resource  = "tool:weather.forecast"
"#;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("i1c-{tag}-{}", alloc_port()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("units")).unwrap();
    std::fs::write(dir.join("units").join("weather.toml"), UNIT).unwrap();
    dir
}

fn config(dir: &Path, http_port: u16) -> (NodeId, GossipConfig) {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("tls"), ..Default::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    cfg.gateway_identity_issuer = Some(ISSUER.into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "forecaster".into(), token: "fc-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "nobody".into(), token: "nobody-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "treasurer".into(), token: "treasurer-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] },
    ];
    cfg.authority = Some(AuthorityConfig {
        declarations: dir.join("units"),
        unit: "weather".into(),
        journal: dir.join("evidence.log"),
        tier: AuthorityTier::Serialised,
        freshness_ms: 120_000,
        skew_ms: 1_000,
        entitle_grants: GrantEntitlement::Node,
        scope: None,
    });
    (NodeId::new("127.0.0.1", port).unwrap(), cfg)
}

/// Issue the forecaster's client certificate from the node's CA, publish it, return its key.
fn forecaster_key(a: &GossipAgent, dir: &Path) -> ed25519_dalek::SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let (cert, key) = crate::issue_client_cert(&dir.join("tls"), ISSUER, "forecaster", &dir.join("client")).unwrap();
    let der = rustls::pki_types::CertificateDer::from_pem_file(&cert).unwrap();
    a.publish_client_cert(der.as_ref()).expect("published");
    let der = rustls::pki_types::PrivatePkcs8KeyDer::from_pem_file(&key).unwrap();
    ed25519_dalek::SigningKey::from_pkcs8_der(der.secret_pkcs8_der()).unwrap()
}

async fn http(http_port: u16, bearer: &str, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    let mut req = reqwest::Client::new().request(method, format!("http://127.0.0.1:{http_port}{path}")).bearer_auth(bearer);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.json::<serde_json::Value>().await.unwrap_or_default())
}

async fn mcp_call(http_port: u16, args: &serde_json::Value, meta: Option<serde_json::Value>) -> serde_json::Value {
    let mut params = serde_json::json!({"name": TOOL, "arguments": args});
    if let Some(m) = meta {
        params["_meta"] = m;
    }
    http(http_port, "fc-secret", reqwest::Method::POST, "/mcp", Some(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}))).await.1
}

async fn started(dir: &Path, http_port: u16) -> (Arc<GossipAgent>, Arc<AtomicUsize>) {
    let (id, cfg) = config(dir, http_port);
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
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
    tokio::time::sleep(Duration::from_millis(150)).await;
    (a, ran)
}

/// **A grant held by a client principal, end to end** (identity-and-lifecycle I1, second half). The forecaster — a
/// service with a client certificate — asks for a grant at `POST /gateway/grants/request`; the request lands under
/// `grant/{hex(principal)}/{request}`, where the supervisor's pending list and decision door find it; the treasurer
/// issues it; the forecaster reads it at `GET /gateway/grants/request/{request}`, proves possession with its client
/// key and the call runs once; the treasurer revokes it against the tool's provider. Without the grant the call is
/// refused. Seen failing first, one build per plant: the client keys not fed to the authority (the call presenting the
/// grant was refused — possession unverifiable); the holder written raw into the key (the request never reached the
/// pending list); the revocation deriving the resource from a node holder only (`unreadable_grant`).
#[tokio::test]
async fn a_grant_held_by_a_client_principal_is_requested_issued_presented_and_revoked() {
    let dir = scratch("e2e");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"city": "Lyon", "days": 3});

    let without = mcp_call(http_port, &args, None).await;
    assert!(without.get("error").is_some(), "no grant, no call: {without}");

    let (status, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args}))).await;
    assert_eq!(status, 200, "{asked}");
    let request = asked["request"].as_str().unwrap().to_string();
    let segment = crate::agent::gateway_authority::grant_holder_segment(FORECASTER);
    assert_eq!(asked["key"], format!("grant/{segment}/{request}"), "the key names the client in hex: {asked}");

    let (_, pending) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    assert!(pending["pending"].as_array().unwrap().iter().any(|r| r["request"] == request.as_str()), "the supervisor sees it: {pending}");
    let (status, issued) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{segment}/{request}"), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200, "the treasurer issues: {issued}");

    let (_, read) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{request}"), None).await;
    assert_eq!(read["state"], "issued", "{read}");
    let grant: crate::mandate::grant::SignedMandateGrant = serde_json::from_value(read["grant"].clone()).unwrap();
    assert_eq!(grant.mandate.holder.as_str(), FORECASTER);
    let resource = asked["resource"].as_str().unwrap();
    let request_bytes = crate::possession_request("tools/call", resource, &crate::arguments_digest(&args));
    let proof = mycelium_core::tls::sign_bytes(&key, &crate::mandate::grant::possession_message(&grant.mandate, &request_bytes));
    let mandate = serde_json::json!({"grant": grant, "possession": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof)});
    let called = mcp_call(http_port, &args, Some(serde_json::json!({"mandate": mandate}))).await;

    let (status, revoked) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{segment}/{request}/revoke"), Some(serde_json::json!({"reason": "season over"}))).await;
    let (_, after) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{request}"), None).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(called.get("result").is_some(), "UNVERIFIED: the client's possession of its grant did not establish it: {called}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the call ran once, under the grant");
    assert_eq!(status, 200, "REVOKE: the grant held by a client was not revoked: {revoked}");
    assert_eq!(after["state"], "revoked", "{after}");
}

/// **Only a client with a certificate on record asks for, or is issued, a grant.** `nobody` holds `mcp:invoke` but no
/// client certificate: its request is refused `not_a_client_holder`, and a decision naming its hex segment is refused
/// `not_a_member` before any record is read. Seen failing first: the request was written, and the decision answered
/// `unknown_request`.
#[tokio::test]
async fn a_principal_without_a_client_certificate_neither_asks_for_nor_is_issued_a_grant() {
    let dir = scratch("nobody");
    let http_port = alloc_port();
    let (a, _ran) = started(&dir, http_port).await;
    let (status, asked) = http(http_port, "nobody-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {}}))).await;
    let segment = crate::agent::gateway_authority::grant_holder_segment("token:fleet/nobody");
    let (dstatus, decided) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{segment}/0000000000000001"), Some(serde_json::json!({"decision": "issue"}))).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((status, asked["error"].as_str()), (409, Some("not_a_client_holder")), "ASKED: {asked}");
    assert_eq!((dstatus, decided["error"].as_str()), (409, Some("not_a_member")), "ISSUABLE: {decided}");
}

/// The holder segment round-trips, and is unambiguous: a node principal is written as it is, a client principal in
/// hex, a node principal in hex is not canonical, and a segment that is neither is refused.
#[test]
fn a_grant_holder_segment_round_trips_and_is_unambiguous() {
    use crate::agent::gateway_authority::{grant_holder_from_segment, grant_holder_segment};
    for p in ["node:127.0.0.1:9000", FORECASTER, "oidc:idp/alice"] {
        assert_eq!(grant_holder_from_segment(&grant_holder_segment(p)).as_deref(), Some(p));
    }
    assert!(!grant_holder_segment(FORECASTER).contains('/'));
    assert_eq!(grant_holder_from_segment(&grant_holder_segment("node:127.0.0.1:9000").bytes().map(|b| format!("{b:02x}")).collect::<String>()), None, "a hex node principal is not canonical");
    for bad in ["", "zz", "abc", "ABCD", "node:a/b"] {
        assert_eq!(grant_holder_from_segment(bad), None, "{bad:?}");
    }
}
