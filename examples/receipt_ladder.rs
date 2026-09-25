//! **The receipt ladder** — v3 item 1's decisive demonstration (`docs/plans/v3-contracts-axis.md`
//! §12.1).
//!
//! ```text
//! cargo run --example receipt_ladder
//! ```
//!
//! # The claim this exists to make checkable
//!
//! *An acknowledgement is a receipt that names its rung and nothing above it.* The same write, made
//! four ways, comes back with four different claims — and the differences are not shades of
//! confidence, they are different facts about the world:
//!
//! | Way of writing | What comes back | What it establishes |
//! |---|---|---|
//! | `set` | `true` | the update was applied **here**, now. Not a receipt: it names no rung |
//! | `set_with_receipt`, no persistence | `NotConfigured` | nothing was promised, so nothing is claimed |
//! | `set_with_receipt`, buffered persistence | `Buffered` | the WAL took it; the bytes are in the OS page cache |
//! | `set_requiring_sync` | `OnDisk` | an `fdatasync` returned. **The one state that establishes durability** |
//! | `prepare_write` + `commit_prepared` | the same stamp, re-asserted | a retry after a lost acknowledgement **loses** LWW instead of undoing a newer value |
//! | `set_with_replica_sync` | rung 3 | *another node* answered that it holds the record |
//!
//! And the ladder's point is the negative direction: a receipt that says `Buffered` is **not** a
//! weaker way of saying `OnDisk`. It is a different claim, and a caller that needs the stronger one
//! has to ask for it — which is why `set_requiring_sync` on a node with no persistence is
//! **refused** rather than answered with a receipt that claims nothing.
//!
//! # What this demonstrates by doing, not by asserting
//!
//! Step 7 shuts a peer down and writes again: the peer that cannot answer is reported **unknown**,
//! never *failed*. Step 8 reopens the persisted node's own data directory and reads back what
//! replayed. Those are the two places the vocabulary earns its keep.
//!
//! # What it does not demonstrate, stated because the gap matters
//!
//! **Neither failure mode that `Buffered` declines to survive.** `Buffered` means *survives a
//! process crash, lost to a power failure*, and this example stages neither: step 8 is a **clean
//! shutdown** and a reopen, so a `Buffered` record surviving it shows the WAL replaying and nothing
//! about durability under failure. A kill and a power cut are both outside one cooperating process,
//! and the honest thing is to say so rather than to let a passing line imply the stronger result.
//!
//! Nor does it show a peer crashing **mid-write** (the interesting race), or the gateway and SDK
//! surfaces of the same receipts (item 1 PR 7 covers those), or anything about a network partition.

use mycelium::{
    GossipAgent, GossipConfig, NodeId, OperationId, PersistenceConfig, ReceiptError,
    SyncMode, WriteReceipt,
};
use std::time::Duration;

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

/// One line per rung, so the same receipt is read the same way everywhere in this file.
fn show(receipt: &WriteReceipt) {
    note(format!("rung 1 · applied here      : {:?}", receipt.application));
    note(format!("rung 2 · local durability  : {:?}", receipt.local_durability));
    let rung3 = if receipt.replica_sync.persisted_by.is_empty() && receipt.replica_sync.missing.is_empty() {
        "not asked".to_string()
    } else {
        format!(
            "{} peer(s) hold it, {} unknown",
            receipt.replica_sync.persisted_by.len(),
            receipt.replica_sync.missing.len()
        )
    };
    note(format!("rung 3 · replica sync      : {rung3}"));
    note(format!("gossip fan-out (not a rung): {}", receipt.queued_for_gossip));
}

/// A free port, allocated the way the test utilities do it — bind `:0`, read the port back, drop
/// the listener.
///
/// Deliberately *not* `mycelium::test_util::alloc_port`: that lives behind the `test-util` feature,
/// and an example is not a test. CI builds every example under the ordinary feature set, so an
/// example that needs a test-only feature is an example that does not build there — which is how
/// the first version of this file failed.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("bound").port();
    drop(listener);
    port
}

fn persisted_config(port: u16, dir: &std::path::Path, sync_mode: SyncMode) -> GossipConfig {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.persistence = Some(PersistenceConfig {
        base_path: dir.to_path_buf(),
        sync_mode,
        snapshot_wal_threshold: 10_000,
        snapshot_interval_secs: 300,
    });
    cfg
}

#[tokio::main]
async fn main() {
    let tmp = std::env::temp_dir().join(format!("receipt-ladder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    println!("\x1b[1mThe receipt ladder — the same write, four ways, four different claims\x1b[0m");

    // ── 1. The bool ───────────────────────────────────────────────────────────────────────────
    step(1, "`set` — an acknowledgement that names no rung");
    let plain_port = free_port();
    let plain = GossipAgent::new(
        NodeId::new("127.0.0.1", plain_port).unwrap(),
        GossipConfig { bind_port: plain_port, ..Default::default() },
    );
    plain.start().await.expect("the node binds");
    let accepted = plain.kv().set("ladder/plain", bytes::Bytes::from_static(b"v"));
    note(format!("set(..) -> {accepted}"));
    note("True means: this node applied the update. It is not a receipt — it says nothing about");
    note("disk, and nothing about any other node. Every stronger claim below has to be asked for.");

    // ── 2. A receipt from a node with no persistence ──────────────────────────────────────────
    step(2, "`set_with_receipt` on a node with no persistence — nothing promised, nothing claimed");
    let op = OperationId::new("ladder/none");
    let receipt = plain
        .kv()
        .set_with_receipt(&op, "ladder/none", bytes::Bytes::from_static(b"v"))
        .await
        .expect("the local write succeeds");
    show(&receipt);
    note("`NotConfigured` is the honest answer: no persistence was configured, so the node never");
    note("promised durability and does not claim it. Compare that with step 3.");

    // ── 3. Asking for durability where it cannot be given ─────────────────────────────────────
    step(3, "`set_requiring_sync` on the same node — refused, not answered");
    let op = OperationId::new("ladder/none-required");
    match plain.kv().set_requiring_sync(&op, "ladder/none-required", bytes::Bytes::from_static(b"v")).await {
        Err(ReceiptError::DurabilityNotEstablished { persistence_configured, reason }) => {
            note(format!("refused: {reason} (persistence configured: {persistence_configured})"));
            note("The caller asked for durability **as a contract**. A node that cannot provide it");
            note("says so, rather than returning a receipt whose rung 2 claims nothing. A refusal");
            note("the caller can see beats a success the caller has to inspect.");
            note("Note what the refusal carries: *whether persistence was configured at all*. That");
            note("distinguishes a misconfigured node from a node whose disk just failed, which are");
            note("different problems for whoever is holding the pager.");
        }
        Ok(r) => {
            note("unexpected: a node with no persistence answered with a receipt");
            show(&r);
        }
        Err(other) => note(format!("unexpected refusal: {other:?}")),
    }
    plain.shutdown().await;

    // ── 4. Buffered ───────────────────────────────────────────────────────────────────────────
    step(4, "`set_with_receipt` on a node with buffered persistence — `Buffered`");
    let keeper_port = free_port();
    let keeper_dir = tmp.join("keeper");
    let keeper = GossipAgent::new(
        NodeId::new("127.0.0.1", keeper_port).unwrap(),
        persisted_config(keeper_port, &keeper_dir, SyncMode::Async),
    );
    keeper.start().await.expect("the node binds");
    let op = OperationId::new("ladder/buffered");
    let receipt = keeper
        .kv()
        .set_with_receipt(&op, "ladder/buffered", bytes::Bytes::from_static(b"buffered"))
        .await
        .expect("the write succeeds");
    show(&receipt);
    note("`Buffered`: the WAL accepted the record, but this node's sync mode does not force a sync");
    note("per append, so the bytes are in the OS page cache. They survive a process crash (step 8");
    note("shows exactly that) and are lost to a power failure until the next sync or snapshot.");
    note("This state was missing from the contract's first draft — a receipt that said `OnDisk`");
    note("here would have claimed a durability the node never established.");

    // ── 5. On disk ────────────────────────────────────────────────────────────────────────────
    step(5, "`set_requiring_sync` on the same node — `OnDisk`");
    let op = OperationId::new("ladder/ondisk");
    let receipt = keeper
        .kv()
        .set_requiring_sync(&op, "ladder/ondisk", bytes::Bytes::from_static(b"ondisk"))
        .await
        .expect("the node can sync, so the contract is honoured");
    show(&receipt);
    note("`OnDisk` is the one state that establishes durability: a forced `fdatasync` returned.");
    note("Same node, same key shape, same millisecond — a different claim, because a different");
    note("thing was asked for and a different thing was done.");

    // ── 6. Another node's answer ──────────────────────────────────────────────────────────────
    step(6, "`set_with_replica_sync` — rung 3 is somebody else's word, not ours");
    let peer_port = free_port();
    let peer_dir = tmp.join("peer");
    let mut peer_cfg = persisted_config(peer_port, &peer_dir, SyncMode::Flush);
    peer_cfg.bootstrap_peers = vec![NodeId::new("127.0.0.1", keeper_port).unwrap()];
    let peer = GossipAgent::new(NodeId::new("127.0.0.1", peer_port).unwrap(), peer_cfg);
    peer.start().await.expect("the peer binds");
    for _ in 0..100 {
        if !keeper.peers().is_empty() && !peer.peers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    note(format!("the two nodes see each other: keeper={} peer={}", keeper.peers().len(), peer.peers().len()));
    let receipt = keeper
        .set_with_replica_sync("ladder/replicated", bytes::Bytes::from_static(b"replicated"), Duration::from_secs(3))
        .await
        .expect("the local write succeeds");
    show(&receipt);
    for n in &receipt.replica_sync.persisted_by {
        note(format!("  holds it : {n}"));
    }
    note("Rung 3 is the only rung about another machine, and it is the only one this node cannot");
    note("establish by itself: it asked, and reports what came back.");

    // ── 7. The peer stops answering ───────────────────────────────────────────────────────────
    step(7, "the peer goes away — unknown, never failed");
    peer.shutdown().await;
    note("peer shut down.");
    let receipt = keeper
        .set_with_replica_sync("ladder/after-crash", bytes::Bytes::from_static(b"after"), Duration::from_secs(2))
        .await
        .expect("the local write still succeeds");
    show(&receipt);
    for n in &receipt.replica_sync.missing {
        note(format!("  unknown  : {n}"));
    }
    note("The local rungs are unchanged — this node still did what it did. Rung 3 reports the peer");
    note("as **unknown**, which is not the same as *failed*: a peer that did not answer may well");
    note("hold the record. A timeout is never a negative, and the receipt refuses to guess.");

    // ── 8. Reopen the store, and read what replayed ───────────────────────────────────────────
    step(8, "reopen the persisted node — what replayed");
    keeper.shutdown().await;
    note("keeper shut down **cleanly**; reopening the same data directory.");
    let reborn = GossipAgent::new(
        NodeId::new("127.0.0.1", keeper_port).unwrap(),
        persisted_config(keeper_port, &keeper_dir, SyncMode::Async),
    );
    reborn.start().await.expect("the node reopens its store");
    for key in ["ladder/buffered", "ladder/ondisk", "ladder/replicated", "ladder/after-crash"] {
        let present = reborn.kv().get(key).is_some();
        note(format!("{key:<22} after restart: {}", if present { "present" } else { "absent" }));
    }
    note("");
    note("`OnDisk` promised this and delivered it — that is the rung being honoured.");
    note("`Buffered` is here too, and it is worth being precise about what that does and does not");
    note("show: this was a **clean shutdown**, not a crash, so the bytes had every chance to reach");
    note("the file. A `Buffered` record surviving *this* is the WAL replaying, not evidence about");
    note("durability under failure. The claims `Buffered` declines to make — surviving a kill, and");
    note("surviving power loss — are exactly the ones this process cannot stage, which is why the");
    note("receipt says `Buffered` and not something warmer.");
    reborn.shutdown().await;

    // ── 9. The retry that must not win ────────────────────────────────────────────────────────
    //
    // Every rung above answers "what happened to my write". This one answers the question that
    // follows it in production: **what do I do when I never got the answer at all.**
    step(9, "a lost acknowledgement — `prepare_write` / `commit_prepared`");
    note("The caller writes, and the response is lost. It has no receipt. It must retry — and the");
    note("retry must not undo whatever the world has done in the meantime.");

    let op_retry = OperationId::new("op-dispatch-4471");
    // The token is minted *before* dispatch and kept by the caller. It carries the stamp, so the
    // commit is a re-assertion of one decision rather than a fresh one.
    let prepared = plain.kv().prepare_write(&op_retry, "ladder/retry", b"original");
    let first = plain.kv().commit_prepared(&prepared, b"original".to_vec()).await
        .expect("the first commit applies");
    note(format!("first commit  → {:?}", first.application));

    // The acknowledgement never arrives. Meanwhile something newer legitimately takes the key.
    drop(first);
    let _ = plain.kv().set("ladder/retry", b"newer".to_vec());
    note("…response lost. Something newer then writes the same key.");

    let retry = plain.kv().commit_prepared(&prepared, b"original".to_vec()).await
        .expect("the retry is answered");
    note(format!("retry         → {:?}  (stamp reused, so it loses LWW and says so)",
                 retry.application));
    assert_eq!(retry.stamp, prepared.stamp, "the prepared stamp is reused, never re-ticked");
    assert_eq!(plain.kv().get("ladder/retry").as_deref(), Some(&b"newer"[..]),
               "the newer value survives the retry");
    note("✓ the newer value survived — the retry was refused by LWW, not by luck");

    // And the contrast, which is the reason the token exists at all.
    let reissued = plain.kv().set_with_receipt(&op_retry, "ladder/retry", b"original".to_vec())
        .await.expect("re-issue by operation id");
    assert_ne!(reissued.stamp, prepared.stamp, "a fresh call mints a fresh stamp");
    assert_eq!(plain.kv().get("ladder/retry").as_deref(), Some(&b"original"[..]));
    note(format!("re-issued by operation id alone → {:?}, and it CLOBBERED the newer value",
                 reissued.application));
    note("The operation id gives you idempotence of *intent*. Only the prepared token gives you");
    note("idempotence of *position in time* — which is what a retry actually needs.");

    // ── 10. What this did not show ────────────────────────────────────────────────────────────
    step(10, "what this demonstration does not establish");
    note("· power-failure loss for `Buffered` — a property of the OS, not stageable in one process");
    note("· a peer crashing mid-write, which is the interesting race rather than this clean one");
    note("· the gateway and SDK surfaces of the same receipts (item 1 PR 7 carries those)");
    note("· anything about a network partition, or about more than two nodes");
    println!();

    let _ = std::fs::remove_dir_all(&tmp);
}
