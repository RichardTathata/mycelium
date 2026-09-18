//! **Replay scenario C — interacting governors** (item 6 PR 6; `docs/design/adaptive-stability.md`
//! §5, and D19: *the combined-feedback harness is replay stage 6, built once, reusing the governors'
//! pure decision functions, not a bespoke rig*).
//!
//! Three loops share inputs: **opacity** reads load; **tuning** changes throughput, which changes
//! load; **membership** changes how many nodes share the load, which changes this node's. Each is
//! stable alone by construction — hysteresis, spacing, settling, a cooldown — and each has unit
//! tests that say so. The claim none of them can make is the one this scenario judges:
//!
//! > **With the loop-breakers on, the three loops settle together under every schedule swept.
//! > With them off, at least one schedule does not settle.**
//!
//! # Why a schedule sweep, and what is real in it
//!
//! Same shape as scenario B (`mandate::scenario_b`): schedules are generated, not hand-picked, and
//! the invariants are asserted after **every step of every schedule**. The decisions are the
//! shipped ones — `TuningGovernor::gate_at`/`acted_at`, `opacity_state_for` →
//! `opacity_transition` → `spaced_transition`, `membership_governor::{decide, classify}` under
//! `control::{decide, spacing_allows, may_propose}` — with their real hysteresis, spacing and
//! settling. What is *modelled* is the plant that couples them: a channel that fills with this
//! node's share of the fleet's inbound, drains in proportion to the writer depth, and takes half
//! as much while the boundary is opaque; and an advisor that recommends a writer depth from the
//! group size, as the real `ClusterTuner` derives it from `N`. The plant is small on purpose: the
//! scenario is about the governors' interaction, and a plant rich enough to be argued about would
//! move the argument to the plant.
//!
//! # The invariants, as numbers
//!
//! The ADR's stability objectives are stated here with fixed numbers, independent of which
//! breakers are on, so that the witness (breakers off) can violate them:
//! two boundary **releases** never closer than [`MIN_RELEASE_GAP_MS`] — the shed is never held
//! (§2), so a release followed at once by a re-shed is the release's cost, and what the release
//! spacing bounds is how often that cost is paid; two knob changes never closer than
//! [`MIN_KNOB_GAP_MS`]; once the last disturbance is [`SETTLE_TICKS`] ticks past, no governor acts
//! again (*at rest*); and the decisive rule — under an enforcing profile with a stale view, no
//! routine scale-down is applied.
//!
//! # What this does not show
//!
//! The ADR's sharper sentence — *several loops can oscillate together while each is stable alone*
//! — is not demonstrated. The witness removes every loop's breaker at once; a witness that kept
//! each loop's own breaker and still oscillated would need a plant tuned to produce that coupling,
//! and this record does not claim to have one. Nor is any real node involved: the loops' wiring
//! (tickers, KV, signals) is outside this sweep, which is exactly why it is deterministic.

use crate::agent::membership_governor::{classify, decide as membership_decide, join_probability, leave_probability, MembershipAction};
use crate::agent::opacity::{opacity_state_for, opacity_transition, spaced_transition, OpacityTransition, Spaced};
use crate::agent::tuning_governor::{HotParam, TuningGovernor};
use crate::control::{self, ActionClass, ActionId, ConfidenceBound, ControlSpec, Profile, SettleState};
use crate::ViewConfidence;

/// One governor pass per tick.
pub const TICK_MS: u64 = 100;
/// The opacity threshold the plant runs at (inside the library's `[0.4, 0.95]` clamp).
pub const THRESHOLD: f32 = 0.75;
/// The governed group's band.
pub const GROUP_MIN: usize = 2;
/// The governed group's band.
pub const GROUP_MAX: usize = 4;
/// Eligible non-members the join probability spreads a deficit over.
const ELIGIBLE_NON_MEMBERS: usize = 3;

/// Stability objective: two boundary releases are never closer than this. (The shed is never held,
/// so it is not spaced — the first sweep stated this on *any* transition and found a release
/// re-shed 100 ms later under the shipped breakers, which is the rule working, not a flap.)
pub const MIN_RELEASE_GAP_MS: u64 = 300;
/// Stability objective: two applied knob changes are never closer than this.
pub const MIN_KNOB_GAP_MS: u64 = 200;
/// Stability objective: this many ticks after the last disturbance, the loops are at rest.
pub const SETTLE_TICKS: usize = 15;

/// The loop-breakers under test. [`Breakers::on`] is production's shape; [`Breakers::off`] is
/// the witness — the same loops with nothing breaking them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Breakers {
    pub tuning_spacing_ms: u64,
    pub tuning_settle_ms: u64,
    pub release_spacing_ms: u64,
    pub hysteresis: f32,
    pub membership_spacing_ms: u64,
}

impl Breakers {
    /// What the shipped defaults amount to at this tick: two ticks of tuning spacing and settling
    /// (`start_cluster_tuner`), a 1 s release spacing (`OpacityHint`), the hysteresis default,
    /// and a three-tick membership cooldown.
    pub fn on() -> Self {
        Self {
            tuning_spacing_ms: 2 * TICK_MS,
            tuning_settle_ms: 2 * TICK_MS,
            release_spacing_ms: 1_000,
            hysteresis: 0.20,
            membership_spacing_ms: 3 * TICK_MS,
        }
    }

    /// No spacing, no settling, no hysteresis, no cooldown.
    pub fn off() -> Self {
        Self { tuning_spacing_ms: 0, tuning_settle_ms: 0, release_spacing_ms: 0, hysteresis: 0.0, membership_spacing_ms: 0 }
    }
}

/// One step a schedule can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// One governor pass; the membership self-election roll, in permille.
    Tick { roll_permille: u32 },
    /// The fleet's total inbound becomes `permille`/1000 of a full channel per tick.
    Inbound(u32),
    /// Other nodes join (+) or leave (−) the group.
    Churn(i8),
    /// Whether the fleet view is stale from here on — a partition, seen from this node.
    Partition(bool),
}

impl Step {
    fn is_disturbance(self) -> bool {
        !matches!(self, Step::Tick { .. })
    }
}

/// What the governors did on one tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Actions {
    pub boundary: Option<OpacityTransition>,
    pub knob: Option<u64>,
    pub membership: Option<(MembershipAction, ActionClass)>,
    /// Proposals a breaker or the predicate held this tick.
    pub held: u8,
}

/// What the sweep observed after one step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub step: Step,
    pub now_ms: u64,
    pub fill: f32,
    pub opaque: bool,
    pub writer_depth: u64,
    pub members: usize,
    pub am_member: bool,
    pub stale: bool,
    pub actions: Actions,
}

/// The advisor: a size term with the real formula's shape (`GossipConfig::auto_writer_channel_depth`
/// is `4N` with a 1024 floor, scaled here so a group of this size moves it — one level per
/// member) plus a **load term** — a fuller channel asks for a deeper writer, in quarters. The load
/// term is what makes tuning and opacity share an input (the ADR's *inbound above what the writer
/// can drain* is the tuning governor's deficit), and its quantization is what an unspaced knob
/// chatters on when the fill hovers at a boundary.
fn advisor_recommendation(members: usize, fill: f32) -> u64 {
    (members.max(1) as u64) * 1024 + ((fill * 4.0) as u64).min(3) * 256
}

/// Run one schedule and return what was observed after every step.
pub fn run(schedule: &[Step], breakers: Breakers, profile: Profile) -> Vec<Observation> {
    let tuning = TuningGovernor::default();
    tuning.set_control_profile(profile);
    tuning.set_control_timing(breakers.tuning_spacing_ms, breakers.tuning_settle_ms);
    let bound = ConfidenceBound { max_staleness_ms: 5_000, min_peers_heard: 1 };
    let opacity_spec = ControlSpec {
        governor: "opacity".into(),
        actuator: "boundary/c".into(),
        spacing_ms: breakers.release_spacing_ms,
        settle_timeout_ms: 0,
        bound: bound.clone(),
        profile,
    };
    let membership_spec = ControlSpec {
        governor: "membership".into(),
        actuator: "group-membership".into(),
        spacing_ms: breakers.membership_spacing_ms,
        settle_timeout_ms: 2 * TICK_MS,
        bound,
        profile,
    };

    // The plant.
    let mut fill: f32 = 0.2;
    let mut prev_fill: f32 = 0.2;
    let mut writer_depth: u64 = 1024;
    let mut members: usize = 3;
    let mut am_member = true;
    let mut opaque = false;
    let mut inbound: f32 = 0.10;
    let mut stale = false;
    let mut now_ms: u64 = 0;

    // The governors' per-actuator state, exactly as the loops keep it.
    let mut last_boundary_ms: Option<u64> = None;
    let mut last_membership_ms: Option<u64> = None;
    let mut settle = SettleState::Idle;
    let mut pending_join: Option<bool> = None;
    let mut seq: u64 = 0;

    let mut out = Vec::with_capacity(schedule.len());
    for &step in schedule {
        let mut actions = Actions::default();
        match step {
            Step::Inbound(permille) => inbound = permille as f32 / 1000.0,
            Step::Churn(delta) => {
                let floor = if am_member { 1 } else { 0 };
                members = (members as i64 + delta as i64).max(floor) as usize;
            }
            Step::Partition(s) => stale = s,
            Step::Tick { roll_permille } => {
                now_ms += TICK_MS;

                // ── Tuning: the advisor recommends from the group size and the load; the gate decides.
                let rec = advisor_recommendation(members, fill);
                match tuning.gate_at(HotParam::WriterDepth, rec, writer_depth, now_ms) {
                    Some(v) if v != writer_depth => {
                        writer_depth = v;
                        tuning.acted_at(HotParam::WriterDepth, v, now_ms);
                        actions.knob = Some(v);
                    }
                    Some(_) => {}
                    None => {
                        if rec != writer_depth {
                            actions.held += 1;
                        }
                    }
                }

                // ── Opacity: the boundary decides from this node's own fill.
                let state = opacity_state_for(opaque, fill, prev_fill, THRESHOLD);
                let proposed = opacity_transition(&state, true, breakers.hysteresis);
                let Spaced { transition, spaced } = spaced_transition(proposed, &opacity_spec, last_boundary_ms, now_ms, profile);
                if spaced {
                    actions.held += 1;
                }
                match transition {
                    OpacityTransition::GoOpaque => {
                        opaque = true;
                        last_boundary_ms = Some(now_ms);
                        actions.boundary = Some(transition);
                    }
                    OpacityTransition::GoTransparent => {
                        opaque = false;
                        last_boundary_ms = Some(now_ms);
                        actions.boundary = Some(transition);
                    }
                    OpacityTransition::Hold => {}
                }

                // ── Membership: the self-election, through the contract.
                let view = ViewConfidence {
                    observer: "scenario-c".into(),
                    peers_known: members,
                    peers_heard: if stale { 0 } else { members },
                    max_staleness_ms: if stale { 60_000 } else { 0 },
                    self_degraded: false,
                };
                // Reconcile: the last action is observed once the membership reflects it.
                if let (SettleState::Pending { .. }, Some(join)) = (&settle, pending_join)
                    && join == am_member
                {
                    settle = SettleState::Idle;
                    pending_join = None;
                }
                let join_p = join_probability(members, GROUP_MIN, ELIGIBLE_NON_MEMBERS, fill as f64);
                let leave_p = leave_probability(members, GROUP_MAX, members, fill as f64);
                let action = membership_decide(am_member, false, join_p, leave_p, roll_permille as f64 / 1000.0);
                if let Some(class) = classify(&action, members, false) {
                    let allowed = control::spacing_allows(&membership_spec, last_membership_ms, now_ms)
                        && control::may_propose(&settle, &membership_spec, now_ms).is_ok()
                        && control::decide(class, &view, &membership_spec.bound, profile).proceeds();
                    if allowed {
                        match action {
                            MembershipAction::Join => {
                                members += 1;
                                am_member = true;
                            }
                            MembershipAction::Leave => {
                                members = members.saturating_sub(1);
                                am_member = false;
                            }
                            MembershipAction::Hold => {}
                        }
                        seq += 1;
                        last_membership_ms = Some(now_ms);
                        settle = SettleState::Pending {
                            id: ActionId { governor: "membership".into(), actuator: "group-membership".into(), seq },
                            since_ms: now_ms,
                        };
                        pending_join = Some(action == MembershipAction::Join);
                        actions.membership = Some((action, class));
                    } else {
                        actions.held += 1;
                    }
                }

                // ── The plant: this node's share of the inbound, halved while opaque, minus what
                //    the writer drains.
                let share = inbound / members.max(1) as f32 * if opaque { 0.5 } else { 1.0 };
                let drain = writer_depth as f32 / 8192.0;
                prev_fill = fill;
                fill = (fill + share - drain).clamp(0.0, 1.0);
            }
        }
        out.push(Observation { step, now_ms, fill, opaque, writer_depth, members, am_member, stale, actions });
    }
    out
}

/// A stability objective broken, with the numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// Two boundary releases closer than [`MIN_RELEASE_GAP_MS`].
    ReleaseFlap { gap_ms: u64 },
    /// Two knob changes closer than [`MIN_KNOB_GAP_MS`].
    KnobChatter { gap_ms: u64 },
    /// A governor acted after the loops should have been at rest.
    NotAtRest { at_ms: u64, actions: Actions },
    /// The decisive rule: a routine scale-down applied on a stale view under an enforcing profile.
    HeldClassApplied { class: ActionClass },
}

/// Every violation in one schedule's observations, each with its index. All of them, not the
/// first: the witness needs to see which objectives the missing breakers broke, and a first-only
/// report lets one kind mask another.
pub fn violations(observations: &[Observation], profile: Profile) -> Vec<(usize, Violation)> {
    let mut found = Vec::new();
    let mut last_release: Option<u64> = None;
    let mut last_knob: Option<u64> = None;
    // The loops must be at rest from `SETTLE_TICKS` ticks after the last disturbance.
    let last_disturbance = observations.iter().rposition(|o| o.step.is_disturbance());
    let rest_from = last_disturbance.map(|i| i + SETTLE_TICKS + 1);

    for (i, o) in observations.iter().enumerate() {
        if o.actions.boundary == Some(OpacityTransition::GoTransparent) {
            if let Some(last) = last_release {
                let gap_ms = o.now_ms - last;
                if gap_ms < MIN_RELEASE_GAP_MS {
                    found.push((i, Violation::ReleaseFlap { gap_ms }));
                }
            }
            last_release = Some(o.now_ms);
        }
        if o.actions.knob.is_some() {
            if let Some(last) = last_knob {
                let gap_ms = o.now_ms - last;
                if gap_ms < MIN_KNOB_GAP_MS {
                    found.push((i, Violation::KnobChatter { gap_ms }));
                }
            }
            last_knob = Some(o.now_ms);
        }
        if let Some((_, class)) = o.actions.membership
            && o.stale
            && profile != Profile::Legacy
            && profile != Profile::Observe
            && control::holds_on_uncertainty(class)
        {
            found.push((i, Violation::HeldClassApplied { class }));
        }
        if let Some(from) = rest_from
            && i >= from
            && (o.actions.boundary.is_some() || o.actions.knob.is_some() || o.actions.membership.is_some())
        {
            found.push((i, Violation::NotAtRest { at_ms: o.now_ms, actions: o.actions }));
        }
    }
    found
}

/// The first violation in one schedule's observations, with its index.
pub fn first_violation(observations: &[Observation], profile: Profile) -> Option<(usize, Violation)> {
    violations(observations, profile).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator for the membership rolls — the seed is part of the
    /// schedule, so the sweep is reproducible by construction.
    fn rolls(seed: u64, n: usize) -> Vec<u32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) % 1000) as u32
            })
            .collect()
    }

    const MAIN_TICKS: usize = 30;
    const TAIL_TICKS: usize = SETTLE_TICKS + 10;

    /// Every combination of: an inbound pattern (steady low · a surge that ends · a square wave ·
    /// a steady hover — 350‰, which for a lone member sits between what the writer drains with the
    /// boundary open and closed, the band an unhysteretic boundary flaps in), a churn pattern
    /// (none · +2 members · −2 members), a partition (none · stale mid-run), and a roll seed.
    /// Each schedule ends with the disturbance removed and a quiet tail, which is where *at rest*
    /// is judged.
    fn schedules() -> Vec<Vec<Step>> {
        let inbound_patterns: [&dyn Fn(usize) -> Option<Step>; 4] = [
            &|_| None,
            &|t| (t == 0).then_some(Step::Inbound(350)),
            &|t| match t {
                3 => Some(Step::Inbound(700)),
                18 => Some(Step::Inbound(100)),
                _ => None,
            },
            &|t| match t % 6 {
                0 => Some(Step::Inbound(650)),
                3 => Some(Step::Inbound(150)),
                _ => None,
            },
        ];
        let churn_patterns: [&dyn Fn(usize) -> Option<Step>; 3] = [
            &|_| None,
            &|t| (t == 5).then_some(Step::Churn(2)),
            &|t| (t == 8).then_some(Step::Churn(-2)),
        ];
        let partitions: [&dyn Fn(usize) -> Option<Step>; 2] = [
            &|_| None,
            &|t| match t {
                6 => Some(Step::Partition(true)),
                14 => Some(Step::Partition(false)),
                _ => None,
            },
        ];
        let mut all = Vec::new();
        for inbound in &inbound_patterns {
            for churn in &churn_patterns {
                for partition in &partitions {
                    for seed in [1u64, 2] {
                        let r = rolls(seed, MAIN_TICKS + TAIL_TICKS);
                        let (main, tail) = r.split_at(MAIN_TICKS);
                        let mut s = Vec::new();
                        for (t, &roll) in main.iter().enumerate() {
                            if let Some(step) = inbound(t) { s.push(step); }
                            if let Some(step) = churn(t) { s.push(step); }
                            if let Some(step) = partition(t) { s.push(step); }
                            s.push(Step::Tick { roll_permille: roll });
                        }
                        // The disturbance ends: low inbound, the view fresh; then the quiet tail.
                        s.push(Step::Inbound(100));
                        s.push(Step::Partition(false));
                        s.extend(tail.iter().map(|&roll| Step::Tick { roll_permille: roll }));
                        all.push(s);
                    }
                }
            }
        }
        all
    }

    /// inbound patterns × churn patterns × partitions × seeds.
    const SWEEP_SIZE: usize = 4 * 3 * 2 * 2;

    #[test]
    fn the_sweep_asserts_its_own_size() {
        assert_eq!(schedules().len(), SWEEP_SIZE);
    }

    /// Under the enforcing profiles — where the breakers act (ADR §7: `Legacy` does not consult
    /// them, `Observe` only counts) — every schedule settles.
    #[test]
    fn with_the_breakers_on_the_three_loops_settle_under_every_schedule() {
        let (mut boundaries, mut knobs, mut memberships) = (0usize, 0usize, 0usize);
        for profile in [Profile::EnforceLocal, Profile::EnforceAllocated] {
            for (n, schedule) in schedules().iter().enumerate() {
                let obs = run(schedule, Breakers::on(), profile);
                if let Some((i, v)) = first_violation(&obs, profile) {
                    panic!("schedule {n} under {profile:?} violated at step {i} ({:?}): {v:?}", obs[i].step);
                }
                boundaries += obs.iter().filter(|o| o.actions.boundary.is_some()).count();
                knobs += obs.iter().filter(|o| o.actions.knob.is_some()).count();
                memberships += obs.iter().filter(|o| o.actions.membership.is_some()).count();
            }
        }
        // Non-vacuity: the sweep exercised every loop, so "settled" is not "never acted".
        assert!(boundaries > 0, "no boundary ever moved");
        assert!(knobs > 0, "no knob ever changed");
        assert!(memberships > 0, "no membership action ever happened");
    }

    /// The ladder's own witness (ADR §7): `Legacy` does not consult the contract, so the shipped
    /// breakers do nothing there — with hysteresis alone the same schedules flap and chatter; and
    /// `Observe` behaves like `Legacy` in the plant while **counting** every hold it did not make.
    #[test]
    fn under_legacy_the_breakers_are_not_consulted_and_under_observe_they_only_count() {
        let legacy_violations: usize =
            schedules().iter().map(|s| violations(&run(s, Breakers::on(), Profile::Legacy), Profile::Legacy).len()).sum();
        assert!(legacy_violations > 0, "Legacy with the breakers configured must behave as if they were off");
        let (mut observe_violations, mut observe_holds) = (0usize, 0usize);
        for s in schedules() {
            let obs = run(&s, Breakers::on(), Profile::Observe);
            observe_violations += violations(&obs, Profile::Observe).len();
            observe_holds += obs.iter().map(|o| o.actions.held as usize).sum::<usize>();
        }
        assert!(observe_violations > 0, "Observe holds nothing, so it oscillates like Legacy");
        assert!(observe_holds > 0, "but it counts what it would have held");
    }

    /// The witness: the same loops, the same schedules, nothing breaking them — at least one
    /// schedule must violate a stability objective, or the objectives are vacuous.
    #[test]
    fn without_the_breakers_at_least_one_schedule_does_not_settle() {
        let mut kinds = std::collections::BTreeMap::<&'static str, usize>::new();
        for schedule in schedules() {
            for (_, v) in violations(&run(&schedule, Breakers::off(), Profile::EnforceLocal), Profile::EnforceLocal) {
                let kind = match v {
                    Violation::ReleaseFlap { .. } => "release flap",
                    Violation::KnobChatter { .. } => "knob chatter",
                    Violation::NotAtRest { .. } => "not at rest",
                    Violation::HeldClassApplied { .. } => "held class applied",
                };
                *kinds.entry(kind).or_default() += 1;
            }
        }
        let total: usize = kinds.values().sum();
        assert!(total > 0, "no schedule oscillated without the breakers — the objectives are vacuous");
        // Both spacing objectives must be the ones doing work, not one of them alone.
        assert!(kinds.contains_key("release flap"), "the boundary never flapped: {kinds:?}");
        assert!(kinds.contains_key("knob chatter"), "the knob never chattered: {kinds:?}");
    }

    /// The decisive rule, seen in the sweep: under `EnforceLocal` a stale view holds routine
    /// scale-down (asserted for every schedule above); under `Legacy` the same schedules do apply
    /// one — so the hold is doing something, not nothing.
    #[test]
    fn a_stale_view_holds_routine_scale_down_only_under_an_enforcing_profile() {
        let mut applied_while_stale_under_legacy = 0usize;
        let mut held_while_stale_under_enforce = 0usize;
        for schedule in schedules() {
            let legacy = run(&schedule, Breakers::on(), Profile::Legacy);
            applied_while_stale_under_legacy += legacy
                .iter()
                .filter(|o| o.stale && matches!(o.actions.membership, Some((_, ActionClass::RoutineScaleDown))))
                .count();
            let enforce = run(&schedule, Breakers::on(), Profile::EnforceLocal);
            assert!(
                !enforce.iter().any(|o| o.stale && matches!(o.actions.membership, Some((_, ActionClass::RoutineScaleDown)))),
                "a routine scale-down was applied on a stale view under EnforceLocal"
            );
            held_while_stale_under_enforce += enforce.iter().filter(|o| o.stale && o.actions.held > 0).count();
        }
        assert!(applied_while_stale_under_legacy > 0, "no schedule ever scaled down while stale under Legacy — the hold would be vacuous");
        assert!(held_while_stale_under_enforce > 0, "nothing was held while stale under EnforceLocal");
    }
}
