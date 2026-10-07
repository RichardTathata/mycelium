//! **A history source that establishes its own coverage** — realignment repairs A3, decision D4, the half
//! 2.24.0 did not build (`docs/plans/realignment-repairs.md` §3.4: *"the signed-heads reader supplies
//! continuity from its checkpoint onward and the availability of what the heads reference, never earlier
//! history"*).
//!
//! [`eligibility::eligible_strict`](super::eligibility::eligible_strict) decides each incumbency rule from a
//! [`TermHistory`], and a `TermHistory` is only as good as whoever built it. Until this module, nothing in
//! the substrate built one: the embedding application assembled `ChainedTerm`s and chose an `Origin`, so
//! "the source establishes coverage" rested on the caller. Here the source is an **appointment stream** in
//! the knowledge layer, and coverage comes from what the substrate can verify about it:
//!
//! - **Authenticity and continuity** — each appointment is a record its appointing authority published on
//!   one stream; each [`SignedHead`] points at one appointment. The heads are offered to the reader's
//!   [`HeadCheckpoints`], which authenticates every head and advances only through an unbroken chain from its
//!   durable checkpoint. A head the reader does not accept contributes nothing.
//! - **Availability** — a verified head whose appointment record cannot be fetched is a **gap**: the term
//!   it names is absent, so the chain breaks there and only the suffix after it is vouched for.
//! - **The head** — the latest head the reader verified. If its record is unavailable, nothing reaches the
//!   present.
//! - **The origin** — the stream's first head (`prev: None`) is the role's genesis. Anything else began
//!   before what this reader can vouch for: [`Origin::Unknown`] unless the caller supplies a
//!   [`Origin::Baseline`] it trusts at that point. Trust on first use, stated: the reader vouches for
//!   continuity from its first checkpoint, **never earlier history**.
//!
//! What it does not claim: that the appointing authority published every appointment there was.

use super::eligibility::{ChainedTerm, Origin, TermHistory};
use super::handover::TermRecord;
use super::TermId;
use crate::knowledge::heads::{CheckpointStore, HeadCheckpoints, HeadVerdict, SignedHead};
use crate::knowledge::issuer::{MemberKeySource, TrustedExternalIssuers};
use crate::knowledge::RecordId;

/// A term id standing for a record the source could not produce; it never matches a real term, so the
/// chain breaks there.
fn unavailable(seq: u64) -> TermId {
    TermId::new(format!("<unavailable appointment at head seq {seq}>")).expect("non-empty")
}

/// Build the [`TermHistory`] an appointment stream supports.
///
/// `chain` is the stream's heads as the source holds them, oldest first. `records` resolves a head's
/// [`RecordId`] to the appointment it records, or `None` when it cannot be fetched. `origin` is used only if
/// the verified chain does not begin at the stream's first head — a baseline the caller trusts at the
/// point the chain begins.
pub fn history_from_appointment_stream<S: CheckpointStore>(
    reader: &mut HeadCheckpoints<S>,
    chain: &[SignedHead],
    records: impl Fn(&RecordId) -> Option<TermRecord>,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
    origin: Origin,
) -> TermHistory {
    let verified = accepted(reader, chain, members, external);
    build(&verified, records, origin)
}

/// From verified heads (oldest first) to chained terms, the origin and the head.
fn build(
    verified: &[&SignedHead],
    records: impl Fn(&RecordId) -> Option<TermRecord>,
    origin: Origin,
) -> TermHistory {
    let Some(last) = verified.last() else {
        return TermHistory::from_chain(Vec::new(), Origin::Unknown, &unavailable(0));
    };
    let mut terms: Vec<ChainedTerm> = Vec::new();
    // The term id of the previous verified head, or the marker for one whose record was unavailable.
    let mut previous: Option<TermId> = None;
    let mut head_term = unavailable(last.head.seq);
    for (i, sh) in verified.iter().enumerate() {
        let this = records(&sh.head.record);
        let prev_for_this = if i == 0 {
            match (&sh.head.prev, &origin) {
                (None, _) => None,
                (Some(_), Origin::Baseline { after, .. }) => Some(after.clone()),
                (Some(_), _) => Some(unavailable(sh.head.seq.saturating_sub(1))),
            }
        } else {
            previous.clone()
        };
        match this {
            Some(record) => {
                previous = Some(record.term.clone());
                if std::ptr::eq(*sh, *last) {
                    head_term = record.term.clone();
                }
                terms.push(ChainedTerm { record, previous: prev_for_this });
            }
            None => previous = Some(unavailable(sh.head.seq)),
        }
    }
    let first_is_genesis = verified.first().is_some_and(|h| h.head.prev.is_none());
    let origin = if first_is_genesis {
        Origin::Genesis
    } else {
        match origin {
            Origin::Genesis => Origin::Unknown, // a caller's "genesis" the stream does not show
            other => other,
        }
    };
    TermHistory::from_chain(terms, origin, &head_term)
}

/// Which offered heads the reader accepted, oldest first. Continuity stops at the first head the
/// reader will not vouch for — an unauthenticated head, a fork, a gap it cannot bridge — so nothing after
/// it counts, even if a later head would authenticate on its own.
fn accepted<'a, S: CheckpointStore>(
    reader: &mut HeadCheckpoints<S>,
    chain: &'a [SignedHead],
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Vec<&'a SignedHead> {
    let mut out = Vec::new();
    for sh in chain {
        match reader.offer(sh, chain, members, external) {
            HeadVerdict::Advanced { .. } | HeadVerdict::AlreadyHeld => out.push(sh),
            _ => break, // continuity stops at the first head the reader will not vouch for
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::heads::MemoryCheckpointStore;
    use crate::knowledge::issuer::MemberKeys;
    use crate::knowledge::store::Head;
    use crate::knowledge::IssuerId;
    use crate::mandate::eligibility::{eligible_strict, RuleVerdict};
    use crate::mandate::handover::{IncumbencyRules, Ineligible};
    use crate::mandate::PrincipalId;
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

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

    fn history(a: &Authority, heads: &[SignedHead], records: &HashMap<RecordId, TermRecord>, origin: Origin) -> TermHistory {
        let mut reader = HeadCheckpoints::open(MemoryCheckpointStore::new()).unwrap();
        history_from_appointment_stream(&mut reader, heads, |r| records.get(r).cloned(), &a.members, &a.ext, origin)
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
        let gapped = history(&a, &heads, &records, Origin::Unknown);
        assert!(matches!(eligible_strict(&rules, &gapped, &pid("ada"), 17 * DAY).cumulative, Some(RuleVerdict::Unknown(_))));
        let whole = history(&a, &heads, &full, Origin::Unknown);
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
        let h = history(&a, &heads, &records, Origin::Genesis);
        assert!(!h.coverage().from_genesis, "a caller's Genesis the stream does not show is not taken");
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
        let h = history(&a, &heads, &records, Origin::Unknown);
        assert_eq!(h.coverage().continuous_suffix, 0, "no head is accepted");
        assert!(matches!(eligible_strict(&rules, &h, &pid("ada"), 10 * DAY).cumulative, Some(RuleVerdict::Unknown(_))));
    }
}
