//! Row K2's promises against live nodes: the exact scope admits at most N across gateways under concurrent calls; no
//! quorum is not established; the tally converges and refuses after it; stale or unknown coverage is not established;
//! every decision names its scope.
//!
//! The decisions are driven through `ae_preflight` — the function every door runs (`/mcp`, `/a2a` unary and stream,
//! the LLM doors, the federation route and the provider check) — on each node's own context, so the counting is the
//! enforcement point's and the network between the nodes is real. Multi-node consensus: a listener on every voting
//! node, registered before the first proposal, and a structural peer-ready poll, never a fixed sleep.

use super::super::action_evaluator::{AeEvidence, FleetState, HistoryScope, PreflightRefusal};
use super::super::evidence_journal::{EvidenceJournal, EvidenceProfile};
use super::super::http::{ae_preflight, Preflight};
use crate::consensus::ConsensusConfig;
use crate::{GossipAgent, GossipConfig, NodeId, ReferenceEvaluator, Rule};
use std::sync::Arc;
use std::time::Duration;

const TEN_MIN: u64 = 600_000;
const POINT: &str = "gateway:mcp/tools/call";

fn port() -> u16 {
    crate::test_util::alloc_port()
}

/// A node at `p` dialling `peers`, with a strict evidence journal and *at most `limit` posts in ten minutes* for the
/// anonymous actor, counted at `scope`.
async fn node(p: u16, peers: &[u16], limit: u32, scope: HistoryScope, tune: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
    node_with(p, peers, posts(limit, TEN_MIN, scope), tune).await
}

/// *At most `limit` posts in `window_ms`* for the anonymous actor, counted at `scope` under the counter `c-posts`.
fn posts(limit: u32, window_ms: u64, scope: HistoryScope) -> ReferenceEvaluator {
    ReferenceEvaluator::new("rev-k2")
        .with_catalogue("cat-k2", "1")
        .map_action("tools/call", "*")
        .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", "tool:post").at_most(limit, window_ms).scoped(scope, "c-posts"))
}

/// A node at `p` dialling `peers`, with a strict evidence journal and `policy`.
async fn node_with(p: u16, peers: &[u16], policy: ReferenceEvaluator, tune: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
    let mut cfg = GossipConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        bind_port: p,
        bootstrap_peers: peers.iter().map(|q| NodeId::new("127.0.0.1", *q).unwrap()).collect(),
        ..GossipConfig::default()
    };
    tune(&mut cfg);
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg));
    let dir = std::env::temp_dir().join(format!("k2-fleet-{p}"));
    let _ = std::fs::remove_dir_all(&dir);
    agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    agent.with_action_evaluator(Arc::new(policy));
    agent.start().await.unwrap();
    agent
}

async fn until(cond: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

/// Structural readiness: every node sees `n - 1` peers and its history index has been read.
async fn ready(nodes: &[&Arc<GossipAgent>]) {
    let n = nodes.len();
    for a in nodes {
        let ctx = Arc::clone(&a.task_ctx);
        let others: Vec<String> = nodes.iter().map(|o| o.node_id().to_string()).filter(|o| *o != a.node_id().to_string()).collect();
        assert!(
            until(
                || ctx.peers.pin().len() + 1 >= n
                    && ctx.history_index.get().is_some_and(|i| matches!(
                        i.view("gateway", "probe", crate::HistoryRequest { window_ms: 1, max_entries: 1 }, 1).0,
                        crate::History::View(_)
                    ))
                    // Every other node's tally heartbeat has arrived (it is written from start).
                    && others.iter().all(|o| mycelium_core::ops::kv_get(&ctx, &super::heartbeat_key(o)).is_some()),
                Duration::from_secs(20),
            )
            .await,
            "the mesh forms, the history is read and every heartbeat has arrived"
        );
    }
}

/// One limited call at `a`'s gateway.
async fn post(a: &Arc<GossipAgent>) -> Preflight {
    ae_preflight(&a.task_ctx, None, "tools/call", "tool:post", &serde_json::json!({}), &serde_json::json!({}), POINT).await
}

fn admitted(p: &Preflight) -> Option<&AeEvidence> {
    match p {
        Preflight::Proceed(e) => Some(e),
        _ => None,
    }
}

fn refusal(p: &Preflight) -> &PreflightRefusal {
    match p {
        Preflight::Refuse(r) => r,
        Preflight::Proceed(_) => panic!("admitted, expected a refusal"),
        Preflight::Inert => panic!("inert, expected a refusal"),
    }
}

fn said(r: &PreflightRefusal) -> String {
    let d = r.decision();
    format!("{} {} {}", d.reason, d.checked.join(" "), d.errors.join(" "))
}

/// **The exact scope admits at most N across three gateways under concurrent calls** — twelve calls, four at each
/// gateway at once, under *at most three in ten minutes*: at most three are admitted, each holding a different seat
/// decided by consensus; once the seats are full, every gateway denies, naming the scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_consensus_scope_admits_at_most_n_across_three_gateways_under_concurrent_calls() {
    let (p1, p2, p3) = (port(), port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let c = node(p3, &[p1, p2], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b, &c].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b, &c]).await;

    let mut calls = Vec::new();
    for gw in [&a, &b, &c] {
        for _ in 0..4 {
            let gw = Arc::clone(gw);
            calls.push(tokio::spawn(async move { post(&gw).await }));
        }
    }
    let mut outcomes = Vec::new();
    for call in calls {
        outcomes.push(call.await.unwrap());
    }
    let seats: Vec<_> = outcomes
        .iter()
        .filter_map(admitted)
        .map(|e| {
            let h = e.history.as_ref().expect("a fleet decision carries what it read");
            assert_eq!(h.fleet.len(), 1, "{h:?}");
            assert_eq!(h.fleet[0].counter.scope, HistoryScope::FleetConsensus, "the decision names its scope");
            assert_eq!(h.seats.len(), 1, "an admitted call holds a seat: {h:?}");
            h.seats[0].seat
        })
        .collect();
    assert!(!seats.is_empty() && seats.len() <= 3, "at most three of twelve concurrent calls admitted: {seats:?}");
    let mut distinct = seats.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), seats.len(), "each admitted call holds its own seat: {seats:?}");
    for r in outcomes.iter().filter_map(|o| match o {
        Preflight::Refuse(r) => Some(r),
        _ => None,
    }) {
        assert!(said(r).contains("fleet_consensus"), "every refusal names the scope: {r:?}");
    }

    // Filled to the limit, sequentially: then every gateway denies — an authority deciding no, naming the scope.
    let mut total = seats.len();
    for gw in [&a, &b, &c].iter().cycle().take(6) {
        let out = post(gw).await;
        if admitted(&out).is_some() {
            total += 1;
            continue;
        }
        if total == 3 {
            let r = refusal(&out);
            assert!(matches!(r, PreflightRefusal::Denied(_)), "full seats deny: {r:?}");
            assert!(said(r).contains("fleet_consensus"), "{r:?}");
        }
    }
    assert!(total <= 3, "the fleet admitted {total} under a limit of three");
    for gw in [&a, &b, &c] {
        if total == 3 {
            let out = post(gw).await;
            assert!(matches!(refusal(&out), PreflightRefusal::Denied(_)), "{:?}", refusal(&out));
        }
    }
    for n in [a, b, c] {
        n.shutdown().await;
    }
}

/// Sequentially, the exact scope admits exactly N across gateways — the first three calls, wherever they land — and
/// the fourth is denied at a gateway that admitted nothing itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_consensus_scope_admits_exactly_n_spread_over_gateways() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;

    for (i, gw) in [&a, &a, &a].iter().enumerate() {
        let out = post(gw).await;
        assert!(admitted(&out).is_some(), "call {i} is within the fleet's limit: {:?}", refusal(&out));
    }
    // A per-point count would admit three more at `b`; the fleet's counter admits none.
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::Denied(_)), "{r:?}");
    assert!(said(r).contains("fleet_consensus"), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **No quorum gives `Indeterminate`**: a peer that does not vote leaves a two-node cluster without a quorum, so no
/// seat can be decided and the limited call is not established — never admitted, never denied.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn without_a_quorum_the_consensus_scope_is_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetConsensus, |_| {}).await;
    // `b` runs no consensus listener: it is a peer that never votes.
    let b = node(p2, &[p1], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    ready(&[&a, &b]).await;

    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "no quorum is not established: {r:?}");
    let text = said(r);
    assert!(text.contains("fleet_consensus") && text.contains("no quorum"), "{text}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **The tally converges and refuses after convergence**: two permits at `a`; once `a`'s tally has reached `b`, `b`
/// admits the third and refuses the fourth — and `a`, once `b`'s tally reaches it, refuses too. Every decision names
/// the scope, says it is approximate, and carries its coverage.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_tally_scope_converges_and_refuses_after_convergence() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a, &b]).await;

    for i in 0..2 {
        let out = post(&a).await;
        let e = admitted(&out).unwrap_or_else(|| panic!("permit {i} at a: {:?}", refusal(&out)));
        let h = e.history.as_ref().unwrap();
        assert_eq!(h.fleet[0].counter.scope, HistoryScope::FleetTally);
        assert!(e.checked.iter().any(|c| c.contains("history scope=fleet_tally") && c.contains(crate::FLEET_TALLY_CAVEAT)), "{:?}", e.checked);
    }
    // Convergence, observed: `b` holds `a`'s heartbeat and a tally entry with both permits.
    let a_id = a.node_id().to_string();
    let bctx = Arc::clone(&b.task_ctx);
    assert!(
        until(
            || {
                let prefix = format!("{}{a_id}/c/", super::TALLY_PREFIX);
                mycelium_core::ops::kv_get(&bctx, &super::heartbeat_key(&a_id)).is_some()
                    && bctx.kv_state.store.pin().iter().any(|(k, e)| {
                        k.starts_with(&prefix)
                            && e.data.as_ref().and_then(|v| serde_json::from_slice::<serde_json::Value>(v).ok())
                                .is_some_and(|v| v["t"].as_array().is_some_and(|t| t.len() >= 2))
                    })
            },
            Duration::from_secs(20),
        )
        .await,
        "a's tally reaches b"
    );
    let out = post(&b).await;
    let e = admitted(&out).unwrap_or_else(|| panic!("the third, at b: {:?}", refusal(&out)));
    match &e.history.as_ref().unwrap().fleet[0].state {
        FleetState::Tally { counted, publishers, .. } => {
            assert_eq!(*counted, 2, "a's two, summed at b");
            assert!(publishers.contains(&a_id), "a is in the coverage: {publishers:?}");
        }
        other => panic!("{other:?}"),
    }
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::Denied(_)), "the fourth, after convergence: {r:?}");
    assert!(said(r).contains("fleet_tally") && said(r).contains("approximate"), "{r:?}");

    let actx = Arc::clone(&a.task_ctx);
    let b_id = b.node_id().to_string();
    assert!(
        until(|| mycelium_core::ops::kv_get(&actx, &super::heartbeat_key(&b_id)).is_some(), Duration::from_secs(20)).await,
        "b's tally reaches a"
    );
    assert!(
        until(
            || {
                matches!(
                    super::establish(
                        &actx,
                        &[crate::FleetCounter::new(HistoryScope::FleetTally, "c-posts", 3, TEN_MIN)],
                        "gateway",
                        crate::PRINCIPAL_ANONYMOUS,
                        actx.hlc.decision_now_ms(),
                    )[0].state,
                    FleetState::Tally { counted: 3, .. }
                )
            },
            Duration::from_secs(20),
        )
        .await,
        "a sums b's permit"
    );
    let out = post(&a).await;
    assert!(matches!(refusal(&out), PreflightRefusal::Denied(_)), "{:?}", refusal(&out));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Unknown coverage is not established**: a node that has heard from no peer cannot say whose tallies it is
/// missing, so a tally rule decides nothing — not an empty tally, not a permit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_isolated_node_does_not_establish_a_tally() {
    let a = node(port(), &[], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a]).await;
    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    let text = said(r);
    assert!(text.contains("fleet_tally") && text.contains("view of the fleet is not established"), "{text}");
    a.shutdown().await;
}

/// **Stale coverage is not established**: a peer whose heartbeat is older than `control_max_staleness_ms` may have
/// admitted calls this node cannot see, so its count is not zero and the tally decides nothing, naming the peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stale_publisher_makes_the_tally_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a, &b]).await;

    // A member `b` hears from — never started, so no real heartbeat can overwrite the forgery — that last published
    // ten minutes ago.
    let silent = NodeId::new("127.0.0.1", port()).unwrap();
    b.task_ctx.peers.pin().insert(silent.clone(), mycelium_core::sim_seam::mono_instant());
    let a_id = silent.to_string();
    let stale = serde_json::json!({ "as_of": b.task_ctx.hlc.decision_now_ms() - TEN_MIN, "enforces": true });
    let _ = mycelium_core::ops::kv_set(
        &b.task_ctx,
        Arc::from(super::heartbeat_key(&a_id).as_str()),
        bytes::Bytes::from(serde_json::to_vec(&stale).unwrap()),
    );
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    let text = said(r);
    assert!(text.contains("fleet_tally") && text.contains("stale") && text.contains(&a_id), "{text}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Through a door, not only the function**: three posts over `/mcp` at gateway `a` fill the fleet's counter, so a
/// call reaching gateway `b` is denied — the doors run `ae_preflight`, and the count is the fleet's.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_mcp_door_counts_into_the_fleet_counter() {
    let (p1, p2, http) = (port(), port(), port());
    let any_post = |scope| {
        ReferenceEvaluator::new("rev-k2")
            .with_catalogue("cat-k2", "1")
            .map_action("tools/call", "*")
            .allow(Rule::new(crate::PRINCIPAL_ANONYMOUS, "tools/call", "*").at_most(3, TEN_MIN).scoped(scope, "c-posts"))
    };
    let start = |p: u16, peers: Vec<u16>, http_port: Option<u16>| async move {
        let cfg = GossipConfig {
            bind_address: "127.0.0.1".parse().unwrap(),
            bind_port: p,
            http_port,
            bootstrap_peers: peers.iter().map(|q| NodeId::new("127.0.0.1", *q).unwrap()).collect(),
            ..GossipConfig::default()
        };
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p).unwrap(), cfg));
        let dir = std::env::temp_dir().join(format!("k2-door-{p}"));
        let _ = std::fs::remove_dir_all(&dir);
        agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
        agent.with_action_evaluator(Arc::new(any_post(HistoryScope::FleetConsensus)));
        agent.start().await.unwrap();
        agent
    };
    let a = start(p1, vec![], Some(http)).await;
    let b = start(p2, vec![p1], None).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    // A flake (#642), closed: four times in CI (098f6f3c, 518283e4, 2c960486, #646's run 39181781180) a call here
    // timed out in the dispatch although its handler ran. The dispatch is to this node, and `rpc_call_framed`
    // registered for the reply only after emitting the request, so a provider answering from another worker first
    // lost its reply (`rpc::PendingReply`; the deterministic regression is
    // `a_reply_sent_before_the_caller_waits_still_reaches_it`). The instrumentation stays: the tool name is this
    // test's own, the handler counts its runs, and a timeout's diagnostics (`mcp_bridge`'s warning, kept for tests
    // per gateway node) say where the time went.
    let tool_name = format!("k2-door-{p1}");
    let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ran_in = Arc::clone(&ran);
    let _tool = a.mcp().register_mcp_tool(tool_name.as_str(), serde_json::json!({"type": "object", "properties": {}}), move |_args| {
        let ran = Arc::clone(&ran_in);
        async move {
            ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::json!("ran"))
        }
    });
    let client = reqwest::Client::new();
    let call = || async {
        client
            .post(format!("http://127.0.0.1:{http}/mcp"))
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                      "params": {"name": tool_name, "arguments": {}}}))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };
    assert!(
        until(|| a.task_ctx.kv_state.store.pin().keys().any(|k| k.starts_with(&format!("tools/{tool_name}/"))), Duration::from_secs(5)).await,
        "the tool is registered"
    );
    for i in 0..3 {
        let r = call().await;
        assert!(
            r.get("error").is_none(),
            "post {i} over /mcp is within the fleet's limit: {r}; handler ran {} time(s); a's peers {:?}; tool keys {:?}; timeout {:?}",
            ran.load(std::sync::atomic::Ordering::SeqCst),
            a.task_ctx.peers.pin().keys().map(|k| k.to_string()).collect::<Vec<_>>(),
            mycelium_core::ops::kv_scan_prefix(&a.task_ctx, "tools/").into_iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>(),
            super::super::http::TEST_LAST_TIMEOUT.pin().get(&a.node_id().to_string()),
        );
    }
    let r = call().await;
    assert_eq!(r["error"]["code"], -32030, "the fourth over /mcp is denied: {r}");
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::Denied(_)) && said(r).contains("fleet_consensus"), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

// ── #642's review, round 1 ──────────────────────────────────────────────────────────────────────

/// A limited call by `actor` on `resource` at `a`'s gateway.
async fn call_as(a: &Arc<GossipAgent>, actor: &str, resource: &str, point: &str) -> Preflight {
    let who = super::super::gateway_caller::ResolvedPrincipal { principal: actor.to_string(), scopes: Vec::new() };
    ae_preflight(&a.task_ctx, Some(&who), "tools/call", resource, &serde_json::json!({}), &serde_json::json!({}), point).await
}

/// **Finding 1: a claim waiting on a quorum holds up no other call.** `b` never votes, so `a`'s claim for the
/// anonymous actor waits out its ballots. Meanwhile a call by another actor whose history shares the anonymous
/// actor's gate stripe, under a per-point limit, and an uncovered call by the anonymous actor itself, are decided at
/// once — the claim runs after the stripe is released.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_claim_waiting_on_a_quorum_holds_up_no_other_call() {
    let (p1, p2) = (port(), port());
    let anon = crate::PRINCIPAL_ANONYMOUS;
    // An actor on the anonymous actor's stripe, found against a scratch index of the same shape.
    let scratch = super::super::evidence_journal::EvidenceJournal::open(
        std::env::temp_dir().join(format!("k2-stripe-{p1}")).join("e.log"), EvidenceProfile::Strict,
    ).unwrap();
    let index = super::super::evidence_journal::HistoryIndex::new(&scratch);
    let neighbour = (0..10_000)
        .map(|i| format!("oidc:idp/u{i}"))
        .find(|x| std::ptr::eq(index.gate(x), index.gate(anon)))
        .expect("an actor on the same stripe");
    let policy = posts(3, TEN_MIN, HistoryScope::FleetConsensus)
        .allow(Rule::new(neighbour.clone(), "tools/call", "tool:read").at_most(3, TEN_MIN))
        .allow(Rule::new(anon, "tools/call", "tool:free"));
    let a = node_with(p1, &[], policy.clone(), |_| {}).await;
    let b = node_with(p2, &[p1], policy, |_| {}).await; // runs no consensus listener: no quorum
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    ready(&[&a, &b]).await;

    let claimer = Arc::clone(&a);
    let claim = tokio::spawn(async move { post(&claimer).await });
    tokio::time::sleep(Duration::from_millis(300)).await; // the claim is under way
    let started = std::time::Instant::now();
    let other = call_as(&a, &neighbour, "tool:read", POINT).await;
    let uncovered = call_as(&a, anon, "tool:free", POINT).await;
    let took = started.elapsed();
    assert!(admitted(&other).is_some(), "the other actor is within its own rule: {:?}", refusal(&other));
    assert!(admitted(&uncovered).is_some(), "the uncovered call is within its own rule: {:?}", refusal(&uncovered));
    assert!(took < Duration::from_secs(2), "neither waited on the claim: {took:?}");
    assert!(!claim.is_finished(), "the claim was still waiting on its quorum");
    let out = claim.await.unwrap();
    assert!(matches!(refusal(&out), PreflightRefusal::NotEstablished(_)), "{:?}", refusal(&out));
    a.shutdown().await;
    b.shutdown().await;
}

/// A provider pair for the serve-loop tests (#642's review, N1, N2, N5): `a` enforces at the provider and serves
/// `tool:slow@a` and the protected kind `depot.custom`; `b` never votes, so a claim at `a` waits out its quorum. The
/// gateway client `oidc:idp/alice`, holding `mcp:invoke`, is limited at the exact scope by a rule that **requires that
/// scope** — the shape of every guide and contract example; `b`'s own calls are uncovered.
async fn provider_pair(tune_a: impl FnOnce(&mut GossipConfig)) -> (Arc<GossipAgent>, Arc<GossipAgent>, NodeId, NodeId) {
    let (p1, p2) = (port(), port());
    let a_id = NodeId::new("127.0.0.1", p1).unwrap();
    let b_id = NodeId::new("127.0.0.1", p2).unwrap();
    let node_of = super::super::gateway_caller::node_principal;
    let policy = ReferenceEvaluator::new("rev-k2")
        .with_catalogue("cat-k2", "1")
        .map_action("tools/call", "*")
        .map_action("depot.custom", "*")
        .allow(
            Rule::new(ALICE, "tools/call", format!("tool:slow@{a_id}"))
                .requiring_scopes(["mcp:invoke"])
                .at_most(3, TEN_MIN)
                .scoped(HistoryScope::FleetConsensus, "c-slow"),
        )
        .allow(
            Rule::new(ALICE, "tools/call", format!("tool:remote-echo@{a_id}"))
                .requiring_scopes(["mcp:invoke"])
                .at_most(3, TEN_MIN)
                .scoped(HistoryScope::FleetConsensus, "c-bridged"),
        )
        .allow(
            Rule::new(ALICE, "depot.custom", format!("res:x@{a_id}"))
                .requiring_scopes(["mcp:invoke"])
                .at_most(3, TEN_MIN)
                .scoped(HistoryScope::FleetConsensus, "c-custom"),
        )
        .allow(Rule::new(node_of(&b_id), "*", "*"));
    let a = node_with(p1, &[], policy.clone(), |c| {
        c.protected_rpc_kinds = vec!["depot.custom".into()];
        tune_a(c);
    })
    .await;
    let b = node_with(p2, &[p1], policy, |_| {}).await;
    a.with_provider_enforcement();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    std::mem::forget(_listener); // `a` votes for the life of the test; `b` never does
    ready(&[&a, &b]).await;
    (a, b, a_id, b_id)
}

const ALICE: &str = "oidc:idp/alice";

/// `alice`'s call to `target`, framed by `from` as its gateway would, holding `mcp:invoke`.
async fn alice_calls(
    from: &Arc<GossipAgent>, target: &NodeId, kind: &str, resource: Option<&str>, app: Vec<u8>, within: Duration,
) -> Result<bytes::Bytes, super::super::rpc::RpcError> {
    let framed = super::super::gateway_caller::frame_with_context_and_mandate(
        &from.task_ctx, target, ALICE, &["mcp:invoke".to_string()], bytes::Bytes::from(app), None, resource,
    )
    .expect("an envelope");
    super::super::rpc::rpc_call_framed(&from.task_ctx, target.clone(), Arc::from(kind), framed, within).await
}

fn slow_call() -> Vec<u8> {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "slow", "arguments": {}}})
        .to_string()
        .into_bytes()
}

/// **Finding 1 and N1, at the provider:** a check waiting on a claim does not stall its tool's serve loop — for a
/// rule that **requires a scope**, decided from the envelope the check itself builds. `alice`'s call (holding
/// `mcp:invoke`) waits on a quorum `b` will not give, while `b`'s call to the same tool is answered at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_provider_claim_does_not_stall_its_tool_loop() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    let _tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, slow_call(), Duration::from_secs(40)).await });
    tokio::time::sleep(Duration::from_millis(300)).await; // alice's check is waiting on its claim
    let started = std::time::Instant::now();
    let theirs = b.service().rpc_call(a_id.clone(), "mcp.invoke", slow_call(), Duration::from_secs(3)).await;
    let took = started.elapsed();
    let body: serde_json::Value = serde_json::from_slice(&theirs.expect("answered within 3 s")).unwrap();
    assert!(body.get("error").is_none(), "b's uncovered call ran: {body}");
    assert!(took < Duration::from_secs(2), "the tool loop was not held by the claim: {took:?}");
    let own: serde_json::Value = serde_json::from_slice(&hers.await.unwrap().expect("answered")).unwrap();
    let text = own.to_string();
    assert!(own.get("error").is_some() && text.contains("fleet_consensus"), "alice's claim found no quorum: {own}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **N2: the deferred checks are bounded.** With a bound of one, a second waiting call is left in the channel, so the
/// loop stops draining it — the channel's own bound pushes back again — and `b`'s uncovered call behind it waits for
/// the first claim to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_full_set_of_deferred_checks_stops_draining_the_channel() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    super::super::provider_enforcement::TEST_DEFERRED_BOUND.pin().insert(a_id.to_string(), 1);
    let _tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut hers = Vec::new();
    for _ in 0..2 {
        let (from, to) = (Arc::clone(&a), a_id.clone());
        hers.push(tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, slow_call(), Duration::from_secs(60)).await }));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let started = std::time::Instant::now();
    let theirs = b.service().rpc_call(a_id.clone(), "mcp.invoke", slow_call(), Duration::from_secs(60)).await;
    let took = started.elapsed();
    assert!(theirs.is_ok(), "b's call is answered in the end");
    assert!(took > Duration::from_secs(2), "b's call waited behind the full set: {took:?}");
    for h in hers {
        let _ = h.await;
    }
    super::super::provider_enforcement::TEST_DEFERRED_BOUND.pin().remove(&a_id.to_string());
    a.shutdown().await;
    b.shutdown().await;
}

/// **N2: a check that panics is answered**, `authority_not_established`, never left to time out.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_deferred_check_that_panics_is_answered() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    super::TEST_PANIC_CLAIM.pin().insert(a_id.to_string(), ());
    let _tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let answered = alice_calls(&a, &a_id, "mcp.invoke", None, slow_call(), Duration::from_secs(10)).await;
    super::TEST_PANIC_CLAIM.pin().remove(&a_id.to_string());
    let body: serde_json::Value = serde_json::from_slice(&answered.expect("answered, not timed out")).unwrap();
    assert!(body.to_string().contains("authority_not_established"), "{body}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **N5: a serve loop that ends answers what it was still deciding.** The MCP tool withdrawn, and an `rpc_rx`
/// receiver dropped, each while `alice`'s claim waits on a quorum: both calls are answered at once with a refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_serve_loop_that_ends_answers_its_deferred_checks() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    let tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, slow_call(), Duration::from_secs(40)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    drop(tool);
    let body: serde_json::Value = serde_json::from_slice(&hers.await.unwrap().expect("answered")).unwrap();
    assert!(started.elapsed() < Duration::from_secs(3), "answered when the loop ended: {:?}", started.elapsed());
    assert!(body.to_string().contains("authority_not_established"), "{body}");

    let mut rx = a.service().rpc_rx("depot.custom");
    let resource = format!("res:x@{a_id}");
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move {
        alice_calls(&from, &to, "depot.custom", Some(&resource), b"work".to_vec(), Duration::from_secs(40)).await
    });
    let started = std::time::Instant::now();
    assert!(tokio::time::timeout(Duration::from_millis(800), rx.recv()).await.is_err(), "nothing is yielded while it waits");
    drop(rx);
    let body = hers.await.unwrap().expect("answered");
    assert!(started.elapsed() < Duration::from_secs(3), "answered when the receiver was dropped: {:?}", started.elapsed());
    assert!(String::from_utf8_lossy(&body).contains("authority_not_established"), "{}", String::from_utf8_lossy(&body));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 2: a seat decided too late for its window does not admit.** A seat's lease is fixed when it is
/// proposed and the call is admitted after the round; admitting one whose remaining lease is shorter than the window
/// would let the seat reopen, and admit again, within one window. With a margin of one lost ballot a claim is
/// admitted; with the margin cut to half a second, one delayed past it is not established — its seat stays held.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_seat_decided_too_late_for_its_window_is_not_admitted() {
    let (p1, p2) = (port(), port());
    // A 1.5 s window and a 0.5 s margin: a lease of exactly two seconds, so a claim delayed 0.8 s leaves 1.2 s.
    let a = node_with(p1, &[], posts(3, 1_500, HistoryScope::FleetConsensus), |_| {}).await;
    let b = node_with(p2, &[p1], posts(3, 1_500, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    let me = a.node_id().to_string();
    // The first claim gets a margin of at least one ballot lost to its timeouts, so it is admitted under load too
    // (review L1: at 500 ms it was exposed to the flake this margin fixes).
    super::TEST_CLAIM_HOOKS.pin().insert(me.clone(), (4_100, 0));
    let out = post(&a).await;
    assert!(admitted(&out).is_some(), "a claim inside the margin is admitted: {:?}", refusal(&out));
    super::TEST_CLAIM_HOOKS.pin().insert(me.clone(), (500, 800));
    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    assert!(said(r).contains("decided too late"), "{r:?}");
    super::TEST_CLAIM_HOOKS.pin().remove(&me);
    assert!(super::seat_admissible(5_000, 4_000, 1_000) && !super::seat_admissible(5_000, 4_001, 1_000));
    a.shutdown().await;
    b.shutdown().await;
}

/// A claim at `a` whose first `lost` seat prepares `b` leaves unanswered: each lost one costs a prepare phase (2 s)
/// before the next ballot. Returns the preflight and how long it took.
async fn claim_losing_prepares(a: &Arc<GossipAgent>, b: &Arc<GossipAgent>, lost: u32) -> (Preflight, Duration) {
    let left = Arc::new(std::sync::atomic::AtomicU32::new(lost));
    crate::consensus::TEST_SWALLOW_PREPARES
        .pin()
        .insert(b.node_id().to_string(), (super::SEAT_SLOT_PREFIX.to_string(), Arc::clone(&left)));
    let started = std::time::Instant::now();
    let out = post(a).await;
    let took = started.elapsed();
    crate::consensus::TEST_SWALLOW_PREPARES.pin().remove(&b.node_id().to_string());
    assert_eq!(left.load(std::sync::atomic::Ordering::SeqCst), 0, "b left {lost} prepare(s) unanswered");
    (out, took)
}

/// A pair of nodes under *at most three posts in ten minutes* at the consensus scope, both voting.
async fn voting_pair() -> (Arc<GossipAgent>, Arc<GossipAgent>, Vec<crate::ConsensusListenerHandle>) {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let listeners = [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    (a, b, listeners)
}

/// **A claim that loses one ballot to its phase timeout is still admitted.** `b` leaves `a`'s first prepare for a seat
/// unanswered, so the first ballot waits out its prepare phase (2 s) and the second decides the seat. The seat's lease
/// is stamped when the claim's decision envelope is built, before the first ballot, and the call is admitted only if
/// the lease still covers a whole window — so the margin is the latency a claim is allowed. It was 2 s, the length of
/// one phase timeout, so every claim that needed a second ballot was refused *decided too late* (the K2 flake under
/// parallel load). The margin now covers a claim's whole bounded duration.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_claim_that_loses_a_ballot_to_its_timeout_is_admitted() {
    let (a, b, _listeners) = voting_pair().await;
    let (out, took) = claim_losing_prepares(&a, &b, 1).await;
    assert!(took >= Duration::from_secs(2), "the first ballot waited out its prepare phase: {took:?}");
    let e = admitted(&out).unwrap_or_else(|| panic!("a claim decided at its second ballot is admitted: {:?}", refusal(&out)));
    let seat = &e.history.as_ref().unwrap().seats[0];
    // The lease still covers a whole window from admission: one seat cannot admit twice within a window.
    let now = mycelium_core::sim_seam::wall_now_ms();
    assert!(super::seat_admissible(seat.expires_at_ms, now, TEN_MIN), "{seat:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Review L2: two ballots lost, about four seconds, are still within the margin** — a margin sized for one lost
/// phase (2.3 s) refuses this claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_claim_that_loses_two_ballots_to_their_timeouts_is_admitted() {
    let (a, b, _listeners) = voting_pair().await;
    let (out, took) = claim_losing_prepares(&a, &b, 2).await;
    assert!(took >= Duration::from_secs(4), "two prepare phases waited out: {took:?}");
    assert!(admitted(&out).is_some(), "decided at the third ballot, admitted: {:?}", refusal(&out));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Review L1: a lease that lapsed before its decision was read is "too late" at once**, not polled for a second and
/// reported as unobserved. A margin of nothing leases the seat for exactly its one-second window; one lost prepare makes
/// the claim take two seconds, so the decision it reads is already ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_lease_that_lapsed_before_its_decision_was_read_is_too_late() {
    let (p1, p2) = (port(), port());
    let a = node_with(p1, &[], posts(3, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let b = node_with(p2, &[p1], posts(3, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    super::TEST_CLAIM_HOOKS.pin().insert(a.node_id().to_string(), (0, 0));
    let (out, _) = claim_losing_prepares(&a, &b, 1).await;
    super::TEST_CLAIM_HOOKS.pin().remove(&a.node_id().to_string());
    let r = refusal(&out);
    assert!(said(r).contains("decided too late") && said(r).contains("had ended"), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Review Q1: a backward wall-clock step does not admit a slow claim.** The margin is cut to half a second (a lease of
/// the window plus one second) and one lost prepare makes the claim take two; while it waits, the wall clock steps back
/// five seconds, so the wall-clock reading alone says the lease still covers a window. The monotonic reading does not.
/// One thread, so the thread-local step reaches both nodes. Not under `sim`, whose wall clock is the kernel's and has no
/// step hook (as `a_rotation_after_the_clock_steps_back_still_outranks_its_predecessor`).
#[cfg(not(feature = "sim"))]
#[tokio::test(flavor = "current_thread")]
async fn a_backward_clock_step_does_not_admit_a_slow_claim() {
    let (a, b, _listeners) = voting_pair().await;
    super::TEST_CLAIM_HOOKS.pin().insert(a.node_id().to_string(), (500, 0));
    let step = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        mycelium_core::sim_seam::step_wall_clock_for_test(-5_000);
    });
    let (out, took) = claim_losing_prepares(&a, &b, 1).await;
    step.await.unwrap();
    mycelium_core::sim_seam::step_wall_clock_for_test(0);
    super::TEST_CLAIM_HOOKS.pin().remove(&a.node_id().to_string());
    assert!(took >= Duration::from_secs(2), "{took:?}");
    let r = refusal(&out);
    assert!(said(r).contains("decided too late") && said(r).contains("the claim took"), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **The lease is the fallback: a holder shut down within its window leaves the seat to its lease** (#648 round 2). One
/// seat in a one-second window with a five-second margin, so a lease of six seconds, its claim losing one prepare. The holder `a` shuts down right after
/// admission: its release task stops without releasing, because the window may not have passed. The seat is still live at
/// `a` after the shutdown and at `b` past the window, and ends at `b` when the lease lapses.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_holder_shut_down_within_its_window_leaves_the_seat_to_its_lease() {
    let (p1, p2) = (port(), port());
    let a = node_with(p1, &[], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let b = node_with(p2, &[p1], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    // A margin of one lost ballot (as `a_seat_is_reclaimed_after_its_window`), and one prepare lost, so the claim is
    // admitted under load too: a 2 s margin failed it "decided too late".
    super::TEST_CLAIM_HOOKS.pin().insert(a.node_id().to_string(), (5_000, 0));
    let (out, _) = claim_losing_prepares(&a, &b, 1).await;
    let admitted_at = std::time::Instant::now();
    let seat = admitted(&out).unwrap_or_else(|| panic!("admitted: {:?}", refusal(&out))).history.as_ref().unwrap().seats[0].clone();
    super::TEST_CLAIM_HOOKS.pin().remove(&a.node_id().to_string());
    a.shutdown().await;
    assert!(super::seat_live(&a.task_ctx, &seat.slot).is_some(), "a shutdown releases nothing");
    let (b_ctx, slot) = (Arc::clone(&b.task_ctx), seat.slot.clone());
    tokio::time::sleep(Duration::from_millis(1_300).saturating_sub(admitted_at.elapsed())).await;
    assert!(super::seat_live(&b_ctx, &slot).is_some(), "past the window the seat is still held: no release, the lease");
    assert!(until(|| super::seat_live(&b_ctx, &slot).is_none(), Duration::from_secs(8)).await, "the lease ends it");
    b.shutdown().await;
}

/// **A permit whose evidence is not recorded schedules no release** (#648 round 2). `a`'s journal has stalled under the
/// strict profile, so its permit is refused `NotRecorded` after its seat was claimed. Nothing released the seat: it is
/// still held past the window, until its lease ends.
#[cfg(feature = "compliance")]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_permit_not_recorded_schedules_no_release() {
    let (p1, p2) = (port(), port());
    let cfg = GossipConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        bind_port: p1,
        ..GossipConfig::default()
    };
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p1).unwrap(), cfg));
    a.with_evidence_journal(EvidenceJournal::stalled(EvidenceProfile::Strict));
    a.with_action_evaluator(Arc::new(posts(1, 1_000, HistoryScope::FleetConsensus)));
    a.start().await.unwrap();
    let b = node_with(p2, &[p1], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    // A margin of one lost ballot, and one prepare lost: admitted by the seat under load too, then refused by the journal.
    super::TEST_CLAIM_HOOKS.pin().insert(a.node_id().to_string(), (5_000, 0));
    let (out, _) = claim_losing_prepares(&a, &b, 1).await;
    let refused_at = std::time::Instant::now();
    super::TEST_CLAIM_HOOKS.pin().remove(&a.node_id().to_string());
    assert!(matches!(refusal(&out), PreflightRefusal::NotRecorded(_)), "{:?}", refusal(&out));
    let slot = super::seat_slot("c-posts", &super::super::evidence_journal::point_of(POINT), crate::PRINCIPAL_ANONYMOUS, 0);
    assert!(super::seat_live(&a.task_ctx, &slot).is_some(), "the seat was claimed");
    tokio::time::sleep(Duration::from_millis(1_300).saturating_sub(refused_at.elapsed())).await;
    assert!(super::seat_live(&a.task_ctx, &slot).is_some(), "past the window the seat is still held: nothing released it");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Review M1: under a short window a saturating actor gets its limit each window**, not once per window plus the
/// lease margin. One seat in a one-second window, posted for as fast as the gateways answer, alternating between them:
/// the holder releases the seat a window after admission, so it is admitted about once a second — and never twice
/// within a second, measured from before one admitting call to after the next.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_saturating_actor_gets_its_limit_each_short_window() {
    let (p1, p2) = (port(), port());
    let a = node_with(p1, &[], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let b = node_with(p2, &[p1], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    let run = std::time::Instant::now();
    let mut admitted_at: Vec<(std::time::Instant, std::time::Instant)> = Vec::new();
    let mut i = 0usize;
    while run.elapsed() < Duration::from_millis(4_500) {
        let gw = if i.is_multiple_of(2) { &a } else { &b };
        i += 1;
        let before = std::time::Instant::now();
        let out = post(gw).await;
        let after = std::time::Instant::now();
        if admitted(&out).is_some() {
            admitted_at.push((before, after));
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // About one a second: four or five unloaded, at least three under parallel load (the claim's own round is slower);
    // without the release the seat is held for the whole lease and only the first is admitted.
    assert!(admitted_at.len() >= 3, "about one admission a second over 4.5 s, got {}", admitted_at.len());
    assert!(admitted_at.len() <= 5, "never more than one a window: {} in 4.5 s", admitted_at.len());
    for w in admitted_at.windows(2) {
        let (before_first, _) = w[0];
        let (_, after_next) = w[1];
        assert!(after_next.duration_since(before_first) >= Duration::from_millis(1_000), "two admissions within a window");
    }
    // The exact oracle: the admission instants the gateways recorded for the seat, a whole window apart.
    let slot = super::seat_slot("c-posts", &super::super::evidence_journal::point_of(POINT), crate::PRINCIPAL_ANONYMOUS, 0);
    let mut at: Vec<std::time::Instant> = [&a, &b]
        .iter()
        .flat_map(|n| super::TEST_ADMISSIONS.pin().get(&format!("{}|{slot}", n.node_id())).cloned().unwrap_or_default())
        .collect();
    at.sort();
    assert_eq!(at.len(), admitted_at.len(), "one recorded admission per admitted call");
    for w in at.windows(2) {
        let gap = w[1].duration_since(w[0]);
        assert!(gap >= Duration::from_millis(1_000), "admissions on one seat {gap:?} apart, less than the window");
    }
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 12: a seat is reclaimed after its window.** One seat in a one-second window: the first call holds it,
/// the second is denied, and once the seat's lease has ended a third is admitted on the same seat. The margin is cut to
/// five seconds (the hook) so the lease ends within the test while a claim may still lose a ballot to its timeout; the
/// derived margin holds a used seat ~19 s past its window.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_seat_is_reclaimed_after_its_window() {
    let (p1, p2) = (port(), port());
    let a = node_with(p1, &[], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let b = node_with(p2, &[p1], posts(1, 1_000, HistoryScope::FleetConsensus), |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    for n in [&a, &b] {
        super::TEST_CLAIM_HOOKS.pin().insert(n.node_id().to_string(), (5_000, 0));
    }
    let first = post(&a).await;
    let seat = admitted(&first).expect("the first holds the seat").history.as_ref().unwrap().seats[0].clone();
    assert!(matches!(refusal(&post(&b).await), PreflightRefusal::Denied(_)), "the window is full");
    // Ended by the holder's release a window after admission — read on `b`, the node that posts next, which sees it
    // only once the release marker arrives (reading it on `a`, the holder, raced the gossip to `b`).
    let ctx = Arc::clone(&b.task_ctx);
    let slot = seat.slot.clone();
    assert!(until(|| super::seat_live(&ctx, &slot).is_none(), Duration::from_secs(15)).await, "the seat ends at b");
    let third = post(&b).await;
    let again = admitted(&third).unwrap_or_else(|| panic!("reclaimed: {:?}", refusal(&third)));
    assert_eq!(again.history.as_ref().unwrap().seats[0].slot, seat.slot, "the same seat, decided afresh");
    for n in [&a, &b] {
        super::TEST_CLAIM_HOOKS.pin().remove(&n.node_id().to_string());
    }
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 12: the electorate decides the seats.** Under `consensus_require_electorate`, a seat outside an
/// electorate is refused, so the limit is not established; once the fleet's `exclusive_default` electorate is
/// declared, the seat is decided there and the call admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_a_required_electorate_the_seats_are_decided_in_it() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let p1 = port();
    let base = std::env::temp_dir().join(format!("k2-electorate-{p1}"));
    let _ = std::fs::remove_dir_all(&base);
    let a = node_with(p1, &[], posts(3, TEN_MIN, HistoryScope::FleetConsensus), |c| {
        c.consensus_require_electorate = true;
        c.persistence = Some(PersistenceConfig {
            base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000,
            snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse,
        });
    })
    .await;
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    ready(&[&a]).await;
    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)) && said(r).contains("ElectorateNotGoverned"), "{r:?}");

    a.mesh().join_group("council");
    let ctx = Arc::clone(&a.task_ctx);
    let member = format!("{}{}", crate::signal::grp_prefix("council"), a.node_id());
    assert!(until(|| mycelium_core::ops::kv_get(&ctx, &member).is_some(), Duration::from_secs(5)).await);
    let mut declared = false;
    for _ in 0..5 {
        if a.declare_electorate("council", true).await.is_ok() {
            declared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(declared, "the electorate is declared");
    let out = post(&a).await;
    assert!(admitted(&out).is_some(), "decided in the fleet's electorate: {:?}", refusal(&out));
    a.shutdown().await;
}

/// **Finding 12: the gateway and the provider count apart, fleet-wide as at one node.** The gateway's three seats
/// full, the provider check of the same rule still has its own three — a call the gateway admitted and the provider
/// re-checks holds a seat at each, never two of one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_gateway_and_the_provider_count_apart() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetConsensus, |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    for _ in 0..3 {
        let out = post(&a).await;
        assert!(admitted(&out).is_some(), "within the limit, admitted: {:?}", refusal(&out));
    }
    assert!(matches!(refusal(&post(&a).await), PreflightRefusal::Denied(_)), "the gateway's seats are full");
    let at_provider = call_as(&b, crate::PRINCIPAL_ANONYMOUS, "tool:post", "provider").await;
    assert!(admitted(&at_provider).is_some(), "the provider counts apart: {:?}", refusal(&at_provider));
    assert_ne!(super::seat_slot("c-posts", "gateway", "x", 0), super::seat_slot("c-posts", "provider", "x", 0));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 12: a wrapper that does not forward `fleet_counters` decides `Indeterminate`.** It forwards the history
/// methods but hides the inner evaluator's counters, so the enforcement point gives no fleet count and the
/// fleet-limited rule is not established — closed, never a permit on a count nobody read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrapper_that_hides_the_fleet_counters_is_not_established() {
    use crate::{ActionEnvelope, ActionEvaluator, Decision, History};
    struct Wrapper(ReferenceEvaluator);
    impl ActionEvaluator for Wrapper {
        fn evaluate(&self, env: &ActionEnvelope) -> Decision {
            self.0.evaluate(env)
        }
        fn mapping(&self, operation: &str, resource: &str) -> crate::ActionMapping {
            self.0.mapping(operation, resource)
        }
        fn history_request(&self) -> Option<crate::HistoryRequest> {
            self.0.history_request()
        }
        fn evaluate_with_history(&self, env: &ActionEnvelope, history: &History) -> Decision {
            self.0.evaluate_with_history(env, history)
        }
    }
    let p1 = port();
    let mut cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: p1, ..GossipConfig::default() };
    cfg.bootstrap_peers = Vec::new();
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p1).unwrap(), cfg));
    let dir = std::env::temp_dir().join(format!("k2-wrapper-{p1}"));
    let _ = std::fs::remove_dir_all(&dir);
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    a.with_action_evaluator(Arc::new(Wrapper(posts(3, TEN_MIN, HistoryScope::FleetConsensus))));
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    ready(&[&a]).await;
    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    assert!(said(r).contains("gave no count"), "{r:?}");
    a.shutdown().await;
}

/// **Finding 3: a restarted node keeps its heartbeat fresh without admitting anything.** `a` restarts and admits
/// nothing; after more than the staleness bound, `b`'s tally is still established — its heartbeat runs from start.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_restarted_node_keeps_its_heartbeat_fresh_without_permits() {
    let (p1, p2) = (port(), port());
    let tune = |c: &mut GossipConfig| c.control_max_staleness_ms = 1_500;
    let a = node(p1, &[p2], 3, HistoryScope::FleetTally, tune).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, tune).await;
    ready(&[&a, &b]).await;
    let a_id = a.node_id().to_string();
    a.shutdown().await;
    drop(a);
    // Whatever `b` last heard from `a` — its departed heartbeat, delivered by shutdown's best-effort flush, or its last
    // live one — the restarted `a` must make current again without admitting anything.
    let a = node(p1, &[p2], 3, HistoryScope::FleetTally, tune).await;
    ready(&[&a, &b]).await;
    tokio::time::sleep(Duration::from_millis(2_000)).await; // past the staleness bound, a admitting nothing
    let out = post(&b).await;
    let e = admitted(&out).unwrap_or_else(|| panic!("a's coverage is current: {:?}", refusal(&out)));
    match &e.history.as_ref().unwrap().fleet[0].state {
        FleetState::Tally { publishers, .. } => assert!(publishers.contains(&a_id), "{publishers:?}"),
        other => panic!("{other:?}"),
    }
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 4: a tally entry is withdrawn once every time in it has aged out** — and not a millisecond before: at the
/// window's last instant the entry still counts (`counted_in` counts a time `t > now - w`), so it stays; at its end it is
/// withdrawn. The instants are the entry's own, passed in, so the test reads no clock: it used a 300 ms window and the
/// node's clock, and under parallel load more than 300 ms could pass between the permit and the check (or the node's
/// own withdrawal tick could take the entry first), failing "still inside its window" with nothing wrong. The window is
/// ten minutes so neither the clock nor the tick can reach it while the test runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_aged_tally_entry_is_withdrawn() {
    let (p1, p2) = (port(), port());
    let a = node_with(p1, &[], posts(3, TEN_MIN, HistoryScope::FleetTally), |_| {}).await;
    let b = node_with(p2, &[p1], posts(3, TEN_MIN, HistoryScope::FleetTally), |_| {}).await;
    ready(&[&a, &b]).await;
    let out = post(&a).await;
    assert!(admitted(&out).is_some(), "within the limit, admitted: {:?}", refusal(&out));
    let ctx = Arc::clone(&a.task_ctx);
    let prefix = format!("{}{}/c/", super::TALLY_PREFIX, a.node_id());
    let published = mycelium_core::ops::kv_scan_prefix(&ctx, &prefix);
    assert_eq!(published.len(), 1, "the permit is published");
    let entry: super::TallyEntry = serde_json::from_slice(&published[0].1).expect("a tally entry");
    assert_eq!(entry.w, TEN_MIN, "kept for the rule's window");
    let last = *entry.t.iter().max().expect("the permit's time");
    assert_eq!(super::withdraw_aged_entries(&ctx, last + entry.w - 1), 0, "still inside its window at its last instant");
    assert_eq!(mycelium_core::ops::kv_scan_prefix(&ctx, &prefix).len(), 1, "kept");
    assert_eq!(super::withdraw_aged_entries(&ctx, last + entry.w), 1, "aged out at the window's end");
    assert!(mycelium_core::ops::kv_scan_prefix(&ctx, &prefix).is_empty(), "withdrawn");
    a.shutdown().await;
    b.shutdown().await;
}

/// **The heartbeat tick withdraws aged entries itself** — nothing in the test calls the withdrawal. A 300 ms window and a
/// tick every 500 ms (a third of a 1.5 s staleness bound): once the permit ages out, a tick takes the entry. Waited for
/// with a generous deadline (thirty ticks), so the assertion is only that it goes, never when.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_heartbeat_tick_withdraws_an_aged_entry() {
    let (p1, p2) = (port(), port());
    let tune = |c: &mut GossipConfig| c.control_max_staleness_ms = 1_500;
    let a = node_with(p1, &[], posts(3, 300, HistoryScope::FleetTally), tune).await;
    let b = node_with(p2, &[p1], posts(3, 300, HistoryScope::FleetTally), tune).await;
    ready(&[&a, &b]).await;
    let out = post(&a).await;
    assert!(admitted(&out).is_some(), "within the limit, admitted: {:?}", refusal(&out));
    let ctx = Arc::clone(&a.task_ctx);
    let prefix = format!("{}{}/c/", super::TALLY_PREFIX, a.node_id());
    assert!(
        until(|| mycelium_core::ops::kv_scan_prefix(&ctx, &prefix).is_empty(), Duration::from_secs(15)).await,
        "the node's own tick withdrew its aged entry"
    );
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 6: a peer with no heartbeat is unknown coverage**, never counted as zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_peer_with_no_heartbeat_makes_the_tally_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a, &b]).await;
    // A member `b` hears from that has published nothing.
    let silent = NodeId::new("127.0.0.1", port()).unwrap();
    b.task_ctx.peers.pin().insert(silent.clone(), mycelium_core::sim_seam::mono_instant());
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    assert!(said(r).contains(&silent.to_string()) && said(r).contains("no heartbeat"), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Finding 8: a heartbeat from beyond the clock-drift bound is unknown coverage**, not fresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_heartbeat_from_the_future_makes_the_tally_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |c| c.max_clock_drift_ms = 1_000).await;
    ready(&[&a, &b]).await;
    // A never-started member, so no real heartbeat can overwrite the forgery.
    let silent = NodeId::new("127.0.0.1", port()).unwrap();
    b.task_ctx.peers.pin().insert(silent.clone(), mycelium_core::sim_seam::mono_instant());
    let a_id = silent.to_string();
    let future = serde_json::json!({ "as_of": b.task_ctx.hlc.decision_now_ms() + TEN_MIN, "enforces": true });
    let _ = mycelium_core::ops::kv_set(
        &b.task_ctx,
        Arc::from(super::heartbeat_key(&a_id).as_str()),
        bytes::Bytes::from(serde_json::to_vec(&future).unwrap()),
    );
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    assert!(said(r).contains("in the future") && said(r).contains(&a_id), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}


/// **N6: an entry an enforcing peer published that this node cannot read is unknown coverage**, never zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_unreadable_tally_entry_makes_the_tally_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a, &b]).await;
    let a_id = a.node_id().to_string();
    let key = super::tally_key(&a_id, "c-posts", "gateway", crate::PRINCIPAL_ANONYMOUS);
    let _ = mycelium_core::ops::kv_set(&b.task_ctx, Arc::from(key.as_str()), bytes::Bytes::from_static(b"not a tally"));
    let out = post(&b).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)), "{r:?}");
    assert!(said(r).contains("cannot read") && said(r).contains(&a_id), "{r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **N7: a permit published while an aged entry is being withdrawn is not lost.** The withdrawal reads the aged entry
/// and pauses before its delete; a permit for the same actor lands meanwhile. Under the tally lock it waits for the
/// withdrawal and writes after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_permit_published_during_a_withdrawal_is_not_lost() {
    let a = node(port(), &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let ctx = Arc::clone(&a.task_ctx);
    let counter = crate::FleetCounter::new(HistoryScope::FleetTally, "c-race", 3, 300);
    let now = ctx.hlc.decision_now_ms();
    super::publish_permit(&ctx, &counter, "gateway", "actor", now - 1_000); // aged out of its 300 ms window
    let key = super::tally_key(&ctx.node_id.to_string(), "c-race", "gateway", "actor");
    super::TEST_WITHDRAW_PAUSE_MS.pin().insert(ctx.node_id.to_string(), 300);
    let wctx = Arc::clone(&ctx);
    let withdrawal = std::thread::spawn(move || super::withdraw_aged_entries(&wctx, now));
    std::thread::sleep(Duration::from_millis(100)); // the withdrawal has read the aged entry and is pausing
    super::publish_permit(&ctx, &counter, "gateway", "actor", now);
    assert_eq!(withdrawal.join().unwrap(), 1);
    super::TEST_WITHDRAW_PAUSE_MS.pin().remove(&ctx.node_id.to_string());
    let entry = mycelium_core::ops::kv_get(&ctx, &key).expect("the fresh permit survives the withdrawal");
    assert!(String::from_utf8_lossy(&entry).contains(&now.to_string()), "{}", String::from_utf8_lossy(&entry));
    a.shutdown().await;
}

/// **A routed call takes part in fleet counting** (post-360 rows R and K2 together): a routing door's preflight is
/// decided and settled as any other, so its permit claims a seat, and its evidence carries both the seat and the route.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_routed_call_claims_a_seat_and_carries_its_route() {
    use crate::agent::route_facts::{Destination, Locality, RouteFacts};
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 1, HistoryScope::FleetConsensus, |_| {}).await;
    let b = node(p2, &[p1], 1, HistoryScope::FleetConsensus, |_| {}).await;
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    let route = RouteFacts::new(Destination::new("llm/m@n1", Locality::InDomain));
    let routed = |gw: Arc<GossipAgent>, route: RouteFacts| async move {
        super::super::http::ae_preflight_routed(
            &gw.task_ctx, None, "tools/call", "tool:post", &serde_json::json!({}), &serde_json::json!({}), POINT, Some(&route),
        )
        .await
    };
    let first = routed(Arc::clone(&a), route.clone()).await;
    let e = admitted(&first).unwrap_or_else(|| panic!("the routed call is within the limit: {:?}", refusal(&first)));
    assert_eq!(e.history.as_ref().unwrap().seats.len(), 1, "the routed permit holds a seat");
    assert!(e.route.is_some(), "and its evidence carries the route");
    let second = routed(Arc::clone(&b), route).await;
    let r = refusal(&second);
    assert!(matches!(r, PreflightRefusal::Denied(_)) && said(r).contains("fleet_consensus"), "the seat is the fleet's: {r:?}");
    a.shutdown().await;
    b.shutdown().await;
}

// ── #642's review, round 3 ──────────────────────────────────────────────────────────────────────

/// **Round 3, finding 1: the bridged MCP loop does not stall on a claim.** An external MCP server bridged at `a`
/// (`connect_mcp_server`) serves `remote-echo`; `alice`'s call waits on a quorum `b` will not give, while `b`'s call to
/// the same bridged tool is forwarded and answered at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bridged_mcp_claim_does_not_stall_its_loop() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    let tools = vec![serde_json::json!({"name": "remote-echo", "inputSchema": {"type": "object"}})];
    let (mock_port, _mock) = super::super::mcp::tests::spawn_mock_mcp_server(tools).await;
    let _bridge = a.mcp().connect_mcp_server(format!("http://127.0.0.1:{mock_port}/")).await.expect("bridged");
    let call = || {
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "remote-echo", "arguments": {}}})
            .to_string()
            .into_bytes()
    };
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, call(), Duration::from_secs(40)).await });
    tokio::time::sleep(Duration::from_millis(300)).await; // alice's check is waiting on its claim
    let started = std::time::Instant::now();
    let theirs = b.service().rpc_call(a_id.clone(), "mcp.invoke", call(), Duration::from_secs(3)).await;
    let took = started.elapsed();
    let body: serde_json::Value = serde_json::from_slice(&theirs.expect("answered within 3 s")).unwrap();
    assert!(body.to_string().contains("echo:remote-echo"), "b's call was forwarded: {body}");
    assert!(took < Duration::from_secs(2), "the bridged loop was not held by the claim: {took:?}");
    let own: serde_json::Value = serde_json::from_slice(&hers.await.unwrap().expect("answered")).unwrap();
    assert!(own.get("error").is_some(), "alice's claim found no quorum: {own}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 3, finding 1: the `rpc/serve` stream does not stall on a claim.** An SDK agent serves `depot.custom` through
/// `GET /gateway/rpc/serve/depot.custom` at `a`; `alice`'s call waits on a quorum, while `b`'s call behind it is checked
/// and answered at once (refused: it names no resource) rather than waiting for the claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_rpc_serve_stream_does_not_stall_on_a_claim() {
    let http = port();
    let (a, b, a_id, _) = provider_pair(|c| c.http_port = Some(http)).await;
    let mut stream = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http}/gateway/rpc/serve/depot.custom"))
        .send()
        .await
        .expect("the serve stream opens");
    assert_eq!(stream.status(), 200);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let resource = format!("res:x@{a_id}");
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move {
        alice_calls(&from, &to, "depot.custom", Some(&resource), b"work".to_vec(), Duration::from_secs(40)).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await; // alice's check is waiting on its claim
    let started = std::time::Instant::now();
    let theirs = b.service().rpc_call(a_id.clone(), "depot.custom", b"work".to_vec(), Duration::from_secs(3)).await;
    let took = started.elapsed();
    let body = theirs.expect("answered within 3 s");
    assert!(String::from_utf8_lossy(&body).contains("resource_unnamed"), "{}", String::from_utf8_lossy(&body));
    assert!(took < Duration::from_secs(2), "the serve stream was not held by the claim: {took:?}");
    let own = hers.await.unwrap().expect("answered");
    assert!(String::from_utf8_lossy(&own).contains("fleet_consensus"), "{}", String::from_utf8_lossy(&own));
    let _ = stream.chunk().await;
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 3, finding 4: an abandoned check stops.** `alice`'s claim is waiting when the tool is withdrawn; she is
/// answered at once — and after the claim would have timed out, nothing was recorded for her and her provisional
/// permit was taken back: the check stopped with the loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_abandoned_check_records_nothing_and_counts_nothing() {
    let (a, b, a_id, _) = provider_pair(|_| {}).await;
    let tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, slow_call(), Duration::from_secs(40)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(tool);
    let body = hers.await.unwrap().expect("answered");
    assert!(String::from_utf8_lossy(&body).contains("authority_not_established"));
    // Past the claim's own bound (2 s × 4 ballots): a check still running would have recorded its refusal by now.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let journal = std::env::temp_dir().join(format!("k2-fleet-{}", a_id.to_string().rsplit(':').next().unwrap())).join("evidence.log");
    let alice_records = super::super::evidence_journal::read_evidence_journal(&journal)
        .unwrap()
        .iter()
        .filter_map(|b| serde_json::from_slice::<AeEvidence>(b).ok())
        .filter(|e| e.subject == ALICE)
        .count();
    assert_eq!(alice_records, 0, "nothing recorded for a call answered withdrawn");
    let index = a.task_ctx.history_index.get().unwrap();
    let crate::History::View(view) =
        index.view("provider", ALICE, crate::HistoryRequest { window_ms: TEN_MIN, max_entries: usize::MAX }, a.task_ctx.hlc.decision_now_ms()).0
    else {
        panic!("a view")
    };
    assert!(view.entries.is_empty(), "the provisional permit was taken back: {:?}", view.entries);
    a.shutdown().await;
    b.shutdown().await;
}



// ── #642's review, round 4 ──────────────────────────────────────────────────────────────────────

/// A TLS node at `p` dialling `peers`, sharing `cert_dir`'s fleet CA, with a journal and `policy`: members whose
/// identities verify, as the non-peer rule needs.
async fn node_tls(
    p: u16, peers: &[u16], policy: ReferenceEvaluator, cert_dir: &std::path::Path, tune: impl FnOnce(&mut GossipConfig),
) -> Arc<GossipAgent> {
    let dir = cert_dir.to_path_buf();
    node_with(p, peers, policy, move |c| {
        c.tls = Some(crate::TlsConfig { auto_cert_dir: dir, ..Default::default() });
        tune(c);
    })
    .await
}

/// [`ready`], and every node trusts every other's identity.
async fn ready_tls(nodes: &[&Arc<GossipAgent>]) {
    ready(nodes).await;
    for a in nodes {
        let ctx = Arc::clone(&a.task_ctx);
        let others: Vec<NodeId> = nodes.iter().map(|o| o.node_id().clone()).filter(|o| o != a.node_id()).collect();
        assert!(
            until(|| others.iter().all(|o| !ctx.trusted_keys(o).is_empty()), Duration::from_secs(20)).await,
            "every identity verifies"
        );
    }
}

fn tally_counted(e: &AeEvidence) -> (u32, Vec<String>) {
    match &e.history.as_ref().unwrap().fleet[0].state {
        FleetState::Tally { counted, publishers, .. } => (*counted, publishers.clone()),
        other => panic!("{other:?}"),
    }
}

/// **N3, round 4: a graceful leave keeps the tally established** — through the leave and after the departed node
/// stops being a peer, its recent permit still counts (a verified member).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_graceful_leave_keeps_the_tally_established() {
    let (p1, p2, p3) = (port(), port(), port());
    let certs = std::env::temp_dir().join(format!("k2-tls-leave-{p1}"));
    let _ = std::fs::remove_dir_all(&certs);
    let a = node_tls(p1, &[], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    let b = node_tls(p2, &[p1], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    // A third member, so `a` still hears the fleet once `b` is gone.
    let c = node_tls(p3, &[p1, p2], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    ready_tls(&[&a, &b, &c]).await;
    let out = post(&b).await;
    assert!(admitted(&out).is_some(), "b admits one: {:?}", refusal(&out));
    let b_id = b.node_id().to_string();
    let actx = Arc::clone(&a.task_ctx);
    let prefix = format!("{}{b_id}/c/", super::TALLY_PREFIX);
    assert!(until(|| !mycelium_core::ops::kv_scan_prefix(&actx, &prefix).is_empty(), Duration::from_secs(10)).await, "b's entry reaches a");
    // The departed heartbeat travels on shutdown's best-effort flush; whether or not it arrives, b's last heartbeat is
    // fresh and its permit counts.
    b.shutdown().await;
    for leave in ["while b is still a peer", "after b is no longer a peer"] {
        if leave.starts_with("after") {
            actx.peers.pin().remove(&b_id.parse::<NodeId>().unwrap());
        }
        let out = post(&a).await;
        let e = admitted(&out).unwrap_or_else(|| panic!("established {leave}: {:?}", refusal(&out)));
        let (counted, publishers) = tally_counted(e);
        assert!(counted >= 1 && publishers.contains(&b_id), "b's permit still counts {leave}: {counted} {publishers:?}");
    }
    a.shutdown().await;
    c.shutdown().await;
}

/// **Round 3 finding 6, round 4 finding 2: a dead member's permits count; a fabricated id's do not.** `c`, a member,
/// admits one and goes; once it is no longer `b`'s peer its permit still counts. A heartbeat and a full set of entries
/// written under a node id nobody issued an identity for count for nothing — they deny nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_dead_members_permits_count_and_a_fabricated_id_counts_nothing() {
    let (p1, p2, p3) = (port(), port(), port());
    let certs = std::env::temp_dir().join(format!("k2-tls-dead-{p1}"));
    let _ = std::fs::remove_dir_all(&certs);
    let a = node_tls(p1, &[], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    let b = node_tls(p2, &[p1], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    let c = node_tls(p3, &[p1, p2], posts(3, TEN_MIN, HistoryScope::FleetTally), &certs, |_| {}).await;
    ready_tls(&[&a, &b, &c]).await;

    // A fabricated id, current and enforcing, holding the whole limit for the actor.
    let fake = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    let now = b.task_ctx.hlc.decision_now_ms();
    for (key, value) in [
        (super::heartbeat_key(&fake), serde_json::json!({"as_of": now, "enforces": true})),
        (
            super::tally_key(&fake, "c-posts", "gateway", crate::PRINCIPAL_ANONYMOUS),
            serde_json::json!({"w": TEN_MIN, "t": [now - 3, now - 2, now - 1]}),
        ),
    ] {
        let _ = mycelium_core::ops::kv_set(&b.task_ctx, Arc::from(key.as_str()), bytes::Bytes::from(serde_json::to_vec(&value).unwrap()));
    }
    let out = post(&b).await;
    let e = admitted(&out).unwrap_or_else(|| panic!("a fabricated id denies nobody: {:?}", refusal(&out)));
    assert!(!tally_counted(e).1.contains(&fake), "the fabricated id is not a publisher");

    // A member that admits one and goes.
    let out = post(&c).await;
    assert!(admitted(&out).is_some(), "within the limit, admitted: {:?}", refusal(&out));
    let c_id = c.node_id().to_string();
    let bctx = Arc::clone(&b.task_ctx);
    let prefix = format!("{}{c_id}/c/", super::TALLY_PREFIX);
    assert!(until(|| !mycelium_core::ops::kv_scan_prefix(&bctx, &prefix).is_empty(), Duration::from_secs(10)).await);
    c.shutdown().await;
    bctx.peers.pin().remove(&c_id.parse::<NodeId>().unwrap());
    let out = post(&b).await;
    let e = admitted(&out).unwrap_or_else(|| panic!("established: {:?}", refusal(&out)));
    let (counted, publishers) = tally_counted(e);
    assert!(counted >= 2 && publishers.contains(&c_id), "c's permit still counts once c is gone: {counted} {publishers:?}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 4, finding 1: residue is collected, and a decision does not read it.** Twenty departed ids, long silent,
/// with aged entries: one collection pass tombstones every key they had. And a decision reads the same number of keys
/// whether a silent node holds one entry or two hundred.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn residue_is_collected_and_a_decision_does_not_read_it() {
    let (p1, p2) = (port(), port());
    let tune = |c: &mut GossipConfig| {
        c.control_max_staleness_ms = 1_500;
        c.max_clock_drift_ms = 1_000;
    };
    let a = node(p1, &[], 3, HistoryScope::FleetTally, tune).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, tune).await;
    ready(&[&a, &b]).await;
    let ctx = Arc::clone(&b.task_ctx);
    let now = ctx.hlc.decision_now_ms();
    let plant = |key: String, value: serde_json::Value| {
        let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(key.as_str()), bytes::Bytes::from(serde_json::to_vec(&value).unwrap()));
    };
    let counter = crate::FleetCounter::new(HistoryScope::FleetTally, "c-posts", 3, TEN_MIN);
    let reads = |c: &crate::FleetCounter| super::tally_state_counted(&ctx, c, "gateway", crate::PRINCIPAL_ANONYMOUS, ctx.hlc.decision_now_ms()).1;

    // One silent node, one entry, then two hundred: the decision's reads do not grow. The node is planted inside
    // the horizon (3.5 s), so `b`'s own residue collector cannot remove it between the measurements — it once did,
    // past the horizon, and the second count came out *lower* (CI on #645). The claim is "does not grow", so `<=`.
    let silent = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    plant(super::heartbeat_key(&silent), serde_json::json!({"as_of": now, "enforces": true, "departed": true}));
    plant(super::tally_key(&silent, "c-posts", "gateway", "actor-0"), serde_json::json!({"w": 1_000, "t": [now]}));
    let with_one = reads(&counter);
    assert!(with_one >= 2, "the silent node's heartbeat and entry are read: {with_one}");
    for i in 1..200 {
        plant(super::tally_key(&silent, "c-posts", "gateway", &format!("actor-{i}")), serde_json::json!({"w": 1_000, "t": [now - 4_000]}));
    }
    assert!(reads(&counter) <= with_one, "a decision's cost does not grow with a node's entries");
    // Nor with another scoped namespace's scopes: two thousand consensus slots on this node (round 5, finding 1).
    for slot in 0..2_000 {
        let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(format!("consensus/life/k2-{slot}/r/1").as_str()), bytes::Bytes::from_static(b"x"));
    }
    assert!(reads(&counter) <= with_one, "a decision's cost does not grow with consensus slots");

    // Twenty departed ids, silent past the horizon (their longest window + staleness + drift = 3.5 s).
    let ids: Vec<String> = (0..20).map(|_| NodeId::new("127.0.0.1", port()).unwrap().to_string()).collect();
    for id in &ids {
        plant(super::heartbeat_key(id), serde_json::json!({"as_of": now - 10_000, "enforces": true, "departed": true}));
        plant(super::tally_key(id, "c-posts", "gateway", "x"), serde_json::json!({"w": 1_000, "t": [now - 9_000]}));
    }
    // Collected by this call or by `b`'s own collector tick before it — which one is not the claim; the keys' absence is.
    super::collect_residue(&ctx, ctx.hlc.decision_now_ms());
    for id in &ids {
        for key in [super::heartbeat_key(id), super::tally_key(id, "c-posts", "gateway", "x")] {
            assert!(mycelium_core::ops::kv_get(&ctx, &key).is_none(), "{key} collected");
        }
    }
    a.shutdown().await;
    b.shutdown().await;
}

/// **An entry whose heartbeat was collected before it is still collected** (the residue flake on #654's CI). A
/// collector removes a departed node's heartbeat and the entries it holds at that moment; an entry of that node that
/// reaches it afterwards — anti-entropy from a peer that had not collected yet, or a gossiped entry overtaken by its
/// heartbeat — has no heartbeat to date it by. It used to be skipped on every pass, and stayed in the store and in the
/// scope roster forever. It is collected once every time in it is past its window, the staleness bound and the
/// clock-drift bound; one that could still count is kept, and one that cannot be read (an entry or a heartbeat) goes
/// once nothing of the node could still count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_outliving_its_collected_heartbeat_is_collected() {
    let tune = |c: &mut GossipConfig| {
        c.control_max_staleness_ms = 1_500;
        c.max_clock_drift_ms = 1_000;
    };
    let b = node(port(), &[], 3, HistoryScope::FleetTally, tune).await;
    let ctx = Arc::clone(&b.task_ctx);
    let plant = |key: &str, value: serde_json::Value| {
        let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(key), bytes::Bytes::from(serde_json::to_vec(&value).unwrap()));
    };
    let gone = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    let now = ctx.hlc.decision_now_ms();
    // The heartbeat alone, past the horizon: collected.
    plant(&super::heartbeat_key(&gone), serde_json::json!({"as_of": now - 10_000, "enforces": true, "departed": true}));
    super::collect_residue(&ctx, ctx.hlc.decision_now_ms());
    assert!(mycelium_core::ops::kv_get(&ctx, &super::heartbeat_key(&gone)).is_none(), "the heartbeat is collected");

    // Its entries arrive afterwards: one aged past window + staleness + drift (1 + 1.5 + 1 s), one still inside it,
    // one unreadable.
    let (aged, fresh, garbled) = (
        super::tally_key(&gone, "c-posts", "gateway", "aged"),
        super::tally_key(&gone, "c-posts", "gateway", "fresh"),
        super::tally_key(&gone, "c-posts", "gateway", "garbled"),
    );
    plant(&aged, serde_json::json!({"w": 1_000, "t": [now - 9_000]}));
    plant(&fresh, serde_json::json!({"w": 1_000, "t": [now - 1_000]}));
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(garbled.as_str()), bytes::Bytes::from_static(b"not json"));
    super::collect_residue(&ctx, ctx.hlc.decision_now_ms());
    assert!(mycelium_core::ops::kv_get(&ctx, &aged).is_none(), "an aged entry without a heartbeat is collected");
    assert!(mycelium_core::ops::kv_get(&ctx, &fresh).is_some(), "an entry that could still count is kept");
    assert!(mycelium_core::ops::kv_get(&ctx, &garbled).is_some(), "an entry nothing can be read from is kept");

    // Once its last time passes the margin, it goes too — and with no entry left that could count, so does the one
    // nothing can be read from: otherwise it, and the node's row in the scope roster, would stay for ever.
    super::collect_residue(&ctx, now + 3_000);
    assert!(mycelium_core::ops::kv_get(&ctx, &fresh).is_none(), "collected once past the margin");
    assert!(mycelium_core::ops::kv_get(&ctx, &garbled).is_none(), "an unreadable entry goes with the node's last datable one");

    // A heartbeat that cannot be read dates nothing either: an aged entry beside it is collected, and so is it (the
    // adversarial review of #656, finding 2).
    let other = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(super::heartbeat_key(&other).as_str()), bytes::Bytes::from_static(b"not json"));
    plant(&super::tally_key(&other, "c-posts", "gateway", "aged"), serde_json::json!({"w": 1_000, "t": [now - 9_000]}));
    super::collect_residue(&ctx, ctx.hlc.decision_now_ms());
    for key in [super::heartbeat_key(&other), super::tally_key(&other, "c-posts", "gateway", "aged")] {
        assert!(mycelium_core::ops::kv_get(&ctx, &key).is_none(), "{key} collected");
    }
    b.shutdown().await;
}

/// **Round 4, finding 1: a returning node's newer records survive a collector that held its old ones**, and its
/// `sys/` tripwire counts nothing. `a` departs; `b`, a peer no longer, collects `a`'s keys at their own stamps; `a`
/// returns under `[persistence]` — re-asserting its keys before its connections come up — and its heartbeat is current
/// at both nodes, with no foreign write counted. A collector's tombstone, stamped at an old record, never removes a
/// newer one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_returning_nodes_newer_records_survive_a_collector() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let (p1, p2) = (port(), port());
    let persist = |p: u16| {
        let base = std::env::temp_dir().join(format!("k2-persist-{p}"));
        move |c: &mut GossipConfig| {
            c.control_max_staleness_ms = 1_500; // a heartbeat tick every 500 ms
            c.max_clock_drift_ms = 500;
            c.persistence = Some(PersistenceConfig {
                base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000,
                snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse,
            });
        }
    };
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("k2-persist-{p1}")));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("k2-persist-{p2}")));
    let a = node(p1, &[p2], 3, HistoryScope::FleetTally, persist(p1)).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, persist(p2)).await;
    ready(&[&a, &b]).await;
    let a_id = a.node_id().to_string();
    let a_node = a.node_id().clone();
    a.shutdown().await;
    drop(a);
    let bctx = Arc::clone(&b.task_ctx);
    bctx.peers.pin().remove(&a_node);
    let hb = super::heartbeat_key(&a_id);
    assert!(
        until(|| bctx.peers.pin().remove(&a_node).is_none() && mycelium_core::ops::kv_get(&bctx, &hb).is_none(), Duration::from_secs(10)).await,
        "b collected a's keys once a was silent past the horizon"
    );

    let a = node(p1, &[p2], 3, HistoryScope::FleetTally, persist(p1)).await;
    ready(&[&a, &b]).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for (who, ctx) in [("a", &a.task_ctx), ("b", &b.task_ctx)] {
        let v = mycelium_core::ops::kv_get(ctx, &hb).unwrap_or_else(|| panic!("a's heartbeat is current at {who}"));
        assert!(!String::from_utf8_lossy(&v).contains("departed"), "a's newer heartbeat at {who}");
    }
    assert_eq!(a.task_ctx.sys_namespace_violations.load(std::sync::atomic::Ordering::Relaxed), 0, "no foreign write counted");

    // And at one store, either order: a tombstone at an old record's stamp never removes a newer write.
    let key: Arc<str> = Arc::from(super::heartbeat_key("127.0.0.1:1").as_str());
    let _ = mycelium_core::ops::kv_set(&bctx, Arc::clone(&key), bytes::Bytes::from_static(b"old"));
    let old = bctx.kv_state.store.pin().get(key.as_ref()).unwrap().timestamp;
    let _ = mycelium_core::ops::kv_set(&bctx, Arc::clone(&key), bytes::Bytes::from_static(b"new"));
    assert!(!mycelium_core::ops::kv_delete_held(&bctx, Arc::clone(&key), old), "the held record is not the observed one");
    assert_eq!(mycelium_core::ops::kv_get(&bctx, &key).as_deref(), Some(&b"new"[..]));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 4, finding 3: an abort after the record is on disk does not take the permit back.** `alice`'s claim
/// succeeds (both nodes vote) and her decision is appended; the tool is withdrawn while the history update is paused.
/// The record is in the journal, and the permit stays counted at the point — delivery unknown, never retracted.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_abort_after_the_record_keeps_the_permit_counted() {
    let (p1, p2) = (port(), port());
    let a_id = NodeId::new("127.0.0.1", p1).unwrap();
    let policy = ReferenceEvaluator::new("rev-k2")
        .with_catalogue("cat-k2", "1")
        .map_action("tools/call", "*")
        .allow(
            Rule::new(ALICE, "tools/call", format!("tool:slow@{a_id}"))
                .requiring_scopes(["mcp:invoke"])
                .at_most(3, TEN_MIN)
                .scoped(HistoryScope::FleetConsensus, "c-record"),
        );
    let a = node_with(p1, &[], policy.clone(), |_| {}).await;
    let b = node_with(p2, &[p1], policy, |_| {}).await;
    a.with_provider_enforcement();
    let _listeners: Vec<_> =
        [&a, &b].iter().map(|n| n.consensus().start_consensus_listener(ConsensusConfig::default())).collect();
    ready(&[&a, &b]).await;
    let tool = a.mcp().register_mcp_tool("slow", serde_json::json!({"type": "object"}), |_args| async { Ok(serde_json::json!("ran")) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    super::super::http::TEST_RECORD_PAUSE_MS.pin().insert(a_id.to_string(), 2_000);
    // `b` leaves alice's first seat prepare unanswered, so her claim is decided at its second ballot, two seconds late.
    // That is the CI flake this test had (main run 39219310547): under load a claim lost a ballot, the 2 s lease margin
    // refused it "decided too late", the refusal was recorded, and the test — which waited for *any* record of alice's —
    // went on to find nothing counted. The margin now covers a lost ballot (#648); this pins it here.
    let lost = Arc::new(std::sync::atomic::AtomicU32::new(1));
    crate::consensus::TEST_SWALLOW_PREPARES
        .pin()
        .insert(b.node_id().to_string(), (super::SEAT_SLOT_PREFIX.to_string(), Arc::clone(&lost)));
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move { alice_calls(&from, &to, "mcp.invoke", None, slow_call(), Duration::from_secs(40)).await });
    let journal = std::env::temp_dir().join(format!("k2-fleet-{p1}")).join("evidence.log");
    let recorded = || {
        super::super::evidence_journal::read_evidence_journal(&journal)
            .unwrap_or_default()
            .iter()
            .filter_map(|b| serde_json::from_slice::<AeEvidence>(b).ok())
            .any(|e| e.subject == ALICE)
    };
    assert!(until(recorded, Duration::from_secs(10)).await, "alice's decision reached the journal");
    crate::consensus::TEST_SWALLOW_PREPARES.pin().remove(&b.node_id().to_string());
    assert_eq!(lost.load(std::sync::atomic::Ordering::SeqCst), 0, "b left alice's first prepare unanswered");
    // The record must be a permit — a refused claim records a decision too, and this test is about a permit.
    let decided: Vec<AeEvidence> = super::super::evidence_journal::read_evidence_journal(&journal)
        .unwrap_or_default()
        .iter()
        .filter_map(|b| serde_json::from_slice::<AeEvidence>(b).ok())
        .filter(|e| e.subject == ALICE)
        .collect();
    assert!(
        decided.iter().all(|e| e.decision == crate::DecisionKind::Permit),
        "alice's call is admitted before the abort: {:?}",
        decided.iter().map(|e| serde_json::to_string(e).unwrap_or_default()).collect::<Vec<_>>()
    );
    drop(tool); // the loop ends while the history update is paused
    let _ = hers.await;
    super::super::http::TEST_RECORD_PAUSE_MS.pin().remove(&a_id.to_string());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let index = a.task_ctx.history_index.get().unwrap();
    let crate::History::View(view) =
        index.view("provider", ALICE, crate::HistoryRequest { window_ms: TEN_MIN, max_entries: usize::MAX }, a.task_ctx.hlc.decision_now_ms()).0
    else {
        panic!("a view")
    };
    // What alice's records say, so a failure names its cause: a refused claim records a decision too.
    let records: Vec<String> = super::super::evidence_journal::read_evidence_journal(&journal)
        .unwrap_or_default()
        .iter()
        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        .filter(|v| v["subject"] == ALICE)
        .map(|v| v.to_string().chars().take(600).collect())
        .collect();
    assert_eq!(view.entries.len(), 1, "the recorded permit stays counted: {:?}; alice's records: {records:?}", view.entries);
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 4, finding 4: `rpc/serve` never runs ahead, and answers what it took.** With the client connected but its
/// claim for `alice` still settling, the client disconnects: `alice` is answered at once, not left to time out.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_rpc_serve_stream_answers_what_it_took_when_the_client_goes() {
    let http = port();
    let (a, b, a_id, _) = provider_pair(|c| c.http_port = Some(http)).await;
    let stream = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http}/gateway/rpc/serve/depot.custom"))
        .send()
        .await
        .expect("the serve stream opens");
    assert_eq!(stream.status(), 200);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let resource = format!("res:x@{a_id}");
    let (from, to) = (Arc::clone(&a), a_id.clone());
    let hers = tokio::spawn(async move {
        alice_calls(&from, &to, "depot.custom", Some(&resource), b"work".to_vec(), Duration::from_secs(40)).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await; // the loop took alice's call; her claim is settling
    let started = std::time::Instant::now();
    drop(stream);
    let body = hers.await.unwrap().expect("answered, not timed out");
    assert!(started.elapsed() < Duration::from_secs(10), "answered when the client went: {:?}", started.elapsed());
    assert!(String::from_utf8_lossy(&body).contains("authority_not_established"), "{}", String::from_utf8_lossy(&body));
    a.shutdown().await;
    b.shutdown().await;
}

/// **Round 4, finding 1: a collector's tombstone loses to a newer write it had not seen.** The collector holds a silent
/// node's old heartbeat; the node wrote a newer one that is still in flight. Collection runs, then the newer write
/// arrives: it stands. A tombstone stamped at the collector's own clock would have beaten it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_collectors_tombstone_loses_to_a_newer_write_it_had_not_seen() {
    let p1 = port();
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |c| {
        c.control_max_staleness_ms = 1_500;
        c.max_clock_drift_ms = 500;
    })
    .await;
    let ctx = Arc::clone(&a.task_ctx);
    let silent = NodeId::new("127.0.0.1", port()).unwrap();
    let key: Arc<str> = Arc::from(super::heartbeat_key(&silent.to_string()).as_str());
    let now = ctx.hlc.decision_now_ms();
    let old = serde_json::to_vec(&serde_json::json!({"as_of": now - 60_000, "enforces": true, "departed": true})).unwrap();
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::clone(&key), bytes::Bytes::from(old));
    // The node's newer heartbeat, written after the record the collector holds, not yet delivered.
    let newer_ts = ctx.hlc.tick();
    let newer = mycelium_core::framing::make_gossip_update_stamped(
        &silent, 4, Arc::clone(&key), bytes::Bytes::from_static(b"{\"as_of\":1,\"enforces\":true}"), false, newer_ts,
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(super::collect_residue(&ctx, ctx.hlc.decision_now_ms()), 1, "the silent node is collected");
    assert!(mycelium_core::ops::kv_get(&ctx, &key).is_none());
    mycelium_core::store::apply_and_notify(&ctx.kv_state, &newer);
    assert!(mycelium_core::ops::kv_get(&ctx, &key).is_some(), "the newer heartbeat stands");
    a.shutdown().await;
}

/// **Round 4, finding 1: the owner's re-assert makes a collector's tombstone lose at the owner.** The node holds its own
/// heartbeat and an aged entry, as a restart restores them; a peer's collection tombstones both at their stamps. After
/// [`super::reassert_own`] — what `start()` runs before its connections come up — neither tombstone applies, so the
/// tripwire, which counts an applied foreign write, has nothing to count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owners_reassert_makes_a_collectors_tombstone_lose() {
    let p1 = port();
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |c| c.control_max_staleness_ms = 60_000).await;
    let ctx = Arc::clone(&a.task_ctx);
    let me = a.node_id().to_string();
    let now = ctx.hlc.decision_now_ms();
    let hb: Arc<str> = Arc::from(super::heartbeat_key(&me).as_str());
    let entry: Arc<str> = Arc::from(super::tally_key(&me, "c-posts", "gateway", "x").as_str());
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::clone(&hb), bytes::Bytes::from_static(b"{\"as_of\":1,\"enforces\":true}"));
    let _ = mycelium_core::ops::kv_set(
        &ctx, Arc::clone(&entry),
        bytes::Bytes::from(serde_json::to_vec(&serde_json::json!({"w": 1_000, "t": [now - 50_000]})).unwrap()),
    );
    let stamp = |k: &Arc<str>| ctx.kv_state.store.pin().get(k.as_ref()).unwrap().timestamp;
    let (hb_ts, entry_ts) = (stamp(&hb), stamp(&entry));
    super::reassert_own(&ctx);
    let collector = NodeId::new("127.0.0.1", port()).unwrap();
    for (key, ts) in [(&hb, hb_ts), (&entry, entry_ts)] {
        let tomb = mycelium_core::framing::make_gossip_update_stamped(&collector, 4, Arc::clone(key), bytes::Bytes::new(), true, ts);
        let applied = mycelium_core::store::apply_and_notify_reporting(&ctx.kv_state, &tomb);
        assert!(
            !matches!(applied, mycelium_core::receipt::LocalApplication::Applied),
            "{key}: a collector's tombstone at the old stamp does not apply ({applied:?})"
        );
    }
    let v = mycelium_core::ops::kv_get(&ctx, &hb).expect("the re-asserted heartbeat stands");
    assert!(!String::from_utf8_lossy(&v).contains("\"as_of\":1,"), "re-asserted, current");
    a.shutdown().await;
}

/// **Round 5, finding 2: a `/mcp` timeout tells the caller it timed out, and nothing about the mesh.** The provider,
/// whether it was this node, the timings, the peer count and the dropped frames stay in the operator's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_mcp_timeout_tells_the_caller_nothing_about_the_mesh() {
    let (p1, http) = (port(), port());
    let cfg = GossipConfig {
        bind_address: "127.0.0.1".parse().unwrap(),
        bind_port: p1,
        http_port: Some(http),
        ..GossipConfig::default()
    };
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p1).unwrap(), cfg));
    a.start().await.unwrap();
    let me = a.node_id().to_string();
    // A tool advertised with no handler behind it: the call is dispatched and never answered.
    let _ = mycelium_core::ops::kv_set(&a.task_ctx, Arc::from(format!("tools/ghost-{p1}/{me}").as_str()), bytes::Bytes::from_static(b"{}"));
    super::super::http::TEST_DISPATCH_TIMEOUT_MS.pin().insert(me.clone(), 300);
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": {"name": format!("ghost-{p1}"), "arguments": {}}}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    super::super::http::TEST_DISPATCH_TIMEOUT_MS.pin().remove(&me);
    assert_eq!(r["error"]["message"], "tool invocation timed out", "{r}");
    assert!(r["error"].get("data").is_none(), "no diagnostics to the caller: {r}");
    let text = r.to_string();
    for leak in [me.as_str(), "to_self", "preflight_ms", "dispatch_ms", "peers", "dropped_frames"] {
        assert!(!text.contains(leak), "{leak} in the caller's error: {r}");
    }
    assert!(super::super::http::TEST_LAST_TIMEOUT.pin().get(&me).is_some(), "the operator's diagnostics were taken");
    a.shutdown().await;
}

/// **Round 5, finding 3: `kv_delete_held` cannot write a tombstone newer than the record held.** Nothing held: it
/// refuses and writes nothing. A held record that is not the observed one: it refuses. The observed one: the tombstone
/// carries exactly its stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_delete_never_writes_a_tombstone_newer_than_the_record() {
    let p1 = port();
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let ctx = Arc::clone(&a.task_ctx);
    let key: Arc<str> = Arc::from("sys/ae-tally/127.0.0.1:1/hb");
    let stamp = |k: &Arc<str>| ctx.kv_state.store.pin().get(k.as_ref()).map(|e| (e.timestamp, e.data.is_none()));
    let far = ctx.hlc.tick();
    assert!(!mycelium_core::ops::kv_delete_held(&ctx, Arc::clone(&key), far), "nothing held");
    assert_eq!(stamp(&key), None, "nothing written");
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::clone(&key), bytes::Bytes::from_static(b"v"));
    let (held, _) = stamp(&key).unwrap();
    assert!(!mycelium_core::ops::kv_delete_held(&ctx, Arc::clone(&key), held + 1), "a stamp the record does not have");
    assert!(!mycelium_core::ops::kv_delete_held(&ctx, Arc::clone(&key), held - 1), "an older observation");
    assert_eq!(stamp(&key), Some((held, false)));
    assert!(mycelium_core::ops::kv_delete_held(&ctx, Arc::clone(&key), held));
    assert_eq!(stamp(&key), Some((held, true)), "the tombstone carries the held record's stamp, no newer");
    assert!(!mycelium_core::ops::kv_delete_held(&ctx, Arc::clone(&key), held), "a tombstone is not a held record");
    a.shutdown().await;
}

/// **Round 5, finding 4: without `[tls]`, a non-peer's absence is unknown coverage.** No member can be verified, so an
/// enforcing node that is no longer a peer is neither counted nor ignored: the tally is not established until its
/// heartbeat passes the horizon — fail closed, never an under-count of a crashed node's permits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_tls_a_non_peers_absence_is_not_established() {
    let (p1, p2) = (port(), port());
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    let b = node(p2, &[p1], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a, &b]).await;
    let out = post(&b).await;
    assert!(admitted(&out).is_some(), "established while every enforcing node is a peer: {:?}", refusal(&out));
    let gone = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    let now = b.task_ctx.hlc.decision_now_ms();
    let hb = serde_json::to_vec(&serde_json::json!({"as_of": now - 1_000, "enforces": true})).unwrap();
    let _ = mycelium_core::ops::kv_set(&b.task_ctx, Arc::from(super::heartbeat_key(&gone).as_str()), bytes::Bytes::from(hb));
    let out = post(&b).await;
    assert!(admitted(&out).is_none(), "not established");
    let why = format!("{:?}", refusal(&out));
    assert!(why.contains(&gone) && why.contains("cannot be verified"), "{why}");
    a.shutdown().await;
    b.shutdown().await;
}

/// **A fleet counter past the bounds costs the enforcement point nothing** (the Cedar adapter's review: a limit of
/// `u32::MAX` read every seat — a slot digest and a KV read each — on every in-scope request, pinning a tokio worker
/// for minutes). Whatever evaluator names it, `establish` reads no seat and no tally of a counter past the seat cap or
/// the history horizon, and `claim_seat` proposes none: each is not established, naming the bound. At the gateway an
/// evaluator that permits on such a counter is refused, not established; at the bounds the same call is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fleet_counter_past_the_bounds_is_not_established_at_the_enforcement_point() {
    use crate::{ActionEnvelope, ActionEvaluator, Decision, FleetCounter, History};
    const CAP: u32 = 1_024;
    const HORIZON: u64 = 24 * 60 * 60 * 1_000;
    /// The reference policy within the bounds, naming whatever counter it is given — an adapter's mistake.
    struct Naming(ReferenceEvaluator, std::sync::Mutex<FleetCounter>);
    impl ActionEvaluator for Naming {
        fn evaluate(&self, env: &ActionEnvelope) -> Decision {
            self.0.evaluate(env)
        }
        fn mapping(&self, operation: &str, resource: &str) -> crate::ActionMapping {
            self.0.mapping(operation, resource)
        }
        fn history_request(&self) -> Option<crate::HistoryRequest> {
            self.0.history_request()
        }
        // Permits on any count it is given for its own counter's key, ignoring the count's own bounds.
        fn evaluate_with_history(&self, env: &ActionEnvelope, history: &History) -> Decision {
            let mut h = history.clone();
            if let History::View(v) = &mut h {
                for c in &mut v.fleet {
                    c.counter.count = 3;
                    c.counter.window_ms = TEN_MIN;
                }
            }
            self.0.evaluate_with_history(env, &h)
        }
        fn fleet_counters(&self, _env: &ActionEnvelope) -> Vec<FleetCounter> {
            vec![self.1.lock().unwrap().clone()]
        }
    }
    let p1 = port();
    let mut cfg = GossipConfig { bind_address: "127.0.0.1".parse().unwrap(), bind_port: p1, ..GossipConfig::default() };
    cfg.bootstrap_peers = Vec::new();
    let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", p1).unwrap(), cfg));
    let dir = std::env::temp_dir().join(format!("k2-bounds-{p1}"));
    let _ = std::fs::remove_dir_all(&dir);
    a.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.log"), EvidenceProfile::Strict).unwrap());
    let naming = Arc::new(Naming(
        posts(3, TEN_MIN, HistoryScope::FleetConsensus),
        std::sync::Mutex::new(FleetCounter::new(HistoryScope::FleetConsensus, "c-posts", CAP + 1, TEN_MIN)),
    ));
    a.with_action_evaluator(Arc::clone(&naming) as Arc<dyn ActionEvaluator>);
    a.start().await.unwrap();
    let _listener = a.consensus().start_consensus_listener(ConsensusConfig::default());
    ready(&[&a]).await;
    let ctx = Arc::clone(&a.task_ctx);
    let actor = crate::PRINCIPAL_ANONYMOUS;

    for scope in [HistoryScope::FleetConsensus, HistoryScope::FleetTally] {
        for (count, window_ms, what) in [(CAP + 1, TEN_MIN, "seats"), (u32::MAX, TEN_MIN, "seats"), (3, HORIZON + 1, "horizon"), (3, 0, "window")] {
            let c = FleetCounter::new(scope, "c-posts", count, window_ms);
            let started = std::time::Instant::now();
            let state = super::establish(&ctx, std::slice::from_ref(&c), "gateway", actor, ctx.hlc.decision_now_ms())[0].state.clone();
            assert!(started.elapsed() < Duration::from_secs(1), "{scope:?} {count}: read nothing ({:?})", started.elapsed());
            let FleetState::NotEstablished(why) = &state else { panic!("{scope:?} {count} in {window_ms} ms: {state:?}") };
            assert!(why.contains(what), "{why}");
        }
    }
    let past = FleetCounter::new(HistoryScope::FleetConsensus, "c-posts", CAP + 1, TEN_MIN);
    match super::claim_seat(&ctx, &past, "gateway", actor, "attempt-past").await {
        super::Claim::NotEstablished(why) => assert!(why.contains("seats"), "{why}"),
        other => panic!("a claim past the cap proposes nothing: {other:?}"),
    }
    let late = FleetCounter::new(HistoryScope::FleetConsensus, "c-posts", 3, HORIZON + 1);
    match super::claim_seat(&ctx, &late, "gateway", actor, "attempt-late").await {
        super::Claim::NotEstablished(why) => assert!(why.contains("horizon"), "{why}"),
        other => panic!("a claim past the horizon proposes nothing: {other:?}"),
    }

    // Through the gateway: the evaluator permits on its own reading; the enforcement point does not.
    let out = post(&a).await;
    let r = refusal(&out);
    assert!(matches!(r, PreflightRefusal::NotEstablished(_)) && said(r).contains("fleet_consensus"), "{r:?}");
    // At the bounds, the same call is admitted: a seat claimed by consensus on this lone node.
    *naming.1.lock().unwrap() = FleetCounter::new(HistoryScope::FleetConsensus, "c-posts", CAP, HORIZON);
    let out = post(&a).await;
    assert!(admitted(&out).is_some(), "at the cap and the horizon: {:?}", refusal(&out));
    a.shutdown().await;
}

/// **An oversized stored window still ages out** (#653's review, finding 2): a tally entry's `w` is read back from the
/// store — written by an earlier release with a week's window, or forged as `u64::MAX` — so `aged` and the residue
/// collector clamp it to `MAX_FLEET_WINDOW_MS`. Unclamped, `now - u64::MAX` saturates to zero, the entry is never aged
/// and a departed node's residue is never collected. An entry whose times are inside the horizon is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversized_stored_window_ages_out_and_its_residue_is_collected() {
    const HORIZON: u64 = 24 * 60 * 60 * 1_000;
    let p1 = port();
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a]).await;
    let ctx = Arc::clone(&a.task_ctx);
    let now = ctx.hlc.decision_now_ms();
    let entry = |w: u64, t: u64| serde_json::to_vec(&serde_json::json!({"w": w, "t": [t]})).unwrap();
    for w in [u64::MAX, 7 * HORIZON] {
        assert!(super::aged(&entry(w, now - HORIZON - 1), now), "w = {w}: a time past the horizon has aged out");
        assert!(!super::aged(&entry(w, now - HORIZON + 60_000), now), "w = {w}: a time inside the horizon has not");
    }
    // This node's own oversized entry is withdrawn by its heartbeat's withdrawal.
    let own = super::tally_key(&a.node_id().to_string(), "c-old", "gateway", "x");
    let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(own.as_str()), bytes::Bytes::from(entry(u64::MAX, now - HORIZON - 1)));
    super::withdraw_aged_entries(&ctx, now);
    assert!(mycelium_core::ops::kv_get(&ctx, &own).is_none(), "the own oversized entry is withdrawn");
    // A departed node silent past the clamped horizon (24 h + staleness + drift): its residue is collected.
    let gone = NodeId::new("127.0.0.1", port()).unwrap().to_string();
    let silent_since = now - HORIZON - ctx.config.control_max_staleness_ms - ctx.config.max_clock_drift_ms - 60_000;
    let plant = |key: String, value: Vec<u8>| {
        let _ = mycelium_core::ops::kv_set(&ctx, Arc::from(key.as_str()), bytes::Bytes::from(value));
    };
    plant(super::heartbeat_key(&gone), serde_json::to_vec(&serde_json::json!({"as_of": silent_since, "enforces": true, "departed": true})).unwrap());
    plant(super::tally_key(&gone, "c-old", "gateway", "x"), entry(u64::MAX, silent_since));
    super::collect_residue(&ctx, ctx.hlc.decision_now_ms());
    for key in [super::heartbeat_key(&gone), super::tally_key(&gone, "c-old", "gateway", "x")] {
        assert!(mycelium_core::ops::kv_get(&ctx, &key).is_none(), "{key} collected");
    }
    a.shutdown().await;
}

/// **`publish_permit` writes nothing for a counter outside the bounds** (#653's review, finding 3): a permit relying on
/// one is refused at the seam, but the guard is what keeps a direct caller from keeping `count` permit times — up to
/// `u32::MAX` of them — in a fleet-visible entry. Within the bounds the same call writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_permit_writes_nothing_for_a_counter_outside_the_bounds() {
    use crate::FleetCounter;
    const HORIZON: u64 = 24 * 60 * 60 * 1_000;
    let p1 = port();
    let a = node(p1, &[], 3, HistoryScope::FleetTally, |_| {}).await;
    ready(&[&a]).await;
    let ctx = Arc::clone(&a.task_ctx);
    let me = a.node_id().to_string();
    let now = ctx.hlc.decision_now_ms();
    for (i, (count, window_ms)) in [(1_025, TEN_MIN), (u32::MAX, TEN_MIN), (3, HORIZON + 1), (3, 0)].into_iter().enumerate() {
        let key = format!("c-out-{i}");
        super::publish_permit(&ctx, &FleetCounter::new(HistoryScope::FleetTally, key.as_str(), count, window_ms), "gateway", "x", now);
        assert!(
            mycelium_core::ops::kv_get(&ctx, &super::tally_key(&me, &key, "gateway", "x")).is_none(),
            "{count} in {window_ms} ms: nothing published"
        );
    }
    super::publish_permit(&ctx, &FleetCounter::new(HistoryScope::FleetTally, "c-in", 1_024, HORIZON), "gateway", "x", now);
    assert!(mycelium_core::ops::kv_get(&ctx, &super::tally_key(&me, "c-in", "gateway", "x")).is_some(), "within the bounds, published");
    a.shutdown().await;
}
