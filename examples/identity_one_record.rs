//! **One record, not two** — why a node's identity and its proof travel together.
//!
//! ```text
//! cargo run --example identity_one_record --features tls,compliance
//! ```
//!
//! # What this demonstrates, and what it does not
//!
//! **Demonstrates, by running:** two nodes that both *require* signed identity proofs still
//! authenticate each other's signed claims; and the record that makes that safe is **one KV entry**
//! carrying the key history *and* the proof, not two entries that happen to arrive together.
//!
//! **Does not demonstrate:** the race it prevents. That needs two processes and a gossip delay, so
//! it lives in the Docker suites. An in-process example cannot lose a race it has no way to run —
//! and an example that *narrated* a race it never ran would be the same liability as a test that
//! cannot fail.
//!
//! # The claim this exists to make checkable
//!
//! Before Phase 3b a node published its identity at `sys/identity/{node}` and the proof that
//! authenticates it at `sys/identity-proof/{node}` — **two** entries, therefore **two gossip
//! messages, with no ordering between them.**
//!
//! A peer that required proofs could see the identity first, find no proof, and reject it —
//! holding **no key** for that node until the proof arrived. The key recovers by itself (the
//! identity watcher re-validates when the proof lands), so the window is transient. A **decision
//! taken inside it is not**: a leader election is one-shot, and a node that could not verify a
//! peer's signature while the window was open does not re-run the election a moment later.
//!
//! `sys/identity-signed/{node}` is the same two facts in **one** entry. One entry cannot arrive in
//! two parts — so the window is closed **by construction rather than by timing**, which is the only
//! kind of fix worth having for a race.
//!
//! # The limit this does *not* close, which matters more than it sounds
//!
//! *Proofs required* is **not** *identity authenticated*. First sighting of a node you have never
//! seen is still **trust on first use**: a self-signed entry is accepted, because there is nothing
//! established to chain it to. What closes that is an **anchor** — a direct, CA-validated
//! connection that records the peer's real key. Proofs close the *unsigned mimic* residual; anchors
//! close the *first sighting* one. They are complementary, not alternatives.
//!
//! That boundary is pinned by a test written to **fail if the window is ever closed**
//! (`lib_tests::identity_proof_default::requiring_proofs_does_not_close_trust_on_first_use`), so
//! the claim cannot rot into prose while the code moves under it.

use mycelium::{GossipAgent, GossipConfig, NodeId, TlsConfig};
use std::sync::Arc;
use std::time::Duration;

fn note(s: impl AsRef<str>) { println!("   {}", s.as_ref()); }

#[tokio::main]
async fn main() {
    let base: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57500);
    let (pa, pb) = (base, base + 1);
    let node_a = NodeId::new("127.0.0.1", pa).expect("node id");

    // One cert dir ⇒ one CA ⇒ mutual trust. A unique dir per run keeps concurrent runs apart.
    let cert_dir = std::env::temp_dir().join(format!("myc-identity-{pa}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mk = |port: u16, boots: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boots;
        cfg.health_check_interval_secs = 1;
        // The opt-in under test. Both nodes refuse any identity they cannot authenticate —
        // including each other's.
        cfg.require_identity_proofs = true;
        cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).expect("id"), cfg))
    };

    println!("\x1b[1mTwo TLS nodes, both requiring signed identity proofs\x1b[0m");
    let a = mk(pa, vec![]);
    let b = mk(pb, vec![node_a.clone()]);
    a.start().await.expect("start a");
    b.start().await.expect("start b");

    for _ in 0..200 {
        if !a.peers().is_empty() && !b.peers().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!a.peers().is_empty(), "the two nodes never peered");
    note("peered");

    // ── 1. the record is one entry ──────────────────────────────────────────────────────────
    println!("\n\x1b[1m1. What A publishes about itself\x1b[0m");

    let sealed = a.kv().get(&format!("sys/identity-signed/{node_a}"))
        .expect("a TLS node publishes its sealed identity record");
    let legacy_id = a.kv().get(&format!("sys/identity/{node_a}"));
    let legacy_proof = a.kv().get(&format!("sys/identity-proof/{node_a}"));

    note(format!("sys/identity-signed/{{A}}  {:>4} bytes  ← version ‖ history ‖ proof, ONE entry",
                 sealed.len()));
    note(format!("sys/identity/{{A}}         {:>4} bytes  ┐ the legacy pair, still published so a",
                 legacy_id.as_ref().map_or(0, |b| b.len())));
    note(format!("sys/identity-proof/{{A}}   {:>4} bytes  ┘ node older than this release still works",
                 legacy_proof.as_ref().map_or(0, |b| b.len())));

    // The shape, checked rather than asserted in prose: a version byte, a whole number of 32-byte
    // keys, and a fixed-width 96-byte proof.
    assert_eq!(sealed[0], 1, "version byte leads, so a future format is refused not misparsed");
    let history_len = sealed.len() - 1 - 96;
    assert!(history_len > 0 && history_len % 32 == 0, "history is a whole number of keys");
    note(format!("→ version {} · {} key(s) · 96-byte proof", sealed[0], history_len / 32));
    note("The pair is two gossip messages with no ordering. This is one, which cannot arrive");
    note("in two parts — the window is closed by construction rather than by timing.");

    // ── 2. and it is sufficient on its own ──────────────────────────────────────────────────
    println!("\n\x1b[1m2. B authenticates A — with proofs required on both sides\x1b[0m");
    note("`roles_of` returns Some only once A's signed claim has gossiped to B *and* A's");
    note("verifying key has reached B's peer_keys — so a key that never arrived shows up here");
    note("as a missing role, not as a silent success.");

    a.advertise_roles(["dispatcher".into()], 3).expect("advertise with a tls identity");

    let mut claim = None;
    for _ in 0..200 {
        if let Some(c) = b.roles_of(&node_a) { claim = Some(c); break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let claim = claim.expect("B never verified A's claim — A's key did not reach peer_keys");
    assert!(claim.has_role("dispatcher"));
    note("✓ B verified A's signed role claim, so A's key authenticated under the sealed record");

    // ── 3. what is still open ───────────────────────────────────────────────────────────────
    println!("\n\x1b[1m3. What requiring proofs does NOT give you\x1b[0m");
    note("*Proofs required* is not *identity authenticated*. First sighting of a node you have");
    note("never seen is still trust-on-first-use: a self-signed entry is accepted, because there");
    note("is nothing established to chain it to. An admitted-but-hostile member can therefore");
    note("still introduce a key for a node nobody has met.");
    note("");
    note("What closes that is an ANCHOR — a direct, CA-validated connection recording the peer's");
    note("real key — after which an unchained key is rejected and counted in");
    note("`identity_anchor_conflicts`. Proofs close the unsigned-mimic residual; anchors close");
    note("the first-sighting one. Complementary, not alternatives.");

    println!("\n\x1b[1mOne entry, because two entries are two messages.\x1b[0m");

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
