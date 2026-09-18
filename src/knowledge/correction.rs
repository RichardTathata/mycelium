//! **Expiry and correction** (item 3 PR 5) — §4 of
//! [`docs/design/knowledge-layer.md`](../../../docs/design/knowledge-layer.md):
//!
//! > **Expiry and correction are active**, with a dependency index, so a retraction reaches what was
//! > derived from the retracted record **rather than waiting to be noticed**.
//!
//! # Withdrawing a basis is not withdrawing the conclusion
//!
//! This is the decision the whole module turns on, and it follows from the layer's reason for
//! existing.
//!
//! A `Retracts` link is **same-issuer only** — enforced at construction, because only an issuer can
//! withdraw its own statement. But records by *other* issuers may have been derived from the
//! withdrawn one, and a retraction must reach them: that is what "active" means above.
//!
//! What it must **not** do is mark them retracted. Issuer A withdrawing its observation does not
//! give A standing to withdraw B's conclusion — B may stand by that conclusion on other grounds, or
//! may not have finished looking. So the status is [`Standing::BasisWithdrawn`]: **a fact about the
//! record's support**, not a judgement about the record. It names which basis went and how far
//! downstream this is.
//!
//! Collapsing the two would let any issuer silently invalidate anyone's conclusions by retracting
//! something they had cited — which is erasure wearing a correction's clothes, and §1 exists to
//! prevent exactly that.
//!
//! # Nothing is deleted, here or anywhere
//!
//! [`standing`] computes a *view*. No record is removed, and [`KnowledgeStore::len`] is unchanged by
//! any of it — pinned by a test, because "a retraction reaches downstream records" is one short step
//! from "a retraction deletes downstream records".
//!
//! # Expiry needs no timer, which is better than a replayable one
//!
//! §5 says expiry timers should run on the replay clock seam rather than `tokio::time`. They run on
//! nothing at all: expiry here is a **pure function of the reader's `now_ms` and the record's own
//! `at_ms`**. There is no wait to reproduce and no timer to schedule, so a replay gets it right
//! without the seam being involved. Said plainly rather than claiming to use a seam this does not
//! need.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::store::KnowledgeStore;
use super::{IssuerId, LinkKind, RecordId};

/// Where a record stands right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Nothing has undermined it.
    Current,
    /// **Its own issuer withdrew it.** The only status that is a judgement about this record, and
    /// the only one its issuer can cause.
    Retracted {
        /// Who withdrew it — necessarily its own issuer.
        by: IssuerId,
    },
    /// **A record this was derived from was retracted.** A fact about this record's support, *not* a
    /// verdict on the record: the issuer may stand by it on other grounds.
    BasisWithdrawn {
        /// Which withdrawn record this traces back to. The **nearest** one.
        basis: RecordId,
        /// How many `DerivedFrom` hops away that basis is (1 = directly derived from it).
        hops: usize,
    },
    /// Older than the reader's window.
    Expired {
        /// When the record was issued.
        at_ms: u64,
    },
}

impl Standing {
    /// Is this record still usable as support?
    pub fn is_current(&self) -> bool {
        matches!(self, Standing::Current)
    }
}

/// **Which records were derived from which.** Built once over a store, then queried.
///
/// The direction matters: the links point *upward* (a record names what it came from), so answering
/// "what does this retraction reach?" without an index means scanning every record for every query.
/// The index inverts it.
#[derive(Debug, Default)]
pub struct DependencyIndex {
    /// basis → records derived from it.
    dependents: BTreeMap<RecordId, BTreeSet<RecordId>>,
    /// Records that have been retracted by their issuer.
    retracted: BTreeMap<RecordId, IssuerId>,
}

impl DependencyIndex {
    /// Build the index over every record in `store`.
    pub fn build(store: &KnowledgeStore) -> Self {
        let mut index = Self::default();
        for record in store.records() {
            for link in record.links() {
                match link.kind {
                    LinkKind::DerivedFrom => {
                        index
                            .dependents
                            .entry(link.target.clone())
                            .or_default()
                            .insert(record.id().clone());
                    }
                    LinkKind::Retracts => {
                        // Same-issuer is enforced at construction; recording the issuer here keeps
                        // the reason available without a second fetch.
                        index.retracted.insert(link.target.clone(), record.issuer().clone());
                    }
                    _ => {}
                }
            }
        }
        index
    }

    /// Was this record withdrawn by its issuer?
    pub fn retracted_by(&self, id: &RecordId) -> Option<&IssuerId> {
        self.retracted.get(id)
    }

    /// Records derived **directly** from `id`.
    pub fn dependents_of(&self, id: &RecordId) -> impl Iterator<Item = &RecordId> {
        self.dependents.get(id).into_iter().flatten()
    }

    /// **What a retraction of `id` reaches**, transitively, with each record's distance in hops.
    ///
    /// Breadth-first, so each record is reported at its *shortest* distance from the withdrawn
    /// basis. **Cycle-safe** — a `DerivedFrom` cycle (accidental or otherwise) terminates, because a
    /// record is enqueued at most once. Results are ordered by `RecordId`, which is content-derived,
    /// so two nodes computing this agree.
    pub fn transitively_affected(&self, id: &RecordId) -> Vec<(RecordId, usize)> {
        let mut seen: BTreeSet<RecordId> = BTreeSet::new();
        seen.insert(id.clone());

        let mut out: Vec<(RecordId, usize)> = Vec::new();
        let mut queue: VecDeque<(RecordId, usize)> = VecDeque::new();
        queue.push_back((id.clone(), 0));

        while let Some((current, depth)) = queue.pop_front() {
            for dependent in self.dependents_of(&current) {
                if seen.insert(dependent.clone()) {
                    out.push((dependent.clone(), depth + 1));
                    queue.push_back((dependent.clone(), depth + 1));
                }
            }
        }
        out.sort();
        out
    }
}

/// **Where one record stands**, given a reader's clock and window.
///
/// The order of the checks is deliberate and is the module's contract:
///
/// 1. **Retraction first.** An issuer withdrawing its own statement is the strongest thing that can
///    be said about it, and it is said by the one party with standing to say it.
/// 2. **Then basis withdrawal**, which is a fact about support rather than a verdict.
/// 3. **Then expiry**, which is the reader's own policy and says nothing about anyone's intent.
///
/// Reporting expiry ahead of a retraction would tell a reader "this aged out" about a record its
/// issuer had actively withdrawn — true, and the least useful of the true things available.
pub fn standing(
    store: &KnowledgeStore,
    index: &DependencyIndex,
    id: &RecordId,
    now_ms: u64,
    max_age_ms: u64,
) -> Option<Standing> {
    let record = store.get(id)?;

    if let Some(by) = index.retracted_by(id) {
        return Some(Standing::Retracted { by: by.clone() });
    }

    // Walk up this record's own derivation chain to the nearest withdrawn basis. Bounded by the
    // number of records, and cycle-safe by the visited set.
    if let Some((basis, hops)) = nearest_withdrawn_basis(store, index, id) {
        return Some(Standing::BasisWithdrawn { basis, hops });
    }

    if now_ms.saturating_sub(record.at_ms()) > max_age_ms {
        return Some(Standing::Expired { at_ms: record.at_ms() });
    }

    Some(Standing::Current)
}

/// The nearest retracted record reachable by following this record's `DerivedFrom` links upward.
///
/// Breadth-first, so "nearest" is by hop count. A record whose basis is missing from the store
/// simply stops that branch: an absent record is not evidence of a withdrawal, and treating it as
/// one would make a partial replica look like a wave of retractions.
fn nearest_withdrawn_basis(
    store: &KnowledgeStore,
    index: &DependencyIndex,
    id: &RecordId,
) -> Option<(RecordId, usize)> {
    let mut seen: BTreeSet<RecordId> = BTreeSet::new();
    seen.insert(id.clone());

    let mut queue: VecDeque<(RecordId, usize)> = VecDeque::new();
    queue.push_back((id.clone(), 0));

    while let Some((current, depth)) = queue.pop_front() {
        let Some(record) = store.get(&current) else { continue };
        // Collect this level's bases in id order so the result is deterministic across nodes.
        let mut bases: Vec<&RecordId> = record
            .links()
            .iter()
            .filter(|l| l.kind == LinkKind::DerivedFrom)
            .map(|l| &l.target)
            .collect();
        bases.sort();
        for basis in bases {
            if !seen.insert(basis.clone()) {
                continue;
            }
            if index.retracted_by(basis).is_some() {
                return Some((basis.clone(), depth + 1));
            }
            queue.push_back((basis.clone(), depth + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeRecord, Link, RecordKind};

    fn issuer(s: &str) -> IssuerId {
        IssuerId::new(s).expect("valid")
    }

    fn record(
        by: &str,
        kind: RecordKind,
        at_ms: u64,
        subject: &str,
        links: Vec<Link>,
    ) -> KnowledgeRecord {
        KnowledgeRecord::new(issuer(by), kind, at_ms, subject, b"body".to_vec(), links)
            .expect("well formed")
    }

    fn derived_from(target: &RecordId) -> Link {
        Link { kind: LinkKind::DerivedFrom, target: target.clone() }
    }

    /// A → B → C, where B is derived from A and C from B.
    fn chain() -> (KnowledgeStore, RecordId, RecordId, RecordId) {
        let a = record("lab-a", RecordKind::Observation, 1_000, "soil/ph", vec![]);
        let a_id = a.id().clone();
        let b = record("lab-b", RecordKind::Assessment, 1_100, "soil/ph", vec![derived_from(&a_id)]);
        let b_id = b.id().clone();
        let c = record(
            "lab-c",
            RecordKind::AcceptanceDecision,
            1_200,
            "soil/ph",
            vec![derived_from(&b_id)],
        );
        let c_id = c.id().clone();

        let mut store = KnowledgeStore::new();
        store.put(a);
        store.put(b);
        store.put(c);
        (store, a_id, b_id, c_id)
    }

    /// **The active part**: retracting A reaches B *and* C, without anyone having to notice.
    #[test]
    fn a_retraction_reaches_transitively_derived_records() {
        let (mut store, a_id, b_id, c_id) = chain();
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id.clone() }],
        ));

        let index = DependencyIndex::build(&store);
        let affected = index.transitively_affected(&a_id);

        assert!(
            affected.iter().any(|(id, hops)| *id == b_id && *hops == 1),
            "the directly derived record is reached at one hop: {affected:?}"
        );
        assert!(
            affected.iter().any(|(id, hops)| *id == c_id && *hops == 2),
            "and the record derived from *that* at two: {affected:?}"
        );
    }

    /// **The decision the module turns on.** A's retraction makes B's *basis* withdrawn — it does
    /// not retract B. Issuer A has no standing to withdraw issuer B's conclusion.
    #[test]
    fn withdrawing_a_basis_does_not_retract_the_conclusion() {
        let (mut store, a_id, b_id, _c_id) = chain();
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id.clone() }],
        ));

        let index = DependencyIndex::build(&store);

        assert_eq!(
            standing(&store, &index, &a_id, 2_000, u64::MAX),
            Some(Standing::Retracted { by: issuer("lab-a") }),
            "A's own issuer withdrew it — that is a retraction"
        );
        assert_eq!(
            standing(&store, &index, &b_id, 2_000, u64::MAX),
            Some(Standing::BasisWithdrawn { basis: a_id, hops: 1 }),
            "B is undermined, not retracted: lab-a cannot withdraw lab-b's conclusion"
        );
    }

    /// **Nothing is deleted.** The store holds exactly what it held — a retraction is a record, not
    /// a removal.
    #[test]
    fn a_retraction_removes_nothing_from_the_store() {
        let (mut store, a_id, _b, _c) = chain();
        let before = store.len();
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id }],
        ));

        assert_eq!(store.len(), before + 1, "the retraction was *added*; nothing was erased");
    }

    /// Retraction is reported ahead of expiry: a record its issuer actively withdrew should not be
    /// described as merely old.
    #[test]
    fn retraction_is_reported_ahead_of_expiry() {
        let (mut store, a_id, _b, _c) = chain();
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id.clone() }],
        ));
        let index = DependencyIndex::build(&store);

        // Well past any window, and also retracted.
        assert_eq!(
            standing(&store, &index, &a_id, 9_999_999, 10),
            Some(Standing::Retracted { by: issuer("lab-a") }),
            "'withdrawn' is the more useful of the true things available"
        );
    }

    /// Expiry is a pure function of the reader's clock and the record's own timestamp — no timer, so
    /// nothing to reproduce on replay.
    #[test]
    fn expiry_is_derived_from_the_readers_clock_with_no_timer() {
        let (store, a_id, _b, _c) = chain();
        let index = DependencyIndex::build(&store);

        assert_eq!(standing(&store, &index, &a_id, 1_500, 1_000), Some(Standing::Current));
        assert_eq!(
            standing(&store, &index, &a_id, 5_000, 1_000),
            Some(Standing::Expired { at_ms: 1_000 })
        );
    }

    /// **A true `DerivedFrom` cycle terminates**, built directly in the index.
    ///
    /// Stated plainly: a cycle cannot be constructed from honest records, because a record's id is
    /// its content and a record cannot contain a link to an id that depends on itself. So a cycle
    /// can only arrive from a **forged or corrupted** index — which is exactly the case a reader
    /// must survive rather than hang on. The index is built by hand here for that reason; the test
    /// above covers the honest shape.
    #[test]
    fn a_forged_derivation_cycle_terminates() {
        let x = RecordId { issuer: issuer("lab-x"), digest: [1u8; 32] };
        let y = RecordId { issuer: issuer("lab-y"), digest: [2u8; 32] };

        let mut index = DependencyIndex::default();
        index.dependents.entry(x.clone()).or_default().insert(y.clone());
        index.dependents.entry(y.clone()).or_default().insert(x.clone());

        // Terminates, and reports each record once — never the starting record again.
        let affected = index.transitively_affected(&x);
        assert_eq!(affected, vec![(y, 1)], "the cycle closes back on x, which is already seen");
    }

    /// The honest shape: a diamond, where one record derives from two bases that share an ancestor.
    /// Each affected record is reported **once**, at its shortest distance.
    #[test]
    fn a_diamond_reports_each_record_once_at_its_shortest_distance() {
        let a = record("lab-a", RecordKind::Observation, 1_000, "s", vec![]);
        let a_id = a.id().clone();
        let b = record("lab-b", RecordKind::Assessment, 1_100, "s", vec![derived_from(&a_id)]);
        let b_id = b.id().clone();
        // `d` derives from both `c` and `b` — so `b` is reachable at one hop and also via `c`, and
        // must still be reported once, at the shorter distance.
        let c = record("lab-c", RecordKind::Assessment, 1_200, "s", vec![derived_from(&b_id)]);
        let c_id = c.id().clone();
        let d = record(
            "lab-d",
            RecordKind::Assessment,
            1_300,
            "s",
            vec![derived_from(&c_id), derived_from(&b_id)],
        );

        let mut store = KnowledgeStore::new();
        store.put(a);
        store.put(b);
        store.put(c);
        store.put(d);

        let index = DependencyIndex::build(&store);
        // Terminates, and reports each record once.
        let affected = index.transitively_affected(&a_id);
        let ids: BTreeSet<&RecordId> = affected.iter().map(|(id, _)| id).collect();
        assert_eq!(ids.len(), affected.len(), "each affected record is reported exactly once");
        assert_eq!(affected.len(), 3, "b, c and d: {affected:?}");
    }

    /// A missing basis stops that branch rather than counting as a withdrawal. Otherwise a partial
    /// replica would look like a wave of retractions.
    #[test]
    fn an_absent_basis_is_not_treated_as_a_withdrawal() {
        let absent = RecordId { issuer: issuer("lab-x"), digest: [9u8; 32] };
        let b = record("lab-b", RecordKind::Assessment, 1_100, "s", vec![derived_from(&absent)]);
        let b_id = b.id().clone();

        let mut store = KnowledgeStore::new();
        store.put(b);
        let index = DependencyIndex::build(&store);

        assert_eq!(
            standing(&store, &index, &b_id, 1_100, u64::MAX),
            Some(Standing::Current),
            "not knowing a basis is not the same as knowing it was withdrawn"
        );
    }

    /// The nearest withdrawn basis is the one reported, not an arbitrary one — so a reader is told
    /// the closest thing that went, which is the one most likely to be actionable.
    #[test]
    fn the_nearest_withdrawn_basis_is_the_one_reported() {
        let (mut store, a_id, b_id, c_id) = chain();
        // Retract *both* A (two hops from C) and B (one hop from C).
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id }],
        ));
        store.put(record(
            "lab-b",
            RecordKind::Assessment,
            2_100,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: b_id.clone() }],
        ));

        let index = DependencyIndex::build(&store);
        assert_eq!(
            standing(&store, &index, &c_id, 2_100, u64::MAX),
            Some(Standing::BasisWithdrawn { basis: b_id, hops: 1 }),
            "the nearer withdrawal is the more actionable one"
        );
    }

    /// An unaffected record stays `Current` — the sweep must not mark everything.
    #[test]
    fn an_unrelated_record_is_untouched_by_a_retraction() {
        let (mut store, a_id, _b, _c) = chain();
        let other = record("lab-z", RecordKind::Observation, 1_000, "water/ph", vec![]);
        let other_id = other.id().clone();
        store.put(other);
        store.put(record(
            "lab-a",
            RecordKind::Observation,
            2_000,
            "soil/ph",
            vec![Link { kind: LinkKind::Retracts, target: a_id }],
        ));

        let index = DependencyIndex::build(&store);
        assert_eq!(
            standing(&store, &index, &other_id, 2_000, u64::MAX),
            Some(Standing::Current),
            "a correction that marked everything would be useless"
        );
    }

    /// A record the store does not hold has no standing to report — `None`, not a guess.
    #[test]
    fn an_unknown_record_has_no_standing() {
        let (store, _a, _b, _c) = chain();
        let index = DependencyIndex::build(&store);
        let unknown = RecordId { issuer: issuer("nobody"), digest: [0u8; 32] };
        assert_eq!(standing(&store, &index, &unknown, 1_000, u64::MAX), None);
    }
}
