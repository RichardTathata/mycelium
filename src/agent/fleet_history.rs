//! **Fleet-wide history scopes** (post-360 row K2; plan §5, decided 2026-10-17).
//!
//! Row K counts a history-limited rule at one enforcement point ([`HistoryScope::EnforcementPoint`]), so an actor
//! that spreads its calls over N gateways can be admitted up to N times the limit. A rule may instead name one of two
//! fleet scopes, the policy choosing per rule:
//!
//! - **[`HistoryScope::FleetConsensus`] — exact.** A counter of `count` *seats*, each a consensus slot
//!   (`ae-seat/{counter}/{actor}/{i}`, the counter digested with the kind of point — the gateway's doors together,
//!   the provider check apart, as at one node — stored under `consensus/life/…` like every decision) whose decision is
//!   **leased for the rule's window plus `SEAT_LEASE_MARGIN_MS`**. A permit claims a free seat by an exclusive
//!   consensus write *before* the work is admitted ([`claim_seat`]), **outside** the per-actor history gate, so a
//!   claim waiting on a quorum holds up no other call; a seat is admitted only if its lease still covers a whole
//!   window from the moment of admission (`seat_admissible`, on the wall and the monotonic clock), so one seat cannot
//!   admit twice within a window. The holder releases the seat a window after admission ([`release_after_window`]);
//!   the lease ends it only if the holder crashes or shuts down first. A slot holds one live decision at a time —
//!   single-decree agreement, the property `distributed_lock` rests on — so at most `count` seats are live at once and
//!   the fleet admits at most `count` in any window, whichever gateways the calls reach. Every seat taken: denied. A
//!   claim that cannot be decided (no quorum, the electorate refused), or that took longer than the margin — which is
//!   derived from the claim's own bounds (every ballot timing out, the convergence poll, its local work), so only
//!   local work past its allowance or a wall-clock step reaches it: the rule decides `Indeterminate` — it fails closed. One consensus round per limited call (more when seats are contended).
//! - **[`HistoryScope::FleetTally`] — approximate.** Every node built with `gateway` and `tls` publishes a heartbeat
//!   (`sys/ae-tally/{node}/hb`, saying whether it enforces), from `start()`, and a last one marked departed at a
//!   graceful shutdown; an enforcement point that admits under a
//!   tally rule also publishes, per counter and actor, the times of its recent permits
//!   (`sys/ae-tally/{node}/c/{counter}/{actor}`, withdrawn once every time in it has aged out of its window). A limit
//!   reads the sum over this node and every enforcing peer ([`establish`]); a peer with no heartbeat, a stale one, or
//!   one from beyond the clock-drift bound is **unknown coverage**, and the tally is not established. Available
//!   without a quorum. **Detection, not prevention:** two gateways can each admit before either's tally reaches the
//!   other, so the fleet can exceed the limit until the tallies converge — every decision at this scope says so
//!   ([`FLEET_TALLY_CAVEAT`](super::action_evaluator::FLEET_TALLY_CAVEAT)).
//!
//! **What is visible fleet-wide, and to whom.** Every member holds these keys, as it holds every gossiped key; the
//! gateway's raw KV routes refuse both namespaces (`sys/` and `consensus/` are owned). A key carries two digests (the
//! first 128 bits of a SHA-256): of the policy's **declared counter id** with the kind of point, and of the actor's
//! principal. The counter id is opaque — the policy chooses it ([`Rule::scoped`](super::action_evaluator::Rule::scoped))
//! and it never encodes the rule's operation or resource, so a member learns *that* an actor counted against *some*
//! counter, and when — as the audit chain's reference record (`AeReference`, under `compliance`) already tells every
//! member the principal, the attempt and its time. A guessable counter id or principal is a pseudonym, not a secret:
//! a member that guesses it can confirm it. The values are an attempt id (a seat) or permit times (a tally).
//!
//! **Growth.** A tally entry is withdrawn once its times age out, so tallies hold only actors active within a window.
//! A seat is not collected: its decision record and the slot's sentinel are kept for good by design
//! (`docs/design/lock-lifecycle.md` §2 — a slot that has had a record must never fall back to the legacy reading,
//! where a committed value with no lease reads live for ever; §8 — the top record is what later decisions and the
//! acceptors' shrunk floor are ordered against). So each (counter, kind of point, actor) that ever claims holds, on
//! every member, at most `count` slots of three to five keys (sentinel, top record, its marker, the legacy committed
//! and lease keys) plus, on every voter, `count` shrunk acceptor entries. The reference evaluator bounds the actors by
//! the policy: an exact fleet limit must name its actor, never `*`. And every counter is bounded wherever it is used:
//! at most [`MAX_FLEET_SEATS`](super::action_evaluator::MAX_FLEET_SEATS) seats (each read, a slot digest and a KV read,
//! on every request the rule covers) and a window of at most
//! [`MAX_FLEET_WINDOW_MS`](super::action_evaluator::MAX_FLEET_WINDOW_MS), the history horizon — a counter outside them
//! is not established and nothing of it is read, claimed or published ([`establish`], [`claim_seat`],
//! [`publish_permit`]).
//!
//! **What is not claimed.** The seat's lease is measured on the proposer's wall clock and judged on each reader's,
//! so the exact bound holds to within the clocks' skew. A seat's release marker is not checked against its owner: any
//! mesh member can write one, as for a lock, because consensus here tolerates crashes, not lies (row A). Without a declared exclusive-default electorate the counter
//! is decided in the cluster scope, whose roster is the peers this node currently sees — a partitioned minority that
//! has declared the rest dead can decide alone; exactness across a partition needs the electorate
//! (`secure-single-domain` requires `consensus_require_electorate`). A claim that times out, or is refused for its
//! latency, may still hold its seat: one no call used, which can only admit fewer. Tally entries are unsigned soft
//! state under a self-owned `sys/` prefix: a member writing another's tally is counted by the `sys/` tripwire, not
//! refused.

use super::action_evaluator::{FleetCount, FleetCounter, FleetState, HistoryScope, SeatClaim};
use super::TaskCtx;
use bytes::Bytes;
use std::sync::Arc;

/// The tally namespace: `sys/ae-tally/{node}/hb` and `sys/ae-tally/{node}/c/{counter}/{actor}` — self-owned.
pub(crate) const TALLY_PREFIX: &str = "sys/ae-tally/";

/// The seats' consensus slot family: `ae-seat/{counter}/{actor}/{seat}` — a slot name; the keys are the decision
/// records under `consensus/life/{escaped slot}/…`.
#[cfg(feature = "consensus")]
pub(crate) const SEAT_SLOT_PREFIX: &str = "ae-seat/";

/// The first 128 bits of a SHA-256, hex — the pseudonym a counter or an actor goes by in a fleet-visible key.
fn digest16(s: &str) -> String {
    use sha2::Digest as _;
    let d: [u8; 32] = sha2::Sha256::digest(s.as_bytes()).into();
    super::action_evaluator::hex32(&d)[..32].to_string()
}

/// A counter's identity at one kind of enforcement point: the gateway's doors count together, the provider check
/// apart — as at one node (row K) — so a call the gateway admitted and the provider re-checks holds one seat at each,
/// never two of one.
fn counter_at(counter_key: &str, point: &str) -> String {
    digest16(&format!("{counter_key}\n{point}"))
}

/// The consensus slot of seat `seat` of `counter` at `point` for `actor`.
#[cfg(feature = "consensus")]
pub(crate) fn seat_slot(counter_key: &str, point: &str, actor: &str, seat: u32) -> String {
    format!("{SEAT_SLOT_PREFIX}{}/{}/{seat}", counter_at(counter_key, point), digest16(actor))
}

/// A tally publisher's heartbeat key.
pub(crate) fn heartbeat_key(node: &str) -> String {
    format!("{TALLY_PREFIX}{node}/hb")
}

/// A publisher's tally of `actor`'s permits under `counter` at `point`.
pub(crate) fn tally_key(node: &str, counter_key: &str, point: &str, actor: &str) -> String {
    format!("{TALLY_PREFIX}{node}/c/{}/{}", counter_at(counter_key, point), digest16(actor))
}

/// What the enforcement point establishes about each counter before the evaluator decides.
pub(crate) fn establish(
    ctx: &Arc<TaskCtx>, counters: &[FleetCounter], point: &str, actor: &str, now_ms: u64,
) -> Vec<FleetCount> {
    counters
        .iter()
        .map(|c| {
            // Bounded before anything is read (the Cedar adapter's review): a consensus counter's seats are read one by
            // one on every request the rule covers, under the actor's gate on an async worker, so a counter past the
            // seat cap or the history horizon is not established — whichever evaluator named it — and costs nothing.
            if let Err(out) = c.check_bounds() {
                return FleetCount::new(c.clone(), FleetState::NotEstablished(format!("the counter is outside the fleet bounds: {out}")));
            }
            let state = match c.scope {
                HistoryScope::FleetConsensus => seats_state(ctx, c, point, actor),
                HistoryScope::FleetTally => tally_state(ctx, c, point, actor, now_ms),
                other => FleetState::NotEstablished(format!("{} is not a fleet scope", other.label())),
            };
            FleetCount::new(c.clone(), state)
        })
        .collect()
}

// ── The consensus counter ────────────────────────────────────────────────────────────────────────

/// How long a seat's claim waits for each phase of a round, and how many ballots it tries.
/// Each phase of a seat claim's ballot — the promises, then the votes — waits at most this long.
#[cfg(feature = "consensus")]
const CLAIM_PHASE_TIMEOUT_MS: u64 = 2_000;
#[cfg(feature = "consensus")]
const CLAIM_PHASE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(CLAIM_PHASE_TIMEOUT_MS);
/// A claim's ballots for one seat, at most.
#[cfg(feature = "consensus")]
const CLAIM_MAX_BALLOTS: u32 = 4;
/// The most a claim sleeps before a ballot retry (the consensus jitter, set here so the bound below holds).
#[cfg(feature = "consensus")]
const CLAIM_RETRY_JITTER_MS: u64 = 50;
/// How long a committed claim polls for the converged decision record, and how often.
#[cfg(feature = "consensus")]
const CLAIM_CONVERGE_BUDGET_MS: u64 = 1_000;
#[cfg(feature = "consensus")]
const CLAIM_CONVERGE_STEP_MS: u64 = 25;
/// What the timeouts above do not bound: the local work of a claim — the acceptor's record and the decision written
/// and synced before they are answered or read back.
#[cfg(feature = "consensus")]
const CLAIM_LOCAL_WORK_MS: u64 = 2_000;

/// **How much longer than the window a seat is leased: the latency a claim is allowed.** The lease is stamped when the
/// claim's decision envelope is built, before its first ballot, and the call is admitted only if the lease still covers
/// a whole window from admission (`seat_admissible`) — so a claim is admitted exactly when it took no longer than this
/// margin. It is therefore derived from the claim's own bounds: every ballot waiting out both phases and its retry
/// pause, the convergence poll, and the local work no timeout bounds. Until 2026-10-19 it was 2 s, the length of **one**
/// phase timeout, so a claim that lost one ballot to its timeout and won the next was always refused *decided too late*
/// (the K2 flake). A claim slower than this, through local work past its allowance or the wall clock stepping forward,
/// is still refused (`Indeterminate`, failing closed).
///
/// The margin is liveness, not safety: a seat admits only while its lease covers a whole window, so two admissions on
/// one seat are at least a window apart whatever the margin is; a longer margin only keeps a used seat held longer
/// after its window, which can admit fewer, never more.
#[cfg(feature = "consensus")]
pub(crate) const SEAT_LEASE_MARGIN_MS: u64 = CLAIM_MAX_BALLOTS as u64 * (2 * CLAIM_PHASE_TIMEOUT_MS + CLAIM_RETRY_JITTER_MS)
    + CLAIM_CONVERGE_BUDGET_MS
    + CLAIM_CONVERGE_STEP_MS
    + CLAIM_LOCAL_WORK_MS;

// The margin is never shorter than one ballot that waits out both phases (the 2 s margin was one phase).
#[cfg(feature = "consensus")]
const _: () = assert!(SEAT_LEASE_MARGIN_MS > 2 * CLAIM_PHASE_TIMEOUT_MS + CLAIM_RETRY_JITTER_MS);

/// Whether a seat whose lease ends at `expires_at_ms` may admit a call at `now_ms` under a `window_ms` limit: only if
/// its lease still covers a whole window from now — otherwise the seat could reopen, and admit again, less than a
/// window after this call. Both times are this node's wall clock (the seat was proposed here).
#[cfg(feature = "consensus")]
pub(crate) fn seat_admissible(expires_at_ms: u64, now_ms: u64, window_ms: u64) -> bool {
    expires_at_ms.saturating_sub(now_ms) >= window_ms
}

/// Test hooks, per node: a lease margin in place of [`SEAT_LEASE_MARGIN_MS`] and a delay injected between a seat's
/// decision and its admission check.
#[cfg(all(test, feature = "consensus"))]
pub(crate) static TEST_CLAIM_HOOKS: std::sync::LazyLock<papaya::HashMap<String, (u64, u64)>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// Test record: per admitting node and seat slot (`{node}|{slot}`; tests share slot names), the monotonic instant of each admission that scheduled its release
/// ([`release_after_window`]) — the point the window is measured from.
#[cfg(all(test, feature = "consensus"))]
pub(crate) static TEST_ADMISSIONS: std::sync::LazyLock<papaya::HashMap<String, Vec<std::time::Instant>>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// Test hook: nodes whose seat claims panic — the case a serve loop must still answer (#642's review, N2).
#[cfg(all(test, feature = "consensus"))]
pub(crate) static TEST_PANIC_CLAIM: std::sync::LazyLock<papaya::HashMap<String, ()>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

#[cfg(feature = "consensus")]
fn lease_margin_ms(_ctx: &TaskCtx) -> u64 {
    #[cfg(test)]
    if let Some((margin, _)) = TEST_CLAIM_HOOKS.pin().get(&_ctx.node_id.to_string()) {
        return *margin;
    }
    SEAT_LEASE_MARGIN_MS
}

/// How many of `c`'s seats for `actor` this node sees held by a live decision.
#[cfg(feature = "consensus")]
fn seats_state(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str) -> FleetState {
    let held = (0..c.count).filter(|i| seat_live(ctx, &seat_slot(&c.key, point, actor, *i)).is_some()).count() as u32;
    FleetState::Seats { held }
}

#[cfg(not(feature = "consensus"))]
fn seats_state(_ctx: &Arc<TaskCtx>, _c: &FleetCounter, _point: &str, _actor: &str) -> FleetState {
    FleetState::NotEstablished("this build has no consensus (feature `consensus`), so no seat can be claimed".into())
}

/// The live decision this node holds for `slot` — its value — or `None`.
#[cfg(feature = "consensus")]
fn seat_live(ctx: &Arc<TaskCtx>, slot: &str) -> Option<Box<crate::consensus_life::Top>> {
    match crate::consensus_life::read_slot(&ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()) {
        crate::consensus_life::SlotView::Top(top) if !top.ended => Some(top),
        _ => None,
    }
}

/// The decision that holds a claimed seat, kept to release it ([`release_after_window`]); nothing without consensus.
#[cfg(feature = "consensus")]
pub(crate) type SeatDecision = Box<crate::consensus_life::Envelope>;
#[cfg(not(feature = "consensus"))]
pub(crate) type SeatDecision = ();

/// What a claim came to.
#[derive(Debug)]
#[cfg_attr(not(feature = "consensus"), allow(dead_code))]
pub(crate) enum Claim {
    /// This permit holds a seat, and the decision that holds it — released a window after the permit is admitted
    /// ([`release_after_window`]).
    Seat(SeatClaim, SeatDecision),
    /// Every seat is held by a live decision: the window already holds `count` permits fleet-wide.
    Full,
    /// No seat could be decided, and why — the rule decides `Indeterminate`.
    NotEstablished(String),
}

/// **Claim a seat for this permit** (row K2's check-then-act): try each seat this node does not see held, in order,
/// by an exclusive consensus write of `attempt_id` leased for the window plus the margin; the first decided for this
/// attempt — read back from the converged decision record, never assumed from `Committed` (the #164 discipline) — is
/// the claim, if its lease still covers a whole window from now. A seat lost to another attempt moves on to the next.
/// Every seat taken: [`Claim::Full`]. Called **without** the actor's history gate held.
#[cfg(feature = "consensus")]
pub(crate) async fn claim_seat(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, attempt_id: &str) -> Claim {
    use crate::consensus::{ConsensusConfig, ConsensusResult};
    let value = Bytes::from(attempt_id.as_bytes().to_vec());
    let lease = std::time::Duration::from_millis(c.window_ms.saturating_add(lease_margin_ms(ctx)));
    let cfg = ConsensusConfig {
        committed_lease_secs: Some(super::consensus_handle::lease_secs_ceil(lease)),
        phase1_timeout: CLAIM_PHASE_TIMEOUT,
        max_ballots: CLAIM_MAX_BALLOTS,
        ballot_retry_jitter_ms: CLAIM_RETRY_JITTER_MS,
        ..ConsensusConfig::default()
    };
    // Bounded before any proposal: a counter past the seat cap or the history horizon claims nothing.
    if let Err(out) = c.check_bounds() {
        return Claim::NotEstablished(format!("the counter is outside the fleet bounds: {out}"));
    }
    let consensus = super::ConsensusHandle { ctx: Arc::clone(ctx) };
    #[cfg(test)]
    {
        let panics = TEST_PANIC_CLAIM.pin().contains_key(&ctx.node_id.to_string());
        if panics {
            panic!("test hook: a seat claim that panics");
        }
    }
    for seat in 0..c.count {
        let slot = seat_slot(&c.key, point, actor, seat);
        if seat_live(ctx, &slot).is_some() {
            continue; // held, as this node sees it: a proposal would be superseded
        }
        // Before the proposal, so at or before the lease's stamp: the monotonic time a claim took is never
        // under-counted (review Q1 — a backward wall-clock step cannot make a slow claim look fast).
        let claim_started = mycelium_core::sim_seam::mono_instant();
        match consensus.exclusive_propose(&slot, value.clone(), cfg.clone()).await {
            ConsensusResult::Committed { .. } => {
                // The converged record decides whose the seat is, polled through the replay seam.
                const BUDGET_MS: u64 = CLAIM_CONVERGE_BUDGET_MS;
                const STEP_MS: u64 = CLAIM_CONVERGE_STEP_MS;
                let mut waited = 0;
                let top = loop {
                    match crate::consensus_life::read_slot(&ctx.kv_state, &slot, mycelium_core::sim_seam::wall_now_ms()) {
                        crate::consensus_life::SlotView::Top(top) if !top.ended => break top,
                        // Our own decision, already ended: its lease lapsed before it was read — the claim was too
                        // slow, and no amount of polling changes that (review L1; it read as "not observed").
                        crate::consensus_life::SlotView::Top(top) if top.value.as_ref() == Some(&value) => {
                            return Claim::NotEstablished(format!(
                                "seat {seat} was decided too late: its lease had ended by the time its decision was \
                                 read (the seat stays held until it ends)"
                            ));
                        }
                        _ => {}
                    }
                    if waited >= BUDGET_MS {
                        return Claim::NotEstablished(format!(
                            "seat {seat}'s decision was not observed within {BUDGET_MS} ms of its commit"
                        ));
                    }
                    mycelium_core::sim_seam::sleep_ms("ae/seat-converge", STEP_MS).await;
                    waited += STEP_MS;
                };
                if top.value.as_ref() != Some(&value) {
                    continue; // another attempt's decision holds it
                }
                #[cfg(test)]
                {
                    let hook = TEST_CLAIM_HOOKS.pin().get(&ctx.node_id.to_string()).copied();
                    if let Some((_, delay)) = hook {
                        mycelium_core::sim_seam::sleep_ms("ae/seat-test-delay", delay).await;
                    }
                }
                let expires = top.env.expires_at_ms().unwrap_or(0);
                let now = mycelium_core::sim_seam::wall_now_ms();
                if !seat_admissible(expires, now, c.window_ms) {
                    return Claim::NotEstablished(format!(
                        "seat {seat} was decided too late: its lease ends {} ms from now, less than the {} ms window, \
                         so it could admit again within one window (the seat stays held until it ends)",
                        expires.saturating_sub(now),
                        c.window_ms
                    ));
                }
                // The same rule on the monotonic clock: the lease's length less the time the claim has taken must
                // still cover a window. The wall clock alone would admit a slow claim after a backward step (Q1).
                let lease_ms = match top.env.term {
                    crate::consensus_life::Term::Lease { ms, .. } => ms,
                    crate::consensus_life::Term::Permanent => 0,
                };
                let took = u64::try_from(mycelium_core::sim_seam::mono_elapsed(&claim_started).as_millis()).unwrap_or(u64::MAX);
                if !seat_admissible(lease_ms, took, c.window_ms) {
                    return Claim::NotEstablished(format!(
                        "seat {seat} was decided too late: the claim took {took} ms of its {lease_ms} ms lease, leaving \
                         less than the {} ms window (the seat stays held until it ends)",
                        c.window_ms
                    ));
                }
                let env = Box::new(top.env.clone());
                return Claim::Seat(SeatClaim::new(c.key.clone(), slot, seat, top.env.token, expires), env);
            }
            ConsensusResult::Superseded { .. } => {} // another attempt took it
            ConsensusResult::Timeout { ballots_tried, .. } => {
                return Claim::NotEstablished(format!(
                    "no quorum decided seat {seat} ({ballots_tried} ballots); without a quorum the fleet count \
                     cannot be claimed"
                ));
            }
            other => {
                return Claim::NotEstablished(format!("seat {seat} could not be proposed: {other:?}"));
            }
        }
    }
    Claim::Full
}

#[cfg(not(feature = "consensus"))]
pub(crate) async fn claim_seat(
    _ctx: &Arc<TaskCtx>, _c: &FleetCounter, _point: &str, _actor: &str, _attempt_id: &str,
) -> Claim {
    Claim::NotEstablished("this build has no consensus (feature `consensus`), so no seat can be claimed".into())
}

/// **Release an admitted permit's seat once its window has passed** (review M1): a window after admission, measured
/// on this node's monotonic clock through the replay seam, the holder writes the decision's release marker
/// (`consensus::release_envelope`). The lease — the window plus [`SEAT_LEASE_MARGIN_MS`] — stays the fallback for a
/// holder that crashes or shuts down first; a shutdown before the window has passed releases nothing.
///
/// Why a release a window after admission keeps the bound. A node reads the seat free in one of two ways. The first is
/// this marker reaching it, and the marker is written at least a window after admission in real time: the timer runs on
/// this node's own monotonic clock, so the release path needs no clock agreement. The second is the lease lapsing on
/// that node's own wall clock (`read_slot`). The lease is the window plus [`SEAT_LEASE_MARGIN_MS`] from the stamp, so
/// that path absorbs a skew of up to about the margin (19 s) before it could read a seat free less than a window after
/// admission. So the guarantee is unchanged: at most `count` per window, to within the nodes' wall-clock skew. It is
/// never early: a permit whose dispatch
/// then fails still holds its seat for the whole window, which is all "a failed dispatch does not free its seat" ever
/// promised against the count. Without it a used seat stayed held for the window **plus the margin**, which at a short
/// window admitted a small fraction of the limit (about 5 % at one second).
#[cfg(feature = "consensus")]
pub(crate) fn release_after_window(ctx: &Arc<TaskCtx>, slot: String, env: SeatDecision, window_ms: u64) {
    #[cfg(test)]
    {
        // The admission point, per seat, for the tests' spacing oracle. Retry-safe: the update builds a new vector.
        let at = mycelium_core::sim_seam::mono_instant();
        let key = format!("{}|{slot}", ctx.node_id);
        TEST_ADMISSIONS.pin().update_or_insert_with(key, |v| v.iter().copied().chain([at]).collect(), || vec![at]);
    }
    let task_ctx = Arc::clone(ctx);
    let mut shutdown = ctx.shutdown_tx.subscribe();
    ctx.spawn_task(async move {
        // Shutdown first: a stopping node never releases, even when both are ready at once.
        tokio::select! { biased;
            // Not released at shutdown: the window may not have passed. The lease ends the seat.
            _ = async { let _ = shutdown.wait_for(|stopping| *stopping).await; } => {}
            _ = mycelium_core::sim_seam::sleep_ms("ae/seat-release", window_ms) => {
                crate::consensus::release_envelope_try(&task_ctx, &slot, &env);
            }
        }
    });
}

/// Without consensus no seat is ever claimed, so there is nothing to release.
#[cfg(not(feature = "consensus"))]
pub(crate) fn release_after_window(_ctx: &Arc<TaskCtx>, _slot: String, _env: SeatDecision, _window_ms: u64) {}

// ── The gossiped tally ───────────────────────────────────────────────────────────────────────────

/// A publisher's tally entry: the window it was kept for and its permit times (epoch ms, its decision clock).
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct TallyEntry {
    w: u64,
    t: Vec<u64>,
}

/// A node's heartbeat: the decision clock it was written at, whether it enforces (has an action evaluator), and
/// whether it has **departed** — shut down gracefully, its last word (#642's review, N3). A node that enforces nothing
/// admits nothing under a tally rule — its doors are not enforcement points — so it is not counted. A departed node
/// admits nothing more: it is known coverage, its remaining entries counted until they age out, never waited for. One
/// whose heartbeat is missing, stale (and not departed) or from the future is unknown.
#[derive(serde::Serialize, serde::Deserialize)]
struct Heartbeat {
    as_of: u64,
    #[serde(default)]
    enforces: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    departed: bool,
}

/// Whether a time read from a tally key is **forged**: past this node's clock plus the clock-drift bound, which no
/// member within the bound can have written (#656's second review, item 1). Tally keys are unsigned; such a time dates
/// nothing — it would never age out of any window.
fn from_the_future(t: u64, now_ms: u64, drift_ms: u64) -> bool {
    t > now_ms.saturating_add(drift_ms)
}

/// Permits in `entry` inside the window ending at `now_ms`: `Ok(0)` when there is none, `Err` when there is one this
/// node cannot read — never counted as zero (#642's review, N6) — or one holding a time from the future, which is read
/// as one it cannot read (#656's second review, item 1).
fn counted_in(entry: Option<Bytes>, now_ms: u64, window_ms: u64, drift_ms: u64) -> Result<u32, ()> {
    let from = now_ms.saturating_sub(window_ms);
    match entry {
        None => Ok(0),
        Some(b) => match serde_json::from_slice::<TallyEntry>(&b) {
            Ok(e) if !e.t.iter().any(|t| from_the_future(*t, now_ms, drift_ms)) => {
                Ok(e.t.iter().filter(|t| **t > from).count() as u32)
            }
            _ => Err(()),
        },
    }
}

/// The fleet's sum for `c` and `actor`, over this node and every enforcing peer — or why it is not established:
/// this node's own view of the fleet is not certain enough (the governors' confidence bound,
/// `control_max_staleness_ms` / `control_min_peers_heard`), or a peer's coverage is unknown — no heartbeat, one older
/// than `control_max_staleness_ms`, or one dated beyond `max_clock_drift_ms` in this node's future. A publisher that
/// is no longer a peer — departed, or declared dead by failure detection — still counts until its entries age out
/// (past the window, the staleness bound and the clock-drift bound), if it is a verified member; one that was removed
/// is not consulted.
fn tally_state(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, now_ms: u64) -> FleetState {
    tally_state_counted(ctx, c, point, actor, now_ms).0
}

/// [`tally_state`], with the number of keys it read — its cost, which grows with the nodes it knows of and never with
/// the entries any of them holds (#642's review, round 4, finding 1).
pub(crate) fn tally_state_counted(
    ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, now_ms: u64,
) -> (FleetState, usize) {
    let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
    let view = super::emergent::compute_view_confidence_at(ctx, mycelium_core::sim_seam::mono_instant());
    if let Err(why) = crate::control::assess(&view, &bound) {
        return (
            FleetState::NotEstablished(format!(
                "this node's view of the fleet is not established ({why:?}), so the tally's coverage is unknown"
            )),
            0,
        );
    }
    let reads = std::cell::Cell::new(0usize);
    let read = |key: &str| {
        reads.set(reads.get() + 1);
        mycelium_core::ops::kv_get(ctx, key)
    };
    let me = ctx.node_id.to_string();
    let mut unknown = Vec::new();
    let mut counted = 0;
    let mut add = |node: &str, unknown: &mut Vec<String>| {
        match counted_in(read(&tally_key(node, &c.key, point, actor)), now_ms, c.window_ms, ctx.config.max_clock_drift_ms) {
            Ok(n) => counted += n,
            Err(()) => unknown.push(format!("{node} (an entry this node cannot read, or one dated in the future)")),
        }
    };
    add(&me, &mut unknown);
    let mut publishers = vec![me.clone()];
    let mut stalest_ms = 0;
    let peers: std::collections::BTreeSet<String> = ctx.peers.pin().keys().map(|p| p.to_string()).collect();
    for peer in &peers {
        let Some(hb) = read(&heartbeat_key(peer)).and_then(|b| serde_json::from_slice::<Heartbeat>(&b).ok()) else {
            unknown.push(format!("{peer} (no heartbeat)"));
            continue;
        };
        if hb.as_of > now_ms.saturating_add(ctx.config.max_clock_drift_ms) {
            unknown.push(format!("{peer} (a heartbeat {} ms in the future)", hb.as_of - now_ms));
            continue;
        }
        let age = now_ms.saturating_sub(hb.as_of);
        if !hb.departed && age > bound.max_staleness_ms {
            unknown.push(format!("{peer} (stale: {age} ms)"));
            continue;
        }
        if !hb.enforces {
            continue; // not an enforcement point: it admits nothing under a tally rule
        }
        if !hb.departed {
            stalest_ms = stalest_ms.max(age);
        }
        add(peer, &mut unknown);
        publishers.push(peer.clone());
    }
    // An enforcing node that is no longer a peer — departed, or declared dead after a crash or a lost last word —
    // still counts while its permits can be inside this counter's window (#642's review, round 3, finding 6), and
    // only if it is a member this node can verify (round 4, finding 2): an identity record it trusts, and not removed.
    // A fabricated id has neither, so its "entries" count for nothing. Its heartbeat older than the window plus the
    // staleness bound plus the clock-drift bound means every entry it wrote has aged out: it is not read further.
    let horizon = c.window_ms.saturating_add(bound.max_staleness_ms).saturating_add(ctx.config.max_clock_drift_ms);
    let (nodes, listed) = tally_nodes(ctx);
    reads.set(reads.get() + listed);
    for node in nodes {
        if node == me || peers.contains(&node) {
            continue;
        }
        let Some(hb) = read(&heartbeat_key(&node))
            .and_then(|b| serde_json::from_slice::<Heartbeat>(&b).ok())
            .filter(|hb| !from_the_future(hb.as_of, now_ms, ctx.config.max_clock_drift_ms))
        else {
            // **Entries without a heartbeat** (#656's adversarial review, finding 1): its heartbeat collected before
            // they reached this node, not arrived yet (this node restarted without persistence and is catching up),
            // unreadable, or dated in the future (forged — it would never age; #656's second review, item 1). Nothing
            // dates the node, so its entry for this counter is dated by its own times, on the collector's bound
            // (`collect_residue`, the same when the writer's window is the counter's): while a time is inside the
            // window plus the staleness and drift bounds, or the entry cannot be read or holds a forged time, a
            // member's count is unknown — never zero — and the collector removes such an entry at once. Without
            // `[tls]` no member can be verified, so any such node is unknown; with it, a node nobody issued an
            // identity for is ignored.
            let pending = match read(&tally_key(&node, &c.key, point, actor)) {
                None => false,
                Some(b) => serde_json::from_slice::<TallyEntry>(&b)
                    .map(|e| e.t.iter().any(|t| *t > now_ms.saturating_sub(horizon)))
                    .unwrap_or(true),
            };
            if pending && (ctx.tls.get().is_none() || verified_member(ctx, &node)) {
                unknown.push(format!("{node} (not a peer: its entry is inside the window and it has no heartbeat)"));
            }
            continue;
        };
        if !hb.enforces || now_ms.saturating_sub(hb.as_of) > horizon {
            continue;
        }
        // Without a `[tls]` identity this node can verify no member, so a non-peer's absence is unknown coverage —
        // not established, never an under-count (#642's review round 5, finding 4). Until the horizon passes.
        if ctx.tls.get().is_none() {
            unknown.push(format!("{node} (not a peer, and without [tls] its membership cannot be verified)"));
            continue;
        }
        if !verified_member(ctx, &node) {
            continue;
        }
        add(&node, &mut unknown);
        publishers.push(node);
    }
    let reads = reads.get();
    if !unknown.is_empty() {
        return (
            FleetState::NotEstablished(format!(
                "the tally of {} is unknown (fresh within {} ms is required) — a count that cannot be read is not zero",
                unknown.join(", "),
                bound.max_staleness_ms
            )),
            reads,
        );
    }
    (FleetState::Tally { counted, publishers, stalest_ms }, reads)
}

/// Whether `node` is a member this node can verify: an identity record it trusts — the keys every verifier of a
/// node's signature uses (`CoreCtx::trusted_keys`: sealed identity records, certificate standing, revocations) — and
/// not removed. A node id nobody issued an identity for is not one.
pub(crate) fn verified_member(ctx: &TaskCtx, node: &str) -> bool {
    let Ok(id) = node.parse::<mycelium_core::NodeId>() else { return false };
    !ctx.removed.is_node_removed(&id) && !ctx.trusted_keys(&id).is_empty()
}

/// Every node this node holds tally keys for, from the store's scope index (`sys/ae-tally/` is a scoped prefix) —
/// one name per node, never a scan of the nodes' entries nor of another prefix's scopes — and how many index
/// entries it visited.
fn tally_nodes(ctx: &TaskCtx) -> (Vec<String>, usize) {
    let (scopes, visited) = mycelium_core::store::scopes_under_visiting(&ctx.kv_state, TALLY_PREFIX);
    let nodes = scopes.into_iter().filter_map(|outer| outer.strip_prefix(TALLY_PREFIX).map(str::to_string)).collect();
    (nodes, visited)
}

/// Publish a permit admitted under `c` into this node's tally of `actor`, and keep this node's heartbeat current.
/// Called under the actor's gate, so two permits of one actor at this point do not lose each other's update.
pub(crate) fn publish_permit(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, at_ms: u64) {
    if c.check_bounds().is_err() {
        return; // never established, so nothing was admitted under it (the seam refuses a permit relying on it)
    }
    let key = tally_key(&ctx.node_id.to_string(), &c.key, point, actor);
    // The read-modify-write and the withdrawal's read-then-delete take one lock (lock-order row 67, #642's review N7),
    // so an aged entry's withdrawal cannot delete a permit published between its read and its delete.
    let tally = ctx.ae_tally_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut entry = mycelium_core::ops::kv_get(ctx, &key)
        .and_then(|b| serde_json::from_slice::<TallyEntry>(&b).ok())
        .unwrap_or_default();
    let from = at_ms.saturating_sub(c.window_ms);
    // A forged time (past this node's clock plus the drift bound) is not kept: it would never age out.
    entry.t.retain(|t| *t > from && !from_the_future(*t, at_ms, ctx.config.max_clock_drift_ms));
    entry.t.push(at_ms);
    entry.t.sort_unstable();
    // The newest `count` are enough: once they are all in a window the sum has reached the limit.
    let keep = (c.count as usize).max(1);
    if entry.t.len() > keep {
        entry.t.drain(..entry.t.len() - keep);
    }
    entry.w = c.window_ms;
    if let Ok(bytes) = serde_json::to_vec(&entry) {
        ctx.soft_state_gate.write(|| {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(key.as_str()), Bytes::from(bytes));
        });
    }
    drop(tally);
    write_heartbeat(ctx);
}

fn write_heartbeat(ctx: &Arc<TaskCtx>) {
    let hb = Heartbeat { as_of: ctx.hlc.decision_now_ms(), enforces: ctx.action_evaluator.get().is_some(), departed: false };
    if let Ok(bytes) = serde_json::to_vec(&hb) {
        ctx.soft_state_gate.write(|| {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(heartbeat_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
        });
    }
}

/// Test hook: a pause, per node, between a withdrawal's read of an aged entry and its delete — where a concurrent
/// permit would be lost without the tally lock (#642's review, N7).
#[cfg(test)]
pub(crate) static TEST_WITHDRAW_PAUSE_MS: std::sync::LazyLock<papaya::HashMap<String, u64>> =
    std::sync::LazyLock::new(papaya::HashMap::new);

/// Whether every time in a tally entry has aged out of the window it was kept for (an unreadable one counts as aged:
/// nothing can be read from it). A forged time — past `now_ms` plus the drift bound — dates nothing, so it is not
/// waited for (#656's second review, item 1).
fn aged(value: &[u8], now_ms: u64, drift_ms: u64) -> bool {
    serde_json::from_slice::<TallyEntry>(value)
        .map(|e| {
            e.t.iter()
                .filter(|t| !from_the_future(**t, now_ms, drift_ms))
                .all(|t| *t <= now_ms.saturating_sub(stored_window(e.w)))
        })
        .unwrap_or(true)
}

/// A stored entry's window, read back from the store, clamped to the horizon (#653's review, finding 2): an entry
/// written by an earlier release with a longer window, or forged with `u64::MAX`, would otherwise never age out, and a
/// departed node's residue never be collected. No counter within the bounds writes a longer one.
fn stored_window(w: u64) -> u64 {
    w.min(super::action_evaluator::MAX_FLEET_WINDOW_MS)
}

/// Withdraw this node's tally entries whose every time has aged out of the window they were kept for (review
/// finding 4): a tally holds only actors active within a window. Under the tally lock, so a permit published
/// concurrently is either seen by the read or lands after the delete (N7).
pub(crate) fn withdraw_aged_entries(ctx: &Arc<TaskCtx>, now_ms: u64) -> usize {
    let prefix = format!("{TALLY_PREFIX}{}/c/", ctx.node_id);
    let drift = ctx.config.max_clock_drift_ms;
    let _tally = ctx.ae_tally_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut withdrawn = 0;
    for (key, value) in mycelium_core::ops::kv_scan_prefix(ctx, &prefix) {
        // **A forged time is scrubbed** (#656's second review, round 2, item 1): tally keys are unsigned, and a time
        // past this node's clock plus the drift bound written into its entry makes every reader read the entry as one
        // it cannot read. Rewritten here without it, under the tally lock, so it holds a counter up for one tick at
        // most; an entry with nothing honest left is withdrawn below.
        if let Ok(mut e) = serde_json::from_slice::<TallyEntry>(&value)
            && e.t.iter().any(|t| from_the_future(*t, now_ms, drift))
        {
            e.t.retain(|t| !from_the_future(*t, now_ms, drift));
            let fresh = e.t.iter().any(|t| *t > now_ms.saturating_sub(stored_window(e.w)));
            if fresh && let Ok(bytes) = serde_json::to_vec(&e) {
                ctx.soft_state_gate.write(|| {
                    let _ = mycelium_core::ops::kv_set(ctx, Arc::clone(&key), Bytes::from(bytes));
                });
                continue;
            }
        }
        if aged(&value, now_ms, drift) {
            #[cfg(test)]
            {
                let pause = TEST_WITHDRAW_PAUSE_MS.pin().get(&ctx.node_id.to_string()).copied();
                if let Some(ms) = pause {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
            }
            ctx.soft_state_gate.write(|| {
                let _ = mycelium_core::ops::kv_delete(ctx, Arc::clone(&key));
            });
            withdrawn += 1;
        }
    }
    withdrawn
}

/// What a collector reads a non-peer's tally key as ([`collect_residue`]).
enum Held {
    /// An entry with a time that could still count somewhere: kept.
    Live,
    /// An entry none of whose honest times can count — past its window plus both bounds, or forged: collected.
    Past,
    /// An entry nothing can be read from: a count that cannot be read, kept until its node's heartbeat goes.
    Unreadable,
    /// The node's heartbeat, with its `as_of` when it can be read and is not from the future.
    Heartbeat(Option<u64>),
}

/// **Collect residue** (#642's review, round 4, finding 1): the tally keys of a node that is not a peer, **each by its
/// own times** (#656 and its reviews). An entry goes once every time in it is past the window it was kept for plus the
/// staleness and clock-drift bounds — by then it counts in no reader's window (exactly so when the writer's window is
/// the reader's counter's, as `publish_permit` writes it) — or at once if its only times are forged (from the
/// future). The heartbeat goes **last**: once none of the node's entries here is live and its `as_of` is past the
/// longest window any counter may have (`MAX_FLEET_WINDOW_MS`) plus both bounds, since a collector cannot see the
/// windows of entries held only elsewhere; one that cannot be read or is from the future goes once none of its node's
/// entries here is live. An entry nothing can be read from is a count that cannot be read, not zero: it goes with the
/// heartbeat (or, with none here, once nothing of its node is live), so an older collector does not erase a newer
/// node's entries in a format it does not know. So a collection
/// that ran before all of a node's keys arrived leaves nothing behind for ever, and none removes a heartbeat while an
/// entry of its node can still count anywhere.
///
/// **Why a non-owner may delete here.** Each key is tombstoned *at the timestamp of the record this node holds*
/// (`kv_delete_held`). Under last-writer-wins that removes exactly that record or an older one, and loses to anything
/// written after it — so a node that returned and wrote a newer heartbeat or a newer permit, even one that has not
/// reached this collector yet, keeps it everywhere: the tombstone can never overtake it. At the returning owner
/// itself the tombstone is older than what it holds (it re-asserts its own keys locally before its connections come
/// up, [`reassert_own`]), so it is not applied and its `sys/` tripwire does not count it. The one case left: an owner
/// that returns holding the collected record and nothing newer — it re-asserts first, so this does not arise.
pub(crate) fn collect_residue(ctx: &Arc<TaskCtx>, now_ms: u64) -> usize {
    let me = ctx.node_id.to_string();
    let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
    let mut collected = 0;
    for node in tally_nodes(ctx).0 {
        if node == me || node.parse::<mycelium_core::NodeId>().is_ok_and(|id| ctx.peers.pin().contains_key(&id)) {
            continue;
        }
        let keys = mycelium_core::store::scan_scope(&ctx.kv_state, &format!("{TALLY_PREFIX}{node}"));
        // The stamps of the records read here: what the collection removes, and nothing written after them.
        let stamps: Vec<Option<u64>> = {
            let store = ctx.kv_state.store.pin();
            keys.iter().map(|(k, _)| store.get(k.as_ref()).map(|e| e.timestamp)).collect()
        };
        let drift = ctx.config.max_clock_drift_ms;
        let margin = bound.max_staleness_ms.saturating_add(drift);
        // What each key says, read once. An entry is dated by its own times, past its window plus both bounds; a time
        // from the future is forged and dates nothing, so an entry holding nothing else goes at once. An entry nothing
        // can be read from is a count that cannot be read, not zero — perhaps a newer node's, in a format this node
        // does not know — so it goes only with its node's heartbeat, once nothing of the node is live (#656's second
        // review, round 2, item 2). The heartbeat is judged last.
        let state: Vec<Held> = keys
            .iter()
            .map(|(k, v)| {
                if k.ends_with("/hb") {
                    return Held::Heartbeat(
                        serde_json::from_slice::<Heartbeat>(v)
                            .ok()
                            .map(|hb| hb.as_of)
                            .filter(|as_of| !from_the_future(*as_of, now_ms, drift)),
                    );
                }
                match serde_json::from_slice::<TallyEntry>(v) {
                    Err(_) => Held::Unreadable,
                    Ok(e) => {
                        let horizon = stored_window(e.w).saturating_add(margin);
                        let live = e
                            .t
                            .iter()
                            .any(|t| !from_the_future(*t, now_ms, drift) && *t > now_ms.saturating_sub(horizon));
                        if live { Held::Live } else { Held::Past }
                    }
                }
            })
            .collect();
        let live = state.iter().any(|s| matches!(s, Held::Live));
        // **The heartbeat goes last** (#656's second review, item 3): only once none of its node's entries here is
        // live, and its `as_of` is past the longest window *any* counter may have plus both bounds — an entry this node
        // does not hold may be kept elsewhere for a longer window than any it holds, and removing the heartbeat early
        // would leave the nodes holding it reading that entry without one. One that cannot be read, or is dated in the
        // future, dates nothing: it goes once none of its node's entries here is live. `!live` is implied for an
        // honest node whose heartbeat this collector has kept up with (its entry times are at most its last heartbeat's
        // `as_of`, and no window exceeds the cap); it is not implied when this collector holds a newer entry than its
        // heartbeat — a node silent past the cap returned and published, and its new heartbeat has not arrived — and
        // removing the old heartbeat then would read that live entry without one until it does.
        let heartbeat_goes = !live
            && state.iter().all(|s| match s {
                Held::Heartbeat(Some(as_of)) => {
                    now_ms.saturating_sub(*as_of) > super::action_evaluator::MAX_FLEET_WINDOW_MS.saturating_add(margin)
                }
                _ => true,
            });
        let mut any = false;
        for (((key, _), stamp), held) in keys.into_iter().zip(stamps).zip(state) {
            let collect = match held {
                Held::Past => true,
                Held::Live => false,
                // Unreadable entries go with the heartbeat — or, with none here, once nothing of the node is live.
                Held::Unreadable | Held::Heartbeat(_) => heartbeat_goes,
            };
            if let (true, Some(ts)) = (collect, stamp) {
                any |= mycelium_core::ops::kv_delete_held(ctx, key, ts);
            }
        }
        collected += usize::from(any);
    }
    collected
}

/// **Before this node's connections come up**: re-assert its own heartbeat and withdraw its own aged entries, on this
/// node only (anti-entropy and the heartbeat task carry them on). Its copies are then newer than any tombstone a peer
/// wrote at the stamp of its old records while it was away, so those tombstones are not applied here and its `sys/`
/// tripwire does not count them (#642's review, round 4, finding 1).
pub(crate) fn reassert_own(ctx: &TaskCtx) {
    let now = ctx.hlc.decision_now_ms();
    let hb = Heartbeat { as_of: now, enforces: ctx.action_evaluator.get().is_some(), departed: false };
    if let Ok(bytes) = serde_json::to_vec(&hb) {
        mycelium_core::ops::kv_write_local(ctx, Arc::from(heartbeat_key(&ctx.node_id.to_string()).as_str()), Some(Bytes::from(bytes)));
    }
    let prefix = format!("{TALLY_PREFIX}{}/c/", ctx.node_id);
    for (key, value) in mycelium_core::ops::kv_scan_prefix(ctx, &prefix) {
        if aged(&value, now, ctx.config.max_clock_drift_ms) {
            mycelium_core::ops::kv_write_local(ctx, key, None);
        }
    }
}

/// This node's **last word** at a graceful shutdown (N3): a heartbeat marked departed, written past the soft-state
/// gate by the shutdown sweep. Its entries are kept: peers go on counting its recent permits until they age out, and
/// do not wait for it.
///
/// **Nobody else deletes a node's tally keys while they can still count** (#642's review, round 3, finding 2): a
/// collector cannot tell a node's old record from one it wrote after returning that has not reached the collector
/// yet, so a plain delete by another node could erase a returned node's heartbeat or a permit it published, and would
/// trip its `sys/` tripwire. A node withdraws its own aged entries (every heartbeat tick, from its next start). What a
/// node that never returns leaves behind is collected by any node once its heartbeat is past the horizon
/// ([`collect_residue`]): each key tombstoned at the stamp of the record held, which loses to anything the owner
/// wrote later.
///
/// **A forged `departed`** — another member writing this node's heartbeat — makes peers skip the staleness check
/// for this node until its next real heartbeat overwrites the forgery (one period at most): for that long a node
/// that is in fact live but has gone quiet is not waited for, though its published entries are still counted. That
/// is no more than a forged `enforces: false` does, which stops peers counting the node at all. Both are detection
/// only — the `sys/` tripwire counts the foreign write at the owner — as every unsigned tally key is.
pub(crate) fn depart(ctx: &TaskCtx) {
    let hb = Heartbeat { as_of: ctx.hlc.decision_now_ms(), enforces: ctx.action_evaluator.get().is_some(), departed: true };
    if let Ok(bytes) = serde_json::to_vec(&hb) {
        let _ = mycelium_core::ops::kv_set(ctx, Arc::from(heartbeat_key(&ctx.node_id.to_string()).as_str()), Bytes::from(bytes));
    }
}

/// **The heartbeat**, from `start()` on every node built with `gateway` and `tls` (review finding 3): re-asserted every
/// third of `control_max_staleness_ms` (between 250 ms and 10 s) through the timer seam until shutdown, so a node
/// with nothing new to report — or one just restarted — is read as current rather than stale; each tick also
/// withdraws aged tally entries. The first write waits for a peer (or five seconds), as the caller-context marker
/// does: a write fanned out to a bootstrap peer not yet listening puts the writer into backoff. Writes go through
/// the soft-state gate, so none overtakes the shutdown sweep.
pub(crate) fn start_heartbeat(ctx: &Arc<TaskCtx>) {
    if ctx.ae_tally_heartbeat.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let period = (ctx.config.control_max_staleness_ms / 3).clamp(250, 10_000);
    let task_ctx = Arc::clone(ctx);
    let mut srx = ctx.shutdown_tx.subscribe();
    ctx.spawn_task(async move {
        let mut waited = 0u64;
        while task_ctx.peers.pin().is_empty() && waited < 5_000 {
            tokio::select! { biased;
                _ = srx.wait_for(|v| *v) => return,
                _ = mycelium_core::sim_seam::sleep_ms("ae/tally-heartbeat-grace", 100) => {}
            }
            waited += 100;
        }
        let mut ticker = mycelium_core::sim_seam::interval_ms(
            "ae/tally-heartbeat", period, tokio::time::MissedTickBehavior::Skip,
        );
        loop {
            tokio::select! { biased;
                _ = srx.wait_for(|v| *v) => return,
                _ = ticker.tick() => {
                    write_heartbeat(&task_ctx);
                    let now = task_ctx.hlc.decision_now_ms();
                    // Adopter-surface Y5: the exposure summary rides the same tick and the same staleness bound.
                    crate::agent::exposure::publish_summary(&task_ctx, now);
                    crate::agent::exposure::collect_summaries(&task_ctx, now);
                    withdraw_aged_entries(&task_ctx, now);
                    collect_residue(&task_ctx, now);
                }
            }
        }
    });
}

/// The live tests start consensus listeners, so they need the `consensus` feature (on by default).
#[cfg(all(test, feature = "consensus"))]
#[path = "fleet_history_tests.rs"]
mod tests;
