//! Identity-and-lifecycle I2, PR 1: the reload path on live agents, and the certificate caches re-judged when
//! the roots change (the roots generation, R4). The record-driven phases, `succession/`, the kept anchors and
//! the re-issue are I2's later PRs (the plan's §8 row for this PR states the split).
use super::*;
use crate::test_util::alloc_port;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("myc-i2-agent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn der(p: &Path) -> Vec<u8> {
    CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec()
}

fn handshake(client: Arc<rustls::ClientConfig>, server: Arc<rustls::ServerConfig>) -> Result<(), String> {
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
    let mut c = rustls::ClientConnection::new(client, name).unwrap();
    let mut s = rustls::ServerConnection::new(server).unwrap();
    for _ in 0..16 {
        let mut buf = Vec::new();
        while c.wants_write() { c.write_tls(&mut buf).unwrap(); }
        if !buf.is_empty() {
            s.read_tls(&mut buf.as_slice()).unwrap();
            s.process_new_packets().map_err(|e| format!("server refused: {e}"))?;
        }
        let mut buf = Vec::new();
        while s.wants_write() { s.write_tls(&mut buf).unwrap(); }
        if !buf.is_empty() {
            c.read_tls(&mut buf.as_slice()).unwrap();
            c.process_new_packets().map_err(|e| format!("client refused: {e}"))?;
        }
        if !c.is_handshaking() && !s.is_handshaking() { return Ok(()); }
    }
    Err("did not finish".into())
}

/// Two minted CAs: their directories and certificates' DER.
fn two_cas(base: &Path) -> ((PathBuf, Vec<u8>), (PathBuf, Vec<u8>)) {
    let mint = |n: &str| {
        let dir = base.join(n);
        let cert = crate::tls::mint_ca(&dir).unwrap();
        (dir, der(&cert))
    };
    (mint("ca1"), mint("ca2"))
}

/// A node's configuration on the secure posture: a certificate issued off-node under `ca_dir`, trusting `trust`.
fn node_cfg(base: &Path, name: &str, ca_dir: &Path, trust: &Path, boot: Vec<NodeId>) -> (NodeId, GossipConfig) {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let (cert, key) = crate::tls::issue_node_cert(ca_dir, &id, &base.join(format!("{name}-issued"))).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.bootstrap_peers = boot;
    cfg.health_check_max_jitter_ms = 1;
    cfg.health_check_interval_secs = 2;
    let mut tls = crate::TlsConfig::default();
    tls.auto_cert_dir = base.join(format!("{name}-node"));
    tls.cert_pem = Some(cert);
    tls.key_pem = Some(key);
    tls.ca_cert_pem = Some(trust.to_path_buf());
    cfg.tls = Some(tls);
    (id, cfg)
}

/// A `NodeTls` alone (no agent), on the secure posture.
fn node_tls(base: &Path, name: &str, ca_dir: &Path, trust: &Path) -> (NodeId, Arc<crate::tls::NodeTls>) {
    let (id, cfg) = node_cfg(base, name, ca_dir, trust, vec![]);
    let tls = crate::tls::load_or_generate(cfg.tls.as_ref().unwrap(), &id, Arc::default(), Arc::default()).unwrap();
    (id, Arc::new(tls))
}

async fn until(what: &str, secs: u64, f: impl Fn() -> bool) -> Duration {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    t.elapsed()
}

/// **Exit 18, the caches (R4; #768's M3): a verdict judged under the old roots does not survive a change of
/// roots** — in the revocation fold's `cert_facts` and in I1's client-certificate cache. Each judgment pauses
/// (a plant) between judging and caching, and the node narrows in the pause, from both roots to the successor's
/// alone, and the cache is cleared there too, as a clear-on-`activate` design would. The certificate judged was
/// issued under the old CA: the next judgment refuses it. Seen failing first with the generation check removed
/// from the cache's lookup: the old-roots verdict, inserted after the clear, was served.
#[tokio::test]
async fn an_old_roots_verdict_cached_across_a_change_of_roots_is_re_judged() {
    let base = fresh_dir("cache");
    let ((ca1, der1), (ca2, der2)) = two_cas(&base);
    let (ia, a) = node_tls(&base, "a", &ca1, &ca1.join("ca-cert.pem"));
    a.activate(a.reroot_material(a.node_cert_der(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    let a2 = crate::tls::issue_node_cert_for_key(&ca2, &ia, a.verifying_key_bytes(), 0).unwrap();
    let (_, x) = node_tls(&base, "x", &ca1, &ca1.join("ca-cert.pem"));
    let old_node_cert = x.node_cert_der();
    let (client, _) = crate::tls::issue_client_cert(&ca1, "fleet", "forecaster", &base.join("client")).unwrap();
    let old_client_cert = der(&client);

    let ledger = revocation::IdentityLedger::default();
    let narrow = |t: &crate::tls::NodeTls| t.activate(t.reroot_material(a2.clone(), vec![der2.clone()]).unwrap()).unwrap();
    assert!(revocation::cert_verdict_for_test(&ledger, &a, &old_node_cert, || {
        narrow(&a);
        revocation::clear_cert_cache_for_test(&ledger);
    }), "judged while both roots were trusted");
    assert!(!revocation::cert_verdict_for_test(&ledger, &a, &old_node_cert, || {}), "the fold re-judges it under the successor alone");

    a.activate(a.reroot_material(a2.clone(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    let clients = client_cert::ClientCertLedger::default();
    assert!(client_cert::client_verdict_for_test(&clients, &a, &old_client_cert, || {
        narrow(&a);
        client_cert::clear_cert_cache_for_test(&clients);
    }), "a client certificate judged while both roots were trusted");
    assert!(!client_cert::client_verdict_for_test(&clients, &a, &old_client_cert, || {}), "and re-judged under the successor alone");
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 18, the loop.** One thread changes the roots back and forth while another judges an old-CA certificate
/// through both caches; once the node is left narrowed, neither cache answers with an old-roots verdict.
#[test]
fn judging_concurrently_with_activate_leaves_no_old_roots_verdict() {
    let base = fresh_dir("loop");
    let ((ca1, der1), (ca2, der2)) = two_cas(&base);
    let (ia, a) = node_tls(&base, "a", &ca1, &ca1.join("ca-cert.pem"));
    let a2 = crate::tls::issue_node_cert_for_key(&ca2, &ia, a.verifying_key_bytes(), 0).unwrap();
    let (_, x) = node_tls(&base, "x", &ca1, &ca1.join("ca-cert.pem"));
    let old = x.node_cert_der();
    let (client, _) = crate::tls::issue_client_cert(&ca1, "fleet", "forecaster", &base.join("client")).unwrap();
    let old_client = der(&client);
    let ledger = Arc::new(revocation::IdentityLedger::default());
    let clients = Arc::new(client_cert::ClientCertLedger::default());
    let wide = a.reroot_material(a2.clone(), vec![der1.clone(), der2.clone()]).unwrap();
    a.activate(wide).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let judge = {
        let (a, ledger, clients, stop, old, old_client) = (Arc::clone(&a), Arc::clone(&ledger), Arc::clone(&clients), Arc::clone(&stop), old.clone(), old_client.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = revocation::cert_verdict_for_test(&ledger, &a, &old, || {});
                let _ = client_cert::client_verdict_for_test(&clients, &a, &old_client, || {});
            }
        })
    };
    for i in 0..200 {
        let roots = if i % 2 == 0 { vec![der2.clone()] } else { vec![der1.clone(), der2.clone()] };
        a.activate(a.reroot_material(a2.clone(), roots).unwrap()).unwrap();
    }
    a.activate(a.reroot_material(a2.clone(), vec![der2.clone()]).unwrap()).unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    judge.join().unwrap();
    assert!(!revocation::cert_verdict_for_test(&ledger, &a, &old, || {}), "no old-roots verdict in cert_facts");
    assert!(!client_cert::client_verdict_for_test(&clients, &a, &old_client, || {}), "no old-roots verdict in the client cache");
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 11, the bulk fetch.** The bulk fetch's TLS client is built once, at `start()`, over
/// `fleet_server_verifier()`. Built before a change of roots, it still judges by the roots the node trusts at each
/// fetch: a peer's gateway presenting a successor certificate is refused before the node widens and accepted
/// after, with the same client. Seen failing first with `fleet_server_verifier()` handing out the check of the
/// moment rather than the delegating one: the client built at start kept refusing.
#[cfg(feature = "gateway")]
#[test]
fn the_bulk_fetch_client_built_at_start_follows_a_change_of_roots() {
    let base = fresh_dir("bulk");
    let ((ca1, der1), (ca2, der2)) = two_cas(&base);
    let (_, a) = node_tls(&base, "a", &ca1, &ca1.join("ca-cert.pem"));
    let (ib, b) = node_tls(&base, "b", &ca1, &ca1.join("ca-cert.pem"));
    let client = Arc::new(bulk::bulk_tls_client_config(Some(a.fleet_server_verifier()), None).unwrap());
    let b2 = crate::tls::issue_node_cert_for_key(&ca2, &ib, b.verifying_key_bytes(), 0).unwrap();
    b.activate(b.reroot_material(b2, vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    let before = handshake(Arc::clone(&client), b.gateway_server_config());
    assert!(before.is_err(), "A trusts CA1 alone: B's successor certificate is refused: {before:?}");
    a.activate(a.reroot_material(a.node_cert_der(), vec![der1, der2]).unwrap()).unwrap();
    assert_eq!(handshake(client, b.gateway_server_config()), Ok(()), "the same client, after A widens");
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 1, the reload path on live agents, no restart.** Two live nodes on `cert_pem` with no `ca-key.pem`
/// widen, cut over and narrow through `activate`, with a KV write crossing the mesh in each phase. At each phase
/// a handshake is shown each way on the live configs. After narrowing, a fresh node trusting **only** the
/// successor receives A's write — A presented its successor certificate on a socket — and a node that trusts
/// both roots but holds an old-root certificate is refused by A both ways, and its write never reaches A.
/// What drives the phases here is the test; the successor-signed record that drives them in a fleet is I2's
/// next PR.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_cert_pem_nodes_widen_cut_over_and_narrow_without_a_restart() {
    let base = fresh_dir("live");
    let ((ca1, der1), (ca2, der2)) = two_cas(&base);
    let (r1, r2) = (ca1.join("ca-cert.pem"), ca2.join("ca-cert.pem"));
    let (ia, ca) = node_cfg(&base, "a", &ca1, &r1, vec![]);
    let (ib, cb) = node_cfg(&base, "b", &ca1, &r1, vec![ia.clone()]);
    let a = Arc::new(GossipAgent::new(ia.clone(), ca.clone()));
    a.start().await.unwrap();
    let b = Arc::new(GossipAgent::new(ib.clone(), cb.clone()));
    b.start().await.unwrap();
    for c in [&ca, &cb] {
        assert!(!c.tls.as_ref().unwrap().auto_cert_dir.join("ca-key.pem").exists(), "the secure posture: no CA key");
    }
    let (ta, tb) = (Arc::clone(a.task_ctx.tls.get().unwrap()), Arc::clone(b.task_ctx.tls.get().unwrap()));
    let write = |from: &GossipAgent, key: &str| { let _ = from.kv().set(key.to_string(), bytes::Bytes::from_static(b"1")); };
    write(&b, "i2/before");
    until("B's write reaches A on CA1", 30, || a.kv().get("i2/before").is_some()).await;

    // Widen: both trust CA1 and CA2, keeping their CA1 certificates.
    for t in [&ta, &tb] {
        t.activate(t.reroot_material(t.node_cert_der(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    }
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "widened: B → A");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "widened: A → B");
    write(&b, "i2/widened");
    until("a write crosses while widened", 15, || a.kv().get("i2/widened").is_some()).await;

    // Cut over: each presents its current key re-issued under CA2.
    let a2 = crate::tls::issue_node_cert_for_key(&ca2, &ia, ta.verifying_key_bytes(), 0).unwrap();
    let b2 = crate::tls::issue_node_cert_for_key(&ca2, &ib, tb.verifying_key_bytes(), 0).unwrap();
    tb.activate(tb.reroot_material(b2.clone(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    ta.activate(ta.reroot_material(a2.clone(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "cut over: B → A");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "cut over: A → B");
    write(&b, "i2/cut-over");
    until("a write crosses once cut over", 15, || a.kv().get("i2/cut-over").is_some()).await;

    // Narrow: CA2 alone.
    tb.activate(tb.reroot_material(b2, vec![der2.clone()]).unwrap()).unwrap();
    ta.activate(ta.reroot_material(a2, vec![der2.clone()]).unwrap()).unwrap();
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "narrowed: B → A");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "narrowed: A → B");
    write(&b, "i2/narrowed");
    until("a write crosses once narrowed", 15, || a.kv().get("i2/narrowed").is_some()).await;

    // A fresh node trusting only CA2 receives A's write: A presented its successor certificate on a socket.
    let (ig, cg) = node_cfg(&base, "g", &ca2, &r2, vec![ia.clone()]);
    let g = Arc::new(GossipAgent::new(ig, cg));
    g.start().await.unwrap();
    write(&a, "i2/from-a");
    until("A's write reaches a CA2-only node", 30, || g.kv().get("i2/from-a").is_some()).await;

    // F trusts both roots but holds a CA1 certificate: refused by narrowed A on both sides (spike M2).
    let (if_, cf) = node_cfg(&base, "f", &ca1, &r1, vec![ia.clone()]);
    let f = Arc::new(GossipAgent::new(if_, cf));
    f.start().await.unwrap();
    let tf = Arc::clone(f.task_ctx.tls.get().unwrap());
    tf.activate(tf.reroot_material(tf.node_cert_der(), vec![der1, der2]).unwrap()).unwrap();
    let f_to_a = handshake(tf.client_config(), ta.server_config());
    assert!(f_to_a.as_ref().is_err_and(|e| e.starts_with("server refused")), "A's accept side: {f_to_a:?}");
    let a_to_f = handshake(ta.client_config(), tf.server_config());
    assert!(a_to_f.as_ref().is_err_and(|e| e.starts_with("client refused")), "A's dial side: {a_to_f:?}");
    write(&f, "i2/from-f");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(a.kv().get("i2/from-f").is_none(), "F's write never reaches A");

    for n in [&a, &b, &g, &f] { n.shutdown().await; }
    let _ = std::fs::remove_dir_all(&base);
}
