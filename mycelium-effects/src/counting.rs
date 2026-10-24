//! A destination whose refusals are **counted** (zero-gaps Z6, D6).
//!
//! `apply_composed` refuses by leg and `apply` refuses by kind; until this module nothing counted
//! either — the evidence journal records a refusal the gateway made, not one the destination made.
//! [`Counting`] wraps any [`EffectDestination`], counts every `Err` by kind and leg and every
//! commit, and answers a [`RefusalSnapshot`]. The counter is a value: it couples to nothing. With
//! feature `metrics` it also increments `mycelium_effects_refusals_total{kind, leg}`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::external::ExternalRefusal;
use crate::{ComposedEffect, CompositionLeg, DestinationCommit, Effect, EffectDestination, EffectRefusal, ResourceAuthority};

/// The counts, by refusal kind and composition leg, plus commits. Atomics, so a shared destination
/// counts from every applier.
#[derive(Debug, Default)]
pub struct RefusalCounts {
    commits:                  AtomicU64,
    conflict:                 AtomicU64,
    failed:                   AtomicU64,
    delivery_unknown:         AtomicU64,
    unauthorised_attribution: AtomicU64,
    unauthorised_authority:   AtomicU64,
    external:                 ExternalRefusalCounts,
}

/// The side-effect contract's refusals (adopter-surface X5), by reason — each a move the record's class
/// or state forbade, refused before the destination was reached.
#[derive(Debug, Default)]
pub struct ExternalRefusalCounts {
    egress_denied:              AtomicU64,
    retry_without_probe:        AtomicU64,
    second_retry:               AtomicU64,
    retry_not_idempotent:       AtomicU64,
    probe_budget_exhausted:     AtomicU64,
    unresolved:                 AtomicU64,
    irreversible_unobservable:  AtomicU64,
    wrong_state:                AtomicU64,
    no_compensation:            AtomicU64,
    compensation_not_permitted: AtomicU64,
    declaration_mismatch:       AtomicU64,
    other:                      AtomicU64,
}

/// A point-in-time copy of [`ExternalRefusalCounts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalRefusalSnapshot {
    pub egress_denied:              u64,
    pub retry_without_probe:        u64,
    pub second_retry:               u64,
    pub retry_not_idempotent:       u64,
    pub probe_budget_exhausted:     u64,
    pub unresolved:                 u64,
    pub irreversible_unobservable:  u64,
    pub wrong_state:                u64,
    pub no_compensation:            u64,
    pub compensation_not_permitted: u64,
    pub declaration_mismatch:       u64,
    /// `not_recorded` and `ledger` — the reconciler's own, not the contract's.
    pub other:                      u64,
}

impl ExternalRefusalSnapshot {
    /// Every contract refusal.
    pub fn total(&self) -> u64 {
        self.egress_denied + self.retry_without_probe + self.second_retry + self.retry_not_idempotent + self.probe_budget_exhausted
            + self.unresolved + self.irreversible_unobservable + self.wrong_state + self.no_compensation + self.compensation_not_permitted
            + self.declaration_mismatch + self.other
    }
}

/// A point-in-time copy of [`RefusalCounts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefusalSnapshot {
    pub commits:                  u64,
    pub conflict:                 u64,
    pub failed:                   u64,
    pub delivery_unknown:         u64,
    pub unauthorised_attribution: u64,
    pub unauthorised_authority:   u64,
    /// The side-effect contract's refusals (X5).
    pub external:                 ExternalRefusalSnapshot,
}

impl RefusalSnapshot {
    /// Every refusal, whatever its kind — the destination's and the contract's.
    pub fn refusals(&self) -> u64 {
        self.conflict + self.failed + self.delivery_unknown + self.unauthorised_attribution + self.unauthorised_authority + self.external.total()
    }
}

impl RefusalCounts {
    /// Count one refusal.
    pub fn record(&self, refusal: &EffectRefusal) {
        let (slot, kind, leg): (&AtomicU64, &'static str, &'static str) = match refusal {
            EffectRefusal::Conflict { .. }    => (&self.conflict, "conflict", "-"),
            EffectRefusal::Failed(_)          => (&self.failed, "failed", "-"),
            EffectRefusal::DeliveryUnknown    => (&self.delivery_unknown, "delivery_unknown", "-"),
            EffectRefusal::Unauthorised { leg: CompositionLeg::Attribution, .. } => (&self.unauthorised_attribution, "unauthorised", "attribution"),
            EffectRefusal::Unauthorised { leg: CompositionLeg::Authority, .. }   => (&self.unauthorised_authority, "unauthorised", "authority"),
            // Both enums are this crate's, so the match is exhaustive here: a variant added to
            // either is a compile error in this file — a refusal is never silently uncounted.
        };
        slot.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_effects_refusals_total", "kind" => kind, "leg" => leg).increment(1);
        #[cfg(not(feature = "metrics"))]
        let _ = (kind, leg);
    }

    /// Count one refusal of the side-effect contract (X5). `Effect(_)` is counted by [`record`](Self::record)
    /// at the point it was answered, not here.
    pub fn record_external(&self, refusal: &ExternalRefusal) {
        let x = &self.external;
        let slot = match refusal {
            ExternalRefusal::Effect(_) => return,
            ExternalRefusal::EgressDenied { .. } => &x.egress_denied,
            ExternalRefusal::RetryWithoutProbe => &x.retry_without_probe,
            ExternalRefusal::SecondRetry => &x.second_retry,
            ExternalRefusal::RetryNotIdempotent { .. } => &x.retry_not_idempotent,
            ExternalRefusal::ProbeBudgetExhausted { .. } => &x.probe_budget_exhausted,
            ExternalRefusal::Unresolved => &x.unresolved,
            ExternalRefusal::IrreversibleUnobservable => &x.irreversible_unobservable,
            ExternalRefusal::State { .. } => &x.wrong_state,
            ExternalRefusal::NoCompensation { .. } => &x.no_compensation,
            ExternalRefusal::CompensationNotPermitted(_) => &x.compensation_not_permitted,
            ExternalRefusal::DeclarationMismatch { .. } => &x.declaration_mismatch,
            // `NotRecorded`, `Ledger`, and any variant added later: counted, never silently dropped.
            _ => &x.other,
        };
        slot.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_effects_refusals_total", "kind" => "external", "leg" => refusal.reason()).increment(1);
    }

    /// Count one commit (fresh or replayed — both are *the effect is at the destination*).
    pub fn record_commit(&self) {
        self.commits.fetch_add(1, Ordering::Relaxed);
    }

    /// The counts now.
    pub fn snapshot(&self) -> RefusalSnapshot {
        RefusalSnapshot {
            commits:                  self.commits.load(Ordering::Relaxed),
            conflict:                 self.conflict.load(Ordering::Relaxed),
            failed:                   self.failed.load(Ordering::Relaxed),
            delivery_unknown:         self.delivery_unknown.load(Ordering::Relaxed),
            unauthorised_attribution: self.unauthorised_attribution.load(Ordering::Relaxed),
            unauthorised_authority:   self.unauthorised_authority.load(Ordering::Relaxed),
            external: ExternalRefusalSnapshot {
                egress_denied:              self.external.egress_denied.load(Ordering::Relaxed),
                retry_without_probe:        self.external.retry_without_probe.load(Ordering::Relaxed),
                second_retry:               self.external.second_retry.load(Ordering::Relaxed),
                retry_not_idempotent:       self.external.retry_not_idempotent.load(Ordering::Relaxed),
                probe_budget_exhausted:     self.external.probe_budget_exhausted.load(Ordering::Relaxed),
                unresolved:                 self.external.unresolved.load(Ordering::Relaxed),
                irreversible_unobservable:  self.external.irreversible_unobservable.load(Ordering::Relaxed),
                wrong_state:                self.external.wrong_state.load(Ordering::Relaxed),
                no_compensation:            self.external.no_compensation.load(Ordering::Relaxed),
                compensation_not_permitted: self.external.compensation_not_permitted.load(Ordering::Relaxed),
                declaration_mismatch:       self.external.declaration_mismatch.load(Ordering::Relaxed),
                other:                      self.external.other.load(Ordering::Relaxed),
            },
        }
    }
}

/// An [`EffectDestination`] that counts. Transparent otherwise: the identity, the receipts and the
/// refusals are the inner destination's.
pub struct Counting<D> {
    inner:  D,
    counts: Arc<RefusalCounts>,
}

impl<D: EffectDestination> Counting<D> {
    /// Wrap `inner` with a fresh counter.
    pub fn new(inner: D) -> Self {
        Self { inner, counts: Arc::new(RefusalCounts::default()) }
    }

    /// Wrap `inner`, sharing a counter (several destinations, one ledger of refusals).
    #[deprecated(since = "2.33.0", note = "no caller, test, SDK or document in the repository references it (post-360 hardening row J, 2026-10-12); kept on the 2.x line — open an issue if you use it and it will be undeprecated")]
    pub fn with_counts(inner: D, counts: Arc<RefusalCounts>) -> Self {
        Self { inner, counts }
    }

    /// The counts now.
    pub fn counts(&self) -> RefusalSnapshot {
        self.counts.snapshot()
    }

    /// The shared counter, for a reader that outlives this wrapper.
    pub fn shared(&self) -> Arc<RefusalCounts> {
        Arc::clone(&self.counts)
    }

    /// The wrapped destination.
    pub fn inner(&self) -> &D {
        &self.inner
    }

    fn observe(&self, r: Result<DestinationCommit, EffectRefusal>) -> Result<DestinationCommit, EffectRefusal> {
        match &r {
            Ok(_) => self.counts.record_commit(),
            Err(e) => self.counts.record(e),
        }
        r
    }
}

impl<D: EffectDestination> EffectDestination for Counting<D> {
    fn identity(&self) -> &str {
        self.inner.identity()
    }

    fn apply(&self, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        self.observe(self.inner.apply(effect))
    }

    // Delegated to the inner destination's own `apply_composed` (which may take the authority
    // inside its transaction), not re-derived from `apply`, so a refusal is counted exactly once.
    fn apply_composed(&self, composed: &ComposedEffect, authority: &ResourceAuthority, now_ms: u64) -> Result<DestinationCommit, EffectRefusal> {
        self.observe(self.inner.apply_composed(composed, authority, now_ms))
    }
}
