//! **A history source that establishes its own coverage** — realignment repairs A3, decision D4, the half
//! 2.24.0 did not build (`docs/plans/realignment-repairs.md` §3.4: *"the signed-heads reader supplies
//! continuity from its checkpoint onward and the availability of what the heads reference, never earlier
//! history"*).
//!
//! [`eligibility::eligible_strict`](super::eligibility::eligible_strict) decides each incumbency rule from a
//! [`TermHistory`], and a `TermHistory` is only as good as whoever built it. Here the source is an
//! **appointment stream** in the knowledge layer — one `(issuer, stream)` the appointing authority
//! publishes — and coverage comes only from what this node can verify about it:
//!
//! - **The head is the reader's own.** The walk starts at the [`Checkpoint`] this node's reader holds for
//!   the pinned `(issuer, stream)` — [`HeadCheckpoints::checkpoint`](crate::knowledge::heads::HeadCheckpoints::checkpoint)
//!   or the durable reader's — never at whatever head a presenter offers. Heads newer than the checkpoint
//!   are ignored; a presenter cannot make the history look more current, or splice in another stream.
//! - **Continuity is the `prev` chain.** From that head the walk follows each head's `prev` digest back
//!   through the presented heads, authenticating each against the pinned issuer's keys in this node's
//!   view: the checkpoint head under a key held as current, each older head under any key the issuer is
//!   known to have held — a `prev` digest a later signature committed to already fixes it. So trust rests on
//!   the current-key head's commitment: a publisher must chain each head from its *own* previous head, never
//!   from one it received. Revoking a compromised key does not end the history **for a reader whose
//!   checkpoint is already on a current-key head**; a reader whose checkpoint is under the revoked key, or
//!   that must advance across a revoked head, vouches for nothing until it holds a current-key checkpoint
//!   (the reader's `offer` refuses revoked intermediates — knowledge K2). Order, duplicates and unrelated
//!   heads in the presented set cannot matter: only the linked chain is read. A stream the reader has seen
//!   **fork since it was opened** vouches for nothing ([`AppointmentStream::from_reader`] /
//!   [`AppointmentStream::from_parts`] read that); the knowledge layer holds forks in memory, so a restart
//!   forgets one.
//! - **Availability.** A linked head whose appointment record cannot be fetched is a **gap**; only the
//!   terms after the last gap are vouched for. The stream must carry one appointment per head: `records`
//!   must answer `None` for a record that is not an appointment (a gap), and a head repeating an earlier
//!   term breaks the chain rather than counting it twice — so re-publishing a term (an amended end time
//!   under the same id) collapses coverage to what follows it.
//! - **The origin.** The walk reaching a head with `prev: None` is the role's genesis. Reaching the head
//!   a caller's [`StreamOrigin::Baseline`] names (by digest) binds that baseline there; a baseline taken at
//!   the checkpoint itself binds nothing (no term after it to carry it), so take it one head back. Anything
//!   else — a missing link, a head that fails to authenticate — leaves the origin unknown. A baseline's
//!   `after` and totals are the caller's: the source checks only where it attaches.
//!
//! What it does not claim: that the appointing authority published every appointment there was, or that
//! this node's reader has heard the authority's newest head. "Through the head" means through **this
//! reader's checkpoint**: a node whose reader has not yet been offered the newest head decides as of the
//! head it holds. `records` must return only a record whose content matches the [`RecordId`] (the
//! knowledge store's content-addressed read does).

use super::eligibility::{ChainedTerm, Origin, TermHistory};
use super::handover::TermRecord;
use super::{PrincipalId, TermId};
use crate::knowledge::heads::{Checkpoint, SignedHead};
use crate::knowledge::issuer::{verify_signed_by, Authenticity, MemberKeySource, TrustedExternalIssuers};
use crate::knowledge::{IssuerId, RecordId};
use std::collections::HashMap;

/// Which stream, and the head this node's reader holds for it. Built by [`from_reader`](Self::from_reader)
/// or [`from_parts`](Self::from_parts), which read the fork state with the checkpoint — not by literal, where
/// `forked: false` could be written by hand.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AppointmentStream<'a> {
    /// The appointing authority — every head and every record it points at must be this issuer's.
    pub issuer: &'a IssuerId,
    /// The stream it publishes appointments on.
    pub stream: &'a str,
    /// This node's reader's checkpoint for `(issuer, stream)`; `None` if it holds none.
    pub checkpoint: Option<Checkpoint>,
    /// Whether the reader has recorded a fork on `(issuer, stream)` — then nothing is vouched for.
    pub forked: bool,
}

impl<'a> AppointmentStream<'a> {
    /// The pinned stream as `reader` holds it: its checkpoint, and whether it has seen a fork there.
    pub fn from_reader<S: crate::knowledge::heads::CheckpointStore>(
        reader: &crate::knowledge::heads::HeadCheckpoints<S>,
        issuer: &'a IssuerId,
        stream: &'a str,
    ) -> Self {
        Self {
            issuer,
            stream,
            checkpoint: reader.checkpoint(issuer, stream),
            forked: reader.forks().iter().any(|f| &f.issuer == issuer && f.stream == stream),
        }
    }

    /// The same from a reader's parts — for the durable reader, whose `checkpoint` and `forks` are exposed
    /// separately ([`DurableHeadCheckpoints`](crate::knowledge::durable::DurableHeadCheckpoints)).
    pub fn from_parts(
        issuer: &'a IssuerId,
        stream: &'a str,
        checkpoint: Option<Checkpoint>,
        forks: &[crate::knowledge::heads::ForkRecord],
    ) -> Self {
        Self {
            issuer,
            stream,
            checkpoint,
            forked: forks.iter().any(|f| &f.issuer == issuer && f.stream == stream),
        }
    }
}

/// What the caller trusts about the role before the earliest head the walk can reach.
#[derive(Clone, Debug)]
pub enum StreamOrigin {
    /// Nothing: earlier history is unknown unless the walk reaches the stream's first head.
    Unknown,
    /// A baseline taken **at a head**: the term that head recorded, and each principal's cumulative
    /// tenure through it. It binds only if the walk links to exactly that head's digest.
    Baseline {
        /// [`Head::digest`](crate::knowledge::store::Head::digest) of the head the baseline was taken at.
        at: [u8; 32],
        /// The term that head recorded — the one the baseline counts through.
        after: TermId,
        /// Each principal's cumulative tenure through `after`.
        cumulative_ms: HashMap<PrincipalId, u64>,
    },
}

/// Build the [`TermHistory`] an appointment stream supports. `presented` is any set of heads — the
/// source's, a presenter's, in any order; only those linked from the reader's checkpoint are read.
pub fn history_from_appointment_stream(
    stream: &AppointmentStream<'_>,
    presented: &[SignedHead],
    records: impl Fn(&RecordId) -> Option<TermRecord>,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
    origin: StreamOrigin,
) -> TermHistory {
    let unknown_head = TermId::new("<no head held for this stream>").expect("non-empty");
    let Some(cp) = stream.checkpoint else {
        return TermHistory::from_chain(Vec::new(), Origin::Unknown, &unknown_head);
    };
    if stream.forked {
        // The authority equivocated on this stream: which branch is its history is not this node's to pick.
        return TermHistory::from_chain(Vec::new(), Origin::Unknown, &unknown_head);
    }
    // Every presented copy of each digest on the pinned stream; signatures are checked only along the walk,
    // so the cost is bounded by the chain, and one bad copy cannot hide a good one.
    let mut by_digest: HashMap<[u8; 32], Vec<&SignedHead>> = HashMap::new();
    for sh in presented {
        if sh.head.issuer == *stream.issuer && sh.head.stream == stream.stream && sh.head.record.issuer == *stream.issuer {
            by_digest.entry(sh.head.digest()).or_default().push(sh);
        }
    }
    // The checkpoint head must verify under a key this node holds as current. An older head is reached only
    // through a `prev` digest a later authority signature committed to, so the hash chain already fixes it;
    // its own signature need only be attributable — a key since revoked still says who signed, and refusing
    // it would end every role's history at the revocation (rotation never did: retained keys stay current).
    let verified = |sh: &SignedHead, newest: bool| -> bool {
        let a = verify_signed_by(stream.issuer, &sh.head.canonical_bytes(), &sh.signature, members, external);
        if newest { matches!(a, Authenticity::Current { .. }) } else { a.is_attributable() }
    };

    // Walk back from the checkpoint along `prev`, newest first.
    let mut linked: Vec<&SignedHead> = Vec::new();
    let mut next = Some(cp.digest);
    let mut reached = None; // how the walk ended: Some(Genesis) / Some(Baseline) / None (unknown)
    while let Some(d) = next {
        if let StreamOrigin::Baseline { at, .. } = &origin
            && !linked.is_empty()
            && d == *at
        {
            reached = Some(true);
            break;
        }
        let newest = linked.is_empty();
        let Some(sh) = by_digest.get(&d).and_then(|copies| copies.iter().copied().find(|sh| verified(sh, newest))) else {
            break;
        };
        if linked.last().is_some_and(|child| sh.head.seq >= child.head.seq) {
            break; // not a descending chain
        }
        linked.push(sh);
        match sh.head.prev {
            None => {
                reached = Some(false);
                break;
            }
            Some(p) => next = Some(p),
        }
    }
    if linked.is_empty() {
        return TermHistory::from_chain(Vec::new(), Origin::Unknown, &unknown_head);
    }
    linked.reverse(); // oldest first

    // Terms after the last gap: a head whose record is unavailable breaks the chain there — and so does a
    // record repeating a term already seen (a head re-pointed at an earlier appointment would count it twice).
    let mut resolved: Vec<Option<TermRecord>> = linked.iter().map(|sh| records(&sh.head.record)).collect();
    let mut seen = std::collections::HashSet::new();
    for r in resolved.iter_mut() {
        if let Some(t) = r
            && !seen.insert(t.term.clone())
        {
            *r = None;
        }
    }
    let last_gap = resolved.iter().rposition(Option::is_none);
    let head_term = match resolved.last() {
        Some(Some(r)) => r.term.clone(),
        _ => TermId::new("<the head's appointment record is unavailable>").expect("non-empty"),
    };
    let start = last_gap.map_or(0, |g| g + 1);
    let whole = last_gap.is_none();
    let mut terms: Vec<ChainedTerm> = Vec::new();
    for r in resolved.into_iter().skip(start).flatten() {
        let previous = terms.last().map(|t: &ChainedTerm| t.record.term.clone());
        terms.push(ChainedTerm { record: r, previous });
    }
    // The origin holds only when nothing is missing between it and the head.
    let origin = match (whole, reached, origin) {
        (true, Some(false), _) => Origin::Genesis,
        (true, Some(true), StreamOrigin::Baseline { after, cumulative_ms, .. }) => {
            if let Some(first) = terms.first_mut() {
                first.previous = Some(after.clone());
            }
            Origin::Baseline { after, cumulative_ms }
        }
        _ => Origin::Unknown,
    };
    TermHistory::from_chain(terms, origin, &head_term)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::heads::{HeadCheckpoints, HeadVerdict, MemoryCheckpointStore};
    use crate::knowledge::issuer::MemberKeys;
    use crate::knowledge::store::Head;
    use crate::mandate::eligibility::{eligible_strict, RuleVerdict};
    use crate::mandate::handover::{IncumbencyRules, Ineligible};
    use crate::mandate::PrincipalId;
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;

    const DAY: u64 = 86_400_000;

    struct Authority {
        sk: SigningKey,
        issuer: IssuerId,
        members: HashMap<NodeId, MemberKeys>,
        ext: TrustedExternalIssuers,
    }

    fn authority() -> Authority {
        let sk = SigningKey::from_bytes(&[11u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        let n = NodeId::new("127.0.0.1", 7311).unwrap();
        let members = HashMap::from([(n.clone(), MemberKeys { retained: vec![pk], ..Default::default() })]);
        Authority { sk, issuer: IssuerId::for_node(&n), members, ext: TrustedExternalIssuers::new() }
    }

    fn pid(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }
    fn tid(n: u64) -> TermId {
        TermId::new(format!("term-{n}")).unwrap()
    }

    /// Terms `from..=to`, a day each, `holder(n)` holding term n; the signed heads (head seq = term n,
    /// chained; the first has `prev: None` only when `from == 1`) and the records they point at.
    fn stream(
        a: &Authority,
        from: u64,
        to: u64,
        holder: impl Fn(u64) -> &'static str,
        sign_with: Option<&SigningKey>,
    ) -> (Vec<SignedHead>, HashMap<RecordId, TermRecord>) {
        let mut heads: Vec<SignedHead> = Vec::new();
        let mut records = HashMap::new();
        for n in from..=to {
            let record_id = RecordId { issuer: a.issuer.clone(), digest: [n as u8; 32] };
            records.insert(
                record_id.clone(),
                TermRecord { holder: pid(holder(n)), term: tid(n), started_ms: n * DAY, ended_ms: (n + 1) * DAY },
            );
            let prev = match heads.last() {
                Some(p) => Some(p.head.digest()),
                None if n == 1 => None,
                None => Some([0xEE; 32]), // a predecessor this source does not hold
            };
            let head = Head { issuer: a.issuer.clone(), stream: "appointments/curator".into(), record: record_id, seq: n, prev };
            let sk = sign_with.unwrap_or(&a.sk);
            let signature = mycelium_core::tls::sign_bytes(sk, &head.canonical_bytes()).to_vec();
            heads.push(SignedHead { head, signature });
        }
        (heads, records)
    }

    /// A reader that has been offered the stream's heads in order, as gossip would — its checkpoint is
    /// the newest it verified.
    fn reader_through(a: &Authority, heads: &[SignedHead]) -> HeadCheckpoints<MemoryCheckpointStore> {
        let mut reader = HeadCheckpoints::open(MemoryCheckpointStore::new()).unwrap();
        for h in heads {
            let v = reader.offer(h, heads, &a.members, &a.ext);
            assert!(matches!(v, HeadVerdict::Advanced { .. }), "{v:?}");
        }
        reader
    }

    fn history_at(
        a: &Authority,
        reader: &HeadCheckpoints<MemoryCheckpointStore>,
        presented: &[SignedHead],
        records: &HashMap<RecordId, TermRecord>,
        origin: StreamOrigin,
    ) -> TermHistory {
        let stream = AppointmentStream::from_reader(reader, &a.issuer, "appointments/curator");
        history_from_appointment_stream(&stream, presented, |r| records.get(r).cloned(), &a.members, &a.ext, origin)
    }

    fn history(a: &Authority, heads: &[SignedHead], records: &HashMap<RecordId, TermRecord>, origin: StreamOrigin) -> TermHistory {
        history_at(a, &reader_through(a, heads), heads, records, origin)
    }

    /// The plan's witness: correctly signed heads with one required record missing → `Unknown`; the
    /// record restored → a real verdict.
    #[test]
    fn a_missing_appointment_record_is_unknown_and_its_return_decides() {
        let a = authority();
        // ada holds every odd term of 1..=15: eight days against an eight-day limit.
        let (heads, mut records) = stream(&a, 1, 16, |n| if n % 2 == 1 { "ada" } else { "bo" }, None);
        let rules = IncumbencyRules { max_cumulative_ms: Some(8 * DAY), ..IncumbencyRules::default() };
        let full = records.clone();
        records.retain(|_, t| t.term != tid(7));
        let gapped = history(&a, &heads, &records, StreamOrigin::Unknown);
        assert!(matches!(eligible_strict(&rules, &gapped, &pid("ada"), 17 * DAY).cumulative, Some(RuleVerdict::Unknown(_))));
        let whole = history(&a, &heads, &full, StreamOrigin::Unknown);
        assert!(whole.coverage().from_genesis, "the stream's first head is the role's genesis");
        assert!(matches!(
            eligible_strict(&rules, &whole, &pid("ada"), 17 * DAY).cumulative,
            Some(RuleVerdict::Ineligible(Ineligible::CumulativeTenure { .. }))
        ));
    }

    /// The plan's second witness: terms 20–30 verified decide consecutive terms; cumulative tenure stays
    /// `Unknown` without a baseline — the reader vouches for nothing before its first checkpoint.
    #[test]
    fn a_stream_held_from_term_20_decides_consecutive_terms_but_not_cumulative_tenure() {
        let a = authority();
        let (heads, records) = stream(&a, 20, 30, |n| if n >= 29 { "ada" } else { "bo" }, None);
        let rules = IncumbencyRules { max_consecutive_terms: Some(3), max_cumulative_ms: Some(10 * DAY), ..IncumbencyRules::default() };
        let h = history(&a, &heads, &records, StreamOrigin::Unknown);
        assert!(!h.coverage().from_genesis, "the stream's earliest held head is not its first");
        let e = eligible_strict(&rules, &h, &pid("ada"), 40 * DAY);
        assert_eq!(e.consecutive, Some(RuleVerdict::Eligible));
        assert!(matches!(e.cumulative, Some(RuleVerdict::Unknown(_))));
    }

    /// The source's own property: a head the reader does not authenticate contributes nothing. Here every
    /// head is signed by a key the authority does not hold, so nothing is vouched for.
    #[test]
    fn heads_signed_by_a_key_the_authority_does_not_hold_vouch_for_nothing() {
        let a = authority();
        let stranger = SigningKey::from_bytes(&[99u8; 32]);
        let (heads, records) = stream(&a, 1, 6, |_| "bo", Some(&stranger));
        let rules = IncumbencyRules { max_cumulative_ms: Some(30 * DAY), ..IncumbencyRules::default() };
        // Even handed a checkpoint naming the last of them, nothing authenticates.
        let stream_ = AppointmentStream {
            issuer: &a.issuer,
            stream: "appointments/curator",
            checkpoint: Some(Checkpoint { seq: 6, digest: heads[5].head.digest() }),
            forked: false,
        };
        let h = history_from_appointment_stream(&stream_, &heads, |r| records.get(r).cloned(), &a.members, &a.ext, StreamOrigin::Unknown);
        assert_eq!(h.coverage().continuous_suffix, 0, "no head is accepted");
        assert!(matches!(eligible_strict(&rules, &h, &pid("ada"), 10 * DAY).cumulative, Some(RuleVerdict::Unknown(_))));
    }

    /// Review of #543, H1: another member's stream — authentic under its own key, ada never in it — does
    /// not stand in for the authority's. The source reads only the pinned `(issuer, stream)`.
    #[test]
    fn another_members_stream_is_not_the_authoritys_history() {
        let a = authority();
        let (real, real_records) = stream(&a, 1, 3, |_| "ada", None);
        let reader = reader_through(&a, &real);
        // A second member signs its own stream of the same name in which bo held every term.
        let sk_n = SigningKey::from_bytes(&[12u8; 32]);
        let n = NodeId::new("127.0.0.1", 7312).unwrap();
        let mut b = Authority { sk: sk_n.clone(), issuer: IssuerId::for_node(&n), members: a.members.clone(), ext: TrustedExternalIssuers::new() };
        b.members.insert(n, MemberKeys { retained: vec![sk_n.verifying_key().to_bytes()], ..Default::default() });
        let (forged, forged_records) = stream(&b, 1, 3, |_| "bo", None);
        let mut presented = forged.clone();
        presented.extend(real.iter().cloned());
        let mut records = forged_records;
        records.extend(real_records);
        let rules = IncumbencyRules { max_consecutive_terms: Some(3), ..IncumbencyRules::default() };
        let h = history_at(&a, &reader, &presented, &records, StreamOrigin::Unknown);
        assert!(matches!(eligible_strict(&rules, &h, &pid("ada"), 5 * DAY).consecutive, Some(RuleVerdict::Ineligible(_))),
            "ada's three terms on the authority's stream decide it");
        let alone = history_at(&a, &reader, &forged, &records, StreamOrigin::Unknown);
        assert_eq!(alone.coverage().continuous_suffix, 0, "the other member's heads link to nothing the reader holds");
    }

    /// Review of #543, H2 and M2: presentation order and duplicates do not change the history — the walk
    /// follows `prev` links from the reader's head.
    #[test]
    fn order_and_duplicates_in_the_presented_heads_change_nothing() {
        let a = authority();
        let (heads, records) = stream(&a, 1, 3, |_| "ada", None);
        let reader = reader_through(&a, &heads);
        let shuffled = vec![heads[0].clone(), heads[2].clone(), heads[1].clone(), heads[1].clone()];
        let rules = IncumbencyRules { max_cumulative_ms: Some(3 * DAY), max_consecutive_terms: Some(4), ..IncumbencyRules::default() };
        let h = history_at(&a, &reader, &shuffled, &records, StreamOrigin::Unknown);
        assert!(h.coverage().from_genesis && h.coverage().through_head);
        assert_eq!(h.coverage().continuous_suffix, 3, "three terms, each once");
        let e = eligible_strict(&rules, &h, &pid("ada"), 5 * DAY);
        assert!(matches!(e.cumulative, Some(RuleVerdict::Ineligible(Ineligible::CumulativeTenure { served_ms, .. })) if served_ms == 3 * DAY));
        assert_eq!(e.consecutive, Some(RuleVerdict::Eligible), "a run of three under a limit of four, not five");
    }

    /// Review of #543, H3: a baseline binds only at the head it names. Offered heads 25.. against a
    /// baseline taken at term 19's head, the terms between are missing, so tenure is unknown.
    #[test]
    fn a_baseline_binds_only_at_the_head_it_was_taken_at() {
        let a = authority();
        let (heads, records) = stream(&a, 1, 30, |n| if n % 2 == 0 { "ada" } else { "bo" }, None);
        let reader = reader_through(&a, &heads);
        let baseline = |at: usize| StreamOrigin::Baseline {
            at: heads[at].head.digest(),
            after: tid(at as u64 + 1),
            cumulative_ms: HashMap::from([(pid("ada"), 5 * DAY)]),
        };
        let rules = IncumbencyRules { max_cumulative_ms: Some(30 * DAY), ..IncumbencyRules::default() };
        // Heads 25..=30 presented; the baseline was taken at term 19 (index 18): not linked.
        let late = &heads[24..];
        let h = history_at(&a, &reader, late, &records, baseline(18));
        assert!(!h.coverage().from_baseline);
        assert!(matches!(eligible_strict(&rules, &h, &pid("ada"), 31 * DAY).cumulative, Some(RuleVerdict::Unknown(_))));
        // Heads 20..=30 presented: the walk reaches term 19's head, and the baseline binds.
        let h = history_at(&a, &reader, &heads[19..], &records, baseline(18));
        assert!(h.coverage().from_baseline && h.coverage().through_head);
        assert_eq!(eligible_strict(&rules, &h, &pid("ada"), 31 * DAY).cumulative, Some(RuleVerdict::Eligible), "5 + 6 days under 30");
    }

    /// Review of #543, M1/M3: the head is the reader's checkpoint, and reading does not move it. Heads
    /// beyond the checkpoint are ignored; the same call twice gives the same history.
    #[test]
    fn the_head_is_the_readers_checkpoint_and_reading_does_not_move_it() {
        let a = authority();
        let (heads, records) = stream(&a, 1, 5, |n| if n <= 3 { "bo" } else { "ada" }, None);
        let reader = reader_through(&a, &heads[..3]);
        let h1 = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        let h2 = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        assert_eq!(h1.coverage(), h2.coverage());
        assert_eq!(h1.coverage().continuous_suffix, 3, "terms 4 and 5 are past what this reader holds");
        assert_eq!(reader.checkpoint(&a.issuer, "appointments/curator").map(|c| c.seq), Some(3));
        // The limit, pinned in the open: ada holds terms 4 and 5, which this reader has not been offered, so
        // under a two-term limit it decides her eligible — "current" means as of this reader's checkpoint.
        let rules = IncumbencyRules { max_consecutive_terms: Some(2), ..IncumbencyRules::default() };
        assert_eq!(eligible_strict(&rules, &h1, &pid("ada"), 6 * DAY).consecutive, Some(RuleVerdict::Eligible));
        let current = history_at(&a, &reader_through(&a, &heads), &heads, &records, StreamOrigin::Unknown);
        assert!(matches!(eligible_strict(&rules, &current, &pid("ada"), 6 * DAY).consecutive, Some(RuleVerdict::Ineligible(_))));
    }

    /// A missing record in the middle leaves the terms after it vouched for and nothing before; the
    /// head's own record missing means nothing reaches the present.
    #[test]
    fn a_gap_keeps_the_suffix_and_a_missing_head_record_keeps_nothing_current() {
        let a = authority();
        let (heads, mut records) = stream(&a, 1, 6, |_| "bo", None);
        let reader = reader_through(&a, &heads);
        let mut gapped = records.clone();
        gapped.retain(|_, t| t.term != tid(3));
        let h = history_at(&a, &reader, &heads, &gapped, StreamOrigin::Unknown);
        assert_eq!(h.coverage().continuous_suffix, 3);
        assert!(!h.coverage().from_genesis && h.coverage().through_head);
        records.retain(|_, t| t.term != tid(6));
        let h = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        assert!(!h.coverage().through_head);
    }

    fn signed(sk: &SigningKey, head: Head) -> SignedHead {
        let signature = mycelium_core::tls::sign_bytes(sk, &head.canonical_bytes()).to_vec();
        SignedHead { head, signature }
    }

    /// The second review of #543, Q7: the stream is pinned too. An authority head on the curator stream
    /// whose `prev` names a head on another of its streams does not pull that stream's terms in.
    #[test]
    fn a_link_into_another_stream_of_the_same_authority_ends_the_walk() {
        let a = authority();
        let other_rec = RecordId { issuer: a.issuer.clone(), digest: [0xA1; 32] };
        let other = signed(&a.sk, Head { issuer: a.issuer.clone(), stream: "appointments/other".into(), record: other_rec.clone(), seq: 1, prev: None });
        let rec = RecordId { issuer: a.issuer.clone(), digest: [0xA2; 32] };
        let cur = signed(&a.sk, Head { issuer: a.issuer.clone(), stream: "appointments/curator".into(), record: rec.clone(), seq: 2, prev: Some(other.head.digest()) });
        let records = HashMap::from([
            (other_rec, TermRecord { holder: pid("ada"), term: tid(1), started_ms: DAY, ended_ms: 2 * DAY }),
            (rec, TermRecord { holder: pid("bo"), term: tid(2), started_ms: 2 * DAY, ended_ms: 3 * DAY }),
        ]);
        let reader = reader_through(&a, std::slice::from_ref(&cur));
        let h = history_at(&a, &reader, &[other, cur], &records, StreamOrigin::Unknown);
        assert_eq!(h.coverage().continuous_suffix, 1, "only the curator stream's own head");
        assert!(!h.coverage().from_genesis, "the other stream's first head is not this role's genesis");
    }

    /// The second review of #543, Q3: a head re-pointed at an earlier appointment would count that term
    /// twice; a repeated term breaks the chain instead.
    #[test]
    fn a_head_repeating_an_earlier_term_breaks_the_chain() {
        let a = authority();
        let (mut heads, records) = stream(&a, 1, 2, |_| "ada", None);
        let again = signed(&a.sk, Head {
            issuer: a.issuer.clone(),
            stream: "appointments/curator".into(),
            record: heads[0].head.record.clone(),
            seq: 3,
            prev: Some(heads[1].head.digest()),
        });
        heads.push(again);
        let reader = reader_through(&a, &heads);
        let h = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        assert!(!h.coverage().through_head, "the head's term repeats term 1");
        let rules = IncumbencyRules { max_cumulative_ms: Some(3 * DAY), ..IncumbencyRules::default() };
        assert!(matches!(eligible_strict(&rules, &h, &pid("ada"), 5 * DAY).cumulative, Some(RuleVerdict::Unknown(_))),
            "not 3 days from counting term 1 twice");
    }

    /// The second review of #543, Q4: when the reader has recorded the authority equivocating on the
    /// stream, no branch is vouched for.
    #[test]
    fn a_forked_stream_vouches_for_nothing() {
        let a = authority();
        let (heads, records) = stream(&a, 1, 3, |_| "bo", None);
        let mut reader = reader_through(&a, &heads);
        let alt = signed(&a.sk, Head {
            issuer: a.issuer.clone(),
            stream: "appointments/curator".into(),
            record: RecordId { issuer: a.issuer.clone(), digest: [0xF0; 32] },
            seq: 3,
            prev: Some(heads[1].head.digest()),
        });
        let _ = reader.offer(&alt, &heads, &a.members, &a.ext);
        assert!(!reader.forks().is_empty(), "the reader recorded the fork");
        let parts = AppointmentStream::from_parts(&a.issuer, "appointments/curator",
            reader.checkpoint(&a.issuer, "appointments/curator"), reader.forks());
        assert!(parts.forked, "from_parts reads the fork as from_reader does");
        let h = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        assert_eq!(h.coverage().continuous_suffix, 0);
        assert!(!h.coverage().through_head);
    }

    /// The second review of #543, Q2: revoking the authority's old key does not end the role's history
    /// (routine rotation never did — a retained key still verifies as current). Heads signed under the
    /// revoked key, reached through `prev` digests the new key's heads committed to, still count; the
    /// checkpoint head itself must be under a current key.
    #[test]
    fn a_revoked_old_key_does_not_end_the_history() {
        let mut a = authority();
        let old = a.sk.clone();
        let new = SigningKey::from_bytes(&[13u8; 32]);
        let n = NodeId::new("127.0.0.1", 7311).unwrap();
        let (mut heads, mut records) = stream(&a, 1, 2, |_| "bo", Some(&old));
        for k in 3..=4u64 {
            let rec = RecordId { issuer: a.issuer.clone(), digest: [k as u8; 32] };
            records.insert(rec.clone(), TermRecord { holder: pid("bo"), term: tid(k), started_ms: k * DAY, ended_ms: (k + 1) * DAY });
            let h = Head { issuer: a.issuer.clone(), stream: "appointments/curator".into(), record: rec, seq: k, prev: Some(heads.last().unwrap().head.digest()) };
            heads.push(signed(&new, h));
        }
        // The reader followed the stream while both keys were current; then the old key is revoked.
        a.members.insert(n.clone(), MemberKeys { retained: vec![old.verifying_key().to_bytes(), new.verifying_key().to_bytes()], ..Default::default() });
        let reader = reader_through(&a, &heads);
        a.members.insert(n, MemberKeys {
            retained: vec![old.verifying_key().to_bytes(), new.verifying_key().to_bytes()],
            revoked: std::collections::HashSet::from([old.verifying_key().to_bytes()]),
        });
        let h = history_at(&a, &reader, &heads, &records, StreamOrigin::Unknown);
        assert!(h.coverage().from_genesis && h.coverage().through_head, "{:?}", h.coverage());
        assert_eq!(h.coverage().continuous_suffix, 4);
    }
}
