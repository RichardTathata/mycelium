//! Identity-and-lifecycle I4, the provider half at a live node: `assess_call`'s read of this node's own KV under its
//! hold, the feeder that offers a held window's checkpoint when it changes, and what a node holds for calls it did
//! and did not admit. The node is its own gateway here — it mints with its node key (`call_mint`, test-only; the
//! gateway's mint is the next PR) — so the mandate, its checkpoints and the member key that verifies them are the
//! real ones. The authority-only halves are in `gateway_authority::call_tests`.

use super::action_evaluator::MandateState;
use super::gateway_authority::call_hooks::{self, Stage};
use super::gateway_authority::{call_checkpoint_key, call_mint, feed_call_checkpoints, local_call_checkpoint, ExecutionAuthority, PresentedCallMandate};
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::mandate::authority::{AuthorizedWork, ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, SignedRevocationCheckpoint};
use crate::mandate::grant::{EntitlementTable, GrantVerifier};
use crate::mandate::{MandateRefusal, ResourceAuthority};
use crate::{GossipAgent, GossipConfig, NodeId};
use std::sync::Arc;
use std::time::Duration;

const OP: &str = "skill.invoke";

/// A started node with a TLS identity and an execution authority, and its directory, removed on drop.
struct Node(Arc<GossipAgent>, Arc<ExecutionAuthority>, std::path::PathBuf);

impl Drop for Node {
    fn drop(&mut self) {
        call_hooks::HOOKS.pin().remove(&call_hooks::key(&self.1));
        // Guarded: only this helper's own directory under the temp dir.
        let ours = self.2.starts_with(std::env::temp_dir()) && self.2.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("i4-call-"));
        if ours {
            let _ = std::fs::remove_dir_all(&self.2);
        }
    }
}

/// `freshness_ms` sets the sweep: a twentieth of it, at least 100 ms.
async fn node(freshness_ms: u64) -> Node {
    let port = crate::test_util::alloc_port();
    let dir = std::env::temp_dir().join(format!("i4-call-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("unused", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 100 },
        FreshnessPolicy { freshness_ms, interval_ms: freshness_ms / 4, delivery_ms: freshness_ms / 12 },
    )
    .unwrap();
    // No entitlement at all: a `call:` mandate needs none (D-6).
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(EntitlementTable::new()), TrustedExternalIssuers::new()));
    a.with_execution_authority(Arc::clone(&authority));
    a.start().await.expect("starts");
    Node(a, authority, dir)
}

impl Node {
    fn gateway(&self) -> String {
        super::gateway_caller::node_principal(self.0.node_id())
    }
    fn resource(&self) -> String {
        format!("skill:depot/dispatch@{}", self.0.node_id())
    }
    fn key(&self) -> Arc<ed25519_dalek::SigningKey> {
        self.0.task_ctx.tls.get().unwrap().signing_key()
    }
    fn now(&self) -> u64 {
        self.0.task_ctx.hlc.decision_now_ms()
    }
    /// A call minted now by this node as its own gateway, valid for `ttl_ms`.
    fn mint(&self, term: &str, ttl_ms: u64) -> PresentedCallMandate {
        let now = self.now();
        call_mint::mint(&self.key(), &self.gateway(), term, OP, &self.resource(), &args(), now, now + ttl_ms)
    }
    fn checkpoint(&self, term: &str, seq: u64, revoked: &[&str]) -> SignedRevocationCheckpoint {
        call_mint::checkpoint(&self.key(), &self.gateway(), &format!("call:{term}"), seq, self.now(), revoked)
    }
    /// Write `term`'s checkpoint to this node's KV, as the issuing gateway does.
    fn write(&self, term: &str, cp: &SignedRevocationCheckpoint) {
        super::state_machine::write_owned_kv(&self.0.task_ctx, &call_checkpoint_key(&self.gateway(), term), serde_json::to_vec(cp).unwrap());
    }
    /// `assess_call` as `admit_after` calls it: the local copy read from this node's KV.
    fn assess(&self, c: &PresentedCallMandate, now: u64) -> Option<MandateState> {
        let ctx = Arc::clone(&self.0.task_ctx);
        let members = super::gateway_member_keys(&ctx);
        let authority = Arc::clone(&self.1);
        self.1
            .assess_call(&self.gateway(), OP, &self.resource(), &args(), c, now, &members, |by, scope| local_call_checkpoint(&ctx, &authority, by, scope))
            .binding
            .map(|b| b.state)
    }
    fn begin(&self, c: &PresentedCallMandate) -> super::gateway_authority::WorkGuard {
        let m = &c.grant.mandate;
        let ctx = Arc::clone(&self.0.task_ctx);
        self.1.begin(
            AuthorizedWork::new(super::gateway_authority::mandate_operation(OP, &self.resource()), Some(m.clone()), m.valid_until_ms),
            Arc::new(move || ctx.hlc.decision_now_ms()),
        )
    }
}

fn args() -> serde_json::Value {
    serde_json::json!({ "text": "restock aisle 4" })
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

/// **N2 and exit 5: a revocation written while the call is being admitted is not missed.** The feeder and the sweep
/// run. Case 1: the revoking checkpoint lands in this node's KV after the window is inserted and before the local
/// read — the read sees it and the call is refused `Revoked` (so nothing is registered; stronger than the row's
/// "cancelled within one sweep"). Case 2: it lands just after the read — the call is established on its carried
/// checkpoint and registered, the write wakes the feeder, which finds the window and offers it, and the sweep cancels
/// the work within its interval. Re-presented, both read `Refused(Revoked)`.
///
/// Seen failing first, in the spike's order — the local copy read before the hold, the write landing between that
/// read and the window's insert: case 1's call is `Established`, the feeder's wake finds no window, and the work is
/// never cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revocation_written_during_admission_is_read_or_fed_never_missed() {
    let n = Arc::new(node(2_000).await);
    assert_eq!(n.1.sweep_interval_ms(), 100);
    let fire_at = |stage: Stage, term: &'static str| {
        let (n2, fired) = (Arc::clone(&n), std::sync::atomic::AtomicBool::new(false));
        call_hooks::HOOKS.pin().insert(
            call_hooks::key(&n.1),
            Arc::new(move |at: Stage| {
                if at == stage && !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    n2.write(term, &n2.checkpoint(term, 2, &[term]));
                }
            }) as call_hooks::Hook,
        );
    };

    // Case 1: between the window's insert and the local read.
    let c1 = n.mint("n2a", 60_000);
    fire_at(Stage::BeforeLocalRead, "n2a");
    let state = n.assess(&c1, n.now());
    assert!(matches!(state, Some(MandateState::Refused(MandateRefusal::Revoked { ref term })) if term == "n2a"), "case 1: {state:?}");

    // Case 2: just after the read.
    let c2 = n.mint("n2b", 60_000);
    fire_at(Stage::AfterLocalRead, "n2b");
    assert_eq!(n.assess(&c2, n.now()), Some(MandateState::Established), "case 2 is admitted on its carried checkpoint");
    call_hooks::HOOKS.pin().remove(&call_hooks::key(&n.1));
    let work = n.begin(&c2);
    let asked = std::time::Instant::now();
    assert!(until(|| work.is_cancelled(), Duration::from_secs(5)).await, "case 2: the feeder offered the revocation and the sweep cancelled the work");
    let took = asked.elapsed();
    // One sweep interval (100 ms), with room for a loaded host; the point is that it is the sweep's, not the window's.
    assert!(took < Duration::from_secs(2), "cancelled within the sweep's bound, not at the 60 s window: {took:?}");
    drop(work);
    assert_eq!(n.1.named_stop_records().last().and_then(|s| s.term.clone()).as_deref(), Some("n2b"));
    for (c, term) in [(&c1, "n2a"), (&c2, "n2b")] {
        let again = n.assess(c, n.now());
        assert!(matches!(again, Some(MandateState::Refused(MandateRefusal::Revoked { term: ref t })) if t == term), "re-presented: {again:?}");
    }
    n.0.shutdown().await;
}

/// **Exit 9: reclamation with the feeder running.** 10³ calls' checkpoints are in this node's KV. While it has
/// admitted none of them it holds none (view sizes zero) — each write woke the feeder, which had no window to read
/// a key for. Once it admits them it holds one checkpoint, one revoked-set entry and one window per call, the
/// counted bound; a revoked one presented again inside its window stays refused; one sweep past the last window
/// the view is back to zero, and stays there when the feeder is woken again by a late write for a forgotten call.
/// Seen failing first: with `offer_checkpoint` keeping a `call:` checkpoint for a scope it holds no window for, the
/// late offer is `Accepted` and held for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_holds_only_the_calls_it_admitted_and_only_for_their_windows() {
    const N: usize = 1_000;
    let n = node(120_000).await;
    let calls: Vec<PresentedCallMandate> = (0..N).map(|i| n.mint(&format!("r{i}"), 30_000)).collect();
    for (i, c) in calls.iter().enumerate() {
        n.write(&format!("r{i}"), &c.checkpoint);
    }
    // The writes woke the feeder; give it a moment to have run on them.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(feed_call_checkpoints(&n.0.task_ctx, &n.1), 0, "no window, so no key is read");
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "a node that admitted none of the calls holds none of them");

    let now = n.now();
    for c in &calls {
        assert_eq!(n.assess(c, now), Some(MandateState::Established));
    }
    assert_eq!(n.1.view_sizes(), (N, N, N), "per admitted call: one checkpoint, one revoked-set entry, one window");
    // Cancel one; it stays refused inside its window, with its work long finished.
    n.write("r7", &n.checkpoint("r7", 2, &["r7"]));
    assert!(until(|| matches!(n.assess(&calls[7], n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))), Duration::from_secs(5)).await);
    assert_eq!(n.1.view_sizes(), (N, N, N));

    // Past the last window: the background sweep runs on the node's own clock, so sweep at that instant by hand.
    let last = calls.iter().map(|c| c.grant.mandate.valid_until_ms).max().unwrap();
    n.1.sweep(last + 1);
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "one sweep past the last window (N is under the per-sweep cap), nothing is held");
    // A late write for a forgotten call wakes the feeder, and nothing comes back.
    n.write("r8", &n.checkpoint("r8", 2, &["r8"]));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(feed_call_checkpoints(&n.0.task_ctx, &n.1), 0);
    assert_eq!(n.1.offer_checkpoint(&n.checkpoint("r8", 3, &["r8"]), n.now(), &super::gateway_member_keys(&n.0.task_ctx)), crate::mandate::authority::CheckpointOffer::NotHeld);
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "and it stays at zero when the feeder is woken again");
    n.0.shutdown().await;
}

/// **Exit 16: the feeder's cost follows the windows this node holds, not the `call:` checkpoints its KV holds.**
/// Ten calls admitted here; the KV holds 10² and then 10⁴ other calls' checkpoints. Each feed reads exactly ten keys
/// either way (the count `feed_call_checkpoints` returns), and offers the one that changed. Counted, not timed: a
/// scan of the family would read 10² and 10⁴. Seen failing first: with the feeder scanning `call/` as the grant
/// feeder scans `grant/`, the count is the family's size.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_call_feeder_reads_one_key_per_held_window_whatever_the_kv_holds() {
    let n = node(120_000).await;
    let mine: Vec<PresentedCallMandate> = (0..10).map(|i| n.mint(&format!("m{i}"), 60_000)).collect();
    let now = n.now();
    for (i, c) in mine.iter().enumerate() {
        n.write(&format!("m{i}"), &c.checkpoint);
        assert_eq!(n.assess(c, now), Some(MandateState::Established));
    }
    let other = n.checkpoint("other", 1, &[]);
    let bytes = serde_json::to_vec(&other).unwrap();
    let mut written = 0;
    for total in [100usize, 10_000] {
        while written < total {
            // Other gateways' calls: keys under the family that are not this node's windows.
            super::state_machine::write_owned_kv(&n.0.task_ctx, &format!("call/node:10.0.0.1:1/x{written}/checkpoint"), bytes.clone());
            written += 1;
        }
        assert!(crate::store::scan_kv_prefix(&n.0.task_ctx.kv_state, "call/").len() >= total + 10);
        assert_eq!(feed_call_checkpoints(&n.0.task_ctx, &n.1), 10, "with {total} other checkpoints in the KV");
    }
    assert_eq!(n.1.view_sizes(), (10, 10, 10), "and none of the others is held");
    // The one that changes is offered: a cancel of m3 reaches the view through the feeder alone.
    n.write("m3", &n.checkpoint("m3", 2, &["m3"]));
    assert!(until(|| matches!(n.assess(&mine[3], n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))), Duration::from_secs(5)).await);
    n.0.shutdown().await;
}

/// **A copy of a call's checkpoint that its gateway did not write does not move the reader** (the rule of
/// 2026-11-12: a security-bearing record in the gossip KV is signed by its owner and verified at read). Any member
/// can write `call/{gateway}/{term}/checkpoint`; here each bad copy is written straight into this node's KV, as a
/// member's gossip would land it, over a running call and before an admission:
///
/// - a **forged** revoking checkpoint — the gateway's name, the call's scope, another key's signature;
/// - a **misplaced** one — the gateway's own signature, for another call's scope;
/// - bytes that are **not a checkpoint**.
///
/// None cancels the running call, none refuses the admission, each is counted, and nothing more is held. The
/// gateway's own revoking checkpoint then cancels the call; and once that is seen, a **tombstone** over the key does
/// not un-cancel it. Seen failing first: with the key-path check taken out of `local_call_checkpoint`, the misplaced
/// copy is not counted; with the count taken out, none is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_misplaced_or_tombstoned_copy_of_a_calls_checkpoint_does_not_move_the_reader() {
    let n = node(2_000).await;
    let forger = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
    let forged = |term: &str| call_mint::checkpoint(&forger, &n.gateway(), &format!("call:{term}"), 2, n.now(), &[term]);
    let refused = || n.1.call_checkpoints_refused();
    let settle = || tokio::time::sleep(Duration::from_millis(400));

    // A running call.
    let c = n.mint("f1", 60_000);
    n.write("f1", &c.checkpoint);
    assert_eq!(n.assess(&c, n.now()), Some(MandateState::Established));
    let work = n.begin(&c);
    assert_eq!(refused(), 0);

    // Forged: verified under the gateway's key, which did not sign it.
    n.write("f1", &forged("f1"));
    settle().await;
    assert!(refused() >= 1, "the forged copy is counted");
    assert!(!work.is_cancelled(), "a forged revocation cancels nothing");
    // Misplaced: genuinely the gateway's, for another call.
    let before = refused();
    n.write("f1", &n.checkpoint("f2", 2, &["f1", "f2"]));
    settle().await;
    assert!(refused() > before, "the misplaced copy is counted");
    assert!(!work.is_cancelled(), "another call's checkpoint at this key cancels nothing");
    // Not a checkpoint at all.
    let before = refused();
    super::state_machine::write_owned_kv(&n.0.task_ctx, &call_checkpoint_key(&n.gateway(), "f1"), b"not a checkpoint".to_vec());
    settle().await;
    assert!(refused() > before, "the unparsed copy is counted");
    assert!(!work.is_cancelled());
    assert_eq!(n.assess(&c, n.now()), Some(MandateState::Established), "and the call is still admitted, on its carried checkpoint");
    assert_eq!(n.1.view_sizes(), (1, 1, 1), "nothing more is held");

    // At admission: a forged copy already in the KV when a new call arrives.
    let c3 = n.mint("f3", 60_000);
    n.write("f3", &forged("f3"));
    let before = refused();
    assert_eq!(n.assess(&c3, n.now()), Some(MandateState::Established), "a forged revocation refuses no admission");
    assert!(refused() > before, "counted at the admission's own read");

    // The gateway's own revocation still moves it.
    n.write("f1", &n.checkpoint("f1", 2, &["f1"]));
    assert!(until(|| work.is_cancelled(), Duration::from_secs(5)).await, "the gateway's own revoking checkpoint cancels the call");
    drop(work);
    // And once seen, a tombstone over the key does not un-cancel it.
    assert!(mycelium_core::ops::kv_delete(&n.0.task_ctx, std::sync::Arc::from(call_checkpoint_key(&n.gateway(), "f1"))));
    settle().await;
    assert!(mycelium_core::ops::kv_get(&n.0.task_ctx, &call_checkpoint_key(&n.gateway(), "f1")).is_none());
    assert!(matches!(n.assess(&c, n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))), "a revocation seen stays seen");
    n.0.shutdown().await;
}
