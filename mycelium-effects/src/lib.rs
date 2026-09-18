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

pub mod sqlite;
#[cfg(feature = "tuple-space")]
pub mod tuple_consumer;

pub use mycelium::{AttemptId, DedupOutcome, DestinationCommit, OperationId, content_hash};
pub use sqlite::SqliteDestination;

use std::sync::Arc;
use std::time::Duration;

/// What a destination is asked to do: apply `payload` under `operation_id`, exactly once.
///
/// `content_hash` is the caller's, computed over the payload with [`content_hash`] so a retry can
/// be told from a change of mind. `attempt_id` is *this* delivery; the dedup row remembers the
/// first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// The logical operation — stable across retries and worker replacement.
    pub operation_id: OperationId,
    /// This delivery attempt.
    pub attempt_id: AttemptId,
    /// The payload's content hash, the divergence detector for *same identity, different content*.
    pub content_hash: u64,
    /// What to apply. The destination's handler decides what it means.
    pub payload: Vec<u8>,
}

impl Effect {
    /// An effect whose hash is computed here, so the two cannot disagree — with the receipt
    /// vocabulary's own [`content_hash`], keyed by the operation and over the payload. One hash for
    /// the whole axis: it is specified and golden-pinned, so a retry on another build hashes the
    /// same, and a divergence is a divergence and not a version skew.
    pub fn new(operation_id: OperationId, attempt_id: AttemptId, payload: Vec<u8>) -> Self {
        let content_hash = content_hash(operation_id.as_str(), &payload, false);
        Self { operation_id, attempt_id, content_hash, payload }
    }
}

/// Why an effect was not committed. **Each names what is true afterwards.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectRefusal {
    /// This `operation_id` was committed with **different content**. Nothing was applied, and
    /// the destination still holds the first version.
    Conflict {
        /// The operation.
        operation_id: OperationId,
        /// What the destination committed.
        committed_hash: u64,
        /// What this attempt presented.
        presented_hash: u64,
    },
    /// The transaction failed and was rolled back: **no business change and no dedup row.** A
    /// retry starts clean.
    Failed(String),
    /// The destination did not answer within the deadline. The effect's fate is **unknown** — it
    /// may commit after this returns — which is a different claim from failure, and the reason
    /// a retry resolves it as `Replayed` or `Fresh` rather than being refused.
    DeliveryUnknown,
}

impl std::fmt::Display for EffectRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { operation_id, committed_hash, presented_hash } => write!(
                f,
                "operation {operation_id} was committed with content {committed_hash:#x}; this \
                 attempt presented {presented_hash:#x} — a retry that changed its content is not a retry"
            ),
            Self::Failed(e) => write!(f, "the destination's transaction failed and was rolled back: {e}"),
            Self::DeliveryUnknown => {
                f.write_str("the destination did not answer in time; the effect's fate is unknown")
            }
        }
    }
}
impl std::error::Error for EffectRefusal {}

/// A destination that can commit an effect exactly once and say so.
///
/// `apply` takes `&self`: a destination is shared between appliers, and it is the destination's
/// transaction — not a Rust borrow — that serialises them.
pub trait EffectDestination: Send + Sync {
    /// The destination's own identity, carried in every receipt. Not a node id.
    fn identity(&self) -> &str;

    /// Apply `effect` and record its dedup result **in one transaction**, returning the receipt.
    fn apply(&self, effect: &Effect) -> Result<DestinationCommit, EffectRefusal>;
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
