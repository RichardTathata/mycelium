//! Identity-and-lifecycle I4, PR 3: the cancel doors — the cancel-forward, the cancel receipt, MCP
//! `notifications/cancelled` and A2A `tasks/cancel` — end to end over two nodes (`call_mint_tests`' gateway and
//! provider). Each test names the plant it was seen failing against.

use super::call_cancel::{self, CallSink, DoorCancel, CANCEL_RPC_KIND, MAX_MCP_CANCEL_ENTRIES};
use super::call_issuer;
use super::call_mint_tests::{node, pair, until, Opts, F_MS, MCP};
use super::gateway_authority::call_checkpoint_key;
use crate::mandate::authority::SignedRevocationCheckpoint;
use crate::{CancelClass, NodeId};
use bytes::Bytes;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The provider's answer to a cancel-forward of `checkpoint`, sent by `from` as a self envelope.
async fn forward_from(from: &super::TaskCtx, to: &NodeId, checkpoint: &SignedRevocationCheckpoint) -> serde_json::Value {
    let payload = serde_json::to_vec(&serde_json::json!({ "v": 1, "checkpoint": checkpoint })).unwrap();
    let reply = super::rpc::rpc_call_ctx(from, to.clone(), Arc::from(CANCEL_RPC_KIND), Bytes::from(payload), Duration::from_secs(10)).await.expect("the provider answers");
    serde_json::from_slice(&reply).unwrap()
}

fn stat(n: &super::call_mint_tests::N, family: &str, label: &str) -> u64 {
    let c = &n.ctx().call_issuer;
    let map = match family {
        "receipts" => &c.cancel_receipts,
        _ => &c.cancel_forwards_refused,
    };
    map.pin().get(label).copied().unwrap_or(0)
}

/// **The cancel-forward stops a running call without the KV** (the maintainer's H1: the primary path). A client's
/// call through `g`'s `/mcp` runs at `p`; the gateway's revoking checkpoint is signed and sent to `p` directly — and
/// **written nowhere**: `p`'s KV still holds the first checkpoint, sequence 1. `p` holds the revocation, its sweep
/// stops the call, and it answers `stopped`: the tool's future was dropped, the stop record names the term. Seen
/// failing first with `answer` planted to skip `hold_call_revocation` (answer `not_running`): the call ran on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_forward_stops_a_running_call_without_the_kv() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let cp = call_checkpoint_key(&g.gateway(), &term);
    assert!(until(|| p.kv(&cp).is_some(), Duration::from_secs(10)).await, "the first checkpoint reached the provider");

    let revoking = call_issuer::test_sign_checkpoint(g.ctx(), &term, 2, g.now(), true);
    let v = forward_from(g.ctx(), p.a.node_id(), &revoking).await;
    assert_eq!(v["class"], "stopped", "the provider answers its own confirmed stop: {v}");
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the client is answered").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    p.running(0).await;
    assert_eq!((p.started.load(Ordering::SeqCst), p.finished.load(Ordering::SeqCst)), (1, 0), "the tool never finished");
    assert_eq!(p.authority.named_stop_records().last().and_then(|s| s.term.clone()), Some(term.clone()));
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&p.kv(&cp).unwrap()).unwrap();
    assert_eq!(held.checkpoint.seq, 1, "the provider's KV still holds the first checkpoint: the stop came from the forward");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **`GossipAgent::cancel_call` returns the receipt the provider answered, and counts it.** The gateway revokes and
/// records, forwards, and the receipt is `Stopped` — the provider's confirmed stop; `GET /stats`'
/// `call_cancel_receipts.stopped` counts it. A second cancel of the same term is `NotRunning`: the revoke is
/// idempotent, the provider still holds the revocation and runs nothing under it. An unknown term is refused,
/// nothing forwarded. Seen failing first with `cancel` planted to return `NotForwarded` without forwarding: the
/// receipt said nothing of the provider's stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_call_returns_the_providers_receipt_and_counts_it() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!((receipt.term.as_str(), &receipt.class), (term.as_str(), &CancelClass::Stopped), "{receipt:?}");
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("answered").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0);
    assert_eq!(stat(&g, "receipts", "stopped"), 1);
    // The revocation also reached the provider by the KV, and on a slower machine (CI's) it stopped the call before
    // the forward arrived: the provider answers from the stop record it keeps, not `NotRunning` (#799's CI, first
    // run — the fail-first evidence of the fix). A second cancel of the stopped term is that case, every time.
    let again = g.a.cancel_call(&term).await.expect("idempotent");
    assert_eq!(again.class, CancelClass::Stopped, "the work under the term was stopped here, and its record says so: {again:?}");
    assert_eq!(g.a.cancel_call("c1").await, Err(crate::CancelRefusal::UnknownTerm));
    let stats: serde_json::Value = reqwest::Client::new().get(format!("http://127.0.0.1:{}/stats", g.http_port)).send().await.unwrap().json().await.unwrap();
    assert_eq!(stats["call_cancel_receipts"]["stopped"].as_u64(), Some(2), "{}", stats["call_cancel_receipts"]);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A cancel-forward from a node other than the term's issuing gateway is refused `cancel_not_from_issuer`, and the
/// call keeps running** (exit 7). `p` itself relays the gateway's genuine, gateway-signed revoking checkpoint to
/// itself: refused by name, counted, nothing held — and the call still runs. Then the gateway's own forward stops it.
/// Seen failing first with `admit` planted to take the issuer from the checkpoint instead of the sender: the relayed
/// checkpoint stopped the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_forward_from_another_node_is_refused_and_the_call_keeps_running() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let revoking = call_issuer::test_sign_checkpoint(g.ctx(), &term, 2, g.now(), true);
    let v = forward_from(p.ctx(), p.a.node_id(), &revoking).await;
    assert_eq!((v["class"].as_str(), v["reason"].as_str()), (Some("refused"), Some("cancel_not_from_issuer")), "{v}");
    assert_eq!(stat(&p, "forwards", "cancel_not_from_issuer"), 1);
    tokio::time::sleep(Duration::from_millis(3 * F_MS / 20 + 200)).await;
    assert_eq!(p.authority.running_count(), 1, "the call keeps running");
    assert!(!answered.is_finished());
    let v = forward_from(g.ctx(), p.a.node_id(), &revoking).await;
    assert_eq!(v["class"], "stopped", "{v}");
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await;
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **No answer is `DeliveryUnknown`, never a negative, and the KV still stops the call** (exit 8's receipt half; the
/// receipts ADR). With the gateway's forwards suppressed, `cancel_call`'s receipt is `DeliveryUnknown` — and the
/// revocation it recorded reaches the provider by the KV and stops the call within *F*. Seen failing first with a
/// failed forward mapped to `NotRunning`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_answer_is_delivery_unknown_and_the_kv_still_stops_the_call() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    call_cancel::test_hooks::SUPPRESS_FORWARD.pin().insert(g.a.node_id().to_string(), ());
    let asked = std::time::Instant::now();
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::DeliveryUnknown, "{receipt:?}");
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("answered").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert!(asked.elapsed() < Duration::from_millis(F_MS), "the KV path stopped it within F: {:?}", asked.elapsed());
    call_cancel::test_hooks::SUPPRESS_FORWARD.pin().remove(&g.a.node_id().to_string());
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **A call that reaches the provider after its cancel-forward is refused `revoked`** — the provider held the
/// revocation it was sent, so the call never runs, whatever arrives later. The gateway mints a call and records it
/// without sending; `cancel_call` answers `NotRegistered` (the provider never saw it) and holds the term's window; the envelope, sent
/// now, is refused `call_mandate_not_established` with `data.cause` `revoked`, the tool not started. Seen failing
/// first with `hold_call_revocation` planted to hold nothing for a scope it has no window for (`NotHeld`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_arriving_after_its_cancel_forward_is_refused_revoked() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let (framed, term) = g.framed(p.a.node_id(), "quick").await;
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    // The call has not reached the provider, which never registered it: it cannot say the call is not running
    // (#799's review, M1).
    assert_eq!(receipt.class, CancelClass::NotRegistered, "{receipt:?}");
    let scope = format!("call:{term}");
    assert!(p.authority.holds_call_window(&crate::mandate::PrincipalId::new(g.gateway()).unwrap(), &scope), "the provider holds the revoked term");
    let v = g.send(p.a.node_id(), framed).await;
    assert_eq!((v["error"]["data"]["detail"].as_str(), v["error"]["data"]["cause"].as_str()), (Some("call_mandate_not_established"), Some("revoked")), "{v}");
    assert_eq!(p.started.load(Ordering::SeqCst), 0, "nothing ran");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **Exit 8: work whose agent made no promise to stop is `Unbounded`, never `Stopped`.** Under a plain (not
/// cancellable) rule, a call to a tool registered with `register_mcp_tool_unstoppable` is admitted and registered
/// unconfirmed under its `call:` mandate; its cancel's receipt is `Unbounded`, and the stop record claims no
/// confirmation. Seen failing first with `call_registrations` planted to count every registration as confirmed: the
/// receipt said `Stopped`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_of_work_whose_agent_made_no_promise_to_stop_is_unbounded() {
    let (g, p, cert_dir) = pair(Opts { mint: true, plain: true, more_doors: true, ..Opts::default() }).await;
    let key = format!("tools/unstoppable/{}", p.a.node_id());
    assert!(until(|| g.kv(&key).is_some(), Duration::from_secs(20)).await);
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let _answered = tokio::spawn(async move { g2.client_call("unstoppable").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::Unbounded, "{receipt:?}");
    assert_eq!(stat(&g, "receipts", "unbounded"), 1);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// One MCP request to `n`'s `/mcp` under `token`.
async fn mcp(n: &super::call_mint_tests::N, token: &str, body: serde_json::Value) -> (u16, String) {
    let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/mcp", n.http_port)).bearer_auth(token).json(&body).send().await.expect("the gateway answers");
    (resp.status().as_u16(), resp.text().await.unwrap_or_default())
}

/// **Exit 7, MCP: `notifications/cancelled` cancels the caller's own call, and another principal's finds nothing.**
/// Alice's `tools/call` (id 7) runs at the provider. Bob's `notifications/cancelled` for request 7 is answered `202`,
/// counted `unmatched`, and the call keeps running. Alice's is answered `202`, and the call stops: her `tools/call` is
/// answered `cancelled`, the tool never finishes, and the receipt is counted `stopped`. Seen failing first with the
/// table's key planted without its principal: Bob's notification stopped Alice's call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notifications_cancelled_cancels_the_callers_own_call_and_no_one_elses() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move {
        mcp(&g2, "alice-secret", serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "slow", "arguments": {"aisle": 4}}})).await
    });
    p.running(1).await;
    let cancel = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7, "reason": "user"}});
    let (status, body) = mcp(&g, "bob-secret", cancel.clone()).await;
    assert_eq!((status, body.as_str()), (202, ""), "a notification has no reply");
    assert_eq!(g.ctx().call_issuer.mcp_cancels.unmatched.load(Ordering::Relaxed), 1, "bob's cancel found no call of his");
    tokio::time::sleep(Duration::from_millis(3 * F_MS / 20 + 200)).await;
    assert_eq!(p.authority.running_count(), 1, "alice's call keeps running");
    let (status, _) = mcp(&g, "alice-secret", cancel).await;
    assert_eq!(status, 202);
    let (_, reply) = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("alice is answered").unwrap();
    assert!(reply.contains("cancelled"), "{reply}");
    p.running(0).await;
    assert_eq!(p.finished.load(Ordering::SeqCst), 0);
    assert_eq!(g.ctx().call_issuer.mcp_cancels.cancelled.load(Ordering::Relaxed), 1);
    assert!(until(|| stat(&g, "receipts", "stopped") == 1, Duration::from_secs(10)).await, "the forward's receipt is counted");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **The `/mcp` table is bounded**: at [`MAX_MCP_CANCEL_ENTRIES`] a new call is not registered and is counted
/// `table_full`; a key already in flight is not registered again (`collisions`); a registration's drop removes its
/// entry; and entries older than `call_mandate_ttl_secs` are evicted to make room. Seen failing first with the cap
/// check taken out: the table grew past it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_cancel_table_is_bounded_and_counts_what_it_refuses() {
    let n = node(Opts { ttl_secs: Some(1), ..Opts::default() }).await;
    let key = |i: usize| ("token:gw/alice".to_string(), String::new(), i.to_string());
    let mut held = Vec::with_capacity(MAX_MCP_CANCEL_ENTRIES);
    for i in 0..MAX_MCP_CANCEL_ENTRIES {
        held.push(call_cancel::register_mcp_call(n.ctx(), key(i)).expect("room"));
    }
    let t = &n.ctx().call_issuer.mcp_cancels;
    assert!(call_cancel::register_mcp_call(n.ctx(), key(MAX_MCP_CANCEL_ENTRIES)).is_none(), "full");
    assert_eq!(t.full.load(Ordering::Relaxed), 1);
    drop(held.pop());
    assert!(call_cancel::register_mcp_call(n.ctx(), key(0)).is_none(), "a key in flight");
    assert_eq!(t.collisions.load(Ordering::Relaxed), 1);
    let again = call_cancel::register_mcp_call(n.ctx(), key(MAX_MCP_CANCEL_ENTRIES - 1)).expect("a dropped registration's entry is gone");
    drop(again);
    std::mem::forget(held);
    // Past the window every entry is stale, and a new call gets room.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let k = key(MAX_MCP_CANCEL_ENTRIES + 1);
    let _fills = call_cancel::register_mcp_call(n.ctx(), key(MAX_MCP_CANCEL_ENTRIES + 2)).expect("one below the cap");
    assert!(call_cancel::register_mcp_call(n.ctx(), k).is_some(), "stale entries are evicted");
    n.a.shutdown().await;
}

/// **A cancel that arrives before the mint sends nothing.** The door's handle is cancelled first (`BeforeMint`);
/// the dispatch then mints, writes its records, sees the flag, revokes the term on disk and refuses
/// `cancelled_before_sent`: the provider never runs the tool. Seen failing first with the dispatch's check taken
/// out: the call was sent and ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_before_the_mint_sends_nothing_and_revokes_the_term() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let sink = Arc::new(CallSink::default());
    assert!(matches!(call_cancel::cancel_at_door(g.ctx(), &sink).await, Ok(DoorCancel::BeforeMint)));
    let caller = super::gateway_caller::ResolvedPrincipal { principal: "token:gw/alice".into(), scopes: vec!["mcp:invoke".into()], subject_token: None };
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "quick", "arguments": {"aisle": 4}}}).to_string();
    let dispatch = super::gateway_caller::gateway_rpc_call_with_mandate(g.ctx(), Some(&caller), p.a.node_id().clone(), Arc::from(MCP), Bytes::from(payload), Duration::from_secs(5), None, None, None);
    let out = call_cancel::CALL_SINK.scope(Arc::clone(&sink), dispatch).await;
    assert_eq!(out.err(), Some(super::gateway_caller::GatewayDispatchError::CancelledBeforeSent));
    let term = sink.term().expect("the dispatch filled the term").to_string();
    let cp = call_checkpoint_key(&g.gateway(), &term);
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&cp).unwrap()).unwrap();
    assert!(held.checkpoint.revoked.iter().any(|t| t.as_str() == term), "the term is revoked on record");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.started.load(Ordering::SeqCst), 0, "nothing was sent");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// One A2A request to `n`'s `/a2a` under `token`.
#[cfg(feature = "a2a")]
async fn a2a(n: &super::call_mint_tests::N, token: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let resp = reqwest::Client::new().post(format!("http://127.0.0.1:{}/a2a", n.http_port)).bearer_auth(token).json(&body).send().await.expect("the gateway answers");
    resp.json().await.unwrap_or_default()
}

/// **Exit 7, A2A: `tasks/cancel` revokes the call's term before it removes the task, and the call stops; another
/// principal's cancel is refused and the call keeps running.** Alice's `tasks/send` to the skill `depot/dispatch`
/// (served at the provider by a receiver that declared it stops) runs; Bob's `tasks/cancel` is refused as not the
/// task's creator and the skill keeps running; Alice's is answered `canceled` once the gateway has recorded the
/// revocation, the skill is told its authority lapsed, and the task is gone — her send's late answer does not bring
/// it back. Seen failing first with `handle_tasks_cancel`'s `cancel_at_door` taken out: the skill ran on.
#[cfg(feature = "a2a")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_cancel_revokes_before_it_removes_and_only_for_the_tasks_owner() {
    let (g, p, cert_dir) = pair(Opts { mint: true, more_doors: true, ..Opts::default() }).await;
    assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await, "the gateway sees the skill");
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let send = serde_json::json!({"id": "t-1", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
    let answered = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", send).await });
    let started = until(|| p.authority.running_count() == 1 || answered.is_finished(), Duration::from_secs(20)).await;
    assert!(started && p.authority.running_count() == 1, "the skill runs under its call: mandate: {:?}", if answered.is_finished() { Some(answered.await.unwrap()) } else { None });
    let v = a2a(&g, "bob-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await;
    assert!(v.get("error").is_some(), "bob is not the task's creator: {v}");
    tokio::time::sleep(Duration::from_millis(3 * F_MS / 20 + 200)).await;
    assert_eq!(p.authority.running_count(), 1, "the skill keeps running");
    let v = a2a(&g, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await;
    assert_eq!(v["result"]["status"]["state"], "canceled", "{v}");
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&call_checkpoint_key(&g.gateway(), &term)).unwrap()).unwrap();
    assert!(held.checkpoint.revoked.iter().any(|t| t.as_str() == term), "revoked on record before the answer");
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the send is answered");
    p.running(0).await;
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the skill stopped");
    let v = a2a(&g, "alice-secret", "tasks/get", serde_json::json!({"id": "t-1"})).await;
    assert_eq!(v["error"]["code"], -32001, "the cancelled task stays gone: {v}");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

// ── #799's review: the fixes, each seen failing first against the plant its doc comment names ───────────────────

/// **H1: `collect` keeps the live issued calls and drops the dead, past a resize of `issued`.** 4 000 terms are held
/// as issued — every other one already past its window + 2·skew — far past papaya's first incremental resize, then
/// `collect` runs. Seen failing first with `collect`'s `papaya::HashMap::retain` put back: papaya 0.1.9's `retain`
/// reads each entry through its tagged pointer after the resize, and the test binary aborts
/// (`misaligned pointer dereference`, `raw/mod.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collect_keeps_the_live_issued_calls_and_drops_the_dead_past_a_resize() {
    let n = node(Opts { mint: true, ..Opts::default() }).await;
    let now = n.now();
    let live: std::collections::BTreeSet<String> = (0..4_000u64).filter(|i| i % 2 == 0).map(|i| format!("t{i}")).collect();
    for i in 0..4_000u64 {
        let until = if i % 2 == 0 { now + 3_600_000 } else { 1 };
        call_issuer::test_hold_issued(n.ctx(), &format!("t{i}"), until);
    }
    let _ = call_issuer::collect(n.ctx(), now);
    let held: std::collections::BTreeSet<String> = n.ctx().call_issuer.issued_terms().into_iter().collect();
    assert_eq!(held, live, "only the live terms are held after collect");
    n.a.shutdown().await;
}

/// **M1 (the reviewer's probe A): a provider that does not enforce never answers `NotRunning` for a call it runs.**
/// The provider has an execution authority and no provider enforcement, so it registers nothing. A client's 30 s call
/// runs there; its cancel's receipt is `NotRegistered` — the provider cannot see the work — while the tool still
/// runs. Seen failing first with 722ead12's answer planted back (no enforcement check, an unregistered scope
/// answered `not_running`): `NotRunning` with the tool started and unfinished.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unenforcing_provider_answers_not_registered_never_not_running() {
    let (g, p, cert_dir) = pair(Opts { mint: true, no_enforce: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::NotRegistered, "{receipt:?}");
    assert_eq!((p.finished.load(Ordering::SeqCst), answered.is_finished()), (0, false), "the call still runs, which is what NotRegistered allows");
    // And it holds nothing (#799's confirm round, M1): a hold here would refuse nothing, since nothing is checked at
    // admission. Seen failing first with the enforcement check taken out of `answer`: one window was held.
    assert_eq!(p.authority.view_sizes().2, 0, "an unenforcing provider holds no revocation");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **M1's narrower case: a plain rule's call whose `call:` slot did not establish is `NotRegistered`.** Under a rule
/// that is not cancellable, a call whose second slot is not a mandate runs unregistered at the enforcing provider;
/// a revocation forwarded for its term is answered `NotRegistered`. Seen failing first against the same plant:
/// `not_running`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_rules_call_with_no_call_mandate_is_not_registered() {
    let (g, p, cert_dir) = pair(Opts { mint: true, plain: true, ..Opts::default() }).await;
    // Framed by the gateway with a second slot that is not a mandate (`malformed`): under the plain rule the call
    // runs, unregistered. Its term is `c{n}` of the envelope, as any minted term would be.
    let n_seen = std::sync::Mutex::new(0u64);
    let bogus = |n: u64| {
        *n_seen.lock().unwrap() = n;
        Some(serde_json::json!({ "not": "a mandate" }))
    };
    let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "slow", "arguments": {"aisle": 4}}}).to_string();
    let stripped = super::gateway_caller::frame_envelope_minting_at(g.ctx(), p.a.node_id(), &g.gateway(), &[], Bytes::from(payload), None, None, None, None, g.now(), Some(&bogus)).expect("fits");
    let term = format!("c{}", *n_seen.lock().unwrap());
    let (g2, target) = (Arc::new(g), p.a.node_id().clone());
    let g3 = Arc::clone(&g2);
    let _running = tokio::spawn(async move { g3.send(&target, stripped).await });
    p.began(1).await;
    assert_eq!(p.authority.running_count(), 0, "unregistered");
    let revoking = call_issuer::test_sign_checkpoint(g2.ctx(), &term, 2, g2.now(), true);
    let v = forward_from(g2.ctx(), p.a.node_id(), &revoking).await;
    assert_eq!(v["class"], "not_registered", "{v}");
    g2.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **L3 (the reviewer's probe B): the hold alone refuses a call that arrives after its cancel.** The revoking
/// checkpoint is forwarded and written nowhere, so the provider's KV holds only sequence 1; the call sent afterwards
/// is refused `revoked` and nothing runs. Seen failing first with `hold_call_revocation` planted to plant the window
/// and answer `Held` without offering the checkpoint: the call ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hold_alone_refuses_a_call_that_arrives_after_its_cancel() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let (framed, term) = g.framed(p.a.node_id(), "quick").await;
    let revoking = call_issuer::test_sign_checkpoint(g.ctx(), &term, 2, g.now(), true);
    let v = forward_from(g.ctx(), p.a.node_id(), &revoking).await;
    assert_eq!(v["class"], "not_registered", "{v}");
    let cp = call_checkpoint_key(&g.gateway(), &term);
    assert!(until(|| p.kv(&cp).is_some(), Duration::from_secs(10)).await);
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&p.kv(&cp).unwrap()).unwrap();
    assert_eq!(held.checkpoint.seq, 1, "the KV holds only the first checkpoint");
    let v = g.send(p.a.node_id(), framed).await;
    assert_eq!((v["error"]["data"]["detail"].as_str(), v["error"]["data"]["cause"].as_str()), (Some("call_mandate_not_established"), Some("revoked")), "{v}");
    assert_eq!(p.started.load(Ordering::SeqCst), 0, "nothing ran");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **L2: an unconfirmed release the KV made before the forward arrived is `Unbounded`, never `Stopped`.** Under a plain
/// rule a tool registered unstoppable runs registered unconfirmed; the gateway's revoke reaches the provider by the KV
/// (no forward), the sweep releases the work, and only then is the cancel forwarded: the provider answers from the
/// unconfirmed stop record. Seen failing first with the `Some(false)` arm planted to answer `stopped`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_release_recorded_before_the_forward_is_unbounded() {
    let (g, p, cert_dir) = pair(Opts { mint: true, plain: true, more_doors: true, ..Opts::default() }).await;
    let key = format!("tools/unstoppable/{}", p.a.node_id());
    assert!(until(|| g.kv(&key).is_some(), Duration::from_secs(20)).await);
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let _answered = tokio::spawn(async move { g2.client_call("unstoppable").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    assert_eq!(call_issuer::revoke_call(g.ctx(), &term).await, Ok(true), "revoked, by the KV only");
    p.running(0).await;
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::Unbounded, "{receipt:?}");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **M2 (the reviewer's probe D): a pre-0.2 re-send of a pending task starts no second call, so its cancel reaches
/// the task's only call and the task stays cancelled.** Alice sends `t-r`, then re-sends it while it is pending: the
/// re-send is answered the pending task and nothing more runs (`running_count` stays 1). Her `tasks/cancel` stops
/// that call, and `tasks/get` afterwards finds no task. Seen failing first with the `InFlight` arm taken out
/// (722ead12's reservation): the re-send ran a second call (`started` 2).
#[cfg(feature = "a2a")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resent_pending_task_runs_no_second_call_and_its_cancel_stays() {
    let (g, p, cert_dir) = pair(Opts { mint: true, more_doors: true, ..Opts::default() }).await;
    assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await);
    let g = Arc::new(g);
    let send = serde_json::json!({"id": "t-r", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
    let (g2, s2) = (Arc::clone(&g), send.clone());
    let first = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", s2).await });
    p.running(1).await;
    let again = a2a(&g, "alice-secret", "tasks/send", send.clone()).await;
    assert_eq!(again["result"]["id"], "t-r", "the re-send is answered the task: {again}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!((p.authority.running_count(), p.started.load(Ordering::SeqCst)), (1, 1), "no second call");
    let v = a2a(&g, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-r"})).await;
    assert_eq!(v["result"]["status"]["state"], "canceled", "{v}");
    let _ = tokio::time::timeout(Duration::from_secs(10), first).await.expect("the send is answered");
    p.running(0).await;
    let v = a2a(&g, "alice-secret", "tasks/get", serde_json::json!({"id": "t-r"})).await;
    assert_eq!(v["error"]["code"], -32001, "the cancelled task stays gone: {v}");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **L5 (#799's confirm round): the `/mcp` table's eviction removes only the stale entry it scanned.** The table is
/// full of entries past `call_mandate_ttl_secs`; between the eviction's scan and its removals, key 0's old call ends
/// and a fresh call registers at key 0. The eviction must leave the fresh entry, so its cancel still finds it. Seen
/// failing first with the removal by key alone (`remove`): the fresh call lost its handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_tables_eviction_leaves_a_fresh_call_registered_at_a_stale_key() {
    let n = node(Opts { ttl_secs: Some(1), ..Opts::default() }).await;
    let key = |i: usize| ("token:gw/alice".to_string(), String::new(), i.to_string());
    let mut held = Vec::with_capacity(MAX_MCP_CANCEL_ENTRIES);
    for i in 0..MAX_MCP_CANCEL_ENTRIES {
        held.push(call_cancel::register_mcp_call(n.ctx(), key(i)).expect("room"));
    }
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let old = held.swap_remove(0);
    let fresh: Arc<std::sync::Mutex<Option<call_cancel::McpRegistration>>> = Arc::default();
    let (ctx, slot) = (Arc::clone(n.ctx()), Arc::clone(&fresh));
    call_cancel::test_hooks::AFTER_EVICT_SCAN.lock().unwrap().push((
        n.a.node_id().to_string(),
        Box::new(move || {
            drop(old);
            *slot.lock().unwrap() = Some(call_cancel::register_mcp_call(&ctx, key(0)).expect("the old call's place"));
        }),
    ));
    let _trigger = call_cancel::register_mcp_call(n.ctx(), key(MAX_MCP_CANCEL_ENTRIES)).expect("room after eviction");
    let fresh_sink = fresh.lock().unwrap().as_ref().expect("the hook ran").sink();
    let held_sink = call_cancel::test_hooks::mcp_sink(n.ctx(), &key(0)).expect("the fresh call is still registered");
    assert!(Arc::ptr_eq(&held_sink, &fresh_sink), "the table holds the fresh call's handle");
    std::mem::forget(held);
    n.a.shutdown().await;
}

// ── §9 O10: the cancel is forwarded at once; only the caller's acknowledgement waits for the disk ───────────────

/// What a burst queued at the gateway's WAL ahead of a cancel is made of.
#[derive(Clone, Copy, Debug)]
enum Burst {
    /// Appends handed to the gateway's WAL alone (`append_try`), applied and gossiped nowhere: under `sync_mode =
    /// "flush"` the writer syncs each one. Not plain `kv_set` writes (#821's CI, 2026-11-21): those are gossiped too,
    /// and the forward then reaches the provider only after every frame queued ahead of it on the same connection —
    /// 15 718 of them took the CI runner 24 s — which is the connection's order, not the gateway's disk. With the burst
    /// gossiped, the stop came when the provider had applied 12 550 of 15 718 burst keys; with it on the WAL alone, in
    /// 13 ms (`docs/operations/what-is-proven.md#call-cancel-behind-collector`).
    WalAppends,
    /// Concurrent mints (`call_issuer::record`): two required-sync writes each, synced under any mode. Each mint gossips
    /// its records only once synced, so few frames are ahead of the forward on the connection: probed for #821's CI,
    /// 800 mints held the stop 11.8 ms, as 800 synced appends gossiped nowhere did.
    Mints,
}

/// Queue `n` units of `burst` at `g`'s WAL, ahead of whatever is asked next; the mints' tasks, still writing.
async fn queue_burst(g: &Arc<super::call_mint_tests::N>, target: &NodeId, burst: Burst, n: usize, tag: &str) -> Vec<tokio::task::JoinHandle<()>> {
    match burst {
        Burst::WalAppends => {
            let wal = g.ctx().wal.get().expect("persists");
            for i in 0..n {
                let u = crate::framing::make_gossip_update(g.a.node_id(), 4, Arc::from(format!("o10/{tag}/{i}")), Bytes::from_static(b"burst"), false, &g.ctx().hlc);
                wal.append_try(crate::framing::sync_entry_from(&u));
            }
            Vec::new()
        }
        Burst::Mints => {
            // Each task's first write is handed to the WAL in the same poll that counts it begun.
            let begun = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let tasks = (0..n)
                .map(|_| {
                    let (g, target, begun) = (Arc::clone(g), target.clone(), Arc::clone(&begun));
                    tokio::spawn(async move {
                        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "quick", "arguments": {"aisle": 4}}}).to_string();
                        let (_, minted) = super::gateway_caller::frame_minting(g.ctx(), &target, &g.gateway(), &[], MCP, Bytes::from(payload), None, None, None, None, g.now());
                        begun.fetch_add(1, Ordering::SeqCst);
                        call_issuer::record(g.ctx(), minted.expect("minted")).await.expect("a mint's records are on disk");
                    })
                })
                .collect();
            assert!(until(|| begun.load(Ordering::SeqCst) == n, Duration::from_secs(30)).await, "every mint of the burst began");
            tasks
        }
    }
}

/// How long `n` units of `burst` take to drain through `g`'s WAL with nothing else asked: the calibration that sizes
/// a burst to this machine's disk, so a fast disk is given a longer burst rather than a pass.
async fn drain_time(g: &Arc<super::call_mint_tests::N>, target: &NodeId, burst: Burst, n: usize) -> Duration {
    let began = std::time::Instant::now();
    let tasks = queue_burst(g, target, burst, n, "calibration").await;
    for t in tasks {
        t.await.expect("a mint of the calibration ran");
    }
    // Behind every append queued above: the WAL serves its queue in order.
    let op = mycelium_core::receipt::OperationId::generate(g.a.node_id());
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    mycelium_core::ops::kv_set_requiring_sync(g.ctx(), op, attempt, Arc::from("o10/calibration/marker"), Bytes::from_static(b"m"), None).await.expect("the marker is on disk");
    began.elapsed()
}

/// **§9 O10's test: during a burst of synced appends ahead of the cancel, the provider's stop arrives inside bound 1
/// while the caller's acknowledgement still waits for the disk.** A gateway with `[persistence]` under `mode` and a
/// provider; a client's call runs at the provider; `burst` is queued at the gateway's WAL, sized from a calibration
/// to drain in about three times *F* on this machine's disk; then `cancel_call`. Asserted, by structure: the call stops at
/// the provider inside *F* (bound 1) — the forward did not wait for the syncs ahead — **while `cancel_call` has not
/// answered**; the answer then comes only once the revocation is on disk (the gateway's own record is the revoking
/// checkpoint when it does), and it took several times longer than the stop.
async fn the_stop_beats_the_acknowledgement_behind(mode: crate::config::SyncMode, burst: Burst) {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, wal: Some((mode, 1_000_000)), ..Opts::default() }).await;
    let g = Arc::new(g);
    let target = p.a.node_id().clone();
    // The disk's pace, then a burst sized to it: about 3·F of syncs, at least 400 and at most 50 000 units — long
    // enough that a forward which waited for it would land outside bound 1 by a clear margin.
    let calibration = 200;
    let per = drain_time(&g, &target, burst, calibration).await / (calibration as u32 + 1);
    let n = ((3 * F_MS) as u128 * 1_000_000 / per.as_nanos().max(1)).clamp(400, 50_000) as usize;

    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    p.began(1).await;
    // The client's call is the newest mint: the calibration's came before it.
    let term = g.ctx().call_issuer.issued_terms().into_iter().max_by_key(|t| t[1..].parse::<u64>().unwrap_or(0)).expect("minted");
    let tasks = queue_burst(&g, &target, burst, n, "burst").await;
    let asked = std::time::Instant::now();
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move {
        let r = g2.a.cancel_call(&t2).await;
        (r, std::time::Instant::now())
    });
    let stopped = until(|| p.authority.running_count() == 0, Duration::from_secs(120)).await;
    let stop_took = asked.elapsed();
    let acknowledged_at_the_stop = cancel.is_finished();
    let (receipt, acked) = tokio::time::timeout(Duration::from_secs(300), cancel).await.expect("the cancel is answered").unwrap();
    let ack_took = acked.saturating_duration_since(asked);
    for t in tasks {
        t.await.expect("a mint of the burst ran");
    }
    eprintln!(
        "§9 O10, {mode:?} behind {n} {burst:?} (one drains in {per:?} here): the provider's stop seen {stop_took:?} after the cancel; the cancel acknowledged {ack_took:?} after it; receipt {receipt:?}"
    );
    assert!(stopped, "the call stopped at the provider");
    assert!(
        stop_took < Duration::from_millis(F_MS),
        "the provider's stop came {stop_took:?} after the cancel, outside bound 1 (F = {F_MS} ms): the forward waited for the {n} {burst:?} ahead of the revocation's sync (the cancel was acknowledged after {ack_took:?})"
    );
    // What O10 promises, and no ratio (#821's CI: on a loaded runner a 3x margin failed with the promise kept): the stop
    // inside bound 1, above, and the acknowledgement not yet given when the stop was seen — the burst, sized to about
    // 3·F, still held the disk then.
    assert!(!acknowledged_at_the_stop, "the cancel was acknowledged ({ack_took:?}) before the provider's stop was seen ({stop_took:?}): the burst did not hold the disk");
    let receipt = receipt.expect("the revocation is on disk");
    // The provider answers `stopped`; under `flush` the answer can reach the gateway's caller too late for the
    // forward's 5 s — the gateway's connection reader awaits the WAL for each inbound record (`append_inbound`), so the
    // answer's frame waits behind the same burst — and is then `DeliveryUnknown`: never a negative (§9 O11's family).
    assert!(matches!(receipt.class, CancelClass::Stopped | CancelClass::DeliveryUnknown), "the provider's stop, or no answer in time: {receipt:?}");
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&call_checkpoint_key(&g.gateway(), &term)).unwrap()).unwrap();
    assert!(held.checkpoint.seq == 2 && held.checkpoint.revoked.iter().any(|t| t.as_str() == term), "acknowledged with the revoking checkpoint in the gateway's own record: {:?}", held.checkpoint);
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the client is answered").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the tool never finished");
    assert_eq!(g.ctx().wal.get().expect("persists").dropped_appends(), 0, "every append of the burst reached the WAL");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// §9 O10 under `sync_mode = "flush"`, behind a burst of synced appends at the gateway's WAL — what O10 promises the
/// forward does not wait for. Seen failing first on main at 7e9486f7 (the forward sent only after the revocation's
/// sync), with plain gossiped writes as the burst; with the burst on the WAL alone it fails there the same way (the
/// revocation's own sync is behind it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_flush_the_providers_stop_beats_the_acknowledgement_behind_a_burst_of_writes() {
    the_stop_beats_the_acknowledgement_behind(crate::config::SyncMode::Flush, Burst::WalAppends).await;
}

/// §9 O10 under the default `sync_mode` (`async`), behind concurrent mints — required-sync writes queue behind each
/// other under any mode. Seen failing first on main at 7e9486f7.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_the_default_mode_the_providers_stop_beats_the_acknowledgement_behind_concurrent_mints() {
    the_stop_beats_the_acknowledgement_behind(crate::config::SyncMode::default(), Burst::Mints).await;
}

/// **A cancel the disk refuses still stops the call, and the caller is told it is not durable** (§9 O10, the
/// maintainer's decision of 2026-11-20, reversing *"stands in this process only, and is not forwarded"*). A client's
/// call runs at the provider; the gateway's WAL is poisoned; `cancel_call` answers `NotDurable` — and the forward was
/// sent: the call stops at the provider, which answered `stopped` (counted, not returned), while the gateway's KV
/// still holds the first checkpoint (a required-sync write applies nothing it could not sync). Seen failing first on
/// main at 7e9486f7: nothing was forwarded, and the call ran on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_the_disk_refuses_still_stops_the_call_and_cancel_call_says_not_durable() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    p.began(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    g.ctx().wal.get().expect("persists").poison_for_test("the disk is gone").await.unwrap();
    let refused = g.a.cancel_call(&term).await;
    // #821's review, F2: the refusal carries what the provider answered, so the caller learns its call stopped.
    assert!(
        matches!(&refused, Err(call_cancel::CancelRefusal::NotDurableForwarded { class: CancelClass::Stopped, .. })),
        "the caller is told the cancel is not on disk, and that the provider stopped the call: {refused:?}"
    );
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the call was stopped all the same").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    p.running(0).await;
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the tool never finished");
    assert_eq!(stat(&g, "receipts", "stopped"), 1, "the provider's answer is counted");
    let held = serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&call_checkpoint_key(&g.gateway(), &term)).unwrap()).unwrap();
    assert_eq!(held.checkpoint.seq, 1, "nothing applied to the gateway's KV: the revocation is in memory only");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// §9 O10 at `/mcp`: a `notifications/cancelled` the disk refuses is answered `202` (a notification has no reply) and
/// the call stops at the provider all the same. Seen failing first on main at 7e9486f7: the call ran on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notifications_cancelled_the_disk_refuses_still_stops_the_call() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move {
        mcp(&g2, "alice-secret", serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "slow", "arguments": {"aisle": 4}}})).await
    });
    p.running(1).await;
    g.ctx().wal.get().expect("persists").poison_for_test("the disk is gone").await.unwrap();
    let (status, _) = mcp(&g, "alice-secret", serde_json::json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7, "reason": "user"}})).await;
    assert_eq!(status, 202);
    let (_, reply) = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the call was stopped all the same").unwrap();
    assert!(reply.contains("cancelled"), "{reply}");
    p.running(0).await;
    assert_eq!(p.finished.load(Ordering::SeqCst), 0);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// §9 O10 at `/a2a`: a `tasks/cancel` the disk refuses is answered `-32603` and the task is kept (so it can be
/// cancelled again), and the skill is stopped at the provider all the same. Seen failing first on main at 7e9486f7:
/// the skill ran on.
#[cfg(feature = "a2a")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tasks_cancel_the_disk_refuses_is_refused_keeps_the_task_and_still_stops_the_skill() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, more_doors: true, ..Opts::default() }).await;
    assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await, "the gateway sees the skill");
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let send = serde_json::json!({"id": "t-1", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
    let answered = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", send).await });
    let started = until(|| p.authority.running_count() == 1 || answered.is_finished(), Duration::from_secs(20)).await;
    assert!(started && p.authority.running_count() == 1, "the skill runs under its call: mandate");
    g.ctx().wal.get().expect("persists").poison_for_test("the disk is gone").await.unwrap();
    let v = a2a(&g, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await;
    assert_eq!(v["error"]["code"], -32603, "not recorded, so not canceled here: {v}");
    p.running(0).await;
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the skill was stopped all the same");
    let _ = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the send is answered");
    // Kept, and not as `completed`: the send's answer was the stop the cancel's forward caused.
    let v = a2a(&g, "alice-secret", "tasks/get", serde_json::json!({"id": "t-1"})).await;
    assert!(v.get("error").is_none() && v["result"]["status"]["state"] != "completed", "the task is kept for a second cancel: {v}");
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **§9 O10 at every door: the forward goes out while the revocation's durable write is held, and the door answers
/// only once it returns.** The gateway's revoke writes are held at a gate (`hold_revoke_writes`) — the shape of a
/// WAL that has not got to the write yet, under any burst. Through `cancel_call`, `/mcp` `notifications/cancelled`
/// and (with `a2a`) `/a2a` `tasks/cancel` in turn: the call stops at the provider and its caller is answered
/// `cancelled` **while the cancel is unanswered** and the gateway's KV still holds the first checkpoint; released, the
/// cancel is answered and the record is the revoking checkpoint. Seen failing first with the old order planted back
/// (`finish_revoke` awaited before the forward is sent, in `cancel` and `cancel_at_door`): the call ran on until the
/// gate was released, *"running_count 1 (wanted 0)"*.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_forward_goes_out_while_the_durable_write_is_held_at_every_door() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, more_doors: cfg!(feature = "a2a"), ..Opts::default() }).await;
    let g = Arc::new(g);
    let checkpoint_seq = |term: &str| serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&call_checkpoint_key(&g.gateway(), term)).unwrap()).unwrap().checkpoint.seq;

    // `cancel_call`.
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let held = call_issuer::test_hooks::hold_revoke_writes(g.a.node_id());
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| held.reached() == 1, Duration::from_secs(10)).await, "the revoke reached its durable write");
    p.running(0).await;
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the client is answered").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert!(!cancel.is_finished(), "cancel_call answered before its revocation was on disk");
    assert_eq!(checkpoint_seq(&term), 1, "nothing in the gateway's KV before the sync");
    held.release(1);
    let receipt = tokio::time::timeout(Duration::from_secs(10), cancel).await.expect("answered once on disk").unwrap().expect("on disk");
    assert_eq!(receipt.class, CancelClass::Stopped, "{receipt:?}");
    assert_eq!(checkpoint_seq(&term), 2, "the revoking checkpoint is the gateway's record once acknowledged");

    // `/mcp` `notifications/cancelled`: its `202` waits for the disk.
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move {
        mcp(&g2, "alice-secret", serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "slow", "arguments": {"aisle": 4}}})).await
    });
    p.running(1).await;
    let g2 = Arc::clone(&g);
    let cancel = tokio::spawn(async move { mcp(&g2, "alice-secret", serde_json::json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7, "reason": "user"}})).await });
    assert!(until(|| held.reached() == 2, Duration::from_secs(10)).await, "the door's revoke reached its durable write");
    p.running(0).await;
    let (_, reply) = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the client is answered").unwrap();
    assert!(reply.contains("cancelled"), "{reply}");
    assert!(!cancel.is_finished(), "the notification was answered before its revocation was on disk");
    held.release(1);
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), cancel).await.expect("answered once on disk").unwrap();
    assert_eq!(status, 202);

    // `/a2a` `tasks/cancel`: `canceled` waits for the disk.
    #[cfg(feature = "a2a")]
    {
        assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await, "the gateway sees the skill");
        let g2 = Arc::clone(&g);
        let send = serde_json::json!({"id": "t-1", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
        let answered = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", send).await });
        p.running(1).await;
        let g2 = Arc::clone(&g);
        let cancel = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await });
        assert!(until(|| held.reached() == 3, Duration::from_secs(10)).await, "the door's revoke reached its durable write");
        p.running(0).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the send is answered");
        assert!(!cancel.is_finished(), "tasks/cancel was answered before its revocation was on disk");
        held.release(1);
        let v = tokio::time::timeout(Duration::from_secs(10), cancel).await.expect("answered once on disk").unwrap();
        assert_eq!(v["result"]["status"]["state"], "canceled", "{v}");
    }
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "no cancelled call finished");
    drop(held);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **§9 O10's crash bound, pinned.** The forward goes out before the revocation is on disk, and a required-sync write
/// applies nothing to the KV until its sync returns — so a gateway that crashes between the forward and the sync
/// leaves **no record** of the revocation, and its caller **no answer at all** (not a "not durable" one). The provider
/// holds the revocation in memory only (D-4: the hold and the window are not persisted). Here: a call runs at the
/// provider; `cancel_call` forwards and its write is held; the provider stops the call; the gateway "crashes" —
/// the cancel's task dropped before the write, the gateway shut down with its exit snapshot sabotaged, so only its
/// WAL carries its records into the restart. After the restart the gateway's KV holds the call's first checkpoint,
/// not the revoking one. The provider restarts; the call's envelope, presented again inside its window, **is
/// admitted and runs** — no record says otherwise. The bound: such a cancel lasts as long as the provider process
/// that holds it, and at most to the call's window. A cancel asked again after the restart writes the revocation
/// durably (`NotForwarded`: the restarted gateway does not know where the call went) and the KV then stops the
/// re-presented call. Seen failing first with the forward planted after the write (the old order): the provider never
/// stopped the call while the write was held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gateway_crash_between_the_forward_and_the_sync_leaves_no_record_and_a_restarted_provider_admits_the_call_again() {
    use crate::test_util::{kv_dir_under, restore_start_snapshot, sabotage_exit_snapshot};
    let port = crate::test_util::alloc_port;
    let (gport, ghttp, pport, phttp) = (port(), port(), port(), port());
    let cert_dir = std::env::temp_dir().join(format!("i4-mint-ca-{gport}"));
    let gdir = std::env::temp_dir().join(format!("i4-mint-{gport}"));
    let pdir = std::env::temp_dir().join(format!("i4-mint-{pport}"));
    for d in [&cert_dir, &gdir, &pdir] {
        let _ = std::fs::remove_dir_all(d);
    }
    let gopts = Opts { mint: true, persist: true, cert_dir: Some(cert_dir.clone()), gossip_port: Some(gport), http_port: Some(ghttp), ..Opts::default() };
    let gid = NodeId::new("127.0.0.1", gport).unwrap();
    let popts = Opts { cert_dir: Some(cert_dir.clone()), bootstrap: vec![gid.clone()], tools: true, gossip_port: Some(pport), http_port: Some(phttp), ..Opts::default() };
    let knows = |x: &crate::GossipAgent, n: &NodeId| x.task_ctx.peer_keys.pin().get(n).is_some_and(|k| !k.is_empty());
    let g = node(gopts.clone()).await;
    let p = node(popts.clone()).await;
    assert!(until(|| knows(&g.a, p.a.node_id()) && knows(&p.a, &gid), Duration::from_secs(20)).await, "the nodes learned each other's keys");
    let pid = p.a.node_id().clone();

    let (framed, term) = g.framed(&pid, "slow").await;
    let g = Arc::new(g);
    let (g2, f2, pid2) = (Arc::clone(&g), framed.clone(), pid.clone());
    let first = tokio::spawn(async move { g2.send(&pid2, f2).await });
    p.running(1).await;
    let held = call_issuer::test_hooks::hold_revoke_writes(&gid);
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| held.reached() == 1, Duration::from_secs(10)).await, "the revoke reached its durable write");
    p.running(0).await;
    let reply = tokio::time::timeout(Duration::from_secs(10), first).await.expect("the call stopped").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");

    // The crash: the cancel never gets past its write, and its caller is never answered.
    cancel.abort();
    assert!(cancel.await.expect_err("dropped before its write").is_cancelled(), "the caller got no answer at all");
    let kv_dir = kv_dir_under(&gdir.join("persist"));
    sabotage_exit_snapshot(&kv_dir);
    g.a.shutdown().await;
    drop(held);
    drop(g);
    tokio::time::sleep(Duration::from_millis(300)).await;
    restore_start_snapshot(&kv_dir);
    let g = node(gopts).await;
    let cp = call_checkpoint_key(&g.gateway(), &term);
    let after = serde_json::from_slice::<SignedRevocationCheckpoint>(&g.kv(&cp).expect("the first checkpoint came back from the WAL")).unwrap();
    assert!(after.checkpoint.seq == 1 && after.checkpoint.revoked.is_empty(), "no record of the revocation after the gateway's restart: {:?}", after.checkpoint);

    // The provider restarts: the revocation it held in memory is gone, and the envelope, inside its window, runs.
    p.a.shutdown().await;
    drop(p);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = node(popts).await;
    // Structural, not a wait (#821's CI): the gateway still holds the old instance's key, so keys prove nothing. The
    // restarted provider holds the call's checkpoint only once the gateway has synced its KV to it, and the gateway
    // counts it connected only once a session to the new instance is up — both facts of the new instance.
    let reachable = || p.kv(&cp).is_some() && g.a.connected_peers().contains(&pid) && knows(&p.a, &gid);
    assert!(until(reachable, Duration::from_secs(30)).await, "the restarted provider is reachable from the gateway and holds its records");
    let g = Arc::new(g);
    let (g2, pid2) = (Arc::clone(&g), pid.clone());
    let again = tokio::spawn(async move { g2.send(&pid2, framed).await });
    let ran = until(|| p.authority.running_count() == 1 || again.is_finished(), Duration::from_secs(20)).await && p.authority.running_count() == 1;
    assert!(ran, "the call re-presented inside its window is admitted after the provider's restart: {:?}", if again.is_finished() { Some(again.await) } else { None });
    p.began(1).await;

    // The remedy: a cancel asked again is written durably, and the KV carries it to the provider.
    let receipt = g.a.cancel_call(&term).await.expect("on disk now");
    assert_eq!(receipt.class, CancelClass::NotForwarded, "the restarted gateway does not know where the call went (D-4)");
    let reply = tokio::time::timeout(Duration::from_secs(10), again).await.expect("the KV stopped the re-presented call").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0);
    g.a.shutdown().await;
    p.a.shutdown().await;
    for d in [&cert_dir, &gdir, &pdir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// **#821's review, F1: a call that completes on its own while its cancel is being recorded is answered as completed,
/// never `canceled`, and its result stays readable.** The reviewer's probe: the skill `soon` runs at the provider and
/// completes on its own in 2 s; the gateway's revoke write is held and its forward held back until the skill has
/// completed, so the provider answers it `not_running` (the work ended with no stop of its own); `tasks/cancel` is
/// asked. The send answers `completed` while the cancel waits; released, the cancel answers `-32002` *"task already
/// completed"* and `tasks/get` returns the completed task with the skill's result. Seen failing first with the guard as
/// #821 first shipped it (any answer during the cancel left unrecorded): the cancel answered `canceled` and `tasks/get`
/// `-32001`. (With the forward suppressed instead — no answer at all — the confirm round's rule answers it unknown:
/// `a_stop_whose_forward_answer_is_lost_is_answered_unknown_never_completed`.)
#[cfg(feature = "a2a")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_that_completes_while_its_cancel_is_recorded_is_answered_completed_and_its_result_kept() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, more_doors: true, ..Opts::default() }).await;
    assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await, "the gateway sees the skill");
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let send = serde_json::json!({"id": "t-1", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "soon"}]}});
    let answered = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", send).await });
    p.running(1).await;
    let held = call_issuer::test_hooks::hold_revoke_writes(g.a.node_id());
    let forward = Arc::new(tokio::sync::Semaphore::new(0));
    call_cancel::test_hooks::HOLD_FORWARD.pin().insert(g.a.node_id().to_string(), Arc::clone(&forward));
    let g2 = Arc::clone(&g);
    let cancel = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await });
    assert!(until(|| held.reached() == 1, Duration::from_secs(10)).await, "the cancel reached its durable write");
    let sent = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the skill completed on its own").unwrap();
    assert_eq!(sent["result"]["status"]["state"], "completed", "{sent}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 1, "the skill finished: the provider stopped nothing");
    p.running(0).await;
    forward.add_permits(1);
    assert!(until(|| stat(&g, "receipts", "not_running") == 1, Duration::from_secs(10)).await, "the provider answered the forward not_running");
    assert!(!cancel.is_finished(), "the cancel waits for its write");
    held.release(1);
    let v = tokio::time::timeout(Duration::from_secs(15), cancel).await.expect("the cancel is answered").unwrap();
    call_cancel::test_hooks::HOLD_FORWARD.pin().remove(&g.a.node_id().to_string());
    forward.close();
    assert_eq!(v["error"]["code"], -32002, "a call that completed is not answered canceled: {v}");
    let v = a2a(&g, "alice-secret", "tasks/get", serde_json::json!({"id": "t-1"})).await;
    assert_eq!(v["result"]["status"]["state"], "completed", "the result stays readable: {v}");
    assert!(v.to_string().contains("dispatched"), "the skill's own result: {v}");
    drop(held);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **#821's review, F3: a caller that drops `cancel_call`'s future stops neither the forward nor its count.** The
/// gateway's revoke write is held; `cancel_call` is asked and its future dropped once the write is reached. The call
/// still stops at the provider, and the provider's answer is still counted in `call_cancel_receipts`. Seen failing
/// first with the forward joined to the caller's future (`tokio::join!`, as #821 first shipped it): the answer was
/// never counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_cancel_call_still_forwards_and_counts_the_providers_answer() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, ..Opts::default() }).await;
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let answered = tokio::spawn(async move { g2.client_call("slow").await });
    p.running(1).await;
    let term = g.ctx().call_issuer.issued_terms().pop().expect("minted");
    let held = call_issuer::test_hooks::hold_revoke_writes(g.a.node_id());
    let (g2, t2) = (Arc::clone(&g), term.clone());
    let cancel = tokio::spawn(async move { g2.a.cancel_call(&t2).await });
    assert!(until(|| held.reached() == 1, Duration::from_secs(10)).await, "the revoke reached its durable write");
    cancel.abort();
    assert!(cancel.await.expect_err("dropped").is_cancelled());
    let reply = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the call stopped").unwrap();
    assert!(reply.to_string().contains("cancelled"), "{reply}");
    assert!(until(|| stat(&g, "receipts", "stopped") == 1, Duration::from_secs(10)).await, "the provider's answer is counted though the caller left");
    drop(held);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **#821's confirm round: a stop the provider did not confirm to the gateway is never answered as a completion.** The
/// reviewer's probe 1: the skill runs at the provider; the gateway's revoke write is held, and its forward is
/// delivered but its answer lost (`DeliveryUnknown`). The provider stops the skill — the send answers the stop's own
/// text, the skill never finishes — while the cancel waits; released, the cancel answers **outcome unknown**
/// (`-32603`), neither `canceled` nor *"task already completed"*, and `tasks/get` holds the send's answer, the text its
/// sender was given. Seen failing first on 3276b116 (the two-way rule): the cancel answered `-32002`.
#[cfg(feature = "a2a")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_whose_forward_answer_is_lost_is_answered_unknown_never_completed() {
    let (g, p, cert_dir) = pair(Opts { mint: true, persist: true, more_doors: true, ..Opts::default() }).await;
    assert!(until(|| crate::store::scan_kv_prefix(&g.ctx().kv_state, "cap/").iter().any(|(k, _)| k.contains("depot")), Duration::from_secs(20)).await, "the gateway sees the skill");
    let g = Arc::new(g);
    let g2 = Arc::clone(&g);
    let send = serde_json::json!({"id": "t-1", "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}});
    let answered = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/send", send).await });
    p.running(1).await;
    let held = call_issuer::test_hooks::hold_revoke_writes(g.a.node_id());
    call_cancel::test_hooks::LOSE_FORWARD_ANSWER.pin().insert(g.a.node_id().to_string(), ());
    let g2 = Arc::clone(&g);
    let cancel = tokio::spawn(async move { a2a(&g2, "alice-secret", "tasks/cancel", serde_json::json!({"id": "t-1"})).await });
    assert!(until(|| held.reached() == 1, Duration::from_secs(10)).await, "the cancel reached its durable write");
    p.running(0).await;
    let sent = tokio::time::timeout(Duration::from_secs(10), answered).await.expect("the send is answered").unwrap();
    assert!(sent.to_string().contains("stopped"), "the send answered the stop's own text: {sent}");
    assert_eq!(p.finished.load(Ordering::SeqCst), 0, "the provider stopped the skill");
    assert!(until(|| stat(&g, "receipts", "delivery_unknown") == 1, Duration::from_secs(10)).await, "the forward's answer was lost");
    held.release(1);
    let v = tokio::time::timeout(Duration::from_secs(15), cancel).await.expect("the cancel is answered").unwrap();
    call_cancel::test_hooks::LOSE_FORWARD_ANSWER.pin().remove(&g.a.node_id().to_string());
    assert_eq!(v["error"]["code"], -32603, "outcome unknown, neither canceled nor completed: {v}");
    let v = a2a(&g, "alice-secret", "tasks/get", serde_json::json!({"id": "t-1"})).await;
    assert!(v.to_string().contains("stopped"), "the send's answer stays readable: {v}");
    drop(held);
    g.a.shutdown().await;
    p.a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
