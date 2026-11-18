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

    // Widen: both trust CA1 and CA2, keeping their CA1 certificates — through the agent's one owner (PR 2a), which
    // re-folds and, at the cutover, publishes the certificate each now presents.
    for n in [&a, &b] {
        let t = n.task_ctx.tls.get().unwrap();
        n.change_roots(t.node_cert_der(), vec![der1.clone(), der2.clone()]).await.unwrap();
    }
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "widened: B → A");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "widened: A → B");
    write(&b, "i2/widened");
    until("a write crosses while widened", 15, || a.kv().get("i2/widened").is_some()).await;

    // Cut over: each presents its current key re-issued under CA2.
    let a2 = crate::tls::issue_node_cert_for_key(&ca2, &ia, ta.verifying_key_bytes(), 0).unwrap();
    let b2 = crate::tls::issue_node_cert_for_key(&ca2, &ib, tb.verifying_key_bytes(), 0).unwrap();
    b.change_roots(b2.clone(), vec![der1.clone(), der2.clone()]).await.unwrap();
    a.change_roots(a2.clone(), vec![der1.clone(), der2.clone()]).await.unwrap();
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "cut over: B → A");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "cut over: A → B");
    write(&b, "i2/cut-over");
    until("a write crosses once cut over", 15, || a.kv().get("i2/cut-over").is_some()).await;

    // Narrow: CA2 alone.
    // Each holds the other's successor record (published at the cutover) before narrowing: an anchor whose only kept
    // certificate is the old CA's would be dropped at the narrow (R4), and that node's writes refused until judged.
    let (ka_path, kb_path) = (revocation::cert_record_key(&ia, &ta.verifying_key_bytes()), revocation::cert_record_key(&ib, &tb.verifying_key_bytes()));
    until("the successor records cross", 15, || a.kv().get(&kb_path).as_deref() == Some(b2.as_slice()) && b.kv().get(&ka_path).as_deref() == Some(a2.as_slice())).await;
    b.change_roots(b2, vec![der2.clone()]).await.unwrap();
    a.change_roots(a2, vec![der2.clone()]).await.unwrap();
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

// ── I2, PR 2a: standing under changed roots ─────────────────────────────────────────────────────────────────────────
// The successor here is minted at generation 1 (`mint_ca_at`), so narrowing to it closes legacy (R1). What drives each
// change of roots is the test, through `GossipAgent::change_roots`; the record that drives it in a fleet is PR 2b.

#[cfg(feature = "provisional-ca-generation")]
fn pem(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

#[cfg(feature = "provisional-ca-generation")]
/// A gen-0 fleet CA and its generation-1 successor: directories and certificates' DER.
fn ca_and_successor(base: &Path) -> ((PathBuf, Vec<u8>), (PathBuf, Vec<u8>)) {
    let d1 = base.join("ca1");
    let c1 = crate::tls::mint_ca(&d1).unwrap();
    let d2 = base.join("ca2");
    let c2 = crate::tls::mint_ca_at(&d2, None, 1).unwrap();
    ((d1, der(&c1)), (d2, der(&c2)))
}

/// A node `id`'s certificate under `ca_dir`, for an existing or a fresh key: (its key, the DER).
fn member_cert(base: &Path, ca_dir: &Path, id: &NodeId, key: Option<[u8; 32]>) -> ([u8; 32], Vec<u8>) {
    match key {
        Some(k) => (k, crate::tls::issue_node_cert_for_key(ca_dir, id, k, 0).unwrap()),
        None => {
            let (cert, _) = crate::tls::issue_node_cert(ca_dir, id, &base.join(format!("member-{}", id.as_str().replace([':', '.'], "_")))).unwrap();
            let d = der(&cert);
            (crate::tls::ed25519_key_from_cert_der(&d).unwrap(), d)
        }
    }
}

fn put(ctx: &TaskCtx, path: &str, bytes: &[u8]) {
    let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path), bytes::Bytes::copy_from_slice(bytes));
}

/// Fold until this call ran one itself: a background fold (the identity watcher's) may be running, and `refresh`
/// then returns at once.
fn refold(ctx: &TaskCtx) {
    while !revocation::refresh_now(ctx) {
        std::thread::yield_now();
    }
}

async fn started(id: &NodeId, cfg: &GossipConfig) -> Arc<GossipAgent> {
    let a = Arc::new(GossipAgent::new(id.clone(), cfg.clone()));
    a.start().await.unwrap();
    a
}

/// **Exit 15 (R1): no legacy after narrowing.** A narrowed node that has not judged N's successor record trusts no key
/// of N — not its old-CA key, not a key in `peer_keys` with no certificate — and a legacy node L (never anchored, keys in
/// `peer_keys` only) has none either, through all three places the rule lives: `trusted_keys`, `is_distrusted`, and
/// `retain_trusted`'s fast path. Its own current key it keeps trusting. While widened in the first succession (roots of
/// generation 0 and 1) nothing changes. Once N's successor record is judged, N is trusted again (the transient refusal,
/// stated). The same holds on a node **started** on the successor's files with no `succession/` — the operator's recovery,
/// or a node installed after the succession (#768's H2) — and without `[persistence]`. Seen failing first with
/// `legacy_closed` reading false everywhere: the narrowed node did not close.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_narrowing_a_node_without_a_judged_successor_record_has_no_trusted_key() {
    let base = fresh_dir("r1");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let (ia, cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    let a = started(&ia, &cfg).await;
    let ctx = &a.task_ctx;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let l = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let (kn, n1) = member_cert(&base, &ca1, &n, None);
    let (stray, kl) = ([0x51u8; 32], crate::tls::ed25519_key_from_cert_der(&member_cert(&base, &ca1, &l, None).1).unwrap());
    put(ctx, &revocation::cert_record_key(&n, &kn), &n1);
    ctx.peer_keys.pin().insert(n.clone(), vec![kn, stray]);
    ctx.peer_keys.pin().insert(l.clone(), vec![kl]);
    refold(ctx);
    assert_eq!(ctx.trusted_keys(&n), vec![kn], "N anchored by its CA1 record");
    assert_eq!(ctx.trusted_keys(&l), vec![kl], "L legacy: from peer_keys, as today");

    a.change_roots(a.task_ctx.tls.get().unwrap().node_cert_der(), vec![der1.clone(), der2.clone()]).await.unwrap();
    assert!(!ctx.key_standing.legacy_closed(), "the first succession's widened node trusts generations 0 and 1");
    assert_eq!((ctx.trusted_keys(&n), ctx.trusted_keys(&l)), (vec![kn], vec![kl]), "widened: unchanged");

    let ka = ctx.tls.get().unwrap().verifying_key_bytes();
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(ka));
    a.change_roots(a2, vec![der2.clone()]).await.unwrap();
    assert!(ctx.key_standing.legacy_closed(), "narrowed to generation 1");
    assert!(ctx.trusted_keys(&n).is_empty(), "N's successor record not judged: no key, not even its old-CA one");
    assert!(ctx.trusted_keys(&l).is_empty(), "L: no legacy class");
    assert!(ctx.key_standing.is_distrusted(&l, &kl) && ctx.key_standing.is_distrusted(&n, &stray));
    let mut offered = vec![kn, stray];
    ctx.retain_trusted(&n, &mut offered);
    assert!(offered.is_empty(), "retain_trusted refuses both");
    assert!(ctx.trusted_keys(&ia).contains(&ka), "its own current key, for itself");

    let (_, n2) = member_cert(&base, &ca2, &n, Some(kn));
    put(ctx, &revocation::cert_record_key(&n, &kn), &n2);
    refold(ctx);
    assert_eq!(ctx.trusted_keys(&n), vec![kn], "N's successor record judged: trusted again");

    // A node started on the successor's files: closed from the first moment, with no succession/.
    let (ib, mut cfg_b) = node_cfg(&base, "b", &ca2, &ca2.join("ca-cert.pem"), vec![]);
    cfg_b.persistence = None;
    let b = started(&ib, &cfg_b).await;
    assert!(!cfg_b.tls.as_ref().unwrap().auto_cert_dir.join("succession").exists());
    assert!(b.task_ctx.key_standing.legacy_closed(), "closed from the root's generation alone");
    b.task_ctx.peer_keys.pin().insert(l.clone(), vec![kl]);
    put(&b.task_ctx, &revocation::cert_record_key(&n, &kn), &n1);
    refold(&b.task_ctx);
    assert!(b.task_ctx.trusted_keys(&l).is_empty() && b.task_ctx.trusted_keys(&n).is_empty(), "L legacy and N old-CA only: nothing");
    for x in [&a, &b] { x.shutdown().await; }
    let _ = std::fs::remove_dir_all(&base);
}

/// **#796's review, must-address 7: removing every anchor of a node fails closed, with legacy open.** A change of roots
/// between two generation-0 CAs leaves legacy open; a node N whose only anchor was a certificate of the dropped root
/// keeps its (empty) entry and trusts nothing — it is not handed back to `peer_keys` — while a node never anchored stays
/// legacy, as today. Seen failing first with `anchored: None` for a node whose anchors are all gone: N answered from
/// `peer_keys`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_every_anchor_of_a_node_fails_closed_with_legacy_open() {
    let base = fresh_dir("m7");
    let ((ca1, der1), (ca2, der2)) = two_cas(&base);
    let (ia, cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    let a = started(&ia, &cfg).await;
    let ctx = &a.task_ctx;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let l = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let (kn, n1) = member_cert(&base, &ca1, &n, None);
    put(ctx, &revocation::cert_record_key(&n, &kn), &n1);
    ctx.peer_keys.pin().insert(n.clone(), vec![kn]);
    ctx.peer_keys.pin().insert(l.clone(), vec![[0x61u8; 32]]);
    refold(ctx);
    assert_eq!(ctx.trusted_keys(&n), vec![kn]);
    let ka = ctx.tls.get().unwrap().verifying_key_bytes();
    a.change_roots(ctx.tls.get().unwrap().node_cert_der(), vec![der1, der2.clone()]).await.unwrap();
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(ka));
    a.change_roots(a2, vec![der2]).await.unwrap();
    assert!(!ctx.key_standing.legacy_closed(), "two generation-0 roots: legacy stays open");
    assert!(revocation::has_anchor_entry_for_test(&ctx.identity_ledger, &n), "the entry stays when empty");
    assert!(ctx.trusted_keys(&n).is_empty(), "N had standing and lost it: nothing, never peer_keys");
    assert_eq!(ctx.trusted_keys(&l), vec![[0x61u8; 32]], "a node never anchored is legacy, as today");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 18 (R4): kept anchors.** While widened, A judges N's old-CA record and then its successor record at the same
/// path, and keeps a certificate from each issuer. At narrowing the old-CA one is dropped (counted) and N stays anchored.
/// Then N's record is overwritten in A's store: N stays anchored — an anchor leaves only when its kept certificate
/// fails the roots, never because its record left the store — and, under `[persistence]`, A restarted on the
/// successor's files with the record still hidden keeps N anchored from the journal, the old-CA certificate restored and
/// dropped again by the first fold. Seen failing first with rev 0.9's prune from the store (a kept certificate dropped
/// when no store record offers it): the old-CA certificate was gone as soon as the successor record overwrote its record.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_anchor_survives_its_record_hidden_and_an_old_ca_one_is_dropped_at_narrowing() {
    let base = fresh_dir("r4");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base.join("persist"),
        sync_mode: crate::config::SyncMode::Flush,
        snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600,
        on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let a = started(&ia, &cfg).await;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let (kn, n1) = member_cert(&base, &ca1, &n, None);
    let path = revocation::cert_record_key(&n, &kn);
    put(&a.task_ctx, &path, &n1);
    refold(&a.task_ctx);
    a.change_roots(a.task_ctx.tls.get().unwrap().node_cert_der(), vec![der1.clone(), der2.clone()]).await.unwrap();
    let (_, n2) = member_cert(&base, &ca2, &n, Some(kn));
    put(&a.task_ctx, &path, &n2);
    refold(&a.task_ctx);
    assert_eq!(revocation::kept_for_test(&a.task_ctx.identity_ledger, &n, &kn).len(), 2, "widened: one certificate per issuer");
    let ka = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(ka));
    let dropped_before = a.task_ctx.identity_ledger.anchors_dropped.load(std::sync::atomic::Ordering::Relaxed);
    a.change_roots(a2.clone(), vec![der2.clone()]).await.unwrap();
    assert_eq!(revocation::kept_for_test(&a.task_ctx.identity_ledger, &n, &kn).len(), 1, "the old-CA certificate dropped");
    assert!(a.task_ctx.identity_ledger.anchors_dropped.load(std::sync::atomic::Ordering::Relaxed) > dropped_before, "and counted");
    assert_eq!(a.task_ctx.trusted_keys(&n), vec![kn]);
    put(&a.task_ctx, &path, b"hidden");
    refold(&a.task_ctx);
    assert_eq!(a.task_ctx.trusted_keys(&n), vec![kn], "the record left the store; the kept certificate did not");
    assert!(a.task_ctx.identity_ledger.anchors_durable_for(&a.task_ctx.identity_ledger.kept_certs()).await, "every kept certificate journaled");
    a.shutdown().await;

    // Restart on the successor's files (the end state the operator reaches), the record still hidden in the store.
    let cert_file = base.join("a2.cert.pem");
    std::fs::write(&cert_file, pem(&a2)).unwrap();
    let mut cfg2 = cfg.clone();
    let tls2 = cfg2.tls.as_mut().unwrap();
    tls2.cert_pem = Some(cert_file);
    tls2.ca_cert_pem = Some(ca2.join("ca-cert.pem"));
    let a = started(&ia, &cfg2).await;
    refold(&a.task_ctx);
    assert_eq!(a.kv().get(&path).as_deref(), Some(&b"hidden"[..]), "still hidden after the restart");
    assert_eq!(a.task_ctx.trusted_keys(&n), vec![kn], "anchored from the journal");
    assert_eq!(revocation::kept_for_test(&a.task_ctx.identity_ledger, &n, &kn).len(), 1, "the journaled old-CA certificate re-judged and dropped");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 18, the race on `anchors_seen` (#768's M3).** A fold judges N's old-CA record while A is widened; between its
/// judging and its merge (a plant), A narrows to the successor alone. The fold merges nothing and reports it did not
/// complete, so no old-CA certificate is kept; the next fold runs under the new roots. Seen failing first with the
/// generation check removed from the merge: the old-CA certificate was kept after the fold.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fold_whose_merge_meets_a_change_of_roots_merges_nothing() {
    let base = fresh_dir("m3");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let (ia, cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    let a = started(&ia, &cfg).await;
    let ctx = Arc::clone(&a.task_ctx);
    let t = Arc::clone(ctx.tls.get().unwrap());
    a.change_roots(t.node_cert_der(), vec![der1, der2.clone()]).await.unwrap();
    // The node's own tasks stop, so no background fold (the identity watcher's) judges N's record before this test's
    // fold does: the store, the ledger and the roots stay as they are.
    a.shutdown().await;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let (kn, n1) = member_cert(&base, &ca1, &n, None);
    put(&ctx, &revocation::cert_record_key(&n, &kn), &n1);
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(t.verifying_key_bytes()));
    let narrow = Arc::clone(&t);
    revocation::MERGE_PLANT.with(|p| *p.borrow_mut() = Some(Box::new(move || {
        narrow.activate(narrow.reroot_material(a2.clone(), vec![der2.clone()]).unwrap()).unwrap();
    })));
    assert!(!revocation::fold_node_for_test(&ctx, &n), "the roots moved under the fold: it did not complete");
    assert!(revocation::kept_for_test(&ctx.identity_ledger, &n, &kn).is_empty(), "nothing merged: no old-CA anchor");
    assert!(revocation::fold_node_for_test(&ctx, &n), "the next fold runs under the new roots");
    assert!(revocation::kept_for_test(&ctx.identity_ledger, &n, &kn).is_empty(), "and refuses the old-CA record");
    let _ = std::fs::remove_dir_all(&base);
}

/// **Exit 19 (I1's client marks) and #768's M4.** A client principal C re-issued under the successor over its key keeps
/// `Current` after narrowing, and one never re-issued, D, reads `Unknown` and is withdrawn from the external-issuer feed
/// (its possession proof verifies nothing). Under `[persistence]`, a restart on the successor's files with both store
/// records hidden re-judges each mark's journaled certificate (`RaisedWithCert`): C `Current`, D `Unknown`. A pre-upgrade
/// `Raised` entry (no certificate) still decodes and holds its mark as before on a restart outside any succession, and
/// reads `Unknown` after a change of roots until a verifying record re-establishes it. Seen failing first with the mark
/// left un-judged (`mark_stands` always true): D stayed `Current` after narrowing.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_marks_are_re_judged_at_a_change_of_roots() {
    use client_cert::{client_key_standing, ClientKeyStanding::*};
    let base = fresh_dir("m4");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base.join("persist"),
        sync_mode: crate::config::SyncMode::Flush,
        snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600,
        on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    // A pre-upgrade journal entry for E: a mark with no certificate.
    let (e_cert, _) = crate::tls::issue_client_cert(&ca1, "fleet", "elder", &base.join("e")).unwrap();
    let e1 = der(&e_cert);
    let ke = crate::tls::ed25519_key_from_cert_der(&e1).unwrap();
    let e_issued = mycelium_core::tls::client_identity(&e1).unwrap().issued_ms.unwrap();
    let journal = base.join("persist").join("identity").join("client-certs.journal");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    {
        let j = super::journal::Journal::open(&journal, "identity/client-certs").unwrap();
        j.append(client_cert::raised_entry_for_test("token:fleet/elder", e_issued, ke)).await.unwrap();
    }
    let a = started(&ia, &cfg).await;
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/elder", &ke), Current, "a Raised entry holds, outside any succession");
    let c_path_of = |p: &str, k: &[u8; 32]| client_cert::client_cert_key(p, k);
    // Records arrive as any member's write would (A is not their publisher, so it does not republish them), and the fold
    // raises the marks.
    let publish = |d: &[u8]| {
        let id = mycelium_core::tls::client_identity(d).unwrap();
        put(&a.task_ctx, &client_cert::client_cert_key(&id.principal(), &id.key), d);
        // Folds this principal now, whatever a background refresh is doing.
        let _ = client_key_standing(&a.task_ctx, &id.principal(), &id.key);
    };
    let (c_cert, _) = crate::tls::issue_client_cert(&ca1, "fleet", "svc", &base.join("c")).unwrap();
    let (d_cert, _) = crate::tls::issue_client_cert(&ca1, "fleet", "other", &base.join("d")).unwrap();
    let (c1, d1) = (der(&c_cert), der(&d_cert));
    let (kc, kd) = (crate::tls::ed25519_key_from_cert_der(&c1).unwrap(), crate::tls::ed25519_key_from_cert_der(&d1).unwrap());
    publish(&c1);
    publish(&d1);
    assert_eq!((client_key_standing(&a.task_ctx, "token:fleet/svc", &kc), client_key_standing(&a.task_ctx, "token:fleet/other", &kd)), (Current, Current));
    let t = Arc::clone(a.task_ctx.tls.get().unwrap());
    a.change_roots(t.node_cert_der(), vec![der1, der2.clone()]).await.unwrap();
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/elder", &ke), Unknown, "after a change of roots a mark without a certificate reads Unknown until re-established");
    let c_mark = a.task_ctx.client_certs.mark("token:fleet/svc").unwrap();
    let c2 = crate::tls::issue_client_cert_for_key(&ca2, "fleet", "svc", kc, c_mark.issued).unwrap();
    publish(&c2);
    assert!(a.task_ctx.client_certs.mark("token:fleet/svc").unwrap().issued > c_mark.issued, "the re-issued serial is later than the mark");
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(t.verifying_key_bytes()));
    a.change_roots(a2.clone(), vec![der2.clone()]).await.unwrap();
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/svc", &kc), Current, "re-issued: keeps its standing");
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/other", &kd), Unknown, "not re-issued: Unknown, never Current");
    assert!(client_cert::current_mark(&a.task_ctx, "token:fleet/other").is_none(), "no current key for the grant doors and the replay window");
    for (p, k) in [("token:fleet/svc", kc), ("token:fleet/other", kd)] {
        put(&a.task_ctx, &c_path_of(p, &k), b"hidden");
    }
    a.shutdown().await;

    let cert_file = base.join("a2.cert.pem");
    std::fs::write(&cert_file, pem(&a2)).unwrap();
    let mut cfg2 = cfg.clone();
    let tls2 = cfg2.tls.as_mut().unwrap();
    tls2.cert_pem = Some(cert_file);
    tls2.ca_cert_pem = Some(ca2.join("ca-cert.pem"));
    let a = started(&ia, &cfg2).await;
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/svc", &kc), Current, "RaisedWithCert re-judged under the successor: passes");
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/other", &kd), Unknown, "RaisedWithCert re-judged: fails");
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/elder", &ke), Unknown, "a Raised mark on a node trusting only successor roots");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#796's review, must-address 8: one owner over rotation and the change of roots.** A rotation in its propagation
/// wait holds the identity's owner; a change of roots asked for in that wait runs only after the rotation has cut over,
/// so the rotation is never refused after its new key was saved and published. (The change of roots, built for the key
/// before the rotation, is then refused by name — the key it was issued for is no longer this node's.) Seen failing
/// first with the owner not taken by `change_roots`: the change ran in the wait and the rotation's `activate` was refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rotation_and_a_change_of_roots_have_one_owner() {
    let base = fresh_dir("m8");
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let mut tls = crate::TlsConfig::default();
    tls.auto_cert_dir = base.join("node");
    cfg.tls = Some(tls);
    let a = started(&id, &cfg).await;
    let t = Arc::clone(a.task_ctx.tls.get().unwrap());
    let (old_key, old_cert, own_root) = (t.verifying_key_bytes(), t.node_cert_der(), t.roots_der()[0].clone());
    let other = base.join("other");
    let other_root = der(&crate::tls::mint_ca(&other).unwrap());
    let rotating = { let a = Arc::clone(&a); tokio::spawn(async move { a.rotate_identity(Duration::from_millis(800)).await }) };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let changed = a.change_roots(old_cert, vec![own_root, other_root]).await;
    let rotated = rotating.await.unwrap();
    let new_key = rotated.expect("the rotation cut over: nothing changed the roots in its wait");
    assert_ne!(new_key, old_key);
    assert_eq!(t.verifying_key_bytes(), new_key);
    let refused = changed.expect_err("the change of roots ran after the rotation, for a key no longer this node's").to_string();
    assert!(refused.contains("not for this node's current key"), "{refused}");
    assert_eq!(t.roots_der().len(), 1, "and changed nothing");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#796's review, must-address 6: an established session with an old-root peer does not outlive a narrow.** A and F
/// are connected on CA1; F trusts both roots throughout, and keeps its CA1 certificate. A narrows to the successor
/// alone. A's outbound session to F is re-judged before its next write and dropped, so A's write after the narrow
/// never reaches F — before, it crossed the old session, F still trusting A's records — and A's inbound session from F
/// is closed on F's next frame and counted. Seen failing first with the writer's re-judgment removed: A's write reached
/// F after the narrow.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_established_session_to_an_old_root_peer_is_dropped_at_narrowing() {
    let base = fresh_dir("m6");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let r1 = ca1.join("ca-cert.pem");
    let (ia, ca) = node_cfg(&base, "a", &ca1, &r1, vec![]);
    let (if_, cf) = node_cfg(&base, "f", &ca1, &r1, vec![ia.clone()]);
    let a = started(&ia, &ca).await;
    let f = started(&if_, &cf).await;
    f.change_roots(f.task_ctx.tls.get().unwrap().node_cert_der(), vec![der1.clone(), der2.clone()]).await.unwrap();
    let write = |from: &GossipAgent, key: &str| { let _ = from.kv().set(key.to_string(), bytes::Bytes::from_static(b"1")); };
    write(&f, "m6/f-first");
    until("F's write reaches A", 30, || a.kv().get("m6/f-first").is_some()).await;
    write(&a, "m6/a-first");
    until("A's write reaches F", 30, || f.kv().get("m6/a-first").is_some()).await;
    let ta = Arc::clone(a.task_ctx.tls.get().unwrap());
    a.change_roots(ta.node_cert_der(), vec![der1, der2.clone()]).await.unwrap();
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(ta.verifying_key_bytes()));
    a.change_roots(a2, vec![der2.clone()]).await.unwrap();
    // F still holds A's CA1 record and trusts CA1: a write from A that reached F would be accepted there.
    write(&a, "m6/a-after");
    write(&f, "m6/f-after");
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(f.kv().get("m6/a-after").is_none(), "A's write after the narrow did not cross the old session");
    let closed = ta.sessions_closed_by_roots();
    assert!(closed[0] + closed[1] >= 1, "A closed F's inbound session, counted: {closed:?}");
    assert!(closed[2] + closed[3] >= 1, "and its outbound one, counted apart: {closed:?}");
    assert_eq!(closed[1] + closed[3], 0, "F holds no successor certificate: an old-root peer, not a session that predates the change");
    for x in [&a, &f] { x.shutdown().await; }
    let _ = std::fs::remove_dir_all(&base);
}

/// **#805's review F3: an expiring kept anchor is judged in-process as at a restart.** N's certificate record expires
/// while A runs, with no change of roots: once it has expired, a fold drops it, so A trusts nothing for N — as a node
/// restarted then would. Seen failing first with the kept certificates re-judged only at a change of roots: N stayed
/// anchored after its certificate expired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_kept_anchor_is_dropped_while_the_node_runs() {
    let base = fresh_dir("f3-anchor");
    let ((ca1, _), _) = two_cas(&base);
    let (ia, cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    let a = started(&ia, &cfg).await;
    let ctx = &a.task_ctx;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let kn = [0x71u8; 32];
    let until_ms = mycelium_core::sim_seam::wall_now_ms() + 3_000;
    let n1 = crate::tls::with_not_after_for_test(until_ms, || crate::tls::issue_node_cert_for_key(&ca1, &n, kn, 0).unwrap());
    assert!(mycelium_core::tls::cert_not_after_ms(&n1).is_some_and(|t| t <= until_ms + 1_000), "a short-lived certificate");
    put(ctx, &revocation::cert_record_key(&n, &kn), &n1);
    refold(ctx);
    assert_eq!(ctx.trusted_keys(&n), vec![kn], "anchored while valid");
    put(ctx, &revocation::cert_record_key(&n, &kn), b"hidden");
    while mycelium_core::sim_seam::wall_now_ms() < until_ms + 1_100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    refold(ctx);
    assert!(ctx.trusted_keys(&n).is_empty(), "expired: dropped in-process, so N has no trusted key");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#805's review F3, the client marks.** A client's certificate expires while the node runs: its key reads `Unknown`
/// once it has expired, though the verdict was cached. Seen failing first with the cache's expiry ignored: the key
/// stayed `Current`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_client_certificate_loses_standing_while_the_node_runs() {
    use client_cert::{client_key_standing, ClientKeyStanding::*};
    let base = fresh_dir("f3-client");
    let ((ca1, _), _) = two_cas(&base);
    let (ia, cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    let a = started(&ia, &cfg).await;
    let kc = [0x72u8; 32];
    let until_ms = mycelium_core::sim_seam::wall_now_ms() + 3_000;
    let c1 = crate::tls::with_not_after_for_test(until_ms, || crate::tls::issue_client_cert_for_key(&ca1, "fleet", "brief", kc, 0).unwrap());
    put(&a.task_ctx, &client_cert::client_cert_key("token:fleet/brief", &kc), &c1);
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/brief", &kc), Current);
    while mycelium_core::sim_seam::wall_now_ms() < until_ms + 1_100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(client_key_standing(&a.task_ctx, "token:fleet/brief", &kc), Unknown, "expired: no current key");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#805's review F5: `retain_trusted`'s fast path reads `legacy_closed`.** With nothing judged at all (an empty
/// standing) and legacy closed, a legacy node's key is dropped by `retain_trusted`, and the knowledge layer's member keys
/// (`knowledge_keys::member_keys_of`, the caller it reaches) read it as revoked. Seen failing first with the fast path
/// reverted to `is_empty()` alone: the key was kept and read as retained.
#[tokio::test]
async fn retain_trusted_with_an_empty_standing_and_legacy_closed_keeps_nothing() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let a = GossipAgent::new(id, cfg);
    let ctx = &a.task_ctx;
    let l = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let kl = [0x73u8; 32];
    ctx.peer_keys.pin().insert(l.clone(), vec![kl]);
    assert!(ctx.key_standing.is_empty(), "nothing judged");
    let mut keys = vec![kl];
    ctx.retain_trusted(&l, &mut keys);
    assert_eq!(keys, vec![kl], "legacy open: kept, as today");
    ctx.key_standing.set_legacy_closed(true);
    let mut keys = vec![kl];
    ctx.retain_trusted(&l, &mut keys);
    assert!(keys.is_empty(), "legacy closed: dropped, on the fast path too");
    #[cfg(feature = "compliance")]
    {
        let members = super::knowledge_keys::member_keys_of(ctx);
        assert!(members.get(&l).is_some_and(|m| m.revoked.contains(&kl)), "the knowledge layer reads it as not trusted");
    }
}

/// **#805's confirm round N2: a failed anchor append is retried through a fold.** Under `[persistence]`, the append of
/// N's kept certificate fails; the next fold, once the backoff has passed, offers it again, and it is journaled — the
/// merge offers every kept certificate, not only new ones. Seen failing first with the merge offering only certificates
/// it did not hold before: the certificate was never journaled.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_anchor_append_is_retried_by_a_later_fold() {
    let base = fresh_dir("n2");
    let ((ca1, _), _) = ca_and_successor(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base.join("persist"),
        sync_mode: crate::config::SyncMode::Flush,
        snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600,
        on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let a = started(&ia, &cfg).await;
    let ledger = &a.task_ctx.identity_ledger;
    assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await, "start settled");
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let (kn, n1) = member_cert(&base, &ca1, &n, None);
    ledger.hook_appends_for_test(None, Some(ledger.anchor_mark()));
    put(&a.task_ctx, &revocation::cert_record_key(&n, &kn), &n1);
    refold(&a.task_ctx);
    let mine = vec![(n.clone(), <[u8; 32]>::from(<sha2::Sha256 as sha2::Digest>::digest(&n1)))];
    assert!(!ledger.anchors_durable_for(&mine).await, "the first append failed");
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    refold(&a.task_ctx);
    assert!(ledger.anchors_durable_for(&mine).await, "retried by the later fold, durable");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **#805's last item: the retry bookkeeping holds only kept certificates.** Under `[persistence]`, the append of N's
/// short-lived certificate fails, which leaves a backoff entry for it; once the certificate has expired and a fold drops
/// it, the entry is gone with it. Seen failing first with the pruning removed: the entry outlived the certificate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_anchor_takes_its_retry_bookkeeping_with_it() {
    let base = fresh_dir("prune");
    let ((ca1, _), _) = two_cas(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base.join("persist"),
        sync_mode: crate::config::SyncMode::Flush,
        snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600,
        on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let a = started(&ia, &cfg).await;
    let ledger = &a.task_ctx.identity_ledger;
    assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await, "start settled");
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let kn = [0x74u8; 32];
    let until_ms = mycelium_core::sim_seam::wall_now_ms() + 3_000;
    let n1 = crate::tls::with_not_after_for_test(until_ms, || crate::tls::issue_node_cert_for_key(&ca1, &n, kn, 0).unwrap());
    ledger.hook_appends_for_test(None, Some(ledger.anchor_mark()));
    put(&a.task_ctx, &revocation::cert_record_key(&n, &kn), &n1);
    refold(&a.task_ctx);
    let mine = vec![(n.clone(), <[u8; 32]>::from(<sha2::Sha256 as sha2::Digest>::digest(&n1)))];
    assert!(!ledger.anchors_durable_for(&mine).await, "the append failed");
    assert!(ledger.anchor_bookkeeping_for_test(&n, &n1), "a backoff entry while the certificate is kept");
    put(&a.task_ctx, &revocation::cert_record_key(&n, &kn), b"hidden");
    ledger.hook_appends_for_test(None, Some(u64::MAX - 1));
    while mycelium_core::sim_seam::wall_now_ms() < until_ms + 1_100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    refold(&a.task_ctx);
    assert!(a.task_ctx.trusted_keys(&n).is_empty(), "expired and dropped");
    assert!(!ledger.anchor_bookkeeping_for_test(&n, &n1), "its bookkeeping went with it");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(feature = "provisional-ca-generation")]
fn persisted(base: &Path) -> crate::config::PersistenceConfig {
    crate::config::PersistenceConfig {
        base_path: base.join("persist"),
        sync_mode: crate::config::SyncMode::Flush,
        snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600,
        on_unreadable: crate::config::OnUnreadable::Refuse,
    }
}

/// **The decision of 2026-11-17, on a live node: a long-lived, often-rotated peer does not exhaust the kept-anchor
/// budget.** N rotates forty times under one CA, each new key's certificate record folded as it arrives. A node keeps
/// its sixteen newest keys, so twenty-four of the forty anchors it journaled are dropped again — and before the
/// compaction the thirty-third was refused for good ("32 per node life"). Now the journal is compacted when the budget
/// is met: nothing is refused, every certificate the node keeps is journaled, and a restart reads back exactly the
/// sixteen newest keys as N's anchors, with every record hidden from the store. Seen failing first with the budget
/// refusing whenever the journal is full: anchors were refused, `anchors_durable_for(kept_certs())` answered `false`,
/// and the restarted node anchored N on older keys.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_often_rotated_peer_does_not_exhaust_the_kept_anchor_budget() {
    let base = fresh_dir("compact");
    let ((ca1, _), _) = ca_and_successor(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(persisted(&base));
    let a = started(&ia, &cfg).await;
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    let ledger = Arc::clone(&a.task_ctx.identity_ledger);
    let mut keys = Vec::new();
    let mut paths = Vec::new();
    for i in 0..40 {
        let (k, cert) = member_cert(&base.join(format!("rot{i}")), &ca1, &n, None);
        let path = revocation::cert_record_key(&n, &k);
        put(&a.task_ctx, &path, &cert);
        refold(&a.task_ctx);
        keys.push(k);
        paths.push(path);
        // Distinct issuance milliseconds, so "newest" is the order of rotation.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    // The budget's compaction runs off the fold's thread; the fold after it journals what it made room for.
    let t = Instant::now();
    loop {
        refold(&a.task_ctx);
        if ledger.anchors_durable_for(&ledger.kept_certs()).await || t.elapsed() > Duration::from_secs(15) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await, "every kept certificate is durable");
    assert_eq!(ledger.anchor_journal_refused.load(std::sync::atomic::Ordering::Relaxed), 0, "no anchor refused by the budget");
    assert!(ledger.compaction.removed.load(std::sync::atomic::Ordering::Relaxed) >= 8, "dropped anchors left the journal");
    let newest: std::collections::HashSet<[u8; 32]> = keys[24..].iter().copied().collect();
    assert_eq!(a.task_ctx.trusted_keys(&n).into_iter().collect::<std::collections::HashSet<_>>(), newest);
    for p in &paths {
        put(&a.task_ctx, p, b"hidden");
    }
    a.shutdown().await;

    let a = started(&ia, &cfg).await;
    refold(&a.task_ctx);
    assert_eq!(a.task_ctx.trusted_keys(&n).into_iter().collect::<std::collections::HashSet<_>>(), newest, "the sixteen newest keys, from the compacted journal");
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// **A change of roots compacts before it answers** (the decision of 2026-11-17; the must-address item #805's confirm
/// round left for PR 2b): with the journal's budget for N full of anchors this node has dropped, the certificate a
/// change of roots keeps is journaled before `change_roots` returns — `anchors_durable_for(kept_certs())` is `true`
/// again, which is what a Narrowed phase waits on. The budget's own background compaction is held off, so only the
/// change of roots can have made room. Seen failing first with the compaction removed from `change_roots`: the kept
/// certificate was not journaled when it returned.
#[cfg(feature = "provisional-ca-generation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_of_roots_compacts_the_anchor_journal_before_it_answers() {
    let base = fresh_dir("compact-roots");
    let ((ca1, der1), (ca2, der2)) = ca_and_successor(&base);
    let (ia, mut cfg) = node_cfg(&base, "a", &ca1, &ca1.join("ca-cert.pem"), vec![]);
    cfg.persistence = Some(persisted(&base));
    let a = started(&ia, &cfg).await;
    let ledger = Arc::clone(&a.task_ctx.identity_ledger);
    let n = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    // Thirty-two old-CA anchors of N journaled: sixteen keys' worth kept, the budget full.
    let mut newest = None;
    for i in 0..32 {
        let (k, cert) = member_cert(&base.join(format!("rot{i}")), &ca1, &n, None);
        put(&a.task_ctx, &revocation::cert_record_key(&n, &k), &cert);
        refold(&a.task_ctx);
        newest = Some(k);
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await);
    assert_eq!(ledger.anchors_journaled_count_for_test(&n), 32, "the budget for N is full, sixteen of its records dead");
    ledger.block_spawned_compaction_for_test();
    // Widen, and N's newest key is re-issued under the successor.
    let kn = newest.unwrap();
    a.change_roots(a.task_ctx.tls.get().unwrap().node_cert_der(), vec![der1.clone(), der2.clone()]).await.unwrap();
    let (_, n2) = member_cert(&base, &ca2, &n, Some(kn));
    put(&a.task_ctx, &revocation::cert_record_key(&n, &kn), &n2);
    refold(&a.task_ctx);
    assert!(!ledger.anchor_journaled_for_test(&n, &n2), "the budget is full and nothing has compacted");
    // Narrow.
    let ka = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
    let (_, a2) = member_cert(&base, &ca2, &ia, Some(ka));
    a.change_roots(a2, vec![der2.clone()]).await.unwrap();
    assert!(ledger.anchor_journaled_for_test(&n, &n2), "journaled before the change of roots returned");
    assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await, "every kept certificate is durable again");
    assert_eq!(a.task_ctx.trusted_keys(&n), vec![kn]);
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}
