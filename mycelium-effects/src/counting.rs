//! A destination whose refusals are **counted** (zero-gaps Z6, D6).
//!
//! `apply_composed` refuses by leg and `apply` refuses by kind; until this module nothing counted
//! either — the evidence journal records a refusal the gateway made, not one the destination made.
//! [`Counting`] wraps any [`EffectDestination`], counts every `Err` by kind and leg and every
//! commit, and answers a [`RefusalSnapshot`]. The counter is a value: it couples to nothing. With
//! feature `metrics` it also increments `mycelium_effects_refusals_total{kind, leg}`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
}

impl RefusalSnapshot {
    /// Every refusal, whatever its kind.
    pub fn refusals(&self) -> u64 {
        self.conflict + self.failed + self.delivery_unknown + self.unauthorised_attribution + self.unauthorised_authority
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
