//! Declared recurring work (adopter-surface Y2): the pure decision, the loader's refusals, and the loop on real
//! agents — two holders under a partition giving one effect, and a missed run in the trace with its reason.

use super::*;
use crate::capability_config::FilterDecl;
use crate::effects::external::{EffectClass, ExternalDestination, ExternalEffect, IdempotencyKey, ProbeAnswer, ProbeBudget, Reconciler};
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
    for id in ["", "a/b", "a b", "close.holder"] {
        let e = refused(&format!("[[schedule]]\nid = {id:?}\nperiod_secs = 60\n{holders}"));
        assert!(e.contains("schedule.id"), "{id:?}: {e}");
    }
    let e = refused(&format!("{ok}[[schedule]]\nid = \"close\"\nperiod_secs = 30\n{holders}"));
    assert!(e.contains("schedule.id") && e.contains("same id"), "{e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\non_missed = \"catch_up\"\n{holders}"));
    assert!(e.contains("on_missed") || e.contains("catch_up"), "{e}");
    let e = refused(&format!("[[schedule]]\nid = \"close\"\nperiod_secs = 60\ncron = \"* * *\"\n{holders}"));
    assert!(e.contains("cron"), "an unknown key is refused, not ignored: {e}");
}

// ── The loop, on real agents ─────────────────────────────────────────────────────────────────────────

/// A destination that deduplicates on the key, as X5's idempotent class declares: the second send of one key
/// with one content is `Replayed`, applied once.
#[derive(Default)]
struct Ledger {
    entries: Mutex<HashMap<String, (u64, u32, u32)>>, // key → (content hash, applications, attempts)
}

impl Ledger {
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
        let fresh = e.1 == 0;
        if fresh { e.1 = 1; }
        Ok(DestinationCommit::new("ledger", if fresh { DedupOutcome::Fresh } else { DedupOutcome::Replayed }))
    }
    fn observe(&self, key: &IdempotencyKey) -> ProbeAnswer {
        if self.entries.lock().unwrap().contains_key(key.operation_id.as_str()) { ProbeAnswer::Done } else { ProbeAnswer::NotDone }
    }
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

/// Y2's exit: two holders under a partition, one effect at the destination. Two nodes that cannot reach each
/// other each see themselves as the only candidate, so each holds and each runs the slot; each sends the run's
/// effect through its own reconciler under the run's key, and the destination applies it once. Healed, the
/// ring's rendezvous winner holds alone.
#[tokio::test]
async fn two_holders_under_a_partition_give_one_effect_at_the_destination() {
    let ports = [alloc_port(), alloc_port()];
    let ids: Vec<NodeId> = ports.iter().map(|p| NodeId::new("127.0.0.1", *p).unwrap()).collect();
    let agents: Vec<GossipAgent> = ports.iter().map(|p| agent(*p, vec![])).collect(); // no seeds: partitioned
    let sinks: Vec<Arc<DecisionSink>> = agents.iter().map(|a| { let s = Arc::new(DecisionSink::default()); a.with_decision_trace(Arc::clone(&s)); s }).collect();
    for a in &agents { a.start().await.unwrap(); }
    let _band: Vec<_> = agents.iter().map(|a| a.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(30))).collect();

    let dir = std::env::temp_dir().join(format!("mycelium-schedule-partition-{}-{}", std::process::id(), ports[0]));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = Arc::new(Ledger::default());
    let mut handles = Vec::new();
    for (i, a) in agents.iter().enumerate() {
        let rec = Arc::new(Reconciler::open(dir.join(format!("ledger-{i}.journal")), Arc::clone(&ledger), crate::EgressPolicy::default()).unwrap());
        handles.push(a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), move |run: ScheduledRun| {
            let rec = Arc::clone(&rec);
            async move {
                let op = run.operation_id();
                let ext = ExternalEffect {
                    effect: Effect::new(op.clone(), AttemptId::fresh(&op), format!("close slot {}", run.slot).into_bytes()),
                    class: EffectClass::Idempotent { compensation: None },
                    probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 60_000 },
                    composition: None,
                };
                rec.apply(&ext, None).await.map(|_| ()).map_err(|e| e.to_string())
            }
        }).unwrap());
    }
    // Both hold: a slot each ran, and the destination applied once.
    let both_ran_one = || ledger.by_key().values().any(|(applied, attempts)| *applied == 1 && *attempts == 2);
    assert!(poll_until(both_ran_one, 10_000).await, "two holders ran one slot: {:?}", ledger.by_key());
    let by_key = ledger.by_key();
    assert!(by_key.values().all(|(applied, _)| *applied == 1), "every key applied once at the destination: {by_key:?}");
    for (i, s) in sinks.iter().enumerate() {
        assert!(s.snapshot().iter().any(|r| r.rule == RUN_RULE && r.reason == "ran"), "node {i} held under the partition and ran");
    }

    // Healed: the candidates see each other, and one holds.
    agents[0].connect_peer(ids[1].clone());
    agents[1].connect_peer(ids[0].clone());
    let holders = |a: &GossipAgent| a.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close.holder")).len();
    let candidates = |a: &GossipAgent| a.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).len();
    assert!(poll_until(|| candidates(&agents[0]) == 2 && candidates(&agents[1]) == 2, 15_000).await, "the candidates meet");
    assert!(poll_until(|| holders(&agents[0]) == 1 && holders(&agents[1]) == 1, 15_000).await,
        "one holder once healed: {} / {}", holders(&agents[0]), holders(&agents[1]));
    let stepped_down = sinks.iter().filter(|s| s.snapshot().iter().any(|r| r.rule == RUN_RULE && r.reason == "not_holder")).count();
    assert_eq!(stepped_down, 1, "the rendezvous loser records that it no longer holds");
    let before: Vec<u64> = handles.iter().map(|h| h.runs()).collect();
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let ran: Vec<bool> = handles.iter().zip(&before).map(|(h, b)| h.runs() > *b).collect();
    assert_eq!(ran.iter().filter(|r| **r).count(), 1, "one node runs once healed: {ran:?}");
    assert!(ledger.by_key().values().all(|(applied, _)| *applied == 1), "{:?}", ledger.by_key());
    drop(handles);
    for a in agents { a.shutdown().await; }
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
        let a = agent(alloc_port(), vec![]);
        let sink = Arc::new(DecisionSink::default());
        a.with_decision_trace(Arc::clone(&sink));
        a.start().await.unwrap();
        let _band = a.capabilities().advertise_capability(Capability::new("ledger", "closer"), Duration::from_secs(30));
        let seen: Arc<Mutex<Vec<ScheduledRun>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        let h = a.run_schedule(&decl("close", 2, policy), move |run| {
            s2.lock().unwrap().push(run);
            async { Ok(()) }
        }).unwrap();
        assert!(poll_until(|| h.runs() >= 1, 10_000).await, "the holder runs the slot due");
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

/// A host whose capabilities do not match `holders` is not a candidate: it never holds, never runs, and says so
/// once.
#[tokio::test]
async fn a_host_outside_the_holder_band_never_runs() {
    let a = agent(alloc_port(), vec![]);
    let sink = Arc::new(DecisionSink::default());
    a.with_decision_trace(Arc::clone(&sink));
    a.start().await.unwrap();
    let h = a.run_schedule(&decl("close", 1, MissedRunPolicy::Skip), |_| async { Ok(()) }).unwrap();
    assert!(poll_until(|| sink.snapshot().iter().any(|r| r.reason == "not_eligible"), 5_000).await);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(h.runs(), 0);
    assert_eq!(sink.snapshot().iter().filter(|r| r.reason == "not_eligible").count(), 1, "said once, not every tick");
    assert!(a.capabilities().resolve(&CapFilter::new(CANDIDATE_NS, "close")).is_empty(), "no candidacy advertised");
    drop(h);
    a.shutdown().await;
}
