//! **One record per decision** — the lifecycle of a lock, a lease and a leadership (row A of the
//! post-360 plan; design record `docs/design/lock-lifecycle.md`).
//!
//! A decision used to live in three keys merged by last-writer-wins on HLC — `committed`, `lease`
//! and `decided` — which travel independently, so a node could hold any combination of their versions
//! and every rule reading *the pair* (value, window) had a combination it read wrong (three review
//! rounds on PR #600). Here the decision and its lifecycle are one immutable **envelope**:
//!
//! - it is the value Paxos decides, so every acceptance, promise report and adoption carries the
//!   window, the lineage, the original proposer and the fencing token with it;
//! - it is written under `consensus/life/{escaped slot}/{ballot:016x}` — one key per decided ballot,
//!   the same bytes from every writer, so last-writer-wins never orders two decisions; a reader takes
//!   the **highest ballot** it holds;
//! - a release is a separate marker key, `…/end/{lineage}-{proposer}`, which no record write can
//!   remove.
//!
//! Layer I learns nothing: these are ordinary keys, merged as every key is.

use crate::node_id::NodeId;
use crate::store::KvState;
use bytes::Bytes;

/// The namespace of decision records, release markers and content-addressed values.
pub(crate) const LIFE_PREFIX: &str = "consensus/life/";

/// The largest value a decision record carries inline; above it the record carries the digest and the
/// value lives under `v/{digest}`, content-addressed (design §2.5).
pub(crate) const LIFE_VALUE_CAP: usize = 4096;

/// Envelope magic: `L1`.
const MAGIC: [u8; 2] = [0x4C, 0x31];
/// The slot's **sentinel** (`…/s`): written, with the same bytes, by every writer of a decision record
/// for the slot and never collected, so a slot that has ever had a record is never read the legacy
/// way, even after its lower records are tombstoned and swept (design §2.3, §8).
pub(crate) const SENTINEL: [u8; 2] = [0x4C, 0x53];

/// How long a decision holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Term {
    /// No window; only a release ends it.
    Permanent,
    /// Ends when the reader's **wall clock** passes `expires_at_ms` — fixed by the original proposer's
    /// wall clock at the first proposal (design §2.3, review finding 4: HLC drift can only lengthen a
    /// lease, never end it early).
    Lease { ms: u64, expires_at_ms: u64 },
}

/// The decision envelope — what Paxos decides (design §2.1).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Envelope {
    /// The slot it decides — inside the digest, so an envelope cannot be replayed into another slot.
    pub(crate) slot:         String,
    pub(crate) term:         Term,
    /// The first ballot of the lineage: the proposal's own first ballot, or the lineage it renews.
    pub(crate) lineage:      u64,
    /// The original proposer.
    pub(crate) proposer:     NodeId,
    /// The fencing token, fixed at the first proposal.
    pub(crate) token:        u64,
    /// SHA-256 of the value.
    pub(crate) value_digest: [u8; 32],
    /// The value — `None` only in a stored record whose value is content-addressed (`v/{digest}`).
    pub(crate) value:        Option<Bytes>,
}

impl Envelope {
    /// A fresh envelope for `value`.
    pub(crate) fn new(slot: &str, value: Bytes, term: Term, lineage: u64, proposer: NodeId, token: u64) -> Self {
        Self {
            slot: slot.to_string(), term, lineage, proposer, token,
            value_digest: crate::consensus::value_digest(&value), value: Some(value),
        }
    }

    /// Wrap a **legacy** acceptance (a raw value accepted from a proposer older than row A) in an
    /// envelope with the adopter's term — the one place an adoption cannot keep the original holder's
    /// window, because the legacy acceptance never carried one (design §4, §7).
    pub(crate) fn wrap_legacy(slot: &str, value: Bytes, term: Term, adopter: NodeId, token: u64, lineage: u64) -> Self {
        Self::new(slot, value, term, lineage, adopter, token)
    }

    pub(crate) fn encode(&self) -> Bytes {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&mycelium_core::serde_fixint::to_vec(self).unwrap_or_default());
        Bytes::from(v)
    }

    /// `None` for anything that is not a well-formed envelope, or whose inline value does not match its
    /// digest.
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 || bytes[..2] != MAGIC { return None; }
        let env: Envelope = mycelium_core::serde_fixint::from_slice(&bytes[2..]).ok()?;
        if let Some(v) = &env.value
            && crate::consensus::value_digest(v) != env.value_digest {
                return None;
            }
        Some(env)
    }

    /// The form stored under the record key: the value moved out when it is over the cap.
    pub(crate) fn stored(&self) -> (Bytes, Option<Bytes>) {
        match &self.value {
            Some(v) if v.len() > LIFE_VALUE_CAP => {
                let mut stripped = self.clone();
                stripped.value = None;
                (stripped.encode(), Some(v.clone()))
            }
            _ => (self.encode(), None),
        }
    }

    /// The identity a release marker names: the value, the original proposer and the lineage. An
    /// adoption and a renewal keep all three, so a release ends both (design §4, §5).
    pub(crate) fn identity(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"mycelium.consensus/life-identity/1");
        h.update(self.slot.as_bytes());
        h.update([0u8]);
        h.update(self.value_digest);
        h.update(self.proposer.to_string().as_bytes());
        h.update([0u8]);
        h.update(self.lineage.to_le_bytes());
        h.finalize().into()
    }

    /// The fencing token's wall-clock expiry, if leased.
    pub(crate) fn expires_at_ms(&self) -> Option<u64> {
        match self.term { Term::Lease { expires_at_ms, .. } => Some(expires_at_ms), Term::Permanent => None }
    }
}

/// `slot` with `%` and `/` percent-encoded, so the escaped form contains no `/` and the record prefix
/// identifies exactly one slot (review finding 8: `lock/a/<16hex>` could otherwise inject a top record
/// into `lock/a`). Injective.
pub(crate) fn escape_slot(slot: &str) -> String {
    let mut out = String::with_capacity(slot.len());
    for c in slot.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn slot_prefix(slot: &str) -> String {
    format!("{LIFE_PREFIX}{}/", escape_slot(slot))
}

/// The scope-index outer key of `slot`'s records (`slot_prefix` without its trailing `/`).
fn scope_of(slot: &str) -> String {
    format!("{LIFE_PREFIX}{}", escape_slot(slot))
}

pub(crate) fn sentinel_key(slot: &str) -> String {
    format!("{}s", slot_prefix(slot))
}

pub(crate) fn record_key(slot: &str, ballot: u64) -> String {
    format!("{}{ballot:016x}", slot_prefix(slot))
}

pub(crate) fn marker_key(slot: &str, lineage: u64, proposer: &NodeId) -> String {
    format!("{}end/{lineage:016x}-{:016x}", slot_prefix(slot), proposer.id_hash())
}

pub(crate) fn value_key(slot: &str, digest: &[u8; 32]) -> String {
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}v/{hex}", slot_prefix(slot))
}

/// A release marker's value: the identity it ends, and the term the collector needs — `0` permanent,
/// `1` lease, then the lease's expiry.
pub(crate) fn encode_marker(env: &Envelope) -> Bytes {
    let mut v = env.identity().to_vec();
    match env.term {
        Term::Permanent => { v.push(0); v.extend_from_slice(&0u64.to_le_bytes()); }
        Term::Lease { expires_at_ms, .. } => { v.push(1); v.extend_from_slice(&expires_at_ms.to_le_bytes()); }
    }
    Bytes::from(v)
}

/// `(identity, Some(expiry) for a lease / None for permanent)`.
pub(crate) fn decode_marker(bytes: &[u8]) -> Option<([u8; 32], Option<u64>)> {
    if bytes.len() != 41 { return None; }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes[..32]);
    let exp = u64::from_le_bytes(bytes[33..41].try_into().ok()?);
    match bytes[32] { 0 => Some((id, None)), 1 => Some((id, Some(exp))), _ => None }
}

/// The latest decision this node holds for a slot.
#[derive(Clone, Debug)]
pub(crate) struct Top {
    pub(crate) ballot: u64,
    pub(crate) env:    Envelope,
    /// The value, resolved from the record or its content-addressed key; `None` until it arrives.
    pub(crate) value:  Option<Bytes>,
    /// Released (a marker for its identity) or its lease is past on the reader's wall clock.
    pub(crate) ended:  bool,
}

/// What this node knows of a slot.
#[derive(Clone, Debug)]
pub(crate) enum SlotView {
    /// No record key at all: a slot never decided under row A — the legacy reading applies (design §2.3).
    NoRecord,
    /// The slot has had records (its sentinel is here) but no record is held: the latest decision has
    /// not arrived here. Reads as not live; never falls back to the legacy reading.
    Unknown,
    /// The highest-ballot decision record held.
    Top(Box<Top>),
}

/// Every valid key under the slot's prefix that holds data: `(remainder, bytes)`. Remainders that are
/// not exactly a record, a marker or a value are ignored (finding 8).
fn scan(kv: &KvState, slot: &str) -> Vec<(String, Bytes)> {
    let prefix = slot_prefix(slot);
    // O(keys of this slot): the store files `consensus/life/{esc}/…` under one scope (review D3).
    mycelium_core::store::scan_scope(kv, &scope_of(slot))
        .into_iter()
        .filter_map(|(k, v)| k.strip_prefix(prefix.as_str()).map(|r| (r.to_string(), v)))
        .collect()
}

/// Exactly 16 **lowercase** hex digits — the one spelling `record_key` writes (review D7).
fn parse_hex_u64(s: &str) -> Option<u64> {
    (s.len() == 16 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| u64::from_str_radix(s, 16).ok()).flatten()
}

/// Read the slot (design §2.3). `wall_now_ms` is this node's wall clock.
pub(crate) fn read_slot(kv: &KvState, slot: &str, wall_now_ms: u64) -> SlotView {
    let entries = scan(kv, slot);
    let mut any_record = false;
    let mut best: Option<(u64, Envelope)> = None;
    for (rem, bytes) in &entries {
        if rem == "s" && bytes.as_ref() == SENTINEL { any_record = true; continue; }
        let Some(ballot) = parse_hex_u64(rem) else { continue };
        any_record = true;
        let Some(env) = Envelope::decode(bytes) else { continue };
        if env.slot != slot { continue; }
        if best.as_ref().is_none_or(|(b, _)| ballot > *b) {
            best = Some((ballot, env));
        }
    }
    let Some((ballot, env)) = best else {
        return if any_record { SlotView::Unknown } else { SlotView::NoRecord };
    };
    let value = env.value.clone().or_else(|| {
        let key = value_key(slot, &env.value_digest);
        let rem = key.strip_prefix(slot_prefix(slot).as_str())?.to_string();
        entries.iter().find(|(r, _)| *r == rem)
            .map(|(_, v)| v.clone())
            .filter(|v| crate::consensus::value_digest(v) == env.value_digest)
    });
    let marker_rem = marker_key(slot, env.lineage, &env.proposer);
    let marker_rem = marker_rem.strip_prefix(slot_prefix(slot).as_str()).unwrap_or_default().to_string();
    let released = entries.iter()
        .find(|(r, _)| *r == marker_rem)
        .and_then(|(_, v)| decode_marker(v))
        .is_some_and(|(id, _)| id == env.identity());
    let lapsed = env.expires_at_ms().is_some_and(|exp| wall_now_ms > exp);
    SlotView::Top(Box::new(Top { ballot, ended: released || lapsed, env, value }))
}

/// Every slot that has decision-record keys on this node, with the remainders under it — for the
/// record collector. Sorted by slot (review round 3, finding 6: replay must select the same slots).
pub(crate) fn slots_with_records(kv: &KvState) -> Vec<String> {
    let mut slots: Vec<String> = mycelium_core::store::scopes_under(kv, LIFE_PREFIX)
        .into_iter()
        .filter_map(|outer| unescape_slot(outer.strip_prefix(LIFE_PREFIX)?))
        .collect();
    slots.sort();
    slots.dedup();
    slots
}

fn unescape_slot(esc: &str) -> Option<String> {
    let mut out = String::with_capacity(esc.len());
    let mut chars = esc.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let code: String = chars.by_ref().take(2).collect();
            match code.as_str() { "25" => out.push('%'), "2F" => out.push('/'), _ => return None }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// What the record collector may do with one slot (design §8): tombstone the decision records below
/// the top (the sweep then removes them), their content-addressed values unless the top shares one, and
/// **lease** markers whose expiry plus the drift bound has passed and whose lineage is not the top's.
/// Never the top record, the sentinel, a permanent marker or a marker of the top's lineage. Keys come
/// back **sorted**, so the writes are replay-deterministic (review D7).
pub(crate) struct Collectable {
    pub(crate) dead_keys: Vec<String>,
}

pub(crate) fn collectable(kv: &KvState, slot: &str, wall_now_ms: u64, drift_ms: u64) -> Option<Collectable> {
    let SlotView::Top(top) = read_slot(kv, slot, wall_now_ms) else { return None };
    let prefix = slot_prefix(slot);
    let top_marker = marker_key(slot, top.env.lineage, &top.env.proposer);
    let mut dead_keys = Vec::new();
    for (rem, bytes) in scan(kv, slot) {
        if let Some(b) = parse_hex_u64(&rem) {
            if b < top.ballot {
                if let Some(env) = Envelope::decode(&bytes)
                    && env.value.is_none() && env.value_digest != top.env.value_digest {
                        dead_keys.push(value_key(slot, &env.value_digest));
                    }
                dead_keys.push(format!("{prefix}{rem}"));
            }
        } else if rem.starts_with("end/")
            && let Some((_, Some(exp))) = decode_marker(&bytes)
            && wall_now_ms > exp.saturating_add(drift_ms)
            && format!("{prefix}{rem}") != top_marker {
                dead_keys.push(format!("{prefix}{rem}"));
            }
    }
    dead_keys.sort();
    dead_keys.dedup();
    Some(Collectable { dead_keys })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(p: u16) -> NodeId { NodeId::new("127.0.0.1", p).unwrap() }

    #[test]
    fn an_envelope_round_trips_and_binds_its_value() {
        let e = Envelope::new("lock/x", Bytes::from_static(b"v"), Term::Lease { ms: 5, expires_at_ms: 9 }, 3, nid(1), 7);
        assert_eq!(Envelope::decode(&e.encode()), Some(e.clone()));
        let mut bad = e.clone();
        bad.value = Some(Bytes::from_static(b"w"));
        assert_eq!(Envelope::decode(&bad.encode()), None, "a value that does not match its digest");
        assert_eq!(Envelope::decode(b"v"), None);
    }

    /// Review finding 8: an escaped slot contains no `/`, so `lock/a/<16hex>` is a different prefix
    /// from `lock/a`'s, and the escaping is injective.
    #[test]
    fn slot_escaping_separates_nested_slot_names() {
        assert_eq!(escape_slot("lock/a"), "lock%2Fa");
        assert!(!slot_prefix("lock/a/0000000000000009").starts_with(&slot_prefix("lock/a")));
        assert_ne!(escape_slot("a%2Fb"), escape_slot("a/b"));
        assert_eq!(unescape_slot(&escape_slot("x/%y")).as_deref(), Some("x/%y"));
    }

    #[test]
    fn a_marker_round_trips() {
        let e = Envelope::new("s", Bytes::from_static(b"v"), Term::Permanent, 1, nid(1), 1);
        assert_eq!(decode_marker(&encode_marker(&e)), Some((e.identity(), None)));
        let l = Envelope::new("s", Bytes::from_static(b"v"), Term::Lease { ms: 1, expires_at_ms: 77 }, 1, nid(1), 1);
        assert_eq!(decode_marker(&encode_marker(&l)), Some((l.identity(), Some(77))));
    }

    #[test]
    fn a_large_value_is_stored_content_addressed() {
        let big = Bytes::from(vec![7u8; LIFE_VALUE_CAP + 1]);
        let e = Envelope::new("s", big.clone(), Term::Permanent, 1, nid(1), 1);
        let (rec, external) = e.stored();
        assert_eq!(external, Some(big));
        assert_eq!(Envelope::decode(&rec).and_then(|r| r.value), None);
    }
}
