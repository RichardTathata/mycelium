//! **Electorate migration — enumerate, carry, complete** (post-360 plan row P2's follow-up;
//! `docs/design/consensus-electorate.md` §8.3).
//!
//! A migration moves who decides from a retired electorate group (the **predecessor**) to its **successor**, and
//! it **terminates**: after the successor's completion record is decided, the predecessor is never consulted again
//! and its members may be removed. The steps, each a consensus decision or a read of one:
//!
//! 1. **close** — every predecessor member closes the group, durably, at the retirement's promise
//!    (`consensus::GateKind::Retire`); once the retirement is chosen no predecessor value can be chosen, because the
//!    promise majority intersects every predecessor majority;
//! 2. **enumerate** — a successor member asks a strict majority of the predecessor for a **manifest**: every slot
//!    each holds acceptor state for *in that group* (the acceptor memory is tagged with the electorate group a
//!    claim passed through — `consensus::AcceptorSlot::groups`, durable in the acceptor record), with its highest
//!    acceptance. The union is the carry set `S`. It is complete: a value chosen in the predecessor is held by a
//!    strict majority of it, which the manifest majority intersects, so some manifest reports it at a ballot at or
//!    above the chosen one — and by the predecessor's own phase-1 rule that is the chosen value;
//! 3. **carry** — for each slot in `S`, one ordinary ballot of the successor proposing the highest acceptance's
//!    decision envelope whole (`ConsensusEngine::propose_carry`): the holder's window, lineage, proposer and token
//!    are kept, so a lock keeps its holder's window and a released decision stays released (its marker names the
//!    lineage). The successor's own phase 1 applies: an acceptance the successor already holds above it wins;
//! 4. **complete** — a decision of the successor on `electorate/{successor}/carried/{predecessor}`
//!    ([`Completion`]), certified by a strict majority of the successor. Until it, the successor decides nothing
//!    outside `S` (the proposer's door refuses, naming this step); after it, anything, without the predecessor.
//!
//! **Restart before completion:** re-enumerate — every step is idempotent. A slot carried before the restart is
//! carried again; the successor's phase 1 finds its own acceptance and re-decides the same envelope. The cost is one
//! ballot per slot in `S`, sequential (stated; a batch would need the engine to run several ballots of one
//! proposer at once).
//!
//! **Liveness, stated:** a migration needs a reachable strict majority of the predecessor — to retire it and to
//! enumerate it — and of the successor. A predecessor that lost its majority cannot migrate; disaster recovery is
//! not built. A manifest entry whose value is known only by digest (a predecessor member that restarted holding an
//! acceptance over the acceptor record's value cap) is resolved from a decision record the reporter or the
//! completer holds; if none does, the carry stops at that slot, named, and is retried.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;

use super::electorate::{self, Completion, ElectorateDecl, ElectorateError, MigrationPhase, MigrationState};
use super::TaskCtx;
use crate::consensus::{ConsensusEngine, ConsensusResult, ManifestEntry};
use crate::node_id::NodeId;

/// Where one completion run on this node stands for one `(group, predecessor)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Slots in the carry set.
    pub(crate) total:   u64,
    /// Slots carried so far.
    pub(crate) carried: u64,
}

/// Test-only: a completion run on this process for the group whose name hashes to `CARRY_STOP_GROUP` stops — as a
/// crash would — after carrying `CARRY_STOP_AFTER` slots (once). Keyed by group so a completion another test runs in
/// the same process does not consume the stop.
#[cfg(test)]
pub(crate) static CARRY_STOP_AFTER: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(crate) static CARRY_STOP_GROUP: AtomicU64 = AtomicU64::new(0);

/// Test-only: the group-name hash `CARRY_STOP_GROUP` holds.
#[cfg(test)]
pub(crate) fn stop_key(group: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    group.hash(&mut h);
    h.finish()
}

static SLOTS_CARRIED: AtomicU64 = AtomicU64::new(0);
static COMPLETIONS: AtomicU64 = AtomicU64::new(0);
static MANIFEST_PAGES: AtomicU64 = AtomicU64::new(0);

/// Slots this process carried into a successor electorate (each successful carry ballot).
pub fn migration_slots_carried() -> u64 {
    SLOTS_CARRIED.load(Ordering::Relaxed)
}

/// Migration completions this process decided or found decided while completing.
pub fn migration_completions() -> u64 {
    COMPLETIONS.load(Ordering::Relaxed)
}

/// Manifest pages this process received while enumerating.
pub fn migration_manifest_pages() -> u64 {
    MANIFEST_PAGES.load(Ordering::Relaxed)
}

fn note(counter: &AtomicU64, metric: &'static str) {
    counter.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!(metric).increment(1);
    #[cfg(not(feature = "metrics"))]
    let _ = metric;
}

/// The highest acceptance the manifests reported for one slot.
#[derive(Clone, Debug)]
struct Highest {
    ballot:      u64,
    digest:      [u8; 32],
    value:       Option<Bytes>,
    is_envelope: bool,
    /// The highest ballot any reporter promised or accepted for the slot: the carry starts above it (review 5).
    above:       u64,
    /// Whether any reporter's memory tags the slot with the retired group (an untagged slot may not be the group's).
    tagged:      bool,
}

/// Where `group`'s migration stands, per predecessor, as this node sees it.
pub(crate) fn state(ctx: &Arc<TaskCtx>, group: &str) -> Vec<MigrationState> {
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let Some(d) = engine.electorate_view(group) else { return Vec::new() };
    d.predecessors.iter().map(|p| {
        let phase = if let Some(c) = engine.completion_view(group, p) {
            MigrationPhase::Complete { slots: c.slots, conflicts: c.conflicts }
        } else if engine.drop_view(group, p).is_some() {
            MigrationPhase::Dropped
        } else if engine.retirement_view(p).is_some_and(|r| r.successor == group) {
            match ctx.electorate_migrations.pin().get(electorate::migration_key(group, p).as_str()) {
                Some(pr) => MigrationPhase::Carrying { carried: pr.carried, total: pr.total },
                None => MigrationPhase::Closed,
            }
        } else {
            MigrationPhase::Waiting
        };
        MigrationState { predecessor: p.clone(), phase }
    }).collect()
}

/// **Complete `group`'s migration** — one attempt; see [`super::GossipAgent::complete_migration`].
pub(crate) async fn complete_outcome(ctx: &Arc<TaskCtx>, group: &str) -> Result<Vec<MigrationState>, ElectorateError> {
    if !electorate::valid_group(group) { return Err(ElectorateError::InvalidGroup); }
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let Some(d) = engine.electorate_view(group) else {
        return Err(ElectorateError::NotDecided(format!("{group} has no electorate visible here yet")));
    };
    if !d.contains(&ctx.node_id) { return Err(ElectorateError::NotAMember); }
    if let Some(r) = engine.retirement_view(group) {
        return Err(ElectorateError::Retired { successor: r.successor.clone() });
    }
    // One completion run per group per node: a second caller is told to wait rather than carry twice.
    let key: Arc<str> = Arc::from(group);
    let entered = ctx.migrations_running.pin().insert(Arc::clone(&key));
    if !entered {
        return Err(ElectorateError::NotDecided(format!("a completion of {group} is already running on this node")));
    }
    let _leave = Running { ctx: Arc::clone(ctx), key };
    for p in &d.predecessors {
        if engine.completion_view(group, p).is_some() { continue; }
        // Not retired here yet: the successor waits (gossip, or the operator has not retired it); nothing to carry.
        // Dropped: never carried.
        let retirement = match engine.retirement_view(p) {
            Some(r) if r.successor == group => r,
            Some(_) | None if engine.drop_view(group, p).is_some() => continue,
            Some(r) => return Err(ElectorateError::Refused(format!(
                "predecessor {p} retired to {}, not {group} — drop it (drop_predecessor) to go on without it", r.successor))),
            None => continue,
        };
        let Some(pd) = engine.electorate_view(p) else {
            return Err(ElectorateError::NotDecided(format!("predecessor {p}'s electorate is not visible here")));
        };
        let set = enumerate(&engine, ctx, &pd, &d).await.map_err(ElectorateError::NotDecided)?;
        let mkey: Arc<str> = Arc::from(electorate::migration_key(group, p).as_str());
        ctx.electorate_migrations.pin().insert(Arc::clone(&mkey), Progress { total: set.len() as u64, carried: 0 });
        tracing::info!(group, predecessor = %p, slots = set.len(), "electorate migration: carry set enumerated");
        let mut conflicts: Vec<String> = Vec::new();
        for (done, (slot, best)) in set.iter().enumerate() {
            if carry_slot(&engine, ctx, &d, p, slot, best).await.map_err(ElectorateError::NotDecided)? == Carried::Conflict {
                conflicts.push(slot.to_string());
            }
            let carried = done as u64 + 1;
            ctx.electorate_migrations.pin().insert(Arc::clone(&mkey), Progress { total: set.len() as u64, carried });
            #[cfg(test)]
            if CARRY_STOP_AFTER.load(Ordering::Relaxed) == carried && CARRY_STOP_GROUP.load(Ordering::Relaxed) == stop_key(group) {
                CARRY_STOP_AFTER.store(0, Ordering::Relaxed);
                return Err(ElectorateError::Refused(format!("test: the completion run stopped after carrying {carried} slot(s)")));
            }
        }
        // A conflict is recorded, not a reason to refuse the completion: it is a fact about the past (two values were
        // chosen for one slot — a forced drop of a predecessor that then retired, a 2.32.0 member that voted without
        // closing, two merged defaults that both held the slot), which no carry can un-choose; refusing would freeze the
        // successor for good with nothing an operator could do. The value the successor holds stands, the tripwire
        // counted each, the log named each, and the completion carries the count and the first names.
        let completion = Completion {
            group: group.to_string(), epoch: d.epoch, predecessor: p.clone(), predecessor_epoch: pd.epoch,
            passes_default: retirement.passes_default, slots: set.len() as u64, conflicts: conflicts.len() as u64,
            conflict_slots: conflicts.iter().take(electorate::CONFLICTS_NAMED).cloned().collect(), digest: carry_set_digest(&set),
        };
        let slot = electorate::carried_slot(group, p);
        let result = engine.propose(
            crate::signal::SignalScope::Group(Arc::from(group)), Arc::from(slot.as_str()), completion.encode(),
            d.quorum(), electorate::decision_config(), None,
        ).await;
        match (result, engine.completion_view(group, p)) {
            (_, Some(c)) => {
                note(&COMPLETIONS, "mycelium_electorate_migration_completions_total");
                // The predecessor's records this member needs once the predecessor is gone, on stable storage now.
                let _ = engine.sync_migration_records(&c).await;
                tracing::info!(group, predecessor = %p, slots = set.len(), "electorate migration: complete");
            }
            (ConsensusResult::ElectorateMismatch { detail, .. }, None) => return Err(ElectorateError::Refused(detail.to_string())),
            (ConsensusResult::NotAMember { .. }, None) => return Err(ElectorateError::NotAMember),
            (other, None) => return Err(ElectorateError::NotDecided(format!(
                "the completion of {group}'s carry from {p} was not decided: {other:?}"))),
        }
    }
    Ok(state(ctx, group))
}

/// Releases the per-group single-flight mark when a completion run ends, however it ends.
struct Running {
    ctx: Arc<TaskCtx>,
    key: Arc<str>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.ctx.migrations_running.pin().remove(&self.key);
    }
}

/// How long one manifest page is waited for, and how many times a member is asked again.
const MANIFEST_PAGE_TIMEOUT_MS: u64 = 3_000;
const MANIFEST_RETRIES: u32 = 3;

/// **Enumerate** the carry set from a strict majority of `pd`'s members: each member's manifest for `pd`, paged;
/// the union keyed by slot, keeping the highest acceptance. This node's own manifest counts when it is a member,
/// read after it closed the group durably. `Err` names what stopped it: no majority answered, or two values at one
/// ballot (a pre-2.30.0 proposer), which no carry can resolve.
async fn enumerate(
    engine: &ConsensusEngine, ctx: &Arc<TaskCtx>, pd: &ElectorateDecl, successor: &ElectorateDecl,
) -> Result<BTreeMap<Arc<str>, Highest>, String> {
    let me = &ctx.node_id;
    let mut heard: Vec<NodeId> = Vec::new();
    let mut set: BTreeMap<Arc<str>, Highest> = BTreeMap::new();
    let fold = |set: &mut BTreeMap<Arc<str>, Highest>, entries: Vec<ManifestEntry>| -> Result<(), String> {
        for e in entries {
            let cur = set.entry(Arc::clone(&e.slot)).or_insert(Highest {
                ballot: 0, digest: [0; 32], value: None, is_envelope: false, above: 0, tagged: false,
            });
            cur.above = cur.above.max(e.promised).max(e.accepted_ballot);
            cur.tagged |= e.tagged;
            if e.accepted_ballot == 0 { continue; }
            let Some(digest) = e.accepted_digest else { continue };
            if e.accepted_ballot > cur.ballot {
                cur.ballot = e.accepted_ballot; cur.digest = digest; cur.value = e.accepted_value; cur.is_envelope = e.accepted_is_envelope;
            } else if e.accepted_ballot == cur.ballot && cur.ballot > 0 {
                if digest != cur.digest {
                    return Err(format!("slot {}: two values at ballot {} in {} — no carry can choose between them",
                        e.slot, cur.ballot, pd.group));
                }
                if cur.value.is_none() { cur.value = e.accepted_value; cur.is_envelope = e.accepted_is_envelope; }
            }
        }
        Ok(())
    };
    if pd.contains(me) {
        if !engine.close_for_carry(pd).await {
            return Err(format!("{} could not be closed durably on this node", pd.group));
        }
        fold(&mut set, engine.local_manifest(&pd.group))?;
        heard.push(me.clone());
    }
    for m in pd.members.iter().filter(|m| *m != me) {
        if heard.len() >= pd.quorum() { break; }
        let mut got = None;
        for _ in 0..MANIFEST_RETRIES {
            match engine.pull_manifest(&pd.group, &successor.group, m, MANIFEST_PAGE_TIMEOUT_MS).await {
                Ok(entries) => { got = Some(entries); break; }
                Err(why) => tracing::debug!(group = %pd.group, member = %m, why, "electorate migration: a manifest did not arrive"),
            }
        }
        if let Some(entries) = got {
            note(&MANIFEST_PAGES, "mycelium_electorate_manifests_total");
            fold(&mut set, entries)?;
            heard.push(m.clone());
        }
    }
    if heard.len() < pd.quorum() {
        return Err(format!("the carry set of {} needs a strict majority of its {} members' manifests; {} answered",
            pd.group, pd.members.len(), heard.len()));
    }
    Ok(set)
}

/// SHA-256 over the sorted carry set — `slot\0ballot LE` per slot — the completion's digest.
fn carry_set_digest(set: &BTreeMap<Arc<str>, Highest>) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"mycelium.electorate-carry-set/1\0");
    for (slot, best) in set {
        h.update(slot.as_bytes());
        h.update([0]);
        h.update(best.ballot.to_le_bytes());
    }
    h.finalize().into()
}

/// What one slot's carry did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carried {
    /// Nothing to carry, or the carried value decided (or already decided) in the successor.
    Carried,
    /// The successor already held a **different** value above the carried one: a detected conflict.
    Conflict,
}

/// **Carry one slot**: one ballot of the successor `d` proposing the highest acceptance's envelope. A slot with
/// no acceptance in any manifest has nothing to carry (no value was chosen in the predecessor). A value known
/// only by digest is resolved from a decision record this node holds, or the carry stops here, named. A ballot that
/// finds a different value already decided above the carried one is a **conflict** (review round 2, A(ii)): counted
/// on the commit-conflict tripwire, logged by name, reported — never counted as carried.
async fn carry_slot(engine: &ConsensusEngine, ctx: &Arc<TaskCtx>, d: &ElectorateDecl, predecessor: &str, slot: &Arc<str>, best: &Highest) -> Result<Carried, String> {
    if best.ballot == 0 { return Ok(Carried::Carried); }
    // An untagged slot (reported for completeness, review 2) that the successor may not decide at all — a cluster-scoped
    // exclusive slot a member once answered, say — was never the predecessor's either: nothing to carry. A tagged one
    // is carried whatever the family check says now. Whether a slot is the successor's cannot always be told (the
    // fleet's default not nameable here — two live holders mid-merge): then the carry stops, named (review round 2, D).
    if !best.tagged {
        match engine.slot_home(slot, &d.group) {
            Ok(true) => {}
            Ok(false) => return Ok(Carried::Carried),
            Err(why) => return Err(format!("slot {slot}: whether it is {}'s to decide cannot be told here — {why}; the carry waits", d.group)),
        }
    }
    let value = match &best.value {
        Some(v) => v.clone(),
        None => engine.resolve_envelope_by_digest(slot, &best.digest).ok_or_else(|| format!(
            "slot {slot}: the acceptance at ballot {} in the carry set is known only by its digest, and no decision record \
             here resolves it — a predecessor member restarted holding a value over the acceptor record's cap; the carry \
             waits for the record", best.ballot))?,
    };
    let envelope = if best.is_envelope {
        match crate::consensus_life::Envelope::decode(&value) {
            Some(e) if e.slot == **slot => value,
            _ => return Err(format!("slot {slot}: the carried acceptance is not a decision envelope for it")),
        }
    } else {
        // A legacy acceptance (pre-row-A proposer): wrapped as an adopter wraps it — a lease, never permanent, since
        // no proposer in the scheme could release it (`consensus::LEGACY_ADOPTION_LEASE_MS`).
        let ms = crate::consensus::LEGACY_ADOPTION_LEASE_MS;
        let term = crate::consensus_life::Term::Lease { ms, expires_at_ms: mycelium_core::sim_seam::wall_now_ms().saturating_add(ms) };
        crate::consensus_life::Envelope::wrap_legacy(slot, value, term, ctx.node_id.clone(), ctx.hlc.tick(), best.ballot).encode()
    };
    let carried_identity = crate::consensus_life::Envelope::decode(&envelope).map(|e| e.identity());
    match engine.propose_carry(&d.group, predecessor, slot, envelope, d.quorum(), best.above).await {
        ConsensusResult::Committed { .. } => {
            note(&SLOTS_CARRIED, "mycelium_electorate_slots_carried_total");
            Ok(Carried::Carried)
        }
        ConsensusResult::Superseded { .. } => {
            // The successor decided something else: its own earlier carry of the same decision (a restart re-carry, the
            // same identity) is carried; another identity is a conflict.
            let top = match crate::consensus_life::read_slot(&ctx.kv_state, slot, mycelium_core::sim_seam::wall_now_ms()) {
                crate::consensus_life::SlotView::Top(t) => Some(t.env.identity()),
                _ => None,
            };
            if top.is_some() && top == carried_identity {
                note(&SLOTS_CARRIED, "mycelium_electorate_slots_carried_total");
                return Ok(Carried::Carried);
            }
            ctx.commit_conflicts.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_electorate_carry_conflicts_total").increment(1);
            if ctx.config.emergent_detectors_enabled {
                crate::agent::emergent::record_event(ctx, "commit_conflict",
                    format!("migration carry conflict: {} already decided another value for {slot} above the one carried from {predecessor}", d.group));
            }
            tracing::warn!(group = %d.group, predecessor, slot = %slot, "electorate migration: the carry found another value \
                decided above the one carried — two values were chosen for one slot (a forced drop that raced a retirement, \
                a 2.32.0 member, two merged defaults); the successor's value stands; counted on commit_conflicts");
            Ok(Carried::Conflict)
        }
        other => Err(format!("slot {slot}: the carry ballot in {} did not decide: {other:?}", d.group)),
    }
}
