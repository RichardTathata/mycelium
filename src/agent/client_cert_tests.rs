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
        // K2's record written by another member (not through this node's door), so this node does not republish it
        // and its tombstone stands: what holds K1 superseded across the restart is the journal's mark alone.
        let (k2_der, k2) = issue(&dir, "forecaster", "k2");
        let k2_path: Arc<str> = Arc::from(client_cert_key(PRINCIPAL, &k2).as_str());
        let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&k2_path), bytes::Bytes::from(k2_der));
        assert_eq!(a.client_key_standing(PRINCIPAL, &k1), ClientKeyStanding::Superseded);
        let _ = super::helpers::kv_delete(&a.task_ctx, k2_path);
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

/// A client certificate for `token:fleet/{name}` signed by the CA in `dir/certs`, with the serial `serial` as given —
/// the shape general PKI tooling produces (`openssl` writes a random 20-byte serial), not `mycelium tls issue`'s.
fn fleet_cert_with_serial(dir: &Path, name: &str, key_seed: u8, serial: Vec<u8>) -> (Vec<u8>, [u8; 32]) {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let ca_pem = std::fs::read_to_string(dir.join("certs").join("ca-key.pem")).unwrap();
    let ca_key = rcgen::KeyPair::from_pem(&ca_pem).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let sk = ed25519_dalek::SigningKey::from_bytes(&[key_seed; 32]);
    let kp = rcgen::KeyPair::try_from(sk.to_pkcs8_der().unwrap().as_bytes()).unwrap();
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.subject_alt_names = vec![rcgen::SanType::URI(rcgen::Ia5String::try_from(crate::client_uri("fleet", name)).unwrap())];
    p.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    p.not_before = rcgen::date_time_ymd(2024, 1, 1);
    p.not_after = rcgen::date_time_ymd(2099, 1, 1);
    (p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec(), sk.verifying_key().to_bytes())
}

/// **A client certificate whose serial carries no readable issuance is refused by name, at the door and as a
/// record** (#741's review, finding 2). A 20-byte serial — what `openssl` writes — reads as no issuance, and was
/// taken as issuance 0: a re-issue then tied with its predecessor and broke on key hex, so half the time the newer
/// certificate was refused `superseded`. The plant: the same key under an 8-byte serial is accepted. Seen failing
/// first: the long-serial certificate was published, and its record read as current.
#[tokio::test]
async fn a_client_certificate_with_no_readable_issuance_is_refused_by_name() {
    let dir = fresh("no-issuance");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let mut long = vec![0x5a; 20];
    long[0] = 0x3c;
    let (long_der, long_key) = fleet_cert_with_serial(&dir, "forecaster", 51, long);
    assert!(crate::client_identity(&long_der).is_some_and(|id| id.issued_ms.is_none()), "a 20-byte serial reads as no issuance");
    let door = a.publish_client_cert(&long_der);
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &long_key).as_str()), bytes::Bytes::from(long_der));
    let as_record = a.client_key_standing(PRINCIPAL, &long_key);
    let (ok_der, ok_key) = fleet_cert_with_serial(&dir, "other", 52, 1_700_000_000_000u64.to_be_bytes().to_vec());
    let plant = a.publish_client_cert(&ok_der).map(|p| p.key);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(door.err(), Some(PublishRefusal::NoReadableIssuance), "ADMITTED at the door: a certificate with no readable issuance");
    assert_eq!(as_record, ClientKeyStanding::Unknown, "ADMITTED as a record: a certificate with no readable issuance read as current");
    assert_eq!(plant, Ok(ok_key), "plant: an 8-byte serial is accepted");
}

/// **A certificate issued in the future is refused at the door and ignored as a record.** Its serial is an hour past
/// this node's clock, beyond the 300 s skew bound: `issued_in_the_future` at the door, unknown as a record, counted.
#[tokio::test]
async fn a_certificate_issued_in_the_future_is_refused_at_the_door_and_ignored_as_a_record() {
    let dir = fresh("future");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let ahead = mycelium_core::sim_seam::wall_now_ms() + 3_600_000;
    let (der, key) = fleet_cert_with_serial(&dir, "forecaster", 53, ahead.to_be_bytes().to_vec());
    let door = a.publish_client_cert(&der);
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &key).as_str()), bytes::Bytes::from(der));
    let before = a.task_ctx.client_certs.future.load(std::sync::atomic::Ordering::Relaxed);
    let standing = a.client_key_standing(PRINCIPAL, &key);
    let counted = a.task_ctx.client_certs.future.load(std::sync::atomic::Ordering::Relaxed) > before;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(door.err(), Some(PublishRefusal::IssuedInTheFuture));
    assert_eq!(standing, ClientKeyStanding::Unknown);
    assert!(counted, "the record's future issuance is counted");
}

/// **Many junk principals cost one store scan per refresh and one chain check per distinct certificate** (#741's
/// review, finding 3). A member writes 150 principals' self-consistent client certificates from a CA that is not the
/// fleet's. `refresh` scans the store once — not once per principal — and a second refresh runs no chain check
/// again, the verdicts cached by the certificate's digest. Seen failing first: one refresh made 151 scans, and the
/// second ran 150 chain checks again.
#[tokio::test]
async fn many_junk_principals_cost_one_scan_and_one_chain_check_per_certificate() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = fresh("junk-principals");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let foreign = fresh("junk-principals-ca");
    crate::mint_ca(&foreign.join("certs")).unwrap();
    for i in 0..150u8 {
        let (der, key) = fleet_cert_with_serial(&foreign, &format!("junk-{i}"), i.wrapping_add(60), vec![1, i]);
        let principal = format!("token:fleet/junk-{i}");
        let _ = super::helpers::kv_set(&a.task_ctx, Arc::from(client_cert_key(&principal, &key).as_str()), bytes::Bytes::from(der));
    }
    // Let the watcher's folds settle, then measure two refreshes of our own.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ledger = &a.task_ctx.client_certs;
    let s0 = ledger.scans.load(Relaxed);
    super::client_cert::refresh(&a.task_ctx);
    let scans = ledger.scans.load(Relaxed) - s0;
    let c1 = ledger.chain_checks.load(Relaxed);
    super::client_cert::refresh(&a.task_ctx);
    let rechecked = ledger.chain_checks.load(Relaxed) - c1;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&foreign);
    assert_eq!(scans, 1, "COST: one refresh scanned the store {scans} times");
    assert_eq!(rechecked, 0, "COST: a second refresh re-ran {rechecked} chain checks");
}

/// **The publishing gateway republishes its record, so a node that joins later reads the newer certificate**
/// (#741's review, finding 4). A publishes K1 then K2 and K2's record is tombstoned; B joins afterwards, with no
/// memory of K2. A republishes K2's record, so B reads it and K1 is superseded at B as well. Seen failing first:
/// nothing republished, B read K1's record and the tombstone, and K1 stayed current at B.
#[tokio::test]
async fn a_node_joining_later_reads_a_tombstoned_newer_record_back() {
    let dir = fresh("republish");
    let (pa, pb) = (alloc_port(), alloc_port());
    let mut ca = config(&dir, pa, false);
    ca.reconnect_backoff_secs = 1;
    let a = started(ca, pa).await;
    let (k1_der, k1) = issue(&dir, "forecaster", "k1");
    a.publish_client_cert(&k1_der).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, k2) = issue(&dir, "forecaster", "k2");
    a.publish_client_cert(&k2_der).unwrap();
    let _ = super::helpers::kv_delete(&a.task_ctx, Arc::from(client_cert_key(PRINCIPAL, &k2).as_str()));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut cb = config(&dir, pb, false);
    cb.bootstrap_peers = vec![NodeId::new("127.0.0.1", pa).unwrap()];
    cb.reconnect_backoff_secs = 1;
    let b = started(cb, pb).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut standing = ClientKeyStanding::Unknown;
    while std::time::Instant::now() < deadline {
        if b.kv().get(&client_cert_key(PRINCIPAL, &k1)).is_some() {
            standing = b.client_key_standing(PRINCIPAL, &k1);
            if standing == ClientKeyStanding::Superseded {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let k2_at_b = b.client_key_standing(PRINCIPAL, &k2);
    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(standing, ClientKeyStanding::Superseded, "ROLLED BACK: a node joining later read K1 as current");
    assert_eq!(k2_at_b, ClientKeyStanding::Current);
}

/// **Publishing the same certificate again does not grow the journal** (#741's confirm round, N1). Fifty identical
/// publishes leave `identity/client-certs.journal` the size the first one left it. Seen failing first: each publish
/// appended a `Published` entry outside the journal's budget.
#[tokio::test]
async fn repeated_identical_publishes_leave_the_journal_bounded() {
    let dir = fresh("journal-bounded");
    let port = alloc_port();
    let a = started(config(&dir, port, true), port).await;
    let journal = dir.join("persist").join("identity").join("client-certs.journal");
    let (der, _) = issue(&dir, "forecaster", "k1");
    a.publish_client_cert(&der).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after_one = std::fs::metadata(&journal).unwrap().len();
    for _ in 0..50 {
        let _ = a.publish_client_cert(&der);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after_fifty = std::fs::metadata(&journal).unwrap().len();
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(after_fifty, after_one, "UNBOUNDED: fifty identical publishes grew the journal from {after_one} to {after_fifty} bytes");
}

/// **Two gateways holding different certificates for one key do not rewrite each other** (#741's confirm round, N2).
/// The CA certified one key twice; this gateway published the earlier certificate, and the later one is written over
/// its record. A valid certificate for the same key with issuance at or above this gateway's own is left alone; an
/// earlier one is still rewritten (the plant). Seen failing first: the later certificate was replaced by this
/// gateway's earlier bytes, which another gateway would replace again, forever.
#[tokio::test]
async fn a_later_certificate_for_the_same_key_is_not_rewritten_by_the_publisher() {
    let dir = fresh("republish-same-key");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let now = mycelium_core::sim_seam::wall_now_ms();
    let (early, key) = fleet_cert_with_serial(&dir, "forecaster", 71, (now - 2_000).to_be_bytes().to_vec());
    let (late, _) = fleet_cert_with_serial(&dir, "forecaster", 71, (now - 1_000).to_be_bytes().to_vec());
    let (earlier, _) = fleet_cert_with_serial(&dir, "forecaster", 71, (now - 3_000).to_be_bytes().to_vec());
    a.publish_client_cert(&early).unwrap();
    let path: Arc<str> = Arc::from(client_cert_key(PRINCIPAL, &key).as_str());
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&path), bytes::Bytes::from(late.clone()));
    let left = super::client_cert::republish_own(&a.task_ctx);
    let kept = a.kv().get(&path).map(|b| b.to_vec());
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&path), bytes::Bytes::from(earlier));
    let rewritten = super::client_cert::republish_own(&a.task_ctx);
    let restored = a.kv().get(&path).map(|b| b.to_vec());
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((left, kept), (0, Some(late)), "FIGHT: the publisher rewrote a later certificate for the same key");
    assert_eq!((rewritten, restored), (1, Some(early)), "plant: an earlier certificate for the key is rewritten");
}

/// **On load the journal keeps the newest published record per principal, whatever order the appends landed in**
/// (#741's confirm round, N3). Appends are spawned, so K2's `Published` entry can land before K1's: after a restart
/// the gateway republishes K2, not K1. Seen failing first: the last entry won, and K1 was the record it held.
#[tokio::test]
async fn the_journal_keeps_the_newest_published_record_whatever_the_append_order() {
    let dir = fresh("journal-order");
    let port = alloc_port();
    // The fleet CA minted before the node's first start, so the certificates below are the node's CA's.
    crate::mint_ca(&dir.join("certs")).unwrap();
    let (k1_der, k1) = issue(&dir, "forecaster", "k1");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (k2_der, k2) = issue(&dir, "forecaster", "k2");
    {
        let identity = dir.join("persist").join("identity");
        std::fs::create_dir_all(&identity).unwrap();
        let j = super::journal::Journal::open(identity.join("client-certs.journal"), "test").unwrap();
        j.append(super::client_cert::published_entry_for_test(PRINCIPAL, &client_cert_key(PRINCIPAL, &k2), &k2_der)).await.unwrap();
        j.append(super::client_cert::published_entry_for_test(PRINCIPAL, &client_cert_key(PRINCIPAL, &k1), &k1_der)).await.unwrap();
    }
    let a = started(config(&dir, port, true), port).await;
    let held = a.task_ctx.client_certs.published_path_for_test(PRINCIPAL);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(held, Some(client_cert_key(PRINCIPAL, &k2)), "OUT OF ORDER: the journal's last entry, not its newest, was kept (k1 = {})", client_cert_key(PRINCIPAL, &k1));
}

/// **A far-future certificate for the same key does not stop the publisher restoring its record** (#741's final
/// confirm). A CA holder writes a same-key certificate issued an hour ahead at the record's path. The skip test is
/// the fold's judgment, skew bound included, so the far-future certificate does not count and the publisher's own is
/// written back. Seen failing first: the skip test read only the chain and the names, and left the planted record.
#[tokio::test]
async fn a_far_future_certificate_for_the_same_key_is_rewritten_by_the_publisher() {
    let dir = fresh("republish-future");
    let port = alloc_port();
    let a = started(config(&dir, port, false), port).await;
    let now = mycelium_core::sim_seam::wall_now_ms();
    let (own, key) = fleet_cert_with_serial(&dir, "forecaster", 72, (now - 1_000).to_be_bytes().to_vec());
    let (ahead, _) = fleet_cert_with_serial(&dir, "forecaster", 72, (now + 3_600_000).to_be_bytes().to_vec());
    a.publish_client_cert(&own).unwrap();
    let path: Arc<str> = Arc::from(client_cert_key(PRINCIPAL, &key).as_str());
    let _ = super::helpers::kv_set(&a.task_ctx, Arc::clone(&path), bytes::Bytes::from(ahead));
    let rewritten = super::client_cert::republish_own(&a.task_ctx);
    let stored = a.kv().get(&path).map(|b| b.to_vec());
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((rewritten, stored), (1, Some(own)), "BLOCKED: a far-future same-key certificate kept the publisher from restoring its record");
}
