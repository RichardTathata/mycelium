//! The tuple-space consumer with effect recovery (item 1 PR 6).
//!
//! The receipts record's §7 says what the tuple space's `complete` is and is not: *the pipeline's
//! own receipt* — the item was acknowledged and the next stage queued, atomically, at the primary.
//! It neither performs nor verifies the consumer's business transaction, and a worker that died
//! after acting but before `complete` is re-delivered by the expired lease, so **the second worker
//! acts again**. Idempotency makes that retry *safe*; it does not prove the effect *happened*.
//!
//! This consumer puts the two receipts in the only order that closes the gap:
//!
//! 1. **`take`** the item — a claim under a lease;
//! 2. **commit its effect** at an [`EffectDestination`], under an `operation_id` derived from the
//!    item (`tuple/{namespace}/{stage}/{id}`), and wait for the destination-commit receipt;
//! 3. only then **`ack`** (or **`complete`**) — the pipeline's receipt, after the destination's.
//!
//! A worker that dies between 2 and 3 leaves the item in flight. The lease expires — or the primary
//! restarts over its WAL, which re-queues every unacknowledged item with its id intact — and the
//! item is delivered again. The second worker's apply comes back **`Replayed`**: the destination
//! remembers the `operation_id`, applies nothing, and the consumer acknowledges an effect that
//! happened exactly once. That is the recovery, and it is a receipt, not a hope.
//!
//! # What a refusal does
//!
//! An effect that is refused is **not acknowledged** — the item stays in flight and will be
//! re-delivered. For [`EffectRefusal::DeliveryUnknown`] and [`EffectRefusal::Failed`] that is the
//! right outcome: the retry resolves the first as `Replayed` or `Fresh`, and starts the second
//! clean. For [`EffectRefusal::Conflict`] it is a **poison pill** — the same id keeps presenting
//! different content and will be refused on every delivery. The consumer surfaces it and does not
//! decide; dead-lettering is the caller's policy, because acknowledging it here would make a
//! conflicting effect vanish, which is the one thing this crate exists to prevent.

use crate::{Effect, EffectDestination, EffectRefusal, apply_within};
use bytes::Bytes;
use mycelium::{AttemptId, DestinationCommit, OperationId};
use mycelium_tuple_space::{TupleError, TupleSpace};
use std::sync::Arc;
use std::time::Duration;

/// A worker over one tuple space and one destination.
pub struct TupleConsumer<D: EffectDestination + 'static> {
    space:       Arc<TupleSpace>,
    destination: Arc<D>,
    namespace:   String,
}

/// One item consumed: its tuple id, and the destination's receipt for its effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consumed {
    /// The tuple id, as `take` returned it.
    pub id: u64,
    /// What the destination said — `Fresh`, or `Replayed` for a re-delivered item whose effect had
    /// already been committed.
    pub receipt: DestinationCommit,
}

/// Why an item was not consumed. In every case the item is **not acknowledged**.
#[derive(Debug)]
pub enum ConsumeError {
    /// The tuple space refused or timed out. Nothing was taken, or the take is still in flight.
    Tuple(TupleError),
    /// The destination refused the effect. The item stays in flight for re-delivery; see the
    /// module docs for which refusals a retry resolves and which it does not.
    Effect(EffectRefusal),
}

impl std::fmt::Display for ConsumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tuple(e) => write!(f, "tuple space: {e}"),
            Self::Effect(e) => write!(f, "effect not committed, item left in flight: {e}"),
        }
    }
}
impl std::error::Error for ConsumeError {}

impl<D: EffectDestination + 'static> TupleConsumer<D> {
    /// A consumer over `space` and `destination`. `namespace` names the space in every
    /// `operation_id`, so two spaces' items can never share one.
    pub fn new(space: Arc<TupleSpace>, destination: Arc<D>, namespace: impl Into<String>) -> Self {
        Self { space, destination, namespace: namespace.into() }
    }

    /// The operation an item is committed under: `tuple/{namespace}/{stage}/{id}`. Stable across
    /// re-deliveries — the tuple id survives lease expiry and a WAL restart — which is what makes
    /// the second apply a replay of the first rather than a new operation.
    pub fn operation_id(&self, stage: &str, id: u64) -> OperationId {
        OperationId::new(format!("tuple/{}/{}/{}", self.namespace, stage, id))
    }

    /// Take one item from `stage`, commit its effect, then acknowledge it.
    ///
    /// Returns once the item is acknowledged. The effect's receipt is in [`Consumed::receipt`]; a
    /// re-delivered item whose effect had already been committed comes back `Replayed`.
    pub async fn consume_one(
        &self,
        stage: &str,
        take_timeout: Duration,
        apply_deadline: Duration,
    ) -> Result<Consumed, ConsumeError> {
        let (id, payload) = self.space.take(stage, take_timeout).await.map_err(ConsumeError::Tuple)?;
        let receipt = self.commit(stage, id, &payload, apply_deadline).await?;
        self.space.ack(id).await.map_err(ConsumeError::Tuple)?;
        Ok(Consumed { id, receipt })
    }

    /// Take one item from `stage`, commit its effect, then **advance** it: acknowledge and put
    /// `next_payload(&payload)` on `next_stage` in one WAL record at the primary.
    ///
    /// The destination's receipt still comes first — `complete` is the pipeline's receipt and never
    /// stands in for it.
    pub async fn consume_one_advancing(
        &self,
        stage: &str,
        next_stage: &str,
        take_timeout: Duration,
        apply_deadline: Duration,
        next_payload: impl FnOnce(&Bytes) -> Bytes,
    ) -> Result<Consumed, ConsumeError> {
        let (id, payload) = self.space.take(stage, take_timeout).await.map_err(ConsumeError::Tuple)?;
        let receipt = self.commit(stage, id, &payload, apply_deadline).await?;
        let next = next_payload(&payload);
        self.space.complete(id, next_stage, next).await.map_err(ConsumeError::Tuple)?;
        Ok(Consumed { id, receipt })
    }

    /// Commit one item's effect under its operation id, with a fresh attempt id per delivery.
    async fn commit(
        &self,
        stage: &str,
        id: u64,
        payload: &Bytes,
        apply_deadline: Duration,
    ) -> Result<DestinationCommit, ConsumeError> {
        let op = self.operation_id(stage, id);
        // Fresh per delivery: a re-delivered item is a *new attempt* of the *same operation*, and
        // two deliveries that shared an attempt id would be indistinguishable afterwards.
        let effect = Effect::new(op.clone(), AttemptId::fresh(&op), payload.to_vec());
        apply_within(Arc::clone(&self.destination), effect, apply_deadline)
            .await
            .map_err(ConsumeError::Effect)
    }
}
