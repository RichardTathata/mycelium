//! Identity-and-lifecycle I1: client certificates — supersession, the publish refusals, the journal across a
//! restart, and the gateway door.

use super::client_cert::{client_cert_key, ClientKeyStanding, PublishRefusal};
use super::GossipAgent;
use crate::config::{GossipConfig, TlsConfig};
use crate::node_id::NodeId;
use crate::test_util::alloc_port;
use rustls::pki_types::pem::PemObject;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PRINCIPAL: &str = "token:fleet/forecaster";

fn fresh(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("myc-client-cert-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn persistence(dir: &Path) -> crate::config::PersistenceConfig {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse }
}

fn config(dir: &Path, port: u16, persist: bool) -> GossipConfig {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    // The development posture: the CA's key sits in `auto_cert_dir`, so the test issues client certificates from it.
    cfg.tls = Some(TlsConfig { auto_cert_dir: dir.join("certs"), ..TlsConfig::default() });
    if persist {
        cfg.persistence = Some(persistence(dir));
    }
    cfg
}

/// Issue a client certificate for `name` from the CA in `dir/certs`, returning its DER and key.
fn issue(dir: &Path, name: &str, out: &str) -> (Vec<u8>, [u8; 32]) {
    let (cert, _key) = crate::issue_client_cert(&dir.join("certs"), "fleet", name, &dir.join(out)).expect("issued from the node's CA");
    let der = rustls::pki_types::CertificateDer::from_pem_file(&cert).unwrap().as_ref().to_vec();
    let key = crate::client_identity(&der).unwrap().key;
    (der, key)
}

async fn started(cfg: GossipConfig, port: u16) -> Arc<GossipAgent> {
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    a.start().await.unwrap();
    a
}

/// **A newer certificate supersedes the principal's older key, for good** (I1's exit: a superseded client key
/// refused once the newer record is read, and still refused after that record is overwritten). K1 is current
/// until K2 — issued later for the same principal — is read; then K1 is superseded and K2 current. Overwriting
/// K2's record with garbage, or tombstoning it, does not bring K1 back: the mark only rises. A key no record
/// names, or another principal's, is unknown. Seen failing first with the fold judging only the records it
/// could see (no mark): K1 read as current again once K2's record was gone.
#[tokio::test]
async fn a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it() {
    let dir = fresh("supersede");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let (k1_der, k1) = issue(&dir, "forecaster", "k1");
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Unknown, "nothing published yet");
    a.publish_client_cert(&k1_der).expect("K1 published");
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Current);
    // A later serial: the issuance clock is milliseconds.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, k2) = issue(&dir, "forecaster", "k2");
    a.publish_client_cert(&k2_der).expect("K2 published");
    assert_eq!(a.client_key_standing(PRINCIPAL, &k2), ClientKeyStanding::Current);
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded);

    let k2_path: Arc<str> = Arc::from(client_cert_key(PRINCIPAL, &k2).as_str());
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&k2_path), bytes::Bytes::from_static(b"garbage"));
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded, "SUPERSESSION LOWERED: K2's record overwritten with garbage brought K1 back");
    let _ = super::helpers::kv_delete(&a.task_ctx, k2_path);
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded, "SUPERSESSION LOWERED: K2's record tombstoned brought K1 back");

    let (_, other) = issue(&dir, "forecaster", "k3-unpublished");
    assert_eq!(a.client_key_standing(PRINCIPAL, &other), ClientKeyStanding::Unknown, "a key no record names is unknown");
    assert_eq!(a.client_key_standing("token:fleet/somebody-else", &k1), ClientKeyStanding::Unknown, "another principal's key is unknown");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A newer record raises the mark when it arrives, not when its key is next asked about.** A member writes K2's
/// record directly (`kv().set`, as the namespace allows); the watcher folds it; the record is then overwritten
/// before anyone asks about either key — and K1 is still superseded. Without the fold on arrival the mark would
/// rise only at a query, which here comes after the overwrite.
#[tokio::test]
async fn a_record_written_by_a_member_supersedes_on_arrival_before_anyone_asks() {
    let dir = fresh("arrival");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let (k1_der, k1) = issue(&dir, "forecaster", "k1");
    a.publish_client_cert(&k1_der).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, k2) = issue(&dir, "forecaster", "k2");
    let k2_path: Arc<str> = Arc::from(client_cert_key(PRINCIPAL, &k2).as_str());
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&k2_path), bytes::Bytes::from(k2_der));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while a.task_ctx.client_certs.mark(PRINCIPAL).map(|m| m.key) != Some(k2) {
        assert!(std::time::Instant::now() < deadline, "the watcher never folded K2's record");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = super::helpers::kv_set(&a.task_ctx, k2_path, bytes::Bytes::from_static(b"garbage"));
    assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The publish door judges before it publishes.** A node certificate is no client certificate; a client
/// certificate from another CA is not the fleet's; one older than the principal's newest already read is refused
/// as superseded (publishing it would change nothing). None of them writes a record.
#[tokio::test]
async fn the_publish_refuses_a_node_certificate_a_foreign_ca_and_a_superseded_key() {
    let dir = fresh("refusals");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let node_der = a.task_ctx.tls.get().unwrap().node_cert_der();
    assert_eq!(a.publish_client_cert(&node_der).unwrap_err(), PublishRefusal::NotAClientCertificate);

    let foreign = fresh("refusals-foreign");
    crate::mint_ca(&foreign.join("certs")).unwrap();
    let (foreign_der, foreign_key) = issue(&foreign, "forecaster", "k");
    assert_eq!(a.publish_client_cert(&foreign_der).unwrap_err(), PublishRefusal::NotIssuedByTheFleetCa);
    assert_eq!(a.client_key_standing(PRINCIPAL, &foreign_key), ClientKeyStanding::Unknown);

    let (k1_der, _) = issue(&dir, "forecaster", "k1");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, _) = issue(&dir, "forecaster", "k2");
    a.publish_client_cert(&k2_der).unwrap();
    assert_eq!(a.publish_client_cert(&k1_der).unwrap_err(), PublishRefusal::Superseded);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&foreign);
}

/// **Under `[persistence]` a supersession survives a restart with the newer record gone** (I1's exit: still
/// refused after a restart). K2 supersedes K1; K2's record is tombstoned; the node restarts over the same
/// directory. The journal holds K2's mark, so K1 stays superseded and K2 current — a member's tombstone retires no
/// key, only a newer certificate does. The plant: the same restart with the journal
/// removed brings K1 back as current — the journal is what holds it. Seen failing first: without the journal
/// open at start, K1 was current after the restart.
#[tokio::test]
async fn a_supersession_survives_a_restart_under_persistence_with_the_newer_record_gone() {
    let dir = fresh("restart");
    let port = alloc_port();
    let (k1, k2) = {
        let a = started(config(&dir, port, true), port).await;
        let (k1_der, k1) = issue(&dir, "forecaster", "k1");
        a.publish_client_cert(&k1_der).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (k2_der, k2) = issue(&dir, "forecaster", "k2");
        a.publish_client_cert(&k2_der).unwrap();
        assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded);
        let _ = super::helpers::kv_delete(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &k2).as_str()));
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.shutdown().await;
        (k1, k2)
    };
    let journal = dir.join("persist").join("identity").join("client-certs.journal");
    assert!(journal.exists(), "the supersession was journaled");

    let b = started(config(&dir, port, true), port).await;
    assert_eq!(b.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded,
        "RESTORED: after a restart with K2's record gone, the superseded K1 read as current");
    // The mark is a verified issuance, so K2 stays current with its record gone: a member's tombstone retires nothing.
    assert_eq!(b.client_key_standing(PRINCIPAL, &k2), ClientKeyStanding::Current, "a tombstone of the newest record does not retire its key");
    b.shutdown().await;

    std::fs::remove_file(&journal).unwrap();
    let c = started(config(&dir, port, true), port).await;
    let without = c.client_key_standing(PRINCIPAL, &k1);
    c.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(without, ClientKeyStanding::Current, "plant: without the journal the older key is back — the journal is what holds it");
}

/// **The door is `identity:write`, and publishes what it judged.** `POST /gateway/identity/client-cert` refuses a
/// token without the scope (403), refuses a node certificate (422 `not_a_client_certificate`), publishes a client
/// certificate (200, naming its principal) and seals the publication into the audit stream as a governance write.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn the_client_cert_door_is_identity_write_and_publishes_what_it_judged() {
    use axum::http::header::AUTHORIZATION;
    let dir = fresh("door");
    let port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = config(&dir, port, false);
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "ops".into(), scopes: vec!["identity:write".into()] },
        crate::GatewayToken { token: "reader".into(), scopes: vec!["kv:read".into()] },
    ];
    let a = started(cfg, port).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (cert_path, _) = crate::issue_client_cert(&dir.join("certs"), "fleet", "forecaster", &dir.join("door")).unwrap();
    let pem = std::fs::read_to_string(&cert_path).unwrap();
    let node_pem = {
        use base64::{engine::general_purpose::STANDARD, Engine};
        format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", STANDARD.encode(a.task_ctx.tls.get().unwrap().node_cert_der()))
    };
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_port}/gateway/identity/client-cert");
    let post = |bearer: &'static str, pem: String| client.post(&url).header(AUTHORIZATION, format!("Bearer {bearer}")).json(&serde_json::json!({"cert_pem": pem})).send();

    assert_eq!(post("reader", pem.clone()).await.unwrap().status(), 403, "a token without identity:write");
    let r = post("ops", node_pem).await.unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["reason"], "not_a_client_certificate");
    let r = post("ops", pem).await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["principal"], PRINCIPAL);
    let key = crate::client_identity(&rustls::pki_types::CertificateDer::from_pem_file(&cert_path).unwrap()).unwrap().key;
    assert_eq!(a.client_key_standing(PRINCIPAL, &key), ClientKeyStanding::Current);
    let id = a.node_id().clone();
    assert!(a.audit_stream(&id).iter().any(|r| r.record.target == "identity/client-cert"), "the publication is sealed");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A member's junk records cannot keep a newer certificate from superseding.** Seventy records under the
/// principal, with paths sorted ahead of K2's and bodies that are no certificate, then K2's genuine record: K1 is
/// superseded. Seen failing first with the fold judging only the first 64 records of a principal: K2 was never
/// judged and K1 stayed current.
#[tokio::test]
async fn junk_records_sorted_ahead_cannot_hide_a_newer_certificate() {
    let dir = fresh("junk");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let (k1_der, k1) = issue(&dir, "forecaster", "k1");
    a.publish_client_cert(&k1_der).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, k2) = issue(&dir, "forecaster", "k2");
    for i in 0..70u8 {
        let mut junk_key = [0u8; 32];
        junk_key[31] = i;
        let _ = super::helpers::kv_set(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &junk_key).as_str()), bytes::Bytes::from_static(b"junk"));
    }
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &k2).as_str()), bytes::Bytes::from(k2_der));
    assert!(k2[..31] != [0u8; 31], "K2's path sorts after the junk");
    let k1_standing = a.client_key_standing(PRINCIPAL, &k1);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(k1_standing, ClientKeyStanding::Superseded, "HIDDEN: junk records kept K2 from superseding K1");
}
