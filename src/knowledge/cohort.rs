//! **Cohorts, declared at admission** (Boundary H item H5) —
//! [`docs/design/knowledge-cohorts.md`](../../../docs/design/knowledge-cohorts.md).
//!
//! # Control dependence, stated by whoever admitted the fleet
//!
//! "Identity is not independence": a reader configures which issuers are one voice. For a fleet of
//! agents that is unworkable by hand — agents are admitted and retired continually — and a colluding
//! population is exactly the case where the reader must not have to guess. So the **operator who
//! admits a fleet declares it** as a cohort, in a signed [`CohortDeclaration`], and a reader that
//! trusts that operator treats the cohort as one control group. A statement made at admission by an
//! accountable party — not an inference by the reader.
//!
//! **A cohort captures control dependence, not evidential lineage.** Two independent organisations
//! repeating one underlying report share an *origin*; grouping issuers does not detect that.
//!
//! # The lifecycle, and the one rule it obeys
//!
//! **When in doubt, merge; never split.** Merging lowers counted independence; splitting would invent
//! it. Every case below follows from that:
//!
//! | Case | Handling |
//! |---|---|
//! | Two trusted operators disagree about membership | Both hold: the member is in both cohorts, so they merge for this reader |
//! | Overlapping cohorts | Merge, as connected components |
//! | Removal | Only by a **superseding** declaration (higher `seq`, same operator and cohort). Evidence issued before the removal keeps the old grouping |
//! | A declaration expires, or no refresh arrives in a partition | **Membership is kept.** Staleness is reported, never treated as removal |
//! | Key rotation | No effect: membership is by issuer identity, not by key |
//! | An agent admitted before its declaration arrives | The reader's undeclared rule applies |
//!
//! # Historical grouping
//!
//! A record is grouped by the union of the cohorts its issuer was in **when the record was issued**
//! and the cohorts it is in **now**. A later removal therefore cannot make earlier evidence look more
//! independent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::issuer::{verify_signed_by, Authenticity, MemberKeySource, TrustedExternalIssuers, UnverifiableReason};
use super::IssuerId;

/// A cohort's identity: which operator declared it, and its name under that operator. Two operators
/// may use the same name for different fleets, so the operator is part of the identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CohortKey {
    /// The declaring operator.
    pub operator: IssuerId,
    /// The cohort's name under that operator.
    pub cohort: String,
}

/// An operator's statement: *these issuers are one fleet under my control*.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CohortDeclaration {
    /// Who declares it. Verified through the configured-external path.
    pub operator: IssuerId,
    /// The cohort's name under that operator.
    pub cohort: String,
    /// Orders this operator's declarations for this cohort. A higher `seq` supersedes a lower one.
    pub seq: u64,
    /// The members, by issuer identity.
    pub members: BTreeSet<IssuerId>,
    /// From when this declaration speaks.
    pub valid_from_ms: u64,
    /// Until when the operator vouches for it being current. Past this it is **stale** — reported,
    /// never treated as removal.
    pub valid_until_ms: u64,
}

impl CohortDeclaration {
    /// This declaration's cohort.
    pub fn key(&self) -> CohortKey {
        CohortKey { operator: self.operator.clone(), cohort: self.cohort.clone() }
    }

    /// The exact bytes the operator signs. Tagged, so a cohort signature can never authenticate a
    /// record or a head. Members are in sorted order, so the same declaration always has the same
    /// bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn lp(out: &mut Vec<u8>, b: &[u8]) {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        let mut out = Vec::new();
        lp(&mut out, b"mycelium.knowledge/cohort/1");
        lp(&mut out, self.operator.as_str().as_bytes());
        lp(&mut out, self.cohort.as_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.valid_from_ms.to_le_bytes());
        out.extend_from_slice(&self.valid_until_ms.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u32).to_le_bytes());
        for m in &self.members {
            lp(&mut out, m.as_str().as_bytes());
        }
        out
    }
}

/// A declaration with the operator's signature over [`CohortDeclaration::canonical_bytes`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCohortDeclaration {
    /// The declaration.
    pub declaration: CohortDeclaration,
    /// The operator's signature.
    pub signature: Vec<u8>,
}

/// What a reader did with an offered declaration.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CohortOffer {
    /// Kept. Every declaration a reader keeps stays in its history; a later one supersedes it only
    /// for evidence issued after the later one's `valid_from_ms`.
    Accepted,
    /// This reader does not take cohort declarations from this operator.
    UntrustedOperator,
    /// The signature does not verify under the operator's configured key.
    Unverifiable(UnverifiableReason),
    /// The same `(operator, cohort, seq)` is already held with different content. Both are kept;
    /// the reader treats the union as the membership (merge, never split).
    ConflictingSameSeq,
    /// Already held, identically.
    AlreadyHeld,
}

/// A reader's verified cohort declarations, from the operators it trusts.
#[derive(Clone, Debug, Default)]
pub struct CohortView {
    /// Operators whose declarations this reader accepts.
    sources: BTreeSet<IssuerId>,
    /// Every accepted declaration, per cohort, in `seq` order (ties kept: see `ConflictingSameSeq`).
    declarations: BTreeMap<CohortKey, Vec<CohortDeclaration>>,
}

/// Where one issuer stood, for one record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    /// Every cohort the issuer is grouped with for this record: at issue time, or now.
    pub cohorts: BTreeSet<CohortKey>,
    /// Whether at least one of those placements rests on a declaration that is still current.
    pub any_current: bool,
}

impl CohortView {
    /// A view that takes declarations from `sources` only.
    pub fn trusting(sources: impl IntoIterator<Item = IssuerId>) -> Self {
        Self { sources: sources.into_iter().collect(), declarations: BTreeMap::new() }
    }

    /// Offer a signed declaration. The operator must be one of this view's sources, and its
    /// signature must verify through the configured-external path.
    pub fn offer(
        &mut self,
        signed: &SignedCohortDeclaration,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> CohortOffer {
        let d = &signed.declaration;
        if !self.sources.contains(&d.operator) {
            return CohortOffer::UntrustedOperator;
        }
        match verify_signed_by(&d.operator, &d.canonical_bytes(), &signed.signature, members, external) {
            Authenticity::Current { .. } => {}
            Authenticity::Revoked { .. } => {
                return CohortOffer::Unverifiable(UnverifiableReason::BadSignature);
            }
            Authenticity::Unverifiable(r) => return CohortOffer::Unverifiable(r),
        }
        let held = self.declarations.entry(d.key()).or_default();
        if held.iter().any(|h| h == d) {
            return CohortOffer::AlreadyHeld;
        }
        let conflicting = held.iter().any(|h| h.seq == d.seq);
        held.push(d.clone());
        held.sort_by_key(|h| h.seq);
        if conflicting { CohortOffer::ConflictingSameSeq } else { CohortOffer::Accepted }
    }

    /// **Where `issuer` stands for a record it issued at `at_ms`, judged at `now_ms`.**
    ///
    /// Membership of a cohort at time `t` is decided by the declarations in force at `t`: those with
    /// `valid_from_ms <= t`, taking the highest `seq` among them (all of them, if several share it —
    /// merge, never split). Expiry does not end membership; only a superseding declaration does. The
    /// placement is the union of membership at `at_ms` and at `now_ms`.
    pub fn placement(&self, issuer: &IssuerId, at_ms: u64, now_ms: u64) -> Placement {
        let mut out = Placement::default();
        for (key, decls) in &self.declarations {
            for t in [at_ms, now_ms] {
                let in_force = in_force_at(decls, t);
                if in_force.iter().any(|d| d.members.contains(issuer)) {
                    out.cohorts.insert(key.clone());
                    if in_force.iter().any(|d| d.members.contains(issuer) && now_ms <= d.valid_until_ms) {
                        out.any_current = true;
                    }
                }
            }
        }
        out
    }

    /// Every `(cohort, member)` pair in force at `t` — the edges that join cohorts to each other
    /// through shared members, whether or not those members have said anything.
    pub fn members_in_force(&self, t: u64) -> Vec<(CohortKey, IssuerId)> {
        let mut out = Vec::new();
        for (key, decls) in &self.declarations {
            for d in in_force_at(decls, t) {
                for m in &d.members {
                    out.push((key.clone(), m.clone()));
                }
            }
        }
        out
    }

    /// Is this view empty?
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }
}

/// The declarations in force at `t`: the highest `seq` among those already valid from `t`.
fn in_force_at(decls: &[CohortDeclaration], t: u64) -> Vec<&CohortDeclaration> {
    let valid: Vec<&CohortDeclaration> = decls.iter().filter(|d| d.valid_from_ms <= t).collect();
    let Some(top) = valid.iter().map(|d| d.seq).max() else { return Vec::new() };
    valid.into_iter().filter(|d| d.seq == top).collect()
}

/// What a reader does with an issuer no trusted declaration or configured group places.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UndeclaredRule {
    /// Each undeclared issuer is its own group. The behaviour before H5, and the default.
    #[default]
    OwnGroup,
    /// All undeclared issuers together are one group. Freshly minted or not-yet-declared issuers
    /// cannot manufacture independence.
    OneGroup,
    /// Undeclared issuers are excluded, and reported.
    Excluded,
}

/// What a reader does with an issuer whose only placements rest on stale declarations.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StaleRule {
    /// Keep the stale placement. Dependence is sticky; this is the default.
    #[default]
    Retain,
    /// Exclude that issuer's evidence until the relationship is refreshed or resolved. It never falls
    /// back to looking independent.
    Exclude,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

    pub(crate) fn operator() -> (SigningKey, IssuerId, TrustedExternalIssuers) {
        let sk = SigningKey::from_bytes(&[11u8; 32]);
        let op = IssuerId::new("operator:acme").unwrap();
        let mut ext = TrustedExternalIssuers::new();
        ext.trust(op.clone(), sk.verifying_key().to_bytes()).unwrap();
        (sk, op, ext)
    }

    pub(crate) fn declare(
        sk: &SigningKey,
        op: &IssuerId,
        cohort: &str,
        seq: u64,
        members: &[&str],
        from: u64,
        until: u64,
    ) -> SignedCohortDeclaration {
        let declaration = CohortDeclaration {
            operator: op.clone(),
            cohort: cohort.into(),
            seq,
            members: members.iter().map(|m| IssuerId::new(m).unwrap()).collect(),
            valid_from_ms: from,
            valid_until_ms: until,
        };
        let signature = mycelium_core::tls::sign_bytes(sk, &declaration.canonical_bytes()).to_vec();
        SignedCohortDeclaration { declaration, signature }
    }

    fn no_members() -> HashMap<NodeId, MemberKeys> {
        HashMap::new()
    }

    #[test]
    fn a_declaration_from_an_untrusted_operator_is_ignored() {
        let (sk, op, ext) = operator();
        let mut v = CohortView::trusting([IssuerId::new("operator:other").unwrap()]);
        assert_eq!(v.offer(&declare(&sk, &op, "fleet", 1, &["a"], 0, 10), &no_members(), &ext), CohortOffer::UntrustedOperator);
        assert!(v.is_empty());
    }

    #[test]
    fn a_forged_declaration_is_refused() {
        let (sk, op, ext) = operator();
        let mut v = CohortView::trusting([op.clone()]);
        let mut d = declare(&sk, &op, "fleet", 1, &["a"], 0, 10);
        d.declaration.members.insert(IssuerId::new("smuggled").unwrap());
        assert_eq!(v.offer(&d, &no_members(), &ext), CohortOffer::Unverifiable(UnverifiableReason::BadSignature));
    }

    /// **Removal only by supersession, and not retroactively.** A record issued while A was a member
    /// keeps that grouping; a record issued after A's removal does not get it.
    #[test]
    fn a_superseding_removal_applies_to_later_evidence_only() {
        let (sk, op, ext) = operator();
        let mut v = CohortView::trusting([op.clone()]);
        v.offer(&declare(&sk, &op, "fleet", 1, &["a", "b"], 0, 10_000), &no_members(), &ext);
        v.offer(&declare(&sk, &op, "fleet", 2, &["b"], 5_000, 10_000), &no_members(), &ext);
        let a = IssuerId::new("a").unwrap();
        assert_eq!(v.placement(&a, 1_000, 6_000).cohorts.len(), 1, "issued while a member");
        assert!(v.placement(&a, 6_000, 6_000).cohorts.is_empty(), "issued after removal");
    }

    /// **Expiry never removes.** Past `valid_until_ms` the placement is kept, and reported stale.
    #[test]
    fn an_expired_declaration_keeps_membership_and_reports_staleness() {
        let (sk, op, ext) = operator();
        let mut v = CohortView::trusting([op.clone()]);
        v.offer(&declare(&sk, &op, "fleet", 1, &["a"], 0, 1_000), &no_members(), &ext);
        let p = v.placement(&IssuerId::new("a").unwrap(), 500, 5_000);
        assert_eq!(p.cohorts.len(), 1);
        assert!(!p.any_current);
    }
}
