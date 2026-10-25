//! The refusal counters (zero-gaps Z6, D6; the side-effect contract's reasons, X5).
//!
//! `apply_composed` refuses by leg and `apply` refuses by kind; until these counters nothing counted
//! either — the evidence journal records a refusal the gateway made, not one the destination made.
//! [`RefusalCounts`] counts every `Err` by kind and leg and every commit, and answers a [`RefusalSnapshot`];
//! the [`Reconciler`](super::Reconciler) counts its own refusals by reason into the same value. The counter
//! is a value: it couples to nothing. With feature `metrics` it also increments
//! `mycelium_effects_refusals_total{kind, leg}` (the metric keeps the companion's name: the counter moved,
//! the series did not). `mycelium-effects`' `Counting<D>` wraps an `EffectDestination` with one.

use std::sync::atomic::{AtomicU64, Ordering};

use super::external::ExternalRefusal;
use super::{CompositionLeg, EffectRefusal};

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
            // Both enums are this module's, so the match is exhaustive here: a variant added to
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
            ExternalRefusal::IrreversibleUnobservable | ExternalRefusal::LeaveOfObservable { .. } => &x.irreversible_unobservable,
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
