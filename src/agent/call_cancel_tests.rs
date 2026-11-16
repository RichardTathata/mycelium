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
    let again = g.a.cancel_call(&term).await.expect("idempotent");
    assert_eq!(again.class, CancelClass::NotRunning, "nothing runs under the term, and the revocation is held: {again:?}");
    assert_eq!(g.a.cancel_call("c1").await, Err(crate::CancelRefusal::UnknownTerm));
    let stats: serde_json::Value = reqwest::Client::new().get(format!("http://127.0.0.1:{}/stats", g.http_port)).send().await.unwrap().json().await.unwrap();
    assert_eq!((stats["call_cancel_receipts"]["stopped"].as_u64(), stats["call_cancel_receipts"]["not_running"].as_u64()), (Some(1), Some(1)), "{}", stats["call_cancel_receipts"]);
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
/// without sending; `cancel_call` answers `NotRunning` and the provider holds the term's window; the envelope, sent
/// now, is refused `call_mandate_not_established` with `data.cause` `revoked`, the tool not started. Seen failing
/// first with `hold_call_revocation` planted to hold nothing for a scope it has no window for (`NotHeld`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_arriving_after_its_cancel_forward_is_refused_revoked() {
    let (g, p, cert_dir) = pair(Opts { mint: true, ..Opts::default() }).await;
    let (framed, term) = g.framed(p.a.node_id(), "quick").await;
    let receipt = g.a.cancel_call(&term).await.expect("recorded");
    assert_eq!(receipt.class, CancelClass::NotRunning, "{receipt:?}");
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
