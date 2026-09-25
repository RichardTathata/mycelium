//! **Three questions an auditor asks** — and the three mechanisms that answer them by running.
//!
//! ```text
//! cargo run --example auditor_questions --features tls,compliance
//! ```
//!
//! # The claim this exists to make checkable
//!
//! An auditor's questions are not the ones an architecture diagram answers. They are narrow,
//! awkward, and about *evidence*:
//!
//! | # | The question | The mechanism | What a bad answer looks like |
//! |---|---|---|---|
//! | 1 | **Who actually did this?** | [`GatewayCaller`] | "the gateway node did it" — true, useless, and the same answer for every client |
//! | 2 | **Can you prove the log was not edited?** | the sealed audit chain + [`AuditSink`] | "we trust our operators" |
//! | 3 | **Can you erase one person?** | [`SubjectKeyRegistry`] | "we deleted the row" — from a gossip mesh that replicated it |
//!
//! # Why the third one is the hard one
//!
//! You cannot un-gossip bytes. An entry that propagated has been written to every replica's store
//! and possibly its write-ahead log, and no delete you issue afterwards can reach into a partitioned
//! node's disk. A substrate that promised true deletion would be lying about physics.
//!
//! So erasure here is **cryptographic**: the personal data is envelope-encrypted per subject, and
//! erasing the subject means destroying that subject's key. The ciphertext stays exactly where it
//! is — in every replica, in every backup — and becomes unreadable **everywhere at once**, including
//! on the node that was partitioned when you pressed the button. That is a stronger property than
//! deletion, and it is also the only one that is true.

use mycelium::{
    audit_key, AuditAction, AuditOutcome, AuditSink, GossipAgent, GossipConfig, NodeId,
    SignedAuditRecord, SubjectKeyRegistry, TlsConfig,
};
use std::sync::{Arc, Mutex};

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}
fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

/// A stand-in for the SIEM / WORM bucket an operator actually exports to.
///
/// The real thing is an S3 object-lock writer or a syslog forwarder. What matters for the contract
/// is the shape: records arrive **in seal order**, already signed, and the sink cannot alter them —
/// it is handed `&SignedAuditRecord`, not a builder.
struct CollectingSink(Mutex<Vec<(u64, String)>>);

impl AuditSink for CollectingSink {
    fn export(&self, record: &SignedAuditRecord) {
        self.0.lock().expect("sink lock").push((record.record.seq, format!("{:?}", record.record.action)));
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57700);
    let id = NodeId::new("127.0.0.1", port).expect("node id");
    let cert_dir = std::env::temp_dir().join(format!("myc-auditor-{port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    // The audit chain is signed, so it needs a TLS identity — an unsigned "tamper-evident" log
    // would be a contradiction in terms.
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));

    let sink = Arc::new(CollectingSink(Mutex::new(Vec::new())));
    agent.with_audit_sink(sink.clone());
    agent.start().await.expect("start");

    /// The principal a gateway would have put in the envelope — used below as the actor an audit
    /// record names, because "who asked" and "what the log says" must be the same fact.
    const PRINCIPAL: &str = "token:depot-issuer/dispatch-bot";

    println!("\x1b[1mThree questions an auditor asks\x1b[0m");

    // ── 1. Who actually did this? ───────────────────────────────────────────────────────────
    step(1, "Who actually did this?");
    note("A provider that logs the calling *node* has answered nothing: every request through a");
    note("gateway arrives from the same node. The envelope a provider receives names the client.");
    note("");

    // The first thing to notice is what this example *cannot do*. `GatewayCaller` is
    // `#[non_exhaustive]` with no public constructor: an application cannot mint one, even in its
    // own process. Only the gateway's auth layer builds it, from a credential it verified.
    //
    //     let forged = GatewayCaller { principal: "admin".into(), .. };   // does not compile
    //
    // That is the property. An identity an application can fabricate is an identity an auditor
    // cannot rely on, so the type refuses to be fabricated.
    note("`GatewayCaller` cannot be constructed here — no public constructor, and the struct is");
    note("#[non_exhaustive]. Only the gateway's auth layer mints one, from a credential it");
    note("verified. An identity the application can fabricate is one an auditor cannot use.");
    note("");
    note("What it carries when a provider receives it:");
    note("  principal    who asked          — `oidc:{sub}` or `token:{issuer}/{name}`, never the credential");
    note("  via          whose gateway      — checked against the RPC frame's verified sender");
    note("  scopes       the credential's scopes ∩ what the route required");
    note("  attestation  Signed{..} on a tls mesh, UnauthenticatedMesh otherwise — stated, not assumed");

    // And here is the fact an operator can actually check: this node publishes the marker that
    // says it strips and verifies the envelope. A secure-profile gateway dispatches only to
    // providers carrying it — a node without it would run the call as *itself*.
    // The marker is written LAZILY — once a peer connects, or after a grace period — never inside
    // `start()`. A gossip write fans out to the bootstrap peers, and a peer that is not listening
    // yet puts this node's outbound writer into reconnect backoff, during which later frames to it
    // are dropped. So a single-node example must wait for it rather than read it immediately.
    let marker = format!("sys/caller-context/{id}");
    let mut seen = None;
    for _ in 0..80 {
        if let Some(v) = agent.kv().get(&marker) { seen = Some(v); break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    match seen {
        Some(v) => {
            note(format!("✓ {marker} = {:?}", String::from_utf8_lossy(&v)));
            note("  This node enforces the envelope, so a secure-profile gateway is willing to");
            note("  dispatch to it. A node without the marker would run the call as *itself*, and");
            note("  the gateway refuses rather than silently losing the caller's identity.");
            note("  The handshake is a fact in the store, not a promise in a README.");
        }
        None => note("· marker not yet published — it is written lazily, after a peer connects or a\n     grace period elapses, so a gossip write cannot land on a peer that is not listening"),
    }

    // ── 2. Can you prove the log was not edited? ────────────────────────────────────────────
    step(2, "Can you prove the log was not edited?");

    for (target, outcome) in [
        ("depot/van-3",  AuditOutcome::Success),
        ("depot/van-7",  AuditOutcome::Success),
        ("ledger/audit", AuditOutcome::Denied),
    ] {
        agent.audit(AuditAction::Invoke, PRINCIPAL, target, outcome, None)
            .expect("seal an audit record");
    }
    note("three events sealed into the hash chain");

    agent.audit_verify(&id).expect("a fresh chain verifies");
    note("✓ chain verifies — each record commits to its predecessor's hash");

    let exported = sink.0.lock().expect("sink lock").clone();
    note(format!("exported to the sink: {} record(s), in seal order {:?}",
                 exported.len(), exported.iter().map(|(s, _)| *s).collect::<Vec<_>>()));
    assert_eq!(exported.len(), 3, "every sealed record reaches the sink");
    note("The sink is handed `&SignedAuditRecord` — already signed, in order, and not a builder.");
    note("An exporter that could edit what it exports would be the hole it exists to close.");

    // Now edit the log, as an insider with store access would.
    let stream = agent.audit_stream(&id);
    // `audit_key` rather than a hand-written format. The key is `…/{seq:016x}`, zero-padded so
    // the prefix scan is lexicographically ordered — and an earlier draft of this example wrote
    // `…/{seq}`, which quietly created a *new* key instead of overwriting one. The chain was
    // untouched, verification passed, and it looked exactly like a substrate failing to notice
    // tampering. It had noticed nothing because nothing had been tampered with.
    let victim = audit_key(&id, stream[1].record.seq);
    assert!(agent.kv().get(victim.as_str()).is_some(), "the key we are about to edit must exist");
    let _ = agent.kv().set(victim.as_str(), b"{\"seq\":1,\"tampered\":true}".to_vec());
    note(format!("…an insider overwrites {victim}"));

    match agent.audit_verify(&id) {
        Err(e) => note(format!("✓ verification now FAILS: {e:?}")),
        Ok(()) => panic!("BREACH: an edited chain still verified"),
    }
    note("Note *how* it failed: a corrupted record no longer decodes, so it drops out of the");
    note("stream — and the hole it leaves is a SequenceGap. You cannot corrupt a record into");
    note("invisibility, because the chain counts as well as links.");
    note("Tamper-EVIDENT, not tamper-proof. Anyone with disk access can change the bytes; what");
    note("they cannot do is make the chain agree with them afterwards — and the exported copy");
    note("already left the building.");

    // ── 3. Can you erase one person? ────────────────────────────────────────────────────────
    step(3, "Can you erase one person?");
    note("The awkward fact first: you cannot un-gossip bytes. An entry that propagated is on every");
    note("replica's disk, and a delete cannot reach a node that is currently partitioned.");

    let registry = SubjectKeyRegistry::new();
    let subject = "person:4471";
    let pii = b"Rosa Alvarez, 14 Sandbank Rd, allergy: penicillin";

    let sealed = registry.encrypt_for(subject, pii);
    let _ = agent.kv().set("depot/volunteer/4471", sealed.clone());
    note(format!("stored {} bytes of ciphertext at depot/volunteer/4471", sealed.len()));

    let read_back = registry.decrypt_for(subject, &sealed).expect("readable while the key lives");
    assert_eq!(read_back, pii);
    note("readable — while the subject's key exists");

    assert!(registry.destroy(subject), "the key existed and was destroyed");
    note("→ destroy(subject)");

    assert!(registry.decrypt_for(subject, &sealed).is_none(), "unreadable once the key is gone");
    note("✓ unreadable — and the ciphertext has not moved. It is still in this store, still in");
    note("  every replica, still in any backup — and now meaningless in all of them at once,");
    note("  including on the node that was partitioned when the request came in.");

    let still_there = agent.kv().get("depot/volunteer/4471").expect("bytes remain");
    assert_eq!(still_there.len(), sealed.len(), "erasure did not need the bytes back");
    note(format!("  (the {} bytes are still at rest; that is the point, not a leak)",
                 still_there.len()));

    println!("\n\x1b[1mWho asked · whether the record stands · whether one person can leave.\x1b[0m");
    println!("Three questions, three mechanisms, none of which is \"trust the operator\".");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
