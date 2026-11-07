//! SPIKE S-succeed (identity-and-lifecycle plan, wave 1b) — throwaway. Does a node on `cert_pem`
//! (no CA key) take a certificate issued under a successor CA without a restart?
use super::*;
use crate::test_util::alloc_port;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

struct Node { id: NodeId, cert: PathBuf, key: PathBuf, cfg: GossipConfig }

fn node(base: &Path, name: &str, ca_dir: &Path, trust: &Path, boot: Vec<NodeId>) -> Node {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let out = base.join(name);
    let (cert, key) = crate::tls::issue_node_cert(ca_dir, &id, &out).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.bootstrap_peers = boot;
    cfg.health_check_max_jitter_ms = 1;
    // SPIKE M1: the join timing under a lower handshake bound (`SPIKE_HS_MS`, default main's 10 000).
    if let Some(ms) = std::env::var("SPIKE_HS_MS").ok().and_then(|v| v.parse().ok()) { cfg.handshake_timeout_ms = ms; }
    // SPIKE M1: and under a shorter health-check (anti-entropy) tick (`SPIKE_HC_S`, default main's 10).
    if let Some(v) = std::env::var("SPIKE_HC_S").ok().and_then(|v| v.parse().ok()) { cfg.health_check_interval_secs = v; }
    cfg.tls = Some(crate::TlsConfig {
        cert_pem: Some(cert.clone()), key_pem: Some(key.clone()), ca_cert_pem: Some(trust.to_path_buf()),
        auto_cert_dir: base.join(format!("{name}-node")),
    });
    Node { id, cert, key, cfg }
}

async fn until(what: &str, secs: u64, f: impl Fn() -> bool) -> Duration {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    t.elapsed()
}

fn der(p: &Path) -> Vec<u8> { CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec() }

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spike_s_succeed_a_cert_pem_node_re_roots_live() {
    let base = std::env::temp_dir().join(format!("spike-s-succeed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (ca1, ca2) = (base.join("ca1"), base.join("ca2"));
    std::fs::create_dir_all(&ca1).unwrap();
    std::fs::create_dir_all(&ca2).unwrap();
    let r1 = crate::tls::mint_ca(&ca1).unwrap();
    let r2 = crate::tls::mint_ca(&ca2).unwrap();

    // ── 0. A and B on CA1 certificates, the secure posture: no ca-key.pem on either node.
    let na = node(&base, "a", &ca1, &r1, vec![]);
    let nb = node(&base, "b", &ca1, &r1, vec![na.id.clone()]);
    for n in [&na, &nb] {
        assert!(!n.cfg.tls.as_ref().unwrap().auto_cert_dir.join("ca-key.pem").exists());
    }
    let a = Arc::new(GossipAgent::new(na.id.clone(), na.cfg.clone()));
    a.start().await.unwrap();
    let b = Arc::new(GossipAgent::new(nb.id.clone(), nb.cfg.clone()));
    b.start().await.unwrap();
    until("A and B peer on CA1", 10, || !a.peers().is_empty() && !b.peers().is_empty()).await;
    let ta = a.task_ctx.tls.get().unwrap();
    let tb = b.task_ctx.tls.get().unwrap();
    for n in [&na, &nb] {
        assert!(!n.cfg.tls.as_ref().unwrap().auto_cert_dir.join("ca-key.pem").exists(), "no CA key appeared on a node");
    }

    // Control: a fresh CA1 node E joins A before any re-root; the same write-reaches-A timing.
    let ne = node(&base, "e", &ca1, &r1, vec![na.id.clone()]);
    let e = Arc::new(GossipAgent::new(ne.id.clone(), ne.cfg.clone()));
    e.start().await.unwrap();
    let _ = e.kv().set("spike/from-e".to_string(), bytes::Bytes::from_static(b"1"));
    let control = until("control: E's (CA1) write reaches A", 20, || a.kv().get("spike/from-e").is_some()).await;
    eprintln!("SPIKE S-succeed: control — a fresh CA1 node's first write reached A in {control:?}");

    // B's current key, re-issued under the successor CA (on the issuer's host, CA2's key there).
    let b2 = base.join("b").join("b.ca2.cert.pem");
    crate::tls::issue_node_cert_for_key(&ca2, &nb.id, &nb.key, &b2, mycelium_core::sim_seam::wall_now_ms()).unwrap();
    assert!(!ta.verify_member_cert(&der(&b2)), "before: A does not know CA2");

    // ── 1. B takes its CA2 certificate live, trusting both roots. A trusts CA1 only.
    let [material_cost, record_cost, activate_cost, reroot_cost] = b.spike_reroot(&b2, &nb.key, &[&r1, &r2]).unwrap();
    eprintln!("SPIKE S-succeed: L2 — one spike_reroot call: material {material_cost:?}; certificate-record write {record_cost:?}; activate {activate_cost:?}; whole {reroot_cost:?}");
    let before = handshake(tb.client_config(), ta.server_config());
    eprintln!("SPIKE S-succeed: B (CA2 cert) dialling A (CA1 only): {before:?}");
    assert!(before.as_ref().is_err_and(|e| e.starts_with("server refused")), "before A re-roots, A refuses B's CA2 cert: {before:?}");
    let dial = handshake(ta.client_config(), tb.server_config());
    eprintln!("SPIKE S-succeed: A (CA1 only) dialling B (CA2 cert): {dial:?}");
    assert!(dial.as_ref().is_err_and(|e| e.starts_with("client refused")), "and A, dialling, refuses B's CA2 server cert: {dial:?}");
    let _ = b.kv().set("spike/after-b-cutover".to_string(), bytes::Bytes::from_static(b"1"));
    let carried = until("B's write after its cutover reaches A (no new handshake can succeed: the existing session carries it)", 10,
        || a.kv().get("spike/after-b-cutover").is_some()).await;
    eprintln!("SPIKE S-succeed: B's post-cutover write reached A in {carried:?}");

    // ── 2. A widens its trust to both roots, live, keeping its CA1 certificate.
    a.spike_reroot(&na.cert, &na.key, &[&r1, &r2]).unwrap();
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "B (CA2) → A (CA1, two roots)");
    assert_eq!(handshake(ta.client_config(), tb.server_config()), Ok(()), "A (CA1) → B (CA2, two roots)");
    assert!(ta.verify_member_cert(&der(&b2)), "after: A's certificate-record check accepts the CA2 cert");

    // ── 3. A real mesh handshake on live A under the new root: a fresh node C, CA2-issued, dials A.
    let nc = node(&base, "c", &ca2, &r1, vec![na.id.clone()]); // C trusts CA1 (A's cert), holds a CA2 cert
    let c = Arc::new(GossipAgent::new(nc.id.clone(), nc.cfg.clone()));
    c.start().await.unwrap();
    let _ = c.kv().set("spike/from-c".to_string(), bytes::Bytes::from_static(b"1"));
    let joined = until("C's (CA2 cert) write reaches live A over a fresh mTLS session", 15, || a.kv().get("spike/from-c").is_some()).await;

    // ── 4. End of the window: A on its own CA2 cert, CA2 root only. A CA1-only node is cut off.
    let a2 = base.join("a").join("a.ca2.cert.pem");
    crate::tls::issue_node_cert_for_key(&ca2, &na.id, &na.key, &a2, mycelium_core::sim_seam::wall_now_ms()).unwrap();
    a.spike_reroot(&a2, &na.key, &[&r2]).unwrap();
    assert_eq!(handshake(tb.client_config(), ta.server_config()), Ok(()), "B (CA2) → A (CA2 only)");
    // L5: a socket session in which A presents its own successor certificate — a fresh node G that trusts
    // only CA2 (so it can verify only A's new certificate) dials A; A's write reaches G.
    let ng = node(&base, "g", &ca2, &r2, vec![na.id.clone()]);
    let g = Arc::new(GossipAgent::new(ng.id.clone(), ng.cfg.clone()));
    g.start().await.unwrap();
    let _ = a.kv().set("spike/from-a-after-narrow".to_string(), bytes::Bytes::from_static(b"1"));
    let g_join = until("L5: A's write reaches G (CA2 trust only) over a session presenting A's CA2 certificate", 20,
        || g.kv().get("spike/from-a-after-narrow").is_some()).await;
    eprintln!("SPIKE S-succeed: L5 — G (trusts CA2 only) received A's write in {g_join:?}: A presented its successor certificate on a socket");
    // M2: F trusts BOTH roots but holds an old-root (CA1) certificate — isolates A's narrowed accept side.
    let nf = node(&base, "f", &ca1, &r1, vec![na.id.clone()]);
    let f = Arc::new(GossipAgent::new(nf.id.clone(), nf.cfg.clone()));
    f.start().await.unwrap();
    f.spike_reroot(&nf.cert, &nf.key, &[&r1, &r2]).unwrap(); // F widens; keeps its CA1 certificate
    let tf = f.task_ctx.tls.get().unwrap();
    let f_to_a = handshake(tf.client_config(), ta.server_config());
    let a_to_f = handshake(ta.client_config(), tf.server_config());
    eprintln!("SPIKE S-succeed: M2 — F (two roots, CA1 cert) → A (CA2 only): {f_to_a:?}; A → F: {a_to_f:?}");
    assert!(f_to_a.as_ref().is_err_and(|e| e.starts_with("server refused")), "A's narrowed accept side refuses an old-root certificate: {f_to_a:?}");
    assert!(a_to_f.as_ref().is_err_and(|e| e.starts_with("client refused")), "A's narrowed dial side refuses an old-root server certificate: {a_to_f:?}");
    let _ = f.kv().set("spike/from-f".to_string(), bytes::Bytes::from_static(b"1"));
    let nd = node(&base, "d", &ca1, &r1, vec![na.id.clone()]);
    let d = Arc::new(GossipAgent::new(nd.id.clone(), nd.cfg.clone()));
    d.start().await.unwrap();
    let td = d.task_ctx.tls.get().unwrap();
    let cut = handshake(td.client_config(), ta.server_config());
    let cut_back = handshake(ta.client_config(), td.server_config());
    eprintln!("SPIKE S-succeed: D (CA1) → A (CA2 only): {cut:?}; A → D: {cut_back:?}");
    assert!(cut.is_err() && cut_back.is_err(), "a CA1-only node is refused both ways once the window ends");
    let _ = d.kv().set("spike/from-d".to_string(), bytes::Bytes::from_static(b"1"));
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(a.kv().get("spike/from-d").is_none(), "D's write never reaches A");
    assert!(a.kv().get("spike/from-f").is_none(), "F's write never reaches A");
    eprintln!("SPIKE S-succeed: membership believes D? {} (a belief, not a session)", a.peers().contains(&nd.id));

    eprintln!("SPIKE S-succeed: SUMMARY hs_ms={} hc_s={} control_join={control:?} c_join={joined:?} g_join={g_join:?} b_carried={carried:?} reroot_whole={reroot_cost:?}", na.cfg.handshake_timeout_ms, na.cfg.health_check_interval_secs);
    for n in [&a, &b, &c, &d, &e, &f, &g] { n.shutdown().await; }
    let _ = std::fs::remove_dir_all(&base);
}
