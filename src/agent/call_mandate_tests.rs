//! Identity-and-lifecycle I4, the provider half at a live node: `assess_call`'s read of this node's own KV under its
//! hold, the feeder that offers a held window's checkpoint when it changes, and what a node holds for calls it did
//! and did not admit. The node is its own gateway here — it mints with its node key (`call_mint`, test-only; the
//! gateway's mint is the next PR) — so the mandate, its checkpoints and the member key that verifies them are the
//! real ones. The authority-only halves are in `gateway_authority::call_tests`.

use super::action_evaluator::MandateState;
use super::gateway_authority::call_hooks::{self, Stage};
use super::gateway_authority::{call_checkpoint_key, call_mint, ExecutionAuthority, PresentedCallMandate};
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
        self.1
            .assess_call(&self.gateway(), OP, &self.resource(), &args(), c, now, &members, |key| mycelium_core::ops::kv_get(&ctx, key))
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

impl Node {
    /// KV reads made for `call:` checkpoints, rescans of the held windows, and written keys the feeder has taken.
    fn reads(&self) -> u64 {
        self.1.call_kv_reads.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn rescans(&self) -> u64 {
        self.1.call_rescans.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Wait until the feeder has taken `written` keys from its queue: it has caught up with every write so far.
    async fn caught_up(&self, written: u64) {
        let seen = || self.1.call_keys_seen.load(std::sync::atomic::Ordering::Relaxed);
        assert!(until(|| seen() >= written, Duration::from_secs(20)).await, "the feeder took {} of {written} written keys", seen());
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

/// **Exit 9: reclamation with the feeder running, past one hold's worth.** 2 500 calls' checkpoints are in this
/// node's KV — more than `RECLAIM_PER_HOLD`. While it has admitted none of them it holds none and reads none: each
/// write reached the feeder, which found no window for it. Once it admits them it holds one checkpoint, one
/// revoked-set entry and one window per call; a revoked one presented again inside its window stays refused; one
/// sweep past the last window the view is back to zero, and stays there when a late write for a forgotten call
/// reaches the feeder. Seen failing first: with `offer_checkpoint` keeping a `call:` checkpoint for a scope it
/// holds no window for, the late offer is `Accepted` and held for good; with one reclaiming hold per sweep, the
/// sweep past the last window leaves 1 476 held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_holds_only_the_calls_it_admitted_and_only_for_their_windows() {
    const N: usize = 2_500;
    const { assert!(N > super::gateway_authority::RECLAIM_PER_HOLD * 2) };
    let n = node(120_000).await;
    let calls: Vec<PresentedCallMandate> = (0..N).map(|i| n.mint(&format!("r{i}"), 30_000)).collect();
    for (i, c) in calls.iter().enumerate() {
        n.write(&format!("r{i}"), &c.checkpoint);
    }
    n.caught_up(N as u64).await;
    assert_eq!((n.reads(), n.rescans()), (0, 0), "no window, so no key is read");
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "a node that admitted none of the calls holds none of them");

    let now = n.now();
    for c in &calls {
        assert_eq!(n.assess(c, now), Some(MandateState::Established));
    }
    assert_eq!(n.1.view_sizes(), (N, N, N), "per admitted call: one checkpoint, one revoked-set entry, one window");
    // Cancel one; it stays refused inside its window, with its work long finished.
    n.write("r7", &n.checkpoint("r7", 2, &["r7"]));
    n.caught_up(N as u64 + 1).await;
    assert!(matches!(n.assess(&calls[7], n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
    assert_eq!(n.1.view_sizes(), (N, N, N));

    // Past the last window: the background sweep runs on the node's own clock, so sweep at that instant by hand.
    let last = calls.iter().map(|c| c.grant.mandate.valid_until_ms).max().unwrap();
    n.1.sweep(last + 1);
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "one sweep past the last window, in as many holds as it takes, nothing is held");
    // A late write for a forgotten call reaches the feeder, and nothing comes back.
    let reads = n.reads();
    n.write("r8", &n.checkpoint("r8", 2, &["r8"]));
    n.caught_up(N as u64 + 2).await;
    assert_eq!(n.reads(), reads, "a forgotten call's key is not read");
    assert_eq!(n.1.offer_checkpoint(&n.checkpoint("r8", 3, &["r8"]), n.now(), &super::gateway_member_keys(&n.0.task_ctx)), crate::mandate::authority::CheckpointOffer::NotHeld);
    assert_eq!(n.1.view_sizes(), (0, 0, 0), "and it stays at zero");
    n.0.shutdown().await;
}

/// **Exit 16: the feeder's work follows the writes that concern this node** — not the `call:` checkpoints its KV
/// holds, and not the windows it holds. Ten calls admitted here. 10² and then 10⁴ other calls' checkpoints are
/// written under the family: the feeder takes every one of those keys and **reads none** (the reads are counted
/// where the KV is read, not reported by the feeder). One of this node's ten is then cancelled: one read, of that
/// key. Last, the fallback: 5 000 keys written while the feeder cannot run overflow its queue of 4 096, and it
/// re-reads the windows it holds — ten reads, once. Counted, not timed. Seen failing first: with the feeder reading
/// every held window on each wake (this PR's first form), the 10² and 10⁴ writes cost ten reads a wake; with the
/// held-window check taken out of the feeder's read, each of them is read.
#[tokio::test]
async fn the_call_feeder_reads_only_the_key_that_changed_and_only_for_a_window_it_holds() {
    let n = node(120_000).await;
    let mine: Vec<PresentedCallMandate> = (0..10).map(|i| n.mint(&format!("m{i}"), 60_000)).collect();
    let now = n.now();
    let mut written = 0u64;
    for (i, c) in mine.iter().enumerate() {
        n.write(&format!("m{i}"), &c.checkpoint);
        written += 1;
        assert_eq!(n.assess(c, now), Some(MandateState::Established));
    }
    n.caught_up(written).await;
    let base = n.reads();
    let other = serde_json::to_vec(&n.checkpoint("other", 1, &[])).unwrap();
    let mut others = 0usize;
    for total in [100usize, 10_000] {
        while others < total {
            // Other gateways' calls, in batches under the feeder's queue, so it is never behind by more than one.
            for _ in 0..500.min(total - others) {
                super::state_machine::write_owned_kv(&n.0.task_ctx, &format!("call/node:10.0.0.1:1/x{others}/checkpoint"), other.clone());
                others += 1;
                written += 1;
            }
            n.caught_up(written).await;
        }
        assert!(crate::store::scan_kv_prefix(&n.0.task_ctx.kv_state, "call/").len() >= total + 10);
        assert_eq!((n.reads() - base, n.rescans()), (0, 0), "with {total} other checkpoints written, none is read");
    }
    assert_eq!(n.1.view_sizes(), (10, 10, 10), "and none of the others is held");
    // The one that changes is read, once: a cancel of m3 reaches the view through the feeder alone.
    n.write("m3", &n.checkpoint("m3", 2, &["m3"]));
    written += 1;
    n.caught_up(written).await;
    assert_eq!(n.reads() - base, 1, "one read, of the key that changed");
    assert!(matches!(n.assess(&mine[3], n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
    // The fallback. This runtime has one thread and nothing here awaits, so the feeder cannot run until the loop ends.
    let before = n.reads();
    for i in 0..5_000 {
        super::state_machine::write_owned_kv(&n.0.task_ctx, &format!("call/node:10.0.0.1:1/y{i}/checkpoint"), other.clone());
    }
    assert!(until(|| n.rescans() == 1, Duration::from_secs(20)).await, "the queue overflowed and the feeder re-read its windows");
    assert_eq!(n.reads() - before, 10, "one read per held window, once");
    n.0.shutdown().await;
}

/// **A copy of a call's checkpoint that its gateway did not write does not move the reader, and is counted** (the
/// rule of 2026-11-12: a security-bearing record in the gossip KV is signed by its owner and verified at read; the
/// maintainer's decision of the same day on #775's H1: a failing copy is *absent, not refused*, with detectors).
/// Any member can write `call/{gateway}/{term}/checkpoint`; here each copy is written straight into this node's KV,
/// as a member's gossip would land it, over a running call and before an admission:
///
/// - a **forged** revoking checkpoint — the gateway's name, the call's scope, another key's signature;
/// - a **forged** copy **at the sequence this node already holds** — which a sequence check alone would skip;
/// - a **misplaced** one — the gateway's own signature, for another call's scope;
/// - bytes that are **not a checkpoint**;
/// - a **tombstone** where this node had read a verified copy.
///
/// None cancels the running call or refuses the admission, and each moves its counter (`refused`, `missing`). The
/// gateway's own revoking checkpoint then cancels the call. After it, the gateway's genuine **first checkpoint
/// written back over the revoking one** is counted `stale` and a tombstone `missing`, and neither un-cancels the
/// call here.
///
/// **The residual, pinned at the end:** a replay is counted `stale` only at a node that already holds the
/// revocation. A provider the replay reaches before the revocation counts nothing and keeps running the call: one
/// admitting later (the `f4` step), one restarted, or one already running the call (the `f5` step). That is bounded by the mandate's window and is what the cancel-forward
/// (PR 3) and the gateway re-asserting its key (PR 2) are for.
///
/// Seen failing first: the key-path check, the count, and the signature check each taken out; the sequence
/// short-circuit put back before the offer (the forged copy at the held sequence is not counted); the `stale` and
/// the `missing` arms each taken out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_misplaced_replayed_or_tombstoned_copy_of_a_calls_checkpoint_does_not_move_the_reader() {
    let n = node(2_000).await;
    let forger = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
    let forged = |term: &str, seq: u64| call_mint::checkpoint(&forger, &n.gateway(), &format!("call:{term}"), seq, n.now(), &[term]);
    let (refused, stale, missing) = (|| n.1.call_checkpoints_refused(), || n.1.call_checkpoints_stale(), || n.1.call_checkpoints_missing());
    let within = Duration::from_secs(30);
    let tombstone = |term: &str| assert!(mycelium_core::ops::kv_delete(&n.0.task_ctx, Arc::from(call_checkpoint_key(&n.gateway(), term))));

    // A running call, its first checkpoint read from the KV at admission.
    let c = n.mint("f1", 60_000);
    n.write("f1", &c.checkpoint);
    assert_eq!(n.assess(&c, n.now()), Some(MandateState::Established));
    let work = n.begin(&c);
    assert_eq!((refused(), stale(), missing()), (0, 0, 0));

    // Forged, at a new sequence: verified under the gateway's key, which did not sign it.
    // Each count is compared with the count before the write, not with an exact number: the store may report one
    // write to the feeder more than once, and a bad copy is counted at each read of it.
    n.write("f1", &forged("f1", 2));
    assert!(until(|| refused() >= 1, within).await, "the forged copy is counted");
    // Forged, at the sequence this node holds.
    let before = refused();
    n.write("f1", &forged("f1", 1));
    assert!(until(|| refused() > before, within).await, "a forged copy at the held sequence is verified and counted, not skipped as old");
    // Misplaced: genuinely the gateway's, for another call.
    let before = refused();
    n.write("f1", &n.checkpoint("f2", 2, &["f1", "f2"]));
    assert!(until(|| refused() > before, within).await, "the misplaced copy is counted");
    // Not a checkpoint at all.
    let before = refused();
    super::state_machine::write_owned_kv(&n.0.task_ctx, &call_checkpoint_key(&n.gateway(), "f1"), b"not a checkpoint".to_vec());
    assert!(until(|| refused() > before, within).await, "the unparsed copy is counted");
    // A tombstone where a verified copy was read: counted at each read that finds it — the admission's own here,
    // and the feeder's if the store reports the delete to it.
    tombstone("f1");
    assert_eq!(n.assess(&c, n.now()), Some(MandateState::Established), "and the call is still admitted, on its carried checkpoint");
    assert!(missing() >= 1, "the tombstone is counted");
    assert!(!work.is_cancelled(), "none of them cancelled the running call");
    assert_eq!((n.1.view_sizes(), stale()), ((1, 1, 1), 0), "nothing more is held");

    // At admission: a forged copy already in the KV when a new call arrives.
    let c3 = n.mint("f3", 60_000);
    n.write("f3", &forged("f3", 2));
    let before = refused();
    assert_eq!(n.assess(&c3, n.now()), Some(MandateState::Established), "a forged revocation refuses no admission");
    assert!(refused() > before, "counted at the admission's own read");

    // The gateway's own revocation still moves it.
    n.write("f1", &n.checkpoint("f1", 2, &["f1"]));
    assert!(until(|| work.is_cancelled(), within).await, "the gateway's own revoking checkpoint cancels the call");
    drop(work);
    // The gateway's genuine first checkpoint, written back over the revoking one by another member.
    n.write("f1", &c.checkpoint);
    assert!(until(|| stale() >= 1, within).await, "the older copy over a newer one this node holds is counted");
    assert!(matches!(n.assess(&c, n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))), "and a revocation seen stays seen");
    // And a tombstone over it.
    let before = missing();
    tombstone("f1");
    assert!(matches!(n.assess(&c, n.now()), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
    assert!(missing() > before);

    // The residual: this node has never read f4's revocation when the first checkpoint is written back over it.
    let c4 = n.mint("f4", 60_000);
    n.write("f4", &c4.checkpoint);
    n.write("f4", &n.checkpoint("f4", 2, &["f4"]));
    n.write("f4", &c4.checkpoint);
    let counted = (refused(), stale());
    assert_eq!(n.assess(&c4, n.now()), Some(MandateState::Established), "stated: a first read of a replayed older copy is established");
    assert_eq!((refused(), stale()), counted, "stated: and nothing here can count it");

    // The residual at a provider **already running** the call (#775's confirm round): it read the first checkpoint
    // at admission; a member writes that same checkpoint back with a later stamp than the gateway's revocation, so
    // under last-writer-wins the revocation never reaches this node and all it sees is the bytes it has verified.
    let c5 = n.mint("f5", 60_000);
    n.write("f5", &c5.checkpoint);
    assert_eq!(n.assess(&c5, n.now()), Some(MandateState::Established));
    let running = n.begin(&c5);
    let (counted, read) = ((refused(), stale()), n.reads());
    n.write("f5", &c5.checkpoint);
    assert!(until(|| n.reads() > read, within).await, "the feeder read the rewritten key");
    tokio::time::sleep(Duration::from_millis(300)).await; // three sweep intervals: nothing stops it
    assert!(!running.is_cancelled(), "stated: the call keeps running");
    assert_eq!((refused(), stale()), counted, "stated: and nothing is counted — `stale` needs a node that holds the revocation");
    drop(running);
    n.0.shutdown().await;
}
