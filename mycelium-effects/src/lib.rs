//! # mycelium-effects — the destination-commit receipt, as a reference destination
//!
//! The v3 contracts axis (item 1, `docs/design/contracts-receipts.md`) names four receipts and keeps
//! them strictly apart: *local application* · *local sync* · *replica sync* · **destination commit**.
//! The substrate provides the first three. It **never** provides the fourth — *at-least-once +
//! idempotent merge = exactly-once effect* is a rule about the caller's resource, and a resource is
//! the only thing that can say whether an effect happened there.
//!
//! This crate is that last rung made generic and inspectable (item 1 PR 5): a **transactional
//! reference destination** whose dedup row — keyed by the caller's `operation_id` — and business
//! change commit **in one transaction**, and which answers with the
//! [`DestinationCommit`](mycelium::DestinationCommit) receipt saying which attempt this was.
//!
//! ## Why this is justified where a shared tracker was not
//!
//! `docs/design/exactly-once-effect.md` declined, with evidence, to extract a shared in-flight
//! tracker across the tuple space and the blackboard: their clocks and persistence differ, and a
//! shared kernel would couple crates that evolve apart. The receipts record (§6) says why this is
//! different: the in-tree users prove the pattern with **bespoke** idempotent merges, none of which
//! yields a receipt a third party can read. A destination sits *beyond* both companions, at the
//! resource the caller already trusts, and couples neither. What it adds is the transaction
//! boundary, the dedup row, and the receipt.
//!
//! ## The contract, in the receipts record's words
//!
//! - **Same `operation_id`, same content, any attempt:** [`DedupOutcome::Replayed`] — nothing is
//!   applied twice. The retry may come from another worker, after a restart, a week later.
//! - **Same `operation_id`, different content:** [`EffectRefusal::Conflict`]. A retry that changed
//!   its mind is not a retry, and a destination that quietly took the second version would have
//!   two effects under one identity.
//! - **The business change fails:** [`EffectRefusal::Failed`], and **no dedup row exists** — the
//!   two commit together or not at all, so a later retry is `Fresh`, not a false `Replayed`.
//! - **The destination does not answer in time:** [`EffectRefusal::DeliveryUnknown`] — *never*
//!   "nothing happened". The effect may have committed after the caller stopped waiting, and the
//!   retry that resolves it is exactly the `Replayed` case above.
//!
//! ## What this crate is not
//!
//! Not the tuple space's `complete` (the pipeline's own receipt, which proves the item was
//! acknowledged and the next stage queued — not that a business transaction happened), and not the
//! *in-process* half — the local fiber runtime the plan's §13 describes — which is beyond v3 (D34).
//! One reference destination, one vocabulary. The `tuple-space` feature adds the consumer that
//! composes the two ([`tuple_consumer`]): effect first, acknowledgement second, so the pipeline's
//! receipt never stands in for the destination's.
//!
//! ## Beyond the fleet: the side-effect contract
//!
//! A destination outside the fleet gives no transaction to commit a dedup row in. [`external`]
//! (adopter-surface X5) is the contract for that case: the destination's class — idempotent,
//! compensatable, or irreversible — **declared before execution** and recorded on the prepared
//! record; on `DeliveryUnknown` a reconciliation probe before any retry, at most one retry, never
//! against a destination that did not declare itself idempotent, and a probe budget past which the
//! record is unresolved and escalated; a compensation as a new, authorised effect whose record names
//! the one it compensates; every forbidden move refused by name and counted; the apply and the probe
//! behind the replay kernel's `Input` seam. [`http`] is the reference HTTP shape.
//!
//! **Since adopter-surface X9 the contract lives in the substrate** — [`mycelium::effects`] — so the
//! published node binary can run it (`mycelium test --faults recovery`), and this crate **re-exports it
//! unchanged**: every `mycelium_effects::` path on this page still resolves. What is this crate's own: the
//! SQLite reference destination ([`sqlite`]), the counting wrapper ([`Counting`]) and the tuple-space
//! consumer. The reconciler's ledger is now a node-local journal, not a SQLite file; `rusqlite` stays here.

#![deny(unsafe_code)]

pub mod counting;
pub mod sqlite;
/// The side-effect contract and the reference HTTP shape, re-exported from the substrate (adopter-surface X9).
pub use mycelium::effects::{external, http};
#[cfg(feature = "tuple-space")]
pub mod tuple_consumer;

pub use mycelium::effects::{check_composition, Composition, CompositionLeg, Effect, EffectRefusal};
pub use mycelium::{AttemptId, DedupOutcome, DestinationCommit, OperationId, content_hash};
pub use sqlite::SqliteDestination;
pub use counting::{Counting, ExternalRefusalCounts, ExternalRefusalSnapshot, RefusalCounts, RefusalSnapshot};
pub use external::{
    CompensationPermit, EffectClass, EffectState, ExternalDestination, ExternalEffect, ExternalRefusal, IdempotencyKey,
    LedgerRecord, ProbeAnswer, ProbeBudget, Reconciler,
};
pub use http::HttpDestination;
pub use mycelium::EgressPolicy;

use std::sync::Arc;
use std::time::Duration;

pub use mycelium::mandate::{Mandate, MandateRefusal, PrincipalId, ResourceAuthority, TermId};

/// An effect with its composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedEffect {
    pub effect: Effect,
    pub composition: Composition,
}

/// A destination that can commit an effect exactly once and say so.
///
/// `apply` takes `&self`: a destination is shared between appliers, and it is the destination's
/// transaction — not a Rust borrow — that serialises them.
pub trait EffectDestination: Send + Sync {
    /// The destination's own identity, carried in every receipt. Not a node id.
    fn identity(&self) -> &str;

    /// Apply `effect` and record its dedup result **in one transaction**, returning the receipt.
    fn apply(&self, effect: &Effect) -> Result<DestinationCommit, EffectRefusal>;

    /// Apply a **composed** effect: refuse unless its composition holds at this resource
    /// (`check_composition` against `authority` at `now_ms`), then [`apply`](Self::apply).
    ///
    /// The check and the transaction are two steps, so the same check-then-act window every
    /// resource in this tree has (`authority-at-execution.md`) exists here: a process that pauses
    /// between them after an epoch is superseded acts late by the pause. Narrowed and stated, not
    /// eliminated; a destination that can take the authority *inside* its transaction should
    /// override this.
    fn apply_composed(
        &self,
        composed: &ComposedEffect,
        authority: &ResourceAuthority,
        now_ms: u64,
    ) -> Result<DestinationCommit, EffectRefusal> {
        check_composition(&composed.composition, authority, now_ms)?;
        self.apply(&composed.effect)
    }
}

/// Apply with a deadline, mapping an overrun to [`EffectRefusal::DeliveryUnknown`].
///
/// The apply runs on a blocking thread; if the deadline passes first, **the apply is not
/// cancelled** — it may still commit — which is precisely why the answer is *unknown* and not
/// *failed*. The caller's next move is to retry with the same `operation_id` and let the dedup row
/// say what happened.
pub async fn apply_within<D: EffectDestination + 'static>(
    destination: Arc<D>,
    effect: Effect,
    deadline: Duration,
) -> Result<DestinationCommit, EffectRefusal> {
    let task = tokio::task::spawn_blocking(move || destination.apply(&effect));
    match tokio::time::timeout(deadline, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(EffectRefusal::Failed(format!("the apply task did not complete: {join}"))),
        Err(_elapsed) => Err(EffectRefusal::DeliveryUnknown),
    }
}
