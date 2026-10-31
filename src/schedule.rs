//! **Declared recurring work** (adopter-surface Y2, D15): a `[[schedule]]` in a unit file, run by the
//! application that embeds the node — or, for an SDK agent, by the gateway's node standing in the band for the client
//! (`GET /gateway/schedules/run`, Y12, which runs this loop with [`spawn_schedule`] and hands each run to the client).
//!
//! This is the voluntary singleton, not a scheduler. Every node that runs a schedule and matches its
//! `holders` filter advertises itself a **candidate** (`cap/{node}/schedule/{id}`, carrying the period and phase it
//! declares); the candidates order themselves by rendezvous on the ring `sched/{id}` ([`crate::election`]), and the
//! first holds. Holding is advertised as `cap/{node}/schedule/{id}.holder` and lasts only while it is re-asserted:
//! a holder that stops is timed out by its readers like any advertisement — **the lease is the evaporation**. The
//! advertisements are re-asserted by the node's persist task, not by the loop; what ties them to the loop is that a
//! run is given **one period** to finish (a deadline through the seam), and work that overruns it is abandoned, the
//! overrun traced (`work_overran`) and the holding and candidacy withdrawn until the loop's next tick — when, being the
//! same rendezvous winner, it holds again: a holder whose work always overruns re-holds every tick and never hands
//! over, and its trace says `work_overran` each time. (A loop whose
//! executor thread is blocked by synchronous work stops ticking while its advertisements are still re-asserted: work
//! that blocks belongs on a blocking thread.) Nothing is assigned and nothing is locked, so a partition has **two
//! holders**, one per side; that is why every run carries an idempotency key, `sched/{id}/{slot}`
//! ([`ScheduledRun::operation_id`]), and why its effect goes through X5's contract (`effects::external`): two
//! holders, one effect at a destination that deduplicates — when the content is the run's alone.
//!
//! **One id is one schedule fleet-wide.** A candidate whose advertisement declares another period or phase is a
//! conflict: no node holds while it is in view (`declaration_mismatch`, traced), and `wire-check` refuses two
//! declarations of one id that differ.
//!
//! **A slot** is a period counted from the Unix epoch plus the phase: the slot due at `now` is
//! `(now − phase) / period`. The node's wall clock is read through the replay seam
//! ([`sim_seam::wall_now_ms`](mycelium_core::sim_seam::wall_now_ms)) and the loop ticks through the timer seam.
//! Each node that runs a slot writes `sched/{id}/last/{node}`, never lowering it; the fleet's last run is the
//! highest entry under the same period and phase, so a holder that ran an older slot later cannot pull it back and
//! a restarted node's stale entry does not count against the fleet's. **A clock ahead** runs its slots early and
//! records them: a holder whose clock is behind that record runs nothing until it reaches it, and says so once
//! (`last_ahead`) — for at most [`MAX_AHEAD_SLOTS`] slots: an entry further ahead is left out and that is said once
//! (`ignored_last_far_ahead`). There is one entry per node that ever ran the schedule, kept for good: no route
//! clears one (the raw KV routes refuse `sched/`), and the bound is what limits a stale one's reach.
//!
//! **Changing a schedule's period or phase** is a new declaration under the same id: while nodes run both, every
//! candidate sees a mismatch and **the schedule runs nowhere** for the rollout. Roll a change by declaring it under a
//! new id (and retiring the old one once the new one runs), or by stopping the schedule everywhere and starting it
//! again with the new declaration.
//!
//! **A loop waits** before it may hold: three lease refreshes ([`warm_up_ms`]) after it starts, so that its peers'
//! advertisements and `last` entries have had that long to reach it before it holds or decides a gap. A delay, not a
//! convergence bound: nothing checks that they did arrive, and a node slow to hear its peers can still hold on its own
//! view.
//!
//! **A missed run is a decision.** Slots after the last recorded run and before the one due now — a holder that
//! was down, a handover longer than a period, a paused process — are decided by the schedule's
//! [`MissedRunPolicy`]: skipped, run late each under its own key, or run once for the gap. The decision is
//! catalogued (`sched.missed`) and recorded in the node's decision trace with its reason and its inputs; each
//! run it causes names it as its parent. A schedule with no recorded run starts at the slot due now: nothing
//! before the first run is counted missed. A `run_once` run is keyed by the gap's newest slot — it *is* that slot's
//! run, so a holder that ran the slot on time and one that runs it for a gap make one effect, provided the content
//! depends on the key alone (content that names the gap would be a `Conflict` at the destination). A failed late or
//! gap run is traced and not retried; the runs after it go on. A failed on-time run is retried at the next tick.
//!
//! **Not built, stated:** no cron-expression language beyond a period and a phase; no fleet-wide calendar; no
//! exclusivity — a partition runs a slot on both sides, and only the destination's deduplication makes that one
//! effect; no backoff of its own for failing work; `mycelium test --faults` has no schedule class. **A recording
//! replays it:** the loop's clock, tick and deadline go through the seams, and `tests/schedule_replay.rs` replays the
//! corpus bundle `tests/replay-corpus/y2-schedule-run/` — a node holding a schedule, deciding a missed run, a run that
//! overruns its deadline, then the gap's run — decision for decision in another process. `run_late`, `skip` and a
//! failed run are covered by `decide`'s unit tests, not a recording.

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
/// How far ahead of this node's clock a recorded run is believed: an entry in `sched/{id}/last/` more than this many
/// slots past the slot due here is left out (and said once, `ignored_last_far_ahead`), so a clock far ahead — a
/// departed node's — cannot silence a schedule until real time catches up. The run_late cap, reused.
pub const MAX_AHEAD_SLOTS: u64 = MAX_CATCH_UP;

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
    if id.starts_with('.') || id.contains("..") {
        return Some("starts with `.` or holds `..`, which reads as a path segment");
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
    /// Its reason: `ran`, `not_due`, `not_holder`, `already_ran` or `last_ahead`.
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
    if last_run == Some(slot) {
        return none(Some(slot), OutcomeKind::NoAction, "already_ran");
    }
    if last_run.is_some_and(|l| l > slot) {
        // A run recorded for a slot this node's clock has not reached: a clock ahead somewhere in the fleet (or here,
        // before a step back). Nothing runs until this clock reaches it — said, never silent.
        return none(Some(slot), OutcomeKind::NoAction, "last_ahead");
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

    /// Runs whose work failed or overran its deadline.
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

/// How long a loop waits after it starts before it may hold: three lease refreshes, the time a peer's
/// advertisement — and its `last` — is given to reach this node or time out of its view. A delay: nothing checks
/// that the view converged in it.
pub fn warm_up_ms(period_ms: u64) -> u64 {
    3 * lease_refresh(period_ms).as_millis() as u64
}

/// The prefix of a schedule's `last` entries — one per node that has held it, `sched/{id}/last/{node}`.
pub fn last_prefix(id: &str) -> String {
    format!("sched/{id}/last/")
}

/// The fleet's last run of `s`, as this node has it: the **highest** slot among the entries written under the same
/// period and phase — monotonic, whichever holder wrote last, so a node that ran an older slot later cannot lower it —
/// leaving out any entry more than [`MAX_AHEAD_SLOTS`] slots ahead of `now_slot`, which a clock far ahead (a departed
/// node's, say) would otherwise make the fleet wait for. Returns the slot and how many entries it left out.
fn fleet_last(kv: &crate::store::KvState, s: &Schedule, now_slot: Option<u64>) -> (Option<u64>, usize) {
    let mut ignored = 0;
    let last = crate::store::scan_kv_prefix(kv, &last_prefix(&s.id))
        .into_iter()
        .filter_map(|(_, v)| {
            let text = std::str::from_utf8(&v).ok()?.to_string();
            let mut parts = text.split_whitespace();
            let slot = parts.next()?.parse::<u64>().ok()?;
            let period = parts.next()?.parse::<u64>().ok()?;
            let phase = parts.next()?.parse::<u64>().ok()?;
            (period == s.period_ms && phase == s.phase_ms).then_some(slot)
        })
        .filter(|slot| {
            let too_far = now_slot.is_some_and(|n| *slot > n.saturating_add(MAX_AHEAD_SLOTS));
            if too_far { ignored += 1; }
            !too_far
        })
        .max();
    (last, ignored)
}

/// A candidate's declaration as its advertisement carries it: `(period_ms, phase_ms)`.
fn declared(cap: &Capability) -> Option<(u64, u64)> {
    let int = |k: &str| match cap.attributes.get(k) {
        Some(crate::capability::CapValue::Integer(v)) if *v >= 0 => Some(*v as u64),
        _ => None,
    };
    Some((int("period_ms")?, int("phase_ms")?))
}

/// Why a run's work did not complete: what [`spawn_schedule`]'s work returns. [`GossipAgent::run_schedule`]'s work
/// fails with a `String`, which is [`Failed`](Self::Failed); the gateway's run stream (adopter-surface Y12) also
/// reports a run it handed to a client that never acknowledged it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunFailure {
    /// The work ran and failed: an on-time slot is retried at the next tick.
    Failed(String),
    /// The run was handed on and no answer came back — the client went away. Nothing is known about whether it ran,
    /// so the slot is **not** recorded: the next holder, or this one's next tick, decides it as a slot not yet run
    /// (on time, or as a missed run under the policy). Traced `unacknowledged`; this node withdraws its holding.
    /// Constructed by the gateway's run stream only, so a build without `gateway` never makes one.
    #[cfg_attr(not(feature = "gateway"), allow(dead_code))]
    Unacknowledged(String),
}

impl crate::GossipAgent {
    /// Run a `[[schedule]]` on this node: take part in electing its holder, and while holding, run `work` for
    /// each slot under the missed-run policy — `mycelium::schedule` has the whole contract. `work` returns
    /// `Err` to have an on-time slot retried at the next tick, and is given one period to finish: past it, the run
    /// is abandoned, the overrun traced, and this node withdraws its holding until its next tick. An effect `work`
    /// causes should carry [`ScheduledRun::operation_id`] as its idempotency key, with content that depends on the
    /// run alone. Decisions go to this agent's decision trace
    /// ([`with_decision_trace`](Self::with_decision_trace)) when one is attached.
    ///
    /// # Errors
    /// The declaration refused by the unit-file loader's rules, by name; a second loop for an id this node already
    /// runs.
    pub fn run_schedule<F, Fut>(&self, decl: &ScheduleDecl, work: F) -> Result<ScheduleHandle, GossipError>
    where
        F: Fn(ScheduledRun) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let work = Arc::new(work);
        spawn_schedule(&self.task_ctx, decl, LoopOptions::default(), move |run| {
            let fut = work(run);
            async move { fut.await.map_err(RunFailure::Failed) }
        })
        .map_err(SpawnError::into_gossip)
    }
}

/// Called with each change of what a loop says when it runs nothing (`not_eligible`, `warming_up`, `not_holder`,
/// `stood_down`, …): the gateway's run stream forwards it to the client as a `status` event.
pub(crate) type StatusFn = Arc<dyn Fn(&'static str) + Send + Sync>;

/// How a loop behaves beyond [`GossipAgent::run_schedule`]'s contract. The default is that contract.
#[derive(Default, Clone)]
pub(crate) struct LoopOptions {
    /// **Stand down on a stall** (the gateway's run stream, Y12): work that overruns its period withdraws this node's
    /// candidacy for [`stand_down_ms`], so another node's candidate wins the ring rather than this one re-holding at
    /// its next tick. An application's own work keeps `run_schedule`'s rule (it re-holds, and says `work_overran`).
    pub stand_down_on_stall: bool,
    pub on_status: Option<StatusFn>,
}

/// Why [`spawn_schedule`] did not start a loop.
#[derive(Debug)]
pub(crate) enum SpawnError {
    /// The declaration, refused by the unit-file loader's rules.
    Invalid(GossipError),
    /// This node already runs a loop for the id.
    Running(GossipError),
}

impl SpawnError {
    fn into_gossip(self) -> GossipError {
        match self {
            SpawnError::Invalid(e) | SpawnError::Running(e) => e,
        }
    }
}

/// How long a stalled client stream's node stands down: ten periods, and at least twice the warm-up, so a peer's
/// candidacy is heard, warms up and holds before this node is a candidate again.
pub fn stand_down_ms(period_ms: u64) -> u64 {
    (10 * period_ms).max(2 * warm_up_ms(period_ms))
}

/// The schedule loop, on the node `ctx` belongs to: [`GossipAgent::run_schedule`]'s body, shared with the gateway's
/// run stream (`GET /gateway/schedules/run`, Y12), which stands this node in the holder band for a client.
pub(crate) fn spawn_schedule<F, Fut>(
    ctx: &Arc<crate::agent::TaskCtx>,
    decl: &ScheduleDecl,
    options: LoopOptions,
    work: F,
) -> Result<ScheduleHandle, SpawnError>
where
    F: Fn(ScheduledRun) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), RunFailure>> + Send + 'static,
{
    let schedule = Schedule::from_decl(decl).map_err(SpawnError::Invalid)?;
    if ctx.schedules_running.pin().insert(Arc::clone(&schedule.id), ()).is_some() {
        return Err(SpawnError::Running(GossipError::InvalidField { field: "schedule.id", reason: format!(
            "{:?}: this node already runs a schedule with this id — one loop per id per node", decl.id
        ) }));
    }
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let runs = Arc::new(AtomicU64::new(0));
    let failures = Arc::new(AtomicU64::new(0));
    let caps = crate::CapabilitiesHandle { ctx: Arc::clone(ctx) };
    let kv = crate::KvHandle::from_core(Arc::clone(&ctx.core));
    let me = ctx.node_id.clone();
    let running = Arc::clone(&ctx.schedules_running);
    let mut shutdown_rx = ctx.shutdown_tx.subscribe();
    let (r, f) = (Arc::clone(&runs), Arc::clone(&failures));
    let loop_ctx = Arc::clone(ctx);
    ctx.spawn_task(async move {
        let id = Arc::clone(&schedule.id);
        // Frees the id however the loop ends — a stop, a shutdown, or `work` panicking through it.
        let _free = FreeId { running, id: Arc::clone(&id) };
        let started_ns = mycelium_core::sim_seam::mono_now_ns();
        let mut runner = Runner {
            schedule, caps, kv, me, ctx: loop_ctx, started_ns, candidacy: None, holding: None, was: None, ignored: 0,
            runs: r, failures: f, options, stood_down_until_ns: None,
        };
        let mut ticker = mycelium_core::sim_seam::interval_ms(
            format!("sched/{id}"), check_interval_ms(runner.schedule.period_ms), tokio::time::MissedTickBehavior::Skip,
        );
        let mut stop_rx = stop_rx;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = shutdown_rx.changed() => break,
                _ = ticker.tick() => runner.tick(&work).await,
            }
        }
        drop(runner);
    });
    Ok(ScheduleHandle { stop: Some(stop_tx), runs, failures })
}

/// Removes a schedule's id from the node's running set when the loop's task ends, by any path.
struct FreeId {
    running: Arc<papaya::HashMap<Arc<str>, ()>>,
    id: Arc<str>,
}

impl Drop for FreeId {
    fn drop(&mut self) {
        self.running.pin().remove(&self.id);
    }
}

/// The loop's state: its advertisements, and what it last said when it ran nothing (so a non-holder records a
/// change, not every tick).
struct Runner {
    schedule: Schedule,
    caps: crate::CapabilitiesHandle,
    kv: crate::KvHandle,
    me: NodeId,
    ctx: Arc<crate::agent::TaskCtx>,
    started_ns: u64,
    candidacy: Option<crate::CapabilityReg>,
    holding: Option<crate::CapabilityReg>,
    was: Option<&'static str>,
    /// How many far-ahead `last` entries the last tick left out, so a change is said once.
    ignored: usize,
    runs: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
    options: LoopOptions,
    /// Set when a stalled client's run overran ([`LoopOptions::stand_down_on_stall`]): no candidacy until then.
    stood_down_until_ns: Option<u64>,
}

impl Runner {
    /// Record `rec` in the agent's trace, returning its sequence number.
    fn record(&self, rec: DecisionRecord) -> Option<u64> {
        self.ctx.decision_sink().and_then(|sink| sink.record(rec).ok())
    }

    /// A `sched.run` record of a tick that runs nothing, once per change of what it says.
    fn say(&mut self, outcome: OutcomeKind, reason: &'static str, inputs: Vec<InputSnapshot>) {
        if self.was != Some(reason) {
            let mut rec = DecisionRecord::new(RUN_RULE, 1, "schedule tick", outcome, reason)
                .node(self.me.to_string(), 0).target(self.schedule.id.to_string()).view(ViewStatus::Unknown).no_parent().no_effect();
            for i in inputs { rec = rec.input(i); }
            self.record(rec);
            if let Some(on_status) = &self.options.on_status {
                on_status(reason);
            }
        }
        self.was = Some(reason);
    }

    async fn tick<F, Fut>(&mut self, work: &F)
    where
        F: Fn(ScheduledRun) -> Fut,
        Fut: Future<Output = Result<(), RunFailure>>,
    {
        let s = self.schedule.clone();
        if let Some(until) = self.stood_down_until_ns {
            if mycelium_core::sim_seam::mono_now_ns() < until {
                self.candidacy = None;
                self.holding = None;
                return self.say(OutcomeKind::NoAction, "stood_down", vec![InputSnapshot::new(
                    "a run of this node's client overran its period: its candidacy is withdrawn for the stand-down",
                    format!("stand_down_ms={}", stand_down_ms(s.period_ms)), Provenance::Local)]);
            }
            self.stood_down_until_ns = None;
        }
        let now = mycelium_core::sim_seam::wall_now_ms();
        let eligible = self.caps.resolve(&s.holders).iter().any(|(n, _)| *n == self.me);
        if eligible && self.candidacy.is_none() {
            let (attr, value) = crate::election::rule_attribute();
            let cap = Capability::new(CANDIDATE_NS, s.id.as_ref()).with(attr, value)
                .with("period_ms", crate::capability::CapValue::Integer(s.period_ms as i64))
                .with("phase_ms", crate::capability::CapValue::Integer(s.phase_ms as i64));
            self.candidacy = Some(self.caps.advertise_unchecked(cap, lease_refresh(s.period_ms)));
        } else if !eligible {
            self.candidacy = None;
        }
        let candidates = self.caps.resolve(&CapFilter::new(CANDIDATE_NS, s.id.as_ref()));
        let clock = |slot: Option<u64>| InputSnapshot::new("the wall clock through the seam, and the slot it falls in",
            format!("now_ms={now} slot={} period_ms={} phase_ms={}", slot.map_or("none".into(), |x| x.to_string()), s.period_ms, s.phase_ms), Provenance::Local);
        if !eligible {
            self.holding = None;
            return self.say(OutcomeKind::NoAction, "not_eligible", vec![clock(s.slot_at(now))]);
        }
        if !candidates.iter().any(|(n, _)| *n == self.me) {
            return; // this node's own candidacy is not in its view yet: nothing to decide on until it is
        }
        let warm = warm_up_ms(s.period_ms);
        if mycelium_core::sim_seam::mono_since(self.started_ns) < Duration::from_millis(warm) {
            return self.say(OutcomeKind::Deferral, "warming_up", vec![InputSnapshot::new(
                "the time since this loop started, against the warm-up (three lease refreshes)", format!("warm_up_ms={warm}"), Provenance::Local)]);
        }
        // One id is one schedule fleet-wide: a candidate declaring another period or phase makes every key ambiguous,
        // so no node holds until the declarations agree.
        let mismatched: Vec<String> = candidates.iter()
            .filter(|(_, c)| declared(c) != Some((s.period_ms, s.phase_ms)))
            .map(|(n, c)| format!("{n}:{:?}", declared(c)))
            .collect();
        if !mismatched.is_empty() {
            self.holding = None;
            return self.say(OutcomeKind::Refusal, "declaration_mismatch", vec![InputSnapshot::new(
                "candidates whose advertisement declares another period or phase (`cap/*/schedule/{id}`, as gossiped)",
                format!("{} (this node: {}/{})", mismatched.join(", "), s.period_ms, s.phase_ms), Provenance::Gossiped)]);
        }
        let winner = crate::election::elect(&s.ring(), &candidates).cloned();
        let holding = winner.as_ref() == Some(&self.me);
        if holding && self.holding.is_none() {
            self.holding = Some(self.caps.advertise_unchecked(Capability::new(CANDIDATE_NS, format!("{}{HOLDER_SUFFIX}", s.id)), lease_refresh(s.period_ms)));
        } else if !holding {
            self.holding = None;
        }
        let (last, ignored) = fleet_last(&self.ctx.kv_state, &s, s.slot_at(now));
        if ignored != self.ignored {
            if ignored > 0 {
                self.record(DecisionRecord::new(RUN_RULE, 1, "schedule tick", OutcomeKind::NoAction, "ignored_last_far_ahead")
                    .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown).no_parent().no_effect()
                    .input(InputSnapshot::new("`sched/{id}/last/{node}` entries more than MAX_AHEAD_SLOTS past this node's slot, left out",
                        format!("ignored={ignored} max_ahead={MAX_AHEAD_SLOTS} slot={}", s.slot_at(now).map_or("none".into(), |x| x.to_string())), Provenance::Gossiped))
                    .input(clock(s.slot_at(now))));
            }
            self.ignored = ignored;
        }
        let tick = decide(&s, now, holding, last);

        let view = InputSnapshot::new(
            "the schedule's candidates as gossiped (`cap/*/schedule/{id}`), this node's eligibility against `holders`, the ring's rendezvous winner",
            format!("candidates={} winner={}", candidates.len(), winner.as_ref().map_or("none".to_string(), |w| w.to_string())),
            Provenance::Gossiped,
        );
        let last_input = InputSnapshot::new("the last slot the fleet ran (the highest `sched/{id}/last/{node}` under this period and phase, as gossiped)",
            last.map_or("none".to_string(), |l| l.to_string()), Provenance::Gossiped);

        if tick.runs.is_empty() {
            // `already_ran` is every tick of a slot after its run: silence. The rest say something once.
            if tick.reason != "already_ran" && tick.reason != "not_due" {
                let outcome = tick.outcome;
                self.say(outcome, tick.reason, vec![view, last_input, clock(tick.slot)]);
            } else {
                self.was = Some(tick.reason);
            }
            return;
        }
        self.was = Some("ran");

        let parent = tick.missed.as_ref().and_then(|m| {
            let rec = DecisionRecord::new(MISSED_RULE, 1, "a gap between the last run and the slot due", m.outcome, m.reason)
                .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown).no_parent()
                .input(last_input.clone()).input(clock(tick.slot))
                .input(InputSnapshot::new("the schedule's missed-run policy", format!("{:?} first={} last={} skipped={} cap={MAX_CATCH_UP}", s.on_missed, m.first, m.last, m.skipped), Provenance::Local));
            let rec = if m.outcome == OutcomeKind::Action { rec.effect(format!("runs for slots {}..={} follow, each naming this record its parent", m.first, m.last)) } else { rec.no_effect() };
            self.record(rec)
        });
        let deadline_stream = format!("sched/{}/deadline", s.id);
        for run in tick.runs {
            let reason = match run.kind { RunKind::OnTime => "ran", RunKind::Late => "ran_late", RunKind::ForGap { .. } => "ran_for_gap" };
            let (key, slot, kind) = (run.operation_id(), run.slot, run.kind);
            // The run's deadline is one period, through the seam: work that overruns it is abandoned, so a holder's
            // lease is never held by work that has stopped making progress.
            let outcome = tokio::select! {
                r = work(run) => Some(r),
                _ = mycelium_core::sim_seam::sleep_ms(&deadline_stream, s.period_ms) => None,
            };
            let (kind_out, reason_out) = match &outcome {
                Some(Ok(())) => (OutcomeKind::Action, reason),
                Some(Err(RunFailure::Failed(_))) => (OutcomeKind::Deferral, "work_failed"),
                Some(Err(RunFailure::Unacknowledged(_))) => (OutcomeKind::Deferral, "unacknowledged"),
                None => (OutcomeKind::Deferral, "work_overran"),
            };
            let mut rec = DecisionRecord::new(RUN_RULE, 1, "schedule tick", kind_out, reason_out)
                .node(self.me.to_string(), 0).target(s.id.to_string()).view(ViewStatus::Unknown)
                .input(view.clone()).input(last_input.clone()).input(clock(tick.slot));
            rec = match parent { Some(p) => rec.parent(p), None => rec.no_parent() };
            match outcome {
                Some(Ok(())) => {
                    let mine = self.kv.get(&format!("{}{}", last_prefix(&s.id), self.me))
                        .and_then(|b| std::str::from_utf8(&b).ok()?.split_whitespace().next()?.parse::<u64>().ok());
                    let written = mine.map_or(slot, |m| m.max(slot));
                    let _ = self.kv.set(format!("{}{}", last_prefix(&s.id), self.me), format!("{written} {} {}", s.period_ms, s.phase_ms));
                    self.runs.fetch_add(1, Ordering::Relaxed);
                    self.record(rec.effect(format!("work done under {key}; this node's sched/{}/last = {written}", s.id)));
                }
                Some(Err(RunFailure::Failed(e))) => {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    let then = if kind == RunKind::OnTime { "retried at the next tick" } else { "not retried: the runs after it go on" };
                    self.record(rec.effect(format!("work failed under {key}: {e}; {then}")));
                }
                Some(Err(RunFailure::Unacknowledged(why))) => {
                    // Handed on and never answered: whether it ran is unknown, so the slot is not recorded. The work
                    // reports this only when its stream has closed, and the stream owns the loop's handle, so the
                    // loop ends next — dropping the runner withdraws the holding and the candidacy.
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    self.record(rec.effect(format!(
                        "the run under {key} was handed on and not acknowledged ({why}); the slot is not recorded, and the loop ends with its stream"
                    )));
                    return;
                }
                None => {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    let then = if self.options.stand_down_on_stall {
                        self.stood_down_until_ns = Some(mycelium_core::sim_seam::mono_now_ns()
                            .saturating_add(stand_down_ms(s.period_ms).saturating_mul(1_000_000)));
                        format!("this node stands down for {} ms, so another candidate holds", stand_down_ms(s.period_ms))
                    } else {
                        "this node withdraws its holding until its next tick".to_string()
                    };
                    self.record(rec.effect(format!("work under {key} overran its deadline of {} ms and was abandoned; {then}", s.period_ms)));
                    self.holding = None;
                    self.candidacy = None;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
