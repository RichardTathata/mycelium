//! The schedule run stream (adopter-surface Y12): a client declares a `[[schedule]]` through the gateway and receives
//! its runs; an acknowledged run records its slot, one never acknowledged does not; a gap while no client held is a
//! missed-run decision with its reason; two holders under a partition hand their clients one key per slot.

use crate::capability::{CapFilter, Capability};
use crate::schedule::{CANDIDATE_NS, MISSED_RULE, RUN_RULE};
use crate::{GossipAgent, GossipConfig, NodeId};
use mycelium_core::decision::DecisionSink;
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// A started node with a gateway, a decision trace, and the holder band's capability advertised on it.
async fn node(peers: Vec<NodeId>, gossip_port: u16) -> (Arc<GossipAgent>, Arc<DecisionSink>, String, crate::CapabilityReg) {
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.bootstrap_peers = peers;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    let sink = Arc::new(DecisionSink::default());
    agent.with_decision_trace(Arc::clone(&sink));
    agent.start().await.unwrap();
    let band = agent.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(50)).await;
    (agent, sink, format!("http://127.0.0.1:{http_port}"), band)
}

fn schedule_toml(id: &str, period_secs: u64, on_missed: &str) -> String {
    format!("[[schedule]]\nid = \"{id}\"\nperiod_secs = {period_secs}\non_missed = \"{on_missed}\"\n[schedule.holders]\nns = \"ledger\"\nname = \"closer\"\n")
}

/// An open run stream, read event by event.
struct Stream {
    resp: reqwest::Response,
    buf: String,
}

impl Stream {
    async fn open(base: &str, toml: &str) -> reqwest::Response {
        reqwest::Client::new().get(format!("{base}/gateway/schedules/run")).query(&[("toml", toml)]).send().await.unwrap()
    }

    /// The next event `(name, data)`, or `None` when the stream ends or `ms` passes.
    async fn next(&mut self, ms: u64) -> Option<(String, serde_json::Value)> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let frame: String = self.buf.drain(..end + 2).collect();
                let (mut event, mut data) = (String::new(), String::new());
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("event:") { event = v.trim().to_string(); }
                    if let Some(v) = line.strip_prefix("data:") { data.push_str(v.trim()); }
                }
                if data.is_empty() { continue; } // a keep-alive comment
                return Some((event, serde_json::from_str(&data).unwrap()));
            }
            let chunk = tokio::time::timeout_at(deadline, self.resp.chunk()).await.ok()?.ok()??;
            self.buf.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    /// The next `run` event.
    async fn run(&mut self, ms: u64) -> Option<serde_json::Value> {
        loop {
            let (event, data) = self.next(ms).await?;
            if event == "run" { return Some(data); }
        }
    }
}

async fn open(base: &str, toml: &str) -> Stream {
    let resp = Stream::open(base, toml).await;
    assert_eq!(resp.status(), 200, "the stream opens");
    let mut s = Stream { resp, buf: String::new() };
    let (event, data) = s.next(5_000).await.expect("the stream says what it declared");
    assert_eq!(event, "declared", "{data}");
    s
}

async fn ack(base: &str, run_id: &str, ok: bool) -> reqwest::Response {
    reqwest::Client::new().post(format!("{base}/gateway/schedules/ack"))
        .json(&serde_json::json!({"run_id": run_id, "ok": ok, "error": if ok { None } else { Some("the client's work failed") }}))
        .send().await.unwrap()
}

fn traced(sink: &DecisionSink, rule: &str, reason: &str) -> usize {
    sink.snapshot().iter().filter(|r| r.rule == rule && r.reason == reason).count()
}

async fn poll_until(mut f: impl FnMut() -> bool, ms: u64) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        if f() { return true; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

fn last_run(agent: &GossipAgent, id: &str) -> Option<u64> {
    let v = agent.kv().get(&format!("sched/{id}/last/{}", agent.node_id()))?;
    std::str::from_utf8(&v).ok()?.split_whitespace().next()?.parse().ok()
}

/// Y12's exit, first half: a schedule declared through the gateway, its runs received by the client. Each run carries
/// the schedule, the slot and the idempotency key; an acknowledged run is the slot run — recorded in `sched/{id}/last`
/// and traced `ran` — and a second answer, or an answer to a run never handed out, is refused.
#[tokio::test]
async fn a_client_receives_its_schedules_runs_and_an_acknowledged_run_records_its_slot() {
    let (agent, sink, base, _band) = node(vec![], alloc_port()).await;
    let mut s = open(&base, &schedule_toml("close", 1, "skip")).await;
    let run = s.run(15_000).await.expect("the node holds and hands the client a run");
    let slot = run["slot"].as_u64().unwrap();
    assert_eq!(run["schedule"], "close");
    assert_eq!(run["kind"], "on_time");
    assert_eq!(run["operation_id"], format!("sched/close/{slot}"), "the run's key is the schedule and the slot");
    assert_eq!(last_run(&agent, "close"), None, "nothing is recorded before the client answers");
    let r = ack(&base, run["run_id"].as_str().unwrap(), true).await;
    assert_eq!(r.status(), 200);
    assert!(poll_until(|| last_run(&agent, "close") == Some(slot), 5_000).await, "the acknowledged slot is recorded");
    assert!(traced(&sink, RUN_RULE, "ran") >= 1, "the run is traced");
    let again = ack(&base, run["run_id"].as_str().unwrap(), true).await;
    assert_eq!(again.status(), 403, "one answer per run");
    assert_eq!(again.json::<serde_json::Value>().await.unwrap()["error"], "unserved_run");
    let never = ack(&base, "close/1/999", true).await;
    assert_eq!(never.status(), 403, "a run never handed out is refused");
    let run_id = run["run_id"].as_str().unwrap();
    assert!(run_id.len() > format!("close/{slot}/").len() + 8, "the run id carries a random part: {run_id}");

    // The next slot arrives. An answer to a guessed id — the schedule, the slot and a counter, the id's old shape — is
    // refused while the run waits; then a failed answer is the work's failure, traced, and the slot is not recorded.
    let next = s.run(5_000).await.expect("the next slot's run");
    let next_slot = next["slot"].as_u64().unwrap();
    for n in 0..4 {
        let guessed = ack(&base, &format!("close/{next_slot}/{n}"), true).await;
        assert_eq!(guessed.status(), 403, "a guessed run id is refused");
    }
    assert!(next["slot"].as_u64().unwrap() > slot);
    let r = ack(&base, next["run_id"].as_str().unwrap(), false).await;
    assert_eq!(r.status(), 200);
    assert!(poll_until(|| traced(&sink, RUN_RULE, "work_failed") >= 1, 5_000).await, "a failed answer is traced");
    drop(s);
    agent.shutdown().await;
}

/// **A run the client never acknowledged is not a run.** The client takes a run and goes away: the run is traced
/// `unacknowledged`, the slot is not recorded, the candidacy is withdrawn and the id freed, so another stream can
/// declare it again.
#[tokio::test]
async fn a_run_never_acknowledged_is_traced_and_its_slot_is_not_recorded() {
    let (agent, sink, base, _band) = node(vec![], alloc_port()).await;
    let mut s = open(&base, &schedule_toml("close", 1, "skip")).await;
    let run = s.run(15_000).await.expect("a run is handed out");
    drop(s); // the client goes away without answering
    assert!(poll_until(|| traced(&sink, RUN_RULE, "unacknowledged") == 1, 5_000).await,
        "the run is traced unacknowledged: {:?}", sink.snapshot().iter().map(|r| r.reason.clone()).collect::<Vec<_>>());
    let rec = sink.snapshot().into_iter().find(|r| r.reason == "unacknowledged").unwrap();
    assert!(rec.effect.as_deref().unwrap_or("").contains(run["operation_id"].as_str().unwrap()), "{rec:?}");
    assert_eq!(last_run(&agent, "close"), None, "the slot is not recorded");
    let candidates = || agent.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).len();
    assert!(poll_until(|| candidates() == 0, 5_000).await, "the candidacy is withdrawn");
    assert!(poll_until(|| agent.task_ctx.schedules_running.pin().get("close").is_none(), 5_000).await, "the id is freed");
    let again = Stream::open(&base, &schedule_toml("close", 1, "skip")).await;
    assert_eq!(again.status(), 200, "the schedule can be declared again");
    drop(again);
    agent.shutdown().await;
}

/// Y12's exit: **a missed run's decision traced with its reason when the client is gone.** A client runs a slot and
/// leaves; while no one holds, slots pass (the wall clock stepped five periods through the seam, what a paused or
/// absent client looks like); the next client's node finds the gap and decides it under the policy — `run_once`
/// hands the client one run for the gap, naming its first slot; `skip` hands it only the slot due.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn slots_missed_while_no_client_held_are_a_decision_with_its_reason() {
    for (policy, reason, gap_kind) in [("run_once", "ran_once", Some("for_gap")), ("skip", "skipped", None)] {
        let (agent, sink, base, _band) = node(vec![], alloc_port()).await;
        let toml = schedule_toml("close", 2, policy);
        let mut s = open(&base, &toml).await;
        let run = s.run(15_000).await.expect("the first client's run");
        let first = run["slot"].as_u64().unwrap();
        assert_eq!(ack(&base, run["run_id"].as_str().unwrap(), true).await.status(), 200);
        assert!(poll_until(|| last_run(&agent, "close") == Some(first), 5_000).await);
        drop(s);
        assert!(poll_until(|| agent.task_ctx.schedules_running.pin().get("close").is_none(), 5_000).await, "{policy}: the client is gone");

        mycelium_core::sim_seam::step_wall_clock_for_test(5 * 2_000);
        let mut s = open(&base, &toml).await;
        let run = s.run(20_000).await.expect("the next client is handed a run");
        mycelium_core::sim_seam::step_wall_clock_for_test(0);
        let missed = sink.snapshot().into_iter().find(|r| r.rule == MISSED_RULE).expect("the gap is decided");
        assert_eq!(missed.reason, reason, "{policy}");
        assert!(missed.inputs.iter().any(|i| i.source.contains("sched/{id}/last") && i.value == first.to_string()),
            "{policy}: the last run before the client left is an input: {missed:?}");
        match gap_kind {
            Some(kind) => {
                assert_eq!(run["kind"], kind, "{policy}: {run}");
                assert_eq!(run["first_slot"], first + 1, "{policy}: the gap starts after the last run: {run}");
            }
            None => assert_eq!(run["kind"], "on_time", "{policy}: skip runs only the slot due: {run}"),
        }
        drop(s);
        agent.shutdown().await;
    }
}

/// Declaring through the stream is refused as the unit-file loader refuses, by name, and one id runs once per node.
#[tokio::test]
async fn the_stream_refuses_by_name_and_one_id_runs_once_per_node() {
    let (agent, _sink, base, _band) = node(vec![], alloc_port()).await;
    let body = |r: reqwest::Response| async move { (r.status().as_u16(), r.json::<serde_json::Value>().await.unwrap()) };

    let (status, v) = body(Stream::open(&base, "[[schedule]\nid=").await).await;
    assert_eq!(status, 400, "{v}");
    let two = format!("{}{}", schedule_toml("a", 60, "skip"), schedule_toml("b", 60, "skip"));
    let (status, v) = body(Stream::open(&base, &two).await).await;
    assert_eq!((status, v["error"].as_str()), (422, Some("one schedule")), "{v}");
    let (status, v) = body(Stream::open(&base, &format!("{}[[capability]]\nns = \"x\"\nname = \"y\"\n", schedule_toml("a", 60, "skip"))).await).await;
    assert_eq!(status, 400, "another section is refused: {v}");
    assert!(v["detail"].as_str().unwrap().contains("capability"), "by name: {v}");
    let (status, v) = body(Stream::open(&base, &schedule_toml("a", 0, "skip")).await).await;
    assert_eq!(status, 422, "{v}");
    assert!(v["detail"].as_str().unwrap().contains("period_secs"), "the loader's refusal, by name: {v}");
    let (status, v) = body(Stream::open(&base, &schedule_toml("a.holder", 60, "skip")).await).await;
    assert_eq!(status, 422, "{v}");

    let held = open(&base, &schedule_toml("a", 60, "skip")).await;
    let (status, v) = body(Stream::open(&base, &schedule_toml("a", 60, "skip")).await).await;
    assert_eq!((status, v["error"].as_str()), (409, Some("schedule_running")), "{v}");
    drop(held);

    // A unit still cannot carry a schedule, and the refusal names the route that runs one.
    let r = reqwest::Client::new().post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({"toml": schedule_toml("a", 60, "skip")})).send().await.unwrap();
    let (status, v) = body(r).await;
    assert_eq!(status, 422, "{v}");
    assert!(v["detail"].as_str().unwrap().contains("/gateway/schedules/run"), "{v}");
    agent.shutdown().await;
}

/// The routes' scope, and an answer only from the principal the run was handed to.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn the_stream_needs_schedule_run_and_a_run_is_answered_by_its_own_principal() {
    use axum::http::header::AUTHORIZATION;
    assert_eq!(super::required_scope(&axum::http::Method::GET, "/gateway/schedules/run"), "schedule:run");
    assert_eq!(super::required_scope(&axum::http::Method::POST, "/gateway/schedules/ack"), "schedule:run");

    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "sched-1".into(), scopes: vec!["schedule:run".into()] },
        crate::GatewayToken { token: "sched-2".into(), scopes: vec!["schedule:run".into()] },
        crate::GatewayToken { token: "serve".into(),   scopes: vec!["mesh:serve".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let _band = agent.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(1));
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();
    let toml = schedule_toml("close", 1, "skip");

    let r = client.get(format!("{base}/gateway/schedules/run")).query(&[("toml", &toml)]).header(AUTHORIZATION, "Bearer serve").send().await.unwrap();
    assert_eq!(r.status(), 403, "mesh:serve does not open a schedule stream");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["required_scope"], "schedule:run");

    let resp = client.get(format!("{base}/gateway/schedules/run")).query(&[("toml", &toml)]).header(AUTHORIZATION, "Bearer sched-1").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut s = Stream { resp, buf: String::new() };
    let run = s.run(15_000).await.expect("a run");
    let run_id = run["run_id"].as_str().unwrap();
    let answer = |tok: &'static str| client.post(format!("{base}/gateway/schedules/ack")).header(AUTHORIZATION, format!("Bearer {tok}"))
        .json(&serde_json::json!({"run_id": run_id, "ok": true})).send();
    let r = answer("sched-2").await.unwrap();
    assert_eq!(r.status(), 403, "another principal cannot answer the run");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "unserved_run");
    assert_eq!(answer("sched-1").await.unwrap().status(), 200, "its own principal answers it");
    drop(s);
    agent.shutdown().await;
}

/// Y12's exit: **one run per slot under a partition.** Two nodes, each standing in the band for its own client. While
/// connected, one holds; cut apart, each side holds and hands its client the slot — under one key, so the clients'
/// effects meet at a destination that deduplicates and the slot is applied once (X5's contract, as Y2 showed for
/// in-process work).
#[cfg(feature = "test-frame-filter")]
#[tokio::test]
async fn two_clients_under_a_partition_are_handed_one_key_per_slot() {
    use crate::store::FrameView;
    use std::collections::{BTreeMap, BTreeSet};
    let ports = [alloc_port(), alloc_port()];
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let (a, _sa, base_a, _ba) = node(vec![ids[1].clone()], ports[0]).await;
    let (b, _sb, base_b, _bb) = node(vec![ids[0].clone()], ports[1]).await;
    let toml = schedule_toml("close", 1, "skip");
    // Each client applies its run under the run's key; the destination keeps the keys it applied and how often each
    // was presented.
    let applied: Arc<std::sync::Mutex<BTreeMap<String, BTreeSet<usize>>>> = Arc::default();
    // Every run handed to either client, as handed: (client, the run's slot, its key).
    let handed: Arc<std::sync::Mutex<Vec<(usize, u64, String)>>> = Arc::default();
    let mut clients = Vec::new();
    for (i, base) in [base_a.clone(), base_b.clone()].into_iter().enumerate() {
        let (applied, handed) = (Arc::clone(&applied), Arc::clone(&handed));
        let mut s = open(&base, &toml).await;
        clients.push(tokio::spawn(async move {
            while let Some(run) = s.run(60_000).await {
                let key = run["operation_id"].as_str().unwrap().to_string();
                handed.lock().unwrap().push((i, run["slot"].as_u64().unwrap(), key.clone()));
                applied.lock().unwrap().entry(key).or_default().insert(i);
                ack(&base, run["run_id"].as_str().unwrap(), true).await;
            }
        }));
    }
    let by_both = |m: &BTreeMap<String, BTreeSet<usize>>| m.values().filter(|s| s.len() == 2).count();
    // Connected: once each node sees both candidates and one holder — the warm-up is a delay, not a convergence check,
    // so a slow start can briefly hold on both sides — one holder hands each slot to one client.
    let holders = |ag: &GossipAgent| ag.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close.holder")).len();
    let candidates = |ag: &GossipAgent| ag.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).len();
    assert!(poll_until(|| candidates(&a) == 2 && candidates(&b) == 2 && holders(&a) == 1 && holders(&b) == 1, 30_000).await,
        "connected, the nodes agree on one holder: {} / {}", holders(&a), holders(&b));
    let seen_before: BTreeSet<String> = applied.lock().unwrap().keys().cloned().collect();
    let after = |m: &BTreeMap<String, BTreeSet<usize>>| m.iter().filter(|(k, _)| !seen_before.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect::<BTreeMap<_, _>>();
    assert!(poll_until(|| after(&applied.lock().unwrap()).len() >= 2, 20_000).await, "connected, the slots run");
    let connected = after(&applied.lock().unwrap());
    assert_eq!(by_both(&connected), 0, "connected, one holder hands each slot to one client: {connected:?}");

    for (i, ag) in [&a, &b].iter().enumerate() {
        let peer = ids[1 - i].id_hash();
        ag.task_ctx.kv_state.frame_filter.pin().insert(40, Arc::new(move |v: &FrameView<'_>| match v {
            FrameView::Kv { sender, .. } => *sender == peer || *sender == 0,
            FrameView::Signal { .. } => true,
        }));
    }
    let cut_apart = poll_until(|| by_both(&applied.lock().unwrap()) >= 1, 20_000).await;
    let keys = applied.lock().unwrap().clone();
    assert!(cut_apart, "cut apart, both sides hand a slot to their clients: {keys:?}");
    // Counted per slot across both clients: each run's key is its own slot's, so the two sides' runs of one slot meet
    // under one key at a deduplicating destination; each client is handed a slot at most once; and no slot is handed
    // more than once per side.
    let handed = handed.lock().unwrap().clone();
    let mut per_slot: BTreeMap<u64, Vec<(usize, String)>> = BTreeMap::new();
    for (client, slot, key) in &handed {
        assert_eq!(key, &format!("sched/close/{slot}"), "a run's key is its slot's: {handed:?}");
        per_slot.entry(*slot).or_default().push((*client, key.clone()));
    }
    for (slot, runs) in &per_slot {
        let clients: BTreeSet<usize> = runs.iter().map(|(c, _)| *c).collect();
        assert_eq!(clients.len(), runs.len(), "slot {slot} handed twice to one client: {runs:?}");
        let keys: BTreeSet<&String> = runs.iter().map(|(_, k)| k).collect();
        assert_eq!(keys.len(), 1, "slot {slot} handed under more than one key: {runs:?}");
    }
    assert!(per_slot.values().any(|r| r.len() == 2), "a slot handed to both clients: {per_slot:?}");

    for ag in [&a, &b] { ag.task_ctx.kv_state.frame_filter.pin().remove(&40); }
    for c in clients { c.abort(); }
    a.shutdown().await;
    b.shutdown().await;
}

/// The 409 is the loop's own check, so a stream racing another — or an application's `run_schedule` — for one id is
/// told `schedule_running`, never `invalid schedule`.
#[tokio::test]
async fn an_id_the_node_already_runs_is_a_409_from_the_loops_own_check() {
    let (agent, _sink, base, _band) = node(vec![], alloc_port()).await;
    let decl: crate::capability_config::ScheduleDecl = toml::from_str::<std::collections::BTreeMap<String, Vec<crate::capability_config::ScheduleDecl>>>(&schedule_toml("app", 60, "skip"))
        .unwrap().remove("schedule").unwrap().remove(0);
    let _app = agent.run_schedule(&decl, |_run| async { Ok(()) }).unwrap();
    let r = Stream::open(&base, &schedule_toml("app", 60, "skip")).await;
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "schedule_running");
    agent.shutdown().await;
}

/// **A stream whose node does not match `holders` says so**: a `status` event `not_eligible`, not silence.
#[tokio::test]
async fn a_node_outside_the_band_tells_its_client_not_eligible() {
    let (agent, _sink, base, _band) = node(vec![], alloc_port()).await;
    let toml = "[[schedule]]\nid = \"close\"\nperiod_secs = 1\n[schedule.holders]\nns = \"ledger\"\nname = \"nobody\"\n";
    let mut s = open(&base, toml).await;
    let mut said = None;
    while let Some((event, data)) = s.next(10_000).await {
        assert_ne!(event, "run", "a node outside the band runs nothing: {data}");
        if event == "status" { said = Some(data); break; }
    }
    let said = said.expect("the stream says why it hands out nothing");
    assert_eq!(said["reason"], "not_eligible", "{said}");
    assert_eq!(said["schedule"], "close");
    agent.shutdown().await;
}

/// **A client that stalls loses the holding to another node.** Two nodes, each with a client; the stalled client's node
/// holds first (an id is chosen whose ring it wins). Its client takes a run and never answers: the run overruns its
/// period, the node stands down — `stood_down`, said on its stream — and the healthy client's node holds and is handed
/// the slots, while the stalled client is handed nothing more.
#[tokio::test]
async fn a_stalled_clients_node_stands_down_and_the_healthy_one_takes_the_slots() {
    let ports = [alloc_port(), alloc_port()];
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    // An id whose ring the first node wins between the two, so the plant (no stand-down) is seen re-holding there.
    let cap = || {
        let (attr, value) = crate::election::rule_attribute();
        Capability::new(CANDIDATE_NS, "x").with(attr, value)
    };
    let pair: Vec<(NodeId, Capability)> = ids.iter().map(|n| (n.clone(), cap())).collect();
    let id = (0..200).map(|n| format!("stall-{n}")).find(|id| crate::election::elect(&format!("sched/{id}"), &pair) == Some(&ids[0]))
        .expect("an id the first node wins");
    let (a, sink_a, base_a, _ba) = node(vec![ids[1].clone()], ports[0]).await;
    let (b, _sb, base_b, _bb) = node(vec![ids[0].clone()], ports[1]).await;
    let toml = schedule_toml(&id, 1, "skip");

    // The stalled client: takes runs, answers none.
    let stalled: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let mut sa = open(&base_a, &toml).await;
    let first = sa.run(15_000).await.expect("the first node holds and hands its client a run");
    stalled.lock().unwrap().push(first["slot"].as_u64().unwrap());
    let st = Arc::clone(&stalled);
    let stalled_reader = tokio::spawn(async move {
        while let Some((event, data)) = sa.next(60_000).await {
            if event == "run" { st.lock().unwrap().push(data["slot"].as_u64().unwrap()); }
        }
    });
    // The healthy client, on the other node.
    let healthy: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let h = Arc::clone(&healthy);
    let base = base_b.clone();
    let mut sb = open(&base_b, &toml).await;
    let healthy_reader = tokio::spawn(async move {
        while let Some(run) = sb.run(60_000).await {
            h.lock().unwrap().push(run["slot"].as_u64().unwrap());
            ack(&base, run["run_id"].as_str().unwrap(), true).await;
        }
    });
    assert!(poll_until(|| traced(&sink_a, RUN_RULE, "work_overran") >= 1, 10_000).await, "the stalled run overruns");
    assert!(poll_until(|| traced(&sink_a, RUN_RULE, "stood_down") >= 1, 5_000).await, "the stalled client's node stands down");
    assert!(poll_until(|| healthy.lock().unwrap().len() >= 3, 20_000).await,
        "the healthy client's node takes the slots: {:?}", healthy.lock().unwrap());
    let stalled_now = stalled.lock().unwrap().clone();
    let healthy_now = healthy.lock().unwrap().clone();
    assert!(stalled_now.len() <= 2, "the stalled client is handed nothing once its node stood down: {stalled_now:?} vs {healthy_now:?}");
    stalled_reader.abort();
    healthy_reader.abort();
    a.shutdown().await;
    b.shutdown().await;
}
