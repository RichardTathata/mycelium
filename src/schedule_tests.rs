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
    // The lease is three refreshes (3 s) from the holder's last write, which landed at most one refresh before the cut.
    assert!(cut.elapsed() >= Duration::from_millis(1_900) && cut.elapsed() <= Duration::from_millis(8_000),
        "the takeover follows the lease, not before it nor long after: {:?}", cut.elapsed());
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
        // #740's review: the schedule's records carry their rules' catalogued revisions.
        crate::rules::assert_catalogued_revisions(&records, &[RUN_RULE, MISSED_RULE]);
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
    assert_eq!(fleet_last(&a.task_ctx.kv_state, &s, Some(now_slot)).0, Some(now_slot + 10), "the highest entry under this period and phase");
    assert!(poll_until(|| traced(&sink, RUN_RULE, "last_ahead") >= 1, 10_000).await, "a run ahead of the clock is said");
    let runs = h.runs();
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert_eq!(h.runs(), runs, "nothing runs while the recorded run is ahead of the clock");
    assert_eq!(traced(&sink, RUN_RULE, "last_ahead"), 1, "said once, not every tick");
    assert_eq!(fleet_last(&a.task_ctx.kv_state, &s, Some(now_slot)).0, Some(now_slot + 10), "`last` is not lowered");
    // #740's confirm round: every `sched.run` site carries the catalogued revision, this one included.
    crate::rules::assert_catalogued_revisions(&sink.snapshot(), &[RUN_RULE]);
    drop(h);
    a.shutdown().await;
}

/// #694's confirmation round, N1: an entry further ahead than `MAX_AHEAD_SLOTS` — a clock a year ahead on a node
/// since gone — is left out, said once, and the schedule runs; nothing would ever clear it otherwise.
#[tokio::test]
async fn a_run_recorded_too_far_ahead_is_left_out_said_once_and_the_schedule_runs() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let s = Schedule::from_decl(&decl("close", 1, MissedRunPolicy::Skip)).unwrap();
    let now_slot = s.slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    let year = 365 * 86_400;
    let _ = a.kv().set(format!("{}10.0.0.7:9000", last_prefix("close")), format!("{} 1000 0", now_slot + year));
    assert_eq!(fleet_last(&a.task_ctx.kv_state, &s, Some(now_slot)), (None, 1), "left out, and counted");
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(poll_until(|| h.runs() >= 2, 15_000).await, "the schedule runs past a year-ahead entry");
    assert_eq!(traced(&sink, RUN_RULE, "ignored_last_far_ahead"), 1, "said once");
    assert_eq!(traced(&sink, RUN_RULE, "last_ahead"), 0);
    // #740's confirm round: every `sched.run` site carries the catalogued revision, this one included.
    crate::rules::assert_catalogued_revisions(&sink.snapshot(), &[RUN_RULE]);
    drop(h);
    a.shutdown().await;
}

/// A `work` that panics takes its loop down, and the id is free again: the next `run_schedule` for it is accepted.
#[tokio::test]
async fn a_panicking_work_frees_the_id() {
    let (a, _sink, _band) = banded(vec![], alloc_port()).await;
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { panic!("work panicked") }).unwrap();
    let free = || a.task_ctx.schedules_running.pin().get("close").is_none();
    assert!(poll_until(free, 15_000).await, "the panic unwound the loop and freed the id");
    drop(h);
    assert!(a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).is_ok());
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

/// R6-05: a failed late run is not retried *when a later run of its tick completes* — that run's record covers it.
/// When none does (here the on-time run fails too), the slot is still the fleet's next and the next tick runs it again,
/// under the same key; the failure's trace says exactly that, not "not retried".
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_failed_late_run_is_run_again_when_nothing_after_it_completes_and_the_trace_says_so() {
    use std::sync::atomic::AtomicBool;
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let fail = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let (f2, s2) = (Arc::clone(&fail), Arc::clone(&seen));
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::RunLate), move |run: ScheduledRun| {
        s2.lock().unwrap().push(run.clone());
        // Every run of the first catch-up tick fails, its on-time run last; every run after it succeeds.
        let failing = f2.load(Ordering::SeqCst);
        if failing && run.kind == RunKind::OnTime { f2.store(false, Ordering::SeqCst); }
        async move { if failing { Err("this tick's runs all fail".to_string()) } else { Ok(()) } }
    }).unwrap();
    assert!(poll_until(|| h.runs() >= 1, 15_000).await);
    fail.store(true, Ordering::SeqCst);
    mycelium_core::sim_seam::step_wall_clock_for_test(3 * 2_000);
    let late_failure = || sink.snapshot().into_iter().find(|r| r.rule == RUN_RULE && r.reason == "work_failed");
    assert!(poll_until(|| late_failure().is_some() && !fail.load(Ordering::SeqCst) && h.runs() >= 2, 10_000).await,
        "the failing tick ran and a later one succeeded: {:?}", sink.snapshot().iter().map(|r| r.reason.clone()).collect::<Vec<_>>());
    mycelium_core::sim_seam::step_wall_clock_for_test(0);
    let effect = late_failure().unwrap().effect.unwrap_or_default();
    assert!(effect.contains("the runs after it go on; the slot is run again only if no later run of this tick completes"),
        "the late failure's trace says when it is run again: {effect}");
    assert!(!effect.contains("not retried"), "{effect}");
    let seen = seen.lock().unwrap().clone();
    let first_late = seen.iter().find(|r| r.kind == RunKind::Late).expect("a late run").slot;
    assert!(seen.iter().filter(|r| r.slot == first_late && r.kind == RunKind::Late).count() >= 2,
        "with nothing after it completing, the failed late slot is run again: {seen:?}");
    drop(h);
    a.shutdown().await;
}

// ── ER2 (R6-02): one rule for the reader and the writer of `sched/{id}/last/{node}` ─────────────────────

fn sched(period_secs: u64, phase_secs: u64) -> Schedule {
    let mut d = decl("close", period_secs, MissedRunPolicy::Skip);
    d.phase_secs = phase_secs;
    Schedule::from_decl(&d).unwrap()
}

#[test]
fn recorded_after_a_lengthened_period_is_the_slot_run() {
    // An entry under a 1 s period, read by a 2 s declaration: its slot counts twice as fast, and no reader counts it.
    assert_eq!(recorded(&sched(2, 0), Some((1_990, 1_000, 0)), 1_000, Some(1_000)), 1_000);
}

#[test]
fn recorded_over_a_poisoned_entry_is_the_slot_run() {
    // Same declaration, far past the reader's bound: the reader leaves it out, so the writer does not keep it.
    assert_eq!(recorded(&sched(2, 0), Some((1_000_000_000, 2_000, 0)), 1_000, Some(1_000)), 1_000);
}

#[test]
fn recorded_under_the_same_declaration_is_never_lowered() {
    assert_eq!(recorded(&sched(2, 0), Some((990, 2_000, 0)), 1_000, Some(1_000)), 1_000, "a lower entry: the slot run");
    assert_eq!(recorded(&sched(2, 0), Some((1_000, 2_000, 0)), 990, Some(1_000)), 1_000, "a late run: the entry kept");
}

#[test]
fn recorded_under_the_same_declaration_ahead_within_the_bound_is_kept() {
    let s = sched(2, 0);
    assert_eq!(recorded(&s, Some((1_010, 2_000, 0)), 1_000, Some(1_000)), 1_010);
    assert_eq!(recorded(&s, Some((1_000 + MAX_AHEAD_SLOTS, 2_000, 0)), 1_000, Some(1_000)), 1_000 + MAX_AHEAD_SLOTS, "on the bound: kept");
    assert_eq!(recorded(&s, Some((1_001 + MAX_AHEAD_SLOTS, 2_000, 0)), 1_000, Some(1_000)), 1_000, "one past it: replaced");
}

#[test]
fn recorded_after_a_phase_change_is_the_slot_run() {
    // Phase 0 → 1 s: the old entry's slot can be one ahead of the new phase's; kept, it would skip that slot.
    assert_eq!(recorded(&sched(2, 1), Some((1_001, 2_000, 0)), 1_000, Some(1_000)), 1_000);
}

#[test]
fn parse_last_reads_the_three_numbers_and_refuses_anything_else() {
    assert_eq!(parse_last(b"7 2000 0"), Some((7, 2_000, 0)));
    assert_eq!(parse_last(b"7 2000 0 extra"), Some((7, 2_000, 0)), "as the reader and the route always read it");
    assert_eq!(parse_last(b"7 2000"), None);
    assert_eq!(parse_last(b"x 2000 0"), None);
    assert_eq!(parse_last(&[0xff, 0x20]), None);
}

/// This node's own `last` entry for `id`, parsed.
fn own_entry(a: &GossipAgent, id: &str) -> Option<(u64, u64, u64)> {
    parse_last(&a.kv().get(&format!("{}{}", last_prefix(id), a.node_id()))?)
}

fn seed_own(a: &GossipAgent, id: &str, value: String) {
    assert!(a.kv().set(format!("{}{}", last_prefix(id), a.node_id()), value), "seed this node's last");
}

/// A work that records each run and succeeds.
fn recording(seen: &Arc<Mutex<Vec<ScheduledRun>>>) -> impl Fn(ScheduledRun) -> std::future::Ready<Result<(), String>> + Send + Sync + 'static {
    let seen = Arc::clone(seen);
    move |run| {
        seen.lock().unwrap().push(run);
        std::future::ready(Ok(()))
    }
}

fn slots(seen: &Arc<Mutex<Vec<ScheduledRun>>>) -> Vec<u64> {
    seen.lock().unwrap().iter().map(|r| r.slot).collect()
}

fn assert_unique(slots: &[u64]) {
    let unique: std::collections::BTreeSet<_> = slots.iter().collect();
    assert_eq!(unique.len(), slots.len(), "each slot run once: {slots:?}");
}

/// R6-02: this node's entry from a one-second declaration, then the schedule run at two seconds. The old entry's slot
/// is about twice the new one; kept, the writer would make it an entry of the new declaration that no reader counts,
/// and the slot due would run again every check interval.
#[tokio::test]
async fn lengthening_a_period_runs_each_slot_once() {
    let (a, _sink, _band) = banded(vec![], alloc_port()).await;
    let old = sched(1, 0).slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    seed_own(&a, "close", format!("{old} 1000 0"));
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), recording(&seen)).unwrap();
    assert!(poll_until(|| h.runs() >= 3, 20_000).await, "three runs: {:?}", slots(&seen));
    drop(h);
    let ran = slots(&seen);
    assert_unique(&ran);
    assert_eq!(own_entry(&a, "close"), Some((*ran.iter().max().unwrap(), 2_000, 0)), "the entry is the last slot run, under the new declaration");
    a.shutdown().await;
}

/// R6-02, D14: an entry of this declaration far past the reader's bound — written by the unfixed writer, or a clock
/// once far ahead — is replaced by the next run, and the trace says so.
#[tokio::test]
async fn a_poisoned_entry_is_replaced_by_the_next_run() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let now_slot = sched(2, 0).slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    seed_own(&a, "close", format!("{} 2000 0", now_slot * 2));
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), recording(&seen)).unwrap();
    assert!(poll_until(|| h.runs() >= 2, 20_000).await, "two runs: {:?}", slots(&seen));
    drop(h);
    let ran = slots(&seen);
    assert_unique(&ran);
    assert_eq!(own_entry(&a, "close").map(|e| e.0), ran.iter().max().copied(), "healed: the entry is the last slot run");
    let replaced = sink.snapshot().into_iter().filter(|r| r.rule == RUN_RULE && r.reason == "ran")
        .filter_map(|r| r.effect).find(|e| e.contains("replacing")).expect("the first run says it replaced the entry");
    assert!(replaced.contains(&format!("{} 2000 0", now_slot * 2)) && replaced.contains("no reader counts"), "{replaced}");
    a.shutdown().await;
}

/// R6-02 as the reviewer ran it: the documented rollout — stop the schedule, start it again on the same node with a
/// longer period — runs each slot once and records the new declaration's slot.
#[tokio::test]
async fn stopping_and_restarting_with_a_longer_period_runs_each_slot_once() {
    let (a, _sink, _band) = banded(vec![], alloc_port()).await;
    let seen1: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), recording(&seen1)).unwrap();
    assert!(poll_until(|| h.runs() >= 1, 15_000).await);
    h.stop();
    assert_eq!(own_entry(&a, "close").map(|e| e.1), Some(1_000));
    assert!(poll_until(|| a.task_ctx.schedules_running.pin().get("close").is_none(), 5_000).await, "the loop ended");
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), recording(&seen)).unwrap();
    assert!(poll_until(|| h.runs() >= 3, 20_000).await, "three runs: {:?}", slots(&seen));
    drop(h);
    let ran = slots(&seen);
    assert_unique(&ran);
    assert_eq!(own_entry(&a, "close"), Some((*ran.iter().max().unwrap(), 2_000, 0)));
    a.shutdown().await;
}

/// A clock held still through the seam until it is dropped.
#[cfg(not(feature = "sim"))]
struct Pinned;
#[cfg(not(feature = "sim"))]
impl Pinned {
    fn at(ms: u64) -> Self {
        mycelium_core::sim_seam::pin_wall_clock_for_test(Some(ms));
        Pinned
    }
}
#[cfg(not(feature = "sim"))]
impl Drop for Pinned {
    fn drop(&mut self) {
        mycelium_core::sim_seam::pin_wall_clock_for_test(None);
    }
}

/// R6-02's phase case (the design's §1): this node ran slot `S` of a two-second schedule at phase 0; restarted at
/// phase 1 s half a second into that slot, the slot due is `S − 1` of the new phase. Kept, the old `S` would be
/// recorded under the new phase, and the new phase's slot `S` would read as already run — skipped.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_phase_change_does_not_skip_a_slot() {
    let (a, _sink, _band) = banded(vec![], alloc_port()).await;
    let s_old = mycelium_core::sim_seam::wall_now_ms() / 2_000;
    let mut pin = Pinned::at(s_old * 2_000 + 500);
    seed_own(&a, "close", format!("{s_old} 2000 0"));
    let mut d = decl("close", 2, MissedRunPolicy::Skip);
    d.phase_secs = 1;
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&d, recording(&seen)).unwrap();
    assert!(poll_until(|| h.runs() >= 1, 15_000).await, "the new phase's slot due runs");
    assert_eq!(slots(&seen), vec![s_old - 1]);
    drop(pin);
    pin = Pinned::at(s_old * 2_000 + 1_100); // the new phase's slot `S` has begun
    let ran = poll_until(|| h.runs() >= 2, 3_000).await;
    drop(h);
    drop(pin);
    assert!(ran, "the new phase's next slot runs, not skipped: {:?}", slots(&seen));
    assert_eq!(slots(&seen), vec![s_old - 1, s_old]);
    assert_eq!(own_entry(&a, "close"), Some((s_old, 2_000, 1_000)));
    a.shutdown().await;
}

/// The design's §1, second trigger, as it reaches a node: **not fixed by ER2, and pinned as it is.** A node that ran
/// with its clock ahead has its entry stamped ahead too; it comes back with its clock corrected, and the entry comes
/// back **with its original stamp** — here from its own store (`[persistence]`: replay, then `hlc.observe` of the
/// highest stamp, `src/agent/lifecycle.rs`); from a peer it would come by anti-entropy, observed the same way. Within
/// `max_clock_drift_ms` (five minutes by default) the node's clock is carried up to the entry's stamp, so every
/// advertisement it writes is stamped ahead of its wall clock and read as from the future (`CapEntry::is_fresh`): the
/// node runs nothing until its clock catches up — silent for the length of the step — and then the entry is counted
/// as any entry of its declaration. Past the drift bound the entry holds its key against the node's own writes (the
/// ledger row `what-is-proven.md#far-future-stamp-holds-its-key`), pinned by
/// `a_replacement_that_loses_to_a_newer_stamp_is_traced_superseded_not_replaced`.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_clock_corrected_within_the_drift_bound_leaves_the_node_silent_until_it_catches_up() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let ahead_ms = (2 * MAX_AHEAD_SLOTS as i64) * 1_000;
    let port = alloc_port();
    let data = std::env::temp_dir().join(format!("mycelium-schedule-clock-{}-{port}", std::process::id()));
    assert!(data.starts_with(std::env::temp_dir()) && data.file_name().is_some());
    let _ = std::fs::remove_dir_all(&data);
    /// Removes the persistence directory however the test ends, a panic included.
    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            if self.0.starts_with(std::env::temp_dir()) && self.0.file_name().is_some() {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
    let _cleanup = RemoveOnDrop(data.clone());
    /// Sets the wall-clock step back to zero however the test ends.
    struct Unstep;
    impl Drop for Unstep {
        fn drop(&mut self) {
            mycelium_core::sim_seam::step_wall_clock_for_test(0);
        }
    }
    let _unstep = Unstep;
    let persisted = || {
        let mut cfg = GossipConfig::auto();
        cfg.bind_port = port;
        cfg.persistence = Some(PersistenceConfig { base_path: data.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
        GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg)
    };

    mycelium_core::sim_seam::step_wall_clock_for_test(ahead_ms);
    let a = persisted();
    a.start().await.unwrap();
    let band = a.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(1));
    let first: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), recording(&first)).unwrap();
    let ran_ahead = poll_until(|| h.runs() >= 1, 15_000).await;
    drop(h);
    let entry = a.kv().get(&format!("{}{}", last_prefix("close"), a.node_id()));
    drop(band);
    a.shutdown().await;
    drop(a);
    mycelium_core::sim_seam::step_wall_clock_for_test(0);
    assert!(ran_ahead, "the node ran a slot with its clock ahead");
    let entry = entry.expect("the slot it ran is recorded");

    let b = persisted();
    b.start().await.unwrap();
    let _band = b.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(1));
    assert_eq!(b.kv().get(&format!("{}{}", last_prefix("close"), b.node_id())).as_deref(), Some(&entry[..]), "the entry is back from the store");
    let now_slot = sched(1, 0).slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    assert!(parse_last(&entry).unwrap().0 > now_slot + MAX_AHEAD_SLOTS, "the entry is past the reader's bound");
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = b.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), recording(&seen)).unwrap();
    // Past the warm-up (three one-second refreshes) and three periods more.
    tokio::time::sleep(Duration::from_millis(warm_up_ms(1_000) + 3_000)).await;
    let runs = h.runs();
    drop(h);
    let still = b.kv().get(&format!("{}{}", last_prefix("close"), b.node_id()));
    b.shutdown().await;
    assert_eq!(runs, 0, "silent until the clock catches up: {:?}", slots(&seen));
    assert_eq!(still.as_deref(), Some(&entry[..]), "the entry is untouched");
}

/// The trace says what the write did (the ack names its rung). An entry of this declaration far past the reader's
/// bound **and stamped past the drift bound** — a peer whose clock was an hour ahead — holds its key against this
/// node's write (LWW: the ledger row `what-is-proven.md#far-future-stamp-holds-its-key`). The run's trace says the
/// write was superseded and the entry stays uncounted, not that it was replaced; the entry holds, and the slot is run
/// again each check interval until the clock passes the stamp — the general far-future-stamp limit, not fixed here.
///
/// **The entry is applied straight to the store**, so this node's HLC never observes it. A real peer frame would be
/// observed first, carrying the HLC up to the drift bound (now + `max_clock_drift_ms`, five minutes by default): the
/// node's own advertisements would read as from the future and it would be silent for those five minutes before the
/// run this test traces.
#[tokio::test]
async fn a_replacement_that_loses_to_a_newer_stamp_is_traced_superseded_not_replaced() {
    let (a, sink, _band) = banded(vec![], alloc_port()).await;
    let now_ms = mycelium_core::sim_seam::wall_now_ms();
    let now_slot = sched(2, 0).slot_at(now_ms).unwrap();
    let key = format!("{}{}", last_prefix("close"), a.node_id());
    let poisoned = format!("{} 2000 0", now_slot * 2);
    let update = mycelium_core::framing::make_gossip_update_stamped(
        a.node_id(), 3, Arc::from(key.as_str()), bytes::Bytes::from(poisoned.clone()), false,
        mycelium_core::hlc::pack(now_ms + 3_600_000, 0),
    );
    crate::store::apply_and_notify(&a.task_ctx.kv_state, &update);
    let h = a.run_schedule(&decl("close", 2, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    let ran = || sink.snapshot().into_iter().filter(|r| r.rule == RUN_RULE && r.reason == "ran").filter_map(|r| r.effect).collect::<Vec<_>>();
    assert!(poll_until(|| !ran().is_empty(), 15_000).await, "the slot runs");
    drop(h);
    let effect = ran()[0].clone();
    assert!(effect.contains("superseded by a newer stamp; the entry stays uncounted"), "{effect}");
    assert!(!effect.contains("replacing"), "no replacement is claimed: {effect}");
    assert_eq!(a.kv().get(&key).as_deref(), Some(poisoned.as_bytes()), "the newer-stamped entry holds its key");
    a.shutdown().await;
}

/// A control: shortening a period records fresh slots — the new declaration's slots are about twice the old entry's,
/// so the maximum was already the slot run.
#[tokio::test]
async fn shortening_a_period_records_fresh_slots() {
    let (a, _sink, _band) = banded(vec![], alloc_port()).await;
    let old = sched(2, 0).slot_at(mycelium_core::sim_seam::wall_now_ms()).unwrap();
    seed_own(&a, "close", format!("{old} 2000 0"));
    let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), recording(&seen)).unwrap();
    assert!(poll_until(|| h.runs() >= 3, 20_000).await, "three runs: {:?}", slots(&seen));
    drop(h);
    let ran = slots(&seen);
    assert_unique(&ran);
    assert_eq!(own_entry(&a, "close"), Some((*ran.iter().max().unwrap(), 1_000, 0)));
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
