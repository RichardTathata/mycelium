//! A destination whose refusals are **counted** (zero-gaps Z6, D6).
//!
//! `apply_composed` refuses by leg and `apply` refuses by kind; until this module nothing counted
//! either — the evidence journal records a refusal the gateway made, not one the destination made.
//! [`Counting`] wraps any [`EffectDestination`], counts every `Err` by kind and leg and every
//! commit, and answers a [`RefusalSnapshot`]. The counter is a value: it couples to nothing. With
//! feature `metrics` it also increments `mycelium_effects_refusals_total{kind, leg}`. The counters
//! themselves are the substrate's since adopter-surface X9 (`mycelium::effects::counting`), re-exported here.

use std::sync::Arc;

pub use mycelium::effects::counting::{ExternalRefusalCounts, ExternalRefusalSnapshot, RefusalCounts, RefusalSnapshot};

use crate::{ComposedEffect, DestinationCommit, Effect, EffectDestination, EffectRefusal, ResourceAuthority};

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
