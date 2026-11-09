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

/// How a test node differs: the certificate directory two nodes share, the peers to bootstrap to, and whether it
/// carries the declared authority (a bare provider does not).
#[derive(Clone)]
struct Node {
    certs: PathBuf,
    bootstrap: Vec<NodeId>,
    authority: bool,
    /// A gossip port fixed in advance (a peer's declaration names it); fresh otherwise.
    port: Option<u16>,
}

fn config(dir: &Path, http_port: u16, n: &Node) -> (NodeId, GossipConfig) {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let port = n.port.unwrap_or_else(alloc_port);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.http_port = Some(http_port);
    cfg.bootstrap_peers = n.bootstrap.clone();
    cfg.reconnect_backoff_secs = 1;
    cfg.tls = Some(TlsConfig { auto_cert_dir: n.certs.clone(), ..Default::default() });
    cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    cfg.gateway_identity_issuer = Some(ISSUER.into());
    cfg.gateway_named_tokens = vec![
        GatewayNamedToken { name: "forecaster".into(), token: "fc-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "nobody".into(), token: "nobody-secret".into(), scopes: vec!["mcp:invoke".into()] },
        GatewayNamedToken { name: "treasurer".into(), token: "treasurer-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] },
    ];
    cfg.authority = n.authority.then(|| AuthorityConfig {
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

/// Issue the forecaster's client certificate from the node's CA (into `out`), publish it, return its key.
fn forecaster_key(a: &GossipAgent, dir: &Path) -> ed25519_dalek::SigningKey {
    forecaster_key_in(a, &dir.join("tls"), &dir.join("client"))
}

fn forecaster_key_in(a: &GossipAgent, certs: &Path, out: &Path) -> ed25519_dalek::SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let (cert, key) = crate::issue_client_cert(certs, ISSUER, "forecaster", out).unwrap();
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

/// `value` as one percent-encoded path segment — the SDKs' `path_segment` / `pathSegment`: everything but the
/// unreserved characters encoded, so a principal's `/` and `:` stay inside the segment.
fn segment(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// The `_meta.mandate` a client presents: the grant and its possession proof over this call, signed with `key`.
fn presented(grant: &crate::mandate::grant::SignedMandateGrant, resource: &str, args: &serde_json::Value, key: &ed25519_dalek::SigningKey) -> serde_json::Value {
    let request_bytes = crate::possession_request("tools/call", resource, &crate::arguments_digest(args));
    let proof = mycelium_core::tls::sign_bytes(key, &crate::mandate::grant::possession_message(&grant.mandate, &request_bytes));
    serde_json::json!({"mandate": {"grant": grant, "possession": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof)}})
}

/// Ask for a grant as the forecaster and have the treasurer issue it, through the SDKs' URL shape (the record's
/// `agent` — the principal — percent-encoded). Returns the request's answer and the grant.
async fn ask_and_issue(http_port: u16, args: &serde_json::Value) -> (serde_json::Value, crate::mandate::grant::SignedMandateGrant) {
    let (status, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args}))).await;
    assert_eq!(status, 200, "{asked}");
    let request = asked["request"].as_str().unwrap();
    let (status, issued) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(request)), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200, "SDK: the treasurer issues through the SDKs' URL shape: {issued}");
    let (_, read) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(request)), None).await;
    assert_eq!(read["state"], "issued", "{read}");
    (asked, serde_json::from_value(read["grant"].clone()).unwrap())
}

async fn revoke(http_port: u16, request: &str) -> (u16, serde_json::Value) {
    http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}/revoke", segment(FORECASTER), segment(request)), Some(serde_json::json!({"reason": "season over"}))).await
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

async fn started(dir: &Path, http_port: u16) -> (Arc<GossipAgent>, Arc<AtomicUsize>) {
    started_as(dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: None }, true).await
}

/// A node, its declaration's authority attached or not, serving the tool or not.
async fn started_as(dir: &Path, http_port: u16, n: &Node, serves: bool) -> (Arc<GossipAgent>, Arc<AtomicUsize>) {
    let (id, cfg) = config(dir, http_port, n);
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("the node starts from its declaration");
    let ran = Arc::new(AtomicUsize::new(0));
    if serves {
        let r = Arc::clone(&ran);
        let handle = a.mcp().register_mcp_tool(TOOL, serde_json::json!({"type": "object"}), move |_| {
            let r = Arc::clone(&r);
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!("sunny"))
            }
        });
        std::mem::forget(handle);
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    (a, ran)
}

/// **A grant held by a client principal, end to end** (identity-and-lifecycle I1, second half). The forecaster — a
/// service with a client certificate — asks for a grant at `POST /gateway/grants/request`; the request lands under
/// `grant/{hex(principal)}/{request}`, where the supervisor's pending list finds it; the treasurer issues it through
/// the SDKs' URL shape (the record's `agent`, percent-encoded); the forecaster reads it at `GET
/// /gateway/grants/request/{request}`, proves possession with its client key and the call runs once. A second grant,
/// issued and revoked before it is used, is refused when presented — the revocation, not the use marker, refuses it.
/// Without a grant the call is refused. Seen failing first, one build per plant: the client keys not fed to the
/// authority (the call presenting the grant was refused — possession unverifiable); the holder written raw into the
/// key (the request never reached the pending list); the revocation deriving the resource from a node holder only
/// (`unreadable_grant`); the doors reading a hex segment only (*SDK*: `400 bad_request`, #743's review, item 2).
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
    assert_eq!(asked["key"], format!("grant/{}/{request}", crate::agent::gateway_authority::grant_holder_segment(FORECASTER)), "the key names the client in hex: {asked}");
    assert!(request.ends_with(&format!("-{}", a.node_id())), "the request id names the gateway that wrote it: {request}");
    let (_, pending) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    let row = pending["pending"].as_array().unwrap().iter().find(|r| r["request"] == request.as_str()).cloned();
    assert!(row.is_some(), "the supervisor sees it: {pending}");
    let row = row.unwrap();
    assert_eq!(row["agent"], FORECASTER, "the row names the principal, which the SDKs put in the path");
    // The SDKs' decide: `grant_decide(row["agent"], row["request"])`, each a percent-encoded segment.
    let (status, issued) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(row["agent"].as_str().unwrap()), segment(&request)), Some(serde_json::json!({"decision": "issue"}))).await;
    assert_eq!(status, 200, "SDK: the treasurer issues through the SDKs' URL shape: {issued}");

    let (_, read) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(&request)), None).await;
    assert_eq!(read["state"], "issued", "{read}");
    let grant: crate::mandate::grant::SignedMandateGrant = serde_json::from_value(read["grant"].clone()).unwrap();
    assert_eq!(grant.mandate.holder.as_str(), FORECASTER);
    let resource = asked["resource"].as_str().unwrap();
    assert_eq!(grant.mandate.grant_resource(), Some(resource), "the grant carries the resource its issue evaluated, signed");
    let called = mcp_call(http_port, &args, Some(presented(&grant, resource, &args, &key))).await;

    // A second grant, revoked before it is used, through the SDKs' URL shape.
    let other = serde_json::json!({"city": "Nantes", "days": 1});
    let (asked2, grant2) = ask_and_issue(http_port, &other).await;
    let (status, revoked) = revoke(http_port, asked2["request"].as_str().unwrap()).await;
    let after_revoke = mcp_call(http_port, &other, Some(presented(&grant2, asked2["resource"].as_str().unwrap(), &other, &key))).await;
    let (_, after) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(asked2["request"].as_str().unwrap())), None).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(called.get("result").is_some(), "UNVERIFIED: the client's possession of its grant did not establish it: {called}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the call ran once, under the grant; the revoked one never");
    assert_eq!(status, 200, "REVOKE: the grant held by a client was not revoked: {revoked}");
    assert_eq!(after["state"], "revoked", "{after}");
    assert!(after_revoke.get("error").is_some() && after_revoke.to_string().contains("revoked"), "the revoked grant, never used, is refused as revoked: {after_revoke}");
}

/// **A revocation evaluates the resource the issue evaluated, and needs no provider** (#743's review, item 1). The
/// gateway A carries the authority; the tool is served by another node, B, so the request's resource is
/// `tool:{tool}@{B}` — not A. The treasurer issues at A; B leaves, and its `tools/` record with it; the revocation at
/// A still succeeds, against `tool:{tool}@{B}`, read from the signed grant. Seen failing first with the resource
/// re-resolved from `tools/` at revoke (`409 no_provider`: the grant stood until its window closed).
#[tokio::test]
async fn a_client_grant_is_revoked_against_its_issued_resource_with_the_provider_gone() {
    let dir_a = scratch("two-a");
    let dir_b = scratch("two-b");
    let certs = dir_a.join("tls");
    let (http_a, http_b, port_b) = (alloc_port(), alloc_port(), alloc_port());
    // A's declaration names the tool at B: a bare `tool:` resource in a unit means this node.
    let at_b = format!("tool:{TOOL}@127.0.0.1:{port_b}");
    std::fs::write(dir_a.join("units").join("weather.toml"), UNIT.replace("resource       = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\"")).replace("resource  = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\""))).unwrap();
    let (a, _ran_a) = started_as(&dir_a, http_a, &Node { certs: certs.clone(), bootstrap: vec![], authority: true, port: None }, false).await;
    let (b, ran_b) = started_as(&dir_b, http_b, &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: false, port: Some(port_b) }, true).await;
    let provided = |n: &GossipAgent| crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &format!("tools/{TOOL}/")).iter().any(|(k, _)| k.ends_with(&n.node_id().to_string()));
    assert!(until(|| provided(&b), Duration::from_secs(15)).await, "A learned that B serves the tool");
    let _key = forecaster_key_in(&a, &certs, &dir_a.join("client"));
    let args = serde_json::json!({"city": "Brest"});
    let (asked, grant) = ask_and_issue(http_a, &args).await;
    assert_eq!(at_b, format!("tool:{TOOL}@{}", b.node_id()));
    let request = asked["request"].as_str().unwrap().to_string();
    b.shutdown().await;
    let gone = until(|| !provided(&b), Duration::from_secs(15)).await;
    let (status, revoked) = revoke(http_a, &request).await;
    let (_, after) = http(http_a, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(&request)), None).await;
    a.shutdown().await;
    for d in [&dir_a, &dir_b] {
        let _ = std::fs::remove_dir_all(d);
    }
    assert_eq!(asked["resource"], at_b.as_str(), "the resource names the provider, not the gateway: {asked}");
    assert_eq!(grant.mandate.grant_resource(), Some(at_b.as_str()));
    assert!(gone, "B's tools/ record left with it");
    assert_eq!(status, 200, "PROVIDER: the revocation needed a live provider: {revoked}");
    assert_eq!(after["state"], "revoked", "{after}");
    assert_eq!(ran_b.load(Ordering::SeqCst), 0);
}

/// **A superseded client key no longer holds a grant** (#743's review, item 3): the forecaster's certificate is issued
/// twice; the grant is issued after the second; a possession proof by the first key is refused at the authority, one
/// by the second runs. (The interleaved-feed race itself is
/// `a_client_key_proves_possession_only_and_a_superseded_one_never_returns`, at the authority.)
#[tokio::test]
async fn a_possession_proof_by_a_superseded_client_key_is_refused() {
    let dir = scratch("supersede");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let old = forecaster_key_in(&a, &dir.join("tls"), &dir.join("client-1"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    let new = forecaster_key_in(&a, &dir.join("tls"), &dir.join("client-2"));
    let args = serde_json::json!({"city": "Lille"});
    let (asked, grant) = ask_and_issue(http_port, &args).await;
    let resource = asked["resource"].as_str().unwrap();
    let by_old = mcp_call(http_port, &args, Some(presented(&grant, resource, &args, &old))).await;
    let by_new = mcp_call(http_port, &args, Some(presented(&grant, resource, &args, &new))).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(by_old.get("error").is_some(), "SUPERSEDED: a proof by the superseded key held the grant: {by_old}");
    assert!(by_new.get("result").is_some(), "the current key holds it: {by_new}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

/// **A grant issued before a rotation is still revoked by its issuer** (#743's review, round 2): the treasurer issues
/// a grant, the node rotates its identity, and the revocation at the same node succeeds — the issued record verifies
/// under a key this node held, not only its current one — and the grant presented afterwards is refused as revoked.
/// A record under the key that no key of this node signed is refused (`409 unreadable_grant`). Seen failing first with
/// the issued record checked against the current key only (`409 unreadable_grant` for the rotated grant), and with the
/// check removed (the forged record was revoked).
#[tokio::test]
async fn a_grant_issued_before_a_rotation_is_revoked_by_its_issuer() {
    let dir = scratch("rotate");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"city": "Caen"});
    let (asked, grant) = ask_and_issue(http_port, &args).await;
    // A second grant, its issued record overwritten by one no key of this node signed (any member can write the key).
    let (asked2, mut forged) = ask_and_issue(http_port, &serde_json::json!({"city": "Rouen"})).await;
    forged.signature = mycelium_core::tls::sign_bytes(&ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]), &forged.mandate.canonical_bytes()).to_vec();
    let forged_key = format!("{}/issued", asked2["key"].as_str().unwrap());
    assert!(a.kv().set(forged_key, bytes::Bytes::from(serde_json::to_vec(&forged).unwrap())));
    let rotated = a.rotate_identity(Duration::from_millis(50)).await;
    let (status, revoked) = revoke(http_port, asked["request"].as_str().unwrap()).await;
    let (fstatus, frevoked) = revoke(http_port, asked2["request"].as_str().unwrap()).await;
    let after = mcp_call(http_port, &args, Some(presented(&grant, asked["resource"].as_str().unwrap(), &args, &key))).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(rotated.is_ok(), "the node rotated: {rotated:?}");
    assert_eq!(status, 200, "ROTATED: the issuer could not revoke a grant it signed before rotating: {revoked}");
    assert_eq!((fstatus, frevoked["error"].as_str()), (409, Some("unreadable_grant")), "a record no key of this node signed is not revoked: {frevoked}");
    assert!(after.get("error").is_some() && after.to_string().contains("revoked"), "the revoked grant is refused as revoked: {after}");
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

/// **The request door reuses, bounds and records** (#743's review, items 5 and 7). Asking twice for the same tool and
/// arguments answers the standing request (`reused`), not a second; past
/// [`MAX_PENDING_GRANT_REQUESTS_PER_CLIENT`](crate::agent::http::grants::MAX_PENDING_GRANT_REQUESTS_PER_CLIENT) standing
/// requests the door refuses `429 too_many_pending`; a classified argument set is withheld from the record with its
/// class and size, as the agent's door writes it. Seen failing first, one build per plant: no reuse (*REUSE*), no
/// bound (*BOUND*), the record without the data-class rendering (*WITHHELD*).
#[tokio::test]
async fn the_request_door_reuses_a_standing_request_bounds_them_and_withholds_a_classified_rendering() {
    let dir = scratch("door");
    let http_port = alloc_port();
    let (a, _ran) = started(&dir, http_port).await;
    let _key = forecaster_key(&a, &dir);
    let ask = |args: serde_json::Value, class: Option<&str>| {
        let mut body = serde_json::json!({"tool": TOOL, "arguments": args});
        if let Some(c) = class {
            body["arguments_class"] = serde_json::json!(c);
        }
        http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(body))
    };
    let (_, first) = ask(serde_json::json!({"city": "Metz"}), Some("personal")).await;
    let (_, again) = ask(serde_json::json!({"city": "Metz"}), Some("personal")).await;
    let mut last = (0, serde_json::Value::Null);
    for i in 1..=crate::agent::http::grants::MAX_PENDING_GRANT_REQUESTS_PER_CLIENT {
        last = ask(serde_json::json!({"city": "Metz", "n": i}), None).await;
    }
    let (_, pending) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/gateway/grants/pending", None).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((again["reused"].as_bool(), &again["request"]), (Some(true), &first["request"]), "REUSE: {again}");
    assert_eq!((last.0, last.1["error"].as_str()), (429, Some("too_many_pending")), "BOUND: {}", last.1);
    let row = pending["pending"].as_array().unwrap().iter().find(|r| r["request"] == first["request"]).cloned().unwrap_or_default();
    assert_eq!(row["arguments"]["class"], "personal", "WITHHELD: {row}");
    assert!(row["arguments"]["withheld"].is_string() && row["arguments"].get("city").is_none(), "WITHHELD: {row}");
    assert_eq!(row["arguments_class"], "personal");
}

/// **Only the authority attached is fed client keys** (#743's review, item 9): a second execution authority, refused
/// as a second, registers no sink.
#[tokio::test]
async fn a_second_execution_authority_registers_no_client_key_sink() {
    use crate::agent::gateway_authority::tests::{authority, keys};
    let a = GossipAgent::new(NodeId::new("127.0.0.1", alloc_port()).unwrap(), GossipConfig::default());
    let (ak, hk) = keys();
    a.with_execution_authority(Arc::new(authority(&ak, &hk)));
    a.with_execution_authority(Arc::new(authority(&ak, &hk)));
    assert_eq!(a.task_ctx.client_key_sinks.pin().len(), 1, "SINK: the refused authority's set was registered");
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

/// **The possession vector** (identity-and-lifecycle I1, the SDKs' half): a grant held by a client, as the gateway
/// serves it, and the client's proof for one `tools/call`, pinned in `tests/fixtures/possession/vector.json`, which
/// jest and pytest rebuild from the same inputs. Here the gateway's own functions rebuild each part — the mandate's
/// canonical bytes and digest, the request bytes over the gateway's arguments digest, the possession message — the
/// issuer's and the holder's signatures verify, and the gateway's `GrantVerifier`, fed the holder's key as a client
/// key, finds the presented grant `Valid`. The vector was written by the Python SDK's implementation and is checked
/// here by the gateway's own code; the SDKs' tests are the ones seen failing first.
#[test]
fn the_possession_vector_is_the_gateways() {
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::knowledge::IssuerId;
    use crate::mandate::grant::{possession_message, EntitlementTable, GrantVerdict, GrantVerifier, SignedMandateGrant};
    use crate::mandate::PrincipalId;
    use base64::Engine;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/possession/vector.json")).unwrap()).unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let seed = |k: &str| -> [u8; 32] { (0..32).map(|i| u8::from_str_radix(&v[k].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap() };
    let grant: SignedMandateGrant = serde_json::from_value(v["grant"].clone()).expect("the grant parses as the gateway's");
    let m = &grant.mandate;
    assert_eq!(hex(&m.canonical_bytes()), v["mandate_canonical_hex"], "the canonical bytes");
    assert_eq!(hex(&m.digest()), v["mandate_digest_hex"]);
    let digest = crate::arguments_digest(&v["arguments"]);
    assert_eq!(hex(&digest), v["arguments_digest_hex"], "the gateway's arguments digest");
    let request = crate::possession_request(v["operation"].as_str().unwrap(), v["resource"].as_str().unwrap(), &digest);
    assert_eq!(hex(&request), v["request_hex"]);
    let message = possession_message(m, &request);
    assert_eq!(hex(&message), v["possession_message_hex"]);
    let issuer = ed25519_dalek::SigningKey::from_bytes(&seed("issuer_seed_hex"));
    let holder = ed25519_dalek::SigningKey::from_bytes(&seed("holder_seed_hex"));
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(b64.encode(issuer.verifying_key().to_bytes()), v["issuer_key_b64"]);
    assert_eq!(b64.encode(holder.verifying_key().to_bytes()), v["holder_key_b64"]);
    assert!(mycelium_core::tls::verify_bytes(&issuer.verifying_key().to_bytes(), &m.canonical_bytes(), &grant.signature), "the issuer's signature");
    assert_eq!(b64.encode(mycelium_core::tls::sign_bytes(&holder, &message)), v["possession_b64"], "the holder's proof, deterministic");

    let mut table = EntitlementTable::new();
    table.entitle_grants(PrincipalId::new(m.established_by.as_str()).unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.set_client_key(IssuerId::new(m.holder.as_str()).unwrap(), 1, holder.verifying_key().to_bytes()).unwrap();
    let proof = b64.decode(v["possession_b64"].as_str().unwrap()).unwrap();
    // The issuer is a node: its key is a member's.
    let issuer_node: NodeId = m.established_by.as_str().strip_prefix("node:").unwrap().parse().unwrap();
    let members = std::collections::HashMap::from([(issuer_node, crate::knowledge::issuer::MemberKeys { retained: vec![issuer.verifying_key().to_bytes()], ..Default::default() })]);
    let verdict = GrantVerifier::new(table).check(&grant, &request, Some(&proof), m.valid_from_ms + 1, &members, &external);
    assert_eq!(verdict, GrantVerdict::Valid, "the gateway's verifier accepts the SDKs' proof");
    // The digest cases (#744's review, item 1): the gateway writes each case as `serialized` — members by UTF-8 bytes —
    // and digests that.
    for case in v["mandate_digest_cases"].as_array().unwrap() {
        assert_eq!(serde_json::to_string(&case["arguments"]).unwrap(), case["serialized"].as_str().unwrap(), "the gateway's form");
        assert_eq!(hex(&crate::arguments_digest(&case["arguments"])), case["digest_hex"], "the gateway's digest of {}", case["serialized"]);
    }
}

/// **The SDKs' shape: a client's own signature and its own grant on one `tools/call`** (#744's review, item 6). The
/// forecaster signs the call with its client key (`cs`/`cn`, as `ClientSigner::sign_tools_call` and both SDKs write
/// them) and presents its grant with a possession proof by the same key, in one `_meta`. **A refused signature leaves
/// the grant unspent** (I1's PR 4): first the call is sent with the signature made over other arguments — refused
/// `client_signature_invalid` by the gateway's own check, before its preflight claims the grant's use — then the honest
/// call under the **same** grant runs once. Seen failing first with the gateway's check removed (the honest call was
/// refused: the grant was already used at the gateway).
#[tokio::test]
async fn a_signed_call_presenting_a_client_grant_runs_at_mcp() {
    let dir = scratch("signed");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"city": "Lyon", "days": 3});
    let (asked, grant) = ask_and_issue(http_port, &args).await;
    let signer = crate::ClientSigner::new(key.clone(), FORECASTER);
    let sign_into = |meta: &mut serde_json::Value, over: &serde_json::Value| {
        let s = crate::ClientSigner::meta(&signer.sign_tools_call(TOOL, over).unwrap());
        meta["cs"] = s["cs"].clone();
        meta["cn"] = s["cn"].clone();
    };
    let mut meta = presented(&grant, asked["resource"].as_str().unwrap(), &args, &key);
    sign_into(&mut meta, &serde_json::json!({"city": "Nice"}));
    let tampered = mcp_call(http_port, &args, Some(meta.clone())).await;
    let ran_before = ran.load(Ordering::SeqCst);
    sign_into(&mut meta, &args);
    let called = mcp_call(http_port, &args, Some(meta)).await;
    let ran_after = ran.load(Ordering::SeqCst);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(tampered["error"]["data"]["reason"], "client_signature_invalid", "refused by the gateway's own check, by name: {tampered}");
    assert_eq!(ran_before, 0, "the tampered call did not run");
    assert!(called.get("result").is_some(), "UNSPENT: the honest call under the same grant was refused: {called}");
    assert_eq!(ran_after, 1, "once");
}
