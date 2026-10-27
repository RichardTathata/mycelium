//! Declared recurring work (adopter-surface Y2): the pure decision, the loader's refusals, and the loop on real
//! agents — two holders under a partition giving one effect, and a missed run in the trace with its reason.

use super::*;
use crate::capability_config::FilterDecl;
use crate::effects::external::{ExternalDestination, IdempotencyKey, ProbeAnswer};
#[cfg(feature = "test-frame-filter")]
use crate::effects::external::{EffectClass, ExternalEffect, ProbeBudget, Reconciler};
use crate::effects::{Effect, EffectRefusal};
use crate::test_util::alloc_port;
use crate::{AttemptId, DedupOutcome, DestinationCommit, GossipAgent, GossipConfig};
use mycelium_core::decision::DecisionSink;
use std::collections::HashMap;
use std::sync::Mutex;

fn decl(id: &str, period_secs: u64, on_missed: MissedRunPolicy) -> ScheduleDecl {
    ScheduleDecl {
        id: id.into(),
        period_secs,
        phase_secs: 0,
        on_missed,
        holders: FilterDecl { ns: "ledger".into(), name: "closer".into(), schema_id: None, attrs: Default::default(), ranking: None },
    }
}

fn schedule(on_missed: MissedRunPolicy) -> Schedule {
    Schedule::from_decl(&decl("close", 60, on_missed)).unwrap()
}

const MIN: u64 = 60_000;

#[test]
fn a_slot_is_a_period_from_the_epoch_plus_the_phase() {
    let mut d = decl("close", 60, MissedRunPolicy::Skip);
    d.phase_secs = 10;
    let s = Schedule::from_decl(&d).unwrap();
    assert_eq!(s.slot_at(9_999), None, "before the first slot's start");
    assert_eq!(s.slot_at(10_000), Some(0));
    assert_eq!(s.slot_at(69_999), Some(0));
    assert_eq!(s.slot_at(70_000), Some(1));
    assert_eq!(s.slot_start_ms(3), 190_000);
}

#[test]
fn a_tick_runs_the_slot_due_only_on_the_holder_and_only_once() {
    let s = schedule(MissedRunPolicy::Skip);
    let t = decide(&s, 10 * MIN + 5, false, Some(9));
    assert_eq!((t.outcome, t.reason, t.runs.len()), (OutcomeKind::NoAction, "not_holder", 0));
    let t = decide(&s, 10 * MIN + 5, true, Some(10));
    assert_eq!((t.outcome, t.reason, t.runs.len()), (OutcomeKind::NoAction, "already_ran", 0));
    let t = decide(&s, 10 * MIN + 5, true, Some(15));
    assert_eq!((t.outcome, t.reason, t.runs.len()), (OutcomeKind::NoAction, "last_ahead", 0), "a run recorded ahead of this clock is said, not silently `already_ran`");
    let t = decide(&s, 10 * MIN + 5, true, Some(9));
    assert_eq!((t.outcome, t.reason), (OutcomeKind::Action, "ran"));
    assert_eq!(t.missed, None, "the next slot after the last run is no gap");
    assert_eq!(t.runs.iter().map(|r| (r.slot, r.kind)).collect::<Vec<_>>(), vec![(10, RunKind::OnTime)]);
    assert_eq!(t.runs[0].operation_id().as_str(), "sched/close/10", "the run's idempotency key is the schedule and the slot");
    // No recorded run: the schedule starts now, and nothing before it is counted missed.
    let t = decide(&s, 10 * MIN + 5, true, None);
    assert_eq!((t.missed, t.runs.len()), (None, 1));
}

#[test]
fn a_missed_run_is_decided_by_the_policy_with_its_reason() {
    let now = 10 * MIN + 5; // slot 10 due; slot 6 was the last run, so 7..=9 were missed
    let t = decide(&schedule(MissedRunPolicy::Skip), now, true, Some(6));
    assert_eq!(t.missed, Some(Missed { first: 7, last: 9, outcome: OutcomeKind::NoAction, reason: "skipped", skipped: 3 }));
    assert_eq!(t.runs.iter().map(|r| r.slot).collect::<Vec<_>>(), vec![10], "skip runs only the slot due");

    let t = decide(&schedule(MissedRunPolicy::RunLate), now, true, Some(6));
    assert_eq!(t.missed.as_ref().map(|m| (m.outcome, m.reason, m.skipped)), Some((OutcomeKind::Action, "ran_late", 0)));
    assert_eq!(t.runs.iter().map(|r| (r.slot, r.kind)).collect::<Vec<_>>(),
        vec![(7, RunKind::Late), (8, RunKind::Late), (9, RunKind::Late), (10, RunKind::OnTime)], "each missed slot under its own key, oldest first");

    let t = decide(&schedule(MissedRunPolicy::RunOnce), now, true, Some(6));
    assert_eq!(t.missed.as_ref().map(|m| (m.outcome, m.reason)), Some((OutcomeKind::Action, "ran_once")));
    assert_eq!(t.runs.iter().map(|r| (r.slot, r.kind)).collect::<Vec<_>>(), vec![(9, RunKind::ForGap { first: 7 }), (10, RunKind::OnTime)]);
    assert_eq!(t.runs[0].covers(), 7..=9, "one run says which slots it stands for");
}

#[test]
fn run_late_catches_up_at_most_the_cap_and_counts_the_rest() {
    let s = schedule(MissedRunPolicy::RunLate);
    let t = decide(&s, 1_000 * MIN, true, Some(0)); // 999 missed
    let m = t.missed.unwrap();
    assert_eq!((m.reason, m.first, m.last, m.skipped), ("ran_late_capped", 1, 999, 999 - MAX_CATCH_UP));
    assert_eq!(t.runs.len() as u64, MAX_CATCH_UP + 1);
    assert_eq!(t.runs.first().map(|r| r.slot), Some(1_000 - MAX_CATCH_UP));
}

#[test]
fn a_schedule_is_refused_by_name() {
    let refused = |toml: &str| NodeCapabilityConfig::from_toml_str(toml).unwrap_err().to_string();
    let holders = "  [schedule.holders]\n  ns = \"ledger\"\n  name = \"closer\"\n";
    let ok = format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\n{holders}");
    let cfg = NodeCapabilityConfig::from_toml_str(&ok).unwrap();
    assert_eq!((cfg.schedules[0].on_missed, cfg.schedules[0].phase_secs), (MissedRunPolicy::Skip, 0), "the defaults");
    assert!(refused("[[schedule]]\nid = \"close\"\nperiod_secs = 60\n").contains("holders"), "no holder band");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 0\n{holders}"));
    assert!(e.contains("schedule.period_secs"), "{e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 604801\n{holders}"));
    assert!(e.contains("schedule.period_secs"), "{e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\nphase_secs = 60\n{holders}"));
    assert!(e.contains("schedule.phase_secs"), "{e}");
    for id in ["", "a/b", "a b", "close.holder", "..", ".close", "a..b"] {
        let e = refused(&format!("[[schedule]]\nid = {id:?}\nperiod_secs = 60\n{holders}"));
        assert!(e.contains("schedule.id"), "{id:?}: {e}");
    }
    let e = refused(&format!("{ok}[[schedule]]\nid = \"close\"\nperiod_secs = 30\n{holders}"));
    assert!(e.contains("schedule.id") && e.contains("same id"), "{e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\non_missed = \"catch_up\"\n{holders}"));
    assert!(e.contains("on_missed") || e.contains("catch_up"), "{e}");
    let e = refused("[[schedule]]\nid = \"close\"\nperiod_secs = 60\n  [schedule.holders]\n  ns = \"\"\n  name = \"closer\"\n");
    assert!(e.contains("schedule.holders"), "an empty holders ns: {e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\n{holders}  [schedule.holders.ranking]\n  attribute = \"load\"\n  order = \"ascending\"\n"));
    assert!(e.contains("schedule.holders") && e.contains("ranking"), "{e}");
    let e = refused("[[capability]]\nns = \"schedule\"\nname = \"close\"\n");
    assert!(e.contains("reserved"), "the schedule namespace is the loop's: {e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\ncron = \"* * *\"\n{holders}"));
    assert!(e.contains("cron"), "an unknown key is refused, not ignored: {e}");
}

/// Clock skew inside a slot changes nothing; across a slot boundary, two holders compute two slots — two keys, two
/// runs. One effect per slot holds for holders whose clocks agree to within the time left in the slot.
#[test]
fn a_skewed_clock_names_the_same_slot_inside_it_and_another_across_its_boundary() {
    let s = schedule(MissedRunPolicy::Skip);
    let key = |now| decide(&s, now, true, Some(9)).runs.last().map(|r| r.operation_id().to_string());
    assert_eq!(key(10 * MIN + 1_000), key(10 * MIN + 40_000), "40 s of skew inside one minute's slot: one key");
    assert_eq!(key(10 * MIN + 1_000).as_deref(), Some("sched/close/10"));
    assert_eq!(decide(&s, 11 * MIN - 500, true, Some(9)).runs.last().map(|r| r.slot), Some(10));
    assert_eq!(decide(&s, 11 * MIN + 500, true, Some(10)).runs.last().map(|r| r.slot), Some(11), "a second of skew across the boundary: the next slot");
}

// ── The loop, on real agents ─────────────────────────────────────────────────────────────────────────

/// A destination that deduplicates on the key as X5's idempotent class declares: the second send of one key with the
/// same content is `Replayed`, applied once; with other content, a `Conflict`.
#[derive(Default)]
struct Ledger {
    entries: Mutex<HashMap<String, (u64, u32, u32)>>, // key → (content hash, applications, attempts)
}

impl Ledger {
    #[cfg(feature = "test-frame-filter")]
    fn by_key(&self) -> HashMap<String, (u32, u32)> {
        self.entries.lock().unwrap().iter().map(|(k, (_, a, t))| (k.clone(), (*a, *t))).collect()
    }
}

impl ExternalDestination for Ledger {
    fn identity(&self) -> &str { "ledger" }
    fn dedup_scope(&self) -> &str { "ledger" }
    fn host(&self) -> Option<&str> { None }
    fn send(&self, key: &IdempotencyKey, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        let mut map = self.entries.lock().unwrap();
        let e = map.entry(key.operation_id.to_string()).or_insert((effect.content_hash, 0, 0));
        e.2 += 1;
        if e.0 != effect.content_hash {
            return Err(EffectRefusal::Conflict { operation_id: effect.operation_id.clone(), committed_hash: e.0, presented_hash: effect.content_hash });
        }
        let fresh = e.1 == 0;
        if fresh { e.1 = 1; }
        Ok(DestinationCommit::new("ledger", if fresh { DedupOutcome::Fresh } else { DedupOutcome::Replayed }))
    }
    fn observe(&self, key: &IdempotencyKey) -> ProbeAnswer {
        if self.entries.lock().unwrap().contains_key(key.operation_id.as_str()) { ProbeAnswer::Done } else { ProbeAnswer::NotDone }
    }
}

#[test]
fn the_test_ledger_refuses_other_content_under_one_key() {
    let l = Ledger::default();
    let op = OperationId::new("sched/close/1");
    let key = IdempotencyKey { scope: "ledger".into(), operation_id: op.clone() };
    assert!(l.send(&key, &Effect::new(op.clone(), AttemptId::fresh(&op), b"a".to_vec())).is_ok());
    assert!(matches!(l.send(&key, &Effect::new(op.clone(), AttemptId::fresh(&op), b"a".to_vec())), Ok(c) if c.dedup == DedupOutcome::Replayed));
    assert!(matches!(l.send(&key, &Effect::new(op.clone(), AttemptId::fresh(&op), b"b".to_vec())), Err(EffectRefusal::Conflict { .. })));
}

fn agent(port: u16, peers: Vec<NodeId>) -> GossipAgent {
    let mut cfg = GossipConfig::auto();
    cfg.bind_port = port;
    cfg.bootstrap_peers = peers;
    GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
}

async fn poll_until(mut f: impl FnMut() -> bool, ms: u64) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        if f() { return true; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

fn traced(sink: &DecisionSink, rule: &str, reason: &str) -> usize {
    sink.snapshot().iter().filter(|r| r.rule == rule && r.reason == reason).count()
}

/// One started agent in the band, with a sink.
async fn banded(peers: Vec<NodeId>, port: u16) -> (GossipAgent, Arc<DecisionSink>, crate::CapabilityReg) {
    let a = agent(port, peers);
    let sink = Arc::new(DecisionSink::default());
    a.with_decision_trace(Arc::clone(&sink));
    a.start().await.unwrap();
    let band = a.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(1));
    (a, sink, band)
}

#[cfg(feature = "test-frame-filter")]
fn effect_work(rec: Arc<Reconciler<Ledger>>) -> impl Fn(ScheduledRun) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync + 'static {
    move |run: ScheduledRun| {
        let rec = Arc::clone(&rec);
        Box::pin(async move {
            let op = run.operation_id();
            let ext = ExternalEffect {
                // The content is the run's key alone, so a second holder's send of the slot is a replay.
                effect: Effect::new(op.clone(), AttemptId::fresh(&op), format!("close {op}").into_bytes()),
                class: EffectClass::Idempotent { compensation: None },
                probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 60_000 },
                composition: None,
            };
            rec.apply(&ext, None).await.map(|_| ()).map_err(|e| e.to_string())
        })
    }
}

/// Y2's exit: two holders under a partition, one effect at the destination — and the lease: a holder that its
/// peer stops hearing is taken over once its advertisements time out, never before. Two connected nodes elect one
/// holder; the link is cut both ways at the frame layer (`test-frame-filter`); the other side holds once the
/// holder's advertisements leave its view (three lease refreshes), so each side runs the slot, each through its own
/// reconciler under the run's key, and the destination applies each key once. Healed, one holds again.
#[cfg(feature = "test-frame-filter")]
#[tokio::test]
async fn two_holders_under_a_partition_give_one_effect_at_the_destination() {
    use crate::store::FrameView;
    let ports = [alloc_port(), alloc_port()];
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let (a, sa, _ba) = banded(vec![ids[1].clone()], ports[0]).await;
    let (b, sb, _bb) = banded(vec![ids[0].clone()], ports[1]).await;
    let agents = [a, b];
    let sinks = [sa, sb];
    let dir = std::env::temp_dir().join(format!("mycelium-schedule-partition-{}-{}", std::process::id(), ports[0]));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = Arc::new(Ledger::default());
    let mut handles = Vec::new();
    for (i, ag) in agents.iter().enumerate() {
        let rec = Arc::new(Reconciler::open(dir.join(format!("ledger-{i}.journal")), Arc::clone(&ledger), crate::EgressPolicy::default()).unwrap());
        handles.push(ag.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), effect_work(rec)).unwrap());
    }
    let holders = |ag: &GossipAgent| ag.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close.holder")).len();
    assert!(poll_until(|| holders(&agents[0]) == 1 && holders(&agents[1]) == 1 && handles.iter().map(|h| h.runs()).sum::<u64>() >= 2, 20_000).await,
        "connected, one holder runs: {} / {}", holders(&agents[0]), holders(&agents[1]));
    let holder = if handles[0].runs() > handles[1].runs() { 0 } else { 1 };
    let other = 1 - holder;
    assert_eq!(handles[other].runs(), 0, "only the holder ran while connected");

    // Cut the link both ways at the frame layer.
    for (i, ag) in agents.iter().enumerate() {
        // The one peer's frames, and an anti-entropy answer from a connection not yet named (sender 0): this node has
        // no other peer.
        let peer = ids[1 - i].id_hash();
        ag.task_ctx.kv_state.frame_filter.pin().insert(40, Arc::new(move |v: &FrameView<'_>| match v {
            FrameView::Kv { sender, .. } => *sender == peer || *sender == 0,
            FrameView::Signal { .. } => true,
        }));
    }
    let cut = std::time::Instant::now();
    assert!(poll_until(|| handles[other].runs() >= 1, 20_000).await, "the other side holds once the holder's lease has evaporated from its view");
    assert!(cut.elapsed() >= Duration::from_millis(1_500), "not before the advertisements time out: {:?}", cut.elapsed());
    let both_ran_one = || ledger.by_key().values().any(|(applied, attempts)| *applied == 1 && *attempts == 2);
    assert!(poll_until(both_ran_one, 10_000).await, "two holders ran one slot: {:?}", ledger.by_key());
    assert!(ledger.by_key().values().all(|(applied, _)| *applied == 1), "every key applied once at the destination: {:?}", ledger.by_key());

    // Heal: one holds again, and the loser says it no longer holds.
    for ag in &agents { ag.task_ctx.kv_state.frame_filter.pin().remove(&40); }
    let candidates = |ag: &GossipAgent| ag.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).len();
    let stepped_down = sinks.iter().map(|s| traced(s, RUN_RULE, "not_holder")).sum::<usize>();
    assert!(poll_until(|| candidates(&agents[0]) == 2 && candidates(&agents[1]) == 2
            && sinks.iter().map(|s| traced(s, RUN_RULE, "not_holder")).sum::<usize>() > stepped_down, 20_000).await,
        "healed, the candidates meet and the rendezvous loser says it no longer holds");
    assert!(poll_until(|| holders(&agents[0]) == 1 && holders(&agents[1]) == 1, 20_000).await,
        "one holder once healed: {} / {}", holders(&agents[0]), holders(&agents[1]));
    let before: Vec<u64> = handles.iter().map(|h| h.runs()).collect();
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let ran: Vec<bool> = handles.iter().zip(&before).map(|(h, b)| h.runs() > *b).collect();
    assert_eq!(ran.iter().filter(|r| **r).count(), 1, "one node runs once healed: {ran:?}");
    assert!(ledger.by_key().values().all(|(applied, _)| *applied == 1), "{:?}", ledger.by_key());
    drop(handles);
    for ag in agents { ag.shutdown().await; }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Y2's exit: a missed run in the trace with its reason. A holder whose wall clock jumps five periods — what a
/// paused process sees — finds slots no one ran; under `run_once` it decides one run for the gap, records the
/// decision with the gap and the policy, and the run names that record its parent. The jump goes through the
/// seam (`sim_seam::step_wall_clock_for_test`), the clock the loop reads — a hook of the non-`sim` build; under
/// `sim` the kernel owns the clock.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_missed_run_is_a_decision_in_the_trace_with_its_reason() {
    for (policy, reason, run_reason) in [(MissedRunPolicy::RunOnce, "ran_once", Some("ran_for_gap")), (MissedRunPolicy::Skip, "skipped", None)] {
        let (a, sink, _band) = banded(vec![], alloc_port()).await;
        let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        let h = a.run_schedule(&decl("close", 2, policy), move |run| {
            s2.lock().unwrap().push(run);
            async { Ok(()) }
        }).unwrap();
        assert!(poll_until(|| h.runs() >= 1, 15_000).await, "the holder runs the slot due");
        let first = seen.lock().unwrap()[0].slot;
        mycelium_core::sim_seam::step_wall_clock_for_test(5 * 2_000);
        let missed = || sink.snapshot().into_iter().find(|r| r.rule == MISSED_RULE);
        assert!(poll_until(|| missed().is_some(), 10_000).await, "{policy:?}: the gap is decided");
        mycelium_core::sim_seam::step_wall_clock_for_test(0);
        let m = missed().unwrap();
        assert_eq!(m.reason, reason, "{policy:?}");
        let policy_input = m.inputs.iter().find(|i| i.source.contains("missed-run policy")).expect("the policy is an input");
        assert!(policy_input.value.contains(&format!("first={}", first + 1)), "{policy:?}: the gap is named: {}", policy_input.value);
        assert!(m.inputs.iter().any(|i| i.source.contains("sched/{id}/last") && i.value == first.to_string()), "{policy:?}: the last run is an input: {m:?}");
        let records = sink.snapshot();
        let m_seq = records.iter().find(|r| r.rule == MISSED_RULE).map(|r| r.seq).unwrap();
        match run_reason {
            Some(rr) => {
                let run = records.iter().find(|r| r.rule == RUN_RULE && r.reason == rr).expect("the gap's run is recorded");
                assert_eq!(run.parent_seq, Some(m_seq), "the run names the decision its parent");
                let gap = seen.lock().unwrap().iter().find(|r| matches!(r.kind, RunKind::ForGap { .. })).cloned().expect("the work saw the gap run");
                assert_eq!(gap.covers().start(), &(first + 1));
            }
            None => assert!(seen.lock().unwrap().iter().all(|r| r.kind == RunKind::OnTime), "skip runs nothing for the gap"),
        }
        drop(h);
        a.shutdown().await;
    }
}

/// A run recorded ahead of this node's clock — a peer whose clock runs fast ran the slot early — is said once
/// (`last_ahead`) and nothing runs until this clock reaches it; the fleet's `last` is the highest entry under the
/// schedule's period and phase, so this node's later, older run does not lower it, and an entry written under
/// another period does not count.
#[tokio::test]
async fn a_run_recorded_ahead_of_this_clock_is_said_and_runs_nothing_until_reached() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let s = Schedule::from_decl(&decl("close", 2, MissedRunPolicy::Skip)).unwrap();
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(poll_until(|| h.runs() >= 1, 15_000).await);
    let now_slot = s.slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    // A fast peer's entry, ten slots ahead; and one under another period, further still, which must not count.
    let _ = a.kv().set(format!("{}10.0.0.9:9000", last_prefix("close")), format!("{} 2000 0", now_slot + 10));
    let _ = a.kv().set(format!("{}10.0.0.8:9000", last_prefix("close")), format!("{} 60000 0", now_slot + 1_000));
    assert_eq!(fleet_last(&a.task_ctx.kv_state, &s), Some(now_slot + 10), "the highest entry under this period and phase");
    assert!(poll_until(|| traced(&sink, RUN_RULE, "last_ahead") >= 1, 10_000).await, "a run ahead of the clock is said");
    let runs = h.runs();
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert_eq!(h.runs(), runs, "nothing runs while the recorded run is ahead of the clock");
    assert_eq!(traced(&sink, RUN_RULE, "last_ahead"), 1, "said once, not every tick");
    assert_eq!(fleet_last(&a.task_ctx.kv_state, &s), Some(now_slot + 10), "`last` is not lowered");
    drop(h);
    a.shutdown().await;
}

/// A host whose capabilities do not match `holders` is not a candidate: it never holds, never runs, and says so
/// once.
#[tokio::test]
async fn a_host_outside_the_holder_band_never_runs() {
    let a = agent(alloc_port(), vec![]);
    let sink = Arc::new(DecisionSink::default());
    a.with_decision_trace(Arc::clone(&sink));
    a.start().await.unwrap();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(poll_until(|| traced(&sink, RUN_RULE, "not_eligible") >= 1, 5_000).await);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(h.runs(), 0);
    assert_eq!(traced(&sink, RUN_RULE, "not_eligible"), 1, "said once, not every tick");
    assert!(a.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).is_empty(), "no candidacy advertised");
    drop(h);
    a.shutdown().await;
}

/// A loop warms up before it may hold, and work that overruns its period is abandoned: the overrun traced, the
/// holding withdrawn, the loop alive.
#[tokio::test]
async fn work_that_overruns_its_period_is_abandoned_and_traced() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let started = std::time::Instant::now();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| std::future::pending::<Result<(), String>>()).unwrap();
    assert!(poll_until(|| traced(&sink, RUN_RULE, "warming_up") >= 1, 5_000).await, "the warm-up is said");
    assert!(poll_until(|| traced(&sink, RUN_RULE, "work_overran") >= 1, 15_000).await, "the overrun is traced");
    assert!(started.elapsed() >= Duration::from_millis(warm_up_ms(1_000)), "no run before the warm-up: {:?}", started.elapsed());
    assert!(poll_until(|| traced(&sink, RUN_RULE, "work_overran") >= 2, 10_000).await, "the loop goes on after an overrun");
    assert_eq!(h.runs(), 0);
    assert!(h.failures() >= 2);
    drop(h);
    a.shutdown().await;
}

/// A late run that fails is traced and the runs after it go on; the on-time run lands.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_failed_late_run_is_traced_and_the_runs_after_it_go_on() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::RunLate), |run: ScheduledRun| async move {
        if run.kind == RunKind::Late { Err("late runs fail here".to_string()) } else { Ok(()) }
    }).unwrap();
    assert!(poll_until(|| h.runs() >= 1, 15_000).await);
    mycelium_core::sim_seam::step_wall_clock_for_test(3 * 2_000);
    assert!(poll_until(|| traced(&sink, RUN_RULE, "work_failed") >= 2 && h.runs() >= 2, 10_000).await,
        "both late runs failed and the on-time run still ran: {:?}", sink.snapshot().iter().map(|r| r.reason.clone()).collect::<Vec<_>>());
    mycelium_core::sim_seam::step_wall_clock_for_test(0);
    drop(h);
    a.shutdown().await;
}

/// One id is one schedule fleet-wide: two nodes declaring it with different periods both refuse to hold, and say
/// why; a second loop for an id this node runs is refused.
#[tokio::test]
async fn one_id_declared_two_ways_holds_nowhere_and_one_node_runs_one_loop_per_id() {
    let ports = [alloc_port(), alloc_port()];
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let (a, sa, _ba) = banded(vec![ids[1].clone()], ports[0]).await;
    let (b, sb, _bb) = banded(vec![ids[0].clone()], ports[1]).await;
    let ha = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).is_err(), "a second loop for one id");
    let hb = b.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(poll_until(|| traced(&sa, RUN_RULE, "declaration_mismatch") >= 1 && traced(&sb, RUN_RULE, "declaration_mismatch") >= 1, 20_000).await,
        "both say the declarations disagree");
    let runs = (ha.runs(), hb.runs());
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!((ha.runs(), hb.runs()), runs, "neither runs while they disagree");
    drop(ha);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let again = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) });
    assert!(again.is_ok(), "a stopped loop frees its id");
    drop(again);
    drop(hb);
    a.shutdown().await;
    b.shutdown().await;
}

/// `schedule/` is the loop's namespace: an advertisement there through the public door writes nothing.
#[tokio::test]
async fn the_schedule_namespace_is_refused_at_the_public_advertise_door() {
    let a = agent(alloc_port(), vec![]);
    a.start().await.unwrap();
    let _reg = a.capabilities().advertise_capability(Capability::new(CANDIDATE_NS, "close"), Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).is_empty(), "nothing advertised under `schedule`");
    a.shutdown().await;
}
