//! **B1, PR 2: the cancel-forward and its reply on the control lane** (`docs/design/control-lane.md` §5, the tests it
//! assigns to PR 2: T1, T1r, T4b, T4c, T5, T5b, T8, T9; T1b is the mints test in `call_cancel_tests`). Two nodes from
//! `call_mint_tests`' harness: `g` the gateway, `p` the provider. Each test names the plant it was seen failing against.
//! Assertions are structural — what stopped while what was held, what was served, what was counted — never a duration.

use super::call_cancel::{test_hooks, CANCEL_RPC_KIND};
use super::call_mint_tests::{node, pair, until, Opts};
use crate::{CancelClass, NodeId};
use bytes::Bytes;
use mycelium_core::writer::{WriteHold, TEST_WRITE_HOLD};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// A hold in `TEST_WRITE_HOLD`, released and removed when dropped — a failing test leaves no hold behind.
struct Hold(String, Arc<WriteHold>);

impl Hold {
    /// Hold the writer `key` names: a peer's id for its data writer, `control/{peer}` for its lane.
    fn new(key: String) -> Self {
        let h = WriteHold::new();
        TEST_WRITE_HOLD.pin().insert(key.clone(), Arc::clone(&h));
        Self(key, h)
    }
    fn reached(&self) -> u64 {
        self.1.reached.load(Ordering::SeqCst)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(h) = TEST_WRITE_HOLD.pin().remove(&self.0) {
            h.release();
        }
    }
}

/// Hold `g`'s data writer to `to` in a write: a KV write of `g` goes out and the writer waits with it.
async fn hold_data_writer(g: &super::call_mint_tests::N, to: &NodeId, tag: &str) -> Hold {
    let hold = Hold::new(to.to_string());
    let i = std::sync::atomic::AtomicU64::new(0);
    assert!(
        until(|| { let n = i.fetch_add(1, Ordering::Relaxed); let _ = g.a.kv().set(format!("b1/{tag}/{n}"), b"x".to_vec()); hold.reached() >= 1 }, Duration::from_secs(10)).await,
        "the gateway's data writer to the provider reached its hold",
    );
    hold
}

/// A running client call through `g` to `p`, and its term.
async fn running_call(g: &Arc<super::call_mint_tests::N>, p: &super::call_mint_tests::N) -> (tokio::task::JoinHandle<serde_json::Value>, String) {
    let g2 = Arc::clone(g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    (answered, term)
}

/// **T1: a cancel-forward reaches the provider while the gateway's data writer is held.** The call runs at `p`; `g`'s
/// data writer to `p` is held in a write, so neither the shared copy of the forward nor the KV revocation can leave.
/// The provider stops the call **while the writer is still held**, and the receipt is `Stopped`. Seen failing first
/// with no lane copy (the forward sent by `rpc_call_ctx`, the shared path alone): the call ran on while held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_forward_reaches_the_provider_while_the_gateways_data_writer_is_held() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let (answered, term) = running_call(&g, &p).await;
    let hold = hold_data_writer(&g, p.a.node_id(), "t1").await;
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| p.authority.running_count() == 0, Duration::from_secs(10)).await, "HELD: the call ran on while the gateway's data writer was held");
    assert!(TEST_WRITE_HOLD.pin().contains_key(&p.a.node_id().to_string()), "…and the writer was still held when it stopped");
    let receipt = tokio::time::timeout(Duration::from_secs(10), cancel).await.expect("the cancel is answered").unwrap().expect("recorded");
    assert_eq!(receipt.class, CancelClass::Stopped, "{receipt:?}");
    assert!(g.a.system_stats().control_lane_sent >= 1, "the forward rode the lane");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the tool never finished");
    drop(hold);
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T1r: a cancel reaches the provider while its data reader is held** (the test of D1). The provider persists under
/// `flush`; its reader of the gateway's data connection is held mid-record — a gateway write's WAL hand-off paused
/// after its apply (`pause_inbound_after_apply`) — so nothing behind that record on the data connection is read. The
/// cancel still stops the call while the reader is held, receipt `Stopped`. Seen failing first with the lane copy put
/// into the data writer's channel (option (a) of the design, `ControlLane::send` handing it to `peer_writers`): the
/// forward waited behind the held reader and the call ran on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_reaches_the_provider_while_its_data_reader_is_held() {
    let (g, p, cert_dir) = pair(Opts { mint: true, provider_persist: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let (answered, term) = running_call(&g, &p).await;
    let key = "b1/t1r/held";
    let pause = p.ctx().wal.get().expect("the provider persists").pause_inbound_after_apply(key);
    let _ = g.a.kv().set(key, b"x".to_vec());
    tokio::time::timeout(Duration::from_secs(10), pause.reached()).await.expect("the provider's reader reached the held record");
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| p.authority.running_count() == 0, Duration::from_secs(10)).await, "HELD: the call ran on while the provider's data reader was held");
    let receipt = tokio::time::timeout(Duration::from_secs(10), cancel).await.expect("the cancel is answered").unwrap().expect("recorded");
    assert_eq!(receipt.class, CancelClass::Stopped, "{receipt:?}");
    pause.release();
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T4b: no gateway client reaches the lane.** A client with `mesh:write` emits a frame of a control kind through
/// `/gateway/signal/emit` — the test kind, which the gateway's doors leave open, and `sys.call-cancel.v1`, which they
/// refuse — and the gateway's lane counters stay at 0: only `emit_signal_control` offers a frame to the lane, and no
/// door calls it. Seen failing first with `emit_signal` classifying by kind (a frame of a control kind also offered to
/// the lane): `control_lane_sent` was 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_gateway_client_reaches_the_lane() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let client = reqwest::Client::new();
    let emit = |kind: &str| {
        client.post(format!("http://127.0.0.1:{}/gateway/signal/emit", g.http_port))
            .bearer_auth("mesher-secret")
            .json(&serde_json::json!({"kind": kind, "scope": format!("node:{}", p.a.node_id()), "payload_b64": "eA=="}))
            .send()
    };
    let mut rx = p.a.mesh().signal_rx(mycelium_core::control_lane::TEST_CONTROL_KIND);
    assert_eq!(emit(mycelium_core::control_lane::TEST_CONTROL_KIND).await.unwrap().status(), 200, "the test kind is open at the doors");
    assert!(tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.is_ok(), "and reaches the provider");
    assert_eq!(emit(CANCEL_RPC_KIND).await.unwrap().status(), 403, "the substrate's kind is refused at the doors");
    let s = g.a.system_stats();
    assert_eq!((s.control_lane_sent, s.control_lane_full, s.control_lanes_open), (0, 0, 0), "LANE: a gateway client's frame reached the lane");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T4c: a storm of cancels is bounded by the lane.** 4 × 64 terms minted toward `p` and cancelled at once, with the
/// gateway's lane to `p` held in a write: the lane takes what its depth holds (64, and the batch in hand), counts the
/// rest `full`, and every cancel is still served by the provider — the shared path carries it — and gossip converges.
/// Async, every receipt is an answer (none `DeliveryUnknown`). Run again under `flush` with the provider's
/// quorum-evidence appends held (M4): every forward is still served; a receipt may then be `DeliveryUnknown`, since the
/// shared copies wait, under `flush`, behind the provider's reader applying the revocations' own records one fsync at
/// a time — the wait the lane exists to avoid, which this test holds the lane to force. Seen failing first with the
/// depth unbounded
/// (`CONTROL_LANE_DEPTH` 100 000): nothing was counted `full`, every copy queued behind the held write.
async fn a_storm_of_cancels_is_bounded_by_the_lane(persist: bool) {
    let (g, p, cert_dir) = pair(Opts { mint: true, provider_persist: persist, ..Opts::default() }).await;
    let g = Arc::new(g);
    let to = p.a.node_id().clone();
    let held_evidence = persist.then(|| p.ctx().wal.get().expect("persists").hold_appends_under("sys/quorum/"));
    let mut terms = Vec::new();
    for _ in 0..256 {
        terms.push(g.framed(&to, "quick").await.1);
    }
    // The mints' records reach the provider first, so the cancels' shared copies wait behind nothing of theirs.
    let minted = crate::store::scan_kv_prefix(&g.ctx().kv_state, "call/").len();
    assert!(until(|| crate::store::scan_kv_prefix(&p.ctx().kv_state, "call/").len() >= minted, Duration::from_secs(60)).await, "the mints' records reached the provider");
    let served = test_hooks::served(&to);
    let lane_hold = Hold::new(format!("control/{to}"));
    let cancels: Vec<_> = terms.iter().cloned().map(|t| {
        let g = Arc::clone(&g);
        tokio::spawn(async move { g.a.cancel_call(&t).await })
    }).collect();
    let mut unanswered = 0;
    for c in cancels {
        let r = tokio::time::timeout(Duration::from_secs(60), c).await.expect("a cancel is answered").unwrap().expect("recorded");
        if r.class == CancelClass::DeliveryUnknown { unanswered += 1; }
    }
    let s = g.a.system_stats();
    assert!(lane_hold.reached() >= 1, "the lane reached its hold");
    assert!(s.control_lane_full >= 1, "UNBOUNDED: no lane copy was refused full behind the held write (sent {})", s.control_lane_sent);
    assert!(s.control_lane_sent <= 64 + 256, "sent {}", s.control_lane_sent);
    assert!(until(|| test_hooks::served(&to) - served >= 256, Duration::from_secs(60)).await, "every forward was served by the provider: {}", test_hooks::served(&to) - served);
    if !persist {
        assert_eq!(unanswered, 0, "every cancel was answered by the shared path");
    }
    let _ = g.a.kv().set("b1/t4c/after", b"v".to_vec());
    assert!(until(|| p.kv("b1/t4c/after").is_some(), Duration::from_secs(15)).await, "gossip converges");
    drop(lane_hold);
    if let Some(h) = held_evidence { h.release(); }
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_storm_of_cancels_is_bounded_by_the_lane_async() {
    a_storm_of_cancels_is_bounded_by_the_lane(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_storm_of_cancels_is_bounded_by_the_lane_under_flush_with_the_wal_held() {
    a_storm_of_cancels_is_bounded_by_the_lane(true).await;
}

/// **T5: each cancel attempt is served once, though sent twice.** One cancel of a running call goes on the lane and by
/// the shared path, one encoded frame; the provider's `serve` takes it once — the second copy is the same nonce, dropped
/// by the seen-set before it reaches `rpc_rx` (no late duplicate is counted) — and the receipt is `Stopped`. Seen
/// failing first with the lane copy re-encoded (a fresh nonce): the twin passed the seen-set and was refused only by
/// the envelope's replay window, counted as a late duplicate. (`serve` still took it once: the replay window is a
/// second guard behind the seen-set.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_cancel_attempt_is_served_once_though_sent_twice() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let (answered, term) = running_call(&g, &p).await;
    let before = test_hooks::served(p.a.node_id());
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::Stopped, "{receipt:?}");
    assert!(g.a.system_stats().control_lane_sent >= 1, "the forward rode the lane");
    // Long enough for the slower copy to have arrived too.
    let _ = g.a.kv().set("b1/t5/after", b"v".to_vec());
    assert!(until(|| p.kv("b1/t5/after").is_some(), Duration::from_secs(10)).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(test_hooks::served(p.a.node_id()) - before, 1, "TWICE: one attempt was served more than once");
    assert_eq!(p.ctx().control_lane.stats.late_duplicates.load(Ordering::Relaxed), 0, "TWIN: the second copy passed the seen-set (it is not the same frame)");
    assert_eq!(g.ctx().rpc_reply_sender_mismatches.load(Ordering::Relaxed), 0);
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T5b: a late twin of a cancel is counted as a late duplicate, not a replay.** The gateway's data writer to `p` is
/// held, so the forward arrives on the lane only and is served. The provider's seen-set is then trimmed — as its
/// emergency trim does under a flood — and the held shared twin released: it reaches `rpc_rx`, the envelope's replay
/// window refuses it, and it is counted in `control_late_duplicates`, not `caller_envelopes_replayed`. Nothing is served
/// again. Seen failing first with the classification removed (`verify_received` counting every `Replayed` as a caller
/// replay): `caller_envelopes_replayed` rose and `control_late_duplicates` stayed 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_twin_of_a_cancel_is_counted_as_a_late_duplicate_not_a_replay() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let (answered, term) = running_call(&g, &p).await;
    let hold = hold_data_writer(&g, p.a.node_id(), "t5b").await;
    let served = test_hooks::served(p.a.node_id());
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::Stopped, "the lane copy was served: {receipt:?}");
    assert_eq!(test_hooks::served(p.a.node_id()) - served, 1);
    let replayed = p.ctx().caller_replay.replayed.load(Ordering::Relaxed);
    // The emergency trim, to past every nonce held: the twin's nonce is forgotten.
    p.ctx().seen.emergency_trim(u64::MAX);
    drop(hold);
    let late = || p.ctx().control_lane.stats.late_duplicates.load(Ordering::Relaxed);
    assert!(until(|| late() == 1, Duration::from_secs(15)).await, "REPLAY: the late twin was not counted as a late duplicate (caller replays {})", p.ctx().caller_replay.replayed.load(Ordering::Relaxed));
    assert_eq!(p.ctx().caller_replay.replayed.load(Ordering::Relaxed), replayed, "caller_envelopes_replayed is unchanged");
    assert_eq!(test_hooks::served(p.a.node_id()) - served, 1, "nothing was served again");
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T8: a provider without the lane still answers** (a mixed fleet, emulated). The provider runs with
/// `control_lane = false`: the gateway's forward arrives on its lane, the provider's reply goes by the shared path only,
/// and the receipt is `Stopped`. A run against the v2.40.x binary is **not run** here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_without_the_lane_still_answers() {
    let (g, p, cert_dir) = pair(Opts { mint: true, provider_no_lane: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let (answered, term) = running_call(&g, &p).await;
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::Stopped, "{receipt:?}");
    assert!(g.a.system_stats().control_lane_sent >= 1, "the request rode the gateway's lane");
    assert_eq!(p.a.system_stats().control_lane_sent, 0, "the provider, lane off, replied by the shared path only");
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **T9 (M6): a cancel that overtakes its call refuses the call.** The gateway's data writer to `p` is held before the
/// client calls, so the call's request waits behind it; the cancel goes on the lane and reaches `p` first. `p` holds
/// the revocation of a term it has not registered and answers `NotRegistered`; released, the call arrives and is
/// refused, and it never runs. Seen failing first with the provider's hold of an unregistered term removed (`answer`
/// returning `not_registered` without holding the revocation): the call ran once it arrived.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_that_overtakes_its_call_refuses_the_call() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let hold = hold_data_writer(&g, p.a.node_id(), "t9").await;
    // The call as the gateway's dispatch sends it — minted, its records written — and emitted before the cancel, so its
    // request is queued behind the held writer (a client call through `/mcp` could still be refused by the gateway
    // itself, seeing its own revocation before it dispatches).
    let pid = p.a.node_id().clone();
    let (framed, term) = g.framed(&pid, "slow").await;
    let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let e2 = Arc::clone(&emitted);
    super::rpc::after_emit::set(g.a.node_id(), Arc::new(move || e2.store(true, Ordering::SeqCst)));
    let (g2, to) = (Arc::clone(&g), pid.clone());
    let answered = tokio::spawn(async move { g2.send(&to, framed).await });
    assert!(until(|| emitted.load(Ordering::SeqCst), Duration::from_secs(10)).await, "the call's request was emitted");
    super::rpc::after_emit::clear(g.a.node_id());
    // The revocation's durable write is held, so it enters no KV and the KV carries it nowhere: only the forward on the
    // lane tells the provider (otherwise the revoking record, queued right behind the call on the data connection,
    // refuses the call whatever the provider held).
    let revoke_held = super::call_issuer::test_hooks::hold_revoke_writes(g.a.node_id());
    let served = test_hooks::served(&pid);
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| test_hooks::served(&pid) > served, Duration::from_secs(10)).await, "the forward reached the provider on the lane, ahead of its call");
    assert_eq!(p.started.load(Ordering::SeqCst), 0);
    drop(hold);
    assert!(until(|| p.started.load(Ordering::SeqCst) > 0 || answered.is_finished(), Duration::from_secs(20)).await, "the call arrived");
    assert_eq!(p.started.load(Ordering::SeqCst), 0, "RAN: the call ran after its cancel had overtaken it");
    let reply = tokio::time::timeout(Duration::from_secs(20), answered).await.expect("the call is answered").unwrap();
    assert!(reply.to_string().contains("revoked"), "the call was refused as revoked: {reply}");
    drop(revoke_held);
    let receipt = tokio::time::timeout(Duration::from_secs(20), cancel).await.expect("the cancel is answered").unwrap();
    eprintln!("B1 T9: the cancel's receipt {receipt:?}");
    if let Ok(r) = receipt {
        assert_eq!(r.class, CancelClass::NotRegistered, "the cancel overtook its call: {r:?}");
    }
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A reply goes on the lane only for a request that passed its receive boundary as its own sender** (#832's round-3
/// review). The gateway sends `p` a genuine cancel-forward — its own envelope, its own checkpoint — in a frame whose
/// `sender` names a third node `c`, which `p` knows (a data writer, a `peers` entry). `p`'s receive boundary refuses
/// it (the envelope's `via` is not the frame's sender) and answers the refusal to the frame's `sender`, `c` — **by the
/// shared path only**: `c` receives the refusal, no lane opens to `c`, and no connect is made. Seen failing first with
/// the refusal answered on the lane (`rpc_respond_ctx` offering the reply to a control request to the lane): `p`
/// opened a lane to `c`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spoofed_sender_gets_no_lane_from_the_provider() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let c = node(Opts { cert_dir: Some(cert_dir.clone()), bootstrap: vec![p.a.node_id().clone()], ..Opts::default() }).await;
    let (pid, cid) = (p.a.node_id().clone(), c.a.node_id().clone());
    assert!(until(|| p.a.peers().contains(&cid), Duration::from_secs(20)).await, "p knows c");
    // p holds a connected data writer to c: nothing but the rule keeps a lane to c from opening.
    p.a.connect_peer(cid.clone());
    assert!(
        until(|| p.a.peer_writers.pin().get(&cid).is_some_and(|e| e.is_live() && e.connected.load(Ordering::Acquire)), Duration::from_secs(20)).await,
        "p's data writer to c connected",
    );
    let (_, term) = g.framed(&pid, "quick").await;
    let revoking = super::call_issuer::test_sign_checkpoint(g.ctx(), &term, 2, g.now(), true);
    let app = Bytes::from(serde_json::to_vec(&serde_json::json!({ "v": 1, "checkpoint": revoking })).unwrap());
    let framed = super::gateway_caller::frame_self(g.ctx(), &pid, app);
    let mut payload = fastrand::u64(1..).to_le_bytes().to_vec();
    payload.extend_from_slice(&framed);
    let correlation = u64::from_le_bytes(payload[..8].try_into().unwrap());
    let mut answers = c.a.mesh().signal_rx("rpc.result");
    let forged = mycelium_core::framing::WireMessage::Signal {
        ttl: g.ctx().default_ttl, nonce: fastrand::u64(1..), sender: cid.clone(),
        scope: crate::SignalScope::Individual(pid.clone()), kind: Arc::from(CANCEL_RPC_KIND), payload: Bytes::from(payload), hlc_seq: None,
    };
    assert!(mycelium_core::framing::dispatch_gossip_try_send(
        &g.ctx().gossip_txs, forged, g.a.node_id().id_hash(), mycelium_core::framing::ForwardHint::Individual(pid.clone()), &g.ctx().kv_state.dropped_frames,
    ));
    let answered = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(sig) = answers.recv().await {
            if sig.payload.len() >= 8 && u64::from_le_bytes(sig.payload[..8].try_into().unwrap()) == correlation {
                return sig;
            }
        }
        panic!("c's channel closed");
    }).await.expect("p answered the forged-sender forward, to c");
    assert_eq!(answered.sender, pid);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = p.a.system_stats();
    assert_eq!(s.control_lane_sent, 0, "SPOOFED: p answered a forged sender on the lane");
    assert!(p.ctx().control_lane.writers().pin().get(&cid).is_none(), "no lane writer to c");
    assert_eq!(s.control_lane_connect_failed, 0);
    c.a.shutdown().await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
