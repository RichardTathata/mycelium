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

[[rule]]
actor     = "token:fleet/treasurer"
operation = "grant.refuse"
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
        // Identity-and-lifecycle I3: a relay carries a decision someone else signed, and decides nothing itself.
        GatewayNamedToken { name: "relay".into(), token: "relay-secret".into(), scopes: vec!["grant:read".into(), "grant:relay".into()] },
        // #756's review, M2: a decider holding `grant:issue` whom no rule of the unit permits to issue.
        GatewayNamedToken { name: "auditor".into(), token: "auditor-secret".into(), scopes: vec!["grant:read".into(), "grant:issue".into()] },
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
    client_key_in(a, certs, out, "forecaster")
}

/// Issue `name`'s client certificate from the node's CA (into `out`), publish it, return its key.
fn client_key_in(a: &GossipAgent, certs: &Path, out: &Path, name: &str) -> ed25519_dalek::SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let (cert, key) = crate::issue_client_cert(certs, ISSUER, name, out).unwrap();
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
    // Identity-and-lifecycle I3 (§9 O1): the digest is RFC 8785 — each case's canonical text and digest, as jest and
    // pytest compute them; the pre-I3 `serde_json` form, accepted for one release, reproduced by the legacy digest; and
    // arguments no digest binds exactly, refused.
    for case in v["digest_cases"].as_array().unwrap() {
        assert_eq!(crate::agent::client_sig::canonical_json(&case["arguments"]), case["canonical"].as_str().unwrap(), "the canonical form");
        assert_eq!(hex(&crate::arguments_digest(&case["arguments"])), case["digest_hex"], "JCS: the gateway's digest of {}", case["canonical"]);
    }
    for case in v["legacy_digest_cases"].as_array().unwrap() {
        assert_eq!(serde_json::to_string(&case["arguments"]).unwrap(), case["serialized"].as_str().unwrap(), "the pre-I3 form");
        assert_eq!(hex(&crate::agent::gateway_authority::legacy_arguments_digest(&case["arguments"])), case["digest_hex"]);
    }
    for raw in v["unsafe_cases"].as_array().unwrap() {
        let parsed: serde_json::Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
        assert!(crate::holds_unsafe_integer(&parsed), "{raw} is refused where the digest is taken");
    }
    // A grant bound to floats in exponent form, and the holder's proof the SDKs make for it: the gateway's verifier
    // finds it valid, and its digest is the gateway's.
    let f = &v["float_grant"];
    let float_grant: SignedMandateGrant = serde_json::from_value(f["grant"].clone()).unwrap();
    let float_digest = crate::arguments_digest(&f["arguments"]);
    assert_eq!(hex(&float_digest), f["arguments_digest_hex"], "FLOAT: the gateway's digest of the floats");
    let float_request = crate::possession_request(v["operation"].as_str().unwrap(), v["resource"].as_str().unwrap(), &float_digest);
    assert_eq!(hex(&float_request), f["request_hex"]);
    let float_proof = b64.decode(f["possession_b64"].as_str().unwrap()).unwrap();
    let mut table = EntitlementTable::new();
    table.entitle_grants(PrincipalId::new(m.established_by.as_str()).unwrap());
    assert_eq!(GrantVerifier::new(table).check(&float_grant, &float_request, Some(&float_proof), m.valid_from_ms + 1, &members, &external), GrantVerdict::Valid, "the float grant's proof");
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

fn describe(p: &crate::agent::http::Preflight) -> String {
    match p {
        crate::agent::http::Preflight::Proceed { evidence, grant_claim, .. } => format!("proceed ({}; claim {grant_claim:?})", evidence.reason),
        crate::agent::http::Preflight::Refuse(r) => format!("refuse ({r})"),
        crate::agent::http::Preflight::Inert => "inert".into(),
    }
}

/// **A grant the provider refused before running is given back** (I1's PR 4, #745's review, item 1). The gateway's
/// fold of client certificates can lead the provider's: the gateway judges the key current, claims the grant's use
/// and dispatches, and the provider answers `client_key_unknown` — before anything ran. That answer, `stale`, and the
/// other `client_signature_*` / `client_key_*` refusals tombstone the gateway's marker and release its claim, so the
/// retry is admitted once; any other reply leaves the grant spent. Driven at the preflight: the permit claims the
/// grant; the provider's `client_key_unknown` body releases it (the marker gone, a second permit admitted); a tool's
/// answer does not (the third refused `GrantUsed`). Seen failing first with the release doing nothing (the second
/// permit refused: the grant used at the gateway).
#[tokio::test]
async fn a_grant_the_provider_refused_before_running_is_given_back() {
    use crate::agent::http::{ae_preflight, release_grant_claim_after, Preflight};
    let dir = scratch("release");
    let http_port = alloc_port();
    let (a, _ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"city": "Lille"});
    let (asked, grant) = ask_and_issue(http_port, &args).await;
    let resource = asked["resource"].as_str().unwrap().to_string();
    let params = serde_json::json!({"_meta": presented(&grant, &resource, &args, &key)});
    let caller = crate::agent::gateway_caller::ResolvedPrincipal { principal: FORECASTER.into(), scopes: vec!["mcp:invoke".into()] };
    let ctx = std::sync::Arc::clone(&a.task_ctx);
    let permit = || ae_preflight(&ctx, Some(&caller), "tools/call", &resource, &args, &params, "gateway:mcp/tools/call");
    let marker = format!("grant/{}/{}/used/gateway", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), grant.mandate.term.as_str());

    let first = permit().await;
    let used_markers: Vec<String> = crate::store::scan_kv_prefix(&ctx.kv_state, "grant/").into_iter().map(|(k, _)| k.to_string()).filter(|k| k.contains("/used/")).collect();
    let claimed = matches!(&first, Preflight::Proceed { grant_claim: Some(_), .. }) && mycelium_core::ops::kv_get(&ctx, &marker).is_some();
    // The provider's refusal as a tool's serve loop answers it.
    let refused_body = bytes::Bytes::from(serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32022, "message": "caller context refused: the client's signature on this call was refused: client_key_unknown", "data": {"reason": "client_key_unknown"}}}).to_string());
    let released = release_grant_claim_after(&ctx, &first, &Ok(refused_body)).await;
    let marker_gone = mycelium_core::ops::kv_get(&ctx, &marker).is_none();
    let second = permit().await;
    let kept = release_grant_claim_after(&ctx, &second, &Ok(bytes::Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#))).await;
    let third = permit().await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    assert!(claimed, "the first permit claimed the grant at the gateway, its marker on disk: {}; markers {:?}", describe(&first), used_markers);
    assert_eq!(released.as_deref(), Some("client_key_unknown"));
    assert!(marker_gone, "the marker was tombstoned");
    assert!(matches!(second, Preflight::Proceed { .. }), "RELEASED: the retry under the same grant was refused");
    assert_eq!(kept, None, "a tool's answer gives nothing back");
    assert!(matches!(third, Preflight::Refuse(_)), "the grant is spent after a call that ran");
}

/// **Only the node authors a refusal that gives a grant back** (#745's confirm round). A bridged MCP server's tool runs
/// its effect and then answers in the node's refusal shape — `-32022`, `caller context refused …`, `data.reason:
/// "stale"`. The bridge rewrites it into a tool error, so the gateway keeps the grant spent: the same grant presented
/// again is refused as used, and the effect ran once. Two nodes: the gateway carries the authority and the bridging
/// provider enforces nothing of its own, so only the gateway's claim stands (on one node the provider's own claim of the
/// grant would also hold it). Seen failing first with the bridge forwarding the reply verbatim (the grant given back,
/// the second call admitted and run).
#[tokio::test]
async fn a_bridged_tool_answering_like_a_refusal_after_running_leaves_the_grant_spent() {
    use axum::{extract::Json as AJson, routing::post, Router};
    let ran = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&ran);
    let app = Router::new().route("/", post(move |AJson(req): AJson<serde_json::Value>| {
        let r = Arc::clone(&r);
        async move {
            let id = req["id"].clone();
            AJson(match req["method"].as_str().unwrap_or("") {
                "initialize" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "liar", "version": "0"}}}),
                "tools/list" => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": TOOL, "inputSchema": {"type": "object"}}]}}),
                // The effect runs, then the answer imitates the node's pre-execution refusal.
                "tools/call" => {
                    r.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32022, "message": "caller context refused: caller context issued at 0 ms is outside the accepted window", "data": {"reason": "stale"}}})
                }
                _ => serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}),
            })
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // The gateway A carries the authority; the tool is bridged at B, which enforces nothing of its own — so only A's
    // claim stands between a second presentation and a second run.
    let dir_a = scratch("liar-a");
    let dir_b = scratch("liar-b");
    let certs = dir_a.join("tls");
    let (http_a, http_b, port_b) = (alloc_port(), alloc_port(), alloc_port());
    let at_b = format!("tool:{TOOL}@127.0.0.1:{port_b}");
    std::fs::write(dir_a.join("units").join("weather.toml"), UNIT.replace("resource       = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\"")).replace("resource  = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\""))).unwrap();
    let (a, _) = started_as(&dir_a, http_a, &Node { certs: certs.clone(), bootstrap: vec![], authority: true, port: None }, false).await;
    let (b, _) = started_as(&dir_b, http_b, &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: false, port: Some(port_b) }, false).await;
    let _bridge = b.mcp().connect_mcp_server(format!("http://127.0.0.1:{mock_port}/")).await.expect("bridged");
    let provided = || crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &format!("tools/{TOOL}/")).iter().any(|(k, _)| k.ends_with(&b.node_id().to_string()))
        && mycelium_core::ops::kv_get(&a.task_ctx, &format!("sys/caller-context/{}", b.node_id())).is_some();
    assert!(until(provided, Duration::from_secs(15)).await, "A learned that B serves the tool");
    let key = forecaster_key_in(&a, &certs, &dir_a.join("client"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let args = serde_json::json!({"city": "Arles"});
    let (asked, grant) = ask_and_issue(http_a, &args).await;
    let resource = asked["resource"].as_str().unwrap();
    let first = mcp_call(http_a, &args, Some(presented(&grant, resource, &args, &key))).await;
    let again = mcp_call(http_a, &args, Some(presented(&grant, resource, &args, &key))).await;
    b.shutdown().await;
    a.shutdown().await;
    server.abort();
    for d in [&dir_a, &dir_b] {
        let _ = std::fs::remove_dir_all(d);
    }
    assert!(again["error"]["message"].as_str().is_some_and(|m| m.contains("already used")), "GIVEN BACK: the grant admitted a second call: {again}");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the effect ran once");
    assert_eq!(first["error"]["code"], -32000, "the bridged answer is a tool error, not the node's refusal: {first}");
}

/// **The request door refuses arguments no digest binds** (identity-and-lifecycle I3, §9 O1): a number of magnitude
/// 2^53 or more — an integer past ±(2^53 − 1), or a float such as `1e21` — is refused `400 unsafe_integer` before a
/// grant is asked for, since the grant's RFC 8785 digest writes it as the nearest double. A float in exponent form
/// below that bound is accepted. Seen failing first with the door's check removed (the request was written).
#[tokio::test]
async fn the_request_door_refuses_an_unsafe_integer_and_accepts_an_exponent_form_float() {
    let dir = scratch("unsafe");
    let http_port = alloc_port();
    let (a, _ran) = started(&dir, http_port).await;
    let _key = forecaster_key(&a, &dir);
    let ask = |args: serde_json::Value| http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args})));
    let (big_status, big) = ask(serde_json::json!({"id": 9_007_199_254_740_993u64})).await;
    let (huge_status, huge) = ask(serde_json::json!({"huge": 1e21})).await;
    let (float_status, float) = ask(serde_json::json!({"tolerance": 0.00001})).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((big_status, big["error"].as_str()), (400, Some("unsafe_integer")), "UNSAFE: {big}");
    assert_eq!((huge_status, huge["error"].as_str()), (400, Some("unsafe_integer")), "{huge}");
    assert_eq!(float_status, 200, "an exponent-form float is asked for: {float}");
    assert_eq!(float["arguments_digest"], crate::agent::action_evaluator::hex32(&crate::arguments_digest(&serde_json::json!({"tolerance": 0.00001}))));
}

/// **A request asked before identity-and-lifecycle I3 is the same request for the window, and its grant runs**
/// (#750's review, items 2 and 4). A standing request recorded with the legacy digest — as a pre-I3 gateway wrote it,
/// for arguments whose two forms differ — is reused when the client asks again; the grant issued on it binds the
/// legacy digest, and a client proving possession over the RFC 8785 digest runs the call under it (either digest of
/// the call's own arguments binds exactly them). The match is counted on `/stats`, once at the door and once at
/// the provider (this node is both). Seen failing first, one build per plant: the reuse matching the RFC 8785 digest only (a second request
/// written), and the binding and the proof required to share one form (the call refused, bound to other arguments).
#[tokio::test]
async fn a_request_asked_before_i3_is_reused_and_its_grant_runs_counted_on_stats() {
    use crate::agent::action_evaluator::hex32;
    let dir = scratch("legacy");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"\u{ff01}": 1, "\u{1F600}": 2});
    let legacy = hex32(&crate::legacy_arguments_digest(&args));
    assert_ne!(legacy, hex32(&crate::arguments_digest(&args)), "the case is one where the two forms differ");
    let ask = || http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args})));
    let (status, first) = ask().await;
    assert_eq!(status, 200, "{first}");
    // The record as a pre-I3 gateway wrote it: the legacy digest.
    let request_key = first["key"].as_str().unwrap().to_string();
    let mut record: serde_json::Value = serde_json::from_slice(&mycelium_core::ops::kv_get(&a.task_ctx, &request_key).unwrap()).unwrap();
    record["arguments_digest"] = serde_json::json!(legacy);
    assert!(a.kv().set(request_key.clone(), bytes::Bytes::from(serde_json::to_vec(&record).unwrap())));
    let (_, again) = ask().await;
    let request = again["request"].as_str().unwrap_or_default().to_string();
    let (issue_status, issued) = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(&request)), Some(serde_json::json!({"decision": "issue"}))).await;
    let (_, read) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(&request)), None).await;
    let called = match serde_json::from_value::<crate::mandate::grant::SignedMandateGrant>(read["grant"].clone()) {
        Ok(grant) => mcp_call(http_port, &args, Some(presented(&grant, first["resource"].as_str().unwrap(), &args, &key))).await,
        Err(e) => serde_json::json!({"error": format!("no grant: {e}: {read}")}),
    };
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((again["reused"].as_bool(), &again["request"]), (Some(true), &first["request"]), "REUSE: the pre-I3 request was not reused: {again}");
    assert_eq!(again["arguments_digest"], legacy.as_str(), "the answer names the digest the grant will bind: {again}");
    assert_eq!(issue_status, 200, "{issued}");
    assert!(called.get("result").is_some(), "PAIRING: the grant bound to the legacy digest, proved over RFC 8785, did not run: {called}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // This node is the gateway and, with provider enforcement, the provider: one call, counted once at each point.
    assert_eq!((stats["legacy_digest_matches"].as_u64(), stats["provider_legacy_digest_matches"].as_u64()), (Some(1), Some(1)), "STATS: {stats}");
}

/// **A provider that does not yet trust the gateway's key gives the grant back** (#752's review, item 1). A provider
/// that has just joined may not yet hold a key for the gateway — its own handshake to it, or the gateway's certificate
/// record, has not arrived — and refuses the gateway's caller context `unknown_signer` at its receive boundary,
/// before anything runs (`gateway_caller::verify`). That refusal gives the gateway's claim of a single-use grant
/// back, as `client_key_unknown` does, so the same grant runs once the provider has learned the key. Two nodes: the
/// gateway A carries the authority; B serves the tool natively and enforces nothing of its own. The plant: B's
/// standing for A anchored to no key, so `trusted_keys(A)` is empty at B; B's own fold of the store restores it.
/// Seen failing first with `unknown_signer` absent from `pre_execution_refusal` (the retry refused: the grant already
/// used at the gateway).
#[tokio::test]
async fn a_provider_that_does_not_yet_trust_the_gateway_gives_the_grant_back() {
    let dir_a = scratch("signer-a");
    let dir_b = scratch("signer-b");
    let certs = dir_a.join("tls");
    let (http_a, http_b, port_b) = (alloc_port(), alloc_port(), alloc_port());
    let at_b = format!("tool:{TOOL}@127.0.0.1:{port_b}");
    std::fs::write(dir_a.join("units").join("weather.toml"), UNIT.replace("resource       = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\"")).replace("resource  = \"tool:weather.forecast\"", &format!("resource = \"{at_b}\""))).unwrap();
    let (a, _) = started_as(&dir_a, http_a, &Node { certs: certs.clone(), bootstrap: vec![], authority: true, port: None }, false).await;
    let (b, ran) = started_as(&dir_b, http_b, &Node { certs: certs.clone(), bootstrap: vec![a.node_id().clone()], authority: false, port: Some(port_b) }, true).await;
    let a_id = a.node_id().clone();
    let provided = || crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &format!("tools/{TOOL}/")).iter().any(|(k, _)| k.ends_with(&b.node_id().to_string()))
        && mycelium_core::ops::kv_get(&a.task_ctx, &format!("sys/caller-context/{}", b.node_id())).is_some();
    assert!(until(provided, Duration::from_secs(15)).await, "A learned that B serves the tool");
    // B's standing for A anchored, by its certificate record, before the plant (so the restore below re-derives it).
    let anchored = || b.task_ctx.key_standing.get(&a_id).is_some_and(|s| s.anchored.as_ref().is_some_and(|k| !k.is_empty()));
    assert!(until(anchored, Duration::from_secs(15)).await, "B anchored A's key from its certificate record");
    let key = forecaster_key_in(&a, &certs, &dir_a.join("client"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let args = serde_json::json!({"city": "Nancy"});
    let (asked, grant) = ask_and_issue(http_a, &args).await;
    let resource = asked["resource"].as_str().unwrap().to_string();

    // The plant: B holds no key it trusts for A.
    b.task_ctx.key_standing.set(&a_id, mycelium_core::removal::NodeStanding { anchored: Some(Default::default()), revoked: Default::default(), counted: Default::default() });
    let untrusted = b.trusted_identity_keys(&a_id).is_empty();
    let first = mcp_call(http_a, &args, Some(presented(&grant, &resource, &args, &key))).await;
    let ran_first = ran.load(Ordering::SeqCst);
    // B learns A's key: its own fold of the store re-anchors it.
    crate::agent::revocation::refresh(&b.task_ctx);
    let trusted = !b.trusted_identity_keys(&a_id).is_empty();
    let again = mcp_call(http_a, &args, Some(presented(&grant, &resource, &args, &key))).await;
    let ran_again = ran.load(Ordering::SeqCst);
    b.shutdown().await;
    a.shutdown().await;
    for d in [&dir_a, &dir_b] {
        let _ = std::fs::remove_dir_all(d);
    }
    assert!(untrusted, "the plant: B trusts no key for A");
    assert!(first.to_string().contains("unknown_signer"), "the provider refused the gateway's caller context, by name: {first}");
    assert_eq!(ran_first, 0, "nothing ran");
    assert!(trusted, "B trusts A's key again after its fold");
    assert!(again.get("result").is_some(), "SPENT: the retry under the same grant was refused: {again}");
    assert_eq!(ran_again, 1, "the tool ran once");
}

/// **A signed decision is the decider's, wherever it is carried** (identity-and-lifecycle I3, D3 and D5). The
/// treasurer, holding a client certificate and `grant:issue`, signs its answers; a relay holding only `grant:relay`
/// carries them. Under `grant:relay` an unsigned body is refused (`decision_unsigned`); a signed one presented for
/// another request is refused (`decision_retargeted`); one addressed to another node is refused (`not_addressed_here`);
/// one signed with a key that is not the named decider's is refused (`not_the_decider`); one signed by a principal
/// without `grant:issue` here is refused (`decider_lacks_scope`); a token holding neither scope is refused naming both.
/// The honest relayed issue succeeds with the **treasurer** as the actor: the grant's purpose and the evidence record
/// name the signer, the relay is named beside it, and the grant runs; the relayed signed revocation revokes it. An
/// unsigned decision by the treasurer's own bearer still succeeds this release and is counted
/// (`grant_decisions_unsigned`). Seen failing first, one build per plant: listed in the PR.
#[tokio::test]
async fn a_signed_decision_relayed_under_grant_relay_is_the_signers() {
    use crate::agent::action_evaluator::{AeEvidence, RecordKind};
    let dir = scratch("decide");
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let fc_key = forecaster_key(&a, &dir);
    let treasurer_key = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    const TREASURER: &str = "token:fleet/treasurer";
    let node = a.node_id().to_string();
    let ask = |args: serde_json::Value| http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": args})));
    let (args1, args2, args3) = (serde_json::json!({"city": "Lyon"}), serde_json::json!({"city": "Metz"}), serde_json::json!({"city": "Brest"}));
    let (_, r1) = ask(args1.clone()).await;
    let (_, r2) = ask(args2.clone()).await;
    let (_, r3) = ask(args3.clone()).await;
    let (req1, req2, req3) = (r1["request"].as_str().unwrap().to_string(), r2["request"].as_str().unwrap().to_string(), r3["request"].as_str().unwrap().to_string());
    let sign = |key: &ed25519_dalek::SigningKey, decider: &str, node: &str, request: &str, decision: &str, ttl: Option<u64>, reason: Option<&str>| {
        crate::sign_grant_decision(key, decider, node, FORECASTER, request, decision, ttl, reason, mycelium_core::sim_seam::wall_now_ms())
    };
    let decide = |bearer: &'static str, request: String, body: serde_json::Value| async move {
        http(http_port, bearer, reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(&request)), Some(body)).await
    };
    let issue_signed = |s: crate::SignedDecision| serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": s});

    let unsigned = decide("relay-secret", req1.clone(), serde_json::json!({"decision": "issue", "ttl_secs": 600})).await;
    let retargeted = decide("relay-secret", req2.clone(), issue_signed(sign(&treasurer_key, TREASURER, &node, &req1, "issue", Some(600), None))).await;
    let elsewhere = decide("relay-secret", req1.clone(), issue_signed(sign(&treasurer_key, TREASURER, "127.0.0.1:1", &req1, "issue", Some(600), None))).await;
    let foreign = decide("relay-secret", req1.clone(), issue_signed(sign(&fc_key, TREASURER, &node, &req1, "issue", Some(600), None))).await;
    let no_scope = decide("relay-secret", req1.clone(), issue_signed(sign(&fc_key, FORECASTER, &node, &req1, "issue", Some(600), None))).await;
    let other_terms = decide("relay-secret", req1.clone(), issue_signed(sign(&treasurer_key, TREASURER, &node, &req1, "issue", Some(60), None))).await;
    let neither = decide("fc-secret", req1.clone(), issue_signed(sign(&treasurer_key, TREASURER, &node, &req1, "issue", Some(600), None))).await;
    let honest = decide("relay-secret", req1.clone(), issue_signed(sign(&treasurer_key, TREASURER, &node, &req1, "issue", Some(600), None))).await;
    let (_, read) = http(http_port, "fc-secret", reqwest::Method::GET, &format!("/gateway/grants/request/{}", segment(&req1)), None).await;
    let grant: Option<crate::mandate::grant::SignedMandateGrant> = serde_json::from_value(read["grant"].clone()).ok();
    let called = match &grant {
        Some(g) => mcp_call(http_port, &args1, Some(presented(g, r1["resource"].as_str().unwrap(), &args1, &fc_key))).await,
        None => serde_json::json!({"error": format!("no grant: {read}")}),
    };
    // The second request: issued signed, then revoked signed, both relayed.
    let _ = decide("relay-secret", req2.clone(), issue_signed(sign(&treasurer_key, TREASURER, &node, &req2, "issue", Some(600), None))).await;
    let revoked = http(http_port, "relay-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{}/revoke", segment(FORECASTER), segment(&req2)),
        Some(serde_json::json!({"reason": "season over", "signed": sign(&treasurer_key, TREASURER, &node, &req2, "revoke", None, Some("season over"))}))).await;
    let revocation: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{req2}/revoked", crate::agent::gateway_authority::grant_holder_segment(FORECASTER)))
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    // The treasurer's own bearer, unsigned: admitted this release, and counted.
    let own = decide("treasurer-secret", req3.clone(), serde_json::json!({"decision": "issue", "ttl_secs": 60})).await;
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    a.shutdown().await;
    let journal: Vec<AeEvidence> = crate::read_evidence_journal(&dir.join("evidence.log")).unwrap().iter().filter_map(|b| serde_json::from_slice(b).ok()).collect();
    let _ = std::fs::remove_dir_all(&dir);

    let code = |r: &(u16, serde_json::Value)| (r.0, r.1["error"].as_str().map(str::to_string));
    assert_eq!(code(&unsigned), (403, Some("decision_unsigned".into())), "UNSIGNED: {}", unsigned.1);
    assert_eq!(code(&retargeted), (403, Some("decision_retargeted".into())), "RETARGET: {}", retargeted.1);
    assert_eq!(code(&elsewhere), (409, Some("not_addressed_here".into())), "ADDRESSED: {}", elsewhere.1);
    assert_eq!(code(&foreign), (403, Some("not_the_decider".into())), "FOREIGN: {}", foreign.1);
    assert_eq!(code(&no_scope), (403, Some("decider_lacks_scope".into())), "DECIDER_SCOPE: {}", no_scope.1);
    assert_eq!(code(&other_terms), (403, Some("not_the_decider".into())), "TERMS: a body whose terms differ from the signed ones: {}", other_terms.1);
    assert_eq!((neither.0, neither.1["or_scope"].as_str()), (403, Some("grant:relay")), "NEITHER: {}", neither.1);
    assert_eq!(honest.0, 200, "RELAY: the relayed signed issue: {}", honest.1);
    let grant = grant.expect("a grant was issued");
    assert!(grant.mandate.purpose.contains(TREASURER) && grant.mandate.purpose.contains("token:fleet/relay"), "PURPOSE: {}", grant.mandate.purpose);
    assert!(called.get("result").is_some(), "the relayed grant runs: {called}");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    let issue_record = journal.iter().find(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue" && r.subject == TREASURER);
    assert!(issue_record.is_some(), "JOURNAL: the decision's actor is the signer: {:?}", journal.iter().filter(|r| r.operation == "grant.issue").map(|r| &r.subject).collect::<Vec<_>>());
    assert!(!journal.iter().any(|r| r.operation == "grant.issue" && r.subject == "token:fleet/relay"), "the relay is never the actor");
    assert_eq!(revoked.0, 200, "REVOKE: {}", revoked.1);
    assert_eq!((revocation["issuer"].as_str(), revocation["relayed_by"].as_str()), (Some(TREASURER), Some("token:fleet/relay")), "{revocation}");
    assert_eq!(own.0, 200, "{}", own.1);
    assert_eq!(stats["grant_decisions_unsigned"].as_u64(), Some(1), "COUNTED: {stats}");
}

// ── #754's review: signed decisions, the edges ─────────────────────────────────────────────────────────────────────

const DECIDER: &str = "token:fleet/treasurer";

/// A treasurer-signed decision on the forecaster's request `request`, at `t`.
fn signed_at(key: &ed25519_dalek::SigningKey, node: &str, request: &str, decision: &str, ttl: Option<u64>, reason: Option<&str>, t: u64) -> crate::SignedDecision {
    crate::sign_grant_decision(key, DECIDER, node, FORECASTER, request, decision, ttl, reason, t)
}

async fn post_decision(http_port: u16, bearer: &str, request: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    http(http_port, bearer, reqwest::Method::POST, &format!("/gateway/grants/{}/{}", segment(FORECASTER), segment(request)), Some(body)).await
}

async fn post_revoke(http_port: u16, bearer: &str, request: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    http(http_port, bearer, reqwest::Method::POST, &format!("/gateway/grants/{}/{}/revoke", segment(FORECASTER), segment(request)), Some(body)).await
}

fn code(r: &(u16, serde_json::Value)) -> (u16, Option<String>) {
    (r.0, r.1["error"].as_str().map(str::to_string))
}

/// A started node with the forecaster's and the treasurer's certificates, and `n` requests by the forecaster.
async fn decision_fleet(tag: &str, n: usize) -> (PathBuf, u16, Arc<GossipAgent>, Arc<AtomicUsize>, ed25519_dalek::SigningKey, Vec<String>) {
    let dir = scratch(tag);
    let http_port = alloc_port();
    let (a, ran) = started(&dir, http_port).await;
    let _ = forecaster_key(&a, &dir);
    let treasurer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer"), "treasurer");
    let mut requests = Vec::new();
    for i in 0..n {
        let (status, asked) = http(http_port, "fc-secret", reqwest::Method::POST, "/gateway/grants/request", Some(serde_json::json!({"tool": TOOL, "arguments": {"city": "Lyon", "n": i}}))).await;
        assert_eq!(status, 200, "{asked}");
        requests.push(asked["request"].as_str().unwrap().to_string());
    }
    (dir, http_port, a, ran, treasurer, requests)
}

fn journal_of(dir: &Path) -> Vec<crate::agent::action_evaluator::AeEvidence> {
    crate::read_evidence_journal(&dir.join("evidence.log")).unwrap().iter().filter_map(|b| serde_json::from_slice(b).ok()).collect()
}

/// **Concurrent copies of one decision are answered once** (#754's review, finding 1). Sixteen copies of one signed
/// issue, sixteen of one unsigned issue and sixteen of one signed revocation, each batch sent at once: in each, exactly
/// one is answered `200`, the rest `already_answered` / `already_revoked`, or `409 decision_in_flight` while the first
/// is still writing; one grant is signed per request, one `Decided` record written per answer, the unsigned decision
/// counted once; and the request's checkpoint ends at the revocation's sequence. Seen failing first: before the claim
/// every copy was answered `200`, each a separately signed grant with its own evidence record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_copies_of_one_decision_are_answered_once() {
    use crate::agent::action_evaluator::RecordKind;
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("concurrent", 2).await;
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let n = 16;
    let issue = serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed_at(&treasurer, &node, &requests[0], "issue", Some(600), None, now)});
    let signed_issues = futures_util::future::join_all((0..n).map(|_| post_decision(http_port, "relay-secret", &requests[0], issue.clone()))).await;
    let unsigned_issues = futures_util::future::join_all((0..n).map(|_| post_decision(http_port, "treasurer-secret", &requests[1], serde_json::json!({"decision": "issue", "ttl_secs": 60})))).await;
    let revoke = serde_json::json!({"reason": "season over", "signed": signed_at(&treasurer, &node, &requests[0], "revoke", None, Some("season over"), now)});
    let revokes = futures_util::future::join_all((0..n).map(|_| post_revoke(http_port, "relay-secret", &requests[0], revoke.clone()))).await;
    let (_, stats) = http(http_port, "treasurer-secret", reqwest::Method::GET, "/stats", None).await;
    let checkpoint: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{}/checkpoint", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), requests[0]))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    for (name, batch, done) in [("SIGNED", &signed_issues, "already_answered"), ("UNSIGNED", &unsigned_issues, "already_answered"), ("REVOKE", &revokes, "already_revoked")] {
        let ok = batch.iter().filter(|r| r.0 == 200).count();
        let others: Vec<_> = batch.iter().filter(|r| r.0 != 200).map(code).collect();
        assert_eq!(ok, 1, "{name}: {ok} of {n} answered 200; the rest {others:?}");
        assert!(others.iter().all(|c| c == &(409, Some(done.into())) || c == &(409, Some("decision_in_flight".into()))), "{name}: {others:?}");
    }
    let decided = |op: &str| journal.iter().filter(|r| r.kind == RecordKind::Decided && r.operation == op && r.subject == DECIDER).count();
    assert_eq!((decided("grant.issue"), decided("grant.revoke")), (2, 1), "EVIDENCE: one Decided record per answer");
    assert_eq!(stats["grant_decisions_unsigned"].as_u64(), Some(1), "COUNTED once: {stats}");
    assert_eq!(checkpoint["checkpoint"]["seq"].as_u64(), Some(2), "CHECKPOINT: the revocation's sequence stands: {checkpoint}");
}

/// **The window a signed decision is fresh in** (#754's review, findings 2 and 7): `CALLER_CONTEXT_MAX_AGE_MS`
/// (300 s) plus `max_clock_drift_ms` into the past, `max_clock_drift_ms` into the future — 600 s and 300 s at the
/// default drift. Just inside each edge is admitted; just past each is refused `stale`. A body whose key is not base64
/// is refused `400 decision_signature_malformed`.
#[tokio::test]
async fn a_signed_decision_is_fresh_inside_its_window_and_refused_stale_or_malformed_outside() {
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("stale", 5).await;
    let node = a.node_id().to_string();
    let drift = a.task_ctx.config.max_clock_drift_ms;
    let (past, ahead) = (crate::agent::gateway_caller::CALLER_CONTEXT_MAX_AGE_MS + drift, if drift == 0 { crate::agent::gateway_caller::CALLER_CONTEXT_MAX_AGE_MS } else { drift });
    let now = mycelium_core::sim_seam::wall_now_ms();
    let at = |request: &str, t: u64| serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed_at(&treasurer, &node, request, "issue", Some(600), None, t)});
    let too_old = post_decision(http_port, "relay-secret", &requests[0], at(&requests[0], now - past - 5_000)).await;
    let too_new = post_decision(http_port, "relay-secret", &requests[1], at(&requests[1], now + ahead + 5_000)).await;
    let old_inside = post_decision(http_port, "relay-secret", &requests[2], at(&requests[2], now - past + 5_000)).await;
    let new_inside = post_decision(http_port, "relay-secret", &requests[3], at(&requests[3], now + ahead - 5_000)).await;
    let mut bad = at(&requests[4], now);
    bad["signed"]["k"] = serde_json::json!("not base64!");
    let malformed = post_decision(http_port, "relay-secret", &requests[4], bad).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code(&too_old), (403, Some("stale".into())), "STALE_PAST: {}", too_old.1);
    assert_eq!(code(&too_new), (403, Some("stale".into())), "STALE_FUTURE: {}", too_new.1);
    assert_eq!(old_inside.0, 200, "{}", old_inside.1);
    assert_eq!(new_inside.0, 200, "{}", new_inside.1);
    assert_eq!(code(&malformed), (400, Some("decision_signature_malformed".into())), "{}", malformed.1);
}

/// **A decider's superseded key is not the decider's** (#754's review, finding 7): after a newer certificate for the
/// treasurer, a decision signed with the older key is refused `not_the_decider`, and the newer key's is admitted.
#[tokio::test]
async fn a_decision_signed_with_a_superseded_key_is_not_the_deciders() {
    let (dir, http_port, a, _ran, older, requests) = decision_fleet("superseded", 2).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let newer = client_key_in(&a, &dir.join("tls"), &dir.join("client-treasurer-2"), "treasurer");
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let by = |key: &ed25519_dalek::SigningKey, request: &str| serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed_at(key, &node, request, "issue", Some(600), None, now)});
    let by_older = post_decision(http_port, "relay-secret", &requests[0], by(&older, &requests[0])).await;
    let by_newer = post_decision(http_port, "relay-secret", &requests[1], by(&newer, &requests[1])).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code(&by_older), (403, Some("not_the_decider".into())), "SUPERSEDED: {}", by_older.1);
    assert!(by_older.1["detail"].as_str().unwrap_or_default().contains("newer certificate"), "{}", by_older.1);
    assert_eq!(by_newer.0, 200, "{}", by_newer.1);
}

/// **A signed refusal, replays one at a time, and the evidence kept** (#754's review, findings 4 and 7). A signed
/// refusal is answered and its record keeps the signed body and the relay; repeated, it is `already_answered`. A signed
/// issue, repeated, is `already_answered`; its revocation, repeated, `already_revoked`; the old issue after the
/// revocation, `already_answered`. Each `Decided` record of a signed decision carries `signed_decision` — the body,
/// the terms and `relayed_by` — and its signature re-verifies from the record alone. Seen failing first with the body
/// dropped after verification (the records said only `"signed": true`).
#[tokio::test]
async fn a_signed_refusal_and_replays_one_at_a_time_are_answered_once_and_the_evidence_kept() {
    use crate::agent::action_evaluator::RecordKind;
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("replay", 2).await;
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let refuse = serde_json::json!({"decision": "refuse", "reason": "not this season", "signed": signed_at(&treasurer, &node, &requests[0], "refuse", None, Some("not this season"), now)});
    let refused = post_decision(http_port, "relay-secret", &requests[0], refuse.clone()).await;
    let refused_again = post_decision(http_port, "relay-secret", &requests[0], refuse).await;
    let issue = serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed_at(&treasurer, &node, &requests[1], "issue", Some(600), None, now)});
    let issued = post_decision(http_port, "relay-secret", &requests[1], issue.clone()).await;
    let issued_again = post_decision(http_port, "relay-secret", &requests[1], issue.clone()).await;
    let revoke = serde_json::json!({"reason": "season over", "signed": signed_at(&treasurer, &node, &requests[1], "revoke", None, Some("season over"), now)});
    let revoked = post_revoke(http_port, "relay-secret", &requests[1], revoke.clone()).await;
    let revoked_again = post_revoke(http_port, "relay-secret", &requests[1], revoke).await;
    let issued_after = post_decision(http_port, "relay-secret", &requests[1], issue).await;
    let segment_hex = crate::agent::gateway_authority::grant_holder_segment(FORECASTER);
    let record = |key: String| -> serde_json::Value { mycelium_core::ops::kv_get(&a.task_ctx, &key).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() };
    let refusal = record(format!("grant/{segment_hex}/{}/refused", requests[0]));
    let revocation = record(format!("grant/{segment_hex}/{}/revoked", requests[1]));
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(refused.0, 200, "{}", refused.1);
    assert_eq!(code(&refused_again), (409, Some("already_answered".into())));
    assert_eq!(issued.0, 200, "{}", issued.1);
    assert_eq!(code(&issued_again), (409, Some("already_answered".into())));
    assert_eq!(revoked.0, 200, "{}", revoked.1);
    assert_eq!(code(&revoked_again), (409, Some("already_revoked".into())));
    assert_eq!(code(&issued_after), (409, Some("already_answered".into())));
    for (name, rec) in [("refusal", &refusal), ("revocation", &revocation)] {
        assert_eq!(rec["relayed_by"].as_str(), Some("token:fleet/relay"), "{name}: {rec}");
        let signed: Result<crate::SignedDecision, _> = serde_json::from_value(rec["signed"].clone());
        assert!(signed.is_ok(), "KEPT: the {name} record keeps the signed body: {rec}");
    }
    for op in ["grant.refuse", "grant.issue", "grant.revoke"] {
        let r = journal.iter().find(|r| r.kind == RecordKind::Decided && r.operation == op && r.subject == DECIDER).unwrap_or_else(|| panic!("no Decided {op}"));
        let kept = r.signed_decision.clone().unwrap_or_default();
        assert_eq!(kept["relayed_by"].as_str(), Some("token:fleet/relay"), "EVIDENCE: {op}: {kept}");
        let signed: crate::SignedDecision = serde_json::from_value(kept["signed"].clone()).unwrap_or_else(|e| panic!("EVIDENCE: {op}: {e}: {kept}"));
        let decision = kept["decision"].as_str().unwrap();
        assert!(signed.verify_signature(decision, kept["ttl_secs"].as_u64(), kept["reason"].as_str()), "RE-VERIFY: {op}: {kept}");
    }
}

/// **A signed decision needs `gateway_identity_issuer`** (#754's review, finding 5), as I1's doors refuse a signed call
/// without it: this gateway's principals would be qualified by its node id, which no client certificate names.
#[tokio::test]
async fn a_signed_decision_at_a_gateway_without_an_identity_issuer_is_refused_by_name() {
    let dir = scratch("no-issuer");
    let http_port = alloc_port();
    let (id, mut cfg) = config(&dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: None });
    cfg.gateway_identity_issuer = None;
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("starts");
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let body = serde_json::json!({"decision": "issue", "signed": signed_at(&key, &a.node_id().to_string(), "r1", "issue", None, None, mycelium_core::sim_seam::wall_now_ms())});
    let answer = post_decision(http_port, "relay-secret", "r1", body).await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code(&answer), (409, Some("gateway_identity_issuer_unset".into())), "ISSUER: {}", answer.1);
}

/// **A node without an identity cannot judge a decider's key** (#754's review, finding 7): `decision_unverifiable`
/// where `[tls]` is unset — checked on the verifier directly, since such a node signs no grant to decide on.
#[test]
fn a_node_without_an_identity_cannot_verify_a_signed_decision() {
    let a = GossipAgent::new(NodeId::new("127.0.0.1", alloc_port()).unwrap(), GossipConfig::default());
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let signed = signed_at(&key, &a.node_id().to_string(), "r1", "issue", None, None, 1);
    let got = crate::agent::grant_decision::verify(&a.task_ctx, &signed, FORECASTER, "r1", "issue", None, None, 1);
    assert_eq!(got.map_err(|e| e.code()), Err("decision_unverifiable"));
}

/// **An issue never lowers a later checkpoint** (#754's review, finding 1's open question). Before the claim, a stale
/// concurrent copy of an issue could write the first checkpoint (sequence 1) after a revocation wrote the second, and
/// the KV keeps the later write. With the claim no route reaches that order, so it is posed directly: a revocation's
/// checkpoint placed beside a pending request, then the issue — refused `already_revoked`, the checkpoint unchanged.
/// Seen failing first with the guard removed (the issue answered and the checkpoint lowered to 1). The refused issue's
/// `Decided` record is followed by its execution, `None` — nothing ran: the guard stands before the checkpoint is
/// offered or written (#754's confirm round, residual b; #756's review, M1 and L5) — seen failing first with the guard
/// returning without one, and again with it recording `Failed`.
#[tokio::test]
async fn an_issue_never_lowers_a_later_checkpoint() {
    let (dir, http_port, a, _ran, treasurer, requests) = decision_fleet("monotonic", 2).await;
    let node = a.node_id().to_string();
    let now = mycelium_core::sim_seam::wall_now_ms();
    let issue = |request: &str| serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed_at(&treasurer, &node, request, "issue", Some(600), None, now)});
    let _ = post_decision(http_port, "relay-secret", &requests[0], issue(&requests[0])).await;
    let revoke = serde_json::json!({"reason": "r", "signed": signed_at(&treasurer, &node, &requests[0], "revoke", None, Some("r"), now)});
    let revoked = post_revoke(http_port, "relay-secret", &requests[0], revoke).await;
    let segment_hex = crate::agent::gateway_authority::grant_holder_segment(FORECASTER);
    let later = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{segment_hex}/{}/checkpoint", requests[0])).unwrap();
    let beside = format!("grant/{segment_hex}/{}/checkpoint", requests[1]);
    assert!(a.kv().set(beside.clone(), later));
    let answer = post_decision(http_port, "relay-secret", &requests[1], issue(&requests[1])).await;
    let held: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &beside).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    use crate::agent::action_evaluator::{Execution, RecordKind};
    // The refused issue's own decision: the `Decided` record whose signed body names the second request.
    let second = journal
        .iter()
        .find(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue" && r.signed_decision.as_ref().is_some_and(|d| d["signed"]["request"] == requests[1].as_str()))
        .map(|r| r.operation_id.clone())
        .expect("the refused issue was decided");
    let executions: Vec<Execution> = journal.iter().filter(|r| r.kind == RecordKind::Execution && r.operation_id == second).map(|r| r.execution).collect();
    assert_eq!(executions, vec![Execution::None], "RECORDED: the refused issue's execution, nothing ran");
    assert_eq!(revoked.0, 200, "{}", revoked.1);
    assert_eq!(code(&answer), (409, Some("already_revoked".into())), "MONOTONIC: {}", answer.1);
    assert_eq!(held["checkpoint"]["seq"].as_u64(), Some(2), "the later checkpoint stands: {held}");
}


/// **A relay cannot launder a decider the policy refuses** (#756's review, M2; the substrate half of §9 O7's refusal
/// case). The auditor holds a client certificate and `grant:issue`, so every door check passes, but no rule of the
/// unit permits it to issue. Its signed issue, carried by the relay, is refused by policy (`403`, the evaluator's
/// reason) and recorded `Decided` with the **auditor** — not the relay — as the subject, and no grant is written. A
/// unit's rules are allowances, so an actor no rule names is `Indeterminate` (authority not established), never a
/// permit; a deny would need a rule that prohibits, which the unit format has not. Seen failing first with the relay
/// used as the actor (the record's subject the relay).
#[tokio::test]
async fn a_relayed_signed_decision_the_policy_refuses_is_refused_with_the_decider_as_subject() {
    use crate::agent::action_evaluator::{DecisionKind, RecordKind};
    let (dir, http_port, a, _ran, _treasurer, requests) = decision_fleet("laundered", 1).await;
    let auditor = client_key_in(&a, &dir.join("tls"), &dir.join("client-auditor"), "auditor");
    const AUDITOR: &str = "token:fleet/auditor";
    let node = a.node_id().to_string();
    let signed = crate::sign_grant_decision(&auditor, AUDITOR, &node, FORECASTER, &requests[0], "issue", Some(600), None, mycelium_core::sim_seam::wall_now_ms());
    let answer = post_decision(http_port, "relay-secret", &requests[0], serde_json::json!({"decision": "issue", "ttl_secs": 600, "signed": signed})).await;
    let issued = mycelium_core::ops::kv_get(&a.task_ctx, &format!("grant/{}/{}/issued", crate::agent::gateway_authority::grant_holder_segment(FORECASTER), requests[0]));
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let door_codes = ["not_the_decider", "decision_retargeted", "not_addressed_here", "stale", "decider_lacks_scope", "decision_unsigned", "decision_in_flight"];
    assert_eq!(answer.0, 403, "{}", answer.1);
    assert!(!door_codes.contains(&answer.1["error"].as_str().unwrap_or_default()), "POLICY: a door refusal, not the evaluator's: {}", answer.1);
    assert!(issued.is_none(), "no grant was written");
    let decided: Vec<_> = journal.iter().filter(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue").collect();
    assert_eq!(decided.len(), 1, "{decided:?}");
    assert_eq!((decided[0].subject.as_str(), decided[0].decision), (AUDITOR, DecisionKind::Indeterminate), "SUBJECT: the decider is the actor refused");
}

/// **Every return after a permit records an execution** (#756's review, L1). A node with an execution authority and
/// no identity (`[tls]` unset) decides an issue — the evaluator permits the treasurer — and cannot sign it: `403
/// no_identity`, and the decision's execution is recorded `None`, nothing ran. Seen failing first with the return
/// recording nothing.
#[tokio::test]
async fn an_issue_the_node_cannot_sign_records_that_nothing_ran() {
    use crate::agent::action_evaluator::{Execution, RecordKind};
    let dir = scratch("no-identity");
    let http_port = alloc_port();
    let (id, mut cfg) = config(&dir, http_port, &Node { certs: dir.join("tls"), bootstrap: vec![], authority: true, port: None });
    cfg.tls = None;
    let a = Arc::new(GossipAgent::new(id, cfg));
    a.start().await.expect("a node with an authority and no identity starts");
    let agent = crate::agent::gateway_caller::node_principal(a.node_id());
    let request = "0000000000000001";
    let record = serde_json::json!({
        "request": request, "agent": agent, "tool": TOOL,
        "arguments_digest": crate::agent::action_evaluator::hex32(&crate::arguments_digest(&serde_json::json!({"city": "Lyon"}))),
        "resource": format!("tool:{TOOL}@{}", a.node_id()),
        "expires_at_ms": mycelium_core::sim_seam::wall_now_ms() + 60_000,
    });
    assert!(a.kv().set(format!("grant/{agent}/{request}"), bytes::Bytes::from(serde_json::to_vec(&record).unwrap())));
    let answer = http(http_port, "treasurer-secret", reqwest::Method::POST, &format!("/gateway/grants/{}/{request}", segment(&agent)), Some(serde_json::json!({"decision": "issue", "ttl_secs": 60}))).await;
    a.shutdown().await;
    let journal = journal_of(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code(&answer), (403, Some("no_identity".into())), "{}", answer.1);
    let decided = journal.iter().find(|r| r.kind == RecordKind::Decided && r.operation == "grant.issue").map(|r| r.operation_id.clone()).expect("the issue was decided");
    let executions: Vec<Execution> = journal.iter().filter(|r| r.kind == RecordKind::Execution && r.operation_id == decided).map(|r| r.execution).collect();
    assert_eq!(executions, vec![Execution::None], "RECORDED: nothing ran");
}

/// **Spike S-exchange (2026-11-11), throwaway: the envelope's size with a grant, a `cs` and an exchanged token.**
/// Frames a real envelope through the one producer — an `oidc:` principal of Keycloak's shape, `mcp:invoke`, the
/// mandate a client presents for a grant issued on this node, and a client signature — and reports its bytes. The
/// exchanged token is not a field yet; its cost is the bytes `,"ot":"<token>"` adds, at the sizes Keycloak 26.2.5
/// issued in the spike (1218 B, 1235 B with one dynamic-scope value). Never merges.
#[tokio::test]
async fn spike_s_exchange_envelope_size() {
    let dir = scratch("sx-size");
    let http_port = alloc_port();
    let (a, _ran) = started(&dir, http_port).await;
    let key = forecaster_key(&a, &dir);
    let args = serde_json::json!({"city": "Lyon", "days": 3});
    let (asked, grant) = ask_and_issue(http_port, &args).await;
    let resource = asked["resource"].as_str().unwrap().to_string();
    let mandate = presented(&grant, &resource, &args, &key)["mandate"].clone();
    let signer = crate::ClientSigner::new(key.clone(), FORECASTER);
    let cs = signer.sign_tools_call(TOOL, &args).unwrap();
    let principal = "oidc:http://127.0.0.1:18080/realms/fleet/5f1d2c7e-8a3b-4c9d-9e0f-1a2b3c4d5e6f";
    let app = bytes::Bytes::from(serde_json::to_vec(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": TOOL, "arguments": args}})).unwrap());
    let measure = |mandate: Option<&serde_json::Value>, client: Option<&(crate::ClientSignature, crate::ClientNonce)>| {
        let framed = crate::agent::gateway_caller::frame_with_client_signature_at(&a.task_ctx, a.node_id(), principal, &["mcp:invoke".to_string()], app.clone(), mandate, Some(&resource), client, 1_794_000_000_000).expect("fits today");
        framed.len() - 5 - 2 - app.len()
    };
    let bare = measure(None, None);
    let with_grant = measure(Some(&mandate), None);
    let with_grant_cs = measure(Some(&mandate), Some(&cs));
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let ot = |token: usize| token + r#","ot":"""#.len();
    println!("SX envelope bare={bare} +grant={with_grant} +grant+cs={with_grant_cs} cap={}", crate::agent::gateway_caller::MAX_ENVELOPE_BYTES);
    for token in [1218usize, 1235, 2_048, 4_096] {
        println!("SX with ot of {token} B: {} B ({} B under the cap)", with_grant_cs + ot(token), crate::agent::gateway_caller::MAX_ENVELOPE_BYTES as i64 - (with_grant_cs + ot(token)) as i64);
    }
}
