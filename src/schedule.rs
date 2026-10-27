//! **Declared recurring work** (adopter-surface Y2, D15): a `[[schedule]]` in a unit file, run by the
//! application that embeds the node.
//!
//! This is the voluntary singleton, not a scheduler. Every node that runs a schedule and matches its
//! `holders` filter advertises itself a **candidate** (`cap/{node}/schedule/{id}`); the candidates order
//! themselves by rendezvous on the ring `sched/{id}` ([`crate::election`]), and the first holds. Holding is
//! advertised as `cap/{node}/schedule/{id}.holder` and lasts only while the holder re-asserts it: a holder that
//! stops is timed out by its readers like any advertisement — **the lease is the evaporation**. Nothing is
//! assigned and nothing is locked, so a partition has **two holders**, one per side; that is why every run
//! carries an idempotency key, `sched/{id}/{slot}` ([`ScheduledRun::operation_id`]), and why its effect goes
//! through X5's contract (`effects::external`): two holders, one effect at a destination that deduplicates.
//!
//! **A slot** is a period counted from the Unix epoch plus the phase: the slot due at `now` is
//! `(now − phase) / period`. The node's wall clock is read through the replay seam
//! ([`sim_seam::wall_now_ms`](mycelium_core::sim_seam::wall_now_ms)) and the loop ticks through the timer seam,
//! so a recording replays the ticks. The last slot run is written to `sched/{id}/last` (last-writer-wins, kept
//! across a handover), so a new holder knows what its predecessor ran.
//!
//! **A missed run is a decision.** Slots after the last recorded run and before the one due now — a holder that
//! was down, a handover longer than a period, a paused process — are decided by the schedule's
//! [`MissedRunPolicy`]: skipped, run late each under its own key, or run once for the gap. The decision is
//! catalogued (`sched.missed`) and recorded in the node's decision trace with its reason and its inputs; each
//! run it causes names it as its parent. A schedule with no recorded run starts at the slot due now: nothing
//! before the first run is counted missed.
//!
//! **Not built, stated:** no cron-expression language beyond a period and a phase; no fleet-wide calendar; no
//! exclusivity — a partition runs a slot on both sides, and only the destination's deduplication makes that one
//! effect; work that fails is retried at the next tick under the same key, without a backoff of its own.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycelium_core::decision::{DecisionRecord, InputSnapshot, OutcomeKind, Provenance, ViewStatus};

use crate::capability::{CapFilter, Capability};
use crate::capability_config::{MissedRunPolicy, NodeCapabilityConfig, ScheduleDecl};
use crate::error::GossipError;
use crate::node_id::NodeId;
use crate::OperationId;

/// The rule a holder's tick decides under: run the slot due, or say why not.
pub const RUN_RULE: &str = "sched.run";
/// The rule a holder decides missed runs under.
pub const MISSED_RULE: &str = "sched.missed";
/// The capability namespace a schedule's candidates and holder advertise in.
pub const CANDIDATE_NS: &str = "schedule";
/// The suffix of the holder's advertisement name: `schedule/{id}.holder`.
pub const HOLDER_SUFFIX: &str = ".holder";
/// The most missed slots `run_late` runs in one tick; older ones are skipped and counted.
pub const MAX_CATCH_UP: u64 = 64;

/// The KV key holding the last slot the fleet ran for `id` — `sched/{id}/last`, the slot as decimal ASCII.
pub fn last_key(id: &str) -> String {
    format!("sched/{id}/last")
}

/// Why `id` cannot name a schedule, or `None`. One key segment — it appears in `sched/{id}/last`, the
/// candidate's capability name and every run's idempotency key — and not ending in `.holder`, which would make
/// it another schedule's holder advertisement.
pub fn id_problem(id: &str) -> Option<&'static str> {
    if id.is_empty() {
        return Some("is empty");
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Some("is not one key segment ([A-Za-z0-9._-]+)");
    }
    if id.ends_with(HOLDER_SUFFIX) {
        return Some("ends in `.holder`, the suffix of a schedule's holder advertisement");
    }
    if crate::capability::capability_name_problem(CANDIDATE_NS, id).is_some() {
        return Some("is reserved as a capability name");
    }
    None
}

/// A validated schedule: the declaration in milliseconds, its holder filter converted.
#[derive(Clone, Debug)]
pub struct Schedule {
    id: Arc<str>,
    period_ms: u64,
    phase_ms: u64,
    on_missed: MissedRunPolicy,
    holders: CapFilter,
}

impl Schedule {
    /// The schedule a `[[schedule]]` declares, refused by the same rules as the unit-file loader.
    pub fn from_decl(decl: &ScheduleDecl) -> Result<Self, GossipError> {
        NodeCapabilityConfig { schedules: vec![decl.clone()], ..Default::default() }.validate()?;
        let holders = decl.holders.to_filter().map_err(|e| GossipError::InvalidField { field: "schedule.holders", reason: e })?;
        Ok(Self {
            id: Arc::from(decl.id.as_str()),
            period_ms: decl.period_secs * 1_000,
            phase_ms: decl.phase_secs * 1_000,
            on_missed: decl.on_missed,
            holders,
        })
    }

    /// The schedule's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The period in milliseconds.
    pub fn period_ms(&self) -> u64 {
        self.period_ms
    }

    /// The slot due at `now_ms` (Unix ms), or `None` before the first slot's start.
    pub fn slot_at(&self, now_ms: u64) -> Option<u64> {
        now_ms.checked_sub(self.phase_ms).map(|t| t / self.period_ms)
    }

    /// When `slot` starts, in Unix ms.
    pub fn slot_start_ms(&self, slot: u64) -> u64 {
        slot.saturating_mul(self.period_ms).saturating_add(self.phase_ms)
    }

    /// The election ring: the schedule's own name, so two schedules over one band spread their holders.
    pub fn ring(&self) -> String {
        format!("sched/{}", self.id)
    }
}

/// Why a run is happening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// The slot due now.
    OnTime,
    /// A missed slot, run late under its own key (`run_late`).
    Late,
    /// One run for a gap of missed slots, from `first` to the run's slot (`run_once`).
    ForGap { first: u64 },
}

/// One run of a schedule, handed to the application's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledRun {
    /// The schedule's id.
    pub schedule: Arc<str>,
    /// The slot this run is for — for a [`RunKind::ForGap`] run, the gap's newest slot.
    pub slot: u64,
    /// Why it runs.
    pub kind: RunKind,
    /// When the slot started, in Unix ms.
    pub slot_start_ms: u64,
}

impl ScheduledRun {
    /// The run's idempotency key, `sched/{id}/{slot}`: the operation id an effect the run causes carries, so two
    /// holders running one slot produce one effect at a destination that deduplicates (X5).
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!("sched/{}/{}", self.schedule, self.slot))
    }

    /// The slots this run stands for: its own, or a gap's.
    pub fn covers(&self) -> std::ops::RangeInclusive<u64> {
        match self.kind {
            RunKind::ForGap { first } => first..=self.slot,
            _ => self.slot..=self.slot,
        }
    }
}

/// The `sched.missed` decision: what happened to slots `first..=last`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missed {
    /// The oldest missed slot.
    pub first: u64,
    /// The newest missed slot.
    pub last: u64,
    /// The decision's outcome kind.
    pub outcome: OutcomeKind,
    /// Its reason: `skipped`, `ran_late`, `ran_late_capped` or `ran_once`.
    pub reason: &'static str,
    /// Missed slots nothing runs for (all of them under `skip`; those past [`MAX_CATCH_UP`] under `run_late`).
    pub skipped: u64,
}

/// What one tick decided — pure, from the schedule, the time, whether this node holds, and the last slot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The slot due, if any.
    pub slot: Option<u64>,
    /// The `sched.run` outcome for the tick.
    pub outcome: OutcomeKind,
    /// Its reason: `ran`, `not_due`, `not_holder` or `already_ran`.
    pub reason: &'static str,
    /// The missed-run decision, when there was a gap.
    pub missed: Option<Missed>,
    /// The runs, oldest first; the slot due now last.
    pub runs: Vec<ScheduledRun>,
}

/// One tick's decision. `last_run` is the last slot the fleet recorded running (`sched/{id}/last`).
pub fn decide(s: &Schedule, now_ms: u64, holding: bool, last_run: Option<u64>) -> Tick {
    let none = |slot, outcome, reason| Tick { slot, outcome, reason, missed: None, runs: Vec::new() };
    let Some(slot) = s.slot_at(now_ms) else { return none(None, OutcomeKind::NoAction, "not_due") };
    if !holding {
        return none(Some(slot), OutcomeKind::NoAction, "not_holder");
    }
    if last_run.is_some_and(|l| l >= slot) {
        return none(Some(slot), OutcomeKind::NoAction, "already_ran");
    }
    let run = |slot, kind| ScheduledRun { schedule: Arc::clone(&s.id), slot, kind, slot_start_ms: s.slot_start_ms(slot) };
    let mut runs = Vec::new();
    let missed = match last_run {
        Some(l) if slot - l > 1 => {
            let (first, last) = (l + 1, slot - 1);
            let gap = last - first + 1;
            Some(match s.on_missed {
                MissedRunPolicy::Skip => Missed { first, last, outcome: OutcomeKind::NoAction, reason: "skipped", skipped: gap },
                MissedRunPolicy::RunLate => {
                    let from = first.max(slot.saturating_sub(MAX_CATCH_UP));
                    runs.extend((from..=last).map(|m| run(m, RunKind::Late)));
                    let skipped = from - first;
                    Missed { first, last, outcome: OutcomeKind::Action, reason: if skipped == 0 { "ran_late" } else { "ran_late_capped" }, skipped }
                }
                MissedRunPolicy::RunOnce => {
                    runs.push(run(last, RunKind::ForGap { first }));
                    Missed { first, last, outcome: OutcomeKind::Action, reason: "ran_once", skipped: 0 }
                }
            })
        }
        _ => None,
    };
    runs.push(run(slot, RunKind::OnTime));
    Tick { slot: Some(slot), outcome: OutcomeKind::Action, reason: "ran", missed, runs }
}

/// A running schedule. Dropping it (or [`stop`](Self::stop)) ends the loop and withdraws its advertisements.
pub struct ScheduleHandle {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    runs: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
}

impl ScheduleHandle {
    /// Stop the loop.
    pub fn stop(mut self) {
        self.stop.take();
    }

    /// Runs this node's work completed.
    pub fn runs(&self) -> u64 {
        self.runs.load(Ordering::Relaxed)
    }

    /// Runs whose work failed (each retried at the next tick under the same key).
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}

impl Drop for ScheduleHandle {
    fn drop(&mut self) {
        self.stop.take();
    }
}

/// How often the loop looks: a tenth of the period, between 100 ms and 5 s.
fn check_interval_ms(period_ms: u64) -> u64 {
    (period_ms / 10).clamp(100, 5_000)
}

/// How often the candidate and holder advertisements re-assert: the check interval, at least a second. A
/// reader times one out at three of these — the lease.
fn lease_refresh(period_ms: u64) -> Duration {
    Duration::from_millis(check_interval_ms(period_ms).max(1_000))
}

impl crate::GossipAgent {
    /// Run a `[[schedule]]` on this node: take part in electing its holder, and while holding, run `work` for
    /// each slot under the missed-run policy — `mycelium::schedule` has the whole contract. `work` returns
    /// `Err` to have the slot retried at the next tick; an effect it causes should carry
    /// [`ScheduledRun::operation_id`] as its idempotency key. Decisions go to this agent's decision trace
    /// ([`with_decision_trace`](Self::with_decision_trace)) when one is attached.
    ///
    /// # Errors
    /// The declaration refused by the unit-file loader's rules, by name.
    pub fn run_schedule<F, Fut>(&self, decl: &ScheduleDecl, work: F) -> Result<ScheduleHandle, GossipError>
    where
        F: Fn(ScheduledRun) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let schedule = Schedule::from_decl(decl)?;
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let runs = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let ctx = Arc::clone(&self.task_ctx);
        let (caps, kv, me) = (self.capabilities(), self.kv(), self.node_id().clone());
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let (r, f) = (Arc::clone(&runs), Arc::clone(&failures));
        self.spawn_task(async move {
            let mut runner = Runner { schedule, caps, kv, me, ctx, candidacy: None, holding: None, was: None, runs: r, failures: f };
            let mut ticker = mycelium_core::sim_seam::interval_ms(
                format!("sched/{}", runner.schedule.id), check_interval_ms(runner.schedule.period_ms), tokio::time::MissedTickBehavior::Skip,
            );
            let mut stop_rx = stop_rx;
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = shutdown_rx.changed() => break,
                    _ = ticker.tick() => runner.tick(&work).await,
                }
            }
        });
        Ok(ScheduleHandle { stop: Some(stop_tx), runs, failures })
    }
}

/// The loop's state: its advertisements, and what it last said about holding (so a non-holder records a
/// change, not every tick).
struct Runner {
    schedule: Schedule,
    caps: crate::CapabilitiesHandle,
    kv: crate::KvHandle,
    me: NodeId,
    ctx: Arc<crate::agent::TaskCtx>,
    candidacy: Option<crate::CapabilityReg>,
    holding: Option<crate::CapabilityReg>,
    was: Option<&'static str>,
    runs: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
}

impl Runner {
    async fn tick<F, Fut>(&mut self, work: &F)
    where
        F: Fn(ScheduledRun) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let s = &self.schedule;
        let now = mycelium_core::sim_seam::wall_now_ms();
        let eligible = self.caps.resolve(&s.holders).iter().any(|(n, _)| *n == self.me);
        if eligible && self.candidacy.is_none() {
            let (attr, value) = crate::election::rule_attribute();
            self.candidacy = Some(self.caps.advertise_capability(Capability::new(CANDIDATE_NS, s.id.as_ref()).with(attr, value), lease_refresh(s.period_ms)));
        } else if !eligible {
            self.candidacy = None;
        }
        let candidates = self.caps.resolve(&CapFilter::new(CANDIDATE_NS, s.id.as_ref()));
        if eligible && !candidates.iter().any(|(n, _)| *n == self.me) {
            return; // this node's own candidacy is not in its view yet: nothing to decide on until it is
        }
        let winner = crate::election::elect(&s.ring(), &candidates).cloned();
        let holding = eligible && winner.as_ref() == Some(&self.me);
        if holding && self.holding.is_none() {
            self.holding = Some(self.caps.advertise_capability(Capability::new(CANDIDATE_NS, format!("{}{HOLDER_SUFFIX}", s.id)), lease_refresh(s.period_ms)));
        } else if !holding {
            self.holding = None;
        }
        let last_raw = self.kv.get(&last_key(&s.id));
        let last = last_raw.as_ref().and_then(|b| std::str::from_utf8(b).ok()?.trim().parse::<u64>().ok());
        let tick = decide(s, now, holding, last);

        let view = || InputSnapshot::new(
            "the schedule's candidates as gossiped (`cap/*/schedule/{id}`), this node's eligibility against `holders`, the ring's rendezvous winner",
            format!("candidates={} eligible={eligible} winner={}", candidates.len(), winner.as_ref().map_or("none".to_string(), |w| w.to_string())),
            Provenance::Gossiped,
        );
        let last_input = || InputSnapshot::new("the last slot the fleet ran (`sched/{id}/last`, as gossiped)",
            last.map_or("none".to_string(), |l| l.to_string()), Provenance::Gossiped);
        let clock = || InputSnapshot::new("the wall clock through the seam, and the slot it falls in",
            format!("now_ms={now} slot={} period_ms={} phase_ms={}", tick.slot.map_or("none".into(), |x| x.to_string()), s.period_ms, s.phase_ms), Provenance::Local);
        let record = |rec: DecisionRecord| self.ctx.decision_sink().and_then(|sink| sink.record(rec).ok());

        // A non-holder records when what it says changes, not every tick: the trace keeps what was decided.
        if tick.runs.is_empty() {
            let reason = if !eligible { "not_eligible" } else { tick.reason };
            if matches!(reason, "not_holder" | "not_eligible") && self.was != Some(reason) {
                let rec = DecisionRecord::new(RUN_RULE, 1, "schedule tick", OutcomeKind::NoAction, reason)
                    .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown).no_parent().no_effect()
                    .input(view()).input(clock());
                record(rec);
            }
            self.was = Some(reason);
            return;
        }
        self.was = Some("ran");

        let parent = tick.missed.as_ref().and_then(|m| {
            let rec = DecisionRecord::new(MISSED_RULE, 1, "a gap between the last run and the slot due", m.outcome, m.reason)
                .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown).no_parent()
                .input(last_input()).input(clock())
                .input(InputSnapshot::new("the schedule's missed-run policy", format!("{:?} first={} last={} skipped={} cap={MAX_CATCH_UP}", s.on_missed, m.first, m.last, m.skipped), Provenance::Local));
            let rec = if m.outcome == OutcomeKind::Action { rec.effect(format!("runs for slots {}..={} follow, each naming this record its parent", m.first, m.last)) } else { rec.no_effect() };
            record(rec)
        });
        for run in tick.runs {
            let reason = match run.kind { RunKind::OnTime => "ran", RunKind::Late => "ran_late", RunKind::ForGap { .. } => "ran_for_gap" };
            let key = run.operation_id();
            let slot = run.slot;
            let outcome = work(run).await;
            let mut rec = DecisionRecord::new(RUN_RULE, 1, "schedule tick", if outcome.is_ok() { OutcomeKind::Action } else { OutcomeKind::Deferral },
                    if outcome.is_ok() { reason } else { "work_failed" })
                .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown)
                .input(view()).input(last_input()).input(clock());
            rec = match parent { Some(p) => rec.parent(p), None => rec.no_parent() };
            match outcome {
                Ok(()) => {
                    let _ = self.kv.set(last_key(&s.id), slot.to_string());
                    self.runs.fetch_add(1, Ordering::Relaxed);
                    record(rec.effect(format!("work done under {key}; sched/{}/last = {slot}", s.id)));
                }
                Err(e) => {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    record(rec.effect(format!("work failed under {key}: {e}; retried at the next tick")));
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
