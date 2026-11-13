//! The acceptor's decision functions, exported for the model in `mycelium-model` (critique plan row CR10).
//!
//! Gated by the `model-internals` cargo feature, which no shipped build enables, so a normal build does not
//! widen the public API. Every function here **wraps** a `pub(crate)` function of [`crate::consensus`] or
//! [`crate::consensus_life`] and adds no rule of its own: the model checker explores interleavings of the code
//! the engine runs, not of a copy. What the model cannot reach this way — the engine's methods that read the
//! store through a `TaskCtx` — it re-implements, and `mycelium-model`'s crate documentation lists each one.
//!
//! **Not stable.** Any item here can move or change shape between patch releases.

use std::sync::Arc;

use bytes::Bytes;

use crate::consensus::{
    claim_envelope, electorate_vote_filter, phase1_choice, prepare_slot, set_aside_finished, value_digest, Accepted,
    AcceptorMemory, AcceptorSlot, Carry, Phase1Choice, PrepareOutcome,
};
use crate::consensus_life::{self as life, Envelope, SlotView, Term};
use crate::node_id::NodeId;

/// One acceptor's memory of one slot — [`AcceptorSlot`] without the group tags, which only a migration's
/// manifest reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Acceptor {
    /// The highest ballot promised or accepted.
    pub promised:    u64,
    /// The proposer (`NodeId::id_hash`) it was promised to.
    pub promised_to: Option<u64>,
    /// The ballot and decision envelope most recently accepted.
    pub accepted:    Option<(u64, Bytes)>,
}

impl Acceptor {
    fn memory(&self, slot: &Arc<str>) -> AcceptorMemory {
        let memory = AcceptorMemory::new();
        memory.pin().insert(Arc::clone(slot), AcceptorSlot {
            promised:    self.promised,
            promised_to: self.promised_to,
            accepted:    self.accepted.as_ref().map(|(b, e)| (*b, Accepted::Envelope(e.clone()))),
            groups:      Vec::new(),
        });
        memory
    }

    fn read(memory: &AcceptorMemory, slot: &Arc<str>) -> Self {
        let state = memory.pin().get(slot).cloned().unwrap_or_default();
        Self {
            promised:    state.promised,
            promised_to: state.promised_to,
            accepted:    state.accepted.and_then(|(b, a)| a.value().map(|v| (b, v))),
        }
    }
}

/// What [`prepare`] answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    /// Promised; carries what this acceptor had accepted.
    Promised(Option<(u64, Bytes)>),
    /// Refused; carries the ballot this acceptor holds.
    Refused(u64),
}

/// [`prepare_slot`] over one acceptor's state: the state afterwards and the answer.
pub fn prepare(acceptor: &Acceptor, slot: &str, ballot: u64, proposer: u64, floor: u64) -> (Acceptor, Prepared) {
    let slot: Arc<str> = Arc::from(slot);
    let memory = acceptor.memory(&slot);
    let answer = match prepare_slot(&memory, &slot, ballot, proposer, floor) {
        PrepareOutcome::Promised(acc) => Prepared::Promised(acc.and_then(|(b, a)| a.value().map(|v| (b, v)))),
        PrepareOutcome::Refused { promised, .. } => Prepared::Refused(promised),
    };
    (Acceptor::read(&memory, &slot), answer)
}

/// [`claim_envelope`] over one acceptor's state: the state afterwards when the claim is granted.
pub fn accept(acceptor: &Acceptor, slot: &str, ballot: u64, envelope: &Bytes, proposer: u64, floor: u64) -> Option<Acceptor> {
    let slot: Arc<str> = Arc::from(slot);
    let memory = acceptor.memory(&slot);
    claim_envelope(&memory, &slot, ballot, envelope, proposer, floor).then(|| Acceptor::read(&memory, &slot))
}

/// What a proposer may propose after its promise quorum answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase1 {
    /// Keep the current value.
    Keep,
    /// Carry this envelope, accepted at this ballot.
    Adopt(u64, Bytes),
    /// Propose nothing.
    Blocked,
}

/// The end of the engine's prepare phase: the promise quorum's `reports` (ballot, accepted envelope) through
/// `Carry::report`, the acceptances at or below `ended` set aside (`set_aside_finished`), then [`phase1_choice`]
/// — which, for a proposal that renews nothing, is `choose_after_prepare`. `own` is the proposer's own envelope, `current` the one it carries.
pub fn phase1(slot: &str, own: &Bytes, current: &Bytes, reports: &[(u64, Bytes)], ended: u64) -> Phase1 {
    let Some(own) = Envelope::decode(own) else { return Phase1::Blocked };
    let carry = Carry { own, renewing: None, legacy_term: Term::Permanent };
    let mut reported = reports.iter()
        .map(|(b, e)| carry.report(slot, *b, Some(value_digest(e)), Some(e.clone()), true))
        .collect::<Vec<_>>();
    set_aside_finished(&mut reported, 0, false, ended);
    match phase1_choice(current, &reported, &carry) {
        Phase1Choice::Keep => Phase1::Keep,
        Phase1Choice::Adopt(b, v) => Phase1::Adopt(b, v),
        Phase1Choice::Blocked => Phase1::Blocked,
    }
}

/// A decision envelope's bytes ([`Envelope::new`], encoded): `lease` is `(ms, expires_at_ms)`, `None` permanent.
pub fn envelope(slot: &str, value: &[u8], lease: Option<(u64, u64)>, lineage: u64, proposer: &NodeId, token: u64) -> Bytes {
    let term = match lease {
        Some((ms, expires_at_ms)) => Term::Lease { ms, expires_at_ms },
        None => Term::Permanent,
    };
    Envelope::new(slot, Bytes::copy_from_slice(value), term, lineage, proposer.clone(), token).encode()
}

/// The top decision a node holds for a slot, as [`life::read_slot`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Top {
    /// The record's ballot.
    pub ballot:   u64,
    /// The decision envelope.
    pub envelope: Bytes,
    /// Released, or its lease is past on the reader's wall clock.
    pub ended:    bool,
}

/// [`life::read_slot`] over a store holding exactly these decision `records` (ballot, envelope) and a release
/// marker for each envelope in `released`, read at `wall_now_ms`. `None` when the node holds no record.
pub fn read_top(slot: &str, records: &[(u64, Bytes)], released: &[Bytes], wall_now_ms: u64) -> Option<Top> {
    let kv = crate::store::KvState::with_scoped_prefixes(0, &[life::LIFE_PREFIX]);
    let put = |key: String, value: Bytes| crate::store::apply_and_notify(&kv, &crate::framing::GossipUpdate {
        nonce: 1, sender: 1, ttl: 1, is_tombstone: false, timestamp: 1, key: Arc::from(key.as_str()), value,
    });
    if !records.is_empty() {
        put(life::sentinel_key(slot), Bytes::from_static(&life::SENTINEL));
    }
    for (ballot, env) in records {
        put(life::record_key(slot, *ballot), env.clone());
    }
    for env in released.iter().filter_map(|e| Envelope::decode(e)) {
        put(life::marker_key(slot, env.lineage, &env.proposer), life::encode_marker(&env));
    }
    match life::read_slot(&kv, slot, wall_now_ms) {
        SlotView::Top(top) => Some(Top { ballot: top.ballot, envelope: top.env.encode(), ended: top.ended }),
        SlotView::NoRecord | SlotView::Unknown => None,
    }
}

/// The voters a proposal counts (`electorate_vote_filter`): the electorate's roster, narrowed by a trust slice.
/// `None` is no filter.
pub fn vote_filter(trust: Option<&[u64]>, roster: Option<&[u64]>) -> Option<Vec<u64>> {
    let set = |s: &[u64]| s.iter().copied().collect();
    let mut out: Vec<u64> = electorate_vote_filter(trust.map(set), roster.map(set))?
        .into_iter().collect();
    out.sort_unstable();
    Some(out)
}
