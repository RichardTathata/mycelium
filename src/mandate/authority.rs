//! **Authority at execution** (Boundary H item A1) —
//! [`docs/design/authority-at-execution.md`](../../../docs/design/authority-at-execution.md).
//!
//! # The gap this closes
//!
//! Letting a mandate expire stopped **new admissions** wherever a mandate was checked. It did not
//! stop work **already admitted**: a task admitted a second before expiry ran on, a queued item
//! dequeued an hour later ran on, a retry ran on, and delegated work could outlive its parent. And a
//! revocation that never reached a disconnected reader was indistinguishable from "nothing revoked".
//!
//! # The contract (plan §7 A1, rev 0.4)
//!
//! 1. **Every protected operation requires an established mandate.** [`ExecutionGate::check`] denies
//!    work that carries none.
//! 2. **Missing or unverifiable authority prevents execution.** Unknown freshness is a denial.
//! 3. **Validity is checked at the execution boundary**, not only at admission — every
//!    [`ExecutionGate::check`] re-runs the resource's own [`ResourceAuthority::check`] (epoch, scope,
//!    window, operation) at that moment. The gate declares its resource's **AE2 tier**; an advisory
//!    resource cannot be in the strict profile.
//! 4. **Queued, retried and delegated work keeps the requirement.** Work is checked at dequeue and at
//!    every retry; [`AuthorizedWork::delegate`] can never extend its parent's window.
//! 5. **Long-running work declares a continuation policy**, and a class without a demonstrable bound
//!    is [`DrainBound::Unbounded`] — never assumed bounded.
//!
//! # Expiry and revocation are separate
//!
//! **Expiry** is decided locally from `valid_until_ms` under the [`ClockModel`] (*s* bounds each
//! clock's deviation from real time). **Revocation** needs delivery: present authority requires an
//! authority-signed [`RevocationCheckpoint`] that is fresh by the exact predicate
//! `(a − r) ≤ 2s ∧ (r − a) ≤ F − 2s`. Silence is never "nothing revoked", a replayed old checkpoint
//! never refreshes freshness, and a partition longer than *F* − 2*s* stops protected work — the
//! intended failure direction.
//!
//! # Time is passed in
//!
//! Nothing here reads a clock. Every `now_ms` is the caller's, so the whole contract replays
//! deterministically and both clock extremes are testable exactly.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Mandate, PrincipalId, ResourceAuthority, TermId};
use crate::knowledge::issuer::{
    verify_signed_by, Authenticity, MemberKeySource, TrustedExternalIssuers, UnverifiableReason,
};
use crate::knowledge::IssuerId;

// ── the clock model and freshness ───────────────────────────────────────────────────────────

/// *s* bounds **each** participating clock's deviation from real time: `|C(t) − t| ≤ s`. Any two
/// clocks therefore differ by at most 2*s*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockModel {
    /// The bound *s*, in milliseconds.
    pub skew_ms: u64,
}

/// The freshness parameters for revocation checkpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreshnessPolicy {
    /// *F*: the maximum real age of the revocation view present authority may rest on.
    pub freshness_ms: u64,
    /// *I*: how often each authority issues a checkpoint, even when nothing was revoked.
    pub interval_ms: u64,
    /// *D*: the maximum delivery delay the deployment tolerates before failing closed.
    pub delivery_ms: u64,
}

/// Why a profile's parameters were refused at start-up.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// *F* ≤ 4*s*: the guaranteed-acceptance window *F* − 4*s* is empty.
    FreshnessTooShort {
        /// *F*.
        freshness_ms: u64,
        /// *s*.
        skew_ms: u64,
    },
    /// *I* + *D* > *F* − 4*s*: a connected reader could be denied spuriously at the clock extremes.
    IntervalDoesNotFit {
        /// *I* + *D*.
        needed_ms: u64,
        /// *F* − 4*s*.
        window_ms: u64,
    },
    /// The resource is advisory: it offers no boundary the check can sit inside, so it cannot be
    /// in the strict profile (AE2 §4).
    AdvisoryResource,
}

impl FreshnessPolicy {
    /// Check the profile's parameters: *F* > 4*s* and *I* + *D* ≤ *F* − 4*s*.
    pub fn validate(&self, clock: ClockModel) -> Result<(), ProfileError> {
        let four_s = clock.skew_ms.saturating_mul(4);
        if self.freshness_ms <= four_s {
            return Err(ProfileError::FreshnessTooShort { freshness_ms: self.freshness_ms, skew_ms: clock.skew_ms });
        }
        let window = self.freshness_ms - four_s;
        let needed = self.interval_ms.saturating_add(self.delivery_ms);
        if needed > window {
            return Err(ProfileError::IntervalDoesNotFit { needed_ms: needed, window_ms: window });
        }
        Ok(())
    }

    /// **The freshness predicate, exactly:** a checkpoint issued at `a` (authority clock) is fresh at
    /// `r` (reader clock) iff `(a − r) ≤ 2s ∧ (r − a) ≤ F − 2s`.
    ///
    /// Safety: accepted ⇒ real age ≤ *F*. Liveness: real age ≤ *F* − 4*s* ⇒ accepted.
    pub fn is_fresh(&self, clock: ClockModel, issued_at_ms: u64, reader_now_ms: u64) -> bool {
        let (a, r) = (i128::from(issued_at_ms), i128::from(reader_now_ms));
        let two_s = 2 * i128::from(clock.skew_ms);
        (a - r) <= two_s && (r - a) <= i128::from(self.freshness_ms) - two_s
    }

    /// Is `issued_at_ms` further in the future than any honest clock allows?
    pub fn is_future_dated(&self, clock: ClockModel, issued_at_ms: u64, reader_now_ms: u64) -> bool {
        i128::from(issued_at_ms) - i128::from(reader_now_ms) > 2 * i128::from(clock.skew_ms)
    }
}

// ── revocation checkpoints ──────────────────────────────────────────────────────────────────

/// An authority's signed statement: *as of `issued_at_ms`, these appointments in `scope` are
/// revoked* — issued at least every *I*, **even when nothing is revoked**.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationCheckpoint {
    /// The establishing authority.
    pub authority: PrincipalId,
    /// The scope it covers. A checkpoint for scope X says nothing about scope Y.
    pub scope: String,
    /// Monotonic per `(authority, scope)`. A lower or equal `seq` adds nothing.
    pub seq: u64,
    /// When the authority issued it, on the authority's clock.
    pub issued_at_ms: u64,
    /// The appointments (terms) revoked in this scope.
    pub revoked: BTreeSet<TermId>,
}

impl RevocationCheckpoint {
    /// The exact bytes the authority signs. Tagged.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn lp(out: &mut Vec<u8>, b: &[u8]) {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        let mut out = Vec::new();
        lp(&mut out, b"mycelium.mandate/revocation-checkpoint/1");
        lp(&mut out, self.authority.as_str().as_bytes());
        lp(&mut out, self.scope.as_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&(self.revoked.len() as u32).to_le_bytes());
        for t in &self.revoked {
            lp(&mut out, t.as_str().as_bytes());
        }
        out
    }
}

/// A checkpoint with its authority's signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRevocationCheckpoint {
    /// The checkpoint.
    pub checkpoint: RevocationCheckpoint,
    /// The authority's signature over [`RevocationCheckpoint::canonical_bytes`].
    pub signature: Vec<u8>,
}

/// What a reader did with an offered checkpoint.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointOffer {
    /// Retained: the newest this reader holds for its `(authority, scope)`.
    Accepted,
    /// A `seq` no higher than the one held: a replay, which adds nothing and cannot refresh freshness.
    Replayed {
        /// The `seq` this reader holds.
        held: u64,
    },
    /// Dated further in the future than any honest clock allows: a clock fault or a forgery.
    FutureDated,
    /// The signature does not verify for the named authority.
    Unverifiable(UnverifiableReason),
}

/// A reader's revocation view: the newest verified checkpoint per `(authority, scope)`.
#[derive(Clone, Debug, Default)]
pub struct RevocationView {
    newest: BTreeMap<(PrincipalId, String), RevocationCheckpoint>,
}

/// Where a reader stands on revocation for one appointment.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevocationStanding {
    /// A fresh checkpoint does not list it.
    NotRevoked,
    /// A checkpoint lists it.
    Revoked,
    /// No checkpoint, or none fresh: the reader does not know. **Never read as "not revoked".**
    Unknown,
}

impl RevocationView {
    /// An empty view: every standing is `Unknown` until a checkpoint arrives.
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer a signed checkpoint.
    pub fn offer(
        &mut self,
        signed: &SignedRevocationCheckpoint,
        policy: &FreshnessPolicy,
        clock: ClockModel,
        now_ms: u64,
        members: &impl MemberKeySource,
        external: &TrustedExternalIssuers,
    ) -> CheckpointOffer {
        let c = &signed.checkpoint;
        let Some(authority) = IssuerId::new(c.authority.as_str()) else {
            return CheckpointOffer::Unverifiable(UnverifiableReason::UntrustedExternal);
        };
        match verify_signed_by(&authority, &c.canonical_bytes(), &signed.signature, members, external) {
            Authenticity::Current { .. } => {}
            Authenticity::Revoked { .. } => return CheckpointOffer::Unverifiable(UnverifiableReason::BadSignature),
            Authenticity::Unverifiable(r) => return CheckpointOffer::Unverifiable(r),
        }
        if policy.is_future_dated(clock, c.issued_at_ms, now_ms) {
            return CheckpointOffer::FutureDated;
        }
        let key = (c.authority.clone(), c.scope.clone());
        if let Some(held) = self.newest.get(&key)
            && c.seq <= held.seq
        {
            return CheckpointOffer::Replayed { held: held.seq };
        }
        self.newest.insert(key, c.clone());
        CheckpointOffer::Accepted
    }

    /// Where `term`, appointed by `authority` in `scope`, stands at `now_ms`.
    pub fn standing(
        &self,
        authority: &PrincipalId,
        scope: &str,
        term: &TermId,
        policy: &FreshnessPolicy,
        clock: ClockModel,
        now_ms: u64,
    ) -> RevocationStanding {
        let Some(c) = self.newest.get(&(authority.clone(), scope.to_string())) else {
            return RevocationStanding::Unknown;
        };
        if c.revoked.contains(term) {
            // A revocation, once seen, stands whatever the freshness: revocation is monotonic.
            return RevocationStanding::Revoked;
        }
        if !policy.is_fresh(clock, c.issued_at_ms, now_ms) {
            return RevocationStanding::Unknown;
        }
        RevocationStanding::NotRevoked
    }
}

// ── work, continuation and drain bounds ─────────────────────────────────────────────────────

/// The AE2 tier of the resource a gate protects.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceTier {
    /// A transaction the check sits inside: *HardPrevention*.
    Transactional,
    /// A single owner orders check and effect: *SelfImposedPrevention*.
    Serialised,
    /// No boundary: detection only. **Not admissible in the strict profile.**
    Advisory,
}

/// How long-running work continues once admitted.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continuation {
    /// Re-check authority at least every `interval_ms`; cancel on failure.
    ReauthorizeAt {
        /// The re-check interval.
        interval_ms: u64,
    },
    /// Run to completion within `max_duration_ms`. `enforced` says whether the **resource** enforces
    /// that limit (and can confirm termination). A declared but unenforced limit is not a bound.
    RunToCompletion {
        /// The permitted duration.
        max_duration_ms: u64,
        /// Whether the resource enforces it.
        enforced: bool,
    },
}

/// What an operation class declares about stopping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopContract {
    /// How work of this class continues.
    pub continuation: Continuation,
    /// Declared time from a cancellation request to the work stopping. `None` = not declared.
    pub cancellation_latency_ms: Option<u64>,
    /// Declared time to confirm a stop. `None` = the resource cannot confirm one.
    pub confirmation_latency_ms: Option<u64>,
}

/// The bound on T_drain for a class, or why there is none.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainBound {
    /// T_drain ≤ this many ms after the real expiry instant.
    Bounded(u64),
    /// No demonstrable bound. Excluded from A1's T_drain guarantee — never assumed bounded.
    Unbounded(&'static str),
}

impl StopContract {
    /// T_drain's bound for work of this class, under `clock`.
    ///
    /// - `ReauthorizeAt`: *s* + interval + cancellation latency + confirmation latency.
    /// - `RunToCompletion` (enforced): *s* + remaining permitted duration + confirmation latency.
    pub fn drain_bound(&self, clock: ClockModel, remaining_ms: u64) -> DrainBound {
        let Some(confirm) = self.confirmation_latency_ms else {
            return DrainBound::Unbounded("the resource cannot confirm a stop");
        };
        match self.continuation {
            Continuation::ReauthorizeAt { interval_ms } => match self.cancellation_latency_ms {
                Some(cancel) => DrainBound::Bounded(clock.skew_ms + interval_ms + cancel + confirm),
                None => DrainBound::Unbounded("no declared cancellation latency"),
            },
            Continuation::RunToCompletion { max_duration_ms, enforced } => {
                if !enforced {
                    return DrainBound::Unbounded("max_duration is not enforced by the resource");
                }
                DrainBound::Bounded(clock.skew_ms + remaining_ms.min(max_duration_ms) + confirm)
            }
        }
    }
}

/// Work admitted under a mandate, carrying the authority it runs under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedWork {
    /// The protected operation.
    pub operation: String,
    /// The mandate it runs under. `None` = it claims no mandate, which a protected operation may
    /// not do.
    pub mandate: Option<Mandate>,
    /// The latest this work may run, **never** past its mandate's `valid_until_ms`.
    pub not_after_ms: u64,
}

impl AuthorizedWork {
    /// Admit work for `operation` under `mandate`, wanting to run until `requested_not_after_ms`.
    /// The window is clamped to the mandate's own.
    pub fn new(operation: impl Into<String>, mandate: Option<Mandate>, requested_not_after_ms: u64) -> Self {
        let cap = mandate.as_ref().map_or(requested_not_after_ms, |m| m.valid_until_ms);
        Self { operation: operation.into(), mandate, not_after_ms: requested_not_after_ms.min(cap) }
    }

    /// Delegate part of this work. The child carries the **same** mandate and can never outlive its
    /// parent, whatever it asks for.
    pub fn delegate(&self, operation: impl Into<String>, requested_not_after_ms: u64) -> Self {
        Self {
            operation: operation.into(),
            mandate: self.mandate.clone(),
            not_after_ms: requested_not_after_ms.min(self.not_after_ms),
        }
    }
}

// ── the gate ────────────────────────────────────────────────────────────────────────────────

/// Why work may not run now.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionDenial {
    /// A protected operation carried no mandate.
    NoMandate,
    /// The resource's own check refused it: superseded, wrong scope, out of window, not enumerated.
    Refused(super::MandateRefusal),
    /// Past the work's own window (never later than its mandate's).
    WorkExpired,
    /// The appointment is revoked.
    Revoked,
    /// Revocation standing is unknown: no fresh checkpoint. Denied under the profile.
    RevocationUnknown,
}

/// A protected resource's execution gate: its authority, its tier, and the profile's time rules.
#[derive(Clone, Debug)]
pub struct ExecutionGate {
    resource: ResourceAuthority,
    clock: ClockModel,
    freshness: FreshnessPolicy,
}

impl ExecutionGate {
    /// A gate for `resource` of `tier` under the strict profile. **Refuses** an advisory resource,
    /// and parameters that break *F* > 4*s* or *I* + *D* ≤ *F* − 4*s*.
    pub fn strict(
        resource: ResourceAuthority,
        tier: ResourceTier,
        clock: ClockModel,
        freshness: FreshnessPolicy,
    ) -> Result<Self, ProfileError> {
        if tier == ResourceTier::Advisory {
            return Err(ProfileError::AdvisoryResource);
        }
        freshness.validate(clock)?;
        Ok(Self { resource, clock, freshness })
    }

    /// **May this work run now?** Call it at admission, at dequeue, at every retry, and at every
    /// re-authorisation checkpoint: authority is re-established each time, never inherited.
    pub fn check(&self, work: &AuthorizedWork, revocations: &RevocationView, now_ms: u64) -> Result<(), ExecutionDenial> {
        let Some(m) = &work.mandate else { return Err(ExecutionDenial::NoMandate) };
        if now_ms > work.not_after_ms {
            return Err(ExecutionDenial::WorkExpired);
        }
        self.resource.check(m, &work.operation, now_ms).map_err(ExecutionDenial::Refused)?;
        match revocations.standing(&m.established_by, &m.scope, &m.term, &self.freshness, self.clock, now_ms) {
            RevocationStanding::NotRevoked => Ok(()),
            RevocationStanding::Revoked => Err(ExecutionDenial::Revoked),
            RevocationStanding::Unknown => Err(ExecutionDenial::RevocationUnknown),
        }
    }

    /// The clock model this gate runs under.
    pub fn clock(&self) -> ClockModel {
        self.clock
    }
}

// ── measuring the stop ──────────────────────────────────────────────────────────────────────

/// One work item's stop, as three separately recorded moments. Reaching a checkpoint and
/// *requesting* cancellation does not prove the work, or its effects, stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StopRecord {
    /// When cancellation was requested.
    pub requested_ms: Option<u64>,
    /// When the work acknowledged it.
    pub acknowledged_ms: Option<u64>,
    /// When the resource confirmed the work stopped. `None` = **unconfirmed**, never "stopped".
    pub confirmed_ms: Option<u64>,
}

/// T_admit and T_drain for one expiry, measured separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrainReport {
    /// Time from the real expiry instant to the last admission. `None` = nothing was admitted after it.
    pub t_admit_ms: Option<u64>,
    /// Time from expiry to the last **confirmed** stop, over work that confirmed.
    pub t_drain_confirmed_ms: Option<u64>,
    /// Work whose stop was requested or acknowledged but never confirmed.
    pub unconfirmed: usize,
}

/// Build the report from the admissions seen and the stops recorded, for an expiry at `expiry_ms`.
pub fn drain_report(expiry_ms: u64, admissions_ms: &[u64], stops: &[StopRecord]) -> DrainReport {
    let t_admit_ms = admissions_ms.iter().filter(|t| **t >= expiry_ms).max().map(|t| t - expiry_ms);
    let t_drain_confirmed_ms =
        stops.iter().filter_map(|s| s.confirmed_ms).max().map(|t| t.saturating_sub(expiry_ms));
    let unconfirmed = stops.iter().filter(|s| s.confirmed_ms.is_none()).count();
    DrainReport { t_admit_ms, t_drain_confirmed_ms, unconfirmed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mandate::grant::tests::{mandate, world, World};
    use ed25519_dalek::SigningKey;

    const S: u64 = 100; // clock bound s
    const F: u64 = 10_000; // freshness F
    fn clock() -> ClockModel {
        ClockModel { skew_ms: S }
    }
    fn policy() -> FreshnessPolicy {
        FreshnessPolicy { freshness_ms: F, interval_ms: 4_000, delivery_ms: 1_000 }
    }

    fn checkpoint(w: &World, seq: u64, issued_at_ms: u64, revoked: &[&str]) -> SignedRevocationCheckpoint {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "cap/fleet".into(),
            seq,
            issued_at_ms,
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        let signature = mycelium_core::tls::sign_bytes(&w.authority, &c.canonical_bytes()).to_vec();
        SignedRevocationCheckpoint { checkpoint: c, signature }
    }

    fn gate() -> ExecutionGate {
        ExecutionGate::strict(ResourceAuthority::new("cap/fleet", 1), ResourceTier::Transactional, clock(), policy()).unwrap()
    }

    fn view_with(w: &World, cps: &[SignedRevocationCheckpoint], now: u64) -> RevocationView {
        let mut v = RevocationView::new();
        for c in cps {
            v.offer(c, &policy(), clock(), now, &w.members, &w.external);
        }
        v
    }

    fn work(w: &World) -> AuthorizedWork {
        AuthorizedWork::new("serve:x/y", Some(mandate(w, "cap/fleet", &["serve:x/y"], 1)), u64::MAX)
    }

    /// Work whose mandate outlives the freshness tests' horizon, so a denial there can only be about
    /// revocation freshness, never about the mandate's own window.
    fn long_work(w: &World) -> AuthorizedWork {
        let mut m = mandate(w, "cap/fleet", &["serve:x/y"], 1);
        m.valid_until_ms = 1_000_000;
        AuthorizedWork::new("serve:x/y", Some(m), u64::MAX)
    }

    // ── the profile's parameters ──

    #[test]
    fn the_profile_refuses_parameters_that_break_the_inequalities_and_advisory_resources() {
        assert!(matches!(FreshnessPolicy { freshness_ms: 400, ..policy() }.validate(clock()), Err(ProfileError::FreshnessTooShort { .. })));
        assert!(matches!(FreshnessPolicy { interval_ms: 9_000, ..policy() }.validate(clock()), Err(ProfileError::IntervalDoesNotFit { .. })));
        assert!(policy().validate(clock()).is_ok());
        assert_eq!(
            ExecutionGate::strict(ResourceAuthority::new("s", 1), ResourceTier::Advisory, clock(), policy()).err(),
            Some(ProfileError::AdvisoryResource)
        );
    }

    // ── the freshness predicate at both clock extremes ──

    /// Authority +s, reader −s: measured age understates real age by 2s. Real age F is accepted
    /// (safety holds at the edge); real age F + 1 ms is refused.
    #[test]
    fn safety_at_the_extreme_that_understates_age() {
        let t_issue = 1_000_000u64;
        let a = t_issue + S;
        let reader = |real_age: u64| t_issue + real_age - S;
        assert!(policy().is_fresh(clock(), a, reader(F)));
        assert!(!policy().is_fresh(clock(), a, reader(F + 1)));
    }

    /// Authority −s, reader +s: measured age overstates real age by 2s. Real age F − 4s is accepted
    /// (liveness holds at the edge).
    #[test]
    fn liveness_at_the_extreme_that_overstates_age() {
        let t_issue = 1_000_000u64;
        let a = t_issue - S;
        let r = t_issue + (F - 4 * S) + S;
        assert!(policy().is_fresh(clock(), a, r));
    }

    #[test]
    fn a_checkpoint_dated_beyond_2s_in_the_future_is_refused() {
        let w = world();
        let r = 1_000_000u64;
        let mut v = RevocationView::new();
        assert_eq!(v.offer(&checkpoint(&w, 1, r + 2 * S, &[]), &policy(), clock(), r, &w.members, &w.external), CheckpointOffer::Accepted);
        assert_eq!(v.offer(&checkpoint(&w, 2, r + 2 * S + 1, &[]), &policy(), clock(), r, &w.members, &w.external), CheckpointOffer::FutureDated);
    }

    // ── revocation freshness ──

    /// **A replayed old checkpoint never refreshes freshness.** Re-delivering seq 1 adds nothing, and
    /// authority is denied once the genuine checkpoint ages out.
    #[test]
    fn a_replayed_checkpoint_does_not_refresh_freshness() {
        let w = world();
        let cp = checkpoint(&w, 1, 1_000, &[]);
        let mut v = view_with(&w, std::slice::from_ref(&cp), 1_000);
        assert!(gate().check(&long_work(&w), &v, 2_000).is_ok());
        assert_eq!(v.offer(&cp, &policy(), clock(), 9_000, &w.members, &w.external), CheckpointOffer::Replayed { held: 1 });
        assert_eq!(gate().check(&long_work(&w), &v, 1_000 + F), Err(ExecutionDenial::RevocationUnknown));
    }

    /// **Silence is not "nothing revoked".** With no checkpoint at all, and in a partition where
    /// none arrives, protected work is denied.
    #[test]
    fn silence_and_partition_deny() {
        let w = world();
        assert_eq!(gate().check(&long_work(&w), &RevocationView::new(), 1_000), Err(ExecutionDenial::RevocationUnknown));
        let v = view_with(&w, &[checkpoint(&w, 1, 1_000, &[])], 1_000);
        assert!(gate().check(&long_work(&w), &v, 5_000).is_ok());
        assert_eq!(gate().check(&long_work(&w), &v, 1_000 + F), Err(ExecutionDenial::RevocationUnknown), "partition: no new checkpoint");
    }

    #[test]
    fn a_checkpoint_for_one_scope_does_not_refresh_another() {
        let w = world();
        let mut other = checkpoint(&w, 1, 1_000, &[]);
        other.checkpoint.scope = "cap/other".into();
        other.signature = mycelium_core::tls::sign_bytes(&w.authority, &other.checkpoint.canonical_bytes()).to_vec();
        let v = view_with(&w, &[other], 1_000);
        assert_eq!(gate().check(&work(&w), &v, 2_000), Err(ExecutionDenial::RevocationUnknown));
    }

    #[test]
    fn a_revoked_appointment_is_denied() {
        let w = world();
        let v = view_with(&w, &[checkpoint(&w, 1, 1_000, &["t1"])], 1_000);
        assert_eq!(gate().check(&work(&w), &v, 2_000), Err(ExecutionDenial::Revoked));
    }

    #[test]
    fn a_forged_checkpoint_is_not_accepted() {
        let w = world();
        let mut cp = checkpoint(&w, 1, 1_000, &[]);
        cp.signature = mycelium_core::tls::sign_bytes(&SigningKey::from_bytes(&[88u8; 32]), &cp.checkpoint.canonical_bytes()).to_vec();
        let mut v = RevocationView::new();
        assert_eq!(v.offer(&cp, &policy(), clock(), 1_000, &w.members, &w.external), CheckpointOffer::Unverifiable(UnverifiableReason::BadSignature));
    }

    // ── the execution contract ──

    /// A protected operation without a mandate is refused.
    #[test]
    fn a_protected_operation_without_a_mandate_is_refused() {
        let w = world();
        let v = view_with(&w, &[checkpoint(&w, 1, 1_000, &[])], 1_000);
        let unmandated = AuthorizedWork::new("serve:x/y", None, u64::MAX);
        assert_eq!(gate().check(&unmandated, &v, 2_000), Err(ExecutionDenial::NoMandate));
    }

    /// **Admitted work is re-checked at execution.** Admitted before expiry, dequeued or retried
    /// after it: refused.
    #[test]
    fn queued_and_retried_work_is_refused_after_expiry() {
        let w = world();
        let fresh_near = |t: u64| view_with(&w, &[checkpoint(&w, 1, t, &[])], t);
        let job = work(&w);
        assert!(gate().check(&job, &fresh_near(9_000), 9_500).is_ok(), "admitted before expiry (valid_until 10_000)");
        assert!(matches!(gate().check(&job, &fresh_near(10_400), 10_500), Err(ExecutionDenial::WorkExpired | ExecutionDenial::Refused(_))), "dequeued after expiry");
        assert!(gate().check(&job, &fresh_near(10_900), 11_000).is_err(), "retried after expiry");
    }

    /// **Delegation never extends authority.** A child asking to run past its parent is clamped.
    #[test]
    fn delegated_work_cannot_outlive_its_parent() {
        let w = world();
        let parent = AuthorizedWork::new("serve:x/y", Some(mandate(&w, "cap/fleet", &["serve:x/y"], 1)), 8_000);
        let child = parent.delegate("serve:x/y", 50_000);
        assert_eq!(child.not_after_ms, 8_000);
        assert_eq!(parent.mandate, child.mandate);
        let v = view_with(&w, &[checkpoint(&w, 1, 8_500, &[])], 8_500);
        assert_eq!(gate().check(&child, &v, 8_600), Err(ExecutionDenial::WorkExpired));
    }

    /// Work can never be admitted past its mandate's window, whatever it requests.
    #[test]
    fn work_is_clamped_to_its_mandates_window() {
        let w = world();
        assert_eq!(work(&w).not_after_ms, 10_000);
    }

    /// A `ReauthorizeAt` task is cancelled at its first re-check after expiry.
    #[test]
    fn a_reauthorizing_task_fails_its_first_check_after_expiry() {
        let w = world();
        let job = work(&w);
        let interval = 2_000;
        let mut t = 6_000;
        let first_failure = loop {
            let v = view_with(&w, &[checkpoint(&w, 1, t, &[])], t);
            if gate().check(&job, &v, t).is_err() {
                break t;
            }
            t += interval;
        };
        assert!(first_failure > 10_000 && first_failure <= 10_000 + interval, "{first_failure}");
    }

    // ── drain bounds and measurement ──

    #[test]
    fn drain_bounds_per_continuation_and_unbounded_classes() {
        let reauth = StopContract { continuation: Continuation::ReauthorizeAt { interval_ms: 2_000 }, cancellation_latency_ms: Some(300), confirmation_latency_ms: Some(200) };
        assert_eq!(reauth.drain_bound(clock(), 0), DrainBound::Bounded(S + 2_000 + 300 + 200));
        let run = StopContract { continuation: Continuation::RunToCompletion { max_duration_ms: 5_000, enforced: true }, cancellation_latency_ms: None, confirmation_latency_ms: Some(200) };
        assert_eq!(run.drain_bound(clock(), 1_500), DrainBound::Bounded(S + 1_500 + 200), "remaining permitted duration, not the maximum");
        let unenforced = StopContract { continuation: Continuation::RunToCompletion { max_duration_ms: 5_000, enforced: false }, ..run };
        assert!(matches!(unenforced.drain_bound(clock(), 1_500), DrainBound::Unbounded(_)));
        let unconfirmable = StopContract { confirmation_latency_ms: None, ..reauth };
        assert!(matches!(unconfirmable.drain_bound(clock(), 0), DrainBound::Unbounded(_)));
    }

    /// T_admit and T_drain are reported separately, and a stop never confirmed is **unconfirmed**,
    /// never counted as stopped.
    #[test]
    fn the_drain_report_keeps_requested_acknowledged_and_confirmed_apart() {
        let stops = [
            StopRecord { requested_ms: Some(10_100), acknowledged_ms: Some(10_150), confirmed_ms: Some(10_400) },
            StopRecord { requested_ms: Some(10_100), acknowledged_ms: Some(10_200), confirmed_ms: None },
        ];
        let r = drain_report(10_000, &[9_000, 10_050], &stops);
        assert_eq!(r, DrainReport { t_admit_ms: Some(50), t_drain_confirmed_ms: Some(400), unconfirmed: 1 });
    }
}
