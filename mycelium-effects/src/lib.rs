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

#![deny(unsafe_code)]

pub mod counting;
pub mod sqlite;
#[cfg(feature = "tuple-space")]
pub mod tuple_consumer;

pub use mycelium::{AttemptId, DedupOutcome, DestinationCommit, OperationId, content_hash};
pub use sqlite::SqliteDestination;
pub use counting::{Counting, RefusalCounts, RefusalSnapshot};

use std::sync::Arc;
use std::time::Duration;

pub use mycelium::mandate::{Mandate, MandateRefusal, PrincipalId, ResourceAuthority, TermId};

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
///
/// `#[non_exhaustive]` since the composed path: a `_` arm must **fail closed** — an unrecognised
/// refusal is *not committed*, never *retry until it lands*.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The effect's **composition** did not hold at the destination
    /// (`docs/design/composed-effect.md` §9): `leg` names which of *attributed* or *authorised*
    /// failed, and `reason` says why in the mandate contract's own words. **Nothing was applied
    /// and no dedup row exists.** A caller must treat this as a denial, never as a retryable
    /// fault — retrying an unauthorised effect until it lands is the laundering a revocation
    /// exists to stop.
    Unauthorised { leg: CompositionLeg, reason: String },
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
            Self::Unauthorised { leg, reason } => {
                write!(f, "the effect's composition did not hold ({leg}): {reason}; nothing was applied")
            }
        }
    }
}
impl std::error::Error for EffectRefusal {}

/// Which leg of the composed guarantee a destination refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositionLeg {
    /// The principal is not the mandate's holder: the effect is presented under someone else's
    /// authority.
    Attribution,
    /// The mandate does not authorise this operation at this resource now — a superseded epoch,
    /// the wrong scope, outside its window, or the operation not enumerated.
    Authority,
}

impl std::fmt::Display for CompositionLeg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Attribution => "attribution",
            Self::Authority => "authority",
        })
    }
}

/// What a **composed** effect carries beside its bytes: who it is attributed to, under what
/// authority, for which operation, and from which domain — the legs the evidence record
/// reconstructs after the fact (`composed-effect.md` §3), presented *before* the commit so a
/// destination can refuse rather than merely record.
///
/// The destination enforces two: the effect is **attributed** (the principal is the mandate's
/// holder) and **authorised** (`ResourceAuthority::check` accepts the mandate for this operation at
/// this resource now). The **domain** leg is carried, not re-verified — a destination holds no
/// trust bundle, so it records the origin the gateway established, and the record says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composition {
    /// The verified principal the gateway resolved (`ActionEnvelope::actor`).
    pub principal: PrincipalId,
    /// The operation the mandate must enumerate (`ActionEnvelope::operation`).
    pub operation: String,
    /// The mandate presented for it — carried to the resource so it can verify for itself.
    pub mandate: Mandate,
    /// The origin domain when the caller came through a federated edge; `None` is local.
    pub origin_domain: Option<String>,
}

#[cfg(feature = "envelope")]
impl Composition {
    /// Build a composition **at a provider**, from the caller context its tool handler already
    /// receives (`RequestPrincipal::Client(caller)`): the gateway's verified principal, the mandate
    /// the caller carried (`GatewayCaller::mandate`, *carried, not verified* — the destination
    /// verifies it for itself, which is the point), and this call's operation and resource in the
    /// form a grant must enumerate. A caller that carried no mandate cannot compose an effect, and
    /// the error says so rather than composing one without authority.
    ///
    /// `operation` and `resource` are what the provider derives for the call — for an MCP tool,
    /// `tools/call` and `tool:{name}@{this node}` — the same derivation the provider's own
    /// enforcement point uses, so the destination and both enforcement points check one string.
    pub fn from_caller(
        caller: &mycelium::GatewayCaller,
        operation: &str,
        resource: &str,
        origin_domain: Option<String>,
    ) -> Result<Self, String> {
        Self::from_carried(&caller.principal, caller.mandate.as_ref(), operation, resource, origin_domain)
    }

    /// [`from_caller`](Self::from_caller) for the whole call a tool handler receives
    /// (`register_mcp_tool_with_call`): works on the gateway path **and** on a member's own mandated
    /// direct call, because [`McpCall`](mycelium::McpCall) keeps the carried mandate either way and
    /// names the principal as a mandate does (`node:{id}` for a member acting for itself).
    pub fn from_call(
        call: &mycelium::McpCall,
        operation: &str,
        resource: &str,
        origin_domain: Option<String>,
    ) -> Result<Self, String> {
        Self::from_carried(&call.principal_string(), call.carried_mandate.as_ref(), operation, resource, origin_domain)
    }

    /// [`from_caller`](Self::from_caller) over its pieces — the verified principal and the carried
    /// mandate as the caller context holds it — for a provider that has them apart, and for tests:
    /// a `GatewayCaller` is built only by the verifying receive path, which is right, so this is the
    /// form that can be exercised without one.
    pub fn from_carried(
        principal: &str,
        carried_mandate: Option<&serde_json::Value>,
        operation: &str,
        resource: &str,
        origin_domain: Option<String>,
    ) -> Result<Self, String> {
        let principal = PrincipalId::new(principal).ok_or("the caller context names no principal")?;
        let Some(raw) = carried_mandate else {
            return Err("the caller carried no mandate — an effect cannot be composed without authority".into());
        };
        let presented: mycelium::PresentedMandate =
            serde_json::from_value(raw.clone()).map_err(|e| format!("the carried mandate does not parse: {e}"))?;
        Ok(Self {
            principal,
            operation: mycelium::mandate_operation(operation, resource),
            mandate: presented.grant.mandate,
            origin_domain,
        })
    }
}

/// An effect with its composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedEffect {
    pub effect: Effect,
    pub composition: Composition,
}

/// The composition check, pure: attribution first, then the mandate contract's own check
/// ([`ResourceAuthority::check`]). The order is the reason a caller sees: an effect presented under
/// someone else's mandate is an attribution failure whatever that mandate would have permitted.
pub fn check_composition(
    composition: &Composition,
    authority: &ResourceAuthority,
    now_ms: u64,
) -> Result<(), EffectRefusal> {
    if composition.mandate.holder != composition.principal {
        return Err(EffectRefusal::Unauthorised {
            leg: CompositionLeg::Attribution,
            reason: format!(
                "the effect is attributed to {:?} but the mandate is held by {:?}",
                composition.principal.as_str(),
                composition.mandate.holder.as_str()
            ),
        });
    }
    authority
        .check(&composition.mandate, &composition.operation, now_ms)
        .map_err(|e: MandateRefusal| EffectRefusal::Unauthorised { leg: CompositionLeg::Authority, reason: e.to_string() })
}

#[cfg(feature = "envelope")]
impl Composition {
    /// Build a composition from what the gateway already established — the envelope's verified
    /// actor, its operation and resource (the operation a grant must enumerate is
    /// `{operation}:{resource_key}`, as the gateway's own mandate assessment computes it) — and the
    /// mandate the caller presented, so a provider that commits at a destination hands over the
    /// gateway's facts rather than assembling them by hand.
    ///
    /// Refused before any check when the envelope was assembled under a **different** binding than
    /// the presented grant (holder, term, scope or epoch differ): the two would then describe two
    /// appointments, and a composition must describe one.
    pub fn from_envelope(
        envelope: &mycelium::ActionEnvelope,
        presented: &mycelium::PresentedMandate,
        origin_domain: Option<String>,
    ) -> Result<Self, String> {
        let principal = PrincipalId::new(&envelope.actor).ok_or("the envelope names no actor")?;
        let mandate = presented.grant.mandate.clone();
        if let Some(b) = &envelope.mandate {
            if b.holder != mandate.holder {
                return Err(format!("the envelope is bound to holder {:?}, the presented grant to {:?}", b.holder.as_str(), mandate.holder.as_str()));
            }
            if b.term != mandate.term {
                return Err(format!("the envelope is bound to term {:?}, the presented grant to {:?}", b.term.as_str(), mandate.term.as_str()));
            }
            if b.scope != mandate.scope {
                return Err(format!("the envelope is bound to scope {:?}, the presented grant to {:?}", b.scope, mandate.scope));
            }
            if b.epoch != mandate.epoch {
                return Err(format!("the envelope is bound to epoch {}, the presented grant to {}", b.epoch, mandate.epoch));
            }
        }
        Ok(Self {
            principal,
            operation: mycelium::mandate_operation(&envelope.operation, &envelope.resource),
            mandate,
            origin_domain,
        })
    }
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
