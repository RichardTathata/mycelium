//! **The acceptor's memory guard** — detection, never prevention (`docs/design/consensus-electorate.md` §8.5;
//! the hazard PR #604 documents).
//!
//! An electorate member's promises and acceptances are durable on its own disk (`sys/consensus-accepted/{me}/{slot}`,
//! read once at start by `consensus::prewarm_accepted`). A member restarted over an **empty or older** store, or one
//! that never configured `[persistence]`, answers the next prepare as a fresh acceptor: it can promise below what it
//! promised and forget what it accepted, and a quorum counting it can decide a **second value**. Nothing prevented
//! that, and until this module nothing detected it. Three detections, each by name, each counted:
//!
//! 1. **Persistence required** ([`persistence_required`]): a node that would adopt, declare, propose or answer a genesis
//!    for an electorate it is a member of with no `[persistence]` refuses by name (`ElectorateError::PersistenceRequired`,
//!    `409 persistence_required` at the gateway). Deterministic: `persist_sync` returns `true` with nothing on disk when
//!    no WAL is configured (`consensus.rs`, "no promise was made"), so such a member forgets on **every** restart. A
//!    node that is not a member, and consensus outside an electorate, are untouched.
//! 2. **An empty store, warned** ([`check_memory_for`]): a member of an adopted electorate whose acceptor memory holds
//!    nothing for the group's genesis slot. Every member accepted the genesis (a genesis is decided by all of them), and
//!    the collector's `shrink_acceptor` keeps `{promised}` rather than deleting, so a member that kept its store always
//!    holds at least that one record. Checked at `start()` from the committed records the store holds, and when the
//!    committed record first arrives by gossip (an empty store has no record at start). **Warned, not refused:** the
//!    committed record is read unverified here (a forged one is refused by `electorate_view`, not by this check), and a
//!    brand-new member legitimately holds nothing until its genesis is decided — this check fires only on a *committed*
//!    genesis, which is how "not yet decided" is told from "decided, and I hold nothing".
//! 3. **A forgotten promise, withheld** ([`install`]): on the core's inbound seam (`CoreCtx::inbound_observer`,
//!    `connection::apply_inbound`), a peer's copy of this node's **own** acceptor record is compared with the memory
//!    for that slot — independently of what LWW made of it in the store. Memory is monotone (a promise never lowers;
//!    `shrink_acceptor` raises `promised` to the ended ballot), so a copy whose promised or accepted ballot is
//!    **above** memory is an answer this node gave and no longer holds. The slot goes into
//!    `TaskCtx::acceptor_withheld`: this node answers no prepare and no proposal for it, as acceptor or proposer,
//!    **until the next start**, when `prewarm_accepted` reads the repaired record (the copy reached the store; an
//!    orderly shutdown snapshots it, a crash before a snapshot may lose it and the next start detects it again). A
//!    withheld slot never resumes without a restart: the memory that would make its answers safe is on disk, not in the
//!    process, and refreshing it live would race the acceptor's own writes. **The disk is repaired at once**, though
//!    (#605's review, finding 1): on detection the **merged** record — the higher promise, the higher acceptance — is
//!    written to this node's own key under `acceptor_records` ([`repair_own_record`]), a Layer III write of a key the
//!    agent owns, not a Layer I guard. Without it a node that had already promised a ballot empty-handed before the
//!    copy arrived would have written `{promised}` with a fresh HLC, and its own write would beat the copy by LWW
//!    locally and fleet-wide — the next start would read the empty-handed record and the forgotten acceptance would be
//!    gone for good. `persist_acceptor` writes nothing for a withheld slot, so no answer in flight lands over the
//!    merged record.
//!
//! **What this cannot see** (the limit, stated in §8.5 and `what-is-proven.md`): detection 3 needs the copy to arrive
//! — a promise whose gossip reached no survivor is forgotten unseen; and it needs to arrive **before** the node answers
//! a higher ballot for the slot — once the node has promised ballot `b+1` empty-handed, a copy carrying `b` is below
//! memory and reads as stale. Between the restart and the repair's arrival the hazard is open; the test states where its
//! protection depends on that order.
//!
//! Nothing here touches Layer I: core reports an arrival through a seam the agent installs, the comparison and the
//! withholding are the agent's, and no write is guarded or refused — the `sys/` tripwire's rule, one layer up.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;

use super::electorate::{genesis_slot, ElectorateDecl, ELECTORATE_SLOT_PREFIX};
use super::TaskCtx;
use crate::consensus::{accepted_key, consensus_ns, decode_acceptor, encode_acceptor, AcceptorMemory, AcceptorSlot, FLOOR_SENTINEL};
use crate::node_id::NodeId;

static PERSISTENCE_REFUSED: AtomicU64 = AtomicU64::new(0);
static MEMORY_EMPTY: AtomicU64 = AtomicU64::new(0);
static PROMISES_FORGOTTEN: AtomicU64 = AtomicU64::new(0);

/// Refusals because an electorate member has no `[persistence]`: an adoption (counted once per record), a
/// declaration, a genesis proposal at the door, or a genesis answer at the acceptor (each counted as it happens).
pub fn electorate_persistence_refused() -> u64 {
    PERSISTENCE_REFUSED.load(Ordering::Relaxed)
}

/// Electorate groups this node is a member of whose genesis slot its acceptor memory holds nothing for — a store
/// emptier than the fleet's record of this node. Warned once per group.
pub fn acceptor_memory_empty_for_electorate() -> u64 {
    MEMORY_EMPTY.load(Ordering::Relaxed)
}

/// Slots this process withheld every answer for because a peer's copy of its own acceptor record carried a promise or
/// an acceptance above its memory — an answer it gave and forgot. Once per slot; cleared only by a restart.
pub fn acceptor_promises_forgotten() -> u64 {
    PROMISES_FORGOTTEN.load(Ordering::Relaxed)
}

pub(crate) use super::electorate::PERSISTENCE_REQUIRED_DETAIL;

/// Bound on each once-only notice set.
const NOTICES_CAP: usize = 4096;

/// The once-only notices, one bounded set per kind (`TaskCtx::acceptor_notices`), so records of one kind — unverified
/// committed records, say — cannot exhaust another kind's set and silence it (#605's review, finding 6).
#[derive(Clone, Copy)]
pub(crate) enum Notice {
    /// A persistence refusal at a door (`persist:{group}/{epoch}:{site}`).
    Persist = 0,
    /// An adoption refused for want of persistence (`{group}/{epoch}`).
    Adopt = 1,
    /// An empty store for a committed genesis (`{group}`).
    Empty = 2,
}

/// The notice sets' type: one bounded set per [`Notice`] kind.
pub(crate) type NoticeSets = [papaya::HashSet<Arc<str>>; 3];

/// A fresh [`NoticeSets`].
pub(crate) fn notice_sets() -> NoticeSets {
    [papaya::HashSet::new(), papaya::HashSet::new(), papaya::HashSet::new()]
}

/// `true` the first time `key` is noticed under `kind` in this process (bounded per kind: past the cap nothing is new).
fn notice_once(notices: &NoticeSets, kind: Notice, key: &str) -> bool {
    let set = notices[kind as usize].pin();
    if set.contains(key) || set.len() >= NOTICES_CAP { return false; }
    set.insert(Arc::from(key))
}

/// Whether `key` was noticed under `kind` (tests).
#[cfg(test)]
pub(crate) fn noticed(ctx: &TaskCtx, kind: Notice, key: &str) -> bool {
    ctx.acceptor_notices[kind as usize].pin().contains(key)
}

/// Whether `decl` names this node and this node has no `[persistence]` — the condition every door refuses on.
pub(crate) fn persistence_required(ctx: &TaskCtx, decl: &ElectorateDecl) -> bool {
    decl.contains(&ctx.node_id) && ctx.config.persistence.is_none()
}

/// Counts a persistence refusal and warns once per `(group, epoch, site)`.
pub(crate) fn note_persistence_refused(ctx: &TaskCtx, group: &str, epoch: u64, site: &str) {
    PERSISTENCE_REFUSED.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_acceptor_guard_total", "event" => "persistence_required").increment(1);
    if notice_once(&ctx.acceptor_notices, Notice::Persist, &format!("{group}/{epoch}:{site}")) {
        tracing::warn!(group, epoch, site, "electorate: refused — {PERSISTENCE_REQUIRED_DETAIL} \
            (see mycelium::electorate_persistence_refused)");
    }
}

/// The adoption half of detection 1: `true` when `electorate_view` must not adopt `decl` on this node. Counted on
/// every refused view of the record (the counter does not depend on the notice — #605's review, finding 6); warned
/// once per record.
pub(crate) fn refuse_adoption_without_persistence(ctx: &TaskCtx, decl: &ElectorateDecl) -> bool {
    if !persistence_required(ctx, decl) { return false; }
    PERSISTENCE_REFUSED.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_acceptor_guard_total", "event" => "persistence_required").increment(1);
    if notice_once(&ctx.acceptor_notices, Notice::Adopt, &format!("{}/{}", decl.group, decl.epoch)) {
        tracing::warn!(group = %decl.group, epoch = decl.epoch, "electorate: not adopted — {PERSISTENCE_REQUIRED_DETAIL} \
            (see mycelium::electorate_persistence_refused)");
    }
    true
}

/// Whether `memory` holds anything for `slot` — a promise or an acceptance (`shrink_acceptor` leaves the promise).
fn holds(memory: &AcceptorMemory, slot: &str) -> bool {
    memory.pin().get(slot).is_some_and(|s| s.promised > 0 || s.accepted.is_some())
}

/// Detection 2, over the leaf state (so the inbound observer can run it without the agent): warns once per group.
fn check_memory(memory: &AcceptorMemory, notices: &NoticeSets, me: &NodeId, decl: &ElectorateDecl) {
    if !decl.contains(me) { return; }
    let slot = genesis_slot(&decl.group, decl.epoch);
    if holds(memory, &slot) { return; }
    if !notice_once(notices, Notice::Empty, &decl.group) { return; }
    MEMORY_EMPTY.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_acceptor_guard_total", "event" => "memory_empty").increment(1);
    tracing::warn!(group = %decl.group, epoch = decl.epoch, slot = %slot,
        "consensus: this node is a member of a decided electorate and holds no acceptor record for its genesis — \
         its store is emptier than the fleet's record of it (a restart over an empty or older store, or a reissued \
         identity); every promise and vote it gave before is forgotten until the records are repaired. Restore the \
         persistence directory at its latest state, or retire the node (guide 04 § Changing an electorate's members; \
         see mycelium::acceptor_memory_empty_for_electorate)");
}

/// Detection 2 for an adopted electorate (`electorate_view`, on adoption).
pub(crate) fn check_memory_for(ctx: &TaskCtx, decl: &ElectorateDecl) {
    check_memory(&ctx.consensus_accepted, &ctx.acceptor_notices, &ctx.node_id, decl);
}

/// A committed electorate record's value, decoded — written raw, or inside a decision envelope.
fn decode_committed(value: &Bytes) -> Option<ElectorateDecl> {
    ElectorateDecl::decode(value).or_else(|| crate::consensus::raw_of(value).and_then(|v| ElectorateDecl::decode(&v)))
}

/// Detection 2 at `start()`, after `prewarm_accepted`: every group whose highest committed genesis the store holds
/// names this node is checked against the recovered memory. A store with no records warns nothing here — the record
/// arrives by gossip and the inbound observer checks it then.
pub(crate) fn check_at_start(ctx: &TaskCtx) {
    let prefix = format!("{}{ELECTORATE_SLOT_PREFIX}", consensus_ns::COMMITTED);
    let mut top: std::collections::HashMap<String, ElectorateDecl> = std::collections::HashMap::new();
    for (_, value) in crate::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
        let Some(decl) = decode_committed(&value) else { continue };
        match top.get(&decl.group) {
            Some(cur) if cur.epoch >= decl.epoch => {}
            _ => { top.insert(decl.group.clone(), decl); }
        }
    }
    for decl in top.values() { check_memory_for(ctx, decl); }
}

/// The ballot of a record's acceptance, `0` for none.
fn accepted_ballot(s: &AcceptorSlot) -> u64 {
    s.accepted.as_ref().map_or(0, |(b, _)| *b)
}

/// **The merge**, pure: the higher promise and whom it was promised to (an equal promise keeps `b`'s promisee — the
/// callers pass memory, the later answer, as `b`), the higher acceptance — and `promised` never below the acceptance
/// it carries (a malformed copy is clamped; accepting at a ballot is promising it).
pub(crate) fn merge_records(a: &AcceptorSlot, b: &AcceptorSlot) -> AcceptorSlot {
    let (pa, pb) = (accepted_ballot(a), accepted_ballot(b));
    let accepted = if pa > pb { a.accepted.clone() } else { b.accepted.clone() };
    let (promised, promised_to) = if a.promised > b.promised { (a.promised, a.promised_to) } else { (b.promised, b.promised_to) };
    let floor = accepted_ballot(&AcceptorSlot { promised: 0, promised_to: None, accepted: accepted.clone(), groups: Vec::new() });
    // The electorate groups the slot is tagged with (P2's migration) are the union: a tag is never removed.
    let mut groups = a.groups.clone();
    for g in &b.groups { if !groups.contains(g) { groups.push(Arc::clone(g)); } }
    groups.sort();
    AcceptorSlot { promised: promised.max(floor), promised_to, accepted, groups }
}

/// **The rule**: is `copy` — a peer's copy of this node's own record — an answer this node gave and does not hold?
/// Memory never lowers, so a promise above memory's is forgotten; and an acceptance the copy carries that memory
/// does not — at a ballot at or above memory's promise — is forgotten too (#605's confirming review, finding 2: memory
/// `{b to Y, none}` against copy `{b to X, (b, vX)}` is a vote at `b` this node cast and would cast again for another
/// value), **unless** memory's promise is the collector's floor (`FLOOR_SENTINEL`): a collected slot legitimately has an
/// older acceptance-bearing copy at exactly its ended ballot.
fn forgotten(copy: &AcceptorSlot, held: &AcceptorSlot) -> bool {
    if copy.promised > held.promised { return true; }
    let (ca, ha) = (accepted_ballot(copy), accepted_ballot(held));
    ca > ha && ca >= held.promised && held.promised_to != Some(FLOOR_SENTINEL)
}

/// Detection 3, over the leaf state: a peer's copy of this node's own record for `slot` against its memory. On
/// detection the slot is withheld and the **merged** record ([`merge_records`]) is returned for [`repair_own_record`]
/// to write; `None` otherwise.
fn compare_own_record(
    memory: &AcceptorMemory, withheld: &papaya::HashSet<Arc<str>>, slot: &str, value: &Bytes,
) -> Option<AcceptorSlot> {
    let copy = decode_acceptor(value)?;
    let held = memory.pin().get(slot).cloned().unwrap_or_default();
    if !forgotten(&copy, &held) { return None; }
    let copy_accepted = accepted_ballot(&copy);
    let merged = merge_records(&copy, &held);
    if withheld.pin().insert(Arc::from(slot)) {
        PROMISES_FORGOTTEN.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_acceptor_guard_total", "event" => "promise_forgotten").increment(1);
        tracing::warn!(slot, copy_promised = copy.promised, copy_accepted, held_promised = held.promised,
            "consensus: a peer holds this node's own acceptor record with a promise or acceptance above this node's \
             memory — an answer this node gave and has forgotten (a restart over an empty or older store). Withholding \
             every answer for the slot until the next start, and writing the merged record so that start reads it \
             (see mycelium::acceptor_promises_forgotten)");
    }
    Some(merged)
}

/// **Repairs this node's own durable record** for a withheld slot (#605's review, finding 1): writes the merged record
/// to `sys/consensus-accepted/{me}/{slot}` with a fresh HLC — applied, gossiped, handed to the WAL — under
/// `acceptor_records`, the lock every writer of this node's acceptor records takes (lock-order row 56; a leaf hold
/// across one synchronous store apply, never an `await`). **Memory is re-read under the lock and merged again**
/// (the confirming review's finding 1): `merged` was computed from a read outside it, and a listener promise at a
/// higher ballot can land — and its record be written and its ack sent — between that read and this write; the record
/// written here must not say less than any answer that is out. A Layer III write of a key this node owns, so the next
/// start's `prewarm_accepted` reads the forgotten promise and acceptance back even where this node's own empty-handed
/// write had won LWW over the copy. Memory itself is left as it is: the slot stays withheld until that start.
pub(crate) fn repair_own_record(ctx: &Arc<TaskCtx>, slot: &str, merged: &AcceptorSlot) {
    let engine = crate::agent::helpers::make_consensus_engine_ctx(ctx, false, false, 0, None);
    let _records = ctx.acceptor_records.lock().unwrap_or_else(|e| e.into_inner());
    let now = ctx.consensus_accepted.pin().get(slot).cloned().unwrap_or_default();
    let record = merge_records(merged, &now);
    let upd = engine.kv_set_returning(accepted_key(&ctx.node_id, slot), encode_acceptor(&record));
    engine.wal_try(&upd);
}

/// Whether this node withholds every answer for `slot` (detection 3). Read on every prepare and proposal; empty-set
/// fast path.
pub(crate) fn withheld(ctx: &TaskCtx, slot: &str) -> bool {
    let set = ctx.acceptor_withheld.pin();
    !set.is_empty() && set.contains(slot)
}

/// A proposer's outcome on a withheld slot: a timeout labelled `acceptor_withheld`, nothing more sent. Checked at the
/// door, at the top of every ballot and before the accept-phase claim (#605's review, finding 5), so a slot withheld
/// mid-proposal ends it at the next step; `ballots_tried` says how far it got.
pub(crate) fn withheld_timeout(slot: Arc<str>, ballots_tried: u32, quorum_required: usize) -> crate::consensus::ConsensusResult {
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_consensus_timeouts_total", "reason" => "acceptor_withheld").increment(1);
    tracing::warn!(slot = %slot, ballots_tried, "consensus: not proposing — this node withholds every answer for the slot \
        since a forgotten promise was detected; restart it over the repaired store");
    crate::consensus::ConsensusResult::Timeout { slot, ballots_tried, votes_last_ballot: 0, quorum_required }
}

/// Installs detection 3 (and detection 2's gossip half) on the core's inbound seam, once, before any connection.
/// The closure captures leaf state and a `Weak` context (for the repair write) — never a strong `Arc` to the context
/// that holds it — so a stopped agent is freed.
pub(crate) fn install(ctx: &Arc<TaskCtx>) {
    let me = ctx.node_id.clone();
    let memory = Arc::clone(&ctx.consensus_accepted);
    let withheld = Arc::clone(&ctx.acceptor_withheld);
    let notices = Arc::clone(&ctx.acceptor_notices);
    let weak = Arc::downgrade(ctx);
    let own_prefix = format!("{}{me}/", mycelium_core::signal::kv_ns::CONSENSUS_ACCEPTED);
    let committed_prefix = format!("{}{ELECTORATE_SLOT_PREFIX}", consensus_ns::COMMITTED);
    let prefixes = vec![own_prefix.clone(), committed_prefix.clone()];
    let observer = mycelium_core::context::InboundObserver {
        prefixes,
        on_inbound: Arc::new(move |update| {
            if update.is_tombstone { return; }
            if let Some(slot) = update.key.strip_prefix(own_prefix.as_str()) {
                if let Some(merged) = compare_own_record(&memory, &withheld, slot, &update.value)
                    && let Some(ctx) = weak.upgrade() {
                        repair_own_record(&ctx, slot, &merged);
                    }
            } else if update.key.starts_with(committed_prefix.as_str())
                && let Some(decl) = decode_committed(&update.value) {
                    check_memory(&memory, &notices, &me, &decl);
                }
        }),
    };
    let _ = ctx.inbound_observer.set(observer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{encode_acceptor, Accepted, AcceptorSlot};

    fn copy(promised: u64, accepted: Option<u64>) -> Bytes {
        encode_acceptor(&AcceptorSlot {
            promised, promised_to: Some(7),
            accepted: accepted.map(|b| (b, Accepted::DigestOnly([9; 32]))),
            groups: Vec::new(),
        })
    }

    /// The comparison that withholds: a copy above memory — a promise or an acceptance this node does not hold — and
    /// nothing else. An empty memory is below any promise; a shrunk record's floor is not above the memory it came
    /// from; a copy of what this node holds is nothing.
    #[test]
    fn a_copy_above_memory_withholds_the_slot_and_a_copy_at_or_below_it_does_not() {
        let memory: AcceptorMemory = papaya::HashMap::new();
        let withheld: papaya::HashSet<Arc<str>> = papaya::HashSet::new();
        let before = acceptor_promises_forgotten();
        assert!(compare_own_record(&memory, &withheld, "a", &copy(0, None)).is_none());
        assert!(withheld.pin().is_empty(), "a record of nothing is nothing");
        memory.pin().insert(Arc::from("b"), AcceptorSlot { promised: 5, promised_to: Some(1), accepted: Some((4, Accepted::DigestOnly([1; 32]))), groups: Vec::new() });
        assert!(compare_own_record(&memory, &withheld, "b", &copy(5, Some(4))).is_none());
        assert!(compare_own_record(&memory, &withheld, "b", &copy(3, Some(2))).is_none());
        assert!(withheld.pin().is_empty(), "a copy at or below memory is one this node wrote");
        // The merged record: the copy's higher promise (and promisee), memory's higher acceptance.
        let merged = compare_own_record(&memory, &withheld, "b", &copy(6, None)).expect("a promise above memory was forgotten");
        assert!(withheld.pin().contains("b"));
        assert_eq!((merged.promised, merged.promised_to, merged.accepted.as_ref().map(|(b, _)| *b)), (6, Some(7), Some(4)));
        assert!(compare_own_record(&memory, &withheld, "b", &copy(6, None)).is_some(), "the record is merged again; the slot was withheld already");
        assert_eq!(acceptor_promises_forgotten() - before, 1, "once per slot");
        memory.pin().insert(Arc::from("c"), AcceptorSlot { promised: 2, promised_to: Some(FLOOR_SENTINEL), accepted: None, groups: Vec::new() });
        assert!(compare_own_record(&memory, &withheld, "c", &copy(2, Some(2))).is_none());
        assert!(!withheld.pin().contains("c"), "an acceptance at a shrunk record's floor is one the collector shed");
        let merged = compare_own_record(&memory, &withheld, "c", &copy(3, Some(3))).expect("an acceptance (and its promise) above memory is forgotten");
        assert_eq!((merged.promised, merged.accepted.as_ref().map(|(b, _)| *b)), (3, Some(3)), "the copy's acceptance is the higher one");
        assert!(withheld.pin().contains("c"));
        assert!(compare_own_record(&memory, &withheld, "d", &copy(1, None)).is_some());
        assert!(withheld.pin().contains("d"), "an empty memory is below any promise");
        assert!(compare_own_record(&memory, &withheld, "e", &Bytes::from_static(b"garbage")).is_none());
        assert!(!withheld.pin().contains("e"), "a copy that does not decode is not compared");
    }

    /// **A forgotten acceptance at an equal ballot** (the confirming review's finding 2): memory `{b to Y, none}`
    /// against the copy `{b to X, (b, vX)}` is a vote this node cast at `b` and would cast again for Y's value — forgotten;
    /// the same copy against the collector's floor `{b, FLOOR_SENTINEL, none}` is a collected slot's legitimate older
    /// record — not. And the merge never leaves `promised` below the acceptance it carries.
    #[test]
    fn an_acceptance_at_the_promised_ballot_is_forgotten_unless_the_promise_is_the_collection_floor() {
        let held = AcceptorSlot { promised: 4, promised_to: Some(0xEE), accepted: None, groups: Vec::new() };
        let copy = AcceptorSlot { promised: 4, promised_to: Some(0xAA), accepted: Some((4, Accepted::DigestOnly([3; 32]))), groups: Vec::new() };
        assert!(forgotten(&copy, &held), "an acceptance at the promised ballot that memory lacks");
        let floor = AcceptorSlot { promised: 4, promised_to: Some(FLOOR_SENTINEL), accepted: None, groups: Vec::new() };
        assert!(!forgotten(&copy, &floor), "a collected slot's floor legitimately has an older acceptance-bearing copy");
        let below = AcceptorSlot { promised: 3, promised_to: Some(0xAA), accepted: Some((3, Accepted::DigestOnly([3; 32]))), groups: Vec::new() };
        assert!(!forgotten(&below, &held), "an acceptance below the promise is one memory may have shed");
        let same = AcceptorSlot { promised: 4, promised_to: Some(0xEE), accepted: Some((4, Accepted::DigestOnly([3; 32]))), groups: Vec::new() };
        assert!(!forgotten(&copy, &same), "an acceptance memory holds is nothing");
        let merged = merge_records(&copy, &held);
        assert_eq!((merged.promised, merged.promised_to, accepted_ballot(&merged)), (4, Some(0xEE), 4), "the merge keeps the later promisee and the acceptance");
        let malformed = AcceptorSlot { promised: 1, promised_to: Some(1), accepted: Some((6, Accepted::DigestOnly([3; 32]))), groups: Vec::new() };
        assert_eq!(merge_records(&malformed, &held).promised, 6, "promised is clamped to the acceptance it carries");
        assert!(forgotten(&malformed, &held));
    }
}
