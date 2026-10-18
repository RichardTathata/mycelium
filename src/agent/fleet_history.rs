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
//!   window from the moment of admission (`seat_admissible`), so one seat cannot admit twice within a window. The
//!   seat stays held until its lease ends, never released early. A slot holds one live decision at a time —
//!   single-decree agreement, the property `distributed_lock` rests on — so at most `count` seats are live at once and
//!   the fleet admits at most `count` in any window, whichever gateways the calls reach. Every seat taken: denied. A
//!   claim that cannot be decided (no quorum, the electorate refused), or that took longer than the margin: the rule
//!   decides `Indeterminate` — it fails closed. One consensus round per limited call (more when seats are contended).
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
//! the policy: an exact fleet limit must name its actor, never `*`.
//!
//! **What is not claimed.** The seat's lease is measured on the proposer's wall clock and judged on each reader's,
//! so the exact bound holds to within the clocks' skew. Without a declared exclusive-default electorate the counter
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
#[cfg(feature = "consensus")]
const CLAIM_PHASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(feature = "consensus")]
const CLAIM_MAX_BALLOTS: u32 = 4;

/// How much longer than the window a seat is leased (review finding 2): the lease is fixed when the claim is
/// proposed and the call is admitted only after the round, so the lease must outlast the window measured from
/// admission. A claim slower than this is refused rather than admitted (`seat_admissible`).
#[cfg(feature = "consensus")]
pub(crate) const SEAT_LEASE_MARGIN_MS: u64 = 2_000;

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

/// What a claim came to.
#[derive(Debug)]
#[cfg_attr(not(feature = "consensus"), allow(dead_code))]
pub(crate) enum Claim {
    /// This permit holds a seat.
    Seat(SeatClaim),
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
        ..ConsensusConfig::default()
    };
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
        match consensus.exclusive_propose(&slot, value.clone(), cfg.clone()).await {
            ConsensusResult::Committed { .. } => {
                // The converged record decides whose the seat is, polled through the replay seam.
                const BUDGET_MS: u64 = 1000;
                const STEP_MS: u64 = 25;
                let mut waited = 0;
                let top = loop {
                    if let Some(top) = seat_live(ctx, &slot) {
                        break top;
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
                return Claim::Seat(SeatClaim::new(c.key.clone(), slot, seat, top.env.token, expires));
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

/// Permits in `entry` inside the window ending at `now_ms`: `Ok(0)` when there is none, `Err` when there is one this
/// node cannot read — never counted as zero (#642's review, N6).
fn counted_in(entry: Option<Bytes>, now_ms: u64, window_ms: u64) -> Result<u32, ()> {
    let from = now_ms.saturating_sub(window_ms);
    match entry {
        None => Ok(0),
        Some(b) => serde_json::from_slice::<TallyEntry>(&b)
            .map(|e| e.t.iter().filter(|t| **t > from).count() as u32)
            .map_err(|_| ()),
    }
}

/// The fleet's sum for `c` and `actor`, over this node and every enforcing peer — or why it is not established:
/// this node's own view of the fleet is not certain enough (the governors' confidence bound,
/// `control_max_staleness_ms` / `control_min_peers_heard`), or a peer's coverage is unknown — no heartbeat, one older
/// than `control_max_staleness_ms`, or one dated beyond `max_clock_drift_ms` in this node's future. A publisher
/// failure detection has declared dead, or that was removed, is no longer a peer and is not consulted.
fn tally_state(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, now_ms: u64) -> FleetState {
    let bound = crate::control::ConfidenceBound::from_config(&ctx.config);
    let view = super::emergent::compute_view_confidence_at(ctx, mycelium_core::sim_seam::mono_instant());
    if let Err(why) = crate::control::assess(&view, &bound) {
        return FleetState::NotEstablished(format!(
            "this node's view of the fleet is not established ({why:?}), so the tally's coverage is unknown"
        ));
    }
    let me = ctx.node_id.to_string();
    let mut unknown = Vec::new();
    let mut counted = 0;
    let mut add = |node: &str, unknown: &mut Vec<String>| {
        match counted_in(mycelium_core::ops::kv_get(ctx, &tally_key(node, &c.key, point, actor)), now_ms, c.window_ms) {
            Ok(n) => counted += n,
            Err(()) => unknown.push(format!("{node} (an entry this node cannot read)")),
        }
    };
    add(&me, &mut unknown);
    let mut publishers = vec![me.clone()];
    let mut stalest_ms = 0;
    let peers: std::collections::BTreeSet<String> = ctx.peers.pin().keys().map(|p| p.to_string()).collect();
    for peer in &peers {
        let Some(hb) = mycelium_core::ops::kv_get(ctx, &heartbeat_key(peer))
            .and_then(|b| serde_json::from_slice::<Heartbeat>(&b).ok())
        else {
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
    // still counts: its recent permits stand until they age out (#642's review, round 3, finding 6). It is not
    // waited for: it admits nothing more unless it returns, and then it is a peer again.
    for (node, hb) in known_heartbeats(ctx) {
        if node == me || peers.contains(&node) || !hb.enforces {
            continue;
        }
        if hb.as_of > now_ms.saturating_add(ctx.config.max_clock_drift_ms) {
            unknown.push(format!("{node} (a heartbeat {} ms in the future)", hb.as_of - now_ms));
            continue;
        }
        add(&node, &mut unknown);
        publishers.push(node);
    }
    if !unknown.is_empty() {
        return FleetState::NotEstablished(format!(
            "the tally of {} is unknown (fresh within {} ms is required) — a count that cannot be read is not zero",
            unknown.join(", "),
            bound.max_staleness_ms
        ));
    }
    FleetState::Tally { counted, publishers, stalest_ms }
}

/// Publish a permit admitted under `c` into this node's tally of `actor`, and keep this node's heartbeat current.
/// Called under the actor's gate, so two permits of one actor at this point do not lose each other's update.
pub(crate) fn publish_permit(ctx: &Arc<TaskCtx>, c: &FleetCounter, point: &str, actor: &str, at_ms: u64) {
    let key = tally_key(&ctx.node_id.to_string(), &c.key, point, actor);
    // The read-modify-write and the withdrawal's read-then-delete take one lock (lock-order row 64, #642's review N7),
    // so an aged entry's withdrawal cannot delete a permit published between its read and its delete.
    let tally = ctx.ae_tally_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut entry = mycelium_core::ops::kv_get(ctx, &key)
        .and_then(|b| serde_json::from_slice::<TallyEntry>(&b).ok())
        .unwrap_or_default();
    let from = at_ms.saturating_sub(c.window_ms);
    entry.t.retain(|t| *t > from);
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
/// nothing can be read from it).
fn aged(value: &[u8], now_ms: u64) -> bool {
    serde_json::from_slice::<TallyEntry>(value)
        .map(|e| e.t.iter().all(|t| *t <= now_ms.saturating_sub(e.w)))
        .unwrap_or(true)
}

/// Withdraw this node's tally entries whose every time has aged out of the window they were kept for (review
/// finding 4): a tally holds only actors active within a window. Under the tally lock, so a permit published
/// concurrently is either seen by the read or lands after the delete (N7).
pub(crate) fn withdraw_aged_entries(ctx: &Arc<TaskCtx>, now_ms: u64) -> usize {
    let prefix = format!("{TALLY_PREFIX}{}/c/", ctx.node_id);
    let _tally = ctx.ae_tally_lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut withdrawn = 0;
    for (key, value) in mycelium_core::ops::kv_scan_prefix(ctx, &prefix) {
        if aged(&value, now_ms) {
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

/// Every node's heartbeat this node holds, by node — departed or not, peer or not.
fn known_heartbeats(ctx: &TaskCtx) -> Vec<(String, Heartbeat)> {
    mycelium_core::ops::kv_scan_prefix(ctx, TALLY_PREFIX)
        .into_iter()
        .filter_map(|(key, value)| {
            let node = key.strip_prefix(TALLY_PREFIX)?.strip_suffix("/hb")?.to_string();
            let hb = serde_json::from_slice::<Heartbeat>(&value).ok()?;
            Some((node, hb))
        })
        .collect()
}

/// This node's **last word** at a graceful shutdown (N3): a heartbeat marked departed, written past the soft-state
/// gate by the shutdown sweep. Its entries are kept: peers go on counting its recent permits until they age out, and
/// do not wait for it.
///
/// **Nobody else deletes a node's tally keys** (#642's review, round 3, finding 2): a collector cannot tell a node's
/// old record from one it wrote after returning that has not reached the collector yet, so any delete by another
/// node could erase a returned node's heartbeat or a permit it published, and would trip its `sys/` tripwire. A node
/// withdraws its own aged entries (every heartbeat tick, from its next start). What a node that never returns
/// leaves behind is bounded: its heartbeat and one entry per (counter, actor) it counted in its last window, each
/// read as aged and counting nothing.
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
                    withdraw_aged_entries(&task_ctx, now);
                }
            }
        }
    });
}

/// The live tests start consensus listeners, so they need the `consensus` feature (on by default).
#[cfg(all(test, feature = "consensus"))]
#[path = "fleet_history_tests.rs"]
mod tests;
