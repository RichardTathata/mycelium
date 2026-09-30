//! A3's exit gate (`docs/plans/design-time-tooling.md` §10): an unsigned or untrusted entry is
//! refused 403 by name; a signed one appears under `installable/` on a **second node**; a bearer
//! without `artifact:publish` is refused by the scope layer; a malformed line is 400; an entry
//! under a librarian-managed key is 409.

#![cfg(feature = "gateway")]

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use mycelium::{alloc_port, CapFilter, Capability, GatewayToken, GossipAgent, GossipConfig, NodeId};
use mycelium_wasm_host::{artifact_router, ArtifactId, InstallableCatalog, InstallableEntry};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_entry_reaches_a_second_node_and_the_refusals_are_by_name() {
    let (ga, ha, gb) = (alloc_port(), alloc_port(), alloc_port());
    let trusted_key = SigningKey::from_bytes(&[51u8; 32]);
    let stranger = SigningKey::from_bytes(&[52u8; 32]);
    let librarian_key = SigningKey::from_bytes(&[53u8; 32]);
    let trusted = trusted_key.verifying_key().to_bytes();

    let cfg_a = GossipConfig {
        bind_port: ga,
        http_port: Some(ha),
        gateway_scoped_tokens: vec![
            GatewayToken { token: "artpub".into(), scopes: vec!["artifact:publish".into()] },
            GatewayToken { token: "kvw".into(), scopes: vec!["kv:write".into()] },
        ],
        ..Default::default()
    };
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", ga).unwrap(), cfg_a));
    a.with_http_routes(artifact_router(
        Arc::clone(&a),
        vec![trusted, librarian_key.verifying_key().to_bytes()],
        Some(librarian_key.verifying_key().to_bytes()),
    ));
    a.start().await.unwrap();
    let cfg_b = GossipConfig {
        bind_port: gb,
        bootstrap_peers: vec![NodeId::new("127.0.0.1", ga).unwrap()],
        ..Default::default()
    };
    let b = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gb).unwrap(), cfg_b));
    b.start().await.unwrap();

    let http = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{ha}/gateway/artifacts/publish");
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{ha}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let post = |tok: &'static str, body: serde_json::Value| {
        let http = http.clone();
        let url = url.clone();
        async move {
            let r = http.post(url).bearer_auth(tok).json(&body).send().await.unwrap();
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            (status, body)
        }
    };
    let artifact = ArtifactId::of(b"an optimizer nobody uploads through a gateway");
    let entry = |name: &str| InstallableEntry::new(Capability::new("route", name), artifact).with_cost(10, 1);
    let line = |e: &InstallableEntry| serde_json::json!({ "entry_hex": hex(&e.encode()) });

    // The plant: a trusted publisher's signed line is written, and a second node sees it.
    let signed = entry("optimize").signed_by(&trusted_key);
    let (status, body) = post("artpub", line(&signed)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["key"], signed.kv_key());
    assert_eq!(body["signer"], format!("ed25519:{}", hex(&trusted)));
    let mut seen_on_b = false;
    for _ in 0..200 {
        let cat = InstallableCatalog::from_kv(&b.kv());
        if cat.resolve_best(&CapFilter::new("route", "optimize")).is_some_and(|e| e.artifact == artifact) {
            seen_on_b = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen_on_b, "the catalogue line gossips to a second node");
    assert!(InstallableCatalog::from_kv(&b.kv())
        .resolve_best(&CapFilter::new("route", "optimize"))
        .unwrap()
        .verify_provenance(&[trusted]), "what arrived is the signed line, provenance intact");

    // Refusals, each by name.
    let (status, body) = post("artpub", line(&entry("unsigned"))).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("unsigned entry")), "{body}");
    let (status, body) = post("artpub", line(&entry("foreign").signed_by(&stranger))).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("untrusted publisher")), "{body}");
    let mut tampered = entry("tampered").signed_by(&trusted_key);
    tampered.provides.name = Arc::from("tampered-after-signing");
    let (status, body) = post("artpub", line(&tampered)).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("provenance does not verify")), "{body}");
    let (status, body) = post("artpub", line(&entry("mirrored").signed_by(&librarian_key))).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("librarian-managed signer")), "{body}");
    let (status, body) = post("artpub", serde_json::json!({ "entry_hex": "zz" })).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("malformed entry")), "{body}");
    let (status, body) = post("artpub", serde_json::json!({ "entry_hex": "ff00" })).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("malformed entry")), "{body}");
    // The scope layer: kv:write is not artifact:publish, and no bearer is no door.
    let (status, body) = post("kvw", line(&signed)).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["required_scope"], "artifact:publish");
    let r = http.post(&url).json(&line(&signed)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    // Nothing refused was written: b still holds exactly the one line.
    assert_eq!(InstallableCatalog::from_kv(&b.kv()).entries().len(), 1);

    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_with_no_trusted_publishers_admits_nothing() {
    let (g, h) = (alloc_port(), alloc_port());
    let key = SigningKey::from_bytes(&[54u8; 32]);
    let cfg = GossipConfig {
        bind_port: g,
        http_port: Some(h),
        gateway_scoped_tokens: vec![GatewayToken { token: "artpub".into(), scopes: vec!["artifact:publish".into()] }],
        ..Default::default()
    };
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", g).unwrap(), cfg));
    a.with_http_routes(artifact_router(Arc::clone(&a), vec![], None));
    a.start().await.unwrap();
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{h}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let signed = InstallableEntry::new(Capability::new("route", "optimize"), ArtifactId::of(b"x")).signed_by(&key);
    let r = http
        .post(format!("http://127.0.0.1:{h}/gateway/artifacts/publish"))
        .bearer_auth("artpub")
        .json(&serde_json::json!({ "entry_hex": hex(&signed.encode()) }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "no trusted publishers configured");
    assert!(InstallableCatalog::from_kv(&a.kv()).entries().is_empty());
    a.shutdown().await;
}
