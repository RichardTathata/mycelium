//! Identity-and-lifecycle I2, PR 2b: the CA's commitment on a live node — pinned from the operator's file only and
//! persisted, refused without `[persistence]`, published for detection, and a KV copy compared with the pin and never
//! adopted (the forged-record tests of `sys/ca-commit/`).
use super::*;
use crate::test_util::alloc_port;
use crate::tls::{CaCommitment, CA_COMMIT_FILE, CA_COMMIT_PIN_PREFIX, SUCCESSION_DIR};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("myc-i2-commit-agent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn der(p: &Path) -> Vec<u8> {
    CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec()
}

fn ca_key(dir: &Path) -> ed25519_dalek::SigningKey {
    use base64::Engine;
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let pem = std::fs::read_to_string(dir.join("ca-key.pem")).unwrap();
    let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    ed25519_dalek::SigningKey::from_pkcs8_der(&base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap()
}

/// A fleet CA in `base/ca1` committed to a successor in `base/ca2`: the old CA's directory and its commitment.
fn committed(base: &Path) -> (PathBuf, CaCommitment) {
    let d1 = base.join("ca1");
    crate::tls::mint_ca(&d1).unwrap();
    let (file, _) = crate::tls::commit_successor(&d1, &base.join("ca2")).unwrap();
    (d1, CaCommitment::from_pem(&std::fs::read_to_string(file).unwrap()).unwrap())
}

/// A second commitment the old CA's key signed, naming another successor: what a holder of a stolen CA key makes.
fn rival(base: &Path, d1: &Path, tag: &str) -> CaCommitment {
    let other = der(&crate::tls::mint_ca_at(&base.join(tag), None, 1).unwrap());
    CaCommitment::sign(&der(&d1.join("ca-cert.pem")), &ca_key(d1), &other).unwrap()
}

/// A node on the secure posture under the CA in `d1`, with or without `[persistence]`.
fn node(base: &Path, name: &str, d1: &Path, persist: bool) -> (NodeId, GossipConfig) {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let (cert, key) = crate::tls::issue_node_cert(d1, &id, &base.join(format!("{name}-issued"))).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let mut tls = crate::TlsConfig::default();
    tls.auto_cert_dir = base.join(format!("{name}-node"));
    std::fs::create_dir_all(&tls.auto_cert_dir).unwrap();
    tls.cert_pem = Some(cert);
    tls.key_pem = Some(key);
    tls.ca_cert_pem = Some(d1.join("ca-cert.pem"));
    cfg.tls = Some(tls);
    if persist {
        cfg.persistence = Some(crate::config::PersistenceConfig {
            base_path: base.join(format!("{name}-persist")),
            sync_mode: crate::config::SyncMode::Flush,
            snapshot_wal_threshold: 1_000,
            snapshot_interval_secs: 3_600,
            on_unreadable: crate::config::OnUnreadable::Refuse,
        });
    }
    (id, cfg)
}

/// The commitments a node holds pins for.
fn held(a: &GossipAgent) -> Vec<CaCommitment> {
    a.task_ctx.ca_commitment.pinned().iter().map(|c| (**c).clone()).collect()
}

fn cert_dir(cfg: &GossipConfig) -> PathBuf {
    cfg.tls.as_ref().unwrap().auto_cert_dir.clone()
}

async fn started(id: &NodeId, cfg: &GossipConfig) -> Arc<GossipAgent> {
    let a = Arc::new(GossipAgent::new(id.clone(), cfg.clone()));
    a.start().await.unwrap();
    a
}

fn put(ctx: &TaskCtx, path: &str, bytes: &[u8]) {
    let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path), bytes::Bytes::copy_from_slice(bytes));
}

/// **#805's review F1: a node holding a commitment and no `[persistence]` refuses to start, by name** — with the
/// operator's file, and with only a pin an earlier start left. Nothing is pinned by the refused start. Seen failing
/// first with the check removed: the node started and pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_holding_a_commitment_without_persistence_refuses_to_start_by_name() {
    let base = fresh_dir("nopersist");
    let (d1, _) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, false);
    std::fs::copy(d1.join(CA_COMMIT_FILE), cert_dir(&cfg).join(CA_COMMIT_FILE)).unwrap();
    let err = GossipAgent::new(id.clone(), cfg.clone()).start().await.expect_err("no [persistence]").to_string();
    assert!(err.contains("[persistence]") && err.contains("ca-commit.pem"), "{err}");
    assert!(!cert_dir(&cfg).join(SUCCESSION_DIR).exists(), "nothing pinned");

    // Only the pin, as a start under `[persistence]` leaves it, and `[persistence]` then taken away.
    let (id2, mut cfg2) = node(&base, "b", &d1, true);
    std::fs::copy(d1.join(CA_COMMIT_FILE), cert_dir(&cfg2).join(CA_COMMIT_FILE)).unwrap();
    started(&id2, &cfg2).await.shutdown().await;
    std::fs::remove_file(cert_dir(&cfg2).join(CA_COMMIT_FILE)).unwrap();
    cfg2.persistence = None;
    let err = GossipAgent::new(id2, cfg2).start().await.expect_err("a pin and no [persistence]").to_string();
    assert!(err.contains("[persistence]") && err.contains(CA_COMMIT_PIN_PREFIX), "{err}");
    let _ = std::fs::remove_dir_all(&base);
}

/// **The decisions of 2026-11-15 (a) and 2026-11-16, on a live node: the pin comes from the operator's file, is
/// persisted, and a conflicting commitment in that file at a later start is refused and counted.** The node pins,
/// publishes its commitment at `sys/ca-commit/{self}` (the copy for detection), and reports `id.ca_succession_committed`'s
/// fact. Restarted with the operator's file gone, it still holds the pin. Restarted with a second commitment the old
/// CA's key signed in that file, it keeps the first, counts one conflict, and still publishes the first. Its own copy,
/// overwritten by another member, is republished. Seen failing first with the commitment not published (no KV copy)
/// and, in a second build, not registered as the node's own record (the overwritten copy stayed overwritten).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_pins_from_the_operators_file_publishes_it_and_keeps_the_pin_across_restarts() {
    let base = fresh_dir("pin");
    let (d1, c) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, true);
    let file = cert_dir(&cfg).join(CA_COMMIT_FILE);
    std::fs::copy(d1.join(CA_COMMIT_FILE), &file).unwrap();
    let a = started(&id, &cfg).await;
    let state = &a.task_ctx.ca_commitment;
    assert_eq!(held(&a), vec![c.clone()]);
    assert_eq!(state.pinned_for(&c.predecessor).as_deref(), Some(&c));
    assert!(state.committed());
    let path = ca_commit::ca_commit_key(&id, &c.predecessor);
    assert_eq!(a.kv().get(&path).as_deref(), Some(c.to_pem().as_bytes()), "published for detection");
    put(&a.task_ctx, &path, b"overwritten by another member");
    revocation::republish_own(&a.task_ctx, &a.task_ctx.identity_ledger);
    assert_eq!(a.kv().get(&path).as_deref(), Some(c.to_pem().as_bytes()), "its own copy republished");
    a.shutdown().await;

    std::fs::remove_file(&file).unwrap();
    let a = started(&id, &cfg).await;
    assert_eq!(held(&a), vec![c.clone()], "the pin is the node's memory");
    assert_eq!(a.task_ctx.ca_commitment.conflicts.load(Ordering::Relaxed), 0);
    a.shutdown().await;

    let second = rival(&base, &d1, "rival");
    std::fs::write(&file, second.to_pem()).unwrap();
    let a = started(&id, &cfg).await;
    assert_eq!(held(&a), vec![c.clone()], "the pin stands");
    assert_eq!(a.task_ctx.ca_commitment.conflicts.load(Ordering::Relaxed), 1, "the second commitment refused and counted");
    assert_eq!(a.kv().get(&path).as_deref(), Some(c.to_pem().as_bytes()));
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **The forged-record tests of `sys/ca-commit/` (the decision of 2026-11-16): the KV carries commitments for
/// detection only.** On a node that pinned C:
/// - a copy **not signed by the old CA** (an attacker's key, naming the old CA) is read as absent and counted once
///   (`ca_commitment_unverified`), however often it is read; so is one that is not a commitment at all;
/// - a copy **the old CA's key did sign**, naming another successor — a stolen key's — is refused and counted once
///   (`ca_commitment_conflict`);
/// - C itself under another node's path is nothing;
/// - and through all of it the pin is C.
///
/// On a node with **no pin**, a valid copy of C in the KV is read and **never pinned**: no pin in memory, no
/// `succession/` on disk, and none after a restart — absence fails closed. Seen failing first, one build each: with a
/// verified copy adopted when the node has no pin (the node pinned from the KV); with the copy's signature not
/// assessed (the forged copy counted as a conflict); with the comparison against the pin removed (no conflict counted).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kv_copy_of_a_commitment_is_compared_with_the_pin_and_never_becomes_one() {
    let base = fresh_dir("kv");
    let (d1, c) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, true);
    std::fs::copy(d1.join(CA_COMMIT_FILE), cert_dir(&cfg).join(CA_COMMIT_FILE)).unwrap();
    let a = started(&id, &cfg).await;
    let state = Arc::clone(&a.task_ctx.ca_commitment);
    let counts = || (state.unverified.load(Ordering::Relaxed), state.conflicts.load(Ordering::Relaxed));
    let at = |n: u16| format!("{}127.0.0.1:{n}", ca_commit::CA_COMMIT_PREFIX);
    ca_commit::scan(&a.task_ctx);
    assert_eq!(counts(), (0, 0), "its own copy is the pin");

    let stolen = rival(&base, &d1, "rival");
    let attacker_dir = base.join("attacker");
    let attacker_ca = der(&crate::tls::mint_ca_at(&attacker_dir, None, 1).unwrap());
    let forged = CaCommitment::sign(&der(&d1.join("ca-cert.pem")), &ca_key(&attacker_dir), &attacker_ca);
    assert!(forged.is_err(), "the signer refuses a key that is not the certificate's");
    // Forged by hand: the stolen-key commitment's body under a signature the old CA did not make.
    let mut forged = stolen.clone();
    forged.sig[0] ^= 1;
    put(&a.task_ctx, &at(1), forged.to_pem().as_bytes());
    for _ in 0..3 {
        ca_commit::scan(&a.task_ctx);
    }
    assert_eq!(counts(), (1, 0), "not signed by the old CA: absent, counted once");
    put(&a.task_ctx, &at(2), b"not a commitment");
    ca_commit::scan(&a.task_ctx);
    assert_eq!(counts(), (2, 0));

    put(&a.task_ctx, &at(3), stolen.to_pem().as_bytes());
    for _ in 0..3 {
        ca_commit::scan(&a.task_ctx);
    }
    assert_eq!(counts(), (2, 1), "signed by the old CA's key, another successor: refused, counted once");
    put(&a.task_ctx, &at(4), c.to_pem().as_bytes());
    ca_commit::scan(&a.task_ctx);
    assert_eq!(counts(), (2, 1), "the pinned commitment under another node's path is nothing");
    assert_eq!(held(&a), vec![c.clone()], "the pin is unchanged");
    a.shutdown().await;

    // A node with no pin: the KV's copy is read, and never pinned.
    let (idn, cfgn) = node(&base, "n", &d1, true);
    let n = started(&idn, &cfgn).await;
    put(&n.task_ctx, &at(5), c.to_pem().as_bytes());
    ca_commit::scan(&n.task_ctx);
    assert!(held(&n).is_empty(), "a KV copy never becomes the source of a pin");
    assert!(!n.task_ctx.ca_commitment.committed());
    assert_eq!(n.task_ctx.ca_commitment.conflicts.load(Ordering::Relaxed), 0);
    n.shutdown().await;
    assert!(!cert_dir(&cfgn).join(SUCCESSION_DIR).exists(), "nothing persisted");
    let n = started(&idn, &cfgn).await;
    assert!(n.kv().get(&at(5)).is_some(), "the copy is still in the store after the restart");
    ca_commit::scan(&n.task_ctx);
    assert!(held(&n).is_empty(), "nor after a restart");
    n.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#812's review F3 (a) and its confirm round's C1, on a live node: beside a pin, an operator's file is never a
/// stolen-key alarm unless a CA's key signed it, and only an early file lets the start go on.** A node with a pin:
/// - restarted with a `ca-commit.pem` carrying a garbage signature, **refuses to start** naming the file — and no
///   conflict is counted by the start that follows;
/// - restarted with the successor's own commitment, placed **before it trusts the successor**, starts: nothing is
///   pinned, its pin stands, and the file has its own signal (`ca_commitment_file_untrusted_ca`), apart from the KV's
///   unverified copies and from conflicts.
///
/// Seen failing first, one build each: with every unassessable file beside a pin passed over (the node started on the
/// garbage signature); and with the early file counted among the KV's unverified copies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn beside_a_pin_a_file_no_ca_signed_refuses_the_start_and_an_early_one_has_its_own_signal() {
    let base = fresh_dir("unsigned-file");
    let (d1, c) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, true);
    let file = cert_dir(&cfg).join(CA_COMMIT_FILE);
    std::fs::copy(d1.join(CA_COMMIT_FILE), &file).unwrap();
    started(&id, &cfg).await.shutdown().await;
    let mut forged = rival(&base, &d1, "rival");
    forged.sig = [7u8; 64];
    std::fs::write(&file, forged.to_pem()).unwrap();
    let err = GossipAgent::new(id.clone(), cfg.clone()).start().await.expect_err("no CA signed it").to_string();
    assert!(err.contains("ca-commit.pem") && err.contains("did not sign"), "{err}");

    let (file2, _) = crate::tls::commit_successor(&base.join("ca2"), &base.join("ca3")).unwrap();
    std::fs::copy(&file2, &file).unwrap();
    let a = started(&id, &cfg).await;
    let state = &a.task_ctx.ca_commitment;
    assert_eq!(held(&a), vec![c.clone()], "the pin stands, and the early commitment is not pinned");
    assert_eq!(state.file_untrusted_ca.load(Ordering::Relaxed), 1, "the operator's file has its own signal");
    assert_eq!((state.unverified.load(Ordering::Relaxed), state.conflicts.load(Ordering::Relaxed)), (0, 0));
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// A node `id` restarted on the successor's files: a certificate for it under the CA in `ca_dir`, trusting that CA
/// alone, over the same certificate directory and persistence.
fn on_successor(base: &Path, id: &NodeId, cfg: &GossipConfig, ca_dir: &Path, tag: &str) -> GossipConfig {
    let (cert, key) = crate::tls::issue_node_cert(ca_dir, id, &base.join(format!("{tag}-issued"))).unwrap();
    let mut cfg = cfg.clone();
    let tls = cfg.tls.as_mut().unwrap();
    tls.cert_pem = Some(cert);
    tls.key_pem = Some(key);
    tls.ca_cert_pem = Some(ca_dir.join("ca-cert.pem"));
    cfg
}

/// **The decision of 2026-11-18, on a live node: a pin is per committing CA.** A node pins CA0's commitment to CA1.
/// Restarted on CA1's files — it now trusts CA1 — with CA1's own commitment to CA2 in its operator's file, it **pins
/// that too**: two pins, no conflict counted, the CA0 pin kept (spent) and only the CA1 pin, whose CA it trusts,
/// published. Restarted again it holds both. Restarted with a **second commitment CA1's key signed**, it keeps both
/// pins and counts one conflict. Seen failing first with one pin for the node: CA1's commitment was not pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_pins_one_commitment_per_committing_ca_and_keeps_both_across_restarts() {
    let base = fresh_dir("per-ca");
    let (d1, first) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, true);
    let file = cert_dir(&cfg).join(CA_COMMIT_FILE);
    std::fs::copy(d1.join(CA_COMMIT_FILE), &file).unwrap();
    started(&id, &cfg).await.shutdown().await;

    let d2 = base.join("ca2");
    let (file2, _) = crate::tls::commit_successor(&d2, &base.join("ca3")).unwrap();
    let second = CaCommitment::from_pem(&std::fs::read_to_string(&file2).unwrap()).unwrap();
    let cfg1 = on_successor(&base, &id, &cfg, &d2, "a-ca1");
    std::fs::copy(&file2, &file).unwrap();
    let a = started(&id, &cfg1).await;
    let mut got = held(&a);
    got.sort_by_key(|c| c.digest());
    let mut want = vec![first.clone(), second.clone()];
    want.sort_by_key(|c| c.digest());
    assert_eq!(got, want, "one pin per committing CA");
    let state = &a.task_ctx.ca_commitment;
    assert_eq!((state.unverified.load(Ordering::Relaxed), state.conflicts.load(Ordering::Relaxed)), (0, 0), "a different promiser: not a conflict");
    assert_eq!(state.pinned_for(&second.predecessor).as_deref(), Some(&second));
    assert_eq!(a.kv().get(&ca_commit::ca_commit_key(&id, &second.predecessor)).as_deref(), Some(second.to_pem().as_bytes()), "the pin of the CA it trusts is published");
    a.shutdown().await;

    let a = started(&id, &cfg1).await;
    assert_eq!(held(&a).len(), 2, "both pins read back");
    a.shutdown().await;

    let rival = der(&crate::tls::mint_ca_at(&base.join("rival2"), None, 2).unwrap());
    let stolen = CaCommitment::sign(&der(&d2.join("ca-cert.pem")), &ca_key(&d2), &rival).unwrap();
    std::fs::write(&file, stolen.to_pem()).unwrap();
    let a = started(&id, &cfg1).await;
    assert_eq!(held(&a).len(), 2, "nothing more pinned");
    assert_eq!(a.task_ctx.ca_commitment.pinned_for(&second.predecessor).as_deref(), Some(&second), "CA1's pin stands");
    assert_eq!(a.task_ctx.ca_commitment.conflicts.load(Ordering::Relaxed), 1, "the same CA's second commitment: refused and counted");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **The decision of 2026-11-18, in the KV: a copy is compared with the pin held for its own CA, and never pins.** A
/// node holds a spent pin for CA0 and trusts only CA1; it holds **no pin for CA1**. In the KV:
/// - a **forged** copy naming CA1 as its signer (CA1's key did not sign it) is read as absent, counted unverified,
///   and pins nothing;
/// - CA1's **genuine** commitment to CA2 verifies, contradicts no pin this node holds, and is **not counted** — and
///   it pins nothing either: no pin in memory, no pin file, and none after a restart.
///
/// Once the operator places CA1's commitment and the node pins it, a second commitment CA1's key signed, in the KV,
/// is a conflict. Seen failing first, one build each: with a verified copy from a CA the node holds no pin for
/// pinned from the KV; and with a verified copy compared with every pin instead of its own CA's (the genuine CA1
/// copy counted as a conflict of CA0's pin).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kv_copy_is_compared_with_the_pin_for_its_own_ca_and_never_pins() {
    let base = fresh_dir("kv-per-ca");
    let (d1, first) = committed(&base);
    let (id, cfg) = node(&base, "a", &d1, true);
    let file = cert_dir(&cfg).join(CA_COMMIT_FILE);
    std::fs::copy(d1.join(CA_COMMIT_FILE), &file).unwrap();
    started(&id, &cfg).await.shutdown().await;
    std::fs::remove_file(&file).unwrap();

    let d2 = base.join("ca2");
    let (file2, _) = crate::tls::commit_successor(&d2, &base.join("ca3")).unwrap();
    let second = CaCommitment::from_pem(&std::fs::read_to_string(&file2).unwrap()).unwrap();
    let cfg1 = on_successor(&base, &id, &cfg, &d2, "a-ca1");
    let a = started(&id, &cfg1).await;
    let state = Arc::clone(&a.task_ctx.ca_commitment);
    let counts = || (state.unverified.load(Ordering::Relaxed), state.conflicts.load(Ordering::Relaxed));
    let at = |n: u16| format!("{}127.0.0.1:{n}/{}", ca_commit::CA_COMMIT_PREFIX, "ab".repeat(32));
    assert_eq!(held(&a), vec![first.clone()], "the spent pin for CA0 alone");
    let mut forged = second.clone();
    forged.sig[0] ^= 1;
    put(&a.task_ctx, &at(1), forged.to_pem().as_bytes());
    ca_commit::scan(&a.task_ctx);
    assert_eq!(counts(), (1, 0), "CA1's key did not sign it: absent, counted");
    put(&a.task_ctx, &at(2), second.to_pem().as_bytes());
    ca_commit::scan(&a.task_ctx);
    assert_eq!(counts(), (1, 0), "genuine, from a CA this node holds no pin for: not a conflict");
    assert_eq!(held(&a), vec![first.clone()], "and nothing in the KV pins");
    let pin_files = || std::fs::read_dir(cert_dir(&cfg).join(SUCCESSION_DIR)).unwrap().count();
    assert_eq!(pin_files(), 1, "no pin file written");
    a.shutdown().await;
    let a = started(&id, &cfg1).await;
    ca_commit::scan(&a.task_ctx);
    assert_eq!(held(&a), vec![first.clone()], "nor after a restart, the copies still in the store");
    a.shutdown().await;

    // The operator places CA1's commitment: pinned. A second one CA1's key signed, in the KV, is then a conflict.
    std::fs::copy(&file2, &file).unwrap();
    let a = started(&id, &cfg1).await;
    assert_eq!(held(&a).len(), 2);
    let rival = der(&crate::tls::mint_ca_at(&base.join("rival2"), None, 2).unwrap());
    let stolen = CaCommitment::sign(&der(&d2.join("ca-cert.pem")), &ca_key(&d2), &rival).unwrap();
    put(&a.task_ctx, &at(3), stolen.to_pem().as_bytes());
    for _ in 0..2 {
        ca_commit::scan(&a.task_ctx);
    }
    assert_eq!(a.task_ctx.ca_commitment.conflicts.load(Ordering::Relaxed), 1, "compared with the pin for CA1: refused, counted once");
    assert_eq!(held(&a).len(), 2);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// A node that pinned C, with a hostile prefix: `junk` small values no CA signed, in keys that sort ahead of any
/// node's. Returns the node and its state.
async fn flooded(base: &Path, d1: &Path, junk: u32) -> (Arc<GossipAgent>, Arc<ca_commit::CommitmentState>) {
    let (id, cfg) = node(base, "a", d1, true);
    std::fs::copy(d1.join(CA_COMMIT_FILE), cert_dir(&cfg).join(CA_COMMIT_FILE)).unwrap();
    let a = started(&id, &cfg).await;
    for i in 0..junk {
        put(&a.task_ctx, &format!("{}!{i:05}", ca_commit::CA_COMMIT_PREFIX), format!("junk {i}").as_bytes());
    }
    // The identity watcher scans on its own as the writes land: let it finish (bounded).
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let state = Arc::clone(&a.task_ctx.ca_commitment);
    (a, state)
}

/// **#812's review F4: "counted once" holds past the bound, whatever a member writes under the prefix.** Any admitted
/// member can write any key, so the prefix is hostile: 1,100 distinct junk values and one genuine conflicting
/// commitment. After the first scan, scanning again and again counts nothing more — no unverified copy, and the
/// conflict stays at one — and the unverified count never passes its bound. The reviewer's test, adopted. Seen
/// failing first with the noted set cleared when full: the unverified count passed its bound at the first scans
/// (the reviewer's run of the same plant shows each later scan re-counting every copy and the conflict).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flooded_prefix_is_counted_once_and_never_again() {
    let base = fresh_dir("flood");
    let (d1, _) = committed(&base);
    let (a, state) = flooded(&base, &d1, 1_100).await;
    // Sorted ahead of the junk, so the bounded scan reads it.
    put(&a.task_ctx, &format!("{} conflict", ca_commit::CA_COMMIT_PREFIX), rival(&base, &d1, "rival").to_pem().as_bytes());
    let counts = || (state.unverified.load(Ordering::Relaxed), state.conflicts.load(Ordering::Relaxed));
    // Every copy is read within two scans of 1,024 (the cursor moves on from where the watcher's own scans left it).
    for _ in 0..2 {
        ca_commit::scan(&a.task_ctx);
    }
    let first = counts();
    assert_eq!(first.1, 1, "the genuine conflict is counted");
    assert!(first.0 >= 1 && first.0 <= 1_024, "unverified copies are counted up to the bound: {}", first.0);
    for _ in 0..3 {
        ca_commit::scan(&a.task_ctx);
        assert_eq!(counts(), first, "nothing new in the KV: a scan counts nothing");
    }
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#812's review F4: a scan reads a bounded number of copies and says how many it left.** With 1,100 junk keys
/// beside the node's own copy, a scan reads 1,024 copies and reports the other 77 on `ca_commitment_scan_skipped` — a
/// gauge, the same after every scan. Seen failing first with no bound on the copies a scan reads: it reported 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_reads_a_bounded_number_of_copies_and_reports_the_rest() {
    let base = fresh_dir("bound");
    let (d1, _) = committed(&base);
    let (a, state) = flooded(&base, &d1, 1_100).await;
    for _ in 0..2 {
        ca_commit::scan(&a.task_ctx);
        assert_eq!(state.scan_skipped.load(Ordering::Relaxed), 1_101 - 1_024, "what the scan did not read, not a running total");
    }
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#812's confirm round C3: keys that sort ahead of a genuine conflicting copy delay its reading and never hide
/// it.** 1,024 junk keys sort ahead of every node's path (a node id begins with a digit), and a second commitment the
/// CA's key signed sits at a node's path behind them — 1,026 readable copies with the node's own. Each scan starts
/// where the last stopped, so two consecutive scans read every copy: the conflict is counted within two scans, once.
/// The reviewer's test. Seen failing first with every scan starting at the first key: after five scans the conflict
/// had never been read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_behind_a_flood_of_keys_is_counted_within_a_bounded_number_of_scans() {
    let base = fresh_dir("behind-flood");
    let (d1, _) = committed(&base);
    let (a, state) = flooded(&base, &d1, 1_024).await;
    let stolen = rival(&base, &d1, "rival");
    put(&a.task_ctx, &format!("{}127.0.0.1:7/{}", ca_commit::CA_COMMIT_PREFIX, "ab".repeat(32)), stolen.to_pem().as_bytes());
    for _ in 0..2 {
        ca_commit::scan(&a.task_ctx);
    }
    assert_eq!(state.conflicts.load(Ordering::Relaxed), 1, "read within ceil(1026 / 1024) = 2 scans");
    assert_eq!(state.scan_skipped.load(Ordering::Relaxed), 2, "each scan leaves two copies for the next");
    for _ in 0..3 {
        ca_commit::scan(&a.task_ctx);
    }
    assert_eq!(state.conflicts.load(Ordering::Relaxed), 1, "and counted once");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#812's review F4: a value too large to be a commitment is skipped unread.** One 64 KiB value under the prefix is
/// neither hashed nor parsed: it is not counted as an unverified copy, and the scan reports one copy skipped. Seen
/// failing first with no size bound: it was read and counted as unverified, and nothing was reported skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_too_large_to_be_a_commitment_is_skipped_unread() {
    let base = fresh_dir("oversize");
    let (d1, _) = committed(&base);
    let (a, state) = flooded(&base, &d1, 0).await;
    put(&a.task_ctx, &format!("{}127.0.0.1:9", ca_commit::CA_COMMIT_PREFIX), &vec![b'x'; 64 * 1024]);
    ca_commit::scan(&a.task_ctx);
    assert_eq!(state.unverified.load(Ordering::Relaxed), 0, "not read, so not counted");
    assert_eq!(state.scan_skipped.load(Ordering::Relaxed), 1);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}
